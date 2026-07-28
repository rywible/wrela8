//! FlowWir (plans/M6.md item B, decision 2): the typed, suspension-
//! explicit form between `sema::typed::TypedProgram` and `mwir::MwirProgram`
//! — for **async fns/methods only**. A sync fn's own typed tree keeps the
//! exact M5 `typed` -> `mwir` path unchanged (`lower.rs` is untouched by
//! this item beyond a one-line visibility bump on `mwir::fmt_inst`, and
//! every M5 mwir/asm golden must not move) — FlowWir exists specifically
//! because an async fn's own body cannot lower straight to mwir's flat,
//! non-suspending instruction list: 04-compiler.md §2's "async functions
//! lower ahead of time to state machines in statically reserved frame
//! slots" needs a real state-machine shape to lower *from*, and this
//! module is that shape, plus its own stable text dump
//! (`--stage=flowwir`, `bin/wrela.rs`).
//!
//! ## Shape decisions (this item's own "yours to finalize", recorded once)
//!
//! - **Embed `mwir::Inst`, don't invent a parallel instruction enum**
//!   (the plan's own preference, when the shape permits): every
//!   non-suspending, straight-line op an async body needs — arithmetic,
//!   comparisons, struct/enum construction and projection, pattern-test
//!   sub-tests, `Jump`/`JumpIfFalse` for *local* (never-suspending)
//!   control flow, `Return`/`AssertFail` — is exactly what `mwir::Inst`
//!   already carries, so `FlowInst::Mwir(mwir::Inst)` wraps it verbatim
//!   rather than re-declaring an equivalent set. A handful of ops have no
//!   mwir counterpart at all (a suspension-adjacent scope's own
//!   bookkeeping: reading the clock, constructing a duration, sending a
//!   one-way message, opening/closing a group scope, starting a group
//!   child) — those five get their own `FlowInst` variants, below.
//! - **Frame rule: every temp gets a slot, no liveness analysis**
//!   (`FrameLayout::temp_types`, "spill-everything's sibling" per the
//!   plan's own words) — a `FlowWirFn`'s whole temp set, both the ones
//!   live across a suspension and the ones that never see one, occupies
//!   the identical uniform table `mwir::MwirFn::temp_types` already uses
//!   for a sync fn's temps; the *only* new fact FlowWir adds is that this
//!   fn has more than one `State`, each holding its own straight-line
//!   slice of ops, rather than mwir's single flat list. Dumbest correct
//!   at M6: a real liveness pass (which temps actually need a frame slot
//!   vs. which could stay in a register within one state) is a real
//!   optimization item C/D never asked for and the cleverness budget
//!   (CLAUDE.md) has not been spent on.
//! - **Intra-state control flow**: an `if`/`while`/`for`/`match` that
//!   contains no `await` *anywhere* inside it (`contains_await` below)
//!   lowers exactly like `lower.rs`'s own sync-fn logic — local
//!   `Jump`/`JumpIfFalse` targets are indices into the *current state's
//!   own* `ops` list, never a cross-state reference, and `return`/`break`/
//!   `continue` embed as ordinary `Mwir(Inst::Return)`/`Mwir(Inst::Jump)`
//!   ops exactly as `mwir::Inst` already allows mid-list (nothing after an
//!   embedded, always-taken `Return` is ever reached — the state's own
//!   trailing `Transition` is then dead, mirroring `lower.rs`'s own
//!   harmless trailing `Inst::Return {value: None}`). An `if`/`while`
//!   whose body/branch contains an `await` **must** split into real
//!   states (a suspension is a genuinely different resume point, not a
//!   local jump target) — `flowwir_lower.rs`'s own module doc works
//!   through the `Branch`/loop-back-edge shapes. `match`/`for` stay
//!   intra-state only (fail closed, named, if an arm/body contains an
//!   `await` — no required golden needs either splitting, so the
//!   generality is not built).
//! - **Lineage/deadline plumbing** (decision 2's own "explicit"):
//!   `FrameLayout` reserves two dedicated `u64` frame slots per
//!   `FlowWirFn` — `lineage_group_slot` (the ambient group id) and
//!   `lineage_deadline_slot` (the ambient effective deadline) — always
//!   `Temp(0)`/`Temp(1)` (allocated first, before `self`/params/every
//!   other temp), an implicit parameter every async fn/method carries per
//!   02-language.md §9.5's "ambient lineage... signatures never thread a
//!   context parameter." Nothing in this item's own required goldens
//!   reads or writes them (the real inheritance/narrowing mechanism is
//!   item F's job) — they exist here purely so every later item has a
//!   stable slot to target, per the plan's own "the IR just must carry
//!   them."
//! - **`with group` -> explicit create/close + cleanup edges**:
//!   `FlowInst::GroupCreate`/`GroupClose` bracket a group's scope;
//!   `GroupClose::cleanup_states` is the dumbest referenceable shape the
//!   plan asks for — a `Vec<usize>` naming this group's own `defer`
//!   cleanup states, in reverse (source) registration order, each one a
//!   dedicated `State` (not inlined) specifically so a *later* item
//!   (cancellation, item F) can jump into the identical graph out of
//!   band, from a path this walk's own normal top-down lowering never
//!   takes. A `defer` registered *outside* any group inlines its body as
//!   ordinary ops at its own natural block-scope exit (mirrors
//!   `lower.rs`'s `run_defers` exactly) — it is not part of any
//!   cancellation domain at M6 (02-language.md §10 ties the cleanup graph
//!   to a group), so nothing needs to reference it independently.
//! - **Self-rooted paths across `await`** (02-language.md §9.2): a local
//!   bound to a self-rooted field chain (`x = self.cache.value`) is kept
//!   in the lowering environment as the *path itself*
//!   (`flowwir_lower::Binding::SelfPath(Vec<String>, Type)`, field names
//!   only, no temp) rather than a computed `Temp` — every later read of
//!   that local re-derives it fresh, via the dedicated `FlowInst::SelfPath`
//!   op, in whatever state the read actually happens in (before or after
//!   a suspension — the mechanism does not care, it always re-projects
//!   from the fn's own stable `self` temp). This is what "the frame
//!   records the field path and re-derives it" (02 §9.2's own words)
//!   means at the IR level: the *path* survives a suspension, never a
//!   pre-computed value. An external-rooted path is never bound this way
//!   (`sema::bodies::check_cross_await` already rejects it wherever it
//!   would matter); it lowers as an ordinary computed temp exactly like
//!   any other value.
//!
//! ## Fail-closed set (this item's own boundary, named rather than faked)
//!
//! `flowwir_lower.rs`'s own module doc enumerates the full list; the
//! headline items: a plain (non-actor, non-group) fn/method call inside
//! an async body's expression position (no required golden needs one —
//! every call this item's goldens make is an actor call, a `send`, or a
//! group child, all of which have their own dedicated lowering); a
//! `match`/`for` containing an `await`; an `await`/`send`/group-child
//! nested inside a larger expression (only a direct `let`/assignment/
//! `return`/bare-statement operand is supported); a `defer` body
//! containing an `await`; a `with group` body that exits early (`return`)
//! from inside it (the group's own natural fall-through close is what
//! this item lowers; early-exit teardown ordering is item F's job);
//! `Option`-typed `?` (only `Result` is supported); a `g.start` callee
//! naming a `self`-method (only a bare top-level `async fn` callee is
//! exercised); every generic instantiation (no async generic exists in
//! the M6 surface).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::mwir::{self, Temp};
use crate::sema::types::{self, Type};
use crate::syntax::ast::AccessMode;

