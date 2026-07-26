//! MachineWir (plans/M5.md item B, decision 3): the typed tree's lowered
//! target — one plain enum instruction list per fn, flat and linear, no
//! basic-block graph. This module owns the shape (`MwirProgram`/`MwirFn`/
//! `Inst`), its stable text dump (`--stage=mwir`), and the aggregate
//! layout rule (`size_of`/`LayoutCtx`) codegen (plans/M5.md item C) will
//! share. `lower.rs` owns the typed-tree walk that produces a
//! `MwirProgram`; this file never inspects `sema::typed` itself.
//!
//! ## Shape (decided here, plan's own suggestion finalized)
//!
//! - `MwirProgram { fns, rodata }`: `fns` is keyed by the *exact* string
//!   `sema::typed::CalleeKey::spelling()` produces for whatever declared
//!   the fn/method/instantiation — a plain top-level fn's bare name, a
//!   method's `Struct.member`, an instantiated generic's own
//!   `"fn:name[args]"`/`"struct:name[args].member"` spelling — so a
//!   `Call` instruction's own `key` field is always copied verbatim from
//!   the typed tree's own `CalleeKey::spelling()`, never re-derived or
//!   re-matched a second way. `rodata` is every `Str`/`BStr` literal's
//!   decoded bytes, deduplicated by first occurrence, referenced by
//!   `ConstText::data` (an index) — `Call`/`ConstText`/every instruction
//!   below is "yours to finalize" per the plan; this is that finalization,
//!   recorded once, here.
//! - `MwirFn { receiver, params, ret, temp_types, body }`: every value
//!   (scalar or aggregate) lives in a numbered `Temp` — a virtual slot,
//!   *not* a machine register or stack offset (codegen, item C, decides
//!   that). `temp_types[t.0]` is temp `t`'s own static type (`sema::types::Type`,
//!   reused directly, decision: no duplicate type representation, exactly
//!   like the typed tree itself). `receiver`/`params` name which temps
//!   are bound at entry (`receiver` also carries the declared
//!   `AccessMode` — `Mut` is what makes a call site expect the callee to
//!   hand a final self value back, see `Inst::Call::write_backs`
//!   below). `ret` is the declared return type; a plain `Type` rather
//!   than the plan's own suggested `ret_size: usize` — a byte size is a
//!   *layout* fact (`size_of` below), derivable from `ret` whenever
//!   codegen actually needs it, and computing it here would force this
//!   module's lowering entry point to also carry a whole-program
//!   struct/enum field-type table it otherwise never needs (every value
//!   this module manipulates already carries its own `Type` from the
//!   typed tree; nothing here ever needs one value's *byte size* to
//!   decide what instruction to emit).
//!
//! ## Aggregate layout (decision 3's "documented dumb layout")
//!
//! One rule, `size_of` below, shared by codegen/runtime later:
//! - Every scalar (`bool`/every integer width/`char`/`f32`/`f64`/`unit`/
//!   `never`) occupies exactly one 8-byte slot — no packing, ever, at any
//!   width (decision 4's "fixed frame layout"/"every mwir temp lives in a
//!   fixed stack slot" starts here: a slot is always 8 bytes, so codegen
//!   never computes a sub-word offset).
//! - `[T; N]`: `size_of(T) * N` (element stride × len, per the plan).
//! - `(A, B, ...)`: the sum of each component's own size, in order — a
//!   tuple is an anonymous struct.
//! - A struct: the sum of its fields' own sizes, in declaration order.
//! - An enum (`Option`/`Result`/a user enum): one 8-byte tag slot plus
//!   the *widest* variant's own payload size (payload fields summed like
//!   a struct, then the max taken across variants) — "tag u64 + max-
//!   payload union", the plan's own words, so every variant's payload
//!   lives at the identical fixed offset (tag first, payload immediately
//!   after) regardless of which variant is actually live; reading a
//!   variant's payload while a *different* tag is live is always
//!   in-bounds (garbage, never a fault) — `lower.rs` relies on exactly
//!   this fact to compute pattern-match sub-tests unconditionally,
//!   documented at `lower::lower_pattern_test`.
//!
//! `size_of` needs one whole-program fact `sema::typed::TypedProgram`
//! does not keep: a plain (non-generic) struct's/enum's own field/
//! variant-payload *types* (the typed tree only keeps `TypedStruct::fields`'s
//! *names*, decision 5's own producer-gap note — enough for `lower.rs`'s
//! instruction emission, since every value there already carries its own
//! type from the expression that produced it, but not enough to add up a
//! *whole aggregate's* byte size independently). `LayoutCtx`/
//! `build_layout_ctx` below recompute that one fact the same dumb way
//! `sema::dump`/`sema::check` already recompute `specialize`/`declare`
//! from the raw `ast::Module` rather than threading extra state through
//! `check_typed` (`sema::mod.rs`'s own `dump` doc comment: "dumb, no
//! state threaded from check") — callers who need real sizes (item C)
//! build one `LayoutCtx` per module once, the same way they already
//! parse the module once. Lowering itself (`lower.rs`) never needs a
//! `LayoutCtx` at all (see above); it is exercised here only by this
//! module's own unit tests plus whatever item C ends up needing.
//!
//! **Known gap, disclosed rather than faked**: `size_of`/`LayoutCtx`
//! cover every *plain* (non-generic) struct/enum and every array whose
//! length is a literal or a plain module `const` reference (the same
//! literal-or-const scope `lower.rs`'s own `eval_array_len` accepts).
//! An *instantiated* generic struct/enum's own substituted field/variant
//! types are not reconstructed here — `sema::generics`'s own
//! substitution machinery (`Subst`/`subst_type`) is private to that
//! module and re-deriving it independently here would duplicate real
//! logic rather than a dumb fact, so `size_of` fails closed
//! (`Err("sizing an instantiated generic struct/enum is not implemented yet")`)
//! for a `Type::Named` naming an instantiation key. None of item B's own
//! required goldens need it (the "generic instantiation" case lowers a
//! generic *fn* instantiated at a concrete scalar type, never a generic
//! struct/enum) — recorded here as the honest boundary, not silently
//! routed around.
//!
//! ## Instruction set (decided here; the plan's own list finalized)
//!
//! Every arithmetic/shift/negation abort message that never embeds a
//! *runtime* value (every ordinary-overflow, division-by-zero,
//! remainder-by-zero, unary-negation-overflow, and `<<`-lost-bits case)
//! is precomputed once at lowering time via `abort_message`/
//! `neg_abort_message` below and carried on the instruction verbatim —
//! codegen prints it unchanged at its abort path (decision 4: "prints
//! the evaluator's own abandonment message shape"). The two cases whose
//! own wording embeds a value only known at *runtime* — a shift's own
//! out-of-range count, an index's own out-of-bounds value — carry just
//! the compile-time half instead (`bits`, `len`): codegen (item C) is
//! responsible for interpolating the live register value into the exact
//! same template `eval::value`/`eval::interp` use (documented on
//! `Inst::Shift`/`Inst::IndexGet` below).
//!
//! `assert`/`panic`'s own message is precomputed the same way, but only
//! when it is a literal string (`sema::typed::TypedExprKind::Str`) —
//! `lower.rs` fails closed on a non-literal assert/panic message (see
//! its own module doc's fail-closed enumeration): the evaluator falls
//! back to Rust's own `{:?}` `Debug` formatting for a non-`Str` message
//! value (`eval::interp::render_message`), and reproducing that in
//! machine code is not a real lowering, it is a fake one.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::sema::types::{self, Type};
use crate::syntax::ast::BinOp;

// --- temps -------------------------------------------------------------

/// A virtual value slot, numbered in allocation order within one
/// `MwirFn` — never a register or stack offset (codegen's own job, item
/// C). `Copy`/`PartialEq`/`Ord` so a temp is as cheap to carry around and
/// as easy to put in a `BTreeMap` key (pattern-binding tables, `lower.rs`)
/// as a bare integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Temp(pub usize);

impl std::fmt::Display for Temp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "t{}", self.0)
    }
}

// --- program/fn shape ----------------------------------------------------

/// The whole lowered program: every lowered fn/method/`init`/generic
/// instantiation, keyed by `sema::typed::CalleeKey::spelling()`
/// (`BTreeMap`, CLAUDE.md), plus every `Str`/`BStr` literal's decoded
/// bytes (`rodata`, index-addressed by `Inst::ConstText::data`, in first-
/// occurrence order across the whole lowering walk — deterministic
/// because `lower.rs` walks `fns`/`structs`/`instantiations` in their own
/// already-`BTreeMap`-ordered iteration).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MwirProgram {
    pub fns: BTreeMap<String, MwirFn>,
    pub rodata: Vec<Vec<u8>>,
}

