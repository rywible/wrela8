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
//! yet proven). It walks the **typed** tree (`walk_typed_*`); the
//! historical AST walkers were deleted once `check` only ever saw
//! `TypedProgram`.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::SemaError;
use crate::sema::access::{self, EffectMap};
use crate::sema::bodies::{self, FnCtx, ModuleCtx};
use crate::sema::paths::{PathStep, StoragePath, render_path};
use crate::sema::typed::{
    CalleeKey, TypedCallArg, TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn,
    TypedForIter, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind,
    TypedStruct,
};
use crate::sema::types::{self, Type};
use crate::syntax::ast::{AccessMode, Span};

/// Dumb, fixed cap on loop fixed-point iteration (mirrors
/// `MAX_GENERIC_DEPTH`'s sibling: dumb, fixed, no measurement).
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

fn init_error(message: String, span: Span) -> SemaError {
    SemaError::at("init", message, span)
}

fn move_error(message: String, span: Span) -> SemaError {
    SemaError::at("move", message, span)
}

fn overlap_error(message: String, span: Span) -> SemaError {
    SemaError::at("overlap", message, span)
}

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
///
/// plans/M7.md item E3 / 02-language.md §3.1's second bullet: protocol
/// resources (capabilities, permits, receipts — and E2's sealed queue
/// values) must be consumed, returned, or transferred on every path.
/// A live `Init` root of such a type at exit is `error[move]`.
fn check_exit_obligations(
    state: &StateMap,
    fctx: &FnCtx,
    wctx: &WCtx,
    span: Span,
) -> Result<(), SemaError> {
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
    check_protocol_consumption(state, fctx, wctx, span)
}

/// The protocol resource `ty` carries at any nesting, rendered — or
/// `None`. Sibling of `sema::types::type_contains_capability`: Option /
/// array / tuple / `Result` / `own` / `fn`, and a named type's declared
/// components (struct fields and enum payloads), with an `Actor[T]` cut
/// so a handle to a driver is not the driver's own `DeviceCap`.
///
/// plans/M7.md item I: `is_protocol_consuming_type` answered only the
/// root `Named` leaf, so `Option[DeviceCap[D]]`, `[DeviceCap[D]; 1]`, and
/// a plain `struct CapBundle { cap: DeviceCap[D] }` were droppable without
/// claim — the same one-wrapper-deep hole `DmaShared`'s lend rejection
/// had. The diagnostic names the *carried* type so a wrapper's spelling
/// still says why (`golden/err-cap-drop-option`, `err-cap-drop-wrapped`).
fn protocol_resource_carried(ty: &Type, mctx: &ModuleCtx) -> Option<String> {
    // plans/M13.md item O: re-derive from `must_consume` (`resource(manual)`
    // is must_consume by fiat).
    fn walk(ty: &Type, mctx: &ModuleCtx, seen: &mut BTreeSet<String>) -> Option<String> {
        use crate::sema::types::TypeArg;
        match ty {
            Type::Named(name, _) if crate::sema::classes::name_must_consume(name, false) => {
                Some(types::render_type(ty))
            }
            Type::Named(name, _) if name == "Actor" => None,
            Type::Array(elem, _) => walk(elem, mctx, seen),
            Type::Tuple(elems) => elems.iter().find_map(|e| walk(e, mctx, seen)),
            Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
                walk(inner, mctx, seen)
            }
            Type::Result(ok, err) => walk(ok, mctx, seen).or_else(|| walk(err, mctx, seen)),
            Type::Fn(params, ret) => params
                .iter()
                .find_map(|(_, t)| walk(t, mctx, seen))
                .or_else(|| walk(ret, mctx, seen)),
            Type::Named(name, targs) => {
                if !seen.insert(name.clone()) {
                    return None;
                }
                // `resource(manual)` fiat: the named type itself must_consume.
                if mctx
                    .structs
                    .get(name.as_str())
                    .is_some_and(|s| s.decl.is_manual_resource)
                {
                    seen.remove(name);
                    return Some(types::render_type(ty));
                }
                let via_fields = mctx
                    .structs
                    .get(name.as_str())
                    .and_then(|s| {
                        s.decl
                            .component_types
                            .iter()
                            .find_map(|(t, _)| walk(t, mctx, seen))
                    })
                    .or_else(|| {
                        mctx.enums.get(name.as_str()).and_then(|e| {
                            e.component_types
                                .iter()
                                .find_map(|(t, _)| walk(t, mctx, seen))
                        })
                    });
                let via_targs = targs.iter().find_map(|a| match a {
                    TypeArg::Type(t) => walk(t, mctx, seen),
                    _ => None,
                });
                let found = via_fields.or(via_targs);
                seen.remove(name);
                found
            }
            _ => None,
        }
    }
    walk(ty, mctx, &mut BTreeSet::new())
}

