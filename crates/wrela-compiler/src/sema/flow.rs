//! One per-body CFG: definite initialization, moves, and exclusivity
//! (plans/M2.md items E/F, one CFG/one pass file per decision 3).
//! Definite init tracks initialization state per storage path (paths.rs)
//! on every control-flow edge (02-language.md §3.2); moves track `take`
//! deinitialization and use-after-take (§3, §3.1); exclusivity forbids
//! overlapping `mut`/read while a `mut` is active (§3, §8.2). Flips
//! `values.data.copies-implicitly`, `values.resource.move-spells-take`,
//! `values.exclusivity.no-overlap`.
//!
//! Shape: this pass runs after `access` (mirroring/read-loans/receiver
//! mutability already proven) and before `matches` (exhaustiveness not
//! yet proven — see the note on `walk_match` below). It re-walks every
//! body exactly like `bodies.rs`/`access.rs`/`matches.rs` do, reusing
//! `bodies::ModuleCtx`/`StructInfo`/`FnCtx` and `access::EffectMap`/
//! `resolve_receiver_mode` wholesale (the established `pub(crate)` reuse
//! pattern — nothing in an earlier pass is restructured).
//!
//! CFG shape: rather than materializing an explicit graph, each
//! statement-list walk (`walk_block`) is a direct structural
//! interpreter over the (loop-free, structured) AST — `if`/`match`
//! clone the incoming state once per arm and *meet* the arms' resulting
//! states back together (a name is initialized after the join only when
//! every non-diverging inbound edge initialized it — `return`/`break`/
//! `continue` drop out of the merge, matching plans/M2.md item 2
//! verbatim). `for`/`while` bodies are re-run (their own real,
//! error-raising walk, not a silent shadow pass) from a candidate entry
//! state that starts as the pre-loop state and is replaced, each
//! iteration, by the *meet* of the pre-loop state's own back-edges (its
//! own fallthrough plus every `continue`) — repeated until the candidate
//! stops changing or a small fixed cap is hit (`LOOP_FIXED_POINT_CAP`,
//! `MAX_GENERIC_DEPTH`'s sibling: dumb, fixed, no measurement). A `take`
//! inside the loop body of something initialized before the loop and
//! never restored is therefore caught on the *second* real pass (the
//! first pass's own back-edge state already carries the un-restored
//! `Moved` state into the second run, which then rejects the very same
//! `take`/read that broke the invariant) rather than needing a separate
//! non-erroring shadow interpreter.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::SemaError;
use crate::sema::access::{self, EffectMap};
use crate::sema::bodies::{self, FnCtx, ModuleCtx, StructInfo};
use crate::sema::paths::{PathStep, StoragePath, const_index_value, render_path};
use crate::sema::types::{self, Type};
use crate::syntax::ast::{
    self, AccessMode, Arg, AssignOp, AssignStmt, ClosureBody, ClosureExpr, DeferBody, DeferStmt,
    Expr, ForStmt, IfStmt, Item, MatchStmt, Member, Module, Pattern, Span, Stmt, UnaryOp,
    WhileStmt,
};

/// Dumb, fixed cap on loop fixed-point iteration (mirrors
/// `bodies::MAX_GENERIC_DEPTH`'s "no measurement, no configuration"
/// shape) — every loop the M2 corpus expresses converges in 1-2 real
/// passes; a program that somehow doesn't stabilize within this cap just
/// gets checked against the last candidate reached, rather than looping
/// forever.
const LOOP_FIXED_POINT_CAP: usize = 4;

// --- per-path initialization/move state ------------------------------------

/// One storage path's state (deliverable 2): `Uninit` (never assigned —
/// an ordinary local before its first assignment, or one of `init`'s own
/// fields, 02-language.md §7.1), `Init` (holds a valid value), `Moved`
/// (taken from and not since restored). Only resources and (per decision
/// 9) data-after-`take` ever become `Moved`; only an ordinary local
/// before its first write, or an `init`'s own not-yet-assigned field, is
/// ever `Uninit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathState {
    Uninit,
    Init,
    Moved,
}

/// One `defer`'s registration stack, shared for the whole function/
/// method/`init` body walk (deliverable: 02-language.md §10's "verifies
/// the places it names are valid at every exit"): a true call stack —
/// `walk_block` pushes each `Stmt::Defer` it meets onto this as it
/// processes the block's own statements in order, and pops back to its
/// own entry length before returning, so a nested block's own defers
/// never leak past that block's own exit (the docs' "against the
/// enclosing block"). `return`/`?` check against the *whole* stack (they
/// exit the entire function); `break`/`continue` check only the slice
/// registered since the nearest enclosing loop was entered (`loop_marker`
/// below) — defers registered outside the loop stay pending for a later,
/// real exit.
type DStack<'a> = Vec<&'a DeferStmt>;

/// Re-validates every currently active `defer` at one real exit — a
/// block's own normal completion, a `return`, a `?`, or a `break`/
/// `continue` crossing the enclosing loop (02-language.md §10: "its
/// accesses activate when it *runs*, not when it is registered ... the
/// compiler verifies the places it names are valid at every exit").
/// `active` is already sliced to exactly the defers live at this exit,
/// oldest-registered first (registration order); they run in *reverse*
/// registration order at cleanup, so this walks them back-to-front,
/// threading one running clone of `exit_state` through each one's own
/// body-walk in turn — reusing the very same `walk_expr`/`walk_block`
/// machinery that checks an ordinary statement, exactly like the
/// snapshot this replaces used to at registration time only. Threading
/// (rather than re-cloning `exit_state` for each) means a later-
/// registered/earlier-run defer's own effects (a `take`, say) are visible
/// to whichever earlier-registered defer is checked next
/// (`err-defer-taken-by-defer`). Never mutates the real, still-
/// propagating state passed by the caller — the deferred action doesn't
/// actually run here, only its accesses are validated against a
/// hypothetical replay of cleanup order.
fn check_active_defers<'a>(
    active: &[&'a DeferStmt],
    exit_desc: &str,
    exit_state: &StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
) -> Result<(), SemaError> {
    let mut cur = exit_state.clone();
    for d in active.iter().rev() {
        let mut scratch: DStack<'a> = Vec::new();
        let result = match &d.body {
            DeferBody::Expr(e) => walk_expr(e, &mut cur, fctx, wctx, &mut scratch, 0),
            DeferBody::Suite(stmts) => {
                walk_block(stmts, &mut cur, fctx, wctx, &mut scratch, 0).map(|_| ())
            }
        };
        result.map_err(|mut err| {
            // No raw `line:col` text here (unlike the primary message):
            // `sema.check.roundtrip-stable`'s oracle compares diagnostics
            // with spans erased (reformatting shifts every span), and
            // this extra line isn't one of the ` at <path>:<line>` tails
            // that oracle already knows to strip (sema/generics.rs's
            // chain format) — embedding a bare `L:C` here would make an
            // otherwise identical diagnostic disagree after a pretty/
            // reparse round trip.
            err.extra_lines.push(format!(
                "  this `defer` must be valid at every exit, including {exit_desc}"
            ));
            err
        })?;
    }
    Ok(())
}

/// The state map a walk threads through one function/method/init body:
/// sparse — a path absent from the map inherits its closest recorded
/// ancestor's state (`state_of`), or `Uninit` if no ancestor was ever
/// recorded (ordinary locals start absent; every parameter/`self` root
/// is seeded explicitly at entry, so this default only ever applies to
/// genuinely new bindings).
type StateMap = BTreeMap<StoragePath, PathState>;

fn state_of(path: &StoragePath, state: &StateMap) -> PathState {
    for len in (0..=path.steps.len()).rev() {
        let candidate = path.prefix(len);
        if let Some(s) = state.get(&candidate) {
            return *s;
        }
    }
    PathState::Uninit
}

/// The first (deterministic — `BTreeMap`'s own sorted order) tracked
/// entry at or under `prefix` that isn't `Init` — the "is this whole
/// value fully restored" query the field-take-and-restore obligation and
/// the "self/param used whole" checks both need (02-language.md §3.2,
/// §7.1).
fn first_bad_under(prefix: &StoragePath, state: &StateMap) -> Option<(StoragePath, PathState)> {
    state
        .iter()
        .find(|(p, s)| p.starts_with(prefix) && **s != PathState::Init)
        .map(|(p, s)| (p.clone(), *s))
}

