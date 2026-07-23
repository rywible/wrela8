//! The comptime evaluator's one obvious recursive walk (plans/M3.md item
//! B, decision 4): every entry point ends up here, walking the typed
//! tree (`sema::typed`) only — no name resolution, no typing, no
//! desugaring, nothing re-derived from the ast. A fact the walk needs
//! but the tree lacks is a producer bug: this file's own two additions
//! to the typed tree (`typed::TypedStruct::fields`, `typed::TypedProgram::enums`)
//! are exactly that — documented at their own declaration site, not
//! silently patched around here.
//!
//! Control flow (`return`/`break`/`continue`, and `?`'s own early
//! return) is one `Unwind` error channel threaded through Rust's own
//! `?`; comptime abandonment (overflow, failed assert, out-of-bounds,
//! explicit `panic`, a blown quota) is `Unwind::Error` — a build error
//! naming the operation, carrying the live comptime call stack (decision
//! 5). `defer` runs at every real exit in reverse registration order,
//! block-scoped exactly like `sema::flow`'s own `check_active_defers`
//! (this file's own runtime mirror of that pass, not a re-derivation of
//! anything the typed tree itself lacks — the tree already carries every
//! `Defer` statement in its own block).

use std::collections::BTreeMap;

use crate::eval::quota::{MAX_CALL_DEPTH, Quota};
use crate::eval::value::{self, Env, Value};
use crate::sema::bodies::{self, InstKind};
use crate::sema::generics;
use crate::sema::typed::{
    CalleeKey, TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn, TypedForIter,
    TypedInstantiation, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind,
    TypedStruct,
};
use crate::sema::types::{Type, TypeArg};
use crate::syntax::ast::BinOp;

/// One comptime build-error diagnostic (plans/M3.md decision 5):
/// `message` names the operation/condition that abandoned; `stack` is
/// the live comptime call stack at the point of failure, outermost
/// (the const/context that kicked off evaluation) first, innermost
/// (the deepest call) last — `eval/mod.rs` renders both into a
/// `sema::SemaError` (`error[comptime]: ...`, decision "pick a stable
/// diagnostic format consistent with the existing house style": typed
/// nodes carry no spans, so the stack *is* the location story).
#[derive(Debug, Clone, PartialEq)]
pub struct EvalError {
    pub message: String,
    pub stack: Vec<String>,
}

/// Non-local control flow, threaded as the error side of every
/// evaluation function's `Result` so Rust's own `?` implements
/// short-circuiting propagation for free — `Return`/`Break`/`Continue`
/// are ordinary, legal control flow (never rendered as a diagnostic);
/// only `Error` ever reaches a caller outside this module.
enum Unwind {
    Error(EvalError),
    Return(Value),
    Break,
    Continue,
}

type R<T> = Result<T, Unwind>;

impl From<EvalError> for Unwind {
    fn from(e: EvalError) -> Unwind {
        Unwind::Error(e)
    }
}

/// The evaluator's shared, non-scope state: the typed program being
/// walked, the running quota, and the live comptime call stack (callee
/// spellings/const-context names, in call order) diagnostics read from.
struct Interp<'p> {
    program: &'p TypedProgram,
    quota: Quota,
    stack: Vec<String>,
}

impl<'p> Interp<'p> {
    fn abandon(&self, message: impl Into<String>) -> Unwind {
        Unwind::Error(EvalError {
            message: message.into(),
            stack: self.stack.clone(),
        })
    }

    fn tick_step(&mut self) -> R<()> {
        self.quota.tick_step().map_err(|m| self.abandon(m))
    }

    fn charge(&mut self, n: u64) -> R<()> {
        self.quota.charge_memory(n).map_err(|m| self.abandon(m))
    }

    /// Enters one call/const-reference frame: ticks a step, pushes the
    /// frame name, and depth-checks (`quota::MAX_CALL_DEPTH`) — every
    /// call *and* every `const` cross-reference goes through this, so an
    /// unbounded `const` cycle (nothing before this evaluator existed
    /// to notice one) fails closed exactly like unbounded recursion.
    fn enter(&mut self, name: String) -> R<()> {
        self.tick_step()?;
        if self.stack.len() >= MAX_CALL_DEPTH {
            return Err(self.abandon(format!(
                "recursion depth exceeded {MAX_CALL_DEPTH} while evaluating `{name}`"
            )));
        }
        self.stack.push(name);
        Ok(())
    }

    fn leave(&mut self) {
        self.stack.pop();
    }
}

// --- top-level entry points ------------------------------------------------

/// Evaluates one module-level `const`'s own initializer (the integration
/// surface, plans/M3.md item B: "const initializers evaluated with the
/// real evaluator, replacing M2-H's literal-only subset").
pub fn eval_const(program: &TypedProgram, name: &str) -> Result<Value, EvalError> {
    let Some(c) = program.consts.get(name) else {
        return Err(EvalError {
            message: format!("internal error: const `{name}` not found in the checked program"),
            stack: vec![],
        });
    };
    eval_top(program, &c.value, name.to_string())
}

/// Evaluates an arbitrary comptime expression outside any `const`'s own
/// declaration (the other integration surface: a const-generic
/// argument, `generics.rs`'s own `eval_const_expr` replacement) —
/// `context` names the site for the call-stack diagnostic (e.g. the
/// generic being instantiated).
pub fn eval_standalone(
    program: &TypedProgram,
    expr: &TypedExpr,
    context: String,
) -> Result<Value, EvalError> {
    eval_top(program, expr, context)
}

fn eval_top(program: &TypedProgram, expr: &TypedExpr, context: String) -> Result<Value, EvalError> {
    let mut ctx = Interp {
        program,
        quota: Quota::new(),
        stack: Vec::new(),
    };
    if let Err(e) = ctx.enter(context) {
        return Err(unwind_to_error(e));
    }
    let mut env: Env = vec![BTreeMap::new()];
    let mut dstack: Vec<&TypedDeferBody> = Vec::new();
    let result = eval_expr(expr, &mut env, &mut dstack, 0, &mut ctx);
    match result {
        Ok(v) => Ok(v),
        Err(u) => Err(unwind_to_error(u)),
    }
}

fn unwind_to_error(u: Unwind) -> EvalError {
    match u {
        Unwind::Error(e) => e,
        // A bare comptime expression (a `const` initializer, a generic
        // argument) has no enclosing function/loop for `return`/`break`/
        // `continue` to escape from — `bodies::check_try` already
        // rejects a bare `?` outside a `Result`/`Option`-returning
        // function, so none of these three can legally reach here. Fail
        // closed rather than let a producer bug panic the whole
        // compiler process (the eval fuzz smoke's own "never panics"
        // invariant, plans/M3.md item F).
        Unwind::Return(_) | Unwind::Break | Unwind::Continue => EvalError {
            message: "internal error: control flow escaped a top-level comptime expression"
                .to_string(),
            stack: vec![],
        },
    }
}

// --- callee/struct/enum resolution (typed-tree lookups only) -------------

