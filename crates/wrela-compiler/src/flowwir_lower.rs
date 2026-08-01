use std::collections::BTreeMap;

use crate::eval::value;
use crate::flowwir::{
    AwaitKind, FlowInst, FlowWirFn, FlowWirProgram, FrameLayout, State, Transition,
};
use crate::lower_queue::{self, QueueSink};
use crate::lower_shared;
use crate::mwir::{self, Inst, Temp};
use crate::sema::bodies;
use crate::sema::typed::{
    CalleeKey, TypedCallArg, TypedDeferBody, TypedElif, TypedExpr, TypedExprKind, TypedFn,
    TypedForIter, TypedMatchArm, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt,
    TypedStmtKind, TypedStruct,
};
use crate::sema::types::Type;
use crate::syntax::ast::{AccessMode, BinOp};

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

    fn named(message: impl Into<String>) -> FlowError {
        FlowError {
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> FlowError {
        FlowError {
            message: format!("internal error: {}", message.into()),
        }
    }
}

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

    fn emit(&mut self, op: FlowInst) -> usize {
        self.states[self.cur].ops.push(op);
        self.states[self.cur].ops.len() - 1
    }

    fn emit_mwir(&mut self, inst: Inst) -> usize {
        self.emit(FlowInst::Mwir(inst))
    }

    fn emit_at(&mut self, state: usize, op: FlowInst) {
        self.states[state].ops.push(op);
    }

    fn here(&self) -> usize {
        self.states[self.cur].ops.len()
    }

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

    fn finish_if_unset(&mut self, idx: usize, t: Transition) {
        if self.states[idx].transition.is_none() {
            self.states[idx].transition = Some(t);
        }
    }
}

struct FlowQueueSink<'a, 'p>(&'a mut FlowBuilder<'p>);

impl QueueSink for FlowQueueSink<'_, '_> {
    fn fresh(&mut self, ty: Type) -> Temp {
        self.0.fresh(ty)
    }
    fn emit(&mut self, inst: Inst) -> usize {
        self.0.emit_mwir(inst)
    }
    fn here(&mut self) -> usize {
        self.0.here()
    }
    fn patch(&mut self, idx: usize, target: usize) {
        self.0.patch(idx, target)
    }
}

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
                || args
                    .iter()
                    .filter_map(|a| a.value.as_ref())
                    .any(expr_contains_await)
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
        TypedStmtKind::While { cond, body, .. } => {
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

fn struct_by_name<'p>(prog: &'p TypedProgram, name: &str) -> Option<&'p TypedStruct> {
    prog.structs
        .get(name)
        .or_else(|| prog.imported.structs.get(name))
}