// --- program/fn shape ------------------------------------------------------

/// The whole lowered async surface: one entry per async fn/method, keyed
/// by the exact `sema::typed::CalleeKey::spelling()` string its
/// declaration would produce (identical convention to
/// `mwir::MwirProgram::fns` — a plain top-level fn's bare name, a
/// method's `Struct.member`). A sync fn/method never appears here at all
/// (it never left the M5 `typed` -> `mwir` path); a generic instantiation
/// never appears here either (this item's own disclosed boundary — no
/// async generic exists in the M6 surface, see the module doc's
/// fail-closed set).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlowWirProgram {
    pub fns: BTreeMap<String, FlowWirFn>,
}

/// One async fn/method's own state machine. `receiver` mirrors
/// `mwir::MwirFn::receiver` exactly (`Some((self_temp, mode))` for a
/// method, `None` for a free `async fn`); `params` are the fn's own
/// declared parameters' temps, in declaration order (mirrors
/// `mwir::MwirFn::params`). `states[0]` is always the entry state (the
/// fn's own root turn/call starts executing there); every other state is
/// reachable only via some earlier state's own `Transition`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowWirFn {
    pub receiver: Option<(Temp, AccessMode)>,
    pub params: Vec<(Temp, AccessMode)>,
    pub ret: Type,
    pub frame: FrameLayout,
    pub states: Vec<State>,
}

