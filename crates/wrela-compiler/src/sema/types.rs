//! Declaration typing + classification (plans/M2.md item B): the `Type`
//! enum, resolving every signature/field/const type, and
//! data-vs-resource classification (`resource struct` by fiat, `own[P]
//! T`, and any composite containing a resource, transitively —
//! 02-language.md §3, §6, §7.1), plus `deriving` list validation
//! (§7.5). The check dump's full resolved-signature grammar (decision 8)
//! is also owned here: `mod.rs` only prints the `Module path=...`
//! header and delegates every declaration line to `render_items`.
//!
//! `Type` (decision 4) is one plain enum, structural equality via
//! `derive(PartialEq, Eq, Clone, Debug)`, `Box`/`Vec`, no interning —
//! which is why `syntax::ast` now derives `PartialEq, Eq` throughout
//! (plans/M1.md's AST shape is otherwise unchanged): array lengths,
//! `Bytes[N]`'s length, and generic const arguments all stay unevaluated
//! `Expr`s embedded directly in `Type` (item H evaluates the literal
//! subset later), so `Type`'s own derive needs `Expr` to already derive
//! them.
//!
//! Generic instantiation arguments are resolved structurally but **not**
//! checked/instantiated (that is item H): `Type::Named` carries whatever
//! `TypeArg`s the use site wrote, arity-checked against the declared
//! struct/enum's own generic parameter count (the one item-B-scoped
//! generic validation the plan asks for), nothing more. A bare
//! generic-const identifier (`Bytes[N]`, `Ring[T, N]`) parses as a
//! type-shaped argument — the grammar cannot tell a value name from a
//! type name — so it is specifically unwrapped back into the const
//! expression it actually names rather than misread as a type.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{
    self, AccessMode, Arg, Attr, BinOp, ConstItem, EnumItem, Expr, FieldItem, FnItem, GenericArg,
    GenericParam, InitItem, Item, MatchArm, MatchStmt, Member, Module, NamedType, Param, Pattern,
    Receiver, Span, Stmt, StructItem, VariantPayload,
};
use crate::syntax::printer;

// --- the Type enum -----------------------------------------------------

/// One resolved type (plans/M2.md item B, decision 4). Covers every form
/// 02-language.md §6 lists for revision 0.1's prelude surface (decision
/// 5): scalars, `[T; N]`, tuples, `Option`/`Result`, `own[P] T` (`P`
/// resolved to a declared pool name), `Static[T]`, `Str`, `Bytes[N]`
/// (and bare `Bytes` — bound-elision, parameter position only, §6.2),
/// `fn(mode T, ...) -> R`, a bare generic type parameter, and a user
/// struct/enum reference with its (structurally resolved, not
/// instantiated) generic arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Bool,
    U8,
    U16,
    U32,
    U64,
    Usize,
    I8,
    I16,
    I32,
    I64,
    Isize,
    F32,
    F64,
    Char,
    Unit,
    Never,
    Str,
    /// `[T; N]` — `N` stays an unevaluated expression (item H evaluates
    /// the literal subset).
    Array(Box<Type>, Box<Expr>),
    /// `(A, B, ...)` / one-element `(T,)`.
    Tuple(Vec<Type>),
    Option(Box<Type>),
    Result(Box<Type>, Box<Type>),
    /// `own[P] T` — `P` is the resolved pool name (02-language.md §4).
    Own(String, Box<Type>),
    /// `Static[T]` — always data (02-language.md §6.2: "a copyable
    /// read-only handle"), regardless of `T`.
    Static(Box<Type>),
    /// `Bytes[N]`, or bare `Bytes` (`None`) — legal only in parameter
    /// position (02-language.md §6.2 bound-elision).
    Bytes(Option<Box<Expr>>),
    /// `String[..N]` — owned UTF-8 with compile-time capacity `N`
    /// (02-language.md §6.2). Occupied length is a runtime fact ≤ `N`;
    /// layout is one length word plus `N` byte slots (plans/M9.md item
    /// C1). Bare `String` (bound-elided) is refused by name until a
    /// later item grows it.
    String(Box<Expr>),
    /// `fn(mode T, ...) -> R` (02-language.md §8.3).
    Fn(Vec<(AccessMode, Type)>, Box<Type>),
    /// A bare reference to a type generic parameter currently in scope.
    Generic(String),
    /// A user struct/enum by name, with its (possibly empty) generic
    /// argument list resolved structurally.
    Named(String, Vec<TypeArg>),
}

/// One generic argument at a resolved use site, mirroring
/// `ast::GenericArg`'s three shapes (a type, a bounded-occupancy marker,
/// or a plain comptime expression) with the `Type` case recursively
/// resolved; `Const`/`Bound` keep their expression unevaluated (item H).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeArg {
    Type(Type),
    Const(Expr),
    Bound(Expr),
    /// A bound **pool name** (02-language.md §4's `pool Name`), which is
    /// neither a type nor a const expression. Exactly two type
    /// constructors take one, both hardware (plans/M7.md item D):
    /// `DmaPool[P, N]` (03-hardware.md §1) and `DmaShared[P, L]`
    /// (03-hardware.md §3), each in argument position 0 — the same
    /// identifier `own[P] T` names, which the ast has its own syntax for
    /// and these do not.
    Pool(String),
}

// --- the declared/resolved item shapes the check dump renders ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    Data,
    Resource,
}

#[derive(Debug, Clone)]
pub enum DeclGenericKind {
    Type,
    Const(Type),
}

#[derive(Debug, Clone)]
pub struct DeclGenericParam {
    pub name: String,
    pub kind: DeclGenericKind,
}

#[derive(Debug, Clone)]
pub struct DeclParam {
    pub mode: AccessMode,
    pub name: String,
    pub ty: Type,
}

/// A method/init's receiver. `is_pub`/`is_init` decide how a `Read` mode
/// prints (see `render_receiver`): `mut self`/`take self` are always
/// explicit in source (the grammar has no unwritten default for them),
/// but `Read` is ambiguous — the AST cannot tell a plain `self` from an
/// explicitly spelled `read self` (parser.rs `parse_optional_mode`
/// defaults to `Read` either way) — so a `pub` method (required to spell
/// its effect, 02-language.md §5.1) and `init` (never `pub`, but never
/// "plain" either — it unconditionally begins `mut self`, so `Read` here
/// only ever arises from an explicit spelling) both print the mode; a
/// private ordinary method's `Read` receiver prints bare `self`,
/// deferring to item D's inference.
#[derive(Debug, Clone)]
pub struct DeclReceiver {
    pub mode: AccessMode,
    pub is_pub: bool,
    pub is_init: bool,
}

#[derive(Debug, Clone)]
pub struct DeclFn {
    pub name: String,
    pub is_async: bool,
    /// plans/M7.md item G: `@task(...)` on a `@driver` method — 03 §6's
    /// bottom half. Not a top-level marker (`@test`/`@image`); only
    /// meaningful on a driver member.
    pub is_task: bool,
    pub generics: Vec<DeclGenericParam>,
    pub receiver: Option<DeclReceiver>,
    pub params: Vec<DeclParam>,
    pub ret: Type,
}

#[derive(Debug, Clone)]
pub struct DeclField {
    pub name: String,
    pub ty: Type,
    /// Source `pub` on the field (02-language.md §2 / plans/M13.md item G3).
    /// Carried through declare + subst; bodies.rs refuses cross-module
    /// construct/read/write/pattern-bind of `!is_pub` fields as `error[sema]`.
    pub is_pub: bool,
}

#[derive(Debug, Clone)]
pub enum DeclMember {
    Field(DeclField),
    Fn(DeclFn),
    Init(DeclFn),
    Pool(String),
}

#[derive(Debug, Clone)]
pub struct DeclStruct {
    pub name: String,
    pub generics: Vec<DeclGenericParam>,
    pub deriving: Vec<String>,
    pub classification: Classification,
    pub members: Vec<DeclMember>,
    /// `resource struct`, or `@actor`/`@driver` (02-language.md §7.1) —
    /// resource by fiat, independent of field composition. Classification
    /// bookkeeping only; not rendered directly (`classification` is).
    /// `pub(crate)` (item H, generics.rs): reclassifying a generic
    /// struct's *instantiation* needs this fiat bit alongside its
    /// substituted `component_types` — the same two inputs
    /// `classify_named` below uses, just recomputed once per concrete
    /// instantiation instead of once per declaration.
    pub(crate) is_resource_fiat: bool,
    /// Plans/M6.md item A's own addition: `@actor`/`@driver` specifically
    /// (not `resource struct` in general — `is_resource_fiat` conflates
    /// the two) — the one fact `Actor[T]`'s own validation
    /// (`validate_actor_handles`, below) and `sema::bodies`'s async-surface
    /// checks need that `is_resource_fiat` alone cannot answer (a plain
    /// `resource struct` is not an actor).
    pub(crate) is_actor: bool,
    /// `@driver` specifically, where `is_actor` above conflates
    /// `@actor` with it (plans/M7.md item A). 03-hardware.md §1 turns on
    /// exactly this distinction and nothing else does: a `@driver` **may**
    /// hold capabilities (§1's own worked example holds
    /// `Mmio[VirtioIrqMmio]` in a field and takes `DeviceCap`/`DmaPool`
    /// through its `init`), and an `@actor` may not, "in fields,
    /// parameters, messages, or captures".
    pub(crate) is_driver: bool,
    /// `@layout(<kind>, ...)`'s kind, for a struct carrying that
    /// attribute (plans/M7.md items A/B). Read by
    /// `validate_capability_types` for 03-hardware.md §1/§2's "`Mmio[L]`
    /// — a typed register layout": `L` must name an `@layout(mmio)`
    /// struct. `check_layouts` owns the attribute's *validation* (and has
    /// already run by the time any `DeclStruct` exists); this is only the
    /// already-validated fact, carried forward so the resolved-type passes
    /// can ask it.
    pub(crate) layout_kind: Option<LayoutKind>,
    /// Every field's resolved type + the field's own span, for the
    /// classification/infinite-size pass below — methods/init/pool
    /// members carry no data and do not contribute. `pub(crate)`: see
    /// `is_resource_fiat`.
    pub(crate) component_types: Vec<(Type, Span)>,
    pub(crate) span: Span,
}

#[derive(Debug, Clone)]
pub enum DeclVariantPayload {
    None,
    Tuple(Vec<Type>),
    Named(Vec<(String, Type)>),
}

#[derive(Debug, Clone)]
pub struct DeclVariant {
    pub name: String,
    pub payload: DeclVariantPayload,
}

#[derive(Debug, Clone)]
pub struct DeclEnum {
    pub name: String,
    pub generics: Vec<DeclGenericParam>,
    pub deriving: Vec<String>,
    pub classification: Classification,
    pub variants: Vec<DeclVariant>,
    /// Methods and associated fns (plans/M9.md item B2) — `DeclMember::Fn`
    /// only. Parallel to `DeclStruct::members` for the fn subset so
    /// `bodies`/`access` can zip against the AST the same way.
    pub members: Vec<DeclMember>,
    /// `pub(crate)` (item H): see `DeclStruct::component_types`.
    pub(crate) component_types: Vec<(Type, Span)>,
    pub(crate) span: Span,
}

#[derive(Debug, Clone)]
pub struct DeclConst {
    pub name: String,
    pub ty: Type,
}

/// A module-level `static` (03-hardware.md §3.1, plans/M10.md item A2c).
#[derive(Debug, Clone)]
pub struct DeclStatic {
    pub name: String,
    pub ty: Type,
    /// The `@placed(ADDR)` address — required (decision 586).
    pub addr: u64,
}

#[derive(Debug, Clone)]
pub enum DeclItem {
    Const(DeclConst),
    Static(DeclStatic),
    Fn(DeclFn),
    Struct(DeclStruct),
    Enum(DeclEnum),
    Pool(String),
}

// --- the declare pass ----------------------------------------------------

/// A type generic parameter resolves a bare name to `Type::Generic`; a
/// const generic parameter is never itself a type — used only to reject
/// (in bare type position) or reinterpret (in generic-argument position,
/// see `resolve_type_arg`/`resolve_bytes_arg`) the identifier the grammar
/// hands back for it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenericKind {
    Type,
    Const,
}

/// Every struct/enum's own generic-parameter *count*, for arity-checking
/// a `Name[Args]` use elsewhere in a signature (plans/M2.md item B: "the
/// one generic validation this item does"). Built once, up front, over
/// raw AST so forward references (a field naming a struct declared later
/// in the file) resolve exactly like backward ones — `collect` (item A)
/// already guarantees every module-scope name is unique.
fn build_shapes(module: &Module, imported: &ImportedTypes) -> BTreeMap<String, usize> {
    // plans/M9.md item A1, decision 8: the imported names go in *first*,
    // so a local declaration always wins a spelling contest. It can never
    // actually come to that — `imports::resolve_imports` already rejects
    // an import that "collides with a local declaration" — but the table
    // is the type namespace, and the type namespace does not get to have
    // two answers for one name.
    let mut shapes: BTreeMap<String, usize> = imported.clone();
    for item in &module.items {
        match item {
            Item::Struct(s) => {
                shapes.insert(s.name.clone(), s.generics.len());
            }
            Item::Enum(e) => {
                shapes.insert(e.name.clone(), e.generics.len());
            }
            _ => {}
        }
    }
    shapes
}

fn module_pool_names(module: &Module) -> BTreeSet<String> {
    module
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Pool(p) => Some(p.name.clone()),
            _ => None,
        })
        .collect()
}

/// Resolves every module-level signature into a `Vec<DeclItem>` (source
/// order, `comptime if` items skipped — not expanded until item C,
/// exactly like item A's `collect`/`resolve`), then classifies every
/// struct/enum data-vs-resource. Fail-fast: the first error found in
/// source order (arity/unknown-type/bound-elision during resolution;
/// unknown-deriving/bad-From-shape checked right after each struct's/
/// enum's own header, source order within the item) wins, before
/// classification (which needs every item's fields already resolved,
/// forward references included) even runs.
pub fn declare(module: &Module) -> Result<Vec<DeclItem>, SemaError> {
    declare_with_imports(module, &ImportedTypes::new())
}

/// Local (possibly aliased) type name -> that declaration's own
/// generic-parameter count, for every `struct`/`enum` a module imports
/// (plans/M9.md item A1, decision 8). Built by
/// `imports::imported_type_shapes` off raw AST, so it is available before
/// any module's `declare` has run and import cycles stay free.
pub type ImportedTypes = BTreeMap<String, usize>;

/// `declare` for a module that is part of a build closure: identical in
/// every respect except that `imported`'s names join the module's own
/// type-name table, which is the whole of "an imported `struct`/`enum`
/// name is legal wherever a type is legal" (plans/M9.md item A1) — fn
/// parameter, fn return, struct field, `const` type, `let` annotation
/// (through `bodies::ModuleCtx::resolve_type`, whose own table is built
/// from this same input) and generic argument all resolve through the one
/// `resolve_named` below, so there is exactly one place to teach.
///
/// What this deliberately does *not* do is give the imported name a
/// classification: `classify_all` below is module-local and answers
/// `Data` for any name it cannot see, so a local struct holding a field
/// of an imported `resource`/`@actor` type would be misclassified here.
/// `classify_closure` (decision 10) recomputes every module's
/// classification over the whole closure afterwards, and
/// `sema::check_program_typed` runs it before any consumer of a
/// `DeclStruct`/`DeclEnum` exists.
pub fn declare_with_imports(
    module: &Module,
    imported: &ImportedTypes,
) -> Result<Vec<DeclItem>, SemaError> {
    let shapes = build_shapes(module, imported);
    let module_pools = module_pool_names(module);
    let mut items = Vec::new();
    for item in &module.items {
        match item {
            Item::Const(c) => {
                items.push(DeclItem::Const(declare_const(c, &shapes, &module_pools)?))
            }
            Item::Static(s) => {
                items.push(DeclItem::Static(declare_static(s, &shapes, &module_pools)?))
            }
            Item::Fn(f) => items.push(DeclItem::Fn(declare_fn(
                f,
                &shapes,
                &module_pools,
                &BTreeSet::new(),
                &BTreeMap::new(),
            )?)),
            Item::Struct(s) => {
                items.push(DeclItem::Struct(declare_struct(s, &shapes, &module_pools)?))
            }
            Item::Enum(e) => items.push(DeclItem::Enum(declare_enum(e, &shapes, &module_pools)?)),
            Item::Pool(p) => items.push(DeclItem::Pool(p.name.clone())),
            Item::ComptimeIf(_) => {} // comptime evaluation is item C's job
        }
    }
    classify_all(&mut items)?;
    validate_actor_handles(module, &items)?;
    // plans/M7.md item A, decision 3: 03-hardware.md §1's capability rules
    // are the same *shape* as `validate_actor_handles` above, over a
    // different type set — the same post-declare position, the same
    // ast-alongside-`DeclItem` zip, the same "at any nesting" recursion.
    // A second pass rather than a second mechanism.
    validate_capability_types(module, &items)?;
    validate_enum_own_handles(&items)?;
    Ok(items)
}

/// plans/M7.md item I's sweep: an `own[P] T` inside an **enum variant
/// payload** is unreachable by the one rule that governs it, so it fails
/// closed here.
///
/// 02-language.md §4 / 03-hardware.md §3: `own[P] T`'s `T` must be the
/// payload type `P` was bound with, checked by
/// `eval::image_checks::check_pool_decls` (`golden/err-dma-pool-own-mismatch`).
/// That check collects every `own[P] T` in the build closure from
/// `own_handles_in_closure`, which walks consts, fn signatures, **struct**
/// fields/methods/`init`s, and generic instantiations. It cannot walk an
/// enum's payloads for a concrete reason rather than an oversight:
/// `TypedProgram::enums` is `BTreeMap<String, Vec<String>>` — variant
/// *names* only — because an enum has no body to check, so the typed tree
/// this compiler produces carries no enum payload type anywhere. Verified
/// by running: `enum SlotHolder: Held(own[BlockControl] BlkReqStatus)`
/// against `img.dma_pool[BlkReqHeader](name=BlockControl, ...)` — the
/// exact mismatch `err-dma-pool-own-mismatch` rejects in a fn signature —
/// compiled clean and rendered a report.
///
/// Refused at the declaration instead of silently unchecked. `own[P] T`
/// handles are real (plans/M7.md item E4), but enum payload types still do
/// not reach `check_pool_decls`' declaration-surface walk — accepting one
/// here would leave the handle's pool binding unchecked. Nothing in the
/// docs' aspirational driver puts one in an enum today (every `own[P] T` in
/// `docs/language/examples/virtio-storage.wr` is a parameter, a return, a
/// struct field, an `Option` field, or an array element — all walked).
fn validate_enum_own_handles(items: &[DeclItem]) -> Result<(), SemaError> {
    fn own_in(ty: &Type) -> Option<String> {
        match ty {
            Type::Own(..) => Some(render_type(ty)),
            Type::Array(elem, _) => own_in(elem),
            Type::Tuple(elems) => elems.iter().find_map(own_in),
            Type::Static(inner) | Type::Option(inner) => own_in(inner),
            Type::Result(ok, err) => own_in(ok).or_else(|| own_in(err)),
            Type::Fn(params, ret) => params
                .iter()
                .find_map(|(_, t)| own_in(t))
                .or_else(|| own_in(ret)),
            Type::Named(_, targs) => targs.iter().find_map(|a| match a {
                TypeArg::Type(t) => own_in(t),
                _ => None,
            }),
            _ => None,
        }
    }
    for item in items {
        let DeclItem::Enum(e) = item else { continue };
        for (ty, span) in &e.component_types {
            if let Some(found) = own_in(ty) {
                return Err(SemaError::at(
                    "type",
                    format!(
                        "enum `{}` declares `{found}` in a variant payload; a pool handle there \
                         does not reach the pool-binding check (02-language.md §4 / 03-hardware.md \
                         §3: `T` is the payload type the image bound `P` with — checked over fn \
                         signatures, struct fields and generic instantiations, not enum payloads). \
                         Hold it in a struct field instead",
                        e.name
                    ),
                    *span,
                ));
            }
        }
    }
    Ok(())
}

// --- `Actor[T]` validation (plans/M6.md item A) ---------------------------
//
// `Actor[T]` resolves structurally for any `T` (`resolve_named`, above —
// forward references must work, and no struct's own `is_actor` bit is even
// computed until every item in the module has been declared); this pass
// runs once, after every `DeclItem` exists, and rejects any `Actor[T]`
// whose `T` does not name an `@actor`/`@driver` struct (02-language.md
// §9.1: "Other actors hold generated `Actor[T]` handles"). Struct field
// types are already flattened onto `component_types`; a fn/method/init's
// own parameter/return types are not (they are not classification
// components), so this walks the raw `ast::Module` alongside the resolved
// `DeclItem`s (mirroring `bodies::build_module_ctx`'s own zip) for those.