fn missing_struct(prog: &TypedProgram, name: &str) -> FlowError {
    if let Some(note) = prog.imported.unresolvable.get(name) {
        return FlowError::named(format!("`{name}` {note}"));
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
        return FlowError::named(format!("`{name}` {note}"));
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

fn runtime_layout_field_offset_flow(
    prog: &TypedProgram,
    layout: &str,
    field: &str,
) -> Result<u64, FlowError> {
    lower_shared::runtime_layout_field_offset(prog, layout, field).map_err(FlowError::internal)
}

fn placed_array_field_index_flow(
    array_place: &TypedExpr,
    prog: &TypedProgram,
) -> Result<Option<(TypedExpr, u64, u64, usize)>, FlowError> {
    lower_shared::placed_array_field_index(array_place, prog, |ty| {
        eval_array_len_with_prog(prog, ty).map_err(|e| e.message.clone())
    })
    .map_err(FlowError::internal)
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
        "CallError" => crate::sema::bodies::call_error_variant_index(variant)
            .ok_or_else(|| FlowError::internal(format!("unknown CallError variant `{variant}`"))),
        _ => {
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

fn literal_array_index_elide(idx_expr: &TypedExpr, len: usize) -> Result<Option<usize>, FlowError> {
    if !crate::lower::bounds_elide() {
        return Ok(None);
    }
    let TypedExprKind::Int(text) = &idx_expr.kind else {
        return Ok(None);
    };
    let raw = value::parse_int_literal(text)
        .ok_or_else(|| FlowError::internal("invalid integer literal text"))?;
    let Ok(i) = usize::try_from(raw) else {
        return Ok(None);
    };
    if i < len { Ok(Some(i)) } else { Ok(None) }
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

fn eval_array_len_with_prog(prog: &TypedProgram, ty: &Type) -> Result<usize, FlowError> {
    match ty {
        Type::Array(_, len_expr) => {
            if let Some(n) = bodies::literal_array_len(len_expr) {
                return usize::try_from(n)
                    .map_err(|_| FlowError::internal("array length out of range"));
            }
            if let crate::syntax::ast::Expr::Name(_, name) = len_expr.as_ref() {
                let v = crate::eval::interp::eval_const(prog, name).map_err(|err| {
                    FlowError::internal(format!(
                        "const `{name}` failed to evaluate during array-length lowering: {}",
                        err.message
                    ))
                })?;
                let n = value::as_i128(&v).ok_or_else(|| {
                    FlowError::internal(format!("array length const `{name}` is not an integer"))
                })?;
                return usize::try_from(n)
                    .map_err(|_| FlowError::internal("array length out of range"));
            }
            Err(FlowError::unimplemented("a non-literal array length is"))
        }
        Type::Own(_, inner) => eval_array_len_with_prog(prog, inner),
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

pub fn lower_program(program: &TypedProgram) -> Result<FlowWirProgram, FlowError> {
    lower_program_with(program, &crate::lower::LowerOpts::default())
}

pub fn lower_program_with(
    program: &TypedProgram,
    opts: &crate::lower::LowerOpts,
) -> Result<FlowWirProgram, FlowError> {
    let computed;
    let reachable: &std::collections::BTreeSet<String> = match &opts.only {
        Some(set) => set,
        None => {
            computed = crate::lower::guest_reachable_keys(program, opts);
            &computed
        }
    };
    let mut fns = BTreeMap::new();
    for (name, f) in &program.fns {
        if f.is_async && reachable.contains(name) {
            fns.insert(name.clone(), lower_fn(f, program)?);
        }
    }
    for (sname, s) in &program.structs {
        for (member, f) in &s.methods {
            let key = format!("{sname}.{member}");
            if f.is_async && reachable.contains(&key) {
                fns.insert(key, lower_fn(f, program)?);
            }
        }
        for (member, f) in &s.assoc_fns {
            let key = format!("{sname}.{member}");
            if f.is_async && reachable.contains(&key) {
                fns.insert(key, lower_fn(f, program)?);
            }
        }
        if let Some(f) = &s.init {
            let key = format!("{sname}.init");
            if f.is_async && reachable.contains(&key) {
                fns.insert(key, lower_fn(f, program)?);
            }
        }
    }
    for (name, f) in &program.imported.fns {
        if f.is_async && !fns.contains_key(name) && reachable.contains(name) {
            fns.insert(name.clone(), lower_fn(f, program)?);
        }
    }
    for (sname, s) in &program.imported.structs {
        for (member, f) in &s.methods {
            let key = format!("{sname}.{member}");
            if f.is_async && !fns.contains_key(&key) && reachable.contains(&key) {
                fns.insert(key, lower_fn(f, program)?);
            }
        }
        for (member, f) in &s.assoc_fns {
            let key = format!("{sname}.{member}");
            if f.is_async && !fns.contains_key(&key) && reachable.contains(&key) {
                fns.insert(key, lower_fn(f, program)?);
            }
        }
        if let Some(f) = &s.init {
            let key = format!("{sname}.init");
            if f.is_async && !fns.contains_key(&key) && reachable.contains(&key) {
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
        TypedStmtKind::While { cond, body, .. } => {
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
            let mut nested_mut_writebacks = Vec::new();
            let arg_temps = lower_aligned_args(f, args, b, env, &mut nested_mut_writebacks)?;
            if !nested_mut_writebacks.is_empty() {
                return Err(FlowError::unimplemented(
                    "passing a nested `mut` place as an awaited actor-call argument is",
                ));
            }
            let take_arg_temps: Vec<_> = f
                .params
                .iter()
                .zip(arg_temps.iter())
                .filter(|(p, _)| p.mode == AccessMode::Take)
                .map(|(_, t)| *t)
                .collect();
            Ok((
                AwaitKind::ActorCall {
                    target_temp,
                    method_key,
                    arg_temps,
                    take_arg_temps,
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

fn lower_aligned_args<'a>(
    f: &TypedFn,
    args: &'a [TypedCallArg],
    b: &mut FlowBuilder,
    env: &mut FEnv,
    nested_mut_writebacks: &mut Vec<(&'a TypedExpr, Temp)>,
) -> Result<Vec<Temp>, FlowError> {
    let mut out = Vec::with_capacity(args.len());
    for (param, slot) in f.params.iter().zip(args.iter()) {
        let t = match &slot.value {
            Some(e) if param.mode == AccessMode::Mut => {
                let (t, wb) = lower_mut_arg_place(e, b, env)?;
                if let Some(place) = wb {
                    nested_mut_writebacks.push((place, t));
                }
                t
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

fn lower_flow_call(
    callee: &CalleeKey,
    receiver: &Option<Box<TypedExpr>>,
    args: &[TypedCallArg],
    _result_ty: &Type,
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<Temp, FlowError> {
    let f = resolve_callee_fn(b.prog, callee)?;
    let key = callee.spelling();
    let mode = f.receiver.as_ref().map(|(m, _)| *m);
    match (receiver, mode) {
        (Some(recv_expr), Some(AccessMode::Mut)) => {
            let (self_temp, recv_wb) = lower_mut_arg_place(recv_expr, b, env)?;
            let mut nested_mut_writebacks = Vec::new();
            if let Some(place) = recv_wb {
                nested_mut_writebacks.push((place, self_temp));
            }
            let arg_temps = lower_aligned_args(f, args, b, env, &mut nested_mut_writebacks)?;
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
            for (place, t) in nested_mut_writebacks {
                lower_place_write(place, t, b, env)?;
            }
            Ok(dst)
        }
        (Some(recv_expr), Some(AccessMode::Read | AccessMode::Take)) => {
            let recv_temp = lower_expr_flat(recv_expr, b, env)?;
            let mut nested_mut_writebacks = Vec::new();
            let arg_temps = lower_aligned_args(f, args, b, env, &mut nested_mut_writebacks)?;
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
            for (place, t) in nested_mut_writebacks {
                lower_place_write(place, t, b, env)?;
            }
            Ok(dst)
        }
        _ => {
            let mut nested_mut_writebacks = Vec::new();
            let arg_temps = lower_aligned_args(f, args, b, env, &mut nested_mut_writebacks)?;
            let write_backs = flow_call_write_backs(f, None, &arg_temps);
            let dst = b.fresh(f.ret.clone());
            b.emit_mwir(Inst::Call {
                dst,
                write_backs,
                key,
                args: arg_temps,
            });
            for (place, t) in nested_mut_writebacks {
                lower_place_write(place, t, b, env)?;
            }
            Ok(dst)
        }
    }
}

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
        let c = b.cur();
        b.finish_if_unset(c, Transition::Return(None));
    }
    defers.truncate(group_marker);
    env.pop();
    Ok(diverged)
}

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
        let c = b.cur();
        b.finish_if_unset(c, Transition::Return(None));
    }
    b.switch_to(after_state);
    Ok(())
}

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

fn materialize_place_mut(
    place: &TypedExpr,
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<(Temp, bool), FlowError> {
    match &place.kind {
        TypedExprKind::Local(name) => match env_lookup(env, name) {
            Some(Binding::Temp(t)) => Ok((t, false)),
            _ => Err(FlowError::internal(format!(
                "unbound (or self-path, not a temp) local `{name}` in place position"
            ))),
        },
        TypedExprKind::Field(..) | TypedExprKind::Index(..) => {
            let t = lower_expr_flat(place, b, env)?;
            Ok((t, true))
        }
        _ => Err(FlowError::internal("expression is not an assignable place")),
    }
}

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
                _ => env_insert(env, name.clone(), Binding::Temp(value)),
            }
            Ok(())
        }
        TypedExprKind::Field(base, fname) => {
            if let TypedExprKind::Static(sname) = &base.kind {
                let layout_name = match bodies::unwrap_own(base.ty.clone()) {
                    Type::Named(n, _) => n,
                    other => {
                        return Err(FlowError::internal(format!(
                            "placed static `{sname}` has non-named type {other:?}"
                        )));
                    }
                };
                let offset = runtime_layout_field_offset_flow(b.prog, &layout_name, fname)?;
                let base_temp = lower_expr_flat(base, b, env)?;
                b.emit_mwir(Inst::MmioWrite {
                    base: base_temp,
                    offset,
                    ty: target.ty.clone(),
                    value,
                });
                return Ok(());
            }
            let (base_temp, needs_writeback) = materialize_place_mut(base, b, env)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            let idx = field_index(b.prog, &base_ty, fname)?;
            b.emit_mwir(Inst::SetField {
                base: base_temp,
                index: idx,
                value,
            });
            if needs_writeback {
                lower_place_write(base, base_temp, b, env)?;
            }
            Ok(())
        }
        TypedExprKind::Index(base, idx_expr) => {
            if let Some((static_expr, field_offset, elem_stride, len)) =
                placed_array_field_index_flow(base, b.prog)?
            {
                let base_temp = lower_expr_flat(&static_expr, b, env)?;
                let idx_temp = lower_expr_flat(idx_expr, b, env)?;
                b.emit_mwir(Inst::PlacedIndexSet {
                    base: base_temp,
                    field_offset,
                    index: idx_temp,
                    value,
                    len,
                    elem_stride,
                    ty: target.ty.clone(),
                });
                return Ok(());
            }
            let (base_temp, needs_writeback) = materialize_place_mut(base, b, env)?;
            let len = eval_array_len(&base.ty)?;
            if let Some(i) = literal_array_index_elide(idx_expr, len)? {
                b.emit_mwir(Inst::SetField {
                    base: base_temp,
                    index: i,
                    value,
                });
            } else {
                let idx_temp = lower_expr_flat(idx_expr, b, env)?;
                b.emit_mwir(Inst::IndexSet {
                    base: base_temp,
                    index: idx_temp,
                    value,
                    len,
                });
            }
            if needs_writeback {
                lower_place_write(base, base_temp, b, env)?;
            }
            Ok(())
        }
        _ => Err(FlowError::unimplemented("assigning to this place is")),
    }
}

fn lower_mut_arg_place<'a>(
    expr: &'a TypedExpr,
    b: &mut FlowBuilder,
    env: &mut FEnv,
) -> Result<(Temp, Option<&'a TypedExpr>), FlowError> {
    match &expr.kind {
        TypedExprKind::Local(name) => match env_lookup(env, name) {
            Some(Binding::Temp(t)) => Ok((t, None)),
            _ => Err(FlowError::internal(format!(
                "unbound (or self-path) local `{name}` as `mut` argument"
            ))),
        },
        TypedExprKind::Field(..) | TypedExprKind::Index(..) => {
            let t = lower_expr_flat(expr, b, env)?;
            Ok((t, Some(expr)))
        }
        _ => Err(FlowError::internal(
            "expression is not an assignable `mut` place",
        )),
    }
}

fn collapse_reserve_permit_if_needed(
    expr_ty: &Type,
    src: Temp,
    b: &mut FlowBuilder<'_>,
) -> Result<Temp, FlowError> {
    if !lower_shared::needs_collapse_reserve_permit(expr_ty, &b.temp_types[src.0]) {
        return Ok(src);
    }
    let dst = b.fresh(expr_ty.clone());
    lower_shared::emit_collapse_reserve_permit(dst, src, |inst| {
        b.emit_mwir(inst);
    });
    Ok(dst)
}

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
        TypedExprKind::Local(name) => {
            let t = match env_lookup(env, name) {
                Some(Binding::Temp(t)) => t,
                Some(Binding::SelfPath(path, ty)) => {
                    let dst = b.fresh(ty);
                    b.emit(FlowInst::SelfPath { dst, path });
                    dst
                }
                None => {
                    return Err(FlowError::internal(format!("unbound local `{name}`")));
                }
            };
            collapse_reserve_permit_if_needed(&e.ty, t, b)
        }
        TypedExprKind::Field(base, name) => {
            if let TypedExprKind::Static(sname) = &base.kind {
                let layout_name = match bodies::unwrap_own(base.ty.clone()) {
                    Type::Named(n, _) => n,
                    other => {
                        return Err(FlowError::internal(format!(
                            "placed static `{sname}` has non-named type {other:?}"
                        )));
                    }
                };
                let offset = runtime_layout_field_offset_flow(b.prog, &layout_name, name)?;
                let base_temp = lower_expr_flat(base, b, env)?;
                let dst = b.fresh(e.ty.clone());
                b.emit_mwir(Inst::MmioRead {
                    dst,
                    base: base_temp,
                    offset,
                    ty: e.ty.clone(),
                });
                return Ok(dst);
            }
            let base_temp = lower_expr_flat(base, b, env)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            if let Type::Named(sname, _) = &base_ty {
                if matches!(sname.as_str(), "Duration" | "Instant") && name == "nanos" {
                    let dst = b.fresh(e.ty.clone());
                    b.emit_mwir(Inst::Copy {
                        dst,
                        src: base_temp,
                    });
                    return Ok(dst);
                }
            }
            let idx = field_index(b.prog, &base_ty, name)?;
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::Project {
                dst,
                base: base_temp,
                index: idx,
            });
            Ok(dst)
        }
        TypedExprKind::Take(inner) => {
            let t = lower_expr_flat(inner, b, env)?;
            collapse_reserve_permit_if_needed(&e.ty, t, b)
        }
        TypedExprKind::Const(name) => {
            let v = crate::eval::interp::eval_const(b.prog, name).map_err(|err| {
                FlowError::internal(format!(
                    "const `{name}` failed to evaluate during flowwir lowering: {}",
                    err.message
                ))
            })?;
            lower_flow_const_value(&v, &e.ty, b)
        }
        TypedExprKind::Static(name) => {
            let addr =
                b.prog.statics.get(name).map(|s| s.addr).ok_or_else(|| {
                    FlowError::internal(format!("placed static `{name}` missing"))
                })?;
            let dst = b.fresh(Type::U64);
            b.emit_mwir(Inst::ConstInt {
                dst,
                ty: Type::U64,
                value: addr as i128,
            });
            Ok(dst)
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
            if matches!(sname.as_str(), "Duration" | "Instant") {
                if fields.len() != 1 {
                    return Err(FlowError::internal(format!(
                        "`{sname}` construction must supply exactly one field"
                    )));
                }
                let nanos = lower_expr_flat(&fields[0].1, b, env)?;
                let dst = b.fresh(e.ty.clone());
                b.emit_mwir(Inst::Copy { dst, src: nanos });
                return Ok(dst);
            }
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
            let mut nested_mut_writebacks = Vec::new();
            let arg_temps = lower_aligned_args(f, args, b, env, &mut nested_mut_writebacks)?;
            if !nested_mut_writebacks.is_empty() {
                return Err(FlowError::unimplemented(
                    "passing a nested `mut` place as a `send` argument is",
                ));
            }
            let take_arg_temps: Vec<_> = f
                .params
                .iter()
                .zip(arg_temps.iter())
                .filter(|(p, _)| p.mode == AccessMode::Take)
                .map(|(_, t)| *t)
                .collect();
            let dst = b.fresh(e.ty.clone());
            b.emit(FlowInst::Send {
                dst,
                target,
                method_key,
                arg_temps,
                take_arg_temps,
            });
            Ok(dst)
        }
        TypedExprKind::Intrinsic { key, .. } if key.as_str() == "now" => {
            let dst = b.fresh(e.ty.clone());
            b.emit(FlowInst::Now { dst });
            Ok(dst)
        }
        TypedExprKind::Intrinsic { key, const_arg, .. } if key.as_str() == "entropy" => {
            let n = const_arg.ok_or_else(|| {
                FlowError::internal("`entropy` Intrinsic missing const_arg (sema bug)")
            })?;
            let dst = b.fresh(e.ty.clone());
            b.emit(FlowInst::Entropy { dst, n });
            Ok(dst)
        }
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
        TypedExprKind::Intrinsic { key, .. }
            if crate::sema::bodies::is_untrusted_narrowing_intrinsic(key) =>
        {
            Err(FlowError::unimplemented(
                "`Untrusted[T].checked_le` inside an `async fn`: the synchronous path emits it \
                 (plans/M7.md item H2a); an async narrowing is",
            ))
        }
        TypedExprKind::Intrinsic {
            key,
            receiver,
            type_arg,
            args,
            ..
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
        TypedExprKind::Index(base, idx_expr) => {
            if let Some((static_expr, field_offset, elem_stride, len)) =
                placed_array_field_index_flow(base, b.prog)?
            {
                let base_temp = lower_expr_flat(&static_expr, b, env)?;
                let idx_temp = lower_expr_flat(idx_expr, b, env)?;
                let dst = b.fresh(e.ty.clone());
                b.emit_mwir(Inst::PlacedIndexGet {
                    dst,
                    base: base_temp,
                    field_offset,
                    index: idx_temp,
                    len,
                    elem_stride,
                    ty: e.ty.clone(),
                });
                return Ok(dst);
            }
            let base_temp = lower_expr_flat(base, b, env)?;
            let base_ty = bodies::unwrap_own(base.ty.clone());
            if matches!(base_ty, Type::Bytes(None)) {
                let idx_temp = lower_expr_flat(idx_expr, b, env)?;
                let dst = b.fresh(e.ty.clone());
                b.emit_mwir(Inst::BytesIndexGet {
                    dst,
                    base: base_temp,
                    index: idx_temp,
                });
                return Ok(dst);
            }
            if let Type::Bytes(Some(n_expr)) = &base_ty {
                let cap = bodies::literal_array_len(n_expr)
                    .ok_or_else(|| FlowError::unimplemented("a non-literal Bytes length is"))?;
                let cap = usize::try_from(cap)
                    .map_err(|_| FlowError::internal("Bytes length out of range"))?;
                let i = match &idx_expr.kind {
                    TypedExprKind::Int(text) => {
                        let raw = value::parse_int_literal(text)
                            .ok_or_else(|| FlowError::internal("invalid integer literal text"))?;
                        usize::try_from(raw)
                            .map_err(|_| FlowError::internal("Bytes index out of range"))?
                    }
                    _ => {
                        return Err(FlowError::unimplemented(
                            "indexing `Bytes[N]` with a non-literal index is",
                        ));
                    }
                };
                if i >= cap {
                    return Err(FlowError::internal(format!(
                        "Bytes index {i} out of length {cap}"
                    )));
                }
                let dst = b.fresh(e.ty.clone());
                b.emit_mwir(Inst::Project {
                    dst,
                    base: base_temp,
                    index: i,
                });
                return Ok(dst);
            }
            let len = eval_array_len(&base.ty)?;
            if let Some(i) = literal_array_index_elide(idx_expr, len)? {
                let dst = b.fresh(e.ty.clone());
                b.emit_mwir(Inst::Project {
                    dst,
                    base: base_temp,
                    index: i,
                });
                return Ok(dst);
            }
            let idx_temp = lower_expr_flat(idx_expr, b, env)?;
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::IndexGet {
                dst,
                base: base_temp,
                index: idx_temp,
                len,
            });
            Ok(dst)
        }
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
            let parts = lower_shared::unpack_prepare_block_args(args).map_err(|err| match err {
                lower_shared::PrepareBlockUnpackError::Missing(label) => {
                    FlowError::internal(format!("`prepare_block` without `{label}`"))
                }
                lower_shared::PrepareBlockUnpackError::NonLiteralDeviceWrites => {
                    FlowError::unimplemented(
                        "`prepare_block`'s `device_writes_payload=` as a non-literal bool is",
                    )
                }
            })?;
            let queue = match receiver {
                Some(q) => lower_expr_flat(q, b, env)?,
                None => {
                    return Err(FlowError::internal(
                        "`prepare_block` without a queue receiver",
                    ));
                }
            };
            let permit_t = lower_expr_flat(parts.permit, b, env)?;
            let header_t = lower_expr_flat(parts.header, b, env)?;
            let payload_t = lower_expr_flat(parts.payload, b, env)?;
            let status_t = lower_expr_flat(parts.status, b, env)?;
            let payload_len = lower_shared::prepare_block_payload_len(&parts.payload.ty, b.prog)
                .map_err(|err| match err {
                    lower_shared::PreparePayloadLenError::NoDmaSize => {
                        FlowError::internal("`prepare_block` payload has no `@layout(dma)` size")
                    }
                    lower_shared::PreparePayloadLenError::BadSectorMultiple(n) => {
                        FlowError::unimplemented(format!(
                            "`prepare_block` with payload layout size {n}: virtio-blk requires \
                             a positive multiple of 512"
                        ))
                    }
                })?;
            let dst = b.fresh(e.ty.clone());
            let _ = permit_t;
            let depth = lower_queue::virtqueue_depth_of(&b.temp_types[queue.0])
                .map_err(FlowError::internal)?;
            lower_queue::expand_prepare(
                dst,
                queue,
                header_t,
                payload_t,
                status_t,
                parts.device_writes,
                payload_len as u32,
                depth,
                &mut FlowQueueSink(b),
            )
            .map_err(FlowError::internal)?;
            Ok(dst)
        }
        "VirtQueue.reserve" => {
            let _ = args
                .iter()
                .find(|(l, _)| l == "descriptors")
                .ok_or_else(|| FlowError::internal("`reserve` without `descriptors=`"))?;
            let _ = receiver;
            let permit = b.fresh(Type::Named("QueuePermit".to_string(), vec![]));
            b.emit_mwir(Inst::ConstInt {
                dst: permit,
                ty: Type::U64,
                value: 0,
            });
            if matches!(&e.ty, Type::Result(_, _)) {
                let dst = b.fresh(e.ty.clone());
                b.emit_mwir(Inst::MakeEnum {
                    dst,
                    tag: value::RESULT_OK,
                    payload: vec![permit],
                });
                Ok(dst)
            } else {
                Ok(permit)
            }
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
            let depth = lower_queue::virtqueue_depth_of(&b.temp_types[queue.0])
                .map_err(FlowError::internal)?;
            lower_queue::expand_publish(dst, queue, operation, depth, &mut FlowQueueSink(b))
                .map_err(FlowError::internal)?;
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
            let depth = lower_queue::virtqueue_depth_of(&b.temp_types[queue.0])
                .map_err(FlowError::internal)?;
            lower_queue::expand_drain(queue, max_val, depth, &mut FlowQueueSink(b))
                .map_err(FlowError::internal)?;
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
            let depth = lower_queue::virtqueue_depth_of(&b.temp_types[queue.0])
                .map_err(FlowError::internal)?;
            lower_queue::expand_suppress(queue, depth, &mut FlowQueueSink(b))
                .map_err(FlowError::internal)?;
            let dst = b.fresh(Type::Unit);
            b.emit_mwir(Inst::ConstUnit { dst });
            Ok(dst)
        }
        other => Err(FlowError::unimplemented(format!(
            "queue operation `{other}` inside an async body is"
        ))),
    }
}

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
    if op == BinOp::Add {
        if let (Type::String(ln), Type::String(rn), Type::String(_)) = (&l.ty, &r.ty, &e.ty) {
            let lhs_cap = crate::sema::bodies::literal_array_len(ln)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| {
                    FlowError::unimplemented(
                        "a `String[..N]` capacity that is not a literal is".to_string(),
                    )
                })?;
            let rhs_cap = crate::sema::bodies::literal_array_len(rn)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| {
                    FlowError::unimplemented(
                        "a `String[..N]` capacity that is not a literal is".to_string(),
                    )
                })?;
            let lv = lower_expr_flat(l, b, env)?;
            let rv = lower_expr_flat(r, b, env)?;
            let dst = b.fresh(e.ty.clone());
            b.emit_mwir(Inst::StringConcat {
                dst,
                lhs: lv,
                rhs: rv,
                lhs_cap,
                rhs_cap,
            });
            return Ok(dst);
        }
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
        @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
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
        @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
        match ra:
            case .Ok(v):
                x = v
            case .Err(_):
                pass
        rb = await self.b.step()
        y: u64 = 0
        @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
        match rb:
            case .Ok(v):
                y = v
            case .Err(_):
                pass
        rc = await self.c.step()
        z: u64 = 0
        @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
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
            @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
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
        @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
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
        @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
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
        @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
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
        @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
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
        @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
        match fetched:
            case .Ok(v):
                after = self.cache.value
                return before + after + v
            case .Err(_):
                return before
",
        )
    }

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

    #[test]
    fn self_rooted_path_survives_await_as_a_path_not_a_temp() {
        let program = self_path_program();
        let flow = lower_program(&program).expect("must lower cleanly");
        let f = &flow.fns["Store.refresh"];
        let resume = &f.states[1];
        let self_paths: Vec<&Vec<String>> = resume
            .ops
            .iter()
            .filter_map(|op| match op {
                FlowInst::SelfPath { path, .. } => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(
            self_paths.len(),
            3,
            "expected `before`/`after` to each re-derive independently at every use, got {self_paths:?}"
        );
        for path in self_paths {
            assert_eq!(path, &vec!["cache".to_string(), "value".to_string()]);
        }
        assert!(
            f.states[0]
                .ops
                .iter()
                .all(|op| !matches!(op, FlowInst::SelfPath { .. }))
        );
    }
}
