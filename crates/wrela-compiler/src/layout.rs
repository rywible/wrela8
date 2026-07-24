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
use crate::eval::image::push_line;
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
    /// `entry`/`code`/`rodata` (if nonempty)/`abort`, in ascending-base
    /// (= emission) order. `data` never appears (module doc: always
    /// empty at M5).
    pub sections: Vec<Section>,
}

// --- scratch registers for stub emission (never x0..x8/x29/x30/sp) -----

const X_SP: u8 = 31;
const SCRATCH_A: u8 = 9;
const SCRATCH_B: u8 = 10;
const SCRATCH_C: u8 = 11;

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
pub fn layout_program(program: &CodegenProgram) -> Result<ImageLayout, LayoutError> {
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
    let abort_size = cursor - abort_fixed_base; // both stubs' combined byte length

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

    // --- resolve every Reloc against the now-known section bases --------
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

    verify_section_sizes(&sections, image_base, blob.len() as u64)?;

    Ok(ImageLayout {
        blob,
        entry: entry_base,
        sections,
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
pub fn try_layout_program(
    programs: &BTreeMap<String, TypedProgram>,
    layout_ctx: &LayoutCtx,
) -> Result<Option<ImageLayout>, String> {
    let mut mwir_programs = Vec::with_capacity(programs.len());
    for typed in programs.values() {
        match crate::lower::lower_program(typed) {
            Ok(p) => mwir_programs.push(p),
            Err(_) => return Ok(None),
        }
    }
    let merged = merge_mwir_programs(mwir_programs);
    let codegen_program = match crate::codegen::codegen_program(&merged, layout_ctx) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    layout_program(&codegen_program)
        .map(Some)
        .map_err(|e| e.message)
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
// ## The console ring writer and decimal formatter
//
// `__wrela_ring_write(x0=src_ptr, x1=len)` and `__wrela_fmt_dec(x0=value,
// x1=is_signed) -> x0=len` are two more fixed, hand-assembled subroutines
// (like `__wrela_abort`/`__wrela_abort_val`), placed in the same combined
// "entry" section so every internal call between driver/abort/ring-write/
// fmt-dec resolves as a *local*, directly-computed `BL` (both call site and
// callee live in the same contiguously-assembled word list — no `Reloc`
// needed for these; only calls that leave this section, an `@test(runtime)`
// fn's own code or a literal string in the rodata section, use the
// existing `Reloc::Call`/`Reloc::Rodata` machinery unchanged). Console ring
// bookkeeping (`OFF_RING_DATA_BUMP`/`OFF_RING_DESC_BUMP`, a bump
// allocator — decision 12's "drained once, after halt" rule means nothing
// is ever reclaimed) and the "one `__wrela_ring_write` call per printed
// report line, capped at `console::QUEUE_SIZE` (16) lines per boot" bound
// are documented in full at each fn's own doc comment below.
//
// **Disclosed simplification of the split-ring contract**: this producer
// never reorders or skips a descriptor index, so `__wrela_ring_write` never
// populates `avail.ring[]` at all — the VMM's own console model (item E,
// `wrela-vmm`) reads descriptors `0..avail.idx` directly by index, which is
// exactly what a real virtio consumer would get by walking `avail.ring[]`
// *because* this producer's own `avail.ring[i]` would always equal `i`.
// The `used` ring is never populated or read either (M5 has no completion
// tracking to negotiate: the guest never waits on it, and the transcript is
// read only after the guest halts, decision 12).
//
// **Disclosed simplification of the output cap**: `__wrela_ring_write`
// clamps an over-long write to whatever room remains in `console::
// DATA_SIZE` (silently truncating the tail) and, if all `console::
// QUEUE_SIZE` descriptor slots are already spent, is a silent no-op —
// unlike the plan's own suggested "FAILED (output cap exceeded)" marker
// line, which would need `__wrela_ring_write` itself to still have ring
// capacity to print *that* marker, a circularity this module resolves by
// simply not attempting the marker. Undetected today by any golden (no
// `@test(runtime)` here prints anywhere near 16 KiB or 16 lines); recorded
// here as an honest, disclosed M5 bound rather than silently assumed away.

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

/// `__wrela_ring_write(x0=src_ptr, x1=len)`. Copies `len` bytes from
/// `src_ptr` into the next free slot of `console::DATA_SIZE` (clamping to
/// whatever room remains — module doc's own disclosed output-cap
/// simplification), publishes one descriptor covering them, bumps
/// `avail.idx`, rings the doorbell, and returns (an ordinary leaf `RET`,
/// unlike the two `noreturn` abort stubs). A silent no-op (immediate
/// `RET`) if `console::QUEUE_SIZE` descriptor slots are already spent.
///
/// Register use (this fragment owns every register it touches — nothing
/// survives a `BL` to this fn per this ABI's own "caller-saved
/// everything" convention, module doc above): `x9`-`x18` scratch
/// throughout, retracing (documented once, precisely, since there is no
/// assembler to re-derive this from): `x9`/`x10` = the desc-bump address/
/// value; `x11`/`x12` = the data-bump address/value; `x13` = remaining
/// capacity (clobbered into other uses once the clamp decision is made);
/// `x14` = the destination byte address (preserved across the copy loop
/// for the descriptor's own `addr` field); `x15`/`x16`/`x17` = the copy
/// loop's src/dst cursors and remaining-count; `x18` = the copy loop's
/// one-byte transfer register, then reused as the descriptor-table
/// address scratch once the loop is done.
fn build_ring_write(addrs: &HarnessAddrs, start: usize) -> Asm {
    let mut a = Asm::new(start);
    let desc_bump_addr = addrs.info_base + mi::OFF_RING_DESC_BUMP;
    let data_bump_addr = addrs.info_base + mi::OFF_RING_DATA_BUMP;

    a.load_imm(9, desc_bump_addr);
    a.push(encode::enc_ldr_x_imm(10, 9, 0)); // x10 = desc_bump
    a.push(encode::enc_cmp_imm(10, console::QUEUE_SIZE as u16, true));
    let skip_have_slot = a.skip_placeholder(); // b.lt .have_slot
    a.push(encode::enc_ret(30)); // no slot left: silent no-op
    a.patch_cond(skip_have_slot, Cond::Lt);
    // .have_slot:
    a.load_imm(11, data_bump_addr);
    a.push(encode::enc_ldr_x_imm(12, 11, 0)); // x12 = data_bump
    a.load_imm(13, console::DATA_SIZE);
    a.push(encode::enc_sub_reg(13, 13, 12, true)); // x13 = remaining
    a.push(encode::enc_cmp_reg(1, 13, true)); // len vs remaining
    let skip_len_ok = a.skip_placeholder(); // b.le .len_ok
    a.push(encode::enc_mov_reg(1, 13, true)); // clamp: len = remaining
    a.patch_cond(skip_len_ok, Cond::Le);
    // .len_ok:
    a.load_imm(14, addrs.data_base);
    a.push(encode::enc_add_reg(14, 14, 12, true)); // x14 = dst_addr
    a.push(encode::enc_mov_reg(15, 0, true)); // x15 = src cursor
    a.push(encode::enc_mov_reg(16, 14, true)); // x16 = dst cursor
    a.push(encode::enc_mov_reg(17, 1, true)); // x17 = remaining count
    let loop_top = a.abs();
    let skip_loop = a.skip_placeholder(); // cbz x17, .copy_done
    a.push(encode::enc_ldrb_imm(18, 15, 0));
    a.push(encode::enc_strb_imm(18, 16, 0));
    a.push(encode::enc_add_imm(15, 15, 1, true));
    a.push(encode::enc_add_imm(16, 16, 1, true));
    a.push(encode::enc_sub_imm(17, 17, 1, true));
    a.b_to(loop_top);
    a.patch_cbz(skip_loop, 17);
    // .copy_done: descriptor entry at ring_base+DESC_TABLE_OFFSET+desc_bump*16
    a.load_imm(9, console::DESC_ENTRY_SIZE);
    a.push(encode::enc_mul(9, 10, 9, true)); // x9 = desc_bump * 16
    a.load_imm(18, addrs.ring_base + console::DESC_TABLE_OFFSET);
    a.push(encode::enc_add_reg(18, 18, 9, true)); // x18 = desc entry addr
    a.push(encode::enc_str_x_imm(14, 18, 0)); // desc.addr = dst_addr
    a.push(encode::enc_str_w_imm(1, 18, 8)); // desc.len = clamped len
    a.push(encode::enc_mov_reg(0, 31, true)); // x0 = 0 (from xzr)
    a.push(encode::enc_str_w_imm(0, 18, 12)); // desc.flags/next = 0
    // avail.idx = desc_bump + 1 (avail.ring[] is never populated — module
    // doc's own disclosed simplification: this producer never reorders or
    // skips an index, so the VMM reads descriptors 0..avail.idx directly).
    a.push(encode::enc_add_imm(10, 10, 1, true)); // x10 = desc_bump + 1
    a.push(encode::enc_lsl_imm(9, 10, 16, true)); // x9 = idx << 16 (flags=0)
    a.load_imm(18, addrs.ring_base + console::AVAIL_OFFSET);
    a.push(encode::enc_str_w_imm(9, 18, 0));
    // bump counters: data_bump += clamped len (x1); desc_bump = x10 (already
    // desc_bump+1, computed above for the avail.idx write).
    a.push(encode::enc_add_reg(12, 12, 1, true)); // x12 = data_bump + len
    a.load_imm(18, data_bump_addr);
    a.push(encode::enc_str_x_imm(12, 18, 0));
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
/// paragraph): prints `FAILED ` (shared literal) then the caller's own
/// fixed message, then a newline, over the console ring, then runs the
/// landing pad's own tail (above). `msg_ptr`/`msg_len` are stashed on the
/// stack across the two `__wrela_ring_write` calls that need it (`x0`-`x18`
/// are all caller-saved under this ABI, module doc above — nothing survives
/// a `BL` on its own).
fn build_abort_fixed(
    addrs: &HarnessAddrs,
    start: usize,
    ring_write_start: usize,
    failed_word_off: usize,
) -> Asm {
    let mut a = Asm::new(start);
    a.push(encode::enc_sub_imm(31, 31, 16, true)); // sub sp, sp, #16
    a.push(encode::enc_str_x_imm(0, 31, 0));
    a.push(encode::enc_str_x_imm(1, 31, 8));

    a.load_rodata_addr_at(0, failed_word_off);
    a.load_imm(1, 7);
    a.bl_to(ring_write_start);

    a.push(encode::enc_ldr_x_imm(0, 31, 0));
    a.push(encode::enc_ldr_x_imm(1, 31, 8));
    a.bl_to(ring_write_start);

    a.push(encode::enc_add_imm(31, 31, 16, true)); // add sp, sp, #16
    push_abort_tail(&mut a, addrs);
    a
}

/// `__wrela_abort_val(x0=prefix_ptr, x1=prefix_len, x2=value,
/// x3=value_signed, x4=suffix_ptr, x5=suffix_len) -> noreturn` — the
/// test-image variant: prints `FAILED `, the prefix, `value` rendered as
/// decimal (via `__wrela_fmt_dec`), the suffix, then a newline, then the
/// landing-pad tail. All six incoming args are stashed on the stack up
/// front (48 bytes) and reloaded around each of the four
/// `__wrela_ring_write`/one `__wrela_fmt_dec` calls that clobber them.
fn build_abort_val(
    addrs: &HarnessAddrs,
    start: usize,
    ring_write_start: usize,
    fmt_dec_start: usize,
    failed_word_off: usize,
) -> Asm {
    let mut a = Asm::new(start);
    a.push(encode::enc_sub_imm(31, 31, 48, true));
    for (i, reg) in [0u8, 1, 2, 3, 4, 5].into_iter().enumerate() {
        a.push(encode::enc_str_x_imm(reg, 31, (i * 8) as u16));
    }

    a.load_rodata_addr_at(0, failed_word_off);
    a.load_imm(1, 7);
    a.bl_to(ring_write_start);

    a.push(encode::enc_ldr_x_imm(0, 31, 0));
    a.push(encode::enc_ldr_x_imm(1, 31, 8));
    a.bl_to(ring_write_start); // prefix

    a.push(encode::enc_ldr_x_imm(0, 31, 16));
    a.push(encode::enc_ldr_x_imm(1, 31, 24));
    a.bl_to(fmt_dec_start); // x0 = len, written into OFF_TEST_LINE_BUF
    a.push(encode::enc_mov_reg(1, 0, true));
    a.load_imm(0, addrs.info_base + mi::OFF_TEST_LINE_BUF);
    a.bl_to(ring_write_start);

    a.push(encode::enc_ldr_x_imm(0, 31, 32));
    a.push(encode::enc_ldr_x_imm(1, 31, 40));
    a.bl_to(ring_write_start); // suffix

    a.push(encode::enc_add_imm(31, 31, 48, true));
    push_abort_tail(&mut a, addrs);
    a
}

/// The runtime test image's own entry driver (module doc's "Why the entry
/// driver needs no runtime loop at all"): installs core 0's stack pointer,
/// zeroes every harness counter, then one straight-line block per
/// `@test(runtime)` fn in `runtime_tests`' own order — print `test <name>:
/// `, arm the landing pad's own continuation slot, `BL` the test, print
/// `ok\n` and increment the passed counter on an ordinary return (an abort
/// anywhere inside that `BL`'s own call tree instead lands directly at the
/// top of the *next* block, module doc's own landing-pad section) — then
/// the one merged summary line and the exit-code/halt tail. `x8` is set to
/// the fixed `OFF_TEST_LINE_BUF` scratch address before every test call as
/// a defensive measure (this ABI's own aggregate-return convention writes
/// through whatever `x8` holds; a test fn's return value is otherwise
/// unread, but this guarantees an aggregate return, if one ever exists, has
/// somewhere harmless to land rather than an arbitrary stale address).
fn build_entry_driver(
    addrs: &HarnessAddrs,
    start: usize,
    ring_write_start: usize,
    fmt_dec_start: usize,
    runtime_tests: &[String],
    rodata: &mut Vec<Vec<u8>>,
    rodata_cursor: &mut usize,
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
    ] {
        a.load_imm(10, addrs.info_base + off);
        a.push(encode::enc_str_x_imm(9, 10, 0));
    }

    let ok_off = append_rodata(rodata, rodata_cursor, b"ok\n".to_vec());
    let passed_comma_off = append_rodata(rodata, rodata_cursor, b" passed, ".to_vec());
    let failed_tail_off = append_rodata(rodata, rodata_cursor, b" failed\n".to_vec());

    for name in runtime_tests {
        let prefix_bytes = format!("test {name}: ").into_bytes();
        let prefix_len = prefix_bytes.len() as u64;
        let prefix_off = append_rodata(rodata, rodata_cursor, prefix_bytes);

        a.load_rodata_addr_at(0, prefix_off);
        a.load_imm(1, prefix_len);
        a.bl_to(ring_write_start);

        let cont_marker = a.load_imm_placeholder(9);
        a.load_imm(10, addrs.info_base + mi::OFF_TEST_CONTINUATION);
        a.push(encode::enc_str_x_imm(9, 10, 0));

        a.load_imm(8, addrs.info_base + mi::OFF_TEST_LINE_BUF);
        a.bl_call_key(name);

        a.load_rodata_addr_at(0, ok_off);
        a.load_imm(1, 3);
        a.bl_to(ring_write_start);

        a.load_imm(9, addrs.info_base + mi::OFF_TEST_PASSED);
        a.push(encode::enc_ldr_x_imm(10, 9, 0));
        a.push(encode::enc_add_imm(10, 10, 1, true));
        a.push(encode::enc_str_x_imm(10, 9, 0));

        let cont_target = a.abs() as u64;
        a.patch_load_imm(cont_marker, 9, cont_target);
    }

    // Summary line: "<passed> passed, <failed> failed\n".
    a.load_imm(9, addrs.info_base + mi::OFF_TEST_PASSED);
    a.push(encode::enc_ldr_x_imm(0, 9, 0));
    a.push(encode::enc_movz(1, 0, 0, true));
    a.bl_to(fmt_dec_start);
    a.push(encode::enc_mov_reg(1, 0, true));
    a.load_imm(0, addrs.info_base + mi::OFF_TEST_LINE_BUF);
    a.bl_to(ring_write_start);

    a.load_rodata_addr_at(0, passed_comma_off);
    a.load_imm(1, 9);
    a.bl_to(ring_write_start);

    a.load_imm(9, addrs.info_base + mi::OFF_TEST_FAILED);
    a.push(encode::enc_ldr_x_imm(0, 9, 0));
    a.push(encode::enc_movz(1, 0, 0, true));
    a.bl_to(fmt_dec_start);
    a.push(encode::enc_mov_reg(1, 0, true));
    a.load_imm(0, addrs.info_base + mi::OFF_TEST_LINE_BUF);
    a.bl_to(ring_write_start);

    a.load_rodata_addr_at(0, failed_tail_off);
    a.load_imm(1, 8);
    a.bl_to(ring_write_start);

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

/// Places a codegen'd test-image program into the machine's fixed
/// contract, per module doc above: **one** combined "entry" section
/// (`__wrela_ring_write`, `__wrela_fmt_dec`, `__wrela_abort`,
/// `__wrela_abort_val`, the entry driver, in that fixed order — the order
/// every internal local branch/call above assumes), then `code` (every
/// codegen'd fn, `@test(runtime)` fns included — they are ordinary fns to
/// `codegen.rs`, called by name like any other), then `rodata`
/// (`program.rodata`'s own already-interned entries, followed by every
/// harness literal this fn appends — `append_rodata`, above). Every
/// `Reloc` — the harness's own `Call`/`Rodata` entries and every ordinary
/// compiled fn's `Call`/`Rodata`/`AbortFixed`/`AbortVal` — resolves through
/// the identical `patch_bl`/`patch_adrp_add` this file's item-D half
/// already proved; `AbortFixed`/`AbortVal` targets are simply this
/// section's own `abort_fixed_start`/`abort_val_start` words instead of a
/// separate section, since the test image's `__wrela_abort`/
/// `__wrela_abort_val` symbols *are* these words.
///
/// `Err` for a genuine internal inconsistency (module doc mirrors
/// `layout_program`'s own doc here): an out-of-range relocation, or a
/// name in `runtime_tests` `codegen_program` never produced (an internal
/// invariant `bin/wrela.rs`'s own caller is expected to have already
/// checked via `TypedProgram::tests`, kept here anyway as a real `Err`
/// rather than a silent skip).
pub fn layout_test_image(
    program: &CodegenProgram,
    runtime_tests: &[String],
) -> Result<ImageLayout, LayoutError> {
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

    // Shared literal used by both abort bodies, interned once, before
    // either is built (both need its byte offset).
    let failed_word_off = append_rodata(&mut rodata, &mut rodata_cursor, b"FAILED ".to_vec());

    let ring_write_asm = build_ring_write(&addrs, 0);
    let ring_write_start = 0usize;
    let fmt_dec_start = ring_write_start + ring_write_asm.words.len();
    let fmt_dec_asm = build_fmt_dec(&addrs, fmt_dec_start);
    let abort_fixed_start = fmt_dec_start + fmt_dec_asm.words.len();
    let abort_fixed_asm =
        build_abort_fixed(&addrs, abort_fixed_start, ring_write_start, failed_word_off);
    let abort_val_start = abort_fixed_start + abort_fixed_asm.words.len();
    let abort_val_asm = build_abort_val(
        &addrs,
        abort_val_start,
        ring_write_start,
        fmt_dec_start,
        failed_word_off,
    );
    let entry_start = abort_val_start + abort_val_asm.words.len();
    let entry_asm = build_entry_driver(
        &addrs,
        entry_start,
        ring_write_start,
        fmt_dec_start,
        runtime_tests,
        &mut rodata,
        &mut rodata_cursor,
    );

    let mut harness_words: Vec<u32> = Vec::new();
    let mut harness_relocs: Vec<Reloc> = Vec::new();
    for asm in [
        ring_write_asm,
        fmt_dec_asm,
        abort_fixed_asm,
        abort_val_asm,
        entry_asm,
    ] {
        debug_assert_eq!(asm.start, harness_words.len());
        harness_relocs.extend(asm.relocs);
        harness_words.extend(asm.words);
    }

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
        let base = cursor;
        cursor += rodata_bytes.len() as u64;
        Some(base)
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
            Reloc::AbortFixed { .. } | Reloc::AbortVal { .. } => {
                return Err(LayoutError::new(
                    "internal error: the harness section itself must never emit an AbortFixed/AbortVal reloc",
                ));
            }
        }
    }
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

    verify_section_sizes(&sections, image_base, blob.len() as u64)?;

    Ok(ImageLayout {
        blob,
        entry: harness_base + (entry_start as u64) * 4,
        sections,
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
        let out = layout_program(&program).unwrap();
        let names: Vec<&str> = out.sections.iter().map(|s| s.name).collect();
        assert_eq!(names, ["entry", "code", "abort"]);
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
        let out = layout_program(&program).unwrap();
        let names: Vec<&str> = out.sections.iter().map(|s| s.name).collect();
        assert_eq!(names, ["entry", "code", "rodata", "abort"]);
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
        let out = layout_program(&program).unwrap();
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
        assert!(layout_program(&program).is_err());
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
        let a = layout_program(&program).unwrap();
        let b = layout_program(&program).unwrap();
        assert_eq!(a, b);
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

        fn write_u64(&self, off: u64, v: u64) {
            assert!((off as usize) + 8 <= self.len);
            unsafe { std::ptr::write_unaligned(self.ptr.add(off as usize) as *mut u64, v) }
        }

        fn read_bytes(&self, off: u64, n: usize) -> Vec<u8> {
            assert!((off as usize) + n <= self.len);
            unsafe { std::slice::from_raw_parts(self.ptr.add(off as usize), n).to_vec() }
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

    // --- __wrela_ring_write -------------------------------------------------

    fn ring_write_call(prior_desc_bump: u64, prior_data_bump: u64, src: &[u8]) -> HostRam {
        // One combined host page stands in for info/ring/data alike (their
        // real, separate addresses are just three different fields of
        // `HarnessAddrs` — using the same page for all three here only
        // works because none of `console`'s own offsets collide with
        // `machine_info`'s in this synthetic single-page layout; this test
        // never claims that's true of the real, separate machine regions).
        let ram = HostRam::new(4096 * 8);
        let addrs = HarnessAddrs {
            info_base: ram.base(),
            ring_base: ram.base() + 4096,
            data_base: ram.base() + 4096 * 2,
            exit_mmio_addr: 0,
        };
        ram.write_u64(
            addrs.info_base - ram.base() + mi::OFF_RING_DESC_BUMP,
            prior_desc_bump,
        );
        ram.write_u64(
            addrs.info_base - ram.base() + mi::OFF_RING_DATA_BUMP,
            prior_data_bump,
        );

        let asm = build_ring_write(&addrs, 0);
        assert!(asm.relocs.is_empty(), "ring_write must need no Reloc");
        let page = ExecPage::new(&words_of(&asm));

        // src lives in its own host buffer so `call2` can pass a real
        // pointer distinct from the fake "guest RAM" page.
        let src_ram = HostRam::new(src.len().max(1));
        unsafe {
            std::ptr::copy_nonoverlapping(src.as_ptr(), src_ram.ptr, src.len());
        }
        page.call2(src_ram.base(), src.len() as u64);
        ram
    }

    #[test]
    fn ring_write_first_call_publishes_one_descriptor() {
        let ram = ring_write_call(0, 0, b"hello");
        let info = 0u64; // info_base == ram.base() (offset 0)
        assert_eq!(ram.read_u64(info + mi::OFF_RING_DESC_BUMP), 1);
        assert_eq!(ram.read_u64(info + mi::OFF_RING_DATA_BUMP), 5);

        let ring = 4096u64;
        let data = 4096u64 * 2;
        // desc[0]: addr, len, flags/next.
        let desc_addr = ram.read_u64(ring + console::DESC_TABLE_OFFSET);
        assert_eq!(desc_addr, ram.base() + data);
        assert_eq!(
            ram.read_u32(ring + console::DESC_TABLE_OFFSET + 8),
            5,
            "desc.len"
        );
        assert_eq!(
            ram.read_u32(ring + console::DESC_TABLE_OFFSET + 12),
            0,
            "desc.flags/next"
        );
        // avail.idx == 1 (flags stays 0, packed into the same 32-bit word).
        assert_eq!(ram.read_u32(ring + console::AVAIL_OFFSET), 1u32 << 16);
        // doorbell rung.
        assert_eq!(ram.read_u64(ring + console::DOORBELL_OFFSET), 1);
        // data bytes copied verbatim.
        assert_eq!(ram.read_bytes(data, 5), b"hello".to_vec());
    }

    #[test]
    fn ring_write_second_call_uses_the_next_descriptor_and_data_slot() {
        let ram = ring_write_call(1, 5, b"world!");
        let info = 0u64;
        assert_eq!(ram.read_u64(info + mi::OFF_RING_DESC_BUMP), 2);
        assert_eq!(ram.read_u64(info + mi::OFF_RING_DATA_BUMP), 11);
        let ring = 4096u64;
        let data = 4096u64 * 2;
        let desc1_addr = ram.read_u64(ring + console::DESC_TABLE_OFFSET + console::DESC_ENTRY_SIZE);
        assert_eq!(desc1_addr, ram.base() + data + 5);
        assert_eq!(ram.read_bytes(data + 5, 6), b"world!".to_vec());
    }

    #[test]
    fn ring_write_at_queue_capacity_is_a_silent_no_op() {
        let ram = ring_write_call(console::QUEUE_SIZE, 0, b"dropped");
        let info = 0u64;
        // Bump counters unchanged — the call returned immediately.
        assert_eq!(
            ram.read_u64(info + mi::OFF_RING_DESC_BUMP),
            console::QUEUE_SIZE
        );
        assert_eq!(ram.read_u64(info + mi::OFF_RING_DATA_BUMP), 0);
    }

    #[test]
    fn ring_write_clamps_to_remaining_data_capacity() {
        // Only 3 bytes of room left; a 5-byte write must be truncated to 3.
        let prior_data = console::DATA_SIZE - 3;
        let ram = ring_write_call(0, prior_data, b"abcde");
        let info = 0u64;
        assert_eq!(
            ram.read_u64(info + mi::OFF_RING_DATA_BUMP),
            console::DATA_SIZE
        );
        let ring = 4096u64;
        assert_eq!(
            ram.read_u32(ring + console::DESC_TABLE_OFFSET + 8),
            3,
            "clamped desc.len"
        );
    }
}
