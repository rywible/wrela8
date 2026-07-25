//! Lowering (plans/M6.md item B): `sema::typed::TypedProgram` ->
//! `flowwir::FlowWirProgram`, for **async fns/methods only** (a sync fn
//! never reaches this file at all — it stays on the exact M5 `lower.rs`
//! path, decision 2's own hard constraint). `flowwir.rs`'s own module doc
//! records every shape decision this file implements; read that first.
//!
//! ## Shape of the walk
//!
//! One `FlowBuilder` per fn (mirrors `lower.rs`'s `FnBuilder`, generalized
//! from "one flat `Vec<Inst>`" to "several `State`s, each its own small
//! `Vec<FlowInst>`, plus one shared, whole-fn `temp_types` table" — the
//! frame rule, `flowwir.rs`'s own doc). `FlowBuilder::cur` names the
//! state currently being appended to; `emit`/`here`/`patch` all operate
//! on `states[cur]` implicitly, exactly mirroring `FnBuilder::emit`/
//! `here`/`patch_jump`'s own flat-list conventions one level up. A
//! genuinely new state is only ever created at a real suspension boundary
//! (`new_state` + `finish_current` + `switch_to`) — see `flowwir.rs`'s
//! own "Intra-state control flow" section for why an `if`/`while`/`match`/
//! `for` containing no `await` anywhere inside never needs one at all,
//! reusing `lower.rs`'s own local-jump/backpatch technique verbatim
//! (`return`/`break`/`continue` embed as ordinary `Mwir(Inst::Return)`/
//! `Mwir(Inst::Jump)` ops, never a `Transition` of their own, exactly like
//! `mwir::Inst::Return`'s own mid-list legality).
//!
//! `Binding` (this file's own environment value, replacing `lower.rs`'s
//! bare `Temp`) is either an ordinary computed `Temp` or a self-rooted
//! field path (`flowwir.rs`'s own "Self-rooted paths across `await`"
//! section) — `Local(name)` reads re-derive the latter fresh, via
//! `FlowInst::SelfPath`, every single time (never once-computed-then-
//! cached), which is what makes it safe regardless of which state the
//! read actually lands in.
//!
//! ## The two suspending statement shapes
//!
//! Only a direct `let`/assignment/bare-statement operand may itself be
//! `await ...` or `await ...?` (`lower_stmt_operand` below) — an `await`
//! nested any deeper inside a larger expression fails closed, named
//! (`flowwir.rs`'s own disclosed boundary). `?` on an *ordinary* (already
//! synchronous) `Result` value, by contrast, needs no suspension at all —
//! `lower_try_check` embeds its own Ok/Err branch-and-maybe-early-`Return`
//! entirely as ops in the current state (an early return is just another
//! embedded `Mwir(Inst::Return)`, per the module doc above), so it is
//! reachable from `lower_expr_flat` directly, not only from the two
//! statement-level suspending shapes.
//!
//! ## Fail-closed set (this file's own, beyond `flowwir.rs`'s headline
//! list)
//!
//! - `CallValue`/`FnRef`/`OpCall` (a first-class fn value). Plain
//!   `Call` to a top-level sync helper is live (plans/M7.md item E4:
//!   field access after await is refused by the §9.2 scan, so the
//!   flagship finishes through sync helpers).
//! - `Option`-typed `?` (only `Result` is supported). A `?` needing a
//!   `From` conversion is live (plans/M9.md item B) — same rules as
//!   `lower.rs`.
//! - A `match`/`for` containing an `await` anywhere inside (scrutinee,
//!   guard, arm/body) — both stay intra-state-only.
//! - An `elif` chain, or an `if`'s own condition, containing an `await`.
//! - A `defer` body containing an `await`.
//! - An `|` (or) pattern (mirrors `lower.rs`'s own identical gap).
//! - Assigning through, or reading, a nested field/index chain more than
//!   one level deep, unless reached through a `let`-bound self-path
//!   (mirrors `lower.rs`'s own identical restriction).
//! - Every generic instantiation (no async generic exists in the M6
//!   surface — `lower_program` does not even walk
//!   `TypedProgram::instantiations`).
//! - `g.start`'s callee naming a `self`-method (only a bare top-level
//!   `async fn` name is exercised by this item's own required goldens);
//!   `resolve_callee_fn` resolves either shape, but nothing here threads
//!   an implicit `self` into a `self`-method child's own call — a real
//!   gap, disclosed, left for whichever item actually exercises it.

use std::collections::BTreeMap;

use crate::eval::value;
use crate::flowwir::{
    AwaitKind, FlowInst, FlowWirFn, FlowWirProgram, FrameLayout, State, Transition,
};
use crate::mwir::{self, Inst, Temp};
use crate::sema::bodies;
use crate::sema::typed::{
    CalleeKey, TypedDeferBody, TypedElif, TypedExpr, TypedExprKind, TypedFn, TypedForIter,
    TypedMatchArm, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind,
    TypedStruct,
};
use crate::sema::types::Type;
use crate::syntax::ast::{AccessMode, BinOp};

/// The one FlowWir lowering diagnostic — printed by `bin/wrela.rs` the
/// same way `lower::LowerError` already is (`error[unimplemented]: ...`);
/// mirrors that type's own two constructors and reasoning verbatim (the
/// typed tree carries no spans, decision 1, so there is no `at L:C` to
/// add here either).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowError {
    pub message: String,
}

impl FlowError {
    fn unimplemented(construct: impl Into<String>) -> FlowError {
        FlowError {
            message: format!("lowering {} not implemented yet", construct.into()),
        }
    }

    fn internal(message: impl Into<String>) -> FlowError {
        FlowError {
            message: format!("internal error: {}", message.into()),
        }
    }
}

// --- environment: an ordinary temp, or a self-rooted path -----------------

/// `flowwir.rs`'s own "Self-rooted paths across `await`" section:
/// `SelfPath` carries the field-name sequence (never a temp) so every
/// read re-derives it fresh, in whichever state the read actually
/// happens in.
#[derive(Debug, Clone)]
enum Binding {
    Temp(Temp),
    SelfPath(Vec<String>, Type),
}

type FEnv = Vec<BTreeMap<String, Binding>>;

fn env_lookup(env: &FEnv, name: &str) -> Option<Binding> {
    for scope in env.iter().rev() {
        if let Some(b) = scope.get(name) {
            return Some(b.clone());
        }
    }
    None
}

fn env_insert(env: &mut FEnv, name: String, binding: Binding) {
    env.last_mut()
        .expect("at least one scope")
        .insert(name, binding);
}

/// Recognizes a self-rooted whole-value path (02-language.md §9.2):
/// `self` itself (`Some(vec![])`), or a `Field` chain rooted at it
/// (`Some(["cache", "value"])` for `self.cache.value`) — `None` for
/// anything else (an external-rooted path included; `sema::bodies::check_cross_await`
/// already keeps one of those from ever mattering here).
fn self_path_of(e: &TypedExpr) -> Option<Vec<String>> {
    match &e.kind {
        TypedExprKind::Local(n) if n == "self" => Some(Vec::new()),
        TypedExprKind::Field(base, name) => {
            let mut v = self_path_of(base)?;
            v.push(name.clone());
            Some(v)
        }
        _ => None,
    }
}

// --- the state builder ------------------------------------------------------

struct StateWip {
    ops: Vec<FlowInst>,
    transition: Option<Transition>,
}

struct FlowBuilder<'p> {
    prog: &'p TypedProgram,
    ret: Type,
    temp_types: Vec<Type>,
    states: Vec<StateWip>,
    cur: usize,
}

impl<'p> FlowBuilder<'p> {
    fn fresh(&mut self, ty: Type) -> Temp {
        self.temp_types.push(ty);
        Temp(self.temp_types.len() - 1)
    }

    /// Appends `op` to the *current* state's own `ops` (module doc: every
    /// `emit`/`here`/`patch` triple implicitly operates on `states[cur]`).
    fn emit(&mut self, op: FlowInst) -> usize {
        self.states[self.cur].ops.push(op);
        self.states[self.cur].ops.len() - 1
    }

    fn emit_mwir(&mut self, inst: Inst) -> usize {
        self.emit(FlowInst::Mwir(inst))
    }

    /// Appends `op` to an *already-finished* (no longer current) state —
    /// only `lower_with_group`'s own cleanup-chain wiring needs this (the
    /// group's own closing state is no longer `cur` by the time the
    /// chain's real length is known).
    fn emit_at(&mut self, state: usize, op: FlowInst) {
        self.states[state].ops.push(op);
    }

    fn here(&self) -> usize {
        self.states[self.cur].ops.len()
    }

    /// Backpatches a local `Jump`/`JumpIfFalse` inside the *current*
    /// state — every local fixup in this file is emitted and patched
    /// without ever switching `cur` away in between (module doc: this is
    /// exactly what keeps intra-state control flow as simple as
    /// `lower.rs`'s own).
    fn patch(&mut self, idx: usize, target: usize) {
        match &mut self.states[self.cur].ops[idx] {
            FlowInst::Mwir(Inst::Jump { target: t }) => *t = target,
            FlowInst::Mwir(Inst::JumpIfFalse { target: t, .. }) => *t = target,
            other => panic!(
                "flowwir_lower::patch: op {idx} in state {} is not a local jump: {other:?}",
                self.cur
            ),
        }
    }

    fn new_state(&mut self) -> usize {
        self.states.push(StateWip {
            ops: Vec::new(),
            transition: None,
        });
        self.states.len() - 1
    }

    fn switch_to(&mut self, idx: usize) {
        self.cur = idx;
    }

    fn cur(&self) -> usize {
        self.cur
    }

    fn finish(&mut self, idx: usize, t: Transition) {
        assert!(
            self.states[idx].transition.is_none(),
            "flowwir_lower: state {idx} finished twice"
        );
        self.states[idx].transition = Some(t);
    }

    fn finish_current(&mut self, t: Transition) {
        let c = self.cur;
        self.finish(c, t);
    }

    /// Finishes `idx` with `t` only if nothing already did — needed
    /// wherever a block "diverged" (its own bool return) via an
    /// *embedded* op (`Mwir(Inst::Return)`, from a plain `return`, or
    /// from an intra `if`/`match` whose every arm itself ended that way)
    /// rather than via an explicit cross-state exit (`break`/`continue`
    /// in `LoopCtx::Inter` mode, which already calls `finish_current`
    /// itself): the ending state still needs *some* transition to satisfy
    /// "every state ends in one," even though it is dead code after the
    /// embedded op — mirrors `lower.rs`'s own harmless trailing
    /// `Inst::Return {value: None}` for exactly the same reason, one
    /// level up.
    fn finish_if_unset(&mut self, idx: usize, t: Transition) {
        if self.states[idx].transition.is_none() {
            self.states[idx].transition = Some(t);
        }
    }
}

/// One enclosing loop's own bookkeeping — `Intra` (no `await` anywhere in
/// the loop, mirrors `lower.rs::LoopCtx` verbatim: local fixups patched
/// once the loop's own end/cond position is known) or `Inter` (the loop
/// contains an `await`, so `break`/`continue` end the *current* state
/// with a real cross-state `Transition::Jump` instead of a local fixup —
/// `flowwir_lower.rs`'s own module doc, "the two suspending statement
/// shapes" section's sibling for loops).
enum LoopCtx {
    Intra {
        break_fixups: Vec<usize>,
        continue_fixups: Vec<usize>,
        defer_marker: usize,
    },
    Inter {
        cond_state: usize,
        after_state: usize,
        defer_marker: usize,
    },
}

// --- `contains_await`: the intra/inter decision ----------------------------

fn expr_contains_await(e: &TypedExpr) -> bool {
    match &e.kind {
        TypedExprKind::Await(_) => true,
        TypedExprKind::Send(inner) | TypedExprKind::Try(inner, _) => expr_contains_await(inner),
        TypedExprKind::Field(base, _) => expr_contains_await(base),
        TypedExprKind::Binary(_, l, r) => expr_contains_await(l) || expr_contains_await(r),
        TypedExprKind::Index(base, idx) => expr_contains_await(base) || expr_contains_await(idx),
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            receiver.as_deref().is_some_and(expr_contains_await)
                || args.iter().any(|(_, a)| expr_contains_await(a))
        }
        TypedExprKind::Call { receiver, args, .. } => {
            receiver.as_deref().is_some_and(expr_contains_await)
                || args.iter().flatten().any(expr_contains_await)
        }
        _ => false,
    }
}

fn block_contains_await(stmts: &[TypedStmt]) -> bool {
    stmts.iter().any(stmt_contains_await)
}

