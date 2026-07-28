//! Actor/async call surface (plans/M6.md item A): `await`/`send`/
//! `with group`, message args, CallError composition, and the
//! cross-await path rule (02-language.md §9.2/§9.4/§9.5).
//! Extracted from `bodies.rs` along the artifact boundary.

use std::collections::BTreeSet;

use crate::sema::bodies::{
    FnCtx, ModuleCtx, check_call_args, check_expr, check_stmts, is_resource_type,
    missing_method_error, parse_int_literal, scoped, type_error, types_eq,
};
use crate::sema::typed::{
    CalleeKey, TypedCallArg, TypedExpr, TypedExprKind, TypedForIter, TypedPattern,
    TypedPatternKind, TypedStmt, TypedStmtKind,
};
use crate::sema::types::{self, DeclParam, Type, TypeArg};
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{
    self, AccessMode, Arg, BinOp, ClosureBody, DeferBody, Expr, Span, Stmt, WithStmt,
};

// --- plans/M6.md item A: the actor/async surface --------------------------
//
// `Actor[T]` calls (`await`/`send`), `with group(...)`/`g.start`/
// `g.join_all`, and the cross-await path rule (02-language.md §9.2/§9.4/
// §9.5). Every construct outside this exact shape stays fail-closed,
// named, exactly like the rest of decision 7's set.

pub(crate) fn actor_error(message: String, span: Span) -> SemaError {
    SemaError::at("actor", message, span)
}

/// The CallError composition table, verbatim (02-language.md §9.4):
/// `declared R -> Result[R, CallError[never]]`; `declared Result[T, E] ->
/// Result[T, CallError[E]]`. `CallError` is carried as a plain
/// `Type::Named("CallError", [TypeArg::Type(E)])` — the `Option`/`Result`
/// precedent stops at two fixed builtin sums; `CallError`'s own five
/// variants (`Op`/`Cancelled`/`DeadlineExceeded`/`NotAdmitted`/
/// `PeerFailed`) are instead recognized directly wherever a scrutinee's
/// type says `CallError` by name (`variant_payload_types_for`/
/// `matches::shape_of`), the same "builtin_enum_variants precedent" the
/// plan names — a fixed, compiler-known variant/payload table, just with
/// non-empty payloads unlike `Target`/`Failure`'s fieldless ones.
/// Variant erasure (decision 8) ships nothing at M6: no whole-image
/// analysis proves any variant unreachable yet, so every composition
/// keeps the full five-variant `CallError[E]` — recorded, not silently
/// approximated (the plan's own "record what you shipped").
///
/// plans/M13.md item H / decision 4: when the call has `take` arguments,
/// `CallError` grows a second type argument — the take-args tuple — so
/// `NotAdmitted`'s pattern payload is `(Admission, args)` monomorphized
/// per signature. Calls with no `take` args keep the one-argument form
/// (second payload types as `()`).
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

/// `CallError[E]` or `CallError[E, Args]` (item H) for a composed await.
pub(crate) fn call_error_type(e_ty: Type, take_arg_tys: &[Type]) -> Type {
    let mut targs = vec![TypeArg::Type(e_ty)];
    if !take_arg_tys.is_empty() {
        targs.push(TypeArg::Type(Type::Tuple(take_arg_tys.to_vec())));
    }
    Type::Named("CallError".to_string(), targs)
}

/// `NotAdmitted`'s take-args tuple type from a `CallError`'s type arguments
/// (absent / empty → `()`).
pub(crate) fn not_admitted_args_type(targs: &[TypeArg]) -> Type {
    match targs.get(1) {
        Some(TypeArg::Type(t)) => t.clone(),
        _ => Type::Tuple(vec![]),
    }
}

/// `CallError` equality for `?` / `from`: same `E`, Args may differ.
/// A `from(take e: CallError[E])` must accept a site-monomorphized
/// `CallError[E, (T,)]` (item H + item I).
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

