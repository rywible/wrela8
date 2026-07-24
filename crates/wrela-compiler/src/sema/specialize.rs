//! `comptime if` specialization (plans/M3.md item D, decision 8): "the
//! unselected branch is dropped before body checking, so the graph that
//! is checked is the graph that exists" (02-language.md §12). This pass
//! runs first in `sema::check_typed`'s pipeline — before `symbols::collect`
//! even sees the module — and returns a new `Module` with every
//! `Item::ComptimeIf`/`Member::ComptimeIf`/`Stmt::ComptimeIf` node
//! replaced by its selected branch's own items/members/statements,
//! spliced in directly at the same position. Every later pass
//! (`collect`/`resolve`/`declare`/`bodies`/`access`/`flow`/`matches`/
//! `generics`) therefore only ever sees the specialized graph — module-
//! scope and member-scope forms land exactly like a plain declaration
//! always did (nothing downstream needed to change), and statement-scope
//! specialization happens before `bodies::check` ever types the
//! surrounding function, so its own long-standing `error[unimplemented]:
//! \`comptime if\` is not checked yet` (bodies.rs, decision 7) simply
//! never fires for a real program again — kept only as an unreachable
//! defense-in-depth net (see bodies.rs/flow.rs/matches.rs's own comments)
//! in case this pass ever leaves one behind.
//!
//! ## The normative-ordering reading this pass pins (my own, stated
//! explicitly per the M3-D task brief: "where the doc leaves latitude,
//! pick the dumbest workable reading")
//!
//! 02-language.md §12 says a `comptime if` condition "must be comptime-
//! evaluable with the real evaluator" but does not spell out *when*,
//! relative to the rest of the pipeline, that evaluation can happen —
//! and the obvious answer ("whenever its referenced consts are typed and
//! evaluated") is circular for module/member-scope specialization: those
//! branches themselves may contain the very declarations (including
//! `const`s) later specialization needs, and a plain `const`'s own value
//! is not evaluated by the real evaluator until `eval::check_consts`,
//! which runs at the very end of `check_typed` — long after `bodies`
//! would need to have already specialized away every `comptime if` to
//! know what to type in the first place.
//!
//! This pass resolves the circularity with one dedicated, deliberately
//! narrow rule instead of a general const-propagation/data-flow analysis:
//!
//! - A `comptime if` condition (module scope, member scope, or statement
//!   scope — identical rule everywhere, one shared vocabulary) may
//!   reference **only literals and plain top-level `const` items
//!   declared directly in the module** (i.e. `Item::Const` appearing in
//!   `module.items` itself, never nested inside *any* `comptime if`
//!   branch, selected or not) — combined with unary/binary/logical
//!   operators. No fn calls, no locals, no `self`, no generic
//!   type/const parameters (a struct's own generic const parameter,
//!   e.g. the worked example's `BlkDriver[const MODE: DriverMode]`'s
//!   `comptime if MODE == DriverMode.Irq:`, needs *per-instantiation*
//!   specialization — a materially harder feature this pass does not
//!   attempt, the same documented boundary decision 10 already draws
//!   around "generic methods' own type parameters").
//! - This is checked and evaluated by building one small, self-contained
//!   "const skeleton": a throwaway `Module` containing only the specific
//!   top-level consts transitively reachable from *some* `comptime if`
//!   condition somewhere in the whole module (computed once, up front,
//!   by a plain name-harvesting walk — no typing needed for that part),
//!   run through the ordinary `collect -> resolve -> declare -> bodies`
//!   pipeline exactly like any other module. Every condition in the
//!   whole file is then type-checked (`bodies::check_expr`, expecting
//!   `bool` — reusing the real per-expression typing machinery, not a
//!   hand-rolled duplicate) and evaluated (`eval::interp::eval_standalone`,
//!   the real evaluator) against that one skeleton, in a single left-to-
//!   right walk that expands `comptime if` nodes as it encounters them.
//! - A condition referencing anything outside that vocabulary — a const
//!   declared inside another (still-unresolved) `comptime if` branch, a
//!   local, a call, a generic parameter — fails closed with a named
//!   `error[unimplemented]`/`error[comptime]` diagnostic (see
//!   `check_comptime_vocabulary` below) rather than attempting a wrong
//!   answer. `err-comptime-if-not-comptime` pins the local-reference
//!   case (the most "honestly producible" one — see the M3-D task
//!   brief); a const hidden inside another branch fails identically,
//!   unpinned by its own golden only because it is the same diagnostic
//!   shape.
//!
//! ## What this pass deliberately does not specialize
//!
//! A `comptime if` nested inside a closure literal *embedded in an
//! expression* (a const initializer, a call argument, ...) is not
//! rewritten — only the statement lists of a `fn`/`init`/method's own
//! body (and, recursively, every ordinary nested block: `if`/`match`/
//! `for`/`while`/`defer`/`with`) are walked. Reaching one there still
//! fails closed, honestly, via bodies.rs's own residual
//! `error[unimplemented]` — a narrow, disclosed boundary (mirrors
//! `eval::legal`'s own "Known scope boundary: field defaults" precedent),
//! not a silent gap.
//!
//! Legality (plans/M3.md item C): since this vocabulary has no fn calls
//! at all, a `comptime if` condition can never reach a callee — there is
//! nothing for `eval::legal::require_legal` to gate here, so this pass
//! does not call it (item D's legality wiring lands instead on `const`
//! initializers and `comptime assert`, both of which do support calls —
//! see `eval/mod.rs`).