fn stmt_contains_await(s: &TypedStmt) -> bool {
    match &s.kind {
        TypedStmtKind::Let { value, .. } => expr_contains_await(value),
        TypedStmtKind::Assign { target, value } => {
            expr_contains_await(target) || expr_contains_await(value)
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            expr_contains_await(cond)
                || block_contains_await(then_branch)
                || elifs
                    .iter()
                    .any(|e| expr_contains_await(&e.cond) || block_contains_await(&e.body))
                || else_branch.as_deref().is_some_and(block_contains_await)
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            expr_contains_await(scrutinee)
                || arms.iter().any(|a| {
                    a.guard.as_ref().is_some_and(expr_contains_await)
                        || block_contains_await(&a.body)
                })
        }
        TypedStmtKind::While { cond, body } => {
            expr_contains_await(cond) || block_contains_await(body)
        }
        TypedStmtKind::For { iter, body, .. } => {
            let iter_has = match iter {
                TypedForIter::Range(from, to, _) => {
                    expr_contains_await(from) || expr_contains_await(to)
                }
                TypedForIter::Expr(e) => expr_contains_await(e),
            };
            iter_has || block_contains_await(body)
        }
        TypedStmtKind::Return(value) => value.as_ref().is_some_and(expr_contains_await),
        TypedStmtKind::Assert { cond, message } => {
            expr_contains_await(cond) || message.as_ref().is_some_and(expr_contains_await)
        }
        TypedStmtKind::Defer(body) => match body {
            TypedDeferBody::Expr(e) => expr_contains_await(e),
            TypedDeferBody::Suite(s) => block_contains_await(s),
        },
        TypedStmtKind::ExprStmt(e) => expr_contains_await(e),
        // A `send` never suspends (`emit_send`: a one-way `rt_enqueue`
        // call, never a park) — but its arguments are ordinary
        // expressions that may themselves contain an `await`.
        TypedStmtKind::BareSend { expr, .. } => expr_contains_await(expr),
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            capacity.as_ref().is_some_and(expr_contains_await)
                || deadline.as_ref().is_some_and(expr_contains_await)
                || block_contains_await(body)
        }
        _ => false,
    }
}

// --- small lookup helpers (own copies — see module doc: not a reuse of
// `lower.rs`'s private fns, just the same trivial logic written again) ----

/// This module's own struct `name`, else the imported one (plans/M9.md
/// item EE — mirrors `lower::struct_by_name` / `eval::interp`).
fn struct_by_name<'p>(prog: &'p TypedProgram, name: &str) -> Option<&'p TypedStruct> {
    prog.structs
        .get(name)
        .or_else(|| prog.imported.structs.get(name))
}

fn missing_struct(prog: &TypedProgram, name: &str) -> FlowError {
    if let Some(note) = prog.imported.unresolvable.get(name) {
        return FlowError::unimplemented(format!("`{name}` {note}"));
    }
    FlowError::unimplemented(format!(
        "struct `{name}` is not declared in this module and not present in its import closure"
    ))
}

fn missing_callee(prog: &TypedProgram, key: &CalleeKey) -> FlowError {
    let name = match key {
        CalleeKey::Fn(n) => n.clone(),
        CalleeKey::Method(s, _) => s.clone(),
        CalleeKey::FnInstance(k) | CalleeKey::MethodInstance(k, _) => k
            .strip_prefix("fn:")
            .or_else(|| k.strip_prefix("struct:"))
            .unwrap_or(k)
            .split('[')
            .next()
            .unwrap_or(k)
            .to_string(),
    };
    if let Some(note) = prog.imported.unresolvable.get(&name) {
        return FlowError::unimplemented(format!("`{name}` {note}"));
    }
    match key {
        CalleeKey::FnInstance(_) | CalleeKey::MethodInstance(_, _) => {
            FlowError::unimplemented("calling a generic instantiation from an async body is")
        }
        CalleeKey::Fn(n) => FlowError::unimplemented(format!(
            "calling `{n}` — not declared in this module and not present in its import closure"
        )),
        CalleeKey::Method(s, m) => FlowError::unimplemented(format!(
            "calling `{s}.{m}` — not declared in this module and not present in its import closure"
        )),
    }
}

fn field_index(prog: &TypedProgram, base_ty: &Type, field_name: &str) -> Result<usize, FlowError> {
    // plans/M9.md item C1: `String[..N].len` is slot 0.
    if matches!(base_ty, Type::String(_)) {
        return match field_name {
            "len" => Ok(0),
            other => Err(FlowError::internal(format!(
                "unknown String field `{other}`"
            ))),
        };
    }
    let Type::Named(sname, _) = base_ty else {
        return Err(FlowError::internal("field base is not a `Named` type"));
    };
    let s = struct_by_name(prog, sname).ok_or_else(|| missing_struct(prog, sname))?;
    s.fields
        .iter()
        .position(|f| f == field_name)
        .ok_or_else(|| FlowError::internal(format!("unknown field `{field_name}`")))
}

fn variant_index(prog: &TypedProgram, enum_name: &str, variant: &str) -> Result<usize, FlowError> {
    match enum_name {
        "Option" => match variant {
            "None" => Ok(value::OPTION_NONE),
            "Some" => Ok(value::OPTION_SOME),
            other => Err(FlowError::internal(format!(
                "unknown Option variant `{other}`"
            ))),
        },
        "Result" => match variant {
            "Ok" => Ok(value::RESULT_OK),
            "Err" => Ok(value::RESULT_ERR),
            other => Err(FlowError::internal(format!(
                "unknown Result variant `{other}`"
            ))),
        },
        // plans/M7.md item Z2: `CallError[E]` is compiler-known rather than
        // declared — it is carried as an instantiated
        // `Type::Named("CallError", [E])` and so appears in no
        // `TypedProgram::enums` map, which means the generic-instantiation
        // rejection below used to swallow the one `match` a caller needs to
        // observe `Err(CallError.Op(e))` at all. sema already types such an
        // arm (`bodies::variant_payload_types_for`); only the numbering was
        // missing here. It is not restated: `bodies::call_error_variant_index`
        // is the single table, beside the composition it belongs to.
        "CallError" => crate::sema::bodies::call_error_variant_index(variant)
            .ok_or_else(|| FlowError::internal(format!("unknown CallError variant `{variant}`"))),
        _ => {
            // plans/M9.md item A1b / A2: mirror `eval::interp` /
            // `lower::variant_index` — imported enums live in
            // `prog.imported.enums`, not `prog.enums`.
            let en = prog
                .enums
                .get(enum_name)
                .or_else(|| prog.imported.enums.get(enum_name))
                .ok_or_else(|| {
                    FlowError::unimplemented("matching a generic enum instantiation's variant is")
                })?;
            en.variants
                .iter()
                .position(|v| v == variant)
                .ok_or_else(|| {
                    FlowError::internal(format!("unknown variant `{enum_name}.{variant}`"))
                })
        }
    }
}

fn resolve_callee_fn<'p>(
    prog: &'p TypedProgram,
    key: &CalleeKey,
) -> Result<&'p TypedFn, FlowError> {
    match key {
        CalleeKey::Fn(name) => prog
            .fns
            .get(name)
            .or_else(|| prog.imported.fns.get(name))
            .ok_or_else(|| missing_callee(prog, key)),
        CalleeKey::Method(sname, member) => {
            if let Some(s) = struct_by_name(prog, sname) {
                return s
                    .methods
                    .get(member)
                    .or_else(|| s.assoc_fns.get(member))
                    .or_else(|| {
                        if member == "init" {
                            s.init.as_ref()
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| missing_callee(prog, key));
            }
            let e = prog
                .enums
                .get(sname)
                .or_else(|| prog.imported.enums.get(sname))
                .ok_or_else(|| missing_callee(prog, key))?;
            e.methods
                .get(member)
                .or_else(|| e.assoc_fns.get(member))
                .ok_or_else(|| missing_callee(prog, key))
        }
        CalleeKey::FnInstance(_) | CalleeKey::MethodInstance(_, _) => {
            Err(missing_callee(prog, key))
        }
    }
}

fn eval_array_len(ty: &Type) -> Result<usize, FlowError> {
    match ty {
        Type::Array(_, len_expr) => {
            let n = bodies::literal_array_len(len_expr)
                .ok_or_else(|| FlowError::unimplemented("a non-literal array length is"))?;
            usize::try_from(n).map_err(|_| FlowError::internal("array length out of range"))
        }
        Type::Own(_, inner) => eval_array_len(inner),
        _ => Err(FlowError::unimplemented("indexing a non-array value is")),
    }
}

fn assert_message_text(e: &TypedExpr) -> Result<String, FlowError> {
    if let TypedExprKind::Str(text) = &e.kind {
        Ok(String::from_utf8_lossy(&value::decode_str(text)).into_owned())
    } else {
        Err(FlowError::unimplemented(
            "a non-literal `assert`/`panic` message is",
        ))
    }
}

// --- entry point ------------------------------------------------------------

/// Every async fn/method's own state machine, keyed exactly like
/// `mwir::MwirProgram::fns` (`sema::typed::CalleeKey::spelling()`). A
/// generic instantiation is never walked at all (module doc's own
/// disclosed boundary — no async generic exists in the M6 surface).
pub fn lower_program(program: &TypedProgram) -> Result<FlowWirProgram, FlowError> {
    let mut fns = BTreeMap::new();
    for (name, f) in &program.fns {
        if f.is_async {
            fns.insert(name.clone(), lower_fn(f, program)?);
        }
    }
    for (sname, s) in &program.structs {
        for (member, f) in &s.methods {
            if f.is_async {
                fns.insert(format!("{sname}.{member}"), lower_fn(f, program)?);
            }
        }
        for (member, f) in &s.assoc_fns {
            if f.is_async {
                fns.insert(format!("{sname}.{member}"), lower_fn(f, program)?);
            }
        }
        if let Some(f) = &s.init {
            if f.is_async {
                fns.insert(format!("{sname}.init"), lower_fn(f, program)?);
            }
        }
    }
    // plans/M9.md item EE / decision 90: same imported emission
    // `lower::lower_program` does, for async members only.
    for (name, f) in &program.imported.fns {
        if f.is_async && !fns.contains_key(name) {
            fns.insert(name.clone(), lower_fn(f, program)?);
        }
    }
    for (sname, s) in &program.imported.structs {
        for (member, f) in &s.methods {
            let key = format!("{sname}.{member}");
            if f.is_async && !fns.contains_key(&key) {
                fns.insert(key, lower_fn(f, program)?);
            }
        }
        for (member, f) in &s.assoc_fns {
            let key = format!("{sname}.{member}");
            if f.is_async && !fns.contains_key(&key) {
                fns.insert(key, lower_fn(f, program)?);
            }
        }
        if let Some(f) = &s.init {
            let key = format!("{sname}.init");
            if f.is_async && !fns.contains_key(&key) {
                fns.insert(key, lower_fn(f, program)?);
            }
        }
    }
    Ok(FlowWirProgram { fns })
}

fn lower_fn(f: &TypedFn, prog: &TypedProgram) -> Result<FlowWirFn, FlowError> {
    let mut b = FlowBuilder {
        prog,
        ret: f.ret.clone(),
        temp_types: Vec::new(),
        states: Vec::new(),
        cur: 0,
    };
    // Lineage slots: always Temp(0)/Temp(1), allocated before anything
    // else (flowwir.rs's own "Lineage/deadline plumbing" section).
    let lineage_group_slot = b.fresh(Type::U64);
    let lineage_deadline_slot = b.fresh(Type::U64);

    let mut env: FEnv = vec![BTreeMap::new()];
    let receiver = match &f.receiver {
        Some((mode, ty)) => {
            let t = b.fresh(ty.clone());
            env_insert(&mut env, "self".to_string(), Binding::Temp(t));
            Some((t, *mode))
        }
        None => None,
    };
    let mut params = Vec::with_capacity(f.params.len());
    for p in &f.params {
        let t = b.fresh(p.ty.clone());
        env_insert(&mut env, p.name.clone(), Binding::Temp(t));
        params.push((t, p.mode));
    }

    let entry = b.new_state();
    debug_assert_eq!(entry, 0, "the entry state must be state 0");
    b.switch_to(entry);

    let mut defers: Vec<&TypedDeferBody> = Vec::new();
    let mut loops: Vec<LoopCtx> = Vec::new();
    let _diverged = lower_block(&f.body, &mut b, &mut env, &mut defers, &mut loops)?;
    // Whether the body's own top-level flow fell off the end normally or
    // already diverged via an embedded `Mwir(Inst::Return)` (a plain
    // `return`, or an intra `if`/`match` every arm of which ended that
    // way), the current state still needs *some* transition
    // (`finish_if_unset`'s own doc comment) — `Return(None)` is the exact
    // right one in the falls-off-the-end case, and a harmless, dead-code
    // placeholder otherwise (mirrors `lower.rs::lower_fn`'s own trailing
    // bare `Inst::Return`).
    let c = b.cur();
    b.finish_if_unset(c, Transition::Return(None));

    let FlowBuilder {
        temp_types, states, ..
    } = b;
    let frame = FrameLayout {
        temp_types,
        lineage_group_slot,
        lineage_deadline_slot,
    };
    let states = states
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            let transition = s
                .transition
                .ok_or_else(|| FlowError::internal(format!("state {i} never got a transition")))?;
            Ok(State {
                ops: s.ops,
                transition,
            })
        })
        .collect::<Result<Vec<State>, FlowError>>()?;

    Ok(FlowWirFn {
        receiver,
        params,
        ret: f.ret.clone(),
        frame,
        states,
    })
}

