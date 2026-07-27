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
use wrela_machine::{console, layout as machine_layout};

/// Runtime emission + boot harness (plans/M10.md item K): floor stubs,
/// specialized inject helpers, JIT materializers, and `layout_test_image`.
/// Pure section packing / placement / reports stay in this file.
mod harness;

// Runtime / harness emission (plans/M10.md item K) — re-export so
// `layout::build_*` / `layout::layout_test_image` / census paths stay stable.
pub use harness::{
    DEADLOCK_MSG, EXIT_CODE_ABORT_FIXED, EXIT_CODE_ABORT_VAL, EXIT_CODE_NO_RUNTIME,
    TranscriptBound, build_checkpoint_and_vector_stub, build_checkpoint_and_vector_stub_ex,
    check_transcript_bound, compute_transcript_bound, layout_test_image,
};

#[cfg(test)]
pub(crate) use harness::emitted_a64_census_live_counts;

use harness::{
    append_rodata, build_entry_stub, inject_boot_init_fn, inject_checkpoint_irq_fns,
    inject_rt_cross_core_fns, inject_rt_enqueue_and_dispatch_fns, push_halt,
    reinject_runtime_with_rtconfig,
};

#[cfg(test)]
use harness::Asm;

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
    /// Index into the contiguous `WAKE.wake_pending` array (M12 item D).
    pub wake_drain_index: usize,
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
    /// Every turn area in the image (each actor's, then each messageable
    /// driver's, then each free async fn's — `place_runtime_tables` order)
    /// — the set the delivery half scans to find suspended turns whose own
    /// ambient group has just been cancelled. Each entry is that turn's
    /// build-time `(address, TurnId)` pair: the scan still addresses the
    /// turn record absolutely, but plans/M10.md item 0c2 made
    /// `OFF_GROUP_OWNER_TURN` a `TurnId`, so the owner test compares the id
    /// rather than the address. Both come from the same
    /// `RuntimePlacement::turn_addr` expression, so they can never name
    /// different bytes. plans/M10.md item G / decision 671: omitting
    /// messageable-driver turns was the pre-G defect.
    pub turn_areas: Vec<(u64, TurnId)>,
}

/// `build_checkpoint_and_vector_stub`'s own result: the block's words plus
/// the service entry point a caller must resolve against `section_base`.
pub struct CheckpointBlock {
    pub words: Vec<u32>,
    /// `__wrela_checkpoint_service`'s own word offset within `words`.
    /// After M11 I this is `0` (floor trampoline; vector0 lives in `code`).
    pub checkpoint_service_word: usize,
    /// Always `None` after M11 item E — poll lives in `code`.
    pub deadline_poll_word: Option<usize>,
    /// Entry driver should `bl_call_key("__wrela_deadline_poll")`.
    pub has_deadline_poll: bool,
    /// `Reloc::Call` sites for ISR / `@task` / deadline-scan bodies (word
    /// offsets relative to the block start when built with `Asm::new(0)`).
    pub relocs: Vec<Reloc>,
}

/// The shape-only (`base = 0`) service context a sizing pass needs: the
/// arena capacity and the *number* of turn areas are both build-time facts,
/// known long before placement, and they are the only things the emitted
/// word count depends on.
fn group_service_shape(runtime: Option<&RuntimeTables>) -> Option<GroupServiceCtx> {
    let tables = runtime.filter(|t| t.group_arena_capacity > 0)?;
    // plans/M10.md item G / decision 671: same owner set as
    // `place_runtime_tables` — actors, then messageable drivers, then free
    // turns. Word count depends only on the length.
    let n_driver_turns = tables
        .drivers
        .iter()
        .filter(|d| d.mailbox.is_some())
        .count();
    let n = tables.actors.len() + n_driver_turns + tables.free_turns.len();
    Some(GroupServiceCtx {
        arena_base: 0,
        arena_capacity: tables.group_arena_capacity,
        // Shape only: the emitted word count depends on the *number* of
        // turn areas, never on any address or id value (every `load_imm`
        // is a fixed four words). `TurnId::from_index(0)` is a stand-in
        // for exactly that reason — the real ids arrive with
        // `group_service_ctx` below.
        turn_areas: vec![(0, TurnId::from_index(0)); n],
    })
}