use std::collections::BTreeSet;

use crate::eval::{interp, to_sema_error, value::Value};
use crate::sema::bodies::{self, FnCtx, ModuleCtx};
use crate::sema::typed::TypedProgram;
use crate::sema::types::{self, Type};
use crate::sema::{SemaError, symbols, unimplemented_at};
use crate::syntax::ast::{
    ClosureBody, ConstItem, DeferBody, DeferStmt, ElifClause, Expr, FnItem, ForStmt, IfStmt,
    InitItem, Item, MatchArm, MatchStmt, Member, Module, Stmt, StructItem, WhileStmt, WithStmt,
};

/// The one small typed context every `comptime if` condition in the
/// whole module is checked/evaluated against (module doc above).
struct ConstSkeleton {
    mctx: ModuleCtx,
    program: TypedProgram,
}

/// Entry point: `sema::check_typed`'s very first step.
pub fn specialize(module: &Module) -> Result<Module, SemaError> {
    let known_consts = compute_known_consts(module);
    let skeleton = build_const_skeleton(module, &known_consts)?;
    let items = specialize_items(&module.items, &known_consts, &skeleton)?;
    Ok(Module {
        items,
        ..module.clone()
    })
}

// --- step 1: which top-level consts are even in play ----------------------

/// Every `Item::Const` declared directly in `module.items` (never nested
/// inside a `comptime if` branch) — the only consts this pass may ever
/// consult, name -> its own initializer expression.
fn top_level_consts(module: &Module) -> Vec<&ConstItem> {
    module
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Const(c) => Some(c),
            _ => None,
        })
        .collect()
}

/// The transitive closure of top-level const names any `comptime if`
/// condition anywhere in the module (module/member/statement scope,
/// selected or not — harvesting both branches is a conservative
/// superset, never a correctness problem) could possibly reference,
/// starting from every name literally written in some condition and
/// growing through each such const's own initializer. A name that turns
/// out not to be a top-level const at all is simply never added here —
/// `check_comptime_vocabulary` (below) is what actually rejects a
/// condition that reaches for one at evaluation time.
fn compute_known_consts(module: &Module) -> BTreeSet<String> {
    let mut condition_exprs = Vec::new();
    harvest_conditions_items(&module.items, &mut condition_exprs);

    let mut referenced = BTreeSet::new();
    for e in condition_exprs {
        collect_names_in_expr(e, &mut referenced);
    }

    let top_consts = top_level_consts(module);
    let mut known = BTreeSet::new();
    let mut worklist: Vec<String> = referenced.into_iter().collect();
    while let Some(name) = worklist.pop() {
        if known.contains(&name) {
            continue;
        }
        let Some(c) = top_consts.iter().find(|c| c.name == name) else {
            continue;
        };
        known.insert(name);
        let mut deps = BTreeSet::new();
        collect_names_in_expr(&c.value, &mut deps);
        for d in deps {
            if !known.contains(&d) {
                worklist.push(d);
            }
        }
    }
    known
}