// --- statements --------------------------------------------------------

/// Lowers a block, draining (inline — `drain_defers_inline`) whatever
/// `defer`s it registered of its own once it reaches its own natural end
/// (mirrors `lower.rs::lower_block` exactly). `lower_with_group` calls
/// `lower_stmts_no_drain` directly instead, so it can drain its own
/// group-scoped `defer`s as a referenceable cleanup chain rather than
/// inline (`flowwir.rs`'s own "`with group`" section).
fn lower_block<'a>(
    stmts: &'a [TypedStmt],
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, FlowError> {
    let start = defers.len();
    let diverged = lower_stmts_no_drain(stmts, b, env, defers, loops)?;
    if !diverged {
        let active: Vec<&TypedDeferBody> = defers[start..].to_vec();
        drain_defers_inline(&active, b, env)?;
    }
    defers.truncate(start);
    Ok(diverged)
}

fn lower_stmts_no_drain<'a>(
    stmts: &'a [TypedStmt],
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, FlowError> {
    for s in stmts {
        if lower_stmt(s, b, env, defers, loops)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Inlines every defer body in `active`, in reverse (registration) order,
/// as ordinary ops in the *current* state (mirrors `lower.rs::run_defers`
/// exactly) — used for every `defer` outside a `with group`'s own body
/// (module doc: not part of any cancellation domain at M6, so nothing
/// needs to reference it independently).
fn drain_defers_inline(
    active: &[&TypedDeferBody],
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<(), FlowError> {
    for d in active.iter().rev() {
        match d {
            TypedDeferBody::Expr(e) => {
                if expr_contains_await(e) {
                    return Err(FlowError::unimplemented(
                        "a `defer` body containing an `await` is",
                    ));
                }
                lower_expr_flat(e, b, env)?;
            }
            TypedDeferBody::Suite(stmts) => {
                if block_contains_await(stmts) {
                    return Err(FlowError::unimplemented(
                        "a `defer` body containing an `await` is",
                    ));
                }
                let mut inner_defers: Vec<&TypedDeferBody> = Vec::new();
                let mut inner_loops: Vec<LoopCtx> = Vec::new();
                lower_block(stmts, b, env, &mut inner_defers, &mut inner_loops)?;
            }
        }
    }
    Ok(())
}

/// Builds a fresh, dedicated state per defer body in `active` (reverse
/// order), each ending in a real `Transition::Jump` — the caller
/// (`lower_with_group`) chains them together and into whatever comes
/// next. Every state's own ops come from the identical straight-line
/// lowering `drain_defers_inline` would use inline; the only difference
/// is *where* they land (their own referenceable states, not the
/// caller's current one) — `flowwir.rs`'s own "`with group`" section.
fn build_cleanup_chain(
    active: &[&TypedDeferBody],
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<Vec<usize>, FlowError> {
    let mut indices = Vec::with_capacity(active.len());
    for d in active.iter().rev() {
        let st = b.new_state();
        b.switch_to(st);
        match d {
            TypedDeferBody::Expr(e) => {
                if expr_contains_await(e) {
                    return Err(FlowError::unimplemented(
                        "a `defer` body containing an `await` is",
                    ));
                }
                lower_expr_flat(e, b, env)?;
            }
            TypedDeferBody::Suite(stmts) => {
                if block_contains_await(stmts) {
                    return Err(FlowError::unimplemented(
                        "a `defer` body containing an `await` is",
                    ));
                }
                let mut inner_defers: Vec<&TypedDeferBody> = Vec::new();
                let mut inner_loops: Vec<LoopCtx> = Vec::new();
                lower_block(stmts, b, env, &mut inner_defers, &mut inner_loops)?;
            }
        }
        indices.push(st);
    }
    Ok(indices)
}

fn lower_stmt<'a>(
    stmt: &'a TypedStmt,
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, FlowError> {
    match &stmt.kind {
        TypedStmtKind::Let { name, ty, value } => {
            if let Some(path) = self_path_of(value) {
                if !path.is_empty() {
                    env_insert(env, name.clone(), Binding::SelfPath(path, ty.clone()));
                    return Ok(false);
                }
            }
            let v = lower_stmt_operand(value, b, env)?;
            let t = b.fresh(ty.clone());
            b.emit_mwir(Inst::Copy { dst: t, src: v });
            env_insert(env, name.clone(), Binding::Temp(t));
            Ok(false)
        }
        TypedStmtKind::Assign { target, value } => {
            let v = lower_stmt_operand(value, b, env)?;
            lower_place_write(target, v, b, env)?;
            Ok(false)
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => lower_if(cond, then_branch, elifs, else_branch, b, env, defers, loops),
        TypedStmtKind::Match { scrutinee, arms } => {
            if expr_contains_await(scrutinee)
                || arms.iter().any(|a| {
                    a.guard.as_ref().is_some_and(expr_contains_await)
                        || block_contains_await(&a.body)
                })
            {
                return Err(FlowError::unimplemented(
                    "a `match` containing an `await` (in its scrutinee, a guard, or an arm) is",
                ));
            }
            lower_match(scrutinee, arms, b, env, defers, loops)
        }
        TypedStmtKind::While { cond, body } => {
            lower_while(cond, body, b, env, defers, loops)?;
            Ok(false)
        }
        TypedStmtKind::For {
            name,
            elem_ty,
            iter,
            body,
            ..
        } => {
            let iter_has = match iter {
                TypedForIter::Range(from, to, _) => {
                    expr_contains_await(from) || expr_contains_await(to)
                }
                TypedForIter::Expr(e) => expr_contains_await(e),
            };
            if iter_has || block_contains_await(body) {
                return Err(FlowError::unimplemented(
                    "a `for` loop containing an `await` is",
                ));
            }
            lower_for(name, elem_ty, iter, body, b, env, defers, loops)?;
            Ok(false)
        }
        TypedStmtKind::Break => match loops.last() {
            Some(LoopCtx::Intra { defer_marker, .. }) => {
                let marker = *defer_marker;
                let active: Vec<&TypedDeferBody> = defers[marker..].to_vec();
                drain_defers_inline(&active, b, env)?;
                let idx = b.emit_mwir(Inst::Jump { target: usize::MAX });
                if let Some(LoopCtx::Intra { break_fixups, .. }) = loops.last_mut() {
                    break_fixups.push(idx);
                }
                Ok(true)
            }
            Some(LoopCtx::Inter {
                after_state,
                defer_marker,
                ..
            }) => {
                let marker = *defer_marker;
                let after = *after_state;
                let active: Vec<&TypedDeferBody> = defers[marker..].to_vec();
                drain_defers_inline(&active, b, env)?;
                b.finish_current(Transition::Jump(after));
                Ok(true)
            }
            None => Err(FlowError::internal("`break` outside a loop")),
        },
        TypedStmtKind::Continue => match loops.last() {
            Some(LoopCtx::Intra { defer_marker, .. }) => {
                let marker = *defer_marker;
                let active: Vec<&TypedDeferBody> = defers[marker..].to_vec();
                drain_defers_inline(&active, b, env)?;
                let idx = b.emit_mwir(Inst::Jump { target: usize::MAX });
                if let Some(LoopCtx::Intra {
                    continue_fixups, ..
                }) = loops.last_mut()
                {
                    continue_fixups.push(idx);
                }
                Ok(true)
            }
            Some(LoopCtx::Inter {
                cond_state,
                defer_marker,
                ..
            }) => {
                let marker = *defer_marker;
                let cond = *cond_state;
                let active: Vec<&TypedDeferBody> = defers[marker..].to_vec();
                drain_defers_inline(&active, b, env)?;
                b.finish_current(Transition::Jump(cond));
                Ok(true)
            }
            None => Err(FlowError::internal("`continue` outside a loop")),
        },
        TypedStmtKind::Pass => Ok(false),
        TypedStmtKind::Return(value) => {
            let v = match value {
                Some(e) => Some(lower_stmt_operand(e, b, env)?),
                None => None,
            };
            let active: Vec<&TypedDeferBody> = defers[..].to_vec();
            drain_defers_inline(&active, b, env)?;
            b.emit_mwir(Inst::Return { value: v });
            Ok(true)
        }
        TypedStmtKind::Assert { cond, message } => {
            let c = lower_expr_flat(cond, b, env)?;
            let fail_fixup = b.emit_mwir(Inst::JumpIfFalse {
                cond: c,
                target: usize::MAX,
            });
            let after_fixup = b.emit_mwir(Inst::Jump { target: usize::MAX });
            let fail_pos = b.here();
            b.patch(fail_fixup, fail_pos);
            let msg = match message {
                Some(m) => Some(assert_message_text(m)?),
                None => None,
            };
            b.emit_mwir(Inst::AssertFail { message: msg });
            let after_pos = b.here();
            b.patch(after_fixup, after_pos);
            Ok(false)
        }
        TypedStmtKind::ComptimeAssert { .. } => Ok(false),
        TypedStmtKind::Defer(body) => {
            defers.push(body);
            Ok(false)
        }
        TypedStmtKind::ExprStmt(e) => {
            lower_expr_stmt(e, b, env)?;
            Ok(false)
        }
        // plans/M6.md item G: a proven bare `send` lowers exactly like the
        // consumed expression form — `FlowInst::Send` still writes its
        // `Result[unit, Rejected]` outcome into a fresh temp; the only
        // difference is that nothing reads that temp. Deliberately NOT a
        // second lowering path: a proven send and an unproven one must
        // execute identically (the proof is a legality verdict, never a
        // codegen switch), so the same instruction is emitted either way
        // and `codegen::emit_send` never learns the proof exists.
        TypedStmtKind::BareSend { expr, .. } => {
            lower_expr_flat(expr, b, env)?;
            Ok(false)
        }
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            as_name,
            body,
        } => lower_with_group(capacity, deadline, as_name, body, b, env, defers, loops),
    }
}

/// A bare `ExprStmt`'s own three special shapes (`Group.start`, a bare
/// `await`, a bare `await ...?`) plus the ordinary fallback — mirrors
/// `lower_stmt_operand`'s own recognition, but discards the result
/// (nothing binds it).
fn lower_expr_stmt(e: &TypedExpr, b: &mut FlowBuilder, env: &mut FEnv) -> Result<(), FlowError> {
    if let TypedExprKind::Intrinsic {
        key,
        receiver: Some(recv),
        args,
        ..
    } = &e.kind
    {
        if key.as_str() == "Group.start" {
            return lower_group_start(recv, args, b, env);
        }
    }
    lower_stmt_operand(e, b, env)?;
    Ok(())
}

/// Lowers a `let`/assignment/bare-statement's own operand, recognizing
/// the two suspending shapes a statement position may carry (module
/// doc): `await ...` directly, or `await ...?` (a `Try` wrapping an
/// `Await`) — anything else (including an *ordinary*, already-synchronous
/// `?`) falls through to `lower_expr_flat`.
fn lower_stmt_operand(
    value: &TypedExpr,
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<Temp, FlowError> {
    match &value.kind {
        TypedExprKind::Await(_) => {
            let (what, ty) = build_await_kind(value, b, env)?;
            let result_temp = b.fresh(ty);
            suspend_and_resume(what, result_temp, b);
            Ok(result_temp)
        }
        TypedExprKind::Try(inner, conv) if matches!(inner.kind, TypedExprKind::Await(_)) => {
            let (what, ty) = build_await_kind(inner, b, env)?;
            let result_temp = b.fresh(ty.clone());
            suspend_and_resume(what, result_temp, b);
            lower_try_check(result_temp, &ty, conv, b)
        }
        _ => lower_expr_flat(value, b, env),
    }
}

fn suspend_and_resume(what: AwaitKind, result_temp: Temp, b: &mut FlowBuilder) {
    let resume = b.new_state();
    b.finish_current(Transition::Await {
        what,
        resume_state: resume,
        result_temp,
    });
    b.switch_to(resume);
}

/// Builds the `AwaitKind` for `await_expr` (a `TypedExprKind::Await`
/// node) — an actor-handle method call, or a group's own `join_all()`
/// (`sema::bodies::check_await`'s own two recognized shapes, mirrored
/// here one stage later). Returns the await's own composed result type
/// alongside (`await_expr.ty`).
fn build_await_kind(
    await_expr: &TypedExpr,
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<(AwaitKind, Type), FlowError> {
    let TypedExprKind::Await(inner) = &await_expr.kind else {
        return Err(FlowError::internal(
            "build_await_kind called on a non-`Await` node",
        ));
    };
    match &inner.kind {
        TypedExprKind::Call {
            callee,
            receiver: Some(recv),
            args,
        } => {
            let target_temp = lower_expr_flat(recv, b, env)?;
            let method_key = callee.spelling();
            let f = resolve_callee_fn(b.prog, callee)?;
            let arg_temps = lower_aligned_args(f, args, b, env)?;
            Ok((
                AwaitKind::ActorCall {
                    target_temp,
                    method_key,
                    arg_temps,
                },
                await_expr.ty.clone(),
            ))
        }
        TypedExprKind::Intrinsic {
            key,
            receiver: Some(recv),
            ..
        } if key.as_str() == "Group.join_all" => {
            let TypedExprKind::Local(gname) = &recv.kind else {
                return Err(FlowError::internal(
                    "`Group.join_all`'s receiver is not a bare local",
                ));
            };
            let group_temp = match env_lookup(env, gname) {
                Some(Binding::Temp(t)) => t,
                _ => return Err(FlowError::internal(format!("group `{gname}` is not bound"))),
            };
            let child_count = match &await_expr.ty {
                Type::Array(_, len_expr) => {
                    bodies::literal_array_len(len_expr).ok_or_else(|| {
                        FlowError::internal("group join's array length is not a literal")
                    })? as usize
                }
                _ => {
                    return Err(FlowError::internal(
                        "`g.join_all()`'s composed type is not an array",
                    ));
                }
            };
            Ok((
                AwaitKind::GroupJoin {
                    group_temp,
                    child_count,
                },
                await_expr.ty.clone(),
            ))
        }
        // plans/M7.md item E4: `await receipt` — inner is already the
        // Receipt value (not a call).
        _ => {
            let receipt_temp = lower_expr_flat(inner, b, env)?;
            if !matches!(&inner.ty, Type::Named(n, _) if n == "Receipt") {
                return Err(FlowError::unimplemented(
                    "an `await` target other than an actor call, a group's `join_all()`, or a \
                     `Receipt[P]` is",
                ));
            }
            Ok((AwaitKind::Receipt { receipt_temp }, await_expr.ty.clone()))
        }
    }
}

/// Aligns a `Call`'s own `args` (already 1:1 with the callee's declared
/// parameters, `None` for a caller-elided default slot) against `f`'s own
/// stored defaults — mirrors `lower.rs::bind_args`'s alignment, simplified:
/// a default expression lowers in the *caller's* current environment
/// here (not a separate callee-shaped one) — a real message-shaped call's
/// own default, if any, is not expected to reference the remote actor's
/// own `self`/earlier params the way an ordinary in-process call's might;
/// no required golden exercises a defaulted message argument, so this is
/// a disclosed simplification, not a proven equivalence.
fn lower_aligned_args(
    f: &TypedFn,
    args: &[Option<TypedExpr>],
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<Vec<Temp>, FlowError> {
    let mut out = Vec::with_capacity(args.len());
    for (param, slot) in f.params.iter().zip(args.iter()) {
        let t = match slot {
            Some(e) if param.mode == AccessMode::Mut => {
                let TypedExprKind::Local(name) = &e.kind else {
                    return Err(FlowError::unimplemented(
                        "passing a `mut` argument through a nested field/index place \
                         inside an async body is",
                    ));
                };
                match env_lookup(env, name) {
                    Some(Binding::Temp(t)) => t,
                    _ => {
                        return Err(FlowError::internal(format!(
                            "unbound (or self-path) local `{name}` as `mut` argument"
                        )));
                    }
                }
            }
            Some(e) => lower_expr_flat(e, b, env)?,
            None if param.mode == AccessMode::Mut => {
                return Err(FlowError::unimplemented(
                    "writing back a `mut` parameter through a defaulted argument is",
                ));
            }
            None => {
                let default = param.default.as_ref().ok_or_else(|| {
                    FlowError::internal(format!(
                        "missing arg `{}` with no stored default",
                        param.name
                    ))
                })?;
                lower_expr_flat(default, b, env)?
            }
        };
        out.push(t);
    }
    Ok(out)
}

fn flow_call_write_backs(
    f: &TypedFn,
    receiver_temp: Option<Temp>,
    arg_temps: &[Temp],
) -> Vec<(usize, Temp)> {
    let mut write_backs = Vec::new();
    let arg0_is_receiver = receiver_temp.is_some();
    if let Some(st) = receiver_temp {
        if matches!(f.receiver.as_ref().map(|(m, _)| *m), Some(AccessMode::Mut)) {
            write_backs.push((0, st));
        }
    }
    for (i, param) in f.params.iter().enumerate() {
        if param.mode == AccessMode::Mut {
            let args_idx = if arg0_is_receiver { i + 1 } else { i };
            write_backs.push((args_idx, arg_temps[i]));
        }
    }
    write_backs
}

/// Mirrors `lower.rs::lower_call`'s receiver/write-back shape, using this
/// file's `Binding::Temp` environment.
fn lower_flow_call(
    callee: &CalleeKey,
    receiver: &Option<Box<TypedExpr>>,
    args: &[Option<TypedExpr>],
    _result_ty: &Type,
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<Temp, FlowError> {
    let f = resolve_callee_fn(b.prog, callee)?;
    let key = callee.spelling();
    let mode = f.receiver.as_ref().map(|(m, _)| *m);
    match (receiver, mode) {
        (Some(recv_expr), Some(AccessMode::Mut)) => {
            let TypedExprKind::Local(recv_name) = &recv_expr.kind else {
                return Err(FlowError::unimplemented(
                    "calling a `mut self` method through a nested field/index receiver \
                     inside an async body is",
                ));
            };
            let self_temp = match env_lookup(env, recv_name) {
                Some(Binding::Temp(t)) => t,
                _ => {
                    return Err(FlowError::internal(format!(
                        "unbound (or self-path) local `{recv_name}` as mut receiver"
                    )));
                }
            };
            let arg_temps = lower_aligned_args(f, args, b, env)?;
            let write_backs = flow_call_write_backs(f, Some(self_temp), &arg_temps);
            let mut call_args = vec![self_temp];
            call_args.extend(arg_temps);
            let dst = b.fresh(f.ret.clone());
            b.emit_mwir(Inst::Call {
                dst,
                write_backs,
                key,
                args: call_args,
            });
            Ok(dst)
        }
        (Some(recv_expr), Some(AccessMode::Read | AccessMode::Take)) => {
            let recv_temp = lower_expr_flat(recv_expr, b, env)?;
            let arg_temps = lower_aligned_args(f, args, b, env)?;
            let write_backs = flow_call_write_backs(f, Some(recv_temp), &arg_temps);
            let mut call_args = vec![recv_temp];
            call_args.extend(arg_temps);
            let dst = b.fresh(f.ret.clone());
            b.emit_mwir(Inst::Call {
                dst,
                write_backs,
                key,
                args: call_args,
            });
            Ok(dst)
        }
        _ => {
            let arg_temps = lower_aligned_args(f, args, b, env)?;
            let write_backs = flow_call_write_backs(f, None, &arg_temps);
            let dst = b.fresh(f.ret.clone());
            b.emit_mwir(Inst::Call {
                dst,
                write_backs,
                key,
                args: arg_temps,
            });
            Ok(dst)
        }
    }
}

/// `g.start(callee, args...)` (02-language.md §9.5) — `args` here is
/// `Intrinsic::args` (label, value) *without* the leading `"callee"`
/// slot (`sema::bodies::check_group_start`'s own doc comment: an
/// omitted, defaulted argument is elided entirely from this list, unlike
/// an ordinary `Call`'s aligned `args`), so each of the callee's own
/// declared parameters is matched by name here, falling back to its own
/// stored default when absent.
fn lower_group_start(
    recv: &TypedExpr,
    args: &[(String, TypedExpr)],
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<(), FlowError> {
    let TypedExprKind::Local(gname) = &recv.kind else {
        return Err(FlowError::internal(
            "`Group.start`'s receiver is not a bare local",
        ));
    };
    let group_temp = match env_lookup(env, gname) {
        Some(Binding::Temp(t)) => t,
        _ => return Err(FlowError::internal(format!("group `{gname}` is not bound"))),
    };
    let (callee_arg, rest) = args
        .split_first()
        .ok_or_else(|| FlowError::internal("`Group.start` has no callee argument"))?;
    let (label, callee_expr) = callee_arg;
    if label != "callee" {
        return Err(FlowError::internal(
            "`Group.start`'s first argument is not its callee",
        ));
    }
    let TypedExprKind::GroupChild(key) = &callee_expr.kind else {
        return Err(FlowError::internal(
            "`Group.start`'s callee is not a `GroupChild` node",
        ));
    };
    let f = resolve_callee_fn(b.prog, key)?;
    let mut arg_temps = Vec::with_capacity(f.params.len());
    for p in &f.params {
        let found = rest.iter().find(|(n, _)| n == &p.name);
        let t = match found {
            Some((_, e)) => lower_expr_flat(e, b, env)?,
            None => {
                let default = p.default.as_ref().ok_or_else(|| {
                    FlowError::internal(format!("missing group-child arg `{}`", p.name))
                })?;
                lower_expr_flat(default, b, env)?
            }
        };
        arg_temps.push(t);
    }
    b.emit(FlowInst::GroupStart {
        group_temp,
        callee_key: key.spelling(),
        arg_temps,
    });
    Ok(())
}

/// `with group(capacity=.., deadline=..) [as g]:` (02-language.md §9.5,
/// `flowwir.rs`'s own "`with group`" section). The group's own body never
/// drains its `defer`s inline (`lower_stmts_no_drain`, not `lower_block`)
/// — this fn does that itself, via a referenceable cleanup chain
/// (`build_cleanup_chain`) when the body registered any, or a bare,
/// empty-cleanup `GroupClose` when it did not. An early exit (`return`)
/// from inside the body is a disclosed, named gap (module doc's own
/// "Fail-closed set" list is `flowwir.rs`'s, not repeated here): this fn
/// only ever emits `GroupClose` on the body's own natural, non-diverging
/// end.
#[allow(clippy::too_many_arguments)]
fn lower_with_group<'a>(
    capacity: &'a Option<TypedExpr>,
    deadline: &'a Option<TypedExpr>,
    as_name: &Option<String>,
    body: &'a [TypedStmt],
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, FlowError> {
    let cap_t = match capacity {
        Some(e) => Some(lower_expr_flat(e, b, env)?),
        None => None,
    };
    let dl_t = match deadline {
        Some(e) => Some(lower_expr_flat(e, b, env)?),
        None => None,
    };
    let group_temp = b.fresh(Type::Named("Group".to_string(), vec![]));
    b.emit(FlowInst::GroupCreate {
        group_temp,
        capacity: cap_t,
        deadline: dl_t,
    });

    env.push(BTreeMap::new());
    if let Some(name) = as_name {
        env_insert(env, name.clone(), Binding::Temp(group_temp));
    }
    let group_marker = defers.len();
    let diverged = lower_stmts_no_drain(body, b, env, defers, loops)?;
    if !diverged {
        let active: Vec<&TypedDeferBody> = defers[group_marker..].to_vec();
        if active.is_empty() {
            b.emit(FlowInst::GroupClose {
                group_temp,
                cleanup_states: Vec::new(),
            });
        } else {
            let original_end = b.cur();
            let chain = build_cleanup_chain(&active, b, env)?;
            b.emit_at(
                original_end,
                FlowInst::GroupClose {
                    group_temp,
                    cleanup_states: chain.clone(),
                },
            );
            let after = b.new_state();
            for w in chain.windows(2) {
                b.finish(w[0], Transition::Jump(w[1]));
            }
            let last = *chain.last().expect("checked non-empty above");
            b.finish(last, Transition::Jump(after));
            b.finish(original_end, Transition::Jump(chain[0]));
            b.switch_to(after);
        }
    } else {
        // Module doc's own disclosed gap: an early exit from inside the
        // body never runs this group's own `GroupClose`/cleanup chain at
        // M6 (item F's job). The ending state may still be missing a
        // transition (an embedded `Mwir(Inst::Return)`, not an explicit
        // cross-state exit) — `finish_if_unset` keeps the IR well-formed
        // either way, same reasoning as `lower_fn`'s own trailing step.
        let c = b.cur();
        b.finish_if_unset(c, Transition::Return(None));
    }
    defers.truncate(group_marker);
    env.pop();
    Ok(diverged)
}

// --- if/while (intra vs. inter) --------------------------------------------

#[allow(clippy::too_many_arguments)]
fn lower_if<'a>(
    cond: &'a TypedExpr,
    then_branch: &'a [TypedStmt],
    elifs: &'a [TypedElif],
    else_branch: &'a Option<Vec<TypedStmt>>,
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, FlowError> {
    let has_await = expr_contains_await(cond)
        || block_contains_await(then_branch)
        || elifs
            .iter()
            .any(|e| expr_contains_await(&e.cond) || block_contains_await(&e.body))
        || else_branch.as_deref().is_some_and(block_contains_await);
    if !has_await {
        return lower_if_intra(cond, then_branch, elifs, else_branch, b, env, defers, loops);
    }
    if !elifs.is_empty() {
        return Err(FlowError::unimplemented(
            "an `elif` chain where any branch contains an `await` is",
        ));
    }
    if expr_contains_await(cond) {
        return Err(FlowError::unimplemented(
            "an `await` inside an `if`'s own condition is",
        ));
    }
    lower_if_split(cond, then_branch, else_branch, b, env, defers, loops)
}

#[allow(clippy::too_many_arguments)]
fn lower_if_intra<'a>(
    cond: &'a TypedExpr,
    then_branch: &'a [TypedStmt],
    elifs: &'a [TypedElif],
    else_branch: &'a Option<Vec<TypedStmt>>,
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, FlowError> {
    let mut end_fixups = Vec::new();
    let mut all_diverge = true;

    let c = lower_expr_flat(cond, b, env)?;
    let mut next_fixup = b.emit_mwir(Inst::JumpIfFalse {
        cond: c,
        target: usize::MAX,
    });
    env.push(BTreeMap::new());
    let d = lower_block(then_branch, b, env, defers, loops)?;
    env.pop();
    if !d {
        all_diverge = false;
    }
    end_fixups.push(b.emit_mwir(Inst::Jump { target: usize::MAX }));
    let mut pos = b.here();
    b.patch(next_fixup, pos);

    for elif in elifs {
        let c2 = lower_expr_flat(&elif.cond, b, env)?;
        next_fixup = b.emit_mwir(Inst::JumpIfFalse {
            cond: c2,
            target: usize::MAX,
        });
        env.push(BTreeMap::new());
        let d2 = lower_block(&elif.body, b, env, defers, loops)?;
        env.pop();
        if !d2 {
            all_diverge = false;
        }
        end_fixups.push(b.emit_mwir(Inst::Jump { target: usize::MAX }));
        pos = b.here();
        b.patch(next_fixup, pos);
    }

    match else_branch {
        Some(eb) => {
            env.push(BTreeMap::new());
            let de = lower_block(eb, b, env, defers, loops)?;
            env.pop();
            if !de {
                all_diverge = false;
            }
        }
        None => all_diverge = false,
    }

    let end_pos = b.here();
    for idx in end_fixups {
        b.patch(idx, end_pos);
    }
    Ok(all_diverge)
}

fn lower_if_split<'a>(
    cond: &'a TypedExpr,
    then_branch: &'a [TypedStmt],
    else_branch: &'a Option<Vec<TypedStmt>>,
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, FlowError> {
    let c = lower_expr_flat(cond, b, env)?;
    let then_state = b.new_state();
    let else_state = b.new_state();
    b.finish_current(Transition::Branch {
        cond_temp: c,
        then_state,
        else_state,
    });

    b.switch_to(then_state);
    env.push(BTreeMap::new());
    let then_diverged = lower_block(then_branch, b, env, defers, loops)?;
    env.pop();
    let then_end = b.cur();

    b.switch_to(else_state);
    let else_diverged = match else_branch {
        Some(eb) => {
            env.push(BTreeMap::new());
            let d = lower_block(eb, b, env, defers, loops)?;
            env.pop();
            d
        }
        None => false,
    };
    let else_end = b.cur();

    if then_diverged {
        // Already finished if it diverged via an explicit cross-state
        // `break`/`continue`; not yet if via an embedded
        // `Mwir(Inst::Return)` — `finish_if_unset` covers both.
        b.finish_if_unset(then_end, Transition::Return(None));
    }
    if else_diverged {
        b.finish_if_unset(else_end, Transition::Return(None));
    }
    if then_diverged && else_diverged {
        return Ok(true);
    }
    let after = b.new_state();
    if !then_diverged {
        b.finish(then_end, Transition::Jump(after));
    }
    if !else_diverged {
        b.finish(else_end, Transition::Jump(after));
    }
    b.switch_to(after);
    Ok(false)
}

fn lower_while<'a>(
    cond: &'a TypedExpr,
    body: &'a [TypedStmt],
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<(), FlowError> {
    let has_await = expr_contains_await(cond) || block_contains_await(body);
    if !has_await {
        return lower_while_intra(cond, body, b, env, defers, loops);
    }
    if expr_contains_await(cond) {
        return Err(FlowError::unimplemented(
            "an `await` inside a `while` loop's own condition is",
        ));
    }
    lower_while_split(body, cond, b, env, defers, loops)
}

fn lower_while_intra<'a>(
    cond: &'a TypedExpr,
    body: &'a [TypedStmt],
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<(), FlowError> {
    loops.push(LoopCtx::Intra {
        break_fixups: Vec::new(),
        continue_fixups: Vec::new(),
        defer_marker: defers.len(),
    });
    let cond_pos = b.here();
    let c = lower_expr_flat(cond, b, env)?;
    let end_fixup = b.emit_mwir(Inst::JumpIfFalse {
        cond: c,
        target: usize::MAX,
    });
    env.push(BTreeMap::new());
    lower_block(body, b, env, defers, loops)?;
    env.pop();
    b.emit_mwir(Inst::Jump { target: cond_pos });
    let end_pos = b.here();
    b.patch(end_fixup, end_pos);
    let ctx = loops.pop().expect("pushed above");
    let LoopCtx::Intra {
        break_fixups,
        continue_fixups,
        ..
    } = ctx
    else {
        unreachable!("this fn only ever pushes LoopCtx::Intra")
    };
    for idx in break_fixups {
        b.patch(idx, end_pos);
    }
    for idx in continue_fixups {
        b.patch(idx, cond_pos);
    }
    Ok(())
}

/// A loop's back-edge with an `await` inside it: a genuine state cycle
/// (`flowwir.rs`'s own "loop back-edge with an await inside = state
/// cycle") — `cond_state` re-checks the condition every iteration,
/// `Branch`ing into either a fresh `body_state` or the loop's own
/// `after_state`; the body's own natural (non-`break`/`continue`) end
/// jumps straight back to `cond_state`.
fn lower_while_split<'a>(
    body: &'a [TypedStmt],
    cond: &'a TypedExpr,
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<(), FlowError> {
    let cond_state = b.new_state();
    b.finish_current(Transition::Jump(cond_state));
    b.switch_to(cond_state);
    let c = lower_expr_flat(cond, b, env)?;
    let body_state = b.new_state();
    let after_state = b.new_state();
    b.finish_current(Transition::Branch {
        cond_temp: c,
        then_state: body_state,
        else_state: after_state,
    });
    b.switch_to(body_state);
    loops.push(LoopCtx::Inter {
        cond_state,
        after_state,
        defer_marker: defers.len(),
    });
    env.push(BTreeMap::new());
    let diverged = lower_block(body, b, env, defers, loops)?;
    env.pop();
    loops.pop();
    if !diverged {
        b.finish_current(Transition::Jump(cond_state));
    } else {
        // Diverged either via an explicit cross-state `break`/`continue`
        // (which already finished its own state) or via an embedded
        // `Mwir(Inst::Return)` (which did not) — `finish_if_unset` covers
        // both uniformly, same reasoning as `lower_fn`'s own trailing
        // step.
        let c = b.cur();
        b.finish_if_unset(c, Transition::Return(None));
    }
    b.switch_to(after_state);
    Ok(())
}

// --- for (intra only) -------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn lower_for<'a>(
    name: &str,
    elem_ty: &Type,
    iter: &'a TypedForIter,
    body: &'a [TypedStmt],
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<(), FlowError> {
    loops.push(LoopCtx::Intra {
        break_fixups: Vec::new(),
        continue_fixups: Vec::new(),
        defer_marker: defers.len(),
    });
    match iter {
        TypedForIter::Range(from, to, inclusive) => {
            let from_t = lower_expr_flat(from, b, env)?;
            let to_t = lower_expr_flat(to, b, env)?;
            let i_temp = b.fresh(elem_ty.clone());
            b.emit_mwir(Inst::Copy {
                dst: i_temp,
                src: from_t,
            });
            let cond_pos = b.here();
            let cmp_op = if *inclusive { BinOp::Le } else { BinOp::Lt };
            let cond_t = b.fresh(Type::Bool);
            b.emit_mwir(Inst::Compare {
                dst: cond_t,
                op: cmp_op,
                ty: elem_ty.clone(),
                lhs: i_temp,
                rhs: to_t,
            });
            let end_fixup = b.emit_mwir(Inst::JumpIfFalse {
                cond: cond_t,
                target: usize::MAX,
            });
            env.push(BTreeMap::new());
            env_insert(env, name.to_string(), Binding::Temp(i_temp));
            lower_block(body, b, env, defers, loops)?;
            env.pop();
            let incr_pos = b.here();
            let one_t = b.fresh(elem_ty.clone());
            b.emit_mwir(Inst::ConstInt {
                dst: one_t,
                ty: elem_ty.clone(),
                value: 1,
            });
            let next_t = b.fresh(elem_ty.clone());
            b.emit_mwir(Inst::ArithWrapping {
                dst: next_t,
                op: BinOp::AddW,
                ty: elem_ty.clone(),
                lhs: i_temp,
                rhs: one_t,
            });
            b.emit_mwir(Inst::Copy {
                dst: i_temp,
                src: next_t,
            });
            b.emit_mwir(Inst::Jump { target: cond_pos });
            let end_pos = b.here();
            b.patch(end_fixup, end_pos);
            let ctx = loops.pop().expect("pushed above");
            finish_intra_loop_fixups(ctx, end_pos, incr_pos, b);
        }
        TypedForIter::Expr(arr) => {
            let arr_t = lower_expr_flat(arr, b, env)?;
            let len = eval_array_len(&arr.ty)?;
            let idx_t = b.fresh(Type::Usize);
            b.emit_mwir(Inst::ConstInt {
                dst: idx_t,
                ty: Type::Usize,
                value: 0,
            });
            let len_t = b.fresh(Type::Usize);
            b.emit_mwir(Inst::ConstInt {
                dst: len_t,
                ty: Type::Usize,
                value: len as i128,
            });
            let cond_pos = b.here();
            let cond_t = b.fresh(Type::Bool);
            b.emit_mwir(Inst::Compare {
                dst: cond_t,
                op: BinOp::Lt,
                ty: Type::Usize,
                lhs: idx_t,
                rhs: len_t,
            });
            let end_fixup = b.emit_mwir(Inst::JumpIfFalse {
                cond: cond_t,
                target: usize::MAX,
            });
            let elem_t = b.fresh(elem_ty.clone());
            b.emit_mwir(Inst::IndexGet {
                dst: elem_t,
                base: arr_t,
                index: idx_t,
                len,
            });
            env.push(BTreeMap::new());
            env_insert(env, name.to_string(), Binding::Temp(elem_t));
            lower_block(body, b, env, defers, loops)?;
            env.pop();
            let incr_pos = b.here();
            let one_t = b.fresh(Type::Usize);
            b.emit_mwir(Inst::ConstInt {
                dst: one_t,
                ty: Type::Usize,
                value: 1,
            });
            let next_t = b.fresh(Type::Usize);
            b.emit_mwir(Inst::ArithWrapping {
                dst: next_t,
                op: BinOp::AddW,
                ty: Type::Usize,
                lhs: idx_t,
                rhs: one_t,
            });
            b.emit_mwir(Inst::Copy {
                dst: idx_t,
                src: next_t,
            });
            b.emit_mwir(Inst::Jump { target: cond_pos });
            let end_pos = b.here();
            b.patch(end_fixup, end_pos);
            let ctx = loops.pop().expect("pushed above");
            finish_intra_loop_fixups(ctx, end_pos, incr_pos, b);
        }
    }
    Ok(())
}

fn finish_intra_loop_fixups(ctx: LoopCtx, end_pos: usize, incr_pos: usize, b: &mut FlowBuilder) {
    let LoopCtx::Intra {
        break_fixups,
        continue_fixups,
        ..
    } = ctx
    else {
        unreachable!("`for` only ever pushes LoopCtx::Intra")
    };
    for idx in break_fixups {
        b.patch(idx, end_pos);
    }
    for idx in continue_fixups {
        b.patch(idx, incr_pos);
    }
}

// --- match/pattern lowering (intra only; mirrors lower.rs's own logic) -----

fn lower_match<'a>(
    scrutinee: &'a TypedExpr,
    arms: &'a [TypedMatchArm],
    b: &mut FlowBuilder,
    env: &mut FEnv,
    defers: &mut Vec<&'a TypedDeferBody>,
    loops: &mut Vec<LoopCtx>,
) -> Result<bool, FlowError> {
    let sv = lower_expr_flat(scrutinee, b, env)?;
    let mut end_fixups = Vec::new();
    let mut all_diverge = true;

    for arm in arms {
        let mut fail_fixups: Vec<usize> = Vec::new();
        let mut bindings = BTreeMap::new();
        collect_pattern_bindings(&arm.pattern, &mut bindings, b);
        let test = lower_pattern_test(&arm.pattern, sv, &bindings, b, env)?;
        fail_fixups.push(b.emit_mwir(Inst::JumpIfFalse {
            cond: test,
            target: usize::MAX,
        }));
        env.push(
            bindings
                .into_iter()
                .map(|(k, t)| (k, Binding::Temp(t)))
                .collect(),
        );
        if let Some(guard) = &arm.guard {
            let g = lower_expr_flat(guard, b, env)?;
            fail_fixups.push(b.emit_mwir(Inst::JumpIfFalse {
                cond: g,
                target: usize::MAX,
            }));
        }
        let d = lower_block(&arm.body, b, env, defers, loops)?;
        env.pop();
        if !d {
            all_diverge = false;
        }
        end_fixups.push(b.emit_mwir(Inst::Jump { target: usize::MAX }));
        let next_arm_pos = b.here();
        for idx in fail_fixups {
            b.patch(idx, next_arm_pos);
        }
    }
    b.emit_mwir(Inst::AssertFail {
        message: Some(
            "match: no arm matched (exhaustiveness already proved this cannot happen)".to_string(),
        ),
    });
    let match_end = b.here();
    for idx in end_fixups {
        b.patch(idx, match_end);
    }
    Ok(all_diverge)
}

fn collect_pattern_bindings(
    pat: &TypedPattern,
    out: &mut BTreeMap<String, Temp>,
    b: &mut FlowBuilder,
) {
    match &pat.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Literal(_) => {}
        TypedPatternKind::Binding(name) => {
            let t = b.fresh(pat.ty.clone());
            out.insert(name.clone(), t);
        }
        TypedPatternKind::Take(inner) => collect_pattern_bindings(inner, out, b),
        TypedPatternKind::Variant { payload, .. } => {
            for p in payload {
                collect_pattern_bindings(p, out, b);
            }
        }
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            for p in items {
                collect_pattern_bindings(p, out, b);
            }
        }
        TypedPatternKind::Or(_) => {}
    }
}

fn lower_pattern_test(
    pattern: &TypedPattern,
    value: Temp,
    bindings: &BTreeMap<String, Temp>,
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<Temp, FlowError> {
    match &pattern.kind {
        TypedPatternKind::Wildcard => {
            let t = b.fresh(Type::Bool);
            b.emit_mwir(Inst::ConstBool {
                dst: t,
                value: true,
            });
            Ok(t)
        }
        TypedPatternKind::Binding(name) => {
            let dst = *bindings
                .get(name)
                .expect("collect_pattern_bindings pre-allocated every binding name");
            b.emit_mwir(Inst::Copy { dst, src: value });
            let t = b.fresh(Type::Bool);
            b.emit_mwir(Inst::ConstBool {
                dst: t,
                value: true,
            });
            Ok(t)
        }
        TypedPatternKind::Take(inner) => lower_pattern_test(inner, value, bindings, b, env),
        TypedPatternKind::Literal(lit) => {
            let lit_temp = lower_expr_flat(lit, b, env)?;
            let t = b.fresh(Type::Bool);
            b.emit_mwir(Inst::Compare {
                dst: t,
                op: BinOp::Eq,
                ty: pattern.ty.clone(),
                lhs: value,
                rhs: lit_temp,
            });
            Ok(t)
        }
        TypedPatternKind::Variant {
            enum_name,
            variant,
            payload,
        } => {
            let want = variant_index(b.prog, enum_name, variant)?;
            let tag_t = b.fresh(Type::U64);
            b.emit_mwir(Inst::EnumTag {
                dst: tag_t,
                src: value,
            });
            let want_t = b.fresh(Type::U64);
            b.emit_mwir(Inst::ConstInt {
                dst: want_t,
                ty: Type::U64,
                value: want as i128,
            });
            let mut result = b.fresh(Type::Bool);
            b.emit_mwir(Inst::Compare {
                dst: result,
                op: BinOp::Eq,
                ty: Type::U64,
                lhs: tag_t,
                rhs: want_t,
            });
            for (i, subpat) in payload.iter().enumerate() {
                let payload_t = b.fresh(subpat.ty.clone());
                b.emit_mwir(Inst::EnumPayload {
                    dst: payload_t,
                    src: value,
                    index: i,
                });
                let sub_ok = lower_pattern_test(subpat, payload_t, bindings, b, env)?;
                let merged = b.fresh(Type::Bool);
                b.emit_mwir(Inst::BoolAnd {
                    dst: merged,
                    lhs: result,
                    rhs: sub_ok,
                });
                result = merged;
            }
            Ok(result)
        }
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            let result = b.fresh(Type::Bool);
            b.emit_mwir(Inst::ConstBool {
                dst: result,
                value: true,
            });
            let mut result = result;
            for (i, subpat) in items.iter().enumerate() {
                let elem_t = b.fresh(subpat.ty.clone());
                b.emit_mwir(Inst::Project {
                    dst: elem_t,
                    base: value,
                    index: i,
                });
                let sub_ok = lower_pattern_test(subpat, elem_t, bindings, b, env)?;
                let merged = b.fresh(Type::Bool);
                b.emit_mwir(Inst::BoolAnd {
                    dst: merged,
                    lhs: result,
                    rhs: sub_ok,
                });
                result = merged;
            }
            Ok(result)
        }
        TypedPatternKind::Or(_) => Err(FlowError::unimplemented("an `|` (or) pattern is")),
    }
}

// --- places (assignment targets) -------------------------------------------

fn lower_place_write(
    target: &TypedExpr,
    value: Temp,
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<(), FlowError> {
    match &target.kind {
        TypedExprKind::Local(name) => {
            match env_lookup(env, name) {
                Some(Binding::Temp(t)) => {
                    b.emit_mwir(Inst::Copy { dst: t, src: value });
                }
                // Reassigning a name that was (or never was) an ordinary
                // temp — rebind fresh (no prior temp exists to write
                // into). No required golden reassigns a self-path-bound
                // local, so this is the dumbest sound fallback.
                _ => env_insert(env, name.clone(), Binding::Temp(value)),
            }
            Ok(())
        }
        TypedExprKind::Field(base, fname) => {
            let TypedExprKind::Local(base_name) = &base.kind else {
                return Err(FlowError::unimplemented(
                    "assigning through a nested field/index chain (more than one level) is",
                ));
            };
            let base_temp = match env_lookup(env, base_name) {
                Some(Binding::Temp(t)) => t,
                _ => {
                    return Err(FlowError::internal(format!(
                        "unbound (or self-path, not a temp) local `{base_name}` in place position"
                    )));
                }
            };
            let base_ty = bodies::unwrap_own(base.ty.clone());
            let idx = field_index(b.prog, &base_ty, fname)?;
            b.emit_mwir(Inst::SetField {
                base: base_temp,
                index: idx,
                value,
            });
            Ok(())
        }
        _ => Err(FlowError::unimplemented("assigning to this place is")),
    }
}

// --- expressions (await-free contexts only) ---------------------------------

/// Lowers `e` in a context that can never itself suspend — the module
/// doc's own headline fail-closed set names everything this deliberately
/// does not cover.
fn lower_expr_flat(e: &TypedExpr, b: &mut FlowBuilder, env: &mut FEnv) -> Result<Temp, FlowError> {
    match &e.kind {
        TypedExprKind::Int(text) => {
            let raw = value::parse_int_literal(text)
                .ok_or_else(|| FlowError::internal("invalid integer literal text"))?;
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::ConstInt {
                dst,
                ty: e.ty.clone(),
                value: raw,
            });
            Ok(dst)
        }
        TypedExprKind::Bool(v) => {
            let dst = b.fresh(Type::Bool);
            b.emit_mwir(Inst::ConstBool { dst, value: *v });
            Ok(dst)
        }
        TypedExprKind::Unit => {
            let dst = b.fresh(Type::Unit);
            b.emit_mwir(Inst::ConstUnit { dst });
            Ok(dst)
        }
        TypedExprKind::Local(name) => match env_lookup(env, name) {
            Some(Binding::Temp(t)) => Ok(t),
            Some(Binding::SelfPath(path, ty)) => {
                let dst = b.fresh(ty);
                b.emit(FlowInst::SelfPath { dst, path });
                Ok(dst)
            }
            None => Err(FlowError::internal(format!("unbound local `{name}`"))),
        },
        TypedExprKind::Field(base, name) => {
            let base_temp = lower_expr_flat(base, b, env)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            let idx = field_index(b.prog, &base_ty, name)?;
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::Project {
                dst,
                base: base_temp,
                index: idx,
            });
            Ok(dst)
        }
        // Move is a type-system fact; lowering just evaluates the place
        // (mirrors `lower.rs`).
        TypedExprKind::Take(inner) => lower_expr_flat(inner, b, env),
        TypedExprKind::Const(name) => {
            let v = crate::eval::interp::eval_const(b.prog, name).map_err(|err| {
                FlowError::internal(format!(
                    "const `{name}` failed to evaluate during flowwir lowering: {}",
                    err.message
                ))
            })?;
            lower_flow_const_value(&v, &e.ty, b)
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => lower_flow_call(callee, receiver, args, &e.ty, b, env),
        TypedExprKind::StructLiteral { name, fields } => {
            let Type::Named(sname, _) = &e.ty else {
                return Err(FlowError::internal("struct literal type is not `Named`"));
            };
            debug_assert_eq!(name, sname);
            let s = struct_by_name(b.prog, sname).ok_or_else(|| missing_struct(b.prog, sname))?;
            let mut slots: Vec<Option<Temp>> = vec![None; s.fields.len()];
            for (fname, fval) in fields {
                let idx = s
                    .fields
                    .iter()
                    .position(|f| f == fname)
                    .ok_or_else(|| FlowError::internal(format!("unknown field `{fname}`")))?;
                slots[idx] = Some(lower_expr_flat(fval, b, env)?);
            }
            for (i, fname) in s.fields.iter().enumerate() {
                if slots[i].is_none() {
                    return Err(FlowError::unimplemented(format!(
                        "a struct literal missing field `{fname}` (defaults in async) is"
                    )));
                }
            }
            let elems: Vec<Temp> = slots.into_iter().map(|s| s.expect("filled")).collect();
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::MakeAggregate { dst, elems });
            Ok(dst)
        }
        TypedExprKind::Tuple(items) => {
            let mut elems = Vec::with_capacity(items.len());
            for i in items {
                elems.push(lower_expr_flat(i, b, env)?);
            }
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::MakeAggregate { dst, elems });
            Ok(dst)
        }
        TypedExprKind::Binary(op, l, r) => lower_binary_flat(*op, l, r, e, b, env),
        TypedExprKind::Try(inner, conv) => {
            let v = lower_expr_flat(inner, b, env)?;
            lower_try_check(v, &inner.ty, conv, b)
        }
        TypedExprKind::Send(inner) => {
            let TypedExprKind::Call {
                callee,
                receiver: Some(recv),
                args,
            } = &inner.kind
            else {
                return Err(FlowError::internal(
                    "`send`'s inner node is not a receiver call",
                ));
            };
            let target = lower_expr_flat(recv, b, env)?;
            let method_key = callee.spelling();
            let f = resolve_callee_fn(b.prog, callee)?;
            let arg_temps = lower_aligned_args(f, args, b, env)?;
            let dst = b.fresh(e.ty.clone());
            b.emit(FlowInst::Send {
                dst,
                target,
                method_key,
                arg_temps,
            });
            Ok(dst)
        }
        TypedExprKind::Intrinsic { key, .. } if key.as_str() == "now" => {
            let dst = b.fresh(e.ty.clone());
            b.emit(FlowInst::Now { dst });
            Ok(dst)
        }
        TypedExprKind::Intrinsic { key, args, .. } if key.as_str() == "ms" => {
            let (_, n_expr) = args
                .first()
                .ok_or_else(|| FlowError::internal("`ms` has no `n` argument"))?;
            let n = lower_expr_flat(n_expr, b, env)?;
            let dst = b.fresh(e.ty.clone());
            b.emit(FlowInst::Duration { dst, n });
            Ok(dst)
        }
        // plans/M7.md item H1: the async half of `lower.rs`'s own MMIO
        // arm. The *sync* half emits, and the sync half is the whole of
        // what this item needs: a driver's `init` is a plain `fn`
        // (03-hardware.md §1's own worked constructor), and the async
        // surface that reads registers is 03 §6's ISR and §7's bottom-half
        // task, both of which plans/M7.md item G owns. Failing closed here
        // rather than in the `{other:?}` catch-all so a reader is told
        // which item, not shown a typed node.
        TypedExprKind::Intrinsic { key, .. }
            if crate::sema::bodies::is_mmio_access_intrinsic(key)
                || crate::sema::bodies::is_device_transport_intrinsic(key)
                || crate::sema::bodies::is_irq_cap_intrinsic(key) =>
        {
            Err(FlowError::unimplemented(
                "a typed MMIO access, bring-up transition, or IRQ operation (03-hardware.md \
                 §2/§6/§9) inside an `async fn`: the synchronous path emits these (plans/M7.md \
                 items H1/G), and a driver's own `init` is synchronous. The async register \
                 readers are 03 §6's ISR and §7's bottom-half task — until the remaining item-G \
                 surface lands for async, this is",
            ))
        }
        // plans/M7.md item H2a: narrowing is emitted on the sync path; an
        // async body that needs one can call a sync helper. Fail closed
        // by name rather than half-emitting through FlowWir.
        TypedExprKind::Intrinsic { key, .. }
            if crate::sema::bodies::is_untrusted_narrowing_intrinsic(key) =>
        {
            Err(FlowError::unimplemented(
                "`Untrusted[T].checked_le` inside an `async fn`: the synchronous path emits it \
                 (plans/M7.md item H2a); an async narrowing is",
            ))
        }
        // plans/M7.md item E4: the flagship's own `async` roundtrip
        // publishes on the same path as the sync handoff methods — emit
        // the same MWIR ops `lower.rs` does (decision 20's package +
        // ring write order).
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            args,
        } if crate::sema::bodies::is_queue_op_intrinsic(key) => {
            lower_flow_queue_op(key, receiver, type_arg, args, e, b, env)
        }
        TypedExprKind::Intrinsic { key, .. }
            if crate::sema::bodies::is_queue_op_deferred(key).is_some() =>
        {
            Err(FlowError::unimplemented(format!(
                "deferred queue operation `{key}` inside an async body is"
            )))
        }
        TypedExprKind::Await(_) | TypedExprKind::GroupChild(_) => Err(FlowError::unimplemented(
            "an `await`/group-child nested inside a larger expression (only a direct \
             `let`/assignment/`return`/bare-statement operand is supported) is",
        )),
        other => Err(FlowError::unimplemented(format!(
            "lowering this expression shape ({other:?}) inside an async body is"
        ))),
    }
}

fn lower_flow_const_value(
    v: &crate::eval::value::Value,
    ty: &Type,
    b: &mut FlowBuilder,
) -> Result<Temp, FlowError> {
    use crate::eval::value::Value;
    match v {
        Value::U8(_)
        | Value::U16(_)
        | Value::U32(_)
        | Value::U64(_)
        | Value::Usize(_)
        | Value::I8(_)
        | Value::I16(_)
        | Value::I32(_)
        | Value::I64(_)
        | Value::Isize(_) => {
            let raw = value::as_i128(v).expect("integer Value");
            let dst = b.fresh(ty.clone());
            b.emit_mwir(Inst::ConstInt {
                dst,
                ty: ty.clone(),
                value: raw,
            });
            Ok(dst)
        }
        Value::Bool(x) => {
            let dst = b.fresh(Type::Bool);
            b.emit_mwir(Inst::ConstBool { dst, value: *x });
            Ok(dst)
        }
        Value::Unit => {
            let dst = b.fresh(Type::Unit);
            b.emit_mwir(Inst::ConstUnit { dst });
            Ok(dst)
        }
        _ => Err(FlowError::unimplemented(
            "this const value shape inside an async body is",
        )),
    }
}

/// Sync-helper / in-process call from an async body (plans/M7.md item E4).
/// Mirrors `lower.rs::lower_call`'s receiver/write-back shape, using this
/// file's `Binding::Temp` environment.
fn lower_flow_queue_op(
    key: &str,
    receiver: &Option<Box<TypedExpr>>,
    type_arg: &Option<Type>,
    args: &[(String, TypedExpr)],
    e: &TypedExpr,
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<Temp, FlowError> {
    match key {
        "VirtQueue.prepare_block" => {
            let permit = args
                .iter()
                .find(|(l, _)| l == "permit")
                .ok_or_else(|| FlowError::internal("`prepare_block` without `permit=`"))?;
            let header = args
                .iter()
                .find(|(l, _)| l == "header")
                .ok_or_else(|| FlowError::internal("`prepare_block` without `header=`"))?;
            let payload = args
                .iter()
                .find(|(l, _)| l == "payload")
                .ok_or_else(|| FlowError::internal("`prepare_block` without `payload=`"))?;
            let status = args
                .iter()
                .find(|(l, _)| l == "status")
                .ok_or_else(|| FlowError::internal("`prepare_block` without `status=`"))?;
            let device_writes_arg = args
                .iter()
                .find(|(l, _)| l == "device_writes_payload")
                .ok_or_else(|| {
                    FlowError::internal("`prepare_block` without `device_writes_payload=`")
                })?;
            let device_writes = match &device_writes_arg.1.kind {
                TypedExprKind::Bool(v) => *v,
                _ => {
                    return Err(FlowError::unimplemented(
                        "`prepare_block`'s `device_writes_payload=` as a non-literal bool is",
                    ));
                }
            };
            let queue = match receiver {
                Some(q) => lower_expr_flat(q, b, env)?,
                None => {
                    return Err(FlowError::internal(
                        "`prepare_block` without a queue receiver",
                    ));
                }
            };
            let permit_t = lower_expr_flat(&permit.1, b, env)?;
            let header_t = lower_expr_flat(&header.1, b, env)?;
            let payload_t = lower_expr_flat(&payload.1, b, env)?;
            let status_t = lower_expr_flat(&status.1, b, env)?;
            let payload_len =
                crate::lower::layout_dma_size(&payload.1.ty, b.prog).ok_or_else(|| {
                    FlowError::internal("`prepare_block` payload has no `@layout(dma)` size")
                })?;
            if payload_len == 0 || payload_len % 512 != 0 {
                return Err(FlowError::unimplemented(format!(
                    "`prepare_block` with payload layout size {payload_len}: virtio-blk requires \
                     a positive multiple of 512"
                )));
            }
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::QueuePrepare {
                dst,
                queue,
                permit: permit_t,
                header: header_t,
                payload: payload_t,
                status: status_t,
                device_writes,
                payload_len: payload_len as u32,
            });
            Ok(dst)
        }
        "VirtQueue.reserve_proven" => {
            let _ = args
                .iter()
                .find(|(l, _)| l == "descriptors")
                .ok_or_else(|| FlowError::internal("`reserve_proven` without `descriptors=`"))?;
            let _ = receiver;
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::ConstInt {
                dst,
                ty: Type::U64,
                value: 0,
            });
            Ok(dst)
        }
        "VirtQueue.publish" => {
            let op = args
                .iter()
                .find(|(l, _)| l == "operation")
                .ok_or_else(|| FlowError::internal("`publish` without `operation=`"))?;
            let queue = match receiver {
                Some(q) => lower_expr_flat(q, b, env)?,
                None => {
                    return Err(FlowError::internal("`publish` without a queue receiver"));
                }
            };
            let operation = lower_expr_flat(&op.1, b, env)?;
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::QueuePublish {
                dst,
                queue,
                operation,
                steps: crate::virtqueue::PUBLISH_WRITE_ORDER,
            });
            Ok(dst)
        }
        "VirtQueue.reject" => {
            for (_, a) in args {
                let _ = lower_expr_flat(a, b, env)?;
            }
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::ConstInt {
                dst,
                ty: Type::U64,
                value: 0,
            });
            let _ = receiver;
            Ok(dst)
        }
        "VirtQueue.drain" => {
            let queue = match receiver {
                Some(q) => lower_expr_flat(q, b, env)?,
                None => {
                    return Err(FlowError::internal("`drain` without a queue receiver"));
                }
            };
            let max_val = match type_arg {
                Some(Type::Named(_, targs)) => match targs.first() {
                    Some(crate::sema::types::TypeArg::Bound(crate::syntax::ast::Expr::Int(
                        _,
                        text,
                    ))) => text
                        .parse::<u16>()
                        .map_err(|_| FlowError::internal(format!("drain max `{text}`")))?,
                    _ => {
                        return Err(FlowError::internal(
                            "`drain` type_arg Bound is not an integer literal",
                        ));
                    }
                },
                _ => {
                    return Err(FlowError::internal("`drain` without a folded max Bound"));
                }
            };
            let _ = args;
            b.emit_mwir(Inst::QueueDrain {
                queue,
                max: max_val,
            });
            let dst = b.fresh(Type::Unit);
            b.emit_mwir(Inst::ConstUnit { dst });
            Ok(dst)
        }
        "VirtQueue.suppress_interrupts" => {
            let queue = match receiver {
                Some(q) => lower_expr_flat(q, b, env)?,
                None => {
                    return Err(FlowError::internal(
                        "`suppress_interrupts` without a queue receiver",
                    ));
                }
            };
            let _ = type_arg;
            let _ = args;
            b.emit_mwir(Inst::QueueSuppressInterrupts { queue });
            let dst = b.fresh(Type::Unit);
            b.emit_mwir(Inst::ConstUnit { dst });
            Ok(dst)
        }
        other => Err(FlowError::unimplemented(format!(
            "queue operation `{other}` inside an async body is"
        ))),
    }
}