/// The real service context, once `rtdata` is placed: every turn area in
/// the image (each actor's, then each messageable driver's, then each free
/// async fn's — `place_runtime_tables`'s own byte order) plus the group
/// arena's own base.
fn group_service_ctx(
    placement: &RuntimePlacement,
    tables: &RuntimeTables,
) -> Option<GroupServiceCtx> {
    if tables.group_arena_capacity == 0 {
        return None;
    }
    // plans/M10.md item G / decision 671: an actor's `TurnId` is its
    // `tables.actors` index; a messageable driver's is `actors.len()` plus
    // its rank among messageable drivers; a free turn's is the one
    // `place_runtime_tables` recorded in `turn_ids`. Order matches the
    // turn array (and the shape pass's length). Omitting drivers was the
    // pre-G defect: a messageable driver's parked turn was never
    // force-resumed by the deadline delivery scan.
    let mut turn_areas: Vec<(u64, TurnId)> = placement
        .actors
        .iter()
        .enumerate()
        .map(|(i, a)| (a.turn, TurnId::from_index(i)))
        .collect();
    let mut next_turn = tables.actors.len();
    for (i, d) in tables.drivers.iter().enumerate() {
        if d.mailbox.is_none() {
            continue;
        }
        let Some(addrs) = placement.driver_mailboxes.get(&i) else {
            continue;
        };
        turn_areas.push((addrs.turn, TurnId::from_index(next_turn)));
        next_turn += 1;
    }
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

// --- section packing helpers ---------------------------------------------

fn round_up(n: u64, align: u64) -> u64 {
    n.div_ceil(align) * align
}

/// Steer the placement cursor to `RTDATA_BASE` after the packed
/// entry/code/rodata/abort/checkpoint (and optional rtcode) run.
/// Fails closed if that run would overrun the fixed base, or if the
/// tables alone exceed `RTDATA_SIZE_MAX` (mailbox blowup ceiling).
/// plans/M11.md item C / decisions 750–753.
///
/// plans/M12.md item C (decisions 875–879): `tables.total_bytes` already
/// folds uniform ring-data padding; the diagnostic names that padding so
/// a blowup is measurable rather than opaque. Offset-table fallback is a
/// human gate (decision 2), never taken here.
fn steer_rtdata_base(cursor: u64, tables: &RuntimeTables) -> Result<u64, LayoutError> {
    if tables.total_bytes > machine_layout::RTDATA_SIZE_MAX {
        return Err(LayoutError::new(format!(
            "rtdata needs {} bytes (rings padding {}), which exceeds RTDATA_SIZE_MAX ({})",
            tables.total_bytes,
            tables.rings_padding,
            machine_layout::RTDATA_SIZE_MAX
        )));
    }
    let packed_end = round_up(cursor, 8);
    if packed_end > machine_layout::RTDATA_BASE {
        return Err(LayoutError::new(format!(
            "sections before rtdata end at {packed_end:#x}, past RTDATA_BASE ({:#x})",
            machine_layout::RTDATA_BASE
        )));
    }
    Ok(machine_layout::RTDATA_BASE)
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

/// plans/M8.md item C2 / M10 F2 / M11 G: `rt_xsend` redirect became a
/// fixed trampoline `__wrela_xsend_<edge>` (decision 804) whose body is
/// generic wrela over ring facts. One symbol per request-ring edge index.
fn xsend_trampoline(edge: usize) -> String {
    format!("__wrela_xsend_{edge}")
}

fn xreply_trampoline(edge: usize) -> String {
    format!("__wrela_xreply_{edge}")
}

/// plans/M8.md item C2 / M10 F2: which core a call site runs on. An actor method
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
    // M10 F2: specialized runtime bodies (`rt_drain`, `rt_run_one`, …)
    // `Reloc::Call` `rt_enqueue` to admit into a *local* mailbox after
    // draining a request ring. Their keys are synthetic (space-bearing) and
    // are not turn owners — `caller_core` would fall back to 0 and wrongly
    // redirect the Call to `rt_xsend`, re-publishing into the request ring
    // (SPSC witness fault: ring grows while the consuming core alone runs).
    // Only source fns' Calls are candidates for the cross-core redirect;
    // hand-asm drain never hit this path (glue `bl_call_key`, not
    // `Reloc::Call` through `program.fns`).
    // M11 G: generic `__wrela_rt_drain` / `__wrela_try_enqueue` are the
    // same shape — not space-bearing, but must not redirect either.
    if crate::codegen::symbol_is_synthetic(caller_key)
        || caller_key.starts_with("__wrela_")
        || caller_key.starts_with("__enqueue_")
        || caller_key.starts_with("__method_")
        || caller_key.starts_with("__resume_")
    {
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
    // M11 G (decision 804): trampoline `__wrela_xsend_<edge>` when rings
    // are already installed; during `cross_core_edges` / `cross_core_rings`
    // the ring set is still empty — return a stable sentinel so edge
    // discovery still works, and the Call-patch path (after
    // `add_cross_core_rings`) re-resolves to the real trampoline.
    let edge = w.tables.rings.iter().enumerate().find_map(|(i, r)| {
        if r.kind == RingKind::Request
            && r.src == caller
            && r.actor.as_deref() == Some(target_actor)
        {
            Some(i)
        } else {
            None
        }
    });
    match edge {
        Some(edge) => {
            if edge >= crate::rtconfig::RING_POOL_COUNT {
                return Err(LayoutError::new(format!(
                    "image needs request-ring edge {edge}; trampoline pool is {} (plans/M11.md decision 802)",
                    crate::rtconfig::RING_POOL_COUNT
                )));
            }
            Ok(Some(xsend_trampoline(edge)))
        }
        None => {
            // Pre-ring-install discovery, or a missing ring after install.
            if w.tables.rings.is_empty() {
                Ok(Some(format!(
                    "__wrela_xsend_pending {caller} {target_actor}"
                )))
            } else {
                Err(LayoutError::new(format!(
                    "internal error: cross-core edge {caller} -> `{target_actor}` has no request ring"
                )))
            }
        }
    }
}

/// M11 G (decision 804): `rt_xreply src->dst` Call keys resolve to
/// `__wrela_xreply_<edge>` trampolines. Returns `None` when `target` is
/// not an xreply symbol.
fn resolve_xreply_edge(target: &str, w: &RuntimeWiring) -> Option<String> {
    let (src, dst) = crate::codegen::rt_xreply_cores(target)?;
    let edge = w.tables.rings.iter().enumerate().find_map(|(i, r)| {
        if r.kind == RingKind::Reply && r.src == src && r.dst == dst {
            Some(i)
        } else {
            None
        }
    })?;
    if edge >= crate::rtconfig::RING_POOL_COUNT {
        return None;
    }
    Some(xreply_trampoline(edge))
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
                debug_assert!(
                    sym.starts_with("__wrela_xsend_"),
                    "cross-core redirect must be an xsend trampoline, got {sym}"
                );
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

/// Resolve mailbox root `name`'s full placed addresses from a live
/// placement (plans/M10.md item D / decision 614; item F / decision 631
/// needs `state` / `turn` / `head` too).
fn resolve_mailbox_actor_addrs(
    placement: &RuntimePlacement,
    tables: &RuntimeTables,
    name: &str,
) -> Option<ActorAddrs> {
    if let Some((i, _)) = tables
        .actors
        .iter()
        .enumerate()
        .find(|(_, a)| a.name == name)
    {
        return placement.actors.get(i).copied();
    }
    for (i, d) in tables.drivers.iter().enumerate() {
        if d.mailbox.is_some() && d.name == name {
            return placement.driver_mailboxes.get(&i).copied();
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
            if tables.drivers.get(di).is_some_and(|d| d.has_wake) {
                for task in driver_task_method_names(boot.modules, driver) {
                    let wake_drain_index = wake_drains.len();
                    wake_drains.push(WakeDrainEntry {
                        driver_state: state,
                        wake_drain_index,
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

/// plans/M7.md item G / M12 item D: absolute address of `@driver`
/// `driver`'s sticky wake-pending word in the contiguous `WAKE` array.
/// First drain index for that driver (shared-bit semantics when a driver
/// declares multiple `@task`s).
fn driver_wake_pending_addr(
    _placement: &RuntimePlacement,
    tables: &RuntimeTables,
    driver: &str,
) -> Result<u64, LayoutError> {
    for d in &tables.drivers {
        // Decision 18: runtime table names are rendered
        // (`BlkDriver[DriverMode.Irq]`); `Inst::Wake` carries the bare
        // struct name from the FnRef.
        let bare = d.name.split('[').next().unwrap_or(d.name.as_str());
        if d.name != driver && bare != driver {
            continue;
        }
        let Some(idx) = d.wake_drain_index else {
            // Unreachable from source: `sema` rejects `wake(D.m)` when `m`
            // is not `@task` (`golden/err-wake-not-task`), and only a
            // `@task` reserves a wake-pending drain slot.
            return Err(LayoutError::new(format!(
                "internal error: `Wake` for `{driver}` but that driver has no `@task` \
                 (no wake-pending drain was reserved)"
            )));
        };
        let Some(&addr) = tables.wake_pending_addrs.get(idx) else {
            return Err(LayoutError::new(format!(
                "internal error: `@driver` `{driver}` wake drain {idx} has no WAKE address"
            )));
        };
        return Ok(addr);
    }
    // Author-reachable: a `@driver` with `wake(...)` compiled into the
    // module, while this `@image` never declared that driver (sibling of
    // `irq_driver_undeclared` / the LoadIrqVector soak find).
    Err(wake_driver_undeclared(driver))
}

/// plans/M10.md item H / decision 682: `@driver` `driver`'s placed state
/// base — the same name resolution `WakePending` uses, without the
/// wake-pending offset.
fn driver_state_addr(
    placement: &RuntimePlacement,
    tables: &RuntimeTables,
    driver: &str,
) -> Result<u64, LayoutError> {
    for (i, d) in tables.drivers.iter().enumerate() {
        let bare = d.name.split('[').next().unwrap_or(d.name.as_str());
        if d.name != driver && bare != driver {
            continue;
        }
        let Some(&state_base) = placement.drivers.get(i) else {
            return Err(LayoutError::new(format!(
                "internal error: `@driver` `{driver}` has no placed state"
            )));
        };
        return Ok(state_base);
    }
    Err(LayoutError::new(format!(
        "internal error: Reloc::DriverState names `@driver` `{driver}`, which this image's \
         runtime tables never placed"
    )))
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
        // Gaps wider than alignment padding are refused, with one
        // steered exception: `rtdata` sits at the fixed `RTDATA_BASE`
        // (plans/M11.md item C). Layout advances the cursor to that
        // address after packing entry..checkpoint; the gap is the
        // deliberate packing window, not drift. Any other >=8-byte gap
        // still means the section table and placement have diverged.
        let gap = b.base - a_end;
        let steered_rtdata = b.name == "rtdata"
            && b.base == machine_layout::RTDATA_BASE
            && a_end <= machine_layout::RTDATA_BASE;
        if gap >= 8 && !steered_rtdata {
            return Err(LayoutError::new(format!(
                "internal error: a {gap}-byte gap between section `{}` and `{}` exceeds every                  alignment this module ever rounds to",
                a.name, b.name
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
    // plans/M12.md item C: uniform-stride summary — the VMM needs
    // `stride` to derive CTL addresses and overlap spans under
    // CTL-then-DATA packing. Absent when this image has no rings.
    if let Some(tables) = &layout.runtime {
        if !tables.rings.is_empty() {
            out.push_str(&format!(
                "Rings count={} stride={} padding={} bytes={}\n",
                tables.rings.len(),
                tables.ring_stride,
                tables.rings_padding,
                rings_reservation_bytes(&tables.rings)
            ));
        }
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
    let layouts = closure_layout_types(b.modules, b.programs)?;
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
/// exact-bytes section: `types::check_layouts` is a pure function of one
/// specialized module, then `types::complete_layouts` finishes any
/// `runtime` layout whose array length is a `const` name (plans/M10.md
/// item E1, carried from A2b / decision 581). Without that second pass a
/// deferred layout has `size: None` and every `require_size` consumer
/// rejects rather than lying — correct, but unusable the moment a
/// `runtime` table with a const length reaches this path (E3/E4).
fn closure_layout_types(
    modules: &BTreeMap<String, Module>,
    programs: &BTreeMap<String, TypedProgram>,
) -> Result<BTreeMap<String, crate::sema::types::LayoutType>, LayoutError> {
    let mut out = BTreeMap::new();
    for (key, module) in modules {
        let specialized = crate::sema::specialize::specialize(module)
            .map_err(|e| LayoutError::new(format!("pool backing: {}", e.message)))?;
        let mut layouts = crate::sema::types::check_layouts(&specialized)
            .map_err(|e| LayoutError::new(format!("pool backing: {}", e.message)))?;
        let Some(program) = programs.get(key) else {
            return Err(LayoutError::new(format!(
                "internal error: module `{key}` is in the build closure without a typed program, \
                 so `complete_layouts` cannot resolve a `@layout(runtime)` const array length \
                 (plans/M10.md item E1)"
            )));
        };
        crate::sema::types::complete_layouts(&specialized, program, &mut layouts)
            .map_err(|e| LayoutError::new(format!("pool backing: {}", e.message)))?;
        for l in layouts {
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
    let layouts = closure_layout_types(b.modules, b.programs)?;
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

    // plans/M7.md item E1 / M10 H: fallible-`init` abort messages must be
    // interned before `inject_boot_init_fn` so `emit_boot_init` sees
    // `err_msg` offsets (decision 680).
    let mut rodata_entries: Vec<Vec<u8>> = program.rodata.clone();
    let mut rodata_cursor: usize = rodata_entries.iter().map(Vec::len).sum();
    if let Some(w) = wiring.as_mut() {
        intern_fallible_init_abort_messages(w, &mut rodata_entries, &mut rodata_cursor);
    }

    // M10 E3/E4/F/F2/H + M11 E/F: specialized runtime bodies into code;
    // reinject deadline/run_one/child_poll against live RT/GROUPS first.
    let mut program_owned;
    let program = if let Some(w) = wiring.as_ref() {
        program_owned = program.clone();
        reinject_runtime_with_rtconfig(&mut program_owned, w)?;
        inject_rt_enqueue_and_dispatch_fns(&mut program_owned, w);
        inject_rt_cross_core_fns(&mut program_owned, w);
        inject_boot_init_fn(&mut program_owned, w);
        inject_checkpoint_irq_fns(&mut program_owned, w);
        &program_owned
    } else {
        program
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
    let rodata_bytes: Vec<u8> = rodata_entries
        .iter()
        .flat_map(|entry| entry.iter().copied())
        .collect();
    let runtime: Option<&RuntimeTables> = wiring.as_ref().map(|w| &w.tables);

    // plans/M10.md item C / decision 655: delete the named abort-stub
    // builders. Build images still need a landing address for
    // Reloc::AbortFixed/AbortVal (assert / bounds); keep a minimal halt
    // with distinct exit codes for post-mortem, not the test print path.
    let mut abort_fixed_words = Vec::new();
    push_halt(&mut abort_fixed_words, EXIT_CODE_ABORT_FIXED);
    let mut abort_val_words = Vec::new();
    push_halt(&mut abort_val_words, EXIT_CODE_ABORT_VAL);
    // plans/M6.md item F: the checkpoint block's own vector-0 body is the
    // real deadline scan whenever this build has a group arena, so it needs
    // already-placed `rtdata` addresses — which are not known until after
    // this very block's own size fixes `cursor`. Built twice, exactly like
    // the runtime glue block below: once with a shape-only placeholder
    // context purely to learn the word count (never address-dependent), then
    // again with the real addresses once `rtdata_base` exists.
    let checkpoint_shape = group_service_shape(runtime);
    let (irq_shape, wake_shape) = checkpoint_irq_shape(boot.as_ref(), None, runtime);
    let link_cp_body = runtime.is_some();
    let checkpoint_block = build_checkpoint_and_vector_stub_ex(
        checkpoint_shape.as_ref(),
        &irq_shape,
        &wake_shape,
        link_cp_body,
    );
    let checkpoint_words = checkpoint_block.words;
    let checkpoint_service_word = checkpoint_block.checkpoint_service_word;
    let checkpoint_relocs_shape = checkpoint_block.relocs;

    // M10 H: boot_init lives in `code` under `rt_boot_init 0` (decision
    // 680). Glue is empty after F2; the `rtcode` section is absent.
    let rtcode_words_len = 0usize;

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

    // --- rtdata (plans/M6.md item C, decision 3; M11 item C / 722):
    // reserved, zeroed bytes for this image's own static actor runtime
    // tables at the fixed `RTDATA_BASE` — absent entirely when `runtime`
    // is `None` (no actors), never a zero-size placeholder section. ----
    let rtdata_base = if let Some(tables) = runtime.filter(|t| t.total_bytes > 0) {
        let base = steer_rtdata_base(cursor, tables)?;
        cursor = base;
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
                true,
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
    // M10 H/K: no `rtcode` section (boot_init in `code`; glue deleted in K).
    let empty_symbols = BTreeMap::new();
    let glue_symbols: &BTreeMap<String, usize> = &empty_symbols;
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
                    // plans/M8.md item C2 / M11 G: a cross-core enqueue
                    // resolves to `__wrela_xsend_<edge>`; an `rt_xreply`
                    // Call resolves to `__wrela_xreply_<edge>` (decision 804).
                    let redirect = resolve_cross_core_edge(key, target, wiring.as_ref())?;
                    let xreply = wiring.as_ref().and_then(|w| resolve_xreply_edge(target, w));
                    let target_owned: String =
                        redirect.or(xreply).unwrap_or_else(|| target.clone());
                    let target = target_owned.as_str();
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
                    let addrs = resolve_mailbox_actor_addrs(p, t, actor).ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: Reloc::MailboxAddr names actor `{actor}`, which this \
                             image's runtime tables never placed a mailbox for"
                        ))
                    })?;
                    let addr = match field {
                        crate::codegen::MailboxField::Ring => addrs.ring,
                        crate::codegen::MailboxField::Head => addrs.head,
                        crate::codegen::MailboxField::Tail => addrs.tail,
                        crate::codegen::MailboxField::Count => addrs.count,
                        crate::codegen::MailboxField::State => addrs.state,
                        crate::codegen::MailboxField::Turn => addrs.turn,
                    };
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                // M10 E3 / decision 621
                Reloc::RrCursor { word, core } => {
                    let p = placement.as_ref().ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::RrCursor exists but this image has no \
                             runtime placement",
                        )
                    })?;
                    let addr = p.rr_cursors.get(*core).copied().ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: Reloc::RrCursor names core {core}, but this image \
                             only placed {} rr_cursor(s)",
                            p.rr_cursors.len()
                        ))
                    })?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                // M10 F2 / decision 634
                Reloc::RingAddr {
                    word,
                    ring_index,
                    field,
                } => {
                    let p = placement.as_ref().ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::RingAddr exists but this image has no \
                             runtime placement",
                        )
                    })?;
                    let addrs = p.rings.get(*ring_index).copied().ok_or_else(|| {
                        LayoutError::new(format!(
                            "internal error: Reloc::RingAddr names ring_index {ring_index}, but \
                             this image only placed {} ring(s)",
                            p.rings.len()
                        ))
                    })?;
                    let addr = match field {
                        crate::codegen::RingField::Ring => addrs.ring,
                        crate::codegen::RingField::Head => addrs.head,
                        crate::codegen::RingField::Tail => addrs.tail,
                        crate::codegen::RingField::Count => addrs.count,
                    };
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                // M10 H / decision 682
                Reloc::DriverState { word, driver } => {
                    let (p, t) = match (placement.as_ref(), runtime_live) {
                        (Some(p), Some(t)) => (p, t),
                        _ => {
                            return Err(LayoutError::new(
                                "internal error: a Reloc::DriverState exists but this image has \
                                 no runtime tables",
                            ));
                        }
                    };
                    let addr = driver_state_addr(p, t, driver)?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                // M10 H / decision 683
                Reloc::DeviceRegsBase { word, device } => {
                    let addr = device_regs
                        .iter()
                        .find(|r| r.device == *device)
                        .map(|r| r.base)
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: Reloc::DeviceRegsBase names device#{device}, \
                                 which this image never placed"
                            ))
                        })?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::PoolBase { word, pool } => {
                    let addr = pools
                        .iter()
                        .find(|p| &p.backing.name == pool)
                        .map(|p| p.base)
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: Reloc::PoolBase names pool `{pool}`, which this \
                                 image never placed"
                            ))
                        })?;
                    patch_load_imm_words(&mut all_code_words, base + word, addr);
                }
                Reloc::PoolSlot {
                    word,
                    pool,
                    index,
                    slot_bytes,
                } => {
                    let base_addr = pools
                        .iter()
                        .find(|p| &p.backing.name == pool)
                        .map(|p| p.base)
                        .ok_or_else(|| {
                            LayoutError::new(format!(
                                "internal error: Reloc::PoolSlot names pool `{pool}`, which this \
                                 image never placed"
                            ))
                        })?;
                    let addr = base_addr + *index * *slot_bytes;
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
    let rtcode_words: Vec<u32> = Vec::new();

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
    // M10 F2: secondary-core entries live in `code` under
    // `rt_secondary_core_entry <core>` (decision 633). Resolve against
    // `fn_word_base` / `code_base`, not glue/`rtcode`.
    let core_entries: Vec<(usize, u64)> = match (wiring.as_ref(), code_base) {
        (Some(w), cb) if w.tables.cores > 1 => (1..w.tables.cores)
            .filter_map(|core| {
                let key = crate::codegen::rt_secondary_core_entry_symbol(core);
                fn_word_base
                    .get(&key)
                    .map(|&word| (core, cb + (word as u64) * 4))
            })
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
        Ok((m, _)) => m,
        Err(_) => return Ok(None),
    };
    layout_program(
        &codegen_program,
        Some(BootCtx {
            graph,
            modules,
            programs,
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
    let mut seen = BTreeSet::new();
    for prog in programs.values() {
        for (name, s) in &prog.statics {
            // Imported `@placed` statics are spliced into every importer's
            // `statics` map (sema); emit each name once.
            if !seen.insert(name.clone()) {
                continue;
            }
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
    /// the instance lives, exactly like an actor's `state_size`.
    pub state_size: u64,
    /// True when the driver declares a `@task`. Sticky wake-pending bits
    /// live in the contiguous `WAKE.wake_pending` array (M12 item D /
    /// decisions 880–882), not as a trailing word of driver state.
    pub has_wake: bool,
    /// Index into `wake_pending_addrs` / `WAKE.wake_pending` for this
    /// driver's first `@task` drain. Filled by `fill_checkpoint_irq_facts`.
    pub wake_drain_index: Option<usize>,
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
    /// plans/M12.md item F (decisions 886–889): image
    /// `GROUP_MAX_CHILDREN` fact — `max(FLOOR=2, max g.start children
    /// over group sites)`. Drives `GROUP_SLOT_SIZE = 64 + N*16` and the
    /// generated `GroupSlot` child fields. Floor 2 for empty-arena images.
    pub group_max_children: usize,
    /// plans/M8.md item C2: this image's own cross-core SPSC rings, in
    /// `cross_core_rings`'s canonical order. Empty for every single-core
    /// image and for a cross-core image whose graph has no cross-core
    /// message edge (decision 28's own "emit nothing" rule).
    pub rings: Vec<RingLayout>,
    /// plans/M12.md item C (decision 875): image-wide uniform ring-data
    /// stride in bytes (`max(capacity * slot_size)`), or `0` when there
    /// are no rings. Padding cost is `rings_padding`.
    pub ring_stride: u64,
    /// plans/M12.md item C: `n_rings * ring_stride - sum(capacity *
    /// slot_size)` — bytes spent to buy a uniformly-strided type. Printed
    /// on the report's `Rings` line; folded into `total_bytes` before
    /// `steer_rtdata_base` so `RTDATA_SIZE_MAX` fails closed with the
    /// number.
    pub rings_padding: u64,
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
    /// M11 F (decision 790): mailbox-root names per live core, in
    /// `mailbox_root_names` order filtered by placement — the RR select
    /// list. Empty until `RuntimeWiring::derive` fills them.
    pub select_by_core: Vec<Vec<String>>,
    /// M11 F: `true` when this core has any inbound cross-core ring.
    pub drain_by_core: Vec<bool>,
    /// M11 F: `(callee_key, child_index, turn_index)` per `g.start` site,
    /// in `BTreeMap` key order.
    pub child_sites: Vec<(String, usize, usize)>,
    /// M11 G (decision 801): image handle word per request ring (parallel
    /// to `rings`); 0 / unused for reply rings.
    pub ring_target_handles: Vec<u64>,
    /// M11 G: mailbox-root handle words in root order (enqueue stubs).
    pub enqueue_handles: Vec<u64>,
    /// M11 G: mailbox-root names parallel to `enqueue_handles`.
    pub enqueue_actors: Vec<String>,
    /// M11 J: per root (enqueue_actors order): `(method_key, is_async, reply_is_aggregate)`.
    pub root_methods: Vec<Vec<(String, bool, bool)>>,
    /// M11 J: placement core per root (enqueue_actors order).
    pub root_cores: Vec<usize>,
    /// M11 H: count of boot `init` calls (drivers then actors with `init`).
    pub n_boot_calls: usize,
    /// M11 I: pending-vector bit indices for sealed IRQ binds (decision 823).
    pub irq_vector_bits: Vec<u64>,
    /// M11 I: absolute wake-pending word addresses (decision 823).
    pub wake_pending_addrs: Vec<u64>,
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

    /// plans/M8.md item C2 / M12 item C: installs this image's own
    /// cross-core rings and grows `total_bytes` by their **padded**
    /// reservation (all CTLs, then uniformly-strided DATA). Called once,
    /// by `RuntimeWiring::derive`, right after `stripe_for_cores` — the
    /// rings are placed **last** in `rtdata` (after the group arena), so
    /// nothing an existing golden pins moves for an image that has none.
    pub fn add_cross_core_rings(&mut self, rings: Vec<RingLayout>) {
        self.ring_stride = ring_data_stride_bytes(&rings);
        self.rings_padding = rings_padding_bytes(&rings);
        self.total_bytes += rings_reservation_bytes(&rings);
        self.rings = rings;
    }
}

/// Image-wide max of `capacity * slot_size` (decision 875). `0` when empty.
pub fn ring_data_stride_bytes(rings: &[RingLayout]) -> u64 {
    rings
        .iter()
        .map(|r| r.capacity * r.slot_size)
        .max()
        .unwrap_or(0)
}

/// `n * stride - sum(raw data)` — the bytes uniform stride spends.
pub fn rings_padding_bytes(rings: &[RingLayout]) -> u64 {
    if rings.is_empty() {
        return 0;
    }
    let stride = ring_data_stride_bytes(rings);
    let raw: u64 = rings.iter().map(|r| r.capacity * r.slot_size).sum();
    (rings.len() as u64) * stride - raw
}

/// CTL block (`n * 24`) plus padded DATA (`n * stride`).
pub fn rings_reservation_bytes(rings: &[RingLayout]) -> u64 {
    if rings.is_empty() {
        return 0;
    }
    let n = rings.len() as u64;
    n * MAILBOX_BOOKKEEPING_SIZE + n * ring_data_stride_bytes(rings)
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
    /// Logical ring size: `capacity * slot_size` plus the same three-word
    /// head/tail/count bookkeeping a mailbox carries. The **placed**
    /// reservation after M12 item C is larger when strides differ — see
    /// `rings_reservation_bytes` / `rings_padding_bytes`. Report `bytes=`
    /// still spells this logical size (VMM forge check).
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

// M10 F dissolved `BRK_XREPLY_UNKNOWN_CORE` (0xACD8): specialized
// `emit_rt_select_and_run` dense-matches `xreply_remotes` (decision 557
// typed core tag + decision 630). F2's `build_rt_xreply` still owns the
// ring-full trap above.

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
    /// Kept for unit tests / future inject-time validation; specialized
    /// emit uses Reloc variants (decision 683) instead.
    #[allow(dead_code)]
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
    // plans/M12.md item F: image `GROUP_MAX_CHILDREN` fact (floor 2).
    group_max_children: usize,
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
    // plans/M7.md item G / M12 item D: a `@task` marks `has_wake`; the
    // sticky wake-pending bit lives in contiguous `WAKE.wake_pending`
    // (decisions 880–882), not a trailing word of driver state.
    // plans/M8.md item D: a `mailbox=` on the declaration makes the driver
    // messageable, and its mailbox is sized by the *identical* arithmetic
    // an actor's is, from the identical `merge_actor_pub_methods` shapes —
    // one mailbox story, not a driver-shaped copy of it.
    let mut decl_items: Option<Vec<crate::sema::types::DeclItem>> = None;
    let mut drivers = Vec::with_capacity(graph.drivers.len());
    for decl in &graph.drivers {
        let name = crate::sema::types::render_type(&decl.actor_type);
        let state_size = mwir::size_of(&decl.actor_type, layout_ctx)
            .map_err(|e| format!("driver `{name}`'s own state: {e}"))?
            as u64;
        let has_wake = driver_declares_task(modules, &name);
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
            has_wake,
            wake_drain_index: None,
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
    let group_max_children = group_max_children.max(crate::codegen::GROUP_MAX_CHILDREN_FLOOR);
    let group_slot = crate::codegen::group_slot_size(group_max_children);
    total_bytes += ready_queue_capacity * 8 + RR_CURSOR_SIZE + group_arena_capacity * group_slot;

    Ok(Some(RuntimeTables {
        actors,
        drivers,
        free_turns,
        n_turns,
        turn_stride,
        ready_queue_capacity,
        group_arena_capacity,
        group_max_children,
        // Single-core until placement says otherwise (`stripe_for_cores`),
        // and ringless until `add_cross_core_rings` says otherwise.
        // select/drain/child facts filled later by `fill_rtconfig_facts`.
        rings: Vec::new(),
        cores: 1,
        total_bytes,
        ..Default::default()
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
    // plans/M12.md item G / decisions 890–893: census line after the
    // per-static list. `spans` is live `N_INIT_SLOTS` (not the INIT_SPAN
    // placeholder pool); `count` excludes high-zone INIT_SPAN placeholders
    // so the ratchet `N ≤ FIXED_SET_LEN + spans` stays honest while
    // `runtime.wr` still imports INIT_SPAN0..7. Absent when no placed
    // statics (same no-placeholder rule as the lines above).
    if !layout.placed_statics.is_empty() {
        let spans = layout
            .runtime
            .as_ref()
            .map(|t| {
                crate::rtconfig::live_init_span_count(t)
                    .expect("sealed ImageLayout runtime tables agree with placement")
            })
            .unwrap_or(0);
        let census = crate::placed_static_census::summarize(&layout.placed_statics, spans);
        push_line(out, 1, &census.render_line());
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
        // plans/M12.md item C (decision 875): uniform ring-stride cost,
        // printed so a reviewer can see the padding trade. Absent when
        // this image has no cross-core rings (like the `Ring kind=` lines).
        if !tables.rings.is_empty() {
            push_line(
                out,
                1,
                &format!(
                    "Rings count={} stride={} padding={} bytes={}",
                    tables.rings.len(),
                    tables.ring_stride,
                    tables.rings_padding,
                    rings_reservation_bytes(&tables.rings)
                ),
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

/// plans/M10.md item E2 / decision 669: one group's index into the
/// group arena, encoded the way ambient lineage already encodes it —
/// `arena_index + 1`, with `0` meaning "no ambient group". That zero is
/// the `Option[GroupId]` None niche (decision 567's convention, already
/// the machine's). Same shape as [`TurnId`]: private field, `from_index`
/// only, so `GroupId(0)` is unconstructible in Rust.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct GroupId(u32);

impl GroupId {
    pub fn from_index(index: usize) -> GroupId {
        let biased = u32::try_from(index + 1)
            .expect("a group arena with over 4 billion slots cannot fit the machine's memory");
        GroupId(biased)
    }

    pub fn get(self) -> u32 {
        debug_assert!(self.0 != 0, "GroupId(0) is the None niche, not an id");
        self.0
    }

    pub fn index(self) -> usize {
        debug_assert!(self.0 != 0, "GroupId(0) is the None niche, not an id");
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
    /// index. The `state` word here is unchanged either way.
    ///
    /// M12 item D: sticky wake-pending bits are no longer a trail word of
    /// driver state — they live at `wake_base` (contiguous `WAKE` array).
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
    /// M12 item D (decisions 880–882): base of the contiguous
    /// `WAKE.wake_pending` array, placed after rings. Equal to the rings
    /// end when `wake_pending_addrs` is empty (no reservation).
    pub wake_base: u64,
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

    /// `log2(turn_stride)` — the shift the index→address rule scales an
    /// index by, and the whole reason item 0a made the stride a power of
    /// two. `0` for an image with no turns, which then never indexes (no
    /// `rt_*` routine is emitted at all). (The one live emitter of the
    /// rule, `codegen::push_turn_addr_from_id`, multiplies by a relocated
    /// stride instead — see its doc; the dead harness twin that used this
    /// shift was deleted in M10 item M, sweep find L-11.)
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
    let group_slot = crate::codegen::group_slot_size(
        tables
            .group_max_children
            .max(crate::codegen::GROUP_MAX_CHILDREN_FLOOR),
    );
    cursor += tables.group_arena_capacity * group_slot;
    // plans/M8.md item C2 / M12 item C (decision 875): cross-core rings
    // last — all CTL records contiguously, then uniformly-strided DATA.
    // Every address above this point is unchanged for an image with none.
    let n_rings = tables.rings.len() as u64;
    let stride = if tables.rings.is_empty() {
        0
    } else {
        // Prefer the value `add_cross_core_rings` recorded; recompute if a
        // unit test built tables by hand without that path.
        let s = tables.ring_stride;
        if s == 0 {
            ring_data_stride_bytes(&tables.rings)
        } else {
            s
        }
    };
    let ctl_base = cursor;
    let data_base = ctl_base + n_rings * MAILBOX_BOOKKEEPING_SIZE;
    let mut rings = Vec::with_capacity(tables.rings.len());
    for (i, _r) in tables.rings.iter().enumerate() {
        let i = i as u64;
        let head = ctl_base + i * MAILBOX_BOOKKEEPING_SIZE;
        rings.push(RingAddrs {
            ring: data_base + i * stride,
            head,
            tail: head + 8,
            count: head + 16,
        });
    }
    let rings_end = data_base + n_rings * stride;
    // M12 item D: contiguous WAKE.wake_pending after rings. Length comes
    // from `wake_pending_addrs` (filled to the drain count before place).
    let n_wake = tables.wake_pending_addrs.len() as u64;
    let wake_base = rings_end;
    let _wake_end = wake_base + n_wake * 8;
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
        wake_base,
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

// plans/M10.md item H: `build_boot_init` / `emit_boot_init_arg` deleted —
// specialized `codegen::emit_boot_init` lives in `code` under
// `rt_boot_init 0` (decisions 680–684). `build_boot_init_calls` remains:
// it materializes the call specs inject_boot_init_fn consumes.

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
    /// M11 I: IRQ handler stubs to overwrite (`handler_key`, `driver_state`).
    irq_calls: Vec<(String, u64)>,
    /// M11 I: wake `@task` stubs (`task_key`, `driver_state`).
    wake_calls: Vec<(String, u64)>,
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
        let group_max_children = crate::codegen::group_max_children_of(boot.group_child_index);
        let Some(mut tables) = compute_runtime_tables(
            boot.graph,
            boot.modules,
            boot.layout_ctx,
            boot.async_frames,
            group_max_children,
        )
        .map_err(LayoutError::new)?
        .filter(|t| t.total_bytes > 0) else {
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
        let layouts = closure_layout_types(boot.modules, boot.programs)?;
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
            irq_calls: Vec::new(),
            wake_calls: Vec::new(),
        };
        // plans/M8.md item C2: the ring set, last — it is derived from the
        // finished placement plus the compiled call sites, and it grows
        // `rtdata` by exactly its own reservation.
        let rings = cross_core_rings(program, &wiring)?;
        reject_unlowerable_cross_core_shapes(&rings, &wiring, boot, program)?;
        wiring.tables.add_cross_core_rings(rings);
        // M11 F: stamp select/drain/child facts onto tables so dump and
        // reinject share one `rtconfig::generate` input (decision 790).
        fill_rtconfig_facts(&mut wiring)?;
        // M11 I: IRQ/wake facts for checkpoint body (decision 823).
        fill_checkpoint_irq_facts(&mut wiring, boot)?;
        Ok(Some(wiring))
    }
}

/// Fill `RuntimeTables::{select_by_core,drain_by_core,child_sites,
/// ring_target_handles,enqueue_handles,enqueue_actors}` from the finished
/// wiring (plans/M11.md item F / decision 790; item G / decision 801).
fn fill_rtconfig_facts(wiring: &mut RuntimeWiring) -> Result<(), LayoutError> {
    let roots = mailbox_root_names(&wiring.tables);
    let mut select_by_core: Vec<Vec<String>> = vec![Vec::new(); wiring.tables.cores];
    for (i, name) in roots.iter().enumerate() {
        let core = wiring.actor_cores.get(i).copied().unwrap_or(0);
        if core < select_by_core.len() {
            select_by_core[core].push(name.clone());
        }
    }
    let mut drain_by_core = vec![false; wiring.tables.cores];
    for r in &wiring.tables.rings {
        if r.dst < drain_by_core.len() {
            drain_by_core[r.dst] = true;
        }
    }
    let actor_n = wiring.tables.actors.len();
    let msg_drivers = wiring
        .tables
        .drivers
        .iter()
        .filter(|d| d.mailbox.is_some())
        .count();
    let mut child_sites = Vec::new();
    for (callee_key, &child_index) in &wiring.group_child_index {
        let Some(pos) = wiring
            .tables
            .free_turns
            .iter()
            .position(|(k, _)| k == callee_key)
        else {
            continue;
        };
        child_sites.push((callee_key.clone(), child_index, actor_n + msg_drivers + pos));
    }
    // Handle space: actors then drivers then devices (image_decl_handle_word).
    // Mailbox roots are actors then messageable drivers — handle word for
    // actor i is i; for messageable driver at drivers[j] it is actor_n + j.
    let mut enqueue_handles = Vec::new();
    let mut enqueue_actors = Vec::new();
    for (i, a) in wiring.tables.actors.iter().enumerate() {
        enqueue_handles.push(i as u64);
        enqueue_actors.push(a.name.clone());
    }
    for (j, d) in wiring.tables.drivers.iter().enumerate() {
        if d.mailbox.is_some() {
            enqueue_handles.push((actor_n + j) as u64);
            enqueue_actors.push(d.name.clone());
        }
    }
    let mut ring_target_handles = Vec::with_capacity(wiring.tables.rings.len());
    for r in &wiring.tables.rings {
        match r.kind {
            RingKind::Request => {
                let actor = r.actor.as_deref().unwrap_or("");
                let h = enqueue_actors
                    .iter()
                    .zip(enqueue_handles.iter())
                    .find(|(n, _)| *n == actor)
                    .map(|(_, h)| *h)
                    .unwrap_or(0);
                ring_target_handles.push(h);
            }
            RingKind::Reply => ring_target_handles.push(0),
        }
    }
    wiring.tables.select_by_core = select_by_core;
    wiring.tables.drain_by_core = drain_by_core;
    wiring.tables.child_sites = child_sites;
    wiring.tables.ring_target_handles = ring_target_handles;
    let mut root_methods = Vec::with_capacity(enqueue_actors.len());
    let mut root_cores = Vec::with_capacity(enqueue_actors.len());
    for (i, name) in enqueue_actors.iter().enumerate() {
        let methods = wiring
            .dispatch
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, m)| m.clone())
            .unwrap_or_default();
        root_methods.push(methods);
        // `actor_cores` is parallel to `mailbox_root_names` == enqueue_actors order.
        root_cores.push(wiring.actor_cores.get(i).copied().unwrap_or(0));
    }
    wiring.tables.enqueue_handles = enqueue_handles;
    wiring.tables.enqueue_actors = enqueue_actors;
    wiring.tables.root_methods = root_methods;
    wiring.tables.root_cores = root_cores;
    // M11 H: drivers then actors (same call order as former emit_boot_init).
    let n_boot_calls = wiring
        .driver_init_calls
        .iter()
        .chain(wiring.init_calls.iter())
        .filter(|c| c.is_some())
        .count();
    if n_boot_calls > crate::rtconfig::BOOT_CALL_POOL_COUNT {
        return Err(LayoutError::new(format!(
            "image needs {n_boot_calls} boot init calls; pool is {}",
            crate::rtconfig::BOOT_CALL_POOL_COUNT
        )));
    }
    wiring.tables.n_boot_calls = n_boot_calls;
    Ok(())
}

/// M11 I / decision 823 / M12 item D: stamp IRQ vector bits + contiguous
/// `WAKE.wake_pending` addresses onto `tables`, and handler/task keys onto
/// `wiring` for inject.
fn fill_checkpoint_irq_facts(
    wiring: &mut RuntimeWiring,
    boot: &BootCtx,
) -> Result<(), LayoutError> {
    // Place once for driver_state addresses (wake region still empty).
    let rtdata = place_runtime_tables(wrela_machine::layout::RTDATA_BASE, &wiring.tables);
    let (irq, wake) = checkpoint_irq_shape(Some(boot), Some(&rtdata), Some(&wiring.tables));
    if irq.len() > crate::rtconfig::IRQ_CALL_POOL_COUNT {
        return Err(LayoutError::new(format!(
            "image needs {} IRQ stubs; pool is {}",
            irq.len(),
            crate::rtconfig::IRQ_CALL_POOL_COUNT
        )));
    }
    if wake.len() > crate::rtconfig::WAKE_CALL_POOL_COUNT {
        return Err(LayoutError::new(format!(
            "image needs {} wake stubs; pool is {}",
            wake.len(),
            crate::rtconfig::WAKE_CALL_POOL_COUNT
        )));
    }
    // Reserve the contiguous WAKE array after rings, then re-place so
    // `wake_base` sits past the ring reservation.
    wiring.tables.total_bytes += (wake.len() as u64) * 8;
    wiring.tables.wake_pending_addrs = vec![0; wake.len()];
    let rtdata = place_runtime_tables(wrela_machine::layout::RTDATA_BASE, &wiring.tables);
    wiring.tables.irq_vector_bits = irq.iter().map(|e| e.vector).collect();
    wiring.tables.wake_pending_addrs = (0..wake.len())
        .map(|i| rtdata.wake_base + (i as u64) * 8)
        .collect();
    // First drain index per driver (shared-bit / Reloc::WakePending target).
    for d in &mut wiring.tables.drivers {
        d.wake_drain_index = None;
    }
    for e in &wake {
        // Match by placed driver_state address.
        if let Some(di) = rtdata
            .drivers
            .iter()
            .position(|&addr| addr == e.driver_state)
        {
            let d = &mut wiring.tables.drivers[di];
            if d.wake_drain_index.is_none() {
                d.wake_drain_index = Some(e.wake_drain_index);
            }
        }
    }
    wiring.irq_calls = irq
        .into_iter()
        .map(|e| (e.handler_key, e.driver_state))
        .collect();
    wiring.wake_calls = wake
        .into_iter()
        .map(|e| (e.task_key, e.driver_state))
        .collect();
    Ok(())
}

pub struct BootCtx<'a> {
    pub graph: &'a ImageGraph,
    pub modules: &'a BTreeMap<String, Module>,
    /// Typed programs for the same closure — needed so
    /// `closure_layout_types` can run `complete_layouts` (plans/M10.md
    /// item E1 / A2b carry): a `@layout(runtime)` array length that is a
    /// `const` name has no size until after const evaluation, and that
    /// evaluation's results live here.
    pub programs: &'a BTreeMap<String, TypedProgram>,
    pub layout_ctx: &'a LayoutCtx,
    /// `codegen::async_frame_sizes`' result for this same build — every
    /// async fn's own persistent frame bytes, the park-and-resume
    /// redesign's sizing input (`compute_runtime_tables`'s own doc).
    pub async_frames: &'a BTreeMap<String, u64>,
    /// `codegen::compute_group_child_indices`' result for this same build
    /// (plans/M6.md item F / M10 E4): every `g.start`-able callee's own
    /// fixed child-slot ordinal — consumed by `__wrela_child_poll` /
    /// rtconfig child ladders (M11 F). Empty for a build with no
    /// `with group(...)` sites at all.
    pub group_child_index: &'a BTreeMap<String, usize>,
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

    /// plans/M10.md item E1: `closure_layout_types` must run
    /// `complete_layouts`, not only `check_layouts`. Without that pass a
    /// `@layout(runtime)` whose array length is a `const` name stays
    /// deferred (`size: None`) and `require_size` rejects — fail-closed,
    /// but wrong once such a layout reaches this path. The case below is
    /// exactly 03 §3.1's shape (`[TurnArea; N_TURNS]`); it fails the
    /// `require_size` assertion if the completion call is removed.
    #[test]
    fn closure_layout_types_completes_runtime_const_lengths() {
        let src = "\
module examples.e1_closure_layout_complete

const N_TURNS: u32 = 4

@layout(runtime, endian=little)
struct TurnArea:
    state: u32
    waiter: u32

@layout(runtime, endian=little)
struct TurnTable:
    rr_cursor: u64
    turns: [TurnArea; N_TURNS]
";
        let tokens = crate::syntax::lexer::lex(src).expect("lex");
        let module = crate::syntax::parser::parse(tokens).expect("parse");
        let key = module.path.join(".");
        let program = crate::sema::check_typed(&module, "<e1>").expect("check");
        let mut modules = BTreeMap::new();
        modules.insert(key.clone(), module);
        let mut programs = BTreeMap::new();
        programs.insert(key, program);

        // Without the fix this call still returns Ok, but TurnTable has
        // `size: None` and the require_size below is the pin that goes red.
        let layouts = closure_layout_types(&modules, &programs)
            .expect("closure_layout_types completes rather than rejecting");
        let table = layouts
            .get("TurnTable")
            .expect("TurnTable is in the closure");
        // 8 (rr_cursor) + 4 * 8 (TurnArea = u32+u32): same bytes
        // `golden/check-layout-runtime-const-len` pins via the dump path.
        assert_eq!(
            table
                .require_size("closure_layout_types after E1")
                .expect("completed"),
            40
        );
        assert_eq!(table.size, Some(40));
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
        let gen_key: Vec<String> = crate::loader::IMAGE_RUNTIME_MODULE_KEY
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let gen_module = crate::rtconfig::parse_generated(&crate::rtconfig::stub_text())
            .expect("stub must parse");
        let mut modules_vec = BTreeMap::new();
        modules_vec.insert(root_key.clone(), module.clone());
        modules_vec.insert(runtime_key.clone(), runtime_loaded.module.clone());
        modules_vec.insert(gen_key.clone(), gen_module);
        let mut paths = BTreeMap::new();
        paths.insert(root_key.clone(), "<test>".to_string());
        paths.insert(
            runtime_key.clone(),
            runtime_loaded.file.display().to_string(),
        );
        paths.insert(gen_key, crate::rtconfig::GENERATED_INPUT_PATH.to_string());
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

    /// plans/M11.md item I: checkpoint section is the floor trampoline;
    /// irq/wake lists no longer change its words (algorithms are wrela).
    #[test]
    fn checkpoint_section_ignores_irq_wake_lists() {
        let linked = build_checkpoint_and_vector_stub(None);
        let empty = build_checkpoint_and_vector_stub_ex(None, &[], &[], true);
        assert_eq!(
            linked.words, empty.words,
            "empty irq/wake must match the linked trampoline"
        );
        assert_eq!(linked.relocs, empty.relocs);
        let multi = build_checkpoint_and_vector_stub_ex(
            None,
            &[IrqVectorEntry {
                vector: 1,
                handler_key: "struct:BlkDriver.on_queue_irq".into(),
                driver_state: 0x4050_0000,
            }],
            &[WakeDrainEntry {
                driver_state: 0x4050_0000,
                wake_drain_index: 0,
                task_key: "struct:BlkDriver.drain".into(),
            }],
            true,
        );
        assert_eq!(
            multi.words, linked.words,
            "irq/wake must not change checkpoint section words after item I"
        );
        assert_eq!(
            linked.words.len(),
            7,
            "floor 5 + mov x0,#0 + BL (M13 item N per-core arg)"
        );
        assert_eq!(linked.relocs.len(), 1, "one BL to __wrela_rt_checkpoint");
        let bare = build_checkpoint_and_vector_stub_ex(None, &[], &[], false);
        assert_eq!(bare.words.len(), 1, "unlinked section is bare ret");
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
        let out = compute_runtime_tables(
            &graph,
            &modules,
            &ctx,
            &BTreeMap::new(),
            crate::codegen::GROUP_MAX_CHILDREN_FLOOR,
        )
        .unwrap();
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
        let tables = compute_runtime_tables(
            &graph,
            &modules,
            &ctx,
            &BTreeMap::new(),
            crate::codegen::GROUP_MAX_CHILDREN_FLOOR,
        )
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

    /// plans/M10.md item G / decision 671: `group_service_ctx` must name
    /// every turn `place_runtime_tables` laid down — actors, then
    /// messageable drivers, then free turns. Pre-G omitted drivers, so a
    /// messageable driver's parked turn was invisible to the deadline
    /// delivery scan. Fails first against that omission.
    #[test]
    fn group_service_ctx_includes_messageable_driver_turns() {
        let tables = RuntimeTables {
            actors: vec![ActorRuntimeLayout {
                name: "A".to_string(),
                state_size: 8,
                mailbox_capacity: 2,
                slot_size: 16,
                frame_size: 64,
            }],
            drivers: vec![
                DriverRuntimeLayout {
                    name: "Msg".to_string(),
                    state_size: 8,
                    has_wake: false,
                    wake_drain_index: None,
                    mailbox: Some(DriverMailbox {
                        capacity: 2,
                        slot_size: 16,
                        frame_size: 64,
                    }),
                },
                DriverRuntimeLayout {
                    name: "Silent".to_string(),
                    state_size: 8,
                    has_wake: false,
                    wake_drain_index: None,
                    mailbox: None,
                },
            ],
            free_turns: vec![("f".to_string(), 64)],
            n_turns: 3, // actor + messageable driver + free; Silent has none
            turn_stride: 64,
            group_arena_capacity: 1,
            ready_queue_capacity: 3,
            ..RuntimeTables::default()
        };
        let base = 0x4000u64;
        let p = place_runtime_tables(base, &tables);
        let ctx = group_service_ctx(&p, &tables).expect("group arena present");
        assert_eq!(
            ctx.turn_areas.len(),
            3,
            "actors + messageable drivers + free turns"
        );
        assert_eq!(ctx.turn_areas[0].0, p.actors[0].turn);
        assert_eq!(ctx.turn_areas[0].1, TurnId::from_index(0));
        let driver = p
            .driver_mailboxes
            .get(&0)
            .expect("messageable driver placed");
        assert_eq!(ctx.turn_areas[1].0, driver.turn);
        assert_eq!(ctx.turn_areas[1].1, TurnId::from_index(1));
        assert_eq!(ctx.turn_areas[2].0, p.free_turns["f"]);
        assert_eq!(ctx.turn_areas[2].1, TurnId::from_index(2));
        let shape = group_service_shape(Some(&tables)).expect("shape");
        assert_eq!(
            shape.turn_areas.len(),
            ctx.turn_areas.len(),
            "shape length must match real ctx (word-count contract)"
        );
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
        let err = compute_runtime_tables(
            &graph,
            &modules,
            &ctx,
            &BTreeMap::new(),
            crate::codegen::GROUP_MAX_CHILDREN_FLOOR,
        )
        .unwrap_err();
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
        let tables = compute_runtime_tables(
            &graph,
            &modules,
            &ctx,
            &BTreeMap::new(),
            crate::codegen::GROUP_MAX_CHILDREN_FLOOR,
        )
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
    @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
    match v:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, \"rejected\"

@test(runtime)
async fn asks_driver(d: Actor[BlkDriver]):
    v = await d.get()
    @discard(reason=\"migrated: deliberate Err discard (M13 item L)\")
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
    fn boot_init_call_stub_emits_init_call_reloc() {
        // M11 H: zero-fill sequencing lives in `__wrela_rt_boot_init`;
        // specialized stubs only emit one init Call (decision 812).
        use crate::codegen::{
            BootInitArgSpec, BootInitCallSpec, BootInitSlotSpec, Reloc, emit_boot_init_call,
        };
        let slot = BootInitSlotSpec {
            name: "A".into(),
            is_driver: false,
            state_size: 8,
            init: Some(BootInitCallSpec {
                key: "A.init".into(),
                args: vec![BootInitArgSpec::Word(7)],
                fallible: false,
                err_msg: None,
            }),
        };
        let f = emit_boot_init_call(&slot);
        assert!(
            f.relocs.iter().any(|r| matches!(
                r,
                Reloc::Call { key, .. } if key == "A.init"
            )),
            "boot init call stub must Call the init key"
        );
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
    fn verify_section_sizes_accepts_steered_rtdata_gap() {
        // plans/M11.md item C: the only legal >=8-byte inter-section gap is
        // the packing window before `rtdata` at `RTDATA_BASE`.
        let sections = vec![
            Section {
                name: "checkpoint",
                base: machine_layout::IMAGE_BASE,
                size: 0x100,
            },
            Section {
                name: "rtdata",
                base: machine_layout::RTDATA_BASE,
                size: 32,
            },
        ];
        let blob_len = machine_layout::RTDATA_BASE + 32 - machine_layout::IMAGE_BASE;
        assert!(verify_section_sizes(&sections, machine_layout::IMAGE_BASE, blob_len).is_ok());
    }

    #[test]
    fn verify_section_sizes_rejects_unsteered_wide_gap() {
        let sections = vec![
            Section {
                name: "a",
                base: 0x1000,
                size: 16,
            },
            Section {
                name: "b",
                base: 0x1100,
                size: 16,
            },
        ];
        assert!(verify_section_sizes(&sections, 0x1000, 0x1100 + 16 - 0x1000).is_err());
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

    /// plans/M10.md item C / decision 600 (note 599): a `wrela build`
    /// image's AbortFixed/AbortVal landings are halt-only — they never
    /// call the console append/commit path. Console overflow there is
    /// unreachable; extending `check_transcript_bound` to `layout_program`
    /// would check a transcript shape build images do not produce.
    #[test]
    fn layout_program_abort_landings_are_halt_only() {
        let mut expected_fixed = Vec::new();
        push_halt(&mut expected_fixed, EXIT_CODE_ABORT_FIXED);
        let mut expected_val = Vec::new();
        push_halt(&mut expected_val, EXIT_CODE_ABORT_VAL);

        let mut fns = BTreeMap::new();
        fns.insert(
            "f".to_string(),
            crate::codegen::CodegenFn {
                frame_size: 0,
                code: vec![(encode::enc_ret(30), "ret".to_string())],
                relocs: Vec::new(),
            },
        );
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
        };
        let layout = layout_program(&program, None).unwrap();
        let abort = layout
            .sections
            .iter()
            .find(|s| s.name == "abort")
            .expect("abort section");
        let abort_bytes = (expected_fixed.len() + expected_val.len()) * 4;
        assert_eq!(abort.size, abort_bytes as u64);

        let off = (abort.base - machine_layout::IMAGE_BASE) as usize;
        let mut expected = Vec::new();
        for w in expected_fixed.iter().chain(expected_val.iter()) {
            expected.extend_from_slice(&w.to_le_bytes());
        }
        assert_eq!(
            &layout.blob[off..off + expected.len()],
            expected.as_slice(),
            "build-image abort landings must be push_halt, not the print path"
        );
    }
}
