//! Statement/expression typing (plans/M2.md item C): assignment
//! introduction/reassignment, `if`/`while` condition typing, `for`
//! typing, operator desugar (02-language.md §7.4, §8.2, 05-library.md
//! §8), call checking (arity, labels), enum literals and leading-dot
//! inference, pattern typing, `is`, closures as structural `fn` types,
//! `?`, `assert`, `defer`. Also where the fail-closed set (decision 7)
//! beyond imports lands: `comptime if`/`comptime assert`, f-strings,
//! `await`/`send`/`with` (group/pool), `@image` bodies.
//!
//! Shape (decision 4): no unification, no constraint solver — every
//! expression is either checked against an expected type the grammar
//! already supplies (`check_expr`), or synthesized on its own
//! (`synth_expr`, called by `check_expr`, which then gates the result
//! against `expected` when one was given). Everything clones freely
//! (`Type`/`DeclFn`/AST nodes all derive `Clone`): `ModuleCtx` below owns
//! plain copies of every declared item's ast + resolved-type pair
//! instead of borrowing, so no lifetime threads through the whole file.
//!
//! plans/M3.md item A: `check_expr`/`check_stmt` now return a typed node
//! (`typed::TypedExpr`/`typed::TypedStmt`) instead of a bare `Type`/`()` —
//! see `typed.rs`'s own module doc comment for the tree's shape. This
//! file is still the one place that *produces* the typed tree;
//! `access.rs`/`flow.rs`/`matches.rs` call `check_expr`/`check_pattern`
//! exactly as before and only ever read `.ty` off the result where they
//! previously got a bare `Type` back (a recorded non-goal: those three
//! passes are not retrofitted onto the typed tree in M3).
//!
//! A generic declaration's *own* body is still not type-checked here: a
//! generic struct/enum's members, and a generic fn/method's body, are
//! skipped entirely (no error — just not visited) by `check_top_fn`/
//! `check_struct_bodies` below. A *use* of a generic type/fn from a
//! non-generic body (a construction, a call — explicit `[Args]` or, for
//! a top-level generic `fn`, inferred — a field/method/variant lookup
//! through an already-typed value) now resolves the concrete
//! instantiation and enqueues it (item H, `generics.rs` owns
//! substitution + the queue + the actual per-instantiation checking,
//! `mod.rs::check` runs it last). What still fails closed via
//! `unimplemented_at("generic instantiation is", ...)` is item H's own
//! documented scope boundary: a generic *method* (its own `[...]`,
//! beyond its struct's, if any — never instantiated), a bare reference to
//! a generic type/fn as a first-class value without calling it, and a
//! generic-argument shape deeper than this item resolves.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::sema::generics;
use crate::sema::typed::{
    CalleeKey, TestDecl, TestKind, TypedClosureBody, TypedClosureParam, TypedConst, TypedDeferBody,
    TypedElif, TypedExpr, TypedExprKind, TypedFn, TypedForIter, TypedMatchArm, TypedParam,
    TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind, TypedStruct,
};
use crate::sema::types::{
    self, Classification, DeclMember, DeclParam, DeclVariantPayload, Type, TypeArg,
};
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{
    self, AccessMode, Arg, AssertStmt, AssignOp, AssignStmt, BinOp, ClosureBody, ClosureExpr,
    DeferBody, DeferStmt, Expr, ForStmt, IfStmt, Item, MatchStmt, Member, Module, Pattern, Span,
    Stmt, UnaryOp, WhileStmt,
};

// --- item H: the generic-instantiation queue ------------------------------
//
// "Typing a `Generic[Args]` use enqueues the concrete instantiation"
// (plans/M2.md item H): every fail-closed generic-instantiation point this
// pass used to have now instead resolves the concrete use (`generics.rs`
// owns substitution) and registers it here, keyed by a canonical
// `"<kind>:<name>[<args>]"` string (decision 1's "BTreeSet-keyed by name +
// resolved args", realized as a `BTreeMap` so the first discovery's call
// site — the one used for the "instantiated at" chain — is kept rather
// than overwritten by a later, redundant use of the same instantiation).
// `current_chain` is the "instantiated at" stack this exact walk is
// running under: empty for the module's own (non-generic) bodies, and set
// by `generics::check`, which assigns `current_chain` directly around each
// instantiation while it re-runs this same pass over one instantiation's
// substituted declaration — so a
// generic use *discovered while checking an instantiation* enqueues with
// that instantiation's own chain plus one more frame, which is exactly
// how a nested instantiation's diagnostic gets one `instantiated at` line
// per level (decision 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum InstKind {
    Struct,
    Enum,
    Fn,
}

impl InstKind {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            InstKind::Struct => "struct",
            InstKind::Enum => "enum",
            InstKind::Fn => "fn",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct QueuedInstantiation {
    pub(crate) kind: InstKind,
    pub(crate) name: String,
    pub(crate) args: Vec<types::TypeArg>,
    /// The instantiation chain leading to this one, outermost first,
    /// this instantiation's own triggering call site last (decision 2:
    /// "one `instantiated at` line per level, innermost first" — printed
    /// by reversing this).
    pub(crate) chain: Vec<Span>,
}

/// Fixed recursion cap on the instantiation chain (item H): a cycle or a
/// genuinely unbounded generic recursion both hit this before the process
/// stack would. Deliberately small and fixed — no measurement, no
/// configuration (ROADMAP.md's "dumbness is permanent").
pub(crate) const MAX_GENERIC_DEPTH: usize = 64;

// --- module-wide lookup context ------------------------------------------

/// One struct's declared shape, for body typing: the resolved
/// declaration (`types::declare`'s output) plus a parallel, owned copy of
/// its ast members (same order, `comptime if` members already filtered
/// out — mirrors exactly what `types::declare_struct` iterated) so field
/// defaults, method/init bodies, param defaults, and per-member generics
/// are all available without re-walking the module.
// `pub(crate)` throughout `StructInfo`/`FnInfo`/`ModuleCtx` (plans/M2.md
// item D, decision 10's minimal-footprint rule): access.rs re-walks
// bodies with the declared signatures exactly like this pass does, so it
// reuses this same lookup context wholesale (`build_module_ctx`) rather
// than duplicating struct/enum/fn table construction — nothing here is
// restructured, only exposed. `Clone` (item H, generics.rs): a generic
// instantiation's substituted `StructInfo` is built fresh (owned) per
// use, so callers that also handle the plain (borrowed, from `mctx`)
// case need both to fit one `Cow`-typed local.
#[derive(Clone)]
pub(crate) struct StructInfo {
    pub(crate) decl: types::DeclStruct,
    pub(crate) ast_members: Vec<Member>,
}

impl StructInfo {
    pub(crate) fn members(&self) -> impl Iterator<Item = (&Member, &DeclMember)> {
        self.ast_members.iter().zip(self.decl.members.iter())
    }

    pub(crate) fn field_ty(&self, name: &str) -> Option<Type> {
        self.members().find_map(|(am, dm)| match (am, dm) {
            (Member::Field(f), DeclMember::Field(d)) if f.name == name => Some(d.ty.clone()),
            _ => None,
        })
    }

    pub(crate) fn has_member_named(&self, name: &str) -> bool {
        self.ast_members.iter().any(|m| match m {
            Member::Fn(f) => f.name == name,
            Member::Field(f) => f.name == name,
            _ => false,
        })
    }

    pub(crate) fn assoc_fn(&self, name: &str) -> Option<(&ast::FnItem, &types::DeclFn)> {
        self.members().find_map(|(am, dm)| match (am, dm) {
            (Member::Fn(f), DeclMember::Fn(d)) if f.name == name && f.receiver.is_none() => {
                Some((f, d))
            }
            _ => None,
        })
    }

    pub(crate) fn method(&self, name: &str) -> Option<(&ast::FnItem, &types::DeclFn)> {
        self.members().find_map(|(am, dm)| match (am, dm) {
            (Member::Fn(f), DeclMember::Fn(d)) if f.name == name && f.receiver.is_some() => {
                Some((f, d))
            }
            _ => None,
        })
    }

    pub(crate) fn init(&self) -> Option<(&ast::InitItem, &types::DeclFn)> {
        self.members().find_map(|(am, dm)| match (am, dm) {
            (Member::Init(i), DeclMember::Init(d)) => Some((i, d)),
            _ => None,
        })
    }
}

/// One top-level fn's ast (params/defaults/generics/attrs/body) plus its
/// resolved declaration. `Clone` (plans/M4.md item A): the multi-module
/// entry (`sema::check_program`) splices an *already-built* `FnInfo`
/// straight from the exporting module's own independent `ModuleCtx`
/// into an importing module's, under the (possibly aliased) local name
/// — a plain, owned copy, never a re-check (see that function's own doc
/// comment for why this is sound even across an import cycle).
#[derive(Clone)]
pub(crate) struct FnInfo {
    pub(crate) ast: ast::FnItem,
    pub(crate) decl: types::DeclFn,
}

/// Everything body-typing needs to resolve names beyond the current
/// function: struct/enum/fn/const declarations, generic arity (for
/// annotation resolution and the generic-instantiation fail-closed
/// check), and module-scope pool names. Built once per `check` call from
/// `module` + `declare`'s already-resolved `decl_items`; nothing here
/// borrows either, so no lifetime parameter is needed anywhere in this
/// file (decision 4: clone freely).
pub(crate) struct ModuleCtx {
    pub(crate) shapes: BTreeMap<String, usize>,
    pub(crate) module_pools: BTreeSet<String>,
    pub(crate) structs: BTreeMap<String, StructInfo>,
    pub(crate) enums: BTreeMap<String, types::DeclEnum>,
    pub(crate) fns: BTreeMap<String, FnInfo>,
    pub(crate) consts: BTreeMap<String, Type>,
    /// Every module-level `const`'s own initializer expression, by name
    /// (item H, generics.rs): a const generic argument may spell a bare
    /// `const` name (decision 4), so evaluating it needs the *value*, not
    /// just the type `consts` above already carries.
    pub(crate) const_values: BTreeMap<String, Expr>,
    /// Item H's instantiation worklist — see the module-level doc comment
    /// above `InstKind`. Interior mutability (`RefCell`) is deliberate: it
    /// lets every existing check function go on taking `&ModuleCtx`
    /// unchanged (decision 10's minimal-footprint rule) while still
    /// accumulating instantiation requests discovered arbitrarily deep in
    /// the walk.
    pub(crate) generics_queue: RefCell<BTreeMap<String, QueuedInstantiation>>,
    /// The instantiation chain the *current* walk over this `ModuleCtx`
    /// is running under — empty for the module's own bodies, set by
    /// `generics::check` while re-running this pass over a substituted
    /// declaration. See the module-level doc comment above `InstKind`.
    pub(crate) current_chain: RefCell<Vec<Span>>,
}

impl ModuleCtx {
    /// Resolves an ast type exactly like `types::declare` did (reusing
    /// its own `resolve_type`), with no generics in scope — every body
    /// this pass actually checks lives inside a non-generic declaration
    /// (item H's job otherwise), so a local annotation, closure param
    /// annotation, etc. can never legally name a generic parameter here.
    pub(crate) fn resolve_type(
        &self,
        ty: &ast::Type,
        local_pools: &BTreeSet<String>,
    ) -> Result<Type, SemaError> {
        types::resolve_type(
            ty,
            &self.shapes,
            &self.module_pools,
            local_pools,
            &BTreeMap::new(),
            false,
        )
    }
}

/// Widened to `pub(crate)` (item G, matches.rs): the exhaustiveness pass
/// rebuilds its own `ModuleCtx` the same dumb, no-state-threaded way
/// every other pass does (mirrors `mod.rs::dump` re-running `declare`).
pub(crate) fn build_module_ctx(module: &Module, decl_items: &[types::DeclItem]) -> ModuleCtx {
    let mut shapes = BTreeMap::new();
    let mut module_pools = BTreeSet::new();
    let mut structs = BTreeMap::new();
    let mut enums = BTreeMap::new();
    let mut fns = BTreeMap::new();
    let mut consts = BTreeMap::new();
    let mut const_values = BTreeMap::new();

    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();

    for (ai, di) in ast_items.iter().zip(decl_items.iter()) {
        match (ai, di) {
            (Item::Struct(s), types::DeclItem::Struct(d)) => {
                shapes.insert(s.name.clone(), s.generics.len());
                let ast_members: Vec<Member> = s
                    .members
                    .iter()
                    .filter(|m| !matches!(m, Member::ComptimeIf(_)))
                    .cloned()
                    .collect();
                structs.insert(
                    s.name.clone(),
                    StructInfo {
                        decl: d.clone(),
                        ast_members,
                    },
                );
            }
            (Item::Enum(e), types::DeclItem::Enum(d)) => {
                shapes.insert(e.name.clone(), e.generics.len());
                enums.insert(e.name.clone(), d.clone());
            }
            (Item::Fn(f), types::DeclItem::Fn(d)) => {
                fns.insert(
                    f.name.clone(),
                    FnInfo {
                        ast: f.clone(),
                        decl: d.clone(),
                    },
                );
            }
            (Item::Const(c), types::DeclItem::Const(d)) => {
                consts.insert(c.name.clone(), d.ty.clone());
                const_values.insert(c.name.clone(), c.value.clone());
            }
            (Item::Pool(p), types::DeclItem::Pool(_)) => {
                module_pools.insert(p.name.clone());
            }
            _ => unreachable!("declare()'s items must pair 1:1 with the filtered ast items"),
        }
    }

    ModuleCtx {
        shapes,
        module_pools,
        structs,
        enums,
        fns,
        consts,
        const_values,
        generics_queue: RefCell::new(BTreeMap::new()),
        current_chain: RefCell::new(Vec::new()),
    }
}

/// Registers one concrete instantiation (item H): builds the canonical
/// key (decision 1), checks the recursion depth cap against `mctx`'s
/// *current* chain (the instantiation(s) already being checked when this
/// use was discovered — empty for an ordinary module-level use), and
/// inserts it if this exact `(kind, name, args)` has not been seen yet —
/// first discovery wins the recorded call site, which is what makes the
/// eventual "instantiated at" chain deterministic when the same
/// instantiation is used from more than one place. Returns the canonical
/// key either way, so a caller that only needs "has this been queued"
/// doesn't have to re-render it.
pub(crate) fn enqueue_instantiation(
    mctx: &ModuleCtx,
    kind: InstKind,
    name: &str,
    args: &[types::TypeArg],
    call_span: Span,
) -> Result<String, SemaError> {
    let key = generics::canonical_key(kind, name, args);
    let mut chain = mctx.current_chain.borrow().clone();
    chain.push(call_span);
    if chain.len() > MAX_GENERIC_DEPTH {
        return Err(SemaError::at(
            "generic",
            format!(
                "instantiation depth exceeded {MAX_GENERIC_DEPTH} while instantiating `{name}`"
            ),
            call_span,
        ));
    }
    mctx.generics_queue
        .borrow_mut()
        .entry(key.clone())
        .or_insert_with(|| QueuedInstantiation {
            kind,
            name: name.to_string(),
            args: args.to_vec(),
            chain,
        });
    Ok(key)
}

// --- per-body checking context -------------------------------------------

/// One function/method/init/closure body's typing state: the current
/// return type (for `return`/`?`), a local-variable scope stack (only a
/// closure pushes a new one — mirrors `symbols::Resolver`'s scope model
/// exactly, decision 3's name-resolution shape reused for types), and
/// the pool names visible by bare name inside `own[P]` annotations here
/// (a struct's own `pool` members, when checking one of its
/// methods/init; otherwise just the module's).
/// Widened to `pub(crate)` (item G, matches.rs): the exhaustiveness pass
/// re-derives scrutinee types by re-walking every body in lockstep with
/// this same flat-scope model (only a closure pushes a new scope), so it
/// needs the same local-variable state `check_expr` reads/writes.
pub(crate) struct FnCtx {
    pub(crate) ret_ty: Type,
    locals: Vec<BTreeMap<String, Type>>,
    pub(crate) local_pools: BTreeSet<String>,
}

impl FnCtx {
    pub(crate) fn new(ret_ty: Type, local_pools: BTreeSet<String>) -> FnCtx {
        FnCtx {
            ret_ty,
            locals: vec![BTreeMap::new()],
            local_pools,
        }
    }

    pub(crate) fn push_scope(&mut self) {
        self.locals.push(BTreeMap::new());
    }