/// The lattice meet used to combine two control-flow edges' states for
/// the same path (deliverable 2's "a name is initialized after a join
/// only if initialized on every inbound edge"): agreement wins outright;
/// disagreement always resolves to the *not safely usable* state — `Init`
/// only survives meeting itself, `Uninit` beats `Init` (never assigned on
/// some edge), and any other disagreement (either edge `Moved`) becomes
/// `Moved` (taken on some edge).
fn meet(a: PathState, b: PathState) -> PathState {
    if a == b {
        return a;
    }
    match (a, b) {
        (PathState::Init, other) | (other, PathState::Init) => other,
        _ => PathState::Moved,
    }
}

fn meet_two(a: &StateMap, b: &StateMap) -> StateMap {
    let mut keys: BTreeSet<StoragePath> = a.keys().cloned().collect();
    keys.extend(b.keys().cloned());
    let mut out = StateMap::new();
    for k in keys {
        out.insert(k.clone(), meet(state_of(&k, a), state_of(&k, b)));
    }
    out
}

fn meet_all(maps: Vec<StateMap>) -> StateMap {
    let mut iter = maps.into_iter();
    let Some(first) = iter.next() else {
        return StateMap::new();
    };
    let mut acc = first;
    for m in iter {
        acc = meet_two(&acc, &m);
    }
    acc
}

/// Records `new_state` at exactly `path`, dropping every stale
/// descendant entry (a fresh assignment/take of `path` supersedes
/// whatever was separately tracked about its own fields/indices).
fn set_state(path: &StoragePath, state: &mut StateMap, new_state: PathState) {
    let stale: Vec<StoragePath> = state
        .keys()
        .filter(|k| *k != path && k.starts_with(path))
        .cloned()
        .collect();
    for k in stale {
        state.remove(&k);
    }
    state.insert(path.clone(), new_state);
}

// --- storage paths from place expressions ----------------------------------

/// Reads `expr` as a storage path (paths.rs) if it is one (a name, or a
/// field/single-index chain rooted at one) — `None` for anything else (a
/// call, a literal, a binary expression, a parenthesized-away take
/// wrapper never reaching here, ...). Mirrors `access.rs`'s own
/// `is_full_place`, restated to build the path rather than a bool.
///
/// A root name only counts as a storage path when it is a *tracked*
/// local (a parameter, `self`, or an ordinary assigned/pattern-bound
/// local) — `fctx` decides that. Every other bare name (a builtin
/// prelude constructor like `None`/`Some`/`Ok`/`Err`/`panic`, a
/// module-level fn/const referenced as a value, a struct/enum type name)
/// is not a storage location at all, so it is never `Uninit`/`Moved` and
/// never needs a "readable" check — treated as a fresh value instead,
/// exactly like a call result or a literal.
fn as_path(expr: &Expr, fctx: &FnCtx) -> Option<StoragePath> {
    match expr {
        Expr::Name(_, name) => {
            if fctx.lookup_local(name).is_some() {
                Some(StoragePath::root(name.clone()))
            } else {
                None
            }
        }
        Expr::Field(base, _, name) => Some(as_path(base, fctx)?.field(name.clone())),
        Expr::Index(base, _, args) => {
            if args.len() != 1 {
                return None;
            }
            let step = match const_index_value(&args[0]) {
                Some(v) => PathStep::Index(v),
                None => PathStep::RuntimeIndex,
            };
            Some(as_path(base, fctx)?.index(step))
        }
        _ => None,
    }
}