fn validate_actor_type(
    ty: &Type,
    span: Span,
    structs: &BTreeMap<String, &DeclStruct>,
) -> Result<(), SemaError> {
    match ty {
        Type::Named(name, targs) if name == "Actor" => {
            let inner = match targs.first() {
                Some(TypeArg::Type(t)) => t,
                _ => {
                    return Err(SemaError::at(
                        "type",
                        "`Actor` requires a type argument".to_string(),
                        span,
                    ));
                }
            };
            let Type::Named(actor_name, actor_targs) = inner else {
                return Err(SemaError::at(
                    "type",
                    format!(
                        "`Actor[{}]` must name an `@actor`/`@driver` struct",
                        render_type(inner)
                    ),
                    span,
                ));
            };
            // A handle to a *generic instantiation* is outside the M6
            // surface (`flowwir.rs`'s own module doc: "no async generic
            // exists in the M6 surface"), and this is the one place that
            // can say so: every later resolution drops these type
            // arguments on the floor and looks the method up under the
            // generic *base* struct's own name. Accepting the type here
            // is what let `Actor[Box[u64]]` typecheck clean and then hit
            // `flowwir_lower`'s own `internal error: unknown struct
            // `Box`` producer-bug guard — an unimplemented path failing
            // open, and reported as a compiler bug rather than as the
            // scope limit it actually is. Rejected by name instead.
            if !actor_targs.is_empty() {
                return Err(SemaError::at(
                    "actor",
                    format!(
                        "`Actor[{}]` names a generic instantiation; an actor handle to a \
                         generic struct is not implemented (M6 scope)",
                        render_type(inner)
                    ),
                    span,
                ));
            }
            match structs.get(actor_name.as_str()) {
                Some(s) if s.is_actor => Ok(()),
                _ => Err(SemaError::at(
                    "type",
                    format!(
                        "`Actor[{actor_name}]` requires `{actor_name}` to be an \
                         `@actor`/`@driver` struct"
                    ),
                    span,
                )),
            }
        }
        Type::Array(elem, _) => validate_actor_type(elem, span, structs),
        Type::Tuple(elems) => {
            for e in elems {
                validate_actor_type(e, span, structs)?;
            }
            Ok(())
        }
        Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
            validate_actor_type(inner, span, structs)
        }
        Type::Result(ok, err) => {
            validate_actor_type(ok, span, structs)?;
            validate_actor_type(err, span, structs)
        }
        Type::Fn(params, ret) => {
            for (_, t) in params {
                validate_actor_type(t, span, structs)?;
            }
            validate_actor_type(ret, span, structs)
        }
        Type::Named(_, targs) => {
            for a in targs {
                if let TypeArg::Type(t) = a {
                    validate_actor_type(t, span, structs)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_fn_actor_types(
    span: Span,
    params: &[DeclParam],
    ret: &Type,
    structs: &BTreeMap<String, &DeclStruct>,
) -> Result<(), SemaError> {
    for p in params {
        validate_actor_type(&p.ty, span, structs)?;
    }
    validate_actor_type(ret, span, structs)
}

// --- declaration-position message-shape validation (post-review fix) -----
//
// 02-language.md §9.1: "Handles cannot appear in messages, replies, or
// runtime collections." A public method of an `@actor`/`@driver` struct's
// own parameter list *is* the message shape, and its return type *is* the
// reply shape (02 §9.4: calling a public method through `Actor[T]` yields
// an awaitable composing that exact declared type) — a declaration
// carrying `Actor[T]` there can never be legally called or replied to
// (`bodies::check_await_actor_call`/`check_send_call` both already
// require `mf.is_pub`, so the only way to *reach* such a method is
// exactly the message path this clause forbids). Checking this only at
// the call site (`bodies::check_message_args`'s own `Actor[T]`-arg
// rejection) left the *declaration* itself silently acceptable — dead
// surface that only ever errors lazily, the first time anyone dares call
// it. Fail-closed doctrine says reject at declaration; this is that
// fix, run in the same post-declare pass as `validate_actor_handles`
// (`declare`'s own call site).
//
// `init` is exempt: an `init`'s own parameters are the image's wiring
// arguments (`img.actor(A, disk=disk.handle(), ...)`), not a runtime
// message — substituted at build time by `eval::image_checks`'s own
// decl-reference mechanism, never admitted through a mailbox. A
// *non*-`pub` method is exempt too: 02 §9.2's own "calls on self are
// ordinary calls" — only a `pub` method is ever reachable through an
// `Actor[T]` handle at all (`mf.is_pub`, the identical gate the call-site
// checks already enforce), so a private method's parameter list is never
// a message shape; it may freely take/return `Actor[T]` (a same-actor
// helper handed a peer handle already held elsewhere — a field, or a
// public method's own local — `golden/check-actor-private-handle-helper`).
//
// "At any nesting" (array/struct-of-handle counts): `type_contains_actor_handle`
// recurses through every composite shape `validate_actor_type` does, plus
// one more `validate_actor_type` never needed — a *named* type's own
// declared components (`component_types`), so a plain data struct with an
// `Actor[T]` field, passed by value as a message argument, is caught too
// (a `BTreeSet` cycle guard, `seen`, makes this safe against a
// self-referential struct shape — `classify_all`'s own infinite-size
// check already rejects a genuinely infinite one before this ever runs,
// but a merely self-*referential-through-`own`* one is legal data and
// must not infinite-loop this walk).

/// Every declared **struct's *and enum's*** own component types, by name —
/// the one input each composite-containment walk in this file needs
/// (`type_contains_actor_handle`, `type_contains_capability`,
/// `collect_mmio_layouts`).
///
/// Deliberately a *second* map alongside the `BTreeMap<String,
/// &DeclStruct>` those walks used to take. That map answers genuinely
/// struct-shaped questions — `is_actor` for `validate_actor_type`,
/// `layout_kind` for `validate_capability_args` — which an enum has no
/// answer to. But it was silently answering a third, differently shaped
/// question too: *"what does this named type hold?"*, for which an enum's
/// variant payloads are exactly as load-bearing as a struct's fields, and
/// for which `structs.get(name)` returned `None` on every enum.
///
/// plans/M7.md item I's sweep found all three walks failing open through
/// that one word at once: a `DeviceCap[D]`, an `Mmio[L]` or an `Actor[T]`
/// held inside an enum **variant payload** was invisible, so
/// 03-hardware.md §1's `@actor` containment *and* its unforgeability
/// floor, §2's no-alias rule, and 02-language.md §9.1's handle rules each
/// silently admitted the enum-wrapped spelling of the shape they reject
/// directly (`golden/err-cap-actor-enum-field`, `err-cap-enum-return`,
/// `err-mmio-alias-enum`, `err-actor-handle-in-enum-message`).
fn components_by_name(items: &[DeclItem]) -> BTreeMap<String, &[(Type, Span)]> {
    let mut out: BTreeMap<String, &[(Type, Span)]> = BTreeMap::new();
    for item in items {
        match item {
            DeclItem::Struct(s) => {
                out.insert(s.name.clone(), s.component_types.as_slice());
            }
            DeclItem::Enum(e) => {
                out.insert(e.name.clone(), e.component_types.as_slice());
            }
            _ => {}
        }
    }
    out
}

fn type_contains_actor_handle(
    ty: &Type,
    components: &BTreeMap<String, &[(Type, Span)]>,
    seen: &mut BTreeSet<String>,
) -> bool {
    match ty {
        Type::Named(name, _) if name == "Actor" => true,
        Type::Array(elem, _) => type_contains_actor_handle(elem, components, seen),
        Type::Tuple(elems) => elems
            .iter()
            .any(|e| type_contains_actor_handle(e, components, seen)),
        Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
            type_contains_actor_handle(inner, components, seen)
        }
        Type::Result(ok, err) => {
            type_contains_actor_handle(ok, components, seen)
                || type_contains_actor_handle(err, components, seen)
        }
        Type::Fn(params, ret) => {
            params
                .iter()
                .any(|(_, t)| type_contains_actor_handle(t, components, seen))
                || type_contains_actor_handle(ret, components, seen)
        }
        Type::Named(name, targs) => {
            if !seen.insert(name.clone()) {
                return false; // already visited on this path: cycle guard.
            }
            let via_fields = components.get(name.as_str()).is_some_and(|c| {
                c.iter()
                    .any(|(t, _)| type_contains_actor_handle(t, components, seen))
            });
            let via_targs = targs.iter().any(
                |a| matches!(a, TypeArg::Type(t) if type_contains_actor_handle(t, components, seen)),
            );
            seen.remove(name);
            via_fields || via_targs
        }
        _ => false,
    }
}

fn validate_message_shape(
    struct_name: &str,
    method_name: &str,
    span: Span,
    params: &[DeclParam],
    ret: &Type,
    components: &BTreeMap<String, &[(Type, Span)]>,
) -> Result<(), SemaError> {
    for p in params {
        if type_contains_actor_handle(&p.ty, components, &mut BTreeSet::new()) {
            return Err(SemaError::at(
                "actor",
                format!(
                    "an `Actor[T]` handle cannot appear in a message (`{struct_name}.{method_name}`'s \
                     own `{}: {}` — 02-language.md §9.1)",
                    p.name,
                    render_type(&p.ty)
                ),
                span,
            ));
        }
    }
    if type_contains_actor_handle(ret, components, &mut BTreeSet::new()) {
        return Err(SemaError::at(
            "actor",
            format!(
                "an `Actor[T]` handle cannot appear in a reply (`{struct_name}.{method_name}`'s own \
                 return type `{}` — 02-language.md §9.1)",
                render_type(ret)
            ),
            span,
        ));
    }
    // plans/M7.md item I's sweep: `Result[T, never]` is the one declared
    // reply 02-language.md §9.4's composition table cannot round-trip,
    // and it produced a **wrong answer** at M7.
    //
    // The table's two rows collide there. `declared Result[T, E] ->
    // Result[T, CallError[E]]` with `E = never` is character-for-character
    // `declared R -> Result[R, CallError[never]]` with `R = T`, so the
    // composed type carries no evidence of which row made it —
    // `sema::bodies::compose_call_error` is not injective, and
    // `decompose_call_error` (whose own doc claimed the pair was total
    // "for every `t`, both arms") answers `T`.
    //
    // Item Z1's transport then reads the two ends of one `await` through
    // two different predicates, which is exactly where the ambiguity
    // becomes bytes: the *caller* sizes its staging slot from the
    // decomposed declared reply (`codegen::flow_reply_stage_size`), while
    // the *callee*'s dispatch arm decides whether to hand over a staging
    // pointer at all from `codegen::is_aggregate(&f.ret)` on the real
    // declared return (`layout.rs`'s own `reply_is_aggregate`). Verified
    // by running, both ways:
    //
    //   - aggregate `T` — the caller reserves `sizeof(T)` and the callee
    //     writes `sizeof(Result[T, never])`, one tag word more. Every
    //     payload field arrives shifted by a word (a declared
    //     `Ok(Triple(1001, 2002, 3003))` read back as `a=0` (the tag),
    //     `b=1001`, `c=2002`) and the extra word lands past the slot, on
    //     the frame's own `lr` save — masked today only because the
    //     resume path re-saves `lr` on entry. A silent wrong answer.
    //   - scalar `T` — the caller decides the reply is scalar and never
    //     publishes a staging address at all, while the callee's dispatch
    //     arm still loads `[waker + OFF_TURN_REPLY_SLOT]` (never written,
    //     so zero) into `x8` and writes through it. Observed as a real
    //     guest fault at `ipa=0x0`.
    //
    // The docs do not disambiguate the two rows, so this compiler does not
    // get to pick one: 03/02 are normative and a guess here would be a
    // silent language decision. Refused by name at the declaration
    // instead, which is also where every other message/reply shape rule in
    // this fn is enforced (`golden/err-actor-reply-never-error`). A
    // `never` nested any deeper (`Result[T, Option[never]]`) is untouched
    // and correct — only the error type *itself* collides.
    if let Type::Result(_, e) = ret {
        if matches!(**e, Type::Never) {
            return Err(SemaError::at(
                "actor",
                format!(
                    "`{struct_name}.{method_name}` declares the reply `{}`, and \
                     02-language.md §9.4's composition table cannot round-trip it: `declared \
                     Result[T, E] -> Result[T, CallError[E]]` with `E = never` is the same \
                     composed type as `declared T -> Result[T, CallError[never]]`, so a caller \
                     cannot tell which reply it is awaiting. Declare the reply as `{}` if it \
                     never fails, or give the error an inhabited type",
                    render_type(ret),
                    render_type(match ret {
                        Type::Result(t, _) => t,
                        _ => unreachable!(),
                    })
                ),
                span,
            ));
        }
    }
    // No further reply-shape rule beyond the `Actor[T]` one above
    // survives here, and the history is worth keeping because it was a
    // *wrong answer*, not a missing feature.
    //
    // plans/M6.md item H1 rejected EVERY aggregate reply at this point:
    // the turn record carried one scalar word, an aggregate return travels
    // by pointer under this machine's calling convention, and the caller's
    // `Await` composition re-labelled that returned *pointer* as
    // `Ok(<guest address>)` — so a declared `Err` was observed as a
    // success carrying an address.
    //
    // plans/M7.md item Z1 built the transport that rule was waiting on:
    // the awaiting turn's own record carries the address of a caller-owned,
    // statically sized staging slot (`codegen::OFF_TURN_REPLY_SLOT`), the
    // callee's dispatch hands it to the method in `x8`, and the declared
    // reply is written straight into the awaiting frame. Item Z2 then
    // added the one thing transport alone could not give a declared
    // `Result[T, E]`: 02-language.md §9.4 maps its `Err(e)` to
    // `Err(CallError.Op(e))` — a re-*tagging*, not a copy, so the resume
    // stub recomposes it field-wise (`codegen::emit_recompose_staged_result`)
    // instead of copying the staged bytes over. `golden/boot-actor-reply-result`
    // is the flip witness: M6-H1's own `Store.load -> Result[u64, FsError]`
    // program, both arms asserted by value through a real boot, plus a
    // `Result[Triple, FsError]` method whose declared `T` is *wider* than
    // its whole composed payload area — the shape a bulk copy would have
    // corrupted.
    //
    // So the reply rules that remain are exactly the pre-existing ones:
    // no `Actor[T]` at any nesting, in a parameter or a reply.
    Ok(())
}

fn validate_actor_handles(module: &Module, items: &[DeclItem]) -> Result<(), SemaError> {
    let mut structs: BTreeMap<String, &DeclStruct> = BTreeMap::new();
    for item in items {
        if let DeclItem::Struct(s) = item {
            structs.insert(s.name.clone(), s);
        }
    }
    let components = components_by_name(items);
    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();
    for (ai, di) in ast_items.iter().zip(items.iter()) {
        match (ai, di) {
            (Item::Fn(f), DeclItem::Fn(d)) => {
                validate_fn_actor_types(f.span, &d.params, &d.ret, &structs)?;
            }
            (Item::Struct(s), DeclItem::Struct(d)) => {
                for (ty, span) in &d.component_types {
                    validate_actor_type(ty, *span, &structs)?;
                }
                for m in &s.members {
                    match m {
                        Member::Fn(f) => {
                            let Some(DeclMember::Fn(fd)) = d
                                .members
                                .iter()
                                .find(|dm| matches!(dm, DeclMember::Fn(x) if x.name == f.name))
                            else {
                                continue;
                            };
                            validate_fn_actor_types(f.span, &fd.params, &fd.ret, &structs)?;
                            // Post-review fix: a *public* method of an
                            // `@actor`/`@driver` struct is a message
                            // shape — see `validate_message_shape`'s own
                            // doc comment for the full reasoning (init
                            // exempt, non-`pub` methods exempt, checked
                            // here rather than only at the call site).
                            if d.is_actor && f.is_pub && f.receiver.is_some() {
                                validate_message_shape(
                                    &d.name,
                                    &f.name,
                                    f.span,
                                    &fd.params,
                                    &fd.ret,
                                    &components,
                                )?;
                            }
                        }
                        Member::Init(i) => {
                            let Some(DeclMember::Init(id)) = d
                                .members
                                .iter()
                                .find(|dm| matches!(dm, DeclMember::Init(_)))
                            else {
                                continue;
                            };
                            validate_fn_actor_types(i.span, &id.params, &id.ret, &structs)?;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

// --- capability containment + unforgeability (plans/M7.md item A) ---------
//
// 03-hardware.md §1, the three sentences this pass implements:
//
//   "Their constructors are not source-visible: no address, import, or
//    cast creates one."
//   "`@actor` structs cannot hold capabilities in fields, parameters,
//    messages, or captures; a driver may export safe actor APIs but never
//    raw capabilities."
//
// plans/M7.md decision 3 fixes where: "Capabilities are checked where
// `Actor[T]` already is ... Extend that pass; do not build a second
// mechanism." So this runs in `declare`, immediately after
// `validate_actor_handles`, walks the same ast-alongside-`DeclItem` zip,
// and recurses through composites with the same cycle-guarded walk.
//
// **The asymmetry is the rule.** A `@driver` *may* hold capabilities —
// §1's own worked example declares `irq_regs: Mmio[VirtioIrqMmio]` as a
// field and takes `DeviceCap`/`DmaPool` through its `init`. An `@actor`
// may not, anywhere. And neither may export one: a `pub` method of either
// is a message shape (02-language.md §9.4 — its parameters are the
// message and its return is the reply), which is exactly what "a driver
// may export safe actor APIs but never raw capabilities" forbids.
//
// **`init` is not exempt here, unlike the `Actor[T]` rule.** An `init`'s
// parameters are image wiring rather than a message
// (`validate_message_shape`'s own note), which is precisely why a
// *driver*'s `init` is the one place a capability legitimately enters a
// program at all. For an `@actor` that same reasoning inverts: an
// `init` capability parameter is the image handing an actor a capability,
// which is the thing §1 forbids — and, left unchecked, it is *reachable*,
// because `eval::image_checks`' own substitution rule accepts an unwired
// capability parameter for `img.actor(...)` exactly as it does for
// `img.driver(...)`.
//
// **What is NOT checked here, named rather than implied.** "Captures" —
// a closure inside an `@actor` body capturing a capability. With fields
// and parameters both closed there is no expression of capability type an
// actor body can name at all (an actor cannot read one from `self`, cannot
// receive one, and cannot construct one), so no capture can exist to
// check; the arm would be dead code and is deliberately absent rather
// than written and untested. The moment an actor can name a capability,
// this is where the arm goes.

/// The type `ty` carries at any nesting whose *name* satisfies `leaf`,
/// rendered — or `None`. The same walk `type_contains_actor_handle`
/// performs (including its `seen` cycle guard and its recursion through a
/// named type's own declared components, so a plain data struct — or an
/// enum variant payload, `components_by_name` — wrapping the sought type
/// is caught wherever the wrapper appears).
///
/// **The leaf set is a parameter, and the walk is not** (plans/M8.md item
/// D, decision 23). Two rules ask this question over two different sets:
/// containment/unforgeability asks about 03 §1 capabilities plus the other
/// sealed authorities (`contains_capability`), and the messageable-driver
/// message shape asks about those *plus* `InterruptCell[T]`
/// (`driver_message_forbidden_carried`). Sharing the traversal is the
/// whole point — a second copy would be the one that forgets
/// `Option[...]`, or a plain wrapper struct's fields, which is exactly the
/// class of miss item I's sweep already found once
/// (`golden/err-dma-shared-lend-wrapped`).
fn type_carries_named(
    ty: &Type,
    components: &BTreeMap<String, &[(Type, Span)]>,
    seen: &mut BTreeSet<String>,
    leaf: &dyn Fn(&str) -> bool,
) -> Option<String> {
    match ty {
        Type::Named(name, _) if leaf(name) => Some(render_type(ty)),
        // An `Actor[T]` handle is not `T`'s authority (02-language.md §9.1 /
        // 03-hardware.md §1). Recursing into `T` would refuse every
        // `@actor` that holds `Actor[SomeDriver]` — the flagship shape —
        // because the driver's own `Mmio`/`IrqCap` fields would surface
        // here. Same cut as `eval::legal::capability_in_type`.
        Type::Named(name, _) if name == "Actor" => None,
        Type::Array(elem, _) => type_carries_named(elem, components, seen, leaf),
        Type::Tuple(elems) => elems
            .iter()
            .find_map(|e| type_carries_named(e, components, seen, leaf)),
        Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
            type_carries_named(inner, components, seen, leaf)
        }
        Type::Result(ok, err) => type_carries_named(ok, components, seen, leaf)
            .or_else(|| type_carries_named(err, components, seen, leaf)),
        Type::Fn(params, ret) => params
            .iter()
            .find_map(|(_, t)| type_carries_named(t, components, seen, leaf))
            .or_else(|| type_carries_named(ret, components, seen, leaf)),
        Type::Named(name, targs) => {
            if !seen.insert(name.clone()) {
                return None; // already visited on this path: cycle guard.
            }
            let via_fields = components.get(name.as_str()).and_then(|c| {
                c.iter()
                    .find_map(|(t, _)| type_carries_named(t, components, seen, leaf))
            });
            let found = via_fields.or_else(|| {
                targs.iter().find_map(|a| match a {
                    TypeArg::Type(t) => type_carries_named(t, components, seen, leaf),
                    _ => None,
                })
            });
            seen.remove(name);
            found
        }
        _ => None,
    }
}

/// The capability type `ty` contains, at any nesting, rendered — or
/// `None`. The containment/unforgeability leaf set: 03 §1's capabilities,
/// §9's protocol states, §4's sealed queue values, §5's receipt.
fn type_contains_capability(
    ty: &Type,
    components: &BTreeMap<String, &[(Type, Span)]>,
    seen: &mut BTreeSet<String>,
) -> Option<String> {
    type_carries_named(ty, components, seen, &|n| {
        crate::eval::image_checks::is_sealed_authority_type_name(n)
    })
}

fn contains_capability(
    ty: &Type,
    components: &BTreeMap<String, &[(Type, Span)]>,
) -> Option<String> {
    type_contains_capability(ty, components, &mut BTreeSet::new())
}

/// plans/M8.md item D: the sealed authority `ty` carries at any nesting
/// (03 §1 capability, §4 sealed queue value, §9 protocol state, §5
/// receipt), rendered — or `None`. Exactly `contains_capability` above,
/// exported so the image-level messageable-`@driver` check
/// (`layout::check_driver_message_surface`) asks the *identical* question
/// with the identical wrapper/cycle-guarded reach, rather than growing a
/// second walk that could disagree about `Option[DeviceCap]` or a plain
/// struct with a capability field. `items` is the build closure's own
/// `declare` output, which is where the component table comes from.
pub fn sealed_authority_carried(ty: &Type, items: &[DeclItem]) -> Option<String> {
    contains_capability(ty, &components_by_name(items))
}

/// plans/M8.md item D, decision 23: what may not cross a **messageable
/// `@driver`'s mailbox** in either direction — every `sealed_authority_carried`
/// name, plus `InterruptCell[T]`.
///
/// `InterruptCell` is deliberately *not* on the sealed-authority list
/// itself. M7 decision 17 settled that it is a builtin like `Actor[T]`, not
/// a capability: its constructor `InterruptCell(v)` is source-visible, an
/// `@actor` may hold one, and every structural rule that list drives
/// (unforgeability, `@layout` exclusion, protocol consumption, actor
/// containment) would give the wrong answer for it. What is true of it is
/// narrower and belongs exactly here: 03-hardware.md §6 calls it "the
/// **sole** ISR/ordinary-code channel", interrupt-atomic with respect to
/// every vector that may touch the cell — a channel between one driver's
/// ISR and that same driver's ordinary code. A mailbox is a different
/// channel between different principals, and a cell that crosses it is a
/// second, unordered one, carrying the interrupt-status word's value to a
/// sender that owns none of §6's ordering.
pub fn driver_message_forbidden_carried(ty: &Type, items: &[DeclItem]) -> Option<String> {
    type_carries_named(ty, &components_by_name(items), &mut BTreeSet::new(), &|n| {
        crate::eval::image_checks::is_sealed_authority_type_name(n) || n == "InterruptCell"
    })
}

/// 03-hardware.md §1/§2: "`Mmio[L]` — a typed register layout derived from
/// that device", whose §2 example is `Mmio[VirtioIrqMmio]` over an
/// `@layout(mmio)` struct. `L` must therefore name one. Structured exactly
/// like `validate_actor_type` — a whole-module question the per-annotation
/// resolver cannot ask (forward references must work), asked once here.
///
/// The other three capabilities' arguments are deliberately unvalidated:
/// `DeviceCap[D]`'s `D` is a device type, and the device set that names
/// one is 06-machine.md §6's closed stdlib list, which does not exist
/// yet — `img.device[D](...)` accepts any declared struct today, and this
/// pass would have to invent a rule to say otherwise; `IrqCap[V]`'s vector
/// is bound from the image graph by plans/M7.md item G; `DmaPool[P, N]`'s
/// pool identity is item D. Each is left structural rather than given a
/// made-up rule.
fn validate_capability_args(
    ty: &Type,
    span: Span,
    structs: &BTreeMap<String, &DeclStruct>,
) -> Result<(), SemaError> {
    match ty {
        Type::Named(name, targs) if name == "Mmio" => {
            let inner = match targs.first() {
                Some(TypeArg::Type(t)) => t,
                _ => {
                    return Err(SemaError::at(
                        "type",
                        "`Mmio` requires a type argument (03-hardware.md §1)".to_string(),
                        span,
                    ));
                }
            };
            let Type::Named(layout_name, _) = inner else {
                return Err(SemaError::at(
                    "type",
                    format!(
                        "`Mmio[{}]` must name an `@layout(mmio)` struct (03-hardware.md §2)",
                        render_type(inner)
                    ),
                    span,
                ));
            };
            match structs.get(layout_name.as_str()) {
                Some(s) if s.layout_kind == Some(LayoutKind::Mmio) => Ok(()),
                _ => Err(SemaError::at(
                    "type",
                    format!(
                        "`Mmio[{layout_name}]` requires `{layout_name}` to be an \
                         `@layout(mmio)` struct (03-hardware.md §2: a typed register layout)"
                    ),
                    span,
                )),
            }
        }
        // plans/M7.md item D, 03-hardware.md §3: "**Shared control memory**
        // (descriptor tables, rings) is `DmaShared[P, L]`". `L` is the
        // control structure's own layout, and a device reads it, so it is
        // `@layout(dma)` for the same reason a transfer payload's `T` is —
        // exact size, offsets, padding and endianness, reported before
        // anything touches it. The `mmio` kind is a register map, not
        // memory, and `wire` is deliberately target-independent; neither is
        // what a descriptor table is.
        Type::Named(name, targs) if name == "DmaShared" => {
            let inner = match targs.get(1) {
                Some(TypeArg::Type(t)) => t,
                _ => {
                    return Err(SemaError::at(
                        "type",
                        "`DmaShared[P, L]` requires a layout type argument `L` \
                         (03-hardware.md §3)"
                            .to_string(),
                        span,
                    ));
                }
            };
            let Type::Named(layout_name, _) = inner else {
                return Err(SemaError::at(
                    "type",
                    format!(
                        "`DmaShared[..., {}]` must name an `@layout(dma)` struct \
                         (03-hardware.md §3)",
                        render_type(inner)
                    ),
                    span,
                ));
            };
            match structs.get(layout_name.as_str()) {
                Some(s) if s.layout_kind == Some(LayoutKind::Dma) => Ok(()),
                _ => Err(SemaError::at(
                    "type",
                    format!(
                        "`DmaShared[..., {layout_name}]` requires `{layout_name}` to be an \
                         `@layout(dma)` struct (03-hardware.md §3: shared control memory a \
                         device reads, exposed field-wise)"
                    ),
                    span,
                )),
            }
        }
        Type::Array(elem, _) => validate_capability_args(elem, span, structs),
        Type::Tuple(elems) => {
            for e in elems {
                validate_capability_args(e, span, structs)?;
            }
            Ok(())
        }
        Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
            validate_capability_args(inner, span, structs)
        }
        Type::Result(ok, err) => {
            validate_capability_args(ok, span, structs)?;
            validate_capability_args(err, span, structs)
        }
        Type::Fn(params, ret) => {
            for (_, t) in params {
                validate_capability_args(t, span, structs)?;
            }
            validate_capability_args(ret, span, structs)
        }
        Type::Named(_, targs) => {
            for a in targs {
                if let TypeArg::Type(t) = a {
                    validate_capability_args(t, span, structs)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Who a fn belongs to, for the capability rules that turn on it. The
/// three cases 03-hardware.md §1 distinguishes and nothing more.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CapOwner {
    /// A `@driver` struct's member: the one holder §1 permits.
    Driver,
    /// An `@actor` struct's member: "cannot hold capabilities in fields,
    /// parameters, messages, or captures".
    Actor,
    /// A free fn, or a plain (non-actor, non-driver) struct's member.
    /// May *hold* a capability in a parameter — that is how a driver
    /// delegates to a helper — subject to provenance
    /// (`eval::legal::check_provenance`), which is the rule that decides
    /// whether such a fn is reachable through a driver's authority at all.
    Plain,
}

/// Every capability rule that applies to one fn/method/init signature.
///
/// `is_pub_method` is true only for a `pub` method with a receiver — the
/// exact gate `validate_message_shape` uses for the `Actor[T]` rule, and
/// for the same reason: only such a method is reachable through an
/// `Actor[T]` handle, so only such a method's signature is a message
/// shape (02-language.md §9.4).
/// The `DmaShared[P, L]` a type names at any nesting, rendered — or
/// `None`. A sibling of `type_contains_capability` asking one narrower
/// question, because 03-hardware.md §3's rule is `DmaShared`'s alone and
/// not every capability's (a `DeviceCap[D]` parameter lent `read` is
/// ordinary driver code).
/// plans/M7.md item I's sweep: this walk used to stop at a named type,
/// looking only into its *type arguments* — so a `DmaShared[P, L]` held
/// as a **field of a plain wrapper struct** was invisible, and
/// `read bundle: RingBundle` lent exactly the ordinary borrow of shared
/// control memory that `read ring: DmaShared[..]` is rejected for
/// (`golden/err-dma-shared-lend-wrapped`). It recurses through the shared
/// `components_by_name` table now, the same reach
/// `type_contains_capability` has always had for the containment rules —
/// which is where the discrepancy showed: `DmaShared` *is* a capability
/// type, so an `@actor` field or a `pub` method parameter carrying one
/// through a wrapper was already caught; only a `@driver`'s own private
/// method, which containment deliberately permits, reached this rule and
/// found it shallower.
fn dma_shared_in_type(
    ty: &Type,
    components: &BTreeMap<String, &[(Type, Span)]>,
    seen: &mut BTreeSet<String>,
) -> Option<String> {
    match ty {
        Type::Named(name, _) if name == "DmaShared" => Some(render_type(ty)),
        Type::Array(elem, _) => dma_shared_in_type(elem, components, seen),
        Type::Tuple(elems) => elems
            .iter()
            .find_map(|e| dma_shared_in_type(e, components, seen)),
        Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
            dma_shared_in_type(inner, components, seen)
        }
        Type::Result(ok, err) => dma_shared_in_type(ok, components, seen)
            .or_else(|| dma_shared_in_type(err, components, seen)),
        Type::Fn(params, ret) => params
            .iter()
            .find_map(|(_, t)| dma_shared_in_type(t, components, seen))
            .or_else(|| dma_shared_in_type(ret, components, seen)),
        Type::Named(name, targs) => {
            if !seen.insert(name.clone()) {
                return None; // already visited on this path: cycle guard.
            }
            let via_fields = components.get(name.as_str()).and_then(|c| {
                c.iter()
                    .find_map(|(t, _)| dma_shared_in_type(t, components, seen))
            });
            let found = via_fields.or_else(|| {
                targs.iter().find_map(|a| match a {
                    TypeArg::Type(t) => dma_shared_in_type(t, components, seen),
                    _ => None,
                })
            });
            seen.remove(name);
            found
        }
        _ => None,
    }
}

fn validate_fn_capability_types(
    struct_name: Option<&str>,
    fn_name: &str,
    span: Span,
    params: &[DeclParam],
    ret: &Type,
    owner: CapOwner,
    is_pub_method: bool,
    structs: &BTreeMap<String, &DeclStruct>,
    components: &BTreeMap<String, &[(Type, Span)]>,
) -> Result<(), SemaError> {
    let where_ = match struct_name {
        Some(s) => format!("{s}.{fn_name}"),
        None => fn_name.to_string(),
    };
    for p in params {
        validate_capability_args(&p.ty, span, structs)?;
        // 03-hardware.md §3, `DmaShared[P, L]`'s own second sentence:
        // "It cannot be read as bytes or **lent as a plain value**."
        // Shared control memory is permanently shared, and the only
        // sanctioned way to touch it is a field-wise typed operation
        // carrying the target's volatile/cache/ordering semantics — a
        // `read`/`mut` loan hands out an ordinary borrow that carries
        // none of that, which is exactly the thing the sentence forbids.
        // `take` is untouched: moving the handle (into a driver's own
        // field, or through a queue constructor) is how it gets anywhere
        // at all. This never blocks the field-wise operations themselves
        // — those are methods on the builtin type, which no source can
        // declare.
        if p.mode != AccessMode::Take {
            if let Some(found) = dma_shared_in_type(&p.ty, components, &mut BTreeSet::new()) {
                // The parameter's own declared type, plus what it carries
                // when the two differ — a wrapper struct's name alone
                // would not say why it is refused, and the capability's
                // name alone would not match anything the reader wrote.
                let declared = render_type(&p.ty);
                let carries = if declared == found {
                    String::new()
                } else {
                    format!(", which carries `{found}`")
                };
                return Err(SemaError::at(
                    "type",
                    format!(
                        "`{where_}` lends `{}: {declared}`{carries} — 03-hardware.md §3: shared \
                         control memory \"cannot be read as bytes or lent as a plain value\", it \
                         exposes only field-wise typed operations that carry the target's \
                         volatile/cache/ordering semantics. Move it with `take` instead",
                        p.name
                    ),
                    span,
                ));
            }
        }
        let Some(found) = contains_capability(&p.ty, components) else {
            continue;
        };
        if is_pub_method && owner != CapOwner::Plain {
            return Err(SemaError::at(
                "type",
                format!(
                    "`{where_}` is a `pub` method of an `@actor`/`@driver` struct, so its \
                     parameters are a message shape — a capability cannot appear there \
                     (`{}: {found}`; 03-hardware.md §1: a driver may export safe actor APIs but \
                     never raw capabilities)",
                    p.name
                ),
                span,
            ));
        }
        if owner == CapOwner::Actor {
            return Err(SemaError::at(
                "type",
                format!(
                    "`@actor` struct `{}` cannot hold a capability in a parameter \
                     (`{where_}`'s own `{}: {found}` — 03-hardware.md §1)",
                    struct_name.unwrap_or_default(),
                    p.name
                ),
                span,
            ));
        }
    }
    validate_capability_args(ret, span, structs)?;
    let Some(found) = contains_capability(ret, components) else {
        return Ok(());
    };
    // plans/M7.md item E3: `Receipt[P]` is minted only by `publish` /
    // `reject` / the handoff admission commit — returning one is how a
    // handoff method transfers the caller endpoint, and 03-hardware.md §5
    // blesses that shape *by name* on a public driver method ("any public
    // synchronous `@driver` method with exactly one `take p: P` parameter
    // and result `Receipt[P]`"). So this arm must run before the
    // pub-method rejection below, or every handoff signature is illegally
    // rejected as "raw capabilities".
    //
    // plans/M8.md item D narrowed this list. `QueuePermit` / `QueueOp`
    // (M7 item E2) are minted by `reserve_proven` / `prepare_block` and
    // must reach `prepare_block` / `publish` — which is a *private*
    // driver-internal handoff, and `is_pub_method` is exactly the gate
    // that distinguishes the two. Whitelisting them ahead of the
    // pub-method arm let a `pub` driver method declare a sealed queue
    // value as its reply; harmless while no driver could be messaged, and
    // a laundering channel the moment one can be (item D). 03 §5 names no
    // public convention for either name, so neither gets one here: they
    // fall through to the ordinary rules below, which permit them on a
    // private method (`CapOwner::Plain`/`Driver`, `is_pub_method` false)
    // and on a free helper, and refuse them as an exported reply.
    if found.starts_with("Receipt[") || found == "Receipt" {
        return Ok(());
    }
    if (found.starts_with("QueuePermit") || found.starts_with("QueueOp"))
        && !(is_pub_method && owner != CapOwner::Plain)
    {
        return Ok(());
    }
    if is_pub_method && owner != CapOwner::Plain {
        return Err(SemaError::at(
            "type",
            format!(
                "`{where_}` is a `pub` method of an `@actor`/`@driver` struct and returns \
                 `{found}` — 03-hardware.md §1: a driver may export safe actor APIs but never \
                 raw capabilities"
            ),
            span,
        ));
    }
    // The general unforgeability arm. Nothing in the source language can
    // *produce* a capability (03-hardware.md §1: "their constructors are
    // not source-visible"), so a signature claiming to return one is
    // either unimplementable or laundering a capability it received —
    // and either way it is the source-visible constructor the sentence
    // forbids. This is the floor, and it is deliberately wider than any
    // rule §1 spells out: the day a real minting operation exists in the
    // language (item C partitions an `Mmio[L]` out of a claim; item D
    // sub-allocates a pool), it is this arm that has to learn about it.
    Err(SemaError::at(
        "type",
        format!(
            "`{where_}` declares the return type `{found}`, but a capability's constructor is \
             not source-visible (03-hardware.md §1) — no function can produce one, so none may \
             claim to return one"
        ),
        span,
    ))
}

fn validate_capability_types(module: &Module, items: &[DeclItem]) -> Result<(), SemaError> {
    let mut structs: BTreeMap<String, &DeclStruct> = BTreeMap::new();
    for item in items {
        if let DeclItem::Struct(s) = item {
            structs.insert(s.name.clone(), s);
        }
    }
    let components = components_by_name(items);
    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();
    for (ai, di) in ast_items.iter().zip(items.iter()) {
        match (ai, di) {
            // A module `const`'s declared type. A `const` is a
            // comptime-evaluated value (02-language.md §12), and no
            // comptime evaluation can produce a capability — a `const`
            // typed as one is a constructor claim with nothing behind it.
            (Item::Const(c), DeclItem::Const(d)) => {
                validate_capability_args(&d.ty, c.span, &structs)?;
                if let Some(found) = contains_capability(&d.ty, &components) {
                    return Err(SemaError::at(
                        "type",
                        format!(
                            "`const {}` is declared `{found}`, but a capability's constructor is \
                             not source-visible (03-hardware.md §1) — no comptime value is one",
                            d.name
                        ),
                        c.span,
                    ));
                }
            }
            (Item::Fn(f), DeclItem::Fn(d)) => {
                validate_fn_capability_types(
                    None,
                    &d.name,
                    f.span,
                    &d.params,
                    &d.ret,
                    CapOwner::Plain,
                    false,
                    &structs,
                    &components,
                )?;
            }
            (Item::Struct(s), DeclItem::Struct(d)) => {
                let owner = if d.is_driver {
                    CapOwner::Driver
                } else if d.is_actor {
                    CapOwner::Actor
                } else {
                    CapOwner::Plain
                };
                for (ty, span) in &d.component_types {
                    validate_capability_args(ty, *span, &structs)?;
                    if owner != CapOwner::Actor {
                        continue;
                    }
                    if let Some(found) = contains_capability(ty, &components) {
                        return Err(SemaError::at(
                            "type",
                            format!(
                                "`@actor` struct `{}` cannot hold a capability in a field \
                                 (`{found}` — 03-hardware.md §1); only a `@driver` may",
                                d.name
                            ),
                            *span,
                        ));
                    }
                }
                for m in &s.members {
                    match m {
                        Member::Fn(f) => {
                            let Some(DeclMember::Fn(fd)) = d
                                .members
                                .iter()
                                .find(|dm| matches!(dm, DeclMember::Fn(x) if x.name == f.name))
                            else {
                                continue;
                            };
                            validate_fn_capability_types(
                                Some(&d.name),
                                &f.name,
                                f.span,
                                &fd.params,
                                &fd.ret,
                                owner,
                                f.is_pub && f.receiver.is_some(),
                                &structs,
                                &components,
                            )?;
                        }
                        Member::Init(i) => {
                            let Some(DeclMember::Init(id)) = d
                                .members
                                .iter()
                                .find(|dm| matches!(dm, DeclMember::Init(_)))
                            else {
                                continue;
                            };
                            // Never a `pub` method: an `init` has no
                            // message shape at all (it is image wiring),
                            // which is exactly what makes a *driver*'s
                            // `init` the one legitimate entry point for a
                            // capability into a program.
                            validate_fn_capability_types(
                                Some(&d.name),
                                "init",
                                i.span,
                                &id.params,
                                &id.ret,
                                owner,
                                false,
                                &structs,
                                &components,
                            )?;
                        }
                        _ => {}
                    }
                }
            }
            (Item::Enum(_), DeclItem::Enum(e)) => {
                // An enum variant's payload is ordinary data composition:
                // a capability inside one would be a capability held by
                // whatever holds the enum, and constructing that variant
                // would be constructing a capability container. The
                // *field*-level rules above already catch it wherever the
                // enum is held by an actor; this catches the declaration
                // itself, which is where it is legible.
                for (ty, span) in &e.component_types {
                    validate_capability_args(ty, *span, &structs)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Everything `eval::legal::check_provenance` needs from this pass, and
/// nothing it could derive on its own (plans/M7.md item A, decision 3).
///
/// 03-hardware.md §1's provenance sentence — "a function that touches
/// MMIO, DMA, or IRQ state must be reachable through the owning driver's
/// authority" — is whole-graph reachability over the typed callee graph,
/// which is `eval::legal`'s own shape. But three of its inputs are
/// *declaration* facts the typed tree does not carry: which struct is a
/// `@driver`, where each fn was declared (a typed node has no span at
/// all — `typed.rs` decision 1 drops them), and which named types carry a
/// capability at any nesting. All three are computed here, by the walk
/// this file already maintains, and handed over as data.
pub struct CapabilityAuthority {
    /// Every `Struct.member` key belonging to a `@driver` — the roots of
    /// "the owning driver's authority".
    pub roots: BTreeSet<String>,
    /// Each classifiable key's own declaration span, for the diagnostic.
    /// A key with no entry (a generic instantiation, which is synthesized
    /// and has no source location of its own) gets a location-free
    /// diagnostic rather than an invented `0:0`.
    pub spans: BTreeMap<String, Span>,
    /// Every declared struct/enum name whose type carries a capability at
    /// any nesting — the *flattened* answer, so the reachability walk can
    /// ask "does this type touch hardware state" with a set lookup
    /// instead of a second copy of `type_contains_capability`'s recursion
    /// through struct fields. One walk, in the file that owns it.
    pub capability_bearing: BTreeSet<String>,
}

pub fn capability_authority(module: &Module, items: &[DeclItem]) -> CapabilityAuthority {
    let components = components_by_name(items);
    let mut roots = BTreeSet::new();
    let mut capability_bearing = BTreeSet::new();
    for item in items {
        match item {
            DeclItem::Struct(s) => {
                if contains_capability(&Type::Named(s.name.clone(), Vec::new()), &components)
                    .is_some()
                {
                    capability_bearing.insert(s.name.clone());
                }
                if !s.is_driver {
                    continue;
                }
                for m in &s.members {
                    match m {
                        DeclMember::Fn(f) => {
                            roots.insert(format!("{}.{}", s.name, f.name));
                        }
                        DeclMember::Init(_) => {
                            roots.insert(format!("{}.init", s.name));
                        }
                        _ => {}
                    }
                }
            }
            DeclItem::Enum(e) => {
                // Item I's sweep: this arm was written but could never
                // fire — `contains_capability` consulted a struct-only
                // map, so `structs.get(<enum name>)` was always `None`
                // and no enum ever entered `capability_bearing`. A fn
                // taking a capability-bearing enum was therefore invisible
                // to `eval::legal::check_provenance` — 03-hardware.md §1's
                // provenance sentence failing open through the same hole
                // `components_by_name`'s own doc comment describes.
                if contains_capability(&Type::Named(e.name.clone(), Vec::new()), &components)
                    .is_some()
                {
                    capability_bearing.insert(e.name.clone());
                }
            }
            _ => {}
        }
    }
    // Spans, from the raw ast alongside the same items — `DeclFn` carries
    // no span of its own (only `DeclStruct`/`DeclEnum` do), so this is the
    // one place a fn's declaration location is available at all.
    let mut spans = BTreeMap::new();
    for item in &module.items {
        match item {
            Item::Fn(f) => {
                spans.insert(f.name.clone(), f.span);
            }
            Item::Struct(s) => {
                for m in &s.members {
                    match m {
                        Member::Fn(f) => {
                            spans.insert(format!("{}.{}", s.name, f.name), f.span);
                        }
                        Member::Init(i) => {
                            spans.insert(format!("{}.init", s.name), i.span);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    CapabilityAuthority {
        roots,
        spans,
        capability_bearing,
    }
}

// --- `@layout` exact bytes (plans/M7.md item B, 03-hardware.md §2/§3) -----
//
// 03-hardware.md §3, the whole rule this pass implements: "`@layout(kind,
// ...)` is the one exact-bytes mechanism, with four kinds: `dma`
// (device-visible memory, checked against the target ABI), `mmio`
// (register maps, §2), `wire` (persistent/network bytes — exact
// encoding independent of any target, no capabilities or target-dependent
// fields inside), and `runtime` (the machine's own tables, §3.1). For
// every `@layout` type the compiler reports exact
// size, offsets, padding, and endianness, and rejects anything implicit or
// target-dependent." plans/M7.md decision 4 fixes the shape: "reports
// exact bytes or fails. No implicit padding, no target-dependent field,
// no inference."
//
// **Pass order (decided here, plans/M7.md item B).** This runs *before*
// `symbols::resolve`, i.e. before name resolution, unlike every other
// check in this file. Two reasons, both load-bearing:
//
//   1. A `@layout` field's type is not an ordinary annotation — it is an
//      encoding, drawn from a closed set of exact-width scalars (plus §2's
//      `ReadOnly`/`WriteOnly` register wrappers). Nothing about it is
//      name-resolution-dependent, so inside a `@layout` struct an
//      unknown name is not "unknown", it is "not an exact-bytes type".
//   2. 03 §3 forbids a capability type inside a `wire` layout by name, and
//      no capability type exists yet (plans/M7.md item A mints them).
//      Checked after resolution, that rule would be dead code today and
//      would report `error[name]: unknown name \`DmaPool\`` — a diagnostic
//      naming the wrong cause. Checked here, the rule is live now and
//      keeps producing the better diagnostic once item A lands.
//
// Everything below therefore reads raw `ast` types (rendered with
// `printer::print_type_bare`), never a resolved `types::Type`.
//
// **Sizes here are encoding sizes, not machine sizes.** `mwir::size_of`
// answers "how many bytes does the machine give this value" (one 8-byte
// slot per scalar); this answers "how many bytes does this field occupy on
// the wire / in the register map". The two deliberately disagree, which is
// exactly why `@layout` needs its own table rather than reusing that one.

/// 03-hardware.md §3's four layout kinds.
///
/// `Runtime` (03-hardware.md §3.1) is the fourth kind, live since
/// plans/M10.md item A2: it is the only kind whose field may be a nested
/// `@layout(runtime)` struct or a fixed-length array of one, and the
/// nesting is exclusive in both directions — a `runtime` field is never a
/// `dma`/`mmio`/`wire` layout, and none of those three nests a `runtime`
/// one (`nested_layout_kind_error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    Dma,
    Mmio,
    Wire,
    Runtime,
}

impl LayoutKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LayoutKind::Dma => "dma",
            LayoutKind::Mmio => "mmio",
            LayoutKind::Wire => "wire",
            LayoutKind::Runtime => "runtime",
        }
    }
}

/// A `@layout`'s declared byte order. Never inferred and never defaulted
/// (plans/M7.md decision 4: "no inference") — `endian=` is required on
/// every `@layout`, whatever its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutEndian {
    Little,
    Big,
}

impl LayoutEndian {
    pub fn as_str(self) -> &'static str {
        match self {
            LayoutEndian::Little => "little",
            LayoutEndian::Big => "big",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutField {
    pub name: String,
    /// The field's declared type, spelled exactly as source wrote it.
    pub ty: String,
    pub offset: u64,
    pub size: u64,
}

/// One entry of a laid-out `@layout` type, in ascending offset order
/// (which is also declaration order — the pass requires the two agree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutEntry {
    Field(LayoutField),
    /// A *declared* hole: bytes no field covers. The only way to create
    /// one is an explicit `@offset(...)` that skips ahead, which is why
    /// this is reported rather than rejected — the padding a `@layout`
    /// rejects is the padding the compiler would have to *invent*
    /// (`implicit_padding_error`, below).
    Padding {
        offset: u64,
        size: u64,
    },
}

/// One `@layout` type, fully laid out: 03 §3's "exact size, offsets,
/// padding, and endianness", as data.
///
/// **Or not yet laid out.** Since plans/M10.md item A2b a `runtime` layout
/// whose array length is a `const` name (03 §3.1's own `[TurnArea;
/// N_TURNS]`) leaves `check_layouts` *deferred*: `size` is `None`, `padding`
/// is 0 and `entries` is empty, because the early pass evaluates nothing and
/// therefore has no offsets to report. `complete_layouts` fills all three in
/// after const evaluation. `None` is the whole point of the `Option`: every
/// consumer that needs a byte count must refuse it by name (`require_size`)
/// rather than read a plausible-looking 0 — a zero-byte `@layout` is exactly
/// the fail-open 03 §3 exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutType {
    pub name: String,
    pub kind: LayoutKind,
    pub endian: LayoutEndian,
    /// Total bytes: the end of the last field. There is no trailing
    /// padding and no alignment round-up — a `@layout` type is exactly
    /// the bytes its fields cover. `None` while this layout's sizing is
    /// deferred (see the type's own note).
    pub size: Option<u64>,
    /// Total declared-hole bytes (the sum of every `Padding` entry).
    pub padding: u64,
    pub entries: Vec<LayoutEntry>,
}

impl LayoutType {
    /// This layout's total bytes, or the named fail-closed rejection that
    /// says it has none yet (plans/M10.md item A2b, requirement 4: "a
    /// deferred layout that never got completed must not silently report a
    /// wrong or absent size"). `context` names the consumer, so a reader
    /// learns which pass reached an uncompleted layout, not merely that one
    /// did.
    ///
    /// Location-free (`omit_location`): the failure is a pass-order fact
    /// about a whole layout, not a fact about one source position, and a
    /// `0:0` would be a worse answer than none.
    pub fn require_size(&self, context: &str) -> Result<u64, SemaError> {
        match self.size {
            Some(size) => Ok(size),
            None => Err(SemaError {
                category: "type",
                message: format!(
                    "`@layout` type `{}` has no computed size at {context}: its array length is a \
                     `const` name, so `sema::types::check_layouts` deferred its sizing and \
                     `complete_layouts` (which resolves the length after const evaluation) never \
                     ran on it (03-hardware.md §3.1, plans/M10.md item A2b)",
                    self.name
                ),
                line: 0,
                col: 0,
                extra_lines: Vec::new(),
                omit_location: true,
                missing_method: None,
            }),
        }
    }
}

/// The exact-width integer field types (03 §3's "exact bytes"). Deliberately
/// *not* a superset:
///
/// - `usize`/`isize` are target-dependent by definition;
/// - `f32`/`f64` are target-dependent too — 02-language.md §6.1 has them
///   only "where the target enables them";
/// - `bool`/`char` have no byte encoding pinned anywhere in the docs, so
///   this compiler cannot report an exact one for them without inventing
///   it.
fn scalar_field_size(name: &str) -> Option<u64> {
    match name {
        "u8" | "i8" => Some(1),
        "u16" | "i16" => Some(2),
        "u32" | "i32" => Some(4),
        "u64" | "i64" => Some(8),
        _ => None,
    }
}

/// 03-hardware.md §2's register wrappers.
const MMIO_WRAPPERS: &[&str] = &["ReadOnly", "WriteOnly"];

fn layout_error(message: String, span: Span) -> SemaError {
    // Category `type` (a bad declaration shape), the same category
    // `bodies::check_marker_attr_shape` uses for a malformed `@test`:
    // `xtask`'s `SEMA_CATEGORIES` is a fixed set (plans/M2.md decision 1)
    // and this item does not extend it.
    SemaError::at("type", message, span)
}

/// The `@layout` attribute's own shape: `@layout(<kind>, endian=<order>)`.
/// Nothing else is accepted — an unrecognized argument is a real rejection
/// rather than a silently ignored one, because every `@layout` argument
/// that exists changes the reported bytes.
fn parse_layout_attr(
    struct_name: &str,
    attr: &Attr,
) -> Result<(LayoutKind, LayoutEndian), SemaError> {
    let mut kind = None;
    let mut endian = None;
    for (i, arg) in attr.args.iter().enumerate() {
        match &arg.label {
            None => {
                if i != 0 || kind.is_some() {
                    return Err(layout_error(
                        format!(
                            "`@layout` on struct `{struct_name}` takes one positional argument \
                             (its kind); `{}` is a second one",
                            printer::print_expr_bare(&arg.value)
                        ),
                        arg.span,
                    ));
                }
                let Expr::Name(_, name) = &arg.value else {
                    return Err(layout_error(
                        format!(
                            "`@layout`'s kind on struct `{struct_name}` must be the bare name \
                             `dma`, `mmio`, `wire`, or `runtime` (03-hardware.md §3)"
                        ),
                        arg.span,
                    ));
                };
                kind = Some(match name.as_str() {
                    "dma" => LayoutKind::Dma,
                    "mmio" => LayoutKind::Mmio,
                    "wire" => LayoutKind::Wire,
                    "runtime" => LayoutKind::Runtime,
                    other => {
                        return Err(layout_error(
                            format!(
                                "unknown `@layout` kind `{other}` on struct `{struct_name}`; the \
                                 four kinds are `dma`, `mmio`, `wire`, and `runtime` \
                                 (03-hardware.md §3)"
                            ),
                            arg.span,
                        ));
                    }
                });
            }
            Some(label) if label == "endian" => {
                if endian.is_some() {
                    return Err(layout_error(
                        format!("`@layout` on struct `{struct_name}` declares `endian=` twice"),
                        arg.span,
                    ));
                }
                let Expr::Name(_, name) = &arg.value else {
                    return Err(layout_error(
                        format!(
                            "`@layout`'s `endian=` on struct `{struct_name}` must be the bare \
                             name `little` or `big` (03-hardware.md §3)"
                        ),
                        arg.span,
                    ));
                };
                endian = Some(match name.as_str() {
                    "little" => LayoutEndian::Little,
                    "big" => LayoutEndian::Big,
                    other => {
                        return Err(layout_error(
                            format!(
                                "`@layout`'s `endian=` on struct `{struct_name}` must be `little` \
                                 or `big`, found `{other}` (03-hardware.md §3)"
                            ),
                            arg.span,
                        ));
                    }
                });
            }
            Some(label) => {
                return Err(layout_error(
                    format!(
                        "unknown `@layout` argument `{label}=` on struct `{struct_name}`; \
                         `@layout` takes its kind plus `endian=` and nothing else — every \
                         argument that exists changes the reported bytes (03-hardware.md §3)"
                    ),
                    arg.span,
                ));
            }
        }
    }
    let Some(kind) = kind else {
        return Err(layout_error(
            format!(
                "`@layout` on struct `{struct_name}` names no kind; write its kind first, one of \
                 `dma`, `mmio`, `wire`, or `runtime` (03-hardware.md §3)"
            ),
            attr.span,
        ));
    };
    let Some(endian) = endian else {
        return Err(layout_error(
            format!(
                "`@layout({}, ...)` on struct `{struct_name}` declares no `endian=`; a `@layout` \
                 type's byte order is never inferred — write `endian=little` or `endian=big` \
                 (03-hardware.md §3)",
                kind.as_str()
            ),
            attr.span,
        ));
    };
    Ok((kind, endian))
}

/// `@offset(n)`'s own shape: exactly one positional integer literal. The
/// value is decoded with `bodies::parse_int_literal` (the same decoder
/// every other integer literal in this compiler goes through), never
/// evaluated — a `const`-named offset is inference by another name and is
/// rejected here.
fn parse_offset_attr(struct_name: &str, field_name: &str, attr: &Attr) -> Result<u64, SemaError> {
    let bad = || {
        layout_error(
            format!(
                "`@offset` on field `{struct_name}.{field_name}` takes exactly one integer \
                 literal (e.g. `@offset(0x060)`)"
            ),
            attr.span,
        )
    };
    let [arg] = attr.args.as_slice() else {
        return Err(bad());
    };
    if arg.label.is_some() {
        return Err(bad());
    }
    let Expr::Int(_, text) = &arg.value else {
        return Err(bad());
    };
    let value = super::bodies::parse_int_literal(text).ok_or_else(bad)?;
    u64::try_from(value).map_err(|_| bad())
}

/// Every `@layout` struct declared in one module, by name. `layout_field_bytes`
/// needs the *declaration*, not just the name: 03 §3.1's nested `runtime`
/// field is sized by laying the nested struct out, recursively.
type LayoutDecls<'a> = BTreeMap<String, &'a StructItem>;

/// A `@layout` field's exact bytes: its size, and the alignment its own
/// offset is checked against.
///
/// The two are the same number for every field of a `dma`/`mmio`/`wire`
/// layout — those are sized integers and register wrappers, whose natural
/// alignment *is* their width — which is why this pass carried only a size
/// until plans/M10.md item A2. §3.1's two new field shapes separate them:
/// `[TurnArea; 4]` is 32 bytes wide and 4-byte aligned, and a nested
/// struct's alignment is the widest alignment among its own fields, not its
/// total size. Nothing rounds a size *up* to an alignment anywhere — 03 §3
/// is explicit that a `@layout` type is exactly the bytes its fields cover,
/// with no trailing padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FieldBytes {
    size: u64,
    align: u64,
}

impl FieldBytes {
    /// A sized integer or a register wrapper: width and alignment agree.
    fn scalar(size: u64) -> Self {
        FieldBytes { size, align: size }
    }
}

/// How deep one `@layout(runtime)` type may nest another. A `runtime`
/// layout is a table of tables — the deepest shape 03 §3.1 describes is
/// two levels (`TurnTable` → `[TurnArea; N]`) — so this is generous by an
/// order of magnitude and exists only as a floor: cycle detection already
/// makes the recursion finite, but "finite" is not "small", and a module
/// declaring a thousand-long chain of nested layouts would otherwise
/// recurse a thousand frames deep inside a compiler pass. Fail closed with
/// a named diagnostic instead of trusting the process stack.
const MAX_LAYOUT_NEST_DEPTH: usize = 16;

/// How many nested layouts one top-level `@layout` may expand, in total.
///
/// The depth cap alone does not bound the *work*: a nested type is laid out
/// from scratch at every mention (this pass keeps no cache — it is a pure
/// function of the ast), so a chain of 16 structs each naming the next four
/// times expands `4^16` layouts from eighty lines of source. That is not a
/// wrong answer, it is no answer at all — the pass would appear to hang, and
/// the fuzzer's `sema` lane reaches `check_layouts` on every iteration.
/// Bounded by counting expansions rather than by memoizing them: a cache is
/// the clever fix and needs a profile to buy (ROADMAP.md's cleverness
/// budget), while a budget is the dumb one and fails closed. 1024 is three
/// orders of magnitude above any table 03 §3.1 describes.
const MAX_LAYOUT_NEST_EXPANSIONS: u32 = 1024;

/// The largest number of bytes one `@layout` type may cover.
///
/// A fail-closed floor, exactly like the two nesting bounds above, and it
/// exists for the same reason: since plans/M10.md item A2b an array length
/// can be a `const`, so a one-line edit to a `const` turns a four-element
/// table into a `2^40`-element one, and every number downstream of a
/// `@layout` size (a DMA pool's backing bytes, a placed table's extent) is a
/// real allocation. The flagship machine is 1 GiB in total
/// (ROADMAP.md/06-machine.md), so a *single* exact-bytes declaration
/// claiming more than 16 MiB is a mistake in the declaration and not a
/// table; refused by name rather than reported as a size nothing can hold.
/// Raise it in the item that has an image needing more — never silently.
const MAX_LAYOUT_BYTES: u64 = 16 * 1024 * 1024;

/// The array lengths the *completion* pass resolved: `const` name -> value,
/// already checked to be a positive integer (`collect_length_consts`). The
/// early pass carries `None` here and defers instead — decision 580's purity
/// (it runs before name resolution and evaluates nothing) is unchanged, and
/// this table is the whole difference between the two passes.
type LengthConsts = BTreeMap<String, u64>;

/// The nesting recursion's whole state: the chain of layout structs
/// currently being laid out, outermost first (cycle detector and depth
/// counter), and the remaining expansion budget. One per top-level
/// `@layout` — `check_one_layout` builds it fresh, so no layout's budget is
/// spent by its siblings.
struct NestCtx<'a> {
    stack: Vec<String>,
    budget: u32,
    /// `None` in the early pass (`check_layouts`): a `const`-named array
    /// length has no value here and the layout is deferred. `Some(table)` in
    /// the later pass (`complete_layouts`): every length resolves or is a
    /// named rejection.
    lens: Option<&'a LengthConsts>,
}

/// A `@layout` field's exact bytes, or the named rejection that says why it
/// has none. `decls` is every `@layout` struct declared in this module, so
/// a nested one is sized (03 §3.1's `runtime` allowance) or rejected as the
/// scope limit it is, rather than as an unsized type it is not.
///
/// `Ok(None)` is the third outcome plans/M10.md item A2b adds: **deferred** —
/// this field is (or nests) an array whose length is a `const` name, which
/// the early pass may not evaluate. Every *other* rule about the field has
/// already been checked when this returns; only its byte count is unknown,
/// and `complete_layouts` is what supplies it.
fn layout_field_bytes(
    struct_name: &str,
    field_name: &str,
    kind: LayoutKind,
    ty: &ast::Type,
    decls: &LayoutDecls,
    nest: &mut NestCtx<'_>,
    span: Span,
) -> Result<Option<FieldBytes>, SemaError> {
    let rendered = printer::print_type_bare(ty);
    if let ast::Type::Array(a) = ty {
        return array_field_bytes(
            struct_name,
            field_name,
            kind,
            a,
            &rendered,
            decls,
            nest,
            span,
        );
    }
    let ast::Type::Named(n) = ty else {
        return Err(no_exact_size_error(
            struct_name,
            field_name,
            &rendered,
            kind,
            span,
        ));
    };
    if n.args.is_empty() {
        if let Some(size) = scalar_field_size(&n.name) {
            return Ok(Some(FieldBytes::scalar(size)));
        }
        if matches!(n.name.as_str(), "usize" | "isize") {
            return Err(layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` has a target-dependent \
                     width; a `@layout` type's bytes are exact on every target — use a sized \
                     integer (`u8`/`u16`/`u32`/`u64`, or their signed forms) \
                     (03-hardware.md §3)"
                ),
                span,
            ));
        }
        if matches!(n.name.as_str(), "f32" | "f64") {
            return Err(layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` is target-dependent: \
                     02-language.md §6.1 has `f32`/`f64` only \"where the target enables them\", \
                     and a `@layout` type's bytes are exact on every target (03-hardware.md §3)"
                ),
                span,
            ));
        }
        if let Some(nested) = decls.get(&n.name) {
            return nested_field_bytes(
                struct_name,
                field_name,
                kind,
                nested,
                &rendered,
                decls,
                nest,
                span,
            );
        }
    }
    // plans/M7.md item D self-audit finding: this pass used to carry its
    // own second copy of the capability name list, which item A's own
    // "one list, in one place — several copies could disagree; one
    // cannot" note had already ruled out. It consults the shared list
    // now, which is also how `DmaShared[P, L]` (03 §3's shared control
    // memory, no byte encoding of its own) became covered here with no
    // further code. This pass runs before name resolution, so a plain
    // name check is all it can do and all it needs.
    if n.name == "DmaShared" {
        // 03-hardware.md §3, `DmaShared[P, L]`'s own second sentence:
        // "It **cannot be read as bytes** or lent as a plain value."
        // A `@layout` field is precisely a byte view — declaring one as
        // `DmaShared[P, L]` is asking the compiler to say which bytes it
        // is, which is the thing the sentence rules out. This is a
        // permanent rule, not a fail-closed floor: no later item makes
        // shared control memory describable as bytes.
        return Err(layout_error(
            format!(
                "field `{struct_name}.{field_name}: {rendered}` is shared control memory; \
                 03-hardware.md §3: it \"cannot be read as bytes or lent as a plain value\", and \
                 a `@layout` field is exactly a byte view. Name the control structure's own \
                 `@layout(dma)` type as `L` instead"
            ),
            span,
        ));
    }
    if crate::eval::image_checks::is_sealed_authority_type_name(&n.name) {
        let kind_text = crate::eval::image_checks::sealed_authority_kind(&n.name);
        return Err(match kind {
            LayoutKind::Wire => layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` is {kind_text}; a `wire` \
                     layout is exact bytes independent of any target and can hold no \
                     capability (03-hardware.md §3)"
                ),
                span,
            ),
            // `Runtime` joins them for the same basic reason, and 03 §3.1
            // says it by name too ("carries no capability").
            LayoutKind::Dma | LayoutKind::Mmio | LayoutKind::Runtime => layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` is {kind_text}; it has no \
                     byte encoding, so a `@layout` type cannot hold one (03-hardware.md §3)"
                ),
                span,
            ),
        });
    }
    if MMIO_WRAPPERS.contains(&n.name.as_str()) {
        if kind != LayoutKind::Mmio {
            return Err(layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` wraps a register, but \
                     `{struct_name}` is a `@layout({})` type; `ReadOnly`/`WriteOnly` exist only \
                     in a register map (03-hardware.md §2)",
                    kind.as_str()
                ),
                span,
            ));
        }
        let inner = match n.args.as_slice() {
            [GenericArg::Type(t)] => t,
            _ => {
                return Err(layout_error(
                    format!(
                        "field `{struct_name}.{field_name}: {rendered}` must wrap exactly one \
                         register type (e.g. `ReadOnly[u32]`) (03-hardware.md §2)"
                    ),
                    span,
                ));
            }
        };
        let ast::Type::Named(i) = inner else {
            return Err(no_exact_size_error(
                struct_name,
                field_name,
                &rendered,
                kind,
                span,
            ));
        };
        return match scalar_field_size(&i.name).filter(|_| i.args.is_empty()) {
            Some(size) => Ok(Some(FieldBytes::scalar(size))),
            None => Err(layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` wraps `{}`, which is not a \
                     sized integer register (`u8`/`u16`/`u32`/`u64`, or their signed forms) \
                     (03-hardware.md §2)",
                    printer::print_type_bare(inner)
                ),
                span,
            )),
        };
    }
    Err(no_exact_size_error(
        struct_name,
        field_name,
        &rendered,
        kind,
        span,
    ))
}

/// 03 §3.1's array field: `[T; N]`, in a `runtime` layout only.
///
/// Size is `N * size_of(T)` — no stride rounding, no trailing padding — and
/// the array's alignment is its element's, because that is the alignment
/// every element needs and an array adds no requirement of its own.
///
/// The element's own rules are checked whether or not the length is known
/// (plans/M10.md item A2b requirement 1: the early pass still checks
/// *shape*), so `[usize; N_TURNS]` is refused before name resolution exactly
/// as `[usize; 4]` is. Only the multiplication waits.
#[allow(clippy::too_many_arguments)]
fn array_field_bytes(
    struct_name: &str,
    field_name: &str,
    kind: LayoutKind,
    a: &ast::ArrayType,
    rendered: &str,
    decls: &LayoutDecls,
    nest: &mut NestCtx<'_>,
    span: Span,
) -> Result<Option<FieldBytes>, SemaError> {
    if kind != LayoutKind::Runtime {
        // The allowance is 03 §3.1's, and it is stated as belonging to the
        // fourth kind alone: "It adds one allowance the other three kinds
        // do not have". So this is a permanent scope rule, not a floor —
        // said by name rather than folded into "no exact byte size", which
        // would be false (`[u32; 4]` has a perfectly exact size; a `dma`
        // layout just may not have one).
        return Err(layout_error(
            format!(
                "field `{struct_name}.{field_name}: {rendered}` is an array, but `{struct_name}` \
                 is a `@layout({})` type; a fixed-length array field is the `runtime` kind's own \
                 allowance, which \"the other three kinds do not have\" (03-hardware.md §3.1)",
                kind.as_str()
            ),
            span,
        ));
    }
    let len = array_field_len(struct_name, field_name, rendered, &a.len, nest, span)?;
    let elem_rendered = printer::print_type_bare(&a.elem);
    let elem = match &a.elem {
        ast::Type::Named(n) if n.args.is_empty() && scalar_field_size(&n.name).is_some() => Some(
            FieldBytes::scalar(scalar_field_size(&n.name).expect("just matched")),
        ),
        ast::Type::Named(n) if n.args.is_empty() && decls.contains_key(&n.name) => {
            let nested = decls[&n.name];
            nested_field_bytes(
                struct_name,
                field_name,
                kind,
                nested,
                rendered,
                decls,
                nest,
                span,
            )?
        }
        // Everything else, in one message rather than a second copy of the
        // scalar table's rejections: §3.1 spells the element set out
        // ("another `@layout(runtime)` type, or a fixed-length array of
        // one"), so `[usize; 4]`, `[[u32; 2]; 2]`, `[bool; 8]` and
        // `[DeviceCap[D]; 2]` are all the same rejection — the element is
        // not one of the two things an array field's element may be.
        // Notably `[usize; N]` is refused here too: decision 563 /
        // plans/M10.md item A2 add **no** `usize` exemption for the
        // `runtime` kind, because one target-dependent layout class breaks
        // the property the whole mechanism exists for.
        _ => {
            return Err(layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` has element type \
                     `{elem_rendered}`, which is not an array field's element type; that is a \
                     sized integer (`u8`/`u16`/`u32`/`u64`, or their signed forms) or a nested \
                     `@layout(runtime)` struct (03-hardware.md §3.1)"
                ),
                span,
            ));
        }
    };
    // The element itself is deferred (it nests a table whose own array
    // length is a `const` name), so this field is too. Every rule about the
    // element that does not need its byte count has already run inside
    // `nested_field_bytes` — kind, capability, cycle, depth, budget.
    let Some(elem) = elem else { return Ok(None) };
    // An array is elements back to back at stride `size_of(T)`. If that
    // stride is not a multiple of `T`'s own alignment, element 1 onwards
    // land misaligned, and the only fix is padding between elements — the
    // one thing 03 §3 says a `@layout` never invents. Refused here, at the
    // declaration, rather than reported as a size the elements do not
    // actually have.
    if elem.size % elem.align != 0 {
        return Err(layout_error(
            format!(
                "field `{struct_name}.{field_name}: {rendered}` has element type \
                 `{elem_rendered}`, which is {} byte(s) wide but {}-byte aligned, so every \
                 element after the first would need implicit padding to be aligned; a `@layout` \
                 type never pads implicitly (03-hardware.md §3)",
                elem.size, elem.align
            ),
            span,
        ));
    }
    // The length is a `const` name and this is the early pass: defer. The
    // element's rules above have all been checked already.
    let Some(len) = len else { return Ok(None) };
    let size = len.checked_mul(elem.size).ok_or_else(|| {
        layout_error(
            format!(
                "field `{struct_name}.{field_name}: {rendered}` is {len} elements of {} byte(s), \
                 which does not fit in a 64-bit byte count; a `@layout` type's size is exact \
                 (03-hardware.md §3)",
                elem.size
            ),
            span,
        )
    })?;
    Ok(Some(FieldBytes {
        size,
        align: elem.align,
    }))
}

/// An array field's length: **an integer literal, or the name of a
/// module-level `const`** (03-hardware.md §3.1; plans/M10.md decisions 580
/// and 581, item A2b).
///
/// Two passes read this one function, and the difference between them is
/// `nest.lens`:
///
/// - `None` — the early pass (`check_layouts`), which runs before name
///   resolution and evaluates nothing. A literal is decoded here; a `const`
///   name **defers** (`Ok(None)`), and the layout is completed later. It is
///   not resolved here, and no second name resolver is built to try:
///   decision 580's rejected alternative (ii) stands in full.
/// - `Some(table)` — the later pass (`complete_layouts`), which runs after
///   const evaluation with every needed `const` already evaluated by the one
///   real evaluator. A name resolves out of the table or is a named
///   rejection; nothing defers twice.
///
/// **`@offset(n)` does not move** (`parse_offset_attr`): decision 580's
/// reasoning applies to it unchanged, and only lengths are what M10's
/// per-image tables need. An offset the compiler must evaluate is still
/// inference by another name.
///
/// Anything that is neither a literal nor a bare name — arithmetic in the
/// length position, a field access, a call — is a named rejection in both
/// passes. A `const` whose own *initializer* is arithmetic works fine
/// (`const N = BASE * 2`): that is the evaluator's job, and it does it.
fn array_field_len(
    struct_name: &str,
    field_name: &str,
    rendered: &str,
    len: &Expr,
    nest: &NestCtx<'_>,
    span: Span,
) -> Result<Option<u64>, SemaError> {
    if let Expr::Name(_, name) = len {
        let Some(lens) = nest.lens else {
            // The early pass: defer, do not evaluate, do not guess.
            return Ok(None);
        };
        // `collect_length_consts` put every name this module's `@layout`
        // structs mention into the table, or failed closed naming the one it
        // could not. A miss here is therefore a producer disagreement, and
        // it is reported as the rule it is rather than as an `internal
        // error:` (which is a bug by house rule, CLAUDE.md).
        let Some(value) = lens.get(name) else {
            return Err(layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` has length `{name}`, which \
                     this module's own `const`s do not define; an array field's length is an \
                     integer literal or the name of a module-level `const` (03-hardware.md §3.1)"
                ),
                span,
            ));
        };
        return Ok(Some(*value));
    }
    let Expr::Int(_, text) = len else {
        return Err(layout_error(
            format!(
                "field `{struct_name}.{field_name}: {rendered}` has a length that is neither an \
                 integer literal nor the name of a module-level `const`; an array field's length \
                 is one of those two and nothing else — a length this compiler would have to \
                 type-check an expression to learn is inference by another name, the same rule \
                 `@offset(n)` already states (03-hardware.md §3.1, plans/M10.md decisions 580, \
                 581)"
            ),
            span,
        ));
    };
    let value = super::bodies::parse_int_literal(text)
        .and_then(|v| u64::try_from(v).ok())
        .ok_or_else(|| {
            layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` has a length this compiler \
                     cannot read as a byte count (03-hardware.md §3)"
                ),
                span,
            )
        })?;
    if value == 0 {
        // A zero-length array is a zero-byte field, and "size zero" is
        // never a reportable answer here (the empty-layout guard below says
        // the same thing one level up). It would also make the alignment
        // check divide by zero.
        return Err(layout_error(
            format!(
                "field `{struct_name}.{field_name}: {rendered}` has length 0; a `@layout` field \
                 covers at least one byte (03-hardware.md §3)"
            ),
            span,
        ));
    }
    Ok(Some(value))
}

/// A field whose type is another `@layout` struct declared in this module.
///
/// 03 §3.1 allows exactly one shape of this: a `runtime` layout nesting a
/// `runtime` layout. Both other combinations are refused, and the two
/// refusals say different true things — a `dma`/`mmio`/`wire` layout
/// nesting one of its own kind is the M7 item E gap (a missing feature),
/// while any nesting that crosses the `runtime` boundary is a permanent
/// rule (§3.1: a `runtime` layout "is not device-visible", so it is neither
/// a DMA payload nor a register map, in either direction).
#[allow(clippy::too_many_arguments)]
fn nested_field_bytes(
    struct_name: &str,
    field_name: &str,
    kind: LayoutKind,
    nested: &StructItem,
    rendered: &str,
    decls: &LayoutDecls,
    nest: &mut NestCtx<'_>,
    span: Span,
) -> Result<Option<FieldBytes>, SemaError> {
    if kind != LayoutKind::Runtime {
        return Err(match declared_layout_kind(&nested.attrs) {
            Some(LayoutKind::Runtime) => {
                nested_layout_kind_error(struct_name, field_name, kind, rendered, span)
            }
            // Unchanged since plans/M7.md item B: composing a `dma` payload
            // out of a header layout plus a status layout is still not
            // implemented, and still belongs to the item that owns those
            // shapes. Item A2 widened the `runtime` kind only.
            _ => layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` nests a `@layout` type; a \
                     nested `@layout` field is not implemented (plans/M7.md item E owns the \
                     composite queue/DMA layouts that need it)"
                ),
                span,
            ),
        });
    }
    if nest.stack.iter().any(|n| n == &nested.name) {
        // A cycle, direct (`struct A: a: A`) or transitive. Caught here,
        // at the field that closes the loop, so the diagnostic can print
        // the whole chain — and caught *before* recursing, so this is a
        // diagnostic rather than a stack overflow.
        let mut chain = nest.stack.clone();
        chain.push(nested.name.clone());
        return Err(layout_error(
            format!(
                "field `{struct_name}.{field_name}: {rendered}` nests `{}`, which is already \
                 being laid out ({}); a `@layout` type cannot contain itself, directly or \
                 transitively — its size would have no finite value (03-hardware.md §3.1)",
                nested.name,
                chain.join(" -> ")
            ),
            span,
        ));
    }
    if nest.stack.len() >= MAX_LAYOUT_NEST_DEPTH {
        return Err(layout_error(
            format!(
                "field `{struct_name}.{field_name}: {rendered}` nests `@layout(runtime)` types \
                 more than {MAX_LAYOUT_NEST_DEPTH} deep ({}); 03-hardware.md §3.1's tables nest \
                 two levels, and this pass refuses a deeper chain rather than recursing on one",
                nest.stack.join(" -> ")
            ),
            span,
        ));
    }
    let Some(left) = nest.budget.checked_sub(1) else {
        return Err(layout_error(
            format!(
                "field `{struct_name}.{field_name}: {rendered}` expands more than \
                 {MAX_LAYOUT_NEST_EXPANSIONS} nested `@layout(runtime)` types in one declaration; \
                 a nested layout is sized from scratch at every mention, so a wide *and* deep \
                 nesting graph has no answer this pass will finish computing (03-hardware.md §3.1)"
            ),
            span,
        ));
    };
    nest.budget = left;
    let attr = nested
        .attrs
        .iter()
        .find(|a| a.name == "layout")
        .expect("`decls` holds only structs carrying `@layout`");
    // Lays the nested type out from scratch, every time it is named. Its
    // own rejections (including a malformed `@layout` on it) surface here
    // with their own spans, which is the honest answer: the outer type has
    // no size until the inner one does. Recomputation is deliberate — this
    // pass is a pure function of the ast with no cache (`check_layouts`'
    // own purity note), and the corpus's deepest `runtime` chain is two;
    // `MAX_LAYOUT_NEST_EXPANSIONS` above is what keeps that affordable.
    let (inner, align) = lay_out_struct(nested, attr, decls, nest)?;
    if inner.kind != LayoutKind::Runtime {
        return Err(nested_layout_kind_error(
            struct_name,
            field_name,
            kind,
            rendered,
            span,
        ));
    }
    // The nested table's own sizing may itself be deferred (its array length
    // is a `const` name); then so is this field's. The kind check above has
    // already run, so the *rules* are checked either way.
    match inner.size {
        Some(size) => Ok(Some(FieldBytes { size, align })),
        None => Ok(None),
    }
}

/// The nesting rule's cross-kind half, in one place so both directions read
/// the same (03-hardware.md §3.1).
fn nested_layout_kind_error(
    struct_name: &str,
    field_name: &str,
    kind: LayoutKind,
    rendered: &str,
    span: Span,
) -> SemaError {
    layout_error(
        format!(
            "field `{struct_name}.{field_name}: {rendered}` nests a `@layout` type of a different \
             kind, and `{struct_name}` is a `@layout({})` type; only a `runtime` layout may nest \
             another, and only a `runtime` layout may be nested — a `runtime` layout \"is not \
             device-visible\", so it is neither a `dma` payload nor an `mmio` register map \
             (03-hardware.md §3.1)",
            kind.as_str()
        ),
        span,
    )
}

fn no_exact_size_error(
    struct_name: &str,
    field_name: &str,
    rendered: &str,
    kind: LayoutKind,
    span: Span,
) -> SemaError {
    // `Runtime` has no register wrappers — §2's `ReadOnly`/`WriteOnly`
    // exist only in a register map — but it does have §3.1's two extra
    // field shapes, so its version of this message names them and cites
    // the subsection that grants them.
    let (extra, cite) = match kind {
        LayoutKind::Mmio => (
            ", optionally wrapped in `ReadOnly`/`WriteOnly`",
            "03-hardware.md §3",
        ),
        LayoutKind::Dma | LayoutKind::Wire => ("", "03-hardware.md §3"),
        LayoutKind::Runtime => (
            ", a nested `@layout(runtime)` struct, or a fixed-length array of one",
            "03-hardware.md §3.1",
        ),
    };
    layout_error(
        format!(
            "field `{struct_name}.{field_name}: {rendered}` has no exact byte size; a `@layout` \
             field is a sized integer (`u8`/`u16`/`u32`/`u64`, or their signed forms){extra} \
             ({cite})"
        ),
        span,
    )
}

/// Lays out one `@layout` struct, checking every rule as it goes. Discards
/// the alignment `lay_out_struct` also computes — only a *nested* field
/// needs it (`nested_field_bytes`); a top-level layout's own alignment is
/// nothing 03 §3 reports, because nothing rounds a `@layout` type's size up
/// to it.
fn check_one_layout(
    s: &StructItem,
    attr: &Attr,
    decls: &LayoutDecls,
    lens: Option<&LengthConsts>,
) -> Result<LayoutType, SemaError> {
    let mut nest = NestCtx {
        stack: Vec::new(),
        budget: MAX_LAYOUT_NEST_EXPANSIONS,
        lens,
    };
    lay_out_struct(s, attr, decls, &mut nest).map(|(l, _align)| l)
}

/// `check_one_layout`'s body, plus the layout's alignment (the widest
/// alignment among its fields) and the `NestCtx` a `runtime` field recurses
/// through. `nest.stack` holds the chain of layout structs currently being
/// laid out, outermost first; `nested_field_bytes` reads it to refuse a
/// cycle and an over-deep chain before either can recurse.
fn lay_out_struct(
    s: &StructItem,
    attr: &Attr,
    decls: &LayoutDecls,
    nest: &mut NestCtx<'_>,
) -> Result<(LayoutType, u64), SemaError> {
    let name = s.name.clone();
    let (kind, endian) = parse_layout_attr(&name, attr)?;
    if !s.generics.is_empty() {
        return Err(layout_error(
            format!(
                "`@layout` struct `{name}` is generic; a `@layout` type's size and offsets are \
                 exact and cannot depend on a generic argument (03-hardware.md §3)"
            ),
            s.span,
        ));
    }
    let mut walk = Walk::default();
    nest.stack.push(name.clone());
    let laid = lay_out_fields(s, &name, kind, decls, nest, &mut walk);
    nest.stack.pop();
    laid?;
    if !walk.saw_field {
        return Err(layout_error(
            format!(
                "`@layout` struct `{name}` declares no fields; a `@layout` type is an exact byte \
                 layout and has no empty form (03-hardware.md §3)"
            ),
            s.span,
        ));
    }
    if walk.deferred {
        // plans/M10.md item A2b: at least one array length is a `const`
        // name, so this layout has no offsets and no size *yet*. Reported as
        // the absence it is — `size: None`, no entries — never as a zero.
        // `complete_layouts` produces the real thing.
        return Ok((
            LayoutType {
                name,
                kind,
                endian,
                size: None,
                padding: 0,
                entries: Vec::new(),
            },
            1,
        ));
    }
    // Every size-dependent rule below this line runs only on a layout whose
    // sizes are all real: the total-bytes bound here, and overlap/alignment
    // inside `lay_out_fields`. On a deferred layout they run in
    // `complete_layouts` instead, on the completed table (item A2b
    // requirement 2) — they are never skipped, only postponed.
    if walk.cursor > MAX_LAYOUT_BYTES {
        return Err(layout_error(
            format!(
                "`@layout` struct `{name}` covers {} bytes, more than the {MAX_LAYOUT_BYTES} this \
                 compiler will lay out in one declaration; the machine has 1 GiB in total, so a \
                 single exact-bytes declaration this large is a mistake in the declaration rather \
                 than a table (03-hardware.md §3)",
                walk.cursor
            ),
            s.span,
        ));
    }
    Ok((
        LayoutType {
            name,
            kind,
            endian,
            size: Some(walk.cursor),
            padding: walk.padding,
            entries: walk.entries,
        },
        walk.align,
    ))
}

/// One `lay_out_struct` call's running state, in one struct so the field
/// walk takes one `&mut` rather than six.
struct Walk {
    entries: Vec<LayoutEntry>,
    cursor: u64,
    padding: u64,
    /// The widest alignment among the fields — a *nested* field's own
    /// requirement (`nested_field_bytes`); a top-level layout's alignment is
    /// nothing 03 §3 reports.
    align: u64,
    /// `(name, start, end)` of the previous field, for the overlap-vs-order
    /// diagnostic split.
    last_field: Option<(String, u64, u64)>,
    /// Any field at all was declared (the empty-layout guard). Distinct from
    /// `last_field`, which a deferred field does not set — a layout whose
    /// only field is deferred still declares a field.
    saw_field: bool,
    /// plans/M10.md item A2b: some field's byte count is not known yet, so
    /// this whole layout's sizing is deferred to `complete_layouts`. Once
    /// set, the offset arithmetic stops (there is nothing true to compute)
    /// while the per-field rule checks continue.
    deferred: bool,
}

impl Default for Walk {
    fn default() -> Self {
        Walk {
            entries: Vec::new(),
            cursor: 0,
            padding: 0,
            align: 1,
            last_field: None,
            saw_field: false,
            deferred: false,
        }
    }
}

/// `lay_out_struct`'s field walk, split out only so `nest.stack` is popped
/// on every exit path — including the rejections, of which there are
/// a dozen. `walk` is `lay_out_struct`'s own local; nothing here is shared
/// with a sibling layout.
fn lay_out_fields(
    s: &StructItem,
    name: &str,
    kind: LayoutKind,
    decls: &LayoutDecls,
    nest: &mut NestCtx<'_>,
    walk: &mut Walk,
) -> Result<(), SemaError> {
    for m in &s.members {
        let f = match m {
            Member::Field(f) => f,
            // A `@layout` type is an encoding, not behavior: its methods,
            // constructor, and pool bindings are all surface this item
            // does not check, so they fail closed rather than being
            // silently accepted and silently unchecked.
            Member::Fn(f) => {
                return Err(layout_error(
                    format!(
                        "`@layout` struct `{name}` declares a method (`{}`); a `@layout` type \
                         declares fields only (03-hardware.md §2/§3)",
                        f.name
                    ),
                    f.span,
                ));
            }
            Member::Init(i) => {
                return Err(layout_error(
                    format!(
                        "`@layout` struct `{name}` declares an `init`; a `@layout` type declares \
                         fields only (03-hardware.md §2/§3)"
                    ),
                    i.span,
                ));
            }
            Member::Pool(p) => {
                return Err(layout_error(
                    format!(
                        "`@layout` struct `{name}` declares a pool (`{}`); a `@layout` type \
                         declares fields only (03-hardware.md §2/§3)",
                        p.name
                    ),
                    p.span,
                ));
            }
            Member::ComptimeIf(c) => {
                return Err(layout_error(
                    format!(
                        "`@layout` struct `{name}` declares a `comptime if` member; a `@layout` \
                         type's fields are exact and unconditional (03-hardware.md §3)"
                    ),
                    c.span,
                ));
            }
        };
        walk.saw_field = true;
        let bytes = layout_field_bytes(name, &f.name, kind, &f.ty, decls, nest, f.span)?;
        // The field-attribute rules are *shape*, not size — one `@offset`,
        // an integer-literal argument, no other attribute — so they run on
        // every field, including a deferred one (item A2b requirement 1: the
        // early pass keeps checking everything it can check).
        let mut explicit: Option<u64> = None;
        for a in &f.attrs {
            if a.name == "offset" {
                if explicit.is_some() {
                    return Err(layout_error(
                        format!("field `{name}.{}` carries more than one `@offset`", f.name),
                        a.span,
                    ));
                }
                explicit = Some(parse_offset_attr(name, &f.name, a)?);
            } else {
                return Err(layout_error(
                    format!(
                        "unknown attribute `@{}` on field `{name}.{}`; a `@layout` field's only \
                         attribute is `@offset(n)` (02-language.md §13)",
                        a.name, f.name
                    ),
                    a.span,
                ));
            }
        }
        // plans/M10.md item A2b: this field's byte count is a `const` name
        // the early pass may not evaluate, so no offset after it is
        // computable. Every rule that does not need a byte count has already
        // run; the ones that do — overlap, alignment, total size — run in
        // `complete_layouts` on the completed table.
        let Some(FieldBytes { size, align }) = bytes else {
            walk.deferred = true;
            continue;
        };
        if walk.deferred {
            continue;
        }
        walk.align = walk.align.max(align);
        let offset = explicit.unwrap_or(walk.cursor);
        // plans/M7.md item I's sweep: `@offset(n)` accepts any `n` a
        // `u64` holds, and the two additions below (`offset + size`, and
        // the `cursor` advance) both overflowed on one. In a debug build
        // that was a `panic!` out of `wrela dump --stage=layout-types`; in
        // a release build it would have *wrapped*, so
        // `@offset(0xFFFFFFFFFFFFFFFF) z: u8` would have reported a
        // `size=0` layout — a zero-byte `@layout(dma)` type, hence a DMA
        // pool of `count` slots and zero bytes of backing, which is the
        // fail-open this chapter exists to prevent. A field whose last
        // byte does not exist is rejected here by name instead, before
        // either addition runs.
        let field_end = offset.checked_add(size).ok_or_else(|| {
            layout_error(
                format!(
                    "field `{name}.{}: {}` at offset {offset:#x} is {size} byte(s) wide, so its \
                     last byte lies past the end of a 64-bit address space; a `@layout` type's \
                     offsets and size are exact (03-hardware.md §3)",
                    f.name,
                    printer::print_type_bare(&f.ty)
                ),
                f.span,
            )
        })?;
        if offset < walk.cursor {
            let (prev_name, prev_start, prev_end) = walk
                .last_field
                .clone()
                .unwrap_or_else(|| (String::from("<start>"), walk.cursor, walk.cursor));
            // Two distinct violations share this one condition, and the
            // diagnostic must not claim the wrong one: a field declared
            // after `prev` may sit entirely *before* it (an ordering
            // violation with no byte in common) or genuinely share bytes
            // with it. Saying "overlaps" for the first case asserts a
            // fact that is false — `earlier` at 0x0..0x4 does not touch
            // `later` at 0x10..0x14 — and a reader who checks it loses
            // trust in the rest of the message.
            let overlaps = field_end > prev_start;
            return Err(layout_error(
                if overlaps {
                    format!(
                        "field `{name}.{}` at offset {offset:#x} overlaps `{name}.{prev_name}` \
                         ({prev_start:#x}..{prev_end:#x}); a `@layout` type's fields are declared \
                         in ascending offset order and never overlap (03-hardware.md §2)",
                        f.name
                    )
                } else {
                    format!(
                        "field `{name}.{}` at offset {offset:#x} is declared after \
                         `{name}.{prev_name}` ({prev_start:#x}..{prev_end:#x}) but lies before \
                         it; a `@layout` type's fields are declared in ascending offset order \
                         and never overlap (03-hardware.md §2)",
                        f.name
                    )
                },
                f.span,
            ));
        }
        // Checked against the field's *alignment*, not its size. The two
        // are the same number for every sized integer and register wrapper,
        // which is why this read `offset % size` until plans/M10.md item
        // A2; they part company for §3.1's array and nested-struct fields,
        // where `size % align == 0` but `size` itself is not the
        // requirement (a `[TurnArea; 4]` is 32 bytes and needs 4-byte
        // alignment, not 32-byte).
        if offset % align != 0 {
            return Err(match explicit {
                Some(n) => layout_error(
                    format!(
                        "field `{name}.{}: {}` at `@offset({n:#x})` is not {align}-byte aligned \
                         (03-hardware.md §2)",
                        f.name,
                        printer::print_type_bare(&f.ty)
                    ),
                    f.span,
                ),
                None => implicit_padding_error(name, &f.name, &f.ty, offset, align, f.span),
            });
        }
        if offset > walk.cursor {
            let gap = offset - walk.cursor;
            walk.entries.push(LayoutEntry::Padding {
                offset: walk.cursor,
                size: gap,
            });
            walk.padding += gap;
        }
        walk.entries.push(LayoutEntry::Field(LayoutField {
            name: f.name.clone(),
            ty: printer::print_type_bare(&f.ty),
            offset,
            size,
        }));
        walk.cursor = field_end;
        walk.last_field = Some((f.name.clone(), offset, walk.cursor));
    }
    Ok(())
}

/// The implicit-padding rejection (plans/M7.md decision 4: "no implicit
/// padding"). It fires exactly when a field with no `@offset` would land
/// at a natural offset its own alignment does not divide — the one place a
/// conventional compiler inserts padding silently. This one refuses and
/// says how many bytes it would have had to invent.
fn implicit_padding_error(
    struct_name: &str,
    field_name: &str,
    ty: &ast::Type,
    offset: u64,
    align: u64,
    span: Span,
) -> SemaError {
    let needed = align - (offset % align);
    layout_error(
        format!(
            "field `{struct_name}.{field_name}: {}` follows the previous field at offset \
             {offset:#x} and would need {needed} byte(s) of implicit padding to be {align}-byte \
             aligned; a `@layout` type never pads implicitly — give `{field_name}` an explicit \
             `@offset(...)` (03-hardware.md §3)",
            printer::print_type_bare(ty)
        ),
        span,
    )
}

/// `@placed`, accepted only on a module-level `static` (03-hardware.md §3.1,
/// plans/M10.md item A2c). Everywhere else is a named position error —
/// retargeting the total refusal item A shipped (`err-placed-unimplemented`).
///
/// This exists because unknown attributes are otherwise **silently
/// ignored** (`sema::bodies::test_attr_kind`'s own note: 02-language.md
/// §13's "unknown attributes are errors" is not yet enforced anywhere).
/// Narrow by construction: it names exactly one attribute and does not
/// turn on §13's general rule.
///
/// Walks every attribute position the ast has (item, member, field,
/// `comptime if` branch at both scopes). On `Item::Static` the attribute
/// is left alone — [`declare_static`] / [`validate_placed_statics`] own
/// its argument shape, runtime-layout requirement, and uniqueness.
fn check_placed_attrs(module: &Module) -> Result<(), SemaError> {
    fn refuse_wrong_position(attrs: &[Attr]) -> Result<(), SemaError> {
        let Some(attr) = attrs.iter().find(|a| a.name == "placed") else {
            return Ok(());
        };
        Err(SemaError::at(
            "type",
            "`@placed` is legal only on a module-level `static` of a `@layout(runtime)` type \
             (03-hardware.md §3.1); it is legal nowhere else"
                .to_string(),
            attr.span,
        ))
    }
    fn walk_members(members: &[Member]) -> Result<(), SemaError> {
        for m in members {
            match m {
                Member::Field(f) => refuse_wrong_position(&f.attrs)?,
                Member::Fn(f) => refuse_wrong_position(&f.attrs)?,
                Member::Init(i) => refuse_wrong_position(&i.attrs)?,
                Member::Pool(p) => refuse_wrong_position(&p.attrs)?,
                Member::ComptimeIf(c) => {
                    refuse_wrong_position(&c.attrs)?;
                    walk_members(&c.then_branch)?;
                    if let Some(e) = &c.else_branch {
                        walk_members(e)?;
                    }
                }
            }
        }
        Ok(())
    }
    fn walk_items(items: &[Item]) -> Result<(), SemaError> {
        for item in items {
            match item {
                Item::Static(_) => {
                    // `@placed` is owned by declare_static / validate_placed_statics.
                }
                Item::Const(c) => refuse_wrong_position(&c.attrs)?,
                Item::Fn(f) => refuse_wrong_position(&f.attrs)?,
                Item::Pool(p) => refuse_wrong_position(&p.attrs)?,
                Item::Struct(s) => {
                    refuse_wrong_position(&s.attrs)?;
                    walk_members(&s.members)?;
                }
                Item::Enum(e) => {
                    refuse_wrong_position(&e.attrs)?;
                    walk_members(&e.members)?;
                }
                Item::ComptimeIf(c) => {
                    refuse_wrong_position(&c.attrs)?;
                    walk_items(&c.then_branch)?;
                    if let Some(e) = &c.else_branch {
                        walk_items(e)?;
                    }
                }
            }
        }
        Ok(())
    }
    walk_items(&module.items)
}

/// After `declare` + `check_layouts`: every `static` must name a
/// `@layout(runtime)` type, and at most one static may claim each address
/// (03-hardware.md §3.1, plans/M10.md item A2c).
pub fn validate_placed_statics(
    decl_items: &[DeclItem],
    layouts: &[LayoutType],
) -> Result<(), SemaError> {
    let mut by_addr: BTreeMap<u64, String> = BTreeMap::new();
    for item in decl_items {
        let DeclItem::Static(s) = item else {
            continue;
        };
        let Type::Named(type_name, targs) = &s.ty else {
            return Err(SemaError {
                category: "type",
                message: format!(
                    "`static {}` has type `{}`, but `@placed` requires a `@layout(runtime)` type \
                     (03-hardware.md §3.1)",
                    s.name,
                    render_type(&s.ty)
                ),
                line: 0,
                col: 0,
                extra_lines: Vec::new(),
                omit_location: true,
                missing_method: None,
            });
        };
        if !targs.is_empty() {
            return Err(SemaError {
                category: "type",
                message: format!(
                    "`static {}` has type `{}`, but `@placed` requires a non-generic \
                     `@layout(runtime)` type (03-hardware.md §3.1)",
                    s.name,
                    render_type(&s.ty)
                ),
                line: 0,
                col: 0,
                extra_lines: Vec::new(),
                omit_location: true,
                missing_method: None,
            });
        }
        let Some(layout) = layouts.iter().find(|l| l.name == *type_name) else {
            return Err(SemaError {
                category: "type",
                message: format!(
                    "`static {}` has type `{type_name}`, which is not a `@layout` type; \
                     `@placed` requires a `@layout(runtime)` type (03-hardware.md §3.1)",
                    s.name
                ),
                line: 0,
                col: 0,
                extra_lines: Vec::new(),
                omit_location: true,
                missing_method: None,
            });
        };
        if layout.kind != LayoutKind::Runtime {
            return Err(SemaError {
                category: "type",
                message: format!(
                    "`static {}` has type `{type_name}` (`@layout({})`), but `@placed` requires \
                     a `@layout(runtime)` type (03-hardware.md §3.1)",
                    s.name,
                    layout.kind.as_str()
                ),
                line: 0,
                col: 0,
                extra_lines: Vec::new(),
                omit_location: true,
                missing_method: None,
            });
        }
        if let Some(earlier) = by_addr.insert(s.addr, s.name.clone()) {
            return Err(SemaError {
                category: "type",
                message: format!(
                    "`static {}` and `static {earlier}` both claim `@placed({:#x})`; \
                     03-hardware.md §3.1 allows at most one placed static per address",
                    s.name, s.addr
                ),
                line: 0,
                col: 0,
                extra_lines: Vec::new(),
                omit_location: true,
                missing_method: None,
            });
        }
    }
    Ok(())
}

/// Every `@layout` type declared in `module`, laid out and checked, in
/// declaration order. Also rejects `@offset` on a field of a struct that
/// is not a `@layout` at all (02-language.md §13: "`@offset(n)` — field
/// offset inside a `@layout` declaration"), and a struct carrying two
/// `@layout` attributes.
///
/// Runs before name resolution (this section's own pass-order note) and is
/// therefore a pure function of the specialized ast — no symbol table, no
/// resolved type, no evaluator. `sema::check_typed`/`check_program_typed`
/// call it for its rejections; `wrela dump --stage=layout-types` and the
/// image report's own exact-bytes section call it for its table.
///
/// It also runs `check_placed_attrs` first: `@placed` is a §3.1
/// layout-class attribute and this is the only whole-module pass that owns
/// 03 §3. Acceptance of `@placed` on a `static` (and the runtime-layout /
/// uniqueness rules) is [`validate_placed_statics`], which needs declare's
/// resolved types and runs after this pass.
///
/// **A `runtime` layout whose array length is a `const` name comes back
/// deferred** (`size: None`), not rejected — plans/M10.md item A2b. Decision
/// 580's purity is untouched: this pass still evaluates nothing and still
/// resolves no name. [`complete_layouts`] finishes the job after const
/// evaluation, and every consumer of a byte count refuses an uncompleted
/// layout by name ([`LayoutType::require_size`]).
pub fn check_layouts(module: &Module) -> Result<Vec<LayoutType>, SemaError> {
    check_placed_attrs(module)?;
    // Every struct carrying a `@layout` at all, well-formed or not — the
    // same set this pass has always collected, now carrying the declaration
    // itself so 03 §3.1's nested `runtime` field can be sized by laying the
    // nested struct out (plans/M10.md item A2). A malformed `@layout` on a
    // *nested* struct therefore surfaces its own rejection, with its own
    // span, at whichever of the two declarations this pass reaches first.
    let mut decls: LayoutDecls = BTreeMap::new();
    for item in &module.items {
        if let Item::Struct(s) = item {
            if s.attrs.iter().any(|a| a.name == "layout") {
                decls.insert(s.name.clone(), s);
            }
        }
    }
    let mut out = Vec::new();
    for item in &module.items {
        let Item::Struct(s) = item else { continue };
        let attrs: Vec<&Attr> = s.attrs.iter().filter(|a| a.name == "layout").collect();
        match attrs.as_slice() {
            [] => {
                for m in &s.members {
                    let Member::Field(f) = m else { continue };
                    if let Some(a) = f.attrs.iter().find(|a| a.name == "offset") {
                        return Err(layout_error(
                            format!(
                                "`@offset` on field `{}.{}` outside a `@layout` declaration; \
                                 `@offset(n)` is a field offset inside a `@layout` type \
                                 (02-language.md §13)",
                                s.name, f.name
                            ),
                            a.span,
                        ));
                    }
                }
            }
            [attr] => out.push(check_one_layout(s, attr, &decls, None)?),
            [_, second, ..] => {
                return Err(layout_error(
                    format!(
                        "struct `{}` carries more than one `@layout` attribute; a type has one \
                         exact byte layout or none (03-hardware.md §3)",
                        s.name
                    ),
                    second.span,
                ));
            }
        }
    }
    Ok(out)
}

/// The **later layout-completion pass** (plans/M10.md item A2b, decision
/// 581): resolves the `const` array lengths `check_layouts` deferred and
/// finishes those layouts' sizing.
///
/// **Where it runs, and why there.** After `eval::check_comptime`, i.e. after
/// every `const` in `program` has been type-checked by `bodies::check` and
/// evaluated by the one real evaluator — the earliest point at which a length
/// can be resolved *without* building a second name resolver, which is the
/// alternative decision 580 rejected. It cannot run earlier: `program.consts`
/// does not exist before `bodies::check`, and evaluating a `const` needs the
/// typed program. It must not run later than the first consumer of a byte
/// count, so the pipeline calls it immediately after the comptime pass and
/// before anything reads `TypedProgram::layouts`.
///
/// **What it re-checks.** Everything, on the completed table: it re-lays the
/// deferred structs out through the same `lay_out_struct` the early pass
/// uses, with `nest.lens` supplied, so overlap, ordering, alignment, implicit
/// padding, the nesting bounds and the total-size bound all apply to the real
/// numbers (item A2b requirement 2). Nothing is checked in a weaker form
/// because it was deferred; it is checked later, not less.
///
/// A no-op — not even a walk of the module — when no layout deferred, which
/// is every program that does not use a `const` length.
pub fn complete_layouts(
    module: &Module,
    program: &crate::sema::typed::TypedProgram,
    layouts: &mut [LayoutType],
) -> Result<(), SemaError> {
    if layouts.iter().all(|l| l.size.is_some()) {
        return Ok(());
    }
    let mut decls: LayoutDecls = BTreeMap::new();
    for item in &module.items {
        if let Item::Struct(s) = item {
            if s.attrs.iter().any(|a| a.name == "layout") {
                decls.insert(s.name.clone(), s);
            }
        }
    }
    let lens = collect_length_consts(&decls, program)?;
    for l in layouts.iter_mut() {
        if l.size.is_some() {
            continue;
        }
        let Some(s) = decls.get(&l.name) else {
            // A deferred layout whose declaration is not in the module handed
            // to this pass: there is nothing here to complete it from, so it
            // fails closed rather than travelling on with no size
            // (requirement 4). `require_size` always errors on a deferred
            // layout, which is what makes this the whole rejection.
            return Err(l
                .require_size("layout completion")
                .expect_err("a deferred layout has no size, so `require_size` rejects"));
        };
        let attr = s
            .attrs
            .iter()
            .find(|a| a.name == "layout")
            .expect("`decls` holds only structs carrying `@layout`");
        let completed = check_one_layout(s, attr, &decls, Some(&lens))?;
        // The completed layout must actually be complete: with `lens`
        // supplied nothing may defer twice, and a `None` here would be this
        // pass silently failing to do the one thing it exists for.
        completed.require_size("the end of layout completion")?;
        *l = completed;
    }
    Ok(())
}

/// Every `const` name an array length in `decls` mentions, evaluated once,
/// checked to be a length a `@layout` field can have.
///
/// Evaluation goes through `eval::interp::eval_const` — the same evaluator a
/// plain module-level `const` already runs through, so a length that depends
/// on another `const` (`const N = BASE * 2`) works for free, and a `const`
/// that `specialize` removed with its `comptime if` branch is simply not in
/// `program.consts` and is refused by name below. There is no second
/// resolver anywhere in this file: this reads the one real table.
///
/// Four fail-closed rejections, each named: not a `const` of this module; not
/// an integer; zero; negative. Zero is illegal for the same reason a literal
/// `0` length is (a `@layout` field covers at least one byte), and negative
/// for the more basic one that a byte count is not signed. A huge but legal
/// value is *not* rejected here — it is rejected by `MAX_LAYOUT_BYTES` once
/// multiplied out, where the number in the diagnostic is the one that is
/// actually too big.
fn collect_length_consts(
    decls: &LayoutDecls,
    program: &crate::sema::typed::TypedProgram,
) -> Result<LengthConsts, SemaError> {
    let mut out: LengthConsts = BTreeMap::new();
    for s in decls.values() {
        for m in &s.members {
            let Member::Field(f) = m else { continue };
            let ast::Type::Array(a) = &f.ty else { continue };
            let Expr::Name(_, name) = &a.len else {
                continue;
            };
            if out.contains_key(name) {
                continue;
            }
            let where_ = format!("field `{}.{}`'s array length", s.name, f.name);
            if !program.consts.contains_key(name) {
                return Err(layout_error(
                    format!(
                        "{where_} is `{name}`, which is not a module-level `const` visible here; \
                         an array field's length is an integer literal or the name of a \
                         module-level `const` — a name a `comptime if` removed, a local, or a \
                         type is not one (03-hardware.md §3.1, plans/M10.md item A2b)"
                    ),
                    f.span,
                ));
            }
            let value = crate::eval::interp::eval_const(program, name).map_err(|e| {
                layout_error(
                    format!("{where_} `{name}` does not evaluate: {}", e.message),
                    f.span,
                )
            })?;
            let Some(n) = crate::eval::value::as_i128(&value) else {
                return Err(layout_error(
                    format!(
                        "{where_} is `{name}`, whose value is not an integer; an array field's \
                         length is a count of elements (03-hardware.md §3.1)"
                    ),
                    f.span,
                ));
            };
            if n <= 0 {
                return Err(layout_error(
                    format!(
                        "{where_} is `{name}`, whose value is {n}; a `@layout` field covers at \
                         least one byte, so an array length is one or more (03-hardware.md §3.1)"
                    ),
                    f.span,
                ));
            }
            let n = u64::try_from(n).map_err(|_| {
                layout_error(
                    format!(
                        "{where_} is `{name}`, whose value {n} is not a byte count this compiler \
                         can use (03-hardware.md §3.1)"
                    ),
                    f.span,
                )
            })?;
            out.insert(name.clone(), n);
        }
    }
    Ok(out)
}

/// Renders one already-checked `@layout` type in the M1 dump style
/// (`Kind key=value` lines, two-space indent per level), starting at
/// `depth`. Shared verbatim by `wrela dump --stage=layout-types` and the
/// image report's own exact-bytes section so the two can never drift:
/// same facts, same spelling, different indentation.
///
/// Byte offsets print as hex (`offset=0x60`) and byte counts as decimal
/// (`size=4`), the same split the report's own `Layout` section already
/// uses for `base=`/`size=` — an offset is an address inside the map, a
/// size is a count.
///
/// `Err` for an **uncompleted** layout (plans/M10.md item A2b requirement 4):
/// a deferred layout that reached a dump or the image report is a pass-order
/// bug, and it fails closed here rather than printing `size=0` — the exact
/// zero-byte lie 03 §3's own rules exist to prevent.
pub fn push_layout_lines(out: &mut String, depth: usize, l: &LayoutType) -> Result<(), SemaError> {
    let size = l.require_size("the `@layout` table dump")?;
    push_line(
        out,
        depth,
        &format!(
            "Layout name={} kind={} endian={} size={size} padding={}",
            l.name,
            l.kind.as_str(),
            l.endian.as_str(),
            l.padding
        ),
    );
    for e in &l.entries {
        match e {
            LayoutEntry::Field(f) => push_line(
                out,
                depth + 1,
                &format!(
                    "Field name={} type={} offset={:#x} size={}",
                    f.name, f.ty, f.offset, f.size
                ),
            ),
            LayoutEntry::Padding { offset, size } => push_line(
                out,
                depth + 1,
                &format!("Padding offset={offset:#x} size={size}"),
            ),
        }
    }
    Ok(())
}

/// `wrela dump --stage=layout-types`'s whole artifact: one `Module
/// path=...` block per module in the build closure that declares at least
/// one `@layout` type, each carrying its own types in declaration order.
/// A module with nothing to say is absent entirely (the report's own
/// facts-only rule); a closure with no `@layout` type at all is just the
/// version header.
///
/// `by_module` is supplied in the caller's own deterministic order (a
/// `BTreeMap` walk keyed by dotted module address, or the single-file
/// case's one entry).
pub fn dump_layouts(by_module: &[(String, Vec<LayoutType>)]) -> Result<String, SemaError> {
    let mut out = String::from("LayoutTypes v0\n");
    for (path, layouts) in by_module {
        if layouts.is_empty() {
            continue;
        }
        push_line(&mut out, 1, &format!("Module path={path}"));
        for l in layouts {
            push_layout_lines(&mut out, 2, l)?;
        }
    }
    Ok(out)
}

// --- typed MMIO: registers + claim partitioning (plans/M7.md item C) ------
//
// 03-hardware.md §2, the two sentences this section owns: "A driver or
// sealed protocol partitions its claim into declared, non-overlapping
// layouts ... Minting a layout consumes those byte ranges from the claim;
// two live layouts can never alias a register."
//
// ## Where a claim lives, and what consumes from it
//
// A claim is **a device's register map, reached through the `DeviceCap[D]`
// the image binding mints** — 03 §1: "The device itself is named once, at
// the image binding (`img.driver(BlkDriver, device=blk_device)`), the
// single source of truth." A driver has at most one `DeviceCap`
// (`eval::image_checks::check_capability_substitution` enforces that
// already), so **a driver has exactly one claim**, and the claim needs no
// representation of its own beyond the driver that owns it: what a claim
// *is* comes from the image, what a claim is *partitioned into* comes from
// the driver's own declaration. That split is the whole design.
//
// The partition is the driver's own declared `Mmio[L]`-typed **fields** —
// those, and nothing else, are what the driver holds *live*
// (03 §2's own word). A driver holding `irq_regs: Mmio[VirtioIrqMmio]`
// has minted `VirtioIrqMmio`'s byte ranges out of its claim; a second
// field minting a layout that shares a byte with the first is exactly
// "two live layouts alias a register", and is rejected here naming both
// (`hardware.mmio.no-alias`).
//
// An `Mmio[L]` **parameter** is deliberately *not* a mint: it is how an
// already-minted layout is delivered to the driver's `init` or lent to a
// helper fn. Reading it as a second mint would make the one shape 03 §1's
// own worked example needs — an `init` parameter assigned to the field of
// the same layout type — self-aliasing, which is nonsense. Provenance
// (`eval::legal::check_provenance`) is what governs *who* may hold a lent
// layout; this rule governs *what* the owning driver may partition.
//
// ## What a mint consumes: fields, not extent
//
// A layout consumes exactly its declared **field** ranges, never its
// declared holes. 03 §2's own worked example is the argument: its 0x60
// bytes of leading padding "belong to the sealed transport's own
// partition, not to this layout" (`golden/check-layout-mmio`'s own words,
// written at item B before any of this existed) — and §2's very next
// sentence says the sealed transport protocol owns exactly that
// initialization/queue/status/config partition. Consuming the hole would
// make the driver's ISR partition and the transport's partition collide by
// construction, which is the opposite of what the paragraph describes.
//
// ## The boundaries, named rather than discovered later
//
// - **Two drivers bound to one device.** Whether that is legal at all is
//   an image-graph question (`img.driver(A, device=d)` twice), and the
//   graph checks live in `eval::image_checks`, not here. This pass is
//   per-driver and says so: it cannot see the graph, so it cannot see two
//   drivers sharing a claim. `Mmio[L]` is minted by the sealed transport
//   (`claim`/`map_partition`, plans/M7.md item H1), not at the image binding;
//   the cross-driver half still belongs in `eval::image_checks` when two
//   drivers can share a device.
// - **A plain struct holding an `Mmio[L]`, held by no driver.** It mints
//   nothing, because nothing gives it a claim; provenance already rejects
//   any fn that touches it without a driver's authority.

/// A register's declared direction — 03-hardware.md §2's `ReadOnly[T]` /
/// `WriteOnly[T]`. `None` (an `@layout(mmio)` field written as a bare
/// scalar) is not a third direction: it is a register with *no* declared
/// direction, and `sema::bodies` rejects both operations on one by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmioDirection {
    ReadOnly,
    WriteOnly,
}

impl MmioDirection {
    pub fn wrapper(self) -> &'static str {
        match self {
            MmioDirection::ReadOnly => "ReadOnly",
            MmioDirection::WriteOnly => "WriteOnly",
        }
    }
}

/// One declared register of an `@layout(mmio)` type, in the shape an
/// access needs: its direction, the exact-width scalar it carries, and the
/// bytes it occupies in the claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmioRegister {
    pub name: String,
    pub direction: Option<MmioDirection>,
    /// The wrapped scalar's own name (`"u32"`), exactly as source wrote
    /// it — `bodies::scalar_type_by_name` turns it back into a `Type`.
    pub scalar: String,
    pub offset: u64,
    pub size: u64,
}

/// Splits a `@layout(mmio)` field's declared type text into its direction
/// and the scalar it wraps.
///
/// This reads back the source spelling `check_one_layout` already stored
/// (`LayoutField::ty`, `printer::print_type_bare`'s own output) rather
/// than `LayoutField` growing a structured field, for one concrete
/// reason: `LayoutType`/`LayoutField` are constructed literally outside
/// this file (`report.rs`'s own exact-bytes determinism test), so a new
/// field is not a local change. The parse is total over exactly the three
/// shapes `layout_field_size` can accept for an `mmio` field —
/// `ReadOnly[<scalar>]`, `WriteOnly[<scalar>]`, `<scalar>` — and
/// `mmio_registers_are_read_back_from_the_checked_layout` below asserts it
/// against `check_layouts`' own output, not against hand-written strings.
fn split_register_type(rendered: &str) -> (Option<MmioDirection>, &str) {
    for (prefix, dir) in [
        ("ReadOnly[", MmioDirection::ReadOnly),
        ("WriteOnly[", MmioDirection::WriteOnly),
    ] {
        if let Some(rest) = rendered.strip_prefix(prefix) {
            if let Some(inner) = rest.strip_suffix(']') {
                return (Some(dir), inner);
            }
        }
    }
    (None, rendered)
}

/// `layout`'s declared register named `name`, or `None` if it declares no
/// such register. Declared holes are never registers — a `@offset` that
/// skips bytes names nothing.
pub fn mmio_register(layout: &LayoutType, name: &str) -> Option<MmioRegister> {
    layout.entries.iter().find_map(|e| match e {
        LayoutEntry::Field(f) if f.name == name => {
            let (direction, scalar) = split_register_type(&f.ty);
            Some(MmioRegister {
                name: f.name.clone(),
                direction,
                scalar: scalar.to_string(),
                offset: f.offset,
                size: f.size,
            })
        }
        _ => None,
    })
}

/// Every register `layout` declares, in ascending offset order — the
/// diagnostic surface for "this layout declares no register `x`".
pub fn mmio_register_names(layout: &LayoutType) -> Vec<String> {
    layout
        .entries
        .iter()
        .filter_map(|e| match e {
            LayoutEntry::Field(f) => Some(f.name.clone()),
            LayoutEntry::Padding { .. } => None,
        })
        .collect()
}

/// One `Mmio[L]` mint found on a driver: which field carries it, that
/// field's own declared type (which is *not* always `Mmio[L]` — a plain
/// wrapper struct reaches one too), and which layout it named.
struct Mint {
    field: String,
    field_ty: String,
    layout: String,
    span: Span,
}

/// Collects every `Mmio[L]` a driver field's declared type carries, at any
/// nesting — including through a plain wrapper struct or an enum variant
/// payload, which is the same reach `type_contains_capability` already
/// gives the containment rules (one walk's shape, two questions;
/// `components_by_name` is the shared table). Order is the type's own
/// structural order, so the diagnostic is deterministic.
fn collect_mmio_layouts(
    ty: &Type,
    components: &BTreeMap<String, &[(Type, Span)]>,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<String>,
) {
    match ty {
        Type::Named(name, targs) if name == "Mmio" => {
            if let Some(TypeArg::Type(Type::Named(layout, _))) = targs.first() {
                out.push(layout.clone());
            }
        }
        Type::Array(elem, _) => collect_mmio_layouts(elem, components, seen, out),
        Type::Tuple(elems) => {
            for e in elems {
                collect_mmio_layouts(e, components, seen, out);
            }
        }
        Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
            collect_mmio_layouts(inner, components, seen, out)
        }
        Type::Result(ok, err) => {
            collect_mmio_layouts(ok, components, seen, out);
            collect_mmio_layouts(err, components, seen, out);
        }
        Type::Fn(params, ret) => {
            for (_, t) in params {
                collect_mmio_layouts(t, components, seen, out);
            }
            collect_mmio_layouts(ret, components, seen, out);
        }
        Type::Named(name, targs) => {
            if seen.insert(name.clone()) {
                if let Some(c) = components.get(name.as_str()) {
                    for (t, _) in c.iter() {
                        collect_mmio_layouts(t, components, seen, out);
                    }
                }
            }
            for a in targs {
                if let TypeArg::Type(t) = a {
                    collect_mmio_layouts(t, components, seen, out);
                }
            }
        }
        _ => {}
    }
}

/// The byte ranges `layout` consumes from a claim: one `(start, end)` per
/// declared register, ascending. Declared holes consume nothing (this
/// section's own "fields, not extent" note).
fn consumed_ranges(layout: &LayoutType) -> Vec<(u64, u64, String)> {
    layout
        .entries
        .iter()
        .filter_map(|e| match e {
            LayoutEntry::Field(f) => Some((f.offset, f.offset + f.size, f.name.clone())),
            LayoutEntry::Padding { .. } => None,
        })
        .collect()
}

/// Every `@layout(mmio)` type the `@driver` `driver` mints through its own
/// declared fields, in the fields' own structural order — exactly the set
/// `check_mmio_claims` (below) proves pairwise disjoint. Public because
/// `layout.rs` needs the *same* set to size the device's register window:
/// 03-hardware.md §2's "minting a layout consumes those byte ranges from
/// the claim" is one rule, so the window a claim hands out and the ranges
/// the no-alias rule partitions must come from one walk, never two
/// (plans/M7.md item H1).
///
/// `None` when `driver` names no `@driver` in `items` at all.
pub fn driver_mmio_mints(items: &[DeclItem], driver: &str) -> Option<Vec<String>> {
    let mut structs: BTreeMap<String, &DeclStruct> = BTreeMap::new();
    for item in items {
        if let DeclItem::Struct(s) = item {
            structs.insert(s.name.clone(), s);
        }
    }
    mmio_mints_of(driver, &structs, &components_by_name(items))
}

/// The same walk over already-built tables — `sema::bodies` has them
/// (`ModuleCtx::structs`/`enums`) and `layout.rs` builds them from
/// `DeclItem`s, and they must agree about which layouts a driver mints or
/// the mint operation and the window that backs it would disagree.
///
/// Two tables and not one, because the two questions are different:
/// `structs` answers "is `driver` a `@driver`, and which types does it
/// declare as *fields*" — a field is a mint and a parameter is not
/// (`hardware.mmio.no-alias`) — while `components` is the shared nesting
/// table `collect_mmio_layouts` walks to reach a layout through a wrapper
/// struct or an enum variant payload.
pub fn mmio_mints_of(
    driver: &str,
    structs: &BTreeMap<String, &DeclStruct>,
    components: &BTreeMap<String, &[(Type, Span)]>,
) -> Option<Vec<String>> {
    let d = structs.get(driver).filter(|d| d.is_driver)?;
    let mut out = Vec::new();
    for m in &d.members {
        if let DeclMember::Field(f) = m {
            collect_mmio_layouts(&f.ty, components, &mut BTreeSet::new(), &mut out);
        }
    }
    Some(out)
}

/// The exclusive end of the highest byte `layout`'s declared registers
/// consume — `consumed_ranges`' own answer, reduced. `0` for a layout that
/// declares only holes (03 §2: a declared hole belongs to the sealed
/// transport, not to this layout).
pub fn mmio_consumed_end(layout: &LayoutType) -> u64 {
    consumed_ranges(layout)
        .into_iter()
        .map(|(_, end, _)| end)
        .max()
        .unwrap_or(0)
}

/// 03-hardware.md §2's claim-partitioning sentence, checked
/// (`hardware.mmio.no-alias`): for every `@driver`, the layouts its own
/// fields mint must consume disjoint byte ranges from its one claim.
///
/// Runs after `declare` (it needs resolved field types and the
/// `@driver`/struct-composition facts `DeclStruct` carries) and takes
/// `check_layouts`' already-checked table, so every layout named here is
/// known well-formed. Fail-fast in declaration order, like every other
/// check in this file: the first conflicting *pair* wins, and the
/// diagnostic names both mints, both layouts, both registers and the
/// exact overlapping bytes — 03 §2 is a rule about a pair, so a message
/// naming one half of one would be unactionable.
pub fn check_mmio_claims(
    module: &Module,
    items: &[DeclItem],
    layouts: &[LayoutType],
) -> Result<(), SemaError> {
    let components = components_by_name(items);
    let by_name: BTreeMap<&str, &LayoutType> =
        layouts.iter().map(|l| (l.name.as_str(), l)).collect();

    // Field spans live only on the ast (a `DeclField` carries none), so
    // the two are walked together exactly like `validate_capability_types`
    // above does — same filtered zip, same 1:1 guarantee from `declare`.
    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();
    for (ai, di) in ast_items.iter().zip(items.iter()) {
        let (Item::Struct(s), DeclItem::Struct(d)) = (ai, di) else {
            continue;
        };
        if !d.is_driver {
            continue;
        }
        let ast_fields: Vec<&ast::FieldItem> = s
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Field(f) => Some(f),
                _ => None,
            })
            .collect();
        let decl_fields: Vec<&DeclField> = d
            .members
            .iter()
            .filter_map(|m| match m {
                DeclMember::Field(f) => Some(f),
                _ => None,
            })
            .collect();

        let mut mints: Vec<Mint> = Vec::new();
        for (af, df) in ast_fields.iter().zip(decl_fields.iter()) {
            let mut found = Vec::new();
            collect_mmio_layouts(&df.ty, &components, &mut BTreeSet::new(), &mut found);
            for layout in found {
                mints.push(Mint {
                    field: df.name.clone(),
                    field_ty: render_type(&df.ty),
                    layout,
                    span: af.span,
                });
            }
        }

        for (i, mint) in mints.iter().enumerate() {
            let Some(l) = by_name.get(mint.layout.as_str()) else {
                continue; // not an `@layout(mmio)` type: `validate_capability_types` owns that
            };
            for prior in &mints[..i] {
                let Some(pl) = by_name.get(prior.layout.as_str()) else {
                    continue;
                };
                for (start, end, reg) in consumed_ranges(l) {
                    for (pstart, pend, preg) in consumed_ranges(pl) {
                        if start < pend && pstart < end {
                            let lo = start.max(pstart);
                            let hi = end.min(pend);
                            return Err(layout_error(
                                format!(
                                    "`@driver` `{}` mints two live MMIO layouts that alias the \
                                     same register: field `{}: {}` mints `{}`, claiming `{}.{}` \
                                     ({start:#x}..{end:#x}), and field `{}: {}` already mints \
                                     `{}`, claiming `{}.{}` ({pstart:#x}..{pend:#x}) — they share \
                                     bytes {lo:#x}..{hi:#x}. Minting a layout consumes those byte \
                                     ranges from the claim; two live layouts can never alias a \
                                     register (03-hardware.md §2)",
                                    d.name,
                                    mint.field,
                                    mint.field_ty,
                                    mint.layout,
                                    mint.layout,
                                    reg,
                                    prior.field,
                                    prior.field_ty,
                                    prior.layout,
                                    prior.layout,
                                    preg,
                                ),
                                mint.span,
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

// --- type resolution -----------------------------------------------------

fn declare_const(
    c: &ConstItem,
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
) -> Result<DeclConst, SemaError> {
    match &c.ty {
        Some(t) => {
            let ty = resolve_type(
                t,
                shapes,
                module_pools,
                &BTreeSet::new(),
                &BTreeMap::new(),
                false,
            )?;
            Ok(DeclConst {
                name: c.name.clone(),
                ty,
            })
        }
        // A const without a declared type needs its initializer's type
        // (body typing, item C) to infer one — fail closed rather than
        // guess (decision 7's spirit, applied to a form item B cannot
        // resolve on its own).
        None => Err(unimplemented_at("a const's inferred type is", c.span)),
    }
}

/// `static NAME: Type` with required `@placed(ADDR)` (03-hardware.md §3.1,
/// plans/M10.md item A2c / decision 586). Address is an integer literal,
/// same shape as `@offset`. Runtime-layout and uniqueness checks run in
/// [`validate_placed_statics`] once the layout table exists.
fn declare_static(
    s: &crate::syntax::ast::StaticItem,
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
) -> Result<DeclStatic, SemaError> {
    let ty = resolve_type(
        &s.ty,
        shapes,
        module_pools,
        &BTreeSet::new(),
        &BTreeMap::new(),
        false,
    )?;
    let addr = parse_placed_attr_on_static(s)?;
    Ok(DeclStatic {
        name: s.name.clone(),
        ty,
        addr,
    })
}

/// Exactly one `@placed(ADDR)` on a `static`, ADDR an integer literal.
fn parse_placed_attr_on_static(s: &crate::syntax::ast::StaticItem) -> Result<u64, SemaError> {
    let mut found: Option<&Attr> = None;
    for attr in &s.attrs {
        if attr.name != "placed" {
            continue;
        }
        if found.is_some() {
            return Err(SemaError::at(
                "type",
                format!(
                    "`static {}` declares `@placed` twice; 03-hardware.md §3.1 binds one address \
                     per static (plans/M10.md item A2c)",
                    s.name
                ),
                attr.span,
            ));
        }
        found = Some(attr);
    }
    let Some(attr) = found else {
        return Err(SemaError::at(
            "type",
            format!(
                "`static {}` requires `@placed(ADDR)`: 03-hardware.md §3.1 binds a module-level \
                 static of a `@layout(runtime)` type to a fixed address, and this revision has no \
                 unplaced static storage (plans/M10.md item A2c, decision 586)",
                s.name
            ),
            s.span,
        ));
    };
    let bad = || {
        SemaError::at(
            "type",
            format!(
                "`@placed` on `static {}` takes exactly one integer literal (e.g. \
                 `@placed(0x40000000)`)",
                s.name
            ),
            attr.span,
        )
    };
    let [arg] = attr.args.as_slice() else {
        return Err(bad());
    };
    if arg.label.is_some() {
        return Err(bad());
    }
    let Expr::Int(_, text) = &arg.value else {
        return Err(bad());
    };
    let value = super::bodies::parse_int_literal(text).ok_or_else(bad)?;
    u64::try_from(value).map_err(|_| bad())
}

fn declare_fn(
    f: &FnItem,
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
    local_pools: &BTreeSet<String>,
    outer_generics: &BTreeMap<String, GenericKind>,
) -> Result<DeclFn, SemaError> {
    let mut scope = outer_generics.clone();
    let decl_generics =
        resolve_generics(&f.generics, shapes, module_pools, local_pools, &mut scope)?;
    let receiver = f.receiver.as_ref().map(|r| DeclReceiver {
        mode: r.mode,
        is_pub: f.is_pub,
        is_init: false,
    });
    let mut params = Vec::new();
    for p in &f.params {
        let ty = resolve_type(&p.ty, shapes, module_pools, local_pools, &scope, true)?;
        params.push(DeclParam {
            mode: p.mode,
            name: p.name.clone(),
            ty,
        });
    }
    let ret = resolve_ret(&f.ret, shapes, module_pools, local_pools, &scope, f.is_pub)?;
    let is_task = f.attrs.iter().any(|a| a.name == "task");
    // plans/M13.md item C cuts 6–7: `is_task` is still name-only, but
    // `priority=` / `budget=` (and any other undeferred kwarg) must not be
    // silently ignored. `trigger=` / `poll=` stay for the ISR gate.
    if let Some(attr) = f.attrs.iter().find(|a| a.name == "task") {
        check_task_attr_args(attr)?;
    }
    Ok(DeclFn {
        name: f.name.clone(),
        is_async: f.is_async,
        is_task,
        generics: decl_generics,
        receiver,
        params,
        ret,
    })
}

/// `@task(...)` argument audit (plans/M13.md item C cuts 6–7).
///
/// Before this check, every kwarg was parsed and dropped — only the
/// attribute name set `is_task`. Cut kwargs become `error[type]`;
/// `trigger=` / `poll=` remain legal (deferred to the ISR gate).
fn check_task_attr_args(attr: &Attr) -> Result<(), SemaError> {
    for a in &attr.args {
        match a.label.as_deref() {
            Some("trigger") | Some("poll") => {}
            Some("priority") => {
                return Err(SemaError::at(
                    "type",
                    "`@task(priority=...)` was cut at the revision boundary (plans/M13.md \
                     item C / decision 13); scheduling is FIFO-per-mailbox + round-robin \
                     (04-compiler.md §2) — see ROADMAP recorded intentions for bands"
                        .to_string(),
                    a.span,
                ));
            }
            Some("budget") => {
                return Err(SemaError::at(
                    "type",
                    "`@task(budget=...)` was cut at the revision boundary (plans/M13.md \
                     item C / decision 13); the FIFO+RR scheduler cannot honor a \
                     per-task budget — see ROADMAP recorded intentions for bands"
                        .to_string(),
                    a.span,
                ));
            }
            Some(other) => {
                return Err(SemaError::at(
                    "type",
                    format!(
                        "`@task` has no argument `{other}` (revision 0.1 allows \
                         `trigger=` / `poll=` only; plans/M13.md item C)"
                    ),
                    a.span,
                ));
            }
            None => {
                return Err(SemaError::at(
                    "type",
                    "`@task`'s arguments must be labeled (`trigger=` / `poll=`) \
                     (plans/M13.md item C)"
                        .to_string(),
                    a.span,
                ));
            }
        }
    }
    Ok(())
}

/// `init` is never generic and never `pub` (02-language.md §7.1); its
/// receiver always prints its mode explicitly (see `DeclReceiver`'s doc
/// comment) rather than collapsing a `Read` mode to bare `self`.
fn declare_init(
    i: &InitItem,
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
    local_pools: &BTreeSet<String>,
    outer_generics: &BTreeMap<String, GenericKind>,
) -> Result<DeclFn, SemaError> {
    let mut params = Vec::new();
    for p in &i.params {
        let ty = resolve_type(
            &p.ty,
            shapes,
            module_pools,
            local_pools,
            outer_generics,
            true,
        )?;
        params.push(DeclParam {
            mode: p.mode,
            name: p.name.clone(),
            ty,
        });
    }
    let ret = resolve_ret(
        &i.ret,
        shapes,
        module_pools,
        local_pools,
        outer_generics,
        false,
    )?;
    Ok(DeclFn {
        name: "init".to_string(),
        is_async: false,
        is_task: false,
        generics: Vec::new(),
        receiver: Some(DeclReceiver {
            mode: i.receiver.mode,
            is_pub: false,
            is_init: true,
        }),
        params,
        ret,
    })
}

/// Sentinel name for a private `-> Result[T]` whose error set is not yet
/// inferred (plans/M13.md item K / decision 10). Not source-nameable —
/// only `resolve_ret`'s one-argument `Result` arm constructs it.
pub(crate) const INFERRED_ERROR_SET_NAME: &str = "__InferredErrorSet";

/// Synthetic multi-member error-set carrier (same `Type::Named` shape
/// `CallError` uses). Rendered as `A | B | …`; a single member collapses
/// to that member, and an empty set is `never`.
pub(crate) const ERROR_SET_NAME: &str = "__ErrorSet";

pub(crate) fn inferred_error_set_marker() -> Type {
    Type::Named(INFERRED_ERROR_SET_NAME.to_string(), vec![])
}

pub(crate) fn is_inferred_error_set(ty: &Type) -> bool {
    matches!(ty, Type::Named(n, a) if n == INFERRED_ERROR_SET_NAME && a.is_empty())
}

pub(crate) fn is_inferred_result(ty: &Type) -> bool {
    matches!(ty, Type::Result(_, e) if is_inferred_error_set(e))
}

/// Union the collected error sources into the type typed dumps print.
/// Dedupes by rendered spelling (stable); empty → `never`; one → that
/// type; several → `__ErrorSet[A, B, …]` rendered as `A | B | …`.
pub(crate) fn finalize_error_set(mut members: Vec<Type>) -> Type {
    members.sort_by(|a, b| render_type(a).cmp(&render_type(b)));
    members.dedup_by(|a, b| render_type(a) == render_type(b));
    match members.len() {
        0 => Type::Never,
        1 => members.pop().expect("len == 1"),
        _ => Type::Named(
            ERROR_SET_NAME.to_string(),
            members.into_iter().map(TypeArg::Type).collect(),
        ),
    }
}

fn resolve_ret(
    ret: &Option<ast::Type>,
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
    local_pools: &BTreeSet<String>,
    generics: &BTreeMap<String, GenericKind>,
    is_pub: bool,
) -> Result<Type, SemaError> {
    match ret {
        // plans/M13.md item K / decision 10: private `-> Result[T]`
        // (one-argument form). `pub` refuses here so the golden is a
        // declare-time `error[type]`, not a later body surprise.
        Some(ast::Type::Named(n)) if n.name == "Result" && n.args.len() == 1 => {
            if is_pub {
                return Err(SemaError::at(
                    "type",
                    "a `pub` signature must declare a nominal error type — write \
                     `Result[T, E]`, not `Result[T]` (02-language.md §5)"
                        .to_string(),
                    n.span,
                ));
            }
            let args = expect_type_args(n, 1)?;
            let ok = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            Ok(Type::Result(
                Box::new(ok),
                Box::new(inferred_error_set_marker()),
            ))
        }
        // Resolved types are spelled fully (decision 8): an omitted
        // return type is `unit`, printed explicitly like every other
        // type rather than leaving the arrow off.
        Some(t) => resolve_type(t, shapes, module_pools, local_pools, generics, false),
        None => Ok(Type::Unit),
    }
}

/// Resolves one `[generics]` list, threading each param into `scope` as
/// it goes (so a later const param's bound type, or a sibling param, can
/// never see an *earlier* param except itself — reflecting declaration
/// order — while still allowing forward reference to the struct's own
/// name via `shapes`, already fully built).
fn resolve_generics(
    generics: &[GenericParam],
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
    local_pools: &BTreeSet<String>,
    scope: &mut BTreeMap<String, GenericKind>,
) -> Result<Vec<DeclGenericParam>, SemaError> {
    let mut decl = Vec::new();
    for g in generics {
        match g {
            GenericParam::Type { name, .. } => {
                scope.insert(name.clone(), GenericKind::Type);
                decl.push(DeclGenericParam {
                    name: name.clone(),
                    kind: DeclGenericKind::Type,
                });
            }
            GenericParam::Const { name, ty, .. } => {
                let rty = resolve_type(ty, shapes, module_pools, local_pools, scope, false)?;
                scope.insert(name.clone(), GenericKind::Const);
                decl.push(DeclGenericParam {
                    name: name.clone(),
                    kind: DeclGenericKind::Const(rty),
                });
            }
        }
    }
    Ok(decl)
}

fn has_actor_or_driver(attrs: &[Attr]) -> bool {
    attrs
        .iter()
        .any(|a| a.name == "actor" || a.name == "driver")
}

enum DerivingShape<'a> {
    Struct(&'a StructItem),
    Enum(&'a EnumItem),
}

/// `deriving(...)` validation (02-language.md §7.5, decision closed
/// list): `Format` (plans/M9.md item C2) and `From` (exactly one
/// variant with exactly one field/payload — a struct has no variants, so
/// "one field total" is its version of the same rule); any other name is
/// an error. Neither `Vec<String>` deriving list nor `StructItem`/
/// `EnumItem` carries a span of its own for the `deriving(...)` clause,
/// so errors point at the whole declaration's span — the most precise
/// location available without widening the AST.
fn validate_deriving(
    deriving: &[String],
    shape: &DerivingShape,
    span: Span,
) -> Result<(), SemaError> {
    for name in deriving {
        match name.as_str() {
            "Format" => validate_format_shape(shape, span)?,
            "From" => validate_from_shape(shape, span)?,
            other => {
                return Err(SemaError::at(
                    "type",
                    format!("unknown deriving `{other}`"),
                    span,
                ));
            }
        }
    }
    Ok(())
}

/// plans/M9.md item C2: `deriving(Format)` generates
/// `max_formatted_len() -> usize` and `format(read self) -> String[..N]`
/// (empty-spec form; `{expr:spec}` is comptime syntax for f-strings /
/// item D). Fieldless structs and unit enums format their names;
/// fieldful structs sum scalar field bounds. Payload enums refuse by
/// name. `Secret` has no Format (05 §6).
fn validate_format_shape(shape: &DerivingShape, span: Span) -> Result<(), SemaError> {
    let type_name = match shape {
        DerivingShape::Struct(s) => s.name.as_str(),
        DerivingShape::Enum(e) => e.name.as_str(),
    };
    if type_name == "Secret" {
        return Err(secret_has_no_format(span));
    }
    match shape {
        DerivingShape::Struct(s) => {
            // Fieldful structs are validated for scalar Format-ability
            // when Decl members are built (need resolved field types).
            let _ = s;
        }
        DerivingShape::Enum(e) => {
            if e.variants.is_empty() {
                return Err(SemaError::at(
                    "type",
                    "deriving(Format) requires at least one variant".to_string(),
                    span,
                ));
            }
            for v in &e.variants {
                if !matches!(v.payload, VariantPayload::None) {
                    return Err(SemaError::at(
                        "type",
                        format!(
                            "deriving(Format) requires unit variants; `{}.{}` has a payload",
                            e.name, v.name
                        ),
                        span,
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Decimal `Format` bound for a core scalar (archive §10). `None` when
/// the type has no standard Format — deriving refuses that field.
pub(crate) fn scalar_format_bound(ty: &Type) -> Option<u64> {
    match ty {
        Type::Bool => Some(5), // "false"
        Type::U8 => Some(3),
        Type::U16 => Some(5),
        Type::U32 => Some(10),
        Type::U64 | Type::Usize => Some(20),
        Type::I8 => Some(4),
        Type::I16 => Some(6),
        Type::I32 => Some(11),
        Type::I64 | Type::Isize => Some(20),
        Type::Char => Some(4),
        _ => None,
    }
}

/// 05-library.md §6: "`Secret` has no `Format`." Enforced by type name
/// today — `Secret[T]` is still the marked-value refusal (item G); a
/// user `struct`/`enum` spelled `Secret` that tries to declare or
/// derive Format is refused here by the same rule.
pub(crate) fn secret_has_no_format(span: Span) -> SemaError {
    SemaError::at(
        "type",
        "`Secret` has no `Format` (05-library.md §6)".to_string(),
        span,
    )
}

/// `String[..N]` as an ast type annotation (Format writer return).
fn string_bound_ast_ty(span: Span, n: u64) -> ast::Type {
    ast::Type::Named(NamedType {
        span,
        name: "String".to_string(),
        args: vec![GenericArg::Bound(Expr::Int(span, n.to_string()))],
    })
}

/// Resolved `String[..N]` for DeclFn return types.
fn string_bound_ty(n: u64) -> Type {
    Type::String(Box::new(Expr::Int(Span::default(), n.to_string())))
}

fn validate_from_shape(shape: &DerivingShape, span: Span) -> Result<(), SemaError> {
    let field_count = match shape {
        DerivingShape::Struct(s) => s
            .members
            .iter()
            .filter(|m| matches!(m, Member::Field(_)))
            .count(),
        DerivingShape::Enum(e) => {
            if e.variants.len() != 1 {
                return Err(SemaError::at(
                    "type",
                    "deriving(From) requires exactly one variant".to_string(),
                    span,
                ));
            }
            match &e.variants[0].payload {
                VariantPayload::None => 0,
                VariantPayload::Tuple(types) => types.len(),
                VariantPayload::Named(fields) => fields.len(),
            }
        }
    };
    if field_count != 1 {
        return Err(SemaError::at(
            "type",
            "deriving(From) requires exactly one field".to_string(),
            span,
        ));
    }
    Ok(())
}

/// plans/M9.md item B3: `deriving(From)` generates a real associated
/// `from(take source: Source) -> Self` (05 §8 / 02 §7.5), not a
/// structural wrap on `?`'s error path (supersedes decision 106). The
/// DeclFn is what call sites and `?` resolve; the FnItem body is the
/// ordinary construction that body-checking turns into a TypedFn.
///
/// Conflict (decision 137): a type may not both `deriving(From)` and
/// declare its own `from` — 02 §7.5's closed list is not a macro that
/// merges with a hand-written peer; one construct, one mechanism.
fn derived_from_conflict(type_name: &str, span: Span) -> SemaError {
    SemaError::at(
        "type",
        format!("deriving(From) conflicts with an explicit `from` on `{type_name}`"),
        span,
    )
}

/// The DeclFn half of a generated `from` (return type `Type::Named(name,
/// [])`, one `take source` parameter).
fn derived_from_decl(type_name: &str, source_ty: Type) -> DeclFn {
    DeclFn {
        name: "from".to_string(),
        is_async: false,
        is_task: false,
        generics: Vec::new(),
        receiver: None,
        params: vec![DeclParam {
            mode: AccessMode::Take,
            name: "source".to_string(),
            ty: source_ty,
        }],
        ret: Type::Named(type_name.to_string(), vec![]),
    }
}

/// The AST FnItem half: `pub fn from(take source: Source) -> Self` with
/// body `return Self(<field>=source)` / `return Self.<Variant>(source)`.
/// `pub` so an imported type's generated `from` is reachable the same
/// way a hand-written one is (decision 123's import rule).
pub(crate) fn derived_from_fn_item_struct(
    type_name: &str,
    field: &FieldItem,
    span: Span,
) -> FnItem {
    let source = Expr::Name(span, "source".to_string());
    let construct = Expr::Call(
        Box::new(Expr::Name(span, type_name.to_string())),
        span,
        vec![Arg {
            span,
            label: Some(field.name.clone()),
            mode: AccessMode::Read,
            value: source,
        }],
    );
    FnItem {
        span,
        name: "from".to_string(),
        is_pub: true,
        is_async: false,
        doc: None,
        attrs: Vec::new(),
        generics: Vec::new(),
        receiver: None,
        params: vec![Param {
            span,
            mode: AccessMode::Take,
            name: "source".to_string(),
            ty: field.ty.clone(),
            default: None,
        }],
        ret: Some(ast::Type::Named(NamedType {
            span,
            name: type_name.to_string(),
            args: Vec::new(),
        })),
        body: Some(vec![Stmt::Return(span, Some(construct))]),
    }
}

/// Enum form: body `return Type.Variant(source)`.
pub(crate) fn derived_from_fn_item_enum(
    type_name: &str,
    variant: &str,
    source_ast_ty: &ast::Type,
    span: Span,
) -> FnItem {
    let source = Expr::Name(span, "source".to_string());
    let construct = Expr::Call(
        Box::new(Expr::Field(
            Box::new(Expr::Name(span, type_name.to_string())),
            span,
            variant.to_string(),
        )),
        span,
        vec![Arg {
            span,
            label: None,
            mode: AccessMode::Read,
            value: source,
        }],
    );
    FnItem {
        span,
        name: "from".to_string(),
        is_pub: true,
        is_async: false,
        doc: None,
        attrs: Vec::new(),
        generics: Vec::new(),
        receiver: None,
        params: vec![Param {
            span,
            mode: AccessMode::Take,
            name: "source".to_string(),
            ty: source_ast_ty.clone(),
            default: None,
        }],
        ret: Some(ast::Type::Named(NamedType {
            span,
            name: type_name.to_string(),
            args: Vec::new(),
        })),
        body: Some(vec![Stmt::Return(span, Some(construct))]),
    }
}

// ===========================================================================
// plans/M9.md item C2: `deriving(Format)` generates a real Format contract
// (05 §6 / 02 §7.5) — associated `max_formatted_len() -> usize` and method
// `format(read self) -> String[..N]`. Empty-spec only: `{expr:spec}` is
// comptime syntax for f-strings (item D), not a runtime parameter.
// Same DeclFn + FnItem shape as B3's `from`.
// ===========================================================================

fn derived_format_conflict(type_name: &str, span: Span) -> SemaError {
    SemaError::at(
        "type",
        format!("deriving(Format) conflicts with an explicit Format member on `{type_name}`"),
        span,
    )
}

fn format_usize_ret(span: Span) -> ast::Type {
    ast::Type::Named(NamedType {
        span,
        name: "usize".to_string(),
        args: Vec::new(),
    })
}

fn derived_max_formatted_len_decl() -> DeclFn {
    DeclFn {
        name: "max_formatted_len".to_string(),
        is_async: false,
        is_task: false,
        generics: Vec::new(),
        receiver: None,
        params: Vec::new(),
        ret: Type::Usize,
    }
}

fn derived_format_decl(bound: u64) -> DeclFn {
    DeclFn {
        name: "format".to_string(),
        is_async: false,
        is_task: false,
        generics: Vec::new(),
        receiver: Some(DeclReceiver {
            mode: AccessMode::Read,
            is_pub: true,
            is_init: false,
        }),
        params: Vec::new(),
        ret: string_bound_ty(bound),
    }
}

fn int_lit(span: Span, value: u64) -> Expr {
    Expr::Int(span, value.to_string())
}

/// A text literal in the same spelling the lexer emits (`"..."` with
/// escapes), so `eval::value::decode_str` can consume it.
fn str_lit(span: Span, text: &str) -> Expr {
    let mut raw = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => raw.push_str("\\\\"),
            '"' => raw.push_str("\\\""),
            '\n' => raw.push_str("\\n"),
            '\r' => raw.push_str("\\r"),
            '\t' => raw.push_str("\\t"),
            '\0' => raw.push_str("\\0"),
            other => raw.push(other),
        }
    }
    raw.push('"');
    Expr::Str(span, raw)
}

/// Associated `max_formatted_len() -> usize` with body `return N`.
pub(crate) fn derived_max_formatted_len_fn_item(bound: u64, span: Span) -> FnItem {
    FnItem {
        span,
        name: "max_formatted_len".to_string(),
        is_pub: true,
        is_async: false,
        doc: None,
        attrs: Vec::new(),
        generics: Vec::new(),
        receiver: None,
        params: Vec::new(),
        ret: Some(format_usize_ret(span)),
        body: Some(vec![Stmt::Return(span, Some(int_lit(span, bound)))]),
    }
}

/// Fieldless-struct `format(read self) -> String[..N]` returning the type name.
pub(crate) fn derived_format_fn_item_struct(type_name: &str, span: Span) -> FnItem {
    let bound = type_name.len() as u64;
    FnItem {
        span,
        name: "format".to_string(),
        is_pub: true,
        is_async: false,
        doc: None,
        attrs: Vec::new(),
        generics: Vec::new(),
        receiver: Some(Receiver {
            span,
            mode: AccessMode::Read,
        }),
        params: Vec::new(),
        ret: Some(string_bound_ast_ty(span, bound)),
        body: Some(vec![Stmt::Return(span, Some(str_lit(span, type_name)))]),
    }
}

/// Fieldful-struct `format`: `"Name(f1=" + self.f1.format() + ", f2=" + ... + ")"`.
pub(crate) fn derived_format_fn_item_struct_fields(
    type_name: &str,
    fields: &[(String, Type)],
    bound: u64,
    span: Span,
) -> FnItem {
    let mut expr = str_lit(span, &format!("{type_name}("));
    for (i, (fname, _)) in fields.iter().enumerate() {
        if i > 0 {
            expr = Expr::Binary(
                span,
                BinOp::Add,
                Box::new(expr),
                Box::new(str_lit(span, ", ")),
            );
        }
        expr = Expr::Binary(
            span,
            BinOp::Add,
            Box::new(expr),
            Box::new(str_lit(span, &format!("{fname}="))),
        );
        let format_call = Expr::Call(
            Box::new(Expr::Field(
                Box::new(Expr::Field(
                    Box::new(Expr::Name(span, "self".to_string())),
                    span,
                    fname.clone(),
                )),
                span,
                "format".to_string(),
            )),
            span,
            vec![],
        );
        expr = Expr::Binary(span, BinOp::Add, Box::new(expr), Box::new(format_call));
    }
    expr = Expr::Binary(
        span,
        BinOp::Add,
        Box::new(expr),
        Box::new(str_lit(span, ")")),
    );
    FnItem {
        span,
        name: "format".to_string(),
        is_pub: true,
        is_async: false,
        doc: None,
        attrs: Vec::new(),
        generics: Vec::new(),
        receiver: Some(Receiver {
            span,
            mode: AccessMode::Read,
        }),
        params: Vec::new(),
        ret: Some(string_bound_ast_ty(span, bound)),
        body: Some(vec![Stmt::Return(span, Some(expr))]),
    }
}

/// Bound for `deriving(Format)` on a struct: fieldless → name length;
/// fieldful → `Name(f1=<scalar>, f2=...)` with each scalar's Format bound.
pub(crate) fn struct_format_bound(
    type_name: &str,
    fields: &[(String, Type)],
    span: Span,
) -> Result<u64, SemaError> {
    if fields.is_empty() {
        return Ok(type_name.len() as u64);
    }
    let mut bound = type_name.len() as u64 + 1; // "Name("
    for (i, (fname, fty)) in fields.iter().enumerate() {
        if i > 0 {
            bound += 2; // ", "
        }
        bound += fname.len() as u64 + 1; // "f="
        let Some(fb) = scalar_format_bound(fty) else {
            return Err(SemaError::at(
                "type",
                format!(
                    "deriving(Format) field `{fname}` has type `{}` with no standard Format",
                    render_type(fty)
                ),
                span,
            ));
        };
        bound += fb;
    }
    Ok(bound + 1) // ")"
}

/// Unit-enum `format(read self) -> String[..N]` matching each variant name.
pub(crate) fn derived_format_fn_item_enum(variants: &[String], bound: u64, span: Span) -> FnItem {
    let arms = variants
        .iter()
        .map(|v| MatchArm {
            span,
            pattern: Pattern::Variant {
                span,
                enum_name: None,
                variant: v.clone(),
                payload: Vec::new(),
            },
            guard: None,
            body: vec![Stmt::Return(span, Some(str_lit(span, v)))],
        })
        .collect();
    FnItem {
        span,
        name: "format".to_string(),
        is_pub: true,
        is_async: false,
        doc: None,
        attrs: Vec::new(),
        generics: Vec::new(),
        receiver: Some(Receiver {
            span,
            mode: AccessMode::Read,
        }),
        params: Vec::new(),
        ret: Some(string_bound_ast_ty(span, bound)),
        body: Some(vec![Stmt::Match(MatchStmt {
            span,
            scrutinee: Expr::Name(span, "self".to_string()),
            arms,
        })]),
    }
}

/// True when a DeclFn is exactly the Format contract's associated bound
/// member: `max_formatted_len() -> usize`, no receiver.
pub(crate) fn is_format_max_formatted_len(d: &DeclFn) -> bool {
    d.name == "max_formatted_len"
        && d.receiver.is_none()
        && d.params.is_empty()
        && d.ret == Type::Usize
        && !d.is_async
}

/// True when a DeclFn is exactly the Format contract's writer:
/// `format(read self) -> String[..N]`.
pub(crate) fn is_format_writer(d: &DeclFn) -> bool {
    d.name == "format"
        && matches!(
            &d.receiver,
            Some(r) if r.mode == AccessMode::Read
        )
        && d.params.is_empty()
        && matches!(d.ret, Type::String(_))
        && !d.is_async
}

fn declare_struct(
    s: &StructItem,
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
) -> Result<DeclStruct, SemaError> {
    validate_deriving(&s.deriving, &DerivingShape::Struct(s), s.span)?;
    let mut scope = BTreeMap::new();
    let decl_generics = resolve_generics(
        &s.generics,
        shapes,
        module_pools,
        &BTreeSet::new(),
        &mut scope,
    )?;
    let local_pools: BTreeSet<String> = s
        .members
        .iter()
        .filter_map(|m| match m {
            Member::Pool(p) => Some(p.name.clone()),
            _ => None,
        })
        .collect();
    let mut members = Vec::new();
    let mut component_types = Vec::new();
    for m in &s.members {
        match m {
            Member::Field(f) => {
                let ty = resolve_type(&f.ty, shapes, module_pools, &local_pools, &scope, false)?;
                component_types.push((ty.clone(), f.span));
                members.push(DeclMember::Field(DeclField {
                    name: f.name.clone(),
                    ty,
                    is_pub: f.is_pub,
                }));
            }
            Member::Fn(f) => members.push(DeclMember::Fn(declare_fn(
                f,
                shapes,
                module_pools,
                &local_pools,
                &scope,
            )?)),
            Member::Init(i) => members.push(DeclMember::Init(declare_init(
                i,
                shapes,
                module_pools,
                &local_pools,
                &scope,
            )?)),
            Member::Pool(p) => members.push(DeclMember::Pool(p.name.clone())),
            Member::ComptimeIf(_) => {} // comptime evaluation is item C's job
        }
    }
    // plans/M9.md item B3: generate DeclFn `from` for deriving(From).
    if s.deriving.iter().any(|d| d == "From") {
        if members
            .iter()
            .any(|m| matches!(m, DeclMember::Fn(f) if f.name == "from"))
        {
            return Err(derived_from_conflict(&s.name, s.span));
        }
        let source_ty = members
            .iter()
            .find_map(|m| match m {
                DeclMember::Field(f) => Some(f.ty.clone()),
                _ => None,
            })
            .expect("validate_from_shape already required exactly one field");
        members.push(DeclMember::Fn(derived_from_decl(&s.name, source_ty)));
    }
    // plans/M9.md item C2: generate Format DeclFns for deriving(Format).
    if s.deriving.iter().any(|d| d == "Format") {
        if members.iter().any(|m| {
            matches!(
                m,
                DeclMember::Fn(f) if f.name == "format" || f.name == "max_formatted_len"
            )
        }) {
            return Err(derived_format_conflict(&s.name, s.span));
        }
        let fields: Vec<(String, Type)> = members
            .iter()
            .filter_map(|m| match m {
                DeclMember::Field(f) => Some((f.name.clone(), f.ty.clone())),
                _ => None,
            })
            .collect();
        let bound = struct_format_bound(&s.name, &fields, s.span)?;
        members.push(DeclMember::Fn(derived_max_formatted_len_decl()));
        members.push(DeclMember::Fn(derived_format_decl(bound)));
    }
    // 05 §6: Secret has no Format — catch a hand-declared contract on a
    // type spelled `Secret` (deriving already refused in validate_format_shape).
    if s.name == "Secret"
        && members.iter().any(|m| {
            matches!(
                m,
                DeclMember::Fn(f) if is_format_max_formatted_len(f) || is_format_writer(f)
            )
        })
    {
        return Err(secret_has_no_format(s.span));
    }
    Ok(DeclStruct {
        name: s.name.clone(),
        generics: decl_generics,
        deriving: s.deriving.clone(),
        classification: Classification::Data, // placeholder; classify_all fills this in
        members,
        is_resource_fiat: s.is_resource || has_actor_or_driver(&s.attrs),
        is_actor: has_actor_or_driver(&s.attrs),
        is_driver: s.attrs.iter().any(|a| a.name == "driver"),
        layout_kind: declared_layout_kind(&s.attrs),
        component_types,
        span: s.span,
    })
}

// ===========================================================================
// plans/M7.md item G, decision 18: re-declare a struct's members after
// per-instantiation `comptime if` expansion (`specialize::expand_deferred_members`).
// Generics on the result are cleared — the instantiation is concrete.
// ===========================================================================
pub(crate) fn declare_struct_members_for_instantiation(
    name: &str,
    expanded_members: &[Member],
    template: &DeclStruct,
    mctx: &crate::sema::bodies::ModuleCtx,
    call_span: Span,
) -> Result<DeclStruct, SemaError> {
    let shapes = &mctx.shapes;
    let module_pools = &mctx.module_pools;
    let local_pools: BTreeSet<String> = expanded_members
        .iter()
        .filter_map(|m| match m {
            Member::Pool(p) => Some(p.name.clone()),
            _ => None,
        })
        .collect();
    // Instantiation is concrete: no generic scope (const args already
    // expanded out of the AST; type args are substituted by the caller).
    let scope = BTreeMap::new();
    let mut members = Vec::new();
    let mut component_types = Vec::new();
    for m in expanded_members {
        match m {
            Member::Field(f) => {
                let ty = resolve_type(&f.ty, shapes, module_pools, &local_pools, &scope, false)?;
                component_types.push((ty.clone(), f.span));
                members.push(DeclMember::Field(DeclField {
                    name: f.name.clone(),
                    ty,
                    is_pub: f.is_pub,
                }));
            }
            Member::Fn(f) => members.push(DeclMember::Fn(declare_fn(
                f,
                shapes,
                module_pools,
                &local_pools,
                &scope,
            )?)),
            Member::Init(i) => members.push(DeclMember::Init(declare_init(
                i,
                shapes,
                module_pools,
                &local_pools,
                &scope,
            )?)),
            Member::Pool(p) => members.push(DeclMember::Pool(p.name.clone())),
            Member::ComptimeIf(c) => {
                return Err(SemaError::at(
                    "comptime",
                    format!(
                        "internal error: deferred `comptime if` on `{name}` survived \
                         instantiation expansion"
                    ),
                    c.span,
                ));
            }
        }
    }
    let _ = call_span;
    Ok(DeclStruct {
        name: name.to_string(),
        generics: Vec::new(),
        deriving: template.deriving.clone(),
        classification: template.classification,
        members,
        is_resource_fiat: template.is_resource_fiat,
        is_actor: template.is_actor,
        is_driver: template.is_driver,
        layout_kind: template.layout_kind,
        component_types,
        span: template.span,
    })
}

/// `@layout(<kind>, ...)`'s kind, for a struct that carries the attribute
/// at all — `None` for every ordinary struct, and `None` too for a
/// malformed `@layout` this cannot read (which `check_layouts` has
/// already rejected before `declare` ever runs: `sema::mod`'s pipeline
/// calls it first, deliberately, before name resolution). Deliberately
/// tolerant rather than a second parser: the one consumer is
/// `validate_capability_types`' own "`Mmio[L]` needs `L` to be an
/// `@layout(mmio)` type" check, and a program that reaches it has a
/// well-formed `@layout` on every struct that has one at all.
fn declared_layout_kind(attrs: &[Attr]) -> Option<LayoutKind> {
    let attr = attrs.iter().find(|a| a.name == "layout")?;
    let arg = attr.args.iter().find(|a| a.label.is_none())?;
    let Expr::Name(_, kind) = &arg.value else {
        return None;
    };
    match kind.as_str() {
        "dma" => Some(LayoutKind::Dma),
        "mmio" => Some(LayoutKind::Mmio),
        "wire" => Some(LayoutKind::Wire),
        // This table and `parse_layout_attr`'s are independent, and only
        // that one is exhaustive-checked by the compiler; a kind added
        // there and forgotten here silently yields `None`, i.e. "not a
        // layout at all". Both move together, always.
        "runtime" => Some(LayoutKind::Runtime),
        _ => None,
    }
}

fn declare_enum(
    e: &EnumItem,
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
) -> Result<DeclEnum, SemaError> {
    validate_deriving(&e.deriving, &DerivingShape::Enum(e), e.span)?;
    let mut scope = BTreeMap::new();
    let decl_generics = resolve_generics(
        &e.generics,
        shapes,
        module_pools,
        &BTreeSet::new(),
        &mut scope,
    )?;
    let mut variants = Vec::new();
    let mut component_types = Vec::new();
    let mut seen_names: BTreeSet<String> = BTreeSet::new();
    for v in &e.variants {
        if !seen_names.insert(v.name.clone()) {
            return Err(SemaError::at(
                "type",
                format!("duplicate variant `{}` on enum `{}`", v.name, e.name),
                v.span,
            ));
        }
        let payload = match &v.payload {
            VariantPayload::None => DeclVariantPayload::None,
            VariantPayload::Tuple(types) => {
                let mut rtypes = Vec::new();
                for t in types {
                    let rt =
                        resolve_type(t, shapes, module_pools, &BTreeSet::new(), &scope, false)?;
                    component_types.push((rt.clone(), t.span()));
                    rtypes.push(rt);
                }
                DeclVariantPayload::Tuple(rtypes)
            }
            VariantPayload::Named(fields) => {
                let mut rfields = Vec::new();
                for f in fields {
                    let rt =
                        resolve_type(&f.ty, shapes, module_pools, &BTreeSet::new(), &scope, false)?;
                    component_types.push((rt.clone(), f.ty.span()));
                    rfields.push((f.name.clone(), rt));
                }
                DeclVariantPayload::Named(rfields)
            }
        };
        variants.push(DeclVariant {
            name: v.name.clone(),
            payload,
        });
    }
    let mut members = Vec::new();
    for m in &e.members {
        match m {
            Member::Fn(f) => {
                if !seen_names.insert(f.name.clone()) {
                    return Err(SemaError::at(
                        "type",
                        format!(
                            "method `{}` collides with a variant or method on enum `{}`",
                            f.name, e.name
                        ),
                        f.span,
                    ));
                }
                members.push(DeclMember::Fn(declare_fn(
                    f,
                    shapes,
                    module_pools,
                    &BTreeSet::new(),
                    &scope,
                )?));
            }
            Member::Field(f) => {
                return Err(SemaError::at(
                    "type",
                    format!("an enum may not declare fields (at `{}`)", f.name),
                    f.span,
                ));
            }
            Member::Init(i) => {
                return Err(SemaError::at(
                    "type",
                    "an enum may not declare `init`".to_string(),
                    i.span,
                ));
            }
            Member::Pool(p) => {
                return Err(SemaError::at(
                    "type",
                    format!("an enum may not declare a `pool` (at `{}`)", p.name),
                    p.span,
                ));
            }
            Member::ComptimeIf(c) => {
                return Err(unimplemented_at(
                    "comptime if members on an enum are",
                    c.span,
                ));
            }
        }
    }
    // plans/M9.md item B3: generate DeclFn `from` for deriving(From).
    if e.deriving.iter().any(|d| d == "From") {
        if !seen_names.insert("from".to_string()) {
            return Err(derived_from_conflict(&e.name, e.span));
        }
        let source_ty = match &variants[0].payload {
            DeclVariantPayload::Tuple(types) => types[0].clone(),
            DeclVariantPayload::Named(fields) => fields[0].1.clone(),
            DeclVariantPayload::None => {
                unreachable!("validate_from_shape already required exactly one field")
            }
        };
        members.push(DeclMember::Fn(derived_from_decl(&e.name, source_ty)));
    }
    // plans/M9.md item C2: generate Format DeclFns for deriving(Format).
    if e.deriving.iter().any(|d| d == "Format") {
        if !seen_names.insert("max_formatted_len".to_string())
            || !seen_names.insert("format".to_string())
        {
            return Err(derived_format_conflict(&e.name, e.span));
        }
        let bound = e.variants.iter().map(|v| v.name.len()).max().unwrap_or(0) as u64;
        members.push(DeclMember::Fn(derived_max_formatted_len_decl()));
        members.push(DeclMember::Fn(derived_format_decl(bound)));
    }
    if e.name == "Secret"
        && members.iter().any(|m| {
            matches!(
                m,
                DeclMember::Fn(f) if is_format_max_formatted_len(f) || is_format_writer(f)
            )
        })
    {
        return Err(secret_has_no_format(e.span));
    }
    Ok(DeclEnum {
        name: e.name.clone(),
        generics: decl_generics,
        deriving: e.deriving.clone(),
        classification: Classification::Data, // placeholder; classify_all fills this in
        variants,
        members,
        component_types,
        span: e.span,
    })
}

/// Resolves one `ast::Type` into `Type`. `param_position` is true only
/// for a direct fn/method/init non-receiver parameter's own type — the
/// one place 02-language.md §6.2 lets a bounded type (`Bytes`, in this
/// prelude) omit its bound; every nested position (fields, consts,
/// returns, array elements, tuple elements, generic arguments, `own`'s
/// payload, a `fn(...)` type's own parameter types) resolves with it
/// false, so elision cannot smuggle itself in one level down.
pub(crate) fn resolve_type(
    ty: &ast::Type,
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
    local_pools: &BTreeSet<String>,
    generics: &BTreeMap<String, GenericKind>,
    param_position: bool,
) -> Result<Type, SemaError> {
    match ty {
        ast::Type::Named(n) => resolve_named(
            n,
            shapes,
            module_pools,
            local_pools,
            generics,
            param_position,
        ),
        ast::Type::Array(a) => {
            let elem = resolve_type(&a.elem, shapes, module_pools, local_pools, generics, false)?;
            Ok(Type::Array(Box::new(elem), Box::new(a.len.clone())))
        }
        ast::Type::Tuple(t) => {
            let mut elems = Vec::with_capacity(t.elems.len());
            for e in &t.elems {
                elems.push(resolve_type(
                    e,
                    shapes,
                    module_pools,
                    local_pools,
                    generics,
                    false,
                )?);
            }
            Ok(Type::Tuple(elems))
        }
        ast::Type::Own(o) => {
            if o.pool.len() != 1 {
                // An actor-scoped `Owner.Name` path needs actor scoping
                // (M6+) that does not exist yet — fail closed rather
                // than guess which pool it means (decision 7).
                return Err(unimplemented_at("a dotted pool path is", o.span));
            }
            let pool_name = &o.pool[0];
            if !module_pools.contains(pool_name) && !local_pools.contains(pool_name) {
                return Err(SemaError::at(
                    "type",
                    format!("unknown pool `{pool_name}`"),
                    o.span,
                ));
            }
            let inner = resolve_type(&o.inner, shapes, module_pools, local_pools, generics, false)?;
            Ok(Type::Own(pool_name.clone(), Box::new(inner)))
        }
        ast::Type::Fn(f) => {
            let mut params = Vec::with_capacity(f.params.len());
            for p in &f.params {
                let t = resolve_type(&p.ty, shapes, module_pools, local_pools, generics, false)?;
                params.push((p.mode, t));
            }
            let ret = match &f.ret {
                Some(r) => resolve_type(r, shapes, module_pools, local_pools, generics, false)?,
                None => Type::Unit,
            };
            Ok(Type::Fn(params, Box::new(ret)))
        }
    }
}

fn expect_arity(n: &NamedType, expected: usize) -> Result<(), SemaError> {
    if n.args.len() != expected {
        return Err(SemaError::at(
            "type",
            format!(
                "`{}` expects {expected} generic argument(s), found {}",
                n.name,
                n.args.len()
            ),
            n.span,
        ));
    }
    Ok(())
}

/// `expected` positional generic-arguments, each required to be a plain
/// type (used only by `Option`/`Result`/`Static`, whose fixed prelude
/// arity is never a const — decision 5).
fn expect_type_args<'a>(
    n: &'a NamedType,
    expected: usize,
) -> Result<Vec<&'a ast::Type>, SemaError> {
    expect_arity(n, expected)?;
    let mut out = Vec::with_capacity(expected);
    for a in &n.args {
        match a {
            GenericArg::Type(t) => out.push(t),
            GenericArg::Expr(_) | GenericArg::Bound(_) => {
                return Err(SemaError::at(
                    "type",
                    format!("`{}` requires a type argument", n.name),
                    n.span,
                ));
            }
        }
    }
    Ok(out)
}

/// `Bytes[N]` / bare `Bytes` (02-language.md §6.2, plans/M2.md decision
/// 5 — only the exact form ships in the M2 prelude, not `Bytes[..N]`).
fn resolve_bytes(n: &NamedType, param_position: bool) -> Result<Type, SemaError> {
    if n.args.is_empty() {
        return if param_position {
            Ok(Type::Bytes(None))
        } else {
            Err(SemaError::at(
                "type",
                "`Bytes` needs an explicit length outside parameter position".to_string(),
                n.span,
            ))
        };
    }
    expect_arity(n, 1)?;
    match &n.args[0] {
        GenericArg::Expr(e) => Ok(Type::Bytes(Some(Box::new(e.clone())))),
        // A bare identifier (a const generic parameter or a `const`
        // item) parses as a type-shaped argument — the grammar cannot
        // tell a value name from a type name — so it is unwrapped back
        // into the length expression it actually names.
        GenericArg::Type(ast::Type::Named(inner)) if inner.args.is_empty() => Ok(Type::Bytes(
            Some(Box::new(Expr::Name(inner.span, inner.name.clone()))),
        )),
        GenericArg::Type(_) => Err(SemaError::at(
            "type",
            "`Bytes[N]` needs a length, not a type".to_string(),
            n.span,
        )),
        GenericArg::Bound(_) => Err(unimplemented_at("`Bytes[..N]` (bounded) is", n.span)),
    }
}

/// `String[..N]` (02-language.md §6.2 / plans/M9.md item C1). Exact
/// `String[N]` and bare `String` are refused by name — the `..N`
/// spelling is the bounded-occupancy form, and bound-elision is a
/// separate surface this item does not grow.
fn resolve_string(n: &NamedType) -> Result<Type, SemaError> {
    if n.args.is_empty() {
        return Err(unimplemented_at("`String` (bound-elided) is", n.span));
    }
    expect_arity(n, 1)?;
    match &n.args[0] {
        GenericArg::Bound(e) => {
            // plans/M9.md item K1: refuse a literal capacity the layout
            // fn cannot represent (same rule as concat / f-string sums).
            if let Some(cap) = crate::sema::bodies::literal_array_len(e) {
                if !crate::sema::bodies::string_capacity_fits(cap) {
                    return Err(SemaError::at(
                        "type",
                        format!(
                            "`String[..N]` capacity {cap} exceeds the {}-element build limit",
                            crate::sema::bodies::MAX_STRING_CAPACITY
                        ),
                        n.span,
                    ));
                }
            }
            Ok(Type::String(Box::new(e.clone())))
        }
        // `String[CAP]` (no `..`) — exact form, refused by name.
        // Bound form `String[..CAP]` is always `GenericArg::Bound`.
        GenericArg::Type(ast::Type::Named(_inner)) if _inner.args.is_empty() => Err(SemaError::at(
            "type",
            "`String[..N]` needs a bounded-occupancy argument (`..N`), not `String[N]`".to_string(),
            n.span,
        )),
        GenericArg::Expr(_) => Err(SemaError::at(
            "type",
            "`String[..N]` needs a bounded-occupancy argument (`..N`), not `String[N]`".to_string(),
            n.span,
        )),
        GenericArg::Type(_) => Err(SemaError::at(
            "type",
            "`String[..N]` needs a capacity bound, not a type".to_string(),
            n.span,
        )),
    }
}

/// Compiler-known type names that resolve with no import
/// (plans/M9.md item I). These are the annotation-position half of what
/// `sema/prelude.rs` used to list; each name already has its real
/// definition in [`resolve_named`] (or in `eval::image_checks` for
/// sealed-authority types). Value-only names (`Some`, `group`, …) are
/// *not* here — see `symbols::is_resolvable_without_import`.
pub fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        // 02 §6.1 scalars + `Str` (literal surface).
        "bool"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "isize"
            | "f32"
            | "f64"
            | "char"
            | "unit"
            | "never"
            | "Str"
            // 02 §2 fixed prelude types.
            | "Option"
            | "Result"
            // Literal / string surface (02 §1.1 / §6.2).
            | "Static"
            | "Bytes"
            | "String"
            // Image builder opaque type (05 §9).
            | "Image"
            // Actor / hardware / MMIO / marked-value / interrupt surface.
            | "Actor"
            | "BootError"
            | "VirtQueue"
            | "QueuePermit"
            | "QueueOp"
            | "Receipt"
            | "IoCompletion"
            | "CompletionOutcome"
            | "Target"
            | "Failure"
            | "DriverMode"
            | "ReadOnly"
            | "WriteOnly"
            | "Untrusted"
            | "Validated"
            | "Secret"
            | "InterruptCell"
            // Time types stay annotation-resolvable without an import
            // (plans/M9.md item E decision 300 / item I decision 470).
            | "Duration"
            | "Instant"
            // plans/M10.md item E2 / decision 669: 1-based group arena
            // index; `Option[GroupId]` niche at 0.
            | "GroupId"
    ) || crate::eval::image_checks::is_sealed_authority_type_name(name)
}

fn resolve_named(
    n: &NamedType,
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
    local_pools: &BTreeSet<String>,
    generics: &BTreeMap<String, GenericKind>,
    param_position: bool,
) -> Result<Type, SemaError> {
    let scalar = match n.name.as_str() {
        "bool" => Some(Type::Bool),
        "u8" => Some(Type::U8),
        "u16" => Some(Type::U16),
        "u32" => Some(Type::U32),
        "u64" => Some(Type::U64),
        "usize" => Some(Type::Usize),
        "i8" => Some(Type::I8),
        "i16" => Some(Type::I16),
        "i32" => Some(Type::I32),
        "i64" => Some(Type::I64),
        "isize" => Some(Type::Isize),
        "f32" => Some(Type::F32),
        "f64" => Some(Type::F64),
        "char" => Some(Type::Char),
        "unit" => Some(Type::Unit),
        "never" => Some(Type::Never),
        "Str" => Some(Type::Str),
        // The `@image` builder's own opaque resource type (plans/M4.md
        // item B, decision 5: "opaque builtin resource types"), needed
        // only so an `@image fn`'s declared `-> Image` return type
        // resolves (02-language.md §12.1) — recognized here exactly like
        // every other zero-argument prelude name above, not backed by a
        // real declared struct. `img.driver`/`img.actor`/`img.device`/
        // `img.pool`/`img.dma_pool`/`decl.handle()` all resolve to the
        // builder surface's *other* opaque type, `ImageDecl` — recognized
        // by `sema::bodies`'s own intrinsic dispatch directly (never
        // written by source, so it never needs a annotation-position
        // resolution here).
        "Image" => Some(Type::Named("Image".to_string(), vec![])),
        // plans/M7.md item E1: 03-hardware.md §1/§9's `BootError` — a
        // zero-argument prelude enum (variants in `builtin_enum_variants`).
        "BootError" => Some(Type::Named("BootError".to_string(), vec![])),
        // plans/M7.md item E2: sealed permit (zero type arguments).
        // `QueueOp` carries its payload brand as of E3 — see the match
        // arm below.
        "QueuePermit" => Some(Type::Named(n.name.clone(), vec![])),
        // plans/M7.md item E3: `IoError` lived here as a prelude enum; at
        // plans/M9.md item A2 it moved to `stdlib/core/io_error.wr` and
        // resolves through the ordinary imported-type path (A1).
        // =================================================================
        // plans/M7.md item G, decision 18: prelude enums as annotation
        // types (`const MODE: DriverMode`). Same zero-arg Named shape as
        // `Image`; variants live in `builtin_enum_variants`.
        // =================================================================
        "DriverMode" | "Target" | "Failure" => Some(Type::Named(n.name.clone(), vec![])),
        // plans/M8.md item G, decision 17: 03-hardware.md §9's
        // `CompletionOutcome` — same zero-argument prelude-enum shape,
        // minted only by `VirtQueue.recover` but nameable in an
        // annotation (a helper that classifies one takes it by value).
        "CompletionOutcome" => Some(Type::Named("CompletionOutcome".to_string(), vec![])),
        // plans/M10.md item E2 / decision 669: opaque 1-based group arena
        // index. Zero-argument Named, like `Image` — not a DeclStruct, so
        // source cannot forge one by field init (decision 567's niche
        // stays unconstructible from source until a minting site lands).
        "GroupId" => Some(Type::Named("GroupId".to_string(), vec![])),
        _ => None,
    };
    if let Some(t) = scalar {
        expect_arity(n, 0)?;
        return Ok(t);
    }
    match n.name.as_str() {
        "Option" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Option(Box::new(inner)));
        }
        "Result" => {
            let args = expect_type_args(n, 2)?;
            let ok = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            let err = resolve_type(args[1], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Result(Box::new(ok), Box::new(err)));
        }
        "Static" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Static(Box::new(inner)));
        }
        "Bytes" => return resolve_bytes(n, param_position),
        // plans/M9.md item C1: `String[..N]` (02 §6.2).
        "String" => return resolve_string(n),
        // plans/M7.md item E2/E3: `QueueOp[P]` — sealed prepared operation
        // carrying the transfer-payload brand `P` so `publish` can yield
        // `Receipt[P]`. `prepare_block` always produces the branded form.
        // plans/M8.md item G, decision 18 grows it to
        // `QueueOp[P, <idempotent>]`: 03-hardware.md §9's no-auto-retry
        // rule needs the author's idempotence declaration to survive from
        // the `prepare_block` that made the operation to the `publish` that
        // issues it — including across a helper's signature — so it is part
        // of the operation's type, not a fact about one call site.
        "QueueOp" => {
            expect_arity(n, 2)?;
            let GenericArg::Type(payload) = &n.args[0] else {
                return Err(SemaError::at(
                    "type",
                    "`QueueOp[P, <idempotent>]`'s first argument is the transfer-payload type"
                        .to_string(),
                    n.span,
                ));
            };
            let inner = resolve_type(payload, shapes, module_pools, local_pools, generics, false)?;
            let idempotent = match &n.args[1] {
                GenericArg::Expr(Expr::Bool(_, v)) => *v,
                _ => {
                    return Err(SemaError::at(
                        "type",
                        "`QueueOp[P, <idempotent>]`'s second argument is the literal `true` or \
                         `false` — 03-hardware.md §9's no-auto-retry rule reads it off the \
                         operation's own type, and the compiler cannot infer it"
                            .to_string(),
                        n.span,
                    ));
                }
            };
            return Ok(Type::Named(
                "QueueOp".to_string(),
                vec![
                    TypeArg::Type(inner),
                    TypeArg::Const(Expr::Bool(ast::Span::default(), idempotent)),
                ],
            ));
        }
        // plans/M7.md item E3: `Receipt[P]` (03-hardware.md §5) — sealed
        // resource state machine. `P` is the payload brand the receipt
        // recovers; minted only by `publish` / `reject` / handoff
        // admission.
        "Receipt" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named(
                "Receipt".to_string(),
                vec![TypeArg::Type(inner)],
            ));
        }
        // plans/M7.md item E4: `IoCompletion[P]` (03-hardware.md §3/§8) —
        // resolved receipt: payload ownership returns, `status` is the
        // device/protocol outcome, `written_len` is the Untrusted
        // producer for checked narrowing.
        "IoCompletion" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named(
                "IoCompletion".to_string(),
                vec![TypeArg::Type(inner)],
            ));
        }
        // `Actor[T]` (plans/M6.md item A, 02-language.md §9.1): the
        // generated handle type — `T` is structurally resolved here
        // exactly like `Option`/`Static`'s own inner argument; *which*
        // structs `T` may legally name (`@actor`/`@driver` only) is a
        // whole-module question this per-annotation resolver cannot ask
        // (a forward reference to a struct declared later in the file is
        // legal, mirroring `shapes`'s own forward-reference story) — validated
        // once, after every item is declared, by `validate_actor_handles`
        // below (called from `declare`). Retires the M4-C placeholder
        // comment in golden `image-basic` (`Store.disk`'s own `u32` stand-in).
        "Actor" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named("Actor".to_string(), vec![TypeArg::Type(inner)]));
        }
        // plans/M7.md item E1: `VirtQueue[..N]` (03-hardware.md §4) —
        // bounded occupancy, the same `..N` spelling 05-library.md §10
        // reserves for "bounded-occupancy parameters". The depth is a
        // const expression (a literal or a module `const`); it must be a
        // nonzero power of two at the configure site / report emission,
        // not here (this resolver has no const evaluator).
        "VirtQueue" => {
            expect_arity(n, 1)?;
            match &n.args[0] {
                GenericArg::Bound(e) => {
                    return Ok(Type::Named(
                        "VirtQueue".to_string(),
                        vec![TypeArg::Bound(e.clone())],
                    ));
                }
                GenericArg::Expr(e) => {
                    return Ok(Type::Named(
                        "VirtQueue".to_string(),
                        vec![TypeArg::Bound(e.clone())],
                    ));
                }
                GenericArg::Type(ast::Type::Named(inner)) if inner.args.is_empty() => {
                    // `VirtQueue[..QDEPTH]` where QDEPTH is a const name
                    // parses as a type-shaped argument; unwrap it.
                    return Ok(Type::Named(
                        "VirtQueue".to_string(),
                        vec![TypeArg::Bound(Expr::Name(inner.span, inner.name.clone()))],
                    ));
                }
                _ => {
                    return Err(SemaError::at(
                        "type",
                        "`VirtQueue[..N]` needs a depth bound (03-hardware.md §4 / 05-library.md \
                         §10: bounded occupancy is spelled `..N`)"
                            .to_string(),
                        n.span,
                    ));
                }
            }
        }
        // `ReadOnly[T]`/`WriteOnly[T]` (plans/M7.md item B, 03-hardware.md
        // §2): the typed-MMIO register wrappers, resolved structurally
        // exactly like `Actor[T]` above — *where* they are legal (only an
        // `@layout(mmio)` field) is a whole-declaration question this
        // per-annotation resolver cannot ask, and `check_layouts` (below)
        // already asks it before this pass ever runs. Their access rules
        // are item C; nothing here gives them one.
        "ReadOnly" | "WriteOnly" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named(n.name.clone(), vec![TypeArg::Type(inner)]));
        }
        // plans/M7.md item H2a, 03-hardware.md §8: one marked-value
        // mechanism, three policies. `Untrusted[T]` is the only one M7's
        // honest-scope line keeps IN; `Validated[F, T]` and `Secret[T]`
        // are recognized here so the refusal is the mechanism rejecting
        // an unimplemented policy by name, not an unknown-type miss.
        "Untrusted" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named(
                "Untrusted".to_string(),
                vec![TypeArg::Type(inner)],
            ));
        }
        "Validated" => {
            // Arity is still checked so a wrong-shape spell reports that
            // first; the policy refusal is what a well-shaped name gets.
            let _args = expect_type_args(n, 2)?;
            return Err(SemaError::at(
                "type",
                "the marked-value mechanism refuses policy `Validated[F, T]` — plans/M9.md \
                 item G2 defers it (decision 353): needs `FormatValidator[F, T].validate` and \
                 `into_value(take self)` (05-library.md §6); only `Untrusted[T]` is live \
                 (03-hardware.md §8)"
                    .to_string(),
                n.span,
            ));
        }
        "Secret" => {
            let _args = expect_type_args(n, 1)?;
            return Err(SemaError::at(
                "type",
                "the marked-value mechanism refuses policy `Secret[T]` — plans/M9.md item G3 \
                 defers it (decision 354): needs secret-preserving transforms and the comptime \
                 non-serialization rule (02-language.md §12); only `Untrusted[T]` is live \
                 (03-hardware.md §8)"
                    .to_string(),
                n.span,
            ));
        }
        // plans/M7.md item G, decision 17: `InterruptCell[T]` — 03 §6's
        // sole ISR/ordinary-code channel. `T` is structurally resolved
        // here; which `T` is legal (`u32` today) is asked at the
        // constructor / method site in `bodies`, not here.
        "InterruptCell" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named(
                "InterruptCell".to_string(),
                vec![TypeArg::Type(inner)],
            ));
        }
        _ => {}
    }
    // 03-hardware.md §1's four capability types (plans/M7.md item A):
    // `DeviceCap[D]`, `Mmio[L]`, `IrqCap[V]`, `DmaPool[P, N]`. Arity comes
    // from the one shared list (`eval::image_checks::CAPABILITY_TYPES`);
    // each argument resolves through the *general* `resolve_type_arg`, not
    // `expect_type_args`, so a type argument stays a type and a const
    // argument (`DmaPool[BlockControl, 256.KiB]`'s own `N`) stays an
    // unevaluated const expression — exactly what a user struct's own
    // generic arguments already do. Nothing is invented about what the
    // arguments *mean*: `Mmio[L]`'s "`L` is a `@layout(mmio)` type" is a
    // whole-module question, asked once by `validate_capability_types`
    // below (the same split `Actor[T]` already uses), and `IrqCap[V]`'s
    // vector and `DmaPool`'s pool identity belong to plans/M7.md items G
    // and D, which are where they first mean anything.
    //
    // Resolving here is the *whole* of "the type exists". Being spellable
    // is not being constructible: 03 §1's "their constructors are not
    // source-visible" is enforced separately and by name — a declaration
    // or import taking one of these names (`symbols::collect`,
    // `imports::resolve_imports`), a construction or cast attempt
    // (`bodies`' own arms), a fn claiming to return one, an `@actor`
    // holding one (`validate_capability_types`).
    if let Some(arity) = crate::eval::image_checks::capability_generic_arity(&n.name) {
        expect_arity(n, arity)?;
        // plans/M7.md item D: `DmaPool[P, N]` and `DmaShared[P, L]` name a
        // bound **pool** in argument position 0 — 03-hardware.md §1's own
        // worked driver constructor is
        // `take pool: DmaPool[BlockControl, 256.KiB]`, and `BlockControl`
        // there is the same `pool Name` declaration `own[BlockControl] T`
        // names elsewhere in that example. `own[P] T` has dedicated ast
        // syntax for that identifier; these two do not, so position 0 is
        // resolved against the declared pool names instead of the type
        // table. Every other argument of every capability type resolves
        // through the general `resolve_type_arg`, so a type argument stays
        // a type and a const argument (`N`) stays an unevaluated const
        // expression.
        let pool_first = matches!(n.name.as_str(), "DmaPool" | "DmaShared");
        let mut targs = Vec::with_capacity(n.args.len());
        for (i, a) in n.args.iter().enumerate() {
            if pool_first && i == 0 {
                targs.push(resolve_pool_type_arg(
                    &n.name,
                    a,
                    module_pools,
                    local_pools,
                )?);
                continue;
            }
            targs.push(resolve_type_arg(
                a,
                shapes,
                module_pools,
                local_pools,
                generics,
            )?);
        }
        return Ok(Type::Named(n.name.clone(), targs));
    }
    // 03-hardware.md §9's bring-up chain, as types (plans/M7.md item H1):
    // `ResetDevice[D]` .. `RunningDevice[D]`, one name per state, each
    // taking the device type. Resolved exactly like a capability above
    // (structurally, arity-checked, nothing invented about what `D`
    // means); *which* of them a transition produces is
    // `sema::bodies::check_device_transition`'s question, not this
    // resolver's, and none of them is constructible from source.
    if crate::eval::image_checks::is_protocol_state_type_name(&n.name) {
        expect_arity(n, 1)?;
        let args = expect_type_args(n, 1)?;
        let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
        return Ok(Type::Named(n.name.clone(), vec![TypeArg::Type(inner)]));
    }
    if let Some(kind) = generics.get(&n.name) {
        if !n.args.is_empty() {
            return Err(SemaError::at(
                "type",
                format!("generic parameter `{}` cannot take arguments", n.name),
                n.span,
            ));
        }
        return match kind {
            GenericKind::Type => Ok(Type::Generic(n.name.clone())),
            GenericKind::Const => Err(SemaError::at(
                "type",
                format!("`{}` is a const parameter, not a type", n.name),
                n.span,
            )),
        };
    }
    if let Some(&generic_count) = shapes.get(&n.name) {
        if n.args.len() != generic_count {
            return Err(SemaError::at(
                "type",
                format!(
                    "`{}` expects {generic_count} generic argument(s), found {}",
                    n.name,
                    n.args.len()
                ),
                n.span,
            ));
        }
        let mut targs = Vec::with_capacity(n.args.len());
        for a in &n.args {
            targs.push(resolve_type_arg(
                a,
                shapes,
                module_pools,
                local_pools,
                generics,
            )?);
        }
        return Ok(Type::Named(n.name.clone(), targs));
    }
    Err(SemaError::at(
        "type",
        format!("unknown type `{}`", n.name),
        n.span,
    ))
}