/// The statically-reserved frame (04-compiler.md §2) this fn's every
/// activation lives in: `temp_types[t.0]` is temp `t`'s own static type,
/// covering *every* temp this fn ever allocates — a suspension-crossing
/// value and a single-state-local scratch value are kept in the exact
/// same uniform table (module doc's own "Frame rule"; no liveness
/// analysis narrows this at M6). `lineage_group_slot`/
/// `lineage_deadline_slot` are always `Temp(0)`/`Temp(1)` — allocated
/// before `self`, before every declared parameter, before any ordinary
/// local — the ambient-lineage pair (module doc's own "Lineage/deadline
/// plumbing" section) every async fn/method implicitly carries.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameLayout {
    pub temp_types: Vec<Type>,
    pub lineage_group_slot: Temp,
    pub lineage_deadline_slot: Temp,
}

impl FrameLayout {
    pub fn temp_count(&self) -> usize {
        self.temp_types.len()
    }
}

/// One state: a straight-line run of non-suspending ops, ending in the
/// one `Transition` that decides what happens next. `ops` never contains
/// a suspension of its own (an `Await` is always this state's own
/// `Transition`, never a mid-list op) — see the module doc's "Intra-state
/// control flow" section for how `return`/`break`/`continue`, which
/// *can* end a state's own reachable path early, still live as ordinary
/// embedded ops (mirroring `mwir::Inst::Return`'s own mid-list legality)
/// rather than forcing a `Transition` of their own.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub ops: Vec<FlowInst>,
    pub transition: Transition,
}

