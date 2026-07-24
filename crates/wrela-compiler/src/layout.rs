//! Layout + emission (plans/M5.md item D): places a checked, lowered,
//! codegen'd program (`codegen::CodegenProgram`, item C's own output) into
//! the wrela machine's fixed memory contract (`wrela_machine::layout`, item
//! A) as **one flat blob**, loaded at `IMAGE_BASE` — no ELF, no linker, no
//! relocation format beyond this module's own four `codegen::Reloc`
//! variants. `wrela-machine`'s constants are consumed, never redefined; a
//! missing/wrong constant is this module's own bug to report, never a
//! contract this module is licensed to change (CLAUDE.md's "consume, don't
//! touch").
//!
//! ## The emitted blob's own section order (fixed, documented here once)
//!
//! ```text
//! IMAGE_BASE  entry   (see "Entry/abort contract" below)
//!             code    (every codegen'd fn's own words, `CodegenProgram::fns`'s
//!                      own `BTreeMap` key order, 4-byte aligned — always
//!                      true, every word is 4 bytes)
//!             rodata  (the codegen rodata pool's own bytes, concatenated in
//!                      order, 8-byte aligned at its own section base —
//!                      ABSENT when empty, per the no-placeholder rule: at
//!                      M5 this only happens for a program with no checked
//!                      arithmetic/panic/assert at all, since every abort
//!                      call interns its own message text here)
//!             data    (mutable globals — ALWAYS ABSENT at M5: module
//!                      `const`s are folded/immutable per `lower.rs`'s own
//!                      "every scalar const folds to a literal at its use
//!                      site" rule, so there is never anything to place —
//!                      recorded here as a fact, not a silently skipped
//!                      feature: nothing in this milestone's surface can
//!                      ever populate this section)
//!             abort   (the two abort-routine stubs, `__wrela_abort` then
//!                      `__wrela_abort_val`, 4-byte aligned — always
//!                      present: every codegen'd checked op names one of
//!                      these two symbols via `Reloc::AbortFixed`/
//!                      `Reloc::AbortVal`, whether or not any instance of
//!                      it is ever actually reached at runtime)
//! ```
//!
//! The report's own `Layout` section (`render_layout_section`, below) also
//! prints the two *fixed* machine regions below `IMAGE_BASE` (`pages` —
//! `wrela_machine::layout::MACHINE_INFO_BASE`'s own page plus the console
//! ring/data pages, one combined fact — and `stacks` — the four reserved
//! per-core stacks) even though this module never places anything there
//! itself (the VMM, item E, owns those pages' actual contents) — decision
//! 7's own "(code/rodata/data/stacks/pages)" enumeration named them as
//! report facts, not blob sections, and this module honors that by
//! reporting them from the same `wrela_machine` constants every build
//! shares, never by writing image bytes into that address range.
//!
//! ## Entry/abort contract (frozen here; item E replaces the *bodies*
//! below, never the *shape* — the exact obligation `codegen.rs`'s own
//! "abort contract" doc section names)
//!
//! - **Entry** = `IMAGE_BASE` (the blob's very first byte — the first,
//!   always-present section). vCPU 0 starts executing here (06-machine.md
//!   §3). At M5-D, since no test-running runtime exists yet (item E owns
//!   it), entry does exactly two things: (1) install core 0's own initial
//!   stack pointer (`sp = core_stack_base(0) + CORE_STACK_SIZE` —
//!   `wrela-machine`'s own documented convention for what every codegen'd
//!   fn's prologue already assumes is live on entry) via a `MOVZ`+3×`MOVK`
//!   materialize into a scratch register, then `ADD sp, Xn, #0` (the
//!   architectural `MOV SP, Xn` alias — `ADD (immediate)`'s `Rd`/`Rn`
//!   field `31` always denotes `SP`, never `XZR`, in this instruction
//!   class); (2) fall straight into the shared halt sequence below with
//!   its own documented placeholder exit code. Item E **replaces** this
//!   body wholesale with the real runtime driver (install SP, iterate
//!   every `@test(runtime)` fn, print its report line over the console
//!   ring, then halt) — the SP-install half will likely survive verbatim;
//!   the halt-immediately half is exactly what gets replaced.
//! - **`__wrela_abort`/`__wrela_abort_val`** (placed in the `abort`
//!   section, in that order): `codegen.rs`'s own abort ABI hands each its
//!   arguments in fixed registers (`x0`/`x1` for `__wrela_abort`'s
//!   `msg_ptr`/`msg_len`; `x0..x5` for `__wrela_abort_val`'s six-register
//!   form) — at M5-D, with no console ring writer yet (item E/decision 12
//!   own that), this module's own placeholder body does not move or
//!   inspect those registers at all: they are simply left exactly where
//!   the caller's `BL` already put them ("fixed scratch registers" — the
//!   abort info *is* the incoming registers, already in place, nothing
//!   further to store). Both stubs fall straight into the identical
//!   shared halt sequence, each with its own documented placeholder exit
//!   code (distinct from entry's, and from each other, purely so a future
//!   post-mortem memory dump can tell which path halted at a glance). Item
//!   E **keeps** this shape (both symbols still reached via the identical
//!   `BL`, still noreturn) and **extends** the body: print `x0..x5`'s own
//!   message over the console ring *before* the halt sequence, never
//!   instead of it — the exit-code-store-then-trap tail is expected to
//!   survive unchanged.
//! - **The shared halt sequence** (`push_halt`, below — used by entry and
//!   both abort stubs alike, one shape, no special-casing): materialize
//!   the exit code into a scratch register; store it to
//!   `machine_info::OFF_EXIT_CODE` (an ordinary, non-trapping store — so
//!   the value is visible in a plain guest memory dump even before any
//!   trap fires, mirroring `wrela-machine`'s own doc comment on that
//!   field exactly); store the same value to `mmio::EXIT_MMIO_ADDR` (the
//!   real "I'm done" signal — decision E's own exit protocol: this
//!   address is never backed by RAM, so the store necessarily takes a
//!   data-abort VMM exit); then `BRK #0`, a defensive terminal instruction
//!   in case execution ever continues past the trap (nothing traps on it
//!   yet at item D — no VMM runs this blob at all until item E — so this
//!   is pure defense-in-depth against a future host that resumes the
//!   guest after an unhandled MMIO exit instead of tearing it down).
//! - Scratch registers `x9`/`x10`/`x11` are used throughout entry/abort
//!   stub emission (`codegen.rs`'s own `x9..x14` scratch convention,
//!   reused here for consistency even though these stubs are never
//!   spill-everything code with the same frame): never `x0..x8` (the call
//!   ABI's own argument registers, which the abort stubs must leave
//!   untouched) and never `x29`/`x30`/`sp` (no frame pointer exists in
//!   this ABI at all, decision 4; `x30`/`sp` are not live across a `BL`
//!   that never returns, so nothing needs saving here either).
//!
//! ## Relocation resolution (the four `codegen::Reloc` variants, item C's
//! own fixups)
//!
//! `Reloc::Call{word,key}`/`Reloc::AbortFixed{word}`/`Reloc::AbortVal{word}`
//! all resolve the identical way: compute the byte delta from the `BL`
//! instruction's own placed address to the target symbol's own placed
//! address, and re-encode with `encode::enc_bl` — range-checked against
//! the imm26 encoder's own ±128 MiB reach *before* encoding (a real `Err`,
//! never a debug-only assertion someone could build past in release mode).
//! `Reloc::Rodata{word_adrp,byte_offset}` decodes its own live register
//! number directly out of the placeholder `ADRP` word's low 5 bits (both
//! `codegen.rs`'s placeholder `ADRP rd, #0` and the paired `ADD rd, rd,
//! #0` already carry the real register in that field — nothing else in
//! either placeholder word is non-zero), then re-encodes a real
//! page-relative `ADRP`+byte-offset `ADD` pair against the rodata
//! section's own now-known absolute base. Out-of-range in either
//! direction is `Err(LayoutError)`, never silently wrapped — nothing at
//! M5's image sizes can ever hit either limit, and the check exists so
//! that remains provably true rather than assumed.
//!
//! ## Section-size verification (`image.layout.sections-verified`'s own
//! teeth)
//!
//! `verify_section_sizes` re-derives, from the section table alone, that
//! every section starts where the previous one's own end (plus at most a
//! few bytes of alignment padding) says it should, and that the emitted
//! blob's own total length matches the last section's own end exactly —
//! a real, load-bearing assertion any future bug in this module's own
//! bookkeeping would trip, not a restatement of already-true arithmetic.

use std::collections::BTreeMap;

use crate::codegen::{CodegenProgram, Reloc};
use crate::encode;
use crate::eval::image::{ImageGraph, push_line};
use crate::mwir::{self, LayoutCtx};
use crate::sema::SemaError;
use crate::sema::typed::TypedProgram;
use crate::syntax::ast::Module;
use wrela_machine::{console, layout as machine_layout, machine_info, mmio};

// --- errors ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutError {
    pub message: String,
}

impl LayoutError {
    fn new(message: impl Into<String>) -> LayoutError {
        LayoutError {
            message: message.into(),
        }
    }
}

// --- output shape --------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: &'static str,
    pub base: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageLayout {
    /// The whole emitted blob, `blob[0]` corresponding to `IMAGE_BASE`.
    /// Deterministic, no timestamps/paths/host data anywhere in it
    /// (decision 10's own discipline — every byte here is a pure function
    /// of `program`'s own already-deterministic content).
    pub blob: Vec<u8>,
    /// Always `IMAGE_BASE` (the entry section's own base) — kept as its
    /// own field rather than re-derived by callers, matching decision 7's
    /// own separate `Entry base=0x...` report fact.
    pub entry: u64,
    /// `entry`/`code`/`rodata` (if nonempty)/`abort`/`rtdata` (if this
    /// image has actors — plans/M6.md item C), in ascending-base (=
    /// emission) order. `data` never appears (module doc: always empty).
    pub sections: Vec<Section>,
    /// Plans/M6.md item C, decision 3: this image's own static actor
    /// runtime-table sizing — `None` for an image with no actors at all
    /// (the no-placeholder rule: `render_layout_section` below emits no
    /// accounting lines when this is `None`), `Some` (even
    /// `RuntimeTables::actors` alone, never a fake empty one) the moment
    /// `ImageGraph::actors` is nonempty, build or test image alike (a
    /// build-only image with actors still gets the `rtdata` reservation
    /// and the report's own accounting facts — the plan's own "runtime
    /// tables are emitted for any image with actors, tests or not").
    pub runtime: Option<RuntimeTables>,
}

// --- scratch registers for stub emission (never x0..x8/x29/x30/sp) -----

const X_SP: u8 = 31;
const SCRATCH_A: u8 = 9;
const SCRATCH_B: u8 = 10;
const SCRATCH_C: u8 = 11;
/// The same bit pattern as `X_SP` (register field `31`), used only where
/// the instruction's own Rt/source-register position is meant — `STR`'s
/// own `Rt=11111` always denotes `XZR`, never `SP` (unlike `ADD`
/// (immediate)'s Rd/Rn field, where `31` means `SP`) — a separate name so
/// a reader never has to reason about which encoding class is in play at
/// each call site.
const X_ZR: u8 = 31;

/// Placeholder failure exit codes (module doc's own "Entry/abort
/// contract" section) — distinct only so a post-mortem guest memory dump
/// can tell, at a glance, which placeholder path halted; item E is
/// expected to keep writing *a* documented exit code at this same
/// `machine_info::OFF_EXIT_CODE` offset even once these bodies grow real
/// console output, whether or not it reuses these exact numeric values.
pub const EXIT_CODE_NO_RUNTIME: u64 = 0xE000_0001;
pub const EXIT_CODE_ABORT_FIXED: u64 = 0xE000_0002;
pub const EXIT_CODE_ABORT_VAL: u64 = 0xE000_0003;

fn push_load_imm(words: &mut Vec<u32>, reg: u8, value: i64) {
    let bits = value as u64;
    let h0 = (bits & 0xFFFF) as u16;
    let h1 = ((bits >> 16) & 0xFFFF) as u16;
    let h2 = ((bits >> 32) & 0xFFFF) as u16;
    let h3 = ((bits >> 48) & 0xFFFF) as u16;
    words.push(encode::enc_movz(reg, h0, 0, true));
    words.push(encode::enc_movk(reg, h1, 16, true));
    words.push(encode::enc_movk(reg, h2, 32, true));
    words.push(encode::enc_movk(reg, h3, 48, true));
}

/// The shared halt sequence every placeholder stub (entry, both abort
/// symbols) ends in — module doc's own "shared halt sequence" paragraph.
fn push_halt(words: &mut Vec<u32>, exit_code: u64) {
    push_load_imm(words, SCRATCH_A, exit_code as i64);
    let exit_code_addr = machine_layout::MACHINE_INFO_BASE + machine_info::OFF_EXIT_CODE;
    push_load_imm(words, SCRATCH_B, exit_code_addr as i64);
    words.push(encode::enc_str_x_imm(SCRATCH_A, SCRATCH_B, 0));
    push_load_imm(words, SCRATCH_C, mmio::EXIT_MMIO_ADDR as i64);
    words.push(encode::enc_str_x_imm(SCRATCH_A, SCRATCH_C, 0));
    words.push(encode::enc_brk(0));
}

fn build_entry_stub() -> Vec<u32> {
    let mut words = Vec::new();
    let sp_top = machine_layout::core_stack_base(0) + machine_layout::CORE_STACK_SIZE;
    push_load_imm(&mut words, SCRATCH_A, sp_top as i64);
    words.push(encode::enc_add_imm(X_SP, SCRATCH_A, 0, true)); // `mov sp, x9`
    push_halt(&mut words, EXIT_CODE_NO_RUNTIME);
    words
}

fn build_abort_stub(exit_code: u64) -> Vec<u32> {
    let mut words = Vec::new();
    push_halt(&mut words, exit_code);
    words
}

/// plans/M6.md item E, decision 7/06 §4: `__wrela_checkpoint_service` is
/// now real — the shared target every `codegen::Reloc::CheckpointService`
/// `BL` resolves to, in every image flavor (ordinary `wrela build`/
/// `--stage=report` and `wrela test`'s runtime harness alike; also the
/// entry driver's own park-resume path, below, calls it directly). Item
/// D's own bare `ret` is replaced by the real mask-arm-recheck loop over
/// the per-core pending word (`wrela_machine::pending::core_word_addr(0)`
/// — M6 is core-0-only, exactly like `codegen::FnCtx::checkpoint`'s own
/// identical address choice, which is the load-test half of this same
/// sequence: this fn is only ever reached once that test already found
/// the word nonzero).
///
/// **The vector table, honestly collapsed**: 06 §4/the plan's own item-E
/// task text calls for "a static vector table in rtdata" so a set bit
/// dispatches to its own registered service routine. At M6 there is
/// exactly one vector (index 0, the deadline/cancel vector) — a real
/// runtime-loaded table with one entry is pure ceremony over what a
/// single, directly-relocated `BL` to `__wrela_vector0_service` (emitted
/// immediately below, at a *compile-time-known* word offset — no
/// relocation across sections needed at all, since both routines are
/// built together, here, in one pass) already gives byte-for-byte:
/// exactly one link-time-resolved call target. CLAUDE.md's "no layers for
/// their own sake" governs over the plan's own literal wording here — a
/// table an M6 image can never populate with a second entry is not a
/// table, it is one `BL`, and building the indirection anyway before a
/// second vector exists would be exactly the "cleverness bought without
/// a profile" rule bars. **What multi-vector growth actually needs**
/// (disclosed, not built ahead of need): a real address table in rtdata
/// (or a fixed global data region, whichever a later milestone's own
/// static-sizing pass finds cleaner) plus a per-bit test-and-dispatch
/// loop in place of this fn's own single unconditional dispatch below —
/// the surrounding mask-arm-recheck loop shape does not change.
///
/// **Mask-arm-recheck, the M6-simple (single-core) form**: loop { read
/// the word; if zero, done; dispatch (the one vector); clear; reread
/// (recheck) }. Clearing is a plain whole-word zero-store, not a
/// bit-clear — honest only because bit 0 is the *only* bit any writer
/// ever sets at M6 (the VMM's own raise path, below, never sets another
/// bit); a real multi-vector version must AND-clear only the bit(s) just
/// serviced (this crate's `encode.rs` has no bitwise-not/BIC encoder yet,
/// deliberately not added for a floor nothing here needs). Rereading
/// after the clear-store is the actual "recheck": a raise landing
/// anywhere between our own read and our own clear-store (the VMM writes
/// from a different host thread, entirely unsynchronized with this loop)
/// is never lost — it is simply serviced on the loop's next iteration
/// rather than this one, so "arm" (there is nothing to separately
/// re-enable here — the pending word has no mask bit of its own, unlike
/// `InterruptCell`'s per-vector mask) collapses into "the loop always
/// rereads before deciding it is done." **What multi-core would
/// additionally need** (disclosed): this whole read-test-clear sequence
/// must become a single atomic RMW (`LDXR`/`STXR` or an atomic AND),
/// since a *different* vCPU's own checkpoint could race this one's clear
/// against the VMM's raise from yet another host thread — single-core
/// M6 has no such second reader/writer to race against, which is exactly
/// why a plain load/store loop is honestly sufficient here and would not
/// be once core 1+ start (a later milestone's own job, per the plan's own
/// "M6 is core-0-only").
///
/// Returns `(words, checkpoint_service_word_offset)` — the second value
/// is `__wrela_checkpoint_service`'s own entry point, *relative to the
/// start of `words`* (not `0`, since `__wrela_vector0_service` is placed
/// first so its own address needs no forward reference at all). Every
/// caller must resolve `Reloc::CheckpointService`/its own local `BL`s
/// against `section_base + checkpoint_service_word_offset * 4`, never
/// `section_base` alone.
///
/// **The vector-0 service routine's own contract** (item F's plug-in
/// point, named precisely so F never has to reshape this fn): called via
/// `BL` from the loop below, with the caller's own `x30` already saved —
/// a service routine may clobber `x0..x14` freely (checkpoints fire
/// between arbitrary instructions of interrupted code, so nothing about
/// a live register survives one anyway, by construction) but must
/// preserve `x28` (`codegen::X_FRAME`, the persistent turn-frame base
/// register live across suspension) and `sp`, and returns via its own
/// ordinary `ret` (its own `x30`, set fresh by the `BL`, never the
/// caller's). Clearing the pending bit is `__wrela_checkpoint_service`'s
/// own job, unconditionally, after every dispatch — never the routine's:
/// a vector routine's whole contract is "do the vector's work
/// synchronously, then return," exactly the shape item F's group-
/// cancellation delivery already needs (deliver cancellation to every
/// target the expired group names, then return).
pub fn build_checkpoint_and_vector_stub() -> (Vec<u32>, usize) {
    let mut a = Asm::new(0);

    // --- __wrela_vector0_service --- placed first: word offset 0, so the
    // checkpoint loop's own `BL` below needs no forward-reference bookkeeping.
    let vector0_start = a.abs();
    debug_assert_eq!(vector0_start, 0);
    let observed_addr = machine_layout::MACHINE_INFO_BASE + machine_info::OFF_VECTOR0_OBSERVED;
    a.load_imm(SCRATCH_A, observed_addr);
    a.push(encode::enc_ldr_x_imm(SCRATCH_B, SCRATCH_A, 0));
    a.push(encode::enc_add_imm(SCRATCH_B, SCRATCH_B, 1, true));
    a.push(encode::enc_str_x_imm(SCRATCH_B, SCRATCH_A, 0));
    a.push(encode::enc_ret(30));

    // --- __wrela_checkpoint_service ---
    let checkpoint_service_word = a.abs();
    a.push(encode::enc_sub_imm(X_SP, X_SP, 16, true)); // sub sp, sp, #16
    a.push(encode::enc_str_x_imm(30, X_SP, 0)); // str x30, [sp]  (BLR below clobbers it)
    let loop_top = a.abs();
    let pending_addr = wrela_machine::pending::core_word_addr(0);
    a.load_imm(SCRATCH_A, pending_addr);
    a.push(encode::enc_ldr_x_imm(SCRATCH_B, SCRATCH_A, 0));
    let skip_done = a.skip_placeholder(); // cbz X_B, .done
    a.bl_to(vector0_start);
    // Reload the address fresh — the callee's own contract (above) may
    // clobber any of x9..x14, so nothing from before the `BL` survives it.
    a.load_imm(SCRATCH_A, pending_addr);
    a.push(encode::enc_str_x_imm(X_ZR, SCRATCH_A, 0)); // clear (M6: whole-word == bit-0-only, module doc above)
    a.b_to(loop_top); // recheck
    let done = a.abs();
    a.patch_cbz(skip_done, SCRATCH_B);
    debug_assert_eq!(done, a.abs());
    a.push(encode::enc_ldr_x_imm(30, X_SP, 0));
    a.push(encode::enc_add_imm(X_SP, X_SP, 16, true));
    a.push(encode::enc_ret(30));

    (a.words, checkpoint_service_word)
}

// --- section packing helpers ---------------------------------------------

fn round_up(n: u64, align: u64) -> u64 {
    n.div_ceil(align) * align
}

fn pad_to(blob: &mut Vec<u8>, image_base: u64, target_addr: u64) {
    let want = (target_addr - image_base) as usize;
    debug_assert!(blob.len() <= want);
    while blob.len() < want {
        blob.push(0);
    }
}

// --- reloc resolution ------------------------------------------------------

/// `imm26`'s own signed byte range (`encode::word_offset`'s `bits=26`):
/// `half_range = 1 << (bits+1)` bytes either side of zero.
const BL_HALF_RANGE_BYTES: i64 = 1i64 << 27;

/// `imm21`'s own signed *page* range (`ADRP`'s 21-bit signed page count).
const ADRP_HALF_RANGE_PAGES: i64 = 1i64 << 20;

fn patch_bl(
    words: &mut [u32],
    idx: usize,
    this_addr: u64,
    target_addr: u64,
) -> Result<(), LayoutError> {
    let delta = target_addr as i64 - this_addr as i64;
    if delta <= -BL_HALF_RANGE_BYTES || delta >= BL_HALF_RANGE_BYTES {
        return Err(LayoutError::new(format!(
            "relocation out of range: a `BL` at {this_addr:#x} targets {target_addr:#x} \
             ({delta} bytes away) — outside the imm26 encoder's own +/-128 MiB reach"
        )));
    }
    words[idx] = encode::enc_bl(delta as i32);
    Ok(())
}

/// Patches the four-word `load_imm` starting at `word` (a
/// `Reloc::TurnFrameAddr` site — `MOVZ` + three `MOVK`s) with `value`,
/// preserving the destination register the emitter already encoded
/// (bits [4:0], identical in both instruction forms).
fn patch_load_imm_words(words: &mut [u32], word: usize, value: u64) {
    let rd = (words[word] & 0x1F) as u8;
    words[word] = encode::enc_movz(rd, (value & 0xFFFF) as u16, 0, true);
    words[word + 1] = encode::enc_movk(rd, ((value >> 16) & 0xFFFF) as u16, 16, true);
    words[word + 2] = encode::enc_movk(rd, ((value >> 32) & 0xFFFF) as u16, 32, true);
    words[word + 3] = encode::enc_movk(rd, ((value >> 48) & 0xFFFF) as u16, 48, true);
}

fn patch_adrp_add(
    words: &mut [u32],
    word_adrp: usize,
    this_addr: u64,
    target_addr: u64,
) -> Result<(), LayoutError> {
    let reg = (words[word_adrp] & 0x1F) as u8;
    let this_page = this_addr & !0xFFF;
    let target_page = target_addr & !0xFFF;
    let page_delta = (target_page as i64 - this_page as i64) / 4096;
    if page_delta < -ADRP_HALF_RANGE_PAGES || page_delta >= ADRP_HALF_RANGE_PAGES {
        return Err(LayoutError::new(format!(
            "relocation out of range: an `ADRP` at {this_addr:#x} targets page {target_page:#x} \
             ({page_delta} pages away) — outside the imm21 encoder's own range"
        )));
    }
    words[word_adrp] = encode::enc_adrp(reg, page_delta as i32);
    words[word_adrp + 1] = encode::enc_add_imm(reg, reg, (target_addr & 0xFFF) as u16, true);
    Ok(())
}

// --- section-size verification (image.layout.sections-verified's teeth) --

fn verify_section_sizes(
    sections: &[Section],
    image_base: u64,
    blob_len: u64,
) -> Result<(), LayoutError> {
    let Some(first) = sections.first() else {
        return Err(LayoutError::new(
            "internal error: an image with no sections at all",
        ));
    };
    if first.base != image_base {
        return Err(LayoutError::new(format!(
            "internal error: the first section `{}` does not start at IMAGE_BASE ({:#x} != {:#x})",
            first.name, first.base, image_base
        )));
    }
    for pair in sections.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let a_end = a.base + a.size;
        if a_end > b.base {
            return Err(LayoutError::new(format!(
                "internal error: section `{}` (ends {:#x}) overlaps section `{}` (starts {:#x})",
                a.name, a_end, b.name, b.base
            )));
        }
        // The only gaps this module ever inserts are alignment padding —
        // at most 7 bytes (the widest alignment used, rodata's 8-byte
        // rule). A larger gap means the section table and the actual
        // padding logic have drifted apart.
        if b.base - a_end >= 8 {
            return Err(LayoutError::new(format!(
                "internal error: a {}-byte gap between section `{}` and `{}` exceeds every \
                 alignment this module ever rounds to",
                b.base - a_end,
                a.name,
                b.name
            )));
        }
    }
    let last = sections.last().expect("checked non-empty above");
    let want_len = last.base + last.size - image_base;
    if blob_len != want_len {
        return Err(LayoutError::new(format!(
            "internal error: the emitted blob is {blob_len} bytes but the section table implies {want_len}"
        )));
    }
    Ok(())
}

// --- top-level entry: CodegenProgram -> ImageLayout -----------------------