/// One generic argument at a user struct/enum's use site. Structural
/// only (item H checks/instantiates): a real type resolves recursively;
/// a bare identifier naming an in-scope const generic is unwrapped into
/// its const expression exactly like `Bytes[N]` above, for the same
/// grammar-ambiguity reason, rather than rejected as "not a type".
/// `DmaPool[P, N]`/`DmaShared[P, L]`'s own argument position 0
/// (plans/M7.md item D): a bare identifier that must name a `pool`
/// declared in this module or this fn's own scope — the identical set
/// `own[P] T` resolves its pool against, and the identical
/// module-scoped-only limitation (a pool declared in another module does
/// not resolve here yet, exactly as it does not there).
fn resolve_pool_type_arg(
    ctor: &str,
    a: &GenericArg,
    module_pools: &BTreeSet<String>,
    local_pools: &BTreeSet<String>,
) -> Result<TypeArg, SemaError> {
    let GenericArg::Type(ast::Type::Named(inner)) = a else {
        return Err(SemaError::at(
            "type",
            format!(
                "`{ctor}`'s first argument names the DMA pool it is authority over \
                 (03-hardware.md §1/§3), which is a `pool` declaration, not a value"
            ),
            span_of_generic_arg(a),
        ));
    };
    if !inner.args.is_empty() {
        return Err(SemaError::at(
            "type",
            format!(
                "`{ctor}`'s first argument names a `pool` declaration, which takes no generic \
                 arguments of its own"
            ),
            inner.span,
        ));
    }
    if !module_pools.contains(&inner.name) && !local_pools.contains(&inner.name) {
        return Err(SemaError::at(
            "type",
            format!(
                "unknown pool `{}` — `{ctor}`'s first argument names a `pool` declaration \
                 (02-language.md §4), the same identifier `own[{}] T` would name",
                inner.name, inner.name
            ),
            inner.span,
        ));
    }
    Ok(TypeArg::Pool(inner.name.clone()))
}