/// 02-language.md §3.1: "If its only consumers are protocol operations
/// (capabilities, permits, receipts), every control-flow path must
/// explicitly consume, return, or transfer it". A still-`Init` owned
/// root that *carries* a protocol-consuming type is a drop — illegal in
/// every state for a receipt, and the honesty hole E2 left open for a
/// dropped permit. Wrappers count (item I): see `protocol_resource_carried`.
fn check_protocol_consumption(
    state: &StateMap,
    fctx: &FnCtx,
    wctx: &WCtx,
    span: Span,
) -> Result<(), SemaError> {
    let mut checked: BTreeSet<String> = BTreeSet::new();
    for (path, st) in state {
        if *st != PathState::Init || !path.steps.is_empty() {
            continue;
        }
        let name = &path.root;
        if !checked.insert(name.clone()) {
            continue;
        }
        // Borrowed roots (`read` / `mut`) stay with the caller — only an
        // owned `take` parameter or an ordinary local can be dropped here.
        match wctx.modes.get(name) {
            Some(AccessMode::Read) | Some(AccessMode::Mut) => continue,
            Some(AccessMode::Take) | None => {}
        }
        let Some(ty) = fctx.lookup_local(name) else {
            continue;
        };
        let Some(found) = protocol_resource_carried(&ty, wctx.mctx) else {
            continue;
        };
        let rendered = types::render_type(&ty);
        let subject = if rendered == found {
            format!("`{name}` is a protocol resource (`{found}`)")
        } else {
            format!("`{name}` carries a protocol resource (`{found}`)")
        };
        return Err(move_error(
            format!(
                "{subject}; every path must consume, return, or \
                 transfer it (02-language.md §3.1 / 03-hardware.md §5) — dropping one is illegal"
            ),
            span,
        ));
    }
    Ok(())
}

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

fn join_outcomes(arms: Vec<Outcome>) -> Outcome {
    let mut breaks = Vec::new();
    let mut continues = Vec::new();
    let mut fallthroughs = Vec::new();
    for o in arms {
        breaks.extend(o.breaks);
        continues.extend(o.continues);
        if let Some(s) = o.fallthrough {
            fallthroughs.push(s);
        }
    }
    Outcome {
        fallthrough: if fallthroughs.is_empty() {
            None
        } else {
            Some(meet_all(fallthroughs))
        },
        breaks,
        continues,
    }
}

// --- module-wide checking context + entry points ---------------------------

/// Every parameter's (and, where applicable, `self`'s) declared access
/// mode, root-keyed — the only extra bookkeeping flow needs beyond
/// `bodies::ModuleCtx`/`FnCtx`.
struct WCtx<'a> {
    mctx: &'a ModuleCtx,
    effects: &'a EffectMap,
    modes: BTreeMap<String, AccessMode>,
    /// Whether this body is a struct's `init` (02-language.md §7.1).
    is_init: bool,
}

