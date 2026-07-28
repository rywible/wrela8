//! Boot-init argument materialization and `RuntimeWiring`.

use std::collections::BTreeMap;

use crate::eval::image::ImageGraph;
use crate::eval::value::Value;
use crate::flowwir::{AwaitKind, FlowInst, FlowWirProgram, Transition};
use crate::mwir::LayoutCtx;
use crate::sema::typed::TypedProgram;
use crate::syntax::ast::Module;

use super::place::place_runtime_tables;
use super::rtdata::{
    ActorRuntimeLayout, DriverRuntimeLayout, RingKind, RingLayout, RuntimeTables, TurnId,
    actor_method_index_tables, compute_runtime_tables, count_with_group_sites, mailbox_root_names,
    merge_actor_pub_methods,
};
use super::{
    DeviceRegs, GroupServiceCtx, IrqHostInject, IrqVectorEntry, LayoutError, PoolPlacement,
    WakeDrainEntry, append_rodata, build_irq_host_injects, checkpoint_irq_shape,
    closure_imported_types, closure_layout_types, cross_core_rings, driver_declares_task,
    driver_task_method_names, group_service_shape, reject_unlowerable_cross_core_shapes,
};

/// plans/M7.md item W: every struct's own declared `init`, in the shape
/// boot needs to *call* it — the `program.fns` key, the declared
/// parameter list (name, access mode, declared type, declaration order)
/// and the declared return type. `build_boot_init_calls` (below) turns
/// one of these plus one `ActorDecl`'s own wiring arguments into the
/// argument words `build_boot_init` loads into `x1..`.
///
/// What this replaced, recorded because it was a *rejection* and is not
/// one any more: until item W this returned only which structs declared
/// a **zero-argument** `init` — the one shape a boot sequence with no
/// argument marshalling could call — plus the parameter count of every
/// other one, purely so `RuntimeWiring::derive` could refuse to lay the
/// image out at all. That guard existed because the two halves of the
/// rule had silently composed into a wrong answer: `eval::image_checks`
/// accepts `depth=7` (it really does name a real `Sink.init` parameter),
/// boot never called that `init`, and the actor booted with `depth == 0`
/// while every assertion over it read 0 and all three tiers reported
/// success. Boot now calls a declared `init` with its declared
/// arguments, so the guard is gone; what remains fails closed on the
/// *specific shape it cannot marshal*, named one at a time in
/// `build_boot_init_calls`, never on "declares parameters at all".
pub(crate) struct ActorInit {
    /// `"{Struct}.init"` — `lower::lower_struct`'s own key for the
    /// compiled body, which is what `Asm::bl_call_key` resolves against.
    pub(crate) key: String,
    pub(crate) params: Vec<crate::sema::types::DeclParam>,
    pub(crate) ret: crate::sema::types::Type,
}

pub(crate) fn actor_inits(
    modules: &BTreeMap<String, Module>,
) -> Result<BTreeMap<String, ActorInit>, LayoutError> {
    use crate::sema::types::{DeclItem, DeclMember};

    let imported = closure_imported_types(modules)
        .map_err(|e| LayoutError::new(format!("actor boot init: {}", e.message)))?;
    let mut out: BTreeMap<String, ActorInit> = BTreeMap::new();
    for (addr, module) in modules {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("actor boot init: {}", e.message)))?;
        let items = crate::sema::types::declare_with_imports(&specialized, &imported[addr])
            .map_err(|e| LayoutError::new(format!("actor boot init: {}", e.message)))?;
        for item in items {
            let DeclItem::Struct(s) = item else { continue };
            for m in &s.members {
                if let DeclMember::Init(f) = m {
                    out.insert(
                        s.name.clone(),
                        ActorInit {
                            key: format!("{}.init", s.name),
                            params: f.params.clone(),
                            ret: f.ret.clone(),
                        },
                    );
                }
            }
        }
    }
    Ok(out)
}

/// One declared actor instance's own boot-time `init` call: the compiled
/// body's key and its already-materialized argument words, in declared
/// parameter order. `build_boot_init` loads word `i` into `x{i+1}`.
///
/// **The ABI is not restated here, it is derived**: `codegen::emit_prologue`
/// spills the receiver from `x0` into the frame's `self_ptr` slot (a
/// pointer — the receiver is always by address), then walks `f.params` in
/// declaration order spilling each one from the next register up, by
/// value for a non-aggregate and by address for an aggregate
/// (`codegen::is_aggregate`), and refuses past `x8`. `codegen`'s own
/// `Inst::Call` emitter is the mirror image of that, and
/// `build_rt_select_and_run_core`'s hand-assembled dispatch already
/// relies on the `x0`-is-`self`-pointer half (`a.load_imm(0, addrs.state)`
/// before every method call). Boot is a third caller of the identical
/// convention: `x0` = the actor's own state address, `x1..` = the
/// scalar arguments. Aggregates are not passed at all — they would need
/// a staging buffer boot has nowhere to put — so `boot_init_arg_word`
/// fails closed on every one of them instead.
#[derive(Debug)]
pub(crate) struct BootInitCall {
    pub(crate) key: String,
    pub(crate) args: Vec<BootInitArg>,
    /// plans/M7.md item E1: `true` when `init` returns
    /// `Result[unit, BootError]` — boot must arm `x8` with a reply slot
    /// and abort on `Err`.
    pub(crate) fallible: bool,
    /// When `fallible`: `(rodata_byte_offset, len)` of the abort message
    /// `"{key} returned Err"`, interned once before either assembly pass
    /// so the sizing and real-address builds agree on every word. `None`
    /// until `intern_fallible_init_abort_messages` runs (and forever for
    /// an infallible `init`).
    pub(crate) err_msg: Option<(usize, usize)>,
}

/// One materialized `init` argument word — or the promise of one whose
/// value is not known until the section table is placed.
///
/// plans/M7.md item H1: a `DeviceCap[D]` argument is decision 11's own
/// representation, the base address of the device's declared register
/// window, and that address exists only after `place_device_regs` has run.
/// Every other argument is already a build-time constant by the time
/// `build_boot_init_calls` sees it. Carrying the one unresolved case as
/// its own variant (rather than threading placement through the whole
/// derivation, or patching a word after the fact) keeps the emitted word
/// count identical in both of `build_runtime_block`'s passes — a
/// `load_imm` is four words whatever it loads — which is the invariant
/// both image flavors' two-pass assembly rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BootInitArg {
    Word(u64),
    /// The base of `ImageGraph::devices[.0]`'s own register window.
    DeviceRegsBase(usize),
    /// The base of the named pool's own backing in `pooldata` — decision
    /// 11's word for a `DmaPool[P, N]`, which item D placed and reported
    /// but nothing could yet pass to an `init`.
    PoolBase(String),
    /// plans/M7.md item E4 / decision 19: one `own[P] T` — the guest
    /// address of slot `index` in pool `name` (`base + index * slot_bytes`).
    OwnSlot {
        pool: String,
        index: u64,
        slot_bytes: u64,
    },
    /// plans/M7.md item E4: `[own[P] T; N]` — boot builds a table of
    /// `count` slot addresses on its own stack and passes the table's
    /// address (this machine's bare-pointer aggregate ABI).
    OwnHandleArray {
        pool: String,
        count: u64,
        slot_bytes: u64,
    },
}