// --- harvesting: find every `comptime if` condition anywhere ---------------

fn harvest_conditions_items<'a>(items: &'a [Item], out: &mut Vec<&'a Expr>) {
    for item in items {
        match item {
            Item::ComptimeIf(c) => {
                out.push(&c.cond);
                harvest_conditions_items(&c.then_branch, out);
                if let Some(b) = &c.else_branch {
                    harvest_conditions_items(b, out);
                }
            }
            Item::Fn(f) => {
                if let Some(body) = &f.body {
                    harvest_conditions_stmts(body, out);
                }
            }
            Item::Struct(s) => harvest_conditions_members(&s.members, out),
            Item::Enum(_) | Item::Pool(_) | Item::Const(_) => {}
        }
    }
}

fn harvest_conditions_members<'a>(members: &'a [Member], out: &mut Vec<&'a Expr>) {
    for m in members {
        match m {
            Member::ComptimeIf(c) => {
                out.push(&c.cond);
                harvest_conditions_members(&c.then_branch, out);
                if let Some(b) = &c.else_branch {
                    harvest_conditions_members(b, out);
                }
            }
            Member::Fn(f) => {
                if let Some(body) = &f.body {
                    harvest_conditions_stmts(body, out);
                }
            }
            Member::Init(i) => harvest_conditions_stmts(&i.body, out),
            Member::Field(_) | Member::Pool(_) => {}
        }
    }
}

fn harvest_conditions_stmts<'a>(stmts: &'a [Stmt], out: &mut Vec<&'a Expr>) {
    for s in stmts {
        harvest_conditions_stmt(s, out);
    }
}

fn harvest_conditions_stmt<'a>(stmt: &'a Stmt, out: &mut Vec<&'a Expr>) {
    match stmt {
        Stmt::ComptimeIf(c) => {
            out.push(&c.cond);
            harvest_conditions_stmts(&c.then_branch, out);
            if let Some(b) = &c.else_branch {
                harvest_conditions_stmts(b, out);
            }
        }
        Stmt::If(i) => {
            harvest_conditions_stmts(&i.then_branch, out);
            for elif in &i.elifs {
                harvest_conditions_stmts(&elif.body, out);
            }
            if let Some(b) = &i.else_branch {
                harvest_conditions_stmts(b, out);
            }
        }
        Stmt::Match(m) => {
            for arm in &m.arms {
                harvest_conditions_stmts(&arm.body, out);
            }
        }
        Stmt::For(f) => harvest_conditions_stmts(&f.body, out),
        Stmt::While(w) => harvest_conditions_stmts(&w.body, out),
        Stmt::Defer(d) => {
            if let DeferBody::Suite(s) = &d.body {
                harvest_conditions_stmts(s, out);
            }
        }
        Stmt::With(w) => harvest_conditions_stmts(&w.body, out),
        Stmt::Assign(_)
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::Return(..)
        | Stmt::Pass(_)
        | Stmt::Assert(_)
        | Stmt::Send(..)
        | Stmt::Expr(..)
        | Stmt::ComptimeAssert(..) => {}
    }
}

