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
//! (beyond its struct's, if any — never instantiated; an open gap), associated functions on a still-
//! generic enum type name, a bare reference to a generic type/fn as a
//! first-class value without calling it, and a generic-argument shape
//! deeper than this item resolves.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::sema::generics;
use crate::sema::typed::{
    CalleeKey, TestDecl, TestKind, TypedCallArg, TypedClosureBody, TypedClosureParam, TypedConst,
    TypedDeferBody, TypedElif, TypedEnum, TypedExpr, TypedExprKind, TypedFn, TypedForIter,
    TypedMatchArm, TypedParam, TypedPattern, TypedPatternKind, TypedProgram, TypedStmt,
    TypedStmtKind, TypedStruct,
};
use crate::sema::types::{
    self, Classification, DeclMember, DeclParam, DeclVariantPayload, Type, TypeArg,
};
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{
    self, AccessMode, Arg, AssertStmt, AssignOp, AssignStmt, BinOp, ClosureBody, ClosureExpr,
    DeferBody, DeferStmt, Expr, ForStmt, IfStmt, Item, MatchArm, MatchStmt, Member, Module,
    NamedType, Pattern, Span, Stmt, UnaryOp, VariantPayload, WhileStmt,
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
/// configuration (CLAUDE.md's "dumbness is permanent").
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

    /// Source `pub` on a field (02-language.md §2 / plans/M13.md item G3).
    pub(crate) fn field_is_pub(&self, name: &str) -> Option<bool> {
        self.decl.members.iter().find_map(|m| match m {
            DeclMember::Field(d) if d.name == name => Some(d.is_pub),
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
    /// plans/M13.md item M / decision 1: spans where
    /// `VirtQueue.reserve` was typed as `QueuePermit` because the use
    /// site expected a permit (`check_virtqueue_reserve`). `reserve_proof`
    /// must succeed whenever this is non-empty; otherwise the site may
    /// keep the Result (and item L refuses silent `Err` discard).
    pub(crate) reserve_permit_demands: RefCell<Vec<Span>>,
    /// plans/M13.md item N: sync loops that omit `@budget`, pending the
    /// observation-discharge check after bodies are typed.
    pub(crate) unbounded_sync_loops: RefCell<Vec<crate::sema::typed::UnboundedSyncLoop>>,
    /// plans/M13.md item K: finalized return types for private
    /// `-> Result[T]` fns (and methods, keyed `Owner.method`), filled
    /// after each body is checked so a later caller sees the concrete
    /// error set rather than the declare-time marker.
    pub(crate) inferred_rets: RefCell<BTreeMap<String, Type>>,
    /// Dotted module path this `ModuleCtx` was built for (plans/M13.md
    /// item G3): field visibility compares the use-site module against
    /// each struct's declaring module.
    pub(crate) module_path: String,
    /// Loader closure key for this module (plans/M15.md item H): equals
    /// `module.path` for ordinary packages; `["core","runtime"]` for the
    /// auto-injected stdlib runtime. `@dmb` is legal only on that key.
    pub(crate) loader_key: Vec<String>,
    /// Local spelling → dotted declaring module for every struct in
    /// `structs` (own declarations + spliced / HH-reachable imports).
    pub(crate) struct_decl_module: BTreeMap<String, String>,
    /// Local spelling → dotted declaring module for free fns (generics
    /// re-check of an imported body needs the exporter as use-site).
    pub(crate) fn_decl_module: BTreeMap<String, String>,
    /// When `generics::check` re-types an exporter's body under an
    /// importer's tables, the exporter's dotted path — field visibility
    /// uses this instead of `module_path` while set.
    pub(crate) visibility_home: RefCell<Option<String>>,
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
    let module_path = module.path.join(".");
    let mut shapes: BTreeMap<String, usize> = imported.clone();
    let mut module_pools = BTreeSet::new();
    let mut structs = BTreeMap::new();
    let mut enums = BTreeMap::new();
    let mut fns = BTreeMap::new();
    let mut consts = BTreeMap::new();
    let mut statics = BTreeMap::new();
    let mut const_values = BTreeMap::new();
    let mut struct_decl_module = BTreeMap::new();
    let mut fn_decl_module = BTreeMap::new();

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
                struct_decl_module.insert(s.name.clone(), module_path.clone());
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
                fn_decl_module.insert(f.name.clone(), module_path.clone());
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
        reserve_permit_demands: RefCell::new(Vec::new()),
        unbounded_sync_loops: RefCell::new(Vec::new()),
        inferred_rets: RefCell::new(BTreeMap::new()),
        module_path,
        loader_key: module.path.clone(),
        struct_decl_module,
        fn_decl_module,
        visibility_home: RefCell::new(None),
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
    pub(crate) quarantined_by_queue: BTreeMap<String, (String, String)>,
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
/// (err-mwir-if-else-scope-leak). This is
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
    // plans/M13.md item M: hand QueuePermit collapse demands to
    // `reserve_proof`.
    program.reserve_permit_demands = mctx.reserve_permit_demands.borrow().clone();
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
pub(crate) fn check_layout_assert_fn(
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
        let receiver = f
            .receiver
            .as_ref()
            .map(|r| (r.mode.unwrap_or(AccessMode::Read), self_ty.clone()));
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
                let receiver = f
                    .receiver
                    .as_ref()
                    .map(|r| (r.mode.unwrap_or(AccessMode::Read), self_ty.clone()));
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
                    receiver: Some((i.receiver.mode.unwrap_or(AccessMode::Mut), self_ty.clone())),
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
                    let v = parse_int_literal(text).ok_or_else(|| {
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

pub(crate) fn check_stmts(
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
        Stmt::Break(span) => Ok(TypedStmt {
            span: *span,
            kind: TypedStmtKind::Break,
        }),
        Stmt::Continue(span) => Ok(TypedStmt {
            span: *span,
            kind: TypedStmtKind::Continue,
        }),
        Stmt::Pass(span) => Ok(TypedStmt {
            span: *span,
            kind: TypedStmtKind::Pass,
        }),
        Stmt::Return(span, e) => check_return(*span, e, fctx, mctx),
        Stmt::Assert(a) => check_assert(a, fctx, mctx),
        Stmt::Defer(d) => check_defer(d, fctx, mctx),
        Stmt::With(w) => check_with(w, fctx, mctx),
        Stmt::Send(span, e) => check_send_stmt(*span, e, fctx, mctx),
        Stmt::Expr(span, e) => Ok(TypedStmt {
            span: *span,
            kind: TypedStmtKind::ExprStmt(check_expr(e, None, fctx, mctx)?),
        }),
        Stmt::Dmb(attr) => check_dmb(attr, mctx),
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

/// `@dmb(ishst)` / `@dmb(ishld)` (plans/M15.md item H, decisions 1080–1085).
/// Legal only inside the auto-injected `stdlib/core/runtime.wr` (loader
/// key `core.runtime`). Lowers to one DMB word; not an author-facing
/// 05 §9 intrinsic.
fn check_dmb(attr: &ast::Attr, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let runtime_ok = mctx.loader_key.len() == crate::loader::RUNTIME_MODULE_KEY.len()
        && mctx
            .loader_key
            .iter()
            .zip(crate::loader::RUNTIME_MODULE_KEY.iter())
            .all(|(a, b)| a == *b);
    if !runtime_ok {
        return Err(SemaError::at(
            "intrinsic",
            "`@dmb` is legal only inside `stdlib/core/runtime.wr` (plans/M15.md item H)"
                .to_string(),
            attr.span,
        ));
    }
    if attr.args.len() != 1 {
        return Err(SemaError::at(
            "intrinsic",
            "`@dmb` takes exactly one argument: `ishst` or `ishld`".to_string(),
            attr.span,
        ));
    }
    let arg = &attr.args[0];
    if arg.label.is_some() || arg.mode != AccessMode::Read {
        return Err(SemaError::at(
            "intrinsic",
            "`@dmb` takes a positional barrier option (`ishst` or `ishld`), not a labeled \
             or `mut`/`take` argument"
                .to_string(),
            arg.span,
        ));
    }
    let key = match &arg.value {
        Expr::Name(_, name) if name == "ishst" => "dmb.ishst",
        Expr::Name(_, name) if name == "ishld" => "dmb.ishld",
        _ => {
            return Err(SemaError::at(
                "intrinsic",
                "`@dmb` option must be `ishst` or `ishld`".to_string(),
                arg.span,
            ));
        }
    };
    // Two literal `key:` sites so plans/M9.md item AA's intrinsic surface
    // census sees both spellings (plans/M15.md item H).
    let kind = match key {
        "dmb.ishst" => TypedExprKind::Intrinsic {
            key: "dmb.ishst".to_string(),
            receiver: None,
            type_arg: None,
            const_arg: None,
            args: Vec::new(),
        },
        "dmb.ishld" => TypedExprKind::Intrinsic {
            key: "dmb.ishld".to_string(),
            receiver: None,
            type_arg: None,
            const_arg: None,
            args: Vec::new(),
        },
        _ => unreachable!("option gated above"),
    };
    Ok(TypedStmt {
        span: attr.span,
        kind: TypedStmtKind::ExprStmt(TypedExpr {
            span: attr.span,
            ty: Type::Unit,
            kind,
        }),
    })
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
        span,
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
        span: i.span,
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
        span: w.span,
        kind: TypedStmtKind::While { cond, body, budget },
    })
}

fn check_match(m: &MatchStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    let discard_ok = match &m.discard {
        Some(attr) => {
            check_discard_attr(attr)?;
            true
        }
        None => false,
    };
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
    // plans/M13.md item L / decision 9 (extended by item M): no silent
    // `Err` discard of a CallError- or CapacityError-bearing Result
    // without `@discard(reason="...")` on this match.
    if !discard_ok {
        check_no_silent_err_discard(&sty, &arms, &m.arms, m.span)?;
    }
    Ok(TypedStmt {
        span: m.span,
        kind: TypedStmtKind::Match { scrutinee, arms },
    })
}

/// `@discard(reason="...")` — plans/M13.md decision 9 / 02 §13.
fn check_discard_attr(attr: &crate::syntax::ast::Attr) -> Result<(), SemaError> {
    debug_assert_eq!(attr.name, "discard");
    if attr.args.len() != 1 {
        return Err(SemaError::at(
            "sema",
            "`@discard` takes exactly one argument `reason=\"...\"` (02-language.md §13)"
                .to_string(),
            attr.span,
        ));
    }
    let a = &attr.args[0];
    let Some(label) = a.label.as_deref() else {
        return Err(SemaError::at(
            "sema",
            "`@discard` takes `reason=\"...\"` (labeled); a positional argument is not the \
             deliberate-discard spelling (02-language.md §13)"
                .to_string(),
            a.span,
        ));
    };
    if label != "reason" {
        return Err(SemaError::at(
            "sema",
            format!("`@discard` takes `reason=\"...\"`; found `{label}=` (02-language.md §13)"),
            a.span,
        ));
    }
    if a.mode != AccessMode::Read {
        return Err(SemaError::at(
            "sema",
            "`@discard`'s `reason=` is a string literal, not a `mut`/`take` place".to_string(),
            a.span,
        ));
    }
    match &a.value {
        Expr::Str(_, text) if !text.is_empty() => Ok(()),
        Expr::Str(_, _) => Err(SemaError::at(
            "sema",
            "`@discard(reason=\"...\")` requires a non-empty reason string".to_string(),
            a.span,
        )),
        _ => Err(SemaError::at(
            "sema",
            "`@discard(reason=\"...\")` requires a string literal reason".to_string(),
            a.span,
        )),
    }
}

/// True when `ty` is `Result[_, CallError[...]]` (the await/send/`?`
/// failure vocabulary after plans/M13.md items I/J).
fn result_err_is_call_error(ty: &Type) -> bool {
    match ty {
        Type::Result(_, err) => matches!(&**err, Type::Named(n, _) if n == "CallError"),
        _ => false,
    }
}

/// True when `ty` is `Result[_, CapacityError]` (proof-conditioned
/// `VirtQueue.reserve` after plans/M13.md item M).
fn result_err_is_capacity_error(ty: &Type) -> bool {
    match ty {
        Type::Result(_, err) => {
            matches!(&**err, Type::Named(n, targs) if n == "CapacityError" && targs.is_empty())
        }
        _ => false,
    }
}

/// plans/M13.md item L (+ M): a match arm that binds `Result.Err` of a
/// CallError- or CapacityError-bearing Result via wildcard or an unused
/// binding is a silent discard unless the match carries `@discard`.
fn check_no_silent_err_discard(
    sty: &Type,
    arms: &[TypedMatchArm],
    ast_arms: &[MatchArm],
    match_span: Span,
) -> Result<(), SemaError> {
    let err_name = if result_err_is_call_error(sty) {
        "CallError"
    } else if result_err_is_capacity_error(sty) {
        "CapacityError"
    } else {
        return Ok(());
    };
    for (arm, ast_arm) in arms.iter().zip(ast_arms.iter()) {
        if err_arm_is_silent_discard(&arm.pattern, &arm.body) {
            let mut e = SemaError::at(
                "sema",
                format!(
                    "silent `Err` discard of `{err_name}` — consume the error, or annotate the \
                     `match` with `@discard(reason=\"...\")` (02-language.md §9.4)"
                ),
                ast_arm.span,
            );
            e.extra_lines = vec![
                "  no silent `Err` discard without `@discard(reason=)`".to_string(),
                "  plans/M13.md item L / decision 9".to_string(),
            ];
            let _ = match_span;
            return Err(e);
        }
    }
    Ok(())
}

/// True when this pattern is a `Result.Err` arm (or a whole-Result
/// wildcard/binding covering Err) that discards its payload.
fn err_arm_is_silent_discard(pattern: &TypedPattern, body: &[TypedStmt]) -> bool {
    match &pattern.kind {
        TypedPatternKind::Variant {
            enum_name,
            variant,
            payload,
        } if (enum_name == "Result" || enum_name.is_empty()) && variant == "Err" => {
            match payload.first() {
                Some(inner) => pattern_is_silent_discard(inner, body),
                // Fieldless Err — still a discard of the error value.
                None => true,
            }
        }
        TypedPatternKind::Or(alts) => alts.iter().any(|a| err_arm_is_silent_discard(a, body)),
        // A bare wildcard / binding against the whole Result covers Err.
        TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => {
            pattern_is_silent_discard(pattern, body)
        }
        TypedPatternKind::Take(inner) => err_arm_is_silent_discard(inner, body),
        _ => false,
    }
}

fn pattern_is_silent_discard(pattern: &TypedPattern, body: &[TypedStmt]) -> bool {
    match &pattern.kind {
        TypedPatternKind::Wildcard => true,
        TypedPatternKind::Binding(name) => !typed_stmts_use_local(body, name),
        TypedPatternKind::Take(inner) => pattern_is_silent_discard(inner, body),
        TypedPatternKind::Tuple(items) | TypedPatternKind::Array(items) => {
            !items.is_empty() && items.iter().all(|i| pattern_is_silent_discard(i, body))
        }
        TypedPatternKind::Variant { payload, .. } => {
            payload.is_empty() || payload.iter().all(|p| pattern_is_silent_discard(p, body))
        }
        TypedPatternKind::Or(alts) => {
            !alts.is_empty() && alts.iter().all(|a| pattern_is_silent_discard(a, body))
        }
        TypedPatternKind::Literal(_) => false,
    }
}

fn typed_stmts_use_local(stmts: &[TypedStmt], name: &str) -> bool {
    let mut found = false;
    for s in stmts {
        walk_typed_stmt_locals(s, &mut |n| {
            if n == name {
                found = true;
            }
        });
        if found {
            return true;
        }
    }
    false
}

fn walk_typed_stmt_locals(s: &TypedStmt, f: &mut dyn FnMut(&str)) {
    // Deliberately walks every subexpression that can name a local — used
    // only to decide whether an Err binding is read (item L). New typed
    // stmt kinds must get a real arm (exhaustive match).
    match &s.kind {
        TypedStmtKind::Let { value, .. } => walk_typed_expr_locals(value, f),
        TypedStmtKind::Assign { target, value } => {
            walk_typed_expr_locals(target, f);
            walk_typed_expr_locals(value, f);
        }
        TypedStmtKind::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        } => {
            walk_typed_expr_locals(cond, f);
            for s in then_branch {
                walk_typed_stmt_locals(s, f);
            }
            for e in elifs {
                walk_typed_expr_locals(&e.cond, f);
                for s in &e.body {
                    walk_typed_stmt_locals(s, f);
                }
            }
            if let Some(b) = else_branch {
                for s in b {
                    walk_typed_stmt_locals(s, f);
                }
            }
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            walk_typed_expr_locals(scrutinee, f);
            for arm in arms {
                for s in &arm.body {
                    walk_typed_stmt_locals(s, f);
                }
                if let Some(g) = &arm.guard {
                    walk_typed_expr_locals(g, f);
                }
            }
        }
        TypedStmtKind::For { iter, body, .. } => {
            match iter {
                TypedForIter::Range(start, end, _) => {
                    walk_typed_expr_locals(start, f);
                    walk_typed_expr_locals(end, f);
                }
                TypedForIter::Expr(e) => walk_typed_expr_locals(e, f),
            }
            for s in body {
                walk_typed_stmt_locals(s, f);
            }
        }
        TypedStmtKind::While { cond, body, .. } => {
            walk_typed_expr_locals(cond, f);
            for s in body {
                walk_typed_stmt_locals(s, f);
            }
        }
        TypedStmtKind::Return(Some(e))
        | TypedStmtKind::ExprStmt(e)
        | TypedStmtKind::BareSend { expr: e, .. } => walk_typed_expr_locals(e, f),
        TypedStmtKind::Assert { cond, message }
        | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
            walk_typed_expr_locals(cond, f);
            if let Some(m) = message {
                walk_typed_expr_locals(m, f);
            }
        }
        TypedStmtKind::Defer(body) => match body {
            TypedDeferBody::Expr(e) => walk_typed_expr_locals(e, f),
            TypedDeferBody::Suite(stmts) => {
                for s in stmts {
                    walk_typed_stmt_locals(s, f);
                }
            }
        },
        TypedStmtKind::WithGroup {
            capacity,
            deadline,
            body,
            ..
        } => {
            if let Some(c) = capacity {
                walk_typed_expr_locals(c, f);
            }
            if let Some(d) = deadline {
                walk_typed_expr_locals(d, f);
            }
            for s in body {
                walk_typed_stmt_locals(s, f);
            }
        }
        TypedStmtKind::Break
        | TypedStmtKind::Continue
        | TypedStmtKind::Pass
        | TypedStmtKind::Return(None) => {}
    }
}

fn walk_typed_expr_locals(e: &TypedExpr, f: &mut dyn FnMut(&str)) {
    match &e.kind {
        TypedExprKind::Local(name) => f(name),
        TypedExprKind::Field(base, _)
        | TypedExprKind::Await(base)
        | TypedExprKind::Send(base)
        | TypedExprKind::Try(base, _)
        | TypedExprKind::Neg(base)
        | TypedExprKind::BitNot(base)
        | TypedExprKind::Take(base)
        | TypedExprKind::ToScalar(base)
        | TypedExprKind::Not(base)
        | TypedExprKind::Panic(base) => walk_typed_expr_locals(base, f),
        TypedExprKind::Index(base, idx) => {
            walk_typed_expr_locals(base, f);
            walk_typed_expr_locals(idx, f);
        }
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::OpCall(_, l, r)
        | TypedExprKind::And(l, r)
        | TypedExprKind::Or(l, r) => {
            walk_typed_expr_locals(l, f);
            walk_typed_expr_locals(r, f);
        }
        TypedExprKind::Call { receiver, args, .. } => {
            if let Some(r) = receiver {
                walk_typed_expr_locals(r, f);
            }
            for a in args {
                if let Some(t) = &a.value {
                    walk_typed_expr_locals(t, f);
                }
            }
        }
        TypedExprKind::CallValue(callee, args) => {
            walk_typed_expr_locals(callee, f);
            for a in args {
                if let Some(t) = &a.value {
                    walk_typed_expr_locals(t, f);
                }
            }
        }
        TypedExprKind::Intrinsic { receiver, args, .. } => {
            if let Some(r) = receiver {
                walk_typed_expr_locals(r, f);
            }
            for (_, a) in args {
                walk_typed_expr_locals(a, f);
            }
        }
        TypedExprKind::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                walk_typed_expr_locals(v, f);
            }
        }
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for i in items {
                walk_typed_expr_locals(i, f);
            }
        }
        TypedExprKind::EnumConstruct { args, .. } => {
            for a in args {
                if let Some(t) = &a.value {
                    walk_typed_expr_locals(t, f);
                }
            }
        }
        TypedExprKind::Is(scrut, _) => walk_typed_expr_locals(scrut, f),
        TypedExprKind::Closure { body, .. } => match body {
            TypedClosureBody::Expr(e) => walk_typed_expr_locals(e, f),
            TypedClosureBody::Suite(stmts) => {
                for s in stmts {
                    walk_typed_stmt_locals(s, f);
                }
            }
        },
        TypedExprKind::Int(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::Str(_)
        | TypedExprKind::BStr(_)
        | TypedExprKind::Const(_)
        | TypedExprKind::Unit
        | TypedExprKind::GroupChild(_)
        | TypedExprKind::PoolName(_)
        | TypedExprKind::FnRef(_)
        | TypedExprKind::Static(_) => {}
    }
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
                span,
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
                span,
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
        span: a.span,
        kind: TypedStmtKind::Assert { cond, message },
    })
}

