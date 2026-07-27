//! Statement/expression typing (plans/M2.md item C): assignment
//! introduction/reassignment, `if`/`while` condition typing, `for`
//! typing, operator desugar (02-language.md §7.4, §8.2, 05-library.md
//! §8), call checking (arity, labels), enum literals and leading-dot
//! inference, pattern typing, `is`, closures as structural `fn` types,
//! `?`, `assert`, `defer`. Also where the fail-closed set (decision 7)
//! beyond imports lands: `comptime if`/`comptime assert`, f-strings
//! (item D: desugar onto Format + `String` concat), `await`/`send`/
//! `with` (group/pool), `@image` bodies.
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
//! `mod.rs::check` runs it last). Generic-enum *variant* construction
//! under an expected instantiated type (`Lookup.Absent` → `Lookup[u32]`)
//! is included (plans/M9.md item J2c). What still fails closed via
//! `unimplemented_at("generic instantiation is", ...)` is item H's own
//! documented scope boundary: a generic *method*'s own type parameters
//! (beyond its struct's, if any — never instantiated; ledger gap
//! `sema.generics.method-params`), associated functions on a still-
//! generic enum type name, a bare reference to a generic type/fn as a
//! first-class value without calling it, and a generic-argument shape
//! deeper than this item resolves.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::sema::generics;
use crate::sema::typed::{
    CalleeKey, TestDecl, TestKind, TypedClosureBody, TypedClosureParam, TypedConst, TypedDeferBody,
    TypedElif, TypedEnum, TypedExpr, TypedExprKind, TypedFn, TypedForIter, TypedMatchArm,
    TypedParam, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt, TypedStmtKind,
    TypedStruct,
};
use crate::sema::types::{
    self, Classification, DeclMember, DeclParam, DeclVariantPayload, Type, TypeArg,
};
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{
    self, AccessMode, Arg, AssertStmt, AssignOp, AssignStmt, BinOp, ClosureBody, ClosureExpr,
    DeferBody, DeferStmt, Expr, ForStmt, IfStmt, Item, MatchStmt, Member, Module, NamedType,
    Pattern, Span, Stmt, UnaryOp, VariantPayload, WhileStmt, WithStmt,
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
    /// A method's (or associated fn's) own type/const parameters
    /// (plans/M13.md item Q): keyed `(receiver type, method, type-args)`.
    /// `QueuedInstantiation::name` is the method name; `receiver` holds
    /// the concrete `Type::Named` the call was made through.
    Method,
}

impl InstKind {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            InstKind::Struct => "struct",
            InstKind::Enum => "enum",
            InstKind::Fn => "fn",
            InstKind::Method => "method",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct QueuedInstantiation {
    pub(crate) kind: InstKind,
    pub(crate) name: String,
    pub(crate) args: Vec<types::TypeArg>,
    /// Concrete receiver type for `InstKind::Method`
    /// (`Type::Named(struct_or_enum, struct_args)`); `None` otherwise.
    pub(crate) receiver: Option<Type>,
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
    // =====================================================================
    // plans/M7.md item G, decision 18: `comptime if` members deferred by
    // `specialize` because they name this struct's own const generics
    // (e.g. `MODE == DriverMode.Irq`). Concrete `ast_members` stay 1:1 with
    // `decl.members` for the zip; instantiation expands these first.
    // =====================================================================
    pub(crate) deferred_comptime_members: Vec<Member>,
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

/// One enum's declared shape (plans/M9.md item B2): the resolved
/// `DeclEnum` plus its AST method members, parallel to `StructInfo` so
/// method/associated-fn lookup and body checking share one zip.
#[derive(Clone)]
pub(crate) struct EnumInfo {
    pub(crate) decl: types::DeclEnum,
    pub(crate) ast_members: Vec<Member>,
}

impl EnumInfo {
    pub(crate) fn members(&self) -> impl Iterator<Item = (&Member, &DeclMember)> {
        self.ast_members.iter().zip(self.decl.members.iter())
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

    pub(crate) fn has_member_named(&self, name: &str) -> bool {
        self.ast_members.iter().any(|m| match m {
            Member::Fn(f) => f.name == name,
            _ => false,
        })
    }
}

impl std::ops::Deref for EnumInfo {
    type Target = types::DeclEnum;
    fn deref(&self) -> &Self::Target {
        &self.decl
    }
}

impl std::ops::DerefMut for EnumInfo {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.decl
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
    pub(crate) enums: BTreeMap<String, EnumInfo>,
    pub(crate) fns: BTreeMap<String, FnInfo>,
    pub(crate) consts: BTreeMap<String, Type>,
    /// Module-level placed statics (03-hardware.md §3.1, plans/M10.md item
    /// A2c): name → (type, `@placed` address).
    pub(crate) statics: BTreeMap<String, StaticInfo>,
    /// Every `@layout` type this module declares, by name (plans/M7.md
    /// item C): `check_mmio_access` needs a register's declared direction,
    /// width and offset, and `types::check_layouts` is the one pass that
    /// computes them. Not spliced across an import, deliberately — an
    /// `Mmio[L]`'s own `L` must be an `@layout(mmio)` struct declared in
    /// the same module (`types::validate_capability_args`), so a layout
    /// from another module can never be reached through one.
    pub(crate) layouts: BTreeMap<String, types::LayoutType>,
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
    /// plans/M7.md item E1: every `VirtQueue.configure(pool=take P, ...,
    /// depth=N)` site observed while typing this module — `(pool name,
    /// depth)`. Layout/report read this from `TypedProgram` (copied at
    /// the end of `check`) so the ring geometry has one source of truth.
    pub(crate) virtqueue_configures: RefCell<Vec<(String, u16)>>,
    /// plans/M13.md item N: sync loops that omit `@budget`, pending the
    /// observation-discharge check after bodies are typed.
    pub(crate) unbounded_sync_loops: RefCell<Vec<crate::sema::typed::UnboundedSyncLoop>>,
    /// plans/M13.md item K: finalized return types for private
    /// `-> Result[T]` fns (and methods, keyed `Owner.method`), filled
    /// after each body is checked so a later caller sees the concrete
    /// error set rather than the declare-time marker.
    pub(crate) inferred_rets: RefCell<BTreeMap<String, Type>>,
}

/// One placed static's resolved type (plans/M10.md item A2c). Address lives
/// on `DeclStatic` / `TypedStatic`; ModuleCtx only needs the type for name
/// resolution in bodies.
#[derive(Debug, Clone)]
pub(crate) struct StaticInfo {
    pub(crate) ty: Type,
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
///
/// `imported` (plans/M9.md item A1) is the same imported-type arity table
/// `types::declare_with_imports` was given — `shapes` below *is* the
/// type-annotation arity table `ModuleCtx::resolve_type` reads, so
/// without it a `let x: ImportedType = ...` annotation inside a body
/// would still fail with `unknown type` after the signature positions had
/// been fixed. One table, one answer, both passes.
pub(crate) fn build_module_ctx(
    module: &Module,
    decl_items: &[types::DeclItem],
    imported: &types::ImportedTypes,
) -> ModuleCtx {
    let mut shapes: BTreeMap<String, usize> = imported.clone();
    let mut module_pools = BTreeSet::new();
    let mut structs = BTreeMap::new();
    let mut enums = BTreeMap::new();
    let mut fns = BTreeMap::new();
    let mut consts = BTreeMap::new();
    let mut statics = BTreeMap::new();
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
                // plans/M7.md item G, decision 18: keep deferred `comptime if`
                // members aside so `ast_members` stays 1:1 with `decl.members`.
                let mut ast_members = Vec::new();
                let mut deferred_comptime_members = Vec::new();
                for m in &s.members {
                    match m {
                        Member::ComptimeIf(_) => deferred_comptime_members.push(m.clone()),
                        other => ast_members.push(other.clone()),
                    }
                }
                // plans/M9.md item B3: Decl already carries the generated
                // `from`; append the matching FnItem so the zip stays 1:1.
                if s.deriving.iter().any(|d| d == "From") {
                    let field = s
                        .members
                        .iter()
                        .find_map(|m| match m {
                            Member::Field(f) => Some(f),
                            _ => None,
                        })
                        .expect("validate_from_shape already required exactly one field");
                    ast_members.push(Member::Fn(types::derived_from_fn_item_struct(
                        &s.name, field, s.span,
                    )));
                }
                // plans/M9.md item C2: matching FnItems for Decl's generated Format.
                if s.deriving.iter().any(|d| d == "Format") {
                    let fields: Vec<(String, Type)> = d
                        .members
                        .iter()
                        .filter_map(|m| match m {
                            types::DeclMember::Field(f) => Some((f.name.clone(), f.ty.clone())),
                            _ => None,
                        })
                        .collect();
                    let bound = types::struct_format_bound(&s.name, &fields, s.span)
                        .expect("declare already validated Format shape");
                    ast_members.push(Member::Fn(types::derived_max_formatted_len_fn_item(
                        bound, s.span,
                    )));
                    if fields.is_empty() {
                        ast_members.push(Member::Fn(types::derived_format_fn_item_struct(
                            &s.name, s.span,
                        )));
                    } else {
                        ast_members.push(Member::Fn(types::derived_format_fn_item_struct_fields(
                            &s.name, &fields, bound, s.span,
                        )));
                    }
                }
                structs.insert(
                    s.name.clone(),
                    StructInfo {
                        decl: d.clone(),
                        ast_members,
                        deferred_comptime_members,
                    },
                );
            }
            (Item::Enum(e), types::DeclItem::Enum(d)) => {
                shapes.insert(e.name.clone(), e.generics.len());
                let mut ast_members = e.members.clone();
                // plans/M9.md item B3: matching FnItem for Decl's generated `from`.
                if e.deriving.iter().any(|d| d == "From") {
                    let v = &e.variants[0];
                    let source_ty = match &v.payload {
                        VariantPayload::Tuple(types) => &types[0],
                        VariantPayload::Named(fields) => &fields[0].ty,
                        VariantPayload::None => {
                            unreachable!("validate_from_shape already required exactly one field")
                        }
                    };
                    ast_members.push(Member::Fn(types::derived_from_fn_item_enum(
                        &e.name, &v.name, source_ty, e.span,
                    )));
                }
                // plans/M9.md item C2: matching FnItems for Decl's generated Format.
                if e.deriving.iter().any(|d| d == "Format") {
                    let bound = e.variants.iter().map(|v| v.name.len()).max().unwrap_or(0) as u64;
                    let names: Vec<String> = e.variants.iter().map(|v| v.name.clone()).collect();
                    ast_members.push(Member::Fn(types::derived_max_formatted_len_fn_item(
                        bound, e.span,
                    )));
                    ast_members.push(Member::Fn(types::derived_format_fn_item_enum(
                        &names, bound, e.span,
                    )));
                }
                enums.insert(
                    e.name.clone(),
                    EnumInfo {
                        decl: d.clone(),
                        ast_members,
                    },
                );
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
            (Item::Static(s), types::DeclItem::Static(d)) => {
                statics.insert(s.name.clone(), StaticInfo { ty: d.ty.clone() });
            }
            (Item::Pool(p), types::DeclItem::Pool(_)) => {
                module_pools.insert(p.name.clone());
            }
            _ => unreachable!("declare()'s items must pair 1:1 with the filtered ast items"),
        }
    }

    // plans/M7.md item C: `check_layouts` is a pure function of the
    // already-specialized module and has *already run and succeeded* by
    // the time any caller in the real pipeline reaches here
    // (`sema::check_typed`/`check_program_typed` both call it first,
    // deliberately, before name resolution). Recomputing it is cheaper and
    // far less invasive than threading the table through every
    // `build_module_ctx` call site; `unwrap_or_default` covers the one
    // caller that builds a ctx outside that order (`specialize`'s own
    // const skeleton), where the worst it can do is lose a register — an
    // access then gets the ordinary named "declares no register"
    // rejection, never a silent accept.
    let layouts = types::check_layouts(module)
        .unwrap_or_default()
        .into_iter()
        .map(|l| (l.name.clone(), l))
        .collect();

    ModuleCtx {
        shapes,
        module_pools,
        structs,
        enums,
        fns,
        consts,
        statics,
        const_values,
        layouts,
        generics_queue: RefCell::new(BTreeMap::new()),
        current_chain: RefCell::new(Vec::new()),
        virtqueue_configures: RefCell::new(Vec::new()),
        unbounded_sync_loops: RefCell::new(Vec::new()),
        inferred_rets: RefCell::new(BTreeMap::new()),
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
    debug_assert_ne!(
        kind,
        InstKind::Method,
        "method instantiations use enqueue_method_instantiation"
    );
    enqueue_instantiation_inner(mctx, kind, name, args, None, call_span)
}

/// Enqueues a method-owned generic instantiation (plans/M13.md item Q).
/// Key is `method:{ReceiverType}.{method}[{args}]`.
pub(crate) fn enqueue_method_instantiation(
    mctx: &ModuleCtx,
    receiver: &Type,
    method: &str,
    args: &[types::TypeArg],
    call_span: Span,
) -> Result<String, SemaError> {
    enqueue_instantiation_inner(
        mctx,
        InstKind::Method,
        method,
        args,
        Some(receiver.clone()),
        call_span,
    )
}

fn enqueue_instantiation_inner(
    mctx: &ModuleCtx,
    kind: InstKind,
    name: &str,
    args: &[types::TypeArg],
    receiver: Option<Type>,
    call_span: Span,
) -> Result<String, SemaError> {
    let key = match (&kind, &receiver) {
        (InstKind::Method, Some(recv)) => generics::canonical_method_key(recv, name, args),
        _ => generics::canonical_key(kind, name, args),
    };
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
            receiver,
            chain,
        });
    Ok(key)
}

// --- per-body checking context -------------------------------------------

/// One function/method/init/closure body's typing state: the current
/// return type (for `return`/`?`), a local-variable scope stack — a
/// closure pushes a new one, and so does every non-closure suite below
/// the top of a function body (an `if`/`elif`/`else` branch, a
/// `while`/`for` body, a `match` arm: see `scoped`), mirroring
/// `symbols::Resolver`'s scope model and `lower.rs`'s own per-block
/// `LEnv` push/pop (found+fixed: err-mwir-if-else-scope-leak — a typed
/// declaration in each branch of an `if`/`else` was wrongly merged into
/// one binding because no scope was pushed per branch), and the pool
/// names visible by bare name inside `own[P]` annotations here (a
/// struct's own `pool` members, when checking one of its methods/init;
/// otherwise just the module's).
/// Widened to `pub(crate)` (item G, matches.rs): the exhaustiveness pass
/// re-derives scrutinee types by re-walking every body in lockstep with
/// this same scope model, so it needs the same local-variable state
/// `check_expr` reads/writes.
pub(crate) struct FnCtx {
    pub(crate) ret_ty: Type,
    locals: Vec<BTreeMap<String, Type>>,
    pub(crate) local_pools: BTreeSet<String>,
    /// Plans/M6.md item A: one `with group(...) as g:` block's own
    /// children so far — `g`'s bare name to `(unified child return type,
    /// count of static `g.start` call sites seen)`. Not scoped by
    /// `push_scope`/`pop_scope` (a separate, flat map: `g`'s own *type*
    /// binding already lives in `locals` and gets that ordinary scope
    /// discipline) — `bodies::check_with` removes this entry itself once
    /// the `with`-block's body is fully checked, so a *different*
    /// `with group(...) as g:` later in the same fn never sees a stale
    /// entry under the same reused variable name.
    pub(crate) group_children: BTreeMap<String, (Type, usize)>,
    /// Plans/M6.md item A: is the body currently being checked an
    /// `async fn`/method's own? `await`/`send`/`with group` all require
    /// this (the whole actor/async statement surface is only meaningful
    /// inside a body that can actually suspend — a plain `fn` "never
    /// suspends", 02-language.md §5) — set once, right after `FnCtx::new`,
    /// by `check_top_fn`/`check_struct_members`, never toggled mid-walk
    /// (a closure body shares its enclosing fn's own `fctx`, so a closure
    /// inside an async method still sees `in_async = true`, matching "a
    /// lending call is synchronous" being about cross-await *paths*,
    /// §9.2, not about whether `await` may textually appear there at all
    /// — out of scope to refine further at M6).
    pub(crate) in_async: bool,
    /// Bare function / method name for sync-loop discharge recording
    /// (plans/M13.md item N). Empty for const/field-default contexts.
    pub(crate) fn_name: String,
    /// plans/M8.md item G, decision 18: is the statement being checked
    /// inside a `match` arm that can match 03-hardware.md §9's
    /// `CompletionOutcome.Unknown`? Set by `check_match` for the duration
    /// of one arm's body and restored after — the one place §9's "Source
    /// must not auto-retry a non-idempotent operation on `Unknown`" has a
    /// site to attach to. Counted rather than flagged so nested matches
    /// (an `Unknown` arm containing another `match`) restore correctly.
    unknown_outcome_arms: usize,
    /// plans/M8.md item H attack 1: pool brand a `VirtQueue.recover` just
    /// quarantined on a named queue place, keyed by that place
    /// (`self.queue`, a local, …). `reclaim`'s `pool=`/`payload=` must
    /// match — the declaration otherwise mints an `own[P2]` handle whose
    /// bytes still belong to `P1` (04-compiler.md §1: "DMA ownership
    /// transitions are valid").
    ///
    /// **Scoped with `scoped()`, not by hand.** A brand recorded inside a
    /// conditionally executed region must not be visible after it — the
    /// first fix restored only across `match` arms and left `if`/`while`
    /// leaking (item H follow-up, 2026-07-25): `recover` inside `if` then
    /// `reclaim` outside typechecked, and a two-brand if/else overwrite
    /// re-opened the original hole at runtime. `scoped()` already bounds
    /// every `if`/`elif`/`else`/`while`/`for`/`match`-arm suite; folding
    /// the map into that push/pop is the one place a future construct
    /// cannot forget.
    quarantined_by_queue: BTreeMap<String, (String, String)>,
    /// plans/M13.md item K: when `ret_ty` is private `Result[T]` (error
    /// set still the declare-time marker), accumulate every `Err` /
    /// `?` error source reaching `return`. `None` for ordinary signatures.
    inferred_errors: Option<Vec<Type>>,
}

impl FnCtx {
    pub(crate) fn new(ret_ty: Type, local_pools: BTreeSet<String>) -> FnCtx {
        let inferred_errors = if types::is_inferred_result(&ret_ty) {
            Some(Vec::new())
        } else {
            None
        };
        FnCtx {
            ret_ty,
            locals: vec![BTreeMap::new()],
            local_pools,
            group_children: BTreeMap::new(),
            in_async: false,
            fn_name: String::new(),
            unknown_outcome_arms: 0,
            quarantined_by_queue: BTreeMap::new(),
            inferred_errors,
        }
    }

    fn record_inferred_error(&mut self, ty: Type) {
        if self.inferred_errors.is_none() {
            return;
        }
        if types::is_inferred_error_set(&ty) {
            return;
        }
        if matches!(ty, Type::Never) {
            return;
        }
        // Flatten a previously-inferred multi-member set so `?` on a
        // callee whose set is `A | B` contributes `A` and `B` separately.
        if let Type::Named(n, args) = &ty {
            if n == types::ERROR_SET_NAME {
                for a in args {
                    if let types::TypeArg::Type(t) = a {
                        self.record_inferred_error(t.clone());
                    }
                }
                return;
            }
        }
        let Some(v) = &mut self.inferred_errors else {
            return;
        };
        if v.iter().any(|e| types_eq(e, &ty)) {
            return;
        }
        v.push(ty);
    }

    /// Is the current position inside a `CompletionOutcome.Unknown` arm?
    pub(crate) fn in_unknown_outcome_arm(&self) -> bool {
        self.unknown_outcome_arms > 0
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

    /// plans/M7.md item E1: after `VirtQueue.configure(..., device=mut
    /// <local>, ...)` succeeds, the local's type becomes
    /// `QueuesConfiguredDevice[D]` — 03 §9's consuming transition, applied
    /// to a `mut` argument that survives the call (the docs' own spelling).
    pub(crate) fn retype_local(&mut self, name: &str, ty: Type) -> bool {
        for scope in self.locals.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), ty);
                return true;
            }
        }
        false
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

/// Runs `f` with a fresh innermost scope pushed, always popping it again
/// before returning — even on error, so a rejected body never leaves a
/// stray scope on the stack (only relevant because `mod.rs::check` keeps
/// checking further items after one body errors, in `check_program`'s
/// multi-module walk). Used for every non-closure suite that must NOT
/// leak its own typed (`name: T = value`) declarations sideways to a
/// sibling branch or past the construct: an `if`/`elif`/`else` branch, a
/// `while`/`for` body, a `match` arm's pattern-bindings + guard + body
/// (err-mwir-if-else-scope-leak, ledger `sema.init.definite`). This is
/// the one place `lower.rs`'s own per-block `LEnv` push/pop is mirrored
/// here — see `check_assign`'s doc comment for how a *plain* (untyped)
/// assignment still reaches an outer scope instead (the arm-merge idiom,
/// 02-language.md §8.1).
///
/// Also restores `quarantined_by_queue` (plans/M8.md item H): a
/// `VirtQueue.recover` brand recorded inside this suite must not be
/// visible to a `reclaim` after it. Same boundary as the name scope —
/// one push/pop, nothing for the next control-flow construct to forget.
pub(crate) fn scoped<T>(
    fctx: &mut FnCtx,
    f: impl FnOnce(&mut FnCtx) -> Result<T, SemaError>,
) -> Result<T, SemaError> {
    fctx.push_scope();
    let saved_quarantine = fctx.quarantined_by_queue.clone();
    let result = f(fctx);
    fctx.quarantined_by_queue = saved_quarantine;
    fctx.pop_scope();
    result
}

/// Binds `name` to `ty` in the current (innermost) scope: a plain insert
/// if this is the first binding here, an equality check if `name` is
/// already bound in *this same* scope (re-binding a match arm's own
/// pattern name, a `for` binding, or a typed declaration a second time
/// in the same block requires the same type — a dumb, sound stand-in for
/// real flow-sensitivity, which items E/F's flow pass owns). Since
/// `scoped` now pushes a fresh scope per `if`/`elif`/`else` branch,
/// `while`/`for` body, and `match` arm, this never sees a sibling
/// branch's own binding — only genuine same-block reuse. Reaching a
/// *different* branch's binding of the same name (the arm-merge idiom,
/// 02-language.md §8.1) is `check_assign`'s job for a plain, untyped
/// assignment, which looks outward across scopes instead of calling
/// this function — see its own doc comment.
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
    // plans/M4.md item C (`image.graph.pools-bound-once`/`seal-fully-bound`):
    // this module's own module-scoped `pool` declarations, kept verbatim
    // for the post-seal graph check (`eval::image_checks::check_pools_bound`)
    // to name one that never got bound — see `TypedProgram::declared_pools`'s
    // own doc comment.
    program.declared_pools = mctx.module_pools.clone();
    // plans/M4.md item B / plans/M9.md item I: the five stdlib enums
    // (`sema::stdlib_enums`) are injected into every module's own
    // `TypedProgram` unconditionally — the dumbest way to make
    // `eval::interp::variant_index` index `Target`/`Failure`/…
    // constructions with no evaluator-side special case. Variant
    // order comes from `stdlib/core/*.wr`. Harmless for a module that
    // never mentions them (this field is not part of the typed dump).
    for name in [
        "Target",
        "Failure",
        "BootError",
        // plans/M9.md item A2: `IoError` is no longer injected — it arrives
        // through the ordinary import splice from `stdlib/core/io_error.wr`.
        "DriverMode",
        // plans/M8.md item G: 03-hardware.md §9's `CompletionOutcome`.
        // Injected for the same reason as the five above — it is what
        // `lower::variant_index` reads to turn `case .Unknown:` into a
        // tag compare, with no lowering-side special case.
        "CompletionOutcome",
    ] {
        // plans/M9.md item QQ: load failures are `error[build]`, not panic.
        let variants = crate::sema::stdlib_enums::variant_strs(name)?
            .ok_or_else(|| {
                SemaError::at(
                    "build",
                    format!("stdlib enum `{name}` missing from the auto-visible table"),
                    Span::default(),
                )
            })?
            .iter()
            .map(|v| v.to_string())
            .collect();
        program
            .enums
            .insert(name.to_string(), TypedEnum::from_variants(variants));
    }
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
            (Item::Static(_), types::DeclItem::Static(d)) => {
                program.statics.insert(
                    d.name.clone(),
                    crate::sema::typed::TypedStatic {
                        ty: d.ty.clone(),
                        addr: d.addr,
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
                check_marker_attr_shape(f, true)?;
                let test_kind = test_attr_kind(f)?;
                if test_kind == Some(TestKind::Exhaustive) {
                    check_exhaustive_test_params(f, d, mctx)?;
                }
                if test_kind == Some(TestKind::Runtime) {
                    check_runtime_test_params(f, d)?;
                }
                // plans/M9.md item H: `@layout_assert` signature before
                // the body walk, same timing as `@test`/`@image` shape.
                check_layout_assert_fn(f, d, mctx)?;
                if let Some(tf) = check_top_fn(f, d, mctx)? {
                    if is_image_fn(f) {
                        // plans/M4.md item B's own minimal slice of
                        // decision 6 ("exactly one reachable `@image` in
                        // the closure"): this only catches two `@image`
                        // fns in the *same* module — the cross-module
                        // "zero, or more than one, across the whole
                        // build closure" case needs every module's own
                        // `TypedProgram` at once and is the `--stage=image`
                        // driver's own job (`bin/wrela.rs`); item C pins
                        // the full "list every candidate" diagnostic.
                        if let Some(existing) = &program.image_fn {
                            return Err(SemaError::at(
                                "build",
                                format!(
                                    "more than one `@image` fn in this module (`{existing}` and `{}`)",
                                    f.name
                                ),
                                f.span,
                            ));
                        }
                        program.image_fn = Some(f.name.clone());
                    }
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
                // M4-F sweep fix: `@test`/`@image` markers on a struct's
                // own fn members were silently ignored (the fn simply
                // never registered as a test or image candidate). Checked
                // on the raw ast so it fires for generic structs too,
                // whose bodies `check_struct_bodies` otherwise skips.
                for m in &s.members {
                    if let ast::Member::Fn(mf) = m {
                        check_marker_attr_shape(mf, false)?;
                    }
                }
                if let Some(ts) = check_struct_bodies(s, mctx)? {
                    program.structs.insert(s.name.clone(), ts);
                }
            }
            (Item::Enum(e), types::DeclItem::Enum(_d)) => {
                // A generic enum's own variant order is recorded once it
                // is instantiated (item H's job); a plain enum's is
                // recorded here, alongside every other plain top-level
                // declaration this pass checks (`typed::TypedProgram::enums`'s
                // own doc comment). Methods/associated fns (plans/M9.md
                // item B2) are checked into the same entry.
                if e.generics.is_empty() {
                    if let Some(te) = check_enum_bodies(e, mctx)? {
                        program.enums.insert(e.name.clone(), te);
                    }
                }
            }
            _ => {}
        }
    }
    // plans/M7.md item E1: hand the configure sites to layout/report.
    program.virtqueue_configures = mctx.virtqueue_configures.borrow().clone();
    // plans/M13.md item N: hand unbounded sync-loop sites to the
    // observation-discharge check in `sema::mod`.
    program.unbounded_sync_loops = mctx.unbounded_sync_loops.borrow().clone();
    Ok(program)
}

/// M4-F sweep fix (plans/M4.md item F): the `@test`/`@image`/
/// `@layout_assert` marker attributes were previously read through
/// `find`/`any`, so a duplicate (`@image @image fn ...`) or a conflicting
/// pair (`@test @image`) silently collapsed to one — a silent
/// approximation of a declaration shape the docs never define. The dumb
/// rule, pinned by goldens: at most one marker from the {`@test`,
/// `@image`, `@layout_assert`} family per fn, and the family is only
/// valid on a *top-level* fn — on a struct's method or assoc fn the
/// marker used to be ignored entirely (the fn just never registered),
/// which was a silent accept, not a decision. Category `type` (a bad
/// declaration shape), same as `test_attr_kind`'s own diagnostics.
pub(crate) fn check_marker_attr_shape(f: &ast::FnItem, top_level: bool) -> Result<(), SemaError> {
    let markers: Vec<&ast::Attr> = f
        .attrs
        .iter()
        .filter(|a| a.name == "test" || a.name == "image" || a.name == "layout_assert")
        .collect();
    if let Some(first) = markers.first() {
        if !top_level {
            return Err(type_error(
                format!(
                    "`@{}` is only valid on a top-level fn, not a struct member (`{}`)",
                    first.name, f.name
                ),
                first.span,
            ));
        }
        if markers.len() > 1 {
            return Err(type_error(
                format!(
                    "fn `{}` carries more than one `@test`/`@image`/`@layout_assert` marker \
                     attribute (`@{}` and `@{}`) — at most one is valid",
                    f.name, markers[0].name, markers[1].name
                ),
                markers[1].span,
            ));
        }
    }
    Ok(())
}

pub(crate) fn is_image_fn(f: &ast::FnItem) -> bool {
    f.attrs.iter().any(|a| a.name == "image")
}

pub(crate) fn is_layout_assert_fn(f: &ast::FnItem) -> bool {
    f.attrs.iter().any(|a| a.name == "layout_assert")
}

