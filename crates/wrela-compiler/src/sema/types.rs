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
    is_resource_fiat: bool,
    /// Every field's resolved type + the field's own span, for the
    /// classification/infinite-size pass below — methods/init/pool
    /// members carry no data and do not contribute.
    component_types: Vec<(Type, Span)>,
    span: Span,
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
    component_types: Vec<(Type, Span)>,
    span: Span,
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
    Ok(items)
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
        unreachable!("classify_named: `{name}` is neither a known struct nor enum");
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

fn classify_type(
    ty: &Type,
    span: Span,
    structs: &BTreeMap<String, &DeclStruct>,
    enums: &BTreeMap<String, &DeclEnum>,
    memo: &mut BTreeMap<String, Classification>,
    in_progress: &mut BTreeSet<String>,
) -> Result<Classification, SemaError> {
    Ok(match ty {
        Type::Own(..) => Classification::Resource,
        Type::Static(_) => Classification::Data,
        Type::Named(name, _args) => classify_named(name, span, structs, enums, memo, in_progress)?,
        Type::Array(elem, _) => classify_type(elem, span, structs, enums, memo, in_progress)?,
        Type::Tuple(elems) => {
            let mut r = Classification::Data;
            for e in elems {
                if classify_type(e, span, structs, enums, memo, in_progress)?
                    == Classification::Resource
                {
                    r = Classification::Resource;
                }
            }
            r
        }
        Type::Option(inner) => classify_type(inner, span, structs, enums, memo, in_progress)?,
        Type::Result(ok, err) => {
            let a = classify_type(ok, span, structs, enums, memo, in_progress)?;
            let b = classify_type(err, span, structs, enums, memo, in_progress)?;
            if a == Classification::Resource || b == Classification::Resource {
                Classification::Resource
            } else {
                Classification::Data
            }
        }
        Type::Generic(_) => Classification::Data,
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
        | Type::Never => Classification::Data,
    })
}

// --- the check dump (decision 8) ------------------------------------------

pub fn render_items(items: &[DeclItem], out: &mut String) {
    for item in items {
        render_item(item, 1, out);
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

fn render_receiver(r: &DeclReceiver) -> String {
    match r.mode {
        AccessMode::Mut => "mut self".to_string(),
        AccessMode::Take => "take self".to_string(),
        AccessMode::Read if r.is_init || r.is_pub => "read self".to_string(),
        AccessMode::Read => "self".to_string(),
    }
}

fn render_fn_signature(f: &DeclFn) -> String {
    let mut parts = Vec::with_capacity(f.params.len() + 1);
    if let Some(r) = &f.receiver {
        parts.push(render_receiver(r));
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

fn render_type_arg(arg: &TypeArg) -> String {
    match arg {
        TypeArg::Type(t) => render_type(t),
        TypeArg::Const(e) => printer::print_expr_bare(e),
        TypeArg::Bound(e) => format!("..{}", printer::print_expr_bare(e)),
    }
}

fn render_item(item: &DeclItem, depth: usize, out: &mut String) {
    match item {
        DeclItem::Const(c) => push_line(
            out,
            depth,
            &format!("Const {}: {}", c.name, render_type(&c.ty)),
        ),
        DeclItem::Fn(f) => {
            let label = if f.is_async { "AsyncFn" } else { "Fn" };
            push_line(out, depth, &format!("{label} {}", render_fn_signature(f)));
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
                render_member(m, depth + 1, out);
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

fn render_member(m: &DeclMember, depth: usize, out: &mut String) {
    match m {
        DeclMember::Field(f) => push_line(
            out,
            depth,
            &format!("field {}: {}", f.name, render_type(&f.ty)),
        ),
        DeclMember::Fn(f) => {
            let prefix = if f.is_async { "async fn " } else { "fn " };
            push_line(out, depth, &format!("{prefix}{}", render_fn_signature(f)));
        }
        DeclMember::Init(f) => push_line(out, depth, &render_fn_signature(f)),
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
