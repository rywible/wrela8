use std::collections::{BTreeMap, BTreeSet};

use crate::sema::SemaError;
use crate::sema::bodies::{self, ModuleCtx};
use crate::sema::typed::{
    CalleeKey, TypedCallArg, TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn,
    TypedForIter, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind,
    TypedStruct,
};
use crate::sema::types::{DeclFn, DeclItem, DeclMember, DeclParam, Type};
use crate::syntax::ast::{
    self as ast, AccessMode, ClosureBody, DeferBody, Expr, Member, Module, Span, Stmt, UnaryOp,
};

pub type EffectMap = crate::sema::typed::EffectMap;

pub fn infer_effects(
    module: &Module,
    decl_items: &[DeclItem],
    imported: &crate::sema::types::ImportedTypes,
) -> EffectMap {
    let mctx = bodies::build_module_ctx(module, decl_items, imported);
    infer_private_effects(&mctx)
}

pub(crate) fn infer_effects_over(mctx: &ModuleCtx) -> EffectMap {
    infer_private_effects(mctx)
}

pub(crate) fn check(program: &mut TypedProgram, mctx: &ModuleCtx) -> Result<(), SemaError> {
    let effects = infer_private_effects(mctx);
    program.effects = effects.clone();
    apply_effects_to_program(program, &effects);
    for c in program.consts.values() {
        let mut actx = ACtx::new(mctx, &effects, BTreeSet::new());
        check_typed_expr(&c.value, &mut actx)?;
    }
    for f in program.fns.values_mut() {
        check_typed_fn(f, mctx, &effects)?;
    }
    for s in program.structs.values_mut() {
        check_typed_struct(s, mctx, &effects)?;
    }
    for (ename, e) in program.enums.iter_mut() {
        if let Some(info) = mctx.enums.get(ename) {
            for (mname, f) in e.methods.iter_mut() {
                if let Some((af, fd)) = info.method(mname) {
                    let mode = resolve_receiver_mode(af, fd, ename, mctx, &effects)?;
                    if let Some((m, _)) = f.receiver.as_mut() {
                        *m = mode;
                    }
                }
                check_typed_fn(f, mctx, &effects)?;
            }
            for f in e.assoc_fns.values_mut() {
                check_typed_fn(f, mctx, &effects)?;
            }
        } else {
            for f in e.methods.values_mut().chain(e.assoc_fns.values_mut()) {
                check_typed_fn(f, mctx, &effects)?;
            }
        }
    }
    Ok(())
}

fn apply_effects_to_program(program: &mut TypedProgram, effects: &EffectMap) {
    for s in program.structs.values_mut() {
        apply_effects_to_struct(s, effects);
    }
    for ((owner, method), eff) in effects {
        if let Some(e) = program.enums.get_mut(owner) {
            if let Some(f) = e.methods.get_mut(method) {
                if let Some((mode, _)) = f.receiver.as_mut() {
                    *mode = *eff;
                }
            }
        }
    }
}

fn apply_effects_to_struct(s: &mut TypedStruct, effects: &EffectMap) {
    for (mname, f) in s.methods.iter_mut() {
        if let Some(eff) = effects.get(&(s.name.clone(), mname.clone())) {
            if let Some((mode, _)) = f.receiver.as_mut() {
                *mode = *eff;
            }
        }
    }
}

