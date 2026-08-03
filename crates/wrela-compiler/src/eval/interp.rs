use std::collections::BTreeMap;

use crate::eval::quota::{MAX_CALL_DEPTH, Quota};
use crate::eval::value::{self, Env, Value};
use crate::sema::bodies::{self, InstKind};
use crate::sema::generics;
use crate::sema::typed::{
    CalleeKey, TypedCallArg, TypedClosureBody, TypedDeferBody, TypedEnum, TypedExpr, TypedExprKind,
    TypedFn, TypedForIter, TypedInstantiation, TypedPattern, TypedPatternKind, TypedProgram,
    TypedStmt, TypedStmtKind, TypedStruct,
};
use crate::sema::types::{Type, TypeArg};
use crate::syntax::ast::{AccessMode, BinOp};

#[derive(Debug, Clone, PartialEq)]
pub struct EvalError {
    pub message: String,
    pub stack: Vec<String>,
}

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

const EVAL_STACK_SIZE: usize = 256 * 1024 * 1024;

fn run_on_guarded_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        let handle = std::thread::Builder::new()
            .stack_size(EVAL_STACK_SIZE)
            .spawn_scoped(scope, f)
            .expect("spawn comptime-eval thread");
        match handle.join() {
            Ok(v) => v,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

struct Interp<'p> {
    program: &'p TypedProgram,
    quota: Quota,
    stack: Vec<String>,
    image: Option<crate::eval::image::ImageGraph>,
    sealed_image: Option<crate::eval::image::ImageGraph>,
    slotmap_next_id: u64,
}

impl<'p> Interp<'p> {
    fn abandon(&self, message: impl Into<String>) -> Unwind {
        Unwind::Error(EvalError {
            message: message.into(),
            stack: self.stack.clone(),
        })
    }

    fn abandon_missing(&self, name: &str, fallback: impl Into<String>) -> Unwind {
        match self.program.imported.unresolvable.get(name) {
            Some(note) => self.abandon(format!("`{name}` {note}")),
            None => self.abandon(fallback),
        }
    }

    fn tick_step(&mut self) -> R<()> {
        self.quota.tick_step().map_err(|m| self.abandon(m))
    }

    fn charge(&mut self, n: u64) -> R<()> {
        self.quota.charge_memory(n).map_err(|m| self.abandon(m))
    }

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

pub fn eval_const(program: &TypedProgram, name: &str) -> Result<Value, EvalError> {
    let Some(c) = program
        .consts
        .get(name)
        .or_else(|| program.imported.consts.get(name))
    else {
        return Err(EvalError {
            message: format!("internal error: const `{name}` not found in the checked program"),
            stack: vec![],
        });
    };
    eval_top(program, &c.value, name.to_string())
}

pub fn eval_standalone(
    program: &TypedProgram,
    expr: &TypedExpr,
    context: String,
) -> Result<Value, EvalError> {
    eval_top(program, expr, context)
}

pub fn eval_test(program: &TypedProgram, name: &str) -> Result<Value, EvalError> {
    let Some(f) = program.fns.get(name) else {
        return Err(EvalError {
            message: format!("internal error: test fn `{name}` not found in the checked program"),
            stack: vec![],
        });
    };
    run_on_guarded_stack(move || {
        let mut ctx = Interp {
            program,
            quota: Quota::new(),
            stack: Vec::new(),
            image: None,
            sealed_image: None,
            slotmap_next_id: 1,
        };
        match run_call(f, None, name.to_string(), |_, _| Ok(()), &mut ctx) {
            Ok(outcome) => Ok(outcome.result),
            Err(u) => Err(unwind_to_error(u)),
        }
    })
}

pub fn eval_test_case(
    program: &TypedProgram,
    name: &str,
    args: &[Value],
) -> Result<Value, EvalError> {
    let Some(f) = program.fns.get(name) else {
        return Err(EvalError {
            message: format!("internal error: test fn `{name}` not found in the checked program"),
            stack: vec![],
        });
    };
    run_on_guarded_stack(move || {
        let mut ctx = Interp {
            program,
            quota: Quota::new(),
            stack: Vec::new(),
            image: None,
            sealed_image: None,
            slotmap_next_id: 1,
        };
        let bind = |env: &mut Env, _ctx: &mut Interp| {
            for (p, v) in f.params.iter().zip(args.iter()) {
                env_insert(env, p.name.clone(), v.clone());
            }
            Ok(())
        };
        match run_call(f, None, name.to_string(), bind, &mut ctx) {
            Ok(outcome) => Ok(outcome.result),
            Err(u) => Err(unwind_to_error(u)),
        }
    })
}

pub fn eval_layout_assert(
    program: &TypedProgram,
    name: &str,
    f: &TypedFn,
    report: Value,
) -> Result<(), EvalError> {
    run_on_guarded_stack(move || {
        let mut ctx = Interp {
            program,
            quota: Quota::new(),
            stack: Vec::new(),
            image: None,
            sealed_image: None,
            slotmap_next_id: 1,
        };
        let bind = |env: &mut Env, ctx: &mut Interp| {
            let Some(p) = f.params.first() else {
                return Err(ctx.abandon(format!(
                    "`@layout_assert` fn `{name}` takes no parameters (expected `report: ImageReport`)"
                )));
            };
            env_insert(env, p.name.clone(), report.clone());
            Ok(())
        };
        match run_call(f, None, name.to_string(), bind, &mut ctx) {
            Ok(_) => Ok(()),
            Err(u) => Err(unwind_to_error(u)),
        }
    })
}

pub fn eval_image(
    program: &TypedProgram,
    fn_name: &str,
) -> Result<crate::eval::image::ImageGraph, EvalError> {
    let Some(f) = program.fns.get(fn_name) else {
        return Err(EvalError {
            message: format!(
                "internal error: `@image` fn `{fn_name}` not found in the checked program"
            ),
            stack: vec![],
        });
    };
    run_on_guarded_stack(move || {
        let mut ctx = Interp {
            program,
            quota: Quota::new(),
            stack: Vec::new(),
            image: None,
            sealed_image: None,
            slotmap_next_id: 1,
        };
        match run_call(f, None, fn_name.to_string(), |_, _| Ok(()), &mut ctx) {
            Ok(_) => ctx.sealed_image.ok_or_else(|| EvalError {
                message: format!("`@image` fn `{fn_name}` returned without calling `img.seal()`"),
                stack: vec![fn_name.to_string()],
            }),
            Err(u) => Err(unwind_to_error(u)),
        }
    })
}

fn eval_top(program: &TypedProgram, expr: &TypedExpr, context: String) -> Result<Value, EvalError> {
    run_on_guarded_stack(move || {
        let mut ctx = Interp {
            program,
            quota: Quota::new(),
            stack: Vec::new(),
            image: None,
            sealed_image: None,
            slotmap_next_id: 1,
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
    })
}

fn unwind_to_error(u: Unwind) -> EvalError {
    match u {
        Unwind::Error(e) => e,
        Unwind::Return(_) | Unwind::Break | Unwind::Continue => EvalError {
            message: "internal error: control flow escaped a top-level comptime expression"
                .to_string(),
            stack: vec![],
        },
    }
}

fn struct_by_name<'p>(program: &'p TypedProgram, name: &str) -> Option<&'p TypedStruct> {
    program
        .structs
        .get(name)
        .or_else(|| program.imported.structs.get(name))
}

