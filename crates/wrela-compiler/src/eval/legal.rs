//! Legality inference (plans/M3.md item C, decision 7): whole-graph
//! comptime-callable classification computed directly from the typed
//! tree's own callee keys (`sema::typed::CalleeKey`) — no re-derivation
//! from the ast, no annotation, no constraint solver.
//!
//! 02-language.md §12: "A plain `fn` is comptime-callable when its
//! transitive closure is deterministic and free of I/O, async/actor
//! operations, and hardware effects; legality is inferred (no
//! annotation) and violations are diagnosed with the offending call
//! path." Decision 7 pins the concrete illegal set for M3's world:
//! `await`/`send`/actor operations, `with` group/pool, pool operations,
//! `@image` bodies, and anything still fail-closed.
//!
//! ## Illegal operations in the typed tree
//!
//! Decision 7's illegal set is now representable: `await`/`send`/
//! `with group`/restricted `@image` builder intrinsics/`now()`/MMIO
//! and related hardware effects all exist as typed nodes. Whole-graph
//! classification walks the typed tree once, driven by an effect-class →
//! allowed-contexts table (`effect_reason` below). Contexts today are
//! comptime, ISR, and bottom-half (`@task`); hardware provenance is the
//! same walk with a capability-authority visitor. A new `TypedExprKind`/
//! `TypedStmtKind` variant forces an arm in the shared walk
//! (`eval::walk`) and, when it carries an effect, a row in the table.
//!
//! ## What "closure" means for classification
//!
//! Decision 4 (plans/M3.md): closures are non-escaping, direct
//! application — there is no separate graph node for a closure literal,
//! no `CalleeKey` names one. Wherever a `TypedExprKind::Closure` appears
//! (called directly through a `CallValue`, or merely constructed and
//! passed along), its body is scanned as part of whichever fn/method/
//! instantiation body it was found in — the enclosing node's own
//! transitive closure absorbs it, exactly like plans/M3.md's own phrase
//! ("a closure's body is part of its enclosing fn's closure-set").
//!
//! ## Known scope boundary: field defaults
//!
//! A struct literal only carries its *explicitly supplied* fields
//! (typed.rs: an omitted, defaulted field is elided, its default living
//! once on `TypedStruct::field_defaults`) — so a fn that constructs a
//! struct while leaving a defaulted field to its default does not, in
//! its own typed body, carry whatever that default itself calls. That
//! boundary still stands for *field* defaults.
//!
//! **Parameter defaults are not in that boundary** (plans/M9.md item K1):
//! `TypedParam::default` is scanned with the fn's body for both
//! comptime legality and ISR effects. Call sites store `None` for an
//! omitted slot, so the intrinsic/`now()` lives only on the param; a
//! body-only scan let `fn stamp(t: Instant = now())` look Legal and
//! ISR-clean while an omitted-arg call still evaluated `now()`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::sema::SemaError;
use crate::sema::typed::{
    TypedExpr, TypedExprKind, TypedFn, TypedForIter, TypedInstantiation, TypedProgram, TypedStmt,
    TypedStmtKind,
};
use crate::syntax::ast::Span;

// --- the public shape (shaped for M3-D/E's consumers) ---------------------

/// One node's classification: comptime-callable, or not — with the call
/// path from the node itself down to the offending operation (decision
/// 7's "offending call path", the generics chain-diagnostic precedent
/// applied to legality instead of a missing requirement).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Legal,
    Illegal { path: Vec<String>, reason: String },
}

/// The whole program's classification (`classify` below): every
/// function/method/instantiation key the typed tree carries, mapped to
/// its own `Verdict` — computed once, over the whole callee graph, so
/// `comptime if`/`comptime assert`/const-initializer checking (M3-D)
/// just looks a key up instead of re-walking anything.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Legality {
    pub verdicts: BTreeMap<String, Verdict>,
}

impl Legality {
    /// A key `classify` never registered a node for (an unresolved/
    /// builtin callee outside the typed program's own maps) is treated
    /// as legal by default: unknowns are not decision-7 illegal
    /// operations, and there is no information to the contrary.
    pub fn verdict(&self, key: &str) -> Verdict {
        self.verdicts.get(key).cloned().unwrap_or(Verdict::Legal)
    }
}

/// Checks that `key` (a `CalleeKey::spelling()`-shaped string — a fn
/// name, `Struct.method`, or a `generics::canonical_key` instantiation
/// spelling, same namespace `classify` itself keys `Legality` by) is
/// comptime-callable, producing the house-format diagnostic if not.
/// `context` names the comptime site requiring legality (e.g.
/// `"comptime assert"`, `"const LIMIT"`, `"comptime if"` — M3-D's own
/// call sites choose the wording); `span` is that *context*'s own
/// location, since a typed node carries none of its own (typed.rs:
/// decision 1 drops spans entirely) — the caller always has one (the
/// `const`/`comptime` site itself, still on the ast).
pub fn require_legal(
    legality: &Legality,
    key: &str,
    context: &str,
    span: Span,
) -> Result<(), SemaError> {
    match legality.verdict(key) {
        Verdict::Legal => Ok(()),
        Verdict::Illegal { path, reason } => {
            let mut extra_lines = Vec::new();
            for hop in path.windows(2) {
                extra_lines.push(format!("  `{}` calls `{}`", hop[0], hop[1]));
            }
            let last = path.last().cloned().unwrap_or_else(|| key.to_string());
            extra_lines.push(format!("  `{last}` uses {reason}"));
            Err(SemaError {
                category: "comptime",
                message: format!(
                    "`{context}` requires a comptime-legal closure, but `{key}` is not \
                     comptime-callable"
                ),
                line: span.line,
                col: span.col,
                extra_lines,
                omit_location: false,
                missing_method: None,
            })
        }
    }
}

/// The result of scanning one standalone expression that is not itself a
/// node `classify`'s own graph tracks — a `const` initializer, a
/// `comptime assert`'s condition/message (plans/M3.md item D's own
/// legality-wiring surface: `sema::mod::check_typed`/`eval::check_consts`
/// `require_legal` every key `callees` names against the program-wide
/// `Legality` `classify` already computed, which accounts for each
/// callee's own transitive closure — no second graph, no re-walk beyond
/// this one expression). `illegal` is plans/M4.md item B's own addition:
/// a builder intrinsic used directly in a `const` initializer or
/// `comptime assert` carries no `CalleeKey` at all (it is not a `Call`),
/// so `callees` alone cannot catch it the way it catches an illegal
/// *callee*; this is the direct, first-hand "this expression itself
/// commits a decision-7/`@image`-only violation" answer, mirroring
/// `NodeInfo::illegal` one level up.
pub struct StandaloneScan {
    pub callees: BTreeSet<String>,
    pub illegal: Option<String>,
}

/// Scans one standalone expression, reusing the identical exhaustive
/// `scan_expr` this module already maintains for `classify` itself, so a
/// new `TypedExprKind` variant forces an arm here for free.
pub fn scan_standalone(expr: &TypedExpr) -> StandaloneScan {
    let mut scan = BodyScan::default();
    scan_expr(expr, &mut scan);
    StandaloneScan {
        callees: scan.callees,
        illegal: scan.illegal,
    }
}

/// Scans one whole statement body (a fn/method's own, never a nested
/// closure's — decision 4 folds a closure's body into whichever
/// enclosing fn constructed it, exactly like `classify`'s own
/// `insert_fn_node`) for its direct callees, reusing the identical
/// exhaustive `scan_stmts` `classify` builds every node from. plans/M4.md
/// item B's own `eval::check_image_legality` uses this on the one
/// reachable `@image` fn's body directly: `classify` already exempts
/// that fn's own node from being marked illegal by a *directly*-written
/// intrinsic (decision 5), but nothing before this call site ever
/// actually required that fn's own verdict to be legal — this is what
/// lets a transitively-illegal callee (one this fn merely calls) still
/// reject the build, exactly like a `const`/`@test` already would.
pub fn direct_callees_of_body(body: &[TypedStmt]) -> BTreeSet<String> {
    let mut scan = BodyScan::default();
    scan_stmts(body, &mut scan);
    scan.callees
}