/// One lowered fn/method/`init`/instantiation body. `receiver` is
/// `Some((self_temp, mode))` for a method/`init` (mirrors
/// `sema::typed::TypedFn::receiver` exactly); a `Mut`-mode receiver is
/// what makes a *call site* targeting this fn expect a written-back self
/// value back (`Inst::Call::write_backs`) — `init` reuses this
/// uniformly (`sema::bodies` already types `init`'s own receiver as
/// `Mut`, so no separate "is this an init" flag exists here at all;
/// `lower.rs`'s own call-site logic is what special-cases `init`'s
/// *result*, not this shape). Each entry of `params` carries the
/// parameter's own declared `AccessMode` so codegen's prologue/epilogue
/// can pass/write-back a non-receiver `mut` the same way it already
/// handles a `mut` receiver (02-language.md §5.1 / plans/M9.md item CC).
#[derive(Debug, Clone, PartialEq)]
pub struct MwirFn {
    pub receiver: Option<(Temp, crate::syntax::ast::AccessMode)>,
    pub params: Vec<(Temp, crate::syntax::ast::AccessMode)>,
    pub ret: Type,
    /// `temp_types[t.0]` is temp `t`'s own static type; `temp_types.len()`
    /// is this fn's own temp count (the plan's own suggested `temps:
    /// usize` field, generalized to also carry each temp's type — the
    /// dump wants both, so one `Vec` serves both purposes instead of two
    /// fields that could disagree).
    pub temp_types: Vec<Type>,
    pub body: Vec<Inst>,
}

impl MwirFn {
    pub fn temp_count(&self) -> usize {
        self.temp_types.len()
    }
}

// --- instructions --------------------------------------------------------

/// One MachineWir instruction. Every arithmetic/shift op reuses
/// `syntax::ast::BinOp` directly rather than a duplicate enum (decision
/// 4: "dumb, no seams for their own sake" — the typed tree's own
/// `TypedExprKind::Binary` already carries the exact same `BinOp`, so
/// this just keeps carrying it one stage further).
#[derive(Debug, Clone, PartialEq)]
pub enum Inst {
    // --- constants ---------------------------------------------------
    /// An integer-scalar constant (any of the ten integer widths, or
    /// `char` — carried as its codepoint's own ordinal — see
    /// `Inst::ConstChar` for why `char` gets its own variant instead).
    ConstInt {
        dst: Temp,
        ty: Type,
        value: i128,
    },
    ConstBool {
        dst: Temp,
        value: bool,
    },
    /// `bits` is the value's own IEEE-754 bit pattern, widened to `u64`
    /// (an `f32`'s 32 bits in the low half, zero-extended) — `ty` (`F32`/
    /// `F64`) says which width the low bits mean; carrying bits instead
    /// of a Rust `f64` keeps `Inst` comparable with a derived `PartialEq`
    /// (`f64: Eq` does not hold, NaN especially) without hand-rolling one.
    ConstFloat {
        dst: Temp,
        ty: Type,
        bits: u64,
    },
    /// `char` is its own variant (not folded into `ConstInt`) because it
    /// is not one of `int_shape`'s ten integer scalar types
    /// (`eval::value::int_shape`) — giving it a plain `char` payload
    /// avoids inventing a width/signedness for something that has
    /// neither.
    ConstChar {
        dst: Temp,
        value: char,
    },
    ConstUnit {
        dst: Temp,
    },
    /// A `Static[Str]`/`Static[Bytes[N]]` literal's decoded bytes,
    /// interned into `MwirProgram::rodata`; `data` indexes that table.
    ConstText {
        dst: Temp,
        data: usize,
    },

    /// An explicit copy — `lower.rs` emits this for both an ordinary
    /// value read/rebind *and* `take` (decision 2: "a take is a copy in
    /// mwir", the evaluator's own "you just move" observational
    /// contract preserved exactly).
    Copy {
        dst: Temp,
        src: Temp,
    },

    // --- aggregates: struct/tuple/array literals, static projection ---
    /// Builds a struct/tuple/fixed-array value from its already-lowered
    /// element/field temps, in the aggregate's own declared/positional
    /// order (`size_of`'s own layout rule: field `i` occupies slot `i`).
    MakeAggregate {
        dst: Temp,
        elems: Vec<Temp>,
    },
    /// plans/M9.md item C2: format a core scalar into `String[..capacity]`
    /// (decimal / bool / char). `src_ty` selects the conversion.
    FormatScalar {
        dst: Temp,
        src: Temp,
        src_ty: Type,
        capacity: usize,
    },
    /// plans/M9.md item C2: concatenate two `String[..lhs_cap]` /
    /// `String[..rhs_cap]` values into `String[..lhs_cap+rhs_cap]`.
    StringConcat {
        dst: Temp,
        lhs: Temp,
        rhs: Temp,
        lhs_cap: usize,
        rhs_cap: usize,
    },
    /// A compile-time-known-offset read: a struct field (by
    /// `TypedStruct::fields`'s own index), a tuple component, or a fixed-
    /// array literal element accessed by a *literal* index. Never
    /// bounds-checked — the index is a producer-guaranteed-valid
    /// constant, not a runtime value (`Inst::IndexGet` below is the
    /// dynamic-index, bounds-checked counterpart).
    Project {
        dst: Temp,
        base: Temp,
        index: usize,
    },
    /// The in-place counterpart of `Project`, for a `mut`-place field
    /// write (`self.field = ...`, `place.0 = ...`): overwrites slot
    /// `index` of the aggregate held in `base` with `value`.
    SetField {
        base: Temp,
        index: usize,
        value: Temp,
    },

    /// `base[index]` (`base`'s type is `[T; N]`) — bounds-checked against
    /// the compile-time-known `len` (`N`, resolved by `lower.rs`'s own
    /// `eval_array_len`). A failure's exact wording
    /// (`eval::interp::place_mut`/`eval_expr`'s own
    /// `"index {i} out of bounds (length {len})"`) embeds the *runtime*
    /// index value — codegen renders that at its own abort path using
    /// the live `index` operand's value; this instruction only carries
    /// the compile-time half (`len`).
    IndexGet {
        dst: Temp,
        base: Temp,
        index: Temp,
        len: usize,
    },
    /// The in-place counterpart of `IndexGet`, for `base[index] = value`.
    IndexSet {
        base: Temp,
        index: Temp,
        value: Temp,
        len: usize,
    },
    /// Indexed load through a placed `@layout(runtime)` array field
    /// (plans/M10.md item B1). `base` holds the placed static's address
    /// word; `field_offset` / `elem_stride` are dense layout bytes (not
    /// mwir slot sizes); bounds-checked against `len` exactly like
    /// `IndexGet`, which is why this is the first source path that emits
    /// `bl __wrela_abort_val` against a placed table.
    PlacedIndexGet {
        dst: Temp,
        base: Temp,
        field_offset: u64,
        index: Temp,
        len: usize,
        elem_stride: u64,
        ty: Type,
    },
    /// In-place counterpart of `PlacedIndexGet` for
    /// `STATIC.array_field[i] = value`.
    PlacedIndexSet {
        base: Temp,
        field_offset: u64,
        index: Temp,
        value: Temp,
        len: usize,
        elem_stride: u64,
        ty: Type,
    },
    /// Packed-byte load through an unbounded `Bytes` parameter handle
    /// (plans/M10.md item B4 / decisions 595–596). `base` holds the
    /// two-word `(addr, len)` handle; bounds-checked against the handle's
    /// own `len` word; loads one packed byte via `ldrb` (not a slot
    /// stride — packed is what rodata / the console device use).
    BytesIndexGet {
        dst: Temp,
        base: Temp,
        index: Temp,
    },