    pub(crate) fn pop_scope(&mut self) {
        self.locals.pop();
    }

    /// A read-position lookup: innermost scope outward, matching
    /// `symbols::Resolver::resolve_name`'s search order.
    pub(crate) fn lookup_local(&self, name: &str) -> Option<Type> {
        for scope in self.locals.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t.clone());
            }
        }
        None
    }

    pub(crate) fn lookup_innermost(&self, name: &str) -> Option<Type> {
        self.locals
            .last()
            .expect("at least one scope")
            .get(name)
            .cloned()
    }

    pub(crate) fn insert_local(&mut self, name: String, ty: Type) {
        self.locals
            .last_mut()
            .expect("at least one scope")
            .insert(name, ty);
    }
}

/// Binds `name` to `ty` in the current (innermost) scope: a plain insert
/// if this is the first binding, an equality check if `name` is already
/// bound there — this is how a match arm's pattern binding, a `for`
/// binding, and a fresh assignment all interact with a name reused by an
/// *earlier* sibling branch in the same flat scope (name resolution
/// permits this — only a closure pushes a new scope — so typing must
/// decide what happens: reusing the name requires the same type, which
/// is a dumb, sound, non-flow-sensitive stand-in for the real arm-merge
/// analysis flow's pass (items E/F) owns).
fn bind_local(fctx: &mut FnCtx, name: &str, ty: Type, span: Span) -> Result<(), SemaError> {
    if let Some(existing) = fctx.lookup_innermost(name) {
        if !types_eq(&existing, &ty) {
            return Err(type_error(
                format!(
                    "`{name}` is already bound to type `{}` here; found `{}`",
                    types::render_type(&existing),
                    types::render_type(&ty)
                ),
                span,
            ));
        }
        Ok(())
    } else {
        fctx.insert_local(name.to_string(), ty);
        Ok(())
    }
}

// --- entry point -----------------------------------------------------------

/// The body-typing pass (plans/M2.md item C): runs after `declare`
/// (types.rs), which this needs for every signature/field/classification
/// already resolved. Fail-fast, source order, one module-wide walk: for
/// each top-level `const`/`fn`/`struct`, checks its body/bodies; `enum`
/// and `pool` items have none. A generic declaration's own body (or a
/// generic member's, inside an otherwise-concrete struct) is skipped —
/// not an error, just unchecked; item H (`generics::check`, run by
/// `mod.rs::check` right after `matches::check`) checks each one exactly
/// once, concretely, for every instantiation this walk (or item D's/G's
/// re-walks of the same module) discovers and enqueues into `mctx`.
///
/// `mctx` is built once by the caller (`mod.rs::check`) and shared with
/// `access::check`/`matches::check`/`generics::check` so item H's
/// instantiation queue accumulates across all of them (see the doc
/// comment on `InstKind` above).
///
/// plans/M3.md item A: also returns the typed program (decision 1) for
/// every plain (non-generic) top-level `const`/`fn`/`struct` this walk
/// checks; `mod.rs::check_typed` fills in `instantiations` afterward
/// (`generics::check` drains the queue this — and `access`/`flow`/
/// `matches`'s own re-derivation — populates). The plain `check` stage
/// (`mod.rs::check`) discards this return value; the pass's own
/// diagnostics/behavior are unchanged either way.
pub(crate) fn check(
    module: &Module,
    decl_items: &[types::DeclItem],
    mctx: &ModuleCtx,
) -> Result<TypedProgram, SemaError> {
    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();
    let mut program = TypedProgram::default();
    for (ai, di) in ast_items.iter().zip(decl_items.iter()) {
        match (ai, di) {
            (Item::Const(c), types::DeclItem::Const(d)) => {
                let mut fctx = FnCtx::new(Type::Unit, mctx.module_pools.clone());
                let value = check_expr(&c.value, Some(&d.ty), &mut fctx, mctx)?;
                program.consts.insert(
                    c.name.clone(),
                    TypedConst {
                        ty: d.ty.clone(),
                        value,
                    },
                );
            }
            (Item::Fn(f), types::DeclItem::Fn(d)) => {
                // plans/M3.md item E: `@test`'s own shape validation runs
                // whether or not the fn is generic (a generic `@test` fn
                // fails closed below, symmetric with `@image`'s own
                // whole-declaration fail-closed a few lines up) — done
                // *before* `check_top_fn` so the diagnostic fires even
                // when the body itself would otherwise check cleanly.
                let test_kind = test_attr_kind(f)?;
                if test_kind == Some(TestKind::Exhaustive) {
                    check_exhaustive_test_params(f, d, mctx)?;
                }
                if let Some(tf) = check_top_fn(f, d, mctx)? {
                    program.fns.insert(f.name.clone(), tf);
                    if let Some(kind) = test_kind {
                        program.tests.push(TestDecl {
                            name: f.name.clone(),
                            kind,
                        });
                    }
                } else if test_kind.is_some() {
                    return Err(unimplemented_at("`@test` on a generic fn is", f.span));
                }
            }
            (Item::Struct(s), types::DeclItem::Struct(_)) => {
                if let Some(ts) = check_struct_bodies(s, mctx)? {
                    program.structs.insert(s.name.clone(), ts);
                }
            }
            (Item::Enum(e), types::DeclItem::Enum(d)) => {
                // A generic enum's own variant order is recorded once it
                // is instantiated (item H's job); a plain enum's is
                // recorded here, alongside every other plain top-level
                // declaration this pass checks (`typed::TypedProgram::enums`'s
                // own doc comment).
                if e.generics.is_empty() {
                    program.enums.insert(
                        e.name.clone(),
                        d.variants.iter().map(|v| v.name.clone()).collect(),
                    );
                }
            }
            _ => {}
        }
    }
    Ok(program)
}

pub(crate) fn is_image_fn(f: &ast::FnItem) -> bool {
    f.attrs.iter().any(|a| a.name == "image")
}

/// `@test`/`@test(runtime)` recognition (plans/M3.md item E,
/// 02-language.md §12.2). This is the *only* attribute-shape validation
/// this milestone adds — every attribute besides `@image` (whole-body
/// fail-closed, above) and `@test` (here) still goes entirely
/// unvalidated by sema, exactly as it did before this item (13's own
/// "unknown attributes are errors" rule is not yet enforced anywhere;
/// see the session report). Returns `Ok(None)` when `f` carries no
/// `@test` attribute at all (the overwhelmingly common case, so callers
/// never need to special-case "not a test"); `Ok(Some(kind))` for a
/// validated one; `Err` for a malformed one — a `@test` fn declared with
/// parameters (decision 9's own "@test fns with parameters: diagnose"),
/// or an attribute argument that is not the bare name `runtime`.
/// Category `type`, `arity_error`'s own neighbor — a bad declaration
/// shape, not a new diagnostic category (`xtask`'s `SEMA_CATEGORIES` is
/// a fixed set, plans/M2.md decision 1; this item does not extend it).
pub(crate) fn test_attr_kind(f: &ast::FnItem) -> Result<Option<TestKind>, SemaError> {
    let Some(attr) = f.attrs.iter().find(|a| a.name == "test") else {
        return Ok(None);
    };
    let kind = match attr.args.as_slice() {
        [] => TestKind::Comptime,
        [arg] => match &arg.value {
            Expr::Name(_, name) if name == "runtime" && arg.label.is_none() => TestKind::Runtime,
            Expr::Name(_, name) if name == "exhaustive" && arg.label.is_none() => {
                TestKind::Exhaustive
            }
            _ => {
                return Err(type_error(
                    "`@test`'s only argument is the bare name `runtime` or `exhaustive`"
                        .to_string(),
                    attr.span,
                ));
            }
        },
        _ => {
            return Err(type_error(
                "`@test` takes at most one argument (`runtime` or `exhaustive`)".to_string(),
                attr.span,
            ));
        }
    };
    // An exhaustive test's whole point is its parameters (the enumerated
    // domain — their types are validated against the *resolved*
    // declaration in `check`'s own per-item loop, not here where only
    // raw ast is visible); the other two kinds take none.
    match kind {
        TestKind::Exhaustive if f.params.is_empty() => Err(type_error(
            format!(
                "`@test(exhaustive)` fn `{}` needs at least one parameter (the enumerated domain)",
                f.name
            ),
            f.span,
        )),
        TestKind::Comptime | TestKind::Runtime if !f.params.is_empty() => Err(type_error(
            format!("`@test` fn `{}` takes no arguments", f.name),
            f.span,
        )),
        _ => Ok(Some(kind)),
    }
}

/// `@test(exhaustive)`'s parameter validation (02-language.md §12.2),
/// run against the *resolved* declaration: every parameter must be
/// default-mode (`read` — the domain is data handed in by value, never
/// `mut`/`take`) and of an enumerable type — `bool`, `u8`, `i8`, or a
/// fieldless non-generic module `enum` — the finite domains small
/// enough to enumerate outright (`eval::quota::MAX_EXHAUSTIVE_CASES`
/// caps the *product* at run time; this check bounds each factor's
/// kind). Everything else is rejected here, at declaration, so
/// `wrela test` never has to invent a domain.
fn check_exhaustive_test_params(
    f: &ast::FnItem,
    d: &types::DeclFn,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    for p in &d.params {
        if p.mode != AccessMode::Read {
            return Err(type_error(
                format!(
                    "`@test(exhaustive)` fn `{}`'s parameter `{}` must be a plain (read) parameter",
                    f.name, p.name
                ),
                f.span,
            ));
        }
        let enumerable = match &p.ty {
            Type::Bool | Type::U8 | Type::I8 => true,
            Type::Named(name, targs) if targs.is_empty() => match mctx.enums.get(name) {
                Some(en) => en
                    .variants
                    .iter()
                    .all(|v| matches!(v.payload, types::DeclVariantPayload::None)),
                None => false,
            },
            _ => false,
        };
        if !enumerable {
            return Err(type_error(
                format!(
                    "`@test(exhaustive)` fn `{}`'s parameter `{}` has no enumerable domain \
                     (supported: `bool`, `u8`, `i8`, a fieldless enum), found `{}`",
                    f.name,
                    p.name,
                    types::render_type(&p.ty)
                ),
                f.span,
            ));
        }
    }
    Ok(())
}

pub(crate) fn local_pool_names(info: &StructInfo) -> BTreeSet<String> {
    info.ast_members
        .iter()
        .filter_map(|m| match m {
            Member::Pool(p) => Some(p.name.clone()),
            _ => None,
        })
        .collect()
}

/// `pub(crate)` (item H, generics.rs): re-run verbatim over a
/// substituted, generics-cleared copy of a generic fn/method's own ast
/// (`generics::instantiate_fn`) — the "dumbest workable shape" plans/M2.md
/// item H asks for. Nothing below reads `f.generics` for anything but
/// this guard, so a cleared copy behaves exactly like a real non-generic
/// declaration; every type it needs instead comes from `d`, which the
/// caller has already substituted.
///
/// Returns `Ok(None)` for a generic fn's own (unchecked) body — item H's
/// job elsewhere — and `Ok(Some(typed_fn))` for every concrete body this
/// checks (plans/M3.md item A): a plain top-level fn here, or (via
/// `generics::check_one_instantiation`) an instantiated generic fn, which
/// is always concrete by the time it reaches this function.
pub(crate) fn check_top_fn(
    f: &ast::FnItem,
    d: &types::DeclFn,
    mctx: &ModuleCtx,
) -> Result<Option<TypedFn>, SemaError> {
    if is_image_fn(f) {
        // The whole declaration is unchecked (decision 7): the image
        // constructor's semantics (device/actor/pool wiring) are M4's.
        return Err(unimplemented_at("@image bodies are", f.span));
    }
    if !f.generics.is_empty() {
        return Ok(None); // generic body: item H's job, not checked here.
    }
    let mut fctx = FnCtx::new(d.ret.clone(), mctx.module_pools.clone());
    let params = check_params_with_defaults(&f.params, &d.params, &mut fctx, mctx)?;
    let body = match &f.body {
        Some(body) => check_stmts(body, &mut fctx, mctx)?,
        // The parser accepts the bodyless signature shorthand a few doc
        // tables use; whether a real declaration may be bodyless is a
        // later milestone's question (see parse_fn_tail), so sema fails
        // closed rather than treating it as an empty body.
        None => return Err(unimplemented_at("bodyless functions are", f.span)),
    };
    Ok(Some(TypedFn {
        receiver: None,
        params,
        ret: d.ret.clone(),
        body,
    }))
}

