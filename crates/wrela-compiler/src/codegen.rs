//! Codegen (plans/M5.md item C): `mwir::MwirProgram` -> per-fn A76 machine
//! code, with a stable `--stage=asm` dump. Consumes `mwir.rs`'s instruction
//! set and `encode.rs`'s pure `typed-args -> u32` encoder; never modifies
//! either (CLAUDE.md's "consume, don't touch" rule for this item).
//!
//! ## Two-pass emission (decision: the plan's own "size everything, then
//! encode with resolved offsets" option, not backpatching)
//!
//! Per fn: pass 1 computes the frame (every temp's byte offset,
//! `build_frame`) and then re-uses the *identical* per-instruction
//! emission function once with an all-zero jump-target table purely to
//! *count* how many machine words each `mwir::Inst` expands to (branch
//! word-count never depends on the *value* of a jump target — only a
//! genuinely out-of-range branch distance could, and this milestone's
//! functions are far too small for that to ever bite, so no dependency
//! exists between the two passes' word counts). That count list is
//! prefix-summed into `word_offsets[mwir_idx] -> starting word index`,
//! including one sentinel entry one past the last instruction (the
//! shared epilogue's own start word, the target every `Return`
//! branches to). Pass 2 re-runs the same emission function with the now-
//! known table, so every local `Jump`/`JumpIfFalse` resolves to a real
//! PC-relative `B`/`CBZ` immediate. Rodata interning happens in both
//! passes but is idempotent (content-addressed `BTreeMap`, plans/M5.md
//! item C's own "BTreeMap dedup" requirement) so re-running it twice is
//! harmless, not double-counted.
//!
//! Every compile-time integer constant (bounds, tag values, shift
//! widths, array lengths, element strides, ...) is always materialized
//! via `load_imm` — `MOVZ` + three unconditional `MOVK`s, exactly four
//! words, regardless of the value's own magnitude. This is deliberately
//! *not* optimized to skip zero halfwords or to use an immediate-form
//! instruction when the constant happens to be small: a per-instruction
//! word count that depended on the constant's own bit pattern would
//! still be perfectly *sound* (compile-time constants are known in both
//! passes identically) but the uniform form is simply dumber, and dumb
//! is the point (decision 4).
//!
//! Two things are *not* resolved at this stage, staying symbolic
//! (plans/M5.md item C, "Text/rodata" — the dump shows `rodata+0xNN`):
//! a `Call`'s own target function, and every rodata reference (`ADRP`+
//! `ADD` pair). Both are emitted with a placeholder `#0` immediate plus
//! a `Reloc` entry recording what item D's layout pass must patch in
//! once the whole program is laid out into one flat blob (each
//! function's own code doesn't yet know where any *other* function, or
//! the rodata section, ends up). `Reloc::Call{word,key}` names the word
//! index of the `BL` and the callee's own `MwirProgram::fns` key;
//! `Reloc::Rodata{word_adrp,byte_offset}` names the `ADRP`'s own word
//! index (the paired `ADD` always immediately follows, `word_adrp+1`)
//! and the byte offset the referenced entry starts at *within the
//! rodata section* (already computable now: rodata entries are
//! sequential and their own sizes are already fixed, only the
//! section's absolute base address is still unknown). `Reloc::Abort*`
//! similarly name a `BL`'s own word index for the two abort symbols
//! below. Local control flow (`Jump`/`JumpIfFalse`, and the call site's
//! own post-call bookkeeping) never needs a reloc: it resolves inside
//! this same pass.
//!
//! ## Frame layout (decision 4: spill-everything, fixed frame)
//!
//! ```text
//! [sp+0                 .. temps_end)   one region per mwir Temp, in
//!                                       temp-number order; temp `t`
//!                                       occupies [offset(t), offset(t)
//!                                       + mwir::size_of(temp_types[t])),
//!                                       every size already a multiple
//!                                       of 8 (mwir's own "no packing,
//!                                       always an 8-byte-slot-multiple"
//!                                       rule) so no alignment padding
//!                                       is ever needed between temps.
//! [temps_end            .. +8)          self_ptr_save -- present iff
//!                                       this fn has a receiver: the
//!                                       incoming aggregate pointer for
//!                                       `self`, saved once at entry so
//!                                       the epilogue can still find it
//!                                       after arbitrary intervening
//!                                       calls have clobbered x0..x8.
//! [..                   .. +8)          ret_ptr_save -- present iff
//!                                       this fn's own return type is
//!                                       an aggregate: the incoming x8
//!                                       result pointer, saved for the
//!                                       same reason.
//! [frame_size-8         .. frame_size)  saved x30 (lr) -- the only
//!                                       register this ABI preserves
//!                                       across a call (see below).
//! ```
//! `frame_size` is `temps_end + (8 if receiver) + (8 if aggregate ret)
//! + 8`, rounded up to 16 (SP stays 16-byte aligned, AAPCS64's own
//! requirement, harmless to keep even though nothing here calls out to
//! real AAPCS64 code). Every offset is SP-relative and always
//! *unsigned* (`encode.rs`'s `LDR`/`STR` immediate forms are unsigned-
//! offset-only) — the frame is built low-to-high specifically so no
//! offset is ever negative. Frames whose rounded size would exceed 4095
//! bytes fail closed (`CodegenError::unimplemented`, "large frames"
//! below): every fn in the required golden corpus fits in a few hundred
//! bytes at most, and staying within one `ADD`/`SUB`-immediate's own
//! `imm12` range means the prologue/epilogue never need a second
//! adjustment instruction — one more place dumbness is free here.
//!
//! **x29 (fp) is never touched by this ABI.** This is an internal-only
//! convention (CLAUDE.md, "no FFI exists") with no unwinder and no
//! debugger to satisfy; the only register that must survive a `BL`
//! (which clobbers `x30`) is the return address itself, so only `x30`
//! is saved/restored. Scratch computation always uses `x9`-`x14`
//! (never overlapping the `x0`-`x8` call-argument/result registers,
//! and never `x29`/`x30`/`sp`).
//!
//! ## Calling convention (decision 4, AAPCS64-*shaped*, not AAPCS64)
//!
//! Scalar arguments/results travel in registers by *value*: `x0..x7`
//! for up to 8 arguments (receiver first, when present, exactly
//! mirroring `mwir::Inst::Call::args`'s own "receiver first" order —
//! more than 8 total arguments fails closed, named
//! `CodegenError::unimplemented`, "more than 8 call arguments"; nothing
//! in the required golden corpus needs stack args, so none are
//! implemented), a scalar result in `x0`.
//!
//! **Every aggregate argument or result travels as a bare pointer to
//! the *caller's own* temp slot — never a defensive scratch copy.**
//! This is safe with no extra bookkeeping because of one invariant this
//! module maintains everywhere: a callee, at its own entry, always
//! copies an incoming aggregate's bytes *into its own local frame slot*
//! for that receiver/parameter (word-by-word, `size_of`-many words,
//! fully unrolled) and thereafter only ever reads/writes that local
//! copy — so aliasing the caller's own memory is harmless (the callee
//! never mutates through the incoming pointer *except* at its own
//! epilogue, and only for a `mut`/`init` receiver, see below). A
//! scalar result in an aggregate-returning fn works the mirror way: the
//! caller passes `x8 = &(dst temp's own slot)` *before* the call, and
//! the callee writes its result directly into that memory at every
//! `Return` — no post-call copy-back exists because there was never a
//! copy to begin with.
//!
//! **`self_write_back`/`mut self`, worked out from first principles
//! (mwir's own doc names the requirement, this module supplies the
//! mechanism):** a receiver is *always* an aggregate (every struct/enum
//! `self` has a `Type::Named`), so it is *always* passed by the same
//! bare-pointer rule above — this holds regardless of the receiver's
//! own declared `AccessMode`, and regardless of `Inst::Call::
//! self_write_back`. The callee's own prologue always copies the
//! incoming self bytes into its own local `receiver` temp slot (so its
//! body's `Project`/`SetField` instructions operate on an ordinary
//! local aggregate, same as any other temp); the callee's own
//! *epilogue* additionally copies that local slot's *current* bytes
//! back out through the *original* incoming pointer (saved in
//! `self_ptr_save` at entry) — but only when `MwirFn::receiver`'s own
//! mode is `Mut` (`init` included: `sema::bodies` already types `init`'s
//! receiver `Mut`, mirrored uniformly, no separate "is this init" case
//! anywhere in this module). Since the pointer the caller passed *is*
//! the address of its own `args[0]` temp slot, this write-back lands
//! exactly where `self_write_back`'s own field name promises, with the
//! call site itself doing nothing special at all — **`self_write_back`
//! is read nowhere in this module**; it is fully, automatically
//! satisfied by the uniform bare-pointer rule plus the callee's own
//! mode-driven epilogue, and this paragraph is that fact's proof, not
//! an aspiration. (A `Read`/`Take` receiver's callee never runs that
//! epilogue step, so the caller's aliasing is inert there too — no
//! mutation ever happens through the pointer in that case.)
//!
//! ## The abort contract (item E's exact obligation)
//!
//! Every checked operation that can fail branches to one of two
//! `noreturn` stub symbols, called via `BL` with a placeholder target
//! (`Reloc::AbortFixed`/`Reloc::AbortVal`) exactly like an ordinary
//! call — never inlined, never returned from.
//!
//! - **`__wrela_abort(x0: msg_ptr, x1: msg_len) -> noreturn`** — every
//!   abort whose *entire* message is fixed at compile time (every
//!   ordinary/wrapping-overflow, div/rem-by-zero, div `MIN/-1`,
//!   negation-overflow, `.to[T]()` out-of-range, `<<` lost-bits, and
//!   `assert`/`panic`/match-fallthrough message). `msg_ptr` is a byte
//!   offset into the rodata section (an unresolved `ADRP`+`ADD` pair at
//!   this stage, `Reloc::Rodata`); `msg_len` is the message's own byte
//!   length, an ordinary immediate (`load_imm`).
//! - **`__wrela_abort_val(x0: prefix_ptr, x1: prefix_len, x2: value,
//!   x3: value_signed, x4: suffix_ptr, x5: suffix_len) -> noreturn`** —
//!   the two messages whose own wording embeds a *runtime* value
//!   (`eval::value::eval_shift`'s `"shift count {c} is out of range for
//!   a {bits}-bit type"`, `eval::interp`'s `"index {i} out of bounds
//!   (length {len})"`). `prefix`/`suffix` are the fixed text either side
//!   of the interpolated value (`"shift count "` / `" is out of range
//!   for a {bits}-bit type"` with `bits` already baked in as a compile-
//!   time constant; `"index "` / `" out of bounds (length {len})"` with
//!   `len` baked in the same way) — both rodata refs, same as above.
//!   `value` is the operand's own live register value, already in this
//!   module's canonical 64-bit sign/zero-extended form (see below);
//!   `value_signed` (`0`/`1`) tells the runtime whether to render it as
//!   a signed or unsigned decimal (only the shift-count case can ever be
//!   negative — an index is always `usize`). Item E is expected to
//!   print `prefix`, then `value` as a decimal (per `value_signed`),
//!   then `suffix`, then abandon the running test — the exact same
//!   text `eval::value`/`eval::interp` would have produced for the
//!   identical failure.
//!
//! `BL` (not a bare `B`) is used for both, uniformly with every ordinary
//! call, even though neither ever returns — one call-emission shape,
//! no special case for "this call happens to be a noreturn one".
//!
//! ## The canonical-slot invariant (what makes every op this simple)
//!
//! Every scalar temp's 8-byte slot always holds its value's *signed*
//! 64-bit representation: an unsigned type's value zero-extended, a
//! signed type's value sign-extended. This one invariant is why:
//! `Compare` can always use a *signed* 64-bit `CMP`/`CSET` regardless of
//! the operand's own declared signedness (an unsigned value's zero-
//! extended form is always non-negative as a signed i64, so signed and
//! unsigned orderings coincide exactly on it); bitwise `& | ^`/`~` never
//! need extra truncation (two's-complement sign-extension commutes with
//! bitwise ops bit-for-bit); narrow (`<64`-bit) checked `+ - *` can
//! compute at full 64-bit width and simply *bounds-check the raw
//! result* against the target type's own `[min,max]` (64 bits is always
//! wide enough that the true, unwrapped sum/difference/product of two
//! sign/zero-extended-from-`<64`-bit operands never itself overflows 64
//! bits — this is the exact same trick `eval::value` plays with `i128`,
//! one register width narrower); and narrow checked `/` needs no bounds
//! check at all beyond the explicit signed `MIN/-1` pre-check (see
//! below). `narrow_to_width` (this module) is the one helper that
//! *restores* the invariant after an op whose raw 64-bit result is
//! *not* automatically canonical: `ArithWrapping` (truncate-and-
//! optionally-resign to the type's own width — the modulo-2^width
//! reduction `eval_wrapping` performs), `BitNot` (flipping a sign/zero-
//! extended register's *extension* bits too, which must be cleared/
//! re-signed afterward), `Shift`'s `Shl` result, and a successful
//! `Convert`. It is `LSL #(64-bits)` then `LSR #(64-bits)` (unsigned) or
//! `ASR #(64-bits)` (signed) — a no-op when `bits == 64`.
//!
//! ## Overflow detection per op class (decision 4's own required list)
//!
//! - **`+ - *`, width `< 64`**: compute at 64-bit width (safe, per the
//!   invariant above), then `CMP` the raw result against the target
//!   type's own `[min,max]` (both materialized via `load_imm`, compared
//!   signed) and branch to `__wrela_abort` on either bound failing.
//! - **`+ -`, width `== 64`**: `ADDS`/`SUBS` and read the flags —
//!   `Cond::Vs` (signed overflow, both ops) for `I64`/`Isize`;
//!   `Cond::Cs` (unsigned carry, `ADD`) / `Cond::Cc` (unsigned borrow,
//!   `SUB`) for `U64`/`Usize`.
//! - **`*`, width `== 64`**: `MUL` (low 64 bits) plus `SMULH`/`UMULH`
//!   (high 64 bits — decision 4's own "smulh compare", the reason
//!   `encode.rs` gains these two functions, see the module-level note
//!   below). Unsigned: overflow iff the high word is nonzero. Signed:
//!   overflow iff the high word isn't the low word's own sign-extension
//!   (`ASR #63` of the low word) — the standard two-register-product
//!   overflow test.
//! - **`/ %`, division by zero**: `CMP` the divisor against `XZR`,
//!   `B.EQ` to `__wrela_abort` (`abort_zero`), every width, every
//!   signedness, both ops.
//! - **`/`, signed `MIN/-1`**: an *explicit* pre-check, uniform across
//!   every signed width (`8`/`16`/`32`/`64`) — `lhs == type::MIN &&
//!   rhs == -1`, both compares, both branches to `__wrela_abort`
//!   (`abort_overflow`) — *not* a post-divide bounds check. This is
//!   deliberate, not merely uniform-for-uniformity's sake: at 64-bit
//!   width, ARM's `SDIV` never traps and instead silently *wraps*
//!   `i64::MIN / -1` back to `i64::MIN` itself — a value that is
//!   trivially back in-range, defeating any bounds check computed
//!   *after* dividing. The explicit pre-check is the only form that is
//!   correct at every width uniformly, so it is used at every width
//!   uniformly, even though width `< 64` would also be catchable by a
//!   post-divide bounds check (64-bit `SDIV` on a narrow, sign-extended
//!   operand cannot itself wrap — the true 64-bit quotient of two
//!   values within a narrower range is always exactly representable in
//!   64 bits). Never checked for `%` — `interp.rs`'s own
//!   `eval_div_rem` never reaches this case for `Rem` (mirrored exactly
//!   in `mwir::Inst::DivRem`'s own doc), and it is also arithmetically
//!   moot: `lhs - (lhs/rhs)*rhs` for `rhs == -1` always reduces to `0`
//!   even when the *quotient* itself wrapped in hardware (`MSUB`'s own
//!   64-bit wraparound cancels exactly, `2*i64::MIN mod 2^64 == 0`).
//!   `%` is computed via `SDIV`/`UDIV` then `MSUB` (`ARM` has no direct
//!   remainder instruction): `rem = lhs - (lhs/rhs)*rhs`.
//! - **Shifts**: the range check `count < 0 || count >= bits`
//!   (`eval_shift`'s own wording) is done as *one* `CMP`/`B.HS` using an
//!   **unsigned** comparison of the (possibly-signed, canonically sign-
//!   extended) count register against `bits` — a negative signed count,
//!   reinterpreted as unsigned, is always astronomically larger than
//!   any `bits <= 64`, so the single unsigned test rejects both "too
//!   negative" and "too large" at once; this is a correctness
//!   technique (the same one range checks are conventionally compiled
//!   to), not a profiled optimization, so it needs no cleverness
//!   budget entry. `Shl`'s own additional "lost nonzero high bits"
//!   check first guards `count == 0` with a `CBZ` (skipping straight to
//!   the real shift — `count == 0` is the one case that would otherwise
//!   falsely trip the check at `bits == 64`, since `LSRV` by a shift
//!   amount that is itself a multiple of 64 is architecturally a no-op,
//!   not a full clear), then computes `shift_amt = bits - count`
//!   (register-register `SUB`) and checks `(cleared_lhs LSRV shift_amt)
//!   != 0` — `cleared_lhs` is `lhs` already truncated to exactly `bits`
//!   width via `narrow_to_width(..., signed=false)` regardless of the
//!   type's own signedness (the "lost bits" test is defined on the
//!   *unsigned* bit pattern, `eval_shift`'s own `bit_pattern = (a as
//!   u128) & mask`, independent of `signed`). The shift itself is a
//!   register-controlled `LSLV`/`LSRV`/`ASRV` by the live `count`;
//!   `Shl`'s result gets `narrow_to_width` (with the type's *real*
//!   signedness this time, to re-sign appropriately); `Shr`'s does not
//!   (an `ASR`/`LSR` of an already-canonical operand by `count < bits`
//!   is automatically canonical — it can only clear or sign-fill bits
//!   above the result's own top, never introduce a bit that needs
//!   truncating).
//! - **`.to[T]()` (`Convert`)**: bounds-check the source's own
//!   (canonical, signed-64) value against the *target* type's
//!   `[min,max]`, materialized as signed 64-bit constants — this is
//!   exact for every target narrower than 64 bits. A 64-bit unsigned
//!   target only ever needs a lower-bound check (`>= 0`; the upper
//!   bound is the register's own full range, nothing to compare
//!   against). A 64-bit signed target only ever needs a check when the
//!   *source* is 64-bit unsigned (`>= 0`, rejecting a source magnitude
//!   past `i64::MAX`); every other 64-bit-target combination always
//!   succeeds (the source's own range is provably a subset). On
//!   success, `narrow_to_width` re-canonicalizes to the target's own
//!   width/signedness (a no-op when the target is 64-bit). Floating
//!   source/target types fail closed (see below) — nothing in the
//!   required golden corpus exercises `Convert` at all, so this is a
//!   reasoned, documented best-effort rather than a golden-proven path.
//!
//! ## `encode.rs` extension: `SMULH`/`UMULH`
//!
//! The one instruction class item A's subset omitted that this item
//! genuinely needs: a 64-bit-by-64-bit widening multiply's *high* word,
//! the only way to detect `U64`/`I64`/`Usize`/`Isize` multiplication
//! overflow without a 128-bit register. Added to `encode.rs` as
//! `enc_smulh`/`enc_umulh`, same "Data-processing (3 source)" ARM ARM
//! class `MUL`/`MSUB` already use (`op31 = 0b010`, `Ra` fixed to `31`,
//! `op54` `0b00`/`0b10` picking signed/unsigned) — two new pure
//! functions plus their own hand-verified unit tests, the existing
//! `encoding_table_golden` test left untouched (an existing golden must
//! not move, CLAUDE.md).
//!
//! ## Codegen-level fail-closed list (deliberately tiny — mwir already
//! gated the large stuff)
//!
//! - **Any floating-point value or operation** (`ConstFloat`, and every
//!   other instruction whose `ty` is `F32`/`F64`). `encode.rs`'s A76
//!   subset (item A) never gained an FP/SIMD encoder — nothing in the
//!   required golden corpus uses a float (verified directly: none of
//!   the seven reused `mwir-*` inputs declare an `f32`/`f64` value) —
//!   so this is an honest, disclosed gap, not a silently approximated
//!   one.
//! - **`ConstText`** (and, transitively, any `Static[Str]`/
//!   `Static[Bytes[N]]`-typed value). `mwir::size_of` itself already
//!   fails closed on a bare `Type::Str` (its own module doc's "known
//!   gap" note) — `Static[Str]`'s *layout* was never solved upstream,
//!   so this module cannot lay out a slot for one either; inherits the
//!   gap rather than inventing a new layout convention mwir itself does
//!   not have.
//! - **More than 8 call arguments** (the calling convention's own
//!   documented scope; stack args are unimplemented).
//! - **A frame whose rounded size exceeds 4095 bytes** (the `ADD`/`SUB`
//!   immediate's own `imm12` range; a second adjustment instruction for
//!   larger frames is unimplemented).
//! - **Sizing a temp `mwir::size_of` itself cannot size** (an
//!   instantiated generic struct/enum, a non-literal array/`Bytes`
//!   length, ...) — passed straight through as `mwir::size_of`'s own
//!   `Err`, not re-worded.

