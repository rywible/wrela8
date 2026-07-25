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
/// (`BlkDevice capacity_sectors= features=` / optional `vector=`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlkReport {
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
    /// whose own ambient group has just been cancelled.
    pub turn_areas: Vec<u64>,
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
        turn_areas: vec![0; tables.actors.len() + tables.free_turns.len()],
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
    let mut turn_areas: Vec<u64> = placement.actors.iter().map(|a| a.turn).collect();
    turn_areas.extend(placement.free_turns.values().copied());
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

    for &turn in &g.turn_areas {
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
        a.push(encode::enc_ldr_x_imm(T1, SLOT, OFF_GROUP_OWNER_TURN as u16));
        a.load_imm(T2, turn);
        a.push(encode::enc_cmp_reg(T1, T2, true));
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
    // already resolves an `Actor[T]` handle against `graph.drivers` too — but
    // `compute_runtime_tables` sizes mailboxes for `graph.actors` only, so a
    // messaged driver lands here as well. It gets its own sentence rather than
    // the (wrong) "you never declared it" advice, in the same words
    // `resolve_runtime_test_args` already uses for its own driver floor.
    let declared_driver = graph.is_some_and(|g| {
        g.drivers
            .iter()
            .any(|d| crate::sema::types::render_type(&d.actor_type) == actor)
    });
    if declared_driver {
        return LayoutError::new(format!(
            "this image sends to `{actor}` (an `await` or `send` through an `Actor[{actor}]` \
             handle), but `{actor}` is declared as a `@driver` — driver mailboxes are not \
             wired for messaging yet (M6-D's own floor: only an `img.actor(...)` declaration \
             gets a mailbox and admission routine)"
        ));
    }
    LayoutError::new(format!(
        "this image sends to actor `{actor}` (an `await` or `send` through an \
         `Actor[{actor}]` handle) but never declares a `{actor}` instance — add \
         `img.actor({actor}, mailbox=...)` to the `@image` fn, or remove the call: a \
         handle type with no declared instance has no mailbox to admit into"
    ))
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
            return Err(LayoutError::new(format!(
                "internal error: `Wake` for `{driver}` but that driver has no `@task` \
                 (no wake-pending word was reserved)"
            )));
        };
        let Some(&state_base) = placement.drivers.get(i) else {
            return Err(LayoutError::new(format!(
                "internal error: `@driver` `{driver}` has no placed state"
            )));
        };
        return Ok(state_base + off);
    }
    Err(LayoutError::new(format!(
        "internal error: `Wake` names `{driver}`, which this image never declared as a `@driver`"
    )))
}

