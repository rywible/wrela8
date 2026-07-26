//! Match exhaustiveness (plans/M2.md item G): compositional usefulness
//! over closed sums, `bool`, tuples, and fixed arrays; integers and
//! everything unbounded require a wildcard; a wildcard (or any arm) that
//! covers nothing is an error; guarded arms never contribute; `|`
//! alternatives bind the same names at the same types (02-language.md
//! §7.2).
//!
//! Shape: this pass re-walks every body the bodies pass (bodies.rs)
//! already type-checked, looking only for `match` statements and `is`
//! expressions. It needs each scrutinee's resolved type to build a
//! pattern matrix, so it reuses bodies.rs's own typing machinery via
//! `pub(crate)` visibility widenings (no restructuring): `ModuleCtx`/
//! `build_module_ctx`, `FnCtx` and its scope methods, `check_expr` (the
//! "synthesize an expression's type in a local context" entry point),
//! `check_pattern` (pattern typing + binding, reused verbatim so a match
//! arm's or `is`'s bound names land in the re-walked `FnCtx` exactly like
//! the bodies pass left them), `types_eq`, `decl_variant_payload_types`,
//! and `literal_array_len`.
//!
//! The re-walk itself (`check_stmt`/`walk_expr` below) is new code, not a
//! reuse of bodies.rs's own `check_stmt`/`synth_expr`: those fully
//! re-validate every expression (which this pass does not need — bodies
//! already ran) and, more importantly, do not report `match`/`is` sites
//! back to a caller. This file's walk is intentionally the dumb, obvious
//! shape: mirror the statement/expression dispatch just far enough to
//! (a) find every `match`/`is` and (b) keep `FnCtx`'s locals accurate for
//! later scrutinees that reference earlier bindings (assignments, `for`,
//! arm/`is` pattern bindings, closure parameters).
//!
//! One documented gap (closures): a closure's parameter types are only
//! knowable from its call-site's expected `fn` type (bodies.rs's
//! `check_closure`); reproducing that here would mean duplicating the
//! call/struct-literal/argument-checking machinery this item's plan asks
//! us not to. A closure with every parameter explicitly annotated is
//! walked (its param types come straight from `mctx.resolve_type`, real
//! reuse, no call-site context needed); a closure with any unannotated
//! parameter is left unwalked — not an error (bodies.rs already checked
//! it), just unchecked for nested `match`/`is` by this pass, exactly like
//! item H's generic bodies are unchecked by bodies.rs itself.

use std::collections::BTreeMap;

use crate::sema::SemaError;
use crate::sema::bodies::{self, FnCtx, ModuleCtx};
use crate::sema::types::{self, Type};
use crate::syntax::ast::{
    self, AssignStmt, ClosureBody, ClosureExpr, DeferBody, Expr, ForStmt, IfStmt, Item, MatchStmt,
    Member, Module, Pattern, Span, Stmt, UnaryOp, WhileStmt,
};

fn match_error(message: String, span: Span) -> SemaError {
    SemaError::at("match", message, span)
}

// --- entry point: mirrors bodies::check's own top-level walk -------------

/// The exhaustiveness pass (plans/M2.md item G): runs after `bodies`
/// (mod.rs's `check`), one module-wide re-walk exactly like `bodies::check`
/// itself. `mctx` is shared with every other pass (built once by
/// `mod.rs::check`) so item H's instantiation queue accumulates across
/// all of them (see `bodies.rs`'s doc comment on `InstKind`).
pub(crate) fn check(
    module: &Module,
    decl_items: &[types::DeclItem],
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();
    for (ai, di) in ast_items.iter().zip(decl_items.iter()) {
        match (ai, di) {
            (Item::Const(c), types::DeclItem::Const(_)) => {
                let mut fctx = FnCtx::new(Type::Unit, mctx.module_pools.clone());
                walk_expr(&c.value, &mut fctx, mctx)?;
            }
            (Item::Fn(f), types::DeclItem::Fn(d)) => check_top_fn(f, d, mctx)?,
            (Item::Struct(s), types::DeclItem::Struct(_)) => check_struct_bodies(s, mctx)?,
            _ => {}
        }
    }
    Ok(())
}

/// `pub(crate)` (item H, generics.rs): re-run over a substituted,
/// generics-cleared copy of a generic fn's own ast+decl, mirroring
/// `bodies::check_top_fn`'s own widening.
pub(crate) fn check_top_fn(
    f: &ast::FnItem,
    d: &types::DeclFn,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    // A generic fn's body is item H's territory, unchecked here exactly
    // like bodies.rs skips it; an `@image` fn always fails closed in the
    // bodies pass, so this point is never reached for one (mod.rs's
    // `check` is fail-fast: bodies runs, and returns, before matches
    // does).
    if !f.generics.is_empty() {
        return Ok(());
    }
    let mut fctx = FnCtx::new(d.ret.clone(), mctx.module_pools.clone());
    // Plans/M6.md item A: this pass re-derives its own `FnCtx` per body
    // (module doc) rather than sharing `bodies::check`'s own — `in_async`
    // must be set here too, exactly like `bodies::check_top_fn`, so this
    // pass's own `bodies::check_expr` re-invocations (`check_assign`'s own
    // type-inference call, `check_match_stmt`'s scrutinee) see the same
    // gate bodies.rs already enforced.
    fctx.in_async = f.is_async;
    walk_params_with_defaults(&f.params, &d.params, &mut fctx, mctx)?;
    if let Some(body) = &f.body {
        check_stmts(body, &mut fctx, mctx)?;
    }
    Ok(())
}