pub(crate) fn check_typed_fn(
    f: &mut TypedFn,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> Result<(), SemaError> {
    let local_pools = mctx.module_pools.clone();
    let mut actx = ACtx::new(mctx, effects, local_pools);
    if let Some((mode, ty)) = &f.receiver {
        actx.locals.insert(
            "self".to_string(),
            LocalInfo {
                ty: Some(ty.clone()),
                mode: Some(*mode),
            },
        );
    }
    for p in &f.params {
        actx.locals.insert(
            p.name.clone(),
            LocalInfo {
                ty: Some(p.ty.clone()),
                mode: Some(p.mode),
            },
        );
        if let Some(d) = &p.default {
            check_typed_expr(d, &mut actx)?;
        }
    }
    check_typed_stmts(&f.body, &mut actx)
}

pub(crate) fn check_typed_struct(
    s: &mut TypedStruct,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> Result<(), SemaError> {
    apply_effects_to_struct(s, effects);
    for d in s.field_defaults.values() {
        let mut actx = ACtx::new(mctx, effects, BTreeSet::new());
        check_typed_expr(d, &mut actx)?;
    }
    if let Some(info) = mctx.structs.get(&s.name) {
        for (mname, f) in s.methods.iter_mut() {
            if let Some((af, fd)) = info.method(mname) {
                let mode = resolve_receiver_mode(af, fd, &s.name, mctx, effects)?;
                if let Some((m, _)) = f.receiver.as_mut() {
                    *m = mode;
                }
            }
            check_typed_fn(f, mctx, effects)?;
        }
        for f in s.assoc_fns.values_mut() {
            check_typed_fn(f, mctx, effects)?;
        }
    } else {
        for f in s.methods.values_mut().chain(s.assoc_fns.values_mut()) {
            check_typed_fn(f, mctx, effects)?;
        }
    }
    if let Some(f) = s.init.as_mut() {
        check_typed_fn(f, mctx, effects)?;
    }
    Ok(())
}

fn check_typed_stmts(stmts: &[TypedStmt], actx: &mut ACtx<'_>) -> Result<(), SemaError> {
    for s in stmts {
        check_typed_stmt(s, actx)?;
    }
    Ok(())
}

fn check_typed_stmt(stmt: &TypedStmt, actx: &mut ACtx<'_>) -> Result<(), SemaError> {
    match &stmt.kind {
        TypedStmtKind::Let { name, ty, value } => {
            check_typed_expr(value, actx)?;
            actx.locals.insert(
                name.clone(),
                LocalInfo {
                    ty: Some(ty.clone()),
                    mode: None,
                },
            );
            Ok(())
        }
        TypedStmtKind::Assign { target, value } => {
            check_typed_expr(value, actx)?;
            check_assign_target(target, actx)?;
            if let TypedExprKind::Local(name) = &target.kind {
                if !actx.locals.contains_key(name) {
                    actx.locals.insert(
                        name.clone(),
                        LocalInfo {
                            ty: Some(target.ty.clone()),
                            mode: None,
                        },
                    );
                }
            }
            Ok(())
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            check_typed_expr(cond, actx)?;
            check_typed_stmts(then_branch, actx)?;
            for e in elifs {
                check_typed_expr(&e.cond, actx)?;
                check_typed_stmts(&e.body, actx)?;
            }
            if let Some(b) = else_branch {
                check_typed_stmts(b, actx)?;
            }
            Ok(())
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            check_typed_expr(scrutinee, actx)?;
            for arm in arms {
                bind_typed_pattern(&arm.pattern, actx);
                if let Some(g) = &arm.guard {
                    check_typed_expr(g, actx)?;
                }
                check_typed_stmts(&arm.body, actx)?;
            }
            Ok(())
        }
        TypedStmtKind::For {
            name,
            elem_ty,
            take_binding,
            iter,
            body,
            ..
        } => {
            let iterable_is_take = matches!(
                iter,
                TypedForIter::Expr(e) if matches!(e.kind, TypedExprKind::Take(_))
            );
            if *take_binding && !iterable_is_take {
                return Err(access_error(
                    "`for take` requires the iterable itself be `take`n: write `for take x in take array` \
                     — elements cannot be taken one at a time out of an array through a runtime index"
                        .to_string(),
                    stmt.span,
                ));
            }
            if !*take_binding && iterable_is_take {
                return Err(access_error(
                    "a `take`n iterable requires a `take` binding: write `for take x in take array`"
                        .to_string(),
                    stmt.span,
                ));
            }
            match iter {
                TypedForIter::Range(a, b, _) => {
                    check_typed_expr(a, actx)?;
                    check_typed_expr(b, actx)?;
                }
                TypedForIter::Expr(e) => {
                    check_typed_expr(e, actx)?;
                }
            }
            let mode = if !*take_binding && bodies::is_resource_type(elem_ty, actx.mctx) {
                Some(AccessMode::Read)
            } else {
                None
            };
            actx.locals.insert(
                name.clone(),
                LocalInfo {
                    ty: Some(elem_ty.clone()),
                    mode,
                },
            );
            check_typed_stmts(body, actx)
        }
        TypedStmtKind::While { cond, body, .. } => {
            check_typed_expr(cond, actx)?;
            check_typed_stmts(body, actx)
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => Ok(()),
        TypedStmtKind::Return(v) => match v {
            Some(e) => check_typed_expr(e, actx).map(|_| ()),
            None => Ok(()),
        },
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            check_typed_expr(cond, actx)?;
            if let Some(m) = message {
                check_typed_expr(m, actx)?;
            }
            Ok(())
        }
        TypedStmtKind::Defer(body) => match body {
            TypedDeferBody::Expr(e) => check_typed_expr(e, actx).map(|_| ()),
            TypedDeferBody::Suite(stmts) => check_typed_stmts(stmts, actx),
        },
        TypedStmtKind::ExprStmt(e) | TypedStmtKind::BareSend { expr: e, .. } => {
            check_typed_expr(e, actx).map(|_| ())
        }
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            as_name,
            body,
        } => {
            if let Some(c) = capacity {
                check_typed_expr(c, actx)?;
            }
            if let Some(d) = deadline {
                check_typed_expr(d, actx)?;
            }
            if let Some(name) = as_name {
                actx.locals.insert(
                    name.clone(),
                    LocalInfo {
                        ty: Some(Type::Named("Group".to_string(), vec![])),
                        mode: None,
                    },
                );
            }
            check_typed_stmts(body, actx)
        }
    }
}

fn bind_typed_pattern(p: &TypedPattern, actx: &mut ACtx<'_>) {
    match &p.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Literal(_) => {}
        TypedPatternKind::Binding(name) => {
            actx.locals.insert(
                name.clone(),
                LocalInfo {
                    ty: Some(p.ty.clone()),
                    mode: None,
                },
            );
        }
        TypedPatternKind::Take(inner) => bind_typed_pattern(inner, actx),
        TypedPatternKind::Variant { payload, .. } => {
            for sp in payload {
                bind_typed_pattern(sp, actx);
            }
        }
        TypedPatternKind::Tuple(items)
        | TypedPatternKind::Array(items)
        | TypedPatternKind::Or(items) => {
            for sp in items {
                bind_typed_pattern(sp, actx);
            }
        }
    }
}