pub(crate) fn check_params_with_defaults(
    ast_params: &[ast::Param],
    decl_params: &[DeclParam],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedParam>, SemaError> {
    let mut out = Vec::with_capacity(decl_params.len());
    for (ap, dp) in ast_params.iter().zip(decl_params.iter()) {
        fctx.insert_local(dp.name.clone(), dp.ty.clone());
        let default = match &ap.default {
            Some(def) => Some(check_expr(def, Some(&dp.ty), fctx, mctx)?),
            None => None,
        };
        out.push(TypedParam {
            mode: dp.mode,
            name: dp.name.clone(),
            ty: dp.ty.clone(),
            default,
        });
    }
    Ok(out)
}

fn check_struct_bodies(
    s: &ast::StructItem,
    mctx: &ModuleCtx,
) -> Result<Option<TypedStruct>, SemaError> {
    if !s.generics.is_empty() {
        return Ok(None); // generic struct: item H's job, not checked here.
    }
    let info = mctx.structs.get(&s.name).expect("struct present in mctx");
    let self_ty = Type::Named(s.name.clone(), vec![]);
    Ok(Some(check_struct_members(info, self_ty, mctx)?))
}

/// `pub(crate)` (item H, generics.rs): the guts of `check_struct_bodies`,
/// pulled out to take a `StructInfo` (and its `self_ty`) directly instead
/// of looking either up by name in `mctx` — `mctx.structs` only ever
/// holds each struct's *declared* (unsubstituted) shape, never a
/// substituted instantiation's, so item H's own re-run needs to hand this
/// its already-substituted `StructInfo` straight through rather than
/// stashing it under a name this function would then have to re-look-up.
/// `check_struct_bodies` above is now just this with the ordinary
/// (non-generic, `self_ty = Type::Named(name, [])`) case wired in. Always
/// concrete (unlike `check_top_fn`): both callers only ever reach this
/// with a non-generic struct/instantiation, so it always returns a real
/// `TypedStruct` (plans/M3.md item A).
pub(crate) fn check_struct_members(
    info: &StructInfo,
    self_ty: Type,
    mctx: &ModuleCtx,
) -> Result<TypedStruct, SemaError> {
    let struct_name = match &self_ty {
        Type::Named(name, _) => name.clone(),
        other => unreachable!("check_struct_members: self_ty `{other:?}` is not Type::Named"),
    };
    let local_pools = local_pool_names(info);
    let mut fields = Vec::new();
    let mut field_defaults = BTreeMap::new();
    let mut methods = BTreeMap::new();
    let mut assoc_fns = BTreeMap::new();
    let mut init = None;
    for (am, dm) in info.members() {
        match (am, dm) {
            (Member::Field(af), DeclMember::Field(df)) => {
                fields.push(af.name.clone());
                if let Some(def) = &af.default {
                    let mut fctx = FnCtx::new(Type::Unit, local_pools.clone());
                    fctx.insert_local("self".to_string(), self_ty.clone());
                    let typed_def = check_expr(def, Some(&df.ty), &mut fctx, mctx)?;
                    field_defaults.insert(af.name.clone(), typed_def);
                }
            }
            (Member::Fn(f), DeclMember::Fn(fd)) => {
                if !f.generics.is_empty() {
                    continue; // generic method: item H's job.
                }
                let mut fctx = FnCtx::new(fd.ret.clone(), local_pools.clone());
                fctx.insert_local("self".to_string(), self_ty.clone());
                let params = check_params_with_defaults(&f.params, &fd.params, &mut fctx, mctx)?;
                let body = match &f.body {
                    Some(body) => check_stmts(body, &mut fctx, mctx)?,
                    // Same fail-closed rule as top-level fns: the
                    // bodyless shorthand is doc-table syntax, not a
                    // checked declaration (this shape also panicked
                    // access.rs's effect inference before b78b95e — the
                    // golden err-unimplemented-bodyless pins it).
                    None => return Err(unimplemented_at("bodyless functions are", f.span)),
                };
                let receiver = f.receiver.as_ref().map(|r| (r.mode, self_ty.clone()));
                let tf = TypedFn {
                    receiver,
                    params,
                    ret: fd.ret.clone(),
                    body,
                };
                if f.receiver.is_some() {
                    methods.insert(f.name.clone(), tf);
                } else {
                    assoc_fns.insert(f.name.clone(), tf);
                }
            }
            (Member::Init(i), DeclMember::Init(fd)) => {
                let mut fctx = FnCtx::new(fd.ret.clone(), local_pools.clone());
                fctx.insert_local("self".to_string(), self_ty.clone());
                let params = check_params_with_defaults(&i.params, &fd.params, &mut fctx, mctx)?;
                let body = check_stmts(&i.body, &mut fctx, mctx)?;
                init = Some(TypedFn {
                    receiver: Some((i.receiver.mode, self_ty.clone())),
                    params,
                    ret: fd.ret.clone(),
                    body,
                });
            }
            _ => {}
        }
    }
    Ok(TypedStruct {
        name: struct_name,
        fields,
        field_defaults,
        methods,
        assoc_fns,
        init,
    })
}

// --- statements --------------------------------------------------------

fn check_stmts(
    stmts: &[Stmt],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedStmt>, SemaError> {
    let mut out = Vec::with_capacity(stmts.len());
    for s in stmts {
        out.push(check_stmt(s, fctx, mctx)?);
    }
    Ok(out)
}

fn check_stmt(stmt: &Stmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    match stmt {
        Stmt::Assign(a) => check_assign(a, fctx, mctx),
        Stmt::If(i) => check_if(i, fctx, mctx),
        Stmt::Match(m) => check_match(m, fctx, mctx),
        Stmt::For(f) => check_for(f, fctx, mctx),
        Stmt::While(w) => check_while(w, fctx, mctx),
        Stmt::Break(_) => Ok(TypedStmt {
            kind: TypedStmtKind::Break,
        }),
        Stmt::Continue(_) => Ok(TypedStmt {
            kind: TypedStmtKind::Continue,
        }),
        Stmt::Pass(_) => Ok(TypedStmt {
            kind: TypedStmtKind::Pass,
        }),
        Stmt::Return(span, e) => check_return(*span, e, fctx, mctx),
        Stmt::Assert(a) => check_assert(a, fctx, mctx),
        Stmt::Defer(d) => check_defer(d, fctx, mctx),
        Stmt::With(w) => Err(unimplemented_at("`with` is", w.span)),
        Stmt::Send(span, _e) => Err(unimplemented_at("`send` is", *span)),
        Stmt::Expr(_span, e) => Ok(TypedStmt {
            kind: TypedStmtKind::ExprStmt(check_expr(e, None, fctx, mctx)?),
        }),
        // plans/M3.md item D: `sema::specialize` runs before this pass
        // (`mod.rs::check_typed`) and eliminates every `comptime if`
        // node from the tree it hands to `collect`/`resolve`/`declare`/
        // `bodies` — the selected branch's statements are spliced in
        // directly, so the graph this pass ever sees already IS the
        // specialized graph (decision 8). Reaching this arm would mean
        // `specialize` left one behind (a producer bug); it stays fail-
        // closed as a defense-in-depth net, not because it is expected
        // to fire.
        Stmt::ComptimeIf(c) => Err(unimplemented_at("`comptime if` is", c.span)),
        Stmt::ComptimeAssert(span, cond, message) => {
            check_comptime_assert(*span, cond, message, fctx, mctx)
        }
    }
}

/// `comptime assert` (plans/M3.md item D, decision 8): typed exactly
/// like a plain `assert` (`check_assert` above) — condition typed as
/// `bool`, message required to be a text literal — except the result
/// carries the statement's own `span` (`TypedStmtKind::ComptimeAssert`'s
/// own doc comment explains why) and is never evaluated here: evaluation
/// is `eval::check_comptime_asserts`'s job, once the whole program is
/// assembled, unconditionally (independent of whether anything calls the
/// fn/method this statement lives in) — decision 8's "evaluates after
/// typing."
fn check_comptime_assert(
    span: Span,
    cond: &Expr,
    message: &Option<Expr>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    let cond = check_expr(cond, Some(&Type::Bool), fctx, mctx)?;
    let message = match message {
        Some(msg) => match msg {
            Expr::Str(..) => Some(check_expr(msg, None, fctx, mctx)?),
            Expr::FStr(_) => return Err(unimplemented_at("f-strings are", msg.span())),
            other => {
                return Err(type_error(
                    "comptime assert message must be a text literal".to_string(),
                    other.span(),
                ));
            }
        },
        None => None,
    };
    Ok(TypedStmt {
        kind: TypedStmtKind::ComptimeAssert {
            span,
            cond,
            message,
        },
    })
}

fn check_if(i: &IfStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let cond = check_expr(&i.cond, Some(&Type::Bool), fctx, mctx)?;
    let then_branch = check_stmts(&i.then_branch, fctx, mctx)?;
    let mut elifs = Vec::with_capacity(i.elifs.len());
    for elif in &i.elifs {
        let ec = check_expr(&elif.cond, Some(&Type::Bool), fctx, mctx)?;
        let eb = check_stmts(&elif.body, fctx, mctx)?;
        elifs.push(TypedElif { cond: ec, body: eb });
    }
    let else_branch = match &i.else_branch {
        Some(b) => Some(check_stmts(b, fctx, mctx)?),
        None => None,
    };
    Ok(TypedStmt {
        kind: TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        },
    })
}

fn check_while(w: &WhileStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let cond = check_expr(&w.cond, Some(&Type::Bool), fctx, mctx)?;
    let body = check_stmts(&w.body, fctx, mctx)?;
    Ok(TypedStmt {
        kind: TypedStmtKind::While { cond, body },
    })
}

fn check_match(m: &MatchStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let scrutinee = check_expr(&m.scrutinee, None, fctx, mctx)?;
    let sty = scrutinee.ty.clone();
    let mut arms = Vec::with_capacity(m.arms.len());
    for arm in &m.arms {
        let pattern = check_pattern(&arm.pattern, &sty, fctx, mctx)?;
        let guard = match &arm.guard {
            Some(g) => Some(check_expr(g, Some(&Type::Bool), fctx, mctx)?),
            None => None,
        };
        let body = check_stmts(&arm.body, fctx, mctx)?;
        arms.push(TypedMatchArm {
            pattern,
            guard,
            body,
        });
    }
    Ok(TypedStmt {
        kind: TypedStmtKind::Match { scrutinee, arms },
    })
}

fn check_return(
    span: Span,
    e: &Option<Expr>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    match e {
        Some(expr) => {
            let ret_ty = fctx.ret_ty.clone();
            let te = check_expr(expr, Some(&ret_ty), fctx, mctx)?;
            Ok(TypedStmt {
                kind: TypedStmtKind::Return(Some(te)),
            })
        }
        None => {
            if !types_eq(&fctx.ret_ty, &Type::Unit) {
                return Err(type_error(
                    format!(
                        "expected a return value of type `{}`",
                        types::render_type(&fctx.ret_ty)
                    ),
                    span,
                ));
            }
            Ok(TypedStmt {
                kind: TypedStmtKind::Return(None),
            })
        }
    }
}

fn check_assert(
    a: &AssertStmt,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    let cond = check_expr(&a.cond, Some(&Type::Bool), fctx, mctx)?;
    let message = match &a.message {
        Some(msg) => match msg {
            Expr::Str(..) => Some(check_expr(msg, None, fctx, mctx)?),
            Expr::FStr(_) => return Err(unimplemented_at("f-strings are", msg.span())),
            other => {
                return Err(type_error(
                    "assert message must be a text literal".to_string(),
                    other.span(),
                ));
            }
        },
        None => None,
    };
    Ok(TypedStmt {
        kind: TypedStmtKind::Assert { cond, message },
    })
}

fn check_for(f: &ForStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let raw_iterable: &Expr = match &f.iterable {
        Expr::Unary(_, UnaryOp::Take, inner) => inner.as_ref(),
        other => other,
    };
    let (elem_ty, iter) = match raw_iterable {
        Expr::Range(rspan, from, to, incl) => {
            let (ft, tt) = check_same_type_operands(from, to, fctx, mctx)?;
            if !is_integer_scalar(&ft.ty) {
                return Err(type_error(
                    format!(
                        "range endpoints must be an integer type, found `{}`",
                        types::render_type(&ft.ty)
                    ),
                    *rspan,
                ));
            }
            let ety = ft.ty.clone();
            (ety, TypedForIter::Range(ft, tt, *incl))
        }
        other => {
            let te = check_expr(other, None, fctx, mctx)?;
            match &te.ty {
                Type::Array(elem, _) => {
                    let ety = (**elem).clone();
                    (ety, TypedForIter::Expr(te))
                }
                _ => {
                    return Err(type_error(
                        format!(
                            "`for` requires a range or fixed array, found `{}`",
                            types::render_type(&te.ty)
                        ),
                        other.span(),
                    ));
                }
            }
        }
    };
    bind_local(fctx, &f.name, elem_ty.clone(), f.span)?;
    let body = check_stmts(&f.body, fctx, mctx)?;
    Ok(TypedStmt {
        kind: TypedStmtKind::For {
            name: f.name.clone(),
            elem_ty,
            take_binding: f.take_binding,
            iter,
            body,
        },
    })
}

fn check_defer(d: &DeferStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    if let Some((what, span)) = scan_defer_forbidden(&d.body) {
        return Err(type_error(format!("defer body cannot {what}"), span));
    }
    let body = match &d.body {
        DeferBody::Expr(e) => TypedDeferBody::Expr(Box::new(check_expr(e, None, fctx, mctx)?)),
        DeferBody::Suite(stmts) => TypedDeferBody::Suite(check_stmts(stmts, fctx, mctx)?),
    };
    Ok(TypedStmt {
        kind: TypedStmtKind::Defer(body),
    })
}

fn check_assign(
    a: &AssignStmt,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    if matches!(a.value, Expr::Closure(_)) {
        return Err(type_error("closures cannot be stored".to_string(), a.span));
    }
    if let Expr::Name(_, name) = &a.target {
        if fctx.lookup_innermost(name).is_some() {
            let target_t = check_expr(&a.target, None, fctx, mctx)?;
            let value_t = if a.op == AssignOp::Assign {
                check_expr(&a.value, Some(&target_t.ty), fctx, mctx)?
            } else {
                check_compound_assign(a.op, &target_t, &a.value, a.span, fctx, mctx)?
            };
            return Ok(TypedStmt {
                kind: TypedStmtKind::Assign {
                    target: target_t,
                    value: value_t,
                },
            });
        }
        if a.op != AssignOp::Assign {
            return Err(type_error(
                "compound assignment requires an existing local".to_string(),
                a.span,
            ));
        }
        let (ty, value_t) = match &a.ty {
            Some(ann) => {
                let resolved = mctx.resolve_type(ann, &fctx.local_pools)?;
                let vt = check_expr(&a.value, Some(&resolved), fctx, mctx)?;
                (resolved, vt)
            }
            None => {
                let vt = check_expr(&a.value, None, fctx, mctx)?;
                let t = vt.ty.clone();
                (t, vt)
            }
        };
        bind_local(fctx, name, ty.clone(), a.span)?;
        return Ok(TypedStmt {
            kind: TypedStmtKind::Let {
                name: name.clone(),
                ty,
                value: value_t,
            },
        });
    }
    // A non-name target (field, index) already exists; its type comes
    // from evaluating the place itself.
    let target_t = check_expr(&a.target, None, fctx, mctx)?;
    let value_t = if a.op == AssignOp::Assign {
        check_expr(&a.value, Some(&target_t.ty), fctx, mctx)?
    } else {
        check_compound_assign(a.op, &target_t, &a.value, a.span, fctx, mctx)?
    };
    Ok(TypedStmt {
        kind: TypedStmtKind::Assign {
            target: target_t,
            value: value_t,
        },
    })
}

/// `a += b` desugars to `a = a.add(b)` (02-language.md §7.4): compute
/// `b`'s type checked against `a`'s current type (same-type operand
/// rule), run the same operator-resolution logic binary expressions use,
/// and require the result still fit back into `a`'s type (true
/// automatically for every builtin scalar op; for a user-type operator
/// method it holds exactly when the method's declared return type is the
/// operand type, the 05§8 shape). The returned `TypedExpr` is the fully
/// desugared `target op value` computation — the typed tree's own
/// `Assign` node has no separate "compound" shape (`typed.rs`'s own doc
/// comment).
fn check_compound_assign(
    op: AssignOp,
    target: &TypedExpr,
    value: &Expr,
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let binop = match op {
        AssignOp::Add => BinOp::Add,
        AssignOp::Sub => BinOp::Sub,
        AssignOp::Mul => BinOp::Mul,
        AssignOp::Div => BinOp::Div,
        AssignOp::Rem => BinOp::Rem,
        AssignOp::BitAnd => BinOp::BitAnd,
        AssignOp::BitOr => BinOp::BitOr,
        AssignOp::BitXor => BinOp::BitXor,
        AssignOp::Shl => BinOp::Shl,
        AssignOp::Shr => BinOp::Shr,
        AssignOp::Assign => unreachable!("Assign never reaches check_compound_assign"),
    };
    let value_t = check_expr(value, Some(&target.ty), fctx, mctx)?;
    let result = build_binop_expr(binop, target.clone(), value_t, span, mctx)?;
    if !types_eq(&result.ty, &target.ty) {
        return Err(type_error(
            format!(
                "`{}` would change the type of the target from `{}` to `{}`",
                op.as_str(),
                types::render_type(&target.ty),
                types::render_type(&result.ty)
            ),
            span,
        ));
    }
    Ok(result)
}

// --- patterns (02-language.md §7.2) --------------------------------------

/// Widened to `pub(crate)` (item G, matches.rs): the exhaustiveness pass
/// reuses this verbatim to bind a match arm's/`is`'s pattern names into
/// the re-walked `FnCtx` exactly as this pass does, rather than
/// reimplementing pattern-binding. Its `Ok` payload (plans/M3.md item A)
/// is the typed pattern; every existing caller outside this file
/// discards it via `?;` unbound, so nothing there changes.
pub(crate) fn check_pattern(
    p: &Pattern,
    scrutinee: &Type,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedPattern, SemaError> {
    match p {
        Pattern::Wildcard(_) => Ok(TypedPattern {
            ty: scrutinee.clone(),
            kind: TypedPatternKind::Wildcard,
        }),
        Pattern::Literal(_span, expr) => {
            let te = check_expr(expr, Some(scrutinee), fctx, mctx)?;
            Ok(TypedPattern {
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Literal(Box::new(te)),
            })
        }
        Pattern::Binding(span, name) => {
            bind_local(fctx, name, scrutinee.clone(), *span)?;
            Ok(TypedPattern {
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Binding(name.clone()),
            })
        }
        Pattern::Take(_span, inner) => {
            let tp = check_pattern(inner, scrutinee, fctx, mctx)?;
            Ok(TypedPattern {
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Take(Box::new(tp)),
            })
        }
        Pattern::Variant {
            span,
            enum_name,
            variant,
            payload,
        } => {
            let payload_types =
                variant_payload_types_for(scrutinee, enum_name.as_deref(), variant, *span, mctx)?;
            if payload.len() != payload_types.len() {
                return Err(type_error(
                    format!(
                        "variant `{variant}` expects {} payload element(s), found {}",
                        payload_types.len(),
                        payload.len()
                    ),
                    *span,
                ));
            }
            let mut typed_payload = Vec::with_capacity(payload.len());
            for (sp, ty) in payload.iter().zip(payload_types.iter()) {
                typed_payload.push(check_pattern(sp, ty, fctx, mctx)?);
            }
            Ok(TypedPattern {
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Variant {
                    enum_name: resolved_enum_name(scrutinee),
                    variant: variant.clone(),
                    payload: typed_payload,
                },
            })
        }
        Pattern::Tuple(span, items) => {
            let Type::Tuple(elems) = scrutinee else {
                return Err(type_error(
                    format!(
                        "expected a tuple pattern for type `{}`",
                        types::render_type(scrutinee)
                    ),
                    *span,
                ));
            };
            if items.len() != elems.len() {
                return Err(type_error(
                    format!(
                        "tuple pattern expects {} element(s), found {}",
                        elems.len(),
                        items.len()
                    ),
                    *span,
                ));
            }
            let mut typed_items = Vec::with_capacity(items.len());
            for (sp, ty) in items.iter().zip(elems.iter()) {
                typed_items.push(check_pattern(sp, ty, fctx, mctx)?);
            }
            Ok(TypedPattern {
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Tuple(typed_items),
            })
        }
        Pattern::Array(span, items) => {
            let Type::Array(elem, len_expr) = scrutinee else {
                return Err(type_error(
                    format!(
                        "expected an array pattern for type `{}`",
                        types::render_type(scrutinee)
                    ),
                    *span,
                ));
            };
            if let Some(n) = literal_array_len(len_expr) {
                if n != items.len() as i128 {
                    return Err(type_error(
                        format!(
                            "array pattern expects {n} element(s), found {}",
                            items.len()
                        ),
                        *span,
                    ));
                }
            }
            let mut typed_items = Vec::with_capacity(items.len());
            for sp in items {
                typed_items.push(check_pattern(sp, elem, fctx, mctx)?);
            }
            Ok(TypedPattern {
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Array(typed_items),
            })
        }
        Pattern::Or(_span, alts) => {
            // Same-bindings-same-types across alternatives is item G's
            // job (exhaustiveness); each alternative is independently
            // well-formed against the scrutinee here.
            let mut typed_alts = Vec::with_capacity(alts.len());
            for alt in alts {
                typed_alts.push(check_pattern(alt, scrutinee, fctx, mctx)?);
            }
            Ok(TypedPattern {
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Or(typed_alts),
            })
        }
    }
}

/// Resolves a scrutinee's own enum name for the typed tree (`typed.rs`'s
/// `EnumConstruct`/`Variant` payload): `"Option"`/`"Result"` for the two
/// builtin sums, else a user enum's bare name. Only ever called after
/// `variant_payload_types_for` has already restricted `ty` to one of
/// these three shapes (or returned an error), so the fallthrough is
/// unreachable, not a fail-closed case.
fn resolved_enum_name(ty: &Type) -> String {
    match ty {
        Type::Option(_) => "Option".to_string(),
        Type::Result(_, _) => "Result".to_string(),
        Type::Named(name, _) => name.clone(),
        other => unreachable!(
            "resolved_enum_name: `{}` is not an enum-shaped type",
            types::render_type(other)
        ),
    }
}

/// Widened to `pub(crate)` (item G, matches.rs): the exhaustiveness pass
/// needs the same literal-length reading to decide whether a fixed array
/// type is component-wise checkable (plans/M2.md item G) or must fall
/// back to "unbounded, needs a wildcard" like an integer/`char`/string.
pub(crate) fn literal_array_len(e: &Expr) -> Option<i128> {
    match e {
        Expr::Int(_, text) => parse_int_literal(text),
        _ => None, // needs comptime evaluation; skip the arity check rather than fail closed.
    }
}

/// Resolves a pattern's (or a leading-dot expression's) variant payload
/// types against the scrutinee/expected type: `Option`/`Result` are
/// builtin sums handled directly (their variants never route through
/// `mctx.enums`); a user enum's variants come from `mctx`. Anything else
/// cannot carry a variant pattern/construction.
fn variant_payload_types_for(
    scrutinee: &Type,
    enum_name: Option<&str>,
    variant: &str,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<Vec<Type>, SemaError> {
    match scrutinee {
        Type::Option(inner) => {
            if let Some(n) = enum_name {
                if n != "Option" {
                    return Err(type_error(
                        format!("expected an `Option` pattern, found `{n}`"),
                        span,
                    ));
                }
            }
            match variant {
                "Some" => Ok(vec![(**inner).clone()]),
                "None" => Ok(vec![]),
                other => Err(type_error(
                    format!("`Option` has no variant `{other}`"),
                    span,
                )),
            }
        }
        Type::Result(ok, err) => {
            if let Some(n) = enum_name {
                if n != "Result" {
                    return Err(type_error(
                        format!("expected a `Result` pattern, found `{n}`"),
                        span,
                    ));
                }
            }
            match variant {
                "Ok" => Ok(vec![(**ok).clone()]),
                "Err" => Ok(vec![(**err).clone()]),
                other => Err(type_error(
                    format!("`Result` has no variant `{other}`"),
                    span,
                )),
            }
        }
        Type::Named(name, targs) => {
            if let Some(n) = enum_name {
                if n != name {
                    return Err(type_error(
                        format!("expected a `{name}` pattern, found `{n}`"),
                        span,
                    ));
                }
            }
            // A generic enum instantiation (item H): substitute + enqueue
            // it, then read the (now concrete) variant off the
            // substituted declaration instead of the declared one.
            let e = if targs.is_empty() {
                match mctx.enums.get(name) {
                    Some(e) => e.clone(),
                    None => return Err(type_error(format!("`{name}` is not an enum"), span)),
                }
            } else {
                generics::instantiate_enum(mctx, name, targs, span)?
            };
            let Some(dv) = e.variants.iter().find(|v| v.name == variant) else {
                return Err(type_error(
                    format!("enum `{name}` has no variant `{variant}`"),
                    span,
                ));
            };
            Ok(decl_variant_payload_types(dv))
        }
        other => Err(type_error(
            format!(
                "cannot match a variant pattern against type `{}`",
                types::render_type(other)
            ),
            span,
        )),
    }
}

/// Widened to `pub(crate)` (item G, matches.rs): the exhaustiveness pass
/// needs a closed enum's own variant payload types (declaration order) to
/// build its constructor matrix — the same mapping `bodies.rs` already
/// uses for pattern typing and `?`'s `From` conversion.
pub(crate) fn decl_variant_payload_types(dv: &types::DeclVariant) -> Vec<Type> {
    match &dv.payload {
        DeclVariantPayload::None => vec![],
        DeclVariantPayload::Tuple(types_) => types_.clone(),
        DeclVariantPayload::Named(fields) => fields.iter().map(|(_, t)| t.clone()).collect(),
    }
}

// --- expressions: the central check/synth pair ---------------------------

/// Checks `expr` against `expected` (decision 4): synthesizes its type
/// (`synth_expr`, which uses `expected` internally wherever the grammar
/// needs it — literal defaulting, closures, `Some`/`Ok`/`Err`/leading-dot
/// construction, array/tuple literals), then gates the result against
/// `expected` when one was supplied. Always returns the typed node (which
/// embeds the actual type, plans/M3.md item A), so callers that need just
/// the type (call-argument checking, `for`'s range endpoints, ...) read
/// `.ty` off it.
/// Widened to `pub(crate)` (item G, matches.rs): this is the one function
/// that pass reuses to synthesize an expression's type in a local
/// context — the "dumbest workable route" plans/M2.md item G calls for,
/// rather than reimplementing expression typing to find a `match`/`is`
/// scrutinee's type.
pub(crate) fn check_expr(
    expr: &Expr,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let actual = synth_expr(expr, expected, fctx, mctx)?;
    if let Some(exp) = expected {
        if !types_eq(&actual.ty, exp) {
            return Err(type_error(
                format!(
                    "expected `{}`, found `{}`",
                    types::render_type(exp),
                    types::render_type(&actual.ty)
                ),
                expr.span(),
            ));
        }
    }
    Ok(actual)
}

fn synth_expr(
    expr: &Expr,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    match expr {
        Expr::Int(span, text) => synth_int_literal(*span, text, expected),
        Expr::Float(span, text) => synth_float_literal(*span, text, expected),
        Expr::Str(_span, text) => Ok(TypedExpr {
            ty: Type::Static(Box::new(Type::Str)),
            kind: TypedExprKind::Str(text.clone()),
        }),
        Expr::BStr(span, text) => {
            let len = bstr_byte_len(text);
            let ty = Type::Static(Box::new(Type::Bytes(Some(Box::new(Expr::Int(
                *span,
                len.to_string(),
            ))))));
            Ok(TypedExpr {
                ty,
                kind: TypedExprKind::BStr(text.clone()),
            })
        }
        Expr::Char(_span, text) => Ok(TypedExpr {
            ty: Type::Char,
            kind: TypedExprKind::Char(text.clone()),
        }),
        Expr::FStr(f) => Err(unimplemented_at("f-strings are", f.span)),
        Expr::Bool(_span, v) => Ok(TypedExpr {
            ty: Type::Bool,
            kind: TypedExprKind::Bool(*v),
        }),
        Expr::Unit(_span) => Ok(TypedExpr {
            ty: Type::Unit,
            kind: TypedExprKind::Unit,
        }),
        Expr::Name(span, name) => synth_name(*span, name, expected, fctx, mctx),
        Expr::Field(base, span, name) => check_field_expr(base, *span, name, fctx, mctx),
        Expr::Index(base, span, args) => synth_index(base, *span, args, fctx, mctx),
        Expr::Call(callee, span, args) => check_call(callee, *span, args, expected, fctx, mctx),
        Expr::Unary(span, UnaryOp::Neg, inner) => {
            check_unary_neg(inner, expected, *span, fctx, mctx)
        }
        Expr::Unary(span, UnaryOp::BitNot, inner) => {
            let it = check_expr(inner, expected, fctx, mctx)?;
            if !is_integer_scalar(&it.ty) {
                return Err(type_error(
                    format!(
                        "`~` requires an integer type, found `{}`",
                        types::render_type(&it.ty)
                    ),
                    *span,
                ));
            }
            let ty = it.ty.clone();
            Ok(TypedExpr {
                ty,
                kind: TypedExprKind::BitNot(Box::new(it)),
            })
        }
        Expr::Unary(span, UnaryOp::Await, _inner) => Err(unimplemented_at("await is", *span)),
        Expr::Unary(_span, UnaryOp::Take, inner) => {
            let it = check_expr(inner, expected, fctx, mctx)?;
            let ty = it.ty.clone();
            Ok(TypedExpr {
                ty,
                kind: TypedExprKind::Take(Box::new(it)),
            })
        }
        Expr::Try(span, inner) => check_try(*span, inner, fctx, mctx),
        Expr::Binary(span, op, l, r) => check_binary(*op, l, r, *span, fctx, mctx),
        Expr::Range(span, _from, _to, _incl) => Err(type_error(
            "a range is only a value directly inside `for`".to_string(),
            *span,
        )),
        Expr::Is(_span, scrutinee, pattern) => {
            let st = check_expr(scrutinee, None, fctx, mctx)?;
            let sty = st.ty.clone();
            let pt = check_pattern(pattern, &sty, fctx, mctx)?;
            Ok(TypedExpr {
                ty: Type::Bool,
                kind: TypedExprKind::Is(Box::new(st), Box::new(pt)),
            })
        }
        Expr::Not(_span, inner) => {
            let it = check_expr(inner, Some(&Type::Bool), fctx, mctx)?;
            Ok(TypedExpr {
                ty: Type::Bool,
                kind: TypedExprKind::Not(Box::new(it)),
            })
        }
        Expr::And(_span, l, r) => {
            let lt = check_expr(l, Some(&Type::Bool), fctx, mctx)?;
            let rt = check_expr(r, Some(&Type::Bool), fctx, mctx)?;
            Ok(TypedExpr {
                ty: Type::Bool,
                kind: TypedExprKind::And(Box::new(lt), Box::new(rt)),
            })
        }
        Expr::Or(_span, l, r) => {
            let lt = check_expr(l, Some(&Type::Bool), fctx, mctx)?;
            let rt = check_expr(r, Some(&Type::Bool), fctx, mctx)?;
            Ok(TypedExpr {
                ty: Type::Bool,
                kind: TypedExprKind::Or(Box::new(lt), Box::new(rt)),
            })
        }
        Expr::DotVariant(span, name, args) => {
            let Some(exp) = expected else {
                return Err(type_error(
                    format!("cannot infer an enum type for `.{name}`"),
                    *span,
                ));
            };
            let exp = exp.clone();
            let payload_types = variant_payload_types_for(&exp, None, name, *span, mctx)?;
            let typed_args = check_variant_args(&payload_types, args, *span, fctx, mctx)?;
            let enum_name = resolved_enum_name(&exp);
            Ok(TypedExpr {
                ty: exp,
                kind: TypedExprKind::EnumConstruct {
                    enum_name,
                    variant: name.clone(),
                    args: typed_args,
                },
            })
        }
        Expr::Closure(c) => check_closure(c, expected, fctx, mctx),
        Expr::Send(span, _inner) => Err(unimplemented_at("send is", *span)),
        Expr::Tuple(span, items) => synth_tuple(*span, items, expected, fctx, mctx),
        Expr::List(span, items) => synth_list(*span, items, expected, fctx, mctx),
    }
}

fn synth_int_literal(
    span: Span,
    text: &str,
    expected: Option<&Type>,
) -> Result<TypedExpr, SemaError> {
    let value = parse_int_literal(text)
        .ok_or_else(|| type_error("invalid integer literal".to_string(), span))?;
    let ty = match expected {
        Some(t) if is_integer_scalar(t) => {
            check_int_range(value, t, span)?;
            t.clone()
        }
        Some(t) => {
            return Err(type_error(
                format!(
                    "expected `{}`, found an integer literal",
                    types::render_type(t)
                ),
                span,
            ));
        }
        None => {
            if value <= i64::MAX as i128 {
                Type::I64
            } else if value <= u64::MAX as i128 {
                Type::U64
            } else {
                return Err(type_error("integer literal out of range".to_string(), span));
            }
        }
    };
    Ok(TypedExpr {
        ty,
        kind: TypedExprKind::Int(text.to_string()),
    })
}

fn synth_float_literal(
    span: Span,
    text: &str,
    expected: Option<&Type>,
) -> Result<TypedExpr, SemaError> {
    let ty = match expected {
        Some(t) if is_float_scalar(t) => t.clone(),
        Some(t) => {
            return Err(type_error(
                format!(
                    "expected `{}`, found a float literal",
                    types::render_type(t)
                ),
                span,
            ));
        }
        None => Type::F64,
    };
    Ok(TypedExpr {
        ty,
        kind: TypedExprKind::Float(text.to_string()),
    })
}

fn synth_name(
    span: Span,
    name: &str,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Some(ty) = fctx.lookup_local(name) {
        return Ok(TypedExpr {
            ty,
            kind: TypedExprKind::Local(name.to_string()),
        });
    }
    if let Some(ty) = mctx.consts.get(name) {
        return Ok(TypedExpr {
            ty: ty.clone(),
            kind: TypedExprKind::Const(name.to_string()),
        });
    }
    if let Some(f) = mctx.fns.get(name) {
        if !f.decl.generics.is_empty() {
            return Err(unimplemented_at("generic instantiation is", span));
        }
        return Ok(TypedExpr {
            ty: fn_value_type(&f.decl),
            kind: TypedExprKind::FnRef(CalleeKey::Fn(name.to_string())),
        });
    }
    if mctx.structs.contains_key(name) || mctx.enums.contains_key(name) {
        return Err(type_error(format!("`{name}` is a type, not a value"), span));
    }
    match name {
        "None" => match expected {
            Some(t @ Type::Option(_)) => Ok(TypedExpr {
                ty: t.clone(),
                kind: TypedExprKind::EnumConstruct {
                    enum_name: "Option".to_string(),
                    variant: "None".to_string(),
                    args: vec![],
                },
            }),
            _ => Err(type_error(
                "cannot infer the type of `None` without context".to_string(),
                span,
            )),
        },
        "Some" | "Ok" | "Err" | "panic" => Err(type_error(
            format!("`{name}` cannot be used without being called"),
            span,
        )),
        _ => Err(type_error(
            format!("cannot determine the type of `{name}`"),
            span,
        )),
    }
}

pub(crate) fn fn_value_type(d: &types::DeclFn) -> Type {
    let params = d.params.iter().map(|p| (p.mode, p.ty.clone())).collect();
    Type::Fn(params, Box::new(d.ret.clone()))
}

pub(crate) fn unwrap_own(ty: Type) -> Type {
    match ty {
        Type::Own(_, inner) => *inner,
        other => other,
    }
}

fn check_field_expr(
    base: &Expr,
    span: Span,
    name: &str,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Expr::Name(_, bname) = base {
        if fctx.lookup_local(bname).is_none() {
            if let Some(s) = mctx.structs.get(bname.as_str()) {
                if !s.decl.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", span));
                }
                if let Some((_, d)) = s.assoc_fn(name) {
                    let key = CalleeKey::Method(bname.clone(), name.to_string());
                    return Ok(TypedExpr {
                        ty: fn_value_type(d),
                        kind: TypedExprKind::FnRef(key),
                    });
                }
                if s.has_member_named(name) {
                    return Err(type_error(
                        format!("cannot reference method `{name}` without calling it"),
                        span,
                    ));
                }
                return Err(type_error(
                    format!("type `{bname}` has no associated function `{name}`"),
                    span,
                ));
            }
            if let Some(e) = mctx.enums.get(bname.as_str()) {
                if !e.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", span));
                }
                if let Some(dv) = e.variants.iter().find(|v| v.name == name) {
                    if matches!(dv.payload, DeclVariantPayload::None) {
                        return Ok(TypedExpr {
                            ty: Type::Named(e.name.clone(), vec![]),
                            kind: TypedExprKind::EnumConstruct {
                                enum_name: e.name.clone(),
                                variant: name.to_string(),
                                args: vec![],
                            },
                        });
                    }
                    return Err(type_error(
                        format!("variant `{name}` requires a payload"),
                        span,
                    ));
                }
                return Err(type_error(
                    format!("enum `{bname}` has no variant `{name}`"),
                    span,
                ));
            }
        }
    }
    let base_t = check_expr(base, None, fctx, mctx)?;
    let base_ty = unwrap_own(base_t.ty.clone());
    match &base_ty {
        Type::Named(sname, targs) => {
            // A generic instantiation's field (item H): substitute +
            // enqueue it, then read the field's (now concrete) type off
            // the substituted declaration instead of the declared one.
            let s = if targs.is_empty() {
                match mctx.structs.get(sname.as_str()) {
                    Some(s) => std::borrow::Cow::Borrowed(s),
                    None => {
                        return Err(type_error(
                            format!("type `{sname}` has no field `{name}`"),
                            span,
                        ));
                    }
                }
            } else {
                std::borrow::Cow::Owned(generics::instantiate_struct(mctx, sname, targs, span)?)
            };
            if let Some(ty) = s.field_ty(name) {
                return Ok(TypedExpr {
                    ty,
                    kind: TypedExprKind::Field(Box::new(base_t), name.to_string()),
                });
            }
            if s.has_member_named(name) {
                return Err(type_error(
                    format!("cannot reference method `{name}` without calling it"),
                    span,
                ));
            }
            Err(type_error(
                format!("type `{sname}` has no field `{name}`"),
                span,
            ))
        }
        other => Err(type_error(
            format!("type `{}` has no field `{name}`", types::render_type(other)),
            span,
        )),
    }
}

fn synth_index(
    base: &Expr,
    span: Span,
    args: &[Expr],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Expr::Name(_, n) = base {
        if mctx.structs.contains_key(n) || mctx.enums.contains_key(n) || mctx.fns.contains_key(n) {
            return Err(unimplemented_at("generic instantiation is", span));
        }
    }
    let base_t = check_expr(base, None, fctx, mctx)?;
    let base_ty = unwrap_own(base_t.ty.clone());
    if args.len() != 1 {
        return Err(type_error(
            format!("indexing takes exactly one argument, found {}", args.len()),
            span,
        ));
    }
    match &base_ty {
        Type::Array(elem, _) => {
            let idx_t = check_expr(&args[0], Some(&Type::Usize), fctx, mctx)?;
            Ok(TypedExpr {
                ty: (**elem).clone(),
                kind: TypedExprKind::Index(Box::new(base_t), Box::new(idx_t)),
            })
        }
        Type::Bytes(_) => {
            let idx_t = check_expr(&args[0], Some(&Type::Usize), fctx, mctx)?;
            Ok(TypedExpr {
                ty: Type::U8,
                kind: TypedExprKind::Index(Box::new(base_t), Box::new(idx_t)),
            })
        }
        other => Err(type_error(
            format!("type `{}` cannot be indexed", types::render_type(other)),
            span,
        )),
    }
}

fn synth_tuple(
    span: Span,
    items: &[Expr],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Some(Type::Tuple(exp_elems)) = expected {
        if exp_elems.len() != items.len() {
            return Err(type_error(
                format!(
                    "tuple expects {} element(s), found {}",
                    exp_elems.len(),
                    items.len()
                ),
                span,
            ));
        }
        let exp_elems = exp_elems.clone();
        let mut typed_items = Vec::with_capacity(items.len());
        for (item, ety) in items.iter().zip(exp_elems.iter()) {
            typed_items.push(check_expr(item, Some(ety), fctx, mctx)?);
        }
        return Ok(TypedExpr {
            ty: Type::Tuple(exp_elems),
            kind: TypedExprKind::Tuple(typed_items),
        });
    }
    let mut typed_items = Vec::with_capacity(items.len());
    for item in items {
        typed_items.push(check_expr(item, None, fctx, mctx)?);
    }
    let elems = typed_items.iter().map(|t| t.ty.clone()).collect();
    Ok(TypedExpr {
        ty: Type::Tuple(elems),
        kind: TypedExprKind::Tuple(typed_items),
    })
}

fn synth_list(
    span: Span,
    items: &[Expr],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Some(Type::Array(elem, len_expr)) = expected {
        let elem = (**elem).clone();
        let len_expr = len_expr.clone();
        if let Some(n) = literal_array_len(&len_expr) {
            if n != items.len() as i128 {
                return Err(type_error(
                    format!("array expects {n} element(s), found {}", items.len()),
                    span,
                ));
            }
        }
        let mut typed_items = Vec::with_capacity(items.len());
        for item in items {
            typed_items.push(check_expr(item, Some(&elem), fctx, mctx)?);
        }
        return Ok(TypedExpr {
            ty: Type::Array(Box::new(elem), len_expr),
            kind: TypedExprKind::List(typed_items),
        });
    }
    if items.is_empty() {
        return Err(type_error(
            "cannot infer the element type of an empty array literal".to_string(),
            span,
        ));
    }
    let first = check_expr(&items[0], None, fctx, mctx)?;
    let elem_ty = first.ty.clone();
    let mut typed_items = Vec::with_capacity(items.len());
    typed_items.push(first);
    for item in &items[1..] {
        typed_items.push(check_expr(item, Some(&elem_ty), fctx, mctx)?);
    }
    let len = Expr::Int(span, items.len().to_string());
    Ok(TypedExpr {
        ty: Type::Array(Box::new(elem_ty), Box::new(len)),
        kind: TypedExprKind::List(typed_items),
    })
}

// --- unary `-`, binary operators (02-language.md §7.4, §8.2; 05-library.md §8) --

fn is_integer_scalar(t: &Type) -> bool {
    matches!(
        t,
        Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::I64
            | Type::Isize
    )
}

fn is_signed_scalar(t: &Type) -> bool {
    matches!(
        t,
        Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Isize
    )
}

fn is_float_scalar(t: &Type) -> bool {
    matches!(t, Type::F32 | Type::F64)
}

fn is_numeric_scalar(t: &Type) -> bool {
    is_integer_scalar(t) || is_float_scalar(t)
}

/// Structural type equality, used everywhere in place of derived
/// `PartialEq` on `Type`: `Type::Array`/`Type::Bytes` embed their length
/// as an unevaluated `ast::Expr` (types.rs, item H evaluates the literal
/// subset), and `Expr`'s derived `PartialEq` also compares spans — so
/// the *same* `[T; 3]` written at two different source locations would
/// otherwise never compare equal. `same_len_expr` below compares length
/// expressions by value/name instead, ignoring span.
/// Widened to `pub(crate)` (item G, matches.rs): the `|` alternative
/// binding-consistency check (02-language.md §7.2: "same bindings, same
/// types") needs the same span-insensitive comparison bodies.rs uses
/// throughout, rather than the derived (span-sensitive) `PartialEq`.
pub(crate) fn types_eq(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Bool, Type::Bool)
        | (Type::U8, Type::U8)
        | (Type::U16, Type::U16)
        | (Type::U32, Type::U32)
        | (Type::U64, Type::U64)
        | (Type::Usize, Type::Usize)
        | (Type::I8, Type::I8)
        | (Type::I16, Type::I16)
        | (Type::I32, Type::I32)
        | (Type::I64, Type::I64)
        | (Type::Isize, Type::Isize)
        | (Type::F32, Type::F32)
        | (Type::F64, Type::F64)
        | (Type::Char, Type::Char)
        | (Type::Unit, Type::Unit)
        | (Type::Never, Type::Never)
        | (Type::Str, Type::Str) => true,
        (Type::Array(e1, l1), Type::Array(e2, l2)) => types_eq(e1, e2) && same_len_expr(l1, l2),
        (Type::Tuple(a1), Type::Tuple(a2)) => {
            a1.len() == a2.len() && a1.iter().zip(a2.iter()).all(|(x, y)| types_eq(x, y))
        }
        (Type::Option(x), Type::Option(y)) => types_eq(x, y),
        (Type::Result(a1, b1), Type::Result(a2, b2)) => types_eq(a1, a2) && types_eq(b1, b2),
        (Type::Own(p1, t1), Type::Own(p2, t2)) => p1 == p2 && types_eq(t1, t2),
        (Type::Static(x), Type::Static(y)) => types_eq(x, y),
        (Type::Bytes(None), Type::Bytes(None)) => true,
        (Type::Bytes(Some(l1)), Type::Bytes(Some(l2))) => same_len_expr(l1, l2),
        (Type::Fn(p1, r1), Type::Fn(p2, r2)) => {
            p1.len() == p2.len()
                && p1
                    .iter()
                    .zip(p2.iter())
                    .all(|((m1, t1), (m2, t2))| m1 == m2 && types_eq(t1, t2))
                && types_eq(r1, r2)
        }
        (Type::Generic(n1), Type::Generic(n2)) => n1 == n2,
        (Type::Named(n1, a1), Type::Named(n2, a2)) => {
            n1 == n2
                && a1.len() == a2.len()
                && a1.iter().zip(a2.iter()).all(|(x, y)| type_args_eq(x, y))
        }
        _ => false,
    }
}

