//! Structural generic instantiation (plans/M2.md item H): typing a
//! `Generic[Args]` use enqueues the concrete instantiation (keyed in a
//! `BTreeMap` by a canonical `"<kind>:<name>[<args>]"` string — decision
//! 1's "BTreeSet-keyed by name + resolved args", realized this way so
//! `Type`/`Expr` never need an `Ord` impl of their own; see
//! `bodies.rs`'s `InstKind`/`QueuedInstantiation`/`ModuleCtx::generics_queue`
//! doc comments for the queue itself and the "instantiated at" chain
//! mechanism). Each unique instantiation is checked exactly once: this
//! file builds a substituted copy of the declaration (`Subst`,
//! `subst_*` below — decision 4's "clone freely", applied to `Type`
//! trees instead of source text) and feeds it back through the *same*
//! per-declaration check functions `bodies.rs`/`access.rs`/`matches.rs`
//! already run for a non-generic declaration (each widened `pub(crate)`
//! for exactly this reuse — decision 10's minimal-footprint rule, no
//! restructuring).
//!
//! A missing requirement discovered while checking an instantiation
//! reports the multi-line chain diagnostic pinned verbatim in
//! 02-language.md §7.3 (decision 2) — see `finalize_diagnostic` below for
//! the exact, deliberately narrow shape this recognizes (documented
//! there, and in the session report).
//!
//! Const arguments (02-language.md §6.3, decision 4) evaluate only as a
//! literal, a `const` name (recursively, to another literal/`const`/
//! variant path), or a fieldless-enum variant path — `eval_const_expr`
//! below; anything else fails closed via the existing `unimplemented_at`
//! helper.
//!
//! Scope boundary (documented, not silently approximated): a *method*'s
//! own generic parameters (beyond its struct's, if any) are never
//! instantiated — only top-level generic `fn`s, and generic
//! `struct`/`enum` types, are. A call that would need one fails closed
//! with `unimplemented_at("generic instantiation is", ...)`, exactly as
//! every fail-closed point already did before this item landed.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::bodies::{self, FnInfo, InstKind, ModuleCtx, QueuedInstantiation, StructInfo};
use crate::sema::typed::TypedInstantiation;
use crate::sema::types::{
    self, Classification, DeclEnum, DeclField, DeclFn, DeclGenericKind, DeclGenericParam,
    DeclMember, DeclParam, DeclStruct, DeclVariant, DeclVariantPayload, Type, TypeArg,
};
use crate::sema::{SemaError, access, flow, matches, unimplemented_at};
use crate::syntax::ast::{Arg, BinOp, Expr, Module, Span, Stmt};
use crate::syntax::printer;

// --- canonical keys ---------------------------------------------------

/// `"<kind>:<name>[<args>]"`, span-insensitive (`types::render_type_arg`
/// already ignores spans for the shapes M2 supports — same reasoning as
/// `bodies::types_eq`'s own `same_len_expr`). This is both the
/// `BTreeMap` key `bodies::enqueue_instantiation` uses and the display
/// spelling the chain diagnostic cites (`` `hash_pair[Sector]` ``).
pub(crate) fn canonical_key(kind: InstKind, name: &str, args: &[TypeArg]) -> String {
    format!("{}:{}", kind.tag(), display_name(name, args))
}

fn display_name(name: &str, args: &[TypeArg]) -> String {
    if args.is_empty() {
        return name.to_string();
    }
    let rendered: Vec<String> = args.iter().map(types::render_type_arg).collect();
    format!("{name}[{}]", rendered.join(", "))
}

// --- substitution: Type::Generic(name) -> concrete Type ----------------

#[derive(Debug, Clone, Default)]
struct Subst {
    types: BTreeMap<String, Type>,
    consts: BTreeMap<String, Expr>,
}

fn subst_type(ty: &Type, subst: &Subst) -> Type {
    match ty {
        Type::Generic(name) => subst.types.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Type::Array(elem, len) => Type::Array(
            Box::new(subst_type(elem, subst)),
            Box::new(subst_expr(len, subst)),
        ),
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(|t| subst_type(t, subst)).collect()),
        Type::Option(inner) => Type::Option(Box::new(subst_type(inner, subst))),
        Type::Result(ok, err) => Type::Result(
            Box::new(subst_type(ok, subst)),
            Box::new(subst_type(err, subst)),
        ),
        Type::Own(pool, inner) => Type::Own(pool.clone(), Box::new(subst_type(inner, subst))),
        Type::Static(inner) => Type::Static(Box::new(subst_type(inner, subst))),
        Type::Bytes(Some(len)) => Type::Bytes(Some(Box::new(subst_expr(len, subst)))),
        Type::Bytes(None) => Type::Bytes(None),
        Type::Fn(params, ret) => Type::Fn(
            params
                .iter()
                .map(|(m, t)| (*m, subst_type(t, subst)))
                .collect(),
            Box::new(subst_type(ret, subst)),
        ),
        Type::Named(name, targs) => Type::Named(
            name.clone(),
            targs.iter().map(|a| subst_type_arg(a, subst)).collect(),
        ),
        other => other.clone(),
    }
}

