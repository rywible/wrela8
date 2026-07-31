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
//!   stack pointer (`sp = core_stack_base_n(0, N) + CORE_STACK_SIZE` —
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
use crate::eval::image::ImageGraph;
use crate::flowwir::{AwaitKind, FlowInst, FlowWirProgram, Transition};
use crate::mwir::{self, LayoutCtx};
use crate::sema::SemaError;
use crate::sema::typed::TypedProgram;
use crate::syntax::ast::Module;
// `console` is used only by this file's own `#[cfg(test)]` module.
#[cfg(test)]
use wrela_machine::console;
use wrela_machine::layout as machine_layout;

/// Runtime emission + boot harness (plans/M10.md item K): floor stubs,
/// specialized inject helpers, JIT materializers, and `layout_test_image`.
/// Pure section packing / placement / reports stay in this file.
mod harness;
mod place;
mod report_lines;

mod boot_init;
mod rtdata;

pub(crate) use boot_init::device_index_of;
pub use rtdata::{
    ActorAddrs, ActorRuntimeLayout, DriverMailbox, DriverRuntimeLayout, GroupId, RingAddrs,
    RingKind, RingLayout, RuntimePlacement, RuntimeTables, TurnId, actor_method_index_tables,
    compute_runtime_tables, count_with_group_sites, mailbox_root_names, resolve_runtime_test_args,
    ring_data_stride_bytes, rings_padding_bytes, rings_reservation_bytes,
};
pub(crate) use rtdata::{
    MAILBOX_BOOKKEEPING_SIZE, REPLY_SLOT_SIZE, RR_CURSOR_SIZE, declared_mailbox_capacity,
    merge_actor_pub_methods, turn_owner,
};

pub use boot_init::BootCtx;
pub(crate) use boot_init::{
    BootInitArg, BootInitCall, RuntimeWiring, actor_inits, build_boot_init_calls,
    intern_fallible_init_abort_messages,
};

pub use place::place_runtime_tables;
pub use report_lines::{
    append_blk_vmm_lines, append_ring_vmm_lines, append_vmm_runtime_lines, attach_blk_report,
    parsed_runtime_tail, render_layout_section,
};

// Runtime / harness emission (plans/M10.md item K) — re-export so
// `layout::build_*` / `layout::layout_test_image` / census paths stay stable.
pub use harness::{
    DEADLOCK_MSG, EXIT_CODE_ABORT_FIXED, EXIT_CODE_ABORT_VAL, EXIT_CODE_NO_RUNTIME,
    TranscriptBound, build_checkpoint_and_vector_stub, build_checkpoint_and_vector_stub_ex,
    check_transcript_bound, compute_transcript_bound, lane1_pair_bytes, lane2_marker_bytes,
    lane2_pair_bytes, layout_test_image,
};

#[cfg(test)]
pub(crate) use harness::emitted_a64_census_live_counts;

use harness::{
    append_rodata, build_entry_stub, inject_boot_init_fn, inject_checkpoint_irq_fns,
    inject_rt_cross_core_fns, inject_rt_enqueue_and_dispatch_fns, push_halt, test_runner_facts,
};

#[cfg(test)]
use harness::Asm;

// --- errors ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutError {
    pub message: String,
}