fn whole_or_state(path: &StoragePath, state: &StateMap, wctx: &WCtx<'_>) -> PathState {
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
    wctx: &WCtx<'_>,
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

fn check_takeable(
    path: &StoragePath,
    state: &StateMap,
    wctx: &WCtx<'_>,
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

fn check_overwrite_live(
    path: &StoragePath,
    ty: Option<&Type>,
    state: &StateMap,
    wctx: &WCtx<'_>,
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

fn check_storing_typed(value: &TypedExpr, fctx: &FnCtx, wctx: &WCtx<'_>) -> Result<(), SemaError> {
    let Some(path) = typed_as_path(value, fctx, wctx) else {
        return Ok(());
    };
    if bodies::is_resource_type(&value.ty, wctx.mctx) {
        return Err(move_error(
            format!(
                "`{}` is a resource; use `take` to move it here (an implicit copy is not allowed)",
                render_path(&path)
            ),
            value.span,
        ));
    }
    let _ = fctx;
    Ok(())
}

fn check_body_exit(
    outcome: &Outcome,
    fctx: &FnCtx,
    wctx: &WCtx<'_>,
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
    check_exit_obligations(final_state, fctx, wctx, span)
}

/// The flow pass (plans/M2.md items E/F): walks the typed tree. Uses
/// `TypedProgram::effects` for effective receiver modes.
pub(crate) fn check(program: &TypedProgram, mctx: &ModuleCtx) -> Result<(), SemaError> {
    let effects = &program.effects;
    for f in program.fns.values() {
        check_typed_fn_inner(f, mctx, effects, false)?;
    }
    for s in program.structs.values() {
        check_typed_struct(s, mctx, effects)?;
    }
    for e in program.enums.values() {
        for f in e.methods.values().chain(e.assoc_fns.values()) {
            check_typed_fn_inner(f, mctx, effects, false)?;
        }
    }
    Ok(())
}

pub(crate) fn check_typed_fn(
    f: &TypedFn,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> Result<(), SemaError> {
    check_typed_fn_inner(f, mctx, effects, false)
}

fn check_typed_fn_inner(
    f: &TypedFn,
    mctx: &ModuleCtx,
    effects: &EffectMap,
    is_init: bool,
) -> Result<(), SemaError> {
    let mut modes = BTreeMap::new();
    let mut state = StateMap::new();
    let mut fctx = FnCtx::new(f.ret.clone(), mctx.module_pools.clone());
    fctx.in_async = f.is_async;
    if let Some((mode, ty)) = &f.receiver {
        fctx.insert_local("self".to_string(), ty.clone());
        modes.insert("self".to_string(), *mode);
        if !is_init {
            state.insert(StoragePath::root("self"), PathState::Init);
        }
    }
    for p in &f.params {
        modes.insert(p.name.clone(), p.mode);
        state.insert(StoragePath::root(p.name.clone()), PathState::Init);
        fctx.insert_local(p.name.clone(), p.ty.clone());
    }
    let wctx = WCtx {
        mctx,
        effects,
        modes,
        is_init,
    };
    let mut dstack: TypedDStack = Vec::new();
    let outcome = walk_typed_block(&f.body, &mut state, &mut fctx, &wctx, &mut dstack, 0)?;
    let span = f.body.first().map(|s| s.span).unwrap_or_default();
    check_body_exit(&outcome, &fctx, &wctx, span)
}

pub(crate) fn check_typed_struct(
    s: &TypedStruct,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> Result<(), SemaError> {
    for f in s.methods.values().chain(s.assoc_fns.values()) {
        check_typed_fn_inner(f, mctx, effects, false)?;
    }
    if let Some(f) = &s.init {
        let mut modes = BTreeMap::new();
        let mut state = StateMap::new();
        let mut fctx = FnCtx::new(f.ret.clone(), mctx.module_pools.clone());
        if let Some((mode, ty)) = &f.receiver {
            fctx.insert_local("self".to_string(), ty.clone());
            modes.insert("self".to_string(), *mode);
            // init: fields start Uninit (02-language.md §7.1).
            for field in &s.fields {
                state.insert(
                    StoragePath::root("self").field(field.clone()),
                    PathState::Uninit,
                );
            }
        }
        for p in &f.params {
            modes.insert(p.name.clone(), p.mode);
            state.insert(StoragePath::root(p.name.clone()), PathState::Init);
            fctx.insert_local(p.name.clone(), p.ty.clone());
        }
        let wctx = WCtx {
            mctx,
            effects,
            modes,
            is_init: true,
        };
        let mut dstack: TypedDStack = Vec::new();
        let outcome = walk_typed_block(&f.body, &mut state, &mut fctx, &wctx, &mut dstack, 0)?;
        let span = f.body.first().map(|s| s.span).unwrap_or_default();
        check_body_exit(&outcome, &fctx, &wctx, span)?;
    }
    Ok(())
}

type TypedDStack<'a> = Vec<&'a TypedDeferBody>;

/// Re-validates every currently active `defer` at one real exit.
fn check_active_defers<'a>(
    active: &[&'a TypedDeferBody],
    exit_desc: &str,
    exit_state: &StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx<'_>,
) -> Result<(), SemaError> {
    let mut cur = exit_state.clone();
    for d in active.iter().rev() {
        let mut scratch: TypedDStack<'a> = Vec::new();
        let result = match d {
            TypedDeferBody::Expr(e) => walk_typed_expr(e, &mut cur, fctx, wctx, &mut scratch, 0),
            TypedDeferBody::Suite(stmts) => {
                walk_typed_block(stmts, &mut cur, fctx, wctx, &mut scratch, 0).map(|_| ())
            }
        };
        result.map_err(|mut err| {
            err.extra_lines.push(format!(
                "  this `defer` must be valid at every exit, including {exit_desc}"
            ));
            err
        })?;
    }
    Ok(())
}

fn typed_as_path(expr: &TypedExpr, fctx: &FnCtx, wctx: &WCtx<'_>) -> Option<StoragePath> {
    match &expr.kind {
        TypedExprKind::Local(name) => {
            if fctx.lookup_local(name).is_some() {
                Some(StoragePath::root(name.clone()))
            } else {
                None
            }
        }
        TypedExprKind::Field(base, name) => {
            if is_typed_method_reference(expr, fctx, wctx) {
                return None;
            }
            let base_path = typed_as_path(base, fctx, wctx)?;
            Some(base_path.field(name.clone()))
        }
        TypedExprKind::Index(base, idx) => {
            let step = match &idx.kind {
                TypedExprKind::Int(text) => match text.parse::<i128>() {
                    Ok(v) => PathStep::Index(v),
                    Err(_) => PathStep::RuntimeIndex,
                },
                _ => PathStep::RuntimeIndex,
            };
            Some(typed_as_path(base, fctx, wctx)?.index(step))
        }
        _ => None,
    }
}

fn is_typed_method_reference(expr: &TypedExpr, fctx: &FnCtx, wctx: &WCtx<'_>) -> bool {
    let TypedExprKind::Field(base, name) = &expr.kind else {
        return false;
    };
    let base_ty = bodies::unwrap_own(base.ty.clone());
    let Type::Named(sname, targs) = &base_ty else {
        return false;
    };
    let _ = fctx;
    if !targs.is_empty() {
        // Instantiated generic: treat as method if no field of that name
        // is known on the unspecialized decl (conservative).
        if let Some(s) = wctx.mctx.structs.get(sname.as_str()) {
            return s.field_ty(name).is_none()
                && (s.method(name).is_some() || s.assoc_fn(name).is_some());
        }
        return false;
    }
    if let Some(s) = wctx.mctx.structs.get(sname.as_str()) {
        return s.field_ty(name).is_none()
            && (s.method(name).is_some() || s.assoc_fn(name).is_some());
    }
    if let Some(e) = wctx.mctx.enums.get(sname.as_str()) {
        return e.method(name).is_some() || e.assoc_fn(name).is_some();
    }
    false
}

fn walk_typed_place_subexprs(
    expr: &TypedExpr,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx<'_>,
    dstack: &mut TypedDStack<'_>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    match &expr.kind {
        TypedExprKind::Field(base, _) => {
            walk_typed_place_subexprs(base, state, fctx, wctx, dstack, loop_marker)
        }
        TypedExprKind::Index(base, idx) => {
            walk_typed_place_subexprs(base, state, fctx, wctx, dstack, loop_marker)?;
            walk_typed_expr(idx, state, fctx, wctx, dstack, loop_marker)
        }
        _ => Ok(()),
    }
}

fn process_typed_operand(
    expr: &TypedExpr,
    mode: AccessMode,
    activated: &mut Vec<StoragePath>,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx<'_>,
    synthetic: bool,
    dstack: &mut TypedDStack<'_>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    let Some(path) = typed_as_path(expr, fctx, wctx) else {
        return walk_typed_expr(expr, state, fctx, wctx, dstack, loop_marker);
    };
    walk_typed_place_subexprs(expr, state, fctx, wctx, dstack, loop_marker)?;
    match mode {
        AccessMode::Take => {
            check_no_overlap(&path, activated, expr.span)?;
            check_takeable(&path, state, wctx, expr.span)?;
            set_state(&path, state, PathState::Moved);
            activated.push(path);
        }
        AccessMode::Read | AccessMode::Mut => {
            check_readable(&path, state, wctx, expr.span)?;
            check_no_overlap(&path, activated, expr.span)?;
            if synthetic && mode == AccessMode::Read {
                check_storing_typed(expr, fctx, wctx)?;
            }
            if mode == AccessMode::Mut {
                activated.push(path);
            }
        }
    }
    Ok(())
}

fn walk_storing_typed(
    value: &TypedExpr,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx<'_>,
    dstack: &mut TypedDStack<'_>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    let mut activated = Vec::new();
    process_typed_operand(
        value,
        AccessMode::Read,
        &mut activated,
        state,
        fctx,
        wctx,
        true,
        dstack,
        loop_marker,
    )
}

fn receiver_mode_for_callee(callee: &CalleeKey, wctx: &WCtx<'_>) -> Option<AccessMode> {
    let (owner, method) = match callee {
        CalleeKey::Method(o, m) => (o.as_str(), m.as_str()),
        CalleeKey::MethodInstance(o, m) => {
            let bare = o
                .strip_prefix("struct:")
                .or_else(|| o.strip_prefix("enum:"))
                .unwrap_or(o.as_str())
                .split('[')
                .next()
                .unwrap_or(o.as_str());
            (bare, m.as_str())
        }
        _ => return None,
    };
    // 03 §9 bring-up states are builtins with no DeclStruct. Their
    // consuming transitions must still Take the receiver for
    // protocol-consumption (same table as Intrinsic keys below).
    if crate::eval::image_checks::is_protocol_state_type_name(owner) {
        return Some(protocol_state_method_mode(owner, method));
    }
    if let Some(s) = wctx.mctx.structs.get(owner) {
        if let Some((af, d)) = s.method(method) {
            return access::resolve_receiver_mode(af, d, owner, wctx.mctx, wctx.effects).ok();
        }
    }
    if let Some(e) = wctx.mctx.enums.get(owner) {
        if let Some((af, d)) = e.method(method) {
            return access::resolve_receiver_mode(af, d, owner, wctx.mctx, wctx.effects).ok();
        }
    }
    None
}

/// Effective receiver mode for a 03 §9 bring-up state transition
/// (historical AST `receiver_of` table). `take_irq` is Take only on the
/// claimed-only mint (no negotiate/start). After `negotiate` — and after
/// `VirtQueue.configure` rebinds the local to `QueuesConfiguredDevice` —
/// it must be Read so `start` can still consume the state. (AST flow
/// never saw the configure rebind, so its table only named
/// `FeaturesAccepted*`; typed flow must also keep `QueuesConfigured*`.)
fn protocol_state_method_mode(state: &str, method: &str) -> AccessMode {
    match method {
        "negotiate" | "start" | "reset" => AccessMode::Take,
        "map_partition" | "read_capacity_sectors" => AccessMode::Read,
        "take_irq"
            if state.starts_with("FeaturesAccepted") || state.starts_with("QueuesConfigured") =>
        {
            AccessMode::Read
        }
        "take_irq" => AccessMode::Take,
        _ => AccessMode::Read,
    }
}

/// Receiver mode for a sealed-transport `Intrinsic`, keyed by the
/// intrinsic spelling plus the receiver's protocol-state type name.
fn intrinsic_receiver_mode(key: &str, receiver: &TypedExpr) -> AccessMode {
    let place = match &receiver.kind {
        TypedExprKind::Take(inner) => inner.as_ref(),
        _ => receiver,
    };
    let base_ty = bodies::unwrap_own(place.ty.clone());
    let Type::Named(sname, _) = &base_ty else {
        return AccessMode::Read;
    };
    if !crate::eval::image_checks::is_protocol_state_type_name(sname) {
        return AccessMode::Read;
    }
    let method = key.strip_prefix("Device.").unwrap_or(key);
    protocol_state_method_mode(sname, method)
}

fn walk_typed_call(
    callee: &CalleeKey,
    receiver: Option<&TypedExpr>,
    args: &[TypedCallArg],
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx<'_>,
    dstack: &mut TypedDStack<'_>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    let mut activated: Vec<StoragePath> = Vec::new();
    if let Some(recv) = receiver {
        let mode = receiver_mode_for_callee(callee, wctx).unwrap_or(AccessMode::Read);
        process_typed_operand(
            recv,
            mode,
            &mut activated,
            state,
            fctx,
            wctx,
            false,
            dstack,
            loop_marker,
        )?;
    }
    for a in args {
        if let Some(v) = &a.value {
            process_typed_operand(
                v,
                a.mode,
                &mut activated,
                state,
                fctx,
                wctx,
                false,
                dstack,
                loop_marker,
            )?;
        }
    }
    Ok(())
}

fn pattern_has_take(p: &TypedPattern) -> bool {
    match &p.kind {
        TypedPatternKind::Take(_) => true,
        TypedPatternKind::Wildcard
        | TypedPatternKind::Literal(_)
        | TypedPatternKind::Binding(_) => false,
        TypedPatternKind::Variant { payload, .. }
        | TypedPatternKind::Tuple(payload)
        | TypedPatternKind::Array(payload)
        | TypedPatternKind::Or(payload) => payload.iter().any(pattern_has_take),
    }
}

/// Applies a pattern's move effect to the scrutinee place (plans/M7.md
/// item I). Same two reasons as the former AST `apply_pattern_move`:
/// 1. `take` appears anywhere in the pattern.
/// 2. The scrutinee *carries* a protocol resource but is not itself one
///    by name (`Option[Receipt]`, …) — destructuring is how the inner
///    value becomes reachable; leaving the wrapper `Init` would launder
///    the drop check.
fn apply_typed_pattern_move(
    scrutinee: &TypedExpr,
    pattern: &TypedPattern,
    state: &mut StateMap,
    fctx: &FnCtx,
    wctx: &WCtx<'_>,
    span: Span,
) -> Result<(), SemaError> {
    let scrutinee_ty = &scrutinee.ty;
    let root_must_consume = matches!(
        scrutinee_ty,
        Type::Named(n, _) if crate::sema::classes::name_must_consume(n, false)
    );
    let move_protocol_wrapper =
        protocol_resource_carried(scrutinee_ty, wctx.mctx).is_some() && !root_must_consume;
    if !pattern_has_take(pattern) && !move_protocol_wrapper {
        return Ok(());
    }
    let Some(path) = typed_as_path(scrutinee, fctx, wctx) else {
        return Ok(());
    };
    check_takeable(&path, state, wctx, span)?;
    set_state(&path, state, PathState::Moved);
    Ok(())
}

fn bind_typed_pattern_flow(p: &TypedPattern, state: &mut StateMap, fctx: &mut FnCtx) {
    match &p.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Literal(_) => {}
        TypedPatternKind::Binding(name) => {
            fctx.insert_local(name.clone(), p.ty.clone());
            state.insert(StoragePath::root(name.clone()), PathState::Init);
        }
        TypedPatternKind::Take(inner) => bind_typed_pattern_flow(inner, state, fctx),
        TypedPatternKind::Or(alts) => {
            if let Some(first) = alts.first() {
                bind_typed_pattern_flow(first, state, fctx);
            }
        }
        TypedPatternKind::Variant { payload, .. }
        | TypedPatternKind::Tuple(payload)
        | TypedPatternKind::Array(payload) => {
            for sp in payload {
                bind_typed_pattern_flow(sp, state, fctx);
            }
        }
    }
}

fn walk_typed_block<'a>(
    body: &'a [TypedStmt],
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx<'_>,
    dstack: &mut TypedDStack<'a>,
    loop_marker: usize,
) -> Result<Outcome, SemaError> {
    let entry_len = dstack.len();
    let mut breaks = Vec::new();
    let mut continues = Vec::new();
    for stmt in body {
        if let TypedStmtKind::Defer(d) = &stmt.kind {
            dstack.push(d);
        }
        let out = walk_typed_stmt(stmt, state, fctx, wctx, dstack, loop_marker)?;
        breaks.extend(out.breaks);
        continues.extend(out.continues);
        match out.fallthrough {
            Some(new_state) => *state = new_state,
            None => {
                dstack.truncate(entry_len);
                return Ok(Outcome {
                    fallthrough: None,
                    breaks,
                    continues,
                });
            }
        }
    }
    check_active_defers(
        &dstack[entry_len..],
        "this block's own normal completion",
        state,
        fctx,
        wctx,
    )?;
    dstack.truncate(entry_len);
    Ok(Outcome {
        fallthrough: Some(state.clone()),
        breaks,
        continues,
    })
}

fn is_err_return_typed(e: &TypedExpr) -> bool {
    match &e.kind {
        TypedExprKind::EnumConstruct { variant, .. } => variant == "Err",
        TypedExprKind::Call {
            callee: CalleeKey::Fn(n),
            ..
        } => n == "Err",
        _ => false,
    }
}

fn walk_typed_stmt<'a>(
    stmt: &'a TypedStmt,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx<'_>,
    dstack: &mut TypedDStack<'a>,
    loop_marker: usize,
) -> Result<Outcome, SemaError> {
    match &stmt.kind {
        TypedStmtKind::Let { name, ty, value } => {
            walk_storing_typed(value, state, fctx, wctx, dstack, loop_marker)?;
            fctx.insert_local(name.clone(), ty.clone());
            state.insert(StoragePath::root(name.clone()), PathState::Init);
            Ok(fallthrough(state.clone()))
        }
        TypedStmtKind::Assign { target, value } => {
            walk_storing_typed(value, state, fctx, wctx, dstack, loop_marker)?;
            if let Some(path) = typed_as_path(target, fctx, wctx) {
                if let Some(mode) = wctx.modes.get(&path.root) {
                    if *mode == AccessMode::Read {
                        return Err(init_error(
                            format!(
                                "`{}` is a `read` parameter; it cannot be assigned",
                                path.root
                            ),
                            target.span,
                        ));
                    }
                }
                check_overwrite_live(&path, Some(&target.ty), state, wctx, target.span)?;
                set_state(&path, state, PathState::Init);
            } else {
                walk_typed_expr(target, state, fctx, wctx, dstack, loop_marker)?;
            }
            Ok(fallthrough(state.clone()))
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            let mut outcomes = Vec::new();
            let mut st = state.clone();
            walk_typed_expr(cond, &mut st, fctx, wctx, dstack, loop_marker)?;
            outcomes.push(bodies::scoped(fctx, |fctx| {
                walk_typed_block(then_branch, &mut st, fctx, wctx, dstack, loop_marker)
            })?);
            for e in elifs {
                let mut st = state.clone();
                walk_typed_expr(&e.cond, &mut st, fctx, wctx, dstack, loop_marker)?;
                outcomes.push(bodies::scoped(fctx, |fctx| {
                    walk_typed_block(&e.body, &mut st, fctx, wctx, dstack, loop_marker)
                })?);
            }
            match else_branch {
                Some(b) => {
                    let mut st = state.clone();
                    outcomes.push(bodies::scoped(fctx, |fctx| {
                        walk_typed_block(b, &mut st, fctx, wctx, dstack, loop_marker)
                    })?);
                }
                None => outcomes.push(fallthrough(state.clone())),
            }
            Ok(join_outcomes(outcomes))
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            let mut entry = state.clone();
            walk_typed_expr(scrutinee, &mut entry, fctx, wctx, dstack, loop_marker)?;
            let mut outs = Vec::new();
            for arm in arms {
                let mut st = entry.clone();
                let outcome = bodies::scoped(fctx, |fctx| {
                    apply_typed_pattern_move(
                        scrutinee,
                        &arm.pattern,
                        &mut st,
                        fctx,
                        wctx,
                        arm.pattern.span,
                    )?;
                    bind_typed_pattern_flow(&arm.pattern, &mut st, fctx);
                    if let Some(g) = &arm.guard {
                        walk_typed_expr(g, &mut st, fctx, wctx, dstack, loop_marker)?;
                    }
                    walk_typed_block(&arm.body, &mut st, fctx, wctx, dstack, loop_marker)
                })?;
                outs.push(outcome);
            }
            Ok(join_outcomes(outs))
        }
        TypedStmtKind::For {
            name,
            elem_ty,
            take_binding,
            iter,
            body,
            ..
        } => walk_typed_for(
            name,
            elem_ty,
            *take_binding,
            iter,
            body,
            state,
            fctx,
            wctx,
            dstack,
        ),
        TypedStmtKind::While { cond, body, .. } => {
            walk_typed_while(cond, body, state, fctx, wctx, dstack)
        }
        TypedStmtKind::Break => {
            check_active_defers(&dstack[loop_marker..], "a `break` exit", state, fctx, wctx)?;
            Ok(Outcome {
                fallthrough: None,
                breaks: vec![state.clone()],
                continues: Vec::new(),
            })
        }
        TypedStmtKind::Continue => {
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
        TypedStmtKind::Pass => Ok(fallthrough(state.clone())),
        TypedStmtKind::Return(v) => {
            if let Some(e) = v {
                walk_storing_typed(e, state, fctx, wctx, dstack, loop_marker)?;
            }
            check_active_defers(dstack, "a `return` exit", state, fctx, wctx)?;
            if wctx.is_init && v.as_ref().is_some_and(is_err_return_typed) {
                return Ok(Outcome {
                    fallthrough: None,
                    breaks: Vec::new(),
                    continues: Vec::new(),
                });
            }
            check_exit_obligations(state, fctx, wctx, stmt.span)?;
            Ok(Outcome {
                fallthrough: None,
                breaks: Vec::new(),
                continues: Vec::new(),
            })
        }
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            walk_typed_expr(cond, state, fctx, wctx, dstack, loop_marker)?;
            if let Some(m) = message {
                walk_typed_expr(m, state, fctx, wctx, dstack, loop_marker)?;
            }
            Ok(fallthrough(state.clone()))
        }
        TypedStmtKind::Defer(_) => Ok(fallthrough(state.clone())),
        TypedStmtKind::ExprStmt(e) | TypedStmtKind::BareSend { expr: e, .. } => {
            walk_typed_expr(e, state, fctx, wctx, dstack, loop_marker)?;
            // 02-language.md §11: `panic` abandons — `never`-typed
            // statements have no fallthrough (same as `return`).
            if matches!(e.ty, Type::Never) {
                return Ok(Outcome {
                    fallthrough: None,
                    breaks: Vec::new(),
                    continues: Vec::new(),
                });
            }
            Ok(fallthrough(state.clone()))
        }
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            as_name,
            body,
        } => {
            if let Some(c) = capacity {
                walk_typed_expr(c, state, fctx, wctx, dstack, loop_marker)?;
            }
            if let Some(d) = deadline {
                walk_typed_expr(d, state, fctx, wctx, dstack, loop_marker)?;
            }
            bodies::scoped(fctx, |fctx| {
                if let Some(name) = as_name {
                    fctx.insert_local(name.clone(), Type::Named("Group".to_string(), vec![]));
                    state.insert(StoragePath::root(name.clone()), PathState::Init);
                }
                walk_typed_block(body, state, fctx, wctx, dstack, loop_marker)
            })
        }
    }
}

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