fn subst_type_arg(arg: &TypeArg, subst: &Subst) -> TypeArg {
    match arg {
        TypeArg::Type(t) => TypeArg::Type(subst_type(t, subst)),
        TypeArg::Const(e) => TypeArg::Const(subst_expr(e, subst)),
        TypeArg::Bound(e) => TypeArg::Bound(subst_expr(e, subst)),
    }
}

/// Only ever rewrites a bare `Expr::Name` naming a const generic
/// parameter — the *only* shape a length/const expression takes inside a
/// generic declaration that this item resolves (mirrors
/// `bodies::types_eq`'s own `same_len_expr` scope: M2 does not evaluate
/// arbitrary expressions, only compare these two shapes).
fn subst_expr(e: &Expr, subst: &Subst) -> Expr {
    if let Expr::Name(_, name) = e {
        if let Some(v) = subst.consts.get(name) {
            return v.clone();
        }
    }
    e.clone()
}

/// `pub(crate)` reuse for `flow.rs`'s own best-effort place typing
/// (decision 10's minimal-footprint rule, the same pattern the rest of
/// this file's `pub(crate)` surface follows): substitutes a single
/// unsubstituted field type using a struct's own generics zipped against
/// a use site's concrete type arguments — a type-only `Subst` (no consts;
/// a field's own type never needs one, only an array/`Bytes` *length*
/// might, and `place_type`'s callers only ever ask "is this field's type
/// a resource", never its size) so a resource classification through an
/// instantiated generic struct's field (`Box[DmaBlock].item`, not just a
/// bare generic parameter) is answered correctly instead of silently
/// falling back to "unknown" the moment a use site's type arguments are
/// non-empty.
pub(crate) fn subst_field_type(
    field_ty: &Type,
    generics: &[DeclGenericParam],
    targs: &[TypeArg],
) -> Type {
    let mut types = BTreeMap::new();
    for (g, a) in generics.iter().zip(targs.iter()) {
        if let TypeArg::Type(t) = a {
            types.insert(g.name.clone(), t.clone());
        }
    }
    let subst = Subst {
        types,
        consts: BTreeMap::new(),
    };
    subst_type(field_ty, &subst)
}

// --- const argument evaluation (decision 4) -----------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConstVal {
    Int(i128),
    Bool(bool),
    /// Raw lexed char text (the lexer's own escape-validated spelling) —
    /// M2 never needs the decoded codepoint, only to carry the value
    /// through substitution/rendering unchanged.
    Char(String),
    Variant(String, String),
}

impl ConstVal {
    fn to_expr(&self, span: Span) -> Expr {
        match self {
            ConstVal::Int(n) => Expr::Int(span, n.to_string()),
            ConstVal::Bool(b) => Expr::Bool(span, *b),
            ConstVal::Char(text) => Expr::Char(span, text.clone()),
            ConstVal::Variant(enum_name, variant) => Expr::Field(
                Box::new(Expr::Name(span, enum_name.clone())),
                span,
                variant.clone(),
            ),
        }
    }
}

/// A bare `const` reference is resolved by looking its own initializer
/// back up in `mctx.const_values` and evaluating *that* — bounded so a
/// `const` cycle (which nothing before item H rejects, since M2 has no
/// comptime evaluator to notice one) cannot loop forever; hitting the
/// cap just falls to the same fail-closed outcome as any other
/// unsupported shape.
const MAX_CONST_LOOKUP_DEPTH: u32 = 8;

fn eval_const_expr(e: &Expr, mctx: &ModuleCtx, depth: u32) -> Result<ConstVal, SemaError> {
    match e {
        Expr::Int(span, text) => bodies::parse_int_literal(text)
            .map(ConstVal::Int)
            .ok_or_else(|| SemaError::at("type", "invalid integer literal".to_string(), *span)),
        Expr::Bool(_, b) => Ok(ConstVal::Bool(*b)),
        Expr::Char(_, text) => Ok(ConstVal::Char(text.clone())),
        Expr::Name(span, name) => {
            if depth >= MAX_CONST_LOOKUP_DEPTH {
                return Err(unimplemented_at("this const generic argument is", *span));
            }
            let Some(value) = mctx.const_values.get(name) else {
                return Err(unimplemented_at("this const generic argument is", *span));
            };
            eval_const_expr(value, mctx, depth + 1)
        }
        Expr::Field(base, span, variant) => {
            let Expr::Name(_, enum_name) = base.as_ref() else {
                return Err(unimplemented_at("this const generic argument is", *span));
            };
            let Some(en) = mctx.enums.get(enum_name) else {
                return Err(unimplemented_at("this const generic argument is", *span));
            };
            let Some(dv) = en.variants.iter().find(|v| &v.name == variant) else {
                return Err(unimplemented_at("this const generic argument is", *span));
            };
            if !matches!(dv.payload, DeclVariantPayload::None) {
                return Err(unimplemented_at("this const generic argument is", *span));
            }
            Ok(ConstVal::Variant(enum_name.clone(), variant.clone()))
        }
        other => Err(unimplemented_at(
            "this const generic argument is",
            other.span(),
        )),
    }
}