    // --- enums ---------------------------------------------------------
    /// Builds an enum value: `tag` is the variant's own declaration-order
    /// index (`eval::value::OPTION_NONE`/.../`RESULT_ERR` for the two
    /// builtin sums, else a user enum's own position in
    /// `TypedProgram::enums`, exactly like `interp::variant_index`).
    MakeEnum {
        dst: Temp,
        tag: usize,
        payload: Vec<Temp>,
    },
    /// Reads an enum value's own tag as a `u64`-typed temp (`temp_types`
    /// for the `dst` temp is always `Type::U64`).
    EnumTag {
        dst: Temp,
        src: Temp,
    },
    /// Reads payload slot `index` of an enum value — always safe
    /// regardless of the enum's *actual* live tag (`size_of`'s own "tag +
    /// max-payload union" layout: every variant's payload lives at the
    /// identical fixed offset), which is exactly what lets `lower.rs`
    /// compute a pattern's payload sub-tests unconditionally before ever
    /// confirming the tag matches (`lower::lower_pattern_test`'s own doc
    /// comment).
    EnumPayload {
        dst: Temp,
        src: Temp,
        index: usize,
    },

    // --- arithmetic (02-language.md §6.1) -------------------------------
    /// Ordinary `+ - *`: abandons on overflow in every profile. `op` is
    /// one of `BinOp::Add`/`Sub`/`Mul`; `abort` is the exact evaluator
    /// wording (`abort_message`) — fully precomputed, since none of these
    /// three ever embeds a runtime value.
    ArithChecked {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
        abort: String,
    },
    /// Wrapping `+% -% *%`: reduces modulo `2^width`, never abandons.
    /// Also doubles as ordinary (never-overflowing) `+ - * / %` on a
    /// *float* `ty` (`F32`/`F64`) — 02-language.md §6.1's own "floats
    /// never abandon on ordinary `+ - *`" (and division: IEEE 754 has no
    /// division-by-zero trap, it produces an infinity/NaN): a float
    /// operand never needs `ArithChecked`/`DivRem`'s own abort machinery
    /// at all, so `lower.rs` routes it through this instruction instead —
    /// `ty` alone tells codegen which real op to emit (a wrapping integer
    /// op, or a plain floating op), so no separate "float arith"
    /// instruction exists.
    ArithWrapping {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
    },
    /// `/ %`: truncates toward zero; abandons on division by zero
    /// (`abort_zero`) and on the signed `MIN / -1` overflow case
    /// (`abort_overflow` — reachable for `Div` only, never `Rem`, exactly
    /// like `eval::value::eval_div_rem`; carried uniformly on both ops
    /// anyway, matching that fn's own uniform bounds-check, rather than
    /// special-casing `Rem` to omit a field it can never trigger).
    DivRem {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
        abort_zero: String,
        abort_overflow: String,
    },
    /// `<< >>`: abandons on an out-of-range count (`>=` `bits`) —
    /// `eval::value::eval_shift`'s own
    /// `"shift count {c} is out of range for a {bits}-bit type"` embeds
    /// the *runtime* count, so only `bits` travels here; codegen
    /// interpolates the live `rhs` value. `<<` additionally abandons on
    /// lost high bits — that wording never embeds a runtime value, so
    /// `lost` carries it precomputed (`Some(..)` for `Shl`, `None` for
    /// `Shr`, which can never lose bits).
    Shift {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
        bits: u32,
        lost: Option<String>,
    },
    /// `& | ^` — never abandons.
    Bitwise {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
    },
    /// `< <= > >= == !=` on scalars/`bool`/`char` (`TypedExprKind::Binary`'s
    /// own documented scope — a user type's operator is always `OpCall`,
    /// an ordinary `Call`, never this). `dst` is always `Type::Bool`;
    /// `ty` is the *operand* type (codegen needs it to pick signed vs.
    /// unsigned vs. floating comparison — `eval::value::eval_compare`'s
    /// own per-`Value`-shape dispatch, one level earlier).
    Compare {
        dst: Temp,
        op: BinOp,
        ty: Type,
        lhs: Temp,
        rhs: Temp,
    },
    /// Unary negation: abandons on the one signed overflow case
    /// (`MIN.neg()`); never for a float. `abort` is
    /// `neg_abort_message()`, fully precomputed (never embeds a runtime
    /// value).
    Neg {
        dst: Temp,
        ty: Type,
        src: Temp,
        abort: String,
    },
    /// Bitwise `~` — never abandons.
    BitNot {
        dst: Temp,
        ty: Type,
        src: Temp,
    },
    /// `x.to[T]()` (02-language.md §6.1): a checked scalar-to-scalar
    /// conversion — `ty` is the *target* `T`; abandons out of range.
    /// `abort` is fully precomputed (`eval::value::eval_to_scalar`'s own
    /// `` `.to[{}]()` conversion out of range`` — the target type's own
    /// name is compile-time known, so unlike `Shift`/`IndexGet` this
    /// message never needs a runtime value interpolated).
    Convert {
        dst: Temp,
        ty: Type,
        src: Temp,
        abort: String,
    },
    /// Boolean `not` (`!`) — logical negation of a `bool` temp, distinct
    /// from `BitNot` (`~`, which does not exist for `bool`, per
    /// `int_shape`).
    Not {
        dst: Temp,
        src: Temp,
    },
    /// A trap-free, non-short-circuiting boolean AND — `lower.rs`'s own
    /// internal combinator for folding a pattern's several sub-tests
    /// (tag match + every payload sub-pattern, or every tuple/array
    /// element) into one result; never used for source-level `and`
    /// (`TypedExprKind::And`), which lowers to real short-circuit control
    /// flow instead (its own right operand may have side effects/traps a
    /// pattern sub-test never does — `lower.rs`'s own module doc explains
    /// the distinction).
    BoolAnd {
        dst: Temp,
        lhs: Temp,
        rhs: Temp,
    },

    // --- control flow: a flat, explicitly-ordered instruction index ---
    /// An unconditional jump to `target`, an instruction index into this
    /// same `MwirFn::body` (decision 3: "label indices into the flat
    /// list" — no separate label/block type, the index *is* the label).
    Jump {
        target: usize,
    },
    /// Jumps to `target` when `cond` (a `bool` temp) is false; falls
    /// through otherwise. The one conditional-jump flavor this module
    /// needs (decision: "jump/jump-if" realized as this single form) —
    /// every other shape (`if`/`while`/`for`/`match`/short-circuit `and`/
    /// `or`) lowers to one or more of these, `lower.rs`'s own module doc
    /// works through each.
    JumpIfFalse {
        cond: Temp,
        target: usize,
    },

    // --- calls -----------------------------------------------------------
    /// `key` is a `sema::typed::CalleeKey::spelling()` string, copied
    /// verbatim from the typed tree's own resolved callee — the same
    /// spelling `MwirProgram::fns` is keyed by, so a call always finds
    /// its own target by exact string equality, never re-resolved.
    /// `args` are the already-lowered argument temps, receiver first when
    /// the callee has one (mirrors `sema::typed::TypedFn::params`'s own
    /// declared order — `bind_params`'s "1:1 with the callee's declared
    /// parameters" convention, one level down). `dst` always gets a fresh
    /// temp of the callee's own return type, even when that type is
    /// `unit`/the result is discarded (an `ExprStmt`) — one uniform shape
    /// rather than an `Option` no call site actually needs to special-
    /// case (a discarded temp is simply never read again). `write_backs`
    /// lists every `(args-index, place_temp)` the callee is expected to
    /// write back into: a `Mut` receiver (args index 0, when present)
    /// and every non-receiver `mut` parameter whose call-site operand
    /// is a place (02-language.md §5.1 / plans/M9.md item CC). Each
    /// `place_temp` is the same temp that appears at `args[index]` —
    /// codegen passes those by pointer and the callee's epilogue writes
    /// through the saved pointer, so the call site itself does nothing
    /// after the `BL` (the same proof that previously applied only to
    /// `mut self`). Sorted by args-index for a deterministic dump.
    Call {
        dst: Temp,
        write_backs: Vec<(usize, Temp)>,
        key: String,
        args: Vec<Temp>,
    },
    /// `value` is `None` for a bare `return`/a `unit`-returning fn falling
    /// off the end of its body.
    Return {
        value: Option<Temp>,
    },

