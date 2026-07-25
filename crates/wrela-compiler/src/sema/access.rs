//! Access-mode checking (plans/M2.md item D): call-site mirroring of
//! `mut`/`take` markers against declared parameter modes (02-language.md
//! §5.1), the place-expression requirement on a mirrored operand, `pub`
//! methods spelling their receiver effect (private plain-`self` methods
//! get the least effect inferred instead), `read`-parameter loans (a
//! `read` binding — including `read self` — can never be assigned, have
//! a field assigned, or be passed onward as `mut`/`take`), and the
//! "calling through a receiver" rule: a `mut self` method needs a
//! mutable place, a `take self` method needs an owned one.
//!
//! Shape (decision 3/10): this pass runs after `bodies.rs`, which has
//! already proven the whole (non-generic) module type-checks — arity,
//! labels, and every expression's type are already known-good. This pass
//! does **not** re-derive full type inference; it re-walks the same
//! bodies with a much smaller "which struct/method does this call
//! resolve to" question, reusing `bodies::build_module_ctx`'s lookup
//! tables wholesale (widened `pub(crate)` there, plans/M2.md item D's
//! footprint rule — nothing in `bodies.rs`/`types.rs` is restructured).
//! Tracking is scoped to what a call target actually needs: every
//! tracked name (a parameter, `self`, or a closure parameter) keeps its
//! declared type and mode; a plain local introduced by assignment/
//! pattern/`for` keeps a best-effort type (propagated from its
//! initializer through the same lookup) and no mode at all (an ordinary
//! owned binding is never restricted). A method call whose receiver
//! expression's type cannot be determined this way (a chain rooted in
//! something other than a tracked name, e.g. a call whose own callee this
//! pass cannot resolve) fails closed rather than silently skipping
//! mirroring; nothing in the M2 sema corpus (goldens in this commit
//! included) needs more than this.
//!
//! Receiver-effect inference (item 3): the AST cannot distinguish a
//! plain `self` from an explicitly spelled `read self` (types.rs's
//! `DeclReceiver` doc comment) — so a private method's *inferred* effect
//! is the only thing ever computed here, and the `pub`-must-spell rule is
//! checked the only honest way available: a `pub` method with the
//! ambiguous `Read` receiver mode is an error exactly when its body's
//! inferred requirement exceeds `read` (it needed `mut`/`take` and didn't
//! say so); a `pub` method that already spells `mut self`/`take self` is
//! never second-guessed. Inference itself is a dumb fixpoint over every
//! private plain-`self` method in the module at once (self-calls are
//! always same-struct, so a whole-module round-robin is simplest and
//! correct): start every candidate at `read`, escalate to `mut` (a field
//! of `self` is assigned; `self` or a field of `self` is passed as a
//! `mut` argument; a field of `self` is taken, which per 02 §3.2 must be
//! restorable and so needs `mut` access, not full ownership; a `mut
//! self`/`take self` method is called directly on `self`) or `take`
//! (`self` itself, whole, is taken; a `take self` method is called
//! directly on `self`), to a fixed point. A private method that needs
//! more than a *direct* `self.method()`/`self.field = ...` pattern to
//! reach its true requirement (e.g. a nested `self.field.mutate()` call
//! escalating through a field's own type) is not chased further by
//! inference — the honest fallback is that such a method must spell its
//! receiver explicitly, exactly like a `pub` method would; the "calling
//! through a receiver" check below (which *does* resolve nested field
//! chains for the mutability question) still enforces it correctly
//! either way, so this is a documented scope limit, not a soundness gap
//! (see the session report for the worked-through reasoning).

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::bodies::{self, ModuleCtx, StructInfo};
use crate::sema::generics;
use crate::sema::types::{self, DeclFn, DeclItem, DeclMember, DeclParam, Type};
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{
    self, AccessMode, Arg, AssignStmt, ClosureBody, ClosureExpr, DeferBody, Expr, ForStmt, IfStmt,
    Item, MatchStmt, Member, Module, Pattern, Span, Stmt, UnaryOp, WhileStmt,
};

/// `(struct name, method name) -> inferred receiver effect`, for every
/// private plain-`self` method in the module — the check dump's own
/// lookup key (types.rs's `render_member`) and this pass's internal one.
pub type EffectMap = BTreeMap<(String, String), AccessMode>;

/// Computes the receiver-effect map alone, with no error checking — used
/// by `check` below and, independently, by `mod.rs`'s `dump` (decision 8:
/// private methods print their inferred effect once this pass lands).
pub fn infer_effects(module: &Module, decl_items: &[DeclItem]) -> EffectMap {
    let mctx = bodies::build_module_ctx(module, decl_items);
    infer_private_effects(&mctx)
}

/// `pub(crate)` (item H, generics.rs): the same computation as
/// `infer_effects` above, but from an already-built `ModuleCtx` — the
/// shared one every pass now uses (see `check`'s own doc comment) — since
/// `generics::check` only ever has that, never a fresh `module`+
/// `decl_items` pair to rebuild one from.
pub(crate) fn infer_effects_over(mctx: &ModuleCtx) -> EffectMap {
    infer_private_effects(mctx)
}

/// The access pass (plans/M2.md item D): fail-fast, source order, same
/// module-wide traversal shape as `bodies::check`. `mctx` is shared with
/// `bodies::check`/`matches::check`/`generics::check` (built once by
/// `mod.rs::check`) so item H's instantiation queue accumulates across
/// every pass that discovers a generic use (see `bodies.rs`'s doc comment
/// on `InstKind`).
pub(crate) fn check(
    module: &Module,
    decl_items: &[DeclItem],
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let effects = infer_private_effects(mctx);
    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();
    for (ai, di) in ast_items.iter().zip(decl_items.iter()) {
        match (ai, di) {
            (Item::Const(c), DeclItem::Const(_)) => {
                let mut actx = ACtx::new(mctx, &effects, BTreeSet::new());
                check_expr(&c.value, &mut actx)?;
            }
            (Item::Fn(f), DeclItem::Fn(d)) => check_top_fn(f, d, mctx, &effects)?,
            (Item::Struct(s), DeclItem::Struct(_)) => check_struct_bodies(s, mctx, &effects)?,
            _ => {}
        }
    }
    Ok(())
}

// --- receiver-effect inference (item 3) -----------------------------------

fn rank(m: AccessMode) -> u8 {
    match m {
        AccessMode::Read => 0,
        AccessMode::Mut => 1,
        AccessMode::Take => 2,
    }
}

fn escalate(acc: &mut AccessMode, m: AccessMode) {
    if rank(m) > rank(*acc) {
        *acc = m;
    }
}

/// Every private, plain-`self` (`Read`, not `pub`), non-generic method in
/// a struct — generic or not. Receiver-effect inference is purely
/// structural (self-calls always resolve within the same struct; nothing
/// below reads a field's or parameter's *type*, only `self`/marker/call
/// shapes), so a generic struct's own declared methods get exactly one
/// inferred effect apiece, computed once here from the *declaration*
/// (unsubstituted `T`s and all) and reused unchanged by every concrete
/// instantiation item H later checks — recomputing it per instantiation
/// would ask the exact same structural question of the exact same body
/// every time. A method that is itself separately generic (its own
/// `[...]`, beyond the struct's) is still skipped: item H's own scope
/// boundary (a generic method is never instantiated, so it is never
/// checked, so there is nothing to infer an effect *for*).
fn private_candidates(mctx: &ModuleCtx) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (sname, s) in &mctx.structs {
        for (am, dm) in s.members() {
            if let (Member::Fn(f), DeclMember::Fn(d)) = (am, dm) {
                if f.generics.is_empty() {
                    if let Some(r) = &d.receiver {
                        if r.mode == AccessMode::Read && !r.is_pub {
                            out.push((sname.clone(), f.name.clone()));
                        }
                    }
                }
            }
        }
    }
    out
}

