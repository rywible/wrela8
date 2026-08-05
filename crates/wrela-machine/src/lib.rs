pub mod pixels;
pub mod report;
pub mod sha256;
pub mod vmm_process;

pub const MACHINE_REVISION: u32 = 1;

pub const MACHINE_REVISION_STR: &str = "wrela-machine-v1";

pub const CORE_SLOTS: usize = 32;

pub mod layout {
    pub const DRAM_SIZE: u64 = 1 << 30;

    pub const DRAM_BASE: u64 = 0x4000_0000;

    pub const MACHINE_INFO_BASE: u64 = DRAM_BASE;
    pub const MACHINE_INFO_SIZE: u64 = 0x1000;

    pub const STACKS_BASE: u64 = 0x4001_0000;
    pub const CORE_STACK_SIZE: u64 = 1 << 20;

    pub const fn dram_end() -> u64 {
        DRAM_BASE + DRAM_SIZE
    }

    pub const fn core_stack_base_n(core: usize, n_cores: usize) -> u64 {
        debug_assert!(n_cores >= 1);
        debug_assert!(core < n_cores);
        dram_end() - ((n_cores - core) as u64) * CORE_STACK_SIZE
    }

    pub const IMAGE_BASE: u64 = 0x4050_0000;

    pub const RTDATA_BASE: u64 = IMAGE_BASE + 0x4_0000;

    pub const RTDATA_SIZE_MAX: u64 = 256 << 10;

    pub const PIXELS_DATA_BASE_MIN: u64 = RTDATA_BASE;
    pub const PIXELS_PROGRAM_BYTES_MAX: u64 = 64 << 20;
    pub const PIXELS_STATE_BYTES_MAX: u64 = 512 << 20;
    pub const PIXELS_FRAMEBUFFER_BYTES_MAX: u64 = 128 << 20;
    pub const PIXELS_REGION_ALIGNMENT: u64 = 64 << 10;
    pub const PIXELS_STATE_PAGE_ALIGNMENT: u64 = 4096;
}

pub mod machine_info {
    pub const OFF_REVISION: u64 = 0x00;
    pub const REVISION_FIELD_SIZE: u64 = 32;

    pub const OFF_WALL_SEED: u64 = 0x20;

    pub const OFF_NEXT_DEADLINE: u64 = 0x28;

    pub const OFF_EXIT_CODE: u64 = 0x30;

    pub const OFF_TEST_CONTINUATION: u64 = 0x38;

    pub const OFF_TEST_PASSED: u64 = 0x40;
    pub const OFF_TEST_FAILED: u64 = 0x48;

    pub const OFF_RING_DATA_BUMP: u64 = 0x50;
    pub const OFF_RING_DESC_BUMP: u64 = 0x58;

    pub const OFF_LINE_START: u64 = 0x60;

    pub const OFF_CORE_MARK: u64 = 0x68;
    pub const CORE_MARK_STRIDE: u64 = 8;

    pub const fn core_mark_addr(core: usize) -> u64 {
        super::layout::MACHINE_INFO_BASE + OFF_CORE_MARK + (core as u64) * CORE_MARK_STRIDE
    }

    pub const fn core_mark_running(core: usize) -> u64 {
        core as u64 + 1
    }

    pub const OFF_TEST_LINE_BUF: u64 = 0x218;
    pub const TEST_LINE_BUF_SIZE: u64 = 256;

    pub const OFF_ENTROPY_DEST: u64 = 0x318;

    pub const OFF_ENTROPY_LEN: u64 = 0x320;

    pub const ENTROPY_LEN_MAX: u64 = 64;

    pub const OFF_VECTOR0_OBSERVED: u64 = 0x200;

    pub const OFF_ABORT_LATCH: u64 = 0x208;

    pub const OFF_TEST_NEXT: u64 = 0x210;

    pub const OFF_SLOTMAP_NEXT_ID: u64 = 0x328;
}

pub mod console {
    use super::layout::DRAM_BASE;

    pub const QUEUE_SIZE: u64 = 256;

    pub const RING_BASE: u64 = DRAM_BASE + 0x1000;
    pub const RING_SIZE: u64 = 2 * 0x1000;

    pub const DESC_TABLE_OFFSET: u64 = 0;
    pub const DESC_ENTRY_SIZE: u64 = 16;
    pub const DESC_TABLE_SIZE: u64 = QUEUE_SIZE * DESC_ENTRY_SIZE;

    pub const AVAIL_OFFSET: u64 = DESC_TABLE_OFFSET + DESC_TABLE_SIZE;
    pub const AVAIL_SIZE: u64 = 4 + 2 * QUEUE_SIZE;