fn walk_params_with_defaults(
    ast_params: &[ast::Param],
    decl_params: &[types::DeclParam],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    for (ap, dp) in ast_params.iter().zip(decl_params.iter()) {
        fctx.insert_local(dp.name.clone(), dp.ty.clone());
        if let Some(def) = &ap.default {
            walk_expr(def, fctx, mctx)?;
        }
    }
    Ok(())
}

fn check_struct_bodies(s: &ast::StructItem, mctx: &ModuleCtx) -> Result<(), SemaError> {
    if !s.generics.is_empty() {
        return Ok(()); // generic struct: item H's job, not checked here.
    }
    let info = mctx.structs.get(&s.name).expect("struct present in mctx");
    let self_ty = Type::Named(s.name.clone(), vec![]);
    check_struct_members(info, self_ty, mctx)
}

/// `pub(crate)` (item H, generics.rs): the guts of `check_struct_bodies`,
/// pulled out to take a `StructInfo` (and its `self_ty`) directly — see
/// `bodies::check_struct_members`'s own doc comment.
pub(crate) fn check_struct_members(
    info: &bodies::StructInfo,
    self_ty: Type,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let local_pools = bodies::local_pool_names(info);
    for (am, dm) in info.members() {
        match (am, dm) {
            (Member::Field(af), types::DeclMember::Field(_)) => {
                if let Some(def) = &af.default {
                    let mut fctx = FnCtx::new(Type::Unit, local_pools.clone());
                    fctx.insert_local("self".to_string(), self_ty.clone());
                    walk_expr(def, &mut fctx, mctx)?;
                }
            }
            (Member::Fn(f), types::DeclMember::Fn(fd)) => {
                if !f.generics.is_empty() {
                    continue; // generic method: item H's job.
                }
                let mut fctx = FnCtx::new(fd.ret.clone(), local_pools.clone());
                fctx.in_async = f.is_async; // mirrors bodies::check_struct_members
                fctx.insert_local("self".to_string(), self_ty.clone());
                walk_params_with_defaults(&f.params, &fd.params, &mut fctx, mctx)?;
                if let Some(body) = &f.body {
                    check_stmts(body, &mut fctx, mctx)?;
                }
            }
            (Member::Init(i), types::DeclMember::Init(fd)) => {
                let mut fctx = FnCtx::new(fd.ret.clone(), local_pools.clone());
                fctx.insert_local("self".to_string(), self_ty.clone());
                walk_params_with_defaults(&i.params, &fd.params, &mut fctx, mctx)?;
                check_stmts(&i.body, &mut fctx, mctx)?;
            }
            _ => {}
        }
    }
    Ok(())
}

// --- statement re-walk ----------------------------------------------------

fn check_stmts(stmts: &[Stmt], fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    for s in stmts {
        check_stmt(s, fctx, mctx)?;
    }
    Ok(())
}

fn check_stmt(stmt: &Stmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    match stmt {
        Stmt::Assign(a) => check_assign(a, fctx, mctx),
        Stmt::If(i) => check_if(i, fctx, mctx),
        Stmt::Match(m) => check_match_stmt(m, fctx, mctx),
        Stmt::For(f) => check_for(f, fctx, mctx),
        Stmt::While(w) => check_while(w, fctx, mctx),
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => Ok(()),
        Stmt::Return(_, e) => match e {
            Some(expr) => walk_expr(expr, fctx, mctx),
            None => Ok(()),
        },
        Stmt::Assert(a) => {
            walk_expr(&a.cond, fctx, mctx)?;
            if let Some(msg) = &a.message {
                walk_expr(msg, fctx, mctx)?;
            }
            Ok(())
        }
        Stmt::Defer(d) => match &d.body {
            DeferBody::Expr(e) => walk_expr(e, fctx, mctx),
            DeferBody::Suite(stmts) => check_stmts(stmts, fctx, mctx),
        },
        // Plans/M6.md item A: `with group(...) [as g]:` now type-checks
        // (bodies.rs lifted the fail-closed) — its body must still be
        // walked for nested `match`/`is`, exactly like every other block
        // construct here; `g`'s own binding is re-derived in this pass's
        // own `fctx` the same way `bodies::check_with` bound it (a
        // `match`/`is` scrutinee inside the block may reference it, e.g.
        // `g.join_all()`'s own receiver). `comptime if` is eliminated by
        // `sema::specialize` before this pass (or any pass after
        // `declare`) ever runs (plans/M3.md item D) — reaching one here
        // would mean `specialize` left one behind. Nothing to walk there.
        Stmt::With(w) => {
            walk_expr(&w.expr, fctx, mctx)?;
            bodies::scoped(fctx, |fctx| {
                if let Some(name) = &w.as_name {
                    fctx.insert_local(name.clone(), Type::Named("Group".to_string(), vec![]));
                    // Mirrors `bodies::check_with`'s own seeding — see
                    // `bodies::compute_group_children`'s own doc comment
                    // for why this must be a pure re-computation rather
                    // than shared mutable state.
                    if let Some(children) =
                        bodies::compute_group_children(&w.body, name, fctx, mctx)?
                    {
                        fctx.group_children.insert(name.clone(), children);
                    }
                }
                check_stmts(&w.body, fctx, mctx)
            })
        }
        // plans/M6.md item G: a bare `send` statement types now (its
        // legality is `sema::send_proof`'s whole-image question, decided
        // after this pass), so its message arguments must be walked for
        // a nested `match`/`is` exactly like any other call's.
        Stmt::Send(_, e) => walk_expr(e, fctx, mctx),
        Stmt::ComptimeIf(_) => Ok(()),
        // `comptime assert`'s condition/message are ordinary typed
        // expressions (decision 8 lifted the fail-closed) — walked here
        // exactly like a plain `assert`'s in case either embeds a nested
        // `match`/`is` this pass must still find.
        Stmt::ComptimeAssert(_, cond, message) => {
            walk_expr(cond, fctx, mctx)?;
            if let Some(msg) = message {
                walk_expr(msg, fctx, mctx)?;
            }
            Ok(())
        }
        Stmt::Expr(_, e) => walk_expr(e, fctx, mctx),
    }
}

