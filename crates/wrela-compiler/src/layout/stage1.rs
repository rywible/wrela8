use super::{ImageLayout, LayoutError, Section};
use crate::encode;
use wrela_machine::layout as machine_layout;

const PAGE: u64 = wrela_machine::stage1::GRANULE;
const L2_BLOCK: u64 = 2 * 1024 * 1024;
const ENTRIES: usize = 512;

// `build_tables` maps one L3 page for the machine MMIO window. A device
// register beyond that block would be emitted unmapped, so an access that
// must reach stage 2 and exit to the VMM would instead become a guest
// stage-1 fault. This must fail the build, not a Rasputin run.
const _: () = {
    assert!(wrela_machine::mmio::MMIO_END > wrela_machine::mmio::MMIO_BASE);
    assert!(
        wrela_machine::mmio::MMIO_END
            <= (wrela_machine::mmio::MMIO_BASE & !(L2_BLOCK - 1)) + L2_BLOCK
    );
};
const ATTR_DEVICE: u64 = 0;
const ATTR_NORMAL: u64 = 1 << 2;
const AP_READ_ONLY: u64 = 1 << 7;
const INNER_SHAREABLE: u64 = 3 << 8;
const ACCESS_FLAG: u64 = 1 << 10;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionClass {
    Invalid,
    Device,
    ReadExecute,
    ReadOnlyNx,
    ReadWriteNx,
}