use std::collections::BTreeMap;

use crate::encode::{self, Cond};
use crate::mwir::{self, Inst, LayoutCtx, MwirFn, MwirProgram, Temp};
use crate::sema::types::Type;
use crate::syntax::ast::{AccessMode, BinOp};

// --- scratch register numbering (fixed, never reused for anything else) ---

const X_LR: u8 = 30;
const X_SP: u8 = 31;
const X_ZR: u8 = 31; // same encoding, different meaning outside load/store.

/// General-purpose scratch registers this module uses for every
/// computation. Never overlaps `x0..x8` (call args/results) or
/// `x9..x14` used simultaneously in ways that would clobber a still-
/// live value — every emission function below documents which of these
/// it holds live across which step.
const X_A: u8 = 9;
const X_B: u8 = 10;
const X_C: u8 = 11;
const X_D: u8 = 12;
const X_E: u8 = 13;
const X_F: u8 = 14;

fn reg_name(r: u8) -> String {
    match r {
        X_SP => "sp".to_string(),
        X_LR => "lr".to_string(),
        _ => format!("x{r}"),
    }
}

// --- errors ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenError {
    pub message: String,
}

impl CodegenError {
    fn unimplemented(what: &str) -> CodegenError {
        CodegenError {
            message: format!("codegen for {what} is not implemented yet"),
        }
    }

    fn internal(msg: impl Into<String>) -> CodegenError {
        CodegenError {
            message: format!("internal error: {}", msg.into()),
        }
    }
}

// --- output shape ------------------------------------------------------------