/// One `fctx` scope per branch (`bodies::scoped`) — mirrors
/// `bodies::check_if` exactly, so this pass's own re-walk keeps a typed
/// declaration inside one branch from leaking into a sibling or past the
/// whole `if` (err-mwir-if-else-scope-leak).
fn check_if(i: &IfStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    walk_expr(&i.cond, fctx, mctx)?;
    bodies::scoped(fctx, |fctx| check_stmts(&i.then_branch, fctx, mctx))?;
    for elif in &i.elifs {
        walk_expr(&elif.cond, fctx, mctx)?;
        bodies::scoped(fctx, |fctx| check_stmts(&elif.body, fctx, mctx))?;
    }
    if let Some(b) = &i.else_branch {
        bodies::scoped(fctx, |fctx| check_stmts(b, fctx, mctx))?;
    }
    Ok(())
}

fn check_while(w: &WhileStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    walk_expr(&w.cond, fctx, mctx)?;
    bodies::scoped(fctx, |fctx| check_stmts(&w.body, fctx, mctx))
}

/// Mirrors `bodies::check_for`'s own elem-type derivation (range vs fixed
/// array, `take`-consumed iterable unwrapped the same way, a bare numeric
/// literal endpoint deferring to its concrete sibling) purely to keep
/// `fctx`'s locals accurate for a `for` binding used as a later
/// scrutinee; typing itself is `bodies::check_expr` reused, not
/// reimplemented.
fn check_for(f: &ForStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    walk_expr(&f.iterable, fctx, mctx)?;
    let raw_iterable: &Expr = match &f.iterable {
        Expr::Unary(_, UnaryOp::Take, inner) => inner.as_ref(),
        other => other,
    };
    let elem_ty = match raw_iterable {
        Expr::Range(_, from, to, _incl) => {
            let first =
                if bodies::is_bare_numeric_literal(from) && !bodies::is_bare_numeric_literal(to) {
                    to.as_ref()
                } else {
                    from.as_ref()
                };
            bodies::check_expr(first, None, fctx, mctx)?.ty
        }
        other => match bodies::check_expr(other, None, fctx, mctx)?.ty {
            Type::Array(elem, _) => *elem,
            other_ty => other_ty, // unreachable given bodies already validated `for`.
        },
    };
    // Loop binding + body share one scope, same as `bodies::check_for`.
    bodies::scoped(fctx, |fctx| {
        fctx.insert_local(f.name.clone(), elem_ty);
        check_stmts(&f.body, fctx, mctx)
    })
}

/// Mirrors `bodies::check_assign`'s own scoping exactly (see its doc
/// comment): a typed declaration only reuses a same-block binding
/// (`lookup_innermost`); a plain assignment reuses one from any scope
/// still open (`lookup_local`), the arm-merge idiom (02-language.md
/// §8.1).
fn check_assign(a: &AssignStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    walk_expr(&a.value, fctx, mctx)?;
    if let Expr::Name(_, name) = &a.target {
        let already_bound = if a.ty.is_some() {
            fctx.lookup_innermost(name).is_some()
        } else {
            fctx.lookup_local(name).is_some()
        };
        if already_bound {
            return Ok(()); // reassignment/compound-assign: type unchanged.
        }
        // A fresh local (bodies already validated this can only be a
        // plain `=`, never a compound assign).
        let ty = match &a.ty {
            Some(ann) => mctx.resolve_type(ann, &fctx.local_pools)?,
            None => bodies::check_expr(&a.value, None, fctx, mctx)?.ty,
        };
        fctx.insert_local(name.clone(), ty);
        Ok(())
    } else {
        walk_expr(&a.target, fctx, mctx)
    }
}

// --- `match` and `is` ------------------------------------------------------

/// One flattened pattern row: `Ctor(i, sub)` covers a value under the
/// `i`-th constructor of the column's type (a closed enum's `i`-th
/// variant in declaration order; the sole constructor of `bool`'s `true`
/// (0)/`false` (1), `unit` (0), a tuple, or a fixed array) together with
/// its sub-pattern rows for that constructor's fields; `Wild` covers
/// every value of the column's type; `Opaque` is one otherwise-unmodeled
/// point (an integer/`char`/string/etc. literal) — plans/M2.md item G:
/// "integers, chars, strings, and anything unbounded require a wildcard
/// or binding arm", so `Opaque` never contributes to full coverage on its
/// own, only `Wild` does.
#[derive(Clone, Debug)]
enum RPat {
    Wild,
    Ctor(usize, Vec<RPat>),
    Opaque,
}