// --- resolving raw call-site brackets into `TypeArg`s -------------------

/// `Name[Args](...)`'s `Args` are raw `Expr`s (the parser cannot tell
/// indexing from a generic bracket — `ast::Expr::Index`'s own doc
/// comment) — this is the call-site mirror of `types::resolve_type_arg`
/// (which runs on `ast::GenericArg`, the *annotation*-position grammar).
/// A bare name is a scalar/struct/enum type if it names one, else a
/// const-shaped expression (evaluated later, lazily, by `eval_const_expr`
/// — resolving it eagerly here would reject a legal `const` name before
/// even trying, since a name alone can't be told apart from a type name
/// syntactically past this point either). No nested generic arguments
/// (`Ring[Foo[Bar], 4]`) — that call shape fails closed at its own
/// `unimplemented_at` in `resolve_call_type_arg`'s fallthrough.
pub(crate) fn resolve_call_targs(
    targs: &[Expr],
    mctx: &ModuleCtx,
) -> Result<Vec<TypeArg>, SemaError> {
    targs
        .iter()
        .map(|e| resolve_call_type_arg(e, mctx))
        .collect()
}

fn resolve_call_type_arg(e: &Expr, mctx: &ModuleCtx) -> Result<TypeArg, SemaError> {
    match e {
        Expr::Name(_, name) => {
            if let Some(t) = bodies::scalar_type_by_name(name) {
                return Ok(TypeArg::Type(t));
            }
            if mctx.structs.contains_key(name) || mctx.enums.contains_key(name) {
                return Ok(TypeArg::Type(Type::Named(name.clone(), vec![])));
            }
            // Not a known type name: a const-shaped argument (a `const`
            // item's own name, most commonly) — left unevaluated here,
            // `eval_const_expr` resolves it when the argument actually
            // binds to a const parameter.
            Ok(TypeArg::Const(e.clone()))
        }
        Expr::Int(..) | Expr::Bool(..) | Expr::Char(..) | Expr::Field(..) => {
            Ok(TypeArg::Const(e.clone()))
        }
        other => Err(unimplemented_at(
            "this generic argument shape is",
            other.span(),
        )),
    }
}

// --- building a `Subst` from a declaration's generics + resolved args ---

fn check_arity(
    generics: &[DeclGenericParam],
    args: &[TypeArg],
    name: &str,
    call_span: Span,
) -> Result<(), SemaError> {
    if generics.len() != args.len() {
        return Err(SemaError::at(
            "type",
            format!(
                "`{name}` expects {} generic argument(s), found {}",
                generics.len(),
                args.len()
            ),
            call_span,
        ));
    }
    Ok(())
}

