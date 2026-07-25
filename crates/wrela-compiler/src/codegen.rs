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

/// plans/M6.md decision 6: any *backward* unconditional `Jump` is a loop's
/// own back-edge (the exact, and only, shape `lower.rs`/`flowwir_lower.rs`
/// ever emit for one — a `while`/`for`'s own trailing repeat-jump to its
/// condition check). A forward `Jump` (an `if`/`match` arm's own
/// end-of-block skip) is never a loop back-edge and never gets a
/// checkpoint. `target <= idx` (not just `<`) is deliberately inclusive:
/// no producer ever emits a genuine self-jump, so this can never
/// misclassify anything in practice, and stays the simpler, dumber check.
fn is_loop_back_edge(inst: &Inst, idx: usize) -> bool {
    matches!(inst, Inst::Jump { target } if *target <= idx)
}

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
                    | "Rejected"
                    // plans/M7.md item G, decision 17: one word, passed by
                    // value like every other builtin pseudo-type.
                    | "InterruptCell"
            ) || crate::eval::image_checks::is_sealed_authority_type_name(name) =>
        {
            false
        }
        Type::Named(..) | Type::Tuple(_) | Type::Array(..) | Type::Option(_) | Type::Result(..) => {
            true
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
    lr_off: usize,
    size: usize,
}

fn round_up_16(n: usize) -> usize {
    (n + 15) & !15
}