/// `@layout_assert` shape (plans/M9.md item H, 02-language.md §12.1):
/// exactly one plain (read) parameter whose type is the stdlib
/// `ImageReport` (named `ImageReport` after import, possibly aliased —
/// identified by the fixed field set decision 220 freezes), returning
/// `unit`. Attribute takes no arguments.
fn check_layout_assert_fn(
    f: &ast::FnItem,
    d: &types::DeclFn,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let Some(attr) = f.attrs.iter().find(|a| a.name == "layout_assert") else {
        return Ok(());
    };
    if !attr.args.is_empty() {
        return Err(type_error(
            "`@layout_assert` takes no arguments".to_string(),
            attr.span,
        ));
    }
    if d.params.len() != 1 {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}` must take exactly one parameter (`report: ImageReport`)",
                f.name
            ),
            f.span,
        ));
    }
    let p = &d.params[0];
    if p.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}`'s parameter `{}` must be a plain (read) parameter",
                f.name, p.name
            ),
            f.span,
        ));
    }
    let Type::Named(type_name, args) = &p.ty else {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}`'s parameter must have type `ImageReport`, found `{}`",
                f.name,
                types::render_type(&p.ty)
            ),
            f.span,
        ));
    };
    if !args.is_empty() {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}`'s parameter must have type `ImageReport`, found `{}`",
                f.name,
                types::render_type(&p.ty)
            ),
            f.span,
        ));
    }
    if !mctx_has_image_report(mctx, type_name) {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}`'s parameter type `{type_name}` is not the stdlib \
                 `ImageReport` (import it with `from core.image_report import ImageReport`)",
                f.name
            ),
            f.span,
        ));
    }
    if d.ret != Type::Unit {
        return Err(type_error(
            format!(
                "`@layout_assert` fn `{}` must return `unit`, found `{}`",
                f.name,
                types::render_type(&d.ret)
            ),
            f.span,
        ));
    }
    Ok(())
}

/// True when `type_name` resolves in this module to a struct with the
/// fixed `ImageReport` field set (decision 220) — the import's local
/// spelling, or a same-module declaration of that shape.
fn mctx_has_image_report(mctx: &ModuleCtx, type_name: &str) -> bool {
    const FIELDS: &[&str] = &[
        "machine_revision",
        "entry",
        "pages_base",
        "pages_size",
        "stacks_base",
        "stacks_size",
        "code_base",
        "code_size",
    ];
    let Some(info) = mctx.structs.get(type_name) else {
        return false;
    };
    let names: BTreeSet<&str> = info
        .decl
        .members
        .iter()
        .filter_map(|m| match m {
            DeclMember::Field(f) => Some(f.name.as_str()),
            _ => None,
        })
        .collect();
    FIELDS.iter().all(|n| names.contains(n)) && names.len() == FIELDS.len()
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
        TestKind::Comptime if !f.params.is_empty() => Err(type_error(
            format!("`@test` fn `{}` takes no arguments", f.name),
            f.span,
        )),
        // plans/M6.md decision 11b, 02-language.md §12.2 (added at item-D
        // verification): `@test(runtime)` may now declare `Actor[T]`
        // params — the runner supplies the image's unique declared `T`
        // instance's handle. Shape validation (mode `read`, type exactly
        // `Actor[T]`) runs against the *resolved* declaration
        // (`check_runtime_test_params`, below), mirroring
        // `check_exhaustive_test_params`'s own split between "arity here,
        // shape there" — this fn only sees raw `ast`.
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

/// `@test(runtime)`'s own parameter validation (02-language.md §12.2,
/// decision 11b): every parameter must be plain (`read` — a handle is
/// borrowed for the run, never mutated or consumed) and of type exactly
/// `Actor[T]`. `T` itself is already guaranteed to name a real
/// `@actor`/`@driver` struct by this point — `types::validate_actor_handles`
/// runs at declare time, over every fn's own resolved parameter types
/// (`validate_fn_actor_types`, unconditional, not test-specific), so this
/// fn only needs to confirm the *shape* (plain mode, bare `Actor[T]`, not
/// nested in an array/tuple/aggregate) — re-deriving "is `T` an actor" a
/// second time would duplicate a check that already ran.
fn check_runtime_test_params(f: &ast::FnItem, d: &types::DeclFn) -> Result<(), SemaError> {
    for p in &d.params {
        if p.mode != AccessMode::Read {
            return Err(type_error(
                format!(
                    "`@test(runtime)` fn `{}`'s parameter `{}` must be a plain (read) `Actor[T]` \
                     handle",
                    f.name, p.name
                ),
                f.span,
            ));
        }
        let is_handle =
            matches!(&p.ty, Type::Named(name, targs) if name == "Actor" && targs.len() == 1);
        if !is_handle {
            return Err(type_error(
                format!(
                    "`@test(runtime)` fn `{}`'s parameter `{}` must be an `Actor[T]` handle, \
                     found `{}`",
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
        // plans/M4.md item B: the fail-closed above is lifted — an
        // `@image` fn's body is ordinary comptime-legal code (checked
        // exactly like any other plain fn below) plus the builder
        // intrinsics 05-library.md §9 names (`check_call_by_name`/
        // `check_call_by_field`/`check_call_index`'s own new arms,
        // recognized by callee spelling, decision 5). The two shape
        // rules 02-language.md §12.1 states directly are checked here,
        // before the body walk, so a malformed `@image` declaration
        // fails with its own honest diagnostic rather than a confusing
        // one from deeper inside the ordinary body checker: it must be a
        // plain (non-generic) fn — a generic `@image` constructor is not
        // a documented shape, and generic instantiation of a "unique
        // reachable @image" makes no sense — and it must declare
        // `-> Image` (returning anything else can never be legal, since
        // `img.seal()` is the only producer of an `Image` value).
        if !f.generics.is_empty() {
            return Err(unimplemented_at("a generic `@image` fn is", f.span));
        }
        if d.ret != Type::Named("Image".to_string(), vec![]) {
            return Err(type_error(
                format!(
                    "`@image` fn `{}` must return `Image`, found `{}`",
                    f.name,
                    types::render_type(&d.ret)
                ),
                f.span,
            ));
        }
    }
    if !f.generics.is_empty() {
        return Ok(None); // generic body: item H's job, not checked here.
    }
    let mut fctx = FnCtx::new(d.ret.clone(), mctx.module_pools.clone());
    fctx.in_async = f.is_async;
    fctx.fn_name = f.name.clone();
    let params = check_params_with_defaults(&f.params, &d.params, &mut fctx, mctx)?;
    let body = match &f.body {
        Some(body) => check_stmts(body, &mut fctx, mctx)?,
        // The parser accepts the bodyless signature shorthand a few doc
        // tables use; whether a real declaration may be bodyless is a
        // later milestone's question (see parse_fn_tail), so sema fails
        // closed rather than treating it as an empty body.
        None => return Err(unimplemented_at("bodyless functions are", f.span)),
    };
    if f.is_async {
        check_cross_await(&body)?;
    }
    if d.is_task {
        return Err(type_error(
            format!(
                "`@task` is only valid on a `@driver` method (03-hardware.md §6's bottom half); \
                 top-level fn `{}` cannot carry it",
                f.name
            ),
            f.span,
        ));
    }
    let ret = finalize_inferred_ret(&d.ret, fctx.inferred_errors, &f.name, None, mctx);
    Ok(Some(TypedFn {
        receiver: None,
        params,
        ret,
        body,
        is_async: f.is_async,
        is_task: false,
        is_layout_assert: is_layout_assert_fn(f),
        is_pub: f.is_pub,
    }))
}

/// plans/M13.md item K: replace the declare-time `Result[T, <inferred>]`
/// marker with the union of collected `Err`/`?` sources, and publish the
/// concrete return type for later callers in the same module.
fn finalize_inferred_ret(
    declared: &Type,
    inferred_errors: Option<Vec<Type>>,
    fn_name: &str,
    owner: Option<&str>,
    mctx: &ModuleCtx,
) -> Type {
    let Some(errs) = inferred_errors else {
        return declared.clone();
    };
    let Type::Result(ok, err) = declared else {
        return declared.clone();
    };
    if !types::is_inferred_error_set(err) {
        return declared.clone();
    }
    let err_ty = types::finalize_error_set(errs);
    let ret = Type::Result(ok.clone(), Box::new(err_ty));
    let key = inferred_ret_key(owner, fn_name);
    mctx.inferred_rets.borrow_mut().insert(key, ret.clone());
    ret
}

fn inferred_ret_key(owner: Option<&str>, fn_name: &str) -> String {
    match owner {
        Some(o) => format!("{o}.{fn_name}"),
        None => fn_name.to_string(),
    }
}

fn resolved_ret(declared: &Type, owner: Option<&str>, fn_name: &str, mctx: &ModuleCtx) -> Type {
    mctx.inferred_rets
        .borrow()
        .get(&inferred_ret_key(owner, fn_name))
        .cloned()
        .unwrap_or_else(|| declared.clone())
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

/// plans/M9.md item B2: check an enum's methods/associated fns into a
/// `TypedEnum`, same body rules as `check_struct_bodies` (02 §5 applies
/// unchanged). Variants themselves have no bodies.
fn check_enum_bodies(e: &ast::EnumItem, mctx: &ModuleCtx) -> Result<Option<TypedEnum>, SemaError> {
    if !e.generics.is_empty() {
        return Ok(None);
    }
    let info = mctx.enums.get(&e.name).expect("enum present in mctx");
    let self_ty = Type::Named(e.name.clone(), vec![]);
    let mut methods = BTreeMap::new();
    let mut assoc_fns = BTreeMap::new();
    for (am, dm) in info.members() {
        let (Member::Fn(f), DeclMember::Fn(fd)) = (am, dm) else {
            continue;
        };
        if !f.generics.is_empty() {
            continue; // generic method: same boundary as structs.
        }
        if f.is_async && f.receiver.is_none() {
            return Err(unimplemented_at(
                "an `async fn` with no receiver (associated fn) is",
                f.span,
            ));
        }
        let mut fctx = FnCtx::new(fd.ret.clone(), mctx.module_pools.clone());
        fctx.in_async = f.is_async;
        fctx.fn_name = f.name.clone();
        fctx.insert_local("self".to_string(), self_ty.clone());
        let params = check_params_with_defaults(&f.params, &fd.params, &mut fctx, mctx)?;
        let body = match &f.body {
            Some(body) => check_stmts(body, &mut fctx, mctx)?,
            None => return Err(unimplemented_at("bodyless functions are", f.span)),
        };
        if f.is_async {
            check_cross_await(&body)?;
        }
        if fd.is_task {
            return Err(type_error(
                format!(
                    "`@task` is only valid on a `@driver` method (03-hardware.md §6); \
                     `{}.{}` is an enum method",
                    e.name, f.name
                ),
                f.span,
            ));
        }
        let receiver = f.receiver.as_ref().map(|r| (r.mode, self_ty.clone()));
        let ret =
            finalize_inferred_ret(&fd.ret, fctx.inferred_errors, &f.name, Some(&e.name), mctx);
        let tf = TypedFn {
            receiver,
            params,
            ret,
            body,
            is_async: f.is_async,
            is_task: fd.is_task,
            is_layout_assert: false,
            is_pub: f.is_pub,
        };
        if f.receiver.is_some() {
            methods.insert(f.name.clone(), tf);
        } else {
            assoc_fns.insert(f.name.clone(), tf);
        }
    }
    // plans/M9.md item JJ: carry payload types so the importer's typed
    // reachability closure can walk `Good(Payload)` the same way Decl-
    // side already walked ModuleCtx.
    let variant_payload_types = info
        .variants
        .iter()
        .map(|v| match &v.payload {
            DeclVariantPayload::None => Vec::new(),
            DeclVariantPayload::Tuple(tys) => tys.clone(),
            DeclVariantPayload::Named(fields) => fields.iter().map(|(_, t)| t.clone()).collect(),
        })
        .collect();
    validate_format_contract(&e.name, &methods, &assoc_fns, e.span)?;
    Ok(Some(TypedEnum {
        variants: info.variants.iter().map(|v| v.name.clone()).collect(),
        variant_payload_types,
        methods,
        assoc_fns,
    }))
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
    let mut field_types = BTreeMap::new();
    let mut field_defaults = BTreeMap::new();
    let mut methods = BTreeMap::new();
    let mut assoc_fns = BTreeMap::new();
    let mut init = None;
    for (am, dm) in info.members() {
        match (am, dm) {
            (Member::Field(af), DeclMember::Field(df)) => {
                fields.push(af.name.clone());
                // The already-resolved declared type, kept by name
                // (`TypedStruct::field_types`'s own doc comment) — the
                // same `df.ty` this arm already uses as the expected type
                // for the field's own default, just below.
                field_types.insert(af.name.clone(), df.ty.clone());
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
                // Plans/M6.md item A: an `async fn` with no receiver
                // (an associated fn) is not a documented shape — 02
                // §9.1/§9.5's whole async surface is methods (through
                // `self` or `Actor[T]`) and top-level fns (group
                // children); an associated fn is neither. Fail closed,
                // named, rather than silently accepting an uncallable
                // declaration.
                if f.is_async && f.receiver.is_none() {
                    return Err(unimplemented_at(
                        "an `async fn` with no receiver (associated fn) is",
                        f.span,
                    ));
                }
                let mut fctx = FnCtx::new(fd.ret.clone(), local_pools.clone());
                fctx.in_async = f.is_async;
                fctx.fn_name = f.name.clone();
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
                if f.is_async {
                    check_cross_await(&body)?;
                }
                if fd.is_task {
                    if !info.decl.is_driver {
                        return Err(type_error(
                            format!(
                                "`@task` is only valid on a `@driver` method (03-hardware.md §6); \
                                 `{struct_name}` is not a `@driver`"
                            ),
                            f.span,
                        ));
                    }
                    if f.is_async {
                        return Err(type_error(
                            format!(
                                "`@task` `{struct_name}.{}` must be a plain `fn`, not `async fn` \
                                 (03-hardware.md §6: the bottom half never stays active while \
                                 waiting)",
                                f.name
                            ),
                            f.span,
                        ));
                    }
                    if f.receiver.is_none() {
                        return Err(type_error(
                            format!(
                                "`@task` `{struct_name}.{}` must be a method with a `self` receiver",
                                f.name
                            ),
                            f.span,
                        ));
                    }
                }
                let receiver = f.receiver.as_ref().map(|r| (r.mode, self_ty.clone()));
                let ret = finalize_inferred_ret(
                    &fd.ret,
                    fctx.inferred_errors,
                    &f.name,
                    Some(&struct_name),
                    mctx,
                );
                let tf = TypedFn {
                    receiver,
                    params,
                    ret,
                    body,
                    is_async: f.is_async,
                    is_task: fd.is_task,
                    is_layout_assert: false,
                    is_pub: f.is_pub,
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
                let ret = finalize_inferred_ret(
                    &fd.ret,
                    fctx.inferred_errors,
                    "init",
                    Some(&struct_name),
                    mctx,
                );
                init = Some(TypedFn {
                    receiver: Some((i.receiver.mode, self_ty.clone())),
                    params,
                    ret,
                    body,
                    is_async: false,
                    is_task: false,
                    is_layout_assert: false,
                    is_pub: false,
                });
            }
            _ => {}
        }
    }
    validate_format_contract(&struct_name, &methods, &assoc_fns, info.decl.span)?;
    Ok(TypedStruct {
        name: struct_name,
        fields,
        field_types,
        field_defaults,
        methods,
        assoc_fns,
        init,
        is_actor: info.decl.is_actor,
        is_driver: info.decl.is_driver,
    })
}

/// plans/M9.md item C2: when both Format contract members are present
/// with the exact signatures (05 §6), prove the writer's max occupied
/// length against `max_formatted_len`'s literal bound. A partial pair
/// (wrong signature / only one name) is ordinary methods, not Format.
fn validate_format_contract(
    type_name: &str,
    methods: &BTreeMap<String, TypedFn>,
    assoc_fns: &BTreeMap<String, TypedFn>,
    span: Span,
) -> Result<(), SemaError> {
    let Some(max_fn) = assoc_fns.get("max_formatted_len") else {
        return Ok(());
    };
    let Some(fmt_fn) = methods.get("format") else {
        return Ok(());
    };
    if !typed_is_format_max(max_fn) || !typed_is_format_writer(fmt_fn) {
        return Ok(());
    }
    if type_name == "Secret" {
        return Err(types::secret_has_no_format(span));
    }
    let bound = format_bound_from_body(&max_fn.body, span)?;
    if !string_capacity_fits(i128::from(bound)) {
        return Err(type_error(
            format!("Format max_formatted_len bound {bound} is out of range for `String[..N]`"),
            span,
        ));
    }
    let Type::String(n_expr) = &fmt_fn.ret else {
        return Err(type_error(
            "Format.format must return `String[..N]`".to_string(),
            span,
        ));
    };
    let ret_n = literal_array_len(n_expr).ok_or_else(|| {
        type_error(
            "Format.format return capacity must be a literal".to_string(),
            span,
        )
    })?;
    let ret_n = u64::try_from(ret_n).map_err(|_| {
        type_error(
            "Format.format return capacity is out of range".to_string(),
            span,
        )
    })?;
    if ret_n != bound {
        return Err(type_error(
            format!("Format.format returns `String[..{ret_n}]` but max_formatted_len is {bound}"),
            span,
        ));
    }
    check_format_writer_against_bound(&fmt_fn.body, bound, span)
}

fn typed_is_format_max(f: &TypedFn) -> bool {
    f.receiver.is_none() && f.params.is_empty() && f.ret == Type::Usize && !f.is_async
}

fn typed_is_format_writer(f: &TypedFn) -> bool {
    matches!(&f.receiver, Some((AccessMode::Read, _)))
        && f.params.is_empty()
        && matches!(f.ret, Type::String(_))
        && !f.is_async
}

fn format_bound_from_body(body: &[TypedStmt], span: Span) -> Result<u64, SemaError> {
    let mut bound: Option<u64> = None;
    collect_format_bound_returns(body, span, &mut |v| {
        match bound {
            None => bound = Some(v),
            Some(b) if b == v => {}
            Some(b) => {
                return Err(type_error(
                    format!("Format max_formatted_len returns disagreeing bounds ({b} vs {v})"),
                    span,
                ));
            }
        }
        Ok(())
    })?;
    bound.ok_or_else(|| {
        type_error(
            "Format max_formatted_len body must return an integer literal so the bound can be proven"
                .to_string(),
            span,
        )
    })
}

fn check_format_writer_against_bound(
    body: &[TypedStmt],
    bound: u64,
    span: Span,
) -> Result<(), SemaError> {
    let mut saw = false;
    collect_format_string_returns(body, span, &mut |need| {
        saw = true;
        if need > bound {
            return Err(type_error(
                format!("Format.format exceeds proven max_formatted_len bound ({need} > {bound})"),
                span,
            ));
        }
        Ok(())
    })?;
    if !saw {
        return Err(type_error(
            "Format.format body must return a string expression whose bound can be proven"
                .to_string(),
            span,
        ));
    }
    Ok(())
}

fn collect_format_bound_returns(
    body: &[TypedStmt],
    span: Span,
    on_ret: &mut dyn FnMut(u64) -> Result<(), SemaError>,
) -> Result<(), SemaError> {
    for s in body {
        match &s.kind {
            TypedStmtKind::Return(Some(e)) => match &e.kind {
                TypedExprKind::Int(text) => {
                    let v = crate::eval::value::parse_int_literal(text).ok_or_else(|| {
                        type_error(
                            "Format max_formatted_len must return an integer literal".to_string(),
                            span,
                        )
                    })?;
                    if v < 0 {
                        return Err(type_error(
                            "Format max_formatted_len must return a non-negative integer"
                                .to_string(),
                            span,
                        ));
                    }
                    on_ret(v as u64)?;
                }
                _ => {
                    return Err(type_error(
                        "Format max_formatted_len body must return an integer literal so the bound can be proven"
                            .to_string(),
                        span,
                    ));
                }
            },
            TypedStmtKind::Return(None) => {
                return Err(type_error(
                    "Format max_formatted_len body must return an integer literal so the bound can be proven"
                        .to_string(),
                    span,
                ));
            }
            TypedStmtKind::Match { arms, .. } => {
                for arm in arms {
                    collect_format_bound_returns(&arm.body, span, on_ret)?;
                }
            }
            TypedStmtKind::If {
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                collect_format_bound_returns(then_branch, span, on_ret)?;
                for e in elifs {
                    collect_format_bound_returns(&e.body, span, on_ret)?;
                }
                if let Some(eb) = else_branch {
                    collect_format_bound_returns(eb, span, on_ret)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Max occupied length of a Format writer return expression.
fn format_expr_max_len(e: &TypedExpr) -> Result<u64, SemaError> {
    match &e.kind {
        TypedExprKind::Str(text) => Ok(crate::eval::value::decode_str(text).len() as u64),
        TypedExprKind::Binary(BinOp::Add, l, r) => {
            Ok(format_expr_max_len(l)? + format_expr_max_len(r)?)
        }
        TypedExprKind::Call {
            callee: CalleeKey::Method(_, m),
            ..
        } if m == "format" => match &e.ty {
            Type::String(n) => {
                let n = literal_array_len(n).ok_or_else(|| {
                    type_error(
                        "Format.format return capacity must be a literal".to_string(),
                        Span::default(),
                    )
                })?;
                u64::try_from(n).map_err(|_| {
                    type_error(
                        "Format.format return capacity is out of range".to_string(),
                        Span::default(),
                    )
                })
            }
            _ => Err(type_error(
                "Format.format call must return `String[..N]`".to_string(),
                Span::default(),
            )),
        },
        _ => Err(type_error(
            "Format.format body must return a string literal, string `+`, or `.format()` call so the bound can be proven"
                .to_string(),
            Span::default(),
        )),
    }
}

fn collect_format_string_returns(
    body: &[TypedStmt],
    span: Span,
    on_ret: &mut dyn FnMut(u64) -> Result<(), SemaError>,
) -> Result<(), SemaError> {
    for s in body {
        match &s.kind {
            TypedStmtKind::Return(Some(e)) => {
                let need = format_expr_max_len(e).map_err(|err| type_error(err.message, span))?;
                on_ret(need)?;
            }
            TypedStmtKind::Return(None) => {
                return Err(type_error(
                    "Format.format body must return a string expression whose bound can be proven"
                        .to_string(),
                    span,
                ));
            }
            TypedStmtKind::Match { arms, .. } => {
                for arm in arms {
                    collect_format_string_returns(&arm.body, span, on_ret)?;
                }
            }
            TypedStmtKind::If {
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                collect_format_string_returns(then_branch, span, on_ret)?;
                for e in elifs {
                    collect_format_string_returns(&e.body, span, on_ret)?;
                }
                if let Some(eb) = else_branch {
                    collect_format_string_returns(eb, span, on_ret)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
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
        Stmt::With(w) => check_with(w, fctx, mctx),
        Stmt::Send(span, e) => check_send_stmt(*span, e, fctx, mctx),
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
            // F-strings type as `String[..N]` (item D); assert messages
            // stay literal-only so lower can bake a fixed `AssertFail`
            // payload.
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

/// Each branch is its own scope (`scoped`): a typed declaration made in
/// `then_branch` must not be visible in an `elif`/`else` sibling, nor
/// survive past the whole `if` (err-mwir-if-else-scope-leak). A *plain*
/// (untyped) assignment reusing an outer name still crosses branches
/// fine — `check_assign` reaches outward past whatever scope `scoped`
/// pushed, so the arm-merge idiom (02-language.md §8.1) is unaffected.
fn check_if(i: &IfStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let cond = check_expr(&i.cond, Some(&Type::Bool), fctx, mctx)?;
    let then_branch = scoped(fctx, |fctx| check_stmts(&i.then_branch, fctx, mctx))?;
    let mut elifs = Vec::with_capacity(i.elifs.len());
    for elif in &i.elifs {
        let ec = check_expr(&elif.cond, Some(&Type::Bool), fctx, mctx)?;
        let eb = scoped(fctx, |fctx| check_stmts(&elif.body, fctx, mctx))?;
        elifs.push(TypedElif { cond: ec, body: eb });
    }
    let else_branch = match &i.else_branch {
        Some(b) => Some(scoped(fctx, |fctx| check_stmts(b, fctx, mctx))?),
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
    let budget = resolve_loop_budget(w.budget.as_ref(), w.span, fctx, mctx)?;
    let cond = check_expr(&w.cond, Some(&Type::Bool), fctx, mctx)?;
    let body = scoped(fctx, |fctx| check_stmts(&w.body, fctx, mctx))?;
    Ok(TypedStmt {
        kind: TypedStmtKind::While { cond, body, budget },
    })
}

fn check_match(m: &MatchStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let scrutinee = check_expr(&m.scrutinee, None, fctx, mctx)?;
    let sty = scrutinee.ty.clone();
    // plans/M8.md item G, decision 18: matching 03-hardware.md §9's
    // `CompletionOutcome` is the only place the no-auto-retry rule has a
    // site. Anything *other* than a `CompletionOutcome` scrutinee leaves
    // the flag alone entirely.
    let outcome_match = matches!(&sty, Type::Named(n, targs)
        if n == "CompletionOutcome" && targs.is_empty());
    let mut arms = Vec::with_capacity(m.arms.len());
    for arm in &m.arms {
        // Pattern bindings, guard, and body all share one pushed scope
        // per arm: a binding from one arm's pattern must not leak into a
        // sibling arm or past the whole `match`, exactly like an
        // `if`/`elif`/`else` branch. `scoped()` also restores the
        // reclaim-quarantine map (plans/M8.md item H).
        let unknown_arm = outcome_match && pattern_can_match_unknown(&arm.pattern);
        if unknown_arm {
            fctx.unknown_outcome_arms += 1;
        }
        let checked = scoped(fctx, |fctx| {
            let pattern = check_pattern(&arm.pattern, &sty, fctx, mctx)?;
            let guard = match &arm.guard {
                Some(g) => Some(check_expr(g, Some(&Type::Bool), fctx, mctx)?),
                None => None,
            };
            let body = check_stmts(&arm.body, fctx, mctx)?;
            Ok((pattern, guard, body))
        });
        if unknown_arm {
            fctx.unknown_outcome_arms -= 1;
        }
        let (pattern, guard, body) = checked?;
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

/// plans/M8.md item G, decision 18: can this arm's pattern match
/// `CompletionOutcome.Unknown`? Deliberately **over**-approximate — a
/// wildcard or a plain binding covers `Unknown` just as surely as
/// `case .Unknown:` does, and 03-hardware.md §9's rule is about the value
/// the arm may be looking at, not about how the author spelled it. Only a
/// variant pattern that names one of the other two arms is excluded.
fn pattern_can_match_unknown(p: &Pattern) -> bool {
    match p {
        Pattern::Wildcard(_) | Pattern::Binding(_, _) => true,
        Pattern::Take(_, inner) => pattern_can_match_unknown(inner),
        Pattern::Or(_, alts) => alts.iter().any(pattern_can_match_unknown),
        Pattern::Variant { variant, .. } => variant == "Unknown",
        // A literal / tuple / array pattern against a fieldless enum is
        // already a type error; answering `false` here changes nothing.
        Pattern::Literal(_, _) | Pattern::Tuple(_, _) | Pattern::Array(_, _) => false,
    }
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
            // F-strings type as `String[..N]` (item D); assert messages
            // stay literal-only so lower can bake a fixed `AssertFail`
            // payload.
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
            if is_untrusted_type(&ft.ty) || is_untrusted_type(&tt.ty) {
                let bad = if is_untrusted_type(&ft.ty) {
                    from.span()
                } else {
                    to.span()
                };
                return Err(untrusted_use_error("a range bound", bad));
            }
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
    // The loop binding and any typed declaration inside the body are
    // scoped to the body itself, same as `if`/`while`/`match` — see
    // `scoped`'s doc comment.
    let body = scoped(fctx, |fctx| {
        bind_local(fctx, &f.name, elem_ty.clone(), f.span)?;
        check_stmts(&f.body, fctx, mctx)
    })?;
    let budget = resolve_loop_budget(f.budget.as_ref(), f.span, fctx, mctx)?;
    Ok(TypedStmt {
        kind: TypedStmtKind::For {
            name: f.name.clone(),
            elem_ty,
            take_binding: f.take_binding,
            iter,
            body,
            budget,
        },
    })
}

/// Sync-loop `@budget(bound=N)` / observation discharge (02 §8.1,
/// plans/M11.md decision 721; plans/M12.md item B; plans/M13.md item N /
/// decision 11).
///
/// - Sync (`!in_async`): `@budget` yields `Some(N)` for the trip counter;
///   omitting it records the site for the post-body observation-discharge
///   check (every head→back-edge path must observe) and returns `None`.
/// - Async: attribute optional (checkpoint path unchanged); returns `None`
///   so no trip counter is emitted. A present attribute is still shape-
///   checked so a typo fails closed.
fn resolve_loop_budget(
    budget: Option<&ast::Attr>,
    loop_span: Span,
    fctx: &FnCtx,
    mctx: &ModuleCtx,
) -> Result<Option<u64>, SemaError> {
    match budget {
        None => {
            if fctx.in_async {
                Ok(None)
            } else {
                let mut sites = mctx.unbounded_sync_loops.borrow_mut();
                let ordinal = sites.iter().filter(|s| s.fn_name == fctx.fn_name).count();
                sites.push(crate::sema::typed::UnboundedSyncLoop {
                    fn_name: fctx.fn_name.clone(),
                    span: loop_span,
                    ordinal,
                });
                Ok(None)
            }
        }
        Some(attr) => {
            let n = parse_budget_bound_attr(attr, mctx)?;
            if fctx.in_async {
                // Async half stays a gap: keep checkpoint behaviour; do not
                // emit a trip counter from this attribute yet.
                Ok(None)
            } else {
                Ok(Some(n))
            }
        }
    }
}

fn parse_budget_bound_attr(attr: &ast::Attr, mctx: &ModuleCtx) -> Result<u64, SemaError> {
    if attr.name != "budget" {
        return Err(SemaError::at(
            "sema",
            format!(
                "only `@budget(bound=N)` may annotate a loop; found `@{}`",
                attr.name
            ),
            attr.span,
        ));
    }
    if attr.args.len() != 1 {
        return Err(SemaError::at(
            "sema",
            "`@budget` on a loop takes exactly one argument `bound=N` (02-language.md §8.1)"
                .to_string(),
            attr.span,
        ));
    }
    let arg = &attr.args[0];
    match &arg.label {
        Some(label) if label == "bound" => {}
        Some(other) => {
            return Err(SemaError::at(
                "sema",
                format!(
                    "`@budget` on a loop takes `bound=N`; found `{other}=` (02-language.md §8.1)"
                ),
                arg.span,
            ));
        }
        None => {
            return Err(SemaError::at(
                "sema",
                "`@budget` on a loop takes `bound=N` (labeled); a positional argument is not the sync-loop discharge (02-language.md §8.1)"
                    .to_string(),
                arg.span,
            ));
        }
    }
    if arg.mode != AccessMode::Read {
        return Err(SemaError::at(
            "sema",
            "`@budget(bound=N)`'s `N` is a comptime integer, not a `mut`/`take` place".to_string(),
            arg.span,
        ));
    }
    match &arg.value {
        Expr::Int(span, text) => {
            let n: i128 = text.parse().map_err(|_| {
                SemaError::at(
                    "sema",
                    format!("`@budget(bound=N)` requires an integer literal; found `{text}`"),
                    *span,
                )
            })?;
            budget_bound_from_i128(n, *span)
        }
        Expr::Name(span, name) => budget_bound_from_const_name(name, *span, attr.span, mctx),
        other => Err(SemaError::at(
            "sema",
            "`@budget(bound=N)` requires a comptime-known integer literal or the name of a \
             module-level `const` whose comptime value is one or more for N \
             (02-language.md §8.1, 03-hardware.md §3.1)"
                .to_string(),
            other.span(),
        )),
    }
}

/// Resolve `@budget(bound=NAME)` via the same maps layout lengths use:
/// `mctx.consts` / `mctx.const_values` (imported consts are already spliced).
/// Value rule matches `collect_length_consts` (integer comptime ≥ 1).
fn budget_bound_from_const_name(
    name: &str,
    name_span: Span,
    attr_span: Span,
    mctx: &ModuleCtx,
) -> Result<u64, SemaError> {
    let Some(ty) = mctx.consts.get(name) else {
        return Err(SemaError::at(
            "sema",
            format!(
                "`@budget(bound=N)`'s `N` is `{name}`, which is not a module-level `const` \
                 visible here; a loop bound is an integer literal or the name of a \
                 module-level `const` whose comptime value is one or more — a name a \
                 `comptime if` removed, a local, or a type is not one (02-language.md §8.1, \
                 03-hardware.md §3.1)"
            ),
            attr_span,
        ));
    };
    if !is_integer_scalar(ty) {
        return Err(SemaError::at(
            "sema",
            format!(
                "`@budget(bound=N)`'s `N` is `{name}`, whose type is not an integer; a loop \
                 bound is a count of trips (02-language.md §8.1, 03-hardware.md §3.1)"
            ),
            attr_span,
        ));
    }
    let Some(init) = mctx.const_values.get(name) else {
        return Err(SemaError::at(
            "sema",
            format!(
                "`@budget(bound=N)`'s `N` is `{name}`, which is not a module-level `const` \
                 visible here; a loop bound is an integer literal or the name of a \
                 module-level `const` whose comptime value is one or more (02-language.md \
                 §8.1, 03-hardware.md §3.1)"
            ),
            attr_span,
        ));
    };
    let n = match init {
        Expr::Int(_, text) => parse_int_literal(text).ok_or_else(|| {
            SemaError::at(
                "sema",
                format!(
                    "`@budget(bound=N)`'s `N` is `{name}`, whose value `{text}` is not an \
                     integer literal (02-language.md §8.1)"
                ),
                attr_span,
            )
        })?,
        // Chase a const whose initializer is another const name (same maps;
        // no second evaluator — layout's full `eval_const` path is for the
        // post-typing completion pass).
        Expr::Name(_, other) => {
            return budget_bound_from_const_name(other, name_span, attr_span, mctx);
        }
        _ => {
            return Err(SemaError::at(
                "sema",
                format!(
                    "`@budget(bound=N)`'s `N` is `{name}`, whose initializer is not a \
                     comptime integer literal (02-language.md §8.1, 03-hardware.md §3.1)"
                ),
                attr_span,
            ));
        }
    };
    if n < 1 {
        return Err(SemaError::at(
            "sema",
            format!(
                "`@budget(bound=N)`'s `N` is `{name}`, whose value is {n}; a loop bound is \
                 a comptime-known integer ≥ 1 (02-language.md §8.1, 03-hardware.md §3.1)"
            ),
            attr_span,
        ));
    }
    budget_bound_from_i128(n, attr_span)
}

fn budget_bound_from_i128(n: i128, span: Span) -> Result<u64, SemaError> {
    if n < 1 {
        return Err(SemaError::at(
            "sema",
            format!("`@budget(bound=N)` requires N ≥ 1; found {n}"),
            span,
        ));
    }
    u64::try_from(n).map_err(|_| {
        SemaError::at(
            "sema",
            format!("`@budget(bound=N)` value {n} does not fit a trip counter"),
            span,
        )
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

/// `name: T = value` (`a.ty.is_some()`) is a genuine declaration: it may
/// only ever reuse a binding already sitting in *this exact* block
/// (`lookup_innermost` — re-declaring the same name with the same type
/// twice in one suite, `bind_local`'s job below); a name one scope out
/// is a *different* binding as far as this statement is concerned (never
/// merged with it — `symbols::resolve` has already rejected the case
/// where that would shadow an outer local, 02-language.md §3.2, so
/// reaching here with the name absent from the innermost scope always
/// means "declare fresh, scoped to this block" is correct and safe).
///
/// `name = value` (`a.ty.is_none()`) is the arm-merge idiom's plain
/// reassignment (02-language.md §8.1): it must find a binding introduced
/// in *any* enclosing scope (`lookup_local`), including one made by an
/// already-finished sibling branch's own untyped first assignment,
/// because that is exactly how a name conditionally initialized in every
/// arm survives to be read after the construct. Only when no scope at
/// all already has the name does a plain assignment fall through to
/// introducing a fresh binding (necessarily scoped to the innermost
/// block, same as a typed declaration — a name first bound inside one
/// branch, with no annotation, still cannot escape that branch, exactly
/// like `lower.rs`'s own per-block `LEnv`).
fn check_assign(
    a: &AssignStmt,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    if matches!(a.value, Expr::Closure(_)) {
        return Err(type_error("closures cannot be stored".to_string(), a.span));
    }
    if let Expr::Name(_, name) = &a.target {
        let already_bound = if a.ty.is_some() {
            fctx.lookup_innermost(name)
        } else {
            fctx.lookup_local(name)
        };
        if already_bound.is_some() {
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
        // `CallError[E]` (plans/M6.md item A): carried as `Type::Named`
        // (not a dedicated `Type` variant like `Option`/`Result` — see
        // `compose_call_error`'s own doc comment), so this falls straight
        // through the `Type::Named` arm below already — no special case
        // needed here.
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

/// The largest `String[..N]` capacity a build accepts — the same bound
/// `[elem; N]` already carries (`check_array_len`'s 65536-element build
/// limit), for the same reason and with the same number: a `String[..N]`
/// is one length word plus `N` byte slots, so `N` is an element count in
/// exactly the sense an array's is. At the limit the aggregate is
/// `8 * (1 + 65536)` = 512 KiB, which is already far past anything a
/// 1 GiB guest image should hold in one value.
pub(crate) const MAX_STRING_CAPACITY: i128 = 65_536;

/// Whether `n` is a layout-representable `String[..N]` capacity
/// (plans/M9.md item K1, corrected 2026-07-26 by a `fuzz lower` find).
/// Layout is one length word plus `N` byte slots (`mwir::size_of`:
/// `8 * (1 + N)`). An i128 sum that fits only in i128 (or a usize that
/// overflows the slot product) used to typecheck and then panic the
/// compiler at lowering.
///
/// **Arithmetic representability is not enough, which is what K1 got
/// wrong.** `8 * (1 + N)` not overflowing `usize` admits `N = 2^60`: the
/// byte count is computable, and then `lower::emit_string_aggregate` calls
/// `Vec::with_capacity(1 + N)` and Rust aborts with `capacity overflow` —
/// a non-`internal error` panic, i.e. exactly the fail-open this predicate
/// exists to close. `cargo xtask fuzz lower --seed 101` found it at
/// iteration 6134 by truncating `golden/err-fstring-bound-overflow` so the
/// declared `String[..2^60]` survived without the concat sum that K1 did
/// guard. So the bound is a **build limit**, not an overflow check.
pub(crate) fn string_capacity_fits(n: i128) -> bool {
    (0..=MAX_STRING_CAPACITY).contains(&n)
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
        // `CallError[E]` (plans/M6.md item A, 02-language.md §9.4): a
        // fixed, compiler-known five-variant sum (the
        // `builtin_enum_variants` precedent, extended to carry payload
        // types — `Target`/`Failure`'s own fieldless variants need none).
        // `Admission`/`Peer` are opaque builtin payload types (M7 grows
        // their fields; pattern-matching the *variant* is all M6 needs).
        Type::Named(name, targs) if name == "CallError" => {
            if let Some(n) = enum_name {
                if n != "CallError" {
                    return Err(type_error(
                        format!("expected a `CallError` pattern, found `{n}`"),
                        span,
                    ));
                }
            }
            let Some(TypeArg::Type(e_ty)) = targs.first() else {
                return Err(type_error(
                    "`CallError` is missing its error argument".to_string(),
                    span,
                ));
            };
            match variant {
                "Op" => Ok(vec![e_ty.clone()]),
                "Cancelled" => Ok(vec![]),
                "DeadlineExceeded" => Ok(vec![]),
                "NotAdmitted" => Ok(vec![Type::Named("Admission".to_string(), vec![])]),
                "PeerFailed" => Ok(vec![Type::Named("Peer".to_string(), vec![])]),
                other => Err(type_error(
                    format!("`CallError` has no variant `{other}`"),
                    span,
                )),
            }
        }
        // plans/M8.md item G / plans/M9.md item I: an auto-visible stdlib
        // enum (`CompletionOutcome`, `BootError`, `DriverMode`, `Target`,
        // `Failure`) may have no `DeclEnum` in `mctx.enums` — its variants
        // live in `sema::stdlib_enums`. Every one of them is fieldless, so
        // the payload answer is always the empty vector; this arm exists so
        // `match o: case .Unknown:` type-checks the same way a module-
        // declared enum's does, with no per-enum special case.
        Type::Named(name, targs)
            if targs.is_empty() && crate::sema::stdlib_enums::is_auto_visible(name) =>
        {
            if let Some(n) = enum_name {
                if n != name {
                    return Err(type_error(
                        format!("expected a `{name}` pattern, found `{n}`"),
                        span,
                    ));
                }
            }
            // plans/M9.md item QQ: load failures are `error[build]`, not panic.
            let variants = crate::sema::stdlib_enums::variant_strs(name)?.ok_or_else(|| {
                type_error(format!("enum `{name}` has no variant `{variant}`"), span)
            })?;
            if variants.contains(&variant) {
                Ok(vec![])
            } else {
                Err(type_error(
                    format!("enum `{name}` has no variant `{variant}`"),
                    span,
                ))
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
                    Some(e) => std::borrow::Cow::Borrowed(&e.decl),
                    None => return Err(type_error(format!("`{name}` is not an enum"), span)),
                }
            } else {
                std::borrow::Cow::Owned(generics::instantiate_enum(mctx, name, targs, span)?)
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
            // plans/M13.md item K: private `Result[T]` accepts any
            // `Result[T, E]` and records `E` into the inferred set.
            if let (Type::Result(exp_ok, exp_err), Type::Result(act_ok, act_err)) =
                (exp, &actual.ty)
            {
                if types::is_inferred_error_set(exp_err) && types_eq(exp_ok, act_ok) {
                    fctx.record_inferred_error((**act_err).clone());
                    return Ok(actual);
                }
            }
            // plans/M7.md item H2a: an `Untrusted[T]` is never silently
            // coerced to a plain `T`. Prefer the mechanism's own wording
            // over a bare expected/found mismatch whenever the found
            // type is marked and the expected type is unmarked.
            if let Some(msg) = untrusted_coercion_message(exp, &actual.ty) {
                return Err(type_error(msg, expr.span()));
            }
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
        Expr::Str(span, text) => {
            // plans/M9.md item C1: a text literal coerced into
            // `String[..N]` when the expected type asks for one (02 §6.2 /
            // §1.1). Occupied byte length must fit the capacity.
            if let Some(Type::String(n_expr)) = expected {
                let n = literal_array_len(n_expr).ok_or_else(|| {
                    unimplemented_at("a `String[..N]` capacity that is not a literal is", *span)
                })?;
                let n = u64::try_from(n).map_err(|_| {
                    type_error("`String[..N]` capacity is out of range".to_string(), *span)
                })?;
                let bytes = crate::eval::value::decode_str(text);
                if (bytes.len() as u64) > n {
                    return Err(type_error(
                        format!(
                            "text literal of {} bytes exceeds `String[..{n}]` capacity",
                            bytes.len()
                        ),
                        *span,
                    ));
                }
                return Ok(TypedExpr {
                    ty: Type::String(n_expr.clone()),
                    kind: TypedExprKind::Str(text.clone()),
                });
            }
            Ok(TypedExpr {
                ty: Type::Static(Box::new(Type::Str)),
                kind: TypedExprKind::Str(text.clone()),
            })
        }
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
        Expr::FStr(f) => check_fstr(f, fctx, mctx),
        Expr::Bool(_span, v) => Ok(TypedExpr {
            ty: Type::Bool,
            kind: TypedExprKind::Bool(*v),
        }),
        Expr::Unit(_span) => Ok(TypedExpr {
            ty: Type::Unit,
            kind: TypedExprKind::Unit,
        }),
        Expr::Name(span, name) => synth_name(*span, name, expected, fctx, mctx),
        Expr::Field(base, span, name) => check_field_expr(base, *span, name, expected, fctx, mctx),
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
        Expr::Unary(span, UnaryOp::Await, inner) => check_await(inner, *span, fctx, mctx),
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
        Expr::Send(span, inner) => check_send(inner, *span, fctx, mctx),
        Expr::Tuple(span, items) => synth_tuple(*span, items, expected, fctx, mctx),
        Expr::List(span, items) => synth_list(*span, items, expected, fctx, mctx),
        Expr::ArrayRepeat(span, elem, count) => {
            synth_array_repeat(*span, elem, count, expected, fctx, mctx)
        }
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
    if let Some(info) = mctx.statics.get(name) {
        return Ok(TypedExpr {
            ty: info.ty.clone(),
            kind: TypedExprKind::Static(name.to_string()),
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
    expected: Option<&Type>,
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
                // plans/M9.md item B2: associated fns share the `Type.name`
                // spelling with fieldless variants. Look them up first so
                // a method never surfaces as "no variant".
                if let Some((_, d)) = e.assoc_fn(name) {
                    // Associated fns on a generic enum (and method-owned
                    // type params) stay item H / J2b's deferred boundary.
                    if !e.generics.is_empty() || !d.generics.is_empty() {
                        return Err(unimplemented_at("generic instantiation is", span));
                    }
                    let key = CalleeKey::Method(bname.clone(), name.to_string());
                    return Ok(TypedExpr {
                        ty: fn_value_type(d),
                        kind: TypedExprKind::FnRef(key),
                    });
                }
                if e.has_member_named(name) {
                    return Err(type_error(
                        format!("cannot reference method `{name}` without calling it"),
                        span,
                    ));
                }
                if e.variants.iter().any(|v| v.name == name) {
                    // plans/M9.md item J2c: generic-enum variant construction
                    // via expected type (`return Lookup.Absent` under
                    // `Lookup[u32]`). `instantiate_enum` already existed;
                    // this path simply refused before.
                    let (targs, decl) =
                        resolve_enum_for_variant_construction(bname, e, expected, span, mctx)?;
                    let dv = decl
                        .variants
                        .iter()
                        .find(|v| v.name == name)
                        .expect("name membership checked above");
                    if matches!(dv.payload, DeclVariantPayload::None) {
                        // plans/M9.md item DD / decision 9: the local
                        // lookup key (`bname`), not `e.name` (the
                        // exporter's spelling). Same rule as
                        // `check_struct_construction` — one spelling.
                        return Ok(TypedExpr {
                            ty: Type::Named(bname.clone(), targs),
                            kind: TypedExprKind::EnumConstruct {
                                enum_name: bname.clone(),
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
                    format!("enum `{bname}` has no variant or associated function `{name}`"),
                    span,
                ));
            }
            // plans/M4.md item B (05-library.md §9's own `Target`/
            // `Failure` prelude enums, decision 5): recognized only once
            // `bname` is not a real module struct/enum, so a module that
            // declares its own `Target`/`Failure` shadows this fallback
            // exactly like it would any other prelude name.
            // plans/M9.md item QQ: load failures are `error[build]`, not panic.
            if let Some(variants) = crate::sema::stdlib_enums::variant_strs(bname.as_str())? {
                if variants.contains(&name) {
                    return Ok(TypedExpr {
                        ty: Type::Named(bname.clone(), vec![]),
                        kind: TypedExprKind::EnumConstruct {
                            enum_name: bname.clone(),
                            variant: name.to_string(),
                            args: vec![],
                        },
                    });
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
    // plans/M7.md item C (03-hardware.md §2): a register selected out of
    // an `Mmio[L]` is not a value. It has no representation, cannot be
    // bound to a local, passed, stored or returned — the only two things
    // that exist are `.read()` and `.write(v)`, handled one level up in
    // `check_call_by_field`. Reaching *here* with an `Mmio[L]` base means
    // the source wrote a bare selection, so this is where that is named.
    if let Type::Named(cap, targs) = &base_ty {
        if cap == "Mmio" {
            return Err(mmio_bare_selection_error(targs, name, span, mctx));
        }
    }
    // plans/M7.md item E4: `IoCompletion[P]` fields — 03 §3/§8.
    // 0 payload, 1 status (`Result[unit, IoError]`), 2 written_len
    // (`Untrusted[usize]`).
    if let Type::Named(n, targs) = &base_ty {
        if n == "IoCompletion" {
            let Some(types::TypeArg::Type(payload)) = targs.first() else {
                return Err(type_error(
                    "`IoCompletion` with no payload type argument".to_string(),
                    span,
                ));
            };
            let field_ty = match name {
                "payload" => payload.clone(),
                "status" => Type::Result(
                    Box::new(Type::Unit),
                    Box::new(Type::Named("IoError".to_string(), vec![])),
                ),
                "written_len" => untrusted_type(Type::Usize),
                other => {
                    return Err(type_error(
                        format!(
                            "`IoCompletion[P]` has fields `payload`, `status`, and `written_len`; \
                             found `{other}`"
                        ),
                        span,
                    ));
                }
            };
            let index: usize = match name {
                "payload" => 0,
                "status" => 1,
                "written_len" => 2,
                _ => unreachable!(),
            };
            // Reuse Field spelling; lower maps the name to Project index
            // via the same order size_of uses.
            let _ = index;
            return Ok(TypedExpr {
                ty: field_ty,
                kind: TypedExprKind::Field(Box::new(base_t), name.to_string()),
            });
        }
    }
    // plans/M9.md item C1: `String[..N].len` is the occupied byte length
    // (slot 0 of the length-plus-N-bytes layout). Not a DeclStruct field;
    // lower maps the name to Project index 0.
    if matches!(&base_ty, Type::String(_)) {
        if name == "len" {
            return Ok(TypedExpr {
                ty: Type::Usize,
                kind: TypedExprKind::Field(Box::new(base_t), name.to_string()),
            });
        }
        return Err(type_error(
            format!(
                "`{}` has field `len` only; found `{name}`",
                types::render_type(&base_ty)
            ),
            span,
        ));
    }
    // plans/M10.md item B4 / decision 595: unbounded `Bytes` handle's
    // capacity word. The base address is not a field — source cannot
    // observe it.
    if matches!(&base_ty, Type::Bytes(None)) {
        if name == "len" {
            return Ok(TypedExpr {
                ty: Type::Usize,
                kind: TypedExprKind::Field(Box::new(base_t), name.to_string()),
            });
        }
        return Err(type_error(
            format!("type `Bytes` has no field `{name}`"),
            span,
        ));
    }
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
            // Bare integer literals must take the index's `usize` width
            // (02-language.md §6.1); a named `Untrusted[usize]` must be
            // rejected by the marked-value rule before any coercion.
            let idx_t = if is_bare_numeric_literal(&args[0]) {
                check_expr(&args[0], Some(&Type::Usize), fctx, mctx)?
            } else {
                let idx_t = check_expr(&args[0], None, fctx, mctx)?;
                if is_untrusted_type(&idx_t.ty) {
                    return Err(untrusted_use_error("an array index", args[0].span()));
                }
                if !types_eq(&idx_t.ty, &Type::Usize) {
                    return Err(type_error(
                        format!(
                            "expected `usize`, found `{}`",
                            types::render_type(&idx_t.ty)
                        ),
                        args[0].span(),
                    ));
                }
                idx_t
            };
            Ok(TypedExpr {
                ty: (**elem).clone(),
                kind: TypedExprKind::Index(Box::new(base_t), Box::new(idx_t)),
            })
        }
        Type::Bytes(_) => {
            let idx_t = if is_bare_numeric_literal(&args[0]) {
                check_expr(&args[0], Some(&Type::Usize), fctx, mctx)?
            } else {
                let idx_t = check_expr(&args[0], None, fctx, mctx)?;
                if is_untrusted_type(&idx_t.ty) {
                    return Err(untrusted_use_error("an array index", args[0].span()));
                }
                if !types_eq(&idx_t.ty, &Type::Usize) {
                    return Err(type_error(
                        format!(
                            "expected `usize`, found `{}`",
                            types::render_type(&idx_t.ty)
                        ),
                        args[0].span(),
                    ));
                }
                idx_t
            };
            Ok(TypedExpr {
                ty: Type::U8,
                kind: TypedExprKind::Index(Box::new(base_t), Box::new(idx_t)),
            })
        }
        // plans/M9.md item C1: `String[..N][i]` → `u8` (bounds against
        // occupied length at eval/lower time).
        Type::String(_) => {
            let idx_t = if is_bare_numeric_literal(&args[0]) {
                check_expr(&args[0], Some(&Type::Usize), fctx, mctx)?
            } else {
                let idx_t = check_expr(&args[0], None, fctx, mctx)?;
                if is_untrusted_type(&idx_t.ty) {
                    return Err(untrusted_use_error("an array index", args[0].span()));
                }
                if !types_eq(&idx_t.ty, &Type::Usize) {
                    return Err(type_error(
                        format!(
                            "expected `usize`, found `{}`",
                            types::render_type(&idx_t.ty)
                        ),
                        args[0].span(),
                    ));
                }
                idx_t
            };
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

/// `[elem; N]` (plans/M9.md item F1 decision 343): desugar to a fixed
/// list of `N` copies. `N` must be a literal usize after const-generic
/// substitution.
fn synth_array_repeat(
    span: Span,
    elem: &Expr,
    count: &Expr,
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let n = literal_array_len(count).ok_or_else(|| {
        type_error(
            "`[elem; N]` needs a literal usize count (after const-generic substitution)"
                .to_string(),
            count.span(),
        )
    })?;
    if n < 0 {
        return Err(type_error(
            "`[elem; N]` count must be nonnegative".to_string(),
            count.span(),
        ));
    }
    if n > 65_536 {
        return Err(type_error(
            format!("`[elem; N]` count {n} exceeds the 65536-element build limit"),
            count.span(),
        ));
    }
    let n_usize = n as usize;
    let elem_expected = match expected {
        Some(Type::Array(elem_ty, len_expr)) => {
            if let Some(en) = literal_array_len(len_expr) {
                if en != n {
                    return Err(type_error(
                        format!("array expects {en} element(s), found {n}"),
                        span,
                    ));
                }
            }
            Some(elem_ty.as_ref())
        }
        _ => None,
    };
    let first = check_expr(elem, elem_expected, fctx, mctx)?;
    let elem_ty = first.ty.clone();
    let mut typed_items = Vec::with_capacity(n_usize);
    typed_items.push(first);
    for _ in 1..n_usize {
        typed_items.push(check_expr(elem, Some(&elem_ty), fctx, mctx)?);
    }
    let len = count.clone();
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
        (Type::String(l1), Type::String(l2)) => same_len_expr(l1, l2),
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
        // plans/M9.md item F1 decision 341: `..N` and `N` are the same
        // const argument at a `const` generic parameter.
        (types::TypeArg::Const(x), types::TypeArg::Bound(y))
        | (types::TypeArg::Bound(x), types::TypeArg::Const(y)) => same_len_expr(x, y),
        // plans/M7.md item D introduced `TypeArg::Pool`; equality was
        // incomplete (Pool vs Pool fell to `false`), so `Option[DmaShared
        // [P, L]] = None` rendered identical expected/found and still
        // rejected. Protocol-consumption needs that assignment to work.
        (types::TypeArg::Pool(x), types::TypeArg::Pool(y)) => x == y,
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
        // plans/M8.md item G: `QueueOp[P, <idempotent>]`'s own const
        // argument is a bool literal — the first non-integer one this
        // comparator has seen. Without this arm two identically-declared
        // operations compare unequal and render identically, which is the
        // exact shape of the `TypeArg::Pool` bug noted just above.
        (Expr::Bool(_, b1), Expr::Bool(_, b2)) => b1 == b2,
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
    // plans/M9.md item C2: `String[..N] + String[..M] -> String[..N+M]`.
    // Text literals coerce to `String[..len]` in this position.
    if op == BinOp::Add {
        if let Some(out) = check_string_add(l, r, span, fctx, mctx)? {
            return Ok(out);
        }
    }
    let (lt, rt) = check_same_type_operands(l, r, fctx, mctx)?;
    build_binop_expr(op, lt, rt, span, mctx)
}