/// plans/M7.md item G, decision 12: the vector bit index an `IrqCap` for
/// `@driver` `driver` materializes. Read from the sealed graph's
/// `vector=` on that driver's bound device — the same fact
/// `eval::image_checks::check_vector_bindings` already validated.
fn driver_irq_vector(graph: Option<&ImageGraph>, driver: &str) -> Result<u64, LayoutError> {
    let Some(graph) = graph else {
        return Err(LayoutError::new(format!(
            "internal error: `LoadIrqVector` for `{driver}` needs the sealed image graph, but \
             this layout has none"
        )));
    };
    // Decision 18: `LoadIrqVector` may carry `struct:BlkDriver[DriverMode.Irq]`
    // (instantiation owner) or the bare `BlkDriver`.
    let bare_want = driver
        .strip_prefix("struct:")
        .unwrap_or(driver)
        .split('[')
        .next()
        .unwrap_or(driver);
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
    Err(LayoutError::new(format!(
        "internal error: `LoadIrqVector` names `{driver}`, which this image never declared as a \
         `@driver`"
    )))
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
    if pool.backing.device.is_none() {
        return Err(LayoutError::new(format!(
            "`VirtQueue.configure` consumes pool `{pool_name}`, which is not device-reachable \
             (`img.dma_pool(..., device=...)`); decision 5: only DMA pools are device-reachable"
        )));
    }
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
        .iter()
        .find_map(|d| crate::eval::image_checks::device_vector(&d.args));
    Ok(Some(BlkReport {
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

/// Append the VMM-facing `BlkDevice`/`BlkQueue`/`BlkPool` lines (and the
/// decision-2c accounting fact E1 can honestly derive) for a test-image
/// hand-built report. No-op when `layout.blk` is `None`.
pub fn append_blk_vmm_lines(out: &mut String, layout: &ImageLayout) {
    let Some(blk) = &layout.blk else {
        return;
    };
    out.push_str(&format!(
        "BlkDevice capacity_sectors={} features={:#x}{}\n",
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
    for p in &layout.pools {
        if p.backing.device.is_none() {
            continue;
        }
        out.push_str(&format!(
            "BlkPool name={} base={:#x} size={:#x}\n",
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
    /// `eval::image_checks` already refuses a second binding of the same
    /// device, so this is not a list).
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
    let mut out = Vec::new();
    for module in modules.values() {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("device register windows: {}", e.message)))?;
        out.extend(
            crate::sema::types::declare(&specialized)
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
        Some(b) => RuntimeWiring::derive(b)?,
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
                            return Err(LayoutError::new(format!(
                                "internal error: `Wake` for `{driver}` needs rtdata placement"
                            )));
                        }
                    };
                    let addr = driver_wake_pending_addr(p, t, driver)?;
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
                | Reloc::GroupArenaBase { .. }
                | Reloc::IrqVector { .. }
                | Reloc::WakePending { .. } => {
                    return Err(LayoutError::new(
                        "internal error: the runtime block itself must never emit an \
                         AbortVal/CheckpointService/TurnFrameAddr/GroupArenaBase/\
                         IrqVector/WakePending reloc",
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
    Ok(ImageLayout {
        blob,
        entry: entry_base,
        sections,
        runtime: runtime.cloned(),
        pools,
        device_regs,
        blk: None, // filled by attach_blk_report after layout
        irq_host_injects,
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

/// plans/M7.md item G, decision 18: fold every checked struct
/// instantiation into `LayoutCtx` under its rendered type spelling
/// (`BlkDriver[DriverMode.Irq]`), so `mwir::size_of` can size a mode-
/// specialized driver's state the same way it sizes a plain one.
pub fn enrich_layout_ctx_with_instantiations(
    ctx: &mut LayoutCtx,
    programs: &BTreeMap<String, TypedProgram>,
) {
    use crate::sema::typed::TypedInstantiation;
    for typed in programs.values() {
        for (key, inst) in &typed.instantiations {
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
    // plans/M7.md item E1: capacity is an image-declared build constant.
    // Stamp it onto every TypedProgram before lower so
    // `read_capacity_sectors` can emit it as a ConstInt.
    let capacity = crate::eval::image_checks::blk_capacity_sectors(graph);
    // Decision 18: instantiations must be sizeable before codegen.
    let mut layout_ctx = layout_ctx.clone();
    enrich_layout_ctx_with_instantiations(&mut layout_ctx, programs);
    let layout_ctx = &layout_ctx;
    let mut mwir_programs = Vec::with_capacity(programs.len());
    for typed in programs.values() {
        let mut stamped = typed.clone();
        stamped.blk_capacity_sectors = capacity;
        match crate::lower::lower_program(&stamped) {
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
        match crate::flowwir_lower::lower_program(&stamped) {
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
        Ok(Some(layout))
    })
    .or_else(|e| Err(e.message))
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
/// One `@driver` declaration's own static sizing (plans/M7.md item H1).
/// Deliberately *not* an `ActorRuntimeLayout`: a driver is an actor root
/// (02 §9.1) but M6-D's floor still stands — only an `img.actor(...)`
/// declaration gets a mailbox, a ring and an admission routine
/// (`unresolved_call_target`'s own driver sentence), so a driver has
/// exactly one static fact today, its state bytes. A turn area follows
/// when item G gives a driver a `@task`; a mailbox follows when a driver
/// is messageable. Two half-filled `ActorRuntimeLayout`s would have made
/// both of those look already-decided.
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
    /// plans/M7.md item Z1 (decision 9a): whether this method's own
    /// *declared* reply is an aggregate (`codegen::is_aggregate`, the one
    /// shared ABI predicate — never a copy of it, exactly as
    /// `sema::types::validate_message_shape` already calls it). Such a
    /// method is handed its caller's staging-slot address in `x8` by the
    /// dispatch arm below and writes its reply straight into the awaiting
    /// frame; a scalar-reply method's arm is untouched, down to the word.
    reply_is_aggregate: bool,
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
                    reply_is_aggregate: crate::codegen::is_aggregate(&f.ret),
                    param_sizes,
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

    let mut out: BTreeMap<String, ActorInit> = BTreeMap::new();
    for module in modules.values() {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("actor boot init: {}", e.message)))?;
        let items = crate::sema::types::declare(&specialized)
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
/// A declaration handle (`Value::ImageDecl`) becomes its own
/// construction-order index within its kind — the identical number
/// `build_test_root_args` hands a `@test(runtime)` root for an `Actor[T]`
/// parameter. **What that number is and is not**: nothing in the machine
/// reads a handle's value yet, because `codegen` routes every
/// `await`/`send` statically, by actor type, to that actor's own
/// `rt_enqueue` routine (`codegen::rt_enqueue_symbol`) — so this word is
/// the build-time identity the report already prints as `actor#0`/
/// `driver#0`, not an address and not a mailbox. It is materialized
/// rather than skipped because the guest can store and compare it, and
/// because the day handles become dynamic this is the one place that has
/// to change. A pool reference is named by a string, not an index
/// (`ImageDeclRef`'s own two recording disciplines), so it has no word at
/// all and fails closed.
fn boot_init_arg_word(value: &crate::eval::value::Value) -> Option<u64> {
    use crate::eval::image::ImageDeclRef;
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
        Value::ImageDecl(ImageDeclRef::Device(i))
        | Value::ImageDecl(ImageDeclRef::Driver(i))
        | Value::ImageDecl(ImageDeclRef::Actor(i)) => *i as u64,
        // Every remaining shape is either an aggregate (no register
        // representation: `Struct`/`Tuple`/`Array`/`Enum`/`Str`/`Bytes`),
        // a float (`codegen` has no FP/SIMD encoder subset at all —
        // `Inst::ConstFloat` fails closed for the identical reason), a
        // callable (`Fn`/`Closure` — not a value this machine passes), or
        // a pool handle, which is named rather than indexed.
        Value::F32(_)
        | Value::F64(_)
        | Value::Str(_)
        | Value::Bytes(_)
        | Value::Tuple(_)
        | Value::Array(_)
        | Value::Struct(_)
        | Value::Enum(_, _)
        | Value::Fn(_)
        | Value::Closure { .. }
        | Value::ImageDecl(ImageDeclRef::Pool(_))
        | Value::ImageDecl(ImageDeclRef::DmaPool(_)) => return None,
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
fn check_field_wired_args(
    kind: &str,
    name: &str,
    decl_args: &[crate::eval::image::DeclArg],
) -> Result<(), LayoutError> {
    for a in decl_args {
        if crate::eval::image_checks::is_reserved_actor_arg(&a.label) {
            continue;
        }
        let word = boot_init_arg_word(&a.value);
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
    let Some(init) = inits.get(&name) else {
        check_field_wired_args(kind, &name, decl_args)?;
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
        let Some(word) = boot_init_arg_word(&a.value) else {
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
    if graph.actors.is_empty() && graph.drivers.is_empty() && async_frames.is_empty() {
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

    // plans/M7.md item H1: every declared `@driver` instance's own state
    // bytes, sized by the identical `mwir::size_of` an actor's are — which
    // is only answerable at all since this item taught it that a
    // capability is one word.
    // plans/M7.md item G: a `@task` adds one trailing wake-pending word
    // (sticky bit; mask–arm–recheck for the ISR→bottom-half edge).
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
        drivers.push(DriverRuntimeLayout {
            name,
            state_size,
            wake_pending_off,
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
    for d in &drivers {
        total_bytes += d.state_size;
    }
    for (_, area) in &free_turns {
        total_bytes += area;
    }
    total_bytes += ready_queue_capacity * 8
        + RR_CURSOR_SIZE
        + group_arena_capacity * crate::codegen::GROUP_SLOT_SIZE;

    Ok(Some(RuntimeTables {
        actors,
        drivers,
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
        // plans/M7.md item H1: one line per declared `@driver` instance.
        // Deliberately its own line kind, not an `Actor ...` line with
        // blanks: a driver has no mailbox, no ring and no turn area
        // today (M6-D's floor), and printing `mailbox=0 slot=0 frame=0`
        // would read as three decisions this milestone has not made.
        for d in &tables.drivers {
            push_line(
                out,
                1,
                &format!("Driver name={} state={}", d.name, d.state_size),
            );
        }
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
    // - `BlkPool name= base= size=` is the *mapping* line, and it exists
    //   for device-reachable pools only. It is the exact format
    //   `wrela-vmm`'s own `parse_report` already reads (plans/M7.md item
    //   F), and the list of them is the whole of what that VMM maps for
    //   its device model — decision 5's security property, in the
    //   artifact rather than in a comment: a pool with no `device=` never
    //   produces one, so no device can reach it.
    //
    // An image that declares a device-reachable pool but no queue is not
    // bootable yet, by design: `parse_report` refuses a `BlkPool` line
    // with no `BlkDevice`/`BlkQueue` to bind it to, and those two lines
    // are plans/M7.md item E's to emit. Fail-closed and named, rather
    // than a window mapped for a device model that was never configured.
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
        if p.backing.device.is_none() {
            continue;
        }
        push_line(
            out,
            1,
            &format!(
                "BlkPool name={} base={:#x} size={:#x}",
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
                "BlkDevice capacity_sectors={} features={:#x}{}",
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
    /// plans/M7.md item H1: each declared `@driver` instance's own state
    /// address, in `RuntimeTables::drivers` order. Placed after every
    /// actor's region and before the free-turn areas — a driver's state is
    /// the only region it has, so there is nothing else to interleave.
    pub drivers: Vec<u64>,
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
    let mut drivers = Vec::with_capacity(tables.drivers.len());
    for d in &tables.drivers {
        drivers.push(cursor);
        cursor += d.state_size;
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
        drivers,
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
    start: usize,
    mut call_dispatch: impl FnMut(&mut Asm, usize),
) -> Asm {
    use crate::codegen::{
        BRK_ACTOR_TURN_CANCELLED, OFF_TURN_CUR_METHOD, OFF_TURN_REPLY, OFF_TURN_REPLY_SLOT,
        OFF_TURN_RESUME_READY, OFF_TURN_SUSPENDED, OFF_TURN_WAKER, TURN_RECORD_SIZE,
        TURN_STATUS_CANCELLED, TURN_STATUS_SUSPENDED,
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
            // enqueueing this very message. `x9`/`x10` only: `x0` (self),
            // `x1`/`x2` (args) and `x15` (method index) are all live
            // across this preamble.
            a.load_imm(9, addrs.turn);
            a.push(encode::enc_ldr_x_imm(10, 9, OFF_TURN_WAKER as u16));
            let skip_have_waker = a.skip_placeholder(); // cbnz x10, .have_waker
            a.push(encode::enc_brk(BRK_REPLY_SLOT_NO_WAKER));
            a.patch_cbnz(skip_have_waker, 10);
            a.push(encode::enc_ldr_x_imm(8, 10, OFF_TURN_REPLY_SLOT as u16));
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
            // plans/M6.md item F: `TURN_STATUS_CANCELLED` from an *actor*
            // turn has no delivery channel — the turn record carries one
            // scalar reply word and no error tag, so there is nothing to
            // hand the awaiting turn but a lie. Fail closed, loudly, rather
            // than approximate. Unreachable at M6 by construction: an actor
            // turn's lineage slots are zeroed at fresh selection (above), so
            // its ambient group is only ever one the method opened itself —
            // and that group's owner IS this turn, which
            // `emit_checkpoint_cancellation_test` exempts from termination.
            // The day an actor turn can inherit a caller's lineage, this is
            // the arm that must grow a real `CallError` reply channel.
            a.push(encode::enc_cmp_imm(0, TURN_STATUS_CANCELLED as u16, true));
            let skip_completed = a.skip_placeholder(); // b.ne .completed
            a.push(encode::enc_brk(BRK_ACTOR_TURN_CANCELLED));
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
    rr_cursor_addr: u64,
    start: usize,
) -> Asm {
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
fn build_group_child_poll(
    child_turn_addr: u64,
    child_key: &str,
    group_arena_base: u64,
    child_index: usize,
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
    a.push(encode::enc_ldr_x_imm(10, 12, OFF_GROUP_JOIN_WAITER as u16));
    let skip_no_waiter = a.skip_placeholder(); // cbz x10 -> nothing waiting
    a.load_imm(11, 1);
    a.push(encode::enc_str_x_imm(11, 10, OFF_TURN_RESUME_READY as u16));
    let no_wake = a.abs();
    a.patch_cbnz(skip_still_active, 13);
    a.patch_cbz(skip_no_waiter, 10);
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
    /// `rt_run_one`'s own absolute word index (the entry driver's
    /// scheduler-tick target). Present whenever any glue exists at all.
    rt_run_one_start: usize,
}

fn build_runtime_glue_block(
    tables: &RuntimeTables,
    actor_dispatch: &[(String, Vec<(String, bool, bool)>)],
    placement: &RuntimePlacement,
    // plans/M6.md item F: every static `g.start` call site's own
    // `(callee_key, child_index)` — `BootCtx::group_child_index`, sorted
    // (`BTreeMap`'s own iteration order, CLAUDE.md's determinism rule) so
    // poll-routine placement never depends on hash order.
    group_child_index: &BTreeMap<String, usize>,
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
            a.frame_size,
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
            poll_start,
        );
        cursor += poll_asm.words.len();
        child_poll_starts.push(poll_start);
        asms.push(poll_asm);
    }
    let rt_run_one_start = cursor;
    let run_one_asm = build_rt_run_one(
        &select_starts,
        &child_poll_starts,
        placement.rr_cursor,
        rt_run_one_start,
    );
    asms.push(run_one_asm);
    RuntimeGlue {
        asms,
        symbols,
        rt_run_one_start,
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
            let mut candidates: Vec<String> = Vec::new();
            let mut actor_index: Option<usize> = None;
            for (i, a) in graph.actors.iter().enumerate() {
                if crate::sema::types::render_type(&a.actor_type) == target_name {
                    candidates.push(format!("actor#{i}"));
                    actor_index = Some(i);
                }
            }
            for (i, d) in graph.drivers.iter().enumerate() {
                if crate::sema::types::render_type(&d.actor_type) == target_name {
                    candidates.push(format!("driver#{i}"));
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
            let Some(idx) = actor_index else {
                return Err(format!(
                    "runtime test `{name}`'s own parameter `{}: Actor[{target_name}]` resolves \
                     to a driver, not an actor — driver handles are not yet wired for runtime \
                     tests (M6-D's own floor)",
                    p.name
                ));
            };
            args.push(idx as u64);
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
    fn derive(boot: &BootCtx) -> Result<Option<RuntimeWiring>, LayoutError> {
        let Some(tables) =
            compute_runtime_tables(boot.graph, boot.modules, boot.layout_ctx, boot.async_frames)
                .map_err(LayoutError::new)?
                .filter(|t| t.total_bytes > 0)
        else {
            return Ok(None);
        };
        let shapes = merge_actor_pub_methods(boot.modules, boot.layout_ctx)?;
        let dispatch = tables
            .actors
            .iter()
            .map(|a| {
                let methods = shapes.get(&a.name).cloned().unwrap_or_default();
                let keys = methods
                    .iter()
                    .map(|m| {
                        (
                            format!("{}.{}", a.name, m.name),
                            m.is_async,
                            m.reply_is_aggregate,
                        )
                    })
                    .collect();
                (a.name.clone(), keys)
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
        Ok(Some(RuntimeWiring {
            tables,
            dispatch,
            init_calls,
            driver_init_calls,
            state_sizes,
            driver_state_sizes,
            group_child_index: boot.group_child_index.clone(),
        }))
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
    // plans/M6.md item F #3: `__wrela_deadline_poll`'s own harness-absolute
    // word index, present only for a build with a group arena. Called once
    // per scheduler tick (module doc on `emit_deadline_poll`).
    deadline_poll_word: Option<usize>,
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
        a.bl_to(line_begin_start);
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
    // like every test's own line above. It is also boot's own abort
    // landing (above): an `init` that aborted has already printed and
    // counted its failure, and lands here to report the totals and exit.
    if let Some(marker) = boot_cont_marker {
        let target = a.addr(harness_base);
        a.patch_load_imm(marker, 9, target);
    }
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
    /// `codegen::compute_group_child_indices`' result for this same build
    /// (plans/M6.md item F): every `g.start`-able callee's own fixed
    /// child-slot ordinal — the one fact `build_runtime_glue_block` needs
    /// to build each static call site's own poll routine
    /// (`build_group_child_poll`) alongside the actor glue. Empty for a
    /// build with no `with group(...)` sites at all.
    pub group_child_index: &'a BTreeMap<String, usize>,
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
    // behavior, byte-identical. Derived by `RuntimeWiring::derive`, the
    // one copy `layout_program` uses too (that fn's own module block above
    // has the full reasoning).
    let mut wiring: Option<RuntimeWiring> = match &boot {
        Some(b) => RuntimeWiring::derive(b)?,
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
            | Reloc::GroupArenaBase { .. }
            | Reloc::IrqVector { .. }
            | Reloc::WakePending { .. } => {
                return Err(LayoutError::new(
                    "internal error: the harness section itself must never emit an \
                     AbortFixed/AbortVal/CheckpointService/GroupArenaBase/IrqVector/\
                     WakePending reloc",
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
                            return Err(LayoutError::new(format!(
                                "internal error: `Wake` for `{driver}` needs rtdata placement"
                            )));
                        }
                    };
                    let addr = driver_wake_pending_addr(p, t, driver)?;
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
        assert_eq!(boot_init_arg_word(&Value::U8(200)), Some(200));
        assert_eq!(boot_init_arg_word(&Value::U16(40000)), Some(40000));
        assert_eq!(boot_init_arg_word(&Value::U64(u64::MAX)), Some(u64::MAX));
        assert_eq!(
            boot_init_arg_word(&Value::I32(-3)),
            Some(0xFFFF_FFFF_FFFF_FFFD)
        );
        assert_eq!(boot_init_arg_word(&Value::I64(-1)), Some(u64::MAX));
        assert_eq!(boot_init_arg_word(&Value::Bool(true)), Some(1));
        assert_eq!(boot_init_arg_word(&Value::Bool(false)), Some(0));
        assert_eq!(boot_init_arg_word(&Value::Char('A')), Some(65));
        assert_eq!(boot_init_arg_word(&Value::Unit), Some(0));
    }

    #[test]
    fn a_handle_init_argument_is_its_own_construction_order_index() {
        use crate::eval::image::ImageDeclRef;
        use crate::eval::value::Value;
        assert_eq!(
            boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Actor(2))),
            Some(2)
        );
        assert_eq!(
            boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Driver(1))),
            Some(1)
        );
        assert_eq!(
            boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Device(0))),
            Some(0)
        );
        // A pool is named, never indexed (`ImageDeclRef`'s own two
        // recording disciplines) — there is no word for it, so it fails
        // closed rather than picking one.
        assert_eq!(
            boot_init_arg_word(&Value::ImageDecl(ImageDeclRef::Pool("Buffers".to_string()))),
            None
        );
    }

    #[test]
    fn an_aggregate_or_float_init_argument_has_no_word_at_all() {
        use crate::eval::value::Value;
        assert_eq!(boot_init_arg_word(&Value::F64(1.0)), None);
        assert_eq!(boot_init_arg_word(&Value::Str(b"hi".to_vec())), None);
        assert_eq!(boot_init_arg_word(&Value::Tuple(vec![Value::U8(1)])), None);
        assert_eq!(boot_init_arg_word(&Value::Array(vec![Value::U8(1)])), None);
        assert_eq!(boot_init_arg_word(&Value::Struct(vec![Value::U8(1)])), None);
        assert_eq!(boot_init_arg_word(&Value::Enum(0, vec![])), None);
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
    /// `parse_report` learned `BlkPool name= base= size=`): exactly one
    /// per *device-reachable* pool, and none at all for a pool no device
    /// can reach. This is the artifact half of decision 5 — the list of
    /// `BlkPool` lines is the whole of what the VMM maps.
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
        assert_eq!(blk, vec!["BlkPool name=Control base=0x2000 size=0x10"]);
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
            crate::codegen::TURN_RECORD_SIZE,
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
            crate::codegen::TURN_RECORD_SIZE,
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
            crate::codegen::TURN_RECORD_SIZE,
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
            crate::codegen::TURN_RECORD_SIZE,
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
            crate::codegen::TURN_RECORD_SIZE,
            sel0_start,
        ));
        let sel1_start = combined.len();
        combined.extend(build_rt_select_and_run(
            &a1,
            capacity,
            slot_size,
            &[(m1_start, false)],
            crate::codegen::TURN_RECORD_SIZE,
            sel1_start,
        ));
        let run_one_start = combined.len();
        let run_one = build_rt_run_one(&[sel0_start, sel1_start], &[], cursor_addr, run_one_start);
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