impl BootInitArg {
    /// The word this argument actually loads. `regs`/`pools` are the
    /// placed window lists; a reference with no matching placement is an
    /// internal inconsistency (both parameters only exist on a driver
    /// whose binding `eval::image_checks` already resolved), reported
    /// rather than silently zeroed.
    /// Kept for unit tests / future inject-time validation; specialized
    /// emit uses Reloc variants (decision 683) instead.
    #[allow(dead_code)]
    pub(crate) fn resolve(
        &self,
        regs: &[DeviceRegs],
        pools: &[PoolPlacement],
    ) -> Result<u64, LayoutError> {
        match self {
            BootInitArg::Word(w) => Ok(*w),
            BootInitArg::DeviceRegsBase(i) => regs
                .iter()
                .find(|r| r.device == *i)
                .map(|r| r.base)
                .ok_or_else(|| {
                    LayoutError::new(format!(
                        "internal error: boot passes a `DeviceCap` for device#{i}, which has no \
                         placed register window"
                    ))
                }),
            BootInitArg::PoolBase(name) => pools
                .iter()
                .find(|p| &p.backing.name == name)
                .map(|p| p.base)
                .ok_or_else(|| {
                    LayoutError::new(format!(
                        "internal error: boot passes a `DmaPool` for pool `{name}`, which has no \
                         placed backing"
                    ))
                }),
            BootInitArg::OwnSlot {
                pool,
                index,
                slot_bytes,
            } => {
                let p = pools
                    .iter()
                    .find(|p| &p.backing.name == pool)
                    .ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: boot passes an `own` into pool `{pool}`, which has no \
                             placed backing"
                        ))
                    })?;
                Ok(p.base + *index * *slot_bytes)
            }
            BootInitArg::OwnHandleArray { .. } => Err(LayoutError::new(
                "internal error: `OwnHandleArray` has no single resolve word — emit via \
                 `emit_boot_init_arg`"
                    .to_string(),
            )),
        }
    }
}

/// The one place a build-time `eval::value::Value` becomes the 64-bit
/// word boot loads into an argument register. Deliberately exhaustive and
/// deliberately narrow: `None` means "this compiler has no register
/// representation for this value", and the caller turns that into a named
/// diagnostic rather than into a zero.
///
/// The encodings are `codegen`'s own, not new ones — an integer is
/// `Inst::ConstInt`'s `load_imm(value as i64)` (a negative value is
/// therefore its sign-extended two's complement, exactly as a compiled
/// `-5` would be), a bool is `Inst::ConstBool`'s 0/1, a char is
/// `Inst::ConstChar`'s code point, and `unit` is all-zero (the same fact
/// `build_boot_init`'s own zero-fill already rests on).
///
/// Counts that define the shared image-declaration handle space
/// (plans/M8.md item H attack 6). Derived once from the sealed graph so
/// every consumer (`boot_init_arg_word`, `resolve_runtime_test_args`)
/// sees the same shift.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HandleSpace {
    pub(crate) n_actors: usize,
    pub(crate) n_drivers: usize,
}

impl HandleSpace {
    pub(crate) fn from_graph(graph: &ImageGraph) -> Self {
        Self {
            n_actors: graph.actors.len(),
            n_drivers: graph.drivers.len(),
        }
    }
}

/// **Contract: no two distinct image declarations share a handle word,
/// whatever their kind.** Every `ImageDeclRef` variant is named here so a
/// fourth kind cannot quietly reuse a number — it either gets a fresh
/// range or fails closed like a pool.
///
/// Layout (dumb, deterministic, actors-then-drivers-then-devices):
/// - `Actor(i)`  → `i`
/// - `Driver(i)` → `n_actors + i`
/// - `Device(i)` → `n_actors + n_drivers + i`
/// - `Pool` / `DmaPool` → no word (`None`); they are named by string, not
///   indexed (`ImageDeclRef`'s own two recording disciplines).
///
/// Why one space for all three indexed kinds: `decl.handle()` erases the
/// declaration's type into a bare `u32` (`check_image_decl_method_intrinsic`
/// accepts it on any `ImageDecl`), so a kind-local scheme for devices is
/// the same shape of hole item D decision 22 left for actors/drivers —
/// found by orchestrator spot-probe after the first attack-6 fix covered
/// only two of the three kinds.
pub(crate) fn image_decl_handle_word(
    space: HandleSpace,
    decl: &crate::eval::image::ImageDeclRef,
) -> Option<u64> {
    use crate::eval::image::ImageDeclRef;
    match decl {
        ImageDeclRef::Actor(i) => Some(*i as u64),
        ImageDeclRef::Driver(i) => Some((space.n_actors + *i) as u64),
        ImageDeclRef::Device(i) => Some((space.n_actors + space.n_drivers + *i) as u64),
        ImageDeclRef::Pool(_) | ImageDeclRef::DmaPool(_) => None,
    }
}

/// A declaration handle (`Value::ImageDecl`) becomes its word in the
/// shared space (`image_decl_handle_word`) — the identical number
/// `resolve_runtime_test_args` hands a `@test(runtime)` root for an
/// `Actor[T]` parameter. **What that number is and is not**: `codegen`
/// still routes every `await`/`send` statically by actor type today, but
/// the guest can store and compare the word (and the day handles become
/// dynamic this is the one place that has to change). A pool reference
/// is named by a string, not an index, so it has no word and fails closed.
pub(crate) fn boot_init_arg_word(
    value: &crate::eval::value::Value,
    space: HandleSpace,
) -> Option<u64> {
    Some(match value {
        Value::U8(n) => *n as u64,
        Value::U16(n) => *n as u64,
        Value::U32(n) => *n as u64,
        Value::U64(n) | Value::Usize(n) => *n,
        Value::I8(n) => *n as i64 as u64,
        Value::I16(n) => *n as i64 as u64,
        Value::I32(n) => *n as i64 as u64,
        Value::I64(n) | Value::Isize(n) => *n as u64,
        Value::Bool(b) => u64::from(*b),
        Value::Char(c) => *c as u32 as u64,
        Value::Unit => 0,
        Value::ImageDecl(decl) => return image_decl_handle_word(space, decl),
        // Every remaining shape is either an aggregate (no register
        // representation: `Struct`/`Tuple`/`Array`/`Enum`/`Str`/`Bytes`),
        // a float (`codegen` has no FP/SIMD encoder subset at all —
        // `Inst::ConstFloat` fails closed for the identical reason), or a
        // callable (`Fn`/`Closure` — not a value this machine passes).
        Value::F32(_)
        | Value::F64(_)
        | Value::Str(_)
        | Value::Bytes(_)
        | Value::Tuple(_)
        | Value::Array(_)
        | Value::Struct(_)
        | Value::Enum(_, _)
        | Value::Fn(_)
        | Value::Closure { .. } => return None,
    })
}