/// One straight-line op. `Mwir` is the embedding decision (module doc);
/// the other five variants are the ones no `mwir::Inst` shape covers —
/// every one of them is non-suspending (a real suspension is always a
/// `Transition::Await`, never an op).
#[derive(Debug, Clone, PartialEq)]
pub enum FlowInst {
    /// A straight-line `mwir::Inst`, verbatim — every `Jump`/`JumpIfFalse`
    /// inside one of these targets a *local* index into this same
    /// state's own `ops` (module doc: never a cross-state reference).
    Mwir(mwir::Inst),
    /// Re-derives a self-rooted field chain fresh, at exactly this point
    /// in the program (module doc's own "Self-rooted paths across
    /// `await`") — `path` is the field-name sequence from `self` down
    /// (`["cache", "value"]` for `self.cache.value`), never a cached
    /// temp from an earlier state.
    SelfPath { dst: Temp, path: Vec<String> },
    /// `now()` (plans/M6.md decision 11): reads the clock — runtime-only,
    /// non-suspending. The real MMIO lowering is item D/E's job; this op
    /// just names the read.
    Now { dst: Temp },
    /// `entropy[N]()` (plans/M17.md item E / freeze 1): sealed runtime
    /// entropy fill — non-suspending. `n` is the comptime length
    /// (`1..=ENTROPY_LEN_MAX`); codegen expands a packed scratch into the
    /// slot-per-byte `Bytes[N]` `dst`.
    Entropy { dst: Temp, n: u64 },
    /// `ms(n)`: constructs a duration value from `n` — comptime-legal,
    /// non-suspending. The real tick-scale conversion is item D's job
    /// (`sema::bodies`'s own doc comment on the `"ms"` intrinsic); this
    /// op just carries `n` forward, opaque, exactly like `now`'s reading.
    Duration { dst: Temp, n: Temp },
    /// `send target.method(args...)` (02-language.md §9.4): enqueues a
    /// one-way message — non-suspending (never a state split, unlike
    /// `await`). `dst` always gets the `Result[unit, CallError[never]]`
    /// value (plans/M13.md item J / decision 5). `take_arg_temps` are the
    /// take-mode argument temps handed back inside `NotAdmitted` when
    /// enqueue refuses — same construction as an awaited call's reject
    /// arm (item H).
    Send {
        dst: Temp,
        target: Temp,
        method_key: String,
        arg_temps: Vec<Temp>,
        take_arg_temps: Vec<Temp>,
    },
    /// `with group(capacity=.., deadline=..) [as g]:`'s own opening
    /// bracket (02-language.md §9.5) — `group_temp` is always allocated
    /// (even with no `as` binding, since `GroupClose` still needs to name
    /// it); `capacity`/`deadline` are the already-lowered optional
    /// constructor arguments.
    GroupCreate {
        group_temp: Temp,
        capacity: Option<Temp>,
        deadline: Option<Temp>,
    },
    /// `g.start(callee, args...)` (02-language.md §9.5) — `callee_key` is
    /// the child's own `sema::typed::CalleeKey::spelling()` (a bare
    /// top-level `async fn`'s name at M6, module doc's own disclosed
    /// boundary on the `self`-method callee shape); `arg_temps` align
    /// 1:1 with the callee's own declared parameters (a caller-omitted,
    /// defaulted slot is filled from the callee's own stored default,
    /// mirroring `lower.rs`'s `bind_args` convention one stage later).
    GroupStart {
        group_temp: Temp,
        callee_key: String,
        arg_temps: Vec<Temp>,
    },
    /// The group's own closing bracket — `cleanup_states` names this
    /// group's own `defer` cleanup chain (module doc's own "`with group`"
    /// section), reverse registration order, empty when the group's body
    /// registered no `defer` of its own.
    GroupClose {
        group_temp: Temp,
        cleanup_states: Vec<usize>,
    },
}

/// What a state does once its own straight-line `ops` finish.
#[derive(Debug, Clone, PartialEq)]
pub enum Transition {
    /// This activation's own end — `None` for a bare `return`/falling off
    /// a `unit`-returning body's own end, mirroring `mwir::Inst::Return`.
    Return(Option<Temp>),
    /// A real suspension: `what` names the awaitable, `resume_state` is
    /// where execution continues once it resolves, `result_temp` is the
    /// frame slot that awaitable's own composed result lands in (module
    /// doc: this is the *only* place a suspension is ever represented —
    /// never a mid-state op).
    Await {
        what: AwaitKind,
        resume_state: usize,
        result_temp: Temp,
    },
    /// An unconditional cross-state jump — a loop's own back-edge when
    /// its body contains an `await` (a state cycle, per the plan's own
    /// "loop back-edge with an await inside = state cycle"), or the link
    /// between a group's own close and its cleanup chain / the state
    /// after it.
    Jump(usize),
    /// A cross-state conditional split — used only when at least one
    /// side of an `if`/`while` contains an `await` (module doc: an
    /// await-free `if`/`while` never needs this, it stays intra-state).
    Branch {
        cond_temp: Temp,
        then_state: usize,
        else_state: usize,
    },
    /// Unconditional abandonment — mirrors `mwir::Inst::AssertFail`'s own
    /// role, lifted to a `Transition` only where lowering itself needs to
    /// end a *state* rather than embed a mid-list op (a should-be-
    /// unreachable producer-bug guard; `flowwir_lower.rs` never emits
    /// this outside of one such defensive path).
    Abort { msg: String },
}