/// Walks the nested sub-expressions of a place chain that themselves
/// need checking (an index argument's own expression — `arr[compute()]`
/// — everything else in a place chain is just names/field labels with
/// nothing further to walk).
fn walk_place_subexprs<'a>(
    expr: &'a Expr,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    match expr {
        Expr::Field(base, _, _) => {
            walk_place_subexprs(base, state, fctx, wctx, dstack, loop_marker)
        }
        Expr::Index(base, _, args) => {
            walk_place_subexprs(base, state, fctx, wctx, dstack, loop_marker)?;
            for a in args {
                walk_expr(a, state, fctx, wctx, dstack, loop_marker)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A lightweight, best-effort place typing (mirrors `access.rs`'s own
/// `name_ty`/`check_field`: this pass does not re-derive full inference,
/// only enough to answer "is this place a resource" for the implicit-copy
/// check). `own[P] T` unwraps to `T` (calling through a pool handle
/// derives access to the payload, 02-language.md §4).
fn place_type(expr: &Expr, fctx: &FnCtx, mctx: &ModuleCtx) -> Option<Type> {
    match expr {
        Expr::Name(_, name) => fctx
            .lookup_local(name)
            .or_else(|| mctx.consts.get(name).cloned()),
        Expr::Field(base, _, name) => {
            let base_ty = bodies::unwrap_own(place_type(base, fctx, mctx)?);
            if let Type::Named(sname, targs) = &base_ty {
                let s = mctx.structs.get(sname)?;
                let field_ty = s.field_ty(name)?;
                if targs.is_empty() {
                    return Some(field_ty);
                }
                // A field accessed through an instantiated generic
                // struct (`Box[DmaBlock].item`): the struct table only
                // ever holds the *unsubstituted* declaration, so the
                // field's own type must be substituted here using this
                // use site's concrete type arguments — otherwise a
                // resource classification hiding behind a generic field
                // (the implicit-copy and overwrite-live checks below,
                // both keyed on `place_type`) would silently see `None`
                // and never fire (crate::sema::generics's own
                // `subst_field_type`, widened `pub(crate)` for exactly
                // this reuse, decision 10).
                return Some(crate::sema::generics::subst_field_type(
                    &field_ty,
                    &s.decl.generics,
                    targs,
                ));
            }
            None
        }
        Expr::Index(base, _, args) => {
            if args.len() != 1 {
                return None;
            }
            match bodies::unwrap_own(place_type(base, fctx, mctx)?) {
                Type::Array(elem, _) => Some(*elem),
                Type::Bytes(_) => Some(Type::U8),
                _ => None,
            }
        }
        _ => None,
    }
}

// --- diagnostics ------------------------------------------------------------

fn init_error(message: String, span: Span) -> SemaError {
    SemaError::at("init", message, span)
}

fn move_error(message: String, span: Span) -> SemaError {
    SemaError::at("move", message, span)
}

fn overlap_error(message: String, span: Span) -> SemaError {
    SemaError::at("overlap", message, span)
}

// --- the per-operand checks: readable / takeable / storing / overwrite ----

/// The state to use when *using* `path`: for an exact root that is a
/// known parameter/`self` (a "whole value use", 02-language.md §3.2/
/// §7.1), any not-fully-restored descendant counts against it (a field
/// left `Moved`/`Uninit` makes the *whole* value unusable); otherwise the
/// path's own ancestor-resolved state.
fn whole_or_state(path: &StoragePath, state: &StateMap, wctx: &WCtx) -> PathState {
    if path.is_root() && wctx.modes.get(&path.root) == Some(&AccessMode::Mut) {
        first_bad_under(path, state)
            .map(|(_, s)| s)
            .unwrap_or(PathState::Init)
    } else {
        state_of(path, state)
    }
}

fn check_readable(
    path: &StoragePath,
    state: &StateMap,
    wctx: &WCtx,
    span: Span,
) -> Result<(), SemaError> {
    match whole_or_state(path, state, wctx) {
        PathState::Uninit => Err(init_error(
            format!("`{}` is not initialized here", render_path(path)),
            span,
        )),
        PathState::Moved => Err(move_error(
            format!("`{}` was already taken", render_path(path)),
            span,
        )),
        PathState::Init => Ok(()),
    }
}

/// `take place` (deliverable 3): the place must currently hold a value
/// (never-initialized -> `error[init]`, already-taken -> `error[move]`),
/// moving out of an array through a runtime index is always forbidden
/// (02-language.md §3.2), and a whole-root take is only legal when the
/// root is owned by this function (an ordinary local, or a `take`-mode
/// parameter/`self`) — taking a *borrowed* (`mut`-mode) root whole would
/// move a value this function does not own (access.rs's own doc comment
/// on `Mut`/`Take`: "moving a borrowed value is item E/F's job").
///
/// A `read`-mode root is rejected here too, for *any* take under it (not
/// only a whole-root one): `access.rs`'s own `Expr::Unary(Take, ...)`
/// check already rejects an ordinary `take place`/`take place.field`
/// expression against a `read` root earlier in the frozen pass order, but
/// a `take` written inside a `match`/`is` *pattern* payload never passes
/// through that expression-level check at all (`Pattern::Take` is a
/// different AST node, walked by `access.rs`'s `bind_pattern_locals`,
/// which only binds names — it never re-derives the scrutinee's root
/// mode) — so this pass is the only one that ever sees it. Checked on
/// every path depth (not gated behind `path.is_root()` the way the
/// `mut`-root case below is) to mirror `access.rs`'s own field-inclusive
/// `place_root_name` reach.
fn check_takeable(
    path: &StoragePath,
    state: &StateMap,
    wctx: &WCtx,
    span: Span,
) -> Result<(), SemaError> {
    match whole_or_state(path, state, wctx) {
        PathState::Uninit => {
            return Err(init_error(
                format!("`{}` is not initialized here", render_path(path)),
                span,
            ));
        }
        PathState::Moved => {
            return Err(move_error(
                format!("`{}` was already taken", render_path(path)),
                span,
            ));
        }
        PathState::Init => {}
    }
    if path
        .steps
        .iter()
        .any(|s| matches!(s, PathStep::RuntimeIndex))
    {
        return Err(move_error(
            format!(
                "moving `{}` out of an array through a runtime index is forbidden",
                render_path(path)
            ),
            span,
        ));
    }
    if let Some(AccessMode::Read) = wctx.modes.get(&path.root) {
        return Err(move_error(
            format!("`{}` is a `read` parameter; it cannot be taken", path.root),
            span,
        ));
    }
    if path.is_root() {
        if let Some(AccessMode::Mut) = wctx.modes.get(&path.root) {
            return Err(move_error(
                format!(
                    "`{}` is a `mut` place; taking it whole would move a value this function only borrows",
                    path.root
                ),
                span,
            ));
        }
    }
    Ok(())
}

/// Exclusivity (deliverable 4, 02-language.md §8.2): once an earlier
/// operand in this call's receiver-then-argument evaluation order
/// activated `mut`/`take` on a path, no later operand may touch
/// overlapping storage — two overlapping `mut`s, `mut` then a later
/// read, `mut`/`take` mixes, all reduce to the same overlap check on
/// storage paths.
fn check_no_overlap(
    path: &StoragePath,
    activated: &[StoragePath],
    span: Span,
) -> Result<(), SemaError> {
    for a in activated {
        if a.overlaps(path) {
            return Err(overlap_error(
                format!(
                    "`{}` overlaps `{}`, still exclusively active earlier in this call",
                    render_path(path),
                    render_path(a)
                ),
                span,
            ));
        }
    }
    Ok(())
}

/// Overwriting a live resource is always an error (02-language.md §3.1:
/// "move it or finish it first"); overwriting live data is fine (checked
/// only when `ty` classifies as a resource).
fn check_overwrite_live(
    path: &StoragePath,
    ty: Option<&Type>,
    state: &StateMap,
    wctx: &WCtx,
    span: Span,
) -> Result<(), SemaError> {
    if let Some(t) = ty {
        if bodies::is_resource_type(t, wctx.mctx) && state_of(path, state) == PathState::Init {
            return Err(move_error(
                format!(
                    "`{}` still holds a live resource; move it or finish it before overwriting",
                    render_path(path)
                ),
                span,
            ));
        }
    }
    Ok(())
}

/// The implicit-copy check (decision 9/deliverable 3): any binding/
/// storing use of a resource *place* without `take` is an error — an
/// assignment source, a literal/constructor operand (a struct/enum
/// field, a tuple/array/list element), all reduce to "is this bare
/// expression a place, and is its type a resource". A fresh value (a
/// call result, a literal/constructor) is never a place, so it always
/// passes; a `take`-wrapped operand never reaches this check at all (it
/// is handled as a real move instead, never as a bare value).
fn check_storing_value(value: &Expr, fctx: &FnCtx, wctx: &WCtx) -> Result<(), SemaError> {
    let Some(path) = as_path(value, fctx) else {
        return Ok(());
    };
    if let Some(ty) = place_type(value, fctx, wctx.mctx) {
        if bodies::is_resource_type(&ty, wctx.mctx) {
            return Err(move_error(
                format!(
                    "`{}` is a resource; use `take` to move it here (an implicit copy is not allowed)",
                    render_path(&path)
                ),
                value.span(),
            ));
        }
    }
    Ok(())
}

/// The field-take-and-restore / definite-init-of-`init`'s-own-fields
/// obligation (02-language.md §3.2, §7.1): at every function-level exit,
/// every **borrowed** (`mut`-mode) parameter/`self` root must either be
/// wholly `Moved` (unreachable for a `mut`-mode root — nothing can take
/// it whole, `check_takeable` already forbids that) or have no not-`Init`
/// path anywhere under it (every taken field restored). This is
/// deliberately scoped to `mut`-mode roots only: a `take`-mode parameter
/// or an ordinary local is *owned* by this function, so leaving one of
/// its fields moved at exit is fine (decision 5: no consume-on-every-path
/// obligation exists for any M2-expressible type; the auto-reclaim
/// machinery tears down whatever remains, piecewise) — only a `mut`
/// root's caller-visible coherence is actually at stake here.
fn check_exit_obligations(state: &StateMap, wctx: &WCtx, span: Span) -> Result<(), SemaError> {
    for (root, mode) in &wctx.modes {
        if *mode != AccessMode::Mut {
            continue;
        }
        let root_path = StoragePath::root(root.clone());
        if let Some((bad, _)) = first_bad_under(&root_path, state) {
            return Err(init_error(
                format!(
                    "`{}` must be fully initialized/restored before `{}` is used whole or the function returns",
                    render_path(&bad),
                    root
                ),
                span,
            ));
        }
    }
    Ok(())
}

/// One operand's full processing (a call's receiver or argument, but
/// also reused — with a throwaway `activated` list and `mode: Read` — for
/// any other "bare value in a storing/reading position" spot, since the
/// place/non-place dispatch and the storing-check are exactly the same
/// question there). `synthetic` marks a "no declared parameter mode"
/// position (a struct-literal/enum-variant/pseudo-constructor field) —
/// the implicit-copy check only ever applies there; a real declared
/// parameter's `Read`/`Mut` mode is a loan, never a copy.
fn process_operand<'a>(
    expr: &'a Expr,
    mode: AccessMode,
    activated: &mut Vec<StoragePath>,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    synthetic: bool,
    span: Span,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    let Some(path) = as_path(expr, fctx) else {
        return walk_expr(expr, state, fctx, wctx, dstack, loop_marker);
    };
    walk_place_subexprs(expr, state, fctx, wctx, dstack, loop_marker)?;
    match mode {
        AccessMode::Take => {
            check_no_overlap(&path, activated, span)?;
            check_takeable(&path, state, wctx, span)?;
            set_state(&path, state, PathState::Moved);
            activated.push(path);
        }
        AccessMode::Read | AccessMode::Mut => {
            check_readable(&path, state, wctx, span)?;
            check_no_overlap(&path, activated, span)?;
            if synthetic && mode == AccessMode::Read {
                check_storing_value(expr, fctx, wctx)?;
            }
            if mode == AccessMode::Mut {
                activated.push(path);
            }
        }
    }
    Ok(())
}

/// The same per-operand processing used at a call's argument list, for a
/// single bare (non-`Arg`) expression in a storing position — an
/// assignment source or a `return` expression. A throwaway `activated`
/// list is correct: exclusivity is scoped to call-expression argument
/// lists/receivers only (deliverable 4's "keep it to the documented
/// rule").
fn walk_storing_expr<'a>(
    value: &'a Expr,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    let mut activated = Vec::new();
    process_operand(
        value,
        AccessMode::Read,
        &mut activated,
        state,
        fctx,
        wctx,
        true,
        value.span(),
        dstack,
        loop_marker,
    )
}

// --- pattern payload moves (decision 9's "pattern payload", §7.2) ---------

/// Whether `p` requests a `take` anywhere in its shape. A `match`/`is`
/// pattern never moves a payload implicitly; writing `take` anywhere in
/// it does. Since a payload is not addressable by its own storage path
/// (an enum variant's payload has no field name; a tuple/array pattern's
/// element paths are not tracked at this granularity either), any `take`
/// found anywhere in the pattern moves the *whole* scrutinee place — for
/// an enum this is exact (a sum type has exactly one active payload, so
/// taking any of it invalidates the whole value, not merely an approximation
/// of it); for a tuple/array pattern this is a conservative, sound
/// over-approximation (flagged in the session report) rather than
/// per-element precision.
fn pattern_has_take(p: &Pattern) -> bool {
    match p {
        Pattern::Take(..) => true,
        Pattern::Wildcard(_) | Pattern::Literal(..) | Pattern::Binding(..) => false,
        Pattern::Variant { payload, .. } => payload.iter().any(pattern_has_take),
        Pattern::Tuple(_, items) | Pattern::Array(_, items) | Pattern::Or(_, items) => {
            items.iter().any(pattern_has_take)
        }
    }
}

/// Applies a pattern's move effect (if any) to `scrutinee`'s own storage
/// path, if it has one (a fresh scrutinee — a call result — has nothing
/// to move from).
fn apply_pattern_move(
    scrutinee: &Expr,
    pattern: &Pattern,
    state: &mut StateMap,
    fctx: &FnCtx,
    wctx: &WCtx,
    span: Span,
) -> Result<(), SemaError> {
    if !pattern_has_take(pattern) {
        return Ok(());
    }
    let Some(path) = as_path(scrutinee, fctx) else {
        return Ok(());
    };
    check_takeable(&path, state, wctx, span)?;
    set_state(&path, state, PathState::Moved);
    Ok(())
}

/// Every name a pattern binds is a fresh local for the duration of the
/// arm/success branch (`bodies::check_pattern` — reused verbatim,
/// exactly like `matches.rs` does — already gave each one its type in
/// `fctx`; this only needs to mark each one `Init` in the flow state,
/// since a pattern never leaves a binding partially formed).
fn seed_pattern_bindings(p: &Pattern, state: &mut StateMap) {
    match p {
        Pattern::Wildcard(_) | Pattern::Literal(..) => {}
        Pattern::Binding(_, name) => {
            state.insert(StoragePath::root(name.clone()), PathState::Init);
        }
        Pattern::Take(_, inner) => seed_pattern_bindings(inner, state),
        Pattern::Variant { payload, .. } => {
            for p in payload {
                seed_pattern_bindings(p, state);
            }
        }
        Pattern::Tuple(_, items) | Pattern::Array(_, items) | Pattern::Or(_, items) => {
            for p in items {
                seed_pattern_bindings(p, state);
            }
        }
    }
}

// --- the general expression walk -------------------------------------------

fn walk_expr<'a>(
    expr: &'a Expr,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    match expr {
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::BStr(..)
        | Expr::Char(..)
        | Expr::Bool(..)
        | Expr::Unit(_)
        | Expr::FStr(_) => Ok(()),
        Expr::Name(span, name) => {
            if fctx.lookup_local(name).is_some() {
                check_readable(&StoragePath::root(name.clone()), state, wctx, *span)
            } else {
                Ok(()) // a module fn/const, struct/enum type name, or a builtin
                // prelude constructor (`None`/`Some`/`Ok`/`Err`/`panic`) — never a
                // storage location, so never `Uninit`/`Moved`.
            }
        }
        Expr::Field(base, _, _) => match as_path(expr, fctx) {
            Some(path) => {
                walk_place_subexprs(expr, state, fctx, wctx, dstack, loop_marker)?;
                check_readable(&path, state, wctx, expr.span())
            }
            None => walk_expr(base, state, fctx, wctx, dstack, loop_marker),
        },
        Expr::Index(base, ispan, args) => match as_path(expr, fctx) {
            Some(path) => {
                walk_place_subexprs(expr, state, fctx, wctx, dstack, loop_marker)?;
                check_readable(&path, state, wctx, *ispan)
            }
            None => {
                walk_expr(base, state, fctx, wctx, dstack, loop_marker)?;
                for a in args {
                    walk_expr(a, state, fctx, wctx, dstack, loop_marker)?;
                }
                Ok(())
            }
        },
        Expr::Call(callee, span, args) => {
            walk_call(callee, *span, args, state, fctx, wctx, dstack, loop_marker)
        }
        Expr::Unary(span, UnaryOp::Take, inner) => {
            let Some(path) = as_path(inner, fctx) else {
                // access.rs already requires `take`'s operand to be a
                // full place; unreachable for a module that reached
                // flow, but fail closed rather than panic if it somehow
                // isn't.
                return Err(move_error(
                    "`take` requires a place expression".to_string(),
                    *span,
                ));
            };
            walk_place_subexprs(inner, state, fctx, wctx, dstack, loop_marker)?;
            check_takeable(&path, state, wctx, *span)?;
            set_state(&path, state, PathState::Moved);
            Ok(())
        }
        Expr::Unary(_, _, inner) => walk_expr(inner, state, fctx, wctx, dstack, loop_marker),
        Expr::Try(_, inner) => {
            // `?` returns from the enclosing function on `Err` (02-
            // language.md §8.2/§10): the same real, unforked `state`
            // this pass already uses for both continuations (see the
            // module doc comment on `?`'s own move semantics) means every
            // currently active defer — the *whole* stack, not sliced by
            // `loop_marker`, since `?` exits the function, not merely a
            // loop — must be re-validated here, exactly like a `return`.
            walk_expr(inner, state, fctx, wctx, dstack, loop_marker)?;
            check_active_defers(dstack, "a `?` exit", state, fctx, wctx)
        }
        Expr::Binary(_, _, l, r) => {
            walk_expr(l, state, fctx, wctx, dstack, loop_marker)?;
            walk_expr(r, state, fctx, wctx, dstack, loop_marker)
        }
        Expr::Range(_, a, b, _) => {
            walk_expr(a, state, fctx, wctx, dstack, loop_marker)?;
            walk_expr(b, state, fctx, wctx, dstack, loop_marker)
        }
        Expr::Is(_, scrutinee, pattern) => {
            walk_expr(scrutinee, state, fctx, wctx, dstack, loop_marker)?;
            let sty = bodies::check_expr(scrutinee, None, fctx, wctx.mctx)?.ty;
            apply_pattern_move(scrutinee, pattern, state, fctx, wctx, scrutinee.span())?;
            bodies::check_pattern(pattern, &sty, fctx, wctx.mctx)?;
            seed_pattern_bindings(pattern, state);
            Ok(())
        }
        Expr::Not(_, inner) => walk_expr(inner, state, fctx, wctx, dstack, loop_marker),
        Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            walk_expr(l, state, fctx, wctx, dstack, loop_marker)?;
            walk_expr(r, state, fctx, wctx, dstack, loop_marker)
        }
        Expr::DotVariant(_, _, args) => {
            let mut activated = Vec::new();
            for a in args {
                process_operand(
                    &a.value,
                    a.mode,
                    &mut activated,
                    state,
                    fctx,
                    wctx,
                    true,
                    a.span,
                    dstack,
                    loop_marker,
                )?;
            }
            Ok(())
        }
        Expr::Closure(c) => walk_closure(c, state, fctx, wctx, dstack, loop_marker),
        Expr::Send(span, _) => Err(bodies_send_unreachable(*span)),
        Expr::Tuple(_, items) | Expr::List(_, items) => {
            let mut activated = Vec::new();
            for i in items {
                process_operand(
                    i,
                    AccessMode::Read,
                    &mut activated,
                    state,
                    fctx,
                    wctx,
                    true,
                    i.span(),
                    dstack,
                    loop_marker,
                )?;
            }
            Ok(())
        }
    }
}