fn span_of_generic_arg(a: &GenericArg) -> Span {
    match a {
        GenericArg::Type(t) => t.span(),
        GenericArg::Expr(e) | GenericArg::Bound(e) => e.span(),
    }
}

fn resolve_type_arg(
    a: &GenericArg,
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
    local_pools: &BTreeSet<String>,
    generics: &BTreeMap<String, GenericKind>,
) -> Result<TypeArg, SemaError> {
    match a {
        GenericArg::Type(ast::Type::Named(inner))
            if inner.args.is_empty() && generics.get(&inner.name) == Some(&GenericKind::Const) =>
        {
            Ok(TypeArg::Const(Expr::Name(inner.span, inner.name.clone())))
        }
        GenericArg::Type(t) => Ok(TypeArg::Type(resolve_type(
            t,
            shapes,
            module_pools,
            local_pools,
            generics,
            false,
        )?)),
        GenericArg::Expr(e) => Ok(TypeArg::Const(e.clone())),
        GenericArg::Bound(e) => Ok(TypeArg::Bound(e.clone())),
    }
}

// --- data-vs-resource classification --------------------------------------
//
// 02-language.md §3, §7.1: `resource struct`/`@actor`/`@driver` is a
// resource by fiat; `own[P] T` is always a resource (a pool handle,
// regardless of `T`); `Static[T]` is always data (a copyable handle,
// regardless of `T`); everything else is a resource exactly when some
// component is, transitively. `own`/`Static` are themselves the only
// indirection M2's type system has, so recursing into their payload
// is deliberately skipped below, both for classification (fixed either
// way) and for the infinite-size check (a pool handle's fixed layout is
// exactly what makes a self-referential `own[P] Self` field finite) —
// every other composite (Named struct/enum, array, tuple, Option,
// Result) recurses. A bare generic type parameter's real classification
// depends on the concrete instantiation (item H); until then it is
// conservatively treated as data.
//
// A cycle found while recursing through plain (non-`own`/`Static`)
// composition is reported at the innermost field/variant whose
// reference closed the loop — `error[type]: ... is infinitely sized`
// (plans/M2.md item B): recursion through generic instantiation is not
// caught here (that needs item H's monomorphization; its own depth cap
// is where that class of cycle is meant to be rejected).