/// Places `program` into the machine's fixed layout as one flat blob
/// (module doc's own section order), resolving every `Reloc`. `Err` only
/// for a genuine internal inconsistency (a call target codegen itself
/// never produced, an out-of-range relocation) — never for an ordinary
/// "this program doesn't lower" outcome, which is decided one layer up,
/// before this fn is ever called (see `try_layout_program`, below).
///
/// `runtime` (plans/M6.md item C): `Some(tables)` reserves one more
/// section, `rtdata`, sized exactly `tables.total_bytes` — zeroed,
/// uninitialized bytes, the same "no allocation, all sized at build time"
/// discipline every other section here already follows. Every existing
/// caller of this ordinary (non-test) build path passes `None` whenever
/// `ImageGraph::actors` is empty (the overwhelming majority of today's
/// corpus); an actor-bearing `wrela build`/`--stage=report` image still
/// never runs any of this milestone's runtime code (the placeholder entry
/// stub above is untouched) — the reservation exists because decision 3
/// says the tables are part of the image, tests or not, not because
/// anything here executes against them yet.
pub fn layout_program(
    program: &CodegenProgram,
    runtime: Option<&RuntimeTables>,
) -> Result<ImageLayout, LayoutError> {
    let image_base = machine_layout::IMAGE_BASE;

    let entry_words = build_entry_stub();

    let mut code_words: Vec<u32> = Vec::new();
    let mut fn_word_base: BTreeMap<String, usize> = BTreeMap::new();
    for (key, f) in &program.fns {
        fn_word_base.insert(key.clone(), code_words.len());
        for (w, _text) in &f.code {
            code_words.push(*w);
        }
    }

    let rodata_bytes: Vec<u8> = program
        .rodata
        .iter()
        .flat_map(|entry| entry.iter().copied())
        .collect();

    let abort_fixed_words = build_abort_stub(EXIT_CODE_ABORT_FIXED);
    let abort_val_words = build_abort_stub(EXIT_CODE_ABORT_VAL);
    let (checkpoint_words, checkpoint_service_word) = build_checkpoint_and_vector_stub();

    // --- place sections, fixed order: entry, code, rodata?, abort. ------
    let mut cursor = image_base;

    let entry_base = cursor;
    let entry_size = (entry_words.len() * 4) as u64;
    cursor += entry_size;

    cursor = round_up(cursor, 4);
    let code_base = cursor;
    let code_size = (code_words.len() * 4) as u64;
    cursor += code_size;

    let rodata_base = if rodata_bytes.is_empty() {
        None
    } else {
        cursor = round_up(cursor, 8);
        let base = cursor;
        cursor += rodata_bytes.len() as u64;
        Some(base)
    };

    cursor = round_up(cursor, 4);
    let abort_fixed_base = cursor;
    cursor += (abort_fixed_words.len() * 4) as u64;
    let abort_val_base = cursor;
    cursor += (abort_val_words.len() * 4) as u64;
    let abort_size = cursor - abort_fixed_base; // both abort stubs' combined byte length

    let checkpoint_base = cursor;
    let checkpoint_size = (checkpoint_words.len() * 4) as u64;
    cursor += checkpoint_size;
    // `__wrela_checkpoint_service`'s own entry point (module doc on
    // `build_checkpoint_and_vector_stub`): `__wrela_vector0_service` is
    // placed first in this section, so the section's own base is never
    // the right `Reloc::CheckpointService` target on its own.
    let checkpoint_service_addr = checkpoint_base + (checkpoint_service_word as u64) * 4;

    let mut sections = vec![
        Section {
            name: "entry",
            base: entry_base,
            size: entry_size,
        },
        Section {
            name: "code",
            base: code_base,
            size: code_size,
        },
    ];
    if let Some(rb) = rodata_base {
        sections.push(Section {
            name: "rodata",
            base: rb,
            size: rodata_bytes.len() as u64,
        });
    }
    sections.push(Section {
        name: "abort",
        base: abort_fixed_base,
        size: abort_size,
    });
    // plans/M6.md decision 6/item D: `__wrela_checkpoint_service`, its own
    // section (distinct from `abort` — a checkpoint's own service call is
    // never an abort) — every image flavor reserves it, even one whose own
    // reachable surface never emits a checkpoint (no per-program
    // conditionality here, mirroring `abort`'s own unconditional presence).
    sections.push(Section {
        name: "checkpoint",
        base: checkpoint_base,
        size: checkpoint_size,
    });

    // --- rtdata (plans/M6.md item C, decision 3): reserved, zeroed bytes
    // for this image's own static actor runtime tables — absent entirely
    // when `runtime` is `None` (no actors), never a zero-size placeholder
    // section. -------------------------------------------------------------
    let rtdata_base = if let Some(tables) = runtime.filter(|t| t.total_bytes > 0) {
        cursor = round_up(cursor, 8);
        let base = cursor;
        sections.push(Section {
            name: "rtdata",
            base,
            size: tables.total_bytes,
        });
        // `rtdata` is this image shape's own final section — nothing
        // consumes `cursor` past it, mirroring the identical rodata-base
        // convention a few lines above this fn's own test-image sibling.
        Some(base)
    } else {
        None
    };

    // --- resolve every Reloc against the now-known section bases --------
    let runtime_live = runtime.filter(|t| t.total_bytes > 0);
    let placement = match (rtdata_base, runtime_live) {
        (Some(base), Some(tables)) => Some(place_runtime_tables(base, tables)),
        _ => None,
    };
    let mut all_code_words = code_words;
    for (key, f) in &program.fns {
        let base = fn_word_base[key];
        for reloc in &f.relocs {
            match reloc {
                Reloc::Call { word, key: target } => {
                    let target_base = *fn_word_base.get(target).ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: call target `{target}` was never codegen'd"
                        ))
                    })?;
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = code_base + (target_base * 4) as u64;
                    patch_bl(&mut all_code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::Rodata {
                    word_adrp,
                    byte_offset,
                } => {
                    let rb = rodata_base.ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::Rodata exists but the rodata section is empty",
                        )
                    })?;
                    let this_addr = code_base + ((base + word_adrp) * 4) as u64;
                    let target_addr = rb + *byte_offset as u64;
                    patch_adrp_add(
                        &mut all_code_words,
                        base + word_adrp,
                        this_addr,
                        target_addr,
                    )?;
                }
                Reloc::AbortFixed { word } => {
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    patch_bl(
                        &mut all_code_words,
                        base + word,
                        this_addr,
                        abort_fixed_base,
                    )?;
                }
                Reloc::AbortVal { word } => {
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    patch_bl(&mut all_code_words, base + word, this_addr, abort_val_base)?;
                }
                Reloc::CheckpointService { word } => {
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    patch_bl(
                        &mut all_code_words,
                        base + word,
                        this_addr,
                        checkpoint_service_addr,
                    )?;
                }
                Reloc::TurnFrameAddr { word, key: fn_key } => {
                    let addr = placement
                        .as_ref()
                        .zip(runtime_live)
                        .and_then(|(p, t)| p.turn_area_for(fn_key, t))
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: async fn `{fn_key}` needs a turn area but this \
                                 image's runtime tables never sized one"
                            ))
                        })?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::GroupArenaBase { word } => {
                    let addr = placement.as_ref().map(|p| p.group_arena).ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a `with group` op needs the group arena but this \
                             image's runtime tables never sized one"
                                .to_string(),
                        )
                    })?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
            }
        }
    }

    // --- serialize -------------------------------------------------------
    let mut blob = Vec::new();
    for w in &entry_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    pad_to(&mut blob, image_base, code_base);
    for w in &all_code_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    if let Some(rb) = rodata_base {
        pad_to(&mut blob, image_base, rb);
        blob.extend_from_slice(&rodata_bytes);
    }
    pad_to(&mut blob, image_base, abort_fixed_base);
    for w in &abort_fixed_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    for w in &abort_val_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    pad_to(&mut blob, image_base, checkpoint_base);
    for w in &checkpoint_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    if let (Some(rb), Some(tables)) = (rtdata_base, runtime.filter(|t| t.total_bytes > 0)) {
        pad_to(&mut blob, image_base, rb);
        blob.resize(blob.len() + tables.total_bytes as usize, 0);
    }

    verify_section_sizes(&sections, image_base, blob.len() as u64)?;

    Ok(ImageLayout {
        blob,
        entry: entry_base,
        sections,
        runtime: runtime.cloned(),
    })
}

// --- whole-program orchestration (lower -> codegen -> layout) ------------

/// Merges one `mwir::LayoutCtx` per module in the build closure (project
/// cases place a spliced-in struct's own field-type declaration in a
/// *different* file than the one holding `@image` — `mwir::build_layout_ctx`
/// itself only ever sees one raw `ast::Module` at a time, module-local, so
/// a single module's own ctx is not enough whenever any struct/enum lives
/// outside the `@image`-owning file). Later modules win on an exact-name
/// collision (undisclosed generalization beyond what any of today's
/// goldens exercise — every real case here has module-unique struct/enum
/// names).
pub fn merge_layout_ctx(modules: &BTreeMap<String, Module>) -> Result<LayoutCtx, SemaError> {
    let mut merged = LayoutCtx::default();
    for module in modules.values() {
        let ctx = crate::mwir::build_layout_ctx(module)?;
        merged.structs.extend(ctx.structs);
        merged.enums.extend(ctx.enums);
        merged.struct_field_names.extend(ctx.struct_field_names);
    }
    Ok(merged)
}

/// Merges every module's own `lower::lower_program` output into one
/// `MwirProgram` — needed because `bodies::check` (the typed-tree
/// producer) only ever populates one module's own `TypedProgram::fns`/
/// `structs` from *that module's own* `ast::Module::items`
/// (`sema::mod.rs`'s import splice only ever grows `ModuleCtx`, the
/// name-resolution table used *while checking a body*, never the final
/// `TypedProgram` itself — so an imported struct's own methods live only
/// in the *declaring* module's own `TypedProgram`, never copied into an
/// importer's). A project's own reachable surface is consequently spread
/// across every module in the build closure, not just the one owning
/// `@image` — this fn lowers each module's own program independently and
/// concatenates the results. Rodata indices are rebased per module (each
/// module's own `MwirProgram::rodata` starts at index 0; `ConstText::data`
/// references inside that module's own fn bodies are shifted by the
/// running total already merged) so a merged `ConstText` still points at
/// the right bytes — dead code today (nothing in any current golden's
/// reachable surface uses `Static[Str]` at all, `codegen.rs`'s own
/// fail-closed list), kept correct anyway rather than assumed away.
/// `fns` keys are expected to be module-unique in practice (a struct name
/// is not currently checked for cross-module uniqueness anywhere in this
/// compiler); a same-spelling collision resolves last-module-wins, a
/// disclosed simplification no existing program can trigger.
fn merge_mwir_programs(programs: Vec<mwir::MwirProgram>) -> mwir::MwirProgram {
    let mut merged_fns: BTreeMap<String, mwir::MwirFn> = BTreeMap::new();
    let mut merged_rodata: Vec<Vec<u8>> = Vec::new();
    for p in programs {
        let offset = merged_rodata.len();
        merged_rodata.extend(p.rodata);
        for (key, mut f) in p.fns {
            if offset != 0 {
                for inst in &mut f.body {
                    if let mwir::Inst::ConstText { data, .. } = inst {
                        *data += offset;
                    }
                }
            }
            merged_fns.insert(key, f);
        }
    }
    mwir::MwirProgram {
        fns: merged_fns,
        rodata: merged_rodata,
    }
}

/// Runs the whole item-D pipeline (per-module `lower::lower_program`,
/// merged via `merge_mwir_programs`, then `codegen::codegen_program` ->
/// `layout_program`) over every `TypedProgram` in the build closure —
/// `programs` is the same whole-closure map `bin/wrela.rs`/`xtask` already
/// build for `report::render`'s own input digests.
///
/// `Ok(None)` — never a silent per-fn skip — is this module's own "all or
/// nothing" rule for a program whose reachable surface does not fully
/// lower/codegen (decision 2's own fail-closed set: a closure, a `?`, ...
/// in some method nothing here needs to name individually, since
/// `lower::lower_program`/`codegen::codegen_program` already report the
/// exact construct via their own `LowerError`/`CodegenError`, discarded
/// here only because there is no single per-program diagnostic slot to
/// put it in without inventing one this item does not need: every report-
/// bearing golden that exists today fully lowers, so this path is
/// currently unexercised by any accept case, and is not a feature this
/// item builds ahead of a program that actually needs it). `Err` is
/// reserved for `layout_program`'s own genuine internal-consistency/
/// out-of-range failures, which — unlike an ordinary lowering rejection —
/// are never expected and must never be swallowed.
///
/// `graph`/`modules` (plans/M6.md item C, added to this fn's own
/// signature — recorded deliberately, not a silent scope-creep): the one
/// necessary, disclosed exception to this item's own "do not touch
/// `bin/wrela.rs`" boundary. Static per-actor accounting (decision 3)
/// needs three facts that never appear together anywhere else already
/// reachable from this fn's own frozen callers: which actor struct each
/// declared instance names and its own declared `mailbox=` capacity
/// (`ImageGraph`, `eval::image.rs` — evaluation-time wiring, never visible
/// to `TypedProgram`/`LayoutCtx`), and which of that struct's own methods
/// are `pub` (only a `pub` method is ever a message shape, 02 §9.2 — a
/// declare-phase-only fact, `sema::types::DeclFn::receiver::is_pub`, never
/// carried onto `sema::typed::TypedFn`). Threading `graph`/`modules` two
/// parameters deeper here — and updating this fn's own two call sites
/// (`bin/wrela.rs::build_report`, `xtask`'s determinism oracle) with the
/// one already-in-scope local variable each already holds — is the
/// smallest change that avoids inventing a second, redundant
/// `eval::interp::eval_image` evaluation inside this module (which would
/// avoid the two call-site edits at the cost of a real architectural
/// smell: re-running the whole comptime evaluator a second time for data
/// its own caller already computed once, purely to route around a file
/// restriction). Both callers already hold `graph`/`modules` in scope at
/// the exact point they call this fn; neither edit changes any other
/// behavior.
pub fn try_layout_program(
    programs: &BTreeMap<String, TypedProgram>,
    layout_ctx: &LayoutCtx,
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
) -> Result<Option<ImageLayout>, String> {
    let mut mwir_programs = Vec::with_capacity(programs.len());
    for typed in programs.values() {
        match crate::lower::lower_program(typed) {
            Ok(p) => mwir_programs.push(p),
            Err(_) => return Ok(None),
        }
    }
    let merged = merge_mwir_programs(mwir_programs);
    // The async half (park-and-resume): every module's own async fns
    // lower through FlowWir and compile alongside the sync half, so a
    // build image's `Actor`/`Turn` accounting and its compiled state
    // machines can never disagree with the test-image path's.
    let mut flow_fns = BTreeMap::new();
    for typed in programs.values() {
        match crate::flowwir_lower::lower_program(typed) {
            Ok(p) => flow_fns.extend(p.fns),
            Err(_) => return Ok(None),
        }
    }
    let flow = crate::flowwir::FlowWirProgram { fns: flow_fns };
    let method_index = match actor_method_index_tables(modules, layout_ctx) {
        Ok(m) => m,
        Err(_) => return Ok(None),
    };
    let async_frames = match crate::codegen::async_frame_sizes(&flow, layout_ctx) {
        Ok(m) => m,
        Err(_) => return Ok(None),
    };
    let group_arena_capacity = count_with_group_sites(modules);
    let codegen_program = match crate::codegen::codegen_program_with_async(
        &merged,
        &flow,
        layout_ctx,
        &method_index,
        group_arena_capacity,
    ) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    let runtime_tables = compute_runtime_tables(graph, modules, layout_ctx, &async_frames)?;
    layout_program(&codegen_program, runtime_tables.as_ref())
        .map(Some)
        .map_err(|e| e.message)
}

// ===========================================================================
// plans/M6.md item C, decision 3: static actor runtime-table sizing — a
// pure function of `(graph, modules, layout_ctx)`, computed once here and
// consumed by both `layout_program`/`layout_test_image` (the `rtdata`
// reservation) and `render_layout_section` (the report's own accounting
// facts) via `ImageLayout::runtime`, so the two can never drift apart.

/// One actor declaration's own static sizing (04 §2/§7, decision 3):
/// facts only, never a placeholder — every field here is a real byte
/// count this image's own `rtdata` section actually reserves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorRuntimeLayout {
    /// The declared actor struct's own name (`types::render_type` of
    /// `ActorDecl::actor_type`) — matches the report's own existing
    /// `Actor index=N type=<name>` line so a reader can correlate the two
    /// without a shared index scheme.
    pub name: String,
    /// The declared `mailbox=` bound (02 §9.2: "the compiler derives the
    /// capacity from the closed sender set" — M6 ships the dumbest sound
    /// floor, the *declared* bound, not derivation; recorded honestly in
    /// plans/M6.md, mailbox capacity *derivation* is explicitly OUT of
    /// M6's own scope line).
    pub mailbox_capacity: u64,
    /// One ring slot's own byte size: 16 (the method-index tag plus the
    /// admitted message's own **waker** word — the awaiting turn's turn
    /// area address, or 0 for a one-way `send`; `codegen::OFF_TURN_*`'s
    /// own module doc has the whole park-and-resume contract) plus the
    /// widest of this actor's own `pub` methods' param blobs (each
    /// param's own `mwir::size_of`, summed). A method with zero params
    /// (or an actor with no `pub` methods at all) still costs the
    /// 16-byte tag+waker pair alone — the minimum a slot can ever be.
    pub slot_size: u64,
    /// This actor struct's own field storage (`mwir::size_of` over the
    /// struct's own field list, `LayoutCtx`) — where the actor instance
    /// itself lives (decision 3's own "fixed data-section slot per
    /// declared actor instance" answer, recorded in plans/M6.md).
    pub state_size: u64,
    /// This actor's own **turn area**: the fixed 48-byte turn record
    /// (`codegen::TURN_RECORD_SIZE` — busy/suspended/resume_ready/reply/
    /// waker/cur_method) plus the widest persistent frame any of its own
    /// `pub async fn` methods needs (`codegen::async_frame_sizes` — 04
    /// §2's "statically reserved frame slots", where a parked turn's
    /// live temps actually survive its `ret` to the scheduler). One area
    /// per actor, never one per queued message: non-reentrancy caps
    /// in-flight activations at one (item C's frame-arena reading,
    /// unchanged — now finally holding real async locals, the growth
    /// item C's own note predicted).
    pub frame_size: u64,
}

/// Every actor's own sizing, plus the scheduler's own fixed-capacity
/// tables (plans/M6.md item C, decision 3's own "Scheduler tables"
/// paragraph). `total_bytes` is the exact `rtdata` section size
/// `layout_program`/`layout_test_image` reserve — computed once, here,
/// from the per-actor/per-table sizes below, so the reservation and the
/// report's own totals line can never disagree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeTables {
    pub actors: Vec<ActorRuntimeLayout>,
    /// One turn area per **free** async fn (every `FlowWirProgram::fns`
    /// key that is not a declared actor's own method — `@test(runtime)`
    /// roots foremost; a compiled-but-unreachable free `async fn` still
    /// gets one so its own `Reloc::TurnFrameAddr` resolves): `(fn key,
    /// area bytes)` where area = `codegen::TURN_RECORD_SIZE` + that fn's
    /// own persistent frame. The root test turn parks/resumes through
    /// its own area via the identical machinery an actor turn uses —
    /// decision 3's "+1 root" ready-queue slot, now made concrete.
    pub free_turns: Vec<(String, u64)>,
    /// Ready-queue capacity: every actor plus the one root test turn
    /// (decision 3's own "fixed capacity = actor count + 1 root").
    /// Reserved as a real `u64`-per-slot table; `rt_select_and_run` (below)
    /// does not yet populate it (an O(actor-count) round-robin scan is the
    /// dumbest correct selection at M6's own actor counts, module doc on
    /// `build_rt_select_and_run` below) — reserved now so a later
    /// milestone's real event-driven wake can start using it without a
    /// layout change, exactly like `wrela_machine::machine_info::
    /// OFF_NEXT_DEADLINE`'s own precedent.
    pub ready_queue_capacity: u64,
    /// A real (not hand-waved) static count of `with group(...)` sites
    /// found across every raw module in the build closure (decision 3's
    /// own "fixed small global group arena sized from static with-site
    /// count" — `count_with_group_sites`, below, an honest AST-level walk
    /// that does not descend into closure bodies or `comptime if`
    /// branches; recorded as a disclosed gap in plans/M6.md, inert at C
    /// since no group actually executes against this arena until item F
    /// wires it). Always `0` in today's report-bearing corpus (no
    /// existing actor-bearing golden uses `with group` yet).
    pub group_arena_capacity: u64,
    /// The exact `rtdata` section size: every actor's own state + ring +
    /// bookkeeping + frame bytes, plus the ready-queue table, the
    /// round-robin cursor word, and the group arena.
    pub total_bytes: u64,
}

/// Per-actor ring bookkeeping beyond the ring bytes themselves: head,
/// tail, count (3 `u64`s). Reply plumbing no longer lives here at all —
/// M6-C's own `last_result` slot (the nested-drain placeholder's side
/// channel) is deleted with the placeholder itself: a completing turn's
/// reply is delivered to the *awaiting turn's* own reply slot
/// (`codegen::OFF_TURN_REPLY`, via the waker carried in the message).
const MAILBOX_BOOKKEEPING_SIZE: u64 = 3 * 8; // head, tail, count
/// The deterministic round-robin cursor word `rt_run_one` reads/advances
/// (04 §2's own tie-breaker) — one `u64`, placed right after the
/// ready-queue table (`RuntimeTables::total_bytes`'s own byte order).
const RR_CURSOR_SIZE: u64 = 8;

fn value_as_u64(v: &crate::eval::value::Value) -> Option<u64> {
    use crate::eval::value::Value;
    match *v {
        Value::U8(n) => Some(n as u64),
        Value::U16(n) => Some(n as u64),
        Value::U32(n) => Some(n as u64),
        Value::U64(n) => Some(n),
        Value::Usize(n) => Some(n as u64),
        Value::I8(n) if n >= 0 => Some(n as u64),
        Value::I16(n) if n >= 0 => Some(n as u64),
        Value::I32(n) if n >= 0 => Some(n as u64),
        Value::I64(n) if n >= 0 => Some(n as u64),
        Value::Isize(n) if n >= 0 => Some(n as u64),
        _ => None,
    }
}

/// One actor struct's own shape, re-derived from raw source the same way
/// `merge_layout_ctx`/`mwir::build_layout_ctx` already re-derive
/// `sema::specialize::specialize` -> `sema::types::declare` per module
/// (this module's own established precedent for "recompute from the raw
/// AST rather than thread extra state out of an earlier pass") — the one
/// additional fact `LayoutCtx` does not carry: each `pub` method's own
/// name, `is_async` color, and param types (02 §9.2: only a `pub` method
/// is ever reachable through `Actor[T]`, so only these are message
/// shapes). Keyed by struct name, last-module-wins on a same-spelling
/// collision — the identical disclosed simplification `merge_layout_ctx`
/// already carries.
#[derive(Clone)]
struct ActorMethodShape {
    /// plans/M6.md item D: this method's own bare name — added to this
    /// struct (previously anonymous within its own `Vec`) so
    /// `actor_method_index_tables`, below, can hand out a stable
    /// `(actor, method name) -> dispatch index` table, the exact same
    /// declaration order `dispatch: &[usize]` (`build_rt_select_and_run`)
    /// already numbers methods in.
    name: String,
    /// The method's own color — the dispatch arms in
    /// `rt_select_and_run` read the two return ABIs differently (a sync
    /// method returns its reply in `x0`; an async state machine returns
    /// status in `x0` and, when completed, its reply in `x1` —
    /// `codegen::TURN_STATUS_*`), so every dispatch entry carries this
    /// flag alongside its call target.
    is_async: bool,
    param_sizes: Vec<u64>,
}

fn merge_actor_pub_methods(
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
) -> Result<BTreeMap<String, Vec<ActorMethodShape>>, LayoutError> {
    use crate::sema::types::{DeclItem, DeclMember};

    let mut out: BTreeMap<String, Vec<ActorMethodShape>> = BTreeMap::new();
    for module in modules.values() {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("actor runtime layout: {}", e.message)))?;
        let items = crate::sema::types::declare(&specialized)
            .map_err(|e| LayoutError::new(format!("actor runtime layout: {}", e.message)))?;
        for item in items {
            let DeclItem::Struct(s) = item else { continue };
            let mut methods = Vec::new();
            for m in &s.members {
                let DeclMember::Fn(f) = m else { continue };
                let Some(recv) = &f.receiver else { continue };
                if !recv.is_pub {
                    continue;
                }
                let mut param_sizes = Vec::with_capacity(f.params.len());
                for p in &f.params {
                    let size = mwir::size_of(&p.ty, layout_ctx).map_err(|e| {
                        LayoutError::new(format!(
                            "actor `{}`'s own `{}` message shape: {e}",
                            s.name, f.name
                        ))
                    })?;
                    param_sizes.push(size as u64);
                }
                methods.push(ActorMethodShape {
                    name: f.name.clone(),
                    is_async: f.is_async,
                    param_sizes,
                });
            }
            out.insert(s.name, methods);
        }
    }
    Ok(out)
}

