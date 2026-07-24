//! The wrela machine contract, as Rust types and constants.
//!
//! Normative source: docs/language/06-machine.md. This crate is shared by
//! the compiler (which emits images for the machine) and the VMM (which
//! implements it). If a value here disagrees with the doc, the doc wins and
//! this crate is wrong.
//!
//! Machine layout contract v1 (plans/M5.md decision 6): every address and
//! size the compiler's emission (item D) and the VMM's boot (item E) must
//! agree on lives here, once, as `pub const`s — this crate's whole reason
//! to exist. Full behavioral coverage (the VMM actually reading the
//! machine-info page, actually starting vCPU 0 at `IMAGE_BASE`, actually
//! draining the console ring) arrives with item E's boot; this item's own
//! oracle is the non-overlap unit test at the bottom of this file, which
//! proves the map is internally coherent before anything is built on top
//! of it.

/// Machine contract revision (numeric form). The compiler seals this into
/// the build identity; the VMM refuses an image built for another
/// revision. Pre-dates this item; unchanged.
pub const MACHINE_REVISION: u32 = 1;

/// Machine contract revision, as the string form 06-machine.md names in
/// its own title (`**wrela-machine-v1**`): fixed 32-byte, NUL-padded, the
/// exact byte layout `machine_info::OFF_REVISION` holds in the guest and
/// the report's build identity uses (both wired up at item D/E; this
/// crate only defines the one shared string so neither side can drift).
pub const MACHINE_REVISION_STR: &str = "wrela-machine-v1";

/// Always four vCPUs. Hosts with more cores run VMM threads on the surplus.
pub const VCPUS: usize = 4;

/// Guest-physical memory layout (06-machine.md §2). Flagship profile: one
/// fixed layout, published here per machine revision, that the compiler's
/// emission and the VMM's boot both consume — no discovery, no negotiation
/// (06 §3).
///
/// Map, low to high (all addresses are guest-physical, all sizes in
/// bytes; nothing here overlaps — `tests::regions_do_not_overlap` walks
/// every region below and proves it):
///
/// ```text
/// 0x4000_0000  MACHINE_INFO_BASE   (4 KiB)   machine-info page
/// 0x4000_1000  console::RING_BASE  (8 KiB)   console ring metadata + doorbell
/// 0x4000_3000  console::DATA_BASE  (16 KiB)  console tx byte buffers
/// 0x4000_7000  pending::BASE       (4 KiB)   per-core pending-vector page
/// 0x4000_8000  .. 0x4001_0000               reserved (device-page growth)
/// 0x4001_0000  STACKS_BASE         (4 MiB)   4 per-core stacks, 1 MiB each
/// 0x4041_0000  .. 0x4050_0000               reserved (stack growth room)
/// 0x4050_0000  IMAGE_BASE          (rest)    sealed image, loaded flat
/// ```
///
/// (`console::RING_BASE`'s own region grew from 4 KiB to 8 KiB, and
/// `console::DATA_BASE` moved from `0x4000_2000` to `0x4000_3000`
/// accordingly, when `console::QUEUE_SIZE` grew from 16 to 256 — the
/// M5-G adversarial sweep's own find/fix, `console`'s own module doc
/// below tells the whole story. `console::DATA_SIZE` itself is
/// unchanged at 16 KiB.)
///
/// Below `DRAM_BASE` entirely, in a separate address range the VMM never
/// backs with real RAM pages: the two trapped MMIO registers
/// (`CLOCK_MMIO_ADDR`, `EXIT_MMIO_ADDR`) — see their own doc comments for
/// why they live outside the RAM window.
pub mod layout {
    /// Total guest DRAM for the flagship profile (06-machine.md §2: "the
    /// flagship profile is 1 GiB").
    pub const DRAM_SIZE: u64 = 1 << 30; // 1 GiB