/// Classifies every fn/method/instantiation key the typed program
/// carries (decision 7): builds the whole-program callee graph directly
/// from typed callee keys, scans each node's own body for a decision-7
/// illegal operation (today: none are representable — see the module
/// doc), then propagates "illegal" through the graph to a fixed point.
/// `BTreeMap`/`BTreeSet` throughout (CLAUDE.md): iteration order, and so
/// which callee's path wins when several are illegal, is deterministic.
pub fn classify(program: &TypedProgram) -> Legality {
    let mut nodes = build_nodes(program);
    // plans/M4.md item B, decision 5: the one reachable `@image` fn
    // (`TypedProgram::image_fn`) is the *only* place a builder intrinsic
    // is legal — its own node's scan above still flags it exactly like
    // any other body that directly contains one (the exhaustive-match
    // discipline the module doc describes applies uniformly), so that
    // flag is cleared here, once, rather than teaching the scan itself
    // about "which fn is currently being scanned." A helper fn the
    // `@image` fn merely calls is *not* exempted (a deliberate, narrow
    // scope boundary: every worked example spells the whole builder
    // sequence directly in the `@image` fn's own body; the disclosed
    // corollary is that a program factoring builder calls into a helper
    // fn is not yet supported by item B).
    if let Some(image_fn) = &program.image_fn {
        if let Some(info) = nodes.get_mut(image_fn) {
            info.illegal = None;
        }
    }
    let mut verdicts: BTreeMap<String, Verdict> = BTreeMap::new();
    for (key, info) in &nodes {
        let verdict = match &info.illegal {
            Some(reason) => Verdict::Illegal {
                path: vec![info.display_name.clone()],
                reason: reason.clone(),
            },
            None => Verdict::Legal,
        };
        verdicts.insert(key.clone(), verdict);
    }

    // Fixed-point propagation. Recursion/cycles are legal by decision 7
    // (quotas bound them at eval time, plans/M3.md) — this is a plain
    // reachability closure, not a cycle-sensitive analysis: "illegal" is
    // monotonic (a node, once marked, never un-marks), so a finite graph
    // reaches a fixed point in at most `nodes.len()` passes. Dumb on
    // purpose (decision 7: "plain worklist or DFS with a visited set");
    // a whole-pass-per-iteration fixpoint needs no visited set at all,
    // and it terminates for the same reason the plan's own DFS would.
    loop {
        let snapshot = verdicts.clone();
        let mut changed = false;
        for (key, info) in &nodes {
            if matches!(snapshot.get(key), Some(Verdict::Illegal { .. })) {
                continue;
            }
            for callee in &info.callees {
                if let Some(Verdict::Illegal { path, reason }) = snapshot.get(callee) {
                    let mut full_path = vec![info.display_name.clone()];
                    full_path.extend(path.iter().cloned());
                    verdicts.insert(
                        key.clone(),
                        Verdict::Illegal {
                            path: full_path,
                            reason: reason.clone(),
                        },
                    );
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }

    Legality { verdicts }
}

// --- provenance: the same graph, a second color (plans/M7.md item A) -------
//
// 03-hardware.md §1: "The compiler checks provenance transitively — a
// function that touches MMIO, DMA, or IRQ state must be reachable through
// the owning driver's authority."
//
// plans/M7.md decision 3: "Provenance ... is the same whole-graph
// reachability `eval::legal::classify` already computes for comptime
// legality — reuse its shape, a second color." That is taken literally.
// Same `build_nodes` graph, same callee edges, same fixed point; only the
// direction and the seed differ. Legality propagates *backwards* along
// edges (a caller inherits its callee's illegality); provenance
// propagates *forwards* from a seed (a callee inherits its caller's
// authority), which is exactly what "reachable through" means.
//
// ## What "touches MMIO, DMA, or IRQ state" means at this milestone
//
// Precisely: **the fn's own signature or body names a capability-typed
// value**. Nothing narrower is available and nothing wider would be
// honest, because at item A there is no MMIO access, no DMA operation and
// no IRQ binding in the language at all — item C owns register access,
// item D owns pools, item G owns vectors. What a fn *can* do today is
// hold the authority: take an `Mmio[L]`/`DeviceCap[D]`/`IrqCap[V]`/
// `DmaPool[P, N]` parameter, or evaluate an expression of one of those
// types (reading a field, passing one along, binding one to a local).
//
// This is not a placeholder for the real rule; it is the real rule's
// *base*. Holding the authority is the precondition for every operation
// C/D/G will add, so a fn that passes this check is exactly the set those
// operations will be allowed in. **How C and D plug in**: each adds its
// operation to `hardware_touch_reason` below — an MMIO load/store node, a
// DMA publish/reclaim node — as a second `Some(...)` arm alongside the
// type-based one. Nothing else changes: the seed, the propagation, the
// diagnostic and the roots are already whole. The arm is a match on the
// typed node kind, which is why `expr_hardware_reason` is written as an
// exhaustive match over `TypedExprKind` exactly like `expr_illegal_reason`
// is — a new node kind for an MMIO access will not compile until it
// answers this question too.
//
// ## The rule, and the one reading it takes
//
// A node is **authorized** if it is a `@driver`'s own member (the driver
// owns the capability) or is reachable from one through the callee graph.
// A node that touches and is not authorized is rejected.
//
// "Reachable through the owning driver's authority" is read as
// *at-least-one path from some driver*, not *every path from a driver*.
// That is sound rather than lax, and here is why: a caller that is not
// itself authorized cannot construct the capability argument such a call
// needs — no source construct produces one
// (`hardware.capabilities.unforgeable`), so its only source would be that
// caller's own parameter or field, which makes the *caller* a touching
// node subject to this same rule. The chain therefore bottoms out at a
// driver or at a rejection, whichever comes first.
//
// ## Scope boundary, stated rather than discovered
//
// This runs per module, because the callee graph does: a `CalleeKey`
// names a local spelling, and an imported callee resolves to no node in
// the importing module's graph (`sema::check_program_typed`'s splice
// copies a declaration's *shape* under the local name, not an edge). So a
// capability-touching helper in a module that declares no `@driver` is
// rejected even if a driver in another module calls it. That is the
// fail-closed direction — the alternative (merging every module's graph
// under bare keys) would silently *accept* a touching fn merely because
// some other module happened to declare a same-named fn that a driver
// calls. The diagnostic says "in this module" so the boundary is in the
// message, not only in the ledger.

/// One provenance check's declaration-side inputs, computed by
/// `sema::types::capability_authority` (which owns the walk that answers
/// all three) and handed here as data.
pub type Authority = crate::sema::types::CapabilityAuthority;

/// 03-hardware.md §1's provenance sentence, checked. Fail-fast in
/// `BTreeMap` key order, like every other whole-program check here.
pub fn check_provenance(program: &TypedProgram, authority: &Authority) -> Result<(), SemaError> {
    let nodes = build_nodes(program);
    let touches = hardware_touches(program, authority);
    if touches.is_empty() {
        return Ok(()); // no capability anywhere: nothing to authorize.
    }

    // Forward reachability from the driver roots. Same dumb whole-pass
    // fixed point `classify` uses, for the same reason: "authorized" is
    // monotonic, so a finite graph settles in at most `nodes.len()`
    // passes, and no visited set is needed.
    let mut authorized: BTreeSet<String> = authority
        .roots
        .iter()
        .filter(|k| nodes.contains_key(*k))
        .cloned()
        .collect();
    // =====================================================================
    // plans/M7.md item G, decision 18: a mode-generic `@driver`'s checked
    // bodies live under `struct:Name[Args].member` keys (instantiations),
    // while `capability_authority` roots are still the bare `Name.member`
    // spellings from the unsubstituted template. Seed every instantiation
    // method of a `@driver` whose bare name is already a root family.
    // =====================================================================
    let driver_root_names: BTreeSet<&str> = authority
        .roots
        .iter()
        .filter_map(|r| r.split('.').next())
        .collect();
    for key in nodes.keys() {
        let Some(rest) = key.strip_prefix("struct:") else {
            continue;
        };
        let Some(dot) = rest.rfind('.') else {
            continue;
        };
        let type_part = &rest[..dot];
        let bare = type_part.split('[').next().unwrap_or(type_part);
        if driver_root_names.contains(bare) {
            authorized.insert(key.clone());
        }
    }
    loop {
        let mut changed = false;
        for (key, info) in &nodes {
            if !authorized.contains(key) {
                continue;
            }
            for callee in &info.callees {
                if authorized.insert(callee.clone()) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    for (key, reason) in &touches {
        if authorized.contains(key) {
            continue;
        }
        let span = authority.spans.get(key).copied();
        return Err(SemaError {
            category: "type",
            message: format!(
                "`{key}` touches hardware state ({reason}) but is not reachable through any \
                 `@driver`'s authority in this module — 03-hardware.md §1: a function that \
                 touches MMIO, DMA, or IRQ state must be reachable through the owning driver's \
                 authority"
            ),
            line: span.map(|s| s.line).unwrap_or(0),
            col: span.map(|s| s.col).unwrap_or(0),
            extra_lines: Vec::new(),
            omit_location: span.is_none(),
            missing_method: None,
        });
    }
    Ok(())
}

/// Every node key that touches hardware state, with the reason naming
/// what it touched — same key namespace `build_nodes` uses, so the two
/// maps are directly comparable.
fn hardware_touches(program: &TypedProgram, authority: &Authority) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, f) in &program.fns {
        note_touch(&mut out, name.clone(), f, authority);
    }
    for (struct_name, s) in &program.structs {
        for (member, f) in &s.methods {
            note_touch(&mut out, format!("{struct_name}.{member}"), f, authority);
        }
        for (member, f) in &s.assoc_fns {
            note_touch(&mut out, format!("{struct_name}.{member}"), f, authority);
        }
        if let Some(f) = &s.init {
            note_touch(&mut out, format!("{struct_name}.init"), f, authority);
        }
    }
    for (key, inst) in &program.instantiations {
        match inst {
            TypedInstantiation::Fn(f) => note_touch(&mut out, key.clone(), f, authority),
            TypedInstantiation::Struct(s) => {
                for (member, f) in &s.methods {
                    note_touch(&mut out, format!("{key}.{member}"), f, authority);
                }
                for (member, f) in &s.assoc_fns {
                    note_touch(&mut out, format!("{key}.{member}"), f, authority);
                }
                if let Some(f) = &s.init {
                    note_touch(&mut out, format!("{key}.init"), f, authority);
                }
            }
            TypedInstantiation::Enum => {}
        }
    }
    out
}

fn note_touch(out: &mut BTreeMap<String, String>, key: String, f: &TypedFn, authority: &Authority) {
    // The signature first: a capability parameter is the plainest form of
    // "this fn holds the authority", and naming the parameter makes the
    // diagnostic checkable on its face.
    for p in &f.params {
        if let Some(found) = capability_in_type(&p.ty, authority) {
            out.insert(key, format!("its own `{}: {found}`", p.name));
            return;
        }
    }
    // Then the body. A receiver is deliberately *not* consulted: a
    // `@driver`'s own methods are roots anyway, and a `read self` on a
    // capability-holding struct would otherwise mark every one of them
    // with a less informative reason than whatever their body actually
    // does.
    let mut scan = BodyScan::default();
    scan_hardware_stmts(&f.body, authority, &mut scan);
    if let Some(reason) = scan.illegal {
        out.insert(key, reason);
    }
}

/// The capability `ty` names at any nesting, rendered — or `None`.
/// Composites recurse; a *named* type is answered by
/// `authority.capability_bearing`, the flattened set
/// `sema::types::capability_authority` already computed with the walk
/// that owns struct-field recursion, so this is not a second copy of that
/// rule.
fn capability_in_type(ty: &crate::sema::types::Type, authority: &Authority) -> Option<String> {
    use crate::sema::types::{Type, TypeArg, render_type};
    match ty {
        Type::Named(name, _)
            if crate::eval::image_checks::is_capability_type_name(name)
                || authority.capability_bearing.contains(name) =>
        {
            Some(render_type(ty))
        }
        // An `Actor[T]` handle is not the driver's authority — holding one
        // lets a `@test`/`@actor` call public methods; the callee still has
        // to be reachable from a `@driver` root. Recursing into `T` would
        // mark every `Actor[BlkDriver]` parameter as a hardware touch and
        // refuse the flagship caller (plans/M7.md item E4).
        Type::Named(name, _) if name == "Actor" => None,
        Type::Array(elem, _) => capability_in_type(elem, authority),
        Type::Tuple(elems) => elems.iter().find_map(|e| capability_in_type(e, authority)),
        Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
            capability_in_type(inner, authority)
        }
        Type::Result(ok, err) => {
            capability_in_type(ok, authority).or_else(|| capability_in_type(err, authority))
        }
        Type::Fn(params, ret) => params
            .iter()
            .find_map(|(_, t)| capability_in_type(t, authority))
            .or_else(|| capability_in_type(ret, authority)),
        Type::Named(_, targs) => targs.iter().find_map(|a| match a {
            TypeArg::Type(t) => capability_in_type(t, authority),
            _ => None,
        }),
        _ => None,
    }
}

/// Does this typed expression itself touch hardware state? Exhaustive
/// over `TypedExprKind` for the same reason `expr_illegal_reason` is: the
/// operations plans/M7.md items C and D add (an MMIO load/store, a DMA
/// publish) will be new node kinds, and this match must stop compiling
/// until each answers here. Today exactly one answer is possible — the
/// expression's *type* is a capability — because no hardware operation is
/// representable yet.
fn expr_hardware_reason(e: &TypedExpr, authority: &Authority) -> Option<String> {
    let by_type = capability_in_type(&e.ty, authority).map(|found| format!("a `{found}` value"));
    match &e.kind {
        // plans/M7.md item C (03-hardware.md §2): the first *operation*
        // arm this match has ever had — an MMIO register read or write is
        // a hardware touch in its own right, not merely a fn that happens
        // to name a capability-typed value. The distinction is real even
        // though today's two rules overlap: the access's own type is the
        // register's scalar (`u32`), so `by_type` says nothing about it,
        // and it is the *receiver* one level down that carries the
        // `Mmio[L]`. Answering here means the diagnostic names what the fn
        // did ("an MMIO register read") rather than what it held.
        TypedExprKind::Intrinsic { key, .. }
            if crate::sema::bodies::is_mmio_access_intrinsic(key) =>
        {
            Some(if key == "Mmio.read" {
                "an MMIO register read".to_string()
            } else {
                "an MMIO register write".to_string()
            })
        }
        // plans/M7.md item H1 (03-hardware.md §9): the sealed transport's
        // own transitions are hardware touches for the same reason and by
        // the same rule — a fn that claims a device, or partitions that
        // claim, is operating the device's own bring-up protocol, whatever
        // its result type happens to be.
        TypedExprKind::Intrinsic { key, .. }
            if crate::sema::bodies::is_device_transport_intrinsic(key) =>
        {
            Some(match key.as_str() {
                "Device.claim" => "a device claim (03-hardware.md §9's bring-up chain)".to_string(),
                "Device.take_irq" => {
                    "taking an `IrqCap` from a claimed device (03-hardware.md §6)".to_string()
                }
                _ => "an MMIO claim partitioning".to_string(),
            })
        }
        // plans/M7.md item G (03-hardware.md §6): binding a vector or
        // unmasking one is an IRQ-state touch — the same §1 provenance
        // sentence that already covers holding an `IrqCap[V]`.
        TypedExprKind::Intrinsic { key, .. } if crate::sema::bodies::is_irq_cap_intrinsic(key) => {
            Some(if key == "IrqCap.bind" {
                "binding an interrupt vector (03-hardware.md §6)".to_string()
            } else {
                "unmasking an interrupt vector (03-hardware.md §6)".to_string()
            })
        }
        // plans/M7.md item E2: reserving / preparing a queue operation is
        // a hardware touch (descriptor capacity / DMA payload handoff).
        TypedExprKind::Intrinsic { key, .. } if crate::sema::bodies::is_queue_op_intrinsic(key) => {
            Some(if key == "VirtQueue.reserve" {
                "a proven queue reservation (03-hardware.md §4)".to_string()
            } else {
                "a prepared block operation (03-hardware.md §4)".to_string()
            })
        }
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
        | TypedExprKind::Field(..)
        | TypedExprKind::Index(..)
        | TypedExprKind::Call { .. }
        | TypedExprKind::CallValue(..)
        | TypedExprKind::ToScalar(_)
        | TypedExprKind::Neg(_)
        | TypedExprKind::BitNot(_)
        | TypedExprKind::Take(_)
        | TypedExprKind::Try(..)
        | TypedExprKind::Binary(..)
        | TypedExprKind::OpCall(..)
        | TypedExprKind::Is(..)
        | TypedExprKind::Not(_)
        | TypedExprKind::And(..)
        | TypedExprKind::Or(..)
        | TypedExprKind::EnumConstruct { .. }
        | TypedExprKind::Closure { .. }
        | TypedExprKind::Tuple(_)
        | TypedExprKind::List(_)
        | TypedExprKind::StructLiteral { .. }
        | TypedExprKind::Panic(_)
        | TypedExprKind::PoolName(_)
        | TypedExprKind::Intrinsic { .. }
        | TypedExprKind::Await(_)
        | TypedExprKind::Send(_)
        | TypedExprKind::GroupChild(_) => by_type,
    }
}

// --- the callee graph: one node per classifiable key -----------------------

/// One graph node: a fn/method/instantiation body's own direct callees
/// (by `CalleeKey::spelling()`) and whether its own body directly
/// contains a decision-7 illegal operation.
struct NodeInfo {
    display_name: String,
    callees: BTreeSet<String>,
    illegal: Option<String>,
}

/// Registers every classifiable key `TypedProgram` carries: top-level
/// fns (`program.fns`, keyed by bare name — identical to
/// `CalleeKey::Fn`'s own spelling), each plain struct's methods/
/// assoc-fns/init (keyed `Struct.member`, identical to
/// `CalleeKey::Method`'s spelling), each plain enum's methods/assoc-fns
/// (plans/M9.md item B2 — same keying), and every instantiation
/// (`program.instantiations`, already keyed by
/// `generics::canonical_key`'s spelling — identical to
/// `CalleeKey::FnInstance`; a struct instantiation's own methods/
/// assoc-fns/init are additionally registered `key.member`, identical to
/// `CalleeKey::MethodInstance`'s spelling). An enum instantiation still
/// carries no method payload (generic enum methods fail closed at the
/// same boundary as generic struct methods).
fn build_nodes(program: &TypedProgram) -> BTreeMap<String, NodeInfo> {
    let mut nodes = BTreeMap::new();

    for (name, f) in &program.fns {
        insert_fn_node(&mut nodes, name.clone(), f);
    }

    // plans/M9.md item E: imported/spliced fns (time constructors, and
    // any other `from core...` free fn) must be classifiable so an
    // `@image` fn that calls them is not fail-closed as "cross-module
    // callee" — their bodies are already typed and sit on
    // `TypedProgram::imported`.
    for (name, f) in &program.imported.fns {
        if !nodes.contains_key(name) {
            insert_fn_node(&mut nodes, name.clone(), f);
        }
    }

    for (struct_name, s) in &program.structs {
        for (member, f) in &s.methods {
            insert_fn_node(&mut nodes, format!("{struct_name}.{member}"), f);
        }
        for (member, f) in &s.assoc_fns {
            insert_fn_node(&mut nodes, format!("{struct_name}.{member}"), f);
        }
        if let Some(f) = &s.init {
            insert_fn_node(&mut nodes, format!("{struct_name}.init"), f);
        }
    }

    for (struct_name, s) in &program.imported.structs {
        for (member, f) in &s.methods {
            let key = format!("{struct_name}.{member}");
            if !nodes.contains_key(&key) {
                insert_fn_node(&mut nodes, key, f);
            }
        }
        for (member, f) in &s.assoc_fns {
            let key = format!("{struct_name}.{member}");
            if !nodes.contains_key(&key) {
                insert_fn_node(&mut nodes, key, f);
            }
        }
        if let Some(f) = &s.init {
            let key = format!("{struct_name}.init");
            if !nodes.contains_key(&key) {
                insert_fn_node(&mut nodes, key, f);
            }
        }
    }

    for (enum_name, e) in &program.enums {
        for (member, f) in &e.methods {
            insert_fn_node(&mut nodes, format!("{enum_name}.{member}"), f);
        }
        for (member, f) in &e.assoc_fns {
            insert_fn_node(&mut nodes, format!("{enum_name}.{member}"), f);
        }
    }

    for (key, inst) in &program.instantiations {
        match inst {
            TypedInstantiation::Fn(f) => insert_fn_node(&mut nodes, key.clone(), f),
            TypedInstantiation::Struct(s) => {
                for (member, f) in &s.methods {
                    insert_fn_node(&mut nodes, format!("{key}.{member}"), f);
                }
                for (member, f) in &s.assoc_fns {
                    insert_fn_node(&mut nodes, format!("{key}.{member}"), f);
                }
                if let Some(f) = &s.init {
                    insert_fn_node(&mut nodes, format!("{key}.init"), f);
                }
            }
            TypedInstantiation::Enum => {}
        }
    }

    nodes
}

fn insert_fn_node(nodes: &mut BTreeMap<String, NodeInfo>, key: String, f: &TypedFn) {
    let mut scan = BodyScan::default();
    scan_stmts(&f.body, &mut scan);
    // plans/M9.md item K1: omitted-arg calls leave `None` in the Call's
    // arg list; the default expression lives only on `TypedParam`. Scan
    // it here so `now()` (and any other illegal op) in a default marks
    // the fn Illegal for comptime the same way a body use would.
    for p in &f.params {
        if let Some(def) = &p.default {
            scan_expr(def, &mut scan);
        }
    }
    // Plans/M6.md item A: an `async fn`'s own color is itself an async
    // operation (02-language.md §12: "free of ... async/actor
    // operations" names the fn, not merely its statements) —
    // unconditionally illegal for comptime, independent of what its body
    // contains (an async fn with no internal `await`/`send`/`with group`
    // is still illegal). `note_illegal` only sets this when the body scan
    // itself found nothing more specific, so a real internal reason
    // (e.g. an `await`) still wins the diagnostic's own wording.
    if f.is_async {
        scan.note_illegal("an `async fn`");
    }
    nodes.insert(
        key.clone(),
        NodeInfo {
            display_name: key,
            callees: scan.callees,
            illegal: scan.illegal,
        },
    );
}

/// Accumulates one body's own direct callees and (if found) the first
/// decision-7 illegal operation its own statements/expressions directly
/// contain — not counting anything a callee itself does, which is
/// `classify`'s own fixed-point's job.
#[derive(Default)]
struct BodyScan {
    callees: BTreeSet<String>,
    illegal: Option<String>,
}

impl BodyScan {
    fn note_illegal(&mut self, reason: &str) {
        if self.illegal.is_none() {
            self.illegal = Some(reason.to_string());
        }
    }
}

// --- helpers shared by the effect table and ISR signature checks ---

fn is_float_type(ty: &crate::sema::types::Type) -> bool {
    matches!(
        ty,
        crate::sema::types::Type::F32 | crate::sema::types::Type::F64
    )
}

fn dma_touch_reason(ty: &crate::sema::types::Type) -> Option<String> {
    use crate::sema::types::Type;
    match ty {
        Type::Own(pool, inner) => Some(format!(
            "own[{pool}] {}",
            crate::sema::types::render_type(inner)
        )),
        Type::Named(name, _) if name == "DmaShared" => Some(crate::sema::types::render_type(ty)),
        Type::Option(inner) | Type::Static(inner) => dma_touch_reason(inner),
        Type::Array(elem, _) => dma_touch_reason(elem),
        Type::Tuple(elems) => elems.iter().find_map(dma_touch_reason),
        Type::Result(ok, err) => dma_touch_reason(ok).or_else(|| dma_touch_reason(err)),
        _ => None,
    }
}

/// `self.<field> = ...` where the field is not an `InterruptCell[T]`.
fn is_plain_self_field_channel(target: &TypedExpr) -> bool {
    let TypedExprKind::Field(base, _) = &target.kind else {
        return false;
    };
    let TypedExprKind::Local(name) = &base.kind else {
        return false;
    };
    if name != "self" {
        return false;
    }
    !crate::sema::bodies::is_interrupt_cell_type(&target.ty)
}

fn is_format_method_callee(callee: &crate::sema::typed::CalleeKey) -> bool {
    match callee {
        crate::sema::typed::CalleeKey::Method(_, m)
        | crate::sema::typed::CalleeKey::MethodInstance(_, m) => m == "format",
        _ => false,
    }
}

// --- effect-class → allowed-contexts table + one typed-tree walk --------
//
// Contexts: Comptime (legality), Isr (03 §6 effect set), BottomHalf (`@task`).
// Hardware provenance uses the same walk skeleton with a capability visitor.
// Reasons that differ by context live in `effect_reason`; classification of
// a node into effect classes is `stmt_effects` / `expr_effects`.

use super::walk::{self, Visitor};

/// Where an effect is being judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectContext {
    Comptime,
    Isr,
    BottomHalf,
}

/// Effect classes drawn from expr/stmt nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectClass {
    Await,
    Send,
    WithGroup,
    BareSend,
    MmioAccess,
    DeviceTransport,
    QueueOp,
    IrqCapOp,
    RestrictedIntrinsic,
    Now,
    GroupOp,
    Float,
    Format,
    UnboundedLoop,
    PlainSelfFieldChannel,
    DmaPayload,
}

/// Diagnostic reason when `effect` is forbidden in `ctx`, or `None` if
/// allowed / not policed in that context.
fn effect_reason(effect: EffectClass, ctx: EffectContext) -> Option<&'static str> {
    use EffectClass::*;
    use EffectContext::*;
    match (effect, ctx) {
        (Await, Comptime) => Some("an `await` expression"),
        (Await, Isr) => Some("an `await`"),
        (Await, BottomHalf) => Some("an `await` (stays active while waiting)"),
        (Send, Comptime) => Some("a `send` expression"),
        (Send, Isr) => Some("a `send` (call another actor)"),
        (Send, BottomHalf) => Some("a `send` (call another actor)"),
        (WithGroup, Comptime) => Some("a `with group` block"),
        (WithGroup, Isr) => Some("a `with group` (call another actor / block)"),
        (WithGroup, BottomHalf) => Some("a `with group` (stays active while waiting)"),
        (BareSend, Comptime) => Some("a `send` statement"),
        (BareSend, Isr) => Some("a bare `send` (call another actor)"),
        (BareSend, BottomHalf) => Some("a bare `send` (call another actor)"),
        (MmioAccess, Comptime) => Some("a volatile MMIO register access"),
        (MmioAccess, Isr | BottomHalf) => None,
        (DeviceTransport, Comptime) => Some("a device bring-up transition (03-hardware.md §9)"),
        (DeviceTransport, Isr) => Some("a device bring-up transition (not in the ISR effect set)"),
        (DeviceTransport, BottomHalf) => None,
        (QueueOp, Comptime) => Some("a queue operation (03-hardware.md §4)"),
        (QueueOp, Isr) => {
            Some("a queue operation (not in the ISR effect set — drain belongs in `@task`)")
        }
        (QueueOp, BottomHalf) => None,
        (IrqCapOp, Comptime) => Some("an interrupt-vector operation (03-hardware.md §6)"),
        (IrqCapOp, Isr) => Some("an interrupt-vector bind/unmask (not in the ISR effect set)"),
        (IrqCapOp, BottomHalf) => None,
        (RestrictedIntrinsic, Comptime) => Some("an `@image` builder intrinsic"),
        (RestrictedIntrinsic, Isr | BottomHalf) => None,
        (Now, Comptime) => Some("`now()` (a runtime-only clock read)"),
        (Now, Isr) => Some("a runtime clock read (`now()` — not in the ISR effect set)"),
        (Now, BottomHalf) => None,
        (GroupOp, Comptime) => Some("a `group` construct"),
        (GroupOp, Isr) => Some("a `group` construct (call another actor / block)"),
        (GroupOp, BottomHalf) => None,
        (Float, Isr) => Some("floating point"),
        (Float, Comptime | BottomHalf) => None,
        (Format, Isr) => Some("formatting (f-string / Format)"),
        (Format, Comptime | BottomHalf) => None,
        (UnboundedLoop, Isr) => {
            Some("a loop (drain unbounded work — loops belong in the bottom half)")
        }
        (UnboundedLoop, Comptime | BottomHalf) => None,
        (PlainSelfFieldChannel, Isr) => {
            Some("a plain field as an ISR channel (03-hardware.md §6: use `InterruptCell[T]`)")
        }
        (PlainSelfFieldChannel, Comptime | BottomHalf) => None,
        (DmaPayload, Isr) => Some("a device-owned DMA payload (`own[P] T` / `DmaShared`)"),
        (DmaPayload, Comptime | BottomHalf) => None,
    }
}