/// `send` always fails closed in `bodies.rs` before `flow` ever runs
/// (plans/M2.md decision 7); this is unreachable in practice — a fail-
/// closed diagnostic rather than a panic, per the item's own "never
/// approximation" instruction, in case that invariant is ever broken.
fn bodies_send_unreachable(span: Span) -> SemaError {
    crate::sema::unimplemented_at("`send` is", span)
}

fn walk_closure<'a>(
    c: &'a ClosureExpr,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    fctx.push_scope();
    for p in &c.params {
        if let Some(t) = &p.ty {
            let ty = wctx.mctx.resolve_type(t, &fctx.local_pools)?;
            fctx.insert_local(p.name.clone(), ty);
        }
        state.insert(StoragePath::root(p.name.clone()), PathState::Init);
    }
    match &c.body {
        ClosureBody::Expr(e) => walk_expr(e, state, fctx, wctx, dstack, loop_marker)?,
        ClosureBody::Suite(stmts) => {
            walk_block(stmts, state, fctx, wctx, dstack, loop_marker)?;
        }
    }
    fctx.pop_scope();
    Ok(())
}

// --- calls: receiver + argument-list exclusivity + move/storing dispatch --

/// Whether `callee` resolves to a position with *no* declared parameter
/// modes (a struct literal without `init`, an enum variant, or an
/// unresolvable builtin pseudo-constructor like `Some`/`Ok`/`Err`) —
/// exactly the shapes where the implicit-copy check applies to an
/// unmarked argument. A real fn/method/`init`/associated-fn call, or a
/// call through a `fn`-typed value, always has declared modes (mirrored
/// by `access.rs` already, so this pass only needs `Arg.mode` itself,
/// never the declared mode).
fn is_synthetic_call(callee: &Expr, fctx: &FnCtx, wctx: &WCtx) -> bool {
    match callee {
        Expr::Name(_, name) => {
            if fctx.lookup_local(name).is_some() {
                return false;
            }
            if wctx.mctx.consts.contains_key(name) {
                return false;
            }
            if wctx.mctx.fns.contains_key(name) {
                return false;
            }
            if let Some(s) = wctx.mctx.structs.get(name) {
                return s.init().is_none();
            }
            true // `Some`/`Ok`/`Err`/`panic`.
        }
        Expr::Field(base, _, _) => {
            if let Expr::Name(_, bname) = base.as_ref() {
                if fctx.lookup_local(bname).is_none() {
                    if wctx.mctx.structs.contains_key(bname) {
                        return false; // associated fn.
                    }
                    if wctx.mctx.enums.contains_key(bname) {
                        return true; // enum variant construction.
                    }
                }
            }
            false // a real method call.
        }
        Expr::Index(inner, _, _) => is_synthetic_call(inner, fctx, wctx),
        _ => false,
    }
}