fn type_args_eq(a: &types::TypeArg, b: &types::TypeArg) -> bool {
    match (a, b) {
        (types::TypeArg::Type(x), types::TypeArg::Type(y)) => types_eq(x, y),
        (types::TypeArg::Const(x), types::TypeArg::Const(y)) => same_len_expr(x, y),
        (types::TypeArg::Bound(x), types::TypeArg::Bound(y)) => same_len_expr(x, y),
        _ => false,
    }
}

/// Only the two shapes an M2 length/const argument actually takes — a
/// literal integer or a bare `const`/generic-param name — compare by
/// value; anything else is conservatively unequal (comparing two
/// arbitrary expressions honestly needs comptime evaluation, item M3).
fn same_len_expr(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Int(_, t1), Expr::Int(_, t2)) => parse_int_literal(t1) == parse_int_literal(t2),
        (Expr::Name(_, n1), Expr::Name(_, n2)) => n1 == n2,
        _ => false,
    }
}

fn int_bounds(ty: &Type) -> Option<(i128, i128)> {
    match ty {
        Type::U8 => Some((0, u8::MAX as i128)),
        Type::U16 => Some((0, u16::MAX as i128)),
        Type::U32 => Some((0, u32::MAX as i128)),
        Type::U64 | Type::Usize => Some((0, u64::MAX as i128)),
        Type::I8 => Some((i8::MIN as i128, i8::MAX as i128)),
        Type::I16 => Some((i16::MIN as i128, i16::MAX as i128)),
        Type::I32 => Some((i32::MIN as i128, i32::MAX as i128)),
        Type::I64 | Type::Isize => Some((i64::MIN as i128, i64::MAX as i128)),
        _ => None,
    }
}