/// A fixup item D's layout pass must resolve once the whole program is
/// laid out into one flat blob (module doc's own "Two things are not
/// resolved" section).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reloc {
    /// The `BL` at `word` targets `key` (a `MwirProgram::fns` key).
    Call { word: usize, key: String },
    /// The `ADRP`+`ADD` pair starting at `word_adrp` (the `ADD` is
    /// always `word_adrp + 1`) targets rodata byte offset `byte_offset`
    /// within the eventual rodata section.
    Rodata {
        word_adrp: usize,
        byte_offset: usize,
    },
    /// The `BL` at `word` targets `__wrela_abort`.
    AbortFixed { word: usize },
    /// The `BL` at `word` targets `__wrela_abort_val`.
    AbortVal { word: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodegenFn {
    pub frame_size: usize,
    /// One entry per emitted machine word: the encoded `u32` plus this
    /// module's own stable, reviewable mnemonic-ish rendering of it
    /// (never re-decoded from the bits — recorded directly at
    /// emission time, since this module always knows exactly what it
    /// just encoded).
    pub code: Vec<(u32, String)>,
    pub relocs: Vec<Reloc>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CodegenProgram {
    pub fns: BTreeMap<String, CodegenFn>,
    pub rodata: Vec<Vec<u8>>,
}

// --- rodata pool (BTreeMap dedup, CLAUDE.md) --------------------------------

struct RodataPool {
    entries: Vec<Vec<u8>>,
    index: BTreeMap<Vec<u8>, usize>,
}

impl RodataPool {
    fn new() -> RodataPool {
        RodataPool {
            entries: Vec::new(),
            index: BTreeMap::new(),
        }
    }

    /// Seeds the pool from `MwirProgram::rodata` (already deduped by
    /// `lower.rs`), preserving every existing index 1:1 so
    /// `Inst::ConstText::data` stays a valid index into this pool too.
    fn seed(&mut self, initial: &[Vec<u8>]) {
        for bytes in initial {
            self.intern(bytes.clone());
        }
    }

    /// Content-addressed dedup: interning the same bytes twice (e.g.
    /// once in codegen's own sizing pass, once in its real emission
    /// pass) returns the same index both times.
    fn intern(&mut self, bytes: Vec<u8>) -> usize {
        if let Some(&i) = self.index.get(&bytes) {
            return i;
        }
        let i = self.entries.len();
        self.index.insert(bytes.clone(), i);
        self.entries.push(bytes);
        i
    }

    fn byte_offset(&self, idx: usize) -> usize {
        self.entries[..idx].iter().map(Vec::len).sum()
    }
}

// --- type helpers ------------------------------------------------------------

fn strip_wrappers(ty: &Type) -> &Type {
    match ty {
        Type::Own(_, inner) => strip_wrappers(inner),
        Type::Static(inner) => strip_wrappers(inner),
        other => other,
    }
}

fn is_aggregate(ty: &Type) -> bool {
    matches!(
        strip_wrappers(ty),
        Type::Named(..) | Type::Tuple(_) | Type::Array(..) | Type::Option(_) | Type::Result(..)
    )
}

/// `(bit width, signed)` for the ten integer scalar types — a small,
/// deliberate duplicate of `eval::value::int_shape` (private to that
/// module), the same "recompute the dumb fact, don't thread state"
/// pattern `mwir.rs`'s own module doc already uses for `eval_array_len`.
fn int_shape(ty: &Type) -> Option<(u32, bool)> {
    match strip_wrappers(ty) {
        Type::U8 => Some((8, false)),
        Type::U16 => Some((16, false)),
        Type::U32 => Some((32, false)),
        Type::U64 | Type::Usize => Some((64, false)),
        Type::I8 => Some((8, true)),
        Type::I16 => Some((16, true)),
        Type::I32 => Some((32, true)),
        Type::I64 | Type::Isize => Some((64, true)),
        _ => None,
    }
}

/// `[min,max]` as signed 64-bit host constants — exact for every width
/// `<= 64` except `U64`/`Usize`'s own upper bound (`u64::MAX` does not
/// fit an `i64`); callers needing that width use the flag-based path
/// instead and never call this for it.
fn int_bounds_i64(ty: &Type) -> Option<(i64, i64)> {
    match strip_wrappers(ty) {
        Type::U8 => Some((0, u8::MAX as i64)),
        Type::U16 => Some((0, u16::MAX as i64)),
        Type::U32 => Some((0, u32::MAX as i64)),
        Type::U64 | Type::Usize => Some((0, i64::MAX)),
        Type::I8 => Some((i8::MIN as i64, i8::MAX as i64)),
        Type::I16 => Some((i16::MIN as i64, i16::MAX as i64)),
        Type::I32 => Some((i32::MIN as i64, i32::MAX as i64)),
        Type::I64 | Type::Isize => Some((i64::MIN, i64::MAX)),
        _ => None,
    }
}

fn is_float(ty: &Type) -> bool {
    matches!(strip_wrappers(ty), Type::F32 | Type::F64)
}

// --- frame layout ------------------------------------------------------------

struct Frame {
    temp_offset: Vec<usize>,
    temp_size: Vec<usize>,
    self_ptr_off: Option<usize>,
    ret_ptr_off: Option<usize>,
    lr_off: usize,
    size: usize,
}

fn round_up_16(n: usize) -> usize {
    (n + 15) & !15
}

fn build_frame(f: &MwirFn, layout: &LayoutCtx) -> Result<Frame, CodegenError> {
    let mut offset = 0usize;
    let mut temp_offset = Vec::with_capacity(f.temp_types.len());
    let mut temp_size = Vec::with_capacity(f.temp_types.len());
    for ty in &f.temp_types {
        let sz = mwir::size_of(ty, layout).map_err(|e| CodegenError::unimplemented(&e))?;
        temp_offset.push(offset);
        temp_size.push(sz);
        offset += sz;
    }
    let self_ptr_off = if f.receiver.is_some() {
        let o = offset;
        offset += 8;
        Some(o)
    } else {
        None
    };
    let ret_ptr_off = if is_aggregate(&f.ret) {
        let o = offset;
        offset += 8;
        Some(o)
    } else {
        None
    };
    let lr_off = offset;
    offset += 8;
    let size = round_up_16(offset);
    if size > 4095 {
        return Err(CodegenError::unimplemented(
            "frames larger than 4095 bytes (the ADD/SUB-immediate imm12 range)",
        ));
    }
    Ok(Frame {
        temp_offset,
        temp_size,
        self_ptr_off,
        ret_ptr_off,
        lr_off,
        size,
    })
}

impl Frame {
    fn off(&self, t: Temp) -> usize {
        self.temp_offset[t.0]
    }

    fn size_of_temp(&self, t: Temp) -> usize {
        self.temp_size[t.0]
    }
}

// --- field/payload offset helpers -------------------------------------------

/// The byte offset and size of logical field/element `index` within an
/// already-built aggregate of type `base_ty` (`Project`/`SetField`'s
/// own "compile-time-known-offset" scope: a struct field, a tuple
/// component, or a fixed-array element).
fn field_offset_size(
    base_ty: &Type,
    index: usize,
    layout: &LayoutCtx,
) -> Result<(usize, usize), CodegenError> {
    match strip_wrappers(base_ty) {
        Type::Tuple(elems) => {
            let mut off = 0usize;
            for e in &elems[..index] {
                off += mwir::size_of(e, layout).map_err(|e| CodegenError::unimplemented(&e))?;
            }
            let sz = mwir::size_of(&elems[index], layout)
                .map_err(|e| CodegenError::unimplemented(&e))?;
            Ok((off, sz))
        }
        Type::Array(elem, _) => {
            let sz = mwir::size_of(elem, layout).map_err(|e| CodegenError::unimplemented(&e))?;
            Ok((sz * index, sz))
        }
        Type::Named(name, targs) => {
            if !targs.is_empty() {
                return Err(CodegenError::unimplemented(
                    "field access on an instantiated generic struct",
                ));
            }
            let fields = layout
                .structs
                .get(name)
                .ok_or_else(|| CodegenError::internal(format!("unknown struct `{name}`")))?;
            let mut off = 0usize;
            for f in &fields[..index] {
                off += mwir::size_of(f, layout).map_err(|e| CodegenError::unimplemented(&e))?;
            }
            let sz = mwir::size_of(&fields[index], layout)
                .map_err(|e| CodegenError::unimplemented(&e))?;
            Ok((off, sz))
        }
        other => Err(CodegenError::internal(format!(
            "`Project`/`SetField` base is not an aggregate type: {other:?}"
        ))),
    }
}

/// The byte offset of enum payload slot `index`, past the 8-byte tag —
/// module doc has no room to restate this, so the reasoning lives here:
/// `EnumPayload`/`MakeEnum` never carry *which variant* is live, only
/// `base`/`src`'s own enum type, so a specific payload slot's offset is
/// computed the same way regardless of which variant actually built the
/// value (mirroring `mwir::size_of`'s own "every variant's payload
/// lives at the identical fixed offset" invariant one level down): slot
/// `j`'s own width is the *widest* field at position `j` across every
/// variant that has one there. This is exact for every enum the
/// required golden corpus uses (`Option`/`Result`/every user enum
/// tested all have at most one payload field per variant, so there is
/// only ever slot `0` to place) and is a reasoned, disclosed
/// generalization — not golden-proven — for a hypothetical multi-field
/// variant.
fn enum_payload_offset(
    base_ty: &Type,
    index: usize,
    layout: &LayoutCtx,
) -> Result<usize, CodegenError> {
    const TAG: usize = 8;
    let variants: Vec<Vec<Type>> = match strip_wrappers(base_ty) {
        Type::Option(inner) => vec![Vec::new(), vec![(**inner).clone()]],
        Type::Result(ok, err) => vec![vec![(**ok).clone()], vec![(**err).clone()]],
        Type::Named(name, targs) => {
            if !targs.is_empty() {
                return Err(CodegenError::unimplemented(
                    "payload access on an instantiated generic enum",
                ));
            }
            layout
                .enums
                .get(name)
                .ok_or_else(|| CodegenError::internal(format!("unknown enum `{name}`")))?
                .clone()
        }
        other => Err(CodegenError::internal(format!(
            "`EnumPayload` base is not an enum type: {other:?}"
        )))?,
    };
    let mut off = TAG;
    for j in 0..index {
        let mut widest = 0usize;
        for v in &variants {
            if let Some(ty) = v.get(j) {
                let sz = mwir::size_of(ty, layout).map_err(|e| CodegenError::unimplemented(&e))?;
                widest = widest.max(sz);
            }
        }
        off += widest;
    }
    Ok(off)
}

// --- per-fn emission context -------------------------------------------------

struct FnCtx<'a> {
    frame: &'a Frame,
    layout: &'a LayoutCtx,
    rodata: &'a mut RodataPool,
    /// `word_offsets[i]` is the starting word index of `body[i]`'s own
    /// emitted code; `word_offsets[body.len()]` is the shared
    /// epilogue's own start (the sentinel every `Return` branches to).
    word_offsets: &'a [usize],
    words: Vec<(u32, String)>,
    relocs: Vec<Reloc>,
}

impl<'a> FnCtx<'a> {
    fn push(&mut self, word: u32, text: String) {
        self.words.push((word, text));
    }

    fn cur_word(&self) -> usize {
        self.words.len()
    }

    // --- loads/stores between a frame slot and a scratch register -----

    fn load_slot(&mut self, reg: u8, off: usize) {
        let off = off as u16;
        self.push(
            encode::enc_ldr_x_imm(reg, X_SP, off),
            format!("ldr {}, [sp, #{off}]", reg_name(reg)),
        );
    }

    fn store_slot(&mut self, reg: u8, off: usize) {
        let off = off as u16;
        self.push(
            encode::enc_str_x_imm(reg, X_SP, off),
            format!("str {}, [sp, #{off}]", reg_name(reg)),
        );
    }

    /// Loads an 8-byte word from `[base_reg, #byte_off]` (`base_reg`
    /// holds a runtime-computed address, e.g. an index-scaled array
    /// element pointer — unlike `load_slot`, `base_reg` need not be
    /// `sp`).
    fn load_ptr(&mut self, reg: u8, base_reg: u8, byte_off: usize) {
        let byte_off = byte_off as u16;
        self.push(
            encode::enc_ldr_x_imm(reg, base_reg, byte_off),
            format!(
                "ldr {}, [{}, #{byte_off}]",
                reg_name(reg),
                reg_name(base_reg)
            ),
        );
    }

    fn store_ptr(&mut self, reg: u8, base_reg: u8, byte_off: usize) {
        let byte_off = byte_off as u16;
        self.push(
            encode::enc_str_x_imm(reg, base_reg, byte_off),
            format!(
                "str {}, [{}, #{byte_off}]",
                reg_name(reg),
                reg_name(base_reg)
            ),
        );
    }

    /// `reg = sp + #off` — the address of a frame slot, for a call's own
    /// aggregate-by-pointer argument/result, or an array's own base
    /// address before index-scaling.
    fn addr_of_slot(&mut self, reg: u8, off: usize) {
        let off = off as u16;
        self.push(
            encode::enc_add_imm(reg, X_SP, off, true),
            format!("add {}, sp, #{off}", reg_name(reg)),
        );
    }

    /// Materializes a 64-bit constant, always exactly four words
    /// (`MOVZ` + three unconditional `MOVK`s — module doc's own
    /// "deliberately not optimized" note).
    fn load_imm(&mut self, reg: u8, value: i64) {
        let bits = value as u64;
        let h0 = (bits & 0xFFFF) as u16;
        let h1 = ((bits >> 16) & 0xFFFF) as u16;
        let h2 = ((bits >> 32) & 0xFFFF) as u16;
        let h3 = ((bits >> 48) & 0xFFFF) as u16;
        self.push(
            encode::enc_movz(reg, h0, 0, true),
            format!("movz {}, #{h0:#x}", reg_name(reg)),
        );
        self.push(
            encode::enc_movk(reg, h1, 16, true),
            format!("movk {}, #{h1:#x}, lsl #16", reg_name(reg)),
        );
        self.push(
            encode::enc_movk(reg, h2, 32, true),
            format!("movk {}, #{h2:#x}, lsl #32", reg_name(reg)),
        );
        self.push(
            encode::enc_movk(reg, h3, 48, true),
            format!("movk {}, #{h3:#x}, lsl #48", reg_name(reg)),
        );
    }

    /// Copies `size` bytes (always a multiple of 8) from `[sp,
    /// #src_off]` to `[sp, #dst_off]`, fully unrolled through scratch
    /// register `X_A` (`MakeAggregate`/`Project`/`SetField`/`MakeEnum`'s
    /// shared "both sides are known compile-time frame offsets" copy
    /// shape).
    fn copy_slot_to_slot(&mut self, dst_off: usize, src_off: usize, size: usize) {
        let mut w = 0;
        while w < size {
            self.load_slot(X_A, src_off + w);
            self.store_slot(X_A, dst_off + w);
            w += 8;
        }
    }

    /// Re-canonicalizes `reg` (already holding a 64-bit value) to
    /// exactly `bits` width, module doc's own "canonical-slot
    /// invariant" section — a no-op at `bits == 64`.
    fn narrow_to_width(&mut self, reg: u8, bits: u32, signed: bool) {
        if bits >= 64 {
            return;
        }
        let shift = (64 - bits) as u8;
        self.push(
            encode::enc_lsl_imm(reg, reg, shift, true),
            format!("lsl {}, {}, #{shift}", reg_name(reg), reg_name(reg)),
        );
        if signed {
            self.push(
                encode::enc_asr_imm(reg, reg, shift, true),
                format!("asr {}, {}, #{shift}", reg_name(reg), reg_name(reg)),
            );
        } else {
            self.push(
                encode::enc_lsr_imm(reg, reg, shift, true),
                format!("lsr {}, {}, #{shift}", reg_name(reg), reg_name(reg)),
            );
        }
    }

    // --- local control flow --------------------------------------------

    fn branch_target_delta(&self, target_mwir_idx: usize, this_word: usize) -> i32 {
        let target_word = self.word_offsets[target_mwir_idx];
        (target_word as i64 - this_word as i64) as i32 * 4
    }

    fn b_unconditional(&mut self, target_mwir_idx: usize) {
        let this_word = self.cur_word();
        let delta = self.branch_target_delta(target_mwir_idx, this_word);
        self.push(encode::enc_b(delta), format!("b #{delta}"));
    }

    fn cbz(&mut self, reg: u8, target_mwir_idx: usize) {
        let this_word = self.cur_word();
        let delta = self.branch_target_delta(target_mwir_idx, this_word);
        self.push(
            encode::enc_cbz(reg, delta, true),
            format!("cbz {}, #{delta}", reg_name(reg)),
        );
    }

    // --- rodata + abort calls -------------------------------------------

    /// `reg = &rodata_bytes` (symbolic `ADRP`+`ADD`, `Reloc::Rodata`).
    fn load_rodata_addr(&mut self, reg: u8, data_index: usize) {
        let byte_offset = self.rodata.byte_offset(data_index);
        let word_adrp = self.cur_word();
        self.push(
            encode::enc_adrp(reg, 0),
            format!("adrp {}, rodata+{byte_offset:#x}", reg_name(reg)),
        );
        self.push(
            encode::enc_add_imm(reg, reg, 0, true),
            format!(
                "add {}, {}, rodata+{byte_offset:#x}",
                reg_name(reg),
                reg_name(reg)
            ),
        );
        self.relocs.push(Reloc::Rodata {
            word_adrp,
            byte_offset,
        });
    }

    fn bl_symbolic_call(&mut self, key: &str) {
        let word = self.cur_word();
        self.push(encode::enc_bl(0), format!("bl <{key}>"));
        self.relocs.push(Reloc::Call {
            word,
            key: key.to_string(),
        });
    }

    /// `__wrela_abort(x0=msg_ptr, x1=msg_len)` — interns `message`,
    /// loads its rodata address into `x0`, its length into `x1`, calls.
    fn abort_fixed(&mut self, message: &str) {
        let bytes = message.as_bytes().to_vec();
        let len = bytes.len();
        let idx = self.rodata.intern(bytes);
        self.load_rodata_addr(0, idx);
        self.load_imm(1, len as i64);
        let word = self.cur_word();
        self.push(encode::enc_bl(0), "bl <__wrela_abort>".to_string());
        self.relocs.push(Reloc::AbortFixed { word });
    }

    /// `__wrela_abort_val(x0=prefix_ptr, x1=prefix_len, x2=value,
    /// x3=value_signed, x4=suffix_ptr, x5=suffix_len)`. `value_reg` must
    /// not be `x0..x5` (every call site below uses `X_A`/`X_B`/... which
    /// never collide).
    fn abort_val(&mut self, prefix: &str, value_reg: u8, signed: bool, suffix: &str) {
        // `value_reg` may itself be clobbered by the moves below if it
        // aliases x0..x5 — every call site uses a scratch register
        // outside that range, so stash it in x2 first regardless.
        self.push(
            encode::enc_mov_reg(2, value_reg, true),
            format!("mov x2, {}", reg_name(value_reg)),
        );
        let prefix_bytes = prefix.as_bytes().to_vec();
        let prefix_len = prefix_bytes.len();
        let prefix_idx = self.rodata.intern(prefix_bytes);
        let suffix_bytes = suffix.as_bytes().to_vec();
        let suffix_len = suffix_bytes.len();
        let suffix_idx = self.rodata.intern(suffix_bytes);
        self.load_rodata_addr(0, prefix_idx);
        self.load_imm(1, prefix_len as i64);
        self.load_imm(3, signed as i64);
        self.load_rodata_addr(4, suffix_idx);
        self.load_imm(5, suffix_len as i64);
        let word = self.cur_word();
        self.push(encode::enc_bl(0), "bl <__wrela_abort_val>".to_string());
        self.relocs.push(Reloc::AbortVal { word });
    }
}

fn cond_mnemonic(cond: Cond) -> &'static str {
    match cond {
        Cond::Eq => "eq",
        Cond::Ne => "ne",
        Cond::Cs => "cs",
        Cond::Cc => "cc",
        Cond::Mi => "mi",
        Cond::Pl => "pl",
        Cond::Vs => "vs",
        Cond::Vc => "vc",
        Cond::Hi => "hi",
        Cond::Ls => "ls",
        Cond::Ge => "ge",
        Cond::Lt => "lt",
        Cond::Gt => "gt",
        Cond::Le => "le",
        Cond::Al => "al",
        Cond::Nv => "nv",
    }
}