    /// Guest-physical DRAM base. `0x4000_0000` is the conventional
    /// aarch64 VM RAM base (matches QEMU's `virt` machine and most
    /// hand-rolled aarch64 bare-metal setups) — chosen for familiarity to
    /// anyone reading a disassembly or a HVF register dump, not because
    /// anything below imitates real hardware (06's own framing: "nothing
    /// below imitates real hardware unless imitation is useful").
    pub const DRAM_BASE: u64 = 0x4000_0000;

    /// One page (06 §3: "points `x0` at the machine-info page"). Field
    /// layout lives in `machine_info`, below.
    pub const MACHINE_INFO_BASE: u64 = DRAM_BASE;
    pub const MACHINE_INFO_SIZE: u64 = 0x1000;

    /// Reserved region for the 4 per-core stacks (06 §1: "4 vCPUs,
    /// always"). Only core 0 runs at M5 (plans/M5.md decision 11: "cores
    /// 1-3 are reserved in the layout but never started (sync only)") —
    /// all four are still reserved here so a later milestone starting the
    /// other cores needs no layout change, only a VMM change.
    pub const STACKS_BASE: u64 = 0x4001_0000;
    /// 1 MiB per core. Generous for a spill-everything frame convention
    /// (plans/M5.md decision 4) with no register allocation to shrink
    /// frames; revisit only with a profile (CLAUDE.md's cleverness
    /// budget), never preemptively.
    pub const CORE_STACK_SIZE: u64 = 1 << 20; // 1 MiB

    /// Guest-physical base of core `n`'s stack (`n` in `0..VCPUS`), i.e.
    /// the stack pointer's *initial* value: SP grows down, so codegen
    /// initializes `sp = core_stack_base(n) + CORE_STACK_SIZE`, the top
    /// of the reservation, and the reservation's own base is the lowest
    /// address that stack may ever reach.
    pub const fn core_stack_base(core: usize) -> u64 {
        STACKS_BASE + (core as u64) * CORE_STACK_SIZE
    }

    /// Sealed image load base; vCPU 0 starts at the image entry here (06
    /// §3). Placed a round 5 MiB into RAM, after the info/console pages
    /// and the stacks region, with reserved padding on both sides
    /// (documented in the module-level map above) for pages a later
    /// milestone might add without having to move the image itself.
    pub const IMAGE_BASE: u64 = 0x4050_0000;
}

/// The machine-info page's field layout (06 §3: "machine revision,
/// wall-clock seed, provisioned secrets channel" — the secrets channel is
/// stdlib-milestone territory, not named as a field here yet; the four
/// fields M5 actually needs are). One page (`layout::MACHINE_INFO_SIZE`),
/// all fields packed at the front with 8-byte alignment; the remainder of
/// the page is unused padding, reserved for later fields the same way
/// `next_deadline` is reserved for M6 today.
pub mod machine_info {
    /// Offset 0: the machine revision string (`MACHINE_REVISION_STR`),
    /// fixed 32 bytes, NUL-padded — the guest can assert its own build
    /// was booted on the revision it expects without an MMIO round trip.
    pub const OFF_REVISION: u64 = 0x00;
    pub const REVISION_FIELD_SIZE: u64 = 32;

    /// Offset 0x20 (32): wall-clock seed, `u64`. The VMM's one reference
    /// point for `now()`'s wall-clock component (06 §3); the monotonic
    /// side is `CLOCK_MMIO_ADDR` reads (decision 13).
    pub const OFF_WALL_SEED: u64 = 0x20;

    /// Offset 0x28 (40): the next-deadline slot, `u64` — 06 §5's park
    /// protocol ("when a core's scheduler has no ready work it parks with
    /// its next deadline written to the machine-info page"). Reserved,
    /// unused at M5 (sync-only, no parking): the offset is fixed now so
    /// M6 does not have to move any other field to make room.
    pub const OFF_NEXT_DEADLINE: u64 = 0x28;

