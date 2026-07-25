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
//! ## The await/send/with/pool-op question, answered
//!
//! Every one of those operations is *unrepresentable* in the current
//! typed tree. `bodies.rs` fails closed on each before a typed node is
//! ever built:
//!
//!   - `Expr::Unary(_, UnaryOp::Await, _)` -> `error[unimplemented]:
//!     await is not checked yet` (bodies.rs, `check_expr`)
//!   - `Expr::Send` -> `error[unimplemented]: send is not checked yet`
//!   - `Stmt::With` -> `error[unimplemented]: \`with\` is not checked
//!     yet` (covers both the group and scoped-pool forms — one node,
//!     one fail-closed arm)
//!   - an `@image` fn's body -> `error[unimplemented]: @image bodies are
//!     not checked yet` (`bodies::is_image_fn`)
//!
//! `sema::check_typed` runs this exact `bodies::check` pass first in its
//! pipeline (`sema/mod.rs`) and returns `Err` the moment any of these is
//! reached, before a `TypedProgram` exists at all — so a `TypedProgram`
//! this module ever sees is, by construction, already free of every one
//! of decision 7's illegal operations (golden/err-unimplemented-await
//! pins the await case; the other three fail closed the identical way,
//! unpinned by a dedicated golden only because M2's goldens never
//! exercised them). There is no representable "illegal" typed node to
//! classify yet.
//!
//! This module is honest about that rather than inventing a fixture: the
//! per-node scan below (`expr_illegal_reason`/`stmt_illegal_reason`) is
//! an *exhaustive* match over every current `TypedExprKind`/
//! `TypedStmtKind` variant, returning "legal" for all of them — not a
//! wildcard `_ => None`. When M5 lifts any of decision 7's fail-closed
//! diagnostics, `typed.rs` gains a real node for it and *this match stops
//! compiling* until a real arm (returning `Some("<operation>")`) is added
//! here — the illegal-op detection this module owns is enforced by the
//! compiler itself, not by a comment promising someone will remember.
//! Until then, whole-graph classification over the representable set is
//! the entire job, and every reachable verdict is `Legal` — proven by
//! the unit tests below (plain/transitive/recursive/mutually-recursive/
//! generic-instantiation/method/closure cases), not asserted by fiat.
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
//! its own typed body, carry whatever that default itself calls. Since
//! nothing illegal is representable anywhere yet (see above), this has
//! no effect on any verdict today; it is recorded here as a deliberate,
//! disclosed boundary for whoever revisits this once M5 makes an illegal
//! field default constructible, rather than silently mishandled.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::sema::SemaError;
use crate::sema::typed::{
    TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn, TypedForIter,
    TypedInstantiation, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind,
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
    /// as legal by default: with nothing illegal representable in the
    /// typed tree at all (see the module doc), there is no information
    /// to the contrary, and "unknown" cannot itself be one of decision
    /// 7's named illegal operations.
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
        TypedExprKind::Int(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::Str(_)
        | TypedExprKind::BStr(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Unit
        | TypedExprKind::Local(_)
        | TypedExprKind::Const(_)
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

/// The same body walk `scan_stmts` performs, asking the hardware question
/// instead of the legality one. Deliberately a second walk rather than a
/// second field on `BodyScan`: `classify`'s scan runs on every program and
/// this one only ever runs when a capability exists somewhere, and folding
/// the two would make `classify`'s hot path pay for a rule it does not
/// use. A closure's body folds into its enclosing fn here for the identical
/// reason it does there (plans/M3.md decision 4) — which is also how 03 §1's
/// "or captures" is covered for a *driver*'s own closures.
fn scan_hardware_stmts(stmts: &[TypedStmt], authority: &Authority, scan: &mut BodyScan) {
    for s in stmts {
        scan_hardware_stmt(s, authority, scan);
    }
}

fn scan_hardware_stmt(stmt: &TypedStmt, authority: &Authority, scan: &mut BodyScan) {
    match &stmt.kind {
        TypedStmtKind::Let { value, .. } => scan_hardware_expr(value, authority, scan),
        TypedStmtKind::Assign { target, value } => {
            scan_hardware_expr(target, authority, scan);
            scan_hardware_expr(value, authority, scan);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            scan_hardware_expr(cond, authority, scan);
            scan_hardware_stmts(then_branch, authority, scan);
            for elif in elifs {
                scan_hardware_expr(&elif.cond, authority, scan);
                scan_hardware_stmts(&elif.body, authority, scan);
            }
            if let Some(b) = else_branch {
                scan_hardware_stmts(b, authority, scan);
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            scan_hardware_expr(scrutinee, authority, scan);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    scan_hardware_expr(g, authority, scan);
                }
                scan_hardware_stmts(&arm.body, authority, scan);
            }
        }
        TypedStmtKind::For { iter, body, .. } => {
            match iter {
                TypedForIter::Range(from, to, _) => {
                    scan_hardware_expr(from, authority, scan);
                    scan_hardware_expr(to, authority, scan);
                }
                TypedForIter::Expr(e) => scan_hardware_expr(e, authority, scan),
            }
            scan_hardware_stmts(body, authority, scan);
        }
        TypedStmtKind::While { cond, body } => {
            scan_hardware_expr(cond, authority, scan);
            scan_hardware_stmts(body, authority, scan);
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => {}
        TypedStmtKind::Return(value) => {
            if let Some(e) = value {
                scan_hardware_expr(e, authority, scan);
            }
        }
        TypedStmtKind::Assert { cond, message } => {
            scan_hardware_expr(cond, authority, scan);
            if let Some(m) = message {
                scan_hardware_expr(m, authority, scan);
            }
        }
        TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            scan_hardware_expr(cond, authority, scan);
            if let Some(m) = message {
                scan_hardware_expr(m, authority, scan);
            }
        }
        TypedStmtKind::Defer(body) => match body {
            TypedDeferBody::Expr(e) => scan_hardware_expr(e, authority, scan),
            TypedDeferBody::Suite(stmts) => scan_hardware_stmts(stmts, authority, scan),
        },
        TypedStmtKind::ExprStmt(e) => scan_hardware_expr(e, authority, scan),
        TypedStmtKind::BareSend { expr, .. } => scan_hardware_expr(expr, authority, scan),
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                scan_hardware_expr(c, authority, scan);
            }
            if let Some(d) = deadline {
                scan_hardware_expr(d, authority, scan);
            }
            scan_hardware_stmts(body, authority, scan);
        }
    }
}