/// A `Value`'s own shape, for a diagnostic that has to name what it
/// could not marshal without printing the whole value.
fn value_shape_name(value: &crate::eval::value::Value) -> &'static str {
    use crate::eval::image::ImageDeclRef;

    match value {
        Value::U8(_)
        | Value::U16(_)
        | Value::U32(_)
        | Value::U64(_)
        | Value::Usize(_)
        | Value::I8(_)
        | Value::I16(_)
        | Value::I32(_)
        | Value::I64(_)
        | Value::Isize(_) => "an integer",
        Value::Bool(_) => "a bool",
        Value::Char(_) => "a char",
        Value::Unit => "unit",
        Value::F32(_) | Value::F64(_) => "a floating-point value",
        Value::Str(_) => "a string",
        Value::Bytes(_) => "a byte string",
        Value::Tuple(_) => "a tuple",
        Value::Array(_) => "an array",
        Value::Struct(_) => "a struct value",
        Value::Enum(_, _) => "an enum value",
        Value::Fn(_) => "a function reference",
        Value::Closure { .. } => "a closure",
        Value::ImageDecl(ImageDeclRef::Device(_)) => "a device handle",
        Value::ImageDecl(ImageDeclRef::Driver(_)) => "a driver handle",
        Value::ImageDecl(ImageDeclRef::Actor(_)) => "an actor handle",
        Value::ImageDecl(ImageDeclRef::Pool(_)) => "a pool handle",
        Value::ImageDecl(ImageDeclRef::DmaPool(_)) => "a DMA-pool handle",
    }
}

/// **plans/M7.md item W's residual, closed here** (item W named item D as
/// its owner; this is that).
///
/// The residual, in item W's own words: "A handle wired through an `init`
/// *parameter* now carries its real construction index; a handle wired
/// straight to a *field* (05-library.md §9's literal-constructor path,
/// which has no `init` to call) still arrives as the zero the state-fill
/// leaves. The two paths genuinely disagree. ... the fix is either
/// materializing into the field's own offset with W's marshalling or
/// failing closed on a nonzero index."
///
/// **Failing closed is the option taken**, for three reasons stated once
/// so the choice is reviewable rather than inferred:
///
/// 1. It is *exact*, not conservative. A field-wired argument whose boot
///    word is `0` agrees with the state-fill byte for byte — there is no
///    disagreement to close, which is why `golden/image-field-wired-accept`
///    (`led=<actor#0>`) stays green untouched. Every word that is not `0`
///    is a wrong answer today, and every one of them is now rejected. The
///    two paths therefore never disagree again, which is the whole
///    property the residual asked for.
/// 2. Materializing would build a mechanism with no consumer.
///    `codegen` routes every `await`/`send` statically, by actor *type*
///    (`codegen::rt_enqueue_symbol`), so nothing reads a handle word at
///    runtime; storing one into a field offset would be an unobservable
///    write, and the house rule is that a feature waits for the thing that
///    needs it. When an `Actor[T]` becomes comparable, storable or
///    sendable, this rejection is what fails and points at the work.
/// 3. It generalizes correctly rather than only to handles. Item W found
///    the defect through a handle, but a *scalar* wired to a field of a
///    no-`init` struct is silently zero for exactly the same reason
///    (`img.actor(Store, seed=8)` against `Store.seed: u32`, no `init` —
///    accepted by `eval::image_checks`' literal-constructor arm, never
///    materialized by anything). "Nonzero index" and "nonzero value" are
///    one rule: *a field-wired argument must equal the zero the state-fill
///    leaves*. A value with no register representation at all is rejected
///    too, since this compiler cannot show it is zero either.
/// Image-wiring labels that are never field/init arguments — the same
/// sets `eval::image_checks::reserved_args` uses, restated here by `kind`
/// because that helper is not `pub` and `is_reserved_actor_arg` only
/// covers the actor half. Sharing the actor predicate for drivers would
/// let `device=` fall through as a field wire (and, once device handles
/// left word 0, fail closed against zero-fill — the exact break
/// `check-driver-mode-irq`/`-poll` hit under attack 6's device half).
fn is_reserved_wiring_arg(kind: &str, label: &str) -> bool {
    match kind {
        "driver" => matches!(label, "device" | "core" | "mailbox"),
        "actor" => crate::eval::image_checks::is_reserved_actor_arg(label),
        _ => false,
    }
}

fn check_field_wired_args(
    kind: &str,
    name: &str,
    decl_args: &[crate::eval::image::DeclArg],
    space: HandleSpace,
) -> Result<(), LayoutError> {
    for a in decl_args {
        if is_reserved_wiring_arg(kind, &a.label) {
            continue;
        }
        let word = boot_init_arg_word(&a.value, space);
        if word == Some(0) {
            continue;
        }
        let what = match word {
            Some(w) => format!("materializes as {w}"),
            None => format!(
                "is {} and has no register representation at all",
                value_shape_name(&a.value)
            ),
        };
        return Err(LayoutError::new(format!(
            "{kind} `{name}` declares no `init`, so this image wires `{}=...` to its field of \
             that name — and boot has nothing to call: it zero-fills the whole state slot and \
             stops (05-library.md §9's literal-constructor path). The wired value {what}, which \
             is not the zero the state-fill leaves, so the {kind} would boot with a value this \
             image did not declare. Failing closed (plans/M7.md item W's residual, owned by item \
             D) rather than reporting success over a wrong answer. Give `{name}` an `init` that \
             takes it, or drop the argument.",
            a.label
        )));
    }
    Ok(())
}

/// plans/M7.md item W: every declared actor *and driver* instance's own
/// boot `init` call, in `graph.actors`/`graph.drivers` order — which is
/// `RuntimeTables::actors`/`RuntimeTables::drivers` order too
/// (`compute_runtime_tables` builds one entry per graph entry, in the same
/// walks), so each result indexes 1:1 against the matching
/// `RuntimePlacement` list.
///
/// **plans/M7.md item H1, decision 10's second prerequisite**: until this
/// item the walk was `graph.actors` only, so a `@driver`'s `init` was
/// never called at boot at all and no capability of any kind had bytes at
/// runtime. A driver's `init` is now called exactly like an actor's, with
/// one difference that is the whole point: its `DeviceCap[D]` parameter
/// carries no explicit image argument (05-library.md §9 substitutes it)
/// and is materialized as decision 11's own word — the base of the device
/// register window `place_device_regs` reserved for the device its
/// `device=` binding names.
///
/// `None` for an actor whose struct declares no `init` at all: its state
/// is its own literal constructor's, and `build_boot_init`'s zero-fill
/// already gave every field a defined value (`eval::image_checks`'s own
/// missing-slot note rests on exactly that).
///
/// Everything this cannot do fails closed with an ordinary named
/// diagnostic — never `internal error:` (that spelling is reserved for a
/// producer bug), and never a zero. The shapes, all of them:
///
/// - a **fallible `init`** (`-> Result[...]`): running one means driving
///   03-hardware.md §9's consuming transition chain from boot, which is
///   plans/M7.md item H's bring-up work. Until then a fallible `init`
///   fails closed rather than having its `Result` dropped on the floor.
///   Note this arm also closes a live defect that predates item W: a
///   *zero-argument* `init` returning a `Result` was called by the old
///   boot sequence with no `x8`, so the callee wrote its aggregate reply
///   through whatever `x8` happened to hold — a real guest fault at
///   `ipa=0x0`, verified by running it.
/// - any other **non-`unit` return**, for the same reason with no item to
///   name: boot has nowhere to put the value.
/// - a **capability parameter other than `DeviceCap[D]` on a `@driver`**
///   (recognized by name exactly as `eval::image_checks` recognizes them):
///   `eval::image_checks::check_capability_substitution` already refuses
///   each of these at the image binding, naming the item that mints it;
///   this is the same refusal for the shape that check cannot see (an
///   `img.actor(...)` naming a struct that is not an `@actor`).
/// - an **`Actor[T]` parameter with no explicit argument**: 05-library.md
///   §9 lets an actor handle be substituted by type rather than wired by
///   name, and boot materializes only what the image explicitly wired.
/// - a **pool handle argument** (05-library.md §9's "create the initial
///   handles", wired as `blocks=take cache_blocks`): plans/M7.md item E4
///   / decision 19 materializes each `own[P] T` as the guest address of
///   one pool slot. A single `own` is that word; an `[own; N]` is a
///   stack-built table of them passed by the bare-pointer aggregate ABI.
///   Any other parameter type wired from a pool still fails closed.
/// - an **argument whose value has no register representation**
///   (an aggregate, a float, ...).
/// - **more than eight arguments**, the register budget `x1..x8` leaves
///   once `x0` carries the receiver — `codegen`'s own identical limit.
pub(crate) fn build_boot_init_calls(
    graph: &ImageGraph,
    inits: &BTreeMap<String, ActorInit>,
    backings: &BTreeMap<String, crate::eval::image_checks::PoolBacking>,
) -> Result<(Vec<Option<BootInitCall>>, Vec<Option<BootInitCall>>), LayoutError> {
    let mut actors = Vec::with_capacity(graph.actors.len());
    for decl in &graph.actors {
        actors.push(one_boot_init_call(
            "actor",
            &decl.actor_type,
            &decl.args,
            None,
            graph,
            inits,
            backings,
        )?);
    }
    let mut drivers = Vec::with_capacity(graph.drivers.len());
    for decl in &graph.drivers {
        let device = device_index_of(&decl.args);
        drivers.push(one_boot_init_call(
            "driver",
            &decl.actor_type,
            &decl.args,
            device,
            graph,
            inits,
            backings,
        )?);
    }
    Ok((actors, drivers))
}