/// One classifiable declaration, borrowed out of a `DeclItem` — the only
/// three facts classification reads. Extracting them means the one
/// classification algorithm below runs over a single module and over a
/// whole build closure without being written twice (plans/M9.md item A1,
/// decision 10).
struct ClassifyNode<'a> {
    is_resource_fiat: bool,
    component_types: &'a [(Type, Span)],
    span: Span,
}

/// Module address -> that module's classifiable declarations, in source
/// order. The single-module caller uses one entry under the empty key.
type ClassifyTables<'a> = BTreeMap<Vec<String>, Vec<(String, ClassifyNode<'a>)>>;

/// Module address -> local type name -> `(exporting module, exported
/// name)`, for every `struct`/`enum` that module imports
/// (`imports::imported_type_targets`). Empty for a single-module build.
pub type ImportedTypeTargets = BTreeMap<Vec<String>, BTreeMap<String, (Vec<String>, String)>>;

fn classify_nodes(items: &[DeclItem]) -> Vec<(String, ClassifyNode<'_>)> {
    let mut out = Vec::new();
    for item in items {
        match item {
            DeclItem::Struct(s) => out.push((
                s.name.clone(),
                ClassifyNode {
                    is_resource_fiat: s.is_resource_fiat,
                    component_types: &s.component_types,
                    span: s.span,
                },
            )),
            DeclItem::Enum(e) => out.push((
                e.name.clone(),
                ClassifyNode {
                    // An enum is never a resource by fiat (02-language.md
                    // §3): only its payloads can make it one.
                    is_resource_fiat: false,
                    component_types: &e.component_types,
                    span: e.span,
                },
            )),
            _ => {}
        }
    }
    out
}

