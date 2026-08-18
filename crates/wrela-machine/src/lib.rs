pub mod input;
pub mod pixels;
pub mod report;
pub mod sha256;
pub mod vmm_process;

pub const MACHINE_REVISION: u32 = 1;

pub const MACHINE_REVISION_STR: &str = "wrela-machine-v1";

pub const CORE_SLOTS: usize = 32;

/// Host-readable diagnostic block-frequency census. It is excluded from
/// sealed product behavior, but its fixed location is shared by the compiler,
/// runtime, and VMM so validation snapshots never depend on host discovery.
pub mod lane2 {
    pub const BASE: u64 = 0x4040_0000;
    pub const ENABLED_OFFSET: u64 = 0;
    pub const HITS_OFFSET: u64 = 8;
    pub const BLOCK_CAPACITY: usize = 16_384;
    pub const CORE_CAPACITY: usize = 4;
    pub const CORE_STRIDE: u64 = (BLOCK_CAPACITY as u64) * 8;
}

pub mod layout {
    /// The fixed guest reservation on the 1 GiB Rasputin product host.
    ///
    /// Half of physical memory remains host-owned so Linux, KVM, the VMM,
    /// display, and measurement tooling never depend on swap or overcommit.
    pub const DRAM_SIZE: u64 = 512 << 20;

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

    /// The image starts on an AArch64 branch-reach region boundary. P7's
    /// generated visibility evaluator is large enough that an unaligned base
    /// could straddle a 2 MiB region even while remaining within its reserved
    /// text window.
    pub const IMAGE_BASE: u64 = 0x4060_0000;

    pub const RTDATA_BASE: u64 = IMAGE_BASE + 0x40_0000;

    pub const RTDATA_SIZE_MAX: u64 = 256 << 10;

    pub const STAGE1_FAULT_BASE: u64 = RTDATA_BASE + RTDATA_SIZE_MAX;
    pub const STAGE1_FAULT_SIZE: u64 = 2 * super::stage1::COMMON_PROTECTION_GRANULE;
    pub const STAGE1_TABLES_BASE: u64 = STAGE1_FAULT_BASE + STAGE1_FAULT_SIZE;
    pub const STAGE1_TABLES_SIZE: u64 = 128 << 10;
    pub const PIXELS_DATA_BASE_MIN: u64 = STAGE1_TABLES_BASE + STAGE1_TABLES_SIZE;
    pub const PIXELS_PROGRAM_BYTES_MAX: u64 = 64 << 20;
    pub const PIXELS_STATE_BYTES_MAX: u64 = 256 << 20;
    pub const PIXELS_FRAMEBUFFER_BYTES_MAX: u64 = 128 << 20;
    pub const PIXELS_REGION_ALIGNMENT: u64 = 64 << 10;
    pub const PIXELS_STATE_PAGE_ALIGNMENT: u64 = 4096;
}

/// Compiler-owned stage-1 translation contract shared unchanged by HVF and
/// KVM. The complete table layout and boot values are emitted in the image
/// report; host adapters only install these values.
pub mod stage1 {
    pub const GRANULE: u64 = 4096;
    pub const COMMON_PROTECTION_GRANULE: u64 = 16 * 1024;
    pub const MAIR_EL1: u64 = 0x0000_0000_0000_ff04;
    pub const TCR_EL1: u64 = 0x0000_0002_0080_3519;
    pub const SCTLR_EL1: u64 = 0x0000_0000_30d8_5c1f;
    pub const WXN_BIT: u64 = 1 << 19;
    pub const SYSTEM_INSTRUCTION_ALLOWLIST_V1: &str = "wrela-system-instructions-v1:brk-imm16,dmb-ishld,dmb-ishst,mrs-elr_el1,mrs-esr_el1,mrs-far_el1,mrs-spsr_el1";

    const L2_BLOCK: u64 = 2 * 1024 * 1024;
    const ENTRIES: usize = 512;

    // The table builders map exactly one L3 page for the machine MMIO window.
    // A device register placed at or beyond that block's end would be left
    // unmapped, turning every legitimate access into a guest stage-1 fault
    // discovered only at run time, on both hosts. Fail the build instead.
    const _: () = {
        assert!(crate::mmio::MMIO_END > crate::mmio::MMIO_BASE);
        assert!(crate::mmio::MMIO_END <= (crate::mmio::MMIO_BASE & !(L2_BLOCK - 1)) + L2_BLOCK);
    };

    const ATTR_NORMAL: u64 = 1 << 2;
    const AP_READ_ONLY: u64 = 1 << 7;
    const INNER_SHAREABLE: u64 = 3 << 8;
    const ACCESS_FLAG: u64 = 1 << 10;
    const PXN: u64 = 1 << 53;
    const UXN: u64 = 1 << 54;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Class {
        Invalid,
        Device,
        ReadExecute,
        ReadOnlyNx,
        ReadWriteNx,
    }