/// The `device#N` an `img.driver(..., device=...)` declaration binds, if
/// its `device=` argument is a device reference at all. `None` is never a
/// silent default: the one caller that needs it turns it into a named
/// rejection on the `DeviceCap[D]` parameter that would have used it.
pub(crate) fn device_index_of(args: &[crate::eval::image::DeclArg]) -> Option<usize> {
    use crate::eval::image::ImageDeclRef;
    args.iter()
        .find(|a| a.label == "device")
        .and_then(|a| match &a.value {
            Value::ImageDecl(ImageDeclRef::Device(i)) => Some(*i),
            _ => None,
        })
}

/// One declaration's own boot `init` call. `kind` is the noun every
/// diagnostic here uses (`actor`/`driver`) so a driver is never described
/// as an actor and vice versa; `device` is the driver's own bound device
/// index (`None` for an actor, which binds none).
fn one_boot_init_call(
    kind: &str,
    decl_type: &crate::sema::types::Type,
    decl_args: &[crate::eval::image::DeclArg],
    device: Option<usize>,
    graph: &ImageGraph,
    inits: &BTreeMap<String, ActorInit>,
    backings: &BTreeMap<String, crate::eval::image_checks::PoolBacking>,
) -> Result<Option<BootInitCall>, LayoutError> {
    use crate::sema::types::{Type, render_type};

    let name = render_type(decl_type);
    let space = HandleSpace::from_graph(graph);
    let Some(init) = inits.get(&name) else {
        check_field_wired_args(kind, &name, decl_args, space)?;
        return Ok(None);
    };
    if init.ret != Type::Unit {
        let rendered = render_type(&init.ret);
        // plans/M7.md item E1: a fallible `init` returning
        // `Result[unit, BootError]` is now real — 03 §1's own constructor
        // signature. Boot allocates a reply slot, calls `init`, and on
        // `Err` aborts with a diagnosable line (plans/M6.md decision 12 /
        // plans/M7.md decision 8). Any other non-`unit` return still fails
        // closed: boot has nowhere to put the value.
        let ok_fallible = matches!(
            &init.ret,
            Type::Result(ok, err)
                if matches!(ok.as_ref(), Type::Unit)
                    && matches!(err.as_ref(), Type::Named(n, _) if n == "BootError")
        );
        if !ok_fallible {
            return Err(LayoutError::new(if matches!(init.ret, Type::Result(..)) {
                format!(
                    "{kind} `{name}` declares a fallible `init` returning `{rendered}`, and this \
                     image declares an instance of it — boot can only handle \
                     `Result[unit, BootError]` (03-hardware.md §1/§9); any other error type \
                     would need a recovery path this machine does not have yet"
                )
            } else {
                format!(
                    "{kind} `{name}` declares `init` returning `{rendered}`, and this image \
                     declares an instance of it — boot can only call an `init` returning \
                     `unit` or `Result[unit, BootError]`, and has nowhere to put a returned value."
                )
            }));
        }
    }
    if init.params.len() > 8 {
        return Err(LayoutError::new(format!(
            "{kind} `{name}`'s own `init` declares {} parameters; boot can pass at most 8 \
             (`x0` carries the receiver, leaving `x1..x8`) — the identical limit \
             `codegen` places on every other call.",
            init.params.len()
        )));
    }
    let mut args = Vec::with_capacity(init.params.len());
    for p in &init.params {
        // Reserved labels are skipped through the same predicate
        // `eval::image_checks` accepts them by, so the acceptance rule
        // and this materialization rule can never disagree about which
        // label is image-wiring metadata rather than an `init`
        // argument (a parameter that happens to be named `mailbox` is
        // therefore unsatisfiable on both sides alike, not satisfiable
        // on one).
        let wired = decl_args.iter().find(|a| {
            a.label == p.name && !crate::eval::image_checks::is_reserved_actor_arg(&a.label)
        });
        let Some(a) = wired else {
            let param_ty = render_type(&p.ty);
            if let Type::Named(tn, targs) = &p.ty {
                // plans/M7.md item H1: **the mint, materialized.** A
                // `@driver`'s `DeviceCap[D]` parameter carries no explicit
                // argument — 05-library.md §9 substitutes it from the
                // `device=` binding, and `check_capability_substitution`
                // has already checked that `D` *is* the device that
                // binding names. Its word is decision 11's: the base of
                // that device's own declared register window.
                if tn == "DeviceCap" {
                    let Some(i) = device else {
                        return Err(LayoutError::new(format!(
                            "{kind} `{name}`'s own `init` takes `{}: {param_ty}`, but this \
                             declaration binds no device — a `DeviceCap[D]` is authority over one \
                             device instance and is minted only from an `img.driver(..., \
                             device=...)` binding (03-hardware.md §1).",
                            p.name
                        )));
                    };
                    args.push(BootInitArg::DeviceRegsBase(i));
                    continue;
                }
                // plans/M7.md item H1: a `DmaPool[P, N]` parameter is
                // substituted the same way, and its word is decision 11's
                // — the base of pool `P`'s own backing, which item D
                // already sized, placed and reported.
                // `check_dma_pool_mint` has already checked that `P` is
                // bound, DMA, reachable from *this* driver's device and at
                // least `N` bytes wide, so nothing is re-derived here.
                if tn == "DmaPool" {
                    let Some(crate::sema::types::TypeArg::Pool(pool)) = targs.first() else {
                        return Err(LayoutError::new(format!(
                            "internal error: `{name}.init`'s own `{}: {param_ty}` names no pool",
                            p.name
                        )));
                    };
                    args.push(BootInitArg::PoolBase(pool.clone()));
                    continue;
                }
                // plans/M7.md item G, decision 12: an `IrqCap[V]` parameter
                // is the vector bit index. `check_irq_cap_mint` already
                // required `vector=`; the word is known here, so it is a
                // plain `Word` rather than a reloc against placement.
                if tn == "IrqCap" {
                    let Some(i) = device else {
                        return Err(LayoutError::new(format!(
                            "{kind} `{name}`'s own `init` takes `{}: {param_ty}`, but this \
                             declaration binds no device — an `IrqCap[V]` is minted from a \
                             device's declared `vector=` (03-hardware.md §6).",
                            p.name
                        )));
                    };
                    let Some(dev) = graph.devices.get(i) else {
                        return Err(LayoutError::new(format!(
                            "internal error: `{name}.init` takes an `IrqCap` for device#{i}, \
                             which does not exist"
                        )));
                    };
                    let Some(v) = crate::eval::image_checks::device_vector(&dev.args) else {
                        return Err(LayoutError::new(format!(
                            "internal error: `{name}.init` takes an `IrqCap` for device#{i}, \
                             which declared no `vector=` — `check_vector_bindings` should have \
                             rejected first"
                        )));
                    };
                    args.push(BootInitArg::Word(v));
                    continue;
                }
                if crate::eval::image_checks::is_capability_type_name(tn) {
                    return Err(LayoutError::new(format!(
                        "{kind} `{name}`'s own `init` takes `{}: {param_ty}`, a capability this \
                         image never wires explicitly — the image binding mints a `DeviceCap[D]`, \
                         a `DmaPool[P, N]` and an `IrqCap[V]` (from `vector=`) and nothing else \
                         (plans/M7.md items H1/G); an `Mmio[L]` comes from the sealed transport's \
                         own `map_partition` (03-hardware.md §2/§9), and the rest are named by \
                         `eval::image_checks::check_capability_substitution`. Failing closed \
                         rather than passing a zero.",
                        p.name
                    )));
                }
                if crate::eval::image_checks::is_protocol_state_type_name(tn) {
                    return Err(LayoutError::new(format!(
                        "{kind} `{name}`'s own `init` takes `{}: {param_ty}`, a bring-up state \
                         (03-hardware.md §9). A state is produced by a transition inside the \
                         driver, never handed to it: boot mints the `DeviceCap[D]` and the \
                         driver's own `init` calls `claim`.",
                        p.name
                    )));
                }
                if crate::eval::image_checks::is_handle_type_name(tn) {
                    return Err(LayoutError::new(format!(
                        "{kind} `{name}`'s own `init` takes `{}: {param_ty}` with no \
                         `{}=...` argument in this image — 05-library.md §9 allows an actor \
                         handle to be substituted by type there, but boot materializes only \
                         the arguments the image wires by name. Wire it explicitly, or wait \
                         for handle substitution.",
                        p.name, p.name
                    )));
                }
            }
            return Err(LayoutError::new(format!(
                "{kind} `{name}`'s own `init` takes `{}: {param_ty}` and this image wires no \
                 `{}=...` argument for it, so boot has no value to pass.",
                p.name, p.name
            )));
        };
        // plans/M7.md item E4 / decision 19: a pool wired to an `own[P] T`
        // or `[own[P] T; N]` parameter becomes the initial handles
        // 05-library.md §9 promises. Each handle is one word — the guest
        // address of a pool slot. A single `own` is that word; an array
        // is a pre-built table of them passed by the bare-pointer
        // aggregate ABI.
        if matches!(
            a.value,
            crate::eval::value::Value::ImageDecl(
                crate::eval::image::ImageDeclRef::Pool(_)
                    | crate::eval::image::ImageDeclRef::DmaPool(_)
            )
        ) {
            let pool_name = match &a.value {
                crate::eval::value::Value::ImageDecl(
                    crate::eval::image::ImageDeclRef::Pool(n)
                    | crate::eval::image::ImageDeclRef::DmaPool(n),
                ) => n.clone(),
                _ => unreachable!(),
            };
            let backing = backings.get(&pool_name).ok_or_else(|| {
                LayoutError::new(format!(
                    "internal error: `{name}.init` wires pool `{pool_name}`, which has no \
                     PoolBacking — `check_pool_decls` should have rejected first"
                ))
            })?;
            match &p.ty {
                Type::Own(own_pool, _) if own_pool == &pool_name => {
                    if backing.slots < 1 {
                        return Err(LayoutError::new(format!(
                            "{kind} `{name}` wires `{}=...` to a single `own[{pool_name}] _`, but \
                             pool `{pool_name}` declares zero slots",
                            a.label
                        )));
                    }
                    args.push(BootInitArg::OwnSlot {
                        pool: pool_name,
                        index: 0,
                        slot_bytes: backing.slot_bytes,
                    });
                    continue;
                }
                Type::Array(elem, len_expr) => {
                    if let Type::Own(own_pool, _) = elem.as_ref() {
                        if own_pool == &pool_name {
                            let n = crate::sema::bodies::literal_array_len(len_expr).ok_or_else(
                                || {
                                    LayoutError::new(format!(
                                        "{kind} `{name}`'s own `{}: {}` has a non-literal array \
                                         length — boot can only materialize a fixed `[own; N]`",
                                        p.name,
                                        render_type(&p.ty),
                                    ))
                                },
                            )?;
                            if n as u64 != backing.slots {
                                return Err(LayoutError::new(format!(
                                    "{kind} `{name}` wires `{}=...` to `[own[{pool_name}] _; {n}]`, \
                                     but pool `{pool_name}` declares {} slots — 05-library.md §9's \
                                     initial handles are exactly one per slot",
                                    a.label, backing.slots
                                )));
                            }
                            args.push(BootInitArg::OwnHandleArray {
                                pool: pool_name,
                                count: backing.slots,
                                slot_bytes: backing.slot_bytes,
                            });
                            continue;
                        }
                    }
                }
                _ => {}
            }
            return Err(LayoutError::new(format!(
                "{kind} `{name}` wires `{}=...` to `{name}.init`'s own `{}: {}` from a declared \
                 pool. The pool is real, but that parameter is not an `own[{pool_name}] T` or \
                 `[own[{pool_name}] T; N]` — 05-library.md §9's \"create the initial handles\" \
                 only substitutes those shapes (plans/M7.md item E4 / decision 19).",
                a.label,
                p.name,
                render_type(&p.ty),
            )));
        }
        let Some(word) = boot_init_arg_word(&a.value, space) else {
            return Err(LayoutError::new(format!(
                "{kind} `{name}` wires `{}=...` to `{name}.init`'s own `{}: {}`, but the \
                 value is {} — boot passes arguments in registers (`x1..`), and this \
                 compiler has no register representation for that shape. Failing closed \
                 rather than passing a zero.",
                a.label,
                p.name,
                render_type(&p.ty),
                value_shape_name(&a.value)
            )));
        };
        args.push(BootInitArg::Word(word));
    }
    Ok(Some(BootInitCall {
        key: init.key.clone(),
        args,
        fallible: matches!(
            &init.ret,
            Type::Result(ok, err)
                if matches!(ok.as_ref(), Type::Unit)
                    && matches!(err.as_ref(), Type::Named(n, _) if n == "BootError")
        ),
        err_msg: None,
    }))
}