/// The column type's shape for the exhaustiveness matrix (plans/M2.md
/// item G, decision 4: dumb, no unification) — every M2 scrutinee type
/// reduces to one of these five buckets.
enum TyShape {
    Bool,
    Unit,
    /// A closed sum's variants in declaration order: name (unused by the
    /// matrix itself, only by witness rendering) + payload types.
    Sum(Vec<(String, Vec<Type>)>),
    Tuple(Vec<Type>),
    /// A fixed array with a literal length, expanded to that many copies
    /// of the element type (component-wise, plans/M2.md item G).
    Array(Vec<Type>),
    /// Integers, `char`, `Str`/`Static`/`Bytes`, non-literal-length
    /// arrays, `fn` values, `own`/generic types, and anything else this
    /// pass does not enumerate constructors for — requires an explicit
    /// wildcard/binding arm, exactly like an unbounded scalar.
    Opaque,
}

fn shape_of(ty: &Type, mctx: &ModuleCtx) -> TyShape {
    match ty {
        Type::Bool => TyShape::Bool,
        Type::Unit => TyShape::Unit,
        Type::Option(inner) => TyShape::Sum(vec![
            ("Some".to_string(), vec![(**inner).clone()]),
            ("None".to_string(), vec![]),
        ]),
        Type::Result(ok, err) => TyShape::Sum(vec![
            ("Ok".to_string(), vec![(**ok).clone()]),
            ("Err".to_string(), vec![(**err).clone()]),
        ]),
        // `CallError[E]` (plans/M6.md item A): a fixed five-variant sum,
        // mirroring `bodies::variant_payload_types_for`'s own new arm —
        // exhaustiveness over a `match`ed `CallError` needs the identical
        // fixed shape that pass uses to type each arm's payload.
        Type::Named(name, targs) if name == "CallError" => {
            let e_ty = match targs.first() {
                Some(types::TypeArg::Type(t)) => t.clone(),
                _ => Type::Never,
            };
            TyShape::Sum(vec![
                ("Op".to_string(), vec![e_ty]),
                ("Cancelled".to_string(), vec![]),
                ("DeadlineExceeded".to_string(), vec![]),
                (
                    "NotAdmitted".to_string(),
                    vec![Type::Named("Admission".to_string(), vec![])],
                ),
                (
                    "PeerFailed".to_string(),
                    vec![Type::Named("Peer".to_string(), vec![])],
                ),
            ])
        }
        // plans/M8.md item G / plans/M9.md item I: an auto-visible stdlib
        // enum (`CompletionOutcome` and the rest of `stdlib_enums`) is a
        // closed, fieldless sum that may have no `DeclEnum` behind it —
        // so exhaustiveness over `CompletionOutcome` is checked by exactly
        // the machinery every module-declared enum gets, and a missing
        // `Unknown` arm is reported by the same message.
        Type::Named(name, targs)
            if targs.is_empty() && crate::sema::stdlib_enums::is_auto_visible(name) =>
        {
            // plans/M9.md item QQ: `shape_of` returns `TyShape`, not
            // `Result` — threading fallibility through usefulness /
            // flatten / render would rewrite the whole match algorithm.
            // `stdlib_enums::prepare` runs at every check entry before
            // `matches::check`, so a corrupt stdlib already diagnosed
            // there; this expect is "prepare held", not "ignore errors".
            TyShape::Sum(
                crate::sema::stdlib_enums::variant_strs(name)
                    .expect("stdlib_enums::prepare runs before matches")
                    .expect("guarded by is_auto_visible")
                    .iter()
                    .map(|v| ((*v).to_string(), Vec::new()))
                    .collect(),
            )
        }
        Type::Named(name, targs) if targs.is_empty() => match mctx.enums.get(name) {
            Some(e) => TyShape::Sum(
                e.variants
                    .iter()
                    .map(|v| (v.name.clone(), bodies::decl_variant_payload_types(v)))
                    .collect(),
            ),
            // A struct scrutinee (or an unresolvable name — cannot arise,
            // bodies already validated it) is never matched by anything
            // but a wildcard/binding pattern; treat it as opaque.
            None => TyShape::Opaque,
        },
        // plans/M9.md item J2c: a generic-instantiated user enum
        // (`Lookup[u32]`) is an ordinary closed sum once substituted —
        // the same `instantiate_enum` pattern typing already calls. The
        // old fall-through treated every non-empty `targs` Named as
        // `Opaque`, so a fully-covered match falsely demanded `_`.
        Type::Named(name, targs) if !targs.is_empty() => match mctx.enums.get(name) {
            Some(_) => match crate::sema::generics::instantiate_enum(
                mctx,
                name,
                targs,
                Span { line: 0, col: 0 },
            ) {
                Ok(decl) => TyShape::Sum(
                    decl.variants
                        .iter()
                        .map(|v| (v.name.clone(), bodies::decl_variant_payload_types(v)))
                        .collect(),
                ),
                // Bodies already accepted this scrutinee type; a failure
                // here would be a producer bug. Stay opaque rather than
                // panic — exhaustiveness then demands `_` (fail closed).
                Err(_) => TyShape::Opaque,
            },
            None => TyShape::Opaque,
        },
        Type::Tuple(elems) => TyShape::Tuple(elems.clone()),
        Type::Array(elem, len_expr) => match bodies::literal_array_len(len_expr) {
            Some(n) if n >= 0 => TyShape::Array(vec![(**elem).clone(); n as usize]),
            _ => TyShape::Opaque,
        },
        _ => TyShape::Opaque,
    }
}

/// This shape's constructors: `(index, sub-field types)` in declaration
/// order. Empty for `TyShape::Opaque` (no enumerable constructors).
fn ctor_infos(shape: &TyShape) -> Vec<(usize, Vec<Type>)> {
    match shape {
        TyShape::Bool => vec![(0, vec![]), (1, vec![])],
        TyShape::Unit => vec![(0, vec![])],
        TyShape::Sum(vs) => vs
            .iter()
            .enumerate()
            .map(|(i, (_, p))| (i, p.clone()))
            .collect(),
        TyShape::Tuple(elems) => vec![(0, elems.clone())],
        TyShape::Array(elems) => vec![(0, elems.clone())],
        TyShape::Opaque => vec![],
    }
}