impl LayoutError {
    pub(crate) fn new(message: impl Into<String>) -> LayoutError {
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
    /// Sealed bring-up count (`Image(..., cores=N)` / `PlacementTable.cores`),
    /// plans/M15.md item D. Drives report `Cores`/`CoreStack` and high-DRAM
    /// SP install. Always ≥ 1.
    pub cores: usize,
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
    blob.resize(want, 0);
}

// --- reloc resolution ------------------------------------------------------

/// `imm26`'s own signed byte range (`encode::word_offset`'s `bits=26`):
/// `half_range = 1 << (bits+1)` bytes either side of zero.
const BL_HALF_RANGE_BYTES: i64 = 1i64 << 27;

/// `imm21`'s own signed *page* range (`ADRP`'s 21-bit signed page count).
const ADRP_HALF_RANGE_PAGES: i64 = 1i64 << 20;

/// `imm21`'s own signed *byte* range (`ADR`'s 21-bit signed byte offset):
/// ±1 MiB either side of the instruction's own address.
///
/// plans/codegen-pareto.md decision 1703 / freeze 1713. This is the whole
/// content of "the range proof": every `Reloc::RodataAdr` site's
/// `target − this` distance is measured against it at layout time, once the
/// addresses are real, and a site outside it **fails the build**
/// ([`adr_out_of_range`]). It is never widened, never rounded, and there is
/// no path that emits an `ADR` without passing through here.
const ADR_HALF_RANGE_BYTES: i64 = 1i64 << 20;

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
    // same shape — not space-bearing, but must not redirect either. Both
    // families are what `is_compiler_glue_symbol` means.
    if crate::codegen::is_compiler_glue_symbol(caller_key) {
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
/// FlowWir actually contains, as `(sending core, target mailbox root)`
/// pairs. Derived from placement crossed with `Send` / `Await{ActorCall}`
/// sites — never from a compiled `CodegenProgram` (Wave 1: rings before
/// runtime codegen).
fn cross_core_edges(
    flow: &FlowWirProgram,
    w: &RuntimeWiring,
) -> Result<BTreeSet<(usize, String)>, LayoutError> {
    let mut out = BTreeSet::new();
    if w.placement.cores <= 1 {
        return Ok(out);
    }
    for (key, f) in &flow.fns {
        let mut method_keys: Vec<String> = Vec::new();
        for state in &f.states {
            for op in &state.ops {
                if let FlowInst::Send { method_key, .. } = op {
                    method_keys.push(method_key.clone());
                }
            }
            if let Transition::Await {
                what: AwaitKind::ActorCall { method_key, .. },
                ..
            } = &state.transition
            {
                method_keys.push(method_key.clone());
            }
        }
        for mk in method_keys {
            let actor = mk.split('.').next().unwrap_or(mk.as_str()).to_string();
            let target = crate::codegen::rt_enqueue_symbol(&actor);
            if let Some(sym) = resolve_cross_core_edge(key, &target, Some(w))? {
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
    flow: &FlowWirProgram,
    w: &RuntimeWiring,
) -> Result<Vec<RingLayout>, LayoutError> {
    let edges = cross_core_edges(flow, w)?;
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
    flow: &FlowWirProgram,
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
    // Wave 1: checkpoint = FlowWir back-edge (Jump/Branch to a prior state),
    // not `Reloc::CheckpointService` on a compiled program.
    for (key, f) in &flow.fns {
        let has_checkpoint = f
            .states
            .iter()
            .enumerate()
            .any(|(i, s)| match &s.transition {
                Transition::Jump(t) if *t <= i => true,
                Transition::Branch {
                    then_state,
                    else_state,
                    ..
                } if *then_state <= i || *else_state <= i => true,
                _ => false,
            });
        if !has_checkpoint {
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
    // plans/codegen-pareto.md item F5: **preserve the form the emitter
    // encoded**. A tail call is an ordinary call edge — same
    // `Reloc::Call`, same reachability, same cross-core resolution — but
    // it is a `B`, not a `BL`, and rewriting it as a `BL` here would
    // silently reinstate a return through a frame that no longer exists.
    // Bit 31 is the `op` field that distinguishes the two
    // (`encode::b_bl`), and it is the only difference between them.
    // Fail closed on anything that is not already a `B`/`BL`: the class
    // field is bits [30:26] = 0b00101 for exactly those two, and a
    // placeholder that is neither means the reloc names a word the
    // emitter did not put a branch at.
    if (words[idx] >> 26) & 0b1_1111 != 0b00101 {
        return Err(LayoutError::new(format!(
            "internal error: a call relocation names word {idx} at {this_addr:#x}, which holds              {:#010x} — not a `B`/`BL`",
            words[idx]
        )));
    }
    let links = words[idx] & 0x8000_0000 != 0;
    words[idx] = if links {
        encode::enc_bl(delta as i32)
    } else {
        encode::enc_b(delta as i32)
    };
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
            Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Return(_, None)
            | Stmt::Pass(_)
            | Stmt::Dmb(_) => {}
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

/// **The `ADR` range proof** (plans/codegen-pareto.md decision 1731, freeze
/// 1713). Patch the single-word `ADR` at `word_adr` to reach `target_addr`,
/// or refuse.
///
/// The refusal is a **hard build error**, not a fallback (decision 1732).
/// A fallback is not available at this point even in principle: the
/// `ADRP`+`ADD` pair is two words where codegen already committed one, so
/// widening here would move every address after this site — including the
/// addresses this pass has already patched and the section sizes
/// `verify_section_sizes` is about to check. The honest choices were "prove
/// it at layout and error" or "iterate layout to a fixpoint"; the second is
/// a whole new pass shape for a condition that is 4–10× away from firing
/// (decision 1703's measured headroom), so this errors, loudly, naming the
/// site and the distance, and telling the reader which knob turns the
/// substitution off.
fn patch_adr(
    words: &mut [u32],
    word_adr: usize,
    this_addr: u64,
    target_addr: u64,
) -> Result<(), LayoutError> {
    let reg = (words[word_adr] & 0x1F) as u8;
    let delta = target_addr as i64 - this_addr as i64;
    if !(-ADR_HALF_RANGE_BYTES..ADR_HALF_RANGE_BYTES).contains(&delta) {
        return Err(adr_out_of_range(this_addr, target_addr, delta));
    }
    words[word_adr] = encode::enc_adr(reg, delta as i32);
    Ok(())
}

/// The one diagnostic freeze 1713 demands: an out-of-range `ADR` site names
/// itself, its target, its distance and the ±1 MiB bound, and says what to
/// do. Split out of [`patch_adr`] so the unit that proves the refusal fires
/// asserts on the *same* text a build would print.
fn adr_out_of_range(this_addr: u64, target_addr: u64, delta: i64) -> LayoutError {
    LayoutError::new(format!(
        "relocation out of range: an `ADR` at {this_addr:#x} targets {target_addr:#x}, \
         {delta} bytes away — outside `ADR`'s own ±1 MiB (±{ADR_HALF_RANGE_BYTES} byte) \
         reach. `OptId::AdrAddressing` (plans/codegen-pareto.md item B) substitutes one \
         `ADR` for an `ADRP`+`ADD` pair only where the whole image proves every site is \
         in reach; this image is too large between its code and its rodata for that. \
         Build in `dev`, or drop `OptId::AdrAddressing` from `RELEASE_OPTS`, to get the \
         two-word page-relative form back."
    ))
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
    verify_branch_region(sections)?;
    Ok(())
}

/// plans/codegen-pareto.md decision 1705 / 1754 — SOG §4.8's **same-region
/// property**: every branch and its target must sit inside one aligned
/// 2 MiB region.
///
/// Every branch this backend emits is `PC`-relative within the text, and
/// every branchable target — fn entries, block leaders, the abort tail, the
/// checkpoint service — lives between `entry` and `checkpoint`. So the
/// property is exactly "that span does not straddle a 2 MiB boundary", and
/// it is checked here rather than bought by moving the text base: the base
/// `IMAGE_BASE + 0x50` is *not* 2 MiB-aligned, but nothing branches across
/// a boundary because the whole text is two orders of magnitude smaller
/// than a region. Aligning the base instead would cost ~1 MiB of image
/// padding (`IMAGE_BASE` is `0x4050_0000`, the next 2 MiB boundary is
/// `0x4060_0000`) or a machine-contract move of `IMAGE_BASE` itself —
/// see `plans/codegen-pareto-D.md`.
///
/// **Fail closed, not assumed.** The property holds today by a factor of
/// ~24, but unlike its neighbours in [`verify_section_sizes`] this one is
/// *reachable from a source program*: an image whose text outgrows 2 MiB
/// breaks it without any editing mistake. So it is a build error, not an
/// internal error, and it says what to do about it.
fn verify_branch_region(sections: &[Section]) -> Result<(), LayoutError> {
    let branchable: Vec<&Section> = sections
        .iter()
        .filter(|s| matches!(s.name, "entry" | "code" | "abort" | "checkpoint"))
        .collect();
    let (Some(first), Some(last)) = (branchable.first(), branchable.last()) else {
        return Ok(());
    };
    let lo = first.base;
    let hi = last.base + last.size;
    if !crate::blocklayout::same_region_holds(lo, hi) {
        return Err(LayoutError::new(format!(
            "branchable text spans {lo:#x}..{hi:#x} ({} bytes), which straddles a \
             {region}-byte region boundary — SOG §4.8 requires every branch and its target to \
             share one 2 MiB region, so the text base must move to a region boundary \
             (plans/codegen-pareto.md decision 1754)",
            hi - lo,
            region = crate::blocklayout::REGION_BYTES
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
pub(crate) fn verify_ring_windows(
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
        Some(b) => RuntimeWiring::derive(b)?,
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

    // Single CodegenProgram already includes live runtime.wr (lowered
    // against swapped rtconfig upstream). Remaining inject_* are named
    // floor specialization (ImageStatic stubs + aliases), not a second
    // codegen path.
    let mut program_owned;
    let program = if let Some(w) = wiring.as_ref() {
        program_owned = program.clone();
        apply_resume_remaps(&mut program_owned, w);
        inject_rt_enqueue_and_dispatch_fns(&mut program_owned, w)?;
        inject_rt_cross_core_fns(&mut program_owned, w)?;
        inject_boot_init_fn(&mut program_owned, w);
        inject_checkpoint_irq_fns(&mut program_owned, w);
        &program_owned
    } else {
        program
    };

    let entry_words = build_entry_stub();

    let mut code_words: Vec<u32> =
        Vec::with_capacity(program.fns.values().map(|f| f.code.len()).sum());
    let mut fn_word_base: BTreeMap<String, usize> = BTreeMap::new();
    for (key, f) in &program.fns {
        fn_word_base.insert(key.clone(), code_words.len());
        for ew in &f.code {
            code_words.push(ew.word);
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
                Reloc::RodataAdr { word, byte_offset } => {
                    let rb = rodata_base.ok_or_else(|| {
                        LayoutError::new(
                            "internal error: a Reloc::RodataAdr exists but the rodata section is \
                             empty",
                        )
                    })?;
                    let this_addr = code_base + ((base + word) * 4) as u64;
                    let target_addr = rb + *byte_offset as u64;
                    patch_adr(&mut all_code_words, base + word, this_addr, target_addr)?;
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
    // The section table already knows how far the blob reaches; reserve it
    // once rather than growing a ~300 KB buffer by repeated doubling.
    // `verify_section_sizes` below is what actually holds the two in sync.
    let mut blob = Vec::with_capacity(
        sections
            .iter()
            .map(|s| (s.base + s.size).saturating_sub(image_base) as usize)
            .max()
            .unwrap_or(0),
    );
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
    let cores = wiring.as_ref().map(|w| w.tables.cores).unwrap_or(1).max(1);
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
        cores,
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
/// Result of the one-check → one-lower image compile (live rtconfig
/// swapped before the single `CodegenProgram`).
pub struct ImageCodegen {
    pub program: CodegenProgram,
    pub modules: BTreeMap<String, Module>,
    pub programs: BTreeMap<String, TypedProgram>,
    pub flow: FlowWirProgram,
    pub async_frames: BTreeMap<String, u64>,
    pub group_child_index: BTreeMap<String, usize>,
    pub layout_ctx: LayoutCtx,
    /// Generated `core.__image_runtime` source (digest / report).
    pub rtconfig_text: String,
}

/// Eval wiring facts → live `rtconfig::generate_with` → swap stub
/// `__image_runtime` → re-check → single lower/codegen with
/// wiring-conditional force-root seeds. No second `CodegenProgram`.
///
/// `emit_comptime_tests` is `LowerOpts`' own flag, threaded through
/// because this fn owns every `LowerOpts` the image path builds: 02 §12.2
/// keeps a comptime-legal bare `@test` out of production images, and
/// `diff-eval` — the one caller that boots those same bodies as guest
/// code to compare tiers — opts back in. Without it the oracle's own
/// `runtime_tests` are marked host-only, never lower, and every case
/// fails closed at `layout_test_image`'s "was never codegen'd" guard.
/// Every production caller (`wrela build`/`test`, the VMM's conformance
/// images, `fuzz async`) passes `false`.
pub fn lower_and_codegen_image(
    modules: &BTreeMap<String, Module>,
    programs: &BTreeMap<String, TypedProgram>,
    layout_ctx: &LayoutCtx,
    graph: &ImageGraph,
    runtime_tests: &[String],
    async_tests: &BTreeSet<String>,
    emit_comptime_tests: bool,
) -> Result<ImageCodegen, String> {
    let capacity = crate::eval::image_checks::blk_capacity_sectors(graph);
    let mut layout_ctx = layout_ctx.clone();
    enrich_layout_ctx_with_instantiations(&mut layout_ctx, programs);

    // Pass 1: FlowWir against stub-checked programs — enough for
    // `RuntimeWiring::derive` (rings / tables; no CodegenProgram).
    let reach_opts = crate::lower::LowerOpts {
        emit_comptime_tests,
        only: None,
    };
    let reachable = crate::lower::guest_reachable_keys_closure(programs, &reach_opts);
    let derive_opts = crate::lower::LowerOpts {
        emit_comptime_tests,
        only: Some(reachable),
    };
    let flow_derive = lower_flow_closure(programs, capacity, &derive_opts)?;
    let async_frames_derive =
        crate::codegen::async_frame_sizes(&flow_derive, &layout_ctx).map_err(|e| e.message)?;
    let (group_child_index_derive, _) =
        crate::codegen::compute_group_child_indices(&flow_derive).map_err(|e| e.message)?;
    let boot = BootCtx {
        graph,
        modules,
        programs,
        layout_ctx: &layout_ctx,
        async_frames: &async_frames_derive,
        group_child_index: &group_child_index_derive,
        flow: &flow_derive,
    };
    let wiring = RuntimeWiring::derive(&boot).map_err(|e| e.message)?;
    let tests = test_runner_facts(
        runtime_tests,
        async_tests,
        wiring.as_ref().map(|w| &w.tables),
    );
    let need_live = wiring.is_some() || !tests.is_empty();

    let (live_modules, live_programs, rtconfig_text, force_opts) = if need_live {
        let (text, force_opts) = generate_live_rtconfig(wiring.as_ref(), &tests)?;
        let (mods, progs) = recheck_with_live_rtconfig(modules, &text)?;
        (mods, progs, text, force_opts)
    } else {
        (
            modules.clone(),
            programs.clone(),
            String::new(),
            crate::lower::ImageForceRootOpts::default(),
        )
    };

    let mut only = crate::lower::guest_reachable_keys_closure(&live_programs, &reach_opts);
    crate::lower::seed_image_force_roots(&mut only, &live_programs, force_opts);
    let lower_opts = crate::lower::LowerOpts {
        emit_comptime_tests,
        only: Some(only),
    };
    let mwir = lower_mwir_closure(&live_programs, capacity, &lower_opts)?;
    let flow = lower_flow_closure(&live_programs, capacity, &lower_opts)?;
    let method_index =
        actor_method_index_tables(&live_modules, &layout_ctx).map_err(|e| e.message)?;
    let group_arena_capacity = count_with_group_sites(&live_modules);
    let enqueue_specs = mailbox_enqueue_specs(graph, &live_modules, &layout_ctx)?;
    let program = crate::codegen::codegen_program_with_async(
        &mwir,
        &flow,
        &layout_ctx,
        &method_index,
        group_arena_capacity,
        &enqueue_specs,
    )
    .map_err(|e| e.message)?;
    let async_frames =
        crate::codegen::async_frame_sizes(&flow, &layout_ctx).map_err(|e| e.message)?;
    let (group_child_index, _) =
        crate::codegen::compute_group_child_indices(&flow).map_err(|e| e.message)?;
    Ok(ImageCodegen {
        program,
        modules: live_modules,
        programs: live_programs,
        flow,
        async_frames,
        group_child_index,
        layout_ctx,
        rtconfig_text,
    })
}

fn generate_live_rtconfig(
    wiring: Option<&RuntimeWiring>,
    tests: &[crate::rtconfig::TestRunnerFact],
) -> Result<(String, crate::lower::ImageForceRootOpts), String> {
    match wiring {
        Some(w) => {
            let mut extras = crate::rtconfig::extras_from_tables(&w.tables)?;
            extras.tests = tests.to_vec();
            extras.has_boot_init = true;
            let text = crate::rtconfig::generate_with(&w.tables, &extras)?;
            let opts = crate::lower::ImageForceRootOpts {
                with_wiring: true,
                with_test_runner: !tests.is_empty(),
                n_tests: tests.len(),
                n_boot_calls: w.tables.n_boot_calls,
                n_irq_calls: w.tables.irq_vector_bits.len(),
                n_wake_calls: w.tables.wake_pending_addrs.len(),
                n_cores: w.tables.cores,
            };
            Ok((text, opts))
        }
        None => {
            let mut tables = RuntimeTables {
                n_turns: 0,
                turn_stride: 0,
                ready_queue_capacity: 1,
                group_arena_capacity: 0,
                total_bytes: 128,
                cores: 1,
                ..RuntimeTables::default()
            };
            tables.stripe_for_cores(1);
            let mut extras = crate::rtconfig::RtconfigExtras::default();
            extras.tests = tests.to_vec();
            extras.has_boot_init = false;
            let text = crate::rtconfig::generate_with(&tables, &extras)?;
            let opts = crate::lower::ImageForceRootOpts {
                with_wiring: false,
                with_test_runner: !tests.is_empty(),
                n_tests: tests.len(),
                n_boot_calls: 0,
                n_irq_calls: 0,
                n_wake_calls: 0,
                n_cores: 1,
            };
            Ok((text, opts))
        }
    }
}

fn recheck_with_live_rtconfig(
    modules: &BTreeMap<String, Module>,
    rtconfig_text: &str,
) -> Result<(BTreeMap<String, Module>, BTreeMap<String, TypedProgram>), String> {
    let gen_module = crate::rtconfig::parse_generated(rtconfig_text)?;
    let gen_key: Vec<String> = crate::loader::IMAGE_RUNTIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let mut modules_vec: BTreeMap<Vec<String>, Module> = BTreeMap::new();
    let mut paths: BTreeMap<Vec<String>, String> = BTreeMap::new();
    for (dot, m) in modules {
        let key: Vec<String> = dot.split('.').map(|s| s.to_string()).collect();
        if key.as_slice() == crate::loader::IMAGE_RUNTIME_MODULE_KEY {
            continue;
        }
        paths.insert(key.clone(), format!("<{dot}>"));
        modules_vec.insert(key, m.clone());
    }
    // Live image always needs `core.runtime` + generated config together.
    let (runtime_key, runtime_loaded) = crate::loader::load_runtime_module()
        .map_err(|_| "stdlib/core/runtime.wr missing".to_string())?;
    modules_vec.insert(runtime_key.clone(), runtime_loaded.module);
    paths.insert(runtime_key, runtime_loaded.file.display().to_string());
    modules_vec.insert(gen_key.clone(), gen_module);
    paths.insert(gen_key, crate::rtconfig::GENERATED_INPUT_PATH.to_string());
    // The first check may have discarded `core.time` (dump/report
    // discipline). Live re-check still needs it whenever any module
    // mentions a time-prelude name — otherwise `ms`/`Instant.less_than`
    // resolve as non-callable stubs.
    let time_key: Vec<String> = crate::loader::TIME_MODULE_KEY
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let needs_time = modules_vec
        .values()
        .any(crate::loader::module_mentions_time);
    if needs_time && !modules_vec.contains_key(&time_key) {
        let (loaded_key, time_loaded) = crate::loader::load_time_module()
            .map_err(|_| "stdlib/core/time.wr missing".to_string())?;
        debug_assert_eq!(loaded_key, time_key);
        paths.insert(time_key.clone(), time_loaded.file.display().to_string());
        modules_vec.insert(time_key, time_loaded.module);
    }
    let programs_vec = crate::sema::check_program_typed(&modules_vec, &paths).map_err(|e| {
        format!(
            "error[{}]: live rtconfig re-check: {}",
            e.category, e.message
        )
    })?;
    let programs: BTreeMap<String, TypedProgram> = programs_vec
        .into_iter()
        .map(|(k, p)| (k.join("."), p))
        .collect();
    let modules: BTreeMap<String, Module> = modules_vec
        .into_iter()
        .map(|(k, m)| (k.join("."), m))
        .collect();
    Ok((modules, programs))
}

fn lower_mwir_closure(
    programs: &BTreeMap<String, TypedProgram>,
    capacity: Option<u64>,
    opts: &crate::lower::LowerOpts,
) -> Result<mwir::MwirProgram, String> {
    let mut mwir_programs = Vec::with_capacity(programs.len());
    for typed in programs.values() {
        let mut stamped = typed.clone();
        stamped.blk_capacity_sectors = capacity;
        mwir_programs
            .push(crate::lower::lower_program_with(&stamped, opts).map_err(|e| e.message)?);
    }
    Ok(merge_mwir_programs(mwir_programs))
}

fn lower_flow_closure(
    programs: &BTreeMap<String, TypedProgram>,
    capacity: Option<u64>,
    opts: &crate::lower::LowerOpts,
) -> Result<FlowWirProgram, String> {
    let mut flow_fns = BTreeMap::new();
    for typed in programs.values() {
        let mut stamped = typed.clone();
        stamped.blk_capacity_sectors = capacity;
        flow_fns.extend(
            crate::flowwir_lower::lower_program_with(&stamped, opts)
                .map_err(|e| e.message)?
                .fns,
        );
    }
    Ok(FlowWirProgram { fns: flow_fns })
}

pub(super) fn apply_resume_remaps(program: &mut CodegenProgram, wiring: &RuntimeWiring) {
    let Ok(extras) = crate::rtconfig::extras_from_tables(&wiring.tables) else {
        return;
    };
    let remaps = crate::rtconfig::stub_call_remaps(&extras, wiring.tables.cores);
    if remaps.is_empty() {
        return;
    }
    for f in program.fns.values_mut() {
        crate::rtconfig::remap_call_keys(f, &remaps);
    }
}

/// Same as [`try_layout_program`], but also returns the `CodegenProgram`
/// layout already built — so the image report can score proxy-cycles
/// without a second lower (plans/M18.md item R).
pub fn try_layout_with_codegen(
    programs: &BTreeMap<String, TypedProgram>,
    layout_ctx: &LayoutCtx,
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
) -> Result<Option<(ImageLayout, CodegenProgram)>, String> {
    let empty_tests: &[String] = &[];
    let empty_async = BTreeSet::new();
    // Item W / fallible-`init` refusals must fail `wrela build` even when
    // the rest of image layout is optional (`Ok(None)` → report without
    // an `.img`). `RuntimeWiring::derive` also runs these, but only after
    // FlowWir ring discovery — and cross-core unlowerable shapes there
    // are deliberately soft for `--stage=report` (pinned by
    // `err-cross-core-*` report goldens). So the boot-init law is checked
    // here first, before the soft lower/codegen gate.
    {
        let inits = actor_inits(modules).map_err(|e| e.message)?;
        let layouts = closure_layout_types(modules, programs).map_err(|e| e.message)?;
        let backings =
            crate::eval::image_checks::pool_backings(graph, &layouts).map_err(|e| e.message)?;
        build_boot_init_calls(graph, &inits, &backings).map_err(|e| e.message)?;
    }
    let compiled = match lower_and_codegen_image(
        modules,
        programs,
        layout_ctx,
        graph,
        empty_tests,
        &empty_async,
        false,
    ) {
        Ok(c) => c,
        // A fail-closed resource violation is **hard** — a blown pool bound
        // is not "this shape did not lower", and absorbing it hands the
        // caller a silent image-less report and exit code 0 (the fail-open
        // plans/M20.md item B measured once `BLOCK_POOL_COUNT` was
        // exhausted under decision 1607's wider owner set).
        Err(e) if e.starts_with(crate::codegen::FAIL_CLOSED_PREFIX) => return Err(e),
        // Soft: reachable surface / cross-core shape did not fully lower.
        // Report stages keep an ImageReport without an `.img` — the
        // `err-cross-core-*` report goldens pin exactly that.
        Err(_) => return Ok(None),
    };
    layout_program(
        &compiled.program,
        Some(BootCtx {
            graph,
            modules: &compiled.modules,
            programs: &compiled.programs,
            layout_ctx: &compiled.layout_ctx,
            async_frames: &compiled.async_frames,
            group_child_index: &compiled.group_child_index,
            flow: &compiled.flow,
        }),
    )
    .and_then(|mut layout| {
        attach_blk_report(&mut layout, graph, &compiled.programs)?;
        layout.placed_statics = collect_placed_statics(&compiled.programs)?;
        Ok(Some((layout, compiled.program)))
    })
    .or_else(|e| Err(e.message))
}

pub fn try_layout_program(
    programs: &BTreeMap<String, TypedProgram>,
    layout_ctx: &LayoutCtx,
    graph: &ImageGraph,
    modules: &BTreeMap<String, Module>,
) -> Result<Option<ImageLayout>, String> {
    Ok(try_layout_with_codegen(programs, layout_ctx, graph, modules)?.map(|(layout, _)| layout))
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
    verify_device_window_statics(&out)?;
    Ok(out)
}

/// The reserved device-page growth window (`wrela-machine`'s own map:
/// `0x4000_8000 .. 0x4001_0000`), where `runtime.wr` parks the counter pages.
const DEVICE_WINDOW_LO: u64 = 0x4000_8000;
const DEVICE_WINDOW_HI: u64 = 0x4001_0000;

/// Every `@placed` static inside the device-page growth window must fit inside
/// it and touch no other static there.
///
/// plans/lane1-per-core.md item A made `LANE1` `N_CORES` rows wide, so for the
/// first time a static in this window has an image-dependent extent: at
/// `METHOD_CALL_POOL_COUNT = 128` a row is 1048 bytes, and enough cores walk
/// the stripe off the end of the window. Nothing else would notice — a placed
/// static is just an address plus a layout type, and the guest would quietly
/// increment a counter on top of whatever came next. So this refuses, naming
/// the two statics and the window, instead of approximating (CLAUDE.md: fail
/// closed).
///
/// Scoped to this window on purpose, **not** generalized to all placed
/// statics: `INIT_SPAN{k}` overlays deliberately alias the rtdata state they
/// zero (`RT` / actor state — `boot_init`'s coalesced spans), so a global
/// non-overlap rule would be false. Inside the growth window there are no
/// overlays, only pages.
fn verify_device_window_statics(placed: &[PlacedStatic]) -> Result<(), LayoutError> {
    let mut in_window: Vec<&PlacedStatic> = placed
        .iter()
        .filter(|p| p.addr >= DEVICE_WINDOW_LO && p.addr < DEVICE_WINDOW_HI)
        .collect();
    in_window.sort_by_key(|p| p.addr);
    for p in &in_window {
        let end = p.addr.saturating_add(p.size);
        if end > DEVICE_WINDOW_HI {
            return Err(LayoutError::new(format!(
                "placed static `{}` (`{}`) spans {:#x}..{:#x}, past the end of the reserved \
                 device-page window ({DEVICE_WINDOW_LO:#x}..{DEVICE_WINDOW_HI:#x}) — this image \
                 declares too many cores for the per-core `LANE1` stripe \
                 (plans/lane1-per-core.md item A: one {}-byte row per core); place fewer cores",
                p.name, p.ty, p.addr, end, p.size
            )));
        }
    }
    for pair in in_window.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let a_end = a.addr.saturating_add(a.size);
        if a_end > b.addr {
            return Err(LayoutError::new(format!(
                "placed statics `{}` (`{}`, {:#x}..{:#x}) and `{}` (`{}`, at {:#x}) overlap in the \
                 reserved device-page window ({DEVICE_WINDOW_LO:#x}..{DEVICE_WINDOW_HI:#x}) — a \
                 per-core stripe grew into the next page; place fewer cores",
                a.name, a.ty, a.addr, a_end, b.name, b.ty, b.addr
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::boot_init::{
        ActorInit, HandleSpace, actor_inits, boot_init_arg_word, image_decl_handle_word,
    };
    use super::*;
    use crate::codegen::CodegenFn;

    fn fn_words(words: &[u32]) -> CodegenFn {
        CodegenFn {
            frame_size: 16,
            code: words
                .iter()
                .map(|w| {
                    crate::cost::EmittedWord::new(
                        *w,
                        String::new(),
                        crate::cost::CostRule::Alu,
                        None,
                        &[],
                    )
                })
                .collect(),
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
                code: a
                    .words
                    .iter()
                    .map(|w| {
                        crate::cost::EmittedWord::new(
                            *w,
                            String::new(),
                            crate::cost::CostRule::Alu,
                            None,
                            &[],
                        )
                    })
                    .collect(),
                relocs: a.relocs,
            },
        );
        let codegen = crate::codegen::CodegenProgram {
            fns,
            rodata: codegen.rodata.clone(),
            ..Default::default()
        };
        // Wave 1+: test-image layout seeds `__wrela_rt_primary_entry` via
        // `lower_and_codegen_image`. This unit only needs the probe to
        // resolve through ordinary `layout_program` (no boot wiring).
        let laid = layout_program(&codegen, None)
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
        let mut words = vec![encode::enc_bl(0)];
        patch_bl(&mut words, 0, 0x1000, 0x1010).unwrap();
        assert_eq!(words[0], encode::enc_bl(0x10));
    }

    #[test]
    fn patch_bl_encodes_a_negative_offset() {
        let mut words = vec![encode::enc_bl(0)];
        patch_bl(&mut words, 0, 0x2000, 0x1000).unwrap();
        assert_eq!(words[0], encode::enc_bl(-0x1000));
    }

    /// plans/codegen-pareto.md item F5: a tail call is patched as the
    /// `B` the emitter encoded, never rewritten into a `BL`. A `BL`
    /// here would return into a frame this function already dropped.
    #[test]
    fn patch_bl_keeps_a_tail_calls_own_b_form() {
        let mut words = vec![encode::enc_b(0)];
        patch_bl(&mut words, 0, 0x1000, 0x1010).unwrap();
        assert_eq!(words[0], encode::enc_b(0x10));
        assert_ne!(words[0], encode::enc_bl(0x10));
    }

    /// ...and a word that is neither form is a refusal, not a guess.
    #[test]
    fn patch_bl_fails_closed_on_a_word_that_is_not_a_branch() {
        let mut words = vec![0u32];
        assert!(patch_bl(&mut words, 0, 0x1000, 0x1010).is_err());
    }

    #[test]
    fn patch_bl_fails_closed_out_of_range() {
        let mut words = vec![encode::enc_bl(0)];
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

    // --- ADR byte math + the fail-closed range proof (item B1) -----------

    /// plans/codegen-pareto.md decision 1731: `ADR` is byte-granular, so a
    /// forward and a backward site both resolve to a plain signed byte
    /// distance from the instruction's **own** address — no page rounding,
    /// no paired `ADD`, and the live register number read back out of the
    /// placeholder exactly as `patch_adrp_add` reads it.
    #[test]
    fn patch_adr_resolves_a_byte_distance_in_both_directions() {
        let mut words = vec![encode::enc_adr(9, 0)];
        patch_adr(&mut words, 0, 0x4050_0004, 0x4050_0ABC).unwrap();
        assert_eq!(words[0], encode::enc_adr(9, 0x0ABC - 0x0004));

        let mut words = vec![encode::enc_adr(10, 0)];
        patch_adr(&mut words, 0, 0x4050_1000, 0x4050_0040).unwrap();
        assert_eq!(words[0], encode::enc_adr(10, 0x0040 - 0x1000));
    }

    /// The **whole point of freeze 1713**: a site outside `ADR`'s ±1 MiB
    /// reach fails the build, in both directions, and the diagnostic names
    /// the site, the target, the distance and the way out. It never emits a
    /// wrong `ADR` and it never silently falls back (decision 1732).
    #[test]
    fn patch_adr_out_of_range_fails_the_build_rather_than_emitting_a_wrong_adr() {
        // One byte past the positive edge.
        let mut words = vec![encode::enc_adr(9, 0)];
        let this = 0x4050_0000u64;
        let far = this + ADR_HALF_RANGE_BYTES as u64;
        let err = patch_adr(&mut words, 0, this, far).expect_err("must refuse");
        assert!(
            err.message.contains("relocation out of range")
                && err.message.contains("`ADR` at 0x40500000")
                && err.message.contains("1048576 bytes away")
                && err.message.contains("±1 MiB")
                && err.message.contains("OptId::AdrAddressing"),
            "the refusal must name the site, the distance and the way out: {}",
            err.message
        );
        assert_eq!(
            words[0],
            encode::enc_adr(9, 0),
            "the placeholder must be left untouched on refusal"
        );

        // One byte past the negative edge.
        let mut words = vec![encode::enc_adr(9, 0)];
        let this = 0x4050_0000u64;
        let back = this - ADR_HALF_RANGE_BYTES as u64 - 4;
        patch_adr(&mut words, 0, this, back).expect_err("must refuse backward too");

        // And the last in-range site on each side still resolves, so the
        // bound is a bound and not an off-by-one moat.
        let mut words = vec![encode::enc_adr(9, 0)];
        patch_adr(&mut words, 0, this, this + ADR_HALF_RANGE_BYTES as u64 - 4).unwrap();
        let mut words = vec![encode::enc_adr(9, 0)];
        patch_adr(&mut words, 0, this, this - ADR_HALF_RANGE_BYTES as u64).unwrap();
    }

    /// The proof is live on a **real** image, not only on the helper: a
    /// program whose rodata references go through `OptId::AdrAddressing`
    /// lays out, and every `Reloc::RodataAdr` site's resolved word decodes
    /// back to the address the rodata section actually sits at.
    #[test]
    fn an_adr_addressed_image_lays_out_and_every_site_resolves_to_its_rodata_byte() {
        use crate::opts::{CompileMode, apply_mode};

        apply_mode(CompileMode::Release);
        let src = "module examples.layout_adr_rodata\n\npub fn add(a: u8, b: u8) -> u8:\n    \
                   return a + b\n";
        let tokens = crate::syntax::lexer::lex(src).expect("lex");
        let module = crate::syntax::parser::parse(tokens).expect("parse");
        let typed = crate::sema::check_typed(&module, "<test>").expect("check");
        let lctx = crate::mwir::build_layout_ctx(&module, &Default::default()).expect("layout ctx");
        let mwir = crate::lower::lower_program(&typed).expect("lower");
        let program = crate::codegen::codegen_program(&mwir, &lctx).expect("codegen");

        let sites: usize = program
            .fns
            .values()
            .map(|f| {
                f.relocs
                    .iter()
                    .filter(|r| matches!(r, Reloc::RodataAdr { .. }))
                    .count()
            })
            .sum();
        assert!(sites > 0, "this fixture must exercise the new reloc class");

        let out = layout_program(&program, None).expect("an ADR-addressed image must lay out");
        let rodata = out
            .sections
            .iter()
            .find(|s| s.name == "rodata")
            .expect("rodata section");
        let rodata_end = rodata.base + rodata.size as u64;

        // Scan every instruction-bearing section (never `rodata` itself —
        // string bytes can spell any encoding) for `ADR`-shaped words and
        // resolve each one. `enc_adr` has exactly one production caller,
        // `push_rodata_addr`/`load_rodata_addr`, so the count is the site
        // count and each target must land inside rodata.
        let mut found = 0usize;
        for s in &out.sections {
            if s.name == "rodata" {
                continue;
            }
            for i in (0..s.size).step_by(4) {
                let addr = s.base + i as u64;
                let at = (addr - machine_layout::IMAGE_BASE) as usize;
                if at + 4 > out.blob.len() {
                    break;
                }
                let w = u32::from_le_bytes(out.blob[at..at + 4].try_into().expect("4 bytes"));
                if w & 0x9F00_0000 != 0x1000_0000 {
                    continue;
                }
                // Decode imm21 back out of `ADR` and sign-extend it.
                let imm = (((w >> 5) & 0x7FFFF) << 2) | ((w >> 29) & 0x3);
                let imm = ((imm << 11) as i32) >> 11;
                let target = (addr as i64 + imm as i64) as u64;
                assert!(
                    (rodata.base..rodata_end).contains(&target),
                    "section `{}` +{i}: a resolved ADR points at {target:#x}, outside rodata \
                     [{:#x},{rodata_end:#x})",
                    s.name,
                    rodata.base
                );
                found += 1;
            }
        }
        assert_eq!(
            found, sites,
            "every emitted Reloc::RodataAdr site must appear in the blob as a resolved ADR"
        );
    }

    // --- section packing / alignment -------------------------------------

    #[test]
    fn layout_places_entry_then_code_then_abort_when_rodata_is_empty() {
        let mut fns = BTreeMap::new();
        fns.insert("f".to_string(), fn_words(&[0xAABB_CCDD]));
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
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
            ..Default::default()
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
        let mut g = fn_words(&[encode::enc_bl(0)]);
        g.relocs.push(Reloc::Call {
            word: 0,
            key: "f".to_string(),
        });
        fns.insert("f".to_string(), fn_words(&[0x1111_1111, 0x2222_2222]));
        fns.insert("g".to_string(), g);
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
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
            ..Default::default()
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

    /// plans/lane1-per-core.md item A. `LANE1_ROW` is one `Lane1Stripe` row:
    /// three `u64` counters + `METHOD_CALL_POOL_COUNT` method-hit words, the
    /// size the report publishes for a one-core image.
    const LANE1_ROW: u64 = 3 * 8 + (crate::rtconfig::METHOD_CALL_POOL_COUNT as u64) * 8;

    fn window_static(name: &str, addr: u64, size: u64) -> PlacedStatic {
        PlacedStatic {
            name: name.to_string(),
            ty: format!("{name}Ty"),
            addr,
            size,
        }
    }

    /// One `Lane2Counters`: `enabled: u64` + `hits: [u64; BLOCK_POOL_COUNT]`.
    /// plans/M20.md item B raised the pool to 3072, so this is 24584 bytes
    /// where it was 8200 — which is why `LANE1` moved.
    const LANE2_BYTES: u64 = 8 + (crate::rtconfig::BLOCK_POOL_COUNT as u64) * 8;

    #[test]
    fn device_window_accepts_the_live_lane_pages() {
        assert_eq!(LANE1_ROW, 1048);
        assert_eq!(LANE2_BYTES, 24584);
        // plans/M20.md item B: five rows, not the pre-M20 nineteen — the
        // widened `LANE2` page consumes the space the stripe used to grow
        // into. Five still covers the Pi 5 (freeze 1621); the trade is
        // recorded on `rtconfig::BLOCK_POOL_COUNT`.
        for cores in [1u64, 2, 3, 5] {
            let placed = vec![
                window_static("LANE2", 0x4000_8800, LANE2_BYTES),
                window_static("LANE1", 0x4000_e900, cores * LANE1_ROW),
                // Outside the window: never considered.
                window_static("RT", 0x4054_0000, 3072),
            ];
            verify_device_window_statics(&placed)
                .unwrap_or_else(|e| panic!("cores={cores}: {}", e.message));
        }
    }

    #[test]
    fn device_window_refuses_a_stripe_that_reaches_the_next_page() {
        // The pre-item-A address: two rows already walk into `LANE2`, which is
        // exactly why the stripe moved above it.
        let placed = vec![
            window_static("LANE1", 0x4000_8000, 2 * LANE1_ROW),
            window_static("LANE2", 0x4000_8800, LANE2_BYTES),
        ];
        let err = verify_device_window_statics(&placed).expect_err("LANE1 reaches LANE2");
        assert!(
            err.message.contains("overlap") && err.message.contains("LANE2"),
            "{}",
            err.message
        );
    }

    #[test]
    fn device_window_refuses_a_stripe_past_the_end_of_the_window() {
        // plans/M20.md item B: six rows leave the window at the post-M20
        // base (five fit), where twenty were needed at 0x4000b000.
        let placed = vec![
            window_static("LANE2", 0x4000_8800, LANE2_BYTES),
            window_static("LANE1", 0x4000_e900, 6 * LANE1_ROW),
        ];
        let err = verify_device_window_statics(&placed).expect_err("6 rows leave the window");
        assert!(
            err.message
                .contains("past the end of the reserved device-page window"),
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
            cores: 1,
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

    /// plans/codegen-pareto.md decision 1754: the same-region property is
    /// **proved**, not assumed from an aligned base. A branchable text span
    /// that straddles a 2 MiB boundary is refused, and one that does not is
    /// accepted whether or not its base is aligned.
    #[test]
    fn verify_branch_region_refuses_a_straddling_text_span() {
        let straddle = vec![
            Section {
                name: "entry",
                base: 0x405F_FFF0,
                size: 16,
            },
            Section {
                name: "code",
                base: 0x4060_0000,
                size: 16,
            },
        ];
        let err = verify_branch_region(&straddle).expect_err("straddling must fail closed");
        assert!(err.message.contains("straddles"), "{}", err.message);

        // The image as it is actually laid out today (`boot-actors`, items
        // A+B+C): an unaligned base whose whole text still lives in one
        // region, by a factor of ~24. The sizes are a fixture — the real
        // check runs on every image build — but they are the real sizes, so
        // the margin this fixture claims is the margin the image has.
        let real = vec![
            Section {
                name: "entry",
                base: 0x4050_0000,
                size: 80,
            },
            Section {
                name: "code",
                base: 0x4050_0050,
                size: 85136,
            },
            Section {
                name: "abort",
                base: 0x4051_4ff4,
                size: 120,
            },
            Section {
                name: "checkpoint",
                base: 0x4051_506c,
                size: 28,
            },
        ];
        assert!(verify_branch_region(&real).is_ok());
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
            ..Default::default()
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
            ..Default::default()
        }
    }

    #[test]
    fn transcript_bound_counts_one_line_per_test_plus_the_summary() {
        let program = program_with_rodata(b"boom");
        let tests = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let bound = compute_transcript_bound(&program, &tests);
        // 3 tests + 1 summary + 2 lane1 + the item-B quiesce-timeout line
        assert_eq!(bound.lines, 7);
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
            ..Default::default()
        };
        let tests = vec!["only_test".to_string()];
        let bound = compute_transcript_bound(&program, &tests);
        // "test " (5) + "only_test" (9) + ": " (2) = 16, plus
        // max(3, 7 + 2*len(DEADLOCK_MSG) + 20 + 1) for the one test line
        // (the deadlock diagnostic is the longest message even with an
        // empty rodata pool), plus the summary's own exact 2*20+9+8=57.
        let failed_len = 7 + 2 * DEADLOCK_MSG.len() as u64 + 20 + 1;
        // + lane1 scalar line + the item-B `lane1 quiesce=timeout` line
        // + hits over-approx (METHOD_CALL_POOL_COUNT pairs).
        const LANE1_SCALAR: u64 = 12 + 20 + 9 + 20 + 10 + 20 + 1;
        const LANE1_QUIESCE: u64 = 21 + 1;
        let lane1_hits = 11 + lane1_pair_bytes() + 1;
        assert_eq!(
            bound.worst_case_bytes,
            16 + failed_len + 57 + LANE1_SCALAR + LANE1_QUIESCE + lane1_hits
        );
    }

    /// **Decision 1610's proof obligation.** The Lane 1 and Lane 2 hits
    /// reservations must over-approximate the widest line the guest can
    /// actually print, which is what `harness.rs` claims about itself and
    /// what item B measured to be false for Lane 2.
    ///
    /// The guest's widest line is exact arithmetic, not a guess:
    /// `"lane1 hits="` / `"lane2 hits="` (11 B) + `n` pairs of
    /// `<id>:<count>` at their real digit widths + `n - 1` separating
    /// commas + (Lane 2 only) `" truncated="` and its count + `"\n"`.
    /// `n` is `METHOD_CALL_POOL_COUNT` for Lane 1 and
    /// `BLOCK_BOUND_PRINT_PAIRS` for Lane 2 — the latter because the guest
    /// dump now stops there and reports the rest in the marker.
    #[test]
    fn lane_hit_reservations_over_approximate_the_widest_printable_line() {
        const COUNT_DIGITS: u64 = 20; // u64 guest counter: no static bound.

        let n = crate::rtconfig::METHOD_CALL_POOL_COUNT as u64;
        // A flat method index is < METHOD_CALL_POOL_COUNT = 128 → 3 digits.
        let lane1_widest = n * (3 + 1 + COUNT_DIGITS) + (n - 1);
        assert!(
            lane1_pair_bytes() >= lane1_widest,
            "lane 1 reservation {} must cover the widest printable pair list {lane1_widest}",
            lane1_pair_bytes()
        );
        assert_eq!(
            lane1_pair_bytes() - lane1_widest,
            1,
            "and must not be loose by more than the one byte the trailing-comma \
             over-charge costs"
        );

        let pairs = crate::rtconfig::BLOCK_BOUND_PRINT_PAIRS as u64;
        // A Lane 2 id is < BLOCK_POOL_COUNT = 3072 → 4 digits.
        let lane2_widest = pairs * (4 + 1 + COUNT_DIGITS) + (pairs - 1);
        assert!(
            lane2_pair_bytes() >= lane2_widest,
            "lane 2 reservation {} must cover the widest printable pair list {lane2_widest}",
            lane2_pair_bytes()
        );
        assert_eq!(lane2_pair_bytes() - lane2_widest, 1);

        // The marker itself: ` truncated=` + at most BLOCK_POOL_COUNT.
        let marker_widest = " truncated=".len() as u64 + 4;
        assert!(
            lane2_marker_bytes() >= marker_widest,
            "the truncation marker must be reserved for, not discovered at run time"
        );

        // And the whole Lane 2 line fits the console with Lane 1's own
        // reservation alongside it — the arithmetic decision 1610 states.
        let lane1_line = 11 + lane1_pair_bytes() + 1;
        let lane2_line = 11 + lane2_pair_bytes() + lane2_marker_bytes() + 1;
        assert_eq!(lane1_line, 3212, "lane 1 hits line reservation");
        assert_eq!(lane2_line, 3355, "lane 2 hits line reservation");
        assert!(
            lane1_line + lane2_line < console::DATA_SIZE,
            "both hit lines together must leave room for the test/summary lines"
        );
    }

    /// The Lane 2 reservation tracks `BLOCK_BOUND_PRINT_PAIRS`, not the
    /// (unbounded) real non-zero block count — so raising the pool cannot
    /// silently make the bound false again the way item B's widening did.
    #[test]
    fn lane2_reservation_is_bounded_by_the_print_pair_cap() {
        let per_pair = lane2_pair_bytes() / (crate::rtconfig::BLOCK_BOUND_PRINT_PAIRS as u64);
        assert_eq!(per_pair, 4 + 1 + 20 + 1, "id digits from BLOCK_POOL_COUNT");
        assert!(
            lane2_pair_bytes() < (crate::rtconfig::BLOCK_POOL_COUNT as u64) * per_pair,
            "the reservation must be the *printable* cap, not the whole pool"
        );
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
                code: vec![crate::cost::EmittedWord::new(
                    encode::enc_ret(30),
                    "ret".to_string(),
                    crate::cost::CostRule::Branch,
                    None,
                    &[30],
                )],
                relocs: Vec::new(),
            },
        );
        let program = CodegenProgram {
            fns,
            rodata: Vec::new(),
            ..Default::default()
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