fn check_assign_target(target: &TypedExpr, actx: &mut ACtx<'_>) -> Result<(), SemaError> {
    if let Some(root) = typed_place_root(target) {
        if actx.locals.get(root).and_then(|li| li.mode) == Some(AccessMode::Read) {
            return Err(access_error(
                format!("`{root}` is a `read` parameter; it cannot be assigned"),
                target.span,
            ));
        }
    }
    check_typed_expr(target, actx).map(|_| ())
}

fn typed_place_root(e: &TypedExpr) -> Option<&str> {
    match &e.kind {
        TypedExprKind::Local(n) => Some(n.as_str()),
        TypedExprKind::Field(base, _) | TypedExprKind::Index(base, _) => typed_place_root(base),
        _ => None,
    }
}

fn is_typed_full_place(e: &TypedExpr) -> bool {
    match &e.kind {
        TypedExprKind::Local(_) => true,
        TypedExprKind::Field(base, _) | TypedExprKind::Index(base, _) => is_typed_full_place(base),
        _ => false,
    }
}

fn check_typed_expr(expr: &TypedExpr, actx: &mut ACtx<'_>) -> Result<Type, SemaError> {
    match &expr.kind {
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            if let Some(r) = receiver {
                check_typed_expr(r, actx)?;
                check_typed_receiver_mutability(r, callee, actx)?;
            }
            check_typed_call_args(callee, args, actx)?;
            Ok(expr.ty.clone())
        }
        TypedExprKind::CallValue(callee, args) => {
            check_typed_expr(callee, actx)?;
            if let Type::Fn(params, _) = &callee.ty {
                for (a, (expected, _)) in args.iter().zip(params.iter()) {
                    if a.value.is_some() && a.mode != *expected {
                        let span = a.value.as_ref().map(|v| v.span).unwrap_or(expr.span);
                        return Err(access_error(
                            mirror_message_positional(*expected, a.mode),
                            span,
                        ));
                    }
                }
            }
            for a in args {
                check_typed_arg_mode(a, actx)?;
                if let Some(v) = &a.value {
                    check_typed_expr(v, actx)?;
                }
            }
            Ok(expr.ty.clone())
        }
        TypedExprKind::Take(inner) => {
            if !is_typed_full_place(inner) {
                return Err(access_error(
                    "operand of `take` must be a place expression (name, field, index)".to_string(),
                    expr.span,
                ));
            }
            if let Some(root) = typed_place_root(inner) {
                if actx.locals.get(root).and_then(|li| li.mode) == Some(AccessMode::Read) {
                    return Err(access_error(
                        format!("`{root}` is a `read` parameter; it cannot be taken"),
                        expr.span,
                    ));
                }
            }
            check_typed_expr(inner, actx)
        }
        TypedExprKind::Field(base, _) => {
            check_typed_expr(base, actx)?;
            Ok(expr.ty.clone())
        }
        TypedExprKind::Index(base, idx) => {
            check_typed_expr(base, actx)?;
            check_typed_expr(idx, actx)?;
            Ok(expr.ty.clone())
        }
        TypedExprKind::Is(scrut, pat) => {
            check_typed_expr(scrut, actx)?;
            bind_typed_pattern(pat, actx);
            Ok(expr.ty.clone())
        }
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::OpCall(_, l, r)
        | TypedExprKind::And(l, r)
        | TypedExprKind::Or(l, r) => {
            check_typed_expr(l, actx)?;
            check_typed_expr(r, actx)?;
            Ok(expr.ty.clone())
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                if a.mode == AccessMode::Mut {
                    let span = a.value.as_ref().map(|v| v.span).unwrap_or(expr.span);
                    return Err(access_error(
                        "`mut` is not a legal marker here; a payload value is either unmarked or `take`"
                            .to_string(),
                        span,
                    ));
                }
                check_typed_arg_mode(a, actx)?;
                if let Some(v) = &a.value {
                    check_typed_expr(v, actx)?;
                }
            }
            Ok(expr.ty.clone())
        }
        TypedExprKind::Closure { params, body } => {
            for p in params {
                actx.locals.insert(
                    p.name.clone(),
                    LocalInfo {
                        ty: Some(p.ty.clone()),
                        mode: Some(p.mode),
                    },
                );
            }
            match body {
                TypedClosureBody::Expr(e) => {
                    check_typed_expr(e, actx)?;
                }
                TypedClosureBody::Suite(stmts) => check_typed_stmts(stmts, actx)?,
            }
            Ok(expr.ty.clone())
        }
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                check_typed_expr(i, actx)?;
            }
            Ok(expr.ty.clone())
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                check_typed_expr(v, actx)?;
            }
            Ok(expr.ty.clone())
        }
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                check_typed_expr(r, actx)?;
            }
            for (_, a) in args {
                check_typed_expr(a, actx)?;
            }
            Ok(expr.ty.clone())
        }
        TypedExprKind::ToScalar(inner)
        | TypedExprKind::Neg(inner)
        | TypedExprKind::BitNot(inner)
        | TypedExprKind::Not(inner)
        | TypedExprKind::Panic(inner)
        | TypedExprKind::Await(inner)
        | TypedExprKind::Send(inner)
        | TypedExprKind::Try(inner, _) => {
            check_typed_expr(inner, actx)?;
            Ok(expr.ty.clone())
        }
        _ => Ok(expr.ty.clone()),
    }
}