fn all_wild(n: usize) -> Vec<RPat> {
    vec![RPat::Wild; n]
}

/// Flattens one pattern (already validated by `bodies::check_pattern`
/// against `ty`) into every alternative row `|` produces — a plain
/// pattern always yields exactly one row; nesting fans out by cross
/// product (a `Some(.A | .B)` yields two rows, one per alternative).
fn flatten_pattern(p: &Pattern, ty: &Type, mctx: &ModuleCtx) -> Vec<RPat> {
    match p {
        Pattern::Wildcard(_) | Pattern::Binding(_, _) => vec![RPat::Wild],
        Pattern::Take(_, inner) => flatten_pattern(inner, ty, mctx),
        Pattern::Or(_, alts) => alts
            .iter()
            .flat_map(|alt| flatten_pattern(alt, ty, mctx))
            .collect(),
        Pattern::Literal(_, expr) => match (shape_of(ty, mctx), expr) {
            (TyShape::Bool, Expr::Bool(_, b)) => {
                vec![RPat::Ctor(if *b { 0 } else { 1 }, vec![])]
            }
            _ => vec![RPat::Opaque],
        },
        Pattern::Variant {
            variant, payload, ..
        } => {
            let TyShape::Sum(variants) = shape_of(ty, mctx) else {
                return vec![RPat::Opaque]; // unreachable: already type-checked.
            };
            let Some(idx) = variants.iter().position(|(n, _)| n == variant) else {
                return vec![RPat::Opaque]; // unreachable: already type-checked.
            };
            let payload_tys = variants[idx].1.clone();
            flatten_children(payload, &payload_tys, mctx)
                .into_iter()
                .map(|c| RPat::Ctor(idx, c))
                .collect()
        }
        Pattern::Tuple(_, items) => {
            let Type::Tuple(elem_tys) = ty else {
                return vec![RPat::Opaque]; // unreachable: already type-checked.
            };
            flatten_children(items, elem_tys, mctx)
                .into_iter()
                .map(|c| RPat::Ctor(0, c))
                .collect()
        }
        Pattern::Array(_, items) => {
            let Type::Array(elem, len_expr) = ty else {
                return vec![RPat::Opaque]; // unreachable: already type-checked.
            };
            match bodies::literal_array_len(len_expr) {
                Some(n) if n >= 0 => {
                    let elem_tys: Vec<Type> = std::iter::repeat((**elem).clone())
                        .take(n as usize)
                        .collect();
                    flatten_children(items, &elem_tys, mctx)
                        .into_iter()
                        .map(|c| RPat::Ctor(0, c))
                        .collect()
                }
                _ => vec![RPat::Opaque],
            }
        }
    }
}

/// Cross product of each child pattern's own alternatives, one child per
/// `tys` entry (component-wise, plans/M2.md item G).
fn flatten_children(items: &[Pattern], tys: &[Type], mctx: &ModuleCtx) -> Vec<Vec<RPat>> {
    let mut combos: Vec<Vec<RPat>> = vec![vec![]];
    for (item, ty) in items.iter().zip(tys.iter()) {
        let alts = flatten_pattern(item, ty, mctx);
        let mut next = Vec::with_capacity(combos.len() * alts.len().max(1));
        for prefix in &combos {
            for a in &alts {
                let mut v = prefix.clone();
                v.push(a.clone());
                next.push(v);
            }
        }
        combos = next;
    }
    combos
}

/// One arm's rows, each tagged with the span to report if that row turns
/// out useless: the arm pattern's own span in the common case, or —
/// since flattening loses which top-level `|` alternative produced a row
/// — each alternative's own span when the arm pattern is itself an `Or`.
fn arm_rows(pattern: &Pattern, ty: &Type, mctx: &ModuleCtx) -> Vec<(Span, RPat)> {
    match pattern {
        Pattern::Or(_, alts) => alts
            .iter()
            .flat_map(|alt| {
                let span = alt.span();
                flatten_pattern(alt, ty, mctx)
                    .into_iter()
                    .map(move |r| (span, r))
                    .collect::<Vec<_>>()
            })
            .collect(),
        _ => {
            let span = pattern.span();
            flatten_pattern(pattern, ty, mctx)
                .into_iter()
                .map(move |r| (span, r))
                .collect()
        }
    }
}

/// Specializes `matrix`'s first column on constructor `idx` of arity
/// `arity`: a matching `Ctor` row contributes its sub-rows; a `Wild` row
/// contributes `arity` wildcards (it covers every constructor); anything
/// else (a different constructor, or `Opaque`) contributes nothing.
fn specialize(matrix: &[Vec<RPat>], idx: usize, arity: usize) -> Vec<Vec<RPat>> {
    matrix
        .iter()
        .filter_map(|r| match &r[0] {
            RPat::Ctor(i, sub) if *i == idx => {
                let mut n = sub.clone();
                n.extend(r[1..].iter().cloned());
                Some(n)
            }
            RPat::Wild => {
                let mut n = all_wild(arity);
                n.extend(r[1..].iter().cloned());
                Some(n)
            }
            _ => None,
        })
        .collect()
}