/// `compose_call_error`'s exact inverse — the declared reply type behind
/// an already-composed `await` result: `Result[T, CallError[never]]` ->
/// `T`; `Result[T, CallError[E]]` (E != `never`) -> `Result[T, E]`.
/// `None` for anything that is not a composed actor-call result at all.
///
/// plans/M7.md item Z1 (decision 9b) needs this to size an async fn's own
/// reply staging slot (`codegen::Frame::reply_stage_off`) and to decide,
/// per `await` site, whether the wide transport is needed at all: the
/// composed type is already in the FlowWir frame, so inverting it is
/// strictly cheaper than threading the declared type through
/// `flowwir_lower` as a second, drift-prone copy of the same fact.
///
/// It lives here, immediately under the composition it inverts, for the
/// same "one shared definition" reason `sema::types::validate_message_shape`
/// calls `codegen::is_aggregate` directly rather than copying it: the day
/// the table above changes, both halves are on the same screen and cannot
/// silently disagree.
///
/// **The pair is NOT total, and the exception is load-bearing** (found by
/// plans/M7.md item I's sweep; this comment used to claim
/// `decompose_call_error(&compose_call_error(t)) == Some(t)` for every
/// `t`, which is false). `compose_call_error` is not injective: `t = T`
/// and `t = Result[T, never]` both compose to `Result[T, CallError[never]]`,
/// because §9.4's two rows genuinely collide when `E` is `never`. This
/// answers `T` for that composed type, so a `Result[T, never]` reply
/// round-tripped to the *wrong* declared type — and item Z1's transport
/// then read the two ends of one `await` through two different
/// predicates (this one caller-side, `codegen::is_aggregate(&f.ret)`
/// callee-side), which turned the ambiguity into a shifted payload for an
/// aggregate `T` and a write through a null `x8` for a scalar one. The
/// collision is refused at the declaration now
/// (`sema::types::validate_message_shape`, `golden/err-actor-reply-never-error`),
/// which is what restores totality over every reply shape that can reach
/// here — a `never` nested any deeper (`Result[T, Option[never]]`)
/// composes and decomposes correctly and is untouched.
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

/// `CallError[E]`'s own variant *numbering* — 02-language.md §9.4 declares
/// the order (`Op`, `Cancelled`, `DeadlineExceeded`, `NotAdmitted`,
/// `PeerFailed`) and `variant_payload_types_for`/`matches::shape_of` above
/// build exactly that order when they type an arm's payload; this is the
/// same table read as an index, which is what a lowered `match`'s own tag
/// comparison needs. `None` for a name that is not a `CallError` variant
/// at all (sema has already rejected those, so a lowering caller treats it
/// as a producer bug).
///
/// It lives here, beside the composition, because `CallError` is the one
/// enum this compiler knows *without* a declaration: it is carried as an
/// instantiated `Type::Named("CallError", [E])` and therefore appears in
/// no `TypedProgram::enums` map, so every consumer that would otherwise
/// look the numbering up has to be told it. Consumers, all cross-checked
/// against this order: `codegen::CALL_ERROR_TAG_CANCELLED` (= 1),
/// `codegen::enum_payload_offset`'s own `CallError` arm, and
/// `flowwir_lower::variant_index`.
pub(crate) fn call_error_variant_index(variant: &str) -> Option<usize> {
    match variant {
        "Op" => Some(0),
        "Cancelled" => Some(1),
        "DeadlineExceeded" => Some(2),
        "NotAdmitted" => Some(3),
        _ => None,
    }
}

/// Message-value restrictions (02-language.md §9.3): a `mut` loan or a
/// lent closure is rejected, named; `take` of a resource is M7 (fail
/// closed, named, distinct from the flat rejection — the plan's own
/// "distinct message from the flat rejection"); `take` of plain data (not
/// a resource) and a bare `Static[T]`/plain-data argument are both fine,
/// same as an ordinary call. Otherwise identical to `check_call_args`
/// (label/positional binding, defaults) — duplicated rather than
/// threaded through it because `check_call_args` does not return which
/// source `Arg` (and so which `AccessMode`) filled which slot, and this
/// needs exactly that to apply the restriction per argument.
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
        // 02-language.md §9.3: resources move into messages only with
        // `take`. Unmarked/`read` used to typecheck and leave the sender
        // initialized while the mailbox held a copy — double ownership.
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
            // plans/M7.md item E4 / 03-hardware.md §5: a handoff may
            // `take` an `own[P] T` transfer payload into an awaitable
            // driver call. Other resource takes in messages stay closed.
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