/// What a `Transition::Await` actually awaits (02-language.md §9.4/§9.5):
/// an actor-handle method call, a group's own `join_all()`, or a
/// `Receipt[P]`.
///
/// **Hard cap:** these three variants are the closed set. Do not add
/// peephole/`Suspend`/`ResumeCompose` synthetic MWIR Insts unless an
/// addition deletes real code in `flowwir_lower` / async codegen — the
/// await transition *is* the suspension representation (module doc).
#[derive(Debug, Clone, PartialEq)]
pub enum AwaitKind {
    /// `target_temp` is the already-lowered `Actor[T]` handle value (a
    /// plain `Temp` suffices — module doc's own note: the receiver
    /// expression evaluates synchronously, right at the call, so it can
    /// never itself need to survive a suspension the way a `let`-bound
    /// local can). `method_key` is `"{ActorStruct}.{method}"` (the same
    /// spelling `CalleeKey::Method::spelling()` would produce).
    ActorCall {
        target_temp: Temp,
        method_key: String,
        arg_temps: Vec<Temp>,
        /// plans/M13.md item H: temps of `take`-mode message params, in
        /// declaration order — packed into `NotAdmitted`'s args tuple on
        /// enqueue-fail (caller still owns them; enqueue did not commit).
        take_arg_temps: Vec<Temp>,
    },
    /// `g.join_all()` — `group_temp` is the group local's own temp;
    /// `child_count` is the number of static `g.start` sites found in
    /// that group's body (`sema::bodies::compute_group_children`'s own
    /// count, carried forward rather than re-derived).
    GroupJoin {
        group_temp: Temp,
        child_count: usize,
    },
    /// `await receipt` (plans/M7.md item E4, 03-hardware.md §3/§5):
    /// park until the driver's `drain` resolves this `Receipt[P]` into
    /// an `IoCompletion[P]`.
    Receipt { receipt_temp: Temp },
}

// --- the `--stage=flowwir` dump (M1 dump style) -----------------------------

pub fn dump(program: &FlowWirProgram) -> String {
    let mut out = String::new();
    out.push_str("Program\n");
    for (key, f) in &program.fns {
        let mut header = format!(
            "Fn key={key} ret={} states={} frame={}",
            types::render_type(&f.ret),
            f.states.len(),
            f.frame.temp_count()
        );
        let _ = write!(
            header,
            " lineage=[group={},deadline={}]",
            f.frame.lineage_group_slot, f.frame.lineage_deadline_slot
        );
        if let Some((t, mode)) = &f.receiver {
            let _ = write!(header, " receiver={t}:{}", mode.as_str());
        }
        if !f.params.is_empty() {
            let ps: Vec<String> = f
                .params
                .iter()
                .map(|(t, mode)| {
                    if *mode == AccessMode::Read {
                        t.to_string()
                    } else {
                        format!("{t}:{}", mode.as_str())
                    }
                })
                .collect();
            let _ = write!(header, " params=[{}]", ps.join(","));
        }
        push_line(&mut out, 1, &header);
        for (i, ty) in f.frame.temp_types.iter().enumerate() {
            push_line(
                &mut out,
                2,
                &format!("Temp t{i} ty={}", types::render_type(ty)),
            );
        }
        for (i, state) in f.states.iter().enumerate() {
            push_line(&mut out, 2, &format!("State {i}"));
            for (j, op) in state.ops.iter().enumerate() {
                let line = format!("{j:04}: {}", fmt_flow_inst(op));
                push_line(&mut out, 3, &line);
            }
            push_line(
                &mut out,
                3,
                &format!("Transition {}", fmt_transition(&state.transition)),
            );
        }
    }
    out
}

fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

fn join_temps(ts: &[Temp]) -> String {
    ts.iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn fmt_flow_inst(op: &FlowInst) -> String {
    match op {
        FlowInst::Mwir(inst) => format!("Mwir {}", mwir::fmt_inst(inst)),
        FlowInst::SelfPath { dst, path } => {
            format!("SelfPath dst={dst} path=self.{}", path.join("."))
        }
        FlowInst::Now { dst } => format!("Now dst={dst}"),
        FlowInst::Entropy { dst, n } => format!("Entropy dst={dst} n={n}"),
        FlowInst::Duration { dst, n } => format!("Duration dst={dst} n={n}"),
        FlowInst::Send {
            dst,
            target,
            method_key,
            arg_temps,
            take_arg_temps,
        } => {
            if take_arg_temps.is_empty() {
                format!(
                    "Send dst={dst} target={target} method={method_key} args=[{}]",
                    join_temps(arg_temps)
                )
            } else {
                format!(
                    "Send dst={dst} target={target} method={method_key} args=[{}] take=[{}]",
                    join_temps(arg_temps),
                    join_temps(take_arg_temps)
                )
            }
        }
        FlowInst::GroupCreate {
            group_temp,
            capacity,
            deadline,
        } => {
            let mut s = format!("GroupCreate group={group_temp}");
            if let Some(c) = capacity {
                let _ = write!(s, " capacity={c}");
            }
            if let Some(d) = deadline {
                let _ = write!(s, " deadline={d}");
            }
            s
        }
        FlowInst::GroupStart {
            group_temp,
            callee_key,
            arg_temps,
        } => format!(
            "GroupStart group={group_temp} callee={callee_key} args=[{}]",
            join_temps(arg_temps)
        ),
        FlowInst::GroupClose {
            group_temp,
            cleanup_states,
        } => {
            let cs: Vec<String> = cleanup_states.iter().map(|i| i.to_string()).collect();
            format!("GroupClose group={group_temp} cleanup=[{}]", cs.join(","))
        }
    }
}

fn fmt_transition(t: &Transition) -> String {
    match t {
        Transition::Return(value) => match value {
            Some(v) => format!("Return value={v}"),
            None => "Return".to_string(),
        },
        Transition::Await {
            what,
            resume_state,
            result_temp,
        } => format!(
            "Await what={} resume={resume_state} result={result_temp}",
            fmt_await_kind(what)
        ),
        Transition::Jump(target) => format!("Jump target={target}"),
        Transition::Branch {
            cond_temp,
            then_state,
            else_state,
        } => format!("Branch cond={cond_temp} then={then_state} else={else_state}"),
        Transition::Abort { msg } => format!("Abort msg={msg:?}"),
    }
}

fn fmt_await_kind(k: &AwaitKind) -> String {
    match k {
        AwaitKind::ActorCall {
            target_temp,
            method_key,
            arg_temps,
            take_arg_temps,
        } => {
            if take_arg_temps.is_empty() {
                format!(
                    "ActorCall{{target={target_temp},method={method_key},args=[{}]}}",
                    join_temps(arg_temps)
                )
            } else {
                format!(
                    "ActorCall{{target={target_temp},method={method_key},args=[{}],take=[{}]}}",
                    join_temps(arg_temps),
                    join_temps(take_arg_temps)
                )
            }
        }
        AwaitKind::GroupJoin {
            group_temp,
            child_count,
        } => format!("GroupJoin{{group={group_temp},children={child_count}}}"),
        AwaitKind::Receipt { receipt_temp } => {
            format!("Receipt{{receipt={receipt_temp}}}")
        }
    }
}
