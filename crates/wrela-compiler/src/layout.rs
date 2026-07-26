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
//! ring/data pages, one combined fact — and `stacks` — the three reserved
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

use std::collections::{BTreeMap, BTreeSet};

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
    /// plans/M7.md item D: every bound pool's own placed window, in the
    /// `pooldata` section, name-sorted (`pool_backings`' own `BTreeMap`
    /// order — `image.report.deterministic`). Empty for an image that
    /// declares no pool at all, which is why no existing golden without
    /// one moved when this landed.
    pub pools: Vec<PoolPlacement>,
    /// plans/M7.md item H1: every device this image binds to a `@driver`,
    /// with its own placed register window in the `devregs` section.
    /// Empty for an image that binds no driver.
    pub device_regs: Vec<DeviceRegs>,
    /// plans/M7.md item E1: the virtio-blk transport configuration the
    /// VMM's `parse_report` already consumes (`BlkDevice`/`BlkQueue`),
    /// derived from the image's `capacity_sectors=`/`required_features=`
    /// and the driver's `VirtQueue.configure` call. `None` until a
    /// configure site exists — an image with a device-reachable pool but
    /// no queue still emits `BlkPool` alone (dump accounting), and the
    /// test-image hand-built report only learns these lines when this is
    /// `Some` (so a pool-only image stays bootable without a device model).
    pub blk: Option<BlkReport>,
    /// plans/M7.md item G: host-side `interrupt_status` writes the VMM
    /// must perform before the guest runs, one per bound ISR. Empty when
    /// the image binds no vector. Carried into `wrela test`'s runtime
    /// report so the HVF path can raise a value the guest could not have
    /// produced (`IRQ_HOST_STATUS_MAGIC`).
    pub irq_host_injects: Vec<IrqHostInject>,
    /// plans/M8.md item C1: `(core, entry address)` for every **secondary**
    /// core this image brings up, ascending. Core 0's entry is the
    /// already-published `entry` field above; these are the addresses the
    /// VMM starts vCPUs `1..` at once the guest rings
    /// `mmio::RELEASE_MMIO_ADDR`. Empty for every single-core image, which
    /// is why no pre-C1 report golden gains a line.
    pub core_entries: Vec<(usize, u64)>,
    /// plans/M10.md item A2c / decision 588: every `@placed` static in the
    /// build closure, name-sorted. Empty when the image declares none —
    /// so no pre-A2c report golden gains a line.
    pub placed_statics: Vec<PlacedStatic>,
}

/// One `@placed` static as the image report publishes it (03-hardware.md
/// §3.1: "the address is a checked build output rather than a convention").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedStatic {
    pub name: String,
    pub ty: String,
    pub addr: u64,
    pub size: u64,
}

/// One virtio-blk queue as the report and the VMM both see it
/// (`BlkQueue index= size= desc= avail= used= doorbell=`). Addresses come
/// from `virtqueue::place_ring` against a declared DMA pool — never
/// invented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlkQueueReport {
    pub index: u16,
    pub size: u16,
    pub desc: u64,
    pub avail: u64,
    pub used: u64,
    pub doorbell: u64,
    /// Pool whose backing hosts the ring (the `BlkPool` name).
    pub pool_name: String,
}

/// The closed virtio-blk device configuration emitted into the report
/// (`BlkDevice device= capacity_sectors= features=` / optional `vector=`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlkReport {
    /// plans/M8.md item P: **which** declared device this configuration is
    /// for — the device of the pool the single `VirtQueue.configure` site
    /// consumes, which is 03-hardware.md §1's own "the device is named
    /// once, at the image binding" read back out of the graph. Every
    /// device-facing fact on this struct is read from *that* device's
    /// arguments, never from "whichever device declared one first": with
    /// two devices in an image those are different answers.
    pub device: usize,
    pub capacity_sectors: u64,
    pub features: u64,
    /// Device-owned pending-word bit (`1..=63`). `None` is 03 §7's poll
    /// build — no vector, and the used ring alone is the completion signal.
    pub vector: Option<u64>,
    pub queue: BlkQueueReport,
    /// Decision 2c: descriptors a single blk op needs, and the occupancy
    /// bound `floor(queue_depth / descriptors_per_op)` (plans/M7.md item
    /// E2 / 03-hardware.md §4). Expected exits-per-op stays deferred —
    /// plans/M7.md decision 21: a doorbell is a polled shared-memory write
    /// (06 §5), not a fixed exit count, and inventing `1` spends E1's
    /// deferral without a prediction the VMM's measured `exits` can check.
    pub descriptors_per_op: u16,
    pub occupancy_bound: u16,
}

/// One host-side interrupt injection the VMM performs before the vCPU
/// runs (plans/M7.md item G, 03 §6's `interrupt_status` writer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrqHostInject {
    /// Guest base of the device's `devregs` window.
    pub base: u64,
    /// Byte offset of `interrupt_status` within that window (`0x60`).
    pub offset: u64,
    /// Value written (little-endian `u32`). Always `IRQ_HOST_STATUS_MAGIC`.
    pub status: u32,
    /// Pending-word bit to raise after the write (`1..=63`).
    pub vector: u64,
}

/// Hand-picked host status word for item G's HVF oracle: nonzero in
/// bits the ISR's handled mask does not claim (`0xA500`) plus bit 0
/// (`INT_VRING`-shaped). An ISR that asserts `status ==` this value
/// proves the read saw the host write, not a guest-produced zero; an
/// ISR that masks with `1` still publishes a 1-bit level signal.
pub const IRQ_HOST_STATUS_MAGIC: u32 = 0x0000_A501;
/// Virtio ISR status register offset (03 §6 / the worked `VirtioIrqMmio`).
pub const IRQ_STATUS_OFFSET: u64 = 0x60;

/// One bound pool as it was actually placed: everything the checker
/// resolved about the declaration (`PoolBacking` — 03-hardware.md §3's
/// size, purpose, device reachability and alignment) plus the one fact
/// only placement knows, its guest base address.
///
/// The `base`/`bytes` pair is what plans/M7.md decision 5 turns into a
/// security property: the report emits one `BlkPool name= base= size=`
/// line per *device-reachable* pool, and that list is the whole of what
/// the VMM maps for its device model (`wrela-vmm`'s own
/// `devices::GuestMem::window_offset` admits an address only if it lies
/// wholly inside one of them). Everything else in the image — code,
/// rodata, the actor runtime tables, another pool's slots — is outside
/// every window by construction, which `verify_pool_windows` below
/// re-derives rather than assumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolPlacement {
    pub backing: crate::eval::image_checks::PoolBacking,
    pub base: u64,
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
/// **The vector table**: 06 §4 calls for a static vector table so a set
/// bit dispatches to its registered service. Bit 0 is always
/// `__wrela_vector0_service` (deadline/cancel). Device-owned bits
/// (`1..=63`, decision 12) dispatch by a compile-time-unrolled per-bit
/// test-and-`BL` against each `IrqCap.bind` site the sealed graph
/// recorded — still not an rtdata-loaded table (CLAUDE.md: no layers for
/// their own sake); the targets are `Reloc::Call` into the `code`
/// section, patched once bases are known. An image with no device
/// vectors keeps the M6 byte-identical single-`BL` / whole-word-clear
/// loop (pinned by every pre-G checkpoint golden).
///
/// **Mask-arm-recheck**: loop { read pending; if work, dispatch set bits
/// and AND-clear only those bits (`BIC`); drain every driver's sticky
/// wake-pending bit into its `@task` (fixed-point until quiet); reread }.
/// A raise landing between read and clear is serviced on the next
/// iteration. Single-core / no nesting (03 §6 rev 0.1) keeps a plain
/// load/store sufficient; multi-core would need an atomic RMW clear.
///
/// Returns words plus `__wrela_checkpoint_service`'s word offset within
/// them (not `0` — vector0 is placed first). Callers resolve
/// `Reloc::CheckpointService` against
/// `section_base + checkpoint_service_word * 4`. `relocs` carries every
/// ISR/`@task` `BL` (word offsets relative to the block start).
///
/// **Vector-0 / ISR / `@task` contract**: called via `BL` with the
/// caller's `x30` already saved — may clobber `x0..x14`, must preserve
/// `x28`/`sp`, returns via ordinary `ret`. ISR and `@task` bodies take
/// `x0 = driver_state` (the ordinary method receiver). Pending-bit
/// clear is this service's job after dispatch, never the routine's.
pub fn build_checkpoint_and_vector_stub(group: Option<&GroupServiceCtx>) -> CheckpointBlock {
    build_checkpoint_and_vector_stub_ex(group, &[], &[])
}

/// plans/M7.md item G: full checkpoint builder. `irq_vectors` / `wake_drains`
/// empty ⇒ byte-identical to the M6 single-vector loop.
pub fn build_checkpoint_and_vector_stub_ex(
    group: Option<&GroupServiceCtx>,
    irq_vectors: &[IrqVectorEntry],
    wake_drains: &[WakeDrainEntry],
) -> CheckpointBlock {
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
    if let Some(g) = group.filter(|g| g.arena_capacity > 0) {
        emit_deadline_scan_and_delivery(&mut a, g);
    }
    a.push(encode::enc_ret(30));

    // --- __wrela_checkpoint_service ---
    let checkpoint_service_word = a.abs();
    a.push(encode::enc_sub_imm(X_SP, X_SP, 16, true)); // sub sp, sp, #16
    a.push(encode::enc_str_x_imm(30, X_SP, 0)); // str x30, [sp]  (BL below clobbers it)
    let pending_addr = wrela_machine::pending::core_word_addr(0);
    let multi = !irq_vectors.is_empty() || !wake_drains.is_empty();
    if !multi {
        // M6 byte-identical path: one vector, whole-word clear.
        let loop_top = a.abs();
        a.load_imm(SCRATCH_A, pending_addr);
        a.push(encode::enc_ldr_x_imm(SCRATCH_B, SCRATCH_A, 0));
        let skip_done = a.skip_placeholder(); // cbz X_B, .done
        a.bl_to(vector0_start);
        a.load_imm(SCRATCH_A, pending_addr);
        a.push(encode::enc_str_x_imm(X_ZR, SCRATCH_A, 0));
        a.b_to(loop_top);
        let done = a.abs();
        a.patch_cbz(skip_done, SCRATCH_B);
        debug_assert_eq!(done, a.abs());
    } else {
        // plans/M7.md item G: multi-vector + wake-pending drain.
        // Registers inside the loop (reloaded after every BL):
        //   x9  = pending word address / scratch
        //   x10 = pending bits (live snapshot)
        //   x11 = clear-mask accumulator / did_work flag
        //   x12 = per-bit test
        //   x0  = driver state (ISR / @task receiver)
        let loop_top = a.abs();
        a.push(encode::enc_movz(SCRATCH_C, 0, 0, true)); // did_work = 0
        a.load_imm(SCRATCH_A, pending_addr);
        a.push(encode::enc_ldr_x_imm(SCRATCH_B, SCRATCH_A, 0));
        let skip_pending = a.skip_placeholder(); // cbz pending, .after_pending

        // clear_mask accumulator in x11 for the pending half; did_work
        // is rebuilt as 1 once any bit is serviced.
        a.push(encode::enc_movz(SCRATCH_C, 0, 0, true)); // clear_mask = 0

        // bit 0 → vector0
        a.push(encode::enc_movz(9, 1, 0, true));
        a.push(encode::enc_and_reg(12, SCRATCH_B, 9, true));
        let skip_v0 = a.skip_placeholder(); // cbz x12, .skip_v0
        // Preserve pending + clear_mask across the BL (callee clobbers x9..).
        a.push(encode::enc_sub_imm(X_SP, X_SP, 16, true));
        a.push(encode::enc_str_x_imm(SCRATCH_B, X_SP, 0));
        a.push(encode::enc_str_x_imm(SCRATCH_C, X_SP, 8));
        a.bl_to(vector0_start);
        a.push(encode::enc_ldr_x_imm(SCRATCH_B, X_SP, 0));
        a.push(encode::enc_ldr_x_imm(SCRATCH_C, X_SP, 8));
        a.push(encode::enc_add_imm(X_SP, X_SP, 16, true));
        a.push(encode::enc_movz(9, 1, 0, true));
        a.push(encode::enc_orr_reg(SCRATCH_C, SCRATCH_C, 9, true)); // clear_mask |= 1
        a.patch_cbz(skip_v0, 12);

        // Device-owned vectors.
        for entry in irq_vectors {
            let mask = 1u64 << (entry.vector & 63);
            a.load_imm(9, mask);
            a.push(encode::enc_and_reg(12, SCRATCH_B, 9, true));
            let skip = a.skip_placeholder();
            a.push(encode::enc_sub_imm(X_SP, X_SP, 32, true));
            a.push(encode::enc_str_x_imm(SCRATCH_B, X_SP, 0));
            a.push(encode::enc_str_x_imm(SCRATCH_C, X_SP, 8));
            a.push(encode::enc_str_x_imm(9, X_SP, 16)); // mask
            a.load_imm(0, entry.driver_state); // x0 = self
            a.bl_call_key(&entry.handler_key);
            a.push(encode::enc_ldr_x_imm(SCRATCH_B, X_SP, 0));
            a.push(encode::enc_ldr_x_imm(SCRATCH_C, X_SP, 8));
            a.push(encode::enc_ldr_x_imm(9, X_SP, 16));
            a.push(encode::enc_add_imm(X_SP, X_SP, 32, true));
            a.push(encode::enc_orr_reg(SCRATCH_C, SCRATCH_C, 9, true));
            a.patch_cbz(skip, 12);
        }

        // BIC-clear serviced bits, keep any raise that landed mid-dispatch.
        a.load_imm(SCRATCH_A, pending_addr);
        a.push(encode::enc_ldr_x_imm(SCRATCH_B, SCRATCH_A, 0));
        a.push(encode::enc_bic_reg(SCRATCH_B, SCRATCH_B, SCRATCH_C, true));
        a.push(encode::enc_str_x_imm(SCRATCH_B, SCRATCH_A, 0));
        a.push(encode::enc_movz(SCRATCH_C, 1, 0, true)); // did_work = 1
        a.patch_cbz(skip_pending, SCRATCH_B);

        // Sticky wake-pending drain (03 §6 mask–arm–recheck for the
        // ISR→bottom-half edge). Fixed-point: a `@task` that re-wakes
        // itself is consumed before this service returns.
        let wake_top = a.abs();
        a.push(encode::enc_movz(12, 0, 0, true)); // any_wake = 0
        for w in wake_drains {
            let pending_word = w.driver_state + w.wake_pending_off;
            a.load_imm(SCRATCH_A, pending_word);
            a.push(encode::enc_ldr_x_imm(SCRATCH_B, SCRATCH_A, 0));
            let skip_w = a.skip_placeholder();
            a.push(encode::enc_str_x_imm(X_ZR, SCRATCH_A, 0)); // clear first
            a.push(encode::enc_sub_imm(X_SP, X_SP, 16, true));
            a.push(encode::enc_str_x_imm(12, X_SP, 0)); // save any_wake
            a.push(encode::enc_str_x_imm(SCRATCH_C, X_SP, 8)); // save did_work
            a.load_imm(0, w.driver_state);
            a.bl_call_key(&w.task_key);
            a.push(encode::enc_ldr_x_imm(12, X_SP, 0));
            a.push(encode::enc_ldr_x_imm(SCRATCH_C, X_SP, 8));
            a.push(encode::enc_add_imm(X_SP, X_SP, 16, true));
            a.push(encode::enc_movz(12, 1, 0, true)); // any_wake = 1
            a.push(encode::enc_movz(SCRATCH_C, 1, 0, true)); // did_work = 1
            a.patch_cbz(skip_w, SCRATCH_B);
        }
        a.push(encode::enc_cbnz(
            12,
            ((wake_top as i64 - a.abs() as i64) * 4) as i32,
            true,
        ));

        // Recheck: pending raise or wake during the drains above.
        a.push(encode::enc_cbnz(
            SCRATCH_C,
            ((loop_top as i64 - a.abs() as i64) * 4) as i32,
            true,
        ));
    }
    a.push(encode::enc_ldr_x_imm(30, X_SP, 0));
    a.push(encode::enc_add_imm(X_SP, X_SP, 16, true));
    a.push(encode::enc_ret(30));

    // --- __wrela_deadline_poll (plans/M6.md item F #3) ------------------
    let deadline_poll_word = match group.filter(|g| g.arena_capacity > 0) {
        Some(g) => {
            let start = a.abs();
            emit_deadline_poll(&mut a, g);
            Some(start)
        }
        None => None,
    };

    CheckpointBlock {
        words: a.words,
        checkpoint_service_word,
        deadline_poll_word,
        relocs: a.relocs,
    }
}

/// One sealed `IrqCap.bind` site, ready for the checkpoint dispatch loop.
/// `driver_state` is 0 on the sizing pass and the absolute state address
/// on the real-address pass (word count never depends on the value).
#[derive(Debug, Clone)]
pub struct IrqVectorEntry {
    pub vector: u64,
    pub handler_key: String,
    pub driver_state: u64,
}

/// One `@driver`'s sticky wake-pending → `@task` drain site.
#[derive(Debug, Clone)]
pub struct WakeDrainEntry {
    pub driver_state: u64,
    pub wake_pending_off: u64,
    pub task_key: String,
}

/// The whole-image facts the vector-0 deadline service and the scheduler's
/// own deadline poll need (plans/M6.md item F #2/#3). Every address here is
/// a real, already-placed `rtdata` address — which is why both routines are
/// built twice, placeholder then real, exactly like every other
/// address-bearing hand-assembled routine in this module (word counts never
/// depend on address *values*, only on `arena_capacity`/`turn_areas.len()`,
/// both of which are known before placement).
#[derive(Debug, Clone, Default)]
pub struct GroupServiceCtx {
    pub arena_base: u64,
    pub arena_capacity: u64,
    /// Every turn area in the image (each actor's, then each free async
    /// fn's) — the set the delivery half scans to find suspended turns
    /// whose own ambient group has just been cancelled. Each entry is that
    /// turn's build-time `(address, TurnId)` pair: the scan still addresses
    /// the turn record absolutely, but plans/M10.md item 0c2 made
    /// `OFF_GROUP_OWNER_TURN` a `TurnId`, so the owner test compares the id
    /// rather than the address. Both come from the same
    /// `RuntimePlacement::turn_addr` expression, so they can never name
    /// different bytes.
    pub turn_areas: Vec<(u64, TurnId)>,
}

/// `build_checkpoint_and_vector_stub`'s own result: the block's words plus
/// the two entry points a caller must resolve against `section_base`.
pub struct CheckpointBlock {
    pub words: Vec<u32>,
    /// `__wrela_checkpoint_service`'s own word offset within `words`.
    pub checkpoint_service_word: usize,
    /// `__wrela_deadline_poll`'s own word offset, present only for a build
    /// that actually has a group arena.
    pub deadline_poll_word: Option<usize>,
    /// `Reloc::Call` sites for ISR / `@task` bodies (word offsets relative
    /// to the block start when built with `Asm::new(0)`).
    pub relocs: Vec<Reloc>,
}

/// The shape-only (`base = 0`) service context a sizing pass needs: the
/// arena capacity and the *number* of turn areas are both build-time facts,
/// known long before placement, and they are the only things the emitted
/// word count depends on.
fn group_service_shape(runtime: Option<&RuntimeTables>) -> Option<GroupServiceCtx> {
    let tables = runtime.filter(|t| t.group_arena_capacity > 0)?;
    Some(GroupServiceCtx {
        arena_base: 0,
        arena_capacity: tables.group_arena_capacity,
        // Shape only: the emitted word count depends on the *number* of
        // turn areas, never on any address or id value (every `load_imm`
        // is a fixed four words). `TurnId::from_index(0)` is a stand-in
        // for exactly that reason — the real ids arrive with
        // `group_service_ctx` below.
        turn_areas: vec![(0, TurnId::from_index(0)); tables.actors.len() + tables.free_turns.len()],
    })
}

/// The real service context, once `rtdata` is placed: every turn area in
/// the image (each actor's, then each free async fn's — `place_runtime_tables`'s
/// own byte order) plus the group arena's own base.
fn group_service_ctx(
    placement: &RuntimePlacement,
    tables: &RuntimeTables,
) -> Option<GroupServiceCtx> {
    if tables.group_arena_capacity == 0 {
        return None;
    }
    // An actor's `TurnId` is its `tables.actors` index (`turn_id_for`'s own
    // positional rule); a free turn's is the one `place_runtime_tables`
    // recorded in `turn_ids` under the same key `free_turns` uses. Order is
    // kept identical to the shape pass's, and to what this scan emitted
    // before item 0c2 — a reordering is behaviourally inert (each unrolled
    // arm is self-contained) but would make the golden `rtcode` diff
    // unreadable.
    let mut turn_areas: Vec<(u64, TurnId)> = placement
        .actors
        .iter()
        .enumerate()
        .map(|(i, a)| (a.turn, TurnId::from_index(i)))
        .collect();
    for (key, &addr) in &placement.free_turns {
        let Some(&id) = placement.turn_ids.get(key) else {
            // `place_runtime_tables` fills `free_turns` and `turn_ids` from
            // the same loop over the same keys, so this is unreachable;
            // skipping rather than panicking leaves the shape-vs-real word
            // count assert as the one thing that reports a disagreement,
            // loudly, the way every other producer bug here does.
            continue;
        };
        turn_areas.push((addr, id));
    }
    Some(GroupServiceCtx {
        arena_base: placement.group_arena,
        arena_capacity: tables.group_arena_capacity,
        turn_areas,
    })
}

/// The vector-0 service's real body (plans/M6.md item F #2, 04-compiler.md
/// §4): the deadline scan, then cancellation delivery. Runs synchronously
/// inside `__wrela_checkpoint_service`'s own dispatch, so it inherits that
/// routine's contract verbatim — may clobber `x9..x14`, must preserve
/// `x28`/`sp`, no calls of its own (so `x30` needs no saving here).
///
/// **Step 1, the scan** — a fully-unrolled linear walk of the static arena
/// (CLAUDE.md's "linear scans over the static arena", no timer wheel):
/// every `in_use`, not-yet-`cancelled` slot with a nonzero `deadline_ns`
/// that the clock has passed gets `cancelled = 1`. The clock read is the
/// ordinary trapping `CLOCK_MMIO_ADDR` load (`codegen::emit_now`'s own
/// address), so it is a real, recorded `ChoiceRead` in the VMM's choice
/// sequence and replays from the log rather than from the host clock.
///
/// **Step 2, "cancels child registrations recursively" (04 §4), which this
/// scan already performs — recorded, because it looks like an omission**:
/// deadlines only ever *narrow* (`codegen::emit_group_create` stores
/// `min(ambient, own)`, with a group declaring no deadline of its own
/// inheriting the ambient one unchanged), so every descendant of an expired
/// group carries an effective deadline no later than its ancestor's and is
/// therefore expired at the very same instant this same single pass
/// examines it. Deadline expiry is also M6's *only* cancellation source
/// (no `race`, no explicit cancel API, and an abandon is image-fatal per
/// decision 12). A separate parent-to-child propagation pass would
/// therefore be provably dead code today — and, worse, an *unsound* one
/// unless iterated to a fixed point, since a child group can occupy a lower
/// arena index than its parent (`GroupCreate` claims the first free slot).
/// The day a second cancellation source exists, the fixed-point pass is the
/// thing to add here.
///
/// **Step 3, delivery to parked turns**: a turn suspended on an `await`
/// whose own ambient group has just been cancelled may have nothing left
/// that would ever wake it, so the scan makes it `resume_ready` — its own
/// resume path then composes `CallError::Cancelled` and terminates at the
/// checkpoint that follows (`codegen::emit_await_resume`/
/// `emit_checkpoint_cancellation_test`). A group's own *owner* turn is
/// deliberately excluded (`codegen::OFF_GROUP_OWNER_TURN`): its frame is
/// never terminated, so force-resuming it would hand the `with`-block's own
/// body a reply that never arrived. **Disclosed floor**: an owner parked on
/// an await that can never resolve is therefore not woken by this scan — it
/// is woken transitively when its own children are cancelled and harvested
/// (`layout::build_group_child_poll`), which is the only shape any M6
/// golden constructs; a group with no outstanding children whose owner
/// awaits an actor that never replies is not constructible at M6's own
/// acyclic-handle source surface (item D's own recorded finding).
fn emit_deadline_scan_and_delivery(a: &mut Asm, g: &GroupServiceCtx) {
    use crate::codegen::{
        GROUP_SLOT_SIZE, OFF_GROUP_CANCELLED, OFF_GROUP_DEADLINE, OFF_GROUP_IN_USE,
        OFF_GROUP_OWNER_TURN, OFF_TURN_BUSY, OFF_TURN_RESUME_READY, OFF_TURN_SUSPENDED,
        TURN_RECORD_SIZE,
    };
    const NOW: u8 = SCRATCH_A; // x9  — the clock read, live across the whole scan
    const SLOT: u8 = SCRATCH_B; // x10 — the candidate slot address
    const T0: u8 = SCRATCH_C; // x11
    const T1: u8 = 12;
    const T2: u8 = 13;

    a.load_imm(T0, wrela_machine::mmio::CLOCK_MMIO_ADDR);
    a.push(encode::enc_ldr_x_imm(NOW, T0, 0));

    for i in 0..g.arena_capacity {
        a.load_imm(SLOT, g.arena_base + i * GROUP_SLOT_SIZE);
        a.push(encode::enc_ldr_x_imm(T0, SLOT, OFF_GROUP_IN_USE as u16));
        let skip_a = a.skip_placeholder(); // cbz -> next slot
        a.push(encode::enc_ldr_x_imm(T0, SLOT, OFF_GROUP_CANCELLED as u16));
        let skip_b = a.skip_placeholder(); // cbnz -> already cancelled
        a.push(encode::enc_ldr_x_imm(T0, SLOT, OFF_GROUP_DEADLINE as u16));
        let skip_c = a.skip_placeholder(); // cbz -> no deadline
        // Expired iff now >= deadline (unsigned — both are raw ns).
        a.push(encode::enc_cmp_reg(NOW, T0, true));
        let skip_d = a.skip_placeholder(); // b.cc -> not yet
        a.load_imm(T1, 1);
        a.push(encode::enc_str_x_imm(T1, SLOT, OFF_GROUP_CANCELLED as u16));
        let next = a.abs();
        a.patch_cbz(skip_a, T0);
        a.patch_cbnz(skip_b, T0);
        a.patch_cbz(skip_c, T0);
        a.patch_cond(skip_d, Cond::Cc);
        debug_assert_eq!(next, a.abs());
    }

    for &(turn, turn_id) in &g.turn_areas {
        a.load_imm(T0, turn);
        a.push(encode::enc_ldr_x_imm(T1, T0, OFF_TURN_BUSY as u16));
        let skip_a = a.skip_placeholder(); // cbz -> not busy
        a.push(encode::enc_ldr_x_imm(T1, T0, OFF_TURN_SUSPENDED as u16));
        let skip_b = a.skip_placeholder(); // cbz -> running, not parked
        // Ambient group = this turn's own frame `Temp(0)`, always the first
        // slot past the 48-byte turn record (`flowwir::FrameLayout`'s own
        // fixed lineage convention, `codegen::LINEAGE_GROUP_SLOT`).
        a.push(encode::enc_ldr_x_imm(T1, T0, TURN_RECORD_SIZE as u16));
        let skip_c = a.skip_placeholder(); // cbz -> no ambient group
        a.push(encode::enc_sub_imm(T1, T1, 1, true));
        a.load_imm(T2, GROUP_SLOT_SIZE);
        a.push(encode::enc_mul(T1, T1, T2, true));
        a.load_imm(SLOT, g.arena_base);
        a.push(encode::enc_add_reg(SLOT, SLOT, T1, true));
        a.push(encode::enc_ldr_x_imm(T1, SLOT, OFF_GROUP_CANCELLED as u16));
        let skip_d = a.skip_placeholder(); // cbz -> not cancelled
        // plans/M10.md item 0c2: `owner_turn` is an `Option[TurnId]` (a
        // `u32` at +56), so this arm compares the build-time `TurnId` of
        // the turn it is about — not, as before, the build-time *address*
        // of that turn. Equality only, so no index→address step. `ldr w` /
        // `cmp w`: an `x` load here would fold the padding word above the
        // field in as high bits and no comparison would ever match.
        a.push(encode::enc_ldr_w_imm(T1, SLOT, OFF_GROUP_OWNER_TURN as u16));
        a.load_imm(T2, turn_id.get() as u64);
        a.push(encode::enc_cmp_reg(T1, T2, false));
        let skip_e = a.skip_placeholder(); // b.eq -> this turn owns the group
        a.load_imm(T1, 1);
        a.push(encode::enc_str_x_imm(T1, T0, OFF_TURN_RESUME_READY as u16));
        let next = a.abs();
        a.patch_cbz(skip_a, T1);
        a.patch_cbz(skip_b, T1);
        a.patch_cbz(skip_c, T1);
        a.patch_cbz(skip_d, T1);
        a.patch_cond(skip_e, Cond::Eq);
        debug_assert_eq!(next, a.abs());
    }
}

/// `__wrela_deadline_poll()` (plans/M6.md item F #3) — the scheduler's own
/// half of the deadline protocol, called once per entry-driver scheduler
/// tick. Two jobs, in one linear scan of the static arena:
///
/// 1. **Arm the park** (06-machine.md §5): the minimum effective deadline
///    over every live, not-yet-cancelled group is written to
///    `machine_info::OFF_NEXT_DEADLINE` (`0` when no group has one), which
///    is exactly what the entry driver's own park branch reads and what the
///    VMM sleeps until. Written every tick rather than maintained
///    incrementally — the arena is a handful of static slots, and an
///    incremental min would need invalidation on every create/close/cancel
///    (CLAUDE.md's cleverness budget: no profile, no cleverness).
///
/// 2. **Raise the deadline vector when the guest is *running***. M6's real
///    injector is this service (decision 7), but the VMM can only raise a
///    vector at an exit, and a `.wr` program that always has ready work
///    never parks — at M6 nothing else can block a turn forever (item D's
///    own finding: no deadlock is constructible at the acyclic-handle
///    source surface), so a spinning child would otherwise run past its
///    deadline unnoticed. So when the poll finds the minimum deadline
///    already passed it sets this core's own pending word (bit 0) — the
///    identical word the VMM's own raise writes, observed the identical
///    way. That routing is not ceremony: setting the pending word instead
///    of calling the scan directly is *what makes the cancellation land at
///    a checkpoint* (02-language.md §9.5: "never between arbitrary
///    instructions") rather than at whatever instruction the scheduler
///    happened to be at, and it is the only reason the injection point is
///    deterministic and replay-identical.
///
/// The clock read is the ordinary trapping `CLOCK_MMIO_ADDR` load, so every
/// poll that finds a live deadline costs one recorded `ClockRead` — real,
/// deliberate, and the honest price of a tick-granularity deadline service
/// with no timer hardware. Leaf routine: clobbers `x9..x13` only, never
/// `x28`/`sp`, and calls nothing.
fn emit_deadline_poll(a: &mut Asm, g: &GroupServiceCtx) {
    use crate::codegen::{
        GROUP_SLOT_SIZE, OFF_GROUP_CANCELLED, OFF_GROUP_DEADLINE, OFF_GROUP_IN_USE,
    };
    const MIN: u8 = SCRATCH_A; // x9 — 0 = no live deadline
    const SLOT: u8 = SCRATCH_B; // x10
    const T0: u8 = SCRATCH_C; // x11
    const T1: u8 = 12;
    const T2: u8 = 13;

    a.push(encode::enc_movz(MIN, 0, 0, true));
    for i in 0..g.arena_capacity {
        a.load_imm(SLOT, g.arena_base + i * GROUP_SLOT_SIZE);
        a.push(encode::enc_ldr_x_imm(T0, SLOT, OFF_GROUP_IN_USE as u16));
        let skip_a = a.skip_placeholder(); // cbz -> next
        a.push(encode::enc_ldr_x_imm(T0, SLOT, OFF_GROUP_CANCELLED as u16));
        let skip_b = a.skip_placeholder(); // cbnz -> already cancelled
        a.push(encode::enc_ldr_x_imm(T0, SLOT, OFF_GROUP_DEADLINE as u16));
        let skip_c = a.skip_placeholder(); // cbz -> no deadline
        // min = (min == 0 || this < min) ? this : min
        a.push(encode::enc_cmp_reg(T0, MIN, true));
        a.push(encode::enc_csel(T1, T0, MIN, Cond::Cc, true)); // T1 = this < min ? this : min
        a.push(encode::enc_cmp_imm(MIN, 0, true));
        a.push(encode::enc_csel(MIN, T0, T1, Cond::Eq, true)); // min == 0 -> take this
        let next = a.abs();
        a.patch_cbz(skip_a, T0);
        a.patch_cbnz(skip_b, T0);
        a.patch_cbz(skip_c, T0);
        debug_assert_eq!(next, a.abs());
    }
    a.load_imm(
        T0,
        machine_layout::MACHINE_INFO_BASE + machine_info::OFF_NEXT_DEADLINE,
    );
    a.push(encode::enc_str_x_imm(MIN, T0, 0));
    let skip_done = a.skip_placeholder(); // cbz MIN -> nothing armed
    a.load_imm(T0, wrela_machine::mmio::CLOCK_MMIO_ADDR);
    a.push(encode::enc_ldr_x_imm(T1, T0, 0)); // T1 = now
    a.push(encode::enc_cmp_reg(T1, MIN, true));
    let skip_not_yet = a.skip_placeholder(); // b.cc -> deadline still in the future
    a.load_imm(T0, wrela_machine::pending::core_word_addr(0));
    a.load_imm(T2, 1);
    a.push(encode::enc_str_x_imm(T2, T0, 0)); // raise vector 0
    let done = a.abs();
    a.patch_cbz(skip_done, MIN);
    a.patch_cond(skip_not_yet, Cond::Cc);
    debug_assert_eq!(done, a.abs());
    a.push(encode::enc_ret(30));
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

/// The one diagnostic both image flavors report for a `Reloc::Call` whose
/// target is neither a compiled fn nor one of this image's own runtime-glue
/// routines.
///
/// The `__rt_enqueue_X` arm is a **real, user-reachable source condition**,
/// not an internal inconsistency, and was reported as the latter until the
/// item-F/G follow-up audit: `codegen` emits one of these for every
/// `await`/`send` through an `Actor[X]` handle, while `layout` only builds
/// an `rt_enqueue` routine for an actor this image actually *declares*
/// (`RuntimeTables::actors`, straight from `ImageGraph::actors`). A program
/// that types fine — an `Actor[X]` parameter is a perfectly good type
/// whether or not any `X` instance is declared — and whose `@image` fn
/// never calls `img.actor(X, ...)` lands here through `wrela build`,
/// `wrela dump --stage=report` and `wrela test` alike. It gets a named
/// diagnostic that says what to do, in the same voice
/// `resolve_runtime_test_args` already uses for the sibling "no unique
/// declared instance" condition.
///
/// The other arm keeps the internal-error framing on purpose: an ordinary
/// compiled-fn key that reached relocation without codegen ever producing
/// it is a `lower`/`codegen` disagreement about the program's own call
/// graph, never anything a source file can express — both halves of every
/// key here come out of the same `MwirProgram`/`FlowWirProgram` this same
/// `CodegenProgram` was built from.
fn unresolved_call_target(target: &str, graph: Option<&ImageGraph>) -> LayoutError {
    let Some(actor) = crate::codegen::rt_enqueue_actor(target) else {
        return LayoutError::new(format!(
            "internal error: call target `{target}` was never codegen'd or registered as a \
             runtime-glue symbol"
        ));
    };
    // A `@driver` *is* an actor root (02 §9.1) and `resolve_runtime_test_args`
    // already resolves an `Actor[T]` handle against `graph.drivers` too. Since
    // plans/M8.md item D a driver gets a mailbox and an admission routine
    // exactly when its declaration carries `mailbox=` (05-library.md §9) — so
    // what lands here is the narrower, still-real condition: a declared
    // `@driver` that was never made messageable. It gets its own sentence,
    // naming the one label that fixes it, rather than the (wrong) "you never
    // declared it" advice.
    let declared_driver = graph.is_some_and(|g| {
        g.drivers
            .iter()
            .any(|d| crate::sema::types::render_type(&d.actor_type) == actor)
    });
    if declared_driver {
        return LayoutError::new(format!(
            "this image sends to `{actor}` (an `await` or `send` through an `Actor[{actor}]` \
             handle), but `{actor}` is declared as a `@driver` with no `mailbox=` — a driver is \
             messageable only when its declaration says so (05-library.md §9): add \
             `mailbox=n` to `img.driver({actor}, ...)`, or remove the call. Without one there \
             is no mailbox to admit into and no admission routine to call"
        ));
    }
    LayoutError::new(format!(
        "this image sends to actor `{actor}` (an `await` or `send` through an \
         `Actor[{actor}]` handle) but never declares a `{actor}` instance — add \
         `img.actor({actor}, mailbox=...)` to the `@image` fn, or remove the call: a \
         handle type with no declared instance has no mailbox to admit into"
    ))
}

/// plans/M8.md item C2: `__rt_xsend_<src core>_<Actor>` — the cross-core
/// send routine an `__rt_enqueue_<Actor>` call is *redirected* to when the
/// call site's own core is not the target's. One symbol per (sending core,
/// target mailbox root) pair, because the ring it writes is one per
/// (sending core, target mailbox root) pair.
fn xsend_symbol(src_core: usize, actor: &str) -> String {
    format!("__rt_xsend_{src_core}_{actor}")
}

/// plans/M8.md item C2: which core a call site runs on. An actor method
/// runs on its actor's core, a `@driver` method on its driver's (core 0 by
/// shape decision 2), and anything else is a free turn — and the only free
/// turns that run are the root turns core 0's entry driver drives.
fn caller_core(caller_key: &str, w: &RuntimeWiring) -> usize {
    attributed_core(caller_key, w).unwrap_or(0)
}

/// The same lookup as [`caller_core`], but **honest about not knowing**.
///
/// `caller_core`'s `None => 0` fallback reads "a free key is a free turn,
/// and free turns are core 0's entry driver's". That is true of a free
/// *turn*; it is false of a free *function*, which is an ordinary callee
/// and runs on whatever core its caller runs on. The two are the same key
/// shape — no `Actor.` prefix — so `turn_owner` cannot tell them apart.
///
/// For a *sizing* question the difference does not matter (a free fn owns
/// no turn area either way). For a *proof* question it decides the answer:
/// `reject_unlowerable_cross_core_shapes`' checkpoint arm exists to prove a
/// fn does **not** run off core 0, and answering "core 0" for a key it
/// cannot attribute makes the proof vacuous exactly where it is needed.
/// That was a live defect — a loop hoisted out of a core-1 actor method
/// into a free fn passed the guard and then ate core 0's cross-core wake,
/// hanging the image (`golden/err-cross-core-checkpoint-free-fn`).
///
/// A free key that owns a **free turn area** (`RuntimeTables::free_turns`
/// — the `@test(runtime)` roots and free `async fn`s) is still core 0, and
/// positively so: 06 §3 makes boot and the root turns the entry core's, and
/// `reply_ring_capacity`'s own comment records the same. That is the line
/// between the two free-key shapes, and it is why this returns `Some(0)`
/// for one and `None` for the other.
fn attributed_core(caller_key: &str, w: &RuntimeWiring) -> Option<usize> {
    let actor_names: Vec<String> = w.tables.actors.iter().map(|a| a.name.clone()).collect();
    let driver_names: Vec<String> = w.tables.drivers.iter().map(|d| d.name.clone()).collect();
    if let Some(owner) =
        turn_owner(caller_key, &actor_names).or_else(|| turn_owner(caller_key, &driver_names))
    {
        return Some(w.placement.core_of_actor_type(owner).unwrap_or(0));
    }
    if w.tables.free_turns.iter().any(|(k, _)| k == caller_key) {
        return Some(0);
    }
    None
}

/// plans/M8.md item C2, the item that lifted C1's own build error: a
/// `send`/`await` whose sender and target live on **different cores** is
/// now *lowered*, not refused — 04-compiler.md §3's "cross-core actor
/// edges keep identical message semantics, lowered to compiler-generated
/// bounded SPSC rings in guest memory".
///
/// Returns the symbol the `Reloc::Call` should resolve to: `None` keeps
/// codegen's own `__rt_enqueue_<Actor>` (every same-core edge, every
/// single-core image — the as-if fast path §3's last sentence preserves by
/// name), `Some(sym)` redirects to that edge's `__rt_xsend_*`. Codegen
/// emits exactly one symbolic call either way and never learns which it
/// got, which is what makes the two paths' message semantics identical by
/// construction rather than by agreement.
///
/// Two shapes are still refused here, each named rather than approximated:
/// an actor struct with instances on two different cores (the generated
/// admission routine is per struct, so there is no honest core to compare
/// against), and a cross-core target with an aggregate-reply method (see
/// `cross_core_rings`' own refusal — the aggregate never rides the ring).
fn resolve_cross_core_edge(
    caller_key: &str,
    target: &str,
    wiring: Option<&RuntimeWiring>,
) -> Result<Option<String>, LayoutError> {
    let Some(w) = wiring else {
        return Ok(None);
    };
    if w.placement.cores <= 1 {
        return Ok(None);
    }
    let Some(target_actor) = crate::codegen::rt_enqueue_actor(target) else {
        return Ok(None);
    };
    let caller = caller_core(caller_key, w);
    let Some(target_core) = w.placement.core_of_actor_type(&target_actor) else {
        // Two instances of one actor struct on two different cores: the
        // generated admission routine is keyed by struct name and cannot
        // tell them apart, so there is no honest core to compare against.
        return Err(LayoutError::new(format!(
            "this image declares `{target_actor}` instances on more than one core, but the \
             generated admission routine (`{target}`) is per actor struct, not per instance — \
             give each instance its own struct, or place them on one core (plans/M8.md item C1)"
        )));
    };
    if caller == target_core {
        return Ok(None);
    }
    let sym = xsend_symbol(caller, &target_actor);
    Ok(Some(sym))
}

/// plans/M8.md item C2: every cross-core message edge this image's own
/// compiled code actually contains, as `(sending core, target mailbox
/// root)` pairs. Derived from the sealed graph's placement (04 §3: "the
/// inputs, the inference, and the final table are published in the report
/// and sealed into the build identity") crossed with the `Reloc::Call`
/// sites codegen emitted — never from a heuristic, and never from the
/// wiring graph alone, which records handles rather than message sites.
fn cross_core_edges(
    program: &CodegenProgram,
    w: &RuntimeWiring,
) -> Result<BTreeSet<(usize, String)>, LayoutError> {
    let mut out = BTreeSet::new();
    if w.placement.cores <= 1 {
        return Ok(out);
    }
    for (key, f) in &program.fns {
        for reloc in &f.relocs {
            let Reloc::Call { key: target, .. } = reloc else {
                continue;
            };
            if let Some(sym) = resolve_cross_core_edge(key, target, Some(w))? {
                let actor = crate::codegen::rt_enqueue_actor(target)
                    .expect("resolve_cross_core_edge only redirects an rt_enqueue target")
                    .to_string();
                debug_assert_eq!(sym, xsend_symbol(caller_core(key, w), &actor));
                out.insert((caller_core(key, w), actor));
            }
        }
    }
    Ok(out)
}

/// plans/M8.md item C2: this image's own ring set, in the canonical order
/// the report publishes and `place_runtime_tables` places — request lanes
/// first, then reply lanes, each sorted by `(src, dst, actor)`.
///
/// **Capacity comes from the sealed graph, and an edge whose capacity
/// cannot be derived is a build error naming the edge** (CLAUDE.md's
/// fail-closed rule; no silent truncation and no spin-until-space):
///
/// - a **request** ring `s -> d` for target `A` is exactly as deep as
///   `A`'s own declared mailbox, and carries `A`'s own mailbox slot format
///   — the ring is a staging area in front of one mailbox, so making it a
///   second, differently-shaped queue would have been inventing a bound
///   nothing declared. A zero-capacity mailbox is refused by name.
/// - a **reply** ring `d -> s` is as deep as the number of turn areas on
///   core `s`, which is a hard bound on outstanding replies bound for `s`:
///   a turn area holds at most one in-flight activation (non-reentrancy,
///   04 §2) and therefore at most one outstanding `await`. That is what
///   makes `BRK_XREPLY_RING_FULL` an unreachability guard rather than a
///   dropped reply.
fn cross_core_rings(
    program: &CodegenProgram,
    w: &RuntimeWiring,
) -> Result<Vec<RingLayout>, LayoutError> {
    let edges = cross_core_edges(program, w)?;
    if edges.is_empty() {
        return Ok(Vec::new());
    }
    let mut requests: Vec<RingLayout> = Vec::new();
    let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (src, actor) in &edges {
        let Some(dst) = w.placement.core_of_actor_type(actor) else {
            return Err(LayoutError::new(format!(
                "internal error: cross-core edge to `{actor}` has no single placed core"
            )));
        };
        let Some((mailbox_capacity, slot_size)) = mailbox_root_shape(&w.tables, actor) else {
            return Err(LayoutError::new(format!(
                "internal error: cross-core edge to `{actor}`, which has no runtime mailbox"
            )));
        };
        if mailbox_capacity == 0 {
            return Err(LayoutError::new(format!(
                "core {src} sends to actor `{actor}` on core {dst}, but `{actor}` declares a \
                 mailbox capacity of 0: a cross-core edge's ring is sized from the mailbox it \
                 feeds (04-compiler.md §3, plans/M8.md item C2), so there is no capacity to \
                 derive — declare `mailbox=` on that `img.actor(...)`"
            )));
        }
        requests.push(RingLayout {
            src: *src,
            dst,
            kind: RingKind::Request,
            actor: Some(actor.to_string()),
            capacity: mailbox_capacity,
            slot_size,
        });
        pairs.insert((*src, dst));
    }
    let mut replies: Vec<RingLayout> = Vec::new();
    for (src, dst) in &pairs {
        // The reply flows the other way: produced on `dst`, consumed on
        // `src`, sized by the turn areas that live on `src`.
        let capacity = reply_ring_capacity(w, *src);
        if capacity == 0 {
            return Err(LayoutError::new(format!(
                "core {src} sends to core {dst}, but core {src} owns no turn area, so the reply \
                 ring's capacity cannot be derived (plans/M8.md item C2)"
            )));
        }
        replies.push(RingLayout {
            src: *dst,
            dst: *src,
            kind: RingKind::Reply,
            actor: None,
            capacity,
            slot_size: REPLY_SLOT_SIZE,
        });
    }
    requests.sort_by(|a, b| (a.src, a.dst, &a.actor).cmp(&(b.src, b.dst, &b.actor)));
    replies.sort_by(|a, b| (a.src, a.dst).cmp(&(b.src, b.dst)));
    requests.extend(replies);
    Ok(requests)
}

/// plans/M8.md item C2's two fail-closed arms — the shapes this item
/// lowers *around* rather than through, each refused by name instead of
/// approximated (CLAUDE.md: "an unimplemented path errors loudly; it never
/// approximates"). Both exist only because a turn can now run on a
/// secondary core at all; neither is reachable in a single-core image.
///
/// **1. An aggregate reply across a core boundary.** 04 §3 requires a
/// cross-core edge's message semantics to be identical, and this item
/// delivers that by routing the request and the reply *word* over rings.
/// An aggregate reply does not travel that way: `build_rt_select_and_run`'s
/// aggregate arm loads `[waker + OFF_TURN_REPLY_SLOT]` and hands the callee
/// the awaiting turn's own staging-slot address in `x8`, and the callee
/// writes the aggregate straight into that frame — a direct store into
/// another core's memory with no ring and no ordering the compiler placed
/// there.
///
/// Two reasons, and the first was found by removing this arm and running
/// the gate rather than by reading the code. (a) **The waker that reaches
/// that arm is core-tagged** (decision 30), so `[waker + ...]` dereferences
/// an address with the tag still in its top bits: the boot faults on core 1
/// with `FAR_EL1=0x2000000040501660` — the tag, not memory. Untagging it
/// there is a one-line change, which is what makes reason (b) the real one.
/// (b) Even untagged, the store is a direct cross-core write with no
/// publication this compiler placed, and under decision 11's baton **no
/// oracle this project owns could show it broken** — a green boot would be
/// evidence of the baton, not of correct publish/acquire ordering. Shipping
/// it would be shipping an untested claim, so it is refused instead.
///
/// Refused per target actor rather than per method, because the ring's own
/// slot format is per actor.
///
/// **2. A checkpoint inside a turn placed on a secondary core.**
/// `__wrela_checkpoint_service` and every `codegen::FnCtx::checkpoint` test
/// name **core 0's** pending word by a baked-in constant
/// (`pending::core_word_addr(0)`), and the service clears that whole word.
/// A turn running on core 1 that reached a loop back-edge would therefore
/// service — and *clear* — core 0's pending word, silently eating the very
/// wake this item's rings raise. Per-core checkpoint services (and the
/// per-core deadline word they imply) are real work with their own
/// oracles; they are not smuggled in here.
fn reject_unlowerable_cross_core_shapes(
    rings: &[RingLayout],
    w: &RuntimeWiring,
    boot: &BootCtx,
    program: &CodegenProgram,
) -> Result<(), LayoutError> {
    let _ = boot;
    if w.placement.cores <= 1 {
        return Ok(());
    }
    for ring in rings.iter().filter(|r| r.kind == RingKind::Request) {
        let Some(actor) = &ring.actor else { continue };
        let Some((_, methods)) = w.dispatch.iter().find(|(name, _)| name == actor) else {
            continue;
        };
        if let Some((key, _, _)) = methods.iter().find(|(_, _, agg)| *agg) {
            return Err(LayoutError::new(format!(
                "core {} sends to actor `{actor}` on core {}, but `{key}` declares an aggregate \
                 reply: an aggregate is written straight into the awaiting turn's frame through \
                 `x8`, which does not travel the cross-core reply ring this edge lowers to \
                 (04-compiler.md §3, plans/M8.md item C2). Place both ends on one core, or make \
                 the reply a scalar",
                ring.src, ring.dst
            )));
        }
    }
    for (key, f) in &program.fns {
        if !f
            .relocs
            .iter()
            .any(|r| matches!(r, Reloc::CheckpointService { .. }))
        {
            continue;
        }
        // Fail closed on "cannot attribute", not just on "attributed to a
        // secondary core": the whole job of this arm is to *prove* the fn
        // runs on core 0, and an unattributed key is exactly the case that
        // shipped the hang.
        match attributed_core(key, w) {
            Some(0) => {}
            Some(core) => {
                return Err(LayoutError::new(format!(
                    "`{key}` runs on core {core} and contains a checkpoint (a loop back-edge), \
                     but `__wrela_checkpoint_service` and every checkpoint test name core 0's \
                     own pending word by construction — servicing one from core {core} would \
                     clear the wake a cross-core ring raised for core 0. Place this actor on \
                     core 0, or remove the loop; per-core checkpoint services are not part of \
                     plans/M8.md item C2"
                )));
            }
            None => {
                return Err(LayoutError::new(format!(
                    "`{key}` contains a checkpoint (a loop back-edge) and is not owned by any \
                     declared actor or driver, so this build cannot prove which core it runs \
                     on. In a multi-core image that is refused: a checkpoint serviced from a \
                     secondary core clears core 0's pending word and eats the wake a cross-core \
                     ring raised for it. Move the loop into a method of an actor placed on core \
                     0, or remove it; per-core checkpoint services are not part of plans/M8.md \
                     item C2"
                )));
            }
        }
    }
    Ok(())
}

/// plans/M10.md item D / decision 613: `(mailbox-root name, capacity,
/// slot_size)` for every actor and messageable driver — the inputs
/// `emit_rt_enqueue` specializes on. Independent of async frame sizes
/// (slot width is method shapes only), so callers can compute this
/// before `codegen_program_with_async`.
pub fn mailbox_enqueue_specs(
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
) -> Result<Vec<(String, u64, u64)>, String> {
    if graph.actors.is_empty() && graph.drivers.is_empty() {
        return Ok(Vec::new());
    }
    let shapes = merge_actor_pub_methods(modules, layout_ctx).map_err(|e| e.message)?;
    let mut out = Vec::new();
    for decl in &graph.actors {
        let name = crate::sema::types::render_type(&decl.actor_type);
        let mailbox_capacity = declared_mailbox_capacity(&decl.args, &format!("actor `{name}`"))?
            .ok_or_else(|| {
            format!(
                "actor `{name}` has no declared `mailbox=` capacity (plans/M6.md decision 3: \
                 the declared bound is the whole of M6's own mailbox-capacity story; derivation \
                 is out of scope)"
            )
        })?;
        let methods = shapes.get(&name).map(Vec::as_slice).unwrap_or(&[]);
        let max_args_bytes = methods
            .iter()
            .map(|m| m.param_sizes.iter().sum::<u64>())
            .max()
            .unwrap_or(0);
        let slot_size = 16 + max_args_bytes;
        out.push((name, mailbox_capacity, slot_size));
    }
    for decl in &graph.drivers {
        let name = crate::sema::types::render_type(&decl.actor_type);
        let Some(capacity) = declared_mailbox_capacity(&decl.args, &format!("driver `{name}`"))?
        else {
            continue;
        };
        let methods = shapes.get(&name).map(Vec::as_slice).unwrap_or(&[]);
        let max_args_bytes = methods
            .iter()
            .map(|m| m.param_sizes.iter().sum::<u64>())
            .max()
            .unwrap_or(0);
        let slot_size = 16 + max_args_bytes;
        out.push((name, capacity, slot_size));
    }
    Ok(out)
}

/// Resolve mailbox root `name`'s ring bookkeeping addresses from a live
/// placement (plans/M10.md item D / decision 614).
fn resolve_mailbox_ring_addrs(
    placement: &RuntimePlacement,
    tables: &RuntimeTables,
    name: &str,
) -> Option<RingAddrs> {
    if let Some((i, _)) = tables
        .actors
        .iter()
        .enumerate()
        .find(|(_, a)| a.name == name)
    {
        return placement.actors.get(i).map(|a| a.mailbox());
    }
    for (i, d) in tables.drivers.iter().enumerate() {
        if d.mailbox.is_some() && d.name == name {
            return placement.driver_mailboxes.get(&i).map(|a| a.mailbox());
        }
    }
    None
}

/// One mailbox root's own `(capacity, slot size)` — a declared actor's, or
/// (plans/M8.md item D) a messageable `@driver`'s. The one lookup
/// `cross_core_rings` needs, so a ring feeding a driver's mailbox is sized
/// from that mailbox exactly as one feeding an actor's is.
fn mailbox_root_shape(tables: &RuntimeTables, name: &str) -> Option<(u64, u64)> {
    if let Some(a) = tables.actors.iter().find(|a| a.name == name) {
        return Some((a.mailbox_capacity, a.slot_size));
    }
    tables
        .drivers
        .iter()
        .find(|d| d.name == name)
        .and_then(|d| d.mailbox.as_ref())
        .map(|m| (m.capacity, m.slot_size))
}

/// How many turn areas live on `core` — every mailbox root placed there,
/// plus (on
/// core 0 only) every free-turn area, since 06 §3 makes boot and the root
/// turns the entry core's. The bound `build_rt_xreply`'s own
/// unreachability argument rests on.
fn reply_ring_capacity(w: &RuntimeWiring, core: usize) -> u64 {
    let actors = w.actor_cores.iter().filter(|c| **c == core).count() as u64;
    let free = if core == 0 {
        w.tables.free_turns.len() as u64
    } else {
        0
    };
    actors + free
}

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

/// Does `@driver` `name` declare any `@task` method? Walks the raw
/// modules (attrs live on the AST; `LayoutCtx` has types only).
fn driver_declares_task(modules: &BTreeMap<String, Module>, name: &str) -> bool {
    !driver_task_method_names(modules, name).is_empty()
}

/// Every `@task` method name on `@driver` `name` (AST walk).
fn driver_task_method_names(modules: &BTreeMap<String, Module>, name: &str) -> Vec<String> {
    // Decision 18: runtime tables pass `BlkDriver[DriverMode.Irq]`; the
    // AST struct is the bare name.
    let bare = name.split('[').next().unwrap_or(name);
    let mut out = Vec::new();
    for m in modules.values() {
        for item in &m.items {
            let crate::syntax::ast::Item::Struct(s) = item else {
                continue;
            };
            if s.name != bare {
                continue;
            }
            if !s.attrs.iter().any(|a| a.name == "driver") {
                continue;
            }
            for mem in &s.members {
                if let crate::syntax::ast::Member::Fn(f) = mem {
                    if f.attrs.iter().any(|a| a.name == "task") {
                        out.push(f.name.clone());
                    }
                }
            }
        }
    }
    out
}

/// plans/M7.md item G: ISR bind sites + wake drains for the checkpoint
/// service. Addresses are 0 on the sizing pass; the real-address pass
/// fills them from `placement.drivers`.
fn checkpoint_irq_shape(
    boot: Option<&BootCtx>,
    placement: Option<&RuntimePlacement>,
    tables: Option<&RuntimeTables>,
) -> (Vec<IrqVectorEntry>, Vec<WakeDrainEntry>) {
    let Some(boot) = boot else {
        return (Vec::new(), Vec::new());
    };
    let mut irq_vectors = Vec::new();
    let mut wake_drains = Vec::new();
    for (di, decl) in boot.graph.drivers.iter().enumerate() {
        let crate::sema::types::Type::Named(driver, targs) = &decl.actor_type else {
            continue;
        };
        let state = placement
            .and_then(|p| p.drivers.get(di).copied())
            .unwrap_or(0);
        // Decision 18: codegen keys for a mode-generic driver are
        // `struct:BlkDriver[DriverMode.Irq].method` (MethodInstance).
        let key_prefix = if targs.is_empty() {
            driver.clone()
        } else {
            format!(
                "struct:{}",
                crate::sema::types::render_type(&decl.actor_type)
            )
        };
        let vector = device_index_of(&decl.args)
            .and_then(|i| boot.graph.devices.get(i))
            .and_then(|d| crate::eval::image_checks::device_vector(&d.args));
        if let Some(v) = vector {
            for handler in irq_bind_handlers_in_driver(boot.modules, driver) {
                irq_vectors.push(IrqVectorEntry {
                    vector: v,
                    handler_key: format!("{key_prefix}.{handler}"),
                    driver_state: state,
                });
            }
        }
        if let Some(tables) = tables {
            if let Some(off) = tables.drivers.get(di).and_then(|d| d.wake_pending_off) {
                for task in driver_task_method_names(boot.modules, driver) {
                    wake_drains.push(WakeDrainEntry {
                        driver_state: state,
                        wake_pending_off: off,
                        task_key: format!("{key_prefix}.{task}"),
                    });
                }
            }
        }
    }
    (irq_vectors, wake_drains)
}

/// AST walk: every `*.bind(self.<handler>)` site inside `@driver` `driver`.
/// `check_vector_bindings` already validated these; layout only needs the
/// handler names for the dispatch table.
fn irq_bind_handlers_in_driver(modules: &BTreeMap<String, Module>, driver: &str) -> Vec<String> {
    let mut out = Vec::new();
    for m in modules.values() {
        for item in &m.items {
            let crate::syntax::ast::Item::Struct(s) = item else {
                continue;
            };
            if s.name != driver {
                continue;
            }
            for mem in &s.members {
                let body: &[crate::syntax::ast::Stmt] = match mem {
                    crate::syntax::ast::Member::Fn(f) => f.body.as_deref().unwrap_or(&[]),
                    crate::syntax::ast::Member::Init(i) => &i.body,
                    _ => continue,
                };
                collect_bind_handlers_stmts(body, &mut out);
            }
        }
    }
    out
}

fn collect_bind_handlers_stmts(stmts: &[crate::syntax::ast::Stmt], out: &mut Vec<String>) {
    use crate::syntax::ast::Stmt;
    for s in stmts {
        match s {
            Stmt::Expr(_, e) | Stmt::Send(_, e) => collect_bind_handlers_expr(e, out),
            Stmt::Assign(a) => {
                collect_bind_handlers_expr(&a.target, out);
                collect_bind_handlers_expr(&a.value, out);
            }
            Stmt::Return(_, Some(e)) => collect_bind_handlers_expr(e, out),
            Stmt::If(i) => {
                collect_bind_handlers_expr(&i.cond, out);
                collect_bind_handlers_stmts(&i.then_branch, out);
                for elif in &i.elifs {
                    collect_bind_handlers_expr(&elif.cond, out);
                    collect_bind_handlers_stmts(&elif.body, out);
                }
                if let Some(b) = &i.else_branch {
                    collect_bind_handlers_stmts(b, out);
                }
            }
            Stmt::Match(m) => {
                collect_bind_handlers_expr(&m.scrutinee, out);
                for arm in &m.arms {
                    collect_bind_handlers_stmts(&arm.body, out);
                }
            }
            Stmt::While(w) => {
                collect_bind_handlers_expr(&w.cond, out);
                collect_bind_handlers_stmts(&w.body, out);
            }
            Stmt::For(f) => {
                collect_bind_handlers_expr(&f.iterable, out);
                collect_bind_handlers_stmts(&f.body, out);
            }
            Stmt::ComptimeIf(c) => {
                collect_bind_handlers_stmts(&c.then_branch, out);
                if let Some(b) = &c.else_branch {
                    collect_bind_handlers_stmts(b, out);
                }
            }
            Stmt::With(w) => {
                collect_bind_handlers_expr(&w.expr, out);
                collect_bind_handlers_stmts(&w.body, out);
            }
            Stmt::Defer(d) => match &d.body {
                crate::syntax::ast::DeferBody::Suite(body) => {
                    collect_bind_handlers_stmts(body, out);
                }
                crate::syntax::ast::DeferBody::Expr(e) => collect_bind_handlers_expr(e, out),
            },
            Stmt::Assert(a) => {
                collect_bind_handlers_expr(&a.cond, out);
                if let Some(m) = &a.message {
                    collect_bind_handlers_expr(m, out);
                }
            }
            Stmt::ComptimeAssert(_, cond, msg) => {
                collect_bind_handlers_expr(cond, out);
                if let Some(m) = msg {
                    collect_bind_handlers_expr(m, out);
                }
            }
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Return(_, None) | Stmt::Pass(_) => {}
        }
    }
}

fn collect_bind_handlers_expr(e: &crate::syntax::ast::Expr, out: &mut Vec<String>) {
    use crate::syntax::ast::Expr;
    match e {
        Expr::Call(callee, _, args) => {
            if let Expr::Field(_, _, method) = callee.as_ref() {
                if method == "bind" {
                    for a in args {
                        if let Expr::Field(base, _, handler) = &a.value {
                            if matches!(base.as_ref(), Expr::Name(_, n) if n == "self") {
                                out.push(handler.clone());
                            }
                        }
                    }
                }
            }
            collect_bind_handlers_expr(callee, out);
            for a in args {
                collect_bind_handlers_expr(&a.value, out);
            }
        }
        Expr::Field(base, _, _)
        | Expr::Unary(_, _, base)
        | Expr::Not(_, base)
        | Expr::Try(_, base)
        | Expr::Send(_, base) => collect_bind_handlers_expr(base, out),
        Expr::Index(base, _, args) => {
            collect_bind_handlers_expr(base, out);
            for a in args {
                collect_bind_handlers_expr(a, out);
            }
        }
        Expr::Binary(_, _, l, r) | Expr::And(_, l, r) | Expr::Or(_, l, r) => {
            collect_bind_handlers_expr(l, out);
            collect_bind_handlers_expr(r, out);
        }
        Expr::Tuple(_, items) | Expr::List(_, items) => {
            for i in items {
                collect_bind_handlers_expr(i, out);
            }
        }
        Expr::ArrayRepeat(_, elem, count) => {
            collect_bind_handlers_expr(elem, out);
            collect_bind_handlers_expr(count, out);
        }
        Expr::DotVariant(_, _, args) => {
            for a in args {
                collect_bind_handlers_expr(&a.value, out);
            }
        }
        Expr::Range(_, a, b, _) => {
            collect_bind_handlers_expr(a, out);
            collect_bind_handlers_expr(b, out);
        }
        Expr::Is(_, scrutinee, _) => collect_bind_handlers_expr(scrutinee, out),
        Expr::Closure(c) => {
            if let crate::syntax::ast::ClosureBody::Expr(e) = &c.body {
                collect_bind_handlers_expr(e, out);
            }
        }
        Expr::Name(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::BStr(..)
        | Expr::Char(..)
        | Expr::Bool(..)
        | Expr::Unit(..)
        | Expr::FStr(_) => {}
    }
}

/// Host injects for every device that owns a vector **and** has a bound
/// ISR. The status value is always `IRQ_HOST_STATUS_MAGIC` at
/// `IRQ_STATUS_OFFSET` — the HVF oracle's hand-computed host write.
fn build_irq_host_injects(
    boot: Option<&BootCtx>,
    device_regs: &[DeviceRegs],
) -> Vec<IrqHostInject> {
    let Some(boot) = boot else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for r in device_regs {
        let Some(dev) = boot.graph.devices.get(r.device) else {
            continue;
        };
        let Some(vector) = crate::eval::image_checks::device_vector(&dev.args) else {
            continue;
        };
        // Decision 18: DeviceRegs.driver is the rendered instantiation
        // name; the AST struct is the bare name.
        let bare = r.driver.split('[').next().unwrap_or(r.driver.as_str());
        if irq_bind_handlers_in_driver(boot.modules, bare).is_empty() {
            continue;
        }
        out.push(IrqHostInject {
            base: r.base,
            offset: IRQ_STATUS_OFFSET,
            status: IRQ_HOST_STATUS_MAGIC,
            vector,
        });
    }
    out
}

/// plans/M7.md item G: absolute address of `@driver` `driver`'s sticky
/// wake-pending word (trailing word of its state). `placement.drivers`
/// already holds absolute addresses (`place_runtime_tables` starts its
/// cursor at `rtdata_base`).
fn driver_wake_pending_addr(
    placement: &RuntimePlacement,
    tables: &RuntimeTables,
    driver: &str,
) -> Result<u64, LayoutError> {
    for (i, d) in tables.drivers.iter().enumerate() {
        // Decision 18: runtime table names are rendered
        // (`BlkDriver[DriverMode.Irq]`); `Inst::Wake` carries the bare
        // struct name from the FnRef.
        let bare = d.name.split('[').next().unwrap_or(d.name.as_str());
        if d.name != driver && bare != driver {
            continue;
        }
        let Some(off) = d.wake_pending_off else {
            // Unreachable from source: `sema` rejects `wake(D.m)` when `m`
            // is not `@task` (`golden/err-wake-not-task`), and only a
            // `@task` reserves the wake-pending word.
            return Err(LayoutError::new(format!(
                "internal error: `Wake` for `{driver}` but that driver has no `@task` \
                 (no wake-pending word was reserved)"
            )));
        };
        let Some(&state_base) = placement.drivers.get(i) else {
            // Unreachable from source: `place_runtime_tables` emits one
            // base per `tables.drivers` entry.
            return Err(LayoutError::new(format!(
                "internal error: `@driver` `{driver}` has no placed state"
            )));
        };
        return Ok(state_base + off);
    }
    // Author-reachable: a `@driver` with `wake(...)` compiled into the
    // module, while this `@image` never declared that driver (sibling of
    // `irq_driver_undeclared` / the LoadIrqVector soak find).
    Err(wake_driver_undeclared(driver))
}

fn wake_driver_undeclared(driver: &str) -> LayoutError {
    LayoutError::new(format!(
        "`wake` names `@driver` `{driver}`, which this image never declared — add \
         `img.driver({driver}, ...)` to the `@image` fn, or remove the `wake` \
         (03-hardware.md §6)"
    ))
}

/// plans/M10.md item 0c3: the one message shared by every
/// `Reloc::TurnsBase`/`Reloc::TurnStride` resolution guard — one producer-bug
/// site rather than four copies of the same sentence.
///
/// Unreachable from any source program, and the reason is structural: the
/// only emitter of either reloc is `codegen::push_turn_addr_from_id`, called
/// only from `emit_queue_drain`, which needs a `@driver` bound to a device
/// with a virtqueue — and that requires a sealed `@image`, which is exactly
/// what makes `RuntimePlacement` exist. Kept rather than unwrapped: a
/// producer bug that somehow reached here must fail closed and loudly.
fn turns_deref_needs_rtdata() -> String {
    "internal error: a virtqueue drain needs the `RT.turns` base and stride to reach the turn a \
     slot's waiter/reply-stage names, but this image's runtime tables were never placed"
        .to_string()
}

fn wake_needs_rtdata(driver: &str) -> LayoutError {
    LayoutError::new(format!(
        "`wake` for `@driver` `{driver}` needs a sealed `@image` that declares that driver — \
         this layout has no runtime tables for it. Add `img.driver({driver}, ...)` to an \
         `@image` fn, or remove the `wake` (03-hardware.md §6)"
    ))
}

/// plans/M7.md item G, decision 12: the vector bit index an `IrqCap` for
/// `@driver` `driver` materializes. Read from the sealed graph's
/// `vector=` on that driver's bound device — the same fact
/// `eval::image_checks::check_vector_bindings` already validated.
///
/// The "no graph / driver never declared" arms are author-reachable: a
/// `@driver` that binds an IRQ lowers a `LoadIrqVector`, and
/// `layout_test_image` will try to patch it even when this module has no
/// `@image` (or an `@image` that never wires that driver). Those get a
/// named `error[build]` diagnostic — never `internal error:`, which is
/// reserved for states only a producer bug can make. The remaining arms
/// (`no device=`, missing `device#i`, no `vector=`) stay internal: every
/// sealed graph that reaches here already passed `check_init_args` /
/// `check_vector_bindings` / `check_driver_mode` (plans/M8.md item H soak,
/// seed 8103 find).
fn driver_irq_vector(graph: Option<&ImageGraph>, driver: &str) -> Result<u64, LayoutError> {
    // Decision 18: `LoadIrqVector` may carry `struct:BlkDriver[DriverMode.Irq]`
    // (instantiation owner) or the bare `BlkDriver`.
    let bare_want = driver
        .strip_prefix("struct:")
        .unwrap_or(driver)
        .split('[')
        .next()
        .unwrap_or(driver);
    let Some(graph) = graph else {
        return Err(irq_driver_undeclared(bare_want));
    };
    for decl in &graph.drivers {
        let crate::sema::types::Type::Named(name, _) = &decl.actor_type else {
            continue;
        };
        if name != driver && name != bare_want {
            continue;
        }
        let Some(i) = device_index_of(&decl.args) else {
            return Err(LayoutError::new(format!(
                "internal error: `@driver` `{driver}` has a `LoadIrqVector` but no `device=` \
                 binding"
            )));
        };
        let Some(dev) = graph.devices.get(i) else {
            return Err(LayoutError::new(format!(
                "internal error: `@driver` `{driver}` binds device#{i}, which does not exist"
            )));
        };
        return crate::eval::image_checks::device_vector(&dev.args).ok_or_else(|| {
            LayoutError::new(format!(
                "internal error: `@driver` `{driver}` has a `LoadIrqVector` but its device \
                 declared no `vector=` — `check_vector_bindings` should have rejected first"
            ))
        });
    }
    Err(irq_driver_undeclared(bare_want))
}

/// Author-reachable refusal: a `LoadIrqVector` reloc has nowhere to read
/// the vector from. Shared by the `graph: None` path (lower-fuzz /
/// `layout_test_image` without a `BootCtx`) and the empty/missing-driver
/// path (`wrela test` with no `@image`, or an `@image` that never wired
/// this driver) — both are the same author mistake.
fn irq_driver_undeclared(driver: &str) -> LayoutError {
    LayoutError::new(format!(
        "`LoadIrqVector` names `@driver` `{driver}`, which this image never declared — add \
         `img.driver({driver}, device=...)` with `vector=N` (1..=63) on the device to an \
         `@image` fn, or drop the IRQ bind for a poll build (03-hardware.md §6/§7)"
    ))
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

/// Internal-error audit (the item-F/G follow-up's own second half): every
/// `Err` below is genuinely unreachable from any source program, and stays
/// framed as an internal error for that reason. Nothing here reads the
/// program at all — every `base`/`size` argument was computed moments
/// earlier by `layout_program`/`layout_test_image` from one monotonically
/// advancing `cursor`, and `blob_len` from the identical word/byte counts
/// that fixed those sizes. A source file cannot make two sections overlap,
/// open a gap wider than the 8-byte alignment either fn ever rounds to, or
/// move the first section off `IMAGE_BASE`; only an editing mistake in
/// those two placement bodies can, which is precisely what this fn is for.
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

// --- pool backing: the `pooldata` section (plans/M7.md item D) ------------
//
// 05-library.md §9: `img.pool`/`img.dma_pool` "reserve exact backing". The
// reservation is zeroed image bytes in one section named `pooldata`,
// exactly like `rtdata`'s own actor tables and for the identical reason
// (no allocation anywhere in this machine; every byte is sized at build
// time). It is this image shape's own final section, after `rtdata`.
//
// **Why one section rather than one per pool.** A section is a *report*
// fact the VMM presence-checks; a pool window is a *mapping* fact the VMM
// enforces, and the two are reported separately and deliberately — the
// per-pool `Pool`/`BlkPool` lines carry each window's own base and size
// (`render_layout_section`), so nothing is lost by keeping the section
// table one entry wider rather than N entries wider. It also keeps
// `verify_section_sizes`' own "gaps are alignment padding only" rule
// intact: a pool declared with 1-byte alignment next to one declared with
// 8 would otherwise produce an inter-section gap that rule would (rightly)
// refuse.

/// Places every bound pool's backing sequentially from `cursor`, each at
/// its own declared alignment, in `backings`' own name-sorted order
/// (`image.report.deterministic`: a `BTreeMap` walk, no other ordering
/// input exists). Returns the placements, the section's own base/size and
/// the advanced cursor — `None` when this image binds no pool at all, in
/// which case no `pooldata` section exists and nothing about the image
/// changes (which is why every golden without a pool stayed byte-identical
/// when this landed).
fn place_pools(
    cursor: u64,
    sections: &[Section],
    backings: &BTreeMap<String, crate::eval::image_checks::PoolBacking>,
) -> Result<Option<(Vec<PoolPlacement>, u64, u64, u64)>, LayoutError> {
    if backings.is_empty() {
        return Ok(None);
    }
    // `pooldata` is the last section either image flavor places, so its
    // base must be past every section already placed. Checked here rather
    // than left to `verify_section_sizes`, which runs after serialization
    // — and serialization's own `pad_to` would trip a `debug_assert`
    // first, which is a panic in a debug build and silence in a release
    // one. This is the exact bug the first draft of this item had (the
    // `rtdata` block never advanced `cursor`, because it used to be the
    // final section), so it gets a real error rather than an assumption.
    let placed_end = sections
        .iter()
        .map(|s| s.base + s.size)
        .max()
        .unwrap_or(cursor);
    if cursor < placed_end {
        return Err(LayoutError::new(format!(
            "internal error: pool backing would be placed at {cursor:#x}, inside a section that \
             ends at {placed_end:#x}"
        )));
    }
    Ok(place_pools_unchecked(cursor, backings))
}

/// The placement itself, with no section-table check — the half
/// `layout_test_image` needs *before* its section table exists, because
/// plans/M7.md item H1 made a pool's base an `init` argument word and the
/// boot-init block is assembled first. `place_pools` (above) is this plus
/// the check, so the two can never place a pool differently.
fn place_pools_unchecked(
    cursor: u64,
    backings: &BTreeMap<String, crate::eval::image_checks::PoolBacking>,
) -> Option<(Vec<PoolPlacement>, u64, u64, u64)> {
    if backings.is_empty() {
        return None;
    }
    let base = round_up(cursor, 8);
    let mut at = base;
    let mut out = Vec::with_capacity(backings.len());
    for b in backings.values() {
        at = round_up(at, b.align.max(1));
        out.push(PoolPlacement {
            backing: b.clone(),
            base: at,
        });
        at += b.bytes;
    }
    Some((out, base, at - base, at))
}

/// plans/M7.md decision 5, re-derived rather than asserted: **the windows
/// this image declares reachable are pool backing and nothing else.**
///
/// The VMM maps exactly the `BlkPool name= base= size=` lines
/// `render_layout_section` emits, and treats every address inside one of
/// them as device-reachable (`wrela-vmm`'s `devices::GuestMem`). So the
/// security property on the compiler's side is a placement property, and
/// this function checks it from the finished section table and the
/// finished placement list — the same inputs the report is rendered from,
/// not the intermediate cursors that produced them:
///
/// 1. every pool window is non-empty and lies wholly inside the
///    `pooldata` section;
/// 2. no two pool windows overlap;
/// 3. `pooldata` is disjoint from every other section — which
///    `verify_section_sizes` already proves for the whole table, so (1)
///    is what extends that proof to each individual window.
///
/// Together: an address inside any declared window is inside `pooldata`,
/// therefore outside `entry`/`code`/`rodata`/`abort`/`checkpoint`/
/// `rtcode`/`rtdata` — outside this image's own instructions, its abort
/// strings, its runtime routines and every actor's state and mailbox.
/// What it does *not* prove is anything about the VMM: that lives in
/// `wrela-vmm`'s own `GuestMem::window_offset` tests
/// (`hardware.dma.pool-reachability`), which is the other half of the same
/// sentence.
fn verify_pool_windows(sections: &[Section], pools: &[PoolPlacement]) -> Result<(), LayoutError> {
    if pools.is_empty() {
        return Ok(());
    }
    let Some(sec) = sections.iter().find(|s| s.name == "pooldata") else {
        return Err(LayoutError::new(
            "internal error: this image places pool windows but reserves no `pooldata` section",
        ));
    };
    let sec_end = sec.base + sec.size;
    for (i, p) in pools.iter().enumerate() {
        if p.backing.bytes == 0 {
            return Err(LayoutError::new(format!(
                "internal error: pool `{}` was placed with a zero-byte window",
                p.backing.name
            )));
        }
        let end = p.base.checked_add(p.backing.bytes).ok_or_else(|| {
            LayoutError::new(format!(
                "internal error: pool `{}`'s window overflows a u64",
                p.backing.name
            ))
        })?;
        if p.base < sec.base || end > sec_end {
            return Err(LayoutError::new(format!(
                "internal error: pool `{}`'s window [{:#x}, {end:#x}) is not inside the \
                 `pooldata` section [{:#x}, {sec_end:#x}) — plans/M7.md decision 5: a declared \
                 pool window is the whole of what a device can reach",
                p.backing.name, p.base, sec.base
            )));
        }
        for other in &pools[..i] {
            let other_end = other.base + other.backing.bytes;
            if p.base < other_end && other.base < end {
                return Err(LayoutError::new(format!(
                    "internal error: pools `{}` and `{}` were placed at overlapping windows",
                    other.backing.name, p.backing.name
                )));
            }
        }
    }
    Ok(())
}

/// plans/M7.md item E1 / decision 5: every reported ring region
/// (descriptor table, available ring, used ring, doorbell) is re-derived
/// to lie wholly inside the named DMA pool's backing — `verify_pool_windows`'
/// sibling for the ring. A second local derivation that could disagree
/// about which bytes the device reaches is forbidden: both this check and
/// the emitter call `virtqueue::place_ring` against the same pool base and
/// depth.
fn verify_ring_windows(
    pools: &[PoolPlacement],
    blk: &Option<BlkReport>,
) -> Result<(), LayoutError> {
    let Some(blk) = blk else {
        return Ok(());
    };
    let q = &blk.queue;
    let Some(pool) = pools.iter().find(|p| p.backing.name == q.pool_name) else {
        return Err(LayoutError::new(format!(
            "internal error: BlkQueue names pool `{}`, which has no placed window",
            q.pool_name
        )));
    };
    if pool.backing.device.is_none() {
        return Err(LayoutError::new(format!(
            "internal error: BlkQueue's pool `{}` is not device-reachable — decision 5: only \
             DMA pools are device-reachable memory",
            q.pool_name
        )));
    }
    let Some(placed) = crate::virtqueue::place_ring(pool.base, q.size) else {
        return Err(LayoutError::new(format!(
            "internal error: BlkQueue size {} is not a nonzero power of two",
            q.size
        )));
    };
    if placed.desc != q.desc
        || placed.avail != q.avail
        || placed.used != q.used
        || placed.doorbell != q.doorbell
    {
        return Err(LayoutError::new(format!(
            "internal error: reported BlkQueue ring addresses disagree with \
             virtqueue::place_ring(pool_base={:#x}, depth={}) — emitter and verifier must share \
             one derivation",
            pool.base, q.size
        )));
    }
    if placed.bytes > pool.backing.bytes {
        return Err(LayoutError::new(format!(
            "internal error: ring for queue depth {} needs {} bytes but pool `{}` only has {}",
            q.size, placed.bytes, q.pool_name, pool.backing.bytes
        )));
    }
    let pool_end = pool.base + pool.backing.bytes;
    for (what, addr, len) in [
        (
            "descriptor table",
            q.desc,
            crate::virtqueue::desc_bytes(q.size),
        ),
        (
            "available ring",
            q.avail,
            crate::virtqueue::avail_bytes(q.size),
        ),
        ("used ring", q.used, crate::virtqueue::used_bytes(q.size)),
        (
            "doorbell word",
            q.doorbell,
            crate::virtqueue::DOORBELL_BYTES,
        ),
    ] {
        let end = addr.checked_add(len).ok_or_else(|| {
            LayoutError::new(format!(
                "internal error: blk {what} address overflows a u64"
            ))
        })?;
        if addr < pool.base || end > pool_end {
            return Err(LayoutError::new(format!(
                "internal error: blk {what} at [{addr:#x}, {end:#x}) is not inside pool `{}`'s \
                 window [{:#x}, {pool_end:#x}) — plans/M7.md decision 5",
                q.pool_name, pool.base
            )));
        }
    }
    Ok(())
}

/// Collect every module's recorded `VirtQueue.configure` sites
/// (`TypedProgram::virtqueue_configures`, filled by sema). Machine v1
/// allows exactly one; more than one fails closed here.
fn find_virtqueue_configure(
    programs: &BTreeMap<String, crate::sema::typed::TypedProgram>,
) -> Result<Option<(String, u16)>, LayoutError> {
    let mut found: Option<(String, u16)> = None;
    for prog in programs.values() {
        for site in &prog.virtqueue_configures {
            if found.is_some() {
                return Err(LayoutError::new(
                    "this image has more than one `VirtQueue.configure` call; machine v1's                      `blk` has exactly one queue"
                        .to_string(),
                ));
            }
            found = Some(site.clone());
        }
    }
    Ok(found)
}

/// Build the `BlkDevice`/`BlkQueue` report facts from placed pools and
/// the driver's `VirtQueue.configure` site. Returns `None` when no
/// configure exists (pool-only images stay without a device model).
pub fn derive_blk_report(
    pools: &[PoolPlacement],
    graph: &ImageGraph,
    programs: &BTreeMap<String, crate::sema::typed::TypedProgram>,
) -> Result<Option<BlkReport>, LayoutError> {
    let Some((pool_name, depth)) = find_virtqueue_configure(programs)? else {
        return Ok(None);
    };
    let Some(pool) = pools.iter().find(|p| p.backing.name == pool_name) else {
        return Err(LayoutError::new(format!(
            "`VirtQueue.configure` consumes pool `{pool_name}`, which has no placed backing"
        )));
    };
    // plans/M8.md item P: the blk device *is* the device of the pool the
    // ring lives in. Nothing else in the image names it, and every
    // device-facing fact below is read from this index rather than from a
    // scan over all devices — with two declared devices those differ.
    let Some(blk_device) = pool.backing.device else {
        return Err(LayoutError::new(format!(
            "`VirtQueue.configure` consumes pool `{pool_name}`, which is not device-reachable \
             (`img.dma_pool(..., device=...)`); decision 5: only DMA pools are device-reachable"
        )));
    };
    let Some(placed) = crate::virtqueue::place_ring(pool.base, depth) else {
        return Err(LayoutError::new(format!(
            "`VirtQueue.configure`'s depth={depth} is not a nonzero power of two"
        )));
    };
    // plans/M7.md item E4 / decision 20: ring + single-flight packaging
    // (meta/header/status) must both fit; the VMM reaches only declared
    // pool bytes, so a prepare that wrote past the window would be a
    // guest fault rather than a build error.
    let Some(needed) = crate::virtqueue::control_bytes_needed(depth) else {
        return Err(LayoutError::new(format!(
            "`VirtQueue.configure`'s depth={depth} is not a nonzero power of two"
        )));
    };
    if needed > pool.backing.bytes {
        return Err(LayoutError::new(format!(
            "`VirtQueue.configure` needs a {depth}-deep ring plus packaging ({needed} bytes) but \
             pool `{pool_name}` only reserves {pool_bytes} bytes — enlarge the `img.dma_pool` or \
             shrink the depth",
            pool_bytes = pool.backing.bytes,
        )));
    }
    // plans/M8.md item P, decision 25: with two declared devices, "the
    // device that declares `capacity_sectors=`" and "the device the ring
    // lives in" stop being the same question. That they are still one
    // question is enforced at declaration time —
    // `image_checks::check_blk_config_names_the_blk_device` — which is why
    // the two graph-wide scans below stay the single derivation of each
    // fact rather than growing a device-scoped twin that could disagree
    // with the lowerer's own reading.
    let capacity = crate::eval::image_checks::blk_capacity_sectors(graph).ok_or_else(|| {
        LayoutError::new(
            "this image configures a virtio-blk queue but declares no `capacity_sectors=` on \
             its `img.device` (plans/M7.md item E1: capacity is an image-declared build constant)"
                .to_string(),
        )
    })?;
    let features = crate::eval::image_checks::blk_accepted_features(graph, programs)
        .map_err(|e| LayoutError::new(e.message))?;
    let vector = graph
        .devices
        .get(blk_device)
        .and_then(|d| crate::eval::image_checks::device_vector(&d.args));
    Ok(Some(BlkReport {
        device: blk_device,
        capacity_sectors: capacity,
        features,
        vector,
        queue: BlkQueueReport {
            index: 0,
            size: depth,
            desc: placed.desc,
            avail: placed.avail,
            used: placed.used,
            doorbell: placed.doorbell,
            pool_name,
        },
        descriptors_per_op: crate::virtqueue::DESCRIPTORS_PER_BLK_OP,
        occupancy_bound: crate::virtqueue::occupancy_bound(
            depth,
            crate::virtqueue::DESCRIPTORS_PER_BLK_OP,
        ),
    }))
}

/// Every line the VMM's own `parse_report` consumes beyond the `Machine
/// revision=`/`Input path=`/`Section name=`/`Entry base=` preamble, in one
/// place — `bin/wrela.rs`'s runtime tier and `xtask`'s own two hand-built
/// report writers all call this rather than each carrying their own copy
/// of the list.
///
/// plans/M8.md item C3 collapsed the copies rather than adding a fourth:
/// the `Ring` lines this item's admission recorder needs were the second
/// fact (after item C1's `CoreEntry`) that `bin/wrela.rs` emitted and
/// `xtask` did not, which is why `xtask`'s runtime-test images could not
/// boot a cross-core image at all. A single writer makes that class of
/// drift a compile-time impossibility instead of a discovery.
///
/// Order matters only for human readability — `parse_report` is
/// line-oriented and order-independent — and mirrors `render_layout_
/// section`'s own order so the two artifacts read alike.
pub fn append_vmm_runtime_lines(out: &mut String, layout: &ImageLayout) {
    // plans/M8.md item C1 / 06 §3: where the VMM starts vCPU N once the
    // guest rings `mmio::RELEASE_MMIO_ADDR`. Absent for a single-core image.
    for (core, base) in &layout.core_entries {
        out.push_str(&format!("CoreEntry core={core} base={base:#x}\n"));
    }
    // plans/M8.md item C3: this image's cross-core rings, addresses
    // included — 06 §8 makes the VMM the recorder of "per-mailbox
    // cross-core admission order", and the admission happens in guest
    // memory the VMM has to be told about. Absent for a single-core image.
    append_ring_vmm_lines(out, layout);
    append_blk_vmm_lines(out, layout);
    // plans/M7.md item G: host `interrupt_status` write + vector raise.
    for inj in &layout.irq_host_injects {
        out.push_str(&format!(
            "IrqHostInject base={:#x} offset={:#x} status={:#x} vector={}\n",
            inj.base, inj.offset, inj.status, inj.vector
        ));
    }
}

/// The `Ring ...` lines, in `RuntimeTables::rings` order — the same order
/// `build_rt_drain` walks its lanes, which is what makes the VMM's
/// reconstruction of admission order an ordered one. Shares
/// `render_layout_section`'s own rendering so the runtime report and the
/// `--stage=report` artifact cannot disagree about a ring.
fn append_ring_vmm_lines(out: &mut String, layout: &ImageLayout) {
    for line in ring_report_lines(layout) {
        out.push_str(&line);
        out.push('\n');
    }
}

/// One `Ring kind=... base=0x...` line per cross-core ring. Empty for an
/// image with no cross-core message edge. The addresses are recomputed
/// from the already-placed `rtdata` base through the identical
/// `place_runtime_tables` the emitter used, never by a second rule that
/// could drift from it.
fn ring_report_lines(layout: &ImageLayout) -> Vec<String> {
    let Some(tables) = &layout.runtime else {
        return Vec::new();
    };
    if tables.rings.is_empty() {
        return Vec::new();
    }
    let rtdata_base = layout
        .sections
        .iter()
        .find(|s| s.name == "rtdata")
        .map(|s| s.base)
        .unwrap_or_else(|| {
            debug_assert!(false, "an image with rings always places `rtdata`");
            0
        });
    let addrs = place_runtime_tables(rtdata_base, tables).rings;
    tables
        .rings
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "Ring kind={} src={} dst={} target={} cap={} slot={} bytes={} base={:#x}",
                r.kind_name(),
                r.src,
                r.dst,
                r.actor.as_deref().unwrap_or("-"),
                r.capacity,
                r.slot_size,
                r.bytes(),
                addrs[i].ring,
            )
        })
        .collect()
}

/// Append the VMM-facing `BlkDevice`/`BlkQueue`/`BlkPool` lines (and the
/// decision-2c accounting fact E1 can honestly derive) for a test-image
/// hand-built report. No-op when `layout.blk` is `None`.
pub fn append_blk_vmm_lines(out: &mut String, layout: &ImageLayout) {
    let Some(blk) = &layout.blk else {
        return;
    };
    out.push_str(&format!(
        "BlkDevice device=device#{} capacity_sectors={} features={:#x}{}\n",
        blk.device,
        blk.capacity_sectors,
        blk.features,
        match blk.vector {
            Some(v) => format!(" vector={v}"),
            None => String::new(),
        }
    ));
    let q = &blk.queue;
    out.push_str(&format!(
        "BlkQueue index={} size={} desc={:#x} avail={:#x} used={:#x} doorbell={:#x}\n",
        q.index, q.size, q.desc, q.avail, q.used, q.doorbell
    ));
    // Every device-reachable pool — ring *and* payload — matching the
    // full `--stage=report` `BlkPool` set (decision 5: the VMM maps exactly
    // these windows). Emitting only the queue's control pool left payload
    // DMA unmapped and failed the flagship write at the first descriptor.
    //
    // plans/M8.md item P: each line carries the device it is bound to, and
    // the set is still *every* device-reachable pool, not just this
    // device's — the VMM needs to know a window exists in order to refuse
    // it to a device that does not own it.
    for p in &layout.pools {
        let Some(dev) = p.backing.device else {
            continue;
        };
        out.push_str(&format!(
            "BlkPool name={} device=device#{dev} base={:#x} size={:#x}\n",
            p.backing.name, p.base, p.backing.bytes
        ));
    }
    out.push_str(&format!(
        "BlkAccounting descriptors_per_op={} occupancy_bound={}\n",
        blk.descriptors_per_op, blk.occupancy_bound
    ));
}

/// Fill `layout.blk` from configure sites + placed pools, then re-verify
/// ring windows. Called by every consumer that has `programs` after layout.
pub fn attach_blk_report(
    layout: &mut ImageLayout,
    graph: &ImageGraph,
    programs: &BTreeMap<String, crate::sema::typed::TypedProgram>,
) -> Result<(), LayoutError> {
    layout.blk = derive_blk_report(&layout.pools, graph, programs)?;
    verify_ring_windows(&layout.pools, &layout.blk)
}

// --- device register windows: the `devregs` section (item H1) -------------
//
// **Decision 11's other half.** A capability is one word holding a guest
// base address; a `DeviceCap[D]`'s address is *this* — the base of the
// declared register window of the device the image bound to that driver.
//
// **Why the window is a declared region of guest DRAM, not a trapping
// address.** 06-machine.md §3 is explicit that "the VMM ... preconfigures
// every device, queue, and **shared-memory window** the report declares —
// device topology is a *build output*, not a probed fact", and that "cold
// boot is a design property: there is nothing to negotiate". Machine v1
// has no virtio MMIO transport at all (`wrela-vmm`'s `devices` module doc:
// no `MagicValue`/`DeviceID`/`QueueSel` register file exists, because
// `BlkConfig` *is* the transport configuration, parsed out of the report),
// and 06 §5's own notification mechanism is a shared-memory doorbell word,
// not a trap. `wrela_machine::mmio` reserves exactly three trapping
// registers — clock, exit, park — and nothing else in that window is
// mapped, so a device register file placed there would fault the boot.
// So this machine's device registers are a declared shared-memory window,
// the same kind of thing its doorbell already is.
//
// **Why its own section, not room inside `rtdata`.** `verify_pool_windows`'
// own doc spells the property item D established: device-reachable memory
// is disjoint from this image's instructions and from every actor's state
// and mailbox. A register window is the second kind of memory a device
// model writes, so it gets the same treatment — its own section, its own
// placement check (`verify_device_windows`), never bytes interleaved with
// actor state.
//
// **Sizing comes from item C's mint set, not from a second walk.** The
// window is as wide as the highest byte any layout the driver mints
// consumes (`types::driver_mmio_mints` + `types::mmio_consumed_end` — the
// exact set `check_mmio_claims` proves pairwise disjoint), rounded up to 8
// and never smaller than one word. A driver that mints nothing still gets
// a word, so its `DeviceCap[D]` still names a real, mapped address rather
// than a zero.

/// One declared device's own register window, as sized and placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRegs {
    /// Index into `ImageGraph::devices` — the `device#N` the report and
    /// every edge already spell.
    pub device: usize,
    pub device_type: String,
    /// The `@driver` whose `device=` binding names it (one per device:
    /// `eval::image_checks::check_device_bound_once` refuses a second
    /// binding of the same device, so this is not a list).
    pub driver: String,
    pub base: u64,
    pub size: u64,
}

/// Every declared device that some `@driver` binds, with the window width
/// its driver's own declared `Mmio[L]` fields ask for — in `graph.devices`
/// order, which is `image.report.deterministic`'s own construction order.
fn device_register_windows(
    boot: Option<&BootCtx>,
) -> Result<Vec<(usize, String, String, u64)>, LayoutError> {
    use crate::eval::image::ImageDeclRef;
    use crate::eval::value::Value;

    let Some(b) = boot else {
        return Ok(Vec::new());
    };
    let layouts = closure_layout_types(b.modules)?;
    let decls = closure_decl_items(b.modules)?;

    let mut out: Vec<(usize, String, String, u64)> = Vec::new();
    for d in &b.graph.drivers {
        let driver = crate::sema::types::render_type(&d.actor_type);
        // `device=` is optional on `img.driver(...)` — 03-hardware.md §1
        // names it "the single source of truth" for *which* device a
        // driver's authority is over, and a driver that claims none (no
        // capability parameter, no `Mmio[L]` field) is a legal, if
        // currently pointless, declaration: `golden/err-image-driver-message`
        // is exactly one. Such a driver gets no register window, and the
        // one thing that would have needed it — a `DeviceCap[D]`
        // parameter — is refused by name in `one_boot_init_call`.
        let Some(Value::ImageDecl(ImageDeclRef::Device(idx))) = d
            .args
            .iter()
            .find(|a| a.label == "device")
            .map(|a| &a.value)
        else {
            continue;
        };
        let device_type = match b.graph.devices.get(*idx) {
            Some(dev) => crate::sema::types::render_type(&dev.device_type),
            None => {
                return Err(LayoutError::new(format!(
                    "internal error: `@driver` `{driver}` binds device#{idx}, which this image \
                     does not declare"
                )));
            }
        };
        let mut extent = 0u64;
        if let Some(mints) = crate::sema::types::driver_mmio_mints(&decls, &driver) {
            for name in mints {
                if let Some(l) = layouts.get(&name) {
                    extent = extent.max(crate::sema::types::mmio_consumed_end(l));
                }
            }
        }
        let size = round_up(extent.max(8), 8);
        out.push((*idx, device_type, driver, size));
    }
    out.sort_by_key(|(idx, _, _, _)| *idx);
    Ok(out)
}

/// Places every device register window sequentially from `cursor`, each
/// 8-byte aligned. Returns the placements, the section's own base/size and
/// the advanced cursor — `None` when this image binds no driver at all, in
/// which case no `devregs` section exists.
fn place_device_regs(
    cursor: u64,
    windows: &[(usize, String, String, u64)],
) -> Option<(Vec<DeviceRegs>, u64, u64, u64)> {
    if windows.is_empty() {
        return None;
    }
    let base = round_up(cursor, 8);
    let mut at = base;
    let mut out = Vec::with_capacity(windows.len());
    for (device, device_type, driver, size) in windows {
        out.push(DeviceRegs {
            device: *device,
            device_type: device_type.clone(),
            driver: driver.clone(),
            base: at,
            size: *size,
        });
        at += size;
    }
    Some((out, base, at - base, at))
}

/// `verify_pool_windows`' sibling, for the same reason and by the same
/// method: every placed register window is non-empty, lies wholly inside
/// the `devregs` section, and overlaps no other. The section table is
/// already proved disjoint by `verify_section_sizes`, so this is what
/// extends that proof to each individual window — an address a driver
/// reaches through an `Mmio[L]` is inside `devregs`, therefore outside
/// this image's instructions, its runtime tables, every actor's state and
/// mailbox, and every DMA pool.
fn verify_device_windows(sections: &[Section], regs: &[DeviceRegs]) -> Result<(), LayoutError> {
    if regs.is_empty() {
        return Ok(());
    }
    let Some(sec) = sections.iter().find(|s| s.name == "devregs") else {
        return Err(LayoutError::new(
            "internal error: this image places device register windows but reserves no `devregs` \
             section",
        ));
    };
    let sec_end = sec.base + sec.size;
    for (i, r) in regs.iter().enumerate() {
        if r.size == 0 {
            return Err(LayoutError::new(format!(
                "internal error: device#{} was placed with a zero-byte register window",
                r.device
            )));
        }
        let end = r.base + r.size;
        if r.base < sec.base || end > sec_end {
            return Err(LayoutError::new(format!(
                "internal error: device#{}'s register window [{:#x}, {end:#x}) is not inside the \
                 `devregs` section [{:#x}, {sec_end:#x})",
                r.device, r.base, sec.base
            )));
        }
        for other in &regs[..i] {
            let other_end = other.base + other.size;
            if r.base < other_end && other.base < end {
                return Err(LayoutError::new(format!(
                    "internal error: device#{} and device#{} were placed at overlapping register \
                     windows",
                    other.device, r.device
                )));
            }
        }
    }
    Ok(())
}

/// Every declared struct/enum in a build closure, as `DeclItem`s — the
/// same specialize-then-declare pass `actor_inits` runs, kept separate
/// because its consumers ask a different question of the result.
fn closure_decl_items(
    modules: &BTreeMap<String, Module>,
) -> Result<Vec<crate::sema::types::DeclItem>, LayoutError> {
    let imported = closure_imported_types(modules)
        .map_err(|e| LayoutError::new(format!("device register windows: {}", e.message)))?;
    let mut out = Vec::new();
    for (addr, module) in modules {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("device register windows: {}", e.message)))?;
        out.extend(
            crate::sema::types::declare_with_imports(&specialized, &imported[addr])
                .map_err(|e| LayoutError::new(format!("device register windows: {}", e.message)))?,
        );
    }
    Ok(out)
}

/// Every `@layout` type in a build closure, by name — `layout_program`'s
/// own input to `eval::image_checks::pool_backings`. Built from the raw
/// `ast::Module` closure the same way `bin/wrela.rs` builds the report's
/// exact-bytes section, rather than threaded through `BootCtx` from the
/// typed programs: `types::check_layouts` is a pure function of one
/// specialized module, so the checker's table (`TypedProgram::layouts`)
/// and this one are the same table computed twice, never two rules.
fn closure_layout_types(
    modules: &BTreeMap<String, Module>,
) -> Result<BTreeMap<String, crate::sema::types::LayoutType>, LayoutError> {
    let mut out = BTreeMap::new();
    for module in modules.values() {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("pool backing: {}", e.message)))?;
        for l in crate::sema::types::check_layouts(&specialized)
            .map_err(|e| LayoutError::new(format!("pool backing: {}", e.message)))?
        {
            out.insert(l.name.clone(), l);
        }
    }
    Ok(out)
}

/// This image's own pool backing, resolved from the sealed graph.
/// `eval::image_checks::check_sealed` already ran `pool_backings` and
/// already rejected every bad declaration by name, so an `Err` here means
/// the two disagreed — a producer bug, reported as one rather than
/// silently placing a window nobody checked.
fn image_pool_backings(
    boot: Option<&BootCtx>,
) -> Result<BTreeMap<String, crate::eval::image_checks::PoolBacking>, LayoutError> {
    let Some(b) = boot else {
        return Ok(BTreeMap::new());
    };
    let layouts = closure_layout_types(b.modules)?;
    crate::eval::image_checks::pool_backings(b.graph, &layouts).map_err(|e| {
        LayoutError::new(format!(
            "internal error: a pool declaration this image's own graph check accepted cannot be \
             placed: {}",
            e.message
        ))
    })
}

// --- top-level entry: CodegenProgram -> ImageLayout -----------------------

/// Places `program` into the machine's fixed layout as one flat blob
/// (module doc's own section order), resolving every `Reloc`. `Err` only
/// for a genuine internal inconsistency (a call target codegen itself
/// never produced, an out-of-range relocation) — never for an ordinary
/// "this program doesn't lower" outcome, which is decided one layer up,
/// before this fn is ever called (see `try_layout_program`, below).
///
/// `boot` (plans/M6.md item C, then the item-F/G follow-up that made this
/// path work at all): `Some(ctx)` for a build that declares actors reserves
/// two more sections — `rtcode`, this image's own runtime routines
/// (`build_runtime_block`: every actor's `__rt_enqueue_*`/
/// `rt_select_and_run`, each `g.start` site's poll routine, `rt_run_one`,
/// and boot-init), and `rtdata`, sized exactly `tables.total_bytes` of
/// zeroed, uninitialized bytes — the same "no allocation, all sized at
/// build time" discipline every other section here already follows.
/// `None` (no `@actor` in the build closure, or a caller with nothing to
/// derive from) keeps this fn byte-identical to its pre-M6 behavior.
///
/// The `rtcode` half is not optional decoration: `codegen` lowers every
/// `await`/`send` through an `Actor[T]` handle to a `Reloc::Call` at the
/// symbolic `codegen::rt_enqueue_symbol` name, so an image that *messages*
/// an actor cannot resolve its own relocations without it. Before that was
/// wired here, `wrela build`/`--stage=report` on any such image died with
/// `internal error: call target `__rt_enqueue_X` was never codegen'd`.
///
/// Both sections are present for **any** actor-bearing image, tests or not
/// (decision 3's own rule), even though `build_entry_stub` above still
/// halts with `EXIT_CODE_NO_RUNTIME` and therefore never calls into them:
/// they are there because they are part of the image, not because anything
/// in a `wrela build` image executes them yet. The entry driver is the one
/// thing that legitimately differs between this fn and `layout_test_image`
/// — see the `RuntimeWiring`/`build_runtime_block` module block.
pub fn layout_program(
    program: &CodegenProgram,
    boot: Option<BootCtx>,
) -> Result<ImageLayout, LayoutError> {
    let image_base = machine_layout::IMAGE_BASE;

    let mut wiring: Option<RuntimeWiring> = match &boot {
        Some(b) => RuntimeWiring::derive(b, program)?,
        None => None,
    };

    let entry_words = build_entry_stub();

    let mut code_words: Vec<u32> = Vec::new();
    let mut fn_word_base: BTreeMap<String, usize> = BTreeMap::new();
    for (key, f) in &program.fns {
        fn_word_base.insert(key.clone(), code_words.len());
        for (w, _text) in &f.code {
            code_words.push(*w);
        }
    }

    // plans/M7.md item E1: fallible-`init` abort messages are interned
    // into the same rodata pool an `assert` failure's text already uses,
    // once, before either `build_runtime_block` pass.
    let mut rodata_entries: Vec<Vec<u8>> = program.rodata.clone();
    let mut rodata_cursor: usize = rodata_entries.iter().map(Vec::len).sum();
    if let Some(w) = wiring.as_mut() {
        intern_fallible_init_abort_messages(w, &mut rodata_entries, &mut rodata_cursor);
    }
    let rodata_bytes: Vec<u8> = rodata_entries
        .iter()
        .flat_map(|entry| entry.iter().copied())
        .collect();
    let runtime: Option<&RuntimeTables> = wiring.as_ref().map(|w| &w.tables);

    let abort_fixed_words = build_abort_stub(EXIT_CODE_ABORT_FIXED);
    let abort_val_words = build_abort_stub(EXIT_CODE_ABORT_VAL);
    // plans/M6.md item F: the checkpoint block's own vector-0 body is the
    // real deadline scan whenever this build has a group arena, so it needs
    // already-placed `rtdata` addresses — which are not known until after
    // this very block's own size fixes `cursor`. Built twice, exactly like
    // the runtime glue block below: once with a shape-only placeholder
    // context purely to learn the word count (never address-dependent), then
    // again with the real addresses once `rtdata_base` exists.
    let checkpoint_shape = group_service_shape(runtime);
    let (irq_shape, wake_shape) = checkpoint_irq_shape(boot.as_ref(), None, runtime);
    let checkpoint_block =
        build_checkpoint_and_vector_stub_ex(checkpoint_shape.as_ref(), &irq_shape, &wake_shape);
    let checkpoint_words = checkpoint_block.words;
    let checkpoint_service_word = checkpoint_block.checkpoint_service_word;
    let checkpoint_relocs_shape = checkpoint_block.relocs;

    // This image's own runtime routines (`build_runtime_block`), built
    // twice for the identical reason the checkpoint block above is: their
    // word count never depends on `rtdata`'s address, but their bytes do,
    // and `rtdata`'s base is not known until this very block's own size
    // has moved `cursor`. Word indices are relative to the `rtcode`
    // section's own base (this fn's own placement, unlike
    // `layout_test_image`'s, gives the block a section of its own rather
    // than a slice of the combined harness section).
    // The sizing pass gets **address-free but structurally real**
    // placements: the same device windows and pool backings this image
    // will place, at base 0. Word counts do not depend on address values
    // (`build_runtime_glue_block`'s own doc), but `BootInitArg::resolve`'s
    // "no placed window" guard does depend on the *entries* existing — and
    // that guard is a real one, so it is fed a real list rather than
    // switched off for one of the two passes.
    let sizing_device_regs = place_device_regs(0, &device_register_windows(boot.as_ref())?)
        .map(|(regs, _, _, _)| regs)
        .unwrap_or_default();
    let sizing_pools = place_pools_unchecked(0, &image_pool_backings(boot.as_ref())?)
        .map(|(pools, _, _, _)| pools)
        .unwrap_or_default();
    let dummy_runtime_block = wiring
        .as_ref()
        .map(|w| {
            build_runtime_block(
                w,
                &place_runtime_tables(0, &w.tables),
                &sizing_device_regs,
                &sizing_pools,
                0,
                None, // AbortFixed reloc — abort lives in another section
            )
        })
        .transpose()?;
    let rtcode_words_len = dummy_runtime_block
        .as_ref()
        .map(|b| b.words.len())
        .unwrap_or(0);

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

    let rtcode_base = if rtcode_words_len > 0 {
        let base = cursor;
        cursor += (rtcode_words_len * 4) as u64;
        Some(base)
    } else {
        None
    };

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
    // plans/M6.md item F/G follow-up: `rtcode` — this image's own runtime
    // routines, the exact block `layout_test_image` places inside its
    // combined harness section. Absent entirely for an image with no
    // actors, never a zero-size placeholder section.
    if let Some(base) = rtcode_base {
        sections.push(Section {
            name: "rtcode",
            base,
            size: (rtcode_words_len * 4) as u64,
        });
    }

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
        // plans/M7.md item D: `rtdata` used to be this image shape's own
        // final section, so nothing advanced `cursor` past it. `pooldata`
        // now follows it, so it does.
        cursor += tables.total_bytes;
        Some(base)
    } else {
        None
    };

    // --- pooldata (plans/M7.md item D): 05-library.md §9's "reserve
    // exact backing", zeroed image bytes exactly like `rtdata`'s. This
    // image shape's own final section — nothing consumes `cursor` past
    // it. Absent entirely for an image that binds no pool.
    // --- devregs (plans/M7.md item H1): one declared register window per
    // device this image binds to a `@driver` — decision 11's own address
    // for a `DeviceCap[D]`, and the base every `Mmio[L]` partition
    // addresses from. Placed *before* `pooldata`, in the identical order
    // `layout_test_image` places it: plans/M6.md item F/G's rule is that
    // the two image flavors emit the same memory map for the same source,
    // and the boot-init words that carry these bases are built from the
    // same `build_runtime_block` in both. Absent entirely for an image
    // that binds no driver.
    let device_windows = device_register_windows(boot.as_ref())?;
    let placed_regs = place_device_regs(cursor, &device_windows);
    let device_regs: Vec<DeviceRegs> = match &placed_regs {
        Some((regs, base, size, end)) => {
            sections.push(Section {
                name: "devregs",
                base: *base,
                size: *size,
            });
            cursor = *end;
            regs.clone()
        }
        None => Vec::new(),
    };

    let pool_backings = image_pool_backings(boot.as_ref())?;
    let placed_pools = place_pools(cursor, &sections, &pool_backings)?;
    let pools: Vec<PoolPlacement> = match &placed_pools {
        Some((pools, base, size, end)) => {
            sections.push(Section {
                name: "pooldata",
                base: *base,
                size: *size,
            });
            cursor = *end;
            pools.clone()
        }
        None => Vec::new(),
    };
    let _ = cursor;

    // --- resolve every Reloc against the now-known section bases --------
    let runtime_live = runtime.filter(|t| t.total_bytes > 0);
    let placement = match (rtdata_base, runtime_live) {
        (Some(base), Some(tables)) => Some(place_runtime_tables(base, tables)),
        _ => None,
    };
    // Second pass over the checkpoint block, now that `rtdata` is placed.
    // The two word-count guards here and just below are unreachable from
    // any source program (internal-error audit): both blocks are built from
    // the identical shape inputs in both passes and differ only in the
    // *values* a fixed four-word `load_imm` materializes, so a source file
    // has no way to change one pass's length without changing the other's.
    // They are kept as real `Err`s rather than `debug_assert`s because a
    // length disagreement would silently corrupt every later section base.
    let (mut checkpoint_words, checkpoint_relocs) = match (&placement, runtime_live) {
        (Some(pl), Some(tables)) => {
            let (irq_real, wake_real) = checkpoint_irq_shape(boot.as_ref(), Some(pl), Some(tables));
            let real = build_checkpoint_and_vector_stub_ex(
                group_service_ctx(pl, tables).as_ref(),
                &irq_real,
                &wake_real,
            );
            if real.words.len() != checkpoint_words.len() {
                return Err(LayoutError::new(
                    "internal error: the checkpoint block's own word count changed between its \
                     sizing pass and its real-address pass",
                ));
            }
            (real.words, real.relocs)
        }
        _ => (checkpoint_words, checkpoint_relocs_shape),
    };
    // plans/M7.md item G: ISR / `@task` `BL`s inside the checkpoint section.
    for reloc in &checkpoint_relocs {
        match reloc {
            Reloc::Call { word, key } => {
                let target_base = *fn_word_base.get(key).ok_or_else(|| {
                    LayoutError::new(format!(
                        "internal error: checkpoint dispatch target `{key}` was never codegen'd"
                    ))
                })?;
                let this_addr = checkpoint_base + (*word as u64) * 4;
                let target_addr = code_base + (target_base as u64) * 4;
                patch_bl(&mut checkpoint_words, *word, this_addr, target_addr)?;
            }
            other => {
                return Err(LayoutError::new(format!(
                    "internal error: checkpoint block emitted unexpected reloc {other:?}"
                )));
            }
        }
    }
    // Second pass over the runtime block, now that `rtdata` is placed —
    // the identical shape the checkpoint block above uses.
    let runtime_block = match (&wiring, &placement) {
        (Some(w), Some(pl)) => {
            let real = build_runtime_block(w, pl, &device_regs, &pools, 0, None)?;
            if real.words.len() != rtcode_words_len {
                return Err(LayoutError::new(
                    "internal error: the runtime block's own word count changed between its \
                     sizing pass and its real-address pass",
                ));
            }
            Some(real)
        }
        _ => None,
    };
    let empty_symbols = BTreeMap::new();
    let glue_symbols: &BTreeMap<String, usize> = runtime_block
        .as_ref()
        .map(|b| &b.symbols)
        .unwrap_or(&empty_symbols);
    let mut all_code_words = code_words;
    // Internal-error audit (the item-F/G follow-up's own second half), for
    // the three non-`Call` guards below — each is unreachable from any
    // source program, and each says so here rather than being demoted:
    //
    // - `Reloc::Rodata` with an empty `rodata` section: codegen only emits
    //   that reloc by interning a literal into `RodataPool`, which is the
    //   very thing that makes `program.rodata` non-empty.
    // - `Reloc::TurnFrameAddr`/`Reloc::GroupArenaBase` with no runtime
    //   tables: both are emitted only from compiled *async* code, and
    //   `compute_runtime_tables` returns `None` only when the build has
    //   neither a declared actor nor a single async fn (`async_frames`
    //   empty). One async fn is enough to size a table set whose
    //   `total_bytes` is already non-zero (a ready queue and an RR cursor
    //   at minimum), so the two conditions are mutually exclusive.
    //   `turn_area_for` likewise partitions every `async_frames` key into
    //   exactly one of "owned by a declared actor" or "free turn", from
    //   the same map codegen keyed its relocs by.
    //
    // The `Reloc::Call` arm is the one that was *not* unreachable — see
    // `unresolved_call_target`.
    for (key, f) in &program.fns {
        let base = fn_word_base[key];
        for reloc in &f.relocs {
            match reloc {
                Reloc::Call { word, key: target } => {
                    // A compiled `Send`/`Await{ActorCall}` op's own symbolic
                    // call target is a per-actor runtime-glue routine
                    // (`glue_symbols`, `rtcode`-section-relative) rather than
                    // an ordinary `program.fns` entry (`fn_word_base`,
                    // `code`-section-relative) — the identical two-scheme
                    // lookup `layout_test_image` already does, and the whole
                    // reason a messaged-actor image lays out at all.
                    // plans/M8.md item C2: a cross-core edge resolves to its
                    // own `__rt_xsend_*` ring producer instead of the
                    // same-core admission routine — same ABI, same
                    // rejection contract, one symbol swapped.
                    let redirect = resolve_cross_core_edge(key, target, wiring.as_ref())?;
                    let target = redirect.as_deref().unwrap_or(target.as_str());
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = if let Some(target_base) = fn_word_base.get(target) {
                        code_base + (*target_base as u64) * 4
                    } else if let (Some(rc), Some(glue_word)) =
                        (rtcode_base, glue_symbols.get(target))
                    {
                        rc + (*glue_word as u64) * 4
                    } else {
                        return Err(unresolved_call_target(
                            target,
                            boot.as_ref().map(|b| b.graph),
                        ));
                    };
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
                Reloc::TurnIdImm { word, key: fn_key } => {
                    // plans/M10.md item 0c1: the same owner-resolution rule
                    // as `TurnFrameAddr` above, stopping one step earlier —
                    // at the index rather than the address it scales to.
                    let id = placement
                        .as_ref()
                        .zip(runtime_live)
                        .and_then(|(p, t)| p.turn_id_for(fn_key, t))
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: async fn `{fn_key}` needs a turn id but this \
                                 image's runtime tables never sized one"
                            ))
                        })?;
                    patch_load_imm_words(&mut all_code_words, base + word, id.get() as u64);
                }
                Reloc::TurnsBase { word } => {
                    // plans/M10.md item 0c3: `RT.turns`' own base, which is
                    // `rtdata_base` exactly (item 0b put the array first).
                    let addr = placement
                        .as_ref()
                        .map(|p| p.turns_base)
                        .ok_or_else(|| LayoutError::new(turns_deref_needs_rtdata()))?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::TurnStride { word } => {
                    let stride = placement
                        .as_ref()
                        .map(|p| p.turn_stride)
                        .ok_or_else(|| LayoutError::new(turns_deref_needs_rtdata()))?;
                    patch_load_imm_words(&mut all_code_words, base + word, stride);
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
                Reloc::IrqVector { word, driver } => {
                    let vector = driver_irq_vector(boot.as_ref().map(|b| b.graph), driver)?;
                    patch_load_imm_words(&mut all_code_words, base + word, vector);
                }
                Reloc::WakePending { word, driver } => {
                    let (p, t) = match (placement.as_ref(), runtime_live) {
                        (Some(p), Some(t)) => (p, t),
                        _ => {
                            return Err(wake_needs_rtdata(driver));
                        }
                    };
                    let addr = driver_wake_pending_addr(p, t, driver)?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                // M10 D / decision 614
                Reloc::MailboxAddr { word, actor, field } => {
                    let (p, t) = match (placement.as_ref(), runtime_live) {
                        (Some(p), Some(t)) => (p, t),
                        _ => {
                            return Err(LayoutError::new(
                                "internal error: a Reloc::MailboxAddr exists but this image has \
                                 no runtime tables",
                            ));
                        }
                    };
                    let addrs = resolve_mailbox_ring_addrs(p, t, actor).ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: Reloc::MailboxAddr names actor `{actor}`, which this \
                             image's runtime tables never placed a mailbox for"
                        ))
                    })?;
                    let addr = match field {
                        crate::codegen::MailboxField::Ring => addrs.ring,
                        crate::codegen::MailboxField::Tail => addrs.tail,
                        crate::codegen::MailboxField::Count => addrs.count,
                    };
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
            }
        }
    }

    // The `rtcode` section's own relocations: `Reloc::Call` (dispatch +
    // boot-init `init` calls), and — plans/M7.md item E1 — `Reloc::Rodata`
    // + `Reloc::AbortFixed` on a fallible `init`'s `Err` path (the same
    // `__wrela_abort` contract an `assert` failure inside `init` already
    // uses from the `code` section). Any other kind appearing here would
    // be a real internal inconsistency, so it is rejected rather than
    // guessed at.
    //
    // Internal-error audit: the "never codegen'd" Call guard's own targets
    // are a declared actor's `pub` method keys and its `init` key, all read
    // out of the same module set `lower`/`codegen` compiled — and a method
    // that fails to lower stops the whole attempt one layer up, at
    // `try_layout_program`'s "all or nothing" rule, long before here. It is
    // the *undeclared*-actor direction that was reachable, and that is the
    // `Reloc::Call` case handled by `unresolved_call_target` above. The
    // AbortVal/CheckpointService/TurnFrameAddr/GroupArenaBase rejection is
    // structural — `build_boot_init` emits none of those.
    let mut rtcode_words: Vec<u32> = runtime_block
        .as_ref()
        .map(|b| b.words.clone())
        .unwrap_or_default();
    if let (Some(block), Some(rc)) = (&runtime_block, rtcode_base) {
        for reloc in &block.relocs {
            match reloc {
                Reloc::Call { word, key } => {
                    let target_base = *fn_word_base.get(key).ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: runtime-glue call target `{key}` was never codegen'd"
                        ))
                    })?;
                    let this_addr = rc + (*word as u64) * 4;
                    let target_addr = code_base + (target_base as u64) * 4;
                    patch_bl(&mut rtcode_words, *word, this_addr, target_addr)?;
                }
                Reloc::Rodata {
                    word_adrp,
                    byte_offset,
                } => {
                    let rb = rodata_base.ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a runtime-block Reloc::Rodata exists but the rodata \
                             section is empty",
                        )
                    })?;
                    let this_addr = rc + (*word_adrp as u64) * 4;
                    let target_addr = rb + *byte_offset as u64;
                    patch_adrp_add(&mut rtcode_words, *word_adrp, this_addr, target_addr)?;
                }
                Reloc::AbortFixed { word } => {
                    let this_addr = rc + (*word as u64) * 4;
                    patch_bl(&mut rtcode_words, *word, this_addr, abort_fixed_base)?;
                }
                Reloc::AbortVal { .. }
                | Reloc::CheckpointService { .. }
                | Reloc::TurnFrameAddr { .. }
                | Reloc::TurnIdImm { .. }
                | Reloc::TurnsBase { .. }
                | Reloc::TurnStride { .. }
                | Reloc::GroupArenaBase { .. }
                | Reloc::IrqVector { .. }
                | Reloc::WakePending { .. }
                | Reloc::MailboxAddr { .. } => {
                    return Err(LayoutError::new(
                        "internal error: the runtime block itself must never emit an \
                         AbortVal/CheckpointService/TurnFrameAddr/TurnIdImm/TurnsBase/TurnStride/\
                         GroupArenaBase/IrqVector/WakePending/MailboxAddr reloc",
                    ));
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
    if let Some(rc) = rtcode_base {
        pad_to(&mut blob, image_base, rc);
        for w in &rtcode_words {
            blob.extend_from_slice(&w.to_le_bytes());
        }
    }
    if let (Some(rb), Some(tables)) = (rtdata_base, runtime.filter(|t| t.total_bytes > 0)) {
        pad_to(&mut blob, image_base, rb);
        blob.resize(blob.len() + tables.total_bytes as usize, 0);
    }
    // plans/M7.md item H1: `devregs` is emitted before `pooldata`, the
    // same order it is placed in — 06-machine.md §3's "zeroes the declared
    // reservations" applies to a register window exactly as it does to a
    // pool's backing.
    if let Some((_, base, size, _)) = &placed_regs {
        pad_to(&mut blob, image_base, *base);
        blob.resize(blob.len() + *size as usize, 0);
    }
    if let Some((_, base, size, _)) = &placed_pools {
        pad_to(&mut blob, image_base, *base);
        blob.resize(blob.len() + *size as usize, 0);
    }

    verify_section_sizes(&sections, image_base, blob.len() as u64)?;
    verify_pool_windows(&sections, &pools)?;
    verify_device_windows(&sections, &device_regs)?;
    // blk filled later — ring verify runs in attach_blk_report

    let irq_host_injects = build_irq_host_injects(boot.as_ref(), &device_regs);
    // plans/M8.md item C1: each secondary core's entry, `rtcode`-relative
    // word index resolved against that section's own placed base.
    let core_entries: Vec<(usize, u64)> = match (&runtime_block, rtcode_base) {
        (Some(b), Some(rc)) => b
            .core_entry_starts
            .iter()
            .map(|&(core, word)| (core, rc + (word as u64) * 4))
            .collect(),
        _ => Vec::new(),
    };
    Ok(ImageLayout {
        blob,
        entry: entry_base,
        sections,
        runtime: runtime.cloned(),
        pools,
        device_regs,
        blk: None, // filled by attach_blk_report after layout
        irq_host_injects,
        core_entries,
        placed_statics: Vec::new(), // filled by try_layout_program from TypedPrograms
    })
}

// --- whole-program orchestration (lower -> codegen -> layout) ------------

/// plans/M9.md item A1: the build closure's imported-type arity table,
/// per module, keyed the dotted-address way this file's closures are.
/// Every re-derivation of `sema::types::declare` below needs it for the
/// same reason `sema::check_program_typed` does — a signature naming an
/// imported `struct`/`enum` no longer fails to resolve, so re-running
/// `declare` without the table would reintroduce exactly the
/// `unknown type` this item removed, one layer down and as a
/// `LayoutError` instead of a diagnostic.
///
/// Built over **specialized** modules, exactly as `sema::check_program_typed`
/// builds it (decision 11): a `struct`/`enum` declared inside a module-level
/// `comptime if` only exists once `specialize` has run, so a raw-AST table
/// here would list fewer type names than sema's did and this file would
/// reject a program the checker accepted. `specialize` is pure, and every
/// loop below already re-runs it per module — this file's own established
/// "recompute rather than thread extra state" convention.
fn closure_imported_types(
    modules: &BTreeMap<String, Module>,
) -> Result<BTreeMap<String, crate::sema::types::ImportedTypes>, SemaError> {
    let mut specialized: BTreeMap<String, Module> = BTreeMap::new();
    for (addr, m) in modules {
        specialized.insert(addr.clone(), crate::sema::specialize::specialize(m)?);
    }
    let by_addr: Vec<(Vec<String>, &Module)> = specialized
        .iter()
        .map(|(addr, m)| (addr.split('.').map(str::to_string).collect(), m))
        .collect();
    let shapes = crate::sema::imports::closure_type_shapes(&by_addr);
    Ok(specialized
        .iter()
        .map(|(addr, m)| {
            let mut imported = crate::sema::imports::imported_type_shapes(m, &shapes);
            // plans/M9.md item PP: same Duration/Instant inject
            // `build_layout_ctx` / `check_typed` perform. Without it,
            // `merge_actor_pub_methods` (and any other declare-with-
            // closure-imports path) fails on a prelude-only `: Duration`
            // after check already accepted.
            if crate::loader::module_mentions_time(m) {
                for name in ["Duration", "Instant"] {
                    imported.entry(name.to_string()).or_insert(0);
                }
            }
            (addr.clone(), imported)
        })
        .collect())
}

/// Merges one `mwir::LayoutCtx` per module in the build closure (project
/// cases place a spliced-in struct's own field-type declaration in a
/// *different* file than the one holding `@image` — `mwir::build_layout_ctx`
/// itself only ever sees one raw `ast::Module` at a time, module-local, so
/// a single module's own ctx is not enough whenever any struct/enum lives
/// outside the `@image`-owning file). Later modules win on an exact-name
/// collision (undisclosed generalization beyond what any of today's
/// goldens exercise — every real case here has module-unique struct/enum
/// names).
///
/// plans/M9.md item FF: after the own-decl merge, each aliased import is
/// installed under the *local* spelling (decision 9). Own decls above are
/// keyed by the exporter's AST name; the typed tree / MWIR / codegen look
/// up the author's spelling. One install here — never a
/// `get(local).or_else(|| get(exporter))` at a use site.
pub fn merge_layout_ctx(modules: &BTreeMap<String, Module>) -> Result<LayoutCtx, SemaError> {
    let imported = closure_imported_types(modules)?;
    let mut merged = LayoutCtx::default();
    for (addr, module) in modules {
        let ctx = crate::mwir::build_layout_ctx(module, &imported[addr])?;
        merged.structs.extend(ctx.structs);
        merged.enums.extend(ctx.enums);
        merged.struct_field_names.extend(ctx.struct_field_names);
    }
    install_aliased_import_layouts(&mut merged, modules)?;
    Ok(merged)
}

/// plans/M9.md item FF / decision 100: for every `from M import T as A`
/// (and only when `A != T`), copy the exporter's layout entry to key `A`
/// and re-key any self-`Type::Named` inside it. Unaliased imports need
/// nothing — the exporter module's own build already contributed under
/// `T`, which is the local spelling too. Rejected: a lookup-time
/// fallback that tries both spellings (exactly what let this bug
/// reappear one layer down, three times).
fn install_aliased_import_layouts(
    ctx: &mut LayoutCtx,
    modules: &BTreeMap<String, Module>,
) -> Result<(), SemaError> {
    let mut specialized: BTreeMap<String, Module> = BTreeMap::new();
    for (addr, m) in modules {
        specialized.insert(addr.clone(), crate::sema::specialize::specialize(m)?);
    }
    let by_addr: Vec<(Vec<String>, &Module)> = specialized
        .iter()
        .map(|(addr, m)| (addr.split('.').map(str::to_string).collect(), m))
        .collect();
    let shapes = crate::sema::imports::closure_type_shapes(&by_addr);
    for module in specialized.values() {
        let targets = crate::sema::imports::imported_type_targets(module, &shapes);
        // Group non-identity aliases by exporting module so each layout
        // copy gets the whole-signature substitution (plans/M9.md item GG),
        // not only the owning type (FF).
        let mut subs_by_exporter: BTreeMap<Vec<String>, BTreeMap<String, String>> = BTreeMap::new();
        for (local, (target_mod, target_name)) in &targets {
            if local != target_name {
                subs_by_exporter
                    .entry(target_mod.clone())
                    .or_default()
                    .insert(target_name.clone(), local.clone());
            }
        }
        for (local, (target_mod, target_name)) in &targets {
            if local == target_name {
                continue;
            }
            let subs = &subs_by_exporter[target_mod];
            if let Some(mut fields) = ctx.structs.get(target_name).cloned() {
                for f in &mut fields {
                    crate::sema::types::rekey_type_names(f, subs);
                }
                ctx.structs.insert(local.clone(), fields);
                if let Some(names) = ctx.struct_field_names.get(target_name).cloned() {
                    ctx.struct_field_names.insert(local.clone(), names);
                }
            }
            if let Some(mut payloads) = ctx.enums.get(target_name).cloned() {
                for payload in &mut payloads {
                    for t in payload {
                        crate::sema::types::rekey_type_names(t, subs);
                    }
                }
                ctx.enums.insert(local.clone(), payloads);
            }
        }
    }
    Ok(())
}

/// plans/M7.md item G, decision 18: fold every checked struct
/// instantiation into `LayoutCtx` under its rendered type spelling
/// (`BlkDriver[DriverMode.Irq]`), so `mwir::size_of` can size a mode-
/// specialized driver's state the same way it sizes a plain one.
/// plans/M9.md item II: also fold `imported.instantiations` — those keys
/// are already under the importer's alias spelling after the typed
/// splice, so a body that constructs `Box[Item]` (peer aliased) sizes
/// under `Box[Item]`, not only the exporter's `Box[Src]`.
pub fn enrich_layout_ctx_with_instantiations(
    ctx: &mut LayoutCtx,
    programs: &BTreeMap<String, TypedProgram>,
) {
    use crate::sema::typed::TypedInstantiation;
    for typed in programs.values() {
        let own = typed.instantiations.iter();
        let imported = typed.imported.instantiations.iter();
        for (key, inst) in own.chain(imported) {
            let TypedInstantiation::Struct(s) = inst else {
                continue;
            };
            let display = key.strip_prefix("struct:").unwrap_or(key.as_str());
            let fields: Vec<crate::sema::types::Type> = s
                .fields
                .iter()
                .filter_map(|n| s.field_types.get(n).cloned())
                .collect();
            ctx.struct_field_names
                .insert(display.to_string(), s.fields.clone());
            ctx.structs.insert(display.to_string(), fields);
        }
    }
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
/// Concatenates per-module MWIR programs into one (plans/M9.md item H3 /
/// M10.md item A2d). Rodata indices are rebased per module; `fns` keys
/// collide last-module-wins.
pub fn merge_mwir_programs(programs: Vec<mwir::MwirProgram>) -> mwir::MwirProgram {
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
    // plans/M7.md item E1: capacity is an image-declared build constant.
    // Stamp it onto every TypedProgram before lower so
    // `read_capacity_sectors` can emit it as a ConstInt.
    let capacity = crate::eval::image_checks::blk_capacity_sectors(graph);
    // Decision 18: instantiations must be sizeable before codegen.
    let mut layout_ctx = layout_ctx.clone();
    enrich_layout_ctx_with_instantiations(&mut layout_ctx, programs);
    let layout_ctx = &layout_ctx;
    // plans/M9.md item H3: one reachable set over the whole build
    // closure, so a library module with no actors does not emit its
    // entire surface into the merged image.
    let reachable =
        crate::lower::guest_reachable_keys_closure(programs, &crate::lower::LowerOpts::default());
    let lower_opts = crate::lower::LowerOpts {
        emit_comptime_tests: false,
        only: Some(reachable),
    };
    let mut mwir_programs = Vec::with_capacity(programs.len());
    for typed in programs.values() {
        let mut stamped = typed.clone();
        stamped.blk_capacity_sectors = capacity;
        match crate::lower::lower_program_with(&stamped, &lower_opts) {
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
        let mut stamped = typed.clone();
        stamped.blk_capacity_sectors = capacity;
        match crate::flowwir_lower::lower_program_with(&stamped, &lower_opts) {
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
    let enqueue_specs = match mailbox_enqueue_specs(graph, modules, layout_ctx) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let codegen_program = match crate::codegen::codegen_program_with_async(
        &merged,
        &flow,
        layout_ctx,
        &method_index,
        group_arena_capacity,
        &enqueue_specs,
    ) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    // plans/M6.md item F/G follow-up: the same `BootCtx` the test-image
    // path builds — `layout_program` derives its runtime tables *and* its
    // runtime routines from it, through the one shared
    // `RuntimeWiring::derive`/`build_runtime_block` pair, so the two image
    // flavors can never again disagree about what runtime machinery an
    // actor-bearing image contains.
    // (`codegen_program_with_async` above already called this same fn and
    // propagated its two disclosed floors, so this call can only ever
    // succeed here — the `Err` arm keeps this fn's own "all or nothing"
    // rule rather than introducing a second, differently-shaped outcome.)
    let group_child_index = match crate::codegen::compute_group_child_indices(&flow) {
        Ok(m) => m,
        Err(_) => return Ok(None),
    };
    layout_program(
        &codegen_program,
        Some(BootCtx {
            graph,
            modules,
            layout_ctx,
            async_frames: &async_frames,
            group_child_index: &group_child_index,
        }),
    )
    .and_then(|mut layout| {
        // plans/M7.md item E1: BlkDevice/BlkQueue from configure + pools.
        attach_blk_report(&mut layout, graph, programs)?;
        // plans/M10.md item A2c / decision 588: publish every placed static.
        layout.placed_statics = collect_placed_statics(programs)?;
        Ok(Some(layout))
    })
    .or_else(|e| Err(e.message))
}

/// Every `@placed` static across the build closure, name-sorted, with the
/// layout type's completed size (plans/M10.md item A2c).
fn collect_placed_statics(
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<Vec<PlacedStatic>, LayoutError> {
    let mut out = Vec::new();
    for prog in programs.values() {
        for (name, s) in &prog.statics {
            let crate::sema::types::Type::Named(ty_name, _) = &s.ty else {
                return Err(LayoutError::new(format!(
                    "internal error: placed static `{name}` has non-named type"
                )));
            };
            let size = prog
                .layouts
                .iter()
                .find(|l| l.name == *ty_name)
                .and_then(|l| l.size)
                .ok_or_else(|| {
                    LayoutError::new(format!(
                        "internal error: placed static `{name}` type `{ty_name}` has no completed size"
                    ))
                })?;
            out.push(PlacedStatic {
                name: name.clone(),
                ty: ty_name.clone(),
                addr: s.addr,
                size,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
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
    /// This actor's own **turn area**: the fixed turn record
    /// (`codegen::TURN_RECORD_SIZE` — busy/suspended/resume_ready/reply/
    /// waker/cur_method/reply_slot/reply_tag) plus the widest persistent frame any of its own
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
/// One `@driver` declaration's own static sizing (plans/M7.md item H1).
/// Deliberately *not* an `ActorRuntimeLayout`: a driver is an actor root
/// (02 §9.1) and, since plans/M8.md item D, may own a mailbox — but only
/// when its declaration says `mailbox=` (05-library.md §9). The state
/// bytes are unconditional; the mailbox half is `Option`, so a driver
/// with no `mailbox=` still has exactly one static fact and no zeroed
/// ring/slot/frame numbers pretending to be decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverRuntimeLayout {
    pub name: String,
    /// This driver struct's own field storage (`mwir::size_of`) — where
    /// the instance lives, exactly like an actor's `state_size`. When the
    /// driver declares a `@task`, this also includes one trailing word for
    /// the sticky wake-pending bit (plans/M7.md item G).
    pub state_size: u64,
    /// Byte offset of the wake-pending word within `state_size`, when the
    /// driver has a `@task`. Layout patches `Reloc::WakePending` against
    /// `driver_state_base + wake_pending_off`.
    pub wake_pending_off: Option<u64>,
    /// plans/M8.md item D (decision 19): present exactly when this
    /// declaration carried `mailbox=n`. The three numbers are the same
    /// three an `ActorRuntimeLayout` carries and are computed by the same
    /// arithmetic — a messageable driver is admitted, selected and
    /// dispatched by the identical routines an `@actor` is
    /// (`build_rt_enqueue` / `build_rt_select_and_run_symbolic`), never a
    /// second path.
    pub mailbox: Option<DriverMailbox>,
}

/// The mailbox half of a messageable `@driver` (plans/M8.md item D).
/// Field-for-field the mailbox half of `ActorRuntimeLayout`; kept as its
/// own struct so "has a mailbox" is one `Option`, not three sentinel
/// zeros.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverMailbox {
    pub capacity: u64,
    pub slot_size: u64,
    pub frame_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeTables {
    pub actors: Vec<ActorRuntimeLayout>,
    /// plans/M7.md item H1: one entry per declared `@driver` instance, in
    /// `ImageGraph::drivers` order. Until this item a `@driver`'s state
    /// was never reserved and its `init` never called, so no capability of
    /// any kind had bytes at runtime — decision 10's second prerequisite.
    pub drivers: Vec<DriverRuntimeLayout>,
    /// One turn area per **free** async fn (every `FlowWirProgram::fns`
    /// key that is not a declared actor's own method — `@test(runtime)`
    /// roots foremost; a compiled-but-unreachable free `async fn` still
    /// gets one so its own `Reloc::TurnFrameAddr` resolves): `(fn key,
    /// area bytes)` where area = `codegen::TURN_RECORD_SIZE` + that fn's
    /// own persistent frame. The root test turn parks/resumes through
    /// its own area via the identical machinery an actor turn uses —
    /// decision 3's "+1 root" ready-queue slot, now made concrete.
    pub free_turns: Vec<(String, u64)>,
    /// plans/M10.md item 0a (decision 552): how many turn areas this image
    /// has — every actor, every **messageable** driver, every free async
    /// fn. The three sets above, counted once, so `total_bytes` and
    /// `place_runtime_tables` cannot disagree about how many strides they
    /// are accounting for.
    pub n_turns: u64,
    /// plans/M10.md item 0a (decision 552): the **uniform** byte stride
    /// every turn area is reserved at — `round_up_pow2` of the widest raw
    /// area over all three owner kinds, or `0` when `n_turns == 0`.
    ///
    /// The per-owner `frame_size` fields (`ActorRuntimeLayout::frame_size`,
    /// `DriverMailbox::frame_size`, `free_turns.1`) stay the **raw** area —
    /// `TURN_RECORD_SIZE + widest owned async frame` — deliberately. Two
    /// things depend on that: the report's own `frame=` lines say how much
    /// of the stride is real (a reader can subtract and see the padding),
    /// and `build_rt_select_and_run_core`'s lineage-zeroing guard is keyed
    /// on "does this owner actually have frame slots past its record",
    /// which the stride cannot answer — under a uniform stride it would
    /// answer "yes" for a bare actor and emit two stores into padding.
    /// Only the *reservation* (this field) is uniform.
    pub turn_stride: u64,
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
    /// plans/M8.md item C2: this image's own cross-core SPSC rings, in
    /// `cross_core_rings`'s canonical order. Empty for every single-core
    /// image and for a cross-core image whose graph has no cross-core
    /// message edge (decision 28's own "emit nothing" rule).
    pub rings: Vec<RingLayout>,
    /// How many cores this image brings up (`placement::PlacementTable::
    /// cores` — `1` for every single-core image, `VCPUS` for a cross-core
    /// graph). plans/M8.md item C1: the scheduler's own per-core state is
    /// **striped by this count** — one ready-queue table and one
    /// round-robin cursor per live core, never one global set shared
    /// across cores (04 §2: one event loop *per core*, no migration). A
    /// single-core image stripes by 1, which is byte-for-byte the
    /// pre-C1 reservation.
    pub cores: usize,
    /// The exact `rtdata` section size: every actor's own state + ring +
    /// bookkeeping + frame bytes, plus the per-core ready-queue tables,
    /// the per-core round-robin cursor words, and the group arena.
    pub total_bytes: u64,
}

impl RuntimeTables {
    /// Restripes the scheduler tables for `cores` live cores, recomputing
    /// `total_bytes`. Called once, by `RuntimeWiring::derive`, as soon as
    /// placement is known — `compute_runtime_tables` itself cannot call
    /// `placement::place` (placement calls *it* for the per-actor sizes it
    /// packs on, so the dependency runs one way only).
    pub fn stripe_for_cores(&mut self, cores: usize) {
        debug_assert!(cores >= 1);
        let old = self.cores as u64;
        let new = cores as u64;
        let per_core = self.ready_queue_capacity * 8 + RR_CURSOR_SIZE;
        self.total_bytes = self.total_bytes - old * per_core + new * per_core;
        self.cores = cores;
    }

    /// plans/M8.md item C2: installs this image's own cross-core rings and
    /// grows `total_bytes` by exactly their reservation. Called once, by
    /// `RuntimeWiring::derive`, right after `stripe_for_cores` — the rings
    /// are placed **last** in `rtdata` (after the group arena), so nothing
    /// an existing golden pins moves for an image that has none.
    pub fn add_cross_core_rings(&mut self, rings: Vec<RingLayout>) {
        for r in &rings {
            self.total_bytes += r.bytes();
        }
        self.rings = rings;
    }
}

/// plans/M8.md item C2 (04-compiler.md §3: "cross-core actor edges ...
/// lowered to compiler-generated bounded SPSC rings in guest memory").
/// Which of the two lanes a ring is — decision 29 keeps them separate so a
/// request lane's back-pressure can never sit in front of a reply that has
/// nowhere else to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingKind {
    /// `src` -> `dst`: an admitted message, waiting to be handed to
    /// `dst`'s own `__rt_enqueue_<actor>` by `dst`'s drain.
    Request,
    /// `src` -> `dst`: a completed turn's reply word plus the address of
    /// the turn record on `dst` it belongs to.
    Reply,
}

/// One cross-core SPSC ring's own static shape. The producer is core
/// `src` and the consumer is core `dst` — **one producer because one
/// core**, not because one actor (decision 28): two actors on core 0 that
/// both message the same actor on core 1 share one ring, and the baton
/// (decision 11) plus the per-core cooperative loop mean neither can be
/// mid-enqueue when the other starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingLayout {
    pub src: usize,
    pub dst: usize,
    pub kind: RingKind,
    /// The target actor struct, for a `Request` ring: the ring feeds
    /// exactly one mailbox, which is what makes its slot format identical
    /// to that mailbox's own and its capacity derivable from the sealed
    /// graph. `None` for a `Reply` ring (a reply is addressed to a turn
    /// record, not to an actor).
    pub actor: Option<String>,
    pub capacity: u64,
    pub slot_size: u64,
}

impl RingLayout {
    /// `capacity * slot_size` plus the same three-word head/tail/count
    /// bookkeeping a mailbox carries (`MAILBOX_BOOKKEEPING_SIZE`).
    pub fn bytes(&self) -> u64 {
        self.capacity * self.slot_size + MAILBOX_BOOKKEEPING_SIZE
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            RingKind::Request => "request",
            RingKind::Reply => "reply",
        }
    }
}

/// A bounded ring's four placed addresses — the only thing
/// `build_ring_enqueue` and the drain routines address. A mailbox is one
/// of these (`ActorAddrs::mailbox`) and so is every cross-core ring, which
/// is exactly why the cross-core producer *is* `build_rt_enqueue`'s own
/// body rather than a second implementation of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingAddrs {
    pub ring: u64,
    pub head: u64,
    pub tail: u64,
    pub count: u64,
}

/// A reply slot: the destination turn's own `TurnId` in the low half of
/// word 0, the reply `tag` in the high half (plans/M10.md item J,
/// decision 665 — the upper half was unused padding after 0c1), then the
/// reply word at +8. `REPLY_SLOT_SIZE` stays 16: the tag rides the spare
/// half rather than growing the ring.
const REPLY_SLOT_SIZE: u64 = 16;

/// plans/M8.md item C2, decision 30 / plans/M10.md item 0c1, decision 557:
/// the **originating core + 1**, so a completing turn can tell a local
/// waker (0 — every same-core send and every single-core image) from one
/// whose turn record lives on another core.
///
/// This used to be `(src_core + 1) << 61`, OR'd into the waker's own
/// 64-bit turn-area address and masked back off with a `load_imm`+`bic`
/// pair at every read. Item 0c1 splits that one word into two adjacent
/// `u32` fields — `waker_turn` at `OFF_TURN_WAKER` and `waker_core` at
/// `OFF_TURN_WAKER + 4` — so the core travels in its own field, in its own
/// register (`x4` at the `rt_enqueue` ABI), and the untagging disappears
/// entirely. `+1` rather than the bare core index is still what makes "no
/// tag" and "from core 0" distinct, and it is the same `Option`-niche
/// convention `TurnId` itself uses.
fn waker_core_tag(src_core: usize) -> u16 {
    (src_core as u16) + 1
}

/// The one index→address rule, emitted (plans/M10.md item 0c1): `id_reg`
/// holds an `Option[TurnId]` already known nonzero, and comes back holding
/// `turns_base + ((id - 1) << log2_stride)` — `RuntimePlacement::turn_addr`
/// made of instructions. `scratch` is clobbered.
///
/// Two words of arithmetic past the base's own `load_imm`, not ROADMAP's
/// "single shifted-register add": `encode::enc_add_reg` is shift-0 only and
/// buying an `enc_add_reg_lsl` here would be an unmeasured optimization
/// (CLAUDE.md's cleverness budget applies to the compiler too).
fn push_turn_addr_from_id(a: &mut Asm, id_reg: u8, scratch: u8, turns_base: u64, log2_stride: u8) {
    a.load_imm(scratch, turns_base);
    a.push(encode::enc_sub_imm(id_reg, id_reg, 1, true));
    a.push(encode::enc_lsl_imm(id_reg, id_reg, log2_stride, true));
    a.push(encode::enc_add_reg(id_reg, scratch, id_reg, true));
}

/// plans/M8.md item C2: the reply ring was full. Unreachable by
/// construction rather than by hope — a reply ring `d -> s` is sized to
/// the number of turn areas on core `s` (`reply_ring_capacity`), each of
/// which can have at most one outstanding `await` (non-reentrancy caps
/// in-flight activations at one per turn area), so there can never be more
/// undelivered replies bound for `s` than the ring holds. Same class and
/// same treatment as `BRK_REPLY_SLOT_NO_WAKER` above.
const BRK_XREPLY_RING_FULL: u16 = 0xACD7;

/// plans/M8.md item C2: a waker carried a core tag naming a core this
/// image never brought up. Unreachable: the tag is written by
/// `build_rt_xsend`, one build-time constant per emitted routine.
const BRK_XREPLY_UNKNOWN_CORE: u16 = 0xACD8;

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
    /// plans/M7.md item Z1 (decision 9a): whether this method's own
    /// *declared* reply is an aggregate (`codegen::is_aggregate`, the one
    /// shared ABI predicate — never a copy of it, exactly as
    /// `sema::types::validate_message_shape` already calls it). Such a
    /// method is handed its caller's staging-slot address in `x8` by the
    /// dispatch arm below and writes its reply straight into the awaiting
    /// frame; a scalar-reply method's arm is untouched, down to the word.
    reply_is_aggregate: bool,
    param_sizes: Vec<u64>,
    /// plans/M8.md item D: the message shape itself — every parameter's
    /// declared type plus the declared reply — so the messageable-driver
    /// check (`check_driver_message_surface`) can ask
    /// `sema::types::driver_message_forbidden_carried` about the exact
    /// types the author wrote. Sizes cannot answer an authority question:
    /// a `Receipt[P]`, an `InterruptCell[u32]` and a `u64` are all one
    /// word.
    param_types: Vec<crate::sema::types::Type>,
    ret: crate::sema::types::Type,
    /// 03-hardware.md §6's bottom half. A `@task` is woken by an ISR, not
    /// admitted from a mailbox; a `pub` `@task` on a messageable driver
    /// would give one turn body two entry paths (decision 21).
    is_task: bool,
    /// 03-hardware.md §5's handoff calling convention, recognized by the
    /// one predicate `sema::handoff::is_handoff_signature` (never a copy
    /// of it): a public *synchronous* `@driver` method with exactly one
    /// `take p: P` parameter and result `Receipt[P]`. plans/M8.md item E,
    /// decision 33: it is the one shape whose reply may carry a
    /// `Receipt[P]` across a driver mailbox, because §5 blesses precisely
    /// this signature and nothing else can produce the pair the caller
    /// then `await`s.
    is_handoff: bool,
}

fn merge_actor_pub_methods(
    modules: &BTreeMap<String, Module>,
    layout_ctx: &LayoutCtx,
) -> Result<BTreeMap<String, Vec<ActorMethodShape>>, LayoutError> {
    use crate::sema::types::{DeclItem, DeclMember};

    let imported = closure_imported_types(modules)
        .map_err(|e| LayoutError::new(format!("actor runtime layout: {}", e.message)))?;
    let mut out: BTreeMap<String, Vec<ActorMethodShape>> = BTreeMap::new();
    for (addr, module) in modules {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("actor runtime layout: {}", e.message)))?;
        let items = crate::sema::types::declare_with_imports(&specialized, &imported[addr])
            .map_err(|e| LayoutError::new(format!("actor runtime layout: {}", e.message)))?;
        for item in items {
            let DeclItem::Struct(s) = item else { continue };
            // plans/M9.md item MM: only `@actor`/`@driver` structs own
            // message shapes. A generic stdlib type (`List[T, N]`,
            // `SlotMap[T, N]`) is not an actor — sizing its template
            // methods hits bare type parameters and must not run here.
            if !s.is_actor {
                continue;
            }
            if !s.generics.is_empty() {
                continue;
            }
            let mut methods = Vec::new();
            for m in &s.members {
                let DeclMember::Fn(f) = m else { continue };
                let Some(recv) = &f.receiver else { continue };
                if !recv.is_pub {
                    continue;
                }
                let mut param_sizes = Vec::with_capacity(f.params.len());
                let mut param_types = Vec::with_capacity(f.params.len());
                for p in &f.params {
                    let size = mwir::size_of(&p.ty, layout_ctx).map_err(|e| {
                        LayoutError::new(format!(
                            "actor `{}`'s own `{}` message shape: {e}",
                            s.name, f.name
                        ))
                    })?;
                    param_sizes.push(size as u64);
                    param_types.push(p.ty.clone());
                }
                methods.push(ActorMethodShape {
                    name: f.name.clone(),
                    is_async: f.is_async,
                    reply_is_aggregate: crate::codegen::is_aggregate(&f.ret),
                    param_sizes,
                    param_types,
                    ret: f.ret.clone(),
                    is_task: f.is_task,
                    is_handoff: s.is_driver && crate::sema::handoff::is_handoff_signature(f),
                });
            }
            out.insert(s.name, methods);
        }
    }
    Ok(out)
}

/// plans/M7.md item W: every struct's own declared `init`, in the shape
/// boot needs to *call* it — the `program.fns` key, the declared
/// parameter list (name, access mode, declared type, declaration order)
/// and the declared return type. `build_boot_init_calls` (below) turns
/// one of these plus one `ActorDecl`'s own wiring arguments into the
/// argument words `build_boot_init` loads into `x1..`.
///
/// What this replaced, recorded because it was a *rejection* and is not
/// one any more: until item W this returned only which structs declared
/// a **zero-argument** `init` — the one shape a boot sequence with no
/// argument marshalling could call — plus the parameter count of every
/// other one, purely so `RuntimeWiring::derive` could refuse to lay the
/// image out at all. That guard existed because the two halves of the
/// rule had silently composed into a wrong answer: `eval::image_checks`
/// accepts `depth=7` (it really does name a real `Sink.init` parameter),
/// boot never called that `init`, and the actor booted with `depth == 0`
/// while every assertion over it read 0 and all three tiers reported
/// success. Boot now calls a declared `init` with its declared
/// arguments, so the guard is gone; what remains fails closed on the
/// *specific shape it cannot marshal*, named one at a time in
/// `build_boot_init_calls`, never on "declares parameters at all".
struct ActorInit {
    /// `"{Struct}.init"` — `lower::lower_struct`'s own key for the
    /// compiled body, which is what `Asm::bl_call_key` resolves against.
    key: String,
    params: Vec<crate::sema::types::DeclParam>,
    ret: crate::sema::types::Type,
}

fn actor_inits(
    modules: &BTreeMap<String, Module>,
) -> Result<BTreeMap<String, ActorInit>, LayoutError> {
    use crate::sema::types::{DeclItem, DeclMember};

    let imported = closure_imported_types(modules)
        .map_err(|e| LayoutError::new(format!("actor boot init: {}", e.message)))?;
    let mut out: BTreeMap<String, ActorInit> = BTreeMap::new();
    for (addr, module) in modules {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("actor boot init: {}", e.message)))?;
        let items = crate::sema::types::declare_with_imports(&specialized, &imported[addr])
            .map_err(|e| LayoutError::new(format!("actor boot init: {}", e.message)))?;
        for item in items {
            let DeclItem::Struct(s) = item else { continue };
            for m in &s.members {
                if let DeclMember::Init(f) = m {
                    out.insert(
                        s.name.clone(),
                        ActorInit {
                            key: format!("{}.init", s.name),
                            params: f.params.clone(),
                            ret: f.ret.clone(),
                        },
                    );
                }
            }
        }
    }
    Ok(out)
}

/// One declared actor instance's own boot-time `init` call: the compiled
/// body's key and its already-materialized argument words, in declared
/// parameter order. `build_boot_init` loads word `i` into `x{i+1}`.
///
/// **The ABI is not restated here, it is derived**: `codegen::emit_prologue`
/// spills the receiver from `x0` into the frame's `self_ptr` slot (a
/// pointer — the receiver is always by address), then walks `f.params` in
/// declaration order spilling each one from the next register up, by
/// value for a non-aggregate and by address for an aggregate
/// (`codegen::is_aggregate`), and refuses past `x8`. `codegen`'s own
/// `Inst::Call` emitter is the mirror image of that, and
/// `build_rt_select_and_run_core`'s hand-assembled dispatch already
/// relies on the `x0`-is-`self`-pointer half (`a.load_imm(0, addrs.state)`
/// before every method call). Boot is a third caller of the identical
/// convention: `x0` = the actor's own state address, `x1..` = the
/// scalar arguments. Aggregates are not passed at all — they would need
/// a staging buffer boot has nowhere to put — so `boot_init_arg_word`
/// fails closed on every one of them instead.
#[derive(Debug)]
struct BootInitCall {
    key: String,
    args: Vec<BootInitArg>,
    /// plans/M7.md item E1: `true` when `init` returns
    /// `Result[unit, BootError]` — boot must arm `x8` with a reply slot
    /// and abort on `Err`.
    fallible: bool,
    /// When `fallible`: `(rodata_byte_offset, len)` of the abort message
    /// `"{key} returned Err"`, interned once before either assembly pass
    /// so the sizing and real-address builds agree on every word. `None`
    /// until `intern_fallible_init_abort_messages` runs (and forever for
    /// an infallible `init`).
    err_msg: Option<(usize, usize)>,
}

/// One materialized `init` argument word — or the promise of one whose
/// value is not known until the section table is placed.
///
/// plans/M7.md item H1: a `DeviceCap[D]` argument is decision 11's own
/// representation, the base address of the device's declared register
/// window, and that address exists only after `place_device_regs` has run.
/// Every other argument is already a build-time constant by the time
/// `build_boot_init_calls` sees it. Carrying the one unresolved case as
/// its own variant (rather than threading placement through the whole
/// derivation, or patching a word after the fact) keeps the emitted word
/// count identical in both of `build_runtime_block`'s passes — a
/// `load_imm` is four words whatever it loads — which is the invariant
/// both image flavors' two-pass assembly rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BootInitArg {
    Word(u64),
    /// The base of `ImageGraph::devices[.0]`'s own register window.
    DeviceRegsBase(usize),
    /// The base of the named pool's own backing in `pooldata` — decision
    /// 11's word for a `DmaPool[P, N]`, which item D placed and reported
    /// but nothing could yet pass to an `init`.
    PoolBase(String),
    /// plans/M7.md item E4 / decision 19: one `own[P] T` — the guest
    /// address of slot `index` in pool `name` (`base + index * slot_bytes`).
    OwnSlot {
        pool: String,
        index: u64,
        slot_bytes: u64,
    },
    /// plans/M7.md item E4: `[own[P] T; N]` — boot builds a table of
    /// `count` slot addresses on its own stack and passes the table's
    /// address (this machine's bare-pointer aggregate ABI).
    OwnHandleArray {
        pool: String,
        count: u64,
        slot_bytes: u64,
    },
}

impl BootInitArg {
    /// The word this argument actually loads. `regs`/`pools` are the
    /// placed window lists; a reference with no matching placement is an
    /// internal inconsistency (both parameters only exist on a driver
    /// whose binding `eval::image_checks` already resolved), reported
    /// rather than silently zeroed.
    fn resolve(&self, regs: &[DeviceRegs], pools: &[PoolPlacement]) -> Result<u64, LayoutError> {
        match self {
            BootInitArg::Word(w) => Ok(*w),
            BootInitArg::DeviceRegsBase(i) => regs
                .iter()
                .find(|r| r.device == *i)
                .map(|r| r.base)
                .ok_or_else(|| {
                    LayoutError::new(format!(
                        "internal error: boot passes a `DeviceCap` for device#{i}, which has no \
                         placed register window"
                    ))
                }),
            BootInitArg::PoolBase(name) => pools
                .iter()
                .find(|p| &p.backing.name == name)
                .map(|p| p.base)
                .ok_or_else(|| {
                    LayoutError::new(format!(
                        "internal error: boot passes a `DmaPool` for pool `{name}`, which has no \
                         placed backing"
                    ))
                }),
            BootInitArg::OwnSlot {
                pool,
                index,
                slot_bytes,
            } => {
                let p = pools
                    .iter()
                    .find(|p| &p.backing.name == pool)
                    .ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: boot passes an `own` into pool `{pool}`, which has no \
                             placed backing"
                        ))
                    })?;
                Ok(p.base + *index * *slot_bytes)
            }
            BootInitArg::OwnHandleArray { .. } => Err(LayoutError::new(
                "internal error: `OwnHandleArray` has no single resolve word — emit via \
                 `emit_boot_init_arg`"
                    .to_string(),
            )),
        }
    }
}

/// The one place a build-time `eval::value::Value` becomes the 64-bit
/// word boot loads into an argument register. Deliberately exhaustive and
/// deliberately narrow: `None` means "this compiler has no register
/// representation for this value", and the caller turns that into a named
/// diagnostic rather than into a zero.
///
/// The encodings are `codegen`'s own, not new ones — an integer is
/// `Inst::ConstInt`'s `load_imm(value as i64)` (a negative value is
/// therefore its sign-extended two's complement, exactly as a compiled
/// `-5` would be), a bool is `Inst::ConstBool`'s 0/1, a char is
/// `Inst::ConstChar`'s code point, and `unit` is all-zero (the same fact
/// `build_boot_init`'s own zero-fill already rests on).
///
/// Counts that define the shared image-declaration handle space
/// (plans/M8.md item H attack 6). Derived once from the sealed graph so
/// every consumer (`boot_init_arg_word`, `resolve_runtime_test_args`)
/// sees the same shift.
#[derive(Clone, Copy, Debug, Default)]
struct HandleSpace {
    n_actors: usize,
    n_drivers: usize,
}

impl HandleSpace {
    fn from_graph(graph: &ImageGraph) -> Self {
        Self {
            n_actors: graph.actors.len(),
            n_drivers: graph.drivers.len(),
        }
    }
}

/// **Contract: no two distinct image declarations share a handle word,
/// whatever their kind.** Every `ImageDeclRef` variant is named here so a
/// fourth kind cannot quietly reuse a number — it either gets a fresh
/// range or fails closed like a pool.
///
/// Layout (dumb, deterministic, actors-then-drivers-then-devices):
/// - `Actor(i)`  → `i`
/// - `Driver(i)` → `n_actors + i`
/// - `Device(i)` → `n_actors + n_drivers + i`
/// - `Pool` / `DmaPool` → no word (`None`); they are named by string, not
///   indexed (`ImageDeclRef`'s own two recording disciplines).
///
/// Why one space for all three indexed kinds: `decl.handle()` erases the
/// declaration's type into a bare `u32` (`check_image_decl_method_intrinsic`
/// accepts it on any `ImageDecl`), so a kind-local scheme for devices is
/// the same shape of hole item D decision 22 left for actors/drivers —
/// found by orchestrator spot-probe after the first attack-6 fix covered
/// only two of the three kinds.
fn image_decl_handle_word(
    space: HandleSpace,
    decl: &crate::eval::image::ImageDeclRef,
) -> Option<u64> {
    use crate::eval::image::ImageDeclRef;
    match decl {
        ImageDeclRef::Actor(i) => Some(*i as u64),
        ImageDeclRef::Driver(i) => Some((space.n_actors + *i) as u64),
        ImageDeclRef::Device(i) => Some((space.n_actors + space.n_drivers + *i) as u64),
        ImageDeclRef::Pool(_) | ImageDeclRef::DmaPool(_) => None,
    }
}

/// A declaration handle (`Value::ImageDecl`) becomes its word in the
/// shared space (`image_decl_handle_word`) — the identical number
/// `resolve_runtime_test_args` hands a `@test(runtime)` root for an
/// `Actor[T]` parameter. **What that number is and is not**: `codegen`
/// still routes every `await`/`send` statically by actor type today, but
/// the guest can store and compare the word (and the day handles become
/// dynamic this is the one place that has to change). A pool reference
/// is named by a string, not an index, so it has no word and fails closed.
fn boot_init_arg_word(value: &crate::eval::value::Value, space: HandleSpace) -> Option<u64> {
    use crate::eval::value::Value;

    Some(match value {
        Value::U8(n) => *n as u64,
        Value::U16(n) => *n as u64,
        Value::U32(n) => *n as u64,
        Value::U64(n) | Value::Usize(n) => *n,
        Value::I8(n) => *n as i64 as u64,
        Value::I16(n) => *n as i64 as u64,
        Value::I32(n) => *n as i64 as u64,
        Value::I64(n) | Value::Isize(n) => *n as u64,
        Value::Bool(b) => u64::from(*b),
        Value::Char(c) => *c as u32 as u64,
        Value::Unit => 0,
        Value::ImageDecl(decl) => return image_decl_handle_word(space, decl),
        // Every remaining shape is either an aggregate (no register
        // representation: `Struct`/`Tuple`/`Array`/`Enum`/`Str`/`Bytes`),
        // a float (`codegen` has no FP/SIMD encoder subset at all —
        // `Inst::ConstFloat` fails closed for the identical reason), or a
        // callable (`Fn`/`Closure` — not a value this machine passes).
        Value::F32(_)
        | Value::F64(_)
        | Value::Str(_)
        | Value::Bytes(_)
        | Value::Tuple(_)
        | Value::Array(_)
        | Value::Struct(_)
        | Value::Enum(_, _)
        | Value::Fn(_)
        | Value::Closure { .. } => return None,
    })
}

/// A `Value`'s own shape, for a diagnostic that has to name what it
/// could not marshal without printing the whole value.
fn value_shape_name(value: &crate::eval::value::Value) -> &'static str {
    use crate::eval::image::ImageDeclRef;
    use crate::eval::value::Value;

    match value {
        Value::U8(_)
        | Value::U16(_)
        | Value::U32(_)
        | Value::U64(_)
        | Value::Usize(_)
        | Value::I8(_)
        | Value::I16(_)
        | Value::I32(_)
        | Value::I64(_)
        | Value::Isize(_) => "an integer",
        Value::Bool(_) => "a bool",
        Value::Char(_) => "a char",
        Value::Unit => "unit",
        Value::F32(_) | Value::F64(_) => "a floating-point value",
        Value::Str(_) => "a string",
        Value::Bytes(_) => "a byte string",
        Value::Tuple(_) => "a tuple",
        Value::Array(_) => "an array",
        Value::Struct(_) => "a struct value",
        Value::Enum(_, _) => "an enum value",
        Value::Fn(_) => "a function reference",
        Value::Closure { .. } => "a closure",
        Value::ImageDecl(ImageDeclRef::Device(_)) => "a device handle",
        Value::ImageDecl(ImageDeclRef::Driver(_)) => "a driver handle",
        Value::ImageDecl(ImageDeclRef::Actor(_)) => "an actor handle",
        Value::ImageDecl(ImageDeclRef::Pool(_)) => "a pool handle",
        Value::ImageDecl(ImageDeclRef::DmaPool(_)) => "a DMA-pool handle",
    }
}

/// **plans/M7.md item W's residual, closed here** (item W named item D as
/// its owner; this is that).
///
/// The residual, in item W's own words: "A handle wired through an `init`
/// *parameter* now carries its real construction index; a handle wired
/// straight to a *field* (05-library.md §9's literal-constructor path,
/// which has no `init` to call) still arrives as the zero the state-fill
/// leaves. The two paths genuinely disagree. ... the fix is either
/// materializing into the field's own offset with W's marshalling or
/// failing closed on a nonzero index."
///
/// **Failing closed is the option taken**, for three reasons stated once
/// so the choice is reviewable rather than inferred:
///
/// 1. It is *exact*, not conservative. A field-wired argument whose boot
///    word is `0` agrees with the state-fill byte for byte — there is no
///    disagreement to close, which is why `golden/image-field-wired-accept`
///    (`led=<actor#0>`) stays green untouched. Every word that is not `0`
///    is a wrong answer today, and every one of them is now rejected. The
///    two paths therefore never disagree again, which is the whole
///    property the residual asked for.
/// 2. Materializing would build a mechanism with no consumer.
///    `codegen` routes every `await`/`send` statically, by actor *type*
///    (`codegen::rt_enqueue_symbol`), so nothing reads a handle word at
///    runtime; storing one into a field offset would be an unobservable
///    write, and the house rule is that a feature waits for the thing that
///    needs it. When an `Actor[T]` becomes comparable, storable or
///    sendable, this rejection is what fails and points at the work.
/// 3. It generalizes correctly rather than only to handles. Item W found
///    the defect through a handle, but a *scalar* wired to a field of a
///    no-`init` struct is silently zero for exactly the same reason
///    (`img.actor(Store, seed=8)` against `Store.seed: u32`, no `init` —
///    accepted by `eval::image_checks`' literal-constructor arm, never
///    materialized by anything). "Nonzero index" and "nonzero value" are
///    one rule: *a field-wired argument must equal the zero the state-fill
///    leaves*. A value with no register representation at all is rejected
///    too, since this compiler cannot show it is zero either.
/// Image-wiring labels that are never field/init arguments — the same
/// sets `eval::image_checks::reserved_args` uses, restated here by `kind`
/// because that helper is not `pub` and `is_reserved_actor_arg` only
/// covers the actor half. Sharing the actor predicate for drivers would
/// let `device=` fall through as a field wire (and, once device handles
/// left word 0, fail closed against zero-fill — the exact break
/// `check-driver-mode-irq`/`-poll` hit under attack 6's device half).
fn is_reserved_wiring_arg(kind: &str, label: &str) -> bool {
    match kind {
        "driver" => matches!(label, "device" | "core" | "mailbox"),
        "actor" => crate::eval::image_checks::is_reserved_actor_arg(label),
        _ => false,
    }
}

fn check_field_wired_args(
    kind: &str,
    name: &str,
    decl_args: &[crate::eval::image::DeclArg],
    space: HandleSpace,
) -> Result<(), LayoutError> {
    for a in decl_args {
        if is_reserved_wiring_arg(kind, &a.label) {
            continue;
        }
        let word = boot_init_arg_word(&a.value, space);
        if word == Some(0) {
            continue;
        }
        let what = match word {
            Some(w) => format!("materializes as {w}"),
            None => format!(
                "is {} and has no register representation at all",
                value_shape_name(&a.value)
            ),
        };
        return Err(LayoutError::new(format!(
            "{kind} `{name}` declares no `init`, so this image wires `{}=...` to its field of \
             that name — and boot has nothing to call: it zero-fills the whole state slot and \
             stops (05-library.md §9's literal-constructor path). The wired value {what}, which \
             is not the zero the state-fill leaves, so the {kind} would boot with a value this \
             image did not declare. Failing closed (plans/M7.md item W's residual, owned by item \
             D) rather than reporting success over a wrong answer. Give `{name}` an `init` that \
             takes it, or drop the argument.",
            a.label
        )));
    }
    Ok(())
}

/// plans/M7.md item W: every declared actor *and driver* instance's own
/// boot `init` call, in `graph.actors`/`graph.drivers` order — which is
/// `RuntimeTables::actors`/`RuntimeTables::drivers` order too
/// (`compute_runtime_tables` builds one entry per graph entry, in the same
/// walks), so each result indexes 1:1 against the matching
/// `RuntimePlacement` list.
///
/// **plans/M7.md item H1, decision 10's second prerequisite**: until this
/// item the walk was `graph.actors` only, so a `@driver`'s `init` was
/// never called at boot at all and no capability of any kind had bytes at
/// runtime. A driver's `init` is now called exactly like an actor's, with
/// one difference that is the whole point: its `DeviceCap[D]` parameter
/// carries no explicit image argument (05-library.md §9 substitutes it)
/// and is materialized as decision 11's own word — the base of the device
/// register window `place_device_regs` reserved for the device its
/// `device=` binding names.
///
/// `None` for an actor whose struct declares no `init` at all: its state
/// is its own literal constructor's, and `build_boot_init`'s zero-fill
/// already gave every field a defined value (`eval::image_checks`'s own
/// missing-slot note rests on exactly that).
///
/// Everything this cannot do fails closed with an ordinary named
/// diagnostic — never `internal error:` (that spelling is reserved for a
/// producer bug), and never a zero. The shapes, all of them:
///
/// - a **fallible `init`** (`-> Result[...]`): running one means driving
///   03-hardware.md §9's consuming transition chain from boot, which is
///   plans/M7.md item H's bring-up work. Until then a fallible `init`
///   fails closed rather than having its `Result` dropped on the floor.
///   Note this arm also closes a live defect that predates item W: a
///   *zero-argument* `init` returning a `Result` was called by the old
///   boot sequence with no `x8`, so the callee wrote its aggregate reply
///   through whatever `x8` happened to hold — a real guest fault at
///   `ipa=0x0`, verified by running it.
/// - any other **non-`unit` return**, for the same reason with no item to
///   name: boot has nowhere to put the value.
/// - a **capability parameter other than `DeviceCap[D]` on a `@driver`**
///   (recognized by name exactly as `eval::image_checks` recognizes them):
///   `eval::image_checks::check_capability_substitution` already refuses
///   each of these at the image binding, naming the item that mints it;
///   this is the same refusal for the shape that check cannot see (an
///   `img.actor(...)` naming a struct that is not an `@actor`).
/// - an **`Actor[T]` parameter with no explicit argument**: 05-library.md
///   §9 lets an actor handle be substituted by type rather than wired by
///   name, and boot materializes only what the image explicitly wired.
/// - a **pool handle argument** (05-library.md §9's "create the initial
///   handles", wired as `blocks=take cache_blocks`): plans/M7.md item E4
///   / decision 19 materializes each `own[P] T` as the guest address of
///   one pool slot. A single `own` is that word; an `[own; N]` is a
///   stack-built table of them passed by the bare-pointer aggregate ABI.
///   Any other parameter type wired from a pool still fails closed.
/// - an **argument whose value has no register representation**
///   (an aggregate, a float, ...).
/// - **more than eight arguments**, the register budget `x1..x8` leaves
///   once `x0` carries the receiver — `codegen`'s own identical limit.
fn build_boot_init_calls(
    graph: &ImageGraph,
    inits: &BTreeMap<String, ActorInit>,
    backings: &BTreeMap<String, crate::eval::image_checks::PoolBacking>,
) -> Result<(Vec<Option<BootInitCall>>, Vec<Option<BootInitCall>>), LayoutError> {
    let mut actors = Vec::with_capacity(graph.actors.len());
    for decl in &graph.actors {
        actors.push(one_boot_init_call(
            "actor",
            &decl.actor_type,
            &decl.args,
            None,
            graph,
            inits,
            backings,
        )?);
    }
    let mut drivers = Vec::with_capacity(graph.drivers.len());
    for decl in &graph.drivers {
        let device = device_index_of(&decl.args);
        drivers.push(one_boot_init_call(
            "driver",
            &decl.actor_type,
            &decl.args,
            device,
            graph,
            inits,
            backings,
        )?);
    }
    Ok((actors, drivers))
}

/// The `device#N` an `img.driver(..., device=...)` declaration binds, if
/// its `device=` argument is a device reference at all. `None` is never a
/// silent default: the one caller that needs it turns it into a named
/// rejection on the `DeviceCap[D]` parameter that would have used it.
fn device_index_of(args: &[crate::eval::image::DeclArg]) -> Option<usize> {
    use crate::eval::image::ImageDeclRef;
    use crate::eval::value::Value;
    args.iter()
        .find(|a| a.label == "device")
        .and_then(|a| match &a.value {
            Value::ImageDecl(ImageDeclRef::Device(i)) => Some(*i),
            _ => None,
        })
}

/// One declaration's own boot `init` call. `kind` is the noun every
/// diagnostic here uses (`actor`/`driver`) so a driver is never described
/// as an actor and vice versa; `device` is the driver's own bound device
/// index (`None` for an actor, which binds none).
fn one_boot_init_call(
    kind: &str,
    decl_type: &crate::sema::types::Type,
    decl_args: &[crate::eval::image::DeclArg],
    device: Option<usize>,
    graph: &ImageGraph,
    inits: &BTreeMap<String, ActorInit>,
    backings: &BTreeMap<String, crate::eval::image_checks::PoolBacking>,
) -> Result<Option<BootInitCall>, LayoutError> {
    use crate::sema::types::{Type, render_type};

    let name = render_type(decl_type);
    let space = HandleSpace::from_graph(graph);
    let Some(init) = inits.get(&name) else {
        check_field_wired_args(kind, &name, decl_args, space)?;
        return Ok(None);
    };
    if init.ret != Type::Unit {
        let rendered = render_type(&init.ret);
        // plans/M7.md item E1: a fallible `init` returning
        // `Result[unit, BootError]` is now real — 03 §1's own constructor
        // signature. Boot allocates a reply slot, calls `init`, and on
        // `Err` aborts with a diagnosable line (plans/M6.md decision 12 /
        // plans/M7.md decision 8). Any other non-`unit` return still fails
        // closed: boot has nowhere to put the value.
        let ok_fallible = matches!(
            &init.ret,
            Type::Result(ok, err)
                if matches!(ok.as_ref(), Type::Unit)
                    && matches!(err.as_ref(), Type::Named(n, _) if n == "BootError")
        );
        if !ok_fallible {
            return Err(LayoutError::new(if matches!(init.ret, Type::Result(..)) {
                format!(
                    "{kind} `{name}` declares a fallible `init` returning `{rendered}`, and this \
                     image declares an instance of it — boot can only handle \
                     `Result[unit, BootError]` (03-hardware.md §1/§9); any other error type \
                     would need a recovery path this machine does not have yet"
                )
            } else {
                format!(
                    "{kind} `{name}` declares `init` returning `{rendered}`, and this image \
                     declares an instance of it — boot can only call an `init` returning \
                     `unit` or `Result[unit, BootError]`, and has nowhere to put a returned value."
                )
            }));
        }
    }
    if init.params.len() > 8 {
        return Err(LayoutError::new(format!(
            "{kind} `{name}`'s own `init` declares {} parameters; boot can pass at most 8 \
             (`x0` carries the receiver, leaving `x1..x8`) — the identical limit \
             `codegen` places on every other call.",
            init.params.len()
        )));
    }
    let mut args = Vec::with_capacity(init.params.len());
    for p in &init.params {
        // Reserved labels are skipped through the same predicate
        // `eval::image_checks` accepts them by, so the acceptance rule
        // and this materialization rule can never disagree about which
        // label is image-wiring metadata rather than an `init`
        // argument (a parameter that happens to be named `mailbox` is
        // therefore unsatisfiable on both sides alike, not satisfiable
        // on one).
        let wired = decl_args.iter().find(|a| {
            a.label == p.name && !crate::eval::image_checks::is_reserved_actor_arg(&a.label)
        });
        let Some(a) = wired else {
            let param_ty = render_type(&p.ty);
            if let Type::Named(tn, targs) = &p.ty {
                // plans/M7.md item H1: **the mint, materialized.** A
                // `@driver`'s `DeviceCap[D]` parameter carries no explicit
                // argument — 05-library.md §9 substitutes it from the
                // `device=` binding, and `check_capability_substitution`
                // has already checked that `D` *is* the device that
                // binding names. Its word is decision 11's: the base of
                // that device's own declared register window.
                if tn == "DeviceCap" {
                    let Some(i) = device else {
                        return Err(LayoutError::new(format!(
                            "{kind} `{name}`'s own `init` takes `{}: {param_ty}`, but this \
                             declaration binds no device — a `DeviceCap[D]` is authority over one \
                             device instance and is minted only from an `img.driver(..., \
                             device=...)` binding (03-hardware.md §1).",
                            p.name
                        )));
                    };
                    args.push(BootInitArg::DeviceRegsBase(i));
                    continue;
                }
                // plans/M7.md item H1: a `DmaPool[P, N]` parameter is
                // substituted the same way, and its word is decision 11's
                // — the base of pool `P`'s own backing, which item D
                // already sized, placed and reported.
                // `check_dma_pool_mint` has already checked that `P` is
                // bound, DMA, reachable from *this* driver's device and at
                // least `N` bytes wide, so nothing is re-derived here.
                if tn == "DmaPool" {
                    let Some(crate::sema::types::TypeArg::Pool(pool)) = targs.first() else {
                        return Err(LayoutError::new(format!(
                            "internal error: `{name}.init`'s own `{}: {param_ty}` names no pool",
                            p.name
                        )));
                    };
                    args.push(BootInitArg::PoolBase(pool.clone()));
                    continue;
                }
                // plans/M7.md item G, decision 12: an `IrqCap[V]` parameter
                // is the vector bit index. `check_irq_cap_mint` already
                // required `vector=`; the word is known here, so it is a
                // plain `Word` rather than a reloc against placement.
                if tn == "IrqCap" {
                    let Some(i) = device else {
                        return Err(LayoutError::new(format!(
                            "{kind} `{name}`'s own `init` takes `{}: {param_ty}`, but this \
                             declaration binds no device — an `IrqCap[V]` is minted from a \
                             device's declared `vector=` (03-hardware.md §6).",
                            p.name
                        )));
                    };
                    let Some(dev) = graph.devices.get(i) else {
                        return Err(LayoutError::new(format!(
                            "internal error: `{name}.init` takes an `IrqCap` for device#{i}, \
                             which does not exist"
                        )));
                    };
                    let Some(v) = crate::eval::image_checks::device_vector(&dev.args) else {
                        return Err(LayoutError::new(format!(
                            "internal error: `{name}.init` takes an `IrqCap` for device#{i}, \
                             which declared no `vector=` — `check_vector_bindings` should have \
                             rejected first"
                        )));
                    };
                    args.push(BootInitArg::Word(v));
                    continue;
                }
                if crate::eval::image_checks::is_capability_type_name(tn) {
                    return Err(LayoutError::new(format!(
                        "{kind} `{name}`'s own `init` takes `{}: {param_ty}`, a capability this \
                         image never wires explicitly — the image binding mints a `DeviceCap[D]`, \
                         a `DmaPool[P, N]` and an `IrqCap[V]` (from `vector=`) and nothing else \
                         (plans/M7.md items H1/G); an `Mmio[L]` comes from the sealed transport's \
                         own `map_partition` (03-hardware.md §2/§9), and the rest are named by \
                         `eval::image_checks::check_capability_substitution`. Failing closed \
                         rather than passing a zero.",
                        p.name
                    )));
                }
                if crate::eval::image_checks::is_protocol_state_type_name(tn) {
                    return Err(LayoutError::new(format!(
                        "{kind} `{name}`'s own `init` takes `{}: {param_ty}`, a bring-up state \
                         (03-hardware.md §9). A state is produced by a transition inside the \
                         driver, never handed to it: boot mints the `DeviceCap[D]` and the \
                         driver's own `init` calls `claim`.",
                        p.name
                    )));
                }
                if crate::eval::image_checks::is_handle_type_name(tn) {
                    return Err(LayoutError::new(format!(
                        "{kind} `{name}`'s own `init` takes `{}: {param_ty}` with no \
                         `{}=...` argument in this image — 05-library.md §9 allows an actor \
                         handle to be substituted by type there, but boot materializes only \
                         the arguments the image wires by name. Wire it explicitly, or wait \
                         for handle substitution.",
                        p.name, p.name
                    )));
                }
            }
            return Err(LayoutError::new(format!(
                "{kind} `{name}`'s own `init` takes `{}: {param_ty}` and this image wires no \
                 `{}=...` argument for it, so boot has no value to pass.",
                p.name, p.name
            )));
        };
        // plans/M7.md item E4 / decision 19: a pool wired to an `own[P] T`
        // or `[own[P] T; N]` parameter becomes the initial handles
        // 05-library.md §9 promises. Each handle is one word — the guest
        // address of a pool slot. A single `own` is that word; an array
        // is a pre-built table of them passed by the bare-pointer
        // aggregate ABI.
        if matches!(
            a.value,
            crate::eval::value::Value::ImageDecl(
                crate::eval::image::ImageDeclRef::Pool(_)
                    | crate::eval::image::ImageDeclRef::DmaPool(_)
            )
        ) {
            let pool_name = match &a.value {
                crate::eval::value::Value::ImageDecl(
                    crate::eval::image::ImageDeclRef::Pool(n)
                    | crate::eval::image::ImageDeclRef::DmaPool(n),
                ) => n.clone(),
                _ => unreachable!(),
            };
            let backing = backings.get(&pool_name).ok_or_else(|| {
                LayoutError::new(format!(
                    "internal error: `{name}.init` wires pool `{pool_name}`, which has no \
                     PoolBacking — `check_pool_decls` should have rejected first"
                ))
            })?;
            match &p.ty {
                Type::Own(own_pool, _) if own_pool == &pool_name => {
                    if backing.slots < 1 {
                        return Err(LayoutError::new(format!(
                            "{kind} `{name}` wires `{}=...` to a single `own[{pool_name}] _`, but \
                             pool `{pool_name}` declares zero slots",
                            a.label
                        )));
                    }
                    args.push(BootInitArg::OwnSlot {
                        pool: pool_name,
                        index: 0,
                        slot_bytes: backing.slot_bytes,
                    });
                    continue;
                }
                Type::Array(elem, len_expr) => {
                    if let Type::Own(own_pool, _) = elem.as_ref() {
                        if own_pool == &pool_name {
                            let n = crate::sema::bodies::literal_array_len(len_expr).ok_or_else(
                                || {
                                    LayoutError::new(format!(
                                        "{kind} `{name}`'s own `{}: {}` has a non-literal array \
                                         length — boot can only materialize a fixed `[own; N]`",
                                        p.name,
                                        render_type(&p.ty),
                                    ))
                                },
                            )?;
                            if n as u64 != backing.slots {
                                return Err(LayoutError::new(format!(
                                    "{kind} `{name}` wires `{}=...` to `[own[{pool_name}] _; {n}]`, \
                                     but pool `{pool_name}` declares {} slots — 05-library.md §9's \
                                     initial handles are exactly one per slot",
                                    a.label, backing.slots
                                )));
                            }
                            args.push(BootInitArg::OwnHandleArray {
                                pool: pool_name,
                                count: backing.slots,
                                slot_bytes: backing.slot_bytes,
                            });
                            continue;
                        }
                    }
                }
                _ => {}
            }
            return Err(LayoutError::new(format!(
                "{kind} `{name}` wires `{}=...` to `{name}.init`'s own `{}: {}` from a declared \
                 pool. The pool is real, but that parameter is not an `own[{pool_name}] T` or \
                 `[own[{pool_name}] T; N]` — 05-library.md §9's \"create the initial handles\" \
                 only substitutes those shapes (plans/M7.md item E4 / decision 19).",
                a.label,
                p.name,
                render_type(&p.ty),
            )));
        }
        let Some(word) = boot_init_arg_word(&a.value, space) else {
            return Err(LayoutError::new(format!(
                "{kind} `{name}` wires `{}=...` to `{name}.init`'s own `{}: {}`, but the \
                 value is {} — boot passes arguments in registers (`x1..`), and this \
                 compiler has no register representation for that shape. Failing closed \
                 rather than passing a zero.",
                a.label,
                p.name,
                render_type(&p.ty),
                value_shape_name(&a.value)
            )));
        };
        args.push(BootInitArg::Word(word));
    }
    Ok(Some(BootInitCall {
        key: init.key.clone(),
        args,
        fallible: matches!(
            &init.ret,
            Type::Result(ok, err)
                if matches!(ok.as_ref(), Type::Unit)
                    && matches!(err.as_ref(), Type::Named(n, _) if n == "BootError")
        ),
        err_msg: None,
    }))
}

/// Intern one abort message per fallible `init` into `rodata`, recording
/// the offset/len on the call. Must run **once** before either of
/// `build_runtime_block`'s two assembly passes, so both see the same
/// offsets and emit the same word count.
///
/// Message shape matches an `assert` failure inside `init`: the harness
/// `__wrela_abort` prepends `FAILED `, so the interned text is just
/// `"{Actor}.init returned Err"` — the `@driver`/`@actor` struct name is
/// already in `BootInitCall::key`. The concrete `BootError` variant is
/// not recovered (would need a second formatting path over the reply
/// slot); named in the plan's Done prose rather than pretended.
fn intern_fallible_init_abort_messages(
    wiring: &mut RuntimeWiring,
    rodata: &mut Vec<Vec<u8>>,
    rodata_cursor: &mut usize,
) {
    for call in wiring
        .init_calls
        .iter_mut()
        .chain(wiring.driver_init_calls.iter_mut())
        .flatten()
    {
        if !call.fallible || call.err_msg.is_some() {
            continue;
        }
        let bytes = format!("{} returned Err", call.key).into_bytes();
        let len = bytes.len();
        let off = append_rodata(rodata, rodata_cursor, bytes);
        call.err_msg = Some((off, len));
    }
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
                Item::Const(_)
                | Item::Enum(_)
                | Item::Pool(_)
                | Item::ComptimeIf(_)
                | Item::Static(_) => {}
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

/// plans/M8.md item D: the `mailbox=` capacity one declaration carries, or
/// `None` when it carries none. One reader for both `img.actor` (where the
/// label is required — M6 decision 3) and `img.driver` (where it is what
/// makes the driver messageable at all — 05-library.md §9), so the two can
/// never read the same label differently.
fn declared_mailbox_capacity(
    args: &[crate::eval::image::DeclArg],
    who: &str,
) -> Result<Option<u64>, String> {
    let Some(arg) = args.iter().find(|a| a.label == "mailbox") else {
        return Ok(None);
    };
    let capacity = value_as_u64(&arg.value).ok_or_else(|| {
        format!("{who}'s own `mailbox=` value is not a plain non-negative integer")
    })?;
    Ok(Some(capacity))
}

/// Every root that owns a mailbox, in the one order every consumer walks:
/// each declared `@actor`, then each messageable `@driver`
/// (`graph.drivers` order). `turn_owner`, `place_runtime_tables`,
/// `build_runtime_glue_block` and `RuntimePlacement::turn_area_for` all
/// read this order; a second spelling of it anywhere would be a silent
/// index skew between a turn area and the routine that writes it.
fn mailbox_root_names(tables: &RuntimeTables) -> Vec<String> {
    let mut out: Vec<String> = tables.actors.iter().map(|a| a.name.clone()).collect();
    for d in &tables.drivers {
        if d.mailbox.is_some() {
            out.push(d.name.clone());
        }
    }
    out
}

/// plans/M8.md item D, the security surface (decisions 20/21). A
/// messageable `@driver`'s mailbox admits its `pub` methods, so those
/// signatures are message shapes (02-language.md §9.4) and 03-hardware.md
/// §1's "a driver may export safe actor APIs but **never raw
/// capabilities**" is what decides which of them may exist at all.
///
/// Three refusals, each a separate sentence because each names a different
/// leak:
///
/// 1. **Nothing sealed crosses the mailbox, in either direction.** No 03
///    §1 capability, §9 protocol state or §4 sealed queue value may appear
///    in a parameter or reply — and *that* rule is not re-implemented
///    here: `sema::types::validate_fn_capability_types` already refuses
///    every one of them for every `pub` method of an actor or driver,
///    whether or not a mailbox exists (M7 decision 3: "checked where
///    `Actor[T]` already is ... do not build a second mechanism"), and
///    `err-driver-message-capability` pins that it still fires on a
///    *messageable* driver. What this pass adds is the two names that pass
///    lets through, each for its own good reason:
///
///    - **`Receipt[P]`**, which 03 §5 blesses by name for the handoff
///      convention — and a handoff needs the caller-side `await receipt`
///      that plans/M8.md item E makes executable. Refused here, by name,
///      pointing at item E, rather than admitted into a mailbox whose
///      caller cannot resolve what comes back.
///    - **`InterruptCell[T]`** (decision 23), which is not sealed
///      authority at all (M7 decision 17: source-constructible, an
///      `@actor` may hold one) and so is invisible to the containment
///      rules — but which 03 §6 calls "the **sole** ISR/ordinary-code
///      channel". A cell in a message is a second channel between
///      different principals, carrying the interrupt-status word's value
///      to a sender that owns none of §6's ordering.
///
///    Both directions are checked, because for `InterruptCell` both are
///    reachable: the parameter arm is live precisely where sema's is not.
/// 2. **The wrong effect set.** A `@task` bottom half (03 §6) is woken by
///    an ISR and drains completions; it is not a message. Declaring one
///    `pub` on a messageable driver would give one turn body two entry
///    paths — a wake and an admission — and the mailbox path carries none
///    of §6's ordering.
/// 3. **The ISR itself.** An interrupt handler bound with `irq.bind` runs
///    in 03 §6's restricted effect set against its device's registers.
///    Admitting one from a mailbox would run device acknowledge work as an
///    ordinary turn, at an arbitrary time, on behalf of an arbitrary
///    sender.
/// The reason clause both directions of `check_driver_message_surface`
/// append, keyed on which name was found. One writer, so a parameter and a
/// reply carrying the same type can never be told two different stories.
fn why_forbidden_across_a_driver_mailbox(found: &str) -> &'static str {
    if found.starts_with("InterruptCell") {
        // 03-hardware.md §6, quoted rather than paraphrased on "sole",
        // because that word is the whole argument.
        return ". 03-hardware.md §6: `InterruptCell[T]` is \"the sole ISR/ordinary-code \
                channel\", interrupt-atomic with respect to every vector that may touch the \
                cell — a channel between this driver's ISR and this driver's own ordinary code. \
                A mailbox is a different channel between different principals, and a cell that \
                crosses it is a second, unordered one. Export the value the cell holds, not the \
                cell";
    }
    if found.starts_with("Receipt") {
        // plans/M8.md item E, decision 33: item D's floor here refused a
        // receipt in *either* direction, naming this item. The reply
        // direction is now 03-hardware.md §5's blessed handoff and is
        // gone; this is the half that survives, and its reason is
        // narrower than "authority". A receipt names a slot in *this
        // driver's* queue, and 03 §5 gives it exactly one consumer on
        // the driver side — the bottom half's `claim`/`recover`. A sender
        // that could post one back into the mailbox would name a queue
        // slot it does not own, at a time the driver did not choose.
        return ". 03-hardware.md §5 gives a receipt one owner and one resolution: the caller \
                holds it and `await`s it; the driver's own bottom half resolves it. A mailbox \
                message posting one back into the driver would let an arbitrary sender name a \
                slot in this driver's queue. The handoff direction — a `Receipt[P]` *reply* \
                from a public synchronous method with exactly one `take p: P` parameter — is \
                the convention §5 blesses, and is accepted";
    }
    ", which 03-hardware.md §1 keeps inside the driver (\"a driver may export safe actor APIs \
     but never raw capabilities\")"
}

fn check_driver_message_surface(
    driver: &str,
    methods: &[ActorMethodShape],
    modules: &BTreeMap<String, Module>,
    decl_items: &[crate::sema::types::DeclItem],
) -> Result<(), String> {
    let bare = driver.split('[').next().unwrap_or(driver);
    let tasks = driver_task_method_names(modules, driver);
    let isrs = irq_bind_handlers_in_driver(modules, bare);
    for m in methods {
        for (i, ty) in m.param_types.iter().enumerate() {
            let Some(found) = crate::sema::types::driver_message_forbidden_carried(ty, decl_items)
            else {
                continue;
            };
            return Err(format!(
                "`@driver` `{driver}` is declared with `mailbox=`, so its `pub` method \
                 `{driver}.{}` is a message shape — and parameter #{} carries `{found}`{}",
                m.name,
                i + 1,
                why_forbidden_across_a_driver_mailbox(&found)
            ));
        }
        // plans/M8.md item E, decision 33: 03-hardware.md §5's handoff
        // reply is not probed at all, and the reason is structural rather
        // than an exemption. §5's signature is "exactly one `take p: P`
        // parameter and result `Receipt[P]`" — the *same* `P` — so the
        // loop above has already probed the whole payload as parameter
        // #k, with the same walk and the same leaf set. Probing
        // `Receipt[P]` here again could only ever refuse the `Receipt`
        // wrapper, which is the convention itself. Item D's floor
        // (`golden/err-driver-message-receipt`) was this arm refusing that
        // wrapper; it is retargeted at the direction that survives — a
        // receipt as a message *parameter* (05-library.md §2), which the
        // loop above still refuses by name.
        if !m.is_handoff {
            if let Some(found) =
                crate::sema::types::driver_message_forbidden_carried(&m.ret, decl_items)
            {
                return Err(format!(
                    "`@driver` `{driver}` is declared with `mailbox=`, so its `pub` method \
                     `{driver}.{}` is a message shape — and its reply carries `{found}`{}",
                    m.name,
                    why_forbidden_across_a_driver_mailbox(&found)
                ));
            }
        }
        if m.is_task || tasks.iter().any(|t| *t == m.name) {
            return Err(format!(
                "`@driver` `{driver}` is declared with `mailbox=`, but its `@task` bottom half \
                 `{driver}.{}` is `pub` — 03-hardware.md §6: a bottom half is woken by an ISR \
                 and drains completions, it is not a message. One turn body cannot have both \
                 entry paths; make it private",
                m.name
            ));
        }
        if isrs.iter().any(|h| *h == m.name) {
            return Err(format!(
                "`@driver` `{driver}` is declared with `mailbox=`, but its interrupt handler \
                 `{driver}.{}` is `pub` — 03-hardware.md §6: an ISR runs in the restricted \
                 interrupt effect set against its own device's registers, never as an admitted \
                 turn on behalf of a sender. Make it private",
                m.name
            ));
        }
    }
    Ok(())
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
    if graph.actors.is_empty() && graph.drivers.is_empty() && async_frames.is_empty() {
        return Ok(None);
    }
    let shapes = merge_actor_pub_methods(modules, layout_ctx).map_err(|e| e.message)?;
    // plans/M8.md item D: the turn-area owner set is every *mailbox* root,
    // not every actor — a messageable driver's own `pub async fn` parks in
    // its own turn area exactly as an actor's does, and must not be sized
    // as a free turn.
    let mut actor_names: Vec<String> = graph
        .actors
        .iter()
        .map(|d| crate::sema::types::render_type(&d.actor_type))
        .collect();
    for decl in &graph.drivers {
        let name = crate::sema::types::render_type(&decl.actor_type);
        if declared_mailbox_capacity(&decl.args, &format!("driver `{name}`"))?.is_some() {
            actor_names.push(name);
        }
    }

    let mut actors = Vec::with_capacity(graph.actors.len());
    for decl in &graph.actors {
        let name = crate::sema::types::render_type(&decl.actor_type);
        let mailbox_capacity = declared_mailbox_capacity(&decl.args, &format!("actor `{name}`"))?
            .ok_or_else(|| {
            format!(
                "actor `{name}` has no declared `mailbox=` capacity (plans/M6.md decision 3: \
                 the declared bound is the whole of M6's own mailbox-capacity story; derivation \
                 is out of scope)"
            )
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

    // plans/M7.md item H1: every declared `@driver` instance's own state
    // bytes, sized by the identical `mwir::size_of` an actor's are — which
    // is only answerable at all since this item taught it that a
    // capability is one word.
    // plans/M7.md item G: a `@task` adds one trailing wake-pending word
    // (sticky bit; mask–arm–recheck for the ISR→bottom-half edge).
    // plans/M8.md item D: a `mailbox=` on the declaration makes the driver
    // messageable, and its mailbox is sized by the *identical* arithmetic
    // an actor's is, from the identical `merge_actor_pub_methods` shapes —
    // one mailbox story, not a driver-shaped copy of it.
    let mut decl_items: Option<Vec<crate::sema::types::DeclItem>> = None;
    let mut drivers = Vec::with_capacity(graph.drivers.len());
    for decl in &graph.drivers {
        let name = crate::sema::types::render_type(&decl.actor_type);
        let mut state_size = mwir::size_of(&decl.actor_type, layout_ctx)
            .map_err(|e| format!("driver `{name}`'s own state: {e}"))?
            as u64;
        let wake_pending_off = if driver_declares_task(modules, &name) {
            let off = state_size;
            state_size += 8;
            Some(off)
        } else {
            None
        };
        let capacity = declared_mailbox_capacity(&decl.args, &format!("driver `{name}`"))?;
        let mailbox = match capacity {
            None => None,
            Some(capacity) => {
                let methods = shapes.get(&name).map(Vec::as_slice).unwrap_or(&[]);
                if decl_items.is_none() {
                    // The component table `sealed_authority_carried` walks
                    // spans modules: a plain wrapper struct declared in one
                    // module can carry a capability into a driver method
                    // declared in another.
                    decl_items = Some(closure_decl_items(modules).map_err(|e| e.message)?);
                }
                check_driver_message_surface(
                    &name,
                    methods,
                    modules,
                    decl_items.as_deref().unwrap_or(&[]),
                )?;
                let max_args_bytes = methods
                    .iter()
                    .map(|m| m.param_sizes.iter().sum::<u64>())
                    .max()
                    .unwrap_or(0);
                let max_async_frame = async_frames
                    .iter()
                    .filter(|(key, _)| turn_owner(key, &actor_names) == Some(name.as_str()))
                    .map(|(_, &bytes)| bytes)
                    .max()
                    .unwrap_or(0);
                Some(DriverMailbox {
                    capacity,
                    slot_size: 16 + max_args_bytes,
                    frame_size: crate::codegen::TURN_RECORD_SIZE + max_async_frame,
                })
            }
        };
        drivers.push(DriverRuntimeLayout {
            name,
            state_size,
            wake_pending_off,
            mailbox,
        });
    }

    let free_turns: Vec<(String, u64)> = async_frames
        .iter()
        .filter(|(key, _)| turn_owner(key, &actor_names).is_none())
        .map(|(key, &bytes)| (key.clone(), crate::codegen::TURN_RECORD_SIZE + bytes))
        .collect();

    // "actor count + 1 root" (decision 3), where "actor" is every mailbox
    // root: a messageable driver is selected by the same round-robin tick.
    let messageable_drivers = drivers.iter().filter(|d| d.mailbox.is_some()).count() as u64;
    let ready_queue_capacity = graph.actors.len() as u64 + messageable_drivers + 1;
    let group_arena_capacity = count_with_group_sites(modules);

    // plans/M10.md item 0a (decision 552): every turn area is *reserved* at
    // one image-wide power-of-two stride, so a turn reference can become an
    // index scaled by a shift instead of a bumped address. The stride is
    // `round_up_pow2` of the widest raw area over **all three** owner kinds
    // — actors, messageable drivers, free async fns. Missing one of them
    // undersizes the stride and the array overlaps itself, which is a
    // corrupted transcript rather than a compile error.
    let n_turns = actors.len() as u64 + messageable_drivers + free_turns.len() as u64;
    let widest_turn_area = actors
        .iter()
        .map(|a| a.frame_size)
        .chain(
            drivers
                .iter()
                .filter_map(|d| d.mailbox.as_ref())
                .map(|mb| mb.frame_size),
        )
        .chain(free_turns.iter().map(|(_, area)| *area))
        .max()
        .unwrap_or(0);
    // `widest_turn_area >= TURN_RECORD_SIZE` (64) whenever `n_turns > 0`, so
    // the degenerate `n <= 1` cases of the rounding never arise here.
    let turn_stride = if n_turns == 0 {
        0
    } else {
        1u64 << (64 - (widest_turn_area - 1).leading_zeros())
    };

    let mut total_bytes = 0u64;
    for a in &actors {
        total_bytes += a.state_size + a.mailbox_capacity * a.slot_size + MAILBOX_BOOKKEEPING_SIZE;
    }
    for d in &drivers {
        total_bytes += d.state_size;
        if let Some(mb) = &d.mailbox {
            total_bytes += mb.capacity * mb.slot_size + MAILBOX_BOOKKEEPING_SIZE;
        }
    }
    // Every turn area, at the uniform stride — one term, in place of the
    // three per-owner sums it replaces. `place_runtime_tables` bumps the
    // identical stride at the identical three sites; `verify_section_sizes`'
    // blob-length check is what catches the two ever disagreeing.
    total_bytes += n_turns * turn_stride;
    total_bytes += ready_queue_capacity * 8
        + RR_CURSOR_SIZE
        + group_arena_capacity * crate::codegen::GROUP_SLOT_SIZE;

    Ok(Some(RuntimeTables {
        actors,
        drivers,
        free_turns,
        n_turns,
        turn_stride,
        ready_queue_capacity,
        group_arena_capacity,
        // Single-core until placement says otherwise (`stripe_for_cores`),
        // and ringless until `add_cross_core_rings` says otherwise.
        rings: Vec::new(),
        cores: 1,
        total_bytes,
    }))
}

// --- report rendering (decision 7's own Layout section) -------------------

/// The two fixed, always-present machine regions below `IMAGE_BASE`
/// (module doc's own "pages"/"stacks" reporting note): the machine-info
/// page plus the console ring/data pages, combined into one `pages` fact,
/// and the three reserved per-core stacks as one `stacks` fact.
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
    // plans/M8.md item C1: one line per secondary core this image brings
    // up — the address the VMM starts that vCPU at once core 0 rings
    // `mmio::RELEASE_MMIO_ADDR` (06 §3: "releases the other vCPUs").
    // Absent entirely for a single-core image, so no pre-C1 report golden
    // moves; core 0's own entry stays the `Entry base=` line above.
    for (core, base) in &layout.core_entries {
        push_line(out, 1, &format!("CoreEntry core={core} base={base:#x}"));
    }

    // plans/M10.md item A2c / decision 588: one line per `@placed` static.
    // Absent entirely when the image declares none, so no pre-A2c report
    // golden moves.
    for s in &layout.placed_statics {
        push_line(
            out,
            1,
            &format!(
                "PlacedStatic name={} type={} addr={:#x} size={}",
                s.name, s.ty, s.addr, s.size
            ),
        );
    }

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
        // plans/M7.md item H1: one line per declared `@driver` instance.
        // Deliberately its own line kind, not an `Actor ...` line with
        // blanks: a driver without `mailbox=` has no ring and no turn
        // area, and printing `mailbox=0 slot=0 frame=0` would read as
        // three decisions the image did not make.
        // plans/M8.md item D: a driver declared with `mailbox=` gains the
        // same three facts, appended to its own line — so the report says
        // which drivers are messageable and how much they cost, and a
        // driver without a mailbox still says nothing it does not have.
        for d in &tables.drivers {
            let mailbox = match &d.mailbox {
                None => String::new(),
                Some(mb) => format!(
                    " mailbox={} slot={} frame={}",
                    mb.capacity, mb.slot_size, mb.frame_size
                ),
            };
            push_line(
                out,
                1,
                &format!("Driver name={} state={}{mailbox}", d.name, d.state_size),
            );
        }
        // plans/M8.md item C2: this image's own cross-core SPSC rings —
        // 04 §3's "sealed graph" made visible, one line per ring, so a
        // reviewer can see how many there are, which core produces into
        // each, how deep it is, and where the capacity came from. Absent
        // entirely for an image with no cross-core message edge.
        //
        // plans/M8.md item C3, decision 42: the line also carries the
        // ring's own placed `base=`, because the report is the VMM's whole
        // configuration and 06 §8 makes the VMM the recorder of
        // "per-mailbox cross-core admission order" — an admission this VMM
        // cannot address is one it cannot witness. One renderer
        // (`ring_report_lines`) serves this artifact and the runtime
        // report the VMM actually parses, so the two cannot disagree.
        for line in ring_report_lines(layout) {
            push_line(out, 1, &line);
        }
        // plans/M10.md item 0b (decision 555): the turn array's own three
        // facts, published so an image can see — and later assert — what
        // the uniform stride costs it. Placed directly under the per-owner
        // `frame=` lines it summarizes, so a reviewer reads `frame=56 /
        // frame=472` and then `stride=1024` on adjacent lines and can take
        // the padding off the page; `Totals` stays last.
        //
        // Deliberately *not* folded into `Totals`: that would churn a line
        // reviewers skim, in every actor-bearing golden, for a reason
        // unrelated to totals. Absent entirely when this image has no
        // runtime table, like every other line in this block.
        //
        // Making these two numbers `@layout_assert`-able is a separate item
        // (decision 568): `ImageReport` is a closed eight-field stdlib
        // surface, and a stdlib change does not belong in a byte-identity
        // commit.
        push_line(
            out,
            1,
            &format!(
                "Turns count={} stride={} bytes={}",
                tables.n_turns,
                tables.turn_stride,
                tables.n_turns * tables.turn_stride
            ),
        );
        push_line(
            out,
            1,
            &format!(
                "Totals actors={} drivers={} ready_queue={} group_arena={} bytes={}",
                tables.actors.len(),
                tables.drivers.len(),
                tables.ready_queue_capacity,
                tables.group_arena_capacity,
                tables.total_bytes
            ),
        );
    }

    // --- plans/M7.md item H1: the device register windows, appended for
    // the identical reason every accounting block above it is — facts
    // only, absent entirely for an image that binds no driver.
    //
    // **One** line kind, deliberately — unlike item D's pool block, which
    // emits an accounting `Pool ...` line *and* a mapping `BlkPool ...`
    // line. A mapping line exists to be parsed by a device model, and
    // `wrela-vmm::parse_report` has no register-window field to parse into:
    // machine v1's virtio-blk model has no register file at all
    // (06-machine.md §3, `wrela-vmm`'s `devices` module doc). Inventing a
    // device-kind-prefixed second line now would be naming a protocol
    // against a consumer that does not exist; `DeviceRegs` carries every
    // fact such a consumer would need (base, size, which device, which
    // driver), and item G — which gives the window's other side a writer,
    // 03 §6's own `interrupt_status` — is where the mapping half lands.
    for r in &layout.device_regs {
        push_line(
            out,
            1,
            &format!(
                "DeviceRegs device=device#{} type={} driver={} base={:#x} size={}",
                r.device, r.device_type, r.driver, r.base, r.size
            ),
        );
    }
    // plans/M7.md item G: the mapping half of DeviceRegs — a host write
    // into `interrupt_status` plus the vector to raise. Parsed by the
    // VMM (`IrqHostInject`); absent when no ISR is bound.
    for inj in &layout.irq_host_injects {
        push_line(
            out,
            1,
            &format!(
                "IrqHostInject base={:#x} offset={:#x} status={:#x} vector={}",
                inj.base, inj.offset, inj.status, inj.vector
            ),
        );
    }

    // --- plans/M7.md item D: DMA pool accounting (a milestone exit
    // criterion), appended after the actor tables for the identical
    // reason those were appended after `Entry base=...`: facts only,
    // absent entirely for an image with no pool, so no existing golden
    // moves that did not gain a pool.
    //
    // Two line kinds, and the split is the point:
    //
    // - `Pool ...` is the *accounting* line: 03-hardware.md §3's five
    //   declared facts about every bound pool, device-reachable or not
    //   (`PoolBacking`'s own doc comment derives each one). It is a
    //   report fact; nothing consumes it as configuration.
    // - `BlkPool name= device= base= size=` is the *mapping* line, and it
    //   exists for device-reachable pools only. It is the exact format
    //   `wrela-vmm`'s own `parse_report` reads (plans/M7.md item F,
    //   plans/M8.md item P), and the list of them is the whole of what
    //   that VMM maps — decision 5's security property, in the artifact
    //   rather than in a comment: a pool with no `device=` never produces
    //   one, so no device can reach it.
    //
    //   plans/M8.md item P made the line carry **its own device**, which
    //   is what the property needed all along: 03-hardware.md §3's "all
    //   memory a device can reach originates from *its* bound pools" is
    //   per-device, and until this landed the VMM handed every window to
    //   its one device model. The set emitted here is still every
    //   device-reachable pool in the image, not just the modelled
    //   device's — the VMM must know a window exists in order to refuse
    //   it to a device that does not own it, which is exactly what
    //   `golden/err-boot-blk-cross-device-pool` proves at boot.
    //
    // An image that declares a device-reachable pool but no queue is not
    // bootable yet, by design: `parse_report` refuses a `BlkPool` line
    // with no `BlkDevice`/`BlkQueue` to bind it to (those lines come from
    // item E1 when a configure site exists). Fail-closed and named,
    // rather than a window mapped for a device model that was never
    // configured.
    for p in &layout.pools {
        let b = &p.backing;
        let kind = if b.is_dma { "dma" } else { "image" };
        let mut line = format!(
            "Pool name={} kind={} payload={} slots={} slot_bytes={} base={:#x} size={} align={} \
             coherency=coherent",
            b.name, kind, b.payload, b.slots, b.slot_bytes, p.base, b.bytes, b.align
        );
        match b.device {
            Some(i) => line.push_str(&format!(" device=device#{i}")),
            None => line.push_str(" device=none"),
        }
        push_line(out, 1, &line);
    }
    for p in &layout.pools {
        let Some(dev) = p.backing.device else {
            continue;
        };
        push_line(
            out,
            1,
            &format!(
                "BlkPool name={} device=device#{dev} base={:#x} size={:#x}",
                p.backing.name, p.base, p.backing.bytes
            ),
        );
    }
    // plans/M7.md item E1: the VMM-facing device/queue lines
    // `parse_report` already consumes. Absent entirely until a
    // `VirtQueue.configure` site exists (`layout.blk`).
    if let Some(blk) = &layout.blk {
        push_line(
            out,
            1,
            &format!(
                "BlkDevice device=device#{} capacity_sectors={} features={:#x}{}",
                blk.device,
                blk.capacity_sectors,
                blk.features,
                match blk.vector {
                    Some(v) => format!(" vector={v}"),
                    None => String::new(),
                }
            ),
        );
        let q = &blk.queue;
        push_line(
            out,
            1,
            &format!(
                "BlkQueue index={} size={} desc={:#x} avail={:#x} used={:#x} doorbell={:#x}",
                q.index, q.size, q.desc, q.avail, q.used, q.doorbell
            ),
        );
        // Decision 2c / plans/M7.md item E2: occupancy bound is
        // floor(queue_depth / descriptors_per_op). Exits-per-op stays
        // deferred (decision 21).
        push_line(
            out,
            1,
            &format!(
                "BlkAccounting descriptors_per_op={} queue_depth={} occupancy_bound={}",
                blk.descriptors_per_op, q.size, blk.occupancy_bound
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

/// plans/M10.md item 0b (decisions 554/567): one turn's index into the
/// single contiguous `RT.turns` array `place_runtime_tables` lays down at
/// `rtdata_base`. The whole point of item 0 — the value a waker, a
/// reply-ring slot or a group's `owner_turn` will carry once 0c1/0c2/0c3
/// land, in place of a raw turn-area address.
///
/// **1-based, deliberately.** Array index 0 is a live turn in every
/// actor-bearing image (`tables.actors[0]`'s turn is placed first), so `0`
/// is not a free value — it is *made* free by biasing the id, and it then
/// serves as the `Option[TurnId]` niche. That is already the machine's
/// convention for exactly this problem: a group id is `arena_index + 1`
/// with `0` meaning "no ambient group". The field is private and
/// `from_index` is the only constructor, so `TurnId(0)` is
/// unconstructible — this type is the one place the niche is enforced,
/// rather than nine `cbz` sites assuming it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct TurnId(u32);

impl TurnId {
    /// The id of turn-array element `index` (0-based, `place_runtime_tables`'
    /// own order: actors, then messageable drivers, then free turns).
    pub fn from_index(index: usize) -> TurnId {
        // `n_turns` is bounded by the declaration count of one image; a
        // u32 overflow here would mean a 4-billion-root image, which the
        // 1 GiB ceiling refuses long before this does.
        let biased = u32::try_from(index + 1)
            .expect("a turn array with over 4 billion entries cannot fit the machine's memory");
        TurnId(biased)
    }

    /// The 1-based value an `Option[TurnId]` word holds — never `0`, which
    /// is what makes every existing `cbz`/`str xzr` "no waker" test keep
    /// meaning "none".
    pub fn get(self) -> u32 {
        debug_assert!(self.0 != 0, "TurnId(0) is the None niche, not an id");
        self.0
    }

    /// The 0-based array index — what `RuntimePlacement::turn_addr` scales
    /// by the stride. Bias removal lives here and nowhere else.
    pub fn index(self) -> usize {
        debug_assert!(self.0 != 0, "TurnId(0) is the None niche, not an id");
        self.0 as usize - 1
    }
}

/// One actor's own absolute runtime-table addresses, placed sequentially
/// from a given base (`rtdata_base` for a real image, or a host-mmap'd
/// stand-in base for a JIT/HVF test) — the exact byte order
/// `compute_runtime_tables`'s own `RuntimeTables::total_bytes` already
/// accounts for (state, ring, head/tail/count), so a real image's `rtdata`
/// section and this fn's own addresses can never disagree.
#[derive(Debug, Clone, Copy)]
pub struct ActorAddrs {
    pub state: u64,
    pub ring: u64,
    pub head: u64,
    pub tail: u64,
    pub count: u64,
    /// This actor's own turn area base — no longer bumped alongside the
    /// four fields above (plans/M10.md item 0b): it is
    /// `turns_base + (this actor's TurnId index << log2 stride)`, an
    /// element of the one contiguous turn array. Still a build-time
    /// absolute address, because the fully-unrolled scans want one.
    ///
    /// The fixed 48-byte turn record
    /// (`codegen::OFF_TURN_*`: busy/suspended/resume_ready/reply/waker/
    /// cur_method) followed by its persistent async frame slots. The
    /// address every message this actor's turns *send* carries as their
    /// waker, and the address `Reloc::TurnFrameAddr` resolves to for its
    /// own async methods.
    pub turn: u64,
}

impl ActorAddrs {
    /// This actor's mailbox, as the plain bounded ring it is — the shape
    /// `build_ring_enqueue` produces into, shared verbatim with every
    /// cross-core ring (plans/M8.md item C2, decision 28).
    pub fn mailbox(&self) -> RingAddrs {
        RingAddrs {
            ring: self.ring,
            head: self.head,
            tail: self.tail,
            count: self.count,
        }
    }
}

/// Every runtime-table address, placed from one `base` (`rtdata_base` for
/// a real image, a host-mmap'd stand-in for a JIT/HVF test) in the exact
/// byte order `compute_runtime_tables::total_bytes` accounts for.
///
/// plans/M10.md item 0b (decision 554) re-grouped that order. It is now:
/// the whole **turn array** first (`n_turns * turn_stride`, actors then
/// messageable drivers then free turns), then each actor's region (state,
/// ring, head/tail/count — the turn area no longer rides along), then each
/// driver's state and, when messageable, its own ring/head/tail/count, then
/// the per-core scheduler stripe, the group arena, and the cross-core
/// rings.
///
/// Turns first buys the one property `TurnId` needs: `turns_base ==
/// rtdata_base` exactly, so the address the whole runtime indexes from *is*
/// a section base and needs no arithmetic to resolve. Keeping them last
/// instead would only have preserved goldens this item moves anyway.
#[derive(Debug, Clone, Default)]
pub struct RuntimePlacement {
    /// plans/M10.md item 0b: the base of the one contiguous turn array —
    /// equal to `base` itself, and so to `rtdata_base` for a real image.
    pub turns_base: u64,
    /// The uniform stride the array is indexed at, copied from
    /// `RuntimeTables::turn_stride` so `turn_addr` below needs no second
    /// argument (`0` for an image with no turns, which then never indexes).
    pub turn_stride: u64,
    /// plans/M10.md item 0b: free async fn key -> that fn's own `TurnId` —
    /// the exact twin of `free_turns` below, and the only owner kind whose
    /// id is not positional (an actor's is its `tables.actors` index; a
    /// messageable driver's is `actors.len()` + its rank among the
    /// messageable drivers). `turn_id_for` is the reader; nothing indexes
    /// this map directly.
    pub turn_ids: BTreeMap<String, TurnId>,
    pub actors: Vec<ActorAddrs>,
    /// plans/M7.md item H1: each declared `@driver` instance's own state
    /// address, in `RuntimeTables::drivers` order. Placed after every
    /// actor's region and before the free-turn areas.
    ///
    /// plans/M8.md item D: a messageable driver's region continues past
    /// its state with the same ring/head/tail/count/turn run an actor's
    /// does, in the same order `RuntimeTables::total_bytes` accounts for —
    /// `driver_mailboxes` below holds those addresses, keyed by the same
    /// index. The `state` word here is unchanged either way, so every
    /// pre-item-D consumer (boot state fill, `Reloc::WakePending`, the ISR
    /// table) reads exactly what it always did.
    pub drivers: Vec<u64>,
    /// plans/M8.md item D: `RuntimeTables::drivers` index -> that
    /// messageable driver's own mailbox addresses. An `ActorAddrs`
    /// deliberately, not a driver-shaped twin: it is handed straight to
    /// `build_rt_enqueue` / `build_rt_select_and_run_symbolic`, the same
    /// two routines every actor's mailbox is built from. Its `state` field
    /// is the same address `drivers[i]` holds.
    pub driver_mailboxes: BTreeMap<usize, ActorAddrs>,
    /// fn key -> free-turn area base (`RuntimeTables::free_turns` order).
    pub free_turns: BTreeMap<String, u64>,
    /// The deterministic round-robin cursor word each core's own
    /// `rt_run_one` reads/advances (04 §2's tie-breaker; at M6 every
    /// scheduling key is equal, so the cursor is the whole selection order
    /// among that core's ready actors). One per live core
    /// (`RuntimeTables::cores`) — 04 §2's event loop is per core, so its
    /// cursor is too; a shared cursor would make one core's selection
    /// depend on another's, which is exactly the migration/stealing the
    /// chapter forbids. `rr_cursors[0]` is core 0's, at the identical
    /// address the single global cursor occupied before item C1.
    pub rr_cursors: Vec<u64>,
    /// plans/M6.md item F: the whole-image group arena's own base address
    /// — `Reloc::GroupArenaBase`'s own resolution target, placed last
    /// (`RuntimeTables::total_bytes`'s own byte-order doc: actors, free
    /// turns, ready-queue table, rr cursor, then the group arena).
    pub group_arena: u64,
    /// plans/M8.md item C2: each cross-core ring's own placed addresses, in
    /// `RuntimeTables::rings` order — placed after the group arena, so an
    /// image with no cross-core edge places byte-for-byte what it did
    /// before this item.
    pub rings: Vec<RingAddrs>,
}

impl RuntimePlacement {
    /// The address of turn-array element `id` — the one index→address rule,
    /// `turns_base + (index << log2 stride)`. `place_runtime_tables` fills
    /// every `ActorAddrs::turn` / `free_turns` value from this same
    /// expression, so a build-time address and a `TurnId` can never name
    /// different bytes.
    pub fn turn_addr(&self, id: TurnId) -> u64 {
        self.turns_base + (id.index() as u64) * self.turn_stride
    }

    /// `log2(turn_stride)` — the shift `push_turn_addr_from_id` scales an
    /// index by, and the whole reason item 0a made the stride a power of
    /// two. `0` for an image with no turns, which then never indexes (no
    /// `rt_*` routine is emitted at all).
    pub fn log2_turn_stride(&self) -> u8 {
        if self.turn_stride == 0 {
            0
        } else {
            debug_assert!(self.turn_stride.is_power_of_two());
            self.turn_stride.trailing_zeros() as u8
        }
    }

    /// The `TurnId` of async fn `key`'s turn (`turn_owner`'s own rule): an
    /// actor method's turn is its actor's; a messageable driver's `pub async
    /// fn` parks in the driver's one turn (plans/M8.md item D —
    /// non-reentrancy is per root, not per method); anything else owns its
    /// own free turn. `None` only for a key the tables never sized — an
    /// internal inconsistency the caller reports loudly.
    ///
    /// plans/M10.md item 0b: this is *the* owner-resolution rule.
    /// `turn_area_for` below is defined in terms of it rather than beside
    /// it, so there is one rule and not two that could skew.
    pub fn turn_id_for(&self, key: &str, tables: &RuntimeTables) -> Option<TurnId> {
        let roots = mailbox_root_names(tables);
        match turn_owner(key, &roots) {
            Some(root) => {
                if let Some(i) = tables.actors.iter().position(|a| a.name == root) {
                    return Some(TurnId::from_index(i));
                }
                let di = tables.drivers.iter().position(|d| d.name == root)?;
                // `driver_mailboxes` is keyed by `tables.drivers` index and
                // is a `BTreeMap`, so its key order *is* the messageable
                // subsequence of `tables.drivers` — the same order
                // `place_runtime_tables` assigned indices in.
                let rank = self.driver_mailboxes.keys().position(|k| *k == di)?;
                Some(TurnId::from_index(tables.actors.len() + rank))
            }
            None => self.turn_ids.get(key).copied(),
        }
    }

    /// The turn area address for async fn `key` — `turn_id_for` scaled by
    /// the stride.
    pub fn turn_area_for(&self, key: &str, tables: &RuntimeTables) -> Option<u64> {
        self.turn_id_for(key, tables).map(|id| self.turn_addr(id))
    }
}

pub fn place_runtime_tables(base: u64, tables: &RuntimeTables) -> RuntimePlacement {
    // plans/M10.md item 0b (decision 554): the turn array comes first and
    // whole, so `turns_base == base` and element `i` sits at a plain
    // stride multiple. Each area is *reserved* at the image-wide uniform
    // stride (item 0a), not at its owner's raw area — the owner's own
    // `frame_size` still says how much of it is live.
    //
    // Deliberately **not** aligned up to the stride: `add`-based indexing
    // does not need it, and `verify_section_sizes`' 8-byte inter-section
    // gap rule reports a larger gap as a producer bug (the prefix every
    // fuzz lane treats as a failure), not as an outcome.
    let turns_base = base;
    let turn_addr = |index: usize| turns_base + (index as u64) * tables.turn_stride;
    let mut cursor = base + tables.n_turns * tables.turn_stride;
    let mut actors = Vec::with_capacity(tables.actors.len());
    for (i, a) in tables.actors.iter().enumerate() {
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
        actors.push(ActorAddrs {
            state,
            ring,
            head,
            tail,
            count,
            // The first `tables.actors.len()` turn-array elements are the
            // actors', in this order.
            turn: turn_addr(i),
        });
    }
    let mut drivers = Vec::with_capacity(tables.drivers.len());
    let mut driver_mailboxes = BTreeMap::new();
    // The turn array continues with the messageable drivers, in
    // `tables.drivers` order (`mailbox_root_names`' own order).
    let mut next_turn = tables.actors.len();
    for (i, d) in tables.drivers.iter().enumerate() {
        let state = cursor;
        drivers.push(state);
        cursor += d.state_size;
        // plans/M8.md item D: same four regions, same order, same
        // arithmetic as the actor loop above.
        if let Some(mb) = &d.mailbox {
            let ring = cursor;
            cursor += mb.capacity * mb.slot_size;
            let head = cursor;
            cursor += 8;
            let tail = cursor;
            cursor += 8;
            let count = cursor;
            cursor += 8;
            let turn = turn_addr(next_turn);
            next_turn += 1;
            driver_mailboxes.insert(
                i,
                ActorAddrs {
                    state,
                    ring,
                    head,
                    tail,
                    count,
                    turn,
                },
            );
        }
    }
    // ...and ends with the free turns, in `tables.free_turns` order.
    let mut free_turns = BTreeMap::new();
    let mut turn_ids = BTreeMap::new();
    for (k, (key, _area)) in tables.free_turns.iter().enumerate() {
        let index = next_turn + k;
        free_turns.insert(key.clone(), turn_addr(index));
        turn_ids.insert(key.clone(), TurnId::from_index(index));
    }
    debug_assert_eq!(
        next_turn + tables.free_turns.len(),
        tables.n_turns as usize,
        "`compute_runtime_tables` and `place_runtime_tables` disagree about how many turns exist"
    );
    // plans/M8.md item C1: one ready-queue table + one round-robin cursor
    // per live core, uniformly strided (each core's pair sits at
    // `base + core * (ready_queue_capacity * 8 + RR_CURSOR_SIZE)`). With
    // `cores == 1` this is byte-for-byte the pre-C1 single reservation.
    let sched_base = cursor;
    let per_core = tables.ready_queue_capacity * 8 + RR_CURSOR_SIZE;
    let mut rr_cursors = Vec::with_capacity(tables.cores);
    for core in 0..tables.cores {
        rr_cursors.push(sched_base + (core as u64) * per_core + tables.ready_queue_capacity * 8);
    }
    cursor = sched_base + (tables.cores as u64) * per_core;
    let group_arena = cursor;
    cursor += tables.group_arena_capacity * crate::codegen::GROUP_SLOT_SIZE;
    // plans/M8.md item C2: the cross-core rings, last. Every address above
    // this point is unchanged for an image with none, which is what makes
    // "a single-core image emits no ring machinery at all" a placement fact
    // rather than a claim.
    let mut rings = Vec::with_capacity(tables.rings.len());
    for r in &tables.rings {
        let ring = cursor;
        cursor += r.capacity * r.slot_size;
        let head = cursor;
        cursor += 8;
        let tail = cursor;
        cursor += 8;
        let count = cursor;
        cursor += 8;
        rings.push(RingAddrs {
            ring,
            head,
            tail,
            count,
        });
    }
    RuntimePlacement {
        turns_base,
        turn_stride: tables.turn_stride,
        turn_ids,
        actors,
        drivers,
        driver_mailboxes,
        free_turns,
        rr_cursors,
        group_arena,
        rings,
    }
}

/// `rt_enqueue_actor(x0=method_idx, x1=arg0, x2=arg1, x3=waker_turn,
/// x4=waker_core) -> x0 (0=admitted, 1=rejected — the `send`/call admission
/// outcome, 02 §9.4's `NotAdmitted`/`Rejected` path, the minimal
/// encoding of it)`. Admission alone — never selection, never dispatch,
/// never readiness: a bounded ring insert, FIFO by construction (always
/// appended at `tail`, always drained from `head` by
/// `rt_select_and_run`) — 04 §2's "admission occupies one logical
/// mailbox slot until selection; selection is FIFO per mailbox by
/// admission order". The waker — plans/M10.md item 0c1: the awaiting
/// turn's own **`Option[TurnId]`** in `x3` (0 for a one-way `send`) plus
/// its `Option[CoreId]` in `x4` (0 = local), not the raw turn-area address
/// with a core tag in its top bits that it used to be — is stored into the
/// two `u32` halves of the slot's second word and carried to selection,
/// where the dispatched turn's completion delivers its reply there.
/// Admission is deliberately independent of the
/// target's `busy` flag: a message to a busy(-suspended) actor QUEUES —
/// decision 4's non-reentrancy lives entirely in selection, never here.
/// A full ring (`count == capacity`) is rejected without touching
/// `tail`/`count` at all — the caller's own arguments are left exactly
/// where they were (in its own registers, now that they never left
/// them), mirroring 02 §9.4's "an outcome that did not consume
/// [arguments] hands them back" at this ABI granularity (a real
/// `NotAdmitted(..)` payload carry-back is item G's job).
///
/// The arguments are **by value in `x1`/`x2`** — plans/M10.md item D0,
/// decision 610. They used to be `x1 = args_ptr` (the address of a
/// 2-word scratch pair in the caller's own frame) plus `x2 =
/// nargs_words`, copied out by a runtime loop. Nothing reachable ever
/// passed more than two words, and the *consumer* half of this ABI
/// (`build_rt_select_and_run`) already loaded exactly `x1`/`x2` out of
/// the slot, so this makes the two halves symmetric and removes the one
/// raw address that crossed this call boundary.
///
/// Register use (leaf fn, owns every register it touches, never `x0..x4`
/// until the outcome/scratch reuse below): `x9`/`x10` = count addr/value,
/// then reused as scratch after the branch; `x11` = capacity, then a
/// scratch; `x12`/`x13` = tail addr/value; `x14`/`x15` = slot-size scratch,
/// then the computed slot address. `x28` (`X_FRAME`) is never touched —
/// `emit_await_suspend` keeps using it across this `bl`.
pub fn build_rt_enqueue(
    addrs: &ActorAddrs,
    capacity: u64,
    slot_size: u64,
    start: usize,
) -> Vec<u32> {
    build_ring_enqueue(&addrs.mailbox(), capacity, slot_size, start)
}

/// `build_rt_enqueue`'s whole body, over any bounded ring — a mailbox
/// (above) or a cross-core request ring (plans/M8.md item C2). Every word
/// of the admission machinery is shared: a cross-core send is *the same
/// bounded-ring insert*, into a different ring, which is what makes 04 §3's
/// "cross-core actor edges keep identical message semantics" a structural
/// fact here rather than a claim two implementations have to keep true.
pub fn build_ring_enqueue(
    addrs: &RingAddrs,
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
    // plans/M10.md item 0c1 (decision 557): the slot's one waker word is
    // now two adjacent `u32` fields — `waker_turn` (an `Option[TurnId]`,
    // `x3`) at +8 and `waker_core` (an `Option[CoreId]`, `x4`) at +12 —
    // carried through to the selected turn's record at `OFF_TURN_WAKER`/
    // `OFF_TURN_WAKER + 4` unchanged in shape. `slot_size` does not move:
    // both fields live inside the word the tagged address occupied, which
    // is the whole point of the two-`u32` encoding (a second 64-bit word
    // would have cost 8 bytes on every mailbox slot image-wide).
    a.push(encode::enc_str_w_imm(3, 15, 8)); // slot.waker_turn = w3
    a.push(encode::enc_str_w_imm(4, 15, 12)); // slot.waker_core = w4

    // The arguments arrive **by value** in `x1`/`x2` (plans/M10.md item
    // D0, decision 610), so the store count is a build-time constant of
    // this per-ring specialization rather than a runtime `nargs` the
    // caller has to compute. It is written as *the same expression*
    // `build_rt_select_and_run` uses to load them back out
    // (`arg_words` there, search this file) — producer and consumer
    // agreeing by construction, not by two constants that happen to
    // match. The `saturating_sub`/`.min(2)` is load-bearing at both
    // ends: the smallest legal slot is `slot_size = 16` (a no-arg
    // message), where storing a fixed word would write past this ring
    // slot into the next one.
    let arg_words = ((slot_size.saturating_sub(16)) / 8).min(2);
    if arg_words >= 1 {
        a.push(encode::enc_str_x_imm(1, 15, 16)); // slot.arg0 = x1
    }
    if arg_words >= 2 {
        a.push(encode::enc_str_x_imm(2, 15, 24)); // slot.arg1 = x2
    }

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

/// Raises core `core`'s own pending word (`pending::core_word_addr`), bit
/// 0 — 06 §5's doorbell: "a shared-memory word, no trap". A plain
/// read-modify-write is sound here for the same reason
/// `build_checkpoint_and_vector_stub`'s own clear is: decision 11's baton
/// means exactly one vCPU is inside `hv_vcpu_run` at any instant, so there
/// is no concurrent writer to race. Uses `x9`/`x10`/`x11`.
///
/// Bit 0 rather than a ring-private vector bit is deliberate
/// (decision 30): the pending word is a "something changed, re-derive
/// readiness from memory" signal, and 04 §2 requires exactly that of it
/// ("wakes are idempotent; the runtime park primitive has mask-arm-recheck
/// semantics"). A core woken for a ring drains its rings because its loop
/// always does, not because a bit told it to.
fn push_raise_pending(a: &mut Asm, core: usize) {
    a.load_imm(9, wrela_machine::pending::core_word_addr(core));
    a.push(encode::enc_ldr_x_imm(10, 9, 0));
    a.push(encode::enc_movz(11, 1, 0, true));
    a.push(encode::enc_orr_reg(10, 10, 11, true));
    a.push(encode::enc_str_x_imm(10, 9, 0));
}

/// Emits `head`/`tail` advance-and-wrap for a bounded ring: `reg` holds
/// the current index, `addr_reg` is scratch, `cursor_addr` is the head or
/// tail word's own address. Shared by every drain lane below so the wrap
/// arithmetic exists once.
fn push_ring_advance(a: &mut Asm, reg: u8, scratch: u8, cursor_addr: u64, capacity: u64) {
    a.push(encode::enc_add_imm(reg, reg, 1, true));
    a.load_imm(scratch, capacity);
    a.push(encode::enc_cmp_reg(reg, scratch, true));
    let skip_nowrap = a.skip_placeholder(); // b.lt .nowrap
    a.push(encode::enc_movz(reg, 0, 0, true));
    let nowrap = a.abs();
    a.patch_cond(skip_nowrap, Cond::Lt);
    debug_assert_eq!(nowrap, a.abs());
    a.load_imm(scratch, cursor_addr);
    a.push(encode::enc_str_x_imm(reg, scratch, 0));
}

/// plans/M8.md item C2: `__rt_xsend_<src>_<Actor>` — a **cross-core send**,
/// with byte-for-byte the ABI `__rt_enqueue_<Actor>` has
/// (`x0=method_idx, x1=arg0, x2=arg1, x3=waker_turn, x4=waker_core -> x0` =
/// 0 admitted / 1 rejected). That identity is the whole design: codegen
/// emits one symbolic call for every `send`/`await` and never learns
/// whether the edge crosses a core, so 04 §3's "cross-core actor edges keep
/// identical message semantics" holds by construction rather than by two
/// code paths agreeing.
///
/// Three steps, in order:
///
/// 1. **Tag the waker** with the sending core (`waker_core_tag`, decision
///    30) — skipped for `waker_turn == 0`, a one-way `send`, which expects
///    no reply. The tag is what lets the completing turn on the far core
///    tell a local waker from one whose turn record lives back here.
/// 2. **Enqueue into the request ring** — literally `build_ring_enqueue`,
///    the same routine a mailbox admission uses, against this edge's ring.
///    A full ring returns 1 and touches nothing, exactly as a full mailbox
///    does (02 §9.4's `NotAdmitted`/`Rejected` path): the sender's own
///    already-emitted rejection handling fires unchanged, and no message is
///    ever silently dropped or truncated.
/// 3. **Wake the owning core** — raise its pending word. Ordered *after*
///    the enqueue, so a core woken by this store always finds the message
///    already published.
///
/// The wake is skipped on rejection: nothing was published, so there is
/// nothing to wake for.
fn build_rt_xsend(
    ring_enqueue_start: usize,
    src_core: usize,
    dst_core: usize,
    start: usize,
) -> Asm {
    let mut a = Asm::new(start);
    a.push(encode::enc_sub_imm(31, 31, 16, true));
    a.push(encode::enc_str_x_imm(30, 31, 0));

    // plans/M10.md item 0c1: the tag is a whole field now, so "tagging" is
    // one `movz` into `x4` instead of a `load_imm` of a shifted constant
    // OR'd into the address in `x3`. `x3` (the `Option[TurnId]`) is left
    // exactly as the caller passed it; a waker-less `send` (`w3 == 0`)
    // leaves `x4` as the caller's own zero, so "no waker" stays a single
    // zero test on one field.
    let skip_notag = a.skip_placeholder(); // cbz w3, .notag
    a.push(encode::enc_movz(4, waker_core_tag(src_core), 0, false));
    let notag = a.abs();
    a.patch_cbz_w(skip_notag, 3);
    debug_assert_eq!(notag, a.abs());

    a.bl_to(ring_enqueue_start);
    let skip_out = a.skip_placeholder(); // cbnz x0, .out (rejected)
    push_raise_pending(&mut a, dst_core);
    a.push(encode::enc_movz(0, 0, 0, true)); // admitted
    let out = a.abs();
    a.patch_cbnz(skip_out, 0);
    debug_assert_eq!(out, a.abs());

    a.push(encode::enc_ldr_x_imm(30, 31, 0));
    a.push(encode::enc_add_imm(31, 31, 16, true));
    a.push(encode::enc_ret(30));
    a
}

/// plans/M8.md item C2: `__rt_xreply_<src>_<dst>(x0 = destination
/// `TurnId` in the low half / reply tag in the high half, x1 = reply
/// word)` — the **reply half** of a cross-core edge.
/// plans/M10.md item 0c1 moved `x0` from a raw turn-area address to the
/// index; item J packs the reply tag into the high half of the same
/// word (decision 665), so the ring slot stays 16 bytes.
/// This routine never dereferences the TurnId, it only publishes the
/// packed word into a slot the destination core's own drain reads.
/// A reply is an edge in the other direction and travels the same way a
/// request does: a bounded ring plus a wake, never a store straight into
/// another core's turn record.
///
/// Its own lane, separate from the request ring (decision 29). A request
/// ring legitimately back-pressures — a full mailbox on the far side leaves
/// messages sitting in it — and a reply has nowhere to go back to, so
/// sharing one ring would let a stalled request lane strand a reply. Sized
/// so full is unreachable (`reply_ring_capacity`), with a `BRK` rather than
/// a rejection if it ever is: there is no caller to hand a rejection to.
///
/// A leaf routine (no `BL`), so it clobbers no link register and needs no
/// frame: `x9..x15` scratch, `x0`/`x1` its arguments.
fn build_rt_xreply(addrs: &RingAddrs, capacity: u64, dst_core: usize, start: usize) -> Asm {
    let mut a = Asm::new(start);

    a.load_imm(9, addrs.count);
    a.push(encode::enc_ldr_x_imm(10, 9, 0));
    a.load_imm(11, capacity);
    a.push(encode::enc_cmp_reg(10, 11, true));
    let skip_ok = a.skip_placeholder(); // b.lt .ok
    a.push(encode::enc_brk(BRK_XREPLY_RING_FULL));
    let ok = a.abs();
    a.patch_cond(skip_ok, Cond::Lt);
    debug_assert_eq!(ok, a.abs());

    // slot = ring + tail * REPLY_SLOT_SIZE
    a.load_imm(12, addrs.tail);
    a.push(encode::enc_ldr_x_imm(13, 12, 0));
    a.load_imm(14, REPLY_SLOT_SIZE);
    a.push(encode::enc_mul(14, 13, 14, true));
    a.load_imm(15, addrs.ring);
    a.push(encode::enc_add_reg(15, 15, 14, true));
    // plans/M10.md item J (decision 665): word 0 is
    // `TurnId | (reply_tag << 32)` — TurnId in the low half (0c1), tag in
    // the high half that 0c1 left as unused padding. `REPLY_SLOT_SIZE`
    // stays 16.
    a.push(encode::enc_str_x_imm(0, 15, 0)); // slot.turn_and_tag = x0
    a.push(encode::enc_str_x_imm(1, 15, 8)); // slot.reply = x1

    push_ring_advance(&mut a, 13, 12, addrs.tail, capacity);

    a.load_imm(9, addrs.count);
    a.push(encode::enc_ldr_x_imm(10, 9, 0));
    a.push(encode::enc_add_imm(10, 10, 1, true));
    a.push(encode::enc_str_x_imm(10, 9, 0));

    push_raise_pending(&mut a, dst_core);
    a.push(encode::enc_ret(30));
    a
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
///   - **Aggregate replies** (plans/M7.md item Z1, decision 9a): an arm
///     whose method declares an aggregate reply first loads the parked
///     caller's staging-slot address out of the turn record
///     (`[waker + OFF_TURN_REPLY_SLOT]`) into `x8`, this machine's
///     aggregate-return-pointer register, so the method writes its reply
///     straight into the awaiting frame. Such an arm then delivers a
///     deterministic 0 in the scalar reply word — the value already went
///     through `x8`, and nothing else is copied.
///
/// `dispatch[i]` = (call target, is_async). Register use: `x9..x13`
/// scratch; `x15` = method_idx, live across the dispatch chain; `x0`
/// (self ptr) / `x1`/`x2` (args) are set before `.dispatch` and must
/// survive every arm's own preamble.
///
/// This JIT/HVF-facing entry point carries no aggregate-reply flags: every
/// dispatch target it is ever given is a hand-built conformance stub with
/// a scalar reply (`wrela-vmm`'s own harness pair, and this file's own
/// `harness_jit` suite), so the flag is `false` by construction rather
/// than by convention. The real image path is
/// `build_rt_select_and_run_symbolic` below, which carries the real ones.
pub fn build_rt_select_and_run(
    addrs: &ActorAddrs,
    capacity: u64,
    slot_size: u64,
    dispatch: &[(usize, bool)],
    frame_area_size: u64,
    // plans/M10.md item 0c1: the turn array's own base and log2 stride —
    // the two build-time constants `push_turn_addr_from_id` needs to turn
    // the `Option[TurnId]` a waker now is back into an address. For a real
    // image these are `RuntimePlacement::turns_base` /
    // `log2_turn_stride()`; a JIT/HVF harness passes its own stand-in pair.
    turns_base: u64,
    log2_stride: u8,
    start: usize,
) -> Vec<u32> {
    let colors: Vec<(bool, bool)> = dispatch
        .iter()
        .map(|(_, is_async)| (*is_async, false))
        .collect();
    build_rt_select_and_run_core(
        addrs,
        capacity,
        slot_size,
        &colors,
        frame_area_size,
        &[],
        turns_base,
        log2_stride,
        start,
        |a, idx| a.bl_to(dispatch[idx].0),
    )
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
    dispatch: &[(String, bool, bool)],
    frame_area_size: u64,
    // plans/M8.md item C2: `(remote core, that core pair's own
    // `__rt_xreply_*` start word)` for every core whose turns can hold a
    // waker on a message admitted here. Empty for every single-core image
    // and for every actor no other core sends to — such an actor's
    // delivery path keeps its pre-C2 bytes exactly.
    xreply: &[(usize, usize)],
    turns_base: u64,
    log2_stride: u8,
    start: usize,
) -> Asm {
    let colors: Vec<(bool, bool)> = dispatch
        .iter()
        .map(|(_, is_async, reply_is_aggregate)| (*is_async, *reply_is_aggregate))
        .collect();
    build_rt_select_and_run_core(
        addrs,
        capacity,
        slot_size,
        &colors,
        frame_area_size,
        xreply,
        turns_base,
        log2_stride,
        start,
        |a, idx| a.bl_call_key(&dispatch[idx].0),
    )
}

/// plans/M7.md item Z1: the dispatch arm's own should-be-unreachable
/// guard — a method whose declared reply is an aggregate was selected on
/// a turn with **no waker**. Unreachable by construction: the only
/// waker-less admission is `send`, and 02-language.md §9.4 makes `send`'s
/// target a unit-returning method (enforced at `sema::bodies`'
/// `check_send_call`), so no aggregate-reply method can ever be enqueued
/// without one. A 0 here is a producer bug in this compiler, not a
/// program's doing — the same class as the dispatch table's own
/// no-arm-matched `brk 0xACD0` right below it, and deliberately the same
/// treatment.
const BRK_REPLY_SLOT_NO_WAKER: u16 = 0xACD6;

#[allow(clippy::too_many_arguments)]
fn build_rt_select_and_run_core(
    addrs: &ActorAddrs,
    capacity: u64,
    slot_size: u64,
    // Per dispatch index, in declaration order: `(is_async,
    // reply_is_aggregate)` — the two build-time facts an arm's own shape
    // depends on (`ActorMethodShape`).
    methods: &[(bool, bool)],
    // This actor's own whole turn-area size (`ActorRuntimeLayout::frame_size`)
    // — the turn record plus its widest async frame. An actor with no async
    // method has exactly the record and no frame slots at all.
    frame_area_size: u64,
    // plans/M8.md item C2: see `build_rt_select_and_run_symbolic`. Empty
    // means "no cross-core waker can reach this actor" and emits not one
    // extra instruction.
    xreply: &[(usize, usize)],
    // plans/M10.md item 0c1: see `build_rt_select_and_run`.
    turns_base: u64,
    log2_stride: u8,
    start: usize,
    mut call_dispatch: impl FnMut(&mut Asm, usize),
) -> Asm {
    use crate::codegen::{
        CALL_ERROR_TAG_CANCELLED, OFF_TURN_CUR_METHOD, OFF_TURN_REPLY, OFF_TURN_REPLY_SLOT,
        OFF_TURN_REPLY_TAG, OFF_TURN_RESUME_READY, OFF_TURN_SUSPENDED, OFF_TURN_WAKER,
        REPLY_TAG_OK, TURN_RECORD_SIZE, TURN_STATUS_CANCELLED, TURN_STATUS_SUSPENDED,
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
    // plans/M10.md item 0c1: the slot's waker word is two `u32` fields —
    // `waker_turn` at +8, `waker_core` at +12 — copied field-for-field into
    // the turn record's own pair at `OFF_TURN_WAKER`/`+4`. `ldr w`, never
    // `ldr x`: an `x` load here would fold the core into the index's high
    // bits and reinvent the tagging this item deleted.
    a.push(encode::enc_ldr_w_imm(10, 13, 8)); // w10 = waker_turn
    a.push(encode::enc_ldr_w_imm(14, 13, 12)); // w14 = waker_core
    a.load_imm(9, addrs.turn);
    a.push(encode::enc_str_x_imm(15, 9, OFF_TURN_CUR_METHOD as u16));
    a.push(encode::enc_str_w_imm(10, 9, OFF_TURN_WAKER as u16));
    a.push(encode::enc_str_w_imm(14, 9, OFF_TURN_WAKER as u16 + 4));
    // plans/M6.md item F: a freshly selected turn starts with *no* ambient
    // lineage (02-language.md §9.5 — a message carries no group; a turn's
    // lineage is its own task root's, and an actor turn's root is the
    // message that started it). The two lineage slots are the first two
    // words past the turn record (`codegen::LINEAGE_GROUP_SLOT`/
    // `LINEAGE_DEADLINE_SLOT`, `flowwir::FrameLayout`'s fixed convention),
    // and a previous activation of a *different* method on this same actor
    // could otherwise leave a stale group id behind — harmless today only
    // because every M6 actor method that opens a group also closes it, but
    // correct by construction now rather than by accident.
    // Guarded on the area actually *having* those two slots: an actor with
    // no `async` method at all gets a turn area of exactly
    // `TURN_RECORD_SIZE` bytes and no frame slots past it, so storing there
    // would scribble on whatever `place_runtime_tables` put next (a real
    // bug the flagship group goldens caught the moment this zeroing was
    // added unguarded — the group arena's own `in_use` words, three regions
    // later, came back set and `GroupCreate` reported "arena capacity
    // exceeded").
    //
    // plans/M10.md item 0a: the guard stays keyed on this owner's own **raw**
    // area (`ActorRuntimeLayout::frame_size` / `DriverMailbox::frame_size`,
    // both unchanged by the uniform-stride reservation), never on
    // `RuntimeTables::turn_stride`. The stride answers "how many bytes were
    // reserved", not "does this owner have lineage slots" — keyed on the
    // stride, a bare actor whose area is exactly `TURN_RECORD_SIZE` would
    // start emitting these two stores into its own padding, changing
    // `rtcode` for no reason and scribbling past its record the moment the
    // grouping changes.
    if frame_area_size >= TURN_RECORD_SIZE + 16 {
        a.push(encode::enc_str_x_imm(31, 9, TURN_RECORD_SIZE as u16));
        a.push(encode::enc_str_x_imm(31, 9, (TURN_RECORD_SIZE + 8) as u16));
    }
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
    for (idx, &(is_async, reply_is_aggregate)) in methods.iter().enumerate() {
        a.push(encode::enc_cmp_imm(15, idx as u16, true));
        let skip_next = a.skip_placeholder(); // b.ne .next
        if reply_is_aggregate {
            // plans/M7.md item Z1 (decision 9a): hand the method its
            // caller's own staging slot in `x8`. The waker is this turn
            // record's own (stored at fresh selection, still there on a
            // resume — it is only cleared at delivery), and the parked
            // caller wrote `OFF_TURN_REPLY_SLOT` immediately before
            // enqueueing this very message. `x9`..`x13` only: `x0` (self),
            // `x1`/`x2` (args) and `x15` (method index) are all live
            // across this preamble.
            //
            // plans/M10.md item 0c1 (decision 565): two index→address
            // conversions, because neither word is an address any more.
            // The waker is an `Option[TurnId]`, and `OFF_TURN_REPLY_SLOT`
            // is a `(TurnId, byte offset within that turn area)` pair — a
            // frame-interior reference, whose offset is the *caller's*
            // per-fn `Frame::reply_stage_off` and so cannot be recovered
            // from an index alone.
            a.load_imm(9, addrs.turn);
            a.push(encode::enc_ldr_w_imm(10, 9, OFF_TURN_WAKER as u16));
            let skip_have_waker = a.skip_placeholder(); // cbnz w10, .have_waker
            a.push(encode::enc_brk(BRK_REPLY_SLOT_NO_WAKER));
            a.patch_cbnz_w(skip_have_waker, 10);
            // x10 = the waker's own turn area.
            push_turn_addr_from_id(&mut a, 10, 11, turns_base, log2_stride);
            a.push(encode::enc_ldr_w_imm(8, 10, OFF_TURN_REPLY_SLOT as u16));
            a.push(encode::enc_ldr_w_imm(
                11,
                10,
                OFF_TURN_REPLY_SLOT as u16 + 4,
            ));
            // x8 = that turn area + the staging slot's own interior offset.
            push_turn_addr_from_id(&mut a, 8, 12, turns_base, log2_stride);
            a.push(encode::enc_add_reg(8, 8, 11, true));
        }
        call_dispatch(&mut a, idx);
        if is_async {
            // x0 = status; on completion x1 = the scalar reply.
            a.push(encode::enc_cmp_imm(0, TURN_STATUS_SUSPENDED as u16, true));
            let skip_not_suspended = a.skip_placeholder(); // b.ne .not_suspended
            // Suspended: a real slice ran; busy stays set; x0 is
            // already 1 (TURN_STATUS_SUSPENDED) — the "ran" report.
            to_epilogue.push(a.skip_placeholder());
            let not_suspended = a.abs();
            a.patch_cond(skip_not_suspended, Cond::Ne);
            debug_assert_eq!(not_suspended, a.abs());
            // plans/M10.md item J: `TURN_STATUS_CANCELLED` from an actor
            // turn delivers `CallError::Cancelled` through the reply tag
            // (decision 559) — the representation gap that used to force
            // `BRK_ACTOR_TURN_CANCELLED` is closed. Still rare by
            // construction at M6 (lineage zeroed at fresh selection), but
            // no longer a trap when it does happen.
            a.push(encode::enc_cmp_imm(0, TURN_STATUS_CANCELLED as u16, true));
            let skip_completed = a.skip_placeholder(); // b.ne .completed
            a.push(encode::enc_movz(9, 0, 0, true)); // reply = 0
            a.load_imm(14, CALL_ERROR_TAG_CANCELLED);
            to_deliver.push(a.skip_placeholder()); // b .deliver
            let completed = a.abs();
            a.patch_cond(skip_completed, Cond::Ne);
            debug_assert_eq!(completed, a.abs());
        }
        // The one word that differs between the three method shapes —
        // what `.deliver` will store into the waker's own reply slot.
        // A sync method's return IS completion (reply in x0); an async
        // one that got here completed (reply in x1); and an
        // aggregate-reply method of either color already wrote its whole
        // reply through `x8` into the awaiting frame, so its scalar word
        // is a deliberate, deterministic 0 — that word is image-visible,
        // and a stable 0 beats whatever register state the method
        // happened to leave behind.
        let reply_reg = match (reply_is_aggregate, is_async) {
            (true, _) => 31, // xzr
            (false, true) => 1,
            (false, false) => 0,
        };
        a.push(encode::enc_mov_reg(9, reply_reg, true)); // x9 = reply
        a.load_imm(14, REPLY_TAG_OK); // x14 = Ok
        to_deliver.push(a.skip_placeholder());
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
    a.push(encode::enc_ldr_w_imm(11, 10, OFF_TURN_WAKER as u16));
    let skip_no_waker = a.skip_placeholder(); // cbz w11, .no_waker
    // plans/M8.md item C2: a waker whose `waker_core` field is nonzero
    // (decision 30) names a turn record on **another** core, so its reply
    // goes back the way the request came — over that core pair's own reply
    // ring — instead of being stored straight into a remote turn record.
    // Emitted only for an actor that a cross-core edge can actually reach;
    // every single-core image, and every actor no other core messages,
    // keeps the untouched two-store delivery below, word for word.
    //
    // plans/M10.md item 0c1: the core is its own `u32` field at
    // `OFF_TURN_WAKER + 4`, so this is one `ldr w` and a `cbz w` — the
    // `lsr`/`load_imm`/`bic` untag chain is gone, and `x11` stays a pure
    // `TurnId` all the way to the remote arm's `x0`.
    let mut to_after_remote: Vec<usize> = Vec::new();
    if !xreply.is_empty() {
        a.push(encode::enc_ldr_w_imm(13, 10, OFF_TURN_WAKER as u16 + 4));
        let skip_local = a.skip_placeholder(); // cbz w13, .local
        for (remote_core, routine) in xreply {
            a.push(encode::enc_cmp_imm(13, (*remote_core as u16) + 1, false));
            let skip_arm = a.skip_placeholder(); // b.ne .next_arm
            // x0 = TurnId | (reply_tag << 32); x1 = reply. Item J packs
            // the tag into the high half of the TurnId word (decision 665).
            a.push(encode::enc_mov_reg(0, 11, false)); // w0 = waker TurnId
            a.push(encode::enc_lsl_imm(15, 14, 32, true));
            a.push(encode::enc_orr_reg(0, 0, 15, true));
            a.push(encode::enc_mov_reg(1, 9, true)); // x1 = reply
            a.bl_to(*routine);
            to_after_remote.push(a.skip_placeholder()); // b .after_remote
            let next_arm = a.abs();
            a.patch_cond(skip_arm, Cond::Ne);
            debug_assert_eq!(next_arm, a.abs());
        }
        a.push(encode::enc_brk(BRK_XREPLY_UNKNOWN_CORE));
        let local = a.abs();
        a.patch_cbz_w(skip_local, 13);
        debug_assert_eq!(local, a.abs());
    }
    // .local: index→address, then tag + reply + resume_ready.
    push_turn_addr_from_id(&mut a, 11, 12, turns_base, log2_stride);
    a.push(encode::enc_str_x_imm(14, 11, OFF_TURN_REPLY_TAG as u16));
    a.push(encode::enc_str_x_imm(9, 11, OFF_TURN_REPLY as u16));
    a.push(encode::enc_movz(12, 1, 0, true));
    a.push(encode::enc_str_x_imm(12, 11, OFF_TURN_RESUME_READY as u16));
    let no_waker = a.abs();
    a.patch_cbz_w(skip_no_waker, 11);
    for m in &to_after_remote {
        let this = a.start + m;
        let delta = (no_waker as i64 - this as i64) * 4;
        a.words[*m] = encode::enc_b(delta as i32);
    }
    debug_assert_eq!(no_waker, a.abs());
    if !xreply.is_empty() {
        // The remote arm's `BL` clobbered `x10`; the turn-record stores
        // below need it back. (No-op for every image with no remote arm.)
        a.load_imm(10, addrs.turn);
    }
    a.push(encode::enc_str_x_imm(31, 10, 0)); // busy = 0 (xzr)
    // waker = 0 (hygiene). Deliberately still ONE 64-bit `str xzr`: the two
    // `u32` fields plans/M10.md item 0c1 introduced (`waker_turn` at +32,
    // `waker_core` at +36) are the two halves of exactly this word, so one
    // store clears both. Two `str wzr` would be two words to say the same
    // thing, and `None` is 0 for both fields by the same niche convention.
    a.push(encode::enc_str_x_imm(31, 10, OFF_TURN_WAKER as u16));
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
/// plans/M6.md item F: once no actor reports "ran," this fn also tries
/// every group-child poll routine in fixed program order (`child_poll_starts`,
/// below) — a `g.start`ed child is never part of the round-robin cursor at
/// all (there is no admission-ordering fairness question between a
/// group's own children the way there is between actors' mailboxes;
/// `RuntimePlacement`'s own per-child free-turn area already gives each
/// one a fixed, unique poll site). The entry driver loops this between a
/// root turn's own suspend points; "nothing ready" with the root still
/// incomplete is the deadlock condition (`DEADLOCK_MSG`).
fn build_rt_run_one(
    select_starts: &[usize],
    child_poll_starts: &[usize],
    // plans/M8.md item C2: this core's own inbound-ring drain, when it has
    // any inbound lane. Called **first**, before selection: a message that
    // crossed a core boundary has to reach a mailbox before the FIFO order
    // 04 §2 promises can mean anything for it. `None` for every core with
    // no inbound ring — every core of every pre-C2 image — which emits not
    // one extra instruction.
    drain_start: Option<usize>,
    rr_cursor_addr: u64,
    start: usize,
) -> Asm {
    let mut a = Asm::new(start);
    a.push(encode::enc_sub_imm(31, 31, 16, true));
    a.push(encode::enc_str_x_imm(30, 31, 0));
    let n = select_starts.len();
    let mut to_out: Vec<usize> = Vec::new();
    if let Some(drain) = drain_start {
        // A drain that moved anything reports progress on its own: the
        // caller's loop comes straight back here, and the root turn's own
        // `resume_ready` re-check (the entry driver's loop) sees a reply
        // this drain just delivered. Bounded by ring occupancy, so this can
        // never spin.
        a.bl_to(drain);
        let skip = a.skip_placeholder(); // cbz x0, .continue
        to_out.push(a.skip_placeholder()); // b .out (x0 already holds 1)
        let cont = a.abs();
        a.patch_cbz(skip, 0);
        debug_assert_eq!(cont, a.abs());
    }
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
    for &poll in child_poll_starts {
        a.bl_to(poll);
        let skip_notran = a.skip_placeholder(); // cbz x0, .skip
        to_out.push(a.skip_placeholder());
        let skip_to = a.abs();
        a.patch_cbz(skip_notran, 0);
        debug_assert_eq!(skip_to, a.abs());
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

/// plans/M8.md item C2: one core's own **inbound ring drain**,
/// `rt_drain_<core>() -> x0 (1 = something moved, 0 = both lanes were
/// empty)`. 04 §2 puts one cooperative loop on each core "over the actors
/// placed there"; this is the step that turns a cross-core arrival into
/// something that loop can select, and it runs *inside* that loop rather
/// than in any interrupt or host callback — the only thing the far core did
/// was publish into memory and raise a word.
///
/// Order and shape:
///
/// 1. **Clear this core's own pending word first** (secondary cores only —
///    core 0's word belongs to `__wrela_checkpoint_service`, which already
///    clears it on the park-resume path). Clear-then-re-derive is
///    06 §5/04 §2's mask-arm-recheck: a wake that lands *during* the drain
///    re-raises the word, so the core does not sleep on it, and re-running
///    the drain is idempotent because readiness is read out of memory
///    every time.
/// 2. **Reply lanes**, one ring per sending core: write the reply word into
///    the destination turn record and set `resume_ready`. Both stores are
///    to memory this core owns the scheduling of, which is the point of
///    routing the reply through a ring at all.
/// 3. **Request lanes**, one ring per (sending core, target mailbox root):
///    hand each message to the *same* `__rt_enqueue_<Actor>` a same-core
///    send would have called, with the identical register ABI. If that
///    admission is **rejected** (the mailbox is full), the message is left
///    in the ring and this lane stops — back-pressure, never a drop. The
///    ring then fills, and the next sender is rejected at its own send site
///    exactly as a full mailbox rejects today (decision 29's own rule).
///
/// `x2` is the whole slot's argument-word count rather than the method's
/// own: a ring slot and the mailbox slot it feeds are the same size by
/// construction (`cross_core_rings`), so copying the full argument area is
/// in bounds at both ends and needs no per-method table on this path.
fn build_rt_drain(
    core: usize,
    // (ring addrs, capacity, slot size, that mailbox root's actor name —
    // M10 D: `bl_call_key(rt_enqueue_symbol(actor))` into the compiled body)
    request_lanes: &[(RingAddrs, u64, u64, String)],
    // (ring addrs, capacity)
    reply_lanes: &[(RingAddrs, u64)],
    // plans/M10.md item 0c1: see `build_rt_select_and_run`. The reply lane
    // is the one place this routine dereferences a `TurnId`.
    turns_base: u64,
    log2_stride: u8,
    start: usize,
) -> Asm {
    use crate::codegen::{OFF_TURN_REPLY, OFF_TURN_REPLY_TAG, OFF_TURN_RESUME_READY};
    let mut a = Asm::new(start);
    a.push(encode::enc_sub_imm(31, 31, 16, true));
    a.push(encode::enc_str_x_imm(30, 31, 0));
    a.push(encode::enc_str_x_imm(31, 31, 8)); // moved = 0 (xzr)

    if core != 0 {
        a.load_imm(9, wrela_machine::pending::core_word_addr(core));
        a.push(encode::enc_str_x_imm(31, 9, 0));
    }

    for (addrs, capacity) in reply_lanes {
        let top = a.abs();
        a.load_imm(9, addrs.count);
        a.push(encode::enc_ldr_x_imm(10, 9, 0));
        let skip_empty = a.skip_placeholder(); // cbz x10, .next
        a.load_imm(9, addrs.head);
        a.push(encode::enc_ldr_x_imm(11, 9, 0));
        a.load_imm(12, REPLY_SLOT_SIZE);
        a.push(encode::enc_mul(12, 11, 12, true));
        a.load_imm(13, addrs.ring);
        a.push(encode::enc_add_reg(13, 13, 12, true));
        // plans/M10.md item 0c1 / item J: word 0 is
        // `TurnId | (reply_tag << 32)`; x12 (the slot offset, dead from
        // here) is the scratch the index→address block borrows, and
        // `push_ring_advance` below reloads it anyway.
        a.push(encode::enc_ldr_x_imm(14, 13, 0)); // TurnId | (tag << 32)
        a.push(encode::enc_ldr_x_imm(15, 13, 8)); // reply word
        a.push(encode::enc_lsr_imm(16, 14, 32, true)); // x16 = reply_tag
        a.push(encode::enc_mov_reg(14, 14, false)); // w14 = TurnId (clear high)
        push_turn_addr_from_id(&mut a, 14, 12, turns_base, log2_stride);
        a.push(encode::enc_str_x_imm(16, 14, OFF_TURN_REPLY_TAG as u16));
        a.push(encode::enc_str_x_imm(15, 14, OFF_TURN_REPLY as u16));
        a.push(encode::enc_movz(16, 1, 0, true));
        a.push(encode::enc_str_x_imm(16, 14, OFF_TURN_RESUME_READY as u16));
        push_ring_advance(&mut a, 11, 12, addrs.head, *capacity);
        a.load_imm(9, addrs.count);
        a.push(encode::enc_ldr_x_imm(10, 9, 0));
        a.push(encode::enc_sub_imm(10, 10, 1, true));
        a.push(encode::enc_str_x_imm(10, 9, 0));
        a.push(encode::enc_movz(16, 1, 0, true));
        a.push(encode::enc_str_x_imm(16, 31, 8)); // moved = 1
        a.b_to(top);
        let next = a.abs();
        a.patch_cbz(skip_empty, 10);
        debug_assert_eq!(next, a.abs());
    }

    for (addrs, capacity, slot_size, actor) in request_lanes {
        // plans/M10.md item D0 (decision 610): the destination mailbox's
        // own `rt_enqueue` now takes its arguments **by value** in
        // `x1`/`x2`, so this lane loads them out of the request-ring slot
        // instead of handing over `slot + 16` as a pointer *into the
        // cross-core ring*. Same expression as both the enqueue's stores
        // and dispatch's loads, `.min(2)` included.
        //
        // Disclosed, not silent: this clamp is new **here**. This lane
        // used to pass an unclamped `(slot_size - 16) / 8`, so a request
        // ring whose slot reserves 3+ argument words would have had them
        // all copied — and then dispatch, which has always clamped to 2,
        // would never have loaded the rest. Unreachable today (no
        // reachable call site passes more than two words, enforced at
        // `codegen.rs`'s `emit_marshal_and_call`); unifying the two
        // expressions for real is item F2's job (decision 659).
        let arg_words = ((slot_size.saturating_sub(16)) / 8).min(2);
        let top = a.abs();
        a.load_imm(9, addrs.count);
        a.push(encode::enc_ldr_x_imm(10, 9, 0));
        let skip_empty = a.skip_placeholder(); // cbz x10, .next
        a.load_imm(9, addrs.head);
        a.push(encode::enc_ldr_x_imm(11, 9, 0));
        a.load_imm(12, *slot_size);
        a.push(encode::enc_mul(12, 11, 12, true));
        a.load_imm(13, addrs.ring);
        a.push(encode::enc_add_reg(13, 13, 12, true));
        a.push(encode::enc_ldr_x_imm(0, 13, 0)); // method_idx
        // plans/M10.md item 0c1: the waker travels as two `u32` fields, and
        // BOTH must be reloaded on every lap — `x4` left stale from a
        // previous lane would deliver this message's reply to the wrong
        // core, which is the one new failure mode the `x4` ABI introduces.
        a.push(encode::enc_ldr_w_imm(3, 13, 8)); // waker_turn
        a.push(encode::enc_ldr_w_imm(4, 13, 12)); // waker_core
        // Absent argument registers are zeroed rather than left holding
        // whatever the drain loop last put there: the destination
        // mailbox's slot may be wider than this ring's, in which case the
        // enqueue stores a register this lane never loaded.
        if arg_words >= 1 {
            a.push(encode::enc_ldr_x_imm(1, 13, 16)); // x1 = arg0
        } else {
            a.push(encode::enc_mov_reg(1, 31, true)); // mov x1, xzr
        }
        if arg_words >= 2 {
            a.push(encode::enc_ldr_x_imm(2, 13, 24)); // x2 = arg1
        } else {
            a.push(encode::enc_mov_reg(2, 31, true)); // mov x2, xzr
        }
        // M10 D / decision 615: compiled specialized body in `code`.
        a.bl_call_key(&crate::codegen::rt_enqueue_symbol(actor));
        // Rejected: the target mailbox is full. Leave the message in the
        // ring (back-pressure) and stop this lane — never a drop.
        let skip_full = a.skip_placeholder(); // cbnz x0, .next
        a.load_imm(9, addrs.head);
        a.push(encode::enc_ldr_x_imm(11, 9, 0));
        push_ring_advance(&mut a, 11, 12, addrs.head, *capacity);
        a.load_imm(9, addrs.count);
        a.push(encode::enc_ldr_x_imm(10, 9, 0));
        a.push(encode::enc_sub_imm(10, 10, 1, true));
        a.push(encode::enc_str_x_imm(10, 9, 0));
        a.push(encode::enc_movz(16, 1, 0, true));
        a.push(encode::enc_str_x_imm(16, 31, 8)); // moved = 1
        a.b_to(top);
        let next = a.abs();
        a.patch_cbz(skip_empty, 10);
        a.patch_cbnz(skip_full, 0);
        debug_assert_eq!(next, a.abs());
    }

    a.push(encode::enc_ldr_x_imm(0, 31, 8)); // x0 = moved
    a.push(encode::enc_ldr_x_imm(30, 31, 0));
    a.push(encode::enc_add_imm(31, 31, 16, true));
    a.push(encode::enc_ret(30));
    a
}

/// plans/M8.md item C1: one **secondary core's own entry block** (core
/// `core` in `1..RuntimeTables::cores`) — 06-machine.md §3's "enters the
/// per-core event loops", for the cores core 0 releases.
///
/// Deliberately the whole of what a secondary core does at C1, in eleven
/// instructions, with no call outside this same block:
///
/// 1. install this core's own stack pointer (`core_stack_base(core) +
///    CORE_STACK_SIZE` — the per-core state 06 §3 names first; every
///    codegen'd prologue already assumes `sp` is live);
/// 2. store this core's own bring-up mark (`machine_info::core_mark_addr`)
///    — the guest-written evidence the VMM checks at halt, and the one
///    thing that makes "core 1 executed" falsifiable rather than assumed;
/// 3. loop: run one tick of **this core's own** event loop
///    (`rt_run_one_core`, over exactly the actors placed here — 04 §2's
///    "one per core, over the actors placed there", no stealing, no
///    migration), and go again while it reports progress;
/// 4. when nothing on this core is ready, **park** — the ordinary trapping
///    store to `mmio::PARK_MMIO_ADDR`. The VMM deschedules this core until
///    its own pending word is raised; it never spins, never polls a wall
///    clock, and never gets the baton back on its own.
/// 5. on resume, branch straight back to the loop top: readiness is
///    re-derived from memory, so a wake decides nothing (the mask-arm-
///    recheck idempotency 04 §2 requires).
///
/// Two things this deliberately does **not** do at C1, each named rather
/// than silently absent. It does not call `__wrela_checkpoint_service`:
/// that routine lives in a different image section from this block on the
/// `layout_program` flavor, and nothing can raise a vector on a secondary
/// core until cross-core rings exist (item C2), so the call would be an
/// unreachable cross-section reloc bought on speculation. And it does not
/// consult `machine_info::OFF_NEXT_DEADLINE`: that word is core 0's park
/// deadline, and no turn can arm a deadline on a secondary core while no
/// message can reach one — item C2 gives a woken secondary both.
fn build_secondary_core_entry(core: usize, rt_run_one_core: usize, start: usize) -> Asm {
    let mut a = Asm::new(start);
    let sp_top = machine_layout::core_stack_base(core) + machine_layout::CORE_STACK_SIZE;
    a.load_imm(9, sp_top);
    a.push(encode::enc_add_imm(31, 9, 0, true)); // mov sp, x9

    a.load_imm(9, machine_info::core_mark_running(core));
    a.load_imm(10, machine_info::core_mark_addr(core));
    a.push(encode::enc_str_x_imm(9, 10, 0));

    let loop_top = a.abs();
    a.bl_to(rt_run_one_core);
    {
        // cbnz x0, .loop_top — a slice ran; try again before parking.
        let this = a.abs();
        let delta = (loop_top as i64 - this as i64) * 4;
        a.push(encode::enc_cbnz(0, delta as i32, true));
    }
    // Nothing ready on this core: park. The stored value is unread by the
    // VMM (`mmio::PARK_MMIO_ADDR`'s own contract) — the core index it
    // needs is the vCPU that trapped.
    a.load_imm(9, wrela_machine::mmio::PARK_MMIO_ADDR);
    a.load_imm(10, 0);
    a.push(encode::enc_str_x_imm(10, 9, 0));
    a.b_to(loop_top);
    a
}

/// One static `g.start` call site's own poll routine (item F #2): checks
/// its own callee's fixed free-turn area for `busy && suspended &&
/// resume_ready` and, if ready, resumes it (an ordinary `BL` to the
/// callee's own compiled entry — the fresh-vs-resume discriminant is the
/// callee's own job, `codegen::emit_async_entry`'s doc). `x0 -> 1` iff this
/// call made real progress (either a resumed slice ran, whether it went on
/// to suspend again or finished, or nothing here was ready at all reports
/// `x0 -> 0`) — `build_rt_run_one`'s own "did anything run this tick"
/// convention, shared with `rt_select_and_run`. On completion/cancellation:
/// writes this child's own `(tag, payload)` into the group arena
/// (`group_child_tag_off`/`group_child_payload_off` at `child_index`),
/// decrements `active_children`, clears the child's own `busy` (harvested —
/// available for a later loop iteration of the same `with`-site to reuse),
/// and — iff `active_children` reaches zero and a `join_waiter` is
/// registered — wakes it (`OFF_TURN_RESUME_READY = 1`, the identical
/// generic "something changed, re-check the root" signal the entry driver
/// already polls for). `child_turn_addr`/`group_arena_base` are real,
/// already-placed addresses (this fn is built twice, placeholder then
/// real, exactly like every other runtime-glue routine in this module).
#[allow(clippy::too_many_arguments)]
fn build_group_child_poll(
    child_turn_addr: u64,
    child_key: &str,
    group_arena_base: u64,
    child_index: usize,
    // plans/M10.md item 0c2: the two build-time constants
    // `push_turn_addr_from_id` needs to turn the `Option[TurnId]` a
    // `join_waiter` now is back into the address its `resume_ready` word
    // lives at.
    turns_base: u64,
    log2_stride: u8,
    start: usize,
) -> Asm {
    use crate::codegen::{
        GROUP_SLOT_SIZE, OFF_GROUP_ACTIVE_CHILDREN, OFF_GROUP_JOIN_WAITER, OFF_TURN_BUSY,
        OFF_TURN_RESUME_READY, OFF_TURN_SUSPENDED, TURN_RECORD_SIZE, TURN_STATUS_CANCELLED,
        TURN_STATUS_SUSPENDED, group_child_payload_off, group_child_tag_off,
    };
    let mut a = Asm::new(start);
    a.push(encode::enc_sub_imm(31, 31, 16, true));
    a.push(encode::enc_str_x_imm(30, 31, 0));

    let mut to_out: Vec<usize> = Vec::new(); // x0 already set; jump to epilogue.

    a.load_imm(9, child_turn_addr);
    a.push(encode::enc_ldr_x_imm(10, 9, OFF_TURN_BUSY as u16));
    let skip_a = a.skip_placeholder(); // cbz -> not ready
    a.push(encode::enc_ldr_x_imm(10, 9, OFF_TURN_SUSPENDED as u16));
    let skip_b = a.skip_placeholder(); // cbz -> not ready
    a.push(encode::enc_ldr_x_imm(10, 9, OFF_TURN_RESUME_READY as u16));
    let skip_c = a.skip_placeholder(); // cbz -> not ready

    // Ready: resume (x0 arbitrary — the resume path ignores incoming args).
    a.load_imm(0, 0);
    a.bl_call_key(child_key);
    a.push(encode::enc_cmp_imm(0, TURN_STATUS_SUSPENDED as u16, true));
    let skip_still_susp = a.skip_placeholder(); // b.eq -> still suspended (ran a slice, nothing to harvest yet)

    // Completed or cancelled: harvest into the group arena.
    a.push(encode::enc_cmp_imm(0, TURN_STATUS_CANCELLED as u16, true));
    a.push(encode::enc_cset(11, Cond::Eq, true)); // x11 = tag (0 Ok / 1 Cancelled)
    a.load_imm(12, child_turn_addr + TURN_RECORD_SIZE); // &this child's own Temp(0) (its ambient group)
    a.push(encode::enc_ldr_x_imm(13, 12, 0)); // x13 = group id, encoded (arena_index + 1)
    a.push(encode::enc_sub_imm(13, 13, 1, true));
    a.load_imm(14, GROUP_SLOT_SIZE);
    a.push(encode::enc_mul(13, 13, 14, true));
    a.load_imm(12, group_arena_base);
    a.push(encode::enc_add_reg(12, 12, 13, true)); // x12 = group addr
    a.push(encode::enc_str_x_imm(
        11,
        12,
        group_child_tag_off(child_index) as u16,
    ));
    a.push(encode::enc_str_x_imm(
        1,
        12,
        group_child_payload_off(child_index) as u16,
    ));
    a.push(encode::enc_ldr_x_imm(
        13,
        12,
        OFF_GROUP_ACTIVE_CHILDREN as u16,
    ));
    a.push(encode::enc_sub_imm(13, 13, 1, true));
    a.push(encode::enc_str_x_imm(
        13,
        12,
        OFF_GROUP_ACTIVE_CHILDREN as u16,
    ));
    a.load_imm(9, child_turn_addr);
    a.push(encode::enc_str_x_imm(31, 9, OFF_TURN_BUSY as u16)); // busy = 0 (harvested)

    let skip_still_active = a.skip_placeholder(); // cbnz x13 -> no wake yet
    // plans/M10.md item 0c2: `join_waiter` is an `Option[TurnId]` (a `u32`
    // at +48). `ldr w`/`cbz w` test exactly the four bytes the field
    // occupies — the 1-based niche (decision 567) keeps `cbz` meaning
    // "nobody waiting" — and the address the wake actually needs comes
    // from the one index→address rule.
    a.push(encode::enc_ldr_w_imm(10, 12, OFF_GROUP_JOIN_WAITER as u16));
    let skip_no_waiter = a.skip_placeholder(); // cbz w10 -> nothing waiting
    push_turn_addr_from_id(&mut a, 10, 11, turns_base, log2_stride);
    a.load_imm(11, 1);
    a.push(encode::enc_str_x_imm(11, 10, OFF_TURN_RESUME_READY as u16));
    let no_wake = a.abs();
    a.patch_cbnz(skip_still_active, 13);
    a.patch_cbz_w(skip_no_waiter, 10);
    debug_assert_eq!(no_wake, a.abs());
    a.push(encode::enc_movz(0, 1, 0, true)); // ran
    to_out.push(a.skip_placeholder());

    let still_susp = a.abs();
    a.patch_cond(skip_still_susp, Cond::Eq);
    debug_assert_eq!(still_susp, a.abs());
    a.push(encode::enc_movz(0, 1, 0, true)); // ran a slice, still parked
    to_out.push(a.skip_placeholder());

    let not_ready = a.abs();
    a.patch_cbz(skip_a, 10);
    a.patch_cbz(skip_b, 10);
    a.patch_cbz(skip_c, 10);
    debug_assert_eq!(not_ready, a.abs());
    a.push(encode::enc_movz(0, 0, 0, true)); // nothing to do

    let epilogue = a.abs();
    for m in &to_out {
        let this = a.start + m;
        let delta = (epilogue as i64 - this as i64) * 4;
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
    /// Core 0's own `rt_run_one` absolute word index (the entry driver's
    /// scheduler-tick target). Present whenever any glue exists at all.
    rt_run_one_start: usize,
    /// plans/M8.md item C1: each secondary core's own entry block
    /// (`build_secondary_core_entry`), absolute word index, in core order
    /// `1..tables.cores`. Empty for every single-core image — which is
    /// what keeps their bytes unchanged.
    core_entry_starts: Vec<usize>,
}

fn build_runtime_glue_block(
    tables: &RuntimeTables,
    actor_dispatch: &[(String, Vec<(String, bool, bool)>)],
    placement: &RuntimePlacement,
    // plans/M8.md item C1: each actor's own core, in `tables.actors`
    // order — the report's Placement section, consumed as the *only*
    // assignment (shape decision 2: never a second truth). Every entry is
    // `0` for a single-core image.
    actor_cores: &[usize],
    // plans/M6.md item F: every static `g.start` call site's own
    // `(callee_key, child_index)` — `BootCtx::group_child_index`, sorted
    // (`BTreeMap`'s own iteration order, CLAUDE.md's determinism rule) so
    // poll-routine placement never depends on hash order.
    group_child_index: &BTreeMap<String, usize>,
    start: usize,
) -> RuntimeGlue {
    let mut asms = Vec::new();
    let mut symbols = BTreeMap::new();
    // plans/M8.md item D: one loop over every mailbox root — each declared
    // actor, then each messageable `@driver`, in `mailbox_root_names`'
    // order (which `actor_dispatch` is built in). A messageable driver
    // reaches the *identical* `build_rt_enqueue` and
    // `build_rt_select_and_run_symbolic`, so there is exactly one admission
    // routine shape and one dispatch routine shape in the machine; nothing
    // below can tell an actor from a driver, which is the point.
    let mut roots: Vec<(&str, &ActorAddrs, u64, u64, u64)> =
        Vec::with_capacity(tables.actors.len() + tables.drivers.len());

    for (i, a) in tables.actors.iter().enumerate() {
        roots.push((
            a.name.as_str(),
            &placement.actors[i],
            a.mailbox_capacity,
            a.slot_size,
            a.frame_size,
        ));
    }
    for (i, d) in tables.drivers.iter().enumerate() {
        let (Some(mb), Some(addrs)) = (&d.mailbox, placement.driver_mailboxes.get(&i)) else {
            continue;
        };
        roots.push((
            d.name.as_str(),
            addrs,
            mb.capacity,
            mb.slot_size,
            mb.frame_size,
        ));
    }
    let mut select_starts = Vec::with_capacity(roots.len());
    let mut cursor = start;
    // --- plans/M8.md item C2: the cross-core ring routines --------------
    //
    // Emitted **before** the per-actor pairs below, for one mechanical
    // reason: a selected turn's delivery arm calls `__rt_xreply_*`, and a
    // local `BL` needs its target's word index already fixed. An image with
    // no cross-core edge emits nothing here at all, so every pre-C2 image's
    // per-actor pairs still start at `start` and every pinned byte holds
    // (decision 12's own "emits nothing" shape, kept).
    //
    // `xreply_by_producer[d]` = the `(remote core, routine)` list an actor
    // placed on core `d` needs; `request_lanes[dst]` = what core `dst`'s
    // own drain consumes.
    let mut xreply_by_producer: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    let mut request_lanes: BTreeMap<usize, Vec<(RingAddrs, u64, u64, String)>> = BTreeMap::new();
    let mut reply_lanes: BTreeMap<usize, Vec<(RingAddrs, u64)>> = BTreeMap::new();
    for (ri, ring) in tables.rings.iter().enumerate() {
        let addrs = placement.rings[ri];
        match ring.kind {
            RingKind::Reply => {
                let start_here = cursor;
                let asm = build_rt_xreply(&addrs, ring.capacity, ring.dst, start_here);
                cursor += asm.words.len();
                asms.push(asm);
                xreply_by_producer
                    .entry(ring.src)
                    .or_default()
                    .push((ring.dst, start_here));
                reply_lanes
                    .entry(ring.dst)
                    .or_default()
                    .push((addrs, ring.capacity));
            }
            RingKind::Request => {
                let enqueue_start = cursor;
                let enqueue_words =
                    build_ring_enqueue(&addrs, ring.capacity, ring.slot_size, enqueue_start);
                cursor += enqueue_words.len();
                asms.push(Asm {
                    start: enqueue_start,
                    words: enqueue_words,
                    relocs: Vec::new(),
                });
                let xsend_start = cursor;
                let asm = build_rt_xsend(enqueue_start, ring.src, ring.dst, xsend_start);
                cursor += asm.words.len();
                asms.push(asm);
                let actor = ring.actor.clone().unwrap_or_default();
                symbols.insert(xsend_symbol(ring.src, &actor), xsend_start);
                request_lanes.entry(ring.dst).or_default().push((
                    addrs,
                    ring.capacity,
                    ring.slot_size,
                    actor,
                ));
            }
        }
    }

    for (i, (_name, addrs, capacity, slot_size, frame_size)) in roots.iter().enumerate() {
        let (_, dispatch_keys) = &actor_dispatch[i];

        // M10 D / decision 615: per-actor mailbox admission lives in the
        // `code` section (`emit_rt_enqueue` under `rt_enqueue_symbol`); do
        // not place a hand-asm twin into glue_symbols. Cross-core request
        // rings still emit `build_ring_enqueue` above for `xsend` (F2).
        let select_start = cursor;
        // plans/M8.md item C2: only an actor whose own core produces
        // replies for another core carries the remote-waker arm.
        let empty: Vec<(usize, usize)> = Vec::new();
        let my_core = actor_cores.get(i).copied().unwrap_or(0);
        let xreply = xreply_by_producer.get(&my_core).unwrap_or(&empty);
        let select_asm = build_rt_select_and_run_symbolic(
            addrs,
            *capacity,
            *slot_size,
            dispatch_keys,
            *frame_size,
            xreply,
            placement.turns_base,
            placement.log2_turn_stride(),
            select_start,
        );
        cursor += select_asm.words.len();
        select_starts.push(select_start);
        asms.push(select_asm);
    }
    let mut child_poll_starts = Vec::with_capacity(group_child_index.len());
    for (callee_key, &child_index) in group_child_index {
        let Some(&child_turn_addr) = placement.free_turns.get(callee_key) else {
            // A callee this pass never sized a free-turn area for — an
            // internal inconsistency (`compute_group_child_indices` and
            // `RuntimeTables::free_turns` must agree on every async fn key);
            // skip rather than panic, `layout_program`'s own reloc
            // resolution catches the real underlying disagreement loudly.
            continue;
        };
        let poll_start = cursor;
        let poll_asm = build_group_child_poll(
            child_turn_addr,
            callee_key,
            placement.group_arena,
            child_index,
            placement.turns_base,
            placement.log2_turn_stride(),
            poll_start,
        );
        cursor += poll_asm.words.len();
        child_poll_starts.push(poll_start);
        asms.push(poll_asm);
    }
    // plans/M8.md item C1: one `rt_run_one` per live core, each scanning
    // exactly the actors placed on it (04 §2: "one per core, over the
    // actors placed there"). With `cores == 1` every actor is on core 0 by
    // the single-core floor, so core 0's routine is word-for-word the one
    // this fn emitted before C1 — that identity is what keeps every M5-M7
    // boot transcript byte-identical, and it is asserted by the goldens
    // rather than assumed here.
    //
    // Group child polls stay on core 0: a `with group(...)` child is a
    // free turn, and the only free turns that run are the root test turn's
    // own, which is core 0's (06 §3: boot and the root turns are the entry
    // core's). Item C2 revisits this the moment a turn can run elsewhere.
    // plans/M8.md item C2: one inbound-ring drain per core that has any
    // inbound lane, placed after the per-actor enqueue routines it calls.
    let mut drain_starts: BTreeMap<usize, usize> = BTreeMap::new();
    for core in 0..tables.cores {
        let empty_req: Vec<(RingAddrs, u64, u64, String)> = Vec::new();
        let empty_rep: Vec<(RingAddrs, u64)> = Vec::new();
        let reqs = request_lanes.get(&core).unwrap_or(&empty_req);
        let reps = reply_lanes.get(&core).unwrap_or(&empty_rep);
        if reqs.is_empty() && reps.is_empty() {
            continue;
        }
        let resolved: Vec<(RingAddrs, u64, u64, String)> = reqs
            .iter()
            .filter(|(_, _, _, actor)| roots.iter().any(|(n, ..)| n == actor))
            .map(|(addrs, cap, slot, actor)| (*addrs, *cap, *slot, actor.clone()))
            .collect();
        let start_here = cursor;
        let asm = build_rt_drain(
            core,
            &resolved,
            reps,
            placement.turns_base,
            placement.log2_turn_stride(),
            start_here,
        );
        cursor += asm.words.len();
        drain_starts.insert(core, start_here);
        asms.push(asm);
    }
    let mut rt_run_one_starts = Vec::with_capacity(tables.cores);
    for core in 0..tables.cores {
        let core_selects: Vec<usize> = select_starts
            .iter()
            .enumerate()
            .filter(|(i, _)| actor_cores.get(*i).copied().unwrap_or(0) == core)
            .map(|(_, &s)| s)
            .collect();
        let core_polls: &[usize] = if core == 0 { &child_poll_starts } else { &[] };
        let start_here = cursor;
        let run_one_asm = build_rt_run_one(
            &core_selects,
            core_polls,
            drain_starts.get(&core).copied(),
            placement.rr_cursors[core],
            start_here,
        );
        cursor += run_one_asm.words.len();
        rt_run_one_starts.push(start_here);
        asms.push(run_one_asm);
    }
    // Each secondary core's own entry block, after every routine it calls.
    let mut core_entry_starts = Vec::new();
    for core in 1..tables.cores {
        let start_here = cursor;
        let entry_asm = build_secondary_core_entry(core, rt_run_one_starts[core], start_here);
        cursor += entry_asm.words.len();
        core_entry_starts.push(start_here);
        asms.push(entry_asm);
    }
    let _ = cursor;
    RuntimeGlue {
        asms,
        symbols,
        rt_run_one_start: rt_run_one_starts[0],
        core_entry_starts,
    }
}

/// plans/M6.md decision 11b (02-language.md §12.2): resolves every
/// runtime test's own declared `Actor[T]` params against the image
/// graph's own declared instances — `T`'s *unique* instance across
/// `graph.actors`/`graph.drivers` (both are actor roots, 02 §9.1: "A
/// struct marked `@actor` ... or `@driver` ... is an actor"), by build-
/// time index (04-compiler.md §6's own "Actor as-if" license: a handle's
/// runtime value is just that instance's own build-time-constant index).
/// Zero or more than one instance is a named `error[build]` line listing
/// every candidate (`actor#i`/`driver#i`, the identical spelling
/// `eval::image::dump`'s own edge lines already use) — sema
/// (`check_runtime_test_params`) already guarantees every param here is
/// a plain `Actor[T]` handle, so the only failure mode left is a real
/// ambiguity/absence in *this* image. A test with no params (every sync
/// test, and any async test that declares none) resolves to an empty arg
/// list — byte-identical to every pre-decision-11b test.
pub fn resolve_runtime_test_args(
    program: &crate::sema::typed::TypedProgram,
    runtime_tests: &[String],
    graph: &crate::eval::image::ImageGraph,
) -> Result<BTreeMap<String, Vec<u64>>, String> {
    let mut out = BTreeMap::new();
    for name in runtime_tests {
        let f = &program.fns[name];
        let mut args = Vec::with_capacity(f.params.len());
        for p in &f.params {
            let crate::sema::types::Type::Named(_, targs) = &p.ty else {
                return Err(format!(
                    "internal error: runtime test `{name}`'s own param `{}` is not an \
                     `Actor[T]` handle (sema should have already rejected this)",
                    p.name
                ));
            };
            let Some(crate::sema::types::TypeArg::Type(inner)) = targs.first() else {
                return Err(format!(
                    "internal error: runtime test `{name}`'s own `Actor[T]` param `{}` has no \
                     type argument",
                    p.name
                ));
            };
            let target_name = crate::sema::types::render_type(inner);
            let space = HandleSpace::from_graph(graph);
            let mut candidates: Vec<String> = Vec::new();
            let mut actor_index: Option<usize> = None;
            for (i, a) in graph.actors.iter().enumerate() {
                if crate::sema::types::render_type(&a.actor_type) == target_name {
                    candidates.push(format!("actor#{i}"));
                    actor_index = Some(i);
                }
            }
            // plans/M8.md item D: a `@driver` declared with `mailbox=` is a
            // messageable actor root, so a runtime test may hold its handle
            // like any other. A driver *without* one still resolves as a
            // candidate — that is how the count check above stays honest —
            // but produces the named floor below rather than an index.
            // plans/M8.md item H attack 6: the handle word shares one index
            // space with every other `ImageDecl` (`image_decl_handle_word`).
            let mut driver_index: Option<usize> = None;
            for (i, d) in graph.drivers.iter().enumerate() {
                if crate::sema::types::render_type(&d.actor_type) == target_name {
                    candidates.push(format!("driver#{i}"));
                    if d.args.iter().any(|a| a.label == "mailbox") {
                        driver_index = Some(i);
                    }
                }
            }
            if candidates.len() != 1 {
                return Err(format!(
                    "runtime test `{name}`'s own parameter `{}: Actor[{target_name}]` needs \
                     exactly one declared `{target_name}` instance in this image; found {} ({})",
                    p.name,
                    candidates.len(),
                    if candidates.is_empty() {
                        "none".to_string()
                    } else {
                        candidates.join(", ")
                    }
                ));
            }
            let Some(idx) = actor_index
                .and_then(|i| {
                    image_decl_handle_word(space, &crate::eval::image::ImageDeclRef::Actor(i))
                })
                .or_else(|| {
                    driver_index.and_then(|i| {
                        image_decl_handle_word(space, &crate::eval::image::ImageDeclRef::Driver(i))
                    })
                })
            else {
                return Err(format!(
                    "runtime test `{name}`'s own parameter `{}: Actor[{target_name}]` resolves \
                     to a `@driver` declared with no `mailbox=` — a driver is messageable only \
                     when its declaration says so (05-library.md §9), so there is nothing for \
                     this handle to call. Add `mailbox=n` to `img.driver({target_name}, ...)`",
                    p.name
                ));
            };
            args.push(idx);
        }
        out.insert(name.clone(), args);
    }
    Ok(out)
}

/// plans/M6.md item D, completed by plans/M7.md item W: the real boot
/// sequence's own actor-state half — every actor's own state slot
/// zero-initialized, then every declared `init` called with its declared
/// arguments, before any root turn runs (`build_entry_driver`'s own
/// `bl_to(boot_init_start)`, right after the console/test-counter zeroing
/// it already did).
///
/// **Two passes, not one interleaved walk**, and that is the point:
/// every actor's whole state is defined before *any* `init` body runs, so
/// an `init` can be handed another actor's handle (item W's own
/// `Value::ImageDecl` argument) without depending on which declaration
/// order the image happened to use. The word count is identical either
/// way — this is a sequencing guarantee, not a size change — and no
/// pinned golden's bytes move for it, because no golden dumps this
/// fragment (the report pins section *sizes*, which are unchanged for
/// every image whose `init`s take no arguments).
///
/// The arguments themselves are `build_boot_init_calls`'s product: word
/// `i` goes to `x{i+1}`, `x0` is the actor's own state address (the
/// receiver, by pointer). That convention is `codegen::emit_prologue`'s,
/// derived rather than assumed — `BootInitCall`'s own doc comment has the
/// derivation. Item W's own doc comment on `ActorInit` records what this
/// used to be (a zero-argument-only call, and a rejection for everything
/// else) and why it is not that any more.
/// `abort_fixed_local`: when `Some(abs_word)`, a fallible `init`'s `Err`
/// path `bl_to`s that harness-local `__wrela_abort` (the test image —
/// same section, absolute word index already known). When `None`, the
/// path emits `Reloc::AbortFixed` instead (the ordinary `layout_program`
/// image, whose abort lives in a different section). Either way the
/// guest loads the interned message into `x0`/`x1` first; the test
/// harness abort prints it, and the build-path stub ignores it and
/// exits — the same contract an `assert` failure inside `init` already
/// has on each flavor.
#[allow(clippy::too_many_arguments)]
fn build_boot_init(
    actor_addrs: &[ActorAddrs],
    driver_addrs: &[u64],
    state_sizes: &[u64],
    driver_state_sizes: &[u64],
    init_calls: &[Option<BootInitCall>],
    driver_init_calls: &[Option<BootInitCall>],
    device_regs: &[DeviceRegs],
    pools: &[PoolPlacement],
    start: usize,
    abort_fixed_local: Option<usize>,
) -> Result<Asm, LayoutError> {
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
    // Every state slot is zero-filled first — actors, then drivers, in
    // 06-machine.md §3's own "runs typed driver and actor initialization
    // in image dependency order" spirit, applied to the one ordering fact
    // this machine has: nothing may read a field before its `init` runs,
    // so every slot is defined before any `init` is called.
    let state_slots = actor_addrs
        .iter()
        .map(|a| a.state)
        .zip(state_sizes.iter().copied())
        .chain(
            driver_addrs
                .iter()
                .copied()
                .zip(driver_state_sizes.iter().copied()),
        );
    for (state, size) in state_slots {
        let mut w = 0u64;
        while w < size {
            a.load_imm(9, state + w);
            a.push(encode::enc_str_x_imm(31, 9, 0)); // store xzr (unit is Copy/all-zero-valid)
            w += 8;
        }
    }
    // plans/M7.md item H1: **drivers first.** 06 §3 step 3 is explicit —
    // "runs typed driver and actor initialization in image dependency
    // order" — and a driver is the root of that order by construction: an
    // actor may hold an `Actor[Driver]` handle (`golden/appliance`'s own
    // cache actor does), and no driver may hold an actor's anything (03 §1:
    // "a driver may export safe actor APIs but never raw capabilities").
    let calls = driver_addrs
        .iter()
        .copied()
        .zip(driver_init_calls)
        .chain(actor_addrs.iter().map(|a| a.state).zip(init_calls));
    for (state, call) in calls {
        let Some(call) = call else { continue };
        // plans/M7.md item E4: `[own; N]` args build a temporary table on
        // this stack frame; free it after the call (and after any fallible
        // reply slot, which is nested inside).
        let mut array_stack: u64 = 0;
        for (i, arg) in call.args.iter().enumerate() {
            array_stack += emit_boot_init_arg(&mut a, i as u8 + 1, arg, device_regs, pools)?;
        }
        a.load_imm(0, state);
        // plans/M7.md item E1: a fallible `init` returns
        // `Result[unit, BootError]` through `x8` (this machine's aggregate
        // return pointer). Stage 16 bytes on the stack, point `x8` at them,
        // call, then check the tag — `Err` is image-fatal with a
        // diagnosable line through the **same** `__wrela_abort` path an
        // `assert` failure inside `init` already uses (plans/M6.md
        // decision 12 / plans/M7.md decision 8; H1 arms
        // `OFF_TEST_CONTINUATION` before boot so the landing pad works).
        if call.fallible {
            let (msg_off, msg_len) = call.err_msg.ok_or_else(|| {
                LayoutError::new(format!(
                    "internal error: fallible `{}` has no interned abort message — \
                     `intern_fallible_init_abort_messages` must run before assembly",
                    call.key
                ))
            })?;
            a.push(encode::enc_sub_imm(31, 31, 16, true)); // reply slot
            a.push(encode::enc_add_imm(8, 31, 0, true)); // mov x8, sp
            a.bl_call_key(&call.key);
            // Load tag from [sp]; RESULT_OK == 0.
            a.push(encode::enc_ldr_x_imm(9, 31, 0));
            a.push(encode::enc_add_imm(31, 31, 16, true)); // drop reply slot
            // cbz x9, ok — skip the abort when tag == 0.
            let ok_fixup = a.skip_placeholder();
            // On Err: guest emits its own FAILED line via `__wrela_abort`
            // (never a host-invented transcript). Message names which
            // `init` failed; the BootError variant is not recovered.
            a.load_rodata_addr_at(0, msg_off);
            a.load_imm(1, msg_len as u64);
            if let Some(abort_abs) = abort_fixed_local {
                a.bl_to(abort_abs);
            } else {
                let w = a.abs();
                a.push(encode::enc_bl(0));
                a.relocs.push(Reloc::AbortFixed { word: w });
            }
            a.patch_cbz(ok_fixup, 9);
        } else {
            a.bl_call_key(&call.key);
        }
        if array_stack > 0 {
            // Immediate ADD only reaches 12 bits unsigned; handle tables
            // for M7's working images stay well under that (CACHE_BLOCKS=64
            // → 512 bytes). Fail closed rather than emit a wrong free.
            if array_stack >= 4096 {
                return Err(LayoutError::new(format!(
                    "internal error: own-handle array stack frame is {array_stack} bytes; the \
                     unsigned-immediate ADD encoder reaches 4095"
                )));
            }
            a.push(encode::enc_add_imm(31, 31, array_stack as u16, true));
        }
    }
    a.push(encode::enc_ldr_x_imm(30, 31, 0));
    a.push(encode::enc_add_imm(31, 31, 16, true));
    a.push(encode::enc_ret(30));
    Ok(a)
}

/// Emit one boot `init` argument into `reg`. Returns stack bytes allocated
/// for an `[own; N]` table (0 for every other shape).
fn emit_boot_init_arg(
    a: &mut Asm,
    reg: u8,
    arg: &BootInitArg,
    regs: &[DeviceRegs],
    pools: &[PoolPlacement],
) -> Result<u64, LayoutError> {
    match arg {
        BootInitArg::OwnHandleArray {
            pool,
            count,
            slot_bytes,
        } => {
            let pool_base = pools
                .iter()
                .find(|p| &p.backing.name == pool)
                .map(|p| p.base)
                .ok_or_else(|| {
                    LayoutError::new(format!(
                        "internal error: boot builds an own-handle array for pool `{pool}`, which \
                         has no placed backing"
                    ))
                })?;
            let raw = count.checked_mul(8).ok_or_else(|| {
                LayoutError::new("own-handle array byte count overflow".to_string())
            })?;
            // AAPCS64: SP must stay 16-byte aligned.
            let bytes = ((raw + 15) / 16) * 16;
            if bytes == 0 || bytes >= 4096 {
                return Err(LayoutError::new(format!(
                    "internal error: own-handle array for pool `{pool}` wants {bytes} bytes \
                     (count={count}); boot's unsigned-immediate SUB reaches 4095"
                )));
            }
            a.push(encode::enc_sub_imm(31, 31, bytes as u16, true));
            for i in 0..*count {
                a.load_imm(SCRATCH_A, pool_base + i * *slot_bytes);
                a.push(encode::enc_str_x_imm(SCRATCH_A, 31, (i * 8) as u16));
            }
            a.push(encode::enc_add_imm(reg, 31, 0, true)); // mov reg, sp
            Ok(bytes)
        }
        other => {
            a.load_imm(reg, other.resolve(regs, pools)?);
            Ok(0)
        }
    }
}

// ===========================================================================
// plans/M6.md item F/G follow-up (the found-and-fixed `layout_program`
// defect): the runtime machinery, derived and assembled **once**, for both
// image flavors.
//
// Until this landed, only `layout_test_image` built the per-actor
// `__rt_enqueue_*`/`rt_select_and_run` glue, the group-child poll routines,
// `rt_run_one` and the boot-init routine. `layout_program` — the path
// `wrela build`/`wrela dump --stage=report` take — reserved `rtdata` but
// emitted none of the code that addresses it, so the first `.wr` image that
// actually *messaged* an actor (any `await`/`send` through an `Actor[T]`
// handle, which codegen lowers to a `Reloc::Call` at the symbolic
// `codegen::rt_enqueue_symbol` name) died in reloc resolution with
// `internal error: call target `__rt_enqueue_X` was never codegen'd` — an
// internal-error guard on a plainly user-reachable path. `tests/golden/
// appliance` never caught it because its actors are declared and never
// messaged, so no such `Reloc::Call` is ever emitted.
//
// The rule (item C's own, restated): the runtime tables **and** the routines
// that address them are part of the image, tests or not. So both paths now
// derive their inputs through `RuntimeWiring::derive` and assemble the exact
// same words through `build_runtime_block`. The only thing that legitimately
// differs is the entry driver — `layout_test_image`'s real console harness +
// test roots vs. `layout_program`'s `build_entry_stub` placeholder, which
// still halts with `EXIT_CODE_NO_RUNTIME` and therefore never calls any of
// this. That the block is unreachable in a `wrela build` image today is the
// identical, already-recorded position `rtdata`'s own reservation takes
// (`layout_program`'s doc): it is *there* because it is part of the image,
// not because anything executes it yet. The moment a real non-test image
// entry exists (M7+), it is one `bl_to(boot_init_start)` away, byte-for-byte
// the same machinery `wrela test` already boots for real.

/// Every whole-build fact the runtime block needs, derived once from a
/// `BootCtx` so the two image flavors can never disagree about an actor's
/// dispatch keys, its `init`, or the group-child index. `None` means "this
/// build has no actor runtime at all" (no `@actor` declaration, or tables
/// that size to zero bytes) — the overwhelming majority of today's corpus,
/// for which both paths stay byte-identical to their pre-M6 behavior.
struct RuntimeWiring {
    tables: RuntimeTables,
    /// Per actor, in `tables.actors` order: its own name and its `pub`
    /// method dispatch keys (`"{Actor}.{method}"`, `program.fns` keys) with
    /// each one's asyncness and (plans/M7.md item Z1) whether its declared
    /// reply is an aggregate.
    dispatch: Vec<(String, Vec<(String, bool, bool)>)>,
    /// Per declared actor *instance*, in `tables.actors` order: the boot
    /// `init` call to make for it, or `None` if its struct declares no
    /// `init` at all (plans/M7.md item W, `build_boot_init_calls`).
    ///
    /// Per instance rather than per struct name, because the arguments
    /// come from the *declaration* (`ActorDecl::args`) and not from the
    /// struct: two `img.actor(Same, ...)` calls are two calls with two
    /// argument lists.
    init_calls: Vec<Option<BootInitCall>>,
    /// plans/M7.md item H1: the same, per declared `@driver` instance, in
    /// `tables.drivers` order.
    driver_init_calls: Vec<Option<BootInitCall>>,
    state_sizes: Vec<u64>,
    driver_state_sizes: Vec<u64>,
    group_child_index: BTreeMap<String, usize>,
    /// plans/M8.md item C1: each actor instance's own core, in
    /// `tables.actors` (= `ImageGraph::actors`) order — read straight off
    /// the report's own Placement table (`placement::place`), never
    /// re-derived here. Shape decision 2: the report's assignment *is* the
    /// runtime's assignment, or there are two truths.
    actor_cores: Vec<usize>,
    /// The whole placement table, kept for the cross-core edge check both
    /// image flavors run during reloc resolution.
    placement: crate::placement::PlacementTable,
}

impl RuntimeWiring {
    /// One derivation for both image flavors, with no flavor-conditional
    /// behavior in it at all. plans/M6.md item F/G's own found-and-fixed
    /// defect (this module's block comment above) is exactly that the
    /// runtime block **is** part of the image, tests or not, so the day
    /// `layout_program` grows a real entry it must find the identical boot
    /// sequence `wrela test` already boots. plans/M7.md item W removed the
    /// one exception that had grown back: a `reject_parameterized_init`
    /// flag, set only by `layout_test_image`, that made a parameterized
    /// `init` a build error on the path that boots and a silent no-op on
    /// the path that does not. Both paths now materialize the same
    /// arguments and fail closed on the same shapes.
    fn derive(
        boot: &BootCtx,
        // plans/M8.md item C2: the compiled program, for the one fact
        // placement cannot supply — which `send`/`await` sites actually
        // exist, and therefore which cross-core edges this image has rings
        // for. Both image flavors pass their own, so neither can end up
        // with a ring set the other does not have.
        program: &CodegenProgram,
    ) -> Result<Option<RuntimeWiring>, LayoutError> {
        let Some(mut tables) =
            compute_runtime_tables(boot.graph, boot.modules, boot.layout_ctx, boot.async_frames)
                .map_err(LayoutError::new)?
                .filter(|t| t.total_bytes > 0)
        else {
            return Ok(None);
        };
        // plans/M8.md item C1: placement first — it decides how many cores
        // this image brings up, which stripes the scheduler tables before
        // anything is placed or emitted against them.
        let placement = crate::placement::place(boot.graph, boot.modules, boot.layout_ctx)
            .map_err(LayoutError::new)?;
        tables.stripe_for_cores(placement.cores);
        // plans/M8.md item C1's second fail-closed arm. A `@driver`'s ISR
        // and `@task` bottom half are emitted into the **checkpoint
        // service**, which only core 0's entry driver and core 0's compiled
        // code ever call; its `init` likewise runs in core 0's boot
        // sequence. So a driver inferred or annotated onto a secondary core
        // would be a second truth of exactly the shape shape decision 2
        // forbids — the report saying `core=2` while every one of that
        // driver's own instructions runs on core 0. 04 §3 is explicit
        // ("a `@driver`'s vectors, pools, permits, and recovery lanes live
        // on its core; there is no cross-core hardware state"), so this is
        // refused rather than approximated. Lifting it is item C2's
        // per-core checkpoint work, not a silent demotion here.
        for (i, d) in tables.drivers.iter().enumerate() {
            let core = placement
                .core_of(&crate::eval::image::ImageDeclRef::Driver(i))
                .unwrap_or(0);
            if core != 0 {
                return Err(LayoutError::new(format!(
                    "driver#{i} (`{}`) is placed on core {core}, but a `@driver`'s ISR, `@task` \
                     bottom half and boot `init` all run in core 0's checkpoint service and boot \
                     sequence — plans/M8.md item C1 brings up secondary cores for actors only. \
                     Place this driver on core 0 (`core=0`), or wait for item C2's per-core \
                     device lanes",
                    d.name
                )));
            }
        }
        // plans/M8.md item C2, on top of item D: one entry per **mailbox
        // root**, in `mailbox_root_names` order — every declared actor,
        // then every messageable `@driver`. A driver's entry is always 0:
        // shape decision 2 keeps `@driver` on core 0 and the arm just above
        // refuses anything else, so a messageable driver is only ever a
        // ring *destination*, never a ring source, and 04 §3's "a
        // `@driver`'s vectors, pools, permits, and recovery lanes live on
        // its core" is not in tension — a ring slot carries a method index,
        // a waker and that method's own argument words, and nothing else.
        let mut actor_cores: Vec<usize> = (0..tables.actors.len())
            .map(|i| {
                placement
                    .core_of(&crate::eval::image::ImageDeclRef::Actor(i))
                    .unwrap_or(0)
            })
            .collect();
        actor_cores.extend(
            tables
                .drivers
                .iter()
                .filter(|d| d.mailbox.is_some())
                .map(|_| 0),
        );
        let shapes = merge_actor_pub_methods(boot.modules, boot.layout_ctx)?;
        // plans/M8.md item D: dispatch tables are per *mailbox root*, in
        // `mailbox_root_names`' order — the same order
        // `build_runtime_glue_block` walks. A messageable driver's methods
        // are numbered by the identical `merge_actor_pub_methods` shapes
        // `actor_method_index_tables` hands codegen, so an admitted method
        // index means the same thing on both sides.
        let dispatch = mailbox_root_names(&tables)
            .into_iter()
            .map(|name| {
                let methods = shapes.get(&name).cloned().unwrap_or_default();
                let keys = methods
                    .iter()
                    .map(|m| {
                        (
                            format!("{name}.{}", m.name),
                            m.is_async,
                            m.reply_is_aggregate,
                        )
                    })
                    .collect();
                (name, keys)
            })
            .collect();
        // Every rejection this pass can still make lives in here, keyed on
        // the shape boot genuinely cannot marshal rather than on "declares
        // parameters at all" (`build_boot_init_calls`'s own doc comment
        // lists them). Derived against the *declared* actor set, never
        // against every struct in the closure — an `init` on a plain data
        // struct is ordinary, legal code (`Pair.init(lo, hi)` in
        // `golden/boot-actor-reply-struct`) and is none of this pass's
        // business.
        let layouts = closure_layout_types(boot.modules)?;
        let backings =
            crate::eval::image_checks::pool_backings(boot.graph, &layouts).map_err(|e| {
                LayoutError::new(format!(
                    "internal error: a pool declaration this image's own graph check accepted \
                     cannot be read for own-handle materialization: {}",
                    e.message
                ))
            })?;
        let (init_calls, driver_init_calls) =
            build_boot_init_calls(boot.graph, &actor_inits(boot.modules)?, &backings)?;
        debug_assert_eq!(
            init_calls.len(),
            tables.actors.len(),
            "one boot `init` call per declared actor instance"
        );
        debug_assert_eq!(
            driver_init_calls.len(),
            tables.drivers.len(),
            "one boot `init` call per declared driver instance"
        );
        let state_sizes = tables.actors.iter().map(|a| a.state_size).collect();
        let driver_state_sizes = tables.drivers.iter().map(|d| d.state_size).collect();
        let mut wiring = RuntimeWiring {
            tables,
            dispatch,
            init_calls,
            driver_init_calls,
            state_sizes,
            driver_state_sizes,
            group_child_index: boot.group_child_index.clone(),
            actor_cores,
            placement,
        };
        // plans/M8.md item C2: the ring set, last — it is derived from the
        // finished placement plus the compiled call sites, and it grows
        // `rtdata` by exactly its own reservation.
        let rings = cross_core_rings(program, &wiring)?;
        reject_unlowerable_cross_core_shapes(&rings, &wiring, boot, program)?;
        wiring.tables.add_cross_core_rings(rings);
        Ok(Some(wiring))
    }
}

/// The assembled runtime block: `build_runtime_glue_block`'s routines
/// followed by `build_boot_init`'s, one contiguous run of words starting at
/// word index `start` within whichever section the caller places it in.
/// Every index here (`symbols`, `rt_run_one_start`, `boot_init_start`, and
/// each reloc's own `word`) is section-relative in exactly that sense.
///
/// Word counts never depend on `placement`'s address *values* (every
/// `load_imm` is a fixed four words) — only on the wiring's own shapes — so
/// both callers build this twice: once against a placeholder placement
/// purely to learn `words.len()` before `rtdata`'s base can be known, then
/// again against the real placement for the bytes that ship. Both assert
/// the two passes agreed on the length.
struct RuntimeBlock {
    words: Vec<u32>,
    relocs: Vec<Reloc>,
    symbols: BTreeMap<String, usize>,
    rt_run_one_start: usize,
    /// plans/M8.md item C1: `(core, section-relative word index)` for every
    /// secondary core's own entry block. Empty for a single-core image.
    core_entry_starts: Vec<(usize, usize)>,
    boot_init_start: usize,
}

/// `device_regs` is this image's own placed register windows — empty on
/// the *sizing* pass (which runs before any section exists) and real on
/// the address pass. Nothing about the block's length depends on it: a
/// `load_imm` is four words whatever it loads, which is exactly the
/// invariant both image flavors' two-pass assembly already asserts.
fn build_runtime_block(
    wiring: &RuntimeWiring,
    placement: &RuntimePlacement,
    device_regs: &[DeviceRegs],
    pools: &[PoolPlacement],
    start: usize,
    abort_fixed_local: Option<usize>,
) -> Result<RuntimeBlock, LayoutError> {
    let glue = build_runtime_glue_block(
        &wiring.tables,
        &wiring.dispatch,
        placement,
        &wiring.actor_cores,
        &wiring.group_child_index,
        start,
    );
    let mut words = Vec::new();
    let mut relocs = Vec::new();
    for asm in &glue.asms {
        words.extend(asm.words.iter().copied());
        relocs.extend(asm.relocs.iter().cloned());
    }
    let boot_init_start = start + words.len();
    let boot_init = build_boot_init(
        &placement.actors,
        &placement.drivers,
        &wiring.state_sizes,
        &wiring.driver_state_sizes,
        &wiring.init_calls,
        &wiring.driver_init_calls,
        device_regs,
        pools,
        boot_init_start,
        abort_fixed_local,
    )?;
    words.extend(boot_init.words.iter().copied());
    relocs.extend(boot_init.relocs.iter().cloned());
    Ok(RuntimeBlock {
        words,
        relocs,
        symbols: glue.symbols,
        rt_run_one_start: glue.rt_run_one_start,
        core_entry_starts: glue
            .core_entry_starts
            .iter()
            .enumerate()
            .map(|(i, &w)| (i + 1, w))
            .collect(),
        boot_init_start,
    })
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

    /// plans/M10.md item B4: `BL __wrela_console_append_bytes`.
    /// Pre: `x0` holds the packed-byte base. Sets `x1 = x2 = len` so the
    /// Bytes handle's capacity equals the copy length (every harness
    /// literal call site).
    fn bl_console_append_bytes(&mut self, len: u64) {
        self.load_imm(1, len);
        self.load_imm(2, len);
        self.bl_call_key("__wrela_console_append_bytes");
    }

    /// plans/M10.md item B4: like [`Self::bl_console_append_bytes`] but
    /// `x0`/`x1` already hold `(base, len)` — copies `x1` into `x2`.
    fn bl_console_append_bytes_xy(&mut self) {
        self.push(encode::enc_mov_reg(2, 1, true));
        self.bl_call_key("__wrela_console_append_bytes");
    }

    /// plans/M10.md item B4: `BL __wrela_console_append_line_buf`.
    /// Pre: `x0` holds the byte length written into `OFF_TEST_LINE_BUF`.
    fn bl_console_append_line_buf(&mut self) {
        self.bl_call_key("__wrela_console_append_line_buf");
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

    /// The 32-bit forms, for the `u32` fields plans/M10.md item 0c1
    /// introduced (`waker_turn`/`waker_core`, a reply-ring slot's
    /// destination `TurnId`). `cbz w`/`cbnz w` tests exactly the four bytes
    /// the field occupies — an `x` test here would fold the *adjacent*
    /// field in as high bits, which is precisely the confusion decision
    /// 557's two-`u32` encoding exists to avoid.
    fn patch_cbz_w(&mut self, marker: usize, reg: u8) {
        let target = self.abs();
        let this = self.start + marker;
        let delta = (target as i64 - this as i64) * 4;
        self.words[marker] = encode::enc_cbz(reg, delta as i32, false);
    }

    fn patch_cbnz_w(&mut self, marker: usize, reg: u8) {
        let target = self.abs();
        let this = self.start + marker;
        let delta = (target as i64 - this as i64) * 4;
        self.words[marker] = encode::enc_cbnz(reg, delta as i32, false);
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

/// The shared tail every abort body ends in: clear the re-entrancy latch
/// (plans/M10.md item B1 / decision 591), increment
/// `machine_info::OFF_TEST_FAILED` and long-jump to the landing pad's own
/// continuation address (module doc's own "landing pad" section) — never
/// `RET`. Clobbers `x9`/`x10`.
fn push_abort_tail(a: &mut Asm, addrs: &HarnessAddrs) {
    // Clear latch before the continuation jump so a later green test never
    // observes a stale "already aborting" bit from a prior abort.
    a.load_imm(9, addrs.info_base + mi::OFF_ABORT_LATCH);
    a.push(encode::enc_movz(10, 0, 0, true));
    a.push(encode::enc_str_x_imm(10, 9, 0));
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
///
/// plans/M10.md item B1 / decision 591: a one-word re-entrancy latch at
/// `OFF_ABORT_LATCH` routes a second entry (bounds failure inside the
/// console print path) straight to the halt tail without printing again.
fn build_abort_fixed(
    addrs: &HarnessAddrs,
    start: usize,
    append_start: usize,
    commit_start: usize,
    failed_word_off: usize,
    newline_off: usize,
) -> Asm {
    // M10 B4: callers use compiled `__wrela_console_append_*` via
    // `bl_call_key`; hand-asm `build_ring_append` stays until B5.
    let _ = append_start;
    let mut a = Asm::new(start);
    // Latch check before any SP work — re-entry must not touch the stack.
    a.load_imm(9, addrs.info_base + mi::OFF_ABORT_LATCH);
    a.push(encode::enc_ldr_x_imm(10, 9, 0));
    let reenter = a.skip_placeholder(); // cbnz x10 → shared_tail
    a.push(encode::enc_movz(10, 1, 0, true));
    a.push(encode::enc_str_x_imm(10, 9, 0));

    a.push(encode::enc_sub_imm(31, 31, 16, true)); // sub sp, sp, #16
    a.push(encode::enc_str_x_imm(0, 31, 0));
    a.push(encode::enc_str_x_imm(1, 31, 8));

    a.load_rodata_addr_at(0, failed_word_off);
    a.bl_console_append_bytes(7);

    a.push(encode::enc_ldr_x_imm(0, 31, 0));
    a.push(encode::enc_ldr_x_imm(1, 31, 8));
    a.bl_console_append_bytes_xy();

    a.load_rodata_addr_at(0, newline_off);
    a.bl_console_append_bytes(1);

    a.push(encode::enc_add_imm(31, 31, 16, true)); // add sp, sp, #16
    // M10 B2: compiled `__wrela_line_commit` (hand-asm still emitted for B5).
    let _ = commit_start;
    a.bl_call_key("__wrela_line_commit");
    a.patch_cbnz(reenter, 10);
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
///
/// Same re-entrancy latch as `build_abort_fixed` (decision 591).
fn build_abort_val(
    addrs: &HarnessAddrs,
    start: usize,
    append_start: usize,
    commit_start: usize,
    fmt_dec_start: usize,
    failed_word_off: usize,
    newline_off: usize,
) -> Asm {
    // M10 B4: see build_abort_fixed — hand-asm append kept until B5.
    let _ = append_start;
    let mut a = Asm::new(start);
    a.load_imm(9, addrs.info_base + mi::OFF_ABORT_LATCH);
    a.push(encode::enc_ldr_x_imm(10, 9, 0));
    let reenter = a.skip_placeholder(); // cbnz x10 → shared_tail
    a.push(encode::enc_movz(10, 1, 0, true));
    a.push(encode::enc_str_x_imm(10, 9, 0));

    a.push(encode::enc_sub_imm(31, 31, 48, true));
    for (i, reg) in [0u8, 1, 2, 3, 4, 5].into_iter().enumerate() {
        a.push(encode::enc_str_x_imm(reg, 31, (i * 8) as u16));
    }

    a.load_rodata_addr_at(0, failed_word_off);
    a.bl_console_append_bytes(7);

    a.push(encode::enc_ldr_x_imm(0, 31, 0));
    a.push(encode::enc_ldr_x_imm(1, 31, 8));
    a.bl_console_append_bytes_xy(); // prefix

    a.push(encode::enc_ldr_x_imm(0, 31, 16));
    a.push(encode::enc_ldr_x_imm(1, 31, 24));
    // M10 B3: wrela `__wrela_fmt_dec` (hand-asm retained until B5).
    let _ = fmt_dec_start;
    a.bl_call_key("__wrela_fmt_dec"); // x0 = len, written into OFF_TEST_LINE_BUF
    // M10 B4: append goes through compiled `__wrela_console_append_line_buf`.
    a.bl_console_append_line_buf();

    a.push(encode::enc_ldr_x_imm(0, 31, 32));
    a.push(encode::enc_ldr_x_imm(1, 31, 40));
    a.bl_console_append_bytes_xy(); // suffix

    a.load_rodata_addr_at(0, newline_off);
    a.bl_console_append_bytes(1);

    a.push(encode::enc_add_imm(31, 31, 48, true));
    // M10 B2: compiled `__wrela_line_commit` (hand-asm still emitted for B5).
    let _ = commit_start;
    a.bl_call_key("__wrela_line_commit");
    a.patch_cbnz(reenter, 10);
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
    // plans/M6.md item F #3: `__wrela_deadline_poll`'s own harness-absolute
    // word index, present only for a build with a group arena. Called once
    // per scheduler tick (module doc on `emit_deadline_poll`).
    deadline_poll_word: Option<usize>,
    rodata: &mut Vec<Vec<u8>>,
    rodata_cursor: &mut usize,
    boot_init_start: Option<usize>,
    test_args: &BTreeMap<String, Vec<u64>>,
    // plans/M8.md item C1: how many cores this image brings up
    // (`RuntimeTables::cores`). `1` emits not one extra instruction — the
    // whole of what keeps every M5-M7 boot byte-identical.
    cores: usize,
) -> Asm {
    // M10 B4: append goes through compiled `__wrela_console_append_*`;
    // hand-asm `build_ring_append` stays until B5.
    let _ = append_start;
    let mut a = Asm::new(start);
    let sp_top = machine_layout::core_stack_base(0) + machine_layout::CORE_STACK_SIZE;

    a.load_imm(9, sp_top);
    a.push(encode::enc_add_imm(31, 9, 0, true)); // mov sp, x9

    // 06-machine.md §3 step 3, in the order the chapter states it: "the
    // entry installs per-core state, **releases the other vCPUs**, runs
    // typed driver and actor initialization in image dependency order,
    // opens mailboxes atomically, and enters the per-core event loops."
    // Core 0's own mark goes down first (the same word every secondary
    // writes — the VMM checks all of them at halt), then the release
    // doorbell tells the VMM how many cores this image brings up.
    //
    // "Released" means eligible to run, not running: the VMM hands the
    // baton to each released core in turn, and each of them runs before
    // this one continues (plans/M8.md decision 11 — never concurrently).
    // Boot init below therefore still runs before any turn anywhere, which
    // is what the chapter's own ordering requires.
    if cores > 1 {
        a.load_imm(9, machine_info::core_mark_running(0));
        a.load_imm(10, machine_info::core_mark_addr(0));
        a.push(encode::enc_str_x_imm(9, 10, 0));
        a.load_imm(9, cores as u64);
        a.load_imm(10, wrela_machine::mmio::RELEASE_MMIO_ADDR);
        a.push(encode::enc_str_x_imm(9, 10, 0));
    }

    a.push(encode::enc_movz(9, 0, 0, true)); // x9 = 0
    for off in [
        mi::OFF_TEST_PASSED,
        mi::OFF_TEST_FAILED,
        mi::OFF_RING_DATA_BUMP,
        mi::OFF_RING_DESC_BUMP,
        mi::OFF_LINE_START,
        mi::OFF_ABORT_LATCH,
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
    //
    // plans/M7.md item H1, **found by running**: an `assert` inside a
    // boot-time `init` used to fault the guest at `pc=0x0` instead of
    // reporting anything. `push_abort_tail` long-jumps through
    // `machine_info::OFF_TEST_CONTINUATION`, and until this commit that
    // word was written only inside the per-test loop below — so any abort
    // *before* the first test branched to address zero. That is a live
    // defect of item W (which is what first made boot call an `init` with
    // arguments at all); item H1 is where it surfaced, because an abort
    // inside a driver's `init` is this item's own vacuity control
    // (`golden/err-boot-driver-init-runs`).
    //
    // The fix is the landing pad the rest of this file already uses: open
    // a report line and point the continuation at the summary block, both
    // *before* boot runs. On success nothing is appended to that line and
    // the first test's own `line_begin` re-opens it at the identical
    // position, so a green image's transcript is byte-identical; on an
    // abort the message lands on a line of its own, `OFF_TEST_FAILED`
    // counts it, and the image exits nonzero through the ordinary summary
    // — 06-machine.md §3's own "typed driver and actor initialization"
    // failing is image-fatal with a diagnosable line (plans/M6.md decision
    // 12, plans/M7.md decision 8), never a fault at zero.
    let boot_cont_marker = if let Some(boot_init) = boot_init_start {
        // M10 B2: compiled `__wrela_line_begin` (hand-asm still emitted for B5).
        let _ = line_begin_start;
        a.bl_call_key("__wrela_line_begin");
        let marker = a.load_imm_placeholder(9);
        a.load_imm(10, addrs.info_base + mi::OFF_TEST_CONTINUATION);
        a.push(encode::enc_str_x_imm(9, 10, 0));
        a.bl_to(boot_init);
        Some(marker)
    } else {
        None
    };

    let ok_off = append_rodata(rodata, rodata_cursor, b"ok\n".to_vec());
    let passed_comma_off = append_rodata(rodata, rodata_cursor, b" passed, ".to_vec());
    let failed_tail_off = append_rodata(rodata, rodata_cursor, b" failed\n".to_vec());

    for name in runtime_tests {
        let prefix_bytes = format!("test {name}: ").into_bytes();
        let prefix_len = prefix_bytes.len() as u64;
        let prefix_off = append_rodata(rodata, rodata_cursor, prefix_bytes);

        // M10 B2: compiled `__wrela_line_begin` (hand-asm still emitted for B5).
        let _ = line_begin_start;
        a.bl_call_key("__wrela_line_begin");

        a.load_rodata_addr_at(0, prefix_off);
        a.bl_console_append_bytes(prefix_len);

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
            let mut continue_after_loop = false;
            let status_loop_top = a.abs();
            let skip_done = a.skip_placeholder(); // cbz x0, .done
            let drive_top = a.abs();
            // plans/M6.md item F #3: the deadline service's own scheduler
            // half, once per tick, before anything is selected — it arms
            // `OFF_NEXT_DEADLINE` for the park branch below AND raises the
            // deadline vector when the minimum live deadline has already
            // passed, so a turn that keeps running (rather than parking)
            // still observes its group's cancellation at its very next
            // checkpoint. Absent entirely for a build with no group arena,
            // byte-identical to every pre-item-F image.
            if let Some(poll) = deadline_poll_word {
                a.bl_to(poll);
            }
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
            // plans/M8.md item C2, decision 31: in a **cross-core** image
            // "nothing ready here" is never local evidence of deadlock —
            // another core may be one turn away from pushing a reply onto
            // this core's inbound ring — so core 0 parks unconditionally
            // and the VMM's own `core N parked and no core is runnable`
            // becomes the deadlock diagnostic (it already fails closed with
            // a named line rather than hanging). A single-core image is
            // untouched, down to the word: the deadline test and
            // `DEADLOCK_MSG` arm below are exactly M6's.
            if cores > 1 {
                a.load_imm(9, wrela_machine::mmio::PARK_MMIO_ADDR);
                a.load_imm(10, 0);
                a.push(encode::enc_str_x_imm(10, 9, 0));
                a.bl_to(checkpoint_service_word);
                a.b_to(drive_top);
                let reenter = a.abs();
                a.patch_cbnz(skip_reenter, 10);
                debug_assert_eq!(reenter, a.abs());
                a.bl_call_key(name); // resume (the fn's own discriminant routes)
                a.b_to(status_loop_top);
                let done = a.abs();
                a.patch_cbz(skip_done, 0);
                debug_assert_eq!(done, a.abs());
                let _ = ddl_off;
                continue_after_loop = true;
            }
            if !continue_after_loop {
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
        }

        a.load_rodata_addr_at(0, ok_off);
        a.bl_console_append_bytes(3);

        // M10 B2: compiled `__wrela_line_commit` (hand-asm still emitted for B5).
        let _ = commit_start;
        a.bl_call_key("__wrela_line_commit");

        a.load_imm(9, addrs.info_base + mi::OFF_TEST_PASSED);
        a.push(encode::enc_ldr_x_imm(10, 9, 0));
        a.push(encode::enc_add_imm(10, 10, 1, true));
        a.push(encode::enc_str_x_imm(10, 9, 0));

        let cont_target = a.addr(harness_base);
        a.patch_load_imm(cont_marker, 9, cont_target);
    }

    // Summary line: "<passed> passed, <failed> failed\n" — one descriptor,
    // like every test's own line above. It is also boot's own abort
    // landing (above): an `init` that aborted has already printed and
    // counted its failure, and lands here to report the totals and exit.
    if let Some(marker) = boot_cont_marker {
        let target = a.addr(harness_base);
        a.patch_load_imm(marker, 9, target);
    }
    // M10 B2: compiled `__wrela_line_begin` (hand-asm still emitted for B5).
    let _ = line_begin_start;
    a.bl_call_key("__wrela_line_begin");

    a.load_imm(9, addrs.info_base + mi::OFF_TEST_PASSED);
    a.push(encode::enc_ldr_x_imm(0, 9, 0));
    a.push(encode::enc_movz(1, 0, 0, true));
    // M10 B3 + B4: fmt_dec then append_line_buf (x0 = len).
    let _ = fmt_dec_start;
    a.bl_call_key("__wrela_fmt_dec");
    a.bl_console_append_line_buf();

    a.load_rodata_addr_at(0, passed_comma_off);
    a.bl_console_append_bytes(9);

    a.load_imm(9, addrs.info_base + mi::OFF_TEST_FAILED);
    a.push(encode::enc_ldr_x_imm(0, 9, 0));
    a.push(encode::enc_movz(1, 0, 0, true));
    // M10 B3 + B4
    a.bl_call_key("__wrela_fmt_dec");
    a.bl_console_append_line_buf();

    a.load_rodata_addr_at(0, failed_tail_off);
    a.bl_console_append_bytes(8);

    // M10 B2: compiled `__wrela_line_commit` (hand-asm still emitted for B5).
    let _ = commit_start;
    a.bl_call_key("__wrela_line_commit");

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
    /// `codegen::compute_group_child_indices`' result for this same build
    /// (plans/M6.md item F): every `g.start`-able callee's own fixed
    /// child-slot ordinal — the one fact `build_runtime_glue_block` needs
    /// to build each static call site's own poll routine
    /// (`build_group_child_poll`) alongside the actor glue. Empty for a
    /// build with no `with group(...)` sites at all.
    pub group_child_index: &'a BTreeMap<String, usize>,
}

/// plans/M10.md item B2: inject force-rooted `core.runtime` helpers into
/// a `CodegenProgram` that was lowered without the auto-loaded module
/// (fuzz / diff-eval / profile / older conformance shortcuts). Real
/// `wrela test` already lowers them via `guest_reachable_keys_closure`;
/// this is the fail-closed backstop so `layout_test_image`'s harness
/// `bl_call_key("__wrela_line_*")` never meets a missing symbol.
fn with_force_rooted_runtime(program: &CodegenProgram) -> Result<CodegenProgram, LayoutError> {
    let missing: Vec<&str> = crate::lower::RUNTIME_FORCE_ROOT_KEYS
        .iter()
        .copied()
        .filter(|k| !program.fns.contains_key(*k))
        .collect();
    if missing.is_empty() {
        return Ok(program.clone());
    }
    let runtime_cg = codegen_runtime_force_roots().map_err(|m| {
        LayoutError::new(format!(
            "internal error: could not codegen force-rooted runtime helpers ({missing:?}): {m}"
        ))
    })?;
    let mut out = program.clone();
    let rodata_byte_base: usize = out.rodata.iter().map(Vec::len).sum();
    for (key, mut f) in runtime_cg.fns {
        if out.fns.contains_key(&key) {
            continue;
        }
        if rodata_byte_base != 0 {
            for r in &mut f.relocs {
                if let Reloc::Rodata { byte_offset, .. } = r {
                    *byte_offset += rodata_byte_base;
                }
            }
        }
        out.fns.insert(key, f);
    }
    out.rodata.extend(runtime_cg.rodata);
    Ok(out)
}

/// Lower + codegen every `RUNTIME_FORCE_ROOT_KEYS` entry from
/// `stdlib/core/runtime.wr` as a standalone `CodegenProgram`.
fn codegen_runtime_force_roots() -> Result<CodegenProgram, String> {
    let (runtime_key, runtime_loaded) = crate::loader::load_runtime_module()
        .map_err(|_| "stdlib/core/runtime.wr missing".to_string())?;
    let mut modules = BTreeMap::new();
    modules.insert(runtime_key.clone(), runtime_loaded.module);
    let mut paths = BTreeMap::new();
    paths.insert(
        runtime_key.clone(),
        runtime_loaded.file.display().to_string(),
    );
    let programs_vec = crate::sema::check_program_typed(&modules, &paths).map_err(|e| e.message)?;
    let programs: BTreeMap<String, crate::sema::typed::TypedProgram> = programs_vec
        .into_iter()
        .map(|(k, p)| (k.join("."), p))
        .collect();
    let modules_dot: BTreeMap<String, Module> =
        modules.into_iter().map(|(k, m)| (k.join("."), m)).collect();
    // Force-root seeds plus their callee closure (M10 B3: `fmt_dec` →
    // `store_at` / `extract_one` / …; M10 B4: append → `copy_*_range`).
    // Seeding only the root keys leaves those helpers un-codegen'd.
    let only =
        crate::lower::guest_reachable_keys_closure(&programs, &crate::lower::LowerOpts::default());
    let lower_opts = crate::lower::LowerOpts {
        emit_comptime_tests: false,
        only: Some(only),
    };
    let mut mwir_programs = Vec::new();
    let mut flow_fns = BTreeMap::new();
    for typed in programs.values() {
        mwir_programs
            .push(crate::lower::lower_program_with(typed, &lower_opts).map_err(|e| e.message)?);
        flow_fns.extend(
            crate::flowwir_lower::lower_program_with(typed, &lower_opts)
                .map_err(|e| e.message)?
                .fns,
        );
    }
    let mwir = merge_mwir_programs(mwir_programs);
    let flow = crate::flowwir::FlowWirProgram { fns: flow_fns };
    let mut layout_ctx = merge_layout_ctx(&modules_dot).map_err(|e| e.message)?;
    enrich_layout_ctx_with_instantiations(&mut layout_ctx, &programs);
    let method_index =
        actor_method_index_tables(&modules_dot, &layout_ctx).map_err(|e| e.message)?;
    // M10 item D: dump / no-@image paths have no mailbox roots — empty specs.
    crate::codegen::codegen_program_with_async(&mwir, &flow, &layout_ctx, &method_index, 0, &[])
        .map_err(|e| e.message)
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
    // M10 B2: harness bl_call_keys force-rooted console helpers. Callers
    // that already lowered `core.runtime` are a no-op; others get them
    // injected here so the reloc never fails closed on a missing symbol.
    let program = with_force_rooted_runtime(program)?;
    let program = &program;
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
    // behavior, byte-identical. Derived by `RuntimeWiring::derive`, the
    // one copy `layout_program` uses too (that fn's own module block above
    // has the full reasoning).
    let mut wiring: Option<RuntimeWiring> = match &boot {
        Some(b) => RuntimeWiring::derive(b, program)?,
        None => None,
    };
    // plans/M7.md item E1: intern fallible-`init` abort messages before
    // either runtime-block assembly pass (same pool as the shared
    // `FAILED `/newline literals above).
    if let Some(w) = wiring.as_mut() {
        intern_fallible_init_abort_messages(w, &mut rodata, &mut rodata_cursor);
    }
    let runtime_tables: Option<RuntimeTables> = wiring.as_ref().map(|w| w.tables.clone());

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
    // plans/M6.md item F: built twice (shape-only, then with real `rtdata`
    // addresses), exactly like the runtime glue block below — the vector-0
    // body is now the real deadline scan whenever this build has a group
    // arena. `layout_program`'s own copy of this two-pass shape has the
    // full reasoning.
    let checkpoint_shape = group_service_shape(runtime_tables.as_ref());
    let (irq_shape, wake_shape) =
        checkpoint_irq_shape(boot.as_ref(), None, runtime_tables.as_ref());
    let checkpoint_block =
        build_checkpoint_and_vector_stub_ex(checkpoint_shape.as_ref(), &irq_shape, &wake_shape);
    let checkpoint_service_offset = checkpoint_block.checkpoint_service_word;
    let deadline_poll_offset = checkpoint_block.deadline_poll_word;
    let checkpoint_words_len = checkpoint_block.words.len();
    // `bl_call_key` records block-relative words when built at start=0;
    // shift them to harness-absolute for the shared reloc resolver.
    let checkpoint_relocs: Vec<Reloc> = checkpoint_block
        .relocs
        .into_iter()
        .map(|r| match r {
            Reloc::Call { word, key } => Reloc::Call {
                word: word + checkpoint_start,
                key,
            },
            other => other,
        })
        .collect();
    let checkpoint_asm = Asm {
        start: checkpoint_start,
        words: checkpoint_block.words,
        relocs: checkpoint_relocs,
    };
    // `__wrela_checkpoint_service`'s own harness-absolute word index (see
    // `build_checkpoint_and_vector_stub`'s doc: `__wrela_vector0_service`
    // sits first, so the section's own start is never the right target).
    let checkpoint_service_word = checkpoint_start + checkpoint_service_offset;
    let deadline_poll_word = deadline_poll_offset.map(|o| checkpoint_start + o);

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
    let sizing_device_regs = place_device_regs(0, &device_register_windows(boot.as_ref())?)
        .map(|(regs, _, _, _)| regs)
        .unwrap_or_default();
    let sizing_pools = place_pools_unchecked(0, &image_pool_backings(boot.as_ref())?)
        .map(|(pools, _, _, _)| pools)
        .unwrap_or_default();
    let dummy_block = wiring
        .as_ref()
        .map(|w| {
            build_runtime_block(
                w,
                &place_runtime_tables(0, &w.tables),
                &sizing_device_regs,
                &sizing_pools,
                glue_start,
                Some(abort_fixed_start),
            )
        })
        .transpose()?;
    let runtime_words_len = dummy_block.as_ref().map(|b| b.words.len()).unwrap_or(0);
    let rt_run_one_start = dummy_block.as_ref().map(|b| b.rt_run_one_start);
    let boot_init_start_opt = dummy_block.as_ref().map(|b| b.boot_init_start);
    // plans/M8.md item C1: word indices only (identical across both
    // assembly passes, asserted below), so they are known here — before
    // `rtdata_base` exists — which is what lets the entry driver's own
    // release store be emitted in the single pass it is built in.
    let core_entry_starts: Vec<(usize, usize)> = dummy_block
        .as_ref()
        .map(|b| b.core_entry_starts.clone())
        .unwrap_or_default();
    let cores = wiring.as_ref().map(|w| w.tables.cores).unwrap_or(1);

    let entry_start = glue_start + runtime_words_len;
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
        deadline_poll_word,
        &mut rodata,
        &mut rodata_cursor,
        boot_init_start_opt,
        test_args,
        cores,
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
    if let Some(b) = &dummy_block {
        harness_words.extend(b.words.iter().copied());
    }
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

    // plans/M7.md item D: the same `pooldata` reservation `layout_program`
    // makes, for the same reason — a test image that declares a pool
    // reserves its backing too, or the two image flavors would emit
    // different memory maps for the same source (plans/M6.md item F/G's
    // own rule that the two flavors emit and reject identically). Only
    // the *backing* is resolved here; the placement itself waits until
    // the section table exists, so `place_pools` can check its own base
    // against every section already placed.
    let pool_backings = image_pool_backings(boot.as_ref())?;

    // plans/M7.md item H1: the same `devregs` reservation `layout_program`
    // makes, at the same point in the same order (after `rtdata`, before
    // `pooldata`). Placed *here*, before the runtime block's real-address
    // pass below, because that pass is what bakes each `DeviceCap[D]`
    // argument word into boot's own `init` call.
    let device_windows = device_register_windows(boot.as_ref())?;
    let placed_regs = place_device_regs(cursor, &device_windows);
    let device_regs: Vec<DeviceRegs> = match &placed_regs {
        Some((regs, _, _, end)) => {
            cursor = *end;
            regs.clone()
        }
        None => Vec::new(),
    };
    let pool_cursor = cursor;
    let _ = cursor;
    // The pools' own bases, needed *now* rather than after the section
    // table exists, for the same reason `device_regs` is: item H1 made a
    // `DmaPool[P, N]` `init` argument a real address word, and the
    // boot-init block that carries it is assembled below. `place_pools`
    // is called again once `sections` exists — same fn, same cursor, same
    // backings, so the two placements are the same placement.
    let early_pools = place_pools_unchecked(pool_cursor, &pool_backings)
        .map(|(pools, _, _, _)| pools)
        .unwrap_or_default();

    // Now that `rtdata_base` is real, rebuild the address-dependent
    // fragments (glue routines + boot-init) at the identical word offsets
    // the placeholder pass already reserved — replacing their
    // placeholder-valued bytes in `harness_words` in place.
    let (glue_symbols, real_placement): (BTreeMap<String, usize>, Option<RuntimePlacement>) =
        if let Some(w) = &wiring {
            let tables = &w.tables;
            let real_base =
                rtdata_base.expect("rtdata reserved above whenever runtime_tables is Some");
            let placement = place_runtime_tables(real_base, tables);
            // The checkpoint block's own second pass (module doc on
            // `build_checkpoint_and_vector_stub`): vector-0 / deadline /
            // ISR / wake-drain now address the real, placed rtdata.
            // Same word count by construction, asserted. Call relocs were
            // already recorded on the sizing pass (identical sites).
            let (irq_real, wake_real) =
                checkpoint_irq_shape(boot.as_ref(), Some(&placement), Some(tables));
            if group_service_ctx(&placement, tables).is_some()
                || !irq_real.is_empty()
                || !wake_real.is_empty()
            {
                let real_cp = build_checkpoint_and_vector_stub_ex(
                    group_service_ctx(&placement, tables).as_ref(),
                    &irq_real,
                    &wake_real,
                );
                if real_cp.words.len() != checkpoint_words_len {
                    return Err(LayoutError::new(
                        "internal error: the checkpoint block's own word count changed between \
                         its sizing pass and its real-address pass",
                    ));
                }
                for (i, word) in real_cp.words.iter().enumerate() {
                    harness_words[checkpoint_start + i] = *word;
                }
            }
            let real_block = build_runtime_block(
                w,
                &placement,
                &device_regs,
                &early_pools,
                glue_start,
                Some(abort_fixed_start),
            )?;
            if real_block.words.len() != runtime_words_len {
                return Err(LayoutError::new(
                    "internal error: the runtime block's own word count changed between its \
                     sizing pass and its real-address pass",
                ));
            }
            for (i, word) in real_block.words.iter().enumerate() {
                harness_words[glue_start + i] = *word;
            }
            // `build_rt_select_and_run_symbolic`'s own dispatch chain (and
            // `build_boot_init`'s own `init` calls) carry real
            // `Reloc::Call`s — a sync method's real compiled body, or an
            // async method's real state-machine entry — which must resolve
            // exactly like every other harness-section call, or the emitted
            // `BL` stays a self-referencing placeholder.
            harness_relocs.extend(real_block.relocs.iter().cloned());
            debug_assert_eq!(glue_start + real_block.words.len(), entry_start);
            (real_block.symbols, Some(placement))
        } else {
            (BTreeMap::new(), None)
        };

    // Resolves a `Reloc::TurnFrameAddr` key to its real turn-area
    // address (`RuntimePlacement::turn_area_for`'s own rule) — an
    // internal error if no tables exist or the key was never sized.
    // Internal-error audit: unreachable from any source program, for the
    // identical reason `layout_program`'s own copy of this guard is —
    // a `TurnFrameAddr` exists only for an async fn, one async fn is
    // already enough for `compute_runtime_tables` to size a non-empty
    // table set, and `turn_area_for` partitions every key of the same
    // `async_frames` map codegen keyed its relocs by.
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
    // plans/M10.md item 0c1: the same resolution one step earlier — the
    // `TurnId` itself, for a `Reloc::TurnIdImm`. Same unreachability
    // argument as `turn_area_addr` above, since `turn_area_for` *is*
    // `turn_id_for` scaled by the stride.
    let turn_id_imm = |key: &str| -> Result<u64, LayoutError> {
        let (Some(tables), Some(placement)) = (&runtime_tables, &real_placement) else {
            return Err(LayoutError::new(format!(
                "internal error: async fn `{key}` needs a turn id but this image has no \
                 runtime tables"
            )));
        };
        placement
            .turn_id_for(key, tables)
            .map(|id| id.get() as u64)
            .ok_or_else(|| {
                LayoutError::new(format!(
                    "internal error: async fn `{key}`'s own turn id was never sized"
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
    if let Some((_, base, size, _)) = &placed_regs {
        sections.push(Section {
            name: "devregs",
            base: *base,
            size: *size,
        });
    }
    let placed_pools = place_pools(pool_cursor, &sections, &pool_backings)?;
    let pools: Vec<PoolPlacement> = match &placed_pools {
        Some((pools, base, size, _)) => {
            sections.push(Section {
                name: "pooldata",
                base: *base,
                size: *size,
            });
            pools.clone()
        }
        None => Vec::new(),
    };

    // --- resolve relocs ----------------------------------------------------
    // Internal-error audit: every guard in this harness loop is unreachable
    // from any source program. `Call` targets here are the entry driver's
    // own `@test(runtime)` roots (already checked against `fn_word_base` at
    // the top of this fn), a declared actor's `pub` method keys and its
    // zero-argument `init` — all read from the same module set `codegen`
    // compiled, with a method that fails to lower stopping the attempt one
    // layer up. `Rodata` cannot outrun its own pool (interning a literal is
    // what fills it). The last arm is structural: the harness builders in
    // this file emit no such reloc.
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
            | Reloc::TurnIdImm { .. }
            | Reloc::TurnsBase { .. }
            | Reloc::TurnStride { .. }
            | Reloc::GroupArenaBase { .. }
            | Reloc::IrqVector { .. }
            | Reloc::WakePending { .. }
            | Reloc::MailboxAddr { .. } => {
                return Err(LayoutError::new(
                    "internal error: the harness section itself must never emit an \
                     AbortFixed/AbortVal/CheckpointService/TurnIdImm/TurnsBase/TurnStride/\
                     GroupArenaBase/IrqVector/WakePending/MailboxAddr reloc",
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
                    // second (`codegen::rt_enqueue_actor`'s doc records the
                    // one disclosed way the two naming schemes could ever
                    // collide, and why nothing enforces against it yet).
                    // The `else` arm is the audit's one genuinely
                    // user-reachable find — `unresolved_call_target`.
                    // plans/M8.md item C2: see `layout_program`'s twin —
                    // a cross-core edge resolves to its own `__rt_xsend_*`.
                    let redirect = resolve_cross_core_edge(key, target, wiring.as_ref())?;
                    let target = redirect.as_deref().unwrap_or(target.as_str());
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = if let Some(target_base) = fn_word_base.get(target) {
                        code_base + (*target_base as u64) * 4
                    } else if let Some(glue_word) = glue_symbols.get(target) {
                        harness_base + (*glue_word as u64) * 4
                    } else {
                        return Err(unresolved_call_target(
                            target,
                            boot.as_ref().map(|b| b.graph),
                        ));
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
                Reloc::TurnIdImm { word, key: fn_key } => {
                    // plans/M10.md item 0c1: this turn's own `TurnId`, as
                    // the waker of every message it awaits and as the owner
                    // half of its `OFF_TURN_REPLY_SLOT` pair.
                    let id = turn_id_imm(fn_key)?;
                    patch_load_imm_words(&mut code_words, base + word, id);
                }
                Reloc::TurnsBase { word } => {
                    // plans/M10.md item 0c3, twin of `layout_program`'s arm.
                    let addr = real_placement
                        .as_ref()
                        .map(|p| p.turns_base)
                        .ok_or_else(|| LayoutError::new(turns_deref_needs_rtdata()))?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                Reloc::TurnStride { word } => {
                    let stride = real_placement
                        .as_ref()
                        .map(|p| p.turn_stride)
                        .ok_or_else(|| LayoutError::new(turns_deref_needs_rtdata()))?;
                    patch_load_imm_words(&mut code_words, base + word, stride);
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
                Reloc::IrqVector { word, driver } => {
                    let vector = driver_irq_vector(boot.as_ref().map(|b| b.graph), driver)?;
                    patch_load_imm_words(&mut code_words, base + word, vector);
                }
                Reloc::WakePending { word, driver } => {
                    let (p, t) = match (real_placement.as_ref(), runtime_tables.as_ref()) {
                        (Some(p), Some(t)) => (p, t),
                        _ => {
                            return Err(wake_needs_rtdata(driver));
                        }
                    };
                    let addr = driver_wake_pending_addr(p, t, driver)?;
                    patch_load_imm_words(&mut code_words, base + word, addr);
                }
                // M10 D / decision 614
                Reloc::MailboxAddr { word, actor, field } => {
                    let (p, t) = match (real_placement.as_ref(), runtime_tables.as_ref()) {
                        (Some(p), Some(t)) => (p, t),
                        _ => {
                            return Err(LayoutError::new(
                                "internal error: a Reloc::MailboxAddr exists but this image has \
                                 no runtime tables",
                            ));
                        }
                    };
                    let addrs = resolve_mailbox_ring_addrs(p, t, actor).ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: Reloc::MailboxAddr names actor `{actor}`, which this \
                             image's runtime tables never placed a mailbox for"
                        ))
                    })?;
                    let addr = match field {
                        crate::codegen::MailboxField::Ring => addrs.ring,
                        crate::codegen::MailboxField::Tail => addrs.tail,
                        crate::codegen::MailboxField::Count => addrs.count,
                    };
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
    if let Some((_, base, size, _)) = &placed_regs {
        pad_to(&mut blob, image_base, *base);
        blob.resize(blob.len() + *size as usize, 0);
    }
    if let Some((_, base, size, _)) = &placed_pools {
        pad_to(&mut blob, image_base, *base);
        blob.resize(blob.len() + *size as usize, 0);
    }

    verify_section_sizes(&sections, image_base, blob.len() as u64)?;
    verify_pool_windows(&sections, &pools)?;
    verify_device_windows(&sections, &device_regs)?;
    // blk filled later — ring verify runs in attach_blk_report

    let irq_host_injects = build_irq_host_injects(boot.as_ref(), &device_regs);
    // plans/M8.md item C1: harness-section word indices resolved against
    // that section's own base (which is `IMAGE_BASE` on this flavor).
    let core_entries: Vec<(usize, u64)> = core_entry_starts
        .iter()
        .map(|&(core, word)| (core, harness_base + (word as u64) * 4))
        .collect();
    Ok(ImageLayout {
        blob,
        entry: harness_base + (entry_start as u64) * 4,
        sections,
        // plans/M6.md item D: real at last for an actor-bearing test image
        // (`bin/wrela.rs::test_cmd` now passes a real `BootCtx` — the item-C
        // sub-note's own "staged, named work" is this commit).
        runtime: runtime_tables,
        pools,
        device_regs,
        blk: None, // filled by attach_blk_report after layout
        irq_host_injects,
        core_entries,
        placed_statics: Vec::new(),
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

    #[test]
    fn force_rooted_probe_resolves_via_bl_call_key() {
        // plans/M10.md item A2d / decision 583: after force-rooted emit,
        // a hand-asm `Reloc::Call` / `bl_call_key` finds `__wrela_runtime_probe`
        // in `fn_word_base` (not glue).
        let src = "\
module examples.bl_call_probe

@test(runtime)
pub fn t():
    return
";
        let tokens = crate::syntax::lexer::lex(src).expect("lex");
        let module = crate::syntax::parser::parse(tokens).expect("parse");
        let (runtime_key, runtime_loaded) = match crate::loader::load_runtime_module() {
            Ok(v) => v,
            Err(_) => panic!("runtime.wr must load"),
        };
        let root_key = module.path.clone();
        let mut modules_vec = BTreeMap::new();
        modules_vec.insert(root_key.clone(), module.clone());
        modules_vec.insert(runtime_key.clone(), runtime_loaded.module.clone());
        let mut paths = BTreeMap::new();
        paths.insert(root_key.clone(), "<test>".to_string());
        paths.insert(
            runtime_key.clone(),
            runtime_loaded.file.display().to_string(),
        );
        let programs_vec = crate::sema::check_program_typed(&modules_vec, &paths).expect("check");
        let programs: BTreeMap<String, crate::sema::typed::TypedProgram> = programs_vec
            .into_iter()
            .map(|(k, p)| (k.join("."), p))
            .collect();
        let modules: BTreeMap<String, Module> = modules_vec
            .into_iter()
            .map(|(k, m)| (k.join("."), m))
            .collect();
        let reachable = crate::lower::guest_reachable_keys_closure(
            &programs,
            &crate::lower::LowerOpts::default(),
        );
        assert!(reachable.contains("__wrela_runtime_probe"));
        let lower_opts = crate::lower::LowerOpts {
            emit_comptime_tests: false,
            only: Some(reachable),
        };
        let mut mwir_programs = Vec::new();
        let mut flow_fns = BTreeMap::new();
        for typed in programs.values() {
            mwir_programs.push(crate::lower::lower_program_with(typed, &lower_opts).expect("mwir"));
            flow_fns.extend(
                crate::flowwir_lower::lower_program_with(typed, &lower_opts)
                    .expect("flow")
                    .fns,
            );
        }
        let mwir = merge_mwir_programs(mwir_programs);
        let flow = crate::flowwir::FlowWirProgram { fns: flow_fns };
        let mut layout_ctx = merge_layout_ctx(&modules).expect("layout ctx");
        enrich_layout_ctx_with_instantiations(&mut layout_ctx, &programs);
        let method_index = actor_method_index_tables(&modules, &layout_ctx).expect("index");
        let codegen = crate::codegen::codegen_program_with_async(
            &mwir,
            &flow,
            &layout_ctx,
            &method_index,
            0,
            &[],
        )
        .expect("codegen");
        assert!(
            codegen.fns.contains_key("__wrela_runtime_probe"),
            "probe must be in codegen fns"
        );
        // Hand-asm BL into the force-rooted probe — same path console
        // builders will use. Layout must resolve it via fn_word_base.
        let mut a = Asm::new(0);
        a.bl_call_key("__wrela_runtime_probe");
        a.push(encode::enc_ret(30));
        let mut fns = codegen.fns.clone();
        fns.insert(
            "__wrela_hand_asm_caller".into(),
            crate::codegen::CodegenFn {
                frame_size: 16,
                code: a.words.iter().map(|w| (*w, String::new())).collect(),
                relocs: a.relocs,
            },
        );
        let codegen = crate::codegen::CodegenProgram {
            fns,
            rodata: codegen.rodata.clone(),
        };
        let runtime_tests = vec!["t".to_string()];
        let async_tests = BTreeSet::new();
        let test_args = BTreeMap::new();
        let laid = layout_test_image(&codegen, &runtime_tests, &async_tests, None, &test_args)
            .expect("layout must resolve bl_call_key to force-rooted probe");
        assert!(!laid.sections.is_empty());
    }

    /// plans/M7.md item G self-audit: empty irq/wake lists keep the M6
    /// single-vector / whole-word-clear loop byte-identical; a non-empty
    /// list takes the multi-vector path (BIC + per-bit BL) and is longer.
    #[test]
    fn checkpoint_empty_irq_wake_is_byte_identical_to_m6_path() {
        let m6 = build_checkpoint_and_vector_stub(None);
        let empty = build_checkpoint_and_vector_stub_ex(None, &[], &[]);
        assert_eq!(
            m6.words, empty.words,
            "empty irq/wake must stay byte-identical to the M6 builder"
        );
        assert_eq!(m6.relocs, empty.relocs);
        let multi = build_checkpoint_and_vector_stub_ex(
            None,
            &[IrqVectorEntry {
                vector: 1,
                handler_key: "struct:BlkDriver.on_queue_irq".into(),
                driver_state: 0x4050_0000,
            }],
            &[WakeDrainEntry {
                driver_state: 0x4050_0000,
                wake_pending_off: 24,
                task_key: "struct:BlkDriver.drain".into(),
            }],
        );
        assert!(
            multi.words.len() > m6.words.len(),
            "multi-vector path must emit more words than the M6 loop"
        );
        assert!(
            multi
                .words
                .iter()
                .any(|w| *w == encode::enc_bic_reg(10, 10, 11, true)),
            "multi-vector path must emit BIC to clear only serviced bits"
        );
        assert_eq!(multi.relocs.len(), 2, "one ISR BL + one @task BL");
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

    /// plans/M9.md item FF: an aliased import is a LayoutCtx key under the
    /// local spelling, not only the exporter's AST name.
    #[test]
    fn merge_layout_ctx_keys_aliased_imports_under_local_spelling() {
        let mut modules = BTreeMap::new();
        modules.insert(
            "lib.pair".to_string(),
            parse_one_module(
                "\
module lib.pair

pub struct Pair:
    a: u32
    b: u32

pub enum Color:
    Red
    Blue
",
            ),
        );
        modules.insert(
            "app.main".to_string(),
            parse_one_module(
                "\
module app.main

from lib.pair import Pair as Duo
from lib.pair import Color as Hue

fn use(d: Duo, h: Hue):
    pass
",
            ),
        );
        let ctx = merge_layout_ctx(&modules).unwrap();
        assert!(
            ctx.structs.contains_key("Duo"),
            "aliased struct must be keyed under the local spelling"
        );
        assert!(
            ctx.structs.contains_key("Pair"),
            "exporter's own spelling remains for the exporter module"
        );
        assert!(
            ctx.enums.contains_key("Hue"),
            "aliased enum must be keyed under the local spelling"
        );
        assert!(ctx.enums.contains_key("Color"));
        assert_eq!(
            ctx.struct_field_names.get("Duo").map(|v| v.as_slice()),
            Some(["a".to_string(), "b".to_string()].as_slice())
        );
        // size_of under the local spelling must succeed — the codegen miss
        // that motivated this item.
        let duo = crate::sema::types::Type::Named("Duo".to_string(), vec![]);
        assert_eq!(mwir::size_of(&duo, &ctx), Ok(16));
        let hue = crate::sema::types::Type::Named("Hue".to_string(), vec![]);
        assert_eq!(mwir::size_of(&hue, &ctx), Ok(8));
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
        // No async method -> the turn area is exactly the turn record.
        assert_eq!(a.frame_size, crate::codegen::TURN_RECORD_SIZE);
        assert_eq!(tables.ready_queue_capacity, 2); // 1 actor + root
        assert_eq!(tables.group_arena_capacity, 0);
        // plans/M10.md item 0a / item J: the turn area is *reserved* at the
        // uniform stride (here: one turn, raw area 64 -> stride 64), while
        // `a.frame_size` above still reports the raw record size.
        assert_eq!(tables.n_turns, 1);
        assert_eq!(tables.turn_stride, 64);
        let expect_total = a.state_size + a.mailbox_capacity as u64 * a.slot_size + 24 /* head/tail/count */
                + tables.n_turns * tables.turn_stride
                + tables.ready_queue_capacity * 8
                + 8; // rr cursor
        assert_eq!(tables.total_bytes, expect_total);
    }

    /// plans/M10.md item 0b (decision 554): the turn array is one
    /// contiguous run at `rtdata_base` — `turns_base == base` exactly, each
    /// element one stride from the last, and every owner's state/ring/
    /// bookkeeping placed *after* the whole array rather than interleaved
    /// with it. This is the invariant that makes a `TurnId` an index.
    #[test]
    fn place_runtime_tables_groups_every_turn_into_one_array_at_the_base() {
        let tables = RuntimeTables {
            actors: vec![
                ActorRuntimeLayout {
                    name: "A".to_string(),
                    state_size: 8,
                    mailbox_capacity: 2,
                    slot_size: 16,
                    frame_size: 64,
                },
                ActorRuntimeLayout {
                    name: "B".to_string(),
                    state_size: 8,
                    mailbox_capacity: 2,
                    slot_size: 16,
                    frame_size: 120,
                },
            ],
            free_turns: vec![("f".to_string(), 64)],
            n_turns: 3,
            turn_stride: 128,
            ready_queue_capacity: 3,
            ..RuntimeTables::default()
        };
        let base = 0x4000u64;
        let p = place_runtime_tables(base, &tables);

        // Turns first, contiguous, one stride apart.
        assert_eq!(p.turns_base, base, "`turns_base` is `rtdata_base` itself");
        assert_eq!(p.turn_stride, tables.turn_stride);
        assert_eq!(p.actors[0].turn, base);
        assert_eq!(p.actors[1].turn, base + 128);
        assert_eq!(p.free_turns["f"], base + 256);

        // ...and nothing else is inside the array: the first actor's own
        // state begins immediately past all three turns.
        assert_eq!(p.actors[0].state, base + 3 * 128);
        assert_eq!(p.actors[1].state, p.actors[0].count + 8);

        // `TurnId` is 1-based (decision 567) and `turn_addr` is the one
        // index->address rule the build-time addresses above came from.
        assert_eq!(p.turn_ids["f"].get(), 3);
        assert_eq!(p.turn_ids["f"].index(), 2);
        assert_eq!(TurnId::from_index(0).get(), 1);
        for (key, want) in [("A.tick", base), ("B.tick", base + 128), ("f", base + 256)] {
            let id = p.turn_id_for(key, &tables).expect("a sized turn owner");
            assert_eq!(p.turn_addr(id), want, "{key}");
            assert_eq!(p.turn_area_for(key, &tables), Some(want), "{key}");
        }
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

    // --- plans/M7.md item W: init-argument materialization ----------------

    fn wired(
        label: &str,
        ty: crate::sema::types::Type,
        value: crate::eval::value::Value,
    ) -> crate::eval::image::DeclArg {
        crate::eval::image::DeclArg {
            label: label.to_string(),
            ty,
            value,
        }
    }

    fn decl_with(
        actor_type: &str,
        args: Vec<crate::eval::image::DeclArg>,
    ) -> crate::eval::image::ActorDecl {
        crate::eval::image::ActorDecl {
            actor_type: crate::sema::types::Type::Named(actor_type.to_string(), vec![]),
            args,
        }
    }

    #[test]
    fn an_integer_init_argument_is_its_own_sign_extended_word() {
        use crate::eval::value::Value;
        // `codegen`'s own `Inst::ConstInt` encoding (`load_imm(value as
        // i64)`), restated as an assertion rather than as a comment: a
        // negative argument must arrive sign-extended, or an `i32 -3`
        // becomes 4294967293 in the callee's 8-byte slot.
        let z = HandleSpace::default();
        assert_eq!(boot_init_arg_word(&Value::U8(200), z), Some(200));
        assert_eq!(boot_init_arg_word(&Value::U16(40000), z), Some(40000));
        assert_eq!(boot_init_arg_word(&Value::U64(u64::MAX), z), Some(u64::MAX));
        assert_eq!(
            boot_init_arg_word(&Value::I32(-3), z),
            Some(0xFFFF_FFFF_FFFF_FFFD)
        );
        assert_eq!(boot_init_arg_word(&Value::I64(-1), z), Some(u64::MAX));
        assert_eq!(boot_init_arg_word(&Value::Bool(true), z), Some(1));
        assert_eq!(boot_init_arg_word(&Value::Bool(false), z), Some(0));
        assert_eq!(boot_init_arg_word(&Value::Char('A'), z), Some(65));
        assert_eq!(boot_init_arg_word(&Value::Unit, z), Some(0));
    }

    #[test]
    fn a_handle_init_argument_is_its_own_construction_order_index() {
        use crate::eval::image::ImageDeclRef;
        use crate::eval::value::Value;
        // Shared space: actors, then drivers, then devices (plans/M8.md
        // item H attack 6). With three actors and two drivers, `driver#1`
        // is word 4 and `device#0` is word 5 — never a kind-local 0/1.
        let space = HandleSpace {
            n_actors: 3,
            n_drivers: 2,
        };
        assert_eq!(
            boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Actor(2)), space),
            Some(2)
        );
        assert_eq!(
            boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Driver(1)), space),
            Some(4)
        );
        assert_eq!(
            boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Device(0)), space),
            Some(5)
        );
        // A pool is named, never indexed (`ImageDeclRef`'s own two
        // recording disciplines) — there is no word for it, so it fails
        // closed rather than picking one.
        assert_eq!(
            boot_init_arg_word(
                &Value::ImageDecl(ImageDeclRef::Pool("Buffers".to_string())),
                HandleSpace::default()
            ),
            None
        );
    }

    #[test]
    fn actor_and_driver_handle_words_never_collide() {
        use crate::eval::image::ImageDeclRef;
        use crate::eval::value::Value;
        // plans/M8.md item H attack 6: before the shared space, actor#0
        // and driver#0 both materialised as word 0 — observable through
        // the `u32` `decl.handle()` spelling (`boot-handle-index-distinct`
        // is the HVF witness).
        let space = HandleSpace {
            n_actors: 1,
            n_drivers: 1,
        };
        let actor0 = boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Actor(0)), space);
        let driver0 = boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Driver(0)), space);
        assert_eq!(actor0, Some(0));
        assert_eq!(driver0, Some(1));
        assert_ne!(actor0, driver0);
    }

    #[test]
    fn image_decl_handle_words_are_duplicate_free() {
        use crate::eval::image::ImageDeclRef;
        use std::collections::BTreeSet;
        // Property over a mixed image: every indexed `ImageDeclRef` gets
        // a distinct word. A fourth kind that quietly reused a number
        // would shrink the set relative to the declaration count.
        let space = HandleSpace {
            n_actors: 2,
            n_drivers: 1,
        };
        let decls = [
            ImageDeclRef::Actor(0),
            ImageDeclRef::Actor(1),
            ImageDeclRef::Driver(0),
            ImageDeclRef::Device(0),
            ImageDeclRef::Device(1),
        ];
        let mut words = BTreeSet::new();
        for d in &decls {
            let w = image_decl_handle_word(space, d)
                .unwrap_or_else(|| panic!("indexed kind {d:?} must have a handle word"));
            assert!(
                words.insert(w),
                "duplicate handle word {w} for {d:?} — image_decl_handle_word's \
                 contract is that no two distinct declarations share a word"
            );
        }
        assert_eq!(words.len(), decls.len());
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Pool("Buffers".into())),
            None
        );
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::DmaPool("Payloads".into())),
            None
        );
        // Concrete layout for this space: actors 0..2, driver 2, devices 3..
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Actor(0)),
            Some(0)
        );
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Actor(1)),
            Some(1)
        );
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Driver(0)),
            Some(2)
        );
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Device(0)),
            Some(3)
        );
        assert_eq!(
            image_decl_handle_word(space, &ImageDeclRef::Device(1)),
            Some(4)
        );
    }

    #[test]
    fn resolve_runtime_test_args_uses_the_shared_handle_space() {
        // Same collision, through the `@test(runtime)` handle path item D
        // decision 22 opened: a messageable `driver#0` must not be handed
        // to the root as word 0 when `actor#0` already is.
        let mut graph = ImageGraph::default();
        graph.actors.push(crate::eval::image::ActorDecl {
            actor_type: crate::sema::types::Type::Named("Scale".into(), vec![]),
            args: vec![],
        });
        graph.drivers.push(crate::eval::image::DriverDecl {
            actor_type: crate::sema::types::Type::Named("BlkDriver".into(), vec![]),
            args: vec![crate::eval::image::DeclArg {
                label: "mailbox".into(),
                ty: crate::sema::types::Type::U64,
                value: crate::eval::value::Value::U64(4),
            }],
        });
        let src = "\
module m

@actor
pub struct Scale:
    pub fn get(read self) -> u64:
        return 1

@driver
pub struct BlkDriver:
    pub fn get(read self) -> u64:
        return 2

@test(runtime)
async fn asks_actor(s: Actor[Scale]):
    v = await s.get()
    match v:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, \"rejected\"

@test(runtime)
async fn asks_driver(d: Actor[BlkDriver]):
    v = await d.get()
    match v:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, \"rejected\"
";
        let modules = one_module("m", src);
        let program = crate::sema::check_typed(modules.get("m").unwrap(), "m.wr").expect("types");
        let names = vec!["asks_actor".into(), "asks_driver".into()];
        let args = resolve_runtime_test_args(&program, &names, &graph).expect("resolve");
        assert_eq!(
            args.get("asks_actor").map(|v| v.as_slice()),
            Some([0u64].as_slice())
        );
        assert_eq!(
            args.get("asks_driver").map(|v| v.as_slice()),
            Some([1u64].as_slice())
        );
    }

    #[test]
    fn an_aggregate_or_float_init_argument_has_no_word_at_all() {
        use crate::eval::value::Value;
        let z = HandleSpace::default();
        assert_eq!(boot_init_arg_word(&Value::F64(1.0), z), None);
        assert_eq!(boot_init_arg_word(&Value::Str(b"hi".to_vec()), z), None);
        assert_eq!(
            boot_init_arg_word(&Value::Tuple(vec![Value::U8(1)]), z),
            None
        );
        assert_eq!(
            boot_init_arg_word(&Value::Array(vec![Value::U8(1)]), z),
            None
        );
        assert_eq!(
            boot_init_arg_word(&Value::Struct(vec![Value::U8(1)]), z),
            None
        );
        assert_eq!(boot_init_arg_word(&Value::Enum(0, vec![]), z), None);
    }

    #[test]
    fn init_arguments_are_ordered_by_the_declared_parameter_list_not_the_wiring() {
        use crate::eval::value::Value;
        use crate::sema::types::Type;
        let src = "\
module m

@actor
pub struct Store:
    lo: u8
    hi: u16

    init(mut self, lo: u8, hi: u16):
        self.lo = lo
        self.hi = hi
";
        let modules = one_module("m", src);
        let inits = actor_inits(&modules).unwrap();
        let mut graph = ImageGraph::default();
        // Wired hi-then-lo, with the reserved `mailbox=` in between — the
        // materialized order must still be `lo`, `hi` (the `init`'s own
        // declaration order), and `mailbox` must not become an argument.
        graph.actors.push(decl_with(
            "Store",
            vec![
                wired("hi", Type::U16, Value::U16(40000)),
                wired("mailbox", Type::U32, Value::U32(4)),
                wired("lo", Type::U8, Value::U8(200)),
            ],
        ));
        let (calls, _) = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap();
        let call = calls[0].as_ref().expect("Store declares an `init`");
        assert_eq!(call.key, "Store.init");
        assert_eq!(
            call.args,
            vec![BootInitArg::Word(200), BootInitArg::Word(40000)]
        );
    }

    #[test]
    fn an_actor_with_no_declared_init_gets_no_boot_call() {
        let src = "\
module m

@actor
pub struct Store:
    count: u32
";
        let modules = one_module("m", src);
        let inits = actor_inits(&modules).unwrap();
        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Store", Some(4)));
        let (calls, _) = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap();
        assert!(
            calls[0].is_none(),
            "no `init` means the zero-fill is the whole construction"
        );
    }

    /// plans/M7.md item W's residual, the half no golden reaches: item W
    /// found it through a *handle* (`golden/err-image-field-handle-unmaterialized`
    /// is that one), but a plain scalar wired to a field of a no-`init`
    /// struct is silently zero at boot for exactly the same reason —
    /// `eval::image_checks`' literal-constructor arm accepts it and
    /// nothing materializes it. "Nonzero index" and "nonzero value" are
    /// one rule, and this is the second half of it.
    #[test]
    fn a_field_wired_scalar_must_also_equal_the_state_fills_zero() {
        use crate::eval::image::DeclArg;
        use crate::eval::value::Value;
        use crate::sema::types::Type;

        let src = "\
module m

@actor
pub struct Store:
    seed: u32
";
        let modules = one_module("m", src);
        let inits = actor_inits(&modules).unwrap();

        let wired = |v: Value| {
            let mut d = actor_decl("Store", Some(4));
            d.args.push(DeclArg {
                label: "seed".to_string(),
                ty: Type::I64,
                value: v,
            });
            let mut graph = ImageGraph::default();
            graph.actors.push(d);
            build_boot_init_calls(&graph, &inits, &BTreeMap::new())
        };

        // Exact, not conservative: a wired zero *is* what the state-fill
        // leaves, so there is no disagreement to close.
        assert!(wired(Value::I64(0)).is_ok());

        let err = wired(Value::I64(8)).expect_err("8 is not the state-fill's zero");
        assert!(err.message.contains("materializes as 8"), "{}", err.message);
        assert!(
            err.message.contains("declares no `init`"),
            "{}",
            err.message
        );

        // A value this compiler cannot even show is zero fails too.
        let err = wired(Value::Str(b"x".to_vec())).expect_err("no register representation");
        assert!(
            err.message.contains("no register representation at all"),
            "{}",
            err.message
        );

        // The reserved wiring labels are image metadata, never fields —
        // shared with `eval::image_checks` through one predicate so the
        // two can never disagree about which is which.
        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Store", Some(4)));
        assert!(build_boot_init_calls(&graph, &inits, &BTreeMap::new()).is_ok());
    }

    /// One `init` taking one capability parameter of type `cap_ty`.
    fn cap_init(cap_ty: crate::sema::types::Type) -> BTreeMap<String, ActorInit> {
        use crate::sema::types::{DeclParam, Type};
        use crate::syntax::ast::AccessMode;
        let mut inits = BTreeMap::new();
        inits.insert(
            "Blk".to_string(),
            ActorInit {
                key: "Blk.init".to_string(),
                params: vec![DeclParam {
                    mode: AccessMode::Take,
                    name: "cap".to_string(),
                    ty: cap_ty,
                }],
                ret: Type::Unit,
            },
        );
        inits
    }

    fn named1(name: &str, arg: &str) -> crate::sema::types::Type {
        use crate::sema::types::{Type, TypeArg};
        Type::Named(
            name.to_string(),
            vec![TypeArg::Type(Type::Named(arg.to_string(), vec![]))],
        )
    }

    /// plans/M7.md item H1: **the mint, at the one place it becomes a
    /// word.** A `@driver`'s `DeviceCap[D]` parameter carries no explicit
    /// image argument at all — 05-library.md §9 substitutes it — so the
    /// only thing that can give it a value is the `device=` binding, and
    /// the value is decision 11's: that device's own register-window base.
    ///
    /// Unit-tested rather than golden-tested for the *unresolved* half:
    /// `BootInitArg::DeviceRegsBase` deliberately survives derivation
    /// without an address, because `build_boot_init_calls` runs before any
    /// section is placed. `golden/boot-device-claim` is the other half —
    /// the same argument, resolved, executed and asserted on real
    /// hardware.
    #[test]
    fn a_driver_device_cap_materializes_its_devices_register_window() {
        let inits = cap_init(named1("DeviceCap", "VirtioBlock"));
        let mut graph = ImageGraph::default();
        graph.devices.push(crate::eval::image::DeviceDecl {
            device_type: crate::sema::types::Type::Named("VirtioBlock".to_string(), vec![]),
            args: Vec::new(),
        });
        graph.drivers.push(crate::eval::image::DriverDecl {
            actor_type: crate::sema::types::Type::Named("Blk".to_string(), vec![]),
            args: vec![wired(
                "device",
                crate::sema::types::Type::Named("Image".to_string(), vec![]),
                crate::eval::value::Value::ImageDecl(crate::eval::image::ImageDeclRef::Device(0)),
            )],
        });
        let (actors, drivers) = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap();
        assert!(actors.is_empty());
        let call = drivers[0].as_ref().expect("the driver declares an `init`");
        assert_eq!(call.args, vec![BootInitArg::DeviceRegsBase(0)]);
        // And the resolution step itself: the word is the window's base,
        // never a zero, and a missing window is an internal error rather
        // than a silent one.
        let regs = vec![DeviceRegs {
            device: 0,
            device_type: "VirtioBlock".to_string(),
            driver: "Blk".to_string(),
            base: 0x4050_1234,
            size: 8,
        }];
        assert_eq!(call.args[0].resolve(&regs, &[]).unwrap(), 0x4050_1234);
        let err = call.args[0].resolve(&[], &[]).unwrap_err();
        assert!(
            err.message.contains("no placed register window"),
            "{}",
            err.message
        );
    }

    /// The same parameter on an `img.actor(...)` declaration, which binds
    /// no device at all — a shape `eval::image_checks` rejects first
    /// (`check_capability_substitution`'s own actor arm), restated here so
    /// boot never substitutes a zero if it ever reached this pass alone.
    #[test]
    fn a_device_cap_with_no_device_binding_fails_closed() {
        let inits = cap_init(named1("DeviceCap", "VirtioBlock"));
        let mut graph = ImageGraph::default();
        graph.actors.push(actor_decl("Blk", Some(4)));
        let err = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap_err();
        assert!(err.message.contains("binds no device"), "{}", err.message);
    }

    /// Every capability that is *not* a `DeviceCap[D]`, and every bring-up
    /// state, still fails closed here — the mint is one specific thing,
    /// not "capabilities work now". `Mmio[L]` in particular names where it
    /// really comes from (03 §2/§9's `map_partition`), which is the stale
    /// half decision 10 asked H1 to fix.
    #[test]
    fn every_other_capability_and_every_state_still_fails_closed() {
        for (ty, needle) in [
            (named1("Mmio", "VirtioIrqMmio"), "map_partition"),
            (named1("IrqCap", "V"), "vector="),
            (
                named1("DriverClaimedDevice", "VirtioBlock"),
                "produced by a transition inside the driver",
            ),
        ] {
            let inits = cap_init(ty);
            let mut graph = ImageGraph::default();
            graph.devices.push(crate::eval::image::DeviceDecl {
                device_type: crate::sema::types::Type::Named("VirtioBlock".to_string(), vec![]),
                args: Vec::new(),
            });
            graph.drivers.push(crate::eval::image::DriverDecl {
                actor_type: crate::sema::types::Type::Named("Blk".to_string(), vec![]),
                args: vec![wired(
                    "device",
                    crate::sema::types::Type::Named("Image".to_string(), vec![]),
                    crate::eval::value::Value::ImageDecl(crate::eval::image::ImageDeclRef::Device(
                        0,
                    )),
                )],
            });
            let err = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap_err();
            assert!(err.message.contains(needle), "{}", err.message);
        }
    }

    #[test]
    fn more_than_eight_init_arguments_fails_closed() {
        use crate::eval::value::Value;
        use crate::sema::types::{DeclParam, Type};
        use crate::syntax::ast::AccessMode;
        // `codegen` reaches this shape first in practice (its own
        // prologue refuses a ninth parameter, `error[unimplemented]:
        // codegen for more than 8 call arguments`), so no source golden
        // can pin this arm — it is boot's own restatement of the same
        // register budget, and this test is what keeps it honest. Eight
        // is genuinely reachable and genuinely works: the eighth argument
        // lands in `x8`.
        let params: Vec<DeclParam> = (0..9)
            .map(|i| DeclParam {
                mode: AccessMode::Read,
                name: format!("a{i}"),
                ty: Type::U32,
            })
            .collect();
        let mut inits = BTreeMap::new();
        inits.insert(
            "Wide".to_string(),
            ActorInit {
                key: "Wide.init".to_string(),
                params,
                ret: Type::Unit,
            },
        );
        let args = (0..9)
            .map(|i| wired(&format!("a{i}"), Type::U32, Value::U32(i)))
            .collect();
        let mut graph = ImageGraph::default();
        graph.actors.push(decl_with("Wide", args));
        let err = build_boot_init_calls(&graph, &inits, &BTreeMap::new()).unwrap_err();
        assert!(err.message.contains("at most 8"), "{}", err.message);
    }

    #[test]
    fn boot_init_zero_fills_every_actor_before_it_calls_any_init() {
        // The sequencing guarantee `build_boot_init`'s own doc comment
        // claims, asserted rather than described: with two actors and an
        // `init` on the first, the `BL` must come after *both* state
        // slots are zeroed, so an `init` handed another actor's handle
        // never observes an undefined neighbour.
        let addrs = vec![
            ActorAddrs {
                state: 0x1000,
                ring: 0x2000,
                head: 0x3000,
                tail: 0x3008,
                count: 0x3010,
                turn: 0x4000,
            },
            ActorAddrs {
                state: 0x5000,
                ring: 0x6000,
                head: 0x7000,
                tail: 0x7008,
                count: 0x7010,
                turn: 0x8000,
            },
        ];
        let calls = vec![
            Some(BootInitCall {
                key: "A.init".to_string(),
                args: vec![BootInitArg::Word(7)],
                fallible: false,
                err_msg: None,
            }),
            None,
        ];
        let asm =
            build_boot_init(&addrs, &[], &[8, 8], &[], &calls, &[], &[], &[], 0, None).unwrap();
        let bl_word = asm.relocs.iter().find_map(|r| match r {
            Reloc::Call { word, key } if key == "A.init" => Some(*word),
            _ => None,
        });
        let bl_word = bl_word.expect("the declared `init` is called");
        // Prologue (2) + two actors' zero-fill (4 + 1 each) = 12 words
        // before the first argument load; the call itself is at 12 + 4
        // (the argument) + 4 (`x0`) = 20.
        assert_eq!(bl_word, 20);
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

    // --- pool placement + the decision-5 window oracle --------------------

    use crate::eval::image_checks::PoolBacking;

    fn backing(name: &str, bytes: u64, align: u64, device: Option<usize>) -> PoolBacking {
        PoolBacking {
            name: name.to_string(),
            is_dma: device.is_some(),
            payload: "Hdr".to_string(),
            slots: 1,
            slot_bytes: bytes,
            bytes,
            align,
            device,
        }
    }

    fn backings(list: Vec<PoolBacking>) -> BTreeMap<String, PoolBacking> {
        list.into_iter().map(|b| (b.name.clone(), b)).collect()
    }

    #[test]
    fn pools_are_placed_in_name_order_each_at_its_own_alignment() {
        // Deterministic ordering is a hard requirement
        // (`image.report.deterministic`): the only ordering input is the
        // `BTreeMap` key, so declaration order in the `@image` fn cannot
        // move a window.
        let m = backings(vec![
            backing("Zeta", 3, 1, None),
            backing("Alpha", 5, 8, Some(0)),
            backing("Mid", 2, 2, None),
        ]);
        let (pools, base, size, end) = place_pools(0x1004, &[], &m)
            .expect("no section overlaps")
            .expect("three pools reserve a section");
        assert_eq!(base, 0x1008, "the section itself is 8-byte aligned");
        let names: Vec<&str> = pools.iter().map(|p| p.backing.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Mid", "Zeta"]);
        assert_eq!(pools[0].base, 0x1008); // align 8
        assert_eq!(pools[1].base, 0x100e); // 0x100d rounded up to align 2
        assert_eq!(pools[2].base, 0x1010); // align 1, straight after
        assert_eq!(end, 0x1013);
        assert_eq!(size, end - base);
        // Placing twice cannot disagree with itself.
        assert_eq!(
            place_pools(0x1004, &[], &m).unwrap(),
            Some((pools, base, size, end))
        );
    }

    /// `pooldata` is the last section either image flavor places, so its
    /// base must be past every section already placed. The first draft of
    /// this item got that wrong — `rtdata` used to be the final section
    /// and never advanced the cursor — and the symptom was a `debug_assert`
    /// panic inside serialization, which is silence in a release build.
    #[test]
    fn pool_backing_placed_inside_an_existing_section_is_refused() {
        let m = backings(vec![backing("A", 16, 8, Some(0))]);
        let sections = vec![Section {
            name: "rtdata",
            base: 0x1000,
            size: 0x100,
        }];
        let err = place_pools(0x1080, &sections, &m).expect_err("0x1080 is inside rtdata");
        assert!(
            err.message.contains("inside a section that ends at 0x1100"),
            "{}",
            err.message
        );
        assert!(place_pools(0x1100, &sections, &m).is_ok());
    }

    #[test]
    fn an_image_with_no_pool_reserves_no_pooldata_section() {
        assert_eq!(place_pools(0x1000, &[], &BTreeMap::new()).unwrap(), None);
    }

    /// plans/M7.md decision 5, on the compiler's side: every declared
    /// window is pool backing and nothing else. The three ways that can be
    /// false are each a real rejection, not a debug assertion.
    #[test]
    fn a_pool_window_outside_the_pooldata_section_is_refused() {
        let sections = vec![
            Section {
                name: "rtdata",
                base: 0x1000,
                size: 0x100,
            },
            Section {
                name: "pooldata",
                base: 0x1100,
                size: 0x40,
            },
        ];
        let inside = vec![PoolPlacement {
            backing: backing("A", 0x40, 8, Some(0)),
            base: 0x1100,
        }];
        verify_pool_windows(&sections, &inside).expect("wholly inside its own section");

        // One byte before the section — which is the last byte of
        // `rtdata`, an actor's own state or mailbox.
        let before = vec![PoolPlacement {
            backing: backing("A", 0x40, 8, Some(0)),
            base: 0x10ff,
        }];
        let err = verify_pool_windows(&sections, &before).expect_err("reaches into rtdata");
        assert!(err.message.contains("not inside the `pooldata` section"));

        // Straddling the end.
        let past = vec![PoolPlacement {
            backing: backing("A", 0x41, 8, Some(0)),
            base: 0x1100,
        }];
        let err = verify_pool_windows(&sections, &past).expect_err("runs past the section");
        assert!(err.message.contains("not inside the `pooldata` section"));
    }

    #[test]
    fn two_overlapping_pool_windows_are_refused() {
        let sections = vec![Section {
            name: "pooldata",
            base: 0x1000,
            size: 0x100,
        }];
        let overlapping = vec![
            PoolPlacement {
                backing: backing("A", 0x20, 8, Some(0)),
                base: 0x1000,
            },
            PoolPlacement {
                backing: backing("B", 0x20, 8, None),
                base: 0x1010,
            },
        ];
        let err = verify_pool_windows(&sections, &overlapping).expect_err("A and B overlap");
        assert!(
            err.message.contains("overlapping windows"),
            "{}",
            err.message
        );
    }

    #[test]
    fn placed_windows_with_no_pooldata_section_are_refused() {
        let sections = vec![Section {
            name: "code",
            base: 0x1000,
            size: 0x100,
        }];
        let pools = vec![PoolPlacement {
            backing: backing("A", 0x10, 8, Some(0)),
            base: 0x1000,
        }];
        let err = verify_pool_windows(&sections, &pools).expect_err("no section to be inside of");
        assert!(err.message.contains("reserves no `pooldata` section"));
    }

    /// The report line the VMM actually consumes (plans/M7.md item F's
    /// `parse_report` learned `BlkPool name= base= size=`; plans/M8.md
    /// item P added `device=`): exactly one per *device-reachable* pool,
    /// and none at all for a pool no device can reach. This is the
    /// artifact half of decision 5 — the list of `BlkPool` lines is the
    /// whole of what the VMM maps, and each one names the single device
    /// that may reach it.
    #[test]
    fn only_device_reachable_pools_become_blkpool_windows() {
        let layout = ImageLayout {
            blob: Vec::new(),
            entry: 0x1000,
            sections: vec![Section {
                name: "pooldata",
                base: 0x2000,
                size: 0x30,
            }],
            runtime: None,
            device_regs: Vec::new(),
            irq_host_injects: Vec::new(),
            core_entries: Vec::new(),
            placed_statics: Vec::new(),
            pools: vec![
                PoolPlacement {
                    backing: backing("Control", 0x10, 8, Some(0)),
                    base: 0x2000,
                },
                PoolPlacement {
                    backing: backing("Scratch", 0x20, 8, None),
                    base: 0x2010,
                },
            ],
            blk: None,
        };
        let mut out = String::new();
        render_layout_section(&mut out, &layout);
        let blk: Vec<&str> = out
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("BlkPool "))
            .collect();
        assert_eq!(
            blk,
            vec!["BlkPool name=Control device=device#0 base=0x2000 size=0x10"]
        );
        // Both pools still get their own accounting line (03 §3's five
        // declared facts), device-reachable or not.
        assert_eq!(
            out.lines()
                .filter(|l| l.trim_start().starts_with("Pool name="))
                .count(),
            2
        );
        assert!(out.contains("Pool name=Scratch kind=image"));
        assert!(out.contains("device=none"));
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

        fn call2_at(&self, byte_offset: usize, a0: u64, a1: u64) -> u64 {
            assert!(byte_offset < self.len);
            let f: extern "C" fn(u64, u64) -> u64 =
                unsafe { std::mem::transmute(self.ptr.add(byte_offset)) };
            f(a0, a1)
        }

        /// plans/M10.md item 0c1: `rt_enqueue`'s ABI grew an `x4`
        /// (`Option[CoreId]`), so the admission tests need five arguments.
        fn call5_at(&self, byte_offset: usize, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
            assert!(byte_offset < self.len);
            let f: extern "C" fn(u64, u64, u64, u64, u64) -> u64 =
                unsafe { std::mem::transmute(self.ptr.add(byte_offset)) };
            f(a0, a1, a2, a3, a4)
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

    /// plans/M10.md item B1 / decision 591: a second entry into
    /// `__wrela_abort` with the latch already set skips printing and
    /// lands at the shared halt/continuation tail. First entry sets the
    /// latch and clears it in that same tail.
    #[test]
    fn abort_reentrancy_latch_skips_print_on_second_entry() {
        let ram = HostRam::new(4096 * 4);
        let addrs = HarnessAddrs {
            info_base: ram.base(),
            ring_base: ram.base() + 4096,
            data_base: ram.base() + 4096 * 2,
            exit_mmio_addr: 0,
        };
        // Combined page: [ret stub][abort_fixed][landing ret]
        // append/commit are the same 1-word `ret` at word 0; abort starts
        // at word 1; continuation lands at the final `ret`.
        let ret = encode::enc_ret(30);
        let abort_start = 1usize;
        let abort = build_abort_fixed(&addrs, abort_start, 0, 0, 0, 0);
        // Rodata relocs for FAILED/newline are unresolved here — re-entry
        // never reaches them. First entry would need real rodata; we only
        // exercise the latch-set path's early exit.
        let mut words = vec![ret];
        words.extend(abort.words.iter().copied());
        let land_off = words.len();
        words.push(ret);
        let page = ExecPage::new(&words);
        let land_addr = page.ptr as u64 + (land_off * 4) as u64;
        ram.write_u64(mi::OFF_TEST_CONTINUATION, land_addr);

        // Pre-set latch: second-entry path. Must not touch ring bump,
        // must clear latch, must increment failed.
        ram.write_u64(mi::OFF_ABORT_LATCH, 1);
        ram.write_u64(mi::OFF_TEST_FAILED, 0);
        ram.write_u64(mi::OFF_RING_DATA_BUMP, 42);
        let _ = page.call2_at(abort_start * 4, 0, 0);
        assert_eq!(
            ram.read_u64(mi::OFF_ABORT_LATCH),
            0,
            "tail clears the latch"
        );
        assert_eq!(ram.read_u64(mi::OFF_TEST_FAILED), 1);
        assert_eq!(
            ram.read_u64(mi::OFF_RING_DATA_BUMP),
            42,
            "re-entry must skip console print (ring bump unchanged)"
        );
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
        OFF_TURN_BUSY, OFF_TURN_REPLY, OFF_TURN_RESUME_READY, OFF_TURN_SUSPENDED,
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
        /// plans/M10.md item 0c1: a stand-in two-element turn array —
        /// element 0 is the actor's own turn, element 1 the waker record —
        /// at a power-of-two stride, exactly the shape
        /// `place_runtime_tables` lays down. `Reloc`-free: these tests hand
        /// the base and log2 stride straight to `build_rt_select_and_run`.
        const TURN_STRIDE: u64 = 64;
        const LOG2_TURN_STRIDE: u8 = 6;
        /// The waker's own `TurnId` — 1-based, so element 1 is id 2. This is
        /// what an admission now passes in `x3`, in place of the address.
        const WAKER_ID: u64 = 2;

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
            // Turn-array element 1 (`WAKER_ID`), one stride past the
            // actor's own — a detached record past the turn area proper
            // (`TURN_RECORD_SIZE` is 64, matching this fixture's stride).
            let waker = addrs.turn + Self::TURN_STRIDE;
            ActorFixture { ram, addrs, waker }
        }

        /// The turn array's base — element 0 is this actor's own turn.
        fn turns_base(&self) -> u64 {
            self.addrs.turn
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
            crate::codegen::TURN_RECORD_SIZE,
            f.turns_base(),
            ActorFixture::LOG2_TURN_STRIDE,
            select_start,
        ));

        let page = ExecPage::new(&combined);
        let enqueue_off = enqueue_start * 4;
        let select_off = select_start * 4;

        // Arguments by value, `x1` = arg0 / `x2` = arg1 (plans/M10.md
        // item D0, decision 610); this slot is 24 bytes, so only `x1` is
        // stored.
        assert_eq!(
            page.call5_at(enqueue_off, 0, 10, 0, ActorFixture::WAKER_ID, 0),
            0,
            "first enqueue admitted"
        );
        assert_eq!(
            page.call5_at(enqueue_off, 1, 20, 0, ActorFixture::WAKER_ID, 0),
            0,
            "second enqueue admitted"
        );
        assert_eq!(f.read(addrs.count), 2);

        // A third, over capacity=2: rejected, ring state untouched
        // (02 §9.4: an outcome that did not consume arguments hands them
        // back — the minimal encoding is simply "never mutated").
        assert_eq!(
            page.call5_at(enqueue_off, 0, 30, 0, ActorFixture::WAKER_ID, 0),
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
            crate::codegen::TURN_RECORD_SIZE,
            f.turns_base(),
            ActorFixture::LOG2_TURN_STRIDE,
            select_start,
        ));
        let page = ExecPage::new(&combined);

        assert_eq!(
            page.call5_at(enqueue_start * 4, 0, 7, 0, 0, 0),
            0,
            "send admitted (waker_turn = 0)"
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
            crate::codegen::TURN_RECORD_SIZE,
            f.turns_base(),
            ActorFixture::LOG2_TURN_STRIDE,
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
        // plans/M10.md item 0c1: the slot's waker word is
        // `(waker_turn: u32, waker_core: u32)` — id 2, core 0 (local).
        f.write(addrs.ring + 8, ActorFixture::WAKER_ID);
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
            crate::codegen::TURN_RECORD_SIZE,
            f.turns_base(),
            ActorFixture::LOG2_TURN_STRIDE,
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
            crate::codegen::TURN_RECORD_SIZE,
            f.turns_base(),
            ActorFixture::LOG2_TURN_STRIDE,
            select_start,
        ));
        let page = ExecPage::new(&combined);

        assert_eq!(
            page.call5_at(enqueue_start * 4, 0, 0, 0, ActorFixture::WAKER_ID, 0),
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
        // plans/M10.md item 0c1: a stand-in two-element turn array holding
        // the two waker records — element 0 (`TurnId` 1) and element 1
        // (`TurnId` 2), at a power-of-two stride. The actors' own turn areas
        // are addressed as build-time constants and need not be in it.
        const LOG2_TURN_STRIDE: u8 = 7;
        let turns_base = base + 2048;
        let waker0 = turns_base;
        let waker1 = turns_base + (1 << LOG2_TURN_STRIDE);

        // Hand-seed one no-arg message per actor. The slot's waker word is
        // `(waker_turn: u32, waker_core: u32)`; both cores are 0 (local).
        for (a, id) in [(a0, 1u64), (a1, 2u64)] {
            ram.write_u64(a.ring - base, 0);
            ram.write_u64(a.ring + 8 - base, id);
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
            crate::codegen::TURN_RECORD_SIZE,
            turns_base,
            LOG2_TURN_STRIDE,
            sel0_start,
        ));
        let sel1_start = combined.len();
        combined.extend(build_rt_select_and_run(
            &a1,
            capacity,
            slot_size,
            &[(m1_start, false)],
            crate::codegen::TURN_RECORD_SIZE,
            turns_base,
            LOG2_TURN_STRIDE,
            sel1_start,
        ));
        let run_one_start = combined.len();
        let run_one = build_rt_run_one(
            &[sel0_start, sel1_start],
            &[],
            None,
            cursor_addr,
            run_one_start,
        );
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