/// Intern one abort message per fallible `init` into `rodata`, recording
/// the offset/len on the call. Must run **once** before either of
/// `build_runtime_block`'s two assembly passes, so both see the same
/// offsets and emit the same word count.
///
/// Message shape matches an `assert` failure inside `init`: the harness
/// `__wrela_abort` prepends `FAILED `, so the interned text is just
/// `"{Actor}.init returned Err"` — the `@driver`/`@actor` struct name is
/// already in `BootInitCall::key`. The concrete `BootError` variant is
/// not recovered (would need a second formatting path over the reply
/// slot); named in the plan's Done prose rather than pretended.
pub(crate) fn intern_fallible_init_abort_messages(
    wiring: &mut RuntimeWiring,
    rodata: &mut Vec<Vec<u8>>,
    rodata_cursor: &mut usize,
) {
    for call in wiring
        .init_calls
        .iter_mut()
        .chain(wiring.driver_init_calls.iter_mut())
        .flatten()
    {
        if !call.fallible || call.err_msg.is_some() {
            continue;
        }
        let bytes = format!("{} returned Err", call.key).into_bytes();
        let len = bytes.len();
        let off = append_rodata(rodata, rodata_cursor, bytes);
        call.err_msg = Some((off, len));
    }
}

// plans/M10.md item H: `build_boot_init` / `emit_boot_init_arg` deleted —
// specialized `codegen::emit_boot_init` lives in `code` under
// `rt_boot_init 0` (decisions 680–684). `build_boot_init_calls` remains:
// it materializes the call specs inject_boot_init_fn consumes.