fn enum_by_name<'p>(program: &'p TypedProgram, name: &str) -> Option<&'p TypedEnum> {
    program
        .enums
        .get(name)
        .or_else(|| program.imported.enums.get(name))
}

fn instantiation_by_key<'p>(
    program: &'p TypedProgram,
    key: &str,
) -> Option<&'p TypedInstantiation> {
    program
        .instantiations
        .get(key)
        .or_else(|| program.imported.instantiations.get(key))
}

fn resolve_fn<'p>(program: &'p TypedProgram, key: &CalleeKey) -> Option<&'p TypedFn> {
    match key {
        CalleeKey::Fn(name) => program
            .fns
            .get(name)
            .or_else(|| program.imported.fns.get(name)),
        CalleeKey::FnInstance(ikey) => match instantiation_by_key(program, ikey) {
            Some(TypedInstantiation::Fn(f)) => Some(f),
            _ => None,
        },
        CalleeKey::Method(sname, member) => {
            if let Some(s) = struct_by_name(program, sname) {
                return resolve_struct_member(s, member);
            }
            resolve_enum_member(enum_by_name(program, sname)?, member)
        }
        CalleeKey::MethodInstance(ikey, member) => match instantiation_by_key(program, ikey) {
            Some(TypedInstantiation::Struct(s)) => resolve_struct_member(s, member),
            _ => None,
        },
    }
}

pub(crate) fn callee_decl_name(key: &CalleeKey) -> String {
    let raw = match key {
        CalleeKey::Fn(name) => name.clone(),
        CalleeKey::Method(sname, _) => sname.clone(),
        CalleeKey::FnInstance(k) | CalleeKey::MethodInstance(k, _) => k.clone(),
    };
    let no_prefix = raw
        .strip_prefix("fn:")
        .or_else(|| raw.strip_prefix("struct:"))
        .or_else(|| raw.strip_prefix("method:"))
        .unwrap_or(&raw);
    let before_args = no_prefix.split('[').next().unwrap_or(no_prefix);
    before_args
        .split('.')
        .next()
        .unwrap_or(before_args)
        .to_string()
}

fn resolve_struct_member<'p>(s: &'p TypedStruct, member: &str) -> Option<&'p TypedFn> {
    if member == "init" {
        s.init.as_ref()
    } else {
        s.methods.get(member).or_else(|| s.assoc_fns.get(member))
    }
}

fn resolve_enum_member<'p>(e: &'p TypedEnum, member: &str) -> Option<&'p TypedFn> {
    e.methods.get(member).or_else(|| e.assoc_fns.get(member))
}

fn resolve_struct_by_type<'p>(
    program: &'p TypedProgram,
    name: &str,
    targs: &[TypeArg],
) -> Option<&'p TypedStruct> {
    if targs.is_empty() {
        struct_by_name(program, name)
    } else {
        let key = generics::canonical_key(InstKind::Struct, name, targs);
        match instantiation_by_key(program, &key) {
            Some(TypedInstantiation::Struct(s)) => Some(s),
            _ => None,
        }
    }
}

fn struct_of<'p>(program: &'p TypedProgram, key: &CalleeKey) -> Option<&'p TypedStruct> {
    match key {
        CalleeKey::Method(sname, _) => struct_by_name(program, sname),
        CalleeKey::MethodInstance(ikey, _) => match instantiation_by_key(program, ikey) {
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
        "CallError" => crate::sema::bodies::call_error_variant_index(variant).ok_or_else(|| {
            ctx.abandon(format!(
                "internal error: unknown CallError variant `{variant}`"
            ))
        }),
        _ => {
            let Some(en) = program
                .enums
                .get(enum_name)
                .or_else(|| program.imported.enums.get(enum_name))
            else {
                return Err(ctx.abandon_missing(
                    enum_name,
                    format!(
                        "evaluating a generic enum instantiation's variant (`{enum_name}.{variant}`) is not supported yet"
                    ),
                ));
            };
            en.variants
                .iter()
                .position(|v| v == variant)
                .ok_or_else(|| {
                    ctx.abandon(format!(
                        "internal error: unknown variant `{enum_name}.{variant}`"
                    ))
                })
        }
    }
}

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