fn check_int_range(value: i128, ty: &Type, span: Span) -> Result<(), SemaError> {
    let (min, max) = int_bounds(ty).expect("check_int_range called with a non-integer type");
    if value < min || value > max {
        return Err(type_error(
            format!(
                "integer literal out of range for `{}`",
                types::render_type(ty)
            ),
            span,
        ));
    }
    Ok(())
}

pub(crate) fn parse_int_literal(text: &str) -> Option<i128> {
    let cleaned: String = text.chars().filter(|c| *c != '_').collect();
    let (radix, digits): (u32, &str) = if let Some(d) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        (16, d)
    } else if let Some(d) = cleaned
        .strip_prefix("0o")
        .or_else(|| cleaned.strip_prefix("0O"))
    {
        (8, d)
    } else if let Some(d) = cleaned
        .strip_prefix("0b")
        .or_else(|| cleaned.strip_prefix("0B"))
    {
        (2, d)
    } else {
        (10, cleaned.as_str())
    };
    i128::from_str_radix(digits, radix).ok()
}

/// Decoded byte length of a byte-string literal's raw (still-escaped)
/// source text (lexer.rs: "contents kept raw"): each escape (already
/// validated at lex time — `\xNN`, or one of `\\ \" \' \n \r \t \0`)
/// contributes exactly one byte; anything else contributes its own
/// UTF-8 length.
fn bstr_byte_len(text: &str) -> u64 {
    let mut len = 0u64;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('x') => {
                    chars.next();
                    chars.next();
                    len += 1;
                }
                Some(_) => len += 1,
                None => {}
            }
        } else {
            len += c.len_utf8() as u64;
        }
    }
    len
}

fn check_unary_neg(
    inner: &Expr,
    expected: Option<&Type>,
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    match inner {
        Expr::Int(ispan, text) => {
            let raw = parse_int_literal(text)
                .ok_or_else(|| type_error("invalid integer literal".to_string(), *ispan))?;
            let value = -raw;
            let ty = match expected {
                Some(t) if is_integer_scalar(t) => {
                    check_int_range(value, t, *ispan)?;
                    t.clone()
                }
                Some(t) => {
                    return Err(type_error(
                        format!(
                            "expected `{}`, found an integer literal",
                            types::render_type(t)
                        ),
                        *ispan,
                    ));
                }
                None => {
                    check_int_range(value, &Type::I64, *ispan)?;
                    Type::I64
                }
            };
            let literal = TypedExpr {
                ty: ty.clone(),
                kind: TypedExprKind::Int(text.clone()),
            };
            Ok(TypedExpr {
                ty,
                kind: TypedExprKind::Neg(Box::new(literal)),
            })
        }
        Expr::Float(_, text) => {
            let te = synth_float_literal(inner.span(), text, expected)?;
            let ty = te.ty.clone();
            Ok(TypedExpr {
                ty,
                kind: TypedExprKind::Neg(Box::new(te)),
            })
        }
        _ => {
            let it = check_expr(inner, expected, fctx, mctx)?;
            if (is_integer_scalar(&it.ty) && is_signed_scalar(&it.ty)) || is_float_scalar(&it.ty) {
                let ty = it.ty.clone();
                Ok(TypedExpr {
                    ty,
                    kind: TypedExprKind::Neg(Box::new(it)),
                })
            } else {
                Err(type_error(
                    format!(
                        "unary `-` requires a signed integer or float type, found `{}`",
                        types::render_type(&it.ty)
                    ),
                    span,
                ))
            }
        }
    }
}

fn check_binary(
    op: BinOp,
    l: &Expr,
    r: &Expr,
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let (lt, rt) = check_same_type_operands(l, r, fctx, mctx)?;
    build_binop_expr(op, lt, rt, span, mctx)
}