    pub const USED_OFFSET: u64 = AVAIL_OFFSET + AVAIL_SIZE;
    pub const USED_SIZE: u64 = 4 + 8 * QUEUE_SIZE;

    pub const DOORBELL_OFFSET: u64 = USED_OFFSET + USED_SIZE;
    pub const DOORBELL_SIZE: u64 = 8;

    pub const RING_USED_BYTES: u64 = DOORBELL_OFFSET + DOORBELL_SIZE;

    pub const DATA_BASE: u64 = RING_BASE + RING_SIZE;
    pub const DATA_SIZE: u64 = 4 * 0x1000;
}

pub mod pending {
    use super::layout::DRAM_BASE;

    pub const BASE: u64 = DRAM_BASE + 0x7000;
    pub const SIZE: u64 = 0x1000;

    pub const WORD_SIZE: u64 = 8;

    pub const fn core_word_addr(core: usize) -> u64 {
        BASE + (core as u64) * WORD_SIZE
    }

    pub const IDLE_OFF: u64 = 0x800;

    pub const fn core_idle_addr(core: usize) -> u64 {
        BASE + IDLE_OFF + (core as u64) * WORD_SIZE
    }
}

pub mod mmio {
    pub const MMIO_BASE: u64 = 0x0800_0000;

    pub const CLOCK_MMIO_ADDR: u64 = MMIO_BASE;

    pub const EXIT_MMIO_ADDR: u64 = MMIO_BASE + 0x1000;

    pub const PARK_MMIO_ADDR: u64 = MMIO_BASE + 0x2000;

    pub const RELEASE_MMIO_ADDR: u64 = MMIO_BASE + 0x3000;

    pub const QUIESCE_MMIO_ADDR: u64 = MMIO_BASE + 0x4000;

    pub const ENTROPY_MMIO_ADDR: u64 = MMIO_BASE + 0x5000;
}

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

pub mod display {
    pub use crate::pixels::{BYTES_PER_PIXEL, HEIGHT, REFRESH_HZ, WIDTH};
}

pub mod virtio {
    pub const DESC_SIZE: u64 = 16;

    pub const DOORBELL_BYTES: u64 = 8;

    pub const DESC_F_NEXT: u16 = 1;
    pub const DESC_F_WRITE: u16 = 2;

    pub const F_BLK_FLUSH: u64 = 1 << 9;
    pub const F_VERSION_1: u64 = 1 << 32;

    pub const DEVICE_FEATURES: u64 = F_VERSION_1 | F_BLK_FLUSH;

    pub const REQ_HEADER_SIZE: u64 = 16;

    pub const REQ_STATUS_SIZE: u64 = 1;

    pub const SLOT_BOOK_LAST_USED: u64 = 0;
    pub const SLOT_BOOK_EPOCH: u64 = 8;
    pub const SLOT_BOOK_QUIESCED: u64 = 16;
    pub const SLOT_BOOK_QUARANTINE_STAMP: u64 = 24;
    pub const SLOT_BOOK_BYTES: u64 = 32;

    pub const fn desc_bytes(depth: u16) -> u64 {
        depth as u64 * DESC_SIZE
    }

    pub const fn avail_bytes(depth: u16) -> u64 {
        4 + 2 * depth as u64
    }

    pub const fn used_bytes(depth: u16) -> u64 {
        4 + 8 * depth as u64
    }