fn stmt_kind_effects(kind: &TypedStmtKind) -> Vec<EffectClass> {
    match kind {
        TypedStmtKind::BareSend { .. } => vec![EffectClass::BareSend],
        TypedStmtKind::WithGroup { .. } => vec![EffectClass::WithGroup],
        TypedStmtKind::While { .. } | TypedStmtKind::For { .. } => {
            vec![EffectClass::UnboundedLoop]
        }
        TypedStmtKind::Assign { target, .. } if is_plain_self_field_channel(target) => {
            vec![EffectClass::PlainSelfFieldChannel]
        }
        _ => Vec::new(),
    }
}

fn stmt_effects(stmt: &TypedStmt) -> Vec<EffectClass> {
    stmt_kind_effects(&stmt.kind)
}

/// Test helpers folded into the effect table.
#[cfg(test)]
fn stmt_isr_forbidden_reason(kind: &TypedStmtKind) -> Option<&'static str> {
    stmt_kind_effects(kind)
        .into_iter()
        .find_map(|e| effect_reason(e, EffectContext::Isr))
}

#[cfg(test)]
fn expr_isr_forbidden_reason(e: &TypedExpr) -> Option<&'static str> {
    expr_effects(e)
        .into_iter()
        .find_map(|ef| effect_reason(ef, EffectContext::Isr))
}