fn build_subst(
    generics: &[DeclGenericParam],
    args: &[TypeArg],
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Subst, SemaError> {
    let mut subst = Subst::default();
    for (g, a) in generics.iter().zip(args.iter()) {
        match (&g.kind, a) {
            (DeclGenericKind::Type, TypeArg::Type(t)) => {
                subst.types.insert(g.name.clone(), t.clone());
            }
            (DeclGenericKind::Const(_), TypeArg::Const(e)) => {
                let v = eval_const_expr(e, mctx, 0)?;
                subst.consts.insert(g.name.clone(), v.to_expr(e.span()));
            }
            (DeclGenericKind::Type, _) => {
                return Err(SemaError::at(
                    "type",
                    format!("generic parameter `{}` requires a type argument", g.name),
                    call_span,
                ));
            }
            (DeclGenericKind::Const(_), _) => {
                return Err(SemaError::at(
                    "type",
                    format!("generic parameter `{}` requires a const argument", g.name),
                    call_span,
                ));
            }
        }
    }
    Ok(subst)
}

// --- substituting a declaration's members --------------------------------

fn subst_decl_field(f: &DeclField, subst: &Subst) -> DeclField {
    DeclField {
        name: f.name.clone(),
        ty: subst_type(&f.ty, subst),
    }
}

fn subst_decl_param(p: &DeclParam, subst: &Subst) -> DeclParam {
    DeclParam {
        mode: p.mode,
        name: p.name.clone(),
        ty: subst_type(&p.ty, subst),
    }
}

/// Substitutes a *member* fn/method/init's signature using the
/// *enclosing struct's* substitution: its own generics (if it happens to
/// have any, beyond the struct's — a "generic method", item H's own
/// documented scope boundary) are left exactly as declared, since they
/// are never themselves instantiated.
fn subst_decl_fn_member(f: &DeclFn, subst: &Subst) -> DeclFn {
    DeclFn {
        name: f.name.clone(),
        is_async: f.is_async,
        generics: f.generics.clone(),
        receiver: f.receiver.clone(),
        params: f
            .params
            .iter()
            .map(|p| subst_decl_param(p, subst))
            .collect(),
        ret: subst_type(&f.ret, subst),
    }
}

/// Substitutes a *directly instantiated* fn's own signature, clearing its
/// generics list — this is the one place a `DeclFn` actually stops being
/// generic (`bodies::check_top_fn`'s guard is `f.generics.is_empty()` on
/// the *ast*, but every check function downstream reads types off `d`
/// alone, so clearing this is what actually makes the substituted
/// declaration behave like a real concrete one).
fn subst_decl_fn_direct(f: &DeclFn, subst: &Subst) -> DeclFn {
    DeclFn {
        name: f.name.clone(),
        is_async: f.is_async,
        generics: Vec::new(),
        receiver: f.receiver.clone(),
        params: f
            .params
            .iter()
            .map(|p| subst_decl_param(p, subst))
            .collect(),
        ret: subst_type(&f.ret, subst),
    }
}

fn subst_decl_member(m: &DeclMember, subst: &Subst) -> DeclMember {
    match m {
        DeclMember::Field(f) => DeclMember::Field(subst_decl_field(f, subst)),
        DeclMember::Fn(f) => DeclMember::Fn(subst_decl_fn_member(f, subst)),
        DeclMember::Init(f) => DeclMember::Init(subst_decl_fn_member(f, subst)),
        DeclMember::Pool(p) => DeclMember::Pool(p.clone()),
    }
}

/// Reclassifies a substituted struct/enum (decision 4's "a struct
/// generic over T containing a T is a resource iff the argument is"):
/// `bodies::is_resource_type` mirrors `types::classify_type` exactly and
/// already resolves a `Type::Named` component through `mctx`'s
/// (unsubstituted, but structurally identical for this purpose — a
/// *named* component's own classification never depends on the
/// *outer* instantiation's args) struct/enum tables.
fn reclassify(
    is_resource_fiat: bool,
    component_types: &[(Type, Span)],
    mctx: &ModuleCtx,
) -> Classification {
    let resource = is_resource_fiat
        || component_types
            .iter()
            .any(|(t, _)| bodies::is_resource_type(t, mctx));
    if resource {
        Classification::Resource
    } else {
        Classification::Data
    }
}

fn subst_decl_struct(d: &DeclStruct, subst: &Subst, mctx: &ModuleCtx) -> DeclStruct {
    let members: Vec<DeclMember> = d
        .members
        .iter()
        .map(|m| subst_decl_member(m, subst))
        .collect();
    let component_types: Vec<(Type, Span)> = d
        .component_types
        .iter()
        .map(|(t, sp)| (subst_type(t, subst), *sp))
        .collect();
    let classification = reclassify(d.is_resource_fiat, &component_types, mctx);
    DeclStruct {
        name: d.name.clone(),
        generics: Vec::new(),
        deriving: d.deriving.clone(),
        classification,
        members,
        is_resource_fiat: d.is_resource_fiat,
        component_types,
        span: d.span,
    }
}

fn subst_variant_payload(p: &DeclVariantPayload, subst: &Subst) -> DeclVariantPayload {
    match p {
        DeclVariantPayload::None => DeclVariantPayload::None,
        DeclVariantPayload::Tuple(ts) => {
            DeclVariantPayload::Tuple(ts.iter().map(|t| subst_type(t, subst)).collect())
        }
        DeclVariantPayload::Named(fs) => DeclVariantPayload::Named(
            fs.iter()
                .map(|(n, t)| (n.clone(), subst_type(t, subst)))
                .collect(),
        ),
    }
}

fn subst_decl_enum(d: &DeclEnum, subst: &Subst, mctx: &ModuleCtx) -> DeclEnum {
    let variants: Vec<DeclVariant> = d
        .variants
        .iter()
        .map(|v| DeclVariant {
            name: v.name.clone(),
            payload: subst_variant_payload(&v.payload, subst),
        })
        .collect();
    let component_types: Vec<(Type, Span)> = d
        .component_types
        .iter()
        .map(|(t, sp)| (subst_type(t, subst), *sp))
        .collect();
    let classification = reclassify(false, &component_types, mctx);
    DeclEnum {
        name: d.name.clone(),
        generics: Vec::new(),
        deriving: d.deriving.clone(),
        classification,
        variants,
        component_types,
        span: d.span,
    }
}

// --- instantiation entry points: build + enqueue -------------------------

/// Resolves `name[args]` as a struct instantiation: looks up the
/// *declared* shape, substitutes it, and enqueues it (item H: "typing a
/// `Generic[Args]` use enqueues the concrete instantiation") — every
/// caller (a construction call, a field/method/variant lookup through an
/// already-typed `Type::Named` value, an annotation) routes through this
/// one function, so every use registers exactly once per unique
/// `(name, args)` (the queue's own dedup).
pub(crate) fn instantiate_struct(
    mctx: &ModuleCtx,
    name: &str,
    args: &[TypeArg],
    call_span: Span,
) -> Result<StructInfo, SemaError> {
    let Some(orig) = mctx.structs.get(name) else {
        return Err(SemaError::at(
            "type",
            format!("unknown type `{name}`"),
            call_span,
        ));
    };
    check_arity(&orig.decl.generics, args, name, call_span)?;
    let subst = build_subst(&orig.decl.generics, args, mctx, call_span)?;
    let decl = subst_decl_struct(&orig.decl, &subst, mctx);
    bodies::enqueue_instantiation(mctx, InstKind::Struct, name, args, call_span)?;
    Ok(StructInfo {
        decl,
        ast_members: orig.ast_members.clone(),
    })
}

pub(crate) fn instantiate_enum(
    mctx: &ModuleCtx,
    name: &str,
    args: &[TypeArg],
    call_span: Span,
) -> Result<DeclEnum, SemaError> {
    let Some(orig) = mctx.enums.get(name) else {
        return Err(SemaError::at(
            "type",
            format!("unknown type `{name}`"),
            call_span,
        ));
    };
    check_arity(&orig.generics, args, name, call_span)?;
    let subst = build_subst(&orig.generics, args, mctx, call_span)?;
    let decl = subst_decl_enum(orig, &subst, mctx);
    bodies::enqueue_instantiation(mctx, InstKind::Enum, name, args, call_span)?;
    Ok(decl)
}

pub(crate) fn instantiate_fn(
    mctx: &ModuleCtx,
    name: &str,
    args: &[TypeArg],
    call_span: Span,
) -> Result<FnInfo, SemaError> {
    let Some(orig) = mctx.fns.get(name) else {
        return Err(SemaError::at(
            "type",
            format!("unknown function `{name}`"),
            call_span,
        ));
    };
    check_arity(&orig.decl.generics, args, name, call_span)?;
    let subst = build_subst(&orig.decl.generics, args, mctx, call_span)?;
    let decl = subst_decl_fn_direct(&orig.decl, &subst);
    let mut ast = orig.ast.clone();
    ast.generics = Vec::new();
    bodies::enqueue_instantiation(mctx, InstKind::Fn, name, args, call_span)?;
    Ok(FnInfo { ast, decl })
}

// --- item 2: inferring a generic fn's type arguments ---------------------

/// The dumbest honest inference (item 2): a type parameter used directly
/// as a parameter's own type (`a: T`) is inferred from that argument's
/// synthesized type; a parameter whose declared type is anything other
/// than a bare `Type::Generic` (nested in an array/tuple/`Option`/...) is
/// not consulted at all — a genuinely deeper case than "direct
/// `T`-as-parameter-type", so it simply contributes nothing towards
/// inferring that parameter (decision: "if easy" nested inference is not
/// attempted). A const generic parameter is never inferred (there is no
/// comptime engine yet to read a value back out of an argument
/// expression) — if the fn declares one, inference always reports it
/// uninferable. Mismatched or never-constrained type parameters are
/// named in the error, exactly as item 2 asks.
pub(crate) fn infer_fn_targs(
    fi: &FnInfo,
    args: &[Arg],
    fctx: &mut bodies::FnCtx,
    mctx: &ModuleCtx,
    call_span: Span,
) -> Result<Vec<TypeArg>, SemaError> {
    let bound = bind_args_positionally(&fi.decl.params, args);
    let mut inferred: BTreeMap<String, Type> = BTreeMap::new();
    for (i, p) in fi.decl.params.iter().enumerate() {
        let Type::Generic(gname) = &p.ty else {
            continue;
        };
        let Some(arg_expr) = bound[i] else {
            continue; // a default-valued, unbound parameter: nothing to infer from.
        };
        let synthesized = bodies::check_expr(arg_expr, None, fctx, mctx)?.ty;
        if let Some(existing) = inferred.get(gname) {
            if !bodies::types_eq(existing, &synthesized) {
                return Err(SemaError::at(
                    "generic",
                    format!(
                        "`{}` requires explicit `[Args]`: parameter `{gname}` is both `{}` and `{}`",
                        fi.decl.name,
                        types::render_type(existing),
                        types::render_type(&synthesized)
                    ),
                    call_span,
                ));
            }
        } else {
            inferred.insert(gname.clone(), synthesized);
        }
    }
    let mut out = Vec::with_capacity(fi.decl.generics.len());
    for g in &fi.decl.generics {
        match &g.kind {
            DeclGenericKind::Type => match inferred.get(&g.name) {
                Some(t) => out.push(TypeArg::Type(t.clone())),
                None => {
                    return Err(SemaError::at(
                        "generic",
                        format!(
                            "`{}` requires explicit `[Args]`: parameter `{}` cannot be inferred",
                            fi.decl.name, g.name
                        ),
                        call_span,
                    ));
                }
            },
            DeclGenericKind::Const(_) => {
                return Err(SemaError::at(
                    "generic",
                    format!(
                        "`{}` requires explicit `[Args]`: const parameter `{}` cannot be inferred",
                        fi.decl.name, g.name
                    ),
                    call_span,
                ));
            }
        }
    }
    Ok(out)
}

/// Binds `args` to `decl_params` (label or positional cursor, mirroring
/// `bodies::check_call_args`'s own binding order) far enough to hand
/// `infer_fn_targs` each bound parameter's argument *expression* — arity/
/// label validity itself is re-checked properly afterwards by the real
/// `check_call_args` call `bodies.rs` makes once the instantiation is
/// resolved, so this binder is deliberately permissive (out-of-range or
/// duplicate binds are simply ignored here rather than reported twice).
fn bind_args_positionally<'a>(decl_params: &[DeclParam], args: &'a [Arg]) -> Vec<Option<&'a Expr>> {
    let mut bound: Vec<Option<&'a Expr>> = vec![None; decl_params.len()];
    let mut cursor = 0usize;
    for a in args {
        let idx = match &a.label {
            Some(lbl) => decl_params.iter().position(|p| &p.name == lbl),
            None => {
                while cursor < decl_params.len() && bound[cursor].is_some() {
                    cursor += 1;
                }
                let i = cursor;
                cursor += 1;
                Some(i).filter(|&i| i < decl_params.len())
            }
        };
        if let Some(idx) = idx {
            bound[idx] = Some(&a.value);
        }
        // An unresolvable label/arity mismatch is left for the real
        // `check_call_args` to report properly; inference just skips it.
    }
    bound
}