// ===========================================================================
// plans/M6.md item F/G follow-up (the found-and-fixed `layout_program`
// defect): the runtime machinery, derived and assembled **once**, for both
// image flavors.
//
// Until this landed, only `layout_test_image` built the per-actor
// `__rt_enqueue_*`/`rt_select_and_run` glue, the group-child poll routines,
// `rt_run_one` and the boot-init routine. `layout_program` — the path
// `wrela build`/`wrela dump --stage=report` take — reserved `rtdata` but
// emitted none of the code that addresses it, so the first `.wr` image that
// actually *messaged* an actor (any `await`/`send` through an `Actor[T]`
// handle, which codegen lowers to a `Reloc::Call` at the symbolic
// `codegen::rt_enqueue_symbol` name) died in reloc resolution with
// `internal error: call target `__rt_enqueue_X` was never codegen'd` — an
// internal-error guard on a plainly user-reachable path. `tests/golden/
// appliance` never caught it because its actors are declared and never
// messaged, so no such `Reloc::Call` is ever emitted.
//
// The rule (item C's own, restated): the runtime tables **and** the routines
// that address them are part of the image, tests or not. So both paths now
// derive their inputs through `RuntimeWiring::derive` and assemble the exact
// same words through `build_runtime_block`. The only thing that legitimately
// differs is the entry driver — `layout_test_image`'s real console harness +
// test roots vs. `layout_program`'s `build_entry_stub` placeholder, which
// still halts with `EXIT_CODE_NO_RUNTIME` and therefore never calls any of
// this. That the block is unreachable in a `wrela build` image today is the
// identical, already-recorded position `rtdata`'s own reservation takes
// (`layout_program`'s doc): it is *there* because it is part of the image,
// not because anything executes it yet. The moment a real non-test image
// entry exists (M7+), it is one `bl_to(boot_init_start)` away, byte-for-byte
// the same machinery `wrela test` already boots for real.

/// Every whole-build fact the runtime block needs, derived once from a
/// `BootCtx` so the two image flavors can never disagree about an actor's
/// dispatch keys, its `init`, or the group-child index. `None` means "this
/// build has no actor runtime at all" (no `@actor` declaration, or tables
/// that size to zero bytes) — the overwhelming majority of today's corpus,
/// for which both paths stay byte-identical to their pre-M6 behavior.
pub(crate) struct RuntimeWiring {
    pub(crate) tables: RuntimeTables,
    /// Per actor, in `tables.actors` order: its own name and its `pub`
    /// method dispatch keys (`"{Actor}.{method}"`, `program.fns` keys) with
    /// each one's asyncness and (plans/M7.md item Z1) whether its declared
    /// reply is an aggregate.
    pub(crate) dispatch: Vec<(String, Vec<(String, bool, bool)>)>,
    /// Per declared actor *instance*, in `tables.actors` order: the boot
    /// `init` call to make for it, or `None` if its struct declares no
    /// `init` at all (plans/M7.md item W, `build_boot_init_calls`).
    ///
    /// Per instance rather than per struct name, because the arguments
    /// come from the *declaration* (`ActorDecl::args`) and not from the
    /// struct: two `img.actor(Same, ...)` calls are two calls with two
    /// argument lists.
    pub(crate) init_calls: Vec<Option<BootInitCall>>,
    /// plans/M7.md item H1: the same, per declared `@driver` instance, in
    /// `tables.drivers` order.
    pub(crate) driver_init_calls: Vec<Option<BootInitCall>>,
    pub(crate) state_sizes: Vec<u64>,
    pub(crate) driver_state_sizes: Vec<u64>,
    pub(crate) group_child_index: BTreeMap<String, usize>,
    /// plans/M8.md item C1: each actor instance's own core, in
    /// `tables.actors` (= `ImageGraph::actors`) order — read straight off
    /// the report's own Placement table (`placement::place`), never
    /// re-derived here. Shape decision 2: the report's assignment *is* the
    /// runtime's assignment, or there are two truths.
    pub(crate) actor_cores: Vec<usize>,
    /// The whole placement table, kept for the cross-core edge check both
    /// image flavors run during reloc resolution.
    pub(crate) placement: crate::placement::PlacementTable,
    /// M11 I: IRQ handler stubs to overwrite (`handler_key`, `driver_state`).
    pub(crate) irq_calls: Vec<(String, u64)>,
    /// M11 I: wake `@task` stubs (`task_key`, `driver_state`).
    pub(crate) wake_calls: Vec<(String, u64)>,
}