/// The whole answer, keyed `(module address, type name)`.
type ClassifyMemo = BTreeMap<(Vec<String>, String), Classification>;

/// Runs the classification over `tables`, visiting modules in BTree order
/// and declarations in source order within each — the same fail-fast,
/// deterministic discipline every other whole-program pass uses.
fn classify_core(
    tables: &ClassifyTables<'_>,
    imports: &ImportedTypeTargets,
) -> Result<ClassifyMemo, SemaError> {
    let mut memo = ClassifyMemo::new();
    let mut in_progress = BTreeSet::new();
    for (mkey, nodes) in tables {
        for (name, node) in nodes {
            classify_named(
                mkey,
                name,
                node.span,
                tables,
                imports,
                &mut memo,
                &mut in_progress,
            )?;
        }
    }
    Ok(memo)
}

fn write_back(items: &mut [DeclItem], mkey: &[String], memo: &ClassifyMemo) {
    for item in items.iter_mut() {
        match item {
            DeclItem::Struct(s) => {
                s.classification = memo[&(mkey.to_vec(), s.name.clone())];
            }
            DeclItem::Enum(e) => {
                e.classification = memo[&(mkey.to_vec(), e.name.clone())];
            }
            _ => {}
        }
    }
}

fn classify_all(items: &mut [DeclItem]) -> Result<(), SemaError> {
    let key: Vec<String> = Vec::new();
    let memo = {
        let tables: ClassifyTables<'_> = BTreeMap::from([(key.clone(), classify_nodes(items))]);
        classify_core(&tables, &ImportedTypeTargets::new())?
    };
    write_back(items, &key, &memo);
    Ok(())
}

