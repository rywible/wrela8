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
    // plans/M7.md item A, decision 3: 03-hardware.md §1's capability rules
    // are the same *shape* as `validate_actor_handles` above, over a
    // different type set — the same post-declare position, the same
    // ast-alongside-`DeclItem` zip, the same "at any nesting" recursion.
    // A second pass rather than a second mechanism.
    validate_capability_types(module, &items)?;
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
    // No reply-shape rule beyond the `Actor[T]` one above survives here,
    // and the history is worth keeping because it was a *wrong answer*,
    // not a missing feature.
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

/// The capability type `ty` contains, at any nesting, rendered — or
/// `None`. The same walk `type_contains_actor_handle` performs (including
/// its `seen` cycle guard and its recursion through a named struct's own
/// declared fields, so a plain data struct wrapping a capability is caught
/// wherever the wrapper appears), asking about a different type set.
fn type_contains_capability(
    ty: &Type,
    structs: &BTreeMap<String, &DeclStruct>,
    seen: &mut BTreeSet<String>,
) -> Option<String> {
    match ty {
        Type::Named(name, _) if crate::eval::image_checks::is_capability_type_name(name) => {
            Some(render_type(ty))
        }
        Type::Array(elem, _) => type_contains_capability(elem, structs, seen),
        Type::Tuple(elems) => elems
            .iter()
            .find_map(|e| type_contains_capability(e, structs, seen)),
        Type::Own(_, inner) | Type::Static(inner) | Type::Option(inner) => {
            type_contains_capability(inner, structs, seen)
        }
        Type::Result(ok, err) => type_contains_capability(ok, structs, seen)
            .or_else(|| type_contains_capability(err, structs, seen)),
        Type::Fn(params, ret) => params
            .iter()
            .find_map(|(_, t)| type_contains_capability(t, structs, seen))
            .or_else(|| type_contains_capability(ret, structs, seen)),
        Type::Named(name, targs) => {
            if !seen.insert(name.clone()) {
                return None; // already visited on this path: cycle guard.
            }
            let via_fields = structs.get(name.as_str()).and_then(|s| {
                s.component_types
                    .iter()
                    .find_map(|(t, _)| type_contains_capability(t, structs, seen))
            });
            let found = via_fields.or_else(|| {
                targs.iter().find_map(|a| match a {
                    TypeArg::Type(t) => type_contains_capability(t, structs, seen),
                    _ => None,
                })
            });
            seen.remove(name);
            found
        }
        _ => None,
    }
}