/// Checks two operands that must share one type (a binary operator's
/// sides, a range's endpoints), with no unification (decision 4): one
/// side is synthesized on its own, then the other is checked against it.
/// A bare, unannotated integer/float literal defers to a concrete
/// sibling when there is one — `0 .. n` (or `n + 1`) types the literal
/// against `n`'s type rather than defaulting it first and rejecting `n`
/// — so ordinary code with the literal on either side works the same
/// way; only two bare literals together fall back to plain left-to-right
/// (both then default identically, so it never matters which is first).
/// Returns the typed pair in the *original* `(a, b)` order regardless of
/// which side was synthesized first internally.
fn check_same_type_operands(
    a: &Expr,
    b: &Expr,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(TypedExpr, TypedExpr), SemaError> {
    if is_bare_numeric_literal(a) && !is_bare_numeric_literal(b) {
        let bt = check_expr(b, None, fctx, mctx)?;
        let at = check_expr(a, Some(&bt.ty), fctx, mctx)?;
        Ok((at, bt))
    } else {
        let at = check_expr(a, None, fctx, mctx)?;
        let bt = check_expr(b, Some(&at.ty), fctx, mctx)?;
        Ok((at, bt))
    }
}

pub(crate) fn is_bare_numeric_literal(e: &Expr) -> bool {
    matches!(e, Expr::Int(..) | Expr::Float(..))
}

/// Both operands already share a type by the time this runs
/// (`check_binary` calls `check_same_type_operands`;
/// `check_compound_assign` checks the value against the target's type).
/// Builtin scalar ops never desugar (02-language.md §7.4): a user
/// (`Named`) type's `+ - * / %` and `<` resolve to the matching 05§8
/// method (`OpCall`, decision 1); everything else in the table
/// (wrapping, shifts, bitwise, `==`/`!=`) is core-scalar-only and stays
/// the primitive `Binary` node.
fn build_binop_expr(
    op: BinOp,
    l: TypedExpr,
    r: TypedExpr,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    use BinOp::*;
    let ty = l.ty.clone();
    match op {
        Add | Sub | Mul | Div | Rem => {
            if is_numeric_scalar(&ty) {
                return Ok(TypedExpr {
                    ty,
                    kind: TypedExprKind::Binary(op, Box::new(l), Box::new(r)),
                });
            }
            if let Type::Named(name, targs) = &ty {
                let method = match op {
                    Add => "add",
                    Sub => "subtract",
                    Mul => "multiply",
                    Div => "divide",
                    Rem => "remainder",
                    _ => unreachable!(),
                };
                let (ret_ty, key) = resolve_operator_method(name, targs, method, &ty, mctx, span)?;
                return Ok(TypedExpr {
                    ty: ret_ty,
                    kind: TypedExprKind::OpCall(key, Box::new(l), Box::new(r)),
                });
            }
            Err(type_error(
                format!(
                    "operator `{}` is not supported for type `{}`",
                    op.as_str(),
                    types::render_type(&ty)
                ),
                span,
            ))
        }
        AddW | SubW | MulW => {
            if is_integer_scalar(&ty) {
                Ok(TypedExpr {
                    ty,
                    kind: TypedExprKind::Binary(op, Box::new(l), Box::new(r)),
                })
            } else {
                Err(type_error(
                    format!(
                        "wrapping arithmetic requires an integer type, found `{}`",
                        types::render_type(&ty)
                    ),
                    span,
                ))
            }
        }
        Shl | Shr | BitAnd | BitOr | BitXor => {
            if is_integer_scalar(&ty) {
                Ok(TypedExpr {
                    ty,
                    kind: TypedExprKind::Binary(op, Box::new(l), Box::new(r)),
                })
            } else {
                Err(type_error(
                    format!(
                        "`{}` requires an integer type, found `{}`",
                        op.as_str(),
                        types::render_type(&ty)
                    ),
                    span,
                ))
            }
        }
        Lt | Le | Gt | Ge => {
            if is_numeric_scalar(&ty) || matches!(ty, Type::Char) {
                return Ok(TypedExpr {
                    ty: Type::Bool,
                    kind: TypedExprKind::Binary(op, Box::new(l), Box::new(r)),
                });
            }
            if let Type::Named(name, targs) = &ty {
                if op == Lt {
                    let (ret, key) =
                        resolve_operator_method(name, targs, "less_than", &ty, mctx, span)?;
                    if ret != Type::Bool {
                        return Err(type_error(
                            format!("`{name}.less_than` must return `bool`"),
                            span,
                        ));
                    }
                    return Ok(TypedExpr {
                        ty: Type::Bool,
                        kind: TypedExprKind::OpCall(key, Box::new(l), Box::new(r)),
                    });
                }
                return Err(unimplemented_at(
                    "derived comparisons (`<=`, `>`, `>=`) on a user type are",
                    span,
                ));
            }
            Err(type_error(
                format!(
                    "comparison is not supported for type `{}`",
                    types::render_type(&ty)
                ),
                span,
            ))
        }
        Eq | Ne => {
            if is_resource_type(&ty, mctx) {
                return Err(type_error(
                    format!(
                        "cannot compare resource type `{}` with `==`",
                        types::render_type(&ty)
                    ),
                    span,
                ));
            }
            Ok(TypedExpr {
                ty: Type::Bool,
                kind: TypedExprKind::Binary(op, Box::new(l), Box::new(r)),
            })
        }
    }
}

/// Mirrors `types::classify_type` (already computed, memoized, per
/// struct/enum in `mctx`) to answer the one question the operator pass
/// needs: is this composite type's structural `==` forbidden because it
/// (transitively) contains a resource? The compound-propagation rule
/// itself (own/array/tuple/Option/Result propagate, everything else is
/// data) is `types::resource_propagates`'s one exhaustive triage point,
/// shared with `classify_type`; the only thing supplied here is the leaf
/// question — a named type's resource-ness, read straight from `mctx`'s
/// already-computed classifications rather than recursively re-deriving
/// them.
pub(crate) fn is_resource_type(ty: &Type, mctx: &ModuleCtx) -> bool {
    types::resource_propagates(ty, &mut |name, _args| {
        mctx.structs
            .get(name)
            .map(|s| s.decl.classification == Classification::Resource)
            .or_else(|| {
                mctx.enums
                    .get(name)
                    .map(|e| e.classification == Classification::Resource)
            })
            .unwrap_or(false)
    })
}

/// Resolves `<type-name>.<method>` as an operator-desugar target
/// (05-library.md §8 shape: `fn <method>(read self, right: <Self>) ->
/// R`), returning the method's declared result type `R` and the callee
/// key the typed tree's `OpCall` node carries (plans/M3.md item A):
/// `targs` is the operand's own (possibly empty) generic argument list
/// (item H): a non-empty one substitutes + enqueues the concrete
/// instantiation first (`generics::instantiate_struct`), so the shape
/// check below runs against the concrete method exactly like it does for
/// a non-generic operand, and the key names that instantiation.
fn resolve_operator_method(
    name: &str,
    targs: &[TypeArg],
    method: &str,
    self_ty: &Type,
    mctx: &ModuleCtx,
    span: Span,
) -> Result<(Type, CalleeKey), SemaError> {
    let s = if targs.is_empty() {
        match mctx.structs.get(name) {
            Some(s) => std::borrow::Cow::Borrowed(s),
            None => {
                return Err(missing_method_error(
                    format!("type `{name}` has no operator method `{method}`"),
                    name,
                    method,
                    span,
                ));
            }
        }
    } else {
        std::borrow::Cow::Owned(generics::instantiate_struct(mctx, name, targs, span)?)
    };
    let Some((_, d)) = s.method(method) else {
        return Err(missing_method_error(
            format!("type `{name}` has no operator method `{method}`"),
            name,
            method,
            span,
        ));
    };
    let receiver_read = d
        .receiver
        .as_ref()
        .map(|r| r.mode == AccessMode::Read)
        .unwrap_or(false);
    let shape_ok = receiver_read
        && d.generics.is_empty()
        && d.params.len() == 1
        && types_eq(&d.params[0].ty, self_ty);
    if !shape_ok {
        return Err(type_error(
            format!(
                "`{name}.{method}` does not match the operator method shape `{method}(read self, right: {name}) -> ...`"
            ),
            span,
        ));
    }
    let key = if targs.is_empty() {
        CalleeKey::Method(name.to_string(), method.to_string())
    } else {
        CalleeKey::MethodInstance(
            generics::canonical_key(InstKind::Struct, name, targs),
            method.to_string(),
        )
    };
    Ok((d.ret.clone(), key))
}

// --- `?` (02-language.md §7.4, §8.2; 05-library.md §1) --------------------

fn check_try(
    span: Span,
    inner: &Expr,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let inner_t = check_expr(inner, None, fctx, mctx)?;
    match inner_t.ty.clone() {
        Type::Result(t_ok, t_err) => match fctx.ret_ty.clone() {
            Type::Result(_, ret_err) => {
                if types_eq(&t_err, &ret_err) {
                    Ok(TypedExpr {
                        ty: *t_ok,
                        kind: TypedExprKind::Try(Box::new(inner_t), None),
                    })
                } else if let Some((conv_ret, key)) = try_from_conversion(&t_err, &ret_err, mctx) {
                    if types_eq(&conv_ret, &ret_err) {
                        Ok(TypedExpr {
                            ty: *t_ok,
                            kind: TypedExprKind::Try(Box::new(inner_t), Some(key)),
                        })
                    } else {
                        Err(type_error(
                            format!(
                                "`?` conversion `from` must return `{}`, found `{}`",
                                types::render_type(&ret_err),
                                types::render_type(&conv_ret)
                            ),
                            span,
                        ))
                    }
                } else {
                    Err(type_error(
                        format!(
                            "`?` cannot convert error type `{}` to `{}`",
                            types::render_type(&t_err),
                            types::render_type(&ret_err)
                        ),
                        span,
                    ))
                }
            }
            _ => Err(type_error(
                "`?` on a `Result` requires an enclosing function returning `Result`".to_string(),
                span,
            )),
        },
        Type::Option(t_inner) => match &fctx.ret_ty {
            Type::Option(_) => Ok(TypedExpr {
                ty: *t_inner,
                kind: TypedExprKind::Try(Box::new(inner_t), None),
            }),
            _ => Err(type_error(
                "`?` on an `Option` requires an enclosing function returning `Option`".to_string(),
                span,
            )),
        },
        other => Err(type_error(
            format!(
                "`?` requires a `Result` or `Option`, found `{}`",
                types::render_type(&other)
            ),
            span,
        )),
    }
}

/// The one hop `?` may take (02-language.md §7.4: "no chains, no
/// implicit widening"): `target_ty` either matches `err_ty` directly
/// (checked by the caller before this runs) or names a struct/enum
/// declaring the conversion — a user-written associated `from(take
/// source: E) -> Self`, or the equivalent `deriving(From)` generates
/// (05-library.md §8) from its single field/payload. Returns the
/// conversion's return type plus the `<Target>.from`-shaped callee key
/// (plans/M3.md item A) — the same key regardless of which of the two
/// shapes produced it, since both desugar identically for a consumer of
/// the typed tree (the evaluator, item B, special-cases the
/// `deriving(From)` body's own synthesis, not this key).
fn try_from_conversion(
    err_ty: &Type,
    target_ty: &Type,
    mctx: &ModuleCtx,
) -> Option<(Type, CalleeKey)> {
    let Type::Named(name, targs) = target_ty else {
        return None;
    };
    if !targs.is_empty() {
        return None;
    }
    if let Some(s) = mctx.structs.get(name) {
        if s.decl.deriving.iter().any(|d| d == "From") {
            let field_ty = s.decl.members.iter().find_map(|m| match m {
                DeclMember::Field(f) => Some(f.ty.clone()),
                _ => None,
            });
            if let Some(ft) = field_ty {
                if types_eq(&ft, err_ty) {
                    return Some((
                        target_ty.clone(),
                        CalleeKey::Method(name.clone(), "from".to_string()),
                    ));
                }
            }
        }
        if let Some((_, d)) = s.assoc_fn("from") {
            let shape_ok = d.generics.is_empty()
                && d.params.len() == 1
                && d.params[0].mode == AccessMode::Take
                && types_eq(&d.params[0].ty, err_ty);
            if shape_ok {
                return Some((
                    d.ret.clone(),
                    CalleeKey::Method(name.clone(), "from".to_string()),
                ));
            }
        }
    }
    if let Some(e) = mctx.enums.get(name) {
        if e.deriving.iter().any(|d| d == "From") {
            if let Some(dv) = e.variants.first() {
                if let Some(pt) = decl_variant_payload_types(dv).into_iter().next() {
                    if types_eq(&pt, err_ty) {
                        return Some((
                            target_ty.clone(),
                            CalleeKey::Method(name.clone(), "from".to_string()),
                        ));
                    }
                }
            }
        }
    }
    None
}

// --- closures (02-language.md §8.3) --------------------------------------

fn check_closure(
    c: &ClosureExpr,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let Some(Type::Fn(exp_params, exp_ret)) = expected.cloned() else {
        return Err(type_error(
            "a closure needs a known function type from its call-site context".to_string(),
            c.span,
        ));
    };
    if c.params.len() != exp_params.len() {
        return Err(arity_error(exp_params.len(), c.params.len(), c.span));
    }
    fctx.push_scope();
    let result = check_closure_body(c, &exp_params, &exp_ret, fctx, mctx);
    fctx.pop_scope();
    let (params, body) = result?;
    Ok(TypedExpr {
        ty: Type::Fn(exp_params, exp_ret),
        kind: TypedExprKind::Closure { params, body },
    })
}

fn check_closure_body(
    c: &ClosureExpr,
    exp_params: &[(AccessMode, Type)],
    exp_ret: &Type,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<(Vec<TypedClosureParam>, TypedClosureBody), SemaError> {
    let mut typed_params = Vec::with_capacity(c.params.len());
    for (cp, (mode, ety)) in c.params.iter().zip(exp_params.iter()) {
        let pty = match &cp.ty {
            Some(t) => {
                let resolved = mctx.resolve_type(t, &fctx.local_pools)?;
                if !types_eq(&resolved, ety) {
                    return Err(type_error(
                        format!(
                            "closure parameter `{}` expects `{}`, found `{}`",
                            cp.name,
                            types::render_type(ety),
                            types::render_type(&resolved)
                        ),
                        cp.span,
                    ));
                }
                resolved
            }
            None => ety.clone(),
        };
        fctx.insert_local(cp.name.clone(), pty.clone());
        typed_params.push(TypedClosureParam {
            mode: *mode,
            name: cp.name.clone(),
            ty: pty,
        });
    }
    let body = match &c.body {
        ClosureBody::Expr(e) => {
            let te = check_expr(e, Some(exp_ret), fctx, mctx)?;
            TypedClosureBody::Expr(Box::new(te))
        }
        ClosureBody::Suite(stmts) => {
            let saved_ret = std::mem::replace(&mut fctx.ret_ty, exp_ret.clone());
            let r = check_stmts(stmts, fctx, mctx);
            fctx.ret_ty = saved_ret;
            TypedClosureBody::Suite(r?)
        }
    };
    Ok((typed_params, body))
}

// --- calls: fn/method/associated-fn/init/struct-literal/enum-variant ----

fn call_fn_value(
    callee: TypedExpr,
    args: &[Arg],
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    match callee.ty.clone() {
        Type::Fn(params, ret) => {
            let typed_args = check_positional_args(&params, args, span, fctx, mctx)?;
            Ok(TypedExpr {
                ty: *ret,
                kind: TypedExprKind::CallValue(Box::new(callee), typed_args),
            })
        }
        other => Err(type_error(
            format!("type `{}` is not callable", types::render_type(&other)),
            span,
        )),
    }
}

fn check_call(
    callee: &Expr,
    span: Span,
    args: &[Arg],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    match callee {
        Expr::Index(inner, ispan, targs) => {
            check_call_index(inner, *ispan, targs, args, span, fctx, mctx)
        }
        Expr::Name(_, name) => check_call_by_name(name, span, args, expected, fctx, mctx),
        Expr::Field(base, fspan, name) => {
            check_call_by_field(base, *fspan, name, span, args, fctx, mctx)
        }
        other => {
            let callee_t = check_expr(other, None, fctx, mctx)?;
            call_fn_value(callee_t, args, span, fctx, mctx)
        }
    }
}

/// Callee shaped `expr[targs](args)` — either a scalar conversion
/// (`x.to[T]()`, `x.checked_to[T]()`, `x.truncate_to[T]()`) or a generic
/// instantiation with explicit arguments (`Ring[Sector, 4](...)`,
/// `hash_pair[Sector](...)`, item H): the latter resolves `targs` (raw
/// `Expr`s — `generics::resolve_call_targs`), substitutes + enqueues the
/// concrete instantiation, and checks the call against the substituted
/// signature exactly like the non-generic path does. A generic *method*
/// called this way (`x.method[Args](...)`) is item H's documented scope
/// boundary and still fails closed.
fn check_call_index(
    inner: &Expr,
    ispan: Span,
    targs: &[Expr],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Expr::Field(base, fspan, mname) = inner {
        if mname == "to" || mname == "checked_to" || mname == "truncate_to" {
            if targs.len() != 1 {
                return Err(type_error(
                    "a conversion needs exactly one type argument".to_string(),
                    ispan,
                ));
            }
            let base_t = check_expr(base, None, fctx, mctx)?;
            if !is_scalar(&base_t.ty) {
                return Err(type_error(
                    format!(
                        "`.{mname}` is only defined for scalar types, found `{}`",
                        types::render_type(&base_t.ty)
                    ),
                    *fspan,
                ));
            }
            if !args.is_empty() {
                return Err(type_error(
                    format!("`.{mname}()` takes no arguments"),
                    call_span,
                ));
            }
            if mname == "checked_to" {
                return Err(unimplemented_at("checked_to conversion is", call_span));
            }
            if mname == "truncate_to" {
                return Err(unimplemented_at("truncate_to conversion is", call_span));
            }
            let target = scalar_type_by_name_expr(&targs[0]).ok_or_else(|| {
                type_error("`.to` target must be a scalar type".to_string(), ispan)
            })?;
            return Ok(TypedExpr {
                ty: target,
                kind: TypedExprKind::ToScalar(Box::new(base_t)),
            });
        }
        // `x.method[Args](...)`: a generic method call — item H's scope
        // boundary, documented above.
        return Err(unimplemented_at("generic instantiation is", call_span));
    }
    if let Expr::Name(_, name) = inner {
        if fctx.lookup_local(name).is_none() {
            if let Some(fi) = mctx.fns.get(name) {
                if !fi.decl.generics.is_empty() {
                    let type_args = generics::resolve_call_targs(targs, mctx)?;
                    let fi = generics::instantiate_fn(mctx, name, &type_args, call_span)?;
                    let typed_args = check_call_args(
                        &fi.ast.params,
                        &fi.decl.params,
                        args,
                        call_span,
                        fctx,
                        mctx,
                    )?;
                    let key = CalleeKey::FnInstance(generics::canonical_key(
                        InstKind::Fn,
                        name,
                        &type_args,
                    ));
                    return Ok(TypedExpr {
                        ty: fi.decl.ret,
                        kind: TypedExprKind::Call {
                            callee: key,
                            receiver: None,
                            args: typed_args,
                        },
                    });
                }
            } else if let Some(si) = mctx.structs.get(name) {
                if !si.decl.generics.is_empty() {
                    let type_args = generics::resolve_call_targs(targs, mctx)?;
                    let si = generics::instantiate_struct(mctx, name, &type_args, call_span)?;
                    return check_struct_construction(&si, &type_args, args, call_span, fctx, mctx);
                }
            }
        }
    }
    Err(unimplemented_at("generic instantiation is", call_span))
}

fn scalar_type_by_name_expr(e: &Expr) -> Option<Type> {
    match e {
        Expr::Name(_, name) => scalar_type_by_name(name),
        _ => None,
    }
}

pub(crate) fn scalar_type_by_name(name: &str) -> Option<Type> {
    Some(match name {
        "bool" => Type::Bool,
        "u8" => Type::U8,
        "u16" => Type::U16,
        "u32" => Type::U32,
        "u64" => Type::U64,
        "usize" => Type::Usize,
        "i8" => Type::I8,
        "i16" => Type::I16,
        "i32" => Type::I32,
        "i64" => Type::I64,
        "isize" => Type::Isize,
        "f32" => Type::F32,
        "f64" => Type::F64,
        "char" => Type::Char,
        _ => return None,
    })
}

fn is_scalar(t: &Type) -> bool {
    is_numeric_scalar(t) || matches!(t, Type::Bool | Type::Char)
}

fn check_call_by_name(
    name: &str,
    call_span: Span,
    args: &[Arg],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Some(ty) = fctx.lookup_local(name) {
        let callee_t = TypedExpr {
            ty,
            kind: TypedExprKind::Local(name.to_string()),
        };
        return call_fn_value(callee_t, args, call_span, fctx, mctx);
    }
    if let Some(c) = mctx.consts.get(name) {
        let callee_t = TypedExpr {
            ty: c.clone(),
            kind: TypedExprKind::Const(name.to_string()),
        };
        return call_fn_value(callee_t, args, call_span, fctx, mctx);
    }
    if let Some(f) = mctx.fns.get(name) {
        if !f.decl.generics.is_empty() {
            // `name(args)`, no explicit `[Args]` — item H/item 2's
            // inference: a type parameter used directly as a parameter's
            // own type is inferred from that argument's synthesized
            // type; anything else (a const parameter, an uninferable or
            // mismatched type parameter) reports `error[generic]` naming
            // the parameter, per item 2.
            let type_args = generics::infer_fn_targs(f, args, fctx, mctx, call_span)?;
            let fi = generics::instantiate_fn(mctx, name, &type_args, call_span)?;
            let typed_args =
                check_call_args(&fi.ast.params, &fi.decl.params, args, call_span, fctx, mctx)?;
            let key =
                CalleeKey::FnInstance(generics::canonical_key(InstKind::Fn, name, &type_args));
            return Ok(TypedExpr {
                ty: fi.decl.ret,
                kind: TypedExprKind::Call {
                    callee: key,
                    receiver: None,
                    args: typed_args,
                },
            });
        }
        let typed_args =
            check_call_args(&f.ast.params, &f.decl.params, args, call_span, fctx, mctx)?;
        return Ok(TypedExpr {
            ty: f.decl.ret.clone(),
            kind: TypedExprKind::Call {
                callee: CalleeKey::Fn(name.to_string()),
                receiver: None,
                args: typed_args,
            },
        });
    }
    if let Some(s) = mctx.structs.get(name) {
        if !s.decl.generics.is_empty() {
            // Struct construction has no inference (only `fn` calls do,
            // 02-language.md §6.3/§7.3) — explicit `Name[Args](...)` is
            // always required.
            return Err(SemaError::at(
                "generic",
                format!("`{name}` requires explicit `[Args]`"),
                call_span,
            ));
        }
        return check_struct_construction(s, &[], args, call_span, fctx, mctx);
    }
    match name {
        "Some" => {
            if args.len() != 1 {
                return Err(arity_error(1, args.len(), call_span));
            }
            let inner_expected = match expected {
                Some(Type::Option(inner)) => Some((**inner).clone()),
                _ => None,
            };
            let it = check_expr(&args[0].value, inner_expected.as_ref(), fctx, mctx)?;
            let ty = Type::Option(Box::new(it.ty.clone()));
            Ok(TypedExpr {
                ty,
                kind: TypedExprKind::EnumConstruct {
                    enum_name: "Option".to_string(),
                    variant: "Some".to_string(),
                    args: vec![it],
                },
            })
        }
        "Ok" => {
            if args.len() != 1 {
                return Err(arity_error(1, args.len(), call_span));
            }
            let (t_expected, e_ty) = match expected {
                Some(Type::Result(t, e)) => (Some((**t).clone()), Some((**e).clone())),
                _ => (None, None),
            };
            let t_typed = check_expr(&args[0].value, t_expected.as_ref(), fctx, mctx)?;
            let e_ty = e_ty.ok_or_else(|| {
                type_error(
                    "cannot infer the error type of `Ok(...)` without context".to_string(),
                    call_span,
                )
            })?;
            let ty = Type::Result(Box::new(t_typed.ty.clone()), Box::new(e_ty));
            Ok(TypedExpr {
                ty,
                kind: TypedExprKind::EnumConstruct {
                    enum_name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    args: vec![t_typed],
                },
            })
        }
        "Err" => {
            if args.len() != 1 {
                return Err(arity_error(1, args.len(), call_span));
            }
            let (t_ty_opt, e_expected) = match expected {
                Some(Type::Result(t, e)) => (Some((**t).clone()), Some((**e).clone())),
                _ => (None, None),
            };
            let e_typed = check_expr(&args[0].value, e_expected.as_ref(), fctx, mctx)?;
            let t_ty = t_ty_opt.ok_or_else(|| {
                type_error(
                    "cannot infer the ok type of `Err(...)` without context".to_string(),
                    call_span,
                )
            })?;
            let ty = Type::Result(Box::new(t_ty), Box::new(e_typed.ty.clone()));
            Ok(TypedExpr {
                ty,
                kind: TypedExprKind::EnumConstruct {
                    enum_name: "Result".to_string(),
                    variant: "Err".to_string(),
                    args: vec![e_typed],
                },
            })
        }
        "panic" => {
            if args.len() != 1 {
                return Err(arity_error(1, args.len(), call_span));
            }
            let mt = check_expr(
                &args[0].value,
                Some(&Type::Static(Box::new(Type::Str))),
                fctx,
                mctx,
            )?;
            Ok(TypedExpr {
                ty: Type::Never,
                kind: TypedExprKind::Panic(Box::new(mt)),
            })
        }
        _ => Err(type_error(format!("`{name}` is not callable"), call_span)),
    }
}

fn check_call_by_field(
    base: &Expr,
    fspan: Span,
    name: &str,
    call_span: Span,
    args: &[Arg],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Expr::Name(_, bname) = base {
        if fctx.lookup_local(bname).is_none() {
            if let Some(s) = mctx.structs.get(bname.as_str()) {
                if !s.decl.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", call_span));
                }
                if let Some((af, d)) = s.assoc_fn(name) {
                    if !d.generics.is_empty() {
                        return Err(unimplemented_at("generic instantiation is", call_span));
                    }
                    let typed_args =
                        check_call_args(&af.params, &d.params, args, call_span, fctx, mctx)?;
                    let key = CalleeKey::Method(bname.clone(), name.to_string());
                    return Ok(TypedExpr {
                        ty: d.ret.clone(),
                        kind: TypedExprKind::Call {
                            callee: key,
                            receiver: None,
                            args: typed_args,
                        },
                    });
                }
                return Err(type_error(
                    format!("type `{bname}` has no associated function `{name}`"),
                    fspan,
                ));
            }
            if let Some(e) = mctx.enums.get(bname.as_str()) {
                if !e.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", call_span));
                }
                if let Some(dv) = e.variants.iter().find(|v| v.name == name) {
                    let payload_types = decl_variant_payload_types(dv);
                    let typed_args =
                        check_variant_args(&payload_types, args, call_span, fctx, mctx)?;
                    return Ok(TypedExpr {
                        ty: Type::Named(e.name.clone(), vec![]),
                        kind: TypedExprKind::EnumConstruct {
                            enum_name: e.name.clone(),
                            variant: name.to_string(),
                            args: typed_args,
                        },
                    });
                }
                return Err(type_error(
                    format!("enum `{bname}` has no variant `{name}`"),
                    fspan,
                ));
            }
        }
    }
    let base_t = check_expr(base, None, fctx, mctx)?;
    let base_ty = unwrap_own(base_t.ty.clone());
    match &base_ty {
        Type::Named(sname, targs) => {
            // A method call through a generic instantiation (item H):
            // substitute + enqueue it, then check the call against the
            // substituted method's (now concrete) signature.
            let s = if targs.is_empty() {
                match mctx.structs.get(sname.as_str()) {
                    Some(s) => std::borrow::Cow::Borrowed(s),
                    None => {
                        return Err(missing_method_error(
                            format!("type `{sname}` has no method `{name}`"),
                            sname,
                            name,
                            fspan,
                        ));
                    }
                }
            } else {
                std::borrow::Cow::Owned(generics::instantiate_struct(
                    mctx, sname, targs, call_span,
                )?)
            };
            let Some((mf, d)) = s.method(name) else {
                return Err(missing_method_error(
                    format!("type `{sname}` has no method `{name}`"),
                    sname,
                    name,
                    fspan,
                ));
            };
            if !d.generics.is_empty() {
                // A generic *method* (its own `[...]`, beyond the
                // struct's, if any) is item H's documented scope
                // boundary.
                return Err(unimplemented_at("generic instantiation is", call_span));
            }
            let typed_args = check_call_args(&mf.params, &d.params, args, call_span, fctx, mctx)?;
            let key = if targs.is_empty() {
                CalleeKey::Method(sname.clone(), name.to_string())
            } else {
                CalleeKey::MethodInstance(
                    generics::canonical_key(InstKind::Struct, sname, targs),
                    name.to_string(),
                )
            };
            Ok(TypedExpr {
                ty: d.ret.clone(),
                kind: TypedExprKind::Call {
                    callee: key,
                    receiver: Some(Box::new(base_t)),
                    args: typed_args,
                },
            })
        }
        other => {
            let type_name = types::render_type(other);
            Err(missing_method_error(
                format!("type `{type_name}` has no method `{name}`"),
                &type_name,
                name,
                fspan,
            ))
        }
    }
}