fn scan_hardware_expr(e: &TypedExpr, authority: &Authority, scan: &mut BodyScan) {
    if let Some(reason) = expr_hardware_reason(e, authority) {
        if scan.illegal.is_none() {
            scan.illegal = Some(reason);
        }
    }
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
        | TypedExprKind::FnRef(_)
        | TypedExprKind::PoolName(_)
        | TypedExprKind::GroupChild(_) => {}
        TypedExprKind::Field(base, _) => scan_hardware_expr(base, authority, scan),
        TypedExprKind::Index(base, idx) => {
            scan_hardware_expr(base, authority, scan);
            scan_hardware_expr(idx, authority, scan);
        }
        TypedExprKind::Call { receiver, args, .. } => {
            if let Some(r) = receiver {
                scan_hardware_expr(r, authority, scan);
            }
            for a in args.iter().flatten() {
                scan_hardware_expr(a, authority, scan);
            }
        }
        TypedExprKind::CallValue(callee, args) => {
            scan_hardware_expr(callee, authority, scan);
            for a in args {
                scan_hardware_expr(a, authority, scan);
            }
        }
        TypedExprKind::ToScalar(inner)
        | TypedExprKind::Neg(inner)
        | TypedExprKind::BitNot(inner)
        | TypedExprKind::Take(inner)
        | TypedExprKind::Not(inner)
        | TypedExprKind::Panic(inner)
        | TypedExprKind::Await(inner)
        | TypedExprKind::Send(inner) => scan_hardware_expr(inner, authority, scan),
        TypedExprKind::Try(inner, _) => scan_hardware_expr(inner, authority, scan),
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::OpCall(_, l, r)
        | TypedExprKind::And(l, r)
        | TypedExprKind::Or(l, r) => {
            scan_hardware_expr(l, authority, scan);
            scan_hardware_expr(r, authority, scan);
        }
        TypedExprKind::Is(inner, _) => scan_hardware_expr(inner, authority, scan),
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                scan_hardware_expr(a, authority, scan);
            }
        }
        TypedExprKind::Closure { body, .. } => match body {
            TypedClosureBody::Expr(e) => scan_hardware_expr(e, authority, scan),
            TypedClosureBody::Suite(stmts) => scan_hardware_stmts(stmts, authority, scan),
        },
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                scan_hardware_expr(i, authority, scan);
            }
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                scan_hardware_expr(v, authority, scan);
            }
        }
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                scan_hardware_expr(r, authority, scan);
            }
            for (_, a) in args {
                scan_hardware_expr(a, authority, scan);
            }
        }
    }
}

