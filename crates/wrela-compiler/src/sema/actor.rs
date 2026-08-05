use std::collections::BTreeSet;

use crate::sema::bodies::{
    FnCtx, InstKind, ModuleCtx, check_call_args, check_expr, check_stmts, is_resource_type,
    missing_method_error, scoped, type_error, types_eq,
};
use crate::sema::generics;
use crate::sema::typed::{
    CalleeKey, TypedCallArg, TypedExpr, TypedExprKind, TypedForIter, TypedPattern,
    TypedPatternKind, TypedStmt, TypedStmtKind,
};
use crate::sema::types::{self, DeclParam, Type, TypeArg};
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{
    self, AccessMode, Arg, BinOp, ClosureBody, DeferBody, Expr, Span, Stmt, WithStmt,
};

pub(crate) fn actor_error(message: String, span: Span) -> SemaError {
    SemaError::at("actor", message, span)
}

pub(crate) fn compose_call_error(raw: &Type, take_arg_tys: &[Type]) -> Type {
    let e_ty = match raw {
        Type::Result(_, e) => (**e).clone(),
        _ => Type::Never,
    };
    let ok_ty = match raw {
        Type::Result(t, _) => (**t).clone(),
        other => other.clone(),
    };
    Type::Result(
        Box::new(ok_ty),
        Box::new(call_error_type(e_ty, take_arg_tys)),
    )
}

pub(crate) fn call_error_type(e_ty: Type, take_arg_tys: &[Type]) -> Type {
    let mut targs = vec![TypeArg::Type(e_ty)];
    if !take_arg_tys.is_empty() {
        targs.push(TypeArg::Type(Type::Tuple(take_arg_tys.to_vec())));
    }
    Type::Named("CallError".to_string(), targs)
}

pub(crate) fn not_admitted_args_type(targs: &[TypeArg]) -> Type {
    match targs.get(1) {
        Some(TypeArg::Type(t)) => t.clone(),
        _ => Type::Tuple(vec![]),
    }
}

pub(crate) fn call_error_e_compatible(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Named(n1, a1), Type::Named(n2, a2)) if n1 == "CallError" && n2 == "CallError" => {
            match (a1.first(), a2.first()) {
                (Some(TypeArg::Type(e1)), Some(TypeArg::Type(e2))) => types_eq(e1, e2),
                _ => false,
            }
        }
        _ => false,
    }
}

pub(crate) fn decompose_call_error(composed: &Type) -> Option<Type> {
    let Type::Result(t, e) = composed else {
        return None;
    };
    let Type::Named(name, targs) = &**e else {
        return None;
    };
    if name != "CallError" {
        return None;
    }
    let Some(TypeArg::Type(inner)) = targs.first() else {
        return None;
    };
    if matches!(inner, Type::Never) {
        Some((**t).clone())
    } else {
        Some(Type::Result(t.clone(), Box::new(inner.clone())))
    }
}

pub(crate) fn call_error_variant_index(variant: &str) -> Option<usize> {
    match variant {
        "Op" => Some(0),
        "Cancelled" => Some(1),
        "DeadlineExceeded" => Some(2),
        "NotAdmitted" => Some(3),
        _ => None,
    }
}