// --- draining the queue: the actual per-instantiation checking ----------

/// Item H's own pass, run last (`mod.rs::check`): drains
/// `mctx.generics_queue` to a fixed point — checking one instantiation
/// can enqueue more (a nested generic use inside its substituted body),
/// so this loops until nothing new appears — running the *same* three
/// passes (`bodies`/`access`/`matches`) `check` already ran, concretely,
/// against each substituted declaration. `path` is cited verbatim in the
/// chain diagnostic (decision 2; see `mod.rs::check`'s own doc comment).
/// plans/M3.md item A: also returns every drained instantiation's typed
/// body (`typed::TypedInstantiation`), keyed by this file's own
/// canonical spelling — the identical string a `Call`/`OpCall` node's
/// `CalleeKey::FnInstance`/`MethodInstance` carries for the same
/// instantiation (`typed.rs`'s own doc comment).
pub(crate) fn check(
    _module: &Module,
    _decl_items: &[types::DeclItem],
    mctx: &ModuleCtx,
    path: &str,
) -> Result<BTreeMap<String, TypedInstantiation>, SemaError> {
    let mut processed: BTreeSet<String> = BTreeSet::new();
    let mut typed_instantiations: BTreeMap<String, TypedInstantiation> = BTreeMap::new();
    loop {
        let next = {
            let q = mctx.generics_queue.borrow();
            q.iter()
                .find(|(k, _)| !processed.contains(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
        };
        let Some((key, entry)) = next else {
            break;
        };
        processed.insert(key.clone());
        *mctx.current_chain.borrow_mut() = entry.chain.clone();
        let result = check_one_instantiation(mctx, &entry);
        *mctx.current_chain.borrow_mut() = Vec::new();
        match result {
            Ok(typed_inst) => {
                typed_instantiations.insert(key, typed_inst);
            }
            Err(e) => return Err(finalize_diagnostic(e, &entry, mctx, path)),
        }
    }
    Ok(typed_instantiations)
}

fn check_one_instantiation(
    mctx: &ModuleCtx,
    entry: &QueuedInstantiation,
) -> Result<TypedInstantiation, SemaError> {
    let call_span = *entry
        .chain
        .last()
        .expect("a queued instantiation's chain always has at least its own triggering call");
    match entry.kind {
        InstKind::Fn => {
            let fi = instantiate_fn(mctx, &entry.name, &entry.args, call_span)?;
            let tf = bodies::check_top_fn(&fi.ast, &fi.decl, mctx)?
                .expect("an instantiated fn is always concrete, never itself generic");
            let empty_effects = access::EffectMap::new();
            access::check_top_fn(&fi.ast, &fi.decl, mctx, &empty_effects)?;
            flow::check_top_fn(&fi.ast, &fi.decl, mctx, &empty_effects)?;
            matches::check_top_fn(&fi.ast, &fi.decl, mctx)?;
            Ok(TypedInstantiation::Fn(tf))
        }
        InstKind::Struct => {
            let si = instantiate_struct(mctx, &entry.name, &entry.args, call_span)?;
            let self_ty = Type::Named(entry.name.clone(), entry.args.clone());
            let ts = bodies::check_struct_members(&si, self_ty.clone(), mctx)?;
            let effects = access::infer_effects_over(mctx);
            access::check_struct_members(&si, self_ty.clone(), mctx, &effects)?;
            flow::check_struct_members(&si, self_ty.clone(), mctx, &effects)?;
            matches::check_struct_members(&si, self_ty, mctx)?;
            Ok(TypedInstantiation::Struct(ts))
        }
        InstKind::Enum => {
            // Enums carry no bodies/methods (02-language.md §7.2):
            // substitution + reclassification (already run by
            // `instantiate_enum`) is the whole of "checking" one.
            instantiate_enum(mctx, &entry.name, &entry.args, call_span)?;
            Ok(TypedInstantiation::Enum)
        }
    }
}

// --- the requirement-chain diagnostic (decision 2) -----------------------
//
// Pinned verbatim (02-language.md §7.3):
//
//   error[generic]: `hash_pair[Sector]` requires `Sector.hash(read self) -> u64`
//     required by `a.hash()` at util/hash.wr:2
//     instantiated at storage/extent.wr:41
//
// This is recognized, not derived through general inference (decision 4
// rules out a constraint solver): a *directly substituted* instantiation
// fn body that fails with exactly the "no method"/"no operator method"
// shape `bodies.rs`'s own `check_call_by_field`/`resolve_operator_method`
// already produce, on a parameter whose *un*substituted type was a bare
// `T`, is recognized as a missing requirement. The required method's
// return type comes from re-scanning the *original* (unsubstituted)
// declaration's own body for the narrow shape 02 §7.3's own example is:
// a `return` whose expression is directly `<param>.<method>()`, or a
// builtin same-type-result binary operator (`+ - * / % & | ^ << >>`)
// applied to two such calls — decision 4's expected-type propagation
// applied by hand to exactly the return-position case the docs pin.
// Anything else recognizably "missing" but outside this shape, or any
// other kind of failure, keeps its ordinary one-line message and still
// gets the `instantiated at` chain appended (item 3): only the "required
// by" line and the location-free primary line are specific to this exact
// shape.

fn finalize_diagnostic(
    e: SemaError,
    entry: &QueuedInstantiation,
    mctx: &ModuleCtx,
    path: &str,
) -> SemaError {
    if e.category == "type" && e.extra_lines.is_empty() {
        if let Some((type_name, method_name)) = e.missing_method.clone() {
            if let Some((call_expr, ret_ty)) =
                find_requirement(mctx, entry, &type_name, &method_name)
            {
                let sig = format!(
                    "{type_name}.{method_name}(read self) -> {}",
                    types::render_type(&ret_ty)
                );
                let display = display_name(&entry.name, &entry.args);
                let mut extra_lines = vec![format!(
                    "  required by `{}` at {path}:{}",
                    printer::print_expr_bare(call_expr),
                    call_expr.span().line
                )];
                for span in entry.chain.iter().rev() {
                    extra_lines.push(format!("  instantiated at {path}:{}", span.line));
                }
                return SemaError {
                    category: "generic",
                    message: format!("`{display}` requires `{sig}`"),
                    line: 0,
                    col: 0,
                    extra_lines,
                    omit_location: true,
                    missing_method: None,
                };
            }
        }
    }
    let mut e = e;
    for span in entry.chain.iter().rev() {
        e.extra_lines
            .push(format!("  instantiated at {path}:{}", span.line));
    }
    e
}

/// Only ever recognizes a top-level generic `fn` (`InstKind::Fn`) — the
/// pinned example's own shape, and item H's documented scope boundary
/// (a generic *method* is never instantiated at all, so there is no
/// "original body" to scan for one; a struct/enum instantiation's own
/// missing-method failures fall back to the ordinary one-line-plus-chain
/// case instead of trying to attribute the failure to one particular
/// method among many).
fn find_requirement<'a>(
    mctx: &'a ModuleCtx,
    entry: &QueuedInstantiation,
    type_name: &str,
    method_name: &str,
) -> Option<(&'a Expr, Type)> {
    if entry.kind != InstKind::Fn {
        return None;
    }
    let fi = mctx.fns.get(&entry.name)?;
    let target_param = fi
        .decl
        .generics
        .iter()
        .zip(entry.args.iter())
        .find_map(|(g, a)| match (&g.kind, a) {
            (DeclGenericKind::Type, TypeArg::Type(Type::Named(n, targs)))
                if n == type_name && targs.is_empty() =>
            {
                Some(g.name.clone())
            }
            _ => None,
        })?;
    let mut param_types = BTreeMap::new();
    for p in &fi.decl.params {
        param_types.insert(p.name.clone(), p.ty.clone());
    }
    let body = fi.ast.body.as_ref()?;
    let (call_expr, found_method) = infer_requirement_call(body, &target_param, &param_types)?;
    if found_method != method_name {
        return None;
    }
    Some((call_expr, fi.decl.ret.clone()))
}

fn infer_requirement_call<'a>(
    body: &'a [Stmt],
    generic_param: &str,
    param_types: &BTreeMap<String, Type>,
) -> Option<(&'a Expr, String)> {
    for stmt in body {
        if let Stmt::Return(_, Some(expr)) = stmt {
            if let Some(found) = scan_return_expr(expr, generic_param, param_types) {
                return Some(found);
            }
        }
    }
    None
}