    /// Reconstruct the one canonical table image from the authenticated
    /// protection matrix. Hosts compare this byte-for-byte before entering a
    /// vCPU, so a report cannot authenticate a self-consistent but different
    /// translation graph.
    pub fn canonical_tables(
        protections: &[crate::report::ProtectionRange],
    ) -> Result<(Vec<u8>, u32), String> {
        let page_count = (crate::layout::DRAM_SIZE / GRANULE) as usize;
        let mut classes = vec![Class::Invalid; page_count];
        for range in protections {
            let class = match range.class.as_str() {
                "invalid" => Class::Invalid,
                "normal-ro-rx" => Class::ReadExecute,
                "normal-ro-nx" => Class::ReadOnlyNx,
                "normal-rw-nx" => Class::ReadWriteNx,
                other => return Err(format!("unknown stage1 protection class `{other}`")),
            };
            let start = range
                .base
                .checked_sub(crate::layout::DRAM_BASE)
                .ok_or_else(|| "stage1 protection begins below DRAM".to_string())?;
            if start % GRANULE != 0 || range.size == 0 || range.size % GRANULE != 0 {
                return Err("stage1 protection matrix is not page aligned".to_string());
            }
            let first = usize::try_from(start / GRANULE)
                .map_err(|_| "stage1 protection index exceeds usize".to_string())?;
            let count = usize::try_from(range.size / GRANULE)
                .map_err(|_| "stage1 protection length exceeds usize".to_string())?;
            let end = first
                .checked_add(count)
                .filter(|end| *end <= classes.len())
                .ok_or_else(|| "stage1 protection exceeds DRAM".to_string())?;
            classes[first..end].fill(class);
        }

        let mut pages = vec![[0_u64; ENTRIES]; 3];
        let root = crate::layout::STAGE1_TABLES_BASE;
        pages[0][0] = table_descriptor(root + GRANULE);
        pages[0][1] = table_descriptor(root + 2 * GRANULE);

        let mmio_start = crate::mmio::MMIO_BASE;
        let mmio_end = crate::mmio::MMIO_END;
        let low_base = mmio_start & !(L2_BLOCK - 1);
        let low_index = ((low_base >> 21) & 0x1ff) as usize;
        let low_l3 = push_l3(&mut pages, low_base, |gpa| {
            if (mmio_start..mmio_end).contains(&gpa) {
                Class::Device
            } else {
                Class::Invalid
            }
        })?;
        pages[1][low_index] = table_descriptor(low_l3);

        for block in 0..(crate::layout::DRAM_SIZE / L2_BLOCK) as usize {
            let first = block * ENTRIES;
            let slice = &classes[first..first + ENTRIES];
            let base = crate::layout::DRAM_BASE + block as u64 * L2_BLOCK;
            if slice.iter().all(|class| *class == slice[0]) {
                pages[2][block] = leaf_descriptor(base, slice[0], true);
            } else {
                let l3 = push_l3(&mut pages, base, |gpa| {
                    classes[((gpa - crate::layout::DRAM_BASE) / GRANULE) as usize]
                })?;
                pages[2][block] = table_descriptor(l3);
            }
        }
        let used_pages = u32::try_from(pages.len())
            .map_err(|_| "stage1 table page count exceeds u32".to_string())?;
        let reservation = usize::try_from(crate::layout::STAGE1_TABLES_SIZE)
            .map_err(|_| "stage1 table reservation exceeds usize".to_string())?;
        if pages.len() * GRANULE as usize > reservation {
            return Err("stage1 tables exceed the fixed reservation".to_string());
        }
        let mut bytes = Vec::with_capacity(reservation);
        for page in pages {
            for entry in page {
                bytes.extend_from_slice(&entry.to_le_bytes());
            }
        }
        bytes.resize(reservation, 0);
        Ok((bytes, used_pages))
    }

    fn push_l3(
        pages: &mut Vec<[u64; ENTRIES]>,
        base: u64,
        class: impl Fn(u64) -> Class,
    ) -> Result<u64, String> {
        let address = crate::layout::STAGE1_TABLES_BASE
            .checked_add(pages.len() as u64 * GRANULE)
            .ok_or_else(|| "stage1 table address overflows".to_string())?;
        let mut page = [0_u64; ENTRIES];
        for (index, entry) in page.iter_mut().enumerate() {
            let gpa = base + index as u64 * GRANULE;
            *entry = leaf_descriptor(gpa, class(gpa), false);
        }
        pages.push(page);
        Ok(address)
    }