/// `reply_stage_size` is 0 for every sync fn and for any async fn with no
/// aggregate-reply `await` site (`build_frame_flow` derives the real
/// number); a nonzero value reserves `Frame::reply_stage_off`.
fn build_frame(
    f: &MwirFn,
    layout: &LayoutCtx,
    reply_stage_size: usize,
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
        reply_stage_off,
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
            // plans/M7.md item E4: `IoCompletion[P]` is a real aggregate
            // (payload + status + written_len), not a sealed one-word
            // authority type.
            if name == "IoCompletion" {
                let Some(crate::sema::types::TypeArg::Type(payload)) = targs.first() else {
                    return Err(CodegenError::internal(
                        "`IoCompletion` with no payload type".to_string(),
                    ));
                };
                let fields = [
                    payload.clone(),
                    Type::Result(
                        Box::new(Type::Unit),
                        Box::new(Type::Named("IoError".to_string(), vec![])),
                    ),
                    Type::Named(
                        "Untrusted".to_string(),
                        vec![crate::sema::types::TypeArg::Type(Type::Usize)],
                    ),
                ];
                if index >= fields.len() {
                    return Err(CodegenError::internal(format!(
                        "`IoCompletion` field index {index} out of range"
                    )));
                }
                let mut off = 0usize;
                for f in &fields[..index] {
                    off += mwir::size_of(f, layout).map_err(|e| CodegenError::unimplemented(&e))?;
                }
                let sz = mwir::size_of(&fields[index], layout)
                    .map_err(|e| CodegenError::unimplemented(&e))?;
                return Ok((off, sz));
            }
            // plans/M7.md item G, decision 18: look up by rendered type.
            let key = if targs.is_empty() {
                name.clone()
            } else {
                crate::sema::types::render_type(&Type::Named(name.clone(), targs.to_vec()))
            };
            let fields = layout
                .structs
                .get(&key)
                .ok_or_else(|| CodegenError::internal(format!("unknown struct `{key}`")))?;
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
        // plans/M7.md item Z2: `CallError[E]` (02-language.md §9.4's own
        // five-variant composition, `sema::bodies::compose_call_error`) is
        // the one enum this machine carries as an *instantiated*
        // `Type::Named`, so the generic-instantiation rejection in the arm
        // below would refuse it — leaving the offset authority a hole in
        // exactly the place the `Err(e) -> Err(CallError.Op(e))`
        // recomposition needs one. Its variant list is compiler-known and
        // fixed: the identical order `sema::matches::shape_of` builds,
        // which is what `CALL_ERROR_TAG_CANCELLED` is numbered against and
        // what `mwir::size_of`'s own `CallError` arm sizes. Named here so
        // the recomposition derives `Op`'s payload offset instead of
        // assuming it.
        Type::Named(name, targs) if name == "CallError" => {
            let Some(crate::sema::types::TypeArg::Type(e_ty)) = targs.first() else {
                return Err(CodegenError::internal(
                    "`CallError` with no error type argument",
                ));
            };
            vec![
                vec![e_ty.clone()],                                     // Op(E)
                Vec::new(),                                             // Cancelled
                Vec::new(),                                             // DeadlineExceeded
                vec![Type::Named("Admission".to_string(), Vec::new())], // NotAdmitted
                vec![Type::Named("Peer".to_string(), Vec::new())],      // PeerFailed
            ]
        }
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
    /// The base register every frame-slot access goes through: `X_SP`
    /// for a sync fn's ordinary stack frame; `X_FRAME` (x28, holding the
    /// fn's own persistent turn area address) for an async state
    /// machine, whose locals must survive a suspension's `ret` to the
    /// scheduler (`Reloc::TurnFrameAddr`'s own doc comment).
    slot_base: u8,
    /// A fixed byte bias added to every slot offset: `0` for sync fns;
    /// `TURN_RECORD_SIZE` for async fns (the frame slots sit immediately
    /// past the 48-byte turn record within the turn area).
    slot_bias: usize,
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
        let off = (off + self.slot_bias) as u16;
        let base = self.slot_base;
        self.push(
            encode::enc_ldr_x_imm(reg, base, off),
            format!("ldr {}, [{}, #{off}]", reg_name(reg), reg_name(base)),
        );
    }

    fn store_slot(&mut self, reg: u8, off: usize) {
        let off = (off + self.slot_bias) as u16;
        let base = self.slot_base;
        self.push(
            encode::enc_str_x_imm(reg, base, off),
            format!("str {}, [{}, #{off}]", reg_name(reg), reg_name(base)),
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
        self.push(
            encode::enc_ldr_x_imm(X_B, X_A, 0),
            format!("ldr {}, [{}]", reg_name(X_B), reg_name(X_A)),
        );
        self.push(
            encode::enc_cbz(X_B, 8, true),
            format!("cbz {}, #8", reg_name(X_B)),
        );
        let word = self.cur_word();
        self.push(
            encode::enc_bl(0),
            "bl <__wrela_checkpoint_service>".to_string(),
        );
        self.relocs.push(Reloc::CheckpointService { word });
    }

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
    /// The async entry's own fresh-vs-resume fork (the one consumer):
    /// skip forward over the fresh prologue when the suspended
    /// discriminant is nonzero.
    Cbnz(u8),
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
            SkipKind::Cbnz(r) => (
                encode::enc_cbnz(r, delta, true),
                format!("cbnz {}, #{delta}", reg_name(r)),
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
            ctx.push(enc, format!("{mnem} {rt}, [{}, #{off}]", reg_name(X_A)));
            ctx.store_slot(X_B, ctx.frame.off(*dst));
        }
        // plans/M7.md item G, decision 12: load the driver's vector bit
        // index into an `IrqCap` word. The immediate is patched by layout
        // once the sealed graph's `vector=` is known — identical shape to
        // `Reloc::TurnFrameAddr`/`GroupArenaBase`.
        Inst::LoadIrqVector { dst, driver } => {
            let word = ctx.words.len();
            ctx.load_imm(X_A, 0);
            if let Some((_, text)) = ctx.words.get_mut(word) {
                *text = format!("irq-vector[{}] {}", driver, reg_name(X_A));
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
                    );
                }
                8 => {
                    ctx.push(
                        encode::enc_ldar_x(X_B, X_A),
                        format!("ldar {}, [{}]", reg_name(X_B), reg_name(X_A)),
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
                    );
                }
                8 => {
                    ctx.push(
                        encode::enc_stlr_x(X_B, X_A),
                        format!("stlr {}, [{}]", reg_name(X_B), reg_name(X_A)),
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
        // plans/M7.md item G: sticky store of 1 into the driver's
        // wake-pending word. Level-triggered: a wake before/during/after
        // the bottom half's cell observation remains set until the
        // scheduler clears it after a run that finds the bit still clear
        // on recheck (HVF commit wires that loop).
        Inst::Wake { driver } => {
            let word = ctx.words.len();
            ctx.load_imm(X_A, 0);
            if let Some((_, text)) = ctx.words.get_mut(word) {
                *text = format!("wake-pending[{}] {}", driver, reg_name(X_A));
            }
            ctx.relocs.push(Reloc::WakePending {
                word,
                driver: driver.clone(),
            });
            ctx.load_imm(X_B, 1);
            ctx.push(
                encode::enc_str_x_imm(X_B, X_A, 0),
                format!("str {}, [{}]", reg_name(X_B), reg_name(X_A)),
            );
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
            ctx.push(enc, format!("{mnem} {rt}, [{}, #{off}]", reg_name(X_A)));
        }
        // plans/M7.md item E4 / decision 20: package into the control-pool
        // area after the ring, then mint QueueOp = desc head 0.
        Inst::QueuePrepare {
            dst,
            queue,
            permit: _,
            header,
            payload,
            status,
            device_writes,
            payload_len,
        } => {
            emit_queue_prepare(
                ctx,
                f,
                *dst,
                *queue,
                *header,
                *payload,
                *status,
                *device_writes,
                *payload_len,
            )?;
        }
        // plans/M7.md item E3/E4 / decision 16/20: sealed write order with
        // real DRAM stores against pool-backed addresses.
        Inst::QueuePublish {
            dst,
            queue,
            operation,
            steps: _,
        } => {
            emit_queue_publish(ctx, f, *dst, *queue, *operation)?;
        }
        Inst::QueueDrain { queue, max } => {
            emit_queue_drain(ctx, f, *queue, *max)?;
        }
        Inst::QueueSuppressInterrupts { queue } => {
            emit_queue_suppress_interrupts(ctx, f, *queue)?;
        }
        Inst::QueueClaim {
            dst,
            queue,
            receipt,
        } => {
            emit_queue_claim(ctx, f, *dst, *queue, *receipt)?;
        }
        Inst::DeviceReset { dst, device, queue } => {
            emit_device_reset(ctx, f, *dst, *device, *queue)?;
        }
    }
    Ok(())
}

/// Descriptor-table depth of a `VirtQueue[..N]` temp, or a named error.
fn virtqueue_depth_of(ty: &Type) -> Result<u16, CodegenError> {
    let Type::Named(name, targs) = ty else {
        return Err(CodegenError::internal(format!(
            "queue temp is `{}`, not `VirtQueue[..N]`",
            crate::sema::types::render_type(ty)
        )));
    };
    if name != "VirtQueue" {
        return Err(CodegenError::internal(format!(
            "queue temp is `{name}`, not `VirtQueue[..N]`"
        )));
    }
    let Some(crate::sema::types::TypeArg::Bound(expr)) = targs.first() else {
        return Err(CodegenError::internal(
            "`VirtQueue` with no bound depth".to_string(),
        ));
    };
    let text = match expr {
        crate::syntax::ast::Expr::Int(_, t) => t.as_str(),
        _ => {
            return Err(CodegenError::unimplemented(
                "`VirtQueue[..N]` whose depth is not an integer literal (const-name depths need \
                 folding before codegen)",
            ));
        }
    };
    let n: u64 = text.parse().map_err(|_| {
        CodegenError::internal(format!("`VirtQueue[..{text}]` depth is not a u64 literal"))
    })?;
    u16::try_from(n)
        .map_err(|_| CodegenError::internal(format!("`VirtQueue[..{n}]` depth does not fit u16")))
}

/// `prepare_block`: write header/status into packaging, record meta, mint
/// QueueOp = absolute meta address (decision 22). Ring head stays 0.
#[allow(clippy::too_many_arguments)]
fn emit_queue_prepare(
    ctx: &mut FnCtx,
    f: &MwirFn,
    dst: Temp,
    queue: Temp,
    header: Temp,
    payload: Temp,
    status: Temp,
    device_writes: bool,
    payload_len: u32,
) -> Result<(), CodegenError> {
    let depth = virtqueue_depth_of(&f.temp_types[queue.0])?;
    let placed = crate::virtqueue::place_ring(0, depth).ok_or_else(|| {
        CodegenError::internal(format!("place_ring(0, {depth}) refused a proven depth"))
    })?;
    let meta = crate::virtqueue::meta_offset(placed.bytes);
    let header_off = meta + crate::virtqueue::SLOT_META_BYTES;
    let status_off = header_off + crate::virtqueue::REQ_HEADER_SIZE;

    // X_C = pool base (queue word).
    ctx.load_slot(X_C, ctx.frame.off(queue));
    // X_D = header destination = pool + header_off.
    ctx.load_imm(X_D, header_off as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    // Pack BlkReqHeader: frame ABI is three 8-byte slots (kind, reserved,
    // sector); device layout is u32/u32/u64 at +0/+4/+8.
    let hdr_base = ctx.frame.off(header);
    ctx.load_slot(X_A, hdr_base); // kind
    ctx.push(
        encode::enc_str_w_imm(X_A, X_D, 0),
        format!("str w{}, [{}, #0]", X_A, reg_name(X_D)),
    );
    ctx.load_slot(X_A, hdr_base + 8); // reserved
    ctx.push(
        encode::enc_str_w_imm(X_A, X_D, 4),
        format!("str w{}, [{}, #4]", X_A, reg_name(X_D)),
    );
    ctx.load_slot(X_A, hdr_base + 16); // sector
    ctx.push(
        encode::enc_str_x_imm(X_A, X_D, 8),
        format!("str {}, [{}, #8]", reg_name(X_A), reg_name(X_D)),
    );

    // Status byte at pool + status_off.
    ctx.load_imm(X_D, status_off as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    ctx.load_slot(X_A, ctx.frame.off(status));
    ctx.push(
        encode::enc_strb_imm(X_A, X_D, 0),
        format!("strb w{}, [{}, #0]", X_A, reg_name(X_D)),
    );

    // Meta base = pool + meta.
    ctx.load_imm(X_D, meta as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    // payload addr
    ctx.load_slot(X_A, ctx.frame.off(payload));
    ctx.store_ptr(X_A, X_D, crate::virtqueue::SLOT_META_PAYLOAD as usize);
    // header addr
    ctx.load_imm(X_A, header_off as i64);
    ctx.push(
        encode::enc_add_reg(X_A, X_C, X_A, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_A),
            reg_name(X_C),
            reg_name(X_A)
        ),
    );
    ctx.store_ptr(X_A, X_D, crate::virtqueue::SLOT_META_HEADER as usize);
    // status addr
    ctx.load_imm(X_A, status_off as i64);
    ctx.push(
        encode::enc_add_reg(X_A, X_C, X_A, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_A),
            reg_name(X_C),
            reg_name(X_A)
        ),
    );
    ctx.store_ptr(X_A, X_D, crate::virtqueue::SLOT_META_STATUS as usize);
    // payload_len
    ctx.load_imm(X_A, payload_len as i64);
    ctx.store_ptr(X_A, X_D, crate::virtqueue::SLOT_META_PAYLOAD_LEN as usize);
    // flags = DEVICE_WRITES? | INFLIGHT (RESOLVED cleared)
    let flags = crate::virtqueue::SLOT_FLAG_INFLIGHT
        | if device_writes {
            crate::virtqueue::SLOT_FLAG_DEVICE_WRITES
        } else {
            0
        };
    ctx.load_imm(X_A, flags as i64);
    ctx.store_ptr(X_A, X_D, crate::virtqueue::SLOT_META_FLAGS as usize);
    // Stamp the queue's live reset epoch into the slot (plans/M7.md item H2b).
    // X_C still holds the pool base from above.
    ctx.load_imm(
        X_A,
        (placed.bytes + crate::virtqueue::SLOT_BOOK_EPOCH) as i64,
    );
    ctx.push(
        encode::enc_add_reg(X_A, X_C, X_A, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_A),
            reg_name(X_C),
            reg_name(X_A)
        ),
    );
    ctx.load_ptr(X_A, X_A, 0);
    ctx.store_ptr(X_A, X_D, crate::virtqueue::SLOT_META_EPOCH as usize);
    // Clear waiter / reply_stage for a fresh op.
    ctx.store_ptr(X_ZR, X_D, crate::virtqueue::SLOT_META_WAITER as usize);
    ctx.store_ptr(X_ZR, X_D, crate::virtqueue::SLOT_META_REPLY_STAGE as usize);
    // QueueOp = absolute meta address (X_D).
    ctx.store_slot(X_D, ctx.frame.off(dst));
    Ok(())
}

/// `publish`: write_descriptors → publish_available → notify_queue.
fn emit_queue_publish(
    ctx: &mut FnCtx,
    f: &MwirFn,
    dst: Temp,
    queue: Temp,
    operation: Temp,
) -> Result<(), CodegenError> {
    let depth = virtqueue_depth_of(&f.temp_types[queue.0])?;
    let placed = crate::virtqueue::place_ring(0, depth).ok_or_else(|| {
        CodegenError::internal(format!("place_ring(0, {depth}) refused a proven depth"))
    })?;
    let meta = crate::virtqueue::meta_offset(placed.bytes);
    // X_C = pool base; X_D = meta (QueueOp word — absolute address).
    ctx.load_slot(X_C, ctx.frame.off(queue));
    ctx.load_slot(X_D, ctx.frame.off(operation));

    // Load packaging.
    ctx.load_ptr(X_A, X_D, crate::virtqueue::SLOT_META_HEADER as usize); // header addr → will reuse
    // Spill header/payload/status/len/flags into frame-adjacent? Use
    // registers carefully: after each desc write we reload from meta.
    // Desc 0 (header): NEXT, device-readable, len=16, next=1
    // X_A already header addr. Write desc[0].
    emit_desc_entry(
        ctx,
        /*pool*/ X_C,
        /*desc_index*/ 0,
        /*addr_reg*/ X_A,
        /*len*/ crate::virtqueue::REQ_HEADER_SIZE as u32,
        /*flags*/ crate::virtqueue::DESC_F_NEXT,
        /*next*/ 1,
        placed.desc,
    )?;

    // Desc 1 (payload)
    ctx.load_ptr(X_A, X_D, crate::virtqueue::SLOT_META_PAYLOAD as usize);
    ctx.load_ptr(X_B, X_D, crate::virtqueue::SLOT_META_PAYLOAD_LEN as usize);
    ctx.load_ptr(0, X_D, crate::virtqueue::SLOT_META_FLAGS as usize); // flags → x0 temporarily
    // data flags: NEXT | (WRITE if device_writes)
    // Build flags in X_SCRATCH: start NEXT, OR WRITE if bit0 of meta flags.
    // Reload meta base — X_D still holds it if nothing clobbered it.
    // emit_desc_entry clobbers X_A/X_B/X_C? It uses pool reg — keep X_C as pool.
    // After first emit_desc, X_C should still be pool if emit_desc doesn't clobber.
    // Re-load pool and meta to be safe.
    ctx.load_slot(X_C, ctx.frame.off(queue));
    ctx.load_slot(X_D, ctx.frame.off(operation));
    ctx.load_ptr(X_A, X_D, crate::virtqueue::SLOT_META_PAYLOAD as usize);
    ctx.load_ptr(X_B, X_D, crate::virtqueue::SLOT_META_PAYLOAD_LEN as usize);
    let data_flags_base = crate::virtqueue::DESC_F_NEXT;
    // flags = NEXT | ((meta_flags & DEVICE_WRITES) << 1) — mask so INFLIGHT
    // does not become a spurious WRITE bit.
    ctx.load_ptr(0, X_D, crate::virtqueue::SLOT_META_FLAGS as usize);
    ctx.load_imm(1, crate::virtqueue::SLOT_FLAG_DEVICE_WRITES as i64);
    ctx.push(
        encode::enc_and_reg(0, 0, 1, true),
        format!("and {}, {}, {}", reg_name(0), reg_name(0), reg_name(1)),
    );
    ctx.push(
        encode::enc_lsl_imm(0, 0, 1, true),
        format!("lsl {}, {}, #1", reg_name(0), reg_name(0)),
    );
    ctx.load_imm(1, data_flags_base as i64);
    ctx.push(
        encode::enc_orr_reg(0, 0, 1, true),
        format!("orr {}, {}, {}", reg_name(0), reg_name(0), reg_name(1)),
    );
    // x0 = data flags, x1 = len (from X_B), xA = addr. emit_desc with len in reg?
    // Refactor emit_desc to take len as u32 immediate — payload_len is known
    // at prepare but publish reads it from meta. Use the register len.
    emit_desc_entry_len_reg(
        ctx,
        X_C,
        1,
        X_A,
        X_B, // len reg
        0,   // flags reg
        2,   // next
        placed.desc,
    )?;

    // Desc 2 (status): WRITE only, len=1, next=0
    ctx.load_slot(X_C, ctx.frame.off(queue));
    ctx.load_slot(X_D, ctx.frame.off(operation));
    let _ = meta; // geometry used for ring offsets below
    ctx.load_ptr(X_A, X_D, crate::virtqueue::SLOT_META_STATUS as usize);
    emit_desc_entry(
        ctx,
        X_C,
        2,
        X_A,
        crate::virtqueue::REQ_STATUS_SIZE as u32,
        crate::virtqueue::DESC_F_WRITE,
        0,
        placed.desc,
    )?;

    // publish_available: avail.ring[avail.idx % depth] = head (0); then
    // avail.idx++.
    ctx.load_slot(X_C, ctx.frame.off(queue));
    ctx.load_imm(X_D, placed.avail as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    // Load avail.idx (u16 at +2)
    ctx.push(
        encode::enc_ldrh_imm(X_A, X_D, 2),
        format!("ldrh w{}, [{}, #2]", X_A, reg_name(X_D)),
    );
    // slot = idx & (depth-1)
    ctx.load_imm(X_B, (depth as u64 - 1) as i64);
    ctx.push(
        encode::enc_and_reg(X_B, X_A, X_B, true),
        format!(
            "and {}, {}, {}",
            reg_name(X_B),
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    // ring entry addr = avail + 4 + 2*slot
    ctx.push(
        encode::enc_lsl_imm(X_B, X_B, 1, true),
        format!("lsl {}, {}, #1", reg_name(X_B), reg_name(X_B)),
    );
    ctx.load_imm(0, 4);
    ctx.push(
        encode::enc_add_reg(X_B, X_B, 0, true),
        format!("add {}, {}, {}", reg_name(X_B), reg_name(X_B), reg_name(0)),
    );
    ctx.push(
        encode::enc_add_reg(X_B, X_D, X_B, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_B),
            reg_name(X_D),
            reg_name(X_B)
        ),
    );
    // store head 0 as u16
    ctx.load_imm(0, 0);
    ctx.push(
        encode::enc_strh_imm(0, X_B, 0),
        format!("strh w{}, [{}, #0]", 0, reg_name(X_B)),
    );
    // avail.idx++
    ctx.load_imm(X_B, 1);
    ctx.push(
        encode::enc_add_reg(X_A, X_A, X_B, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_A),
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    ctx.push(
        encode::enc_strh_imm(X_A, X_D, 2),
        format!("strh w{}, [{}, #2]", X_A, reg_name(X_D)),
    );

    // notify_queue: store 1 to doorbell
    ctx.load_slot(X_C, ctx.frame.off(queue));
    ctx.load_imm(X_D, placed.doorbell as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    ctx.load_imm(X_A, 1);
    ctx.store_ptr(X_A, X_D, 0);

    // Receipt word = operation (meta absolute address).
    ctx.load_slot(X_A, ctx.frame.off(operation));
    ctx.store_slot(X_A, ctx.frame.off(dst));
    Ok(())
}

/// `suppress_interrupts`: store `VIRTQ_AVAIL_F_NO_INTERRUPT` into avail.flags.
fn emit_queue_suppress_interrupts(
    ctx: &mut FnCtx,
    f: &MwirFn,
    queue: Temp,
) -> Result<(), CodegenError> {
    let depth = virtqueue_depth_of(&f.temp_types[queue.0])?;
    let placed = crate::virtqueue::place_ring(0, depth).ok_or_else(|| {
        CodegenError::internal(format!("place_ring(0, {depth}) refused a proven depth"))
    })?;
    ctx.load_slot(X_C, ctx.frame.off(queue));
    ctx.load_imm(X_D, placed.avail as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    ctx.load_imm(X_A, crate::virtqueue::AVAIL_F_NO_INTERRUPT as i64);
    ctx.push(
        encode::enc_strh_imm(X_A, X_D, 0),
        format!("strh w{}, [{}, #0]", X_A, reg_name(X_D)),
    );
    Ok(())
}

/// Used-ring walk for one completion (single-flight). `max` is the source
/// bound; revision 0.1 never has more than one in flight, so one resolve
/// per call is the whole drain. Validation faults abort by name.
///
/// When the used ring is quiet, this emits one 06 §5 park (clock + short
/// deadline + `PARK_MMIO`) so the VMM's doorbell poll can run before the
/// bottom half claims — the same shape the hand-assembled blk conformance
/// guest uses after ringing the doorbell. A completion on that park
/// suppresses the sleep; an empty ring after the park returns without
/// resolving (claim then fails closed by name).
fn emit_queue_drain(
    ctx: &mut FnCtx,
    f: &MwirFn,
    queue: Temp,
    max: u16,
) -> Result<(), CodegenError> {
    let _ = max; // source bound; single-flight processes at most one
    let depth = virtqueue_depth_of(&f.temp_types[queue.0])?;
    let placed = crate::virtqueue::place_ring(0, depth).ok_or_else(|| {
        CodegenError::internal(format!("place_ring(0, {depth}) refused a proven depth"))
    })?;
    let meta_off = crate::virtqueue::meta_offset(placed.bytes);
    let comp_off = crate::virtqueue::completion_offset(placed.bytes);
    let book_off = placed.bytes; // last_used u64

    // X_C = pool
    ctx.load_slot(X_C, ctx.frame.off(queue));
    // X_D = book (last_used)
    ctx.load_imm(X_D, book_off as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    ctx.load_ptr(X_A, X_D, 0); // last_used
    // X_B = used.idx
    ctx.load_imm(X_E, placed.used as i64);
    ctx.push(
        encode::enc_add_reg(X_E, X_C, X_E, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_E),
            reg_name(X_C),
            reg_name(X_E)
        ),
    );
    ctx.push(
        encode::enc_ldrh_imm(X_B, X_E, 2),
        format!("ldrh w{}, [{}, #2]", X_B, reg_name(X_E)),
    );
    // pending = used_idx - last (both as u16 in low half)
    ctx.push(
        encode::enc_sub_reg(0, X_B, X_A, true),
        format!("sub {}, {}, {}", reg_name(0), reg_name(X_B), reg_name(X_A)),
    );
    // if pending != 0, skip the empty→park→recheck path
    let skip_empty = ctx.emit_skip(SkipKind::Cbnz(0));
    // Used ring quiet: yield once so the host can poll the doorbell
    // (06 §5; VMM blk conformance parks after publish for the same reason).
    emit_doorbell_poll_park(ctx);
    // Recheck used.idx vs last_used after the park.
    ctx.load_slot(X_C, ctx.frame.off(queue));
    ctx.load_imm(X_D, book_off as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    ctx.load_ptr(X_A, X_D, 0);
    ctx.load_imm(X_E, placed.used as i64);
    ctx.push(
        encode::enc_add_reg(X_E, X_C, X_E, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_E),
            reg_name(X_C),
            reg_name(X_E)
        ),
    );
    ctx.push(
        encode::enc_ldrh_imm(X_B, X_E, 2),
        format!("ldrh w{}, [{}, #2]", X_B, reg_name(X_E)),
    );
    ctx.push(
        encode::enc_sub_reg(0, X_B, X_A, true),
        format!("sub {}, {}, {}", reg_name(0), reg_name(X_B), reg_name(X_A)),
    );
    let skip_still_empty = ctx.emit_skip(SkipKind::Cbnz(0));
    let done_from_empty = ctx.emit_skip(SkipKind::Cond(Cond::Al));
    ctx.patch_skip(skip_still_empty, SkipKind::Cbnz(0));
    ctx.patch_skip(skip_empty, SkipKind::Cbnz(0));

    // slot = last & (depth-1)
    ctx.load_imm(X_B, (depth as u64 - 1) as i64);
    ctx.push(
        encode::enc_and_reg(X_B, X_A, X_B, true),
        format!(
            "and {}, {}, {}",
            reg_name(X_B),
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    // entry = used + 4 + slot*8 → X_F
    ctx.push(
        encode::enc_lsl_imm(X_B, X_B, 3, true),
        format!("lsl {}, {}, #3", reg_name(X_B), reg_name(X_B)),
    );
    ctx.load_imm(0, 4);
    ctx.push(
        encode::enc_add_reg(X_B, X_B, 0, true),
        format!("add {}, {}, {}", reg_name(X_B), reg_name(X_B), reg_name(0)),
    );
    ctx.push(
        encode::enc_add_reg(X_F, X_E, X_B, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_F),
            reg_name(X_E),
            reg_name(X_B)
        ),
    );
    // id (u32) → X_B, len (u32) → x0
    ctx.push(
        encode::enc_ldr_w_imm(X_B, X_F, 0),
        format!("ldr w{}, [{}, #0]", X_B, reg_name(X_F)),
    );
    ctx.push(
        encode::enc_ldr_w_imm(0, X_F, 4),
        format!("ldr w{}, [{}, #4]", 0, reg_name(X_F)),
    );

    // --- validate id (unknown / stale-by-epoch / duplicate) ---
    // Order matches `virtqueue::validate_completion_id` so a reset that
    // leaves INFLIGHT set still surfaces as StaleId, not DuplicateId.
    ctx.load_imm(X_F, crate::virtqueue::EXPECTED_HEAD as i64);
    ctx.push(
        encode::enc_cmp_reg(X_B, X_F, true),
        format!("cmp {}, {}", reg_name(X_B), reg_name(X_F)),
    );
    let id_ok = ctx.emit_skip(SkipKind::Cond(Cond::Eq));
    ctx.abort_fixed(crate::virtqueue::CompletionFault::UnknownId { id: 0 }.abort_message());
    ctx.patch_skip(id_ok, SkipKind::Cond(Cond::Eq));

    // meta → X_D; live epoch → X_F; stamped slot epoch → X_A
    ctx.load_slot(X_C, ctx.frame.off(queue));
    ctx.load_imm(X_D, meta_off as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    ctx.load_imm(X_F, (book_off + crate::virtqueue::SLOT_BOOK_EPOCH) as i64);
    ctx.push(
        encode::enc_add_reg(X_F, X_C, X_F, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_F),
            reg_name(X_C),
            reg_name(X_F)
        ),
    );
    ctx.load_ptr(X_F, X_F, 0); // current_epoch
    ctx.load_ptr(X_A, X_D, crate::virtqueue::SLOT_META_EPOCH as usize);
    ctx.push(
        encode::enc_cmp_reg(X_A, X_F, true),
        format!("cmp {}, {}", reg_name(X_A), reg_name(X_F)),
    );
    let epoch_ok = ctx.emit_skip(SkipKind::Cond(Cond::Eq));
    ctx.abort_fixed(
        crate::virtqueue::CompletionFault::StaleId {
            id: 0,
            slot_epoch: 0,
            current_epoch: 0,
        }
        .abort_message(),
    );
    ctx.patch_skip(epoch_ok, SkipKind::Cond(Cond::Eq));

    ctx.load_ptr(X_B, X_D, crate::virtqueue::SLOT_META_FLAGS as usize);
    ctx.load_imm(X_F, crate::virtqueue::SLOT_FLAG_INFLIGHT as i64);
    ctx.push(
        encode::enc_and_reg(X_F, X_B, X_F, true),
        format!(
            "and {}, {}, {}",
            reg_name(X_F),
            reg_name(X_B),
            reg_name(X_F)
        ),
    );
    let inflight_ok = ctx.emit_skip(SkipKind::Cbnz(X_F));
    ctx.abort_fixed(crate::virtqueue::CompletionFault::DuplicateId { id: 0 }.abort_message());
    ctx.patch_skip(inflight_ok, SkipKind::Cbnz(X_F));

    // --- length ---
    // x0 = used.len; payload_len in X_E; device_writes bit in X_F
    ctx.load_ptr(X_E, X_D, crate::virtqueue::SLOT_META_PAYLOAD_LEN as usize);
    ctx.load_imm(X_F, crate::virtqueue::SLOT_FLAG_DEVICE_WRITES as i64);
    ctx.push(
        encode::enc_and_reg(X_F, X_B, X_F, true),
        format!(
            "and {}, {}, {}",
            reg_name(X_F),
            reg_name(X_B),
            reg_name(X_F)
        ),
    );
    ctx.push(
        encode::enc_cmp_imm(0, 1, true),
        format!("cmp {}, #1", reg_name(0)),
    );
    let len_ge1 = ctx.emit_skip(SkipKind::Cond(Cond::Cs)); // used.len >= 1 (HS)
    ctx.abort_fixed(
        crate::virtqueue::CompletionFault::BadLength {
            reported: 0,
            capacity: 0,
        }
        .abort_message(),
    );
    ctx.patch_skip(len_ge1, SkipKind::Cond(Cond::Cs));
    // buffer_facing = used.len - 1 → X_A
    ctx.load_imm(X_A, 1);
    ctx.push(
        encode::enc_sub_reg(X_A, 0, X_A, true),
        format!("sub {}, {}, {}", reg_name(X_A), reg_name(0), reg_name(X_A)),
    );
    // if device_writes: buffer_facing <= payload_len; else buffer_facing == 0
    let is_write = ctx.emit_skip(SkipKind::Cbnz(X_F)); // skip OUT path when DEVICE_WRITES
    // OUT path
    ctx.push(
        encode::enc_cmp_imm(X_A, 0, true),
        format!("cmp {}, #0", reg_name(X_A)),
    );
    let out_ok = ctx.emit_skip(SkipKind::Cond(Cond::Eq));
    ctx.abort_fixed(
        crate::virtqueue::CompletionFault::BadLength {
            reported: 0,
            capacity: 0,
        }
        .abort_message(),
    );
    ctx.patch_skip(out_ok, SkipKind::Cond(Cond::Eq));
    let after_len = ctx.emit_skip(SkipKind::Cond(Cond::Al));
    ctx.patch_skip(is_write, SkipKind::Cbnz(X_F));
    // IN path: buffer_facing <= payload_len
    ctx.push(
        encode::enc_cmp_reg(X_A, X_E, true),
        format!("cmp {}, {}", reg_name(X_A), reg_name(X_E)),
    );
    let in_ok = ctx.emit_skip(SkipKind::Cond(Cond::Ls));
    ctx.abort_fixed(
        crate::virtqueue::CompletionFault::BadLength {
            reported: 0,
            capacity: 0,
        }
        .abort_message(),
    );
    ctx.patch_skip(in_ok, SkipKind::Cond(Cond::Ls));
    ctx.patch_skip(after_len, SkipKind::Cond(Cond::Al));

    // --- build IoCompletion in stash ---
    // Reload meta/pool; X_A still buffer_facing
    ctx.load_slot(X_C, ctx.frame.off(queue));
    ctx.load_imm(X_D, meta_off as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    // Spill buffer_facing to X_E
    ctx.push(
        encode::enc_mov_reg(X_E, X_A, true),
        format!("mov {}, {}", reg_name(X_E), reg_name(X_A)),
    );
    // status byte
    ctx.load_ptr(X_A, X_D, crate::virtqueue::SLOT_META_STATUS as usize);
    ctx.push(
        encode::enc_ldrb_imm(X_B, X_A, 0),
        format!("ldrb w{}, [{}, #0]", X_B, reg_name(X_A)),
    );
    // payload own handle
    ctx.load_ptr(X_F, X_D, crate::virtqueue::SLOT_META_PAYLOAD as usize);
    // comp base → X_A
    ctx.load_imm(X_A, comp_off as i64);
    ctx.push(
        encode::enc_add_reg(X_A, X_C, X_A, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_A),
            reg_name(X_C),
            reg_name(X_A)
        ),
    );
    ctx.store_ptr(X_F, X_A, 0); // payload
    // status Result: tag 0 if STATUS_OK else 1
    ctx.push(
        encode::enc_cmp_imm(X_B, 0, true),
        format!("cmp {}, #0", reg_name(X_B)),
    );
    ctx.load_imm(X_F, 0);
    ctx.load_imm(0, 1);
    ctx.push(
        encode::enc_csel(X_F, X_F, 0, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_F),
            reg_name(X_F),
            reg_name(0)
        ),
    );
    ctx.store_ptr(X_F, X_A, 8); // Result tag
    ctx.store_ptr(X_ZR, X_A, 16); // Ok(unit) / Err(OutOfRange=0)
    ctx.store_ptr(X_E, X_A, 24); // written_len

    // flags: clear INFLIGHT, set RESOLVED
    ctx.load_ptr(X_B, X_D, crate::virtqueue::SLOT_META_FLAGS as usize);
    ctx.load_imm(X_F, crate::virtqueue::SLOT_FLAG_INFLIGHT as i64);
    ctx.push(
        encode::enc_bic_reg(X_B, X_B, X_F, true),
        format!(
            "bic {}, {}, {}",
            reg_name(X_B),
            reg_name(X_B),
            reg_name(X_F)
        ),
    );
    ctx.load_imm(X_F, crate::virtqueue::SLOT_FLAG_RESOLVED as i64);
    ctx.push(
        encode::enc_orr_reg(X_B, X_B, X_F, true),
        format!(
            "orr {}, {}, {}",
            reg_name(X_B),
            reg_name(X_B),
            reg_name(X_F)
        ),
    );
    ctx.store_ptr(X_B, X_D, crate::virtqueue::SLOT_META_FLAGS as usize);

    // Copy to reply_stage if registered; wake waiter if registered.
    ctx.load_ptr(X_F, X_D, crate::virtqueue::SLOT_META_REPLY_STAGE as usize);
    let no_stage = ctx.emit_skip(SkipKind::Cbz(X_F));
    // copy 32 bytes X_A → X_F
    for w in [0usize, 8, 16, 24] {
        ctx.load_ptr(X_B, X_A, w);
        ctx.store_ptr(X_B, X_F, w);
    }
    ctx.patch_skip(no_stage, SkipKind::Cbz(X_F));

    ctx.load_ptr(X_F, X_D, crate::virtqueue::SLOT_META_WAITER as usize);
    let no_waiter = ctx.emit_skip(SkipKind::Cbz(X_F));
    ctx.load_imm(X_B, 1);
    ctx.push(
        encode::enc_str_x_imm(X_B, X_F, OFF_TURN_RESUME_READY as u16),
        format!(
            "str {}, [{}, #{OFF_TURN_RESUME_READY}]",
            reg_name(X_B),
            reg_name(X_F)
        ),
    );
    ctx.store_ptr(X_ZR, X_D, crate::virtqueue::SLOT_META_WAITER as usize);
    ctx.patch_skip(no_waiter, SkipKind::Cbz(X_F));

    // last_used++
    ctx.load_slot(X_C, ctx.frame.off(queue));
    ctx.load_imm(X_D, book_off as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    ctx.load_ptr(X_A, X_D, 0);
    ctx.load_imm(X_B, 1);
    ctx.push(
        encode::enc_add_reg(X_A, X_A, X_B, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_A),
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    ctx.store_ptr(X_A, X_D, 0);

    ctx.patch_skip(done_from_empty, SkipKind::Cond(Cond::Al));
    Ok(())
}

/// `RunningDevice.reset(queue=mut q)` (plans/M7.md item H2b / decision 23):
/// bump the queue's live epoch (fail closed on wrap), copy the device word
/// through. Does not reclaim DMA or clear the used ring — a completion
/// stamped with the prior epoch is `StaleId` on the next drain.
fn emit_device_reset(
    ctx: &mut FnCtx,
    f: &MwirFn,
    dst: Temp,
    device: Temp,
    queue: Temp,
) -> Result<(), CodegenError> {
    let depth = virtqueue_depth_of(&f.temp_types[queue.0])?;
    let placed = crate::virtqueue::place_ring(0, depth).ok_or_else(|| {
        CodegenError::internal(format!("place_ring(0, {depth}) refused a proven depth"))
    })?;
    let epoch_off = placed.bytes + crate::virtqueue::SLOT_BOOK_EPOCH;
    // X_C = pool; X_D = &current_epoch
    ctx.load_slot(X_C, ctx.frame.off(queue));
    ctx.load_imm(X_D, epoch_off as i64);
    ctx.push(
        encode::enc_add_reg(X_D, X_C, X_D, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_D),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    ctx.load_ptr(X_A, X_D, 0);
    // Exhaustion retires rather than wrap (03-hardware.md §4).
    ctx.load_imm(X_B, -1);
    ctx.push(
        encode::enc_cmp_reg(X_A, X_B, true),
        format!("cmp {}, {}", reg_name(X_A), reg_name(X_B)),
    );
    let not_max = ctx.emit_skip(SkipKind::Cond(Cond::Ne));
    ctx.abort_fixed(
        "driver fault: reset epoch exhausted (03-hardware.md §4: identities never wrap)",
    );
    ctx.patch_skip(not_max, SkipKind::Cond(Cond::Ne));
    ctx.load_imm(X_B, 1);
    ctx.push(
        encode::enc_add_reg(X_A, X_A, X_B, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_A),
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    ctx.store_ptr(X_A, X_D, 0);
    // Running -> Running: device word is unchanged (authority-only on v1).
    ctx.load_slot(X_A, ctx.frame.off(device));
    ctx.store_slot(X_A, ctx.frame.off(dst));
    Ok(())
}

/// One 06 §5 park so the VMM's doorbell poll can run: read the clock,
/// write `now + 20ms` to `OFF_NEXT_DEADLINE`, trap on `PARK_MMIO`. A blk
/// completion on that park suppresses the sleep (same numbers as the
/// hand-assembled conformance guest in `wrela-vmm`).
fn emit_doorbell_poll_park(ctx: &mut FnCtx) {
    ctx.load_imm(X_A, wrela_machine::mmio::CLOCK_MMIO_ADDR as i64);
    ctx.load_ptr(X_B, X_A, 0);
    ctx.load_imm(X_C, 20_000_000);
    ctx.push(
        encode::enc_add_reg(X_B, X_B, X_C, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_B),
            reg_name(X_B),
            reg_name(X_C)
        ),
    );
    let deadline_addr =
        wrela_machine::layout::MACHINE_INFO_BASE + wrela_machine::machine_info::OFF_NEXT_DEADLINE;
    ctx.load_imm(X_A, deadline_addr as i64);
    ctx.store_ptr(X_B, X_A, 0);
    ctx.load_imm(X_A, wrela_machine::mmio::PARK_MMIO_ADDR as i64);
    ctx.store_ptr(X_B, X_A, 0);
}

/// Sync claim of a drain-resolved receipt's IoCompletion stash (decision 22).
/// `receipt` holds the meta absolute address (same word publish minted).
fn emit_queue_claim(
    ctx: &mut FnCtx,
    f: &MwirFn,
    dst: Temp,
    queue: Temp,
    receipt: Temp,
) -> Result<(), CodegenError> {
    let _ = queue; // pool is recoverable from meta; kept for API symmetry
    let _ = f;
    // X_D = meta (receipt word)
    ctx.load_slot(X_D, ctx.frame.off(receipt));
    // flags must include RESOLVED
    ctx.load_ptr(X_A, X_D, crate::virtqueue::SLOT_META_FLAGS as usize);
    ctx.load_imm(X_B, crate::virtqueue::SLOT_FLAG_RESOLVED as i64);
    ctx.push(
        encode::enc_and_reg(X_A, X_A, X_B, true),
        format!(
            "and {}, {}, {}",
            reg_name(X_A),
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    let ok = ctx.emit_skip(SkipKind::Cbnz(X_A));
    ctx.abort_fixed("driver fault: claim of a receipt that is not RESOLVED");
    ctx.patch_skip(ok, SkipKind::Cbnz(X_A));
    // stash = meta + SLOT_META_BYTES + header + status pad
    let stash_delta = crate::virtqueue::SLOT_META_BYTES + crate::virtqueue::REQ_HEADER_SIZE + 8;
    ctx.load_imm(X_A, stash_delta as i64);
    ctx.push(
        encode::enc_add_reg(X_A, X_D, X_A, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_A),
            reg_name(X_D),
            reg_name(X_A)
        ),
    );
    // Copy 32-byte IoCompletion stash → dst
    let dst_off = ctx.frame.off(dst);
    for w in [0usize, 8, 16, 24] {
        ctx.load_ptr(X_B, X_A, w);
        ctx.store_slot(X_B, dst_off + w);
    }
    // Clear RESOLVED so a second claim aborts (single resolve).
    ctx.load_ptr(X_A, X_D, crate::virtqueue::SLOT_META_FLAGS as usize);
    ctx.load_imm(X_B, crate::virtqueue::SLOT_FLAG_RESOLVED as i64);
    ctx.push(
        encode::enc_bic_reg(X_A, X_A, X_B, true),
        format!(
            "bic {}, {}, {}",
            reg_name(X_A),
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    ctx.store_ptr(X_A, X_D, crate::virtqueue::SLOT_META_FLAGS as usize);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_desc_entry(
    ctx: &mut FnCtx,
    pool_reg: u8,
    desc_index: u16,
    addr_reg: u8,
    len: u32,
    flags: u16,
    next: u16,
    desc_base_off: u64,
) -> Result<(), CodegenError> {
    // desc_addr = pool + desc_base_off + index*16. Use X_TMP = 1 as scratch
    // for the descriptor entry pointer — but addr_reg might be X_A; pool X_C.
    // Scratch: use register 1 (X_B) only if addr_reg isn't X_B.
    let entry = if addr_reg == X_B { 0u8 } else { X_B };
    ctx.load_imm(entry, (desc_base_off + desc_index as u64 * 16) as i64);
    ctx.push(
        encode::enc_add_reg(entry, pool_reg, entry, true),
        format!(
            "add {}, {}, {}",
            reg_name(entry),
            reg_name(pool_reg),
            reg_name(entry)
        ),
    );
    ctx.push(
        encode::enc_str_x_imm(addr_reg, entry, 0),
        format!("str {}, [{}, #0]", reg_name(addr_reg), reg_name(entry)),
    );
    // len as u32 — materialize into a free reg
    let len_reg = if addr_reg == 0 { X_A } else { 0 };
    ctx.load_imm(len_reg, len as i64);
    ctx.push(
        encode::enc_str_w_imm(len_reg, entry, 8),
        format!("str w{}, [{}, #8]", len_reg, reg_name(entry)),
    );
    ctx.load_imm(len_reg, flags as i64);
    ctx.push(
        encode::enc_strh_imm(len_reg, entry, 12),
        format!("strh w{}, [{}, #12]", len_reg, reg_name(entry)),
    );
    ctx.load_imm(len_reg, next as i64);
    ctx.push(
        encode::enc_strh_imm(len_reg, entry, 14),
        format!("strh w{}, [{}, #14]", len_reg, reg_name(entry)),
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_desc_entry_len_reg(
    ctx: &mut FnCtx,
    pool_reg: u8,
    desc_index: u16,
    addr_reg: u8,
    len_reg: u8,
    flags_reg: u8,
    next: u16,
    desc_base_off: u64,
) -> Result<(), CodegenError> {
    // Find a scratch that isn't pool/addr/len/flags.
    let used = [pool_reg, addr_reg, len_reg, flags_reg];
    let entry = (0u8..8).find(|r| !used.contains(r)).ok_or_else(|| {
        CodegenError::internal("no scratch register for descriptor entry pointer".to_string())
    })?;
    ctx.load_imm(entry, (desc_base_off + desc_index as u64 * 16) as i64);
    ctx.push(
        encode::enc_add_reg(entry, pool_reg, entry, true),
        format!(
            "add {}, {}, {}",
            reg_name(entry),
            reg_name(pool_reg),
            reg_name(entry)
        ),
    );
    ctx.push(
        encode::enc_str_x_imm(addr_reg, entry, 0),
        format!("str {}, [{}, #0]", reg_name(addr_reg), reg_name(entry)),
    );
    ctx.push(
        encode::enc_str_w_imm(len_reg, entry, 8),
        format!("str w{}, [{}, #8]", len_reg, reg_name(entry)),
    );
    ctx.push(
        encode::enc_strh_imm(flags_reg, entry, 12),
        format!("strh w{}, [{}, #12]", flags_reg, reg_name(entry)),
    );
    let next_reg = (0u8..8)
        .find(|r| !used.contains(r) && *r != entry)
        .ok_or_else(|| CodegenError::internal("no scratch for desc next".to_string()))?;
    ctx.load_imm(next_reg, next as i64);
    ctx.push(
        encode::enc_strh_imm(next_reg, entry, 14),
        format!("strh w{}, [{}, #14]", next_reg, reg_name(entry)),
    );
    Ok(())
}

/// A register's transfer width in bytes, from its declared scalar type
/// alone (03-hardware.md §2: "The compiler and target ABI check width,
/// alignment, non-overlap, bounds, and endianness"). Fails closed on a
/// signed register (no sign-extending load is emitted, and silently
/// zero-extending one would be a wrong answer, not a missing feature) and
/// on an offset outside the unsigned-immediate encoder's scaled reach.
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
            write_back_self_skipping_interrupt_cells(f, frame, self_temp, ctx)?;
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
            );
            match kind {
                InterruptCellRmw::Swap => {
                    ctx.push(
                        encode::enc_stlr_w(X_B, X_A),
                        format!("stlr w{}, [{}]", X_B, reg_name(X_A)),
                    );
                }
                InterruptCellRmw::FetchOr => {
                    ctx.push(
                        encode::enc_orr_reg(X_D, X_C, X_B, false),
                        format!("orr w{}, w{}, w{}", X_D, X_C, X_B),
                    );
                    ctx.push(
                        encode::enc_stlr_w(X_D, X_A),
                        format!("stlr w{}, [{}]", X_D, reg_name(X_A)),
                    );
                }
            }
        }
        8 => {
            ctx.push(
                encode::enc_ldar_x(X_C, X_A),
                format!("ldar {}, [{}]", reg_name(X_C), reg_name(X_A)),
            );
            match kind {
                InterruptCellRmw::Swap => {
                    ctx.push(
                        encode::enc_stlr_x(X_B, X_A),
                        format!("stlr {}, [{}]", reg_name(X_B), reg_name(X_A)),
                    );
                }
                InterruptCellRmw::FetchOr => {
                    ctx.push(
                        encode::enc_orr_reg(X_D, X_C, X_B, true),
                        format!(
                            "orr {}, {}, {}",
                            reg_name(X_D),
                            reg_name(X_C),
                            reg_name(X_B)
                        ),
                    );
                    ctx.push(
                        encode::enc_stlr_x(X_D, X_A),
                        format!("stlr {}, [{}]", reg_name(X_D), reg_name(X_A)),
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

/// `mut self` write-back that leaves `InterruptCell` fields alone — the
/// live word is authoritative (ISR/ordinary ops already STLR'd it).
/// Writing the frame copy back would stomp an ISR update that landed at a
/// checkpoint during this turn.
fn write_back_self_skipping_interrupt_cells(
    f: &MwirFn,
    frame: &Frame,
    self_temp: Temp,
    ctx: &mut FnCtx,
) -> Result<(), CodegenError> {
    let self_ptr_off = frame
        .self_ptr_off
        .ok_or_else(|| CodegenError::internal("mut receiver but no self_ptr slot"))?;
    ctx.load_slot(X_A, self_ptr_off);
    let self_ty = &f.temp_types[self_temp.0];
    let Type::Named(name, targs) = strip_wrappers(self_ty) else {
        // Non-named receiver: fall back to whole-aggregate write-back.
        let size = frame.size_of_temp(self_temp);
        let src_off = frame.off(self_temp);
        let mut w = 0;
        while w < size {
            ctx.load_slot(X_B, src_off + w);
            ctx.store_ptr(X_B, X_A, w);
            w += 8;
        }
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
    let fields = ctx.layout.structs.get(&layout_key).ok_or_else(|| {
        CodegenError::internal(format!("unknown struct `{layout_key}` in layout ctx"))
    })?;
    let src_base = frame.off(self_temp);
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
                ctx.load_slot(X_B, src_base + off + w);
                ctx.store_ptr(X_B, X_A, off + w);
                w += 8;
            }
        }
        off += sz;
    }
    Ok(())
}

// --- per-fn driver: two passes, prologue length measured up front ----------

fn emit_fn(
    f: &MwirFn,
    layout: &LayoutCtx,
    rodata: &mut RodataPool,
) -> Result<CodegenFn, CodegenError> {
    // A sync fn never awaits, so it never stages a reply (0).
    let frame = build_frame(f, layout, 0)?;

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
        };
        if is_loop_back_edge(inst, i) {
            probe.checkpoint();
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
    };
    emit_prologue(f, &frame, &mut ctx)?;
    debug_assert_eq!(ctx.words.len(), prologue_len);
    for (i, inst) in f.body.iter().enumerate() {
        if is_loop_back_edge(inst, i) {
            ctx.checkpoint();
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
//      word-count sizing, `FnCtx::b_unconditional`/`cbz`, decision 6's own
//      loop-back-edge checkpoint test) is completely unaware it is looking
//      at a flattened multi-state program rather than an ordinary mwir
//      body; the exact same `is_loop_back_edge`-style position test
//      (`target flat index <= this flat index`) that already drives sync
//      fns' own checkpoints drives an async fn's `Transition::Jump`-shaped
//      loop back-edges too (`lower_while_split`'s own state-cycle shape),
//      with no new heuristic needed.
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

/// Whether `key` is a compiler-synthesized call target rather than a
/// source fn's own `CalleeKey`. The whole rule: a synthesized symbol
/// contains a character no wrela identifier may contain (a space), so
/// the two namespaces cannot overlap. Any future glue symbol must keep
/// that property — this fn is the one place to check it against.
pub fn symbol_is_synthetic(key: &str) -> bool {
    key.contains(' ')
}

/// The one place the symbol's own spelling lives, so `rt_enqueue_symbol`
/// and `rt_enqueue_actor` can never drift apart. The trailing space is
/// load-bearing (see `rt_enqueue_actor` above), not cosmetic.
const RT_ENQUEUE_PREFIX: &str = "rt_enqueue ";

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
//   +32 waker         the turn area address of whichever turn awaits
//                     THIS turn's completion (0 = none: a `send`, or the
//                     root). The waker is the suspended turn's identity:
//                     at M6 turn identity and turn-area address are in
//                     static bijection (one area per entity, all placed
//                     at build time), so the address itself is the
//                     dumbest correct representation — an index would
//                     need a runtime index->address table nothing else
//                     requires. Recorded in the ledger clause.
//   +40 cur_method    the in-flight method's dispatch index (actors
//                     only) — saved at fresh selection so the resume
//                     path can re-enter the same compiled method.
//   +48 reply_slot    plans/M7.md item Z1 (decision 9a): the address of
//                     THIS turn's own reply staging slot (`Frame::
//                     reply_stage_off`, an area-interior address) while
//                     it is parked on an actor `await` whose declared
//                     reply is an *aggregate* — the value the callee's
//                     dispatch hands its method in `x8`, this machine's
//                     aggregate-return-pointer register, so the callee
//                     writes its declared reply straight into the
//                     awaiting frame and nothing is copied at delivery.
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
pub const OFF_TURN_BUSY: u64 = 0;
pub const OFF_TURN_SUSPENDED: u64 = 8;
pub const OFF_TURN_RESUME_READY: u64 = 16;
pub const OFF_TURN_REPLY: u64 = 24;
pub const OFF_TURN_WAKER: u64 = 32;
pub const OFF_TURN_CUR_METHOD: u64 = 40;
pub const OFF_TURN_REPLY_SLOT: u64 = 48;
pub const TURN_RECORD_SIZE: u64 = 56;

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
/// A fixed, small bound on children per group (a disclosed floor, not a
/// hidden narrowing — plans/M6.md item F's own recorded reading of
/// decision 1's "starts may sit in loops": every required M6 golden opens
/// at most two `g.start` children in any one group; a third fails closed,
/// named, at `codegen::compute_group_child_indices`).
pub const GROUP_MAX_CHILDREN: usize = 2;
pub const GROUP_SLOT_SIZE: u64 = OFF_GROUP_CHILDREN_BASE + (GROUP_MAX_CHILDREN as u64) * 16;
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

pub fn group_child_tag_off(child_index: usize) -> u64 {
    OFF_GROUP_CHILDREN_BASE + (child_index as u64) * 16
}
pub fn group_child_payload_off(child_index: usize) -> u64 {
    group_child_tag_off(child_index) + 8
}

/// A rejected admission on an `await`'s own enqueue aborts (`BRK`) rather
/// than composing a real `CallError[NotAdmitted(..)]` value — the same
/// disclosed floor the nested-drain placeholder carried, unchanged by the
/// park-and-resume redesign; item G's send-proof/err-corpus work owns the
/// real composition. No required M6 conformance boot ever fills a mailbox
/// on an awaited call.
pub const BRK_AWAIT_ACTOR_REJECTED: u16 = 0xACD3;

/// plans/M6.md item F: an *actor* turn that reports `TURN_STATUS_CANCELLED`
/// to `rt_select_and_run`. Unreachable at M6 by construction (that routine's
/// own comment has the proof) and deliberately fail-closed rather than
/// approximated — the turn record has one scalar reply word and no error
/// channel, so there is nothing honest to deliver to the awaiting turn.
pub const BRK_ACTOR_TURN_CANCELLED: u16 = 0xACD5;

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
/// (`arena_capacity`, `layout::RuntimeTables::group_arena_capacity`) and
/// each `g.start`-able callee's own fixed child-slot ordinal
/// (`compute_group_child_indices`, below).
pub struct GroupCtx {
    pub arena_capacity: u64,
    pub child_index: BTreeMap<String, usize>,
}

/// `callee_key -> its own fixed child-slot ordinal` (0-based, within
/// whichever group starts it) — computed once, whole-program, by counting
/// each `FlowInst::GroupStart` in program order per `(owner fn,
/// group_temp)` pair. Two disclosed floors enforced here, named rather
/// than silently narrowed (module doc on `GROUP_MAX_CHILDREN`): more than
/// `GROUP_MAX_CHILDREN` children in one group scope, or the identical
/// callee named from more than one static `g.start` site anywhere in the
/// build (M6's one-free-turn-area-per-fn floor, `layout::RuntimeTables::
/// free_turns` — two concurrent instances of the same callee have nowhere
/// to live).
pub fn compute_group_child_indices(
    flow: &FlowWirProgram,
) -> Result<BTreeMap<String, usize>, CodegenError> {
    let mut out = BTreeMap::new();
    for (fn_key, f) in &flow.fns {
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
                    if this_idx >= GROUP_MAX_CHILDREN {
                        return Err(CodegenError::unimplemented(&format!(
                            "more than {GROUP_MAX_CHILDREN} `g.start` children in one group \
                             scope (fn `{fn_key}`, plans/M6.md item F's own disclosed floor)"
                        )));
                    }
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
    }
    Ok(out)
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

/// Builds the `Frame` for a FlowWir fn: `f.frame.temp_types` plus three
/// dedicated extra `u64` slots this file's own codegen needs beyond what
/// `flowwir_lower.rs` allocated — `state_temp` (the dispatch header's own
/// "which state" slot, module doc above) and a 2-word `arg_scratch`
/// buffer (`Send`/`Await{ActorCall}`'s own marshaling area: `rt_enqueue`'s
/// real ABI takes a *pointer* to a contiguous args blob, `layout.rs`'s own
/// module doc, not individual register values — and an async fn's own
/// `arg_temps` are ordinary, independently-allocated frame slots with no
/// guaranteed adjacency, so a small owned, always-contiguous scratch pair
/// is the dumbest correct marshaling area). Reuses `build_frame` verbatim
/// (never forked) via a synthetic `MwirFn` shape carrying exactly these
/// temps.
fn build_frame_flow(
    f: &FlowWirFn,
    layout: &LayoutCtx,
) -> Result<(Frame, Temp, Temp, Temp), CodegenError> {
    let mut temp_types = f.frame.temp_types.clone();
    let state_temp = Temp(temp_types.len());
    temp_types.push(Type::U64);
    let scratch0 = Temp(temp_types.len());
    temp_types.push(Type::U64);
    let scratch1 = Temp(temp_types.len());
    temp_types.push(Type::U64);
    let synthetic = MwirFn {
        receiver: f.receiver,
        params: f.params.clone(),
        ret: f.ret.clone(),
        temp_types,
        body: Vec::new(),
    };
    let frame = build_frame(&synthetic, layout, flow_reply_stage_size(f, layout)?)?;
    Ok((frame, state_temp, scratch0, scratch1))
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
    ctx.load_imm(X_FRAME, 0);
    // Overwrite the rendered text so the dump names the symbolic target
    // (the raw words stay the placeholder zeros layout patches).
    for (i, w) in ctx.words[word..word + 4].iter_mut().enumerate() {
        w.1 = format!("turn-frame[{i}] {} <{fn_key}>", reg_name(X_FRAME));
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
    );
    let fork = ctx.emit_skip(SkipKind::Cbnz(X_A));

    // --- fresh path: spill self/params into the persistent frame -------
    let mut next_reg = 0u8;
    if let Some((self_temp, _mode)) = f.receiver {
        let self_ptr_off = ctx
            .frame
            .self_ptr_off
            .ok_or_else(|| CodegenError::internal("receiver present but no self_ptr slot"))?;
        ctx.store_slot(next_reg, self_ptr_off);
        let size = ctx.frame.size_of_temp(self_temp);
        let dst_off = ctx.frame.off(self_temp);
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
        );
    }
    ctx.load_slot(X_A, ctx.frame.off(state_temp));
    for (i, &flat_idx) in resume_target.iter().enumerate() {
        ctx.push(
            encode::enc_cmp_imm(X_A, i as u16, true),
            format!("cmp {}, #{i}", reg_name(X_A)),
        );
        ctx.b_cond_to(Cond::Eq, flat_idx);
    }
    ctx.push(
        encode::enc_brk(BRK_ASYNC_DISPATCH_NO_STATE_MATCHED),
        format!("brk #{BRK_ASYNC_DISPATCH_NO_STATE_MATCHED:#x}"),
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
        );
    } else {
        ctx.push(encode::enc_mov_reg(1, 0, true), "mov x1, x0".to_string());
    }
    if let Some((self_temp, mode)) = f.receiver {
        if mode == AccessMode::Mut {
            // plans/M7.md item G, decision 17: same InterruptCell skip as
            // the sync epilogue — live cells are not frame-owned.
            write_back_self_skipping_interrupt_cells(f, ctx.frame, self_temp, ctx)?;
        }
    }
    ctx.load_imm(0, TURN_STATUS_COMPLETED as i64);
    ctx.load_slot(X_LR, ctx.frame.lr_off);
    ctx.push(encode::enc_ret(X_LR), "ret".to_string());
    Ok(())
}

impl FnCtx<'_> {
    /// `B.<cond>` to a flattened target position — `b_unconditional`/`cbz`'s
    /// own sibling for the dispatch header's own compare-and-branch chain.
    fn b_cond_to(&mut self, cond: Cond, target_flat_idx: usize) {
        let this_word = self.cur_word();
        let delta = self.branch_target_delta(target_flat_idx, this_word);
        self.push(
            encode::enc_b_cond(cond, delta),
            format!("b.{} #{delta}", cond_mnemonic(cond)),
        );
    }
}

/// Marshals `arg_temps` (at most 2, item C's own hand-assembled-dispatch
/// floor — `layout.rs`'s own module doc) into the dedicated scratch pair
/// and calls `symbol` — `rt_enqueue_<Actor>`'s own real ABI
/// (`x0=method_idx, x1=args_ptr, x2=nargs_words, x3=waker`), shared
/// verbatim by `Send` (waker = 0: one-way, nobody to resume, the sender
/// never suspends) and `Await{ActorCall}` (waker = this turn's own area
/// address, already live in `X_FRAME`).
fn emit_marshal_and_call(
    method_idx: usize,
    arg_temps: &[Temp],
    ctx: &mut FnCtx,
    symbol: &str,
    scratch0: Temp,
    scratch1: Temp,
    waker_is_self_turn: bool,
) -> Result<(), CodegenError> {
    if arg_temps.len() > 2 {
        return Err(CodegenError::unimplemented(
            "more than 2 scalar message args (item C's own hand-assembled mailbox-slot floor)",
        ));
    }
    let scratch_offs = [ctx.frame.off(scratch0), ctx.frame.off(scratch1)];
    for (i, t) in arg_temps.iter().enumerate() {
        ctx.load_slot(X_A, ctx.frame.off(*t));
        ctx.store_slot(X_A, scratch_offs[i]);
    }
    if !arg_temps.is_empty() {
        ctx.addr_of_slot(1, scratch_offs[0]);
    }
    ctx.load_imm(2, arg_temps.len() as i64);
    if waker_is_self_turn {
        ctx.push(
            encode::enc_mov_reg(3, X_FRAME, true),
            format!("mov x3, {}", reg_name(X_FRAME)),
        );
    } else {
        ctx.load_imm(3, 0);
    }
    ctx.load_imm(0, method_idx as i64);
    ctx.bl_symbolic_call(symbol);
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
/// `rt_enqueue` call, never a suspension. `dst` is filled with a minimal
/// `Result[unit,Rejected]`-shaped two-word value: tag = the real admission
/// outcome (`rt_enqueue`'s own `x0`, 0 admitted/1 rejected), payload
/// zeroed. **Disclosed floor, not silently narrowed**: composing a real
/// `Rejected[..]` payload (which sender, which reason) is item G's own
/// send-proof/err-corpus job — no required M6-D conformance case ever
/// fills a mailbox, so this path's own tag-only half is what actually
/// executes.
fn emit_send(
    dst: Temp,
    method_key: &str,
    arg_temps: &[Temp],
    ctx: &mut FnCtx,
    method_index: &ActorMethodIndex,
    scratch0: Temp,
    scratch1: Temp,
) -> Result<(), CodegenError> {
    let (actor, idx) = lookup_method_idx(method_key, method_index)?;
    emit_marshal_and_call(
        idx,
        arg_temps,
        ctx,
        &rt_enqueue_symbol(&actor),
        scratch0,
        scratch1,
        false, // one-way: no reply slot, no waker — the sender never suspends.
    )?;
    let dst_off = ctx.frame.off(dst);
    ctx.store_slot(0, dst_off); // x0 already holds rt_enqueue's own outcome.
    let dst_size = ctx.frame.size_of_temp(dst);
    if dst_size > 8 {
        ctx.load_imm(X_A, 0);
        let mut w = 8;
        while w < dst_size {
            ctx.store_slot(X_A, dst_off + w);
            w += 8;
        }
    }
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
fn emit_now(dst: Temp, ctx: &mut FnCtx) {
    ctx.load_imm(X_A, wrela_machine::mmio::CLOCK_MMIO_ADDR as i64);
    ctx.load_ptr(X_B, X_A, 0);
    ctx.store_slot(X_B, ctx.frame.off(dst));
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
) -> Result<(), CodegenError> {
    const X_ARENA: u8 = 15;
    const X_CAND: u8 = 16;
    const X_TAG: u8 = 17;

    let word = ctx.cur_word();
    ctx.load_imm(X_ARENA, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.1 = format!("group-arena-base {}", reg_name(X_ARENA));
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
    ctx.push(
        encode::enc_cmp_imm(X_B, 0, true),
        format!("cmp {}, #0", reg_name(X_B)),
    );
    ctx.push(
        encode::enc_csel(X_E, X_D, X_B, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_E),
            reg_name(X_D),
            reg_name(X_B)
        ),
    );
    ctx.push(
        encode::enc_cmp_imm(X_C, 0, true),
        format!("cmp {}, #0", reg_name(X_C)),
    );
    ctx.push(
        encode::enc_csel(X_F, X_D, X_C, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_F),
            reg_name(X_D),
            reg_name(X_C)
        ),
    );
    ctx.push(
        encode::enc_cmp_reg(X_E, X_F, true),
        format!("cmp {}, {}", reg_name(X_E), reg_name(X_F)),
    );
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
    ctx.push(
        encode::enc_csel(X_TAG, X_E, X_F, Cond::Ls, true),
        format!(
            "csel {}, {}, {}, ls",
            reg_name(X_TAG),
            reg_name(X_E),
            reg_name(X_F)
        ),
    );
    ctx.push(
        encode::enc_cmp_reg(X_TAG, X_D, true),
        format!("cmp {}, {}", reg_name(X_TAG), reg_name(X_D)),
    );
    ctx.push(
        encode::enc_csel(X_TAG, X_ZR, X_TAG, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_TAG),
            reg_name(X_ZR),
            reg_name(X_TAG)
        ),
    );
    // X_TAG now holds the effective (narrowed) deadline. Stash the old
    // ambient group (X_A) as the new group's parent before we clobber the
    // lineage slot — `parent_group = (old_ambient == 0) ? GROUP_NO_PARENT
    // : old_ambient - 1`.
    ctx.push(
        encode::enc_sub_imm(X_B, X_A, 1, true),
        format!("sub {}, {}, #1", reg_name(X_B), reg_name(X_A)),
    );
    ctx.load_imm(X_D, GROUP_NO_PARENT as i64);
    ctx.push(
        encode::enc_cmp_imm(X_A, 0, true),
        format!("cmp {}, #0", reg_name(X_A)),
    );
    ctx.push(
        encode::enc_csel(X_B, X_D, X_B, Cond::Eq, true),
        format!(
            "csel {}, {}, {}, eq",
            reg_name(X_B),
            reg_name(X_D),
            reg_name(X_B)
        ),
    );
    // X_B now holds parent_group.

    let mut to_after: Vec<usize> = Vec::new();
    for i in 0..gctx.arena_capacity {
        if i == 0 {
            ctx.push(
                encode::enc_add_imm(X_CAND, X_ARENA, 0, true),
                format!("add {}, {}, #0", reg_name(X_CAND), reg_name(X_ARENA)),
            );
        } else {
            ctx.load_imm(X_D, (i * GROUP_SLOT_SIZE) as i64);
            ctx.push(
                encode::enc_add_reg(X_CAND, X_ARENA, X_D, true),
                format!(
                    "add {}, {}, {}",
                    reg_name(X_CAND),
                    reg_name(X_ARENA),
                    reg_name(X_D)
                ),
            );
        }
        ctx.push(
            encode::enc_ldr_x_imm(X_D, X_CAND, OFF_GROUP_IN_USE as u16),
            format!(
                "ldr {}, [{}, #{OFF_GROUP_IN_USE}]",
                reg_name(X_D),
                reg_name(X_CAND)
            ),
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
        );
        for off in [
            OFF_GROUP_ACTIVE_CHILDREN,
            OFF_GROUP_CANCELLED,
            OFF_GROUP_JOIN_WAITER,
        ] {
            ctx.push(
                encode::enc_str_x_imm(X_ZR, X_CAND, off as u16),
                format!("str xzr, [{}, #{off}]", reg_name(X_CAND)),
            );
        }
        for c in 0..GROUP_MAX_CHILDREN {
            for off in [group_child_tag_off(c), group_child_payload_off(c)] {
                ctx.push(
                    encode::enc_str_x_imm(X_ZR, X_CAND, off as u16),
                    format!("str xzr, [{}, #{off}]", reg_name(X_CAND)),
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
        );
        ctx.push(
            encode::enc_str_x_imm(X_B, X_CAND, OFF_GROUP_PARENT as u16),
            format!(
                "str {}, [{}, #{OFF_GROUP_PARENT}]",
                reg_name(X_B),
                reg_name(X_CAND)
            ),
        );
        // The owning frame (02-language.md §9.5's own "parent"): this
        // turn's persistent area, which is exactly `X_FRAME`. Every
        // cancellation observation site compares against it to decide
        // whether a cancelled group terminates the observing activation (a
        // child started into the group) or merely hands it a `CallError`
        // (the `with`-block's own body).
        ctx.push(
            encode::enc_str_x_imm(X_FRAME, X_CAND, OFF_GROUP_OWNER_TURN as u16),
            format!(
                "str {}, [{}, #{OFF_GROUP_OWNER_TURN}]",
                reg_name(X_FRAME),
                reg_name(X_CAND)
            ),
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
        ctx.words.push((0, String::new()));
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
        ctx.words[j] = (encode::enc_b(delta), format!("b #{delta}"));
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
    emit_group_addr_from_temp(ctx, group_temp, X_B, X_A);
    ctx.push(
        encode::enc_ldr_x_imm(X_C, X_B, OFF_GROUP_CANCELLED as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_CANCELLED}]",
            reg_name(X_C),
            reg_name(X_B)
        ),
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
    );
    ctx.push(
        encode::enc_str_x_imm(X_ZR, X_B, group_child_payload_off(child_index) as u16),
        format!(
            "str xzr, [{}, #{}]",
            reg_name(X_B),
            group_child_payload_off(child_index)
        ),
    );
    let to_after = ctx.words.len();
    ctx.words.push((0, String::new()));
    ctx.patch_skip(skip_admit, SkipKind::Cbz(X_C));

    // Write the ambient lineage into the child's own persistent frame
    // (Temp(0)/Temp(1) — always the first two slots past the child's own
    // 48-byte turn record header) before ever calling it.
    let word = ctx.cur_word();
    ctx.load_imm(X_C, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.1 = format!("turn-frame[{}] {} <{callee_key}>", 0, reg_name(X_C));
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
    );
    for off in [OFF_TURN_SUSPENDED, OFF_TURN_RESUME_READY, OFF_TURN_WAKER] {
        ctx.push(
            encode::enc_str_x_imm(X_ZR, X_C, off as u16),
            format!("str xzr, [{}, #{off}]", reg_name(X_C)),
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
    );
    ctx.load_imm(X_F, GROUP_SLOT_SIZE as i64);
    ctx.push(
        encode::enc_mul(X_E, X_E, X_F, true),
        format!(
            "mul {}, {}, {}",
            reg_name(X_E),
            reg_name(X_E),
            reg_name(X_F)
        ),
    );
    let word = ctx.cur_word();
    ctx.load_imm(group_addr_reg, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.1 = "group-arena-base (g.start)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.push(
        encode::enc_add_reg(group_addr_reg, group_addr_reg, X_E, true),
        format!(
            "add {}, {}, {}",
            reg_name(group_addr_reg),
            reg_name(group_addr_reg),
            reg_name(X_E)
        ),
    );
    // active_children += 1 (admission).
    ctx.push(
        encode::enc_ldr_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
    );
    ctx.push(
        encode::enc_add_imm(X_A, X_A, 1, true),
        format!("add {}, {}, #1", reg_name(X_A), reg_name(X_A)),
    );
    ctx.push(
        encode::enc_str_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "str {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
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
    ctx.bl_symbolic_call(callee_key);
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
    ctx.load_imm(X_FRAME, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.1 = format!("turn-frame[{}] {} <{fn_key}>", 0, reg_name(X_FRAME));
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
    );
    ctx.load_imm(X_F, GROUP_SLOT_SIZE as i64);
    ctx.push(
        encode::enc_mul(X_E, X_E, X_F, true),
        format!(
            "mul {}, {}, {}",
            reg_name(X_E),
            reg_name(X_E),
            reg_name(X_F)
        ),
    );
    let word = ctx.cur_word();
    ctx.load_imm(group_addr_reg, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.1 = "group-arena-base (g.start harvest)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.push(
        encode::enc_add_reg(group_addr_reg, group_addr_reg, X_E, true),
        format!(
            "add {}, {}, {}",
            reg_name(group_addr_reg),
            reg_name(group_addr_reg),
            reg_name(X_E)
        ),
    );

    ctx.push(
        encode::enc_cmp_imm(0, TURN_STATUS_SUSPENDED as u16, true),
        format!("cmp x0, #{TURN_STATUS_SUSPENDED}"),
    );
    let skip_still_running = ctx.emit_skip(SkipKind::Cond(Cond::Eq)); // suspended: leave parked, nothing to harvest yet.

    // Completed or cancelled: tag = 0 (Ok) unless status ==
    // TURN_STATUS_CANCELLED, in which case tag = 1 (the composed
    // `CallError::Cancelled`, this item's own floor — module doc above).
    ctx.push(
        encode::enc_cmp_imm(0, TURN_STATUS_CANCELLED as u16, true),
        format!("cmp x0, #{TURN_STATUS_CANCELLED}"),
    );
    ctx.push(
        encode::enc_cset(X_A, Cond::Eq, true),
        format!("cset {}, eq", reg_name(X_A)),
    );
    ctx.push(
        encode::enc_str_x_imm(X_A, group_addr_reg, group_child_tag_off(child_index) as u16),
        format!(
            "str {}, [{}, #{}]",
            reg_name(X_A),
            reg_name(group_addr_reg),
            group_child_tag_off(child_index)
        ),
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
    );
    // Completed/cancelled (never suspended): decrement active_children —
    // this admission's own count is now settled — and clear this child's
    // own `busy` (harvested inline; available for a later loop iteration
    // of this same `g.start` site to reuse). A suspended child leaves both
    // untouched: still `busy`, still counted `active`, for
    // `layout::build_group_child_poll` to harvest later.
    ctx.push(
        encode::enc_ldr_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
    );
    ctx.push(
        encode::enc_sub_imm(X_A, X_A, 1, true),
        format!("sub {}, {}, #1", reg_name(X_A), reg_name(X_A)),
    );
    ctx.push(
        encode::enc_str_x_imm(X_A, group_addr_reg, OFF_GROUP_ACTIVE_CHILDREN as u16),
        format!(
            "str {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
            reg_name(X_A),
            reg_name(group_addr_reg)
        ),
    );
    let word = ctx.cur_word();
    ctx.load_imm(X_A, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.1 = format!("turn-frame[{}] {} <{callee_key}>", 0, reg_name(X_A));
    }
    ctx.relocs.push(Reloc::TurnFrameAddr {
        word,
        key: callee_key.to_string(),
    });
    ctx.push(
        encode::enc_str_x_imm(X_ZR, X_A, OFF_TURN_BUSY as u16),
        format!("str xzr, [{}, #{OFF_TURN_BUSY}]", reg_name(X_A)),
    );

    ctx.patch_skip(skip_still_running, SkipKind::Cond(Cond::Eq));
    let after = ctx.cur_word();
    let delta = (after as i64 - to_after as i64) as i32 * 4;
    ctx.words[to_after] = (encode::enc_b(delta), format!("b #{delta}"));
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
fn emit_group_close(group_temp: Temp, ctx: &mut FnCtx) -> Result<(), CodegenError> {
    let word = ctx.cur_word();
    ctx.load_imm(X_A, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.1 = "group-arena-base (GroupClose)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.load_slot(X_B, ctx.frame.off(group_temp)); // encoded group id (i+1)
    ctx.push(
        encode::enc_sub_imm(X_B, X_B, 1, true),
        format!("sub {}, {}, #1", reg_name(X_B), reg_name(X_B)),
    );
    ctx.load_imm(X_C, GROUP_SLOT_SIZE as i64);
    ctx.push(
        encode::enc_mul(X_B, X_B, X_C, true),
        format!(
            "mul {}, {}, {}",
            reg_name(X_B),
            reg_name(X_B),
            reg_name(X_C)
        ),
    );
    ctx.push(
        encode::enc_add_reg(X_A, X_A, X_B, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_A),
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    // Restore ambient lineage from this group's own `parent_group`.
    ctx.push(
        encode::enc_ldr_x_imm(X_B, X_A, OFF_GROUP_PARENT as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_PARENT}]",
            reg_name(X_B),
            reg_name(X_A)
        ),
    );
    ctx.load_imm(X_C, GROUP_NO_PARENT as i64);
    ctx.push(
        encode::enc_cmp_reg(X_B, X_C, true),
        format!("cmp {}, {}", reg_name(X_B), reg_name(X_C)),
    );
    let skip_no_parent = ctx.emit_skip(SkipKind::Cond(Cond::Eq)); // == GROUP_NO_PARENT -> no-parent arm

    // Had a parent: new ambient group = parent_index + 1; new ambient
    // deadline = the parent slot's own (already-narrowed) deadline.
    ctx.push(
        encode::enc_add_imm(X_B, X_B, 1, true),
        format!("add {}, {}, #1", reg_name(X_B), reg_name(X_B)),
    );
    ctx.store_slot(X_B, ctx.frame.off(LINEAGE_GROUP_SLOT));
    ctx.push(
        encode::enc_sub_imm(X_C, X_B, 1, true),
        format!("sub {}, {}, #1", reg_name(X_C), reg_name(X_B)),
    );
    ctx.load_imm(X_D, GROUP_SLOT_SIZE as i64);
    ctx.push(
        encode::enc_mul(X_C, X_C, X_D, true),
        format!(
            "mul {}, {}, {}",
            reg_name(X_C),
            reg_name(X_C),
            reg_name(X_D)
        ),
    );
    let word2 = ctx.cur_word();
    ctx.load_imm(X_D, 0);
    for w in ctx.words[word2..word2 + 4].iter_mut() {
        w.1 = "group-arena-base (GroupClose parent deadline)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word: word2 });
    ctx.push(
        encode::enc_add_reg(X_C, X_D, X_C, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_C),
            reg_name(X_D),
            reg_name(X_C)
        ),
    );
    ctx.push(
        encode::enc_ldr_x_imm(X_D, X_C, OFF_GROUP_DEADLINE as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_DEADLINE}]",
            reg_name(X_D),
            reg_name(X_C)
        ),
    );
    ctx.store_slot(X_D, ctx.frame.off(LINEAGE_DEADLINE_SLOT));
    let to_free = ctx.cur_word();
    ctx.words.push((0, String::new()));

    ctx.patch_skip(skip_no_parent, SkipKind::Cond(Cond::Eq));
    // No parent: ambient becomes "none" (0/0).
    ctx.store_slot(X_ZR, ctx.frame.off(LINEAGE_GROUP_SLOT));
    ctx.store_slot(X_ZR, ctx.frame.off(LINEAGE_DEADLINE_SLOT));

    // Both arms converge here: free the slot.
    let free = ctx.cur_word();
    let delta = (free as i64 - to_free as i64) as i32 * 4;
    ctx.words[to_free] = (encode::enc_b(delta), format!("b #{delta}"));
    ctx.push(
        encode::enc_str_x_imm(X_ZR, X_A, OFF_GROUP_IN_USE as u16),
        format!("str xzr, [{}, #{OFF_GROUP_IN_USE}]", reg_name(X_A)),
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
    scratch0: Temp,
    scratch1: Temp,
) -> Result<(), CodegenError> {
    match op {
        FlowInst::Mwir(inst) => emit_one(inst, f, ctx),
        FlowInst::SelfPath { dst, path } => emit_self_path(*dst, path, f, ctx),
        FlowInst::Now { dst } => {
            emit_now(*dst, ctx);
            Ok(())
        }
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
            ctx.push(
                encode::enc_mul(X_A, X_A, X_B, true),
                format!(
                    "mul {}, {}, {}",
                    reg_name(X_A),
                    reg_name(X_A),
                    reg_name(X_B)
                ),
            );
            ctx.store_slot(X_A, ctx.frame.off(*dst));
            Ok(())
        }
        FlowInst::Send {
            dst,
            target: _,
            method_key,
            arg_temps,
        } => emit_send(
            *dst,
            method_key,
            arg_temps,
            ctx,
            method_index,
            scratch0,
            scratch1,
        ),
        FlowInst::GroupCreate {
            group_temp,
            capacity,
            deadline,
        } => emit_group_create(*group_temp, *capacity, *deadline, ctx, gctx),
        FlowInst::GroupStart {
            group_temp,
            callee_key,
            arg_temps,
        } => emit_group_start(*group_temp, callee_key, arg_temps, ctx, gctx, fn_key),
        FlowInst::GroupClose { group_temp, .. } => emit_group_close(*group_temp, ctx),
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
/// - `X_D = 1` iff that same group's `owner_turn` is this turn's own
///   persistent area (`X_FRAME`), else `0` — the child-vs-owner
///   distinction `OFF_GROUP_OWNER_TURN`'s own doc comment explains.
///
/// Clobbers `X_A`/`X_B`/`X_E`. A no-op producing `X_C = X_D = 0` when the
/// whole build has no group arena at all, which is what keeps every
/// pre-item-F async golden byte-identical (`emit_checkpoint_cancellation_test`
/// below has the full reasoning); callers must not emit it in that case.
fn emit_group_cancelled_flags(ctx: &mut FnCtx) {
    ctx.push(
        encode::enc_movz(X_C, 0, 0, true),
        format!("movz {}, #0", reg_name(X_C)),
    );
    ctx.push(
        encode::enc_movz(X_D, 0, 0, true),
        format!("movz {}, #0", reg_name(X_D)),
    );
    ctx.load_slot(X_A, ctx.frame.off(LINEAGE_GROUP_SLOT));
    let skip_no_group = ctx.emit_skip(SkipKind::Cbz(X_A));
    let word = ctx.cur_word();
    ctx.load_imm(X_B, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.1 = "group-arena-base (cancel flags)".to_string();
    }
    ctx.relocs.push(Reloc::GroupArenaBase { word });
    ctx.push(
        encode::enc_sub_imm(X_A, X_A, 1, true),
        format!("sub {}, {}, #1", reg_name(X_A), reg_name(X_A)),
    );
    ctx.load_imm(X_E, GROUP_SLOT_SIZE as i64);
    ctx.push(
        encode::enc_mul(X_A, X_A, X_E, true),
        format!(
            "mul {}, {}, {}",
            reg_name(X_A),
            reg_name(X_A),
            reg_name(X_E)
        ),
    );
    ctx.push(
        encode::enc_add_reg(X_B, X_B, X_A, true),
        format!(
            "add {}, {}, {}",
            reg_name(X_B),
            reg_name(X_B),
            reg_name(X_A)
        ),
    );
    ctx.push(
        encode::enc_ldr_x_imm(X_A, X_B, OFF_GROUP_CANCELLED as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_CANCELLED}]",
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    ctx.push(
        encode::enc_cmp_imm(X_A, 0, true),
        format!("cmp {}, #0", reg_name(X_A)),
    );
    ctx.push(
        encode::enc_cset(X_C, Cond::Ne, true),
        format!("cset {}, ne", reg_name(X_C)),
    );
    ctx.push(
        encode::enc_ldr_x_imm(X_A, X_B, OFF_GROUP_OWNER_TURN as u16),
        format!(
            "ldr {}, [{}, #{OFF_GROUP_OWNER_TURN}]",
            reg_name(X_A),
            reg_name(X_B)
        ),
    );
    ctx.push(
        encode::enc_cmp_reg(X_A, X_FRAME, true),
        format!("cmp {}, {}", reg_name(X_A), reg_name(X_FRAME)),
    );
    ctx.push(
        encode::enc_cset(X_D, Cond::Eq, true),
        format!("cset {}, eq", reg_name(X_D)),
    );
    ctx.patch_skip(skip_no_group, SkipKind::Cbz(X_A));
}

fn emit_checkpoint_cancellation_test(ctx: &mut FnCtx, gctx: &GroupCtx) {
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
    emit_group_cancelled_flags(ctx);
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
    ctx.load_imm(0, TURN_STATUS_CANCELLED as i64);
    ctx.load_slot(X_LR, ctx.frame.lr_off);
    ctx.push(encode::enc_ret(X_LR), "ret".to_string());
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
        );
        ctx.push(
            encode::enc_ldr_x_imm(VAL_PAYLOAD, group_reg, group_child_payload_off(c) as u16),
            format!(
                "ldr {}, [{}, #{}]",
                reg_name(VAL_PAYLOAD),
                reg_name(group_reg),
                group_child_payload_off(c)
            ),
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
        ctx.push(
            encode::enc_cmp_imm(VAL_TAG, 0, true),
            format!("cmp {}, #0", reg_name(VAL_TAG)),
        );
        ctx.push(
            encode::enc_csel(VAL_PAYLOAD, VAL_PAYLOAD, VAL_CONST, Cond::Eq, true),
            format!(
                "csel {}, {}, {}, eq",
                reg_name(VAL_PAYLOAD),
                reg_name(VAL_PAYLOAD),
                reg_name(VAL_CONST)
            ),
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
fn emit_group_addr_from_temp(ctx: &mut FnCtx, group_temp: Temp, dst_reg: u8, scratch_reg: u8) {
    let word = ctx.cur_word();
    ctx.load_imm(dst_reg, 0);
    for w in ctx.words[word..word + 4].iter_mut() {
        w.1 = "group-arena-base (join_all)".to_string();
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
    );
    // arena index -> byte offset (a real bug this golden's first real boot
    // caught: an earlier draft added the raw index to the arena base
    // instead of `index * GROUP_SLOT_SIZE`, invisible for arena index 0
    // alone since `0 * anything == 0`, wrong for any other slot).
    ctx.load_imm(X_D, GROUP_SLOT_SIZE as i64);
    ctx.push(
        encode::enc_mul(scratch_reg, scratch_reg, X_D, true),
        format!(
            "mul {}, {}, {}",
            reg_name(scratch_reg),
            reg_name(scratch_reg),
            reg_name(X_D)
        ),
    );
    ctx.push(
        encode::enc_add_reg(dst_reg, dst_reg, scratch_reg, true),
        format!(
            "add {}, {}, {}",
            reg_name(dst_reg),
            reg_name(dst_reg),
            reg_name(scratch_reg)
        ),
    );
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
    state_temp: Temp,
    scratch0: Temp,
    scratch1: Temp,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match what {
        AwaitKind::ActorCall {
            target_temp: _,
            method_key,
            arg_temps,
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
                ctx.addr_of_slot(X_A, stage_off);
                ctx.push(
                    encode::enc_str_x_imm(X_A, X_FRAME, OFF_TURN_REPLY_SLOT as u16),
                    format!(
                        "str {}, [{}, #{OFF_TURN_REPLY_SLOT}]",
                        reg_name(X_A),
                        reg_name(X_FRAME)
                    ),
                );
            }
            emit_marshal_and_call(
                idx,
                arg_temps,
                ctx,
                &rt_enqueue_symbol(&actor),
                scratch0,
                scratch1,
                true, // waker = this turn's own area (X_FRAME).
            )?;
            // A rejected admission aborts. plans/M6.md item H3: it used
            // to abort as a bare `BRK`, which is a real EL1 exception
            // into a vector table this machine never installs, so the
            // operator saw a raw `esr=... pc=0x200` dump and no hint of
            // the cause. Fail closed *legibly* instead — the ordinary
            // abort path already prints a message over the console ring
            // before halting, and this is a condition a plain program
            // can reach (fill a mailbox with consumed-`Result` sends,
            // then `await`). The real fix is 02 §9.4's
            // `CallError::NotAdmitted` composition, which does not exist
            // (see this arm's ledger note on
            // `actors.calls.callerror-composition`).
            let skip = ctx.emit_skip(SkipKind::Cbz(0));
            ctx.abort_fixed(&format!(
                "await rejected: `{actor}`'s mailbox was full (M6 does not compose CallError::NotAdmitted, so a full mailbox is fatal here)"
            ));
            ctx.patch_skip(skip, SkipKind::Cbz(0));
            // Park: suspended = 1, status = suspended, return to the
            // scheduler (the real park — control genuinely leaves this
            // fn; every other ready actor can now run).
            ctx.load_imm(X_A, 1);
            ctx.push(
                encode::enc_str_x_imm(X_A, X_FRAME, OFF_TURN_SUSPENDED as u16),
                format!(
                    "str {}, [{}, #{OFF_TURN_SUSPENDED}]",
                    reg_name(X_A),
                    reg_name(X_FRAME)
                ),
            );
            ctx.load_imm(0, TURN_STATUS_SUSPENDED as i64);
            ctx.load_slot(X_LR, ctx.frame.lr_off);
            ctx.push(encode::enc_ret(X_LR), "ret".to_string());
            Ok(())
        }
        AwaitKind::GroupJoin {
            group_temp,
            child_count,
        } => {
            if *child_count > GROUP_MAX_CHILDREN {
                return Err(CodegenError::unimplemented(&format!(
                    "`g.join_all()` over more than {GROUP_MAX_CHILDREN} children (plans/M6.md \
                     item F's own disclosed floor)"
                )));
            }
            emit_group_addr_from_temp(ctx, *group_temp, X_B, X_A);
            ctx.push(
                encode::enc_ldr_x_imm(X_C, X_B, OFF_GROUP_ACTIVE_CHILDREN as u16),
                format!(
                    "ldr {}, [{}, #{OFF_GROUP_ACTIVE_CHILDREN}]",
                    reg_name(X_C),
                    reg_name(X_B)
                ),
            );
            let skip_park = ctx.emit_skip(SkipKind::Cbnz(X_C));
            // Immediate: every child already harvested — compose now and
            // fall straight through to the resume state, no scheduler
            // round-trip at all.
            emit_compose_group_join_result(ctx, X_B, result_temp, *child_count)?;
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx);
            ctx.b_unconditional(state_flat_base[resume_state]);
            ctx.patch_skip(skip_park, SkipKind::Cbnz(X_C));
            // Park for real: register as this group's own join waiter.
            ctx.push(
                encode::enc_str_x_imm(X_FRAME, X_B, OFF_GROUP_JOIN_WAITER as u16),
                format!(
                    "str {}, [{}, #{OFF_GROUP_JOIN_WAITER}]",
                    reg_name(X_FRAME),
                    reg_name(X_B)
                ),
            );
            ctx.load_imm(X_A, resume_state as i64);
            ctx.store_slot(X_A, ctx.frame.off(state_temp));
            ctx.load_imm(X_A, 1);
            ctx.push(
                encode::enc_str_x_imm(X_A, X_FRAME, OFF_TURN_SUSPENDED as u16),
                format!(
                    "str {}, [{}, #{OFF_TURN_SUSPENDED}]",
                    reg_name(X_A),
                    reg_name(X_FRAME)
                ),
            );
            ctx.load_imm(0, TURN_STATUS_SUSPENDED as i64);
            ctx.load_slot(X_LR, ctx.frame.lr_off);
            ctx.push(encode::enc_ret(X_LR), "ret".to_string());
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
            // Publish stage address, then waiter, then observe RESOLVED
            // (mask–arm–recheck against a drain that already finished).
            ctx.addr_of_slot(X_A, stage_off);
            ctx.load_slot(X_D, ctx.frame.off(*receipt_temp)); // meta
            ctx.store_ptr(X_A, X_D, crate::virtqueue::SLOT_META_REPLY_STAGE as usize);
            ctx.push(
                encode::enc_str_x_imm(X_FRAME, X_D, crate::virtqueue::SLOT_META_WAITER as u16),
                format!(
                    "str {}, [{}, #{}]",
                    reg_name(X_FRAME),
                    reg_name(X_D),
                    crate::virtqueue::SLOT_META_WAITER
                ),
            );
            ctx.load_ptr(X_A, X_D, crate::virtqueue::SLOT_META_FLAGS as usize);
            ctx.load_imm(X_B, crate::virtqueue::SLOT_FLAG_RESOLVED as i64);
            ctx.push(
                encode::enc_and_reg(X_A, X_A, X_B, true),
                format!(
                    "and {}, {}, {}",
                    reg_name(X_A),
                    reg_name(X_A),
                    reg_name(X_B)
                ),
            );
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
            ctx.push(
                encode::enc_add_reg(X_A, X_D, X_A, true),
                format!(
                    "add {}, {}, {}",
                    reg_name(X_A),
                    reg_name(X_D),
                    reg_name(X_A)
                ),
            );
            let result_off = ctx.frame.off(result_temp);
            let mut w = 0usize;
            while w < result_size {
                ctx.load_ptr(X_B, X_A, w);
                ctx.store_slot(X_B, result_off + w);
                w += 8;
            }
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx);
            ctx.b_unconditional(state_flat_base[resume_state]);
            ctx.patch_skip(need_park, SkipKind::Cbz(X_A));
            // Park until drain sets resume_ready.
            ctx.load_imm(X_A, 1);
            ctx.push(
                encode::enc_str_x_imm(X_A, X_FRAME, OFF_TURN_SUSPENDED as u16),
                format!(
                    "str {}, [{}, #{OFF_TURN_SUSPENDED}]",
                    reg_name(X_A),
                    reg_name(X_FRAME)
                ),
            );
            ctx.load_imm(0, TURN_STATUS_SUSPENDED as i64);
            ctx.load_slot(X_LR, ctx.frame.lr_off);
            ctx.push(encode::enc_ret(X_LR), "ret".to_string());
            let _ = (scratch0, scratch1);
            Ok(())
        }
    }
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

/// The resume half (module doc's step 3) — the dispatch chain's landing
/// site for `resume_state`: for `ActorCall`, compose `Ok(reply)` into
/// `result_temp` from the turn record's own reply slot; for `GroupJoin`
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
fn emit_await_resume(
    resume_state: usize,
    result_temp: Temp,
    what: &AwaitKind,
    f: &MwirFn,
    ctx: &mut FnCtx,
    gctx: &GroupCtx,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match what {
        AwaitKind::ActorCall { .. } => {
            // `result_temp`'s own type is always the composed
            // `Result[T, CallError[E]]` (02 §9.4's composition table,
            // `sema::bodies::compose_call_error`) — never the bare
            // declared reply.
            let composed_ty = &f.temp_types[result_temp.0];
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
                    emit_group_cancelled_flags(ctx);
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
            } else if gctx.arena_capacity == 0 {
                // No group exists anywhere in this build, so no await can
                // ever resolve `Cancelled` — emit exactly the pre-item-F
                // sequence, byte-identical (`emit_checkpoint_cancellation_test`'s
                // own reasoning).
                ctx.push(
                    encode::enc_ldr_x_imm(X_A, X_FRAME, OFF_TURN_REPLY as u16),
                    format!(
                        "ldr {}, [{}, #{OFF_TURN_REPLY}]",
                        reg_name(X_A),
                        reg_name(X_FRAME)
                    ),
                );
                ctx.store_slot(X_A, result_off + 8); // payload = the delivered reply
                ctx.load_imm(X_A, 0);
                ctx.store_slot(X_A, result_off); // tag = Ok
            } else {
                // 02-language.md §9.5: "Cancellation becomes observable at
                // `await` and checkpoints." An await inside a cancelled
                // group resolves `Err(CallError::Cancelled)`, not the reply
                // that happened to arrive — for the group's own owner this
                // is the ONLY way it ever observes the cancellation (its
                // frame is deliberately not terminated,
                // `OFF_GROUP_OWNER_TURN`'s own doc comment); for a child
                // the value is composed and then immediately discarded by
                // the termination test below, which is cheaper than
                // branching around it.
                emit_group_cancelled_flags(ctx);
                ctx.push(
                    encode::enc_ldr_x_imm(X_A, X_FRAME, OFF_TURN_REPLY as u16),
                    format!(
                        "ldr {}, [{}, #{OFF_TURN_REPLY}]",
                        reg_name(X_A),
                        reg_name(X_FRAME)
                    ),
                );
                ctx.load_imm(X_B, CALL_ERROR_TAG_CANCELLED as i64);
                ctx.push(
                    encode::enc_cmp_imm(X_C, 0, true),
                    format!("cmp {}, #0", reg_name(X_C)),
                );
                ctx.push(
                    encode::enc_csel(X_A, X_A, X_B, Cond::Eq, true),
                    format!(
                        "csel {}, {}, {}, eq",
                        reg_name(X_A),
                        reg_name(X_A),
                        reg_name(X_B)
                    ),
                );
                // `X_C` is already 0 (`Ok`) / 1 (`Err`) — the same encoding
                // the `Result` tag uses (`value::RESULT_OK`/`RESULT_ERR`).
                ctx.store_slot(X_C, result_off);
                ctx.store_slot(X_A, result_off + 8);
                let mut w = 16;
                while w < result_size {
                    ctx.store_slot(X_ZR, result_off + w);
                    w += 8;
                }
            }
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx);
            ctx.b_unconditional(state_flat_base[resume_state]);
            Ok(())
        }
        AwaitKind::GroupJoin {
            group_temp,
            child_count,
        } => {
            emit_group_addr_from_temp(ctx, *group_temp, X_B, X_A);
            emit_compose_group_join_result(ctx, X_B, result_temp, *child_count)?;
            ctx.checkpoint();
            emit_checkpoint_cancellation_test(ctx, gctx);
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
            emit_checkpoint_cancellation_test(ctx, gctx);
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
    state_temp: Temp,
    scratch0: Temp,
    scratch1: Temp,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match t {
        Transition::Return(value) => emit_one(&Inst::Return { value: *value }, f, ctx),
        Transition::Jump(target_state) => {
            let target_flat = state_flat_base[*target_state];
            // decision 6: every loop back-edge gets a checkpoint — a
            // `Transition::Jump` is only ever backward for a loop's own
            // state-cycle repeat (`flowwir_lower.rs`'s own
            // `lower_while_split`); the identical position test
            // (`is_loop_back_edge`) that drives a sync fn's back-edges
            // drives this one too. plans/M6.md item F: this back-edge is
            // also where a spinning turn's own cancellation is observed
            // (decision 7's flip witness — a deterministic iteration
            // count, never mid-instruction).
            if target_flat <= flat_idx {
                ctx.checkpoint();
                emit_checkpoint_cancellation_test(ctx, gctx);
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
            state_temp,
            scratch0,
            scratch1,
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
    scratch0: Temp,
    scratch1: Temp,
    state_flat_base: &[usize],
) -> Result<(), CodegenError> {
    match entry {
        FlatEntry::Op(op) => {
            emit_flow_op(op, f, ctx, method_index, gctx, fn_key, scratch0, scratch1)
        }
        FlatEntry::Trans(t) => emit_transition(
            t,
            flat_idx,
            f,
            ctx,
            method_index,
            gctx,
            state_temp,
            scratch0,
            scratch1,
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
    let (frame, state_temp, scratch0, scratch1) = build_frame_flow(f, layout)?;
    let (state_flat_base, resume_target, flat) = flatten(f);
    let total = flat.len();

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
        };
        emit_flat_entry(
            entry,
            i,
            &synthetic,
            &mut probe,
            method_index,
            gctx,
            fn_key,
            state_temp,
            scratch0,
            scratch1,
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
    };
    emit_async_entry(&synthetic, fn_key, &mut ctx, state_temp, &resume_target)?;
    debug_assert_eq!(ctx.words.len(), prologue_len);
    for (i, entry) in flat.iter().enumerate() {
        emit_flat_entry(
            entry,
            i,
            &synthetic,
            &mut ctx,
            method_index,
            gctx,
            fn_key,
            state_temp,
            scratch0,
            scratch1,
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

/// Every async fn's own persistent frame byte count (its `Frame::size` —
/// the statically reserved slots its activation lives in, past the
/// 48-byte turn record), keyed exactly like `FlowWirProgram::fns` — the
/// one fact `layout::compute_runtime_tables` needs from this module to
/// size each turn area, computed by the identical `build_frame_flow` the
/// real emission uses so the two can never disagree.
pub fn async_frame_sizes(
    flow: &FlowWirProgram,
    layout: &LayoutCtx,
) -> Result<BTreeMap<String, u64>, CodegenError> {
    let mut out = BTreeMap::new();
    for (key, f) in &flow.fns {
        let (frame, _, _, _) = build_frame_flow(f, layout)?;
        out.insert(key.clone(), frame.size as u64);
    }
    Ok(out)
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
) -> Result<CodegenProgram, CodegenError> {
    let mut rodata = RodataPool::new();
    rodata.seed(&mwir.rodata);
    let gctx = GroupCtx {
        arena_capacity: group_arena_capacity,
        child_index: compute_group_child_indices(flow)?,
    };
    let mut fns = BTreeMap::new();
    for (key, f) in &mwir.fns {
        fns.insert(key.clone(), emit_fn(f, layout, &mut rodata)?);
    }
    for (key, f) in &flow.fns {
        fns.insert(
            key.clone(),
            emit_flowwir_fn(key, f, layout, &mut rodata, method_index, &gctx)?,
        );
    }
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
                        || rt_enqueue_actor(target).is_some_and(|a| !a.is_empty());
                    if !resolvable {
                        return Err(format!(
                            "fn `{key}`: Reloc::Call targets `{target}`, which this \
                             `CodegenProgram` never codegen'd and which is not an \
                             `rt_enqueue` glue symbol either"
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
            }
        }
    }
    Ok(())
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
        let frame = build_frame(&f, &layout, 0).expect("build_frame");
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
        let frame = build_frame(&f, &layout, 0).expect("build_frame");
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
        let none = build_frame(&f, &layout, 0).expect("build_frame");
        assert_eq!(none.reply_stage_off, None);
        assert_eq!(none.lr_off, 8);
        assert_eq!(none.size, 16);
        let staged = build_frame(&f, &layout, 24).expect("build_frame");
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
        assert!(build_frame(&f, &layout, 0).is_err());
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
                code: vec![(0, String::new())],
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
                code: vec![(0, String::new())],
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
                code: vec![(0, String::new()), (0, String::new())],
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
                code: vec![(0, String::new())],
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
        // A source fn's key is its bare name (or `Struct.member`); none
        // can contain a space, so none can collide.
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
    }
}