    /// Offset 0x30 (48): guest-exit code word, `u64`. Decision E's exit
    /// protocol is the trapping store to `mmio::EXIT_MMIO_ADDR`; the
    /// generated runtime writes the same exit code here *first*, as a
    /// plain (non-trapping) store, so the value is visible in an ordinary
    /// guest memory dump (replay logs, a debugger, a crashed-boot
    /// post-mortem) even though the actual "I'm done" signal is the MMIO
    /// trap, not this write.
    pub const OFF_EXIT_CODE: u64 = 0x30;

    /// Offset 0x38 (56): the runtime test harness's own landing-pad
    /// scratch slot (plans/M5.md item E, `wrela-compiler/src/layout.rs`'s
    /// `layout_test_image`) — a `u64` guest address the generated entry
    /// driver stores *right before* calling each `@test(runtime)` fn: the
    /// address to resume the driver at if that call ever aborts (a
    /// per-test fixed "continuation" — the next test's own setup step,
    /// never the aborted call's own `Return`). `__wrela_abort`/
    /// `__wrela_abort_val`, in the test-image build only, load this
    /// address and branch to it directly (`BR`, never `RET`) instead of
    /// unwinding the (arbitrarily deep) aborted call stack — the "landing
    /// pad" mechanism named in the plan. An ordinary (non-test) image
    /// never writes this offset at all; only the test-image entry/abort
    /// bodies reference it.
    pub const OFF_TEST_CONTINUATION: u64 = 0x38;

    /// Offset 0x40/0x48 (64/72): the runtime harness's own pass/fail
    /// counters, `u64` each, zeroed by the entry driver at boot and
    /// incremented by the entry driver (`OFF_TEST_PASSED`, on an ordinary
    /// return) or by the abort routines (`OFF_TEST_FAILED`, on the
    /// landing-pad path above) — kept in guest memory, not a register,
    /// because the landing pad's own `BR` crosses an arbitrary number of
    /// intervening call frames and nothing about this ABI's `BL`
    /// preserves a scratch register across that; a fixed memory slot
    /// survives unconditionally. Read back by the entry driver once, at
    /// the very end, to render the pinned `<N> passed, <M> failed`
    /// summary line.
    pub const OFF_TEST_PASSED: u64 = 0x40;
    pub const OFF_TEST_FAILED: u64 = 0x48;

    /// Offset 0x50/0x58 (80/88): the runtime harness's own console-ring
    /// bookkeeping, `u64` each, zeroed at boot — `OFF_RING_DATA_BUMP` is
    /// the next free byte offset within `console::DATA_SIZE` (a bump
    /// allocator: M5's console model drains the whole ring once, after
    /// the guest halts, decision 12, so nothing is ever reclaimed);
    /// `OFF_RING_DESC_BUMP` is the next free descriptor/avail-ring index
    /// (`0..console::QUEUE_SIZE`). `__wrela_ring_write` (the test image's
    /// own harness subroutine) consumes one descriptor per call — the M5
    /// harness's own documented bound of at most `console::QUEUE_SIZE`
    /// `__wrela_ring_write` calls (in practice, printed report lines) per
    /// boot.
    pub const OFF_RING_DATA_BUMP: u64 = 0x50;
    pub const OFF_RING_DESC_BUMP: u64 = 0x58;

    /// Offset 0x60 (96): the runtime harness's own **line-in-progress**
    /// anchor, `u64`, zeroed at boot — the M5-G adversarial-sweep fix
    /// (`crates/wrela-compiler/src/layout.rs`'s module doc tells the
    /// full story): the harness composes a whole report line (a test's
    /// `test <name>: ok\n`/`test <name>: FAILED ...\n`, or the summary
    /// line) into the console data region across *several*
    /// `__wrela_ring_append` calls before publishing exactly **one**
    /// descriptor for the finished line. `__wrela_line_begin` snapshots
    /// the current `OFF_RING_DATA_BUMP` value here, right before the
    /// first append of a new line; `__wrela_line_commit` reads it back
    /// to compute the finished line's own start address (`console::
    /// DATA_BASE + OFF_LINE_START`) and length (`OFF_RING_DATA_BUMP -
    /// OFF_LINE_START`) for that one descriptor. This is the fix for
    /// the bug the original one-descriptor-per-call scheme had: a test
    /// whose report line took more than one `__wrela_ring_write` call
    /// (every failing test did — prefix, message, newline, at minimum)
    /// spent multiple of `console::QUEUE_SIZE`'s then-tiny 16 slots per
    /// test, silently truncating the transcript well before every
    /// planned test had even run. One line, one descriptor, always,
    /// regardless of how many pieces compose it — the static bound
    /// `layout.rs` now enforces at test-image build time is exactly
    /// "at most `console::QUEUE_SIZE` *lines*" as a result.
    pub const OFF_LINE_START: u64 = 0x60;