fn check_typed_arg_mode(a: &TypedCallArg, actx: &mut ACtx<'_>) -> Result<(), SemaError> {
    let Some(v) = &a.value else {
        return Ok(());
    };
    if a.mode == AccessMode::Read {
        return Ok(());
    }
    if !is_typed_full_place(v) {
        return Err(access_error(
            format!(
                "operand of `{}` must be a place expression (name, field, index)",
                a.mode.as_str()
            ),
            v.span,
        ));
    }
    if let Some(root) = typed_place_root(v) {
        if actx.locals.get(root).and_then(|li| li.mode) == Some(AccessMode::Read)
            && a.mode != AccessMode::Read
        {
            return Err(access_error(
                format!(
                    "`{root}` is a `read` parameter; it cannot be passed as `{}`",
                    a.mode.as_str()
                ),
                v.span,
            ));
        }
    }
    Ok(())
}

fn mirror_message(name: &str, expected: AccessMode, found: AccessMode) -> String {
    match (expected, found) {
        (AccessMode::Read, _) => format!(
            "parameter `{name}` takes an unmarked argument, found `{}`",
            found.as_str()
        ),
        (_, AccessMode::Read) => format!(
            "parameter `{name}` requires `{}`, found an unmarked argument",
            expected.as_str()
        ),
        _ => format!(
            "parameter `{name}` requires `{}`, found `{}`",
            expected.as_str(),
            found.as_str()
        ),
    }
}

fn mirror_message_positional(expected: AccessMode, found: AccessMode) -> String {
    match (expected, found) {
        (AccessMode::Read, _) => {
            format!("this argument takes no marker, found `{}`", found.as_str())
        }
        (_, AccessMode::Read) => format!(
            "this argument requires `{}`, found an unmarked argument",
            expected.as_str()
        ),
        _ => format!(
            "this argument requires `{}`, found `{}`",
            expected.as_str(),
            found.as_str()
        ),
    }
}

fn method_owner_base(key: &str) -> &str {
    let rest = key
        .strip_prefix("struct:")
        .or_else(|| key.strip_prefix("enum:"))
        .unwrap_or(key);
    rest.split('[').next().unwrap_or(rest)
}