    pub const fn quiesce_count_addr(doorbell: u64) -> u64 {
        doorbell + DOORBELL_BYTES + SLOT_BOOK_QUIESCED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regions() -> Vec<(&'static str, u64, u64)> {
        let n = 3usize;
        vec![
            (
                "machine_info",
                layout::MACHINE_INFO_BASE,
                layout::MACHINE_INFO_SIZE,
            ),
            ("console_ring", console::RING_BASE, console::RING_SIZE),
            ("console_data", console::DATA_BASE, console::DATA_SIZE),
            ("pending", pending::BASE, pending::SIZE),
            ("image_base", layout::IMAGE_BASE, 0),
            (
                "core_stack_0",
                layout::core_stack_base_n(0, n),
                layout::CORE_STACK_SIZE,
            ),
            (
                "core_stack_1",
                layout::core_stack_base_n(1, n),
                layout::CORE_STACK_SIZE,
            ),
            (
                "core_stack_2",
                layout::core_stack_base_n(2, n),
                layout::CORE_STACK_SIZE,
            ),
            ("clock_mmio", mmio::CLOCK_MMIO_ADDR, 8),
            ("exit_mmio", mmio::EXIT_MMIO_ADDR, 8),
            ("park_mmio", mmio::PARK_MMIO_ADDR, 8),
            ("release_mmio", mmio::RELEASE_MMIO_ADDR, 8),
            ("quiesce_mmio", mmio::QUIESCE_MMIO_ADDR, 8),
            ("entropy_mmio", mmio::ENTROPY_MMIO_ADDR, 8),
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
        assert!(dram_end - layout::IMAGE_BASE > 512 * (1 << 20));
    }

    #[test]
    fn core_slots_is_the_soft_packing_ceiling() {
        assert_eq!(crate::CORE_SLOTS, 32);
        assert!(
            machine_info::OFF_CORE_MARK + crate::CORE_SLOTS as u64 * machine_info::CORE_MARK_STRIDE
                <= machine_info::OFF_VECTOR0_OBSERVED
        );
    }

    #[test]
    fn core_stack_base_n_is_high_dram_from_the_end() {
        let dram_end = layout::dram_end();
        assert_eq!(dram_end, 0x8000_0000);
        assert_eq!(
            layout::core_stack_base_n(0, 1),
            dram_end - layout::CORE_STACK_SIZE
        );
        assert_eq!(
            layout::core_stack_base_n(0, 2),
            dram_end - 2 * layout::CORE_STACK_SIZE
        );
        assert_eq!(
            layout::core_stack_base_n(1, 2),
            dram_end - layout::CORE_STACK_SIZE
        );
        for n in [1usize, 2, 3, crate::CORE_SLOTS] {
            for c in 0..n {
                let base = layout::core_stack_base_n(c, n);
                assert!(base >= layout::IMAGE_BASE + layout::RTDATA_SIZE_MAX);
                assert_eq!(base + layout::CORE_STACK_SIZE, {
                    if c + 1 < n {
                        layout::core_stack_base_n(c + 1, n)
                    } else {
                        dram_end
                    }
                });
            }
        }
    }

    #[test]
    fn high_stacks_clear_the_image_window() {
        let n = crate::CORE_SLOTS;
        let stacks_lo = layout::core_stack_base_n(0, n);
        let image_hi = layout::RTDATA_BASE + layout::RTDATA_SIZE_MAX;
        assert!(
            image_hi <= stacks_lo,
            "CORE_SLOTS stacks at {stacks_lo:#x} must sit above rtdata window ending {image_hi:#x}"
        );
        assert!(layout::STACKS_BASE < layout::IMAGE_BASE);
        assert!(layout::STACKS_BASE + layout::CORE_STACK_SIZE <= layout::IMAGE_BASE);
    }

    #[test]
    fn rtdata_base_is_the_128kib_packing_window() {
        assert_eq!(layout::RTDATA_BASE, layout::IMAGE_BASE + 0x4_0000);
        assert_eq!(layout::RTDATA_BASE, 0x4054_0000);
        assert!(layout::RTDATA_BASE - layout::IMAGE_BASE > 0xea34);
        assert_eq!(layout::RTDATA_SIZE_MAX, 256 << 10);
        assert!(
            layout::RTDATA_BASE + layout::RTDATA_SIZE_MAX < layout::DRAM_BASE + layout::DRAM_SIZE
        );
    }

    #[test]
    fn console_ring_metadata_fits_the_ring_page() {
        assert!(console::RING_USED_BYTES <= console::RING_SIZE);
    }

    #[test]
    fn pending_words_fit_the_page_and_do_not_overlap() {
        let used = CORE_SLOTS as u64 * pending::WORD_SIZE;
        assert!(used <= pending::SIZE);
        for core in 0..CORE_SLOTS {
            let addr = pending::core_word_addr(core);
            assert!(addr >= pending::BASE && addr + pending::WORD_SIZE <= pending::BASE + used);
        }
        for a in 0..CORE_SLOTS {
            for b in (a + 1)..CORE_SLOTS {
                let (addr_a, addr_b) = (pending::core_word_addr(a), pending::core_word_addr(b));
                assert!(
                    addr_a + pending::WORD_SIZE <= addr_b || addr_b + pending::WORD_SIZE <= addr_a
                );
            }
        }
    }

    #[test]
    fn idle_words_fit_the_page_and_clear_the_pending_words() {
        let pending_used = CORE_SLOTS as u64 * pending::WORD_SIZE;
        assert!(pending_used <= pending::IDLE_OFF);
        let idle_end = pending::IDLE_OFF + CORE_SLOTS as u64 * pending::WORD_SIZE;
        assert!(idle_end <= pending::SIZE);
        for core in 0..CORE_SLOTS {
            let idle = pending::core_idle_addr(core);
            assert_eq!(idle, pending::BASE + pending::IDLE_OFF + core as u64 * 8);
            assert!(idle >= pending::BASE + pending_used);
            assert!(idle + pending::WORD_SIZE <= pending::BASE + pending::SIZE);
            for other in 0..CORE_SLOTS {
                assert_ne!(idle, pending::core_word_addr(other));
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
        assert!(machine_info::OFF_LINE_START + 8 <= machine_info::OFF_CORE_MARK);
        let marks_end =
            machine_info::OFF_CORE_MARK + CORE_SLOTS as u64 * machine_info::CORE_MARK_STRIDE;
        assert!(marks_end <= machine_info::OFF_VECTOR0_OBSERVED);
        assert!(marks_end <= machine_info::OFF_TEST_LINE_BUF);
        assert!(machine_info::OFF_VECTOR0_OBSERVED + 8 <= machine_info::OFF_ABORT_LATCH);
        assert!(machine_info::OFF_ABORT_LATCH + 8 <= machine_info::OFF_TEST_NEXT);
        assert!(machine_info::OFF_TEST_NEXT + 8 <= machine_info::OFF_TEST_LINE_BUF);
        assert!(
            machine_info::OFF_TEST_LINE_BUF + machine_info::TEST_LINE_BUF_SIZE
                <= machine_info::OFF_ENTROPY_DEST
        );
        assert!(machine_info::OFF_ENTROPY_DEST + 8 <= machine_info::OFF_ENTROPY_LEN);
        assert!(machine_info::OFF_ENTROPY_LEN + 8 <= machine_info::OFF_SLOTMAP_NEXT_ID);
        assert!(machine_info::OFF_SLOTMAP_NEXT_ID + 8 <= layout::MACHINE_INFO_SIZE);
        assert!(machine_info::ENTROPY_LEN_MAX == 64);
    }

    #[test]
    fn core_marks_are_distinct_and_never_zero() {
        for core in 0..CORE_SLOTS {
            let addr = machine_info::core_mark_addr(core);
            assert!(addr >= layout::MACHINE_INFO_BASE);
            assert!(addr + 8 <= layout::MACHINE_INFO_BASE + layout::MACHINE_INFO_SIZE);
            assert_ne!(machine_info::core_mark_running(core), 0);
        }
        for a in 0..CORE_SLOTS {
            for b in (a + 1)..CORE_SLOTS {
                assert_ne!(
                    machine_info::core_mark_addr(a),
                    machine_info::core_mark_addr(b)
                );
                assert_ne!(
                    machine_info::core_mark_running(a),
                    machine_info::core_mark_running(b)
                );
            }
        }
    }

    #[test]
    fn machine_revision_str_fits_its_fixed_field() {
        assert!(MACHINE_REVISION_STR.len() as u64 <= machine_info::REVISION_FIELD_SIZE);
    }

    #[test]
    fn virtio_blk_contract_numbers_are_locked() {
        assert_eq!(virtio::DESC_SIZE, 16);
        assert_eq!(virtio::DOORBELL_BYTES, 8);
        assert_eq!(virtio::DESC_F_NEXT, 1);
        assert_eq!(virtio::DESC_F_WRITE, 2);
        assert_eq!(virtio::F_BLK_FLUSH, 1 << 9);
        assert_eq!(virtio::F_VERSION_1, 1 << 32);
        assert_eq!(
            virtio::DEVICE_FEATURES,
            virtio::F_VERSION_1 | virtio::F_BLK_FLUSH
        );
        assert_eq!(virtio::REQ_HEADER_SIZE, 16);
        assert_eq!(virtio::REQ_STATUS_SIZE, 1);
        assert_eq!(virtio::SLOT_BOOK_LAST_USED, 0);
        assert_eq!(virtio::SLOT_BOOK_EPOCH, 8);
        assert_eq!(virtio::SLOT_BOOK_QUIESCED, 16);
        assert_eq!(virtio::SLOT_BOOK_QUARANTINE_STAMP, 24);
        assert_eq!(virtio::SLOT_BOOK_BYTES, 32);
        assert_eq!(virtio::desc_bytes(8), 8 * 16);
        assert_eq!(virtio::avail_bytes(8), 4 + 2 * 8);
        assert_eq!(virtio::used_bytes(8), 4 + 8 * 8);
        assert_eq!(virtio::SLOT_BOOK_QUIESCED, virtio::SLOT_BOOK_EPOCH + 8);
        assert_eq!(
            virtio::SLOT_BOOK_QUARANTINE_STAMP,
            virtio::SLOT_BOOK_QUIESCED + 8
        );
        assert_eq!(
            virtio::SLOT_BOOK_QUARANTINE_STAMP + 8,
            virtio::SLOT_BOOK_BYTES
        );
        assert_eq!(virtio::quiesce_count_addr(0x1000), 0x1000 + 8 + 16);
    }
}
