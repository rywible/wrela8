use std::collections::{BTreeMap, BTreeSet};

use crate::sema::SemaError;
use crate::sema::types::{
    DeclField, DeclItem, DeclMember, DeclStruct, Type, TypeArg, components_by_name,
    declared_layout_kind, push_line, render_type,
};
use crate::syntax::ast::{self, Attr, Expr, GenericArg, Item, Member, Module, Span, StructItem};
use crate::syntax::printer;

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
    pub ty: String,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutEntry {
    Field(LayoutField),
    Padding { offset: u64, size: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutType {
    pub name: String,
    pub kind: LayoutKind,
    pub endian: LayoutEndian,
    pub size: Option<u64>,
    pub padding: u64,
    pub entries: Vec<LayoutEntry>,
}

impl LayoutType {
    pub fn require_size(&self, context: &str) -> Result<u64, SemaError> {
        match self.size {
            Some(size) => Ok(size),
            None => Err(SemaError::nowhere(
                "type",
                format!(
                    "`@layout` type `{}` has no computed size at {context}: its array length is a \
                     `const` name, so `sema::types::check_layouts` deferred its sizing and \
                     `complete_layouts` (which resolves the length after const evaluation) never \
                     ran on it (03-hardware.md §3.1, plans/M10.md item A2b)",
                    self.name
                ),
            )),
        }
    }
}

fn scalar_field_size(name: &str) -> Option<u64> {
    match name {
        "u8" | "i8" => Some(1),
        "u16" | "i16" => Some(2),
        "u32" | "i32" => Some(4),
        "u64" | "i64" => Some(8),
        _ => None,
    }
}

const MMIO_WRAPPERS: &[&str] = &["ReadOnly", "WriteOnly"];

fn layout_error(message: String, span: Span) -> SemaError {
    SemaError::at("type", message, span)
}

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

type LayoutDecls<'a> = BTreeMap<String, &'a StructItem>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FieldBytes {
    size: u64,
    align: u64,
}

impl FieldBytes {
    fn scalar(size: u64) -> Self {
        FieldBytes { size, align: size }
    }
}

pub(crate) const MAX_LAYOUT_NEST_DEPTH: usize = 16;

pub(crate) const MAX_LAYOUT_NEST_EXPANSIONS: u32 = 1024;

const MAX_LAYOUT_BYTES: u64 = 16 * 1024 * 1024;

type LengthConsts = BTreeMap<String, u64>;

struct NestCtx<'a> {
    stack: Vec<String>,
    budget: u32,
    lens: Option<&'a LengthConsts>,
}

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
    if n.name == "DmaShared" {
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
    if crate::sema::classes::name_holds_authority(&n.name) {
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
    let Some(elem) = elem else { return Ok(None) };
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
            return Ok(None);
        };
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
    match inner.size {
        Some(size) => Ok(Some(FieldBytes { size, align })),
        None => Ok(None),
    }
}

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
    if walk.cursor > MAX_LAYOUT_BYTES {
        return Err(layout_error(
            format!(
                "`@layout` struct `{name}` covers {} bytes, more than the {MAX_LAYOUT_BYTES} this \
                 compiler will lay out in one declaration; the machine has 512 MiB in total, so a \
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

struct Walk {
    entries: Vec<LayoutEntry>,
    cursor: u64,
    padding: u64,
    align: u64,
    last_field: Option<(String, u64, u64)>,
    saw_field: bool,
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
        let Some(FieldBytes { size, align }) = bytes else {
            walk.deferred = true;
            continue;
        };
        if walk.deferred {
            continue;
        }
        walk.align = walk.align.max(align);
        let offset = explicit.unwrap_or(walk.cursor);
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
                Item::Static(_) => {}
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
            return Err(SemaError::nowhere(
                "type",
                format!(
                    "`static {}` has type `{}`, but `@placed` requires a `@layout(runtime)` type \
                     (03-hardware.md §3.1)",
                    s.name,
                    render_type(&s.ty)
                ),
            ));
        };
        if !targs.is_empty() {
            return Err(SemaError::nowhere(
                "type",
                format!(
                    "`static {}` has type `{}`, but `@placed` requires a non-generic \
                     `@layout(runtime)` type (03-hardware.md §3.1)",
                    s.name,
                    render_type(&s.ty)
                ),
            ));
        }
        let Some(layout) = layouts.iter().find(|l| l.name == *type_name) else {
            return Err(SemaError::nowhere(
                "type",
                format!(
                    "`static {}` has type `{type_name}`, which is not a `@layout` type; \
                     `@placed` requires a `@layout(runtime)` type (03-hardware.md §3.1)",
                    s.name
                ),
            ));
        };
        if layout.kind != LayoutKind::Runtime {
            return Err(SemaError::nowhere(
                "type",
                format!(
                    "`static {}` has type `{type_name}` (`@layout({})`), but `@placed` requires \
                     a `@layout(runtime)` type (03-hardware.md §3.1)",
                    s.name,
                    layout.kind.as_str()
                ),
            ));
        }
        if let Some(earlier) = by_addr.insert(s.addr, s.name.clone()) {
            return Err(SemaError::nowhere(
                "type",
                format!(
                    "`static {}` and `static {earlier}` both claim `@placed({:#x})`; \
                     03-hardware.md §3.1 allows at most one placed static per address",
                    s.name, s.addr
                ),
            ));
        }
    }
    Ok(())
}

pub fn check_layouts(module: &Module) -> Result<Vec<LayoutType>, SemaError> {
    check_placed_attrs(module)?;
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
        completed.require_size("the end of layout completion")?;
        *l = completed;
    }
    Ok(())
}

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
            if !program.consts.contains_key(name) && !program.imported.consts.contains_key(name) {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmioRegister {
    pub name: String,
    pub direction: Option<MmioDirection>,
    pub scalar: String,
    pub offset: u64,
    pub size: u64,
}

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

struct Mint {
    field: String,
    field_ty: String,
    layout: String,
    span: Span,
}

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

pub fn driver_mmio_mints(items: &[DeclItem], driver: &str) -> Option<Vec<String>> {
    let mut structs: BTreeMap<String, &DeclStruct> = BTreeMap::new();
    for item in items {
        if let DeclItem::Struct(s) = item {
            structs.insert(s.name.clone(), s);
        }
    }
    mmio_mints_of(driver, &structs, &components_by_name(items))
}

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

pub fn mmio_consumed_end(layout: &LayoutType) -> u64 {
    consumed_ranges(layout)
        .into_iter()
        .map(|(_, end, _)| end)
        .max()
        .unwrap_or(0)
}

pub fn check_mmio_claims(
    module: &Module,
    items: &[DeclItem],
    layouts: &[LayoutType],
) -> Result<(), SemaError> {
    let components = components_by_name(items);
    let by_name: BTreeMap<&str, &LayoutType> =
        layouts.iter().map(|l| (l.name.as_str(), l)).collect();

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
                continue;
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