impl PermissionClass {
    pub fn name(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Device => "device-rw-nx",
            Self::ReadExecute => "normal-ro-rx",
            Self::ReadOnlyNx => "normal-ro-nx",
            Self::ReadWriteNx => "normal-rw-nx",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectionRange {
    pub base: u64,
    pub size: u64,
    pub class: PermissionClass,
    pub owner: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage1Layout {
    pub table_base: u64,
    pub table_size: u64,
    pub table_sha256: String,
    pub used_pages: u32,
    pub ttbr0_el1: u64,
    pub tcr_el1: u64,
    pub mair_el1: u64,
    pub sctlr_el1: u64,
    pub vbar_el1: u64,
    pub permissions: Vec<ProtectionRange>,
}

pub fn seal(layout: &mut ImageLayout) -> Result<(), LayoutError> {
    reserve_fixed_regions(layout)?;
    write_fault_trampoline(layout)?;
    verify_system_instruction_allowlist(layout)?;
    let (tables, used_pages, permissions) = build_tables(layout)?;
    let table_offset = blob_offset(machine_layout::STAGE1_TABLES_BASE)?;
    let table_end = table_offset
        .checked_add(tables.len())
        .ok_or_else(|| LayoutError::new("stage1 table blob end overflows"))?;
    layout.blob[table_offset..table_end].copy_from_slice(&tables);
    let reserved_end = table_offset + machine_layout::STAGE1_TABLES_SIZE as usize;
    layout.blob[table_end..reserved_end].fill(0);
    let reserved = &layout.blob[table_offset..reserved_end];
    layout.stage1 = Some(Stage1Layout {
        table_base: machine_layout::STAGE1_TABLES_BASE,
        table_size: machine_layout::STAGE1_TABLES_SIZE,
        table_sha256: wrela_machine::sha256::sha256_hex(reserved),
        used_pages,
        ttbr0_el1: machine_layout::STAGE1_TABLES_BASE,
        tcr_el1: wrela_machine::stage1::TCR_EL1,
        mair_el1: wrela_machine::stage1::MAIR_EL1,
        sctlr_el1: wrela_machine::stage1::SCTLR_EL1,
        vbar_el1: machine_layout::STAGE1_FAULT_BASE,
        permissions,
    });
    Ok(())
}

fn verify_system_instruction_allowlist(layout: &ImageLayout) -> Result<(), LayoutError> {
    for section in &layout.sections {
        if !matches!(
            section.name,
            "entry" | "code" | "abort" | "checkpoint" | "rtcode" | "stage1-vector"
        ) {
            continue;
        }
        let start = blob_offset(section.base)?;
        let size = usize::try_from(section.size)
            .map_err(|_| LayoutError::new("executable section size exceeds usize"))?;
        let end = start
            .checked_add(size)
            .ok_or_else(|| LayoutError::new("executable section end overflows usize"))?;
        let bytes = layout
            .blob
            .get(start..end)
            .ok_or_else(|| LayoutError::new("executable section exceeds the sealed image"))?;
        if bytes.len() % 4 != 0 {
            return Err(LayoutError::new(format!(
                "executable section `{}` is not instruction aligned",
                section.name
            )));
        }
        for (index, chunk) in bytes.chunks_exact(4).enumerate() {
            let word = u32::from_le_bytes(chunk.try_into().expect("four-byte instruction"));
            let is_exception_generation = word & 0xff00_0000 == 0xd400_0000;
            let is_system = word & 0xff00_0000 == 0xd500_0000;
            if !is_exception_generation && !is_system {
                continue;
            }
            if !allowed_system_instruction(word) {
                return Err(LayoutError::new(format!(
                    "system-instruction allowlist refuses word {word:#010x} in `{}` at +{:#x}",
                    section.name,
                    index * 4
                )));
            }
        }
    }
    Ok(())
}

fn allowed_system_instruction(word: u32) -> bool {
    const MRS_ALLOWLIST: [u32; 4] = [0xd538_4022, 0xd538_4003, 0xd538_5200, 0xd538_6001];
    word & 0xffe0_001f == 0xd420_0000
        || word == encode::enc_dmb_ishld()
        || word == encode::enc_dmb_ishst()
        || MRS_ALLOWLIST.contains(&word)
}

fn reserve_fixed_regions(layout: &mut ImageLayout) -> Result<(), LayoutError> {
    let had_tables = layout
        .sections
        .iter()
        .any(|item| item.name == "stage1-tables");
    let fixed = [
        Section {
            name: "stage1-vector",
            base: machine_layout::STAGE1_FAULT_BASE,
            size: wrela_machine::stage1::COMMON_PROTECTION_GRANULE,
        },
        Section {
            name: "stage1-fault-record",
            base: machine_layout::STAGE1_FAULT_BASE
                + wrela_machine::stage1::COMMON_PROTECTION_GRANULE,
            size: wrela_machine::stage1::COMMON_PROTECTION_GRANULE,
        },
        Section {
            name: "stage1-tables",
            base: machine_layout::STAGE1_TABLES_BASE,
            size: machine_layout::STAGE1_TABLES_SIZE,
        },
    ];
    for section in fixed {
        if let Some(existing) = layout
            .sections
            .iter()
            .find(|item| item.name == section.name)
        {
            if existing != &section {
                return Err(LayoutError::new(format!(
                    "stage1 fixed section `{}` was already declared differently",
                    section.name
                )));
            }
            continue;
        }
        let prior_end = layout
            .sections
            .last()
            .map_or(machine_layout::IMAGE_BASE, |prior| prior.base + prior.size);
        if prior_end > section.base {
            return Err(LayoutError::new(format!(
                "image section data ends at {prior_end:#x}, overlapping fixed `{}` at {:#x}",
                section.name, section.base
            )));
        }
        layout.sections.push(section);
    }
    let required = usize::try_from(
        machine_layout::STAGE1_TABLES_BASE + machine_layout::STAGE1_TABLES_SIZE
            - machine_layout::IMAGE_BASE,
    )
    .map_err(|_| LayoutError::new("stage1 fixed image length exceeds usize"))?;
    if !had_tables && layout.blob.len() > required {
        return Err(LayoutError::new(
            "image bytes overlap the fixed stage1 table reservation",
        ));
    }
    if layout.blob.len() < required {
        layout.blob.resize(required, 0);
    }
    Ok(())
}

fn write_fault_trampoline(layout: &mut ImageLayout) -> Result<(), LayoutError> {
    let offset = blob_offset(machine_layout::STAGE1_FAULT_BASE)?;
    let vector_len = wrela_machine::stage1::COMMON_PROTECTION_GRANULE as usize;
    let vector = &mut layout.blob[offset..offset + vector_len];
    vector.fill(0);
    const HANDLER: usize = 0x800;
    for slot in (0..0x800).step_by(0x80) {
        put_word(vector, slot, encode::enc_b((HANDLER - slot) as i32))?;
    }
    let words = [
        0xd538_5200, // mrs x0, ESR_EL1
        0xd538_6001, // mrs x1, FAR_EL1
        0xd538_4022, // mrs x2, ELR_EL1
        0xd538_4003, // mrs x3, SPSR_EL1
        encode::enc_movz(
            4,
            ((machine_layout::STAGE1_FAULT_BASE + wrela_machine::stage1::COMMON_PROTECTION_GRANULE)
                >> 0) as u16,
            0,
            true,
        ),
        encode::enc_movk(
            4,
            ((machine_layout::STAGE1_FAULT_BASE + wrela_machine::stage1::COMMON_PROTECTION_GRANULE)
                >> 16) as u16,
            16,
            true,
        ),
        encode::enc_movk(
            4,
            ((machine_layout::STAGE1_FAULT_BASE + wrela_machine::stage1::COMMON_PROTECTION_GRANULE)
                >> 32) as u16,
            32,
            true,
        ),
        encode::enc_movk(
            4,
            ((machine_layout::STAGE1_FAULT_BASE + wrela_machine::stage1::COMMON_PROTECTION_GRANULE)
                >> 48) as u16,
            48,
            true,
        ),
        encode::enc_stp_x(0, 1, 4, 0),
        encode::enc_stp_x(2, 3, 4, 16),
        encode::enc_movz(
            5,
            (wrela_machine::mmio::FAULT_MMIO_ADDR >> 0) as u16,
            0,
            true,
        ),
        encode::enc_movk(
            5,
            (wrela_machine::mmio::FAULT_MMIO_ADDR >> 16) as u16,
            16,
            true,
        ),
        encode::enc_movk(
            5,
            (wrela_machine::mmio::FAULT_MMIO_ADDR >> 32) as u16,
            32,
            true,
        ),
        encode::enc_movk(
            5,
            (wrela_machine::mmio::FAULT_MMIO_ADDR >> 48) as u16,
            48,
            true,
        ),
        encode::enc_str_x_imm(0, 5, 0),
        encode::enc_b(0),
    ];
    for (index, word) in words.into_iter().enumerate() {
        put_word(vector, HANDLER + index * 4, word)?;
    }
    Ok(())
}

fn put_word(bytes: &mut [u8], offset: usize, word: u32) -> Result<(), LayoutError> {
    let target = bytes
        .get_mut(offset..offset + 4)
        .ok_or_else(|| LayoutError::new("stage1 fault trampoline exceeds its vector page"))?;
    target.copy_from_slice(&word.to_le_bytes());
    Ok(())
}

fn build_tables(layout: &ImageLayout) -> Result<(Vec<u8>, u32, Vec<ProtectionRange>), LayoutError> {
    let dram_pages = (machine_layout::DRAM_SIZE / PAGE) as usize;
    let mut pages = vec![PermissionClass::Invalid; dram_pages];
    let mut owners = vec!["unused".to_string(); dram_pages];

    paint(
        &mut pages,
        &mut owners,
        machine_layout::DRAM_BASE,
        machine_layout::IMAGE_BASE - machine_layout::DRAM_BASE,
        PermissionClass::ReadWriteNx,
        "machine-shared",
    )?;
    for core in 0..layout.cores.max(1) {
        paint(
            &mut pages,
            &mut owners,
            machine_layout::core_stack_base_n(core, layout.cores.max(1)),
            machine_layout::CORE_STACK_SIZE,
            PermissionClass::ReadWriteNx,
            &format!("stack-core-{core}"),
        )?;
    }
    for section in &layout.sections {
        let (class, owner) = match section.name {
            "entry" | "code" | "abort" | "checkpoint" | "rtcode" | "stage1-vector" => {
                (PermissionClass::ReadExecute, section.name)
            }
            "rodata" | "frameprog" | "stage1-tables" => (PermissionClass::ReadOnlyNx, section.name),
            _ => (PermissionClass::ReadWriteNx, section.name),
        };
        paint(
            &mut pages,
            &mut owners,
            align_down(
                section.base,
                wrela_machine::stage1::COMMON_PROTECTION_GRANULE,
            ),
            align_up(
                section.base + section.size,
                wrela_machine::stage1::COMMON_PROTECTION_GRANULE,
            )? - align_down(
                section.base,
                wrela_machine::stage1::COMMON_PROTECTION_GRANULE,
            ),
            class,
            owner,
        )?;
    }

    let mut table_pages = vec![[0_u64; ENTRIES]; 3];
    let root = machine_layout::STAGE1_TABLES_BASE;
    let low_l2 = root + PAGE;
    let dram_l2 = root + 2 * PAGE;
    table_pages[0][0] = table_descriptor(low_l2);
    table_pages[0][1] = table_descriptor(dram_l2);

    let mmio_start = wrela_machine::mmio::MMIO_BASE;
    let mmio_end = wrela_machine::mmio::MMIO_END;
    map_mixed_2m(
        &mut table_pages,
        1,
        align_down(mmio_start, L2_BLOCK),
        |gpa| {
            if gpa >= mmio_start && gpa < mmio_end {
                PermissionClass::Device
            } else {
                PermissionClass::Invalid
            }
        },
    )?;

    for block in 0..(machine_layout::DRAM_SIZE / L2_BLOCK) as usize {
        let first_page = block * ENTRIES;
        let slice = &pages[first_page..first_page + ENTRIES];
        let base = machine_layout::DRAM_BASE + block as u64 * L2_BLOCK;
        if slice.iter().all(|class| *class == slice[0]) {
            table_pages[2][block] = leaf_descriptor(base, slice[0], true);
        } else {
            let next = push_l3(&mut table_pages, base, |gpa| {
                let index = ((gpa - machine_layout::DRAM_BASE) / PAGE) as usize;
                pages[index]
            })?;
            table_pages[2][block] = table_descriptor(next);
        }
    }
    let used_pages = u32::try_from(table_pages.len())
        .map_err(|_| LayoutError::new("stage1 table page count exceeds u32"))?;
    if table_pages.len() * PAGE as usize > machine_layout::STAGE1_TABLES_SIZE as usize {
        return Err(LayoutError::new(format!(
            "stage1 needs {} table page(s), exceeding the {}-page reservation",
            table_pages.len(),
            machine_layout::STAGE1_TABLES_SIZE / PAGE
        )));
    }
    let mut bytes = Vec::with_capacity(table_pages.len() * PAGE as usize);
    for page in table_pages {
        for entry in page {
            bytes.extend_from_slice(&entry.to_le_bytes());
        }
    }
    Ok((bytes, used_pages, compress_permissions(&pages, &owners)))
}

fn map_mixed_2m(
    pages: &mut Vec<[u64; ENTRIES]>,
    l2_page: usize,
    block_base: u64,
    class: impl Fn(u64) -> PermissionClass,
) -> Result<(), LayoutError> {
    let index = ((block_base >> 21) & 0x1ff) as usize;
    let next = push_l3(pages, block_base, class)?;
    pages[l2_page][index] = table_descriptor(next);
    Ok(())
}

fn push_l3(
    pages: &mut Vec<[u64; ENTRIES]>,
    base: u64,
    class: impl Fn(u64) -> PermissionClass,
) -> Result<u64, LayoutError> {
    let address = machine_layout::STAGE1_TABLES_BASE
        .checked_add(pages.len() as u64 * PAGE)
        .ok_or_else(|| LayoutError::new("stage1 table address overflows"))?;
    let mut page = [0_u64; ENTRIES];
    for (index, entry) in page.iter_mut().enumerate() {
        let gpa = base + index as u64 * PAGE;
        *entry = leaf_descriptor(gpa, class(gpa), false);
    }
    pages.push(page);
    Ok(address)
}

fn leaf_descriptor(gpa: u64, class: PermissionClass, block: bool) -> u64 {
    if class == PermissionClass::Invalid {
        return 0;
    }
    let kind = if block { 1 } else { 3 };
    let address_mask = if block {
        0x0000_ffff_ffe0_0000
    } else {
        0x0000_ffff_ffff_f000
    };
    let attrs = match class {
        PermissionClass::Invalid => 0,
        PermissionClass::Device => ATTR_DEVICE | ACCESS_FLAG | PXN | UXN,
        PermissionClass::ReadExecute => {
            ATTR_NORMAL | INNER_SHAREABLE | ACCESS_FLAG | UXN | AP_READ_ONLY
        }
        PermissionClass::ReadOnlyNx => {
            ATTR_NORMAL | INNER_SHAREABLE | ACCESS_FLAG | PXN | UXN | AP_READ_ONLY
        }
        PermissionClass::ReadWriteNx => ATTR_NORMAL | INNER_SHAREABLE | ACCESS_FLAG | PXN | UXN,
    };
    (gpa & address_mask) | attrs | kind
}

fn table_descriptor(address: u64) -> u64 {
    (address & 0x0000_ffff_ffff_f000) | 3
}

fn paint(
    pages: &mut [PermissionClass],
    owners: &mut [String],
    base: u64,
    size: u64,
    class: PermissionClass,
    owner: &str,
) -> Result<(), LayoutError> {
    if base % PAGE != 0 || size % PAGE != 0 {
        return Err(LayoutError::new(format!(
            "stage1 protection `{owner}` [{base:#x}, +{size}) is not page aligned"
        )));
    }
    let start = usize::try_from((base - machine_layout::DRAM_BASE) / PAGE)
        .map_err(|_| LayoutError::new("stage1 protection start exceeds usize"))?;
    let count = usize::try_from(size / PAGE)
        .map_err(|_| LayoutError::new("stage1 protection size exceeds usize"))?;
    let end = start
        .checked_add(count)
        .filter(|end| *end <= pages.len())
        .ok_or_else(|| LayoutError::new(format!("stage1 protection `{owner}` exceeds DRAM")))?;
    for index in start..end {
        let old = pages[index];
        if old != PermissionClass::Invalid && old != class {
            return Err(LayoutError::new(format!(
                "stage1 page {:#x} would mix {} owner `{}` and {} owner `{owner}`",
                machine_layout::DRAM_BASE + index as u64 * PAGE,
                old.name(),
                owners[index],
                class.name()
            )));
        }
        pages[index] = class;
        owners[index] = owner.to_string();
    }
    Ok(())
}

fn compress_permissions(pages: &[PermissionClass], owners: &[String]) -> Vec<ProtectionRange> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for index in 1..=pages.len() {
        if index == pages.len() || pages[index] != pages[start] || owners[index] != owners[start] {
            ranges.push(ProtectionRange {
                base: machine_layout::DRAM_BASE + start as u64 * PAGE,
                size: (index - start) as u64 * PAGE,
                class: pages[start],
                owner: owners[start].clone(),
            });
            start = index;
        }
    }
    ranges
}

fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

fn align_up(value: u64, alignment: u64) -> Result<u64, LayoutError> {
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or_else(|| LayoutError::new("stage1 range end overflows while aligning"))
}

fn blob_offset(address: u64) -> Result<usize, LayoutError> {
    let offset = address
        .checked_sub(machine_layout::IMAGE_BASE)
        .ok_or_else(|| LayoutError::new("stage1 blob address is below IMAGE_BASE"))?;
    usize::try_from(offset).map_err(|_| LayoutError::new("stage1 blob offset exceeds usize"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_permissions_are_wx_closed() {
        let rx = leaf_descriptor(0x4060_0000, PermissionClass::ReadExecute, false);
        let rw = leaf_descriptor(0x40a0_0000, PermissionClass::ReadWriteNx, false);
        assert_ne!(rx & AP_READ_ONLY, 0);
        assert_eq!(rx & PXN, 0);
        assert_ne!(rw & PXN, 0);
        assert_eq!(rw & AP_READ_ONLY, 0);
    }

    #[test]
    fn permission_compression_covers_every_page() {
        let pages = [
            PermissionClass::Invalid,
            PermissionClass::Invalid,
            PermissionClass::ReadWriteNx,
        ];
        let owners = ["unused".into(), "unused".into(), "data".into()];
        let ranges = compress_permissions(&pages, &owners);
        assert_eq!(ranges.iter().map(|range| range.size).sum::<u64>(), 3 * PAGE);
        assert_eq!(ranges.len(), 2);
    }

    #[test]
    fn system_instruction_certificate_is_an_exact_closed_allowlist() {
        for word in [
            encode::enc_brk(0),
            encode::enc_brk(u16::MAX),
            encode::enc_dmb_ishld(),
            encode::enc_dmb_ishst(),
            0xd538_4022,
            0xd538_4003,
            0xd538_5200,
            0xd538_6001,
        ] {
            assert!(allowed_system_instruction(word), "refused {word:#010x}");
        }
        for word in [
            0xd538_0000, // mrs x0, MIDR_EL1
            0xd53b_0020, // mrs x0, CTR_EL0
            0xd50b_7420, // dc zva, x0
            0xd503_201f, // nop/hint
            0xd400_0002, // hvc #0
        ] {
            assert!(!allowed_system_instruction(word), "accepted {word:#010x}");
        }
    }
}