/// plans/M6.md item D (boot-wiring follow-up, decision 11b's own
/// verification): which actor structs declare a *zero-argument* `init`
/// (beyond the implicit `mut self` receiver) — the one shape this item's
/// boot sequence can call safely without a real init-arg materialization
/// pass (`layout::build_boot_init`'s own doc comment names that pass as
/// real, further, deferred work). An actor whose own `init` takes
/// further params is not in this map at all — its own state stays plain
/// zero-initialized, the documented floor, not a silent narrowing.
fn actor_zero_arg_init_keys(
    modules: &BTreeMap<String, Module>,
) -> Result<BTreeMap<String, String>, LayoutError> {
    use crate::sema::types::{DeclItem, DeclMember};

    let mut out = BTreeMap::new();
    for module in modules.values() {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("actor boot init: {}", e.message)))?;
        let items = crate::sema::types::declare(&specialized)
            .map_err(|e| LayoutError::new(format!("actor boot init: {}", e.message)))?;
        for item in items {
            let DeclItem::Struct(s) = item else { continue };
            for m in &s.members {
                if let DeclMember::Init(f) = m {
                    if f.params.is_empty() {
                        out.insert(s.name.clone(), format!("{}.init", s.name));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// plans/M6.md item D: `(actor name) -> (method name) -> its own 0-based
/// dispatch index`, in the exact declaration order `merge_actor_pub_methods`
/// (immediately above) already establishes — the same order
/// `build_rt_select_and_run`'s own `dispatch` table numbers methods in, so
/// codegen's own symbolic `Send`/`Await{ActorCall}` lookups
/// (`codegen::ActorMethodIndex`) can never disagree with the runtime
/// dispatch table actually built alongside it.
pub fn actor_method_index_tables(
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
) -> Result<BTreeMap<String, BTreeMap<String, usize>>, LayoutError> {
    let shapes = merge_actor_pub_methods(modules, layout_ctx)?;
    Ok(shapes
        .into_iter()
        .map(|(actor, methods)| {
            let table = methods
                .into_iter()
                .enumerate()
                .map(|(i, m)| (m.name, i))
                .collect();
            (actor, table)
        })
        .collect())
}

/// A real (not hand-waved) static count of `with group(...)` sites across
/// every raw module in the build closure — `RuntimeTables::
/// group_arena_capacity`'s own doc comment explains the scope and the
/// disclosed gap (closures, `comptime if` branches). Every surviving
/// `Stmt::With` at M6 names a `group(...)` (the scoped-`pool` `with` form
/// stays fail-closed at sema, 02 §10 — plans/M6.md item A's own note), so
/// counting every `Stmt::With` is exact for anything that type-checks.
pub fn count_with_group_sites(modules: &BTreeMap<String, Module>) -> u64 {
    use crate::syntax::ast::{FnItem, InitItem, Item, Member, Stmt};

    fn walk_stmts(stmts: &[Stmt], count: &mut u64) {
        for s in stmts {
            match s {
                Stmt::With(w) => {
                    *count += 1;
                    walk_stmts(&w.body, count);
                }
                Stmt::If(i) => {
                    walk_stmts(&i.then_branch, count);
                    for e in &i.elifs {
                        walk_stmts(&e.body, count);
                    }
                    if let Some(eb) = &i.else_branch {
                        walk_stmts(eb, count);
                    }
                }
                Stmt::Match(m) => {
                    for arm in &m.arms {
                        walk_stmts(&arm.body, count);
                    }
                }
                Stmt::For(f) => walk_stmts(&f.body, count),
                Stmt::While(w) => walk_stmts(&w.body, count),
                Stmt::Defer(d) => {
                    if let crate::syntax::ast::DeferBody::Suite(body) = &d.body {
                        walk_stmts(body, count);
                    }
                }
                Stmt::ComptimeIf(_)
                | Stmt::Assign(_)
                | Stmt::Break(_)
                | Stmt::Continue(_)
                | Stmt::Return(_, _)
                | Stmt::Pass(_)
                | Stmt::Assert(_)
                | Stmt::Send(_, _)
                | Stmt::Expr(_, _)
                | Stmt::ComptimeAssert(_, _, _) => {}
            }
        }
    }

    fn walk_fn(f: &FnItem, count: &mut u64) {
        if let Some(body) = &f.body {
            walk_stmts(body, count);
        }
    }
    fn walk_init(i: &InitItem, count: &mut u64) {
        walk_stmts(&i.body, count);
    }

    let mut count = 0u64;
    for module in modules.values() {
        for item in &module.items {
            match item {
                Item::Fn(f) => walk_fn(f, &mut count),
                Item::Struct(s) => {
                    for m in &s.members {
                        match m {
                            Member::Fn(f) => walk_fn(f, &mut count),
                            Member::Init(i) => walk_init(i, &mut count),
                            Member::Field(_) | Member::Pool(_) | Member::ComptimeIf(_) => {}
                        }
                    }
                }
                Item::Const(_) | Item::Enum(_) | Item::Pool(_) | Item::ComptimeIf(_) => {}
            }
        }
    }
    count
}

/// Which turn area an async fn's own `Reloc::TurnFrameAddr` (and its
/// sizing) belongs to: a `Struct.method` key whose struct is a declared
/// actor shares that actor's one turn area (non-reentrancy: one turn per
/// actor, whichever method it runs); every other key — free fns
/// (`@test(runtime)` roots foremost), plus any non-actor-owned method
/// key — gets its own dedicated free-turn area. One shared rule so
/// `compute_runtime_tables`'s sizing and `layout`'s reloc resolution can
/// never classify a key differently.
fn turn_owner<'k>(key: &'k str, actor_names: &[String]) -> Option<&'k str> {
    key.split_once('.')
        .map(|(prefix, _)| prefix)
        .filter(|prefix| actor_names.iter().any(|a| a == prefix))
}

/// The whole static-sizing pass (module doc above). `Ok(None)` when the
/// image declares no actors AND no async fn exists — the no-placeholder
/// rule: a fully sync image gets no `rtdata` section and no report
/// accounting at all, never a zeroed `RuntimeTables` rendered as if it
/// meant something. `async_frames` is `codegen::async_frame_sizes`'s own
/// result (fn key -> persistent frame bytes), the park-and-resume
/// redesign's one new input: each actor's turn area is sized as the
/// 48-byte record plus the widest of its own async methods' frames, and
/// every non-actor-owned async fn gets its own free-turn area
/// (`RuntimeTables::free_turns`).
pub fn compute_runtime_tables(
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
    async_frames: &BTreeMap<String, u64>,
) -> Result<Option<RuntimeTables>, String> {
    if graph.actors.is_empty() && async_frames.is_empty() {
        return Ok(None);
    }
    let shapes = merge_actor_pub_methods(modules, layout_ctx).map_err(|e| e.message)?;
    let actor_names: Vec<String> = graph
        .actors
        .iter()
        .map(|d| crate::sema::types::render_type(&d.actor_type))
        .collect();

    let mut actors = Vec::with_capacity(graph.actors.len());
    for decl in &graph.actors {
        let name = crate::sema::types::render_type(&decl.actor_type);
        let mailbox_arg = decl
            .args
            .iter()
            .find(|a| a.label == "mailbox")
            .ok_or_else(|| {
                format!(
                    "actor `{name}` has no declared `mailbox=` capacity (plans/M6.md decision 3: \
                 the declared bound is the whole of M6's own mailbox-capacity story; derivation \
                 is out of scope)"
                )
            })?;
        let mailbox_capacity = value_as_u64(&mailbox_arg.value).ok_or_else(|| {
            format!("actor `{name}`'s own `mailbox=` value is not a plain non-negative integer")
        })?;
        let state_size = mwir::size_of(&decl.actor_type, layout_ctx)
            .map_err(|e| format!("actor `{name}`'s own state: {e}"))?
            as u64;
        let methods = shapes.get(&name).map(Vec::as_slice).unwrap_or(&[]);
        let max_args_bytes = methods
            .iter()
            .map(|m| m.param_sizes.iter().sum::<u64>())
            .max()
            .unwrap_or(0);
        let slot_size = 16 + max_args_bytes; // method idx + waker + args
        let max_async_frame = async_frames
            .iter()
            .filter(|(key, _)| turn_owner(key, &actor_names) == Some(name.as_str()))
            .map(|(_, &bytes)| bytes)
            .max()
            .unwrap_or(0);
        actors.push(ActorRuntimeLayout {
            name,
            mailbox_capacity,
            slot_size,
            state_size,
            frame_size: crate::codegen::TURN_RECORD_SIZE + max_async_frame,
        });
    }

    let free_turns: Vec<(String, u64)> = async_frames
        .iter()
        .filter(|(key, _)| turn_owner(key, &actor_names).is_none())
        .map(|(key, &bytes)| (key.clone(), crate::codegen::TURN_RECORD_SIZE + bytes))
        .collect();

    let ready_queue_capacity = graph.actors.len() as u64 + 1;
    let group_arena_capacity = count_with_group_sites(modules);

    let mut total_bytes = 0u64;
    for a in &actors {
        total_bytes += a.state_size
            + a.mailbox_capacity * a.slot_size
            + MAILBOX_BOOKKEEPING_SIZE
            + a.frame_size;
    }
    for (_, area) in &free_turns {
        total_bytes += area;
    }
    total_bytes += ready_queue_capacity * 8
        + RR_CURSOR_SIZE
        + group_arena_capacity * crate::codegen::GROUP_SLOT_SIZE;

    Ok(Some(RuntimeTables {
        actors,
        free_turns,
        ready_queue_capacity,
        group_arena_capacity,
        total_bytes,
    }))
}

// --- report rendering (decision 7's own Layout section) -------------------

/// The two fixed, always-present machine regions below `IMAGE_BASE`
/// (module doc's own "pages"/"stacks" reporting note): the machine-info
/// page plus the console ring/data pages, combined into one `pages` fact,
/// and the four reserved per-core stacks as one `stacks` fact.
fn pages_region() -> (u64, u64) {
    let base = machine_layout::MACHINE_INFO_BASE;
    let end = console::DATA_BASE + console::DATA_SIZE;
    (base, end - base)
}

fn stacks_region() -> (u64, u64) {
    (
        machine_layout::STACKS_BASE,
        wrela_machine::VCPUS as u64 * machine_layout::CORE_STACK_SIZE,
    )
}

/// Appends the `Layout` section (decision 7) to an already-rendered report
/// buffer: `pages`/`stacks` (fixed machine constants, every build) then
/// every section `layout` actually placed (`entry`/`code`/`rodata`?/
/// `abort`), then the separate `Entry base=0x...` fact.
pub fn render_layout_section(out: &mut String, layout: &ImageLayout) {
    let (pages_base, pages_size) = pages_region();
    push_line(
        out,
        1,
        &format!("Section name=pages base={pages_base:#x} size={pages_size}"),
    );
    let (stacks_base, stacks_size) = stacks_region();
    push_line(
        out,
        1,
        &format!("Section name=stacks base={stacks_base:#x} size={stacks_size}"),
    );
    for s in &layout.sections {
        push_line(
            out,
            1,
            &format!("Section name={} base={:#x} size={}", s.name, s.base, s.size),
        );
    }
    push_line(out, 1, &format!("Entry base={:#x}", layout.entry));

    // --- plans/M6.md item C, decision 3: per-actor runtime-table
    // accounting — facts only, absent entirely when this image has no
    // actors (`ImageLayout::runtime`'s own doc comment: never a
    // placeholder). Appended after `Entry base=...` (04-compiler.md §7's
    // own "this milestone appends sections without reshuffling" reading,
    // mirrored here): every existing report golden's `Layout` section
    // text up to and including its own `Entry base=...` line stays
    // byte-identical; only actor-bearing images gain anything past it.
    if let Some(tables) = &layout.runtime {
        for a in &tables.actors {
            push_line(
                out,
                1,
                &format!(
                    "Actor name={} mailbox={} slot={} frame={} state={}",
                    a.name, a.mailbox_capacity, a.slot_size, a.frame_size, a.state_size
                ),
            );
        }
        // Free-turn areas (park-and-resume: one per non-actor-owned
        // async fn — `RuntimeTables::free_turns`'s own doc comment) —
        // facts only, absent when no free async fn exists.
        for (key, area) in &tables.free_turns {
            push_line(out, 1, &format!("Turn fn={key} frame={area}"));
        }
        push_line(
            out,
            1,
            &format!(
                "Totals actors={} ready_queue={} group_arena={} bytes={}",
                tables.actors.len(),
                tables.ready_queue_capacity,
                tables.group_arena_capacity,
                tables.total_bytes
            ),
        );
    }
}

// ===========================================================================
// Emitted runtime routines (plans/M6.md item C, redesigned by the
// park-and-resume mandate): `rt_enqueue` (admission), `rt_select_and_run`
// (per-actor readiness/selection/dispatch/reply-delivery), `rt_run_one`
// (the round-robin scheduler tick over every actor), and the abandon
// path. Hand-assembled via `Asm` (defined below, M5-E's own tool), the
// identical M5-E style: no asm strings, one instruction encoder call at a
// time, conformance established by real execution (JIT'd against
// host-mmap'd stand-in memory, `harness_jit` below, plus real HVF boots
// in `wrela-vmm`'s own conformance tests) rather than by hand-verifying
// encoded bytes.
//
// ## Why one hand-assembled pair *per actor*, not one generic pair indexed
// by a runtime `actor_idx`
//
// Every actor's own ring/state/bookkeeping address is already a build-time
// constant (`RuntimeTables`/`place_runtime_tables`, above/below) — there is
// no `actor_idx` a real caller would ever need to pass at runtime, so a
// generic address-indexed pair would only add a layer of register-offset
// indirection this milestone's own actor counts never need. 04-compiler.md
// §6's own "Actor as-if" license says exactly this is allowed: "the
// compiler may use direct placement, specialized dispatch tables ...
// provided admission order, non-reentrancy, ... are all preserved" — one
// specialized `rt_enqueue_actor`/`rt_select_actor` pair per actor is that
// license exercised at its simplest.
//
// ## Slot layout (decision 3's "method index + args blob", grown a waker)
//
// `[0..8)`: the admitted message's own method index, a plain `u64`.
// `[8..16)`: the message's own waker — the awaiting turn's turn-area
// address, or 0 for a one-way `send` (`codegen::OFF_TURN_*`'s module doc
// carries the whole contract).
// `[16..slot_size)`: the method's own argument blob, raw 8-byte-per-
// scalar-param words in declared parameter order
// (`ActorRuntimeLayout::slot_size`'s own doc comment) — every param here
// is assumed to fit one 8-byte slot (a disclosed, real simplification: a
// message-legal aggregate wider than one slot is out of scope for this
// hand-assembled dispatch; every `pub` actor method in today's whole
// corpus takes only scalar params, so this never silently narrows a real
// case).

/// One actor's own absolute runtime-table addresses, placed sequentially
/// from a given base (`rtdata_base` for a real image, or a host-mmap'd
/// stand-in base for a JIT/HVF test) — the exact byte order
/// `compute_runtime_tables`'s own `RuntimeTables::total_bytes` already
/// accounts for (state, ring, head/tail/count, turn area), so a real
/// image's `rtdata` section and this fn's own addresses can never
/// disagree.
#[derive(Debug, Clone, Copy)]
pub struct ActorAddrs {
    pub state: u64,
    pub ring: u64,
    pub head: u64,
    pub tail: u64,
    pub count: u64,
    /// This actor's own turn area base — the fixed 48-byte turn record
    /// (`codegen::OFF_TURN_*`: busy/suspended/resume_ready/reply/waker/
    /// cur_method) followed by its persistent async frame slots. The
    /// address every message this actor's turns *send* carries as their
    /// waker, and the address `Reloc::TurnFrameAddr` resolves to for its
    /// own async methods.
    pub turn: u64,
}

/// Every runtime-table address, placed from one `base` (`rtdata_base` for
/// a real image, a host-mmap'd stand-in for a JIT/HVF test) in the exact
/// byte order `compute_runtime_tables::total_bytes` accounts for: each
/// actor's region (state, ring, head/tail/count, turn area), then every
/// free-turn area, then the ready-queue table, the round-robin cursor
/// word, and the group arena.
#[derive(Debug, Clone, Default)]
pub struct RuntimePlacement {
    pub actors: Vec<ActorAddrs>,
    /// fn key -> free-turn area base (`RuntimeTables::free_turns` order).
    pub free_turns: BTreeMap<String, u64>,
    /// The deterministic round-robin cursor word `rt_run_one` reads/
    /// advances (04 §2's tie-breaker; at M6 every scheduling key is
    /// equal, so the cursor is the whole selection order among ready
    /// actors).
    pub rr_cursor: u64,
    /// plans/M6.md item F: the whole-image group arena's own base address
    /// — `Reloc::GroupArenaBase`'s own resolution target, placed last
    /// (`RuntimeTables::total_bytes`'s own byte-order doc: actors, free
    /// turns, ready-queue table, rr cursor, then the group arena).
    pub group_arena: u64,
}

impl RuntimePlacement {
    /// The turn area for async fn `key` (`turn_owner`'s own rule):
    /// an actor method's area is its actor's; anything else its own
    /// free-turn area. `None` only for a key the tables never sized —
    /// an internal inconsistency the caller reports loudly.
    pub fn turn_area_for(&self, key: &str, tables: &RuntimeTables) -> Option<u64> {
        let actor_names: Vec<String> = tables.actors.iter().map(|a| a.name.clone()).collect();
        match turn_owner(key, &actor_names) {
            Some(actor) => tables
                .actors
                .iter()
                .position(|a| a.name == actor)
                .map(|i| self.actors[i].turn),
            None => self.free_turns.get(key).copied(),
        }
    }
}

pub fn place_runtime_tables(base: u64, tables: &RuntimeTables) -> RuntimePlacement {
    let mut cursor = base;
    let mut actors = Vec::with_capacity(tables.actors.len());
    for a in &tables.actors {
        let state = cursor;
        cursor += a.state_size;
        let ring = cursor;
        cursor += a.mailbox_capacity * a.slot_size;
        let head = cursor;
        cursor += 8;
        let tail = cursor;
        cursor += 8;
        let count = cursor;
        cursor += 8;
        let turn = cursor;
        cursor += a.frame_size;
        actors.push(ActorAddrs {
            state,
            ring,
            head,
            tail,
            count,
            turn,
        });
    }
    let mut free_turns = BTreeMap::new();
    for (key, area) in &tables.free_turns {
        free_turns.insert(key.clone(), cursor);
        cursor += area;
    }
    cursor += tables.ready_queue_capacity * 8;
    let rr_cursor = cursor;
    cursor += RR_CURSOR_SIZE;
    let group_arena = cursor;
    RuntimePlacement {
        actors,
        free_turns,
        rr_cursor,
        group_arena,
    }
}

/// `rt_enqueue_actor(x0=method_idx, x1=args_ptr, x2=nargs_words,
/// x3=waker) -> x0 (0=admitted, 1=rejected — the `send`/call admission
/// outcome, 02 §9.4's `NotAdmitted`/`Rejected` path, the minimal
/// encoding of it)`. Admission alone — never selection, never dispatch,
/// never readiness: a bounded ring insert, FIFO by construction (always
/// appended at `tail`, always drained from `head` by
/// `rt_select_and_run`) — 04 §2's "admission occupies one logical
/// mailbox slot until selection; selection is FIFO per mailbox by
/// admission order". `waker` (the awaiting turn's own turn-area address,
/// or 0 for a one-way `send`) is stored into the slot's second word and
/// carried to selection, where the dispatched turn's completion delivers
/// its reply there. Admission is deliberately independent of the
/// target's `busy` flag: a message to a busy(-suspended) actor QUEUES —
/// decision 4's non-reentrancy lives entirely in selection, never here.
/// A full ring (`count == capacity`) is rejected without touching
/// `tail`/`count` at all — the caller's own `args_ptr` blob is left
/// exactly where it was, mirroring 02 §9.4's "an outcome that did not
/// consume [arguments] hands them back" at this ABI granularity (a real
/// `NotAdmitted(..)` payload carry-back is item G's job).
///
/// Register use (leaf fn, owns every register it touches, never `x0..x3`
/// until the outcome/scratch reuse below): `x9`/`x10` = count addr/value,
/// then reused as scratch after the branch; `x11` = capacity, then a
/// scratch; `x12`/`x13` = tail addr/value; `x14`/`x15` = slot-size scratch,
/// then the computed slot address; `x16`/`x17`/`x18` = the copy loop's
/// dst/src/remaining-count cursors.
pub fn build_rt_enqueue(
    addrs: &ActorAddrs,
    capacity: u64,
    slot_size: u64,
    start: usize,
) -> Vec<u32> {
    let mut a = Asm::new(start);

    a.load_imm(9, addrs.count);
    a.push(encode::enc_ldr_x_imm(10, 9, 0));
    a.load_imm(11, capacity);
    a.push(encode::enc_cmp_reg(10, 11, true));
    let skip_ok = a.skip_placeholder(); // b.lt .ok
    a.push(encode::enc_movz(0, 1, 0, true)); // rejected
    let to_end = a.skip_placeholder(); // b .end (unconditional, patched below)
    let ok = a.abs();
    a.patch_cond(skip_ok, Cond::Lt);
    debug_assert_eq!(ok, a.abs());

    // .ok: slot = ring + tail * slot_size
    a.load_imm(12, addrs.tail);
    a.push(encode::enc_ldr_x_imm(13, 12, 0));
    a.load_imm(14, slot_size);
    a.push(encode::enc_mul(14, 13, 14, true));
    a.load_imm(15, addrs.ring);
    a.push(encode::enc_add_reg(15, 15, 14, true));

    a.push(encode::enc_str_x_imm(0, 15, 0)); // slot.method_idx = method_idx
    a.push(encode::enc_str_x_imm(3, 15, 8)); // slot.waker = waker

    a.push(encode::enc_add_imm(16, 15, 16, true)); // dst cursor = slot + 16 (past idx+waker)
    a.push(encode::enc_mov_reg(17, 1, true)); // src cursor = args_ptr
    a.push(encode::enc_mov_reg(18, 2, true)); // remaining = nargs_words
    let loop_top = a.abs();
    let skip_loop = a.skip_placeholder(); // cbz x18, .copied
    a.push(encode::enc_ldr_x_imm(9, 17, 0));
    a.push(encode::enc_str_x_imm(9, 16, 0));
    a.push(encode::enc_add_imm(17, 17, 8, true));
    a.push(encode::enc_add_imm(16, 16, 8, true));
    a.push(encode::enc_sub_imm(18, 18, 1, true));
    a.b_to(loop_top);
    let copied = a.abs();
    a.patch_cbz(skip_loop, 18);
    debug_assert_eq!(copied, a.abs());

    // tail = (tail + 1) % capacity
    a.push(encode::enc_add_imm(13, 13, 1, true));
    a.load_imm(9, capacity);
    a.push(encode::enc_cmp_reg(13, 9, true));
    let skip_nowrap = a.skip_placeholder(); // b.lt .nowrap
    a.push(encode::enc_movz(13, 0, 0, true));
    let nowrap = a.abs();
    a.patch_cond(skip_nowrap, Cond::Lt);
    debug_assert_eq!(nowrap, a.abs());
    a.load_imm(12, addrs.tail);
    a.push(encode::enc_str_x_imm(13, 12, 0));

    // count += 1
    a.load_imm(9, addrs.count);
    a.push(encode::enc_ldr_x_imm(10, 9, 0));
    a.push(encode::enc_add_imm(10, 10, 1, true));
    a.push(encode::enc_str_x_imm(10, 9, 0));

    a.push(encode::enc_movz(0, 0, 0, true)); // admitted
    let end = a.abs();
    let this = a.start + to_end;
    let delta = (end as i64 - this as i64) * 4;
    a.words[to_end] = encode::enc_b(delta as i32);
    a.push(encode::enc_ret(30));
    a.words
}

/// `rt_select_actor() -> x0 (1 = ran one turn-slice this call, 0 = not
/// ready — busy with its awaited reply still outstanding, or its own
/// mailbox is empty)`. One "tick" of 04 §2's event loop for one actor,
/// carrying the whole selection half of the park-and-resume contract
/// (`codegen::OFF_TURN_*`'s own module doc; admission is `rt_enqueue`'s
/// entirely separate job):
///
///   - **Readiness**: a busy actor whose turn is parked AND whose reply
///     has been delivered (`suspended && resume_ready`) is ready to
///     *resume*; a non-busy actor with a queued message is ready for a
///     *fresh* turn; anything else reports 0. Decision 4's
///     non-reentrancy is exactly the busy check: a queued second message
///     stays queued until the owning turn fully completes.
///   - **Fresh dispatch**: FIFO pop from `head` — method idx, waker, and
///     args read from the slot; `cur_method`/`waker` saved into the turn
///     record; `head`/`count` advanced HERE, at selection ("admission
///     occupies one logical mailbox slot until selection", 04 §2 — the
///     slot is released the moment the turn starts, not when it ends);
///     `busy = 1`; `BL` the method.
///   - **Resume dispatch**: re-`BL` the SAME compiled method
///     (`cur_method`, the saved dispatch index) — the fn's own entry
///     discriminant routes itself to its saved `resume_state`.
///   - **Status**: each dispatch arm knows its method's color at build
///     time. A sync method's return IS completion (its reply in `x0`) —
///     a sync `pub` method runs as one complete, unsuspendable turn. An
///     async method returns `x0 = TURN_STATUS_*`: suspended means this
///     call ran a real slice (return 1, busy stays set — the park);
///     completed carries the reply in `x1`.
///   - **Delivery**: on completion, the reply is written to the waker's
///     own turn record (`[waker + OFF_TURN_REPLY]`, then
///     `resume_ready = 1`) — waker 0 (a `send`) delivers nowhere; then
///     `busy = 0`.
///
/// `dispatch[i]` = (call target, is_async). Register use: `x9..x13`
/// scratch; `x15` = method_idx, live across the dispatch chain.
pub fn build_rt_select_and_run(
    addrs: &ActorAddrs,
    capacity: u64,
    slot_size: u64,
    dispatch: &[(usize, bool)],
    start: usize,
) -> Vec<u32> {
    let colors: Vec<bool> = dispatch.iter().map(|(_, is_async)| *is_async).collect();
    build_rt_select_and_run_core(addrs, capacity, slot_size, &colors, start, |a, idx| {
        a.bl_to(dispatch[idx].0)
    })
    .words
}

/// The exact same routine as `build_rt_select_and_run` above, but
/// dispatching to a *real compiled program's own `code` section* by fn
/// key (a `Reloc::Call`, resolved by `layout_test_image` exactly like an
/// ordinary compiled call) instead of a same-buffer absolute word index —
/// a sync method's real compiled body and an async method's real compiled
/// state-machine entry are both ordinary `program.fns` entries. Shares
/// every byte of hand-assembly with the JIT-tested original above via
/// `build_rt_select_and_run_core` — never a forked copy.
fn build_rt_select_and_run_symbolic(
    addrs: &ActorAddrs,
    capacity: u64,
    slot_size: u64,
    dispatch: &[(String, bool)],
    start: usize,
) -> Asm {
    let colors: Vec<bool> = dispatch.iter().map(|(_, is_async)| *is_async).collect();
    build_rt_select_and_run_core(addrs, capacity, slot_size, &colors, start, |a, idx| {
        a.bl_call_key(&dispatch[idx].0)
    })
}

fn build_rt_select_and_run_core(
    addrs: &ActorAddrs,
    capacity: u64,
    slot_size: u64,
    method_is_async: &[bool],
    start: usize,
    mut call_dispatch: impl FnMut(&mut Asm, usize),
) -> Asm {
    use crate::codegen::{
        OFF_TURN_CUR_METHOD, OFF_TURN_REPLY, OFF_TURN_RESUME_READY, OFF_TURN_SUSPENDED,
        OFF_TURN_WAKER,
    };
    let mut a = Asm::new(start);
    // Unlike most other hand-assembled fragments in this file (leaf fns,
    // or noreturn like the abort stubs), this one both calls out (`BL`
    // into a dispatched method) *and* returns via an ordinary `ret` — so
    // it must save/restore its own `x30` (link register), exactly the
    // ABI's own "x30 is call-clobbered" rule `codegen.rs`'s real
    // prologues already apply: a first draft of the item-C original
    // skipped this, called the dispatched method, and hung forever (the
    // dispatched method's own `RET x30` correctly returned *into* this
    // fn right after its own `BL`, but this fn's *own* final `ret x30`
    // then read that same, now-stale value instead of its original
    // caller's address, jumping back into itself in an infinite loop —
    // caught by a real JIT execution test hanging, exactly what this
    // module's own "behavior is the oracle" doc paragraph is for).
    a.push(encode::enc_sub_imm(31, 31, 16, true)); // sub sp, sp, #16
    a.push(encode::enc_str_x_imm(30, 31, 0)); // str x30, [sp]
    let mut to_idle_ret: Vec<usize> = Vec::new(); // b .epilogue with x0 already 0
    let mut to_epilogue: Vec<usize> = Vec::new(); // b .epilogue with x0 already set
    let mut to_deliver: Vec<usize> = Vec::new(); // b .deliver with x9 = reply

    // --- readiness -----------------------------------------------------
    // busy?
    a.load_imm(9, addrs.turn);
    a.push(encode::enc_ldr_x_imm(10, 9, 0)); // x10 = busy (OFF_TURN_BUSY = 0)
    let skip_fresh_check = a.skip_placeholder(); // cbz x10, .fresh_check
    // busy: resumable only if suspended && resume_ready.
    a.push(encode::enc_ldr_x_imm(10, 9, OFF_TURN_SUSPENDED as u16));
    let skip_idle_a = a.skip_placeholder(); // cbz x10, .idle
    a.push(encode::enc_ldr_x_imm(10, 9, OFF_TURN_RESUME_READY as u16));
    let skip_idle_b = a.skip_placeholder(); // cbz x10, .idle
    // resume: x15 = cur_method; x0 = self ptr (harmless on resume — the
    // fn's own resume path reads nothing from the arg registers).
    a.push(encode::enc_ldr_x_imm(15, 9, OFF_TURN_CUR_METHOD as u16));
    a.load_imm(0, addrs.state);
    let to_dispatch_from_resume = a.skip_placeholder(); // b .dispatch

    // .idle (busy but not resumable): x0 = 0, epilogue.
    let idle = a.abs();
    a.patch_cbz(skip_idle_a, 10);
    a.patch_cbz(skip_idle_b, 10);
    debug_assert_eq!(idle, a.abs());
    a.push(encode::enc_movz(0, 0, 0, true));
    to_idle_ret.push(a.skip_placeholder());

    // .fresh_check: mailbox empty?
    let fresh_check = a.abs();
    a.patch_cbz(skip_fresh_check, 10);
    debug_assert_eq!(fresh_check, a.abs());
    a.load_imm(9, addrs.count);
    a.push(encode::enc_ldr_x_imm(10, 9, 0));
    let skip_have_msg = a.skip_placeholder(); // cbnz x10, .have_msg
    a.push(encode::enc_movz(0, 0, 0, true));
    to_idle_ret.push(a.skip_placeholder());
    let have_msg = a.abs();
    a.patch_cbnz(skip_have_msg, 10);
    debug_assert_eq!(have_msg, a.abs());

    // --- fresh selection ------------------------------------------------
    // busy = 1
    a.load_imm(9, addrs.turn);
    a.load_imm(10, 1);
    a.push(encode::enc_str_x_imm(10, 9, 0));

    // slot = ring + head * slot_size
    a.load_imm(11, addrs.head);
    a.push(encode::enc_ldr_x_imm(12, 11, 0));
    a.load_imm(13, slot_size);
    a.push(encode::enc_mul(13, 12, 13, true));
    a.load_imm(9, addrs.ring);
    a.push(encode::enc_add_reg(13, 9, 13, true)); // x13 = slot addr

    a.push(encode::enc_ldr_x_imm(15, 13, 0)); // x15 = method_idx
    a.push(encode::enc_ldr_x_imm(10, 13, 8)); // x10 = waker
    a.load_imm(9, addrs.turn);
    a.push(encode::enc_str_x_imm(15, 9, OFF_TURN_CUR_METHOD as u16));
    a.push(encode::enc_str_x_imm(10, 9, OFF_TURN_WAKER as u16));
    // Load only as many 8-byte arg words as this ring's own `slot_size`
    // actually reserves past the 16-byte idx+waker pair (never
    // unconditionally 2): the smallest legal slot is `slot_size=16` (a
    // no-arg message) — reading fixed words regardless would read *past*
    // the ring into `head`/`tail` themselves for a narrower slot. A real
    // HVF boot caught the item-C ancestor of exactly this bug (module
    // doc note); the bound stays load-bearing here.
    let arg_words = ((slot_size.saturating_sub(16)) / 8).min(2);
    if arg_words >= 1 {
        a.push(encode::enc_ldr_x_imm(1, 13, 16)); // x1 = arg0
    }
    if arg_words >= 2 {
        a.push(encode::enc_ldr_x_imm(2, 13, 24)); // x2 = arg1
    }
    // Release the slot NOW — selection, not completion, frees it
    // (04 §2): head = (head + 1) % capacity; count -= 1. `x12` still
    // holds the head value from the slot computation above.
    a.push(encode::enc_add_imm(12, 12, 1, true));
    a.load_imm(9, capacity);
    a.push(encode::enc_cmp_reg(12, 9, true));
    let skip_nowrap = a.skip_placeholder(); // b.lt .nowrap
    a.push(encode::enc_movz(12, 0, 0, true));
    let nowrap = a.abs();
    a.patch_cond(skip_nowrap, Cond::Lt);
    debug_assert_eq!(nowrap, a.abs());
    a.load_imm(11, addrs.head);
    a.push(encode::enc_str_x_imm(12, 11, 0));
    a.load_imm(9, addrs.count);
    a.push(encode::enc_ldr_x_imm(10, 9, 0));
    a.push(encode::enc_sub_imm(10, 10, 1, true));
    a.push(encode::enc_str_x_imm(10, 9, 0));

    a.load_imm(0, addrs.state); // x0 = self ptr (the receiver ABI)

    // --- dispatch (shared by fresh and resume; x15 = method index) -----
    let dispatch = a.abs();
    {
        let this = a.start + to_dispatch_from_resume;
        let delta = (dispatch as i64 - this as i64) * 4;
        a.words[to_dispatch_from_resume] = encode::enc_b(delta as i32);
    }
    for (idx, &is_async) in method_is_async.iter().enumerate() {
        a.push(encode::enc_cmp_imm(15, idx as u16, true));
        let skip_next = a.skip_placeholder(); // b.ne .next
        call_dispatch(&mut a, idx);
        if is_async {
            // x0 = status; on completion x1 = reply.
            let skip_completed = a.skip_placeholder(); // cbz x0, .completed
            // Suspended: a real slice ran; busy stays set; x0 is
            // already 1 (TURN_STATUS_SUSPENDED) — the "ran" report.
            to_epilogue.push(a.skip_placeholder());
            let completed = a.abs();
            a.patch_cbz(skip_completed, 0);
            debug_assert_eq!(completed, a.abs());
            a.push(encode::enc_mov_reg(9, 1, true)); // x9 = reply
            to_deliver.push(a.skip_placeholder());
        } else {
            // A sync method's return IS completion; reply in x0.
            a.push(encode::enc_mov_reg(9, 0, true)); // x9 = reply
            to_deliver.push(a.skip_placeholder());
        }
        let next = a.abs();
        a.patch_cond(skip_next, Cond::Ne);
        debug_assert_eq!(next, a.abs());
    }
    // No dispatch entry matched — an internal-error guard: `rt_enqueue`
    // only ever admits a `method_idx` this same table was built from,
    // and `cur_method` is only ever one it stored itself.
    a.push(encode::enc_brk(0xACD0));

    // --- .deliver: reply (x9) -> waker's record; busy = 0; return 1 ----
    let deliver = a.abs();
    for m in &to_deliver {
        let this = a.start + m;
        let delta = (deliver as i64 - this as i64) * 4;
        a.words[*m] = encode::enc_b(delta as i32);
    }
    debug_assert_eq!(deliver, a.abs());
    a.load_imm(10, addrs.turn);
    a.push(encode::enc_ldr_x_imm(11, 10, OFF_TURN_WAKER as u16));
    let skip_no_waker = a.skip_placeholder(); // cbz x11, .no_waker
    a.push(encode::enc_str_x_imm(9, 11, OFF_TURN_REPLY as u16));
    a.push(encode::enc_movz(12, 1, 0, true));
    a.push(encode::enc_str_x_imm(12, 11, OFF_TURN_RESUME_READY as u16));
    let no_waker = a.abs();
    a.patch_cbz(skip_no_waker, 11);
    debug_assert_eq!(no_waker, a.abs());
    a.push(encode::enc_str_x_imm(31, 10, 0)); // busy = 0 (xzr)
    a.push(encode::enc_str_x_imm(31, 10, OFF_TURN_WAKER as u16)); // waker = 0 (hygiene)
    a.push(encode::enc_movz(0, 1, 0, true)); // ran a turn(-slice)

    // --- .epilogue (every exit; x0 already holds the report) -----------
    let epilogue = a.abs();
    a.push(encode::enc_ldr_x_imm(30, 31, 0)); // ldr x30, [sp]
    a.push(encode::enc_add_imm(31, 31, 16, true)); // add sp, sp, #16
    a.push(encode::enc_ret(30));
    for m in to_idle_ret.iter().chain(to_epilogue.iter()) {
        let this = a.start + m;
        let delta = (epilogue as i64 - this as i64) * 4;
        a.words[*m] = encode::enc_b(delta as i32);
    }
    a
}

/// `rt_run_one() -> x0 (1 = ran one ready turn-slice, 0 = nothing
/// ready)` — 04 §2's selection across every actor on the core, made
/// concrete at M6's defaults: every mailbox head shares one (priority,
/// deadline) key (all normal band, deadline = infinity), so selection is
/// exactly the deterministic round-robin cursor over the per-actor
/// readiness `rt_select_and_run` already encodes. Fully unrolled, two
/// passes over the build-time actor list — pass one tries every actor at
/// or after the cursor, pass two the rest — and the first actor that
/// reports "ran" advances the cursor to its own successor and returns 1.
/// The entry driver loops this between a root turn's own suspend points;
/// "nothing ready" with the root still incomplete is the deadlock
/// condition (`DEADLOCK_MSG`).
fn build_rt_run_one(select_starts: &[usize], rr_cursor_addr: u64, start: usize) -> Asm {
    let mut a = Asm::new(start);
    a.push(encode::enc_sub_imm(31, 31, 16, true));
    a.push(encode::enc_str_x_imm(30, 31, 0));
    let n = select_starts.len();
    let mut to_out: Vec<usize> = Vec::new();
    for pass in 0..2 {
        for (i, &sel) in select_starts.iter().enumerate() {
            // Reload the cursor each arm — the BL below clobbers scratch.
            a.load_imm(9, rr_cursor_addr);
            a.push(encode::enc_ldr_x_imm(10, 9, 0));
            a.push(encode::enc_cmp_imm(10, i as u16, true));
            let skip = a.skip_placeholder(); // pass 0: b.gt (cursor > i -> not yet); pass 1: b.le (already tried)
            a.bl_to(sel);
            let skip_notran = a.skip_placeholder(); // cbz x0, .skip
            // Ran: cursor = (i + 1) % n, report 1.
            a.load_imm(9, rr_cursor_addr);
            a.load_imm(10, ((i + 1) % n) as u64);
            a.push(encode::enc_str_x_imm(10, 9, 0));
            a.push(encode::enc_movz(0, 1, 0, true));
            to_out.push(a.skip_placeholder());
            let skip_to = a.abs();
            a.patch_cond(skip, if pass == 0 { Cond::Gt } else { Cond::Le });
            a.patch_cbz(skip_notran, 0);
            debug_assert_eq!(skip_to, a.abs());
        }
    }
    a.push(encode::enc_movz(0, 0, 0, true)); // nothing ready
    let out = a.abs();
    for m in &to_out {
        let this = a.start + m;
        let delta = (out as i64 - this as i64) * 4;
        a.words[*m] = encode::enc_b(delta as i32);
    }
    a.push(encode::enc_ldr_x_imm(30, 31, 0));
    a.push(encode::enc_add_imm(31, 31, 16, true));
    a.push(encode::enc_ret(30));
    a
}

/// The deadlock diagnostic's exact transcript wording (printed through
/// the ordinary `__wrela_abort` path onto the failing root turn's own
/// test line, then counted as that test's failure — the image exits
/// nonzero): nothing is ready to run and the root turn has not
/// completed, so no progress is possible — fail closed, deterministic,
/// never a hang.
pub const DEADLOCK_MSG: &str =
    "runtime deadlock: no turn is ready and the root turn has not completed";

/// Every actor's own `rt_enqueue`/`rt_select_and_run` pair, placed
/// sequentially from `start`, then the one shared `rt_run_one` scheduler
/// tick over all of them — `layout_test_image` registers each enqueue
/// under `rt_enqueue_symbol` so a compiled `Send`/`Await{ActorCall}`
/// op's own `Reloc::Call` resolves to it; the entry driver reaches
/// `rt_run_one` by the returned word index. Word counts here never
/// depend on `placement`'s own address *values* (every `load_imm` is a
/// fixed four words regardless) — only on `tables`/`actor_dispatch`'s
/// own shapes — so this fn is safe to call once with a placeholder
/// placement purely to learn the total word count, then again with the
/// real, now-known addresses for the bytes that actually ship.
struct RuntimeGlue {
    asms: Vec<Asm>,
    symbols: BTreeMap<String, usize>,
    /// `rt_run_one`'s own absolute word index (the entry driver's
    /// scheduler-tick target). Present whenever any glue exists at all.
    rt_run_one_start: usize,
}

fn build_runtime_glue_block(
    tables: &RuntimeTables,
    actor_dispatch: &[(String, Vec<(String, bool)>)],
    placement: &RuntimePlacement,
    start: usize,
) -> RuntimeGlue {
    let mut asms = Vec::new();
    let mut symbols = BTreeMap::new();
    let mut select_starts = Vec::with_capacity(tables.actors.len());
    let mut cursor = start;
    for (i, a) in tables.actors.iter().enumerate() {
        let addrs = &placement.actors[i];
        let (_, dispatch_keys) = &actor_dispatch[i];

        let enqueue_start = cursor;
        let enqueue_words = build_rt_enqueue(addrs, a.mailbox_capacity, a.slot_size, enqueue_start);
        cursor += enqueue_words.len();
        symbols.insert(crate::codegen::rt_enqueue_symbol(&a.name), enqueue_start);
        asms.push(Asm {
            start: enqueue_start,
            words: enqueue_words,
            relocs: Vec::new(),
        });

        let select_start = cursor;
        let select_asm = build_rt_select_and_run_symbolic(
            addrs,
            a.mailbox_capacity,
            a.slot_size,
            dispatch_keys,
            select_start,
        );
        cursor += select_asm.words.len();
        select_starts.push(select_start);
        asms.push(select_asm);
    }
    let rt_run_one_start = cursor;
    let run_one_asm = build_rt_run_one(&select_starts, placement.rr_cursor, rt_run_one_start);
    asms.push(run_one_asm);
    RuntimeGlue {
        asms,
        symbols,
        rt_run_one_start,
    }
}

/// plans/M6.md item D: the real boot sequence's own actor-state half —
/// every actor's own state slot, zero-initialized before any root turn
/// runs (`build_entry_driver`'s own `bl_to(boot_init_start)`, right after
/// the console/test-counter zeroing it already did). **Disclosed floor,
/// not silently narrowed**: calling a declared `init` (materializing
/// `ActorDecl::args` against its own declared parameter list, in
/// dependency order) is real, further work this item does not ship —
/// every M6-D required conformance actor's own fields are plain data with
/// no declared `init` of their own, so plain zero-initialization is exact
/// for them; a real `init`-arg materialization pass is named, deferred
/// follow-up for whichever later item's own flagship boot actually
/// declares one (recorded in the ledger clause, not silently assumed
/// solved).
fn build_boot_init(
    actor_names: &[String],
    actor_addrs: &[ActorAddrs],
    state_sizes: &[u64],
    init_keys: &BTreeMap<String, String>,
    start: usize,
) -> Asm {
    let mut a = Asm::new(start);
    // Called via `bl_to` from `build_entry_driver` and itself calls out
    // to a real compiled `init` below — `x30` (the link register) is
    // call-clobbered, so it must be saved/restored around this fn's own
    // body exactly like `build_rt_select_and_run_core`'s own hard-won
    // lesson (that fn's own module doc has the full incident report): a
    // first draft of this fn skipped this and looped the whole entry
    // driver from its own start forever (`init`'s own correctly-saved/
    // restored `x30` pointed back at *this* fn's own call site, not this
    // fn's real caller), caught by exactly the same "behavior is the
    // oracle" real-boot test this comment now documents.
    a.push(encode::enc_sub_imm(31, 31, 16, true));
    a.push(encode::enc_str_x_imm(30, 31, 0));
    for ((name, addrs), &size) in actor_names.iter().zip(actor_addrs).zip(state_sizes) {
        let mut w = 0u64;
        while w < size {
            a.load_imm(9, addrs.state + w);
            a.push(encode::enc_str_x_imm(31, 9, 0)); // store xzr (unit is Copy/all-zero-valid)
            w += 8;
        }
        // A zero-argument `init` runs after the zero-fill, overwriting
        // whichever fields it sets — `build_boot_init`'s own module doc
        // has the full reasoning for why only this shape is handled here.
        if let Some(key) = init_keys.get(name) {
            a.load_imm(0, addrs.state);
            a.bl_call_key(key);
        }
    }
    a.push(encode::enc_ldr_x_imm(30, 31, 0));
    a.push(encode::enc_add_imm(31, 31, 16, true));
    a.push(encode::enc_ret(30));
    a
}

// ===========================================================================
// plans/M5.md item E: the runtime test image's own harness.
//
// `layout_program`/`build_entry_stub`/`build_abort_stub` above are the
// *ordinary* image's entry/abort — `wrela build`/`--stage=report`'s own
// path, untouched by this item (the four pre-existing report goldens pin
// that placeholder entry's exact bytes; CLAUDE.md's "existing goldens must
// not move" wins over the module doc's older, pre-item-E speculation that
// item E would replace that body wholesale — it does not, for exactly this
// reason, recorded here instead of silently contradicting the paragraph
// above).
//
// Item E instead adds a **second**, wholly separate placement function,
// `layout_test_image`, used only by `wrela test`'s runtime tier (`bin/
// wrela.rs`): a real driver that calls every `@test(runtime)` fn in
// declaration order, prints each one's own report line over the console
// ring (decision 12), and an extended `__wrela_abort`/`__wrela_abort_val`
// that also print before halting — the exact obligation the pre-existing
// module doc named, now honestly split into its own code path instead of
// silently changing the shared one.
//
// ## The landing pad (the plan's own required mechanism, precisely)
//
// Every runtime test's call goes through one fixed continuation slot,
// `wrela_machine::machine_info::OFF_TEST_CONTINUATION`: right before the
// entry driver's `BL` to a test fn, it stores the address of *its own next
// step* (the point right after that test's own "ok" line and passed-
// counter increment — i.e. the top of the next test's own block, or the
// summary block for the last test) into that slot. `__wrela_abort`/
// `__wrela_abort_val`'s own bodies, extended by this item, print the
// `FAILED ...` line, increment the fail counter, then `LDR` that slot and
// `BR` to it directly — never `RET` — resuming the driver exactly at the
// point the aborted test's own "ok" line would have run, skipping it. This
// is what makes an abort *inside* an arbitrarily deep call chain (a test
// calling a helper calling a helper whose checked arithmetic overflows)
// still land back in the flat, straight-line entry driver: the slot is a
// fixed absolute guest address, not anything SP-relative, so it survives
// regardless of how many un-popped frames sit between the abort site and
// the driver's own stack depth at the point of the original `BL`.
//
// ## Why the entry driver needs no runtime loop at all
//
// Every `@test(runtime)` fn's name is known at compile time (the whole
// point of `wrela test`'s runtime tier: one fixed image per build), so the
// driver is fully unrolled — one straight-line block per test, in
// declaration order, then one summary block — never a real loop over a
// test list. This is why the landing pad's own continuation address can be
// a single fixed slot reused test-to-test: only one test is ever "in
// flight" at a time (M5 is synchronous, one vCPU), so there is never a
// need to remember more than the *current* test's own resume point.
//
// ## The M5-G adversarial-sweep find/fix: one descriptor per LINE, plus a
// static build-time bound (ledger `machine.console.transcript-pinned`)
//
// The original design (this module doc, pre-fix) span one
// `__wrela_ring_write` call — and so one consumed descriptor — per
// *piece* of a report line: a passing test alone cost 2 calls (the
// `test <name>: ` prefix, then `ok\n`); a failing test cost 3-4 (prefix,
// `FAILED `, the message, the newline; `__wrela_abort_val`'s own
// interpolated shape costs one more). With `console::QUEUE_SIZE` fixed
// at 16, a mere 7-8 `@test(runtime)` fns — ordinary, not adversarial —
// silently exhausted the queue mid-summary: `__wrela_ring_write` (below,
// pre-fix) treated a full queue as a silent no-op, so the transcript
// truncated (`"7 passed,"` then nothing) and `wrela test` failed with
// "the wrela VMM's own transcript is not well-formed". Found by the
// M5-G sweep's own adversarial pass, reproduced with a plain 8-test file
// of trivial `assert 1 == 1`-shaped tests.
//
// The fix makes 02 §12.2's "statically bounded output" literally true,
// in two parts:
//
// 1. **One descriptor per printed line, never per call.** Three new
//    fixed subroutines replace the old single `__wrela_ring_write`:
//    `__wrela_line_begin` (snapshots the data-bump cursor as the new
//    line's own start, `machine_info::OFF_LINE_START`), `__wrela_ring_
//    append` (copies bytes into the data region and advances the data-
//    bump cursor — *no* descriptor published, *no* doorbell rung), and
//    `__wrela_line_commit` (publishes exactly *one* descriptor spanning
//    `[OFF_LINE_START, current data-bump)`, then rings the doorbell).
//    Every report line — a test's `ok`/`FAILED ...` line, or the one
//    summary line — is now `line_begin`, one or more `ring_append`
//    calls, `line_commit`: exactly one descriptor, regardless of how
//    many pieces compose the line. `build_entry_driver`/
//    `build_abort_fixed`/`build_abort_val` (below) all follow this
//    pattern; an abort continues (never restarts) the line the entry
//    driver's own prefix already began, so a `FAILED` line is still one
//    descriptor covering "test <name>: FAILED ...\n" in full.
// 2. **A static bound, checked at test-image *build* time, never at
//    runtime.** `check_transcript_bound` (below) computes the worst-case
//    line count (`runtime_tests.len() + 1`) and worst-case byte count
//    (every test's own worst-case line — the longer of its "ok" line or
//    a "FAILED" line whose message is over-approximated by the longest
//    string already interned in `program.rodata`, module doc on
//    `check_transcript_bound` itself has the exact formula) *before*
//    `layout_test_image` ever assembles a single instruction. Either
//    bound exceeded fails the build closed with a named diagnostic
//    (`LayoutError`, surfaced by `bin/wrela.rs`'s existing "could not lay
//    out the test image" wrapper) — never a truncated transcript.
//    `console::QUEUE_SIZE` also grew, 16 -> 256 (`wrela-machine`'s own
//    module doc has the ring-geometry consequences), so an ordinary
//    build with genuinely many tests still fits comfortably; the static
//    check is what makes the bound provably *safe* to rely on, not the
//    bigger number alone.
//
// `__wrela_ring_append`'s own over-capacity behavior changed too: rather
// than silently clamping the write (the old, disclosed simplification),
// it now `BRK`s — a defensive, should-be-unreachable internal-error guard
// (`BRK_LINE_APPEND_OVERFLOW`/`BRK_LINE_COMMIT_OVERFLOW`, below): once
// `check_transcript_bound` has passed, no line the generated driver ever
// composes can exceed either bound, so hitting either `BRK` in practice
// means the static check and the actual generated code have drifted
// apart — a real bug in this module, not a reachable guest condition,
// exactly the same "internal error" framing `layout_program`'s own
// `Reloc` resolution failures already use elsewhere in this file.
//
// **Disclosed simplification of the split-ring contract (unchanged by
// this fix)**: this producer never reorders or skips a descriptor index,
// so nothing here ever populates `avail.ring[]` — the VMM's own console
// model (`wrela-vmm`) reads descriptors `0..avail.idx` directly by index,
// which is exactly what a real virtio consumer would get by walking
// `avail.ring[]` *because* this producer's own `avail.ring[i]` would
// always equal `i`. The `used` ring is never populated or read either
// (M5 has no completion tracking to negotiate: the guest never waits on
// it, and the transcript is read only after the guest halts, decision
// 12).

use crate::encode::Cond;
use wrela_machine::machine_info as mi;

/// Every absolute guest address the harness subroutines below bake in via
/// `Asm::load_imm` — bundled so the exact same generator functions can be
/// re-run in a unit test against a host-mmap'd stand-in region instead of
/// the real (unmapped-in-a-test-process) machine addresses, rather than
/// hand-verifying the encoded bytes by eye (this module's own oracle for
/// the hand-assembled routines below: real execution on this machine's own
/// aarch64 CPU, `#[cfg(test)] mod harness_jit`, below).
#[derive(Debug, Clone, Copy)]
struct HarnessAddrs {
    /// `machine_info::` field base — production: `machine_layout::MACHINE_INFO_BASE`.
    info_base: u64,
    /// `console::RING_BASE` (descriptor table + avail ring + doorbell).
    ring_base: u64,
    /// `console::DATA_BASE` (the byte buffers descriptors point into).
    data_base: u64,
    /// `mmio::EXIT_MMIO_ADDR` — only the entry driver's own summary/halt
    /// tail uses this (never `ring_write`/`fmt_dec`/the abort stubs);
    /// unexercised by the JIT self-tests above (which never call the
    /// entry driver directly — a real MMIO trap needs a real VMM, item
    /// E's own boot golden is that routine's oracle).
    exit_mmio_addr: u64,
}

impl HarnessAddrs {
    fn production() -> HarnessAddrs {
        HarnessAddrs {
            info_base: machine_layout::MACHINE_INFO_BASE,
            ring_base: console::RING_BASE,
            data_base: console::DATA_BASE,
            exit_mmio_addr: mmio::EXIT_MMIO_ADDR,
        }
    }
}

/// A tiny, self-contained word-list builder for the hand-assembled harness
/// routines below — distinct from `codegen.rs`'s `FnCtx` (which is built
/// around mwir's own per-instruction two-pass sizing scheme; this module's
/// code is never generated from mwir at all, just written directly, one
/// fixed shape per fn) but the same spirit: `start` is this fragment's own
/// absolute word index within the eventual combined "entry" section (all
/// of `__wrela_ring_write`/`__wrela_fmt_dec`/`__wrela_abort`/
/// `__wrela_abort_val`/the entry driver are assembled into *one* combined
/// word list, in that fixed order, module doc above), so every local call/
/// branch between them is a directly computed `BL`/`B`/`B.cond` — no
/// `Reloc` needed for anything that stays inside this one section. Only a
/// `Reloc::Call` (an `@test(runtime)` fn, elsewhere in the `code` section)
/// or `Reloc::Rodata` (a literal string, elsewhere in the `rodata` section)
/// crosses out of it, and those reuse the exact same `Reloc` variants/
/// resolution already proven by item D.
struct Asm {
    start: usize,
    words: Vec<u32>,
    relocs: Vec<Reloc>,
}

impl Asm {
    fn new(start: usize) -> Asm {
        Asm {
            start,
            words: Vec::new(),
            relocs: Vec::new(),
        }
    }

    /// This fragment's own current absolute word index (its own `start`
    /// plus how many words it has emitted so far) — the address every
    /// local branch/call computes its delta against.
    fn abs(&self) -> usize {
        self.start + self.words.len()
    }

    /// The real guest-physical address of this fragment's own current
    /// position — `abs()` converted from a word index to a byte address
    /// against `harness_base` (always `machine_layout::IMAGE_BASE`, since
    /// the combined harness section is always placed first, module doc's
    /// own fixed emission order). Needed anywhere a *value a register
    /// will later branch to* is materialized (the landing pad's own
    /// continuation slot) — as opposed to a `BL`/`B`/`B.cond`'s own
    /// PC-relative immediate, which `bl_to`/`b_to`/`patch_cond` already
    /// compute correctly from plain word deltas and never need this.
    fn addr(&self, harness_base: u64) -> u64 {
        harness_base + (self.abs() as u64) * 4
    }

    fn push(&mut self, w: u32) {
        self.words.push(w);
    }

    /// Materializes a 64-bit constant into `reg`, always exactly four
    /// words (`MOVZ` + three unconditional `MOVK`s) — `codegen.rs`'s own
    /// `load_imm`, duplicated here rather than threaded through as a
    /// shared helper (this module's fragments are not `FnCtx`s; CLAUDE.md's
    /// "prefer long obvious files" licenses the small duplication over a
    /// generic seam neither side otherwise needs).
    fn load_imm(&mut self, reg: u8, value: u64) {
        let h0 = (value & 0xFFFF) as u16;
        let h1 = ((value >> 16) & 0xFFFF) as u16;
        let h2 = ((value >> 32) & 0xFFFF) as u16;
        let h3 = ((value >> 48) & 0xFFFF) as u16;
        self.push(encode::enc_movz(reg, h0, 0, true));
        self.push(encode::enc_movk(reg, h1, 16, true));
        self.push(encode::enc_movk(reg, h2, 32, true));
        self.push(encode::enc_movk(reg, h3, 48, true));
    }

    /// Emits a placeholder `load_imm reg, #0` (four words), remembering
    /// where it started so `patch_load_imm` can later overwrite it with
    /// the real value once known — the entry driver's own forward
    /// reference for the landing pad's continuation address (module doc
    /// above): the value (the address of the *next* test's own setup)
    /// isn't known until after this test's whole pass-path block has been
    /// emitted.
    fn load_imm_placeholder(&mut self, reg: u8) -> usize {
        let m = self.words.len();
        self.load_imm(reg, 0);
        m
    }

    fn patch_load_imm(&mut self, marker: usize, reg: u8, value: u64) {
        let h0 = (value & 0xFFFF) as u16;
        let h1 = ((value >> 16) & 0xFFFF) as u16;
        let h2 = ((value >> 32) & 0xFFFF) as u16;
        let h3 = ((value >> 48) & 0xFFFF) as u16;
        self.words[marker] = encode::enc_movz(reg, h0, 0, true);
        self.words[marker + 1] = encode::enc_movk(reg, h1, 16, true);
        self.words[marker + 2] = encode::enc_movk(reg, h2, 32, true);
        self.words[marker + 3] = encode::enc_movk(reg, h3, 48, true);
    }

    /// A local `BL` to another word already placed at absolute index
    /// `target_abs` within this same combined section — no `Reloc`
    /// needed (module doc above): both this call site's own position and
    /// the callee's own start are already known Rust-side values by the
    /// time any caller of this fn runs (module doc's own fixed emission
    /// order: ring_write, fmt_dec, abort_fixed, abort_val, entry).
    fn bl_to(&mut self, target_abs: usize) {
        let this = self.abs();
        let delta = (target_abs as i64 - this as i64) * 4;
        self.push(encode::enc_bl(delta as i32));
    }

    /// `B` (not `BL`) to `target_abs` — used by the digit/copy loops'
    /// own backward branch, where the target word is already known
    /// (it was emitted earlier in this same fragment).
    fn b_to(&mut self, target_abs: usize) {
        let this = self.abs();
        let delta = (target_abs as i64 - this as i64) * 4;
        self.push(encode::enc_b(delta as i32));
    }

    /// A `BL` to an `@test(runtime)` fn — a real `Reloc::Call` (the target
    /// lives in the `code` section, placed elsewhere, base unknown until
    /// the whole image is laid out) — `layout_test_image`'s own resolution
    /// loop patches it exactly like an ordinary compiled call.
    fn bl_call_key(&mut self, key: &str) {
        let w = self.abs();
        self.push(encode::enc_bl(0));
        self.relocs.push(Reloc::Call {
            word: w,
            key: key.to_string(),
        });
    }

    /// `reg = &rodata[byte_offset]` (symbolic `ADRP`+`ADD`, `Reloc::Rodata`,
    /// item D's own resolution unchanged) — `byte_offset` is an *already
    /// interned* rodata entry's own offset (see `RodataAppend`, below);
    /// this fn only ever emits code, never interns.
    fn load_rodata_addr_at(&mut self, reg: u8, byte_offset: usize) {
        let w = self.abs();
        self.push(encode::enc_adrp(reg, 0));
        self.push(encode::enc_add_imm(reg, reg, 0, true));
        self.relocs.push(Reloc::Rodata {
            word_adrp: w,
            byte_offset,
        });
    }

    /// A forward conditional branch whose target isn't known yet — mirrors
    /// `codegen.rs`'s own `emit_skip`/`patch_skip` (`SkipKind`), a small,
    /// deliberate duplicate for the same reason `load_imm` is (this
    /// module's fragments are not `FnCtx`s).
    fn skip_placeholder(&mut self) -> usize {
        let w = self.words.len();
        self.push(0);
        w
    }

    fn patch_cond(&mut self, marker: usize, cond: Cond) {
        let target = self.abs();
        let this = self.start + marker;
        let delta = (target as i64 - this as i64) * 4;
        self.words[marker] = encode::enc_b_cond(cond, delta as i32);
    }

    fn patch_cbz(&mut self, marker: usize, reg: u8) {
        let target = self.abs();
        let this = self.start + marker;
        let delta = (target as i64 - this as i64) * 4;
        self.words[marker] = encode::enc_cbz(reg, delta as i32, true);
    }

    fn patch_cbnz(&mut self, marker: usize, reg: u8) {
        let target = self.abs();
        let this = self.start + marker;
        let delta = (target as i64 - this as i64) * 4;
        self.words[marker] = encode::enc_cbnz(reg, delta as i32, true);
    }
}

/// Appends one literal byte string to the growing rodata pool (shared
/// across the whole test image: `program.rodata`'s own already-interned
/// entries, plus every harness literal this module adds after them) and
/// returns its own byte offset within the eventual concatenated rodata
/// section — the same value `Reloc::Rodata::byte_offset` needs, computed
/// the identical way `codegen.rs`'s private `RodataPool::byte_offset`
/// does, just against a plain `(Vec<Vec<u8>>, running-total-cursor)` pair
/// instead of a `BTreeMap` index (no dedup here: every harness string is
/// already used at most a handful of times and interned at most once by
/// its own call site below, so content-addressing would add bookkeeping
/// this module does not need).
fn append_rodata(rodata: &mut Vec<Vec<u8>>, cursor: &mut usize, bytes: Vec<u8>) -> usize {
    let off = *cursor;
    *cursor += bytes.len();
    rodata.push(bytes);
    off
}

/// BRK immediate operands for the two "should be unreachable" internal-
/// error guards below (module doc's own "M5-G adversarial-sweep find/fix"
/// section) — distinct only so a post-mortem guest memory/register dump
/// can tell the two apart at a glance, exactly like `EXIT_CODE_*` above.
pub const BRK_LINE_APPEND_OVERFLOW: u16 = 0xB0A0;
pub const BRK_LINE_COMMIT_OVERFLOW: u16 = 0xB0A1;

/// `__wrela_line_begin()`. Snapshots the current data-bump cursor
/// (`machine_info::OFF_RING_DATA_BUMP`) into `machine_info::
/// OFF_LINE_START` — the anchor `__wrela_line_commit` (below) later reads
/// back to know where the line-in-progress started. Called once, right
/// before the first `__wrela_ring_append` of a new report line (module
/// doc's own "one descriptor per LINE" section).
///
/// Register use: `x9` = `OFF_RING_DATA_BUMP`'s address; `x10` = its
/// value; `x11` = `OFF_LINE_START`'s address.
fn build_line_begin(addrs: &HarnessAddrs, start: usize) -> Asm {
    let mut a = Asm::new(start);
    let data_bump_addr = addrs.info_base + mi::OFF_RING_DATA_BUMP;
    let line_start_addr = addrs.info_base + mi::OFF_LINE_START;

    a.load_imm(9, data_bump_addr);
    a.push(encode::enc_ldr_x_imm(10, 9, 0)); // x10 = data_bump
    a.load_imm(11, line_start_addr);
    a.push(encode::enc_str_x_imm(10, 11, 0));
    a.push(encode::enc_ret(30));
    a
}

/// `__wrela_ring_append(x0=src_ptr, x1=len)`. Copies `len` bytes from
/// `src_ptr` into the next free bytes of `console::DATA_SIZE`, advancing
/// the data-bump cursor — **no descriptor is published and no doorbell is
/// rung here** (module doc's own "one descriptor per LINE" section):
/// that is `__wrela_line_commit`'s (below) job alone, once, after every
/// piece of a line has been appended. An ordinary leaf `RET`, like the
/// old combined routine this replaces.
///
/// Unlike the pre-fix `__wrela_ring_write` this replaces, an over-long
/// append is never silently clamped: `check_transcript_bound` (this
/// file, called before `layout_test_image` ever assembles a single
/// instruction) has already proven no line the generated driver composes
/// can exceed `console::DATA_SIZE`, so overflowing here means that proof
/// and this code have drifted apart — a real internal bug, not a
/// reachable guest condition — and this guard `BRK`s rather than
/// approximating (module doc's own "should be unreachable" framing).
///
/// Register use (leaf fn, owns every register it touches): `x9`/`x10` =
/// the data-bump address/value (both intact, untouched by the copy loop,
/// through to the final store); `x11` = remaining capacity, then the
/// destination base address; `x12` = the destination byte address
/// (`data_base + old data_bump`); `x13`/`x14`/`x15` = the copy loop's
/// src/dst cursors and remaining count (`x1`, the original `len`, is
/// never itself clobbered by the loop, so it is read again unchanged for
/// the final `data_bump += len`); `x16` = the copy loop's one-byte
/// transfer register.
fn build_ring_append(addrs: &HarnessAddrs, start: usize) -> Asm {
    let mut a = Asm::new(start);
    let data_bump_addr = addrs.info_base + mi::OFF_RING_DATA_BUMP;

    a.load_imm(9, data_bump_addr);
    a.push(encode::enc_ldr_x_imm(10, 9, 0)); // x10 = data_bump (old)
    a.load_imm(11, console::DATA_SIZE);
    a.push(encode::enc_sub_reg(11, 11, 10, true)); // x11 = remaining
    a.push(encode::enc_cmp_reg(1, 11, true)); // len vs remaining
    let skip_ok = a.skip_placeholder(); // b.le .ok
    a.push(encode::enc_brk(BRK_LINE_APPEND_OVERFLOW));
    a.patch_cond(skip_ok, Cond::Le);
    // .ok:
    a.load_imm(11, addrs.data_base);
    a.push(encode::enc_add_reg(12, 11, 10, true)); // x12 = dst = data_base + data_bump
    a.push(encode::enc_mov_reg(13, 0, true)); // x13 = src cursor
    a.push(encode::enc_mov_reg(14, 12, true)); // x14 = dst cursor
    a.push(encode::enc_mov_reg(15, 1, true)); // x15 = remaining count
    let loop_top = a.abs();
    let skip_loop = a.skip_placeholder(); // cbz x15, .done
    a.push(encode::enc_ldrb_imm(16, 13, 0));
    a.push(encode::enc_strb_imm(16, 14, 0));
    a.push(encode::enc_add_imm(13, 13, 1, true));
    a.push(encode::enc_add_imm(14, 14, 1, true));
    a.push(encode::enc_sub_imm(15, 15, 1, true));
    a.b_to(loop_top);
    a.patch_cbz(skip_loop, 15);
    // .done: data_bump += len (x1 is the untouched original len; x9/x10
    // are still the address/old-value from the very top of this fn).
    a.push(encode::enc_add_reg(10, 10, 1, true)); // x10 = old data_bump + len
    a.push(encode::enc_str_x_imm(10, 9, 0));
    a.push(encode::enc_ret(30));
    a
}

/// `__wrela_line_commit()`. Publishes exactly **one** descriptor spanning
/// `[console::DATA_BASE + OFF_LINE_START, console::DATA_BASE +
/// OFF_RING_DATA_BUMP)` — the whole line `__wrela_line_begin`/one-or-more
/// `__wrela_ring_append` calls just composed — bumps `avail.idx`, rings
/// the doorbell, and returns. The direct replacement for the pre-fix
/// `__wrela_ring_write`'s own per-call descriptor-publish half; called
/// once per finished report line (module doc's own "one descriptor per
/// LINE" section), never once per piece.
///
/// The over-capacity guard (`console::QUEUE_SIZE` descriptor slots
/// already spent) is the commit-side twin of `__wrela_ring_append`'s own:
/// `check_transcript_bound` already proved `runtime_tests.len() + 1`
/// (every test's own line, plus the summary) never exceeds
/// `console::QUEUE_SIZE`, so this is the same "should be unreachable"
/// internal-error `BRK`, never a silent no-op.
///
/// Register use: `x9`/`x10` = the desc-bump address/value; `x11`/`x12` =
/// the line-start address/value; `x13`/`x14` = the data-bump address/
/// value (the line's own end); `x15` = the computed line length; `x16` =
/// the line's own start *address* (`data_base + line_start`), then
/// reused as a zero source; `x17` = the descriptor-table byte offset
/// scratch; `x18` = the descriptor-entry/avail/doorbell address scratch,
/// reloaded fresh for each of the three (module's own established style:
/// reloading a small constant via `load_imm` is simpler than threading a
/// value through, CLAUDE.md's "prefer obvious").
fn build_line_commit(addrs: &HarnessAddrs, start: usize) -> Asm {
    let mut a = Asm::new(start);
    let desc_bump_addr = addrs.info_base + mi::OFF_RING_DESC_BUMP;
    let data_bump_addr = addrs.info_base + mi::OFF_RING_DATA_BUMP;
    let line_start_addr = addrs.info_base + mi::OFF_LINE_START;

    a.load_imm(9, desc_bump_addr);
    a.push(encode::enc_ldr_x_imm(10, 9, 0)); // x10 = desc_bump
    a.push(encode::enc_cmp_imm(10, console::QUEUE_SIZE as u16, true));
    let skip_ok = a.skip_placeholder(); // b.lt .ok
    a.push(encode::enc_brk(BRK_LINE_COMMIT_OVERFLOW));
    a.patch_cond(skip_ok, Cond::Lt);
    // .ok:
    a.load_imm(11, line_start_addr);
    a.push(encode::enc_ldr_x_imm(12, 11, 0)); // x12 = line_start
    a.load_imm(13, data_bump_addr);
    a.push(encode::enc_ldr_x_imm(14, 13, 0)); // x14 = data_bump (line end)
    a.push(encode::enc_sub_reg(15, 14, 12, true)); // x15 = len = end - start
    a.load_imm(16, addrs.data_base);
    a.push(encode::enc_add_reg(16, 16, 12, true)); // x16 = line start addr
    a.load_imm(17, console::DESC_ENTRY_SIZE);
    a.push(encode::enc_mul(17, 10, 17, true)); // x17 = desc_bump * 16
    a.load_imm(18, addrs.ring_base + console::DESC_TABLE_OFFSET);
    a.push(encode::enc_add_reg(18, 18, 17, true)); // x18 = desc entry addr
    a.push(encode::enc_str_x_imm(16, 18, 0)); // desc.addr = line start addr
    a.push(encode::enc_str_w_imm(15, 18, 8)); // desc.len = line len
    a.push(encode::enc_mov_reg(9, 31, true)); // x9 = 0 (from xzr)
    a.push(encode::enc_str_w_imm(9, 18, 12)); // desc.flags/next = 0
    // avail.idx = desc_bump + 1 (avail.ring[] is never populated — module
    // doc's own disclosed simplification, unchanged by this fix).
    a.push(encode::enc_add_imm(10, 10, 1, true)); // x10 = desc_bump + 1
    a.push(encode::enc_lsl_imm(9, 10, 16, true)); // x9 = idx << 16 (flags=0)
    a.load_imm(18, addrs.ring_base + console::AVAIL_OFFSET);
    a.push(encode::enc_str_w_imm(9, 18, 0));
    a.load_imm(18, desc_bump_addr);
    a.push(encode::enc_str_x_imm(10, 18, 0));
    // ring the doorbell: store nonzero (module doc: a shared-memory
    // doorbell, never a trap — 06 §5).
    a.load_imm(18, addrs.ring_base + console::DOORBELL_OFFSET);
    a.load_imm(9, 1);
    a.push(encode::enc_str_x_imm(9, 18, 0));
    a.push(encode::enc_ret(30));
    a
}

/// `__wrela_fmt_dec(x0=value, x1=is_signed) -> x0=len`. Renders `value`'s
/// decimal digits (as a signed 64-bit interpretation when `is_signed !=
/// 0`, else unsigned) as ASCII text into the fixed scratch buffer
/// `machine_info::OFF_TEST_LINE_BUF` and returns the byte length written —
/// used both by the summary line's pass/fail counts and by
/// `__wrela_abort_val`'s own runtime-value interpolation (module doc
/// above). A leading `-` is written first when the value is negative and
/// `is_signed != 0`; the magnitude (computed via `SUB xzr, x9` — correct
/// even for `i64::MIN`, whose negation wraps back to the exact unsigned
/// bit pattern of its own magnitude, `2^63`, module doc's own canonical-
/// slot reasoning mirrored here) is then converted digit-by-digit,
/// least-significant first, into the buffer past any sign byte, and
/// reversed in place once the digit count is known.
///
/// Register use (leaf fn, no calls, owns every register it touches):
/// `x9` = the magnitude accumulator (then the reversal loop's second
/// swap temp, once no longer needed); `x10` = `is_signed`, then the neg
/// flag; `x11` = the buffer's fixed base address; `x13` = the write
/// pointer; `x14` = the digits' own start pointer (past any sign byte),
/// remembered for the final in-place reversal; `x15` = digit count;
/// `x16` = the divisor constant `10`, then the reversal loop's `lo`
/// pointer; `x17` = the loop's quotient, then the reversal loop's `hi`
/// pointer; `x18` = the loop's remainder/digit byte, then the reversal
/// loop's first swap temp.
fn build_fmt_dec(addrs: &HarnessAddrs, start: usize) -> Asm {
    let mut a = Asm::new(start);
    let buf_addr = addrs.info_base + mi::OFF_TEST_LINE_BUF;

    a.push(encode::enc_mov_reg(9, 0, true)); // x9 = value
    a.push(encode::enc_mov_reg(10, 1, true)); // x10 = is_signed
    a.load_imm(11, buf_addr); // x11 = buffer base
    a.push(encode::enc_movz(12, 0, 0, true)); // x12 = 0 (neg flag)
    let skip_negcheck = a.skip_placeholder(); // cbz x10, .notneg
    a.push(encode::enc_cmp_imm(9, 0, true));
    let skip_notneg2 = a.skip_placeholder(); // b.ge .notneg
    a.push(encode::enc_movz(12, 1, 0, true)); // neg flag = 1
    a.push(encode::enc_sub_reg(9, 31, 9, true)); // x9 = 0 - x9 (magnitude)
    let notneg = a.abs();
    a.patch_cbz(skip_negcheck, 10);
    a.patch_cond(skip_notneg2, Cond::Ge);
    debug_assert_eq!(notneg, a.abs(), "no code between the two forward targets");

    a.push(encode::enc_mov_reg(13, 11, true)); // x13 = write pointer
    let skip_nosign = a.skip_placeholder(); // cbz x12, .nosign
    a.push(encode::enc_movz(14, 45, 0, true)); // '-'
    a.push(encode::enc_strb_imm(14, 13, 0));
    a.push(encode::enc_add_imm(13, 13, 1, true));
    a.patch_cbz(skip_nosign, 12);
    // .nosign:
    a.push(encode::enc_mov_reg(14, 13, true)); // x14 = digits start
    a.push(encode::enc_movz(15, 0, 0, true)); // x15 = digit count
    a.load_imm(16, 10); // x16 = divisor
    let skip_zero = a.skip_placeholder(); // cbnz x9, .loop
    a.push(encode::enc_movz(17, 48, 0, true)); // '0'
    a.push(encode::enc_strb_imm(17, 13, 0));
    a.push(encode::enc_add_imm(13, 13, 1, true));
    a.push(encode::enc_add_imm(15, 15, 1, true));
    let skip_digits_done_1 = a.skip_placeholder(); // b .digits_done
    let loop_top = a.abs();
    a.patch_cbnz(skip_zero, 9);
    let skip_loop_end = a.skip_placeholder(); // cbz x9, .digits_done
    a.push(encode::enc_udiv(17, 9, 16, true)); // x17 = x9 / 10
    a.push(encode::enc_msub(18, 17, 16, 9, true)); // x18 = x9 - x17*10
    a.push(encode::enc_add_imm(18, 18, 48, true)); // ascii digit
    a.push(encode::enc_strb_imm(18, 13, 0));
    a.push(encode::enc_add_imm(13, 13, 1, true));
    a.push(encode::enc_add_imm(15, 15, 1, true));
    a.push(encode::enc_mov_reg(9, 17, true)); // x9 = quotient
    a.b_to(loop_top);
    let digits_done = a.abs();
    a.patch_cbz(skip_loop_end, 9);
    // Both "digit count is zero" and "loop exhausted" paths land here.
    let this = a.start + skip_digits_done_1;
    let delta = (digits_done as i64 - this as i64) * 4;
    a.words[skip_digits_done_1] = encode::enc_b(delta as i32);
    // .digits_done: reverse [x14 .. x14+x15) in place.
    a.push(encode::enc_mov_reg(16, 14, true)); // x16 = lo
    a.push(encode::enc_add_reg(17, 14, 15, true)); // x17 = hi = x14+x15
    a.push(encode::enc_sub_imm(17, 17, 1, true));
    let rev_top = a.abs();
    a.push(encode::enc_cmp_reg(16, 17, true));
    let skip_rev_done = a.skip_placeholder(); // b.ge .rev_done
    a.push(encode::enc_ldrb_imm(18, 16, 0));
    a.push(encode::enc_ldrb_imm(9, 17, 0));
    a.push(encode::enc_strb_imm(9, 16, 0));
    a.push(encode::enc_strb_imm(18, 17, 0));
    a.push(encode::enc_add_imm(16, 16, 1, true));
    a.push(encode::enc_sub_imm(17, 17, 1, true));
    a.b_to(rev_top);
    a.patch_cond(skip_rev_done, Cond::Ge);
    // .rev_done: len = write_ptr(x13) - base(x11).
    a.push(encode::enc_sub_reg(0, 13, 11, true));
    a.push(encode::enc_ret(30));
    a
}

/// The shared tail every abort body ends in: increment
/// `machine_info::OFF_TEST_FAILED` and long-jump to the landing pad's own
/// continuation address (module doc's own "landing pad" section) — never
/// `RET`. Clobbers `x9`/`x10`.
fn push_abort_tail(a: &mut Asm, addrs: &HarnessAddrs) {
    a.load_imm(9, addrs.info_base + mi::OFF_TEST_FAILED);
    a.push(encode::enc_ldr_x_imm(10, 9, 0));
    a.push(encode::enc_add_imm(10, 10, 1, true));
    a.push(encode::enc_str_x_imm(10, 9, 0));
    a.load_imm(9, addrs.info_base + mi::OFF_TEST_CONTINUATION);
    a.push(encode::enc_ldr_x_imm(9, 9, 0));
    a.push(encode::enc_br(9));
}

/// `__wrela_abort(x0=msg_ptr, x1=msg_len) -> noreturn` — the test-image
/// variant (module doc's "Item E instead adds a second ... `__wrela_abort`"
/// paragraph): appends `FAILED ` (shared literal) then the caller's own
/// fixed message, then a newline, to the *already-open* report line
/// `build_entry_driver`'s own prefix append began (module doc's own "one
/// descriptor per LINE" section — an abort continues that line, it never
/// begins a new one), commits it as the one descriptor covering the whole
/// `test <name>: FAILED ...\n` line, then runs the landing pad's own tail
/// (above). `msg_ptr`/`msg_len` are stashed on the stack across the two
/// `__wrela_ring_append` calls that need it (`x0`-`x18` are all
/// caller-saved under this ABI, module doc above — nothing survives a
/// `BL` on its own).
fn build_abort_fixed(
    addrs: &HarnessAddrs,
    start: usize,
    append_start: usize,
    commit_start: usize,
    failed_word_off: usize,
    newline_off: usize,
) -> Asm {
    let mut a = Asm::new(start);
    a.push(encode::enc_sub_imm(31, 31, 16, true)); // sub sp, sp, #16
    a.push(encode::enc_str_x_imm(0, 31, 0));
    a.push(encode::enc_str_x_imm(1, 31, 8));

    a.load_rodata_addr_at(0, failed_word_off);
    a.load_imm(1, 7);
    a.bl_to(append_start);

    a.push(encode::enc_ldr_x_imm(0, 31, 0));
    a.push(encode::enc_ldr_x_imm(1, 31, 8));
    a.bl_to(append_start);

    a.load_rodata_addr_at(0, newline_off);
    a.load_imm(1, 1);
    a.bl_to(append_start);

    a.push(encode::enc_add_imm(31, 31, 16, true)); // add sp, sp, #16
    a.bl_to(commit_start);
    push_abort_tail(&mut a, addrs);
    a
}

/// `__wrela_abort_val(x0=prefix_ptr, x1=prefix_len, x2=value,
/// x3=value_signed, x4=suffix_ptr, x5=suffix_len) -> noreturn` — the
/// test-image variant: appends `FAILED `, the prefix, `value` rendered as
/// decimal (via `__wrela_fmt_dec`), the suffix, then a newline, onto the
/// already-open line (same "continue, don't restart" rule as
/// `build_abort_fixed` above), commits it as one descriptor, then the
/// landing-pad tail. All six incoming args are stashed on the stack up
/// front (48 bytes) and reloaded around each of the four
/// `__wrela_ring_append`/one `__wrela_fmt_dec` calls that clobber them.
fn build_abort_val(
    addrs: &HarnessAddrs,
    start: usize,
    append_start: usize,
    commit_start: usize,
    fmt_dec_start: usize,
    failed_word_off: usize,
    newline_off: usize,
) -> Asm {
    let mut a = Asm::new(start);
    a.push(encode::enc_sub_imm(31, 31, 48, true));
    for (i, reg) in [0u8, 1, 2, 3, 4, 5].into_iter().enumerate() {
        a.push(encode::enc_str_x_imm(reg, 31, (i * 8) as u16));
    }

    a.load_rodata_addr_at(0, failed_word_off);
    a.load_imm(1, 7);
    a.bl_to(append_start);

    a.push(encode::enc_ldr_x_imm(0, 31, 0));
    a.push(encode::enc_ldr_x_imm(1, 31, 8));
    a.bl_to(append_start); // prefix

    a.push(encode::enc_ldr_x_imm(0, 31, 16));
    a.push(encode::enc_ldr_x_imm(1, 31, 24));
    a.bl_to(fmt_dec_start); // x0 = len, written into OFF_TEST_LINE_BUF
    a.push(encode::enc_mov_reg(1, 0, true));
    a.load_imm(0, addrs.info_base + mi::OFF_TEST_LINE_BUF);
    a.bl_to(append_start);

    a.push(encode::enc_ldr_x_imm(0, 31, 32));
    a.push(encode::enc_ldr_x_imm(1, 31, 40));
    a.bl_to(append_start); // suffix

    a.load_rodata_addr_at(0, newline_off);
    a.load_imm(1, 1);
    a.bl_to(append_start);

    a.push(encode::enc_add_imm(31, 31, 48, true));
    a.bl_to(commit_start);
    push_abort_tail(&mut a, addrs);
    a
}

/// The runtime test image's own entry driver (module doc's "Why the entry
/// driver needs no runtime loop at all"): installs core 0's stack pointer,
/// zeroes every harness counter, then one straight-line block per
/// `@test(runtime)` fn in `runtime_tests`' own order — begin a new report
/// line, append `test <name>: `, arm the landing pad's own continuation
/// slot, `BL` the test, append `ok\n` and commit the line (one descriptor
/// covering the whole `test <name>: ok\n` text) and increment the passed
/// counter on an ordinary return (an abort anywhere inside that `BL`'s own
/// call tree instead continues and commits *this same* line itself —
/// `build_abort_fixed`/`build_abort_val`'s own doc — then lands directly at
/// the top of the *next* block, module doc's own landing-pad section) —
/// then the one merged summary line (begin/append/append/append/append/
/// commit, identically) and the exit-code/halt tail. `x8` is set to the
/// fixed `OFF_TEST_LINE_BUF` scratch address before every test call as a
/// defensive measure (this ABI's own aggregate-return convention writes
/// through whatever `x8` holds; a test fn's return value is otherwise
/// unread, but this guarantees an aggregate return, if one ever exists, has
/// somewhere harmless to land rather than an arbitrary stale address).
#[allow(clippy::too_many_arguments)]
fn build_entry_driver(
    addrs: &HarnessAddrs,
    start: usize,
    harness_base: u64,
    line_begin_start: usize,
    append_start: usize,
    commit_start: usize,
    fmt_dec_start: usize,
    abort_fixed_start: usize,
    runtime_tests: &[String],
    // The park-and-resume additions: which tests are async (compiled
    // state machines whose calls return TURN_STATUS_* — a sync test's
    // return value must never be misread as a status), and where the
    // scheduler tick lives. `rt_run_one_start` is `None` only when no
    // runtime glue exists at all — in which case no test can be async
    // either (an async test is itself a flow fn, which forces the glue
    // block into existence via its own free-turn area).
    async_tests: &std::collections::BTreeSet<String>,
    rt_run_one_start: Option<usize>,
    // plans/M6.md item E: `__wrela_checkpoint_service`'s own harness-
    // absolute word index (module doc on `build_checkpoint_and_vector_stub`)
    // — the park-resume path below calls it directly (06 §4: "the guest
    // observes vectors only at checkpoints and parks", and the park's own
    // resume point *is* one, by construction).
    checkpoint_service_word: usize,
    rodata: &mut Vec<Vec<u8>>,
    rodata_cursor: &mut usize,
    boot_init_start: Option<usize>,
    test_args: &BTreeMap<String, Vec<u64>>,
) -> Asm {
    let mut a = Asm::new(start);
    let sp_top = machine_layout::core_stack_base(0) + machine_layout::CORE_STACK_SIZE;

    a.load_imm(9, sp_top);
    a.push(encode::enc_add_imm(31, 9, 0, true)); // mov sp, x9

    a.push(encode::enc_movz(9, 0, 0, true)); // x9 = 0
    for off in [
        mi::OFF_TEST_PASSED,
        mi::OFF_TEST_FAILED,
        mi::OFF_RING_DATA_BUMP,
        mi::OFF_RING_DESC_BUMP,
        mi::OFF_LINE_START,
    ] {
        a.load_imm(10, addrs.info_base + off);
        a.push(encode::enc_str_x_imm(9, 10, 0));
    }

    // The deadlock diagnostic's message, interned once for every async
    // test's own scheduler loop below (`DEADLOCK_MSG`;
    // `compute_transcript_bound` accounts for it explicitly).
    let deadlock_off = if async_tests.is_empty() {
        None
    } else {
        Some(append_rodata(
            rodata,
            rodata_cursor,
            DEADLOCK_MSG.as_bytes().to_vec(),
        ))
    };

    // plans/M6.md item D: the real boot sequence C deferred — every
    // actor's own state gets zero-initialized (`build_boot_init`) before
    // any root turn (`@test(runtime)` fn) ever runs, so an actor call
    // reaches deterministic, in-range memory rather than whatever the
    // rtdata section's own zeroed-at-load bytes already were (which is
    // itself all-zero at M6 — this call is what makes that a *documented*
    // fact rather than an accident of `layout_program`'s own "reserved,
    // zeroed bytes" section, and the one hook a later item's own real
    // `init`-arg materialization/dependency-order walk extends). Absent
    // entirely for a sync-only image (`boot_init_start: None`) — no
    // actors, nothing to boot.
    if let Some(boot_init) = boot_init_start {
        a.bl_to(boot_init);
    }

    let ok_off = append_rodata(rodata, rodata_cursor, b"ok\n".to_vec());
    let passed_comma_off = append_rodata(rodata, rodata_cursor, b" passed, ".to_vec());
    let failed_tail_off = append_rodata(rodata, rodata_cursor, b" failed\n".to_vec());

    for name in runtime_tests {
        let prefix_bytes = format!("test {name}: ").into_bytes();
        let prefix_len = prefix_bytes.len() as u64;
        let prefix_off = append_rodata(rodata, rodata_cursor, prefix_bytes);

        a.bl_to(line_begin_start);

        a.load_rodata_addr_at(0, prefix_off);
        a.load_imm(1, prefix_len);
        a.bl_to(append_start);

        let cont_marker = a.load_imm_placeholder(9);
        a.load_imm(10, addrs.info_base + mi::OFF_TEST_CONTINUATION);
        a.push(encode::enc_str_x_imm(9, 10, 0));

        // plans/M6.md decision 11b: a test's own already-resolved
        // `Actor[T]` handle values (build-time actor indices) load into
        // x0.., in declared param order — a plain zero-param test (every
        // pre-decision-11b test) loads nothing here, byte-identical.
        if let Some(vals) = test_args.get(name) {
            for (i, v) in vals.iter().enumerate() {
                a.load_imm(i as u8, *v);
            }
        }

        a.load_imm(8, addrs.info_base + mi::OFF_TEST_LINE_BUF);
        a.bl_call_key(name);

        // The root turn's own scheduler loop (async tests only — the
        // root test turn parks/resumes through the IDENTICAL turn-record
        // machinery an actor turn uses, via its own free-turn area):
        // while the test reports TURN_STATUS_SUSPENDED, drive the
        // scheduler — re-enter the test the moment its reply arrives
        // (`resume_ready`), otherwise run one ready actor turn-slice
        // (`rt_run_one` — this interleaving is exactly 04 §2's "awaiting
        // a dependency lets other actors run"), and if NOTHING is ready
        // while the root is still incomplete, no progress is possible:
        // abort with the named deadlock diagnostic (prints `FAILED
        // <DEADLOCK_MSG>` on this test's own line, lands at the next
        // test block via the ordinary landing pad, image exits nonzero).
        // A sync test never enters this loop: its return value in x0 is
        // an ordinary value, not a status word.
        if async_tests.contains(name) {
            let rt_run_one = rt_run_one_start
                .expect("an async test forces the runtime glue block into existence");
            let ddl_off =
                deadlock_off.expect("deadlock message interned whenever async tests exist");
            let status_loop_top = a.abs();
            let skip_done = a.skip_placeholder(); // cbz x0, .done
            let drive_top = a.abs();
            // NB: `Asm` relocs carry ABSOLUTE word indices (the
            // `bl_call_key` convention) — `abs()`, not `words.len()`.
            let root_area_word = a.abs();
            a.load_imm(9, 0); // patched: x9 = &this test's own turn area
            a.relocs.push(Reloc::TurnFrameAddr {
                word: root_area_word,
                key: name.clone(),
            });
            a.push(encode::enc_ldr_x_imm(
                10,
                9,
                crate::codegen::OFF_TURN_RESUME_READY as u16,
            ));
            let skip_reenter = a.skip_placeholder(); // cbnz x10, .reenter
            a.bl_to(rt_run_one);
            {
                // cbnz x0, .drive_top (backward — a slice ran; try again)
                let this = a.abs();
                let delta = (drive_top as i64 - this as i64) * 4;
                a.push(encode::enc_cbnz(0, delta as i32, true));
            }
            // Nothing ready. plans/M6.md item E, decision 7/06 §5: park
            // iff a deadline is pending (`OFF_NEXT_DEADLINE != 0`);
            // otherwise item D's own deadlock diagnostic still applies
            // unchanged — no deadline and nothing ready is no progress,
            // ever (the park path below can never turn a real deadlock
            // into a hang: it only ever fires when *something* names a
            // future wake, which item F's groups are the only real M6
            // producer of — conformance tests exercise it via
            // hand-arranged state, exactly like D's own deadlock test).
            a.load_imm(9, addrs.info_base + mi::OFF_NEXT_DEADLINE);
            a.push(encode::enc_ldr_x_imm(10, 9, 0));
            let skip_park = a.skip_placeholder(); // cbz x10, .deadlock
            // Park (06 §5's own protocol): the deadline is already resident
            // at `OFF_NEXT_DEADLINE` (a real group's expiry write is item
            // F's job; conformance hand-arranges it here, mirroring D's
            // own hand-arranged deadlock state) — x10 already holds it.
            // The trapping store to `PARK_MMIO_ADDR` is the park itself;
            // the VMM reads the real deadline back from
            // `OFF_NEXT_DEADLINE`, not from the value stored here.
            a.load_imm(9, wrela_machine::mmio::PARK_MMIO_ADDR);
            a.push(encode::enc_str_x_imm(10, 9, 0));
            // Resumed: the VMM slept until the deadline (or was woken
            // sooner), raised the vector, and resumed this vCPU with PC
            // advanced past the trapping store above. The park's own
            // resume point is a checkpoint by construction (06 §4:
            // "observed only at checkpoints and parks") — service it
            // directly, unconditionally, then retry the scheduler.
            a.bl_to(checkpoint_service_word);
            a.b_to(drive_top);
            let deadlock = a.abs();
            a.patch_cbz(skip_park, 10);
            debug_assert_eq!(deadlock, a.abs());
            // Deadlock: root not ready, nothing else ready, no deadline
            // pending either — no progress is possible, ever.
            a.load_rodata_addr_at(0, ddl_off);
            a.load_imm(1, DEADLOCK_MSG.len() as u64);
            a.bl_to(abort_fixed_start); // noreturn (landing pad)
            let reenter = a.abs();
            a.patch_cbnz(skip_reenter, 10);
            debug_assert_eq!(reenter, a.abs());
            a.bl_call_key(name); // resume (the fn's own discriminant routes)
            a.b_to(status_loop_top);
            let done = a.abs();
            a.patch_cbz(skip_done, 0);
            debug_assert_eq!(done, a.abs());
        }

        a.load_rodata_addr_at(0, ok_off);
        a.load_imm(1, 3);
        a.bl_to(append_start);

        a.bl_to(commit_start);

        a.load_imm(9, addrs.info_base + mi::OFF_TEST_PASSED);
        a.push(encode::enc_ldr_x_imm(10, 9, 0));
        a.push(encode::enc_add_imm(10, 10, 1, true));
        a.push(encode::enc_str_x_imm(10, 9, 0));

        let cont_target = a.addr(harness_base);
        a.patch_load_imm(cont_marker, 9, cont_target);
    }

    // Summary line: "<passed> passed, <failed> failed\n" — one descriptor,
    // like every test's own line above.
    a.bl_to(line_begin_start);

    a.load_imm(9, addrs.info_base + mi::OFF_TEST_PASSED);
    a.push(encode::enc_ldr_x_imm(0, 9, 0));
    a.push(encode::enc_movz(1, 0, 0, true));
    a.bl_to(fmt_dec_start);
    a.push(encode::enc_mov_reg(1, 0, true));
    a.load_imm(0, addrs.info_base + mi::OFF_TEST_LINE_BUF);
    a.bl_to(append_start);

    a.load_rodata_addr_at(0, passed_comma_off);
    a.load_imm(1, 9);
    a.bl_to(append_start);

    a.load_imm(9, addrs.info_base + mi::OFF_TEST_FAILED);
    a.push(encode::enc_ldr_x_imm(0, 9, 0));
    a.push(encode::enc_movz(1, 0, 0, true));
    a.bl_to(fmt_dec_start);
    a.push(encode::enc_mov_reg(1, 0, true));
    a.load_imm(0, addrs.info_base + mi::OFF_TEST_LINE_BUF);
    a.bl_to(append_start);

    a.load_rodata_addr_at(0, failed_tail_off);
    a.load_imm(1, 8);
    a.bl_to(append_start);

    a.bl_to(commit_start);

    // Exit code: 0 if failed==0, else 1 — stored plainly then via the
    // trapping MMIO store (the same two-writes-one-trap shape
    // `push_halt` uses for the ordinary image, decision E's own protocol).
    a.load_imm(9, addrs.info_base + mi::OFF_TEST_FAILED);
    a.push(encode::enc_ldr_x_imm(9, 9, 0));
    a.push(encode::enc_cmp_imm(9, 0, true));
    a.push(encode::enc_cset(10, Cond::Ne, true));
    a.load_imm(11, addrs.info_base + mi::OFF_EXIT_CODE);
    a.push(encode::enc_str_x_imm(10, 11, 0));
    a.load_imm(12, addrs.exit_mmio_addr);
    a.push(encode::enc_str_x_imm(10, 12, 0));
    a.push(encode::enc_brk(0));
    a
}

/// Max decimal digits `__wrela_fmt_dec` ever writes, sign included:
/// `i64::MIN` (`-9223372036854775808`) and `u64::MAX`
/// (`18446744073709551615`) both render as exactly 20 ASCII characters —
/// the widest either the summary line's two counts or an `AbortVal`
/// line's interpolated value can ever be.
const MAX_DECIMAL_DIGITS: u64 = 20;

/// The worst-case transcript shape `check_transcript_bound` (below)
/// checks against `console::QUEUE_SIZE`/`console::DATA_SIZE` — module
/// doc's own "M5-G adversarial-sweep find/fix" section names this as the
/// static-bound half of the fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptBound {
    /// One descriptor per test's own line, plus one for the summary —
    /// exact, not an over-approximation (module doc's own "one descriptor
    /// per LINE" invariant makes this a hard equality, never a bound).
    pub lines: u64,
    /// Every test's own worst-case line length, summed, plus the
    /// summary's own exact worst case (`worst_case_test_line_bytes`/
    /// `worst_case_summary_line_bytes`, below).
    pub worst_case_bytes: u64,
}

/// The longest single string already interned in `program.rodata` — the
/// documented over-approximation this module's static bound stands on
/// (module doc's own `check_transcript_bound` paragraph): rather than
/// tracing which abort site is actually reachable from which test (real
/// call-graph analysis this module does not have), every test's own
/// worst-case *message* is bounded by the single longest literal string
/// anywhere in the whole program's rodata pool, applied uniformly. Sound
/// (it can only over-count, never under-count, since every reachable
/// message is itself one of `program.rodata`'s own entries) and simple;
/// the plan's own explicit license for "the longest string in the whole
/// rodata pool ... documented over-approximation" over the alternative,
/// harder-to-get-right "each test's own longest reachable message".
fn longest_rodata_len(program: &CodegenProgram) -> u64 {
    program
        .rodata
        .iter()
        .map(|entry| entry.len() as u64)
        .max()
        .unwrap_or(0)
}

/// One test's own worst-case printed line length: `"test " + name + ": "`
/// (exact — both are known at compile time), then the *longer* of the two
/// shapes that line can ever take: `"ok\n"` (always exactly 3 bytes) or a
/// `FAILED` line. A `FAILED` line's own worst case covers *both*
/// `__wrela_abort` (one message, `"FAILED " + msg + "\n"`) and
/// `__wrela_abort_val` (`"FAILED " + prefix + <=20 digits + suffix +
/// "\n"`) uniformly, by allowing for *two* copies of the longest rodata
/// string (covers `AbortVal`'s prefix+suffix pair; strictly more than
/// `AbortFixed`'s one-string shape ever needs) plus the full 20-digit
/// interpolation width — an honest over-approximation applied to every
/// test regardless of which shape (if either) it can actually reach.
fn worst_case_test_line_bytes(name: &str, longest_msg: u64) -> u64 {
    let prefix_len = "test ".len() as u64 + name.len() as u64 + ": ".len() as u64;
    let ok_len = "ok\n".len() as u64;
    let failed_len =
        "FAILED ".len() as u64 + 2 * longest_msg + MAX_DECIMAL_DIGITS + "\n".len() as u64;
    prefix_len + ok_len.max(failed_len)
}

/// The summary line's own worst case, exact rather than over-approximated
/// (both counts are `u64`s, bounded by `MAX_DECIMAL_DIGITS` each):
/// `"<=20 digits> passed, <=20 digits> failed\n"`.
fn worst_case_summary_line_bytes() -> u64 {
    2 * MAX_DECIMAL_DIGITS + " passed, ".len() as u64 + " failed\n".len() as u64
}

/// Computes the exact worst-case shape `layout_test_image`'s own static
/// bound check (below) enforces — a pure function of `program`/
/// `runtime_tests` alone, callable standalone by unit tests without
/// building a whole image.
pub fn compute_transcript_bound(
    program: &CodegenProgram,
    runtime_tests: &[String],
) -> TranscriptBound {
    // The deadlock diagnostic (`DEADLOCK_MSG`) is a FAILED-line message
    // the *harness* interns, after this bound has already been checked —
    // so it must be accounted for here explicitly, not discovered via
    // `program.rodata` (the park-and-resume redesign's one addition to
    // this bound; still an over-approximation, never an undercount).
    let longest_msg = longest_rodata_len(program).max(DEADLOCK_MSG.len() as u64);
    let mut worst_case_bytes = worst_case_summary_line_bytes();
    for name in runtime_tests {
        worst_case_bytes += worst_case_test_line_bytes(name, longest_msg);
    }
    TranscriptBound {
        lines: runtime_tests.len() as u64 + 1,
        worst_case_bytes,
    }
}

/// The static bound check itself (module doc's own "M5-G adversarial-
/// sweep find/fix" section, part 2): `Err` — a real, named, build-time
/// diagnostic, never a runtime truncation — the moment either dimension
/// of `compute_transcript_bound`'s own result would overflow the
/// machine's fixed console geometry (`wrela-machine`'s own
/// `console::QUEUE_SIZE`/`console::DATA_SIZE`). Called first thing in
/// `layout_test_image`, below, before a single harness instruction is
/// ever assembled — 02 §12.2's "statically bounded output" made literally
/// true at build time, per this item's own task.
pub fn check_transcript_bound(
    program: &CodegenProgram,
    runtime_tests: &[String],
) -> Result<(), LayoutError> {
    let bound = compute_transcript_bound(program, runtime_tests);
    if bound.lines > console::QUEUE_SIZE || bound.worst_case_bytes > console::DATA_SIZE {
        return Err(LayoutError::new(format!(
            "this test image's worst-case transcript ({} byte(s) across {} line(s)) exceeds \
             the machine's console bound ({} byte(s) across {} line(s))",
            bound.worst_case_bytes,
            bound.lines,
            console::DATA_SIZE,
            console::QUEUE_SIZE
        )));
    }
    Ok(())
}

/// Places a codegen'd test-image program into the machine's fixed
/// contract, per module doc above: **one** combined "entry" section
/// (`__wrela_ring_append`, `__wrela_line_begin`, `__wrela_line_commit`,
/// `__wrela_fmt_dec`, `__wrela_abort`, `__wrela_abort_val`, the entry
/// driver, in that fixed order — the order every internal local branch/
/// call above assumes), then `code` (every codegen'd fn, `@test(runtime)`
/// fns included — they are ordinary fns to `codegen.rs`, called by name
/// like any other), then `rodata` (`program.rodata`'s own already-interned
/// entries, followed by every harness literal this fn appends —
/// `append_rodata`, above). Every `Reloc` — the harness's own `Call`/
/// `Rodata` entries and every ordinary compiled fn's `Call`/`Rodata`/
/// `AbortFixed`/`AbortVal` — resolves through the identical `patch_bl`/
/// `patch_adrp_add` this file's item-D half already proved;
/// `AbortFixed`/`AbortVal` targets are simply this section's own
/// `abort_fixed_start`/`abort_val_start` words instead of a separate
/// section, since the test image's `__wrela_abort`/`__wrela_abort_val`
/// symbols *are* these words.
///
/// `Err` for a genuine internal inconsistency, **or** for a program whose
/// worst-case transcript provably cannot fit (`check_transcript_bound`,
/// called first, before anything else here) — module doc mirrors
/// `layout_program`'s own doc here for the former: an out-of-range
/// relocation, or a name in `runtime_tests` `codegen_program` never
/// produced (an internal invariant `bin/wrela.rs`'s own caller is expected
/// to have already checked via `TypedProgram::tests`, kept here anyway as
/// a real `Err` rather than a silent skip).
/// plans/M6.md item D: the three whole-build-closure facts needed to run
/// a real actor boot sequence (`compute_runtime_tables`'s own signature,
/// unchanged) — bundled so `layout_test_image`'s own signature grows by
/// exactly one `Option` parameter rather than three. `None` (the
/// overwhelming majority of today's corpus, and every pre-M6 golden)
/// keeps `layout_test_image` byte-identical to its pre-item-D behavior:
/// no `rtdata`, no boot sequence, no runtime-glue routines.
pub struct BootCtx<'a> {
    pub graph: &'a ImageGraph,
    pub modules: &'a BTreeMap<String, Module>,
    pub layout_ctx: &'a LayoutCtx,
    /// `codegen::async_frame_sizes`' result for this same build — every
    /// async fn's own persistent frame bytes, the park-and-resume
    /// redesign's sizing input (`compute_runtime_tables`'s own doc).
    pub async_frames: &'a BTreeMap<String, u64>,
}