    // --- typed MMIO (plans/M7.md item C's surface, item H1's emission) ---
    /// `<mmio>.<register>.read()` (03-hardware.md §2). `base` holds the
    /// `Mmio[L]` value — decision 11's one word, the claim's own guest
    /// base address — and `offset` is the register's declared `@offset`,
    /// already checked for width, alignment and non-overlap by
    /// `types::check_layouts`/`check_mmio_claims`. `ty` is the register's
    /// declared scalar and is the *whole* of what picks the load width:
    /// there is no widening, no promotion and no inference anywhere
    /// downstream of the declaration.
    ///
    /// Effectful by construction: this is a statement-level node with no
    /// value form above it (a register selection has none — 03 §2), so
    /// there is nothing an optimizer could hoist even if this backend had
    /// one (`compiler.codegen.naive-locked`).
    MmioRead {
        dst: Temp,
        base: Temp,
        offset: u64,
        ty: Type,
    },
    /// `<mmio>.<register>.write(v)`. Same base/offset/width discipline as
    /// `MmioRead`; `value` is already the register's declared scalar
    /// (`sema::bodies::check_mmio_access` hands that type to `check_expr`
    /// as the expected type, so a mismatch never reaches here).
    MmioWrite {
        base: Temp,
        offset: u64,
        ty: Type,
        value: Temp,
    },
    /// plans/M7.md item G, decision 12: materialize an `IrqCap[V]`'s
    /// runtime word — the vector bit index the image bound to this
    /// `@driver`. `driver` is the owning struct's name; `layout` resolves
    /// it against the sealed graph's `vector=` and patches the codegen
    /// reloc. Zero is never a valid result (bit 0 is M6's deadline
    /// vector); a driver whose device declared no vector never reaches
    /// here (`eval::image_checks::check_vector_bindings` rejects first).
    LoadIrqVector {
        dst: Temp,
        driver: String,
    },
    // --- plans/M7.md item G, decision 17: InterruptCell[T] (03 §6) --------
    //
    // Every op addresses the **live** driver-state word at
    // `self_ptr + field_off` (prologue's saved receiver pointer), never
    // the frame copy of `self`. A checkpoint can fire mid-turn; an ISR
    // that RMW'd only the frame would be stomped by a `mut self`
    // epilogue write-back. `field_off` is the byte offset of the cell
    // field inside the receiver aggregate (same layout `field_offset_size`
    // uses). `width` is 4 for `InterruptCell[u32]` (W forms).
    /// `load_acquire()` — LDAR from the live cell.
    InterruptCellLoadAcquire {
        dst: Temp,
        field_off: usize,
        width: u8,
    },
    /// `store_release(v)` / construction assign — STLR to the live cell.
    InterruptCellStoreRelease {
        field_off: usize,
        width: u8,
        value: Temp,
    },
    /// `swap_acquire(v)` — LDAXR/STLXR retry; returns the previous value.
    InterruptCellSwapAcquire {
        dst: Temp,
        field_off: usize,
        width: u8,
        value: Temp,
    },
    /// `fetch_or_release(v)` — LDAXR/ORR/STLXR retry; returns the previous value.
    InterruptCellFetchOrRelease {
        dst: Temp,
        field_off: usize,
        width: u8,
        value: Temp,
    },
    /// plans/M7.md item G: `wake(Driver.task)` — sticky store of 1 into
    /// that driver's wake-pending word in `rtdata`. Layout patches the
    /// address. Mask–arm–recheck: the bit is level-triggered; a wake
    /// before/during/after the bottom half's own cell observation is
    /// never lost (the scheduler rechecks the word before deciding the
    /// driver is idle — HVF commit of this item).
    Wake {
        driver: String,
    },

    /// `VirtQueue.prepare_block` (plans/M7.md item E4 / decisions 20–22):
    /// copy the header and status into the control-pool packaging area,
    /// record the payload address / length / direction in the meta slot,
    /// and mint a `QueueOp` word = absolute address of that meta record
    /// (the ring still uses descriptor head 0 for single-flight).
    /// `payload_len` is the `@layout(dma)` size of the own'd type — the
    /// descriptor length the device model validates against `SECTOR_SIZE`.
    QueuePrepare {
        dst: Temp,
        queue: Temp,
        permit: Temp,
        header: Temp,
        payload: Temp,
        status: Temp,
        device_writes: bool,
        payload_len: u32,
    },

    /// `VirtQueue.publish` (plans/M7.md item E3/E4, 03-hardware.md §3/§5,
    /// decision 15/16/20): the sealed ring-write sequence in normative
    /// order. `steps` is exactly `virtqueue::PUBLISH_WRITE_ORDER`. Real
    /// DRAM stores against pool-backed addresses (decision 20). `dst` is
    /// the minted `Receipt[P]` word (same identity as the operation).
    QueuePublish {
        dst: Temp,
        queue: Temp,
        operation: Temp,
        steps: &'static [&'static str],
    },

    /// `VirtQueue.drain` (plans/M7.md item E4, 03-hardware.md §4/§6):
    /// acquire used-ring visibility, validate the device-reported id
    /// against generation/epoch, check the reported length, resolve the
    /// matching receipt (wake its waiter with an `IoCompletion[P]`).
    /// `max` is the bounded drain count from source.
    QueueDrain {
        queue: Temp,
        max: u16,
    },

    /// `VirtQueue.suppress_interrupts` (03-hardware.md §7 / poll builds):
    /// set `VIRTQ_AVAIL_F_NO_INTERRUPT` on the available ring.
    QueueSuppressInterrupts {
        queue: Temp,
    },

    /// `VirtQueue.claim(receipt=take r)` (plans/M7.md item E4 / decision 22):
    /// sync claim of a drain-resolved receipt's `IoCompletion` stash — the
    /// bottom-half dual of `await receipt` when the driver holds the
    /// receipt itself (no parked waiter). Aborts if the meta is not
    /// `RESOLVED`.
    QueueClaim {
        dst: Temp,
        queue: Temp,
        receipt: Temp,
    },

    /// `VirtQueue.recover(receipt=take r)` (plans/M8.md item G / decision 17,
    /// 03-hardware.md §5's `Recovery` state / §9's `CompletionOutcome`):
    /// resolve a receipt through the recovery path and produce the outcome
    /// tag. Reads the slot's stamped epoch against the queue's live epoch
    /// and the slot's flags; returns **no payload** (§9: never reclaim
    /// possibly device-owned memory).
    QueueRecover {
        dst: Temp,
        queue: Temp,
        receipt: Temp,
    },

    /// `VirtQueue.reclaim(pool=P, payload=T)` (plans/M8.md item F /
    /// decision 37, 03-hardware.md §9's "and only then is memory
    /// reclaimed"): hand the quarantined slot's `own[P] T` handle back,
    /// but only once the **host** has recorded a device quiescence since
    /// `recover` quarantined it. Takes no receipt — `recover` consumed
    /// that (§5: a receipt resolves exactly once) and the queue is
    /// single-flight, so the quarantined slot is the queue's own meta
    /// record. Aborts by name when nothing is quarantined, and when the
    /// quiesce count has not moved.
    QueueReclaim {
        dst: Temp,
        queue: Temp,
    },

    /// `RunningDevice.reset(queue=mut q)` (plans/M7.md item H2b / decision 23,
    /// 03-hardware.md §9): full device reset on machine v1. Consumes
    /// `Running`, bumps the queue's live epoch (invalidating every prior
    /// receipt), and yields `Running` again — claim/negotiate/start are
    /// authority-only on this target, so re-walking them would invent a
    /// second configure path for a queue that already exists. Per-queue
    /// reset is a typed rejection (`VirtQueue.reset`), not this inst.
    DeviceReset {
        dst: Temp,
        device: Temp,
        queue: Temp,
    },

    /// Unconditional abandonment: `assert`'s own failure path, an
    /// explicit `panic(msg)`, and match's own defensive "no arm matched"
    /// fallthrough (present for parity with `interp::exec_stmt`'s own
    /// identical fallthrough, even though exhaustiveness already proved
    /// it unreachable — decision: reuse the exhaustiveness guarantee,
    /// never invent a default arm, but still emit the same defensive
    /// abort the evaluator itself keeps). `message` is `None` for a bare
    /// `assert cond` with no message.
    AssertFail {
        message: Option<String>,
    },
}

// --- abort message wording (locked against eval::value, decision 6) -----

/// `+ - *`'s own overflow wording — byte-identical to
/// `eval::value::eval_ordinary`'s `format!("arithmetic overflow in
/// `{}`", op.as_str())`. `op` must be `Add`/`Sub`/`Mul`/`Div`/`Rem`
/// (the five ops that can ever produce this exact message shape —
/// `Div`/`Rem` share it with `DivRem`'s own `abort_overflow` field).
pub fn abort_message(op: BinOp) -> String {
    format!("arithmetic overflow in `{}`", op.as_str())
}

/// Unary negation's own overflow wording — byte-identical to
/// `eval::value::eval_neg`'s fixed string.
pub fn neg_abort_message() -> String {
    "arithmetic overflow in unary `-`".to_string()
}