fn check_struct_construction(
    s: &StructInfo,
    targs: &[TypeArg],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    // Bug 1(b) fix: a construction's synthesized type must carry the
    // instantiation's own args (`Slot[u64, 2](...)` synthesizes
    // `Type::Named("Slot", [u64, 2])`, not a bare `Type::Named("Slot",
    // [])`) — `s`'s own members are already substituted (`s.decl` here is
    // always the already-instantiated `StructInfo` when `targs` is
    // non-empty, per every caller below), so construction-argument
    // checking below (`s.init()`/`check_struct_literal`, which read field/
    // init-param types straight off `s.decl`) was already correct; only
    // the *result* type dropped the args.
    let self_ty = Type::Named(s.decl.name.clone(), targs.to_vec());
    if let Some((ia, id)) = s.init() {
        let typed_args = check_call_args(&ia.params, &id.params, args, call_span, fctx, mctx)?;
        let key = if targs.is_empty() {
            CalleeKey::Method(s.decl.name.clone(), "init".to_string())
        } else {
            CalleeKey::MethodInstance(
                generics::canonical_key(InstKind::Struct, &s.decl.name, targs),
                "init".to_string(),
            )
        };
        let ret_ty = match &id.ret {
            Type::Unit => self_ty.clone(),
            Type::Result(ok, err) if **ok == Type::Unit => {
                Type::Result(Box::new(self_ty.clone()), err.clone())
            }
            _ => {
                return Err(unimplemented_at(
                    "a non-standard init return type is",
                    call_span,
                ));
            }
        };
        // `init` has no receiver expression at the *call* site (there is
        // no existing value to read `self` from yet — construction is
        // what produces it), so the receiver slot stays empty; the
        // callee key alone (`...".init"`) is what the evaluator (item B)
        // will recognize as "allocate `Self`, then run this with the
        // fresh value as `self`".
        return Ok(TypedExpr {
            ty: ret_ty,
            kind: TypedExprKind::Call {
                callee: key,
                receiver: None,
                args: typed_args,
            },
        });
    }
    let fields = check_struct_literal(s, args, call_span, fctx, mctx)?;
    Ok(TypedExpr {
        ty: self_ty,
        kind: TypedExprKind::StructLiteral {
            name: s.decl.name.clone(),
            fields,
        },
    })
}