/// Every bare name an expression reads, recursively — a generous
/// superset (it does not distinguish "read as a value" from "used as a
/// callee", and does not descend into a closure's own statement-suite
/// body — module doc's "what this pass deliberately does not
/// specialize"), used only to seed/grow `known_consts` above; being too
/// generous here only ever pulls in a few extra, harmlessly-unused
/// consts, never a correctness problem.
fn collect_names_in_expr(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Name(_, name) => {
            out.insert(name.clone());
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::BStr(..)
        | Expr::Char(..)
        | Expr::Bool(..)
        | Expr::Unit(..)
        | Expr::FStr(_) => {}
        Expr::Field(base, _, _) => collect_names_in_expr(base, out),
        Expr::Index(base, _, args) => {
            collect_names_in_expr(base, out);
            for a in args {
                collect_names_in_expr(a, out);
            }
        }
        Expr::Call(callee, _, args) => {
            collect_names_in_expr(callee, out);
            for a in args {
                collect_names_in_expr(&a.value, out);
            }
        }
        Expr::Unary(_, _, inner) | Expr::Try(_, inner) | Expr::Not(_, inner) => {
            collect_names_in_expr(inner, out)
        }
        Expr::Binary(_, _, l, r) | Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            collect_names_in_expr(l, out);
            collect_names_in_expr(r, out);
        }
        Expr::Range(_, a, b, _) => {
            collect_names_in_expr(a, out);
            collect_names_in_expr(b, out);
        }
        Expr::Is(_, scrutinee, _pattern) => collect_names_in_expr(scrutinee, out),
        Expr::DotVariant(_, _, args) => {
            for a in args {
                collect_names_in_expr(&a.value, out);
            }
        }
        Expr::Closure(c) => {
            if let ClosureBody::Expr(e) = &c.body {
                collect_names_in_expr(e, out);
            }
        }
        Expr::Send(_, inner) => collect_names_in_expr(inner, out),
        Expr::Tuple(_, items) | Expr::List(_, items) => {
            for i in items {
                collect_names_in_expr(i, out);
            }
        }
    }
}

// --- step 2: the const skeleton every condition checks/evaluates against --

fn build_const_skeleton(
    module: &Module,
    known_consts: &BTreeSet<String>,
) -> Result<ConstSkeleton, SemaError> {
    let items: Vec<Item> = module
        .items
        .iter()
        .filter(|i| matches!(i, Item::Const(c) if known_consts.contains(&c.name)))
        .cloned()
        .collect();
    // No imports: this skeleton exists only to type/evaluate the const
    // vocabulary a `comptime if` condition may use, which never includes
    // an imported name (imports fail closed everywhere else in sema
    // today regardless — `symbols::resolve`); carrying them here would
    // only risk masking the real "imports are not checked yet" error
    // behind this pass's own, unrelated, earlier call.
    let skeleton_module = Module {
        span: module.span,
        path: module.path.clone(),
        doc: None,
        imports: Vec::new(),
        items,
    };
    let symtab = symbols::collect(&skeleton_module)?;
    symbols::resolve(
        &skeleton_module,
        &symtab,
        &crate::sema::imports::ImportBindings::new(),
    )?;
    let decl_items = types::declare(&skeleton_module)?;
    let mctx = bodies::build_module_ctx(&skeleton_module, &decl_items);
    let program = bodies::check(&skeleton_module, &decl_items, &mctx)?;
    Ok(ConstSkeleton { mctx, program })
}

// --- step 3: vocabulary-gated condition evaluation -------------------------

/// Rejects a condition expression outright the moment it uses anything
/// outside this pass's own restricted vocabulary (module doc): a `Name`
/// must be a `known_consts` member (a local, a generic parameter, or a
/// const still hidden inside another `comptime if` branch all land here
/// — `err-comptime-if-not-comptime`'s own case); anything else not in
/// the small supported-operator set fails closed by construction/name
/// instead of a wrong answer.
fn check_comptime_vocabulary(e: &Expr, known_consts: &BTreeSet<String>) -> Result<(), SemaError> {
    match e {
        Expr::Int(..) | Expr::Bool(..) | Expr::Char(..) | Expr::Unit(..) => Ok(()),
        Expr::Name(span, name) => {
            if known_consts.contains(name) {
                Ok(())
            } else {
                Err(SemaError::at(
                    "comptime",
                    format!(
                        "comptime if condition references `{name}`, which is not comptime-\
                         visible here (only literals and top-level consts are — a local, a \
                         generic parameter, or a const declared inside another `comptime if` \
                         branch cannot be)"
                    ),
                    *span,
                ))
            }
        }
        Expr::Unary(_, _, inner) | Expr::Not(_, inner) => {
            check_comptime_vocabulary(inner, known_consts)
        }
        Expr::Binary(_, _, l, r) | Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            check_comptime_vocabulary(l, known_consts)?;
            check_comptime_vocabulary(r, known_consts)
        }
        other => Err(unimplemented_at(
            "this expression form in a `comptime if` condition is",
            other.span(),
        )),
    }
}