fn scoped_env<T>(env: &mut Env, f: impl FnOnce(&mut Env) -> R<T>) -> R<T> {
    env.push(BTreeMap::new());
    let result = f(env);
    env.pop();
    result
}

fn run_defers<'a, 'p>(defers: &[&'a TypedDeferBody], env: &mut Env, ctx: &mut Interp<'p>) -> R<()> {
    for d in defers.iter().rev() {
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
                return scoped_env(env, |env| {
                    exec_block(then_branch, env, dstack, loop_marker, ctx)
                });
            }
            for elif in elifs {
                let ec = eval_expr(&elif.cond, env, dstack, loop_marker, ctx)?;
                if as_bool(&ec) {
                    return scoped_env(env, |env| {
                        exec_block(&elif.body, env, dstack, loop_marker, ctx)
                    });
                }
            }
            if let Some(b) = else_branch {
                return scoped_env(env, |env| exec_block(b, env, dstack, loop_marker, ctx));
            }
            Ok(())
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            let sv = eval_expr(scrutinee, env, dstack, loop_marker, ctx)?;
            for arm in arms {
                if let Some(bindings) = match_pattern(&arm.pattern, &sv, ctx)? {
                    let matched = scoped_env(env, |env| -> R<bool> {
                        for (n, v) in &bindings {
                            env_insert(env, n.clone(), v.clone());
                        }
                        if let Some(guard) = &arm.guard {
                            let gv = eval_expr(guard, env, dstack, loop_marker, ctx)?;
                            if !as_bool(&gv) {
                                return Ok(false);
                            }
                        }
                        exec_block(&arm.body, env, dstack, loop_marker, ctx)?;
                        Ok(true)
                    })?;
                    if matched {
                        return Ok(());
                    }
                }
            }
            Err(ctx.abandon(
                "match: no arm matched (exhaustiveness already proved this cannot happen)",
            ))
        }
        TypedStmtKind::While { cond, body, budget } => {
            let new_marker = dstack.len();
            let mut trips: u64 = 0;
            loop {
                let c = eval_expr(cond, env, dstack, loop_marker, ctx)?;
                if !as_bool(&c) {
                    break;
                }
                if let Some(n) = budget {
                    trips = trips.saturating_add(1);
                    if trips > *n {
                        return Err(ctx.abandon("loop budget exceeded"));
                    }
                }
                ctx.tick_step()?;
                match scoped_env(env, |env| exec_block(body, env, dstack, new_marker, ctx)) {
                    Ok(()) => {}
                    Err(Unwind::Break) => break,
                    Err(Unwind::Continue) => {}
                    Err(other) => return Err(other),
                }
            }
            Ok(())
        }
        TypedStmtKind::For {
            name,
            iter,
            body,
            budget,
            ..
        } => exec_for(name, iter, body, *budget, env, dstack, loop_marker, ctx),
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
        TypedStmtKind::ComptimeAssert { .. } => Ok(()),
        TypedStmtKind::Defer(body) => {
            dstack.push(body);
            Ok(())
        }
        TypedStmtKind::ExprStmt(e) => {
            eval_expr(e, env, dstack, loop_marker, ctx)?;
            Ok(())
        }
        TypedStmtKind::WithGroup { .. } => Err(ctx.abandon(
            "internal error: `with group` reached the comptime evaluator (unreachable — \
             `eval::legal` marks every containing fn illegal for comptime)",
        )),
        TypedStmtKind::BareSend { .. } => Err(ctx.abandon(
            "internal error: a bare `send` statement reached the comptime evaluator \
             (unreachable — `eval::legal` marks every containing fn illegal for comptime)",
        )),
    }
}

