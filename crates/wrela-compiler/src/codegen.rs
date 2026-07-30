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
//! widths, array lengths, element strides, ...) is materialized via
//! `FnCtx::load_imm`. With NarrowImm off (`dev` / default TLS): always
//! `MOVZ` + three unconditional `MOVK`s, exactly four words — the locked
//! naive form (`compiler.codegen.naive-locked`). With NarrowImm on
//! (`opts::apply_mode(Release)`): `MOVZ` at the first non-zero halfword's
//! shift, then `MOVK` only for remaining non-zero halfwords; value `0`
//! is a single `movz #0` (plans/M19.md item I / decision 1486). A
//! per-instruction word count that depends on the constant's bit pattern
//! is still sound under two-pass emit (constants are identical in both
//! passes). Reloc trampoline helpers keep the fixed four-word form
//! (decision 1485) and do not consult the TLS.
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
//! **`write_backs` / `mut self` / non-receiver `mut`, worked out from first
//! principles (mwir's own doc names the requirement, this module supplies
//! the mechanism):** a `Mut` receiver is *always* an aggregate (every
//! struct/enum `self` has a `Type::Named`), so it is *always* passed by
//! the same bare-pointer rule above — this holds regardless of
//! `Inst::Call::write_backs`. A non-receiver `mut` parameter
//! (02-language.md §5.1 / plans/M9.md item CC) is passed by pointer
//! too, even when it is a scalar: the call site puts every
//! `write_backs` args-index in the pointer set alongside aggregates.
//! The callee's own prologue always copies an incoming pointer
//! argument's bytes into its own local temp slot; the callee's own
//! *epilogue* additionally copies that local slot's *current* bytes
//! back out through the *original* incoming pointer (saved at entry) —
//! for a `Mut` receiver (`self_ptr_save`) and for every `Mut` parameter
//! (`param_ptr_offs`). Since the pointer the caller passed *is* the
//! address of its own place temp, this write-back lands exactly where
//! `write_backs`'s own entries promise, with the call site itself doing
//! nothing special after the `BL`. (A `Read`/`Take` argument is never
//! in `write_backs` and never gets an epilogue write.)
//!
//! ## The abort contract (item E's exact obligation)
//!
//! Every checked operation that can fail branches to one of two
//! `noreturn` stub symbols, called via `BL` with a placeholder target
//! (`Reloc::AbortFixed`/`Reloc::AbortVal`) exactly like an ordinary
//! call — never inlined, never returned from.
//!
//! - **`__wrela_abort(x0: *Bytes) -> noreturn`** — every abort whose
//!   *entire* message is fixed at compile time (every ordinary/wrapping-
//!   overflow, div/rem-by-zero, div `MIN/-1`, negation-overflow,
//!   `.to[T]()` out-of-range, `<<` lost-bits, and `assert`/`panic`/
//!   match-fallthrough message). Callers carve a 16-byte `(base, len)`
//!   slot on the stack (`base` = rodata `ADRP`+`ADD`, `len` = byte
//!   length), pass its address in `x0`, and `BL` (noreturn).
//! - **`__wrela_abort_val(x0: *Bytes prefix, x1: value, x2: value_signed,
//!   x3: *Bytes suffix) -> noreturn`** — the two messages whose own
//!   wording embeds a *runtime* value (`eval::value::eval_shift`'s
//!   `"shift count {c} is out of range for a {bits}-bit type"`,
//!   `eval::interp`'s `"index {i} out of bounds (length {len})"`).
//!   `prefix`/`suffix` are stack `Bytes` slots (same shape as abort);
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

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use crate::cost::{CostRule, EmittedWord, FlagEffect, MEM_SP_REG, MemClass, MemRef};
use crate::encode::{self, Cond};
use crate::mwir::{self, Inst, LayoutCtx, MwirFn, MwirProgram, Temp};
use crate::sema::types::Type;
use crate::syntax::ast::{AccessMode, BinOp};

// plans/M15.md item K / decision 1098: test-only front-door. When set,
// `Inst::Dmb` emits nothing — the barrier-deletion golden's mutated arm
// builds the same guest with publish/acquire DMBs stripped. Only the
// CLI (`wrela … --omit-dmb`) and the xtask golden runner set this; it is
// never a production knob. Cleared at the start of every CLI command.
thread_local! {
    static OMIT_DMB: Cell<bool> = const { Cell::new(false) };
}

/// plans/M15.md item K: enable/disable DMB omission for the current thread.
pub fn set_omit_dmb(omit: bool) {
    OMIT_DMB.with(|c| c.set(omit));
}

fn omit_dmb() -> bool {
    OMIT_DMB.with(|c| c.get())
}

// Integrity Phase 2 Item M — Lane 2 in-guest block counters. Test-only
// emission mode (omit-dmb precedent): when set, codegen injects a
// `__wrela_block_hit(id)` call at every basic-block leader; the guest
// dumps the hit vector at exit. Never a production knob. Cleared at the
// start of every CLI command; `NEXT_BLOCK_ID` resets with the flag.
thread_local! {
    static BLOCK_COUNT: Cell<bool> = const { Cell::new(false) };
    static NEXT_BLOCK_ID: Cell<u32> = const { Cell::new(0) };
}

/// Integrity Phase 2 Item M: enable/disable Lane 2 block-counter emission.
pub fn set_block_count(enabled: bool) {
    BLOCK_COUNT.with(|c| c.set(enabled));
    NEXT_BLOCK_ID.with(|c| c.set(0));
}

fn block_count() -> bool {
    BLOCK_COUNT.with(|c| c.get())
}

/// Whether Lane 2 block-counter emission is enabled (layout transcript bound).
pub fn block_count_enabled() -> bool {
    block_count()
}

/// The Lane 2 counter helper's own fn key. It is the one fn instrumentation
/// must never enter: every leader in it would emit `bl <__wrela_block_hit>`
/// into `__wrela_block_hit` itself, which is unbounded self-recursion on the
/// very first hit (measured, plans/M20.md item B: a widened build faults with
/// `unhandled exception` in the guest before the first test line). The
/// pre-M20 `app`-only gate excluded it incidentally — `core.runtime` was
/// never instrumented at all — so decision 1607's widening has to exclude it
/// by name, not by owner.
const BLOCK_HIT_KEY: &str = "__wrela_block_hit";

/// Whether `key` is instrumented under `--block-count`. plans/M20.md item B /
/// decision 1607: **every** owner (`app`, `runtime`, `driver`) is
/// instrumented — the only exclusion is the counter helper itself.
fn block_count_instruments(key: &str) -> bool {
    block_count() && key != BLOCK_HIT_KEY
}

/// plans/M20.md item B: how many Lane 2 block ids the most recent
/// `codegen_program*` call allocated. Both entry points reset
/// `NEXT_BLOCK_ID`, so after a build this is that build's id count — the
/// number decision 1607's widening has to keep under
/// [`crate::rtconfig::BLOCK_POOL_COUNT`]. Read-only; never a knob.
pub fn block_ids_assigned() -> u32 {
    NEXT_BLOCK_ID.with(|c| c.get())
}

// plans/M19.md item B / decision 1421 + item I / 1485–1486: TLS knob for
// narrow-immediate materialization. Default **off**; `opts::apply_mode
// (Release)` turns it on when `OptId::NarrowImm` is in `RELEASE_OPTS`.
// `FnCtx::load_imm` consults this; trampoline/reloc 4-word helpers do not.
thread_local! {
    static NARROW_IMM: Cell<bool> = const { Cell::new(false) };
}

/// plans/M19.md item B/I: enable/disable narrow-imm for the current thread.
pub fn set_narrow_imm(enabled: bool) {
    NARROW_IMM.with(|c| c.set(enabled));
}

/// Whether narrow-imm materialization is enabled (default false).
pub(crate) fn narrow_imm() -> bool {
    NARROW_IMM.with(|c| c.get())
}

// plans/M6.md decision 6 / plans/M11.md decision 740: a *backward*
// unconditional `Jump` (`target <= idx`) is a loop's own back-edge —
// the exact shape `lower.rs` / `flowwir_lower.rs` emit for a `while`/
// `for` trailing repeat. A forward `Jump` (an `if`/`match` arm's
// end-of-block skip) is never a back-edge. Sync `emit_fn` does **not**
// splice a checkpoint onto that back-edge: trip counters (decision 732)
// are the sole sync discharge. Checkpoints on sync back-edges made
// console helpers illegal in multi-core images (M10 decision 597 /
// layout's `Reloc::CheckpointService` ownership check). Async
// `Transition::Jump` still checkpoints via the same position test.

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
/// The persistent-turn-frame base register (async state machines only,
/// `Reloc::TurnFrameAddr`'s own doc comment): loaded fresh at every
/// entry — fresh dispatch or resume — from the fn's own baked-in area
/// address, never live across a turn boundary. Chosen well clear of the
/// argument registers (`x0..x8`), this module's own scratch set
/// (`x9..x14`), and every register the hand-assembled runtime routines
/// in `layout.rs` use (`x9..x17`) — so an `rt_enqueue`/checkpoint call
/// from inside an async body can never clobber it mid-turn.
const X_FRAME: u8 = 28;

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

/// Message prefix marking a codegen error as a **fail-closed resource
/// violation** rather than "this shape did not lower".
///
/// The distinction is load-bearing: `layout::try_layout_with_codegen`
/// deliberately treats an ordinary codegen `Err` as *soft* (report without
/// an `.img`), because a cross-core or partially-lowering shape legitimately
/// produces a report and no image — the `err-cross-core-*` report goldens
/// pin exactly that. A pool overflow is not that. It is a bound the build
/// blew through, and swallowing it hands the caller a silent image-less
/// report and exit code 0 — the fail-open plans/M20.md item B measured on
/// `wrela build --block-count` once `BLOCK_POOL_COUNT` was exhausted.
///
/// A prefix rather than a new error field, for the same reason the
/// producer-bug prefix (`internal_error_census`) is one:
/// `lower_and_codegen_image` already flattens every codegen error to a
/// `String` before layout sees it, so a field would have to be threaded
/// through a conversion that exists to discard structure. One producer, one
/// consumer, both named here.
pub const FAIL_CLOSED_PREFIX: &str = "fail-closed: ";

fn alloc_block_id() -> Result<u32, CodegenError> {
    let id = NEXT_BLOCK_ID.with(|c| {
        let id = c.get();
        c.set(id.saturating_add(1));
        id
    });
    if id as usize >= crate::rtconfig::BLOCK_POOL_COUNT {
        return Err(CodegenError {
            message: format!(
                "{FAIL_CLOSED_PREFIX}block-count pool exhausted (BLOCK_POOL_COUNT={})",
                crate::rtconfig::BLOCK_POOL_COUNT
            ),
        });
    }
    Ok(id)
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
    /// The `BL` at `word` targets `__wrela_checkpoint_service` (plans/
    /// M6.md decision 6/item D task 2): every loop back-edge's own
    /// checkpoint sequence ends in one of these, called only when the
    /// pending word (loaded and tested just before it, `FnCtx::checkpoint`)
    /// is nonzero. At D the target stub is a bare `ret` in every image
    /// flavor (vectors are unraisable until item E) — resolved the same
    /// way `AbortFixed`/`AbortVal` are, one shared symbol per image.
    CheckpointService { word: usize },
    /// The four-word `load_imm` starting at `word` materializes the
    /// absolute address of this async fn's own persistent turn area
    /// (turn record + statically reserved frame slots, `layout.rs`'s
    /// `rtdata` section) — the real turn-suspension mechanism (plans/
    /// M6.md item D verification follow-up, 04-compiler.md §2's "state
    /// machines in statically reserved frame slots" made literal): an
    /// async fn's own locals must survive its own `ret`-to-scheduler
    /// suspension, so they live in this per-turn area (addressed via a
    /// dedicated base register, `X_FRAME`) instead of an SP-relative
    /// stack frame that dies with the call. `key` is the fn's own
    /// `program.fns` key; `layout.rs` resolves it to the owning actor's
    /// turn area (a `Struct.method` key whose struct is a declared
    /// actor) or the fn's own dedicated free-turn area (every other
    /// async fn — `@test(runtime)` roots foremost).
    TurnFrameAddr { word: usize, key: String },
    /// plans/M10.md item 0c1 (decisions 557/567): the four-word `load_imm`
    /// starting at `word` materializes the **`TurnId`** of the turn area
    /// `key` owns — the 1-based index of its element in the one contiguous
    /// `RT.turns` array, not its address. Identical shape and resolution to
    /// `TurnFrameAddr` above (same `key`, same `RuntimePlacement`
    /// owner-resolution rule, via `turn_id_for` rather than
    /// `turn_area_for`), and deliberately a fixed 4-word `load_imm` like
    /// every other relocated constant even though the value fits in one
    /// `movz`: the two-pass sizing this module and `layout.rs` both run
    /// depends on a reloc's width being independent of its value.
    ///
    /// plans/M10.md item 0c3 found the one exception to that rule: a
    /// virtqueue **drain** reads back the `TurnId` a `SLOT_META_WAITER` /
    /// `SLOT_META_REPLY_STAGE` carries and must address the turn it names,
    /// so `TurnsBase`/`TurnStride` below exist for exactly that site.
    /// Everywhere else codegen only *stores* or *compares* a `TurnId`.
    TurnIdImm { word: usize, key: String },
    /// plans/M10.md item 0c3: the four-word `load_imm` starting at `word`
    /// materializes `RuntimePlacement::turns_base` — the base of the one
    /// contiguous `RT.turns` array, and so of `rtdata` itself. One
    /// whole-program constant, exactly like `GroupArenaBase` below (no
    /// `key`: there is one array).
    ///
    /// Needed only because the drain's two slot-meta readers
    /// (`emit_queue_drain`) live in `codegen.rs` while the stride is a
    /// layout-pass fact — `layout.rs`'s own hand-assembled derefs get both
    /// numbers as plain build-time parameters and need no reloc at all.
    TurnsBase { word: usize },
    /// plans/M10.md item 0c3: the four-word `load_imm` starting at `word`
    /// materializes `RuntimeTables::turn_stride`, the uniform power-of-two
    /// element size of the `RT.turns` array. Paired with `TurnsBase` above
    /// to make `turn_addr(id) = turns_base + (id - 1) * turn_stride` out
    /// of instructions — a `mul` by a relocated stride rather than an
    /// `lsl` by a relocated shift, so both halves reuse
    /// `patch_load_imm_words` and neither needs a new patch kind. This is
    /// the same index→address shape `GroupCreate`'s own arena scan already
    /// emits against `GROUP_SLOT_SIZE`.
    TurnStride { word: usize },
    /// The four-word `load_imm` starting at `word` materializes the
    /// absolute base address of the whole-image group arena (plans/M6.md
    /// item F, `layout::RuntimeTables::group_arena_capacity`-many
    /// `GROUP_SLOT_SIZE`-byte slots) — one whole-program constant,
    /// unlike `TurnFrameAddr` (no `key`: there is exactly one arena).
    /// Emitted by `GroupCreate`'s own arena scan and by the group-child
    /// poll routines `layout.rs` hand-assembles.
    GroupArenaBase { word: usize },
    /// plans/M7.md item G, decision 12: the four-word `load_imm` starting
    /// at `word` materializes the vector bit index the image bound to
    /// `@driver` `driver` — an `IrqCap[V]`'s one runtime word. Layout
    /// resolves it from the sealed graph's `vector=` on that driver's
    /// device.
    IrqVector { word: usize, driver: String },
    /// plans/M7.md item G: the four-word `load_imm` starting at `word`
    /// materializes the absolute address of `@driver` `driver`'s sticky
    /// wake-pending word (trailing word of its state).
    WakePending { word: usize, driver: String },
    /// plans/M10.md item D (decisions 613–614) / item F (decision 631): the
    /// four-word `load_imm` starting at `word` materializes one absolute
    /// address of mailbox root `actor`'s placed region — ring bookkeeping
    /// (`ring` / `head` / `tail` / `count`), `state`, or `turn`. Full RT
    /// `@placed` for mailbox `rtdata` is not ready; this is the same
    /// `patch_load_imm_words` shape as `TurnFrameAddr`, not a pointer type.
    MailboxAddr {
        word: usize,
        actor: String,
        field: MailboxField,
    },
    /// plans/M10.md item E3 (decision 621): the four-word `load_imm`
    /// starting at `word` materializes core `core`'s round-robin cursor
    /// address (`RuntimePlacement::rr_cursors[core]`). Same shape as
    /// `MailboxAddr` — full RT `@placed` for the scheduler stripe is not
    /// ready, and the specialized `rt_run_one <core>` body needs the
    /// address without inventing a pointer type.
    RrCursor { word: usize, core: usize },
    /// plans/M10.md item F2 (decision 634): the four-word `load_imm`
    /// starting at `word` materializes one absolute address of cross-core
    /// ring `ring_index` (into `RuntimeTables::rings` /
    /// `RuntimePlacement::rings`) — `ring` / `head` / `tail` / `count`.
    /// Same `patch_load_imm_words` shape as `MailboxAddr`; no pointer type.
    RingAddr {
        word: usize,
        ring_index: usize,
        field: RingField,
    },
    /// plans/M10.md item H (decision 682): the four-word `load_imm`
    /// starting at `word` materializes `@driver` `driver`'s placed state
    /// base (`RuntimePlacement::drivers`). Mirrors `WakePending`'s name
    /// resolution; drivers without mailboxes are not mailbox roots, so
    /// `MailboxAddr::State` cannot name them.
    DriverState { word: usize, driver: String },
    /// plans/M10.md item H (decision 683): four-word `load_imm` of
    /// `device#device`'s placed register-window base (`DeviceRegs`).
    DeviceRegsBase { word: usize, device: usize },
    /// plans/M10.md item H (decision 683): four-word `load_imm` of pool
    /// `pool`'s placed backing base (`PoolPlacement`).
    PoolBase { word: usize, pool: String },
    /// plans/M10.md item H (decision 683): four-word `load_imm` of
    /// `pool_base + index * slot_bytes` — one `own[P] T` slot address.
    PoolSlot {
        word: usize,
        pool: String,
        index: u64,
        slot_bytes: u64,
    },
}

/// Which word of a mailbox root's placed region a `Reloc::MailboxAddr`
/// materializes (plans/M10.md item D / decision 614; item F / decision
/// 627 extends with `Head` / `State` / `Turn` for `rt_select_and_run`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxField {
    Ring,
    Head,
    Tail,
    Count,
    State,
    Turn,
}

/// Which word of a cross-core ring's placed region a `Reloc::RingAddr`
/// materializes (plans/M10.md item F2 / decision 634).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingField {
    Ring,
    Head,
    Tail,
    Count,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodegenFn {
    pub frame_size: usize,
    /// One entry per emitted machine word: encoded `u32`, mnemonic-ish
    /// text (never re-decoded), plus emit-time `CostRule` + dest/src regs
    /// (plans/M18.md freeze 1303). Asm dump prints only word + text.
    pub code: Vec<EmittedWord>,
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
        // plans/M7.md item E4 / decision 19: do **not** strip `Own` here.
        // `own[P] T` is one word (a pool-slot address); treating it as `T`
        // for aggregate classification would pass the address-of-the-word
        // instead of the word, and field offset math would look past an
        // 8-byte slot as if it held `T` inline. Callers that need the
        // payload type use `sema::bodies::unwrap_own` explicitly.
        Type::Static(inner) => strip_wrappers(inner),
        other => other,
    }
}

/// Payload type inside an `own[P] T`, or `ty` unchanged.
fn unwrap_own_ref(ty: &Type) -> &Type {
    match ty {
        Type::Own(_, inner) => inner,
        other => other,
    }
}

pub(crate) fn is_aggregate(ty: &Type) -> bool {
    match strip_wrappers(ty) {
        // plans/M7.md item E4 / decision 19: `own[P] T` is one opaque word
        // (a guest pool-slot address), passed by value like a capability.
        Type::Own(..) => false,
        // Unbounded `Bytes` is a 16-byte (base, len) slot passed by
        // pointer like every other non-scalar — one ABI rule.
        Type::Bytes(None) => true,
        // plans/M6.md item D (verification fix, decision 11b's own boot
        // exercised this for the first time): the M6 builtin-pseudo-type
        // vehicle (`mwir::size_of`'s own doc comment has the full list) is
        // always one opaque 8-byte scalar slot, never a real aggregate —
        // `Actor[T]` in particular is passed by *value* in a register
        // (the handle itself, a build-time-constant index) everywhere a
        // scalar param/return already is, never by pointer. Before this
        // fix, `Type::Named(..)`'s own blanket aggregate classification
        // silently mis-treated it as a by-pointer aggregate — invisible
        // until item D's first real boot of an `Actor[T]`-typed
        // `@test(runtime)` parameter, which faulted dereferencing the
        // handle's own small integer value as if it were an address.
        // plans/M7.md item H1, decision 11: a capability and a bring-up
        // state are each one opaque word (a guest base address), so they
        // are passed by *value* in a register everywhere a scalar is —
        // `mwir::size_of`'s own arm is the other half of this fact, and
        // the two must agree or `init` would receive an address it then
        // dereferenced as a pointer (M6-D's own `Actor[T]` incident).
        Type::Named(name, _)
            if matches!(
                name.as_str(),
                "Actor"
                    | "Group"
                    | "Instant"
                    | "Duration"
                    | "Admission"
                    | "Peer"
                    // plans/M7.md item G, decision 17: one word, passed by
                    // value like every other builtin pseudo-type.
                    | "InterruptCell"
                    // plans/M10.md item D / decision 616 (completing 611):
                    // by-value at the ABI boundary — not a by-pointer
                    // `struct` / `Option` niche packing.
                    | "TurnId"
                    | "CoreId"
                    // plans/M10.md item E2 / decision 669: same list.
                    | "GroupId"
            ) || crate::sema::classes::name_holds_authority(name) =>
        {
            false
        }
        // plans/M10.md item E2 / decision 669: niche-packed `Option[GroupId]`
        // is one bare word passed by value — not the general by-pointer
        // `Option` aggregate (decision 611).
        Type::Option(inner)
            if matches!(
                strip_wrappers(inner),
                Type::Named(name, _) if name == "GroupId"
            ) =>
        {
            false
        }
        Type::Named(..) | Type::Tuple(_) | Type::Array(..) | Type::Option(_) | Type::Result(..) => {
            true
        }
        // plans/M9.md item C1: length word + N byte slots — by-pointer
        // aggregate like every other multi-slot value.
        Type::String(_) => true,
        // Exact `Bytes[N]` stays the slot-per-byte aggregate (decision 596
        // flags this as the anomaly vs packed unbounded handles).
        Type::Bytes(Some(_)) => true,
        _ => false,
    }
}

/// plans/M10.md item E2 / decision 669: `Option[GroupId]` is niche-packed
/// into one word (`None` = 0). Detected here so MakeEnum / EnumTag /
/// EnumPayload emit the niche form rather than tag+payload.
fn is_option_group_id(ty: &Type) -> bool {
    match strip_wrappers(ty) {
        Type::Option(inner) => {
            matches!(strip_wrappers(inner), Type::Named(name, _) if name == "GroupId")
        }
        _ => false,
    }
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
    /// Frame offset of the saved incoming pointer for each `Mut`
    /// non-receiver parameter, in declaration order (plans/M9.md item
    /// CC). Empty when the fn has no `mut` params. Parallel to the
    /// subset of `MwirFn::params` whose mode is `Mut`.
    mut_param_ptr_offs: Vec<(Temp, usize)>,
    ret_ptr_off: Option<usize>,
    /// plans/M7.md item Z1 (decision 9b): this async fn's own **reply
    /// staging slot** — where a callee writes the aggregate reply of an
    /// actor `await` this fn performs. `None` for a sync fn and for any
    /// async fn none of whose own `await` sites has an aggregate declared
    /// reply, which is what keeps every M6 frame byte-for-byte identical
    /// (decision 9c).
    ///
    /// One slot per *fn*, not per await site, sized to the widest declared
    /// reply over that fn's own `Await{ActorCall}` sites: a turn has at
    /// most one outstanding await, so one slot can never be aliased by
    /// two live replies. A sibling of `ret_ptr_off` in both senses — same
    /// register (`x8`), same "who owns the destination memory" answer —
    /// except that this one is the address a *callee* is handed, while
    /// `ret_ptr_off` is where this fn spills the address its own caller
    /// handed it.
    reply_stage_off: Option<usize>,
    /// plans/M17.md item E / freeze 5: packed scratch for `entropy[N]()` —
    /// contiguous `n` bytes the VMM fills, then expanded into the
    /// slot-per-byte `Bytes[N]` destination. `None` when the fn never
    /// emits `FlowInst::Entropy` / `Inst::Entropy`.
    entropy_scratch_off: Option<usize>,
    /// Reserved packed size at `entropy_scratch_off` (max `n` over Entropy
    /// ops in this fn).
    entropy_scratch_size: usize,
    lr_off: usize,
    size: usize,
}

/// The alignment this ABI keeps `sp` at: `Frame::size` is rounded up to
/// 16 (AAPCS64's own requirement, kept even though nothing here calls out
/// to real AAPCS64 code — see the module doc's frame-layout block), and
/// every prologue/epilogue adjustment is by a whole frame size.
///
/// Exported because the proxy-cycle model's SOG §4.5 alignment term
/// (plans/M20.md item I) needs the one fact it can know about a Stack
/// access's *absolute* address: `sp` is unknown, but it is congruent to 0
/// modulo this. Reading it from here rather than restating 16 in
/// `cost/score.rs` keeps the frame rule in one place.
pub const FRAME_SP_ALIGN_BYTES: u64 = 16;

/// Every temp slot is a multiple of this, and every slot offset is too
/// (mwir's "no packing, always an 8-byte-slot-multiple" rule — the module
/// doc's frame-layout block). The §4.5 alignment term quotes this as the
/// reason no frame access can straddle a 16 B or 64 B boundary.
pub const FRAME_SLOT_BYTES: u64 = 8;

fn round_up_16(n: usize) -> usize {
    let a = FRAME_SP_ALIGN_BYTES as usize;
    (n + a - 1) & !(a - 1)
}

/// `reply_stage_size` is 0 for every sync fn and for any async fn with no
/// aggregate-reply `await` site (`build_frame_flow` derives the real
/// number); a nonzero value reserves `Frame::reply_stage_off`.
///
/// `slot_bias` is the same number `FnCtx::slot_bias` will carry — 0 for a
/// sync fn (slots start at `sp`) and `TURN_RECORD_SIZE` for an async one
/// (slots start past the turn record). It is a *parameter* rather than
/// something this fn assumes, because the imm12 ceiling below is a bound
/// on what `addr_of_slot` finally encodes, and that is `off + slot_bias`,
/// not `off`.
fn build_frame(
    f: &MwirFn,
    layout: &LayoutCtx,
    reply_stage_size: usize,
    entropy_scratch_size: usize,
    slot_bias: usize,
) -> Result<Frame, CodegenError> {
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
    let mut mut_param_ptr_offs = Vec::new();
    for (p, mode) in &f.params {
        if *mode == AccessMode::Mut {
            mut_param_ptr_offs.push((*p, offset));
            offset += 8;
        }
    }
    let ret_ptr_off = if is_aggregate(&f.ret) {
        let o = offset;
        offset += 8;
        Some(o)
    } else {
        None
    };
    let reply_stage_off = if reply_stage_size > 0 {
        let o = offset;
        offset += reply_stage_size;
        Some(o)
    } else {
        None
    };
    // Packed entropy scratch (plans/M17.md freeze 5): round the end up to
    // an 8-byte boundary so the following lr slot stays slot-aligned.
    let (entropy_scratch_off, entropy_scratch_size) = if entropy_scratch_size > 0 {
        let o = offset;
        offset += (entropy_scratch_size + 7) & !7;
        (Some(o), entropy_scratch_size)
    } else {
        (None, 0)
    };
    let lr_off = offset;
    offset += 8;
    let size = round_up_16(offset);
    // The imm12 ceiling is on the immediate that actually gets encoded,
    // and for an async fn every slot reference is biased past the turn
    // record: `addr_of_slot` hands `off + slot_bias` straight to
    // `enc_add_imm`, whose field holds 0..4095. Bounding `size` alone let
    // an async frame of, say, 4064 bytes through while its highest
    // aggregate slot encoded as 4064+56 — past the field, where the
    // surplus bits land in `shift`/`S`/`op` and quietly assemble a
    // different instruction (`encode.rs`'s module doc). `size` rather
    // than `size - 1` keeps the bound obviously safe rather than exactly
    // tight: no offset this frame hands out can reach `size`.
    if size + slot_bias > 4095 {
        return Err(CodegenError::unimplemented(&format!(
            "frames larger than {} bytes (the ADD/SUB-immediate imm12 range, less this fn's \
             own {slot_bias}-byte slot bias)",
            4095 - slot_bias
        )));
    }
    Ok(Frame {
        temp_offset,
        temp_size,
        self_ptr_off,
        mut_param_ptr_offs,
        ret_ptr_off,
        reply_stage_off,
        entropy_scratch_off,
        entropy_scratch_size,
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
/// Thin wrapper: offset authority lives in `mwir::field_offset`.
fn field_offset_size(
    base_ty: &Type,
    index: usize,
    layout: &LayoutCtx,
) -> Result<(usize, usize), CodegenError> {
    mwir::field_offset(base_ty, index, layout).map_err(|e| {
        if e.contains("not a literal") || e.contains("not implemented") {
            CodegenError::unimplemented(&e)
        } else {
            CodegenError::internal(e)
        }
    })
}

/// Thin wrapper: offset authority lives in `mwir::enum_payload_offset`.
fn enum_payload_offset(
    base_ty: &Type,
    index: usize,
    layout: &LayoutCtx,
) -> Result<usize, CodegenError> {
    mwir::enum_payload_offset(base_ty, index, layout).map_err(|e| {
        if e.contains("not implemented") {
            CodegenError::unimplemented(&e)
        } else {
            CodegenError::internal(e)
        }
    })
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
    words: Vec<EmittedWord>,
    relocs: Vec<Reloc>,
    /// The base register every frame-slot access goes through: `X_SP`
    /// for a sync fn's ordinary stack frame; `X_FRAME` (x28, holding the
    /// fn's own persistent turn area address) for an async state
    /// machine, whose locals must survive a suspension's `ret` to the
    /// scheduler (`Reloc::TurnFrameAddr`'s own doc comment).
    slot_base: u8,
    /// A fixed byte bias added to every slot offset: `0` for sync fns;
    /// `TURN_RECORD_SIZE` for async fns (the frame slots sit immediately
    /// past the turn record within the turn area).
    slot_bias: usize,
    /// Sequence for Cold-unique MemRefs when a Load/Store address is not
    /// a proven `[base, #imm]` (cost hard-cut item B).
    cold_seq: u64,
}

/// Integrity item D: structural emit-tag shape checked at `FnCtx::push` /
/// `push_mem` (and `push_flags`). Fail closed — never silently under-tag.
///
/// - `Call` ⇒ `dst == Some(0)` (x0 return / clobber)
/// - `Load` with a known (non-unique) address MemRef ⇒ ≥1 src
/// - `Store` with non-unique MemRef ⇒ that MemRef's base ∈ srcs
/// - Unique-cold MemRefs stay exempt (pessimistic address unknown)
fn check_push_shape(rule: CostRule, dst: Option<u8>, srcs: &[u8], mem: Option<&MemRef>) {
    match rule {
        CostRule::Call => {
            assert_eq!(
                dst,
                Some(0),
                "Call must declare dst=Some(0) (x0 return/clobber)"
            );
        }
        CostRule::Load => {
            if let Some(m) = mem {
                if !memref_is_unique_cold(m) {
                    assert!(
                        !srcs.is_empty(),
                        "Load with known address MemRef needs ≥1 src (address base)"
                    );
                }
            }
        }
        CostRule::Store => {
            if let Some(m) = mem {
                if let Some(base) = memref_nonunique_base(m) {
                    assert!(
                        srcs.iter().any(|&r| r == base),
                        "Store with non-unique MemRef requires base reg {base} ∈ srcs (got {srcs:?})"
                    );
                }
            }
        }
        _ => {}
    }
}

fn memref_is_unique_cold(m: &MemRef) -> bool {
    m.class == MemClass::Cold && (m.key & (1u64 << 63)) != 0
}

/// Base register for Stack / cold_stable MemRefs; `None` for unique cold.
fn memref_nonunique_base(m: &MemRef) -> Option<u8> {
    if memref_is_unique_cold(m) {
        None
    } else if m.class == MemClass::Stack {
        Some(MEM_SP_REG)
    } else {
        // cold_stable: base in bits [48..56)
        Some(((m.key >> 48) & 0xFF) as u8)
    }
}

impl<'a> FnCtx<'a> {
    // Best-effort dest/src regs at emit time; `dst=None` / empty `srcs` OK when unknown
    // (scoreboard treats missing operands as no register deps). Never parse mnemonics.
    // Load/Store without a proven address get a unique Cold MemRef; Adrp stays untagged.
    // Integrity item D: structural asserts on Call/Load/Store tag shape at push time.
    fn push(&mut self, word: u32, text: String, rule: CostRule, dst: Option<u8>, srcs: &[u8]) {
        let mem = match rule {
            CostRule::Load | CostRule::Store => Some(self.alloc_unique_cold()),
            _ => None,
        };
        self.push_mem(word, text, rule, dst, srcs, mem);
    }

    // The three-register ALU shape (`<op> d, a, b`, dst `d`, srcs `a`/`b`),
    // written out ~58 times before these existed. Same encoded word, same
    // asm text, same `CostRule` — these are this file's own vocabulary for
    // one instruction, not a layer over `push`.
    fn add_reg(&mut self, d: u8, a: u8, b: u8) {
        self.push(
            encode::enc_add_reg(d, a, b, true),
            format!("add {}, {}, {}", reg_name(d), reg_name(a), reg_name(b)),
            CostRule::Alu,
            Some(d),
            &[a, b],
        );
    }

    fn mul_reg(&mut self, d: u8, a: u8, b: u8) {
        self.push(
            encode::enc_mul(d, a, b, true),
            format!("mul {}, {}, {}", reg_name(d), reg_name(a), reg_name(b)),
            CostRule::Mul,
            Some(d),
            &[a, b],
        );
    }

    fn orr_reg(&mut self, d: u8, a: u8, b: u8) {
        self.push(
            encode::enc_orr_reg(d, a, b, true),
            format!("orr {}, {}, {}", reg_name(d), reg_name(a), reg_name(b)),
            CostRule::Alu,
            Some(d),
            &[a, b],
        );
    }

    fn and_reg(&mut self, d: u8, a: u8, b: u8) {
        self.push(
            encode::enc_and_reg(d, a, b, true),
            format!("and {}, {}, {}", reg_name(d), reg_name(a), reg_name(b)),
            CostRule::Alu,
            Some(d),
            &[a, b],
        );
    }

    /// `cmp a, b` — the two-register compare that sets NZCV, written out
    /// 21 times before this existed. The remaining `enc_cmp_reg` sites keep
    /// their own `push_flags` call: they differ in `dst` or `FlagEffect`.
    fn cmp_reg(&mut self, a: u8, b: u8) {
        self.push_flags(
            encode::enc_cmp_reg(a, b, true),
            format!("cmp {}, {}", reg_name(a), reg_name(b)),
            CostRule::Alu,
            None,
            &[a, b],
            FlagEffect::Write,
        );
    }

    /// Like `push`, plus emit-time NZCV effect (integrity item B).
    fn push_flags(
        &mut self,
        word: u32,
        text: String,
        rule: CostRule,
        dst: Option<u8>,
        srcs: &[u8],
        flags: FlagEffect,
    ) {
        // Flag-setting Alu/Branch paths — not Load/Store/Call; still shape-check.
        check_push_shape(rule, dst, srcs, None);
        let mut ew = EmittedWord::new(word, text, rule, dst, srcs);
        ew.flags = flags;
        self.words.push(ew);
    }

    fn push_mem(
        &mut self,
        word: u32,
        text: String,
        rule: CostRule,
        dst: Option<u8>,
        srcs: &[u8],
        mem: Option<MemRef>,
    ) {
        // Untagged Load/Store → unique cold (pessimistic); proven tags keep their MemRef.
        let mem = match (rule, mem) {
            (CostRule::Load | CostRule::Store, None) => Some(self.alloc_unique_cold()),
            (_, m) => m,
        };
        check_push_shape(rule, dst, srcs, mem.as_ref());
        let mut ew = EmittedWord::new(word, text, rule, dst, srcs);
        ew.mem = mem;
        self.words.push(ew);
    }

    fn alloc_unique_cold(&mut self) -> MemRef {
        let seq = self.cold_seq;
        self.cold_seq = self.cold_seq.wrapping_add(1);
        MemRef::cold_unique(seq)
    }

    fn cur_word(&self) -> usize {
        self.words.len()
    }

    // --- loads/stores between a frame slot and a scratch register -----

    fn load_slot(&mut self, reg: u8, off: usize) {
        let off = (off + self.slot_bias) as u16;
        let base = self.slot_base;
        let mem = MemRef::for_base_imm(base, off as u64);
        self.push_mem(
            encode::enc_ldr_x_imm(reg, base, off),
            format!("ldr {}, [{}, #{off}]", reg_name(reg), reg_name(base)),
            CostRule::Load,
            Some(reg),
            &[base],
            Some(mem),
        );
    }

    fn store_slot(&mut self, reg: u8, off: usize) {
        let off = (off + self.slot_bias) as u16;
        let base = self.slot_base;
        let mem = MemRef::for_base_imm(base, off as u64);
        self.push_mem(
            encode::enc_str_x_imm(reg, base, off),
            format!("str {}, [{}, #{off}]", reg_name(reg), reg_name(base)),
            CostRule::Store,
            None,
            &[reg, base],
            Some(mem),
        );
    }

    /// Loads an 8-byte word from `[base_reg, #byte_off]` (`base_reg`
    /// holds a runtime-computed address, e.g. an index-scaled array
    /// element pointer — unlike `load_slot`, `base_reg` need not be
    /// `sp`).
    fn load_ptr(&mut self, reg: u8, base_reg: u8, byte_off: usize) {
        let byte_off = byte_off as u16;
        let mem = MemRef::for_base_imm(base_reg, byte_off as u64);
        self.push_mem(
            encode::enc_ldr_x_imm(reg, base_reg, byte_off),
            format!(
                "ldr {}, [{}, #{byte_off}]",
                reg_name(reg),
                reg_name(base_reg)
            ),
            CostRule::Load,
            Some(reg),
            &[base_reg],
            Some(mem),
        );
    }

    fn store_ptr(&mut self, reg: u8, base_reg: u8, byte_off: usize) {
        let byte_off = byte_off as u16;
        let mem = MemRef::for_base_imm(base_reg, byte_off as u64);
        self.push_mem(
            encode::enc_str_x_imm(reg, base_reg, byte_off),
            format!(
                "str {}, [{}, #{byte_off}]",
                reg_name(reg),
                reg_name(base_reg)
            ),
            CostRule::Store,
            None,
            &[reg, base_reg],
            Some(mem),
        );
    }

    /// Unsigned byte load `ldrb Wt, [Xn, #imm]`. Shared by
    /// `Inst::BytesIndexGet` and `emit_entropy`'s packed-scratch expand
    /// (plans/M17.md item E / freeze 5) so both reuse one LDRB encoder
    /// call site — an FnCtx method, not a free fn, so the A64
    /// closed-emitter scan (plans/M10.md item F0) does not grow a new
    /// top-level emitter row.
    fn load_byte_imm(&mut self, rt: u8, rn: u8, byte_off: u16) {
        let mem = MemRef::for_base_imm(rn, byte_off as u64);
        self.push_mem(
            encode::enc_ldrb_imm(rt, rn, byte_off),
            format!("ldrb w{rt}, [{}, #{byte_off}]", reg_name(rn)),
            CostRule::Load,
            Some(rt),
            &[rn],
            Some(mem),
        );
    }

    /// `reg = <slot base> + #off` — the address of a frame slot, for a
    /// call's own aggregate-by-pointer argument/result, or an array's own
    /// base address before index-scaling (`slot_base`/`slot_bias`: sp for
    /// sync fns, the persistent turn area for async fns).
    fn addr_of_slot(&mut self, reg: u8, off: usize) {
        let off = (off + self.slot_bias) as u16;
        let base = self.slot_base;
        self.push(
            encode::enc_add_imm(reg, base, off, true),
            format!("add {}, {}, #{off}", reg_name(reg), reg_name(base)),
            CostRule::Alu,
            Some(reg),
            &[base],
        );
    }

    /// Always `MOVZ` + three `MOVK`s (four words). Reloc sites that
    /// layout patches via `patch_load_imm_words` must use this — NarrowImm
    /// must not shrink them (plans/M19.md decision 1485 / item F).
    fn load_imm_naive(&mut self, reg: u8, value: i64) {
        let bits = value as u64;
        let halves: [(u16, u8); 4] = [
            ((bits & 0xFFFF) as u16, 0),
            (((bits >> 16) & 0xFFFF) as u16, 16),
            (((bits >> 32) & 0xFFFF) as u16, 32),
            (((bits >> 48) & 0xFFFF) as u16, 48),
        ];
        let (h0, _) = halves[0];
        self.push(
            encode::enc_movz(reg, h0, 0, true),
            format!("movz {}, #{h0:#x}", reg_name(reg)),
            CostRule::MovWide,
            Some(reg),
            &[],
        );
        for &(imm, shift) in &halves[1..] {
            self.push(
                encode::enc_movk(reg, imm, shift, true),
                format!("movk {}, #{imm:#x}, lsl #{shift}", reg_name(reg)),
                CostRule::MovWide,
                Some(reg),
                &[],
            );
        }
    }

    /// Materializes a 64-bit constant into `reg`.
    ///
    /// NarrowImm off (`dev`): always `MOVZ` + three `MOVK`s (four words).
    /// NarrowImm on: `MOVZ` at the first non-zero halfword's shift, then
    /// `MOVK` only for remaining non-zero halfwords; `0` → one `movz #0`
    /// (plans/M19.md item I / decision 1486). Reloc placeholders use
    /// [`Self::load_imm_naive`] instead (decision 1485).
    fn load_imm(&mut self, reg: u8, value: i64) {
        if !narrow_imm() {
            self.load_imm_naive(reg, value);
            return;
        }
        let bits = value as u64;
        let halves: [(u16, u8); 4] = [
            ((bits & 0xFFFF) as u16, 0),
            (((bits >> 16) & 0xFFFF) as u16, 16),
            (((bits >> 32) & 0xFFFF) as u16, 32),
            (((bits >> 48) & 0xFFFF) as u16, 48),
        ];
        // Narrow path: value 0 → single movz #0; otherwise movz at the
        // first non-zero half, movk for each later non-zero half.
        if bits == 0 {
            self.push(
                encode::enc_movz(reg, 0, 0, true),
                format!("movz {}, #0x0", reg_name(reg)),
                CostRule::MovWide,
                Some(reg),
                &[],
            );
            return;
        }
        let first = halves
            .iter()
            .position(|&(imm, _)| imm != 0)
            .expect("bits != 0 implies a non-zero halfword");
        let (imm0, shift0) = halves[first];
        if shift0 == 0 {
            self.push(
                encode::enc_movz(reg, imm0, 0, true),
                format!("movz {}, #{imm0:#x}", reg_name(reg)),
                CostRule::MovWide,
                Some(reg),
                &[],
            );
        } else {
            self.push(
                encode::enc_movz(reg, imm0, shift0, true),
                format!("movz {}, #{imm0:#x}, lsl #{shift0}", reg_name(reg)),
                CostRule::MovWide,
                Some(reg),
                &[],
            );
        }
        for &(imm, shift) in &halves[first + 1..] {
            if imm == 0 {
                continue;
            }
            self.push(
                encode::enc_movk(reg, imm, shift, true),
                format!("movk {}, #{imm:#x}, lsl #{shift}", reg_name(reg)),
                CostRule::MovWide,
                Some(reg),
                &[],
            );
        }
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
            CostRule::Alu,
            Some(reg),
            &[reg],
        );
        if signed {
            self.push(
                encode::enc_asr_imm(reg, reg, shift, true),
                format!("asr {}, {}, #{shift}", reg_name(reg), reg_name(reg)),
                CostRule::Alu,
                Some(reg),
                &[reg],
            );
        } else {
            self.push(
                encode::enc_lsr_imm(reg, reg, shift, true),
                format!("lsr {}, {}, #{shift}", reg_name(reg), reg_name(reg)),
                CostRule::Alu,
                Some(reg),
                &[reg],
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
        self.push(
            encode::enc_b(delta),
            format!("b #{delta}"),
            CostRule::Branch,
            None,
            &[],
        );
    }

    fn cbz(&mut self, reg: u8, target_mwir_idx: usize) {
        let this_word = self.cur_word();
        let delta = self.branch_target_delta(target_mwir_idx, this_word);
        self.push(
            encode::enc_cbz(reg, delta, true),
            format!("cbz {}, #{delta}", reg_name(reg)),
            CostRule::Branch,
            None,
            &[reg],
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
            CostRule::Adrp,
            Some(reg),
            &[],
        );
        self.push(
            encode::enc_add_imm(reg, reg, 0, true),
            format!(
                "add {}, {}, rodata+{byte_offset:#x}",
                reg_name(reg),
                reg_name(reg)
            ),
            CostRule::Alu,
            Some(reg),
            &[reg],
        );
        self.relocs.push(Reloc::Rodata {
            word_adrp,
            byte_offset,
        });
    }

    /// `bl <key>` — declare x0 clobber (`dst=Some(0)`) and known arg regs as
    /// srcs so the scoreboard waits on the call (integrity item B).
    fn bl_symbolic_call(&mut self, key: &str, arg_srcs: &[u8]) {
        let word = self.cur_word();
        self.push(
            encode::enc_bl(0),
            format!("bl <{key}>"),
            CostRule::Call,
            Some(0),
            arg_srcs,
        );
        self.relocs.push(Reloc::Call {
            word,
            key: key.to_string(),
        });
    }

    /// Integrity Phase 2 Item M: fixed-width `movz/movk` of `id` into x0
    /// then `bl <__wrela_block_hit>`. Fixed 5 words so two-pass sizing
    /// stays identical under NarrowImm.
    fn emit_block_hit(&mut self, id: u32) {
        self.load_imm_naive(0, id as i64);
        self.bl_symbolic_call("__wrela_block_hit", &[0]);
    }

    /// `__wrela_abort(x0=*Bytes)` — interns `message`, builds a stack
    /// `(base, len)` slot, passes its address (noreturn; no SP restore).
    fn abort_fixed(&mut self, message: &str) {
        let bytes = message.as_bytes().to_vec();
        let len = bytes.len();
        let idx = self.rodata.intern(bytes);
        // Noreturn path: carve a 16-byte Bytes slot below the live frame.
        self.push(
            encode::enc_sub_imm(31, 31, 16, true),
            "sub sp, sp, #16  ; abort Bytes slot".to_string(),
            CostRule::Alu,
            Some(31),
            &[31],
        );
        self.load_rodata_addr(X_A, idx);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 0),
            format!("str {}, [sp]  ; Bytes.base", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(0)),
        );
        self.load_imm(X_A, len as i64);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 8),
            format!("str {}, [sp, #8]  ; Bytes.len", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(8)),
        );
        self.push(
            encode::enc_add_imm(0, 31, 0, true),
            "add x0, sp, #0  ; *Bytes".to_string(),
            CostRule::Alu,
            Some(0),
            &[31],
        );
        let word = self.cur_word();
        self.push(
            encode::enc_bl(0),
            "bl <__wrela_abort>".to_string(),
            CostRule::Abort,
            None,
            &[],
        );
        self.relocs.push(Reloc::AbortFixed { word });
    }

    /// plans/M6.md decision 6: "a checkpoint is a short fixed sequence
    /// (load pending word, test, branch to the scheduler's service path)".
    /// Always exactly 7 words, regardless of anything about the call site
    /// (module doc's own "deliberately not optimized" spirit, one level
    /// up): `load_imm` (4) + `ldr` (1) + `cbz` (1, a fixed 2-instruction
    /// skip over the `bl`) + `bl` (1). Scratch-only (`X_A`/`X_B`), safe to
    /// splice in front of any instruction without disturbing a live value
    /// — every checked op's own live operands sit in frame slots, never in
    /// `X_A`/`X_B` across an instruction boundary. The address is core 0's
    /// own pending word (`wrela_machine::pending::core_word_addr`) — M6 is
    /// core-0-only (plans/M6.md's own scope line), so this is never
    /// parameterized by a runtime core id.
    fn checkpoint(&mut self) {
        let addr = wrela_machine::pending::core_word_addr(0);
        self.load_imm(X_A, addr as i64);
        self.push_mem(
            encode::enc_ldr_x_imm(X_B, X_A, 0),
            format!("ldr {}, [{}]", reg_name(X_B), reg_name(X_A)),
            CostRule::Load,
            Some(X_B),
            &[X_A],
            Some(MemRef::for_base_imm(X_A, 0)),
        );
        self.push(
            encode::enc_cbz(X_B, 8, true),
            format!("cbz {}, #8", reg_name(X_B)),
            CostRule::Branch,
            None,
            &[X_B],
        );
        let word = self.cur_word();
        self.push(
            encode::enc_bl(0),
            "bl <__wrela_checkpoint_service>".to_string(),
            CostRule::Call,
            Some(0),
            &[],
        );
        self.relocs.push(Reloc::CheckpointService { word });
    }

    /// `__wrela_abort_val(x0=*prefix, x1=value, x2=signed, x3=*suffix)`.
    /// `value_reg` must not be clobbered before the stash (call sites use
    /// `X_A`/`X_B`/... outside x0..x3).
    fn abort_val(&mut self, prefix: &str, value_reg: u8, signed: bool, suffix: &str) {
        // Stash value before carving stack / building Bytes slots.
        self.push(
            encode::enc_mov_reg(X_B, value_reg, true),
            format!("mov {}, {}", reg_name(X_B), reg_name(value_reg)),
            CostRule::Alu,
            Some(X_B),
            &[value_reg],
        );
        let prefix_bytes = prefix.as_bytes().to_vec();
        let prefix_len = prefix_bytes.len();
        let prefix_idx = self.rodata.intern(prefix_bytes);
        let suffix_bytes = suffix.as_bytes().to_vec();
        let suffix_len = suffix_bytes.len();
        let suffix_idx = self.rodata.intern(suffix_bytes);
        self.push(
            encode::enc_sub_imm(31, 31, 32, true),
            "sub sp, sp, #32  ; abort_val prefix+suffix Bytes".to_string(),
            CostRule::AbortVal,
            Some(31),
            &[31],
        );
        self.load_rodata_addr(X_A, prefix_idx);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 0),
            format!("str {}, [sp]  ; prefix.base", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(0)),
        );
        self.load_imm(X_A, prefix_len as i64);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 8),
            format!("str {}, [sp, #8]  ; prefix.len", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(8)),
        );
        self.load_rodata_addr(X_A, suffix_idx);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 16),
            format!("str {}, [sp, #16]  ; suffix.base", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(16)),
        );
        self.load_imm(X_A, suffix_len as i64);
        self.push_mem(
            encode::enc_str_x_imm(X_A, 31, 24),
            format!("str {}, [sp, #24]  ; suffix.len", reg_name(X_A)),
            CostRule::Store,
            None,
            &[X_A, 31],
            Some(MemRef::stack(24)),
        );
        self.push(
            encode::enc_add_imm(0, 31, 0, true),
            "add x0, sp, #0  ; *prefix".to_string(),
            CostRule::Alu,
            Some(0),
            &[31],
        );
        self.push(
            encode::enc_mov_reg(1, X_B, true),
            format!("mov x1, {}", reg_name(X_B)),
            CostRule::Alu,
            Some(1),
            &[X_B],
        );
        self.load_imm(2, signed as i64);
        self.push(
            encode::enc_add_imm(3, 31, 16, true),
            "add x3, sp, #16  ; *suffix".to_string(),
            CostRule::Alu,
            Some(3),
            &[31],
        );
        let word = self.cur_word();
        self.push(
            encode::enc_bl(0),
            "bl <__wrela_abort_val>".to_string(),
            CostRule::AbortVal,
            None,
            &[],
        );
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
    /// The async entry's own fresh-vs-resume fork (the one consumer):
    /// skip forward over the fresh prologue when the suspended
    /// discriminant is nonzero.
    Cbnz(u8),
}

impl FnCtx<'_> {
    fn emit_skip(&mut self, _kind: SkipKind) -> usize {
        let w = self.cur_word();
        self.words
            .push(EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]));
        w
    }

    fn patch_skip(&mut self, word: usize, kind: SkipKind) {
        let target = self.cur_word();
        let delta = (target as i64 - word as i64) as i32 * 4;
        let (enc, text, srcs, flags) = match kind {
            SkipKind::Cond(c) => {
                // AL/NV ignore NZCV; other conds read flags (integrity item B).
                let flags = match c {
                    Cond::Al | Cond::Nv => FlagEffect::None,
                    _ => FlagEffect::Read,
                };
                (
                    encode::enc_b_cond(c, delta),
                    format!("b.{} #{delta}", cond_mnemonic(c)),
                    Vec::<u8>::new(),
                    flags,
                )
            }
            SkipKind::Cbz(r) => (
                encode::enc_cbz(r, delta, true),
                format!("cbz {}, #{delta}", reg_name(r)),
                vec![r],
                FlagEffect::None,
            ),
            SkipKind::Cbnz(r) => (
                encode::enc_cbnz(r, delta, true),
                format!("cbnz {}, #{delta}", reg_name(r)),
                vec![r],
                FlagEffect::None,
            ),
        };
        self.words[word] =
            EmittedWord::new(enc, text, CostRule::Branch, None, &srcs).with_flags(flags);
    }

    /// `value_reg` must lie outside `[min,max]` (both signed 64-bit
    /// constants) to abort — narrow-width checked `+ - *`'s own scheme
    /// (module doc). Clobbers `X_D`.
    fn check_bounds_i64_or_abort(&mut self, value_reg: u8, min: i64, max: i64, message: &str) {
        self.load_imm(X_D, min);
        self.cmp_reg(value_reg, X_D);
        let skip1 = self.emit_skip(SkipKind::Cond(Cond::Ge));
        self.abort_fixed(message);
        self.patch_skip(skip1, SkipKind::Cond(Cond::Ge));
        self.load_imm(X_D, max);
        self.cmp_reg(value_reg, X_D);
        let skip2 = self.emit_skip(SkipKind::Cond(Cond::Le));
        self.abort_fixed(message);
        self.patch_skip(skip2, SkipKind::Cond(Cond::Le));
    }

    /// `fail_cond` just fired means abort (64-bit-width checked `+ - *`'s
    /// own flag-based scheme, module doc) — branches past the abort
    /// call on the inverted (pass) condition.
    fn check_flags_or_abort(&mut self, fail_cond: Cond, message: &str) {
        let pass = fail_cond.invert();
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
        // plans/M9.md item C2: core-scalar Format into `String[..capacity]`.
        Inst::FormatScalar {
            dst,
            src,
            src_ty,
            capacity,
        } => emit_format_scalar(ctx, *dst, *src, src_ty, *capacity)?,
        // plans/M9.md item C2: `String[..N] + String[..M]`.
        Inst::StringConcat {
            dst,
            lhs,
            rhs,
            lhs_cap,
            rhs_cap,
        } => emit_string_concat(ctx, *dst, *lhs, *rhs, *lhs_cap, *rhs_cap),
        Inst::Project { dst, base, index } => {
            let base_ty = f.temp_types[base.0].clone();
            // plans/M7.md item E4 / decision 19: an `own[P] T` base holds a
            // pool-slot address; project the field from guest memory at
            // that address, not from the 8-byte handle word itself.
            if matches!(base_ty, Type::Own(..)) {
                let payload_ty = unwrap_own_ref(&base_ty);
                let (off, size) = field_offset_size(payload_ty, *index, ctx.layout)?;
                ctx.load_slot(X_A, ctx.frame.off(*base));
                let dst_off = ctx.frame.off(*dst);
                let mut w = 0;
                while w < size {
                    ctx.load_ptr(X_B, X_A, off + w);
                    ctx.store_slot(X_B, dst_off + w);
                    w += 8;
                }
            } else {
                let (off, size) = field_offset_size(&base_ty, *index, ctx.layout)?;
                ctx.copy_slot_to_slot(ctx.frame.off(*dst), ctx.frame.off(*base) + off, size);
            }
        }
        Inst::SetField { base, index, value } => {
            let base_ty = f.temp_types[base.0].clone();
            if matches!(base_ty, Type::Own(..)) {
                let payload_ty = unwrap_own_ref(&base_ty);
                let (off, size) = field_offset_size(payload_ty, *index, ctx.layout)?;
                ctx.load_slot(X_A, ctx.frame.off(*base));
                let src_off = ctx.frame.off(*value);
                let mut w = 0;
                while w < size {
                    ctx.load_slot(X_B, src_off + w);
                    ctx.store_ptr(X_B, X_A, off + w);
                    w += 8;
                }
            } else {
                let (off, size) = field_offset_size(&base_ty, *index, ctx.layout)?;
                ctx.copy_slot_to_slot(ctx.frame.off(*base) + off, ctx.frame.off(*value), size);
            }
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
        // plans/M10.md item B1: dense-layout index through a placed
        // `@layout(runtime)` array field. Bounds-check shape matches
        // `IndexGet` (cmp + `bl __wrela_abort_val`); address is
        // `base + field_offset + index * elem_stride`.
        Inst::PlacedIndexGet {
            dst,
            base,
            field_offset,
            index,
            len,
            elem_stride,
            ty,
        } => {
            emit_placed_index_addr(
                ctx,
                ctx.frame.off(*base),
                *field_offset,
                ctx.frame.off(*index),
                *len,
                *elem_stride,
                X_C,
            );
            let width = mmio_access_width(ty, 0)?;
            let (enc, mnem) = match width {
                1 => (encode::enc_ldrb_imm(X_B, X_C, 0), "ldrb"),
                2 => (encode::enc_ldrh_imm(X_B, X_C, 0), "ldrh"),
                4 => (encode::enc_ldr_w_imm(X_B, X_C, 0), "ldr"),
                _ => (encode::enc_ldr_x_imm(X_B, X_C, 0), "ldr"),
            };
            let rt = if width == 8 {
                reg_name(X_B)
            } else {
                format!("w{X_B}")
            };
            ctx.push(
                enc,
                format!("{mnem} {rt}, [{}, #0]", reg_name(X_C)),
                CostRule::Load,
                Some(X_B),
                &[X_C],
            );
            ctx.store_slot(X_B, ctx.frame.off(*dst));
        }
        Inst::PlacedIndexSet {
            base,
            field_offset,
            index,
            value,
            len,
            elem_stride,
            ty,
        } => {
            emit_placed_index_addr(
                ctx,
                ctx.frame.off(*base),
                *field_offset,
                ctx.frame.off(*index),
                *len,
                *elem_stride,
                X_C,
            );
            let width = mmio_access_width(ty, 0)?;
            ctx.load_slot(X_B, ctx.frame.off(*value));
            let (enc, mnem) = match width {
                1 => (encode::enc_strb_imm(X_B, X_C, 0), "strb"),
                2 => (encode::enc_strh_imm(X_B, X_C, 0), "strh"),
                4 => (encode::enc_str_w_imm(X_B, X_C, 0), "str"),
                _ => (encode::enc_str_x_imm(X_B, X_C, 0), "str"),
            };
            let rt = if width == 8 {
                reg_name(X_B)
            } else {
                format!("w{X_B}")
            };
            ctx.push(
                enc,
                format!("{mnem} {rt}, [{}, #0]", reg_name(X_C)),
                CostRule::Store,
                None,
                &[X_B, X_C],
            );
        }
        // plans/M10.md item B4 / decisions 595–596: packed byte load
        // through an unbounded `Bytes` (base, len) handle.
        Inst::BytesIndexGet { dst, base, index } => {
            emit_bytes_index_addr(ctx, ctx.frame.off(*base), ctx.frame.off(*index), X_C)?;
            ctx.load_byte_imm(X_B, X_C, 0);
            ctx.store_slot(X_B, ctx.frame.off(*dst));
        }
        Inst::MakeEnum { dst, tag, payload } => {
            let dst_off = ctx.frame.off(*dst);
            let dst_ty = f.temp_types[dst.0].clone();
            if is_option_group_id(&dst_ty) {
                // Niche: None = 0; Some(id) = the GroupId word itself
                // (1-based, never zero — decision 567 / 669).
                if *tag == 0 {
                    ctx.load_imm(X_A, 0);
                    ctx.store_slot(X_A, dst_off);
                } else {
                    let p = payload.first().copied().ok_or_else(|| {
                        CodegenError::internal("Some(GroupId) MakeEnum with no payload")
                    })?;
                    let sz = ctx.frame.size_of_temp(p);
                    ctx.copy_slot_to_slot(dst_off, ctx.frame.off(p), sz);
                }
            } else {
                ctx.load_imm(X_A, *tag as i64);
                ctx.store_slot(X_A, dst_off);
                let mut cur = 8usize;
                for p in payload {
                    let sz = ctx.frame.size_of_temp(*p);
                    ctx.copy_slot_to_slot(dst_off + cur, ctx.frame.off(*p), sz);
                    cur += sz;
                }
            }
        }
        Inst::EnumTag { dst, src } => {
            let src_ty = f.temp_types[src.0].clone();
            if is_option_group_id(&src_ty) {
                // tag = (word != 0) ? 1 : 0 — Some vs None.
                ctx.load_slot(X_A, ctx.frame.off(*src));
                ctx.push_flags(
                    encode::enc_cmp_imm(X_A, 0, true),
                    format!("cmp {}, #0", reg_name(X_A)),
                    CostRule::Alu,
                    None,
                    &[X_A],
                    FlagEffect::Write,
                );
                ctx.push_flags(
                    encode::enc_cset(X_A, Cond::Ne, true),
                    format!("cset {}, ne", reg_name(X_A)),
                    CostRule::Alu,
                    Some(X_A),
                    &[],
                    FlagEffect::Read,
                );
                ctx.store_slot(X_A, ctx.frame.off(*dst));
            } else {
                ctx.load_slot(X_A, ctx.frame.off(*src));
                ctx.store_slot(X_A, ctx.frame.off(*dst));
            }
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
                CostRule::Alu,
                None,
                &[],
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
            ctx.cmp_reg(X_A, X_B);
            let cond = compare_cond(*op)?;
            ctx.push_flags(
                encode::enc_cset(X_C, cond, true),
                format!("cset {}, {}", reg_name(X_C), cond_mnemonic(cond)),
                CostRule::Alu,
                Some(X_C),
                &[],
                FlagEffect::Read,
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
            ctx.cmp_reg(X_A, X_D);
            let skip = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
            ctx.abort_fixed(abort);
            ctx.patch_skip(skip, SkipKind::Cond(Cond::Ne));
            ctx.push(
                encode::enc_sub_reg(X_C, X_ZR, X_A, true),
                format!("neg {}, {}", reg_name(X_C), reg_name(X_A)),
                CostRule::Alu,
                Some(X_C),
                &[X_ZR, X_A],
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
                CostRule::Alu,
                Some(X_C),
                &[X_A, X_D],
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
            ctx.push_flags(
                encode::enc_cmp_reg(X_A, X_ZR, true),
                format!("cmp {}, xzr", reg_name(X_A)),
                CostRule::Alu,
                None,
                &[X_A, X_ZR],
                FlagEffect::Write,
            );
            ctx.push_flags(
                encode::enc_cset(X_C, Cond::Eq, true),
                format!("cset {}, eq", reg_name(X_C)),
                CostRule::Alu,
                Some(X_C),
                &[],
                FlagEffect::Read,
            );
            ctx.store_slot(X_C, ctx.frame.off(*dst));
        }
        Inst::BoolAnd { dst, lhs, rhs } => {
            ctx.load_slot(X_A, ctx.frame.off(*lhs));
            ctx.load_slot(X_B, ctx.frame.off(*rhs));
            ctx.and_reg(X_C, X_A, X_B);
            ctx.store_slot(X_C, ctx.frame.off(*dst));
        }
        Inst::Jump { target } => ctx.b_unconditional(*target),
        Inst::JumpIfFalse { cond, target } => {
            ctx.load_slot(X_A, ctx.frame.off(*cond));
            ctx.cbz(X_A, *target);
        }
        Inst::Call {
            dst,
            write_backs,
            key,
            args,
        } => {
            if args.len() > 8 {
                return Err(CodegenError::unimplemented("more than 8 call arguments"));
            }
            let mut by_ptr: BTreeSet<usize> = write_backs.iter().map(|(i, _)| *i).collect();
            for (i, arg) in args.iter().enumerate() {
                let arg_ty = &f.temp_types[arg.0];
                if is_aggregate(arg_ty) {
                    by_ptr.insert(i);
                }
            }
            // One ABI rule: scalar → xN; non-scalar → pointer to caller slot.
            for (i, arg) in args.iter().enumerate() {
                if i > 8 {
                    return Err(CodegenError::unimplemented("more than 8 call arguments"));
                }
                if by_ptr.contains(&i) {
                    ctx.addr_of_slot(i as u8, ctx.frame.off(*arg));
                } else {
                    ctx.load_slot(i as u8, ctx.frame.off(*arg));
                }
            }
            let dst_ty = f.temp_types[dst.0].clone();
            if is_aggregate(&dst_ty) {
                ctx.addr_of_slot(8, ctx.frame.off(*dst));
            }
            let mut arg_srcs: Vec<u8> = (0..args.len().min(8)).map(|i| i as u8).collect();
            if is_aggregate(&dst_ty) {
                arg_srcs.push(8);
            }
            ctx.bl_symbolic_call(key, &arg_srcs);
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
            // Item E's own exact obligation (module doc, "The abort
            // contract"): this text must match `interp::exec_stmt`'s own
            // `TypedStmtKind::Assert` wording byte-for-byte
            // (`format!("assertion failed{msg}")` where `msg` is `""` or
            // `": {message}"`) — the comptime and runtime tiers report the
            // identical failure identically. `lower.rs`'s own
            // `assert_message_text` already strips the message down to its
            // raw literal text (no "assertion failed" prefix baked in
            // there), so this is the one place that prefix belongs.
            let msg = match message {
                Some(m) => format!("assertion failed: {m}"),
                None => "assertion failed".to_string(),
            };
            ctx.abort_fixed(&msg);
        }

        // --- typed MMIO (plans/M7.md item C's surface, item H1's emission)
        //
        // The first — and at M7 the only — memory access this backend
        // emits at an address that is neither a frame slot nor a
        // build-time-constant runtime table: the base comes out of a
        // temp holding an `Mmio[L]` (decision 11's one word), and the
        // offset is the register's own declared `@offset`.
        //
        // **Width is the declaration's, exactly.** A `ReadOnly[u32]` is a
        // 32-bit load and a `WriteOnly[u16]` a 16-bit store — not a
        // uniform 64-bit slot access, which is what every *other* value in
        // this backend gets (`mwir::size_of`'s own "every scalar occupies
        // one 8-byte slot"). A register is not a slot: a wider access
        // would read or clobber the neighbouring bytes of the claim, which
        // is precisely what 03 §2's non-overlap rule exists to prevent, so
        // a width this encoder cannot emit fails closed rather than
        // widening.
        //
        // The loaded value is then spilled into `dst`'s own 8-byte slot;
        // `LDRB`/`LDRH`/`LDR Wt` all zero-extend into the 64-bit register,
        // which is the same representation every other unsigned scalar has
        // here. A *signed* register type would need a sign-extending load
        // this encoder does not have, and says so.
        Inst::MmioRead {
            dst,
            base,
            offset,
            ty,
        } => {
            let width = mmio_access_width(ty, *offset)?;
            ctx.load_slot(X_A, ctx.frame.off(*base));
            let off = *offset as u16;
            let (enc, mnem) = match width {
                1 => (encode::enc_ldrb_imm(X_B, X_A, off), "ldrb"),
                2 => (encode::enc_ldrh_imm(X_B, X_A, off), "ldrh"),
                4 => (encode::enc_ldr_w_imm(X_B, X_A, off), "ldr"),
                _ => (encode::enc_ldr_x_imm(X_B, X_A, off), "ldr"),
            };
            let rt = if width == 8 {
                reg_name(X_B)
            } else {
                format!("w{X_B}")
            };
            ctx.push(
                enc,
                format!("{mnem} {rt}, [{}, #{off}]", reg_name(X_A)),
                CostRule::Load,
                Some(X_B),
                &[X_A],
            );
            ctx.store_slot(X_B, ctx.frame.off(*dst));
        }
        // plans/M7.md item G, decision 12: load the driver's vector bit
        // index into an `IrqCap` word. The immediate is patched by layout
        // once the sealed graph's `vector=` is known — identical shape to
        // `Reloc::TurnFrameAddr`/`GroupArenaBase`.
        Inst::LoadIrqVector { dst, driver } => {
            let word = ctx.words.len();
            ctx.load_imm_naive(X_A, 0);
            if let Some(ew) = ctx.words.get_mut(word) {
                ew.text = format!("irq-vector[{}] {}", driver, reg_name(X_A));
            }
            ctx.relocs.push(Reloc::IrqVector {
                word,
                driver: driver.clone(),
            });
            ctx.store_slot(X_A, ctx.frame.off(*dst));
        }
        // plans/M7.md item G, decision 17: live-cell ops through self_ptr.
        Inst::InterruptCellLoadAcquire {
            dst,
            field_off,
            width,
        } => {
            emit_interrupt_cell_addr(ctx, *field_off)?;
            match *width {
                4 => {
                    ctx.push(
                        encode::enc_ldar_w(X_B, X_A),
                        format!("ldar w{}, [{}]", X_B, reg_name(X_A)),
                        CostRule::LoadAcquire,
                        Some(X_B),
                        &[X_A],
                    );
                }
                8 => {
                    ctx.push(
                        encode::enc_ldar_x(X_B, X_A),
                        format!("ldar {}, [{}]", reg_name(X_B), reg_name(X_A)),
                        CostRule::LoadAcquire,
                        Some(X_B),
                        &[X_A],
                    );
                }
                w => {
                    return Err(CodegenError::internal(format!(
                        "InterruptCellLoadAcquire width {w}"
                    )));
                }
            }
            ctx.store_slot(X_B, ctx.frame.off(*dst));
        }
        Inst::InterruptCellStoreRelease {
            field_off,
            width,
            value,
        } => {
            emit_interrupt_cell_addr(ctx, *field_off)?;
            ctx.load_slot(X_B, ctx.frame.off(*value));
            match *width {
                4 => {
                    ctx.push(
                        encode::enc_stlr_w(X_B, X_A),
                        format!("stlr w{}, [{}]", X_B, reg_name(X_A)),
                        CostRule::StoreRelease,
                        Some(X_B),
                        &[X_A],
                    );
                }
                8 => {
                    ctx.push(
                        encode::enc_stlr_x(X_B, X_A),
                        format!("stlr {}, [{}]", reg_name(X_B), reg_name(X_A)),
                        CostRule::StoreRelease,
                        Some(X_B),
                        &[X_A],
                    );
                }
                w => {
                    return Err(CodegenError::internal(format!(
                        "InterruptCellStoreRelease width {w}"
                    )));
                }
            }
        }
        Inst::InterruptCellSwapAcquire {
            dst,
            field_off,
            width,
            value,
        } => {
            let value_off = ctx.frame.off(*value);
            let dst_off = ctx.frame.off(*dst);
            emit_interrupt_cell_rmw(ctx, *field_off, *width, value_off, InterruptCellRmw::Swap)?;
            ctx.store_slot(X_C, dst_off); // old value left in X_C
        }
        Inst::InterruptCellFetchOrRelease {
            dst,
            field_off,
            width,
            value,
        } => {
            let value_off = ctx.frame.off(*value);
            let dst_off = ctx.frame.off(*dst);
            emit_interrupt_cell_rmw(
                ctx,
                *field_off,
                *width,
                value_off,
                InterruptCellRmw::FetchOr,
            )?;
            ctx.store_slot(X_C, dst_off);
        }
        // plans/M15.md item H: one DMB word, no BL.
        // plans/M15.md item K / decision 1098: `--omit-dmb` drops the word
        // (mutation arm of boot-cross-core-publish-acquire).
        Inst::Dmb { option } => {
            if omit_dmb() {
                return Ok(());
            }
            let (enc, mnem) = match option.as_str() {
                "ishst" => (encode::enc_dmb_ishst(), "dmb ishst"),
                "ishld" => (encode::enc_dmb_ishld(), "dmb ishld"),
                other => {
                    return Err(CodegenError::internal(format!(
                        "unknown Dmb option `{other}` (expected ishst|ishld)"
                    )));
                }
            };
            ctx.push(enc, mnem.to_string(), CostRule::Barrier, None, &[]);
        }
        // plans/M7.md item G: sticky store of 1 into the driver's
        // wake-pending word. Level-triggered: a wake before/during/after
        // the bottom half's cell observation remains set until the
        // scheduler clears it after a run that finds the bit still clear
        // on recheck (HVF commit wires that loop).
        Inst::Wake { driver } => {
            let word = ctx.words.len();
            ctx.load_imm_naive(X_A, 0);
            if let Some(ew) = ctx.words.get_mut(word) {
                ew.text = format!("wake-pending[{}] {}", driver, reg_name(X_A));
            }
            ctx.relocs.push(Reloc::WakePending {
                word,
                driver: driver.clone(),
            });
            ctx.load_imm(X_B, 1);
            ctx.push(
                encode::enc_str_x_imm(X_B, X_A, 0),
                format!("str {}, [{}]", reg_name(X_B), reg_name(X_A)),
                CostRule::Store,
                None,
                &[X_B, X_A],
            );
        }
        // plans/M17.md item Es / freeze 4: shared emitters with FlowWir.
        Inst::Now { dst } => {
            emit_now(*dst, ctx);
        }
        Inst::Entropy { dst, n } => emit_entropy(*dst, *n, ctx)?,
        Inst::SlotMapMint { map } => {
            emit_slotmap_mint_id(*map, ctx)?;
        }
        Inst::MmioWrite {
            base,
            offset,
            ty,
            value,
        } => {
            let width = mmio_access_width(ty, *offset)?;
            ctx.load_slot(X_A, ctx.frame.off(*base));
            ctx.load_slot(X_B, ctx.frame.off(*value));
            let off = *offset as u16;
            let (enc, mnem) = match width {
                1 => (encode::enc_strb_imm(X_B, X_A, off), "strb"),
                2 => (encode::enc_strh_imm(X_B, X_A, off), "strh"),
                4 => (encode::enc_str_w_imm(X_B, X_A, off), "str"),
                _ => (encode::enc_str_x_imm(X_B, X_A, off), "str"),
            };
            let rt = if width == 8 {
                reg_name(X_B)
            } else {
                format!("w{X_B}")
            };
            ctx.push(
                enc,
                format!("{mnem} {rt}, [{}, #{off}]", reg_name(X_A)),
                CostRule::Store,
                None,
                &[X_B, X_A],
            );
        }
        Inst::MemLoad {
            dst,
            base,
            offset,
            width,
        } => {
            emit_mem_load(ctx, *dst, *base, *offset, *width)?;
        }
        Inst::MemStore {
            base,
            offset,
            value,
            width,
        } => {
            emit_mem_store(ctx, *base, *offset, *value, *width)?;
        }
        Inst::PtrOffset { dst, base, offset } => {
            ctx.load_slot(X_A, ctx.frame.off(*base));
            if *offset == 0 {
                ctx.store_slot(X_A, ctx.frame.off(*dst));
            } else {
                ctx.load_imm(X_B, *offset as i64);
                ctx.add_reg(X_C, X_A, X_B);
                ctx.store_slot(X_C, ctx.frame.off(*dst));
            }
        }
        Inst::TurnAddrFromId { dst, id } => {
            ctx.load_slot(X_A, ctx.frame.off(*id));
            push_turn_addr_from_id(ctx, X_A, X_B);
            ctx.store_slot(X_A, ctx.frame.off(*dst));
        }
        Inst::Abort { message } => {
            ctx.abort_fixed(message);
        }
    }
    Ok(())
}

/// `Option[TurnId]` → absolute turn-area address via layout relocs.
/// The `- 1` lives here and in `TurnId::index` and nowhere else.
fn push_turn_addr_from_id(ctx: &mut FnCtx, id_reg: u8, scratch: u8) {
    ctx.push(
        encode::enc_sub_imm(id_reg, id_reg, 1, true),
        format!("sub {}, {}, #1", reg_name(id_reg), reg_name(id_reg)),
        CostRule::Alu,
        Some(id_reg),
        &[id_reg],
    );
    let word = ctx.cur_word();
    ctx.load_imm_naive(scratch, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("turn-stride {}", reg_name(scratch));
    }
    ctx.relocs.push(Reloc::TurnStride { word });
    ctx.mul_reg(id_reg, id_reg, scratch);
    let word = ctx.cur_word();
    ctx.load_imm_naive(scratch, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("turns-base {}", reg_name(scratch));
    }
    ctx.relocs.push(Reloc::TurnsBase { word });
    ctx.add_reg(id_reg, scratch, id_reg);
}

fn emit_mem_addr(ctx: &mut FnCtx, base: Temp, offset: u64) {
    ctx.load_slot(X_A, ctx.frame.off(base));
    if offset == 0 {
        return;
    }
    ctx.load_imm(X_B, offset as i64);
    ctx.add_reg(X_A, X_A, X_B);
}

fn emit_mem_load(
    ctx: &mut FnCtx,
    dst: Temp,
    base: Temp,
    offset: u64,
    width: u8,
) -> Result<(), CodegenError> {
    emit_mem_addr(ctx, base, offset);
    let (enc, mnem) = match width {
        1 => (encode::enc_ldrb_imm(X_B, X_A, 0), "ldrb"),
        2 => (encode::enc_ldrh_imm(X_B, X_A, 0), "ldrh"),
        4 => (encode::enc_ldr_w_imm(X_B, X_A, 0), "ldr"),
        8 => (encode::enc_ldr_x_imm(X_B, X_A, 0), "ldr"),
        w => {
            return Err(CodegenError::internal(format!(
                "MemLoad width {w} (want 1/2/4/8)"
            )));
        }
    };
    let rt = if width == 8 {
        reg_name(X_B)
    } else {
        format!("w{X_B}")
    };
    ctx.push(
        enc,
        format!("{mnem} {rt}, [{}]", reg_name(X_A)),
        CostRule::Load,
        Some(X_B),
        &[X_A],
    );
    ctx.store_slot(X_B, ctx.frame.off(dst));
    Ok(())
}

fn emit_mem_store(
    ctx: &mut FnCtx,
    base: Temp,
    offset: u64,
    value: Temp,
    width: u8,
) -> Result<(), CodegenError> {
    emit_mem_addr(ctx, base, offset);
    ctx.load_slot(X_B, ctx.frame.off(value));
    let (enc, mnem) = match width {
        1 => (encode::enc_strb_imm(X_B, X_A, 0), "strb"),
        2 => (encode::enc_strh_imm(X_B, X_A, 0), "strh"),
        4 => (encode::enc_str_w_imm(X_B, X_A, 0), "str"),
        8 => (encode::enc_str_x_imm(X_B, X_A, 0), "str"),
        w => {
            return Err(CodegenError::internal(format!(
                "MemStore width {w} (want 1/2/4/8)"
            )));
        }
    };
    let rt = if width == 8 {
        reg_name(X_B)
    } else {
        format!("w{X_B}")
    };
    ctx.push(
        enc,
        format!("{mnem} {rt}, [{}]", reg_name(X_A)),
        CostRule::Store,
        None,
        &[X_B, X_A],
    );
    Ok(())
}

fn mmio_access_width(ty: &Type, offset: u64) -> Result<u16, CodegenError> {
    let width = match strip_wrappers(ty) {
        Type::U8 => 1,
        Type::U16 => 2,
        Type::U32 => 4,
        Type::U64 | Type::Usize => 8,
        other => {
            return Err(CodegenError::unimplemented(&format!(
                "an MMIO register declared `{}`: this backend emits only the four unsigned \
                 widths (`u8`/`u16`/`u32`/`u64`/`usize`); a signed register would need a \
                 sign-extending load this encoder does not have",
                crate::sema::types::render_type(&other)
            )));
        }
    };
    if offset % width as u64 != 0 {
        return Err(CodegenError::internal(format!(
            "an MMIO register at offset {offset:#x} is not {width}-byte aligned ( \
             `types::check_layouts` already refuses this)"
        )));
    }
    if offset / width as u64 >= 4096 {
        return Err(CodegenError::unimplemented(&format!(
            "an MMIO register at offset {offset:#x}: the unsigned-immediate load/store encoder \
             reaches {} bytes at this width, and no base-plus-register addressing form is \
             emitted yet. That offset",
            4095 * width as u64
        )));
    }
    Ok(width)
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
/// plans/M9.md item C2: write a formatted scalar into a `String[..capacity]`
/// frame slot (length word + `capacity` byte slots). Occupied length is the
/// real digit/bool/char width; unused slots stay zero.
fn emit_format_scalar(
    ctx: &mut FnCtx,
    dst: Temp,
    src: Temp,
    src_ty: &Type,
    capacity: usize,
) -> Result<(), CodegenError> {
    let dst_off = ctx.frame.off(dst);
    let src_off = ctx.frame.off(src);
    // Zero-fill the whole aggregate.
    for i in 0..=capacity {
        ctx.load_imm(X_A, 0);
        ctx.store_slot(X_A, dst_off + 8 * i);
    }
    match src_ty {
        Type::Bool => {
            ctx.load_slot(X_A, src_off);
            let to_false = ctx.emit_skip(SkipKind::Cbz(X_A));
            // "true"
            ctx.load_imm(X_A, 4);
            ctx.store_slot(X_A, dst_off);
            for (i, b) in b"true".iter().enumerate() {
                ctx.load_imm(X_A, i128::from(*b) as i64);
                ctx.store_slot(X_A, dst_off + 8 * (1 + i));
            }
            let done = ctx.emit_skip(SkipKind::Cond(Cond::Al));
            ctx.patch_skip(to_false, SkipKind::Cbz(X_A));
            // "false"
            ctx.load_imm(X_A, 5);
            ctx.store_slot(X_A, dst_off);
            for (i, b) in b"false".iter().enumerate() {
                ctx.load_imm(X_A, i128::from(*b) as i64);
                ctx.store_slot(X_A, dst_off + 8 * (1 + i));
            }
            ctx.patch_skip(done, SkipKind::Cond(Cond::Al));
            Ok(())
        }
        Type::Char => {
            // UTF-8 encode one scalar value held as a codepoint in the slot.
            ctx.load_slot(X_A, src_off); // codepoint
            // 1-byte ASCII fast path.
            ctx.load_imm(X_B, 0x80);
            ctx.cmp_reg(X_A, X_B);
            let not_ascii = ctx.emit_skip(SkipKind::Cond(Cond::Cs));
            ctx.load_imm(X_B, 1);
            ctx.store_slot(X_B, dst_off);
            ctx.store_slot(X_A, dst_off + 8);
            let done = ctx.emit_skip(SkipKind::Cond(Cond::Al));
            ctx.patch_skip(not_ascii, SkipKind::Cond(Cond::Cs));
            // 2-byte: U+0080..U+07FF
            ctx.load_imm(X_B, 0x800);
            ctx.cmp_reg(X_A, X_B);
            let not_2 = ctx.emit_skip(SkipKind::Cond(Cond::Cs));
            // b0 = 0xC0 | (cp >> 6); b1 = 0x80 | (cp & 0x3F)
            ctx.push(
                encode::enc_lsr_imm(X_C, X_A, 6, true),
                format!("lsr {}, {}, #6", reg_name(X_C), reg_name(X_A)),
                CostRule::Alu,
                Some(X_C),
                &[X_A],
            );
            ctx.load_imm(X_D, 0xC0);
            ctx.orr_reg(X_C, X_C, X_D);
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_E, X_A, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_E, X_E, X_D);
            ctx.load_imm(X_B, 2);
            ctx.store_slot(X_B, dst_off);
            ctx.store_slot(X_C, dst_off + 8);
            ctx.store_slot(X_E, dst_off + 16);
            let done2 = ctx.emit_skip(SkipKind::Cond(Cond::Al));
            ctx.patch_skip(not_2, SkipKind::Cond(Cond::Cs));
            // 3-byte: U+0800..U+FFFF (enough for common Format uses; 4-byte
            // scalars still fit the bound of 4 and use the same path with
            // a wider check below).
            ctx.load_imm(X_B, 0x10000);
            ctx.cmp_reg(X_A, X_B);
            let not_3 = ctx.emit_skip(SkipKind::Cond(Cond::Cs));
            // b0 = 0xE0 | (cp >> 12); b1 = 0x80 | ((cp >> 6) & 0x3F); b2 = 0x80 | (cp & 0x3F)
            ctx.push(
                encode::enc_lsr_imm(X_C, X_A, 12, true),
                format!("lsr {}, {}, #12", reg_name(X_C), reg_name(X_A)),
                CostRule::Alu,
                Some(X_C),
                &[X_A],
            );
            ctx.load_imm(X_D, 0xE0);
            ctx.orr_reg(X_C, X_C, X_D);
            ctx.push(
                encode::enc_lsr_imm(X_E, X_A, 6, true),
                format!("lsr {}, {}, #6", reg_name(X_E), reg_name(X_A)),
                CostRule::Alu,
                Some(X_E),
                &[X_A],
            );
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_E, X_E, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_E, X_E, X_D);
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_F, X_A, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_F, X_F, X_D);
            ctx.load_imm(X_B, 3);
            ctx.store_slot(X_B, dst_off);
            ctx.store_slot(X_C, dst_off + 8);
            ctx.store_slot(X_E, dst_off + 16);
            ctx.store_slot(X_F, dst_off + 24);
            let done3 = ctx.emit_skip(SkipKind::Cond(Cond::Al));
            ctx.patch_skip(not_3, SkipKind::Cond(Cond::Cs));
            // 4-byte
            ctx.push(
                encode::enc_lsr_imm(X_C, X_A, 18, true),
                format!("lsr {}, {}, #18", reg_name(X_C), reg_name(X_A)),
                CostRule::Alu,
                Some(X_C),
                &[X_A],
            );
            ctx.load_imm(X_D, 0xF0);
            ctx.orr_reg(X_C, X_C, X_D);
            ctx.push(
                encode::enc_lsr_imm(X_E, X_A, 12, true),
                format!("lsr {}, {}, #12", reg_name(X_E), reg_name(X_A)),
                CostRule::Alu,
                Some(X_E),
                &[X_A],
            );
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_E, X_E, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_E, X_E, X_D);
            ctx.push(
                encode::enc_lsr_imm(X_F, X_A, 6, true),
                format!("lsr {}, {}, #6", reg_name(X_F), reg_name(X_A)),
                CostRule::Alu,
                Some(X_F),
                &[X_A],
            );
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_F, X_F, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_F, X_F, X_D);
            // reuse X_B for last byte
            ctx.load_imm(X_D, 0x3F);
            ctx.and_reg(X_B, X_A, X_D);
            ctx.load_imm(X_D, 0x80);
            ctx.orr_reg(X_B, X_B, X_D);
            if capacity < 4 {
                return Err(CodegenError::internal(
                    "FormatScalar char capacity < 4".to_string(),
                ));
            }
            ctx.load_imm(X_D, 4);
            ctx.store_slot(X_D, dst_off);
            ctx.store_slot(X_C, dst_off + 8);
            ctx.store_slot(X_E, dst_off + 16);
            ctx.store_slot(X_F, dst_off + 24);
            ctx.store_slot(X_B, dst_off + 32);
            ctx.patch_skip(done3, SkipKind::Cond(Cond::Al));
            ctx.patch_skip(done2, SkipKind::Cond(Cond::Al));
            ctx.patch_skip(done, SkipKind::Cond(Cond::Al));
            Ok(())
        }
        Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::Isize => {
            let signed = matches!(
                src_ty,
                Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Isize
            );
            if capacity == 0 {
                return Err(CodegenError::internal(
                    "FormatScalar integer capacity is 0".to_string(),
                ));
            }
            ctx.load_slot(X_A, src_off); // value
            ctx.load_imm(X_F, 0); // negative flag
            if signed {
                ctx.push_flags(
                    encode::enc_cmp_reg(X_A, X_ZR, true),
                    format!("cmp {}, xzr", reg_name(X_A)),
                    CostRule::Alu,
                    None,
                    &[X_A, X_ZR],
                    FlagEffect::Write,
                );
                let nonneg = ctx.emit_skip(SkipKind::Cond(Cond::Ge));
                ctx.load_imm(X_F, 1);
                ctx.push(
                    encode::enc_sub_reg(X_A, X_ZR, X_A, true),
                    format!("neg {}, {}", reg_name(X_A), reg_name(X_A)),
                    CostRule::Alu,
                    Some(X_A),
                    &[X_ZR, X_A],
                );
                ctx.patch_skip(nonneg, SkipKind::Cond(Cond::Ge));
            }
            // Zero → "0" (with optional leading '-').
            let nonzero = ctx.emit_skip(SkipKind::Cbnz(X_A));
            // Write '0'
            ctx.load_imm(X_B, b'0' as i64);
            ctx.store_slot(X_B, dst_off + 8);
            ctx.load_imm(X_B, 1);
            // if negative: write '-' at [0], '0' at [1], len=2
            ctx.push_flags(
                encode::enc_cmp_reg(X_F, X_ZR, true),
                format!("cmp {}, xzr", reg_name(X_F)),
                CostRule::Alu,
                None,
                &[X_F, X_ZR],
                FlagEffect::Write,
            );
            let no_sign0 = ctx.emit_skip(SkipKind::Cond(Cond::Eq));
            ctx.load_imm(X_C, b'-' as i64);
            ctx.store_slot(X_C, dst_off + 8);
            ctx.load_imm(X_C, b'0' as i64);
            ctx.store_slot(X_C, dst_off + 16);
            ctx.load_imm(X_B, 2);
            ctx.patch_skip(no_sign0, SkipKind::Cond(Cond::Eq));
            ctx.store_slot(X_B, dst_off);
            let done0 = ctx.emit_skip(SkipKind::Cond(Cond::Al));
            ctx.patch_skip(nonzero, SkipKind::Cbnz(X_A));

            // Digit extraction into the high end of the data area.
            // X_I = capacity (write index); X_N = digit count; X_A = abs value
            ctx.load_imm(X_I_REG, capacity as i64);
            ctx.load_imm(X_N_REG, 0);
            let loop_start = ctx.cur_word();
            // dig = X_A % 10; X_A /= 10
            ctx.load_imm(X_B, 10);
            ctx.push(
                encode::enc_udiv(X_C, X_A, X_B, true),
                format!(
                    "udiv {}, {}, {}",
                    reg_name(X_C),
                    reg_name(X_A),
                    reg_name(X_B)
                ),
                CostRule::Udiv,
                Some(X_C),
                &[X_A, X_B],
            );
            ctx.push(
                encode::enc_msub(X_D, X_C, X_B, X_A, true),
                format!(
                    "msub {}, {}, {}, {}",
                    reg_name(X_D),
                    reg_name(X_C),
                    reg_name(X_B),
                    reg_name(X_A)
                ),
                CostRule::Mul,
                Some(X_D),
                // `msub Xd, Xn, Xm, Xa` = `Xa - Xn*Xm`: the accumulator
                // `X_A` is a source (plans/M20.md item E).
                &[X_C, X_B, X_A],
            );
            ctx.load_imm(X_B, b'0' as i64);
            ctx.add_reg(X_D, X_D, X_B);
            ctx.push(
                encode::enc_sub_imm(X_I_REG, X_I_REG, 1, true),
                format!("sub {}, {}, #1", reg_name(X_I_REG), reg_name(X_I_REG)),
                CostRule::Alu,
                Some(X_I_REG),
                &[X_I_REG],
            );
            // store digit at data[X_I]: addr = dst_base + 8 + X_I*8
            ctx.addr_of_slot(X_E, dst_off + 8);
            ctx.load_imm(X_B, 8);
            ctx.mul_reg(X_B, X_I_REG, X_B);
            ctx.add_reg(X_E, X_E, X_B);
            ctx.store_ptr(X_D, X_E, 0);
            ctx.push(
                encode::enc_add_imm(X_N_REG, X_N_REG, 1, true),
                format!("add {}, {}, #1", reg_name(X_N_REG), reg_name(X_N_REG)),
                CostRule::Alu,
                Some(X_N_REG),
                &[X_N_REG],
            );
            ctx.push(
                encode::enc_mov_reg(X_A, X_C, true),
                format!("mov {}, {}", reg_name(X_A), reg_name(X_C)),
                CostRule::Alu,
                Some(X_A),
                &[X_C],
            );
            // loop while X_A != 0
            let here = ctx.cur_word();
            let back = (loop_start as i64 - here as i64) as i32 * 4;
            ctx.push(
                encode::enc_cbnz(X_A, back, true),
                format!("cbnz {}, #{back}", reg_name(X_A)),
                CostRule::Branch,
                None,
                &[X_A],
            );

            // Optional leading '-': decrement X_I, store '-', bump X_N
            ctx.push_flags(
                encode::enc_cmp_reg(X_F, X_ZR, true),
                format!("cmp {}, xzr", reg_name(X_F)),
                CostRule::Alu,
                None,
                &[X_F, X_ZR],
                FlagEffect::Write,
            );
            let no_sign = ctx.emit_skip(SkipKind::Cond(Cond::Eq));
            ctx.push(
                encode::enc_sub_imm(X_I_REG, X_I_REG, 1, true),
                format!("sub {}, {}, #1", reg_name(X_I_REG), reg_name(X_I_REG)),
                CostRule::Alu,
                Some(X_I_REG),
                &[X_I_REG],
            );
            ctx.load_imm(X_D, b'-' as i64);
            ctx.addr_of_slot(X_E, dst_off + 8);
            ctx.load_imm(X_B, 8);
            ctx.mul_reg(X_B, X_I_REG, X_B);
            ctx.add_reg(X_E, X_E, X_B);
            ctx.store_ptr(X_D, X_E, 0);
            ctx.push(
                encode::enc_add_imm(X_N_REG, X_N_REG, 1, true),
                format!("add {}, {}, #1", reg_name(X_N_REG), reg_name(X_N_REG)),
                CostRule::Alu,
                Some(X_N_REG),
                &[X_N_REG],
            );
            ctx.patch_skip(no_sign, SkipKind::Cond(Cond::Eq));

            // Shift data[X_I ..) down to data[0 .. X_N)
            ctx.load_imm(X_A, 0); // j
            let shift_start = ctx.cur_word();
            ctx.cmp_reg(X_A, X_N_REG);
            let shift_done = ctx.emit_skip(SkipKind::Cond(Cond::Cs));
            // load data[X_I + j]
            ctx.add_reg(X_B, X_I_REG, X_A);
            ctx.addr_of_slot(X_E, dst_off + 8);
            ctx.load_imm(X_C, 8);
            ctx.mul_reg(X_D, X_B, X_C);
            ctx.add_reg(X_E, X_E, X_D);
            ctx.load_ptr(X_F, X_E, 0);
            // store data[j]
            ctx.addr_of_slot(X_E, dst_off + 8);
            ctx.mul_reg(X_D, X_A, X_C);
            ctx.add_reg(X_E, X_E, X_D);
            ctx.store_ptr(X_F, X_E, 0);
            ctx.push(
                encode::enc_add_imm(X_A, X_A, 1, true),
                format!("add {}, {}, #1", reg_name(X_A), reg_name(X_A)),
                CostRule::Alu,
                Some(X_A),
                &[X_A],
            );
            let here = ctx.cur_word();
            let back = (shift_start as i64 - here as i64) as i32 * 4;
            ctx.push(
                encode::enc_b(back),
                format!("b #{back}"),
                CostRule::Branch,
                None,
                &[],
            );
            ctx.patch_skip(shift_done, SkipKind::Cond(Cond::Cs));

            // Zero the remaining data slots beyond X_N (unrolled).
            for i in 0..capacity {
                ctx.load_imm(X_A, i as i64);
                ctx.cmp_reg(X_A, X_N_REG);
                let keep = ctx.emit_skip(SkipKind::Cond(Cond::Cc)); // i < n → keep
                ctx.load_imm(X_B, 0);
                ctx.store_slot(X_B, dst_off + 8 * (1 + i));
                ctx.patch_skip(keep, SkipKind::Cond(Cond::Cc));
            }
            ctx.store_slot(X_N_REG, dst_off);
            ctx.patch_skip(done0, SkipKind::Cond(Cond::Al));
            Ok(())
        }
        other => Err(CodegenError::internal(format!(
            "FormatScalar for non-scalar type `{}`",
            crate::sema::types::render_type(other)
        ))),
    }
}

/// Scratch aliases used only inside [`emit_format_scalar`]'s digit loop —
/// kept clear of `X_A`..`X_F` where those are live mid-step.
const X_I_REG: u8 = 15;
const X_N_REG: u8 = 16;

/// plans/M9.md item C2: concatenate two String aggregates.
fn emit_string_concat(
    ctx: &mut FnCtx,
    dst: Temp,
    lhs: Temp,
    rhs: Temp,
    lhs_cap: usize,
    rhs_cap: usize,
) {
    let dst_off = ctx.frame.off(dst);
    let lhs_off = ctx.frame.off(lhs);
    let rhs_off = ctx.frame.off(rhs);
    let out_cap = lhs_cap + rhs_cap;
    // Zero-fill.
    for i in 0..=out_cap {
        ctx.load_imm(X_A, 0);
        ctx.store_slot(X_A, dst_off + 8 * i);
    }
    // out_len = lhs_len + rhs_len
    ctx.load_slot(X_A, lhs_off); // lhs_len
    ctx.load_slot(X_B, rhs_off); // rhs_len
    ctx.add_reg(X_C, X_A, X_B);
    ctx.store_slot(X_C, dst_off);
    // Copy lhs occupied bytes (unrolled against capacity; gated by lhs_len).
    for i in 0..lhs_cap {
        ctx.load_imm(X_D, i as i64);
        ctx.cmp_reg(X_D, X_A);
        let skip = ctx.emit_skip(SkipKind::Cond(Cond::Cs)); // i >= lhs_len
        ctx.load_slot(X_E, lhs_off + 8 * (1 + i));
        ctx.store_slot(X_E, dst_off + 8 * (1 + i));
        ctx.patch_skip(skip, SkipKind::Cond(Cond::Cs));
    }
    // Copy rhs occupied bytes to dst[lhs_len + j].
    for j in 0..rhs_cap {
        ctx.load_imm(X_D, j as i64);
        ctx.cmp_reg(X_D, X_B);
        let skip = ctx.emit_skip(SkipKind::Cond(Cond::Cs)); // j >= rhs_len
        ctx.load_slot(X_E, rhs_off + 8 * (1 + j));
        // dest index = lhs_len + j → byte off = 8 + 8*(lhs_len+j)
        ctx.addr_of_slot(X_F, dst_off + 8);
        ctx.add_reg(X_C, X_A, X_D);
        ctx.load_imm(X_D, 8);
        ctx.mul_reg(X_D, X_C, X_D);
        ctx.add_reg(X_F, X_F, X_D);
        ctx.store_ptr(X_E, X_F, 0);
        ctx.patch_skip(skip, SkipKind::Cond(Cond::Cs));
    }
}

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
    ctx.cmp_reg(X_A, X_B);
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
    ctx.mul_reg(X_E, X_A, X_D);
    ctx.add_reg(out_reg, out_reg, X_E);
}

/// plans/M10.md item B4 / decisions 595–596: address of packed byte
/// `handle.base[index]`, bounds-checked against the handle's own `len`
/// word (runtime, not a compile-time N). Abort shape matches
/// `emit_index_addr` (cmp + `bl __wrela_abort_val`).
fn emit_bytes_index_addr(
    ctx: &mut FnCtx,
    handle_off: usize,
    index_off: usize,
    out_reg: u8,
) -> Result<(), CodegenError> {
    // X_A = index; X_B = handle.len; compare; on fail abort with the
    // live index (length rendered as the handle's own len word).
    ctx.load_slot(X_A, index_off);
    ctx.load_slot(X_B, handle_off + 8);
    ctx.cmp_reg(X_A, X_B);
    let skip = ctx.emit_skip(SkipKind::Cond(Cond::Cc));
    // Suffix embeds the live length so the diagnostic matches IndexGet's
    // `"index {i} out of bounds (length {len})"` shape; the length half
    // is written through a small scratch because abort_val takes one
    // value register. Re-use X_B (still the len) after stashing the index
    // message's value register — abort_val's own contract takes X_A as
    // the interpolated value when we pass it; here we pass X_A = index.
    ctx.abort_val("index ", X_A, false, " out of bounds (Bytes)");
    ctx.patch_skip(skip, SkipKind::Cond(Cond::Cc));
    // out = handle.base + index (elem_stride = 1 packed byte).
    ctx.load_slot(out_reg, handle_off);
    ctx.add_reg(out_reg, out_reg, X_A);
    Ok(())
}

/// plans/M10.md item B1: address of `placed_base[field_offset + i*stride]`
/// with the same bounds-check abort shape as `emit_index_addr`.
fn emit_placed_index_addr(
    ctx: &mut FnCtx,
    base_off: usize,
    field_offset: u64,
    index_off: usize,
    len: usize,
    elem_stride: u64,
    out_reg: u8,
) {
    ctx.load_slot(X_A, index_off);
    ctx.load_imm(X_B, len as i64);
    ctx.cmp_reg(X_A, X_B);
    let skip = ctx.emit_skip(SkipKind::Cond(Cond::Cc));
    ctx.abort_val(
        "index ",
        X_A,
        false,
        &format!(" out of bounds (length {len})"),
    );
    ctx.patch_skip(skip, SkipKind::Cond(Cond::Cc));
    ctx.load_slot(out_reg, base_off);
    if field_offset != 0 {
        ctx.load_imm(X_D, field_offset as i64);
        ctx.add_reg(out_reg, out_reg, X_D);
    }
    ctx.load_imm(X_D, elem_stride as i64);
    ctx.mul_reg(X_E, X_A, X_D);
    ctx.add_reg(out_reg, out_reg, X_E);
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
            match op {
                BinOp::Mul => CostRule::Mul,
                _ => CostRule::Alu,
            },
            Some(X_C),
            &[X_A, X_B],
        );
        let (min, max) = int_bounds_i64(ty).unwrap();
        ctx.check_bounds_i64_or_abort(X_C, min, max, abort);
        ctx.store_slot(X_C, ctx.frame.off(dst));
        return Ok(());
    }
    match op {
        BinOp::Add => {
            ctx.push_flags(
                encode::enc_adds_reg(X_C, X_A, X_B, true),
                format!(
                    "adds {}, {}, {}",
                    reg_name(X_C),
                    reg_name(X_A),
                    reg_name(X_B)
                ),
                CostRule::Alu,
                Some(X_C),
                &[X_A, X_B],
                FlagEffect::Write,
            );
            let fail = if signed { Cond::Vs } else { Cond::Cs };
            ctx.check_flags_or_abort(fail, abort);
        }
        BinOp::Sub => {
            ctx.push_flags(
                encode::enc_subs_reg(X_C, X_A, X_B, true),
                format!(
                    "subs {}, {}, {}",
                    reg_name(X_C),
                    reg_name(X_A),
                    reg_name(X_B)
                ),
                CostRule::Alu,
                Some(X_C),
                &[X_A, X_B],
                FlagEffect::Write,
            );
            let fail = if signed { Cond::Vs } else { Cond::Cc };
            ctx.check_flags_or_abort(fail, abort);
        }
        BinOp::Mul => {
            ctx.mul_reg(X_C, X_A, X_B);
            if signed {
                ctx.push(
                    encode::enc_smulh(X_D, X_A, X_B),
                    format!(
                        "smulh {}, {}, {}",
                        reg_name(X_D),
                        reg_name(X_A),
                        reg_name(X_B)
                    ),
                    CostRule::MulHigh,
                    Some(X_D),
                    &[X_A, X_B],
                );
                ctx.push(
                    encode::enc_asr_imm(X_E, X_C, 63, true),
                    format!("asr {}, {}, #63", reg_name(X_E), reg_name(X_C)),
                    CostRule::Alu,
                    Some(X_E),
                    &[X_C],
                );
                ctx.cmp_reg(X_D, X_E);
            } else {
                ctx.push(
                    encode::enc_umulh(X_D, X_A, X_B),
                    format!(
                        "umulh {}, {}, {}",
                        reg_name(X_D),
                        reg_name(X_A),
                        reg_name(X_B)
                    ),
                    CostRule::MulHigh,
                    Some(X_D),
                    &[X_A, X_B],
                );
                ctx.push_flags(
                    encode::enc_cmp_reg(X_D, X_ZR, true),
                    format!("cmp {}, xzr", reg_name(X_D)),
                    CostRule::Alu,
                    None,
                    &[X_D, X_ZR],
                    FlagEffect::Write,
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
    // plans/M20.md item D: the wrapping `MUL` is the X-form
    // multiply-accumulate group (SOG §3.6: lat 4, thru 1/3, port M, and it
    // stalls pipe M 2 extra cycles), not the 1-cycle integer ALU group the
    // shared push tagged all three arms as.
    let (enc, mnem, rule) = match op {
        BinOp::AddW => (
            encode::enc_add_reg(X_C, X_A, X_B, true),
            "add",
            CostRule::Alu,
        ),
        BinOp::SubW => (
            encode::enc_sub_reg(X_C, X_A, X_B, true),
            "sub",
            CostRule::Alu,
        ),
        BinOp::MulW => (encode::enc_mul(X_C, X_A, X_B, true), "mul", CostRule::Mul),
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
        rule,
        None,
        &[],
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
    ctx.push_flags(
        encode::enc_cmp_reg(X_B, X_ZR, true),
        format!("cmp {}, xzr", reg_name(X_B)),
        CostRule::Alu,
        None,
        &[X_B, X_ZR],
        FlagEffect::Write,
    );
    let skip_zero = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
    ctx.abort_fixed(abort_zero);
    ctx.patch_skip(skip_zero, SkipKind::Cond(Cond::Ne));
    if signed && op == BinOp::Div {
        let (min, _) = int_bounds_i64(ty).unwrap();
        ctx.load_imm(X_D, min);
        ctx.cmp_reg(X_A, X_D);
        let skip_a = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
        ctx.load_imm(X_E, -1);
        ctx.cmp_reg(X_B, X_E);
        let skip_b = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
        ctx.abort_fixed(abort_overflow);
        ctx.patch_skip(skip_a, SkipKind::Cond(Cond::Ne));
        ctx.patch_skip(skip_b, SkipKind::Cond(Cond::Ne));
    }
    // plans/M20.md item D: X-form (`sf = true`) divide, SOG §3.6 — 5-20
    // cycles on pipe M, not the 1-cycle integer ALU group these two sites
    // were tagged as before the A76 table distinguished them.
    //
    // plans/M20.md item E: this site used to push `dst = None, srcs = &[]`,
    // so a 20-cycle divide declared **no dependence edge** and nothing
    // downstream ever waited on its result — a genuine under-cost in the
    // one direction 04 §5 forbids. The quotient lands in `X_C` and the
    // operands are `X_A` (dividend) / `X_B` (divisor).
    let (enc, mnem, rule) = if signed {
        (
            encode::enc_sdiv(X_C, X_A, X_B, true),
            "sdiv",
            CostRule::Sdiv,
        )
    } else {
        (
            encode::enc_udiv(X_C, X_A, X_B, true),
            "udiv",
            CostRule::Udiv,
        )
    };
    ctx.push(
        enc,
        format!(
            "{mnem} {}, {}, {}",
            reg_name(X_C),
            reg_name(X_A),
            reg_name(X_B)
        ),
        rule,
        Some(X_C),
        &[X_A, X_B],
    );
    if op == BinOp::Rem {
        // `msub Xd, Xn, Xm, Xa` computes `Xa - Xn*Xm`, so the accumulator
        // `X_A` (the dividend) is a source too — it was missing here, and
        // at the itoa site below, which under-declared the edge from the
        // divide's own inputs (plans/M20.md item E).
        ctx.push(
            encode::enc_msub(X_C, X_C, X_B, X_A, true),
            format!(
                "msub {}, {}, {}, {}",
                reg_name(X_C),
                reg_name(X_C),
                reg_name(X_B),
                reg_name(X_A)
            ),
            CostRule::Mul,
            Some(X_C),
            &[X_C, X_B, X_A],
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
    ctx.cmp_reg(X_B, X_D);
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
            CostRule::Alu,
            Some(X_C),
            &[X_A],
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
            CostRule::Alu,
            Some(X_D),
            &[X_D, X_B],
        );
        ctx.push(
            encode::enc_lsr_reg(X_E, X_C, X_D, true),
            format!(
                "lsr {}, {}, {}",
                reg_name(X_E),
                reg_name(X_C),
                reg_name(X_D)
            ),
            CostRule::Alu,
            Some(X_E),
            &[X_C, X_D],
        );
        ctx.push_flags(
            encode::enc_cmp_reg(X_E, X_ZR, true),
            format!("cmp {}, xzr", reg_name(X_E)),
            CostRule::Alu,
            None,
            &[X_E, X_ZR],
            FlagEffect::Write,
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
            CostRule::Alu,
            Some(X_F),
            &[X_A, X_B],
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
            CostRule::Alu,
            None,
            &[],
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
            ctx.push_flags(
                encode::enc_cmp_reg(X_A, X_ZR, true),
                format!("cmp {}, xzr", reg_name(X_A)),
                CostRule::Alu,
                None,
                &[X_A, X_ZR],
                FlagEffect::Write,
            );
            let skip = ctx.emit_skip(SkipKind::Cond(Cond::Ge));
            ctx.abort_fixed(abort);
            ctx.patch_skip(skip, SkipKind::Cond(Cond::Ge));
        }
    } else if tbits == 64 && tsigned {
        if !ssigned && sbits == 64 {
            ctx.push_flags(
                encode::enc_cmp_reg(X_A, X_ZR, true),
                format!("cmp {}, xzr", reg_name(X_A)),
                CostRule::Alu,
                None,
                &[X_A, X_ZR],
                FlagEffect::Write,
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
        CostRule::Alu,
        Some(X_C),
        &[X_A],
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
        CostRule::Alu,
        Some(X_SP),
        &[X_SP],
    );
    ctx.store_slot(X_LR, frame.lr_off);
    let mut next_reg = 0u8;
    if let Some((self_temp, mode)) = f.receiver {
        let self_ty = &f.temp_types[self_temp.0];
        // One ABI rule: scalar → xN; non-scalar (or `mut`) → pointer.
        if is_aggregate(self_ty) || mode == AccessMode::Mut {
            let self_ptr_off = frame
                .self_ptr_off
                .ok_or_else(|| CodegenError::internal("receiver present but no self_ptr slot"))?;
            ctx.store_slot(next_reg, self_ptr_off);
            // Never place `InterruptCell` words in the frame copy — ops
            // address the live `self_ptr` cell; copying them in would only
            // create a stale shadow the epilogue must then carefully skip.
            copy_self_fields_skipping_interrupt_cells(
                f,
                frame,
                self_temp,
                ctx,
                SelfFieldCopy::LiveToFrame,
            )?;
        } else {
            ctx.store_slot(next_reg, frame.off(self_temp));
        }
        next_reg += 1;
    }
    let mut mut_ptr_iter = frame.mut_param_ptr_offs.iter();
    for (p, mode) in &f.params {
        if next_reg > 8 {
            return Err(CodegenError::unimplemented("more than 8 call arguments"));
        }
        let ty = &f.temp_types[p.0];
        // Aggregates and `mut` params (even scalars) arrive as pointers
        // (plans/M9.md item CC): copy in, and for `mut` also save the
        // pointer for the epilogue write-back.
        if is_aggregate(ty) || *mode == AccessMode::Mut {
            if *mode == AccessMode::Mut {
                let (pt, ptr_off) = mut_ptr_iter.next().ok_or_else(|| {
                    CodegenError::internal("mut param missing from frame.mut_param_ptr_offs")
                })?;
                if *pt != *p {
                    return Err(CodegenError::internal(
                        "mut_param_ptr_offs order disagrees with MwirFn::params",
                    ));
                }
                ctx.store_slot(next_reg, *ptr_off);
            }
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
    if mut_ptr_iter.next().is_some() {
        return Err(CodegenError::internal(
            "frame.mut_param_ptr_offs has more entries than Mut params",
        ));
    }
    if let Some(ret_ptr_off) = frame.ret_ptr_off {
        ctx.store_slot(8, ret_ptr_off);
    }
    Ok(())
}

fn emit_epilogue(f: &MwirFn, frame: &Frame, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    if let Some((self_temp, mode)) = f.receiver {
        if mode == AccessMode::Mut {
            copy_self_fields_skipping_interrupt_cells(
                f,
                frame,
                self_temp,
                ctx,
                SelfFieldCopy::FrameToLive,
            )?;
        }
    }
    // Non-receiver `mut` params: copy the local slot back through the
    // saved incoming pointer (02-language.md §5.1 / plans/M9.md item CC).
    // No InterruptCell special-case — those cells live only on `self`.
    for (p, ptr_off) in &frame.mut_param_ptr_offs {
        ctx.load_slot(X_A, *ptr_off);
        let size = frame.size_of_temp(*p);
        let src_off = frame.off(*p);
        let mut w = 0;
        while w < size {
            ctx.load_slot(X_B, src_off + w);
            ctx.store_ptr(X_B, X_A, w);
            w += 8;
        }
    }
    ctx.load_slot(X_LR, frame.lr_off);
    ctx.push(
        encode::enc_add_imm(X_SP, X_SP, frame.size as u16, true),
        format!("add sp, sp, #{}", frame.size),
        CostRule::Alu,
        Some(X_SP),
        &[X_SP],
    );
    ctx.push(
        encode::enc_ret(X_LR),
        "ret".to_string(),
        CostRule::Branch,
        None,
        &[X_LR],
    );
    Ok(())
}

// --- plans/M7.md item G, decision 17: InterruptCell live-cell addressing ---

enum InterruptCellRmw {
    Swap,
    FetchOr,
}

/// `X_A = self_ptr + field_off`. Requires a receiver (self_ptr_save).
fn emit_interrupt_cell_addr(ctx: &mut FnCtx, field_off: usize) -> Result<(), CodegenError> {
    let self_ptr_off = ctx.frame.self_ptr_off.ok_or_else(|| {
        CodegenError::internal("InterruptCell op needs a receiver (self_ptr slot)")
    })?;
    ctx.load_slot(X_A, self_ptr_off);
    if field_off != 0 {
        if field_off > 4095 {
            return Err(CodegenError::unimplemented(
                "InterruptCell field_off above add-immediate range",
            ));
        }
        ctx.push(
            encode::enc_add_imm(X_A, X_A, field_off as u16, true),
            format!("add {}, {}, #{field_off}", reg_name(X_A), reg_name(X_A)),
            CostRule::Alu,
            Some(X_A),
            &[X_A],
        );
    }
    Ok(())
}

/// Interrupt-atomic RMW. Leaves the previous cell value in `X_C`.
///
/// Emits `LDAR` / compute / `STLR`, **not** `LDAXR`/`STLXR`. 06 §4
/// delivers vectors only at compiler-emitted checkpoints; revision 0.1
/// is single-core with no nesting (03 §6). No checkpoint is emitted
/// inside this sequence, so no same-core observer can interleave with
/// the RMW — acquire/release alone give the interrupt-atomicity the
/// cell promises. An exclusive pair would be needed if a second core or
/// a nested ISR could clear a monitor mid-RMW; neither exists here.
///
/// (HVF probe, plans/M7.md item G: `LDAXR` against guest DRAM took a
/// data abort on the flagship host; the non-exclusive form is also the
/// one the machine's own delivery rule makes sufficient.)
fn emit_interrupt_cell_rmw(
    ctx: &mut FnCtx,
    field_off: usize,
    width: u8,
    value_off: usize,
    kind: InterruptCellRmw,
) -> Result<(), CodegenError> {
    emit_interrupt_cell_addr(ctx, field_off)?;
    ctx.load_slot(X_B, value_off);
    match width {
        4 => {
            ctx.push(
                encode::enc_ldar_w(X_C, X_A),
                format!("ldar w{}, [{}]", X_C, reg_name(X_A)),
                CostRule::LoadAcquire,
                Some(X_C),
                &[X_A],
            );
            match kind {
                InterruptCellRmw::Swap => {
                    ctx.push(
                        encode::enc_stlr_w(X_B, X_A),
                        format!("stlr w{}, [{}]", X_B, reg_name(X_A)),
                        CostRule::StoreRelease,
                        Some(X_B),
                        &[X_A],
                    );
                }
                InterruptCellRmw::FetchOr => {
                    ctx.push(
                        encode::enc_orr_reg(X_D, X_C, X_B, false),
                        format!("orr w{}, w{}, w{}", X_D, X_C, X_B),
                        CostRule::Alu,
                        Some(X_D),
                        &[X_C, X_B],
                    );
                    ctx.push(
                        encode::enc_stlr_w(X_D, X_A),
                        format!("stlr w{}, [{}]", X_D, reg_name(X_A)),
                        CostRule::StoreRelease,
                        Some(X_D),
                        &[X_A],
                    );
                }
            }
        }
        8 => {
            ctx.push(
                encode::enc_ldar_x(X_C, X_A),
                format!("ldar {}, [{}]", reg_name(X_C), reg_name(X_A)),
                CostRule::LoadAcquire,
                Some(X_C),
                &[X_A],
            );
            match kind {
                InterruptCellRmw::Swap => {
                    ctx.push(
                        encode::enc_stlr_x(X_B, X_A),
                        format!("stlr {}, [{}]", reg_name(X_B), reg_name(X_A)),
                        CostRule::StoreRelease,
                        Some(X_B),
                        &[X_A],
                    );
                }
                InterruptCellRmw::FetchOr => {
                    ctx.orr_reg(X_D, X_C, X_B);
                    ctx.push(
                        encode::enc_stlr_x(X_D, X_A),
                        format!("stlr {}, [{}]", reg_name(X_D), reg_name(X_A)),
                        CostRule::StoreRelease,
                        Some(X_D),
                        &[X_A],
                    );
                }
            }
        }
        w => {
            return Err(CodegenError::internal(format!(
                "InterruptCell RMW width {w}"
            )));
        }
    }
    Ok(())
}

/// Direction of a `mut self` field walk that never touches `InterruptCell`
/// words — those live only at `self_ptr + field_off` (decision 17).
enum SelfFieldCopy {
    /// Prologue: live aggregate → frame slots (skip InterruptCell holes).
    LiveToFrame,
    /// Epilogue: frame slots → live aggregate (same skip — never stomp an
    /// ISR update that landed mid-turn).
    FrameToLive,
}

/// Shared prologue/epilogue walk: copy every non-`InterruptCell` field
/// word; leave InterruptCell frame holes alone so they are never a
/// second source of truth.
fn copy_self_fields_skipping_interrupt_cells(
    f: &MwirFn,
    frame: &Frame,
    self_temp: Temp,
    ctx: &mut FnCtx,
    dir: SelfFieldCopy,
) -> Result<(), CodegenError> {
    let self_ptr_off = frame
        .self_ptr_off
        .ok_or_else(|| CodegenError::internal("mut receiver but no self_ptr slot"))?;
    // Live base in X_A for FrameToLive; for LiveToFrame the prologue just
    // stored the incoming pointer at `self_ptr_off` and still holds it in
    // the ABI register — reload from the save slot so both directions
    // share one path.
    ctx.load_slot(X_A, self_ptr_off);
    let self_ty = &f.temp_types[self_temp.0];
    let Type::Named(name, targs) = strip_wrappers(self_ty) else {
        copy_self_aggregate_words(frame, self_temp, ctx, dir)?;
        return Ok(());
    };
    // plans/M7.md item G, decision 18: instantiated drivers
    // (`BlkDriver[DriverMode.Irq]`) are keyed in LayoutCtx by rendered
    // type spelling — same lookup `mwir::size_of` uses.
    let layout_key = if targs.is_empty() {
        name.clone()
    } else {
        crate::sema::types::render_type(&Type::Named(name.clone(), targs.to_vec()))
    };
    // plans/M9.md item B2: a `mut self` enum method write-back is the
    // whole aggregate (tag + payload), not a field walk — enums live in
    // `LayoutCtx::enums`, not `structs`. Looking only in `structs` was
    // `internal error: unknown struct \`Cell\`` reachable from ordinary
    // source (`Cell.fill`).
    if ctx.layout.enums.contains_key(name.as_str()) || ctx.layout.enums.contains_key(&layout_key) {
        copy_self_aggregate_words(frame, self_temp, ctx, dir)?;
        return Ok(());
    }
    let fields = ctx.layout.structs.get(&layout_key).ok_or_else(|| {
        CodegenError::internal(format!("unknown struct `{layout_key}` in layout ctx"))
    })?;
    let frame_base = frame.off(self_temp);
    let mut off = 0usize;
    for field_ty in fields {
        let sz =
            mwir::size_of(field_ty, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
        if !matches!(
            strip_wrappers(field_ty),
            Type::Named(n, _) if n == "InterruptCell"
        ) {
            let mut w = 0;
            while w < sz {
                match dir {
                    SelfFieldCopy::LiveToFrame => {
                        ctx.load_ptr(X_B, X_A, off + w);
                        ctx.store_slot(X_B, frame_base + off + w);
                    }
                    SelfFieldCopy::FrameToLive => {
                        ctx.load_slot(X_B, frame_base + off + w);
                        ctx.store_ptr(X_B, X_A, off + w);
                    }
                }
                w += 8;
            }
        }
        off += sz;
    }
    Ok(())
}

fn copy_self_aggregate_words(
    frame: &Frame,
    self_temp: Temp,
    ctx: &mut FnCtx,
    dir: SelfFieldCopy,
) -> Result<(), CodegenError> {
    let size = frame.size_of_temp(self_temp);
    let frame_off = frame.off(self_temp);
    let mut w = 0;
    while w < size {
        match dir {
            SelfFieldCopy::LiveToFrame => {
                ctx.load_ptr(X_B, X_A, w);
                ctx.store_slot(X_B, frame_off + w);
            }
            SelfFieldCopy::FrameToLive => {
                ctx.load_slot(X_B, frame_off + w);
                ctx.store_ptr(X_B, X_A, w);
            }
        }
        w += 8;
    }
    Ok(())
}

// --- per-fn driver: two passes, prologue length measured up front ----------

/// 05-library.md §7: mint a fresh non-wrapping `SlotMap` instance id into
/// field 0 (`map_id`), overwriting the body's placeholder `0`. Mirrors
/// `eval::interp::run_init`'s counter; the guest counter lives at
/// `machine_info::OFF_SLOTMAP_NEXT_ID` (zero at boot → first id is 1).
fn emit_slotmap_mint_id(map: Temp, ctx: &mut FnCtx<'_>) -> Result<(), CodegenError> {
    let addr =
        wrela_machine::layout::MACHINE_INFO_BASE + wrela_machine::machine_info::OFF_SLOTMAP_NEXT_ID;
    ctx.load_imm(X_A, addr as i64);
    ctx.push(
        encode::enc_ldr_x_imm(X_B, X_A, 0),
        format!(
            "ldr {}, [{}]  ; SlotMap next id",
            reg_name(X_B),
            reg_name(X_A)
        ),
        CostRule::Load,
        Some(X_B),
        &[X_A],
    );
    ctx.push(
        encode::enc_add_imm(X_C, X_B, 1, true),
        format!("add {}, {}, #1", reg_name(X_C), reg_name(X_B)),
        CostRule::Alu,
        Some(X_C),
        &[X_B],
    );
    // Non-wrapping: a zero after +1 means the u64 space wrapped.
    let skip = ctx.emit_skip(SkipKind::Cbnz(X_C));
    ctx.abort_fixed(
        "SlotMap instance id space exhausted (u64 non-wrapping mint, 05-library.md §7)",
    );
    ctx.patch_skip(skip, SkipKind::Cbnz(X_C));
    ctx.push(
        encode::enc_str_x_imm(X_C, X_A, 0),
        format!("str {}, [{}]", reg_name(X_C), reg_name(X_A)),
        CostRule::Store,
        None,
        &[X_C, X_A],
    );
    // `map_id` is field 0 — first 8 bytes of the aggregate.
    ctx.store_slot(X_C, ctx.frame.off(map));
    Ok(())
}

fn emit_fn(
    _key: &str,
    f: &MwirFn,
    layout: &LayoutCtx,
    rodata: &mut RodataPool,
) -> Result<CodegenFn, CodegenError> {
    // A sync fn never awaits, so it never stages a reply (0). Entropy
    // scratch is reserved when the body emits `Inst::Entropy` (item Es).
    let frame = build_frame(f, layout, 0, mwir_entropy_scratch_size(f), 0)?;

    // plans/M20.md item B / decision 1607: Lane 2 instruments **every**
    // owner, not just `app`. `cost-runtime` is the largest corpus case
    // (2717 of release SUM 4518), so an app-only `f` vector would explain
    // almost none of the scored program — and item C makes `f`
    // load-bearing at block grain. Freeze 1627: the coverage denominator
    // is still the whole scored set, not the instrumented subset. The one
    // exclusion is the counter helper itself (`block_count_instruments`).
    let block_ids = if block_count_instruments(_key) {
        assign_mwir_block_ids(&f.body)?
    } else {
        vec![None; f.body.len()]
    };

    let empty: [usize; 0] = [];
    let mut probe_pro = FnCtx {
        frame: &frame,
        layout,
        rodata,
        word_offsets: &empty,
        words: Vec::new(),
        relocs: Vec::new(),
        slot_base: X_SP,
        slot_bias: 0,
        cold_seq: 0,
    };
    emit_prologue(f, &frame, &mut probe_pro)?;
    let prologue_len = probe_pro.words.len();

    let dummy_targets = vec![0usize; f.body.len() + 1];
    let mut counts = Vec::with_capacity(f.body.len());
    for (i, inst) in f.body.iter().enumerate() {
        let mut probe = FnCtx {
            frame: &frame,
            layout,
            rodata,
            word_offsets: &dummy_targets,
            words: Vec::new(),
            relocs: Vec::new(),
            slot_base: X_SP,
            slot_bias: 0,
            cold_seq: 0,
        };
        // plans/M11.md decision 740: no checkpoint on sync loop back-edges.
        if let Some(id) = block_ids[i] {
            probe.emit_block_hit(id);
        }
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
        slot_base: X_SP,
        slot_bias: 0,
        cold_seq: 0,
    };
    emit_prologue(f, &frame, &mut ctx)?;
    debug_assert_eq!(ctx.words.len(), prologue_len);
    for (i, inst) in f.body.iter().enumerate() {
        // plans/M11.md decision 740: sync loop back-edges carry trip
        // counters only — no `FnCtx::checkpoint` (M10 decision 597
        // dissolved for console helpers; multi-core layout ownership
        // of `Reloc::CheckpointService` stays async-only).
        if let Some(id) = block_ids[i] {
            ctx.emit_block_hit(id);
        }
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

/// Integrity Phase 2 Item M: dumb leader set for a flat MWIR body —
/// index 0, every branch target, and the fallthrough after a branch /
/// return. Same shape Item L uses for block-split `s(b)`.
fn mwir_block_leaders(body: &[Inst]) -> Vec<bool> {
    let n = body.len();
    let mut leaders = vec![false; n];
    if n == 0 {
        return leaders;
    }
    leaders[0] = true;
    for (i, inst) in body.iter().enumerate() {
        match inst {
            Inst::Jump { target } => {
                if *target < n {
                    leaders[*target] = true;
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            Inst::JumpIfFalse { target, .. } => {
                if *target < n {
                    leaders[*target] = true;
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            Inst::Return { .. } => {
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            _ => {}
        }
    }
    leaders
}

fn assign_mwir_block_ids(body: &[Inst]) -> Result<Vec<Option<u32>>, CodegenError> {
    let mut ids = vec![None; body.len()];
    if !block_count() {
        return Ok(ids);
    }
    for (i, is_leader) in mwir_block_leaders(body).into_iter().enumerate() {
        if is_leader {
            ids[i] = Some(alloc_block_id()?);
        }
    }
    Ok(ids)
}

// ============================================================================
// plans/M6.md item D: FlowWir -> machine code (async fn state machines,
// decision 6's checkpoints, the item-C-deferred async dispatch entry).
//
// ## Dispatch header + state bodies + transition tails (item D task 1)
//
// Every async fn/method compiles to ONE ordinary machine-code fn — called
// through the *exact same* ABI a sync fn/method already uses (self ptr in
// x0, up to 2 scalar args in x1/x2, a scalar result in x0, an ordinary
// `ret`) — built from three parts, all sharing the identical `Frame`/
// `FnCtx` machinery this file already has:
//
//   1. A dispatch header: load a dedicated frame slot ("which state am I
//      resuming at", initialized to 0 in the prologue — decision's own
//      "load current state index from the turn frame"), then a
//      compare-and-branch chain, one arm per `FlowWirFn::states` entry, in
//      state order (the dumbest shape the task text itself names: "dumbest
//      — a compare-and-branch chain in state order").
//   2. Each state's own straight-line `ops`, flattened into ONE contiguous
//      instruction stream across every state (`flatten`, below) so the
//      *entire* per-instruction emission this file already has for a sync
//      fn (`emit_one`, reused verbatim for every embedded `FlowInst::Mwir`
//      op — never forked) drives an async fn's straight-line code too. A
//      local `Jump`/`JumpIfFalse` inside one state's own ops is remapped
//      from that state's own 0-based local index to its real position in
//      the flattened stream at flatten time (`remap_local_jumps`) — after
//      that one rewrite, every downstream mechanism (the two-pass
//      word-count sizing, `FnCtx::b_unconditional`/`cbz`) is completely
//      unaware it is looking at a flattened multi-state program rather
//      than an ordinary mwir body. Async `Transition::Jump` back-edges
//      still get decision 6's checkpoint via `target_flat <= flat_idx`
//      (`lower_while_split`'s state-cycle shape); sync `emit_fn` no
//      longer splices checkpoints onto mwir back-edges (plans/M11.md
//      decision 740 — trip counters only).
//   3. Each state's own `Transition`, compiled as one more "flat position"
//      immediately after that state's own ops (so a local jump to
//      "one past this state's last op" — legal, `flowwir_lower.rs`'s own
//      `b.here()` convention — lands exactly on the transition's own
//      compiled code, never needing a special case).
//
// ## Await: genuine park-and-resume (the M6 mandate; replaces item D's
// disclosed nested-drain placeholder, which is deleted, not kept as a
// second mechanism)
//
// `Transition::Await{ActorCall}` now compiles to a real suspension, so
// 04-compiler.md §2's semantics hold structurally rather than by
// simulation:
//
//   1. **Suspend** (the transition's own flat position): save
//      `resume_state` into the persistent state slot; marshal the args;
//      call the target actor's own `rt_enqueue` with a fourth argument —
//      the awaiting turn's own **waker** (`x3 = X_FRAME`, this turn's
//      area address; `OFF_TURN_*` above); mark `suspended = 1`; then
//      **return to the caller** (`x0 = TURN_STATUS_SUSPENDED`) — the
//      caller is always the scheduler (`rt_select_and_run`'s dispatch
//      arm for an actor turn; the entry driver's own loop for the root
//      test turn), which is exactly what lets EVERY ready actor run
//      while this turn is parked, not just the awaited target.
//   2. **Deliver** (in `layout.rs`'s `rt_select_and_run`): when the
//      awaited turn completes, the scheduler writes its reply into
//      `[waker + OFF_TURN_REPLY]` and sets `[waker + OFF_TURN_RESUME_READY]`.
//   3. **Resume** (a dedicated second flat position per await, the
//      dispatch target for `resume_state`): the scheduler re-enters this
//      same compiled fn; the entry's fresh-vs-resume discriminant
//      (`suspended != 0`) routes to the resume dispatch chain, which
//      lands on this await's own resume stub — compose `Ok(reply)` into
//      `result_temp` from `[X_FRAME + OFF_TURN_REPLY]`, run decision 6's
//      checkpoint ("await resume points are checkpoints by
//      construction"), and jump to `resume_state`'s own flat position.
//
// The whole reason this works across a native `ret`: an async fn's frame
// slots are NOT SP-relative — every temp lives in the fn's own persistent
// turn area (`Reloc::TurnFrameAddr`, `FnCtx::slot_base = X_FRAME`), so
// item B's all-temps-in-frame rule is precisely what makes suspension
// need to save nothing but the state index. The fn's own custom entry/
// exit (no `sub sp` at all): load `X_FRAME`, save `x30` into the frame's
// own lr slot, fork on the discriminant; every exit (suspend or
// complete) reloads `x30` from that slot and `ret`s. A completing return
// reports `x0 = TURN_STATUS_COMPLETED` with the scalar value in `x1`
// (the shared async epilogue below), and the mut-receiver writeback runs
// there — only at completion, never at a suspension (nothing can observe
// actor state mid-turn: the actor is busy for the whole span).
//
// `with group`/`g.start`/`g.join_all` genuinely are item F's own runtime
// pieces (no group arena consumer exists anywhere yet, `layout.rs`'s own
// `RuntimeTables::group_arena_capacity` doc comment) — those four ops
// fail closed, named, below; nothing here half-implements cancellation.

use crate::flowwir::{AwaitKind, FlowInst, FlowWirFn, FlowWirProgram, Transition};

/// The per-actor admission symbol `layout.rs` hand-assembles
/// (`build_rt_enqueue`'s own routine) and registers into the very same
/// call-target table `Reloc::Call` already resolves against — from
/// codegen's own point of view, a compiled `Send`/`Await{ActorCall}`
/// op's enqueue is just a symbolic call to this fixed name. (The old
/// `__await_actor_*` glue symbol died with the nested-drain placeholder
/// it belonged to.)
pub fn rt_enqueue_symbol(actor: &str) -> String {
    format!("{RT_ENQUEUE_PREFIX}{actor}")
}

/// The inverse: which actor a symbolic call target names, or `None` for an
/// ordinary compiled-fn key. `layout.rs` needs it to tell a real,
/// diagnosable source condition (this image messages an actor it never
/// declares, so no `rt_enqueue` routine for it exists) apart from a genuine
/// internal inconsistency, instead of reporting both as the latter.
///
/// The shadowing hazard an earlier spelling of this prefix carried is
/// closed by construction rather than by a naming rule: `layout.rs`
/// resolves a `Reloc::Call` against compiled source fns *before* glue
/// symbols, so any synthesized symbol a source fn could also be named
/// would silently shadow the real routine and emit a wrong image. The
/// prefix therefore contains a space — legal in a `BTreeMap` key, and
/// never in a wrela identifier (`syntax::lexer`), so no source fn's own
/// `CalleeKey::spelling()` can collide with one of these no matter what
/// it is called. This needs no reserved-prefix rule in `docs/language/`
/// and cannot be defeated by a cleverly named fn; `symbol_is_synthetic`
/// below states the invariant one place for every future glue symbol.
pub fn rt_enqueue_actor(key: &str) -> Option<&str> {
    key.strip_prefix(RT_ENQUEUE_PREFIX)
}

/// Whether `key` uses the *space-bearing* synthesized spelling: a
/// synthesized symbol contains a character no wrela identifier may
/// contain, so those two namespaces cannot overlap. This is the narrow
/// property, pinned by this file's own tests.
///
/// It is **not** the same question as "is this compiler glue" — M11 G
/// added `__wrela_rt_drain` / `__wrela_try_enqueue` and friends, which are
/// perfectly legal identifiers and therefore invisible to this rule. Ask
/// `is_compiler_glue_symbol` for that; every caller that means "not a
/// source fn" wants it, and both used to spell the prefix list out
/// themselves (and disagreed about it).
pub fn symbol_is_synthetic(key: &str) -> bool {
    key.contains(' ')
}

/// Whether `key` names compiler-generated glue rather than a source fn:
/// the space-bearing symbols above plus the generic `__wrela_*` /
/// `__enqueue_*` / `__method_*` / `__resume_*` families.
pub fn is_compiler_glue_symbol(key: &str) -> bool {
    symbol_is_synthetic(key)
        || key.starts_with("__wrela_")
        || key.starts_with("__enqueue_")
        || key.starts_with("__method_")
        || key.starts_with("__resume_")
}

/// The one place the symbol's own spelling lives, so `rt_enqueue_symbol`
/// and `rt_enqueue_actor` can never drift apart. The trailing space is
/// load-bearing (see `rt_enqueue_actor` above), not cosmetic.
const RT_ENQUEUE_PREFIX: &str = "rt_enqueue ";

/// plans/M10.md item E3 (decision 620): specialized per-core scheduler
/// tick. Space keeps it unrepresentable as a source key (same discipline
/// as `rt_enqueue `).
pub fn rt_run_one_symbol(core: usize) -> String {
    format!("rt_run_one {core}")
}

/// Hand-asm `rt_select_and_run` for mailbox root `actor`, registered in
/// glue so specialized `rt_run_one` can `Reloc::Call` it (item E3).
/// Item F promotes the body itself to a specialized `CodegenFn` under
/// the same key (decision 630).
pub fn rt_select_and_run_symbol(actor: &str) -> String {
    format!("rt_select_and_run {actor}")
}

/// Cross-core reply-ring push for Reply ring `src -> dst`
/// (`emit_rt_xreply`, plans/M10.md item F2 / decision 633). Space-bearing
/// synthetic key so specialized `rt_select_and_run` can `Reloc::Call` it.
/// M11 G remaps these onto `__wrela_xreply_<edge>` trampolines (decision 804).
pub fn rt_xreply_symbol(src_core: usize, dst_core: usize) -> String {
    format!("rt_xreply {src_core}->{dst_core}")
}

/// Parse `rt_xreply src->dst` into `(src, dst)`. Used by layout's G remap.
pub fn rt_xreply_cores(key: &str) -> Option<(usize, usize)> {
    let rest = key.strip_prefix("rt_xreply ")?;
    let (src, dst) = rest.split_once("->")?;
    Some((src.parse().ok()?, dst.parse().ok()?))
}

/// Specialized (E4) group-child poll for free-turn key `callee`.
/// Space-bearing synthetic key — same spelling E3 registered in glue.
pub fn rt_child_poll_symbol(callee: &str) -> String {
    format!("rt_child_poll {callee}")
}

/// Inbound-ring drain for `core` (`emit_rt_drain`, item F2 / decision 633).
pub fn rt_drain_symbol(core: usize) -> String {
    format!("rt_drain {core}")
}

/// Cross-core send for edge `(src_core, actor)` (`emit_rt_xsend`, item F2 /
/// decision 633). Replaces the former `__rt_xsend_*` glue spelling;
/// `resolve_cross_core_edge` returns this key.
pub fn rt_xsend_symbol(src_core: usize, actor: &str) -> String {
    format!("rt_xsend {src_core} {actor}")
}

/// Secondary core `core`'s entry loop (`emit_secondary_core_entry`, item
/// F2 / decision 633). VMM `core_entries` resolve against `fn_word_base`.
pub fn rt_secondary_core_entry_symbol(core: usize) -> String {
    format!("rt_secondary_core_entry {core}")
}

/// Whether `key` is a glue / specialized-synthetic target
/// `rt_run_one` may Call. Select / drain / child_poll are specialized in
/// `code` (items F / F2 / E4).
fn rt_run_one_glue_target(key: &str) -> bool {
    key.strip_prefix("rt_select_and_run ")
        .is_some_and(|a| !a.is_empty())
        || key
            .strip_prefix("rt_child_poll ")
            .is_some_and(|a| !a.is_empty())
        || key
            .strip_prefix("rt_drain ")
            .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
}

/// Whether `key` is a glue target specialized `rt_select_and_run` may
/// Call — specialized `rt_xreply` (item F2) or M11 G trampolines.
fn rt_select_and_run_glue_target(key: &str) -> bool {
    key.strip_prefix("rt_xreply ")
        .is_some_and(|rest| !rest.is_empty())
        || key
            .strip_prefix("__wrela_xreply_")
            .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
}

// M11 F: RtRunOneSpec / RtChildPollSpec deleted with their emitters.
// M11 J: RtSelectMethod / RtSelectAndRunSpec deleted with emit_rt_select_and_run.
// M11 G: RtXsendSpec / RtXreplySpec / RtDrainSpec deleted with emitters.

/// plans/M10.md item H (decision 680): specialized `rt_boot_init` symbol.
/// Space-bearing (` 0`) — unrepresentable as a source key; one body per
/// image (the trailing `0` is not a core index).
pub fn rt_boot_init_symbol() -> String {
    "rt_boot_init 0".to_string()
}

/// One state region `emit_boot_init` zero-fills, then (when `init` is
/// `Some`) calls as receiver (plans/M10.md item H / decision 680).
#[derive(Debug, Clone)]
pub struct BootInitSlotSpec {
    /// Actor mailbox-root name (`Reloc::MailboxAddr::State`) or driver
    /// name (`Reloc::DriverState`).
    pub name: String,
    pub is_driver: bool,
    pub state_size: u64,
    pub init: Option<BootInitCallSpec>,
}

/// One boot `init` call (plans/M10.md item H).
#[derive(Debug, Clone)]
pub struct BootInitCallSpec {
    pub key: String,
    pub args: Vec<BootInitArgSpec>,
    pub fallible: bool,
    /// `(rodata_byte_offset, len)` when `fallible`; set after
    /// `intern_fallible_init_abort_messages`.
    pub err_msg: Option<(usize, usize)>,
}

/// One materialized `init` argument for specialized boot (decision 683).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootInitArgSpec {
    Word(u64),
    DeviceRegsBase(usize),
    PoolBase(String),
    OwnSlot {
        pool: String,
        index: u64,
        slot_bytes: u64,
    },
    OwnHandleArray {
        pool: String,
        count: u64,
        slot_bytes: u64,
    },
}

// --- the turn record (the real park-and-resume contract) --------------------
//
// Every turn-capable entity — each declared actor, and each free async fn
// (a `@test(runtime)` root foremost) — owns one fixed **turn area** in the
// image's `rtdata` section: a fixed-shape `TURN_RECORD_SIZE`-byte record
// (56 bytes as of plans/M7.md item Z1; offsets below)
// followed by that entity's own statically reserved frame slots (the
// widest of its async fns' `Frame`s — one area per entity, never one per
// queued message, because non-reentrancy caps in-flight activations at
// one). These constants are the shared vocabulary between three parties
// that must never disagree: the compiled async fn (this module — reads/
// writes its own record through `X_FRAME`), the hand-assembled
// `rt_enqueue`/`rt_select_and_run`/`rt_run_one` routines (`layout.rs` —
// dispatch, reply delivery, readiness), and the entry driver (`layout.rs`
// — the root turn's own scheduler loop). They live here (not in
// `wrela-machine`) deliberately: the record is `rtdata`-interior compiler
// bookkeeping the VMM never reads — not machine contract.
//
// Record layout (all `u64` words):
//   +0  busy          1 while a turn owns this entity (active OR parked) —
//                     decision 4's structural non-reentrancy flag.
//   +8  suspended     1 while the current activation is parked on an
//                     `await` (set by the fn's own suspend tail; cleared
//                     by its own resume path). Doubles as the
//                     fresh-vs-resume entry discriminant: the compiled
//                     fn's entry reads it and either runs the fresh
//                     prologue (spill args, state=0) or the resume
//                     dispatch (re-enter at the saved state index).
//   +16 resume_ready  1 once the awaited reply has been delivered — the
//                     scheduler re-enters the fn only when
//                     busy && suspended && resume_ready.
//   +24 reply         the delivered scalar reply value (read by the fn's
//                     own per-await resume stub, composed into
//                     `Ok(reply)` there).
//   +32 waker_turn    `Option[TurnId]` (a `u32`): which turn awaits THIS
//                     turn's completion, as its 1-based index into the one
//                     contiguous `RT.turns` array — 0 = none (a `send`, or
//                     the root). plans/M10.md item 0c1, decisions 557/567:
//                     this used to be the waker's turn-area *address*, with
//                     `(src_core + 1) << 61` OR'd into its top three bits,
//                     untagged with a `load_imm`+`bic` pair at every read.
//                     The index needs no runtime table — `turns_base` and
//                     the pow2 `turn_stride` are both whole-image build-time
//                     constants (`RuntimePlacement::turn_addr`), which is
//                     what item 0a/0b bought.
//   +36 waker_core    `Option[CoreId]` (a `u32`): 0 = local (every
//                     same-core send, every single-core image), else the
//                     originating core + 1. The former top-bit tag, in its
//                     own field. These two `u32`s are the two halves of the
//                     ONE 64-bit word the tagged address occupied — never a
//                     second word, which would have cost 8 bytes on every
//                     mailbox slot and every cross-core request-ring slot
//                     image-wide, because the pair rides the whole
//                     `xsend -> ring -> mailbox -> turn record` chain.
//                     Accessed with `ldr w`/`str w` throughout: an `x`
//                     access on either would fold the other in as high bits
//                     and reinvent the bit-twiddling this deleted.
//   +40 cur_method    the in-flight method's dispatch index (actors
//                     only) — saved at fresh selection so the resume
//                     path can re-enter the same compiled method.
//   +48 reply_slot    plans/M7.md item Z1 (decision 9a): THIS turn's own
//   +52               reply staging slot while it is parked on an actor
//                     `await` whose declared reply is an aggregate —
//                     plans/M10.md item 0c1, decision 565: two adjacent
//                     `u32`s, `(TurnId at +48, byte offset within that turn
//                     area at +52)`, in the one word an absolute
//                     frame-interior address used to occupy. It is NOT a
//                     bare `TurnId` and cannot be: the offset is
//                     `Frame::reply_stage_off + slot_bias`, assigned per fn
//                     in `build_frame`, and the reader is the callee's
//                     dispatch arm — a different fn, which can know nothing
//                     of its caller's frame layout. An index plus a named
//                     intra-area offset is what indexing a *field of* an
//                     array element is; no bit packing anywhere.
//
//                     The pair is what the callee's dispatch resolves back
//                     into `x8`, this machine's aggregate-return-pointer
//                     register, so the callee writes its declared reply
//                     straight into the awaiting frame and nothing is
//                     copied at delivery.
//                     `reply` (+24) still carries every scalar reply,
//                     unchanged and byte-for-byte identically.
//
//                     The invariant, exactly (decision 9a): **written
//                     only by its own suspend path, read only by its
//                     callee's dispatch while it is parked.** That holds
//                     because a parked turn cannot begin a second await
//                     — the fn has genuinely `ret`urned to the scheduler
//                     at its one suspension point, and non-reentrancy
//                     (`busy`) admits no second activation — so between
//                     the store and the callee's load there is exactly
//                     one writer and one reader of this word.
//
//                     Why a stale value is never read (the one subtlety;
//                     nothing ever clears this word back to 0, and
//                     nothing needs to): the dispatch arm loads it *only*
//                     in an arm whose method's own declared reply is an
//                     aggregate. That is a build-time property of the
//                     CALLEE, and every caller that can reach that arm is
//                     a turn awaiting exactly that method — so its
//                     suspend path stored this word immediately before
//                     `rt_enqueue`, on the same activation. A scalar-
//                     reply arm never reads it at all, so whatever an
//                     older aggregate-reply await left behind is dead,
//                     not dangerous. (The `rtdata` section starts zeroed,
//                     so the word is 0 until the first aggregate-reply
//                     suspend writes it; the record's own boot state is
//                     still fully deterministic.)
//   +56 reply_tag     plans/M10.md item J (decision 559): the reply
//                     channel's own tag — `0` = Ok (payload in `reply`),
//                     else a `CallError` variant index (`Cancelled` = 1,
//                     `NotAdmitted` = 3) with any payload in `reply`.
//                     The group arena's `(tag, payload)` shape, on the
//                     turn record; retires `BRK_ACTOR_TURN_CANCELLED`.
//
pub const OFF_TURN_BUSY: u64 = 0;
pub const OFF_TURN_SUSPENDED: u64 = 8;
pub const OFF_TURN_RESUME_READY: u64 = 16;
pub const OFF_TURN_REPLY: u64 = 24;
pub const OFF_TURN_WAKER: u64 = 32;
pub const OFF_TURN_CUR_METHOD: u64 = 40;
pub const OFF_TURN_REPLY_SLOT: u64 = 48;
/// plans/M10.md item J (decision 559): the reply channel's own tag word —
/// `(tag, reply)` like the group arena's `(tag, payload)` pairs. `0` =
/// `Ok` (payload in `OFF_TURN_REPLY`); a nonzero value is a `CallError`
/// variant index (`Cancelled` = 1, `NotAdmitted` = 3) with any payload in
/// `OFF_TURN_REPLY` (`Admission` for `NotAdmitted`). Written by
/// `.deliver` / `rt_drain` / `rt_xreply` and by a rejected `await`
/// enqueue; read by `emit_await_resume`.
pub const OFF_TURN_REPLY_TAG: u64 = 56;
pub const TURN_RECORD_SIZE: u64 = 64;

// A compiled async fn's own return-status ABI (distinct from a sync fn's,
// which returns its value in x0 with no status — the dispatch arms in
// `rt_select_and_run` know each method's color at build time and read
// accordingly): x0 = status (0 completed / 1 suspended); when completed,
// x1 = the scalar return value.
pub const TURN_STATUS_COMPLETED: u64 = 0;
pub const TURN_STATUS_SUSPENDED: u64 = 1;
/// plans/M6.md item F (decision 8, 04-compiler.md §4): a checkpoint that
/// observes its own turn's ambient group cancelled terminates the
/// activation early — "the cancelled frame never resumes." Reported the
/// same way `TURN_STATUS_COMPLETED`/`_SUSPENDED` are (`x0`), never x1
/// (there is no real reply to report — whoever reads this status composes
/// `CallError::Cancelled`/an array slot showing it, never a scalar
/// value). Only ever produced by the shared cancellation tail
/// (`emit_async_cancelled_tail`) this item adds; every pre-existing
/// consumer of this ABI (`rt_select_and_run`'s actor dispatch arms) is
/// untouched and still only ever sees 0/1 — no required M6 golden runs an
/// actor method inside a cancelled group's own domain, a disclosed gap
/// recorded in this item's own ledger note, not silently widened here.
pub const TURN_STATUS_CANCELLED: u64 = 2;

// --- the group arena record (plans/M6.md item F, 02-language.md §9.5) ------
//
// One `GROUP_SLOT_SIZE`-byte record per statically-sized arena slot
// (`layout::RuntimeTables::group_arena_capacity` — a real count of
// `with group(...)` sites, item C's own sizing pass), all `u64` words:
//
//   +0  in_use          1 while this slot backs a currently-open `with
//                       group` scope (`GroupCreate`..`GroupClose`).
//   +8  capacity        the declared `capacity=` (0 = no children).
//   +16 active_children how many admitted `g.start` children have not yet
//                       completed/been harvested.
//   +24 deadline_ns     the narrowed effective deadline (0 = none) —
//                       `min(ambient, own)`, decision 8's own inheritance
//                       rule, computed once at `GroupCreate`.
//   +32 cancelled       1 once the vector-0 deadline scan (or a parent
//                       group's own cancellation propagation) marks this
//                       group cancelled.
//   +40 parent_group    the enclosing group's own arena index, or
//                       `GROUP_NO_PARENT` (`u64::MAX`) — a distinct
//                       sentinel from the lineage-slot encoding below
//                       (this field is arena-internal bookkeeping only,
//                       never read as a frame lineage value).
//   +48 join_waiter     the parent turn's own turn-area address, once
//                       `g.join_all()` parks waiting on this group's
//                       children (0 = not yet awaiting / no parent turn
//                       registered).
//   +56 owner_turn      the turn area of the frame that executed this
//                       group's own `GroupCreate` — the group's *parent*
//                       in 02-language.md §9.5's own sense. Written once
//                       at creation, read by every cancellation
//                       observation site to answer the one question that
//                       decides what a cancelled group does to a running
//                       activation: is this turn the group's owner, or a
//                       child started into it? A child's frame is
//                       terminated ("the cancelled frame never resumes",
//                       04-compiler.md §4); the owner's frame is not —
//                       02-language.md §9.5's own "source sees only
//                       `CallError` and its own `defer`s running" requires
//                       the `with`-block's own body to survive long enough
//                       to observe the `CallError` and run its cleanup.
//                       plans/M6.md item F records this reading in full.
//   +64.. child result slots: `GROUP_MAX_CHILDREN` pairs of (tag,
//                       payload), one per static `g.start` call site
//                       ordinal within this group (`GroupCtx::child_index`,
//                       below) — tag 0 = Ok, 1 = the composed
//                       `CallError::Cancelled` (the only non-`Op` variant
//                       M6's own dumbest floor ever produces; a real
//                       `CallError::Op(e)`/other variant composition is
//                       out of this item's own required surface, exactly
//                       like `emit_await_resume`'s own existing scalar-reply
//                       floor).
//
// Lives here, not `wrela-machine`, for the identical reason the turn
// record does (`TURN_RECORD_SIZE`'s own doc comment above): rtdata-interior
// compiler bookkeeping the VMM never reads.
pub const OFF_GROUP_IN_USE: u64 = 0;
pub const OFF_GROUP_CAPACITY: u64 = 8;
pub const OFF_GROUP_ACTIVE_CHILDREN: u64 = 16;
pub const OFF_GROUP_DEADLINE: u64 = 24;
pub const OFF_GROUP_CANCELLED: u64 = 32;
pub const OFF_GROUP_PARENT: u64 = 40;
pub const OFF_GROUP_JOIN_WAITER: u64 = 48;
pub const OFF_GROUP_OWNER_TURN: u64 = 56;
pub const OFF_GROUP_CHILDREN_BASE: u64 = 64;
/// Floor for empty-arena images (plans/M12.md item F / decisions 886–889):
/// `GROUP_MAX_CHILDREN` is an image fact — `max(FLOOR, max g.start children
/// over the image's group sites)` — not a hard Rust cap. Kept as the
/// empty-arena / stub default so placeholder overlays stay 96 bytes.
pub const GROUP_MAX_CHILDREN_FLOOR: usize = 2;
/// Floor slot size (`64 + FLOOR * 16` = 96). Prefer
/// [`group_slot_size`] with the image's `max_children`.
pub const GROUP_SLOT_SIZE: u64 = OFF_GROUP_CHILDREN_BASE + (GROUP_MAX_CHILDREN_FLOOR as u64) * 16;

/// `GROUP_SLOT_SIZE` for an image whose widest group has `max_children`
/// `g.start` sites: `64 + max_children * 16` (2→96, 4→128).
pub fn group_slot_size(max_children: usize) -> u64 {
    OFF_GROUP_CHILDREN_BASE + (max_children as u64) * 16
}
/// `parent_group`'s own "no parent" sentinel — distinct from the
/// lineage-slot encoding (`Temp(0)`'s own "0 = no ambient group, else
/// arena-index+1" scheme) since this field is never read as a lineage
/// value, only ever compared against by the deadline-scan/cancellation
/// routines this item adds.
pub const GROUP_NO_PARENT: u64 = u64::MAX;

/// `CallError[E]`'s own `Cancelled` variant tag — 02-language.md §9.4
/// declares the variant order (`Op`, `Cancelled`, `DeadlineExceeded`,
/// `NotAdmitted`, `PeerFailed`) and `sema::matches::shape_of`'s own
/// `CallError` arm builds exactly that order, which is what every
/// `EnumTag` comparison a `match` lowers to is numbered against.
pub const CALL_ERROR_TAG_CANCELLED: u64 = 1;
/// `CallError[E]`'s own `NotAdmitted` variant tag — same order as
/// `CALL_ERROR_TAG_CANCELLED`. Also the nonzero `OFF_TURN_REPLY_TAG`
/// value that delivers "never ran: mailbox full" (plans/M10.md item J).
pub const CALL_ERROR_TAG_NOT_ADMITTED: u64 = 3;
/// `Admission`'s opaque reason code for mailbox-full (05-library.md §2's
/// `Full | Restarting | StaleRequest | DeadlineUnmeetable` — first
/// variant, plans/M10.md decision 666). One `u64`; no fields yet.
pub const ADMISSION_FULL: u64 = 0;
/// `OFF_TURN_REPLY_TAG` = Ok (group-arena shape: tag 0 = Ok).
pub const REPLY_TAG_OK: u64 = 0;

pub fn group_child_tag_off(child_index: usize) -> u64 {
    OFF_GROUP_CHILDREN_BASE + (child_index as u64) * 16
}
pub fn group_child_payload_off(child_index: usize) -> u64 {
    group_child_tag_off(child_index) + 8
}

fn actor_of_method_key(key: &str) -> &str {
    key.split('.').next().unwrap_or(key)
}
fn method_name_of_key(key: &str) -> &str {
    key.split('.').nth(1).unwrap_or(key)
}

/// `(actor name) -> (method name) -> its own 0-based dispatch index`,
/// exactly the order `layout.rs`'s own `merge_actor_pub_methods` (and
/// therefore `build_rt_select_and_run`'s own `dispatch` table) already
/// uses — threaded in from there (`layout::actor_method_index_tables`) so
/// the two can never number a method differently.
pub type ActorMethodIndex = BTreeMap<String, BTreeMap<String, usize>>;

// --- group runtime context (plans/M6.md item F) -----------------------------

/// The whole-build facts `GroupCreate`/`GroupStart`/the group-child poll
/// routines (`layout.rs`) need, threaded alongside `ActorMethodIndex`
/// everywhere that already threads it: the static arena's own slot count
/// (`arena_capacity`, `layout::RuntimeTables::group_arena_capacity`), the
/// image-wide `max_children` fact (plans/M12.md item F), and each
/// `g.start`-able callee's own fixed child-slot ordinal
/// (`compute_group_child_indices`, below).
pub struct GroupCtx {
    pub arena_capacity: u64,
    /// Image fact: `max(FLOOR, max g.start children over group sites)`.
    pub max_children: usize,
    pub child_index: BTreeMap<String, usize>,
}

impl GroupCtx {
    pub fn slot_size(&self) -> u64 {
        group_slot_size(self.max_children)
    }
}

/// `callee_key -> its own fixed child-slot ordinal` (0-based, within
/// whichever group starts it) — computed once, whole-program, by counting
/// each `FlowInst::GroupStart` in program order per `(owner fn,
/// group_temp)` pair. Returns the map plus the image
/// `GROUP_MAX_CHILDREN` fact (`max(FLOOR, max per-site count)`).
///
/// Duplicate-callee floor still enforced here (M6's one-free-turn-area-
/// per-fn rule). Per-site child counts are no longer hard-capped at 2 —
/// the arena slot grows with the image fact (plans/M12.md item F).
pub fn compute_group_child_indices(
    flow: &FlowWirProgram,
) -> Result<(BTreeMap<String, usize>, usize), CodegenError> {
    let mut out = BTreeMap::new();
    let mut max_children = GROUP_MAX_CHILDREN_FLOOR;
    for (_fn_key, f) in &flow.fns {
        let mut counters: BTreeMap<Temp, usize> = BTreeMap::new();
        for state in &f.states {
            for op in &state.ops {
                if let FlowInst::GroupStart {
                    group_temp,
                    callee_key,
                    ..
                } = op
                {
                    let counter = counters.entry(*group_temp).or_insert(0);
                    let this_idx = *counter;
                    *counter += 1;
                    if out.insert(callee_key.clone(), this_idx).is_some() {
                        return Err(CodegenError::unimplemented(&format!(
                            "async fn `{callee_key}` is `g.start`ed from more than one static \
                             call site (plans/M6.md item F's own disclosed floor: one free-turn \
                             area per fn, M6-C's own sizing)"
                        )));
                    }
                }
            }
        }
        for &count in counters.values() {
            if count > max_children {
                max_children = count;
            }
        }
    }
    Ok((out, max_children))
}

/// Image `GROUP_MAX_CHILDREN` from an already-computed child-index map
/// (`max(FLOOR, max ordinal + 1)`). Same value
/// [`compute_group_child_indices`] returns as its second element.
pub fn group_max_children_of(child_index: &BTreeMap<String, usize>) -> usize {
    child_index
        .values()
        .copied()
        .max()
        .map(|i| i + 1)
        .unwrap_or(0)
        .max(GROUP_MAX_CHILDREN_FLOOR)
}

/// One flattened position: a state's own straight-line op (`FlowInst`,
/// jump targets already remapped to flat indices), a state's own
/// `Transition` (compiled last within its state), or an await's own
/// dedicated **resume stub** — its own flat position immediately after
/// the await transition itself, so the resume dispatch chain has a real,
/// word-offset-addressable landing site per await (module doc's own
/// "Resume" step).
enum FlatEntry {
    Op(FlowInst),
    Trans(Transition),
    /// The park-and-resume re-entry point for the await whose
    /// `resume_state`/`result_temp` these are: compose `Ok(reply)` from
    /// the turn record, checkpoint, jump to `resume_state`'s flat base.
    AwaitResume {
        resume_state: usize,
        result_temp: Temp,
        what: AwaitKind,
    },
}

/// Remaps a *local* (this-state-relative) `Jump`/`JumpIfFalse` target to
/// its real position in the flattened stream — every other `FlowInst`
/// passes through unchanged (module doc's own "after that one rewrite").
fn remap_local_jumps(op: &FlowInst, state_base: usize) -> FlowInst {
    match op {
        FlowInst::Mwir(Inst::Jump { target }) => FlowInst::Mwir(Inst::Jump {
            target: state_base + target,
        }),
        FlowInst::Mwir(Inst::JumpIfFalse { cond, target }) => FlowInst::Mwir(Inst::JumpIfFalse {
            cond: *cond,
            target: state_base + target,
        }),
        other => other.clone(),
    }
}

/// `state_flat_base[i]` is state `i`'s own first flat position (its first
/// op, or its own `Transition` when `ops` is empty) — every inter-state
/// transition (`Jump`/`Branch`) targets exactly this position for its own
/// target state(s). `resume_target[i]` is where the **resume dispatch
/// chain** re-enters for state `i`: for an await's own `resume_state`,
/// its dedicated `AwaitResume` stub (which composes the reply before
/// falling on to the state proper); for every other state, the state's
/// own flat base — a resume can only ever legitimately target an await's
/// resume state, but keeping every arm real keeps the chain's shape dumb
/// and uniform.
fn flatten(f: &FlowWirFn) -> (Vec<usize>, Vec<usize>, Vec<FlatEntry>) {
    let mut state_flat_base = Vec::with_capacity(f.states.len());
    let mut cursor = 0usize;
    for s in &f.states {
        state_flat_base.push(cursor);
        // +1 for this state's own Transition; an Await transition owns a
        // second flat position (its resume stub) immediately after.
        cursor += s.ops.len() + 1;
        if matches!(s.transition, Transition::Await { .. }) {
            cursor += 1;
        }
    }
    let mut resume_target = state_flat_base.clone();
    let mut flat = Vec::with_capacity(cursor);
    for (i, s) in f.states.iter().enumerate() {
        for op in &s.ops {
            flat.push(FlatEntry::Op(remap_local_jumps(op, state_flat_base[i])));
        }
        flat.push(FlatEntry::Trans(s.transition.clone()));
        if let Transition::Await {
            what,
            resume_state,
            result_temp,
        } = &s.transition
        {
            resume_target[*resume_state] = flat.len();
            flat.push(FlatEntry::AwaitResume {
                resume_state: *resume_state,
                result_temp: *result_temp,
                what: what.clone(),
            });
        }
    }
    (state_flat_base, resume_target, flat)
}

/// Builds the `Frame` for a FlowWir fn: `f.frame.temp_types` plus the one
/// dedicated extra `u64` slot this file's own codegen needs beyond what
/// `flowwir_lower.rs` allocated — `state_temp`, the dispatch header's own
/// "which state" slot (module doc above). Reuses `build_frame` verbatim
/// (never forked) via a synthetic `MwirFn` shape carrying exactly that
/// temp.
///
/// plans/M10.md item D0 (decision 610/612) deleted the 2-word
/// `arg_scratch` pair that used to sit next to it: `rt_enqueue`'s ABI
/// took a *pointer* to a contiguous args blob, so an async fn's own
/// independently-allocated `arg_temps` had to be copied into an owned,
/// always-contiguous marshaling area first. The ABI now carries the
/// arguments **by value** in `x1`/`x2`, so there is nothing to make
/// contiguous, and every async frame is 16 bytes smaller.
fn build_frame_flow(f: &FlowWirFn, layout: &LayoutCtx) -> Result<(Frame, Temp), CodegenError> {
    let mut temp_types = f.frame.temp_types.clone();
    let state_temp = Temp(temp_types.len());
    temp_types.push(Type::U64);
    let synthetic = MwirFn {
        receiver: f.receiver,
        params: f.params.clone(),
        ret: f.ret.clone(),
        temp_types,
        body: Vec::new(),
    };
    let frame = build_frame(
        &synthetic,
        layout,
        flow_reply_stage_size(f, layout)?,
        flow_entropy_scratch_size(f),
        TURN_RECORD_SIZE as usize,
    )?;
    Ok((frame, state_temp))
}

/// plans/M17.md item E / freeze 5: max packed scratch bytes across every
/// `FlowInst::Entropy` in this fn (shared region; ops run one at a time).
fn flow_entropy_scratch_size(f: &FlowWirFn) -> usize {
    f.states
        .iter()
        .flat_map(|s| s.ops.iter())
        .filter_map(|op| match op {
            FlowInst::Entropy { n, .. } => Some(*n as usize),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// plans/M17.md item Es / freeze 5: max packed scratch bytes across every
/// sync MWIR `Inst::Entropy` in this fn (shared region; ops run one at a
/// time). Parallel to `flow_entropy_scratch_size`.
fn mwir_entropy_scratch_size(f: &MwirFn) -> usize {
    f.body
        .iter()
        .filter_map(|op| match op {
            Inst::Entropy { n, .. } => Some(*n as usize),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// plans/M7.md item Z1 (decision 9b): how many bytes this fn's own reply
/// staging slot needs — the widest *declared* reply over its own
/// `Await{ActorCall}` sites that is an aggregate, or 0 if it has none (by
/// far the common case, and the one that keeps every M6 frame identical).
///
/// The declared type is recovered by inverting the composition sema
/// already applied (`sema::bodies::decompose_call_error`), never by a
/// second lowering-time channel: `result_temp`'s own frame type is
/// `Result[T, CallError[E]]`, and `T`/`Result[T, E]` is exactly what the
/// callee will write. An await whose composed type is not that shape
/// contributes nothing here — `emit_await_suspend`/`emit_await_resume`
/// read the identical predicate, so the three can never disagree about
/// whether a given site uses the wide transport, and `emit_await_resume`
/// still fails closed loudly on the malformed shape.
fn flow_reply_stage_size(f: &FlowWirFn, layout: &LayoutCtx) -> Result<usize, CodegenError> {
    let mut widest = 0usize;
    for s in &f.states {
        let Transition::Await {
            what, result_temp, ..
        } = &s.transition
        else {
            continue;
        };
        match what {
            AwaitKind::ActorCall { .. } => {
                let Some(declared) =
                    crate::sema::bodies::decompose_call_error(&f.frame.temp_types[result_temp.0])
                else {
                    continue;
                };
                if !is_aggregate(&declared) {
                    continue;
                }
                let sz = mwir::size_of(&declared, layout)
                    .map_err(|e| CodegenError::unimplemented(&e))?;
                widest = widest.max(sz);
            }
            AwaitKind::Receipt { .. } => {
                // `IoCompletion[P]` is the await result — always an aggregate.
                let ty = &f.frame.temp_types[result_temp.0];
                let sz = mwir::size_of(ty, layout).map_err(|e| CodegenError::unimplemented(&e))?;
                widest = widest.max(sz);
            }
            AwaitKind::GroupJoin { .. } => {}
        }
    }
    Ok(widest)
}

/// The resume dispatch chain's own trailing guard: a should-be-
/// unreachable producer-bug `BRK` — a resume can only ever be scheduled
/// with a state index this same fn's own suspend path stored.
const BRK_ASYNC_DISPATCH_NO_STATE_MATCHED: u16 = 0xACD4;

/// The whole async entry sequence (module doc's "Await: genuine
/// park-and-resume"): persistent-frame base load, lr save, the
/// fresh-vs-resume discriminant fork, the fresh prologue (arg/self spill
/// into the persistent frame, state = 0, jump to state 0), and the
/// resume dispatch chain (clear the discriminant + ready flag, re-enter
/// at the saved state's own resume target). Replaces both
/// `emit_prologue` and the old always-run dispatch header for async fns
/// — a sync fn's prologue/epilogue are untouched.
fn emit_async_entry(
    f: &MwirFn,
    fn_key: &str,
    ctx: &mut FnCtx,
    state_temp: Temp,
    resume_target: &[usize],
) -> Result<(), CodegenError> {
    // X_FRAME = &turn area (4 words, patched by layout via TurnFrameAddr).
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_FRAME, 0);
    // Overwrite the rendered text so the dump names the symbolic target
    // (the raw words stay the placeholder zeros layout patches).
    for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
        w.text = format!("turn-frame[{i}] {} <{fn_key}>", reg_name(X_FRAME));
    }
    ctx.relocs.push(Reloc::TurnFrameAddr {
        word,
        key: fn_key.to_string(),
    });
    // Save the caller's return address into the frame's own lr slot —
    // every exit (suspend or complete) reloads it from there.
    ctx.store_slot(X_LR, ctx.frame.lr_off);
    // Fresh-vs-resume fork on the suspended discriminant.
    ctx.push(
        encode::enc_ldr_x_imm(X_A, X_FRAME, OFF_TURN_SUSPENDED as u16),
        format!(
            "ldr {}, [{}, #{OFF_TURN_SUSPENDED}]",
            reg_name(X_A),
            reg_name(X_FRAME)
        ),
        CostRule::Load,
        Some(X_A),
        &[X_FRAME],
    );
    let fork = ctx.emit_skip(SkipKind::Cbnz(X_A));

    // --- fresh path: spill self/params into the persistent frame -------
    let mut next_reg = 0u8;
    if let Some((self_temp, mode)) = f.receiver {
        let self_ty = &f.temp_types[self_temp.0];
        // Same ABI rule as `emit_prologue` (InterruptCell skip-walk shared).
        if is_aggregate(self_ty) || mode == AccessMode::Mut {
            let self_ptr_off = ctx
                .frame
                .self_ptr_off
                .ok_or_else(|| CodegenError::internal("receiver present but no self_ptr slot"))?;
            ctx.store_slot(next_reg, self_ptr_off);
            copy_self_fields_skipping_interrupt_cells(
                f,
                &ctx.frame,
                self_temp,
                ctx,
                SelfFieldCopy::LiveToFrame,
            )?;
        } else {
            ctx.store_slot(next_reg, ctx.frame.off(self_temp));
        }
        next_reg += 1;
    }
    let mut mut_ptr_iter = ctx.frame.mut_param_ptr_offs.iter();
    for (p, mode) in &f.params {
        if next_reg > 8 {
            return Err(CodegenError::unimplemented("more than 8 call arguments"));
        }
        let ty = &f.temp_types[p.0];
        if is_aggregate(ty) || *mode == AccessMode::Mut {
            if *mode == AccessMode::Mut {
                let (pt, ptr_off) = mut_ptr_iter.next().ok_or_else(|| {
                    CodegenError::internal("mut param missing from frame.mut_param_ptr_offs")
                })?;
                if *pt != *p {
                    return Err(CodegenError::internal(
                        "mut_param_ptr_offs order disagrees with MwirFn::params",
                    ));
                }
                ctx.store_slot(next_reg, *ptr_off);
            }
            let size = ctx.frame.size_of_temp(*p);
            let dst_off = ctx.frame.off(*p);
            let mut w = 0;
            while w < size {
                ctx.load_ptr(X_A, next_reg, w);
                ctx.store_slot(X_A, dst_off + w);
                w += 8;
            }
        } else {
            ctx.store_slot(next_reg, ctx.frame.off(*p));
        }
        next_reg += 1;
    }
    if mut_ptr_iter.next().is_some() {
        return Err(CodegenError::internal(
            "frame.mut_param_ptr_offs has more entries than Mut params",
        ));
    }
    // plans/M7.md item Z1: an aggregate-returning async method is handed
    // its caller's destination address in `x8` (this machine's shared
    // aggregate-return ABI, `is_aggregate`/`emit_prologue`), and
    // `Inst::Return` writes the value through it. Spill it into the
    // persistent frame exactly like the sync prologue does — but only on
    // the FRESH path: on a resume there is no `x8` to spill (the
    // scheduler re-enters through `rt_select_and_run`'s own dispatch,
    // which reloads the pointer from the parked caller's record but at a
    // point this fn cannot depend on), and none is needed — the
    // persistent frame still holds the address the fresh entry spilled,
    // and the caller cannot have changed it while parked (it is parked;
    // decision 9a's own invariant).
    if let Some(ret_ptr_off) = ctx.frame.ret_ptr_off {
        ctx.store_slot(8, ret_ptr_off);
    }
    // state = 0 (hygiene: a completed prior activation leaves its last
    // state index behind; a fresh turn's own record is deterministic).
    ctx.load_imm(X_A, 0);
    ctx.store_slot(X_A, ctx.frame.off(state_temp));
    ctx.b_unconditional(0); // state 0's own flat base is always flat index 0.

    // --- resume path: consume the discriminant, dispatch --------------
    ctx.patch_skip(fork, SkipKind::Cbnz(X_A));
    for off in [OFF_TURN_SUSPENDED, OFF_TURN_RESUME_READY] {
        ctx.push(
            encode::enc_str_x_imm(X_ZR, X_FRAME, off as u16),
            format!("str xzr, [{}, #{off}]", reg_name(X_FRAME)),
            CostRule::Store,
            None,
            &[X_ZR, X_FRAME],
        );
    }
    ctx.load_slot(X_A, ctx.frame.off(state_temp));
    for (i, &flat_idx) in resume_target.iter().enumerate() {
        ctx.push_flags(
            encode::enc_cmp_imm(X_A, i as u16, true),
            format!("cmp {}, #{i}", reg_name(X_A)),
            CostRule::Alu,
            None,
            &[X_A],
            FlagEffect::Write,
        );
        ctx.b_cond_to(Cond::Eq, flat_idx);
    }
    ctx.push(
        encode::enc_brk(BRK_ASYNC_DISPATCH_NO_STATE_MATCHED),
        format!("brk #{BRK_ASYNC_DISPATCH_NO_STATE_MATCHED:#x}"),
        CostRule::System,
        None,
        &[],
    );
    Ok(())
}

/// The shared async completion epilogue, at `word_offsets[total]` — the
/// sentinel every embedded `Inst::Return`/`Transition::Return` already
/// branches to via `emit_one`'s ordinary path (which leaves the scalar
/// return value in `x0`): move the value to `x1`, run the mut-receiver
/// writeback (completion is the one moment actor state becomes
/// observable again — the turn is over), report
/// `x0 = TURN_STATUS_COMPLETED`, reload the caller's `x30` from the
/// frame's lr slot, `ret`.
fn emit_async_epilogue(f: &MwirFn, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    if is_aggregate(&f.ret) {
        // plans/M7.md item Z1: an aggregate reply never travels in `x1`.
        // `Inst::Return` has already written the whole value through this
        // fn's spilled `x8` (the awaiting caller's own staging slot), so
        // `x0` holds nothing meaningful here — report a deterministic 0
        // in the scalar reply word rather than whatever register state
        // the body happened to leave behind, since the dispatch arm
        // stores that word into an image-visible turn record.
        ctx.push(
            encode::enc_mov_reg(1, X_ZR, true),
            "mov x1, xzr".to_string(),
            CostRule::Alu,
            Some(1),
            &[X_ZR],
        );
    } else {
        ctx.push(
            encode::enc_mov_reg(1, 0, true),
            "mov x1, x0".to_string(),
            CostRule::Alu,
            Some(1),
            &[0],
        );
    }
    // M11 F (decision 793): park the completing scalar in OFF_TURN_REPLY so
    // generic `__wrela_child_poll` can read it after the resume Call (the
    // specialized emitter captured x1; wrela Calls only bind x0).
    ctx.push(
        encode::enc_str_x_imm(1, X_FRAME, OFF_TURN_REPLY as u16),
        format!(
            "str {}, [{}, #{OFF_TURN_REPLY}]  ; complete → turn.reply",
            reg_name(1),
            reg_name(X_FRAME)
        ),
        CostRule::Store,
        None,
        &[1, X_FRAME],
    );
    if let Some((self_temp, mode)) = f.receiver {
        if mode == AccessMode::Mut {
            // plans/M7.md item G, decision 17: same InterruptCell skip as
            // the sync epilogue — live cells are not frame-owned.
            copy_self_fields_skipping_interrupt_cells(
                f,
                ctx.frame,
                self_temp,
                ctx,
                SelfFieldCopy::FrameToLive,
            )?;
        }
    }
    for (p, ptr_off) in &ctx.frame.mut_param_ptr_offs {
        ctx.load_slot(X_A, *ptr_off);
        let size = ctx.frame.size_of_temp(*p);
        let src_off = ctx.frame.off(*p);
        let mut w = 0;
        while w < size {
            ctx.load_slot(X_B, src_off + w);
            ctx.store_ptr(X_B, X_A, w);
            w += 8;
        }
    }
    ctx.load_imm(0, TURN_STATUS_COMPLETED as i64);
    ctx.load_slot(X_LR, ctx.frame.lr_off);
    ctx.push(
        encode::enc_ret(X_LR),
        "ret".to_string(),
        CostRule::Branch,
        None,
        &[X_LR],
    );
    Ok(())
}

impl FnCtx<'_> {
    /// `B.<cond>` to a flattened target position — `b_unconditional`/`cbz`'s
    /// own sibling for the dispatch header's own compare-and-branch chain.
    fn b_cond_to(&mut self, cond: Cond, target_flat_idx: usize) {
        let this_word = self.cur_word();
        let delta = self.branch_target_delta(target_flat_idx, this_word);
        let flags = match cond {
            Cond::Al | Cond::Nv => FlagEffect::None,
            _ => FlagEffect::Read,
        };
        self.push_flags(
            encode::enc_b_cond(cond, delta),
            format!("b.{} #{delta}", cond_mnemonic(cond)),
            CostRule::Branch,
            None,
            &[],
            flags,
        );
    }
}

/// Loads `arg_temps` (at most 2 — the by-value ABI below carries exactly
/// two argument registers) into `x1`/`x2` and calls `symbol` —
/// `rt_enqueue_<Actor>`'s own real ABI
/// (`x0=method_idx, x1=arg0, x2=arg1, x3=waker_turn, x4=waker_core`),
/// shared verbatim by `Send` (waker = 0: one-way, nobody to resume, the
/// sender never suspends) and `Await{ActorCall}` (waker = this turn's own
/// `TurnId`, a relocated immediate — plans/M10.md item 0c1, decision 557).
///
/// plans/M10.md item D0, decision 610: the arguments used to travel as
/// `x1 = args_ptr` (the address of an owned 2-word frame scratch pair)
/// plus `x2 = nargs_words`, and `build_ring_enqueue` copied `x2` words
/// out of that pointer. They now travel **by value**, which makes this
/// half of the ABI match the consumer half — dispatch has always loaded
/// `x1`/`x2` straight out of the mailbox slot (`layout.rs`'s
/// `build_rt_select_and_run`). Absent arguments are written as an
/// explicit zero rather than left undefined: the callee stores whatever
/// these registers hold into the slot, and a deterministic zero is
/// strictly better than the stale bytes of the slot's previous occupant.
fn emit_marshal_and_call(
    method_idx: usize,
    arg_temps: &[Temp],
    ctx: &mut FnCtx,
    symbol: &str,
    // `Some(this fn's own key)` for an `Await{ActorCall}` — the waker is
    // this turn, named by its `TurnId`; `None` for a `send`, which has no
    // waker at all. plans/M10.md item 0c1: this used to be a plain `bool`
    // and the waker used to be `X_FRAME` itself (the turn area's address),
    // which is exactly the raw reference item 0 exists to delete.
    waker_self_key: Option<&str>,
) -> Result<(), CodegenError> {
    if arg_temps.len() > 2 {
        return Err(CodegenError::unimplemented(
            "more than 2 scalar message args (the by-value mailbox admission ABI carries x1/x2 only)",
        ));
    }
    for reg in [1u8, 2u8] {
        match arg_temps.get(reg as usize - 1) {
            Some(t) => ctx.load_slot(reg, ctx.frame.off(*t)),
            // `mov xN, xzr` — 1 word, no `load_imm` 4-word movz/movk run.
            None => ctx.push(
                encode::enc_mov_reg(reg, X_ZR, true),
                format!("mov x{reg}, xzr"),
                CostRule::Alu,
                Some(reg),
                &[X_ZR],
            ),
        }
    }
    match waker_self_key {
        Some(fn_key) => {
            let word = ctx.cur_word();
            ctx.load_imm_naive(3, 0);
            for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
                w.text = format!("turn-id[{i}] x3 <{fn_key}>");
            }
            ctx.relocs.push(Reloc::TurnIdImm {
                word,
                key: fn_key.to_string(),
            });
        }
        // A `send` has no waker: `x3 = 0` is the `Option[TurnId]` niche,
        // the same zero test every reader already performs.
        None => ctx.load_imm(3, 0),
    }
    // plans/M10.md item 0c1: `x4 = Option[CoreId]`, always 0 here. A
    // same-core admission is local by definition, and a cross-core one goes
    // through `__rt_xsend_*`, which overwrites `x4` with its own source
    // core. Set unconditionally rather than left to the callee: a stale
    // `x4` would deliver this turn's reply to the wrong core.
    ctx.load_imm(4, 0);
    ctx.load_imm(0, method_idx as i64);
    // ABI: x0..x4 live into the call; EmittedWord holds at most 4 srcs.
    ctx.bl_symbolic_call(symbol, &[0, 1, 2, 3]);
    Ok(())
}

fn lookup_method_idx(
    method_key: &str,
    method_index: &ActorMethodIndex,
) -> Result<(String, usize), CodegenError> {
    let actor = actor_of_method_key(method_key).to_string();
    let method = method_name_of_key(method_key);
    let idx = method_index
        .get(&actor)
        .and_then(|m| m.get(method))
        .copied()
        .ok_or_else(|| {
            CodegenError::internal(format!(
                "unknown actor method `{method_key}` (no dispatch index)"
            ))
        })?;
    Ok((actor, idx))
}

/// `send target.method(args...)` (02-language.md §9.4): a one-way
/// `rt_enqueue` call, never a suspension. `dst` is
/// `Result[unit, CallError[never]]` (plans/M13.md item J / decision 5):
/// admitted (`x0 == 0`) → `Ok(unit)`; rejected → the same local
/// `Err(CallError.NotAdmitted(Admission.Full, (take_args...)))`
/// construction as an awaited call's enqueue-fail arm (item H).
fn emit_send(
    dst: Temp,
    method_key: &str,
    arg_temps: &[Temp],
    take_arg_temps: &[Temp],
    ctx: &mut FnCtx,
    method_index: &ActorMethodIndex,
) -> Result<(), CodegenError> {
    let (actor, idx) = lookup_method_idx(method_key, method_index)?;
    emit_marshal_and_call(
        idx,
        arg_temps,
        ctx,
        &rt_enqueue_symbol(&actor),
        None, // one-way: no reply slot, no waker — the sender never suspends.
    )?;
    let dst_off = ctx.frame.off(dst);
    let dst_size = ctx.frame.size_of_temp(dst);
    // Rejected (x0 != 0) skips the Ok arm.
    let skip_ok = ctx.emit_skip(SkipKind::Cbnz(0));
    // Ok(unit): zero-fill, tag = 0.
    let mut w = 0usize;
    while w < dst_size {
        ctx.store_slot(X_ZR, dst_off + w);
        w += 8;
    }
    let done = ctx.emit_skip(SkipKind::Cond(Cond::Al));
    ctx.patch_skip(skip_ok, SkipKind::Cbnz(0));
    emit_not_admitted_local(ctx, dst_off, dst_size, take_arg_temps)?;
    ctx.patch_skip(done, SkipKind::Cond(Cond::Al));
    Ok(())
}

/// `self.field. ... .leaf` (02-language.md §9.2), re-derived fresh from
/// the fn's own stable `self` temp — never a cached value (`flowwir.rs`'s
/// own "Self-rooted paths across await" section). Walks the same
/// `field_offset_size` this file already uses for `Project`, one field at
/// a time, resolving each name against `LayoutCtx::struct_field_names`
/// (item D's own small addition to `mwir::LayoutCtx`, `dst`'s own doc
/// comment there).
fn emit_self_path(
    dst: Temp,
    path: &[String],
    f: &MwirFn,
    ctx: &mut FnCtx,
) -> Result<(), CodegenError> {
    let (self_temp, _) = f
        .receiver
        .ok_or_else(|| CodegenError::internal("SelfPath op in a fn with no receiver"))?;
    let mut cur_off = ctx.frame.off(self_temp);
    let mut cur_ty = f.temp_types[self_temp.0].clone();
    for name in path {
        let base_ty = strip_wrappers(&cur_ty).clone();
        let Type::Named(sname, targs) = &base_ty else {
            return Err(CodegenError::internal(
                "SelfPath: an intermediate step is not a struct type",
            ));
        };
        let layout_key = if targs.is_empty() {
            sname.clone()
        } else {
            crate::sema::types::render_type(&Type::Named(sname.clone(), targs.clone()))
        };
        let names = ctx
            .layout
            .struct_field_names
            .get(&layout_key)
            .ok_or_else(|| {
                CodegenError::internal(format!(
                    "unknown struct `{layout_key}` (no field-name table)"
                ))
            })?;
        let idx = names.iter().position(|n| n == name).ok_or_else(|| {
            CodegenError::internal(format!("unknown field `{name}` on struct `{layout_key}`"))
        })?;
        let (off, _size) = field_offset_size(&base_ty, idx, ctx.layout)?;
        let field_ty = ctx.layout.structs[&layout_key][idx].clone();
        cur_off += off;
        cur_ty = field_ty;
    }
    let size = ctx.frame.size_of_temp(dst);
    ctx.copy_slot_to_slot(ctx.frame.off(dst), cur_off, size);
    Ok(())
}

/// `now()` (plans/M6.md decision 11): a trapping MMIO load —
/// `wrela_machine::mmio::CLOCK_MMIO_ADDR`, the exact address 06-machine.md
/// §5/decision 13's own clock-read protocol already names; the VMM's own
/// exit handler (item E) is what actually returns monotonic ns and logs
/// the read (`machine.clock.trap-logged`), this fn only issues the load.
///
/// Shared by FlowWir (`FlowInst::Now`) and sync MWIR (`Inst::Now` via
/// `emit_one`, plans/M17.md item Es). Do not duplicate the load sequence
/// at either call site.
fn emit_now(dst: Temp, ctx: &mut FnCtx) {
    ctx.load_imm(X_A, wrela_machine::mmio::CLOCK_MMIO_ADDR as i64);
    ctx.load_ptr(X_B, X_A, 0);
    ctx.store_slot(X_B, ctx.frame.off(dst));
}

/// `entropy[N]()` (plans/M17.md item E / freeze 5): park-shaped fill.
/// 1. Packed scratch of `n` bytes (reserved on the frame).
/// 2. Store scratch GPA → `OFF_ENTROPY_DEST`; store `n` → `OFF_ENTROPY_LEN`.
/// 3. Trapping store to `ENTROPY_MMIO_ADDR`.
/// 4. Expand scratch bytes into `Bytes[N]` slot-per-byte `dst`.
///
/// Shared by FlowWir (`FlowInst::Entropy`) and sync MWIR (`Inst::Entropy`
/// via `emit_one`, item Es). Do not duplicate this sequence at either
/// call site.
fn emit_entropy(dst: Temp, n: u64, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    let scratch_off = ctx.frame.entropy_scratch_off.ok_or_else(|| {
        CodegenError::internal("entropy scratch not reserved in frame (codegen bug)")
    })?;
    if n == 0 || n as usize > ctx.frame.entropy_scratch_size {
        return Err(CodegenError::internal(format!(
            "entropy n={n} outside reserved scratch size {}",
            ctx.frame.entropy_scratch_size
        )));
    }
    let max = wrela_machine::machine_info::ENTROPY_LEN_MAX;
    if n > max {
        return Err(CodegenError::internal(format!(
            "entropy n={n} exceeds ENTROPY_LEN_MAX={max}"
        )));
    }

    // Scratch GPA → OFF_ENTROPY_DEST.
    ctx.addr_of_slot(X_A, scratch_off);
    ctx.load_imm(
        X_B,
        (wrela_machine::layout::MACHINE_INFO_BASE + wrela_machine::machine_info::OFF_ENTROPY_DEST)
            as i64,
    );
    ctx.store_ptr(X_A, X_B, 0);

    // n → OFF_ENTROPY_LEN.
    ctx.load_imm(X_A, n as i64);
    ctx.load_imm(
        X_B,
        (wrela_machine::layout::MACHINE_INFO_BASE + wrela_machine::machine_info::OFF_ENTROPY_LEN)
            as i64,
    );
    ctx.store_ptr(X_A, X_B, 0);

    // Trapping store (any value) to ENTROPY_MMIO_ADDR.
    // Rt=31 encodes as XZR in STR (store_ptr's dump text says `sp` because
    // `reg_name(31)` is shared with SP — same as other X_ZR store_ptr sites).
    ctx.load_imm(X_A, wrela_machine::mmio::ENTROPY_MMIO_ADDR as i64);
    ctx.store_ptr(X_ZR, X_A, 0);

    // Expand packed scratch → Bytes[N] slot-per-byte dst.
    let dst_off = ctx.frame.off(dst);
    ctx.addr_of_slot(X_C, scratch_off);
    for i in 0..n as usize {
        ctx.load_byte_imm(X_B, X_C, i as u16);
        ctx.store_slot(X_B, dst_off + i * 8);
    }
    Ok(())
}

/// The two dedicated lineage frame slots every `FlowWirFn` reserves
/// (`flowwir::FrameLayout`'s own doc: "always `Temp(0)`/`Temp(1)`,
/// allocated first, before `self`/params/every other temp") — a fixed
/// convention, not a value threaded from anywhere, so every group op below
/// just names them directly.
const LINEAGE_GROUP_SLOT: Temp = Temp(0);
const LINEAGE_DEADLINE_SLOT: Temp = Temp(1);

/// `with group(...)`'s own opening bracket (02-language.md §9.5,
/// plans/M6.md item F #1): a real, dumbest-correct linear scan of the
/// whole-image group arena (`GroupCtx::arena_capacity` slots, fully
/// unrolled — the count is a small, build-time constant, `CLAUDE.md`'s
/// "linear scans over the static arena" made literal, never a runtime
/// loop) for the first `in_use == 0` slot; a build with every slot
/// occupied aborts, named (an M6 image never nests/loops deeply enough to
/// exhaust an arena sized from its own static with-site count *unless*
/// the same static site's own group is somehow still open when re-entered
/// recursively — M6 has no recursion, so this is a should-never-fire
/// defensive floor, not a real capacity limit any required golden nears).
/// Once a slot is claimed: `capacity`/`active_children`/`cancelled`/
/// `join_waiter`/every child result slot are (re-)initialized (a slot may
/// be reused by a later loop iteration of the identical `with`-site, so
/// hygiene zeroing is real, not optional); the deadline narrows
/// (`min(ambient, own)`, 0 meaning "none" throughout, decision 8's own
/// inheritance rule) via branch-free `CSEL`s (module doc below); the
/// *previous* ambient lineage becomes this group's own `parent_group`
/// (`GROUP_NO_PARENT` if there was none); and the frame's own
/// `LINEAGE_GROUP_SLOT`/`LINEAGE_DEADLINE_SLOT` (plus `group_temp`
/// itself, the `as g` binding if any) become this new group's own
/// `arena_index + 1`/effective deadline — every `Send`/`Await`/nested
/// `with group`/checkpoint compiled *after* this op, until the matching
/// `GroupClose`, reads the new ambient values.
#[allow(clippy::too_many_arguments)]
fn emit_group_create(
    group_temp: Temp,
    capacity: Option<Temp>,
    deadline: Option<Temp>,
    ctx: &mut FnCtx,
    gctx: &GroupCtx,
    // plans/M10.md item 0c2: this fn's own `program.fns` key — the
    // `Reloc::TurnIdImm` key for "this turn", which is the value
    // `OFF_GROUP_OWNER_TURN` now carries in place of `X_FRAME`.
    fn_key: &str,
) -> Result<(), CodegenError> {
    const X_ARENA: u8 = 15;
    const X_CAND: u8 = 16;
    const X_TAG: u8 = 17;

    let word = ctx.cur_word();
    ctx.load_imm_naive(X_ARENA, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("group-arena-base {}", reg_name(X_ARENA));
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });

    // Capture the *old* ambient lineage before anything overwrites it —
    // this group's own `parent_group`/deadline-narrowing inputs.
    ctx.load_slot(X_A, ctx.frame.off(LINEAGE_GROUP_SLOT)); // old ambient group, encoded (0 = none)
    ctx.load_slot(X_B, ctx.frame.off(LINEAGE_DEADLINE_SLOT)); // old ambient deadline (0 = none)
    match deadline {
        Some(t) => ctx.load_slot(X_C, ctx.frame.off(t)),
        None => ctx.load_imm(X_C, 0),
    }
    let own_capacity_off = capacity.map(|t| ctx.frame.off(t));

    // Branch-free narrowing (module doc): 0 means "no deadline" throughout,
    // so it is remapped to a MAX sentinel for the `min`, then remapped back.
    ctx.load_imm(X_D, u64::MAX as i64); // sentinel
    ctx.push_flags(
        encode::enc_cmp_imm(X_B, 0, true),
        format!("cmp {}, #0", reg_name(X_B)),
        CostRule::Alu,
        None,
        &[X_B],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_csel(X_E, X_D, X_B, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_E),
            reg_name(X_D),
            reg_name(X_B)
        ),
        CostRule::Alu,
        Some(X_E),
        &[X_D, X_B],
        FlagEffect::Read,
    );
    ctx.push_flags(
        encode::enc_cmp_imm(X_C, 0, true),
        format!("cmp {}, #0", reg_name(X_C)),
        CostRule::Alu,
        None,
        &[X_C],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_csel(X_F, X_D, X_C, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_F),
            reg_name(X_D),
            reg_name(X_C)
        ),
        CostRule::Alu,
        Some(X_F),
        &[X_D, X_C],
        FlagEffect::Read,
    );
    ctx.cmp_reg(X_E, X_F);
    // `Ls`, not `Le` — **a real bug the first deadline-bearing boot
    // caught** (recorded, not silently fixed): a deadline is a raw
    // `u64` nanosecond count and the "no deadline" sentinel above is
    // `u64::MAX`, which as a *signed* value is `-1`. With `Le` the
    // sentinel therefore compared as smaller than every real deadline,
    // so `min(none, own)` picked the sentinel and the remap below turned
    // it straight back into `0` — every group with a declared deadline
    // and no ambient one (i.e. every top-level `with group(deadline=..)`
    // there is at M6) stored "no deadline" and could never expire.
    // Invisible until a golden actually armed a deadline.
    ctx.push_flags(
        encode::enc_csel(X_TAG, X_E, X_F, Cond::Ls, true),
        format!(
            "csel {}, {}, {}, ls",
            reg_name(X_TAG),
            reg_name(X_E),
            reg_name(X_F)
        ),
        CostRule::Alu,
        Some(X_TAG),
        &[X_E, X_F],
        FlagEffect::Read,
    );
    ctx.cmp_reg(X_TAG, X_D);
    ctx.push_flags(
        encode::enc_csel(X_TAG, X_ZR, X_TAG, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_TAG),
            reg_name(X_ZR),
            reg_name(X_TAG)
        ),
        CostRule::Alu,
        Some(X_TAG),
        &[X_ZR, X_TAG],
        FlagEffect::Read,
    );
    // X_TAG now holds the effective (narrowed) deadline. Stash the old
    // ambient group (X_A) as the new group's parent before we clobber the
    // lineage slot — `parent_group = (old_ambient == 0) ? GROUP_NO_PARENT
    // : old_ambient - 1`.
    ctx.push(
        encode::enc_sub_imm(X_B, X_A, 1, true),
        format!("sub {}, {}, #1", reg_name(X_B), reg_name(X_A)),
        CostRule::Alu,
        Some(X_B),
        &[X_A],
    );
    ctx.load_imm(X_D, GROUP_NO_PARENT as i64);
    ctx.push_flags(
        encode::enc_cmp_imm(X_A, 0, true),
        format!("cmp {}, #0", reg_name(X_A)),
        CostRule::Alu,
        None,
        &[X_A],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_csel(X_B, X_D, X_B, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_B),
            reg_name(X_D),
            reg_name(X_B)
        ),
        CostRule::Alu,
        Some(X_B),
        &[X_D, X_B],
        FlagEffect::Read,
    );
    // X_B now holds parent_group.

    let mut to_after: Vec<usize> = Vec::new();
    for i in 0..gctx.arena_capacity {
        if i == 0 {
            ctx.push(
                encode::enc_add_imm(X_CAND, X_ARENA, 0, true),
                format!("add {}, {}, #0", reg_name(X_CAND), reg_name(X_ARENA)),
                CostRule::Alu,
                Some(X_CAND),
                &[X_ARENA],
            );
        } else {
            ctx.load_imm(X_D, (i * gctx.slot_size()) as i64);
            ctx.add_reg(X_CAND, X_ARENA, X_D);
        }
        ctx.push(
            encode::enc_ldr_x_imm(X_D, X_CAND, OFF_GROUP_IN_USE as u16),
            format!(
                "ldr {}, [{}, #{OFF_GROUP_IN_USE}]",
                reg_name(X_D),
                reg_name(X_CAND)
            ),
            CostRule::Load,
            Some(X_D),
            &[X_CAND],
        );
        let skip_try_next = ctx.emit_skip(SkipKind::Cbnz(X_D)); // in_use != 0 -> try next candidate

        // Found: initialize this slot.
        ctx.load_imm(X_D, 1);
        ctx.push(
            encode::enc_str_x_imm(X_D, X_CAND, OFF_GROUP_IN_USE as u16),
            format!(
                "str {}, [{}, #{OFF_GROUP_IN_USE}]",
                reg_name(X_D),
                reg_name(X_CAND)
            ),
            CostRule::Store,
            None,
            &[X_D, X_CAND],
        );
        match own_capacity_off {
            Some(off) => {
                ctx.load_slot(X_D, off);
            }
            None => ctx.load_imm(X_D, 0),
        }
        ctx.push(
            encode::enc_str_x_imm(X_D, X_CAND, OFF_GROUP_CAPACITY as u16),
            format!(
                "str {}, [{}, #{OFF_GROUP_CAPACITY}]",
                reg_name(X_D),
                reg_name(X_CAND)
            ),
            CostRule::Store,
            None,
            &[X_D, X_CAND],
        );
        for off in [OFF_GROUP_ACTIVE_CHILDREN, OFF_GROUP_CANCELLED] {
            ctx.push(
                encode::enc_str_x_imm(X_ZR, X_CAND, off as u16),
                format!("str xzr, [{}, #{off}]", reg_name(X_CAND)),
                CostRule::Store,
                None,
                &[X_ZR, X_CAND],
            );
        }
        // plans/M10.md item 0c2: `join_waiter` is now an
        // `Option[TurnId]` — a `u32`, so the hygiene zeroing clears the
        // four bytes the field actually occupies and not the four bytes of
        // unused padding above it. `wzr` rather than `xzr` is the honest
        // width; the niche (decision 567) makes `0` still mean "nobody
        // waiting", so this zero test keeps its meaning exactly.
        ctx.push(
            encode::enc_str_w_imm(X_ZR, X_CAND, OFF_GROUP_JOIN_WAITER as u16),
            format!("str wzr, [{}, #{OFF_GROUP_JOIN_WAITER}]", reg_name(X_CAND)),
            CostRule::Store,
            None,
            &[X_ZR, X_CAND],
        );
        for c in 0..gctx.max_children {
            for off in [group_child_tag_off(c), group_child_payload_off(c)] {
                ctx.push(
                    encode::enc_str_x_imm(X_ZR, X_CAND, off as u16),
                    format!("str xzr, [{}, #{off}]", reg_name(X_CAND)),
                    CostRule::Store,
                    None,
                    &[X_ZR, X_CAND],
                );
            }
        }
        ctx.push(
            encode::enc_str_x_imm(X_TAG, X_CAND, OFF_GROUP_DEADLINE as u16),
            format!(
                "str {}, [{}, #{OFF_GROUP_DEADLINE}]",
                reg_name(X_TAG),
                reg_name(X_CAND)
            ),
            CostRule::Store,
            None,
            &[X_TAG, X_CAND],
        );
        ctx.push(
            encode::enc_str_x_imm(X_B, X_CAND, OFF_GROUP_PARENT as u16),
            format!(
                "str {}, [{}, #{OFF_GROUP_PARENT}]",
                reg_name(X_B),
                reg_name(X_CAND)
            ),
            CostRule::Store,
            None,
            &[X_B, X_CAND],
        );
        // The owning turn (02-language.md §9.5's own "parent"): this fn's
        // own turn, which used to be written as `X_FRAME` — the raw
        // address of that turn's persistent area. plans/M10.md item 0c2
        // makes it the turn's **`TurnId`**, a `u32` at +56, because both
        // readers only ever compare it for equality and neither needs an
        // address: `emit_group_cancelled_flags` below compares against
        // this fn's own id, and `emit_deadline_scan_and_delivery`
        // against the build-time id of the turn its unrolled arm is about.
        // Every cancellation observation site still decides the same
        // thing — whether a cancelled group terminates the observing
        // activation (a child started into the group) or merely hands it a
        // `CallError` (the `with`-block's own body).
        let word = ctx.cur_word();
        ctx.load_imm_naive(X_D, 0);
        for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
            w.text = format!("turn-id[{i}] {} <{fn_key}>", reg_name(X_D));
        }
        ctx.relocs.push(Reloc::TurnIdImm {
            word,
            key: fn_key.to_string(),
        });
        ctx.push(
            encode::enc_str_w_imm(X_D, X_CAND, OFF_GROUP_OWNER_TURN as u16),
            format!(
                "str w{X_D}, [{}, #{OFF_GROUP_OWNER_TURN}]",
                reg_name(X_CAND)
            ),
            CostRule::Store,
            None,
            &[X_D, X_CAND],
        );
        // new ambient lineage = i + 1, threaded into the lineage slots +
        // the `g` binding's own `group_temp`.
        ctx.load_imm(X_D, (i + 1) as i64);
        ctx.store_slot(X_D, ctx.frame.off(LINEAGE_GROUP_SLOT));
        ctx.store_slot(X_TAG, ctx.frame.off(LINEAGE_DEADLINE_SLOT));
        ctx.store_slot(X_D, ctx.frame.off(group_temp));

        // A successful init (this candidate's own `in_use == 0` case)
        // must always skip past the overflow abort that follows the last
        // candidate — including on the *last* candidate itself (a
        // disclosed off-by-one this golden's own first real boot caught:
        // an earlier draft only pushed this jump when a further candidate
        // remained, so the loop's own final successful init fell straight
        // through into the abort it had just escaped).
        let j = ctx.words.len();
        ctx.words
            .push(EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]));
        to_after.push(j);
        ctx.patch_skip(skip_try_next, SkipKind::Cbnz(X_D));
    }
    if gctx.arena_capacity == 0 {
        ctx.abort_fixed("with group: arena capacity is zero (internal error)");
    } else {
        ctx.abort_fixed("with group: arena capacity exceeded (plans/M6.md item F)");
    }
    let after = ctx.cur_word();
    for j in to_after {
        let delta = (after as i64 - j as i64) as i32 * 4;
        ctx.words[j] = EmittedWord::new(
            encode::enc_b(delta),
            format!("b #{delta}"),
            CostRule::Branch,
            None,
            &[],
        );
    }
    Ok(())
}

/// `g.start(callee, args...)` (02-language.md §9.5, item F #2): admits a
/// child directly — no mailbox, no `rt_enqueue` (a free async fn has no
/// mailbox at all, only its own dedicated free-turn area, item C/D's own
/// sizing) — by writing the *current* ambient lineage into the callee's
/// own persistent frame (so the child's own awaits/nested groups see this
/// group as their parent) and calling its compiled entry directly, exactly
/// as if this were its first-ever activation. `emit_marshal_and_call`'s own
/// two-scalar-arg floor applies identically (item C's hand-assembled-
/// dispatch floor, unchanged). The call's own return status is handled
/// inline, synchronously, right here — never deferred to a poll for a
/// child that never suspends: `TURN_STATUS_COMPLETED`/`_CANCELLED` harvest
/// immediately (result written into this group's own child-result slot,
/// `active_children` decremented, the join waiter woken if this was the
/// last one still outstanding); `TURN_STATUS_SUSPENDED` leaves the child
/// parked in its own turn area, for `layout.rs`'s own per-site poll routine
/// to keep driving on later scheduler ticks (`rt_run_one`'s own extension).
#[allow(clippy::too_many_arguments)]
fn emit_group_start(
    group_temp: Temp,
    callee_key: &str,
    arg_temps: &[Temp],
    ctx: &mut FnCtx,
    gctx: &GroupCtx,
    fn_key: &str,
) -> Result<(), CodegenError> {
    let child_index = *gctx.child_index.get(callee_key).ok_or_else(|| {
        CodegenError::internal(format!(
            "g.start callee `{callee_key}` has no child-slot ordinal (compute_group_child_indices \
             was not run over the whole program, or disagrees with this fn's own lowering)"
        ))
    })?;
    if arg_temps.len() > 2 {
        return Err(CodegenError::unimplemented(
            "more than 2 scalar `g.start` args (item C's own hand-assembled mailbox-slot floor)",
        ));
    }

    // 04-compiler.md §4's own step one, "atomically closes admission":
    // a `g.start` into an already-cancelled group never runs its child at
    // all — the child's own result slot resolves straight to
    // `CallError::Cancelled` and `active_children` is never incremented,
    // so a `join_all` already parked on this group is not made to wait for
    // a child that will never run. **Disclosed floor, not a silent
    // narrowing**: 04 §4 says a closed-admission attempt gets
    // `NotAdmitted` with its payloads returned; M6's own composition floor
    // has exactly one non-`Ok` variant (`emit_compose_group_join_result`'s
    // own doc), so it resolves `Cancelled` here, and `g.start`'s arguments
    // are plain scalars with nothing to hand back.
    emit_group_addr_from_temp(ctx, group_temp, X_B, X_A, gctx);
    ctx.push(
        encode::enc_ldr_x_imm(X_C, X_B, OFF_GROUP_CANCELLED as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_CANCELLED}]",
            reg_name(X_C),
            reg_name(X_B)
        ),
        CostRule::Load,
        Some(X_C),
        &[X_B],
    );
    let skip_admit = ctx.emit_skip(SkipKind::Cbz(X_C));
    ctx.load_imm(X_A, 1); // tag = Err(CallError::Cancelled)
    ctx.push(
        encode::enc_str_x_imm(X_A, X_B, group_child_tag_off(child_index) as u16),
        format!(
            "str {}, [{}, #{}]",
            reg_name(X_A),
            reg_name(X_B),
            group_child_tag_off(child_index)
        ),
        CostRule::Store,
        None,
        &[X_A, X_B],
    );
    ctx.push(
        encode::enc_str_x_imm(X_ZR, X_B, group_child_payload_off(child_index) as u16),
        format!(
            "str xzr, [{}, #{}]",
            reg_name(X_B),
            group_child_payload_off(child_index)
        ),
        CostRule::Store,
        None,
        &[X_ZR, X_B],
    );
    let to_after = ctx.words.len();
    ctx.words
        .push(EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]));
    ctx.patch_skip(skip_admit, SkipKind::Cbz(X_C));

    // Write the ambient lineage into the child's own persistent frame
    // (Temp(0)/Temp(1) — always the first two slots past the child's own
    // turn record header) before ever calling it.
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_C, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("turn-frame[{}] {} <{callee_key}>", 0, reg_name(X_C));
    }
    ctx.relocs.push(Reloc::TurnFrameAddr {
        word,
        key: callee_key.to_string(),
    });
    ctx.load_slot(X_D, ctx.frame.off(LINEAGE_GROUP_SLOT));
    ctx.push(
        encode::enc_str_x_imm(X_D, X_C, (TURN_RECORD_SIZE) as u16),
        format!(
            "str {}, [{}, #{TURN_RECORD_SIZE}]",
            reg_name(X_D),
            reg_name(X_C)
        ),
        CostRule::Store,
        None,
        &[X_D, X_C],
    );
    ctx.load_slot(X_D, ctx.frame.off(LINEAGE_DEADLINE_SLOT));
    ctx.push(
        encode::enc_str_x_imm(X_D, X_C, (TURN_RECORD_SIZE + 8) as u16),
        format!(
            "str {}, [{}, #{}]",
            reg_name(X_D),
            reg_name(X_C),
            TURN_RECORD_SIZE + 8
        ),
        CostRule::Store,
        None,
        &[X_D, X_C],
    );
    // Mark it busy/fresh (suspended=0, resume_ready=0 — a truly fresh
    // activation, its own entry's fresh-vs-resume fork reads this).
    ctx.load_imm(X_D, 1);
    ctx.push(
        encode::enc_str_x_imm(X_D, X_C, OFF_TURN_BUSY as u16),
        format!(
            "str {}, [{}, #{OFF_TURN_BUSY}]",
            reg_name(X_D),
            reg_name(X_C)
        ),
        CostRule::Store,
        None,
        &[X_D, X_C],
    );
    for off in [OFF_TURN_SUSPENDED, OFF_TURN_RESUME_READY, OFF_TURN_WAKER] {
        ctx.push(
            encode::enc_str_x_imm(X_ZR, X_C, off as u16),
            format!("str xzr, [{}, #{off}]", reg_name(X_C)),
            CostRule::Store,
            None,
            &[X_ZR, X_C],
        );
    }

    // Group address (computed once, before the call, so admission can
    // increment `active_children` *before* this child ever runs — the
    // real bookkeeping `g.join_all()`'s own "how many children remain
    // outstanding" reads; item F's own first real boot caught this
    // exact ordering bug: an earlier draft only ever touched
    // `active_children` *after* the call returned, and in the wrong
    // direction, so a synchronously-completing child left it permanently
    // wrong and `join_all` could never see zero).
    let group_addr_reg = X_D;
    ctx.load_slot(X_E, ctx.frame.off(group_temp)); // encoded group id (i+1)
    ctx.push(
        encode::enc_sub_imm(X_E, X_E, 1, true),
        format!("sub {}, {}, #1", reg_name(X_E), reg_name(X_E)),
        CostRule::Alu,
        Some(X_E),
        &[X_E],
    );
    ctx.load_imm(X_F, gctx.slot_size() as i64);
    ctx.mul_reg(X_E, X_E, X_F);
    let word = ctx.cur_word();
    ctx.load_imm_naive(group_addr_reg, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = "group-arena-base (g.start)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.add_reg(group_addr_reg, group_addr_reg, X_E);
    // active_children += 1 (admission).
    ctx.push(
        encode::enc_ldr_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
        CostRule::Load,
        Some(X_A),
        &[group_addr_reg],
    );
    ctx.push(
        encode::enc_add_imm(X_A, X_A, 1, true),
        format!("add {}, {}, #1", reg_name(X_A), reg_name(X_A)),
        CostRule::Alu,
        Some(X_A),
        &[X_A],
    );
    ctx.push(
        encode::enc_str_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "str {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
        CostRule::Store,
        None,
        &[X_A, group_addr_reg],
    );

    // Marshal args (at most 2 scalars) directly into x0/x1 (a fresh call's
    // own receiver-less ABI: a free async fn's entry takes no receiver, so
    // `x0`/`x1` are its first two ordinary params, mirroring
    // `emit_async_entry`'s own fresh-path arg spill exactly one level up —
    // no `rt_enqueue`-style args-pointer marshaling needed here at all,
    // since this is a direct call, not an admission). `group_addr_reg`
    // (`X_D`) and `X_E`/`X_F` are dead by now — safe to clobber with the
    // marshaled args/the call itself.
    for (i, t) in arg_temps.iter().enumerate() {
        ctx.load_slot(i as u8, ctx.frame.off(*t));
    }
    let arg_srcs: Vec<u8> = (0..arg_temps.len()).map(|i| i as u8).collect();
    ctx.bl_symbolic_call(callee_key, &arg_srcs);
    // `X_FRAME` (x28) is *not* preserved by this call the way a hand-
    // assembled runtime routine's own contract preserves it (`rt_enqueue`/
    // `__wrela_checkpoint_service`'s own documented "must preserve x28")
    // — `callee_key`'s own compiled entry is an ordinary async fn, which
    // loads *its own* persistent-frame address into `X_FRAME` as the very
    // first thing it does (`emit_async_entry`'s own doc). This item's
    // first real HVF boot caught exactly this: every `ctx.store_slot`/
    // `load_slot` call below this line silently addressed the *callee's*
    // own frame instead of this fn's own, once the child had run — must
    // reload this fn's own frame address fresh before touching any slot
    // again, exactly like `emit_async_entry`'s own initial load.
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_FRAME, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("turn-frame[{}] {} <{fn_key}>", 0, reg_name(X_FRAME));
    }
    ctx.relocs.push(Reloc::TurnFrameAddr {
        word,
        key: fn_key.to_string(),
    });
    // x0 = status; x1 = value when COMPLETED. Recompute the group address
    // fresh (the callee's own compiled body may have clobbered any of
    // x0..x17 — nothing survives a `BL` here by convention).
    ctx.load_slot(X_E, ctx.frame.off(group_temp));
    ctx.push(
        encode::enc_sub_imm(X_E, X_E, 1, true),
        format!("sub {}, {}, #1", reg_name(X_E), reg_name(X_E)),
        CostRule::Alu,
        Some(X_E),
        &[X_E],
    );
    ctx.load_imm(X_F, gctx.slot_size() as i64);
    ctx.mul_reg(X_E, X_E, X_F);
    let word = ctx.cur_word();
    ctx.load_imm_naive(group_addr_reg, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = "group-arena-base (g.start harvest)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.add_reg(group_addr_reg, group_addr_reg, X_E);

    ctx.push_flags(
        encode::enc_cmp_imm(0, TURN_STATUS_SUSPENDED as u16, true),
        format!("cmp x0, #{TURN_STATUS_SUSPENDED}"),
        CostRule::Alu,
        None,
        &[0],
        FlagEffect::Write,
    );
    let skip_still_running = ctx.emit_skip(SkipKind::Cond(Cond::Eq)); // suspended: leave parked, nothing to harvest yet.

    // Completed or cancelled: tag = 0 (Ok) unless status ==
    // TURN_STATUS_CANCELLED, in which case tag = 1 (the composed
    // `CallError::Cancelled`, this item's own floor — module doc above).
    ctx.push_flags(
        encode::enc_cmp_imm(0, TURN_STATUS_CANCELLED as u16, true),
        format!("cmp x0, #{TURN_STATUS_CANCELLED}"),
        CostRule::Alu,
        None,
        &[0],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_cset(X_A, Cond::Eq, true),
        format!("cset {}, eq", reg_name(X_A)),
        CostRule::Alu,
        Some(X_A),
        &[],
        FlagEffect::Read,
    );
    ctx.push(
        encode::enc_str_x_imm(X_A, group_addr_reg, group_child_tag_off(child_index) as u16),
        format!(
            "str {}, [{}, #{}]",
            reg_name(X_A),
            reg_name(group_addr_reg),
            group_child_tag_off(child_index)
        ),
        CostRule::Store,
        None,
        &[X_A, group_addr_reg],
    );
    ctx.push(
        encode::enc_str_x_imm(
            1,
            group_addr_reg,
            group_child_payload_off(child_index) as u16,
        ),
        format!(
            "str x1, [{}, #{}]",
            reg_name(group_addr_reg),
            group_child_payload_off(child_index)
        ),
        CostRule::Store,
        None,
        &[1, group_addr_reg],
    );
    // Completed/cancelled (never suspended): decrement active_children —
    // this admission's own count is now settled — and clear this child's
    // own `busy` (harvested inline; available for a later loop iteration
    // of this same `g.start` site to reuse). A suspended child leaves both
    // untouched: still `busy`, still counted `active`, for
    // `codegen::emit_rt_child_poll` to harvest later.
    ctx.push(
        encode::enc_ldr_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
        CostRule::Load,
        Some(X_A),
        &[group_addr_reg],
    );
    ctx.push(
        encode::enc_sub_imm(X_A, X_A, 1, true),
        format!("sub {}, {}, #1", reg_name(X_A), reg_name(X_A)),
        CostRule::Alu,
        Some(X_A),
        &[X_A],
    );
    ctx.push(
        encode::enc_str_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "str {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
        CostRule::Store,
        None,
        &[X_A, group_addr_reg],
    );
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_A, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = format!("turn-frame[{}] {} <{callee_key}>", 0, reg_name(X_A));
    }
    ctx.relocs.push(Reloc::TurnFrameAddr {
        word,
        key: callee_key.to_string(),
    });
    ctx.push(
        encode::enc_str_x_imm(X_ZR, X_A, OFF_TURN_BUSY as u16),
        format!("str xzr, [{}, #{OFF_TURN_BUSY}]", reg_name(X_A)),
        CostRule::Store,
        None,
        &[X_ZR, X_A],
    );

    ctx.patch_skip(skip_still_running, SkipKind::Cond(Cond::Eq));
    let after = ctx.cur_word();
    let delta = (after as i64 - to_after as i64) as i32 * 4;
    ctx.words[to_after] = EmittedWord::new(
        encode::enc_b(delta),
        format!("b #{delta}"),
        CostRule::Branch,
        None,
        &[],
    );
    Ok(())
}

/// The group's own closing bracket (item F #1/#4): free the arena slot
/// and restore the *parent's* ambient lineage into the frame's lineage
/// slots — the cleanup chain itself (`GroupClose::cleanup_states`) is
/// never this op's own job: `flowwir_lower.rs`'s own `lower_with_group`
/// already wires the flat state graph so the natural fall-through from
/// this op's own flat position jumps into the (possibly empty) cleanup
/// chain and back out, in reverse registration order, entirely via
/// ordinary `Transition::Jump` edges — this op runs exactly once, at the
/// group's own natural close, regardless of whether any child/await inside
/// it ever observed cancellation (02-language.md §10: a `defer` runs on
/// every exit).
fn emit_group_close(
    group_temp: Temp,
    ctx: &mut FnCtx,
    gctx: &GroupCtx,
) -> Result<(), CodegenError> {
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_A, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = "group-arena-base (GroupClose)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.load_slot(X_B, ctx.frame.off(group_temp)); // encoded group id (i+1)
    ctx.push(
        encode::enc_sub_imm(X_B, X_B, 1, true),
        format!("sub {}, {}, #1", reg_name(X_B), reg_name(X_B)),
        CostRule::Alu,
        Some(X_B),
        &[X_B],
    );
    ctx.load_imm(X_C, gctx.slot_size() as i64);
    ctx.mul_reg(X_B, X_B, X_C);
    ctx.add_reg(X_A, X_A, X_B);
    // Restore ambient lineage from this group's own `parent_group`.
    ctx.push(
        encode::enc_ldr_x_imm(X_B, X_A, OFF_GROUP_PARENT as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_PARENT}]",
            reg_name(X_B),
            reg_name(X_A)
        ),
        CostRule::Load,
        Some(X_B),
        &[X_A],
    );
    ctx.load_imm(X_C, GROUP_NO_PARENT as i64);
    ctx.cmp_reg(X_B, X_C);
    let skip_no_parent = ctx.emit_skip(SkipKind::Cond(Cond::Eq)); // == GROUP_NO_PARENT -> no-parent arm

    // Had a parent: new ambient group = parent_index + 1; new ambient
    // deadline = the parent slot's own (already-narrowed) deadline.
    ctx.push(
        encode::enc_add_imm(X_B, X_B, 1, true),
        format!("add {}, {}, #1", reg_name(X_B), reg_name(X_B)),
        CostRule::Alu,
        Some(X_B),
        &[X_B],
    );
    ctx.store_slot(X_B, ctx.frame.off(LINEAGE_GROUP_SLOT));
    ctx.push(
        encode::enc_sub_imm(X_C, X_B, 1, true),
        format!("sub {}, {}, #1", reg_name(X_C), reg_name(X_B)),
        CostRule::Alu,
        Some(X_C),
        &[X_B],
    );
    ctx.load_imm(X_D, gctx.slot_size() as i64);
    ctx.mul_reg(X_C, X_C, X_D);
    let word2 = ctx.cur_word();
    ctx.load_imm_naive(X_D, 0);
    for w in ctx.words[word2..word2 + 4].iter_mut() {
        w.text = "group-arena-base (GroupClose parent deadline)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word: word2 });
    ctx.add_reg(X_C, X_D, X_C);
    ctx.push(
        encode::enc_ldr_x_imm(X_D, X_C, OFF_GROUP_DEADLINE as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_DEADLINE}]",
            reg_name(X_D),
            reg_name(X_C)
        ),
        CostRule::Load,
        Some(X_D),
        &[X_C],
    );
    ctx.store_slot(X_D, ctx.frame.off(LINEAGE_DEADLINE_SLOT));
    let to_free = ctx.cur_word();
    ctx.words
        .push(EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]));

    ctx.patch_skip(skip_no_parent, SkipKind::Cond(Cond::Eq));
    // No parent: ambient becomes "none" (0/0).
    ctx.store_slot(X_ZR, ctx.frame.off(LINEAGE_GROUP_SLOT));
    ctx.store_slot(X_ZR, ctx.frame.off(LINEAGE_DEADLINE_SLOT));

    // Both arms converge here: free the slot.
    let free = ctx.cur_word();
    let delta = (free as i64 - to_free as i64) as i32 * 4;
    ctx.words[to_free] = EmittedWord::new(
        encode::enc_b(delta),
        format!("b #{delta}"),
        CostRule::Branch,
        None,
        &[],
    );
    ctx.push(
        encode::enc_str_x_imm(X_ZR, X_A, OFF_GROUP_IN_USE as u16),
        format!("str xzr, [{}, #{OFF_GROUP_IN_USE}]", reg_name(X_A)),
        CostRule::Store,
        None,
        &[X_ZR, X_A],
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_flow_op(
    op: &FlowInst,
    f: &MwirFn,
    ctx: &mut FnCtx,
    method_index: &ActorMethodIndex,
    gctx: &GroupCtx,
    fn_key: &str,
) -> Result<(), CodegenError> {
    match op {
        FlowInst::Mwir(inst) => emit_one(inst, f, ctx),
        FlowInst::SelfPath { dst, path } => emit_self_path(*dst, path, f, ctx),
        FlowInst::Now { dst } => {
            emit_now(*dst, ctx);
            Ok(())
        }
        FlowInst::Entropy { dst, n } => emit_entropy(*dst, *n, ctx),
        FlowInst::Duration { dst, n } => {
            // `ms(n)` -> nanoseconds. plans/M6.md item F: item B/D left
            // this an opaque passthrough ("a real tick-scale conversion
            // has no required golden to derive one from"); item F's own
            // deadline goldens are that golden. The scale is the one the
            // whole machine already agrees on: `now()` reads
            // `CLOCK_MMIO_ADDR`, which 06-machine.md §5/decision 13 define
            // as monotonic **nanoseconds**, and the comptime evaluator has
            // scaled `ms(n)` to `n * 1_000_000` since item A
            // (`eval::interp::eval_intrinsic`'s own `"ms"` arm) — this arm
            // was the tier that disagreed, and 02-language.md §9.5's own
            // `now() + ms(50)` example is only meaningful once it does not.
            const NS_PER_MS: i64 = 1_000_000;
            ctx.load_slot(X_A, ctx.frame.off(*n));
            ctx.load_imm(X_B, NS_PER_MS);
            ctx.mul_reg(X_A, X_A, X_B);
            ctx.store_slot(X_A, ctx.frame.off(*dst));
            Ok(())
        }
        FlowInst::Send {
            dst,
            target: _,
            method_key,
            arg_temps,
            take_arg_temps,
        } => emit_send(
            *dst,
            method_key,
            arg_temps,
            take_arg_temps,
            ctx,
            method_index,
        ),
        FlowInst::GroupCreate {
            group_temp,
            capacity,
            deadline,
        } => emit_group_create(*group_temp, *capacity, *deadline, ctx, gctx, fn_key),
        FlowInst::GroupStart {
            group_temp,
            callee_key,
            arg_temps,
        } => emit_group_start(*group_temp, callee_key, arg_temps, ctx, gctx, fn_key),
        FlowInst::GroupClose { group_temp, .. } => emit_group_close(*group_temp, ctx, gctx),
    }
}

/// The shared cancellation-observation test (plans/M6.md item F #3/#4,
/// decision 6/7's own flip witness): reads the currently-executing turn's
/// own ambient group (`LINEAGE_GROUP_SLOT` — 0 means "no ambient group,"
/// nothing to test) and, if it names a real group, its own `cancelled`
/// flag; when cancelled, this activation terminates immediately via the
/// shared cancellation tail (`total + 1`'s own sentinel position, module
/// doc on `emit_async_cancelled_tail`) — "the cancelled frame never
/// resumes" (04-compiler.md §4). Called from exactly two places, both
/// already checkpoints by construction: a loop back-edge
/// (`emit_transition`'s `Jump` arm) and an await's own resume stub
/// (`emit_await_resume`'s `ActorCall` arm) — never from a sync fn's own
/// `checkpoint()` call sites (a sync fn has no persistent frame/ambient
/// lineage at all, decision 4's own reading of "sync turn mid-execution").
/// The one shared read of "what is my ambient group's cancellation state?"
/// (plans/M6.md item F #2). Leaves, branch-free at the use site:
///
/// - `X_C = 1` iff this turn has an ambient group AND that group's own
///   `cancelled` word is set, else `0`;
/// - `X_D = 1` iff that same group's `owner_turn` is this turn — since
///   plans/M10.md item 0c2 a `TurnId` compared against this fn's own
///   relocated id, not an address compared against `X_FRAME` — else `0`:
///   the child-vs-owner distinction `OFF_GROUP_OWNER_TURN`'s own doc
///   comment explains.
///
/// Clobbers `X_A`/`X_B`/`X_E`. A no-op producing `X_C = X_D = 0` when the
/// whole build has no group arena at all, which is what keeps every
/// pre-item-F async golden byte-identical (`emit_checkpoint_cancellation_test`
/// below has the full reasoning); callers must not emit it in that case.
fn emit_group_cancelled_flags(ctx: &mut FnCtx, fn_key: &str, gctx: &GroupCtx) {
    ctx.push(
        encode::enc_movz(X_C, 0, 0, true),
        format!("movz {}, #0", reg_name(X_C)),
        CostRule::MovWide,
        Some(X_C),
        &[],
    );
    ctx.push(
        encode::enc_movz(X_D, 0, 0, true),
        format!("movz {}, #0", reg_name(X_D)),
        CostRule::MovWide,
        Some(X_D),
        &[],
    );
    ctx.load_slot(X_A, ctx.frame.off(LINEAGE_GROUP_SLOT));
    let skip_no_group = ctx.emit_skip(SkipKind::Cbz(X_A));
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_B, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = "group-arena-base (cancel flags)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.push(
        encode::enc_sub_imm(X_A, X_A, 1, true),
        format!("sub {}, {}, #1", reg_name(X_A), reg_name(X_A)),
        CostRule::Alu,
        Some(X_A),
        &[X_A],
    );
    ctx.load_imm(X_E, gctx.slot_size() as i64);
    ctx.mul_reg(X_A, X_A, X_E);
    ctx.add_reg(X_B, X_B, X_A);
    ctx.push(
        encode::enc_ldr_x_imm(X_A, X_B, OFF_GROUP_CANCELLED as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_CANCELLED}]",
            reg_name(X_A),
            reg_name(X_B)
        ),
        CostRule::Load,
        Some(X_A),
        &[X_B],
    );
    ctx.push_flags(
        encode::enc_cmp_imm(X_A, 0, true),
        format!("cmp {}, #0", reg_name(X_A)),
        CostRule::Alu,
        None,
        &[X_A],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_cset(X_C, Cond::Ne, true),
        format!("cset {}, ne", reg_name(X_C)),
        CostRule::Alu,
        Some(X_C),
        &[],
        FlagEffect::Read,
    );
    // plans/M10.md item 0c2: `owner_turn` is a `TurnId` (a `u32` at +56),
    // so this is a 32-bit load compared against this fn's own relocated
    // `TurnId` immediate instead of a 64-bit load compared against
    // `X_FRAME`. Equality only — no index→address step is needed or
    // wanted here. `ldr w`/`cmp w`: an `x` load would fold the adjacent
    // word in as high bits.
    ctx.push(
        encode::enc_ldr_w_imm(X_A, X_B, OFF_GROUP_OWNER_TURN as u16),
        format!("ldr w{X_A}, [{}, #{OFF_GROUP_OWNER_TURN}]", reg_name(X_B)),
        CostRule::Load,
        Some(X_A),
        &[X_B],
    );
    let word = ctx.cur_word();
    ctx.load_imm_naive(X_E, 0);
    for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
        w.text = format!("turn-id[{i}] {} <{fn_key}>", reg_name(X_E));
    }
    ctx.relocs.push(Reloc::TurnIdImm {
        word,
        key: fn_key.to_string(),
    });
    ctx.push_flags(
        encode::enc_cmp_reg(X_A, X_E, false),
        format!("cmp w{X_A}, w{X_E}"),
        CostRule::Alu,
        None,
        &[X_A, X_E],
        FlagEffect::Write,
    );
    ctx.push_flags(
        encode::enc_cset(X_D, Cond::Eq, true),
        format!("cset {}, eq", reg_name(X_D)),
        CostRule::Alu,
        Some(X_D),
        &[],
        FlagEffect::Read,
    );
    ctx.patch_skip(skip_no_group, SkipKind::Cbz(X_A));
}

fn emit_checkpoint_cancellation_test(ctx: &mut FnCtx, gctx: &GroupCtx, fn_key: &str) {
    if gctx.arena_capacity == 0 {
        // No `with group(...)` exists anywhere in this build — a whole-
        // program fact (`layout::RuntimeTables::group_arena_capacity`),
        // not a per-fn one. Emitting nothing at all here (rather than a
        // "no ambient group" runtime check that would always pass) is
        // what keeps every pre-item-F async golden's own ASM byte-
        // identical: this fn becomes a true no-op, never touching
        // `ctx.words`, whenever the build has no group arena to address
        // in the first place (there would be no `Reloc::GroupArenaBase`
        // target to resolve against either).
        return;
    }
    // `word_offsets` has `total + 2` entries: `[total]` is the shared
    // completion epilogue and `[total + 1]` is the cancellation tail —
    // so the tail is `len() - 1`. **A real off-by-one the first
    // cancellation-bearing boot caught**: `len() - 2` named the
    // *completion* epilogue instead, so a cancelled child returned
    // `TURN_STATUS_COMPLETED` with a garbage reply and its group's own
    // child slot harvested as `Ok`, exactly as if it had finished.
    let cancelled_tail = ctx.word_offsets.len() - 1;
    emit_group_cancelled_flags(ctx, fn_key, gctx);
    // Terminate this activation iff the ambient group is cancelled AND
    // this turn is not that group's own owner (`OFF_GROUP_OWNER_TURN`'s
    // own doc comment): a `g.start`ed child's frame never resumes
    // (04-compiler.md §4), while the `with`-block's own body keeps
    // running so it can observe the `CallError` and reach its cleanup
    // chain (02-language.md §9.5).
    let skip_not_cancelled = ctx.emit_skip(SkipKind::Cbz(X_C));
    let skip_is_owner = ctx.emit_skip(SkipKind::Cbnz(X_D));
    ctx.b_unconditional(cancelled_tail);
    ctx.patch_skip(skip_is_owner, SkipKind::Cbnz(X_D));
    ctx.patch_skip(skip_not_cancelled, SkipKind::Cbz(X_C));
}

/// `word_offsets[total + 1]` — module doc on `emit_checkpoint_cancellation_test`.
/// Reports `TURN_STATUS_CANCELLED`; no mut-receiver writeback (04 §4: "the
/// cancelled frame never resumes" — its own last-observed state is
/// discarded, never published to `self`).
fn emit_async_cancelled_tail(ctx: &mut FnCtx) {
    // M11 F (decision 793): deterministic zero payload for cancelled harvest.
    ctx.push(
        encode::enc_str_x_imm(X_ZR, X_FRAME, OFF_TURN_REPLY as u16),
        format!(
            "str xzr, [{}, #{OFF_TURN_REPLY}]  ; cancelled → turn.reply = 0",
            reg_name(X_FRAME)
        ),
        CostRule::Store,
        None,
        &[X_ZR, X_FRAME],
    );
    ctx.load_imm(0, TURN_STATUS_CANCELLED as i64);
    ctx.load_slot(X_LR, ctx.frame.lr_off);
    ctx.push(
        encode::enc_ret(X_LR),
        "ret".to_string(),
        CostRule::Branch,
        None,
        &[X_LR],
    );
}

/// `g.join_all()`'s own result composition (item F #2, shared by both the
/// "already resolved" immediate path and the real resume path, below):
/// copies each of `child_count` children's own (tag, payload) pair
/// straight from the group arena's own child-result slots into
/// `result_temp`'s frame area — `Array[CallError-composed child type;
/// child_count]`, one 16-byte `Result` element per child, in declared
/// order (`GroupCtx::child_index`'s own ordinal numbering). `group_reg`
/// must already hold the group's own arena address, and must stay live
/// across the whole loop — it is the base of every load below.
///
/// **A real bug the first real HVF boot of `golden/boot-group-join`
/// caught** (recorded, per house rule, not silently fixed away): an
/// earlier draft loaded each child's tag/payload into `X_A`/`X_B` while
/// both call sites passed `X_B` as `group_reg` — so child 0's own
/// *payload* load overwrote the arena address itself, and child 1's tag
/// load addressed `payload_of_child_0 + 72`. With `fetch_a`'s own reply
/// (20) in that slot, that is a 64-bit access to `0x5c`: 4-aligned, not
/// 8-aligned, and this machine runs with the MMU off (every access is
/// Device-nGnRnE, naturally-alignment-checked), so it is an EL1
/// **alignment** fault — taken to `VBAR_EL1 + 0x200` with `VBAR_EL1`
/// never set, i.e. the reported `esr=0x82000006, ipa=0x200, pc=0x200` was
/// the *second* fault (an instruction abort on the unmapped vector page),
/// never a wild branch. Invisible for any single-child group (the clobber
/// lands on the last use) and invisible to dump review. The value
/// registers are now `X_C`/`X_D`, and their disjointness from
/// `group_reg` is checked here rather than trusted.
/// **A second real bug the same boot caught, one layer down** (recorded,
/// not silently fixed): an earlier draft wrote each element at a hardcoded
/// 16-byte stride with the payload as a bare scalar. The composed element
/// type is `Result[T, CallError[E]]`, whose real size is
/// `8 (tag) + max(size_of(T), size_of(CallError[E]))` — for this item's own
/// `Result[u64, CallError[never]]` that is `8 + max(8, 16) = 24`, not 16.
/// The stride is now derived from the array temp's own real size, and the
/// `Err` arm composes a real `CallError::Cancelled` value rather than a
/// raw scalar.
fn emit_compose_group_join_result(
    ctx: &mut FnCtx,
    group_reg: u8,
    result_temp: Temp,
    child_count: usize,
) -> Result<(), CodegenError> {
    const VAL_TAG: u8 = X_C;
    const VAL_PAYLOAD: u8 = X_D;
    const VAL_CONST: u8 = X_E;
    if group_reg == VAL_TAG || group_reg == VAL_PAYLOAD || group_reg == VAL_CONST {
        return Err(CodegenError::internal(format!(
            "`g.join_all()` composition: the group-address register {} is one of the value \
             registers this loop loads into, so it would be clobbered mid-loop",
            reg_name(group_reg)
        )));
    }
    if child_count == 0 {
        return Ok(());
    }
    let total = ctx.frame.size_of_temp(result_temp);
    if total % child_count != 0 {
        return Err(CodegenError::internal(format!(
            "`g.join_all()`'s own result array ({total} bytes) does not divide evenly into \
             {child_count} elements"
        )));
    }
    let elem_size = total / child_count;
    // Every sum this backend lays out is `tag` (one 8-byte slot) followed
    // by its payload area (`enum_payload_offset`'s own `TAG` constant —
    // the one fixed rule, shared with `EnumPayload`'s own emission), so a
    // composed element is `[+0] = Result tag`, `[+8..elem_size] = payload`.
    const PAYLOAD_OFF: usize = 8;
    if elem_size < PAYLOAD_OFF + 8 {
        return Err(CodegenError::internal(format!(
            "`g.join_all()`'s own composed element is {elem_size} bytes — too small to hold a \
             tag plus one payload word"
        )));
    }
    let result_off = ctx.frame.off(result_temp);
    for c in 0..child_count {
        let elem_off = result_off + c * elem_size;
        ctx.push(
            encode::enc_ldr_x_imm(VAL_TAG, group_reg, group_child_tag_off(c) as u16),
            format!(
                "ldr {}, [{}, #{}]",
                reg_name(VAL_TAG),
                reg_name(group_reg),
                group_child_tag_off(c)
            ),
            CostRule::Load,
            Some(VAL_TAG),
            &[group_reg],
        );
        ctx.push(
            encode::enc_ldr_x_imm(VAL_PAYLOAD, group_reg, group_child_payload_off(c) as u16),
            format!(
                "ldr {}, [{}, #{}]",
                reg_name(VAL_PAYLOAD),
                reg_name(group_reg),
                group_child_payload_off(c)
            ),
            CostRule::Load,
            Some(VAL_PAYLOAD),
            &[group_reg],
        );
        // The arena's own child tag is already the `Result` tag by
        // construction (0 = `Ok`, 1 = `Err`); its payload word is the
        // child's scalar reply, which is only meaningful on the `Ok` side.
        // On the `Err` side the payload area holds a whole
        // `CallError[E]` value, whose own first word is its variant tag —
        // `Cancelled` (02-language.md §9.4's declared variant order:
        // `Op`, `Cancelled`, `DeadlineExceeded`, `NotAdmitted`,
        // `PeerFailed`), the only non-`Ok` outcome this item's own runtime
        // can produce. Branch-free, mirroring `emit_group_create`'s own
        // deadline narrowing.
        ctx.load_imm(VAL_CONST, CALL_ERROR_TAG_CANCELLED as i64);
        ctx.push_flags(
            encode::enc_cmp_imm(VAL_TAG, 0, true),
            format!("cmp {}, #0", reg_name(VAL_TAG)),
            CostRule::Alu,
            None,
            &[VAL_TAG],
            FlagEffect::Write,
        );
        ctx.push_flags(
            encode::enc_csel(VAL_PAYLOAD, VAL_PAYLOAD, VAL_CONST, Cond::Eq, true),
            format!(
                "csel {}, {}, {}, eq",
                reg_name(VAL_PAYLOAD),
                reg_name(VAL_PAYLOAD),
                reg_name(VAL_CONST)
            ),
            CostRule::Alu,
            Some(VAL_PAYLOAD),
            &[VAL_PAYLOAD, VAL_CONST],
            FlagEffect::Read,
        );
        ctx.store_slot(VAL_TAG, elem_off);
        ctx.store_slot(VAL_PAYLOAD, elem_off + PAYLOAD_OFF);
        // Zero the rest of the payload area: dead union padding on the
        // `Ok` side, and `CallError::Cancelled`'s own (empty) payload on
        // the `Err` side — deterministic either way, never stale bytes.
        let mut w = PAYLOAD_OFF + 8;
        while w < elem_size {
            ctx.store_slot(X_ZR, elem_off + w);
            w += 8;
        }
    }
    Ok(())
}

/// Computes this group's own arena address (`group_temp`'s own encoded
/// `arena_index + 1` value) into `dst_reg` — the shared address-from-
/// group-temp shape `GroupJoin`'s suspend/resume/immediate paths all need.
fn emit_group_addr_from_temp(
    ctx: &mut FnCtx,
    group_temp: Temp,
    dst_reg: u8,
    scratch_reg: u8,
    gctx: &GroupCtx,
) {
    let word = ctx.cur_word();
    ctx.load_imm_naive(dst_reg, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.text = "group-arena-base (join_all)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.load_slot(scratch_reg, ctx.frame.off(group_temp));
    ctx.push(
        encode::enc_sub_imm(scratch_reg, scratch_reg, 1, true),
        format!(
            "sub {}, {}, #1",
            reg_name(scratch_reg),
            reg_name(scratch_reg)
        ),
        CostRule::Alu,
        Some(scratch_reg),
        &[scratch_reg],
    );
    // arena index -> byte offset (a real bug this golden's first real boot
    // caught: an earlier draft added the raw index to the arena base
    // instead of `index * slot_size`, invisible for arena index 0
    // alone since `0 * anything == 0`, wrong for any other slot).
    ctx.load_imm(X_D, gctx.slot_size() as i64);
    ctx.mul_reg(scratch_reg, scratch_reg, X_D);
    ctx.add_reg(dst_reg, dst_reg, scratch_reg);
}

/// plans/M7.md item Z1: the declared reply type of one `Await{ActorCall}`
/// site, but only when it is an aggregate — i.e. exactly when that site
/// uses the wide reply transport (a staging slot + `x8`) instead of the
/// turn record's own scalar reply word. `None` means "scalar reply,"
/// which is every M6 await site and the case that must keep emitting the
/// identical instruction sequence (decision 9c).
///
/// The single predicate `flow_reply_stage_size` (which reserves the slot),
/// `emit_await_suspend` (which publishes its address) and
/// `emit_await_resume` (which reads the staged value back) all share, so
/// no two of them can disagree about one site.
/// 03-hardware.md §5: is this `Await{ActorCall}` site's own result the
/// bare `Receipt[P]` of the handoff calling convention rather than 02
/// §9.4's composed `Result`? One predicate, read by `emit_await_resume`
/// (which reads the reply back) and by `flow_reply_stage_size` /
/// `aggregate_reply_of_await` (which must *not* reserve a staging slot
/// for it — a receipt is one opaque word, `is_aggregate`'s own sealed-
/// authority arm).
fn is_handoff_receipt_reply(ty: &Type) -> bool {
    matches!(ty, Type::Named(n, _) if n == "Receipt")
}

fn aggregate_reply_of_await(f: &MwirFn, result_temp: Temp) -> Option<Type> {
    let declared = crate::sema::bodies::decompose_call_error(&f.temp_types[result_temp.0])?;
    is_aggregate(&declared).then_some(declared)
}

/// The suspend half of an `Await{ActorCall}`/`Await{GroupJoin}` (module doc's
/// own "park-and-resume" step 1): save `resume_state`, then either enqueue
/// a message with this turn's own waker (`ActorCall`) or, for
/// `GroupJoin`, either resolve immediately (every child already harvested
/// — `active_children == 0` — this item's own disclosed floor: a group
/// whose children all completed *synchronously* inside their own
/// `g.start` never gets a wake event to park on, so this path composes the
/// result right here and continues without ever leaving the fn) or
/// register as this group's own `join_waiter` and park for real.
#[allow(clippy::too_many_arguments)]
fn emit_await_suspend(
    what: &AwaitKind,
    resume_state: usize,
    result_temp: Temp,
    f: &MwirFn,
    ctx: &mut FnCtx,
    method_index: &ActorMethodIndex,
    gctx: &GroupCtx,
    // plans/M10.md item 0c1: this fn's own `program.fns` key — the
    // `Reloc::TurnIdImm` key for "this turn", which is both the waker of
    // every message it awaits and the owner of its own reply staging slot.
    fn_key: &str,
    state_temp: Temp,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match what {
        AwaitKind::ActorCall {
            target_temp: _,
            method_key,
            arg_temps,
            take_arg_temps,
        } => {
            let (actor, idx) = lookup_method_idx(method_key, method_index)?;
            ctx.load_imm(X_A, resume_state as i64);
            ctx.store_slot(X_A, ctx.frame.off(state_temp));
            // plans/M7.md item Z1 (decision 9a/9c): publish this turn's own
            // staging-slot address for the callee's dispatch to pick up —
            // ONLY when the declared reply is an aggregate. A scalar reply
            // emits nothing at all here, so every M6 await site keeps its
            // instruction sequence byte-for-byte. It must land before the
            // `bl rt_enqueue` below, because a same-core callee can be
            // dispatched the moment this turn returns to the scheduler.
            if aggregate_reply_of_await(f, result_temp).is_some() {
                let stage_off = ctx.frame.reply_stage_off.ok_or_else(|| {
                    CodegenError::internal(
                        "an `await` with an aggregate declared reply but no reply staging slot \
                         (`build_frame_flow`/`flow_reply_stage_size` disagree with this site)",
                    )
                })?;
                // plans/M10.md item 0c1 (decision 565): this is the one
                // reference that does NOT reduce to a bare `TurnId`. The
                // value is *frame-interior* — `turn_base + slot_bias +
                // stage_off` — and `stage_off` is `Frame::reply_stage_off`,
                // assigned per fn in `build_frame`, while the reader is the
                // callee's dispatch arm, a different fn entirely. So it is
                // stored as two adjacent `u32`s in the one word it always
                // occupied: this turn's `TurnId` at `OFF_TURN_REPLY_SLOT`,
                // and the byte offset *within that turn area* at +4.
                // `TURN_RECORD_SIZE` and every frame offset are unchanged.
                let word = ctx.cur_word();
                ctx.load_imm_naive(X_A, 0);
                for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
                    w.text = format!("turn-id[{i}] {} <{fn_key}>", reg_name(X_A));
                }
                ctx.relocs.push(Reloc::TurnIdImm {
                    word,
                    key: fn_key.to_string(),
                });
                ctx.push(
                    encode::enc_str_w_imm(X_A, X_FRAME, OFF_TURN_REPLY_SLOT as u16),
                    format!(
                        "str w{X_A}, [{}, #{OFF_TURN_REPLY_SLOT}]",
                        reg_name(X_FRAME)
                    ),
                    CostRule::Store,
                    None,
                    &[X_A, X_FRAME],
                );
                let interior = (stage_off + ctx.slot_bias) as u16;
                ctx.push(
                    encode::enc_movz(X_A, interior, 0, false),
                    format!("movz w{X_A}, #{interior:#x}"),
                    CostRule::MovWide,
                    Some(X_A),
                    &[],
                );
                ctx.push(
                    encode::enc_str_w_imm(X_A, X_FRAME, OFF_TURN_REPLY_SLOT as u16 + 4),
                    format!(
                        "str w{X_A}, [{}, #{}]",
                        reg_name(X_FRAME),
                        OFF_TURN_REPLY_SLOT + 4
                    ),
                    CostRule::Store,
                    None,
                    &[X_A, X_FRAME],
                );
            }
            emit_marshal_and_call(
                idx,
                arg_temps,
                ctx,
                &rt_enqueue_symbol(&actor),
                Some(fn_key), // waker = this turn, by `TurnId`.
            )?;
            // plans/M13.md item H: enqueue-fail — caller still owns the
            // argument words; build
            // `Err(CallError.NotAdmitted(Admission.Full, (take_args...)))`
            // locally (reply_tag semantics still reserve tag 3 for the
            // resume path). Handoff receipts have no CallError channel.
            let composed_ty = &f.temp_types[result_temp.0];
            if is_handoff_receipt_reply(composed_ty) {
                let skip = ctx.emit_skip(SkipKind::Cbz(0));
                ctx.abort_fixed(&format!(
                    "await rejected: `{actor}`'s mailbox was full (a handoff `Receipt` has no \
                     CallError channel for NotAdmitted)"
                ));
                ctx.patch_skip(skip, SkipKind::Cbz(0));
            } else {
                // Admitted (x0 == 0) skips the reject arm and parks;
                // rejected falls through, composes NotAdmitted, resumes.
                let skip_admitted = ctx.emit_skip(SkipKind::Cbz(0));
                let result_off = ctx.frame.off(result_temp);
                let result_size = ctx.frame.size_of_temp(result_temp);
                emit_not_admitted_local(ctx, result_off, result_size, take_arg_temps)?;
                ctx.checkpoint();
                emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
                ctx.b_unconditional(state_flat_base[resume_state]);
                ctx.patch_skip(skip_admitted, SkipKind::Cbz(0));
            }
            // Park: suspended = 1, status = suspended, return to the
            // scheduler (the real park — control genuinely leaves this
            // fn; every other ready actor can now run).
            emit_park_and_return(ctx);
            Ok(())
        }
        AwaitKind::GroupJoin {
            group_temp,
            child_count,
        } => {
            if *child_count > gctx.max_children {
                return Err(CodegenError::unimplemented(&format!(
                    "`g.join_all()` over more than {} children (image GROUP_MAX_CHILDREN fact, \
                     plans/M12.md item F)",
                    gctx.max_children
                )));
            }
            emit_group_addr_from_temp(ctx, *group_temp, X_B, X_A, gctx);
            ctx.push(
                encode::enc_ldr_x_imm(X_C, X_B, OFF_GROUP_ACTIVE_CHILDREN as u16),
                format!(
                    "ldr {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
                    reg_name(X_C),
                    reg_name(X_B)
                ),
                CostRule::Load,
                Some(X_C),
                &[X_B],
            );
            let skip_park = ctx.emit_skip(SkipKind::Cbnz(X_C));
            // Immediate: every child already harvested — compose now and
            // fall straight through to the resume state, no scheduler
            // round-trip at all.
            emit_compose_group_join_result(ctx, X_B, result_temp, *child_count)?;
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            ctx.b_unconditional(state_flat_base[resume_state]);
            ctx.patch_skip(skip_park, SkipKind::Cbnz(X_C));
            // Park for real: register as this group's own join waiter.
            // plans/M10.md item 0c2: by `TurnId` (a `u32` at +48), not by
            // the raw `X_FRAME` address it used to store — the one reader
            // (`codegen::emit_rt_child_poll`) derefs it, and does so
            // through `TurnsBase`/`TurnStride`, the single index→address
            // rule.
            let word = ctx.cur_word();
            ctx.load_imm_naive(X_A, 0);
            for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
                w.text = format!("turn-id[{i}] {} <{fn_key}>", reg_name(X_A));
            }
            ctx.relocs.push(Reloc::TurnIdImm {
                word,
                key: fn_key.to_string(),
            });
            ctx.push(
                encode::enc_str_w_imm(X_A, X_B, OFF_GROUP_JOIN_WAITER as u16),
                format!("str w{X_A}, [{}, #{OFF_GROUP_JOIN_WAITER}]", reg_name(X_B)),
                CostRule::Store,
                None,
                &[X_A, X_B],
            );
            ctx.load_imm(X_A, resume_state as i64);
            ctx.store_slot(X_A, ctx.frame.off(state_temp));
            emit_park_and_return(ctx);
            Ok(())
        }
        AwaitKind::Receipt { receipt_temp } => {
            // Decision 22: receipt word = meta absolute address.
            ctx.load_imm(X_A, resume_state as i64);
            ctx.store_slot(X_A, ctx.frame.off(state_temp));
            let stage_off = ctx.frame.reply_stage_off.ok_or_else(|| {
                CodegenError::internal(
                    "`await receipt` needs a reply staging slot for `IoCompletion` \
                     (`flow_reply_stage_size` disagrees with this site)",
                )
            })?;
            let result_size = mwir::size_of(&f.temp_types[result_temp.0], ctx.layout)
                .map_err(|e| CodegenError::unimplemented(&e))?;
            // Publish stage, then waiter, then observe RESOLVED
            // (mask–arm–recheck against a drain that already finished).
            //
            // plans/M10.md item 0c3: both are indices now. The waiter is
            // this turn's own `TurnId` (a `u32` at `SLOT_META_WAITER`,
            // whose upper half is unused padding); the reply stage is the
            // `(TurnId, byte offset within that turn area)` pair decision
            // 565 gives a frame-interior reference — `stage_off` is
            // `Frame::reply_stage_off`, assigned per fn in `build_frame`,
            // and the reader is `emit_queue_drain`, so an index alone
            // could not recover it. Both fields keep the offsets and the
            // publish order they always had, and `SLOT_META_BYTES` stays
            // 64, so nothing in the DMA pool moves.
            ctx.load_slot(X_D, ctx.frame.off(*receipt_temp)); // meta
            let word = ctx.cur_word();
            ctx.load_imm_naive(X_A, 0);
            for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
                w.text = format!("turn-id[{i}] {} <{fn_key}>", reg_name(X_A));
            }
            ctx.relocs.push(Reloc::TurnIdImm {
                word,
                key: fn_key.to_string(),
            });
            let interior = (stage_off + ctx.slot_bias) as u16;
            ctx.push(
                encode::enc_movz(X_B, interior, 0, false),
                format!("movz w{X_B}, #{interior:#x}"),
                CostRule::MovWide,
                Some(X_B),
                &[],
            );
            ctx.push(
                encode::enc_str_w_imm(X_A, X_D, crate::virtqueue::SLOT_META_REPLY_STAGE as u16),
                format!(
                    "str w{X_A}, [{}, #{}]",
                    reg_name(X_D),
                    crate::virtqueue::SLOT_META_REPLY_STAGE
                ),
                CostRule::Store,
                None,
                &[X_A, X_D],
            );
            ctx.push(
                encode::enc_str_w_imm(X_B, X_D, crate::virtqueue::SLOT_META_REPLY_STAGE as u16 + 4),
                format!(
                    "str w{X_B}, [{}, #{}]",
                    reg_name(X_D),
                    crate::virtqueue::SLOT_META_REPLY_STAGE + 4
                ),
                CostRule::Store,
                None,
                &[X_B, X_D],
            );
            ctx.push(
                encode::enc_str_w_imm(X_A, X_D, crate::virtqueue::SLOT_META_WAITER as u16),
                format!(
                    "str w{X_A}, [{}, #{}]",
                    reg_name(X_D),
                    crate::virtqueue::SLOT_META_WAITER
                ),
                CostRule::Store,
                None,
                &[X_A, X_D],
            );
            ctx.load_ptr(X_A, X_D, crate::virtqueue::SLOT_META_FLAGS as usize);
            ctx.load_imm(X_B, crate::virtqueue::SLOT_FLAG_RESOLVED as i64);
            ctx.and_reg(X_A, X_A, X_B);
            let need_park = ctx.emit_skip(SkipKind::Cbz(X_A));
            // Already resolved: copy completion stash → result_temp and
            // continue into the resume state without leaving the fn.
            // Stash sits at meta - META + (header+status pad) = meta + 64+16+8
            // = meta + 88; equivalently pool-relative completion_offset, but
            // we only have meta here: completion = meta + SLOT_META_BYTES +
            // REQ_HEADER_SIZE + 8.
            let stash_delta =
                crate::virtqueue::SLOT_META_BYTES + crate::virtqueue::REQ_HEADER_SIZE + 8;
            ctx.load_imm(X_A, stash_delta as i64);
            ctx.add_reg(X_A, X_D, X_A);
            let result_off = ctx.frame.off(result_temp);
            let mut w = 0usize;
            while w < result_size {
                ctx.load_ptr(X_B, X_A, w);
                ctx.store_slot(X_B, result_off + w);
                w += 8;
            }
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            ctx.b_unconditional(state_flat_base[resume_state]);
            ctx.patch_skip(need_park, SkipKind::Cbz(X_A));
            // Park until drain sets resume_ready.
            emit_park_and_return(ctx);
            Ok(())
        }
    }
}

/// Mark this turn suspended and return `TURN_STATUS_SUSPENDED` to the
/// scheduler — the real park, emitted identically by every `AwaitKind` arm
/// that genuinely leaves the fn.
fn emit_park_and_return(ctx: &mut FnCtx) {
    ctx.load_imm(X_A, 1);
    ctx.push(
        encode::enc_str_x_imm(X_A, X_FRAME, OFF_TURN_SUSPENDED as u16),
        format!(
            "str {}, [{}, #{OFF_TURN_SUSPENDED}]",
            reg_name(X_A),
            reg_name(X_FRAME)
        ),
        CostRule::Store,
        None,
        &[X_A, X_FRAME],
    );
    ctx.load_imm(0, TURN_STATUS_SUSPENDED as i64);
    ctx.load_slot(X_LR, ctx.frame.lr_off);
    ctx.push(
        encode::enc_ret(X_LR),
        "ret".to_string(),
        CostRule::Branch,
        None,
        &[X_LR],
    );
}

/// plans/M7.md item Z1: the `Ok` half of an aggregate reply's own resume
/// composition — copy the callee-written staging slot into `result_temp`'s
/// payload area (past the 8-byte tag), then zero whatever payload bytes
/// the composed `Result`'s *error* arm makes wider than the declared reply
/// itself, so the whole temp is deterministic no matter which arm is live.
/// The tag is written by the caller (both call sites want `Ok`, but they
/// reach it differently).
///
/// The copy is bounds-checked against the destination rather than trusted.
/// At Z1 it cannot overflow: the declared reply is a non-`Result` `T`, the
/// composed temp is `Result[T, CallError[never]]`, so the payload area is
/// `max(size(T), size(CallError[never]))` — never narrower than `T`. That
/// stops being true the moment item Z2 stages a declared `Result[T, E]`,
/// whose staged size is `8 + max(size(T), size(E))` against a payload area
/// of only `max(size(T), 8 + size(E))` — for a wide `T` and a narrow `E`
/// (say `T` = 24 bytes, `E` = 8) the staged value is genuinely *wider*
/// than the destination, and an unchecked copy here would scribble past
/// the temp onto the next frame slot. Z2 must recompose rather than copy;
/// this check is what makes that a loud build failure instead of silent
/// memory corruption, so it stays even though today nothing can trip it.
fn emit_copy_staged_reply(
    ctx: &mut FnCtx,
    stage_off: usize,
    staged_size: usize,
    result_off: usize,
    result_size: usize,
) -> Result<(), CodegenError> {
    if staged_size + 8 > result_size {
        return Err(CodegenError::internal(format!(
            "a staged reply of {staged_size} byte(s) does not fit the composed result's own \
             {}-byte payload area (plans/M7.md item Z1: the staged declared reply must be \
             recomposed, not copied, when the two shapes differ)",
            result_size.saturating_sub(8)
        )));
    }
    let mut w = 0;
    while w < staged_size {
        ctx.load_slot(X_A, stage_off + w);
        ctx.store_slot(X_A, result_off + 8 + w);
        w += 8;
    }
    // The payload area is `result_size - 8` bytes wide (the tag is the
    // first word), so the last writable word starts at `w == result_size
    // - 16` — hence `+ 16`, not `+ 8`: an off-by-one here would scribble
    // one word past the temp, onto whatever frame slot follows it.
    while w + 16 <= result_size {
        ctx.store_slot(X_ZR, result_off + 8 + w);
        w += 8;
    }
    Ok(())
}

/// plans/M7.md item Z2: the composition for a declared reply that is
/// itself a `Result[T, E]` — the one shape M6-H1 got *wrong* (both arms
/// arrived as `.Ok`, an `Err` observed as a success carrying a guest
/// address).
///
/// 02-language.md §9.4 maps `declared Result[T, E]` to
/// `Result[T, CallError[E]]`, and that is a **re-tagging, not a copy**:
///
/// ```text
///   staged Ok(v)   ->  composed Ok(v)
///   staged Err(e)  ->  composed Err(CallError.Op(e))
/// ```
///
/// The two values genuinely have different shapes — the staged declared
/// value is `8 + max(size(T), size(E))` bytes and the composed temp's own
/// payload area is only `max(size(T), 8 + max(size(E), 8))`, so for a wide
/// `T` and a narrow `E` the staged value is *wider than its destination*
/// (`T` = 24, `E` = 16: 32 staged bytes into a 24-byte payload area, which
/// `golden/boot-actor-reply-result`'s own `Triple` method is exactly).
/// Routing this through `emit_copy_staged_reply` would therefore be both
/// wrong (the staged tag word would land where the payload belongs) and,
/// for that shape, a buffer overrun — which is what that fn's own bounds
/// check exists to turn into a loud build failure. Nothing here copies the
/// staged value whole; every arm is recomposed field-wise.
///
/// **Every offset comes from the offset authority**, never from a hand-
/// assumed `+8`: `enum_payload_offset` places the staged `Result`'s own
/// payload slot, the composed `Result`'s payload slot, and `Op`'s payload
/// slot inside the `CallError[E]` that occupies it; `mwir::size_of` sizes
/// `T` and `E`.
///
/// The emitted shape is the same one `emit_await_resume`'s own
/// group-cancelled arm uses, and for the same reason: an `Ok` payload of several words and an `Err`
/// payload of a tag plus `E` cannot share one `csel`, so the `Err` answer
/// is composed *unconditionally* and the `Ok` overwrite is skipped when
/// the staged tag says `Err` — one forward branch, and both outcomes leave
/// every word of the composed temp deterministic rather than half-written.
/// Composed inside the cancelled path's own skip, so cancellation still
/// wins over whatever the callee staged (02 §9.5).
///
/// Clobbers `X_A` (the copy shuttle) and `X_B` (the staged tag). `X_C`,
/// the group-cancelled flag, is already consumed by the branch that guards
/// this call, and `emit_checkpoint_cancellation_test` recomputes both flags
/// for itself afterwards.
fn emit_recompose_staged_result(
    ctx: &mut FnCtx,
    stage_off: usize,
    declared: &Type,
    composed_ty: &Type,
    result_off: usize,
    result_size: usize,
) -> Result<(), CodegenError> {
    let Type::Result(ok_ty, err_ty) = strip_wrappers(declared) else {
        return Err(CodegenError::internal(format!(
            "the staged declared reply is not a `Result`: {declared:?}"
        )));
    };
    let Type::Result(_, composed_err_ty) = strip_wrappers(composed_ty) else {
        return Err(CodegenError::internal(format!(
            "an actor await's composed result is not a `Result`: {composed_ty:?}"
        )));
    };
    let staged_payload_off = stage_off + enum_payload_offset(declared, 0, ctx.layout)?;
    let ok_payload_off = result_off + enum_payload_offset(composed_ty, 0, ctx.layout)?;
    let op_payload_off = ok_payload_off + enum_payload_offset(composed_err_ty, 0, ctx.layout)?;
    let ok_size = mwir::size_of(ok_ty, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
    let err_size =
        mwir::size_of(err_ty, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
    let result_end = result_off + result_size;
    // Both hold by construction — `size_of(Result[T, CallError[E]])` is
    // `8 + max(size(T), 8 + max(size(E), 8))`, so the payload area is never
    // narrower than `T` nor than `8 + size(E)`. Checked anyway, in the same
    // spirit as `emit_copy_staged_reply`'s own bound: a layout change that
    // broke either one would otherwise scribble past this temp onto the
    // next frame slot, silently.
    if ok_payload_off + ok_size > result_end || op_payload_off + err_size > result_end {
        return Err(CodegenError::internal(format!(
            "a recomposed `Result` reply does not fit its composed temp: ok {ok_size} byte(s) at \
             +{}, `CallError.Op` {err_size} byte(s) at +{}, temp {result_size} byte(s) \
             (plans/M7.md item Z2)",
            ok_payload_off - result_off,
            op_payload_off - result_off
        )));
    }
    // --- staged `Err(e)` -> composed `Err(CallError.Op(e))`, unconditional.
    // `Op` is `CallError[E]`'s own variant 0 (02 §9.4's declared order,
    // the same numbering `CALL_ERROR_TAG_CANCELLED = 1` belongs to).
    ctx.store_slot(X_ZR, ok_payload_off);
    let mut w = 0;
    while w < err_size {
        ctx.load_slot(X_A, staged_payload_off + w);
        ctx.store_slot(X_A, op_payload_off + w);
        w += 8;
    }
    while op_payload_off + w + 8 <= result_end {
        ctx.store_slot(X_ZR, op_payload_off + w);
        w += 8;
    }
    ctx.load_imm(X_A, 1); // tag = Err (`value::RESULT_ERR`)
    ctx.store_slot(X_A, result_off);
    // --- staged `Ok(v)` -> composed `Ok(v)`, overwriting the above.
    ctx.load_slot(X_B, stage_off); // the staged declared `Result`'s own tag
    let skip_ok = ctx.emit_skip(SkipKind::Cbnz(X_B));
    let mut w = 0;
    while w < ok_size {
        ctx.load_slot(X_A, staged_payload_off + w);
        ctx.store_slot(X_A, ok_payload_off + w);
        w += 8;
    }
    while ok_payload_off + w + 8 <= result_end {
        ctx.store_slot(X_ZR, ok_payload_off + w);
        w += 8;
    }
    ctx.store_slot(X_ZR, result_off); // tag = Ok (`value::RESULT_OK`)
    ctx.patch_skip(skip_ok, SkipKind::Cbnz(X_B));
    Ok(())
}

/// plans/M7.md items Z1/Z2: the one place a *staged* declared reply
/// becomes the caller's composed `Result[T, CallError[E]]`. Two shapes,
/// one predicate (`decompose_call_error`'s own output):
///
/// - a non-`Result` declared reply `T` (item Z1) is a straight copy into
///   the composed `Ok` payload — the delivered value and the composed
///   payload have the same shape;
/// - a declared `Result[T, E]` (item Z2) is a re-tagging, and is
///   recomposed field-wise (`emit_recompose_staged_result`).
///
/// Both `emit_await_resume` call sites — the no-group one and the one
/// inside the group-cancelled skip — go through here, so the two can never
/// disagree about which shape a given await site has.
fn emit_compose_staged_reply(
    ctx: &mut FnCtx,
    stage_off: usize,
    declared: &Type,
    composed_ty: &Type,
    result_off: usize,
    result_size: usize,
) -> Result<(), CodegenError> {
    if matches!(strip_wrappers(declared), Type::Result(_, _)) {
        return emit_recompose_staged_result(
            ctx,
            stage_off,
            declared,
            composed_ty,
            result_off,
            result_size,
        );
    }
    let staged_size =
        mwir::size_of(declared, ctx.layout).map_err(|e| CodegenError::unimplemented(&e))?;
    emit_copy_staged_reply(ctx, stage_off, staged_size, result_off, result_size)?;
    ctx.store_slot(X_ZR, result_off); // tag = Ok
    Ok(())
}

/// plans/M13.md item H: build
/// `Err(CallError.NotAdmitted(Admission.Full, (take_args...)))` into the
/// composed result slot from temps the caller still owns (enqueue did not
/// commit). Layout: `Result.tag=Err` at +0, `CallError.tag=NotAdmitted` at
/// +8, `Admission.Full` at +16, take-arg words at +24… (each scalar one
/// slot; aggregates on this arm fail closed — known risk in plans/M13.md).
///
/// Clobbers `X_A`/`X_B`.
fn emit_not_admitted_local(
    ctx: &mut FnCtx,
    result_off: usize,
    result_size: usize,
    take_arg_temps: &[Temp],
) -> Result<(), CodegenError> {
    for t in take_arg_temps {
        let sz = ctx.frame.size_of_temp(*t);
        if sz != 8 {
            return Err(CodegenError::unimplemented(
                "NotAdmitted take-arg handback for a non-scalar argument (plans/M13.md item H; \
                 spill aggregates on the fail branch is not implemented)",
            ));
        }
    }
    // Zero-fill the whole temp so unused payload words stay deterministic.
    let mut w = 0usize;
    while w < result_size {
        ctx.store_slot(X_ZR, result_off + w);
        w += 8;
    }
    ctx.load_imm(X_A, 1); // Result.Err
    ctx.store_slot(X_A, result_off);
    ctx.load_imm(X_B, CALL_ERROR_TAG_NOT_ADMITTED as i64);
    ctx.store_slot(X_B, result_off + 8);
    ctx.load_imm(X_A, ADMISSION_FULL as i64);
    ctx.store_slot(X_A, result_off + 16);
    let mut off = 24usize;
    for t in take_arg_temps {
        if off + 8 > result_size {
            return Err(CodegenError::internal(
                "NotAdmitted take-arg tuple does not fit the composed CallError temp \
                 (size_of/compose_call_error disagree with this site)",
            ));
        }
        ctx.load_slot(X_A, ctx.frame.off(*t));
        ctx.store_slot(X_A, result_off + off);
        off += 8;
    }
    Ok(())
}

/// plans/M10.md item J: compose `result_temp` from the turn record's
/// `(OFF_TURN_REPLY_TAG, OFF_TURN_REPLY)` pair. `tag == 0` → `Ok(reply)`;
/// nonzero → `Err(CallError)` whose variant index is the tag and whose
/// payload (when any) is the reply word (`Admission` for `NotAdmitted`
/// with an empty take-args tuple — local enqueue-fail with take args uses
/// [`emit_not_admitted_local`] instead). Same shape the group arena
/// already uses for `(tag, payload)`.
///
/// `X_A` enters holding the reply word; `X_B` holding the tag. Clobbers
/// `X_A`/`X_B`.
fn emit_compose_from_reply_tag(ctx: &mut FnCtx, result_off: usize, result_size: usize) {
    // `cbnz tag` skips the Ok arm when the tag is a CallError variant.
    let skip_err = ctx.emit_skip(SkipKind::Cbnz(X_B));
    ctx.store_slot(X_A, result_off + 8);
    let mut w = 16;
    while w < result_size {
        ctx.store_slot(X_ZR, result_off + w);
        w += 8;
    }
    ctx.store_slot(X_ZR, result_off);
    let skip_done = ctx.emit_skip(SkipKind::Cond(Cond::Al));
    ctx.patch_skip(skip_err, SkipKind::Cbnz(X_B));
    // Err: +8 = CallError.tag, +16 = payload (Admission / zero).
    ctx.store_slot(X_B, result_off + 8);
    if result_size >= 24 {
        ctx.store_slot(X_A, result_off + 16);
    }
    w = 24;
    while w < result_size {
        ctx.store_slot(X_ZR, result_off + w);
        w += 8;
    }
    ctx.load_imm(X_A, 1); // Result.Err
    ctx.store_slot(X_A, result_off);
    ctx.patch_skip(skip_done, SkipKind::Cond(Cond::Al));
}

/// The resume half (module doc's step 3) — the dispatch chain's landing
/// site for `resume_state`: for `ActorCall`, compose from the turn
/// record's `(reply_tag, reply)` pair; for `GroupJoin`
/// (parked, now woken — either a real child completion or the join
/// waiter's own group getting cancelled and forcibly resumed, item F #3's
/// "make cancelled suspended turns ready-to-resume"), recompose from the
/// group arena directly (the same shared helper the immediate path uses —
/// results may have kept changing after the wake, but every write is
/// idempotent by the time this runs). Either way: decision 6's checkpoint
/// ("await resume points are checkpoints by construction"), this item's
/// own cancellation test (module doc on `emit_checkpoint_cancellation_test`
/// — an `ActorCall` resume whose own ambient group is now cancelled never
/// gets to use its stale composed `Ok(reply)`; it terminates instead, "the
/// cancelled frame never resumes"), then jump on to the resumed state.
#[allow(clippy::too_many_arguments)]
fn emit_await_resume(
    resume_state: usize,
    result_temp: Temp,
    what: &AwaitKind,
    f: &MwirFn,
    ctx: &mut FnCtx,
    gctx: &GroupCtx,
    // plans/M10.md item 0c2: the `Reloc::TurnIdImm` key
    // `emit_group_cancelled_flags` below needs — this fn's own turn is
    // what a group's `owner_turn` is now compared against.
    fn_key: &str,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match what {
        AwaitKind::ActorCall { .. } => {
            // `result_temp`'s own type is always the composed
            // `Result[T, CallError[E]]` (02 §9.4's composition table,
            // `sema::bodies::compose_call_error`) — never the bare
            // declared reply.
            let composed_ty = &f.temp_types[result_temp.0];
            // 03-hardware.md §5's handoff calling convention (plans/M8.md
            // item E, decision 32): the one `Actor[T]` await whose result
            // is *not* 02 §9.4's composed `Result` — its result is
            // `Receipt[P]` by name, the caller-owned endpoint the driver's
            // `return queue.publish(...)` transitioned. One scalar word,
            // delivered in the caller's own turn record exactly like every
            // other scalar reply; the failure vocabulary that matters to it
            // is the receipt's own state machine, reached by `await`ing it.
            if is_handoff_receipt_reply(composed_ty) {
                if gctx.arena_capacity != 0 {
                    // 02 §9.5: "Cancellation becomes observable at
                    // `await`". A composed reply carries `Cancelled` in its
                    // error slot; a bare `Receipt[P]` has no error slot, and
                    // inventing one would mean handing back a forged receipt
                    // word. Fail closed by name rather than resolve a lie —
                    // the honest repair is 03 §9's recovery turn, which is
                    // plans/M8.md item F.
                    return Err(CodegenError::unimplemented(
                        "a handoff `await` (03-hardware.md §5) inside an image that declares a \
                         `with group` — a cancelled handoff receipt has no `CallError` channel \
                         to resolve into and must go to 03-hardware.md §9's recovery turn \
                         (plans/M8.md item F)",
                    ));
                }
                let result_off = ctx.frame.off(result_temp);
                ctx.push(
                    encode::enc_ldr_x_imm(X_A, X_FRAME, OFF_TURN_REPLY as u16),
                    format!(
                        "ldr {}, [{}, #{OFF_TURN_REPLY}]",
                        reg_name(X_A),
                        reg_name(X_FRAME)
                    ),
                    CostRule::Load,
                    Some(X_A),
                    &[X_FRAME],
                );
                ctx.store_slot(X_A, result_off);
                ctx.checkpoint();
                emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
                ctx.b_unconditional(state_flat_base[resume_state]);
                return Ok(());
            }
            if !matches!(composed_ty, Type::Result(_, _)) {
                return Err(CodegenError::internal(format!(
                    "Await's own result_temp is not a composed Result type: {composed_ty:?}"
                )));
            }
            let result_off = ctx.frame.off(result_temp);
            let result_size = ctx.frame.size_of_temp(result_temp);
            // plans/M7.md item Z1: where the declared reply actually is.
            // Scalar (`None`) — the turn record's own reply word, exactly
            // as M6 left it. Aggregate (`Some`) — this fn's own staging
            // slot, which the callee wrote through `x8` before it ever
            // completed, so the record's scalar reply word carries a
            // deliberate 0 for such a method and is not read here at all.
            let staged = match aggregate_reply_of_await(f, result_temp) {
                None => None,
                Some(declared) => {
                    let off = ctx.frame.reply_stage_off.ok_or_else(|| {
                        CodegenError::internal(
                            "an `await` resume with an aggregate declared reply but no reply \
                             staging slot (`flow_reply_stage_size` disagrees with this site)",
                        )
                    })?;
                    Some((off, declared))
                }
            };
            if let Some((stage_off, declared)) = staged {
                if gctx.arena_capacity == 0 {
                    emit_compose_staged_reply(
                        ctx,
                        stage_off,
                        &declared,
                        composed_ty,
                        result_off,
                        result_size,
                    )?;
                } else {
                    // Same rule the scalar path below applies (02 §9.5:
                    // "Cancellation becomes observable at `await`"), but
                    // an aggregate cannot ride a `csel`: the `Ok` payload
                    // is several words and the `Err` payload is one tag
                    // word, so the two compositions are written whole,
                    // one branch apart. Compose the *cancelled* answer
                    // unconditionally first, then skip the `Ok` overwrite
                    // when the flag is set — one forward branch, and both
                    // outcomes leave every payload word deterministic
                    // rather than half-overwritten.
                    //
                    // plans/M7.md item Z2: cancellation wins over whatever
                    // the callee staged, including a staged declared `Err`
                    // — the whole composition sits inside this skip, so a
                    // cancelled await resolves `Err(CallError.Cancelled)`
                    // and never `Err(CallError.Op(e))`.
                    //
                    // Both offsets below come from the offset authority
                    // rather than a hand-assumed `+8`/`+16`: the composed
                    // `Result`'s own payload slot holds the whole
                    // `CallError[E]` (whose tag is its first word), and
                    // `Op`'s payload slot follows that tag.
                    let Type::Result(_, composed_err_ty) = strip_wrappers(composed_ty) else {
                        return Err(CodegenError::internal(format!(
                            "an actor await's composed result is not a `Result`: {composed_ty:?}"
                        )));
                    };
                    let call_error_off =
                        result_off + enum_payload_offset(composed_ty, 0, ctx.layout)?;
                    let op_payload_off =
                        call_error_off + enum_payload_offset(composed_err_ty, 0, ctx.layout)?;
                    emit_group_cancelled_flags(ctx, fn_key, gctx);
                    ctx.load_imm(X_A, CALL_ERROR_TAG_CANCELLED as i64);
                    ctx.store_slot(X_A, call_error_off);
                    let mut w = op_payload_off;
                    while w < result_off + result_size {
                        ctx.store_slot(X_ZR, w);
                        w += 8;
                    }
                    ctx.load_imm(X_A, 1); // tag = Err (`value::RESULT_ERR`)
                    ctx.store_slot(X_A, result_off);
                    let skip_ok = ctx.emit_skip(SkipKind::Cbnz(X_C));
                    emit_compose_staged_reply(
                        ctx,
                        stage_off,
                        &declared,
                        composed_ty,
                        result_off,
                        result_size,
                    )?;
                    ctx.patch_skip(skip_ok, SkipKind::Cbnz(X_C));
                }
            } else {
                // plans/M10.md item J: compose from `(reply_tag, reply)`.
                // Ambient group cancel still wins over a delivered `Ok`
                // (02 §9.5 — the owner's only observation of cancel).
                if gctx.arena_capacity != 0 {
                    emit_group_cancelled_flags(ctx, fn_key, gctx);
                }
                ctx.push(
                    encode::enc_ldr_x_imm(X_A, X_FRAME, OFF_TURN_REPLY as u16),
                    format!(
                        "ldr {}, [{}, #{OFF_TURN_REPLY}]",
                        reg_name(X_A),
                        reg_name(X_FRAME)
                    ),
                    CostRule::Load,
                    Some(X_A),
                    &[X_FRAME],
                );
                ctx.push(
                    encode::enc_ldr_x_imm(X_B, X_FRAME, OFF_TURN_REPLY_TAG as u16),
                    format!(
                        "ldr {}, [{}, #{OFF_TURN_REPLY_TAG}]",
                        reg_name(X_B),
                        reg_name(X_FRAME)
                    ),
                    CostRule::Load,
                    Some(X_B),
                    &[X_FRAME],
                );
                if gctx.arena_capacity != 0 {
                    // If cancelled and the delivered tag is Ok, force
                    // Cancelled. A delivered Cancelled/NotAdmitted stands.
                    let skip_force = ctx.emit_skip(SkipKind::Cbz(X_C));
                    ctx.push_flags(
                        encode::enc_cmp_imm(X_B, 0, true),
                        format!("cmp {}, #0", reg_name(X_B)),
                        CostRule::Alu,
                        None,
                        &[X_B],
                        FlagEffect::Write,
                    );
                    let skip_keep = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
                    ctx.load_imm(X_B, CALL_ERROR_TAG_CANCELLED as i64);
                    ctx.load_imm(X_A, 0);
                    ctx.patch_skip(skip_keep, SkipKind::Cond(Cond::Ne));
                    ctx.patch_skip(skip_force, SkipKind::Cbz(X_C));
                }
                emit_compose_from_reply_tag(ctx, result_off, result_size);
            }
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            ctx.b_unconditional(state_flat_base[resume_state]);
            Ok(())
        }
        AwaitKind::GroupJoin {
            group_temp,
            child_count,
        } => {
            emit_group_addr_from_temp(ctx, *group_temp, X_B, X_A, gctx);
            emit_compose_group_join_result(ctx, X_B, result_temp, *child_count)?;
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            ctx.b_unconditional(state_flat_base[resume_state]);
            Ok(())
        }
        AwaitKind::Receipt { .. } => {
            let stage_off = ctx.frame.reply_stage_off.ok_or_else(|| {
                CodegenError::internal(
                    "`await receipt` resume needs the reply staging slot \
                     (`flow_reply_stage_size` disagrees with this site)",
                )
            })?;
            let result_off = ctx.frame.off(result_temp);
            let result_size = mwir::size_of(&f.temp_types[result_temp.0], ctx.layout)
                .map_err(|e| CodegenError::unimplemented(&e))?;
            let mut w = 0usize;
            while w < result_size {
                ctx.load_slot(X_A, stage_off + w);
                ctx.store_slot(X_A, result_off + w);
                w += 8;
            }
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            ctx.b_unconditional(state_flat_base[resume_state]);
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_transition(
    t: &Transition,
    flat_idx: usize,
    f: &MwirFn,
    ctx: &mut FnCtx,
    method_index: &ActorMethodIndex,
    gctx: &GroupCtx,
    fn_key: &str,
    state_temp: Temp,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match t {
        Transition::Return(value) => emit_one(&Inst::Return { value: *value }, f, ctx),
        Transition::Jump(target_state) => {
            let target_flat = state_flat_base[*target_state];
            // decision 6: every *async* loop back-edge gets a checkpoint —
            // a `Transition::Jump` is only ever backward for a loop's own
            // state-cycle repeat (`flowwir_lower.rs`'s own
            // `lower_while_split`); the position test `target_flat <=
            // flat_idx` is the same classification sync mwir once used
            // (plans/M11.md decision 740 retires the sync half — trip
            // counters only). plans/M6.md item F: this back-edge is
            // also where a spinning turn's own cancellation is observed
            // (decision 7's flip witness — a deterministic iteration
            // count, never mid-instruction).
            if target_flat <= flat_idx {
                ctx.checkpoint();
                emit_checkpoint_cancellation_test(ctx, gctx, fn_key);
            }
            ctx.b_unconditional(target_flat);
            Ok(())
        }
        Transition::Branch {
            cond_temp,
            then_state,
            else_state,
        } => {
            ctx.load_slot(X_A, ctx.frame.off(*cond_temp));
            ctx.cbz(X_A, state_flat_base[*else_state]);
            ctx.b_unconditional(state_flat_base[*then_state]);
            Ok(())
        }
        Transition::Abort { msg } => {
            ctx.abort_fixed(msg);
            Ok(())
        }
        Transition::Await {
            what,
            resume_state,
            result_temp,
        } => emit_await_suspend(
            what,
            *resume_state,
            *result_temp,
            f,
            ctx,
            method_index,
            gctx,
            fn_key,
            state_temp,
            state_flat_base,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn emit_flat_entry(
    entry: &FlatEntry,
    flat_idx: usize,
    f: &MwirFn,
    ctx: &mut FnCtx,
    method_index: &ActorMethodIndex,
    gctx: &GroupCtx,
    fn_key: &str,
    state_temp: Temp,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match entry {
        FlatEntry::Op(op) => emit_flow_op(op, f, ctx, method_index, gctx, fn_key),
        FlatEntry::Trans(t) => emit_transition(
            t,
            flat_idx,
            f,
            ctx,
            method_index,
            gctx,
            fn_key,
            state_temp,
            state_flat_base,
        ),
        FlatEntry::AwaitResume {
            resume_state,
            result_temp,
            what,
        } => emit_await_resume(
            *resume_state,
            *result_temp,
            what,
            f,
            ctx,
            gctx,
            fn_key,
            state_flat_base,
        ),
    }
}

/// The whole driver for one async fn/method: the custom park-and-resume
/// entry (`emit_async_entry`) + flattened state bodies/transitions/
/// await-resume stubs + the shared async completion epilogue, two-pass
/// sized exactly like `emit_fn`'s own sync-fn driver (module doc above;
/// never a forked copy of the per-instruction emission itself). Every
/// frame slot is addressed through `X_FRAME` (the fn's own persistent
/// turn area) — an SP frame would die at the suspension's own `ret`.
fn emit_flowwir_fn(
    fn_key: &str,
    f: &FlowWirFn,
    layout: &LayoutCtx,
    rodata: &mut RodataPool,
    method_index: &ActorMethodIndex,
    gctx: &GroupCtx,
) -> Result<CodegenFn, CodegenError> {
    if is_aggregate(&f.ret) && f.receiver.is_none() {
        // plans/M7.md item Z1 (decision 9d) narrowed this from "any async
        // fn" to "a *free* async fn". An aggregate-returning async
        // **method** now works: its caller parks with a staging slot whose
        // address the callee's own dispatch arm hands it in `x8`
        // (`OFF_TURN_REPLY_SLOT`). A free async fn has no such caller —
        // it is a `@test(runtime)` root, driven by the entry driver, or a
        // `g.start` child, harvested into the group arena's own child
        // result slots, and both of those destinations are exactly one
        // word wide with no staging slot to offer. Fail closed, never a
        // silent truncation; widening THAT is separate, real work.
        return Err(CodegenError::unimplemented(
            "a free (non-method) async fn returning an aggregate — a `@test(runtime)` root's own \
             driver has no reply staging slot to hand it, and a `g.start` child's result slot in \
             the group arena is one word wide (plans/M7.md item Z1 widened the actor-*method* \
             case; this one is not implemented)",
        ));
    }
    let (frame, state_temp) = build_frame_flow(f, layout)?;
    let (state_flat_base, resume_target, flat) = flatten(f);
    let total = flat.len();
    // plans/M20.md item B / decision 1607: same widening as `emit_fn` —
    // `runtime` and `driver` async bodies are instrumented too, with the
    // same single exclusion for the counter helper.
    let block_ids = if block_count_instruments(fn_key) {
        assign_flat_block_ids(&flat, &state_flat_base)?
    } else {
        vec![None; flat.len()]
    };

    let synthetic = MwirFn {
        receiver: f.receiver,
        params: f.params.clone(),
        ret: f.ret.clone(),
        temp_types: {
            let mut t = f.frame.temp_types.clone();
            t.push(Type::U64);
            t.push(Type::U64);
            t.push(Type::U64);
            t
        },
        body: vec![Inst::AssertFail { message: None }; total],
    };

    // The entry probe needs real-length dummy targets (unlike a sync
    // prologue, the async entry emits branches — the fresh path's jump to
    // state 0 and the resume chain's arms — whose widths are fixed but
    // whose emission indexes `word_offsets`). plans/M6.md item F: one
    // extra sentinel past the epilogue (`total + 1`) for the shared
    // cancellation tail (`emit_async_cancelled_tail`'s own doc comment).
    let dummy_targets = vec![0usize; total + 2];
    let mut probe_pro = FnCtx {
        frame: &frame,
        layout,
        rodata,
        word_offsets: &dummy_targets,
        words: Vec::new(),
        relocs: Vec::new(),
        slot_base: X_FRAME,
        slot_bias: TURN_RECORD_SIZE as usize,
        cold_seq: 0,
    };
    emit_async_entry(
        &synthetic,
        fn_key,
        &mut probe_pro,
        state_temp,
        &resume_target,
    )?;
    let prologue_len = probe_pro.words.len();
    let mut counts = Vec::with_capacity(total);
    for (i, entry) in flat.iter().enumerate() {
        let mut probe = FnCtx {
            frame: &frame,
            layout,
            rodata,
            word_offsets: &dummy_targets,
            words: Vec::new(),
            relocs: Vec::new(),
            slot_base: X_FRAME,
            slot_bias: TURN_RECORD_SIZE as usize,
            cold_seq: 0,
        };
        if let Some(id) = block_ids[i] {
            probe.emit_block_hit(id);
        }
        emit_flat_entry(
            entry,
            i,
            &synthetic,
            &mut probe,
            method_index,
            gctx,
            fn_key,
            state_temp,
            &state_flat_base,
        )?;
        counts.push(probe.words.len());
    }
    let mut word_offsets = vec![0usize; total + 2];
    let mut acc = prologue_len;
    for (i, c) in counts.iter().enumerate() {
        word_offsets[i] = acc;
        acc += c;
    }
    word_offsets[total] = acc;

    // plans/M6.md item F: the shared cancellation tail (`total + 1`'s own
    // sentinel) exists only when this *whole build* has a group arena at
    // all (`GroupCtx::arena_capacity > 0`) — `emit_checkpoint_cancellation_test`
    // is the only possible producer of a jump to it, and that fn is
    // itself a no-op whenever `arena_capacity == 0` (its own doc comment).
    // Skipping the tail's own bytes entirely in that case is what keeps
    // every pre-item-F async golden's own ASM/frame-size byte-identical —
    // not merely unreached, genuinely absent.
    let mut probe_epi = FnCtx {
        frame: &frame,
        layout,
        rodata,
        word_offsets: &dummy_targets,
        words: Vec::new(),
        relocs: Vec::new(),
        slot_base: X_FRAME,
        slot_bias: TURN_RECORD_SIZE as usize,
        cold_seq: 0,
    };
    emit_async_epilogue(&synthetic, &mut probe_epi)?;
    word_offsets[total + 1] = acc + probe_epi.words.len();

    let mut ctx = FnCtx {
        frame: &frame,
        layout,
        rodata,
        word_offsets: &word_offsets,
        words: Vec::new(),
        relocs: Vec::new(),
        slot_base: X_FRAME,
        slot_bias: TURN_RECORD_SIZE as usize,
        cold_seq: 0,
    };
    emit_async_entry(&synthetic, fn_key, &mut ctx, state_temp, &resume_target)?;
    debug_assert_eq!(ctx.words.len(), prologue_len);
    for (i, entry) in flat.iter().enumerate() {
        if let Some(id) = block_ids[i] {
            ctx.emit_block_hit(id);
        }
        emit_flat_entry(
            entry,
            i,
            &synthetic,
            &mut ctx,
            method_index,
            gctx,
            fn_key,
            state_temp,
            &state_flat_base,
        )?;
    }
    debug_assert_eq!(ctx.words.len(), word_offsets[total]);
    emit_async_epilogue(&synthetic, &mut ctx)?;
    debug_assert_eq!(ctx.words.len(), word_offsets[total + 1]);
    if gctx.arena_capacity > 0 {
        emit_async_cancelled_tail(&mut ctx);
    }

    Ok(CodegenFn {
        frame_size: frame.size,
        code: ctx.words,
        relocs: ctx.relocs,
    })
}

/// Integrity Phase 2 Item M: leaders on a flattened FlowWir stream.
fn flat_block_leaders(flat: &[FlatEntry], state_flat_base: &[usize]) -> Vec<bool> {
    let n = flat.len();
    let mut leaders = vec![false; n];
    if n == 0 {
        return leaders;
    }
    leaders[0] = true;
    for &b in state_flat_base {
        if b < n {
            leaders[b] = true;
        }
    }
    for (i, entry) in flat.iter().enumerate() {
        match entry {
            FlatEntry::Op(FlowInst::Mwir(Inst::Jump { target })) => {
                if *target < n {
                    leaders[*target] = true;
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::Op(FlowInst::Mwir(Inst::JumpIfFalse { target, .. })) => {
                if *target < n {
                    leaders[*target] = true;
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::Op(FlowInst::Mwir(Inst::Return { .. })) => {
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::Trans(Transition::Jump(state)) => {
                if let Some(&t) = state_flat_base.get(*state) {
                    if t < n {
                        leaders[t] = true;
                    }
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::Trans(Transition::Branch {
                then_state,
                else_state,
                ..
            }) => {
                if let Some(&t) = state_flat_base.get(*then_state) {
                    if t < n {
                        leaders[t] = true;
                    }
                }
                if let Some(&e) = state_flat_base.get(*else_state) {
                    if e < n {
                        leaders[e] = true;
                    }
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::Trans(
                Transition::Return(_) | Transition::Await { .. } | Transition::Abort { .. },
            ) => {
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            FlatEntry::AwaitResume { resume_state, .. } => {
                leaders[i] = true;
                if let Some(&t) = state_flat_base.get(*resume_state) {
                    if t < n {
                        leaders[t] = true;
                    }
                }
                if i + 1 < n {
                    leaders[i + 1] = true;
                }
            }
            _ => {}
        }
    }
    leaders
}

fn assign_flat_block_ids(
    flat: &[FlatEntry],
    state_flat_base: &[usize],
) -> Result<Vec<Option<u32>>, CodegenError> {
    let mut ids = vec![None; flat.len()];
    if !block_count() {
        return Ok(ids);
    }
    for (i, is_leader) in flat_block_leaders(flat, state_flat_base)
        .into_iter()
        .enumerate()
    {
        if is_leader {
            ids[i] = Some(alloc_block_id()?);
        }
    }
    Ok(ids)
}

/// Every async fn's own persistent frame byte count (its `Frame::size` —
/// the statically reserved slots its activation lives in, past the
/// 64-byte turn record), keyed exactly like `FlowWirProgram::fns` — the
/// one fact `layout::compute_runtime_tables` needs from this module to
/// size each turn area, computed by the identical `build_frame_flow` the
/// real emission uses so the two can never disagree.
pub fn async_frame_sizes(
    flow: &FlowWirProgram,
    layout: &LayoutCtx,
) -> Result<BTreeMap<String, u64>, CodegenError> {
    let mut out = BTreeMap::new();
    for (key, f) in &flow.fns {
        let (frame, _) = build_frame_flow(f, layout)?;
        out.insert(key.clone(), frame.size as u64);
    }
    Ok(out)
}

// M11 item F: `emit_rt_run_one` / `emit_rt_child_poll` deleted —
// `__wrela_rt_run_one` / `__wrela_child_poll` in `stdlib/core/runtime.wr`
// (decisions 790–794). Spec structs kept only if still referenced…
// (RtRunOneSpec / RtChildPollSpec removed with the emitters.)

// M11 G (decisions 800–805): emit_rt_xsend / emit_rt_xreply / emit_rt_drain
// deleted — generic `__wrela_rt_xsend` / `__wrela_rt_xreply` /
// `__wrela_rt_drain` in stdlib/core/runtime.wr over ring facts + trampolines.
// M11 J (decisions 830–835): emit_rt_enqueue / emit_rt_select_and_run
// deleted — generic `__wrela_rt_enqueue` / `__wrela_rt_select` in
// stdlib/core/runtime.wr over mailbox facts + `__method_*` dispatch stubs.
// (BRK_REPLY_SLOT_NO_WAKER went with emit_rt_select_and_run.)

/// M11 H / decision 811: floor-cat1 SP install for a secondary core (5 words).
/// Prepended at inject onto `__wrela_secondary_entry_<core>` before the key is
/// republished as `rt_secondary_core_entry <core>` (decision 636 extraction).
/// `n_cores` is the sealed report N (plans/M15.md item D high-DRAM stacks).
pub fn emit_secondary_sp_install(core: usize, n_cores: usize) -> Vec<EmittedWord> {
    let mut words: Vec<EmittedWord> = Vec::new();
    let push = |words: &mut Vec<EmittedWord>,
                w: u32,
                text: String,
                rule: CostRule,
                dst: Option<u8>,
                srcs: &[u8]| {
        words.push(EmittedWord::new(w, text, rule, dst, srcs));
    };
    let load_imm = |words: &mut Vec<EmittedWord>, reg: u8, value: u64, label: &str| {
        let h0 = (value & 0xFFFF) as u16;
        let h1 = ((value >> 16) & 0xFFFF) as u16;
        let h2 = ((value >> 32) & 0xFFFF) as u16;
        let h3 = ((value >> 48) & 0xFFFF) as u16;
        push(
            words,
            encode::enc_movz(reg, h0, 0, true),
            format!("movz {}, #{:#x}  ; {label}", reg_name(reg), value),
            CostRule::MovWide,
            Some(reg),
            &[],
        );
        push(
            words,
            encode::enc_movk(reg, h1, 16, true),
            format!("movk {}, #{:#x}, lsl #16", reg_name(reg), h1),
            CostRule::MovWide,
            Some(reg),
            &[],
        );
        push(
            words,
            encode::enc_movk(reg, h2, 32, true),
            format!("movk {}, #{:#x}, lsl #32", reg_name(reg), h2),
            CostRule::MovWide,
            Some(reg),
            &[],
        );
        push(
            words,
            encode::enc_movk(reg, h3, 48, true),
            format!("movk {}, #{:#x}, lsl #48", reg_name(reg), h3),
            CostRule::MovWide,
            Some(reg),
            &[],
        );
    };
    let n = n_cores.max(1);
    let sp_top =
        wrela_machine::layout::core_stack_base_n(core, n) + wrela_machine::layout::CORE_STACK_SIZE;
    load_imm(&mut words, 9, sp_top, "sp_top");
    push(
        &mut words,
        encode::enc_add_imm(31, 9, 0, true),
        "mov sp, x9".to_string(),
        CostRule::Alu,
        Some(31),
        &[9],
    );
    words
}

// --- stub emitters: shared word-list helpers ------------------------
//
// The `emit_*` stub builders below assemble a plain `Vec<EmittedWord>`
// rather than driving an `FnCtx` (they are hand-shaped fragments, not
// lowered from mwir), so they need a free `push`/`load_imm` pair.
// One copy, at module scope, instead of one nested copy per builder.

fn push(
    words: &mut Vec<EmittedWord>,
    w: u32,
    text: String,
    rule: CostRule,
    dst: Option<u8>,
    srcs: &[u8],
) {
    words.push(EmittedWord::new(w, text, rule, dst, srcs));
}

fn load_imm(words: &mut Vec<EmittedWord>, reg: u8, value: u64, label: &str) {
    let h0 = (value & 0xFFFF) as u16;
    let h1 = ((value >> 16) & 0xFFFF) as u16;
    let h2 = ((value >> 32) & 0xFFFF) as u16;
    let h3 = ((value >> 48) & 0xFFFF) as u16;
    push(
        words,
        encode::enc_movz(reg, h0, 0, true),
        format!("movz {}, #{:#x}  ; {label}", reg_name(reg), value),
        CostRule::MovWide,
        Some(reg),
        &[],
    );
    push(
        words,
        encode::enc_movk(reg, h1, 16, true),
        format!("movk {}, #{:#x}, lsl #16", reg_name(reg), h1),
        CostRule::MovWide,
        Some(reg),
        &[],
    );
    push(
        words,
        encode::enc_movk(reg, h2, 32, true),
        format!("movk {}, #{:#x}, lsl #32", reg_name(reg), h2),
        CostRule::MovWide,
        Some(reg),
        &[],
    );
    push(
        words,
        encode::enc_movk(reg, h3, 48, true),
        format!("movk {}, #{:#x}, lsl #48", reg_name(reg), h3),
        CostRule::MovWide,
        Some(reg),
        &[],
    );
}

/// M11 H / decision 812: one boot `init` call stub (specialized A64 with
/// Relocs). Zero-fill lives in `__wrela_rt_boot_init`; inject overwrites
/// `__boot_call_<i>` with this body. Saves `x30`; no mid-tick checkpoint.
pub fn emit_boot_init_call(slot: &BootInitSlotSpec) -> CodegenFn {
    fn load_state(
        words: &mut Vec<EmittedWord>,
        relocs: &mut Vec<Reloc>,
        reg: u8,
        slot: &BootInitSlotSpec,
    ) {
        let word = words.len();
        load_imm(words, reg, 0, &format!("state {}", slot.name));
        for i in 0..4 {
            if let Some(ew) = words.get_mut(word + i) {
                ew.text = format!("state-addr[{i}] {} x{reg}", slot.name);
            }
        }
        if slot.is_driver {
            relocs.push(Reloc::DriverState {
                word,
                driver: slot.name.clone(),
            });
        } else {
            relocs.push(Reloc::MailboxAddr {
                word,
                actor: slot.name.clone(),
                field: MailboxField::State,
            });
        }
    }
    fn bl_key(words: &mut Vec<EmittedWord>, relocs: &mut Vec<Reloc>, key: &str) {
        let word = words.len();
        push(
            words,
            encode::enc_bl(0),
            format!("bl <{key}>"),
            CostRule::Call,
            Some(0),
            &[],
        );
        relocs.push(Reloc::Call {
            word,
            key: key.to_string(),
        });
    }
    fn emit_arg(
        words: &mut Vec<EmittedWord>,
        relocs: &mut Vec<Reloc>,
        reg: u8,
        arg: &BootInitArgSpec,
    ) -> Result<u64, String> {
        match arg {
            BootInitArgSpec::Word(w) => {
                load_imm(words, reg, *w, "init arg");
                Ok(0)
            }
            BootInitArgSpec::DeviceRegsBase(i) => {
                let word = words.len();
                load_imm(words, reg, 0, &format!("device#{i} regs"));
                for j in 0..4 {
                    if let Some(ew) = words.get_mut(word + j) {
                        ew.text = format!("device-regs[{j}] device#{i} x{reg}");
                    }
                }
                relocs.push(Reloc::DeviceRegsBase { word, device: *i });
                Ok(0)
            }
            BootInitArgSpec::PoolBase(name) => {
                let word = words.len();
                load_imm(words, reg, 0, &format!("pool {name}"));
                for j in 0..4 {
                    if let Some(ew) = words.get_mut(word + j) {
                        ew.text = format!("pool-base[{j}] {name} x{reg}");
                    }
                }
                relocs.push(Reloc::PoolBase {
                    word,
                    pool: name.clone(),
                });
                Ok(0)
            }
            BootInitArgSpec::OwnSlot {
                pool,
                index,
                slot_bytes,
            } => {
                let word = words.len();
                load_imm(words, reg, 0, &format!("own {pool}[{index}]"));
                for j in 0..4 {
                    if let Some(ew) = words.get_mut(word + j) {
                        ew.text = format!("pool-slot[{j}] {pool}[{index}] x{reg}");
                    }
                }
                relocs.push(Reloc::PoolSlot {
                    word,
                    pool: pool.clone(),
                    index: *index,
                    slot_bytes: *slot_bytes,
                });
                Ok(0)
            }
            BootInitArgSpec::OwnHandleArray {
                pool,
                count,
                slot_bytes,
            } => {
                let raw = count
                    .checked_mul(8)
                    .ok_or_else(|| "own-handle array byte count overflow".to_string())?;
                let bytes = ((raw + 15) / 16) * 16;
                if bytes == 0 || bytes >= 4096 {
                    return Err(format!(
                        "own-handle array for pool `{pool}` wants {bytes} bytes \
                         (count={count}); boot's unsigned-immediate SUB reaches 4095"
                    ));
                }
                push(
                    words,
                    encode::enc_sub_imm(31, 31, bytes as u16, true),
                    format!("sub sp, sp, #{bytes}  ; own-handle table"),
                    CostRule::Alu,
                    Some(31),
                    &[31],
                );
                for i in 0..*count {
                    let word = words.len();
                    load_imm(words, 9, 0, &format!("own {pool}[{i}]"));
                    for j in 0..4 {
                        if let Some(ew) = words.get_mut(word + j) {
                            ew.text = format!("pool-slot[{j}] {pool}[{i}] x9");
                        }
                    }
                    relocs.push(Reloc::PoolSlot {
                        word,
                        pool: pool.clone(),
                        index: i,
                        slot_bytes: *slot_bytes,
                    });
                    push(
                        words,
                        encode::enc_str_x_imm(9, 31, (i * 8) as u16),
                        format!("str x9, [sp, #{}]", i * 8),
                        CostRule::Store,
                        None,
                        &[9, 31],
                    );
                }
                push(
                    words,
                    encode::enc_add_imm(reg, 31, 0, true),
                    format!("mov {}, sp", reg_name(reg)),
                    CostRule::Alu,
                    Some(reg),
                    &[31],
                );
                Ok(bytes)
            }
        }
    }

    let mut words: Vec<EmittedWord> = Vec::new();
    let mut relocs: Vec<Reloc> = Vec::new();

    let Some(call) = &slot.init else {
        panic!("emit_boot_init_call: slot `{}` has no init", slot.name);
    };

    // x30 save — second hang regression (layout::build_boot_init doc).
    push(
        &mut words,
        encode::enc_sub_imm(31, 31, 16, true),
        "sub sp, sp, #16".to_string(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_str_x_imm(30, 31, 0),
        "str x30, [sp]".to_string(),
        CostRule::Store,
        None,
        &[30, 31],
    );

    let mut array_stack: u64 = 0;
    for (i, arg) in call.args.iter().enumerate() {
        match emit_arg(&mut words, &mut relocs, i as u8 + 1, arg) {
            Ok(n) => array_stack += n,
            Err(msg) => panic!("emit_boot_init_call: {msg}"),
        }
    }
    load_state(&mut words, &mut relocs, 0, slot);
    if call.fallible {
        let (msg_off, msg_len) = call.err_msg.unwrap_or_else(|| {
            panic!(
                "emit_boot_init_call: fallible `{}` has no interned abort message",
                call.key
            )
        });
        push(
            &mut words,
            encode::enc_sub_imm(31, 31, 16, true),
            "sub sp, sp, #16  ; reply slot".to_string(),
            CostRule::Alu,
            Some(31),
            &[31],
        );
        push(
            &mut words,
            encode::enc_add_imm(8, 31, 0, true),
            "mov x8, sp".to_string(),
            CostRule::Alu,
            Some(8),
            &[31],
        );
        bl_key(&mut words, &mut relocs, &call.key);
        push(
            &mut words,
            encode::enc_ldr_x_imm(9, 31, 0),
            "ldr x9, [sp]  ; Result tag".to_string(),
            CostRule::Load,
            Some(9),
            &[31],
        );
        push(
            &mut words,
            encode::enc_add_imm(31, 31, 16, true),
            "add sp, sp, #16  ; drop reply slot".to_string(),
            CostRule::Alu,
            Some(31),
            &[31],
        );
        let ok_fixup = words.len();
        push(
            &mut words,
            0,
            "cbz x9, .ok".to_string(),
            CostRule::Branch,
            None,
            &[],
        );
        // `__wrela_abort(x0=*Bytes)` — stack slot, then BL (noreturn).
        push(
            &mut words,
            encode::enc_sub_imm(31, 31, 16, true),
            "sub sp, sp, #16  ; abort Bytes slot".to_string(),
            CostRule::Alu,
            Some(31),
            &[31],
        );
        let word_adrp = words.len();
        push(
            &mut words,
            encode::enc_adrp(10, 0),
            format!("adrp x10, rodata+{msg_off}"),
            CostRule::Adrp,
            Some(10),
            &[],
        );
        push(
            &mut words,
            encode::enc_add_imm(10, 10, 0, true),
            format!("add x10, x10, #rodata+{msg_off}"),
            CostRule::Alu,
            Some(10),
            &[10],
        );
        relocs.push(Reloc::Rodata {
            word_adrp,
            byte_offset: msg_off,
        });
        push(
            &mut words,
            encode::enc_str_x_imm(10, 31, 0),
            "str x10, [sp]  ; Bytes.base".to_string(),
            CostRule::Store,
            None,
            &[10, 31],
        );
        load_imm(&mut words, 10, msg_len as u64, "abort msg len");
        push(
            &mut words,
            encode::enc_str_x_imm(10, 31, 8),
            "str x10, [sp, #8]  ; Bytes.len".to_string(),
            CostRule::Store,
            None,
            &[10, 31],
        );
        push(
            &mut words,
            encode::enc_add_imm(0, 31, 0, true),
            "add x0, sp, #0  ; *Bytes".to_string(),
            CostRule::Alu,
            Some(0),
            &[31],
        );
        let abort_word = words.len();
        push(
            &mut words,
            encode::enc_bl(0),
            "bl <__wrela_abort>".to_string(),
            CostRule::Abort,
            None,
            &[],
        );
        relocs.push(Reloc::AbortFixed { word: abort_word });
        let after = words.len();
        let delta = (after as i64 - ok_fixup as i64) * 4;
        if let Some(ew) = words.get_mut(ok_fixup) {
            ew.word = encode::enc_cbz(9, delta as i32, true);
            ew.text = format!("cbz x9, .ok ({delta})");
            ew.rule = CostRule::Branch;
            ew.dst = None;
            ew.srcs = [9, 0, 0, 0];
            ew.src_len = 1;
        }
    } else {
        bl_key(&mut words, &mut relocs, &call.key);
    }
    if array_stack > 0 {
        assert!(
            array_stack < 4096,
            "own-handle array stack frame is {array_stack} bytes"
        );
        push(
            &mut words,
            encode::enc_add_imm(31, 31, array_stack as u16, true),
            format!("add sp, sp, #{array_stack}  ; free own-handle table"),
            CostRule::Alu,
            Some(31),
            &[31],
        );
    }

    push(
        &mut words,
        encode::enc_ldr_x_imm(30, 31, 0),
        "ldr x30, [sp]".to_string(),
        CostRule::Load,
        Some(30),
        &[31],
    );
    push(
        &mut words,
        encode::enc_add_imm(31, 31, 16, true),
        "add sp, sp, #16".to_string(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_ret(30),
        "ret".to_string(),
        CostRule::Branch,
        None,
        &[30],
    );

    CodegenFn {
        frame_size: 16,
        code: words,
        relocs,
    }
}

// --- plans/M11.md item I: checkpoint section floor trampoline -------------
//
// M10 G specialized the full checkpoint/vector body into this section
// (decision 670). M11 I migrates the algorithm to force-rooted wrela
// (`__wrela_vector0` / `__wrela_rt_checkpoint`); the section keeps only
// floor-cat2 LR save/restore around `BL __wrela_rt_checkpoint` (decision
// 821 / 673 extraction — same honesty as H's SP install). IRQ/wake Call
// stubs are inject-only NON_INVENTORY (decision 823), like boot_init_call.

/// One sealed `IrqCap.bind` site (inject overwrites `__irq_call_*`).
#[derive(Debug, Clone)]
pub struct CheckpointIrqSpec {
    pub vector: u64,
    pub handler_key: String,
    pub driver_state: u64,
}

/// One `@driver` sticky wake-pending → `@task` drain site.
#[derive(Debug, Clone)]
pub struct CheckpointWakeSpec {
    pub driver_state: u64,
    pub wake_pending_off: u64,
    pub task_key: String,
}

/// Result of the checkpoint-section trampoline builder.
pub struct CheckpointEmitResult {
    pub words: Vec<u32>,
    pub checkpoint_service_word: usize,
    /// Always `None` after M11 item E: poll lives in `code`.
    pub deadline_poll_word: Option<usize>,
    /// Entry driver should `bl_call_key("__wrela_deadline_poll")`.
    pub has_deadline_poll: bool,
    pub relocs: Vec<Reloc>,
}

/// M11 I / decision 821: floor-cat2 LR save/restore (5 words).
/// Contiguous halves used by [`emit_checkpoint_service_trampoline`]:
/// `sub`/`str` then (after the BL) `ldr`/`add`/`ret`.
pub fn emit_checkpoint_lr_frame() -> Vec<EmittedWord> {
    vec![
        EmittedWord::new(
            encode::enc_sub_imm(31, 31, 16, true),
            "sub sp, sp, #16  ; floor cat2".into(),
            CostRule::Alu,
            Some(31),
            &[31],
        ),
        EmittedWord::new(
            encode::enc_str_x_imm(30, 31, 0),
            "str x30, [sp]  ; floor cat2".into(),
            CostRule::Store,
            None,
            &[30, 31],
        ),
        EmittedWord::new(
            encode::enc_ldr_x_imm(30, 31, 0),
            "ldr x30, [sp]  ; floor cat2".into(),
            CostRule::Load,
            Some(30),
            &[31],
        ),
        EmittedWord::new(
            encode::enc_add_imm(31, 31, 16, true),
            "add sp, sp, #16  ; floor cat2".into(),
            CostRule::Alu,
            Some(31),
            &[31],
        ),
        EmittedWord::new(
            encode::enc_ret(30),
            "ret  ; floor cat2".into(),
            CostRule::Branch,
            None,
            &[30],
        ),
    ]
}

/// M11 I: checkpoint section = floor LR frame around `BL __wrela_rt_checkpoint`.
/// Service entry is at word 0 (vector0 body lives in `code` as `__wrela_vector0`).
/// When `link_body` is false, emit a bare `ret` (no runtime wiring).
///
/// M13 item N: `__wrela_rt_checkpoint` takes `core`; the async/IRQ trampoline
/// is core-0 only, so it materializes `x0 = 0` before the BL.
pub fn emit_checkpoint_service_trampoline(
    has_deadline_poll: bool,
    link_body: bool,
) -> CheckpointEmitResult {
    if !link_body {
        return CheckpointEmitResult {
            words: vec![encode::enc_ret(30)],
            checkpoint_service_word: 0,
            deadline_poll_word: None,
            has_deadline_poll,
            relocs: vec![],
        };
    }
    let frame = emit_checkpoint_lr_frame();
    debug_assert_eq!(frame.len(), 5);
    let mut words = Vec::new();
    let mut relocs = Vec::new();
    // save (2)
    words.push(frame[0].word);
    words.push(frame[1].word);
    // core argument (async checkpoints always service core 0)
    words.push(encode::enc_movz(0, 0, 0, true));
    let bl_word = words.len();
    words.push(encode::enc_bl(0));
    relocs.push(Reloc::Call {
        word: bl_word,
        key: "__wrela_rt_checkpoint".into(),
    });
    // restore (3)
    words.push(frame[2].word);
    words.push(frame[3].word);
    words.push(frame[4].word);
    CheckpointEmitResult {
        words,
        checkpoint_service_word: 0,
        deadline_poll_word: None,
        has_deadline_poll,
        relocs,
    }
}

/// M11 I / decision 823: specialized IRQ handler stub (`x0 = driver_state`).
pub fn emit_checkpoint_irq_call(spec: &CheckpointIrqSpec) -> CodegenFn {
    emit_driver_state_call(&spec.handler_key, spec.driver_state)
}

/// M11 I / decision 823: specialized `@task` wake stub (`x0 = driver_state`).
pub fn emit_checkpoint_wake_call(spec: &CheckpointWakeSpec) -> CodegenFn {
    emit_driver_state_call(&spec.task_key, spec.driver_state)
}

fn emit_driver_state_call(key: &str, driver_state: u64) -> CodegenFn {
    let mut words: Vec<EmittedWord> = Vec::new();
    let mut relocs: Vec<Reloc> = Vec::new();
    push(
        &mut words,
        encode::enc_sub_imm(31, 31, 16, true),
        "sub sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_str_x_imm(30, 31, 0),
        "str x30, [sp]".into(),
        CostRule::Store,
        None,
        &[30, 31],
    );
    load_imm(&mut words, 0, driver_state, "driver_state");
    let bl = words.len();
    push(
        &mut words,
        encode::enc_bl(0),
        format!("bl <{key}>"),
        CostRule::Call,
        Some(0),
        &[0],
    );
    relocs.push(Reloc::Call {
        word: bl,
        key: key.to_string(),
    });
    push(
        &mut words,
        encode::enc_ldr_x_imm(30, 31, 0),
        "ldr x30, [sp]".into(),
        CostRule::Load,
        Some(30),
        &[31],
    );
    push(
        &mut words,
        encode::enc_add_imm(31, 31, 16, true),
        "add sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_ret(30),
        "ret".into(),
        CostRule::Branch,
        None,
        &[30],
    );
    CodegenFn {
        frame_size: 16,
        code: words,
        relocs,
    }
}

/// M11 J / decision 831: specialized method-dispatch stub.
/// ABI: `x0=arg0, x1=arg1, x2=stage` → sets `x8=stage`, `x0=state`, then
/// `bl <method_key>`. Aggregate returns write through `x8`; non-aggregate
/// methods ignore it. Inject overwrites `__method_R_M` placeholders.
pub fn emit_method_call_stub(method_key: &str, state: u64) -> CodegenFn {
    let mut words: Vec<EmittedWord> = Vec::new();
    let mut relocs: Vec<Reloc> = Vec::new();
    push(
        &mut words,
        encode::enc_sub_imm(31, 31, 16, true),
        "sub sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_str_x_imm(30, 31, 0),
        "str x30, [sp]".into(),
        CostRule::Store,
        None,
        &[30, 31],
    );
    // x8 = stage (x2); shift args; x0 = state.
    push(
        &mut words,
        encode::enc_mov_reg(8, 2, true),
        "mov x8, x2  ; aggregate stage".into(),
        CostRule::Alu,
        Some(8),
        &[2],
    );
    push(
        &mut words,
        encode::enc_mov_reg(2, 1, true),
        "mov x2, x1  ; arg1".into(),
        CostRule::Alu,
        Some(2),
        &[1],
    );
    push(
        &mut words,
        encode::enc_mov_reg(1, 0, true),
        "mov x1, x0  ; arg0".into(),
        CostRule::Alu,
        Some(1),
        &[0],
    );
    load_imm(&mut words, 0, state, "actor state");
    let bl = words.len();
    push(
        &mut words,
        encode::enc_bl(0),
        format!("bl <{method_key}>"),
        CostRule::Call,
        Some(0),
        &[0, 1, 2],
    );
    relocs.push(Reloc::Call {
        word: bl,
        key: method_key.to_string(),
    });
    push(
        &mut words,
        encode::enc_ldr_x_imm(30, 31, 0),
        "ldr x30, [sp]".into(),
        CostRule::Load,
        Some(30),
        &[31],
    );
    push(
        &mut words,
        encode::enc_add_imm(31, 31, 16, true),
        "add sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_ret(30),
        "ret".into(),
        CostRule::Branch,
        None,
        &[30],
    );
    CodegenFn {
        frame_size: 16,
        code: words,
        relocs,
    }
}

/// M11 K / decision 851: specialized `@test(runtime)` call stub.
/// Loads resolved handle args into `x0..`, sets `x8` to `OFF_TEST_LINE_BUF`,
/// `bl <test_key>`, returns status in `x0`. Inject overwrites `__test_call_i`.
pub fn emit_test_call_stub(test_key: &str, args: &[u64]) -> CodegenFn {
    let mut words: Vec<EmittedWord> = Vec::new();
    let mut relocs: Vec<Reloc> = Vec::new();
    push(
        &mut words,
        encode::enc_sub_imm(31, 31, 16, true),
        "sub sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_str_x_imm(30, 31, 0),
        "str x30, [sp]".into(),
        CostRule::Store,
        None,
        &[30, 31],
    );
    for (i, &v) in args.iter().enumerate() {
        assert!(i < 8, "emit_test_call_stub: too many args");
        load_imm(&mut words, i as u8, v, &format!("test arg {i}"));
    }
    let line_buf =
        wrela_machine::layout::MACHINE_INFO_BASE + wrela_machine::machine_info::OFF_TEST_LINE_BUF;
    load_imm(&mut words, 8, line_buf, "OFF_TEST_LINE_BUF");
    let bl = words.len();
    let mut call_srcs: Vec<u8> = (0..args.len().min(4)).map(|i| i as u8).collect();
    if call_srcs.len() < 4 {
        call_srcs.push(8);
    }
    push(
        &mut words,
        encode::enc_bl(0),
        format!("bl <{test_key}>"),
        CostRule::Call,
        Some(0),
        &call_srcs,
    );
    relocs.push(Reloc::Call {
        word: bl,
        key: test_key.to_string(),
    });
    push(
        &mut words,
        encode::enc_ldr_x_imm(30, 31, 0),
        "ldr x30, [sp]".into(),
        CostRule::Load,
        Some(30),
        &[31],
    );
    push(
        &mut words,
        encode::enc_add_imm(31, 31, 16, true),
        "add sp, sp, #16".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_ret(30),
        "ret".into(),
        CostRule::Branch,
        None,
        &[30],
    );
    CodegenFn {
        frame_size: 16,
        code: words,
        relocs,
    }
}

/// M11 K / decision 851: append interned `test <name>: ` prefix via
/// `__wrela_console_append_bytes`. Inject overwrites `__test_prefix_i`.
/// Bytes is by-pointer: stack slot `(base, capacity)`, `x0 = &*slot`,
/// `x1 = copy_len`.
pub fn emit_test_prefix_stub(rodata_off: usize, len: u64) -> CodegenFn {
    let mut words: Vec<EmittedWord> = Vec::new();
    let mut relocs: Vec<Reloc> = Vec::new();
    // 16-byte Bytes slot at [sp], LR at [sp,#16].
    push(
        &mut words,
        encode::enc_sub_imm(31, 31, 32, true),
        "sub sp, sp, #32".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_str_x_imm(30, 31, 16),
        "str x30, [sp, #16]".into(),
        CostRule::Store,
        None,
        &[30, 31],
    );
    let word_adrp = words.len();
    push(
        &mut words,
        encode::enc_adrp(9, 0),
        format!("adrp x9, rodata+{rodata_off:#x}"),
        CostRule::Adrp,
        Some(9),
        &[],
    );
    push(
        &mut words,
        encode::enc_add_imm(9, 9, 0, true),
        format!("add x9, x9, #rodata+{rodata_off:#x}"),
        CostRule::Alu,
        Some(9),
        &[9],
    );
    relocs.push(Reloc::Rodata {
        word_adrp,
        byte_offset: rodata_off,
    });
    push(
        &mut words,
        encode::enc_str_x_imm(9, 31, 0),
        "str x9, [sp]  ; Bytes.base".into(),
        CostRule::Store,
        None,
        &[9, 31],
    );
    load_imm(&mut words, 9, len, "Bytes.capacity");
    push(
        &mut words,
        encode::enc_str_x_imm(9, 31, 8),
        "str x9, [sp, #8]  ; Bytes.len".into(),
        CostRule::Store,
        None,
        &[9, 31],
    );
    push(
        &mut words,
        encode::enc_add_imm(0, 31, 0, true),
        "add x0, sp, #0  ; *Bytes".into(),
        CostRule::Alu,
        Some(0),
        &[31],
    );
    load_imm(&mut words, 1, len, "copy len");
    let bl = words.len();
    push(
        &mut words,
        encode::enc_bl(0),
        "bl <__wrela_console_append_bytes>".into(),
        CostRule::Call,
        Some(0),
        &[0, 1],
    );
    relocs.push(Reloc::Call {
        word: bl,
        key: "__wrela_console_append_bytes".into(),
    });
    push(
        &mut words,
        encode::enc_ldr_x_imm(30, 31, 16),
        "ldr x30, [sp, #16]".into(),
        CostRule::Load,
        Some(30),
        &[31],
    );
    push(
        &mut words,
        encode::enc_add_imm(31, 31, 32, true),
        "add sp, sp, #32".into(),
        CostRule::Alu,
        Some(31),
        &[31],
    );
    push(
        &mut words,
        encode::enc_ret(30),
        "ret".into(),
        CostRule::Branch,
        None,
        &[30],
    );
    CodegenFn {
        frame_size: 32,
        code: words,
        relocs,
    }
}

/// The whole-program entry point: every sync fn (`mwir::MwirProgram`, via
/// the existing `emit_fn`) plus every async fn/method (`flowwir::FlowWirProgram`,
/// via `emit_flowwir_fn` above), merged into one `CodegenProgram` sharing
/// one rodata pool — so a sync call and an async dispatch-table entry
/// resolve against the exact same `fn_word_base` map one stage later
/// (`layout.rs`), with no special-casing by color.
pub fn codegen_program_with_async(
    mwir: &MwirProgram,
    flow: &FlowWirProgram,
    layout: &LayoutCtx,
    method_index: &ActorMethodIndex,
    // plans/M6.md item F: `layout::RuntimeTables::group_arena_capacity` —
    // the whole-build static arena size `GroupCreate`'s own scan (and the
    // group-child poll routines `layout.rs` builds alongside it) needs;
    // `0` for a build with no `with group(...)` sites at all (every
    // pre-item-F caller, byte-identical: `GroupCtx` is only ever consulted
    // by a `FlowInst::GroupCreate`/`GroupStart`, neither of which any
    // pre-F program ever lowers).
    group_arena_capacity: u64,
    // plans/M10.md item D / decision 613: per-mailbox-root specialized
    // enqueue bodies (`name`, `capacity`, `slot_size`). Empty when the
    // image has no mailbox roots (dump without an `@image`, sync-only).
    _enqueue_specs: &[(String, u64, u64)],
) -> Result<CodegenProgram, CodegenError> {
    if block_count() {
        NEXT_BLOCK_ID.with(|c| c.set(0));
    }
    let mut rodata = RodataPool::new();
    rodata.seed(&mwir.rodata);
    let (child_index, max_children) = compute_group_child_indices(flow)?;
    let gctx = GroupCtx {
        arena_capacity: group_arena_capacity,
        max_children,
        child_index,
    };
    let mut fns = BTreeMap::new();
    for (key, f) in &mwir.fns {
        fns.insert(key.clone(), emit_fn(key, f, layout, &mut rodata)?);
    }
    for (key, f) in &flow.fns {
        fns.insert(
            key.clone(),
            emit_flowwir_fn(key, f, layout, &mut rodata, method_index, &gctx)?,
        );
    }
    // M11 J: rt_enqueue bodies are generic wrela; layout aliases keys.
    Ok(CodegenProgram {
        fns,
        rodata: rodata.entries,
    })
}

// --- top-level entry ----------------------------------------------------------

pub fn codegen_program(
    mwir: &MwirProgram,
    layout: &LayoutCtx,
) -> Result<CodegenProgram, CodegenError> {
    if block_count() {
        NEXT_BLOCK_ID.with(|c| c.set(0));
    }
    let mut rodata = RodataPool::new();
    rodata.seed(&mwir.rodata);
    let mut fns = BTreeMap::new();
    for (key, f) in &mwir.fns {
        let cf = emit_fn(key, f, layout, &mut rodata)?;
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
        for (i, ew) in f.code.iter().enumerate() {
            push_line(
                &mut out,
                2,
                &format!("{i:04}: {:08x}  {}", ew.word, ew.text),
            );
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

// --- structural validation (plans/M5.md item G) -----------------------------

/// A small, pure, cheap structural sanity check over an already-produced
/// `CodegenProgram` — `cargo xtask fuzz lower`'s own invariant (d). Rejected
/// as too weak to matter (task note, recorded here rather than silently
/// dropped): re-decoding every emitted `u32` back into a mnemonic
/// (`encode::looks_like_valid_a76`-style) would only ever prove this
/// module's own encoder round-trips against itself, never that the bits are
/// *correct* — decision 5 (plans/M5.md) already settled that HVF execution
/// is the one real behavioral oracle for emitted bytes, and the boot golden
/// (item E's own bug #3, a wrong-bit-field `enc_umulh`) is the concrete
/// proof a self-consistent decode/encode round-trip would have missed
/// anyway. What *is* cheap and real: the handful of structural facts a
/// codegen bug could actually violate without any of the existing per-
/// instruction unit tests or `--stage=asm` goldens ever seeing it, because
/// every one of them is a cross-cutting property over the *whole* program
/// rather than one instruction in isolation:
///
/// - every fn's own `code` is non-empty — every `emit_fn` call always
///   emits at least its own fixed-shape prologue and epilogue, regardless
///   of how short the mwir body it wraps is, so an empty `code` vector can
///   only mean a producer bug, never a legitimately tiny fn;
/// - every `Reloc::Call`'s own `word` index is in range for its own fn's
///   `code`, and its `key` resolves — either to another fn this same
///   `CodegenProgram` contains, or (since M6-D, and corrected here by
///   plans/M7.md item Y) to one of the `rt_enqueue <Actor>` glue symbols
///   `layout.rs` hand-assembles, which a compiled `await`/`send` through
///   an `Actor[T]` handle legitimately calls — layout.rs's own `Reloc`
///   resolution (`layout_program`/
///   `layout_test_image`) would otherwise hit its own `"internal error: call
///   target ... was never codegen'd"` guard one stage later, a strictly
///   worse place to first notice this than right here, immediately after
///   the fn that emitted the dangling reloc finishes;
/// - every `Reloc::Rodata`'s own `word_adrp`/`word_adrp + 1` pair (the
///   `ADRP`+`ADD` `codegen.rs` always emits back-to-back, never
///   independently) is in range, and its `byte_offset` names a real
///   position inside the concatenation of every `program.rodata` entry;
/// - every `Reloc::AbortFixed`/`AbortVal`'s own `word` index is in range
///   (their own *target* — `__wrela_abort`/`__wrela_abort_val` — is a
///   layout-time fact this stage has no way to check yet; only the
///   *source* word index is this stage's own responsibility).
///
/// `Err` names the first violation found (fn-key iteration order, then
/// reloc order within that fn) — never a panic, since this exists
/// specifically so the fuzzer can call it on arbitrary fuzzed-and-codegen'd
/// programs and report a clean diagnostic rather than an out-of-bounds
/// index panic reaching all the way out to `catch_unwind`.
pub fn validate(program: &CodegenProgram) -> Result<(), String> {
    let rodata_len: usize = program.rodata.iter().map(Vec::len).sum();
    for (key, f) in &program.fns {
        if f.code.is_empty() {
            return Err(format!(
                "fn `{key}` emitted zero code words (every fn always has at least a \
                 prologue/epilogue)"
            ));
        }
        for reloc in &f.relocs {
            match reloc {
                Reloc::Call { word, key: target } => {
                    if *word >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::Call word {word} is out of range (code has {} \
                             word(s))",
                            f.code.len()
                        ));
                    }
                    // plans/M7.md item Y's own find: this arm predates the
                    // async pipeline and used to demand that every call
                    // target be a compiled fn in this same program. That
                    // has been false since M6-D — a compiled `await`/`send`
                    // through an `Actor[X]` handle emits a symbolic
                    // `bl <rt_enqueue X>`, and that routine is hand-
                    // assembled by `layout.rs` into the harness section,
                    // never codegen'd here. `layout_test_image` already
                    // resolves both naming schemes deliberately
                    // (`fn_word_base` -> glue symbols), so the shape is
                    // legitimate, not a dangling reloc. It went unnoticed
                    // because `validate` has no production caller at all —
                    // only the fuzz lanes reach it, and until item Y there
                    // was no lane that drove the async pipeline.
                    //
                    // A synthesized symbol is checked for being a *real*
                    // glue target rather than waved through on being
                    // synthetic-shaped: `rt_enqueue_actor` must name an
                    // actor, so a garbled `rt_enqueue ` key is still a
                    // finding here, one stage before layout's own guard.
                    let resolvable = program.fns.contains_key(target)
                        || rt_enqueue_actor(target).is_some_and(|a| !a.is_empty())
                        || rt_run_one_glue_target(target)
                        || rt_select_and_run_glue_target(target)
                        || target == "__wrela_rt_run_one"
                        || target == "__wrela_deadline_poll"
                        || target == "__wrela_deadline_scan"
                        || target == "__wrela_rt_checkpoint"
                        || target == "__wrela_vector0";
                    if !resolvable {
                        return Err(format!(
                            "fn `{key}`: Reloc::Call targets `{target}`, which this \
                             `CodegenProgram` never codegen'd and which is not an \
                             `rt_enqueue` / `rt_select_and_run` / `rt_drain` / \
                             `rt_xreply` / `__wrela_*` glue symbol either"
                        ));
                    }
                }
                Reloc::Rodata {
                    word_adrp,
                    byte_offset,
                } => {
                    if word_adrp + 1 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::Rodata word_adrp {word_adrp} (its paired ADD sits \
                             at +1) is out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                    if *byte_offset >= rodata_len {
                        return Err(format!(
                            "fn `{key}`: Reloc::Rodata byte_offset {byte_offset} is out of range \
                             (rodata is {rodata_len} byte(s))"
                        ));
                    }
                }
                Reloc::AbortFixed { word } | Reloc::AbortVal { word } => {
                    if *word >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::AbortFixed/AbortVal word {word} is out of range \
                             (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::CheckpointService { word } => {
                    if *word >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::CheckpointService word {word} is out of range \
                             (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::TurnFrameAddr { word, .. } => {
                    // A four-word `load_imm`: the last patched word sits
                    // at `word + 3` (its *target* — a turn area address —
                    // is a layout-time fact this stage cannot check).
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::TurnFrameAddr word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::TurnsBase { word } | Reloc::TurnStride { word } => {
                    // plans/M10.md item 0c3: the two halves of the drain's
                    // own index→address step, each a four-word `load_imm`
                    // of a layout-time constant.
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::TurnsBase/TurnStride word {word} (a 4-word                              load_imm) is out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::MailboxAddr { word, .. } => {
                    // plans/M10.md item D: four-word load_imm of a mailbox
                    // ring/tail/count address.
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::MailboxAddr word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::RrCursor { word, .. } => {
                    // plans/M10.md item E3 / decision 621: four-word
                    // load_imm of one core's RR cursor address.
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::RrCursor word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::TurnIdImm { word, .. } => {
                    // A four-word `load_imm` — identical shape/reasoning to
                    // `Reloc::TurnFrameAddr` above; its target (a `TurnId`)
                    // is likewise a layout-time fact.
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::TurnIdImm word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::GroupArenaBase { word } => {
                    // A four-word `load_imm` — identical shape/reasoning
                    // to `Reloc::TurnFrameAddr` (its own target, the
                    // whole-image group arena's base address, is a
                    // layout-time fact this stage cannot check).
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::GroupArenaBase word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::IrqVector { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::IrqVector word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::WakePending { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::WakePending word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::RingAddr { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::RingAddr word {word} (a 4-word load_imm) is \
                             out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
                Reloc::DriverState { word, .. }
                | Reloc::DeviceRegsBase { word, .. }
                | Reloc::PoolBase { word, .. }
                | Reloc::PoolSlot { word, .. } => {
                    if word + 3 >= f.code.len() {
                        return Err(format!(
                            "fn `{key}`: Reloc::DriverState/DeviceRegsBase/PoolBase/PoolSlot \
                             word {word} (a 4-word load_imm) is out of range (code has {} word(s))",
                            f.code.len()
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// plans/M10.md item F0: live word counts for image-static specialization
/// emitters (decisions 613 / 620) under the census reference configuration.
#[cfg(test)]
pub(crate) fn emitted_a64_census_specialization_live_counts()
-> std::collections::BTreeMap<&'static str, usize> {
    use std::collections::BTreeMap;
    let mut out = BTreeMap::new();
    // M11 J: emit_rt_enqueue / emit_rt_select_and_run deleted (force-rooted wrela).
    // emit_rt_run_one / emit_rt_child_poll deleted in M11 F.
    // M11 G: emit_rt_xsend / xreply / drain deleted (force-rooted wrela).
    // M11 H: secondary algorithm → wrela; floor SP install measured here.
    out.insert(
        "emit_secondary_sp_install",
        emit_secondary_sp_install(1, 2).len(),
    );
    // emit_boot_init deleted (force-rooted __wrela_rt_boot_init); call
    // stubs are inject-only (decision 812) — not a census REF row.
    // M11 I: checkpoint algorithm → wrela; floor-cat2 LR frame measured here.
    out.insert("emit_checkpoint_lr_frame", emit_checkpoint_lr_frame().len());
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
        let layout = mwir::build_layout_ctx(&module, &Default::default())
            .expect("test source must build a layout ctx");
        (mwir_program, layout)
    }

    /// A blown Lane 2 pool must fail **closed**, not degrade into a report
    /// with no `.img` and exit code 0 (plans/M20.md item B measured that
    /// fail-open on `wrela build --block-count`). Drives the real allocator
    /// to its real bound rather than asserting on a hand-written string.
    #[test]
    fn block_id_pool_exhaustion_is_a_fail_closed_error() {
        set_block_count(true);
        // One below the bound still allocates.
        NEXT_BLOCK_ID.with(|c| c.set((crate::rtconfig::BLOCK_POOL_COUNT - 1) as u32));
        let last = alloc_block_id().expect("the final id in the pool must allocate");
        assert_eq!(last as usize, crate::rtconfig::BLOCK_POOL_COUNT - 1);
        // The next one is over it, and the message must carry the marker
        // `layout::try_layout_with_codegen` routes on.
        let err = alloc_block_id().expect_err("one past the pool must fail");
        assert!(
            err.message.starts_with(FAIL_CLOSED_PREFIX),
            "pool exhaustion must be marked fail-closed, got: {}",
            err.message
        );
        assert!(
            err.message.contains("BLOCK_POOL_COUNT"),
            "the error must name the bound it blew, got: {}",
            err.message
        );
        set_block_count(false);
    }

    /// The other half of the same oracle: an ordinary "did not lower"
    /// codegen error must stay **soft**, or every `err-cross-core-*` report
    /// golden (report, no `.img`) would start failing the build.
    #[test]
    fn an_ordinary_codegen_error_is_not_marked_fail_closed() {
        let soft = CodegenError::unimplemented("some shape");
        assert!(
            !soft.message.starts_with(FAIL_CLOSED_PREFIX),
            "unimplemented must remain soft, got: {}",
            soft.message
        );
        let internal = CodegenError::internal("some invariant");
        assert!(
            !internal.message.starts_with(FAIL_CLOSED_PREFIX),
            "producer bugs travel under their own census-tracked prefix, got: {}",
            internal.message
        );
    }

    // --- frame-slot assignment (task note 5's own first requirement) ---

    #[test]
    fn frame_slots_are_assigned_in_temp_order_with_no_packing() {
        let f = MwirFn {
            receiver: None,
            params: vec![(Temp(0), AccessMode::Read)],
            ret: Type::U64,
            temp_types: vec![Type::U8, Type::U64, Type::Bool],
            body: vec![Inst::Return { value: None }],
        };
        let layout = LayoutCtx::default();
        let frame = build_frame(&f, &layout, 0, 0, 0).expect("build_frame");
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
        let frame = build_frame(&f, &layout, 0, 0, 0).expect("build_frame");
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

    /// plans/M7.md item Z1: the reply staging slot is a sibling of
    /// `ret_ptr_off` — reserved only when asked for, sized exactly as
    /// asked, and pushing `lr` (and so the frame size) out by that much.
    /// The `0` case is the one every M6 frame takes, and it must leave
    /// the frame byte-for-byte as it was (decision 9c).
    #[test]
    fn frame_reserves_the_reply_staging_slot_only_when_sized() {
        let f = MwirFn {
            receiver: None,
            params: vec![],
            ret: Type::U64,
            temp_types: vec![Type::U64],
            body: vec![Inst::Return { value: None }],
        };
        let layout = LayoutCtx::default();
        let none = build_frame(&f, &layout, 0, 0, 0).expect("build_frame");
        assert_eq!(none.reply_stage_off, None);
        assert_eq!(none.lr_off, 8);
        assert_eq!(none.size, 16);
        let staged = build_frame(&f, &layout, 24, 0, 0).expect("build_frame");
        assert_eq!(staged.reply_stage_off, Some(8));
        assert_eq!(staged.lr_off, 32);
        assert_eq!(staged.size, 48);
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
        assert!(build_frame(&f, &layout, 0, 0, 0).is_err());
    }

    /// plans/M9.md item RR: the imm12 ceiling is on `off + slot_bias`, the
    /// number `addr_of_slot` actually encodes — not on `size` alone.
    ///
    /// A frame of exactly 4040 bytes is legal for a sync fn (bias 0, and
    /// `4040 <= 4095`) and must be *refused* for an async one, whose every
    /// slot reference is biased past the `TURN_RECORD_SIZE`-byte turn
    /// record: `4040 + 64 = 4104` is past the field, where the surplus
    /// bit lands in `enc_add_imm`'s `shift` and quietly assembles a
    /// different instruction. Checking `size` alone let exactly this
    /// through.
    #[test]
    fn an_async_frame_is_bounded_by_imm12_less_the_slot_bias() {
        // 503 * 8 = 4024 bytes of temp, + 8 for `lr` = 4032, rounded to
        // 4032; one more 8-byte temp puts it at 4040.
        let f = MwirFn {
            receiver: None,
            params: vec![],
            ret: Type::Unit,
            temp_types: vec![Type::Array(
                Box::new(Type::U64),
                Box::new(ast::Expr::Int(ast::Span::default(), "504".to_string())),
            )],
            body: vec![Inst::Return { value: None }],
        };
        let layout = LayoutCtx::default();

        let sync = build_frame(&f, &layout, 0, 0, 0).expect("legal for a sync frame");
        assert_eq!(sync.size, 4048);

        let bias = TURN_RECORD_SIZE as usize;
        assert!(
            sync.size + bias > 4095,
            "this fixture must straddle the boundary to be a regression lock"
        );
        let Err(err) = build_frame(&f, &layout, 0, 0, bias) else {
            panic!("the same frame must be refused once biased past the turn record");
        };
        assert!(
            err.message.contains("4031"),
            "the diagnostic names the biased ceiling: {}",
            err.message
        );

        // And the largest frame that still fits with the bias applied is
        // accepted, so the bound is not merely conservative-by-accident.
        let smaller = MwirFn {
            temp_types: vec![Type::Array(
                Box::new(Type::U64),
                Box::new(ast::Expr::Int(ast::Span::default(), "500".to_string())),
            )],
            ..f
        };
        let ok = build_frame(&smaller, &layout, 0, 0, bias).expect("fits under 4031 with the bias");
        assert!(ok.size + bias <= 4095);
    }

    /// plans/M19.md item I / decision 1486: small imm under NarrowImm is
    /// one `movz` word (naive path stays four).
    #[test]
    fn narrow_imm_small_constant_emits_one_word() {
        let mwir = const_return_mwir(42);
        let layout = LayoutCtx::default();

        set_narrow_imm(false);
        let naive = codegen_program(&mwir, &layout).expect("naive");
        set_narrow_imm(true);
        let narrow = codegen_program(&mwir, &layout).expect("narrow");
        set_narrow_imm(false);

        let naive_mov = mov_wide_words(&naive.fns["c"]);
        let narrow_mov = mov_wide_words(&narrow.fns["c"]);
        assert_eq!(
            naive_mov.len(),
            4,
            "naive must stay four words: {naive_mov:?}"
        );
        assert_eq!(
            narrow_mov.len(),
            1,
            "small imm must be one movz: {narrow_mov:?}"
        );
        assert_eq!(narrow_mov[0], encode::enc_movz(X_A, 42, 0, true));
        assert_eq!(materialize_mov_wide(&narrow_mov), 42);
        assert_eq!(
            materialize_mov_wide(&naive_mov),
            materialize_mov_wide(&narrow_mov)
        );
    }

    /// Sparse high halfword: NarrowImm skips the zero movks.
    #[test]
    fn narrow_imm_sparse_skips_zero_movks() {
        // bit 48 set only → movz at lsl #48; no movk for the zero halves.
        let value: u64 = 1u64 << 48;
        let mwir = const_return_mwir(value as i64);
        let layout = LayoutCtx::default();

        set_narrow_imm(true);
        let narrow = codegen_program(&mwir, &layout).expect("narrow");
        set_narrow_imm(false);

        let narrow_mov = mov_wide_words(&narrow.fns["c"]);
        assert_eq!(
            narrow_mov,
            vec![encode::enc_movz(X_A, 1, 48, true)],
            "sparse high half must be a single movz lsl #48"
        );
        assert_eq!(materialize_mov_wide(&narrow_mov), value);

        // Two non-zero halves with a zero gap: movz + one movk, not four.
        let value2: u64 = (0xAAu64 << 32) | 0x11;
        let mwir2 = const_return_mwir(value2 as i64);
        set_narrow_imm(true);
        let narrow2 = codegen_program(&mwir2, &layout).expect("narrow2");
        set_narrow_imm(false);
        let mov2 = mov_wide_words(&narrow2.fns["c"]);
        assert_eq!(
            mov2,
            vec![
                encode::enc_movz(X_A, 0x11, 0, true),
                encode::enc_movk(X_A, 0xAA, 32, true),
            ],
            "zero middle half must be skipped"
        );
        assert_eq!(materialize_mov_wide(&mov2), value2);
    }

    /// Narrow and naive materializations yield identical register bits.
    #[test]
    fn narrow_imm_bits_match_naive() {
        let layout = LayoutCtx::default();
        let samples: &[i64] = &[
            0,
            1,
            -1,
            0xFFFF,
            0x1_0000,
            0x1_0000_0000,
            (1i64 << 48) | 0x42,
            i64::MIN,
            i64::MAX,
        ];
        for &v in samples {
            let mwir = const_return_mwir(v);
            set_narrow_imm(false);
            let naive = codegen_program(&mwir, &layout).expect("naive");
            set_narrow_imm(true);
            let narrow = codegen_program(&mwir, &layout).expect("narrow");
            set_narrow_imm(false);
            let naive_bits = materialize_mov_wide(&mov_wide_words(&naive.fns["c"]));
            let narrow_bits = materialize_mov_wide(&mov_wide_words(&narrow.fns["c"]));
            assert_eq!(
                naive_bits, narrow_bits,
                "value {v:#x}: naive {naive_bits:#x} != narrow {narrow_bits:#x}"
            );
            assert_eq!(naive_bits, v as u64, "naive must recover {v:#x}");
        }
    }

    fn const_return_mwir(value: i64) -> MwirProgram {
        MwirProgram {
            fns: BTreeMap::from([(
                "c".to_string(),
                MwirFn {
                    receiver: None,
                    params: vec![],
                    ret: Type::U64,
                    temp_types: vec![Type::U64],
                    body: vec![
                        Inst::ConstInt {
                            dst: Temp(0),
                            ty: Type::U64,
                            value: value as i128,
                        },
                        Inst::Return {
                            value: Some(Temp(0)),
                        },
                    ],
                },
            )]),
            rodata: vec![],
        }
    }

    fn mov_wide_words(f: &CodegenFn) -> Vec<u32> {
        f.code
            .iter()
            .filter(|ew| ew.rule == CostRule::MovWide)
            .map(|ew| ew.word)
            .collect()
    }

    /// Reconstruct the 64-bit value a MOVZ/MOVK sequence leaves in the Rd.
    fn materialize_mov_wide(words: &[u32]) -> u64 {
        let mut val = 0u64;
        for &w in words {
            let imm16 = ((w >> 5) & 0xFFFF) as u64;
            let hw = (w >> 21) & 0b11;
            let shift = hw * 16;
            let opc = (w >> 29) & 0b11;
            match opc {
                0b10 => {
                    // MOVZ: set selected half, zero the rest.
                    val = imm16 << shift;
                }
                0b11 => {
                    // MOVK: set selected half, leave others.
                    let mask = !(0xFFFFu64 << shift);
                    val = (val & mask) | (imm16 << shift);
                }
                other => panic!("unexpected move-wide opc {other:#x} in word {w:#x}"),
            }
        }
        val
    }

    /// plans/M20.md item E: the emitted divide declares its **result and
    /// its operands**, so a consumer of the quotient waits on it.
    ///
    /// Before this item `emit_div_rem` pushed `dst = None, srcs = &[]`,
    /// which meant a 20-cycle divide created no dependence edge at all and
    /// nothing downstream ever waited — a genuine under-cost in the one
    /// direction 04 §5 forbids. A source scan would not catch it (the
    /// `CostRule` tag was already right); only the tags themselves say it.
    #[test]
    fn emitted_divide_declares_result_and_operands() {
        const SRC: &str = r#"
module examples.cost_div_tags

pub fn q(a: u64, b: u64) -> u64:
    return a / b

pub fn r(a: u64, b: u64) -> u64:
    return a % b
"#;
        let tokens = lexer::lex(SRC).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let typed = sema::check_typed(&module, "<test>").expect("check");
        let layout = mwir::build_layout_ctx(&module, &Default::default()).expect("layout");
        let mwir_program = crate::lower::lower_program(&typed).expect("lower");
        let prog = codegen_program(&mwir_program, &layout).expect("codegen");

        let mut divides = 0usize;
        let mut msubs = 0usize;
        for f in prog.fns.values() {
            for ew in &f.code {
                match ew.rule {
                    CostRule::Udiv | CostRule::Sdiv => {
                        divides += 1;
                        assert_eq!(
                            ew.dst,
                            Some(X_C),
                            "the divide must declare its quotient register"
                        );
                        assert!(
                            ew.src_slice().contains(&X_A) && ew.src_slice().contains(&X_B),
                            "the divide must declare both operands, got {:?}",
                            ew.src_slice()
                        );
                    }
                    CostRule::Mul => {
                        // The `%` lowering's `msub Xd, Xn, Xm, Xa` reads
                        // the accumulator `Xa` too.
                        msubs += 1;
                        assert!(
                            ew.src_slice().contains(&X_A),
                            "msub must declare its accumulator source, got {:?}",
                            ew.src_slice()
                        );
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(divides, 2, "one divide per fn");
        assert_eq!(msubs, 1, "only the `%` lowering emits the msub");

        // And the edge is live in the scoreboard: the store of the
        // quotient reads X_C, so it cannot issue before the divide
        // retires. Score the `q` fn alone against the committed profile.
        let table = crate::cost::table::load_default().expect("bench/a76-pi5.toml");
        let place = crate::placement::PlacementTable::default();
        let scored = crate::cost::score_program(&prog, &table, &place).expect("score");
        let q = scored
            .fns
            .iter()
            .find(|f| f.key == "q")
            .expect("fn q scored");
        assert!(
            q.proxy_cycles > table.latency(CostRule::Udiv),
            "the consumer of the quotient must extend past the divide's own {} \
             cycles, got {}",
            table.latency(CostRule::Udiv),
            q.proxy_cycles
        );
    }

    /// plans/M19.md item I Cheap: cost-calls proxy rank drops with NarrowImm
    /// on vs off while BoundsElide stays fixed (many small immediates).
    #[test]
    fn narrow_imm_lowers_cost_calls_proxy_rank() {
        use crate::cost::score::score_program;
        use crate::cost::table::load_default;
        use crate::lower::set_bounds_elide;

        let src = include_str!("../../../tests/golden/cost-calls/input.wr");
        let tokens = lexer::lex(src).expect("lex");
        let module = parser::parse(tokens).expect("parse");
        let typed = sema::check_typed(&module, "<test>").expect("check");
        let layout = mwir::build_layout_ctx(&module, &Default::default()).expect("layout");
        let table = load_default().expect("bench/a76-pi5.toml");

        // Hold BoundsElide fixed on (golden/default path).
        set_bounds_elide(true);
        let mwir_program = crate::lower::lower_program(&typed).expect("lower");

        let place = crate::placement::PlacementTable::default();
        set_narrow_imm(false);
        let off_prog = codegen_program(&mwir_program, &layout).expect("codegen off");
        let off = score_program(&off_prog, &table, &place).expect("score off");

        set_narrow_imm(true);
        let on_prog = codegen_program(&mwir_program, &layout).expect("codegen on");
        let on = score_program(&on_prog, &table, &place).expect("score on");
        set_narrow_imm(false);

        assert!(
            on.total_proxy_cycles < off.total_proxy_cycles,
            "NarrowImm-on {} must rank strictly below NarrowImm-off {} on cost-calls",
            on.total_proxy_cycles,
            off.total_proxy_cycles
        );
        let off_mov: usize = off_prog
            .fns
            .values()
            .map(|f| {
                f.code
                    .iter()
                    .filter(|ew| ew.rule == CostRule::MovWide)
                    .count()
            })
            .sum();
        let on_mov: usize = on_prog
            .fns
            .values()
            .map(|f| {
                f.code
                    .iter()
                    .filter(|ew| ew.rule == CostRule::MovWide)
                    .count()
            })
            .sum();
        assert!(
            on_mov < off_mov,
            "NarrowImm must emit fewer mov_wide words ({on_mov} vs {off_mov})"
        );
    }

    /// plans/M15.md item K / decision 1098: `--omit-dmb` strips every
    /// `Inst::Dmb` word from the asm dump (cheap oracle for the mutation
    /// front-door; the focused boot proves the guest-visible half).
    #[test]
    fn omit_dmb_strips_barrier_words_from_asm() {
        let mwir = MwirProgram {
            fns: BTreeMap::from([(
                "barrier".to_string(),
                MwirFn {
                    receiver: None,
                    params: vec![],
                    ret: Type::Unit,
                    temp_types: vec![],
                    body: vec![
                        Inst::Dmb {
                            option: "ishst".to_string(),
                        },
                        Inst::Dmb {
                            option: "ishld".to_string(),
                        },
                        Inst::Return { value: None },
                    ],
                },
            )]),
            rodata: vec![],
        };
        let layout = LayoutCtx::default();

        set_omit_dmb(false);
        let intact = codegen_program(&mwir, &layout).expect("intact codegen");
        let intact_dump = dump(&intact);
        assert!(
            intact_dump.contains("dmb ishst") && intact_dump.contains("dmb ishld"),
            "intact must emit both barriers:\n{intact_dump}"
        );
        assert!(
            intact_dump.contains(&format!("{:08x}", encode::enc_dmb_ishst())),
            "intact must carry DMB ISHST encoding"
        );
        assert!(
            intact_dump.contains(&format!("{:08x}", encode::enc_dmb_ishld())),
            "intact must carry DMB ISHLD encoding"
        );

        set_omit_dmb(true);
        let mutated = codegen_program(&mwir, &layout).expect("mutated codegen");
        let mutated_dump = dump(&mutated);
        set_omit_dmb(false);
        assert!(
            !mutated_dump.contains("dmb ishst")
                && !mutated_dump.contains("dmb ishld")
                && !mutated_dump.contains(&format!("{:08x}", encode::enc_dmb_ishst()))
                && !mutated_dump.contains(&format!("{:08x}", encode::enc_dmb_ishld())),
            "omit-dmb must strip every DMB word:\n{mutated_dump}"
        );
        let intact_words = intact.fns["barrier"].code.len();
        let mutated_words = mutated.fns["barrier"].code.len();
        assert_eq!(
            intact_words - mutated_words,
            2,
            "exactly two DMB words must disappear under omit-dmb"
        );
    }

    /// Integrity Phase 2 Item M: `--block-count` injects
    /// `bl <__wrela_block_hit>` at every MWIR leader; off leaves asm alone.
    #[test]
    fn block_count_emits_hit_calls_at_leaders() {
        let mwir = MwirProgram {
            fns: BTreeMap::from([(
                "branchy".to_string(),
                MwirFn {
                    receiver: None,
                    params: vec![],
                    ret: Type::Unit,
                    temp_types: vec![Type::Bool],
                    body: vec![
                        Inst::ConstBool {
                            dst: Temp(0),
                            value: true,
                        },
                        Inst::JumpIfFalse {
                            cond: Temp(0),
                            target: 3,
                        },
                        Inst::Jump { target: 4 },
                        Inst::ConstBool {
                            dst: Temp(0),
                            value: false,
                        },
                        Inst::Return { value: None },
                    ],
                },
            )]),
            rodata: vec![],
        };
        let layout = LayoutCtx::default();

        set_block_count(false);
        let off = codegen_program(&mwir, &layout).expect("off");
        let off_dump = dump(&off);
        assert!(
            !off_dump.contains("bl <__wrela_block_hit>"),
            "default must not instrument:\n{off_dump}"
        );

        set_block_count(true);
        let on_a = codegen_program(&mwir, &layout).expect("on a");
        let on_b = codegen_program(&mwir, &layout).expect("on b");
        set_block_count(false);
        let on_dump = dump(&on_a);
        assert_eq!(
            dump(&on_a),
            dump(&on_b),
            "block-count emission must be deterministic across two runs"
        );
        let hits = on_dump.matches("bl <__wrela_block_hit>").count();
        // leaders: 0, 2 (after JumpIfFalse), 3 (target), 4 (after Jump / Return)
        assert_eq!(hits, 4, "expected one hit call per leader:\n{on_dump}");
        assert!(
            on_a.fns["branchy"].code.len() > off.fns["branchy"].code.len(),
            "instrumented body must grow"
        );
    }

    /// plans/M20.md item B / decision 1607: Lane 2 instruments **every**
    /// owner. One two-block fn per owner bucket, all four in one program:
    /// `app`, `runtime` (a `core.runtime.*` key), `driver` (a `.on_*` key)
    /// — and the counter helper itself, which must stay uninstrumented or
    /// its first hit self-recurses forever (measured: the guest faults).
    ///
    /// This is the oracle for the gate drop: restoring
    /// `classify_owner(key) == "app"` at either site makes it fail, because
    /// the runtime and driver bodies would emit no hit call.
    #[test]
    fn block_count_instruments_runtime_and_driver_owners() {
        fn two_block_fn() -> MwirFn {
            MwirFn {
                receiver: None,
                params: vec![],
                ret: Type::Unit,
                temp_types: vec![Type::Bool],
                body: vec![
                    Inst::ConstBool {
                        dst: Temp(0),
                        value: true,
                    },
                    Inst::JumpIfFalse {
                        cond: Temp(0),
                        target: 3,
                    },
                    Inst::Jump { target: 3 },
                    Inst::Return { value: None },
                ],
            }
        }

        let keys = [
            "app_fn",
            "core.runtime.helper",
            "Blk.on_turn",
            "__wrela_block_hit",
        ];
        for k in keys {
            let expect = match k {
                "app_fn" => "app",
                "core.runtime.helper" | "__wrela_block_hit" => "runtime",
                _ => "driver",
            };
            assert_eq!(
                crate::cost::owner::classify_owner(k),
                expect,
                "owner fixture for {k} drifted"
            );
        }

        let mwir = MwirProgram {
            fns: keys
                .iter()
                .map(|k| ((*k).to_string(), two_block_fn()))
                .collect(),
            rodata: vec![],
        };
        let layout = LayoutCtx::default();

        set_block_count(true);
        let on = codegen_program(&mwir, &layout).expect("codegen on");
        let ids = block_ids_assigned();
        set_block_count(false);

        for k in ["app_fn", "core.runtime.helper", "Blk.on_turn"] {
            let hits = on.fns[k]
                .code
                .iter()
                .filter(|w| w.text == "bl <__wrela_block_hit>")
                .count();
            assert_eq!(
                hits,
                3,
                "{k} ({}) must be instrumented at every leader under decision 1607",
                crate::cost::owner::classify_owner(k)
            );
        }
        let self_hits = on.fns["__wrela_block_hit"]
            .code
            .iter()
            .filter(|w| w.text == "bl <__wrela_block_hit>")
            .count();
        assert_eq!(
            self_hits, 0,
            "the counter helper must never be instrumented — that is unbounded self-recursion"
        );
        // 3 instrumented fns × 3 leaders (0, the JumpIfFalse fallthrough,
        // and the shared target); the helper allocates none.
        assert_eq!(ids, 9, "one id per instrumented leader, helper excluded");
    }

    /// plans/M20.md item B: the widened Lane 2 id count on the cost-stage
    /// closure of the `boot-actors` control case, pinned so the number
    /// stays checked rather than living in a commit message. Measured
    /// 2026-07-29: 184 ids widened (123 under the pre-M20 `app`-only gate).
    ///
    /// **Scope of this bound.** This is the closure `wrela dump
    /// --stage=asm|cost` builds — the surface the cost model scores. It is
    /// *not* the `@test(runtime)` boot image, whose widened count is far
    /// larger (2522 for `boot-actors`, 2786 max across `boot-*`) and does
    /// **not** fit `BLOCK_POOL_COUNT`; see this item's report.
    #[test]
    fn block_count_id_count_on_boot_actors_cost_stage_is_pinned() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/golden/boot-actors/input.wr");

        set_block_count(true);
        let prog = crate::cost::stage::codegen_cost_stage(&path);
        let ids = block_ids_assigned();
        set_block_count(false);
        let prog = prog.expect("boot-actors cost-stage codegen under --block-count");

        let hits: usize = prog
            .fns
            .values()
            .map(|f| {
                f.code
                    .iter()
                    .filter(|w| w.text == "bl <__wrela_block_hit>")
                    .count()
            })
            .sum();
        assert_eq!(
            hits, ids as usize,
            "every allocated id must emit exactly one hit call"
        );
        assert_eq!(
            ids, 184,
            "boot-actors cost-stage Lane 2 id count moved; re-measure and cite the new number \
             (plans/M20.md item B)"
        );
        assert!(
            (ids as usize) < crate::rtconfig::BLOCK_POOL_COUNT,
            "cost-stage id count {ids} must stay under BLOCK_POOL_COUNT {}",
            crate::rtconfig::BLOCK_POOL_COUNT
        );
    }

    #[test]
    fn mwir_block_leaders_marks_targets_and_fallthrough() {
        let body = vec![
            Inst::ConstBool {
                dst: Temp(0),
                value: true,
            },
            Inst::JumpIfFalse {
                cond: Temp(0),
                target: 3,
            },
            Inst::Jump { target: 4 },
            Inst::ConstBool {
                dst: Temp(0),
                value: false,
            },
            Inst::Return { value: None },
        ];
        assert_eq!(
            mwir_block_leaders(&body),
            vec![true, false, true, true, true]
        );
    }

    // --- end-to-end: exact word sequences for tiny fns ------------------

    #[test]
    fn add_emission_records_alu_rule_and_regs() {
        // plans/M18.md item C: emit-time CostRule + dest/src regs; never
        // parse mnemonics. Wide checked add uses `adds` with X_C←X_A,X_B.
        let (mwir_program, layout) = compile(
            "module examples.codegen_cost_add\n\npub fn add(a: i64, b: i64) -> i64:\n    return a + b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["add"];
        let adds = f
            .code
            .iter()
            .find(|ew| ew.text.starts_with("adds "))
            .expect("expected adds in add body");
        assert_eq!(adds.rule, CostRule::Alu);
        assert_eq!(adds.dst, Some(X_C));
        assert_eq!(adds.src_slice(), &[X_A, X_B]);
    }

    #[test]
    fn sync_frame_load_store_tagged_stack() {
        // cost hard-cut item B: proven SP-relative slot traffic → Stack.
        let (mwir_program, layout) = compile(
            "module examples.codegen_memref_stack\n\npub fn answer() -> u64:\n    return 42\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["answer"];
        let mem_ops: Vec<&EmittedWord> = f
            .code
            .iter()
            .filter(|ew| matches!(ew.rule, CostRule::Load | CostRule::Store))
            .collect();
        assert!(!mem_ops.is_empty());
        for ew in mem_ops {
            let mem = ew.mem.expect("load/store must be tagged");
            assert_eq!(
                mem.class,
                crate::cost::MemClass::Stack,
                "sp-relative {} should be Stack: {}",
                ew.rule.as_str(),
                ew.text
            );
        }
    }

    #[test]
    fn adrp_has_no_memref() {
        // cost hard-cut item B: Adrp never carries a MemRef.
        // Narrow checked add emits inline abort stubs that ADRP rodata messages.
        let (mwir_program, layout) = compile(
            "module examples.codegen_memref_adrp\n\npub fn add(a: u8, b: u8) -> u8:\n    return a + b\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["add"];
        let adrps: Vec<&EmittedWord> = f
            .code
            .iter()
            .filter(|ew| ew.rule == CostRule::Adrp)
            .collect();
        assert!(!adrps.is_empty(), "expected adrp in abort stubs");
        for ew in &adrps {
            assert_eq!(ew.mem, None, "adrp must not carry MemRef: {}", ew.text);
        }
    }

    #[test]
    fn memref_for_base_imm_non_sp_is_cold_in_codegen_helpers() {
        // Proven [x28, #imm] (async slot base) classifies as Cold, not Stack.
        assert_eq!(
            MemRef::for_base_imm(X_FRAME, 64).class,
            crate::cost::MemClass::Cold
        );
        assert_eq!(MemRef::for_base_imm(X_SP, 16), MemRef::stack(16));
    }

    #[test]
    fn unknown_load_via_push_gets_unique_cold() {
        // Raw FnCtx::push of Load/Store (no proven base+imm) → unique Cold.
        // Checked add overflow path emits `bl <__wrela_abort>`; the
        // register-indirect device/pending forms use push → unique. Here
        // we lock the allocator shape that push uses.
        let u0 = MemRef::cold_unique(0);
        let u1 = MemRef::cold_unique(1);
        assert_eq!(u0.class, crate::cost::MemClass::Cold);
        assert_ne!(u0.key, u1.key);
        assert_ne!(u0.key & (1u64 << 63), 0);
        // Contrast: proven non-SP base+imm is stable (no high bit).
        let stable = MemRef::for_base_imm(X_A, 0);
        assert_eq!(stable.class, crate::cost::MemClass::Cold);
        assert_eq!(stable.key & (1u64 << 63), 0);
    }

    // --- integrity item D: push / push_mem structural asserts ------------

    #[test]
    fn push_shape_call_requires_x0_dst() {
        check_push_shape(CostRule::Call, Some(0), &[], None);
        check_push_shape(CostRule::Call, Some(0), &[1, 2], None);
    }

    #[test]
    #[should_panic(expected = "Call must declare dst=Some(0)")]
    fn push_shape_call_without_x0_dst_fails() {
        check_push_shape(CostRule::Call, None, &[], None);
    }

    #[test]
    #[should_panic(expected = "Call must declare dst=Some(0)")]
    fn push_shape_call_wrong_dst_fails() {
        check_push_shape(CostRule::Call, Some(1), &[], None);
    }

    #[test]
    fn push_shape_load_known_addr_needs_src() {
        check_push_shape(
            CostRule::Load,
            Some(0),
            &[MEM_SP_REG],
            Some(&MemRef::stack(8)),
        );
        check_push_shape(
            CostRule::Load,
            Some(0),
            &[X_A],
            Some(&MemRef::for_base_imm(X_A, 0)),
        );
    }

    #[test]
    #[should_panic(expected = "Load with known address")]
    fn push_shape_load_known_addr_without_src_fails() {
        check_push_shape(CostRule::Load, Some(0), &[], Some(&MemRef::stack(8)));
    }

    #[test]
    fn push_shape_load_unique_cold_empty_srcs_ok() {
        // Unique-cold path still ok when tagged (address unknown / pessimistic).
        check_push_shape(CostRule::Load, Some(0), &[], Some(&MemRef::cold_unique(0)));
    }

    #[test]
    fn push_shape_store_nonunique_requires_base_in_srcs() {
        check_push_shape(
            CostRule::Store,
            None,
            &[0, MEM_SP_REG],
            Some(&MemRef::stack(16)),
        );
        check_push_shape(
            CostRule::Store,
            None,
            &[1, X_FRAME],
            Some(&MemRef::for_base_imm(X_FRAME, 64)),
        );
    }

    #[test]
    #[should_panic(expected = "base reg")]
    fn push_shape_store_nonunique_missing_base_fails() {
        // Stack MemRef base is SP (31); srcs only carry the stored value.
        check_push_shape(CostRule::Store, None, &[0], Some(&MemRef::stack(8)));
    }

    #[test]
    fn push_shape_store_unique_cold_exempt_from_base() {
        check_push_shape(CostRule::Store, None, &[0], Some(&MemRef::cold_unique(3)));
    }

    #[test]
    fn push_shape_untagged_load_store_helpers_unique() {
        // Document the coerce: missing MemRef on Load/Store is treated as
        // unique cold by push_mem; shape check then exempts empty-src Loads.
        assert!(memref_is_unique_cold(&MemRef::cold_unique(0)));
        assert!(!memref_is_unique_cold(&MemRef::stack(0)));
        assert!(!memref_is_unique_cold(&MemRef::for_base_imm(X_A, 0)));
        assert_eq!(memref_nonunique_base(&MemRef::stack(24)), Some(MEM_SP_REG));
        assert_eq!(
            memref_nonunique_base(&MemRef::for_base_imm(X_FRAME, 8)),
            Some(X_FRAME)
        );
        assert_eq!(memref_nonunique_base(&MemRef::cold_unique(1)), None);
    }

    #[test]
    fn a_fn_returning_a_constant_compiles_to_the_expected_word_sequence() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_const_test\n\npub fn answer() -> u64:\n    return 42\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        let f = &program.fns["answer"];
        assert_eq!(f.frame_size, 16);
        let words: Vec<u32> = f.code.iter().map(|ew| ew.word).collect();
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
                let ew = &combo.code[*word];
                assert_eq!(ew.word, encode::enc_bl(0));
                assert_eq!(ew.text, "bl <add_one>");
                assert_eq!(ew.rule, CostRule::Call);
                assert_eq!(ew.dst, Some(0));
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
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
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
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
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
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
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
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
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
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
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
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
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
        let mnems: Vec<&str> = f.code.iter().map(|ew| ew.text.as_str()).collect();
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

    // --- structural validation (plans/M5.md item G, `validate`) -----------

    #[test]
    fn validate_accepts_a_real_multi_fn_program() {
        let (mwir_program, layout) = compile(
            "module examples.codegen_validate_ok\n\npub fn add_one(x: u64) -> u64:\n    return x + 1\n\npub fn use_it(x: u64) -> u64:\n    return add_one(x)\n",
        );
        let program = codegen_program(&mwir_program, &layout).expect("codegen_program");
        // A real program with an actual `Reloc::Call` between two fns and,
        // via `checked_add`'s own literal abort message, a real
        // `Reloc::Rodata` too — both `validate`'s own live paths, not just
        // its empty-program fast path.
        assert!(program.fns.values().any(|f| !f.relocs.is_empty()));
        validate(&program).expect("a real codegen'd program must validate");
    }

    #[test]
    fn validate_rejects_empty_code() {
        let mut program = CodegenProgram::default();
        program.fns.insert(
            "fn:empty".to_string(),
            CodegenFn {
                frame_size: 16,
                code: Vec::new(),
                relocs: Vec::new(),
            },
        );
        let err = validate(&program).unwrap_err();
        assert!(err.contains("emitted zero code words"), "{err}");
    }

    #[test]
    fn validate_rejects_a_call_reloc_to_an_unknown_fn() {
        let mut program = CodegenProgram::default();
        program.fns.insert(
            "fn:caller".to_string(),
            CodegenFn {
                frame_size: 16,
                code: vec![EmittedWord::new(0, String::new(), CostRule::Alu, None, &[])],
                relocs: vec![Reloc::Call {
                    word: 0,
                    key: "fn:ghost".to_string(),
                }],
            },
        );
        let err = validate(&program).unwrap_err();
        assert!(err.contains("never codegen'd"), "{err}");
    }

    #[test]
    fn validate_rejects_a_call_reloc_word_out_of_range() {
        let mut program = CodegenProgram::default();
        program.fns.insert(
            "fn:only".to_string(),
            CodegenFn {
                frame_size: 16,
                code: vec![EmittedWord::new(0, String::new(), CostRule::Alu, None, &[])],
                relocs: vec![Reloc::Call {
                    word: 5,
                    key: "fn:only".to_string(),
                }],
            },
        );
        let err = validate(&program).unwrap_err();
        assert!(err.contains("Reloc::Call word 5 is out of range"), "{err}");
    }

    #[test]
    fn validate_rejects_a_rodata_reloc_byte_offset_out_of_range() {
        let mut program = CodegenProgram::default();
        program.fns.insert(
            "fn:only".to_string(),
            CodegenFn {
                frame_size: 16,
                code: vec![
                    EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]),
                    EmittedWord::new(0, String::new(), CostRule::Alu, None, &[]),
                ],
                relocs: vec![Reloc::Rodata {
                    word_adrp: 0,
                    byte_offset: 100,
                }],
            },
        );
        program.rodata.push(b"hi".to_vec());
        let err = validate(&program).unwrap_err();
        assert!(
            err.contains("Reloc::Rodata byte_offset 100 is out of range"),
            "{err}"
        );
    }

    #[test]
    fn validate_rejects_an_abort_reloc_word_out_of_range() {
        let mut program = CodegenProgram::default();
        program.fns.insert(
            "fn:only".to_string(),
            CodegenFn {
                frame_size: 16,
                code: vec![EmittedWord::new(0, String::new(), CostRule::Alu, None, &[])],
                relocs: vec![Reloc::AbortFixed { word: 3 }],
            },
        );
        let err = validate(&program).unwrap_err();
        assert!(
            err.contains("Reloc::AbortFixed/AbortVal word 3 is out of range"),
            "{err}"
        );
    }

    /// plans/M7.md item H1 self-audit: `mmio_access_width`'s three fail-
    /// closed arms, called directly. The signed-register and out-of-reach
    /// arms are also source-reachable (`golden/err-mmio-signed-register`,
    /// `golden/err-mmio-offset-out-of-reach`); the alignment arm is not —
    /// `types::check_layouts` already refuses a misaligned `@offset`, so
    /// it stays an `internal` rather than a panic, pinned here.
    #[test]
    fn mmio_access_width_fail_closed_arms() {
        let signed = mmio_access_width(&Type::I32, 0).expect_err("signed");
        assert!(
            signed.message.contains("unsigned") && signed.message.contains("sign-extending"),
            "{}",
            signed.message
        );

        let far = mmio_access_width(&Type::U32, 0x10000).expect_err("far");
        assert!(
            far.message.contains("0x10000") && far.message.contains("unsigned-immediate"),
            "{}",
            far.message
        );

        let misaligned = mmio_access_width(&Type::U32, 1).expect_err("misaligned");
        assert!(
            misaligned.message.contains("internal error:")
                && misaligned.message.contains("not 4-byte aligned")
                && misaligned.message.contains("check_layouts"),
            "{}",
            misaligned.message
        );
    }
}

// M11 F: rt_child_poll_tests deleted with emit_rt_child_poll.
// M11 J: rt_select_and_run_tests deleted with emit_rt_select_and_run / emit_rt_enqueue.

#[cfg(test)]
mod synthetic_symbol_tests {
    use super::*;

    /// The shadowing hazard, stated as a test: a source fn cannot be
    /// named anything whose `CalleeKey::spelling()` equals a synthesized
    /// glue symbol, because identifiers cannot contain a space.
    #[test]
    fn synthesized_symbols_are_unrepresentable_as_source_keys() {
        let sym = rt_enqueue_symbol("Doubler");
        assert!(symbol_is_synthetic(&sym), "{sym} must be synthetic");
        assert_eq!(rt_enqueue_actor(&sym), Some("Doubler"));
        for plausible in [
            "rt_enqueue_Doubler",
            "__rt_enqueue_Doubler",
            "Doubler",
            "A.rt_enqueue",
        ] {
            assert!(
                !symbol_is_synthetic(plausible),
                "{plausible} is source-shaped"
            );
            assert_ne!(plausible, sym);
        }
        // M10 E3: the same space discipline for the scheduler tick and
        // the glue targets it Calls.
        assert!(symbol_is_synthetic(&rt_run_one_symbol(0)));
        assert!(symbol_is_synthetic(&rt_select_and_run_symbol("Store")));
        assert!(symbol_is_synthetic(&rt_child_poll_symbol("child")));
        assert!(symbol_is_synthetic(&rt_drain_symbol(1)));
        assert!(symbol_is_synthetic(&rt_xreply_symbol(0, 1)));
        assert!(symbol_is_synthetic(&rt_xsend_symbol(0, "Actor")));
        assert!(symbol_is_synthetic(&rt_secondary_core_entry_symbol(1)));
        assert!(symbol_is_synthetic(&rt_boot_init_symbol()));
    }
}

#[cfg(test)]
mod rt_cross_core_tests {
    use super::*;

    #[test]
    fn xreply_cores_parse_and_secondary_still_pins_run_one() {
        assert_eq!(rt_xreply_cores(&rt_xreply_symbol(1, 0)), Some((1, 0)));
        assert_eq!(rt_xreply_cores("rt_enqueue Actor"), None);

        let sp = emit_secondary_sp_install(1, 2);
        assert_eq!(sp.len(), 5); // floor-cat1 SP (decision 811)
    }
}