fn contains_capability(ty: &Type, structs: &BTreeMap<String, &DeclStruct>) -> Option<String> {
    type_contains_capability(ty, structs, &mut BTreeSet::new())
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
fn validate_fn_capability_types(
    struct_name: Option<&str>,
    fn_name: &str,
    span: Span,
    params: &[DeclParam],
    ret: &Type,
    owner: CapOwner,
    is_pub_method: bool,
    structs: &BTreeMap<String, &DeclStruct>,
) -> Result<(), SemaError> {
    let where_ = match struct_name {
        Some(s) => format!("{s}.{fn_name}"),
        None => fn_name.to_string(),
    };
    for p in params {
        validate_capability_args(&p.ty, span, structs)?;
        let Some(found) = contains_capability(&p.ty, structs) else {
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
    let Some(found) = contains_capability(ret, structs) else {
        return Ok(());
    };
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
                if let Some(found) = contains_capability(&d.ty, &structs) {
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
                    if let Some(found) = contains_capability(ty, &structs) {
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
    let mut structs: BTreeMap<String, &DeclStruct> = BTreeMap::new();
    for item in items {
        if let DeclItem::Struct(s) = item {
            structs.insert(s.name.clone(), s);
        }
    }
    let mut roots = BTreeSet::new();
    let mut capability_bearing = BTreeSet::new();
    for item in items {
        match item {
            DeclItem::Struct(s) => {
                if contains_capability(&Type::Named(s.name.clone(), Vec::new()), &structs).is_some()
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
                if contains_capability(&Type::Named(e.name.clone(), Vec::new()), &structs).is_some()
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
// ...)` is the one exact-bytes mechanism, with three kinds: `dma`
// (device-visible memory, checked against the target ABI), `mmio`
// (register maps, §2), and `wire` (persistent/network bytes — exact
// encoding independent of any target, no capabilities or target-dependent
// fields inside). For every `@layout` type the compiler reports exact
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

/// 03-hardware.md §3's three layout kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    Dma,
    Mmio,
    Wire,
}

impl LayoutKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LayoutKind::Dma => "dma",
            LayoutKind::Mmio => "mmio",
            LayoutKind::Wire => "wire",
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutType {
    pub name: String,
    pub kind: LayoutKind,
    pub endian: LayoutEndian,
    /// Total bytes: the end of the last field. There is no trailing
    /// padding and no alignment round-up — a `@layout` type is exactly
    /// the bytes its fields cover.
    pub size: u64,
    /// Total declared-hole bytes (the sum of every `Padding` entry).
    pub padding: u64,
    pub entries: Vec<LayoutEntry>,
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

/// 03-hardware.md §1's capability type list, by name. These are the types
/// plans/M7.md item A mints; 03 §3 forbids them inside a `wire` layout
/// explicitly, and no capability has a byte encoding on any target, so
/// they are rejected inside every kind — with the `wire` case saying so in
/// the doc's own words.
const CAPABILITY_TYPES: &[&str] = &["DeviceCap", "Mmio", "IrqCap", "DmaPool"];

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
                             `dma`, `mmio`, or `wire` (03-hardware.md §3)"
                        ),
                        arg.span,
                    ));
                };
                kind = Some(match name.as_str() {
                    "dma" => LayoutKind::Dma,
                    "mmio" => LayoutKind::Mmio,
                    "wire" => LayoutKind::Wire,
                    other => {
                        return Err(layout_error(
                            format!(
                                "unknown `@layout` kind `{other}` on struct `{struct_name}`; the \
                                 three kinds are `dma`, `mmio`, and `wire` (03-hardware.md §3)"
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
                 `dma`, `mmio`, or `wire` (03-hardware.md §3)"
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

/// A `@layout` field's exact byte size, or the named rejection that says
/// why it has none. `layout_names` is every `@layout` struct declared in
/// this module, so a nested one is rejected as the scope limit it is
/// rather than as an unsized type it is not.
fn layout_field_size(
    struct_name: &str,
    field_name: &str,
    kind: LayoutKind,
    ty: &ast::Type,
    layout_names: &BTreeSet<String>,
    span: Span,
) -> Result<u64, SemaError> {
    let rendered = printer::print_type_bare(ty);
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
            return Ok(size);
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
        if layout_names.contains(&n.name) {
            return Err(layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` nests a `@layout` type; a \
                     nested `@layout` field is not implemented (plans/M7.md item E owns the \
                     composite queue/DMA layouts that need it)"
                ),
                span,
            ));
        }
    }
    if CAPABILITY_TYPES.contains(&n.name.as_str()) {
        return Err(match kind {
            LayoutKind::Wire => layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` is a capability type \
                     (03-hardware.md §1); a `wire` layout is exact bytes independent of any \
                     target and can hold no capability (03-hardware.md §3)"
                ),
                span,
            ),
            LayoutKind::Dma | LayoutKind::Mmio => layout_error(
                format!(
                    "field `{struct_name}.{field_name}: {rendered}` is a capability type \
                     (03-hardware.md §1); no capability has a byte encoding, so a `@layout` \
                     type cannot hold one (03-hardware.md §3)"
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
            Some(size) => Ok(size),
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

fn no_exact_size_error(
    struct_name: &str,
    field_name: &str,
    rendered: &str,
    kind: LayoutKind,
    span: Span,
) -> SemaError {
    let wrappers = match kind {
        LayoutKind::Mmio => ", optionally wrapped in `ReadOnly`/`WriteOnly`",
        LayoutKind::Dma | LayoutKind::Wire => "",
    };
    layout_error(
        format!(
            "field `{struct_name}.{field_name}: {rendered}` has no exact byte size; a `@layout` \
             field is a sized integer (`u8`/`u16`/`u32`/`u64`, or their signed forms){wrappers} \
             (03-hardware.md §3)"
        ),
        span,
    )
}

/// Lays out one `@layout` struct, checking every rule as it goes.
fn check_one_layout(
    s: &StructItem,
    attr: &Attr,
    layout_names: &BTreeSet<String>,
) -> Result<LayoutType, SemaError> {
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
    let mut entries: Vec<LayoutEntry> = Vec::new();
    let mut cursor: u64 = 0;
    let mut padding: u64 = 0;
    let mut last_field: Option<(String, u64, u64)> = None;
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
        let size = layout_field_size(&name, &f.name, kind, &f.ty, layout_names, f.span)?;
        let mut explicit: Option<u64> = None;
        for a in &f.attrs {
            if a.name == "offset" {
                if explicit.is_some() {
                    return Err(layout_error(
                        format!("field `{name}.{}` carries more than one `@offset`", f.name),
                        a.span,
                    ));
                }
                explicit = Some(parse_offset_attr(&name, &f.name, a)?);
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
        let offset = explicit.unwrap_or(cursor);
        if offset < cursor {
            let (prev_name, prev_start, prev_end) = last_field
                .clone()
                .unwrap_or_else(|| (String::from("<start>"), cursor, cursor));
            // Two distinct violations share this one condition, and the
            // diagnostic must not claim the wrong one: a field declared
            // after `prev` may sit entirely *before* it (an ordering
            // violation with no byte in common) or genuinely share bytes
            // with it. Saying "overlaps" for the first case asserts a
            // fact that is false — `earlier` at 0x0..0x4 does not touch
            // `later` at 0x10..0x14 — and a reader who checks it loses
            // trust in the rest of the message.
            let overlaps = offset + size > prev_start;
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
        if offset % size != 0 {
            return Err(match explicit {
                Some(n) => layout_error(
                    format!(
                        "field `{name}.{}: {}` at `@offset({n:#x})` is not {size}-byte aligned \
                         (03-hardware.md §2)",
                        f.name,
                        printer::print_type_bare(&f.ty)
                    ),
                    f.span,
                ),
                None => implicit_padding_error(&name, &f.name, &f.ty, offset, size, f.span),
            });
        }
        if offset > cursor {
            let gap = offset - cursor;
            entries.push(LayoutEntry::Padding {
                offset: cursor,
                size: gap,
            });
            padding += gap;
        }
        entries.push(LayoutEntry::Field(LayoutField {
            name: f.name.clone(),
            ty: printer::print_type_bare(&f.ty),
            offset,
            size,
        }));
        cursor = offset + size;
        last_field = Some((f.name.clone(), offset, cursor));
    }
    if last_field.is_none() {
        return Err(layout_error(
            format!(
                "`@layout` struct `{name}` declares no fields; a `@layout` type is an exact byte \
                 layout and has no empty form (03-hardware.md §3)"
            ),
            s.span,
        ));
    }
    Ok(LayoutType {
        name,
        kind,
        endian,
        size: cursor,
        padding,
        entries,
    })
}

/// The implicit-padding rejection (plans/M7.md decision 4: "no implicit
/// padding"). It fires exactly when a field with no `@offset` would land
/// at a natural offset its own width does not divide — the one place a
/// conventional compiler inserts padding silently. This one refuses and
/// says how many bytes it would have had to invent.
fn implicit_padding_error(
    struct_name: &str,
    field_name: &str,
    ty: &ast::Type,
    offset: u64,
    size: u64,
    span: Span,
) -> SemaError {
    let needed = size - (offset % size);
    layout_error(
        format!(
            "field `{struct_name}.{field_name}: {}` follows the previous field at offset \
             {offset:#x} and would need {needed} byte(s) of implicit padding to be {size}-byte \
             aligned; a `@layout` type never pads implicitly — give `{field_name}` an explicit \
             `@offset(...)` (03-hardware.md §3)",
            printer::print_type_bare(ty)
        ),
        span,
    )
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
pub fn check_layouts(module: &Module) -> Result<Vec<LayoutType>, SemaError> {
    let mut layout_names = BTreeSet::new();
    for item in &module.items {
        if let Item::Struct(s) = item {
            if s.attrs.iter().any(|a| a.name == "layout") {
                layout_names.insert(s.name.clone());
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
            [attr] => out.push(check_one_layout(s, attr, &layout_names)?),
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
pub fn push_layout_lines(out: &mut String, depth: usize, l: &LayoutType) {
    push_line(
        out,
        depth,
        &format!(
            "Layout name={} kind={} endian={} size={} padding={}",
            l.name,
            l.kind.as_str(),
            l.endian.as_str(),
            l.size,
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
pub fn dump_layouts(by_module: &[(String, Vec<LayoutType>)]) -> String {
    let mut out = String::from("LayoutTypes v0\n");
    for (path, layouts) in by_module {
        if layouts.is_empty() {
            continue;
        }
        push_line(&mut out, 1, &format!("Module path={path}"));
        for l in layouts {
            push_layout_lines(&mut out, 2, l);
        }
    }
    out
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
        is_driver: s.attrs.iter().any(|a| a.name == "driver"),
        layout_kind: declared_layout_kind(&s.attrs),
        component_types,
        span: s.span,
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
    } else if crate::eval::image_checks::is_capability_type_name(name) {
        // 03-hardware.md §1, its own first words: hardware operations
        // require "unforgeable **resource** values". A capability is a
        // resource by fiat, exactly like `@actor`/`@driver`/`resource
        // struct` above — it is never copied, and a struct that holds one
        // is a resource too (which is how `@actor`'s own containment rule
        // and the provenance walk both see through a plain wrapper
        // struct). Recorded here rather than left to the builtin
        // fall-through below, whose whole premise ("every one of these is
        // plain data") is exactly what stops being true for these four.
        in_progress.remove(name);
        memo.insert(name.to_string(), Classification::Resource);
        return Ok(Classification::Resource);
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
        assert_eq!(l.size, 0x68);
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
        let text = dump_layouts(&[("t".to_string(), layouts)]);
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
        assert_eq!(dump_layouts(&[("t".to_string(), none)]), "LayoutTypes v0\n");
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
                "no capability has a byte encoding",
            ),
            (
                "module t\n\n@layout(mmio, endian=little)\nstruct S:\n    r: ReadOnly[bool]\n",
                "wraps `bool`, which is not a sized integer register",
            ),
            (
                "module t\n\n@layout(mmio, endian=little)\nstruct S:\n    r: WriteOnly[u32, u32]\n",
                "must wrap exactly one register type",
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
         struct Blk:\n\
         \x20   id: u32\n\n";

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
                "fn f(read c: DmaPool[Blk]) -> u32:\n    return 0\n",
                "`DmaPool` expects 2 generic argument(s), found 1",
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
                "const C: DmaPool[Blk, 4096] = 0\n",
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
}