fn scan_return_expr<'a>(
    expr: &'a Expr,
    g: &str,
    param_types: &BTreeMap<String, Type>,
) -> Option<(&'a Expr, String)> {
    if let Some(method) = direct_method_call(expr, g, param_types) {
        return Some((expr, method));
    }
    if let Expr::Binary(_, op, l, r) = expr {
        if is_same_type_result_op(*op) {
            if let Some(method) = direct_method_call(l, g, param_types) {
                return Some((l, method));
            }
            if let Some(method) = direct_method_call(r, g, param_types) {
                return Some((r, method));
            }
        }
    }
    None
}

fn is_same_type_result_op(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Rem
            | BinOp::AddW
            | BinOp::SubW
            | BinOp::MulW
            | BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor
            | BinOp::Shl
            | BinOp::Shr
    )
}

/// `<name>.<method>()` (zero arguments — the pinned example's own shape;
/// a requirement with arguments is not attempted) where `name`'s
/// *declared* (unsubstituted) type is exactly the bare generic parameter
/// `g`.
fn direct_method_call(
    expr: &Expr,
    g: &str,
    param_types: &BTreeMap<String, Type>,
) -> Option<String> {
    let Expr::Call(callee, _, args) = expr else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let Expr::Field(base, _, method) = callee.as_ref() else {
        return None;
    };
    let Expr::Name(_, base_name) = base.as_ref() else {
        return None;
    };
    match param_types.get(base_name) {
        Some(Type::Generic(pn)) if pn == g => Some(method.clone()),
        _ => None,
    }
}

