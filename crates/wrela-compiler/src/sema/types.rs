use std::collections::{BTreeMap, BTreeSet};

use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{
    self, AccessMode, Arg, Attr, BinOp, ConstItem, EnumItem, Expr, FieldItem, FnItem, GenericArg,
    GenericParam, InitItem, Item, MatchArm, MatchStmt, Member, Module, NamedType, Param, Pattern,
    Receiver, Span, Stmt, StructItem, VariantPayload,
};
use crate::syntax::printer;

pub use super::layout_types::LayoutKind;

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
    Array(Box<Type>, Box<Expr>),
    Tuple(Vec<Type>),
    Option(Box<Type>),
    Result(Box<Type>, Box<Type>),
    Own(String, Box<Type>),
    Static(Box<Type>),
    Bytes(Option<Box<Expr>>),
    String(Box<Expr>),
    Fn(Vec<(AccessMode, Type)>, Box<Type>),
    Generic(String),
    Named(String, Vec<TypeArg>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeArg {
    Type(Type),
    Const(Expr),
    Bound(Expr),
    Pool(String),
}

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

#[derive(Debug, Clone)]
pub struct DeclReceiver {
    pub mode: Option<AccessMode>,
    pub is_pub: bool,
    pub is_init: bool,
}

#[derive(Debug, Clone)]
pub struct DeclFn {
    pub name: String,
    pub is_async: bool,
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
    pub(crate) is_resource_fiat: bool,
    pub(crate) is_actor: bool,
    pub(crate) is_driver: bool,
    pub(crate) layout_kind: Option<LayoutKind>,
    pub(crate) component_types: Vec<(Type, Span)>,
    pub(crate) span: Span,
    pub(crate) is_manual_resource: bool,
    pub(crate) classes: crate::sema::classes::TypeClasses,
    pub(crate) classes_assigned: bool,
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
    pub members: Vec<DeclMember>,
    pub(crate) component_types: Vec<(Type, Span)>,
    pub(crate) span: Span,
    pub(crate) classes: crate::sema::classes::TypeClasses,
    pub(crate) classes_assigned: bool,
}

#[derive(Debug, Clone)]
pub struct DeclConst {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct DeclStatic {
    pub name: String,
    pub ty: Type,
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenericKind {
    Type,
    Const,
}

fn build_shapes(module: &Module, imported: &ImportedTypes) -> BTreeMap<String, usize> {
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

pub fn declare(module: &Module) -> Result<Vec<DeclItem>, SemaError> {
    declare_with_imports(module, &ImportedTypes::new())
}

pub type ImportedTypes = BTreeMap<String, usize>;

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
            Item::ComptimeIf(_) => {}
        }
    }
    classify_all(&mut items)?;
    crate::sema::classes::assign_classes(&mut items);
    validate_actor_handles(module, &items)?;
    validate_capability_types(module, &items)?;
    Ok(items)
}

fn validate_actor_type(
    ty: &Type,
    span: Span,
    structs: &BTreeMap<String, &DeclStruct>,
    canonical_renderers: &BTreeSet<String>,
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
            let sealed_renderer = canonical_renderers.contains(actor_name)
                && actor_targs.len() == 1
                && matches!(actor_targs[0], TypeArg::Type(_));
            if !actor_targs.is_empty() && !sealed_renderer {
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
                None if sealed_renderer => Ok(()),
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
        Type::Array(elem, _) => validate_actor_type(elem, span, structs, canonical_renderers),
        Type::Tuple(elems) => {
            for e in elems {
                validate_actor_type(e, span, structs, canonical_renderers)?;
            }
            Ok(())
        }
        Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
            validate_actor_type(inner, span, structs, canonical_renderers)
        }
        Type::Result(ok, err) => {
            validate_actor_type(ok, span, structs, canonical_renderers)?;
            validate_actor_type(err, span, structs, canonical_renderers)
        }
        Type::Fn(params, ret) => {
            for (_, t) in params {
                validate_actor_type(t, span, structs, canonical_renderers)?;
            }
            validate_actor_type(ret, span, structs, canonical_renderers)
        }
        Type::Named(_, targs) => {
            for a in targs {
                if let TypeArg::Type(t) = a {
                    validate_actor_type(t, span, structs, canonical_renderers)?;
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
    canonical_renderers: &BTreeSet<String>,
) -> Result<(), SemaError> {
    for p in params {
        validate_actor_type(&p.ty, span, structs, canonical_renderers)?;
    }
    validate_actor_type(ret, span, structs, canonical_renderers)
}

pub(crate) fn components_by_name(items: &[DeclItem]) -> BTreeMap<String, &[(Type, Span)]> {
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
                return false;
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
    structs: &BTreeMap<String, &DeclStruct>,
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
        if message_param_ty_is_resource(&p.ty, structs)
            && p.mode != AccessMode::Take
            && contains_capability(&p.ty, components).is_none()
        {
            return Err(SemaError::at(
                "actor",
                format!(
                    "message parameter `{struct_name}.{method_name}`'s `{}: {}` is a resource and \
                     must be declared `take` (02-language.md §9.3)",
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
    Ok(())
}

fn validate_actor_handles(module: &Module, items: &[DeclItem]) -> Result<(), SemaError> {
    let mut structs: BTreeMap<String, &DeclStruct> = BTreeMap::new();
    for item in items {
        if let DeclItem::Struct(s) = item {
            structs.insert(s.name.clone(), s);
        }
    }
    let canonical_renderers: BTreeSet<String> = module
        .imports
        .iter()
        .filter(|import| import.path == ["core", "render"])
        .flat_map(|import| &import.names)
        .filter(|name| name.name == "Renderer")
        .map(|name| name.alias.as_ref().unwrap_or(&name.name).clone())
        .collect();
    let components = components_by_name(items);
    let ast_items: Vec<&Item> = module
        .items
        .iter()
        .filter(|i| !matches!(i, Item::ComptimeIf(_)))
        .collect();
    for (ai, di) in ast_items.iter().zip(items.iter()) {
        match (ai, di) {
            (Item::Fn(f), DeclItem::Fn(d)) => {
                validate_fn_actor_types(f.span, &d.params, &d.ret, &structs, &canonical_renderers)?;
            }
            (Item::Struct(s), DeclItem::Struct(d)) => {
                for (ty, span) in &d.component_types {
                    validate_actor_type(ty, *span, &structs, &canonical_renderers)?;
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
                            validate_fn_actor_types(
                                f.span,
                                &fd.params,
                                &fd.ret,
                                &structs,
                                &canonical_renderers,
                            )?;
                            if d.is_actor && f.is_pub && f.receiver.is_some() {
                                validate_message_shape(
                                    &d.name,
                                    &f.name,
                                    f.span,
                                    &fd.params,
                                    &fd.ret,
                                    &components,
                                    &structs,
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
                            validate_fn_actor_types(
                                i.span,
                                &id.params,
                                &id.ret,
                                &structs,
                                &canonical_renderers,
                            )?;
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

fn type_carries_named(
    ty: &Type,
    components: &BTreeMap<String, &[(Type, Span)]>,
    seen: &mut BTreeSet<String>,
    leaf: &dyn Fn(&str) -> bool,
) -> Option<String> {
    match ty {
        Type::Named(name, _) if leaf(name) => Some(render_type(ty)),
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
                return None;
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

fn type_contains_capability(
    ty: &Type,
    components: &BTreeMap<String, &[(Type, Span)]>,
    seen: &mut BTreeSet<String>,
) -> Option<String> {
    type_carries_named(ty, components, seen, &|n| {
        crate::sema::classes::name_holds_authority(n)
    })
}

fn contains_capability(
    ty: &Type,
    components: &BTreeMap<String, &[(Type, Span)]>,
) -> Option<String> {
    type_contains_capability(ty, components, &mut BTreeSet::new())
}

pub fn sealed_authority_carried(ty: &Type, items: &[DeclItem]) -> Option<String> {
    contains_capability(ty, &components_by_name(items))
}

pub fn driver_message_forbidden_carried(ty: &Type, items: &[DeclItem]) -> Option<String> {
    type_carries_named(ty, &components_by_name(items), &mut BTreeSet::new(), &|n| {
        crate::sema::classes::name_forbidden_in_driver_message(n)
    })
}

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum CapOwner {
    Driver,
    Actor,
    Plain,
}

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
                return None;
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
        if p.mode != AccessMode::Take {
            if let Some(found) = dma_shared_in_type(&p.ty, components, &mut BTreeSet::new()) {
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
                for (ty, span) in &e.component_types {
                    validate_capability_args(ty, *span, &structs)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub struct CapabilityAuthority {
    pub roots: BTreeSet<String>,
    pub spans: BTreeMap<String, Span>,
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
                if contains_capability(&Type::Named(e.name.clone(), Vec::new()), &components)
                    .is_some()
                {
                    capability_bearing.insert(e.name.clone());
                }
            }
            _ => {}
        }
    }
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

pub use super::layout_types::{
    LayoutEndian, LayoutEntry, LayoutField, LayoutType, MmioDirection, MmioRegister, check_layouts,
    check_mmio_claims, complete_layouts, driver_mmio_mints, dump_layouts, mmio_consumed_end,
    mmio_mints_of, mmio_register, mmio_register_names, push_layout_lines, validate_placed_statics,
};
#[cfg(test)]
pub(crate) use super::layout_types::{MAX_LAYOUT_NEST_DEPTH, MAX_LAYOUT_NEST_EXPANSIONS};

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
        None => Err(unimplemented_at("a const's inferred type is", c.span)),
    }
}

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

fn check_task_attr_args(attr: &Attr) -> Result<(), SemaError> {
    for a in &attr.args {
        match a.label.as_deref() {
            Some("trigger") | Some("poll") => {}
            Some("priority") => {
                return Err(SemaError::at(
                    "type",
                    "`@task(priority=...)` was cut at the revision boundary (plans/M13.md \
                     item C / decision 13); scheduling is FIFO-per-mailbox + round-robin \
                     (04-compiler.md §2); priority bands are a recorded future intention"
                        .to_string(),
                    a.span,
                ));
            }
            Some("budget") => {
                return Err(SemaError::at(
                    "type",
                    "`@task(budget=...)` was cut at the revision boundary (plans/M13.md \
                     item C / decision 13); the FIFO+RR scheduler cannot honor a \
                     per-task budget; budget bands are a recorded future intention"
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

pub(crate) const INFERRED_ERROR_SET_NAME: &str = "__InferredErrorSet";

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
        Some(t) => resolve_type(t, shapes, module_pools, local_pools, generics, false),
        None => Ok(Type::Unit),
    }
}

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

pub(crate) fn scalar_format_bound(ty: &Type) -> Option<u64> {
    match ty {
        Type::Bool => Some(5),
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

pub(crate) fn secret_has_no_format(span: Span) -> SemaError {
    SemaError::at(
        "type",
        "`Secret` has no `Format` (05-library.md §6)".to_string(),
        span,
    )
}

fn string_bound_ast_ty(span: Span, n: u64) -> ast::Type {
    ast::Type::Named(NamedType {
        span,
        name: "String".to_string(),
        args: vec![GenericArg::Bound(Expr::Int(span, n.to_string()))],
    })
}

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

fn derived_from_conflict(type_name: &str, span: Span) -> SemaError {
    SemaError::at(
        "type",
        format!("deriving(From) conflicts with an explicit `from` on `{type_name}`"),
        span,
    )
}

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
            mode: Some(AccessMode::Read),
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
            mode: Some(AccessMode::Read),
        }),
        params: Vec::new(),
        ret: Some(string_bound_ast_ty(span, bound)),
        body: Some(vec![Stmt::Return(span, Some(str_lit(span, type_name)))]),
    }
}

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
            mode: Some(AccessMode::Read),
        }),
        params: Vec::new(),
        ret: Some(string_bound_ast_ty(span, bound)),
        body: Some(vec![Stmt::Return(span, Some(expr))]),
    }
}

pub(crate) fn struct_format_bound(
    type_name: &str,
    fields: &[(String, Type)],
    span: Span,
) -> Result<u64, SemaError> {
    if fields.is_empty() {
        return Ok(type_name.len() as u64);
    }
    let mut bound = type_name.len() as u64 + 1;
    for (i, (fname, fty)) in fields.iter().enumerate() {
        if i > 0 {
            bound += 2;
        }
        bound += fname.len() as u64 + 1;
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
    Ok(bound + 1)
}

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
            mode: Some(AccessMode::Read),
        }),
        params: Vec::new(),
        ret: Some(string_bound_ast_ty(span, bound)),
        body: Some(vec![Stmt::Match(MatchStmt {
            span,
            scrutinee: Expr::Name(span, "self".to_string()),
            arms,
            discard: None,
        })]),
    }
}

pub(crate) fn is_format_max_formatted_len(d: &DeclFn) -> bool {
    d.name == "max_formatted_len"
        && d.receiver.is_none()
        && d.params.is_empty()
        && d.ret == Type::Usize
        && !d.is_async
}

pub(crate) fn is_format_writer(d: &DeclFn) -> bool {
    d.name == "format"
        && matches!(
            &d.receiver,
            Some(r) if r.mode.is_none()
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
            Member::ComptimeIf(_) => {}
        }
    }
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
        classification: Classification::Data,
        members,
        is_resource_fiat: s.is_resource || has_actor_or_driver(&s.attrs),
        is_actor: has_actor_or_driver(&s.attrs),
        is_driver: s.attrs.iter().any(|a| a.name == "driver"),
        layout_kind: declared_layout_kind(&s.attrs),
        component_types,
        span: s.span,
        is_manual_resource: s.is_manual_resource,
        classes: crate::sema::classes::TypeClasses::default(),
        classes_assigned: false,
    })
}

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
        is_manual_resource: template.is_manual_resource,
        classes: template.classes,
        classes_assigned: template.classes_assigned,
    })
}

pub(crate) fn declared_layout_kind(attrs: &[Attr]) -> Option<LayoutKind> {
    let attr = attrs.iter().find(|a| a.name == "layout")?;
    let arg = attr.args.iter().find(|a| a.label.is_none())?;
    let Expr::Name(_, kind) = &arg.value else {
        return None;
    };
    match kind.as_str() {
        "dma" => Some(LayoutKind::Dma),
        "mmio" => Some(LayoutKind::Mmio),
        "wire" => Some(LayoutKind::Wire),
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
        classification: Classification::Data,
        variants,
        members,
        component_types,
        span: e.span,
        classes: crate::sema::classes::TypeClasses::default(),
        classes_assigned: false,
    })
}

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
            if let Some(n) = crate::sema::bodies::literal_array_len(&a.len) {
                if !crate::sema::bodies::array_len_fits(n) {
                    return Err(SemaError::at(
                        "type",
                        format!(
                            "array length {n} exceeds the {}-element build limit",
                            crate::sema::bodies::MAX_ARRAY_LEN
                        ),
                        a.span,
                    ));
                }
            }
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
        GenericArg::Expr(e) => {
            if let Some(len) = crate::sema::bodies::literal_array_len(e) {
                if !crate::sema::bodies::array_len_fits(len) {
                    return Err(SemaError::at(
                        "type",
                        format!(
                            "`Bytes[N]` length {len} exceeds the {}-element build limit",
                            crate::sema::bodies::MAX_ARRAY_LEN
                        ),
                        n.span,
                    ));
                }
            }
            Ok(Type::Bytes(Some(Box::new(e.clone()))))
        }
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

fn resolve_string(n: &NamedType) -> Result<Type, SemaError> {
    if n.args.is_empty() {
        return Err(unimplemented_at("`String` (bound-elided) is", n.span));
    }
    expect_arity(n, 1)?;
    match &n.args[0] {
        GenericArg::Bound(e) => {
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

pub fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
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
            | "Option"
            | "Result"
            | "CallError"
            | "Admission"
            | "Static"
            | "Bytes"
            | "String"
            | "Image"
            | "Actor"
            | "BootError"
            | "VirtQueue"
            | "QueuePermit"
            | "QueueOp"
            | "Receipt"
            | "IoCompletion"
            | "CompletionOutcome"
            | "Target"
            | "Transport"
            | "Failure"
            | "DriverMode"
            | "ReadOnly"
            | "WriteOnly"
            | "Untrusted"
            | "Secret"
            | "InterruptCell"
            | "Duration"
            | "Instant"
            | "GroupId"
    ) || crate::sema::classes::name_holds_authority(name)
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
        "Image" => Some(Type::Named("Image".to_string(), vec![])),
        "BootError" => Some(Type::Named("BootError".to_string(), vec![])),
        "QueuePermit" => Some(Type::Named(n.name.clone(), vec![])),
        "DriverMode" | "Target" | "Transport" | "Failure" => {
            Some(Type::Named(n.name.clone(), vec![]))
        }
        "CompletionOutcome" => Some(Type::Named("CompletionOutcome".to_string(), vec![])),
        "Admission" => Some(Type::Named("Admission".to_string(), vec![])),
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
        "CallError" => {
            let args = expect_type_args(n, 1)?;
            let e = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named("CallError".to_string(), vec![TypeArg::Type(e)]));
        }
        "Static" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Static(Box::new(inner)));
        }
        "Bytes" => return resolve_bytes(n, param_position),
        "String" => return resolve_string(n),
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
        "Receipt" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named(
                "Receipt".to_string(),
                vec![TypeArg::Type(inner)],
            ));
        }
        "IoCompletion" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named(
                "IoCompletion".to_string(),
                vec![TypeArg::Type(inner)],
            ));
        }
        "Actor" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named("Actor".to_string(), vec![TypeArg::Type(inner)]));
        }
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
        "ReadOnly" | "WriteOnly" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named(n.name.clone(), vec![TypeArg::Type(inner)]));
        }
        "Untrusted" => {
            let args = expect_type_args(n, 1)?;
            let inner = resolve_type(args[0], shapes, module_pools, local_pools, generics, false)?;
            return Ok(Type::Named(
                "Untrusted".to_string(),
                vec![TypeArg::Type(inner)],
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
    if let Some(arity) = crate::eval::image_checks::capability_generic_arity(&n.name) {
        expect_arity(n, arity)?;
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

struct ClassifyNode<'a> {
    is_resource_fiat: bool,
    component_types: &'a [(Type, Span)],
    span: Span,
}

type ClassifyTables<'a> = BTreeMap<Vec<String>, Vec<(String, ClassifyNode<'a>)>>;

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

type ClassifyMemo = BTreeMap<(Vec<String>, String), Classification>;

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
        crate::sema::classes::assign_classes(decls);
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
        let c = classify_named(tmod, tname, call_span, tables, imports, memo, in_progress)?;
        in_progress.remove(&key);
        memo.insert(key, c);
        return Ok(c);
    } else if crate::sema::classes::name_holds_authority(name) {
        in_progress.remove(&key);
        memo.insert(key, Classification::Resource);
        return Ok(Classification::Resource);
    } else {
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

fn message_param_ty_is_resource(ty: &Type, structs: &BTreeMap<String, &DeclStruct>) -> bool {
    resource_propagates(ty, &mut |name, _args| {
        if crate::sema::classes::name_holds_authority(name) {
            return true;
        }
        structs
            .get(name)
            .map(|s| s.is_resource_fiat || s.classification == Classification::Resource)
            .unwrap_or(false)
    })
}

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

pub fn render_items(
    items: &[DeclItem],
    effects: &BTreeMap<(String, String), AccessMode>,
    out: &mut String,
) {
    for item in items {
        render_item(item, 1, effects, out);
    }
}

pub(crate) fn push_line(out: &mut String, depth: usize, line: &str) {
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

fn render_receiver(r: &DeclReceiver, override_mode: Option<AccessMode>) -> String {
    match r.mode {
        Some(AccessMode::Mut) => "mut self".to_string(),
        Some(AccessMode::Take) => "take self".to_string(),
        Some(AccessMode::Read) => "read self".to_string(),
        None => match override_mode {
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
                    "Struct {}{} {}{}{}",
                    s.name,
                    render_generics(&s.generics),
                    classification_str(s.classification),
                    if s.is_manual_resource { " manual" } else { "" },
                    render_deriving(&s.deriving)
                ),
            );
            if s.is_manual_resource {
                push_line(out, depth + 1, &s.classes.render_line());
            }
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
            for m in &e.members {
                if let DeclMember::Fn(f) = m {
                    let prefix = if f.is_async { "async fn " } else { "fn " };
                    let override_mode = f.receiver.as_ref().and_then(|r| {
                        if r.mode.is_none() && !r.is_pub && !r.is_init {
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
            let override_mode = f.receiver.as_ref().and_then(|r| {
                if r.mode.is_none() && !r.is_pub && !r.is_init {
                    effects.get(&(owner.name.clone(), f.name.clone())).copied()
                } else {
                    None
                }
            });
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

        assert!(
            resource_propagates(
                &Type::Own("P".to_string(), Box::new(Type::U8)),
                &mut never_resource
            ),
            "own[P] T is always a resource regardless of T"
        );

        assert!(
            !resource_propagates(
                &Type::Static(Box::new(Type::Own("P".to_string(), Box::new(Type::U8)))),
                &mut always_resource
            ),
            "Static[T] is always data regardless of T"
        );

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

    fn layouts_of(src: &str) -> Result<Vec<LayoutType>, SemaError> {
        let tokens = crate::syntax::lexer::lex(src).expect("test source lexes");
        let module = crate::syntax::parser::parse(tokens).expect("test source parses");
        check_layouts(&module)
    }

    fn completed_layouts_of(src: &str) -> Result<Vec<LayoutType>, SemaError> {
        let tokens = crate::syntax::lexer::lex(src).expect("test source lexes");
        let module = crate::syntax::parser::parse(tokens).expect("test source parses");
        crate::sema::check_typed(&module, "t.wr").map(|p| p.layouts)
    }

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
        let done = completed_layouts_of(src).expect("the later pass completes it");
        assert_eq!(done[0].size, Some(24));
        assert_eq!(done[0].entries.len(), 2);
    }

    #[test]
    fn a_const_length_may_depend_on_another_const() {
        let done = completed_layouts_of(
            "module t\n\nconst BASE: u32 = 2\nconst N: u32 = BASE * 3\n\n\
             @layout(runtime, endian=little)\nstruct T:\n    turns: [u32; N]\n",
        )
        .expect("arithmetic in the `const`'s own initializer is the evaluator's job");
        assert_eq!(done[0].size, Some(24));
    }

    #[test]
    fn const_length_guards() {
        let cases: &[(&str, &str)] = &[
            (
                "module t\n\nconst N: i32 = -3\n\n@layout(runtime, endian=little)\n\
                 struct T:\n    turns: [u32; N]\n",
                "whose value is -3",
            ),
            (
                "module t\n\nconst N: bool = true\n\n@layout(runtime, endian=little)\n\
                 struct T:\n    turns: [u32; N]\n",
                "whose value is not an integer",
            ),
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
        assert_eq!(l.size, Some(0x68));
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
        let none = layouts_of("module t\n\nstruct S:\n    n: u32\n").unwrap();
        assert!(none.is_empty());
        assert_eq!(
            dump_layouts(&[("t".to_string(), none)]).expect("nothing to dump"),
            "LayoutTypes v0\n"
        );
    }

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
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct S:\n    a: [usize; 4]\n",
                "which is not an array field's element type",
            ),
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct S:\n    a: [bool; 4]\n",
                "which is not an array field's element type",
            ),
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct S:\n    a: [[u32; 2]; 2]\n",
                "which is not an array field's element type",
            ),
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct S:\n    a: [u32; -1]\n",
                "neither an integer literal nor the name of a module-level `const`",
            ),
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct S:\n    a: [u32; 0]\n",
                "has length 0",
            ),
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct E:\n\
                 \x20   a: u32\n    b: u8\n\n\
                 @layout(runtime, endian=little)\nstruct S:\n    e: [E; 2]\n",
                "would need implicit padding to be aligned",
            ),
            (
                "module t\n\n@layout(runtime, endian=little)\nstruct R:\n\
                 \x20   a: u32\n\n\
                 @layout(dma, endian=little)\nstruct S:\n    r: R\n",
                "nests a `@layout` type of a different kind",
            ),
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

    #[test]
    fn an_empty_layout_has_no_reportable_size() {
        let src = "module t\n\n@layout(dma, endian=little)\nstruct S:\n    pass\n";
        assert!(
            crate::syntax::lexer::lex(src)
                .ok()
                .and_then(|t| crate::syntax::parser::parse(t).ok())
                .map(|m| check_layouts(&m).is_err())
                .unwrap_or(true)
        );
    }

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

    #[test]
    fn image_decl_is_compiler_owned_and_cannot_be_spelled_in_source() {
        let error = check_err(
            "module t\n\nstruct Value:\n    id: u32\n\nfn forge(x: ImageDecl[Value]) -> u32:\n    return 0\n",
        );
        assert_eq!(error.category, "name");
        assert!(
            error.message.contains("`ImageDecl`"),
            "unexpected diagnostic: {}",
            error.message
        );
    }

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

    #[test]
    fn capability_shape_guards() {
        let cases: &[(&str, &str)] = &[
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
            (
                "fn f(take c: DmaPool[Option[u8], 4]) -> u32:\n    return 0\n",
                "takes no generic arguments of its own",
            ),
            (
                "fn f(take c: DmaShared[Slots, u32]) -> u32:\n    return 0\n",
                "must name an `@layout(dma)` struct",
            ),
            (
                "fn f(read c: Mmio[u32]) -> u32:\n    return 0\n",
                "must name an `@layout(mmio)` struct",
            ),
            (
                "fn f(read c: Mmio[Blk]) -> u32:\n    return 0\n",
                "requires `Blk` to be an `@layout(mmio)` struct",
            ),
            (
                "fn f(read c: Mmio[4]) -> u32:\n    return 0\n",
                "`Mmio` requires a type argument",
            ),
            (
                "fn f(read c: Option[Mmio[Blk]]) -> u32:\n    return 0\n",
                "requires `Blk` to be an `@layout(mmio)` struct",
            ),
            (
                "fn f(read c: [(u32, Mmio[Blk]); 2]) -> u32:\n    return 0\n",
                "requires `Blk` to be an `@layout(mmio)` struct",
            ),
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

    #[test]
    fn no_source_construct_produces_a_capability() {
        let cases: &[(&str, &str, &str)] = &[
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
            (
                "a module declaration under the name",
                "struct Mmio:\n    base: u64\n",
                "cannot be declared",
            ),
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
            (
                "a const declared as one",
                "const C: DmaPool[Slots, 4096] = 0\n",
                "no comptime value is one",
            ),
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

    #[test]
    fn an_actor_may_hold_an_actor_handle_to_a_driver() {
        check_ok(&format!(
            "{CAP_PRELUDE}@driver\npub struct D:\n    regs: Mmio[Regs]\n\n\
             @actor\npub struct A:\n    disk: Actor[D]\n\n\
             \x20   init(mut self, disk: Actor[D]):\n        self.disk = disk\n"
        ));
    }

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

        let bare = layouts_of(
            "module t\n\n@layout(mmio, endian=little)\nstruct S:\n\
             \x20   @offset(0x000) plain: u16\n",
        )
        .expect("a bare mmio field is a legal `@layout`");
        let reg = mmio_register(&bare[0], "plain").expect("a declared register");
        assert_eq!(reg.direction, None);
        assert_eq!(reg.scalar, "u16");
    }

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

    const MMIO_PRELUDE: &str = "module t\n\n\
         @layout(mmio, endian=little)\n\
         struct Regs:\n\
         \x20   @offset(0x000) status: ReadOnly[u32]\n\
         \x20   @offset(0x004) ack: WriteOnly[u32]\n\n\
         @driver\npub struct D:\n\
         \x20   regs: Mmio[Regs]\n\n";

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