fn compare_cond(op: BinOp) -> Result<Cond, CodegenError> {
    Ok(match op {
        BinOp::Lt => Cond::Lt,
        BinOp::Le => Cond::Le,
        BinOp::Gt => Cond::Gt,
        BinOp::Ge => Cond::Ge,
        BinOp::Eq => Cond::Eq,
        BinOp::Ne => Cond::Ne,
        other => {
            return Err(CodegenError::internal(format!(
                "`Compare` with a non-ordering op `{}`",
                other.as_str()
            )));
        }
    })
}

/// The condition that fires exactly when `c` does not — a small,
/// deliberate duplicate of `encode::Cond`'s own private `invert` (this
/// module needs it to turn a "this is the failure condition" fact into
/// "skip the abort call when this passes" branch, `encode.rs` never
/// exposes its own copy publicly).
fn invert_cond(c: Cond) -> Cond {
    match c {
        Cond::Eq => Cond::Ne,
        Cond::Ne => Cond::Eq,
        Cond::Cs => Cond::Cc,
        Cond::Cc => Cond::Cs,
        Cond::Mi => Cond::Pl,
        Cond::Pl => Cond::Mi,
        Cond::Vs => Cond::Vc,
        Cond::Vc => Cond::Vs,
        Cond::Hi => Cond::Ls,
        Cond::Ls => Cond::Hi,
        Cond::Ge => Cond::Lt,
        Cond::Lt => Cond::Ge,
        Cond::Gt => Cond::Le,
        Cond::Le => Cond::Gt,
        Cond::Al => Cond::Nv,
        Cond::Nv => Cond::Al,
    }
}

/// A placeholder forward branch emitted now, patched once the real
/// target position (a few words later, in the *same* instruction's own
/// emission — never a cross-instruction mwir jump, which
/// `b_unconditional`/`cbz` already resolve directly from
/// `word_offsets`) is known. Every checked-arithmetic/bounds abort call
/// is reached by *falling through*; the common case (no overflow) skips
/// over it with one of these.
#[derive(Debug, Clone, Copy)]
enum SkipKind {
    Cond(Cond),
    Cbz(u8),
}

impl FnCtx<'_> {
    fn emit_skip(&mut self, _kind: SkipKind) -> usize {
        let w = self.cur_word();
        self.words.push((0, String::new()));
        w
    }

    fn patch_skip(&mut self, word: usize, kind: SkipKind) {
        let target = self.cur_word();
        let delta = (target as i64 - word as i64) as i32 * 4;
        let (enc, text) = match kind {
            SkipKind::Cond(c) => (
                encode::enc_b_cond(c, delta),
                format!("b.{} #{delta}", cond_mnemonic(c)),
            ),
            SkipKind::Cbz(r) => (
                encode::enc_cbz(r, delta, true),
                format!("cbz {}, #{delta}", reg_name(r)),
            ),
        };
        self.words[word] = (enc, text);
    }

    /// `value_reg` must lie outside `[min,max]` (both signed 64-bit
    /// constants) to abort — narrow-width checked `+ - *`'s own scheme
    /// (module doc). Clobbers `X_D`.
    fn check_bounds_i64_or_abort(&mut self, value_reg: u8, min: i64, max: i64, message: &str) {
        self.load_imm(X_D, min);
        self.push(
            encode::enc_cmp_reg(value_reg, X_D, true),
            format!("cmp {}, {}", reg_name(value_reg), reg_name(X_D)),
        );
        let skip1 = self.emit_skip(SkipKind::Cond(Cond::Ge));
        self.abort_fixed(message);
        self.patch_skip(skip1, SkipKind::Cond(Cond::Ge));
        self.load_imm(X_D, max);
        self.push(
            encode::enc_cmp_reg(value_reg, X_D, true),
            format!("cmp {}, {}", reg_name(value_reg), reg_name(X_D)),
        );
        let skip2 = self.emit_skip(SkipKind::Cond(Cond::Le));
        self.abort_fixed(message);
        self.patch_skip(skip2, SkipKind::Cond(Cond::Le));
    }

    /// `fail_cond` just fired means abort (64-bit-width checked `+ - *`'s
    /// own flag-based scheme, module doc) — branches past the abort
    /// call on the inverted (pass) condition.
    fn check_flags_or_abort(&mut self, fail_cond: Cond, message: &str) {
        let pass = invert_cond(fail_cond);
        let skip = self.emit_skip(SkipKind::Cond(pass));
        self.abort_fixed(message);
        self.patch_skip(skip, SkipKind::Cond(pass));
    }
}