/// `+ - *`/comparisons only (module doc: no required golden needs `/ %`,
/// shifts, bitwise ops, or a float operand) — the one special case, an
/// additive `Instant + Duration` (plans/M6.md decision 11's own
/// vocabulary), is represented as an opaque `u64` tick-count add
/// (`flowwir.rs`'s own module doc records the choice; the real unit
/// conversion is item D's job).
fn lower_binary_flat(
    op: BinOp,
    l: &TypedExpr,
    r: &TypedExpr,
    e: &TypedExpr,
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<Temp, FlowError> {
    let instant_ty = Type::Named("Instant".to_string(), vec![]);
    if op == BinOp::Add && l.ty == instant_ty {
        let lv = lower_expr_flat(l, b, env)?;
        let rv = lower_expr_flat(r, b, env)?;
        let dst = b.fresh(instant_ty);
        b.emit_mwir(Inst::ArithWrapping {
            dst,
            op: BinOp::AddW,
            ty: Type::U64,
            lhs: lv,
            rhs: rv,
        });
        return Ok(dst);
    }
    let lv = lower_expr_flat(l, b, env)?;
    let rv = lower_expr_flat(r, b, env)?;
    let ty = l.ty.clone();
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul => {
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::ArithChecked {
                dst,
                op,
                ty,
                lhs: lv,
                rhs: rv,
                abort: mwir::abort_message(op),
            });
            Ok(dst)
        }
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne => {
            let dst = b.fresh(Type::Bool);
            b.emit_mwir(Inst::Compare {
                dst,
                op,
                ty,
                lhs: lv,
                rhs: rv,
            });
            Ok(dst)
        }
        other => Err(FlowError::unimplemented(format!(
            "the binary operator `{}` inside an async body is",
            other.as_str()
        ))),
    }
}