fn exec_for<'a, 'p>(
    name: &str,
    iter: &'a TypedForIter,
    body: &'a [TypedStmt],
    budget: Option<u64>,
    env: &mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<()> {
    let new_marker = dstack.len();
    let mut trips: u64 = 0;
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
                if let Some(n) = budget {
                    trips = trips.saturating_add(1);
                    if trips > n {
                        return Err(ctx.abandon("loop budget exceeded"));
                    }
                }
                ctx.tick_step()?;
                let outcome = scoped_env(env, |env| {
                    env_insert(env, name.to_string(), value::make_int(&elem_ty, i));
                    exec_block(body, env, dstack, new_marker, ctx)
                });
                match outcome {
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
                if let Some(n) = budget {
                    trips = trips.saturating_add(1);
                    if trips > n {
                        return Err(ctx.abandon("loop budget exceeded"));
                    }
                }
                ctx.tick_step()?;
                let outcome = scoped_env(env, |env| {
                    env_insert(env, name.to_string(), elem);
                    exec_block(body, env, dstack, new_marker, ctx)
                });
                match outcome {
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

pub(crate) fn render_message(v: &Value) -> String {
    match v {
        Value::Str(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        other => format!("{other:?}"),
    }
}

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
                ctx.abandon_missing(sname, format!("internal error: struct `{sname}` not found"))
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
            let mut scratch_env: Env = vec![BTreeMap::new()];
            let mut scratch_dstack: Vec<&TypedDeferBody> = Vec::new();
            let mut scratch = Interp {
                program: ctx.program,
                quota: Quota::new(),
                stack: ctx.stack.clone(),
                image: None,
                sealed_image: None,
                slotmap_next_id: 1,
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

fn bind_params<'a, 'p>(
    f: &'p TypedFn,
    args: &'a [TypedCallArg],
    caller_env: &mut Env,
    caller_dstack: &mut Vec<&'a TypedDeferBody>,
    caller_loop_marker: usize,
    callee_env: &mut Env,
    ctx: &mut Interp<'p>,
) -> R<()> {
    for (param, slot) in f.params.iter().zip(args.iter()) {
        let v = match &slot.value {
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

struct CallOutcome {
    result: Value,
    final_self: Option<Value>,
    mut_params: Vec<(usize, Value)>,
}

fn run_call<'p>(
    f: &'p TypedFn,
    self_val: Option<Value>,
    frame_name: String,
    bind: impl FnOnce(&mut Env, &mut Interp<'p>) -> R<()>,
    ctx: &mut Interp<'p>,
) -> R<CallOutcome> {
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
    let mut mut_params = Vec::new();
    for (i, param) in f.params.iter().enumerate() {
        if param.mode == AccessMode::Mut {
            let v = env[0].remove(&param.name).ok_or_else(|| {
                ctx.abandon(format!(
                    "internal error: `mut` parameter `{}` missing from callee frame at return",
                    param.name
                ))
            })?;
            mut_params.push((i, v));
        }
    }
    Ok(CallOutcome {
        result,
        final_self,
        mut_params,
    })
}

fn write_back_mut_params<'a, 'p>(
    f: &TypedFn,
    args: &'a [TypedCallArg],
    mut_params: Vec<(usize, Value)>,
    env: &mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<()> {
    for (i, val) in mut_params {
        let param = &f.params[i];
        let Some(arg_expr) = args.get(i).and_then(|s| s.value.as_ref()) else {
            return Err(ctx.abandon(format!(
                "writing back `mut` parameter `{}` through a defaulted argument is not supported",
                param.name
            )));
        };
        let place = place_mut(arg_expr, env, dstack, loop_marker, ctx)?;
        *place = val;
    }
    Ok(())
}

fn run_init<'p>(
    s: &'p TypedStruct,
    f: &'p TypedFn,
    frame_name: String,
    bind: impl FnOnce(&mut Env, &mut Interp<'p>) -> R<()>,
    ctx: &mut Interp<'p>,
) -> R<Value> {
    let placeholder = Value::Struct(vec![Value::Unit; s.fields.len()]);
    let outcome = run_call(f, Some(placeholder), frame_name, bind, ctx)?;
    let mut self_val = outcome
        .final_self
        .expect("run_call always returns `self` back when given one");
    if !outcome.mut_params.is_empty() {
        let names: Vec<&str> = outcome
            .mut_params
            .iter()
            .map(|(i, _)| f.params[*i].name.as_str())
            .collect();
        return Err(ctx.abandon(format!(
            "writing back `mut` parameter(s) `{}` from `init` is not supported",
            names.join("`, `")
        )));
    }
    if crate::mwir::is_slotmap_type_name(&s.name) {
        let id = ctx.slotmap_next_id;
        if id == 0 {
            return Err(ctx.abandon(
                "SlotMap instance id space exhausted (u64 non-wrapping mint, 05-library.md §7)",
            ));
        }
        ctx.slotmap_next_id = id.wrapping_add(1);
        match &mut self_val {
            Value::Struct(fields) if !fields.is_empty() => {
                fields[0] = Value::U64(id);
            }
            _ => {
                return Err(
                    ctx.abandon("internal error: SlotMap init did not produce a struct value")
                );
            }
        }
    }
    match &f.ret {
        Type::Unit => Ok(self_val),
        Type::Result(_, _) => match outcome.result {
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

fn eval_call<'a, 'p>(
    callee: &CalleeKey,
    receiver: &'a Option<Box<TypedExpr>>,
    args: &'a [TypedCallArg],
    env: &mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<Value> {
    if let CalleeKey::Method(_, m) = callee {
        if m == "format" {
            if let Some(recv) = receiver {
                if crate::sema::types::scalar_format_bound(&recv.ty).is_some() {
                    let sv = eval_expr(recv, env, dstack, loop_marker, ctx)?;
                    return Ok(value::format_scalar(&sv));
                }
            }
        }
    }
    let member_is_init =
        matches!(callee, CalleeKey::Method(_, m) | CalleeKey::MethodInstance(_, m) if m == "init");
    if member_is_init {
        let s = struct_of(ctx.program, callee).ok_or_else(|| {
            ctx.abandon_missing(
                &callee_decl_name(callee),
                format!(
                    "internal error: struct for `{}` not found",
                    callee.spelling()
                ),
            )
        })?;
        let f = resolve_fn(ctx.program, callee).ok_or_else(|| {
            ctx.abandon_missing(
                &callee_decl_name(callee),
                format!(
                    "internal error: `init` for `{}` not found",
                    callee.spelling()
                ),
            )
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
        ctx.abandon_missing(
            &callee_decl_name(callee),
            format!(
                "callee `{}` is not available to comptime evaluation yet (a generic instantiation not yet resolved at this point in the build, plans/M3.md item B's own documented boundary)",
                callee.spelling()
            ),
        )
    })?;
    let mode = f.receiver.as_ref().map(|(m, _)| *m);
    let frame = callee.spelling();
    match (receiver, mode) {
        (Some(recv_expr), Some(mode)) => match mode {
            AccessMode::Mut => {
                let place = place_mut(recv_expr, env, dstack, loop_marker, ctx)?;
                let taken = std::mem::replace(place, Value::Unit);
                let outcome = run_call(
                    f,
                    Some(taken),
                    frame,
                    |callee_env, ictx| {
                        bind_params(f, args, env, dstack, loop_marker, callee_env, ictx)
                    },
                    ctx,
                )?;
                if let Some(sv) = outcome.final_self {
                    let place = place_mut(recv_expr, env, dstack, loop_marker, ctx)?;
                    *place = sv;
                }
                write_back_mut_params(f, args, outcome.mut_params, env, dstack, loop_marker, ctx)?;
                Ok(outcome.result)
            }
            AccessMode::Read | AccessMode::Take => {
                let sv = eval_expr(recv_expr, env, dstack, loop_marker, ctx)?;
                let outcome = run_call(
                    f,
                    Some(sv),
                    frame,
                    |callee_env, ictx| {
                        bind_params(f, args, env, dstack, loop_marker, callee_env, ictx)
                    },
                    ctx,
                )?;
                write_back_mut_params(f, args, outcome.mut_params, env, dstack, loop_marker, ctx)?;
                Ok(outcome.result)
            }
        },
        _ => {
            let outcome = run_call(
                f,
                None,
                frame,
                |callee_env, ictx| bind_params(f, args, env, dstack, loop_marker, callee_env, ictx),
                ctx,
            )?;
            write_back_mut_params(f, args, outcome.mut_params, env, dstack, loop_marker, ctx)?;
            Ok(outcome.result)
        }
    }
}

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
            let c = ctx
                .program
                .consts
                .get(name)
                .or_else(|| ctx.program.imported.consts.get(name))
                .ok_or_else(|| {
                    ctx.abandon_missing(name, format!("internal error: const `{name}` not found"))
                })?;
            ctx.enter(name.clone())?;
            let mut cenv: Env = vec![BTreeMap::new()];
            let mut cdstack: Vec<&TypedDeferBody> = Vec::new();
            let result = eval_expr(&c.value, &mut cenv, &mut cdstack, 0, ctx);
            ctx.leave();
            result
        }
        TypedExprKind::Static(name) => Err(ctx.abandon(format!(
            "internal error: placed static `{name}` has no comptime value (03-hardware.md §3.1)"
        ))),
        TypedExprKind::FnRef(key) => Ok(Value::Fn(key.clone())),
        TypedExprKind::Field(base, name) => {
            let bv = eval_expr(base, env, dstack, loop_marker, ctx)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            if matches!(&base_ty, Type::String(_)) {
                if name != "len" {
                    return Err(ctx.abandon(format!(
                        "internal error: `String[..N]` has no field `{name}`"
                    )));
                }
                return match bv {
                    Value::Str(bytes) => Ok(Value::Usize(bytes.len() as u64)),
                    other => Err(ctx.abandon(format!(
                        "internal error: String.len base is not a Str value ({other:?})"
                    ))),
                };
            }
            let Type::Named(sname, targs) = &base_ty else {
                return Err(ctx.abandon("internal error: field base is not a named type"));
            };
            let s = resolve_struct_by_type(ctx.program, sname, targs).ok_or_else(|| {
                ctx.abandon_missing(sname, format!("internal error: struct `{sname}` not found"))
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
                Value::Str(b) => {
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
            match cv {
                Value::Closure {
                    params,
                    body,
                    env: mut closure_env,
                } => {
                    let mut arg_vals = Vec::with_capacity(args.len());
                    let mut mut_idxs = Vec::new();
                    for (i, (p, a)) in params.iter().zip(args.iter()).enumerate() {
                        if p.mode == AccessMode::Mut {
                            let e = a.value.as_ref().ok_or_else(|| {
                                ctx.abandon(
                                    "internal error: `mut` closure argument missing at call site",
                                )
                            })?;
                            let place = place_mut(e, env, dstack, loop_marker, ctx)?;
                            let taken = std::mem::replace(place, Value::Unit);
                            arg_vals.push(taken);
                            mut_idxs.push(i);
                        } else {
                            let e = a.value.as_ref().ok_or_else(|| {
                                ctx.abandon("internal error: closure argument missing at call site")
                            })?;
                            arg_vals.push(eval_expr(e, env, dstack, loop_marker, ctx)?);
                        }
                    }
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
                    let result = result?;
                    let mut mut_finals = Vec::new();
                    for i in mut_idxs {
                        let name = &params[i].name;
                        let v = closure_env
                            .last_mut()
                            .and_then(|scope| scope.remove(name))
                            .ok_or_else(|| {
                                ctx.abandon(format!(
                                    "internal error: `mut` closure parameter `{name}` missing at return"
                                ))
                            })?;
                        mut_finals.push((i, v));
                    }
                    ctx.leave();
                    for (i, val) in mut_finals {
                        let e = args[i].value.as_ref().ok_or_else(|| {
                            ctx.abandon(
                                "internal error: `mut` closure argument missing for write-back",
                            )
                        })?;
                        let place = place_mut(e, env, dstack, loop_marker, ctx)?;
                        *place = val;
                    }
                    Ok(result)
                }
                Value::Fn(key) => {
                    let f = resolve_fn(ctx.program, &key).ok_or_else(|| {
                        ctx.abandon_missing(
                            &callee_decl_name(&key),
                            format!("internal error: fn value `{}` not found", key.spelling()),
                        )
                    })?;
                    let mut arg_vals = Vec::with_capacity(args.len());
                    for (p, a) in f.params.iter().zip(args.iter()) {
                        if p.mode == AccessMode::Mut {
                            let e = a.value.as_ref().ok_or_else(|| {
                                ctx.abandon("internal error: `mut` argument missing at call site")
                            })?;
                            let place = place_mut(e, env, dstack, loop_marker, ctx)?;
                            let taken = std::mem::replace(place, Value::Unit);
                            arg_vals.push(taken);
                        } else {
                            let e = a.value.as_ref().ok_or_else(|| {
                                ctx.abandon("internal error: argument missing at call site")
                            })?;
                            arg_vals.push(eval_expr(e, env, dstack, loop_marker, ctx)?);
                        }
                    }
                    let frame = key.spelling();
                    let outcome = run_call(
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
                    for (i, val) in outcome.mut_params {
                        let e = args[i].value.as_ref().ok_or_else(|| {
                            ctx.abandon("internal error: `mut` argument missing for write-back")
                        })?;
                        let place = place_mut(e, env, dstack, loop_marker, ctx)?;
                        *place = val;
                    }
                    Ok(outcome.result)
                }
                other => Err(ctx.abandon(format!("internal error: `{other:?}` is not callable"))),
            }
        }
        TypedExprKind::ToScalar(inner) => {
            let iv = eval_expr(inner, env, dstack, loop_marker, ctx)?;
            value::eval_to_scalar(&expr.ty, &iv).map_err(|m| ctx.abandon(m))
        }
        TypedExprKind::Neg(inner) => {
            if let TypedExprKind::Int(text) = &inner.kind {
                let raw = value::parse_int_literal(text)
                    .ok_or_else(|| ctx.abandon("internal error: invalid integer literal text"))?;
                Ok(value::make_int(&expr.ty, -raw))
            } else {
                let iv = eval_expr(inner, env, dstack, loop_marker, ctx)?;
                value::eval_neg(&iv).map_err(|m| ctx.abandon(m))
            }
        }
        TypedExprKind::BitNot(inner) => {
            let iv = eval_expr(inner, env, dstack, loop_marker, ctx)?;
            value::eval_bitnot(&expr.ty, &iv).map_err(|m| ctx.abandon(m))
        }
        TypedExprKind::Take(inner) => eval_expr(inner, env, dstack, loop_marker, ctx),
        TypedExprKind::Try(inner, conv) => {
            let iv = eval_expr(inner, env, dstack, loop_marker, ctx)?;
            let on_option = match &inner.ty {
                Type::Option(_) => true,
                Type::Result(_, _) => false,
                other => {
                    return Err(ctx.abandon(format!(
                        "internal error: `?` on `{}`, which is neither `Option` nor `Result`",
                        crate::sema::types::render_type(other)
                    )));
                }
            };
            let Value::Enum(idx, mut payload) = iv else {
                return Err(ctx.abandon("internal error: `?` on a non-enum value"));
            };
            let succeeded = if on_option {
                idx == value::OPTION_SOME
            } else {
                idx == value::RESULT_OK
            };
            if succeeded {
                return payload
                    .pop()
                    .ok_or_else(|| ctx.abandon("internal error: `Some`/`Ok` with no payload"));
            }
            run_defers(&dstack[..], env, ctx)?;
            if on_option {
                return Err(Unwind::Return(Value::Enum(value::OPTION_NONE, vec![])));
            }
            let e = payload
                .pop()
                .ok_or_else(|| ctx.abandon("internal error: `Err` with no payload"))?;
            let converted = match conv {
                None => e,
                Some(key) => {
                    let f = resolve_fn(ctx.program, key).ok_or_else(|| {
                        ctx.abandon_missing(
                            &callee_decl_name(key),
                            format!(
                                "`?`'s error conversion `{}` is not available",
                                key.spelling()
                            ),
                        )
                    })?;
                    let frame = key.spelling();
                    let outcome = run_call(
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
                    if !outcome.mut_params.is_empty() {
                        return Err(ctx.abandon(
                            "writing back a `mut` parameter from a `?` `from` conversion is not supported",
                        ));
                    }
                    outcome.result
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
                ctx.abandon_missing(
                    &callee_decl_name(key),
                    format!(
                        "internal error: operator method `{}` not found",
                        key.spelling()
                    ),
                )
            })?;
            let frame = key.spelling();
            let outcome = run_call(
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
            if !outcome.mut_params.is_empty() {
                return Err(ctx.abandon(
                    "writing back a `mut` parameter from an operator method call is not supported",
                ));
            }
            Ok(outcome.result)
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
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
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
                ctx.abandon_missing(sname, format!("internal error: struct `{sname}` not found"))
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
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            args,
            ..
        } => eval_intrinsic(
            key,
            receiver,
            type_arg,
            args,
            expr.span,
            env,
            dstack,
            loop_marker,
            ctx,
        ),
        TypedExprKind::PoolName(name) => Ok(Value::Str(name.clone().into_bytes())),
        TypedExprKind::Await(_) => Err(ctx.abandon(
            "internal error: `await` reached the comptime evaluator (unreachable — \
             `eval::legal` marks every containing fn illegal for comptime)",
        )),
        TypedExprKind::Send(_) => Err(ctx.abandon(
            "internal error: `send` reached the comptime evaluator (unreachable — \
             `eval::legal` marks every containing fn illegal for comptime)",
        )),
        TypedExprKind::GroupChild(_) => Err(ctx.abandon(
            "internal error: a group child reference reached the comptime evaluator \
             (unreachable — `eval::legal` marks every containing fn illegal for comptime)",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn eval_intrinsic<'a, 'p>(
    key: &str,
    receiver: &'a Option<Box<TypedExpr>>,
    type_arg: &Option<Type>,
    args: &'a [(String, TypedExpr)],
    span: crate::syntax::ast::Span,
    env: &mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<Value> {
    use crate::eval::image::{DeclArg, ImageGraph, TypedValue};

    ctx.tick_step()?;

    fn split_args<'a>(
        args: &'a [(String, TypedExpr)],
        env: &mut Env,
        dstack: &mut Vec<&'a TypedDeferBody>,
        loop_marker: usize,
        ctx: &mut Interp,
    ) -> R<(Option<String>, Vec<DeclArg>)> {
        let mut pool_name = None;
        let mut out = Vec::with_capacity(args.len());
        for (label, a) in args {
            if let TypedExprKind::PoolName(n) = &a.kind {
                pool_name = Some(n.clone());
                continue;
            }
            let v = eval_expr(a, env, dstack, loop_marker, ctx)?;
            out.push(DeclArg {
                label: label.clone(),
                ty: a.ty.clone(),
                value: v,
                span: a.span,
            });
        }
        Ok((pool_name, out))
    }

    match key {
        "Image" => {
            let mut name_v = None;
            let mut target_v = None;
            let mut cores: Option<usize> = None;
            for (label, a) in args {
                let v = eval_expr(a, env, dstack, loop_marker, ctx)?;
                match label.as_str() {
                    "name" => {
                        name_v = Some(TypedValue {
                            ty: a.ty.clone(),
                            value: v,
                        })
                    }
                    "target" => {
                        target_v = Some(TypedValue {
                            ty: a.ty.clone(),
                            value: v,
                        })
                    }
                    "cores" => {
                        let Some(n) = value::as_i128(&v) else {
                            return Err(ctx.abandon(
                                "`Image(..., cores=...)` requires a comptime usize \
                                 (05-library.md §9)",
                            ));
                        };
                        if n < 1 {
                            return Err(ctx.abandon(format!(
                                "`Image(..., cores={n})` — cores must be a comptime usize ≥ 1 \
                                 (05-library.md §9)"
                            )));
                        }
                        let Ok(n) = usize::try_from(n) else {
                            return Err(ctx.abandon(format!(
                                "`Image(..., cores={n})` does not fit a host `usize`"
                            )));
                        };
                        cores = Some(n);
                    }
                    other => {
                        return Err(ctx.abandon(format!(
                            "`Image(...)` has no `{other}=` argument (05-library.md §9 spells it \
                             `Image(name=..., target=..., cores=N?)`)"
                        )));
                    }
                }
            }
            let (Some(name_v), Some(target_v)) = (name_v, target_v) else {
                return Err(ctx.abandon("`Image(...)` requires both `name` and `target`"));
            };
            if ctx.image.is_some() {
                return Err(ctx.abandon("`Image(...)` was already called once in this evaluation"));
            }
            let mut g = ImageGraph::new(name_v, target_v);
            if let Some(n) = cores {
                g.cores = n;
            }
            ctx.image = Some(g);
            Ok(Value::Unit)
        }
        "Image.device" | "Image.driver" | "Image.actor" | "Image.pool" | "Image.dma_pool"
        | "Image.renderer" => {
            let ty_arg = type_arg
                .clone()
                .ok_or_else(|| ctx.abandon("internal error: missing builder type argument"))?;
            let (pool_name, decl_args) = split_args(args, env, dstack, loop_marker, ctx)?;
            if ctx.image.is_none() {
                return Err(
                    ctx.abandon("no active `Image` builder (`Image(...)` was never called)")
                );
            }
            let result = {
                let g = ctx.image.as_mut().expect("checked above");
                match key {
                    "Image.device" => Ok(g.declare_device(ty_arg, decl_args)),
                    "Image.driver" => Ok(g.declare_driver(ty_arg, decl_args)),
                    "Image.actor" => Ok(g.declare_actor(ty_arg, decl_args)),
                    "Image.renderer" => Ok(g.declare_renderer(ty_arg, decl_args, span)),
                    "Image.pool" => match pool_name {
                        Some(name) => g.declare_pool(name, ty_arg, decl_args),
                        None => Err(
                            "internal error: `img.pool` is missing its own `name=` argument"
                                .to_string(),
                        ),
                    },
                    "Image.dma_pool" => match pool_name {
                        Some(name) => g.declare_dma_pool(name, ty_arg, decl_args),
                        None => Err(
                            "internal error: `img.dma_pool` is missing its own `name=` argument"
                                .to_string(),
                        ),
                    },
                    _ => unreachable!("matched above"),
                }
            };
            result.map_err(|m| ctx.abandon(m))
        }
        "Image.on_failure" => {
            let (_, decl_args) = split_args(args, env, dstack, loop_marker, ctx)?;
            if ctx.image.is_none() {
                return Err(
                    ctx.abandon("no active `Image` builder (`Image(...)` was never called)")
                );
            }
            ctx.image
                .as_mut()
                .expect("checked above")
                .declare_on_failure(decl_args);
            Ok(Value::Unit)
        }
        "Image.check_layout" => {
            let Some((_, f_expr)) = args.first() else {
                return Err(
                    ctx.abandon("internal error: `img.check_layout` is missing its argument")
                );
            };
            let TypedExprKind::FnRef(fkey) = &f_expr.kind else {
                return Err(ctx.abandon(
                    "internal error: `img.check_layout`'s argument is not a fn reference",
                ));
            };
            let fn_key = fkey.spelling();
            if ctx.image.is_none() {
                return Err(
                    ctx.abandon("no active `Image` builder (`Image(...)` was never called)")
                );
            }
            ctx.image
                .as_mut()
                .expect("checked above")
                .declare_check_layout(fn_key);
            Ok(Value::Unit)
        }
        "Image.seal" => {
            let g = ctx
                .image
                .take()
                .ok_or_else(|| ctx.abandon("`img.seal()` called with no active builder"))?;
            let mut g = g;
            g.sealed = true;
            ctx.sealed_image = Some(g);
            Ok(Value::Unit)
        }
        "ImageDecl.handle" => {
            let Some(r) = receiver else {
                return Err(ctx.abandon("internal error: `decl.handle()` is missing its receiver"));
            };
            eval_expr(r, env, dstack, loop_marker, ctx)
        }
        "Array.map_take" | "Array.try_map_take" => {
            eval_array_map_take(key, receiver, args, env, dstack, loop_marker, ctx)
        }
        "Untrusted.checked_le" => {
            let Some(recv) = receiver else {
                return Err(
                    ctx.abandon("internal error: `Untrusted.checked_le` is missing its receiver")
                );
            };
            let Some((_, bound_expr)) = args.iter().find(|(l, _)| l == "bound") else {
                return Err(ctx.abandon(
                    "internal error: `Untrusted.checked_le` is missing its `bound` argument",
                ));
            };
            let payload = eval_expr(recv, env, dstack, loop_marker, ctx)?;
            let bound = eval_expr(bound_expr, env, dstack, loop_marker, ctx)?;
            let le = value::eval_compare(BinOp::Le, &payload, &bound);
            let result = if le {
                Value::Enum(value::RESULT_OK, vec![payload])
            } else {
                Value::Enum(value::RESULT_ERR, vec![Value::Unit])
            };
            ctx.charge(result.weight())?;
            Ok(result)
        }
        other => Err(ctx.abandon(format!(
            "internal error: unknown/runtime-only builder intrinsic `{other}` reached the \
             comptime evaluator"
        ))),
    }
}

fn eval_array_map_take<'a, 'p>(
    key: &str,
    receiver: &'a Option<Box<TypedExpr>>,
    args: &'a [(String, TypedExpr)],
    env: &mut Env,
    dstack: &mut Vec<&'a TypedDeferBody>,
    loop_marker: usize,
    ctx: &mut Interp<'p>,
) -> R<Value> {
    let Some(recv) = receiver else {
        return Err(ctx.abandon(format!("internal error: `{key}` missing array receiver")));
    };
    let arr_v = eval_expr(recv, env, dstack, loop_marker, ctx)?;
    let Value::Array(mut inputs) = arr_v else {
        return Err(ctx.abandon(format!(
            "internal error: `{key}` receiver is not an array ({arr_v:?})"
        )));
    };
    let Some((_, mapper_expr)) = args.first() else {
        return Err(ctx.abandon(format!("internal error: `{key}` missing mapper")));
    };
    let mapper_v = eval_expr(mapper_expr, env, dstack, loop_marker, ctx)?;
    let Value::Fn(mapper_key) = mapper_v else {
        return Err(ctx.abandon(format!(
            "internal error: `{key}` mapper is not a fn value ({mapper_v:?})"
        )));
    };
    let f = resolve_fn(ctx.program, &mapper_key).ok_or_else(|| {
        ctx.abandon_missing(
            &callee_decl_name(&mapper_key),
            format!(
                "internal error: `{key}` mapper `{}` not found",
                mapper_key.spelling()
            ),
        )
    })?;
    let is_try = key == "Array.try_map_take";
    let mut outputs = Vec::with_capacity(inputs.len());
    let mut idx = 0usize;
    while idx < inputs.len() {
        let elem = std::mem::replace(&mut inputs[idx], Value::Unit);
        let frame = mapper_key.spelling();
        let outcome = run_call(
            f,
            None,
            frame,
            |callee_env, _ictx| {
                let pname = f
                    .params
                    .first()
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "x".to_string());
                env_insert(callee_env, pname, elem);
                Ok(())
            },
            ctx,
        )?;
        if is_try {
            match outcome.result {
                Value::Enum(value::RESULT_OK, mut payload) => {
                    let v = payload.pop().unwrap_or(Value::Unit);
                    outputs.push(v);
                }
                Value::Enum(value::RESULT_ERR, payload) => {
                    let _drop_outputs = outputs;
                    let _drop_rest: Vec<_> = inputs.drain(idx + 1..).collect();
                    return Ok(Value::Enum(value::RESULT_ERR, payload));
                }
                other => {
                    return Err(ctx.abandon(format!(
                        "internal error: `try_map_take` mapper returned non-Result ({other:?})"
                    )));
                }
            }
        } else {
            outputs.push(outcome.result);
        }
        idx += 1;
    }
    let arr = Value::Array(outputs);
    if is_try {
        Ok(Value::Enum(value::RESULT_OK, vec![arr]))
    } else {
        Ok(arr)
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
            if op == Add {
                if let (Value::Str(a), Value::Str(b)) = (&lv, &rv) {
                    let mut out = a.clone();
                    out.extend_from_slice(b);
                    ctx.charge(out.len() as u64)?;
                    return Ok(Value::Str(out));
                }
            }
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
        AddW | SubW | MulW => value::eval_wrapping(op, &l.ty, &lv, &rv).map_err(|m| ctx.abandon(m)),
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
        BitAnd | BitOr | BitXor => {
            value::eval_bitwise(op, &l.ty, &lv, &rv).map_err(|m| ctx.abandon(m))
        }
        Lt | Le | Gt | Ge => Ok(Value::Bool(value::eval_compare(op, &lv, &rv))),
        Eq => Ok(Value::Bool(lv == rv)),
        Ne => Ok(Value::Bool(lv != rv)),
    }
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

    #[test]
    fn for_take_consume_moves_each_element_correctly() {
        let program = typed_program(
            "module examples.eval_for_take

pub fn sum_and_consume(take arr: [u64; 4]) -> u64:
    total: u64 = 0
    @budget(bound=4)
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
        let v = eval_const(&program, "RESULT").expect("must evaluate cleanly");
        assert_eq!(v, Value::U64(321));
    }

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
        assert_eq!(eval_const(&program, "AREA").unwrap(), Value::U64(16));
        assert_eq!(eval_const(&program, "TRIED").unwrap(), Value::U64(42));
    }

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

    #[test]
    fn untrusted_checked_le_ok_and_err() {
        let program = typed_program(
            "module examples.eval_untrusted_checked_le

fn narrow(reported: Untrusted[usize], bound: usize) -> Result[usize, unit]:
    return reported.checked_le(bound)
",
        );
        let ok = eval_test_case(&program, "narrow", &[Value::Usize(5), Value::Usize(10)])
            .expect("checked_le(5, 10) must succeed");
        assert_eq!(ok, Value::Enum(value::RESULT_OK, vec![Value::Usize(5)]));
        let err = eval_test_case(&program, "narrow", &[Value::Usize(11), Value::Usize(10)])
            .expect("checked_le(11, 10) must return Err, not abandon");
        assert_eq!(err, Value::Enum(value::RESULT_ERR, vec![Value::Unit]));
    }
}