    fn leaf_descriptor(gpa: u64, class: Class, block: bool) -> u64 {
        if class == Class::Invalid {
            return 0;
        }
        let address_mask = if block {
            0x0000_ffff_ffe0_0000
        } else {
            0x0000_ffff_ffff_f000
        };
        let attrs = match class {
            Class::Invalid => 0,
            Class::Device => ACCESS_FLAG | PXN | UXN,
            Class::ReadExecute => ATTR_NORMAL | INNER_SHAREABLE | ACCESS_FLAG | UXN | AP_READ_ONLY,
            Class::ReadOnlyNx => {
                ATTR_NORMAL | INNER_SHAREABLE | ACCESS_FLAG | PXN | UXN | AP_READ_ONLY
            }
            Class::ReadWriteNx => ATTR_NORMAL | INNER_SHAREABLE | ACCESS_FLAG | PXN | UXN,
        };
        (gpa & address_mask) | attrs | if block { 1 } else { 3 }
    }

    fn table_descriptor(address: u64) -> u64 {
        (address & 0x0000_ffff_ffff_f000) | 3
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn entry(bytes: &[u8], page: usize, index: usize) -> u64 {
            let offset = page * GRANULE as usize + index * size_of::<u64>();
            u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
        }

        #[test]
        fn canonical_table_graph_maps_the_exact_closed_mmio_span_as_device_xn() {
            let protections = [crate::report::ProtectionRange {
                base: crate::layout::DRAM_BASE,
                size: crate::layout::DRAM_SIZE,
                class: "invalid".into(),
                owner: "test".into(),
            }];
            let (bytes, used) = canonical_tables(&protections).unwrap();
            assert_eq!(used, 4);
            assert_eq!(bytes.len(), crate::layout::STAGE1_TABLES_SIZE as usize);
            assert_eq!(
                entry(&bytes, 0, 0),
                table_descriptor(crate::layout::STAGE1_TABLES_BASE + GRANULE)
            );
            assert_eq!(
                entry(&bytes, 0, 1),
                table_descriptor(crate::layout::STAGE1_TABLES_BASE + 2 * GRANULE)
            );
            assert_eq!(entry(&bytes, 2, 0), 0, "invalid DRAM stays unmapped");

            let mmio_l3 = 3;
            let input_index =
                ((crate::mmio::INPUT_STATUS_MMIO_ADDR - crate::mmio::MMIO_BASE) / GRANULE) as usize;
            for index in [0, input_index] {
                let descriptor = entry(&bytes, mmio_l3, index);
                assert_eq!(descriptor & 3, 3);
                assert_ne!(descriptor & PXN, 0);
                assert_ne!(descriptor & UXN, 0);
                assert_eq!(descriptor & ATTR_NORMAL, 0);
            }
            let first_outside =
                ((crate::mmio::MMIO_END - crate::mmio::MMIO_BASE) / GRANULE) as usize;
            assert_eq!(entry(&bytes, mmio_l3, first_outside), 0);
            assert!(
                bytes[used as usize * GRANULE as usize..]
                    .iter()
                    .all(|byte| *byte == 0)
            );
        }
    }
}

/// Semantic guest-fault classification shared by both host backends.
///
/// The two hosts observe the same guest exception through different paths: HVF
/// traps `BRK` to the VMM directly, while KVM lets it reach the guest's own
/// stage-1 vector, so it arrives as a fault-doorbell MMIO exit. Backend
/// conformance compares an *exact semantic class*, so the class must be derived
/// from the guest-visible `ESR_EL1` rather than from which host reported it.
pub mod fault {
    /// Exception classes, ESR_EL1 bits [31:26].
    const EC_INSTRUCTION_ABORT_LOWER_EL: u64 = 0x20;
    const EC_INSTRUCTION_ABORT_SAME_EL: u64 = 0x21;
    const EC_PC_ALIGNMENT: u64 = 0x22;
    const EC_DATA_ABORT_LOWER_EL: u64 = 0x24;
    const EC_DATA_ABORT_SAME_EL: u64 = 0x25;
    const EC_SP_ALIGNMENT: u64 = 0x26;
    const EC_BRK: u64 = 0x3c;

    pub const GUEST_BRK: &str = "guest-brk";
    pub const STAGE1_PERMISSION_FAULT: &str = "stage1-permission-fault";
    pub const MEMORY_ABORT: &str = "memory-abort";
    pub const ALIGNMENT_FAULT: &str = "alignment-fault";
    pub const SYNC_EXCEPTION: &str = "sync-exception";