    /// Offset 0x100 (256), size 256 bytes: a scratch line buffer the
    /// runtime harness composes a decimal integer's ASCII digits into
    /// (`__wrela_fmt_dec`, used by the summary line's pass/fail counts and
    /// by `__wrela_abort_val`'s own runtime-value interpolation) before
    /// handing the bytes to `__wrela_ring_write` — comfortably larger
    /// than any digit sequence a 64-bit value ever needs (at most 20
    /// digits plus a sign). Placed well past every field above, this
    /// page's own "remainder is unused padding, reserved for later
    /// fields" convention (module doc, above).
    pub const OFF_TEST_LINE_BUF: u64 = 0x100;
    pub const TEST_LINE_BUF_SIZE: u64 = 256;

    /// Offset 0x200 (512): plans/M6.md item E's own vector-observation
    /// counter, `u64`, zeroed at boot — `__wrela_vector0_service`'s whole
    /// body (`wrela-compiler/src/layout.rs::build_checkpoint_and_vector_stub`)
    /// increments this every time the checkpoint service dispatches the
    /// one M6 vector (index 0, the deadline/cancel vector), purely so a
    /// conformance test can observe "the vector was actually delivered and
    /// serviced" from outside the guest (a memory flag, per 06 §4's own
    /// checkpoint-injection contract) without needing a console/report
    /// line. Item F's real group-cancellation delivery is expected to grow
    /// this routine's body into real cancellation work; the counter itself
    /// may stay alongside it as a cheap observability hook, or retire —
    /// either way this offset is reserved now, past `OFF_TEST_LINE_BUF`'s
    /// own 256-byte scratch region, so nothing else needs to move.
    pub const OFF_VECTOR0_OBSERVED: u64 = 0x200;
}

/// Console: a runtime-owned tx ring, virtio-shaped (plans/M5.md decision
/// 12 — this is *not* the stdlib virtio-console driver, which arrives
/// with async at M6+; this is the fixed-function path the generated
/// runtime speaks to print report lines at M5). One page of ring
/// metadata (descriptor table + avail ring + used ring + doorbell word),
/// immediately followed by a separate data region the descriptors'
/// addresses point into.
///
/// The ring shape mirrors virtio's split ring (a descriptor table, an
/// avail ring the driver/guest writes, a used ring the device/VMM writes)
/// but is not the full virtio spec: no `used_event`/`avail_event`
/// (`VIRTIO_RING_F_EVENT_IDX`) fields, since the M5 runtime is a single
/// producer with no interrupt suppression to negotiate.
///
/// `QUEUE_SIZE` was originally 16 (one page held the whole ring with room
/// to spare) but the M5-G adversarial sweep found a real bug that number
/// caused: the generated runtime spent one descriptor per
/// `__wrela_ring_write` *call*, not per printed report line, so a mere
/// 7-8 `@test(runtime)` fns (each failing test alone needed 3-4 calls —
/// prefix, message, newline) silently exhausted the queue mid-summary,
/// truncating the transcript. The fix (`wrela-compiler/src/layout.rs`'s
/// module doc has the full design) makes "one descriptor per line" a
/// hard invariant, and separately grows `QUEUE_SIZE` to 256 so a build
/// with genuinely many tests still fits — enforced *statically*, at
/// test-image build time (`layout.rs::check_transcript_bound`), never
/// silently truncated at runtime. 256 descriptor entries (16 bytes each)
/// alone are exactly one page (4 KiB), so the ring's own metadata no
/// longer fits in one page with the avail/used rings and doorbell beside
/// it — `RING_SIZE` grew from one page (4 KiB) to two (8 KiB) to hold all
/// of it with room to spare (`RING_USED_BYTES` is 6672 of the 8192
/// available).
pub mod console {
    use super::layout::DRAM_BASE;