/// A struct without `init` builds from its named-field literal
/// (02-language.md §7.1): every field exactly once unless defaulted,
/// positional only for a one-field struct. Returns only the explicitly
/// supplied fields, declaration order (plans/M3.md item A) — an omitted,
/// defaulted field is elided; its default lives once on
/// `typed::TypedStruct::field_defaults`.
fn check_struct_literal(
    s: &StructInfo,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<(String, TypedExpr)>, SemaError> {
    let fields: Vec<(String, Type, bool)> = s
        .members()
        .filter_map(|(am, dm)| match (am, dm) {
            (Member::Field(af), DeclMember::Field(df)) => {
                Some((af.name.clone(), df.ty.clone(), af.default.is_some()))
            }
            _ => None,
        })
        .collect();
    if fields.len() == 1 && args.len() == 1 && args[0].label.is_none() {
        let vt = check_expr(&args[0].value, Some(&fields[0].1), fctx, mctx)?;
        return Ok(vec![(fields[0].0.clone(), vt)]);
    }
    let mut bound = vec![false; fields.len()];
    let mut slots: Vec<Option<TypedExpr>> = (0..fields.len()).map(|_| None).collect();
    for a in args {
        let Some(label) = &a.label else {
            return Err(type_error(
                "struct construction requires labeled fields (positional only for a one-field struct)".to_string(),
                a.span,
            ));
        };
        let Some(idx) = fields.iter().position(|f| &f.0 == label) else {
            return Err(type_error(format!("unknown field `{label}`"), a.span));
        };
        if bound[idx] {
            return Err(type_error(
                format!("field `{label}` supplied more than once"),
                a.span,
            ));
        }
        bound[idx] = true;
        let fty = fields[idx].1.clone();
        let vt = check_expr(&a.value, Some(&fty), fctx, mctx)?;
        slots[idx] = Some(vt);
    }
    for (i, (name, _, has_default)) in fields.iter().enumerate() {
        if !bound[i] && !has_default {
            return Err(type_error(format!("missing field `{name}`"), call_span));
        }
    }
    let out = fields
        .iter()
        .zip(slots.into_iter())
        .filter_map(|((name, _, _), v)| v.map(|vt| (name.clone(), vt)))
        .collect();
    Ok(out)
}

/// Arity + label checking shared by fn/method/init calls
/// (02-language.md §5.1): each argument binds to a parameter either by
/// label (looked up by name) or positionally (the next not-yet-bound
/// parameter, left to right); every parameter must end up bound exactly
/// once, unless it has a default. Access-mode markers on `args` are
/// parsed but not validated here (item D's job). Returns one slot per
/// declared parameter, in declaration order (plans/M3.md item A): `None`
/// for a parameter this call left to its own stored default
/// (`typed::TypedParam::default`, typed once on the callee's own
/// declaration — never re-typed per call site, since a default may
/// reference the callee's own `self`).
fn check_call_args(
    ast_params: &[ast::Param],
    decl_params: &[DeclParam],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<Option<TypedExpr>>, SemaError> {
    let mut bound = vec![false; decl_params.len()];
    let mut slots: Vec<Option<TypedExpr>> = (0..decl_params.len()).map(|_| None).collect();
    let mut cursor = 0usize;
    for a in args {
        let idx = match &a.label {
            Some(lbl) => {
                let Some(i) = decl_params.iter().position(|p| &p.name == lbl) else {
                    return Err(type_error(
                        format!("unknown parameter label `{lbl}`"),
                        a.span,
                    ));
                };
                i
            }
            None => {
                while cursor < bound.len() && bound[cursor] {
                    cursor += 1;
                }
                if cursor >= decl_params.len() {
                    return Err(type_error("too many arguments".to_string(), a.span));
                }
                let i = cursor;
                cursor += 1;
                i
            }
        };
        if bound[idx] {
            return Err(type_error(
                format!("argument `{}` bound more than once", decl_params[idx].name),
                a.span,
            ));
        }
        bound[idx] = true;
        let pty = decl_params[idx].ty.clone();
        let vt = check_expr(&a.value, Some(&pty), fctx, mctx)?;
        slots[idx] = Some(vt);
    }
    for (i, p) in decl_params.iter().enumerate() {
        if !bound[i] && ast_params[i].default.is_none() {
            return Err(type_error(
                format!("missing argument for parameter `{}`", p.name),
                call_span,
            ));
        }
    }
    Ok(slots)
}

/// Positional-only arg checking against a raw `fn(...)`-typed value
/// (a closure/named-function reference): unlike a real call, there are
/// no parameter names to label against, and a raw `fn` type carries no
/// defaults — every slot is always explicit.
fn check_positional_args(
    params: &[(AccessMode, Type)],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedExpr>, SemaError> {
    if args.len() != params.len() {
        return Err(arity_error(params.len(), args.len(), call_span));
    }
    let mut out = Vec::with_capacity(args.len());
    for (a, (_mode, ty)) in args.iter().zip(params.iter()) {
        if a.label.is_some() {
            return Err(type_error(
                "labeled arguments require a named function".to_string(),
                a.span,
            ));
        }
        out.push(check_expr(&a.value, Some(ty), fctx, mctx)?);
    }
    Ok(out)
}

/// Enum variant construction (`Enum.Variant(...)`, leading-dot
/// `.Variant(...)`): positional only, mirroring the ast's own note that
/// pattern payloads "bind positionally regardless of whether the variant
/// was declared with named fields" (02-language.md §7.2). A variant's
/// payload never has defaults, so every slot is always explicit.
fn check_variant_args(
    payload: &[Type],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedExpr>, SemaError> {
    if args.len() != payload.len() {
        return Err(arity_error(payload.len(), args.len(), call_span));
    }
    let mut out = Vec::with_capacity(args.len());
    for (a, ty) in args.iter().zip(payload.iter()) {
        out.push(check_expr(&a.value, Some(ty), fctx, mctx)?);
    }
    Ok(out)
}

// --- the fail-closed set: defer's own `await`/`?` scan --------------------

/// `defer`'s body cannot `await` and cannot use `?` (02-language.md §10:
/// "a deferred action cannot await and cannot fail recoverably") — a
/// structural pre-scan over the raw ast, so this rejects with
/// `error[type]` before generic statement-checking ever reaches either
/// construct in the defer body (where `await` would otherwise be
/// `error[unimplemented]` and `?` would be checked normally).
fn scan_defer_forbidden(body: &DeferBody) -> Option<(&'static str, Span)> {
    match body {
        DeferBody::Expr(e) => scan_expr_forbidden(e),
        DeferBody::Suite(stmts) => scan_stmts_forbidden(stmts),
    }
}

fn scan_stmts_forbidden(stmts: &[Stmt]) -> Option<(&'static str, Span)> {
    stmts.iter().find_map(scan_stmt_forbidden)
}

fn scan_stmt_forbidden(s: &Stmt) -> Option<(&'static str, Span)> {
    match s {
        Stmt::Assign(a) => scan_expr_forbidden(&a.target).or_else(|| scan_expr_forbidden(&a.value)),
        Stmt::If(i) => scan_expr_forbidden(&i.cond)
            .or_else(|| scan_stmts_forbidden(&i.then_branch))
            .or_else(|| {
                i.elifs.iter().find_map(|e| {
                    scan_expr_forbidden(&e.cond).or_else(|| scan_stmts_forbidden(&e.body))
                })
            })
            .or_else(|| i.else_branch.as_ref().and_then(|b| scan_stmts_forbidden(b))),
        Stmt::Match(m) => scan_expr_forbidden(&m.scrutinee).or_else(|| {
            m.arms.iter().find_map(|a| {
                a.guard
                    .as_ref()
                    .and_then(scan_expr_forbidden)
                    .or_else(|| scan_stmts_forbidden(&a.body))
            })
        }),
        Stmt::For(f) => scan_expr_forbidden(&f.iterable).or_else(|| scan_stmts_forbidden(&f.body)),
        Stmt::While(w) => scan_expr_forbidden(&w.cond).or_else(|| scan_stmts_forbidden(&w.body)),
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => None,
        Stmt::Return(_, e) => e.as_ref().and_then(scan_expr_forbidden),
        Stmt::Assert(a) => scan_expr_forbidden(&a.cond)
            .or_else(|| a.message.as_ref().and_then(scan_expr_forbidden)),
        Stmt::Defer(d) => scan_defer_forbidden(&d.body),
        Stmt::With(w) => scan_expr_forbidden(&w.expr).or_else(|| scan_stmts_forbidden(&w.body)),
        Stmt::Send(_, e) => scan_expr_forbidden(e),
        Stmt::Expr(_, e) => scan_expr_forbidden(e),
        Stmt::ComptimeIf(c) => scan_expr_forbidden(&c.cond)
            .or_else(|| scan_stmts_forbidden(&c.then_branch))
            .or_else(|| c.else_branch.as_ref().and_then(|b| scan_stmts_forbidden(b))),
        Stmt::ComptimeAssert(_, e, m) => {
            scan_expr_forbidden(e).or_else(|| m.as_ref().and_then(scan_expr_forbidden))
        }
    }
}

fn scan_expr_forbidden(e: &Expr) -> Option<(&'static str, Span)> {
    match e {
        Expr::Unary(span, UnaryOp::Await, _) => Some(("await", *span)),
        Expr::Try(span, _) => Some(("use `?`", *span)),
        Expr::Unary(_, _, inner) => scan_expr_forbidden(inner),
        Expr::Field(base, _, _) => scan_expr_forbidden(base),
        Expr::Index(base, _, args) => {
            scan_expr_forbidden(base).or_else(|| args.iter().find_map(scan_expr_forbidden))
        }
        Expr::Call(callee, _, args) => scan_expr_forbidden(callee)
            .or_else(|| args.iter().find_map(|a| scan_expr_forbidden(&a.value))),
        Expr::Binary(_, _, l, r) => scan_expr_forbidden(l).or_else(|| scan_expr_forbidden(r)),
        Expr::Range(_, a, b, _) => scan_expr_forbidden(a).or_else(|| scan_expr_forbidden(b)),
        Expr::Is(_, s, _) => scan_expr_forbidden(s),
        Expr::Not(_, i) => scan_expr_forbidden(i),
        Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            scan_expr_forbidden(l).or_else(|| scan_expr_forbidden(r))
        }
        Expr::DotVariant(_, _, args) => args.iter().find_map(|a| scan_expr_forbidden(&a.value)),
        Expr::Closure(c) => match &c.body {
            ClosureBody::Expr(e) => scan_expr_forbidden(e),
            ClosureBody::Suite(s) => scan_stmts_forbidden(s),
        },
        Expr::Send(_, i) => scan_expr_forbidden(i),
        Expr::Tuple(_, items) | Expr::List(_, items) => items.iter().find_map(scan_expr_forbidden),
        _ => None,
    }
}

// --- shared error helpers --------------------------------------------------

fn type_error(message: String, span: Span) -> SemaError {
    SemaError::at("type", message, span)
}

/// The `type` diagnostic for a missing method/operator method, tagged with
/// structured `(type_name, method_name)` metadata (`SemaError::missing_method`)
/// so `generics.rs`'s requirement-chain diagnostic (item H, decision 2) can
/// recognize this exact shape without parsing `message` back apart. The
/// rendered `message`/category/span are unaffected — the field is metadata
/// only, never printed.
fn missing_method_error(
    message: String,
    type_name: &str,
    method_name: &str,
    span: Span,
) -> SemaError {
    let mut e = type_error(message, span);
    e.missing_method = Some((type_name.to_string(), method_name.to_string()));
    e
}

fn arity_error(expected: usize, found: usize, span: Span) -> SemaError {
    type_error(
        format!("expected {expected} argument(s), found {found}"),
        span,
    )
}

// --- tests --------------------------------------------------------------
//
// 02-language.md §1.1: "Type comes from context; an unconstrained literal
// defaults to `i64` (or `u64` when only that fits). Float literals
// require a fractional part or exponent and default to `f64`." These
// pin `check_int_range`'s per-scalar boundaries and `synth_int_literal`'s
// defaulting directly (the narrowest callable units), rather than via a
// full `check()` run. `types_eq`'s span-insensitivity is pinned
// separately: a real M2 bug was two structurally identical types (same
// array length, different source spans) comparing unequal under derived
// `PartialEq`, which is exactly why this pass carries its own `types_eq`
// instead (see the doc comment above it).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_int_range_boundaries() {
        let span = Span::default();
        // (type, value, expect_ok)
        let cases: Vec<(&str, Type, i128, bool)> = vec![
            ("u8 min", Type::U8, 0, true),
            ("u8 max", Type::U8, 255, true),
            ("u8 above max", Type::U8, 256, false),
            ("u8 below min", Type::U8, -1, false),
            ("i8 min", Type::I8, -128, true),
            ("i8 max", Type::I8, 127, true),
            ("i8 above max", Type::I8, 128, false),
            ("i8 below min", Type::I8, -129, false),
            ("u64 min", Type::U64, 0, true),
            ("u64 max", Type::U64, u64::MAX as i128, true),
            ("u64 below min", Type::U64, -1, false),
            ("u64 above max", Type::U64, u64::MAX as i128 + 1, false),
            (
                "usize behaves like u64",
                Type::Usize,
                u64::MAX as i128,
                true,
            ),
            ("i64 min", Type::I64, i64::MIN as i128, true),
            ("i64 max", Type::I64, i64::MAX as i128, true),
            ("i64 above max", Type::I64, i64::MAX as i128 + 1, false),
            ("i64 below min", Type::I64, i64::MIN as i128 - 1, false),
            (
                "isize behaves like i64",
                Type::Isize,
                i64::MIN as i128,
                true,
            ),
        ];
        for (msg, ty, value, expect_ok) in cases {
            let result = check_int_range(value, &ty, span);
            assert_eq!(result.is_ok(), expect_ok, "{msg}: value {value}");
        }
    }

    /// An unconstrained integer literal (02-language.md §1.1: "an
    /// unconstrained literal defaults to `i64` (or `u64` when only that
    /// fits)"), exercised through `synth_int_literal` (the function that
    /// actually implements the defaulting) with `expected: None`.
    #[test]
    fn synth_int_literal_unconstrained_defaulting() {
        let span = Span::default();
        let small = synth_int_literal(span, "100", None).expect("fits i64");
        assert!(
            matches!(small.ty, Type::I64),
            "a small unconstrained literal defaults to i64, found {:?}",
            small.ty
        );

        let only_u64 = (i64::MAX as i128 + 1).to_string();
        let ty = synth_int_literal(span, &only_u64, None).expect("fits u64 only");
        assert!(
            matches!(ty.ty, Type::U64),
            "a literal beyond i64::MAX but within u64::MAX defaults to u64, found {:?}",
            ty.ty
        );

        let too_big = (u64::MAX as i128 + 1).to_string();
        assert!(
            synth_int_literal(span, &too_big, None).is_err(),
            "a literal beyond u64::MAX has no default type"
        );
    }

    /// `synth_int_literal` against an explicit expected integer type
    /// round-trips `check_int_range`'s verdict; against a non-integer
    /// expected type it is always rejected regardless of value.
    #[test]
    fn synth_int_literal_expected_type_cases() {
        let span = Span::default();
        assert!(synth_int_literal(span, "255", Some(&Type::U8)).is_ok());
        assert!(synth_int_literal(span, "256", Some(&Type::U8)).is_err());
        assert!(
            synth_int_literal(span, "0", Some(&Type::Bool)).is_err(),
            "an integer literal cannot check against a non-integer expected type"
        );
    }

    /// Float literals accept either float scalar and default to `f64`
    /// when unconstrained (02-language.md §1.1); no range check exists
    /// for floats (unlike integers) since `synth_float_literal` never
    /// calls anything like `check_int_range` — only the scalar kind is
    /// checked.
    #[test]
    fn synth_float_literal_cases() {
        let span = Span::default();
        assert!(matches!(
            synth_float_literal(span, "1.0", Some(&Type::F32)),
            Ok(TypedExpr { ty: Type::F32, .. })
        ));
        assert!(matches!(
            synth_float_literal(span, "1.0", Some(&Type::F64)),
            Ok(TypedExpr { ty: Type::F64, .. })
        ));
        assert!(
            synth_float_literal(span, "1.0", Some(&Type::U8)).is_err(),
            "a float literal cannot check against a non-float expected type"
        );
        assert!(matches!(
            synth_float_literal(span, "1.0", None),
            Ok(TypedExpr { ty: Type::F64, .. })
        ));
    }

    /// The real M2 bug `types_eq`'s own doc comment describes: two
    /// structurally identical types differing only in an embedded span
    /// (here, `[u8; 3]` written at two different source locations) must
    /// still compare equal — derived `PartialEq` on `Type`/`Expr` would
    /// not, since `Expr::Int`'s span is part of its derived equality.
    #[test]
    fn types_eq_is_span_insensitive() {
        let len_a = Expr::Int(Span { line: 1, col: 1 }, "3".to_string());
        let len_b = Expr::Int(Span { line: 42, col: 7 }, "3".to_string());
        let a = Type::Array(Box::new(Type::U8), Box::new(len_a.clone()));
        let b = Type::Array(Box::new(Type::U8), Box::new(len_b.clone()));
        assert!(
            types_eq(&a, &b),
            "[u8; 3] at two different spans must compare equal under types_eq"
        );
        // Sanity: derived (span-sensitive) equality does NOT consider
        // these equal, which is exactly why types_eq exists.
        assert_ne!(
            len_a, len_b,
            "the two length exprs differ by span under derived PartialEq"
        );

        // A different length is genuinely a different type.
        let len_c = Expr::Int(Span { line: 1, col: 1 }, "4".to_string());
        let c = Type::Array(Box::new(Type::U8), Box::new(len_c));
        assert!(
            !types_eq(&a, &c),
            "[u8; 3] and [u8; 4] must not compare equal"
        );

        // The same span-insensitivity applies to a Named type's generic
        // const argument.
        let named_a = Type::Named("Ring".to_string(), vec![types::TypeArg::Const(len_a)]);
        let named_b = Type::Named("Ring".to_string(), vec![types::TypeArg::Const(len_b)]);
        assert!(
            types_eq(&named_a, &named_b),
            "Ring[3] at two different spans must compare equal under types_eq"
        );
    }
}