pub(crate) fn check_message_args(
    ast_params: &[ast::Param],
    decl_params: &[DeclParam],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedCallArg>, SemaError> {
    let mut bound = vec![false; decl_params.len()];
    let mut slots: Vec<TypedCallArg> = (0..decl_params.len())
        .map(|_| TypedCallArg {
            mode: AccessMode::Read,
            value: None,
        })
        .collect();
    let mut cursor = 0usize;
    for a in args {
        if a.mode == AccessMode::Mut {
            return Err(actor_error(
                "a message argument cannot be a `mut` loan (02-language.md §9.3)".to_string(),
                a.span,
            ));
        }
        let idx = match &a.label {
            Some(lbl) => {
                let Some(i) = decl_params.iter().position(|p| &p.name == lbl) else {
                    return Err(type_error(
                        format!("unknown parameter label `{lbl}`"),
                        a.span,
                    ));
                };
                i
            }
            None => {
                while cursor < bound.len() && bound[cursor] {
                    cursor += 1;
                }
                if cursor >= decl_params.len() {
                    return Err(type_error("too many arguments".to_string(), a.span));
                }
                let i = cursor;
                cursor += 1;
                i
            }
        };
        if bound[idx] {
            return Err(type_error(
                format!("argument `{}` bound more than once", decl_params[idx].name),
                a.span,
            ));
        }
        bound[idx] = true;
        let pty = decl_params[idx].ty.clone();
        let vt = check_expr(&a.value, Some(&pty), fctx, mctx)?;
        if matches!(vt.ty, Type::Fn(..)) {
            return Err(actor_error(
                format!(
                    "a message argument cannot be a closure (`{}`, 02-language.md §9.3)",
                    decl_params[idx].name
                ),
                a.span,
            ));
        }
        if matches!(&vt.ty, Type::Named(n, _) if n == "Actor") {
            return Err(actor_error(
                format!(
                    "an `Actor[T]` handle cannot appear in a message (`{}`, 02-language.md §9.1)",
                    decl_params[idx].name
                ),
                a.span,
            ));
        }
        if is_resource_type(&vt.ty, mctx) {
            if a.mode != AccessMode::Take {
                return Err(actor_error(
                    format!(
                        "a message argument that is a resource must be moved with `take` \
                         (`{}`, 02-language.md §9.3)",
                        decl_params[idx].name
                    ),
                    a.span,
                ));
            }
            if !matches!(&vt.ty, Type::Own(..)) {
                return Err(unimplemented_at(
                    "`take` of a non-`own` resource in a message is",
                    a.span,
                ));
            }
        }
        slots[idx] = TypedCallArg {
            mode: a.mode,
            value: Some(vt),
        };
    }
    for (i, p) in decl_params.iter().enumerate() {
        if !bound[i] && ast_params[i].default.is_none() {
            return Err(type_error(
                format!("missing argument for parameter `{}`", p.name),
                call_span,
            ));
        }
    }
    Ok(slots)
}

pub(crate) fn check_await(
    inner: &Expr,
    await_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if !fctx.in_async {
        return Err(actor_error(
            "`await` requires an `async fn`/method — a plain `fn` never suspends \
             (02-language.md §5)"
                .to_string(),
            await_span,
        ));
    }
    if !matches!(inner, Expr::Call(..)) {
        let inner_t = check_expr(inner, None, fctx, mctx)?;
        if let Type::Named(n, targs) = &inner_t.ty {
            if n == "Receipt" {
                let Some(types::TypeArg::Type(payload)) = targs.first() else {
                    return Err(type_error(
                        "`Receipt` with no payload type argument".to_string(),
                        await_span,
                    ));
                };
                return Ok(TypedExpr {
                    span: await_span,
                    ty: Type::Named(
                        "IoCompletion".to_string(),
                        vec![types::TypeArg::Type(payload.clone())],
                    ),
                    kind: TypedExprKind::Await(Box::new(inner_t)),
                });
            }
        }
        return Err(actor_error(
            "`await` requires an actor call, a group's `join_all()`, or a `Receipt[P]` \
             (03-hardware.md §3: `completion = await receipt`)"
                .to_string(),
            await_span,
        ));
    }
    let Expr::Call(callee_expr, call_span, args) = inner else {
        unreachable!("checked above");
    };
    let Expr::Field(base, fspan, method_name) = callee_expr.as_ref() else {
        return Err(actor_error(
            "`await` requires a method call through an actor handle or a group's `join_all()` \
             (M6 scope)"
                .to_string(),
            *call_span,
        ));
    };
    if method_name == "join_all" {
        if let Expr::Name(_, gname) = base.as_ref() {
            if fctx.lookup_local(gname) == Some(Type::Named("Group".to_string(), vec![])) {
                return check_await_group_join(gname, args, *call_span, fctx);
            }
        }
    }
    check_await_actor_call(base, *fspan, method_name, args, *call_span, fctx, mctx)
}

pub(crate) fn check_await_actor_call(
    base: &Expr,
    fspan: Span,
    method_name: &str,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let base_t = check_expr(base, None, fctx, mctx)?;
    let Type::Named(outer, targs) = &base_t.ty else {
        return Err(actor_error(
            "`await` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    };
    if outer != "Actor" {
        return Err(actor_error(
            "`await` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    }
    let Some(TypeArg::Type(Type::Named(actor_name, actor_targs))) = targs.first() else {
        return Err(actor_error(
            "`await` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    };
    let instantiated;
    let s = if actor_targs.is_empty() {
        mctx.structs
            .get(actor_name.as_str())
            .ok_or_else(|| actor_error(format!("unknown actor type `{actor_name}`"), fspan))?
    } else {
        instantiated = generics::instantiate_struct(mctx, actor_name, actor_targs, call_span)?;
        &instantiated
    };
    let Some((mf, d)) = s.method(method_name) else {
        return Err(missing_method_error(
            format!("type `{actor_name}` has no method `{method_name}`"),
            actor_name,
            method_name,
            fspan,
        ));
    };
    if !mf.is_pub {
        return Err(actor_error(
            format!(
                "`{method_name}` on `{actor_name}` is not `pub` — only a public method is \
                 callable through `Actor[T]`"
            ),
            fspan,
        ));
    }
    if !d.generics.is_empty() {
        return Err(unimplemented_at("generic instantiation is", call_span));
    }
    let typed_args = check_message_args(&mf.params, &d.params, args, call_span, fctx, mctx)?;
    let take_arg_tys: Vec<Type> = d
        .params
        .iter()
        .zip(typed_args.iter())
        .filter(|(p, _)| p.mode == AccessMode::Take)
        .filter_map(|(_, slot)| slot.value.as_ref().map(|t| t.ty.clone()))
        .collect();
    let callee = if actor_targs.is_empty() {
        CalleeKey::Method(actor_name.clone(), method_name.to_string())
    } else {
        CalleeKey::MethodInstance(
            generics::canonical_key(InstKind::Struct, actor_name, actor_targs),
            method_name.to_string(),
        )
    };
    let call = TypedExpr {
        span: call_span,
        ty: d.ret.clone(),
        kind: TypedExprKind::Call {
            callee,
            receiver: Some(Box::new(base_t)),
            args: typed_args,
        },
    };
    let composed = if s.decl.is_driver && crate::sema::handoff::is_handoff_signature(d) {
        d.ret.clone()
    } else {
        compose_call_error(&d.ret, &take_arg_tys)
    };
    Ok(TypedExpr {
        span: call_span,
        ty: composed,
        kind: TypedExprKind::Await(Box::new(call)),
    })
}

pub(crate) fn check_await_group_join(
    gname: &str,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
) -> Result<TypedExpr, SemaError> {
    if !args.is_empty() {
        return Err(type_error(
            "`join_all` takes no arguments".to_string(),
            call_span,
        ));
    }
    let Some((child_ty, count)) = fctx.group_children.get(gname).cloned() else {
        return Err(actor_error(
            format!("group `{gname}` has no children started (`g.start`) before `join_all`"),
            call_span,
        ));
    };
    let len_expr = Expr::Int(call_span, count.to_string());
    let group_ty = Type::Named("Group".to_string(), vec![]);
    let receiver = Box::new(TypedExpr {
        span: call_span,
        ty: group_ty.clone(),
        kind: TypedExprKind::Local(gname.to_string()),
    });
    let raw = Type::Array(Box::new(child_ty.clone()), Box::new(len_expr.clone()));
    let intrinsic = TypedExpr {
        span: call_span,
        ty: raw,
        kind: TypedExprKind::Intrinsic {
            key: "Group.join_all".to_string(),
            receiver: Some(receiver),
            type_arg: None,
            const_arg: None,
            args: vec![],
        },
    };
    let composed = Type::Array(
        Box::new(compose_call_error(&child_ty, &[])),
        Box::new(len_expr),
    );
    Ok(TypedExpr {
        span: call_span,
        ty: composed,
        kind: TypedExprKind::Await(Box::new(intrinsic)),
    })
}

pub(crate) fn check_send(
    inner: &Expr,
    send_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if !fctx.in_async {
        return Err(actor_error(
            "`send` requires an `async fn`/method context (M6 scope)".to_string(),
            send_span,
        ));
    }
    let Expr::Call(callee_expr, call_span, args) = inner else {
        return Err(actor_error(
            "`send` requires a call expression".to_string(),
            send_span,
        ));
    };
    let Expr::Field(base, fspan, method_name) = callee_expr.as_ref() else {
        return Err(actor_error(
            "`send` requires a method call through an actor handle".to_string(),
            *call_span,
        ));
    };
    check_send_call(base, *fspan, method_name, args, *call_span, fctx, mctx)
}

pub(crate) fn check_send_call(
    base: &Expr,
    fspan: Span,
    method_name: &str,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let base_t = check_expr(base, None, fctx, mctx)?;
    let Type::Named(outer, targs) = &base_t.ty else {
        return Err(actor_error(
            "`send` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    };
    if outer != "Actor" {
        return Err(actor_error(
            "`send` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    }
    let Some(TypeArg::Type(Type::Named(actor_name, actor_targs))) = targs.first() else {
        return Err(actor_error(
            "`send` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    };
    let instantiated;
    let s = if actor_targs.is_empty() {
        mctx.structs
            .get(actor_name.as_str())
            .ok_or_else(|| actor_error(format!("unknown actor type `{actor_name}`"), fspan))?
    } else {
        instantiated = generics::instantiate_struct(mctx, actor_name, actor_targs, call_span)?;
        &instantiated
    };
    let Some((mf, d)) = s.method(method_name) else {
        return Err(missing_method_error(
            format!("type `{actor_name}` has no method `{method_name}`"),
            actor_name,
            method_name,
            fspan,
        ));
    };
    if !mf.is_pub {
        return Err(actor_error(
            format!(
                "`{method_name}` on `{actor_name}` is not `pub` — only a public method is \
                 callable through `Actor[T]`"
            ),
            fspan,
        ));
    }
    if d.ret != Type::Unit {
        return Err(actor_error(
            format!(
                "`send`'s target method must return `unit`, found `{}` (02-language.md §9.4)",
                types::render_type(&d.ret)
            ),
            fspan,
        ));
    }
    if !d.generics.is_empty() {
        return Err(unimplemented_at("generic instantiation is", call_span));
    }
    let typed_args = check_message_args(&mf.params, &d.params, args, call_span, fctx, mctx)?;
    let take_arg_tys: Vec<Type> = d
        .params
        .iter()
        .zip(typed_args.iter())
        .filter(|(p, _)| p.mode == AccessMode::Take)
        .filter_map(|(_, slot)| slot.value.as_ref().map(|t| t.ty.clone()))
        .collect();
    let callee = if actor_targs.is_empty() {
        CalleeKey::Method(actor_name.clone(), method_name.to_string())
    } else {
        CalleeKey::MethodInstance(
            generics::canonical_key(InstKind::Struct, actor_name, actor_targs),
            method_name.to_string(),
        )
    };
    let call = TypedExpr {
        span: call_span,
        ty: Type::Unit,
        kind: TypedExprKind::Call {
            callee,
            receiver: Some(Box::new(base_t)),
            args: typed_args,
        },
    };
    let ty = Type::Result(
        Box::new(Type::Unit),
        Box::new(call_error_type(Type::Never, &take_arg_tys)),
    );
    Ok(TypedExpr {
        span: call_span,
        ty,
        kind: TypedExprKind::Send(Box::new(call)),
    })
}

pub(crate) fn check_send_stmt(
    span: Span,
    e: &Expr,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    let expr = check_send(e, span, fctx, mctx)?;
    Ok(TypedStmt {
        span: span,
        kind: TypedStmtKind::BareSend { span, expr },
    })
}

pub(crate) fn resolve_group_child_callee(
    callee_expr: &Expr,
    fctx: &FnCtx,
    mctx: &ModuleCtx,
) -> Result<(CalleeKey, Vec<ast::Param>, Vec<DeclParam>, Type), SemaError> {
    match callee_expr {
        Expr::Name(span, fname) => {
            let Some(fi) = mctx.fns.get(fname) else {
                return Err(actor_error(
                    format!("`{fname}` is not a fn in this module"),
                    *span,
                ));
            };
            if !fi.decl.is_async {
                return Err(unimplemented_at(
                    "`g.start`'s callee must be `async fn` (a sync fn as a group child) is",
                    *span,
                ));
            }
            if !fi.decl.generics.is_empty() {
                return Err(unimplemented_at("generic instantiation is", *span));
            }
            Ok((
                CalleeKey::Fn(fname.clone()),
                fi.ast.params.clone(),
                fi.decl.params.clone(),
                fi.decl.ret.clone(),
            ))
        }
        Expr::Field(recv, span, method) if matches!(recv.as_ref(), Expr::Name(_, n) if n == "self") =>
        {
            let Some(self_ty) = fctx.lookup_local("self") else {
                return Err(actor_error("`self` is not bound here".to_string(), *span));
            };
            let Type::Named(sname, _) = &self_ty else {
                return Err(actor_error(
                    "`self` is not a struct here".to_string(),
                    *span,
                ));
            };
            let Some(s) = mctx.structs.get(sname.as_str()) else {
                return Err(actor_error(format!("unknown type `{sname}`"), *span));
            };
            let Some((mf, d)) = s.method(method) else {
                return Err(missing_method_error(
                    format!("type `{sname}` has no method `{method}`"),
                    sname,
                    method,
                    *span,
                ));
            };
            if !d.is_async {
                return Err(unimplemented_at(
                    "`g.start`'s callee must be `async fn` (a sync method as a group child) is",
                    *span,
                ));
            }
            if !d.generics.is_empty() {
                return Err(unimplemented_at("generic instantiation is", *span));
            }
            Ok((
                CalleeKey::Method(sname.clone(), method.clone()),
                mf.params.clone(),
                d.params.clone(),
                d.ret.clone(),
            ))
        }
        other => Err(unimplemented_at(
            "`g.start`'s callee must be a bare fn name or `self.method` — anything else is",
            other.span(),
        )),
    }
}

pub(crate) fn check_group_start(
    base_t: TypedExpr,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let Some((callee_arg, rest)) = args.split_first() else {
        return Err(type_error(
            "`g.start` needs a callee argument".to_string(),
            call_span,
        ));
    };
    if callee_arg.label.is_some() {
        return Err(type_error(
            "`g.start`'s callee argument must not be labeled".to_string(),
            callee_arg.span,
        ));
    }
    if !matches!(&base_t.kind, TypedExprKind::Local(_)) {
        return Err(actor_error(
            "`g.start`'s receiver must be a group local".to_string(),
            call_span,
        ));
    }
    let (callee_key, ast_params, decl_params, ret) =
        resolve_group_child_callee(&callee_arg.value, fctx, mctx)?;
    let typed_args = check_call_args(&ast_params, &decl_params, rest, call_span, fctx, mctx)?;
    let callee_fn_ty = Type::Fn(
        decl_params.iter().map(|p| (p.mode, p.ty.clone())).collect(),
        Box::new(ret),
    );
    let child_node = TypedExpr {
        span: call_span,
        ty: callee_fn_ty,
        kind: TypedExprKind::GroupChild(callee_key),
    };
    let mut iargs = vec![("callee".to_string(), child_node)];
    for (p, slot) in decl_params.iter().zip(typed_args.into_iter()) {
        if let Some(v) = slot.value {
            iargs.push((p.name.clone(), v));
        }
    }
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "Group.start".to_string(),
            receiver: Some(Box::new(base_t)),
            type_arg: None,
            const_arg: None,
            args: iargs,
        },
    })
}

pub(crate) fn compute_group_children(
    body: &[Stmt],
    gname: &str,
    fctx: &FnCtx,
    mctx: &ModuleCtx,
) -> Result<Option<(Type, usize)>, SemaError> {
    let mut starts: Vec<&[Arg]> = Vec::new();
    scan_group_starts_stmts(body, gname, &mut starts);
    if starts.is_empty() {
        return Ok(None);
    }
    if let Some(loop_span) = group_starts_inside_loop(body, gname) {
        return Err(actor_error(
            format!(
                "`{gname}.start` cannot appear inside a loop: a group's child count is the number \
                 of *static* `g.start` sites (it is what `join_all`'s own array length and the \
                 group's admission accounting are built from), so one site running twice starts a \
                 child nothing is waiting for — lift the `g.start` out of the loop (M6 scope, \
                 plans/M6.md item H2)"
            ),
            loop_span,
        ));
    }
    let mut result_ty: Option<Type> = None;
    for args in &starts {
        let Some(callee_arg) = args.first() else {
            return Err(type_error(
                "`g.start` needs a callee argument".to_string(),
                Span::default(),
            ));
        };
        let (_, _, _, ret) = resolve_group_child_callee(&callee_arg.value, fctx, mctx)?;
        match &result_ty {
            Some(existing) if *existing != ret => {
                return Err(actor_error(
                    format!(
                        "group `{gname}`'s children must share one return type (M6 scope); \
                         found `{}` and `{}`",
                        types::render_type(existing),
                        types::render_type(&ret)
                    ),
                    callee_arg.span,
                ));
            }
            _ => result_ty = Some(ret),
        }
    }
    Ok(Some((
        result_ty.expect("starts is non-empty"),
        starts.len(),
    )))
}

pub(crate) fn scan_group_starts_stmts<'a>(
    stmts: &'a [Stmt],
    gname: &str,
    out: &mut Vec<&'a [Arg]>,
) {
    for s in stmts {
        scan_group_starts_stmt(s, gname, out);
    }
}

pub(crate) fn group_starts_inside_loop(stmts: &[Stmt], gname: &str) -> Option<Span> {
    fn in_loop_body(body: &[Stmt], gname: &str) -> Option<Span> {
        let mut found = Vec::new();
        scan_group_starts_stmts(body, gname, &mut found);
        found
            .first()
            .map(|args| args.first().map(|a| a.span).unwrap_or_default())
    }
    for s in stmts {
        let hit = match s {
            Stmt::While(w) => in_loop_body(&w.body, gname),
            Stmt::For(f) => in_loop_body(&f.body, gname),
            Stmt::If(i) => group_starts_inside_loop(&i.then_branch, gname)
                .or_else(|| {
                    i.elifs
                        .iter()
                        .find_map(|e| group_starts_inside_loop(&e.body, gname))
                })
                .or_else(|| {
                    i.else_branch
                        .as_ref()
                        .and_then(|b| group_starts_inside_loop(b, gname))
                }),
            Stmt::Match(m) => m
                .arms
                .iter()
                .find_map(|a| group_starts_inside_loop(&a.body, gname)),
            Stmt::With(w) => group_starts_inside_loop(&w.body, gname),
            Stmt::Defer(d) => match &d.body {
                DeferBody::Suite(s) => group_starts_inside_loop(s, gname),
                DeferBody::Expr(_) => None,
            },
            Stmt::ComptimeIf(c) => group_starts_inside_loop(&c.then_branch, gname).or_else(|| {
                c.else_branch
                    .as_ref()
                    .and_then(|b| group_starts_inside_loop(b, gname))
            }),
            _ => None,
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

pub(crate) fn scan_group_starts_stmt<'a>(s: &'a Stmt, gname: &str, out: &mut Vec<&'a [Arg]>) {
    match s {
        Stmt::Assign(a) => {
            scan_group_starts_expr(&a.target, gname, out);
            scan_group_starts_expr(&a.value, gname, out);
        }
        Stmt::If(i) => {
            scan_group_starts_expr(&i.cond, gname, out);
            scan_group_starts_stmts(&i.then_branch, gname, out);
            for elif in &i.elifs {
                scan_group_starts_expr(&elif.cond, gname, out);
                scan_group_starts_stmts(&elif.body, gname, out);
            }
            if let Some(b) = &i.else_branch {
                scan_group_starts_stmts(b, gname, out);
            }
        }
        Stmt::Match(m) => {
            scan_group_starts_expr(&m.scrutinee, gname, out);
            for arm in &m.arms {
                if let Some(g) = &arm.guard {
                    scan_group_starts_expr(g, gname, out);
                }
                scan_group_starts_stmts(&arm.body, gname, out);
            }
        }
        Stmt::For(f) => {
            scan_group_starts_expr(&f.iterable, gname, out);
            scan_group_starts_stmts(&f.body, gname, out);
        }
        Stmt::While(w) => {
            scan_group_starts_expr(&w.cond, gname, out);
            scan_group_starts_stmts(&w.body, gname, out);
        }
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) | Stmt::Dmb(_) => {}
        Stmt::Return(_, e) => {
            if let Some(e) = e {
                scan_group_starts_expr(e, gname, out);
            }
        }
        Stmt::Assert(a) => {
            scan_group_starts_expr(&a.cond, gname, out);
            if let Some(m) = &a.message {
                scan_group_starts_expr(m, gname, out);
            }
        }
        Stmt::Defer(d) => match &d.body {
            DeferBody::Expr(e) => scan_group_starts_expr(e, gname, out),
            DeferBody::Suite(s) => scan_group_starts_stmts(s, gname, out),
        },
        Stmt::With(w) => {
            scan_group_starts_expr(&w.expr, gname, out);
            scan_group_starts_stmts(&w.body, gname, out);
        }
        Stmt::Send(_, e) => scan_group_starts_expr(e, gname, out),
        Stmt::Expr(_, e) => scan_group_starts_expr(e, gname, out),
        Stmt::ComptimeIf(c) => {
            scan_group_starts_expr(&c.cond, gname, out);
            scan_group_starts_stmts(&c.then_branch, gname, out);
            if let Some(b) = &c.else_branch {
                scan_group_starts_stmts(b, gname, out);
            }
        }
        Stmt::ComptimeAssert(_, e, m) => {
            scan_group_starts_expr(e, gname, out);
            if let Some(m) = m {
                scan_group_starts_expr(m, gname, out);
            }
        }
    }
}

pub(crate) fn scan_group_starts_expr<'a>(e: &'a Expr, gname: &str, out: &mut Vec<&'a [Arg]>) {
    if let Expr::Call(callee, _, args) = e {
        if let Expr::Field(base, _, method) = callee.as_ref() {
            if method == "start" {
                if let Expr::Name(_, bn) = base.as_ref() {
                    if bn == gname {
                        out.push(args);
                    }
                }
            }
        }
    }
    match e {
        Expr::Field(b, _, _) => scan_group_starts_expr(b, gname, out),
        Expr::Index(b, _, args) => {
            scan_group_starts_expr(b, gname, out);
            for a in args {
                scan_group_starts_expr(a, gname, out);
            }
        }
        Expr::Call(callee, _, args) => {
            scan_group_starts_expr(callee, gname, out);
            for a in args {
                scan_group_starts_expr(&a.value, gname, out);
            }
        }
        Expr::Unary(_, _, i) => scan_group_starts_expr(i, gname, out),
        Expr::Try(_, i) => scan_group_starts_expr(i, gname, out),
        Expr::Binary(_, _, l, r) => {
            scan_group_starts_expr(l, gname, out);
            scan_group_starts_expr(r, gname, out);
        }
        Expr::Range(_, a, b, _) => {
            scan_group_starts_expr(a, gname, out);
            scan_group_starts_expr(b, gname, out);
        }
        Expr::Is(_, s, _) => scan_group_starts_expr(s, gname, out),
        Expr::Not(_, i) => scan_group_starts_expr(i, gname, out),
        Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            scan_group_starts_expr(l, gname, out);
            scan_group_starts_expr(r, gname, out);
        }
        Expr::DotVariant(_, _, args) => {
            for a in args {
                scan_group_starts_expr(&a.value, gname, out);
            }
        }
        Expr::Closure(c) => match &c.body {
            ClosureBody::Expr(e) => scan_group_starts_expr(e, gname, out),
            ClosureBody::Suite(s) => scan_group_starts_stmts(s, gname, out),
        },
        Expr::Send(_, i) => scan_group_starts_expr(i, gname, out),
        Expr::Tuple(_, items) | Expr::List(_, items) => {
            for i in items {
                scan_group_starts_expr(i, gname, out);
            }
        }
        _ => {}
    }
}

pub(crate) fn check_with(
    w: &WithStmt,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    let Expr::Call(ctor, _cspan, cargs) = &w.expr else {
        return Err(unimplemented_at("`with` is", w.span));
    };
    let Expr::Name(_, ctor_name) = ctor.as_ref() else {
        return Err(unimplemented_at("`with` is", w.span));
    };
    if ctor_name == "pool" {
        return Err(unimplemented_at("`with pool` (scoped pools) is", w.span));
    }
    if ctor_name != "group" {
        return Err(type_error(
            format!(
                "`with {ctor_name}(...)` is not a `with` form: `with` opens exactly two \
                 intrinsic suspend-safe scopes, `group` and scoped `pool`, and there are no \
                 others (02-language.md §10 — an acquire/release API is an ordinary function \
                 used with `defer`, or a closure-taking function)"
            ),
            w.span,
        ));
    }
    if !fctx.in_async {
        return Err(actor_error(
            "`with group` requires an `async fn`/method context — a plain `fn` never \
             suspends (02-language.md §5)"
                .to_string(),
            w.span,
        ));
    }
    let mut capacity = None;
    let mut deadline = None;
    for a in cargs {
        match a.label.as_deref() {
            Some("capacity") => {
                capacity = Some(check_expr(&a.value, Some(&Type::Usize), fctx, mctx)?);
            }
            Some("deadline") => {
                deadline = Some(check_deadline_expr(&a.value, fctx, mctx)?);
            }
            Some(other) => {
                return Err(type_error(
                    format!("`group` has no argument `{other}`"),
                    a.span,
                ));
            }
            None => {
                return Err(type_error(
                    "`group`'s arguments must be labeled (`capacity=`/`deadline=`)".to_string(),
                    a.span,
                ));
            }
        }
    }
    let mut child_count = 0usize;
    let body = scoped(fctx, |fctx| {
        if let Some(name) = &w.as_name {
            fctx.insert_local(name.clone(), Type::Named("Group".to_string(), vec![]));
            if let Some(children) = compute_group_children(&w.body, name, fctx, mctx)? {
                child_count = children.1;
                fctx.group_children.insert(name.clone(), children);
            }
        }
        check_stmts(&w.body, fctx, mctx)
    })?;
    check_group_capacity(capacity.as_ref(), child_count, w.span)?;
    if let Some(name) = &w.as_name {
        fctx.group_children.remove(name);
    }
    Ok(TypedStmt {
        span: w.span,
        kind: TypedStmtKind::WithGroup {
            capacity,
            deadline,
            as_name: w.as_name.clone(),
            body,
        },
    })
}

pub(crate) fn check_group_capacity(
    capacity: Option<&TypedExpr>,
    child_count: usize,
    span: Span,
) -> Result<(), SemaError> {
    if child_count == 0 {
        return Ok(());
    }
    let Some(cap_expr) = capacity else {
        return Err(actor_error(
            format!(
                "this `with group` starts {child_count} child activation(s) but declares no \
                 `capacity=`, and a group's default capacity is zero (02-language.md §9.5) — add \
                 `capacity={child_count}`"
            ),
            span,
        ));
    };
    let TypedExprKind::Int(text) = &cap_expr.kind else {
        return Err(unimplemented_at(
            "a `with group` capacity that is not an integer literal is",
            span,
        ));
    };
    let declared: usize = text.parse().map_err(|_| {
        type_error(
            format!("`capacity={text}` is not a valid group capacity"),
            span,
        )
    })?;
    if child_count > declared {
        return Err(actor_error(
            format!(
                "this `with group` declares `capacity={declared}` but starts {child_count} child \
                 activation(s) (02-language.md §9.5: a group admits up to `capacity` children) — \
                 raise the capacity or start fewer children"
            ),
            span,
        ));
    }
    Ok(())
}

pub(crate) fn check_deadline_expr(
    e: &Expr,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let instant_ty = Type::Named("Instant".to_string(), vec![]);
    match e {
        Expr::Binary(span, BinOp::Add, l, r) => {
            let lt = check_expr(l, None, fctx, mctx)?;
            if lt.ty != instant_ty {
                return Err(type_error(
                    "a group deadline must start from `now()`".to_string(),
                    l.span(),
                ));
            }
            let rt = check_expr(r, None, fctx, mctx)?;
            if rt.ty != Type::Named("Duration".to_string(), vec![]) {
                return Err(type_error(
                    "a group deadline's offset must be a duration (`ms(...)`)".to_string(),
                    r.span(),
                ));
            }
            Ok(TypedExpr {
                span: *span,
                ty: instant_ty,
                kind: TypedExprKind::Binary(BinOp::Add, Box::new(lt), Box::new(rt)),
            })
        }
        other => {
            let t = check_expr(other, None, fctx, mctx)?;
            if t.ty != instant_ty {
                return Err(type_error(
                    format!(
                        "a group deadline must be an `Instant` (`now()` or `now() + ms(...)`), \
                         found `{}`",
                        types::render_type(&t.ty)
                    ),
                    other.span(),
                ));
            }
            Ok(t)
        }
    }
}

pub(crate) struct CrossAwaitScan {
    seen_await: bool,
    after_await: BTreeSet<String>,
    probe: bool,
}

pub(crate) fn check_cross_await(body: &[TypedStmt]) -> Result<(), SemaError> {
    let mut state = CrossAwaitScan {
        seen_await: false,
        after_await: BTreeSet::new(),
        probe: false,
    };
    scan_await_cross_stmts(body, &mut state)
}

pub(crate) fn loop_body_suspends(body: &[TypedStmt]) -> bool {
    let mut probe = CrossAwaitScan {
        seen_await: false,
        after_await: BTreeSet::new(),
        probe: true,
    };
    let scanned = scan_await_cross_stmts(body, &mut probe);
    debug_assert!(scanned.is_ok(), "a probe-mode scan never reports");
    probe.seen_await
}

pub(crate) fn enter_loop_body(body: &[TypedStmt], state: &mut CrossAwaitScan) {
    if state.probe || !loop_body_suspends(body) {
        return;
    }
    state.seen_await = true;
    state.after_await.clear();
}

pub(crate) fn scan_await_cross_stmts(
    stmts: &[TypedStmt],
    state: &mut CrossAwaitScan,
) -> Result<(), SemaError> {
    for s in stmts {
        scan_await_cross_stmt(s, state)?;
    }
    Ok(())
}

pub(crate) fn typed_pattern_bindings(p: &TypedPattern, out: &mut BTreeSet<String>) {
    match &p.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Literal(_) => {}
        TypedPatternKind::Binding(name) => {
            out.insert(name.clone());
        }
        TypedPatternKind::Take(inner) => typed_pattern_bindings(inner, out),
        TypedPatternKind::Variant { payload, .. } => {
            for sp in payload {
                typed_pattern_bindings(sp, out);
            }
        }
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            for i in items {
                typed_pattern_bindings(i, out);
            }
        }
        TypedPatternKind::Or(alts) => {
            for a in alts {
                typed_pattern_bindings(a, out);
            }
        }
    }
}

pub(crate) fn scan_await_cross_stmt(
    s: &TypedStmt,
    state: &mut CrossAwaitScan,
) -> Result<(), SemaError> {
    match &s.kind {
        TypedStmtKind::Let { name, value, .. } => {
            scan_await_cross_expr(value, state)?;
            if state.seen_await {
                state.after_await.insert(name.clone());
            }
            Ok(())
        }
        TypedStmtKind::Assign { target, value } => {
            scan_await_cross_expr(target, state)?;
            scan_await_cross_expr(value, state)?;
            if state.seen_await {
                if let TypedExprKind::Local(name) = &target.kind {
                    state.after_await.insert(name.clone());
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
            scan_await_cross_expr(cond, state)?;
            scan_await_cross_stmts(then_branch, state)?;
            for elif in elifs {
                scan_await_cross_expr(&elif.cond, state)?;
                scan_await_cross_stmts(&elif.body, state)?;
            }
            if let Some(b) = else_branch {
                scan_await_cross_stmts(b, state)?;
            }
            Ok(())
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            scan_await_cross_expr(scrutinee, state)?;
            for arm in arms {
                if state.seen_await {
                    typed_pattern_bindings(&arm.pattern, &mut state.after_await);
                }
                if let Some(g) = &arm.guard {
                    scan_await_cross_expr(g, state)?;
                }
                scan_await_cross_stmts(&arm.body, state)?;
            }
            Ok(())
        }
        TypedStmtKind::For {
            name, iter, body, ..
        } => {
            match iter {
                TypedForIter::Range(a, b, _) => {
                    scan_await_cross_expr(a, state)?;
                    scan_await_cross_expr(b, state)?;
                }
                TypedForIter::Expr(e) => scan_await_cross_expr(e, state)?,
            }
            enter_loop_body(body, state);
            if state.seen_await {
                state.after_await.insert(name.clone());
            }
            scan_await_cross_stmts(body, state)
        }
        TypedStmtKind::While { cond, body, .. } => {
            scan_await_cross_expr(cond, state)?;
            enter_loop_body(body, state);
            scan_await_cross_stmts(body, state)
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => Ok(()),
        TypedStmtKind::Return(value) => match value {
            Some(e) => scan_await_cross_expr(e, state),
            None => Ok(()),
        },
        TypedStmtKind::Assert { cond, message } => {
            scan_await_cross_expr(cond, state)?;
            if let Some(m) = message {
                scan_await_cross_expr(m, state)?;
            }
            Ok(())
        }
        TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            scan_await_cross_expr(cond, state)?;
            if let Some(m) = message {
                scan_await_cross_expr(m, state)?;
            }
            Ok(())
        }
        TypedStmtKind::Defer(_) => Ok(()),
        TypedStmtKind::ExprStmt(e) => scan_await_cross_expr(e, state),
        TypedStmtKind::BareSend { expr, .. } => scan_await_cross_expr(expr, state),
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            as_name,
            body,
        } => {
            if let Some(c) = capacity {
                scan_await_cross_expr(c, state)?;
            }
            if let Some(d) = deadline {
                scan_await_cross_expr(d, state)?;
            }
            if state.seen_await {
                if let Some(n) = as_name {
                    state.after_await.insert(n.clone());
                }
            }
            scan_await_cross_stmts(body, state)
        }
    }
}

pub(crate) fn root_local_name(e: &TypedExpr) -> Option<&str> {
    match &e.kind {
        TypedExprKind::Local(name) => Some(name.as_str()),
        TypedExprKind::Field(base, _) | TypedExprKind::Index(base, _) => root_local_name(base),
        _ => None,
    }
}

pub(crate) fn scan_await_cross_expr(
    e: &TypedExpr,
    state: &mut CrossAwaitScan,
) -> Result<(), SemaError> {
    match &e.kind {
        TypedExprKind::Int(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::Str(_)
        | TypedExprKind::BStr(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Unit
        | TypedExprKind::Local(_)
        | TypedExprKind::Const(_)
        | TypedExprKind::Static(_)
        | TypedExprKind::FnRef(_)
        | TypedExprKind::PoolName(_)
        | TypedExprKind::GroupChild(_) => Ok(()),
        TypedExprKind::Field(base, _) => {
            if state.seen_await && !state.probe {
                if let Some(root) = root_local_name(e) {
                    if root != "self" && !state.after_await.contains(root) {
                        return Err(SemaError::nowhere(
                            "actor",
                            format!(
                                "`{root}`-rooted access cannot span an `await` — only a \
                                 self-rooted path may (02-language.md §9.2)"
                            ),
                        ));
                    }
                }
            }
            scan_await_cross_expr(base, state)
        }
        TypedExprKind::Index(base, idx) => {
            if state.seen_await && !state.probe {
                if let Some(root) = root_local_name(e) {
                    if root != "self" && !state.after_await.contains(root) {
                        return Err(SemaError::nowhere(
                            "actor",
                            format!(
                                "`{root}`-rooted access cannot span an `await` — only a \
                                 self-rooted path may (02-language.md §9.2)"
                            ),
                        ));
                    }
                }
            }
            scan_await_cross_expr(base, state)?;
            scan_await_cross_expr(idx, state)
        }
        TypedExprKind::Call { receiver, args, .. } => {
            if let Some(r) = receiver {
                scan_await_cross_expr(r, state)?;
            }
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                scan_await_cross_expr(a, state)?;
            }
            Ok(())
        }
        TypedExprKind::CallValue(callee, args) => {
            scan_await_cross_expr(callee, state)?;
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                scan_await_cross_expr(a, state)?;
            }
            Ok(())
        }
        TypedExprKind::ToScalar(inner)
        | TypedExprKind::Neg(inner)
        | TypedExprKind::BitNot(inner)
        | TypedExprKind::Take(inner)
        | TypedExprKind::Not(inner) => scan_await_cross_expr(inner, state),
        TypedExprKind::Try(inner, _) => scan_await_cross_expr(inner, state),
        TypedExprKind::Binary(_, l, r) | TypedExprKind::OpCall(_, l, r) => {
            scan_await_cross_expr(l, state)?;
            scan_await_cross_expr(r, state)
        }
        TypedExprKind::Is(inner, _) => scan_await_cross_expr(inner, state),
        TypedExprKind::And(l, r) | TypedExprKind::Or(l, r) => {
            scan_await_cross_expr(l, state)?;
            scan_await_cross_expr(r, state)
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                scan_await_cross_expr(a, state)?;
            }
            Ok(())
        }
        TypedExprKind::Closure { .. } => Ok(()),
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                scan_await_cross_expr(i, state)?;
            }
            Ok(())
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                scan_await_cross_expr(v, state)?;
            }
            Ok(())
        }
        TypedExprKind::Panic(msg) => scan_await_cross_expr(msg, state),
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                scan_await_cross_expr(r, state)?;
            }
            for (_, a) in args {
                scan_await_cross_expr(a, state)?;
            }
            Ok(())
        }
        TypedExprKind::Await(inner) => {
            scan_await_cross_expr(inner, state)?;
            state.seen_await = true;
            state.after_await.clear();
            Ok(())
        }
        TypedExprKind::Send(inner) => scan_await_cross_expr(inner, state),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sema::types::{Type, TypeArg};

    fn call_error_of(e: &Type) -> Type {
        Type::Named("CallError".to_string(), vec![TypeArg::Type(e.clone())])
    }

    #[test]
    fn compose_call_error_wraps_a_plain_declared_type() {
        let composed = compose_call_error(&Type::U64, &[]);
        assert_eq!(
            composed,
            Type::Result(Box::new(Type::U64), Box::new(call_error_of(&Type::Never)))
        );
    }

    #[test]
    fn compose_call_error_rewraps_a_declared_result() {
        let declared = Type::Result(
            Box::new(Type::U32),
            Box::new(Type::Named("FsError".to_string(), vec![])),
        );
        let composed = compose_call_error(&declared, &[]);
        assert_eq!(
            composed,
            Type::Result(
                Box::new(Type::U32),
                Box::new(call_error_of(&Type::Named("FsError".to_string(), vec![])))
            )
        );
    }

    #[test]
    fn compose_call_error_treats_every_non_result_type_uniformly() {
        let cases = vec![
            Type::Unit,
            Type::Option(Box::new(Type::U8)),
            Type::Named("Widget".to_string(), vec![]),
            Type::Static(Box::new(Type::Str)),
        ];
        for ty in cases {
            let composed = compose_call_error(&ty, &[]);
            match composed {
                Type::Result(ok, err) => {
                    assert_eq!(*ok, ty, "the declared type itself must be the Ok payload");
                    assert_eq!(
                        *err,
                        call_error_of(&Type::Never),
                        "error side is CallError[never]"
                    );
                }
                other => panic!("composition must always be a Result, got {other:?}"),
            }
        }
    }

    #[test]
    fn compose_call_error_is_a_pure_function_of_its_input() {
        let a = compose_call_error(&Type::U64, &[]);
        let b = compose_call_error(&Type::U64, &[]);
        assert_eq!(a, b);
    }

    #[test]
    fn compose_call_error_carries_take_arg_tuple() {
        let composed = compose_call_error(&Type::U64, &[Type::U32, Type::U64]);
        let Type::Result(_, err) = composed else {
            panic!("expected Result");
        };
        let Type::Named(name, targs) = *err else {
            panic!("expected CallError");
        };
        assert_eq!(name, "CallError");
        assert_eq!(targs.len(), 2);
        assert_eq!(
            not_admitted_args_type(&targs),
            Type::Tuple(vec![Type::U32, Type::U64])
        );
    }

    #[test]
    fn root_local_name_finds_the_bottom_of_a_field_chain() {
        let self_local = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Local("self".to_string()),
        };
        assert_eq!(root_local_name(&self_local), Some("self"));

        let one_level = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Field(Box::new(self_local.clone()), "fs".to_string()),
        };
        assert_eq!(
            root_local_name(&one_level),
            Some("self"),
            "a one-level field access still roots at self"
        );

        let two_level = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Field(Box::new(one_level), "cache".to_string()),
        };
        assert_eq!(
            root_local_name(&two_level),
            Some("self"),
            "self.fs.cache must still root at self regardless of chain depth"
        );

        let external = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Local("input".to_string()),
        };
        let external_field = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Field(Box::new(external), "value".to_string()),
        };
        assert_eq!(root_local_name(&external_field), Some("input"));

        let external_idx_base = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Local("input".to_string()),
        };
        let external_index = TypedExpr {
            span: Span::default(),
            ty: Type::U64,
            kind: TypedExprKind::Index(
                Box::new(external_idx_base),
                Box::new(TypedExpr {
                    span: Span::default(),
                    ty: Type::Usize,
                    kind: TypedExprKind::Int("0".to_string()),
                }),
            ),
        };
        assert_eq!(
            root_local_name(&external_index),
            Some("input"),
            "bare `input[0]` must root at `input` (Index is not a Field bypass)"
        );

        let no_root = TypedExpr {
            span: Span::default(),
            ty: Type::U64,
            kind: TypedExprKind::Int("1".to_string()),
        };
        assert_eq!(
            root_local_name(&no_root),
            None,
            "a literal has no local root at all"
        );
    }

    #[test]
    fn check_cross_await_accepts_self_and_rejects_external_paths() {
        fn field(base_name: &str, field_name: &str) -> TypedExpr {
            TypedExpr {
                span: Span::default(),
                ty: Type::U64,
                kind: TypedExprKind::Field(
                    Box::new(TypedExpr {
                        span: Span::default(),
                        ty: Type::Unit,
                        kind: TypedExprKind::Local(base_name.to_string()),
                    }),
                    field_name.to_string(),
                ),
            }
        }
        fn let_stmt(name: &str, value: TypedExpr) -> TypedStmt {
            TypedStmt {
                span: Span::default(),
                kind: TypedStmtKind::Let {
                    name: name.to_string(),
                    ty: value.ty.clone(),
                    value,
                },
            }
        }
        let await_node = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Await(Box::new(TypedExpr {
                span: Span::default(),
                ty: Type::Unit,
                kind: TypedExprKind::Local("dummy".to_string()),
            })),
        };

        let self_ok = vec![
            let_stmt("before", field("self", "cache")),
            let_stmt("suspend", await_node.clone()),
            let_stmt("after", field("self", "cache")),
        ];
        assert!(
            check_cross_await(&self_ok).is_ok(),
            "a self-rooted access spanning an await must be accepted"
        );

        let external_after = vec![
            let_stmt("suspend", await_node.clone()),
            let_stmt("bad", field("input", "value")),
        ];
        assert!(
            check_cross_await(&external_after).is_err(),
            "an external-rooted access after an await must be rejected"
        );

        let external_before = vec![
            let_stmt("fine", field("input", "value")),
            let_stmt("suspend", await_node.clone()),
        ];
        assert!(
            check_cross_await(&external_before).is_ok(),
            "an external-rooted access entirely before an await must be accepted"
        );

        let bound_from_await = vec![
            let_stmt("completion", await_node),
            let_stmt("status", field("completion", "status")),
        ];
        assert!(
            check_cross_await(&bound_from_await).is_ok(),
            "a local bound from the await result must be field-accessible after it"
        );
    }
}
