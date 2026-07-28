//! Match exhaustiveness (plans/M2.md item G): compositional usefulness
//! over closed sums, `bool`, tuples, and fixed arrays; integers and
//! everything unbounded require a wildcard; a wildcard (or any arm) that
//! covers nothing is an error; guarded arms never contribute; `|`
//! alternatives bind the same names at the same types (02-language.md
//! §7.2).
//!
//! Walks the typed tree produced by `bodies` — scrutinee types and
//! patterns come from `TypedExpr`/`TypedPattern`; no `FnCtx` rebuild and
//! no `bodies::check_expr` re-invocation.

use std::collections::BTreeMap;

use crate::sema::SemaError;
use crate::sema::bodies::{self, ModuleCtx};
use crate::sema::typed::{
    TypedClosureBody, TypedExpr, TypedExprKind, TypedFn, TypedForIter, TypedMatchArm, TypedPattern,
    TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind, TypedStruct,
};
use crate::sema::types::Type;
use crate::syntax::ast::Span;

fn match_error(message: String, span: Span) -> SemaError {
    SemaError::at("match", message, span)
}

/// Exhaustiveness over a finished `TypedProgram`.
pub(crate) fn check(program: &TypedProgram, mctx: &ModuleCtx) -> Result<(), SemaError> {
    for c in program.consts.values() {
        walk_expr(&c.value, mctx)?;
    }
    for f in program.fns.values() {
        check_fn(f, mctx)?;
    }
    for s in program.structs.values() {
        check_struct(s, mctx)?;
    }
    for e in program.enums.values() {
        for f in e.methods.values().chain(e.assoc_fns.values()) {
            check_fn(f, mctx)?;
        }
    }
    Ok(())
}

/// Instantiation path: check one typed fn body.
pub(crate) fn check_fn(f: &TypedFn, mctx: &ModuleCtx) -> Result<(), SemaError> {
    for p in &f.params {
        if let Some(d) = &p.default {
            walk_expr(d, mctx)?;
        }
    }
    walk_stmts(&f.body, mctx)
}

pub(crate) fn check_struct(s: &TypedStruct, mctx: &ModuleCtx) -> Result<(), SemaError> {
    for d in s.field_defaults.values() {
        walk_expr(d, mctx)?;
    }
    for f in s.methods.values().chain(s.assoc_fns.values()) {
        check_fn(f, mctx)?;
    }
    if let Some(f) = &s.init {
        check_fn(f, mctx)?;
    }
    Ok(())
}

fn walk_stmts(stmts: &[TypedStmt], mctx: &ModuleCtx) -> Result<(), SemaError> {
    for s in stmts {
        walk_stmt(s, mctx)?;
    }
    Ok(())
}

fn walk_stmt(stmt: &TypedStmt, mctx: &ModuleCtx) -> Result<(), SemaError> {
    match &stmt.kind {
        TypedStmtKind::Let { value, .. } => walk_expr(value, mctx),
        TypedStmtKind::Assign { target, value } => {
            walk_expr(target, mctx)?;
            walk_expr(value, mctx)
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            walk_expr(cond, mctx)?;
            walk_stmts(then_branch, mctx)?;
            for e in elifs {
                walk_expr(&e.cond, mctx)?;
                walk_stmts(&e.body, mctx)?;
            }
            if let Some(b) = else_branch {
                walk_stmts(b, mctx)?;
            }
            Ok(())
        }
        TypedStmtKind::Match { scrutinee, arms } => check_match(scrutinee, arms, mctx),
        TypedStmtKind::For { iter, body, .. } => {
            match iter {
                TypedForIter::Range(a, b, _) => {
                    walk_expr(a, mctx)?;
                    walk_expr(b, mctx)?;
                }
                TypedForIter::Expr(e) => walk_expr(e, mctx)?,
            }
            walk_stmts(body, mctx)
        }
        TypedStmtKind::While { cond, body, .. } => {
            walk_expr(cond, mctx)?;
            walk_stmts(body, mctx)
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => Ok(()),
        TypedStmtKind::Return(v) => match v {
            Some(e) => walk_expr(e, mctx),
            None => Ok(()),
        },
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            walk_expr(cond, mctx)?;
            if let Some(m) = message {
                walk_expr(m, mctx)?;
            }
            Ok(())
        }
        TypedStmtKind::Defer(body) => match body {
            crate::sema::typed::TypedDeferBody::Expr(e) => walk_expr(e, mctx),
            crate::sema::typed::TypedDeferBody::Suite(stmts) => walk_stmts(stmts, mctx),
        },
        TypedStmtKind::ExprStmt(e) | TypedStmtKind::BareSend { expr: e, .. } => walk_expr(e, mctx),
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                walk_expr(c, mctx)?;
            }
            if let Some(d) = deadline {
                walk_expr(d, mctx)?;
            }
            walk_stmts(body, mctx)
        }
    }
}