/// The receiver operand + its effective access mode, for a real method
/// call (`base.method(...)`) — `None` for every other callee shape
/// (associated fn/enum variant: `base` names a type, not a value; a
/// plain fn/closure call: no receiver at all).
fn receiver_of<'e>(callee: &'e Expr, fctx: &FnCtx, wctx: &WCtx) -> Option<(&'e Expr, AccessMode)> {
    match callee {
        Expr::Index(inner, _, _) => receiver_of(inner, fctx, wctx),
        Expr::Field(base, _, name) => {
            if let Expr::Name(_, bname) = base.as_ref() {
                if fctx.lookup_local(bname).is_none()
                    && (wctx.mctx.structs.contains_key(bname)
                        || wctx.mctx.enums.contains_key(bname))
                {
                    return None;
                }
            }
            let base_ty = bodies::unwrap_own(place_type(base, fctx, wctx.mctx)?);
            let Type::Named(sname, _) = &base_ty else {
                return None;
            };
            let s = wctx.mctx.structs.get(sname)?;
            let (mf, d) = s.method(name)?;
            let mode = access::resolve_receiver_mode(mf, d, sname, wctx.mctx, wctx.effects).ok()?;
            Some((base.as_ref(), mode))
        }
        _ => None,
    }
}

/// Walks the callee expression itself when it is not a real method's
/// receiver — a plain type/fn name contributes nothing to check (no
/// value read); a real value (a local holding a `fn`, or a scalar
/// conversion's `x` in `x.to[T]()`) still needs its own validity check.
fn walk_callee_base<'a>(
    callee: &'a Expr,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    match callee {
        Expr::Name(_, name) => {
            if fctx.lookup_local(name).is_some() || wctx.mctx.consts.contains_key(name) {
                walk_expr(callee, state, fctx, wctx, dstack, loop_marker)
            } else {
                Ok(())
            }
        }
        Expr::Field(base, _, _) => {
            if let Expr::Name(_, bname) = base.as_ref() {
                if fctx.lookup_local(bname).is_none()
                    && (wctx.mctx.structs.contains_key(bname)
                        || wctx.mctx.enums.contains_key(bname))
                {
                    return Ok(());
                }
            }
            walk_expr(base, state, fctx, wctx, dstack, loop_marker)
        }
        Expr::Index(inner, _, _) => walk_callee_base(inner, state, fctx, wctx, dstack, loop_marker),
        other => walk_expr(other, state, fctx, wctx, dstack, loop_marker),
    }
}

fn walk_call<'a>(
    callee: &'a Expr,
    _span: Span,
    args: &'a [Arg],
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    let synthetic = is_synthetic_call(callee, fctx, wctx);
    let mut activated: Vec<StoragePath> = Vec::new();

    match receiver_of(callee, fctx, wctx) {
        Some((recv_expr, recv_mode)) => {
            let span = recv_expr.span();
            process_operand(
                recv_expr,
                recv_mode,
                &mut activated,
                state,
                fctx,
                wctx,
                false,
                span,
                dstack,
                loop_marker,
            )?;
        }
        None => walk_callee_base(callee, state, fctx, wctx, dstack, loop_marker)?,
    }

    for a in args {
        process_operand(
            &a.value,
            a.mode,
            &mut activated,
            state,
            fctx,
            wctx,
            synthetic,
            a.span,
            dstack,
            loop_marker,
        )?;
    }
    Ok(())
}

// --- statements + the structural CFG walk ----------------------------------

/// One block's control-flow result: `fallthrough` is the merged state
/// reaching the end of the block normally (`None` if every path
/// diverges); `breaks`/`continues` collect the states at every reachable
/// `break`/`continue` not yet consumed by an enclosing loop.
struct Outcome {
    fallthrough: Option<StateMap>,
    breaks: Vec<StateMap>,
    continues: Vec<StateMap>,
}

fn fallthrough(state: StateMap) -> Outcome {
    Outcome {
        fallthrough: Some(state),
        breaks: Vec::new(),
        continues: Vec::new(),
    }
}

fn merge_outcomes(outcomes: Vec<Outcome>) -> Outcome {
    let mut breaks = Vec::new();
    let mut continues = Vec::new();
    let mut fallthroughs = Vec::new();
    for o in outcomes {
        breaks.extend(o.breaks);
        continues.extend(o.continues);
        if let Some(s) = o.fallthrough {
            fallthroughs.push(s);
        }
    }
    let ft = if fallthroughs.is_empty() {
        None
    } else {
        Some(meet_all(fallthroughs))
    };
    Outcome {
        fallthrough: ft,
        breaks,
        continues,
    }
}