#[cfg(test)]
fn bottom_half_expr_forbidden_reason(e: &TypedExpr) -> Option<&'static str> {
    expr_effects(e)
        .into_iter()
        .find_map(|ef| effect_reason(ef, EffectContext::BottomHalf))
}

fn expr_effects(e: &TypedExpr) -> Vec<EffectClass> {
    let mut out = Vec::new();
    match &e.kind {
        TypedExprKind::Await(_) => out.push(EffectClass::Await),
        TypedExprKind::Send(_) => out.push(EffectClass::Send),
        TypedExprKind::Float(_) => out.push(EffectClass::Float),
        TypedExprKind::Binary(..) | TypedExprKind::OpCall(..) | TypedExprKind::Neg(_)
            if is_float_type(&e.ty) =>
        {
            out.push(EffectClass::Float);
        }
        TypedExprKind::Field(..) | TypedExprKind::Take(_) | TypedExprKind::Local(_)
            if dma_touch_reason(&e.ty).is_some() =>
        {
            out.push(EffectClass::DmaPayload);
        }
        TypedExprKind::Call { callee, .. } if is_format_method_callee(callee) => {
            out.push(EffectClass::Format);
        }
        TypedExprKind::Intrinsic { key, .. } => {
            if crate::sema::bodies::is_mmio_access_intrinsic(key) {
                out.push(EffectClass::MmioAccess);
            } else if crate::sema::bodies::is_device_transport_intrinsic(key) {
                out.push(EffectClass::DeviceTransport);
            } else if crate::sema::bodies::is_queue_op_intrinsic(key) {
                out.push(EffectClass::QueueOp);
            } else if crate::sema::bodies::is_irq_cap_intrinsic(key) {
                out.push(EffectClass::IrqCapOp);
            } else if crate::sema::typed::is_restricted_intrinsic(key) {
                out.push(EffectClass::RestrictedIntrinsic);
            } else if key == "now" {
                out.push(EffectClass::Now);
            } else if key.starts_with("Group.") {
                out.push(EffectClass::GroupOp);
            }
            // InterruptCell / wake: ISR-allowed, never an effect class here.
        }
        _ => {}
    }
    out
}