fn infer_private_effects(mctx: &ModuleCtx) -> EffectMap {
    let candidates = private_candidates(mctx);
    let mut effects: EffectMap = candidates
        .iter()
        .cloned()
        .map(|k| (k, AccessMode::Read))
        .collect();
    loop {
        let mut changed = false;
        for key in &candidates {
            let (sname, mname) = key;
            let s = &mctx.structs[sname];
            let (f, _d) = s
                .method(mname)
                .expect("private_candidates only names real methods");
            // A bodyless declaration (the grammar's own "bodyless
            // forms") has nothing to infer from — it stays at the
            // fixpoint's own initial `read` default. This is load-bearing,
            // not vacuous: `private_candidates` names methods of every
            // struct, generic or not, but `bodies::check` never body-checks
            // (and so never fails closed on) an *uninstantiated* generic
            // struct's methods (`check_struct_bodies` skips generic
            // structs entirely) — so a generic struct's bodyless method
            // really can reach this skip.
            let Some(body) = &f.body else {
                continue;
            };
            let required = required_self_effect(body, sname, mctx, &effects);
            if rank(required) > rank(effects[key]) {
                effects.insert(key.clone(), required);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    effects
}

/// What a `self.method()` call's own receiver mode contributes to the
/// *caller's* required effect: explicit `mut`/`take` are used as-is; an
/// explicit `pub read self` never escalates (it is exactly what it
/// says); a private plain-`self` callee defers to its own (possibly
/// still-being-computed) fixpoint entry.
fn effective_declared(
    mode: AccessMode,
    is_pub: bool,
    sname: &str,
    mname: &str,
    effects: &EffectMap,
) -> AccessMode {
    match mode {
        AccessMode::Mut => AccessMode::Mut,
        AccessMode::Take => AccessMode::Take,
        AccessMode::Read => {
            if is_pub {
                AccessMode::Read
            } else {
                effects
                    .get(&(sname.to_string(), mname.to_string()))
                    .copied()
                    .unwrap_or(AccessMode::Read)
            }
        }
    }
}

fn required_self_effect(
    body: &[Stmt],
    sname: &str,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> AccessMode {
    let mut acc = AccessMode::Read;
    scan_stmts_self(body, sname, mctx, effects, &mut acc);
    acc
}

/// How an expression refers to `self`, for the escalation rules: exactly
/// `self` (a whole-value use), a field/index chain rooted at `self` (a
/// partial reference), or unrelated.
enum SelfRef {
    None,
    Whole,
    Field,
}

fn self_ref(e: &Expr) -> SelfRef {
    match e {
        Expr::Name(_, n) if n == "self" => SelfRef::Whole,
        Expr::Field(..) | Expr::Index(..) => {
            if place_root_name(e) == Some("self") {
                SelfRef::Field
            } else {
                SelfRef::None
            }
        }
        _ => SelfRef::None,
    }
}

fn scan_stmts_self(
    stmts: &[Stmt],
    sname: &str,
    mctx: &ModuleCtx,
    effects: &EffectMap,
    acc: &mut AccessMode,
) {
    for s in stmts {
        scan_stmt_self(s, sname, mctx, effects, acc);
    }
}

fn scan_stmt_self(
    stmt: &Stmt,
    sname: &str,
    mctx: &ModuleCtx,
    effects: &EffectMap,
    acc: &mut AccessMode,
) {
    match stmt {
        Stmt::Assign(a) => {
            match self_ref(&a.target) {
                SelfRef::Whole | SelfRef::Field => escalate(acc, AccessMode::Mut),
                SelfRef::None => {}
            }
            scan_expr_self(&a.target, sname, mctx, effects, acc);
            scan_expr_self(&a.value, sname, mctx, effects, acc);
        }
        Stmt::If(i) => {
            scan_expr_self(&i.cond, sname, mctx, effects, acc);
            scan_stmts_self(&i.then_branch, sname, mctx, effects, acc);
            for elif in &i.elifs {
                scan_expr_self(&elif.cond, sname, mctx, effects, acc);
                scan_stmts_self(&elif.body, sname, mctx, effects, acc);
            }
            if let Some(b) = &i.else_branch {
                scan_stmts_self(b, sname, mctx, effects, acc);
            }
        }
        Stmt::Match(m) => {
            scan_expr_self(&m.scrutinee, sname, mctx, effects, acc);
            for arm in &m.arms {
                if let Some(g) = &arm.guard {
                    scan_expr_self(g, sname, mctx, effects, acc);
                }
                scan_stmts_self(&arm.body, sname, mctx, effects, acc);
            }
        }
        Stmt::For(f) => {
            scan_expr_self(&f.iterable, sname, mctx, effects, acc);
            scan_stmts_self(&f.body, sname, mctx, effects, acc);
        }
        Stmt::While(w) => {
            scan_expr_self(&w.cond, sname, mctx, effects, acc);
            scan_stmts_self(&w.body, sname, mctx, effects, acc);
        }
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => {}
        Stmt::Return(_, e) => {
            if let Some(e) = e {
                scan_expr_self(e, sname, mctx, effects, acc);
            }
        }
        Stmt::Assert(a) => {
            scan_expr_self(&a.cond, sname, mctx, effects, acc);
            if let Some(m) = &a.message {
                scan_expr_self(m, sname, mctx, effects, acc);
            }
        }
        Stmt::Defer(d) => match &d.body {
            DeferBody::Expr(e) => scan_expr_self(e, sname, mctx, effects, acc),
            DeferBody::Suite(s) => scan_stmts_self(s, sname, mctx, effects, acc),
        },
        Stmt::With(w) => {
            scan_expr_self(&w.expr, sname, mctx, effects, acc);
            scan_stmts_self(&w.body, sname, mctx, effects, acc);
        }
        Stmt::Send(_, e) => scan_expr_self(e, sname, mctx, effects, acc),
        Stmt::Expr(_, e) => scan_expr_self(e, sname, mctx, effects, acc),
        Stmt::ComptimeIf(c) => {
            scan_expr_self(&c.cond, sname, mctx, effects, acc);
            scan_stmts_self(&c.then_branch, sname, mctx, effects, acc);
            if let Some(b) = &c.else_branch {
                scan_stmts_self(b, sname, mctx, effects, acc);
            }
        }
        Stmt::ComptimeAssert(_, e, m) => {
            scan_expr_self(e, sname, mctx, effects, acc);
            if let Some(m) = m {
                scan_expr_self(m, sname, mctx, effects, acc);
            }
        }
    }
}

fn scan_expr_self(
    e: &Expr,
    sname: &str,
    mctx: &ModuleCtx,
    effects: &EffectMap,
    acc: &mut AccessMode,
) {
    match e {
        Expr::Unary(_, UnaryOp::Take, inner) => {
            match self_ref(inner) {
                SelfRef::Whole => escalate(acc, AccessMode::Take),
                SelfRef::Field => escalate(acc, AccessMode::Mut),
                SelfRef::None => {}
            }
            scan_expr_self(inner, sname, mctx, effects, acc);
        }
        Expr::Unary(_, _, inner) => scan_expr_self(inner, sname, mctx, effects, acc),
        Expr::Field(base, _, _) => scan_expr_self(base, sname, mctx, effects, acc),
        Expr::Index(base, _, args) => {
            scan_expr_self(base, sname, mctx, effects, acc);
            for a in args {
                scan_expr_self(a, sname, mctx, effects, acc);
            }
        }
        Expr::Call(callee, _, args) => {
            for a in args {
                if a.mode != AccessMode::Read {
                    match (a.mode, self_ref(&a.value)) {
                        (AccessMode::Take, SelfRef::Whole) => escalate(acc, AccessMode::Take),
                        (AccessMode::Take, SelfRef::Field) => escalate(acc, AccessMode::Mut),
                        (AccessMode::Mut, SelfRef::Whole) | (AccessMode::Mut, SelfRef::Field) => {
                            escalate(acc, AccessMode::Mut)
                        }
                        _ => {}
                    }
                }
                scan_expr_self(&a.value, sname, mctx, effects, acc);
            }
            if let Expr::Field(base, _, name) = &**callee {
                if matches!(self_ref(base), SelfRef::Whole) {
                    if let Some(s) = mctx.structs.get(sname) {
                        if let Some((_, d)) = s.method(name) {
                            if let Some(r) = &d.receiver {
                                escalate(
                                    acc,
                                    effective_declared(r.mode, r.is_pub, sname, name, effects),
                                );
                            }
                        }
                    }
                }
            }
            scan_expr_self(callee, sname, mctx, effects, acc);
        }
        Expr::Try(_, inner) => scan_expr_self(inner, sname, mctx, effects, acc),
        Expr::Binary(_, _, l, r) | Expr::Range(_, l, r, _) => {
            scan_expr_self(l, sname, mctx, effects, acc);
            scan_expr_self(r, sname, mctx, effects, acc);
        }
        Expr::Is(_, scrutinee, _) => scan_expr_self(scrutinee, sname, mctx, effects, acc),
        Expr::Not(_, inner) => scan_expr_self(inner, sname, mctx, effects, acc),
        Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            scan_expr_self(l, sname, mctx, effects, acc);
            scan_expr_self(r, sname, mctx, effects, acc);
        }
        Expr::DotVariant(_, _, args) => {
            for a in args {
                scan_expr_self(&a.value, sname, mctx, effects, acc);
            }
        }
        Expr::Closure(c) => match &c.body {
            ClosureBody::Expr(e) => scan_expr_self(e, sname, mctx, effects, acc),
            ClosureBody::Suite(s) => scan_stmts_self(s, sname, mctx, effects, acc),
        },
        Expr::Send(_, inner) => scan_expr_self(inner, sname, mctx, effects, acc),
        Expr::Tuple(_, items) | Expr::List(_, items) => {
            for i in items {
                scan_expr_self(i, sname, mctx, effects, acc);
            }
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::BStr(..)
        | Expr::Char(..)
        | Expr::FStr(_)
        | Expr::Bool(..)
        | Expr::Unit(_)
        | Expr::Name(..) => {}
    }
}

// --- shared place helpers --------------------------------------------------

/// The name a place expression ultimately roots at — a bare local, or the
/// base of a field/index chain — `None` for anything else (a call, a
/// literal, a binary expression, ...).
fn place_root_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Name(_, n) => Some(n.as_str()),
        Expr::Field(base, _, _) => place_root_name(base),
        Expr::Index(base, _, _) => place_root_name(base),
        _ => None,
    }
}

/// Whether `expr` is a full place expression per 02-language.md §3/§5.1:
/// a name, or a field/index chain rooted at one, all the way down.
/// Stricter than `ast::is_place_expr` (which only looks at the outermost
/// node — deliberately shallow so the parser can check it before a
/// receiver's own type is known), catching the case that shallow check
/// slips through: an outer `Field`/`Index` whose *base* is not itself a
/// place (`f().field`, `g()[0]`).
fn is_full_place(expr: &Expr) -> bool {
    match expr {
        Expr::Name(..) => true,
        Expr::Field(base, _, _) => is_full_place(base),
        Expr::Index(base, _, _) => is_full_place(base),
        _ => false,
    }
}

fn access_error(message: String, span: Span) -> SemaError {
    SemaError::at("access", message, span)
}

// --- the full check pass: mirroring, read loans, receiver mutability -----

/// One tracked name's declared type (best effort — see the module doc
/// comment) and access mode. `mode: None` marks an ordinary owned local
/// (introduced by assignment/pattern/`for`): unrestricted, always a legal
/// mutable/ownable place. `mode: Some(_)` marks a parameter or `self`:
/// `Read` is a loan (item 4); `Mut`/`Take` carry no restriction of their
/// own here (moving a borrowed value is item E/F's job) but do matter to
/// the "calling through a receiver" origin check.
#[derive(Clone)]
struct LocalInfo {
    ty: Option<Type>,
    mode: Option<AccessMode>,
}

struct ACtx<'a> {
    mctx: &'a ModuleCtx,
    effects: &'a EffectMap,
    locals: BTreeMap<String, LocalInfo>,
    local_pools: BTreeSet<String>,
}

impl<'a> ACtx<'a> {
    fn new(mctx: &'a ModuleCtx, effects: &'a EffectMap, local_pools: BTreeSet<String>) -> Self {
        ACtx {
            mctx,
            effects,
            locals: BTreeMap::new(),
            local_pools,
        }
    }

    /// Resolves an explicit local annotation exactly like `bodies.rs`
    /// did — the module already type-checked, so this can never fail.
    fn resolve_ann(&self, t: &ast::Type) -> Type {
        types::resolve_type(
            t,
            &self.mctx.shapes,
            &self.mctx.module_pools,
            &self.local_pools,
            &BTreeMap::new(),
            false,
        )
        .expect("annotation already validated by types::declare/bodies::check")
    }
}

/// `pub(crate)` (item H, generics.rs): re-run over a substituted,
/// generics-cleared copy of a generic fn's own ast+decl
/// (`generics::instantiate_fn`) — mirrors `bodies::check_top_fn`'s own
/// widening exactly, same reasoning.
pub(crate) fn check_top_fn(
    f: &ast::FnItem,
    d: &DeclFn,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> Result<(), SemaError> {
    if bodies::is_image_fn(f) || !f.generics.is_empty() {
        // `@image` bodies already fail closed in `bodies::check` (never
        // reached from here); a generic body is item H's job, mirroring
        // `bodies::check_top_fn`'s own skip exactly.
        return Ok(());
    }
    let mut actx = ACtx::new(mctx, effects, mctx.module_pools.clone());
    for (ap, dp) in f.params.iter().zip(d.params.iter()) {
        actx.locals.insert(
            dp.name.clone(),
            LocalInfo {
                ty: Some(dp.ty.clone()),
                mode: Some(dp.mode),
            },
        );
        if let Some(def) = &ap.default {
            check_expr(def, &mut actx)?;
        }
    }
    if let Some(body) = &f.body {
        check_stmts(body, &mut actx)?;
    }
    Ok(())
}

/// Resolves a struct method's *effective* receiver mode for this check
/// pass, raising the `pub`-must-spell error (item 3) the one place it is
/// honestly checkable: a `pub` method whose receiver mode is the
/// ambiguous `Read` and whose body needs more.
///
/// `pub(crate)` (items E/F, flow.rs): the flow pass needs this exact
/// effective mode too (a private plain-`self` method's *inferred* mode
/// decides whether `take self`/a field-take-and-restore is legal inside
/// it, and a receiver's effective mode at a call site decides what the
/// receiver operand's own access is) — re-deriving the same inference
/// would duplicate this pass wholesale, so flow.rs calls this verbatim
/// exactly like it reuses `bodies.rs`'s own machinery. Since `access::check`
/// always runs (and returns `Ok`) before `flow::check` in the frozen pass
/// order, the one error this can raise (a `pub` method's own missing
/// receiver spelling) can never actually fire when flow.rs calls it —
/// dumb re-derivation, not a new check.
pub(crate) fn resolve_receiver_mode(
    f: &ast::FnItem,
    fd: &DeclFn,
    sname: &str,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> Result<AccessMode, SemaError> {
    let Some(d) = &fd.receiver else {
        return Ok(AccessMode::Read); // an associated fn: no `self` at all.
    };
    match d.mode {
        AccessMode::Mut => Ok(AccessMode::Mut),
        AccessMode::Take => Ok(AccessMode::Take),
        AccessMode::Read => {
            if d.is_pub {
                // A bodyless declaration (the grammar's own "bodyless
                // forms", ledger `docs.examples.wrela-blocks-lex`) has
                // nothing to infer a requirement from. This path is
                // actually unreachable for a real bodyless fn, not a
                // vacuous acceptance: `resolve_receiver_mode` only ever
                // runs (via `check_struct_members`) on a non-generic
                // struct's methods, and `bodies::check` (frozen pass order,
                // `mod.rs::check`) always runs first and fails closed on a
                // bodyless one before this pass is ever reached — the
                // `Some(body) =` skip below is defensive, not load-bearing.
                let Some(body) = &f.body else {
                    return Ok(AccessMode::Read);
                };
                let required = required_self_effect(body, sname, mctx, effects);
                if required != AccessMode::Read {
                    let span = f
                        .receiver
                        .as_ref()
                        .expect("DeclFn.receiver implies an ast receiver")
                        .span;
                    return Err(access_error(
                        format!(
                            "pub method `{}` needs an explicit `{} self` receiver",
                            f.name,
                            required.as_str()
                        ),
                        span,
                    ));
                }
                Ok(AccessMode::Read)
            } else {
                Ok(effects
                    .get(&(sname.to_string(), f.name.clone()))
                    .copied()
                    .unwrap_or(AccessMode::Read))
            }
        }
    }
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
    let self_ty = Type::Named(s.name.clone(), vec![]);
    check_struct_members(info, self_ty, mctx, effects)
}

/// `pub(crate)` (item H, generics.rs): the guts of `check_struct_bodies`,
/// pulled out to take a `StructInfo` (and its `self_ty`) directly — see
/// `bodies::check_struct_members`'s own doc comment for why (`mctx`
/// always holds each struct's *declared* shape, never a substituted
/// instantiation's).
pub(crate) fn check_struct_members(
    info: &StructInfo,
    self_ty: Type,
    mctx: &ModuleCtx,
    effects: &EffectMap,
) -> Result<(), SemaError> {
    let local_pools = bodies::local_pool_names(info);
    let sname = info.decl.name.clone();
    for (am, dm) in info.members() {
        match (am, dm) {
            (Member::Field(af), DeclMember::Field(_)) => {
                if let Some(def) = &af.default {
                    let mut actx = ACtx::new(mctx, effects, local_pools.clone());
                    actx.locals.insert(
                        "self".to_string(),
                        LocalInfo {
                            ty: Some(self_ty.clone()),
                            mode: Some(AccessMode::Read),
                        },
                    );
                    check_expr(def, &mut actx)?;
                }
            }
            (Member::Fn(f), DeclMember::Fn(fd)) => {
                if !f.generics.is_empty() {
                    continue;
                }
                let self_mode = resolve_receiver_mode(f, fd, &sname, mctx, effects)?;
                let mut actx = ACtx::new(mctx, effects, local_pools.clone());
                actx.locals.insert(
                    "self".to_string(),
                    LocalInfo {
                        ty: Some(self_ty.clone()),
                        mode: Some(self_mode),
                    },
                );
                for (ap, dp) in f.params.iter().zip(fd.params.iter()) {
                    actx.locals.insert(
                        dp.name.clone(),
                        LocalInfo {
                            ty: Some(dp.ty.clone()),
                            mode: Some(dp.mode),
                        },
                    );
                    if let Some(def) = &ap.default {
                        check_expr(def, &mut actx)?;
                    }
                }
                if let Some(body) = &f.body {
                    check_stmts(body, &mut actx)?;
                }
            }
            (Member::Init(i), DeclMember::Init(fd)) => {
                // `init`'s receiver always prints (and is treated as)
                // explicit (types.rs's `DeclReceiver` doc comment) — no
                // pub-spelling question applies (`init` is never `pub`).
                let self_mode = fd
                    .receiver
                    .as_ref()
                    .map(|r| r.mode)
                    .unwrap_or(AccessMode::Read);
                let mut actx = ACtx::new(mctx, effects, local_pools.clone());
                actx.locals.insert(
                    "self".to_string(),
                    LocalInfo {
                        ty: Some(self_ty.clone()),
                        mode: Some(self_mode),
                    },
                );
                for (ap, dp) in i.params.iter().zip(fd.params.iter()) {
                    actx.locals.insert(
                        dp.name.clone(),
                        LocalInfo {
                            ty: Some(dp.ty.clone()),
                            mode: Some(dp.mode),
                        },
                    );
                    if let Some(def) = &ap.default {
                        check_expr(def, &mut actx)?;
                    }
                }
                check_stmts(&i.body, &mut actx)?;
            }
            _ => {}
        }
    }
    Ok(())
}

// --- statements --------------------------------------------------------

fn check_stmts(stmts: &[Stmt], actx: &mut ACtx) -> Result<(), SemaError> {
    for s in stmts {
        check_stmt(s, actx)?;
    }
    Ok(())
}

fn check_stmt(stmt: &Stmt, actx: &mut ACtx) -> Result<(), SemaError> {
    match stmt {
        Stmt::Assign(a) => check_assign(a, actx),
        Stmt::If(i) => check_if(i, actx),
        Stmt::Match(m) => check_match(m, actx),
        Stmt::For(f) => check_for(f, actx),
        Stmt::While(w) => check_while(w, actx),
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => Ok(()),
        Stmt::Return(_, e) => {
            if let Some(e) = e {
                check_expr(e, actx)?;
            }
            Ok(())
        }
        Stmt::Assert(a) => {
            check_expr(&a.cond, actx)?;
            if let Some(m) = &a.message {
                check_expr(m, actx)?;
            }
            Ok(())
        }
        Stmt::Defer(d) => match &d.body {
            DeferBody::Expr(e) => {
                check_expr(e, actx)?;
                Ok(())
            }
            DeferBody::Suite(s) => check_stmts(s, actx),
        },
        Stmt::With(w) => {
            check_expr(&w.expr, actx)?;
            // Plans/M6.md item A: mirrors `bodies::check_with`'s own
            // local binding (`g`'s type, `Type::Named("Group", [])`) so
            // this pass's own best-effort tracker (`ACtx::locals`, never
            // scoped/popped — module doc) can answer a later `g.start`/
            // `g.join_all` at all instead of falling through to the
            // "mirroring... is not checked yet" fail-closed below.
            if let Some(name) = &w.as_name {
                actx.locals.insert(
                    name.clone(),
                    LocalInfo {
                        ty: Some(Type::Named("Group".to_string(), vec![])),
                        mode: None,
                    },
                );
            }
            check_stmts(&w.body, actx)
        }
        Stmt::Send(_, e) => {
            check_expr(e, actx)?;
            Ok(())
        }
        Stmt::Expr(_, e) => {
            check_expr(e, actx)?;
            Ok(())
        }
        Stmt::ComptimeIf(c) => {
            check_expr(&c.cond, actx)?;
            check_stmts(&c.then_branch, actx)?;
            if let Some(b) = &c.else_branch {
                check_stmts(b, actx)?;
            }
            Ok(())
        }
        Stmt::ComptimeAssert(_, e, m) => {
            check_expr(e, actx)?;
            if let Some(m) = m {
                check_expr(m, actx)?;
            }
            Ok(())
        }
    }
}

fn check_if(i: &IfStmt, actx: &mut ACtx) -> Result<(), SemaError> {
    check_expr(&i.cond, actx)?;
    check_stmts(&i.then_branch, actx)?;
    for elif in &i.elifs {
        check_expr(&elif.cond, actx)?;
        check_stmts(&elif.body, actx)?;
    }
    if let Some(b) = &i.else_branch {
        check_stmts(b, actx)?;
    }
    Ok(())
}

fn check_while(w: &WhileStmt, actx: &mut ACtx) -> Result<(), SemaError> {
    check_expr(&w.cond, actx)?;
    check_stmts(&w.body, actx)
}

fn check_match(m: &MatchStmt, actx: &mut ACtx) -> Result<(), SemaError> {
    let sty = check_expr(&m.scrutinee, actx)?;
    for arm in &m.arms {
        bind_pattern_locals(&arm.pattern, sty.as_ref(), actx);
        if let Some(g) = &arm.guard {
            check_expr(g, actx)?;
        }
        check_stmts(&arm.body, actx)?;
    }
    Ok(())
}

/// Bug 2 fix (plans/pre-M3-findings.md #2, values.resource.move-spells-take):
/// `take_binding`/the iterable's own `take` must agree — 02-language.md
/// §3.2's "Consume an array whole with `for take x in take array`" names
/// exactly one sanctioned combination; the other three (a `take` binding
/// over a non-`take` iterable, or a `take`n iterable with a plain
/// binding) are rejected here, before either side is even typed, so the
/// message always names the actual mismatch rather than surfacing as some
/// other, less legible failure downstream. A `take` binding whose
/// iterable is *not* itself `take`n would otherwise let the loop move
/// elements out through a runtime index — forbidden regardless of who
/// owns the array (02 §3.2's own reasoning: "the analysis would depend on
/// runtime history") — while a `take`n iterable with a plain binding
/// throws its consumed elements away unused, never the sanctioned shape.
fn check_for(f: &ForStmt, actx: &mut ACtx) -> Result<(), SemaError> {
    let iterable_is_take = matches!(f.iterable, Expr::Unary(_, UnaryOp::Take, _));
    if f.take_binding && !iterable_is_take {
        return Err(access_error(
            "`for take` requires the iterable itself be `take`n: write `for take x in take array` \
             — elements cannot be taken one at a time out of an array through a runtime index"
                .to_string(),
            f.span,
        ));
    }
    if !f.take_binding && iterable_is_take {
        return Err(access_error(
            "a `take`n iterable requires a `take` binding: write `for take x in take array`"
                .to_string(),
            f.span,
        ));
    }
    let raw_iterable: &Expr = match &f.iterable {
        Expr::Unary(_, UnaryOp::Take, inner) => inner.as_ref(),
        other => other,
    };
    let elem_ty = match raw_iterable {
        Expr::Range(_, from, to, _incl) => {
            let t1 = check_expr(from, actx)?;
            let t2 = check_expr(to, actx)?;
            t1.or(t2)
        }
        _ => {
            // Checked through the *whole* iterable expression (including
            // its own `take`, when present) rather than the manually
            // unwrapped inner place, so a `take`n iterable still goes
            // through this pass's own `Expr::Unary(_, Take, ...)` arm
            // (the ordinary `read`-parameter/place-expression legality
            // check every other `take` gets) instead of relying solely
            // on `flow.rs`'s own re-derivation of the same rule.
            let ty = check_expr(&f.iterable, actx)?;
            ty.and_then(|t| match bodies::unwrap_own(t) {
                Type::Array(elem, _) => Some(*elem),
                _ => None,
            })
        }
    };
    // 02-language.md §3.2: a plain (non-`take`) binding over a *resource*
    // element type is a read loan of each element, not an owned value —
    // only `for take x in take array` (agreement already enforced above)
    // licenses moving `x` further. A data element is an ordinary,
    // independent copy each iteration and stays unrestricted (`mode:
    // None`), matching every other fresh-value binding.
    let mode = if !f.take_binding {
        match &elem_ty {
            Some(t) if bodies::is_resource_type(t, actx.mctx) => Some(AccessMode::Read),
            _ => None,
        }
    } else {
        None
    };
    actx.locals
        .insert(f.name.clone(), LocalInfo { ty: elem_ty, mode });
    check_stmts(&f.body, actx)
}

/// Assignment's own read-loan check (item 4): a `read` binding (a
/// parameter, or `self` under its effective mode) can never be the
/// target of an assignment, whole or by field/index path.
fn check_assign(a: &AssignStmt, actx: &mut ACtx) -> Result<(), SemaError> {
    if let Some(root) = place_root_name(&a.target) {
        if actx.locals.get(root).and_then(|li| li.mode) == Some(AccessMode::Read) {
            return Err(access_error(
                format!("`{root}` is a `read` parameter; it cannot be assigned"),
                a.span,
            ));
        }
    }
    check_expr(&a.target, actx)?;
    let value_ty = check_expr(&a.value, actx)?;
    if let Expr::Name(_, name) = &a.target {
        if !actx.locals.contains_key(name) {
            let ty = match &a.ty {
                Some(t) => Some(actx.resolve_ann(t)),
                None => value_ty,
            };
            actx.locals
                .insert(name.clone(), LocalInfo { ty, mode: None });
        }
    }
    Ok(())
}

// --- patterns: plain owned bindings, best-effort typed --------------------

fn bind_pattern_locals(p: &Pattern, scrutinee_ty: Option<&Type>, actx: &mut ACtx) {
    match p {
        Pattern::Wildcard(_) | Pattern::Literal(..) => {}
        Pattern::Binding(_, name) => {
            actx.locals.entry(name.clone()).or_insert(LocalInfo {
                ty: scrutinee_ty.cloned(),
                mode: None,
            });
        }
        Pattern::Take(_, inner) => bind_pattern_locals(inner, scrutinee_ty, actx),
        Pattern::Variant {
            variant, payload, ..
        } => {
            let payload_tys =
                scrutinee_ty.and_then(|t| variant_payload_types(t, variant, actx.mctx));
            match payload_tys {
                Some(tys) => {
                    for (sp, ty) in payload.iter().zip(tys.iter()) {
                        bind_pattern_locals(sp, Some(ty), actx);
                    }
                }
                None => {
                    for sp in payload {
                        bind_pattern_locals(sp, None, actx);
                    }
                }
            }
        }
        Pattern::Tuple(_, items) => {
            let elem_tys = scrutinee_ty.and_then(|t| match t {
                Type::Tuple(e) => Some(e.clone()),
                _ => None,
            });
            match elem_tys {
                Some(tys) => {
                    for (sp, ty) in items.iter().zip(tys.iter()) {
                        bind_pattern_locals(sp, Some(ty), actx);
                    }
                }
                None => {
                    for sp in items {
                        bind_pattern_locals(sp, None, actx);
                    }
                }
            }
        }
        Pattern::Array(_, items) => {
            let elem_ty = scrutinee_ty.and_then(|t| match t {
                Type::Array(e, _) => Some((**e).clone()),
                _ => None,
            });
            for sp in items {
                bind_pattern_locals(sp, elem_ty.as_ref(), actx);
            }
        }
        Pattern::Or(_, alts) => {
            for sp in alts {
                bind_pattern_locals(sp, scrutinee_ty, actx);
            }
        }
    }
}

/// Mirrors `bodies.rs`'s `variant_payload_types_for`, minus the
/// diagnostics (this pass only needs the payload types when it has them
/// on hand; anything else falls back to untyped bindings).
fn variant_payload_types(scrutinee: &Type, variant: &str, mctx: &ModuleCtx) -> Option<Vec<Type>> {
    match scrutinee {
        Type::Option(inner) => Some(match variant {
            "Some" => vec![(**inner).clone()],
            _ => vec![],
        }),
        Type::Result(ok, err) => match variant {
            "Ok" => Some(vec![(**ok).clone()]),
            "Err" => Some(vec![(**err).clone()]),
            _ => None,
        },
        Type::Named(name, targs) if targs.is_empty() => {
            let e = mctx.enums.get(name)?;
            let dv = e.variants.iter().find(|v| v.name == variant)?;
            Some(match &dv.payload {
                types::DeclVariantPayload::None => vec![],
                types::DeclVariantPayload::Tuple(ts) => ts.clone(),
                types::DeclVariantPayload::Named(fs) => fs.iter().map(|(_, t)| t.clone()).collect(),
            })
        }
        _ => None,
    }
}

// --- expressions -----------------------------------------------------------

/// Checks `expr` for every access-mode concern reachable inside it and
/// returns its type when cheaply derivable (`None` otherwise — never an
/// error on its own; only a *call target* actually needing a type it
/// cannot get fails closed, in `check_call_by_field`/`check_call`).
fn check_expr(expr: &Expr, actx: &mut ACtx) -> Result<Option<Type>, SemaError> {
    match expr {
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::BStr(..)
        | Expr::Char(..)
        | Expr::FStr(_)
        | Expr::Bool(..)
        | Expr::Unit(_) => Ok(None),
        Expr::Name(_, name) => Ok(name_ty(name, actx)),
        Expr::Field(base, _span, name) => check_field(base, name, actx),
        Expr::Index(base, _span, args) => {
            let base_ty = check_expr(base, actx)?;
            for a in args {
                check_expr(a, actx)?;
            }
            Ok(base_ty.map(bodies::unwrap_own).and_then(|t| match t {
                Type::Array(elem, _) => Some(*elem),
                Type::Bytes(_) => Some(Type::U8),
                _ => None,
            }))
        }
        Expr::Call(callee, span, args) => check_call(callee, *span, args, actx),
        Expr::Unary(span, UnaryOp::Take, inner) => {
            // `take` outside a call argument (an assignment source, a
            // `return`, a tuple/array element, ...) is the same
            // read-loan question `check_arg` asks of a `take` argument
            // (02-language.md §3: "an argument, an assignment source, a
            // literal operand, or a pattern payload" are one rule, not
            // four) — checked here so every occurrence goes through it,
            // not just the call-argument shape.
            if !is_full_place(inner) {
                return Err(access_error(
                    "operand of `take` must be a place expression (name, field, index)".to_string(),
                    *span,
                ));
            }
            if let Some(root) = place_root_name(inner) {
                if actx.locals.get(root).and_then(|li| li.mode) == Some(AccessMode::Read) {
                    return Err(access_error(
                        format!("`{root}` is a `read` parameter; it cannot be taken"),
                        *span,
                    ));
                }
            }
            check_expr(inner, actx)
        }
        Expr::Unary(_, _, inner) => {
            check_expr(inner, actx)?;
            Ok(None)
        }
        Expr::Try(_, inner) => {
            let t = check_expr(inner, actx)?;
            Ok(t.and_then(|t| match t {
                Type::Result(ok, _) => Some(*ok),
                Type::Option(inner) => Some(*inner),
                _ => None,
            }))
        }
        Expr::Binary(_, _, l, r) => {
            let lt = check_expr(l, actx)?;
            let rt = check_expr(r, actx)?;
            Ok(lt.or(rt))
        }
        Expr::Range(_, a, b, _) => {
            check_expr(a, actx)?;
            check_expr(b, actx)?;
            Ok(None)
        }
        Expr::Is(_, scrutinee, pattern) => {
            let sty = check_expr(scrutinee, actx)?;
            bind_pattern_locals(pattern, sty.as_ref(), actx);
            Ok(Some(Type::Bool))
        }
        Expr::Not(_, inner) => {
            check_expr(inner, actx)?;
            Ok(Some(Type::Bool))
        }
        Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            check_expr(l, actx)?;
            check_expr(r, actx)?;
            Ok(Some(Type::Bool))
        }
        Expr::DotVariant(_, _name, args) => {
            for a in args {
                check_payload_arg(a, actx)?;
            }
            Ok(None)
        }
        Expr::Closure(c) => check_closure(c, actx),
        Expr::Send(_, inner) => {
            check_expr(inner, actx)?;
            Ok(None)
        }
        Expr::Tuple(_, items) => {
            let mut tys = Vec::with_capacity(items.len());
            for i in items {
                tys.push(check_expr(i, actx)?);
            }
            if tys.iter().all(Option::is_some) {
                Ok(Some(Type::Tuple(
                    tys.into_iter().map(|t| t.unwrap()).collect(),
                )))
            } else {
                Ok(None)
            }
        }
        Expr::List(span, items) => {
            let mut first = None;
            for i in items {
                let t = check_expr(i, actx)?;
                if first.is_none() {
                    first = t;
                }
            }
            Ok(first.map(|e| {
                Type::Array(
                    Box::new(e),
                    Box::new(Expr::Int(*span, items.len().to_string())),
                )
            }))
        }
    }
}

fn name_ty(name: &str, actx: &ACtx) -> Option<Type> {
    if let Some(li) = actx.locals.get(name) {
        return li.ty.clone();
    }
    if let Some(t) = actx.mctx.consts.get(name) {
        return Some(t.clone());
    }
    if let Some(f) = actx.mctx.fns.get(name) {
        // A generic fn's own (unsubstituted) value type is best-effort
        // only (it may carry a bare `Type::Generic` in a param/return
        // position) — fine here: `name_ty` only ever feeds mirroring
        // (arity/label/mode, type-independent) and receiver-mutability
        // (mode-only), never a type comparison.
        return Some(bodies::fn_value_type(&f.decl));
    }
    None
}

/// A bare `x.field` (or `Type.assoc_fn`/`Enum.Variant`, used as a value
/// rather than called) — not a call, so no mirroring/mutability question
/// arises here; only recursion and best-effort typing.
fn check_field(base: &Expr, name: &str, actx: &mut ACtx) -> Result<Option<Type>, SemaError> {
    if let Expr::Name(_, bname) = base {
        if !actx.locals.contains_key(bname) {
            if let Some(s) = actx.mctx.structs.get(bname.as_str()) {
                if let Some((_, d)) = s.assoc_fn(name) {
                    return Ok(Some(bodies::fn_value_type(d)));
                }
                return Ok(None);
            }
            if let Some(e) = actx.mctx.enums.get(bname.as_str()) {
                if e.variants
                    .iter()
                    .any(|v| v.name == name && matches!(v.payload, types::DeclVariantPayload::None))
                {
                    return Ok(Some(Type::Named(e.name.clone(), vec![])));
                }
                return Ok(None);
            }
        }
    }
    let base_ty = check_expr(base, actx)?;
    let Some(base_ty) = base_ty.map(bodies::unwrap_own) else {
        return Ok(None);
    };
    if let Type::Named(sname, targs) = &base_ty {
        // Bug 1 fix (access.rs's own best-effort type tracker): a field
        // reached through an *instantiated* generic struct's value
        // (non-empty `targs`) must substitute the struct's own generics
        // with `targs` in the field's declared type before handing it
        // back — otherwise a further chained method call through this
        // field (`p.items[0].get()`, item H) sees an untyped base and
        // wrongly fails closed (`check_call_by_field`'s "mirroring...
        // is not checked yet"). `subst_field_type` (generics.rs,
        // `pub(crate)`) is a no-op when `targs` is empty, so this
        // subsumes the old `targs.is_empty()`-only path.
        if let Some(s) = actx.mctx.structs.get(sname.as_str()) {
            if let Some(ty) = s.field_ty(name) {
                return Ok(Some(generics::subst_field_type(
                    &ty,
                    &s.decl.generics,
                    targs,
                )));
            }
        }
    }
    Ok(None)
}

fn check_closure(c: &ClosureExpr, actx: &mut ACtx) -> Result<Option<Type>, SemaError> {
    for p in &c.params {
        let ty = p.ty.as_ref().map(|t| actx.resolve_ann(t));
        actx.locals.insert(
            p.name.clone(),
            LocalInfo {
                ty,
                mode: Some(p.mode),
            },
        );
    }
    match &c.body {
        ClosureBody::Expr(e) => {
            check_expr(e, actx)?;
        }
        ClosureBody::Suite(s) => {
            check_stmts(s, actx)?;
        }
    }
    Ok(None)
}

// --- the per-argument universal checks (place-ness + read loans) ---------

fn check_arg(a: &Arg, actx: &mut ACtx) -> Result<(), SemaError> {
    if a.mode != AccessMode::Read {
        if !is_full_place(&a.value) {
            return Err(access_error(
                format!(
                    "operand of `{}` must be a place expression (name, field, index)",
                    a.mode.as_str()
                ),
                a.span,
            ));
        }
        if let Some(root) = place_root_name(&a.value) {
            if actx.locals.get(root).and_then(|li| li.mode) == Some(AccessMode::Read) {
                return Err(access_error(
                    format!(
                        "`{root}` is a `read` parameter; it cannot be passed as `{}`",
                        a.mode.as_str()
                    ),
                    a.span,
                ));
            }
        }
    }
    check_expr(&a.value, actx)?;
    Ok(())
}

/// A payload/literal-operand argument's own checks (a struct literal
/// field, an enum variant's payload, a builtin pseudo-constructor's
/// argument — every position plan item D calls out as "never mirrored",
/// since there is no declared parameter to mirror against at all): every
/// `check_arg` universal check still applies, plus one more that only
/// makes sense here. 02-language.md §3 spells exactly two legal shapes
/// for a value reaching one of these positions — an unmarked value
/// (data copies; a resource must already be owned, i.e. this is a fresh
/// value, not a borrowed place) or an explicit `take` ("a literal
/// operand" is one of the four `take`-legal shapes it names verbatim) —
/// `mut` is neither: there is no receiving parameter for a "loan for the
/// call's duration" to end at, and unlike a real `mut` parameter (mode-
/// mirrored, exclusivity-tracked, and always read back by the callee)
/// a `mut`-marked payload value would otherwise slip past
/// `flow.rs`'s own implicit-copy-of-a-resource check (which only ever
/// fires for the unmarked/`Read` shape) while never deinitializing its
/// source the way `take` does — a live resource would end up with two
/// simultaneously valid owners, violating the "exactly one owner" rule.
fn check_payload_arg(a: &Arg, actx: &mut ACtx) -> Result<(), SemaError> {
    if a.mode == AccessMode::Mut {
        return Err(access_error(
            "`mut` is not a legal marker here; a payload value is either unmarked or `take`"
                .to_string(),
            a.span,
        ));
    }
    check_arg(a, actx)
}

// --- call-site mirroring (item 1) ------------------------------------------

fn mirror_message(name: &str, expected: AccessMode, found: AccessMode) -> String {
    match (expected, found) {
        (AccessMode::Read, _) => format!(
            "parameter `{name}` takes an unmarked argument, found `{}`",
            found.as_str()
        ),
        (_, AccessMode::Read) => format!(
            "parameter `{name}` requires `{}`, found an unmarked argument",
            expected.as_str()
        ),
        _ => format!(
            "parameter `{name}` requires `{}`, found `{}`",
            expected.as_str(),
            found.as_str()
        ),
    }
}

fn mirror_message_positional(expected: AccessMode, found: AccessMode) -> String {
    match (expected, found) {
        (AccessMode::Read, _) => {
            format!("this argument takes no marker, found `{}`", found.as_str())
        }
        (_, AccessMode::Read) => format!(
            "this argument requires `{}`, found an unmarked argument",
            expected.as_str()
        ),
        _ => format!(
            "this argument requires `{}`, found `{}`",
            expected.as_str(),
            found.as_str()
        ),
    }
}

/// Binds `args` to `decl_params` exactly like `bodies::check_call_args`
/// did (label or positional cursor) and compares each bound argument's
/// marker against its parameter's declared mode. Binding itself is
/// assumed valid — arity/labels were already proven by `bodies::check`.
fn check_mirroring_named(decl_params: &[DeclParam], args: &[Arg]) -> Result<(), SemaError> {
    let mut bound = vec![false; decl_params.len()];
    let mut cursor = 0usize;
    for a in args {
        let idx = match &a.label {
            Some(lbl) => match decl_params.iter().position(|p| &p.name == lbl) {
                Some(i) => i,
                None => continue, // unreachable on an already-checked call; defensive only.
            },
            None => {
                while cursor < bound.len() && bound[cursor] {
                    cursor += 1;
                }
                if cursor >= decl_params.len() {
                    continue; // ditto.
                }
                let i = cursor;
                cursor += 1;
                i
            }
        };
        bound[idx] = true;
        let expected = decl_params[idx].mode;
        if a.mode != expected {
            return Err(access_error(
                mirror_message(&decl_params[idx].name, expected, a.mode),
                a.span,
            ));
        }
    }
    Ok(())
}

fn check_mirroring_positional(param_modes: &[AccessMode], args: &[Arg]) -> Result<(), SemaError> {
    for (a, expected) in args.iter().zip(param_modes.iter()) {
        if a.mode != *expected {
            return Err(access_error(
                mirror_message_positional(*expected, a.mode),
                a.span,
            ));
        }
    }
    Ok(())
}

// --- receiver mutability (item 3's "calling through a receiver" rule) ----

fn place_root_mode(expr: &Expr, actx: &ACtx) -> Option<AccessMode> {
    match expr {
        Expr::Name(_, n) => actx.locals.get(n).and_then(|li| li.mode),
        Expr::Field(base, _, _) => place_root_mode(base, actx),
        Expr::Index(base, _, _) => place_root_mode(base, actx),
        _ => None,
    }
}

fn allows_mut_receiver(mode: Option<AccessMode>) -> bool {
    !matches!(mode, Some(AccessMode::Read))
}

fn allows_take_receiver(mode: Option<AccessMode>) -> bool {
    !matches!(mode, Some(AccessMode::Read) | Some(AccessMode::Mut))
}

fn receiver_mutability_error(
    required: AccessMode,
    root_mode: Option<AccessMode>,
    root_name: Option<&str>,
    span: Span,
) -> SemaError {
    let noun = match required {
        AccessMode::Mut => "a mutable place",
        AccessMode::Take => "an owned place",
        AccessMode::Read => "any place",
    };
    let found = match (root_mode, root_name) {
        (Some(mode), Some(n)) => format!("`{}` parameter `{n}`", mode.as_str()),
        _ => "this expression".to_string(),
    };
    access_error(
        format!(
            "calling a `{} self` method requires {noun}, found {found}",
            required.as_str()
        ),
        span,
    )
}

// --- calls: fn/method/associated-fn/init/struct-literal/enum-variant ----

/// Mirrors `bodies.rs`'s `check_call` dispatch (`Expr::Index` for a
/// scalar conversion or generic bracket, `Expr::Name`, `Expr::Field`,
/// then a raw callable expression) but resolves a *mirroring target*
/// instead of a type: every `Arg` reachable from `args` always gets the
/// universal per-argument checks (`check_arg`) regardless of whether the
/// callee resolves further.
fn check_call(
    callee: &Expr,
    span: Span,
    args: &[Arg],
    actx: &mut ACtx,
) -> Result<Option<Type>, SemaError> {
    match callee {
        Expr::Index(inner, _ispan, targs) => {
            // `.to[T]()`/`.checked_to[T]()`/`.truncate_to[T]()` (no
            // parameters to mirror) or `Name[Args](...)` — a generic
            // struct construction or fn call (item H). Mirroring
            // (arity/label/mode) is structural, identical across every
            // instantiation of the same declaration, so this uses the
            // *declared* (unsubstituted) params directly rather than
            // resolving which concrete instantiation `bodies::check`
            // enqueued for this exact call — same simplification as
            // `check_call_by_name`/`check_call_by_field` below. The
            // *result* type still needs the resolved `targs` (Bug 1(b)'s
            // fix, mirrored from `bodies.rs`): a bare, un-instantiated
            // `Type::Named` here would make this pass's own best-effort
            // tracker just as blind to a chained field/method access as
            // `bodies.rs`'s construction path was.
            if let Expr::Name(_, name) = inner.as_ref() {
                if !actx.locals.contains_key(name) {
                    if let Some(f) = actx.mctx.fns.get(name) {
                        if !f.decl.generics.is_empty() {
                            for a in args {
                                check_arg(a, actx)?;
                            }
                            check_mirroring_named(&f.decl.params, args)?;
                            return Ok(Some(f.decl.ret.clone()));
                        }
                    } else if let Some(s) = actx.mctx.structs.get(name) {
                        if !s.decl.generics.is_empty() {
                            let type_args = generics::resolve_call_targs(targs, actx.mctx)?;
                            return check_struct_construction(s, &type_args, args, actx);
                        }
                    }
                }
            }
            // A scalar conversion (`.to[T]()`, ...): no parameters to
            // mirror, just recurse.
            check_expr(inner, actx)?;
            for a in args {
                check_arg(a, actx)?;
            }
            Ok(None)
        }
        Expr::Name(_, name) => check_call_by_name(name, span, args, actx),
        Expr::Field(base, fspan, name) => check_call_by_field(base, *fspan, name, span, args, actx),
        other => {
            let ty = check_expr(other, actx)?;
            for a in args {
                check_arg(a, actx)?;
            }
            match ty {
                Some(Type::Fn(params, ret)) => {
                    let modes: Vec<AccessMode> = params.iter().map(|(m, _)| *m).collect();
                    check_mirroring_positional(&modes, args)?;
                    Ok(Some(*ret))
                }
                _ => Err(unimplemented_at(
                    "mirroring a call through this expression is",
                    span,
                )),
            }
        }
    }
}

fn check_call_by_name(
    name: &str,
    call_span: Span,
    args: &[Arg],
    actx: &mut ACtx,
) -> Result<Option<Type>, SemaError> {
    if let Some(li) = actx.locals.get(name).cloned() {
        for a in args {
            check_arg(a, actx)?;
        }
        return match li.ty {
            Some(Type::Fn(params, ret)) => {
                let modes: Vec<AccessMode> = params.iter().map(|(m, _)| *m).collect();
                check_mirroring_positional(&modes, args)?;
                Ok(Some(*ret))
            }
            _ => Err(unimplemented_at(
                "mirroring a call through this local's type is",
                call_span,
            )),
        };
    }
    if let Some(t) = actx.mctx.consts.get(name).cloned() {
        for a in args {
            check_arg(a, actx)?;
        }
        return match t {
            Type::Fn(params, ret) => {
                let modes: Vec<AccessMode> = params.iter().map(|(m, _)| *m).collect();
                check_mirroring_positional(&modes, args)?;
                Ok(Some(*ret))
            }
            _ => Ok(None), // calling a non-function const: unreachable (bodies.rs validated).
        };
    }
    if let Some(f) = actx.mctx.fns.get(name) {
        // A generic fn called without explicit `[Args]` (item H's
        // inference, decision 2/item 2): mirroring only needs the
        // *declared* params (arity/label/mode, type-independent), so
        // this proceeds identically whether `f` is generic or not —
        // `bodies::check` (which runs first) is what actually resolves
        // and enqueues the concrete instantiation.
        for a in args {
            check_arg(a, actx)?;
        }
        check_mirroring_named(&f.decl.params, args)?;
        return Ok(Some(f.decl.ret.clone()));
    }
    if let Some(s) = actx.mctx.structs.get(name) {
        return check_struct_construction(s, &[], args, actx);
    }
    if name == "Image" {
        // plans/M4.md item B, decision 5: unlike `Some`/`Ok`/`Err`/
        // `RestartIntensity`/`seconds` below (whose own results this
        // pass never needs to track further — nothing calls a method on
        // any of them), `img`'s own type *does* matter: every later
        // `img.driver(...)`/`img.actor(...)`/... in the same `@image`
        // body is a real method call this pass's own best-effort type
        // tracker must not lose (`check_call_by_field`'s own new
        // dispatch, below, keys off exactly this type).
        for a in args {
            check_payload_arg(a, actx)?;
        }
        return Ok(Some(Type::Named("Image".to_string(), vec![])));
    }
    // `Some`/`Ok`/`Err`/`panic`/`RestartIntensity`/`seconds`: builtin
    // pseudo-constructors with no declared parameter modes (they are not
    // real `DeclParam`s).
    for a in args {
        check_payload_arg(a, actx)?;
    }
    Ok(None)
}

fn check_struct_construction(
    s: &StructInfo,
    targs: &[types::TypeArg],
    args: &[Arg],
    actx: &mut ACtx,
) -> Result<Option<Type>, SemaError> {
    // Bug 1(b) fix, mirrored from `bodies.rs`'s own `check_struct_construction`:
    // carry the instantiation's own args into the synthesized type so
    // this pass's best-effort tracker can keep answering field/method
    // questions through it (`check_field`'s `subst_field_type` above).
    let self_ty = Type::Named(s.decl.name.clone(), targs.to_vec());
    if let Some((_ia, id)) = s.init() {
        for a in args {
            check_arg(a, actx)?;
        }
        check_mirroring_named(&id.params, args)?;
        return Ok(Some(match &id.ret {
            Type::Unit => self_ty,
            Type::Result(ok, err) if **ok == Type::Unit => {
                Type::Result(Box::new(self_ty), err.clone())
            }
            _ => self_ty, // a non-standard init return already fails closed in bodies.rs.
        }));
    }
    // A struct literal's fields have no declared parameter mode to
    // mirror against (`DeclField` carries no `mode`, unlike `DeclParam`)
    // — only the universal per-argument (plus payload-only) checks apply.
    for a in args {
        check_payload_arg(a, actx)?;
    }
    Ok(Some(self_ty))
}

/// The result type of an already-`bodies`-accepted MMIO register access:
/// `read()` yields the register's declared scalar, `write(v)` yields
/// `unit`. Best-effort like the rest of this pass — an unknown layout or
/// register simply stops the chain (`None`) rather than inventing a type,
/// and cannot be reached at all through a program `bodies` accepted.
fn mmio_access_result(
    targs: &[types::TypeArg],
    register: &str,
    op: &str,
    actx: &ACtx,
) -> Option<Type> {
    if op == "write" {
        return Some(Type::Unit);
    }
    let types::TypeArg::Type(Type::Named(layout_name, _)) = targs.first()? else {
        return None;
    };
    let layout = actx.mctx.layouts.get(layout_name.as_str())?;
    let reg = types::mmio_register(layout, register)?;
    bodies::scalar_type_by_name(&reg.scalar)
}

fn check_call_by_field(
    base: &Expr,
    fspan: Span,
    name: &str,
    _call_span: Span,
    args: &[Arg],
    actx: &mut ACtx,
) -> Result<Option<Type>, SemaError> {
    if let Expr::Name(_, bname) = base {
        if !actx.locals.contains_key(bname) {
            if let Some(s) = actx.mctx.structs.get(bname.as_str()) {
                if let Some((_af, d)) = s.assoc_fn(name) {
                    for a in args {
                        check_arg(a, actx)?;
                    }
                    check_mirroring_named(&d.params, args)?;
                    return Ok(Some(d.ret.clone()));
                }
                for a in args {
                    check_arg(a, actx)?;
                }
                return Ok(None); // unreachable on an already-checked call.
            }
            if let Some(e) = actx.mctx.enums.get(bname.as_str()) {
                for a in args {
                    check_payload_arg(a, actx)?;
                }
                if e.variants.iter().any(|v| v.name == name) {
                    return Ok(Some(Type::Named(e.name.clone(), vec![])));
                }
                return Ok(None);
            }
        }
    }
    // plans/M7.md item C (03-hardware.md §2): a typed MMIO register access
    // — `<mmio>.<register>.read()` / `.write(v)`. Mirrors
    // `bodies::check_call_by_field`'s own interception exactly: the
    // receiver is a *register selection*, which has no value form, so the
    // general path below would (correctly, for anything else) fail closed
    // on an untyped base. `bodies` has already accepted this call by the
    // time this pass runs, so the only work left here is the universal
    // per-argument checks and handing back the result type for chaining.
    //
    // **Receiver mutability is deliberately not required.** `write(v)`
    // changes the *device*, not the wrela value: an `Mmio[L]` is authority,
    // and 03 §6's own ISR (`fn on_queue_irq(self)`, which both reads
    // `interrupt_status` and writes `interrupt_ack`) declares a plain
    // `self`. Requiring `mut self` would make the one worked example in
    // the docs illegal.
    if let Expr::Field(inner, _, register) = base {
        let saved = actx.locals.clone();
        let inner_ty = check_expr(inner, actx)
            .ok()
            .flatten()
            .map(bodies::unwrap_own);
        match inner_ty {
            Some(Type::Named(ref cap, ref targs)) if cap == "Mmio" => {
                for a in args {
                    check_arg(a, actx)?;
                }
                return Ok(mmio_access_result(targs, register, name, actx));
            }
            // Anything else: restore and fall through to the ordinary
            // path, which re-checks `base` whole and reports whatever it
            // would have reported.
            _ => actx.locals = saved,
        }
    }
    // General field/method access: `base`'s own type decides. `base` is
    // checked (recursed into) before `args`, matching source order.
    let base_ty = check_expr(base, actx)?;
    let root_mode = place_root_mode(base, actx);
    let root_name = place_root_name(base).map(str::to_string);
    for a in args {
        check_arg(a, actx)?;
    }
    // plans/M4.md item B, decision 5: the builder surface's own two
    // opaque types (`sema::bodies`'s own `image_type`/`image_decl_type`)
    // are never real declared structs, so the ordinary struct-method
    // lookup below would (correctly, for every *other* unknown type)
    // fail closed on them — recognized here instead, mirroring
    // `sema::bodies::check_call_by_field`'s own dispatch exactly, so
    // this pass's best-effort tracker reports the right result type
    // (`img.driver`/`img.actor`/`decl.handle()` all get chained further,
    // e.g. `disk.handle()`) instead of losing it.
    if base_ty.as_ref() == Some(&Type::Named("Image".to_string(), vec![])) {
        let result = match name {
            "driver" | "actor" => Type::Named("ImageDecl".to_string(), vec![]),
            "seal" => Type::Named("Image".to_string(), vec![]),
            _ => Type::Unit, // `supervise`/`check_layout`, or an already-rejected name.
        };
        return Ok(Some(result));
    }
    if base_ty.as_ref() == Some(&Type::Named("ImageDecl".to_string(), vec![])) {
        return Ok(Some(Type::Named("ImageDecl".to_string(), vec![])));
    }
    let Some(base_ty) = base_ty.map(bodies::unwrap_own) else {
        return Err(unimplemented_at(
            "mirroring a method call through this base expression is",
            fspan,
        ));
    };
    // Plans/M6.md item A: `Actor[T]`/`Group` are both opaque builtin
    // types, never real declared structs (mirrors the `Image`/`ImageDecl`
    // dispatch just above) — best-effort only: `Group`'s own methods
    // (`start`/`join_all`) have no declared param list to mirror against
    // at all, so this returns `None` (no further chaining) rather than
    // erroring; an `Actor[T]` call resolves its real method (arguments
    // were already mirrored above, uniformly, before this dispatch) for
    // a decent best-effort result type.
    if let Type::Named(n, targs) = &base_ty {
        if n == "Actor" {
            if let Some(types::TypeArg::Type(Type::Named(actor_name, _))) = targs.first() {
                if let Some(s) = actx.mctx.structs.get(actor_name.as_str()) {
                    if let Some((_mf, d)) = s.method(name) {
                        return Ok(Some(d.ret.clone()));
                    }
                }
            }
            return Ok(None);
        }
        if n == "Group" {
            return Ok(None);
        }
    }
    let Type::Named(sname, _targs) = &base_ty else {
        return Err(unimplemented_at(
            "mirroring a method call through this base expression is",
            fspan,
        ));
    };
    // `sname` alone decides the target struct regardless of `_targs`
    // (item H): mirroring/receiver-mutability are structural, unaffected
    // by which concrete instantiation this value came from.
    let Some(s) = actx.mctx.structs.get(sname.as_str()) else {
        return Err(unimplemented_at(
            "mirroring a method call through this base expression is",
            fspan,
        ));
    };
    let Some((_mf, d)) = s.method(name) else {
        return Ok(None); // unreachable on an already-checked call.
    };
    // A generic method's own `[...]` (beyond the struct's, if any) is
    // item H's documented boundary — `bodies::check` (which runs first)
    // fails closed on such a call before this pass ever sees it, so
    // mirroring here proceeds unconditionally: it is either a concrete
    // method or unreachable.
    check_mirroring_named(&d.params, args)?;
    if let Some(r) = &d.receiver {
        let effective = effective_declared(r.mode, r.is_pub, sname, name, actx.effects);
        if effective != AccessMode::Read {
            let ok = match effective {
                AccessMode::Mut => allows_mut_receiver(root_mode),
                AccessMode::Take => allows_take_receiver(root_mode),
                AccessMode::Read => true,
            };
            if !ok {
                return Err(receiver_mutability_error(
                    effective,
                    root_mode,
                    root_name.as_deref(),
                    fspan,
                ));
            }
        }
    }
    Ok(Some(d.ret.clone()))
}