fn check_for(f: &ForStmt, fctx: &mut FnCtx, mctx: &ModuleCtx) -> Result<TypedStmt, SemaError> {
    // Peel a surface `take` so the iterable types as an array/range, then
    // re-wrap the typed node — access requires `TypedExprKind::Take` on
    // `for take x in take arr` (the AST marker is not otherwise visible
    // on `TypedForIter::Expr`).
    let iterable_taken = matches!(&f.iterable, Expr::Unary(_, UnaryOp::Take, _));
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
                    let te = if iterable_taken {
                        TypedExpr {
                            span: expr_span(&f.iterable),
                            ty: te.ty.clone(),
                            kind: TypedExprKind::Take(Box::new(te)),
                        }
                    } else {
                        te
                    };
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
        span: f.span,
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
        span: d.span,
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
                span: a.span,
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
            span: a.span,
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
        span: a.span,
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
            span: p.span(),
            ty: scrutinee.clone(),
            kind: TypedPatternKind::Wildcard,
        }),
        Pattern::Literal(span, expr) => {
            let te = check_expr(expr, Some(scrutinee), fctx, mctx)?;
            Ok(TypedPattern {
                span: *span,
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Literal(Box::new(te)),
            })
        }
        Pattern::Binding(span, name) => {
            bind_local(fctx, name, scrutinee.clone(), *span)?;
            Ok(TypedPattern {
                span: *span,
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Binding(name.clone()),
            })
        }
        Pattern::Take(span, inner) => {
            let tp = check_pattern(inner, scrutinee, fctx, mctx)?;
            Ok(TypedPattern {
                span: *span,
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
                span: *span,
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
                span: *span,
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
                span: *span,
                ty: scrutinee.clone(),
                kind: TypedPatternKind::Array(typed_items),
            })
        }
        Pattern::Or(span, alts) => {
            // Same-bindings-same-types across alternatives is item G's
            // job (exhaustiveness); each alternative is independently
            // well-formed against the scrutinee here.
            let mut typed_alts = Vec::with_capacity(alts.len());
            for alt in alts {
                typed_alts.push(check_pattern(alt, scrutinee, fctx, mctx)?);
            }
            Ok(TypedPattern {
                span: *span,
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

/// The largest fixed-array length / `Bytes[N]` length / `String[..N]`
/// capacity a build accepts. Shared across the three surfaces so a
/// declared `[T; N]` cannot sneak past the limit that `[elem; N]`
/// expressions and `String[..N]` already enforce — without it,
/// `mwir::size_of` multiplies by a guest-chosen `N` and panics under
/// `[profile.release] overflow-checks = true` (adversarial audit, 2026-07-27).
pub(crate) const MAX_ARRAY_LEN: i128 = 65_536;

/// The largest `String[..N]` capacity a build accepts — the same bound
/// `[elem; N]` already carries, for the same reason and with the same
/// number: a `String[..N]` is one length word plus `N` byte slots, so
/// `N` is an element count in exactly the sense an array's is. At the
/// limit the aggregate is `8 * (1 + 65536)` = 512 KiB, which is already
/// far past anything a 1 GiB guest image should hold in one value.
pub(crate) const MAX_STRING_CAPACITY: i128 = MAX_ARRAY_LEN;

/// Whether a literal array / `Bytes[N]` length is within the build limit.
pub(crate) fn array_len_fits(n: i128) -> bool {
    (0..=MAX_ARRAY_LEN).contains(&n)
}

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
/// types against the scrutinee/expected type via [`crate::sema::sum::sum_ctors`].
fn variant_payload_types_for(
    scrutinee: &Type,
    enum_name: Option<&str>,
    variant: &str,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<Vec<Type>, SemaError> {
    let sum_name = match scrutinee {
        Type::Option(_) => Some("Option"),
        Type::Result(_, _) => Some("Result"),
        Type::Named(name, _) => Some(name.as_str()),
        _ => None,
    };
    if let (Some(expected_name), Some(n)) = (sum_name, enum_name) {
        if n != expected_name {
            let article = match expected_name {
                "Option" | "Result" | "CallError" => "an",
                _ => "a",
            };
            return Err(type_error(
                format!("expected {article} `{expected_name}` pattern, found `{n}`"),
                span,
            ));
        }
    }
    let ctors = match crate::sema::sum::sum_ctors(scrutinee, mctx) {
        Ok(c) => c,
        Err(e) => {
            // Preserve the call-site span; sum_ctors uses a zero span.
            return Err(SemaError::at(e.category, e.message, span));
        }
    };
    match ctors.into_iter().find(|(name, _)| name == variant) {
        Some((_, payloads)) => Ok(payloads),
        None => {
            let label = match scrutinee {
                Type::Option(_) => "`Option`".to_string(),
                Type::Result(_, _) => "`Result`".to_string(),
                Type::Named(name, _) if name == "CallError" => "`CallError`".to_string(),
                Type::Named(name, _) => format!("enum `{name}`"),
                _ => types::render_type(scrutinee),
            };
            Err(type_error(
                format!("{label} has no variant `{variant}`"),
                span,
            ))
        }
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
    let mut actual = synth_expr(expr, expected, fctx, mctx)?;
    actual.span = expr_span(expr);
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
            // plans/M13.md item M / decision 1: proof-conditioned collapse
            // for `VirtQueue.reserve` — a use site that expects
            // `QueuePermit` may take `Result[QueuePermit, CapacityError]`
            // (including a local bound from `reserve`); the whole-image
            // proof must then succeed (`reserve_proof`). Direct
            // `reserve` calls with the same expected type also collapse
            // inside `check_virtqueue_reserve`.
            if is_queue_permit(exp) && is_reserve_capacity_result(&actual.ty) {
                mctx.reserve_permit_demands
                    .borrow_mut()
                    .push(expr_span(expr));
                actual.ty = exp.clone();
                return Ok(actual);
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

fn expr_span(e: &Expr) -> Span {
    e.span()
}

fn is_queue_permit(ty: &Type) -> bool {
    matches!(ty, Type::Named(n, targs) if n == "QueuePermit" && targs.is_empty())
}

fn is_reserve_capacity_result(ty: &Type) -> bool {
    match ty {
        Type::Result(ok, err) => {
            is_queue_permit(ok)
                && matches!(&**err, Type::Named(n, targs) if n == "CapacityError" && targs.is_empty())
        }
        _ => false,
    }
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
                    span: *span,
                    ty: Type::String(n_expr.clone()),
                    kind: TypedExprKind::Str(text.clone()),
                });
            }
            Ok(TypedExpr {
                span: *span,
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
                span: *span,
                ty,
                kind: TypedExprKind::BStr(text.clone()),
            })
        }
        Expr::Char(span, text) => Ok(TypedExpr {
            span: *span,
            ty: Type::Char,
            kind: TypedExprKind::Char(text.clone()),
        }),
        Expr::FStr(f) => check_fstr(f, fctx, mctx),
        Expr::Bool(span, v) => Ok(TypedExpr {
            span: *span,
            ty: Type::Bool,
            kind: TypedExprKind::Bool(*v),
        }),
        Expr::Unit(span) => Ok(TypedExpr {
            span: *span,
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
                span: *span,
                ty,
                kind: TypedExprKind::BitNot(Box::new(it)),
            })
        }
        Expr::Unary(span, UnaryOp::Await, inner) => check_await(inner, *span, fctx, mctx),
        Expr::Unary(span, UnaryOp::Take, inner) => {
            let it = check_expr(inner, expected, fctx, mctx)?;
            let ty = it.ty.clone();
            Ok(TypedExpr {
                span: *span,
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
        Expr::Is(span, scrutinee, pattern) => {
            let st = check_expr(scrutinee, None, fctx, mctx)?;
            let sty = st.ty.clone();
            let pt = check_pattern(pattern, &sty, fctx, mctx)?;
            Ok(TypedExpr {
                span: *span,
                ty: Type::Bool,
                kind: TypedExprKind::Is(Box::new(st), Box::new(pt)),
            })
        }
        Expr::Not(span, inner) => {
            let it = check_expr(inner, Some(&Type::Bool), fctx, mctx)?;
            Ok(TypedExpr {
                span: *span,
                ty: Type::Bool,
                kind: TypedExprKind::Not(Box::new(it)),
            })
        }
        Expr::And(span, l, r) => {
            let lt = check_expr(l, Some(&Type::Bool), fctx, mctx)?;
            let rt = check_expr(r, Some(&Type::Bool), fctx, mctx)?;
            Ok(TypedExpr {
                span: *span,
                ty: Type::Bool,
                kind: TypedExprKind::And(Box::new(lt), Box::new(rt)),
            })
        }
        Expr::Or(span, l, r) => {
            let lt = check_expr(l, Some(&Type::Bool), fctx, mctx)?;
            let rt = check_expr(r, Some(&Type::Bool), fctx, mctx)?;
            Ok(TypedExpr {
                span: *span,
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
                span: *span,
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
        span: span,
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
        span: span,
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
            span: span,
            ty,
            kind: TypedExprKind::Local(name.to_string()),
        });
    }
    if let Some(ty) = mctx.consts.get(name) {
        return Ok(TypedExpr {
            span: span,
            ty: ty.clone(),
            kind: TypedExprKind::Const(name.to_string()),
        });
    }
    if let Some(info) = mctx.statics.get(name) {
        return Ok(TypedExpr {
            span: span,
            ty: info.ty.clone(),
            kind: TypedExprKind::Static(name.to_string()),
        });
    }
    if let Some(f) = mctx.fns.get(name) {
        if !f.decl.generics.is_empty() {
            return Err(unimplemented_at("generic instantiation is", span));
        }
        return Ok(TypedExpr {
            span: span,
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
                span: span,
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
                        span: span,
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
                        span: span,
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
                            span: span,
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
                        span: span,
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
            // One ordered table, shared with `mwir::{size_of, field_offset}`
            // and `lower::field_index` (`mwir::io_completion_fields`).
            let fields =
                crate::mwir::io_completion_fields(targs).map_err(|e| type_error(e, span))?;
            let Some((_, field_ty)) = fields.into_iter().find(|(f, _)| *f == name) else {
                return Err(type_error(
                    format!(
                        "`IoCompletion[P]` has fields `payload`, `status`, and `written_len`; \
                         found `{name}`"
                    ),
                    span,
                ));
            };
            return Ok(TypedExpr {
                span: span,
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
                span: span,
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
                span: span,
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
                check_field_privacy(sname, name, &s, span, mctx)?;
                return Ok(TypedExpr {
                    span: span,
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

/// plans/M13.md item G3 / 02-language.md §2: a non-`pub` field is usable
/// only inside its declaring module (construct / read / write /
/// pattern-bind). Generated `core.__image_runtime` tables stay exempt —
/// handwritten `core.runtime` indexes them by design (same carve-out the
/// G1 census used).
pub(crate) fn check_field_privacy(
    type_name: &str,
    field: &str,
    s: &StructInfo,
    span: Span,
    mctx: &ModuleCtx,
) -> Result<(), SemaError> {
    let Some(is_pub) = s.field_is_pub(field) else {
        return Ok(());
    };
    if is_pub {
        return Ok(());
    }
    let decl_mod = mctx
        .struct_decl_module
        .get(type_name)
        .cloned()
        .unwrap_or_else(|| mctx.module_path.clone());
    let decl_parts: Vec<&str> = decl_mod.split('.').collect();
    if decl_parts.as_slice() == crate::loader::IMAGE_RUNTIME_MODULE_KEY {
        return Ok(());
    }
    let use_mod = mctx
        .visibility_home
        .borrow()
        .clone()
        .unwrap_or_else(|| mctx.module_path.clone());
    if use_mod == decl_mod {
        return Ok(());
    }
    Err(SemaError::at(
        "sema",
        format!(
            "field `{field}` of `{type_name}` is private to module `{decl_mod}`; \
             only that module may construct, read, write, or pattern-bind it \
             (02-language.md §2)"
        ),
        span,
    ))
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
                span: span,
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
                span: span,
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
                span: span,
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
            span: span,
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
        span: span,
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
            span: span,
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
        span: span,
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
    if !array_len_fits(n) {
        return Err(type_error(
            format!("`[elem; N]` count {n} exceeds the {MAX_ARRAY_LEN}-element build limit"),
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
        span: span,
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

fn check_int_range(value: i128, ty: &Type, span: Span) -> Result<(), SemaError> {
    let (min, max) =
        crate::eval::value::int_bounds(ty).expect("check_int_range called with a non-integer type");
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

pub(crate) use crate::eval::value::parse_int_literal;

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
                span: span,
                ty: ty.clone(),
                kind: TypedExprKind::Int(text.clone()),
            };
            Ok(TypedExpr {
                span: span,
                ty,
                kind: TypedExprKind::Neg(Box::new(literal)),
            })
        }
        Expr::Float(_, text) => {
            let te = synth_float_literal(inner.span(), text, expected)?;
            let ty = te.ty.clone();
            Ok(TypedExpr {
                span: span,
                ty,
                kind: TypedExprKind::Neg(Box::new(te)),
            })
        }
        _ => {
            let it = check_expr(inner, expected, fctx, mctx)?;
            if (is_integer_scalar(&it.ty) && is_signed_scalar(&it.ty)) || is_float_scalar(&it.ty) {
                let ty = it.ty.clone();
                Ok(TypedExpr {
                    span: span,
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
        span: span,
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
                span: span,
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
                    span: span,
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
                    span: span,
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
                    span: span,
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
                    span: span,
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
                    span: span,
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
                        span: span,
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
                span: span,
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
        if crate::sema::classes::name_holds_authority(name) {
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
        .map(|r| matches!(r.mode, None | Some(AccessMode::Read)))
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
                    span: span,
                    ty: *t_ok,
                    kind: TypedExprKind::Try(Box::new(inner_t), None),
                })
            }
            Type::Result(_, ret_err) => {
                if types_eq(&t_err, &ret_err) || call_error_e_compatible(&t_err, &ret_err) {
                    Ok(TypedExpr {
                        span: span,
                        ty: *t_ok,
                        kind: TypedExprKind::Try(Box::new(inner_t), None),
                    })
                } else if let Some((conv_ret, key)) = try_from_conversion(&t_err, &ret_err, mctx) {
                    if types_eq(&conv_ret, &ret_err) {
                        Ok(TypedExpr {
                            span: span,
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
                span: span,
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
                && (types_eq(&d.params[0].ty, err_ty)
                    || call_error_e_compatible(&d.params[0].ty, err_ty));
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
                && (types_eq(&d.params[0].ty, err_ty)
                    || call_error_e_compatible(&d.params[0].ty, err_ty));
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
        span: c.span,
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
                span: span,
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
        // plans/M17.md item E / freeze 1 (decision 1250): `entropy[N]()` —
        // bare Name + one bracket length + zero call args. Same
        // Index-then-Call shape as `img.pool[T](...)`. Checked before the
        // generic-instantiation fallthrough.
        if name == "entropy" {
            return check_entropy_call(targs, args, ispan, call_span);
        }
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
                span: call_span,
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
                        span: call_span,
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

/// plans/M17.md item E / freeze 1: `entropy[N]() -> Bytes[N]` with
/// comptime integer literal `N` in `1..=ENTROPY_LEN_MAX` (64). Zero call
/// arguments inside `()`.
fn check_entropy_call(
    targs: &[Expr],
    args: &[Arg],
    ispan: Span,
    call_span: Span,
) -> Result<TypedExpr, SemaError> {
    if targs.len() != 1 {
        return Err(type_error(
            "`entropy` needs exactly one length argument (`entropy[N]()`)".to_string(),
            ispan,
        ));
    }
    if !args.is_empty() {
        return Err(type_error(
            "`entropy[...]()` takes no arguments".to_string(),
            call_span,
        ));
    }
    let n_expr = &targs[0];
    let n = match n_expr {
        Expr::Int(_, text) => {
            let raw = parse_int_literal(text).ok_or_else(|| {
                type_error(
                    format!("`entropy[N]` length `{text}` is not an integer literal"),
                    ispan,
                )
            })?;
            let max = wrela_machine::machine_info::ENTROPY_LEN_MAX as i128;
            if raw < 1 || raw > max {
                return Err(type_error(
                    format!(
                        "`entropy[N]` length must be in 1..={max} (plans/M17.md freeze 1), found {raw}"
                    ),
                    ispan,
                ));
            }
            raw as u64
        }
        _ => {
            return Err(type_error(
                "`entropy[N]` needs an integer literal length".to_string(),
                ispan,
            ));
        }
    };
    Ok(TypedExpr {
        span: call_span,
        ty: Type::Bytes(Some(Box::new(n_expr.clone()))),
        kind: TypedExprKind::Intrinsic {
            key: "entropy".to_string(),
            receiver: None,
            type_arg: None,
            const_arg: Some(n),
            args: vec![],
        },
    })
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
pub(crate) fn image_type() -> Type {
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
pub(crate) fn image_decl_type() -> Type {
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
pub(crate) fn check_intrinsic_args(
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
                        span: a.span,
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
pub(crate) fn resolve_intrinsic_struct_type_arg(
    e: &Expr,
    mctx: &ModuleCtx,
) -> Result<Type, SemaError> {
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
        span: ispan,
        ty: image_decl_type(),
        kind: TypedExprKind::Intrinsic {
            key: format!("Image.{mname}"),
            receiver: None,
            type_arg: Some(type_arg),
            const_arg: None,
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
            span: call_span,
            ty,
            kind: TypedExprKind::Local(name.to_string()),
        };
        return call_fn_value(callee_t, args, call_span, fctx, mctx);
    }
    if let Some(c) = mctx.consts.get(name) {
        let callee_t = TypedExpr {
            span: call_span,
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
                span: call_span,
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
            span: call_span,
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
                span: call_span,
                ty,
                kind: TypedExprKind::EnumConstruct {
                    enum_name: "Option".to_string(),
                    variant: "Some".to_string(),
                    args: vec![TypedCallArg {
                        mode: args[0].mode,
                        value: Some(it),
                    }],
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
                span: call_span,
                ty,
                kind: TypedExprKind::EnumConstruct {
                    enum_name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    args: vec![TypedCallArg {
                        mode: args[0].mode,
                        value: Some(t_typed),
                    }],
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
                span: call_span,
                ty,
                kind: TypedExprKind::EnumConstruct {
                    enum_name: "Result".to_string(),
                    variant: "Err".to_string(),
                    args: vec![TypedCallArg {
                        mode: args[0].mode,
                        value: Some(e_typed),
                    }],
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
                span: call_span,
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
                span: call_span,
                ty: image_type(),
                kind: TypedExprKind::Intrinsic {
                    key: "Image".to_string(),
                    receiver: None,
                    type_arg: None,
                    const_arg: None,
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
                span: call_span,
                ty: Type::Named("Instant".to_string(), vec![]),
                kind: TypedExprKind::Intrinsic {
                    key: "now".to_string(),
                    receiver: None,
                    type_arg: None,
                    const_arg: None,
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
    if !crate::sema::classes::name_holds_authority(name) {
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
    crate::sema::classes::name_holds_authority(name).then_some(name)
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
        span: call_span,
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
                        span: call_span,
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
                        span: call_span,
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
                        span: call_span,
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
            return check_virtqueue_method(
                base_t, name, args, fspan, call_span, expected, fctx, mctx,
            );
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
                    span: call_span,
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
                        span: call_span,
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
                        span: call_span,
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
            span: call_span,
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
        span: call_span,
        ty,
        kind: TypedExprKind::Intrinsic {
            key: format!("Mmio.{op}"),
            receiver: Some(Box::new(mmio)),
            type_arg: Some(scalar),
            const_arg: None,
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
// 05-library.md §6). `Untrusted[T]` is live; `Secret` is refuse-by-name;
// `Validated` is demoted to the `resource(manual)` idiom (plans/M13.md item P). Legacy note: `Validated` / `Secret` are
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

/// Is `ty` the marked wrapper `Untrusted[_]`?
pub(crate) fn is_untrusted_type(ty: &Type) -> bool {
    matches!(ty, Type::Named(name, _) if name == "Untrusted")
}

/// 03-hardware.md §8's rejection for an ordinary use of a marked value:
/// names the use and the one transition that would clear it.
pub(crate) fn untrusted_use_error(use_kind: &str, span: Span) -> SemaError {
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
        span: call_span,
        ty: Type::Result(Box::new(inner.clone()), Box::new(Type::Unit)),
        kind: TypedExprKind::Intrinsic {
            key: "Untrusted.checked_le".to_string(),
            receiver: Some(Box::new(receiver)),
            type_arg: Some(inner.clone()),
            const_arg: None,
            args: vec![("bound".to_string(), bound)],
        },
    })
}

// --- sealed transport: see `sema::transport` ----------------------------
use super::transport::*;
pub use super::transport::{
    is_device_transport_intrinsic, is_interrupt_cell_intrinsic, is_interrupt_cell_type,
    is_irq_cap_intrinsic, is_queue_op_deferred, is_queue_op_intrinsic, is_wake_intrinsic,
};

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
                span: call_span,
                ty: Type::Unit,
                kind: TypedExprKind::Intrinsic {
                    key: "IrqCap.unmask".to_string(),
                    receiver: Some(Box::new(irq)),
                    type_arg: None,
                    const_arg: None,
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
        span: call_span,
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "IrqCap.bind".to_string(),
            receiver: Some(Box::new(irq)),
            type_arg: None,
            const_arg: None,
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
        span: span,
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
        span: call_span,
        ty,
        kind: TypedExprKind::Intrinsic {
            key: "InterruptCell.new".to_string(),
            receiver: None,
            type_arg: None,
            const_arg: None,
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
                span: call_span,
                ty: elem_ty,
                kind: TypedExprKind::Intrinsic {
                    key: "InterruptCell.load_acquire".to_string(),
                    receiver: Some(Box::new(cell)),
                    type_arg: None,
                    const_arg: None,
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
                span: call_span,
                ty: ret_ty,
                kind: TypedExprKind::Intrinsic {
                    key: format!("InterruptCell.{method}"),
                    receiver: Some(Box::new(cell)),
                    type_arg: None,
                    const_arg: None,
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
            span: call_span,
            ty: Type::Array(Box::new((**ret).clone()), Box::new(len.clone())),
            kind: TypedExprKind::Intrinsic {
                key: "Array.map_take".to_string(),
                receiver: Some(Box::new(base_t)),
                type_arg: None,
                const_arg: None,
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
                span: call_span,
                ty: Type::Result(
                    Box::new(Type::Array(Box::new((**ok).clone()), Box::new(len.clone()))),
                    Box::new((**err).clone()),
                ),
                kind: TypedExprKind::Intrinsic {
                    key: "Array.try_map_take".to_string(),
                    receiver: Some(Box::new(base_t)),
                    type_arg: None,
                    const_arg: None,
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
        span: call_span,
        ty: Type::Unit,
        kind: TypedExprKind::Intrinsic {
            key: "wake".to_string(),
            receiver: None,
            type_arg: None,
            const_arg: None,
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

// --- actor/async surface: see `sema::actor` ----------------------------
// Nothing in `actor` is more public than `pub(crate)`, so this is a
// crate-internal re-export, not a `pub` one.
pub(crate) use super::actor::*;

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
pub(crate) fn check_image_method_intrinsic(
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
                span: call_span,
                ty: image_decl_type(),
                kind: TypedExprKind::Intrinsic {
                    key: format!("Image.{name}"),
                    receiver: None,
                    type_arg: Some(type_arg),
                    const_arg: None,
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
                span: call_span,
                ty: Type::Unit,
                kind: TypedExprKind::Intrinsic {
                    key: "Image.on_failure".to_string(),
                    receiver: None,
                    type_arg: None,
                    const_arg: None,
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
                span: call_span,
                ty: Type::Unit,
                kind: TypedExprKind::Intrinsic {
                    key: "Image.check_layout".to_string(),
                    receiver: None,
                    type_arg: None,
                    const_arg: None,
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
                span: call_span,
                ty: image_type(),
                kind: TypedExprKind::Intrinsic {
                    key: "Image.seal".to_string(),
                    receiver: None,
                    type_arg: None,
                    const_arg: None,
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
pub(crate) fn check_image_decl_method_intrinsic(
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
        span: call_span,
        ty: image_decl_type(),
        kind: TypedExprKind::Intrinsic {
            key: "ImageDecl.handle".to_string(),
            receiver: Some(Box::new(receiver)),
            type_arg: None,
            const_arg: None,
            args: vec![],
        },
    })
}

pub(crate) fn check_struct_construction(
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
            span: call_span,
            ty: ret_ty,
            kind: TypedExprKind::Call {
                callee: key,
                receiver: None,
                args: typed_args,
            },
        });
    }
    let fields = check_struct_literal(local_name, s, args, call_span, fctx, mctx)?;
    Ok(TypedExpr {
        span: call_span,
        ty: self_ty,
        kind: TypedExprKind::StructLiteral {
            name: local_name.to_string(),
            fields,
        },
    })
}

/// Re-attach an AST arg access mode onto a struct-literal field value.
/// `TypedExprKind::StructLiteral` has no mode slot (unlike `TypedCallArg`),
/// so `take` must become `TypedExprKind::Take` here or flow treats the
/// place as an implicit copy.
fn wrap_struct_field_mode(mode: AccessMode, value: TypedExpr, span: Span) -> TypedExpr {
    match mode {
        AccessMode::Take => TypedExpr {
            span,
            ty: value.ty.clone(),
            kind: TypedExprKind::Take(Box::new(value)),
        },
        AccessMode::Read | AccessMode::Mut => value,
    }
}

/// A struct without `init` builds from its named-field literal
/// (02-language.md §7.1): every field exactly once unless defaulted,
/// positional only for a one-field struct. Returns only the explicitly
/// supplied fields, declaration order (plans/M3.md item A) — an omitted,
/// defaulted field is elided; its default lives once on
/// `typed::TypedStruct::field_defaults`.
pub(crate) fn check_struct_literal(
    local_name: &str,
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
        check_field_privacy(local_name, &fields[0].0, s, args[0].span, mctx)?;
        let vt = check_expr(&args[0].value, Some(&fields[0].1), fctx, mctx)?;
        // Arg.mode carries `take` (AST marker); StructLiteral has no
        // per-field mode slot, so re-wrap for flow/access (same as
        // transport::with_arg_mode / TypedCallArg.mode on Call).
        let vt = wrap_struct_field_mode(args[0].mode, vt, args[0].span);
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
        check_field_privacy(local_name, label, s, a.span, mctx)?;
        let fty = fields[idx].1.clone();
        let vt = check_expr(&a.value, Some(&fty), fctx, mctx)?;
        let vt = wrap_struct_field_mode(a.mode, vt, a.span);
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
pub(crate) fn check_call_args(
    ast_params: &[ast::Param],
    decl_params: &[DeclParam],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedCallArg>, SemaError> {
    let mut bound = vec![false; decl_params.len()];
    let mut slots: Vec<TypedCallArg> = (0..decl_params.len())
        .map(|_| TypedCallArg {
            mode: AccessMode::Read,
            value: None,
        })
        .collect();
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
        slots[idx] = TypedCallArg {
            mode: a.mode,
            value: Some(vt),
        };
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
pub(crate) fn check_positional_args(
    params: &[(AccessMode, Type)],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedCallArg>, SemaError> {
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
        out.push(TypedCallArg {
            mode: a.mode,
            value: Some(check_expr(&a.value, Some(ty), fctx, mctx)?),
        });
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
pub(crate) fn resolve_enum_for_variant_construction<'a>(
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
pub(crate) fn check_variant_args(
    payload: &[Type],
    args: &[Arg],
    call_span: Span,
    fctx: &mut FnCtx,
    mctx: &ModuleCtx,
) -> Result<Vec<TypedCallArg>, SemaError> {
    if args.len() != payload.len() {
        return Err(arity_error(payload.len(), args.len(), call_span));
    }
    let mut out = Vec::with_capacity(args.len());
    for (a, ty) in args.iter().zip(payload.iter()) {
        out.push(TypedCallArg {
            mode: a.mode,
            value: Some(check_expr(&a.value, Some(ty), fctx, mctx)?),
        });
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
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) | Stmt::Dmb(_) => None,
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
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) | Stmt::Dmb(_) => None,
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

pub(crate) fn type_error(message: String, span: Span) -> SemaError {
    SemaError::at("type", message, span)
}

/// The `type` diagnostic for a missing method/operator method, tagged with
/// structured `(type_name, method_name)` metadata (`SemaError::missing_method`)
/// so `generics.rs`'s requirement-chain diagnostic (item H, decision 2) can
/// recognize this exact shape without parsing `message` back apart. The
/// rendered `message`/category/span are unaffected — the field is metadata
/// only, never printed.
pub(crate) fn missing_method_error(
    message: String,
    type_name: &str,
    method_name: &str,
    span: Span,
) -> SemaError {
    let mut e = type_error(message, span);
    e.missing_method = Some((type_name.to_string(), method_name.to_string()));
    e
}

pub(crate) fn arity_error(expected: usize, found: usize, span: Span) -> SemaError {
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
            Ok(TypedExpr {
                span: _,
                ty: Type::F32,
                ..
            })
        ));
        assert!(matches!(
            synth_float_literal(span, "1.0", Some(&Type::F64)),
            Ok(TypedExpr {
                span: _,
                ty: Type::F64,
                ..
            })
        ));
        assert!(
            synth_float_literal(span, "1.0", Some(&Type::U8)).is_err(),
            "a float literal cannot check against a non-float expected type"
        );
        assert!(matches!(
            synth_float_literal(span, "1.0", None),
            Ok(TypedExpr {
                span: _,
                ty: Type::F64,
                ..
            })
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

    /// plans/M17.md item E / freeze 1: `entropy[N]()` types as `Bytes[N]`
    /// with `const_arg = Some(n)` for literal `N` in `1..=64`.
    #[test]
    fn entropy_intrinsic_types_bytes_n_with_const_arg() {
        let src = "module examples.entropy_sema

pub fn sample() -> Bytes[8]:
    return entropy[8]()
";
        let tokens = crate::syntax::lexer::lex(src).expect("lex");
        let module = crate::syntax::parser::parse(tokens).expect("parse");
        let prog = crate::sema::check_typed(&module, "test.wr").expect("check");
        let f = prog.fns.get("sample").expect("sample");
        let TypedStmtKind::Return(Some(e)) = &f.body.last().expect("ret").kind else {
            panic!("expected return");
        };
        assert!(
            matches!(&e.ty, Type::Bytes(Some(len)) if literal_array_len(len) == Some(8)),
            "expected Bytes[8], got {:?}",
            e.ty
        );
        match &e.kind {
            TypedExprKind::Intrinsic {
                key,
                const_arg,
                args,
                ..
            } => {
                assert_eq!(key, "entropy");
                assert_eq!(*const_arg, Some(8));
                assert!(args.is_empty());
            }
            other => panic!("expected Intrinsic, got {other:?}"),
        }
    }

    #[test]
    fn entropy_rejects_zero_and_over_max() {
        for (n, _) in [(0u64, "zero"), (65u64, "over max")] {
            let src = format!(
                "module examples.entropy_bad_{n}

pub fn sample() -> Bytes[{n}]:
    return entropy[{n}]()
"
            );
            let tokens = crate::syntax::lexer::lex(&src).expect("lex");
            let module = crate::syntax::parser::parse(tokens).expect("parse");
            let err = crate::sema::check_typed(&module, "test.wr").expect_err("must reject");
            assert!(
                err.message.contains("1..=") || err.message.contains("length"),
                "n={n}: {}",
                err.message
            );
        }
    }
}
