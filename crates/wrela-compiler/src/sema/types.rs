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
    self, AccessMode, Attr, ConstItem, EnumItem, Expr, FnItem, GenericArg, GenericParam, InitItem,
    Item, Member, Module, NamedType, Span, StructItem, VariantPayload,
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
    pub generics: Vec<DeclGenericParam>,
    pub receiver: Option<DeclReceiver>,
    pub params: Vec<DeclParam>,
    pub ret: Type,
}

#[derive(Debug, Clone)]
pub struct DeclField {
    pub name: String,
    pub ty: Type,
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
    /// `pub(crate)` (item H): see `DeclStruct::component_types`.
    pub(crate) component_types: Vec<(Type, Span)>,
    pub(crate) span: Span,
}

#[derive(Debug, Clone)]
pub struct DeclConst {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub enum DeclItem {
    Const(DeclConst),
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
fn build_shapes(module: &Module) -> BTreeMap<String, usize> {
    let mut shapes = BTreeMap::new();
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
    let shapes = build_shapes(module);
    let module_pools = module_pool_names(module);
    let mut items = Vec::new();
    for item in &module.items {
        match item {
            Item::Const(c) => {
                items.push(DeclItem::Const(declare_const(c, &shapes, &module_pools)?))
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
    Ok(items)
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
            let Type::Named(actor_name, _) = inner else {
                return Err(SemaError::at(
                    "type",
                    format!(
                        "`Actor[{}]` must name an `@actor`/`@driver` struct",
                        render_type(inner)
                    ),
                    span,
                ));
            };
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
// one more `validate_actor_type` never needed — a *named struct*'s own
// declared fields (`component_types`), so a plain data struct with an
// `Actor[T]` field, passed by value as a message argument, is caught too
// (a `BTreeSet` cycle guard, `seen`, makes this safe against a
// self-referential struct shape — `classify_all`'s own infinite-size
// check already rejects a genuinely infinite one before this ever runs,
// but a merely self-*referential-through-`own`* one is legal data and
// must not infinite-loop this walk).
fn type_contains_actor_handle(
    ty: &Type,
    structs: &BTreeMap<String, &DeclStruct>,
    seen: &mut BTreeSet<String>,
) -> bool {
    match ty {
        Type::Named(name, _) if name == "Actor" => true,
        Type::Array(elem, _) => type_contains_actor_handle(elem, structs, seen),
        Type::Tuple(elems) => elems
            .iter()
            .any(|e| type_contains_actor_handle(e, structs, seen)),
        Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
            type_contains_actor_handle(inner, structs, seen)
        }
        Type::Result(ok, err) => {
            type_contains_actor_handle(ok, structs, seen)
                || type_contains_actor_handle(err, structs, seen)
        }
        Type::Fn(params, ret) => {
            params
                .iter()
                .any(|(_, t)| type_contains_actor_handle(t, structs, seen))
                || type_contains_actor_handle(ret, structs, seen)
        }
        Type::Named(name, targs) => {
            if !seen.insert(name.clone()) {
                return false; // already visited on this path: cycle guard.
            }
            let via_fields = structs.get(name.as_str()).is_some_and(|s| {
                s.component_types
                    .iter()
                    .any(|(t, _)| type_contains_actor_handle(t, structs, seen))
            });
            let via_targs = targs.iter().any(
                |a| matches!(a, TypeArg::Type(t) if type_contains_actor_handle(t, structs, seen)),
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
    structs: &BTreeMap<String, &DeclStruct>,
) -> Result<(), SemaError> {
    for p in params {
        if type_contains_actor_handle(&p.ty, structs, &mut BTreeSet::new()) {
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
    if type_contains_actor_handle(ret, structs, &mut BTreeSet::new()) {
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
    Ok(())
}

fn validate_actor_handles(module: &Module, items: &[DeclItem]) -> Result<(), SemaError> {
    let mut structs: BTreeMap<String, &DeclStruct> = BTreeMap::new();
    for item in items {
        if let DeclItem::Struct(s) = item {
            structs.insert(s.name.clone(), s);
        }
    }
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
                                    &d.name, &f.name, f.span, &fd.params, &fd.ret, &structs,
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
    let ret = resolve_ret(&f.ret, shapes, module_pools, local_pools, &scope)?;
    Ok(DeclFn {
        name: f.name.clone(),
        is_async: f.is_async,
        generics: decl_generics,
        receiver,
        params,
        ret,
    })
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
    let ret = resolve_ret(&i.ret, shapes, module_pools, local_pools, outer_generics)?;
    Ok(DeclFn {
        name: "init".to_string(),
        is_async: false,
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

fn resolve_ret(
    ret: &Option<ast::Type>,
    shapes: &BTreeMap<String, usize>,
    module_pools: &BTreeSet<String>,
    local_pools: &BTreeSet<String>,
    generics: &BTreeMap<String, GenericKind>,
) -> Result<Type, SemaError> {
    match ret {
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
/// list): `Format` needs no shape check; `From` needs exactly one
/// variant with exactly one field/payload (a struct has no variants, so
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
            "Format" => {}
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
    Ok(DeclStruct {
        name: s.name.clone(),
        generics: decl_generics,
        deriving: s.deriving.clone(),
        classification: Classification::Data, // placeholder; classify_all fills this in
        members,
        is_resource_fiat: s.is_resource || has_actor_or_driver(&s.attrs),
        is_actor: has_actor_or_driver(&s.attrs),
        component_types,
        span: s.span,
    })
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
    for v in &e.variants {
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
    Ok(DeclEnum {
        name: e.name.clone(),
        generics: decl_generics,
        deriving: e.deriving.clone(),
        classification: Classification::Data, // placeholder; classify_all fills this in
        variants,
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
        _ => {}
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

fn classify_all(items: &mut [DeclItem]) -> Result<(), SemaError> {
    let mut order = Vec::new();
    {
        let mut structs: BTreeMap<String, &DeclStruct> = BTreeMap::new();
        let mut enums: BTreeMap<String, &DeclEnum> = BTreeMap::new();
        for item in items.iter() {
            match item {
                DeclItem::Struct(s) => {
                    order.push(s.name.clone());
                    structs.insert(s.name.clone(), s);
                }
                DeclItem::Enum(e) => {
                    order.push(e.name.clone());
                    enums.insert(e.name.clone(), e);
                }
                _ => {}
            }
        }
        let mut memo = BTreeMap::new();
        let mut in_progress = BTreeSet::new();
        for name in &order {
            let span = structs
                .get(name)
                .map(|s| s.span)
                .or_else(|| enums.get(name).map(|e| e.span))
                .expect("name came from struct/enum scan above");
            classify_named(name, span, &structs, &enums, &mut memo, &mut in_progress)?;
        }
        for item in items.iter_mut() {
            match item {
                DeclItem::Struct(s) => s.classification = memo[&s.name],
                DeclItem::Enum(e) => e.classification = memo[&e.name],
                _ => {}
            }
        }
    }
    Ok(())
}

fn classify_named(
    name: &str,
    call_span: Span,
    structs: &BTreeMap<String, &DeclStruct>,
    enums: &BTreeMap<String, &DeclEnum>,
    memo: &mut BTreeMap<String, Classification>,
    in_progress: &mut BTreeSet<String>,
) -> Result<Classification, SemaError> {
    if let Some(c) = memo.get(name) {
        return Ok(*c);
    }
    if in_progress.contains(name) {
        return Err(SemaError::at(
            "type",
            format!("`{name}` is infinitely sized (recursive by value)"),
            call_span,
        ));
    }
    in_progress.insert(name.to_string());
    let mut resource = false;
    if let Some(s) = structs.get(name) {
        resource = s.is_resource_fiat;
        for (ty, span) in &s.component_types {
            if classify_type(ty, *span, structs, enums, memo, in_progress)?
                == Classification::Resource
            {
                resource = true;
            }
        }
    } else if let Some(e) = enums.get(name) {
        for (ty, span) in &e.component_types {
            if classify_type(ty, *span, structs, enums, memo, in_progress)?
                == Classification::Resource
            {
                resource = true;
            }
        }
    } else {
        // Neither a declared struct nor enum: a builtin `Type::Named`
        // this module resolves without a backing declaration (plans/
        // M4.md item B, decision 5 — `Image`, and `sema::bodies`'s own
        // `ImageDecl`/`Duration`/`RestartIntensity` intrinsic-surface
        // pseudo-types, none registered here since nothing declares
        // them). Every one of these is plain data (never a resource
        // fiat, never composed from one), so this falls through to the
        // same `Classification::Data` a genuinely field-less struct
        // would get — not `unreachable!()`, since `resolve_named` (this
        // file) now legitimately produces such a name.
        in_progress.remove(name);
        memo.insert(name.to_string(), Classification::Data);
        return Ok(Classification::Data);
    }
    in_progress.remove(name);
    let result = if resource {
        Classification::Resource
    } else {
        Classification::Data
    };
    memo.insert(name.to_string(), result);
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

fn classify_type(
    ty: &Type,
    span: Span,
    structs: &BTreeMap<String, &DeclStruct>,
    enums: &BTreeMap<String, &DeclEnum>,
    memo: &mut BTreeMap<String, Classification>,
    in_progress: &mut BTreeSet<String>,
) -> Result<Classification, SemaError> {
    let mut error: Option<SemaError> = None;
    let is_resource = resource_propagates(ty, &mut |name, _args| {
        if error.is_some() {
            return false;
        }
        match classify_named(name, span, structs, enums, memo, in_progress) {
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
        Type::Own(pool, inner) => format!("own[{pool}] {}", render_type(inner)),
        Type::Static(t) => format!("Static[{}]", render_type(t)),
        Type::Bytes(None) => "Bytes".to_string(),
        Type::Bytes(Some(n)) => format!("Bytes[{}]", printer::print_expr_bare(n)),
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
                render_member(m, depth + 1, &s.name, effects, out);
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
        }
        DeclItem::Pool(name) => push_line(out, depth, &format!("Pool {name}")),
    }
}

fn render_member(
    m: &DeclMember,
    depth: usize,
    struct_name: &str,
    effects: &BTreeMap<(String, String), AccessMode>,
    out: &mut String,
) {
    match m {
        DeclMember::Field(f) => push_line(
            out,
            depth,
            &format!("field {}: {}", f.name, render_type(&f.ty)),
        ),
        DeclMember::Fn(f) => {
            let prefix = if f.is_async { "async fn " } else { "fn " };
            // Only a private (`!is_pub`) plain-`self` (`Read`, not `init`)
            // receiver is ambiguous enough to need the access pass's
            // inferred effect (types.rs's own `render_receiver` doc
            // comment); every other shape is already unambiguous in
            // source and needs no lookup.
            let override_mode = f.receiver.as_ref().and_then(|r| {
                if r.mode == AccessMode::Read && !r.is_pub && !r.is_init {
                    effects
                        .get(&(struct_name.to_string(), f.name.clone()))
                        .copied()
                } else {
                    None
                }
            });
            push_line(
                out,
                depth,
                &format!("{prefix}{}", render_fn_signature(f, override_mode)),
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
}