fn decl_params_for_callee<'a>(callee: &CalleeKey, actx: &'a ACtx<'_>) -> Option<&'a [DeclParam]> {
    match callee {
        CalleeKey::Fn(name) => actx.mctx.fns.get(name).map(|f| f.decl.params.as_slice()),
        CalleeKey::FnInstance(key) => {
            let bare = key
                .strip_prefix("fn:")
                .unwrap_or(key.as_str())
                .split('[')
                .next()
                .unwrap_or(key.as_str());
            actx.mctx.fns.get(bare).map(|f| f.decl.params.as_slice())
        }
        CalleeKey::Method(owner, method) | CalleeKey::MethodInstance(owner, method) => {
            let base = match callee {
                CalleeKey::MethodInstance(k, _) => method_owner_base(k),
                _ => owner.as_str(),
            };
            if let Some(s) = actx.mctx.structs.get(base) {
                if let Some((_, d)) = s.method(method).or_else(|| s.assoc_fn(method)) {
                    return Some(d.params.as_slice());
                }
                if method == "init" {
                    if let Some((_, d)) = s.init() {
                        return Some(d.params.as_slice());
                    }
                }
            }
            if let Some(e) = actx.mctx.enums.get(base) {
                if let Some((_, d)) = e.method(method).or_else(|| e.assoc_fn(method)) {
                    return Some(d.params.as_slice());
                }
            }
            None
        }
    }
}

fn check_mirroring_typed(
    decl_params: &[DeclParam],
    args: &[TypedCallArg],
) -> Result<(), SemaError> {
    for (p, a) in decl_params.iter().zip(args.iter()) {
        if a.value.is_none() {
            continue;
        }
        if a.mode != p.mode {
            let span = a.value.as_ref().map(|v| v.span).unwrap_or_default();
            return Err(access_error(mirror_message(&p.name, p.mode, a.mode), span));
        }
    }
    Ok(())
}

fn check_typed_call_args(
    callee: &CalleeKey,
    args: &[TypedCallArg],
    actx: &mut ACtx<'_>,
) -> Result<(), SemaError> {
    if let Some(params) = decl_params_for_callee(callee, actx) {
        check_mirroring_typed(params, args)?;
    }
    for a in args {
        check_typed_arg_mode(a, actx)?;
        if let Some(v) = &a.value {
            check_typed_expr(v, actx)?;
        }
    }
    Ok(())
}

fn allows_mut_receiver(mode: Option<AccessMode>) -> bool {
    !matches!(mode, Some(AccessMode::Read))
}

fn allows_take_receiver(mode: Option<AccessMode>) -> bool {
    !matches!(mode, Some(AccessMode::Read) | Some(AccessMode::Mut))
}

fn receiver_mutability_error(
    required: AccessMode,
    root_mode: Option<AccessMode>,
    root_name: Option<&str>,
    span: Span,
) -> SemaError {
    let noun = match required {
        AccessMode::Mut => "a mutable place",
        AccessMode::Take => "an owned place",
        AccessMode::Read => "any place",
    };
    let found = match (root_mode, root_name) {
        (Some(mode), Some(n)) => format!("`{}` parameter `{n}`", mode.as_str()),
        _ => "this expression".to_string(),
    };
    access_error(
        format!(
            "calling a `{} self` method requires {noun}, found {found}",
            required.as_str()
        ),
        span,
    )
}

