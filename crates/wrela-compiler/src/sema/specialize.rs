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
//!   reference **only literals, prelude fieldless-enum variants
//!   (`DriverMode.Irq`), and plain top-level `const` items declared
//!   directly in the module** (i.e. `Item::Const` appearing in
//!   `module.items` itself, never nested inside *any* `comptime if`
//!   branch, selected or not) — combined with unary/binary/logical
//!   operators. No fn calls, no locals, no `self`.
//! - **plans/M7.md item G, decision 18:** a member/statement `comptime if`
//!   whose condition also names the enclosing struct's own const generic
//!   parameters (e.g. `MODE == DriverMode.Irq` on
//!   `BlkDriver[const MODE: DriverMode]`) is **deferred** — left in the
//!   AST for `generics::instantiate_struct` to expand per instantiation.
//!   Module-scope `comptime if` still cannot name a generic parameter.
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

use std::collections::{BTreeMap, BTreeSet};

use crate::eval::{interp, to_sema_error, value::Value};
use crate::sema::bodies::{self, FnCtx, ModuleCtx};
use crate::sema::stdlib_enums;
use crate::sema::typed::TypedProgram;
use crate::sema::types::{self, Type};
use crate::sema::{SemaError, symbols, unimplemented_at};
use crate::syntax::ast::{
    ClosureBody, ComptimeIfMember, ComptimeIfStmt, ConstItem, DeferBody, DeferStmt, ElifClause,
    Expr, FnItem, ForStmt, GenericParam, IfStmt, InitItem, Item, MatchArm, MatchStmt, Member,
    Module, Stmt, StructItem, WhileStmt, WithStmt,
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
            Item::Enum(_) | Item::Pool(_) | Item::Const(_) | Item::Static(_) => {}
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
        | Expr::Unit(..) => {}
        Expr::FStr(f) => {
            if let Ok(desugared) = crate::sema::fstring::desugar_fstring(f) {
                collect_names_in_expr(&desugared, out);
            }
        }
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
        Expr::ArrayRepeat(_, elem, count) => {
            collect_names_in_expr(elem, out);
            collect_names_in_expr(count, out);
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
    let mctx =
        bodies::build_module_ctx(&skeleton_module, &decl_items, &types::ImportedTypes::new());
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
fn check_comptime_vocabulary(
    e: &Expr,
    known_consts: &BTreeSet<String>,
    enclosing_const_generics: &BTreeSet<String>,
) -> Result<(), SemaError> {
    match e {
        Expr::Int(..) | Expr::Bool(..) | Expr::Char(..) | Expr::Unit(..) => Ok(()),
        Expr::Name(span, name) => {
            if known_consts.contains(name) || enclosing_const_generics.contains(name) {
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
        // plans/M7.md item G, decision 18: `DriverMode.Irq` / `Target.*` /
        // `Failure.*` in a condition (fieldless prelude enum variants).
        Expr::Field(base, span, variant) => match base.as_ref() {
            Expr::Name(_, ename) => {
                // plans/M9.md item QQ: load failures are `error[build]`.
                if stdlib_enums::variant_strs(ename)?
                    .is_some_and(|vs| vs.contains(&variant.as_str()))
                {
                    Ok(())
                } else {
                    Err(unimplemented_at(
                        "this expression form in a `comptime if` condition is",
                        *span,
                    ))
                }
            }
            _ => Err(unimplemented_at(
                "this expression form in a `comptime if` condition is",
                *span,
            )),
        },
        Expr::Unary(_, _, inner) | Expr::Not(_, inner) => {
            check_comptime_vocabulary(inner, known_consts, enclosing_const_generics)
        }
        Expr::Binary(_, _, l, r) | Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            check_comptime_vocabulary(l, known_consts, enclosing_const_generics)?;
            check_comptime_vocabulary(r, known_consts, enclosing_const_generics)
        }
        other => Err(unimplemented_at(
            "this expression form in a `comptime if` condition is",
            other.span(),
        )),
    }
}

fn condition_uses_enclosing_const_generic(
    e: &Expr,
    enclosing_const_generics: &BTreeSet<String>,
) -> bool {
    match e {
        Expr::Name(_, name) => enclosing_const_generics.contains(name),
        Expr::Field(base, _, _) | Expr::Unary(_, _, base) | Expr::Not(_, base) => {
            condition_uses_enclosing_const_generic(base, enclosing_const_generics)
        }
        Expr::Binary(_, _, l, r) | Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            condition_uses_enclosing_const_generic(l, enclosing_const_generics)
                || condition_uses_enclosing_const_generic(r, enclosing_const_generics)
        }
        _ => false,
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
    enclosing_const_generics: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
) -> Result<bool, SemaError> {
    check_comptime_vocabulary(cond, known_consts, enclosing_const_generics)?;
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
    let empty = BTreeSet::new();
    let mut out = Vec::new();
    for item in items {
        specialize_item(item, known_consts, skeleton, &empty, &mut out)?;
    }
    Ok(out)
}

fn specialize_item(
    item: &Item,
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
    enclosing_const_generics: &BTreeSet<String>,
    out: &mut Vec<Item>,
) -> Result<(), SemaError> {
    match item {
        Item::ComptimeIf(c) => {
            // Module-scope: never defer on a generic parameter.
            let selected: &[Item] =
                if eval_condition(&c.cond, known_consts, enclosing_const_generics, skeleton)? {
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
        Item::Const(_) | Item::Enum(_) | Item::Pool(_) | Item::Static(_) => {
            out.push(item.clone());
            Ok(())
        }
        Item::Fn(f) => {
            out.push(Item::Fn(specialize_fn(
                f,
                known_consts,
                skeleton,
                enclosing_const_generics,
            )?));
            Ok(())
        }
        Item::Struct(s) => {
            let enclosing: BTreeSet<String> = s
                .generics
                .iter()
                .filter_map(|g| match g {
                    GenericParam::Const { name, .. } => Some(name.clone()),
                    GenericParam::Type { .. } => None,
                })
                .collect();
            let members = specialize_members(&s.members, known_consts, skeleton, &enclosing)?;
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
    enclosing_const_generics: &BTreeSet<String>,
) -> Result<FnItem, SemaError> {
    let body = match &f.body {
        Some(b) => Some(specialize_stmts(
            b,
            known_consts,
            skeleton,
            enclosing_const_generics,
        )?),
        None => None,
    };
    Ok(FnItem { body, ..f.clone() })
}

fn specialize_init(
    i: &InitItem,
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
    enclosing_const_generics: &BTreeSet<String>,
) -> Result<InitItem, SemaError> {
    Ok(InitItem {
        body: specialize_stmts(&i.body, known_consts, skeleton, enclosing_const_generics)?,
        ..i.clone()
    })
}

fn specialize_members(
    members: &[Member],
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
    enclosing_const_generics: &BTreeSet<String>,
) -> Result<Vec<Member>, SemaError> {
    let mut out = Vec::new();
    for m in members {
        specialize_member(
            m,
            known_consts,
            skeleton,
            enclosing_const_generics,
            &mut out,
        )?;
    }
    Ok(out)
}

fn specialize_member(
    member: &Member,
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
    enclosing_const_generics: &BTreeSet<String>,
    out: &mut Vec<Member>,
) -> Result<(), SemaError> {
    match member {
        Member::ComptimeIf(c) => {
            check_comptime_vocabulary(&c.cond, known_consts, enclosing_const_generics)?;
            if condition_uses_enclosing_const_generic(&c.cond, enclosing_const_generics) {
                // Decision 18: defer for per-instantiation expansion.
                out.push(Member::ComptimeIf(ComptimeIfMember {
                    then_branch: specialize_members(
                        &c.then_branch,
                        known_consts,
                        skeleton,
                        enclosing_const_generics,
                    )?,
                    else_branch: match &c.else_branch {
                        Some(b) => Some(specialize_members(
                            b,
                            known_consts,
                            skeleton,
                            enclosing_const_generics,
                        )?),
                        None => None,
                    },
                    ..c.clone()
                }));
                return Ok(());
            }
            let selected: &[Member] =
                if eval_condition(&c.cond, known_consts, enclosing_const_generics, skeleton)? {
                    &c.then_branch
                } else {
                    match &c.else_branch {
                        Some(b) => b,
                        None => return Ok(()),
                    }
                };
            out.extend(specialize_members(
                selected,
                known_consts,
                skeleton,
                enclosing_const_generics,
            )?);
            Ok(())
        }
        Member::Field(_) | Member::Pool(_) => {
            out.push(member.clone());
            Ok(())
        }
        Member::Fn(f) => {
            out.push(Member::Fn(specialize_fn(
                f,
                known_consts,
                skeleton,
                enclosing_const_generics,
            )?));
            Ok(())
        }
        Member::Init(i) => {
            out.push(Member::Init(specialize_init(
                i,
                known_consts,
                skeleton,
                enclosing_const_generics,
            )?));
            Ok(())
        }
    }
}

fn specialize_stmts(
    stmts: &[Stmt],
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
    enclosing_const_generics: &BTreeSet<String>,
) -> Result<Vec<Stmt>, SemaError> {
    let mut out = Vec::new();
    for s in stmts {
        specialize_stmt(
            s,
            known_consts,
            skeleton,
            enclosing_const_generics,
            &mut out,
        )?;
    }
    Ok(out)
}

fn specialize_stmt(
    stmt: &Stmt,
    known_consts: &BTreeSet<String>,
    skeleton: &ConstSkeleton,
    enclosing_const_generics: &BTreeSet<String>,
    out: &mut Vec<Stmt>,
) -> Result<(), SemaError> {
    match stmt {
        Stmt::ComptimeIf(c) => {
            check_comptime_vocabulary(&c.cond, known_consts, enclosing_const_generics)?;
            if condition_uses_enclosing_const_generic(&c.cond, enclosing_const_generics) {
                out.push(Stmt::ComptimeIf(ComptimeIfStmt {
                    then_branch: specialize_stmts(
                        &c.then_branch,
                        known_consts,
                        skeleton,
                        enclosing_const_generics,
                    )?,
                    else_branch: match &c.else_branch {
                        Some(b) => Some(specialize_stmts(
                            b,
                            known_consts,
                            skeleton,
                            enclosing_const_generics,
                        )?),
                        None => None,
                    },
                    ..c.clone()
                }));
                return Ok(());
            }
            let selected: &[Stmt] =
                if eval_condition(&c.cond, known_consts, enclosing_const_generics, skeleton)? {
                    &c.then_branch
                } else {
                    match &c.else_branch {
                        Some(b) => b,
                        None => return Ok(()),
                    }
                };
            out.extend(specialize_stmts(
                selected,
                known_consts,
                skeleton,
                enclosing_const_generics,
            )?);
            Ok(())
        }
        Stmt::If(i) => {
            let then_branch = specialize_stmts(
                &i.then_branch,
                known_consts,
                skeleton,
                enclosing_const_generics,
            )?;
            let mut elifs = Vec::with_capacity(i.elifs.len());
            for elif in &i.elifs {
                elifs.push(ElifClause {
                    body: specialize_stmts(
                        &elif.body,
                        known_consts,
                        skeleton,
                        enclosing_const_generics,
                    )?,
                    ..elif.clone()
                });
            }
            let else_branch = match &i.else_branch {
                Some(b) => Some(specialize_stmts(
                    b,
                    known_consts,
                    skeleton,
                    enclosing_const_generics,
                )?),
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
                    body: specialize_stmts(
                        &arm.body,
                        known_consts,
                        skeleton,
                        enclosing_const_generics,
                    )?,
                    ..arm.clone()
                });
            }
            out.push(Stmt::Match(MatchStmt { arms, ..m.clone() }));
            Ok(())
        }
        Stmt::For(f) => {
            out.push(Stmt::For(ForStmt {
                body: specialize_stmts(&f.body, known_consts, skeleton, enclosing_const_generics)?,
                ..f.clone()
            }));
            Ok(())
        }
        Stmt::While(w) => {
            out.push(Stmt::While(WhileStmt {
                body: specialize_stmts(&w.body, known_consts, skeleton, enclosing_const_generics)?,
                ..w.clone()
            }));
            Ok(())
        }
        Stmt::Defer(d) => {
            let body = match &d.body {
                DeferBody::Expr(e) => DeferBody::Expr(e.clone()),
                DeferBody::Suite(s) => DeferBody::Suite(specialize_stmts(
                    s,
                    known_consts,
                    skeleton,
                    enclosing_const_generics,
                )?),
            };
            out.push(Stmt::Defer(DeferStmt { body, ..d.clone() }));
            Ok(())
        }
        Stmt::With(w) => {
            out.push(Stmt::With(WithStmt {
                body: specialize_stmts(&w.body, known_consts, skeleton, enclosing_const_generics)?,
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

// ===========================================================================
// plans/M7.md item G, decision 18: per-instantiation expansion of deferred
// `comptime if` nodes that named the enclosing struct's const generics.
// ===========================================================================

/// Rewrite bare const-generic names to their concrete argument expressions.
fn bind_const_names(e: &Expr, consts: &BTreeMap<String, Expr>) -> Expr {
    match e {
        Expr::Name(_, name) => consts.get(name).cloned().unwrap_or_else(|| e.clone()),
        Expr::Field(base, span, variant) => Expr::Field(
            Box::new(bind_const_names(base, consts)),
            *span,
            variant.clone(),
        ),
        Expr::Unary(span, op, inner) => {
            Expr::Unary(*span, *op, Box::new(bind_const_names(inner, consts)))
        }
        Expr::Not(span, inner) => Expr::Not(*span, Box::new(bind_const_names(inner, consts))),
        Expr::Binary(span, op, l, r) => Expr::Binary(
            *span,
            *op,
            Box::new(bind_const_names(l, consts)),
            Box::new(bind_const_names(r, consts)),
        ),
        Expr::And(span, l, r) => Expr::And(
            *span,
            Box::new(bind_const_names(l, consts)),
            Box::new(bind_const_names(r, consts)),
        ),
        Expr::Or(span, l, r) => Expr::Or(
            *span,
            Box::new(bind_const_names(l, consts)),
            Box::new(bind_const_names(r, consts)),
        ),
        other => other.clone(),
    }
}

fn eval_bound_condition(cond: &Expr, mctx: &ModuleCtx) -> Result<bool, SemaError> {
    let known: BTreeSet<String> = mctx.consts.keys().cloned().collect();
    let empty = BTreeSet::new();
    check_comptime_vocabulary(cond, &known, &empty)?;
    let mut fctx = FnCtx::new(Type::Unit, mctx.module_pools.clone());
    let typed_cond = bodies::check_expr(cond, Some(&Type::Bool), &mut fctx, mctx)?;
    let mut program = TypedProgram::default();
    for name in ["Target", "Failure", "DriverMode"] {
        // plans/M9.md item QQ: load failures are `error[build]`.
        if let Some(vs) = stdlib_enums::variant_strs(name)? {
            program.enums.insert(
                name.to_string(),
                crate::sema::typed::TypedEnum::from_variants(
                    vs.iter().map(|v| v.to_string()).collect(),
                ),
            );
        }
    }
    for (name, ty) in &mctx.consts {
        if let Some(raw) = mctx.const_values.get(name) {
            let mut fctx = FnCtx::new(Type::Unit, mctx.module_pools.clone());
            if let Ok(value) = bodies::check_expr(raw, Some(ty), &mut fctx, mctx) {
                program.consts.insert(
                    name.clone(),
                    crate::sema::typed::TypedConst {
                        ty: ty.clone(),
                        value,
                    },
                );
            }
        }
    }
    let value = interp::eval_standalone(&program, &typed_cond, "comptime if".to_string())
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

/// Expand deferred member `comptime if`s under a concrete const-generic
/// substitution (`MODE` → `DriverMode.Irq`), returning only concrete members.
pub(crate) fn expand_deferred_members(
    concrete: &[Member],
    deferred: &[Member],
    const_subst: &BTreeMap<String, Expr>,
    mctx: &ModuleCtx,
) -> Result<Vec<Member>, SemaError> {
    let mut out = Vec::new();
    for m in concrete {
        expand_one_member(m, const_subst, mctx, &mut out)?;
    }
    for m in deferred {
        expand_one_member(m, const_subst, mctx, &mut out)?;
    }
    Ok(out)
}

fn expand_one_member(
    member: &Member,
    const_subst: &BTreeMap<String, Expr>,
    mctx: &ModuleCtx,
    out: &mut Vec<Member>,
) -> Result<(), SemaError> {
    match member {
        Member::ComptimeIf(c) => {
            let bound = bind_const_names(&c.cond, const_subst);
            let selected: &[Member] = if eval_bound_condition(&bound, mctx)? {
                &c.then_branch
            } else {
                match &c.else_branch {
                    Some(b) => b,
                    None => return Ok(()),
                }
            };
            for m in selected {
                expand_one_member(m, const_subst, mctx, out)?;
            }
            Ok(())
        }
        Member::Fn(f) => {
            let body = match &f.body {
                Some(b) => Some(expand_deferred_stmts(b, const_subst, mctx)?),
                None => None,
            };
            out.push(Member::Fn(FnItem { body, ..f.clone() }));
            Ok(())
        }
        Member::Init(i) => {
            out.push(Member::Init(InitItem {
                body: expand_deferred_stmts(&i.body, const_subst, mctx)?,
                ..i.clone()
            }));
            Ok(())
        }
        Member::Field(_) | Member::Pool(_) => {
            out.push(member.clone());
            Ok(())
        }
    }
}

fn expand_deferred_stmts(
    stmts: &[Stmt],
    const_subst: &BTreeMap<String, Expr>,
    mctx: &ModuleCtx,
) -> Result<Vec<Stmt>, SemaError> {
    let mut out = Vec::new();
    for s in stmts {
        expand_one_stmt(s, const_subst, mctx, &mut out)?;
    }
    Ok(out)
}

fn expand_one_stmt(
    stmt: &Stmt,
    const_subst: &BTreeMap<String, Expr>,
    mctx: &ModuleCtx,
    out: &mut Vec<Stmt>,
) -> Result<(), SemaError> {
    match stmt {
        Stmt::ComptimeIf(c) => {
            let bound = bind_const_names(&c.cond, const_subst);
            let selected: &[Stmt] = if eval_bound_condition(&bound, mctx)? {
                &c.then_branch
            } else {
                match &c.else_branch {
                    Some(b) => b,
                    None => return Ok(()),
                }
            };
            for s in selected {
                expand_one_stmt(s, const_subst, mctx, out)?;
            }
            Ok(())
        }
        Stmt::If(i) => {
            out.push(Stmt::If(IfStmt {
                then_branch: expand_deferred_stmts(&i.then_branch, const_subst, mctx)?,
                elifs: i
                    .elifs
                    .iter()
                    .map(|e| {
                        Ok(ElifClause {
                            body: expand_deferred_stmts(&e.body, const_subst, mctx)?,
                            ..e.clone()
                        })
                    })
                    .collect::<Result<Vec<_>, SemaError>>()?,
                else_branch: match &i.else_branch {
                    Some(b) => Some(expand_deferred_stmts(b, const_subst, mctx)?),
                    None => None,
                },
                ..i.clone()
            }));
            Ok(())
        }
        Stmt::Match(m) => {
            let mut arms = Vec::with_capacity(m.arms.len());
            for arm in &m.arms {
                arms.push(MatchArm {
                    body: expand_deferred_stmts(&arm.body, const_subst, mctx)?,
                    ..arm.clone()
                });
            }
            out.push(Stmt::Match(MatchStmt { arms, ..m.clone() }));
            Ok(())
        }
        Stmt::For(f) => {
            out.push(Stmt::For(ForStmt {
                body: expand_deferred_stmts(&f.body, const_subst, mctx)?,
                ..f.clone()
            }));
            Ok(())
        }
        Stmt::While(w) => {
            out.push(Stmt::While(WhileStmt {
                body: expand_deferred_stmts(&w.body, const_subst, mctx)?,
                ..w.clone()
            }));
            Ok(())
        }
        Stmt::Defer(d) => {
            let body = match &d.body {
                DeferBody::Expr(e) => DeferBody::Expr(e.clone()),
                DeferBody::Suite(s) => {
                    DeferBody::Suite(expand_deferred_stmts(s, const_subst, mctx)?)
                }
            };
            out.push(Stmt::Defer(DeferStmt { body, ..d.clone() }));
            Ok(())
        }
        Stmt::With(w) => {
            out.push(Stmt::With(WithStmt {
                body: expand_deferred_stmts(&w.body, const_subst, mctx)?,
                ..w.clone()
            }));
            Ok(())
        }
        other => {
            out.push(other.clone());
            Ok(())
        }
    }
}