/// Division/remainder-by-zero wording — byte-identical to
/// `eval::value::eval_div_rem`'s `format!("{} by zero", ...)`.
pub fn div_zero_message(op: BinOp) -> String {
    format!(
        "{} by zero",
        if op == BinOp::Div {
            "division"
        } else {
            "remainder"
        }
    )
}

/// `<<`'s own lost-bits wording — byte-identical to
/// `eval::value::eval_shift`'s fixed string.
pub fn shift_lost_message() -> String {
    "`<<` lost nonzero high bits".to_string()
}

/// `.to[T]()`'s own out-of-range wording — byte-identical to
/// `eval::value::eval_to_scalar`'s `` format!("`.to[{}]()` conversion out
/// of range", render_type(target)) ``.
pub fn convert_abort_message(target: &Type) -> String {
    format!(
        "`.to[{}]()` conversion out of range",
        types::render_type(target)
    )
}

#[cfg(test)]
mod abort_message_tests {
    use super::*;
    use crate::eval::value::{self, Value};

    // plans/M5.md item B, task note 6: "one checked-op abort payload
    // wording match against the evaluator" — these assert `mwir`'s own
    // precomputed strings stay byte-identical to what `eval::value`
    // actually produces at the exact same failure, so the two can never
    // silently drift apart (CLAUDE.md: "dumb and locked").

    #[test]
    fn ordinary_overflow_wording_matches_the_evaluator() {
        let got = value::eval_ordinary(BinOp::Add, &Type::U8, &Value::U8(250), &Value::U8(10))
            .unwrap_err();
        assert_eq!(got, abort_message(BinOp::Add));
    }

    #[test]
    fn div_overflow_wording_matches_the_evaluator() {
        let got = value::eval_div_rem(
            BinOp::Div,
            &Type::I32,
            &Value::I32(i32::MIN),
            &Value::I32(-1),
        )
        .unwrap_err();
        assert_eq!(got, abort_message(BinOp::Div));
    }

    #[test]
    fn neg_overflow_wording_matches_the_evaluator() {
        let got = value::eval_neg(&Value::I8(i8::MIN)).unwrap_err();
        assert_eq!(got, neg_abort_message());
    }

    #[test]
    fn div_by_zero_wording_matches_the_evaluator() {
        let got = value::eval_div_rem(BinOp::Div, &Type::U32, &Value::U32(9), &Value::U32(0))
            .unwrap_err();
        assert_eq!(got, div_zero_message(BinOp::Div));
    }

    #[test]
    fn rem_by_zero_wording_matches_the_evaluator() {
        let got = value::eval_div_rem(BinOp::Rem, &Type::U32, &Value::U32(9), &Value::U32(0))
            .unwrap_err();
        assert_eq!(got, div_zero_message(BinOp::Rem));
    }

    #[test]
    fn shift_lost_bits_wording_matches_the_evaluator() {
        let got =
            value::eval_shift(BinOp::Shl, &Type::U8, &Value::U8(0xFF), &Value::U8(1)).unwrap_err();
        assert_eq!(got, shift_lost_message());
    }

    #[test]
    fn convert_out_of_range_wording_matches_the_evaluator() {
        let got = value::eval_to_scalar(&Type::U8, &Value::I32(-1)).unwrap_err();
        assert_eq!(got, convert_abort_message(&Type::U8));
    }
}

// --- aggregate layout (decision 3's dumb rule) --------------------------

/// Every plain (non-generic) struct's own field types, and every plain
/// enum's own variant payload types, in declaration order — the one
/// whole-program fact `size_of` needs beyond a bare `Type`
/// (this module's own doc comment explains why, and the disclosed
/// generic-instantiation gap).
#[derive(Debug, Clone, Default)]
pub struct LayoutCtx {
    pub structs: BTreeMap<String, Vec<Type>>,
    pub enums: BTreeMap<String, Vec<Vec<Type>>>,
    /// plans/M6.md item D: a plain struct's own field *names*, in the
    /// identical declaration order `structs`'s own field *types* already
    /// carry — added specifically so codegen can resolve
    /// `flowwir::FlowInst::SelfPath`'s own field-name chain (02-language.md
    /// §9.2's "the frame records the field path and re-derives it") down
    /// to a `Project`-style byte offset, the one fact neither `structs`
    /// (types only) nor the typed tree (`TypedStruct::fields`, gone by the
    /// time codegen runs) makes available together with a whole-program
    /// `LayoutCtx`. Populated alongside `structs` in `build_layout_ctx`,
    /// same source walk, so the two can never disagree on field count or
    /// order.
    pub struct_field_names: BTreeMap<String, Vec<String>>,
}

/// Builds one `LayoutCtx` from a raw module, exactly the way
/// `sema::dump`/`sema::check` recompute `specialize`/`declare` themselves
/// rather than threading extra state out of `check_typed` (this module's
/// own doc comment) — call once per module; only plain (non-generic)
/// top-level `struct`/`enum` declarations contribute (the disclosed
/// generic-instantiation gap above).
///
/// plans/M9.md item PP: when the module mentions a time-prelude name,
/// inject `Duration`/`Instant` arity 0 into the imported-type table the
/// same way `sema::check_typed` does via `inject_time_prelude_types`.
/// Without this, `build_layout_ctx(&module, empty)` is not a subset of
/// what `check_typed` just accepted (NN carry-out 2 / LL residue) — a
/// `: Duration` field fails declare here after the check path spliced
/// `core.time`.
pub fn build_layout_ctx(
    module: &crate::syntax::ast::Module,
    imported: &types::ImportedTypes,
) -> Result<LayoutCtx, crate::sema::SemaError> {
    use crate::sema::types::{DeclEnum, DeclItem, DeclMember, DeclStruct, DeclVariantPayload};

    let specialized = crate::sema::specialize::specialize(module)?;
    let mut imported = imported.clone();
    if crate::loader::module_mentions_time(module) {
        for name in ["Duration", "Instant"] {
            imported.entry(name.to_string()).or_insert(0);
        }
    }
    let items = types::declare_with_imports(&specialized, &imported)?;
    let mut ctx = LayoutCtx::default();
    for item in items {
        match item {
            DeclItem::Struct(DeclStruct { name, members, .. }) => {
                let field_names: Vec<String> = members
                    .iter()
                    .filter_map(|m| match m {
                        DeclMember::Field(f) => Some(f.name.clone()),
                        _ => None,
                    })
                    .collect();
                let fields: Vec<Type> = members
                    .into_iter()
                    .filter_map(|m| match m {
                        DeclMember::Field(f) => Some(f.ty),
                        _ => None,
                    })
                    .collect();
                ctx.struct_field_names.insert(name.clone(), field_names);
                ctx.structs.insert(name, fields);
            }
            DeclItem::Enum(DeclEnum { name, variants, .. }) => {
                let payloads: Vec<Vec<Type>> = variants
                    .into_iter()
                    .map(|v| match v.payload {
                        DeclVariantPayload::None => Vec::new(),
                        DeclVariantPayload::Tuple(tys) => tys,
                        DeclVariantPayload::Named(fields) => {
                            fields.into_iter().map(|(_, t)| t).collect()
                        }
                    })
                    .collect();
                ctx.enums.insert(name, payloads);
            }
            _ => {}
        }
    }
    Ok(ctx)
}