/// `await expr` (02-language.md §9.4/§9.5 + 03-hardware.md §3/§5): an
/// actor-handle method call, a group's `join_all()`, or a `Receipt[P]`
/// value (plans/M7.md item E4: `completion = await receipt`).
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
    // plans/M7.md item E4: `await receipt` — not a call.
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
    let Some(TypeArg::Type(Type::Named(actor_name, _))) = targs.first() else {
        return Err(actor_error(
            "`await` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    };
    let Some(s) = mctx.structs.get(actor_name.as_str()) else {
        return Err(actor_error(
            format!("unknown actor type `{actor_name}`"),
            fspan,
        ));
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
        // Method-owned generics on an actor-message target: ordinary
        // (non-message) method calls instantiate (item Q); the message
        // path still needs take/handoff composition against the
        // substituted signature — fail closed until that lands.
        return Err(unimplemented_at("generic instantiation is", call_span));
    }
    let typed_args = check_message_args(&mf.params, &d.params, args, call_span, fctx, mctx)?;
    // plans/M13.md item H / decision 4: `NotAdmitted` hands back the
    // call's `take` arguments as an owned tuple — collect their types in
    // declaration order for CallError's optional second type argument.
    let take_arg_tys: Vec<Type> = d
        .params
        .iter()
        .zip(typed_args.iter())
        .filter(|(p, _)| p.mode == AccessMode::Take)
        .filter_map(|(_, slot)| slot.value.as_ref().map(|t| t.ty.clone()))
        .collect();
    let call = TypedExpr {
        span: call_span,
        ty: d.ret.clone(),
        kind: TypedExprKind::Call {
            callee: CalleeKey::Method(actor_name.clone(), method_name.to_string()),
            receiver: Some(Box::new(base_t)),
            args: typed_args,
        },
    };
    // 03-hardware.md §5, the handoff calling convention (plans/M8.md item
    // E, decision 32). "Any public synchronous `@driver` method with
    // exactly one `take p: P` parameter and result `Receipt[P]` receives
    // the handoff calling convention" — a *different* convention from
    // 02 §9.4's composed awaitable, and §5 states its result by name:
    // `Receipt[P]`, not `Result[Receipt[P], CallError[never]]`. The
    // receipt is the caller's endpoint on work the device has not done
    // yet; the failure vocabulary that matters to it is the receipt's own
    // state machine (`Resolved` / `Recovery`), reached by `await`ing it,
    // not `CallError`.
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

/// `send actor.method(...)` (02-language.md §9.4), reached both from the
/// expression form (`Expr::Send`) and, for diagnostics only, from the
/// always-rejected bare statement form (`check_send_stmt`). `inner` is
/// always a call (the ast's own comment on both `Expr::Send`/`Stmt::Send`).
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
    let Some(TypeArg::Type(Type::Named(actor_name, _))) = targs.first() else {
        return Err(actor_error(
            "`send` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    };
    let Some(s) = mctx.structs.get(actor_name.as_str()) else {
        return Err(actor_error(
            format!("unknown actor type `{actor_name}`"),
            fspan,
        ));
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
    // plans/M13.md item J / decision 5: `send` is `Result[unit, CallError[never]]`
    // (with take-args as CallError's optional second type argument, same
    // shape as an awaited unit-returning call). Whole-image erasure leaves
    // `NotAdmitted` as the one reachable variant; the bare `Rejected` type
    // is deleted.
    let take_arg_tys: Vec<Type> = d
        .params
        .iter()
        .zip(typed_args.iter())
        .filter(|(p, _)| p.mode == AccessMode::Take)
        .filter_map(|(_, slot)| slot.value.as_ref().map(|t| t.ty.clone()))
        .collect();
    let call = TypedExpr {
        span: call_span,
        ty: Type::Unit,
        kind: TypedExprKind::Call {
            callee: CalleeKey::Method(actor_name.clone(), method_name.to_string()),
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

/// A bare `send` statement (02-language.md §9.4's proof-conditioned
/// form). The call itself is fully typed here, exactly like the
/// expression form; whether the *bare statement* is legal is the
/// whole-image question `sema::send_proof` answers once every module is
/// typed (plans/M6.md item G) — a mailbox capacity lives in the `@image`
/// fn, which no body-checking pass can see. The `send` keyword's own
/// span rides along on the node so that late rejection still reports a
/// real `at L:C` (`TypedStmtKind::BareSend`'s own doc comment).
///
/// Item A's floor — reject every bare `send` here, unconditionally —
/// is what this replaces; a genuine mistake in the call itself (unknown
/// method, bad message argument, a non-`unit` reply, `send` outside an
/// `async fn`) still reports its own error from `check_send` first,
/// before the proof ever runs.
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

/// `g.start(callee, args...)`'s own callee argument (02-language.md
/// §9.5) — the dumbest doc-consistent callee set (recorded, per the
/// plan's own "decide the dumbest doc-consistent callee set"): a bare
/// same-module top-level `async fn` name, or `self.method` naming an
/// `async fn` method on the enclosing struct. Both are recognized
/// directly (not through `synth_name`'s ordinary lookup — an async
/// fn/method is never otherwise a callable value, see `TypedExprKind::GroupChild`'s
/// own doc comment) so no bound-method-value machinery is needed.
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
    // The running child count/unified return type is *not* accumulated
    // here (a mutation every pass that re-invokes `bodies::check_expr`
    // on just this one call — `matches.rs`/`flow.rs`'s own re-derived
    // `fctx`, neither of which replays the whole preceding body through
    // `bodies::check_expr` — would have to reproduce identically): it is
    // computed once, up front, by `compute_group_children` (a pure
    // static scan over the raw `with`-body), and `check_with` seeds
    // `fctx.group_children` with that *before* this body is ever walked.
    // This call only needs its own callee's shape to build its own typed
    // node.
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

/// One `with group(...) as g:` block's own children, computed once, up
/// front, as a **pure** static scan over the raw `with`-body (no
/// dependence on walk order or on `fctx`'s own mutable state, besides a
/// read-only `self`-type lookup for a `self.method` callee) — see
/// `check_group_start`'s own doc comment for why this must not be
/// incremental: `matches.rs`/`flow.rs` both re-derive their own separate
/// `fctx` and re-invoke `bodies::check_expr` on individual sub-
/// expressions out of full sequence (a plain assignment's inferred
/// type, a `match` scrutinee), never replaying the whole preceding body
/// through it — a pure, order-independent scan is the one shape every
/// pass can call identically and get the same answer. `Ok(None)` means
/// no `g.start` call addressing `gname` was found in `body` at all
/// (`join_all`'s own "no children started" error, not this function's).
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

/// plans/M6.md item H2: does a `g.start` for `gname` sit inside a loop?
///
/// Everything downstream treats the number of *static* `g.start` sites as
/// the number of child activations — `join_all`'s own array length, the
/// group arena's admission accounting, and (since H2) the declared
/// `capacity` check. A `g.start` in a loop breaks that identity: it is one
/// static site that runs N times, so the program compiled clean, started
/// two children, and then deadlocked in `join_all` waiting on a count of
/// one. Rejected by name instead, which is the same discipline decision 5
/// already applies to a bare `send` ("outside any loop... so each executes
/// at most once per root turn").
///
/// Written as its own walk that delegates to `scan_group_starts_stmts`
/// once it is inside a loop body, rather than threading a depth counter
/// through that scanner's fifteen arms: the question is only ever asked
/// about whole loop bodies, and reusing the existing scanner keeps the two
/// from disagreeing about what a `g.start` even is.
pub(crate) fn group_starts_inside_loop(stmts: &[Stmt], gname: &str) -> Option<Span> {
    fn in_loop_body(body: &[Stmt], gname: &str) -> Option<Span> {
        let mut found = Vec::new();
        scan_group_starts_stmts(body, gname, &mut found);
        // The offending `g.start`'s own first argument carries the only
        // span this scanner ever sees (`Arg::span`); a zero-argument
        // `g.start` is already a "needs a callee argument" error one
        // layer up, so the fallback is unreachable in practice.
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

/// `with group(capacity=.., deadline=..) [as g]:` (02-language.md §9.5,
/// §10). The scoped `pool` form of `with` (02-language.md §10's other
/// intrinsic scope) stays fail-closed — the M6 honest-scope line only
/// lifts `group`.
///
/// plans/M8.md item R, decision 16: the two rejections below are told
/// apart by name. `with pool` is the language's *other* intrinsic scope,
/// unimplemented — `error[unimplemented]`, the fail-closed category, and
/// the only reason 04-compiler.md §3's own group-vs-pool comparison
/// cannot be written as one pair of same-shaped goldens today. Any other
/// constructor is not a `with` form at all (02 §10: "There are no other
/// `with` forms and no user-declared scope protocols") — a permanent
/// `error[type]`, never a fail-closed one, so no reader is left waiting
/// for a milestone that will never come. Before this split, every
/// non-`group` constructor was blamed on `with pool` by name, which was a
/// wrong answer for `with anything_else(...)`; `pool` itself never
/// reached here at all (`intrinsics::is_bare_resolvable` / item I).
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

/// plans/M6.md item H2: a group admits "up to `capacity` child
/// activations (default zero)" — 02-language.md §9.5, verbatim.
///
/// Before this check the declared capacity was inert: it type-checked as
/// a `Usize` and was stored into the group arena at `OFF_GROUP_CAPACITY`,
/// which **nothing ever read**, so `capacity=0` (and an omitted capacity,
/// the documented default) started and completed a child anyway. The
/// adversarial sweep found it; `boot-group-join` could not, because it
/// declares `capacity=2` with exactly two children, so enforced and
/// ignored look identical there.
///
/// Enforced statically rather than at admission time, deliberately. The
/// runtime alternative — refuse the activation and hand back
/// `NotAdmitted` — needs a `CallError` composition that does not exist at
/// M6 (item H3 is the same missing piece surfacing at a mailbox), so the
/// only honest runtime option available today would be an abort. A build
/// error is both dumber and strictly more useful, and it is exact:
/// `compute_group_children` rejects a `g.start` in a loop, so the static
/// site count IS the activation count.
pub(crate) fn check_group_capacity(
    capacity: Option<&TypedExpr>,
    child_count: usize,
    span: Span,
) -> Result<(), SemaError> {
    if child_count == 0 {
        return Ok(()); // a bare deadline scope: nothing to admit.
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

/// A group's `deadline=` argument (02-language.md §9.5): `now()` alone,
/// or `now() + ms(...)` — the only two shapes the docs' own examples use.
/// Handled directly rather than through `check_binary`/`build_binop_expr`
/// (which require both operands to share one type, decision 4's own
/// same-type-operand rule — `Instant + Duration` is deliberately not a
/// uniform-type op): the primitive `Binary` node is reused for the sum
/// (mirrors its own doc comment's "builtin scalar op" precedent, extended
/// here to the two other builtin primitive-shaped types this milestone
/// adds), confined to this one call site.
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

/// Cross-await access rule (02-language.md §9.2): "a whole-value access
/// rooted at the current actor (`self.fs.cache`) may live across `await`
/// ... but an access rooted in an external argument may not." 04 §1:
/// "whole-value accesses surviving `await` are rooted at the current
/// actor turn." The operative verbs are *live across* / *surviving* —
/// the rule is about a path that predates a suspension and is used after
/// it, not about every field access that happens to sit lexically after
/// an `await` in the same body (plans/M9.md item J2d / decision 525).
///
/// Approximation: a straight-line forward scan over an async body's
/// already-typed statements, threading one `seen_await` flag
/// (conservatively shared across sibling branches — an `await` in one
/// `if` arm taints every statement lexically after the whole `if`, even
/// along a sibling arm that itself had none; over-rejects a little,
/// never under-rejects) plus the set of locals whose binding is observed
/// *after* that flag is set (Let / match-arm / for / `with ... as` /
/// post-await Assign-to-local). Any `Field`-chain (`x.a.b`) whose root
/// is not `self` and is not in that post-await set, found once
/// `seen_await` is set, is rejected — a bare local reference (no field)
/// is unaffected, since only a *nested* access is the "whole-value
/// access" §9.2 restricts. A value bound from the await itself
/// (`completion = await receipt`; 03 §3) is in the post-await set and
/// is allowed; an external argument / pre-await local that spans is not.
///
/// **Loop back edges** (plans/M9.md item RR): a forward scan alone is not
/// conservative over a loop. In
///
/// ```text
/// while i < n:
///     total = total + input.value   # <- runs again after the await below
///     r = await self.peer.get()
/// ```
///
/// `input.value` sits lexically *before* the only `await`, so a pure
/// forward scan never has `seen_await` set when it reaches the access —
/// yet every iteration after the first reads `input` on the far side of
/// the previous iteration's suspension, which is exactly what §9.2
/// forbids (the unrolled two-iteration spelling of the same program is
/// rejected). So a `while`/`for` whose body can suspend enters that body
/// with `seen_await` already set and the post-await exemption cleared:
/// the back edge is treated as a suspension the whole body follows.
/// `loop_body_suspends` answers "can this body suspend" by replaying this
/// same scan in `probe` mode, so there is exactly one walk to keep in
/// step with the grammar rather than a second shadow traversal.
///
/// This keeps the over-reject/never-under-reject direction the rest of
/// the approximation promises: a body that provably runs once still pays
/// the loop rule, which is the safe side.
struct CrossAwaitScan {
    seen_await: bool,
    /// Locals bound after `seen_await` became true — they do not span
    /// any suspension observed so far on this forward scan.
    after_await: BTreeSet<String>,
    /// `loop_body_suspends`'s own mode: walk purely to discover whether a
    /// suspension is reachable, reporting nothing. Two effects, both
    /// required for the probe to be a *predicate* rather than a second
    /// checker: the `Field` arm never raises (a probe must not decide the
    /// diagnostic — the real scan that follows does, with the right
    /// state), and the loop arms skip their own probe (the answer to "does
    /// this body contain an await" does not depend on the back-edge rule,
    /// and skipping keeps a nest of `d` loops linear instead of `2^d`).
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

/// Can `body` reach a suspension? Replays the ordinary scan in `probe`
/// mode, which cannot fail, so the `Err` arm is genuinely unreachable
/// rather than swallowed.
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

/// Shared by the `While` and `For` arms: model the loop's back edge
/// before walking the body (this fn's own `CrossAwaitScan` doc comment).
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
            // A post-await rebind replaces whatever spanned; subsequent
            // field access is on the new value, which did not span.
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
                // Pattern bindings introduced once an await is already
                // in view (including `match await ...: case .Ok(x):`) do
                // not span; bindings entered before any await still can.
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
            // The loop variable is rebound by the header on every
            // iteration, *before* the body runs — so it never spans the
            // back edge, and it belongs in the exemption set even when
            // `enter_loop_body` just cleared it. An `await` inside the
            // body still clears it again, which is right: past that
            // suspension this iteration's binding does span.
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
        // A `defer` body runs at cleanup time, not inline in the forward
        // sequence this scan tracks — 02-language.md §10 already forbids
        // `await` inside one (`scan_defer_forbidden`), so it never itself
        // straddles a suspension.
        TypedStmtKind::Defer(_) => Ok(()),
        TypedStmtKind::ExprStmt(e) => scan_await_cross_expr(e, state),
        // plans/M6.md item G: a bare `send`'s message arguments are
        // ordinary expressions and obey 02-language.md §9.2 exactly like
        // any other call's.
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
                        // No real `L:C` is available here (decision 1:
                        // the typed tree carries no spans at all) —
                        // `omit_location` (`SemaError`'s own multi-line
                        // exception field, `sema::mod`'s doc comment)
                        // suppresses the misleading `at 0:0` a bare
                        // `SemaError::at` would otherwise print.
                        return Err(SemaError {
                            category: "actor",
                            message: format!(
                                "`{root}`-rooted access cannot span an `await` — only a \
                                 self-rooted path may (02-language.md §9.2)"
                            ),
                            line: 0,
                            col: 0,
                            extra_lines: Vec::new(),
                            omit_location: true,
                            missing_method: None,
                        });
                    }
                }
            }
            scan_await_cross_expr(base, state)
        }
        TypedExprKind::Index(base, idx) => {
            // Same §9.2 rule as Field: an external-rooted nested access
            // (including `input[0]`) must not span `await`. Field was
            // checked; Index only recursed — a bypass.
            if state.seen_await && !state.probe {
                if let Some(root) = root_local_name(e) {
                    if root != "self" && !state.after_await.contains(root) {
                        return Err(SemaError {
                            category: "actor",
                            message: format!(
                                "`{root}`-rooted access cannot span an `await` — only a \
                                 self-rooted path may (02-language.md §9.2)"
                            ),
                            line: 0,
                            col: 0,
                            extra_lines: Vec::new(),
                            omit_location: true,
                            missing_method: None,
                        });
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
        TypedExprKind::Closure { .. } => Ok(()), // a lending call is synchronous (02 §9.2) — never itself spans an await.
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
            // A name bound from an earlier await does not span *that*
            // await, but it does span this one, so the exemption is
            // per-suspension and cannot accumulate.
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

    // --- plans/M6.md item A: the CallError composition table + path-
    // rooting classification (pure logic, unit-tested directly per the
    // item's own instruction) --------------------------------------------

    fn call_error_of(e: &Type) -> Type {
        Type::Named("CallError".to_string(), vec![TypeArg::Type(e.clone())])
    }

    /// The table verbatim (02-language.md §9.4): "declared R -> Result[R,
    /// CallError[never]]".
    #[test]
    fn compose_call_error_wraps_a_plain_declared_type() {
        let composed = compose_call_error(&Type::U64, &[]);
        assert_eq!(
            composed,
            Type::Result(Box::new(Type::U64), Box::new(call_error_of(&Type::Never)))
        );
    }

    /// "declared Result[T, E] -> Result[T, CallError[E]]".
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

    /// `Option`/`Static`/a bare user struct all fall through the same
    /// "declared R" branch as any other non-`Result` type — the table has
    /// only two cases, not one per shape.
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

    /// Applying the table twice must not collapse or double-wrap (a
    /// sanity check that the function is a pure, idempotent-shaped
    /// mapping over its input, not a stateful rewrite).
    #[test]
    fn compose_call_error_is_a_pure_function_of_its_input() {
        let a = compose_call_error(&Type::U64, &[]);
        let b = compose_call_error(&Type::U64, &[]);
        assert_eq!(a, b);
    }

    /// plans/M13.md item H: take-arg types become CallError's second
    /// type argument so `NotAdmitted` patterns bind `(Admission, args)`.
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

    /// `root_local_name` (the cross-await path-rooting classifier,
    /// 02-language.md §9.2): a bare local's own root is itself; a nested
    /// field chain's root is whatever `Local` sits at the bottom,
    /// regardless of chain depth; anything else (a literal, a call) has
    /// no local root at all.
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

    /// `check_cross_await` (02-language.md §9.2): a self-rooted access on
    /// both sides of an `await` is legal; an external-rooted path that
    /// *spans* the await (bound before, field-used after) is rejected;
    /// a local bound *from* the await's result and then field-accessed
    /// is legal — it does not span (03 §3 / plans/M9.md item J2d).
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

        // self-rooted before and after the await: legal.
        let self_ok = vec![
            let_stmt("before", field("self", "cache")),
            let_stmt("suspend", await_node.clone()),
            let_stmt("after", field("self", "cache")),
        ];
        assert!(
            check_cross_await(&self_ok).is_ok(),
            "a self-rooted access spanning an await must be accepted"
        );

        // external-rooted, only *after* the await, never bound after it:
        // rejected (the name is an argument / pre-await local).
        let external_after = vec![
            let_stmt("suspend", await_node.clone()),
            let_stmt("bad", field("input", "value")),
        ];
        assert!(
            check_cross_await(&external_after).is_err(),
            "an external-rooted access after an await must be rejected"
        );

        // external-rooted, but entirely *before* the await: legal (the
        // rule is about spanning the suspension, not about touching an
        // external root at all).
        let external_before = vec![
            let_stmt("fine", field("input", "value")),
            let_stmt("suspend", await_node.clone()),
        ];
        assert!(
            check_cross_await(&external_before).is_ok(),
            "an external-rooted access entirely before an await must be accepted"
        );

        // Bound from the await itself, then field-accessed: legal — the
        // local does not span the suspension (03-hardware.md §3).
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