/// `String[..N] + String[..M]` (and text-literal coercion into that form).
/// Returns `None` when this is not a string add, so the ordinary numeric
/// path can run.
fn check_string_add(
    l: &Expr,
    r: &Expr,
    span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Option<TypedExpr>, SemaError> {
    let lt = check_expr(l, None, fctx, mctx)?;
    let lt = coerce_text_literal_to_string(lt, l.span())?;
    let Type::String(ln) = &lt.ty else {
        return Ok(None);
    };
    let ln = literal_array_len(ln).ok_or_else(|| {
        unimplemented_at("a `String[..N]` capacity that is not a literal is", span)
    })?;
    let rt = check_expr(r, None, fctx, mctx)?;
    let rt = coerce_text_literal_to_string(rt, r.span())?;
    let Type::String(rn) = &rt.ty else {
        return Err(type_error(
            format!(
                "expected `String[..N]`, found `{}`",
                types::render_type(&rt.ty)
            ),
            r.span(),
        ));
    };
    let rn = literal_array_len(rn).ok_or_else(|| {
        unimplemented_at("a `String[..N]` capacity that is not a literal is", span)
    })?;
    let sum = ln
        .checked_add(rn)
        .ok_or_else(|| type_error("String concatenation capacity overflows".to_string(), span))?;
    if !string_capacity_fits(sum) {
        return Err(type_error(
            "String concatenation capacity overflows".to_string(),
            span,
        ));
    }
    Ok(Some(TypedExpr {
        ty: Type::String(Box::new(Expr::Int(span, sum.to_string()))),
        kind: TypedExprKind::Binary(BinOp::Add, Box::new(lt), Box::new(rt)),
    }))
}

/// `Static[Str]` text literal → `String[..byte_len]` for Format concat.
fn coerce_text_literal_to_string(te: TypedExpr, span: Span) -> Result<TypedExpr, SemaError> {
    match &te.ty {
        Type::String(_) => Ok(te),
        Type::Static(inner) if matches!(inner.as_ref(), Type::Str) => {
            let TypedExprKind::Str(text) = &te.kind else {
                return Err(type_error(
                    "only a text literal coerces from `Static[Str]` to `String[..N]` here"
                        .to_string(),
                    span,
                ));
            };
            let n = crate::eval::value::decode_str(text).len();
            Ok(TypedExpr {
                ty: Type::String(Box::new(Expr::Int(span, n.to_string()))),
                kind: te.kind,
            })
        }
        _ => Ok(te),
    }
}

/// plans/M9.md item D: `f"..."` → Format + `String` concat, type
/// `String[..N]` with `N` the sum of literal bytes and each operand's
/// `max_formatted_len`.
fn check_fstr(
    f: &crate::syntax::ast::FStringLit,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let desugared = crate::sema::fstring::desugar_fstring(f)?;
    let te = match check_expr(&desugared, None, fctx, mctx) {
        Ok(te) => te,
        Err(e) => return Err(rewrite_fstring_format_error(e)),
    };
    match &te.ty {
        Type::String(_) => Ok(te),
        Type::Static(inner) if matches!(inner.as_ref(), Type::Str) => {
            coerce_text_literal_to_string(te, f.span)
        }
        other => Err(type_error(
            format!(
                "f-string must produce `String[..N]`, found `{}`",
                types::render_type(other)
            ),
            f.span,
        )),
    }
}

/// Map a bare "no method `format`" onto the f-string wording (unbounded
/// operand / no Format). `Secret` by type name keeps 05 §6's sentence.
fn rewrite_fstring_format_error(e: SemaError) -> SemaError {
    if let Some((ty, method)) = &e.missing_method {
        if method == "format" {
            if ty == "Secret" {
                return types::secret_has_no_format(Span {
                    line: e.line,
                    col: e.col,
                });
            }
            return SemaError::at(
                "type",
                format!(
                    "f-string operand of type `{ty}` has no Format \
                     (unbounded / no max_formatted_len; 05-library.md §6)"
                ),
                Span {
                    line: e.line,
                    col: e.col,
                },
            );
        }
    }
    e
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
    // plans/M7.md item H2a: if either operand is `Untrusted[T]`, do not
    // unify the other against it (a bare literal would otherwise be
    // asked to type as `Untrusted[T]` and report a confusing mismatch).
    // The caller (range / binary) then rejects the marked use by name.
    if is_bare_numeric_literal(a) && !is_bare_numeric_literal(b) {
        let bt = check_expr(b, None, fctx, mctx)?;
        if is_untrusted_type(&bt.ty) {
            let at = check_expr(a, None, fctx, mctx)?;
            return Ok((at, bt));
        }
        let at = check_expr(a, Some(&bt.ty), fctx, mctx)?;
        Ok((at, bt))
    } else {
        let at = check_expr(a, None, fctx, mctx)?;
        if is_untrusted_type(&at.ty) {
            let bt = check_expr(b, None, fctx, mctx)?;
            return Ok((at, bt));
        }
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
    // plans/M7.md item H2a / plans/M9.md item G1 decision 351: an
    // `Untrusted[T]` has no arithmetic or comparison form — ordinary use
    // with an unmarked value (or another marked one) is a rejection
    // naming the narrowing transition, never a coercion that preserves
    // taint. Archive 05 §8's "arithmetic preserves untrusted" reading is
    // deliberately not taken: 03 §8 says the value cannot be used until
    // checked-narrowed. Comparisons get their own use-kind so the
    // diagnostic does not call `<`/`==` "arithmetic".
    if is_untrusted_type(&l.ty) || is_untrusted_type(&r.ty) {
        let use_kind = match op {
            Eq | Ne | Lt | Le | Gt | Ge => "a comparison",
            _ => "an arithmetic operand",
        };
        return Err(untrusted_use_error(use_kind, span));
    }
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
        if crate::eval::image_checks::is_sealed_authority_type_name(name) {
            return true;
        }
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
            Type::Result(_, ret_err) if types::is_inferred_error_set(&ret_err) => {
                // plans/M13.md item K: `?` widens the inferred set; no `from`.
                if types::is_inferred_error_set(&t_err) {
                    return Err(type_error(
                        "`?` on a function whose error set is not yet inferred — declare \
                         that function above this one (02-language.md §5)"
                            .to_string(),
                        span,
                    ));
                }
                fctx.record_inferred_error(*t_err);
                Ok(TypedExpr {
                    ty: *t_ok,
                    kind: TypedExprKind::Try(Box::new(inner_t), None),
                })
            }
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
/// source: E) -> Self`, or the `from` `deriving(From)` generates
/// (05-library.md §8 / plans/M9.md item B3). Returns the conversion's
/// return type plus the `<Target>.from`-shaped callee key
/// (plans/M3.md item A). Both shapes are real TypedFns; there is no
/// second structural path.
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
        if let Some((_, d)) = e.assoc_fn("from") {
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
    // plans/M9.md item F1 decision 344: closures are synchronous and
    // non-escaping (02 §8.3) — they cannot suspend.
    if let Some(span) = scan_closure_await(&c.body) {
        return Err(type_error(
            "a closure cannot contain `await`".to_string(),
            span,
        ));
    }
    fctx.push_scope();
    let saved_async = fctx.in_async;
    fctx.in_async = false;
    let result = check_closure_body(c, &exp_params, &exp_ret, fctx, mctx);
    fctx.in_async = saved_async;
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
            check_call_by_field(base, *fspan, name, span, args, expected, fctx, mctx)
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
    // 03-hardware.md §1 (plans/M7.md item A): `DeviceCap[VirtioBlock](...)`
    // is the construction spelling a source author reaches for first, and
    // it must be rejected as forgery rather than as the "generic
    // instantiation is not checked yet" this function's own fall-through
    // would report — a diagnostic naming the wrong cause. Checked before
    // anything else here, since no other arm can produce a capability.
    if let Expr::Name(_, name) = inner {
        capability_forgery_check(name, "constructed", call_span)?;
    }
    if let Expr::Field(base, fspan, mname) = inner {
        if mname == "device" || mname == "pool" || mname == "dma_pool" {
            // `img.device[D](...)`/`img.pool[T](...)`/`img.dma_pool[T](...)`
            // (plans/M4.md item B, decision 5, 05-library.md §9): the
            // builder surface's own bracketed intrinsics. Recognized only
            // when the receiver's own (already-checked) type is the
            // builder's opaque `Image` type — anything else falls
            // through to the ordinary "generic instantiation" scope
            // boundary below, unchanged.
            let base_t = check_expr(base, None, fctx, mctx)?;
            if base_t.ty == image_type() {
                return check_image_bracket_intrinsic(mname, targs, args, *fspan, fctx, mctx);
            }
        }
        if mname == "to" || mname == "checked_to" || mname == "truncate_to" {
            if targs.len() != 1 {
                return Err(type_error(
                    "a conversion needs exactly one type argument".to_string(),
                    ispan,
                ));
            }
            // 03-hardware.md §1's "no address, import, or **cast** creates
            // one" (plans/M7.md item A). `.to[T]` is this language's only
            // conversion form (02-language.md §6.1: "no cast operator" —
            // conversion is a method), so `0x1000.to[Mmio[VirtioIrqMmio]]()`
            // is *the* address-to-capability cast the sentence names. It
            // already fails ("`.to` target must be a scalar type"), which
            // is true but describes the wrong rule; this says which rule.
            if let Some(name) = capability_name_in_type_expr(&targs[0]) {
                capability_forgery_check(name, "cast to", ispan)?;
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
        // `Type.assoc[Args](...)` first — `check_expr` on a type name is
        // `error[type]: is a type, not a value`, so recognize it before
        // synthesizing the base (plans/M13.md item Q).
        if let Expr::Name(_, bname) = base.as_ref() {
            if fctx.lookup_local(bname).is_none() {
                if let Some(s) = mctx.structs.get(bname.as_str()) {
                    if s.decl.generics.is_empty() {
                        if let Some((_, d)) = s.assoc_fn(mname) {
                            if !d.generics.is_empty() {
                                let recv_ty = Type::Named(bname.clone(), vec![]);
                                return check_method_generic_call(
                                    &recv_ty,
                                    mname,
                                    d,
                                    args,
                                    Some(targs),
                                    call_span,
                                    None,
                                    fctx,
                                    mctx,
                                );
                            }
                        }
                    }
                }
                if let Some(e) = mctx.enums.get(bname.as_str()) {
                    if e.generics.is_empty() {
                        if let Some((_, d)) = e.assoc_fn(mname) {
                            if !d.generics.is_empty() {
                                let recv_ty = Type::Named(bname.clone(), vec![]);
                                return check_method_generic_call(
                                    &recv_ty,
                                    mname,
                                    d,
                                    args,
                                    Some(targs),
                                    call_span,
                                    None,
                                    fctx,
                                    mctx,
                                );
                            }
                        }
                    }
                }
            }
        }
        // `x.method[Args](...)`: method-owned generics with explicit args.
        let base_t = check_expr(base, None, fctx, mctx)?;
        let base_ty = unwrap_own(base_t.ty.clone());
        if let Type::Named(sname, recv_targs) = &base_ty {
            if let Some(s) = if recv_targs.is_empty() {
                mctx.structs
                    .get(sname.as_str())
                    .map(std::borrow::Cow::Borrowed)
            } else if mctx.structs.contains_key(sname.as_str()) {
                Some(std::borrow::Cow::Owned(generics::instantiate_struct(
                    mctx, sname, recv_targs, call_span,
                )?))
            } else {
                None
            } {
                if let Some((_, d)) = s.method(mname) {
                    if !d.generics.is_empty() {
                        let recv_ty = Type::Named(sname.clone(), recv_targs.clone());
                        return check_method_generic_call(
                            &recv_ty,
                            mname,
                            d,
                            args,
                            Some(targs),
                            call_span,
                            Some(base_t),
                            fctx,
                            mctx,
                        );
                    }
                }
            }
            if recv_targs.is_empty() {
                if let Some(e) = mctx.enums.get(sname.as_str()) {
                    if let Some((_, d)) = e.method(mname) {
                        if !d.generics.is_empty() {
                            let recv_ty = Type::Named(sname.clone(), vec![]);
                            return check_method_generic_call(
                                &recv_ty,
                                mname,
                                d,
                                args,
                                Some(targs),
                                call_span,
                                Some(base_t),
                                fctx,
                                mctx,
                            );
                        }
                    }
                }
            }
        }
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
                    return check_struct_construction(
                        name, &si, &type_args, args, call_span, fctx, mctx,
                    );
                }
            }
        }
    }
    Err(unimplemented_at("generic instantiation is", call_span))
}

// --- plans/M4.md item B: the `@image` builder surface (05-library.md §9) --
//
// Decision 5: "compiler-recognized ... prelude-style declarations, no
// stdlib source needed ... recognized by callee key exactly like the
// existing prelude/intrinsic machinery" — the whole surface is a
// handful of fixed match arms right alongside `Some`/`Ok`/`Err`/`panic`
// above, producing one dedicated typed node (`TypedExprKind::Intrinsic`,
// `typed.rs`'s own module doc) instead of an ordinary `Call`: none of
// these have a declared parameter list a positional/labeled-default
// alignment could check against, so every argument keeps its own source
// label. Legality (illegal anywhere but the one reachable `@image` fn)
// is `eval::legal`'s job, not this pass's — every intrinsic type-checks
// uniformly wherever it is written, exactly like `Some`/`Ok`/`Err` do.

/// The builder's own opaque `Image` type (the same type an `@image` fn
/// declares as its return type, `types::resolve_named`'s own new arm).
fn image_type() -> Type {
    Type::Named("Image".to_string(), vec![])
}

/// The builder's own opaque declaration-handle type: every
/// `img.device`/`img.driver`/`img.actor`/`img.pool`/`img.dma_pool` call,
/// and `decl.handle()`, produces one (decision 5: "opaque builtin
/// resource types ... declaration handles" — one shared type for every
/// declaration kind, since nothing before item C's graph checks needs to
/// tell them apart structurally). Never resolvable as a source type
/// annotation (no `resolve_named` arm) — it only ever appears as an
/// inferred local's type.
fn image_decl_type() -> Type {
    Type::Named("ImageDecl".to_string(), vec![])
}

/// One evaluated (or, for `img.pool`/`img.dma_pool`'s own `name=`
/// argument, pool-name-referenced) builder argument, still carrying its
/// source label.
type IntrinsicArgs = Vec<(String, TypedExpr)>;

/// Checks/types one builder intrinsic's whole argument list (plans/M4.md
/// item B): every argument must carry a label (`img.check_layout(f)`'s
/// single positional argument is handled by its own dedicated call site,
/// not this shared helper) — a label bound more than once, or an
/// unlabeled argument, is a `type` diagnostic. Each argument is checked
/// with no expected type: the builder's own arguments have no declared
/// parameter list to check against (item C's own job is validating them
/// against the real target, e.g. an actor's `init`), so item B only
/// needs every argument to type-check as *some* ordinary comptime-legal
/// expression. One narrow exception, applied uniformly rather than only
/// for `img.pool`/`img.dma_pool` (harmless everywhere else: `Image`'s own
/// `name=` argument is always a string literal, never a bare identifier,
/// so the pool-name interpretation below never actually matches there):
/// an argument labeled `name` whose value is a bare identifier naming an
/// already-declared module-scope `pool` (02-language.md §4) is the one
/// builder argument that is not an ordinary value expression (a pool
/// name is otherwise only ever spelled inside an `own[P] T` annotation,
/// never referenced as a value) — recorded as a `PoolName` leaf instead
/// of falling through to `synth_name`'s ordinary lookup, which would
/// (correctly, for anything else) reject it.
fn check_intrinsic_args(
    args: &[Arg],
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<IntrinsicArgs, SemaError> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        let Some(label) = &a.label else {
            return Err(type_error(
                "image builder arguments must be labeled".to_string(),
                a.span,
            ));
        };
        if !seen.insert(label.clone()) {
            return Err(type_error(
                format!("argument `{label}` bound more than once"),
                a.span,
            ));
        }
        let typed = if label == "name" {
            match &a.value {
                Expr::Name(_, pool_name)
                    if mctx.module_pools.contains(pool_name)
                        || fctx.local_pools.contains(pool_name) =>
                {
                    TypedExpr {
                        ty: Type::Named("PoolName".to_string(), vec![]),
                        kind: TypedExprKind::PoolName(pool_name.clone()),
                    }
                }
                _ => check_expr(&a.value, None, fctx, mctx)?,
            }
        } else {
            check_expr(&a.value, None, fctx, mctx)?
        };
        out.push((label.clone(), typed));
    }
    Ok(out)
}

/// Resolves a builder intrinsic's own leading bare type-name argument —
/// `img.driver(A, ...)`/`img.actor(A, ...)`'s unlabeled first positional
/// argument, or `img.device[D]`/`img.pool[T]`/`img.dma_pool[T]`'s
/// bracketed one — through the ordinary type resolver
/// (`ModuleCtx::resolve_type`), so every shape that resolver already
/// accepts (a scalar, a plain user struct/enum, `Bytes`, ...) is
/// accepted here too. Only a *bare name* is supported (item B's own
/// documented scope boundary): a further `[...]` on the name itself
/// (`BlkDriver[DriverMode.Irq]`, 02-language.md §12.1's own worked
/// example) is a generic struct instantiation used as a comptime type
/// argument rather than a value, which is a different, larger feature
/// item B does not add — it fails closed with the same
/// `unimplemented_at("generic instantiation is", ...)` every other
/// generic-instantiation scope boundary in this file already uses.
fn resolve_intrinsic_type_arg(e: &Expr, fctx: &FnCtx, mctx: &ModuleCtx) -> Result<Type, SemaError> {
    match e {
        Expr::Name(span, name) => {
            let ast_ty = ast::Type::Named(NamedType {
                span: *span,
                name: name.clone(),
                args: vec![],
            });
            mctx.resolve_type(&ast_ty, &fctx.local_pools)
        }
        _ => Err(unimplemented_at("generic instantiation is", e.span())),
    }
}

/// `img.driver(A, ...)`/`img.actor(A, ...)`'s own leading type argument —
/// deliberately *not* `resolve_intrinsic_type_arg` above: only a struct
/// (never a scalar/`Bytes`/enum) can be `@actor`/`@driver`-attributed, so
/// this looks `name` up directly in `mctx.structs` instead of through the
/// ordinary type-annotation resolver. This is the one difference that
/// matters for a multi-module build (plans/M4.md item A's own disclosed
/// scope line): `mctx.structs` *is* spliced from an imported module's
/// own checked output (`sema::check_program`'s own splice step), while
/// `mctx.shapes` (the type-annotation arity table `resolve_type` reads)
/// is module-local only — so an imported actor/driver struct resolves
/// here, in call/callee position, exactly like constructing it would,
/// even though it could not yet resolve as an explicit type annotation.
///
/// plans/M7.md item G, decision 18: also accepts `BlkDriver[DriverMode.Irq]`
/// (an `Expr::Index` whose base is the struct name) and enqueues the
/// instantiation so the mode-specialized members exist.
fn resolve_intrinsic_struct_type_arg(e: &Expr, mctx: &ModuleCtx) -> Result<Type, SemaError> {
    match e {
        Expr::Name(span, name) => {
            let Some(s) = mctx.structs.get(name) else {
                return Err(type_error(format!("unknown type `{name}`"), *span));
            };
            if !s.decl.generics.is_empty() {
                return Err(unimplemented_at("generic instantiation is", *span));
            }
            Ok(Type::Named(name.clone(), vec![]))
        }
        Expr::Index(base, span, args) => {
            let Expr::Name(nspan, name) = base.as_ref() else {
                return Err(unimplemented_at("generic instantiation is", *span));
            };
            let Some(s) = mctx.structs.get(name) else {
                return Err(type_error(format!("unknown type `{name}`"), *nspan));
            };
            if s.decl.generics.is_empty() {
                return Err(type_error(format!("`{name}` is not generic"), *span));
            }
            let targs = generics::resolve_call_targs(args, mctx)?;
            // Force the instantiation (and its deferred comptime-if
            // expansion) to exist before image checks / layout run.
            // `instantiate_struct` arity-checks and expands MODE branches.
            let _ = generics::instantiate_struct(mctx, name, &targs, *span)?;
            Ok(Type::Named(name.clone(), targs))
        }
        _ => Err(unimplemented_at("generic instantiation is", e.span())),
    }
}

/// `img.device[D](...)`, `img.pool[T](...)`, `img.dma_pool[T](...)` —
/// the bracketed third of the builder surface (05-library.md §9);
/// `check_call_index`'s own new arm dispatches here once the receiver's
/// type is confirmed to be `Image`. All three share the identical shape
/// (one bracketed type argument, otherwise arbitrary labeled arguments),
/// so one function covers them; `mname` only decides the intrinsic's own
/// key spelling.
fn check_image_bracket_intrinsic(
    mname: &str,
    targs: &[Expr],
    args: &[Arg],
    ispan: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if targs.len() != 1 {
        return Err(type_error(
            format!("`img.{mname}` takes exactly one type argument"),
            ispan,
        ));
    }
    let type_arg = resolve_intrinsic_type_arg(&targs[0], fctx, mctx)?;
    let iargs = check_intrinsic_args(args, fctx, mctx)?;
    Ok(TypedExpr {
        ty: image_decl_type(),
        kind: TypedExprKind::Intrinsic {
            key: format!("Image.{mname}"),
            receiver: None,
            type_arg: Some(type_arg),
            args: iargs,
        },
    })
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
    // plans/M7.md item H2a: `Untrusted[T]` is sealed — no source-visible
    // constructor (03-hardware.md §8: the wrapper gates use until an
    // explicit typed transition; the transitions are the narrowings OUT,
    // and the producers are device control values, not a call form).
    if name == "Untrusted" {
        return Err(type_error(
            "`Untrusted[T]` is a sealed marked-value wrapper (03-hardware.md §8); it has no \
             source-visible constructor — a device control value arrives marked, and the only \
             transition out is a checked narrowing such as `.checked_le(bound)`"
                .to_string(),
            call_span,
        ));
    }
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
            ty: resolved_ret(&f.decl.ret, None, name, mctx),
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
        return check_struct_construction(name, s, &[], args, call_span, fctx, mctx);
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
                Some(Type::Result(t, e)) if types::is_inferred_error_set(e) => {
                    // plans/M13.md item K: Ok does not contribute an error;
                    // use `never` so return-gate recording skips it.
                    (Some((**t).clone()), Some(Type::Never))
                }
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
                Some(Type::Result(t, e)) if types::is_inferred_error_set(e) => {
                    // Open error side — any constructed error joins the set.
                    (Some((**t).clone()), None)
                }
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
            fctx.record_inferred_error(e_typed.ty.clone());
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
        // plans/M4.md item B, decision 5 (05-library.md §9): `Image`
        // is the one builder intrinsic called by bare name (every other
        // one is a method call on the value it returns, dispatched from
        // `check_call_by_field`/`check_call_index` below).
        "Image" => {
            let iargs = check_intrinsic_args(args, fctx, mctx)?;
            Ok(TypedExpr {
                ty: image_type(),
                kind: TypedExprKind::Intrinsic {
                    key: "Image".to_string(),
                    receiver: None,
                    type_arg: None,
                    args: iargs,
                },
            })
        }
        // plans/M9.md item E: `seconds` / `ms` deleted as intrinsic arms —
        // ordinary wrela in `stdlib/core/time.wr`, prelude-visible via
        // IMAGE_BUILDER / ACTOR_SURFACE. `now` stays sealed below.
        // Plans/M6.md item A, decision 11 (02-language.md §9.5's own
        // vocabulary): `now()` is runtime-only — the one new
        // `eval::legal` illegal-reason arm decision 11 asks for (mirrors
        // the intrinsic-outside-`@image` precedent: recognized by bare
        // callee spelling, restricted by a dedicated `eval::legal` check
        // rather than by failing here — `now`'s own *type* is legal
        // everywhere the language type-checks; only *evaluating* it at
        // build time is illegal).
        "now" => {
            if !args.is_empty() {
                return Err(type_error(
                    "`now` takes no arguments".to_string(),
                    call_span,
                ));
            }
            Ok(TypedExpr {
                ty: Type::Named("Instant".to_string(), vec![]),
                kind: TypedExprKind::Intrinsic {
                    key: "now".to_string(),
                    receiver: None,
                    type_arg: None,
                    args: vec![],
                },
            })
        }
        // plans/M7.md item G, decision 17: `InterruptCell(0)` — the one
        // source-visible constructor 03 §6's worked example spells
        // (`self.pending = InterruptCell(0)`). Not a capability: the
        // forgery arm below must not catch it.
        "InterruptCell" => check_interrupt_cell_new(args, expected, call_span, fctx, mctx),
        // plans/M7.md item G: `wake(Driver.method)` — 03 §6's statically
        // bound bottom-half wake. Prelude-style bare name (no import).
        "wake" => check_wake_call(args, call_span, fctx, mctx),
        _ => {
            // 03-hardware.md §1 (plans/M7.md item A): a bare
            // `DeviceCap(...)` reaches here (the name resolves — it is a
            // prelude name — but nothing declares it, so no fn/struct arm
            // above matched). "`DeviceCap` is not callable" is true and
            // says nothing; this names the rule instead.
            capability_forgery_check(name, "called", call_span)?;
            Err(type_error(format!("`{name}` is not callable"), call_span))
        }
    }
}

/// 03-hardware.md §1's unforgeability sentence, in one place (plans/M7.md
/// item A): "Their constructors are not source-visible: no address,
/// import, or cast creates one." `attempt` is the verb of whatever the
/// source just tried — `constructed`, `called`, `cast to` — so each
/// rejection names the construct the author actually wrote rather than
/// the generic shape it happens to share with something else. A no-op for
/// every non-capability name, so a call site can ask unconditionally.
fn capability_forgery_check(name: &str, attempt: &str, span: Span) -> Result<(), SemaError> {
    if !crate::eval::image_checks::is_sealed_authority_type_name(name) {
        return Ok(());
    }
    let kind = crate::eval::image_checks::sealed_authority_kind(name);
    let origin = if crate::eval::image_checks::is_protocol_state_type_name(name) {
        "a bring-up state is produced only by the sealed transport's own transitions"
    } else {
        "a capability is minted only where the image binds a declared device to a `@driver`"
    };
    Err(type_error(
        format!(
            "`{name}` is {kind} and cannot be {attempt}: its constructor is not source-visible, \
             and {origin}"
        ),
        span,
    ))
}

/// The capability name a *type-shaped* expression names at its head, if
/// any: `Mmio` in both `Mmio` and `Mmio[VirtioIrqMmio]`. The grammar
/// hands `.to[T]`'s own target back as an `Expr` (types and values share
/// one production there), so this reads the same two shapes
/// `scalar_type_by_name_expr` does, asking a different question.
fn capability_name_in_type_expr(e: &Expr) -> Option<&str> {
    let name = match e {
        Expr::Name(_, n) => n.as_str(),
        Expr::Index(base, _, _) => match base.as_ref() {
            Expr::Name(_, n) => n.as_str(),
            _ => return None,
        },
        _ => return None,
    };
    crate::eval::image_checks::is_sealed_authority_type_name(name).then_some(name)
}

/// Call a method/associated fn that declares its own type parameters
/// (plans/M13.md item Q): infer or resolve `[Args]`, substitute + enqueue,
/// check args against the concrete signature. `receiver_ty` is the
/// concrete `Type::Named` the call goes through; `call_receiver` is the
/// typed receiver expression (`None` for an associated-fn call spelled
/// `Type.method(...)`).
fn check_method_generic_call(
    receiver_ty: &Type,
    method: &str,
    d: &types::DeclFn,
    args: &[Arg],
    explicit_targs: Option<&[Expr]>,
    call_span: Span,
    call_receiver: Option<TypedExpr>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let type_args = match explicit_targs {
        Some(targs) => generics::resolve_call_targs(targs, mctx)?,
        None => generics::infer_method_targs(method, d, args, fctx, mctx, call_span)?,
    };
    let (ast, decl) =
        generics::instantiate_method(mctx, receiver_ty, method, &type_args, call_span)?;
    let typed_args = check_call_args(&ast.params, &decl.params, args, call_span, fctx, mctx)?;
    let key = CalleeKey::FnInstance(generics::canonical_method_key(
        receiver_ty,
        method,
        &type_args,
    ));
    Ok(TypedExpr {
        ty: decl.ret,
        kind: TypedExprKind::Call {
            callee: key,
            receiver: call_receiver.map(Box::new),
            args: typed_args,
        },
    })
}