    /// Queue depth. Module doc above tells the whole "16 -> 256" story.
    pub const QUEUE_SIZE: u64 = 256;

    /// Ring metadata page(s): descriptor table + avail ring + used ring +
    /// doorbell word, in that order. Two pages (8 KiB) since `QUEUE_SIZE`
    /// grew to 256 (module doc above).
    pub const RING_BASE: u64 = DRAM_BASE + 0x1000;
    pub const RING_SIZE: u64 = 2 * 0x1000;

    /// Descriptor table: `QUEUE_SIZE` entries of 16 bytes each (`addr:
    /// u64, len: u32, flags: u16, next: u16` — the virtio descriptor
    /// layout unchanged).
    pub const DESC_TABLE_OFFSET: u64 = 0;
    pub const DESC_ENTRY_SIZE: u64 = 16;
    pub const DESC_TABLE_SIZE: u64 = QUEUE_SIZE * DESC_ENTRY_SIZE; // 256

    /// Avail ring: `flags: u16, idx: u16, ring: [u16; QUEUE_SIZE]` (no
    /// `used_event`, per the module doc above).
    pub const AVAIL_OFFSET: u64 = DESC_TABLE_OFFSET + DESC_TABLE_SIZE; // 256
    pub const AVAIL_SIZE: u64 = 4 + 2 * QUEUE_SIZE; // 36

    /// Used ring: `flags: u16, idx: u16, ring: [(id: u32, len: u32);
    /// QUEUE_SIZE]` (no `avail_event`).
    pub const USED_OFFSET: u64 = AVAIL_OFFSET + AVAIL_SIZE; // 292
    pub const USED_SIZE: u64 = 4 + 8 * QUEUE_SIZE; // 132

    /// Doorbell word: the guest stores any nonzero value here after
    /// publishing new avail entries; the VMM's console model polls/wakes
    /// on it and drains the ring to the captured transcript (06 §5: "hot
    /// paths never trap" — this is a shared-memory doorbell, not an MMIO
    /// trap, unlike the clock and exit words below).
    pub const DOORBELL_OFFSET: u64 = USED_OFFSET + USED_SIZE; // 424
    pub const DOORBELL_SIZE: u64 = 8;

    /// Bytes actually used by ring metadata + doorbell (432 of the 4096
    /// available in `RING_SIZE`) — asserted against `RING_SIZE` by the
    /// non-overlap test below, so a future field addition that overflows
    /// the page fails loudly instead of silently colliding with the data
    /// region.
    pub const RING_USED_BYTES: u64 = DOORBELL_OFFSET + DOORBELL_SIZE;

    /// Data region the ring's descriptors point into: the actual bytes of
    /// every report line the runtime writes. 4 pages (16 KiB) — small,
    /// like the queue depth, and sized for report-line traffic, not
    /// arbitrary console volume.
    pub const DATA_BASE: u64 = RING_BASE + RING_SIZE;
    pub const DATA_SIZE: u64 = 4 * 0x1000;
}

/// Vector delivery (06-machine.md §4, plans/M6.md decision 7): "each
/// virtual vector is a word in a per-core shared-memory page; the VMM
/// raises a vector by a store-release plus a wake of the target vCPU ...
/// the guest observes vectors only at checkpoints and parks, via the same
/// mask–arm–recheck protocol". This is that page's own layout — the
/// contract item C adds *now*, coherently, so item E's VMM-side raise and
/// item D's checkpoint-emitted mask–arm–recheck sequence agree on the same
/// addresses from the start; nothing in this crate or the compiler reads
/// or writes these words yet (M6-C's own event loop never truly parks, its
/// own module doc in `wrela-compiler/src/layout.rs` records why) — that is
/// item E/D's job, not a reason to leave the addresses undecided.
///
/// One word per core (`VCPUS` of them, 8 bytes each — room for 64 vector
/// bits per core, far more than M6's one real injector (decision 7: "the
/// deadline service") ever needs, but a bitmask-per-core is the dumbest
/// shape that generalizes to more vectors later without moving anything).
/// The remainder of the page is reserved.
pub mod pending {
    use super::layout::DRAM_BASE;