/// Visitor: effect-context check, optionally collecting callee edges.
struct EffectVisitor<'a> {
    ctx: EffectContext,
    scan: &'a mut BodyScan,
    collect_callees: bool,
    patterns: bool,
}

impl Visitor for EffectVisitor<'_> {
    fn pre_stmt(&mut self, stmt: &TypedStmt) {
        for effect in stmt_effects(stmt) {
            if let Some(reason) = effect_reason(effect, self.ctx) {
                self.scan.note_illegal(reason);
            }
        }
    }
    fn pre_expr(&mut self, expr: &TypedExpr) {
        for effect in expr_effects(expr) {
            if let Some(reason) = effect_reason(effect, self.ctx) {
                self.scan.note_illegal(reason);
            }
        }
    }
    fn on_callee(&mut self, key: String) {
        if self.collect_callees {
            self.scan.callees.insert(key);
        }
    }
    fn walk_patterns(&self) -> bool {
        self.patterns
    }
}

fn scan_stmts(stmts: &[TypedStmt], scan: &mut BodyScan) {
    let mut v = EffectVisitor {
        ctx: EffectContext::Comptime,
        scan,
        collect_callees: true,
        patterns: true,
    };
    walk::walk_stmts(stmts, &mut v);
}

fn scan_expr(e: &TypedExpr, scan: &mut BodyScan) {
    let mut v = EffectVisitor {
        ctx: EffectContext::Comptime,
        scan,
        collect_callees: true,
        patterns: true,
    };
    walk::walk_expr(e, &mut v);
}

struct HardwareVisitor<'a> {
    authority: &'a Authority,
    scan: &'a mut BodyScan,
}

impl Visitor for HardwareVisitor<'_> {
    fn pre_expr(&mut self, expr: &TypedExpr) {
        if let Some(reason) = expr_hardware_reason(expr, self.authority) {
            if self.scan.illegal.is_none() {
                self.scan.illegal = Some(reason);
            }
        }
    }
}

fn scan_hardware_stmts(stmts: &[TypedStmt], authority: &Authority, scan: &mut BodyScan) {
    let mut v = HardwareVisitor { authority, scan };
    walk::walk_stmts(stmts, &mut v);
}

fn scan_bottom_half_stmts(stmts: &[TypedStmt], scan: &mut BodyScan) {
    let mut v = EffectVisitor {
        ctx: EffectContext::BottomHalf,
        scan,
        collect_callees: false,
        patterns: false,
    };
    walk::walk_stmts(stmts, &mut v);
}

fn scan_isr_forbidden_stmts(stmts: &[TypedStmt], scan: &mut BodyScan) {
    let mut v = EffectVisitor {
        ctx: EffectContext::Isr,
        scan,
        collect_callees: false,
        patterns: false,
    };
    walk::walk_stmts(stmts, &mut v);
}

fn scan_isr_forbidden_expr(e: &TypedExpr, scan: &mut BodyScan) {
    let mut v = EffectVisitor {
        ctx: EffectContext::Isr,
        scan,
        collect_callees: false,
        patterns: false,
    };
    walk::walk_expr(e, &mut v);
}

struct IsrBindVisitor<'a> {
    roots: &'a mut BTreeSet<String>,
}

impl Visitor for IsrBindVisitor<'_> {
    fn pre_expr(&mut self, e: &TypedExpr) {
        if let TypedExprKind::Intrinsic { key, args, .. } = &e.kind {
            if key == "IrqCap.bind" {
                for (label, arg) in args {
                    if label == "handler" {
                        if let TypedExprKind::FnRef(k) = &arg.kind {
                            self.roots.insert(k.spelling());
                        }
                    }
                }
            }
        }
    }
}

fn scan_isr_bind_stmts(stmts: &[TypedStmt], _scan: &mut BodyScan, roots: &mut BTreeSet<String>) {
    let mut v = IsrBindVisitor { roots };
    walk::walk_stmts(stmts, &mut v);
}

// --- ISR effect restriction: the same graph, a third color (plans/M7.md item G) ---
//
// 03-hardware.md §6: "The binding — not a keyword — makes the compiler
// restrict the function's transitive effects to the ISR set" — MMIO,
// InterruptCell, helpers with the same set, and wake; it cannot allocate,
// await, block, call another actor, touch device-owned DMA payloads,
// drain unbounded work, use floating point, or format.
//
// Decision 3: same `build_nodes` graph. Seeds are every `IrqCap.bind`
// handler `FnRef`; forward reachability marks the ISR's transitive
// closure; any node in that closure whose own body (or signature)
// carries a forbidden effect is rejected, with the call path from the
// ISR root naming the effect.

/// Every `@task` method key in `program`. A `wake(Driver.task)` names
/// one of these as a `FnRef` callee edge, but that edge is a *schedule*
/// edge — 03 §6's bottom half is ordinary code, not an ISR helper, so
/// ISR reachability must not follow it.
fn task_method_keys(program: &TypedProgram) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for (sname, s) in &program.structs {
        for (mname, f) in &s.methods {
            if f.is_task {
                keys.insert(format!("{sname}.{mname}"));
            }
        }
    }
    for (ikey, inst) in &program.instantiations {
        if let TypedInstantiation::Struct(s) = inst {
            for (mname, f) in &s.methods {
                if f.is_task {
                    keys.insert(format!("{ikey}.{mname}"));
                }
            }
        }
    }
    keys
}