fn walk_assign<'a>(
    a: &'a AssignStmt,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    if a.op == AssignOp::Assign {
        walk_storing_expr(&a.value, state, fctx, wctx, dstack, loop_marker)?;
    } else {
        walk_expr(&a.value, state, fctx, wctx, dstack, loop_marker)?;
    }

    if let Expr::Name(_, name) = &a.target {
        if fctx.lookup_innermost(name).is_none() {
            let ty = match &a.ty {
                Some(ann) => wctx.mctx.resolve_type(ann, &fctx.local_pools)?,
                None => bodies::check_expr(&a.value, None, fctx, wctx.mctx)?.ty,
            };
            fctx.insert_local(name.clone(), ty);
        }
    } else {
        walk_place_subexprs(&a.target, state, fctx, wctx, dstack, loop_marker)?;
    }

    let Some(path) = as_path(&a.target, fctx) else {
        return Ok(());
    };
    if a.op == AssignOp::Assign {
        let ty = place_type(&a.target, fctx, wctx.mctx);
        check_overwrite_live(&path, ty.as_ref(), state, wctx, a.span)?;
    } else {
        check_readable(&path, state, wctx, a.span)?;
    }
    set_state(&path, state, PathState::Init);
    Ok(())
}

/// `init`'s own `return Err(...)` (02-language.md §7.1): recognized
/// syntactically (the return expression is directly a call to `Err`) —
/// the only pattern the M2 corpus's `init`s use, and the honest limit of
/// what this pass can tell apart without real control analysis.
fn is_err_return(e: &Expr) -> bool {
    matches!(e, Expr::Call(callee, _, _) if matches!(callee.as_ref(), Expr::Name(_, name) if name == "Err"))
}

fn walk_return<'a>(
    span: Span,
    e: &'a Option<Expr>,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    if let Some(expr) = e {
        walk_storing_expr(expr, state, fctx, wctx, dstack, loop_marker)?;
    }
    // `return` exits the entire function: every currently active defer —
    // the whole stack, from every enclosing block, not sliced by
    // `loop_marker` — runs here, in reverse registration order
    // (02-language.md §10). Checked unconditionally, including an
    // `init`'s own `return Err(...)` (the ordinary local-cleanup rule
    // for that path per §7.1 still includes any covering `defer`).
    check_active_defers(dstack, "a `return` exit", state, fctx, wctx)?;
    if wctx.is_init && e.as_ref().is_some_and(is_err_return) {
        return Ok(());
    }
    check_exit_obligations(state, wctx, span)
}

fn walk_stmt<'a>(
    stmt: &'a Stmt,
    state: &StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<Outcome, SemaError> {
    match stmt {
        Stmt::Assign(a) => {
            let mut st = state.clone();
            walk_assign(a, &mut st, fctx, wctx, dstack, loop_marker)?;
            Ok(fallthrough(st))
        }
        Stmt::If(i) => walk_if(i, state, fctx, wctx, dstack, loop_marker),
        Stmt::Match(m) => walk_match(m, state, fctx, wctx, dstack, loop_marker),
        Stmt::For(f) => walk_for(f, state, fctx, wctx, dstack, loop_marker),
        Stmt::While(w) => walk_while(w, state, fctx, wctx, dstack, loop_marker),
        Stmt::Break(_) => {
            // `break` exits only the nearest enclosing loop, not the
            // whole function: only defers registered since that loop was
            // entered (`loop_marker`) run here — a defer registered in an
            // enclosing block outside the loop stays pending for a later,
            // real exit (02-language.md §10).
            check_active_defers(&dstack[loop_marker..], "a `break` exit", state, fctx, wctx)?;
            Ok(Outcome {
                fallthrough: None,
                breaks: vec![state.clone()],
                continues: Vec::new(),
            })
        }
        Stmt::Continue(_) => {
            check_active_defers(
                &dstack[loop_marker..],
                "a `continue` exit",
                state,
                fctx,
                wctx,
            )?;
            Ok(Outcome {
                fallthrough: None,
                breaks: Vec::new(),
                continues: vec![state.clone()],
            })
        }
        Stmt::Return(span, e) => {
            let mut st = state.clone();
            walk_return(*span, e, &mut st, fctx, wctx, dstack, loop_marker)?;
            Ok(Outcome {
                fallthrough: None,
                breaks: Vec::new(),
                continues: Vec::new(),
            })
        }
        Stmt::Pass(_) => Ok(fallthrough(state.clone())),
        Stmt::Assert(a) => {
            let mut st = state.clone();
            walk_expr(&a.cond, &mut st, fctx, wctx, dstack, loop_marker)?;
            if let Some(msg) = &a.message {
                walk_expr(msg, &mut st, fctx, wctx, dstack, loop_marker)?;
            }
            Ok(fallthrough(st))
        }
        Stmt::Defer(d) => {
            // Registration only (deliverable: 02-language.md §10's
            // "verifies the places it names are valid at every exit," not
            // at registration) — pushed onto the block-scoped stack;
            // `walk_block` (this statement's own enclosing block) checks
            // and pops it, and every real exit reached before that check
            // (`return`/`?`/`break`/`continue`) re-validates it too, via
            // `check_active_defers`.
            dstack.push(d);
            Ok(fallthrough(state.clone()))
        }
        Stmt::With(_) | Stmt::Send(..) | Stmt::ComptimeIf(_) | Stmt::ComptimeAssert(..) => {
            // Every one of these already fails closed in `bodies::check`
            // (plans/M2.md decision 7); `mod.rs::check` is fail-fast, so
            // flow never runs over a module containing one.
            unreachable!("bodies.rs already rejects this construct before flow runs")
        }
        Stmt::Expr(_, e) => {
            let mut st = state.clone();
            walk_expr(e, &mut st, fctx, wctx, dstack, loop_marker)?;
            Ok(fallthrough(st))
        }
    }
}

/// Walks one block's own statements, threading `dstack` as a true call
/// stack (02-language.md §10's "registers ... against the enclosing
/// block"): defers registered directly by this block's own statements
/// (not ones already popped by a nested block's own call) are, if this
/// block completes normally, re-validated against exactly that
/// completion state — the block's own fallthrough exit — in reverse
/// registration order, then popped, so they stop being visible to
/// whatever code follows this block once it returns (an early exit
/// reached *during* this block's own walk was already checked in place,
/// by `walk_stmt`'s `Return`/`Break`/`Continue` handling or `walk_expr`'s
/// `?` handling, using `dstack` as it stood at that point).
fn walk_block<'a>(
    stmts: &'a [Stmt],
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<Outcome, SemaError> {
    let start = dstack.len();
    let mut breaks = Vec::new();
    let mut continues = Vec::new();
    for s in stmts {
        let outcome = walk_stmt(s, state, fctx, wctx, dstack, loop_marker)?;
        breaks.extend(outcome.breaks);
        continues.extend(outcome.continues);
        match outcome.fallthrough {
            Some(new_state) => *state = new_state,
            None => {
                dstack.truncate(start);
                return Ok(Outcome {
                    fallthrough: None,
                    breaks,
                    continues,
                });
            }
        }
    }
    check_active_defers(
        &dstack[start..],
        "this block's own normal completion",
        state,
        fctx,
        wctx,
    )?;
    dstack.truncate(start);
    Ok(Outcome {
        fallthrough: Some(state.clone()),
        breaks,
        continues,
    })
}

fn walk_if<'a>(
    i: &'a IfStmt,
    state: &StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<Outcome, SemaError> {
    let mut outcomes = Vec::new();

    let mut st = state.clone();
    walk_expr(&i.cond, &mut st, fctx, wctx, dstack, loop_marker)?;
    outcomes.push(walk_block(
        &i.then_branch,
        &mut st,
        fctx,
        wctx,
        dstack,
        loop_marker,
    )?);

    for elif in &i.elifs {
        let mut st = state.clone();
        walk_expr(&elif.cond, &mut st, fctx, wctx, dstack, loop_marker)?;
        outcomes.push(walk_block(
            &elif.body,
            &mut st,
            fctx,
            wctx,
            dstack,
            loop_marker,
        )?);
    }

    match &i.else_branch {
        Some(body) => {
            let mut st = state.clone();
            outcomes.push(walk_block(body, &mut st, fctx, wctx, dstack, loop_marker)?);
        }
        None => outcomes.push(fallthrough(state.clone())),
    }

    Ok(merge_outcomes(outcomes))
}