// --- tests --------------------------------------------------------------
//
// 02-language.md §6.3: "A const argument is `bool`, `char`, an integer, or
// a fieldless enum, evaluated by the comptime engine." plans/M2.md item H
// narrows this for M2: "Const arguments evaluate only as literals,
// fieldless-enum variants, and direct `const` references (the comptime
// engine is M3); anything else fails closed." These pin `eval_const_expr`
// directly against each of those four shapes plus the arithmetic-rejection
// case, rather than only through a full generic-instantiation golden.
//
// `eval_const_expr` takes a `&ModuleCtx` (for const-name/enum-variant
// lookup), so the tests build one the same way `sema::mod::check` does —
// lex -> parse -> `types::declare` -> `bodies::build_module_ctx` — real
// production code, not a mock/fixture rig.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{lexer, parser};

    fn build_mctx(src: &str) -> ModuleCtx {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        let decl_items = types::declare(&module).expect("test source must declare");
        bodies::build_module_ctx(&module, &decl_items)
    }

    const SRC: &str = "module examples.const_eval

const LIMIT: u64 = 4

enum Color:
    Red
    Green
    Blue

pub fn use_const() -> u64:
    return LIMIT
";

    #[test]
    fn eval_const_expr_literal_int_bool_char() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        assert_eq!(
            eval_const_expr(&Expr::Int(span, "42".to_string()), &mctx, 0).unwrap(),
            ConstVal::Int(42)
        );
        assert_eq!(
            eval_const_expr(&Expr::Bool(span, true), &mctx, 0).unwrap(),
            ConstVal::Bool(true)
        );
        assert_eq!(
            eval_const_expr(&Expr::Char(span, "x".to_string()), &mctx, 0).unwrap(),
            ConstVal::Char("x".to_string())
        );
    }

    /// A bare `const` reference resolves by looking its initializer back
    /// up and evaluating that (here, `LIMIT`'s own `4` literal).
    #[test]
    fn eval_const_expr_resolves_a_const_name() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        let result = eval_const_expr(&Expr::Name(span, "LIMIT".to_string()), &mctx, 0);
        assert_eq!(result.unwrap(), ConstVal::Int(4));
    }

    /// A fieldless enum variant path (`Color.Red`) evaluates to its own
    /// `(enum name, variant name)` pair.
    #[test]
    fn eval_const_expr_fieldless_enum_variant() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        let expr = Expr::Field(
            Box::new(Expr::Name(span, "Color".to_string())),
            span,
            "Red".to_string(),
        );
        let result = eval_const_expr(&expr, &mctx, 0);
        assert_eq!(
            result.unwrap(),
            ConstVal::Variant("Color".to_string(), "Red".to_string())
        );
    }

    /// An unknown const name fails closed rather than guessing.
    #[test]
    fn eval_const_expr_unknown_const_name_fails_closed() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        assert!(eval_const_expr(&Expr::Name(span, "NOPE".to_string()), &mctx, 0).is_err());
    }

    /// Arithmetic is explicitly out of scope for M2's const-argument
    /// evaluator (plans/M2.md item H: "anything else fails closed") — a
    /// binary expression is rejected, not folded.
    #[test]
    fn eval_const_expr_rejects_arithmetic() {
        let mctx = build_mctx(SRC);
        let span = Span::default();
        let expr = Expr::Binary(
            span,
            BinOp::Add,
            Box::new(Expr::Int(span, "1".to_string())),
            Box::new(Expr::Int(span, "1".to_string())),
        );
        assert!(
            eval_const_expr(&expr, &mctx, 0).is_err(),
            "arithmetic in a const argument must fail closed, not evaluate"
        );
    }
}