/// 03-hardware.md §6's ISR effect restriction.
pub fn check_isr_effects(program: &TypedProgram) -> Result<(), SemaError> {
    let roots = collect_isr_roots(program);
    if roots.is_empty() {
        return Ok(());
    }
    let nodes = build_nodes(program);
    let tasks = task_method_keys(program);
    let mut in_isr: BTreeSet<String> = roots
        .iter()
        .filter(|k| nodes.contains_key(*k))
        .cloned()
        .collect();
    loop {
        let mut changed = false;
        for (key, info) in &nodes {
            if !in_isr.contains(key) {
                continue;
            }
            for callee in &info.callees {
                // plans/M7.md item G: `wake`'s FnRef is not an ISR call.
                if tasks.contains(callee) {
                    continue;
                }
                if in_isr.insert(callee.clone()) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut forbidden: BTreeMap<String, String> = BTreeMap::new();
    for key in &in_isr {
        if let Some(reason) = isr_forbidden_of(program, key) {
            forbidden.insert(key.clone(), reason);
        }
    }
    for (key, reason) in &forbidden {
        let path = isr_path_from_root(&roots, &nodes, key).unwrap_or_else(|| vec![key.clone()]);
        let path_text = path.join(" -> ");
        return Err(SemaError {
            category: "type",
            message: format!(
                "ISR `{path_text}` {reason} — 03-hardware.md §6: an interrupt handler's \
                 transitive effects are restricted to typed MMIO, `InterruptCell[T]`, \
                 helpers with that same set, and `wake` of a statically bound task"
            ),
            line: 0,
            col: 0,
            extra_lines: Vec::new(),
            omit_location: true,
            missing_method: None,
        });
    }
    Ok(())
}

/// plans/M7.md item G: every `wake(...)` must sit in an ISR's transitive
/// closure or in a `@task` body. A wake from ordinary code has no
/// delivery contract (03 §6's mask–arm–recheck is for the ISR→bottom-half
/// edge and the bottom half's own re-wake).
pub fn check_wake_sites(program: &TypedProgram) -> Result<(), SemaError> {
    let task_keys = task_method_keys(program);
    let isr_keys = {
        let roots = collect_isr_roots(program);
        if roots.is_empty() {
            BTreeSet::new()
        } else {
            let nodes = build_nodes(program);
            let mut in_isr: BTreeSet<String> = roots
                .iter()
                .filter(|k| nodes.contains_key(*k))
                .cloned()
                .collect();
            loop {
                let mut changed = false;
                for (key, info) in &nodes {
                    if !in_isr.contains(key) {
                        continue;
                    }
                    for callee in &info.callees {
                        if task_keys.contains(callee) {
                            continue;
                        }
                        if in_isr.insert(callee.clone()) {
                            changed = true;
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            in_isr
        }
    };

    let mut bad: Option<String> = None;
    let mut note = |key: &str, f: &TypedFn| {
        if bad.is_some() {
            return;
        }
        if fn_contains_wake(f) && !isr_keys.contains(key) && !task_keys.contains(key) {
            bad = Some(key.to_string());
        }
    };
    for (name, f) in &program.fns {
        note(name, f);
    }
    for (sname, s) in &program.structs {
        if let Some(f) = &s.init {
            note(&format!("{sname}.init"), f);
        }
        for (m, f) in &s.methods {
            note(&format!("{sname}.{m}"), f);
        }
        for (m, f) in &s.assoc_fns {
            note(&format!("{sname}.{m}"), f);
        }
    }
    for (ikey, inst) in &program.instantiations {
        match inst {
            TypedInstantiation::Fn(f) => note(ikey, f),
            TypedInstantiation::Struct(s) => {
                if let Some(f) = &s.init {
                    note(&format!("{ikey}.init"), f);
                }
                for (m, f) in &s.methods {
                    note(&format!("{ikey}.{m}"), f);
                }
                for (m, f) in &s.assoc_fns {
                    note(&format!("{ikey}.{m}"), f);
                }
            }
            TypedInstantiation::Enum => {}
        }
    }
    if let Some(key) = bad {
        return Err(SemaError {
            category: "type",
            message: format!(
                "`wake` in `{key}` is outside an ISR and outside a `@task` — 03-hardware.md §6: \
                 `wake` is an ISR effect (and a bottom half may re-wake itself); ordinary code \
                 cannot"
            ),
            line: 0,
            col: 0,
            extra_lines: Vec::new(),
            omit_location: true,
            missing_method: None,
        });
    }
    Ok(())
}

fn fn_contains_wake(f: &TypedFn) -> bool {
    struct WakeVisitor {
        found: bool,
    }
    impl Visitor for WakeVisitor {
        fn pre_expr(&mut self, e: &TypedExpr) {
            if let TypedExprKind::Intrinsic { key, .. } = &e.kind {
                if key == "wake" {
                    self.found = true;
                }
            }
        }
    }
    let mut v = WakeVisitor { found: false };
    walk::walk_stmts(&f.body, &mut v);
    v.found
}

/// plans/M7.md item G / E4 decision 22: a `@task` bottom half — may drain,
/// claim resolved receipts, and re-wake; must not await or send (03 §7:
/// "never stays active while waiting"). Receipt-shaped work is live in
/// the bottom half (that is where completions resolve); the caller's
/// `await receipt` remains the parked-waiter path.
pub fn check_bottom_half(program: &TypedProgram) -> Result<(), SemaError> {
    let note = |key: &str, f: &TypedFn| -> Result<(), SemaError> {
        if !f.is_task {
            return Ok(());
        }
        if f.is_async {
            return Err(SemaError {
                category: "type",
                message: format!(
                    "`@task` `{key}` is `async` — 03-hardware.md §6/§7: the bottom half never \
                     stays active while waiting (submission turns are not re-entered)"
                ),
                line: 0,
                col: 0,
                extra_lines: Vec::new(),
                omit_location: true,
                missing_method: None,
            });
        }
        if let Some(reason) = bottom_half_forbidden_of(f) {
            return Err(SemaError {
                category: "type",
                message: format!(
                    "`@task` `{key}` {reason} — 03-hardware.md §6/§7: the bottom half drains a \
                     level signal and re-wakes if work remains; it does not await"
                ),
                line: 0,
                col: 0,
                extra_lines: Vec::new(),
                omit_location: true,
                missing_method: None,
            });
        }
        Ok(())
    };
    for (sname, s) in &program.structs {
        for (m, f) in &s.methods {
            note(&format!("{sname}.{m}"), f)?;
        }
    }
    for (ikey, inst) in &program.instantiations {
        if let TypedInstantiation::Struct(s) = inst {
            for (m, f) in &s.methods {
                note(&format!("{ikey}.{m}"), f)?;
            }
        }
    }
    Ok(())
}

fn bottom_half_forbidden_of(f: &TypedFn) -> Option<String> {
    let mut scan = BodyScan::default();
    scan_bottom_half_stmts(&f.body, &mut scan);
    scan.illegal.map(|r| format!("uses {r}"))
}

/// Kept for the decision-22 unit test that pins Receipt is *not* an
/// ISR/`@task` forbidden shape; production scanning no longer consults it.
#[cfg(test)]
fn type_mentions_receipt(ty: &crate::sema::types::Type) -> bool {
    use crate::sema::types::Type;
    match ty {
        Type::Named(n, targs) => {
            n == "Receipt"
                || targs.iter().any(|a| match a {
                    crate::sema::types::TypeArg::Type(t) => type_mentions_receipt(t),
                    _ => false,
                })
        }
        Type::Option(inner) | Type::Static(inner) | Type::Own(_, inner) => {
            type_mentions_receipt(inner)
        }
        Type::Array(elem, _) => type_mentions_receipt(elem),
        Type::Tuple(elems) => elems.iter().any(type_mentions_receipt),
        Type::Result(ok, err) => type_mentions_receipt(ok) || type_mentions_receipt(err),
        _ => false,
    }
}

fn collect_isr_roots(program: &TypedProgram) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    let mut note = |f: &TypedFn| {
        let mut scan = BodyScan::default();
        scan_isr_bind_stmts(&f.body, &mut scan, &mut roots);
    };
    for f in program.fns.values() {
        note(f);
    }
    for s in program.structs.values() {
        if let Some(f) = &s.init {
            note(f);
        }
        for f in s.methods.values() {
            note(f);
        }
        for f in s.assoc_fns.values() {
            note(f);
        }
    }
    for e in program.enums.values() {
        for f in e.methods.values() {
            note(f);
        }
        for f in e.assoc_fns.values() {
            note(f);
        }
    }
    for inst in program.instantiations.values() {
        match inst {
            TypedInstantiation::Fn(f) => note(f),
            TypedInstantiation::Struct(s) => {
                if let Some(f) = &s.init {
                    note(f);
                }
                for f in s.methods.values() {
                    note(f);
                }
                for f in s.assoc_fns.values() {
                    note(f);
                }
            }
            TypedInstantiation::Enum => {}
        }
    }
    roots
}

fn isr_path_from_root(
    roots: &BTreeSet<String>,
    nodes: &BTreeMap<String, NodeInfo>,
    target: &str,
) -> Option<Vec<String>> {
    if roots.contains(target) {
        return Some(vec![target.to_string()]);
    }
    let mut parent: BTreeMap<String, String> = BTreeMap::new();
    let mut queue: Vec<String> = roots.iter().cloned().collect();
    let mut seen: BTreeSet<String> = roots.clone();
    while let Some(cur) = queue.pop() {
        let Some(info) = nodes.get(&cur) else {
            continue;
        };
        for callee in &info.callees {
            if !seen.insert(callee.clone()) {
                continue;
            }
            parent.insert(callee.clone(), cur.clone());
            if callee == target {
                let mut path = vec![target.to_string()];
                let mut walk = target.to_string();
                while let Some(p) = parent.get(&walk) {
                    path.push(p.clone());
                    walk = p.clone();
                }
                path.reverse();
                return Some(path);
            }
            queue.push(callee.clone());
        }
    }
    None
}

fn isr_forbidden_of(program: &TypedProgram, key: &str) -> Option<String> {
    let f = lookup_typed_fn(program, key)?;
    for p in &f.params {
        if let Some(reason) = dma_touch_reason(&p.ty) {
            return Some(format!(
                "touches device DMA state via parameter `{}: {reason}`",
                p.name
            ));
        }
        if is_float_type(&p.ty) {
            return Some(format!(
                "uses floating point via parameter `{}: {}`",
                p.name,
                crate::sema::types::render_type(&p.ty)
            ));
        }
    }
    let mut scan = BodyScan::default();
    scan_isr_forbidden_stmts(&f.body, &mut scan);
    // plans/M9.md item K1: same as `insert_fn_node` — param defaults are
    // part of calling the fn with an omitted arg. Without this, an ISR
    // that calls `stamp()` with `fn stamp(t: Instant = now())` never saw
    // `now()` in either body.
    for p in &f.params {
        if let Some(def) = &p.default {
            scan_isr_forbidden_expr(def, &mut scan);
        }
    }
    scan.illegal.map(|r| format!("uses {r}"))
}

fn lookup_typed_fn<'a>(program: &'a TypedProgram, key: &str) -> Option<&'a TypedFn> {
    if let Some(f) = program.fns.get(key) {
        return Some(f);
    }
    if let Some((sname, member)) = key.split_once('.') {
        if let Some(s) = program.structs.get(sname) {
            if let Some(f) = s.methods.get(member) {
                return Some(f);
            }
            if let Some(f) = s.assoc_fns.get(member) {
                return Some(f);
            }
            if member == "init" {
                return s.init.as_ref();
            }
        }
    }
    if let Some(TypedInstantiation::Fn(f)) = program.instantiations.get(key) {
        return Some(f);
    }
    if let Some((ikey, member)) = key.rsplit_once('.') {
        if let Some(TypedInstantiation::Struct(s)) = program.instantiations.get(ikey) {
            if let Some(f) = s.methods.get(member) {
                return Some(f);
            }
            if let Some(f) = s.assoc_fns.get(member) {
                return Some(f);
            }
            if member == "init" {
                return s.init.as_ref();
            }
        }
    }
    None
}

// --- unit tests --------------------------------------------------------

// --- unit tests --------------------------------------------------------
//
// Built from real source through the real pipeline (`sema::check_typed`)
// wherever convenient; hand-built nodes exercise the effect table for
// ISR / bottom-half arms that need a single expression.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sema;
    use crate::syntax::{lexer, parser};

    fn typed_program(src: &str) -> TypedProgram {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        sema::check_typed(&module, "test.wr").expect("test source must check")
    }

    #[test]
    fn plain_fn_is_legal() {
        let program = typed_program(
            "module examples.legal_plain

pub fn double(x: u64) -> u64:
    return x * 2
",
        );
        let legality = classify(&program);
        assert_eq!(legality.verdict("double"), Verdict::Legal);
    }

    #[test]
    fn transitive_chain_is_legal() {
        let program = typed_program(
            "module examples.legal_chain

fn c(x: u64) -> u64:
    return x + 1

fn b(x: u64) -> u64:
    return c(x) * 2

pub fn a(x: u64) -> u64:
    return b(x)
",
        );
        let legality = classify(&program);
        assert_eq!(legality.verdict("a"), Verdict::Legal);
        assert_eq!(legality.verdict("b"), Verdict::Legal);
        assert_eq!(legality.verdict("c"), Verdict::Legal);
    }

    #[test]
    fn recursion_is_legal() {
        let program = typed_program(
            "module examples.legal_recursion

pub fn countdown(n: u64) -> u64:
    if n == 0:
        return 0
    return countdown(n - 1)
",
        );
        let legality = classify(&program);
        assert_eq!(legality.verdict("countdown"), Verdict::Legal);
    }

    #[test]
    fn mutual_recursion_is_legal() {
        let program = typed_program(
            "module examples.legal_mutual

fn is_even(n: u64) -> bool:
    if n == 0:
        return true
    return is_odd(n - 1)

fn is_odd(n: u64) -> bool:
    if n == 0:
        return false
    return is_even(n - 1)
",
        );
        let legality = classify(&program);
        assert_eq!(legality.verdict("is_even"), Verdict::Legal);
        assert_eq!(legality.verdict("is_odd"), Verdict::Legal);
    }

    #[test]
    fn method_calls_are_classified_by_struct_dot_member_key() {
        let program = typed_program(
            "module examples.legal_methods

pub struct Counter:
    value: u64 = 0

    pub fn get(read self) -> u64:
        return self.value

    pub fn doubled(read self) -> u64:
        return self.get() * 2
",
        );
        let legality = classify(&program);
        assert_eq!(legality.verdict("Counter.get"), Verdict::Legal);
        assert_eq!(legality.verdict("Counter.doubled"), Verdict::Legal);
    }

    #[test]
    fn generic_fn_instantiation_is_classified_independently() {
        let program = typed_program(
            "module examples.legal_generic_fn

pub struct Sample:
    id: u64

    pub fn hash(read self) -> u64:
        return self.id

pub fn hash_pair[T](a: T, b: T) -> u64:
    return a.hash() ^ b.hash()

pub fn use_hash_pair(a: Sample, b: Sample) -> u64:
    return hash_pair[Sample](a, b)
",
        );
        // The generic declaration's own (unchecked) body contributes no
        // node at all (typed.rs: `check_top_fn` returns `Ok(None)` for a
        // generic decl) — only its instantiation is classifiable.
        assert!(!program.fns.contains_key("hash_pair"));
        let legality = classify(&program);
        assert_eq!(
            legality.verdict("fn:hash_pair[Sample]"),
            Verdict::Legal,
            "instantiation key spelling must match generics::canonical_key exactly"
        );
        assert_eq!(legality.verdict("use_hash_pair"), Verdict::Legal);
    }

    #[test]
    fn generic_struct_instantiation_method_is_classified_independently() {
        let program = typed_program(
            "module examples.legal_generic_struct

pub struct Slot[T, const N: usize]:
    items: [Option[T]; N]

    fn first(read self) -> Option[T]:
        return self.items[0]

pub fn use_slot() -> Option[u64]:
    slots = Slot[u64, 2](items=[None, None])
    return slots.first()
",
        );
        let legality = classify(&program);
        assert_eq!(
            legality.verdict("struct:Slot[u64, 2].first"),
            Verdict::Legal,
            "instantiated-struct method key spelling must match CalleeKey::MethodInstance"
        );
        assert_eq!(legality.verdict("use_slot"), Verdict::Legal);
    }

    #[test]
    fn closure_body_folds_into_enclosing_fn_closure_set() {
        // Decision 4: a closure has no graph node of its own — its own
        // calls are folded into whichever fn's body constructed it. This
        // exercises the scan directly (not just `classify`'s verdicts,
        // which would stay `Legal` either way) to prove `helper()` was
        // actually captured, not silently dropped.
        let program = typed_program(
            "module examples.legal_closure

fn helper() -> u64:
    return 7

fn run(body: fn() -> u64) -> u64:
    return body()

pub fn use_closure() -> u64:
    return run(||: helper())
",
        );
        let f = program
            .fns
            .get("use_closure")
            .expect("use_closure must be in the typed program");
        let mut scan = BodyScan::default();
        scan_stmts(&f.body, &mut scan);
        assert!(
            scan.callees.contains("helper"),
            "use_closure's own scan must absorb its closure argument's own call to helper(), \
             got callees={:?}",
            scan.callees
        );
        assert!(scan.callees.contains("run"));

        let legality = classify(&program);
        assert_eq!(legality.verdict("use_closure"), Verdict::Legal);
        assert_eq!(legality.verdict("helper"), Verdict::Legal);
    }

    #[test]
    fn require_legal_accepts_a_legal_key() {
        let program = typed_program(
            "module examples.legal_require

pub fn double(x: u64) -> u64:
    return x * 2
",
        );
        let legality = classify(&program);
        let result = require_legal(&legality, "double", "comptime assert", Span::default());
        assert!(result.is_ok());
    }

    #[test]
    fn require_legal_reports_illegal_verdicts_with_a_call_path_diagnostic() {
        // Since no real program can produce an `Illegal` verdict yet
        // (see the module/tests-module doc), this exercises
        // `require_legal`'s own diagnostic shape directly against a
        // hand-built `Legality` — the multi-hop-chain rendering, not the
        // classification that would normally build one.
        let mut verdicts = BTreeMap::new();
        verdicts.insert(
            "a".to_string(),
            Verdict::Illegal {
                path: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                reason: "await expression".to_string(),
            },
        );
        let legality = Legality { verdicts };
        let err = require_legal(&legality, "a", "comptime assert", Span { line: 5, col: 3 })
            .expect_err("an Illegal verdict must produce a diagnostic");
        assert_eq!(err.category, "comptime");
        assert_eq!(
            err.message,
            "`comptime assert` requires a comptime-legal closure, but `a` is not comptime-callable"
        );
        assert_eq!(err.line, 5);
        assert_eq!(err.col, 3);
        assert!(!err.omit_location);
        assert_eq!(
            err.extra_lines,
            vec![
                "  `a` calls `b`".to_string(),
                "  `b` calls `c`".to_string(),
                "  `c` uses await expression".to_string(),
            ]
        );
    }

    #[test]
    fn unknown_key_defaults_to_legal() {
        let legality = Legality::default();
        assert_eq!(legality.verdict("nonexistent"), Verdict::Legal);
        assert!(
            require_legal(&legality, "nonexistent", "comptime assert", Span::default()).is_ok()
        );
    }

    // --- provenance (plans/M7.md item A, 03-hardware.md §1) --------------
    //
    // Driven through the real `sema::check` pipeline, like every test
    // above: `check_provenance` is wired into it, so these assert the
    // rule as a user meets it rather than a hand-built graph.

    fn provenance(src: &str) -> Result<(), String> {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        sema::check(&module, "test.wr").map_err(|e| e.message)
    }

    const DRIVER_PRELUDE: &str = "module t\n\n\
         @layout(mmio, endian=little)\n\
         struct Regs:\n\
         \x20   @offset(0x000) status: ReadOnly[u32]\n\n\
         @driver\n\
         pub struct D:\n\
         \x20   n: u64\n\n";

    /// The transitive half of the sentence: "reachable through" is a
    /// closure, not a direct-call check. Two hops from the driver.
    #[test]
    fn authority_reaches_transitively_through_the_callee_graph() {
        let src = format!(
            "{DRIVER_PRELUDE}\
             \x20   fn service(read self, read m: Mmio[Regs]) -> u64:\n\
             \x20       return hop_one(m)\n\n\
             fn hop_one(read m: Mmio[Regs]) -> u64:\n\
             \x20   return hop_two(m)\n\n\
             fn hop_two(read m: Mmio[Regs]) -> u64:\n\
             \x20   return 1\n"
        );
        assert_eq!(provenance(&src), Ok(()));
    }

    /// ...and the same graph with the middle hop's own call removed: the
    /// tail is now unreachable, and the *tail* is what is named — not the
    /// still-reachable hop that stopped calling it.
    #[test]
    fn a_severed_tail_is_rejected_and_named() {
        let src = format!(
            "{DRIVER_PRELUDE}\
             \x20   fn service(read self, read m: Mmio[Regs]) -> u64:\n\
             \x20       return hop_one(m)\n\n\
             fn hop_one(read m: Mmio[Regs]) -> u64:\n\
             \x20   return 2\n\n\
             fn hop_two(read m: Mmio[Regs]) -> u64:\n\
             \x20   return 1\n"
        );
        let err = provenance(&src).expect_err("the severed tail must be rejected");
        assert!(err.starts_with("`hop_two` touches hardware state"), "{err}");
    }

    /// A program with no capability anywhere must not pay for this rule,
    /// and — more to the point — must not be *changed* by it. Every
    /// pre-item-A program is this case.
    #[test]
    fn a_program_with_no_capability_is_untouched() {
        assert_eq!(
            provenance(
                "module t\n\nfn helper() -> u64:\n    return 1\n\n\
                 pub fn use_it() -> u64:\n    return helper()\n"
            ),
            Ok(())
        );
    }

    /// A generic *instantiation* is a graph node like any other, and it
    /// is the one node kind with no source location at all (it is
    /// synthesized — `sema::types::capability_authority` records spans
    /// only for declared fns and members). Its diagnostic must therefore
    /// be location-free rather than carry an invented `0:0`; this is the
    /// only case that reaches that branch, and it reaches it because
    /// `fn:hold[Wrap]` sorts before `zz_caller` in the fail-fast
    /// `BTreeMap` order both nodes appear in.
    #[test]
    fn an_instantiation_has_no_span_and_gets_a_location_free_diagnostic() {
        let src = "module t\n\n\
             @layout(mmio, endian=little)\n\
             struct Regs:\n\
             \x20   @offset(0x000) status: ReadOnly[u32]\n\n\
             struct Wrap:\n\
             \x20   m: Mmio[Regs]\n\
             \x20   n: u64\n\n\
             fn hold[T](read m: T) -> u64:\n\
             \x20   return 0\n\n\
             fn zz_caller(read w: Wrap) -> u64:\n\
             \x20   return hold[Wrap](w)\n";
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        let err = sema::check(&module, "test.wr").expect_err("the instantiation touches");
        assert!(
            err.message
                .starts_with("`fn:hold[Wrap]` touches hardware state"),
            "{}",
            err.message
        );
        assert!(
            err.omit_location,
            "an instantiation has no location to cite"
        );
    }

    /// An `@actor` is not an authority root, only a `@driver` is —
    /// 03-hardware.md §1 says "the owning driver's authority", and the
    /// containment rule that keeps an actor from holding one would be
    /// pointless if calling out of an actor conferred authority anyway.
    #[test]
    fn an_actor_is_not_an_authority_root() {
        let src = "module t\n\n\
             @layout(mmio, endian=little)\n\
             struct Regs:\n\
             \x20   @offset(0x000) status: ReadOnly[u32]\n\n\
             @actor\n\
             pub struct A:\n\
             \x20   n: u64\n\n\
             \x20   init(mut self):\n\
             \x20       self.n = 0\n\n\
             \x20   fn call_it(read self) -> u64:\n\
             \x20       return 0\n\n\
             fn holds(read m: Mmio[Regs]) -> u64:\n\
             \x20   return 1\n";
        let err = provenance(src).expect_err("an actor confers no authority");
        assert!(err.starts_with("`holds` touches hardware state"), "{err}");
    }

    // --- typed MMIO access (plans/M7.md item C, 03-hardware.md §2) -------

    const MMIO_PRELUDE: &str = "module t\n\n\
         @layout(mmio, endian=little)\n\
         struct Regs:\n\
         \x20   @offset(0x000) status: ReadOnly[u32]\n\n";

    /// The *operation* arm of `expr_hardware_reason`, distinguished from
    /// the type arm it would otherwise hide behind. `Bundle.peek` has no
    /// capability *parameter* at all — its receiver is deliberately not
    /// consulted (`note_touch`) — so the only thing that can mark it is
    /// its body, and the reason names what it did rather than what it
    /// held. Without this arm the same fn is still rejected (the receiver
    /// field's own `Mmio[Regs]` type marks it one level down), which is
    /// exactly why the *reason text* is what this asserts.
    #[test]
    fn an_mmio_access_is_a_hardware_touch_named_by_its_operation() {
        let read = format!(
            "{MMIO_PRELUDE}struct Bundle:\n\
             \x20   regs: Mmio[Regs]\n\n\
             \x20   fn peek(read self) -> u32:\n\
             \x20       return self.regs.status.read()\n\n\
             @driver\npub struct D:\n    n: u64\n"
        );
        let err = provenance(&read).expect_err("no driver reaches `Bundle.peek`");
        assert!(
            err.starts_with("`Bundle.peek` touches hardware state (an MMIO register read)"),
            "{err}"
        );

        let write = format!(
            "{MMIO_PRELUDE}@layout(mmio, endian=little)\n\
             struct Ack:\n\
             \x20   @offset(0x010) ack: WriteOnly[u32]\n\n\
             struct Bundle:\n\
             \x20   regs: Mmio[Ack]\n\n\
             \x20   fn poke(read self):\n\
             \x20       self.regs.ack.write(1)\n\n\
             @driver\npub struct D:\n    n: u64\n"
        );
        let err = provenance(&write).expect_err("no driver reaches `Bundle.poke`");
        assert!(
            err.starts_with("`Bundle.poke` touches hardware state (an MMIO register write)"),
            "{err}"
        );
    }

    /// The comptime-legality arm (02-language.md §12: "free of I/O ...
    /// and hardware effects"). Asserted against `classify` directly, and
    /// deliberately so: **no source program can reach `require_legal`
    /// with one of these today**, because reaching an MMIO access from a
    /// `const`/`comptime assert`/`@test` needs an `Mmio[L]` *value*, and
    /// nothing in the language produces one (`hardware.capabilities
    /// .unforgeable`). The arm still has to exist — it is what stops the
    /// access from silently becoming comptime-callable the day a
    /// capability can be constructed — so this pins the verdict at the
    /// only layer that can hold it, rather than pretending a golden
    /// covers it. Recorded honestly in `hardware.mmio.typed-access`.
    #[test]
    fn an_mmio_access_is_comptime_illegal() {
        let program = typed_program(&format!(
            "{MMIO_PRELUDE}@driver\npub struct D:\n\
             \x20   regs: Mmio[Regs]\n\n\
             \x20   fn peek(read self) -> u32:\n\
             \x20       return self.regs.status.read()\n\n\
             \x20   fn peek_twice(read self) -> u32:\n\
             \x20       return self.peek() + self.peek()\n"
        ));
        let legality = classify(&program);
        assert_eq!(
            legality.verdict("D.peek"),
            Verdict::Illegal {
                path: vec!["D.peek".to_string()],
                reason: "a volatile MMIO register access".to_string(),
            }
        );
        // ...and it propagates, like every other illegal operation.
        assert!(matches!(
            legality.verdict("D.peek_twice"),
            Verdict::Illegal { .. }
        ));
    }

    /// Arms that a sync ISR's transitive closure cannot currently reach
    /// from source (await/send/with-group require an async context; an
    /// ISR must be a plain `fn`). Named here so the exhaustive match
    /// cannot drop them silently. `for` shares the loop arm with `while`
    /// (`golden/err-isr-for` / `golden/err-isr-while`).
    #[test]
    fn isr_forbidden_reason_names_every_doc_effect() {
        use crate::sema::types::Type;
        assert_eq!(
            stmt_isr_forbidden_reason(&TypedStmtKind::While {
                cond: TypedExpr {
                    span: Span::default(),
                    ty: Type::Bool,
                    kind: TypedExprKind::Bool(true),
                },
                body: vec![],
                budget: None,
            }),
            Some("a loop (drain unbounded work — loops belong in the bottom half)")
        );
        assert_eq!(
            stmt_isr_forbidden_reason(&TypedStmtKind::For {
                name: "i".into(),
                elem_ty: Type::U64,
                take_binding: false,
                iter: TypedForIter::Range(
                    TypedExpr {
                        span: Span::default(),
                        ty: Type::U64,
                        kind: TypedExprKind::Int("0".into()),
                    },
                    TypedExpr {
                        span: Span::default(),
                        ty: Type::U64,
                        kind: TypedExprKind::Int("1".into()),
                    },
                    false,
                ),
                body: vec![],
                budget: None,
            }),
            Some("a loop (drain unbounded work — loops belong in the bottom half)")
        );
        assert_eq!(
            stmt_isr_forbidden_reason(&TypedStmtKind::BareSend {
                span: Span::default(),
                expr: TypedExpr {
                    span: Span::default(),
                    ty: Type::Unit,
                    kind: TypedExprKind::Unit,
                },
            }),
            Some("a bare `send` (call another actor)")
        );
        assert_eq!(
            stmt_isr_forbidden_reason(&TypedStmtKind::WithGroup {
                capacity: None,
                deadline: None,
                as_name: None,
                body: vec![],
            }),
            Some("a `with group` (call another actor / block)")
        );
        let await_expr = TypedExpr {
            span: Span::default(),
            ty: Type::U64,
            kind: TypedExprKind::Await(Box::new(TypedExpr {
                span: Span::default(),
                ty: Type::U64,
                kind: TypedExprKind::Unit,
            })),
        };
        assert_eq!(expr_isr_forbidden_reason(&await_expr), Some("an `await`"));
        let send_expr = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Send(Box::new(TypedExpr {
                span: Span::default(),
                ty: Type::Unit,
                kind: TypedExprKind::Unit,
            })),
        };
        assert_eq!(
            expr_isr_forbidden_reason(&send_expr),
            Some("a `send` (call another actor)")
        );
        let float_expr = TypedExpr {
            span: Span::default(),
            ty: Type::F64,
            kind: TypedExprKind::Float("1.0".into()),
        };
        assert_eq!(
            expr_isr_forbidden_reason(&float_expr),
            Some("floating point")
        );
        let format_expr = TypedExpr {
            span: Span::default(),
            ty: Type::String(Box::new(crate::syntax::ast::Expr::Int(
                Span::default(),
                "10".into(),
            ))),
            kind: TypedExprKind::Call {
                callee: crate::sema::typed::CalleeKey::Method("u32".into(), "format".into()),
                receiver: Some(Box::new(TypedExpr {
                    span: Span::default(),
                    ty: Type::U32,
                    kind: TypedExprKind::Int("1".into()),
                })),
                args: vec![],
            },
        };
        assert_eq!(
            expr_isr_forbidden_reason(&format_expr),
            Some("formatting (f-string / Format)")
        );
        // plans/M7.md item G: wake is on the ISR allowlist (site check is
        // separate); Receipt-shaped work is named for item E even before
        // `Receipt` is a source type — the scanner arm exists so item E
        // cannot land a typed node this match would silently accept.
        let wake_expr = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Intrinsic {
                key: "wake".into(),
                receiver: None,
                type_arg: None,
                args: vec![],
            },
        };
        assert_eq!(expr_isr_forbidden_reason(&wake_expr), None);
        assert!(type_mentions_receipt(&Type::Named(
            "Receipt".into(),
            vec![crate::sema::types::TypeArg::Type(Type::U32)]
        )));
    }

    /// plans/M7.md item G / E4 decision 22: `@task` forbids await/send;
    /// Receipt-shaped values are allowed (bottom half resolves them).
    #[test]
    fn bottom_half_forbidden_reason_names_await_send_not_receipt() {
        use crate::sema::types::Type;
        let await_expr = TypedExpr {
            span: Span::default(),
            ty: Type::U64,
            kind: TypedExprKind::Await(Box::new(TypedExpr {
                span: Span::default(),
                ty: Type::U64,
                kind: TypedExprKind::Unit,
            })),
        };
        assert_eq!(
            bottom_half_expr_forbidden_reason(&await_expr),
            Some("an `await` (stays active while waiting)")
        );
        let send_expr = TypedExpr {
            span: Span::default(),
            ty: Type::Unit,
            kind: TypedExprKind::Send(Box::new(TypedExpr {
                span: Span::default(),
                ty: Type::Unit,
                kind: TypedExprKind::Unit,
            })),
        };
        assert_eq!(
            bottom_half_expr_forbidden_reason(&send_expr),
            Some("a `send` (call another actor)")
        );
        let receipt_ty = Type::Named(
            "Receipt".into(),
            vec![crate::sema::types::TypeArg::Type(Type::U32)],
        );
        let receipt_expr = TypedExpr {
            span: Span::default(),
            ty: receipt_ty,
            kind: TypedExprKind::Unit,
        };
        assert_eq!(bottom_half_expr_forbidden_reason(&receipt_expr), None);
    }
}