fn check_typed_receiver_mutability(
    receiver: &TypedExpr,
    callee: &CalleeKey,
    actx: &mut ACtx<'_>,
) -> Result<(), SemaError> {
    if let Type::Named(n, _) = bodies::unwrap_own(receiver.ty.clone()) {
        if n == "Actor" || n == "Group" {
            return Ok(());
        }
    }
    let (owner_key, method) = match callee {
        CalleeKey::Method(o, m) => (o.as_str(), m.as_str()),
        CalleeKey::MethodInstance(o, m) => (method_owner_base(o), m.as_str()),
        _ => return Ok(()),
    };
    let mode = if let Some(s) = actx.mctx.structs.get(owner_key) {
        if let Some((af, d)) = s.method(method) {
            if d.receiver.is_some() {
                Some(resolve_receiver_mode(
                    af,
                    d,
                    owner_key,
                    actx.mctx,
                    actx.effects,
                )?)
            } else {
                None
            }
        } else {
            None
        }
    } else if let Some(e) = actx.mctx.enums.get(owner_key) {
        if let Some((af, d)) = e.method(method) {
            if d.receiver.is_some() {
                Some(resolve_receiver_mode(
                    af,
                    d,
                    owner_key,
                    actx.mctx,
                    actx.effects,
                )?)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    let Some(mode) = mode else {
        return Ok(());
    };
    if mode == AccessMode::Read {
        return Ok(());
    }
    if !is_typed_full_place(receiver) {
        return Err(receiver_mutability_error(mode, None, None, receiver.span));
    }
    let root = typed_place_root(receiver);
    let root_mode = root.and_then(|r| actx.locals.get(r).and_then(|li| li.mode));
    let ok = match mode {
        AccessMode::Mut => allows_mut_receiver(root_mode),
        AccessMode::Take => allows_take_receiver(root_mode),
        AccessMode::Read => true,
    };
    if !ok {
        return Err(receiver_mutability_error(
            mode,
            root_mode,
            root,
            receiver.span,
        ));
    }
    Ok(())
}

fn rank(m: AccessMode) -> u8 {
    match m {
        AccessMode::Read => 0,
        AccessMode::Mut => 1,
        AccessMode::Take => 2,
    }
}

fn escalate(acc: &mut AccessMode, m: AccessMode) {
    if rank(m) > rank(*acc) {
        *acc = m;
    }
}

fn private_candidates(mctx: &ModuleCtx) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (sname, s) in &mctx.structs {
        for (am, dm) in s.members() {
            if let (Member::Fn(f), DeclMember::Fn(d)) = (am, dm) {
                if f.generics.is_empty() {
                    if let Some(r) = &d.receiver {
                        if r.mode.is_none() && !r.is_pub {
                            out.push((sname.clone(), f.name.clone()));
                        }
                    }
                }
            }
        }
    }
    for (ename, e) in &mctx.enums {
        for (am, dm) in e.members() {
            if let (Member::Fn(f), DeclMember::Fn(d)) = (am, dm) {
                if f.generics.is_empty() {
                    if let Some(r) = &d.receiver {
                        if r.mode.is_none() && !r.is_pub {
                            out.push((ename.clone(), f.name.clone()));
                        }
                    }
                }
            }
        }
    }
    out
}

fn infer_private_effects(mctx: &ModuleCtx) -> EffectMap {
    let candidates = private_candidates(mctx);
    let mut effects: EffectMap = candidates
        .iter()
        .cloned()
        .map(|k| (k, AccessMode::Read))
        .collect();
    loop {
        let mut changed = false;
        for key in &candidates {
            let (tname, mname) = key;
            let (f, _d) = if let Some(s) = mctx.structs.get(tname) {
                s.method(mname)
                    .expect("private_candidates only names real methods")
            } else {
                mctx.enums[tname]
                    .method(mname)
                    .expect("private_candidates only names real methods")
            };
            let Some(body) = &f.body else {
                continue;
            };
            let required = required_self_effect(body, tname, mctx, &effects);
            if rank(required) > rank(effects[key]) {
                effects.insert(key.clone(), required);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    effects
}

fn effective_declared(
    mode: Option<AccessMode>,
    is_pub: bool,
    sname: &str,
    mname: &str,
    effects: &EffectMap,
) -> AccessMode {
    match mode {
        Some(AccessMode::Mut) => AccessMode::Mut,
        Some(AccessMode::Take) => AccessMode::Take,
        Some(AccessMode::Read) => AccessMode::Read,
        None => {
            if is_pub {
                AccessMode::Read
            } else {
                effects
                    .get(&(sname.to_string(), mname.to_string()))
                    .copied()
                    .unwrap_or(AccessMode::Read)
            }
        }
    }
}

fn required_self_effect(
    body: &[Stmt],
    sname: &str,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> AccessMode {
    let mut acc = AccessMode::Read;
    scan_stmts_self(body, sname, mctx, effects, &mut acc);
    acc
}

enum SelfRef {
    None,
    Whole,
    Field,
}

fn self_ref(e: &Expr) -> SelfRef {
    match e {
        Expr::Name(_, n) if n == "self" => SelfRef::Whole,
        Expr::Field(..) | Expr::Index(..) => {
            if place_root_name(e) == Some("self") {
                SelfRef::Field
            } else {
                SelfRef::None
            }
        }
        _ => SelfRef::None,
    }
}

fn scan_stmts_self(
    stmts: &[Stmt],
    sname: &str,
    mctx: &ModuleCtx,
    effects: &EffectMap,
    acc: &mut AccessMode,
) {
    for s in stmts {
        scan_stmt_self(s, sname, mctx, effects, acc);
    }
}

fn scan_stmt_self(
    stmt: &Stmt,
    sname: &str,
    mctx: &ModuleCtx,
    effects: &EffectMap,
    acc: &mut AccessMode,
) {
    match stmt {
        Stmt::Assign(a) => {
            match self_ref(&a.target) {
                SelfRef::Whole | SelfRef::Field => escalate(acc, AccessMode::Mut),
                SelfRef::None => {}
            }
            scan_expr_self(&a.target, sname, mctx, effects, acc);
            scan_expr_self(&a.value, sname, mctx, effects, acc);
        }
        Stmt::If(i) => {
            scan_expr_self(&i.cond, sname, mctx, effects, acc);
            scan_stmts_self(&i.then_branch, sname, mctx, effects, acc);
            for elif in &i.elifs {
                scan_expr_self(&elif.cond, sname, mctx, effects, acc);
                scan_stmts_self(&elif.body, sname, mctx, effects, acc);
            }
            if let Some(b) = &i.else_branch {
                scan_stmts_self(b, sname, mctx, effects, acc);
            }
        }
        Stmt::Match(m) => {
            scan_expr_self(&m.scrutinee, sname, mctx, effects, acc);
            for arm in &m.arms {
                if let Some(g) = &arm.guard {
                    scan_expr_self(g, sname, mctx, effects, acc);
                }
                scan_stmts_self(&arm.body, sname, mctx, effects, acc);
            }
        }
        Stmt::For(f) => {
            scan_expr_self(&f.iterable, sname, mctx, effects, acc);
            scan_stmts_self(&f.body, sname, mctx, effects, acc);
        }
        Stmt::While(w) => {
            scan_expr_self(&w.cond, sname, mctx, effects, acc);
            scan_stmts_self(&w.body, sname, mctx, effects, acc);
        }
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) | Stmt::Dmb(_) => {}
        Stmt::Return(_, e) => {
            if let Some(e) = e {
                scan_expr_self(e, sname, mctx, effects, acc);
            }
        }
        Stmt::Assert(a) => {
            scan_expr_self(&a.cond, sname, mctx, effects, acc);
            if let Some(m) = &a.message {
                scan_expr_self(m, sname, mctx, effects, acc);
            }
        }
        Stmt::Defer(d) => match &d.body {
            DeferBody::Expr(e) => scan_expr_self(e, sname, mctx, effects, acc),
            DeferBody::Suite(s) => scan_stmts_self(s, sname, mctx, effects, acc),
        },
        Stmt::With(w) => {
            scan_expr_self(&w.expr, sname, mctx, effects, acc);
            scan_stmts_self(&w.body, sname, mctx, effects, acc);
        }
        Stmt::Send(_, e) => scan_expr_self(e, sname, mctx, effects, acc),
        Stmt::Expr(_, e) => scan_expr_self(e, sname, mctx, effects, acc),
        Stmt::ComptimeIf(c) => {
            scan_expr_self(&c.cond, sname, mctx, effects, acc);
            scan_stmts_self(&c.then_branch, sname, mctx, effects, acc);
            if let Some(b) = &c.else_branch {
                scan_stmts_self(b, sname, mctx, effects, acc);
            }
        }
        Stmt::ComptimeAssert(_, e, m) => {
            scan_expr_self(e, sname, mctx, effects, acc);
            if let Some(m) = m {
                scan_expr_self(m, sname, mctx, effects, acc);
            }
        }
    }
}

fn scan_expr_self(
    e: &Expr,
    sname: &str,
    mctx: &ModuleCtx,
    effects: &EffectMap,
    acc: &mut AccessMode,
) {
    match e {
        Expr::Unary(_, UnaryOp::Take, inner) => {
            match self_ref(inner) {
                SelfRef::Whole => escalate(acc, AccessMode::Take),
                SelfRef::Field => escalate(acc, AccessMode::Mut),
                SelfRef::None => {}
            }
            scan_expr_self(inner, sname, mctx, effects, acc);
        }
        Expr::Unary(_, _, inner) => scan_expr_self(inner, sname, mctx, effects, acc),
        Expr::Field(base, _, _) => scan_expr_self(base, sname, mctx, effects, acc),
        Expr::Index(base, _, args) => {
            scan_expr_self(base, sname, mctx, effects, acc);
            for a in args {
                scan_expr_self(a, sname, mctx, effects, acc);
            }
        }
        Expr::Call(callee, _, args) => {
            for a in args {
                if a.mode != AccessMode::Read {
                    match (a.mode, self_ref(&a.value)) {
                        (AccessMode::Take, SelfRef::Whole) => escalate(acc, AccessMode::Take),
                        (AccessMode::Take, SelfRef::Field) => escalate(acc, AccessMode::Mut),
                        (AccessMode::Mut, SelfRef::Whole) | (AccessMode::Mut, SelfRef::Field) => {
                            escalate(acc, AccessMode::Mut)
                        }
                        _ => {}
                    }
                }
                scan_expr_self(&a.value, sname, mctx, effects, acc);
            }
            if let Expr::Field(base, _, name) = &**callee {
                if matches!(self_ref(base), SelfRef::Whole) {
                    if let Some(s) = mctx.structs.get(sname) {
                        if let Some((_, d)) = s.method(name) {
                            if let Some(r) = &d.receiver {
                                escalate(
                                    acc,
                                    effective_declared(r.mode, r.is_pub, sname, name, effects),
                                );
                            }
                        }
                    } else if let Some(e) = mctx.enums.get(sname) {
                        if let Some((_, d)) = e.method(name) {
                            if let Some(r) = &d.receiver {
                                escalate(
                                    acc,
                                    effective_declared(r.mode, r.is_pub, sname, name, effects),
                                );
                            }
                        }
                    }
                }
            }
            scan_expr_self(callee, sname, mctx, effects, acc);
        }
        Expr::Try(_, inner) => scan_expr_self(inner, sname, mctx, effects, acc),
        Expr::Binary(_, _, l, r) | Expr::Range(_, l, r, _) => {
            scan_expr_self(l, sname, mctx, effects, acc);
            scan_expr_self(r, sname, mctx, effects, acc);
        }
        Expr::Is(_, scrutinee, _) => scan_expr_self(scrutinee, sname, mctx, effects, acc),
        Expr::Not(_, inner) => scan_expr_self(inner, sname, mctx, effects, acc),
        Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            scan_expr_self(l, sname, mctx, effects, acc);
            scan_expr_self(r, sname, mctx, effects, acc);
        }
        Expr::DotVariant(_, _, args) => {
            for a in args {
                scan_expr_self(&a.value, sname, mctx, effects, acc);
            }
        }
        Expr::Closure(c) => match &c.body {
            ClosureBody::Expr(e) => scan_expr_self(e, sname, mctx, effects, acc),
            ClosureBody::Suite(s) => scan_stmts_self(s, sname, mctx, effects, acc),
        },
        Expr::Send(_, inner) => scan_expr_self(inner, sname, mctx, effects, acc),
        Expr::Tuple(_, items) | Expr::List(_, items) => {
            for i in items {
                scan_expr_self(i, sname, mctx, effects, acc);
            }
        }
        Expr::ArrayRepeat(_, elem, count) => {
            scan_expr_self(elem, sname, mctx, effects, acc);
            scan_expr_self(count, sname, mctx, effects, acc);
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::BStr(..)
        | Expr::Char(..)
        | Expr::Bool(..)
        | Expr::Unit(_)
        | Expr::Name(..) => {}
        Expr::FStr(f) => {
            if let Ok(desugared) = crate::sema::fstring::desugar_fstring(f) {
                scan_expr_self(&desugared, sname, mctx, effects, acc);
            }
        }
    }
}

fn place_root_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Name(_, n) => Some(n.as_str()),
        Expr::Field(base, _, _) => place_root_name(base),
        Expr::Index(base, _, _) => place_root_name(base),
        _ => None,
    }
}

fn access_error(message: String, span: Span) -> SemaError {
    SemaError::at("access", message, span)
}

#[derive(Clone)]
struct LocalInfo {
    #[allow(dead_code)]
    ty: Option<Type>,
    mode: Option<AccessMode>,
}

struct ACtx<'a> {
    mctx: &'a ModuleCtx,
    effects: &'a EffectMap,
    locals: BTreeMap<String, LocalInfo>,
    #[allow(dead_code)]
    local_pools: BTreeSet<String>,
}

impl<'a> ACtx<'a> {
    fn new(mctx: &'a ModuleCtx, effects: &'a EffectMap, local_pools: BTreeSet<String>) -> Self {
        ACtx {
            mctx,
            effects,
            locals: BTreeMap::new(),
            local_pools,
        }
    }
}

pub(crate) fn resolve_receiver_mode(
    f: &ast::FnItem,
    fd: &DeclFn,
    sname: &str,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> Result<AccessMode, SemaError> {
    let Some(d) = &fd.receiver else {
        return Ok(AccessMode::Read);
    };
    match d.mode {
        Some(AccessMode::Mut) => Ok(AccessMode::Mut),
        Some(AccessMode::Take) => Ok(AccessMode::Take),
        Some(AccessMode::Read) => Ok(AccessMode::Read),
        None => {
            if d.is_pub {
                let Some(body) = &f.body else {
                    return Ok(AccessMode::Read);
                };
                let required = required_self_effect(body, sname, mctx, effects);
                if required != AccessMode::Read {
                    let span = f
                        .receiver
                        .as_ref()
                        .expect("DeclFn.receiver implies an ast receiver")
                        .span;
                    return Err(access_error(
                        format!(
                            "pub method `{}` needs an explicit `{} self` receiver",
                            f.name,
                            required.as_str()
                        ),
                        span,
                    ));
                }
                Ok(AccessMode::Read)
            } else {
                Ok(effects
                    .get(&(sname.to_string(), f.name.clone()))
                    .copied()
                    .unwrap_or(AccessMode::Read))
            }
        }
    }
}