fn prune_loop_locals(st: &mut StateMap, baseline_roots: &BTreeSet<String>) {
    st.retain(|p, _| baseline_roots.contains(&p.root));
}

fn walk_typed_while<'a>(
    cond: &'a TypedExpr,
    body: &'a [TypedStmt],
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx<'_>,
    dstack: &mut TypedDStack<'a>,
) -> Result<Outcome, SemaError> {
    let body_marker = dstack.len();
    let baseline_roots: BTreeSet<String> = state.keys().map(|p| p.root.clone()).collect();
    let mut candidate = state.clone();
    let mut out = fallthrough(state.clone());
    let fixed_point = bodies::scoped(fctx, |fctx| {
        for _ in 0..LOOP_FIXED_POINT_CAP {
            let mut st = candidate.clone();
            prune_loop_locals(&mut st, &baseline_roots);
            walk_typed_expr(cond, &mut st, fctx, wctx, dstack, body_marker)?;
            let o = walk_typed_block(body, &mut st, fctx, wctx, dstack, body_marker)?;
            let next = loop_backedge(&o).unwrap_or_else(|| candidate.clone());
            let converged = next == candidate;
            candidate = next;
            out = o;
            if converged {
                break;
            }
        }
        Ok(())
    });
    fixed_point?;
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

fn walk_typed_for<'a>(
    name: &str,
    elem_ty: &Type,
    take_binding: bool,
    iter: &'a TypedForIter,
    body: &'a [TypedStmt],
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx<'_>,
    dstack: &mut TypedDStack<'a>,
) -> Result<Outcome, SemaError> {
    let mut entry = state.clone();
    let body_marker = dstack.len();
    match iter {
        TypedForIter::Range(a, b, _) => {
            walk_typed_expr(a, &mut entry, fctx, wctx, dstack, body_marker)?;
            walk_typed_expr(b, &mut entry, fctx, wctx, dstack, body_marker)?;
        }
        TypedForIter::Expr(e) => {
            if take_binding {
                if let TypedExprKind::Take(inner) = &e.kind {
                    if let Some(path) = typed_as_path(inner, fctx, wctx) {
                        walk_typed_place_subexprs(
                            inner,
                            &mut entry,
                            fctx,
                            wctx,
                            dstack,
                            body_marker,
                        )?;
                        check_takeable(&path, &entry, wctx, e.span)?;
                        set_state(&path, &mut entry, PathState::Moved);
                    } else {
                        walk_typed_expr(inner, &mut entry, fctx, wctx, dstack, body_marker)?;
                    }
                } else {
                    walk_typed_expr(e, &mut entry, fctx, wctx, dstack, body_marker)?;
                }
            } else {
                walk_typed_expr(e, &mut entry, fctx, wctx, dstack, body_marker)?;
            }
        }
    }

    let baseline_roots: BTreeSet<String> = entry.keys().map(|p| p.root.clone()).collect();
    let mut candidate = entry.clone();
    let mut out = fallthrough(entry.clone());
    let fixed_point = bodies::scoped(fctx, |fctx| {
        fctx.insert_local(name.to_string(), elem_ty.clone());
        for _ in 0..LOOP_FIXED_POINT_CAP {
            let mut st = candidate.clone();
            prune_loop_locals(&mut st, &baseline_roots);
            st.insert(StoragePath::root(name.to_string()), PathState::Init);
            let o = walk_typed_block(body, &mut st, fctx, wctx, dstack, body_marker)?;
            let next = loop_backedge(&o).unwrap_or_else(|| candidate.clone());
            let converged = next == candidate;
            candidate = next;
            out = o;
            if converged {
                break;
            }
        }
        Ok(())
    });
    fixed_point?;

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

fn walk_typed_expr(
    expr: &TypedExpr,
    state: &mut StateMap,
    fctx: &mut FnCtx,
    wctx: &WCtx<'_>,
    dstack: &mut TypedDStack<'_>,
    loop_marker: usize,
) -> Result<(), SemaError> {
    if let Some(path) = typed_as_path(expr, fctx, wctx) {
        // Bare place use: must be readable (unless this is a field/index
        // chain walked only for path construction — those recurse below).
        if matches!(
            &expr.kind,
            TypedExprKind::Local(_) | TypedExprKind::Field(_, _) | TypedExprKind::Index(_, _)
        ) {
            // Field/index: check the path itself when used as a value.
            check_readable(&path, state, wctx, expr.span)?;
            return walk_typed_place_subexprs(expr, state, fctx, wctx, dstack, loop_marker);
        }
    }
    match &expr.kind {
        TypedExprKind::Take(inner) => {
            if let Some(path) = typed_as_path(inner, fctx, wctx) {
                walk_typed_place_subexprs(inner, state, fctx, wctx, dstack, loop_marker)?;
                check_takeable(&path, state, wctx, expr.span)?;
                set_state(&path, state, PathState::Moved);
            } else {
                walk_typed_expr(inner, state, fctx, wctx, dstack, loop_marker)?;
            }
            Ok(())
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => walk_typed_call(
            callee,
            receiver.as_deref(),
            args,
            state,
            fctx,
            wctx,
            dstack,
            loop_marker,
        ),
        TypedExprKind::CallValue(c, args) => {
            walk_typed_expr(c, state, fctx, wctx, dstack, loop_marker)?;
            let mut activated = Vec::new();
            for a in args {
                if let Some(v) = &a.value {
                    process_typed_operand(
                        v,
                        a.mode,
                        &mut activated,
                        state,
                        fctx,
                        wctx,
                        false,
                        dstack,
                        loop_marker,
                    )?;
                }
            }
            Ok(())
        }
        TypedExprKind::Try(inner, _) => {
            walk_typed_expr(inner, state, fctx, wctx, dstack, loop_marker)?;
            // `?` is a potential early exit: active defers must be valid.
            check_active_defers(dstack, "a `?` exit", state, fctx, wctx)
        }
        TypedExprKind::Await(inner) => {
            // plans/M7.md item E4 / 03-hardware.md §5: `await receipt`
            // consumes the `Receipt[P]`. Actor-call awaits are not
            // places — fall through.
            if let Some(path) = typed_as_path(inner, fctx, wctx) {
                if matches!(
                    &inner.ty,
                    Type::Named(n, _) if crate::sema::classes::name_must_consume(n, false)
                ) {
                    walk_typed_place_subexprs(inner, state, fctx, wctx, dstack, loop_marker)?;
                    check_takeable(&path, state, wctx, expr.span)?;
                    set_state(&path, state, PathState::Moved);
                    return Ok(());
                }
            }
            walk_typed_expr(inner, state, fctx, wctx, dstack, loop_marker)
        }
        TypedExprKind::Field(base, _)
        | TypedExprKind::ToScalar(base)
        | TypedExprKind::Neg(base)
        | TypedExprKind::BitNot(base)
        | TypedExprKind::Not(base)
        | TypedExprKind::Panic(base)
        | TypedExprKind::Send(base) => {
            walk_typed_expr(base, state, fctx, wctx, dstack, loop_marker)
        }
        TypedExprKind::Index(base, idx) => {
            walk_typed_expr(base, state, fctx, wctx, dstack, loop_marker)?;
            walk_typed_expr(idx, state, fctx, wctx, dstack, loop_marker)
        }
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::OpCall(_, l, r)
        | TypedExprKind::And(l, r)
        | TypedExprKind::Or(l, r) => {
            walk_typed_expr(l, state, fctx, wctx, dstack, loop_marker)?;
            walk_typed_expr(r, state, fctx, wctx, dstack, loop_marker)
        }
        TypedExprKind::Is(s, pat) => {
            walk_typed_expr(s, state, fctx, wctx, dstack, loop_marker)?;
            apply_typed_pattern_move(s, pat, state, fctx, wctx, expr.span)?;
            Ok(())
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            let mut activated = Vec::new();
            for a in args {
                if let Some(v) = &a.value {
                    process_typed_operand(
                        v,
                        a.mode,
                        &mut activated,
                        state,
                        fctx,
                        wctx,
                        true,
                        dstack,
                        loop_marker,
                    )?;
                }
            }
            Ok(())
        }
        TypedExprKind::Closure { body, .. } => match body {
            TypedClosureBody::Expr(e) => walk_typed_expr(e, state, fctx, wctx, dstack, loop_marker),
            TypedClosureBody::Suite(stmts) => {
                let mut nested: TypedDStack = Vec::new();
                walk_typed_block(stmts, state, fctx, wctx, &mut nested, 0)?;
                Ok(())
            }
        },
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            let mut activated = Vec::new();
            for i in items {
                process_typed_operand(
                    i,
                    AccessMode::Read,
                    &mut activated,
                    state,
                    fctx,
                    wctx,
                    true,
                    dstack,
                    loop_marker,
                )?;
            }
            Ok(())
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            let mut activated = Vec::new();
            for (_, v) in fields {
                process_typed_operand(
                    v,
                    AccessMode::Read,
                    &mut activated,
                    state,
                    fctx,
                    wctx,
                    true,
                    dstack,
                    loop_marker,
                )?;
            }
            Ok(())
        }
        TypedExprKind::Intrinsic {
            key,
            receiver,
            args,
            ..
        } => {
            // Same operand discipline as `walk_typed_call`: take-mode
            // receivers (03 §9 `take_irq` / negotiate / start / reset)
            // and take-wrapped args (`with_arg_mode` → `TypedExprKind::Take`)
            // must mark roots Moved, or protocol-consumption falsely
            // reports a live `claimed` after the terminal mint.
            let mut activated: Vec<StoragePath> = Vec::new();
            if let Some(r) = receiver {
                let mode = intrinsic_receiver_mode(key, r);
                process_typed_operand(
                    r,
                    mode,
                    &mut activated,
                    state,
                    fctx,
                    wctx,
                    false,
                    dstack,
                    loop_marker,
                )?;
            }
            for (_, a) in args {
                // Intrinsic args have no mode slot; take-mode sites wrap
                // the value in `TypedExprKind::Take` at construction.
                walk_typed_expr(a, state, fctx, wctx, dstack, loop_marker)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