/// Postfix `?` (02-language.md §7.4) on an already-synchronous `Result`
/// value: tests the tag, projects the `Ok` payload as this expression's
/// own result on the true path, and on the false path builds this fn's
/// own `Err`-wrapped return value and embeds an early `Mwir(Inst::Return)`
/// — entirely as ops in the *current* state (module doc: an early return
/// is just another embedded op, never a `Transition` of its own).
fn lower_try_check(
    value_temp: Temp,
    value_ty: &Type,
    conv: &Option<CalleeKey>,
    b: &mut FlowBuilder,
) -> Result<Temp, FlowError> {
    let (ok_ty, err_ty) = match value_ty {
        Type::Result(o, e) => ((**o).clone(), (**e).clone()),
        _ => {
            return Err(FlowError::unimplemented(
                "`?` on a non-`Result` (e.g. `Option`) value is",
            ));
        }
    };
    let tag_t = b.fresh(Type::U64);
    b.emit_mwir(Inst::EnumTag {
        dst: tag_t,
        src: value_temp,
    });
    let ok_const = b.fresh(Type::U64);
    b.emit_mwir(Inst::ConstInt {
        dst: ok_const,
        ty: Type::U64,
        value: value::RESULT_OK as i128,
    });
    let is_ok = b.fresh(Type::Bool);
    b.emit_mwir(Inst::Compare {
        dst: is_ok,
        op: BinOp::Eq,
        ty: Type::U64,
        lhs: tag_t,
        rhs: ok_const,
    });
    let err_fixup = b.emit_mwir(Inst::JumpIfFalse {
        cond: is_ok,
        target: usize::MAX,
    });
    let ok_payload = b.fresh(ok_ty);
    b.emit_mwir(Inst::EnumPayload {
        dst: ok_payload,
        src: value_temp,
        index: 0,
    });
    let after_fixup = b.emit_mwir(Inst::Jump { target: usize::MAX });
    let err_pos = b.here();
    b.patch(err_fixup, err_pos);
    let err_payload = b.fresh(err_ty);
    b.emit_mwir(Inst::EnumPayload {
        dst: err_payload,
        src: value_temp,
        index: 0,
    });
    let Type::Result(_, ret_err) = &b.ret else {
        return Err(FlowError::internal(
            "`?` used inside a fn whose own declared return type is not `Result`",
        ));
    };
    let target_ty = (**ret_err).clone();
    let converted = match conv {
        Some(key) => lower_from_conversion_flow(err_payload, key, target_ty, b)?,
        None => err_payload,
    };
    let ret_enum = b.fresh(b.ret.clone());
    b.emit_mwir(Inst::MakeEnum {
        dst: ret_enum,
        tag: value::RESULT_ERR,
        payload: vec![converted],
    });
    b.emit_mwir(Inst::Return {
        value: Some(ret_enum),
    });
    let after_pos = b.here();
    b.patch(after_fixup, after_pos);
    Ok(ok_payload)
}