fn walk_expr(e: &TypedExpr, mctx: &ModuleCtx) -> Result<(), SemaError> {
    match &e.kind {
        TypedExprKind::Is(scrutinee, pat) => check_is(scrutinee, pat, mctx),
        TypedExprKind::Field(base, _)
        | TypedExprKind::ToScalar(base)
        | TypedExprKind::Neg(base)
        | TypedExprKind::BitNot(base)
        | TypedExprKind::Take(base)
        | TypedExprKind::Not(base)
        | TypedExprKind::Panic(base)
        | TypedExprKind::Await(base)
        | TypedExprKind::Send(base)
        | TypedExprKind::Try(base, _) => walk_expr(base, mctx),
        TypedExprKind::Index(base, idx) => {
            walk_expr(base, mctx)?;
            walk_expr(idx, mctx)
        }
        TypedExprKind::Call { receiver, args, .. } => {
            if let Some(r) = receiver {
                walk_expr(r, mctx)?;
            }
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                walk_expr(a, mctx)?;
            }
            Ok(())
        }
        TypedExprKind::CallValue(callee, args) => {
            walk_expr(callee, mctx)?;
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                walk_expr(a, mctx)?;
            }
            Ok(())
        }
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::OpCall(_, l, r)
        | TypedExprKind::And(l, r)
        | TypedExprKind::Or(l, r) => {
            walk_expr(l, mctx)?;
            walk_expr(r, mctx)
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args.iter().filter_map(|a| a.value.as_ref()) {
                walk_expr(a, mctx)?;
            }
            Ok(())
        }
        TypedExprKind::Closure { body, .. } => match body {
            TypedClosureBody::Expr(e) => walk_expr(e, mctx),
            TypedClosureBody::Suite(stmts) => walk_stmts(stmts, mctx),
        },
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                walk_expr(i, mctx)?;
            }
            Ok(())
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                walk_expr(v, mctx)?;
            }
            Ok(())
        }
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                walk_expr(r, mctx)?;
            }
            for (_, a) in args {
                walk_expr(a, mctx)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn check_match(
    scrutinee: &TypedExpr,
    arms: &[TypedMatchArm],
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    walk_expr(scrutinee, mctx)?;
    let sty = &scrutinee.ty;
    let mut covered: Vec<RPat> = Vec::new();
    for arm in arms {
        check_or_consistency(&arm.pattern, sty, mctx)?;
        if let Some(g) = &arm.guard {
            walk_expr(g, mctx)?;
        }
        walk_stmts(&arm.body, mctx)?;
        if arm.guard.is_some() {
            continue; // guarded arms never contribute
        }
        for (span, row) in arm_rows(&arm.pattern, sty, mctx) {
            if !row_useful(sty, &row, &covered, mctx) {
                return Err(match_error("unreachable arm".to_string(), span));
            }
            covered.push(row);
        }
    }
    if let Some(w) = first_uncovered(sty, &covered, mctx) {
        return Err(match_error(
            format!(
                "match is not exhaustive: missing {}",
                render_witness(&w, sty, mctx)
            ),
            scrutinee.span,
        ));
    }
    Ok(())
}

fn check_is(scrutinee: &TypedExpr, pat: &TypedPattern, mctx: &ModuleCtx) -> Result<(), SemaError> {
    walk_expr(scrutinee, mctx)?;
    let sty = &scrutinee.ty;
    check_or_consistency(pat, sty, mctx)?;
    if let TypedPatternKind::Or(alts) = &pat.kind {
        let mut covered: Vec<RPat> = Vec::new();
        for alt in alts {
            for (span, row) in arm_rows(alt, sty, mctx) {
                if !row_useful(sty, &row, &covered, mctx) {
                    return Err(match_error("unreachable arm".to_string(), span));
                }
                covered.push(row);
            }
        }
    }
    Ok(())
}

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
        Type::Tuple(elems) => TyShape::Tuple(elems.clone()),
        Type::Array(elem, len_expr) => match bodies::literal_array_len(len_expr) {
            Some(n) if n >= 0 => TyShape::Array(vec![(**elem).clone(); n as usize]),
            _ => TyShape::Opaque,
        },
        // Closed sums (Option/Result/CallError/AUTO_VISIBLE/user enums,
        // including generic instantiate) share one table — `sum_ctors`.
        // plans/M9.md item QQ: `shape_of` returns `TyShape`, not `Result`;
        // `stdlib_enums::prepare` runs at every check entry before
        // `matches::check`, so a corrupt stdlib already diagnosed there —
        // auto-visible expects Ok after prepare; other Err → Opaque
        // (bodies already accepted the scrutinee).
        other => {
            let auto_visible = matches!(
                other,
                Type::Named(name, targs)
                    if targs.is_empty() && crate::sema::stdlib_enums::is_auto_visible(name)
            );
            if auto_visible {
                TyShape::Sum(
                    crate::sema::sum::sum_ctors(other, mctx)
                        .expect("stdlib_enums::prepare runs before matches"),
                )
            } else {
                match crate::sema::sum::sum_ctors(other, mctx) {
                    Ok(ctors) => TyShape::Sum(ctors),
                    Err(_) => TyShape::Opaque,
                }
            }
        }
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
fn flatten_pattern(p: &TypedPattern, ty: &Type, mctx: &ModuleCtx) -> Vec<RPat> {
    match &p.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => vec![RPat::Wild],
        TypedPatternKind::Take(inner) => flatten_pattern(inner, ty, mctx),
        TypedPatternKind::Or(alts) => alts
            .iter()
            .flat_map(|alt| flatten_pattern(alt, ty, mctx))
            .collect(),
        TypedPatternKind::Literal(expr) => match (shape_of(ty, mctx), &expr.kind) {
            (TyShape::Bool, TypedExprKind::Bool(b)) => {
                vec![RPat::Ctor(if *b { 0 } else { 1 }, vec![])]
            }
            _ => vec![RPat::Opaque],
        },
        TypedPatternKind::Variant {
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
        TypedPatternKind::Tuple(items) => {
            let Type::Tuple(elem_tys) = ty else {
                return vec![RPat::Opaque]; // unreachable: already type-checked.
            };
            flatten_children(items, elem_tys, mctx)
                .into_iter()
                .map(|c| RPat::Ctor(0, c))
                .collect()
        }
        TypedPatternKind::Array(items) => {
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
fn flatten_children(items: &[TypedPattern], tys: &[Type], mctx: &ModuleCtx) -> Vec<Vec<RPat>> {
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
fn arm_rows(pattern: &TypedPattern, ty: &Type, mctx: &ModuleCtx) -> Vec<(Span, RPat)> {
    match &pattern.kind {
        TypedPatternKind::Or(alts) => alts
            .iter()
            .flat_map(|alt| {
                let span = alt.span;
                flatten_pattern(alt, ty, mctx)
                    .into_iter()
                    .map(move |r| (span, r))
                    .collect::<Vec<_>>()
            })
            .collect(),
        _ => {
            let span = pattern.span;
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

fn pattern_bindings(p: &TypedPattern, ty: &Type, mctx: &ModuleCtx) -> BTreeMap<String, Type> {
    let mut out = BTreeMap::new();
    collect_bindings(p, ty, mctx, &mut out);
    out
}

fn collect_bindings(
    p: &TypedPattern,
    ty: &Type,
    mctx: &ModuleCtx,
    out: &mut BTreeMap<String, Type>,
) {
    match &p.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Literal(_) => {}
        TypedPatternKind::Binding(name) => {
            out.insert(name.clone(), ty.clone());
        }
        TypedPatternKind::Take(inner) => collect_bindings(inner, ty, mctx, out),
        TypedPatternKind::Or(alts) => {
            if let Some(first) = alts.first() {
                collect_bindings(first, ty, mctx, out);
            }
        }
        TypedPatternKind::Variant {
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
        TypedPatternKind::Tuple(items) => {
            if let Type::Tuple(elems) = ty {
                for (i, t) in items.iter().zip(elems.iter()) {
                    collect_bindings(i, t, mctx, out);
                }
            }
        }
        TypedPatternKind::Array(items) => {
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
fn check_or_consistency(p: &TypedPattern, ty: &Type, mctx: &ModuleCtx) -> Result<(), SemaError> {
    match &p.kind {
        TypedPatternKind::Wildcard
        | TypedPatternKind::Literal(_)
        | TypedPatternKind::Binding(_) => Ok(()),
        TypedPatternKind::Take(inner) => check_or_consistency(inner, ty, mctx),
        TypedPatternKind::Or(alts) => {
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
                        alt.span,
                    ));
                }
            }
            Ok(())
        }
        TypedPatternKind::Variant {
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
        TypedPatternKind::Tuple(items) => {
            if let Type::Tuple(elems) = ty {
                for (i, t) in items.iter().zip(elems.iter()) {
                    check_or_consistency(i, t, mctx)?;
                }
            }
            Ok(())
        }
        TypedPatternKind::Array(items) => {
            if let Type::Array(elem, _) = ty {
                for i in items {
                    check_or_consistency(i, elem, mctx)?;
                }
            }
            Ok(())
        }
    }
}
