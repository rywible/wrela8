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
/// 0x4000_1000  console::RING_BASE  (4 KiB)   console ring metadata + doorbell
/// 0x4000_2000  console::DATA_BASE  (16 KiB)  console tx byte buffers
/// 0x4000_6000  .. 0x4001_0000               reserved (device-page growth)
/// 0x4001_0000  STACKS_BASE         (4 MiB)   4 per-core stacks, 1 MiB each
/// 0x4041_0000  .. 0x4050_0000               reserved (stack growth room)
/// 0x4050_0000  IMAGE_BASE          (rest)    sealed image, loaded flat
/// ```
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
/// producer with no interrupt suppression to negotiate. `QUEUE_SIZE` is
/// deliberately tiny (16) — plenty for report-line traffic, and small
/// enough that the whole ring plus doorbell fits in one page with room to
/// spare.
pub mod console {
    use super::layout::DRAM_BASE;

    /// Queue depth. Small on purpose (module doc above).
    pub const QUEUE_SIZE: u64 = 16;

    /// Ring metadata page: descriptor table + avail ring + used ring +
    /// doorbell word, in that order.
    pub const RING_BASE: u64 = DRAM_BASE + 0x1000;
    pub const RING_SIZE: u64 = 0x1000;

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
    fn machine_info_fields_fit_the_page_and_do_not_overlap() {
        assert!(
            machine_info::OFF_REVISION + machine_info::REVISION_FIELD_SIZE
                <= machine_info::OFF_WALL_SEED
        );
        assert!(machine_info::OFF_WALL_SEED + 8 <= machine_info::OFF_NEXT_DEADLINE);
        assert!(machine_info::OFF_NEXT_DEADLINE + 8 <= machine_info::OFF_EXIT_CODE);
        assert!(machine_info::OFF_EXIT_CODE + 8 <= layout::MACHINE_INFO_SIZE);
    }

    #[test]
    fn machine_revision_str_fits_its_fixed_field() {
        assert!(MACHINE_REVISION_STR.len() as u64 <= machine_info::REVISION_FIELD_SIZE);
    }
}