    /// One page, immediately after the console data region and before the
    /// generic device-page-growth reservation (module-level map above).
    pub const BASE: u64 = DRAM_BASE + 0x7000;
    pub const SIZE: u64 = 0x1000;

    /// Bytes actually used: one `u64` pending-word per core.
    pub const WORD_SIZE: u64 = 8;

    /// Guest-physical address of core `n`'s own pending word (`n` in
    /// `0..VCPUS`) — the word a checkpoint's mask–arm–recheck sequence
    /// (item D) loads, and the word the VMM's deadline-wake/vector-raise
    /// path (item E) stores to before waking that core's vCPU.
    pub const fn core_word_addr(core: usize) -> u64 {
        BASE + (core as u64) * WORD_SIZE
    }
}

/// Device MMIO window: a small, entirely separate physical address range
/// below `layout::DRAM_BASE`, never backed by real RAM pages. Any access
/// here is necessarily unmapped from the guest's point of view, so it
/// takes a data-abort exit under HVF/KVM — that trap *is* the mechanism
/// (06 §5: "MMIO-shaped register access exists only on setup/reset
/// paths, where a trap is fine"; decision 13 spells out the clock read
/// case, decision E the exit case). Neither register is meant to be
/// read/written fast-path; both are exactly the kind of setup/reset
/// access 06 §5 carves the trap exception out for.
pub mod mmio {
    pub const MMIO_BASE: u64 = 0x0800_0000;

    /// One `u64` read: monotonic nanoseconds since boot (decision 13:
    /// "`now()` traps at M5 ... every read exits to the VMM, which
    /// returns monotonic ns and logs the value" — the log entry is the
    /// recorder's clock-read log, 06 §8's replay subset).
    pub const CLOCK_MMIO_ADDR: u64 = MMIO_BASE;

    /// One `u64` store: the guest's exit code. Decision E's whole guest-
    /// exit protocol — "a store of the exit code there is the 'image
    /// done' signal" — no PSCI, no hypercall instruction, just a trapped
    /// store to an address nothing else ever touches. Placed a full page
    /// after `CLOCK_MMIO_ADDR` purely so the two registers are trivially
    /// distinguishable by address range in a fault handler, not because
    /// either is wider than 8 bytes.
    pub const EXIT_MMIO_ADDR: u64 = MMIO_BASE + 0x1000;

    /// One `u64` store: the park protocol's own trapping doorbell
    /// (plans/M6.md item E, 06 §5: "when a core's scheduler has no ready
    /// work it parks with its next deadline written to the machine-info
    /// page; the VMM sleeps the vCPU thread until that deadline or a
    /// wake"). The dumbest reliable, deterministic mechanism available on
    /// both HVF and a future KVM backend — a trapping store, mirroring
    /// `EXIT_MMIO_ADDR`'s own precedent exactly (a WFI trap would work on
    /// HVF too, but a store-exit needs no extra per-backend trap-class
    /// decoding beyond the `decode_data_abort` this crate's consumers
    /// already have for the clock/exit registers, and generalizes
    /// identically to KVM's own MMIO-exit machinery). Protocol: the guest
    /// writes its own next deadline to `machine_info::OFF_NEXT_DEADLINE`
    /// (an ordinary, non-trapping store — visible in a plain guest memory
    /// dump, mirroring `OFF_EXIT_CODE`'s own convention) and only then
    /// performs *this* trapping store (any value; the VMM reads the real
    /// deadline back from `OFF_NEXT_DEADLINE`, not from the stored value
    /// itself) — the VMM rechecks this core's own pending word
    /// (`pending::core_word_addr`) before deciding to sleep at all (the
    /// mask-arm-recheck discipline's own "recheck" half, 06 §4: "a wake
    /// between test and park cannot be lost"): a vector already pending at
    /// the moment of this trap means the VMM resumes the vCPU immediately,
    /// no sleep, so the guest's own next checkpoint (the park's own resume
    /// point counts as one, by construction) observes it right away.
    /// Placed a further page after `EXIT_MMIO_ADDR`, the identical
    /// separate-page-per-register convention.
    pub const PARK_MMIO_ADDR: u64 = MMIO_BASE + 0x2000;
}