/// Whole-closure data-vs-resource classification (plans/M9.md item A1,
/// decision 10). `declare_with_imports` already classified each module on
/// its own, where a field of an *imported* type is invisible and falls
/// through to `Data`; this recomputes every module's answer with the whole
/// closure in view, so a local struct holding a field of an imported
/// `resource`/`@actor` type is the resource it actually is.
///
/// It does not weaken the cycle property `sema::check_program_typed`
/// documents: every module's own `declare` still completes with nothing
/// from any other module, and this pass — like the splice — runs
/// afterwards, over output that already exists regardless of which module
/// imports which. A value cycle that closes *across* modules gets the same
/// `is infinitely sized (recursive by value)` diagnostic it already gets
/// within one, because `in_progress` is keyed by `(module, name)` and the
/// recursion follows imports.
pub fn classify_closure(
    items: &mut BTreeMap<Vec<String>, Vec<DeclItem>>,
    imports: &ImportedTypeTargets,
) -> Result<(), SemaError> {
    let memo = {
        let tables: ClassifyTables<'_> = items
            .iter()
            .map(|(k, v)| (k.clone(), classify_nodes(v)))
            .collect();
        classify_core(&tables, imports)?
    };
    for (mkey, decls) in items.iter_mut() {
        write_back(decls, mkey, &memo);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn classify_named(
    mkey: &[String],
    name: &str,
    call_span: Span,
    tables: &ClassifyTables<'_>,
    imports: &ImportedTypeTargets,
    memo: &mut ClassifyMemo,
    in_progress: &mut BTreeSet<(Vec<String>, String)>,
) -> Result<Classification, SemaError> {
    let key = (mkey.to_vec(), name.to_string());
    if let Some(c) = memo.get(&key) {
        return Ok(*c);
    }
    if in_progress.contains(&key) {
        return Err(SemaError::at(
            "type",
            format!("`{name}` is infinitely sized (recursive by value)"),
            call_span,
        ));
    }
    let local = tables
        .get(mkey)
        .and_then(|nodes| nodes.iter().find(|(n, _)| n == name).map(|(_, d)| d));
    in_progress.insert(key.clone());
    let resource;
    if let Some(d) = local {
        let mut r = d.is_resource_fiat;
        for (ty, span) in d.component_types {
            if classify_type(ty, mkey, *span, tables, imports, memo, in_progress)?
                == Classification::Resource
            {
                r = true;
            }
        }
        resource = r;
    } else if let Some((tmod, tname)) = imports.get(mkey).and_then(|m| m.get(name)) {
        // plans/M9.md item A1: the name is not declared here, it is
        // imported. Follow it into the exporting module's own already-built
        // declarations — the same read-only reuse of another module's
        // finished output the splice performs, and the reason this pass
        // cannot live inside `declare`.
        let c = classify_named(tmod, tname, call_span, tables, imports, memo, in_progress)?;
        in_progress.remove(&key);
        memo.insert(key, c);
        return Ok(c);
    } else if crate::eval::image_checks::is_sealed_authority_type_name(name) {
        // 03-hardware.md §1, its own first words: hardware operations
        // require "unforgeable **resource** values". A capability is a
        // resource by fiat, exactly like `@actor`/`@driver`/`resource
        // struct` above — it is never copied, and a struct that holds one
        // is a resource too (which is how `@actor`'s own containment rule
        // and the provenance walk both see through a plain wrapper
        // struct). Recorded here rather than left to the builtin
        // fall-through below, whose whole premise ("every one of these is
        // plain data") is exactly what stops being true for these names.
        //
        // plans/M7.md item H1: 03 §9's seven bring-up states join the same
        // arm. A protocol state *is* the device's authority in a
        // particular position of the chain — 03 §9's "each fallible
        // transition **consumes** its input state" is precisely the
        // resource rule, and the only reason a transition can consume one
        // is that it is never implicitly copied.
        in_progress.remove(&key);
        memo.insert(key, Classification::Resource);
        return Ok(Classification::Resource);
    } else {
        // Neither a declared struct nor enum: a builtin `Type::Named`
        // this module resolves without a backing declaration (plans/
        // M4.md item B, decision 5 — `Image`, and `sema::bodies`'s own
        // `ImageDecl`/`Duration` intrinsic-surface
        // pseudo-types, none registered here since nothing declares
        // them). Every one of these is plain data (never a resource
        // fiat, never composed from one), so this falls through to the
        // same `Classification::Data` a genuinely field-less struct
        // would get — not `unreachable!()`, since `resolve_named` (this
        // file) now legitimately produces such a name.
        in_progress.remove(&key);
        memo.insert(key, Classification::Data);
        return Ok(Classification::Data);
    }
    in_progress.remove(&key);
    let result = if resource {
        Classification::Resource
    } else {
        Classification::Data
    };
    memo.insert(key, result);
    Ok(result)
}

/// The one exhaustive (no wildcard) compound-propagation rule shared by
/// `classify_type` below and `bodies::is_resource_type`: a resource
/// propagates through `own[..]`, arrays, tuples, `Option`, and `Result`;
/// every scalar/`Fn`/`Generic`/`Static` variant is always data. The only
/// thing that differs between the two callers is how a *named* type's own
/// resource-ness is determined, so that single question is the seam
/// (`named_is_resource`) — `classify_type` answers it with a memoized,
/// cycle-checked recursive classification (`classify_named`, which can
/// fail on a genuinely infinite-by-value type); `is_resource_type` answers
/// it with a plain lookup against already-classified structs/enums in
/// `mctx` and can never fail. Being exhaustive here (not `_ => false`)
/// means a newly added `Type` variant forces a decision at this one triage
/// point instead of both callers silently (and possibly divergently)
/// defaulting it to data. Every subtree is always visited, never
/// short-circuited, so `classify_type`'s cycle detection/memoization (which
/// needs every referenced type visited, not just enough to answer `bool`)
/// gets it for free; `is_resource_type`'s `named_is_resource` has no side
/// effects to lose by the same non-short-circuit traversal.
pub(crate) fn resource_propagates(
    ty: &Type,
    named_is_resource: &mut dyn FnMut(&str, &[TypeArg]) -> bool,
) -> bool {
    match ty {
        Type::Own(..) => true,
        Type::Static(_) => false,
        Type::Named(name, args) => named_is_resource(name, args),
        Type::Array(elem, _) => resource_propagates(elem, named_is_resource),
        Type::Tuple(elems) => {
            let mut r = false;
            for e in elems {
                if resource_propagates(e, named_is_resource) {
                    r = true;
                }
            }
            r
        }
        Type::Option(inner) => resource_propagates(inner, named_is_resource),
        Type::Result(ok, err) => {
            let a = resource_propagates(ok, named_is_resource);
            let b = resource_propagates(err, named_is_resource);
            a || b
        }
        Type::Generic(_) => false,
        Type::Fn(..)
        | Type::Bytes(_)
        | Type::String(_)
        | Type::Str
        | Type::Bool
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::Isize
        | Type::F32
        | Type::F64
        | Type::Char
        | Type::Unit
        | Type::Never => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn classify_type(
    ty: &Type,
    mkey: &[String],
    span: Span,
    tables: &ClassifyTables<'_>,
    imports: &ImportedTypeTargets,
    memo: &mut ClassifyMemo,
    in_progress: &mut BTreeSet<(Vec<String>, String)>,
) -> Result<Classification, SemaError> {
    let mut error: Option<SemaError> = None;
    let is_resource = resource_propagates(ty, &mut |name, _args| {
        if error.is_some() {
            return false;
        }
        match classify_named(mkey, name, span, tables, imports, memo, in_progress) {
            Ok(c) => c == Classification::Resource,
            Err(e) => {
                error = Some(e);
                false
            }
        }
    });
    if let Some(e) = error {
        return Err(e);
    }
    Ok(if is_resource {
        Classification::Resource
    } else {
        Classification::Data
    })
}

// --- the check dump (decision 8) ------------------------------------------

/// `effects` is the access pass's (plans/M2.md item D) inferred receiver
/// effect for every private plain-`self` method, keyed `(struct name,
/// method name)` — threaded in as one extra parameter rather than
/// restructuring this module around it (decision 10's minimal-footprint
/// rule): `mod.rs`'s `dump` computes it via `access::infer_effects` and
/// hands it straight through.
pub fn render_items(
    items: &[DeclItem],
    effects: &BTreeMap<(String, String), AccessMode>,
    out: &mut String,
) {
    for item in items {
        render_item(item, 1, effects, out);
    }
}

fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

fn classification_str(c: Classification) -> &'static str {
    match c {
        Classification::Data => "data",
        Classification::Resource => "resource",
    }
}

fn render_deriving(deriving: &[String]) -> String {
    if deriving.is_empty() {
        String::new()
    } else {
        format!(" deriving({})", deriving.join(", "))
    }
}

fn render_generics(generics: &[DeclGenericParam]) -> String {
    if generics.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = generics
        .iter()
        .map(|g| match &g.kind {
            DeclGenericKind::Type => g.name.clone(),
            DeclGenericKind::Const(ty) => format!("const {}: {}", g.name, render_type(ty)),
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

fn mode_str(mode: AccessMode) -> &'static str {
    match mode {
        AccessMode::Read => "read ",
        AccessMode::Mut => "mut ",
        AccessMode::Take => "take ",
    }
}

/// `override_mode` is the access pass's inferred effect for a private
/// plain-`self` receiver (`None` when the mode is already unambiguous:
/// `mut`/`take` are always explicit in source, and `init`/`pub` always
/// print `read self` outright per the doc comment on `DeclReceiver`).
fn render_receiver(r: &DeclReceiver, override_mode: Option<AccessMode>) -> String {
    match r.mode {
        AccessMode::Mut => "mut self".to_string(),
        AccessMode::Take => "take self".to_string(),
        AccessMode::Read if r.is_init || r.is_pub => "read self".to_string(),
        // A private plain-`self` method prints bare `self` only when it
        // was never a candidate for inference at all (a generic method,
        // item H's job) — once the access pass actually computed an
        // effect for it, even `read`, that computed effect is what
        // prints (plans/M2.md item D, decision 8): "inferred private
        // receiver effects shown", not just the escalated ones.
        AccessMode::Read => match override_mode {
            Some(m) => format!("{} self", m.as_str()),
            None => "self".to_string(),
        },
    }
}

fn render_fn_signature(f: &DeclFn, receiver_override: Option<AccessMode>) -> String {
    let mut parts = Vec::with_capacity(f.params.len() + 1);
    if let Some(r) = &f.receiver {
        parts.push(render_receiver(r, receiver_override));
    }
    for p in &f.params {
        parts.push(format!(
            "{}: {}{}",
            p.name,
            mode_str(p.mode),
            render_type(&p.ty)
        ));
    }
    format!(
        "{}{}({}) -> {}",
        f.name,
        render_generics(&f.generics),
        parts.join(", "),
        render_type(&f.ret)
    )
}

pub fn render_type(ty: &Type) -> String {
    match ty {
        Type::Bool => "bool".to_string(),
        Type::U8 => "u8".to_string(),
        Type::U16 => "u16".to_string(),
        Type::U32 => "u32".to_string(),
        Type::U64 => "u64".to_string(),
        Type::Usize => "usize".to_string(),
        Type::I8 => "i8".to_string(),
        Type::I16 => "i16".to_string(),
        Type::I32 => "i32".to_string(),
        Type::I64 => "i64".to_string(),
        Type::Isize => "isize".to_string(),
        Type::F32 => "f32".to_string(),
        Type::F64 => "f64".to_string(),
        Type::Char => "char".to_string(),
        Type::Unit => "unit".to_string(),
        Type::Never => "never".to_string(),
        Type::Str => "Str".to_string(),
        Type::Array(elem, len) => {
            format!("[{}; {}]", render_type(elem), printer::print_expr_bare(len))
        }
        Type::Tuple(elems) if elems.len() == 1 => format!("({},)", render_type(&elems[0])),
        Type::Tuple(elems) => format!(
            "({})",
            elems.iter().map(render_type).collect::<Vec<_>>().join(", ")
        ),
        Type::Option(t) => format!("Option[{}]", render_type(t)),
        Type::Result(ok, err) => format!("Result[{}, {}]", render_type(ok), render_type(err)),
        // plans/M13.md item K: multi-member inferred error set.
        Type::Named(name, args) if name == ERROR_SET_NAME => args
            .iter()
            .map(render_type_arg)
            .collect::<Vec<_>>()
            .join(" | "),
        Type::Named(name, _) if name == INFERRED_ERROR_SET_NAME => "<inferred>".to_string(),
        Type::Own(pool, inner) => format!("own[{pool}] {}", render_type(inner)),
        Type::Static(t) => format!("Static[{}]", render_type(t)),
        Type::Bytes(None) => "Bytes".to_string(),
        Type::Bytes(Some(n)) => format!("Bytes[{}]", printer::print_expr_bare(n)),
        Type::String(n) => format!("String[..{}]", printer::print_expr_bare(n)),
        Type::Fn(params, ret) => {
            let ps = params
                .iter()
                .map(|(m, t)| format!("{}{}", mode_str(*m), render_type(t)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("fn({ps}) -> {}", render_type(ret))
        }
        Type::Generic(name) => name.clone(),
        Type::Named(name, args) if args.is_empty() => name.clone(),
        Type::Named(name, args) => format!(
            "{name}[{}]",
            args.iter()
                .map(render_type_arg)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `pub(crate)` (item H, generics.rs): the canonical-argument renderer
/// generics.rs needs both for the check dump's own `Type::Named`
/// rendering (via `render_type`, unchanged) and for its own instantiation
/// keys/diagnostics — same span-insensitive text, reused rather than
/// duplicated.
pub(crate) fn render_type_arg(arg: &TypeArg) -> String {
    match arg {
        TypeArg::Type(t) => render_type(t),
        TypeArg::Const(e) => printer::print_expr_bare(e),
        TypeArg::Bound(e) => format!("..{}", printer::print_expr_bare(e)),
        TypeArg::Pool(name) => name.clone(),
    }
}

fn render_item(
    item: &DeclItem,
    depth: usize,
    effects: &BTreeMap<(String, String), AccessMode>,
    out: &mut String,
) {
    match item {
        DeclItem::Const(c) => push_line(
            out,
            depth,
            &format!("Const {}: {}", c.name, render_type(&c.ty)),
        ),
        DeclItem::Static(s) => push_line(
            out,
            depth,
            &format!(
                "Static {}: {} placed={:#x}",
                s.name,
                render_type(&s.ty),
                s.addr
            ),
        ),
        DeclItem::Fn(f) => {
            let label = if f.is_async { "AsyncFn" } else { "Fn" };
            push_line(
                out,
                depth,
                &format!("{label} {}", render_fn_signature(f, None)),
            );
        }
        DeclItem::Struct(s) => {
            push_line(
                out,
                depth,
                &format!(
                    "Struct {}{} {}{}",
                    s.name,
                    render_generics(&s.generics),
                    classification_str(s.classification),
                    render_deriving(&s.deriving)
                ),
            );
            for m in &s.members {
                render_member(m, depth + 1, s, effects, out);
            }
        }
        DeclItem::Enum(e) => {
            push_line(
                out,
                depth,
                &format!(
                    "Enum {}{} {}{}",
                    e.name,
                    render_generics(&e.generics),
                    classification_str(e.classification),
                    render_deriving(&e.deriving)
                ),
            );
            for v in &e.variants {
                render_variant(v, depth + 1, out);
            }
            // plans/M9.md item B2: methods/associated fns, same dump
            // surface structs already use (without `@driver` handoff —
            // an enum is never a driver).
            for m in &e.members {
                if let DeclMember::Fn(f) = m {
                    let prefix = if f.is_async { "async fn " } else { "fn " };
                    let override_mode = f.receiver.as_ref().and_then(|r| {
                        if r.mode == AccessMode::Read && !r.is_pub && !r.is_init {
                            effects.get(&(e.name.clone(), f.name.clone())).copied()
                        } else {
                            None
                        }
                    });
                    push_line(
                        out,
                        depth + 1,
                        &format!("{prefix}{}", render_fn_signature(f, override_mode)),
                    );
                }
            }
        }
        DeclItem::Pool(name) => push_line(out, depth, &format!("Pool {name}")),
    }
}

fn render_member(
    m: &DeclMember,
    depth: usize,
    owner: &DeclStruct,
    effects: &BTreeMap<(String, String), AccessMode>,
    out: &mut String,
) {
    match m {
        DeclMember::Field(f) => {
            let prefix = if f.is_pub { "pub field" } else { "field" };
            push_line(
                out,
                depth,
                &format!("{prefix} {}: {}", f.name, render_type(&f.ty)),
            );
        }
        DeclMember::Fn(f) => {
            let prefix = if f.is_async { "async fn " } else { "fn " };
            // Only a private (`!is_pub`) plain-`self` (`Read`, not `init`)
            // receiver is ambiguous enough to need the access pass's
            // inferred effect (types.rs's own `render_receiver` doc
            // comment); every other shape is already unambiguous in
            // source and needs no lookup.
            let override_mode = f.receiver.as_ref().and_then(|r| {
                if r.mode == AccessMode::Read && !r.is_pub && !r.is_init {
                    effects.get(&(owner.name.clone(), f.name.clone())).copied()
                } else {
                    None
                }
            });
            // plans/M7.md item E3: 03-hardware.md §5 — handoff is
            // signature-determined and "displayed by tooling".
            let handoff = crate::sema::handoff::handoff_dump_prefix(owner, f);
            push_line(
                out,
                depth,
                &format!("{handoff}{prefix}{}", render_fn_signature(f, override_mode)),
            );
        }
        DeclMember::Init(f) => push_line(out, depth, &render_fn_signature(f, None)),
        DeclMember::Pool(name) => push_line(out, depth, &format!("pool {name}")),
    }
}

fn render_variant(v: &DeclVariant, depth: usize, out: &mut String) {
    let line = match &v.payload {
        DeclVariantPayload::None => format!("variant {}", v.name),
        DeclVariantPayload::Tuple(types) => format!(
            "variant {}({})",
            v.name,
            types.iter().map(render_type).collect::<Vec<_>>().join(", ")
        ),
        DeclVariantPayload::Named(fields) => format!(
            "variant {}({})",
            v.name,
            fields
                .iter()
                .map(|(n, t)| format!("{n}: {}", render_type(t)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    push_line(out, depth, &line);
}

// --- plans/M9.md items DD / GG: re-key a spliced declaration ------------
//
// Decision 9: the local spelling is the one name. The ModuleCtx splice
// installs under that key; this walk makes every `Type::Named` inside the
// cloned declaration agree. DD re-keyed only the owning type; GG applies
// one simultaneous substitution of *every* exporter spelling the
// importer bound (parameter, return, field, generic argument, const
// type) — keeping the exporter's spelling only where the importer has
// no binding. Paired with `typed::rekey_struct_names` at the TypedProgram
// splice. Rejected (DD 86 / FF 100 / GG): a fallback that tries local
// then exporter — two sources of truth about the canonical spelling.

/// Re-key a spliced `DeclStruct` under the importer's whole-signature
/// substitution. No-op when `subs` is empty.
pub(crate) fn rekey_decl_struct_names(s: &mut DeclStruct, subs: &BTreeMap<String, String>) {
    if subs.is_empty() {
        return;
    }
    if let Some(to) = subs.get(&s.name) {
        s.name = to.clone();
    }
    for m in &mut s.members {
        match m {
            DeclMember::Field(f) => rekey_type_names(&mut f.ty, subs),
            DeclMember::Fn(f) | DeclMember::Init(f) => rekey_decl_fn_names(f, subs),
            DeclMember::Pool(_) => {}
        }
    }
    for (ty, _) in &mut s.component_types {
        rekey_type_names(ty, subs);
    }
    for g in &mut s.generics {
        if let DeclGenericKind::Const(ty) = &mut g.kind {
            rekey_type_names(ty, subs);
        }
    }
}

/// plans/M9.md item B2 / GG: same whole-signature re-key for an imported
/// enum's method signatures and variant payloads.
pub(crate) fn rekey_decl_enum_names(e: &mut DeclEnum, subs: &BTreeMap<String, String>) {
    if subs.is_empty() {
        return;
    }
    if let Some(to) = subs.get(&e.name) {
        e.name = to.clone();
    }
    for m in &mut e.members {
        match m {
            DeclMember::Fn(f) => rekey_decl_fn_names(f, subs),
            DeclMember::Field(_) | DeclMember::Init(_) | DeclMember::Pool(_) => {}
        }
    }
    for v in &mut e.variants {
        match &mut v.payload {
            DeclVariantPayload::None => {}
            DeclVariantPayload::Tuple(types) => {
                for t in types {
                    rekey_type_names(t, subs);
                }
            }
            DeclVariantPayload::Named(fields) => {
                for (_, t) in fields {
                    rekey_type_names(t, subs);
                }
            }
        }
    }
    for (ty, _) in &mut e.component_types {
        rekey_type_names(ty, subs);
    }
    for g in &mut e.generics {
        if let DeclGenericKind::Const(ty) = &mut g.kind {
            rekey_type_names(ty, subs);
        }
    }
}

/// Re-key a free / associated `DeclFn`'s signature under `subs`.
pub(crate) fn rekey_decl_fn_names(f: &mut DeclFn, subs: &BTreeMap<String, String>) {
    if subs.is_empty() {
        return;
    }
    for p in &mut f.params {
        rekey_type_names(&mut p.ty, subs);
    }
    rekey_type_names(&mut f.ret, subs);
    for g in &mut f.generics {
        if let DeclGenericKind::Const(ty) = &mut g.kind {
            rekey_type_names(ty, subs);
        }
    }
}

/// Collect every `Type::Named` spelling reachable from `ty`, including
/// nested generic type arguments (plans/M9.md item HH).
pub(crate) fn collect_named_type_names(ty: &Type, out: &mut BTreeSet<String>) {
    match ty {
        Type::Array(elem, _) => collect_named_type_names(elem, out),
        Type::Tuple(elems) => {
            for e in elems {
                collect_named_type_names(e, out);
            }
        }
        Type::Option(inner) => collect_named_type_names(inner, out),
        Type::Result(ok, err) => {
            collect_named_type_names(ok, out);
            collect_named_type_names(err, out);
        }
        Type::Own(_, inner) | Type::Static(inner) => collect_named_type_names(inner, out),
        Type::Fn(params, ret) => {
            for (_, p) in params {
                collect_named_type_names(p, out);
            }
            collect_named_type_names(ret, out);
        }
        Type::Named(name, targs) => {
            out.insert(name.clone());
            for a in targs {
                if let TypeArg::Type(t) = a {
                    collect_named_type_names(t, out);
                }
            }
        }
        Type::Bytes(_)
        | Type::String(_)
        | Type::Bool
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::Isize
        | Type::F32
        | Type::F64
        | Type::Char
        | Type::Unit
        | Type::Never
        | Type::Str
        | Type::Generic(_) => {}
    }
}

/// Every named type in a resolved fn signature (params, return,
/// const-generic types). The receiver is always `Self` and is not
/// carried as a `Type` on `DeclReceiver`.
pub(crate) fn collect_named_types_from_decl_fn(f: &DeclFn, out: &mut BTreeSet<String>) {
    for p in &f.params {
        collect_named_type_names(&p.ty, out);
    }
    collect_named_type_names(&f.ret, out);
    for g in &f.generics {
        if let DeclGenericKind::Const(ty) = &g.kind {
            collect_named_type_names(ty, out);
        }
    }
}

/// Every named type in a resolved struct declaration (fields + method
/// signatures). Method *bodies* are a separate AST walk at the splice.
pub(crate) fn collect_named_types_from_decl_struct(s: &DeclStruct, out: &mut BTreeSet<String>) {
    for m in &s.members {
        match m {
            DeclMember::Field(f) => collect_named_type_names(&f.ty, out),
            DeclMember::Fn(f) | DeclMember::Init(f) => collect_named_types_from_decl_fn(f, out),
            DeclMember::Pool(_) => {}
        }
    }
    for (ty, _) in &s.component_types {
        collect_named_type_names(ty, out);
    }
}

/// Every named type in a resolved enum declaration (variant payloads +
/// method signatures).
pub(crate) fn collect_named_types_from_decl_enum(e: &DeclEnum, out: &mut BTreeSet<String>) {
    for v in &e.variants {
        match &v.payload {
            DeclVariantPayload::None => {}
            DeclVariantPayload::Tuple(tys) => {
                for ty in tys {
                    collect_named_type_names(ty, out);
                }
            }
            DeclVariantPayload::Named(fields) => {
                for (_, ty) in fields {
                    collect_named_type_names(ty, out);
                }
            }
        }
    }
    for m in &e.members {
        match m {
            DeclMember::Fn(f) | DeclMember::Init(f) => collect_named_types_from_decl_fn(f, out),
            DeclMember::Field(_) | DeclMember::Pool(_) => {}
        }
    }
    for (ty, _) in &e.component_types {
        collect_named_type_names(ty, out);
    }
}

/// Re-key every `Type::Named` whose spelling is a key of `subs`, in one
/// simultaneous pass. Shared by the DeclStruct splice (DD/GG) and
/// `layout::merge_layout_ctx`'s aliased-import install (FF/GG).
pub(crate) fn rekey_type_names(ty: &mut Type, subs: &BTreeMap<String, String>) {
    if subs.is_empty() {
        return;
    }
    rekey_decl_type(ty, subs);
}

fn rekey_decl_type(ty: &mut Type, subs: &BTreeMap<String, String>) {
    match ty {
        Type::Array(elem, _) => rekey_decl_type(elem, subs),
        Type::Tuple(elems) => {
            for e in elems {
                rekey_decl_type(e, subs);
            }
        }
        Type::Option(inner) => rekey_decl_type(inner, subs),
        Type::Result(ok, err) => {
            rekey_decl_type(ok, subs);
            rekey_decl_type(err, subs);
        }
        Type::Own(_, inner) | Type::Static(inner) => rekey_decl_type(inner, subs),
        Type::Fn(params, ret) => {
            for (_, p) in params {
                rekey_decl_type(p, subs);
            }
            rekey_decl_type(ret, subs);
        }
        Type::Named(name, targs) => {
            if let Some(to) = subs.get(name) {
                *name = to.clone();
            }
            for a in targs {
                match a {
                    TypeArg::Type(t) => rekey_decl_type(t, subs),
                    TypeArg::Const(_) | TypeArg::Bound(_) | TypeArg::Pool(_) => {}
                }
            }
        }
        Type::Bytes(_)
        | Type::String(_)
        | Type::Bool
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::Isize
        | Type::F32
        | Type::F64
        | Type::Char
        | Type::Unit
        | Type::Never
        | Type::Str
        | Type::Generic(_) => {}
    }
}

// --- tests --------------------------------------------------------------
//
// 02-language.md §7.1: "`struct` is a product value — data if every field
// is data; `resource struct` makes it a resource by fiat"; §6.2: "own[P]
// T — pool handle" and "Static[T] ... a copyable read-only handle ... it
// exposes no address and no mutation" (always data, regardless of the
// payload). `resource_propagates`'s own doc comment above states the rule
// this pins: a resource propagates through `own[..]`, arrays, tuples,
// `Option`, `Result`; every scalar/`Fn`/`Generic`/`Static` variant is
// always data; a `Named` type's own resource-ness is answered by the
// caller-supplied closure (memoized recursive classification in
// `classify_type`, a plain lookup in `bodies::is_resource_type`) — neither
// goldens nor the fuzzer check this per-variant table directly.
#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_len() -> Box<Expr> {
        Box::new(Expr::Int(Span::default(), "0".to_string()))
    }

    #[test]
    fn resource_propagates_table() {
        let mut always_resource = |_: &str, _: &[TypeArg]| true;
        let mut never_resource = |_: &str, _: &[TypeArg]| false;

        // Scalars, unit/never, Str, Bytes, Fn, Generic: always data, even
        // when the named-type answer would say otherwise (it is never
        // consulted for these variants).
        let always_data_cases: Vec<(&str, Type)> = vec![
            ("bool", Type::Bool),
            ("u8", Type::U8),
            ("u16", Type::U16),
            ("u32", Type::U32),
            ("u64", Type::U64),
            ("usize", Type::Usize),
            ("i8", Type::I8),
            ("i16", Type::I16),
            ("i32", Type::I32),
            ("i64", Type::I64),
            ("isize", Type::Isize),
            ("f32", Type::F32),
            ("f64", Type::F64),
            ("char", Type::Char),
            ("unit", Type::Unit),
            ("never", Type::Never),
            ("Str", Type::Str),
            ("Bytes[N]", Type::Bytes(Some(dummy_len()))),
            ("bare Bytes", Type::Bytes(None)),
            ("String[..N]", Type::String(dummy_len())),
            (
                "fn(read u8) -> u8",
                Type::Fn(vec![(AccessMode::Read, Type::U8)], Box::new(Type::U8)),
            ),
            ("a generic type parameter", Type::Generic("T".to_string())),
        ];
        for (msg, ty) in &always_data_cases {
            assert!(
                !resource_propagates(ty, &mut always_resource),
                "`{msg}` should be data even though the named-type answer is `resource`"
            );
        }

        // own[P] T is always a resource, regardless of T (§6.2; the
        // payload is never even visited).
        assert!(
            resource_propagates(
                &Type::Own("P".to_string(), Box::new(Type::U8)),
                &mut never_resource
            ),
            "own[P] T is always a resource regardless of T"
        );

        // Static[T] is always data, regardless of T (§6.2's "regardless
        // of T" applies even when T is itself a resource-shaped type).
        assert!(
            !resource_propagates(
                &Type::Static(Box::new(Type::Own("P".to_string(), Box::new(Type::U8)))),
                &mut always_resource
            ),
            "Static[T] is always data regardless of T"
        );

        // Array/Tuple/Option/Result propagate: resource exactly when some
        // component is (§7.1's rule, extended to every composite `Type`
        // this pass resolves).
        let resource_elem = Type::Own("P".to_string(), Box::new(Type::U8));
        let data_elem = Type::U8;

        assert!(
            resource_propagates(
                &Type::Array(Box::new(resource_elem.clone()), dummy_len()),
                &mut never_resource
            ),
            "[own[P] u8; N] is a resource"
        );
        assert!(
            !resource_propagates(
                &Type::Array(Box::new(data_elem.clone()), dummy_len()),
                &mut never_resource
            ),
            "[u8; N] is data"
        );

        assert!(
            resource_propagates(
                &Type::Tuple(vec![data_elem.clone(), resource_elem.clone()]),
                &mut never_resource
            ),
            "a tuple with one resource element is a resource"
        );
        assert!(
            !resource_propagates(
                &Type::Tuple(vec![data_elem.clone(), data_elem.clone()]),
                &mut never_resource
            ),
            "a tuple of only data elements is data"
        );

        assert!(
            resource_propagates(
                &Type::Option(Box::new(resource_elem.clone())),
                &mut never_resource
            ),
            "Option[own[P] u8] is a resource"
        );
        assert!(
            !resource_propagates(
                &Type::Option(Box::new(data_elem.clone())),
                &mut never_resource
            ),
            "Option[u8] is data"
        );

        assert!(
            resource_propagates(
                &Type::Result(Box::new(data_elem.clone()), Box::new(resource_elem.clone())),
                &mut never_resource
            ),
            "Result[u8, own[P] u8] is a resource (Err side)"
        );
        assert!(
            resource_propagates(
                &Type::Result(Box::new(resource_elem.clone()), Box::new(data_elem.clone())),
                &mut never_resource
            ),
            "Result[own[P] u8, u8] is a resource (Ok side)"
        );
        assert!(
            !resource_propagates(
                &Type::Result(Box::new(data_elem.clone()), Box::new(data_elem.clone())),
                &mut never_resource
            ),
            "Result[u8, u8] is data"
        );

        // A bare Named type resolves through the closure argument — both
        // answers are exercised (§7.1: "resource struct makes it a
        // resource by fiat", decided by the caller, not by this function).
        let named = Type::Named("Foo".to_string(), vec![]);
        assert!(
            resource_propagates(&named, &mut always_resource),
            "Named delegates to the closure: a `resource` answer propagates"
        );
        assert!(
            !resource_propagates(&named, &mut never_resource),
            "Named delegates to the closure: a `data` answer propagates"
        );
    }

    // --- `@layout` (plans/M7.md item B) ------------------------------------
    //
    // Every rule with a *source-shaped* rejection is pinned as a golden
    // (`tests/golden/err-layout-*`) — that is the review surface. These
    // cover the declaration-shape guards whose own golden would say
    // nothing a reader could not predict, plus the two properties a golden
    // structurally cannot show: that `check_layouts` is a pure function
    // (`image.report.deterministic`'s own precondition), and that the dump
    // grammar is exactly what this module claims it is.

    fn layouts_of(src: &str) -> Result<Vec<LayoutType>, SemaError> {
        let tokens = crate::syntax::lexer::lex(src).expect("test source lexes");
        let module = crate::syntax::parser::parse(tokens).expect("test source parses");
        check_layouts(&module)
    }

    /// The whole pipeline, so the *later* layout-completion pass runs
    /// (plans/M10.md item A2b): `check_layouts` alone would only ever defer.
    fn completed_layouts_of(src: &str) -> Result<Vec<LayoutType>, SemaError> {
        let tokens = crate::syntax::lexer::lex(src).expect("test source lexes");
        let module = crate::syntax::parser::parse(tokens).expect("test source parses");
        crate::sema::check_typed(&module, "t.wr").map(|p| p.layouts)
    }

    /// The early pass leaves a `const`-named length **deferred** — no size, no
    /// entries — rather than rejecting it (as item A2 did) or guessing at it.
    /// This is the property decision 581 turns on, and no golden can show it:
    /// by the time any artifact is printed, the layout has been completed.
    #[test]
    fn a_const_length_defers_in_the_early_pass_and_completes_in_the_later_one() {
        let src = "module t\n\nconst N: u32 = 4\n\n\
                   @layout(runtime, endian=little)\nstruct T:\n\
                   \x20   rr_cursor: u64\n    turns: [u32; N]\n";
        let early = layouts_of(src).expect("the early pass defers, it does not reject");
        assert_eq!(early.len(), 1);
        assert_eq!(early[0].size, None, "deferred, not zero");
        assert!(early[0].entries.is_empty(), "no offsets are known yet");
        assert_eq!(early[0].padding, 0);
        // And the same declaration, completed: 8 + 4 * 4.
        let done = completed_layouts_of(src).expect("the later pass completes it");
        assert_eq!(done[0].size, Some(24));
        assert_eq!(done[0].entries.len(), 2);
    }

    /// A length that depends on another `const` works, because resolution goes
    /// through the one real evaluator rather than a second scanner that would
    /// have to reimplement arithmetic (decision 580's rejected alternative
    /// (ii)).
    #[test]
    fn a_const_length_may_depend_on_another_const() {
        let done = completed_layouts_of(
            "module t\n\nconst BASE: u32 = 2\nconst N: u32 = BASE * 3\n\n\
             @layout(runtime, endian=little)\nstruct T:\n    turns: [u32; N]\n",
        )
        .expect("arithmetic in the `const`'s own initializer is the evaluator's job");
        assert_eq!(done[0].size, Some(24));
    }

    /// The completion pass's own rejections, by the substring that makes each
    /// message the right one. The two with a source-shaped story of their own
    /// are goldens (`err-layout-runtime-len-not-const`,
    /// `err-layout-runtime-len-zero`, `err-layout-runtime-len-too-big`); these
    /// are the neighbours that would say nothing extra as a golden.
    #[test]
    fn const_length_guards() {
        let cases: &[(&str, &str)] = &[
            // Negative: the same rule zero gets — a `@layout` field covers at
            // least one byte — and worth its own case because a signed `const`
            // reaches the pass as a perfectly valid `i32`.
            (
                "module t\n\nconst N: i32 = -3\n\n@layout(runtime, endian=little)\n\
                 struct T:\n    turns: [u32; N]\n",
                "whose value is -3",
            ),
            // Not an integer at all: a length is a count of elements.
            (
                "module t\n\nconst N: bool = true\n\n@layout(runtime, endian=little)\n\
                 struct T:\n    turns: [u32; N]\n",
                "whose value is not an integer",
            ),
            // A `const` the unselected `comptime if` branch declared does not
            // exist after `specialize`, so the *real* name resolver refuses it
            // first — which is exactly why this pass reads the real const
            // table instead of scanning `module.items` for `const` itself.
            (
                "module t\n\nconst DEBUG: bool = false\n\ncomptime if DEBUG:\n\
                 \x20   const N: u32 = 3\ncomptime else:\n    const M: u32 = 9\n\n\
                 @layout(runtime, endian=little)\nstruct T:\n    turns: [u32; N]\n",
                "unknown name `N`",
            ),
        ];
        for (src, needle) in cases {
            let err = completed_layouts_of(src).expect_err("must be rejected");
            assert!(
                err.message.contains(needle),
                "expected {needle:?} in {:?}",
                err.message
            );
        }
    }

    /// Requirement 4 of plans/M10.md item A2b: an uncompleted layout that
    /// reaches an artifact is a fail-closed rejection, never a `size=0` line.
    /// Unreachable from source — every pipeline entry completes the table
    /// first — so it is asserted directly on the renderers, which is the only
    /// place the guard can be observed.
    #[test]
    fn an_uncompleted_layout_never_reports_a_size() {
        let deferred = LayoutType {
            name: "TurnTable".to_string(),
            kind: LayoutKind::Runtime,
            endian: LayoutEndian::Little,
            size: None,
            padding: 0,
            entries: Vec::new(),
        };
        let mut out = String::new();
        let err = push_layout_lines(&mut out, 0, &deferred).expect_err("must refuse");
        assert_eq!(err.category, "type");
        assert!(err.message.contains("has no computed size"), "{err:?}");
        assert!(
            err.omit_location,
            "a pass-order fact has no source position"
        );
        assert!(
            out.is_empty(),
            "nothing is printed for a layout with no size"
        );
        assert!(dump_layouts(&[("t".to_string(), vec![deferred.clone()])]).is_err());
        // And the same guard on the byte count itself, which is what
        // `img.dma_pool`'s backing and every other consumer asks through.
        assert!(deferred.require_size("a test").is_err());
    }

    const MMIO_EXAMPLE: &str = "module t\n\n\
         @layout(mmio, endian=little)\n\
         struct VirtioIrqMmio:\n\
         \x20   @offset(0x060) interrupt_status: ReadOnly[u32]\n\
         \x20   @offset(0x064) interrupt_ack: WriteOnly[u32]\n";

    #[test]
    fn the_hardware_chapter_example_lays_out_exactly() {
        let layouts = layouts_of(MMIO_EXAMPLE).expect("03-hardware.md §2's own example");
        assert_eq!(layouts.len(), 1);
        let l = &layouts[0];
        assert_eq!(l.kind, LayoutKind::Mmio);
        assert_eq!(l.endian, LayoutEndian::Little);
        // 0x64 + 4: the layout is exactly the bytes its fields cover, with
        // no trailing padding and no alignment round-up.
        assert_eq!(l.size, Some(0x68));
        // The 0x60 bytes below the first field are a declared hole, not an
        // invented one: the author wrote `@offset(0x060)`.
        assert_eq!(l.padding, 0x60);
        assert_eq!(
            l.entries,
            vec![
                LayoutEntry::Padding {
                    offset: 0,
                    size: 0x60
                },
                LayoutEntry::Field(LayoutField {
                    name: "interrupt_status".to_string(),
                    ty: "ReadOnly[u32]".to_string(),
                    offset: 0x60,
                    size: 4,
                }),
                LayoutEntry::Field(LayoutField {
                    name: "interrupt_ack".to_string(),
                    ty: "WriteOnly[u32]".to_string(),
                    offset: 0x64,
                    size: 4,
                }),
            ]
        );
    }

    #[test]
    fn the_dump_grammar_is_fixed() {
        let layouts = layouts_of(MMIO_EXAMPLE).unwrap();
        let text = dump_layouts(&[("t".to_string(), layouts)]).expect("every layout is complete");
        assert_eq!(
            text,
            "LayoutTypes v0\n\
             \x20 Module path=t\n\
             \x20   Layout name=VirtioIrqMmio kind=mmio endian=little size=104 padding=96\n\
             \x20     Padding offset=0x0 size=96\n\
             \x20     Field name=interrupt_status type=ReadOnly[u32] offset=0x60 size=4\n\
             \x20     Field name=interrupt_ack type=WriteOnly[u32] offset=0x64 size=4\n"
        );
    }

    #[test]
    fn check_layouts_is_a_pure_function_of_its_module() {
        let a = layouts_of(MMIO_EXAMPLE).unwrap();
        let b = layouts_of(MMIO_EXAMPLE).unwrap();
        assert_eq!(a, b);
        // A module with no `@layout` type contributes nothing at all
        // (facts only — never an empty placeholder block).
        let none = layouts_of("module t\n\nstruct S:\n    n: u32\n").unwrap();
        assert!(none.is_empty());
        assert_eq!(
            dump_layouts(&[("t".to_string(), none)]).expect("nothing to dump"),
            "LayoutTypes v0\n"
        );
    }

    /// Every declaration-shape guard, by the substring that makes each
    /// message the right one. Kept as a table because these are all one
    /// sentence long and none of them needs a source file to be
    /// understood.
    #[test]
    fn declaration_shape_guards() {
        let cases: &[(&str, &str)] = &[
            (
                "module t\n\n@layout(packed, endian=little)\nstruct S:\n    n: u32\n",
                "unknown `@layout` kind `packed`",
            ),
            (
                "module t\n\n@layout(endian=little)\nstruct S:\n    n: u32\n",
                "names no kind",
            ),
            (
                "module t\n\n@layout(dma, endian=little)\n@layout(wire, endian=big)\n\
                 struct S:\n    n: u32\n",
                "more than one `@layout` attribute",
            ),
            (
                "module t\n\n@layout(dma, endian=little)\nstruct S[const N: usize]:\n    n: u32\n",
                "is generic",
            ),
            (
                "module t\n\n@layout(dma, endian=little)\nstruct S:\n    pool P\n",
                "declares a pool",
            ),
            (
                "module t\n\n@layout(dma, endian=little)\nstruct S:\n\
                 \x20   n: u32\n\n    init(mut self):\n        self.n = 0\n",
                "declares an `init`",
            ),
            (
                "module t\n\n@layout(dma, endian=little)\nstruct S:\n    @offset(N) n: u32\n",
                "takes exactly one integer literal",
            ),
            (
                "module t\n\n@layout(dma, endian=little)\nstruct S:\n\
                 \x20   @offset(0) @offset(4) n: u32\n",
                "more than one `@offset`",
            ),
            (
                "module t\n\n@layout(dma, endian=little)\nstruct S:\n    @packed n: u32\n",
                "unknown attribute `@packed`",
            ),
            // The three field arms whose *sibling* arm is the one a golden
            // pins, kept honest here rather than left as an untested
            // branch of a tested rule: `f32`/`f64` are target-dependent on
            // 02-language.md §6.1's own "where the target enables them"
            // (`golden/err-layout-target-dependent` pins `usize`); a
            // capability in a `dma`/`mmio` layout is rejected for the more
            // basic reason than 03 §3's `wire` sentence
            // (`golden/err-layout-wire-capability` pins that one); and a
            // register wrapper is only ever a wrapper *of a sized integer*
            // (`golden/err-layout-mmio-wrapper` pins the wrong-kind arm).
            (
                "module t\n\n@layout(wire, endian=big)\nstruct S:\n    ratio: f32\n",
                "where the target enables them",
            ),
            (
                "module t\n\n@layout(mmio, endian=little)\nstruct S:\n    cap: DeviceCap[Blk]\n",
                "it has no byte encoding",
            ),
            (
                "module t\n\n@layout(mmio, endian=little)\nstruct S:\n    r: ReadOnly[bool]\n",
                "wraps `bool`, which is not a sized integer register",
            ),
            (
                "module t\n\n@layout(mmio, endian=little)\nstruct S:\n    r: WriteOnly[u32, u32]\n",
                "must wrap exactly one register type",
            ),
            // plans/M10.md item A2, 03-hardware.md §3.1. The `runtime`
            // kind's two new field shapes are pinned end to end by
            // `golden/check-layout-runtime` and their headline rejections by
            // five `golden/err-layout-*` cases; these are the *sibling*
            // arms, each one sentence long and none needing a source file
            // to be understood (the precedent this table already set
            // above).
            //
            // §3.1's element set is closed — "another `@layout(runtime)`
            // type, or a fixed-length array of one" — so everything that is
            // neither a sized integer nor a nested `runtime` layout is one
            // rejection. `[usize; 4]` among them: decision 563 adds **no**
            // `usize` exemption for this kind, because one target-dependent
            // layout class would break the property `@layout` exists for.
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct S:\n    a: [usize; 4]\n",
                "which is not an array field's element type",
            ),
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct S:\n    a: [bool; 4]\n",
                "which is not an array field's element type",
            ),
            // An array *of arrays* is not "a fixed-length array of one"
            // nested layout; it is two levels of a shape §3.1 grants one
            // level of.
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct S:\n    a: [[u32; 2]; 2]\n",
                "which is not an array field's element type",
            ),
            // A negative length is neither an integer literal (it parses as a
            // unary expression) nor a bare `const` name, so plans/M10.md item
            // A2b's widened rule still refuses it — and refuses it *early*,
            // in the pass that evaluates nothing, rather than deferring an
            // expression it would have to type-check to read.
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct S:\n    a: [u32; -1]\n",
                "neither an integer literal nor the name of a module-level `const`",
            ),
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct S:\n    a: [u32; 0]\n",
                "has length 0",
            ),
            // An element whose width is not a multiple of its own alignment
            // (`E` is 5 bytes, 4-byte aligned) puts every element after the
            // first at a misaligned offset, and the only fix is padding
            // between elements — which a `@layout` never invents.
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct E:\n\
                 \x20   a: u32\n    b: u8\n\n\
                 @layout(runtime, endian=little)\nstruct S:\n    e: [E; 2]\n",
                "would need implicit padding to be aligned",
            ),
            // The nesting rule's other direction: `golden/err-layout-\
            // runtime-nests-dma` pins a `runtime` layout reaching for a
            // `dma` one; this is a `dma` layout reaching for a `runtime`
            // one, which must *not* be confused with the M7 item E gap
            // (`golden/err-layout-nested`, a `dma` layout nesting a `dma`
            // one — a missing feature, not a rule).
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct R:\n\
                 \x20   a: u32\n\n\
                 @layout(dma, endian=little)\nstruct S:\n    r: R\n",
                "nests a `@layout` type of a different kind",
            ),
            // A nested layout's alignment is the widest among its fields
            // (4 here), not its size, and it is what the following field's
            // offset is checked against: `t` at offset 1 needs 3 invented
            // bytes.
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct R:\n\
                 \x20   a: u32\n    b: u32\n\n\
                 @layout(runtime, endian=little)\nstruct S:\n    tag: u8\n    r: R\n",
                "would need 3 byte(s) of implicit padding to be 4-byte aligned",
            ),
        ];
        for (src, needle) in cases {
            let err = layouts_of(src).expect_err("must be rejected");
            assert_eq!(err.category, "type");
            assert!(
                err.message.contains(needle),
                "expected {needle:?} in {:?}",
                err.message
            );
        }
    }

    /// A long chain of nested `@layout(runtime)` types — `L0` nests `L1`
    /// nests ... nests `L199` — fails closed with a named diagnostic rather
    /// than recursing 200 frames into this pass.
    ///
    /// Cycle detection already makes the recursion finite (no name repeats),
    /// so "it terminates" was never the question; the question is whether it
    /// terminates by *deciding* or by exhausting the process stack. 200 is
    /// far past the two levels 03 §3.1's tables need and well past
    /// `MAX_LAYOUT_NEST_DEPTH`, and a golden for it would be six hundred
    /// generated lines saying nothing a reader could not predict, so the
    /// input is generated here instead.
    #[test]
    fn a_deep_nesting_chain_fails_closed() {
        let mut src = String::from("module t\n\n");
        for i in 0..200 {
            src.push_str("@layout(runtime, endian=little)\n");
            src.push_str(&format!("struct L{i}:\n"));
            if i == 199 {
                src.push_str("    leaf: u32\n\n");
            } else {
                src.push_str(&format!("    next: L{}\n\n", i + 1));
            }
        }
        let err = layouts_of(&src).expect_err("a 200-deep chain must be refused");
        assert_eq!(err.category, "type");
        assert!(
            err.message
                .contains(&format!("more than {MAX_LAYOUT_NEST_DEPTH} deep")),
            "expected the depth rejection, got {:?}",
            err.message
        );
    }

    /// A nesting graph that is *within* the depth cap but exponentially wide
    /// — sixteen layouts, each naming the next one four times — fails closed
    /// on the expansion budget.
    ///
    /// This is the failure the depth cap does not catch, and it is the more
    /// dangerous one: `4^15` expansions from twenty lines of source is not a
    /// wrong answer, it is a pass that never returns, and `check_layouts` is
    /// on the `sema` fuzz lane's path on every iteration. The test's own
    /// runtime is the assertion — if the budget stopped working this would
    /// hang rather than fail.
    #[test]
    fn a_wide_nesting_graph_fails_closed() {
        let mut src = String::from("module t\n\n");
        for i in 0..16 {
            src.push_str("@layout(runtime, endian=little)\n");
            src.push_str(&format!("struct L{i}:\n"));
            if i == 15 {
                src.push_str("    leaf: u32\n\n");
            } else {
                for f in 0..4 {
                    src.push_str(&format!("    f{f}: L{}\n", i + 1));
                }
                src.push('\n');
            }
        }
        let err = layouts_of(&src).expect_err("a 4-wide 16-deep graph must be refused");
        assert_eq!(err.category, "type");
        assert!(
            err.message.contains(&format!(
                "expands more than {MAX_LAYOUT_NEST_EXPANSIONS} nested"
            )),
            "expected the expansion-budget rejection, got {:?}",
            err.message
        );
    }

    /// The two `runtime` field shapes, laid out (03-hardware.md §3.1).
    /// `golden/check-layout-runtime` is the review surface; this asserts the
    /// one fact a dump cannot show, because nothing prints it — that a
    /// nested layout's *alignment* is the widest among its fields and an
    /// array's is its element's, so neither field is over-aligned to its own
    /// size.
    #[test]
    fn runtime_fields_align_to_their_element_not_their_size() {
        let src = "module t\n\n\
             @layout(runtime, endian=little)\n\
             struct TurnArea:\n\
             \x20   state: u32\n    waiter: u32\n\n\
             @layout(runtime, endian=little)\n\
             struct TurnTable:\n\
             \x20   rr_cursor: u64\n    turns: [TurnArea; 4]\n    one: TurnArea\n";
        let layouts = layouts_of(src).expect("03-hardware.md §3.1's own shape");
        let table = &layouts[1];
        // 8 + 4*8 + 8, with no padding anywhere: a 32-byte array field
        // aligned to 32 would have needed 24 invented bytes at offset 0x8,
        // and a 40-byte struct field aligned to 40 would have needed more
        // still.
        assert_eq!(table.size, Some(48));
        assert_eq!(table.padding, 0);
        assert_eq!(
            table.entries,
            vec![
                LayoutEntry::Field(LayoutField {
                    name: "rr_cursor".to_string(),
                    ty: "u64".to_string(),
                    offset: 0,
                    size: 8,
                }),
                LayoutEntry::Field(LayoutField {
                    name: "turns".to_string(),
                    ty: "[TurnArea; 4]".to_string(),
                    offset: 8,
                    size: 32,
                }),
                LayoutEntry::Field(LayoutField {
                    name: "one".to_string(),
                    ty: "TurnArea".to_string(),
                    offset: 40,
                    size: 8,
                }),
            ]
        );
    }

    /// A `@layout` struct with no fields at all. Its own case because the
    /// parser has no empty-body form — the guard is reachable only through
    /// a body that declares something else this pass already skipped, which
    /// today means nothing does: it is the one rule below that is a
    /// structural floor rather than a source-reachable rejection, and it
    /// stays because "size zero" must never be a reportable answer.
    #[test]
    fn an_empty_layout_has_no_reportable_size() {
        let src = "module t\n\n@layout(dma, endian=little)\nstruct S:\n    pass\n";
        // The parser rejects `pass` as a struct member outright, so this
        // asserts only that nothing accepts it silently.
        assert!(
            crate::syntax::lexer::lex(src)
                .ok()
                .and_then(|t| crate::syntax::parser::parse(t).ok())
                .map(|m| check_layouts(&m).is_err())
                .unwrap_or(true)
        );
    }

    // --- capabilities (plans/M7.md item A, 03-hardware.md §1) ------------

    fn check_err(src: &str) -> SemaError {
        let tokens = crate::syntax::lexer::lex(src).expect("test source lexes");
        let module = crate::syntax::parser::parse(tokens).expect("test source parses");
        crate::sema::check(&module, "test.wr").expect_err("test source must be rejected")
    }

    fn check_ok(src: &str) {
        let tokens = crate::syntax::lexer::lex(src).expect("test source lexes");
        let module = crate::syntax::parser::parse(tokens).expect("test source parses");
        if let Err(e) = crate::sema::check(&module, "test.wr") {
            panic!(
                "expected acceptance, got error[{}]: {}",
                e.category, e.message
            );
        }
    }

    /// One prefix every capability case below shares: an `@layout(mmio)`
    /// type for `Mmio[L]` to name and a plain struct for `DeviceCap[D]`.
    const CAP_PRELUDE: &str = "module t\n\n\
         @layout(mmio, endian=little)\n\
         struct Regs:\n\
         \x20   @offset(0x000) status: ReadOnly[u32]\n\n\
         @layout(dma, endian=little)\n\
         struct Ctl:\n\
         \x20   idx: u16\n\n\
         struct Blk:\n\
         \x20   id: u32\n\n\
         pool Slots\n\n";

    /// Every capability *shape* guard with no source-shaped story of its
    /// own — arity, and the argument-kind rules — kept here rather than
    /// each getting a golden that would say nothing a reader could not
    /// predict (`declaration_shape_guards` above set the precedent).
    #[test]
    fn capability_shape_guards() {
        let cases: &[(&str, &str)] = &[
            // Arity comes from the one shared list
            // (`eval::image_checks::CAPABILITY_TYPES`): each name's count
            // is fixed, and a bare or over-applied spelling is a named
            // rejection rather than a silently different type.
            (
                "fn f(read c: DeviceCap) -> u32:\n    return 0\n",
                "`DeviceCap` expects 1 generic argument(s), found 0",
            ),
            (
                "fn f(read c: Mmio[Regs, Regs]) -> u32:\n    return 0\n",
                "`Mmio` expects 1 generic argument(s), found 2",
            ),
            (
                "fn f(read c: DmaPool[Slots]) -> u32:\n    return 0\n",
                "`DmaPool` expects 2 generic argument(s), found 1",
            ),
            // plans/M7.md item D: `DmaPool[P, N]`/`DmaShared[P, L]` name a
            // bound **pool** in argument position 0 (03-hardware.md §1's
            // own `DmaPool[BlockControl, 256.KiB]`), resolved against the
            // declared pool names rather than the type table. A type there
            // is not a pool, and a pool name is not a type.
            (
                "fn f(take c: DmaPool[Blk, 4096]) -> u32:\n    return 0\n",
                "unknown pool `Blk`",
            ),
            (
                "fn f(take c: DmaShared[Blk, Ctl]) -> u32:\n    return 0\n",
                "unknown pool `Blk`",
            ),
            (
                "fn f(take c: DmaShared[Slots, Blk]) -> u32:\n    return 0\n",
                "requires `Blk` to be an `@layout(dma)` struct",
            ),
            (
                "fn f(take c: DmaShared[Slots, 4]) -> u32:\n    return 0\n",
                "requires a layout type argument",
            ),
            (
                "fn f(take c: DmaShared[4, Ctl]) -> u32:\n    return 0\n",
                "names the DMA pool it is authority over",
            ),
            // plans/M7.md item D self-audit: two more reachable arms with
            // no golden of their own — a pool argument carrying generic
            // arguments (a `pool` declaration has none), and an `L` that
            // is a scalar rather than a named struct.
            (
                "fn f(take c: DmaPool[Option[u8], 4]) -> u32:\n    return 0\n",
                "takes no generic arguments of its own",
            ),
            (
                "fn f(take c: DmaShared[Slots, u32]) -> u32:\n    return 0\n",
                "must name an `@layout(dma)` struct",
            ),
            // `Mmio[L]`'s own argument rule (03 §2), in the two shapes no
            // golden covers: a scalar, and a struct with no `@layout` at
            // all. `golden/err-cap-mmio-layout` pins the third — a
            // `@layout` of the wrong *kind*, which is the interesting one.
            (
                "fn f(read c: Mmio[u32]) -> u32:\n    return 0\n",
                "must name an `@layout(mmio)` struct",
            ),
            (
                "fn f(read c: Mmio[Blk]) -> u32:\n    return 0\n",
                "requires `Blk` to be an `@layout(mmio)` struct",
            ),
            // A const argument where a layout type belongs — `Mmio[4]`
            // resolves (a capability's arguments go through the general
            // `resolve_type_arg`, so a const stays a const), and the
            // argument rule is what rejects it.
            (
                "fn f(read c: Mmio[4]) -> u32:\n    return 0\n",
                "`Mmio` requires a type argument",
            ),
            // The argument rule reaches through composites, not only a
            // bare annotation — the recursion arms of
            // `validate_capability_args`, which no golden exercises.
            (
                "fn f(read c: Option[Mmio[Blk]]) -> u32:\n    return 0\n",
                "requires `Blk` to be an `@layout(mmio)` struct",
            ),
            (
                "fn f(read c: [(u32, Mmio[Blk]); 2]) -> u32:\n    return 0\n",
                "requires `Blk` to be an `@layout(mmio)` struct",
            ),
            // ...and into an enum variant's own payload, the one item
            // declaration kind with no fn signature of its own.
            (
                "enum E:\n    Plain\n    Held(Mmio[Blk])\n",
                "requires `Blk` to be an `@layout(mmio)` struct",
            ),
        ];
        for (body, needle) in cases {
            let src = format!("{CAP_PRELUDE}{body}");
            let err = check_err(&src);
            assert_eq!(err.category, "type", "for {body:?}");
            assert!(
                err.message.contains(needle),
                "expected {needle:?} in {:?}",
                err.message
            );
        }
    }

    /// The unforgeability claim, made checkable rather than argued.
    ///
    /// 03-hardware.md §1: "Their constructors are not source-visible: no
    /// address, import, or cast creates one." The claim this table backs
    /// is stronger and more mechanical than that sentence: **the only
    /// declaration positions from which a capability-typed value can
    /// originate are a `@driver`'s own fields and a fn's own parameters**
    /// — because every other position that could introduce one is
    /// rejected by name below, and every *expression* that could produce
    /// one out of nothing is rejected too.
    ///
    /// Why that is the whole list: a typed expression's type comes from
    /// exactly one of (a) a literal — none of which is ever a named type,
    /// (b) a declared annotation reached by reading it (a field, a
    /// parameter, a local, a `const`), (c) a callee's declared return
    /// type, (d) a builtin intrinsic's own fixed result type — none of
    /// which is a capability, `sema::bodies`' intrinsic table, or (e) a
    /// composition of those (tuple/array/field/index/`Option`/`Result`
    /// unwrapping), which introduces no new named type. The cases below
    /// close (b) for `const`s and (c) for every fn; locals are closed by
    /// induction, since a local's type is its initializer's; and the
    /// construction/call/cast/declare/import cases close the routes that
    /// would have manufactured a value with no annotation at all. What
    /// remains — a `@driver` field and a fn parameter — is exactly what
    /// `check_provenance` and the image binding govern.
    #[test]
    fn no_source_construct_produces_a_capability() {
        let cases: &[(&str, &str, &str)] = &[
            // (a) construction, by every spelling the grammar has.
            (
                "a struct-literal construction",
                "fn f() -> u32:\n    c = DeviceCap[Blk](id=1)\n    return 0\n",
                "cannot be constructed",
            ),
            (
                "a bare call",
                "fn f() -> u32:\n    c = DeviceCap(1)\n    return 0\n",
                "cannot be called",
            ),
            (
                "a conversion (this language's only cast)",
                "fn f(a: u64) -> u32:\n    c = a.to[DeviceCap[Blk]]()\n    return 0\n",
                "cannot be cast to",
            ),
            // (b) a declaration under the name, which would make every
            // spelling above legal at once.
            (
                "a module declaration under the name",
                "struct Mmio:\n    base: u64\n",
                "cannot be declared",
            ),
            // (c) a signature claiming to return one, at any nesting.
            (
                "a fn returning one",
                "fn f() -> DeviceCap[Blk]:\n    panic(\"x\")\n",
                "none may claim to return one",
            ),
            (
                "a fn returning one inside a composite",
                "fn f() -> Option[(u32, Mmio[Regs])]:\n    return None\n",
                "none may claim to return one",
            ),
            (
                "a method returning one",
                "struct S:\n    n: u32\n\n    fn f(read self) -> IrqCap[u32]:\n\
                 \x20       panic(\"x\")\n",
                "none may claim to return one",
            ),
            // (b) a comptime value claiming to be one.
            (
                "a const declared as one",
                "const C: DmaPool[Slots, 4096] = 0\n",
                "no comptime value is one",
            ),
            // plans/M7.md item D: the same sentence for 03 §3's shared
            // control memory, whose first argument is a pool name too.
            (
                "a const declared as shared control memory",
                "const C: DmaShared[Slots, Ctl] = 0\n",
                "no comptime value is one",
            ),
        ];
        for (what, body, needle) in cases {
            let src = format!("{CAP_PRELUDE}{body}");
            let err = check_err(&src);
            assert!(
                err.message.contains(needle),
                "{what}: expected {needle:?} in {:?}",
                err.message
            );
        }
    }

    /// The asymmetry 03-hardware.md §1 turns on, both directions, in one
    /// test — a `@driver` may hold what an `@actor` may not. Without the
    /// accepting half, the containment rule could be satisfied by
    /// rejecting capabilities everywhere, which would be a different
    /// (and wrong) rule.
    #[test]
    fn a_driver_may_hold_a_capability_and_an_actor_may_not() {
        check_ok(&format!(
            "{CAP_PRELUDE}@driver\npub struct D:\n    regs: Mmio[Regs]\n"
        ));
        let err = check_err(&format!(
            "{CAP_PRELUDE}@actor\npub struct A:\n    regs: Mmio[Regs]\n\n\
             \x20   init(mut self):\n        pass\n"
        ));
        assert!(
            err.message.contains("cannot hold a capability in a field"),
            "{:?}",
            err.message
        );
    }

    /// plans/M7.md item E4: an `@actor` may hold `Actor[D]` even when `D`
    /// is a `@driver` whose fields include capabilities. The handle is
    /// not the driver's authority.
    #[test]
    fn an_actor_may_hold_an_actor_handle_to_a_driver() {
        check_ok(&format!(
            "{CAP_PRELUDE}@driver\npub struct D:\n    regs: Mmio[Regs]\n\n\
             @actor\npub struct A:\n    disk: Actor[D]\n\n\
             \x20   init(mut self, disk: Actor[D]):\n        self.disk = disk\n"
        ));
    }

    // --- typed MMIO (plans/M7.md item C, 03-hardware.md §2) -------------
    //
    // Every source-shaped rejection is a golden (`tests/golden/err-mmio-*`)
    // — that is the review surface, same discipline as `@layout` above.
    // These cover the two things a golden structurally cannot: that the
    // register table is read back out of `check_layouts`' own product
    // rather than out of hand-written text, and the *accepting* half of
    // the claim rule whose acceptance a golden shows only implicitly.

    #[test]
    fn mmio_registers_are_read_back_from_the_checked_layout() {
        let layouts = layouts_of(MMIO_EXAMPLE).expect("03-hardware.md §2's own example");
        let l = &layouts[0];

        let status = mmio_register(l, "interrupt_status").expect("a declared register");
        assert_eq!(status.direction, Some(MmioDirection::ReadOnly));
        assert_eq!(status.scalar, "u32");
        assert_eq!((status.offset, status.size), (0x60, 4));

        let ack = mmio_register(l, "interrupt_ack").expect("a declared register");
        assert_eq!(ack.direction, Some(MmioDirection::WriteOnly));
        assert_eq!(ack.scalar, "u32");
        assert_eq!((ack.offset, ack.size), (0x64, 4));

        assert_eq!(mmio_register(l, "nope"), None);
        assert_eq!(
            mmio_register_names(l),
            vec!["interrupt_status".to_string(), "interrupt_ack".to_string()]
        );

        // An `mmio` field with no wrapper has no direction — not a third
        // direction, and not a default. `golden/err-mmio-undirected-register`
        // is what a source author sees; this is the table entry behind it.
        let bare = layouts_of(
            "module t\n\n@layout(mmio, endian=little)\nstruct S:\n\
             \x20   @offset(0x000) plain: u16\n",
        )
        .expect("a bare mmio field is a legal `@layout`");
        let reg = mmio_register(&bare[0], "plain").expect("a declared register");
        assert_eq!(reg.direction, None);
        assert_eq!(reg.scalar, "u16");
    }

    /// The claim rule's accepting half, and the one shape 03-hardware.md
    /// §1's own worked example needs: a driver whose `init` takes
    /// `take regs: Mmio[L]` *and* whose field holds `Mmio[L]` mints that
    /// layout exactly once. Reading the parameter as a second mint would
    /// make §1's constructor self-aliasing, which is why a parameter is
    /// deliberately not a mint (this section's own note).
    #[test]
    fn an_mmio_parameter_delivering_a_field_is_not_a_second_mint() {
        check_ok(&format!(
            "{CAP_PRELUDE}@driver\npub struct D:\n\
             \x20   held: DeviceCap[Blk]\n\
             \x20   regs: Mmio[Regs]\n\
             \x20   n: u32\n\n\
             \x20   init(mut self, take cap: DeviceCap[Blk], take regs: Mmio[Regs]):\n\
             \x20       self.held = take cap\n\
             \x20       self.regs = take regs\n\
             \x20       self.n = 0\n"
        ));
    }

    /// One `@layout(mmio)` type and one driver holding it — the prefix
    /// every access-shape guard below shares.
    const MMIO_PRELUDE: &str = "module t\n\n\
         @layout(mmio, endian=little)\n\
         struct Regs:\n\
         \x20   @offset(0x000) status: ReadOnly[u32]\n\
         \x20   @offset(0x004) ack: WriteOnly[u32]\n\n\
         @driver\npub struct D:\n\
         \x20   regs: Mmio[Regs]\n\n";

    /// Every MMIO access *shape* guard whose own golden would say nothing
    /// a reader could not predict from the accepting case
    /// (`declaration_shape_guards`/`capability_shape_guards` above set the
    /// precedent). The rules with a story — direction, width, endianness,
    /// unknown register, register-as-a-value — are goldens.
    #[test]
    fn mmio_access_shape_guards() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "a bare selection of an unknown register names the mistake it is",
                "        r = self.regs.nope\n        return 0\n",
                "declares no register `nope`",
            ),
            (
                "a register has no operation but read/write",
                "        self.regs.ack.poke(1)\n        return 0\n",
                "has no operation `poke`",
            ),
            (
                "an `Mmio[L]` itself has no methods",
                "        self.regs.reset()\n        return 0\n",
                "has no method `reset`",
            ),
            (
                "`read()` takes no arguments",
                "        return self.regs.status.read(1)\n",
                "read()` takes no arguments",
            ),
            (
                "`write(v)` takes exactly one",
                "        self.regs.ack.write()\n        return 0\n",
                "takes exactly one argument",
            ),
            (
                "...and not two",
                "        self.regs.ack.write(1, 2)\n        return 0\n",
                "takes exactly one argument",
            ),
            (
                "`write(v)`'s value is positional",
                "        self.regs.ack.write(value=1)\n        return 0\n",
                "names no parameter",
            ),
        ];
        for (what, body, needle) in cases {
            let src = format!("{MMIO_PRELUDE}    fn go(read self) -> u32:\n{body}");
            let err = check_err(&src);
            assert!(
                err.message.contains(needle),
                "{what}: expected {needle:?} in {:?}",
                err.message
            );
        }
    }

    /// The claim walk sees a layout wherever a driver field's type carries
    /// one, because each of those is still a layout the driver holds live.
    /// **Every** composite arm of `collect_mmio_layouts` is listed here,
    /// and every one is source-reachable — this is the test that keeps
    /// that claim honest rather than assumed. One test rather than eight
    /// goldens: the rule is the same rule each time, and the composite
    /// arms are `type_contains_capability`'s own shape reused (this
    /// section's own note).
    #[test]
    fn the_claim_walk_reaches_a_layout_through_every_composite() {
        for nested in [
            "Option[Mmio[Regs]]",
            "[Mmio[Regs]; 2]",
            "(Mmio[Regs], u32)",
            "Result[Mmio[Regs], u32]",
            "Static[Mmio[Regs]]",
            "fn() -> Mmio[Regs]",
            "own[P] Mmio[Regs]",
            "Wrapper",
        ] {
            let src = format!(
                "module t\n\npool P\n\n\
                 @layout(mmio, endian=little)\n\
                 struct Regs:\n\
                 \x20   @offset(0x000) status: ReadOnly[u32]\n\n\
                 struct Wrapper:\n\
                 \x20   inner: Mmio[Regs]\n\n\
                 @driver\npub struct D:\n\
                 \x20   a: Mmio[Regs]\n\
                 \x20   b: {nested}\n"
            );
            let err = check_err(&src);
            assert!(
                err.message.contains("alias the same register"),
                "a layout inside `{nested}` is still live: {:?}",
                err.message
            );
        }
    }

    /// A struct that is not a `@driver` has no claim, so it partitions
    /// nothing and this rule does not apply to it — provenance
    /// (`hardware.capabilities.provenance`) is what governs who may touch
    /// what it holds. Asserted directly because it is an *absence*: the
    /// `!d.is_driver` skip is otherwise invisible.
    #[test]
    fn a_struct_with_no_claim_partitions_nothing() {
        check_ok(
            "module t\n\n\
             @layout(mmio, endian=little)\n\
             struct A:\n\
             \x20   @offset(0x000) x: ReadOnly[u32]\n\n\
             @layout(mmio, endian=little)\n\
             struct B:\n\
             \x20   @offset(0x000) y: ReadOnly[u32]\n\n\
             struct Holder:\n\
             \x20   a: Mmio[A]\n\
             \x20   b: Mmio[B]\n",
        );
    }

    /// A layout consumes its declared *registers*, never its declared
    /// holes. Two layouts whose holes cover each other's registers are
    /// disjoint partitions of one claim — which is exactly 03 §2's own
    /// worked example (an ISR partition at 0x60 alongside the sealed
    /// transport's partition below it).
    #[test]
    fn a_declared_hole_consumes_nothing_from_the_claim() {
        check_ok(
            "module t\n\n\
             @layout(mmio, endian=little)\n\
             struct Irq:\n\
             \x20   @offset(0x060) status: ReadOnly[u32]\n\n\
             @layout(mmio, endian=little)\n\
             struct Transport:\n\
             \x20   @offset(0x000) device_status: WriteOnly[u32]\n\n\
             @driver\npub struct D:\n\
             \x20   irq: Mmio[Irq]\n\
             \x20   transport: Mmio[Transport]\n",
        );
    }
}