/// Type-checks (`bodies::check_expr`, expecting `bool` — `err-comptime-
/// if-not-bool`'s own case, the ordinary expected-type mismatch
/// diagnostic) and evaluates (`eval::interp::eval_standalone`, the real
/// evaluator) one `comptime if` condition against the whole module's one
/// shared const skeleton.
fn eval_condition(
    cond: &Expr,
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
) -> Result<bool, SemaError> {
    check_comptime_vocabulary(cond, known_consts)?;
    let mut fctx = FnCtx::new(Type::Unit, skeleton.mctx.module_pools.clone());
    let typed_cond = bodies::check_expr(cond, Some(&Type::Bool), &mut fctx, &skeleton.mctx)?;
    let value = interp::eval_standalone(&skeleton.program, &typed_cond, "comptime if".to_string())
        .map_err(to_sema_error)?;
    match value {
        Value::Bool(b) => Ok(b),
        other => Err(SemaError::at(
            "comptime",
            format!(
                "internal error: comptime if condition evaluated to a non-bool value ({other:?})"
            ),
            cond.span(),
        )),
    }
}

// --- step 4: the expansion walk --------------------------------------------

fn specialize_items(
    items: &[Item],
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
) -> Result<Vec<Item>, SemaError> {
    let mut out = Vec::new();
    for item in items {
        specialize_item(item, known_consts, skeleton, &mut out)?;
    }
    Ok(out)
}

fn specialize_item(
    item: &Item,
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
    out: &mut Vec<Item>,
) -> Result<(), SemaError> {
    match item {
        Item::ComptimeIf(c) => {
            let selected: &[Item] = if eval_condition(&c.cond, known_consts, skeleton)? {
                &c.then_branch
            } else {
                match &c.else_branch {
                    Some(b) => b,
                    None => return Ok(()),
                }
            };
            out.extend(specialize_items(selected, known_consts, skeleton)?);
            Ok(())
        }
        Item::Const(_) | Item::Enum(_) | Item::Pool(_) => {
            out.push(item.clone());
            Ok(())
        }
        Item::Fn(f) => {
            out.push(Item::Fn(specialize_fn(f, known_consts, skeleton)?));
            Ok(())
        }
        Item::Struct(s) => {
            let members = specialize_members(&s.members, known_consts, skeleton)?;
            out.push(Item::Struct(StructItem {
                members,
                ..s.clone()
            }));
            Ok(())
        }
    }
}

fn specialize_fn(
    f: &FnItem,
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
) -> Result<FnItem, SemaError> {
    let body = match &f.body {
        Some(b) => Some(specialize_stmts(b, known_consts, skeleton)?),
        None => None,
    };
    Ok(FnItem { body, ..f.clone() })
}

fn specialize_init(
    i: &InitItem,
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
) -> Result<InitItem, SemaError> {
    Ok(InitItem {
        body: specialize_stmts(&i.body, known_consts, skeleton)?,
        ..i.clone()
    })
}

fn specialize_members(
    members: &[Member],
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
) -> Result<Vec<Member>, SemaError> {
    let mut out = Vec::new();
    for m in members {
        specialize_member(m, known_consts, skeleton, &mut out)?;
    }
    Ok(out)
}