fn resolve_fn<'p>(program: &'p TypedProgram, key: &CalleeKey) -> Option<&'p TypedFn> {
    match key {
        CalleeKey::Fn(name) => program.fns.get(name),
        CalleeKey::FnInstance(ikey) => match program.instantiations.get(ikey) {
            Some(TypedInstantiation::Fn(f)) => Some(f),
            _ => None,
        },
        CalleeKey::Method(sname, member) => {
            resolve_struct_member(program.structs.get(sname)?, member)
        }
        CalleeKey::MethodInstance(ikey, member) => match program.instantiations.get(ikey) {
            Some(TypedInstantiation::Struct(s)) => resolve_struct_member(s, member),
            _ => None,
        },
    }
}

fn resolve_struct_member<'p>(s: &'p TypedStruct, member: &str) -> Option<&'p TypedFn> {
    if member == "init" {
        s.init.as_ref()
    } else {
        s.methods.get(member).or_else(|| s.assoc_fns.get(member))
    }
}

/// The struct a `Method`/`MethodInstance` callee key's `init` allocates,
/// or a `Field`/`Index`/`StructLiteral` node's own struct (from its
/// `Type::Named(name, targs)`) — the one place `generics::canonical_key`
/// is recomputed from scratch (a `Type::Named`'s args are the original,
/// uninstantiated-key form; `CalleeKey::MethodInstance` already carries
/// the canonical key spelling directly, so that case never needs this).
fn resolve_struct_by_type<'p>(
    program: &'p TypedProgram,
    name: &str,
    targs: &[TypeArg],
) -> Option<&'p TypedStruct> {
    if targs.is_empty() {
        program.structs.get(name)
    } else {
        let key = generics::canonical_key(InstKind::Struct, name, targs);
        match program.instantiations.get(&key) {
            Some(TypedInstantiation::Struct(s)) => Some(s),
            _ => None,
        }
    }
}