impl RuntimeWiring {
    /// One derivation for both image flavors, with no flavor-conditional
    /// behavior in it at all. plans/M6.md item F/G's own found-and-fixed
    /// defect (this module's block comment above) is exactly that the
    /// runtime block **is** part of the image, tests or not, so the day
    /// `layout_program` grows a real entry it must find the identical boot
    /// sequence `wrela test` already boots. plans/M7.md item W removed the
    /// one exception that had grown back: a `reject_parameterized_init`
    /// flag, set only by `layout_test_image`, that made a parameterized
    /// `init` a build error on the path that boots and a silent no-op on
    /// the path that does not. Both paths now materialize the same
    /// arguments and fail closed on the same shapes.
    pub(crate) fn derive(boot: &BootCtx) -> Result<Option<RuntimeWiring>, LayoutError> {
        let group_max_children = crate::codegen::group_max_children_of(boot.group_child_index);
        let Some(mut tables) = compute_runtime_tables(
            boot.graph,
            boot.modules,
            boot.layout_ctx,
            boot.async_frames,
            group_max_children,
        )
        .map_err(LayoutError::new)?
        .filter(|t| t.total_bytes > 0) else {
            return Ok(None);
        };
        // plans/M8.md item C1: placement first — it decides how many cores
        // this image brings up, which stripes the scheduler tables before
        // anything is placed or emitted against them.
        let placement = crate::placement::place(
            boot.graph,
            boot.modules,
            boot.layout_ctx,
            boot.graph.cores,
        )
        .map_err(LayoutError::new)?;
        tables.stripe_for_cores(placement.cores);
        // plans/M8.md item C1's second fail-closed arm. A `@driver`'s ISR
        // and `@task` bottom half are emitted into the **checkpoint
        // service**, which only core 0's entry driver and core 0's compiled
        // code ever call; its `init` likewise runs in core 0's boot
        // sequence. So a driver inferred or annotated onto a secondary core
        // would be a second truth of exactly the shape shape decision 2
        // forbids — the report saying `core=2` while every one of that
        // driver's own instructions runs on core 0. 04 §3 is explicit
        // ("a `@driver`'s vectors, pools, permits, and recovery lanes live
        // on its core; there is no cross-core hardware state"), so this is
        // refused rather than approximated. Lifting it is item C2's
        // per-core checkpoint work, not a silent demotion here.
        for (i, d) in tables.drivers.iter().enumerate() {
            let core = placement
                .core_of(&crate::eval::image::ImageDeclRef::Driver(i))
                .unwrap_or(0);
            if core != 0 {
                return Err(LayoutError::new(format!(
                    "driver#{i} (`{}`) is placed on core {core}, but a `@driver`'s ISR, `@task` \
                     bottom half and boot `init` all run in core 0's checkpoint service and boot \
                     sequence — plans/M8.md item C1 brings up secondary cores for actors only. \
                     Place this driver on core 0 (`core=0`), or wait for item C2's per-core \
                     device lanes",
                    d.name
                )));
            }
        }
        // plans/M8.md item C2, on top of item D: one entry per **mailbox
        // root**, in `mailbox_root_names` order — every declared actor,
        // then every messageable `@driver`. A driver's entry is always 0:
        // shape decision 2 keeps `@driver` on core 0 and the arm just above
        // refuses anything else, so a messageable driver is only ever a
        // ring *destination*, never a ring source, and 04 §3's "a
        // `@driver`'s vectors, pools, permits, and recovery lanes live on
        // its core" is not in tension — a ring slot carries a method index,
        // a waker and that method's own argument words, and nothing else.
        let mut actor_cores: Vec<usize> = (0..tables.actors.len())
            .map(|i| {
                placement
                    .core_of(&crate::eval::image::ImageDeclRef::Actor(i))
                    .unwrap_or(0)
            })
            .collect();
        actor_cores.extend(
            tables
                .drivers
                .iter()
                .filter(|d| d.mailbox.is_some())
                .map(|_| 0),
        );
        let shapes = merge_actor_pub_methods(boot.modules, boot.layout_ctx)?;
        // plans/M8.md item D: dispatch tables are per *mailbox root*, in
        // `mailbox_root_names`' order — the same order
        // `build_runtime_glue_block` walks. A messageable driver's methods
        // are numbered by the identical `merge_actor_pub_methods` shapes
        // `actor_method_index_tables` hands codegen, so an admitted method
        // index means the same thing on both sides.
        let dispatch = mailbox_root_names(&tables)
            .into_iter()
            .map(|name| {
                let methods = shapes.get(&name).cloned().unwrap_or_default();
                let keys = methods
                    .iter()
                    .map(|m| {
                        (
                            format!("{name}.{}", m.name),
                            m.is_async,
                            m.reply_is_aggregate,
                        )
                    })
                    .collect();
                (name, keys)
            })
            .collect();
        // Every rejection this pass can still make lives in here, keyed on
        // the shape boot genuinely cannot marshal rather than on "declares
        // parameters at all" (`build_boot_init_calls`'s own doc comment
        // lists them). Derived against the *declared* actor set, never
        // against every struct in the closure — an `init` on a plain data
        // struct is ordinary, legal code (`Pair.init(lo, hi)` in
        // `golden/boot-actor-reply-struct`) and is none of this pass's
        // business.
        let layouts = closure_layout_types(boot.modules, boot.programs)?;
        let backings =
            crate::eval::image_checks::pool_backings(boot.graph, &layouts).map_err(|e| {
                LayoutError::new(format!(
                    "internal error: a pool declaration this image's own graph check accepted \
                     cannot be read for own-handle materialization: {}",
                    e.message
                ))
            })?;
        let (init_calls, driver_init_calls) =
            build_boot_init_calls(boot.graph, &actor_inits(boot.modules)?, &backings)?;
        debug_assert_eq!(
            init_calls.len(),
            tables.actors.len(),
            "one boot `init` call per declared actor instance"
        );
        debug_assert_eq!(
            driver_init_calls.len(),
            tables.drivers.len(),
            "one boot `init` call per declared driver instance"
        );
        let state_sizes = tables.actors.iter().map(|a| a.state_size).collect();
        let driver_state_sizes = tables.drivers.iter().map(|d| d.state_size).collect();
        let mut wiring = RuntimeWiring {
            tables,
            dispatch,
            init_calls,
            driver_init_calls,
            state_sizes,
            driver_state_sizes,
            group_child_index: boot.group_child_index.clone(),
            actor_cores,
            placement,
            irq_calls: Vec::new(),
            wake_calls: Vec::new(),
        };
        // plans/M8.md item C2 / Wave 1: rings from FlowWir call sites (no
        // CodegenProgram) so live rtconfig can generate before runtime codegen.
        let rings = cross_core_rings(boot.flow, &wiring)?;
        reject_unlowerable_cross_core_shapes(&rings, &wiring, boot, boot.flow)?;
        wiring.tables.add_cross_core_rings(rings);
        // M11 F: stamp select/drain/child facts onto tables so dump and
        // live runtime codegen share one `rtconfig::generate` input.
        fill_rtconfig_facts(&mut wiring)?;
        // M11 I: IRQ/wake facts for checkpoint body (decision 823).
        fill_checkpoint_irq_facts(&mut wiring, boot)?;
        Ok(Some(wiring))
    }
}