fn specialize_member(
    member: &Member,
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
    out: &mut Vec<Member>,
) -> Result<(), SemaError> {
    match member {
        Member::ComptimeIf(c) => {
            let selected: &[Member] = if eval_condition(&c.cond, known_consts, skeleton)? {
                &c.then_branch
            } else {
                match &c.else_branch {
                    Some(b) => b,
                    None => return Ok(()),
                }
            };
            out.extend(specialize_members(selected, known_consts, skeleton)?);
            Ok(())
        }
        Member::Field(_) | Member::Pool(_) => {
            out.push(member.clone());
            Ok(())
        }
        Member::Fn(f) => {
            out.push(Member::Fn(specialize_fn(f, known_consts, skeleton)?));
            Ok(())
        }
        Member::Init(i) => {
            out.push(Member::Init(specialize_init(i, known_consts, skeleton)?));
            Ok(())
        }
    }
}

fn specialize_stmts(
    stmts: &[Stmt],
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
) -> Result<Vec<Stmt>, SemaError> {
    let mut out = Vec::new();
    for s in stmts {
        specialize_stmt(s, known_consts, skeleton, &mut out)?;
    }
    Ok(out)
}

fn specialize_stmt(
    stmt: &Stmt,
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
    out: &mut Vec<Stmt>,
) -> Result<(), SemaError> {
    match stmt {
        Stmt::ComptimeIf(c) => {
            let selected: &[Stmt] = if eval_condition(&c.cond, known_consts, skeleton)? {
                &c.then_branch
            } else {
                match &c.else_branch {
                    Some(b) => b,
                    None => return Ok(()),
                }
            };
            out.extend(specialize_stmts(selected, known_consts, skeleton)?);
            Ok(())
        }
        Stmt::If(i) => {
            let then_branch = specialize_stmts(&i.then_branch, known_consts, skeleton)?;
            let mut elifs = Vec::with_capacity(i.elifs.len());
            for elif in &i.elifs {
                elifs.push(ElifClause {
                    body: specialize_stmts(&elif.body, known_consts, skeleton)?,
                    ..elif.clone()
                });
            }
            let else_branch = match &i.else_branch {
                Some(b) => Some(specialize_stmts(b, known_consts, skeleton)?),
                None => None,
            };
            out.push(Stmt::If(IfStmt {
                then_branch,
                elifs,
                else_branch,
                ..i.clone()
            }));
            Ok(())
        }
        Stmt::Match(m) => {
            let mut arms = Vec::with_capacity(m.arms.len());
            for arm in &m.arms {
                arms.push(MatchArm {
                    body: specialize_stmts(&arm.body, known_consts, skeleton)?,
                    ..arm.clone()
                });
            }
            out.push(Stmt::Match(MatchStmt { arms, ..m.clone() }));
            Ok(())
        }
        Stmt::For(f) => {
            out.push(Stmt::For(ForStmt {
                body: specialize_stmts(&f.body, known_consts, skeleton)?,
                ..f.clone()
            }));
            Ok(())
        }
        Stmt::While(w) => {
            out.push(Stmt::While(WhileStmt {
                body: specialize_stmts(&w.body, known_consts, skeleton)?,
                ..w.clone()
            }));
            Ok(())
        }
        Stmt::Defer(d) => {
            let body = match &d.body {
                DeferBody::Expr(e) => DeferBody::Expr(e.clone()),
                DeferBody::Suite(s) => {
                    DeferBody::Suite(specialize_stmts(s, known_consts, skeleton)?)
                }
            };
            out.push(Stmt::Defer(DeferStmt { body, ..d.clone() }));
            Ok(())
        }
        Stmt::With(w) => {
            out.push(Stmt::With(WithStmt {
                body: specialize_stmts(&w.body, known_consts, skeleton)?,
                ..w.clone()
            }));
            Ok(())
        }
        Stmt::Assign(_)
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::Return(..)
        | Stmt::Pass(_)
        | Stmt::Assert(_)
        | Stmt::Send(..)
        | Stmt::Expr(..)
        | Stmt::ComptimeAssert(..) => {
            out.push(stmt.clone());
            Ok(())
        }
    }
}