// --- the callee graph: one node per classifiable key -----------------------

/// One graph node: a fn/method/instantiation body's own direct callees
/// (by `CalleeKey::spelling()`) and whether its own body directly
/// contains a decision-7 illegal operation (today: never — see the
/// module doc's exhaustive-match argument).
struct NodeInfo {
    display_name: String,
    callees: BTreeSet<String>,
    illegal: Option<String>,
}

/// Registers every classifiable key `TypedProgram` carries: top-level
/// fns (`program.fns`, keyed by bare name — identical to
/// `CalleeKey::Fn`'s own spelling), each plain struct's methods/
/// assoc-fns/init (keyed `Struct.member`, identical to
/// `CalleeKey::Method`'s spelling), and every instantiation
/// (`program.instantiations`, already keyed by
/// `generics::canonical_key`'s spelling — identical to
/// `CalleeKey::FnInstance`; a struct instantiation's own methods/
/// assoc-fns/init are additionally registered `key.member`, identical to
/// `CalleeKey::MethodInstance`'s spelling). An enum instantiation has no
/// body (typed.rs: "enums carry no methods") — nothing to register, and
/// nothing any `Call` node could ever target.
fn build_nodes(program: &TypedProgram) -> BTreeMap<String, NodeInfo> {
    let mut nodes = BTreeMap::new();

    for (name, f) in &program.fns {
        insert_fn_node(&mut nodes, name.clone(), f);
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

// --- the illegal-op scan: exhaustive, so a new node kind forces a real
// arm here rather than silently defaulting to "legal" (see module doc).

fn expr_illegal_reason(kind: &TypedExprKind) -> Option<&'static str> {
    match kind {
        TypedExprKind::Int(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::Str(_)
        | TypedExprKind::BStr(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Unit
        | TypedExprKind::Local(_)
        | TypedExprKind::Const(_)
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
        | TypedExprKind::PoolName(_) => None,
        // plans/M4.md item B, decision 5: the ten graph-building builder
        // intrinsics (`typed::is_restricted_intrinsic`) are illegal
        // anywhere but the one reachable `@image` fn — `classify` clears
        // this flag on that one fn's own node afterward, so this arm
        // does not need to know which fn is currently being scanned.
        // `RestartIntensity`/`seconds` share the node kind but are
        // ordinary prelude helpers (excluded from the restricted set),
        // so they fall through to `None`, legal everywhere.
        TypedExprKind::Intrinsic { key, .. } => {
            if crate::sema::typed::is_restricted_intrinsic(key) {
                Some("an `@image` builder intrinsic")
            } else if key == "now" {
                // Plans/M6.md item A, decision 11: `now()` is
                // runtime-only (illegal in every comptime context) — the
                // new illegal-reason arm decision 11 asks for, mirroring
                // the intrinsic-outside-`@image` precedent just above.
                // `ms(n)` shares this node kind but stays comptime-legal
                // (falls through to `None`, same as `seconds`/
                // `RestartIntensity`).
                Some("`now()` (a runtime-only clock read)")
            } else if key.starts_with("Group.") {
                // `Group.start`/`Group.join_all` (plans/M6.md item A):
                // actor/async operations, illegal in every comptime
                // context (02-language.md §12).
                Some("a `group` construct")
            } else {
                None
            }
        }
        // Plans/M6.md item A: `await`/`send` are both decision-7 illegal
        // ops (02-language.md §12: "free of ... async/actor operations").
        TypedExprKind::Await(_) => Some("an `await` expression"),
        TypedExprKind::Send(_) => Some("a `send` expression"),
        // Never independently illegal — the enclosing `Group.start`
        // intrinsic (above) already flags illegal; this is only ever a
        // leaf naming that intrinsic's own callee argument.
        TypedExprKind::GroupChild(_) => None,
    }
}

fn stmt_illegal_reason(kind: &TypedStmtKind) -> Option<&'static str> {
    match kind {
        TypedStmtKind::Let { .. }
        | TypedStmtKind::Assign { .. }
        | TypedStmtKind::If { .. }
        | TypedStmtKind::Match { .. }
        | TypedStmtKind::For { .. }
        | TypedStmtKind::While { .. }
        | TypedStmtKind::Break
        | TypedStmtKind::Continue
        | TypedStmtKind::Pass
        | TypedStmtKind::Return(_)
        | TypedStmtKind::Assert { .. }
        | TypedStmtKind::ComptimeAssert { .. }
        | TypedStmtKind::Defer(_)
        | TypedStmtKind::ExprStmt(_) => None,
        // Plans/M6.md item G: the bare `send` statement form — the same
        // decision-7 illegal op `TypedExprKind::Send` already is (the
        // expression inside would flag it anyway; naming it here keeps
        // the diagnostic's own wording about the statement the author
        // actually wrote).
        TypedStmtKind::BareSend { .. } => Some("a `send` statement"),
        // Plans/M6.md item A: `with group(...)` is a decision-7 illegal
        // op (an actor/async construct, 02-language.md §12).
        TypedStmtKind::WithGroup { .. } => Some("a `with group` block"),
    }
}

// --- the walk: collects callee keys, folding closure bodies into
// whichever enclosing node they were found in (module doc) ------------------

fn scan_stmts(stmts: &[TypedStmt], scan: &mut BodyScan) {
    for s in stmts {
        scan_stmt(s, scan);
    }
}

fn scan_stmt(stmt: &TypedStmt, scan: &mut BodyScan) {
    if let Some(reason) = stmt_illegal_reason(&stmt.kind) {
        scan.note_illegal(reason);
    }
    match &stmt.kind {
        TypedStmtKind::Let { value, .. } => scan_expr(value, scan),
        TypedStmtKind::Assign { target, value } => {
            scan_expr(target, scan);
            scan_expr(value, scan);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            scan_expr(cond, scan);
            scan_stmts(then_branch, scan);
            for elif in elifs {
                scan_expr(&elif.cond, scan);
                scan_stmts(&elif.body, scan);
            }
            if let Some(b) = else_branch {
                scan_stmts(b, scan);
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            scan_expr(scrutinee, scan);
            for arm in arms {
                scan_pattern(&arm.pattern, scan);
                if let Some(g) = &arm.guard {
                    scan_expr(g, scan);
                }
                scan_stmts(&arm.body, scan);
            }
        }
        TypedStmtKind::For { iter, body, .. } => {
            match iter {
                TypedForIter::Range(from, to, _) => {
                    scan_expr(from, scan);
                    scan_expr(to, scan);
                }
                TypedForIter::Expr(e) => scan_expr(e, scan),
            }
            scan_stmts(body, scan);
        }
        TypedStmtKind::While { cond, body } => {
            scan_expr(cond, scan);
            scan_stmts(body, scan);
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => {}
        TypedStmtKind::Return(value) => {
            if let Some(e) = value {
                scan_expr(e, scan);
            }
        }
        TypedStmtKind::Assert { cond, message } => {
            scan_expr(cond, scan);
            if let Some(m) = message {
                scan_expr(m, scan);
            }
        }
        TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            scan_expr(cond, scan);
            if let Some(m) = message {
                scan_expr(m, scan);
            }
        }
        TypedStmtKind::Defer(body) => match body {
            TypedDeferBody::Expr(e) => scan_expr(e, scan),
            TypedDeferBody::Suite(stmts) => scan_stmts(stmts, scan),
        },
        TypedStmtKind::ExprStmt(e) => scan_expr(e, scan),
        TypedStmtKind::BareSend { expr, .. } => scan_expr(expr, scan),
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                scan_expr(c, scan);
            }
            if let Some(d) = deadline {
                scan_expr(d, scan);
            }
            scan_stmts(body, scan);
        }
    }
}

fn scan_expr(e: &TypedExpr, scan: &mut BodyScan) {
    if let Some(reason) = expr_illegal_reason(&e.kind) {
        scan.note_illegal(reason);
    }
    match &e.kind {
        TypedExprKind::Int(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::Str(_)
        | TypedExprKind::BStr(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Unit
        | TypedExprKind::Local(_)
        | TypedExprKind::Const(_) => {}
        TypedExprKind::FnRef(key) => {
            scan.callees.insert(key.spelling());
        }
        TypedExprKind::Field(base, _) => scan_expr(base, scan),
        TypedExprKind::Index(base, idx) => {
            scan_expr(base, scan);
            scan_expr(idx, scan);
        }
        TypedExprKind::Call {
            callee,
            receiver,
            args,
        } => {
            scan.callees.insert(callee.spelling());
            if let Some(r) = receiver {
                scan_expr(r, scan);
            }
            for a in args {
                if let Some(a) = a {
                    scan_expr(a, scan);
                }
            }
        }
        TypedExprKind::CallValue(callee, args) => {
            // The callee expr is walked like any other: a `FnRef(key)`
            // here still adds its edge (the `FnRef` arm above), a
            // `Closure` here still folds its body in (the `Closure` arm
            // below) — an unresolvable target (e.g. a `Local` holding a
            // fn value passed in from elsewhere) contributes no edge,
            // which is the honest "no information" default (module doc).
            scan_expr(callee, scan);
            for a in args {
                scan_expr(a, scan);
            }
        }
        TypedExprKind::ToScalar(inner)
        | TypedExprKind::Neg(inner)
        | TypedExprKind::BitNot(inner)
        | TypedExprKind::Take(inner)
        | TypedExprKind::Not(inner) => scan_expr(inner, scan),
        TypedExprKind::Try(inner, conv) => {
            scan_expr(inner, scan);
            if let Some(key) = conv {
                scan.callees.insert(key.spelling());
            }
        }
        TypedExprKind::Binary(_, l, r) => {
            scan_expr(l, scan);
            scan_expr(r, scan);
        }
        TypedExprKind::OpCall(key, l, r) => {
            scan.callees.insert(key.spelling());
            scan_expr(l, scan);
            scan_expr(r, scan);
        }
        TypedExprKind::Is(inner, pat) => {
            scan_expr(inner, scan);
            scan_pattern(pat, scan);
        }
        TypedExprKind::And(l, r) | TypedExprKind::Or(l, r) => {
            scan_expr(l, scan);
            scan_expr(r, scan);
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                scan_expr(a, scan);
            }
        }
        TypedExprKind::Closure { body, .. } => match body {
            // Decision 4: a closure's body is part of its enclosing fn's
            // closure-set, not a graph node of its own — folded straight
            // into the same `scan` the enclosing body is accumulating.
            TypedClosureBody::Expr(e) => scan_expr(e, scan),
            TypedClosureBody::Suite(stmts) => scan_stmts(stmts, scan),
        },
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                scan_expr(i, scan);
            }
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                scan_expr(v, scan);
            }
        }
        TypedExprKind::Panic(msg) => scan_expr(msg, scan),
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                scan_expr(r, scan);
            }
            for (_, a) in args {
                scan_expr(a, scan);
            }
        }
        TypedExprKind::PoolName(_) => {}
        TypedExprKind::Await(inner) | TypedExprKind::Send(inner) => scan_expr(inner, scan),
        TypedExprKind::GroupChild(key) => {
            // Mirrors `FnRef`'s own arm: names a callee edge (the
            // enclosing fn is already independently marked illegal by
            // the `Group.start` intrinsic itself, above — this edge is
            // only for the graph's own completeness, same reasoning as
            // `FnRef`).
            scan.callees.insert(key.spelling());
        }
    }
}