/// Fill `RuntimeTables::{select_by_core,drain_by_core,child_sites,
/// ring_target_handles,enqueue_handles,enqueue_actors}` from the finished
/// wiring (plans/M11.md item F / decision 790; item G / decision 801).
fn fill_rtconfig_facts(wiring: &mut RuntimeWiring) -> Result<(), LayoutError> {
    let roots = mailbox_root_names(&wiring.tables);
    let mut select_by_core: Vec<Vec<String>> = vec![Vec::new(); wiring.tables.cores];
    for (i, name) in roots.iter().enumerate() {
        let core = wiring.actor_cores.get(i).copied().unwrap_or(0);
        if core < select_by_core.len() {
            select_by_core[core].push(name.clone());
        }
    }
    let mut drain_by_core = vec![false; wiring.tables.cores];
    for r in &wiring.tables.rings {
        if r.dst < drain_by_core.len() {
            drain_by_core[r.dst] = true;
        }
    }
    let actor_n = wiring.tables.actors.len();
    let msg_drivers = wiring
        .tables
        .drivers
        .iter()
        .filter(|d| d.mailbox.is_some())
        .count();
    let mut child_sites = Vec::new();
    for (callee_key, &child_index) in &wiring.group_child_index {
        let Some(pos) = wiring
            .tables
            .free_turns
            .iter()
            .position(|(k, _)| k == callee_key)
        else {
            continue;
        };
        child_sites.push((callee_key.clone(), child_index, actor_n + msg_drivers + pos));
    }
    // Handle space: actors then drivers then devices (image_decl_handle_word).
    // Mailbox roots are actors then messageable drivers — handle word for
    // actor i is i; for messageable driver at drivers[j] it is actor_n + j.
    let mut enqueue_handles = Vec::new();
    let mut enqueue_actors = Vec::new();
    for (i, a) in wiring.tables.actors.iter().enumerate() {
        enqueue_handles.push(i as u64);
        enqueue_actors.push(a.name.clone());
    }
    for (j, d) in wiring.tables.drivers.iter().enumerate() {
        if d.mailbox.is_some() {
            enqueue_handles.push((actor_n + j) as u64);
            enqueue_actors.push(d.name.clone());
        }
    }
    let mut ring_target_handles = Vec::with_capacity(wiring.tables.rings.len());
    for r in &wiring.tables.rings {
        match r.kind {
            RingKind::Request => {
                let actor = r.actor.as_deref().unwrap_or("");
                let h = enqueue_actors
                    .iter()
                    .zip(enqueue_handles.iter())
                    .find(|(n, _)| *n == actor)
                    .map(|(_, h)| *h)
                    .unwrap_or(0);
                ring_target_handles.push(h);
            }
            RingKind::Reply => ring_target_handles.push(0),
        }
    }
    wiring.tables.select_by_core = select_by_core;
    wiring.tables.drain_by_core = drain_by_core;
    wiring.tables.child_sites = child_sites;
    wiring.tables.ring_target_handles = ring_target_handles;
    let mut root_methods = Vec::with_capacity(enqueue_actors.len());
    let mut root_cores = Vec::with_capacity(enqueue_actors.len());
    for (i, name) in enqueue_actors.iter().enumerate() {
        let methods = wiring
            .dispatch
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, m)| m.clone())
            .unwrap_or_default();
        root_methods.push(methods);
        // `actor_cores` is parallel to `mailbox_root_names` == enqueue_actors order.
        root_cores.push(wiring.actor_cores.get(i).copied().unwrap_or(0));
    }
    wiring.tables.enqueue_handles = enqueue_handles;
    wiring.tables.enqueue_actors = enqueue_actors;
    wiring.tables.root_methods = root_methods;
    wiring.tables.root_cores = root_cores;
    // M11 H: drivers then actors (same call order as former emit_boot_init).
    let n_boot_calls = wiring
        .driver_init_calls
        .iter()
        .chain(wiring.init_calls.iter())
        .filter(|c| c.is_some())
        .count();
    if n_boot_calls > crate::rtconfig::BOOT_CALL_POOL_COUNT {
        return Err(LayoutError::new(format!(
            "image needs {n_boot_calls} boot init calls; pool is {}",
            crate::rtconfig::BOOT_CALL_POOL_COUNT
        )));
    }
    wiring.tables.n_boot_calls = n_boot_calls;
    Ok(())
}

/// M11 I / decision 823 / M12 item D: stamp IRQ vector bits + contiguous
/// `WAKE.wake_pending` addresses onto `tables`, and handler/task keys onto
/// `wiring` for inject.
fn fill_checkpoint_irq_facts(
    wiring: &mut RuntimeWiring,
    boot: &BootCtx,
) -> Result<(), LayoutError> {
    // Place once for driver_state addresses (wake region still empty).
    let rtdata = place_runtime_tables(wrela_machine::layout::RTDATA_BASE, &wiring.tables);
    let (irq, wake) = checkpoint_irq_shape(Some(boot), Some(&rtdata), Some(&wiring.tables));
    if irq.len() > crate::rtconfig::IRQ_CALL_POOL_COUNT {
        return Err(LayoutError::new(format!(
            "image needs {} IRQ stubs; pool is {}",
            irq.len(),
            crate::rtconfig::IRQ_CALL_POOL_COUNT
        )));
    }
    if wake.len() > crate::rtconfig::WAKE_CALL_POOL_COUNT {
        return Err(LayoutError::new(format!(
            "image needs {} wake stubs; pool is {}",
            wake.len(),
            crate::rtconfig::WAKE_CALL_POOL_COUNT
        )));
    }
    // Reserve the contiguous WAKE array after rings, then re-place so
    // `wake_base` sits past the ring reservation.
    wiring.tables.total_bytes += (wake.len() as u64) * 8;
    wiring.tables.wake_pending_addrs = vec![0; wake.len()];
    let rtdata = place_runtime_tables(wrela_machine::layout::RTDATA_BASE, &wiring.tables);
    wiring.tables.irq_vector_bits = irq.iter().map(|e| e.vector).collect();
    wiring.tables.wake_pending_addrs = (0..wake.len())
        .map(|i| rtdata.wake_base + (i as u64) * 8)
        .collect();
    // First drain index per driver (shared-bit / Reloc::WakePending target).
    for d in &mut wiring.tables.drivers {
        d.wake_drain_index = None;
    }
    for e in &wake {
        // Match by placed driver_state address.
        if let Some(di) = rtdata
            .drivers
            .iter()
            .position(|&addr| addr == e.driver_state)
        {
            let d = &mut wiring.tables.drivers[di];
            if d.wake_drain_index.is_none() {
                d.wake_drain_index = Some(e.wake_drain_index);
            }
        }
    }
    wiring.irq_calls = irq
        .into_iter()
        .map(|e| (e.handler_key, e.driver_state))
        .collect();
    wiring.wake_calls = wake
        .into_iter()
        .map(|e| (e.task_key, e.driver_state))
        .collect();
    Ok(())
}

pub struct BootCtx<'a> {
    pub graph: &'a ImageGraph,
    pub modules: &'a BTreeMap<String, Module>,
    /// Typed programs for the same closure — needed so
    /// `closure_layout_types` can run `complete_layouts` (plans/M10.md
    /// item E1 / A2b carry): a `@layout(runtime)` array length that is a
    /// `const` name has no size until after const evaluation, and that
    /// evaluation's results live here.
    pub programs: &'a BTreeMap<String, TypedProgram>,
    pub layout_ctx: &'a LayoutCtx,
    /// `codegen::async_frame_sizes`' result for this same build — every
    /// async fn's own persistent frame bytes, the park-and-resume
    /// redesign's sizing input (`compute_runtime_tables`'s own doc).
    pub async_frames: &'a BTreeMap<String, u64>,
    /// `codegen::compute_group_child_indices`' result for this same build
    /// (plans/M6.md item F / M10 E4): every `g.start`-able callee's own
    /// fixed child-slot ordinal — consumed by `__wrela_child_poll` /
    /// rtconfig child ladders (M11 F). Empty for a build with no
    /// `with group(...)` sites at all.
    pub group_child_index: &'a BTreeMap<String, usize>,
    /// Guest FlowWir for this build — Wave 1 ring/checkpoint facts for
    /// `RuntimeWiring::derive` without a compiled `CodegenProgram`.
    pub flow: &'a FlowWirProgram,
}