/// Is `row` (over `tys`, left to right) useful against `matrix` — is
/// there a value `row` matches that no row of `matrix` already matches?
/// Returns a concrete witness row when it is (plain recursive usefulness
/// checking over a pattern matrix, plans/M2.md item G: "no optimization
/// beyond what correctness needs" — this is the textbook Maranget
/// algorithm, generalized only enough to also carry a witness, with
/// `TyShape::Opaque` standing in for the unbounded types it says never
/// get literal-value reasoning).
fn usefulness(
    tys: &[Type],
    row: &[RPat],
    matrix: &[Vec<RPat>],
    mctx: &ModuleCtx,
) -> Option<Vec<RPat>> {
    if tys.is_empty() {
        return if matrix.is_empty() {
            Some(vec![])
        } else {
            None
        };
    }
    let ty0 = &tys[0];
    let rest_tys = &tys[1..];
    let shape = shape_of(ty0, mctx);
    if matches!(shape, TyShape::Opaque) {
        return usefulness_opaque(row, rest_tys, matrix, mctx);
    }
    let infos = ctor_infos(&shape);
    match &row[0] {
        RPat::Wild => {
            for (idx, sub_tys) in &infos {
                let arity = sub_tys.len();
                let specialized = specialize(matrix, *idx, arity);
                if specialized.is_empty() {
                    let mut w = vec![RPat::Ctor(*idx, all_wild(arity))];
                    w.extend(all_wild(rest_tys.len()));
                    return Some(w);
                }
                let mut combined_tys = sub_tys.clone();
                combined_tys.extend(rest_tys.iter().cloned());
                let mut combined_row = all_wild(arity);
                combined_row.extend(row[1..].iter().cloned());
                if let Some(sub_w) = usefulness(&combined_tys, &combined_row, &specialized, mctx) {
                    let (payload_w, rest_w) = sub_w.split_at(arity);
                    let mut w = vec![RPat::Ctor(*idx, payload_w.to_vec())];
                    w.extend(rest_w.iter().cloned());
                    return Some(w);
                }
            }
            None
        }
        RPat::Ctor(idx, sub) => {
            let arity = sub.len();
            let specialized = specialize(matrix, *idx, arity);
            let sub_tys = infos
                .iter()
                .find(|(i, _)| i == idx)
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            let mut combined_tys = sub_tys;
            combined_tys.extend(rest_tys.iter().cloned());
            let mut combined_row = sub.clone();
            combined_row.extend(row[1..].iter().cloned());
            usefulness(&combined_tys, &combined_row, &specialized, mctx).map(|sub_w| {
                let (payload_w, rest_w) = sub_w.split_at(arity);
                let mut w = vec![RPat::Ctor(*idx, payload_w.to_vec())];
                w.extend(rest_w.iter().cloned());
                w
            })
        }
        RPat::Opaque => unreachable!("Opaque rows only arise for Opaque-shaped columns"),
    }
}

/// The `TyShape::Opaque` half of `usefulness`: only a `Wild` row can ever
/// fully cover this column (plans/M2.md item G — no literal-value
/// tracking for integers/`char`/strings/etc.), so a bare literal/`Opaque`
/// row at this column is always useful unless the matrix already carries
/// a `Wild` here.
fn usefulness_opaque(
    row: &[RPat],
    rest_tys: &[Type],
    matrix: &[Vec<RPat>],
    mctx: &ModuleCtx,
) -> Option<Vec<RPat>> {
    let has_wild = matrix.iter().any(|r| matches!(r[0], RPat::Wild));
    if !has_wild {
        let mut w = vec![row[0].clone()];
        w.extend(all_wild(rest_tys.len()));
        return Some(w);
    }
    let default: Vec<Vec<RPat>> = matrix
        .iter()
        .filter(|r| matches!(r[0], RPat::Wild))
        .map(|r| r[1..].to_vec())
        .collect();
    usefulness(rest_tys, &row[1..], &default, mctx).map(|mut sub_w| {
        let mut w = vec![row[0].clone()];
        w.append(&mut sub_w);
        w
    })
}

fn row_useful(ty: &Type, row: &RPat, covered: &[RPat], mctx: &ModuleCtx) -> bool {
    let matrix: Vec<Vec<RPat>> = covered.iter().map(|r| vec![r.clone()]).collect();
    usefulness(
        std::slice::from_ref(ty),
        std::slice::from_ref(row),
        &matrix,
        mctx,
    )
    .is_some()
}

/// One concrete uncovered pattern for `ty` given `covered`, first in
/// declaration order (plans/M2.md item G: "deterministic witness choice
/// — first uncovered in declaration order"), or `None` if `covered` is
/// already exhaustive.
fn first_uncovered(ty: &Type, covered: &[RPat], mctx: &ModuleCtx) -> Option<RPat> {
    let matrix: Vec<Vec<RPat>> = covered.iter().map(|r| vec![r.clone()]).collect();
    usefulness(std::slice::from_ref(ty), &[RPat::Wild], &matrix, mctx)
        .map(|w| w.into_iter().next().expect("one-column witness"))
}

fn render_witness(w: &RPat, ty: &Type, mctx: &ModuleCtx) -> String {
    match w {
        RPat::Wild | RPat::Opaque => "_".to_string(),
        RPat::Ctor(idx, sub) => match shape_of(ty, mctx) {
            TyShape::Bool => if *idx == 0 { "true" } else { "false" }.to_string(),
            TyShape::Unit => "_".to_string(),
            TyShape::Sum(vs) => {
                let (name, payload_tys) = &vs[*idx];
                if payload_tys.is_empty() {
                    format!(".{name}")
                } else {
                    let parts: Vec<String> = sub
                        .iter()
                        .zip(payload_tys.iter())
                        .map(|(s, t)| render_witness(s, t, mctx))
                        .collect();
                    format!(".{name}({})", parts.join(", "))
                }
            }
            TyShape::Tuple(elems) => {
                let parts: Vec<String> = sub
                    .iter()
                    .zip(elems.iter())
                    .map(|(s, t)| render_witness(s, t, mctx))
                    .collect();
                format!("({})", parts.join(", "))
            }
            TyShape::Array(elems) => {
                let parts: Vec<String> = sub
                    .iter()
                    .zip(elems.iter())
                    .map(|(s, t)| render_witness(s, t, mctx))
                    .collect();
                format!("[{}]", parts.join(", "))
            }
            TyShape::Opaque => "_".to_string(),
        },
    }
}

