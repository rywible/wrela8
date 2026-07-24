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
}
