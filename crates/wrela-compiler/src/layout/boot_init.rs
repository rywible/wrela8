use std::collections::BTreeMap;

use crate::eval::image::ImageGraph;
use crate::eval::value::Value;
use crate::flowwir::FlowWirProgram;
use crate::mwir::LayoutCtx;
use crate::sema::typed::TypedProgram;
use crate::syntax::ast::Module;

use super::place::place_runtime_tables;
use super::rtdata::{
    RingKind, RuntimeTables, compute_runtime_tables, mailbox_root_names, merge_actor_pub_methods,
};
use super::{
    DeviceRegs, LayoutError, PoolPlacement, append_rodata, checkpoint_irq_shape,
    closure_imported_types, closure_layout_types, cross_core_rings,
    reject_unlowerable_cross_core_shapes,
};

pub(crate) struct ActorInit {
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

#[derive(Debug)]
pub(crate) struct BootInitCall {
    pub(crate) key: String,
    pub(crate) args: Vec<BootInitArg>,
    pub(crate) fallible: bool,
    pub(crate) err_msg: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BootInitArg {
    Word(u64),
    DeviceRegsBase(usize),
    PoolBase(String),
    OwnSlot {
        pool: String,
        index: u64,
        slot_bytes: u64,
    },
    OwnHandleArray {
        pool: String,
        count: u64,
        slot_bytes: u64,
    },
}

impl BootInitArg {
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

pub(crate) fn image_decl_handle_word(
    space: HandleSpace,
    decl: &crate::eval::image::ImageDeclRef,
) -> Option<u64> {
    use crate::eval::image::ImageDeclRef;
    match decl {
        ImageDeclRef::Actor(i) => Some(*i as u64),
        ImageDeclRef::Driver(i) => Some((space.n_actors + *i) as u64),
        ImageDeclRef::Device(i) => Some((space.n_actors + space.n_drivers + *i) as u64),
        ImageDeclRef::Renderer(_) | ImageDeclRef::Pool(_) | ImageDeclRef::DmaPool(_) => None,
    }
}

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
        Value::ImageDecl(ImageDeclRef::Renderer(_)) => "a renderer handle",
        Value::ImageDecl(ImageDeclRef::Pool(_)) => "a pool handle",
        Value::ImageDecl(ImageDeclRef::DmaPool(_)) => "a DMA-pool handle",
    }
}

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

pub(crate) fn device_index_of(args: &[crate::eval::image::DeclArg]) -> Option<usize> {
    use crate::eval::image::ImageDeclRef;
    args.iter()
        .find(|a| a.label == "device")
        .and_then(|a| match &a.value {
            Value::ImageDecl(ImageDeclRef::Device(i)) => Some(*i),
            _ => None,
        })
}

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
        let wired = decl_args.iter().find(|a| {
            a.label == p.name && !crate::eval::image_checks::is_reserved_actor_arg(&a.label)
        });
        let Some(a) = wired else {
            let param_ty = render_type(&p.ty);
            if let Type::Named(tn, targs) = &p.ty {
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

pub(crate) struct RuntimeWiring {
    pub(crate) tables: RuntimeTables,
    pub(crate) dispatch: Vec<(String, Vec<(String, bool, bool)>)>,
    pub(crate) init_calls: Vec<Option<BootInitCall>>,
    pub(crate) driver_init_calls: Vec<Option<BootInitCall>>,
    pub(crate) state_sizes: Vec<u64>,
    pub(crate) driver_state_sizes: Vec<u64>,
    pub(crate) group_child_index: BTreeMap<String, usize>,
    pub(crate) actor_cores: Vec<usize>,
    pub(crate) placement: crate::placement::PlacementTable,
    pub(crate) irq_calls: Vec<(String, u64)>,
    pub(crate) wake_calls: Vec<(String, u64)>,
}

impl RuntimeWiring {
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
        let placement =
            crate::placement::place(boot.graph, boot.modules, boot.layout_ctx, boot.graph.cores)
                .map_err(LayoutError::new)?;
        tables.stripe_for_cores(placement.cores);
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
        let rings = cross_core_rings(boot.flow, &wiring)?;
        reject_unlowerable_cross_core_shapes(&rings, &wiring, boot, boot.flow)?;
        wiring.tables.add_cross_core_rings(rings);
        fill_rtconfig_facts(&mut wiring)?;
        fill_checkpoint_irq_facts(&mut wiring, boot)?;
        Ok(Some(wiring))
    }
}

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
        root_cores.push(wiring.actor_cores.get(i).copied().unwrap_or(0));
    }
    wiring.tables.enqueue_handles = enqueue_handles;
    wiring.tables.enqueue_actors = enqueue_actors;
    wiring.tables.root_methods = root_methods;
    wiring.tables.root_cores = root_cores;
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

fn fill_checkpoint_irq_facts(
    wiring: &mut RuntimeWiring,
    boot: &BootCtx,
) -> Result<(), LayoutError> {
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
    wiring.tables.total_bytes += (wake.len() as u64) * 8;
    wiring.tables.wake_pending_addrs = vec![0; wake.len()];
    let rtdata = place_runtime_tables(wrela_machine::layout::RTDATA_BASE, &wiring.tables);
    wiring.tables.irq_vector_bits = irq.iter().map(|e| e.vector).collect();
    wiring.tables.wake_pending_addrs = (0..wake.len())
        .map(|i| rtdata.wake_base + (i as u64) * 8)
        .collect();
    for d in &mut wiring.tables.drivers {
        d.wake_drain_index = None;
    }
    for e in &wake {
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

#[derive(Clone, Copy)]
pub struct BootCtx<'a> {
    pub graph: &'a ImageGraph,
    pub modules: &'a BTreeMap<String, Module>,
    pub programs: &'a BTreeMap<String, TypedProgram>,
    pub layout_ctx: &'a LayoutCtx,
    pub async_frames: &'a BTreeMap<String, u64>,
    pub group_child_index: &'a BTreeMap<String, usize>,
    pub flow: &'a FlowWirProgram,
}