// --- `|` alternative binding consistency (02-language.md §7.2) -----------

fn pattern_bindings(p: &Pattern, ty: &Type, mctx: &ModuleCtx) -> BTreeMap<String, Type> {
    let mut out = BTreeMap::new();
    collect_bindings(p, ty, mctx, &mut out);
    out
}

fn collect_bindings(p: &Pattern, ty: &Type, mctx: &ModuleCtx, out: &mut BTreeMap<String, Type>) {
    match p {
        Pattern::Wildcard(_) | Pattern::Literal(_, _) => {}
        Pattern::Binding(_, name) => {
            out.insert(name.clone(), ty.clone());
        }
        Pattern::Take(_, inner) => collect_bindings(inner, ty, mctx, out),
        // An `Or`'s own alternatives are validated to bind identically by
        // `check_or_consistency` before this ever runs on one of them
        // from an enclosing position, so any alternative's bindings
        // stand in for the whole `Or` here.
        Pattern::Or(_, alts) => {
            if let Some(first) = alts.first() {
                collect_bindings(first, ty, mctx, out);
            }
        }
        Pattern::Variant {
            variant, payload, ..
        } => {
            if let TyShape::Sum(variants) = shape_of(ty, mctx) {
                if let Some((_, payload_tys)) = variants.iter().find(|(n, _)| n == variant) {
                    for (sp, spty) in payload.iter().zip(payload_tys.iter()) {
                        collect_bindings(sp, spty, mctx, out);
                    }
                }
            }
        }
        Pattern::Tuple(_, items) => {
            if let Type::Tuple(elems) = ty {
                for (i, t) in items.iter().zip(elems.iter()) {
                    collect_bindings(i, t, mctx, out);
                }
            }
        }
        Pattern::Array(_, items) => {
            if let Type::Array(elem, _) = ty {
                for i in items {
                    collect_bindings(i, elem, mctx, out);
                }
            }
        }
    }
}

fn bindings_eq(a: &BTreeMap<String, Type>, b: &BTreeMap<String, Type>) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|(k, v)| b.get(k).is_some_and(|v2| bodies::types_eq(v, v2)))
}

/// Walks the whole pattern tree (mirroring `collect_bindings`'s shape)
/// checking every `Or` node it finds — at any nesting depth — for
/// consistent bindings across its alternatives (02-language.md §7.2:
/// "same bindings, same types").
fn check_or_consistency(p: &Pattern, ty: &Type, mctx: &ModuleCtx) -> Result<(), SemaError> {
    match p {
        Pattern::Wildcard(_) | Pattern::Literal(_, _) | Pattern::Binding(_, _) => Ok(()),
        Pattern::Take(_, inner) => check_or_consistency(inner, ty, mctx),
        Pattern::Or(_, alts) => {
            let mut iter = alts.iter();
            let Some(first) = iter.next() else {
                return Ok(());
            };
            check_or_consistency(first, ty, mctx)?;
            let first_bindings = pattern_bindings(first, ty, mctx);
            for alt in iter {
                check_or_consistency(alt, ty, mctx)?;
                let b = pattern_bindings(alt, ty, mctx);
                if !bindings_eq(&b, &first_bindings) {
                    return Err(match_error(
                        "`|` alternatives must bind the same names at the same types".to_string(),
                        alt.span(),
                    ));
                }
            }
            Ok(())
        }
        Pattern::Variant {
            variant, payload, ..
        } => {
            if let TyShape::Sum(variants) = shape_of(ty, mctx) {
                if let Some((_, payload_tys)) = variants.iter().find(|(n, _)| n == variant) {
                    for (sp, spty) in payload.iter().zip(payload_tys.iter()) {
                        check_or_consistency(sp, spty, mctx)?;
                    }
                }
            }
            Ok(())
        }
        Pattern::Tuple(_, items) => {
            if let Type::Tuple(elems) = ty {
                for (i, t) in items.iter().zip(elems.iter()) {
                    check_or_consistency(i, t, mctx)?;
                }
            }
            Ok(())
        }
        Pattern::Array(_, items) => {
            if let Type::Array(elem, _) = ty {
                for i in items {
                    check_or_consistency(i, elem, mctx)?;
                }
            }
            Ok(())
        }
    }
}