/// The closed device set of machine v1 (06-machine.md §6). There is no
/// hotplug; a new device is a machine revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Device {
    Blk,
    Net,
    Input,
    Console,
    Entropy,
    Sound,
    Display,
    Clock,
}

impl Device {
    pub const ALL: [Device; 8] = [
        Device::Blk,
        Device::Net,
        Device::Input,
        Device::Console,
        Device::Entropy,
        Device::Sound,
        Device::Display,
        Device::Clock,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Device::Blk => "blk",
            Device::Net => "net",
            Device::Input => "input",
            Device::Console => "console",
            Device::Entropy => "entropy",
            Device::Sound => "sound",
            Device::Display => "display",
            Device::Clock => "clock",
        }
    }
}

/// Flagship display mode (06-machine.md §7). 4K is a stretch profile.
pub mod display {
    pub const WIDTH: u32 = 1920;
    pub const HEIGHT: u32 = 1080;
    pub const REFRESH_HZ: u32 = 60;
    pub const BYTES_PER_PIXEL: u32 = 4;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every declared guest-physical region, name + base + size, RAM and
    /// MMIO alike. The one property this test exists to prove: none of
    /// them overlap. A future edit that shrinks a gap into a collision
    /// fails here before it ever reaches emission (item D) or boot (item
    /// E).
    fn regions() -> Vec<(&'static str, u64, u64)> {
        vec![
            (
                "machine_info",
                layout::MACHINE_INFO_BASE,
                layout::MACHINE_INFO_SIZE,
            ),
            ("console_ring", console::RING_BASE, console::RING_SIZE),
            ("console_data", console::DATA_BASE, console::DATA_SIZE),
            ("pending", pending::BASE, pending::SIZE),
            (
                "core_stack_0",
                layout::core_stack_base(0),
                layout::CORE_STACK_SIZE,
            ),
            (
                "core_stack_1",
                layout::core_stack_base(1),
                layout::CORE_STACK_SIZE,
            ),
            (
                "core_stack_2",
                layout::core_stack_base(2),
                layout::CORE_STACK_SIZE,
            ),
            (
                "core_stack_3",
                layout::core_stack_base(3),
                layout::CORE_STACK_SIZE,
            ),
            // The image region has no fixed size (it is whatever the
            // build emits), so it is checked here only as a zero-size
            // marker: it must start after every region above it and
            // before DRAM ends, which the two dedicated tests below
            // (not this table) prove.
            ("image_base", layout::IMAGE_BASE, 0),
            ("clock_mmio", mmio::CLOCK_MMIO_ADDR, 8),
            ("exit_mmio", mmio::EXIT_MMIO_ADDR, 8),
            ("park_mmio", mmio::PARK_MMIO_ADDR, 8),
        ]
    }

    #[test]
    fn regions_do_not_overlap() {
        let mut rs = regions();
        rs.sort_by_key(|&(_, base, _)| base);
        for pair in rs.windows(2) {
            let (name_a, base_a, size_a) = pair[0];
            let (name_b, base_b, _) = pair[1];
            assert!(
                base_a + size_a <= base_b,
                "region `{name_a}` (0x{base_a:x}..0x{:x}) overlaps `{name_b}` (0x{base_b:x}..)",
                base_a + size_a
            );
        }
    }

