//! The handoff calling convention (plans/M7.md item E3, 03-hardware.md §5).
//!
//! "Any public synchronous `@driver` method with exactly one `take p: P`
//! parameter and result `Receipt[P]` receives the handoff calling
//! convention" — no annotation; the signature is fully determining.
//! Verified here; displayed by the check dump (`handoff fn ...`).

use crate::sema::SemaError;
use crate::sema::bodies::{self, ModuleCtx};
use crate::sema::types::{self, DeclFn, DeclMember, DeclStruct, Type, TypeArg};
use crate::syntax::ast::{AccessMode, Expr, Item, Member, Module, Span, Stmt};

/// Is `f` the handoff signature on a `@driver` (03 §5)?
pub(crate) fn is_handoff_signature(f: &DeclFn) -> bool {
    let Some(r) = &f.receiver else {
        return false;
    };
    if !r.is_pub || f.is_async {
        return false;
    }
    let takes: Vec<_> = f
        .params
        .iter()
        .filter(|p| p.mode == AccessMode::Take)
        .collect();
    if takes.len() != 1 {
        return false;
    }
    let Type::Named(n, args) = &f.ret else {
        return false;
    };
    if n != "Receipt" {
        return false;
    }
    let Some(TypeArg::Type(p)) = args.first() else {
        return false;
    };
    bodies::types_eq(p, &takes[0].ty)
}

/// A `@driver` method that returns `Receipt[_]` but is not handoff-shaped
/// is a named rejection — the signature determines the convention, so a
/// near-miss is not silently an ordinary method.
fn receipt_return_payload(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Named(n, args) if n == "Receipt" => match args.first() {
            Some(TypeArg::Type(p)) => Some(p),
            _ => None,
        },
        _ => None,
    }
}

fn handoff_shape_error(f: &DeclFn, span: Span) -> SemaError {
    SemaError::at(
        "type",
        format!(
            "`{}` returns `{}` but is not the handoff calling convention \
             (03-hardware.md §5): a public synchronous `@driver` method with \
             exactly one `take p: P` parameter and result `Receipt[P]` — found {}",
            f.name,
            types::render_type(&f.ret),
            handoff_shape_summary(f)
        ),
        span,
    )
}

fn handoff_shape_summary(f: &DeclFn) -> String {
    let mut parts = Vec::new();
    if f.is_async {
        parts.push("async".to_string());
    }
    match &f.receiver {
        Some(r) if !r.is_pub => parts.push("not `pub`".to_string()),
        None => parts.push("no receiver".to_string()),
        _ => {}
    }
    let takes = f
        .params
        .iter()
        .filter(|p| p.mode == AccessMode::Take)
        .count();
    if takes != 1 {
        parts.push(format!("{takes} `take` parameter(s)"));
    } else if let Some(p) = f.params.iter().find(|p| p.mode == AccessMode::Take) {
        if let Some(ret_p) = receipt_return_payload(&f.ret) {
            if !bodies::types_eq(ret_p, &p.ty) {
                parts.push(format!(
                    "`take {}` is `{}` but `Receipt` brands `{}`",
                    p.name,
                    types::render_type(&p.ty),
                    types::render_type(ret_p)
                ));
            }
        }
    }
    if parts.is_empty() {
        "an unrecognized near-miss".to_string()
    } else {
        parts.join(", ")
    }
}

/// `self.queue.publish(...)` / `self.queue.reject(...)` — the only legal
/// producer transitions on a handoff path (03 §5).
fn is_producer_transition(e: &Expr) -> bool {
    let Expr::Call(callee, _, _) = e else {
        return false;
    };
    let Expr::Field(_, _, name) = callee.as_ref() else {
        return false;
    };
    name == "publish" || name == "reject"
}

fn check_returns_are_producer(stmts: &[Stmt]) -> Result<(), SemaError> {
    for stmt in stmts {
        match stmt {
            Stmt::Return(rspan, Some(e)) => {
                if !is_producer_transition(e) {
                    return Err(SemaError::at(
                        "type",
                        "a handoff method's every path must `return queue.publish(...)` or \
                         `return queue.reject(...)` (03-hardware.md §5); this `return` is not \
                         a producer transition"
                            .to_string(),
                        *rspan,
                    ));
                }
            }
            Stmt::Return(rspan, None) => {
                return Err(SemaError::at(
                    "type",
                    "a handoff method's every path must `return queue.publish(...)` or \
                     `return queue.reject(...)` (03-hardware.md §5)"
                        .to_string(),
                    *rspan,
                ));
            }
            Stmt::If(i) => {
                check_returns_are_producer(&i.then_branch)?;
                for elif in &i.elifs {
                    check_returns_are_producer(&elif.body)?;
                }
                if let Some(body) = &i.else_branch {
                    check_returns_are_producer(body)?;
                }
            }
            Stmt::Match(m) => {
                for arm in &m.arms {
                    check_returns_are_producer(&arm.body)?;
                }
            }
            Stmt::While(w) => check_returns_are_producer(&w.body)?,
            Stmt::For(f) => check_returns_are_producer(&f.body)?,
            _ => {}
        }
    }
    Ok(())
}

/// Module-wide handoff check: signature shape + producer-transition body.
/// (Fallthrough/`missing return` is already `flow`'s job for a non-`unit`
/// result — this pass only insists every `return` is `publish`/`reject`.)
pub(crate) fn check(
    module: &Module,
    decl_items: &[types::DeclItem],
    _mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();
    for (ai, di) in ast_items.iter().zip(decl_items.iter()) {
        let (Item::Struct(s), types::DeclItem::Struct(d)) = (ai, di) else {
            continue;
        };
        if !d.is_driver {
            for (am, dm) in s.members.iter().zip(d.members.iter()) {
                if let (Member::Fn(f), DeclMember::Fn(fd)) = (am, dm) {
                    if receipt_return_payload(&fd.ret).is_some() {
                        return Err(SemaError::at(
                            "type",
                            format!(
                                "`{}` returns `{}` but is not a `@driver` method — the handoff \
                                 calling convention (03-hardware.md §5) is `@driver`-only",
                                f.name,
                                types::render_type(&fd.ret)
                            ),
                            f.span,
                        ));
                    }
                }
            }
            continue;
        }
        for (am, dm) in s.members.iter().zip(d.members.iter()) {
            let (Member::Fn(f), DeclMember::Fn(fd)) = (am, dm) else {
                continue;
            };
            if receipt_return_payload(&fd.ret).is_none() {
                continue;
            }
            if !is_handoff_signature(fd) {
                return Err(handoff_shape_error(fd, f.span));
            }
            let Some(body) = &f.body else {
                return Err(SemaError::at(
                    "type",
                    format!(
                        "handoff method `{}` needs a body that `return`s `publish`/`reject` \
                         (03-hardware.md §5)",
                        f.name
                    ),
                    f.span,
                ));
            };
            check_returns_are_producer(body)?;
        }
    }
    Ok(())
}

/// Check-dump marker: `handoff fn ...` for a driver method whose
/// signature is fully determining (03 §5: "displayed by tooling").
pub(crate) fn handoff_dump_prefix(owner: &DeclStruct, f: &DeclFn) -> &'static str {
    if owner.is_driver && is_handoff_signature(f) {
        "handoff "
    } else {
        ""
    }
}