pub fn layout_test_image(
    program: &CodegenProgram,
    runtime_tests: &[String],
    // Which of `runtime_tests` are async (state machines with the
    // TURN_STATUS_* return ABI) — the entry driver's own scheduler loop
    // wraps exactly these (`build_entry_driver`'s own doc comment).
    async_tests: &std::collections::BTreeSet<String>,
    boot: Option<BootCtx>,
    // plans/M6.md decision 11b: every runtime test's own already-resolved
    // `Actor[T]` param values (build-time actor indices, `bin/wrela.rs`'s
    // own `resolve_runtime_test_args`), in declared param order — empty
    // for every test with no params (every pre-decision-11b test, byte-
    // identical).
    test_args: &BTreeMap<String, Vec<u64>>,
) -> Result<ImageLayout, LayoutError> {
    check_transcript_bound(program, runtime_tests)?;

    let image_base = machine_layout::IMAGE_BASE;
    let addrs = HarnessAddrs::production();

    let mut code_words: Vec<u32> = Vec::new();
    let mut fn_word_base: BTreeMap<String, usize> = BTreeMap::new();
    for (key, f) in &program.fns {
        fn_word_base.insert(key.clone(), code_words.len());
        for (w, _text) in &f.code {
            code_words.push(*w);
        }
    }
    for name in runtime_tests {
        if !fn_word_base.contains_key(name) {
            return Err(LayoutError::new(format!(
                "internal error: runtime test `{name}` was never codegen'd"
            )));
        }
    }

    let mut rodata: Vec<Vec<u8>> = program.rodata.clone();
    let mut rodata_cursor: usize = rodata.iter().map(Vec::len).sum();

    // Shared literals used by both abort bodies, interned once, before
    // either is built (both need their byte offsets).
    let failed_word_off = append_rodata(&mut rodata, &mut rodata_cursor, b"FAILED ".to_vec());
    let abort_newline_off = append_rodata(&mut rodata, &mut rodata_cursor, b"\n".to_vec());

    // plans/M6.md item D: the real boot wiring C's own sub-note deferred —
    // `RuntimeTables` (item C's own static sizing pass, unchanged) plus
    // each actor's own dispatch-key list (`"{Actor}.{method}"`, the exact
    // `program.fns` keys `build_rt_select_and_run_symbolic`'s own
    // `Reloc::Call`-based dispatch chain targets — a sync method's real
    // compiled body and an async method's real compiled state machine are
    // both just ordinary `program.fns` entries now, no color-based
    // special-casing anywhere in this fn). `None` when `boot` is absent or
    // the build declares no actors — every pre-M6 call site's own
    // behavior, byte-identical.
    let runtime_tables: Option<RuntimeTables> = match &boot {
        Some(b) => compute_runtime_tables(b.graph, b.modules, b.layout_ctx, b.async_frames)
            .map_err(LayoutError::new)?
            .filter(|t| t.total_bytes > 0),
        None => None,
    };
    let actor_dispatch: Vec<(String, Vec<(String, bool)>)> = match (&runtime_tables, &boot) {
        (Some(tables), Some(b)) => {
            let shapes = merge_actor_pub_methods(b.modules, b.layout_ctx)?;
            tables
                .actors
                .iter()
                .map(|a| {
                    let methods = shapes.get(&a.name).cloned().unwrap_or_default();
                    let keys = methods
                        .iter()
                        .map(|m| (format!("{}.{}", a.name, m.name), m.is_async))
                        .collect();
                    (a.name.clone(), keys)
                })
                .collect()
        }
        _ => Vec::new(),
    };
    let init_keys: BTreeMap<String, String> = match &boot {
        Some(b) if runtime_tables.is_some() => actor_zero_arg_init_keys(b.modules)?,
        _ => BTreeMap::new(),
    };
    let actor_names: Vec<String> = runtime_tables
        .as_ref()
        .map(|t| t.actors.iter().map(|a| a.name.clone()).collect())
        .unwrap_or_default();

    let ring_append_asm = build_ring_append(&addrs, 0);
    let ring_append_start = 0usize;
    let line_begin_start = ring_append_start + ring_append_asm.words.len();
    let line_begin_asm = build_line_begin(&addrs, line_begin_start);
    let line_commit_start = line_begin_start + line_begin_asm.words.len();
    let line_commit_asm = build_line_commit(&addrs, line_commit_start);
    let fmt_dec_start = line_commit_start + line_commit_asm.words.len();
    let fmt_dec_asm = build_fmt_dec(&addrs, fmt_dec_start);
    let abort_fixed_start = fmt_dec_start + fmt_dec_asm.words.len();
    let abort_fixed_asm = build_abort_fixed(
        &addrs,
        abort_fixed_start,
        ring_append_start,
        line_commit_start,
        failed_word_off,
        abort_newline_off,
    );
    let abort_val_start = abort_fixed_start + abort_fixed_asm.words.len();
    let abort_val_asm = build_abort_val(
        &addrs,
        abort_val_start,
        ring_append_start,
        line_commit_start,
        fmt_dec_start,
        failed_word_off,
        abort_newline_off,
    );
    // plans/M6.md item E: `__wrela_checkpoint_service` + its own
    // `__wrela_vector0_service` sibling, the exact same real routine pair
    // `layout_program`'s own `build_checkpoint_and_vector_stub` builds for
    // the ordinary image path — placed once here, in the test harness's
    // own combined word section, since a runtime-test image's compiled
    // fns (`program.fns`, below) can carry `Reloc::CheckpointService`
    // exactly like `Reloc::AbortFixed`/`AbortVal`, and the entry driver's
    // own park-resume path (below) calls the service directly too.
    let checkpoint_start = abort_val_start + abort_val_asm.words.len();
    let (checkpoint_words, checkpoint_service_offset) = build_checkpoint_and_vector_stub();
    let checkpoint_asm = Asm {
        start: checkpoint_start,
        words: checkpoint_words,
        relocs: Vec::new(),
    };
    // `__wrela_checkpoint_service`'s own harness-absolute word index (see
    // `build_checkpoint_and_vector_stub`'s doc: `__wrela_vector0_service`
    // sits first, so the section's own start is never the right target).
    let checkpoint_service_word = checkpoint_start + checkpoint_service_offset;

    // plans/M6.md item D: the runtime-glue routines + boot-init sequence
    // (module docs on `build_runtime_glue_block`/`build_boot_init` above)
    // — absent entirely when no actor exists. Built *twice* when present:
    // once with placeholder (`base=0`) addresses purely to learn the
    // total word count (`build_runtime_glue_block`'s own doc: word count
    // never depends on address *values*), which is what lets
    // `boot_init_start`/`entry_start` — and therefore `entry_asm` itself,
    // built only once — be fixed before `rtdata_base` is even known; then
    // again with the real, now-known addresses once `rtdata_base` is
    // computed below, replacing the placeholder-valued bytes in the final
    // buffer at the identical word offsets.
    let glue_start = checkpoint_start + checkpoint_asm.words.len();
    let (dummy_placement, state_sizes): (RuntimePlacement, Vec<u64>) = match &runtime_tables {
        Some(tables) => (
            place_runtime_tables(0, tables),
            tables.actors.iter().map(|a| a.state_size).collect(),
        ),
        None => (RuntimePlacement::default(), Vec::new()),
    };
    let dummy_glue = runtime_tables.as_ref().map(|tables| {
        build_runtime_glue_block(tables, &actor_dispatch, &dummy_placement, glue_start)
    });
    let glue_words_len: usize = dummy_glue
        .as_ref()
        .map(|g| g.asms.iter().map(|a| a.words.len()).sum())
        .unwrap_or(0);
    let rt_run_one_start = dummy_glue.as_ref().map(|g| g.rt_run_one_start);
    let boot_init_start = glue_start + glue_words_len;
    let dummy_boot_init_asm = build_boot_init(
        &actor_names,
        &dummy_placement.actors,
        &state_sizes,
        &init_keys,
        boot_init_start,
    );
    let boot_init_start_opt = runtime_tables.as_ref().map(|_| boot_init_start);

    let entry_start = boot_init_start + dummy_boot_init_asm.words.len();
    let entry_asm = build_entry_driver(
        &addrs,
        entry_start,
        image_base,
        line_begin_start,
        ring_append_start,
        line_commit_start,
        fmt_dec_start,
        abort_fixed_start,
        runtime_tests,
        async_tests,
        rt_run_one_start,
        checkpoint_service_word,
        &mut rodata,
        &mut rodata_cursor,
        boot_init_start_opt,
        test_args,
    );

    let mut harness_words: Vec<u32> = Vec::new();
    let mut harness_relocs: Vec<Reloc> = Vec::new();
    for asm in [
        ring_append_asm,
        line_begin_asm,
        line_commit_asm,
        fmt_dec_asm,
        abort_fixed_asm,
        abort_val_asm,
        checkpoint_asm,
    ] {
        debug_assert_eq!(asm.start, harness_words.len());
        harness_relocs.extend(asm.relocs);
        harness_words.extend(asm.words);
    }
    debug_assert_eq!(glue_start, harness_words.len());
    if let Some(g) = &dummy_glue {
        for asm in &g.asms {
            harness_words.extend(asm.words.clone());
        }
    }
    debug_assert_eq!(boot_init_start, harness_words.len());
    harness_words.extend(dummy_boot_init_asm.words.clone());
    debug_assert_eq!(entry_start, harness_words.len());
    harness_relocs.extend(entry_asm.relocs.clone());
    harness_words.extend(entry_asm.words.clone());

    // --- place sections: entry(harness), code, rodata? -------------------
    let mut cursor = image_base;
    let harness_base = cursor;
    let harness_size = (harness_words.len() * 4) as u64;
    cursor += harness_size;

    cursor = round_up(cursor, 4);
    let code_base = cursor;
    let code_size = (code_words.len() * 4) as u64;
    cursor += code_size;

    let rodata_bytes: Vec<u8> = rodata.iter().flat_map(|e| e.iter().copied()).collect();
    let rodata_base = if rodata_bytes.is_empty() {
        None
    } else {
        cursor = round_up(cursor, 8);
        Some(cursor)
    };
    if rodata_base.is_some() {
        cursor += rodata_bytes.len() as u64;
    }

    // plans/M6.md item C, decision 3: the `rtdata` section, sized exactly
    // `tables.total_bytes` — this image shape's own final section, mirroring
    // `layout_program`'s identical convention.
    let rtdata_base = runtime_tables.as_ref().map(|tables| {
        cursor = round_up(cursor, 8);
        let base = cursor;
        cursor += tables.total_bytes;
        base
    });

    // Now that `rtdata_base` is real, rebuild the address-dependent
    // fragments (glue routines + boot-init) at the identical word offsets
    // the placeholder pass already reserved — replacing their
    // placeholder-valued bytes in `harness_words` in place.
    let (glue_symbols, real_placement): (BTreeMap<String, usize>, Option<RuntimePlacement>) =
        if let Some(tables) = &runtime_tables {
            let real_base =
                rtdata_base.expect("rtdata reserved above whenever runtime_tables is Some");
            let placement = place_runtime_tables(real_base, tables);
            let real_glue =
                build_runtime_glue_block(tables, &actor_dispatch, &placement, glue_start);
            let mut w = glue_start;
            for asm in &real_glue.asms {
                for word in &asm.words {
                    harness_words[w] = *word;
                    w += 1;
                }
                // `build_rt_select_and_run_symbolic`'s own dispatch chain
                // carries real `Reloc::Call`s (a sync method's real compiled
                // body, or an async method's real state-machine entry) —
                // these must resolve exactly like every other harness-section
                // call, or the emitted `BL` stays a self-referencing
                // placeholder.
                harness_relocs.extend(asm.relocs.clone());
            }
            debug_assert_eq!(w, boot_init_start);
            let real_boot_init_asm = build_boot_init(
                &actor_names,
                &placement.actors,
                &state_sizes,
                &init_keys,
                boot_init_start,
            );
            let mut w = boot_init_start;
            for word in &real_boot_init_asm.words {
                harness_words[w] = *word;
                w += 1;
            }
            harness_relocs.extend(real_boot_init_asm.relocs.clone());
            debug_assert_eq!(w, entry_start);
            (real_glue.symbols, Some(placement))
        } else {
            (BTreeMap::new(), None)
        };

    // Resolves a `Reloc::TurnFrameAddr` key to its real turn-area
    // address (`RuntimePlacement::turn_area_for`'s own rule) — an
    // internal error if no tables exist or the key was never sized.
    let turn_area_addr = |key: &str| -> Result<u64, LayoutError> {
        let (Some(tables), Some(placement)) = (&runtime_tables, &real_placement) else {
            return Err(LayoutError::new(format!(
                "internal error: async fn `{key}` needs a turn area but this image has no \
                 runtime tables"
            )));
        };
        placement.turn_area_for(key, tables).ok_or_else(|| {
            LayoutError::new(format!(
                "internal error: async fn `{key}`'s own turn area was never sized"
            ))
        })
    };

    let mut sections = vec![
        Section {
            name: "entry",
            base: harness_base,
            size: harness_size,
        },
        Section {
            name: "code",
            base: code_base,
            size: code_size,
        },
    ];
    if let Some(rb) = rodata_base {
        sections.push(Section {
            name: "rodata",
            base: rb,
            size: rodata_bytes.len() as u64,
        });
    }
    if let (Some(rb), Some(tables)) = (rtdata_base, &runtime_tables) {
        sections.push(Section {
            name: "rtdata",
            base: rb,
            size: tables.total_bytes,
        });
    }

    // --- resolve relocs ----------------------------------------------------
    for reloc in &harness_relocs {
        match reloc {
            Reloc::Call { word, key } => {
                let target_base = *fn_word_base.get(key).ok_or_else(|| {
                    LayoutError::new(format!(
                        "internal error: harness call target `{key}` was never codegen'd"
                    ))
                })?;
                let this_addr = harness_base + (*word as u64) * 4;
                let target_addr = code_base + (target_base as u64) * 4;
                patch_bl(&mut harness_words, *word, this_addr, target_addr)?;
            }
            Reloc::Rodata {
                word_adrp,
                byte_offset,
            } => {
                let rb = rodata_base.ok_or_else(|| {
                    LayoutError::new(
                        "internal error: a harness Reloc::Rodata exists but the rodata section is empty",
                    )
                })?;
                let this_addr = harness_base + (*word_adrp as u64) * 4;
                let target_addr = rb + *byte_offset as u64;
                patch_adrp_add(&mut harness_words, *word_adrp, this_addr, target_addr)?;
            }
            Reloc::TurnFrameAddr { word, key } => {
                // The entry driver's own root-turn-area loads (the
                // scheduler loop reads `resume_ready` through them).
                let addr = turn_area_addr(key)?;
                patch_load_imm_words(&mut harness_words, *word, addr);
            }
            Reloc::AbortFixed { .. }
            | Reloc::AbortVal { .. }
            | Reloc::CheckpointService { .. }
            | Reloc::GroupArenaBase { .. } => {
                return Err(LayoutError::new(
                    "internal error: the harness section itself must never emit an AbortFixed/AbortVal/CheckpointService/GroupArenaBase reloc",
                ));
            }
        }
    }
    for (key, f) in &program.fns {
        let base = fn_word_base[key];
        for reloc in &f.relocs {
            match reloc {
                Reloc::Call { word, key: target } => {
                    // plans/M6.md item D: an async `Send`/`Await{ActorCall}`
                    // op's own symbolic call target is a per-actor runtime
                    // glue routine (`glue_symbols`, harness-section-relative)
                    // rather than an ordinary `program.fns` entry
                    // (`fn_word_base`, code-section-relative) — checked
                    // second, never both at once for the same key (the two
                    // naming schemes, `rt_enqueue_symbol` vs. plain
                    // `Struct.method`, never collide).
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = if let Some(target_base) = fn_word_base.get(target) {
                        code_base + (*target_base as u64) * 4
                    } else if let Some(glue_word) = glue_symbols.get(target) {
                        harness_base + (*glue_word as u64) * 4
                    } else {
                        return Err(LayoutError::new(format!(
                            "internal error: call target `{target}` was never codegen'd or \
                             registered as a runtime-glue symbol"
                        )));
                    };
                    patch_bl(&mut code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::Rodata {
                    word_adrp,
                    byte_offset,
                } => {
                    let rb = rodata_base.ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::Rodata exists but the rodata section is empty",
                        )
                    })?;
                    let this_addr = code_base + ((base + word_adrp) * 4) as u64;
                    let target_addr = rb + *byte_offset as u64;
                    patch_adrp_add(&mut code_words, base + word_adrp, this_addr, target_addr)?;
                }
                Reloc::AbortFixed { word } => {
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = harness_base + (abort_fixed_start as u64) * 4;
                    patch_bl(&mut code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::AbortVal { word } => {
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = harness_base + (abort_val_start as u64) * 4;
                    patch_bl(&mut code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::CheckpointService { word } => {
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = harness_base + (checkpoint_service_word as u64) * 4;
                    patch_bl(&mut code_words, base + word, this_addr, target_addr)?;
                }
                Reloc::TurnFrameAddr { word, key: fn_key } => {
                    // The compiled async fn's own persistent-frame base
                    // load (its X_FRAME setup) — patched with its turn
                    // area's real `rtdata` address.
                    let addr = turn_area_addr(fn_key)?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                Reloc::GroupArenaBase { word } => {
                    let addr = real_placement
                        .as_ref()
                        .map(|p| p.group_arena)
                        .ok_or_else(|| {
                            LayoutError::new(
                                "internal error: a `with group` op needs the group arena but this \
                             image's runtime tables never sized one"
                                    .to_string(),
                            )
                        })?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
            }
        }
    }

    // --- serialize -----------------------------------------------------
    let mut blob = Vec::new();
    for w in &harness_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    pad_to(&mut blob, image_base, code_base);
    for w in &code_words {
        blob.extend_from_slice(&w.to_le_bytes());
    }
    if let Some(rb) = rodata_base {
        pad_to(&mut blob, image_base, rb);
        blob.extend_from_slice(&rodata_bytes);
    }
    if let (Some(rb), Some(tables)) = (rtdata_base, &runtime_tables) {
        pad_to(&mut blob, image_base, rb);
        blob.resize(blob.len() + tables.total_bytes as usize, 0);
    }

    verify_section_sizes(&sections, image_base, blob.len() as u64)?;

    Ok(ImageLayout {
        blob,
        entry: harness_base + (entry_start as u64) * 4,
        sections,
        // plans/M6.md item D: real at last for an actor-bearing test image
        // (`bin/wrela.rs::test_cmd` now passes a real `BootCtx` — the item-C
        // sub-note's own "staged, named work" is this commit).
        runtime: runtime_tables,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::CodegenFn;

    fn fn_words(words: &[u32]) -> CodegenFn {
        CodegenFn {
            frame_size: 16,
            code: words.iter().map(|w| (*w, String::new())).collect(),
            relocs: Vec::new(),
        }
    }

    // --- plans/M6.md item C: RuntimeTables sizing -------------------------

    fn parse_one_module(src: &str) -> Module {
        let tokens = crate::syntax::lexer::lex(src).expect("lex");
        crate::syntax::parser::parse(tokens).expect("parse")
    }

    fn one_module(name: &str, src: &str) -> BTreeMap<String, Module> {
        let mut m = BTreeMap::new();
        m.insert(name.to_string(), parse_one_module(src));
        m
    }

    fn actor_decl(actor_type: &str, mailbox: Option<u32>) -> crate::eval::image::ActorDecl {
        use crate::eval::image::DeclArg;
        use crate::eval::value::Value;
        use crate::sema::types::Type;
        let mut args = Vec::new();
        if let Some(n) = mailbox {
            args.push(DeclArg {
                label: "mailbox".to_string(),
                ty: Type::U32,
                value: Value::U32(n),
            });
        }
        crate::eval::image::ActorDecl {
            actor_type: Type::Named(actor_type.to_string(), vec![]),
            args,
        }
    }

    #[test]
    fn compute_runtime_tables_is_none_for_a_sync_only_image() {
        let modules = one_module("m", "module m\n\nfn f():\n    pass\n");
        let graph = ImageGraph::default();
        let ctx = merge_layout_ctx(&modules).unwrap();
        let out = compute_runtime_tables(&graph, &modules, &ctx, &BTreeMap::new()).unwrap();
        assert!(
            out.is_none(),
            "no actors and no async fns -> no runtime tables at all"
        );
    }

    #[test]
    fn compute_runtime_tables_sizes_state_mailbox_and_slot() {
        let src = "\
module m

@actor
pub struct Store:
    count: u32
    total: u64

    init(mut self):
        self.count = 0
        self.total = 0

    pub fn get(read self) -> u64:
        return self.total

    pub fn bump(mut self, by: u32) -> u64:
        self.total = self.total + by.to[u64]()
        return self.total
";
        let modules = one_module("m", src);
        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Store", Some(4)));
        let ctx = merge_layout_ctx(&modules).unwrap();
        let tables = compute_runtime_tables(&graph, &modules, &ctx, &BTreeMap::new())
            .unwrap()
            .expect("one actor -> Some");
        assert_eq!(tables.actors.len(), 1);
        let a = &tables.actors[0];
        assert_eq!(a.name, "Store");
        assert_eq!(a.mailbox_capacity, 4);
        // state: two u32/u64 fields, each one 8-byte slot (mwir's own
        // "one 8-byte-slot layout rule") -> 16 bytes.
        assert_eq!(a.state_size, 16);
        // slot: 8-byte method tag + 8-byte waker + the widest pub
        // method's own args (`bump`'s one `u32` param, one slot) -> 24;
        // `get` has none.
        assert_eq!(a.slot_size, 24);
        // No async method -> the turn area is exactly the 48-byte record.
        assert_eq!(a.frame_size, crate::codegen::TURN_RECORD_SIZE);
        assert_eq!(tables.ready_queue_capacity, 2); // 1 actor + root
        assert_eq!(tables.group_arena_capacity, 0);
        let expect_total = a.state_size + a.mailbox_capacity as u64 * a.slot_size + 24 /* head/tail/count */ + a.frame_size
                + tables.ready_queue_capacity * 8
                + 8; // rr cursor
        assert_eq!(tables.total_bytes, expect_total);
    }

    #[test]
    fn compute_runtime_tables_fails_closed_without_a_declared_mailbox() {
        let modules = one_module(
            "m",
            "module m\n\n@actor\npub struct Store:\n    count: u32\n\n    init(mut self):\n        self.count = 0\n",
        );
        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Store", None));
        let ctx = merge_layout_ctx(&modules).unwrap();
        let err = compute_runtime_tables(&graph, &modules, &ctx, &BTreeMap::new()).unwrap_err();
        assert!(err.contains("mailbox"));
    }

    #[test]
    fn private_methods_never_contribute_to_the_message_slot_size() {
        let src = "\
module m

@actor
pub struct Store:
    count: u64

    init(mut self):
        self.count = 0

    pub fn get(read self) -> u64:
        return self.count

    fn helper(read self, huge: u64, more: u64, extra: u64) -> u64:
        return huge + more + extra
";
        let modules = one_module("m", src);
        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Store", Some(2)));
        let ctx = merge_layout_ctx(&modules).unwrap();
        let tables = compute_runtime_tables(&graph, &modules, &ctx, &BTreeMap::new())
            .unwrap()
            .unwrap();
        // `get` (pub, no params) is the only message shape; the private
        // `helper`'s own three-`u64`-param body never widens the slot
        // past the 16-byte idx+waker floor.
        assert_eq!(tables.actors[0].slot_size, 16);
    }

    #[test]
    fn count_with_group_sites_counts_every_with_group_and_none_else() {
        let src = "\
module m

async fn one():
    with group(capacity=1) as g:
        pass

fn two():
    if true:
        with group(capacity=2) as g:
            pass
";
        let modules = one_module("m", src);
        assert_eq!(count_with_group_sites(&modules), 2);
    }

    #[test]
    fn count_with_group_sites_is_zero_with_no_with_statements() {
        let modules = one_module("m", "module m\n\nfn f():\n    pass\n");
        assert_eq!(count_with_group_sites(&modules), 0);
    }

    // --- BL reloc math (incl. negative offsets) --------------------------

    #[test]
    fn patch_bl_encodes_a_forward_offset() {
        let mut words = vec![0u32];
        patch_bl(&mut words, 0, 0x1000, 0x1010).unwrap();
        assert_eq!(words[0], encode::enc_bl(0x10));
    }

    #[test]
    fn patch_bl_encodes_a_negative_offset() {
        let mut words = vec![0u32];
        patch_bl(&mut words, 0, 0x2000, 0x1000).unwrap();
        assert_eq!(words[0], encode::enc_bl(-0x1000));
    }

    #[test]
    fn patch_bl_fails_closed_out_of_range() {
        let mut words = vec![0u32];
        let far = 1u64 << 40;
        assert!(patch_bl(&mut words, 0, 0, far).is_err());
    }

    // --- ADRP page math --------------------------------------------------

    #[test]
    fn patch_adrp_add_same_page() {
        // ADRP rd=x9 placeholder, ADD rd=x9,rn=x9 placeholder.
        let mut words = vec![encode::enc_adrp(9, 0), encode::enc_add_imm(9, 9, 0, true)];
        // this_addr and target_addr share a page: page_delta == 0.
        patch_adrp_add(&mut words, 0, 0x4050_0004, 0x4050_0ABC).unwrap();
        assert_eq!(words[0], encode::enc_adrp(9, 0));
        assert_eq!(words[1], encode::enc_add_imm(9, 9, 0x0ABC, true));
    }

    #[test]
    fn patch_adrp_add_crossing_pages_backward() {
        let mut words = vec![
            encode::enc_adrp(10, 0),
            encode::enc_add_imm(10, 10, 0, true),
        ];
        // this_addr's page is one page above target_addr's page.
        patch_adrp_add(&mut words, 0, 0x4050_1000, 0x4050_0040).unwrap();
        assert_eq!(words[0], encode::enc_adrp(10, -1));
        assert_eq!(words[1], encode::enc_add_imm(10, 10, 0x040, true));
    }

    // --- section packing / alignment -------------------------------------

    #[test]
    fn layout_places_entry_then_code_then_abort_when_rodata_is_empty() {
        let mut fns = BTreeMap::new();
        fns.insert("f".to_string(), fn_words(&[0xAABB_CCDD]));
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
        };
        let out = layout_program(&program, None).unwrap();
        let names: Vec<&str> = out.sections.iter().map(|s| s.name).collect();
        assert_eq!(names, ["entry", "code", "abort", "checkpoint"]);
        assert_eq!(out.entry, machine_layout::IMAGE_BASE);
        // No gaps/overlaps, and the blob is exactly as long as the last
        // section's own end implies.
        let last = out.sections.last().unwrap();
        assert_eq!(
            out.blob.len() as u64,
            last.base + last.size - machine_layout::IMAGE_BASE
        );
    }

    #[test]
    fn layout_places_rodata_between_code_and_abort_when_nonempty() {
        let mut fns = BTreeMap::new();
        fns.insert("f".to_string(), fn_words(&[0x1111_1111]));
        let program = CodegenProgram {
            fns,
            rodata: vec![b"hello".to_vec()],
        };
        let out = layout_program(&program, None).unwrap();
        let names: Vec<&str> = out.sections.iter().map(|s| s.name).collect();
        assert_eq!(names, ["entry", "code", "rodata", "abort", "checkpoint"]);
        let rodata = out.sections.iter().find(|s| s.name == "rodata").unwrap();
        // 8-byte aligned base.
        assert_eq!(rodata.base % 8, 0);
        assert_eq!(rodata.size, 5);
    }

    #[test]
    fn call_reloc_resolves_to_the_callees_own_base() {
        let mut fns = BTreeMap::new();
        // `g` calls `f`; `f`'s own code sorts before `g`'s (BTree order).
        let mut g = fn_words(&[0]);
        g.relocs.push(Reloc::Call {
            word: 0,
            key: "f".to_string(),
        });
        fns.insert("f".to_string(), fn_words(&[0x1111_1111, 0x2222_2222]));
        fns.insert("g".to_string(), g);
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
        };
        let out = layout_program(&program, None).unwrap();
        let code = out.sections.iter().find(|s| s.name == "code").unwrap();
        // g's own word 0 is at code_base + 2 words (f is 2 words, first in
        // BTree order); f's own base is code_base + 0.
        let g_word0_addr = code.base + 8;
        let f_base = code.base;
        let delta = f_base as i64 - g_word0_addr as i64;
        let expect = encode::enc_bl(delta as i32);
        let word_index_in_blob = ((g_word0_addr - machine_layout::IMAGE_BASE) / 4) as usize;
        let bytes = &out.blob[word_index_in_blob * 4..word_index_in_blob * 4 + 4];
        let got = u32::from_le_bytes(bytes.try_into().unwrap());
        assert_eq!(got, expect);
    }

    #[test]
    fn unresolved_call_target_is_an_internal_error() {
        let mut fns = BTreeMap::new();
        let mut g = fn_words(&[0]);
        g.relocs.push(Reloc::Call {
            word: 0,
            key: "missing".to_string(),
        });
        fns.insert("g".to_string(), g);
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
        };
        assert!(layout_program(&program, None).is_err());
    }

    // --- section-size verification ---------------------------------------

    #[test]
    fn verify_section_sizes_accepts_a_contiguous_table() {
        let sections = vec![
            Section {
                name: "entry",
                base: 0x1000,
                size: 16,
            },
            Section {
                name: "code",
                base: 0x1010,
                size: 32,
            },
        ];
        assert!(verify_section_sizes(&sections, 0x1000, 48).is_ok());
    }

    #[test]
    fn verify_section_sizes_rejects_a_wrong_blob_length() {
        let sections = vec![Section {
            name: "entry",
            base: 0x1000,
            size: 16,
        }];
        assert!(verify_section_sizes(&sections, 0x1000, 8).is_err());
    }

    #[test]
    fn verify_section_sizes_rejects_overlap() {
        let sections = vec![
            Section {
                name: "a",
                base: 0x1000,
                size: 16,
            },
            Section {
                name: "b",
                base: 0x1008,
                size: 16,
            },
        ];
        assert!(verify_section_sizes(&sections, 0x1000, 24).is_err());
    }

    #[test]
    fn verify_section_sizes_rejects_first_section_not_at_image_base() {
        let sections = vec![Section {
            name: "entry",
            base: 0x2000,
            size: 16,
        }];
        assert!(verify_section_sizes(&sections, 0x1000, 16).is_err());
    }

    // --- determinism -------------------------------------------------------

    #[test]
    fn layout_program_is_deterministic() {
        let mut fns = BTreeMap::new();
        fns.insert("f".to_string(), fn_words(&[0x1111_1111]));
        let program = CodegenProgram {
            fns,
            rodata: vec![b"x".to_vec()],
        };
        let a = layout_program(&program, None).unwrap();
        let b = layout_program(&program, None).unwrap();
        assert_eq!(a, b);
    }

    // --- static transcript bound (M5-G adversarial-sweep find/fix) ---------

    fn program_with_rodata(longest: &[u8]) -> CodegenProgram {
        CodegenProgram {
            fns: BTreeMap::new(),
            rodata: vec![longest.to_vec()],
        }
    }

    #[test]
    fn transcript_bound_counts_one_line_per_test_plus_the_summary() {
        let program = program_with_rodata(b"boom");
        let tests = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let bound = compute_transcript_bound(&program, &tests);
        assert_eq!(bound.lines, 4); // 3 tests + 1 summary
    }

    #[test]
    fn transcript_bound_grows_with_the_longest_rodata_string() {
        let short = program_with_rodata(b"x");
        let long = program_with_rodata(&vec![b'x'; 500]);
        let tests = vec!["t".to_string()];
        let short_bound = compute_transcript_bound(&short, &tests);
        let long_bound = compute_transcript_bound(&long, &tests);
        assert!(long_bound.worst_case_bytes > short_bound.worst_case_bytes);
        // The over-approximation counts the longest message *twice* (an
        // AbortVal's prefix+suffix pair) plus up to 20 interpolated
        // digits. The short program's own floor is `DEADLOCK_MSG` (a
        // harness-interned FAILED-line message, accounted explicitly).
        assert_eq!(
            long_bound.worst_case_bytes - short_bound.worst_case_bytes,
            2 * (500 - DEADLOCK_MSG.len() as u64)
        );
    }

    #[test]
    fn transcript_bound_with_no_rodata_still_covers_the_deadlock_message() {
        let program = CodegenProgram {
            fns: BTreeMap::new(),
            rodata: Vec::new(),
        };
        let tests = vec!["only_test".to_string()];
        let bound = compute_transcript_bound(&program, &tests);
        // "test " (5) + "only_test" (9) + ": " (2) = 16, plus
        // max(3, 7 + 2*len(DEADLOCK_MSG) + 20 + 1) for the one test line
        // (the deadlock diagnostic is the longest message even with an
        // empty rodata pool), plus the summary's own exact 2*20+9+8=57.
        let failed_len = 7 + 2 * DEADLOCK_MSG.len() as u64 + 20 + 1;
        assert_eq!(bound.worst_case_bytes, 16 + failed_len + 57);
    }

    #[test]
    fn check_transcript_bound_accepts_an_ordinary_small_program() {
        let program = program_with_rodata(b"short message");
        let tests: Vec<String> = (0..10).map(|i| format!("test_{i}")).collect();
        assert!(check_transcript_bound(&program, &tests).is_ok());
    }

    #[test]
    fn check_transcript_bound_rejects_too_many_lines() {
        let program = program_with_rodata(b"x");
        let tests: Vec<String> = (0..(console::QUEUE_SIZE as usize))
            .map(|i| format!("t{i}"))
            .collect();
        // QUEUE_SIZE tests + 1 summary line = QUEUE_SIZE + 1 lines: over.
        let err = check_transcript_bound(&program, &tests).unwrap_err();
        assert!(
            err.message.contains("exceeds the machine's console bound"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn check_transcript_bound_rejects_a_worst_case_byte_overflow() {
        // One test whose rodata pool holds a message long enough alone to
        // blow the byte bound, well under the line-count bound.
        let huge = vec![b'x'; (console::DATA_SIZE) as usize];
        let program = program_with_rodata(&huge);
        let tests = vec!["one_test".to_string()];
        assert!(check_transcript_bound(&program, &tests).is_err());
    }
}

// ===========================================================================
// Item E's own oracle for the hand-assembled harness routines above: real
// execution, on this machine's own aarch64 CPU (every development/check
// host this project targets is either Apple Silicon or aarch64 Linux —
// CLAUDE.md's own machine — so this is never a cross-architecture
// emulation trick). No assembler exists to cross-check these bytes
// against (decision 5), so instead of hand-verifying each encoding by eye
// the way `encode.rs`'s own unit tests do for single instructions, this
// writes the generated words into an executable page and calls them as
// an ordinary `extern "C" fn` — the *behavior* is the oracle, exactly the
// same principle decision 5 already states for the VMM/`diff-eval` at the
// whole-image level, applied here one level down, to routines that touch
// no machine-specific absolute address at all (`fmt_dec`) or that touch
// only a host-mmap'd stand-in region (`ring_write`, via `HarnessAddrs`'s
// own test-vs-production split, module doc above) — never the real,
// unmapped-in-a-test-process `wrela_machine` constants.
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod harness_jit {
    use super::*;
    use std::ffi::c_void;

    unsafe extern "C" {
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: i32,
            flags: i32,
            fd: i32,
            offset: i64,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> i32;
        fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
    }

    const PROT_READ: i32 = 1;
    const PROT_WRITE: i32 = 2;
    const PROT_EXEC: i32 = 4;
    const MAP_PRIVATE: i32 = 0x0002;
    const MAP_ANON: i32 = 0x1000;

    /// A host page (or run of pages) holding real, callable machine code —
    /// written RW, then flipped to R-X before it is ever called (two
    /// separate `mmap`/`mprotect` steps, never simultaneously W+X, so this
    /// needs no `MAP_JIT`/hardened-runtime entitlement on macOS).
    struct ExecPage {
        ptr: *mut u8,
        len: usize,
    }

    impl ExecPage {
        fn new(words: &[u32]) -> ExecPage {
            let want = words.len() * 4;
            let len = want.div_ceil(4096) * 4096;
            unsafe {
                let p = mmap(
                    std::ptr::null_mut(),
                    len,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANON,
                    -1,
                    0,
                );
                assert!(!p.is_null() && (p as isize) != -1, "mmap failed");
                let bytes = p as *mut u8;
                for (i, w) in words.iter().enumerate() {
                    std::ptr::write_unaligned(bytes.add(i * 4) as *mut u32, *w);
                }
                let r = mprotect(p, len, PROT_READ | PROT_EXEC);
                assert_eq!(r, 0, "mprotect(R-X) failed");
                ExecPage { ptr: bytes, len }
            }
        }

        /// Calls this page as `extern "C" fn(u64, u64) -> u64` — exactly
        /// the shape every harness routine below has (two integer args,
        /// one integer return, AAPCS64's own leaf-call convention, which
        /// is also this internal ABI's convention for these fns, module
        /// doc above): the host CPU's own C calling convention puts the
        /// arguments in `x0`/`x1` and reads the result from `x0`, so an
        /// ordinary Rust `extern "C"` call through a function pointer
        /// genuinely exercises the exact same register-level contract the
        /// generated code was written against.
        fn call2(&self, a0: u64, a1: u64) -> u64 {
            let f: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(self.ptr) };
            f(a0, a1)
        }

        /// The `_at` family: identical shape to `call2` above, but
        /// entering at `byte_offset` into this same page instead of its
        /// very first byte — plans/M6.md item C's own tests combine
        /// several fragments (stand-in "actor method" bodies,
        /// `rt_enqueue`, `rt_select_and_run`) into one JIT'd page, exactly
        /// the M5 harness's own combined-section technique, and need to
        /// call into the *middle* of it.
        fn call0_at(&self, byte_offset: usize) -> u64 {
            assert!(byte_offset < self.len);
            let f: extern "C" fn() -> u64 =
                unsafe { std::mem::transmute(self.ptr.add(byte_offset)) };
            f()
        }

        fn call4_at(&self, byte_offset: usize, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
            assert!(byte_offset < self.len);
            let f: extern "C" fn(u64, u64, u64, u64) -> u64 =
                unsafe { std::mem::transmute(self.ptr.add(byte_offset)) };
            f(a0, a1, a2, a3)
        }
    }

    impl Drop for ExecPage {
        fn drop(&mut self) {
            unsafe {
                munmap(self.ptr as *mut c_void, self.len);
            }
        }
    }

    /// A host-mmap'd stand-in for one page of "guest RAM" a harness
    /// routine reads/writes via absolute addresses baked in at code-gen
    /// time (`HarnessAddrs`) — real memory a test can inspect afterward,
    /// standing in for `console::RING_BASE`/`DATA_BASE`/
    /// `machine_layout::MACHINE_INFO_BASE` without needing those literal,
    /// unmapped-in-this-process addresses to exist.
    struct HostRam {
        ptr: *mut u8,
        len: usize,
    }

    impl HostRam {
        fn new(len: usize) -> HostRam {
            let len = len.div_ceil(4096) * 4096;
            unsafe {
                let p = mmap(
                    std::ptr::null_mut(),
                    len,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANON,
                    -1,
                    0,
                );
                assert!(!p.is_null() && (p as isize) != -1, "mmap failed");
                // Pre-fault every page by writing it once, up front —
                // exactly what the real VMM does to the whole guest DRAM
                // region before ever starting the vCPU ("zeroes the
                // declared reservations", 06-machine.md §3): an untouched
                // anonymous mapping's pages are backed by the shared,
                // read-only system zero page until first written, and a
                // write performed by *JIT'd code running under this same
                // process* was observed (empirically, chasing down a real
                // test failure) to not reliably fault such a page in on
                // this host — pre-touching here removes that difference
                // between this test harness and the VMM's own always-
                // zeroed-first memory model, rather than working around a
                // JIT-only artifact this module's actual production code
                // path never hits.
                std::ptr::write_bytes(p as *mut u8, 0, len);
                HostRam {
                    ptr: p as *mut u8,
                    len,
                }
            }
        }

        fn base(&self) -> u64 {
            self.ptr as u64
        }

        fn read_u64(&self, off: u64) -> u64 {
            assert!((off as usize) + 8 <= self.len);
            unsafe { std::ptr::read_unaligned(self.ptr.add(off as usize) as *const u64) }
        }

        fn read_u32(&self, off: u64) -> u32 {
            assert!((off as usize) + 4 <= self.len);
            unsafe { std::ptr::read_unaligned(self.ptr.add(off as usize) as *const u32) }
        }

        fn read_bytes(&self, off: u64, n: usize) -> Vec<u8> {
            assert!((off as usize) + n <= self.len);
            unsafe { std::slice::from_raw_parts(self.ptr.add(off as usize), n).to_vec() }
        }

        /// Plans/M6.md item C: pre-seeding a ring's `count`/`head`/`tail`
        /// (ring-full/FIFO-order test setup) needs writes, not just reads
        /// — every M5-era harness test only ever *reads* `HostRam` after
        /// letting generated code write it; this item's own tests are the
        /// first to need the reverse.
        fn write_u64(&self, off: u64, value: u64) {
            assert!((off as usize) + 8 <= self.len);
            unsafe { std::ptr::write_unaligned(self.ptr.add(off as usize) as *mut u64, value) }
        }
    }

    impl Drop for HostRam {
        fn drop(&mut self) {
            unsafe {
                munmap(self.ptr as *mut c_void, self.len);
            }
        }
    }

    fn words_of(asm: &Asm) -> Vec<u32> {
        asm.words.clone()
    }

    // --- __wrela_fmt_dec ---------------------------------------------------
    //
    // No machine address at all beyond the scratch buffer — a plain
    // `HostRam` page stands in for `machine_info::OFF_TEST_LINE_BUF`
    // directly (the fn's own `HarnessAddrs::info_base` field), no offset
    // math needed since `mi::OFF_TEST_LINE_BUF` is folded in by
    // `build_fmt_dec` itself, exactly as production does it.

    fn fmt_dec_call(value: i64, is_signed: bool) -> (u64, String) {
        let ram = HostRam::new(4096);
        // Offset the fake info_base backward so `info_base +
        // OFF_TEST_LINE_BUF` still lands inside the mmap'd page (a real
        // guest address would too, by construction — this just avoids
        // needing a second page).
        let addrs = HarnessAddrs {
            info_base: ram.base(),
            ring_base: ram.base(),
            data_base: ram.base(),
            exit_mmio_addr: 0,
        };
        let asm = build_fmt_dec(&addrs, 0);
        assert!(asm.relocs.is_empty(), "fmt_dec must need no Reloc");
        let page = ExecPage::new(&words_of(&asm));
        let len = page.call2(value as u64, if is_signed { 1 } else { 0 });
        let bytes = ram.read_bytes(mi::OFF_TEST_LINE_BUF, len as usize);
        (len, String::from_utf8(bytes).expect("ascii digits"))
    }

    #[test]
    fn fmt_dec_zero() {
        assert_eq!(fmt_dec_call(0, false), (1, "0".to_string()));
        assert_eq!(fmt_dec_call(0, true), (1, "0".to_string()));
    }

    #[test]
    fn fmt_dec_positive_unsigned() {
        assert_eq!(fmt_dec_call(1, false), (1, "1".to_string()));
        assert_eq!(fmt_dec_call(42, false), (2, "42".to_string()));
        assert_eq!(fmt_dec_call(12345, false), (5, "12345".to_string()));
    }

    #[test]
    fn fmt_dec_u64_max() {
        let (len, s) = fmt_dec_call(u64::MAX as i64, false);
        assert_eq!(s, u64::MAX.to_string());
        assert_eq!(len as usize, s.len());
    }

    #[test]
    fn fmt_dec_negative_signed() {
        assert_eq!(fmt_dec_call(-5, true), (2, "-5".to_string()));
        assert_eq!(fmt_dec_call(-123456, true), (7, "-123456".to_string()));
    }

    #[test]
    fn fmt_dec_i64_min_signed() {
        // The one value whose negation overflows a 64-bit register — the
        // canonical-slot trick (module doc) must still render the exact
        // magnitude via unsigned wraparound.
        let (len, s) = fmt_dec_call(i64::MIN, true);
        assert_eq!(s, i64::MIN.to_string());
        assert_eq!(len as usize, s.len());
    }

    #[test]
    fn fmt_dec_negative_value_but_unsigned_flag_renders_as_huge_unsigned() {
        // `is_signed=false` on a bit pattern that looks negative as i64
        // must render its full *unsigned* magnitude — exactly the
        // canonical-slot invariant `codegen.rs` documents (an unsigned
        // register's value is never reinterpreted as signed).
        let (_len, s) = fmt_dec_call(-1i64, false);
        assert_eq!(s, u64::MAX.to_string());
    }

    // --- __wrela_line_begin / __wrela_ring_append / __wrela_line_commit ----
    //
    // The M5-G fix's own three-way split of the old combined
    // `__wrela_ring_write` (module doc's "one descriptor per LINE"
    // section): a shared `HostRam` stand-in for info/ring/data (one
    // combined page region, exactly like the pre-fix tests below used),
    // with one small helper per routine so a test can freely interleave
    // `line_begin`/`ring_append`*/`line_commit` calls the same way the
    // generated entry driver/abort bodies do.

    struct LineHarness {
        ram: HostRam,
        addrs: HarnessAddrs,
    }

    impl LineHarness {
        fn new() -> LineHarness {
            // `console::RING_SIZE` is 2 pages now (`QUEUE_SIZE` grew to
            // 256, `wrela-machine`'s own module doc has the geometry
            // story) — the ring region needs 2 host pages here too, not
            // 1, or `console::AVAIL_OFFSET` (4096, since the 256-entry
            // desc table alone is exactly one page) lands past this
            // stand-in's own "ring" region and into "data", exactly the
            // aliasing bug this comment is recording so it is never
            // reintroduced.
            let ram = HostRam::new(4096 * 8);
            let addrs = HarnessAddrs {
                info_base: ram.base(),
                ring_base: ram.base() + 4096,
                data_base: ram.base() + 4096 * 3,
                exit_mmio_addr: 0,
            };
            LineHarness { ram, addrs }
        }

        fn line_begin(&self) {
            let asm = build_line_begin(&self.addrs, 0);
            assert!(asm.relocs.is_empty(), "line_begin must need no Reloc");
            ExecPage::new(&words_of(&asm)).call2(0, 0);
        }

        fn ring_append(&self, src: &[u8]) {
            let asm = build_ring_append(&self.addrs, 0);
            assert!(asm.relocs.is_empty(), "ring_append must need no Reloc");
            let page = ExecPage::new(&words_of(&asm));
            // src lives in its own host buffer so the call passes a real
            // pointer distinct from the fake "guest RAM" page.
            let src_ram = HostRam::new(src.len().max(1));
            unsafe {
                std::ptr::copy_nonoverlapping(src.as_ptr(), src_ram.ptr, src.len());
            }
            page.call2(src_ram.base(), src.len() as u64);
        }

        fn line_commit(&self) {
            let asm = build_line_commit(&self.addrs, 0);
            assert!(asm.relocs.is_empty(), "line_commit must need no Reloc");
            ExecPage::new(&words_of(&asm)).call2(0, 0);
        }
    }

    // Offsets relative to `ram.base()` — `info_base`/`ring_base`/
    // `data_base` are `ram.base() + 0`/`+ 4096`/`+ 4096*3` (`LineHarness::new`
    // above; the ring region is 2 pages, matching `console::RING_SIZE`).
    const INFO: u64 = 0;
    const RING: u64 = 4096;
    const DATA: u64 = 4096 * 3;

    #[test]
    fn one_line_from_several_appends_publishes_exactly_one_descriptor() {
        let h = LineHarness::new();
        h.line_begin();
        h.ring_append(b"test foo: ");
        h.ring_append(b"ok");
        h.ring_append(b"\n");
        h.line_commit();

        // Exactly one descriptor and one data-bump advance for the whole
        // line, no matter how many `ring_append` calls composed it — the
        // fix's own central invariant.
        assert_eq!(h.ram.read_u64(INFO + mi::OFF_RING_DESC_BUMP), 1);
        assert_eq!(h.ram.read_u64(INFO + mi::OFF_RING_DATA_BUMP), 13);

        let desc_addr = h.ram.read_u64(RING + console::DESC_TABLE_OFFSET);
        assert_eq!(desc_addr, h.ram.base() + DATA);
        assert_eq!(
            h.ram.read_u32(RING + console::DESC_TABLE_OFFSET + 8),
            13,
            "desc.len covers the whole composed line, not one append"
        );
        assert_eq!(h.ram.read_u32(RING + console::AVAIL_OFFSET), 1u32 << 16);
        assert_eq!(h.ram.read_u64(RING + console::DOORBELL_OFFSET), 1);
        assert_eq!(h.ram.read_bytes(DATA, 13), b"test foo: ok\n".to_vec());
    }

    #[test]
    fn a_second_line_gets_the_next_descriptor_and_continues_the_data_cursor() {
        let h = LineHarness::new();
        h.line_begin();
        h.ring_append(b"hello\n");
        h.line_commit();

        h.line_begin();
        h.ring_append(b"world");
        h.ring_append(b"!\n");
        h.line_commit();

        assert_eq!(h.ram.read_u64(INFO + mi::OFF_RING_DESC_BUMP), 2);
        assert_eq!(h.ram.read_u64(INFO + mi::OFF_RING_DATA_BUMP), 6 + 7);

        let desc0_addr = h.ram.read_u64(RING + console::DESC_TABLE_OFFSET);
        let desc1_addr = h
            .ram
            .read_u64(RING + console::DESC_TABLE_OFFSET + console::DESC_ENTRY_SIZE);
        assert_eq!(desc0_addr, h.ram.base() + DATA);
        assert_eq!(desc1_addr, h.ram.base() + DATA + 6);
        assert_eq!(
            h.ram
                .read_u32(RING + console::DESC_TABLE_OFFSET + console::DESC_ENTRY_SIZE + 8),
            7,
            "second desc.len"
        );
        assert_eq!(h.ram.read_bytes(DATA, 6), b"hello\n".to_vec());
        assert_eq!(h.ram.read_bytes(DATA + 6, 7), b"world!\n".to_vec());
        assert_eq!(h.ram.read_u32(RING + console::AVAIL_OFFSET), 2u32 << 16);
    }

    #[test]
    fn many_lines_can_exceed_the_old_16_descriptor_bound() {
        // The M5-G bug's exact shape, at the routine level: the old
        // combined `__wrela_ring_write` spent a descriptor per *call*, so
        // 16 short lines already exhausted `console::QUEUE_SIZE` when it
        // was 16. Composing each line from a `line_begin`/`ring_append`/
        // `line_commit` triple, one descriptor per line, comfortably
        // clears the old bound (proving the fix, not just the new
        // `QUEUE_SIZE`, is what makes this work: 20 > the *old* 16).
        let h = LineHarness::new();
        for i in 0..20u8 {
            h.line_begin();
            h.ring_append(&[b'A' + i]);
            h.line_commit();
        }
        assert_eq!(h.ram.read_u64(INFO + mi::OFF_RING_DESC_BUMP), 20);
        assert_eq!(h.ram.read_u64(INFO + mi::OFF_RING_DATA_BUMP), 20);
        for i in 0..20u8 {
            assert_eq!(h.ram.read_bytes(DATA + i as u64, 1), vec![b'A' + i]);
        }
    }

    // --- rt_enqueue / rt_select_and_run / rt_run_one ------------------------
    //
    // Stand-in "actor method" bodies (hand-assembled, the identical ABI a
    // real compiled method uses) stand in for compiled `pub fn`s /
    // `pub async fn` state machines, combined into one JIT'd page
    // alongside the real runtime routines — the M5 harness's own
    // combined-section technique, one level up. Sync stand-ins:
    // `add x0, x1, #N; ret` (self in x0 unread, one scalar arg in x1,
    // reply in x0). The async stand-in below implements the full
    // park-and-resume fn contract (`codegen::OFF_TURN_*`) by hand.

    use crate::codegen::{
        OFF_TURN_BUSY, OFF_TURN_REPLY, OFF_TURN_RESUME_READY, OFF_TURN_SUSPENDED, TURN_RECORD_SIZE,
        TURN_STATUS_COMPLETED, TURN_STATUS_SUSPENDED,
    };

    fn stand_in_method(add_const: u16) -> Vec<u32> {
        vec![
            encode::enc_add_imm(0, 1, add_const, true),
            encode::enc_ret(30),
        ]
    }

    /// One actor's own region in a `HostRam` page, laid out exactly the
    /// way `place_runtime_tables` places a real actor (state, ring,
    /// head/tail/count, turn area), plus one detached stand-in **waker
    /// record** (`waker`) an enqueued message can name so a test can
    /// observe reply delivery — standing in for the awaiting turn's own
    /// turn area.
    struct ActorFixture {
        ram: HostRam,
        addrs: ActorAddrs,
        waker: u64,
    }

    impl ActorFixture {
        fn new(capacity: u64, slot_size: u64) -> ActorFixture {
            let state_size: u64 = 8;
            let ram = HostRam::new(4096);
            let base = ram.base();
            let ring = base + state_size;
            let head = ring + capacity * slot_size;
            let addrs = ActorAddrs {
                state: base,
                ring,
                head,
                tail: head + 8,
                count: head + 16,
                turn: head + 24,
            };
            let waker = addrs.turn + TURN_RECORD_SIZE + 64; // detached record, well past the turn area
            ActorFixture { ram, addrs, waker }
        }

        fn rel(&self, addr: u64) -> u64 {
            addr - self.ram.base()
        }

        fn read(&self, addr: u64) -> u64 {
            self.ram.read_u64(self.rel(addr))
        }

        fn write(&self, addr: u64, v: u64) {
            self.ram.write_u64(self.rel(addr), v);
        }
    }

    #[test]
    fn rt_enqueue_admits_fifo_carries_the_waker_and_rejects_when_full() {
        let capacity: u64 = 2;
        let slot_size: u64 = 24; // idx + waker + one scalar arg
        let f = ActorFixture::new(capacity, slot_size);
        let addrs = f.addrs;

        let mut combined: Vec<u32> = Vec::new();
        let method0_start = combined.len();
        combined.extend(stand_in_method(1)); // arg + 1
        let method1_start = combined.len();
        combined.extend(stand_in_method(2)); // arg + 2
        let enqueue_start = combined.len();
        combined.extend(build_rt_enqueue(&addrs, capacity, slot_size, enqueue_start));
        let select_start = combined.len();
        combined.extend(build_rt_select_and_run(
            &addrs,
            capacity,
            slot_size,
            &[(method0_start, false), (method1_start, false)],
            select_start,
        ));

        let page = ExecPage::new(&combined);
        let enqueue_off = enqueue_start * 4;
        let select_off = select_start * 4;

        let arg10: u64 = 10;
        let arg20: u64 = 20;
        let arg30: u64 = 30;

        assert_eq!(
            page.call4_at(enqueue_off, 0, &arg10 as *const u64 as u64, 1, f.waker),
            0,
            "first enqueue admitted"
        );
        assert_eq!(
            page.call4_at(enqueue_off, 1, &arg20 as *const u64 as u64, 1, f.waker),
            0,
            "second enqueue admitted"
        );
        assert_eq!(f.read(addrs.count), 2);

        // A third, over capacity=2: rejected, ring state untouched
        // (02 §9.4: an outcome that did not consume arguments hands them
        // back — the minimal encoding is simply "never mutated").
        assert_eq!(
            page.call4_at(enqueue_off, 0, &arg30 as *const u64 as u64, 1, f.waker),
            1,
            "ring full -> rejected"
        );
        assert_eq!(
            f.read(addrs.count),
            2,
            "a rejected enqueue must not touch count"
        );

        // FIFO dispatch; each completion delivers to the waker record.
        assert_eq!(page.call0_at(select_off), 1, "ran the first queued turn");
        assert_eq!(
            f.read(f.waker + OFF_TURN_REPLY),
            11,
            "FIFO: (method 0, arg 10) enqueued first, dispatched first; reply delivered to the waker"
        );
        assert_eq!(
            f.read(f.waker + OFF_TURN_RESUME_READY),
            1,
            "delivery marks the waker ready to resume"
        );
        assert_eq!(
            f.read(addrs.turn + OFF_TURN_BUSY),
            0,
            "busy cleared after the turn"
        );
        assert_eq!(
            f.read(addrs.count),
            1,
            "selection released the dispatched slot (04 §2) — only the second message remains"
        );

        f.write(f.waker + OFF_TURN_RESUME_READY, 0);
        assert_eq!(page.call0_at(select_off), 1, "ran the second queued turn");
        assert_eq!(
            f.read(f.waker + OFF_TURN_REPLY),
            22,
            "(method 1, arg 20) dispatched second, in admission order"
        );

        assert_eq!(
            page.call0_at(select_off),
            0,
            "mailbox now empty: no turn to run"
        );
    }

    #[test]
    fn a_send_with_no_waker_delivers_nowhere() {
        let capacity: u64 = 2;
        let slot_size: u64 = 24;
        let f = ActorFixture::new(capacity, slot_size);
        let addrs = f.addrs;

        let mut combined: Vec<u32> = Vec::new();
        let method0_start = combined.len();
        combined.extend(stand_in_method(1));
        let enqueue_start = combined.len();
        combined.extend(build_rt_enqueue(&addrs, capacity, slot_size, enqueue_start));
        let select_start = combined.len();
        combined.extend(build_rt_select_and_run(
            &addrs,
            capacity,
            slot_size,
            &[(method0_start, false)],
            select_start,
        ));
        let page = ExecPage::new(&combined);

        let arg: u64 = 7;
        assert_eq!(
            page.call4_at(enqueue_start * 4, 0, &arg as *const u64 as u64, 1, 0),
            0,
            "send admitted (waker = 0)"
        );
        assert_eq!(page.call0_at(select_start * 4), 1, "the send's turn ran");
        assert_eq!(
            f.read(f.waker + OFF_TURN_RESUME_READY),
            0,
            "no waker -> nothing marked ready anywhere"
        );
        assert_eq!(f.read(addrs.turn + OFF_TURN_BUSY), 0);
    }

    #[test]
    fn rt_select_and_run_never_admits_a_second_turn_while_busy() {
        // Decision 4's structural non-reentrancy: with `busy` set (a real
        // parked awaiting turn's state) and no delivered reply, the actor
        // must do nothing — the queued message stays queued.
        let capacity: u64 = 1;
        let slot_size: u64 = 24;
        let f = ActorFixture::new(capacity, slot_size);
        let addrs = f.addrs;
        f.write(addrs.turn + OFF_TURN_BUSY, 1);
        f.write(addrs.turn + OFF_TURN_SUSPENDED, 1); // parked...
        // ...but resume_ready stays 0: the awaited reply has not arrived.
        f.write(addrs.count, 1); // a second message is queued...

        let mut combined: Vec<u32> = Vec::new();
        let method0_start = combined.len();
        combined.extend(stand_in_method(1));
        let select_start = combined.len();
        combined.extend(build_rt_select_and_run(
            &addrs,
            capacity,
            slot_size,
            &[(method0_start, false)],
            select_start,
        ));
        let page = ExecPage::new(&combined);

        assert_eq!(
            page.call0_at(select_start * 4),
            0,
            "busy-suspended actor admits no new turn, even with a message queued"
        );
        assert_eq!(f.read(addrs.count), 1, "count untouched");
        assert_eq!(
            f.read(addrs.turn + OFF_TURN_BUSY),
            1,
            "still owned by the parked turn"
        );
    }

    #[test]
    fn rt_select_and_run_dispatches_correctly_at_the_smallest_possible_ring() {
        // capacity=1, slot_size=16 (idx + waker, no args) — the smallest
        // legal slot; the bounded arg load must never read past the ring.
        let capacity: u64 = 1;
        let slot_size: u64 = 16;
        let f = ActorFixture::new(capacity, slot_size);
        let addrs = f.addrs;
        // Hand-seed one message (method 0, waker = the stand-in record).
        f.write(addrs.ring, 0);
        f.write(addrs.ring + 8, f.waker);
        f.write(addrs.tail, 1);
        f.write(addrs.count, 1);

        // A genuine no-arg method: returns a fixed constant, never reads
        // x1/x2 at all.
        let no_arg_method = vec![encode::enc_movz(0, 42, 0, true), encode::enc_ret(30)];

        let mut combined: Vec<u32> = Vec::new();
        let method0_start = combined.len();
        combined.extend(no_arg_method);
        let select_start = combined.len();
        combined.extend(build_rt_select_and_run(
            &addrs,
            capacity,
            slot_size,
            &[(method0_start, false)],
            select_start,
        ));
        let page = ExecPage::new(&combined);
        assert_eq!(page.call0_at(select_start * 4), 1, "should have dispatched");
        assert_eq!(f.read(f.waker + OFF_TURN_REPLY), 42);
    }

    /// A hand-assembled stand-in that implements the full compiled-async-
    /// fn contract (`codegen.rs`'s park-and-resume module doc) against a
    /// baked-in turn record address: fresh entry parks immediately (as if
    /// its first op were an `await` of something external), a resumed
    /// entry consumes the discriminant + delivered reply and completes
    /// with `reply + 100`.
    fn stand_in_async_method(rec: u64) -> Vec<u32> {
        let mut w: Vec<u32> = Vec::new();
        // load_imm x9, rec (4 words)
        for word in load_imm4(9, rec) {
            w.push(word);
        }
        w.push(encode::enc_ldr_x_imm(10, 9, OFF_TURN_SUSPENDED as u16));
        // cbnz x10, +5 words -> .resume
        w.push(encode::enc_cbnz(10, 5 * 4, true));
        // fresh: suspended = 1; return TURN_STATUS_SUSPENDED.
        w.push(encode::enc_movz(11, 1, 0, true));
        w.push(encode::enc_str_x_imm(11, 9, OFF_TURN_SUSPENDED as u16));
        w.push(encode::enc_movz(0, TURN_STATUS_SUSPENDED as u16, 0, true));
        w.push(encode::enc_ret(30));
        // .resume: clear discriminant + ready, complete with reply + 100.
        w.push(encode::enc_str_x_imm(31, 9, OFF_TURN_SUSPENDED as u16));
        w.push(encode::enc_str_x_imm(31, 9, OFF_TURN_RESUME_READY as u16));
        w.push(encode::enc_ldr_x_imm(1, 9, OFF_TURN_REPLY as u16));
        w.push(encode::enc_add_imm(1, 1, 100, true));
        w.push(encode::enc_movz(0, TURN_STATUS_COMPLETED as u16, 0, true));
        w.push(encode::enc_ret(30));
        w
    }

    fn load_imm4(reg: u8, value: u64) -> [u32; 4] {
        [
            encode::enc_movz(reg, (value & 0xFFFF) as u16, 0, true),
            encode::enc_movk(reg, ((value >> 16) & 0xFFFF) as u16, 16, true),
            encode::enc_movk(reg, ((value >> 32) & 0xFFFF) as u16, 32, true),
            encode::enc_movk(reg, ((value >> 48) & 0xFFFF) as u16, 48, true),
        ]
    }

    /// The whole park-and-resume turn lifecycle through the real
    /// scheduler primitives: fresh dispatch parks (a slice ran, busy
    /// stays), the parked turn is not re-entered until its reply is
    /// delivered, resume completes it, and the completion is delivered to
    /// the ORIGINAL message's waker.
    #[test]
    fn a_parked_turn_resumes_only_after_delivery_and_then_completes_to_its_waker() {
        let capacity: u64 = 2;
        let slot_size: u64 = 16; // no-arg async method
        let f = ActorFixture::new(capacity, slot_size);
        let addrs = f.addrs;

        let mut combined: Vec<u32> = Vec::new();
        let method0_start = combined.len();
        combined.extend(stand_in_async_method(addrs.turn));
        let enqueue_start = combined.len();
        combined.extend(build_rt_enqueue(&addrs, capacity, slot_size, enqueue_start));
        let select_start = combined.len();
        combined.extend(build_rt_select_and_run(
            &addrs,
            capacity,
            slot_size,
            &[(method0_start, true)],
            select_start,
        ));
        let page = ExecPage::new(&combined);

        assert_eq!(
            page.call4_at(enqueue_start * 4, 0, 0, 0, f.waker),
            0,
            "admitted"
        );
        // Fresh dispatch: the turn parks — a real slice ran.
        assert_eq!(
            page.call0_at(select_start * 4),
            1,
            "fresh slice ran (then parked)"
        );
        assert_eq!(
            f.read(addrs.turn + OFF_TURN_BUSY),
            1,
            "parked turn still owns the actor"
        );
        assert_eq!(f.read(addrs.turn + OFF_TURN_SUSPENDED), 1);
        assert_eq!(f.read(addrs.count), 0, "slot released at selection");
        // Not resumable yet: nothing delivered.
        assert_eq!(
            page.call0_at(select_start * 4),
            0,
            "parked + no reply -> not ready"
        );
        // Deliver (what a completing awaited turn's scheduler would do).
        f.write(addrs.turn + OFF_TURN_REPLY, 5);
        f.write(addrs.turn + OFF_TURN_RESUME_READY, 1);
        // Resume: completes with 105, delivered to the waker.
        assert_eq!(page.call0_at(select_start * 4), 1, "resumed and completed");
        assert_eq!(f.read(addrs.turn + OFF_TURN_BUSY), 0);
        assert_eq!(f.read(f.waker + OFF_TURN_REPLY), 105);
        assert_eq!(f.read(f.waker + OFF_TURN_RESUME_READY), 1);
        assert_eq!(page.call0_at(select_start * 4), 0, "idle again");
    }

    /// `rt_run_one`'s deterministic round-robin: with several actors
    /// ready, the cursor decides who runs, and advances past the actor
    /// that ran.
    #[test]
    fn rt_run_one_selects_ready_actors_round_robin_from_the_cursor() {
        let capacity: u64 = 2;
        let slot_size: u64 = 16;
        let state_size: u64 = 8;
        let ram = HostRam::new(4096);
        let base = ram.base();
        let region = |i: u64| -> ActorAddrs {
            let b = base + i * 256;
            let ring = b + state_size;
            let head = ring + capacity * slot_size;
            ActorAddrs {
                state: b,
                ring,
                head,
                tail: head + 8,
                count: head + 16,
                turn: head + 24,
            }
        };
        let a0 = region(0);
        let a1 = region(1);
        let cursor_addr = base + 4096 - 8;
        let waker0 = base + 2048;
        let waker1 = base + 2048 + 128;

        // Hand-seed one no-arg message per actor.
        for (a, w) in [(a0, waker0), (a1, waker1)] {
            ram.write_u64(a.ring - base, 0);
            ram.write_u64(a.ring + 8 - base, w);
            ram.write_u64(a.tail - base, 1);
            ram.write_u64(a.count - base, 1);
        }

        let m0 = vec![encode::enc_movz(0, 10, 0, true), encode::enc_ret(30)];
        let m1 = vec![encode::enc_movz(0, 20, 0, true), encode::enc_ret(30)];

        let mut combined: Vec<u32> = Vec::new();
        let m0_start = combined.len();
        combined.extend(m0);
        let m1_start = combined.len();
        combined.extend(m1);
        let sel0_start = combined.len();
        combined.extend(build_rt_select_and_run(
            &a0,
            capacity,
            slot_size,
            &[(m0_start, false)],
            sel0_start,
        ));
        let sel1_start = combined.len();
        combined.extend(build_rt_select_and_run(
            &a1,
            capacity,
            slot_size,
            &[(m1_start, false)],
            sel1_start,
        ));
        let run_one_start = combined.len();
        let run_one = build_rt_run_one(&[sel0_start, sel1_start], cursor_addr, run_one_start);
        combined.extend(run_one.words);

        let page = ExecPage::new(&combined);
        let run = || page.call0_at(run_one_start * 4);

        // cursor starts at 1 (hand-set): actor 1 must run FIRST even
        // though actor 0 is also ready — the tie-breaker is the cursor.
        ram.write_u64(cursor_addr - base, 1);
        assert_eq!(run(), 1, "one ready turn ran");
        assert_eq!(
            ram.read_u64(waker1 + OFF_TURN_REPLY - base),
            20,
            "cursor=1 -> actor 1 ran first"
        );
        assert_eq!(
            ram.read_u64(cursor_addr - base),
            0,
            "cursor advanced past actor 1 (wrap)"
        );
        assert_eq!(run(), 1, "second tick runs the remaining ready actor");
        assert_eq!(ram.read_u64(waker0 + OFF_TURN_REPLY - base), 10);
        assert_eq!(ram.read_u64(cursor_addr - base), 1);
        assert_eq!(run(), 0, "nothing ready");
    }
}