/// `match`'s own arms only (deliberately no implicit "no arm matched"
/// path): exhaustiveness is `matches.rs`'s job, which runs *after* flow
/// in the frozen pass order. Assuming the written arms are exhaustive
/// here is always safe — a module whose match genuinely isn't exhaustive
/// still gets rejected, just by `matches.rs`'s own diagnostic once flow
/// finishes; flow merely doesn't manufacture a spurious extra edge for a
/// case that (if real) is caught downstream anyway.
fn walk_match<'a>(
    m: &'a MatchStmt,
    state: &StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    loop_marker: usize,
) -> Result<Outcome, SemaError> {
    let mut entry = state.clone();
    walk_expr(&m.scrutinee, &mut entry, fctx, wctx, dstack, loop_marker)?;
    let sty = bodies::check_expr(&m.scrutinee, None, fctx, wctx.mctx)?.ty;

    let mut outcomes = Vec::new();
    for arm in &m.arms {
        let mut st = entry.clone();
        apply_pattern_move(&m.scrutinee, &arm.pattern, &mut st, fctx, wctx, arm.span)?;
        bodies::check_pattern(&arm.pattern, &sty, fctx, wctx.mctx)?;
        seed_pattern_bindings(&arm.pattern, &mut st);
        if let Some(g) = &arm.guard {
            walk_expr(g, &mut st, fctx, wctx, dstack, loop_marker)?;
        }
        outcomes.push(walk_block(
            &arm.body,
            &mut st,
            fctx,
            wctx,
            dstack,
            loop_marker,
        )?);
    }
    Ok(merge_outcomes(outcomes))
}

/// One loop iteration's back-edge state: the meet of falling through the
/// body normally and every `continue` reached within it (both loop back
/// to the condition/next-element check) — `break`s are excluded (they
/// exit the loop, contributing to its own post-loop state instead, see
/// `walk_while`/`walk_for`).
fn loop_backedge(out: &Outcome) -> Option<StateMap> {
    let mut states = Vec::new();
    if let Some(s) = &out.fallthrough {
        states.push(s.clone());
    }
    states.extend(out.continues.iter().cloned());
    if states.is_empty() {
        None
    } else {
        Some(meet_all(states))
    }
}

/// Loop-local freshness: a path whose root never appeared in the state
/// *entering* the loop is, by construction, first bound somewhere inside
/// the loop's own body — and 02-language.md §3.2's "the first assignment
/// to a name introduces it" applies anew on every real iteration, not
/// only the textual first time this analysis's fixed-point re-walk
/// visits that statement. Without this, a resource-typed local first
/// bound inside a loop body (`q = make(i)`) would spuriously fail
/// `check_overwrite_live` starting on the *second* fixed-point pass: nothing
/// about `q`'s value actually carries across a real iteration boundary
/// (unlike a `mut` parameter/field, whose root is already present in
/// `baseline_roots` and so is correctly left to flow through the fixed
/// point), but the fixed point's own carried-forward `candidate` would
/// otherwise still show it `Init` going into the next pass's re-walk of
/// its own introducing assignment. Pruning every non-baseline path out of
/// the candidate before each pass keeps the fixed point meaningful only
/// for paths that genuinely persist across the loop's back edge.
fn prune_loop_locals(st: &mut StateMap, baseline_roots: &BTreeSet<String>) {
    st.retain(|p, _| baseline_roots.contains(&p.root));
}

fn walk_while<'a>(
    w: &'a WhileStmt,
    state: &StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    _loop_marker: usize,
) -> Result<Outcome, SemaError> {
    // A fresh loop boundary for `break`/`continue` inside this loop's own
    // body: only defers registered from here on (this loop's own body,
    // per iteration — see the module doc comment's "Loops" note) are
    // theirs to run; a defer registered by an enclosing block stays
    // pending for whatever real exit eventually reaches it.
    let body_marker = dstack.len();
    let baseline_roots: BTreeSet<String> = state.keys().map(|p| p.root.clone()).collect();
    let mut candidate = state.clone();
    let mut out = fallthrough(state.clone());
    for _ in 0..LOOP_FIXED_POINT_CAP {
        let mut st = candidate.clone();
        prune_loop_locals(&mut st, &baseline_roots);
        walk_expr(&w.cond, &mut st, fctx, wctx, dstack, body_marker)?;
        let o = walk_block(&w.body, &mut st, fctx, wctx, dstack, body_marker)?;
        let next = loop_backedge(&o).unwrap_or_else(|| candidate.clone());
        let converged = next == candidate;
        candidate = next;
        out = o;
        if converged {
            break;
        }
    }
    let mut exit_states = vec![state.clone()];
    if let Some(s) = out.fallthrough {
        exit_states.push(s);
    }
    exit_states.extend(out.breaks);
    Ok(Outcome {
        fallthrough: Some(meet_all(exit_states)),
        breaks: Vec::new(),
        continues: Vec::new(),
    })
}

/// Mirrors `matches::check_for`'s own elem-type derivation exactly
/// (range vs fixed array, `take`-consumed iterable unwrapped the same
/// way) purely to keep `fctx`'s locals accurate; typing itself is
/// `bodies::check_expr` reused, not reimplemented.
fn for_elem_type(f: &ForStmt, fctx: &mut FnCtx, wctx: &WCtx) -> Result<Type, SemaError> {
    let raw_iterable: &Expr = match &f.iterable {
        Expr::Unary(_, UnaryOp::Take, inner) => inner.as_ref(),
        other => other,
    };
    match raw_iterable {
        Expr::Range(_, from, to, _incl) => {
            let (first, _second) =
                if bodies::is_bare_numeric_literal(from) && !bodies::is_bare_numeric_literal(to) {
                    (to.as_ref(), from.as_ref())
                } else {
                    (from.as_ref(), to.as_ref())
                };
            bodies::check_expr(first, None, fctx, wctx.mctx).map(|te| te.ty)
        }
        other => match bodies::check_expr(other, None, fctx, wctx.mctx)?.ty {
            Type::Array(elem, _) => Ok(*elem),
            other_ty => Ok(other_ty),
        },
    }
}

fn walk_for<'a>(
    f: &'a ForStmt,
    state: &StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx,
    dstack: &mut DStack<'a>,
    _loop_marker: usize,
) -> Result<Outcome, SemaError> {
    let mut entry = state.clone();
    let elem_ty = for_elem_type(f, fctx, wctx)?;
    fctx.insert_local(f.name.clone(), elem_ty);

    // The iterable is evaluated exactly once (02-language.md §8.1),
    // before the loop; `for take x in take arr` consumes the array
    // (decision 9). A fresh loop boundary for `break`/`continue` inside
    // the body, same reasoning as `walk_while`.
    let body_marker = dstack.len();
    if let Expr::Unary(span, UnaryOp::Take, inner) = &f.iterable {
        match as_path(inner, fctx) {
            Some(path) => {
                walk_place_subexprs(inner, &mut entry, fctx, wctx, dstack, body_marker)?;
                check_takeable(&path, &entry, wctx, *span)?;
                set_state(&path, &mut entry, PathState::Moved);
            }
            None => walk_expr(inner, &mut entry, fctx, wctx, dstack, body_marker)?,
        }
    } else {
        walk_expr(&f.iterable, &mut entry, fctx, wctx, dstack, body_marker)?;
    }

    let baseline_roots: BTreeSet<String> = entry.keys().map(|p| p.root.clone()).collect();
    let mut candidate = entry.clone();
    let mut out = fallthrough(entry.clone());
    for _ in 0..LOOP_FIXED_POINT_CAP {
        let mut st = candidate.clone();
        prune_loop_locals(&mut st, &baseline_roots);
        st.insert(StoragePath::root(f.name.clone()), PathState::Init);
        let o = walk_block(&f.body, &mut st, fctx, wctx, dstack, body_marker)?;
        let next = loop_backedge(&o).unwrap_or_else(|| candidate.clone());
        let converged = next == candidate;
        candidate = next;
        out = o;
        if converged {
            break;
        }
    }

    let mut exit_states = vec![entry.clone()];
    if let Some(s) = out.fallthrough {
        exit_states.push(s);
    }
    exit_states.extend(out.breaks);
    Ok(Outcome {
        fallthrough: Some(meet_all(exit_states)),
        breaks: Vec::new(),
        continues: Vec::new(),
    })
}

// --- module-wide checking context + entry points ---------------------------