    #[test]
    fn ram_regions_fit_inside_dram() {
        let dram_end = layout::DRAM_BASE + layout::DRAM_SIZE;
        for (name, base, size) in regions() {
            // MMIO lives outside DRAM entirely (module doc: "a separate
            // physical address range below layout::DRAM_BASE").
            if name.ends_with("_mmio") {
                assert!(
                    base < layout::DRAM_BASE,
                    "MMIO region `{name}` unexpectedly overlaps the DRAM window"
                );
                continue;
            }
            assert!(
                base >= layout::DRAM_BASE && base + size <= dram_end,
                "region `{name}` (0x{base:x}, size {size}) does not fit inside DRAM (0x{:x}..0x{dram_end:x})",
                layout::DRAM_BASE
            );
        }
    }

    #[test]
    fn image_base_leaves_room_before_dram_end() {
        let dram_end = layout::DRAM_BASE + layout::DRAM_SIZE;
        assert!(layout::IMAGE_BASE < dram_end);
        // Sanity: there is a real image budget left, not a sliver.
        assert!(dram_end - layout::IMAGE_BASE > 512 * (1 << 20));
    }

    #[test]
    fn console_ring_metadata_fits_the_ring_page() {
        assert!(console::RING_USED_BYTES <= console::RING_SIZE);
    }

    #[test]
    fn pending_words_fit_the_page_and_do_not_overlap() {
        let used = VCPUS as u64 * pending::WORD_SIZE;
        assert!(used <= pending::SIZE);
        for core in 0..VCPUS {
            let addr = pending::core_word_addr(core);
            assert!(addr >= pending::BASE && addr + pending::WORD_SIZE <= pending::BASE + used);
        }
        // Every core's word is distinct and 8-byte-spaced.
        for a in 0..VCPUS {
            for b in (a + 1)..VCPUS {
                let (addr_a, addr_b) = (pending::core_word_addr(a), pending::core_word_addr(b));
                assert!(
                    addr_a + pending::WORD_SIZE <= addr_b || addr_b + pending::WORD_SIZE <= addr_a
                );
            }
        }
    }

    #[test]
    fn machine_info_fields_fit_the_page_and_do_not_overlap() {
        assert!(
            machine_info::OFF_REVISION + machine_info::REVISION_FIELD_SIZE
                <= machine_info::OFF_WALL_SEED
        );
        assert!(machine_info::OFF_WALL_SEED + 8 <= machine_info::OFF_NEXT_DEADLINE);
        assert!(machine_info::OFF_NEXT_DEADLINE + 8 <= machine_info::OFF_EXIT_CODE);
        assert!(machine_info::OFF_EXIT_CODE + 8 <= machine_info::OFF_TEST_CONTINUATION);
        assert!(machine_info::OFF_TEST_CONTINUATION + 8 <= machine_info::OFF_TEST_PASSED);
        assert!(machine_info::OFF_TEST_PASSED + 8 <= machine_info::OFF_TEST_FAILED);
        assert!(machine_info::OFF_TEST_FAILED + 8 <= machine_info::OFF_RING_DATA_BUMP);
        assert!(machine_info::OFF_RING_DATA_BUMP + 8 <= machine_info::OFF_RING_DESC_BUMP);
        assert!(machine_info::OFF_RING_DESC_BUMP + 8 <= machine_info::OFF_LINE_START);
        assert!(machine_info::OFF_LINE_START + 8 <= machine_info::OFF_TEST_LINE_BUF);
        assert!(
            machine_info::OFF_TEST_LINE_BUF + machine_info::TEST_LINE_BUF_SIZE
                <= machine_info::OFF_VECTOR0_OBSERVED
        );
        assert!(machine_info::OFF_VECTOR0_OBSERVED + 8 <= layout::MACHINE_INFO_SIZE);
    }

    #[test]
    fn machine_revision_str_fits_its_fixed_field() {
        assert!(MACHINE_REVISION_STR.len() as u64 <= machine_info::REVISION_FIELD_SIZE);
    }
}