fn struct_of<'p>(program: &'p TypedProgram, key: &CalleeKey) -> Option<&'p TypedStruct> {
    match key {
        CalleeKey::Method(sname, _) => program.structs.get(sname),
        CalleeKey::MethodInstance(ikey, _) => match program.instantiations.get(ikey) {
            Some(TypedInstantiation::Struct(s)) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

fn field_index(fields: &[String], name: &str, ctx: &Interp) -> R<usize> {
    fields
        .iter()
        .position(|f| f == name)
        .ok_or_else(|| ctx.abandon(format!("internal error: unknown field `{name}`")))
}

/// `Option`/`Result`'s own fixed variant vocabulary (`value.rs`'s
/// `OPTION_NONE`/.../`RESULT_ERR`), else a user enum's own declared
/// order (`typed::TypedProgram::enums`, this item's own producer-gap
/// addition) — a generic enum instantiation never reaches this (sema
/// already fails it closed before it can appear in the typed tree,
/// `bodies::check_call_by_field`'s own generic-enum-construction guard).
fn variant_index(program: &TypedProgram, enum_name: &str, variant: &str, ctx: &Interp) -> R<usize> {
    match enum_name {
        "Option" => Ok(match variant {
            "None" => value::OPTION_NONE,
            "Some" => value::OPTION_SOME,
            other => {
                return Err(
                    ctx.abandon(format!("internal error: unknown Option variant `{other}`"))
                );
            }
        }),
        "Result" => Ok(match variant {
            "Ok" => value::RESULT_OK,
            "Err" => value::RESULT_ERR,
            other => {
                return Err(
                    ctx.abandon(format!("internal error: unknown Result variant `{other}`"))
                );
            }
        }),
        _ => {
            let Some(variants) = program.enums.get(enum_name) else {
                return Err(ctx.abandon(format!(
                    "evaluating a generic enum instantiation's variant (`{enum_name}.{variant}`) is not supported yet"
                )));
            };
            variants.iter().position(|v| v == variant).ok_or_else(|| {
                ctx.abandon(format!(
                    "internal error: unknown variant `{enum_name}.{variant}`"
                ))
            })
        }
    }
}

// --- environment ------------------------------------------------------------

fn env_lookup(env: &Env, name: &str) -> Option<Value> {
    for scope in env.iter().rev() {
        if let Some(v) = scope.get(name) {
            return Some(v.clone());
        }
    }
    None
}

fn env_insert(env: &mut Env, name: String, v: Value) {
    env.last_mut().expect("at least one scope").insert(name, v);
}

// --- defer ------------------------------------------------------------------

fn run_defers<'a, 'p>(defers: &[&'a TypedDeferBody], env: &mut Env, ctx: &mut Interp<'p>) -> R<()> {
    for d in defers.iter().rev() {
        // A defer body cannot `await`/`?` (`bodies::scan_defer_forbidden`
        // already rejects both), so only `Unwind::Error` can escape here
        // — no loop/`?` context to thread, so an empty dstack/loop_marker
        // is correct and unreachable in practice.
        let mut inner_dstack: Vec<&TypedDeferBody> = Vec::new();
        match d {
            TypedDeferBody::Expr(e) => {
                eval_expr(e, env, &mut inner_dstack, 0, ctx)?;
            }
            TypedDeferBody::Suite(stmts) => {
                exec_block(stmts, env, &mut inner_dstack, 0, ctx)?;
            }
        }
    }
    Ok(())
}

// --- statements --------------------------------------------------------

/// Walks one block's own statements (mirrors `sema::flow::walk_block`
/// exactly): registers `Defer` statements onto the shared `dstack`,
/// re-validates-turned-*executes* every currently active defer, in
/// reverse registration order, at this block's own normal completion
/// (truncating `dstack` back to what it was on entry) — an early exit
/// reached mid-block (`return`/`break`/`continue`/a `?` that hits `Err`/
/// `None`) already ran its own (differently sliced) defers at the
/// statement that produced it, via `Unwind`'s own early return, so this
/// loop never double-runs anything.
fn exec_block<'a, 'p>(
    stmts: &'a [TypedStmt],
    env: &mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<()> {
    let start = dstack.len();
    for s in stmts {
        exec_stmt(s, env, dstack, loop_marker, ctx)?;
    }
    run_defers(&dstack[start..], env, ctx)?;
    dstack.truncate(start);
    Ok(())
}

fn exec_stmt<'a, 'p>(
    stmt: &'a TypedStmt,
    env: &mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<()> {
    match &stmt.kind {
        TypedStmtKind::Let { name, value, .. } => {
            let v = eval_expr(value, env, dstack, loop_marker, ctx)?;
            env_insert(env, name.clone(), v);
            Ok(())
        }
        TypedStmtKind::Assign { target, value } => {
            let v = eval_expr(value, env, dstack, loop_marker, ctx)?;
            let place = place_mut(target, env, dstack, loop_marker, ctx)?;
            *place = v;
            Ok(())
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            let c = eval_expr(cond, env, dstack, loop_marker, ctx)?;
            if as_bool(&c) {
                return exec_block(then_branch, env, dstack, loop_marker, ctx);
            }
            for elif in elifs {
                let ec = eval_expr(&elif.cond, env, dstack, loop_marker, ctx)?;
                if as_bool(&ec) {
                    return exec_block(&elif.body, env, dstack, loop_marker, ctx);
                }
            }
            if let Some(b) = else_branch {
                return exec_block(b, env, dstack, loop_marker, ctx);
            }
            Ok(())
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            let sv = eval_expr(scrutinee, env, dstack, loop_marker, ctx)?;
            for arm in arms {
                if let Some(bindings) = match_pattern(&arm.pattern, &sv, ctx)? {
                    for (n, v) in &bindings {
                        env_insert(env, n.clone(), v.clone());
                    }
                    if let Some(guard) = &arm.guard {
                        let gv = eval_expr(guard, env, dstack, loop_marker, ctx)?;
                        if !as_bool(&gv) {
                            continue;
                        }
                    }
                    return exec_block(&arm.body, env, dstack, loop_marker, ctx);
                }
            }
            Err(ctx.abandon(
                "match: no arm matched (exhaustiveness already proved this cannot happen)",
            ))
        }
        TypedStmtKind::While { cond, body } => {
            let new_marker = dstack.len();
            loop {
                let c = eval_expr(cond, env, dstack, loop_marker, ctx)?;
                if !as_bool(&c) {
                    break;
                }
                ctx.tick_step()?;
                match exec_block(body, env, dstack, new_marker, ctx) {
                    Ok(()) => {}
                    Err(Unwind::Break) => break,
                    Err(Unwind::Continue) => {}
                    Err(other) => return Err(other),
                }
            }
            Ok(())
        }
        TypedStmtKind::For {
            name, iter, body, ..
        } => exec_for(name, iter, body, env, dstack, loop_marker, ctx),
        TypedStmtKind::Break => {
            run_defers(&dstack[loop_marker..], env, ctx)?;
            Err(Unwind::Break)
        }
        TypedStmtKind::Continue => {
            run_defers(&dstack[loop_marker..], env, ctx)?;
            Err(Unwind::Continue)
        }
        TypedStmtKind::Pass => Ok(()),
        TypedStmtKind::Return(value) => {
            let v = match value {
                Some(e) => eval_expr(e, env, dstack, loop_marker, ctx)?,
                None => Value::Unit,
            };
            run_defers(&dstack[..], env, ctx)?;
            Err(Unwind::Return(v))
        }
        TypedStmtKind::Assert { cond, message } => {
            let c = eval_expr(cond, env, dstack, loop_marker, ctx)?;
            if as_bool(&c) {
                Ok(())
            } else {
                let msg = match message {
                    Some(m) => {
                        let mv = eval_expr(m, env, dstack, loop_marker, ctx)?;
                        format!(": {}", render_message(&mv))
                    }
                    None => String::new(),
                };
                Err(ctx.abandon(format!("assertion failed{msg}")))
            }
        }
        // plans/M3.md item D, decision 8: a `comptime assert` is checked
        // exactly once, unconditionally, by `eval::check_comptime_asserts`
        // — independent of whether anything ever calls the fn/method it
        // lives in (its own vocabulary, module consts/literals only, does
        // not depend on a call's own locals/arguments anyway, so nothing
        // would differ call to call). Re-running it here, per ordinary
        // call execution, would at best be redundant and at worst wrong
        // (this statement's own typed node carries no live `env` lookup
        // path for locals — it is not meant to see any); a no-op.
        TypedStmtKind::ComptimeAssert { .. } => Ok(()),
        TypedStmtKind::Defer(body) => {
            dstack.push(body);
            Ok(())
        }
        TypedStmtKind::ExprStmt(e) => {
            eval_expr(e, env, dstack, loop_marker, ctx)?;
            Ok(())
        }
    }
}

fn exec_for<'a, 'p>(
    name: &str,
    iter: &'a TypedForIter,
    body: &'a [TypedStmt],
    env: &mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<()> {
    let new_marker = dstack.len();
    match iter {
        TypedForIter::Range(from, to, inclusive) => {
            let from_v = eval_expr(from, env, dstack, loop_marker, ctx)?;
            let to_v = eval_expr(to, env, dstack, loop_marker, ctx)?;
            let elem_ty = scalar_ty_of(&from_v);
            let mut i = value::as_i128(&from_v).expect("for-range endpoints are integer scalars");
            let end = value::as_i128(&to_v).expect("for-range endpoints are integer scalars");
            loop {
                if *inclusive {
                    if i > end {
                        break;
                    }
                } else if i >= end {
                    break;
                }
                ctx.tick_step()?;
                env_insert(env, name.to_string(), value::make_int(&elem_ty, i));
                match exec_block(body, env, dstack, new_marker, ctx) {
                    Ok(()) => {}
                    Err(Unwind::Break) => break,
                    Err(Unwind::Continue) => {}
                    Err(other) => return Err(other),
                }
                i += 1;
            }
            Ok(())
        }
        TypedForIter::Expr(arr) => {
            let av = eval_expr(arr, env, dstack, loop_marker, ctx)?;
            let elems = match av {
                Value::Array(v) => v,
                other => {
                    return Err(ctx.abandon(format!(
                        "internal error: `for` iterable is not an array value ({other:?})"
                    )));
                }
            };
            for elem in elems {
                ctx.tick_step()?;
                env_insert(env, name.to_string(), elem);
                match exec_block(body, env, dstack, new_marker, ctx) {
                    Ok(()) => {}
                    Err(Unwind::Break) => break,
                    Err(Unwind::Continue) => {}
                    Err(other) => return Err(other),
                }
            }
            Ok(())
        }
    }
}

/// `pub(crate)` (plans/M3.md item D): `eval/mod.rs::check_one_comptime_assert`
/// reuses this exact helper rather than duplicating it — a `comptime
/// assert` condition is typed as `bool` by `bodies::check_comptime_assert`
/// exactly like a plain `assert`'s, so the same "sema already proved
/// this" invariant holds.
pub(crate) fn as_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        other => {
            unreachable!("as_bool: `{other:?}` is not `bool` (sema already typed this as bool)")
        }
    }
}

fn scalar_ty_of(v: &Value) -> Type {
    match v {
        Value::U8(_) => Type::U8,
        Value::U16(_) => Type::U16,
        Value::U32(_) => Type::U32,
        Value::U64(_) => Type::U64,
        Value::Usize(_) => Type::Usize,
        Value::I8(_) => Type::I8,
        Value::I16(_) => Type::I16,
        Value::I32(_) => Type::I32,
        Value::I64(_) => Type::I64,
        Value::Isize(_) => Type::Isize,
        other => unreachable!("scalar_ty_of: `{other:?}` is not an integer scalar"),
    }
}

/// `pub(crate)` (plans/M3.md item D): shared verbatim with
/// `eval/mod.rs::check_one_comptime_assert`'s own message rendering — a
/// `comptime assert` message is typed exactly like a plain `assert`'s
/// (a text literal), so the identical rendering applies.
pub(crate) fn render_message(v: &Value) -> String {
    match v {
        Value::Str(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        other => format!("{other:?}"),
    }
}

// --- places (assignment targets; a mut-receiver's own self) --------------

fn place_mut<'e, 'a, 'p>(
    expr: &'a TypedExpr,
    env: &'e mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<&'e mut Value> {
    match &expr.kind {
        TypedExprKind::Local(name) => {
            for scope in env.iter_mut().rev() {
                if scope.contains_key(name) {
                    return Ok(scope.get_mut(name).expect("just checked contains_key"));
                }
            }
            Err(ctx.abandon(format!(
                "internal error: unbound local `{name}` in place position"
            )))
        }
        TypedExprKind::Field(base, name) => {
            let base_ty = bodies::unwrap_own(base.ty.clone());
            let Type::Named(sname, targs) = &base_ty else {
                return Err(
                    ctx.abandon("internal error: field base is not a named type in place position")
                );
            };
            let s = resolve_struct_by_type(ctx.program, sname, targs).ok_or_else(|| {
                ctx.abandon(format!("internal error: struct `{sname}` not found"))
            })?;
            let idx = field_index(&s.fields, name, ctx)?;
            let base_val = place_mut(base, env, dstack, loop_marker, ctx)?;
            match base_val {
                Value::Struct(fields) => fields
                    .get_mut(idx)
                    .ok_or_else(|| ctx.abandon("internal error: field index out of range")),
                other => Err(ctx.abandon(format!(
                    "internal error: field base is not a struct value ({other:?})"
                ))),
            }
        }
        TypedExprKind::Index(base, idx_expr) => {
            let idx_v = eval_expr(idx_expr, env, dstack, loop_marker, ctx)?;
            let i = value::as_i128(&idx_v)
                .ok_or_else(|| ctx.abandon("internal error: index is not an integer"))?
                as usize;
            let base_val = place_mut(base, env, dstack, loop_marker, ctx)?;
            match base_val {
                Value::Array(v) => {
                    let len = v.len();
                    v.get_mut(i).ok_or_else(|| {
                        ctx.abandon(format!("index {i} out of bounds (length {len})"))
                    })
                }
                Value::Bytes(_) => Err(ctx.abandon(
                    "assigning into a `Bytes` element is not supported in comptime evaluation yet",
                )),
                other => Err(ctx.abandon(format!(
                    "internal error: index base is not an array value ({other:?})"
                ))),
            }
        }
        _ => Err(ctx.abandon("internal error: expression is not an assignable place")),
    }
}

// --- pattern matching --------------------------------------------------

/// `Some(bindings)` when `pattern` matches `v` (the bindings it
/// introduces, name-ordered by first occurrence — order does not matter,
/// the caller inserts each independently); `None` on a non-match.
fn match_pattern(
    pattern: &TypedPattern,
    v: &Value,
    ctx: &Interp,
) -> R<Option<Vec<(String, Value)>>> {
    match &pattern.kind {
        TypedPatternKind::Wildcard => Ok(Some(vec![])),
        TypedPatternKind::Binding(name) => Ok(Some(vec![(name.clone(), v.clone())])),
        TypedPatternKind::Take(inner) => match_pattern(inner, v, ctx),
        TypedPatternKind::Literal(lit) => {
            // A pattern literal never needs a live `env`/`dstack` walk —
            // it is always a bare scalar/enum-variant literal expression
            // (sema's own pattern grammar), so a throwaway empty
            // env/dstack is correct and unreachable in practice for
            // anything that would need one.
            let mut scratch_env: Env = vec![BTreeMap::new()];
            let mut scratch_dstack: Vec<&TypedDeferBody> = Vec::new();
            let mut scratch = Interp {
                program: ctx.program,
                quota: Quota::new(),
                stack: ctx.stack.clone(),
            };
            let lv = eval_expr(lit, &mut scratch_env, &mut scratch_dstack, 0, &mut scratch)
                .map_err(|_| ctx.abandon("internal error: pattern literal failed to evaluate"))?;
            Ok(if &lv == v { Some(vec![]) } else { None })
        }
        TypedPatternKind::Variant {
            enum_name,
            variant,
            payload,
        } => {
            let Value::Enum(idx, vals) = v else {
                return Err(ctx.abandon("internal error: variant pattern against a non-enum value"));
            };
            let want = variant_index(ctx.program, enum_name, variant, ctx)
                .map_err(|_| ctx.abandon("internal error: unknown variant in pattern"))?;
            if *idx != want {
                return Ok(None);
            }
            let mut out = Vec::new();
            for (p, pv) in payload.iter().zip(vals.iter()) {
                match match_pattern(p, pv, ctx)? {
                    Some(mut b) => out.append(&mut b),
                    None => return Ok(None),
                }
            }
            Ok(Some(out))
        }
        TypedPatternKind::Tuple(items) => {
            let Value::Tuple(vals) = v else {
                return Err(ctx.abandon("internal error: tuple pattern against a non-tuple value"));
            };
            let mut out = Vec::new();
            for (p, pv) in items.iter().zip(vals.iter()) {
                match match_pattern(p, pv, ctx)? {
                    Some(mut b) => out.append(&mut b),
                    None => return Ok(None),
                }
            }
            Ok(Some(out))
        }
        TypedPatternKind::Array(items) => {
            let Value::Array(vals) = v else {
                return Err(ctx.abandon("internal error: array pattern against a non-array value"));
            };
            if items.len() != vals.len() {
                return Ok(None);
            }
            let mut out = Vec::new();
            for (p, pv) in items.iter().zip(vals.iter()) {
                match match_pattern(p, pv, ctx)? {
                    Some(mut b) => out.append(&mut b),
                    None => return Ok(None),
                }
            }
            Ok(Some(out))
        }
        TypedPatternKind::Or(alts) => {
            for alt in alts {
                if let Some(b) = match_pattern(alt, v, ctx)? {
                    return Ok(Some(b));
                }
            }
            Ok(None)
        }
    }
}

// --- calls --------------------------------------------------------------

/// Evaluates a `Call` node's argument slots against the callee's own
/// declared parameters: a supplied slot is evaluated in the *caller's*
/// environment; an elided (defaulted) slot's stored default
/// (`typed::TypedParam::default`) is evaluated in the *callee's own,
/// so-far-assembled* frame (it may reference `self` and/or earlier
/// parameters — `bodies::check_params_with_defaults` types defaults in
/// exactly that incrementally-growing order, so binding proceeds the
/// same way here).
fn bind_params<'a, 'p>(
    f: &'p TypedFn,
    args: &'a [Option<TypedExpr>],
    caller_env: &mut Env,
    caller_dstack: &mut Vec<&'a TypedDeferBody>,
    caller_loop_marker: usize,
    callee_env: &mut Env,
    ctx: &mut Interp<'p>,
) -> R<()> {
    for (param, slot) in f.params.iter().zip(args.iter()) {
        let v = match slot {
            Some(e) => eval_expr(e, caller_env, caller_dstack, caller_loop_marker, ctx)?,
            None => {
                let default = param
                    .default
                    .as_ref()
                    .expect("producer guarantees a default when a call slot is None");
                let mut empty_dstack: Vec<&TypedDeferBody> = Vec::new();
                eval_expr(default, callee_env, &mut empty_dstack, 0, ctx)?
            }
        };
        env_insert(callee_env, param.name.clone(), v);
    }
    Ok(())
}

/// Runs one already-resolved fn/method's body with `self_val` (if any)
/// and positionally-bound `args` (already-evaluated `Value`s — used by
/// `CallValue`, which has no defaults to resolve, decision 1's own
/// "positional only" note). Returns the call's own result value plus,
/// when `self_val` was supplied, the final value of `self` at the end
/// of the callee's own frame (for a `mut`-receiver's write-back).
fn run_call<'p>(
    f: &'p TypedFn,
    self_val: Option<Value>,
    frame_name: String,
    bind: impl FnOnce(&mut Env, &mut Interp<'p>) -> R<()>,
    ctx: &mut Interp<'p>,
) -> R<(Value, Option<Value>)> {
    ctx.enter(frame_name)?;
    let mut env: Env = vec![BTreeMap::new()];
    if let Some(sv) = self_val {
        env_insert(&mut env, "self".to_string(), sv);
    }
    let bind_result = bind(&mut env, ctx);
    let outcome = bind_result.and_then(|()| {
        let mut dstack: Vec<&TypedDeferBody> = Vec::new();
        match exec_block(&f.body, &mut env, &mut dstack, 0, ctx) {
            Ok(()) => Ok(Value::Unit),
            Err(Unwind::Return(v)) => Ok(v),
            Err(Unwind::Break) | Err(Unwind::Continue) => {
                Err(ctx.abandon("internal error: break/continue escaped a function body"))
            }
            Err(e) => Err(e),
        }
    });
    ctx.leave();
    let result = outcome?;
    let final_self = env[0].remove("self");
    Ok((result, final_self))
}

/// `init`'s own special-case (plans/M3.md item B, per the callee-key
/// doc comment on `check_struct_construction`): allocate a fresh `Self`
/// (every field placeholder-filled, since flow's definite-init pass
/// already guarantees the body assigns every one before any real exit),
/// run the body with that as `self`, then translate the body's own
/// (declared, possibly `Result[Unit, E]`) return into the *call's* own
/// synthesized return: implicit/`Ok(())` success wraps the finished
/// `self`; an explicit `Err(e)` propagates `e` and discards `self`.
fn run_init<'p>(
    s: &'p TypedStruct,
    f: &'p TypedFn,
    frame_name: String,
    bind: impl FnOnce(&mut Env, &mut Interp<'p>) -> R<()>,
    ctx: &mut Interp<'p>,
) -> R<Value> {
    let placeholder = Value::Struct(vec![Value::Unit; s.fields.len()]);
    let (body_result, final_self) = run_call(f, Some(placeholder), frame_name, bind, ctx)?;
    let self_val = final_self.expect("run_call always returns `self` back when given one");
    match &f.ret {
        Type::Unit => Ok(self_val),
        Type::Result(_, _) => match body_result {
            Value::Enum(idx, mut payload) if idx == value::RESULT_OK => {
                let _ = payload.pop();
                Ok(Value::Enum(value::RESULT_OK, vec![self_val]))
            }
            Value::Enum(idx, payload) if idx == value::RESULT_ERR => {
                Ok(Value::Enum(value::RESULT_ERR, payload))
            }
            other => Err(ctx.abandon(format!(
                "internal error: `init` returning `Result` produced a non-Result value ({other:?})"
            ))),
        },
        other => Err(ctx.abandon(format!(
            "internal error: `init` with a non-standard return type `{other:?}` (sema's own boundary — this should be unreachable)"
        ))),
    }
}

/// Dispatches one `Call` node's callee key: resolves the target fn/
/// method/`init`, evaluates the receiver (per the target's own declared
/// mode — read/mut/take, decision 4: the mode lives on the callee's
/// declaration, never the call site), and runs it.
fn eval_call<'a, 'p>(
    callee: &CalleeKey,
    receiver: &'a Option<Box<TypedExpr>>,
    args: &'a [Option<TypedExpr>],
    env: &mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<Value> {
    let member_is_init =
        matches!(callee, CalleeKey::Method(_, m) | CalleeKey::MethodInstance(_, m) if m == "init");
    if member_is_init {
        let s = struct_of(ctx.program, callee).ok_or_else(|| {
            ctx.abandon(format!(
                "internal error: struct for `{}` not found",
                callee.spelling()
            ))
        })?;
        let f = resolve_fn(ctx.program, callee).ok_or_else(|| {
            ctx.abandon(format!(
                "internal error: `init` for `{}` not found",
                callee.spelling()
            ))
        })?;
        let frame = callee.spelling();
        return run_init(
            s,
            f,
            frame,
            |callee_env, ictx| bind_params(f, args, env, dstack, loop_marker, callee_env, ictx),
            ctx,
        );
    }
    let f = resolve_fn(ctx.program, callee).ok_or_else(|| {
        ctx.abandon(format!(
            "callee `{}` is not available to comptime evaluation yet (a generic instantiation not yet resolved at this point in the build, plans/M3.md item B's own documented boundary)",
            callee.spelling()
        ))
    })?;
    let mode = f.receiver.as_ref().map(|(m, _)| *m);
    let frame = callee.spelling();
    match (receiver, mode) {
        (Some(recv_expr), Some(mode)) => {
            use crate::syntax::ast::AccessMode;
            match mode {
                AccessMode::Mut => {
                    let place = place_mut(recv_expr, env, dstack, loop_marker, ctx)?;
                    let taken = std::mem::replace(place, Value::Unit);
                    let (result, final_self) = run_call(
                        f,
                        Some(taken),
                        frame,
                        |callee_env, ictx| {
                            bind_params(f, args, env, dstack, loop_marker, callee_env, ictx)
                        },
                        ctx,
                    )?;
                    if let Some(sv) = final_self {
                        let place = place_mut(recv_expr, env, dstack, loop_marker, ctx)?;
                        *place = sv;
                    }
                    Ok(result)
                }
                AccessMode::Read | AccessMode::Take => {
                    let sv = eval_expr(recv_expr, env, dstack, loop_marker, ctx)?;
                    let (result, _) = run_call(
                        f,
                        Some(sv),
                        frame,
                        |callee_env, ictx| {
                            bind_params(f, args, env, dstack, loop_marker, callee_env, ictx)
                        },
                        ctx,
                    )?;
                    Ok(result)
                }
            }
        }
        _ => {
            let (result, _) = run_call(
                f,
                None,
                frame,
                |callee_env, ictx| bind_params(f, args, env, dstack, loop_marker, callee_env, ictx),
                ctx,
            )?;
            Ok(result)
        }
    }
}

// --- expressions ----------------------------------------------------------

fn eval_expr<'a, 'p>(
    expr: &'a TypedExpr,
    env: &mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<Value> {
    match &expr.kind {
        TypedExprKind::Int(text) => {
            let raw = value::parse_int_literal(text)
                .ok_or_else(|| ctx.abandon("internal error: invalid integer literal text"))?;
            Ok(match &expr.ty {
                Type::F32 => Value::F32(raw as f32),
                Type::F64 => Value::F64(raw as f64),
                t => value::make_int(t, raw),
            })
        }
        TypedExprKind::Float(text) => {
            let f: f64 = text
                .parse()
                .map_err(|_| ctx.abandon("internal error: invalid float literal text"))?;
            Ok(match &expr.ty {
                Type::F32 => Value::F32(f as f32),
                _ => Value::F64(f),
            })
        }
        TypedExprKind::Str(text) => {
            let bytes = value::decode_str(text);
            ctx.charge(bytes.len() as u64)?;
            Ok(Value::Str(bytes))
        }
        TypedExprKind::BStr(text) => {
            let bytes = value::decode_bstr(text);
            ctx.charge(bytes.len() as u64)?;
            Ok(Value::Bytes(bytes))
        }
        TypedExprKind::Char(text) => Ok(Value::Char(value::decode_char(text))),
        TypedExprKind::Bool(b) => Ok(Value::Bool(*b)),
        TypedExprKind::Unit => Ok(Value::Unit),
        TypedExprKind::Local(name) => env_lookup(env, name)
            .ok_or_else(|| ctx.abandon(format!("internal error: unbound local `{name}`"))),
        TypedExprKind::Const(name) => {
            let c =
                ctx.program.consts.get(name).ok_or_else(|| {
                    ctx.abandon(format!("internal error: const `{name}` not found"))
                })?;
            ctx.enter(name.clone())?;
            let mut cenv: Env = vec![BTreeMap::new()];
            let mut cdstack: Vec<&TypedDeferBody> = Vec::new();
            let result = eval_expr(&c.value, &mut cenv, &mut cdstack, 0, ctx);
            ctx.leave();
            result
        }
        TypedExprKind::FnRef(key) => Ok(Value::Fn(key.clone())),
        TypedExprKind::Field(base, name) => {
            let bv = eval_expr(base, env, dstack, loop_marker, ctx)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            let Type::Named(sname, targs) = &base_ty else {
                return Err(ctx.abandon("internal error: field base is not a named type"));
            };
            let s = resolve_struct_by_type(ctx.program, sname, targs).ok_or_else(|| {
                ctx.abandon(format!("internal error: struct `{sname}` not found"))
            })?;
            let idx = field_index(&s.fields, name, ctx)?;
            match bv {
                Value::Struct(fields) => fields
                    .into_iter()
                    .nth(idx)
                    .ok_or_else(|| ctx.abandon("internal error: field index out of range")),
                other => Err(ctx.abandon(format!(
                    "internal error: field base is not a struct value ({other:?})"
                ))),
            }
        }
        TypedExprKind::Index(base, idx) => {
            let bv = eval_expr(base, env, dstack, loop_marker, ctx)?;
            let iv = eval_expr(idx, env, dstack, loop_marker, ctx)?;
            let i = value::as_i128(&iv)
                .ok_or_else(|| ctx.abandon("internal error: index is not an integer"))?
                as usize;
            match bv {
                Value::Array(v) => {
                    let len = v.len();
                    v.into_iter().nth(i).ok_or_else(|| {
                        ctx.abandon(format!("index {i} out of bounds (length {len})"))
                    })
                }
                Value::Bytes(b) => {
                    let len = b.len();
                    b.get(i).map(|byte| Value::U8(*byte)).ok_or_else(|| {
                        ctx.abandon(format!("index {i} out of bounds (length {len})"))
                    })
                }
                other => Err(ctx.abandon(format!(
                    "internal error: index base is not indexable ({other:?})"
                ))),
            }
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => eval_call(callee, receiver, args, env, dstack, loop_marker, ctx),
        TypedExprKind::CallValue(callee, args) => {
            let cv = eval_expr(callee, env, dstack, loop_marker, ctx)?;
            let mut arg_vals = Vec::with_capacity(args.len());
            for a in args {
                arg_vals.push(eval_expr(a, env, dstack, loop_marker, ctx)?);
            }
            match cv {
                Value::Closure {
                    params,
                    body,
                    env: mut closure_env,
                } => {
                    ctx.enter("<closure>".to_string())?;
                    closure_env.push(BTreeMap::new());
                    for (p, v) in params.iter().zip(arg_vals) {
                        env_insert(&mut closure_env, p.name.clone(), v);
                    }
                    let mut cdstack: Vec<&TypedDeferBody> = Vec::new();
                    let result = match &body {
                        TypedClosureBody::Expr(e) => {
                            eval_expr(e, &mut closure_env, &mut cdstack, 0, ctx)
                        }
                        TypedClosureBody::Suite(stmts) => {
                            match exec_block(stmts, &mut closure_env, &mut cdstack, 0, ctx) {
                                Ok(()) => Ok(Value::Unit),
                                Err(Unwind::Return(v)) => Ok(v),
                                Err(Unwind::Break) | Err(Unwind::Continue) => Err(ctx.abandon(
                                    "internal error: break/continue escaped a closure body",
                                )),
                                Err(e) => Err(e),
                            }
                        }
                    };
                    ctx.leave();
                    result
                }
                Value::Fn(key) => {
                    let f = resolve_fn(ctx.program, &key).ok_or_else(|| {
                        ctx.abandon(format!(
                            "internal error: fn value `{}` not found",
                            key.spelling()
                        ))
                    })?;
                    let frame = key.spelling();
                    let (result, _) = run_call(
                        f,
                        None,
                        frame,
                        |callee_env, _ictx| {
                            for (p, v) in f.params.iter().zip(arg_vals) {
                                env_insert(callee_env, p.name.clone(), v);
                            }
                            Ok(())
                        },
                        ctx,
                    )?;
                    Ok(result)
                }
                other => Err(ctx.abandon(format!("internal error: `{other:?}` is not callable"))),
            }
        }
        TypedExprKind::ToScalar(inner) => {
            let iv = eval_expr(inner, env, dstack, loop_marker, ctx)?;
            value::eval_to_scalar(&expr.ty, &iv).map_err(|m| ctx.abandon(m))
        }
        TypedExprKind::Neg(inner) => {
            let iv = eval_expr(inner, env, dstack, loop_marker, ctx)?;
            value::eval_neg(&iv).map_err(|m| ctx.abandon(m))
        }
        TypedExprKind::BitNot(inner) => {
            let iv = eval_expr(inner, env, dstack, loop_marker, ctx)?;
            Ok(value::eval_bitnot(&expr.ty, &iv))
        }
        TypedExprKind::Take(inner) => {
            // Values move by Rust move; flow already proved legality, so
            // an ordinary (cloning) read is observably identical to a
            // real move here — the source place is never read again
            // (plans/M3.md item B: "you just move").
            eval_expr(inner, env, dstack, loop_marker, ctx)
        }
        TypedExprKind::Try(inner, conv) => {
            let iv = eval_expr(inner, env, dstack, loop_marker, ctx)?;
            let Value::Enum(idx, mut payload) = iv else {
                return Err(ctx.abandon("internal error: `?` on a non-enum value"));
            };
            if idx == value::OPTION_SOME || idx == value::RESULT_OK {
                return Ok(payload
                    .pop()
                    .expect("Some/Ok always carries one payload value"));
            }
            // `None` or `Err(e)`: run every currently active defer (the
            // whole stack — a `?` exit leaves the entire function, just
            // like `return`) before propagating.
            run_defers(&dstack[..], env, ctx)?;
            if idx == value::OPTION_NONE {
                return Err(Unwind::Return(Value::Enum(value::OPTION_NONE, vec![])));
            }
            let e = payload.pop().expect("Err always carries one payload value");
            let converted = match conv {
                None => e,
                Some(key) => {
                    let f = resolve_fn(ctx.program, key).ok_or_else(|| {
                        ctx.abandon(format!(
                            "internal error: `from` conversion `{}` not found",
                            key.spelling()
                        ))
                    })?;
                    let frame = key.spelling();
                    let (result, _) = run_call(
                        f,
                        None,
                        frame,
                        |callee_env, _ictx| {
                            let pname =
                                f.params.first().map(|p| p.name.clone()).unwrap_or_default();
                            env_insert(callee_env, pname, e);
                            Ok(())
                        },
                        ctx,
                    )?;
                    result
                }
            };
            Err(Unwind::Return(Value::Enum(
                value::RESULT_ERR,
                vec![converted],
            )))
        }
        TypedExprKind::Binary(op, l, r) => eval_binary(*op, l, r, env, dstack, loop_marker, ctx),
        TypedExprKind::OpCall(key, l, r) => {
            let lv = eval_expr(l, env, dstack, loop_marker, ctx)?;
            let rv = eval_expr(r, env, dstack, loop_marker, ctx)?;
            let f = resolve_fn(ctx.program, key).ok_or_else(|| {
                ctx.abandon(format!(
                    "internal error: operator method `{}` not found",
                    key.spelling()
                ))
            })?;
            let frame = key.spelling();
            let (result, _) = run_call(
                f,
                Some(lv),
                frame,
                |callee_env, _ictx| {
                    let pname = f.params.first().map(|p| p.name.clone()).unwrap_or_default();
                    env_insert(callee_env, pname, rv);
                    Ok(())
                },
                ctx,
            )?;
            Ok(result)
        }
        TypedExprKind::Is(inner, pattern) => {
            let iv = eval_expr(inner, env, dstack, loop_marker, ctx)?;
            match match_pattern(pattern, &iv, ctx)? {
                Some(bindings) => {
                    for (n, v) in bindings {
                        env_insert(env, n, v);
                    }
                    Ok(Value::Bool(true))
                }
                None => Ok(Value::Bool(false)),
            }
        }
        TypedExprKind::Not(inner) => {
            let iv = eval_expr(inner, env, dstack, loop_marker, ctx)?;
            Ok(Value::Bool(!as_bool(&iv)))
        }
        TypedExprKind::And(l, r) => {
            let lv = eval_expr(l, env, dstack, loop_marker, ctx)?;
            if !as_bool(&lv) {
                return Ok(Value::Bool(false));
            }
            eval_expr(r, env, dstack, loop_marker, ctx)
        }
        TypedExprKind::Or(l, r) => {
            let lv = eval_expr(l, env, dstack, loop_marker, ctx)?;
            if as_bool(&lv) {
                return Ok(Value::Bool(true));
            }
            eval_expr(r, env, dstack, loop_marker, ctx)
        }
        TypedExprKind::EnumConstruct {
            enum_name,
            variant,
            args,
        } => {
            let idx = variant_index(ctx.program, enum_name, variant, ctx)?;
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval_expr(a, env, dstack, loop_marker, ctx)?);
            }
            let v = Value::Enum(idx, vals);
            ctx.charge(v.weight())?;
            Ok(v)
        }
        TypedExprKind::Closure { params, body } => Ok(Value::Closure {
            params: params.clone(),
            body: body.clone(),
            env: env.clone(),
        }),
        TypedExprKind::Tuple(items) => {
            let mut vals = Vec::with_capacity(items.len());
            for i in items {
                vals.push(eval_expr(i, env, dstack, loop_marker, ctx)?);
            }
            let v = Value::Tuple(vals);
            ctx.charge(v.weight())?;
            Ok(v)
        }
        TypedExprKind::List(items) => {
            let mut vals = Vec::with_capacity(items.len());
            for i in items {
                vals.push(eval_expr(i, env, dstack, loop_marker, ctx)?);
            }
            let v = Value::Array(vals);
            ctx.charge(v.weight())?;
            Ok(v)
        }
        TypedExprKind::StructLiteral { name, fields } => {
            let Type::Named(sname, targs) = &expr.ty else {
                return Err(ctx.abandon("internal error: struct literal type is not Named"));
            };
            debug_assert_eq!(name, sname);
            let s = resolve_struct_by_type(ctx.program, sname, targs).ok_or_else(|| {
                ctx.abandon(format!("internal error: struct `{sname}` not found"))
            })?;
            let mut slots: Vec<Option<Value>> = vec![None; s.fields.len()];
            for (fname, fval) in fields {
                let idx = field_index(&s.fields, fname, ctx)?;
                slots[idx] = Some(eval_expr(fval, env, dstack, loop_marker, ctx)?);
            }
            for (i, fname) in s.fields.iter().enumerate() {
                if slots[i].is_none() {
                    let default = s.field_defaults.get(fname).ok_or_else(|| {
                        ctx.abandon(format!(
                            "internal error: field `{fname}` has neither a supplied value nor a default"
                        ))
                    })?;
                    let mut fenv: Env = vec![BTreeMap::new()];
                    slots[i] = Some(eval_expr(default, &mut fenv, &mut Vec::new(), 0, ctx)?);
                }
            }
            let vals: Vec<Value> = slots
                .into_iter()
                .map(|s| s.expect("every slot filled above"))
                .collect();
            let v = Value::Struct(vals);
            ctx.charge(v.weight())?;
            Ok(v)
        }
        TypedExprKind::Panic(msg) => {
            let mv = eval_expr(msg, env, dstack, loop_marker, ctx)?;
            Err(ctx.abandon(format!("panic: {}", render_message(&mv))))
        }
    }
}

fn eval_binary<'a, 'p>(
    op: BinOp,
    l: &'a TypedExpr,
    r: &'a TypedExpr,
    env: &mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<Value> {
    let lv = eval_expr(l, env, dstack, loop_marker, ctx)?;
    let rv = eval_expr(r, env, dstack, loop_marker, ctx)?;
    use BinOp::*;
    match op {
        Add | Sub | Mul => {
            // Scalar arithmetic's own operand type is `l.ty` (float or
            // integer); floats never abandon on ordinary `+ - *`.
            match (&lv, &rv) {
                (Value::F32(a), Value::F32(b)) => Ok(match op {
                    Add => Value::F32(a + b),
                    Sub => Value::F32(a - b),
                    Mul => Value::F32(a * b),
                    _ => unreachable!(),
                }),
                (Value::F64(a), Value::F64(b)) => Ok(match op {
                    Add => Value::F64(a + b),
                    Sub => Value::F64(a - b),
                    Mul => Value::F64(a * b),
                    _ => unreachable!(),
                }),
                _ => value::eval_ordinary(op, &l.ty, &lv, &rv).map_err(|m| ctx.abandon(m)),
            }
        }
        AddW | SubW | MulW => Ok(value::eval_wrapping(op, &l.ty, &lv, &rv)),
        Div | Rem => match (&lv, &rv) {
            (Value::F32(a), Value::F32(b)) => Ok(match op {
                Div => Value::F32(a / b),
                Rem => Value::F32(a % b),
                _ => unreachable!(),
            }),
            (Value::F64(a), Value::F64(b)) => Ok(match op {
                Div => Value::F64(a / b),
                Rem => Value::F64(a % b),
                _ => unreachable!(),
            }),
            _ => value::eval_div_rem(op, &l.ty, &lv, &rv).map_err(|m| ctx.abandon(m)),
        },
        Shl | Shr => value::eval_shift(op, &l.ty, &lv, &rv).map_err(|m| ctx.abandon(m)),
        BitAnd | BitOr | BitXor => Ok(value::eval_bitwise(op, &l.ty, &lv, &rv)),
        Lt | Le | Gt | Ge => Ok(Value::Bool(value::eval_compare(op, &lv, &rv))),
        Eq => Ok(Value::Bool(lv == rv)),
        Ne => Ok(Value::Bool(lv != rv)),
    }
}

// --- integration tests ---------------------------------------------------
//
// Full pipeline (lex -> parse -> `sema::check_typed` -> `eval_const`), real
// production code rather than hand-built typed trees — each probes one
// piece of item B's own required coverage list end to end.
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

    /// M3-A's own acceptance probe (plans/M3.md's task note on `for take
    /// x in take arr`): the typed tree records `take_binding` per element
    /// but strips the iterable's own outer `take` — this proves move
    /// semantics still evaluate correctly (Rust move == an ordinary
    /// value read here, decision 4's own "you just move") by actually
    /// summing every element of a whole-array take-consuming loop.
    #[test]
    fn for_take_consume_moves_each_element_correctly() {
        let program = typed_program(
            "module examples.eval_for_take

pub fn sum_and_consume(take arr: [u64; 4]) -> u64:
    total: u64 = 0
    for take x in take arr:
        total = total + x
    return total

pub fn make_and_consume() -> u64:
    arr: [u64; 4] = [1, 2, 3, 4]
    return sum_and_consume(take arr)

const RESULT: u64 = make_and_consume()
",
        );
        let v = eval_const(&program, "RESULT").expect("must evaluate cleanly");
        assert_eq!(v, Value::U64(10));
    }

    #[test]
    fn struct_construction_field_access_and_method_call() {
        let program = typed_program(
            "module examples.eval_struct

pub struct Point:
    x: u64
    y: u64

    pub fn sum(read self) -> u64:
        return self.x + self.y

const RESULT: u64 = Point(x=3, y=4).sum()
",
        );
        let v = eval_const(&program, "RESULT").expect("must evaluate cleanly");
        assert_eq!(v, Value::U64(7));
    }

    /// A `mut self` method's mutation is visible in the caller's own
    /// place afterward (`eval_call`'s take-then-write-back path), not
    /// just inside the method's own local frame.
    #[test]
    fn mut_receiver_method_mutates_the_caller_place() {
        let program = typed_program(
            "module examples.eval_mut_receiver

pub struct Counter:
    value: u64

    pub fn increment(mut self, by: u64):
        self.value = self.value + by

pub fn use_counter() -> u64:
    c = Counter(value=10)
    c.increment(5)
    c.increment(2)
    return c.value

const RESULT: u64 = use_counter()
",
        );
        let v = eval_const(&program, "RESULT").expect("must evaluate cleanly");
        assert_eq!(v, Value::U64(17));
    }

    /// `defer` runs at real exit in reverse registration order
    /// (02-language.md §10): the *second*-registered defer runs first.
    /// Observed through a `mut self` method (a bare `return` exit) so
    /// the post-defer mutation is visible afterward — a `return <expr>`
    /// exit evaluates its own expression *before* running defers (there
    /// are no named returns here for a defer to still influence), so a
    /// value returned directly cannot observe a defer's own effect; a
    /// mutated `self` written back to the caller's place after the call
    /// can.
    #[test]
    fn defer_runs_at_exit_in_reverse_registration_order() {
        let program = typed_program(
            "module examples.eval_defer

pub struct Trace:
    log: u64

    pub fn run(mut self):
        defer:
            self.log = self.log * 10 + 1
        defer:
            self.log = self.log * 10 + 2
        self.log = self.log * 10 + 3
        return

pub fn use_trace() -> u64:
    t = Trace(log=0)
    t.run()
    return t.log

const RESULT: u64 = use_trace()
",
        );
        // The body runs first (log -> 3), then defers run in reverse
        // registration order at the bare `return`: the second-
        // registered one (+2) first (log -> 32), then the first-
        // registered one (+1) (-> 321).
        let v = eval_const(&program, "RESULT").expect("must evaluate cleanly");
        assert_eq!(v, Value::U64(321));
    }

    /// Match against a user enum's payload-carrying variant, plus `?`
    /// with no conversion (`Option`'s own case).
    #[test]
    fn match_on_enum_payload_and_try_on_option() {
        let program = typed_program(
            "module examples.eval_match

enum Shape:
    Circle(u64)
    Square(u64)

pub fn area(s: Shape) -> u64:
    match s:
        case .Circle(r):
            return r * r * 3
        case .Square(side):
            return side * side

pub fn double_or_none(o: Option[u64]) -> Option[u64]:
    v = o?
    return Some(v * 2)

pub fn use_try() -> u64:
    match double_or_none(Some(21)):
        case .Some(v):
            return v
        case .None:
            return 0

const AREA: u64 = area(.Square(4))
const TRIED: u64 = use_try()
",
        );
        // `.Square(4)` -> the `case .Square(side)` arm: 4 * 4 = 16.
        assert_eq!(eval_const(&program, "AREA").unwrap(), Value::U64(16));
        assert_eq!(eval_const(&program, "TRIED").unwrap(), Value::U64(42));
    }

    /// Closures: non-escaping, direct application, passed positionally.
    #[test]
    fn closure_direct_application() {
        let program = typed_program(
            "module examples.eval_closure

pub fn apply_twice(f: fn(u64) -> u64, x: u64) -> u64:
    return f(f(x))

const RESULT: u64 = apply_twice(|v: u64| v * 2, 3)
",
        );
        let v = eval_const(&program, "RESULT").expect("must evaluate cleanly");
        assert_eq!(v, Value::U64(12));
    }

    /// Explicit `panic` abandons with the live comptime call stack
    /// (decision 5) — the function has an (unreachable in practice, but
    /// statically present) trailing `return` so sema's own "missing
    /// return on some path" check accepts it; the interpreter still
    /// never reaches that `return`, since `panic` aborts evaluation
    /// immediately. `sema::check_typed` itself now runs every `const`
    /// eagerly (`eval::check_consts`, this item's own integration
    /// surface), so the abandonment surfaces as `check_typed`'s own
    /// `Err` directly — there is no separately-successful typed program
    /// left to call `eval_const` on afterward.
    #[test]
    fn explicit_panic_abandons_with_call_stack() {
        let src = "module examples.eval_panic

pub fn always_fails() -> u64:
    panic(\"deliberate\")
    return 0

const RESULT: u64 = always_fails()
";
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        let err = sema::check_typed(&module, "<test>").expect_err("must abandon at check_typed");
        assert_eq!(err.category, "comptime");
        assert_eq!(err.message, "panic: deliberate");
        assert!(err.omit_location);
        assert_eq!(
            err.extra_lines,
            vec![
                "  while evaluating `RESULT`".to_string(),
                "  while evaluating `always_fails`".to_string(),
            ]
        );
    }
}