/// Every parameter's (and, where applicable, `self`'s) declared access
/// mode, root-keyed — the only extra bookkeeping flow needs beyond
/// `bodies::ModuleCtx`/`FnCtx`: whether a whole-root `take` is legal
/// (owned: an ordinary local absent from this map, or a `Take`-mode
/// root; never a `Read`/`Mut`-mode root) and which roots the
/// field-take-and-restore/definite-init-of-fields obligation applies to.
struct WCtx<'a> {
    mctx: &'a ModuleCtx,
    effects: &'a EffectMap,
    modes: BTreeMap<String, AccessMode>,
    /// Whether this body is a struct's `init` (02-language.md §7.1): an
    /// `init`'s `return Err(...)` exit tears down whatever fields were
    /// initialized so far by the ordinary local-cleanup rule, so — unlike
    /// every other exit — it carries no field-completeness obligation of
    /// its own (`walk_return` special-cases exactly this one shape,
    /// syntactically, since that is the only honestly checkable reading:
    /// distinguishing a "successful" exit from a "the whole value is
    /// abandoned" exit for any richer error shape would need real control
    /// analysis this item does not otherwise do).
    is_init: bool,
}

/// The two checks that only make sense at a function's own boundary
/// (deliverable 2's "value-returning body must return on every path",
/// and the field-restore obligation applied to the implicit end-of-body
/// exit) — shared by `check_top_fn`/`check_struct_members`'s three
/// member shapes.
fn check_body_exit(
    outcome: &Outcome,
    fctx: &FnCtx,
    wctx: &WCtx,
    span: Span,
) -> Result<(), SemaError> {
    let Some(final_state) = &outcome.fallthrough else {
        return Ok(());
    };
    if fctx.ret_ty != Type::Unit {
        return Err(init_error(
            format!(
                "missing return of type `{}` on some path",
                types::render_type(&fctx.ret_ty)
            ),
            span,
        ));
    }
    check_exit_obligations(final_state, wctx, span)
}

/// `pub(crate)` (item H, generics.rs): re-run verbatim over a
/// substituted, generics-cleared copy of a generic fn's own ast+decl,
/// mirroring `bodies::check_top_fn`/`access::check_top_fn`'s own
/// widening exactly.
pub(crate) fn check_top_fn(
    f: &ast::FnItem,
    d: &types::DeclFn,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> Result<(), SemaError> {
    if bodies::is_image_fn(f) || !f.generics.is_empty() {
        return Ok(());
    }
    let Some(body) = &f.body else {
        return Ok(()); // bodyless: already fails closed in bodies::check.
    };

    let mut modes = BTreeMap::new();
    let mut state = StateMap::new();
    let mut fctx = FnCtx::new(d.ret.clone(), mctx.module_pools.clone());
    for (_ap, dp) in f.params.iter().zip(d.params.iter()) {
        modes.insert(dp.name.clone(), dp.mode);
        state.insert(StoragePath::root(dp.name.clone()), PathState::Init);
        fctx.insert_local(dp.name.clone(), dp.ty.clone());
    }
    let wctx = WCtx {
        mctx,
        effects,
        modes,
        is_init: false,
    };

    let mut dstack: DStack = Vec::new();
    let outcome = walk_block(body, &mut state, &mut fctx, &wctx, &mut dstack, 0)?;
    check_body_exit(&outcome, &fctx, &wctx, f.span)
}

fn check_struct_bodies(
    s: &ast::StructItem,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> Result<(), SemaError> {
    if !s.generics.is_empty() {
        return Ok(());
    }
    let info = mctx.structs.get(&s.name).expect("struct present in mctx");
    let self_ty = Type::Named(s.name.clone(), Vec::new());
    check_struct_members(info, self_ty, mctx, effects)
}

/// `pub(crate)` (item H, generics.rs): the guts of `check_struct_bodies`,
/// pulled out to take a `StructInfo` directly, mirroring
/// `bodies::check_struct_members`'s own doc comment.
pub(crate) fn check_struct_members(
    info: &StructInfo,
    self_ty: Type,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> Result<(), SemaError> {
    let sname = info.decl.name.clone();
    let local_pools = bodies::local_pool_names(info);

    for (am, dm) in info.members() {
        match (am, dm) {
            (Member::Fn(f), types::DeclMember::Fn(fd)) => {
                if !f.generics.is_empty() {
                    continue;
                }
                let Some(body) = &f.body else {
                    continue; // bodyless: already fails closed in bodies::check.
                };
                let self_mode = access::resolve_receiver_mode(f, fd, &sname, mctx, effects)?;

                let mut modes = BTreeMap::new();
                let mut state = StateMap::new();
                let mut fctx = FnCtx::new(fd.ret.clone(), local_pools.clone());
                fctx.insert_local("self".to_string(), self_ty.clone());
                modes.insert("self".to_string(), self_mode);
                state.insert(StoragePath::root("self"), PathState::Init);
                for (_ap, dp) in f.params.iter().zip(fd.params.iter()) {
                    modes.insert(dp.name.clone(), dp.mode);
                    state.insert(StoragePath::root(dp.name.clone()), PathState::Init);
                    fctx.insert_local(dp.name.clone(), dp.ty.clone());
                }
                let wctx = WCtx {
                    mctx,
                    effects,
                    modes,
                    is_init: false,
                };

                let mut dstack: DStack = Vec::new();
                let outcome = walk_block(body, &mut state, &mut fctx, &wctx, &mut dstack, 0)?;
                check_body_exit(&outcome, &fctx, &wctx, f.span)?;
            }
            (Member::Init(i), types::DeclMember::Init(fd)) => {
                // `init` (02-language.md §7.1): each field of `self` is
                // checked exactly like an uninitialized local — seeded
                // `Uninit` here, one entry per declared field, rather
                // than seeding `self`'s own root at all (there is
                // nothing coherent to use "whole" until every field is
                // assigned, so a bare `self` read before that point
                // correctly falls back to this pass's default `Uninit`,
                // no root entry needed).
                let self_mode = fd
                    .receiver
                    .as_ref()
                    .map(|r| r.mode)
                    .unwrap_or(AccessMode::Read);

                let mut modes = BTreeMap::new();
                let mut state = StateMap::new();
                let mut fctx = FnCtx::new(fd.ret.clone(), local_pools.clone());
                fctx.insert_local("self".to_string(), self_ty.clone());
                modes.insert("self".to_string(), self_mode);
                for (am2, dm2) in info.members() {
                    if let (Member::Field(field), types::DeclMember::Field(_)) = (am2, dm2) {
                        state.insert(
                            StoragePath::root("self").field(field.name.clone()),
                            PathState::Uninit,
                        );
                    }
                }
                for (_ap, dp) in i.params.iter().zip(fd.params.iter()) {
                    modes.insert(dp.name.clone(), dp.mode);
                    state.insert(StoragePath::root(dp.name.clone()), PathState::Init);
                    fctx.insert_local(dp.name.clone(), dp.ty.clone());
                }
                let wctx = WCtx {
                    mctx,
                    effects,
                    modes,
                    is_init: true,
                };

                let mut dstack: DStack = Vec::new();
                let outcome = walk_block(&i.body, &mut state, &mut fctx, &wctx, &mut dstack, 0)?;
                check_body_exit(&outcome, &fctx, &wctx, i.span)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// The flow pass (plans/M2.md items E/F): fail-fast, source order, same
/// module-wide traversal shape as `bodies::check`/`access::check`.
/// `mctx` is shared with every other pass so item H's instantiation
/// queue accumulates across all of them.
pub(crate) fn check(
    module: &Module,
    decl_items: &[types::DeclItem],
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let effects = access::infer_effects_over(mctx);
    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();
    for (ai, di) in ast_items.iter().zip(decl_items.iter()) {
        match (ai, di) {
            (Item::Fn(f), types::DeclItem::Fn(d)) => check_top_fn(f, d, mctx, &effects)?,
            (Item::Struct(s), types::DeclItem::Struct(_)) => {
                check_struct_bodies(s, mctx, &effects)?
            }
            _ => {}
        }
    }
    Ok(())
}