    /// Name the guest-visible class of a synchronous exception.
    ///
    /// A permission fault is the W^X protection outcome and must stay
    /// distinguishable from an ordinary translation or access-flag abort:
    /// reporting every abort as a permission fault would make an unmapped-page
    /// bug look like a successfully enforced protection.
    pub const fn class(esr: u64) -> &'static str {
        match (esr >> 26) & 0x3f {
            EC_BRK => GUEST_BRK,
            EC_PC_ALIGNMENT | EC_SP_ALIGNMENT => ALIGNMENT_FAULT,
            EC_INSTRUCTION_ABORT_LOWER_EL
            | EC_INSTRUCTION_ABORT_SAME_EL
            | EC_DATA_ABORT_LOWER_EL
            | EC_DATA_ABORT_SAME_EL => {
                // IFSC/DFSC is ISS[5:0]; `0b0011LL` is a permission fault at
                // translation level LL.
                if esr & 0x3c == 0x0c {
                    STAGE1_PERMISSION_FAULT
                } else {
                    MEMORY_ABORT
                }
            }
            _ => SYNC_EXCEPTION,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn esr(ec: u64, fsc: u64) -> u64 {
            (ec << 26) | (1 << 25) | fsc
        }

        #[test]
        fn a_brk_is_the_same_class_however_the_host_observed_it() {
            // HVF traps this to the VMM; KVM delivers it to the guest vector
            // and it returns through the fault doorbell. One class either way.
            assert_eq!(class(esr(EC_BRK, 0)), GUEST_BRK);
            assert_eq!(class(esr(EC_BRK, 0x42)), GUEST_BRK);
        }

        #[test]
        fn permission_faults_stay_distinct_from_other_aborts() {
            for ec in [
                EC_DATA_ABORT_LOWER_EL,
                EC_DATA_ABORT_SAME_EL,
                EC_INSTRUCTION_ABORT_LOWER_EL,
                EC_INSTRUCTION_ABORT_SAME_EL,
            ] {
                for level in 0..4 {
                    assert_eq!(class(esr(ec, 0x0c | level)), STAGE1_PERMISSION_FAULT);
                }
                // Translation and access-flag faults are not protection wins.
                for fsc in [0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x10] {
                    assert_eq!(class(esr(ec, fsc)), MEMORY_ABORT, "ec={ec:#x} fsc={fsc:#x}");
                }
            }
        }

        #[test]
        fn alignment_and_unknown_classes_are_named_not_guessed() {
            assert_eq!(class(esr(EC_PC_ALIGNMENT, 0)), ALIGNMENT_FAULT);
            assert_eq!(class(esr(EC_SP_ALIGNMENT, 0)), ALIGNMENT_FAULT);
            assert_eq!(class(esr(0x00, 0)), SYNC_EXCEPTION);
            assert_eq!(class(esr(0x18, 0)), SYNC_EXCEPTION);
        }
    }
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

    // Sized for instrumented conformance runs: a full debug-framebuffer dump
    // of a 64x32 frame is ~1100 transcript lines (~22 KiB), far past the old
    // 256-descriptor/16 KiB console. The console stays an append-only bump
    // channel; only its capacity grew.
    pub const QUEUE_SIZE: u64 = 4096;

    pub const RING_BASE: u64 = DRAM_BASE + 0x1000;
    pub const RING_SIZE: u64 = 0x1B000;

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
    pub const DATA_SIZE: u64 = 0x20000;
}

pub mod pending {
    use super::layout::DRAM_BASE;

    pub const BASE: u64 = DRAM_BASE + 0x3C000;
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

    /// Fatal stage-1 permission-fault doorbell used only by the fixed vector
    /// trampoline. It is not an application-visible device register.
    pub const FAULT_MMIO_ADDR: u64 = MMIO_BASE + 0x1_0000;
    pub const INPUT_STATUS_MMIO_ADDR: u64 = MMIO_BASE + 0x1_1000;
    pub const INPUT_EVENT_MMIO_ADDR: u64 = INPUT_STATUS_MMIO_ADDR + 8;
    pub const MMIO_END: u64 = INPUT_STATUS_MMIO_ADDR + super::stage1::GRANULE;
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
            ("input_mmio", mmio::INPUT_STATUS_MMIO_ADDR, 16),
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
        let first_stack = layout::core_stack_base_n(0, crate::CORE_SLOTS);
        assert!(layout::IMAGE_BASE < dram_end);
        assert!(
            first_stack - layout::IMAGE_BASE
                > layout::PIXELS_PROGRAM_BYTES_MAX
                    + layout::PIXELS_STATE_BYTES_MAX
                    + layout::PIXELS_FRAMEBUFFER_BYTES_MAX
        );
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
        assert_eq!(dram_end, 0x6000_0000);
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
    fn rtdata_base_is_the_4mib_packing_window() {
        assert_eq!(layout::RTDATA_BASE, layout::IMAGE_BASE + 0x40_0000);
        assert_eq!(layout::RTDATA_BASE, 0x40a0_0000);
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