// --- the big per-instruction dispatcher --------------------------------------

fn emit_one(inst: &Inst, f: &MwirFn, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    match inst {
        Inst::ConstInt { dst, ty, value } => {
            if is_float(ty) {
                return Err(CodegenError::internal("`ConstInt` with a float type"));
            }
            ctx.load_imm(X_A, *value as i64);
            ctx.store_slot(X_A, ctx.frame.off(*dst));
        }
        Inst::ConstBool { dst, value } => {
            ctx.load_imm(X_A, if *value { 1 } else { 0 });
            ctx.store_slot(X_A, ctx.frame.off(*dst));
        }
        Inst::ConstFloat { .. } => {
            return Err(CodegenError::unimplemented(
                "floating-point constants (no FP/SIMD encoder subset exists)",
            ));
        }
        Inst::ConstChar { dst, value } => {
            ctx.load_imm(X_A, *value as u32 as i64);
            ctx.store_slot(X_A, ctx.frame.off(*dst));
        }
        Inst::ConstUnit { dst } => {
            ctx.load_imm(X_A, 0);
            ctx.store_slot(X_A, ctx.frame.off(*dst));
        }
        Inst::ConstText { .. } => {
            return Err(CodegenError::unimplemented(
                "`Static[Str]`/`Static[Bytes[N]]` values (mwir::size_of itself has no layout for a bare `Str` yet)",
            ));
        }
        Inst::Copy { dst, src } => {
            let size = ctx.frame.size_of_temp(*dst);
            ctx.copy_slot_to_slot(ctx.frame.off(*dst), ctx.frame.off(*src), size);
        }
        Inst::MakeAggregate { dst, elems } => {
            let dst_off = ctx.frame.off(*dst);
            let mut cur = 0usize;
            for e in elems {
                let sz = ctx.frame.size_of_temp(*e);
                ctx.copy_slot_to_slot(dst_off + cur, ctx.frame.off(*e), sz);
                cur += sz;
            }
        }
        Inst::Project { dst, base, index } => {
            let base_ty = f.temp_types[base.0].clone();
            let (off, size) = field_offset_size(&base_ty, *index, ctx.layout)?;
            ctx.copy_slot_to_slot(ctx.frame.off(*dst), ctx.frame.off(*base) + off, size);
        }
        Inst::SetField { base, index, value } => {
            let base_ty = f.temp_types[base.0].clone();
            let (off, size) = field_offset_size(&base_ty, *index, ctx.layout)?;
            ctx.copy_slot_to_slot(ctx.frame.off(*base) + off, ctx.frame.off(*value), size);
        }
        Inst::IndexGet {
            dst,
            base,
            index,
            len,
        } => {
            let base_ty = f.temp_types[base.0].clone();
            let elem_ty = array_elem_type(&base_ty)?;
            let elem_size =
                mwir::size_of(&elem_ty, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
            emit_index_addr(
                ctx,
                ctx.frame.off(*base),
                ctx.frame.off(*index),
                *len,
                elem_size,
                X_C,
            );
            let dst_off = ctx.frame.off(*dst);
            let mut w = 0;
            while w < elem_size {
                ctx.load_ptr(X_F, X_C, w);
                ctx.store_slot(X_F, dst_off + w);
                w += 8;
            }
        }
        Inst::IndexSet {
            base,
            index,
            value,
            len,
        } => {
            let base_ty = f.temp_types[base.0].clone();
            let elem_ty = array_elem_type(&base_ty)?;
            let elem_size =
                mwir::size_of(&elem_ty, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
            emit_index_addr(
                ctx,
                ctx.frame.off(*base),
                ctx.frame.off(*index),
                *len,
                elem_size,
                X_C,
            );
            let val_off = ctx.frame.off(*value);
            let mut w = 0;
            while w < elem_size {
                ctx.load_slot(X_F, val_off + w);
                ctx.store_ptr(X_F, X_C, w);
                w += 8;
            }
        }
        Inst::MakeEnum { dst, tag, payload } => {
            let dst_off = ctx.frame.off(*dst);
            ctx.load_imm(X_A, *tag as i64);
            ctx.store_slot(X_A, dst_off);
            let mut cur = 8usize;
            for p in payload {
                let sz = ctx.frame.size_of_temp(*p);
                ctx.copy_slot_to_slot(dst_off + cur, ctx.frame.off(*p), sz);
                cur += sz;
            }
        }
        Inst::EnumTag { dst, src } => {
            ctx.load_slot(X_A, ctx.frame.off(*src));
            ctx.store_slot(X_A, ctx.frame.off(*dst));
        }
        Inst::EnumPayload { dst, src, index } => {
            let src_ty = f.temp_types[src.0].clone();
            let off = enum_payload_offset(&src_ty, *index, ctx.layout)?;
            let size = ctx.frame.size_of_temp(*dst);
            ctx.copy_slot_to_slot(ctx.frame.off(*dst), ctx.frame.off(*src) + off, size);
        }
        Inst::ArithChecked {
            dst,
            op,
            ty,
            lhs,
            rhs,
            abort,
        } => emit_arith_checked(ctx, *op, ty, *lhs, *rhs, *dst, abort)?,
        Inst::ArithWrapping {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => emit_arith_wrapping(ctx, *op, ty, *lhs, *rhs, *dst)?,
        Inst::DivRem {
            dst,
            op,
            ty,
            lhs,
            rhs,
            abort_zero,
            abort_overflow,
        } => emit_div_rem(ctx, *op, ty, *lhs, *rhs, *dst, abort_zero, abort_overflow)?,
        Inst::Shift {
            dst,
            op,
            ty,
            lhs,
            rhs,
            bits,
            lost,
        } => emit_shift(ctx, *op, ty, *lhs, *rhs, *bits, lost.as_deref(), *dst)?,
        Inst::Bitwise {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => {
            if is_float(ty) {
                return Err(CodegenError::internal("`Bitwise` with a float type"));
            }
            ctx.load_slot(X_A, ctx.frame.off(*lhs));
            ctx.load_slot(X_B, ctx.frame.off(*rhs));
            let (enc, mnem) = match op {
                BinOp::BitAnd => (encode::enc_and_reg(X_C, X_A, X_B, true), "and"),
                BinOp::BitOr => (encode::enc_orr_reg(X_C, X_A, X_B, true), "orr"),
                BinOp::BitXor => (encode::enc_eor_reg(X_C, X_A, X_B, true), "eor"),
                other => {
                    return Err(CodegenError::internal(format!(
                        "`Bitwise` with a non-bitwise op `{}`",
                        other.as_str()
                    )));
                }
            };
            ctx.push(
                enc,
                format!(
                    "{mnem} {}, {}, {}",
                    reg_name(X_C),
                    reg_name(X_A),
                    reg_name(X_B)
                ),
            );
            ctx.store_slot(X_C, ctx.frame.off(*dst));
        }
        Inst::Compare {
            dst,
            op,
            ty,
            lhs,
            rhs,
        } => {
            if is_float(ty) {
                return Err(CodegenError::unimplemented("floating-point comparison"));
            }
            ctx.load_slot(X_A, ctx.frame.off(*lhs));
            ctx.load_slot(X_B, ctx.frame.off(*rhs));
            ctx.push(
                encode::enc_cmp_reg(X_A, X_B, true),
                format!("cmp {}, {}", reg_name(X_A), reg_name(X_B)),
            );
            let cond = compare_cond(*op)?;
            ctx.push(
                encode::enc_cset(X_C, cond, true),
                format!("cset {}, {}", reg_name(X_C), cond_mnemonic(cond)),
            );
            ctx.store_slot(X_C, ctx.frame.off(*dst));
        }
        Inst::Neg {
            dst,
            ty,
            src,
            abort,
        } => {
            if is_float(ty) {
                return Err(CodegenError::unimplemented("floating-point negation"));
            }
            let (_, signed) = int_shape(ty)
                .ok_or_else(|| CodegenError::internal(format!("`Neg` on non-integer {ty:?}")))?;
            if !signed {
                return Err(CodegenError::internal("`Neg` on an unsigned type"));
            }
            let (min, _) = int_bounds_i64(ty).unwrap();
            ctx.load_slot(X_A, ctx.frame.off(*src));
            ctx.load_imm(X_D, min);
            ctx.push(
                encode::enc_cmp_reg(X_A, X_D, true),
                format!("cmp {}, {}", reg_name(X_A), reg_name(X_D)),
            );
            let skip = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
            ctx.abort_fixed(abort);
            ctx.patch_skip(skip, SkipKind::Cond(Cond::Ne));
            ctx.push(
                encode::enc_sub_reg(X_C, X_ZR, X_A, true),
                format!("neg {}, {}", reg_name(X_C), reg_name(X_A)),
            );
            ctx.store_slot(X_C, ctx.frame.off(*dst));
        }
        Inst::BitNot { dst, ty, src } => {
            if is_float(ty) {
                return Err(CodegenError::internal("`BitNot` with a float type"));
            }
            let (bits, signed) = int_shape(ty)
                .ok_or_else(|| CodegenError::internal(format!("`BitNot` on non-integer {ty:?}")))?;
            ctx.load_slot(X_A, ctx.frame.off(*src));
            ctx.load_imm(X_D, -1);
            ctx.push(
                encode::enc_eor_reg(X_C, X_A, X_D, true),
                format!(
                    "eor {}, {}, {}",
                    reg_name(X_C),
                    reg_name(X_A),
                    reg_name(X_D)
                ),
            );
            ctx.narrow_to_width(X_C, bits, signed);
            ctx.store_slot(X_C, ctx.frame.off(*dst));
        }
        Inst::Convert {
            dst,
            ty,
            src,
            abort,
        } => emit_convert(ctx, f, ty, *src, *dst, abort)?,
        Inst::Not { dst, src } => {
            ctx.load_slot(X_A, ctx.frame.off(*src));
            ctx.push(
                encode::enc_cmp_reg(X_A, X_ZR, true),
                format!("cmp {}, xzr", reg_name(X_A)),
            );
            ctx.push(
                encode::enc_cset(X_C, Cond::Eq, true),
                format!("cset {}, eq", reg_name(X_C)),
            );
            ctx.store_slot(X_C, ctx.frame.off(*dst));
        }
        Inst::BoolAnd { dst, lhs, rhs } => {
            ctx.load_slot(X_A, ctx.frame.off(*lhs));
            ctx.load_slot(X_B, ctx.frame.off(*rhs));
            ctx.push(
                encode::enc_and_reg(X_C, X_A, X_B, true),
                format!(
                    "and {}, {}, {}",
                    reg_name(X_C),
                    reg_name(X_A),
                    reg_name(X_B)
                ),
            );
            ctx.store_slot(X_C, ctx.frame.off(*dst));
        }
        Inst::Jump { target } => ctx.b_unconditional(*target),
        Inst::JumpIfFalse { cond, target } => {
            ctx.load_slot(X_A, ctx.frame.off(*cond));
            ctx.cbz(X_A, *target);
        }
        Inst::Call {
            dst,
            self_write_back: _,
            key,
            args,
        } => {
            if args.len() > 8 {
                return Err(CodegenError::unimplemented("more than 8 call arguments"));
            }
            for (i, arg) in args.iter().enumerate() {
                let arg_ty = &f.temp_types[arg.0];
                if is_aggregate(arg_ty) {
                    ctx.addr_of_slot(i as u8, ctx.frame.off(*arg));
                } else {
                    ctx.load_slot(i as u8, ctx.frame.off(*arg));
                }
            }
            let dst_ty = f.temp_types[dst.0].clone();
            if is_aggregate(&dst_ty) {
                ctx.addr_of_slot(8, ctx.frame.off(*dst));
            }
            ctx.bl_symbolic_call(key);
            if !is_aggregate(&dst_ty) {
                ctx.store_slot(0, ctx.frame.off(*dst));
            }
        }
        Inst::Return { value } => {
            if let Some(v) = value {
                if is_aggregate(&f.ret) {
                    let ret_ptr_off = ctx.frame.ret_ptr_off.ok_or_else(|| {
                        CodegenError::internal("`Return` with a value but no ret_ptr slot")
                    })?;
                    ctx.load_slot(X_A, ret_ptr_off);
                    let size = ctx.frame.size_of_temp(*v);
                    let v_off = ctx.frame.off(*v);
                    let mut w = 0;
                    while w < size {
                        ctx.load_slot(X_B, v_off + w);
                        ctx.store_ptr(X_B, X_A, w);
                        w += 8;
                    }
                } else {
                    ctx.load_slot(0, ctx.frame.off(*v));
                }
            }
            ctx.b_unconditional(f.body.len());
        }
        Inst::AssertFail { message } => {
            let msg = message
                .clone()
                .unwrap_or_else(|| "assertion failed".to_string());
            ctx.abort_fixed(&msg);
        }
    }
    Ok(())
}

fn array_elem_type(base_ty: &Type) -> Result<Type, CodegenError> {
    match strip_wrappers(base_ty) {
        Type::Array(elem, _) => Ok((**elem).clone()),
        other => Err(CodegenError::internal(format!(
            "indexing a non-array type: {other:?}"
        ))),
    }
}

/// Bounds-checks `index_off`'s own value against `len`, aborting with
/// the evaluator's own out-of-bounds wording on failure, then leaves
/// `out_reg = &base[index]` (`base_off + index*elem_size`). Shared by
/// `IndexGet`/`IndexSet`. Clobbers `X_A`, `X_B`, `X_D`, `X_E` and
/// `out_reg`.
fn emit_index_addr(
    ctx: &mut FnCtx,
    base_off: usize,
    index_off: usize,
    len: usize,
    elem_size: usize,
    out_reg: u8,
) {
    ctx.load_slot(X_A, index_off);
    ctx.load_imm(X_B, len as i64);
    ctx.push(
        encode::enc_cmp_reg(X_A, X_B, true),
        format!("cmp {}, {}", reg_name(X_A), reg_name(X_B)),
    );
    let skip = ctx.emit_skip(SkipKind::Cond(Cond::Cc));
    ctx.abort_val(
        "index ",
        X_A,
        false,
        &format!(" out of bounds (length {len})"),
    );
    ctx.patch_skip(skip, SkipKind::Cond(Cond::Cc));
    ctx.addr_of_slot(out_reg, base_off);
    ctx.load_imm(X_D, elem_size as i64);
    ctx.push(
        encode::enc_mul(X_E, X_A, X_D, true),
        format!(
            "mul {}, {}, {}",
            reg_name(X_E),
            reg_name(X_A),
            reg_name(X_D)
        ),
    );
    ctx.push(
        encode::enc_add_reg(out_reg, out_reg, X_E, true),
        format!(
            "add {}, {}, {}",
            reg_name(out_reg),
            reg_name(out_reg),
            reg_name(X_E)
        ),
    );
}

fn emit_arith_checked(
    ctx: &mut FnCtx,
    op: BinOp,
    ty: &Type,
    lhs: Temp,
    rhs: Temp,
    dst: Temp,
    abort: &str,
) -> Result<(), CodegenError> {
    if is_float(ty) {
        return Err(CodegenError::unimplemented("floating-point arithmetic"));
    }
    let (bits, signed) = int_shape(ty)
        .ok_or_else(|| CodegenError::internal(format!("`ArithChecked` on non-integer {ty:?}")))?;
    ctx.load_slot(X_A, ctx.frame.off(lhs));
    ctx.load_slot(X_B, ctx.frame.off(rhs));
    if bits < 64 {
        let (enc, mnem) = match op {
            BinOp::Add => (encode::enc_add_reg(X_C, X_A, X_B, true), "add"),
            BinOp::Sub => (encode::enc_sub_reg(X_C, X_A, X_B, true), "sub"),
            BinOp::Mul => (encode::enc_mul(X_C, X_A, X_B, true), "mul"),
            other => {
                return Err(CodegenError::internal(format!(
                    "`ArithChecked` with op `{}`",
                    other.as_str()
                )));
            }
        };
        ctx.push(
            enc,
            format!(
                "{mnem} {}, {}, {}",
                reg_name(X_C),
                reg_name(X_A),
                reg_name(X_B)
            ),
        );
        let (min, max) = int_bounds_i64(ty).unwrap();
        ctx.check_bounds_i64_or_abort(X_C, min, max, abort);
        ctx.store_slot(X_C, ctx.frame.off(dst));
        return Ok(());
    }
    match op {
        BinOp::Add => {
            ctx.push(
                encode::enc_adds_reg(X_C, X_A, X_B, true),
                format!(
                    "adds {}, {}, {}",
                    reg_name(X_C),
                    reg_name(X_A),
                    reg_name(X_B)
                ),
            );
            let fail = if signed { Cond::Vs } else { Cond::Cs };
            ctx.check_flags_or_abort(fail, abort);
        }
        BinOp::Sub => {
            ctx.push(
                encode::enc_subs_reg(X_C, X_A, X_B, true),
                format!(
                    "subs {}, {}, {}",
                    reg_name(X_C),
                    reg_name(X_A),
                    reg_name(X_B)
                ),
            );
            let fail = if signed { Cond::Vs } else { Cond::Cc };
            ctx.check_flags_or_abort(fail, abort);
        }
        BinOp::Mul => {
            ctx.push(
                encode::enc_mul(X_C, X_A, X_B, true),
                format!(
                    "mul {}, {}, {}",
                    reg_name(X_C),
                    reg_name(X_A),
                    reg_name(X_B)
                ),
            );
            if signed {
                ctx.push(
                    encode::enc_smulh(X_D, X_A, X_B),
                    format!(
                        "smulh {}, {}, {}",
                        reg_name(X_D),
                        reg_name(X_A),
                        reg_name(X_B)
                    ),
                );
                ctx.push(
                    encode::enc_asr_imm(X_E, X_C, 63, true),
                    format!("asr {}, {}, #63", reg_name(X_E), reg_name(X_C)),
                );
                ctx.push(
                    encode::enc_cmp_reg(X_D, X_E, true),
                    format!("cmp {}, {}", reg_name(X_D), reg_name(X_E)),
                );
            } else {
                ctx.push(
                    encode::enc_umulh(X_D, X_A, X_B),
                    format!(
                        "umulh {}, {}, {}",
                        reg_name(X_D),
                        reg_name(X_A),
                        reg_name(X_B)
                    ),
                );
                ctx.push(
                    encode::enc_cmp_reg(X_D, X_ZR, true),
                    format!("cmp {}, xzr", reg_name(X_D)),
                );
            }
            ctx.check_flags_or_abort(Cond::Ne, abort);
        }
        other => {
            return Err(CodegenError::internal(format!(
                "`ArithChecked` (64-bit) with op `{}`",
                other.as_str()
            )));
        }
    }
    ctx.store_slot(X_C, ctx.frame.off(dst));
    Ok(())
}

fn emit_arith_wrapping(
    ctx: &mut FnCtx,
    op: BinOp,
    ty: &Type,
    lhs: Temp,
    rhs: Temp,
    dst: Temp,
) -> Result<(), CodegenError> {
    if is_float(ty) {
        return Err(CodegenError::unimplemented(
            "floating-point arithmetic (ArithWrapping doubles as float `+ - * / %`)",
        ));
    }
    let (bits, signed) = int_shape(ty)
        .ok_or_else(|| CodegenError::internal(format!("`ArithWrapping` on non-integer {ty:?}")))?;
    ctx.load_slot(X_A, ctx.frame.off(lhs));
    ctx.load_slot(X_B, ctx.frame.off(rhs));
    let (enc, mnem) = match op {
        BinOp::AddW => (encode::enc_add_reg(X_C, X_A, X_B, true), "add"),
        BinOp::SubW => (encode::enc_sub_reg(X_C, X_A, X_B, true), "sub"),
        BinOp::MulW => (encode::enc_mul(X_C, X_A, X_B, true), "mul"),
        other => {
            return Err(CodegenError::internal(format!(
                "`ArithWrapping` with op `{}`",
                other.as_str()
            )));
        }
    };
    ctx.push(
        enc,
        format!(
            "{mnem} {}, {}, {}",
            reg_name(X_C),
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    ctx.narrow_to_width(X_C, bits, signed);
    ctx.store_slot(X_C, ctx.frame.off(dst));
    Ok(())
}

fn emit_div_rem(
    ctx: &mut FnCtx,
    op: BinOp,
    ty: &Type,
    lhs: Temp,
    rhs: Temp,
    dst: Temp,
    abort_zero: &str,
    abort_overflow: &str,
) -> Result<(), CodegenError> {
    if is_float(ty) {
        return Err(CodegenError::unimplemented("floating-point division"));
    }
    let (_, signed) = int_shape(ty)
        .ok_or_else(|| CodegenError::internal(format!("`DivRem` on non-integer {ty:?}")))?;
    ctx.load_slot(X_A, ctx.frame.off(lhs));
    ctx.load_slot(X_B, ctx.frame.off(rhs));
    ctx.push(
        encode::enc_cmp_reg(X_B, X_ZR, true),
        format!("cmp {}, xzr", reg_name(X_B)),
    );
    let skip_zero = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
    ctx.abort_fixed(abort_zero);
    ctx.patch_skip(skip_zero, SkipKind::Cond(Cond::Ne));
    if signed && op == BinOp::Div {
        let (min, _) = int_bounds_i64(ty).unwrap();
        ctx.load_imm(X_D, min);
        ctx.push(
            encode::enc_cmp_reg(X_A, X_D, true),
            format!("cmp {}, {}", reg_name(X_A), reg_name(X_D)),
        );
        let skip_a = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
        ctx.load_imm(X_E, -1);
        ctx.push(
            encode::enc_cmp_reg(X_B, X_E, true),
            format!("cmp {}, {}", reg_name(X_B), reg_name(X_E)),
        );
        let skip_b = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
        ctx.abort_fixed(abort_overflow);
        ctx.patch_skip(skip_a, SkipKind::Cond(Cond::Ne));
        ctx.patch_skip(skip_b, SkipKind::Cond(Cond::Ne));
    }
    let (enc, mnem) = if signed {
        (encode::enc_sdiv(X_C, X_A, X_B, true), "sdiv")
    } else {
        (encode::enc_udiv(X_C, X_A, X_B, true), "udiv")
    };
    ctx.push(
        enc,
        format!(
            "{mnem} {}, {}, {}",
            reg_name(X_C),
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    if op == BinOp::Rem {
        ctx.push(
            encode::enc_msub(X_C, X_C, X_B, X_A, true),
            format!(
                "msub {}, {}, {}, {}",
                reg_name(X_C),
                reg_name(X_C),
                reg_name(X_B),
                reg_name(X_A)
            ),
        );
    } else if op != BinOp::Div {
        return Err(CodegenError::internal(format!(
            "`DivRem` with op `{}`",
            op.as_str()
        )));
    }
    ctx.store_slot(X_C, ctx.frame.off(dst));
    Ok(())
}

fn emit_shift(
    ctx: &mut FnCtx,
    op: BinOp,
    ty: &Type,
    lhs: Temp,
    rhs: Temp,
    bits: u32,
    lost: Option<&str>,
    dst: Temp,
) -> Result<(), CodegenError> {
    if is_float(ty) {
        return Err(CodegenError::internal("`Shift` with a float type"));
    }
    let (_, signed) = int_shape(ty)
        .ok_or_else(|| CodegenError::internal(format!("`Shift` on non-integer {ty:?}")))?;
    ctx.load_slot(X_A, ctx.frame.off(lhs));
    ctx.load_slot(X_B, ctx.frame.off(rhs));
    // Range check: one unsigned compare catches both "negative" and
    // "too large" (module doc's own worked reasoning).
    ctx.load_imm(X_D, bits as i64);
    ctx.push(
        encode::enc_cmp_reg(X_B, X_D, true),
        format!("cmp {}, {}", reg_name(X_B), reg_name(X_D)),
    );
    let skip_range = ctx.emit_skip(SkipKind::Cond(Cond::Cc));
    ctx.abort_val(
        "shift count ",
        X_B,
        signed,
        &format!(" is out of range for a {bits}-bit type"),
    );
    ctx.patch_skip(skip_range, SkipKind::Cond(Cond::Cc));

    if op == BinOp::Shl {
        let skip_zero = ctx.emit_skip(SkipKind::Cbz(X_B));
        ctx.push(
            encode::enc_mov_reg(X_C, X_A, true),
            format!("mov {}, {}", reg_name(X_C), reg_name(X_A)),
        );
        ctx.narrow_to_width(X_C, bits, false);
        ctx.load_imm(X_D, bits as i64);
        ctx.push(
            encode::enc_sub_reg(X_D, X_D, X_B, true),
            format!(
                "sub {}, {}, {}",
                reg_name(X_D),
                reg_name(X_D),
                reg_name(X_B)
            ),
        );
        ctx.push(
            encode::enc_lsr_reg(X_E, X_C, X_D, true),
            format!(
                "lsr {}, {}, {}",
                reg_name(X_E),
                reg_name(X_C),
                reg_name(X_D)
            ),
        );
        ctx.push(
            encode::enc_cmp_reg(X_E, X_ZR, true),
            format!("cmp {}, xzr", reg_name(X_E)),
        );
        let skip_lost = ctx.emit_skip(SkipKind::Cond(Cond::Eq));
        let lost_msg = lost.ok_or_else(|| {
            CodegenError::internal("`Shift` Shl with no `lost` message (mwir producer bug)")
        })?;
        ctx.abort_fixed(lost_msg);
        ctx.patch_skip(skip_lost, SkipKind::Cond(Cond::Eq));
        ctx.patch_skip(skip_zero, SkipKind::Cbz(X_B));

        ctx.push(
            encode::enc_lsl_reg(X_F, X_A, X_B, true),
            format!(
                "lsl {}, {}, {}",
                reg_name(X_F),
                reg_name(X_A),
                reg_name(X_B)
            ),
        );
        ctx.narrow_to_width(X_F, bits, signed);
        ctx.store_slot(X_F, ctx.frame.off(dst));
    } else if op == BinOp::Shr {
        let (enc, mnem) = if signed {
            (encode::enc_asr_reg(X_F, X_A, X_B, true), "asr")
        } else {
            (encode::enc_lsr_reg(X_F, X_A, X_B, true), "lsr")
        };
        ctx.push(
            enc,
            format!(
                "{mnem} {}, {}, {}",
                reg_name(X_F),
                reg_name(X_A),
                reg_name(X_B)
            ),
        );
        ctx.store_slot(X_F, ctx.frame.off(dst));
    } else {
        return Err(CodegenError::internal(format!(
            "`Shift` with op `{}`",
            op.as_str()
        )));
    }
    Ok(())
}

fn emit_convert(
    ctx: &mut FnCtx,
    f: &MwirFn,
    target_ty: &Type,
    src: Temp,
    dst: Temp,
    abort: &str,
) -> Result<(), CodegenError> {
    let src_ty = f.temp_types[src.0].clone();
    if is_float(target_ty) || is_float(&src_ty) {
        return Err(CodegenError::unimplemented(
            "floating-point `.to[T]()` conversion",
        ));
    }
    let (tbits, tsigned) = int_shape(target_ty)
        .ok_or_else(|| CodegenError::internal(format!("`Convert` target {target_ty:?}")))?;
    let (sbits, ssigned) = int_shape(&src_ty)
        .ok_or_else(|| CodegenError::internal(format!("`Convert` source {src_ty:?}")))?;
    ctx.load_slot(X_A, ctx.frame.off(src));
    if tbits == 64 && !tsigned {
        if ssigned {
            ctx.push(
                encode::enc_cmp_reg(X_A, X_ZR, true),
                format!("cmp {}, xzr", reg_name(X_A)),
            );
            let skip = ctx.emit_skip(SkipKind::Cond(Cond::Ge));
            ctx.abort_fixed(abort);
            ctx.patch_skip(skip, SkipKind::Cond(Cond::Ge));
        }
    } else if tbits == 64 && tsigned {
        if !ssigned && sbits == 64 {
            ctx.push(
                encode::enc_cmp_reg(X_A, X_ZR, true),
                format!("cmp {}, xzr", reg_name(X_A)),
            );
            let skip = ctx.emit_skip(SkipKind::Cond(Cond::Ge));
            ctx.abort_fixed(abort);
            ctx.patch_skip(skip, SkipKind::Cond(Cond::Ge));
        }
    } else {
        let (min, max) = int_bounds_i64(target_ty).unwrap();
        ctx.check_bounds_i64_or_abort(X_A, min, max, abort);
    }
    ctx.push(
        encode::enc_mov_reg(X_C, X_A, true),
        format!("mov {}, {}", reg_name(X_C), reg_name(X_A)),
    );
    ctx.narrow_to_width(X_C, tbits, tsigned);
    ctx.store_slot(X_C, ctx.frame.off(dst));
    Ok(())
}

// --- prologue/epilogue -------------------------------------------------------

fn emit_prologue(f: &MwirFn, frame: &Frame, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    ctx.push(
        encode::enc_sub_imm(X_SP, X_SP, frame.size as u16, true),
        format!("sub sp, sp, #{}", frame.size),
    );
    ctx.store_slot(X_LR, frame.lr_off);
    let mut next_reg = 0u8;
    if let Some((self_temp, _mode)) = f.receiver {
        let self_ptr_off = frame
            .self_ptr_off
            .ok_or_else(|| CodegenError::internal("receiver present but no self_ptr slot"))?;
        ctx.store_slot(next_reg, self_ptr_off);
        let size = frame.size_of_temp(self_temp);
        let dst_off = frame.off(self_temp);
        let mut w = 0;
        while w < size {
            ctx.load_ptr(X_A, next_reg, w);
            ctx.store_slot(X_A, dst_off + w);
            w += 8;
        }
        next_reg += 1;
    }
    for p in &f.params {
        if next_reg > 8 {
            return Err(CodegenError::unimplemented("more than 8 call arguments"));
        }
        let ty = &f.temp_types[p.0];
        if is_aggregate(ty) {
            let size = frame.size_of_temp(*p);
            let dst_off = frame.off(*p);
            let mut w = 0;
            while w < size {
                ctx.load_ptr(X_A, next_reg, w);
                ctx.store_slot(X_A, dst_off + w);
                w += 8;
            }
        } else {
            ctx.store_slot(next_reg, frame.off(*p));
        }
        next_reg += 1;
    }
    if let Some(ret_ptr_off) = frame.ret_ptr_off {
        ctx.store_slot(8, ret_ptr_off);
    }
    Ok(())
}

fn emit_epilogue(f: &MwirFn, frame: &Frame, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    if let Some((self_temp, mode)) = f.receiver {
        if mode == AccessMode::Mut {
            let self_ptr_off = frame
                .self_ptr_off
                .ok_or_else(|| CodegenError::internal("mut receiver but no self_ptr slot"))?;
            ctx.load_slot(X_A, self_ptr_off);
            let size = frame.size_of_temp(self_temp);
            let src_off = frame.off(self_temp);
            let mut w = 0;
            while w < size {
                ctx.load_slot(X_B, src_off + w);
                ctx.store_ptr(X_B, X_A, w);
                w += 8;
            }
        }
    }
    ctx.load_slot(X_LR, frame.lr_off);
    ctx.push(
        encode::enc_add_imm(X_SP, X_SP, frame.size as u16, true),
        format!("add sp, sp, #{}", frame.size),
    );
    ctx.push(encode::enc_ret(X_LR), "ret".to_string());
    Ok(())
}

// --- per-fn driver: two passes, prologue length measured up front ----------

fn emit_fn(
    f: &MwirFn,
    layout: &LayoutCtx,
    rodata: &mut RodataPool,
) -> Result<CodegenFn, CodegenError> {
    let frame = build_frame(f, layout)?;

    let empty: [usize; 0] = [];
    let mut probe_pro = FnCtx {
        frame: &frame,
        layout,
        rodata,
        word_offsets: &empty,
        words: Vec::new(),
        relocs: Vec::new(),
    };
    emit_prologue(f, &frame, &mut probe_pro)?;
    let prologue_len = probe_pro.words.len();

    let dummy_targets = vec![0usize; f.body.len() + 1];
    let mut counts = Vec::with_capacity(f.body.len());
    for inst in &f.body {
        let mut probe = FnCtx {
            frame: &frame,
            layout,
            rodata,
            word_offsets: &dummy_targets,
            words: Vec::new(),
            relocs: Vec::new(),
        };
        emit_one(inst, f, &mut probe)?;
        counts.push(probe.words.len());
    }
    let mut word_offsets = vec![0usize; f.body.len() + 1];
    let mut acc = prologue_len;
    for (i, c) in counts.iter().enumerate() {
        word_offsets[i] = acc;
        acc += c;
    }
    word_offsets[f.body.len()] = acc;

    let mut ctx = FnCtx {
        frame: &frame,
        layout,
        rodata,
        word_offsets: &word_offsets,
        words: Vec::new(),
        relocs: Vec::new(),
    };
    emit_prologue(f, &frame, &mut ctx)?;
    debug_assert_eq!(ctx.words.len(), prologue_len);
    for inst in &f.body {
        emit_one(inst, f, &mut ctx)?;
    }
    debug_assert_eq!(ctx.words.len(), word_offsets[f.body.len()]);
    emit_epilogue(f, &frame, &mut ctx)?;

    Ok(CodegenFn {
        frame_size: frame.size,
        code: ctx.words,
        relocs: ctx.relocs,
    })
}

// --- top-level entry ----------------------------------------------------------

pub fn codegen_program(
    mwir: &MwirProgram,
    layout: &LayoutCtx,
) -> Result<CodegenProgram, CodegenError> {
    let mut rodata = RodataPool::new();
    rodata.seed(&mwir.rodata);
    let mut fns = BTreeMap::new();
    for (key, f) in &mwir.fns {
        let cf = emit_fn(f, layout, &mut rodata)?;
        fns.insert(key.clone(), cf);
    }
    Ok(CodegenProgram {
        fns,
        rodata: rodata.entries,
    })
}

// --- the `--stage=asm` dump --------------------------------------------------

pub fn dump(program: &CodegenProgram) -> String {
    let mut out = String::new();
    out.push_str("Program\n");
    for (key, f) in &program.fns {
        push_line(
            &mut out,
            1,
            &format!("Fn key={key} frame={} bytes", f.frame_size),
        );
        for (i, (word, text)) in f.code.iter().enumerate() {
            push_line(&mut out, 2, &format!("{i:04}: {word:08x}  {text}"));
        }
    }
    if !program.rodata.is_empty() {
        push_line(&mut out, 1, "Rodata");
        let mut off = 0usize;
        for (i, bytes) in program.rodata.iter().enumerate() {
            push_line(
                &mut out,
                2,
                &format!("{i}: offset={off:#x} {}", render_bytes(bytes)),
            );
            off += bytes.len();
        }
    }
    out
}

fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

/// The identical lossy-UTF-8, `\`/newline-escaping rendering
/// `mwir::dump`'s own `render_bytes` uses — a small, deliberate
/// duplicate (that helper is private to `mwir.rs`).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sema;
    use crate::syntax::{ast, lexer, parser};

    fn compile(src: &str) -> (MwirProgram, LayoutCtx) {
        let tokens = lexer::lex(src).expect("test source must lex");
        let module = parser::parse(tokens).expect("test source must parse");
        let typed = sema::check_typed(&module, "<test>").expect("test source must check");
        let mwir_program = crate::lower::lower_program(&typed).expect("test source must lower");
        let layout = mwir::build_layout_ctx(&module).expect("test source must build a layout ctx");
        (mwir_program, layout)
    }

    // --- frame-slot assignment (task note 5's own first requirement) ---

    #[test]
    fn frame_slots_are_assigned_in_temp_order_with_no_packing() {
        let f = MwirFn {
            receiver: None,
            params: vec![Temp(0)],
            ret: Type::U64,
            temp_types: vec![Type::U8, Type::U64, Type::Bool],
            body: vec![Inst::Return { value: None }],
        };
        let layout = LayoutCtx::default();
        let frame = build_frame(&f, &layout).expect("build_frame");
        // Every scalar is one 8-byte slot regardless of its own declared
        // width (mwir's own "no packing, ever" rule) — offsets are a
        // plain running sum, never sub-word-aligned.
        assert_eq!(frame.temp_offset, vec![0, 8, 16]);
        assert_eq!(frame.temp_size, vec![8, 8, 8]);
        // No receiver, scalar ret -> no self_ptr/ret_ptr slots; `lr` sits
        // right after the temps; frame size rounds up to 16.
        assert_eq!(frame.self_ptr_off, None);
        assert_eq!(frame.ret_ptr_off, None);
        assert_eq!(frame.lr_off, 24);
        assert_eq!(frame.size, 32);
    }

    #[test]
    fn frame_reserves_self_ptr_and_ret_ptr_slots_when_needed() {
        let f = MwirFn {
            receiver: Some((Temp(0), AccessMode::Mut)),
            params: vec![],
            ret: Type::Named("Point".to_string(), vec![]),
            temp_types: vec![Type::Named("Point".to_string(), vec![])],
            body: vec![Inst::Return { value: None }],
        };
        let mut layout = LayoutCtx::default();
        layout
            .structs
            .insert("Point".to_string(), vec![Type::U64, Type::U64]);
        let frame = build_frame(&f, &layout).expect("build_frame");
        // temps: t0 (Point, 16 bytes) at [0,16); self_ptr at 16; ret_ptr
        // at 24 (the receiver's own aggregate type is also the return
        // type here, but the two slots are still distinct — self_write_
        // back and an aggregate result are independent facts); lr at 32.
        assert_eq!(frame.temp_offset, vec![0]);
        assert_eq!(frame.temp_size, vec![16]);
        assert_eq!(frame.self_ptr_off, Some(16));
        assert_eq!(frame.ret_ptr_off, Some(24));
        assert_eq!(frame.lr_off, 32);
        assert_eq!(frame.size, 48);
    }

    #[test]
    fn a_frame_over_4095_bytes_fails_closed() {
        let f = MwirFn {
            receiver: None,
            params: vec![],
            ret: Type::Unit,
            temp_types: vec![Type::Array(
                Box::new(Type::U64),
                Box::new(ast::Expr::Int(ast::Span::default(), "600".to_string())),
            )],
            body: vec![Inst::Return { value: None }],
        };
        let layout = LayoutCtx::default();
        assert!(build_frame(&f, &layout).is_err());
    }

    // --- end-to-end: exact word sequences for tiny fns ------------------

    #[test]
    fn a_fn_returning_a_constant_compiles_to_the_expected_word_sequence() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_const_test\n\npub fn answer() -> u64:\n    return 42\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["answer"];
        assert_eq!(f.frame_size, 16);
        let words: Vec<u32> = f.code.iter().map(|(w, _)| *w).collect();
        assert_eq!(
            words,
            vec![
                encode::enc_sub_imm(X_SP, X_SP, 16, true),
                encode::enc_str_x_imm(X_LR, X_SP, 8),
                encode::enc_movz(X_A, 0x2a, 0, true),
                encode::enc_movk(X_A, 0, 16, true),
                encode::enc_movk(X_A, 0, 32, true),
                encode::enc_movk(X_A, 0, 48, true),
                encode::enc_str_x_imm(X_A, X_SP, 0),
                encode::enc_ldr_x_imm(0, X_SP, 0),
                encode::enc_b(8),
                encode::enc_b(4),
                encode::enc_ldr_x_imm(X_LR, X_SP, 8),
                encode::enc_add_imm(X_SP, X_SP, 16, true),
                encode::enc_ret(X_LR),
            ]
        );
        // No abort/call/rodata reloc is ever needed for a fn this small.
        assert!(f.relocs.is_empty());
    }

    #[test]
    fn nested_calls_emit_symbolic_call_relocs_pointing_at_the_right_words() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_call_test\n\npub fn add_one(x: u64) -> u64:\n    return x + 1\n\npub fn combo(x: u64) -> u64:\n    return add_one(x)\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let combo = &program.fns["combo"];
        let call_relocs: Vec<&Reloc> = combo
            .relocs
            .iter()
            .filter(|r| matches!(r, Reloc::Call { .. }))
            .collect();
        assert_eq!(call_relocs.len(), 1);
        match call_relocs[0] {
            Reloc::Call { word, key } => {
                assert_eq!(key, "add_one");
                let (enc, text) = &combo.code[*word];
                assert_eq!(*enc, encode::enc_bl(0));
                assert_eq!(text, "bl <add_one>");
            }
            _ => unreachable!(),
        }
    }

    // --- overflow-check branch shapes per op (task note 5's own third
    // requirement) ---

    #[test]
    fn narrow_checked_add_bounds_checks_against_the_target_type() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_narrow\n\npub fn add(a: u32, b: u32) -> u32:\n    return a + b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["add"];
        let mnems: Vec<&str> = f.code.iter().map(|(_, t)| t.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("add x")));
        // Two range compares (min then max), each followed by a
        // `b.ge`/`b.le` skip over an inline `bl <__wrela_abort>`.
        assert_eq!(mnems.iter().filter(|m| m.starts_with("cmp")).count(), 2);
        assert!(mnems.iter().any(|m| m.starts_with("b.ge")));
        assert!(mnems.iter().any(|m| m.starts_with("b.le")));
        assert_eq!(
            mnems.iter().filter(|m| **m == "bl <__wrela_abort>").count(),
            2
        );
    }

    #[test]
    fn wide_checked_add_uses_flags_not_a_bounds_compare() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_wide_signed\n\npub fn add(a: i64, b: i64) -> i64:\n    return a + b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["add"];
        let mnems: Vec<&str> = f.code.iter().map(|(_, t)| t.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("adds x")));
        assert!(mnems.iter().any(|m| m.starts_with("b.vc")));
        assert!(!mnems.iter().any(|m| m.starts_with("cmp")));
    }

    #[test]
    fn wide_unsigned_checked_sub_uses_the_carry_clear_condition() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_wide_unsigned\n\npub fn sub(a: u64, b: u64) -> u64:\n    return a - b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["sub"];
        let mnems: Vec<&str> = f.code.iter().map(|(_, t)| t.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("subs x")));
        assert!(mnems.iter().any(|m| m.starts_with("b.cs")));
    }

    #[test]
    fn wide_checked_mul_uses_smulh_and_a_sign_extension_compare() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_wide_mul\n\npub fn mul(a: i64, b: i64) -> i64:\n    return a * b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["mul"];
        let mnems: Vec<&str> = f.code.iter().map(|(_, t)| t.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("mul x")));
        assert!(mnems.iter().any(|m| m.starts_with("smulh")));
        assert!(mnems.iter().any(|m| m.starts_with("asr")));
        assert!(mnems.iter().any(|m| m.starts_with("b.eq")));
    }

    #[test]
    fn signed_div_checks_min_over_neg_one_before_dividing() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_div\n\npub fn div(a: i32, b: i32) -> i32:\n    return a / b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["div"];
        let mnems: Vec<&str> = f.code.iter().map(|(_, t)| t.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("sdiv")));
        // Two aborts reachable: division-by-zero and MIN/-1 overflow.
        assert_eq!(
            mnems.iter().filter(|m| **m == "bl <__wrela_abort>").count(),
            2
        );
    }

    #[test]
    fn unsigned_div_never_checks_min_over_neg_one() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_overflow_udiv\n\npub fn div(a: u32, b: u32) -> u32:\n    return a / b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["div"];
        let mnems: Vec<&str> = f.code.iter().map(|(_, t)| t.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("udiv")));
        // Only the divisor-zero abort is reachable.
        assert_eq!(
            mnems.iter().filter(|m| **m == "bl <__wrela_abort>").count(),
            1
        );
    }

    #[test]
    fn shift_range_check_is_one_unsigned_compare() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_shift\n\npub fn shl(a: u32, n: u32) -> u32:\n    return a << n\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["shl"];
        let mnems: Vec<&str> = f.code.iter().map(|(_, t)| t.as_str()).collect();
        assert!(mnems.iter().any(|m| m.starts_with("b.cc")));
        assert!(mnems.iter().any(|m| m.starts_with("cbz")));
        assert!(mnems.iter().any(|m| m.starts_with("lsl x")));
    }

    // --- rodata dedup determinism (task note 5's own fourth requirement) --

    #[test]
    fn rodata_pool_dedups_identical_bytes_by_content() {
        let mut pool = RodataPool::new();
        let a = pool.intern(b"hello".to_vec());
        let b = pool.intern(b"world".to_vec());
        let c = pool.intern(b"hello".to_vec());
        assert_eq!(a, c);
        assert_ne!(a, b);
        assert_eq!(pool.entries.len(), 2);
        assert_eq!(pool.byte_offset(0), 0);
        assert_eq!(pool.byte_offset(1), 5);
    }

    #[test]
    fn identical_abort_messages_across_fns_share_one_rodata_entry() {
        // `checked_add`/`double` (mwir-calls-shaped) each abort with
        // `"arithmetic overflow in `+`"`/`` "arithmetic overflow in
        // `*`"`` — two *different* messages; two fns that both add
        // `u32`s, though, should share the identical `"arithmetic
        // overflow in `+`"` entry rather than duplicating it.
        let (mwir_program, layout) = compile(
            "module examples.codegen_rodata_dedup\n\npub fn add1(a: u32, b: u32) -> u32:\n    return a + b\n\npub fn add2(a: u32, b: u32) -> u32:\n    return a + b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        assert_eq!(program.rodata.len(), 1);
        assert_eq!(program.rodata[0], b"arithmetic overflow in `+`");
    }

    #[test]
    fn codegen_is_deterministic_across_repeated_runs() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_determinism\n\npub fn add(a: u32, b: u32) -> u32:\n    return a + b\n",
        );
        let p1 = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let p2 = codegen_program(&mwir_program, &layout).expect("codegen_program");
        assert_eq!(p1, p2);
    }

    // --- fail-closed list ------------------------------------------------

    #[test]
    fn a_float_typed_constant_fails_closed() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_float_fails_closed\n\npub fn half() -> f64:\n    return 0.5\n",
        );
        let err = codegen_program(&mwir_program, &layout).unwrap_err();
        assert!(err.message.contains("floating-point"));
    }

    #[test]
    fn more_than_eight_call_arguments_fails_closed() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_too_many_args\n\npub fn nine(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64, g: u64, h: u64, i: u64) -> u64:\n    return a\n\npub fn caller() -> u64:\n    return nine(1, 2, 3, 4, 5, 6, 7, 8, 9)\n",
        );
        let err = codegen_program(&mwir_program, &layout).unwrap_err();
        assert!(err.message.contains("8 call arguments"));
    }
}