fn check_call_by_field(
    base: &Expr,
    fspan: Span,
    name: &str,
    call_span: Span,
    args: &[Arg],
    expected: Option<&Type>,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if let Expr::Name(_, bname) = base {
        if fctx.lookup_local(bname).is_none() {
            // plans/M7.md item E1: `VirtQueue.configure(...)` — the sealed
            // queue constructor, spelled on the builtin type name. Checked
            // *before* the struct/enum arms so a prelude name that is not
            // a user declaration still reaches it.
            if bname == "VirtQueue" && name == "configure" {
                return check_virtqueue_configure(args, fspan, call_span, fctx, mctx);
            }
            if let Some(s) = mctx.structs.get(bname.as_str()) {
                if !s.decl.generics.is_empty() {
                    return Err(unimplemented_at("generic instantiation is", call_span));
                }
                if let Some((af, d)) = s.assoc_fn(name) {
                    if !d.generics.is_empty() {
                        let recv_ty = Type::Named(bname.clone(), vec![]);
                        return check_method_generic_call(
                            &recv_ty, name, d, args, None, call_span, None, fctx, mctx,
                        );
                    }
                    let typed_args =
                        check_call_args(&af.params, &d.params, args, call_span, fctx, mctx)?;
                    let key = CalleeKey::Method(bname.clone(), name.to_string());
                    return Ok(TypedExpr {
                        ty: resolved_ret(&d.ret, Some(bname), name, mctx),
                        kind: TypedExprKind::Call {
                            callee: key,
                            receiver: None,
                            args: typed_args,
                        },
                    });
                }
                // plans/M7.md item H1, 03-hardware.md §9's bring-up chain:
                // `VirtioBlock.claim(cap=take cap)` — the sealed
                // transport's own entry point, spelled on the *device
                // type* exactly as `docs/language/examples/virtio-storage.wr`
                // spells it. Intercepted only when the struct declares no
                // `claim` of its own, so an ordinary declaration under
                // that name still wins and no existing program changes
                // meaning.
                if name == "claim" {
                    return check_device_claim(bname, args, fspan, call_span, fctx, mctx);
                }
                return Err(type_error(
                    format!("type `{bname}` has no associated function `{name}`"),
                    fspan,
                ));
            }
            if let Some(e) = mctx.enums.get(bname.as_str()) {
                // plans/M9.md item B2: associated fn before variant — a
                // call `Color.from(...)` is a method, not "no variant".
                if let Some((af, d)) = e.assoc_fn(name) {
                    // Generic-enum assoc fns stay deferred; method-owned
                    // type params on a non-generic enum instantiate (Q).
                    if !e.generics.is_empty() {
                        return Err(unimplemented_at("generic instantiation is", call_span));
                    }
                    if !d.generics.is_empty() {
                        let recv_ty = Type::Named(bname.clone(), vec![]);
                        return check_method_generic_call(
                            &recv_ty, name, d, args, None, call_span, None, fctx, mctx,
                        );
                    }
                    let typed_args =
                        check_call_args(&af.params, &d.params, args, call_span, fctx, mctx)?;
                    let key = CalleeKey::Method(bname.clone(), name.to_string());
                    return Ok(TypedExpr {
                        ty: resolved_ret(&d.ret, Some(bname), name, mctx),
                        kind: TypedExprKind::Call {
                            callee: key,
                            receiver: None,
                            args: typed_args,
                        },
                    });
                }
                if e.variants.iter().any(|v| v.name == name) {
                    // plans/M9.md item J2c: `Lookup.Found(x)` under expected
                    // `Lookup[u32]` — instantiate, then check payload args
                    // against the substituted variant.
                    let (targs, decl) =
                        resolve_enum_for_variant_construction(bname, e, expected, call_span, mctx)?;
                    let dv = decl
                        .variants
                        .iter()
                        .find(|v| v.name == name)
                        .expect("name membership checked above");
                    let payload_types = decl_variant_payload_types(dv);
                    let typed_args =
                        check_variant_args(&payload_types, args, call_span, fctx, mctx)?;
                    // plans/M9.md item DD / decision 9: local key, not
                    // `e.name` — see the fieldless arm in `check_field_expr`.
                    return Ok(TypedExpr {
                        ty: Type::Named(bname.clone(), targs),
                        kind: TypedExprKind::EnumConstruct {
                            enum_name: bname.clone(),
                            variant: name.to_string(),
                            args: typed_args,
                        },
                    });
                }
                return Err(type_error(
                    format!("enum `{bname}` has no variant or associated function `{name}`"),
                    fspan,
                ));
            }
        }
    }
    // plans/M7.md item C (03-hardware.md §2): typed MMIO access. The whole
    // legal shape is `<mmio>.<register>.read()` / `.write(v)`, which
    // arrives here as a call whose *receiver* is a field expression over
    // an `Mmio[L]` — 03 §2's own worked example
    // (`self.irq_regs.interrupt_status.read()`), and the docs' own
    // aspirational driver (`docs/language/examples/virtio-storage.wr`,
    // lines 145/149) spelled identically. Intercepted before the receiver
    // is typed as an ordinary value, because a register selection has no
    // value form at all (`check_field_expr`'s own `Mmio` arm above says
    // so). A base whose inner expression is not an `Mmio[L]` — or does not
    // type at all — falls straight through to the ordinary path below,
    // which re-checks it and reports whatever it would have reported.
    if let Expr::Field(inner, _, register) = base {
        if let Ok(mmio) = check_expr(inner, None, fctx, mctx) {
            if let Type::Named(cap, targs) = &unwrap_own(mmio.ty.clone()) {
                if cap == "Mmio" {
                    return check_mmio_access(
                        mmio.clone(),
                        targs,
                        register,
                        name,
                        args,
                        fspan,
                        call_span,
                        fctx,
                        mctx,
                    );
                }
            }
        }
    }
    let base_t = check_expr(base, None, fctx, mctx)?;
    let base_ty = unwrap_own(base_t.ty.clone());
    // plans/M7.md item H1: a bring-up state's own operations
    // (03-hardware.md §9). `map_partition` is live; every other transition
    // in the chain is a named rejection rather than a silent "no method".
    if let Type::Named(state, targs) = &base_ty {
        if crate::eval::image_checks::is_protocol_state_type_name(state) {
            return check_device_state_call(
                base_t.clone(),
                state,
                targs,
                name,
                args,
                fspan,
                call_span,
                fctx,
                mctx,
            );
        }
    }
    // plans/M7.md item H2a, 03-hardware.md §8: `Untrusted[T]`'s only
    // source-visible transition is a checked narrowing. Intercepted
    // before the generic "no method" path so an unimplemented
    // `checked_*` name fails closed by name rather than looking like a
    // missing user method.
    if let Type::Named(marker, targs) = &base_ty {
        if marker == "Untrusted" {
            return check_untrusted_narrowing(
                base_t, targs, name, args, fspan, call_span, fctx, mctx,
            );
        }
    }
    // plans/M7.md item E2, 03-hardware.md §4: queue operations on a
    // `VirtQueue[..N]` value. Intercepted before the generic "no method"
    // path (VirtQueue is a sealed builtin, never a DeclStruct).
    if let Type::Named(q, _) = &base_ty {
        if q == "VirtQueue" {
            return check_virtqueue_method(base_t, name, args, fspan, call_span, fctx, mctx);
        }
    }
    // plans/M7.md item C: an `Mmio[L]` itself has no methods — the only
    // operations 03 §2 gives it go through a *declared register*, which
    // the arm above already took. Named here rather than falling into the
    // generic "type `Mmio` has no method" (which would then try to
    // instantiate `Mmio` as a generic struct and report something else
    // entirely).
    if let Type::Named(cap, targs) = &base_ty {
        if cap == "Mmio" {
            return Err(type_error(
                format!(
                    "`{}` has no method `{name}`; a typed register map is used only through a \
                     declared register — `<mmio>.<register>.read()` or \
                     `<mmio>.<register>.write(v)` (03-hardware.md §2){}",
                    types::render_type(&base_ty),
                    mmio_register_hint(targs, mctx),
                ),
                fspan,
            ));
        }
    }
    // plans/M7.md item G (03-hardware.md §6): `IrqCap[V]`'s two
    // operations — `bind(handler)` and `unmask()`. Binding (not a keyword)
    // is what makes the handler an ISR; the sealed graph pass
    // (`eval::image_checks::check_vector_bindings`) is what enforces
    // "exactly one handler per vector" and "source cannot bind an unowned
    // vector".
    if let Type::Named(cap, _) = &base_ty {
        if cap == "IrqCap" {
            return check_irq_cap_call(base_t, name, args, fspan, call_span, fctx, mctx);
        }
    }
    // plans/M7.md item G, decision 17: `InterruptCell[T]`'s acquire/release
    // ops (03-hardware.md §6).
    if let Type::Named(cell, _) = &base_ty {
        if cell == "InterruptCell" {
            return check_interrupt_cell_call(base_t, name, args, fspan, call_span, fctx, mctx);
        }
    }
    // Plans/M6.md item A (02-language.md §9.4/§9.5): a bare (non-`await`/
    // `send`) call through an `Actor[T]` handle names a message method —
    // the language gives that no synchronous meaning, so it is rejected
    // here, named, rather than falling through to "no method" below.
    // `g.start(...)` is the one `Group` method callable bare (it does not
    // suspend — it only registers a child); `g.join_all()` bare falls to
    // the same "must be awaited" rejection as an actor call.
    if let Type::Named(outer, _) = &base_ty {
        if outer == "Actor" {
            return Err(type_error(
                format!(
                    "calling `{name}` through an `Actor[T]` handle requires `await` or `send`, \
                     not a bare call"
                ),
                call_span,
            ));
        }
        if outer == "Group" {
            return match name {
                "start" => check_group_start(base_t, args, call_span, fctx, mctx),
                "join_all" => Err(type_error(
                    "`join_all` must be `await`ed".to_string(),
                    call_span,
                )),
                other => Err(type_error(
                    format!("`Group` has no method `{other}`"),
                    fspan,
                )),
            };
        }
    }
    if base_ty == image_type() {
        return check_image_method_intrinsic(name, args, call_span, fctx, mctx);
    }
    if base_ty == image_decl_type() {
        return check_image_decl_method_intrinsic(base_t, name, args, fspan, call_span, fctx, mctx);
    }
    // plans/M9.md item F3 decision 347: sealed `[T; N].map_take` /
    // `try_map_take` (05-library.md §7).
    if let Type::Array(elem, len) = &base_ty {
        if name == "map_take" || name == "try_map_take" {
            return check_array_map_take(
                base_t, name, elem, len, args, fspan, call_span, fctx, mctx,
            );
        }
        if name == "each" || name == "each_mut" {
            return Err(type_error(
                format!(
                    "`[{}; N]` has no method `{name}`; lent array iteration is \
                     `List[T, ..N].each` / `.each_mut` (05-library.md §7)",
                    types::render_type(elem)
                ),
                fspan,
            ));
        }
    }
    match &base_ty {
        Type::Named(sname, targs) => {
            // A method call through a generic instantiation (item H):
            // substitute + enqueue it, then check the call against the
            // substituted method's (now concrete) signature.
            if let Some(s) = if targs.is_empty() {
                mctx.structs
                    .get(sname.as_str())
                    .map(std::borrow::Cow::Borrowed)
            } else if mctx.structs.contains_key(sname.as_str()) {
                Some(std::borrow::Cow::Owned(generics::instantiate_struct(
                    mctx, sname, targs, call_span,
                )?))
            } else {
                None
            } {
                let Some((mf, d)) = s.method(name) else {
                    return Err(missing_method_error(
                        format!("type `{sname}` has no method `{name}`"),
                        sname,
                        name,
                        fspan,
                    ));
                };
                if !d.generics.is_empty() {
                    let recv_ty = Type::Named(sname.clone(), targs.clone());
                    return check_method_generic_call(
                        &recv_ty,
                        name,
                        d,
                        args,
                        None,
                        call_span,
                        Some(base_t),
                        fctx,
                        mctx,
                    );
                }
                let typed_args =
                    check_call_args(&mf.params, &d.params, args, call_span, fctx, mctx)?;
                let key = if targs.is_empty() {
                    CalleeKey::Method(sname.clone(), name.to_string())
                } else {
                    CalleeKey::MethodInstance(
                        generics::canonical_key(InstKind::Struct, sname, targs),
                        name.to_string(),
                    )
                };
                return Ok(TypedExpr {
                    ty: resolved_ret(&d.ret, Some(sname), name, mctx),
                    kind: TypedExprKind::Call {
                        callee: key,
                        receiver: Some(Box::new(base_t)),
                        args: typed_args,
                    },
                });
            }
            // plans/M9.md item B2: the same `Type::Named` method path for
            // enums — static name-keyed lookup, no dispatch.
            if targs.is_empty() {
                if let Some(e) = mctx.enums.get(sname.as_str()) {
                    let Some((mf, d)) = e.method(name) else {
                        return Err(missing_method_error(
                            format!("type `{sname}` has no method `{name}`"),
                            sname,
                            name,
                            fspan,
                        ));
                    };
                    if !d.generics.is_empty() {
                        let recv_ty = Type::Named(sname.clone(), vec![]);
                        return check_method_generic_call(
                            &recv_ty,
                            name,
                            d,
                            args,
                            None,
                            call_span,
                            Some(base_t),
                            fctx,
                            mctx,
                        );
                    }
                    let typed_args =
                        check_call_args(&mf.params, &d.params, args, call_span, fctx, mctx)?;
                    return Ok(TypedExpr {
                        ty: resolved_ret(&d.ret, Some(sname), name, mctx),
                        kind: TypedExprKind::Call {
                            callee: CalleeKey::Method(sname.clone(), name.to_string()),
                            receiver: Some(Box::new(base_t)),
                            args: typed_args,
                        },
                    });
                }
            } else if mctx.enums.contains_key(sname.as_str()) {
                return Err(unimplemented_at("generic instantiation is", call_span));
            }
            Err(missing_method_error(
                format!("type `{sname}` has no method `{name}`"),
                sname,
                name,
                fspan,
            ))
        }
        other => {
            // plans/M9.md item C2: core scalars have standard Format
            // (archive §10) — `.format() -> String[..K]` with fixed K.
            if name == "format" {
                if !args.is_empty() {
                    return Err(type_error(
                        format!("too many arguments, expected 0, found {}", args.len()),
                        call_span,
                    ));
                }
                if let Some(k) = types::scalar_format_bound(other) {
                    return Ok(TypedExpr {
                        ty: Type::String(Box::new(Expr::Int(call_span, k.to_string()))),
                        kind: TypedExprKind::Call {
                            callee: CalleeKey::Method(
                                types::render_type(other),
                                "format".to_string(),
                            ),
                            receiver: Some(Box::new(base_t)),
                            args: vec![],
                        },
                    });
                }
            }
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

// --- plans/M7.md item C: typed MMIO access (03-hardware.md §2) ------------
//
// "Raw integer-address MMIO does not exist in the safe language." The only
// two operations that exist are a *declared* register's read and write:
//
//     status = self.irq_regs.interrupt_status.read()
//     self.irq_regs.interrupt_ack.write(handled)
//
// Everything about the access comes from the declaration and nothing from
// the call site:
//
// - **Direction** is the register's own `ReadOnly[T]`/`WriteOnly[T]`
//   wrapper. Reading a `WriteOnly` and writing a `ReadOnly` are two
//   distinct named rejections; a register declared with no wrapper at all
//   has no direction and neither operation exists on it (a third named
//   rejection, not a permissive default).
// - **Width** is `T` — `read()`'s result type and `write(v)`'s argument
//   type *are* the declared register type, so a `u32` register cannot be
//   read into a `u64` or written from a `u16` without the ordinary
//   `.to[T]()` conversion the language already requires. The compiler
//   never widens or truncates a register access silently.
// - **Offset/alignment/bounds** are `check_layouts`' already-checked table
//   (`hardware.layout.exact-bytes`): a register access can only ever name
//   an entry of that table, so there is no second bounds rule here.
// - **Endianness** is the layout's declared `endian=`, checked against the
//   target ABI (03 §2: "The compiler and target ABI check ... and
//   endianness"). This machine is little-endian A76 (06-machine.md §2), so
//   a `@layout(mmio, endian=big)` access needs a byte swap this compiler
//   does not emit, and fails closed rather than reading the wrong bytes.
//
// ## The node, and why it is an `Intrinsic`
//
// `TypedExprKind::Intrinsic { key: "Mmio.read" | "Mmio.write" }`, the same
// vehicle `now`/`ms`/`Group.start`/the whole `@image` builder surface
// already use for an operation with no declared parameter list to align
// against. `receiver` is the `Mmio[L]` expression, `type_arg` is the
// register's declared scalar `T`, and `args` carries the register's own
// name (a `Str` leaf — it names a declared entry, not a value) plus, for a
// write, the value. `eval::legal` gains an arm for both keys in *both* of
// its scans: a hardware touch for provenance (03 §1) and a comptime-
// illegal operation for legality (02-language.md §12: "free of I/O ... and
// hardware effects").
//
// ## What does not exist yet, and exactly why
//
// **Nothing lowers.** `lower.rs` fails closed on both keys, by name, and
// that is the honest state rather than a shortcut: **no `Mmio[L]` value
// can exist at runtime today**, so any code emitted for one of these would
// read or write a fixed offset from whatever a zero-initialized field
// happens to hold. Two independent blockers, both outside this item's
// files, both verified by running the compiler rather than by reading it:
//
//   1. `eval::image_checks::check_capability_substitution` rejects an
//      `Mmio[L]` `init` parameter outright ("nothing mints a `Mmio` yet").
//      That is the mint, and it is the one arm that has to change.
//   2. Even with it changed, `layout::build_boot_init_calls` walks
//      `graph.actors` only — a `@driver`'s `init` is never called at boot
//      at all — and it fails closed on *every* capability parameter
//      besides. So no capability of any kind has bytes at boot today,
//      `DeviceCap[D]` included.
//
// Emitting a load/store against a provably-zero base is the exact shape of
// wrong answer plans/M7.md item W was written to close (an image that
// booted, asserted, and reported success against a field that was never
// materialized). So the access type-checks in full and the backend says
// so; when the mint lands, the fail-closed arm is what has to be replaced.

/// The register-name hint appended to an `Mmio[L]` diagnostic: what this
/// layout actually declares. Empty when the layout cannot be found at all
/// (which `validate_capability_args` has already made impossible for a
/// type that reached body checking — belt and braces, never a panic).
fn mmio_register_hint(targs: &[types::TypeArg], mctx: &ModuleCtx) -> String {
    let Some(layout) = mmio_layout_of(targs, mctx) else {
        return String::new();
    };
    let names = types::mmio_register_names(layout);
    if names.is_empty() {
        return String::new();
    }
    format!(
        "; `{}` declares {}",
        layout.name,
        names
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The `@layout(mmio)` table entry an `Mmio[L]`'s own type arguments name.
fn mmio_layout_of<'a>(
    targs: &[types::TypeArg],
    mctx: &'a ModuleCtx,
) -> Option<&'a types::LayoutType> {
    match targs.first() {
        Some(types::TypeArg::Type(Type::Named(l, _))) => mctx.layouts.get(l.as_str()),
        _ => None,
    }
}

/// `check_field_expr`'s own `Mmio[L]` rejection: a bare register selection
/// (`m.status` with no `.read()`/`.write(v)` after it), or a name that is
/// not a declared register at all. Two different mistakes, two different
/// messages — a reader who mistyped a register name should not be told
/// their register is not a value.
fn mmio_bare_selection_error(
    targs: &[types::TypeArg],
    name: &str,
    span: Span,
    mctx: &ModuleCtx,
) -> SemaError {
    let layout = mmio_layout_of(targs, mctx);
    let known = layout.is_some_and(|l| types::mmio_register(l, name).is_some());
    if !known {
        return type_error(
            format!(
                "`{}` declares no register `{name}`{}",
                layout
                    .map(|l| l.name.clone())
                    .unwrap_or_else(|| "this `@layout(mmio)` type".to_string()),
                mmio_register_hint(targs, mctx),
            ),
            span,
        );
    }
    type_error(
        format!(
            "register `{}.{name}` is not a value; an MMIO register exists only as an access — \
             write `.read()` or `.write(v)` (03-hardware.md §2)",
            layout.map(|l| l.name.as_str()).unwrap_or("?"),
        ),
        span,
    )
}

/// One `<mmio>.<register>.read()` / `.write(v)` (03-hardware.md §2).
#[allow(clippy::too_many_arguments)]
fn check_mmio_access(
    mmio: TypedExpr,
    targs: &[types::TypeArg],
    register: &str,
    op: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let Some(layout) = mmio_layout_of(targs, mctx) else {
        // Unreachable through a checked program: `Mmio[L]` does not
        // resolve at all unless `L` is an `@layout(mmio)` struct of this
        // module (`types::validate_capability_args`). Named, not panicked.
        return Err(type_error(
            format!(
                "`{}`'s layout is not a declared `@layout(mmio)` type (03-hardware.md §2)",
                types::render_type(&mmio.ty)
            ),
            fspan,
        ));
    };
    let Some(reg) = types::mmio_register(layout, register) else {
        return Err(type_error(
            format!(
                "`{}` declares no register `{register}`{}",
                layout.name,
                mmio_register_hint(targs, mctx)
            ),
            fspan,
        ));
    };
    if !matches!(op, "read" | "write") {
        return Err(type_error(
            format!(
                "register `{}.{register}` has no operation `{op}`; a declared MMIO register is \
                 read with `.read()` or written with `.write(v)` (03-hardware.md §2)",
                layout.name
            ),
            fspan,
        ));
    }

    // Direction, from the declaration alone.
    let declared = format!("{}.{register}: {}", layout.name, register_type_text(&reg));
    match (reg.direction, op) {
        (Some(types::MmioDirection::ReadOnly), "read")
        | (Some(types::MmioDirection::WriteOnly), "write") => {}
        (Some(types::MmioDirection::WriteOnly), _) => {
            return Err(type_error(
                format!(
                    "register `{declared}` is write-only and cannot be read (03-hardware.md §2: a \
                     register's declared direction governs its access)"
                ),
                call_span,
            ));
        }
        (Some(types::MmioDirection::ReadOnly), _) => {
            return Err(type_error(
                format!(
                    "register `{declared}` is read-only and cannot be written (03-hardware.md §2: \
                     a register's declared direction governs its access)"
                ),
                call_span,
            ));
        }
        (None, _) => {
            return Err(type_error(
                format!(
                    "register `{declared}` declares no direction, so it has neither a `read()` \
                     nor a `write(v)`; a register map's fields are `ReadOnly[T]` or `WriteOnly[T]` \
                     (03-hardware.md §2)"
                ),
                call_span,
            ));
        }
    }

    // Endianness, against the target ABI (03 §2: "The compiler and target
    // ABI check width, alignment, non-overlap, bounds, and endianness").
    if layout.endian != types::LayoutEndian::Little {
        return Err(unimplemented_at(
            &format!(
                "an access to `{declared}`, whose `@layout(mmio, endian={})` disagrees with this \
                 target's little-endian ABI (06-machine.md §2) and would need a byte swap that is \
                 not emitted, is",
                layout.endian.as_str()
            ),
            call_span,
        ));
    }

    // Width: the register's declared scalar *is* the operation's type.
    let Some(scalar) = scalar_type_by_name(&reg.scalar) else {
        return Err(type_error(
            format!("register `{declared}` has no scalar register type (03-hardware.md §2)"),
            fspan,
        ));
    };

    let mut intrinsic_args = vec![(
        "register".to_string(),
        TypedExpr {
            ty: Type::Static(Box::new(Type::Str)),
            kind: TypedExprKind::Str(register.to_string()),
        },
    )];
    let ty = match op {
        "read" => {
            if !args.is_empty() {
                return Err(type_error(
                    format!("`{}.{register}.read()` takes no arguments", layout.name),
                    call_span,
                ));
            }
            scalar.clone()
        }
        _ => {
            let [arg] = args else {
                return Err(type_error(
                    format!(
                        "`{}.{register}.write(v)` takes exactly one argument, the {} value to \
                         write; found {}",
                        layout.name,
                        types::render_type(&scalar),
                        args.len()
                    ),
                    call_span,
                ));
            };
            if let Some(label) = &arg.label {
                return Err(type_error(
                    format!(
                        "`{}.{register}.write(v)`'s value is positional; `{label}=` names no \
                         parameter",
                        layout.name
                    ),
                    arg.span,
                ));
            }
            // The register's declared width governs the write, by the one
            // mechanism every declared parameter position in this compiler
            // already uses: the scalar is handed to `check_expr` as the
            // *expected* type. An integer literal therefore takes the
            // register's own width (`ack: WriteOnly[u16]` accepts
            // `.write(7)` as a `u16`), and anything else gets the ordinary
            // `expected `u32`, found `u64`` rejection from inside
            // `check_expr` — including a `never` (`panic(...)`). A second,
            // post-hoc `types_eq` guard was written here first and then
            // deleted: no expression reaches it, because `check_expr` with
            // an expected type has already rejected every mismatch.
            let value = check_expr(&arg.value, Some(&scalar), fctx, mctx)?;
            intrinsic_args.push(("value".to_string(), value));
            Type::Unit
        }
    };

    Ok(TypedExpr {
        ty,
        kind: TypedExprKind::Intrinsic {
            key: format!("Mmio.{op}"),
            receiver: Some(Box::new(mmio)),
            type_arg: Some(scalar),
            args: intrinsic_args,
        },
    })
}

/// A register's declared type as source wrote it (`ReadOnly[u32]`, or a
/// bare `u32` for a field with no direction) — the diagnostics quote the
/// declaration, never a reconstruction of it.
fn register_type_text(reg: &types::MmioRegister) -> String {
    match reg.direction {
        Some(d) => format!("{}[{}]", d.wrapper(), reg.scalar),
        None => reg.scalar.clone(),
    }
}

/// Is `key` one of plans/M7.md item C's two MMIO access intrinsics? One
/// list, read by `eval::legal` (twice: provenance and comptime legality)
/// and by `lower.rs`'s own fail-closed arm — three consumers that must
/// agree on exactly which keys are a hardware effect.
pub fn is_mmio_access_intrinsic(key: &str) -> bool {
    matches!(key, "Mmio.read" | "Mmio.write")
}

// --- plans/M7.md item H2a / plans/M9.md item G1: `Untrusted[T]` --------
//
// One marked-value mechanism, three policies (03-hardware.md §8 /
// 05-library.md §6). `Untrusted[T]` is live; `Validated` / `Secret` are
// refused by name at resolve time (`types::resolve_named`) — M9 G2/G3
// deferrals (decisions 353–355), not unknown-type misses. The wrapper is
// sealed: no source-visible constructor, no ordinary use as an index /
// length / allocation size / bound / arithmetic operand / comparison,
// and exactly one implemented narrowing — `checked_le(bound)` — which
// yields `Result[T, unit]`, lowers to a real compare + branch, and
// evaluates as a pure compare at comptime (decision 352).
//
// ## Where a marked value comes from
//
// `IoCompletion[P].written_len` is the live producer (plans/M7.md item E4 /
// decision 22) — `golden/boot-blk-roundtrip` exercises both `checked_le`
// outcomes on a real used-ring length. A source-visible `Untrusted.mark`
// constructor stays rejected.

/// `Untrusted[<inner>]`.
pub(crate) fn untrusted_type(inner: Type) -> Type {
    Type::Named("Untrusted".to_string(), vec![types::TypeArg::Type(inner)])
}

/// Is `ty` the marked wrapper `Untrusted[_]`?
fn is_untrusted_type(ty: &Type) -> bool {
    matches!(ty, Type::Named(name, _) if name == "Untrusted")
}

/// The payload type inside `Untrusted[T]`, when `ty` is one.
#[allow(dead_code)] // reserved for the self-audit / table-driven guards
fn untrusted_payload(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Named(name, targs) if name == "Untrusted" => match targs.first() {
            Some(types::TypeArg::Type(inner)) => Some(inner),
            _ => None,
        },
        _ => None,
    }
}

/// 03-hardware.md §8's rejection for an ordinary use of a marked value:
/// names the use and the one transition that would clear it.
fn untrusted_use_error(use_kind: &str, span: Span) -> SemaError {
    type_error(
        format!(
            "`Untrusted[T]` cannot be used as {use_kind} until checked-narrowed — write \
             `.checked_le(bound)` (03-hardware.md §8)"
        ),
        span,
    )
}

/// When an expected type is unmarked and the found type is `Untrusted[_]`,
/// prefer the mechanism's wording over a bare expected/found mismatch.
fn untrusted_coercion_message(expected: &Type, found: &Type) -> Option<String> {
    if !is_untrusted_type(found) {
        return None;
    }
    // Coercing *into* Untrusted from a plain value is also refused — the
    // wrapper is sealed — but that case is `expected` being Untrusted,
    // handled by the ordinary mismatch (or by the constructor arm).
    if is_untrusted_type(expected) {
        return None;
    }
    Some(format!(
        "`Untrusted[T]` cannot be used as a plain `{}` until checked-narrowed — write \
         `.checked_le(bound)` (03-hardware.md §8); expected `{}`, found `{}`",
        types::render_type(expected),
        types::render_type(expected),
        types::render_type(found),
    ))
}

/// Is `key` the one checked-narrowing intrinsic H2a emits?
pub fn is_untrusted_narrowing_intrinsic(key: &str) -> bool {
    key == "Untrusted.checked_le"
}

/// One `reported.checked_le(bound)` (03-hardware.md §8's own spelling).
#[allow(clippy::too_many_arguments)]
fn check_untrusted_narrowing(
    receiver: TypedExpr,
    targs: &[types::TypeArg],
    name: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let Some(types::TypeArg::Type(inner)) = targs.first() else {
        return Err(type_error(
            "`Untrusted` with no payload type argument".to_string(),
            fspan,
        ));
    };
    if !is_integer_scalar(inner) {
        return Err(type_error(
            format!(
                "`Untrusted[{}]` cannot be checked-narrowed: the payload must be an integer \
                 scalar (03-hardware.md §8's `Untrusted[usize]` worked example)",
                types::render_type(inner)
            ),
            fspan,
        ));
    }
    // Only `checked_le` is implemented. Every other `checked_*` the docs
    // could be read to imply fails closed by name rather than half-built.
    if name != "checked_le" {
        if name.starts_with("checked_") {
            return Err(unimplemented_at(
                &format!(
                    "`Untrusted[T].{name}` (03-hardware.md §8 spells only `.checked_le(bound)`; \
                     any other checked narrowing is"
                ),
                call_span,
            ));
        }
        return Err(type_error(
            format!(
                "`Untrusted[{}]` has no method `{name}`; the only source-visible transition is \
                 `.checked_le(bound)` (03-hardware.md §8)",
                types::render_type(inner)
            ),
            fspan,
        ));
    }
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`Untrusted[{}].checked_le(bound)` takes exactly one argument; found {}",
                types::render_type(inner),
                args.len()
            ),
            call_span,
        ));
    };
    if let Some(label) = &arg.label {
        if label != "bound" {
            return Err(type_error(
                format!(
                    "`Untrusted[{}].checked_le(bound)`'s argument is positional or `bound=`; \
                     `{label}=` names no parameter",
                    types::render_type(inner)
                ),
                arg.span,
            ));
        }
    }
    let bound = check_expr(&arg.value, Some(inner), fctx, mctx)?;
    Ok(TypedExpr {
        ty: Type::Result(Box::new(inner.clone()), Box::new(Type::Unit)),
        kind: TypedExprKind::Intrinsic {
            key: "Untrusted.checked_le".to_string(),
            receiver: Some(Box::new(receiver)),
            type_arg: Some(inner.clone()),
            args: vec![("bound".to_string(), bound)],
        },
    })
}

// --- plans/M7.md item H1: 03-hardware.md §9's sealed transport ------------
//
// The bring-up chain, and the two of its operations this item makes real.
//
// ## The chain, and what a state *is*
//
// `Reset -> Acknowledged -> DriverClaimed -> FeaturesNegotiated ->
// FeaturesAccepted -> QueuesConfigured -> Running`, one builtin type per
// state (`eval::image_checks::PROTOCOL_STATE_TYPES`), each carrying the
// device type — `RunningDevice[VirtioBlock]` is the docs' own spelling and
// the other six follow it. Every one is a resource, which is not a
// decoration: §9's "each fallible transition **consumes** its input state"
// *is* the resource rule, and the only reason a transition can consume one
// is that it is never implicitly copied.
//
// ## `claim`, and why it emits nothing on this target
//
// `VirtioBlock.claim(cap=take cap)` consumes the `DeviceCap[D]` and yields
// `DriverClaimedDevice[D]` — the docs' own comment on the line is "reset +
// acknowledge", i.e. the three status writes a real virtio transport needs
// to walk `Reset -> Acknowledged -> DriverClaimed`. **This machine has no
// status register to write.** 06-machine.md §3: "no discovery ... the VMM
// preconfigures every device, queue, and shared-memory window the report
// declares — device topology is a *build output*, not a probed fact", and
// "cold boot is a design property: there is nothing to negotiate". The VMM
// has no `MagicValue`/`DeviceID`/`Status` register file at all
// (`wrela-vmm::devices`' module doc). So on machine v1 `claim` is a pure
// authority transition: it carries the device's base address forward and
// emits no access. That is a target fact, recorded, not an omission — and
// it is exactly why the *first* MMIO this compiler ever emits is the
// driver's own ISR partition rather than a status write.
//
// ## `map_partition`, and how it feeds item C's rule instead of dodging it
//
// `claimed.map_partition(VirtioIrqMmio)` yields `Mmio[VirtioIrqMmio]`.
// 03 §2: "a driver **or sealed protocol** partitions its claim into
// declared, non-overlapping layouts ... minting a layout consumes those
// byte ranges from the claim". Item C built the *rule* over a driver's
// declared `Mmio[L]` **fields**; this is the *operation*, so the operation
// is constrained to that same set: `map_partition(L)` is legal only inside
// a `@driver` that declares `Mmio[L]` in a field. A partition the no-alias
// rule never saw therefore cannot exist, and the `devregs` window that
// backs the claim is sized from the identical set
// (`layout::device_register_windows`).
//
// ## What is deliberately not here
//
// `negotiate`/`start`/`read_capacity_sectors`/`take_irq`/`VirtQueue.configure`
// are each a named rejection carrying the state they would consume, the
// state they would produce, and what is actually missing. `negotiate` in
// particular is *not* merely unimplemented: on this machine the accepted
// feature set is decided before the guest runs (item F's VMM-side
// `negotiate`, against the image's declared `required_features`), and
// nothing carries that result into the guest — there is no declared window
// for it and no plan item has claimed one. Failing closed says so.

/// The device type an `Mmio`/state type argument names, if it names one.
fn device_type_arg(targs: &[types::TypeArg]) -> Option<&str> {
    match targs.first() {
        Some(types::TypeArg::Type(Type::Named(d, _))) => Some(d.as_str()),
        _ => None,
    }
}