/// Apply `?`'s one-hop `from` conversion inside an async body (same
/// rules as `lower::lower_from_conversion` — Call only, no structural
/// wrap; plans/M9.md item B3).
fn lower_from_conversion_flow(
    err_payload: Temp,
    key: &CalleeKey,
    target_ty: Type,
    b: &mut FlowBuilder,
) -> Result<Temp, FlowError> {
    if resolve_callee_fn(b.prog, key).is_err() {
        return Err(FlowError::internal(format!(
            "`?` conversion `{}` has no TypedFn (deriving(From) must generate one)",
            key.spelling()
        )));
    }
    let dst = b.fresh(target_ty);
    b.emit_mwir(Inst::Call {
        dst,
        write_backs: Vec::new(),
        key: key.spelling(),
        args: vec![err_payload],
    });
    Ok(dst)
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::sema;
    use crate::syntax::{lexer, parser};

    fn typed_program(src: &str) -> TypedProgram {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        sema::check_typed(&module, "<test>").expect("test source must check")
    }

    /// State-count assertions for each of the seven required golden
    /// shapes (`compiler.lower.flowwir-stable`'s own ledger note) —
    /// mirrors `tests/golden/<case>/input.wr` verbatim, so a drift here
    /// would also show up as a golden diff; this test locks the *count*
    /// specifically, independent of the dump's exact text.
    #[test]
    fn state_counts_match_every_required_golden_shape() {
        let basic = typed_program(
            "module examples.flowwir_basic

@actor
pub struct Counter:
    value: u64

    pub fn get(read self) -> u64:
        return self.value

@actor
pub struct Caller:
    counter: Actor[Counter]

    pub async fn run(mut self) -> u64:
        v = await self.counter.get()
        match v:
            case .Ok(n):
                return n
            case .Err(_):
                return 0
",
        );
        let flow = lower_program(&basic).expect("flowwir-basic must lower cleanly");
        assert_eq!(flow.fns["Caller.run"].states.len(), 2);

        let chain = typed_program(
            "module examples.flowwir_chain

@actor
pub struct Alpha:
    value: u64

    pub fn step(read self) -> u64:
        return self.value

@actor
pub struct Chain:
    a: Actor[Alpha]
    b: Actor[Alpha]
    c: Actor[Alpha]

    pub async fn run(mut self) -> u64:
        ra = await self.a.step()
        x: u64 = 0
        match ra:
            case .Ok(v):
                x = v
            case .Err(_):
                pass
        rb = await self.b.step()
        y: u64 = 0
        match rb:
            case .Ok(v):
                y = v
            case .Err(_):
                pass
        rc = await self.c.step()
        z: u64 = 0
        match rc:
            case .Ok(v):
                z = v
            case .Err(_):
                pass
        return x + y + z
",
        );
        let flow = lower_program(&chain).expect("flowwir-chain must lower cleanly");
        assert_eq!(flow.fns["Chain.run"].states.len(), 4);

        let group = typed_program(
            "module examples.check_group

async fn fetch_part(index: u64) -> u64:
    return index * 2

async fn run_group() -> u64:
    total: u64 = 0
    with group(capacity=4) as g:
        g.start(fetch_part, index=0)
        g.start(fetch_part, index=1)
        results = await g.join_all()
        for r in results:
            match r:
                case .Ok(v):
                    total = total + v
                case .Err(_):
                    pass
    return total
",
        );
        let flow = lower_program(&group).expect("check-group must lower cleanly");
        assert_eq!(flow.fns["run_group"].states.len(), 2);

        let deadline = typed_program(
            "module examples.check_deadline

@actor
pub struct Storage:
    value: u64

    pub fn load(read self) -> u64:
        return self.value

async fn bounded_read(storage: Actor[Storage]) -> u64:
    result: u64 = 0
    with group(deadline=now() + ms(50)):
        outcome = await storage.load()
        match outcome:
            case .Ok(v):
                result = v
            case .Err(_):
                pass
    return result
",
        );
        let flow = lower_program(&deadline).expect("check-deadline must lower cleanly");
        assert_eq!(flow.fns["bounded_read"].states.len(), 2);

        let defer = typed_program(
            "module examples.flowwir_defer

@actor
pub struct Store:
    value: u64

    pub fn load(read self) -> u64:
        return self.value

async fn helper(target: Actor[Store]) -> u64:
    result: u64 = 0
    with group(deadline=now() + ms(10)):
        defer:
            result = result + 1
        outcome = await target.load()
        match outcome:
            case .Ok(v):
                result = v
            case .Err(_):
                pass
    return result
",
        );
        let flow = lower_program(&defer).expect("flowwir-defer must lower cleanly");
        assert_eq!(flow.fns["helper"].states.len(), 4);

        let branch = typed_program(
            "module examples.flowwir_branch_await

@actor
pub struct Store:
    value: u64

    pub fn load(read self) -> u64:
        return self.value

async fn maybe_fetch(target: Actor[Store], use_remote: bool) -> u64:
    result: u64 = 0
    if use_remote:
        outcome = await target.load()
        match outcome:
            case .Ok(v):
                result = v
            case .Err(_):
                pass
    else:
        result = 7
    return result
",
        );
        let flow = lower_program(&branch).expect("flowwir-branch-await must lower cleanly");
        assert_eq!(flow.fns["maybe_fetch"].states.len(), 5);

        let loop_await = typed_program(
            "module examples.flowwir_loop_await

@actor
pub struct Store:
    value: u64

    pub fn load(read self) -> u64:
        return self.value

async fn poll_until(target: Actor[Store], tries: u64) -> u64:
    total: u64 = 0
    i: u64 = 0
    while i < tries:
        outcome = await target.load()
        match outcome:
            case .Ok(v):
                total = total + v
            case .Err(_):
                pass
        i = i + 1
    return total
",
        );
        let flow = lower_program(&loop_await).expect("flowwir-loop-await must lower cleanly");
        assert_eq!(flow.fns["poll_until"].states.len(), 5);
    }

    fn self_path_program() -> TypedProgram {
        typed_program(
            "module examples.check_await_self_path

struct Cache:
    value: u64

@actor
pub struct Upstream:
    value: u64

    pub fn get(read self) -> u64:
        return self.value

@actor
pub struct Store:
    cache: Cache
    upstream: Actor[Upstream]

    pub async fn refresh(mut self) -> u64:
        before = self.cache.value
        fetched = await self.upstream.get()
        match fetched:
            case .Ok(v):
                after = self.cache.value
                return before + after + v
            case .Err(_):
                return before
",
        )
    }

    /// Frame-layout determinism (plans/M6.md item B's own required unit
    /// test): lowering the identical program twice produces the exact
    /// same `FrameLayout` — every temp's type, in order, plus the two
    /// fixed lineage slots — never a run-to-run reordering.
    #[test]
    fn frame_layout_is_deterministic_across_two_lowerings() {
        let program = self_path_program();
        let first = lower_program(&program).expect("must lower cleanly");
        let second = lower_program(&program).expect("must lower cleanly");
        let f1 = &first.fns["Store.refresh"].frame;
        let f2 = &second.fns["Store.refresh"].frame;
        assert_eq!(f1.temp_types, f2.temp_types);
        assert_eq!(f1.lineage_group_slot, f2.lineage_group_slot);
        assert_eq!(f1.lineage_deadline_slot, f2.lineage_deadline_slot);
        assert_eq!(f1.lineage_group_slot, Temp(0));
        assert_eq!(f1.lineage_deadline_slot, Temp(1));
    }

    /// Path-carrying across `await` (02-language.md §9.2, module doc's
    /// own "Self-rooted paths across `await`" section): `before`'s own
    /// read (in the resume state, after the `await`) and `after`'s own
    /// read (later in the same state) are each their own independent
    /// `FlowInst::SelfPath` — the path is recorded and re-derived twice,
    /// never a single value carried across the suspension as a raw temp.
    #[test]
    fn self_rooted_path_survives_await_as_a_path_not_a_temp() {
        let program = self_path_program();
        let flow = lower_program(&program).expect("must lower cleanly");
        let f = &flow.fns["Store.refresh"];
        // State 0 is the entry (up to the suspension); every `SelfPath`
        // op lives in the resume state, state 1.
        let resume = &f.states[1];
        let self_paths: Vec<&Vec<String>> = resume
            .ops
            .iter()
            .filter_map(|op| match op {
                FlowInst::SelfPath { path, .. } => Some(path),
                _ => None,
            })
            .collect();
        // Three independent re-derivations: `before`'s own use in the
        // `.Ok` arm (`before + after + v`), `after`'s own use there, and
        // `before`'s own second, separate use in the `.Err` arm (`return
        // before`) — never a single cached value shared across all three.
        assert_eq!(
            self_paths.len(),
            3,
            "expected `before`/`after` to each re-derive independently at every use, got {self_paths:?}"
        );
        for path in self_paths {
            assert_eq!(path, &vec!["cache".to_string(), "value".to_string()]);
        }
        // Never a single cached `Copy` from a pre-await temp into a
        // `before`-shaped local followed by reuse — every read is its own
        // fresh `SelfPath`, so `f.states[0]` (before the suspension)
        // carries no self-path materialization at all (only the receiver
        // call's own `Project` chain to the *handle* field, not `cache`).
        assert!(
            f.states[0]
                .ops
                .iter()
                .all(|op| !matches!(op, FlowInst::SelfPath { .. }))
        );
    }
}