fn check_match_stmt(m: &MatchStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    walk_expr(&m.scrutinee, fctx, mctx)?;
    let sty = bodies::check_expr(&m.scrutinee, None, fctx, mctx)?.ty;
    let mut covered: Vec<RPat> = Vec::new();
    for arm in &m.arms {
        // One `fctx` scope per arm (`bodies::scoped`), covering the
        // pattern's own bindings too — mirrors `bodies::check_match`
        // exactly, so a binding from one arm's pattern never leaks into
        // a sibling arm or past the whole `match`.
        bodies::scoped(fctx, |fctx| {
            // Bind (and re-validate, harmlessly) the pattern first,
            // exactly like the bodies pass's own `check_match` — this
            // also means any pattern this pass cannot itself reason
            // about safely (a generic instantiation) has already failed
            // closed via `bodies::check_pattern` before
            // `flatten_pattern`/`shape_of` below ever look at it.
            bodies::check_pattern(&arm.pattern, &sty, fctx, mctx)?;
            if let Some(guard) = &arm.guard {
                walk_expr(guard, fctx, mctx)?;
            }
            check_stmts(&arm.body, fctx, mctx)
        })?;
        check_or_consistency(&arm.pattern, &sty, mctx)?;
        if arm.guard.is_none() {
            for (span, row) in arm_rows(&arm.pattern, &sty, mctx) {
                if !row_useful(&sty, &row, &covered, mctx) {
                    return Err(match_error("unreachable arm".to_string(), span));
                }
                covered.push(row);
            }
        }
    }
    if let Some(witness) = first_uncovered(&sty, &covered, mctx) {
        return Err(match_error(
            format!(
                "match is not exhaustive: missing {}",
                render_witness(&witness, &sty, mctx)
            ),
            m.span,
        ));
    }
    Ok(())
}

/// `is`'s uselessness check (plans/M2.md item G): a lone pattern is
/// always useful against nothing, so there is nothing to check unless it
/// is itself an `Or` — then each alternative is checked against the ones
/// before it, exactly like match arms are (02-language.md §7.2: "`is` is
/// a refutable test", no exhaustiveness demand, but "the uselessness
/// check applies").
fn check_is_pattern(pattern: &Pattern, sty: &Type, mctx: &ModuleCtx) -> Result<(), SemaError> {
    check_or_consistency(pattern, sty, mctx)?;
    if let Pattern::Or(_, alts) = pattern {
        let mut covered: Vec<RPat> = Vec::new();
        for alt in alts {
            for row in flatten_pattern(alt, sty, mctx) {
                if !row_useful(sty, &row, &covered, mctx) {
                    return Err(match_error("unreachable arm".to_string(), alt.span()));
                }
                covered.push(row);
            }
        }
    }
    Ok(())
}

// --- expression re-walk: finds `is` and recurses into closures -----------

fn walk_expr(e: &Expr, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    match e {
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::BStr(..)
        | Expr::Char(..)
        | Expr::Bool(..)
        | Expr::Unit(_)
        | Expr::Name(..) => Ok(()),
        Expr::FStr(f) => {
            let desugared = crate::sema::fstring::desugar_fstring(f)?;
            walk_expr(&desugared, fctx, mctx)
        }
        Expr::Field(base, _, _) => walk_expr(base, fctx, mctx),
        Expr::Index(base, _, args) => {
            walk_expr(base, fctx, mctx)?;
            for a in args {
                walk_expr(a, fctx, mctx)?;
            }
            Ok(())
        }
        Expr::Call(callee, _, args) => {
            walk_expr(callee, fctx, mctx)?;
            for a in args {
                walk_expr(&a.value, fctx, mctx)?;
            }
            Ok(())
        }
        Expr::Unary(_, _, inner) => walk_expr(inner, fctx, mctx),
        Expr::Try(_, inner) => walk_expr(inner, fctx, mctx),
        Expr::Binary(_, _, l, r) => {
            walk_expr(l, fctx, mctx)?;
            walk_expr(r, fctx, mctx)
        }
        Expr::Range(_, from, to, _) => {
            walk_expr(from, fctx, mctx)?;
            walk_expr(to, fctx, mctx)
        }
        Expr::Is(_, scrutinee, pattern) => {
            walk_expr(scrutinee, fctx, mctx)?;
            let sty = bodies::check_expr(scrutinee, None, fctx, mctx)?.ty;
            bodies::check_pattern(pattern, &sty, fctx, mctx)?;
            check_is_pattern(pattern, &sty, mctx)
        }
        Expr::Not(_, inner) => walk_expr(inner, fctx, mctx),
        Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            walk_expr(l, fctx, mctx)?;
            walk_expr(r, fctx, mctx)
        }
        Expr::DotVariant(_, _, args) => {
            for a in args {
                walk_expr(&a.value, fctx, mctx)?;
            }
            Ok(())
        }
        Expr::Closure(c) => walk_closure(c, fctx, mctx),
        Expr::Send(_, inner) => walk_expr(inner, fctx, mctx),
        Expr::Tuple(_, items) | Expr::List(_, items) => {
            for i in items {
                walk_expr(i, fctx, mctx)?;
            }
            Ok(())
        }
        Expr::ArrayRepeat(_, elem, count) => {
            walk_expr(elem, fctx, mctx)?;
            walk_expr(count, fctx, mctx)
        }
    }
}

/// See the module doc comment's "documented gap": a closure is only
/// re-walked when every parameter carries an explicit annotation (its
/// type then comes straight from `mctx.resolve_type`, real reuse); a
/// closure with any unannotated parameter is left unwalked.
fn walk_closure(c: &ClosureExpr, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<(), SemaError> {
    if c.params.iter().any(|p| p.ty.is_none()) {
        return Ok(());
    }
    fctx.push_scope();
    let result = (|| {
        for p in &c.params {
            let ty = mctx.resolve_type(p.ty.as_ref().expect("checked above"), &fctx.local_pools)?;
            fctx.insert_local(p.name.clone(), ty);
        }
        match &c.body {
            ClosureBody::Expr(e) => walk_expr(e, fctx, mctx),
            ClosureBody::Suite(stmts) => check_stmts(stmts, fctx, mctx),
        }
    })();
    fctx.pop_scope();
    result
}