/// `<Device>.claim(cap=take cap)` (03-hardware.md §9).
fn check_device_claim(
    device: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`{device}.claim(cap=take cap)` takes exactly one argument, the `DeviceCap[{device}]` \
                 the image minted; found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if arg.label.as_deref() != Some("cap") {
        return Err(type_error(
            format!(
                "`{device}.claim`'s own argument is labelled `cap=` (03-hardware.md §9's own \
                 spelling: `{device}.claim(cap=take cap)`)"
            ),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Take {
        return Err(type_error(
            format!(
                "`{device}.claim` consumes the capability: write `cap=take ...` \
                 (03-hardware.md §9 — each transition consumes its input)"
            ),
            arg.span,
        ));
    }
    let expected = Type::Named(
        "DeviceCap".to_string(),
        vec![types::TypeArg::Type(Type::Named(
            device.to_string(),
            vec![],
        ))],
    );
    let cap = check_expr(&arg.value, Some(&expected), fctx, mctx)?;
    let cap_ty = unwrap_own(cap.ty.clone());
    let Type::Named(cap_name, cap_targs) = &cap_ty else {
        return Err(type_error(
            format!(
                "`{device}.claim`'s own `cap=` is a `DeviceCap[{device}]`; found `{}`",
                types::render_type(&cap.ty)
            ),
            arg.span,
        ));
    };
    if cap_name != "DeviceCap" || device_type_arg(cap_targs) != Some(device) {
        return Err(type_error(
            format!(
                "`{device}.claim`'s own `cap=` is a `DeviceCap[{device}]` — authority over *this* \
                 device (03-hardware.md §1); found `{}`",
                types::render_type(&cap.ty)
            ),
            arg.span,
        ));
    }
    let _ = fspan;
    Ok(TypedExpr {
        ty: Type::Named(
            "DriverClaimedDevice".to_string(),
            vec![types::TypeArg::Type(Type::Named(
                device.to_string(),
                vec![],
            ))],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "Device.claim".to_string(),
            receiver: None,
            type_arg: Some(Type::Named(device.to_string(), vec![])),
            args: vec![("cap".to_string(), cap)],
        },
    })
}

/// A method call on one of 03 §9's bring-up states.
#[allow(clippy::too_many_arguments)]
fn check_device_state_call(
    state_expr: TypedExpr,
    state: &str,
    targs: &[types::TypeArg],
    method: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let rendered = types::render_type(&state_expr.ty);
    let device = device_type_arg(targs).unwrap_or("?").to_string();
    match method {
        "map_partition" => {
            check_map_partition(state_expr, &rendered, args, fspan, call_span, fctx, mctx)
        }
        // plans/M7.md item E1, decision 14: `negotiate` is a **build-time**
        // fact. Both sides (the image's `required_features`, and
        // `virtqueue::DEVICE_FEATURES`) are build outputs; an unofferable
        // required feature fails the *build*, and the guest's call is a
        // pure authority transition that always yields
        // `Ok(FeaturesAcceptedDevice[D])`. The call-site `required=`/
        // `optional=` arrays are shape-checked here; the bits themselves
        // are checked against the model when the image seals
        // (`check_blk_device_features`).
        "negotiate" => check_device_negotiate(
            state_expr, state, &device, &rendered, args, fspan, call_span, fctx, mctx,
        ),
        "start" => check_device_start(
            state_expr, state, &device, &rendered, args, fspan, call_span,
        ),
        "reset" => check_device_reset(
            state_expr, state, &device, &rendered, args, fspan, call_span, fctx, mctx,
        ),
        "read_capacity_sectors" => check_device_read_capacity(
            state_expr, state, &device, &rendered, args, fspan, call_span,
        ),
        "take_irq" => {
            if !args.is_empty() {
                return Err(type_error(
                    format!(
                        "`{rendered}.take_irq()` takes no arguments; found {}",
                        args.len()
                    ),
                    call_span,
                ));
            }
            let _ = fspan;
            Ok(TypedExpr {
                ty: Type::Named("IrqCap".to_string(), vec![types::TypeArg::Type(Type::U32)]),
                kind: TypedExprKind::Intrinsic {
                    key: "Device.take_irq".to_string(),
                    receiver: Some(Box::new(state_expr)),
                    type_arg: None,
                    args: Vec::new(),
                },
            })
        }
        other => Err(type_error(
            format!(
                "`{rendered}` has no operation `{other}`; 03-hardware.md §9's bring-up chain \
                 gives a claimed device `map_partition`, `negotiate`, `read_capacity_sectors`, \
                 `take_irq` and `start`; `reset` consumes a `RunningDevice` (plans/M7.md item H2b)"
            ),
            fspan,
        )),
    }
}

fn boot_error_ty() -> Type {
    Type::Named("BootError".to_string(), vec![])
}

fn device_state_ty(state: &str, device: &str) -> Type {
    Type::Named(
        state.to_string(),
        vec![types::TypeArg::Type(Type::Named(
            device.to_string(),
            vec![],
        ))],
    )
}

/// `claimed.negotiate(required=..., optional=...)` — DriverClaimed ->
/// FeaturesAccepted (Result). plans/M7.md decision 14.
#[allow(clippy::too_many_arguments)]
fn check_device_negotiate(
    state_expr: TypedExpr,
    state: &str,
    device: &str,
    rendered: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if state != "DriverClaimedDevice" {
        return Err(type_error(
            format!(
                "`{rendered}.negotiate(...)` consumes a `DriverClaimedDevice[{device}]` \
                 (03-hardware.md §9: `DriverClaimed -> FeaturesAccepted`); found `{rendered}`"
            ),
            fspan,
        ));
    }
    if args.len() != 2 {
        return Err(type_error(
            format!(
                "`{rendered}.negotiate(required=..., optional=...)` takes exactly two labelled \
                 arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let mut required = None;
    let mut optional = None;
    for arg in args {
        match arg.label.as_deref() {
            Some("required") => {
                if required.is_some() {
                    return Err(type_error(
                        "`negotiate`'s `required=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`negotiate`'s `required=` is a feature list, not a moved value: \
                             drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                required = Some(check_expr(&arg.value, None, fctx, mctx)?);
            }
            Some("optional") => {
                if optional.is_some() {
                    return Err(type_error(
                        "`negotiate`'s `optional=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`negotiate`'s `optional=` is a feature list, not a moved value: \
                             drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                optional = Some(check_expr(&arg.value, None, fctx, mctx)?);
            }
            Some(other) => {
                return Err(type_error(
                    format!(
                        "`negotiate`'s own arguments are labelled `required=` and `optional=`; \
                         `{other}=` names no parameter"
                    ),
                    arg.span,
                ));
            }
            None => {
                return Err(type_error(
                    "`negotiate(required=..., optional=...)` requires labelled arguments \
                     (03-hardware.md §9 / docs/language/examples/virtio-storage.wr)"
                        .to_string(),
                    arg.span,
                ));
            }
        }
    }
    let (Some(required), Some(optional)) = (required, optional) else {
        return Err(type_error(
            format!(
                "`{rendered}.negotiate` needs both `required=` and `optional=` \
                 (03-hardware.md §9)"
            ),
            call_span,
        ));
    };
    // Feature lists are arrays (or empty-looking literals). Their element
    // type is a user enum of feature names; the *bits* are checked at
    // image seal, not here — this is the shape half.
    for (label, expr) in [("required", &required), ("optional", &optional)] {
        match &expr.ty {
            Type::Array(_, _) => {}
            other => {
                return Err(type_error(
                    format!(
                        "`negotiate`'s `{label}=` is a feature list (`[...]`); found `{}`",
                        types::render_type(other)
                    ),
                    call_span,
                ));
            }
        }
    }
    let _ = fspan;
    let accepted = device_state_ty("FeaturesAcceptedDevice", device);
    Ok(TypedExpr {
        ty: Type::Result(Box::new(accepted), Box::new(boot_error_ty())),
        kind: TypedExprKind::Intrinsic {
            key: "Device.negotiate".to_string(),
            receiver: Some(Box::new(state_expr)),
            type_arg: Some(Type::Named(device.to_string(), vec![])),
            args: vec![
                ("required".to_string(), required),
                ("optional".to_string(), optional),
            ],
        },
    })
}

/// `negotiated.start()` — QueuesConfigured -> Running (infallible on
/// this machine: the queue was already placed at configure).
fn check_device_start(
    state_expr: TypedExpr,
    state: &str,
    device: &str,
    rendered: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
) -> Result<TypedExpr, SemaError> {
    if state != "QueuesConfiguredDevice" {
        return Err(type_error(
            format!(
                "`{rendered}.start()` consumes a `QueuesConfiguredDevice[{device}]` \
                 (03-hardware.md §9's final `-> Running` transition); found `{rendered}`. \
                 Call `VirtQueue.configure(...)` first"
            ),
            fspan,
        ));
    }
    if !args.is_empty() {
        return Err(type_error(
            format!(
                "`{rendered}.start()` takes no arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    Ok(TypedExpr {
        ty: device_state_ty("RunningDevice", device),
        kind: TypedExprKind::Intrinsic {
            key: "Device.start".to_string(),
            receiver: Some(Box::new(state_expr)),
            type_arg: Some(Type::Named(device.to_string(), vec![])),
            args: Vec::new(),
        },
    })
}

/// `running.reset(queue=mut q)` — Running -> Running with a new epoch
/// (plans/M7.md item H2b / decision 23). Full device reset on machine v1;
/// per-queue reset is a typed rejection on `VirtQueue.reset`.
#[allow(clippy::too_many_arguments)]
fn check_device_reset(
    state_expr: TypedExpr,
    state: &str,
    device: &str,
    rendered: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if state != "RunningDevice" {
        return Err(type_error(
            format!(
                "`{rendered}.reset(...)` consumes a `RunningDevice[{device}]` \
                 (03-hardware.md §9: reset consumes `Running`, producing a new epoch); \
                 found `{rendered}`"
            ),
            fspan,
        ));
    }
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`{rendered}.reset(queue=mut q)` takes exactly one labelled argument; found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if arg.label.as_deref() != Some("queue") {
        return Err(type_error(
            format!(
                "`{rendered}.reset`'s own argument is labelled `queue=` (plans/M7.md item H2b: \
                 the epoch lives in the queue's control-pool bookkeeping)"
            ),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Mut {
        return Err(type_error(
            format!(
                "`{rendered}.reset` mutates the queue's live epoch in place: write `queue=mut ...`"
            ),
            arg.span,
        ));
    }
    let queue = check_expr(&arg.value, None, fctx, mctx)?;
    let queue_ty = unwrap_own(queue.ty.clone());
    let Type::Named(qname, _) = &queue_ty else {
        return Err(type_error(
            format!(
                "`{rendered}.reset`'s `queue=` is a `VirtQueue[..N]`; found `{}`",
                types::render_type(&queue.ty)
            ),
            arg.span,
        ));
    };
    if qname != "VirtQueue" {
        return Err(type_error(
            format!(
                "`{rendered}.reset`'s `queue=` is a `VirtQueue[..N]`; found `{}`",
                types::render_type(&queue.ty)
            ),
            arg.span,
        ));
    }
    let _ = fspan;
    Ok(TypedExpr {
        ty: device_state_ty("RunningDevice", device),
        kind: TypedExprKind::Intrinsic {
            key: "Device.reset".to_string(),
            receiver: Some(Box::new(state_expr)),
            type_arg: Some(Type::Named(device.to_string(), vec![])),
            args: vec![("queue".to_string(), queue)],
        },
    })
}

/// `negotiated.read_capacity_sectors()` — capacity is an image-declared,
/// report-carried fact (`BlkDevice capacity_sectors=`). The guest call
/// lowers to that build constant (decision recorded with decision 14);
/// there is no config register to read on this machine.
fn check_device_read_capacity(
    state_expr: TypedExpr,
    state: &str,
    device: &str,
    rendered: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
) -> Result<TypedExpr, SemaError> {
    if state != "FeaturesAcceptedDevice" && state != "QueuesConfiguredDevice" {
        return Err(type_error(
            format!(
                "`{rendered}.read_capacity_sectors()` is a virtio-blk config read on a \
                 features-accepted (or queues-configured) device; found `{rendered}`"
            ),
            fspan,
        ));
    }
    if !args.is_empty() {
        return Err(type_error(
            format!(
                "`{rendered}.read_capacity_sectors()` takes no arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let _ = device;
    Ok(TypedExpr {
        ty: Type::Result(Box::new(Type::U64), Box::new(boot_error_ty())),
        kind: TypedExprKind::Intrinsic {
            key: "Device.read_capacity_sectors".to_string(),
            receiver: Some(Box::new(state_expr)),
            type_arg: None,
            args: Vec::new(),
        },
    })
}

/// `<state>.map_partition(L)` (03-hardware.md §2/§9).
fn check_map_partition(
    state_expr: TypedExpr,
    rendered: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`{rendered}.map_partition(L)` takes exactly one argument, the `@layout(mmio)` \
                 type to map; found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if let Some(label) = &arg.label {
        return Err(type_error(
            format!("`map_partition(L)`'s layout is positional; `{label}=` names no parameter"),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`map_partition(L)`'s argument is a *type*, not a value: drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let Expr::Name(_, layout_name) = &arg.value else {
        return Err(type_error(
            "`map_partition(L)`'s argument names an `@layout(mmio)` type (03-hardware.md §2), \
             not a value"
                .to_string(),
            arg.span,
        ));
    };
    match mctx.layouts.get(layout_name.as_str()) {
        Some(l) if l.kind == types::LayoutKind::Mmio => {}
        _ => {
            return Err(type_error(
                format!(
                    "`map_partition({layout_name})` requires `{layout_name}` to be an \
                     `@layout(mmio)` struct (03-hardware.md §2: a typed register layout)"
                ),
                arg.span,
            ));
        }
    }
    // 03 §2's partition rule, wired to item C's own check rather than
    // restated: the layouts a `@driver` mints are exactly the ones its
    // declared `Mmio[L]` fields name, `check_mmio_claims` proves *those*
    // pairwise disjoint, and `layout::device_register_windows` sizes the
    // claim's window from the same set. A `map_partition` of anything else
    // would be a live layout no rule ever ranged over.
    let Some(Type::Named(owner, _)) = fctx.lookup_local("self").map(unwrap_own) else {
        return Err(type_error(
            format!(
                "`{rendered}.map_partition({layout_name})` partitions a `@driver`'s own claim \
                 (03-hardware.md §2), so it is only callable from inside one"
            ),
            call_span,
        ));
    };
    let structs: std::collections::BTreeMap<String, &types::DeclStruct> = mctx
        .structs
        .iter()
        .map(|(n, s)| (n.clone(), &s.decl))
        .collect();
    // The nesting table item I's sweep made this walk need: a layout
    // reached through a wrapper struct *or* an enum variant payload, which
    // is why enums are here beside structs (`types::components_by_name`'s
    // own content, built from this pass's own already-declared tables).
    let components: std::collections::BTreeMap<String, &[(Type, Span)]> = mctx
        .structs
        .iter()
        .map(|(n, s)| (n.clone(), s.decl.component_types.as_slice()))
        .chain(
            mctx.enums
                .iter()
                .map(|(n, e)| (n.clone(), e.component_types.as_slice())),
        )
        .collect();
    let Some(mints) = types::mmio_mints_of(&owner, &structs, &components) else {
        return Err(type_error(
            format!(
                "`map_partition({layout_name})` partitions a `@driver`'s own claim, and \
                 `{owner}` is not a `@driver` (03-hardware.md §2)"
            ),
            call_span,
        ));
    };
    if !mints.iter().any(|m| m == layout_name) {
        return Err(type_error(
            format!(
                "`@driver` `{owner}` maps `{layout_name}`, but declares no field of type \
                 `Mmio[{layout_name}]`. A driver's declared `Mmio[L]` fields *are* its partition \
                 of the claim (03-hardware.md §2), and they are what the no-alias rule and the \
                 device's own register window are both derived from — a partition mapped outside \
                 that set would be a live layout no rule ever saw{}",
                if mints.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; `{owner}` declares {}",
                        mints
                            .iter()
                            .map(|m| format!("`Mmio[{m}]`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            ),
            call_span,
        ));
    }
    let _ = fspan;
    Ok(TypedExpr {
        ty: Type::Named(
            "Mmio".to_string(),
            vec![types::TypeArg::Type(Type::Named(
                layout_name.clone(),
                vec![],
            ))],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "Device.map_partition".to_string(),
            receiver: Some(Box::new(state_expr)),
            type_arg: Some(Type::Named(layout_name.clone(), vec![])),
            args: Vec::new(),
        },
    })
}

/// Is `key` one of item H1's sealed-transport intrinsics, including item
/// G's `take_irq`? Same three-consumer discipline as
/// `is_mmio_access_intrinsic` above.
pub fn is_device_transport_intrinsic(key: &str) -> bool {
    matches!(
        key,
        "Device.claim"
            | "Device.map_partition"
            | "Device.negotiate"
            | "Device.start"
            | "Device.reset"
            | "Device.read_capacity_sectors"
            | "Device.take_irq"
            | "VirtQueue.configure"
    )
}

/// plans/M7.md item E2/E3/E4 / G fail-closed keys — used by lower and
/// flowwir so an unimplemented queue/IRQ op names its owner rather than
/// falling into a generic "intrinsic" rejection.
pub fn is_queue_op_deferred(key: &str) -> Option<&'static str> {
    match key {
        "VirtQueue.poll_sources" | "VirtQueue.completions_pending" => {
            Some("plans/M7.md item G (`poll_sources` / `completions_pending`)")
        }
        _ => None,
    }
}

/// Is `key` one of item E2/E3/E4's live queue operations?
pub fn is_queue_op_intrinsic(key: &str) -> bool {
    matches!(
        key,
        "VirtQueue.reserve_proven"
            | "VirtQueue.prepare_block"
            | "VirtQueue.publish"
            | "VirtQueue.reject"
            | "VirtQueue.drain"
            | "VirtQueue.suppress_interrupts"
            | "VirtQueue.claim"
            | "VirtQueue.recover"
            | "VirtQueue.reclaim"
    )
}

/// A method call on a `VirtQueue[..N]` value (03-hardware.md §4).
fn check_virtqueue_method(
    queue: TypedExpr,
    name: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    match name {
        "reserve_proven" => {
            check_virtqueue_reserve_proven(queue, args, fspan, call_span, fctx, mctx)
        }
        "prepare_block" => check_virtqueue_prepare_block(queue, args, fspan, call_span, fctx, mctx),
        "publish" => check_virtqueue_publish(queue, args, fspan, call_span, fctx, mctx),
        "reject" => check_virtqueue_reject(queue, args, fspan, call_span, fctx, mctx),
        "drain" => check_virtqueue_drain(queue, args, fspan, call_span, fctx, mctx),
        "reset" => Err(type_error(
            "`VirtQueue.reset` is per-queue reset, which requires the `RingReset` feature \
             this device model does not offer (03-hardware.md §9: \"per-queue reset (when \
             negotiated)\"; plans/M7.md item H2b / decision 23: machine v1 does full \
             `RunningDevice.reset(queue=mut ...)` only — see `golden/err-device-required-unoffered`)"
                .to_string(),
            fspan,
        )),
        "suppress_interrupts" => {
            check_virtqueue_suppress_interrupts(queue, args, fspan, call_span, fctx, mctx)
        }
        "claim" => check_virtqueue_claim(queue, args, fspan, call_span, fctx, mctx),
        "recover" => check_virtqueue_recover(queue, args, fspan, call_span, fctx, mctx),
        "reclaim" => check_virtqueue_reclaim(queue, args, fspan, call_span, fctx, mctx),
        "poll_sources" | "completions_pending" => Err(unimplemented_at(
            &format!("`VirtQueue.{name}(...)` — plans/M7.md item G (`{name}`) is"),
            call_span,
        )),
        other => Err(type_error(
            format!(
                "`VirtQueue[..N]` has no method `{other}`; 03-hardware.md §4/§5/§9 give \
                 `reserve_proven`, `prepare_block`, `publish`, `reject`, `drain`, \
                 `suppress_interrupts`, `claim`, `recover`, and `reclaim`"
            ),
            fspan,
        )),
    }
}

/// `queue.reserve_proven(descriptors=3)` — yields a `QueuePermit` when
/// the whole-image proof (`sema::reserve_proof`) admits the site.
fn check_virtqueue_reserve_proven(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let depth = virtqueue_type_depth(&queue.ty, mctx).ok_or_else(|| {
        type_error(
            "`reserve_proven` needs a `VirtQueue[..N]` whose depth is a comptime-known \
             nonzero power of two (03-hardware.md §4)"
                .to_string(),
            call_span,
        )
    })?;
    if depth == 0 || !depth.is_power_of_two() {
        return Err(type_error(
            format!(
                "`reserve_proven` on `VirtQueue[..{depth}]`: depth must be a nonzero power of two"
            ),
            call_span,
        ));
    }
    if args.len() != 1 {
        return Err(type_error(
            format!(
                "`VirtQueue.reserve_proven(descriptors=N)` takes exactly one labelled argument; \
                 found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let arg = &args[0];
    if arg.label.as_deref() != Some("descriptors") {
        return Err(type_error(
            "`VirtQueue.reserve_proven`'s own argument is labelled `descriptors=` \
             (03-hardware.md §4)"
                .to_string(),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`reserve_proven`'s `descriptors=` is a count, not a moved value: drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let desc_expr = check_expr(&arg.value, Some(&Type::Usize), fctx, mctx)?;
    let desc_val = virtqueue_depth_value(&desc_expr, mctx).ok_or_else(|| {
        type_error(
            "`reserve_proven`'s `descriptors=` must be a comptime-known integer \
             (03-hardware.md §4)"
                .to_string(),
            arg.span,
        )
    })?;
    if desc_val == 0 || desc_val > u64::from(u16::MAX) {
        return Err(type_error(
            format!("`reserve_proven(descriptors={desc_val})` is not a usable descriptor count"),
            arg.span,
        ));
    }
    if desc_val != u64::from(crate::virtqueue::DESCRIPTORS_PER_BLK_OP) {
        return Err(type_error(
            format!(
                "`reserve_proven(descriptors={desc_val})`: machine v1's virtio-blk operation \
                 uses exactly {} descriptors (header + data + status)",
                crate::virtqueue::DESCRIPTORS_PER_BLK_OP
            ),
            arg.span,
        ));
    }
    let _ = fspan;
    // Encode the resolved depth as a literal Bound on `type_arg` so
    // `sema::reserve_proof` never has to re-resolve a const name.
    Ok(TypedExpr {
        ty: Type::Named("QueuePermit".to_string(), vec![]),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.reserve_proven".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: Some(Type::Named(
                "VirtQueue".to_string(),
                vec![types::TypeArg::Bound(Expr::Int(
                    call_span,
                    depth.to_string(),
                ))],
            )),
            args: vec![("descriptors".to_string(), desc_expr)],
        },
    })
}

/// plans/M8.md item G, decision 18: the one wording for 03-hardware.md §9's
/// no-auto-retry rule, shared by the two sites that can commit the
/// violation (`prepare_block` builds the operation; `publish` issues it).
/// One message, two sites — a hoisted `prepare_block` and an inlined one
/// are the same mistake and read the same way.
fn no_auto_retry_message(site: &str) -> String {
    format!(
        "`{site}` re-issues an operation declared `idempotent=false` inside a \
         `CompletionOutcome.Unknown` arm — 03-hardware.md §9: \"Source must not auto-retry a \
         non-idempotent operation on `Unknown`\". The first attempt may already have taken \
         effect, so retrying it can apply the operation twice. Either establish quiescence \
         first (quarantine the device and pool, or go target-fatal — 03 §9), or, if re-running \
         this exact operation is provably harmless, declare it `idempotent=true` at its \
         `prepare_block`"
    )
}

/// `queue.prepare_block(permit=take ..., header=..., payload=take ...,
/// device_writes_payload=..., status=..., idempotent=...)` — yields a
/// `QueueOp[P, <idempotent>]`.
fn check_virtqueue_prepare_block(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if args.len() != 6 {
        return Err(type_error(
            format!(
                "`VirtQueue.prepare_block(permit=take ..., header=..., payload=take ..., \
                 device_writes_payload=..., status=..., idempotent=...)` takes exactly six \
                 labelled arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let mut permit = None;
    let mut header = None;
    let mut payload = None;
    let mut device_writes = None;
    let mut status = None;
    let mut idempotent: Option<bool> = None;
    for arg in args {
        match arg.label.as_deref() {
            Some("permit") => {
                if permit.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `permit=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Take {
                    return Err(type_error(
                        "`prepare_block` consumes the permit: write `permit=take ...` \
                         (03-hardware.md §4)"
                            .to_string(),
                        arg.span,
                    ));
                }
                let expected = Type::Named("QueuePermit".to_string(), vec![]);
                permit = Some(check_expr(&arg.value, Some(&expected), fctx, mctx)?);
            }
            Some("header") => {
                if header.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `header=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`prepare_block`'s `header=` is a `@layout(dma)` value, not a moved \
                             handle: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                let h = check_expr(&arg.value, None, fctx, mctx)?;
                require_layout_dma(&h.ty, "header", arg.span, mctx)?;
                header = Some(h);
            }
            Some("payload") => {
                if payload.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `payload=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Take {
                    return Err(type_error(
                        "`prepare_block` consumes the transfer payload: write `payload=take ...` \
                         (03-hardware.md §3/§4)"
                            .to_string(),
                        arg.span,
                    ));
                }
                let p = check_expr(&arg.value, None, fctx, mctx)?;
                match &p.ty {
                    Type::Own(_, inner) => {
                        require_layout_dma(inner, "payload", arg.span, mctx)?;
                    }
                    other => {
                        return Err(type_error(
                            format!(
                                "`prepare_block`'s `payload=` is an `own[P] T` transfer handle \
                                 (03-hardware.md §3); found `{}`",
                                types::render_type(other)
                            ),
                            arg.span,
                        ));
                    }
                }
                payload = Some(p);
            }
            Some("device_writes_payload") => {
                if device_writes.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `device_writes_payload=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`prepare_block`'s `device_writes_payload=` is a bool, not a moved \
                             value: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                device_writes = Some(check_expr(&arg.value, Some(&Type::Bool), fctx, mctx)?);
            }
            Some("status") => {
                if status.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `status=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`prepare_block`'s `status=` is a `@layout(dma)` value, not a moved \
                             handle: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                let s = check_expr(&arg.value, None, fctx, mctx)?;
                require_layout_dma(&s.ty, "status", arg.span, mctx)?;
                status = Some(s);
            }
            // plans/M8.md item G, decision 18: 03-hardware.md §9's
            // no-auto-retry rule needs to know whether re-running this
            // operation is harmless, and **nothing in the compiler can
            // work that out** — a write of fixed bytes to a fixed sector
            // is idempotent, an append is not, and both spell the same
            // `prepare_block`. So the author declares it, here, at the one
            // place the operation is constructed. Required, not defaulted:
            // a default in either direction is the compiler guessing.
            Some("idempotent") => {
                if idempotent.is_some() {
                    return Err(type_error(
                        "`prepare_block`'s `idempotent=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`prepare_block`'s `idempotent=` is a declaration, not a moved \
                             value: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                let Expr::Bool(_, v) = &arg.value else {
                    return Err(type_error(
                        "`prepare_block`'s `idempotent=` is a declaration the operation's type \
                         carries, so it must be the literal `true` or `false` \
                         (03-hardware.md §9)"
                            .to_string(),
                        arg.span,
                    ));
                };
                idempotent = Some(*v);
            }
            Some(other) => {
                return Err(type_error(
                    format!(
                        "`prepare_block`'s own arguments are labelled `permit=`, `header=`, \
                         `payload=`, `device_writes_payload=`, `status=`, `idempotent=`; \
                         `{other}=` names no parameter"
                    ),
                    arg.span,
                ));
            }
            None => {
                return Err(type_error(
                    "`prepare_block(...)` requires labelled arguments".to_string(),
                    arg.span,
                ));
            }
        }
    }
    let (
        Some(permit),
        Some(header),
        Some(payload),
        Some(device_writes),
        Some(status),
        Some(idempotent),
    ) = (permit, header, payload, device_writes, status, idempotent)
    else {
        return Err(type_error(
            "`prepare_block` needs `permit=`, `header=`, `payload=`, `device_writes_payload=`, \
             `status=` and `idempotent=`"
                .to_string(),
            call_span,
        ));
    };
    if !idempotent && fctx.in_unknown_outcome_arm() {
        return Err(type_error(
            no_auto_retry_message("prepare_block"),
            call_span,
        ));
    }
    let payload_ty = payload.ty.clone();
    let _ = (fspan, &queue);
    Ok(TypedExpr {
        ty: Type::Named(
            "QueueOp".to_string(),
            vec![
                types::TypeArg::Type(payload_ty),
                // The declaration rides on the operation's *type*, so a
                // `publish` that never sees the `prepare_block` site (one
                // hoisted out of the arm, say) still knows the answer.
                // `Span::default()` keeps two identically-declared
                // operations structurally equal.
                types::TypeArg::Const(Expr::Bool(Span::default(), idempotent)),
            ],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.prepare_block".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            args: vec![
                ("permit".to_string(), permit),
                ("header".to_string(), header),
                ("payload".to_string(), payload),
                ("device_writes_payload".to_string(), device_writes),
                ("status".to_string(), status),
            ],
        },
    })
}

fn require_layout_dma(
    ty: &Type,
    role: &str,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let Type::Named(name, targs) = ty else {
        return Err(type_error(
            format!(
                "`prepare_block`'s `{role}=` must be a `@layout(dma)` struct; found `{}`",
                types::render_type(ty)
            ),
            span,
        ));
    };
    if !targs.is_empty() {
        return Err(type_error(
            format!(
                "`prepare_block`'s `{role}=` must be a `@layout(dma)` struct; found `{}`",
                types::render_type(ty)
            ),
            span,
        ));
    }
    match mctx.layouts.get(name.as_str()) {
        Some(l) if l.kind == types::LayoutKind::Dma => Ok(()),
        _ => Err(type_error(
            format!("`prepare_block`'s `{role}=` must be a `@layout(dma)` struct; `{name}` is not"),
            span,
        )),
    }
}

/// `queue.publish(operation=take ...)` — 03-hardware.md §5 / decision 15:
/// writes the ring in normative order and yields `Receipt[P]` for the
/// packaged payload brand.
fn check_virtqueue_publish(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if args.len() != 1 {
        return Err(type_error(
            format!(
                "`VirtQueue.publish(operation=take ...)` takes exactly one labelled argument; \
                 found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let arg = &args[0];
    if arg.label.as_deref() != Some("operation") {
        return Err(type_error(
            "`VirtQueue.publish`'s own argument is labelled `operation=` (03-hardware.md §4/§5)"
                .to_string(),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Take {
        return Err(type_error(
            "`publish` consumes the prepared operation: write `operation=take ...` \
             (03-hardware.md §4/§5)"
                .to_string(),
            arg.span,
        ));
    }
    let op = check_expr(&arg.value, None, fctx, mctx)?;
    // plans/M8.md item G, decision 18: the operation's own type carries the
    // author's idempotence declaration, so this catches a `prepare_block`
    // hoisted out of the arm just as surely as one written inside it.
    if let Type::Named(n, targs) = &op.ty {
        if n == "QueueOp"
            && matches!(
                targs.get(1),
                Some(types::TypeArg::Const(Expr::Bool(_, false)))
            )
            && fctx.in_unknown_outcome_arm()
        {
            return Err(type_error(no_auto_retry_message("publish"), call_span));
        }
    }
    let payload_ty = match &op.ty {
        Type::Named(n, targs) if n == "QueueOp" => match targs.first() {
            Some(types::TypeArg::Type(p)) => p.clone(),
            _ => {
                return Err(type_error(
                    "`publish`'s `operation=` is a `QueueOp[P]`; found a `QueueOp` with no \
                     payload brand"
                        .to_string(),
                    arg.span,
                ));
            }
        },
        other => {
            return Err(type_error(
                format!(
                    "`publish`'s `operation=` is a `QueueOp[P]` (03-hardware.md §4); found `{}`",
                    types::render_type(other)
                ),
                arg.span,
            ));
        }
    };
    let _ = (fspan, &queue);
    Ok(TypedExpr {
        ty: Type::Named(
            "Receipt".to_string(),
            vec![types::TypeArg::Type(payload_ty)],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.publish".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            args: vec![("operation".to_string(), op)],
        },
    })
}

/// `queue.reject(payload=take p, error=...)` — 03-hardware.md §5:
/// pre-commit failure returns `P` via a resolved receipt.
fn check_virtqueue_reject(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if args.len() != 2 {
        return Err(type_error(
            format!(
                "`VirtQueue.reject(payload=take ..., error=...)` takes exactly two labelled \
                 arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let mut payload = None;
    let mut error = None;
    for arg in args {
        match arg.label.as_deref() {
            Some("payload") => {
                if payload.is_some() {
                    return Err(type_error(
                        "`reject`'s `payload=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Take {
                    return Err(type_error(
                        "`reject` returns the payload through the receipt: write \
                         `payload=take ...` (03-hardware.md §5)"
                            .to_string(),
                        arg.span,
                    ));
                }
                payload = Some(check_expr(&arg.value, None, fctx, mctx)?);
            }
            Some("error") => {
                if error.is_some() {
                    return Err(type_error(
                        "`reject`'s `error=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`reject`'s `error=` is an `IoError` value, not a moved handle: \
                             drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                let expected = Type::Named("IoError".to_string(), vec![]);
                error = Some(check_expr(&arg.value, Some(&expected), fctx, mctx)?);
            }
            Some(other) => {
                return Err(type_error(
                    format!(
                        "`reject`'s own arguments are labelled `payload=` and `error=`; \
                         `{other}=` names no parameter"
                    ),
                    arg.span,
                ));
            }
            None => {
                return Err(type_error(
                    "`reject(...)` requires labelled arguments".to_string(),
                    arg.span,
                ));
            }
        }
    }
    let (Some(payload), Some(error)) = (payload, error) else {
        return Err(type_error(
            "`reject` needs `payload=` and `error=`".to_string(),
            call_span,
        ));
    };
    let _ = (fspan, &queue);
    Ok(TypedExpr {
        ty: Type::Named(
            "Receipt".to_string(),
            vec![types::TypeArg::Type(payload.ty.clone())],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.reject".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            args: vec![
                ("payload".to_string(), payload),
                ("error".to_string(), error),
            ],
        },
    })
}

/// `queue.drain(max=N)` — bounded used-ring walk (03-hardware.md §4/§6).
fn check_virtqueue_drain(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let _ = fspan;
    let depth = virtqueue_type_depth(&queue.ty, mctx).ok_or_else(|| {
        type_error(
            "`drain` needs a `VirtQueue[..N]` whose depth is a comptime-known nonzero power of two"
                .to_string(),
            call_span,
        )
    })?;
    if args.len() != 1 {
        return Err(type_error(
            format!(
                "`VirtQueue.drain(max=N)` takes exactly one labelled argument; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let arg = &args[0];
    if arg.label.as_deref() != Some("max") {
        return Err(type_error(
            "`VirtQueue.drain`'s own argument is labelled `max=` (03-hardware.md §6)".to_string(),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`drain`'s `max=` is a bound, not a moved value: drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let max_expr = check_expr(&arg.value, Some(&Type::Usize), fctx, mctx)?;
    let max_val = virtqueue_depth_value(&max_expr, mctx).ok_or_else(|| {
        type_error(
            "`drain`'s `max=` must be a comptime-known integer (03-hardware.md §6)".to_string(),
            arg.span,
        )
    })?;
    if max_val == 0 || max_val > depth {
        return Err(type_error(
            format!("`drain(max={max_val})` on `VirtQueue[..{depth}]`: max must be in 1..={depth}"),
            arg.span,
        ));
    }
    Ok(TypedExpr {
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.drain".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: Some(Type::Named(
                "VirtQueue".to_string(),
                vec![types::TypeArg::Bound(Expr::Int(
                    call_span,
                    max_val.to_string(),
                ))],
            )),
            args: vec![("max".to_string(), max_expr)],
        },
    })
}

/// `queue.claim(receipt=take r) -> IoCompletion[P]` — plans/M7.md item E4 /
/// decision 22: sync claim of a drain-resolved receipt (bottom-half dual
/// of `await receipt`).
fn check_virtqueue_claim(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let _ = fspan;
    if args.len() != 1 {
        return Err(type_error(
            format!(
                "`VirtQueue.claim(receipt=take ...)` takes exactly one labelled argument; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let arg = &args[0];
    if arg.label.as_deref() != Some("receipt") {
        return Err(type_error(
            "`VirtQueue.claim`'s own argument is labelled `receipt=` (plans/M7.md item E4)"
                .to_string(),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Take {
        return Err(type_error(
            "`claim` consumes the receipt: write `receipt=take ...` (03-hardware.md §5)"
                .to_string(),
            arg.span,
        ));
    }
    let receipt = check_expr(&arg.value, None, fctx, mctx)?;
    let Type::Named(n, targs) = &receipt.ty else {
        return Err(type_error(
            format!(
                "`claim`'s `receipt=` must be a `Receipt[P]`; found `{}`",
                types::render_type(&receipt.ty)
            ),
            arg.span,
        ));
    };
    if n != "Receipt" {
        return Err(type_error(
            format!(
                "`claim`'s `receipt=` must be a `Receipt[P]`; found `{}`",
                types::render_type(&receipt.ty)
            ),
            arg.span,
        ));
    }
    let Some(types::TypeArg::Type(payload)) = targs.first() else {
        return Err(type_error(
            "`Receipt` with no payload type argument".to_string(),
            arg.span,
        ));
    };
    let payload = payload.clone();
    Ok(TypedExpr {
        ty: Type::Named(
            "IoCompletion".to_string(),
            vec![types::TypeArg::Type(payload)],
        ),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.claim".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            args: vec![("receipt".to_string(), receipt)],
        },
    })
}

/// `queue.recover(receipt=take r) -> CompletionOutcome` — plans/M8.md item
/// G / decision 12: 03-hardware.md §5's `Recovery` transition, and the one
/// producer of §9's `CompletionOutcome`.
///
/// **Why this is not a second `claim`.** `claim` is the *resolved* path: it
/// consumes the receipt and returns the payload with the completion, which
/// is only sound because the device provably returned the descriptor in the
/// current epoch. `recover` is the *abandon* path §9 describes ("cancelling
/// in-flight work is a driver protocol, not a dropped future"): it consumes
/// the receipt — receipts resolve exactly once and dropping one is illegal
/// in every state (§5) — reports what is known about the operation's effect,
/// and deliberately returns **no payload**, because after a reset the buffer
/// is possibly device-owned and §9 forbids reclaiming it. Reclaim is
/// quarantine's job (plans/M8.md item F); until it lands the pool slot is
/// simply retired, which is the fail-closed half of the same sentence.
fn check_virtqueue_recover(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let _ = fspan;
    if args.len() != 1 {
        return Err(type_error(
            format!(
                "`VirtQueue.recover(receipt=take ...)` takes exactly one labelled argument; \
                 found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let arg = &args[0];
    if arg.label.as_deref() != Some("receipt") {
        return Err(type_error(
            "`VirtQueue.recover`'s own argument is labelled `receipt=` (03-hardware.md §5)"
                .to_string(),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Take {
        return Err(type_error(
            "`recover` consumes the receipt: write `receipt=take ...` (03-hardware.md §5: \
             a receipt resolves exactly once)"
                .to_string(),
            arg.span,
        ));
    }
    let receipt = check_expr(&arg.value, None, fctx, mctx)?;
    match &receipt.ty {
        Type::Named(n, _) if n == "Receipt" => {}
        other => {
            return Err(type_error(
                format!(
                    "`recover`'s `receipt=` must be a `Receipt[P]`; found `{}`",
                    types::render_type(other)
                ),
                arg.span,
            ));
        }
    }
    // plans/M8.md item H attack 1: remember the receipt's `own[P] T` brand
    // on this queue place so a later `reclaim` cannot declare a different
    // pool and mint a confused handle.
    if let Some(key) = virtqueue_place_key(&queue) {
        if let Some(brand) = receipt_own_brand(&receipt.ty) {
            fctx.quarantined_by_queue.insert(key, brand);
        }
    }
    Ok(TypedExpr {
        ty: Type::Named("CompletionOutcome".to_string(), vec![]),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.recover".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            args: vec![("receipt".to_string(), receipt)],
        },
    })
}

/// `queue.reclaim(pool=P, payload=T) -> own[P] T` — plans/M8.md item F /
/// **decision 37**: 03-hardware.md §9's "affected regions and DMA slots
/// are quarantined, per-queue reset ... or full reset establishes
/// quiescence, and **only then is memory reclaimed**".
///
/// **Why two declaring arguments and no receipt.** `recover` already
/// consumed the receipt (§5: a receipt resolves exactly once, and dropping
/// one is illegal in every state), and with it the only value that carried
/// the payload's brand — so the handle's type has to be *declared* here,
/// exactly as `img.dma_pool[T](name=P, ...)` declares the same pair when
/// the pool is created. Both arguments are bare names with no value form:
/// `pool=` is a bound pool name (02-language.md §4) and `payload=` names
/// the `@layout(dma)` struct the slot holds. They are resolved through the
/// ordinary `own[P] T` resolver, so an undeclared pool and a non-`dma`
/// payload are the same two diagnostics they are in any annotation.
///
/// **What the declaration cannot lie about.** The address handed back is
/// the quarantined slot's own payload word, so the *bytes* are always the
/// abandoned buffer's; the declaration decides which pool the language
/// believes the handle belongs to. plans/M8.md item H attack 1 closes the
/// pool-brand half at build time: `pool=`/`payload=` must match the
/// `own[P] T` brand of the `recover` that quarantined this queue's slot in
/// the same function (same `match` arm). A wrong brand would otherwise
/// survive any path that never reaches a later `publish`/`Receipt` store.
/// Checking the handle against the queue's *device* stays the deliberate
/// trade item P recorded as decision 27 — that is a different sentence.
fn check_virtqueue_reclaim(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let _ = fspan;
    let (pool, payload) = reclaim_declaration(args, call_span)?;
    // Shape first: undeclared pool / non-dma payload keep the diagnostics
    // they have in any `own[P] T` annotation (`golden/err-reclaim-payload-not-dma`).
    let ast_ty = ast::Type::Own(Box::new(ast::OwnType {
        span: call_span,
        pool: vec![pool.1.clone()],
        inner: ast::Type::Named(NamedType {
            span: payload.0,
            name: payload.1.clone(),
            args: vec![],
        }),
    }));
    let ty = mctx.resolve_type(&ast_ty, &fctx.local_pools)?;
    match mctx.layouts.get(payload.1.as_str()) {
        Some(l) if l.kind == types::LayoutKind::Dma => {}
        _ => {
            return Err(type_error(
                format!(
                    "`reclaim`'s `payload=` must be a `@layout(dma)` struct; `{}` is not \
                     (03-hardware.md §3: a transfer payload is `own[P] T` where `T` is \
                     `@layout(dma)`)",
                    payload.1
                ),
                payload.0,
            ));
        }
    }
    // Brand second (plans/M8.md item H attack 1): the declaration must
    // match the `own[P] T` `recover` quarantined on this queue place.
    let Some(key) = virtqueue_place_key(&queue) else {
        return Err(type_error(
            "`reclaim` needs a named `VirtQueue` place (a local or a field) so its \
             `pool=` can be checked against the brand `recover` quarantined on that \
             queue (plans/M8.md item H; 04-compiler.md §1: DMA ownership transitions \
             are valid)"
                .to_string(),
            call_span,
        ));
    };
    let Some((expected_pool, expected_payload)) = fctx.quarantined_by_queue.remove(&key) else {
        return Err(type_error(
            "`reclaim` on this queue has no preceding `recover` in this scope whose \
             receipt brands a pool; write `recover` first, then \
             `reclaim(pool=<that brand>, payload=...)` (plans/M8.md item H / \
             03-hardware.md §9)"
                .to_string(),
            call_span,
        ));
    };
    if pool.1 != expected_pool {
        return Err(type_error(
            format!(
                "`reclaim`'s `pool={}` does not match the pool brand recovered on this \
                 queue (`{expected_pool}`); the handle would be `own[{}]` pointing at \
                 `{expected_pool}`'s bytes (03-hardware.md §9 / 04-compiler.md §1: DMA \
                 ownership transitions are valid)",
                pool.1, pool.1
            ),
            pool.0,
        ));
    }
    if payload.1 != expected_payload {
        return Err(type_error(
            format!(
                "`reclaim`'s `payload={}` does not match the payload type recovered on \
                 this queue (`{expected_payload}`) (03-hardware.md §9)",
                payload.1
            ),
            payload.0,
        ));
    }
    Ok(TypedExpr {
        ty,
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.reclaim".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            args: Vec::new(),
        },
    })
}

/// Place key for a `VirtQueue` receiver — a local name, or `root.field`
/// for a field of a local (the `self.queue` spelling every flagship uses).
fn virtqueue_place_key(queue: &TypedExpr) -> Option<String> {
    match &queue.kind {
        TypedExprKind::Local(n) => Some(n.clone()),
        TypedExprKind::Field(base, field) => match &base.kind {
            TypedExprKind::Local(root) => Some(format!("{root}.{field}")),
            _ => virtqueue_place_key(base).map(|p| format!("{p}.{field}")),
        },
        _ => None,
    }
}

/// `Receipt[own[P] T]` → `(P, T)` — the brand `recover` quarantines and
/// `reclaim` must re-declare. Anything else yields `None` (a receipt that
/// does not carry an `own` payload cannot justify a reclaim brand).
fn receipt_own_brand(ty: &Type) -> Option<(String, String)> {
    let Type::Named(n, args) = ty else {
        return None;
    };
    if n != "Receipt" {
        return None;
    }
    let Some(types::TypeArg::Type(Type::Own(pool, inner))) = args.first() else {
        return None;
    };
    match inner.as_ref() {
        Type::Named(payload, _) => Some((pool.clone(), payload.clone())),
        _ => None,
    }
}

/// The `pool=P, payload=T` pair `reclaim` declares, as two bare names.
/// Shared by `bodies` (which resolves them) and `access` (which only needs
/// the shape to keep a move tracked), so the two passes cannot disagree
/// about what a well-formed `reclaim` looks like.
pub(crate) fn reclaim_declaration(
    args: &[Arg],
    call_span: Span,
) -> Result<((Span, String), (Span, String)), SemaError> {
    let mut pool: Option<(Span, String)> = None;
    let mut payload: Option<(Span, String)> = None;
    for a in args {
        let slot = match a.label.as_deref() {
            Some("pool") => &mut pool,
            Some("payload") => &mut payload,
            _ => {
                return Err(type_error(
                    "`VirtQueue.reclaim(pool=..., payload=...)` takes exactly those two \
                     labelled arguments (03-hardware.md §9)"
                        .to_string(),
                    a.span,
                ));
            }
        };
        if slot.is_some() {
            return Err(type_error(
                format!(
                    "duplicate `{}=` argument",
                    a.label.as_deref().unwrap_or("?")
                ),
                a.span,
            ));
        }
        if a.mode != AccessMode::Read {
            return Err(type_error(
                "`reclaim`'s `pool=`/`payload=` are declarations, not values: they take no \
                 access mode"
                    .to_string(),
                a.span,
            ));
        }
        match &a.value {
            Expr::Name(span, name) => *slot = Some((*span, name.clone())),
            other => {
                return Err(type_error(
                    "`reclaim`'s `pool=`/`payload=` are bare names — a declared pool and a \
                     `@layout(dma)` struct"
                        .to_string(),
                    other.span(),
                ));
            }
        }
    }
    match (pool, payload) {
        (Some(p), Some(t)) => Ok((p, t)),
        _ => Err(type_error(
            "`VirtQueue.reclaim(pool=..., payload=...)` needs both: the pool the quarantined \
             slot belongs to and the `@layout(dma)` payload it holds (03-hardware.md §9)"
                .to_string(),
            call_span,
        )),
    }
}

/// `queue.suppress_interrupts()` — set `VIRTQ_AVAIL_F_NO_INTERRUPT` (poll builds).
fn check_virtqueue_suppress_interrupts(
    queue: TypedExpr,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    _fctx: &mut FnCtx,
    _mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let _ = fspan;
    if !args.is_empty() {
        return Err(type_error(
            format!(
                "`VirtQueue.suppress_interrupts()` takes no arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    Ok(TypedExpr {
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.suppress_interrupts".to_string(),
            receiver: Some(Box::new(queue)),
            type_arg: None,
            args: Vec::new(),
        },
    })
}

/// Depth bound on a `VirtQueue[..N]` type, resolving a const name through
/// `mctx.const_values` the same way `virtqueue_depth_value` does for a
/// typed expression.
fn virtqueue_type_depth(ty: &Type, mctx: &ModuleCtx) -> Option<u64> {
    let Type::Named(name, targs) = ty else {
        return None;
    };
    if name != "VirtQueue" {
        return None;
    }
    let types::TypeArg::Bound(expr) = targs.first()? else {
        return None;
    };
    match expr {
        Expr::Int(_, text) => parse_int_literal(text).and_then(|v| u64::try_from(v).ok()),
        Expr::Name(_, n) => {
            let init = mctx.const_values.get(n)?;
            match init {
                Expr::Int(_, text) => parse_int_literal(text).and_then(|v| u64::try_from(v).ok()),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `VirtQueue.configure(pool=take control_pool, device=mut negotiated,
/// index=0, depth=QDEPTH)?` — FeaturesAccepted -> QueuesConfigured, and
/// the `DmaShared` mint item D left named (03-hardware.md §3/§4).
fn check_virtqueue_configure(
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if args.len() != 4 {
        return Err(type_error(
            format!(
                "`VirtQueue.configure(pool=take ..., device=mut ..., index=..., depth=...)` \
                 takes exactly four labelled arguments; found {}",
                args.len()
            ),
            call_span,
        ));
    }
    let mut pool = None;
    let mut device = None;
    let mut device_local: Option<String> = None;
    let mut index = None;
    let mut depth = None;
    for arg in args {
        match arg.label.as_deref() {
            Some("pool") => {
                if pool.is_some() {
                    return Err(type_error(
                        "`VirtQueue.configure`'s `pool=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Take {
                    return Err(type_error(
                        "`VirtQueue.configure` consumes the DMA pool: write `pool=take ...` \
                         (03-hardware.md §3: the queue owns the shared control memory minted \
                         out of it)"
                            .to_string(),
                        arg.span,
                    ));
                }
                pool = Some(check_expr(&arg.value, None, fctx, mctx)?);
            }
            Some("device") => {
                if device.is_some() {
                    return Err(type_error(
                        "`VirtQueue.configure`'s `device=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Mut {
                    return Err(type_error(
                        "`VirtQueue.configure` takes the device by `mut` so the local becomes \
                         `QueuesConfiguredDevice[D]` after the call (03-hardware.md §9): write \
                         `device=mut ...`"
                            .to_string(),
                        arg.span,
                    ));
                }
                if let Expr::Name(_, n) = &arg.value {
                    device_local = Some(n.clone());
                }
                device = Some(check_expr(&arg.value, None, fctx, mctx)?);
            }
            Some("index") => {
                if index.is_some() {
                    return Err(type_error(
                        "`VirtQueue.configure`'s `index=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`VirtQueue.configure`'s `index=` is a queue index, not a moved \
                             value: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                index = Some(check_expr(&arg.value, Some(&Type::Usize), fctx, mctx)?);
            }
            Some("depth") => {
                if depth.is_some() {
                    return Err(type_error(
                        "`VirtQueue.configure`'s `depth=` appears twice".to_string(),
                        arg.span,
                    ));
                }
                if arg.mode != AccessMode::Read {
                    return Err(type_error(
                        format!(
                            "`VirtQueue.configure`'s `depth=` is a queue depth, not a moved \
                             value: drop the `{}`",
                            arg.mode.as_str()
                        ),
                        arg.span,
                    ));
                }
                depth = Some(check_expr(&arg.value, Some(&Type::Usize), fctx, mctx)?);
            }
            Some(other) => {
                return Err(type_error(
                    format!(
                        "`VirtQueue.configure`'s own arguments are labelled `pool=`, `device=`, \
                         `index=`, `depth=`; `{other}=` names no parameter"
                    ),
                    arg.span,
                ));
            }
            None => {
                return Err(type_error(
                    "`VirtQueue.configure(...)` requires labelled arguments \
                     (docs/language/examples/virtio-storage.wr)"
                        .to_string(),
                    arg.span,
                ));
            }
        }
    }
    let (Some(pool), Some(device_expr), Some(index), Some(depth_expr)) =
        (pool, device, index, depth)
    else {
        return Err(type_error(
            "`VirtQueue.configure` needs `pool=`, `device=`, `index=` and `depth=`".to_string(),
            call_span,
        ));
    };
    // Pool must be a DmaPool[P, N].
    let pool_ty = unwrap_own(pool.ty.clone());
    let Type::Named(pool_name, pool_targs) = &pool_ty else {
        return Err(type_error(
            format!(
                "`VirtQueue.configure`'s `pool=` is a `DmaPool[P, N]`; found `{}`",
                types::render_type(&pool.ty)
            ),
            call_span,
        ));
    };
    if pool_name != "DmaPool" {
        return Err(type_error(
            format!(
                "`VirtQueue.configure`'s `pool=` is a `DmaPool[P, N]`; found `{}`",
                types::render_type(&pool.ty)
            ),
            call_span,
        ));
    }
    let Some(types::TypeArg::Pool(pool_id)) = pool_targs.first() else {
        return Err(type_error(
            "`VirtQueue.configure`'s `DmaPool` names no pool".to_string(),
            call_span,
        ));
    };
    // Device must be FeaturesAcceptedDevice[D].
    let device_ty = unwrap_own(device_expr.ty.clone());
    let Type::Named(dev_state, dev_targs) = &device_ty else {
        return Err(type_error(
            format!(
                "`VirtQueue.configure`'s `device=` is a `FeaturesAcceptedDevice[D]`; found `{}`",
                types::render_type(&device_expr.ty)
            ),
            call_span,
        ));
    };
    if dev_state != "FeaturesAcceptedDevice" {
        return Err(type_error(
            format!(
                "`VirtQueue.configure`'s `device=` is a `FeaturesAcceptedDevice[D]` \
                 (03-hardware.md §9: FeaturesAccepted -> QueuesConfigured); found `{}`",
                types::render_type(&device_expr.ty)
            ),
            call_span,
        ));
    }
    let device_name = device_type_arg(dev_targs).unwrap_or("?").to_string();
    // Depth must be a comptime-known nonzero power of two. Prefer a
    // literal; a module const name is accepted when its value is a
    // literal int (the common `const QDEPTH: usize = 128` spelling).
    let depth_val = virtqueue_depth_value(&depth_expr, mctx).ok_or_else(|| {
        type_error(
            "`VirtQueue.configure`'s `depth=` must be a comptime-known nonzero power of two \
             (VIRTIO 1.2 §2.6); a runtime value would make the ring geometry — which the \
             report, the placer and the VMM all read from one derivation — disagree with \
             itself"
                .to_string(),
            call_span,
        )
    })?;
    if depth_val == 0 || !depth_val.is_power_of_two() || depth_val > u16::MAX as u64 {
        return Err(type_error(
            format!(
                "`VirtQueue.configure`'s `depth={depth_val}` is not a nonzero power of two that \
                 fits virtio's 16-bit queue depth (VIRTIO 1.2 §2.6)"
            ),
            call_span,
        ));
    }
    // index must be 0 on machine v1 (one queue).
    if let TypedExprKind::Int(text) = &index.kind {
        if let Some(v) = parse_int_literal(text) {
            if v != 0 {
                return Err(type_error(
                    format!(
                        "`VirtQueue.configure`'s `index={v}`: machine v1's `blk` has exactly one \
                         queue (index 0)"
                    ),
                    call_span,
                ));
            }
        }
    }
    // Flow-type the mut device local to QueuesConfiguredDevice[D].
    if let Some(local) = &device_local {
        let queued = device_state_ty("QueuesConfiguredDevice", &device_name);
        if !fctx.retype_local(local, queued) {
            return Err(type_error(
                "`VirtQueue.configure`'s `device=mut ...` must name a local so its type can \
                 become `QueuesConfiguredDevice[D]` after the call (03-hardware.md §9)"
                    .to_string(),
                call_span,
            ));
        }
    } else {
        return Err(type_error(
            "`VirtQueue.configure`'s `device=mut ...` must name a local so its type can \
             become `QueuesConfiguredDevice[D]` after the call (03-hardware.md §9)"
                .to_string(),
            call_span,
        ));
    }
    let _ = (fspan, pool_id);
    // Record for layout/report: one derivation of (pool, depth).
    mctx.virtqueue_configures
        .borrow_mut()
        .push((pool_id.clone(), depth_val as u16));
    let queue_ty = Type::Named(
        "VirtQueue".to_string(),
        vec![types::TypeArg::Bound(Expr::Int(
            call_span,
            depth_val.to_string(),
        ))],
    );
    Ok(TypedExpr {
        ty: Type::Result(Box::new(queue_ty), Box::new(boot_error_ty())),
        kind: TypedExprKind::Intrinsic {
            key: "VirtQueue.configure".to_string(),
            receiver: None,
            type_arg: Some(Type::Named(device_name, vec![])),
            args: vec![
                ("pool".to_string(), pool),
                ("device".to_string(), device_expr),
                ("index".to_string(), index),
                ("depth".to_string(), depth_expr),
            ],
        },
    })
}

/// A comptime depth for `VirtQueue.configure`: a literal int, or a
/// module `const` whose initializer is a literal int.
fn virtqueue_depth_value(expr: &TypedExpr, mctx: &ModuleCtx) -> Option<u64> {
    match &expr.kind {
        TypedExprKind::Int(text) => parse_int_literal(text).and_then(|v| u64::try_from(v).ok()),
        TypedExprKind::Const(name) => {
            let init = mctx.const_values.get(name)?;
            match init {
                Expr::Int(_, text) => parse_int_literal(text).and_then(|v| u64::try_from(v).ok()),
                _ => None,
            }
        }
        _ => None,
    }
}

/// plans/M7.md item G: `IrqCap.bind` / `IrqCap.unmask` — the two
/// operations 03-hardware.md §6's worked example names on an `IrqCap`.
pub fn is_irq_cap_intrinsic(key: &str) -> bool {
    matches!(key, "IrqCap.bind" | "IrqCap.unmask")
}

/// plans/M7.md item G, decision 17: `InterruptCell[T]` ops + constructor.
pub fn is_interrupt_cell_intrinsic(key: &str) -> bool {
    matches!(
        key,
        "InterruptCell.new"
            | "InterruptCell.load_acquire"
            | "InterruptCell.store_release"
            | "InterruptCell.swap_acquire"
            | "InterruptCell.fetch_or_release"
    )
}

/// plans/M7.md item G: `wake(Driver.method)`.
pub fn is_wake_intrinsic(key: &str) -> bool {
    key == "wake"
}

/// Is `ty` an `InterruptCell[_]`?
pub fn is_interrupt_cell_type(ty: &Type) -> bool {
    matches!(unwrap_own(ty.clone()), Type::Named(n, _) if n == "InterruptCell")
}

// --- plans/M7.md item G: IrqCap.bind / IrqCap.unmask (03-hardware.md §6) ---
//
// "An interrupt handler is a plain `fn` bound to a vector at image/driver
// wiring (`irq.bind(self.on_queue_irq)`). The binding — not a keyword —
// makes the compiler restrict the function's transitive effects to the
// ISR set". The *effect* half is a later commit of this item; this is
// the binding surface itself.
//
// `self.on_queue_irq` is deliberately *not* an ordinary method value —
// `check_field_expr` rejects "cannot reference method without calling
// it" — so `bind`'s argument is intercepted as a Field of `self` (or a
// bare `Type.method` assoc spelling) and recorded as a `FnRef`. The
// sealed-graph pass then sees every bind site as an `IrqCap.bind`
// intrinsic whose handler arg is that `FnRef`.

/// One `irq.bind(handler)` / `irq.unmask()` (03-hardware.md §6).
fn check_irq_cap_call(
    irq: TypedExpr,
    method: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let rendered = types::render_type(&irq.ty);
    match method {
        "bind" => check_irq_bind(irq, &rendered, args, fspan, call_span, fctx, mctx),
        "unmask" => {
            if !args.is_empty() {
                return Err(type_error(
                    format!(
                        "`{rendered}.unmask()` takes no arguments; found {}",
                        args.len()
                    ),
                    call_span,
                ));
            }
            let _ = fspan;
            Ok(TypedExpr {
                ty: Type::Unit,
                kind: TypedExprKind::Intrinsic {
                    key: "IrqCap.unmask".to_string(),
                    receiver: Some(Box::new(irq)),
                    type_arg: None,
                    args: Vec::new(),
                },
            })
        }
        other => Err(type_error(
            format!(
                "`{rendered}` has no method `{other}`; 03-hardware.md §6 gives an `IrqCap` \
                 `bind(handler)` and `unmask()`"
            ),
            fspan,
        )),
    }
}

fn check_irq_bind(
    irq: TypedExpr,
    rendered: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`{rendered}.bind(handler)` takes exactly one argument, the ISR to bind \
                 (03-hardware.md §6: `irq.bind(self.on_queue_irq)`); found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if let Some(label) = &arg.label {
        return Err(type_error(
            format!("`bind(handler)`'s handler is positional; `{label}=` names no parameter"),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`bind(handler)`'s argument is a method reference, not a moved value: drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let handler = resolve_irq_bind_handler(&arg.value, arg.span, fctx, mctx)?;
    let _ = fspan;
    Ok(TypedExpr {
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "IrqCap.bind".to_string(),
            receiver: Some(Box::new(irq)),
            type_arg: None,
            args: vec![("handler".to_string(), handler)],
        },
    })
}

/// 03 §6's `self.on_queue_irq` / `BlkDriver.on_queue_irq` spelling: a
/// Field naming a method of the enclosing `@driver` (or of the named
/// type), recorded as a `FnRef` so the sealed-graph pass can see the
/// handler key without ever making method references values in general.
fn resolve_irq_bind_handler(
    expr: &Expr,
    span: Span,
    fctx: &FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let Expr::Field(base, _, method) = expr else {
        return Err(type_error(
            "`IrqCap.bind`'s handler is a method reference \
             (`self.on_queue_irq` or `Driver.on_queue_irq` — 03-hardware.md §6)"
                .to_string(),
            span,
        ));
    };
    // `self.on_queue_irq`
    if let Expr::Name(_, name) = base.as_ref() {
        if name == "self" {
            let Some(self_ty) = fctx.lookup_local("self") else {
                return Err(type_error(
                    "`self.on_queue_irq` is only meaningful inside a method with a `self` receiver"
                        .to_string(),
                    span,
                ));
            };
            let Type::Named(sname, targs) = unwrap_own(self_ty.clone()) else {
                return Err(type_error(
                    format!(
                        "`IrqCap.bind`'s handler must name a method of a `@driver`; `self` has type \
                         `{}`",
                        types::render_type(&self_ty)
                    ),
                    span,
                ));
            };
            return irq_handler_fnref(&sname, &targs, method, span, mctx);
        }
        // `BlkDriver.on_queue_irq` — only works for associated fns today;
        // an instance method under a type name is the same rejection
        // `check_field_expr` already gives, restated for this site.
        if let Some(s) = mctx.structs.get(name.as_str()) {
            if s.method(method).is_some() {
                return irq_handler_fnref(name, &[], method, span, mctx);
            }
            if s.assoc_fn(method).is_some() {
                return irq_handler_fnref(name, &[], method, span, mctx);
            }
            return Err(type_error(
                format!("type `{name}` has no method `{method}` to bind as an ISR"),
                span,
            ));
        }
    }
    Err(type_error(
        "`IrqCap.bind`'s handler is a method reference \
         (`self.on_queue_irq` or `Driver.on_queue_irq` — 03-hardware.md §6)"
            .to_string(),
        span,
    ))
}

fn irq_handler_fnref(
    struct_name: &str,
    targs: &[types::TypeArg],
    method: &str,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    // plans/M7.md item G, decision 18: a mode-generic `@driver`'s ISR
    // lives only on the expanded instantiation (`BlkDriver[DriverMode.Irq]`),
    // never on the unsubstituted template in `mctx.structs`.
    let owned;
    let s: &StructInfo = if targs.is_empty() {
        let Some(s) = mctx.structs.get(struct_name) else {
            return Err(type_error(
                format!("type `{struct_name}` is not a declared struct"),
                span,
            ));
        };
        s
    } else {
        owned = generics::instantiate_struct(mctx, struct_name, targs, span)?;
        &owned
    };
    if !s.decl.is_driver {
        return Err(type_error(
            format!(
                "`IrqCap.bind` binds an ISR of a `@driver`; `{struct_name}` is not a `@driver` \
                 (03-hardware.md §6)"
            ),
            span,
        ));
    }
    let Some((_, d)) = s.method(method).or_else(|| s.assoc_fn(method)) else {
        return Err(type_error(
            format!("`@driver` `{struct_name}` has no method `{method}` to bind as an ISR"),
            span,
        ));
    };
    // An ISR is a plain `fn` returning unit with only `self` (03 §6's
    // worked example). Anything else would need a calling convention the
    // checkpoint dispatch does not have.
    if !d.params.is_empty() {
        return Err(type_error(
            format!(
                "ISR `{struct_name}.{method}` must take no parameters beyond `self` \
                 (03-hardware.md §6's worked `on_queue_irq(self)`); found {} parameter(s)",
                d.params.len()
            ),
            span,
        ));
    }
    if d.ret != Type::Unit {
        return Err(type_error(
            format!(
                "ISR `{struct_name}.{method}` must return `unit` (03-hardware.md §6); found `{}`",
                types::render_type(&d.ret)
            ),
            span,
        ));
    }
    if d.is_async {
        return Err(type_error(
            format!(
                "ISR `{struct_name}.{method}` must be a plain `fn`, not `async fn` \
                 (03-hardware.md §6: an interrupt handler is a plain `fn`)"
            ),
            span,
        ));
    }
    let key = if targs.is_empty() {
        CalleeKey::Method(struct_name.to_string(), method.to_string())
    } else {
        CalleeKey::MethodInstance(
            generics::canonical_key(InstKind::Struct, struct_name, targs),
            method.to_string(),
        )
    };
    Ok(TypedExpr {
        ty: fn_value_type(d),
        kind: TypedExprKind::FnRef(key),
    })
}

// --- plans/M7.md item G, decision 17: InterruptCell[T] (03-hardware.md §6) ---
//
// Sole ISR/ordinary-code channel. Constructor `InterruptCell(v)`; methods
// `load_acquire` / `store_release` / `swap_acquire` / `fetch_or_release`.
// Revision 0.1 admits only `T = u32` (the worked example's cell); every
// other `T` fails closed by name rather than inventing a width.

fn interrupt_cell_elem_ty(cell_ty: &Type, span: Span) -> Result<&Type, SemaError> {
    match cell_ty {
        Type::Named(n, targs) if n == "InterruptCell" => match targs.first() {
            Some(types::TypeArg::Type(inner)) => Ok(inner),
            _ => Err(type_error(
                "`InterruptCell` is missing its element type".to_string(),
                span,
            )),
        },
        _ => Err(type_error(
            format!(
                "expected an `InterruptCell[T]`, found `{}`",
                types::render_type(cell_ty)
            ),
            span,
        )),
    }
}

fn require_interrupt_cell_u32(elem: &Type, span: Span) -> Result<(), SemaError> {
    if matches!(elem, Type::U32) {
        return Ok(());
    }
    Err(type_error(
        format!(
            "`InterruptCell[{}]` is not supported yet — revision 0.1 admits only \
             `InterruptCell[u32]` (03-hardware.md §6's worked example; plans/M7.md item G)",
            types::render_type(elem)
        ),
        span,
    ))
}

fn check_interrupt_cell_new(
    args: &[Arg],
    expected: Option<&Type>,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`InterruptCell(value)` takes exactly one argument; found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if let Some(label) = &arg.label {
        return Err(type_error(
            format!(
                "`InterruptCell(value)`'s argument is positional; `{label}=` names no parameter"
            ),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`InterruptCell(value)` takes a plain value; drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let elem_expected = match expected {
        Some(Type::Named(n, targs)) if n == "InterruptCell" => match targs.first() {
            Some(types::TypeArg::Type(inner)) => Some(inner.clone()),
            _ => None,
        },
        _ => Some(Type::U32),
    };
    let value = check_expr(&arg.value, elem_expected.as_ref(), fctx, mctx)?;
    require_interrupt_cell_u32(&value.ty, arg.span)?;
    let ty = Type::Named(
        "InterruptCell".to_string(),
        vec![types::TypeArg::Type(value.ty.clone())],
    );
    Ok(TypedExpr {
        ty,
        kind: TypedExprKind::Intrinsic {
            key: "InterruptCell.new".to_string(),
            receiver: None,
            type_arg: None,
            args: vec![("value".to_string(), value)],
        },
    })
}

fn check_interrupt_cell_call(
    cell: TypedExpr,
    method: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let rendered = types::render_type(&cell.ty);
    let elem = interrupt_cell_elem_ty(&cell.ty, fspan)?;
    require_interrupt_cell_u32(elem, fspan)?;
    let elem_ty = elem.clone();
    match method {
        "load_acquire" => {
            if !args.is_empty() {
                return Err(type_error(
                    format!(
                        "`{rendered}.load_acquire()` takes no arguments; found {}",
                        args.len()
                    ),
                    call_span,
                ));
            }
            Ok(TypedExpr {
                ty: elem_ty,
                kind: TypedExprKind::Intrinsic {
                    key: "InterruptCell.load_acquire".to_string(),
                    receiver: Some(Box::new(cell)),
                    type_arg: None,
                    args: Vec::new(),
                },
            })
        }
        "store_release" | "swap_acquire" | "fetch_or_release" => {
            let [arg] = args else {
                return Err(type_error(
                    format!(
                        "`{rendered}.{method}(value)` takes exactly one argument; found {}",
                        args.len()
                    ),
                    call_span,
                ));
            };
            if let Some(label) = &arg.label {
                return Err(type_error(
                    format!(
                        "`{method}(value)`'s argument is positional; `{label}=` names no parameter"
                    ),
                    arg.span,
                ));
            }
            if arg.mode != AccessMode::Read {
                return Err(type_error(
                    format!(
                        "`{method}(value)` takes a plain value; drop the `{}`",
                        arg.mode.as_str()
                    ),
                    arg.span,
                ));
            }
            let value = check_expr(&arg.value, Some(&elem_ty), fctx, mctx)?;
            let ret_ty = if method == "store_release" {
                Type::Unit
            } else {
                elem_ty
            };
            Ok(TypedExpr {
                ty: ret_ty,
                kind: TypedExprKind::Intrinsic {
                    key: format!("InterruptCell.{method}"),
                    receiver: Some(Box::new(cell)),
                    type_arg: None,
                    args: vec![("value".to_string(), value)],
                },
            })
        }
        other => Err(type_error(
            format!(
                "`{rendered}` has no method `{other}`; 03-hardware.md §6 gives an `InterruptCell` \
                 `load_acquire()`, `store_release(v)`, `swap_acquire(v)`, and `fetch_or_release(v)`"
            ),
            fspan,
        )),
    }
}

// --- plans/M9.md item F3: `[T; N].map_take` / `try_map_take` (05 §7) --------

fn check_array_map_take(
    base_t: TypedExpr,
    name: &str,
    elem: &Type,
    len: &Expr,
    args: &[Arg],
    _fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if args.len() != 1 {
        return Err(arity_error(1, args.len(), call_span));
    }
    let a = &args[0];
    if a.label.is_some() {
        return Err(type_error(
            format!("`{name}`'s mapper argument must not be labeled"),
            a.span,
        ));
    }
    if a.mode != AccessMode::Read {
        return Err(type_error(
            format!("`{name}`'s mapper is passed unmarked (a function value)"),
            a.span,
        ));
    }
    // Closures need a full expected `fn(...) -> R`; return inference for
    // a mapper is out of this item — named functions match the virtio
    // example (`blocks.map_take(CacheLine.invalid)`).
    if matches!(a.value, Expr::Closure(_)) {
        return Err(type_error(
            format!(
                "`{name}` takes a named function today (`fn(take T) -> ...`); \
                 a closure mapper needs return-type inference (plans/M9.md item F3)"
            ),
            a.span,
        ));
    }
    let mapper = check_expr(&a.value, None, fctx, mctx)?;
    let Type::Fn(params, ret) = &mapper.ty else {
        return Err(type_error(
            format!(
                "`{name}` expects a function value, found `{}`",
                types::render_type(&mapper.ty)
            ),
            a.span,
        ));
    };
    if params.len() != 1 || params[0].0 != AccessMode::Take || !types_eq(&params[0].1, elem) {
        return Err(type_error(
            format!(
                "`{name}` expects `fn(take {}) -> ...`, found `{}`",
                types::render_type(elem),
                types::render_type(&mapper.ty)
            ),
            a.span,
        ));
    }
    match name {
        "map_take" => Ok(TypedExpr {
            ty: Type::Array(Box::new((**ret).clone()), Box::new(len.clone())),
            kind: TypedExprKind::Intrinsic {
                key: "Array.map_take".to_string(),
                receiver: Some(Box::new(base_t)),
                type_arg: None,
                args: vec![("mapper".to_string(), mapper)],
            },
        }),
        "try_map_take" => {
            let Type::Result(ok, err) = ret.as_ref() else {
                return Err(type_error(
                    format!(
                        "`try_map_take` expects `fn(take {}) -> Result[U, E]`, found `{}`",
                        types::render_type(elem),
                        types::render_type(&mapper.ty)
                    ),
                    a.span,
                ));
            };
            // Both element classes must be auto-reclaimable (data); a
            // resource T or U refuses by name (05 §7).
            if is_resource_type(elem, mctx) || is_resource_type(ok, mctx) {
                return Err(type_error(
                    "`try_map_take` requires auto-reclaimable (data) element types; \
                     protocol resources need an explicit loop (05-library.md §7)"
                        .to_string(),
                    call_span,
                ));
            }
            Ok(TypedExpr {
                ty: Type::Result(
                    Box::new(Type::Array(Box::new((**ok).clone()), Box::new(len.clone()))),
                    Box::new((**err).clone()),
                ),
                kind: TypedExprKind::Intrinsic {
                    key: "Array.try_map_take".to_string(),
                    receiver: Some(Box::new(base_t)),
                    type_arg: None,
                    args: vec![("mapper".to_string(), mapper)],
                },
            })
        }
        _ => unreachable!(),
    }
}

// --- plans/M7.md item G: wake(Driver.method) (03-hardware.md §6) ------------
//
// "wake(...) a statically bound task." The argument is the same method-
// reference shape `IrqCap.bind` already accepts; the target must carry
// `@task`. Site legality (ISR or bottom half only) is
// `eval::legal::check_wake_sites`.

fn check_wake_call(
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let [arg] = args else {
        return Err(type_error(
            format!(
                "`wake(task)` takes exactly one argument, a statically bound `@task` method \
                 (03-hardware.md §6: `wake(BlkDriver.drain_used)`); found {}",
                args.len()
            ),
            call_span,
        ));
    };
    if let Some(label) = &arg.label {
        return Err(type_error(
            format!("`wake(task)`'s argument is positional; `{label}=` names no parameter"),
            arg.span,
        ));
    }
    if arg.mode != AccessMode::Read {
        return Err(type_error(
            format!(
                "`wake(task)`'s argument is a method reference, not a moved value: drop the `{}`",
                arg.mode.as_str()
            ),
            arg.span,
        ));
    }
    let target = resolve_wake_target(&arg.value, arg.span, fctx, mctx)?;
    Ok(TypedExpr {
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "wake".to_string(),
            receiver: None,
            type_arg: None,
            args: vec![("task".to_string(), target)],
        },
    })
}

fn resolve_wake_target(
    expr: &Expr,
    span: Span,
    fctx: &FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    // Reuse bind's method-reference resolution, then require `@task`.
    let handler = resolve_irq_bind_handler(expr, span, fctx, mctx).map_err(|e| {
        // Retarget the diagnostic wording from bind to wake.
        if e.message.contains("IrqCap.bind") {
            SemaError {
                message: e
                    .message
                    .replace("`IrqCap.bind`'s handler", "`wake`'s task")
                    .replace("to bind as an ISR", "to wake as a bottom half"),
                ..e
            }
        } else {
            e
        }
    })?;
    let TypedExprKind::FnRef(key) = &handler.kind else {
        return Err(type_error(
            "`wake`'s task must be a method reference (`Driver.drain_used`)".to_string(),
            span,
        ));
    };
    let (sname, method): (String, String) = match key {
        CalleeKey::Method(s, m) => (s.clone(), m.clone()),
        CalleeKey::MethodInstance(ikey, m) => {
            // `struct:Name[Args]` — wake needs the bare struct name for
            // the `@task` lookup on the unspecialized DeclFn (attrs live
            // on the declaration, not the instantiation).
            let bare = ikey
                .strip_prefix("struct:")
                .unwrap_or(ikey.as_str())
                .split('[')
                .next()
                .unwrap_or(ikey.as_str());
            (bare.to_string(), m.clone())
        }
        _ => {
            return Err(type_error(
                "`wake`'s task must name a `@driver` method".to_string(),
                span,
            ));
        }
    };
    let Some(s) = mctx.structs.get(sname.as_str()) else {
        return Err(type_error(
            format!("type `{sname}` is not a declared struct"),
            span,
        ));
    };
    let Some((_, d)) = s.method(&method) else {
        return Err(type_error(
            format!("`@driver` `{sname}` has no method `{method}` to wake"),
            span,
        ));
    };
    if !d.is_task {
        return Err(type_error(
            format!(
                "`wake` requires a statically bound `@task` (03-hardware.md §6); \
                 `{sname}.{method}` is not marked `@task`"
            ),
            span,
        ));
    }
    Ok(handler)
}

// --- plans/M6.md item A: the actor/async surface --------------------------
//
// `Actor[T]` calls (`await`/`send`), `with group(...)`/`g.start`/
// `g.join_all`, and the cross-await path rule (02-language.md §9.2/§9.4/
// §9.5). Every construct outside this exact shape stays fail-closed,
// named, exactly like the rest of decision 7's set.

fn actor_error(message: String, span: Span) -> SemaError {
    SemaError::at("actor", message, span)
}

/// The CallError composition table, verbatim (02-language.md §9.4):
/// `declared R -> Result[R, CallError[never]]`; `declared Result[T, E] ->
/// Result[T, CallError[E]]`. `CallError` is carried as a plain
/// `Type::Named("CallError", [TypeArg::Type(E)])` — the `Option`/`Result`
/// precedent stops at two fixed builtin sums; `CallError`'s own five
/// variants (`Op`/`Cancelled`/`DeadlineExceeded`/`NotAdmitted`/
/// `PeerFailed`) are instead recognized directly wherever a scrutinee's
/// type says `CallError` by name (`variant_payload_types_for`/
/// `matches::shape_of`), the same "builtin_enum_variants precedent" the
/// plan names — a fixed, compiler-known variant/payload table, just with
/// non-empty payloads unlike `Target`/`Failure`'s fieldless ones.
/// Variant erasure (decision 8) ships nothing at M6: no whole-image
/// analysis proves any variant unreachable yet, so every composition
/// keeps the full five-variant `CallError[E]` — recorded, not silently
/// approximated (the plan's own "record what you shipped").
pub(crate) fn compose_call_error(raw: &Type) -> Type {
    match raw {
        Type::Result(t, e) => Type::Result(
            t.clone(),
            Box::new(Type::Named(
                "CallError".to_string(),
                vec![TypeArg::Type((**e).clone())],
            )),
        ),
        other => Type::Result(
            Box::new(other.clone()),
            Box::new(Type::Named(
                "CallError".to_string(),
                vec![TypeArg::Type(Type::Never)],
            )),
        ),
    }
}

/// `compose_call_error`'s exact inverse — the declared reply type behind
/// an already-composed `await` result: `Result[T, CallError[never]]` ->
/// `T`; `Result[T, CallError[E]]` (E != `never`) -> `Result[T, E]`.
/// `None` for anything that is not a composed actor-call result at all.
///
/// plans/M7.md item Z1 (decision 9b) needs this to size an async fn's own
/// reply staging slot (`codegen::Frame::reply_stage_off`) and to decide,
/// per `await` site, whether the wide transport is needed at all: the
/// composed type is already in the FlowWir frame, so inverting it is
/// strictly cheaper than threading the declared type through
/// `flowwir_lower` as a second, drift-prone copy of the same fact.
///
/// It lives here, immediately under the composition it inverts, for the
/// same "one shared definition" reason `sema::types::validate_message_shape`
/// calls `codegen::is_aggregate` directly rather than copying it: the day
/// the table above changes, both halves are on the same screen and cannot
/// silently disagree.
///
/// **The pair is NOT total, and the exception is load-bearing** (found by
/// plans/M7.md item I's sweep; this comment used to claim
/// `decompose_call_error(&compose_call_error(t)) == Some(t)` for every
/// `t`, which is false). `compose_call_error` is not injective: `t = T`
/// and `t = Result[T, never]` both compose to `Result[T, CallError[never]]`,
/// because §9.4's two rows genuinely collide when `E` is `never`. This
/// answers `T` for that composed type, so a `Result[T, never]` reply
/// round-tripped to the *wrong* declared type — and item Z1's transport
/// then read the two ends of one `await` through two different
/// predicates (this one caller-side, `codegen::is_aggregate(&f.ret)`
/// callee-side), which turned the ambiguity into a shifted payload for an
/// aggregate `T` and a write through a null `x8` for a scalar one. The
/// collision is refused at the declaration now
/// (`sema::types::validate_message_shape`, `golden/err-actor-reply-never-error`),
/// which is what restores totality over every reply shape that can reach
/// here — a `never` nested any deeper (`Result[T, Option[never]]`)
/// composes and decomposes correctly and is untouched.
pub(crate) fn decompose_call_error(composed: &Type) -> Option<Type> {
    let Type::Result(t, e) = composed else {
        return None;
    };
    let Type::Named(name, targs) = &**e else {
        return None;
    };
    if name != "CallError" {
        return None;
    }
    let Some(TypeArg::Type(inner)) = targs.first() else {
        return None;
    };
    if matches!(inner, Type::Never) {
        Some((**t).clone())
    } else {
        Some(Type::Result(t.clone(), Box::new(inner.clone())))
    }
}

/// `CallError[E]`'s own variant *numbering* — 02-language.md §9.4 declares
/// the order (`Op`, `Cancelled`, `DeadlineExceeded`, `NotAdmitted`,
/// `PeerFailed`) and `variant_payload_types_for`/`matches::shape_of` above
/// build exactly that order when they type an arm's payload; this is the
/// same table read as an index, which is what a lowered `match`'s own tag
/// comparison needs. `None` for a name that is not a `CallError` variant
/// at all (sema has already rejected those, so a lowering caller treats it
/// as a producer bug).
///
/// It lives here, beside the composition, because `CallError` is the one
/// enum this compiler knows *without* a declaration: it is carried as an
/// instantiated `Type::Named("CallError", [E])` and therefore appears in
/// no `TypedProgram::enums` map, so every consumer that would otherwise
/// look the numbering up has to be told it. Consumers, all cross-checked
/// against this order: `codegen::CALL_ERROR_TAG_CANCELLED` (= 1),
/// `codegen::enum_payload_offset`'s own `CallError` arm, and
/// `flowwir_lower::variant_index`.
pub(crate) fn call_error_variant_index(variant: &str) -> Option<usize> {
    match variant {
        "Op" => Some(0),
        "Cancelled" => Some(1),
        "DeadlineExceeded" => Some(2),
        "NotAdmitted" => Some(3),
        "PeerFailed" => Some(4),
        _ => None,
    }
}

/// Message-value restrictions (02-language.md §9.3): a `mut` loan or a
/// lent closure is rejected, named; `take` of a resource is M7 (fail
/// closed, named, distinct from the flat rejection — the plan's own
/// "distinct message from the flat rejection"); `take` of plain data (not
/// a resource) and a bare `Static[T]`/plain-data argument are both fine,
/// same as an ordinary call. Otherwise identical to `check_call_args`
/// (label/positional binding, defaults) — duplicated rather than
/// threaded through it because `check_call_args` does not return which
/// source `Arg` (and so which `AccessMode`) filled which slot, and this
/// needs exactly that to apply the restriction per argument.
fn check_message_args(
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
        if a.mode == AccessMode::Mut {
            return Err(actor_error(
                "a message argument cannot be a `mut` loan (02-language.md §9.3)".to_string(),
                a.span,
            ));
        }
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
        if matches!(vt.ty, Type::Fn(..)) {
            return Err(actor_error(
                format!(
                    "a message argument cannot be a closure (`{}`, 02-language.md §9.3)",
                    decl_params[idx].name
                ),
                a.span,
            ));
        }
        if matches!(&vt.ty, Type::Named(n, _) if n == "Actor") {
            return Err(actor_error(
                format!(
                    "an `Actor[T]` handle cannot appear in a message (`{}`, 02-language.md §9.1)",
                    decl_params[idx].name
                ),
                a.span,
            ));
        }
        if a.mode == AccessMode::Take && is_resource_type(&vt.ty, mctx) {
            // plans/M7.md item E4 / 03-hardware.md §5: a handoff may
            // `take` an `own[P] T` transfer payload into an awaitable
            // driver call. Other resource takes in messages stay closed.
            if !matches!(&vt.ty, Type::Own(..)) {
                return Err(unimplemented_at(
                    "`take` of a non-`own` resource in a message is",
                    a.span,
                ));
            }
        }
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

/// `await expr` (02-language.md §9.4/§9.5 + 03-hardware.md §3/§5): an
/// actor-handle method call, a group's `join_all()`, or a `Receipt[P]`
/// value (plans/M7.md item E4: `completion = await receipt`).
fn check_await(
    inner: &Expr,
    await_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if !fctx.in_async {
        return Err(actor_error(
            "`await` requires an `async fn`/method — a plain `fn` never suspends \
             (02-language.md §5)"
                .to_string(),
            await_span,
        ));
    }
    // plans/M7.md item E4: `await receipt` — not a call.
    if !matches!(inner, Expr::Call(..)) {
        let inner_t = check_expr(inner, None, fctx, mctx)?;
        if let Type::Named(n, targs) = &inner_t.ty {
            if n == "Receipt" {
                let Some(types::TypeArg::Type(payload)) = targs.first() else {
                    return Err(type_error(
                        "`Receipt` with no payload type argument".to_string(),
                        await_span,
                    ));
                };
                return Ok(TypedExpr {
                    ty: Type::Named(
                        "IoCompletion".to_string(),
                        vec![types::TypeArg::Type(payload.clone())],
                    ),
                    kind: TypedExprKind::Await(Box::new(inner_t)),
                });
            }
        }
        return Err(actor_error(
            "`await` requires an actor call, a group's `join_all()`, or a `Receipt[P]` \
             (03-hardware.md §3: `completion = await receipt`)"
                .to_string(),
            await_span,
        ));
    }
    let Expr::Call(callee_expr, call_span, args) = inner else {
        unreachable!("checked above");
    };
    let Expr::Field(base, fspan, method_name) = callee_expr.as_ref() else {
        return Err(actor_error(
            "`await` requires a method call through an actor handle or a group's `join_all()` \
             (M6 scope)"
                .to_string(),
            *call_span,
        ));
    };
    if method_name == "join_all" {
        if let Expr::Name(_, gname) = base.as_ref() {
            if fctx.lookup_local(gname) == Some(Type::Named("Group".to_string(), vec![])) {
                return check_await_group_join(gname, args, *call_span, fctx);
            }
        }
    }
    check_await_actor_call(base, *fspan, method_name, args, *call_span, fctx, mctx)
}

fn check_await_actor_call(
    base: &Expr,
    fspan: Span,
    method_name: &str,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let base_t = check_expr(base, None, fctx, mctx)?;
    let Type::Named(outer, targs) = &base_t.ty else {
        return Err(actor_error(
            "`await` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    };
    if outer != "Actor" {
        return Err(actor_error(
            "`await` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    }
    let Some(TypeArg::Type(Type::Named(actor_name, _))) = targs.first() else {
        return Err(actor_error(
            "`await` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    };
    let Some(s) = mctx.structs.get(actor_name.as_str()) else {
        return Err(actor_error(
            format!("unknown actor type `{actor_name}`"),
            fspan,
        ));
    };
    let Some((mf, d)) = s.method(method_name) else {
        return Err(missing_method_error(
            format!("type `{actor_name}` has no method `{method_name}`"),
            actor_name,
            method_name,
            fspan,
        ));
    };
    if !mf.is_pub {
        return Err(actor_error(
            format!(
                "`{method_name}` on `{actor_name}` is not `pub` — only a public method is \
                 callable through `Actor[T]`"
            ),
            fspan,
        ));
    }
    if !d.generics.is_empty() {
        // Method-owned generics on an actor-message target: ordinary
        // (non-message) method calls instantiate (item Q); the message
        // path still needs take/handoff composition against the
        // substituted signature — fail closed until that lands.
        return Err(unimplemented_at("generic instantiation is", call_span));
    }
    let typed_args = check_message_args(&mf.params, &d.params, args, call_span, fctx, mctx)?;
    let call = TypedExpr {
        ty: d.ret.clone(),
        kind: TypedExprKind::Call {
            callee: CalleeKey::Method(actor_name.clone(), method_name.to_string()),
            receiver: Some(Box::new(base_t)),
            args: typed_args,
        },
    };
    // 03-hardware.md §5, the handoff calling convention (plans/M8.md item
    // E, decision 32). "Any public synchronous `@driver` method with
    // exactly one `take p: P` parameter and result `Receipt[P]` receives
    // the handoff calling convention" — a *different* convention from
    // 02 §9.4's composed awaitable, and §5 states its result by name:
    // `Receipt[P]`, not `Result[Receipt[P], CallError[never]]`. The
    // receipt is the caller's endpoint on work the device has not done
    // yet; the failure vocabulary that matters to it is the receipt's own
    // state machine (`Resolved` / `Recovery`), reached by `await`ing it,
    // not `CallError`.
    let composed = if s.decl.is_driver && crate::sema::handoff::is_handoff_signature(d) {
        d.ret.clone()
    } else {
        compose_call_error(&d.ret)
    };
    Ok(TypedExpr {
        ty: composed,
        kind: TypedExprKind::Await(Box::new(call)),
    })
}

fn check_await_group_join(
    gname: &str,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
) -> Result<TypedExpr, SemaError> {
    if !args.is_empty() {
        return Err(type_error(
            "`join_all` takes no arguments".to_string(),
            call_span,
        ));
    }
    let Some((child_ty, count)) = fctx.group_children.get(gname).cloned() else {
        return Err(actor_error(
            format!("group `{gname}` has no children started (`g.start`) before `join_all`"),
            call_span,
        ));
    };
    let len_expr = Expr::Int(call_span, count.to_string());
    let group_ty = Type::Named("Group".to_string(), vec![]);
    let receiver = Box::new(TypedExpr {
        ty: group_ty.clone(),
        kind: TypedExprKind::Local(gname.to_string()),
    });
    let raw = Type::Array(Box::new(child_ty.clone()), Box::new(len_expr.clone()));
    let intrinsic = TypedExpr {
        ty: raw,
        kind: TypedExprKind::Intrinsic {
            key: "Group.join_all".to_string(),
            receiver: Some(receiver),
            type_arg: None,
            args: vec![],
        },
    };
    let composed = Type::Array(Box::new(compose_call_error(&child_ty)), Box::new(len_expr));
    Ok(TypedExpr {
        ty: composed,
        kind: TypedExprKind::Await(Box::new(intrinsic)),
    })
}

/// `send actor.method(...)` (02-language.md §9.4), reached both from the
/// expression form (`Expr::Send`) and, for diagnostics only, from the
/// always-rejected bare statement form (`check_send_stmt`). `inner` is
/// always a call (the ast's own comment on both `Expr::Send`/`Stmt::Send`).
fn check_send(
    inner: &Expr,
    send_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if !fctx.in_async {
        return Err(actor_error(
            "`send` requires an `async fn`/method context (M6 scope)".to_string(),
            send_span,
        ));
    }
    let Expr::Call(callee_expr, call_span, args) = inner else {
        return Err(actor_error(
            "`send` requires a call expression".to_string(),
            send_span,
        ));
    };
    let Expr::Field(base, fspan, method_name) = callee_expr.as_ref() else {
        return Err(actor_error(
            "`send` requires a method call through an actor handle".to_string(),
            *call_span,
        ));
    };
    check_send_call(base, *fspan, method_name, args, *call_span, fctx, mctx)
}

fn check_send_call(
    base: &Expr,
    fspan: Span,
    method_name: &str,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let base_t = check_expr(base, None, fctx, mctx)?;
    let Type::Named(outer, targs) = &base_t.ty else {
        return Err(actor_error(
            "`send` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    };
    if outer != "Actor" {
        return Err(actor_error(
            "`send` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    }
    let Some(TypeArg::Type(Type::Named(actor_name, _))) = targs.first() else {
        return Err(actor_error(
            "`send` requires a method call through an `Actor[T]` handle".to_string(),
            call_span,
        ));
    };
    let Some(s) = mctx.structs.get(actor_name.as_str()) else {
        return Err(actor_error(
            format!("unknown actor type `{actor_name}`"),
            fspan,
        ));
    };
    let Some((mf, d)) = s.method(method_name) else {
        return Err(missing_method_error(
            format!("type `{actor_name}` has no method `{method_name}`"),
            actor_name,
            method_name,
            fspan,
        ));
    };
    if !mf.is_pub {
        return Err(actor_error(
            format!(
                "`{method_name}` on `{actor_name}` is not `pub` — only a public method is \
                 callable through `Actor[T]`"
            ),
            fspan,
        ));
    }
    if d.ret != Type::Unit {
        return Err(actor_error(
            format!(
                "`send`'s target method must return `unit`, found `{}` (02-language.md §9.4)",
                types::render_type(&d.ret)
            ),
            fspan,
        ));
    }
    if !d.generics.is_empty() {
        return Err(unimplemented_at("generic instantiation is", call_span));
    }
    let typed_args = check_message_args(&mf.params, &d.params, args, call_span, fctx, mctx)?;
    let call = TypedExpr {
        ty: Type::Unit,
        kind: TypedExprKind::Call {
            callee: CalleeKey::Method(actor_name.clone(), method_name.to_string()),
            receiver: Some(Box::new(base_t)),
            args: typed_args,
        },
    };
    let ty = Type::Result(
        Box::new(Type::Unit),
        Box::new(Type::Named("Rejected".to_string(), vec![])),
    );
    Ok(TypedExpr {
        ty,
        kind: TypedExprKind::Send(Box::new(call)),
    })
}

/// A bare `send` statement (02-language.md §9.4's proof-conditioned
/// form). The call itself is fully typed here, exactly like the
/// expression form; whether the *bare statement* is legal is the
/// whole-image question `sema::send_proof` answers once every module is
/// typed (plans/M6.md item G) — a mailbox capacity lives in the `@image`
/// fn, which no body-checking pass can see. The `send` keyword's own
/// span rides along on the node so that late rejection still reports a
/// real `at L:C` (`TypedStmtKind::BareSend`'s own doc comment).
///
/// Item A's floor — reject every bare `send` here, unconditionally —
/// is what this replaces; a genuine mistake in the call itself (unknown
/// method, bad message argument, a non-`unit` reply, `send` outside an
/// `async fn`) still reports its own error from `check_send` first,
/// before the proof ever runs.
fn check_send_stmt(
    span: Span,
    e: &Expr,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedStmt, SemaError> {
    let expr = check_send(e, span, fctx, mctx)?;
    Ok(TypedStmt {
        kind: TypedStmtKind::BareSend { span, expr },
    })
}

/// `g.start(callee, args...)`'s own callee argument (02-language.md
/// §9.5) — the dumbest doc-consistent callee set (recorded, per the
/// plan's own "decide the dumbest doc-consistent callee set"): a bare
/// same-module top-level `async fn` name, or `self.method` naming an
/// `async fn` method on the enclosing struct. Both are recognized
/// directly (not through `synth_name`'s ordinary lookup — an async
/// fn/method is never otherwise a callable value, see `TypedExprKind::GroupChild`'s
/// own doc comment) so no bound-method-value machinery is needed.
fn resolve_group_child_callee(
    callee_expr: &Expr,
    fctx: &FnCtx,
    mctx: &ModuleCtx,
) -> Result<(CalleeKey, Vec<ast::Param>, Vec<DeclParam>, Type), SemaError> {
    match callee_expr {
        Expr::Name(span, fname) => {
            let Some(fi) = mctx.fns.get(fname) else {
                return Err(actor_error(
                    format!("`{fname}` is not a fn in this module"),
                    *span,
                ));
            };
            if !fi.decl.is_async {
                return Err(unimplemented_at(
                    "`g.start`'s callee must be `async fn` (a sync fn as a group child) is",
                    *span,
                ));
            }
            if !fi.decl.generics.is_empty() {
                return Err(unimplemented_at("generic instantiation is", *span));
            }
            Ok((
                CalleeKey::Fn(fname.clone()),
                fi.ast.params.clone(),
                fi.decl.params.clone(),
                fi.decl.ret.clone(),
            ))
        }
        Expr::Field(recv, span, method) if matches!(recv.as_ref(), Expr::Name(_, n) if n == "self") =>
        {
            let Some(self_ty) = fctx.lookup_local("self") else {
                return Err(actor_error("`self` is not bound here".to_string(), *span));
            };
            let Type::Named(sname, _) = &self_ty else {
                return Err(actor_error(
                    "`self` is not a struct here".to_string(),
                    *span,
                ));
            };
            let Some(s) = mctx.structs.get(sname.as_str()) else {
                return Err(actor_error(format!("unknown type `{sname}`"), *span));
            };
            let Some((mf, d)) = s.method(method) else {
                return Err(missing_method_error(
                    format!("type `{sname}` has no method `{method}`"),
                    sname,
                    method,
                    *span,
                ));
            };
            if !d.is_async {
                return Err(unimplemented_at(
                    "`g.start`'s callee must be `async fn` (a sync method as a group child) is",
                    *span,
                ));
            }
            if !d.generics.is_empty() {
                return Err(unimplemented_at("generic instantiation is", *span));
            }
            Ok((
                CalleeKey::Method(sname.clone(), method.clone()),
                mf.params.clone(),
                d.params.clone(),
                d.ret.clone(),
            ))
        }
        other => Err(unimplemented_at(
            "`g.start`'s callee must be a bare fn name or `self.method` — anything else is",
            other.span(),
        )),
    }
}

fn check_group_start(
    base_t: TypedExpr,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let Some((callee_arg, rest)) = args.split_first() else {
        return Err(type_error(
            "`g.start` needs a callee argument".to_string(),
            call_span,
        ));
    };
    if callee_arg.label.is_some() {
        return Err(type_error(
            "`g.start`'s callee argument must not be labeled".to_string(),
            callee_arg.span,
        ));
    }
    if !matches!(&base_t.kind, TypedExprKind::Local(_)) {
        return Err(actor_error(
            "`g.start`'s receiver must be a group local".to_string(),
            call_span,
        ));
    }
    // The running child count/unified return type is *not* accumulated
    // here (a mutation every pass that re-invokes `bodies::check_expr`
    // on just this one call — `matches.rs`/`flow.rs`'s own re-derived
    // `fctx`, neither of which replays the whole preceding body through
    // `bodies::check_expr` — would have to reproduce identically): it is
    // computed once, up front, by `compute_group_children` (a pure
    // static scan over the raw `with`-body), and `check_with` seeds
    // `fctx.group_children` with that *before* this body is ever walked.
    // This call only needs its own callee's shape to build its own typed
    // node.
    let (callee_key, ast_params, decl_params, ret) =
        resolve_group_child_callee(&callee_arg.value, fctx, mctx)?;
    let typed_args = check_call_args(&ast_params, &decl_params, rest, call_span, fctx, mctx)?;
    let callee_fn_ty = Type::Fn(
        decl_params.iter().map(|p| (p.mode, p.ty.clone())).collect(),
        Box::new(ret),
    );
    let child_node = TypedExpr {
        ty: callee_fn_ty,
        kind: TypedExprKind::GroupChild(callee_key),
    };
    let mut iargs = vec![("callee".to_string(), child_node)];
    for (p, slot) in decl_params.iter().zip(typed_args.into_iter()) {
        if let Some(v) = slot {
            iargs.push((p.name.clone(), v));
        }
    }
    Ok(TypedExpr {
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "Group.start".to_string(),
            receiver: Some(Box::new(base_t)),
            type_arg: None,
            args: iargs,
        },
    })
}

/// One `with group(...) as g:` block's own children, computed once, up
/// front, as a **pure** static scan over the raw `with`-body (no
/// dependence on walk order or on `fctx`'s own mutable state, besides a
/// read-only `self`-type lookup for a `self.method` callee) — see
/// `check_group_start`'s own doc comment for why this must not be
/// incremental: `matches.rs`/`flow.rs` both re-derive their own separate
/// `fctx` and re-invoke `bodies::check_expr` on individual sub-
/// expressions out of full sequence (a plain assignment's inferred
/// type, a `match` scrutinee), never replaying the whole preceding body
/// through it — a pure, order-independent scan is the one shape every
/// pass can call identically and get the same answer. `Ok(None)` means
/// no `g.start` call addressing `gname` was found in `body` at all
/// (`join_all`'s own "no children started" error, not this function's).
pub(crate) fn compute_group_children(
    body: &[Stmt],
    gname: &str,
    fctx: &FnCtx,
    mctx: &ModuleCtx,
) -> Result<Option<(Type, usize)>, SemaError> {
    let mut starts: Vec<&[Arg]> = Vec::new();
    scan_group_starts_stmts(body, gname, &mut starts);
    if starts.is_empty() {
        return Ok(None);
    }
    if let Some(loop_span) = group_starts_inside_loop(body, gname) {
        return Err(actor_error(
            format!(
                "`{gname}.start` cannot appear inside a loop: a group's child count is the number \
                 of *static* `g.start` sites (it is what `join_all`'s own array length and the \
                 group's admission accounting are built from), so one site running twice starts a \
                 child nothing is waiting for — lift the `g.start` out of the loop (M6 scope, \
                 plans/M6.md item H2)"
            ),
            loop_span,
        ));
    }
    let mut result_ty: Option<Type> = None;
    for args in &starts {
        let Some(callee_arg) = args.first() else {
            return Err(type_error(
                "`g.start` needs a callee argument".to_string(),
                Span::default(),
            ));
        };
        let (_, _, _, ret) = resolve_group_child_callee(&callee_arg.value, fctx, mctx)?;
        match &result_ty {
            Some(existing) if *existing != ret => {
                return Err(actor_error(
                    format!(
                        "group `{gname}`'s children must share one return type (M6 scope); \
                         found `{}` and `{}`",
                        types::render_type(existing),
                        types::render_type(&ret)
                    ),
                    callee_arg.span,
                ));
            }
            _ => result_ty = Some(ret),
        }
    }
    Ok(Some((
        result_ty.expect("starts is non-empty"),
        starts.len(),
    )))
}

fn scan_group_starts_stmts<'a>(stmts: &'a [Stmt], gname: &str, out: &mut Vec<&'a [Arg]>) {
    for s in stmts {
        scan_group_starts_stmt(s, gname, out);
    }
}

/// plans/M6.md item H2: does a `g.start` for `gname` sit inside a loop?
///
/// Everything downstream treats the number of *static* `g.start` sites as
/// the number of child activations — `join_all`'s own array length, the
/// group arena's admission accounting, and (since H2) the declared
/// `capacity` check. A `g.start` in a loop breaks that identity: it is one
/// static site that runs N times, so the program compiled clean, started
/// two children, and then deadlocked in `join_all` waiting on a count of
/// one. Rejected by name instead, which is the same discipline decision 5
/// already applies to a bare `send` ("outside any loop... so each executes
/// at most once per root turn").
///
/// Written as its own walk that delegates to `scan_group_starts_stmts`
/// once it is inside a loop body, rather than threading a depth counter
/// through that scanner's fifteen arms: the question is only ever asked
/// about whole loop bodies, and reusing the existing scanner keeps the two
/// from disagreeing about what a `g.start` even is.
fn group_starts_inside_loop(stmts: &[Stmt], gname: &str) -> Option<Span> {
    fn in_loop_body(body: &[Stmt], gname: &str) -> Option<Span> {
        let mut found = Vec::new();
        scan_group_starts_stmts(body, gname, &mut found);
        // The offending `g.start`'s own first argument carries the only
        // span this scanner ever sees (`Arg::span`); a zero-argument
        // `g.start` is already a "needs a callee argument" error one
        // layer up, so the fallback is unreachable in practice.
        found
            .first()
            .map(|args| args.first().map(|a| a.span).unwrap_or_default())
    }
    for s in stmts {
        let hit = match s {
            Stmt::While(w) => in_loop_body(&w.body, gname),
            Stmt::For(f) => in_loop_body(&f.body, gname),
            Stmt::If(i) => group_starts_inside_loop(&i.then_branch, gname)
                .or_else(|| {
                    i.elifs
                        .iter()
                        .find_map(|e| group_starts_inside_loop(&e.body, gname))
                })
                .or_else(|| {
                    i.else_branch
                        .as_ref()
                        .and_then(|b| group_starts_inside_loop(b, gname))
                }),
            Stmt::Match(m) => m
                .arms
                .iter()
                .find_map(|a| group_starts_inside_loop(&a.body, gname)),
            Stmt::With(w) => group_starts_inside_loop(&w.body, gname),
            Stmt::Defer(d) => match &d.body {
                DeferBody::Suite(s) => group_starts_inside_loop(s, gname),
                DeferBody::Expr(_) => None,
            },
            Stmt::ComptimeIf(c) => group_starts_inside_loop(&c.then_branch, gname).or_else(|| {
                c.else_branch
                    .as_ref()
                    .and_then(|b| group_starts_inside_loop(b, gname))
            }),
            _ => None,
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

fn scan_group_starts_stmt<'a>(s: &'a Stmt, gname: &str, out: &mut Vec<&'a [Arg]>) {
    match s {
        Stmt::Assign(a) => {
            scan_group_starts_expr(&a.target, gname, out);
            scan_group_starts_expr(&a.value, gname, out);
        }
        Stmt::If(i) => {
            scan_group_starts_expr(&i.cond, gname, out);
            scan_group_starts_stmts(&i.then_branch, gname, out);
            for elif in &i.elifs {
                scan_group_starts_expr(&elif.cond, gname, out);
                scan_group_starts_stmts(&elif.body, gname, out);
            }
            if let Some(b) = &i.else_branch {
                scan_group_starts_stmts(b, gname, out);
            }
        }
        Stmt::Match(m) => {
            scan_group_starts_expr(&m.scrutinee, gname, out);
            for arm in &m.arms {
                if let Some(g) = &arm.guard {
                    scan_group_starts_expr(g, gname, out);
                }
                scan_group_starts_stmts(&arm.body, gname, out);
            }
        }
        Stmt::For(f) => {
            scan_group_starts_expr(&f.iterable, gname, out);
            scan_group_starts_stmts(&f.body, gname, out);
        }
        Stmt::While(w) => {
            scan_group_starts_expr(&w.cond, gname, out);
            scan_group_starts_stmts(&w.body, gname, out);
        }
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => {}
        Stmt::Return(_, e) => {
            if let Some(e) = e {
                scan_group_starts_expr(e, gname, out);
            }
        }
        Stmt::Assert(a) => {
            scan_group_starts_expr(&a.cond, gname, out);
            if let Some(m) = &a.message {
                scan_group_starts_expr(m, gname, out);
            }
        }
        Stmt::Defer(d) => match &d.body {
            DeferBody::Expr(e) => scan_group_starts_expr(e, gname, out),
            DeferBody::Suite(s) => scan_group_starts_stmts(s, gname, out),
        },
        Stmt::With(w) => {
            scan_group_starts_expr(&w.expr, gname, out);
            scan_group_starts_stmts(&w.body, gname, out);
        }
        Stmt::Send(_, e) => scan_group_starts_expr(e, gname, out),
        Stmt::Expr(_, e) => scan_group_starts_expr(e, gname, out),
        Stmt::ComptimeIf(c) => {
            scan_group_starts_expr(&c.cond, gname, out);
            scan_group_starts_stmts(&c.then_branch, gname, out);
            if let Some(b) = &c.else_branch {
                scan_group_starts_stmts(b, gname, out);
            }
        }
        Stmt::ComptimeAssert(_, e, m) => {
            scan_group_starts_expr(e, gname, out);
            if let Some(m) = m {
                scan_group_starts_expr(m, gname, out);
            }
        }
    }
}

fn scan_group_starts_expr<'a>(e: &'a Expr, gname: &str, out: &mut Vec<&'a [Arg]>) {
    if let Expr::Call(callee, _, args) = e {
        if let Expr::Field(base, _, method) = callee.as_ref() {
            if method == "start" {
                if let Expr::Name(_, bn) = base.as_ref() {
                    if bn == gname {
                        out.push(args);
                    }
                }
            }
        }
    }
    match e {
        Expr::Field(b, _, _) => scan_group_starts_expr(b, gname, out),
        Expr::Index(b, _, args) => {
            scan_group_starts_expr(b, gname, out);
            for a in args {
                scan_group_starts_expr(a, gname, out);
            }
        }
        Expr::Call(callee, _, args) => {
            scan_group_starts_expr(callee, gname, out);
            for a in args {
                scan_group_starts_expr(&a.value, gname, out);
            }
        }
        Expr::Unary(_, _, i) => scan_group_starts_expr(i, gname, out),
        Expr::Try(_, i) => scan_group_starts_expr(i, gname, out),
        Expr::Binary(_, _, l, r) => {
            scan_group_starts_expr(l, gname, out);
            scan_group_starts_expr(r, gname, out);
        }
        Expr::Range(_, a, b, _) => {
            scan_group_starts_expr(a, gname, out);
            scan_group_starts_expr(b, gname, out);
        }
        Expr::Is(_, s, _) => scan_group_starts_expr(s, gname, out),
        Expr::Not(_, i) => scan_group_starts_expr(i, gname, out),
        Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            scan_group_starts_expr(l, gname, out);
            scan_group_starts_expr(r, gname, out);
        }
        Expr::DotVariant(_, _, args) => {
            for a in args {
                scan_group_starts_expr(&a.value, gname, out);
            }
        }
        Expr::Closure(c) => match &c.body {
            ClosureBody::Expr(e) => scan_group_starts_expr(e, gname, out),
            ClosureBody::Suite(s) => scan_group_starts_stmts(s, gname, out),
        },
        Expr::Send(_, i) => scan_group_starts_expr(i, gname, out),
        Expr::Tuple(_, items) | Expr::List(_, items) => {
            for i in items {
                scan_group_starts_expr(i, gname, out);
            }
        }
        _ => {}
    }
}

/// `with group(capacity=.., deadline=..) [as g]:` (02-language.md §9.5,
/// §10). The scoped `pool` form of `with` (02-language.md §10's other
/// intrinsic scope) stays fail-closed — the M6 honest-scope line only
/// lifts `group`.
///
/// plans/M8.md item R, decision 16: the two rejections below are told
/// apart by name. `with pool` is the language's *other* intrinsic scope,
/// unimplemented — `error[unimplemented]`, the fail-closed category, and
/// the only reason 04-compiler.md §3's own group-vs-pool comparison
/// cannot be written as one pair of same-shaped goldens today. Any other
/// constructor is not a `with` form at all (02 §10: "There are no other
/// `with` forms and no user-declared scope protocols") — a permanent
/// `error[type]`, never a fail-closed one, so no reader is left waiting
/// for a milestone that will never come. Before this split, every
/// non-`group` constructor was blamed on `with pool` by name, which was a
/// wrong answer for `with anything_else(...)`; `pool` itself never
/// reached here at all (`intrinsics::is_bare_resolvable` / item I).
fn check_with(w: &WithStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let Expr::Call(ctor, _cspan, cargs) = &w.expr else {
        return Err(unimplemented_at("`with` is", w.span));
    };
    let Expr::Name(_, ctor_name) = ctor.as_ref() else {
        return Err(unimplemented_at("`with` is", w.span));
    };
    if ctor_name == "pool" {
        return Err(unimplemented_at("`with pool` (scoped pools) is", w.span));
    }
    if ctor_name != "group" {
        return Err(type_error(
            format!(
                "`with {ctor_name}(...)` is not a `with` form: `with` opens exactly two \
                 intrinsic suspend-safe scopes, `group` and scoped `pool`, and there are no \
                 others (02-language.md §10 — an acquire/release API is an ordinary function \
                 used with `defer`, or a closure-taking function)"
            ),
            w.span,
        ));
    }
    if !fctx.in_async {
        return Err(actor_error(
            "`with group` requires an `async fn`/method context — a plain `fn` never \
             suspends (02-language.md §5)"
                .to_string(),
            w.span,
        ));
    }
    let mut capacity = None;
    let mut deadline = None;
    for a in cargs {
        match a.label.as_deref() {
            Some("capacity") => {
                capacity = Some(check_expr(&a.value, Some(&Type::Usize), fctx, mctx)?);
            }
            Some("deadline") => {
                deadline = Some(check_deadline_expr(&a.value, fctx, mctx)?);
            }
            Some(other) => {
                return Err(type_error(
                    format!("`group` has no argument `{other}`"),
                    a.span,
                ));
            }
            None => {
                return Err(type_error(
                    "`group`'s arguments must be labeled (`capacity=`/`deadline=`)".to_string(),
                    a.span,
                ));
            }
        }
    }
    let mut child_count = 0usize;
    let body = scoped(fctx, |fctx| {
        if let Some(name) = &w.as_name {
            fctx.insert_local(name.clone(), Type::Named("Group".to_string(), vec![]));
            if let Some(children) = compute_group_children(&w.body, name, fctx, mctx)? {
                child_count = children.1;
                fctx.group_children.insert(name.clone(), children);
            }
        }
        check_stmts(&w.body, fctx, mctx)
    })?;
    check_group_capacity(capacity.as_ref(), child_count, w.span)?;
    if let Some(name) = &w.as_name {
        fctx.group_children.remove(name);
    }
    Ok(TypedStmt {
        kind: TypedStmtKind::WithGroup {
            capacity,
            deadline,
            as_name: w.as_name.clone(),
            body,
        },
    })
}

/// plans/M6.md item H2: a group admits "up to `capacity` child
/// activations (default zero)" — 02-language.md §9.5, verbatim.
///
/// Before this check the declared capacity was inert: it type-checked as
/// a `Usize` and was stored into the group arena at `OFF_GROUP_CAPACITY`,
/// which **nothing ever read**, so `capacity=0` (and an omitted capacity,
/// the documented default) started and completed a child anyway. The
/// adversarial sweep found it; `boot-group-join` could not, because it
/// declares `capacity=2` with exactly two children, so enforced and
/// ignored look identical there.
///
/// Enforced statically rather than at admission time, deliberately. The
/// runtime alternative — refuse the activation and hand back
/// `NotAdmitted` — needs a `CallError` composition that does not exist at
/// M6 (item H3 is the same missing piece surfacing at a mailbox), so the
/// only honest runtime option available today would be an abort. A build
/// error is both dumber and strictly more useful, and it is exact:
/// `compute_group_children` rejects a `g.start` in a loop, so the static
/// site count IS the activation count.
fn check_group_capacity(
    capacity: Option<&TypedExpr>,
    child_count: usize,
    span: Span,
) -> Result<(), SemaError> {
    if child_count == 0 {
        return Ok(()); // a bare deadline scope: nothing to admit.
    }
    let Some(cap_expr) = capacity else {
        return Err(actor_error(
            format!(
                "this `with group` starts {child_count} child activation(s) but declares no \
                 `capacity=`, and a group's default capacity is zero (02-language.md §9.5) — add \
                 `capacity={child_count}`"
            ),
            span,
        ));
    };
    let TypedExprKind::Int(text) = &cap_expr.kind else {
        return Err(unimplemented_at(
            "a `with group` capacity that is not an integer literal is",
            span,
        ));
    };
    let declared: usize = text.parse().map_err(|_| {
        type_error(
            format!("`capacity={text}` is not a valid group capacity"),
            span,
        )
    })?;
    if child_count > declared {
        return Err(actor_error(
            format!(
                "this `with group` declares `capacity={declared}` but starts {child_count} child \
                 activation(s) (02-language.md §9.5: a group admits up to `capacity` children) — \
                 raise the capacity or start fewer children"
            ),
            span,
        ));
    }
    Ok(())
}

/// A group's `deadline=` argument (02-language.md §9.5): `now()` alone,
/// or `now() + ms(...)` — the only two shapes the docs' own examples use.
/// Handled directly rather than through `check_binary`/`build_binop_expr`
/// (which require both operands to share one type, decision 4's own
/// same-type-operand rule — `Instant + Duration` is deliberately not a
/// uniform-type op): the primitive `Binary` node is reused for the sum
/// (mirrors its own doc comment's "builtin scalar op" precedent, extended
/// here to the two other builtin primitive-shaped types this milestone
/// adds), confined to this one call site.
fn check_deadline_expr(
    e: &Expr,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    let instant_ty = Type::Named("Instant".to_string(), vec![]);
    match e {
        Expr::Binary(_span, BinOp::Add, l, r) => {
            let lt = check_expr(l, None, fctx, mctx)?;
            if lt.ty != instant_ty {
                return Err(type_error(
                    "a group deadline must start from `now()`".to_string(),
                    l.span(),
                ));
            }
            let rt = check_expr(r, None, fctx, mctx)?;
            if rt.ty != Type::Named("Duration".to_string(), vec![]) {
                return Err(type_error(
                    "a group deadline's offset must be a duration (`ms(...)`)".to_string(),
                    r.span(),
                ));
            }
            Ok(TypedExpr {
                ty: instant_ty,
                kind: TypedExprKind::Binary(BinOp::Add, Box::new(lt), Box::new(rt)),
            })
        }
        other => {
            let t = check_expr(other, None, fctx, mctx)?;
            if t.ty != instant_ty {
                return Err(type_error(
                    format!(
                        "a group deadline must be an `Instant` (`now()` or `now() + ms(...)`), \
                         found `{}`",
                        types::render_type(&t.ty)
                    ),
                    other.span(),
                ));
            }
            Ok(t)
        }
    }
}

/// Cross-await access rule (02-language.md §9.2): "a whole-value access
/// rooted at the current actor (`self.fs.cache`) may live across `await`
/// ... but an access rooted in an external argument may not." 04 §1:
/// "whole-value accesses surviving `await` are rooted at the current
/// actor turn." The operative verbs are *live across* / *surviving* —
/// the rule is about a path that predates a suspension and is used after
/// it, not about every field access that happens to sit lexically after
/// an `await` in the same body (plans/M9.md item J2d / decision 525).
///
/// Approximation: a straight-line forward scan over an async body's
/// already-typed statements, threading one `seen_await` flag
/// (conservatively shared across sibling branches — an `await` in one
/// `if` arm taints every statement lexically after the whole `if`, even
/// along a sibling arm that itself had none; over-rejects a little,
/// never under-rejects) plus the set of locals whose binding is observed
/// *after* that flag is set (Let / match-arm / for / `with ... as` /
/// post-await Assign-to-local). Any `Field`-chain (`x.a.b`) whose root
/// is not `self` and is not in that post-await set, found once
/// `seen_await` is set, is rejected — a bare local reference (no field)
/// is unaffected, since only a *nested* access is the "whole-value
/// access" §9.2 restricts. A value bound from the await itself
/// (`completion = await receipt`; 03 §3) is in the post-await set and
/// is allowed; an external argument / pre-await local that spans is not.
///
/// **Loop back edges** (plans/M9.md item RR): a forward scan alone is not
/// conservative over a loop. In
///
/// ```text
/// while i < n:
///     total = total + input.value   # <- runs again after the await below
///     r = await self.peer.get()
/// ```
///
/// `input.value` sits lexically *before* the only `await`, so a pure
/// forward scan never has `seen_await` set when it reaches the access —
/// yet every iteration after the first reads `input` on the far side of
/// the previous iteration's suspension, which is exactly what §9.2
/// forbids (the unrolled two-iteration spelling of the same program is
/// rejected). So a `while`/`for` whose body can suspend enters that body
/// with `seen_await` already set and the post-await exemption cleared:
/// the back edge is treated as a suspension the whole body follows.
/// `loop_body_suspends` answers "can this body suspend" by replaying this
/// same scan in `probe` mode, so there is exactly one walk to keep in
/// step with the grammar rather than a second shadow traversal.
///
/// This keeps the over-reject/never-under-reject direction the rest of
/// the approximation promises: a body that provably runs once still pays
/// the loop rule, which is the safe side.
struct CrossAwaitScan {
    seen_await: bool,
    /// Locals bound after `seen_await` became true — they do not span
    /// any suspension observed so far on this forward scan.
    after_await: BTreeSet<String>,
    /// `loop_body_suspends`'s own mode: walk purely to discover whether a
    /// suspension is reachable, reporting nothing. Two effects, both
    /// required for the probe to be a *predicate* rather than a second
    /// checker: the `Field` arm never raises (a probe must not decide the
    /// diagnostic — the real scan that follows does, with the right
    /// state), and the loop arms skip their own probe (the answer to "does
    /// this body contain an await" does not depend on the back-edge rule,
    /// and skipping keeps a nest of `d` loops linear instead of `2^d`).
    probe: bool,
}

fn check_cross_await(body: &[TypedStmt]) -> Result<(), SemaError> {
    let mut state = CrossAwaitScan {
        seen_await: false,
        after_await: BTreeSet::new(),
        probe: false,
    };
    scan_await_cross_stmts(body, &mut state)
}

/// Can `body` reach a suspension? Replays the ordinary scan in `probe`
/// mode, which cannot fail, so the `Err` arm is genuinely unreachable
/// rather than swallowed.
fn loop_body_suspends(body: &[TypedStmt]) -> bool {
    let mut probe = CrossAwaitScan {
        seen_await: false,
        after_await: BTreeSet::new(),
        probe: true,
    };
    let scanned = scan_await_cross_stmts(body, &mut probe);
    debug_assert!(scanned.is_ok(), "a probe-mode scan never reports");
    probe.seen_await
}

/// Shared by the `While` and `For` arms: model the loop's back edge
/// before walking the body (this fn's own `CrossAwaitScan` doc comment).
fn enter_loop_body(body: &[TypedStmt], state: &mut CrossAwaitScan) {
    if state.probe || !loop_body_suspends(body) {
        return;
    }
    state.seen_await = true;
    state.after_await.clear();
}

fn scan_await_cross_stmts(
    stmts: &[TypedStmt],
    state: &mut CrossAwaitScan,
) -> Result<(), SemaError> {
    for s in stmts {
        scan_await_cross_stmt(s, state)?;
    }
    Ok(())
}

fn typed_pattern_bindings(p: &TypedPattern, out: &mut BTreeSet<String>) {
    match &p.kind {
        TypedPatternKind::Wildcard | TypedPatternKind::Literal(_) => {}
        TypedPatternKind::Binding(name) => {
            out.insert(name.clone());
        }
        TypedPatternKind::Take(inner) => typed_pattern_bindings(inner, out),
        TypedPatternKind::Variant { payload, .. } => {
            for sp in payload {
                typed_pattern_bindings(sp, out);
            }
        }
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            for i in items {
                typed_pattern_bindings(i, out);
            }
        }
        TypedPatternKind::Or(alts) => {
            for a in alts {
                typed_pattern_bindings(a, out);
            }
        }
    }
}

fn scan_await_cross_stmt(s: &TypedStmt, state: &mut CrossAwaitScan) -> Result<(), SemaError> {
    match &s.kind {
        TypedStmtKind::Let { name, value, .. } => {
            scan_await_cross_expr(value, state)?;
            if state.seen_await {
                state.after_await.insert(name.clone());
            }
            Ok(())
        }
        TypedStmtKind::Assign { target, value } => {
            scan_await_cross_expr(target, state)?;
            scan_await_cross_expr(value, state)?;
            // A post-await rebind replaces whatever spanned; subsequent
            // field access is on the new value, which did not span.
            if state.seen_await {
                if let TypedExprKind::Local(name) = &target.kind {
                    state.after_await.insert(name.clone());
                }
            }
            Ok(())
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            scan_await_cross_expr(cond, state)?;
            scan_await_cross_stmts(then_branch, state)?;
            for elif in elifs {
                scan_await_cross_expr(&elif.cond, state)?;
                scan_await_cross_stmts(&elif.body, state)?;
            }
            if let Some(b) = else_branch {
                scan_await_cross_stmts(b, state)?;
            }
            Ok(())
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            scan_await_cross_expr(scrutinee, state)?;
            for arm in arms {
                // Pattern bindings introduced once an await is already
                // in view (including `match await ...: case .Ok(x):`) do
                // not span; bindings entered before any await still can.
                if state.seen_await {
                    typed_pattern_bindings(&arm.pattern, &mut state.after_await);
                }
                if let Some(g) = &arm.guard {
                    scan_await_cross_expr(g, state)?;
                }
                scan_await_cross_stmts(&arm.body, state)?;
            }
            Ok(())
        }
        TypedStmtKind::For {
            name, iter, body, ..
        } => {
            match iter {
                TypedForIter::Range(a, b, _) => {
                    scan_await_cross_expr(a, state)?;
                    scan_await_cross_expr(b, state)?;
                }
                TypedForIter::Expr(e) => scan_await_cross_expr(e, state)?,
            }
            enter_loop_body(body, state);
            // The loop variable is rebound by the header on every
            // iteration, *before* the body runs — so it never spans the
            // back edge, and it belongs in the exemption set even when
            // `enter_loop_body` just cleared it. An `await` inside the
            // body still clears it again, which is right: past that
            // suspension this iteration's binding does span.
            if state.seen_await {
                state.after_await.insert(name.clone());
            }
            scan_await_cross_stmts(body, state)
        }
        TypedStmtKind::While { cond, body, .. } => {
            scan_await_cross_expr(cond, state)?;
            enter_loop_body(body, state);
            scan_await_cross_stmts(body, state)
        }
        TypedStmtKind::Break | TypedStmtKind::Continue | TypedStmtKind::Pass => Ok(()),
        TypedStmtKind::Return(value) => match value {
            Some(e) => scan_await_cross_expr(e, state),
            None => Ok(()),
        },
        TypedStmtKind::Assert { cond, message } => {
            scan_await_cross_expr(cond, state)?;
            if let Some(m) = message {
                scan_await_cross_expr(m, state)?;
            }
            Ok(())
        }
        TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            scan_await_cross_expr(cond, state)?;
            if let Some(m) = message {
                scan_await_cross_expr(m, state)?;
            }
            Ok(())
        }
        // A `defer` body runs at cleanup time, not inline in the forward
        // sequence this scan tracks — 02-language.md §10 already forbids
        // `await` inside one (`scan_defer_forbidden`), so it never itself
        // straddles a suspension.
        TypedStmtKind::Defer(_) => Ok(()),
        TypedStmtKind::ExprStmt(e) => scan_await_cross_expr(e, state),
        // plans/M6.md item G: a bare `send`'s message arguments are
        // ordinary expressions and obey 02-language.md §9.2 exactly like
        // any other call's.
        TypedStmtKind::BareSend { expr, .. } => scan_await_cross_expr(expr, state),
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            as_name,
            body,
        } => {
            if let Some(c) = capacity {
                scan_await_cross_expr(c, state)?;
            }
            if let Some(d) = deadline {
                scan_await_cross_expr(d, state)?;
            }
            if state.seen_await {
                if let Some(n) = as_name {
                    state.after_await.insert(n.clone());
                }
            }
            scan_await_cross_stmts(body, state)
        }
    }
}

fn root_local_name(e: &TypedExpr) -> Option<&str> {
    match &e.kind {
        TypedExprKind::Local(name) => Some(name.as_str()),
        TypedExprKind::Field(base, _) => root_local_name(base),
        _ => None,
    }
}

fn scan_await_cross_expr(e: &TypedExpr, state: &mut CrossAwaitScan) -> Result<(), SemaError> {
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
        | TypedExprKind::Static(_)
        | TypedExprKind::FnRef(_)
        | TypedExprKind::PoolName(_)
        | TypedExprKind::GroupChild(_) => Ok(()),
        TypedExprKind::Field(base, _) => {
            if state.seen_await && !state.probe {
                if let Some(root) = root_local_name(e) {
                    if root != "self" && !state.after_await.contains(root) {
                        // No real `L:C` is available here (decision 1:
                        // the typed tree carries no spans at all) —
                        // `omit_location` (`SemaError`'s own multi-line
                        // exception field, `sema::mod`'s doc comment)
                        // suppresses the misleading `at 0:0` a bare
                        // `SemaError::at` would otherwise print.
                        return Err(SemaError {
                            category: "actor",
                            message: format!(
                                "`{root}`-rooted access cannot span an `await` — only a \
                                 self-rooted path may (02-language.md §9.2)"
                            ),
                            line: 0,
                            col: 0,
                            extra_lines: Vec::new(),
                            omit_location: true,
                            missing_method: None,
                        });
                    }
                }
            }
            scan_await_cross_expr(base, state)
        }
        TypedExprKind::Index(base, idx) => {
            scan_await_cross_expr(base, state)?;
            scan_await_cross_expr(idx, state)
        }
        TypedExprKind::Call { receiver, args, .. } => {
            if let Some(r) = receiver {
                scan_await_cross_expr(r, state)?;
            }
            for a in args.iter().flatten() {
                scan_await_cross_expr(a, state)?;
            }
            Ok(())
        }
        TypedExprKind::CallValue(callee, args) => {
            scan_await_cross_expr(callee, state)?;
            for a in args {
                scan_await_cross_expr(a, state)?;
            }
            Ok(())
        }
        TypedExprKind::ToScalar(inner)
        | TypedExprKind::Neg(inner)
        | TypedExprKind::BitNot(inner)
        | TypedExprKind::Take(inner)
        | TypedExprKind::Not(inner) => scan_await_cross_expr(inner, state),
        TypedExprKind::Try(inner, _) => scan_await_cross_expr(inner, state),
        TypedExprKind::Binary(_, l, r) | TypedExprKind::OpCall(_, l, r) => {
            scan_await_cross_expr(l, state)?;
            scan_await_cross_expr(r, state)
        }
        TypedExprKind::Is(inner, _) => scan_await_cross_expr(inner, state),
        TypedExprKind::And(l, r) | TypedExprKind::Or(l, r) => {
            scan_await_cross_expr(l, state)?;
            scan_await_cross_expr(r, state)
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                scan_await_cross_expr(a, state)?;
            }
            Ok(())
        }
        TypedExprKind::Closure { .. } => Ok(()), // a lending call is synchronous (02 §9.2) — never itself spans an await.
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                scan_await_cross_expr(i, state)?;
            }
            Ok(())
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                scan_await_cross_expr(v, state)?;
            }
            Ok(())
        }
        TypedExprKind::Panic(msg) => scan_await_cross_expr(msg, state),
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                scan_await_cross_expr(r, state)?;
            }
            for (_, a) in args {
                scan_await_cross_expr(a, state)?;
            }
            Ok(())
        }
        TypedExprKind::Await(inner) => {
            scan_await_cross_expr(inner, state)?;
            state.seen_await = true;
            // A name bound from an earlier await does not span *that*
            // await, but it does span this one, so the exemption is
            // per-suspension and cannot accumulate.
            state.after_await.clear();
            Ok(())
        }
        TypedExprKind::Send(inner) => scan_await_cross_expr(inner, state),
    }
}

/// `img.driver(A, ...)` / `img.actor(A, ...)` / `img.on_failure(...)` /
/// `img.check_layout(f)` / `img.seal()` — every builder method called
/// directly on the `Image` builder value itself (05-library.md §9);
/// `check_call_by_field`'s own new dispatch reaches here once the
/// receiver's type is confirmed to be `Image`. `img.device`/`img.pool`/
/// `img.dma_pool` are *not* handled here — 05 §9 spells all three with a
/// bracketed type argument (`check_image_bracket_intrinsic`, above), so
/// calling one of them without brackets falls through to the ordinary
/// "no method" diagnostic below, which is exactly right (the shape 05
/// §9 gives is the only one item B accepts, decision 5's "accept the
/// shape the worked examples use and fail closed on anything else").
fn check_image_method_intrinsic(
    name: &str,
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    match name {
        "driver" | "actor" => {
            let Some(first) = args.first() else {
                return Err(type_error(
                    format!("`img.{name}` needs a leading type argument"),
                    call_span,
                ));
            };
            if first.label.is_some() {
                return Err(type_error(
                    format!("`img.{name}`'s leading argument must not be labeled"),
                    first.span,
                ));
            }
            let type_arg = resolve_intrinsic_struct_type_arg(&first.value, mctx)?;
            let iargs = check_intrinsic_args(&args[1..], fctx, mctx)?;
            Ok(TypedExpr {
                ty: image_decl_type(),
                kind: TypedExprKind::Intrinsic {
                    key: format!("Image.{name}"),
                    receiver: None,
                    type_arg: Some(type_arg),
                    args: iargs,
                },
            })
        }
        "on_failure" => {
            let iargs = check_intrinsic_args(args, fctx, mctx)?;
            if !iargs.iter().any(|(l, _)| l == "policy") {
                return Err(type_error(
                    "`img.on_failure` requires `policy`".to_string(),
                    call_span,
                ));
            }
            Ok(TypedExpr {
                ty: Type::Unit,
                kind: TypedExprKind::Intrinsic {
                    key: "Image.on_failure".to_string(),
                    receiver: None,
                    type_arg: None,
                    args: iargs,
                },
            })
        }
        "check_layout" => {
            if args.len() != 1 || args[0].label.is_some() {
                return Err(type_error(
                    "`img.check_layout` takes exactly one positional argument".to_string(),
                    call_span,
                ));
            }
            let f = check_expr(&args[0].value, None, fctx, mctx)?;
            // plans/M9.md item H: the argument must be a `@layout_assert`
            // fn reference — validated against the resolved declaration
            // so a plain `fn` cannot be registered and then fail only at
            // report time.
            match &f.kind {
                TypedExprKind::FnRef(key) => {
                    let name = key.spelling();
                    let Some(info) = mctx.fns.get(&name) else {
                        return Err(type_error(
                            format!("`img.check_layout` argument `{name}` is not a resolvable fn"),
                            call_span,
                        ));
                    };
                    if !is_layout_assert_fn(&info.ast) {
                        return Err(type_error(
                            format!(
                                "`img.check_layout` argument `{name}` must carry `@layout_assert`"
                            ),
                            call_span,
                        ));
                    }
                    // Signature already checked at the fn's own declaration
                    // (`check_layout_assert_fn`); re-check here so an
                    // imported assert whose shape was somehow skipped still
                    // fails closed at the registration site.
                    check_layout_assert_fn(&info.ast, &info.decl, mctx)?;
                }
                _ => {
                    return Err(type_error(
                        "`img.check_layout` takes a bare `@layout_assert` fn name".to_string(),
                        call_span,
                    ));
                }
            }
            Ok(TypedExpr {
                ty: Type::Unit,
                kind: TypedExprKind::Intrinsic {
                    key: "Image.check_layout".to_string(),
                    receiver: None,
                    type_arg: None,
                    args: vec![("f".to_string(), f)],
                },
            })
        }
        "seal" => {
            if !args.is_empty() {
                return Err(type_error(
                    "`img.seal` takes no arguments".to_string(),
                    call_span,
                ));
            }
            Ok(TypedExpr {
                ty: image_type(),
                kind: TypedExprKind::Intrinsic {
                    key: "Image.seal".to_string(),
                    receiver: None,
                    type_arg: None,
                    args: vec![],
                },
            })
        }
        _ => Err(type_error(
            format!("`Image` has no builder method `{name}`"),
            call_span,
        )),
    }
}

/// `decl.handle()` — the one method on a builder declaration handle
/// (05-library.md §9); `check_call_by_field`'s own new dispatch reaches
/// here once the receiver's type is confirmed to be `ImageDecl`. Unlike
/// every `Image`-rooted intrinsic above (which mutate the evaluator's
/// one active builder instead of reading a runtime value, decision 6),
/// `handle()` genuinely needs its own receiver's *value* — which
/// declaration it names — so it is the one case `receiver` on
/// `TypedExprKind::Intrinsic` is ever actually read.
fn check_image_decl_method_intrinsic(
    receiver: TypedExpr,
    name: &str,
    args: &[Arg],
    fspan: Span,
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<TypedExpr, SemaError> {
    if name != "handle" {
        return Err(type_error(
            format!("`ImageDecl` has no method `{name}`"),
            fspan,
        ));
    }
    if !args.is_empty() {
        return Err(type_error(
            "`decl.handle` takes no arguments".to_string(),
            call_span,
        ));
    }
    let _ = (fctx, mctx); // no further checking needed: the receiver is already typed.
    Ok(TypedExpr {
        ty: image_decl_type(),
        kind: TypedExprKind::Intrinsic {
            key: "ImageDecl.handle".to_string(),
            receiver: Some(Box::new(receiver)),
            type_arg: None,
            args: vec![],
        },
    })
}

fn check_struct_construction(
    local_name: &str,
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
    //
    // plans/M9.md item DD / decision 9: the name on `Type::Named` (and on
    // the `StructLiteral` / `init` callee key) is the *local* spelling the
    // author wrote — the key `mctx.structs` was looked up under — not
    // `s.decl.name`, which is the exporter's spelling and stays that way
    // across an `import ... as` alias. Method lookup keys on
    // `Type::Named`; using `decl.name` made `Duo(...).sum()` look for
    // `Pair` in a table that only held `Duo`.
    let self_ty = Type::Named(local_name.to_string(), targs.to_vec());
    if let Some((ia, id)) = s.init() {
        let typed_args = check_call_args(&ia.params, &id.params, args, call_span, fctx, mctx)?;
        let key = if targs.is_empty() {
            CalleeKey::Method(local_name.to_string(), "init".to_string())
        } else {
            CalleeKey::MethodInstance(
                generics::canonical_key(InstKind::Struct, local_name, targs),
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
            name: local_name.to_string(),
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
        let pname = decl_params[idx].name.as_str();
        // plans/M7.md item H2a: a length / allocation-size parameter is
        // exactly the kind of use 03-hardware.md §8 forbids of an
        // unmarked-yet `Untrusted[T]`. Named by the parameter's own
        // spelling so the diagnostic can say which use it was, rather
        // than a generic expected/found mismatch.
        let use_kind = match pname {
            "length" | "len" => Some("a length"),
            "capacity" | "size" => Some("an allocation size"),
            _ => None,
        };
        if let Some(kind) = use_kind {
            let probe = check_expr(&a.value, None, fctx, mctx)?;
            if is_untrusted_type(&probe.ty) {
                return Err(untrusted_use_error(kind, a.span));
            }
        }
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

/// Resolves the concrete declaration used when constructing
/// `Enum.Variant` / `Enum.Variant(...)` (plans/M9.md item J2c).
///
/// Non-generic enums keep their declared shape (`targs` empty). Generic
/// enums take type arguments from `expected` (`return Lookup.Absent`
/// under `Lookup[u32]`) and run them through `generics::instantiate_enum`
/// — the same mechanism pattern typing already used. Missing or
/// mismatched expected type is a precise `error[type]`; associated
/// functions / method-owned type params on a generic enum still refuse
/// elsewhere with the existing `unimplemented` boundary.
fn resolve_enum_for_variant_construction<'a>(
    enum_name: &str,
    info: &'a EnumInfo,
    expected: Option<&Type>,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<(Vec<types::TypeArg>, std::borrow::Cow<'a, types::DeclEnum>), SemaError> {
    if info.generics.is_empty() {
        return Ok((vec![], std::borrow::Cow::Borrowed(&info.decl)));
    }
    match expected {
        Some(Type::Named(n, args)) if n == enum_name => {
            let decl = generics::instantiate_enum(mctx, enum_name, args, span)?;
            Ok((args.clone(), std::borrow::Cow::Owned(decl)))
        }
        Some(other) => Err(type_error(
            format!(
                "expected `{}`, found a `{enum_name}` variant",
                types::render_type(other)
            ),
            span,
        )),
        None => Err(type_error(
            format!("cannot infer type arguments for `{enum_name}` variant construction"),
            span,
        )),
    }
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
        Expr::ArrayRepeat(_, elem, count) => {
            scan_expr_forbidden(elem).or_else(|| scan_expr_forbidden(count))
        }
        _ => None,
    }
}

/// plans/M9.md item F1 decision 344: scan a closure body for `await`.
fn scan_closure_await(body: &ClosureBody) -> Option<Span> {
    match body {
        ClosureBody::Expr(e) => scan_expr_await(e),
        ClosureBody::Suite(stmts) => stmts.iter().find_map(scan_stmt_await),
    }
}

fn scan_stmt_await(s: &Stmt) -> Option<Span> {
    match s {
        Stmt::Assign(a) => scan_expr_await(&a.target).or_else(|| scan_expr_await(&a.value)),
        Stmt::If(i) => scan_expr_await(&i.cond)
            .or_else(|| i.then_branch.iter().find_map(scan_stmt_await))
            .or_else(|| {
                i.elifs.iter().find_map(|e| {
                    scan_expr_await(&e.cond).or_else(|| e.body.iter().find_map(scan_stmt_await))
                })
            })
            .or_else(|| {
                i.else_branch
                    .as_ref()
                    .and_then(|b| b.iter().find_map(scan_stmt_await))
            }),
        Stmt::Match(m) => scan_expr_await(&m.scrutinee).or_else(|| {
            m.arms.iter().find_map(|a| {
                a.guard
                    .as_ref()
                    .and_then(scan_expr_await)
                    .or_else(|| a.body.iter().find_map(scan_stmt_await))
            })
        }),
        Stmt::For(f) => {
            scan_expr_await(&f.iterable).or_else(|| f.body.iter().find_map(scan_stmt_await))
        }
        Stmt::While(w) => {
            scan_expr_await(&w.cond).or_else(|| w.body.iter().find_map(scan_stmt_await))
        }
        Stmt::Return(_, e) => e.as_ref().and_then(scan_expr_await),
        Stmt::Assert(a) => {
            scan_expr_await(&a.cond).or_else(|| a.message.as_ref().and_then(scan_expr_await))
        }
        Stmt::Defer(d) => match &d.body {
            DeferBody::Expr(e) => scan_expr_await(e),
            DeferBody::Suite(s) => s.iter().find_map(scan_stmt_await),
        },
        Stmt::With(w) => {
            scan_expr_await(&w.expr).or_else(|| w.body.iter().find_map(scan_stmt_await))
        }
        Stmt::Send(_, e) | Stmt::Expr(_, e) => scan_expr_await(e),
        Stmt::ComptimeIf(c) => scan_expr_await(&c.cond)
            .or_else(|| c.then_branch.iter().find_map(scan_stmt_await))
            .or_else(|| {
                c.else_branch
                    .as_ref()
                    .and_then(|b| b.iter().find_map(scan_stmt_await))
            }),
        Stmt::ComptimeAssert(_, e, m) => {
            scan_expr_await(e).or_else(|| m.as_ref().and_then(scan_expr_await))
        }
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => None,
    }
}

fn scan_expr_await(e: &Expr) -> Option<Span> {
    match e {
        Expr::Unary(span, UnaryOp::Await, _) => Some(*span),
        Expr::Unary(_, _, inner)
        | Expr::Try(_, inner)
        | Expr::Not(_, inner)
        | Expr::Send(_, inner) => scan_expr_await(inner),
        Expr::Field(base, _, _) => scan_expr_await(base),
        Expr::Index(base, _, args) => {
            scan_expr_await(base).or_else(|| args.iter().find_map(scan_expr_await))
        }
        Expr::Call(callee, _, args) => {
            scan_expr_await(callee).or_else(|| args.iter().find_map(|a| scan_expr_await(&a.value)))
        }
        Expr::Binary(_, _, l, r) | Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            scan_expr_await(l).or_else(|| scan_expr_await(r))
        }
        Expr::Range(_, a, b, _) => scan_expr_await(a).or_else(|| scan_expr_await(b)),
        Expr::Is(_, s, _) => scan_expr_await(s),
        Expr::DotVariant(_, _, args) => args.iter().find_map(|a| scan_expr_await(&a.value)),
        Expr::Closure(c) => scan_closure_await(&c.body),
        Expr::Tuple(_, items) | Expr::List(_, items) => items.iter().find_map(scan_expr_await),
        Expr::ArrayRepeat(_, elem, count) => {
            scan_expr_await(elem).or_else(|| scan_expr_await(count))
        }
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

        // TypeArg::Pool (item D) must compare by name — without this,
        // Option[DmaShared[P, L]] = None fails with identical renderings.
        let shared_a = Type::Named(
            "DmaShared".to_string(),
            vec![
                types::TypeArg::Pool("BlockControl".to_string()),
                types::TypeArg::Type(Type::Named("RingControl".to_string(), vec![])),
            ],
        );
        let shared_b = Type::Named(
            "DmaShared".to_string(),
            vec![
                types::TypeArg::Pool("BlockControl".to_string()),
                types::TypeArg::Type(Type::Named("RingControl".to_string(), vec![])),
            ],
        );
        assert!(
            types_eq(&shared_a, &shared_b),
            "DmaShared[P, L] with equal Pool args must compare equal"
        );
        let shared_other = Type::Named(
            "DmaShared".to_string(),
            vec![
                types::TypeArg::Pool("OtherPool".to_string()),
                types::TypeArg::Type(Type::Named("RingControl".to_string(), vec![])),
            ],
        );
        assert!(
            !types_eq(&shared_a, &shared_other),
            "DmaShared with distinct Pool args must not compare equal"
        );
    }

    // --- plans/M6.md item A: the CallError composition table + path-
    // rooting classification (pure logic, unit-tested directly per the
    // item's own instruction) --------------------------------------------

    fn call_error_of(e: &Type) -> Type {
        Type::Named("CallError".to_string(), vec![TypeArg::Type(e.clone())])
    }

    /// The table verbatim (02-language.md §9.4): "declared R -> Result[R,
    /// CallError[never]]".
    #[test]
    fn compose_call_error_wraps_a_plain_declared_type() {
        let composed = compose_call_error(&Type::U64);
        assert_eq!(
            composed,
            Type::Result(Box::new(Type::U64), Box::new(call_error_of(&Type::Never)))
        );
    }

    /// "declared Result[T, E] -> Result[T, CallError[E]]".
    #[test]
    fn compose_call_error_rewraps_a_declared_result() {
        let declared = Type::Result(
            Box::new(Type::U32),
            Box::new(Type::Named("FsError".to_string(), vec![])),
        );
        let composed = compose_call_error(&declared);
        assert_eq!(
            composed,
            Type::Result(
                Box::new(Type::U32),
                Box::new(call_error_of(&Type::Named("FsError".to_string(), vec![])))
            )
        );
    }

    /// `Option`/`Static`/a bare user struct all fall through the same
    /// "declared R" branch as any other non-`Result` type — the table has
    /// only two cases, not one per shape.
    #[test]
    fn compose_call_error_treats_every_non_result_type_uniformly() {
        let cases = vec![
            Type::Unit,
            Type::Option(Box::new(Type::U8)),
            Type::Named("Widget".to_string(), vec![]),
            Type::Static(Box::new(Type::Str)),
        ];
        for ty in cases {
            let composed = compose_call_error(&ty);
            match composed {
                Type::Result(ok, err) => {
                    assert_eq!(*ok, ty, "the declared type itself must be the Ok payload");
                    assert_eq!(
                        *err,
                        call_error_of(&Type::Never),
                        "error side is CallError[never]"
                    );
                }
                other => panic!("composition must always be a Result, got {other:?}"),
            }
        }
    }

    /// Applying the table twice must not collapse or double-wrap (a
    /// sanity check that the function is a pure, idempotent-shaped
    /// mapping over its input, not a stateful rewrite).
    #[test]
    fn compose_call_error_is_a_pure_function_of_its_input() {
        let a = compose_call_error(&Type::U64);
        let b = compose_call_error(&Type::U64);
        assert_eq!(a, b);
    }

    /// `root_local_name` (the cross-await path-rooting classifier,
    /// 02-language.md §9.2): a bare local's own root is itself; a nested
    /// field chain's root is whatever `Local` sits at the bottom,
    /// regardless of chain depth; anything else (a literal, a call) has
    /// no local root at all.
    #[test]
    fn root_local_name_finds_the_bottom_of_a_field_chain() {
        let self_local = TypedExpr {
            ty: Type::Unit,
            kind: TypedExprKind::Local("self".to_string()),
        };
        assert_eq!(root_local_name(&self_local), Some("self"));

        let one_level = TypedExpr {
            ty: Type::Unit,
            kind: TypedExprKind::Field(Box::new(self_local.clone()), "fs".to_string()),
        };
        assert_eq!(
            root_local_name(&one_level),
            Some("self"),
            "a one-level field access still roots at self"
        );

        let two_level = TypedExpr {
            ty: Type::Unit,
            kind: TypedExprKind::Field(Box::new(one_level), "cache".to_string()),
        };
        assert_eq!(
            root_local_name(&two_level),
            Some("self"),
            "self.fs.cache must still root at self regardless of chain depth"
        );

        let external = TypedExpr {
            ty: Type::Unit,
            kind: TypedExprKind::Local("input".to_string()),
        };
        let external_field = TypedExpr {
            ty: Type::Unit,
            kind: TypedExprKind::Field(Box::new(external), "value".to_string()),
        };
        assert_eq!(root_local_name(&external_field), Some("input"));

        let no_root = TypedExpr {
            ty: Type::U64,
            kind: TypedExprKind::Int("1".to_string()),
        };
        assert_eq!(
            root_local_name(&no_root),
            None,
            "a literal has no local root at all"
        );
    }

    /// `check_cross_await` (02-language.md §9.2): a self-rooted access on
    /// both sides of an `await` is legal; an external-rooted path that
    /// *spans* the await (bound before, field-used after) is rejected;
    /// a local bound *from* the await's result and then field-accessed
    /// is legal — it does not span (03 §3 / plans/M9.md item J2d).
    #[test]
    fn check_cross_await_accepts_self_and_rejects_external_paths() {
        fn field(base_name: &str, field_name: &str) -> TypedExpr {
            TypedExpr {
                ty: Type::U64,
                kind: TypedExprKind::Field(
                    Box::new(TypedExpr {
                        ty: Type::Unit,
                        kind: TypedExprKind::Local(base_name.to_string()),
                    }),
                    field_name.to_string(),
                ),
            }
        }
        fn let_stmt(name: &str, value: TypedExpr) -> TypedStmt {
            TypedStmt {
                kind: TypedStmtKind::Let {
                    name: name.to_string(),
                    ty: value.ty.clone(),
                    value,
                },
            }
        }
        let await_node = TypedExpr {
            ty: Type::Unit,
            kind: TypedExprKind::Await(Box::new(TypedExpr {
                ty: Type::Unit,
                kind: TypedExprKind::Local("dummy".to_string()),
            })),
        };

        // self-rooted before and after the await: legal.
        let self_ok = vec![
            let_stmt("before", field("self", "cache")),
            let_stmt("suspend", await_node.clone()),
            let_stmt("after", field("self", "cache")),
        ];
        assert!(
            check_cross_await(&self_ok).is_ok(),
            "a self-rooted access spanning an await must be accepted"
        );

        // external-rooted, only *after* the await, never bound after it:
        // rejected (the name is an argument / pre-await local).
        let external_after = vec![
            let_stmt("suspend", await_node.clone()),
            let_stmt("bad", field("input", "value")),
        ];
        assert!(
            check_cross_await(&external_after).is_err(),
            "an external-rooted access after an await must be rejected"
        );

        // external-rooted, but entirely *before* the await: legal (the
        // rule is about spanning the suspension, not about touching an
        // external root at all).
        let external_before = vec![
            let_stmt("fine", field("input", "value")),
            let_stmt("suspend", await_node.clone()),
        ];
        assert!(
            check_cross_await(&external_before).is_ok(),
            "an external-rooted access entirely before an await must be accepted"
        );

        // Bound from the await itself, then field-accessed: legal — the
        // local does not span the suspension (03-hardware.md §3).
        let bound_from_await = vec![
            let_stmt("completion", await_node),
            let_stmt("status", field("completion", "status")),
        ];
        assert!(
            check_cross_await(&bound_from_await).is_ok(),
            "a local bound from the await result must be field-accessible after it"
        );
    }
}