fn scan_pattern(p: &TypedPattern, scan: &mut BodyScan) {
    match &p.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => {}
        TypedPatternKind::Literal(e) => scan_expr(e, scan),
        TypedPatternKind::Take(inner) => scan_pattern(inner, scan),
        TypedPatternKind::Variant { payload, .. } => {
            for p in payload {
                scan_pattern(p, scan);
            }
        }
        TypedPatternKind::Tuple(elems) | TypedPatternKind::Array(elems) => {
            for p in elems {
                scan_pattern(p, scan);
            }
        }
        TypedPatternKind::Or(alts) => {
            for p in alts {
                scan_pattern(p, scan);
            }
        }
    }
}

// --- unit tests --------------------------------------------------------
//
// Built from real source through the real pipeline (`sema::check_typed`)
// wherever the construct is representable — no hand-built `TypedProgram`
// fixtures. The await/send/with/pool-operation/`@image` cases the M3-C
// task's own test list names are deliberately *not* here: `check_typed`
// fails closed (`error[unimplemented]`) on every one of them before a
// `TypedProgram` exists at all (module doc), so there is no legal way to
// produce a fixture that reaches `classify` containing one — faking one
// would mean hand-constructing a `TypedProgram` around a typed-tree node
// kind that does not exist. `golden/err-unimplemented-await` already
// pins the await case at the sema layer; the other three fail closed the
// identical way. Their day is M5, when `typed.rs` gains real nodes for
// them and this file's exhaustive match forces real arms.

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
}
