//! `@layout` exact-bytes checking / completion / dump, plus typed MMIO
//! register helpers and claim-partition checks (plans/M7.md items B/C,
//! 03-hardware.md §2/§3). Extracted from `types.rs` along the artifact
//! boundary; call sites keep using `sema::types::{...}` via re-exports.

use std::collections::{BTreeMap, BTreeSet};

use crate::sema::typed::TypedProgram;
use crate::sema::types::{
    DeclField, DeclItem, DeclMember, DeclStruct, Type, TypeArg, components_by_name,
    declared_layout_kind, push_line, render_type,
};
use crate::sema::{SemaError, unimplemented_at};
use crate::syntax::ast::{self, Attr, Expr, GenericArg, Item, Member, Module, Span, StructItem};
use crate::syntax::printer;

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
pub(crate) const MAX_LAYOUT_NEST_DEPTH: usize = 16;

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
pub(crate) const MAX_LAYOUT_NEST_EXPANSIONS: u32 = 1024;

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
            // Local or imported module-level const (plans/M15.md item E:
            // runtime.wr sizes overlays to imported `CORE_SLOTS`). `eval_const`
            // already resolves both tables.
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