/// The one 8-byte-slot layout rule (module doc's own "Aggregate layout"
/// section). `Err` only for the disclosed generic-instantiation gap, or
/// an array whose length is neither a literal nor resolvable (mirrors
/// `lower::eval_array_len`'s own scope, duplicated rather than shared —
/// this fn takes no `TypedProgram`/evaluator at all, by design: a plain
/// module const's own value is not threaded in here, so a `const`-named
/// array length here is treated the same as `bodies::literal_array_len`
/// alone would: unresolvable, `Err`).
pub fn size_of(ty: &Type, ctx: &LayoutCtx) -> Result<usize, String> {
    const SLOT: usize = 8;
    match ty {
        Type::Bool
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
        | Type::Never => Ok(SLOT),
        Type::Array(elem, len_expr) => {
            let n = crate::sema::bodies::literal_array_len(len_expr).ok_or_else(|| {
                "array length is not a literal (unsupported by the layout fn)".to_string()
            })?;
            let n = usize::try_from(n).map_err(|_| "array length out of range".to_string())?;
            Ok(size_of(elem, ctx)? * n)
        }
        Type::Tuple(elems) => {
            let mut total = 0;
            for e in elems {
                total += size_of(e, ctx)?;
            }
            Ok(total)
        }
        Type::Option(inner) => Ok(SLOT + size_of(inner, ctx)?),
        Type::Result(ok, err) => Ok(SLOT + size_of(ok, ctx)?.max(size_of(err, ctx)?)),
        // plans/M7.md item E4 / decision 19: `own[P] T` is one 64-bit word
        // holding the guest address of a pool slot. The payload bytes live
        // in `pooldata`; the handle is authority over them, not a by-value
        // copy (which would either invent an allocator at publish time or
        // put device-reachable bytes in actor state). Field/method access
        // through an `own` loads via that address — see codegen's Own arms.
        Type::Own(_, _) => Ok(SLOT),
        Type::Static(inner) => size_of(inner, ctx),
        Type::Bytes(Some(len_expr)) => {
            let n = crate::sema::bodies::literal_array_len(len_expr).ok_or_else(|| {
                "Bytes length is not a literal (unsupported by the layout fn)".to_string()
            })?;
            Ok(SLOT * usize::try_from(n).map_err(|_| "Bytes length out of range".to_string())?)
        }
        // plans/M10.md item B4 / decisions 595–596: an unbounded `Bytes`
        // parameter is a two-word packed-byte handle `(base, len)` — not a
        // slot-per-byte value. Source cannot observe the address; only
        // indexing / console append consume it. 16 bytes, always.
        Type::Bytes(None) => Ok(SLOT * 2),
        // plans/M9.md item C1: `String[..N]` is one length word plus `N`
        // byte slots (each a SLOT, matching `Bytes[N]`'s slot-per-byte
        // convention). Rejected alternative: dense `align8(8+N)` packing
        // — smaller, but a second layout rule beside every other
        // aggregate's slot-stride Project/IndexGet path.
        Type::String(len_expr) => {
            let n = crate::sema::bodies::literal_array_len(len_expr).ok_or_else(|| {
                "String capacity is not a literal (unsupported by the layout fn)".to_string()
            })?;
            if !crate::sema::bodies::string_capacity_fits(n) {
                return Err("String capacity out of range".to_string());
            }
            let n = usize::try_from(n).map_err(|_| "String capacity out of range".to_string())?;
            Ok(SLOT
                .checked_mul(1 + n)
                .ok_or_else(|| "String capacity out of range".to_string())?)
        }
        Type::Fn(_, _) => Err("sizing a `fn` value type is not implemented yet".to_string()),
        Type::Generic(_) => {
            Err("sizing a bare generic parameter is not implemented yet".to_string())
        }
        Type::Str => Err("sizing a bare `Str` (unbounded) has no static size".to_string()),
        // plans/M6.md item D: the M6 builtin-pseudo-type vehicle (`Actor`/
        // `Group`/`Instant`/`Duration`/`Admission`/`Peer`/`Rejected` —
        // `sema::types::resolve_named`'s own recognized names, none of
        // which is ever a real `DeclStruct`/`DeclEnum` entry in this
        // `LayoutCtx`, so the general `Named` path below can never size
        // one). Every one of these is carried, at this milestone, as a
        // small opaque handle/tick-count/reason code — one 8-byte slot,
        // uniformly (`Actor[T]`'s own runtime value is a build-time
        // constant index per 04-compiler.md §6's "Actor as-if" license;
        // `Instant`/`Duration` are opaque `u64` tick counts,
        // `flowwir::FlowInst::Now`/`Duration`'s own doc comments;
        // `Admission`/`Peer`/`Rejected` are opaque builtin payload types
        // not yet grown real fields, `sema::bodies`'s own doc comment on
        // `CallError`'s `NotAdmitted`/`PeerFailed` variants). `CallError[E]`
        // (the one non-empty-`targs` pseudo-type, 02 §9.4's own five-variant
        // composition, `sema::bodies::compose_call_error`) sizes like any
        // other builtin sum: one tag slot plus the widest variant's own
        // payload — `Op(E)` (up to `size_of(E)`) vs. every other variant's
        // own opaque-handle-or-nothing payload (at most one `SLOT`), so
        // `SLOT + size_of(E).max(SLOT)` is exact for the whole real
        // variant set without re-deriving it here a second time.
        // plans/M7.md item H1, decision 11: 03-hardware.md §1's
        // capabilities and §9's seven bring-up states join the same
        // vehicle, for the same reason and with the same width — every one
        // of them is **one 64-bit word holding a guest base address**
        // (`DeviceCap[D]`/every state: the device's own declared register
        // window; `Mmio[L]`: the same base, with the register's declared
        // `@offset` supplied by the layout; `DmaPool[P, N]`: its pool's
        // backing base). This is the one-line addition plans/M7.md
        // decision 10 named as H1's third prerequisite: without it a
        // `DeviceCap[D]`-taking driver could not reach codegen at all,
        // because the general `Named` path below refuses any type argument.
        Type::Named(name, _targs)
            if matches!(
                name.as_str(),
                "Actor"
                    | "Group"
                    | "Instant"
                    | "Duration"
                    | "Admission"
                    | "Peer"
                    | "Rejected"
                    // plans/M7.md item G, decision 17: one 64-bit word in
                    // driver state (the cell's value; ops address the live
                    // word at `self_ptr + field_off`, never a side table).
                    | "InterruptCell"
            ) || crate::eval::image_checks::is_sealed_authority_type_name(name) =>
        {
            // `Actor[T]`/`Rejected[T]` (if ever instantiated) carry their
            // own type argument purely for the type-checker's sake — the
            // argument itself never contributes a byte here (module doc
            // above).
            Ok(SLOT)
        }
        // plans/M7.md item H2a, 03-hardware.md §8: `Untrusted[T]` is a
        // sealed newtype over `T` — one mechanism, no extra fields
        // (archive 05 §8: "adds no field beyond what the check
        // requires"). Sized exactly as its payload.
        Type::Named(name, targs) if name == "Untrusted" => {
            let Some(crate::sema::types::TypeArg::Type(inner)) = targs.first() else {
                return Err("`Untrusted` with no payload type argument".to_string());
            };
            size_of(inner, ctx)
        }
        // plans/M7.md item E4: `IoCompletion[P]` = payload + status +
        // written_len. Field order is load-bearing for Project indices:
        // 0 = payload (`P`), 1 = status (`Result[unit, IoError]`),
        // 2 = written_len (`Untrusted[usize]`).
        Type::Named(name, targs) if name == "IoCompletion" => {
            let Some(crate::sema::types::TypeArg::Type(payload)) = targs.first() else {
                return Err("`IoCompletion` with no payload type argument".to_string());
            };
            let status = Type::Result(
                Box::new(Type::Unit),
                Box::new(Type::Named("IoError".to_string(), vec![])),
            );
            let written = Type::Named(
                "Untrusted".to_string(),
                vec![crate::sema::types::TypeArg::Type(Type::Usize)],
            );
            Ok(size_of(payload, ctx)? + size_of(&status, ctx)? + size_of(&written, ctx)?)
        }
        Type::Named(name, targs) if name == "CallError" => {
            let Some(crate::sema::types::TypeArg::Type(e_ty)) = targs.first() else {
                return Err("`CallError` with no error type argument".to_string());
            };
            Ok(SLOT + size_of(e_ty, ctx)?.max(SLOT))
        }
        // plans/M7.md item E1: `BootError` is a prelude enum (one unit
        // variant), never a DeclEnum in this LayoutCtx — same vehicle as
        // Target/Restart for the builder. Tag only.
        Type::Named(name, targs)
            if targs.is_empty()
                && matches!(
                    name.as_str(),
                    // plans/M8.md item G: 03-hardware.md §9's
                    // `CompletionOutcome` — three fieldless variants, so
                    // the tag word is the whole value.
                    "BootError" | "Target" | "Restart" | "IoError" | "CompletionOutcome"
                ) =>
        {
            Ok(SLOT)
        }
        Type::Named(name, targs) => {
            // plans/M7.md item G, decision 18: an instantiated generic
            // (`BlkDriver[DriverMode.Irq]`) is keyed in `LayoutCtx` by its
            // rendered type spelling — populated from
            // `TypedProgram::instantiations` before codegen/layout.
            let key = if targs.is_empty() {
                name.clone()
            } else {
                crate::sema::types::render_type(&Type::Named(name.clone(), targs.clone()))
            };
            if let Some(fields) = ctx.structs.get(&key) {
                let mut total = 0;
                for f in fields {
                    total += size_of(f, ctx)?;
                }
                return Ok(total);
            }
            if !targs.is_empty() {
                return Err(format!(
                    "sizing an instantiated generic struct/enum `{key}` is not in this layout \
                     context (no matching TypedProgram instantiation)"
                ));
            }
            if let Some(variants) = ctx.enums.get(name) {
                let mut widest = 0usize;
                for payload in variants {
                    let mut total = 0;
                    for f in payload {
                        total += size_of(f, ctx)?;
                    }
                    widest = widest.max(total);
                }
                return Ok(SLOT + widest);
            }
            Err(format!(
                "unknown struct/enum `{name}` in this layout context"
            ))
        }
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn scalars_are_one_eight_byte_slot() {
        let ctx = LayoutCtx::default();
        assert_eq!(size_of(&Type::U8, &ctx), Ok(8));
        assert_eq!(size_of(&Type::I64, &ctx), Ok(8));
        assert_eq!(size_of(&Type::Bool, &ctx), Ok(8));
        assert_eq!(size_of(&Type::Unit, &ctx), Ok(8));
    }

    /// plans/M7.md item H2a: `Untrusted[T]` is a transparent newtype —
    /// sized exactly as its payload, no extra tag or field.
    #[test]
    fn untrusted_is_sized_as_its_payload() {
        use crate::sema::types::TypeArg;
        let ctx = LayoutCtx::default();
        let u = Type::Named("Untrusted".to_string(), vec![TypeArg::Type(Type::Usize)]);
        assert_eq!(size_of(&u, &ctx), Ok(8));
        let u32_u = Type::Named("Untrusted".to_string(), vec![TypeArg::Type(Type::U32)]);
        assert_eq!(size_of(&u32_u, &ctx), size_of(&Type::U32, &ctx));
    }

    #[test]
    fn array_size_is_element_stride_times_len() {
        let ctx = LayoutCtx::default();
        let arr = Type::Array(
            Box::new(Type::U8),
            Box::new(crate::syntax::ast::Expr::Int(
                crate::syntax::ast::Span::default(),
                "5".to_string(),
            )),
        );
        assert_eq!(size_of(&arr, &ctx), Ok(8 * 5));
    }

    /// plans/M9.md item C1: `String[..N]` is one length word + `N` byte slots.
    #[test]
    fn string_bound_size_is_length_word_plus_n_byte_slots() {
        let ctx = LayoutCtx::default();
        let s = Type::String(Box::new(crate::syntax::ast::Expr::Int(
            crate::syntax::ast::Span::default(),
            "8".to_string(),
        )));
        assert_eq!(size_of(&s, &ctx), Ok(8 * (1 + 8)));
    }

    #[test]
    fn tuple_size_is_the_sum_of_components() {
        let ctx = LayoutCtx::default();
        let t = Type::Tuple(vec![Type::U8, Type::U64, Type::Bool]);
        assert_eq!(size_of(&t, &ctx), Ok(8 * 3));
    }

    #[test]
    fn struct_size_is_the_sum_of_field_sizes_in_declaration_order() {
        let mut ctx = LayoutCtx::default();
        ctx.structs.insert(
            "Point".to_string(),
            vec![
                Type::U64,
                Type::U64,
                Type::Array(Box::new(Type::U8), Box::new(dummy_int(2))),
            ],
        );
        let t = Type::Named("Point".to_string(), vec![]);
        // 8 (x) + 8 (y) + (8*2) (a 2-element u8 array) = 32.
        assert_eq!(size_of(&t, &ctx), Ok(8 + 8 + 16));
    }

    #[test]
    fn enum_size_is_tag_plus_the_widest_variant() {
        let mut ctx = LayoutCtx::default();
        ctx.enums.insert(
            "Shape".to_string(),
            vec![vec![Type::U64], vec![Type::U64, Type::U64]],
        );
        let t = Type::Named("Shape".to_string(), vec![]);
        // 8 (tag) + max(8, 16) = 24.
        assert_eq!(size_of(&t, &ctx), Ok(8 + 16));
    }

    #[test]
    fn option_size_is_tag_plus_the_inner_type() {
        let ctx = LayoutCtx::default();
        let t = Type::Option(Box::new(Type::U64));
        assert_eq!(size_of(&t, &ctx), Ok(8 + 8));
    }

    // plans/M10.md item B4 / decision 595: unbounded `Bytes` is a
    // two-word (base, len) handle.
    #[test]
    fn bare_bytes_handle_is_two_words() {
        let ctx = LayoutCtx::default();
        assert_eq!(size_of(&Type::Bytes(None), &ctx), Ok(16));
    }

    #[test]
    fn instantiated_generic_struct_fails_closed() {
        let ctx = LayoutCtx::default();
        let t = Type::Named(
            "struct:Box[u64]".to_string(),
            vec![crate::sema::types::TypeArg::Type(Type::U64)],
        );
        assert!(size_of(&t, &ctx).is_err());
    }

    fn dummy_int(n: i128) -> crate::syntax::ast::Expr {
        crate::syntax::ast::Expr::Int(crate::syntax::ast::Span::default(), n.to_string())
    }
}

// --- the `--stage=mwir` dump (M1 dump style: `Kind key=value`, two-space
// indent per nesting level) ------------------------------------------------

pub fn dump(program: &MwirProgram) -> String {
    let mut out = String::new();
    out.push_str("Program\n");
    for (key, f) in &program.fns {
        let mut header = format!(
            "Fn key={key} ret={} temps={}",
            types::render_type(&f.ret),
            f.temp_count()
        );
        if let Some((t, mode)) = &f.receiver {
            let _ = write!(header, " receiver={t}:{}", mode.as_str());
        }
        if !f.params.is_empty() {
            let ps: Vec<String> = f
                .params
                .iter()
                .map(|(t, mode)| {
                    if *mode == crate::syntax::ast::AccessMode::Read {
                        t.to_string()
                    } else {
                        format!("{t}:{}", mode.as_str())
                    }
                })
                .collect();
            let _ = write!(header, " params=[{}]", ps.join(","));
        }
        push_line(&mut out, 1, &header);
        for (i, ty) in f.temp_types.iter().enumerate() {
            push_line(
                &mut out,
                2,
                &format!("Temp t{i} ty={}", types::render_type(ty)),
            );
        }
        push_line(&mut out, 2, "Body");
        for (i, inst) in f.body.iter().enumerate() {
            let line = format!("{i:04}: {}", fmt_inst(inst));
            push_line(&mut out, 3, &line);
        }
    }
    if !program.rodata.is_empty() {
        push_line(&mut out, 1, "Rodata");
        for (i, bytes) in program.rodata.iter().enumerate() {
            push_line(&mut out, 2, &format!("{i}: {}", render_bytes(bytes)));
        }
    }
    out
}

fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

/// Renders `rodata`'s own bytes as a lossy UTF-8 string, `\`-escaping the
/// two characters (`\`, newline) that would otherwise break the dump's
/// own one-line-per-entry grammar — good enough for a review-visible
/// text dump (never round-tripped back into bytes by anything in this
/// compiler).
fn render_bytes(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// Plans/M6.md item B: bumped from private to `pub(crate)` so
/// `flowwir.rs`'s own `--stage=flowwir` dump can format an embedded
/// `mwir::Inst` (decision: FlowWir embeds `mwir::Inst` directly for its
/// non-suspending ops rather than inventing a parallel formatter) —
/// logic unchanged, this is purely a visibility bump for reuse.
pub(crate) fn fmt_inst(inst: &Inst) -> String {
    match inst {
        Inst::ConstInt { dst, ty, value } => {
            format!(
                "ConstInt dst={dst} ty={} value={value}",
                types::render_type(ty)
            )
        }
        Inst::ConstBool { dst, value } => format!("ConstBool dst={dst} value={value}"),
        Inst::ConstFloat { dst, ty, bits } => {
            format!(
                "ConstFloat dst={dst} ty={} bits={bits}",
                types::render_type(ty)
            )
        }
        Inst::ConstChar { dst, value } => format!("ConstChar dst={dst} value={value:?}"),
        Inst::ConstUnit { dst } => format!("ConstUnit dst={dst}"),
        Inst::ConstText { dst, data } => format!("ConstText dst={dst} data={data}"),
        Inst::Copy { dst, src } => format!("Copy dst={dst} src={src}"),
        Inst::MmioRead {
            dst,
            base,
            offset,
            ty,
        } => format!(
            "MmioRead dst={dst} base={base} offset={offset:#x} ty={}",
            types::render_type(ty)
        ),
        Inst::MmioWrite {
            base,
            offset,
            ty,
            value,
        } => format!(
            "MmioWrite base={base} offset={offset:#x} ty={} value={value}",
            types::render_type(ty)
        ),
        Inst::PlacedIndexGet {
            dst,
            base,
            field_offset,
            index,
            len,
            elem_stride,
            ty,
        } => format!(
            "PlacedIndexGet dst={dst} base={base} field_offset={field_offset:#x} \
             index={index} len={len} elem_stride={elem_stride} ty={}",
            types::render_type(ty)
        ),
        Inst::PlacedIndexSet {
            base,
            field_offset,
            index,
            value,
            len,
            elem_stride,
            ty,
        } => format!(
            "PlacedIndexSet base={base} field_offset={field_offset:#x} index={index} \
             value={value} len={len} elem_stride={elem_stride} ty={}",
            types::render_type(ty)
        ),
        Inst::BytesIndexGet { dst, base, index } => {
            format!("BytesIndexGet dst={dst} base={base} index={index}")
        },
        Inst::QueuePrepare {
            dst,
            queue,
            permit,
            header,
            payload,
            status,
            device_writes,
            payload_len,
        } => format!(
            "QueuePrepare dst={dst} queue={queue} permit={permit} header={header} \
             payload={payload} status={status} device_writes={device_writes} \
             payload_len={payload_len}"
        ),
        Inst::QueuePublish {
            dst,
            queue,
            operation,
            steps,
        } => format!(
            "QueuePublish dst={dst} queue={queue} operation={operation} order=[{}]",
            steps.join(", ")
        ),
        Inst::QueueDrain { queue, max } => {
            format!("QueueDrain queue={queue} max={max}")
        }
        Inst::QueueSuppressInterrupts { queue } => {
            format!("QueueSuppressInterrupts queue={queue}")
        }
        Inst::QueueClaim {
            dst,
            queue,
            receipt,
        } => {
            format!("QueueClaim dst={dst} queue={queue} receipt={receipt}")
        }
        Inst::QueueRecover {
            dst,
            queue,
            receipt,
        } => {
            format!("QueueRecover dst={dst} queue={queue} receipt={receipt}")
        }
        Inst::QueueReclaim { dst, queue } => {
            format!("QueueReclaim dst={dst} queue={queue}")
        }
        Inst::DeviceReset { dst, device, queue } => {
            format!("DeviceReset dst={dst} device={device} queue={queue}")
        }
        Inst::LoadIrqVector { dst, driver } => {
            format!("LoadIrqVector dst={dst} driver={driver}")
        }
        Inst::InterruptCellLoadAcquire {
            dst,
            field_off,
            width,
        } => format!("InterruptCellLoadAcquire dst={dst} field_off={field_off} width={width}"),
        Inst::InterruptCellStoreRelease {
            field_off,
            width,
            value,
        } => format!("InterruptCellStoreRelease field_off={field_off} width={width} value={value}"),
        Inst::InterruptCellSwapAcquire {
            dst,
            field_off,
            width,
            value,
        } => format!(
            "InterruptCellSwapAcquire dst={dst} field_off={field_off} width={width} value={value}"
        ),
        Inst::InterruptCellFetchOrRelease {
            dst,
            field_off,
            width,
            value,
        } => format!(
            "InterruptCellFetchOrRelease dst={dst} field_off={field_off} width={width} value={value}"
        ),
        Inst::Wake { driver } => format!("Wake driver={driver}"),
        Inst::MakeAggregate { dst, elems } => {
            format!("MakeAggregate dst={dst} elems=[{}]", join_temps(elems))
        }
        Inst::FormatScalar {
            dst,
            src,
            src_ty,
            capacity,
        } => {
            format!(
                "FormatScalar dst={dst} src={src} src_ty={} capacity={capacity}",
                crate::sema::types::render_type(src_ty)
            )
        }
        Inst::StringConcat {
            dst,
            lhs,
            rhs,
            lhs_cap,
            rhs_cap,
        } => {
            format!(
                "StringConcat dst={dst} lhs={lhs} rhs={rhs} lhs_cap={lhs_cap} rhs_cap={rhs_cap}"
            )
        }
        Inst::Project { dst, base, index } => {
            format!("Project dst={dst} base={base} index={index}")
        }
        Inst::SetField { base, index, value } => {
            format!("SetField base={base} index={index} value={value}")
        }
        Inst::IndexGet {
            dst,
            base,
            index,
            len,
        } => {
            format!("IndexGet dst={dst} base={base} index={index} len={len}")
        }
        Inst::IndexSet {
            base,
            index,
            value,
            len,
        } => {
            format!("IndexSet base={base} index={index} value={value} len={len}")
        }
        Inst::MakeEnum { dst, tag, payload } => {
            format!(
                "MakeEnum dst={dst} tag={tag} payload=[{}]",
                join_temps(payload)
            )
        }
        Inst::EnumTag { dst, src } => format!("EnumTag dst={dst} src={src}"),
        Inst::EnumPayload { dst, src, index } => {
            format!("EnumPayload dst={dst} src={src} index={index}")
        }
        Inst::ArithChecked {
            dst,
            op,
            ty,
            lhs,
            rhs,
            abort,
        } => format!(
            "ArithChecked op={} ty={} dst={dst} lhs={lhs} rhs={rhs} abort={abort:?}",
            op.as_str(),
            types::render_type(ty)
        ),
        Inst::ArithWrapping {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => format!(
            "ArithWrapping op={} ty={} dst={dst} lhs={lhs} rhs={rhs}",
            op.as_str(),
            types::render_type(ty)
        ),
        Inst::DivRem {
            dst,
            op,
            ty,
            lhs,
            rhs,
            abort_zero,
            abort_overflow,
        } => format!(
            "DivRem op={} ty={} dst={dst} lhs={lhs} rhs={rhs} abort_zero={abort_zero:?} abort_overflow={abort_overflow:?}",
            op.as_str(),
            types::render_type(ty)
        ),
        Inst::Shift {
            dst,
            op,
            ty,
            lhs,
            rhs,
            bits,
            lost,
        } => {
            let mut s = format!(
                "Shift op={} ty={} dst={dst} lhs={lhs} rhs={rhs} bits={bits}",
                op.as_str(),
                types::render_type(ty)
            );
            if let Some(l) = lost {
                let _ = write!(s, " lost={l:?}");
            }
            s
        }
        Inst::Bitwise {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => format!(
            "Bitwise op={} ty={} dst={dst} lhs={lhs} rhs={rhs}",
            op.as_str(),
            types::render_type(ty)
        ),
        Inst::Compare {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => format!(
            "Compare op={} ty={} dst={dst} lhs={lhs} rhs={rhs}",
            op.as_str(),
            types::render_type(ty)
        ),
        Inst::Convert {
            dst,
            ty,
            src,
            abort,
        } => format!(
            "Convert ty={} dst={dst} src={src} abort={abort:?}",
            types::render_type(ty)
        ),
        Inst::Neg {
            dst,
            ty,
            src,
            abort,
        } => format!(
            "Neg ty={} dst={dst} src={src} abort={abort:?}",
            types::render_type(ty)
        ),
        Inst::BitNot { dst, ty, src } => {
            format!("BitNot ty={} dst={dst} src={src}", types::render_type(ty))
        }
        Inst::Not { dst, src } => format!("Not dst={dst} src={src}"),
        Inst::BoolAnd { dst, lhs, rhs } => format!("BoolAnd dst={dst} lhs={lhs} rhs={rhs}"),
        Inst::Jump { target } => format!("Jump target={target:04}"),
        Inst::JumpIfFalse { cond, target } => {
            format!("JumpIfFalse cond={cond} target={target:04}")
        }
        Inst::Call {
            dst,
            write_backs,
            key,
            args,
        } => {
            let mut s = format!("Call key={key} dst={dst} args=[{}]", join_temps(args));
            if !write_backs.is_empty() {
                let parts: Vec<String> = write_backs
                    .iter()
                    .map(|(i, t)| format!("{i}:{t}"))
                    .collect();
                let _ = write!(s, " write_backs=[{}]", parts.join(","));
            }
            s
        }
        Inst::Return { value } => match value {
            Some(v) => format!("Return value={v}"),
            None => "Return".to_string(),
        },
        Inst::AssertFail { message } => match message {
            Some(m) => format!("AssertFail message={m:?}"),
            None => "AssertFail".to_string(),
        },
    }
}

fn join_temps(ts: &[Temp]) -> String {
    ts.iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(",")
}
