use std::time::Duration;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod hv;

pub const WALL_CAP: Duration = Duration::from_secs(30);
/// Hang detector for images carrying a Pixels renderer.
///
/// This is a liveness bound, not a performance bound: a from-scratch certified
/// sweep is legitimately minutes before P12 adds performance admission, and the
/// watchdog is a single total-wall deadline with no progress signal to consult
/// (the guest makes no MMIO exit during the sweep, so an idle-timeout would
/// need a guest-side heartbeat and would perturb the sealed transcripts).
///
/// The bound therefore has to clear the slowest legitimate scene by a wide
/// margin on a *loaded* machine. `check-pixels-hard-csg` measured ~5-9 minutes
/// idle and blew straight through the previous 600s bound once other work was
/// competing for the host, failing with "no core reported the guest exit
/// protocol" — a false hang report, not a real one, and one that would hit any
/// slower runner even unloaded. `check-pixels-tile-boundary` is heavier still
/// (twice the pixels and the highest per-pixel fallback count in the corpus).
/// 1800s keeps roughly a 3x margin over the worst observed case while still
/// bounding a genuinely wedged guest. Bring it back down when P12 lands
/// performance admission and the sweeps get cheap.
pub const PIXELS_WALL_CAP: Duration = Duration::from_secs(1800);

/// Guest FPCR policy: make every floating-point NaN result the architectural
/// default NaN. Pixels' packet ABI seals that bit pattern, so both host
/// backends must install the policy before executing any guest instruction.
pub(crate) const GUEST_FPCR: u64 = 1 << 25;

pub(crate) const fn boot_wall_cap(has_pixels_renderer: bool) -> Duration {
    if has_pixels_renderer {
        PIXELS_WALL_CAP
    } else {
        WALL_CAP
    }
}

pub(crate) fn capped_park_deadline_ns(now_ns: u64, deadline_ns: u64) -> u64 {
    let wall_cap_ns = WALL_CAP.as_nanos() as u64;
    deadline_ns.min(now_ns.saturating_add(wall_cap_ns))
}

#[derive(Debug)]
pub enum VmmError {
    Unsupported(&'static str),
    MachineRevisionMismatch {
        report: String,
        vmm: &'static str,
    },
    MalformedReport(String),
    Io(String),
    Hvf {
        call: &'static str,
        code: i32,
    },
    BadImage(String),
    GuestFault(String),
    ReplayDivergence(String),
    Timeout {
        core: usize,
        transcript_so_far: Vec<u8>,
    },
    HostCoresRefuse {
        requested: usize,
        failed_at: usize,
        code: i32,
    },
}

impl std::fmt::Display for VmmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmmError::Unsupported(what) => write!(f, "unsupported: {what}"),
            VmmError::MachineRevisionMismatch { report, vmm } => write!(
                f,
                "machine revision mismatch: image built for `{report}`, this VMM is `{vmm}`"
            ),
            VmmError::MalformedReport(msg) => write!(f, "malformed report: {msg}"),
            VmmError::Io(msg) => write!(f, "{msg}"),
            VmmError::Hvf { call, code } => {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                {
                    write!(f, "{call} failed: {}", hv::describe_hv_return(*code))
                }
                #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                {
                    write!(f, "{call} failed: code {code}")
                }
            }
            VmmError::BadImage(msg) => write!(f, "bad image: {msg}"),
            VmmError::GuestFault(msg) => write!(f, "guest fault: {msg}"),
            VmmError::ReplayDivergence(msg) => write!(f, "replay divergence: {msg}"),
            VmmError::Timeout {
                core,
                transcript_so_far,
            } => write!(
                f,
                "timeout after {:?} on core {core}: {} byte(s) of transcript captured before the \
                 forced exit",
                WALL_CAP,
                transcript_so_far.len()
            ),
            VmmError::HostCoresRefuse {
                requested,
                failed_at,
                code,
            } => {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                {
                    write!(
                        f,
                        "host refused Cores count={requested}: hv_vcpu_create failed for core \
                         {failed_at}: {}",
                        hv::describe_hv_return(*code)
                    )
                }
                #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                {
                    write!(
                        f,
                        "host refused Cores count={requested}: hv_vcpu_create failed for core \
                         {failed_at}: code {code}"
                    )
                }
            }
        }
    }
}

impl std::error::Error for VmmError {}

#[derive(Debug, Clone, Default)]
pub struct BootOutcome {
    pub transcript: Vec<u8>,
    pub exit_code: u64,
    pub choices: Vec<record::ChoiceEntry>,
    pub exits: u64,
    pub core_marks: Vec<u64>,
    pub lane2_hits: Vec<(u32, u64)>,
    pub frames: Vec<wrela_machine::pixels::PresentedFrame>,
    /// Digest recomputed from the bytes staged by the selected host backend,
    /// one per successfully presented frame.
    pub frame_buffer_digests: Vec<[u8; 32]>,
}

pub use wrela_machine::report::{
    BlkConfig, BlkQueueConfig, CoreEntry, CoreStack, EMPTY_SHA256, IrqHostInject, ParsedReport,
    PoolWindow, ReportSection, RequestRing,
};

pub(crate) fn parse_report(text: &str) -> Result<ParsedReport, VmmError> {
    match wrela_machine::report::parse_report(text) {
        Ok(parsed) => Ok(parsed),
        Err(msg) => {
            if let Some(report) = msg.strip_prefix("machine-revision-mismatch:") {
                Err(VmmError::MachineRevisionMismatch {
                    report: report.to_string(),
                    vmm: wrela_machine::MACHINE_REVISION_STR,
                })
            } else {
                Err(VmmError::MalformedReport(msg))
            }
        }
    }
}

pub(crate) fn validate_report_digests(parsed: &ParsedReport, img: &[u8]) -> Result<(), VmmError> {
    let got = wrela_machine::sha256::sha256_hex(img);
    if got != parsed.image_sha256 {
        return Err(VmmError::BadImage(format!(
            "image sha256 mismatch: report declares {}, blob hashes to {got}",
            parsed.image_sha256
        )));
    }
    for (path, expected) in &parsed.input_digests {
        let p = std::path::Path::new(path);
        if !p.is_file() {
            continue;
        }
        let bytes = std::fs::read(p)
            .map_err(|e| VmmError::Io(format!("read input `{path}` for digest check: {e}")))?;
        let got = wrela_machine::sha256::sha256_hex(&bytes);
        if got != *expected {
            return Err(VmmError::BadImage(format!(
                "input `{path}` sha256 mismatch: report declares {expected}, file hashes to {got}"
            )));
        }
    }
    if parsed.frameprog_sections.is_empty() {
        let alignment = wrela_machine::pixels::FRAME_PROGRAM_HOT_ALIGNMENT_V1 as usize;
        if img
            .chunks_exact(alignment)
            .any(|chunk| chunk.starts_with(&wrela_machine::pixels::FRAME_PROGRAM_MAGIC_V1))
        {
            return Err(VmmError::BadImage(
                "image contains an aligned FrameProgram but the report has no canonical \
                 frameprog section metadata"
                    .to_string(),
            ));
        }
    }
    if let Some(section) = parsed.frameprog_sections.first() {
        if section.base % wrela_machine::layout::PIXELS_REGION_ALIGNMENT != 0 {
            return Err(VmmError::BadImage(format!(
                "frameprog section base {:#x} is not {}-byte aligned",
                section.base,
                wrela_machine::layout::PIXELS_REGION_ALIGNMENT
            )));
        }
        let start = section
            .base
            .checked_sub(wrela_machine::layout::IMAGE_BASE)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| {
                VmmError::BadImage("frameprog section begins below the image base".to_string())
            })?;
        let size = usize::try_from(section.size)
            .map_err(|_| VmmError::BadImage("frameprog section exceeds usize".to_string()))?;
        let end = start
            .checked_add(size)
            .ok_or_else(|| VmmError::BadImage("frameprog section end overflows".to_string()))?;
        let bytes = img.get(start..end).ok_or_else(|| {
            VmmError::BadImage("frameprog section is outside the immutable image blob".to_string())
        })?;
        let programs = validate_frameprog_section(bytes).map_err(VmmError::BadImage)?;
        if programs.len() != parsed.renderer_placements.len() {
            return Err(VmmError::BadImage(format!(
                "frameprog section contains {} program(s), but the report declares {} renderer \
                 placement(s)",
                programs.len(),
                parsed.renderer_placements.len()
            )));
        }
        for (placement, (offset, size)) in parsed.renderer_placements.iter().zip(programs) {
            let base =
                section
                    .base
                    .checked_add(u64::try_from(offset).map_err(|_| {
                        VmmError::BadImage("frameprog offset exceeds u64".to_string())
                    })?)
                    .ok_or_else(|| {
                        VmmError::BadImage("frameprog program base overflows".to_string())
                    })?;
            if placement.frameprog_base != base
                || placement.frameprog_size
                    != u64::try_from(size).map_err(|_| {
                        VmmError::BadImage("frameprog program size exceeds u64".to_string())
                    })?
            {
                return Err(VmmError::BadImage(format!(
                    "renderer {} placement does not match its verified frame program range",
                    placement.index
                )));
            }
        }
    }
    Ok(())
}

fn validate_frameprog_section(bytes: &[u8]) -> Result<Vec<(usize, usize)>, String> {
    let mut cursor = 0_usize;
    let mut programs = 0_usize;
    let mut ranges = Vec::new();
    while cursor < bytes.len() {
        if cursor % wrela_machine::pixels::FRAME_PROGRAM_HOT_ALIGNMENT_V1 as usize != 0 {
            return Err(format!(
                "frameprog program {programs} begins at misaligned section offset {cursor}"
            ));
        }
        if bytes.len() - cursor < wrela_machine::pixels::FRAME_PROGRAM_HEADER_BYTES_V1 as usize {
            return Err("frameprog section ends in a truncated program header".to_string());
        }
        let total = u32::from_le_bytes(
            bytes[cursor + 16..cursor + 20]
                .try_into()
                .expect("header length checked"),
        ) as usize;
        let end = cursor
            .checked_add(total)
            .ok_or_else(|| "frameprog program end overflows".to_string())?;
        let program = bytes
            .get(cursor..end)
            .ok_or_else(|| "frameprog program extends past its section".to_string())?;
        validate_frame_program_v1(program)?;
        let renderer_index = u16::from_le_bytes(
            program[20..22]
                .try_into()
                .expect("FrameProgram header was validated"),
        );
        if usize::from(renderer_index) != programs {
            return Err(format!(
                "frameprog program {programs} declares noncanonical renderer index {renderer_index}"
            ));
        }
        ranges.push((cursor, total));
        programs += 1;
        cursor = end;
        if cursor < bytes.len() {
            let alignment = wrela_machine::pixels::FRAME_PROGRAM_HOT_ALIGNMENT_V1 as usize;
            let next = cursor
                .checked_add((alignment - cursor % alignment) % alignment)
                .ok_or_else(|| "frameprog inter-program alignment overflows".to_string())?;
            if next > bytes.len() || bytes[cursor..next].iter().any(|byte| *byte != 0) {
                return Err("frameprog inter-program padding is nonzero or truncated".to_string());
            }
            cursor = next;
        }
    }
    if programs == 0 {
        return Err("frameprog section contains no programs".to_string());
    }
    Ok(ranges)
}

fn validate_frame_program_v1(bytes: &[u8]) -> Result<(), String> {
    use wrela_machine::pixels as format;

    let header = format::FRAME_PROGRAM_HEADER_BYTES_V1 as usize;
    if bytes.len() > format::FRAME_PROGRAM_MAX_BYTES_V1 as usize {
        return Err("FrameProgram exceeds the machine byte ceiling".to_string());
    }
    if bytes.len() < header {
        return Err("FrameProgram header is truncated".to_string());
    }
    if bytes[0..8] != format::FRAME_PROGRAM_MAGIC_V1 {
        return Err("FrameProgram magic mismatch".to_string());
    }
    let u16_at =
        |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().expect("header checked"));
    let u32_at =
        |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("header checked"));
    if u16_at(8) != format::FRAME_PROGRAM_VERSION_V1
        || u16_at(10) != format::FRAME_PROGRAM_HEADER_BYTES_V1
    {
        return Err("FrameProgram version/header size mismatch".to_string());
    }
    if u16_at(22) != 0 || bytes[34..48].iter().any(|byte| *byte != 0) {
        return Err("FrameProgram reserved header bytes are nonzero".to_string());
    }
    if u32_at(12) & !1 != 0 {
        return Err("FrameProgram flags contain unsupported bits".to_string());
    }
    if u32_at(24) != format::FRAME_PROGRAM_NUMERIC_REVISION_V1
        || u32_at(28) != format::FRAME_PROGRAM_FORMAL_REVISION_V1
    {
        return Err("FrameProgram numeric/formal revision is unsupported".to_string());
    }
    if u32_at(16) as usize != bytes.len() {
        return Err("FrameProgram total_bytes disagrees with its section".to_string());
    }
    if u16_at(32) != format::FrameProgramTableKindV1::REQUIRED_COUNT {
        return Err("FrameProgram directory count mismatch".to_string());
    }
    let directory_end = header
        .checked_add(
            format::FrameProgramTableKindV1::ALL
                .len()
                .checked_mul(format::FRAME_PROGRAM_TABLE_BYTES_V1 as usize)
                .ok_or_else(|| "FrameProgram directory size overflows".to_string())?,
        )
        .ok_or_else(|| "FrameProgram directory end overflows".to_string())?;
    if directory_end > bytes.len() {
        return Err("FrameProgram directory is truncated".to_string());
    }
    let mut ranges = Vec::<std::ops::Range<usize>>::new();
    let mut entries = Vec::<(format::FrameProgramTableKindV1, u32, usize, usize)>::new();
    let mut canonical_end = directory_end;
    for (index, kind) in format::FrameProgramTableKindV1::ALL.into_iter().enumerate() {
        let at = header + index * format::FRAME_PROGRAM_TABLE_BYTES_V1 as usize;
        if u16_at(at) != kind.code() || u16_at(at + 2) != kind.record_bytes() {
            return Err(format!(
                "FrameProgram directory entry {index} is noncanonical"
            ));
        }
        let count = u32_at(at + 4);
        let offset = u32_at(at + 8);
        let byte_len = u32_at(at + 12);
        if count == 0 {
            if offset != 0 || byte_len != 0 {
                return Err(format!(
                    "FrameProgram empty {} table is noncanonical",
                    kind.stable_name()
                ));
            }
            entries.push((kind, 0, 0, usize::from(kind.record_bytes())));
            continue;
        }
        if matches!(
            kind,
            format::FrameProgramTableKindV1::Texture
                | format::FrameProgramTableKindV1::ShadingSummary
                | format::FrameProgramTableKindV1::Transparency
                | format::FrameProgramTableKindV1::Probe
                | format::FrameProgramTableKindV1::Kinetic
                | format::FrameProgramTableKindV1::DebugName
        ) {
            return Err(format!(
                "FrameProgram P5 {} table is unexpectedly populated",
                kind.stable_name()
            ));
        }
        let expected = count
            .checked_mul(u32::from(kind.record_bytes()))
            .ok_or_else(|| "FrameProgram table length overflows".to_string())?;
        if byte_len != expected || u64::from(offset) % format::FRAME_PROGRAM_HOT_ALIGNMENT_V1 != 0 {
            return Err(format!(
                "FrameProgram {} table length/alignment is invalid",
                kind.stable_name()
            ));
        }
        let start = offset as usize;
        let end = start
            .checked_add(byte_len as usize)
            .ok_or_else(|| "FrameProgram table end overflows".to_string())?;
        if start < directory_end || end > bytes.len() {
            return Err(format!(
                "FrameProgram {} table is out of bounds",
                kind.stable_name()
            ));
        }
        if ranges
            .iter()
            .any(|range| start < range.end && range.start < end)
        {
            return Err("FrameProgram tables overlap".to_string());
        }
        let alignment = usize::try_from(format::FRAME_PROGRAM_HOT_ALIGNMENT_V1)
            .map_err(|_| "FrameProgram alignment exceeds usize".to_string())?;
        let canonical_start = canonical_end
            .checked_add(alignment - 1)
            .map(|value| value & !(alignment - 1))
            .ok_or_else(|| "FrameProgram canonical table offset overflows".to_string())?;
        if start != canonical_start {
            return Err(format!(
                "FrameProgram {} table is out of canonical physical order",
                kind.stable_name()
            ));
        }
        ranges.push(start..end);
        entries.push((kind, count, start, usize::from(kind.record_bytes())));
        canonical_end = end;
    }
    ranges.sort_by_key(|range| range.start);
    let mut padding_cursor = directory_end;
    for range in &ranges {
        if bytes[padding_cursor..range.start]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err("FrameProgram alignment padding is nonzero".to_string());
        }
        padding_cursor = range.end;
    }
    if padding_cursor != bytes.len() {
        return Err("FrameProgram has trailing bytes".to_string());
    }
    let mut digest_input = bytes.to_vec();
    let found = digest_input[format::FRAME_PROGRAM_DIGEST_OFFSET_V1
        ..format::FRAME_PROGRAM_DIGEST_OFFSET_V1 + format::FRAME_PROGRAM_DIGEST_BYTES_V1]
        .to_vec();
    digest_input[format::FRAME_PROGRAM_DIGEST_OFFSET_V1
        ..format::FRAME_PROGRAM_DIGEST_OFFSET_V1 + format::FRAME_PROGRAM_DIGEST_BYTES_V1]
        .fill(0);
    if found != wrela_machine::sha256::sha256(&digest_input) {
        return Err("FrameProgram digest mismatch".to_string());
    }

    let (_, immediate_count, immediate_start, immediate_record_bytes) = entries
        .iter()
        .copied()
        .find(|(kind, _, _, _)| *kind == format::FrameProgramTableKindV1::Immediate)
        .expect("canonical namespace includes immediates");
    for index in 0..immediate_count as usize {
        let at = immediate_start + index * immediate_record_bytes;
        if u16_at(at + 2) != 0 || u32_at(at + 12) != 0 {
            return Err(format!(
                "FrameProgram immediate record {index} has nonzero reserved fields"
            ));
        }
    }
    let mut used_immediates = vec![false; immediate_count as usize];
    let mut semantic_tables = Vec::with_capacity(entries.len());
    for (kind, count, start, record_bytes) in entries {
        if kind == format::FrameProgramTableKindV1::Immediate {
            semantic_tables.push(format::FrameTableModelV1 {
                kind,
                records: Vec::new(),
            });
            continue;
        }
        let mut semantic_records = Vec::with_capacity(count as usize);
        for index in 0..count as usize {
            let at = start + index * record_bytes;
            if u32_at(at) != index as u32 {
                return Err(format!(
                    "FrameProgram {} record {index} has a noncanonical stable ID",
                    kind.stable_name()
                ));
            }
            let tag = u16_at(at + 4);
            let flags = u16_at(at + 6);
            if u16_at(at + 14) != 0 {
                return Err(format!(
                    "FrameProgram {} record {index} has nonzero reserved bits",
                    kind.stable_name()
                ));
            }
            let operand_start = u32_at(at + 8);
            let operand_count = u16_at(at + 12);
            let operand_end = operand_start
                .checked_add(u32::from(operand_count))
                .ok_or_else(|| "FrameProgram operand range overflows".to_string())?;
            if operand_end > immediate_count {
                return Err(format!(
                    "FrameProgram {} record {index} has out-of-bounds operands",
                    kind.stable_name()
                ));
            }
            for ordinal in 0..u32::from(operand_count) {
                let immediate_index = (operand_start + ordinal) as usize;
                if std::mem::replace(&mut used_immediates[immediate_index], true) {
                    return Err(format!(
                        "FrameProgram immediate {immediate_index} is referenced more than once"
                    ));
                }
                let immediate_at =
                    immediate_start + (operand_start + ordinal) as usize * immediate_record_bytes;
                if u16_at(immediate_at) != kind.code()
                    || u32_at(immediate_at + 4) != index as u32
                    || u32_at(immediate_at + 8) != ordinal
                {
                    return Err(format!(
                        "FrameProgram {} record {index} has noncanonical immediate ownership",
                        kind.stable_name()
                    ));
                }
            }
            let operands = (0..u32::from(operand_count))
                .map(|ordinal| {
                    let immediate_at = immediate_start
                        + (operand_start + ordinal) as usize * immediate_record_bytes;
                    u64::from_le_bytes(
                        bytes[immediate_at + 16..immediate_at + 24]
                            .try_into()
                            .expect("immediate bounds checked"),
                    )
                })
                .collect();
            semantic_records.push(format::FrameRecordV1 {
                stable_id: index as u32,
                tag,
                flags,
                operands,
            });
        }
        semantic_tables.push(format::FrameTableModelV1 {
            kind,
            records: semantic_records,
        });
    }
    if let Some(index) = used_immediates.iter().position(|used| !used) {
        return Err(format!(
            "FrameProgram immediate {index} is not referenced by a record"
        ));
    }
    let model = format::FrameProgramModelV1 {
        renderer_index: u16_at(20),
        flags: u32_at(12),
        numeric_revision: u32_at(24),
        formal_revision: u32_at(28),
        tables: semantic_tables,
    };
    format::verify_frame_program_model_v1(&model)
        .map_err(|error| format!("FrameProgram semantic verification failed: {error}"))?;
    Ok(())
}

pub(crate) fn guest_dram_offset(guest: u64, nbytes: u64, what: &str) -> Result<usize, VmmError> {
    use wrela_machine::layout as machine_layout;
    let end = guest.checked_add(nbytes).ok_or_else(|| {
        VmmError::BadImage(format!(
            "{what} address {guest:#x}+{nbytes} overflows a u64"
        ))
    })?;
    let dram_end = machine_layout::DRAM_BASE + machine_layout::DRAM_SIZE;
    if guest < machine_layout::DRAM_BASE || end > dram_end {
        return Err(VmmError::BadImage(format!(
            "{what} address {guest:#x}+{nbytes} is outside guest DRAM \
             [{:#x}..{dram_end:#x})",
            machine_layout::DRAM_BASE
        )));
    }
    Ok((guest - machine_layout::DRAM_BASE) as usize)
}

mod boot;
pub mod display;
mod exit_loop;
pub mod lane3;
pub mod replay;

pub(crate) use boot::boot_image_core;
#[cfg(test)]
pub(crate) use boot::host_cores_refuse;
pub use boot::{boot_image, boot_image_with_display};
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) use boot::{
    boot_image_core_with_delayed_raise, core_sp_tops_from_report,
    create_inject as vcpu_create_inject,
};

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) use exit_loop::check_core_marks;
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) use exit_loop::drain_console;
#[cfg(test)]
pub(crate) use exit_loop::{AdmissionWitness, check_vector_in_range};

#[cfg(target_os = "linux")]
pub mod kvm {}

pub mod devices;

pub mod record;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn park_deadline_is_clamped_to_wall_cap() {
        let now = 1_000_000_000u64;
        let wall_ns = WALL_CAP.as_nanos() as u64;
        assert_eq!(
            capped_park_deadline_ns(now, now + 1_000),
            now + 1_000,
            "short deadlines are honoured"
        );
        assert_eq!(
            capped_park_deadline_ns(now, u64::MAX),
            now.saturating_add(wall_ns),
            "u64::MAX must not sleep past WALL_CAP"
        );
        assert_eq!(
            capped_park_deadline_ns(now, now.saturating_add(wall_ns).saturating_add(1)),
            now.saturating_add(wall_ns),
            "anything past WALL_CAP is capped"
        );
    }

    #[test]
    fn pixels_boot_watchdog_is_not_a_pre_p12_performance_gate() {
        assert_eq!(boot_wall_cap(false), WALL_CAP);
        assert_eq!(boot_wall_cap(true), PIXELS_WALL_CAP);
        assert!(PIXELS_WALL_CAP > WALL_CAP);
        // The slowest conformance scene measured ~9-10 minutes on a loaded
        // host, and a watchdog that a legitimate workload can reach reports a
        // hang that did not happen. Keep a multiple of that worst case rather
        // than trimming to whatever the current corpus happens to need.
        assert!(
            PIXELS_WALL_CAP >= Duration::from_secs(1800),
            "the Pixels hang detector must keep a wide margin over the slowest \
             legitimate certified sweep until P12 gates performance"
        );
    }

    fn report_identity(input_path: &str, img: &[u8]) -> String {
        format!(
            "Machine revision={}\nInput path={input_path} sha256={}\nImage sha256={}\n",
            wrela_machine::MACHINE_REVISION_STR,
            EMPTY_SHA256,
            wrela_machine::sha256::sha256_hex(img),
        )
    }

    #[test]
    fn parse_report_accepts_a_well_formed_report() {
        let text = format!(
            "Machine revision={}\nInput path=input.wr sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nSection name=entry base=0x40500000 size=64\nEntry base=0x40500000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        let parsed = parse_report(&text).unwrap();
        assert_eq!(parsed.entry, 0x40500000);
    }

    #[test]
    fn parse_report_rejects_a_wrong_revision() {
        let text = "Machine revision=some-other-machine-v9\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nSection name=entry base=0x40500000 size=1\nEntry base=0x40500000\n";
        match parse_report(text) {
            Err(VmmError::MachineRevisionMismatch { report, .. }) => {
                assert_eq!(report, "some-other-machine-v9");
            }
            other => panic!("expected a machine-revision mismatch, got {other:?}"),
        }
    }

    #[test]
    fn parse_report_rejects_a_missing_revision_line() {
        let text = "Input path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nSection name=entry base=0x40500000 size=1\nEntry base=0x40500000\n";
        assert!(matches!(
            parse_report(text),
            Err(VmmError::MalformedReport(_))
        ));
    }

    #[test]
    fn parse_report_rejects_a_missing_input_line() {
        let text = format!(
            "Machine revision={}\nImage sha256={EMPTY_SHA256}\nSection name=entry base=0x40500000 size=1\nEntry base=0x40500000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        assert!(matches!(
            parse_report(&text),
            Err(VmmError::MalformedReport(_))
        ));
    }

    #[test]
    fn parse_report_rejects_a_missing_image_digest_line() {
        let text = format!(
            "Machine revision={}\nInput path=x sha256={EMPTY_SHA256}\nSection name=entry base=0x40500000 size=1\nEntry base=0x40500000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        match parse_report(&text) {
            Err(VmmError::MalformedReport(msg)) => {
                assert!(msg.contains("Image sha256"), "got {msg}");
            }
            other => panic!("expected MalformedReport, got {other:?}"),
        }
    }

    #[test]
    fn validate_report_digests_rejects_a_tampered_blob() {
        let img = b"sealed-image-bytes";
        let text = format!(
            "{}Section name=entry base=0x40500000 size={}\nEntry base=0x40500000\n",
            report_identity("x.wr", img),
            img.len(),
        );
        let parsed = parse_report(&text).expect("parses");
        assert!(validate_report_digests(&parsed, img).is_ok());
        let err = validate_report_digests(&parsed, b"tampered").expect_err("mismatch");
        assert!(
            matches!(err, VmmError::BadImage(ref msg) if msg.contains("image sha256 mismatch")),
            "got {err:?}"
        );
    }

    fn frameprog_report(
        section_name: &str,
        section_base: u64,
        placement_base: u64,
    ) -> (String, Vec<u8>) {
        let program = wrela_compiler::pixels::fuzz_seed_frame_program().unwrap();
        let offset = usize::try_from(section_base - wrela_machine::layout::IMAGE_BASE).unwrap();
        let mut image = vec![0_u8; offset];
        image.extend_from_slice(&program);
        let report = format!(
            "{}Section name=entry base={:#x} size=1\n\
             Section name={section_name} base={section_base:#x} size={}\n\
             RendererPlacement index=0 frameprog_base={placement_base:#x} frameprog_bytes={} state_base=0x0 state_bytes=0\n\
             Entry base={:#x}\n",
            report_identity("renderer.wr", &image),
            wrela_machine::layout::IMAGE_BASE,
            program.len(),
            program.len(),
            wrela_machine::layout::IMAGE_BASE,
        );
        (report, image)
    }

    #[test]
    fn report_frameprog_metadata_is_mandatory_and_absolutely_aligned() {
        let base =
            wrela_machine::layout::IMAGE_BASE + wrela_machine::layout::PIXELS_REGION_ALIGNMENT;
        let (report, image) = frameprog_report("frameprog", base, base);
        let parsed = parse_report(&report).expect("canonical renderer report");
        validate_report_digests(&parsed, &image).expect("canonical frame program");

        let renamed = report
            .replace("name=frameprog", "name=renderer-data")
            .replace(
                &report
                    .lines()
                    .find(|line| line.trim_start().starts_with("RendererPlacement "))
                    .unwrap()
                    .to_string(),
                "",
            );
        let parsed = parse_report(&renamed).expect("otherwise valid renamed report");
        let error = validate_report_digests(&parsed, &image).expect_err("hidden frame program");
        assert!(
            matches!(error, VmmError::BadImage(ref message) if message.contains("no canonical frameprog")),
            "got {error:?}"
        );

        let misaligned = base + wrela_machine::pixels::FRAME_PROGRAM_HOT_ALIGNMENT_V1;
        let (report, image) = frameprog_report("frameprog", misaligned, misaligned);
        let parsed = parse_report(&report).expect("structurally valid report");
        let error = validate_report_digests(&parsed, &image).expect_err("misaligned base");
        assert!(
            matches!(error, VmmError::BadImage(ref message) if message.contains("not 65536-byte aligned")),
            "got {error:?}"
        );
    }

    fn rehash_frame_program(bytes: &mut [u8]) {
        use wrela_machine::pixels as format;
        bytes[format::FRAME_PROGRAM_DIGEST_OFFSET_V1
            ..format::FRAME_PROGRAM_DIGEST_OFFSET_V1 + format::FRAME_PROGRAM_DIGEST_BYTES_V1]
            .fill(0);
        let digest = wrela_machine::sha256::sha256(bytes);
        bytes[format::FRAME_PROGRAM_DIGEST_OFFSET_V1
            ..format::FRAME_PROGRAM_DIGEST_OFFSET_V1 + format::FRAME_PROGRAM_DIGEST_BYTES_V1]
            .copy_from_slice(&digest);
    }

    #[test]
    fn independent_frame_program_verifier_accepts_canonical_bytes_and_rejects_records() {
        use wrela_machine::pixels as format;
        let bytes = wrela_compiler::pixels::fuzz_seed_frame_program().unwrap();
        assert!(validate_frame_program_v1(&bytes).is_ok());

        let scalar_entry = format::FRAME_PROGRAM_HEADER_BYTES_V1 as usize
            + usize::from(format::FrameProgramTableKindV1::Scalar.code() - 1)
                * format::FRAME_PROGRAM_TABLE_BYTES_V1 as usize;
        let scalar_offset = u32::from_le_bytes(
            bytes[scalar_entry + 8..scalar_entry + 12]
                .try_into()
                .unwrap(),
        ) as usize;
        for offset in [scalar_offset + 4, scalar_offset + 6, scalar_offset + 14] {
            let mut corrupt = bytes.clone();
            corrupt[offset..offset + 2].copy_from_slice(&u16::MAX.to_le_bytes());
            rehash_frame_program(&mut corrupt);
            assert!(
                validate_frame_program_v1(&corrupt).is_err(),
                "accepted record mutation at {offset}"
            );
        }
        let immediate_entry = format::FRAME_PROGRAM_HEADER_BYTES_V1 as usize
            + usize::from(format::FrameProgramTableKindV1::Immediate.code() - 1)
                * format::FRAME_PROGRAM_TABLE_BYTES_V1 as usize;
        let immediate_offset = u32::from_le_bytes(
            bytes[immediate_entry + 8..immediate_entry + 12]
                .try_into()
                .unwrap(),
        ) as usize;
        let operand_offset = u32::from_le_bytes(
            bytes[scalar_offset + 8..scalar_offset + 12]
                .try_into()
                .unwrap(),
        ) as usize;
        for poisoned in [u64::from(f32::NAN.to_bits()), u64::MAX] {
            let mut corrupt = bytes.clone();
            let value = immediate_offset
                + (operand_offset + 1) * format::FRAME_PROGRAM_IMMEDIATE_BYTES_V1 as usize
                + 16;
            corrupt[value..value + 8].copy_from_slice(&poisoned.to_le_bytes());
            rehash_frame_program(&mut corrupt);
            assert!(
                validate_frame_program_v1(&corrupt).is_err(),
                "accepted poisoned f32 word {poisoned:#x}"
            );
        }
        let mut corrupt_digest = bytes;
        corrupt_digest[format::FRAME_PROGRAM_DIGEST_OFFSET_V1] ^= 1;
        assert!(validate_frame_program_v1(&corrupt_digest).is_err());

        let mut trailing = wrela_compiler::pixels::fuzz_seed_frame_program().unwrap();
        trailing.extend([0; 64]);
        let total = u32::try_from(trailing.len()).unwrap();
        trailing[16..20].copy_from_slice(&total.to_le_bytes());
        rehash_frame_program(&mut trailing);
        assert!(validate_frame_program_v1(&trailing).is_err());

        let mut semantic = wrela_compiler::pixels::fuzz_seed_frame_program().unwrap();
        let entry = |kind: format::FrameProgramTableKindV1| {
            format::FRAME_PROGRAM_HEADER_BYTES_V1 as usize
                + usize::from(kind.code() - 1) * format::FRAME_PROGRAM_TABLE_BYTES_V1 as usize
        };
        let table_offset = |bytes: &[u8], kind| {
            let at = entry(kind);
            u32::from_le_bytes(bytes[at + 8..at + 12].try_into().unwrap()) as usize
        };
        let object_offset = table_offset(&semantic, format::FrameProgramTableKindV1::Object);
        let immediate_offset = table_offset(&semantic, format::FrameProgramTableKindV1::Immediate);
        let operand_offset = u32::from_le_bytes(
            semantic[object_offset + 8..object_offset + 12]
                .try_into()
                .unwrap(),
        ) as usize;
        let mut cross_record = semantic.clone();
        let source_root = immediate_offset
            + (operand_offset + 1) * format::FRAME_PROGRAM_IMMEDIATE_BYTES_V1 as usize
            + 16;
        cross_record[source_root..source_root + 8]
            .copy_from_slice(&u64::from(u32::MAX).to_le_bytes());
        rehash_frame_program(&mut cross_record);
        assert!(
            validate_frame_program_v1(&cross_record).is_err(),
            "accepted a rehashed out-of-range object-to-field reference"
        );

        let primitive_count = immediate_offset
            + (operand_offset + 4) * format::FRAME_PROGRAM_IMMEDIATE_BYTES_V1 as usize
            + 16;
        semantic[primitive_count..primitive_count + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        rehash_frame_program(&mut semantic);
        assert!(
            validate_frame_program_v1(&semantic).is_err(),
            "accepted a rehashed semantically malformed object record"
        );
    }

    #[test]
    fn independent_frame_program_section_verifier_checks_indexes_and_padding() {
        use wrela_machine::pixels as format;
        let first = wrela_compiler::pixels::fuzz_seed_frame_program().unwrap();
        let mut second = first.clone();
        second[20..22].copy_from_slice(&1_u16.to_le_bytes());
        rehash_frame_program(&mut second);
        let mut section = first;
        section.resize(
            section
                .len()
                .next_multiple_of(format::FRAME_PROGRAM_HOT_ALIGNMENT_V1 as usize),
            0,
        );
        let second_start = section.len();
        section.extend_from_slice(&second);
        assert!(validate_frameprog_section(&section).is_ok());

        let mut wrong_index = section.clone();
        wrong_index[second_start + 20..second_start + 22].copy_from_slice(&2_u16.to_le_bytes());
        rehash_frame_program(&mut wrong_index[second_start..]);
        assert!(validate_frameprog_section(&wrong_index).is_err());

        if second_start
            > wrong_index[..second_start]
                .iter()
                .rposition(|byte| *byte != 0)
                .unwrap()
                + 1
        {
            let mut nonzero_padding = section;
            nonzero_padding[second_start - 1] = 1;
            assert!(validate_frameprog_section(&nonzero_padding).is_err());
        }
    }

    #[test]
    fn parse_report_rejects_a_missing_section_line() {
        let text = format!(
            "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nEntry base=0x40500000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        assert!(matches!(
            parse_report(&text),
            Err(VmmError::MalformedReport(_))
        ));
    }

    #[test]
    fn parse_report_rejects_a_missing_entry_line() {
        let text = format!(
            "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nSection name=entry base=0x40500000 size=1\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        assert!(matches!(
            parse_report(&text),
            Err(VmmError::MalformedReport(_))
        ));
    }

    #[test]
    fn parse_report_rejects_section_outside_guest_dram() {
        let text = format!(
            "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nSection name=entry base=0x1000 size=64\nEntry base=0x1000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        match parse_report(&text) {
            Err(VmmError::MalformedReport(msg)) => {
                assert!(
                    msg.contains("outside guest DRAM"),
                    "expected DRAM bound diagnostic, got {msg}"
                );
            }
            other => panic!("expected MalformedReport, got {other:?}"),
        }
    }

    #[test]
    fn guest_dram_offset_rejects_oob_and_accepts_in_range() {
        assert!(guest_dram_offset(wrela_machine::layout::DRAM_BASE, 4, "t").is_ok());
        assert!(guest_dram_offset(wrela_machine::layout::DRAM_BASE - 1, 4, "t").is_err());
        assert!(
            guest_dram_offset(
                wrela_machine::layout::DRAM_BASE + wrela_machine::layout::DRAM_SIZE - 3,
                4,
                "t"
            )
            .is_err()
        );
        assert!(guest_dram_offset(0xffff_ffff_ffff_fff0, 32, "t").is_err());
    }

    #[test]
    fn check_vector_in_range_rejects_aliasing_high_vector() {
        let err = check_vector_in_range(64).unwrap_err();
        assert!(
            matches!(&err, VmmError::BadImage(msg) if msg.contains("out of range")),
            "got {err:?}"
        );
        assert!(check_vector_in_range(63).is_ok());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn drain_console_reads_more_than_the_old_16_descriptor_limit() {
        use wrela_machine::{console, layout as machine_layout};

        let buf_len =
            (console::DATA_BASE + console::DATA_SIZE - machine_layout::DRAM_BASE) as usize + 64;
        let mut buf = vec![0u8; buf_len];

        let ring_off = (console::RING_BASE - machine_layout::DRAM_BASE) as usize;
        let data_off = (console::DATA_BASE - machine_layout::DRAM_BASE) as usize;

        let n: usize = 20;
        assert!(n > 16, "this test's whole point is exceeding the old bound");
        for i in 0..n {
            let desc_off = ring_off
                + console::DESC_TABLE_OFFSET as usize
                + i * console::DESC_ENTRY_SIZE as usize;
            let addr = console::DATA_BASE + i as u64;
            buf[desc_off..desc_off + 8].copy_from_slice(&addr.to_le_bytes());
            buf[desc_off + 8..desc_off + 12].copy_from_slice(&1u32.to_le_bytes());
            buf[data_off + i] = b'A' + (i as u8);
        }
        let avail_idx_off = ring_off + console::AVAIL_OFFSET as usize + 2;
        buf[avail_idx_off..avail_idx_off + 2].copy_from_slice(&(n as u16).to_le_bytes());

        let got = drain_console(buf.as_ptr());
        let expect: Vec<u8> = (0..n).map(|i| b'A' + (i as u8)).collect();
        assert_eq!(got, expect);
        assert_eq!(got.len(), 20);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn record_replay_roundtrips_and_detects_every_divergence_shape() {
        use wrela_compiler::encode;

        fn load_imm_words(reg: u8, value: u64) -> Vec<u32> {
            let h0 = (value & 0xFFFF) as u16;
            let h1 = ((value >> 16) & 0xFFFF) as u16;
            let h2 = ((value >> 32) & 0xFFFF) as u16;
            let h3 = ((value >> 48) & 0xFFFF) as u16;
            vec![
                encode::enc_movz(reg, h0, 0, true),
                encode::enc_movk(reg, h1, 16, true),
                encode::enc_movk(reg, h2, 32, true),
                encode::enc_movk(reg, h3, 48, true),
            ]
        }

        let mut words = Vec::new();
        words.extend(load_imm_words(9, wrela_machine::mmio::CLOCK_MMIO_ADDR));
        words.push(encode::enc_ldr_x_imm(0, 9, 0));
        words.push(encode::enc_ldr_x_imm(1, 9, 0));
        words.push(encode::enc_movz(2, 0, 0, true));
        words.extend(load_imm_words(10, wrela_machine::mmio::EXIT_MMIO_ADDR));
        words.push(encode::enc_str_x_imm(2, 10, 0));

        let img_bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let report_text = format!(
            "{}Section name=entry base={:#x} size={}\nEntry base={:#x}\n",
            report_identity("clock-test.wr", &img_bytes),
            wrela_machine::layout::IMAGE_BASE,
            img_bytes.len(),
            wrela_machine::layout::IMAGE_BASE,
        );

        let tmp_dir = std::env::temp_dir().join(format!(
            "wrela-vmm-record-replay-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let img_path = tmp_dir.join("clock.img");
        let report_path = tmp_dir.join("clock.report.txt");
        std::fs::write(&img_path, &img_bytes).expect("write image");
        std::fs::write(&report_path, &report_text).expect("write report");

        let recorded = record::record(&report_path, &img_path).expect("live boot");
        assert_eq!(
            recorded.choices.len(),
            2,
            "the guest reads the clock exactly twice"
        );
        assert!(
            recorded
                .choices
                .iter()
                .all(|c| matches!(c, record::ChoiceEntry::ClockRead { .. })),
            "no park/deadline/vector activity in this hand-built clock-only guest"
        );
        assert_eq!(recorded.exit_code, 0);

        let divergences = record::replay(&report_path, &img_path, &recorded).expect("replay boot");
        assert!(
            divergences.is_empty(),
            "expected no divergence, got {divergences:?}"
        );

        let mut bad_digest = recorded.clone();
        bad_digest.transcript_digest = "not-the-real-digest".to_string();
        let divergences =
            record::replay(&report_path, &img_path, &bad_digest).expect("replay boot");
        assert!(matches!(
            divergences.as_slice(),
            [record::Divergence::TranscriptDigestMismatch { .. }]
        ));

        let mut bad_exit = recorded.clone();
        bad_exit.exit_code = 99;
        let divergences = record::replay(&report_path, &img_path, &bad_exit).expect("replay boot");
        assert!(divergences.contains(&record::Divergence::ExitCodeMismatch {
            expected: 99,
            actual: 0,
        }));

        let mut short_log = recorded.clone();
        short_log.choices.truncate(1);
        let err = record::replay(&report_path, &img_path, &short_log).expect_err("strict underrun");
        assert!(err.to_string().contains("replay divergence"), "got {err}");

        let mut long_log = recorded.clone();
        long_log
            .choices
            .push(record::ChoiceEntry::ClockRead { value: 424242 });
        let err = record::replay(&report_path, &img_path, &long_log).expect_err("strict overrun");
        assert!(err.to_string().contains("replay divergence"), "got {err}");

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn boot_hand_built_image(img_bytes: &[u8], tag: &str) -> BootOutcome {
        let report_text = format!(
            "{}Section name=entry base={:#x} size={}\nEntry base={:#x}\n",
            report_identity(&format!("{tag}.wr"), img_bytes),
            wrela_machine::layout::IMAGE_BASE,
            img_bytes.len(),
            wrela_machine::layout::IMAGE_BASE,
        );
        let tmp_dir =
            std::env::temp_dir().join(format!("wrela-vmm-{tag}-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let img_path = tmp_dir.join(format!("{tag}.img"));
        let report_path = tmp_dir.join(format!("{tag}.report.txt"));
        std::fs::write(&img_path, &img_bytes).expect("write image");
        std::fs::write(&report_path, &report_text).expect("write report");
        let outcome = boot_image(&report_path, &img_path).expect("live boot");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        outcome
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn write_hand_built_image(
        img_bytes: &[u8],
        tag: &str,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let report_text = format!(
            "{}Section name=entry base={:#x} size={}\nEntry base={:#x}\n",
            report_identity(&format!("{tag}.wr"), img_bytes),
            wrela_machine::layout::IMAGE_BASE,
            img_bytes.len(),
            wrela_machine::layout::IMAGE_BASE,
        );
        let tmp_dir =
            std::env::temp_dir().join(format!("wrela-vmm-{tag}-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let img_path = tmp_dir.join(format!("{tag}.img"));
        let report_path = tmp_dir.join(format!("{tag}.report.txt"));
        std::fs::write(&img_path, img_bytes).expect("write image");
        std::fs::write(&report_path, &report_text).expect("write report");
        (report_path, img_path)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn load_imm_words(reg: u8, value: u64) -> Vec<u32> {
        use wrela_compiler::encode;
        let h0 = (value & 0xFFFF) as u16;
        let h1 = ((value >> 16) & 0xFFFF) as u16;
        let h2 = ((value >> 32) & 0xFFFF) as u16;
        let h3 = ((value >> 48) & 0xFFFF) as u16;
        vec![
            encode::enc_movz(reg, h0, 0, true),
            encode::enc_movk(reg, h1, 16, true),
            encode::enc_movk(reg, h2, 32, true),
            encode::enc_movk(reg, h3, 48, true),
        ]
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn hand_built_simple_checkpoint() -> (Vec<u32>, usize) {
        use wrela_compiler::encode;
        use wrela_machine::{layout as machine_layout, machine_info, pending};
        let mut w = Vec::new();
        let observed = machine_layout::MACHINE_INFO_BASE + machine_info::OFF_VECTOR0_OBSERVED;
        let pending_addr = pending::core_word_addr(0);
        w.extend(load_imm_words(9, observed));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        w.push(encode::enc_add_imm(10, 10, 1, true));
        w.push(encode::enc_str_x_imm(10, 9, 0));
        w.push(encode::enc_ret(30));
        let service = w.len();
        w.push(encode::enc_sub_imm(31, 31, 16, true));
        w.push(encode::enc_str_x_imm(30, 31, 0));
        let loop_top = w.len();
        w.extend(load_imm_words(9, pending_addr));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        let cbz_at = w.len();
        w.push(0);
        let bl_at = w.len();
        w.push(encode::enc_bl(((0isize - bl_at as isize) * 4) as i32));
        w.extend(load_imm_words(9, pending_addr));
        w.push(encode::enc_str_x_imm(31, 9, 0));
        let b_at = w.len();
        w.push(encode::enc_b(((loop_top as i64 - b_at as i64) * 4) as i32));
        let done = w.len();
        w[cbz_at] = encode::enc_cbz(10, ((done as i64 - cbz_at as i64) * 4) as i32, true);
        w.push(encode::enc_ldr_x_imm(30, 31, 0));
        w.push(encode::enc_add_imm(31, 31, 16, true));
        w.push(encode::enc_ret(30));
        (w, service)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn check_eq_into(acc: u8, scratch: u8, x_reg: u8, expect: u16, shift: u8) -> Vec<u32> {
        use wrela_compiler::encode;
        let mut w = vec![
            encode::enc_cmp_imm(x_reg, expect, true),
            encode::enc_cset(scratch, encode::Cond::Ne, true),
        ];
        if shift > 0 {
            w.push(encode::enc_lsl_imm(scratch, scratch, shift, true));
        }
        w.push(encode::enc_orr_reg(acc, acc, scratch, true));
        w
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn push_halt(w: &mut Vec<u32>, exit_reg_scratch: u8, addr_scratch: u8, exit_code: u64) {
        use wrela_compiler::encode;
        w.extend(load_imm_words(exit_reg_scratch, exit_code));
        w.extend(load_imm_words(
            addr_scratch,
            wrela_machine::mmio::EXIT_MMIO_ADDR,
        ));
        w.push(encode::enc_str_x_imm(exit_reg_scratch, addr_scratch, 0));
        w.push(encode::enc_brk(0));
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn compile_test_image(src: &str) -> (wrela_compiler::layout::ImageLayout, String) {
        use std::collections::{BTreeMap, BTreeSet};
        use wrela_compiler::sema::typed::TestKind;
        use wrela_compiler::sema::types::{Type, TypeArg};
        use wrela_compiler::{layout, loader};

        let tokens = wrela_compiler::syntax::lexer::lex(src).expect("conformance source must lex");
        let module = wrela_compiler::syntax::parser::parse(tokens).expect("must parse");
        let (runtime_key, runtime_loaded) = match loader::load_runtime_module() {
            Ok(v) => v,
            Err(_) => panic!("stdlib/core/runtime.wr must load"),
        };
        let root_key = module.path.clone();
        let gen_key: Vec<String> = loader::IMAGE_RUNTIME_MODULE_KEY
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let gen_module =
            wrela_compiler::rtconfig::parse_generated(&wrela_compiler::rtconfig::stub_text())
                .expect("rtconfig stub must parse");
        let mut modules_vec = BTreeMap::new();
        modules_vec.insert(root_key.clone(), module.clone());
        modules_vec.insert(runtime_key.clone(), runtime_loaded.module);
        modules_vec.insert(gen_key.clone(), gen_module);
        let mut paths = BTreeMap::new();
        paths.insert(root_key.clone(), "<conformance>".to_string());
        paths.insert(
            runtime_key.clone(),
            runtime_loaded.file.display().to_string(),
        );
        paths.insert(
            gen_key.clone(),
            wrela_compiler::rtconfig::GENERATED_INPUT_PATH.to_string(),
        );
        let time_key: Option<Vec<String>> = if loader::module_mentions_time(&module) {
            let (time_key, time_loaded) = match loader::load_time_module() {
                Ok(v) => v,
                Err(_) => panic!("stdlib/core/time.wr must load"),
            };
            paths.insert(time_key.clone(), time_loaded.file.display().to_string());
            modules_vec.insert(time_key.clone(), time_loaded.module);
            Some(time_key)
        } else {
            None
        };
        let internal_sources = BTreeSet::from([gen_key]);
        let mut programs_vec = wrela_compiler::sema::check_program_typed_with_internal_sources(
            &modules_vec,
            &paths,
            &internal_sources,
        )
        .expect("must check");
        if let Some(tk) = &time_key {
            programs_vec.remove(tk);
            modules_vec.remove(tk);
        }
        let programs: BTreeMap<String, wrela_compiler::sema::typed::TypedProgram> = programs_vec
            .into_iter()
            .map(|(k, p)| (k.join("."), p))
            .collect();
        let modules: BTreeMap<String, wrela_compiler::syntax::ast::Module> = modules_vec
            .into_iter()
            .map(|(k, m)| (k.join("."), m))
            .collect();
        let program = programs
            .get(&root_key.join("."))
            .expect("root program present");
        let runtime_tests: Vec<String> = program
            .tests
            .iter()
            .filter(|t| t.kind == TestKind::Runtime)
            .map(|t| t.name.clone())
            .collect();
        assert!(
            !runtime_tests.is_empty(),
            "a conformance source declares runtime tests"
        );

        let mut layout_ctx = layout::merge_layout_ctx(&modules).expect("layout ctx");
        layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, &programs);
        let graph = match &program.image_fn {
            Some(fn_name) => {
                wrela_compiler::eval::interp::eval_image(program, fn_name).expect("image graph")
            }
            None => Default::default(),
        };
        let async_tests: BTreeSet<String> = runtime_tests
            .iter()
            .filter(|name| program.fns.get(*name).is_some_and(|f| f.is_async))
            .cloned()
            .collect();
        let mut test_args: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for name in &runtime_tests {
            let f = &program.fns[name];
            let mut args = Vec::new();
            for p in &f.params {
                let Type::Named(_, targs) = &p.ty else {
                    panic!("Actor[T] param")
                };
                let Some(TypeArg::Type(inner)) = targs.first() else {
                    panic!("Actor[T] param")
                };
                let target = wrela_compiler::sema::types::render_type(inner);
                let idx = graph
                    .actors
                    .iter()
                    .position(|a| wrela_compiler::sema::types::render_type(&a.actor_type) == target)
                    .expect("unique declared instance");
                args.push(idx as u64);
            }
            test_args.insert(name.clone(), args);
        }
        let compiled = layout::lower_and_codegen_image(
            &modules,
            &programs,
            &layout_ctx,
            &graph,
            None,
            None,
            false,
            &runtime_tests,
            &async_tests,
            false,
        )
        .expect("lower_and_codegen_image");
        let boot = layout::BootCtx {
            graph: &graph,
            modules: &compiled.modules,
            programs: &compiled.programs,
            layouts: &compiled.layouts,
            layout_ctx: &compiled.layout_ctx,
            async_frames: &compiled.async_frames,
            group_child_index: &compiled.group_child_index,
            flow: &compiled.flow,
        };
        let image = layout::layout_test_image(
            &compiled.program,
            &runtime_tests,
            &async_tests,
            Some(boot),
            &test_args,
        )
        .expect("layout_test_image");

        let mut report = report_identity("<conformance>", &image.blob);
        for s in &image.sections {
            report.push_str(&format!(
                "Section name={} base={:#x} size={}\n",
                s.name, s.base, s.size
            ));
        }
        report.push_str(&format!("Entry base={:#x}\n", image.entry));
        for (core, base) in &image.core_entries {
            report.push_str(&format!("CoreEntry core={core} base={base:#x}\n"));
        }
        (image, report)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn compile_program_image(src: &str) -> (wrela_compiler::layout::ImageLayout, String) {
        use std::collections::{BTreeMap, BTreeSet};
        use wrela_compiler::layout;

        let tokens = wrela_compiler::syntax::lexer::lex(src).expect("conformance source must lex");
        let module = wrela_compiler::syntax::parser::parse(tokens).expect("must parse");
        let program =
            wrela_compiler::sema::check_typed(&module, "<conformance>").expect("must check");
        let mut modules = BTreeMap::new();
        modules.insert(module.path.join("."), module.clone());
        let mut programs = BTreeMap::new();
        programs.insert(module.path.join("."), program.clone());
        let layout_ctx = layout::merge_layout_ctx(&modules).expect("layout ctx");
        let graph = match &program.image_fn {
            Some(fn_name) => {
                wrela_compiler::eval::interp::eval_image(&program, fn_name).expect("image graph")
            }
            None => Default::default(),
        };
        let empty_async = BTreeSet::new();
        let compiled = layout::lower_and_codegen_image(
            &modules,
            &programs,
            &layout_ctx,
            &graph,
            None,
            None,
            false,
            &[],
            &empty_async,
            false,
        )
        .expect("lower_and_codegen_image");
        let boot = layout::BootCtx {
            graph: &graph,
            modules: &compiled.modules,
            programs: &compiled.programs,
            layouts: &compiled.layouts,
            layout_ctx: &compiled.layout_ctx,
            async_frames: &compiled.async_frames,
            group_child_index: &compiled.group_child_index,
            flow: &compiled.flow,
        };
        let image = layout::layout_program(&compiled.program, Some(boot)).expect("layout_program");

        let mut report = report_identity("<conformance>", &image.blob);
        for sec in &image.sections {
            report.push_str(&format!(
                "Section name={} base={:#x} size={}\n",
                sec.name, sec.base, sec.size
            ));
        }
        report.push_str(&format!("Entry base={:#x}\n", image.entry));
        for (core, base) in &image.core_entries {
            report.push_str(&format!("CoreEntry core={core} base={base:#x}\n"));
        }
        (image, report)
    }

    fn stamp_image_digest(report: &str, img: &[u8]) -> String {
        let dig = wrela_machine::sha256::sha256_hex(img);
        let mut out = String::new();
        for line in report.lines() {
            if line.starts_with("Image sha256=") {
                out.push_str(&format!("Image sha256={dig}\n"));
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn boot_blob(blob: &[u8], report: &str, tag: &str) -> BootOutcome {
        let report = stamp_image_digest(report, blob);
        let dir = std::env::temp_dir().join(format!("wrela-vmm-conf-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let img_path = dir.join("test.img");
        let report_path = dir.join("test.report.txt");
        std::fs::write(&img_path, blob).expect("write img");
        std::fs::write(&report_path, &report).expect("write report");
        let outcome = boot_image(&report_path, &img_path).expect("boot");
        let _ = std::fs::remove_dir_all(&dir);
        outcome
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn boot_source(src: &str, tag: &str) -> BootOutcome {
        let (image, report) = compile_test_image(src);
        boot_blob(&image.blob, &report, tag)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn assert_guest_transcript(transcript: &[u8], summary: &str) {
        let t = String::from_utf8_lossy(transcript);
        assert!(
            t.starts_with(summary),
            "transcript missing expected summary\n  got: {t:?}\n  want prefix: {summary:?}"
        );
        let rest = &t[summary.len()..];
        let mut lines = rest.lines();
        let turns = lines.next().unwrap_or("");
        let hits = lines.next().unwrap_or("");
        assert!(
            turns.starts_with("lane1 turns=")
                && turns.contains(" run_one=")
                && turns.contains(" messages="),
            "expected lane1 turns trailer, got {turns:?} (full: {t:?})"
        );
        assert!(
            hits.starts_with("lane1 hits="),
            "expected lane1 hits trailer, got {hits:?} (full: {t:?})"
        );
        assert!(
            lines.next().is_none(),
            "unexpected transcript after lane1: {t:?}"
        );
        assert!(t.ends_with('\n'), "transcript must end with newline: {t:?}");
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn park_and_resume_two_hop_await_chain_over_hvf() {
        let src = r#"module conformance.chain

@actor
pub struct Inner:
    value: u64

    init(mut self):
        self.value = 41

    pub fn get(read self) -> u64:
        return self.value

@actor
pub struct Outer:
    inner: Actor[Inner]

    pub async fn relay(read self) -> u64:
        v = await self.inner.get()
        @discard(reason="migrated: deliberate Err discard (M13 item L)")
        match v:
            case .Ok(n):
                return n + 1
            case .Err(_):
                return 0

@test(runtime)
async fn chain(outer: Actor[Outer]):
    v = await outer.relay()
    @discard(reason="migrated: deliberate Err discard (M13 item L)")
    match v:
        case .Ok(n):
            assert n == 42, "expected 42 through the two-hop chain"
        case .Err(_):
            assert false, "call rejected"

@image
pub fn build() -> Image:
    img = Image(name="chain", target=Target.wrela_machine_v1)
    inner = img.actor(Inner, mailbox=4)
    outer = img.actor(Outer, mailbox=4, inner=inner)
    img.on_failure(policy=Failure.Halt)
    return img.seal()
"#;
        let outcome = boot_source(src, "chain");
        assert_guest_transcript(&outcome.transcript, "test chain: ok\n1 passed, 0 failed\n");
        assert_eq!(outcome.exit_code, 0);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn park_and_resume_fifo_second_message_waits_for_the_suspended_turn_over_hvf() {
        let src = r#"module conformance.fifo

@actor
pub struct Stamper:
    log: u64

    pub fn stamp(mut self, v: u64) -> u64:
        self.log = self.log * 10 + v
        return v

@actor
pub struct Worker:
    stamps: Actor[Stamper]
    log: u64

    pub async fn job(mut self, v: u64):
        r = await self.stamps.stamp(v=v)
        @discard(reason="migrated: deliberate Err discard (M13 item L)")
        match r:
            case .Ok(n):
                self.log = self.log * 10 + n
            case .Err(_):
                pass

    pub fn log_value(read self) -> u64:
        return self.log

@test(runtime)
async fn fifo(worker: Actor[Worker]):
    r1 = send worker.job(v=1)
    @discard(reason="migrated: deliberate Err discard (M13 item L)")
    match r1:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, "send 1 rejected"
    r2 = send worker.job(v=2)
    @discard(reason="migrated: deliberate Err discard (M13 item L)")
    match r2:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, "send 2 rejected"
    wl = await worker.log_value()
    @discard(reason="migrated: deliberate Err discard (M13 item L)")
    match wl:
        case .Ok(n):
            assert n == 12, "worker turns must complete in FIFO admission order"
        case .Err(_):
            assert false, "log_value rejected"

@image
pub fn build() -> Image:
    img = Image(name="fifo", target=Target.wrela_machine_v1)
    stamps = img.actor(Stamper, mailbox=4)
    worker = img.actor(Worker, mailbox=4, stamps=stamps)
    img.on_failure(policy=Failure.Halt)
    return img.seal()
"#;
        let outcome = boot_source(src, "fifo");
        assert_guest_transcript(&outcome.transcript, "test fifo: ok\n1 passed, 0 failed\n");
        assert_eq!(outcome.exit_code, 0);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn park_and_resume_interleaves_a_third_actor_while_suspended_over_hvf() {
        let src = r#"module conformance.interleave

@actor
pub struct Log:
    log: u64

    pub fn mark(mut self, v: u64) -> u64:
        self.log = self.log * 100 + v
        return v

    pub fn value(read self) -> u64:
        return self.log

@actor
pub struct ChainActor:
    log: Actor[Log]

    pub async fn chain(read self) -> u64:
        a = await self.log.mark(v=10)
        @discard(reason="migrated: deliberate Err discard (M13 item L)")
        match a:
            case .Ok(_):
                pass
            case .Err(_):
                return 0
        b = await self.log.mark(v=20)
        @discard(reason="migrated: deliberate Err discard (M13 item L)")
        match b:
            case .Ok(n):
                return n
            case .Err(_):
                return 0

@actor
pub struct Third:
    log: Actor[Log]

    pub async fn poke(read self):
        r = await self.log.mark(v=99)
        @discard(reason="migrated: deliberate Err discard (M13 item L)")
        match r:
            case .Ok(_):
                pass
            case .Err(_):
                pass

@test(runtime)
async fn interleave(chain: Actor[ChainActor], third: Actor[Third], log: Actor[Log]):
    s = send third.poke()
    @discard(reason="migrated: deliberate Err discard (M13 item L)")
    match s:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, "send rejected"
    r = await chain.chain()
    @discard(reason="migrated: deliberate Err discard (M13 item L)")
    match r:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, "chain rejected"
    v = await log.value()
    @discard(reason="migrated: deliberate Err discard (M13 item L)")
    match v:
        case .Ok(n):
            assert n == 109920, "the third actor's stamp must interleave between the chain's two"
        case .Err(_):
            assert false, "value rejected"

@image
pub fn build() -> Image:
    img = Image(name="interleave", target=Target.wrela_machine_v1)
    log = img.actor(Log, mailbox=8)
    chain = img.actor(ChainActor, mailbox=4, log=log)
    third = img.actor(Third, mailbox=4, log=log)
    img.on_failure(policy=Failure.Halt)
    return img.seal()
"#;
        let outcome = boot_source(src, "interleave");
        assert_guest_transcript(
            &outcome.transcript,
            "test interleave: ok\n1 passed, 0 failed\n",
        );
        assert_eq!(outcome.exit_code, 0);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn deadlock_diagnostic_prints_the_named_line_and_exits_nonzero_over_hvf() {
        use wrela_compiler::codegen::{OFF_TURN_BUSY, OFF_TURN_SUSPENDED};
        use wrela_compiler::layout::{DEADLOCK_MSG, place_runtime_tables};
        use wrela_machine::layout as machine_layout;

        let src = r#"module conformance.deadlock

@actor
pub struct Stuck:
    value: u64

    pub fn nudge(read self) -> u64:
        return self.value

@test(runtime)
async fn stuck(target: Actor[Stuck]):
    v = await target.nudge()
    @discard(reason="migrated: deliberate Err discard (M13 item L)")
    match v:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, "rejected"

@image
pub fn build() -> Image:
    img = Image(name="deadlock", target=Target.wrela_machine_v1)
    s = img.actor(Stuck, mailbox=4)
    img.on_failure(policy=Failure.Halt)
    return img.seal()
"#;
        let (image, report) = compile_test_image(src);
        let tables = image.runtime.as_ref().expect("actor image has tables");
        let rtdata = image
            .sections
            .iter()
            .find(|s| s.name == "rtdata")
            .expect("rtdata section");
        let placement = place_runtime_tables(rtdata.base, tables);
        let stuck = &placement.actors[0];
        let mut blob = image.blob.clone();
        let patch = |blob: &mut Vec<u8>, addr: u64, v: u64| {
            let off = (addr - machine_layout::IMAGE_BASE) as usize;
            blob[off..off + 8].copy_from_slice(&v.to_le_bytes());
        };
        patch(&mut blob, stuck.turn + OFF_TURN_BUSY, 1);
        patch(&mut blob, stuck.turn + OFF_TURN_SUSPENDED, 1);

        let outcome = boot_blob(&blob, &report, "deadlock");
        assert_guest_transcript(
            &outcome.transcript,
            &format!("test stuck: FAILED {DEADLOCK_MSG}\n0 passed, 1 failed\n"),
        );
        assert_eq!(outcome.exit_code, 1, "fail closed: the image exits nonzero");
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn park_conformance_wakes_at_the_deadline_and_resumes_over_hvf() {
        use wrela_compiler::encode;
        use wrela_machine::{layout as machine_layout, machine_info, mmio, pending};

        const DELTA_NS: u64 = 3_000_000;
        let sp_top = machine_layout::core_stack_base_n(0, 1) + machine_layout::CORE_STACK_SIZE;

        let mut w = Vec::new();
        w.extend(load_imm_words(9, sp_top));
        w.push(encode::enc_add_imm(31, 9, 0, true));

        w.extend(load_imm_words(9, mmio::CLOCK_MMIO_ADDR));
        w.push(encode::enc_ldr_x_imm(1, 9, 0));

        w.extend(load_imm_words(2, DELTA_NS));
        w.push(encode::enc_add_reg(1, 1, 2, true));

        w.extend(load_imm_words(
            9,
            machine_layout::MACHINE_INFO_BASE + machine_info::OFF_NEXT_DEADLINE,
        ));
        w.push(encode::enc_str_x_imm(1, 9, 0));

        w.extend(load_imm_words(9, mmio::PARK_MMIO_ADDR));
        w.push(encode::enc_str_x_imm(1, 9, 0));

        w.extend(load_imm_words(9, pending::core_word_addr(0)));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        w.push(encode::enc_movz(11, 0, 0, true));
        w.extend(check_eq_into(11, 12, 10, 1, 0));

        w.extend(load_imm_words(12, mmio::EXIT_MMIO_ADDR));
        w.push(encode::enc_str_x_imm(11, 12, 0));
        w.push(encode::enc_brk(0));

        let img_bytes: Vec<u8> = w.iter().flat_map(|word| word.to_le_bytes()).collect();
        let outcome = boot_hand_built_image(&img_bytes, "park-wake");
        assert_eq!(
            outcome.exit_code, 0,
            "the pending word must read 1 after the park's own resume (the VMM's raise)"
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn subword_mmio_access_is_a_named_guest_fault_over_hvf() {
        use wrela_compiler::encode;
        use wrela_machine::mmio;

        let mut w = Vec::new();
        w.extend(load_imm_words(9, mmio::CLOCK_MMIO_ADDR));
        w.push(encode::enc_ldrb_imm(1, 9, 0));
        w.push(encode::enc_brk(0));

        let img_bytes: Vec<u8> = w.iter().flat_map(|word| word.to_le_bytes()).collect();
        let (report_path, img_path) = write_hand_built_image(&img_bytes, "mmio-subword");
        let err = boot_image(&report_path, &img_path)
            .expect_err("a 1-byte CLOCK load must fault, never approximate");
        let _ = std::fs::remove_dir_all(report_path.parent().unwrap());
        let msg = err.to_string();
        assert!(
            msg.contains("8-byte access") && msg.contains("1-byte"),
            "must name the width rule: {msg}"
        );
        assert!(
            msg.contains("CLOCK_MMIO_ADDR"),
            "must name the MMIO address: {msg}"
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn vector_raise_observed_at_a_checkpoint_over_hvf() {
        use wrela_compiler::encode;
        use wrela_machine::{layout as machine_layout, machine_info, pending};

        const LOOP_BOUND: u64 = 200_000_000;
        let sp_top = machine_layout::core_stack_base_n(0, 1) + machine_layout::CORE_STACK_SIZE;
        let (cp_words, cp_entry_offset) = hand_built_simple_checkpoint();

        let mut w = Vec::new();
        w.extend(load_imm_words(9, sp_top));
        w.push(encode::enc_add_imm(31, 9, 0, true));
        w.extend(load_imm_words(19, LOOP_BOUND));

        let loop_top = w.len();
        w.extend(load_imm_words(9, pending::core_word_addr(0)));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        w.push(encode::enc_cbz(10, 8, true));
        let bl_word = w.len();
        w.push(0);
        w.push(encode::enc_subs_imm(19, 19, 1, true));
        {
            let this = w.len() as i64;
            let delta = (loop_top as i64 - this) * 4;
            w.push(encode::enc_cbnz(19, delta as i32, true));
        }

        let skip_cp_word = w.len();
        w.push(0);

        let cp_base = w.len();
        {
            let this = bl_word as i64;
            let target = (cp_base + cp_entry_offset) as i64;
            w[bl_word] = encode::enc_bl(((target - this) * 4) as i32);
        }
        w.extend(cp_words);
        {
            let after_cp = w.len() as i64;
            let this = skip_cp_word as i64;
            w[skip_cp_word] = encode::enc_b(((after_cp - this) * 4) as i32);
        }

        w.extend(load_imm_words(
            9,
            machine_layout::MACHINE_INFO_BASE + machine_info::OFF_VECTOR0_OBSERVED,
        ));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        w.push(encode::enc_movz(11, 0, 0, true));
        w.extend(check_eq_into(11, 12, 10, 1, 0));
        w.extend(check_eq_into(11, 12, 19, 0, 1));

        w.extend(load_imm_words(12, wrela_machine::mmio::EXIT_MMIO_ADDR));
        w.push(encode::enc_str_x_imm(11, 12, 0));
        w.push(encode::enc_brk(0));

        let img_bytes: Vec<u8> = w.iter().flat_map(|word| word.to_le_bytes()).collect();
        let (report_path, img_path) = write_hand_built_image(&img_bytes, "vector-raise");
        let (outcome, divergences) = boot_image_core_with_delayed_raise(
            &report_path,
            &img_path,
            None,
            Some((Duration::from_millis(10), 1)),
        )
        .expect("live boot");
        let _ = std::fs::remove_dir_all(report_path.parent().unwrap());
        assert!(divergences.is_empty(), "a live boot cannot diverge");
        assert_eq!(
            outcome.exit_code, 0,
            "bit 0 = observed-count != 1, bit 1 = loop counter != 0"
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn an_el1_fault_into_the_absent_vector_table_names_the_original_esr_over_hvf() {
        use wrela_compiler::encode;
        use wrela_machine::layout as machine_layout;

        let bad = machine_layout::DRAM_BASE + 0x8004;
        let mut w = Vec::new();
        w.extend(load_imm_words(9, bad));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        w.push(encode::enc_brk(0));

        let img_bytes: Vec<u8> = w.iter().flat_map(|word| word.to_le_bytes()).collect();
        let report_text = format!(
            "{}Section name=entry base={:#x} size={}\nEntry base={:#x}\n",
            report_identity("el1-vector.wr", &img_bytes),
            machine_layout::IMAGE_BASE,
            img_bytes.len(),
            machine_layout::IMAGE_BASE,
        );
        let tmp_dir =
            std::env::temp_dir().join(format!("wrela-vmm-el1-vector-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let img_path = tmp_dir.join("el1-vector.img");
        let report_path = tmp_dir.join("el1-vector.report.txt");
        std::fs::write(&img_path, &img_bytes).expect("write image");
        std::fs::write(&report_path, &report_text).expect("write report");
        let err = boot_image(&report_path, &img_path)
            .expect_err("an unaligned 64-bit load must fault, never boot cleanly");
        let _ = std::fs::remove_dir_all(&tmp_dir);

        let msg = err.to_string();
        assert!(
            msg.contains("pc=0x200"),
            "the bare fault is still reported verbatim: {msg}"
        );
        assert!(
            msg.contains("VBAR_EL1(0x0) + 0x200"),
            "the note must name the vector slot: {msg}"
        );
        assert!(
            msg.contains("(EC=0x25)"),
            "the note must carry the ORIGINAL fault's own ESR_EL1 exception class \
             (0x25 = data abort, same EL): {msg}"
        );
        assert!(
            msg.contains(&format!("FAR_EL1={bad:#x}")),
            "the note must carry the real faulting address: {msg}"
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn record_replay_of_the_park_wake_scenario_is_byte_stable_and_detects_tamper() {
        use wrela_compiler::encode;
        use wrela_machine::{layout as machine_layout, machine_info, mmio, pending};

        const DELTA_NS: u64 = 2_000_000;
        let sp_top = machine_layout::core_stack_base_n(0, 1) + machine_layout::CORE_STACK_SIZE;

        let mut w = Vec::new();
        w.extend(load_imm_words(9, sp_top));
        w.push(encode::enc_add_imm(31, 9, 0, true));
        w.extend(load_imm_words(9, mmio::CLOCK_MMIO_ADDR));
        w.push(encode::enc_ldr_x_imm(1, 9, 0));
        w.extend(load_imm_words(2, DELTA_NS));
        w.push(encode::enc_add_reg(1, 1, 2, true));
        w.extend(load_imm_words(
            9,
            machine_layout::MACHINE_INFO_BASE + machine_info::OFF_NEXT_DEADLINE,
        ));
        w.push(encode::enc_str_x_imm(1, 9, 0));
        w.extend(load_imm_words(9, mmio::PARK_MMIO_ADDR));
        w.push(encode::enc_str_x_imm(1, 9, 0));
        w.extend(load_imm_words(9, pending::core_word_addr(0)));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        w.push(encode::enc_movz(11, 0, 0, true));
        w.extend(check_eq_into(11, 12, 10, 1, 0));
        w.extend(load_imm_words(12, mmio::EXIT_MMIO_ADDR));
        w.push(encode::enc_str_x_imm(11, 12, 0));
        w.push(encode::enc_brk(0));

        let img_bytes: Vec<u8> = w.iter().flat_map(|word| word.to_le_bytes()).collect();
        let (report_path, img_path) = write_hand_built_image(&img_bytes, "park-wake-replay");

        let recorded = record::record(&report_path, &img_path).expect("live boot");
        assert_eq!(recorded.exit_code, 0);
        assert_eq!(recorded.choices.len(), 3);
        assert!(matches!(
            recorded.choices[0],
            record::ChoiceEntry::ClockRead { .. }
        ));
        assert!(matches!(
            recorded.choices[1],
            record::ChoiceEntry::DeadlineWake { .. }
        ));
        assert_eq!(
            recorded.choices[2],
            record::ChoiceEntry::VectorRaise { vector: 0 }
        );

        let divergences = record::replay(&report_path, &img_path, &recorded).expect("replay boot");
        assert!(
            divergences.is_empty(),
            "expected no divergence, got {divergences:?}"
        );

        let mut bad_tag = recorded.clone();
        bad_tag.choices[1] = record::ChoiceEntry::ClockRead { value: 0 };
        let err =
            record::replay(&report_path, &img_path, &bad_tag).expect_err("strict tag mismatch");
        assert!(err.to_string().contains("replay divergence"), "got {err}");

        let mut bad_exit = recorded.clone();
        bad_exit.exit_code = 7;
        let divergences = record::replay(&report_path, &img_path, &bad_exit).expect("replay boot");
        assert!(divergences.contains(&record::Divergence::ExitCodeMismatch {
            expected: 7,
            actual: 0,
        }));

        let _ = std::fs::remove_dir_all(report_path.parent().unwrap());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn replay_divergence_and_record_failures_exit_nonzero_through_the_real_binary() {
        use std::process::Command;

        use wrela_machine::vmm_process::{EXIT_REPLAY_DIVERGENCE, EXIT_VMM_FAILURE};
        const GUEST_EXIT_CODE: u64 = 5;

        let build = Command::new("cargo")
            .args(["build", "--quiet", "-p", "wrela-vmm", "--bin", "wrela-vmm"])
            .status()
            .expect("run cargo build");
        assert!(
            build.success(),
            "cargo build -p wrela-vmm --bin wrela-vmm failed"
        );
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("crates/wrela-vmm has two ancestors up to the repo root")
            .to_path_buf();
        let bin = repo_root.join("target/debug/wrela-vmm");
        let entitlements = repo_root.join("crates/wrela-vmm/entitlements.plist");
        let codesign = Command::new("codesign")
            .args(["--force", "--sign", "-", "--entitlements"])
            .arg(&entitlements)
            .arg(&bin)
            .status()
            .expect("run codesign");
        assert!(codesign.success(), "codesign wrela-vmm failed");

        let mut words = Vec::new();
        push_halt(&mut words, 9, 10, GUEST_EXIT_CODE);
        let img_bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let report_text = format!(
            "{}Section name=entry base={:#x} size={}\nEntry base={:#x}\n",
            report_identity("exit-code-contract.wr", &img_bytes),
            wrela_machine::layout::IMAGE_BASE,
            img_bytes.len(),
            wrela_machine::layout::IMAGE_BASE,
        );
        let tmp_dir = std::env::temp_dir().join(format!(
            "wrela-vmm-exit-code-contract-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let img_path = tmp_dir.join("test.img");
        let report_path = tmp_dir.join("test.report.txt");
        let record_path = tmp_dir.join("test.record.txt");
        std::fs::write(&img_path, &img_bytes).expect("write image");
        std::fs::write(&report_path, &report_text).expect("write report");

        let record_out = Command::new(&bin)
            .arg(&report_path)
            .arg(&img_path)
            .arg("--record")
            .arg(&record_path)
            .output()
            .expect("run wrela-vmm --record");
        assert_eq!(
            record_out.status.code(),
            Some(1),
            "a nonzero guest exit code must collapse to process exit 1 on a plain --record boot"
        );

        let clean_replay = Command::new(&bin)
            .arg(&report_path)
            .arg(&img_path)
            .arg("--replay")
            .arg(&record_path)
            .output()
            .expect("run wrela-vmm --replay");
        assert_eq!(
            clean_replay.status.code(),
            Some(1),
            "a clean (non-diverging) replay must mirror the guest's own exit code exactly like a \
             plain boot, never unconditionally ExitCode::SUCCESS"
        );

        let record_text = std::fs::read_to_string(&record_path).expect("read record");
        let tampered_text: String = record_text
            .lines()
            .map(|line| match line.strip_prefix("exit_code=") {
                Some(v) => {
                    let original: u64 = v.parse().unwrap_or(0);
                    format!("exit_code={}", original ^ 0xFF)
                }
                None => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let tampered_path = tmp_dir.join("tampered.record.txt");
        std::fs::write(&tampered_path, &tampered_text).expect("write tampered record");
        let diverged_replay = Command::new(&bin)
            .arg(&report_path)
            .arg(&img_path)
            .arg("--replay")
            .arg(&tampered_path)
            .output()
            .expect("run wrela-vmm --replay (tampered)");
        assert_eq!(
            diverged_replay.status.code(),
            Some(EXIT_REPLAY_DIVERGENCE),
            "a replay with a tampered exit_code must exit EXIT_REPLAY_DIVERGENCE, never 0 \
             (stderr: {})",
            String::from_utf8_lossy(&diverged_replay.stderr)
        );
        assert!(
            String::from_utf8_lossy(&diverged_replay.stderr).contains("exit code mismatch"),
            "the tampered replay's own stderr must name the exit-code mismatch"
        );

        let malformed_path = tmp_dir.join("malformed.record.txt");
        std::fs::write(&malformed_path, b"not a choice log at all\n").expect("write malformed");
        let malformed_replay = Command::new(&bin)
            .arg(&report_path)
            .arg(&img_path)
            .arg("--replay")
            .arg(&malformed_path)
            .output()
            .expect("run wrela-vmm --replay (malformed)");
        assert_eq!(
            malformed_replay.status.code(),
            Some(EXIT_VMM_FAILURE),
            "a malformed --replay record file must exit EXIT_VMM_FAILURE"
        );

        let unwritable_path = tmp_dir.join("no-such-subdir").join("rec.txt");
        let unwritable_record = Command::new(&bin)
            .arg(&report_path)
            .arg(&img_path)
            .arg("--record")
            .arg(&unwritable_path)
            .output()
            .expect("run wrela-vmm --record (unwritable)");
        assert_eq!(
            unwritable_record.status.code(),
            Some(EXIT_VMM_FAILURE),
            "--record to an unwritable path must exit EXIT_VMM_FAILURE"
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    struct BlkImage {
        img_bytes: Vec<u8>,
        report_text: String,
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn build_blk_conformance_image() -> BlkImage {
        use wrela_machine::layout as machine_layout;

        const QUEUE_SIZE: u64 = 8;
        const OFF_DESC: u64 = 0x000;
        const OFF_AVAIL: u64 = 0x080;
        const OFF_USED: u64 = 0x0C0;
        const OFF_DOORBELL: u64 = 0x140;
        const OFF_HDR1: u64 = 0x150;
        const OFF_HDR2: u64 = 0x160;
        const OFF_STATUS1: u64 = 0x170;
        const OFF_STATUS2: u64 = 0x178;
        const OFF_SRC: u64 = 0x200;
        const OFF_DST: u64 = 0x400;
        const DATA_REGION_SIZE: u64 = 0x600;
        const BLK_VECTOR: u64 = 1;

        let payload: Vec<u8> = (0..512u32).map(|i| ((i * 7 + 3) % 256) as u8).collect();
        let expect_first = u64::from_le_bytes(payload[0..8].try_into().unwrap());
        let expect_last = u64::from_le_bytes(payload[504..512].try_into().unwrap());

        let sp_top = machine_layout::core_stack_base_n(0, 1) + machine_layout::CORE_STACK_SIZE;

        fn build_entry(
            sp_top: u64,
            data_base: u64,
            expect_first: u64,
            expect_last: u64,
        ) -> Vec<u32> {
            use wrela_compiler::encode;
            use wrela_machine::{layout as machine_layout, machine_info, mmio, pending};

            let avail = data_base + OFF_AVAIL;
            let used = data_base + OFF_USED;
            let doorbell = data_base + OFF_DOORBELL;

            let mut w = Vec::new();
            w.extend(load_imm_words(9, sp_top));
            w.push(encode::enc_add_imm(31, 9, 0, true));

            let publish = |w: &mut Vec<u32>, idx: u64| {
                w.extend(load_imm_words(9, avail));
                w.extend(load_imm_words(10, (idx << 16) | (0 << 32) | (3 << 48)));
                w.push(encode::enc_str_x_imm(10, 9, 0));
                w.extend(load_imm_words(9, doorbell));
                w.push(encode::enc_movz(10, 1, 0, true));
                w.push(encode::enc_str_x_imm(10, 9, 0));
            };
            let park = |w: &mut Vec<u32>| {
                w.extend(load_imm_words(9, mmio::CLOCK_MMIO_ADDR));
                w.push(encode::enc_ldr_x_imm(11, 9, 0));
                w.extend(load_imm_words(12, 20_000_000));
                w.push(encode::enc_add_reg(11, 11, 12, true));
                w.extend(load_imm_words(
                    9,
                    machine_layout::MACHINE_INFO_BASE + machine_info::OFF_NEXT_DEADLINE,
                ));
                w.push(encode::enc_str_x_imm(11, 9, 0));
                w.extend(load_imm_words(9, mmio::PARK_MMIO_ADDR));
                w.push(encode::enc_str_x_imm(11, 9, 0));
            };

            publish(&mut w, 1);
            park(&mut w);
            publish(&mut w, 2);
            park(&mut w);

            w.extend(load_imm_words(9, used));
            w.push(encode::enc_ldr_x_imm(19, 9, 0));
            w.push(encode::enc_ldr_x_imm(20, 9, 8));
            w.push(encode::enc_ldr_x_imm(21, 9, 16));
            w.extend(load_imm_words(9, data_base + OFF_STATUS1));
            w.push(encode::enc_ldrb_imm(22, 9, 0));
            w.extend(load_imm_words(9, data_base + OFF_STATUS2));
            w.push(encode::enc_ldrb_imm(23, 9, 0));
            w.extend(load_imm_words(9, data_base + OFF_DST));
            w.push(encode::enc_ldr_x_imm(24, 9, 0));
            w.push(encode::enc_ldr_x_imm(25, 9, 504));
            w.extend(load_imm_words(9, pending::core_word_addr(0)));
            w.push(encode::enc_ldr_x_imm(26, 9, 0));

            w.push(encode::enc_movz(1, 0, 0, true));
            let check = |w: &mut Vec<u32>, actual: u8, expect: u64, bit: u8| {
                w.extend(load_imm_words(13, expect));
                w.push(encode::enc_cmp_reg(actual, 13, true));
                w.push(encode::enc_cset(14, encode::Cond::Ne, true));
                if bit > 0 {
                    w.push(encode::enc_lsl_imm(14, 14, bit, true));
                }
                w.push(encode::enc_orr_reg(1, 1, 14, true));
            };
            check(&mut w, 19, 2u64 << 16, 0);
            check(&mut w, 20, 1 | (3u64 << 32), 1);
            check(&mut w, 21, 513, 2);
            check(&mut w, 22, 0, 3);
            check(&mut w, 23, 0, 4);
            check(&mut w, 24, expect_first, 5);
            check(&mut w, 25, expect_last, 6);
            check(&mut w, 26, 1u64 << BLK_VECTOR, 7);

            w.extend(load_imm_words(15, mmio::EXIT_MMIO_ADDR));
            w.push(encode::enc_str_x_imm(1, 15, 0));
            w.push(encode::enc_brk(0));
            w
        }

        let entry_len = build_entry(sp_top, 0, 0, 0).len();
        let code_bytes = (entry_len as u64) * 4;
        const PAGE: u64 = 16 * 1024;
        let code_span = code_bytes.div_ceil(PAGE) * PAGE;
        let data_base = machine_layout::IMAGE_BASE + code_span;
        let words = build_entry(sp_top, data_base, expect_first, expect_last);
        assert_eq!(
            words.len(),
            entry_len,
            "the entry sequence's own length must not depend on the real addresses"
        );

        let mut img: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        img.resize((code_span + DATA_REGION_SIZE) as usize, 0);
        let data_off = (data_base - machine_layout::IMAGE_BASE) as usize;
        let put = |img: &mut Vec<u8>, off: u64, bytes: &[u8]| {
            let at = data_off + off as usize;
            img[at..at + bytes.len()].copy_from_slice(bytes);
        };
        let desc = |img: &mut Vec<u8>, i: u64, addr: u64, len: u32, flags: u16, next: u16| {
            let at = OFF_DESC + i * devices::DESC_SIZE;
            put(img, at, &addr.to_le_bytes());
            put(img, at + 8, &len.to_le_bytes());
            put(img, at + 12, &flags.to_le_bytes());
            put(img, at + 14, &next.to_le_bytes());
        };

        put(&mut img, OFF_HDR1, &devices::T_OUT.to_le_bytes());
        put(&mut img, OFF_HDR1 + 8, &0u64.to_le_bytes());
        desc(
            &mut img,
            0,
            data_base + OFF_HDR1,
            16,
            devices::DESC_F_NEXT,
            1,
        );
        desc(
            &mut img,
            1,
            data_base + OFF_SRC,
            512,
            devices::DESC_F_NEXT,
            2,
        );
        desc(
            &mut img,
            2,
            data_base + OFF_STATUS1,
            1,
            devices::DESC_F_WRITE,
            0,
        );
        put(&mut img, OFF_HDR2, &devices::T_IN.to_le_bytes());
        put(&mut img, OFF_HDR2 + 8, &0u64.to_le_bytes());
        desc(
            &mut img,
            3,
            data_base + OFF_HDR2,
            16,
            devices::DESC_F_NEXT,
            4,
        );
        desc(
            &mut img,
            4,
            data_base + OFF_DST,
            512,
            devices::DESC_F_NEXT | devices::DESC_F_WRITE,
            5,
        );
        desc(
            &mut img,
            5,
            data_base + OFF_STATUS2,
            1,
            devices::DESC_F_WRITE,
            0,
        );
        put(&mut img, OFF_STATUS1, &[0xEE]);
        put(&mut img, OFF_STATUS2, &[0xEE]);
        put(&mut img, OFF_SRC, &payload);

        let report_text = format!(
            "{}Section name=entry base={:#x} size={}\n\
             Entry base={:#x}\n\
             BlkDevice device=device#0 capacity_sectors=16 features={:#x} vector={BLK_VECTOR}\n\
             BlkQueue index=0 size={QUEUE_SIZE} desc={:#x} avail={:#x} used={:#x} doorbell={:#x}\n\
             BlkPool name=BlockControl device=device#0 base={:#x} size={:#x}\n",
            report_identity("blk-conformance.wr", &img),
            machine_layout::IMAGE_BASE,
            code_bytes,
            machine_layout::IMAGE_BASE,
            devices::DEVICE_FEATURES,
            data_base + OFF_DESC,
            data_base + OFF_AVAIL,
            data_base + OFF_USED,
            data_base + OFF_DOORBELL,
            data_base,
            DATA_REGION_SIZE,
        );
        BlkImage {
            img_bytes: img,
            report_text,
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn write_blk_conformance_image(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let built = build_blk_conformance_image();
        let tmp_dir =
            std::env::temp_dir().join(format!("wrela-vmm-{tag}-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let img_path = tmp_dir.join(format!("{tag}.img"));
        let report_path = tmp_dir.join(format!("{tag}.report.txt"));
        std::fs::write(&img_path, &built.img_bytes).expect("write image");
        std::fs::write(&report_path, &built.report_text).expect("write report");
        (report_path, img_path)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn blk_doorbell_write_then_read_completes_over_hvf() {
        let (report_path, img_path) = write_blk_conformance_image("blk-doorbell");
        let outcome = boot_image(&report_path, &img_path).expect("live boot");
        let _ = std::fs::remove_dir_all(report_path.parent().unwrap());
        assert_eq!(
            outcome.exit_code, 0,
            "bit 0 = used header (flags/idx/id0), 1 = used[0].len + id1, 2 = used[1].len, \
             3 = write status byte, 4 = read status byte, 5 = first payload word, \
             6 = last payload word, 7 = pending word (only the blk vector, no deadline wake)"
        );
        let completions: Vec<_> = outcome
            .choices
            .iter()
            .filter(|c| matches!(c, record::ChoiceEntry::DeviceCompletion { .. }))
            .collect();
        assert_eq!(completions.len(), 2, "{:?}", outcome.choices);
        assert!(
            !outcome
                .choices
                .iter()
                .any(|c| matches!(c, record::ChoiceEntry::DeadlineWake { .. })),
            "a completion serviced on the park path must suppress the sleep entirely: {:?}",
            outcome.choices
        );
        match &completions[0] {
            record::ChoiceEntry::DeviceCompletion {
                device,
                queue,
                head,
                status,
                len,
                digest,
            } => {
                assert_eq!((device.as_str(), *queue, *head), ("blk", 0, 0));
                assert_eq!((*status, *len), (0, 1));
                assert!(!digest.is_empty());
            }
            other => panic!("expected a device completion, got {other:?}"),
        }
        match &completions[1] {
            record::ChoiceEntry::DeviceCompletion {
                head, status, len, ..
            } => assert_eq!((*head, *status, *len), (3, 0, 513)),
            other => panic!("expected a device completion, got {other:?}"),
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn blk_completions_record_and_replay_and_every_tamper_diverges() {
        let (report_path, img_path) = write_blk_conformance_image("blk-replay");

        let recorded = record::record(&report_path, &img_path).expect("live boot");
        assert_eq!(recorded.exit_code, 0);
        let completion_indices: Vec<usize> = recorded
            .choices
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c, record::ChoiceEntry::DeviceCompletion { .. }))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(completion_indices.len(), 2);
        let text = recorded.to_text();
        assert!(text.contains("DeviceCompletion device=blk queue=0 head=0 status=0 len=1 digest="));
        assert_eq!(
            record::RecordFile::parse(&text).expect("parses"),
            recorded,
            "the completion tag must survive the record file's own text format"
        );

        let divergences = record::replay(&report_path, &img_path, &recorded).expect("replay boot");
        assert!(
            divergences.is_empty(),
            "expected a clean replay, got {divergences:?}"
        );

        let idx = completion_indices[0];
        let record::ChoiceEntry::DeviceCompletion {
            device,
            queue,
            head,
            status,
            len,
            digest,
        } = recorded.choices[idx].clone()
        else {
            unreachable!("filtered above")
        };
        let tampered = [
            record::ChoiceEntry::DeviceCompletion {
                device: "net".to_string(),
                queue,
                head,
                status,
                len,
                digest: digest.clone(),
            },
            record::ChoiceEntry::DeviceCompletion {
                device: device.clone(),
                queue,
                head: head + 1,
                status,
                len,
                digest: digest.clone(),
            },
            record::ChoiceEntry::DeviceCompletion {
                device: device.clone(),
                queue,
                head,
                status: 1,
                len,
                digest: digest.clone(),
            },
            record::ChoiceEntry::DeviceCompletion {
                device: device.clone(),
                queue,
                head,
                status,
                len: len + 8,
                digest: digest.clone(),
            },
            record::ChoiceEntry::DeviceCompletion {
                device,
                queue,
                head,
                status,
                len,
                digest: "0000000000000000".to_string(),
            },
        ];
        for entry in tampered {
            let mut bad = recorded.clone();
            bad.choices[idx] = entry.clone();
            let err = record::replay(&report_path, &img_path, &bad)
                .expect_err("strict device-completion mismatch");
            assert!(
                err.to_string().contains("replay divergence"),
                "tampering `{}` must abort, got {err}",
                entry.to_text_fields()
            );
        }

        let _ = std::fs::remove_dir_all(report_path.parent().unwrap());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn a_descriptor_outside_every_declared_pool_fails_the_boot_closed_over_hvf() {
        use wrela_machine::layout as machine_layout;
        let built = build_blk_conformance_image();
        let data_base = built
            .report_text
            .lines()
            .find_map(|l| l.strip_prefix("BlkPool name=BlockControl device=device#0 base="))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok())
            .expect("the report declares the pool this image uses");
        let mut img = built.img_bytes.clone();
        let desc1 = (data_base - machine_layout::IMAGE_BASE) as usize + devices::DESC_SIZE as usize;
        img[desc1..desc1 + 8].copy_from_slice(&machine_layout::DRAM_BASE.to_le_bytes());
        let new_digest = wrela_machine::sha256::sha256_hex(&img);
        let report_text = built
            .report_text
            .lines()
            .map(|l| {
                if let Some(rest) = l.strip_prefix("Image sha256=") {
                    let _ = rest;
                    format!("Image sha256={new_digest}")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        let tmp_dir =
            std::env::temp_dir().join(format!("wrela-vmm-blk-oob-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let img_path = tmp_dir.join("blk-oob.img");
        let report_path = tmp_dir.join("blk-oob.report.txt");
        std::fs::write(&img_path, &img).expect("write image");
        std::fs::write(&report_path, &report_text).expect("write report");
        let err = boot_image(&report_path, &img_path)
            .expect_err("a descriptor outside every declared pool must fail the boot closed");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        let msg = err.to_string();
        assert!(
            msg.contains("virtio-blk") && msg.contains("not device-reachable"),
            "the diagnostic must name the device and the rule it broke: {msg}"
        );
    }

    #[test]
    fn parse_report_accepts_a_declared_blk_device() {
        let text = format!(
            "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nSection name=entry base=0x40500000 size=1\nEntry base=0x40500000\n\
             BlkDevice device=device#0 capacity_sectors=2048 features=0x100000200 vector=1\n\
             BlkQueue index=0 size=128 desc=0x40600000 avail=0x40601000 used=0x40602000 doorbell=0x40603000\n\
             BlkPool name=BlockControl device=device#0 base=0x40600000 size=0x10000\n\
             BlkPool name=Foreign device=device#1 base=0x40700000 size=0x1000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        let parsed = parse_report(&text).expect("parses");
        let blk = parsed.blk.expect("a declared device");
        assert_eq!(blk.capacity_sectors, 2048);
        assert_eq!(blk.features, devices::DEVICE_FEATURES);
        assert_eq!(blk.vector, Some(1));
        assert_eq!(blk.queue.size, 128);
        assert_eq!(blk.queue.desc, 0x4060_0000);
        assert_eq!(blk.device, 0);
        assert_eq!(blk.pools.len(), 2);
        assert_eq!(blk.pools[0].name, "BlockControl");
        assert_eq!(blk.pools[0].device, 0);
        assert_eq!(blk.pools[1].name, "Foreign");
        assert_eq!(blk.pools[1].device, 1);
    }

    #[test]
    fn parse_report_rejects_every_malformed_blk_declaration() {
        let head = format!(
            "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nSection name=entry base=0x40500000 size=1\nEntry base=0x40500000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        let device = "BlkDevice device=device#0 capacity_sectors=16 features=0x100000200\n";
        let queue = "BlkQueue index=0 size=8 desc=0x40600000 avail=0x40600100 used=0x40600200 doorbell=0x40600300\n";
        let pool = "BlkPool name=P device=device#0 base=0x40600000 size=0x1000\n";
        for (why, extra) in [
            ("a device with no queue", format!("{device}{pool}")),
            ("a queue with no device", format!("{queue}{pool}")),
            ("a device with no pool", format!("{device}{queue}")),
            ("a pool with no device", pool.to_string()),
            ("two devices", format!("{device}{device}{queue}{pool}")),
            ("two queues", format!("{device}{queue}{queue}{pool}")),
            (
                "a second queue index",
                format!(
                    "{device}BlkQueue index=1 size=8 desc=0x40600000 avail=0x40600100 used=0x40600200 doorbell=0x40600300\n{pool}"
                ),
            ),
            (
                "an unknown field",
                format!(
                    "BlkDevice device=device#0 capacity_sectors=16 features=0x1 mystery=3\n{queue}{pool}"
                ),
            ),
            (
                "a repeated field",
                format!(
                    "BlkDevice device=device#0 capacity_sectors=16 capacity_sectors=32 features=0x1\n{queue}{pool}"
                ),
            ),
            (
                "a missing required field",
                format!("BlkDevice device=device#0 capacity_sectors=16\n{queue}{pool}"),
            ),
            (
                "a field with no `=`",
                format!(
                    "BlkDevice device=device#0 capacity_sectors 16 features=0x1\n{queue}{pool}"
                ),
            ),
            (
                "an unparseable number",
                format!(
                    "BlkDevice device=device#0 capacity_sectors=lots features=0x1\n{queue}{pool}"
                ),
            ),
            (
                "a device line with no `device=`",
                format!("BlkDevice capacity_sectors=16 features=0x1\n{queue}{pool}"),
            ),
            (
                "a pool line with no `device=`",
                format!("{device}{queue}BlkPool name=P base=0x40600000 size=0x1000\n"),
            ),
            (
                "a bare integer device index",
                format!("{device}{queue}BlkPool name=P device=0 base=0x40600000 size=0x1000\n"),
            ),
            (
                "a pool bound to a device with no model",
                format!(
                    "{device}{queue}BlkPool name=P device=device#1 base=0x40600000 size=0x1000\n"
                ),
            ),
            (
                "a queue depth wider than 16 bits",
                format!(
                    "{device}BlkQueue index=0 size=70000 desc=0x40600000 avail=0x40600100 used=0x40600200 doorbell=0x40600300\n{pool}"
                ),
            ),
        ] {
            let text = format!("{head}{extra}");
            assert!(
                matches!(parse_report(&text), Err(VmmError::MalformedReport(_))),
                "{why} must be refused"
            );
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    const CROSS_CORE_SRC: &str = r#"module conformance.cross_core

@actor
pub struct Home:
    value: u64

    init(mut self):
        self.value = 5

    pub fn get(read self) -> u64:
        return self.value

@actor
pub struct Away:
    n: u32

    init(mut self):
        self.n = 0

    pub fn poke(read self) -> u32:
        return self.n

@test(runtime)
async fn boots(home: Actor[Home]):
    v = await home.get()
    @discard(reason="migrated: deliberate Err discard (M13 item L)")
    match v:
        case .Ok(n):
            assert n == 5, "expected 5"
        case .Err(_):
            assert false, "rejected"

@image
pub fn build() -> Image:
    img = Image(name="cross-core-conf", target=Target.wrela_machine_v1, cores=3)
    home = img.actor(Home, mailbox=4, core=0)
    away = img.actor(Away, mailbox=2, core=1)
    img.on_failure(policy=Failure.Halt)
    return img.seal()
"#;

    const TWO_CORE_SRC: &str = r#"module conformance.two_core

@actor
pub struct Home:
    value: u64

    init(mut self):
        self.value = 5

    pub fn get(read self) -> u64:
        return self.value

@actor
pub struct Away:
    n: u32

    init(mut self):
        self.n = 0

    pub fn poke(read self) -> u32:
        return self.n

@test(runtime)
async fn boots(home: Actor[Home]):
    v = await home.get()
    @discard(reason="migrated: deliberate Err discard (M13 item L)")
    match v:
        case .Ok(n):
            assert n == 5, "expected 5"
        case .Err(_):
            assert false, "rejected"

@image
pub fn build() -> Image:
    img = Image(name="two-core-conf", target=Target.wrela_machine_v1, cores=2)
    home = img.actor(Home, mailbox=4, core=0)
    away = img.actor(Away, mailbox=2, core=1)
    img.on_failure(policy=Failure.Halt)
    return img.seal()
"#;

    const SINGLE_CORE_REFUSE_SRC: &str = r#"module conformance.host_refuse

@actor
pub struct Home:
    value: u64

    init(mut self):
        self.value = 1

    pub fn get(read self) -> u64:
        return self.value

@test(runtime)
async fn boots(home: Actor[Home]):
    v = await home.get()
    @discard(reason="migrated: deliberate Err discard (M13 item L)")
    match v:
        case .Ok(n):
            assert n == 1, "expected 1"
        case .Err(_):
            assert false, "rejected"

@image
pub fn build() -> Image:
    img = Image(name="host-refuse", target=Target.wrela_machine_v1, cores=1)
    home = img.actor(Home, mailbox=4, core=0)
    img.on_failure(policy=Failure.Halt)
    return img.seal()
"#;

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn three_cores_come_up_on_a_cross_core_image_over_hvf() {
        let outcome = boot_source(CROSS_CORE_SRC, "c1-three-cores");
        assert_guest_transcript(&outcome.transcript, "test boots: ok\n1 passed, 0 failed\n");
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.core_marks, vec![1, 2, 3]);

        let single = boot_source(
            &CROSS_CORE_SRC
                .replace(", cores=3", "")
                .replace(", core=0", "")
                .replace(", core=1", ""),
            "c1-single-core",
        );
        assert_guest_transcript(&single.transcript, "test boots: ok\n1 passed, 0 failed\n");
        assert_eq!(single.core_marks, vec![0]);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn two_cores_come_up_under_baton_over_hvf() {
        let outcome = boot_source(TWO_CORE_SRC, "f-two-cores");
        assert_guest_transcript(&outcome.transcript, "test boots: ok\n1 passed, 0 failed\n");
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.core_marks, vec![1, 2]);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn concurrent_boot_observes_hv_vcpu_run_overlap_depth() {
        hv::hv_vcpu_run_depth_max_reset();
        assert_eq!(hv::hv_vcpu_run_depth(), 0);
        let outcome = boot_source(TWO_CORE_SRC, "i-overlap-depth");
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.core_marks, vec![1, 2]);
        assert!(
            hv::hv_vcpu_run_depth_max() > 1,
            "expected overlapping hv_vcpu_run (depth_max > 1), got {}",
            hv::hv_vcpu_run_depth_max()
        );
        assert_eq!(
            hv::hv_vcpu_run_depth(),
            0,
            "depth must be quiescent after the boot joins"
        );
    }

    #[test]
    fn host_cores_refuse_names_sealed_n() {
        let err = host_cores_refuse(3, 2, 0xfae9_4005u32 as i32);
        match err {
            VmmError::HostCoresRefuse {
                requested: 3,
                failed_at: 2,
                code,
            } => {
                assert_eq!(code as u32, 0xfae9_4005);
            }
            other => panic!("expected HostCoresRefuse, got {other}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("host refused Cores count=3") && msg.contains("core 2"),
            "{msg}"
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn host_refuses_when_vcpu_create_fails_over_hvf() {
        let _guard = vcpu_create_inject::Guard;
        vcpu_create_inject::arm(0);
        let (image, report) = compile_test_image(SINGLE_CORE_REFUSE_SRC);
        let report = stamp_image_digest(&report, &image.blob);
        let dir = std::env::temp_dir().join(format!(
            "wrela-vmm-host-refuse-{}-{}",
            std::process::id(),
            "f"
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let img_path = dir.join("test.img");
        let report_path = dir.join("test.report.txt");
        std::fs::write(&img_path, &image.blob).expect("img");
        std::fs::write(&report_path, &report).expect("report");
        let err = boot_image(&report_path, &img_path).expect_err("create inject must refuse");
        let _ = std::fs::remove_dir_all(&dir);
        match err {
            VmmError::HostCoresRefuse {
                requested: 1,
                failed_at: 0,
                ..
            } => {}
            other => {
                panic!("expected HostCoresRefuse {{ requested: 1, failed_at: 0 }}, got {other}")
            }
        }
    }

    #[test]
    fn parse_report_accepts_cores_and_high_core_stacks() {
        let n = 2usize;
        let s0 = wrela_machine::layout::core_stack_base_n(0, n);
        let s1 = wrela_machine::layout::core_stack_base_n(1, n);
        let text = format!(
            "Machine revision={}\n\
             Input path=input.wr sha256={}\n\
             Image sha256={}\n\
             Section name=entry base=0x40500000 size=64\n\
             Section name=rtcode base=0x40500100 size=0x200\n\
             Cores count=2\n\
             CoreStack core=0 base={s0:#x} size={:#x}\n\
             CoreStack core=1 base={s1:#x} size={:#x}\n\
             Entry base=0x40500000\n\
             CoreEntry core=1 base=0x40500100\n",
            wrela_machine::MACHINE_REVISION_STR,
            EMPTY_SHA256,
            EMPTY_SHA256,
            wrela_machine::layout::CORE_STACK_SIZE,
            wrela_machine::layout::CORE_STACK_SIZE,
        );
        let parsed = parse_report(&text).expect("Cores+CoreStack");
        assert_eq!(parsed.cores, 2);
        assert_eq!(parsed.core_stacks.len(), 2);
        assert_eq!(parsed.core_stacks[0].base, s0);
        assert_eq!(parsed.core_stacks[1].base, s1);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let tops = core_sp_tops_from_report(&parsed);
            assert_eq!(tops[0], s0 + wrela_machine::layout::CORE_STACK_SIZE);
            assert_eq!(tops[1], s1 + wrela_machine::layout::CORE_STACK_SIZE);
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn check_core_marks_scopes_to_declared_n() {
        use wrela_machine::layout as machine_layout;
        use wrela_machine::machine_info;
        let mut ram = vec![0u8; machine_layout::MACHINE_INFO_SIZE as usize];
        for core in 0..2 {
            let off = (machine_info::core_mark_addr(core) - machine_layout::DRAM_BASE) as usize;
            ram[off..off + 8].copy_from_slice(&machine_info::core_mark_running(core).to_le_bytes());
        }
        check_core_marks(ram.as_ptr(), 2).expect("N=2 marks present");
        let err = check_core_marks(ram.as_ptr(), 3).expect_err("N=3 missing mark");
        let msg = err.to_string();
        assert!(msg.contains("core 2 was released but never ran"), "{msg}");
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn a_miswired_core_entry_fails_the_boot_closed_over_hvf() {
        let (image, report) = compile_test_image(CROSS_CORE_SRC);
        let core1 = image
            .core_entries
            .iter()
            .find(|(c, _)| *c == 1)
            .expect("core 1 entry")
            .1;
        let core2 = image
            .core_entries
            .iter()
            .find(|(c, _)| *c == 2)
            .expect("core 2 entry")
            .1;
        let bad = report.replace(
            &format!("CoreEntry core=2 base={core2:#x}"),
            &format!("CoreEntry core=2 base={core1:#x}"),
        );
        assert!(bad != report, "the report rewrite must have applied");
        let err = parse_report(&bad).expect_err("shared CoreEntry must refuse");
        let msg = err.to_string();
        assert!(
            msg.contains("two cores cannot enter at the same address"),
            "{msg}"
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn a_production_cross_core_image_halts_with_no_runtime_not_a_bring_up_fault() {
        let (image, report) = compile_program_image(CROSS_CORE_SRC);
        assert!(
            !image.core_entries.is_empty(),
            "the program image must still declare its secondary entries — \
             otherwise this test passes for the wrong reason"
        );
        let outcome = boot_blob(&image.blob, &report, "prod-cross-core");
        assert_eq!(
            outcome.exit_code,
            wrela_compiler::layout::EXIT_CODE_NO_RUNTIME,
            "a production image halts with no runtime, on any core count"
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn a_fault_on_a_secondary_core_names_that_core_over_hvf() {
        let (image, report) = compile_test_image(CROSS_CORE_SRC);
        let core1 = image
            .core_entries
            .iter()
            .find(|(c, _)| *c == 1)
            .expect("core 1 entry")
            .1;
        let forged = report
            .lines()
            .find_map(|line| {
                let rest = line.strip_prefix("Section name=entry ")?;
                rest.split_whitespace().find_map(|p| {
                    p.strip_prefix("base=")
                        .map(|b| u64::from_str_radix(b.trim_start_matches("0x"), 16).unwrap())
                })
            })
            .expect("entry section");
        assert_ne!(forged, core1);
        let bad = report.replace(
            &format!("CoreEntry core=1 base={core1:#x}"),
            &format!("CoreEntry core=1 base={forged:#x}"),
        );
        assert!(bad != report, "the report rewrite must have applied");
        let dir = std::env::temp_dir().join(format!("wrela-vmm-c1-fault-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let img_path = dir.join("test.img");
        let report_path = dir.join("test.report.txt");
        std::fs::write(&img_path, &image.blob).expect("write img");
        std::fs::write(&report_path, &bad).expect("write report");
        let err = boot_image(&report_path, &img_path).expect_err("must fail closed");
        let _ = std::fs::remove_dir_all(&dir);
        let msg = err.to_string();
        assert!(
            msg.contains("core 1 was released but never ran its own entry block")
                || (msg.contains("core 1:") && msg.contains("unhandled exception")),
            "{msg}"
        );
    }

    #[test]
    fn parse_report_rejects_malformed_core_entries() {
        let head = format!(
            "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
             Section name=entry base=0x40500000 size=64\n\
             Section name=rtcode base=0x40500100 size=0x200\n\
             Entry base=0x40500000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        for (why, line) in [
            (
                "core 0 is the `Entry base=` line",
                "CoreEntry core=0 base=0x40500100\n",
            ),
            (
                "a core outside the contiguous secondary set",
                "CoreEntry core=3 base=0x40500100\n",
            ),
            (
                "a gap in the core set",
                "CoreEntry core=2 base=0x40500100\n",
            ),
            ("a missing base", "CoreEntry core=1\n"),
            (
                "an unknown field",
                "CoreEntry core=1 base=0x40500100 stack=0x1\n",
            ),
        ] {
            let text = format!("{head}{line}");
            assert!(
                matches!(parse_report(&text), Err(VmmError::MalformedReport(_))),
                "{why} must be refused"
            );
        }
        let ok =
            format!("{head}CoreEntry core=2 base=0x40500200\nCoreEntry core=1 base=0x40500100\n");
        let parsed = parse_report(&ok).expect("parses");
        assert_eq!(
            parsed.core_entries,
            vec![
                CoreEntry {
                    core: 1,
                    base: 0x40500100
                },
                CoreEntry {
                    core: 2,
                    base: 0x40500200
                },
            ]
        );
    }

    #[test]
    fn parse_report_reads_request_rings_and_refuses_malformed_ones() {
        let head = format!(
            "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
             Section name=entry base=0x40500000 size=64\n\
             Section name=rtcode base=0x40500100 size=0x200\n\
             Section name=rtdata base=0x40501000 size=0x4000\n\
             Entry base=0x40500000\nCoreEntry core=1 base=0x40500100\n\
             CoreEntry core=2 base=0x40500200\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        for (why, line) in [
            (
                "a same-core `ring` is not a cross-core edge",
                "Ring kind=request src=0 dst=0 target=A cap=4 slot=16 bytes=88 base=0x40501000\n",
            ),
            (
                "a core this machine does not have",
                "Ring kind=request src=0 dst=7 target=A cap=4 slot=16 bytes=88 base=0x40501000\n",
            ),
            (
                "a lane that is neither request nor reply",
                "Ring kind=gossip src=0 dst=1 target=A cap=4 slot=16 bytes=88 base=0x40501000\n",
            ),
            (
                "a request ring with no target mailbox",
                "Ring kind=request src=0 dst=1 target=- cap=4 slot=16 bytes=88 base=0x40501000\n",
            ),
            (
                "a ring with no address to witness at",
                "Ring kind=request src=0 dst=1 target=A cap=4 slot=16 bytes=88\n",
            ),
            (
                "an unknown field",
                "Ring kind=request src=0 dst=1 target=A cap=4 slot=16 bytes=88 base=0x1 depth=2\n",
            ),
            (
                "an inconsistent cap/slot/bytes triple",
                "Ring kind=request src=0 dst=1 target=A cap=4 slot=16 bytes=99 base=0x40501000\n",
            ),
        ] {
            let text = format!("{head}{line}");
            let err = parse_report(&text).expect_err(why);
            assert!(
                matches!(err, VmmError::MalformedReport(_)),
                "{why} must be refused: {err}"
            );
            if why.contains("cap/slot/bytes") {
                let msg = err.to_string();
                assert!(
                    msg.contains("bytes must equal cap*slot+") || msg.contains("forged triple"),
                    "{why}: {msg}"
                );
            }
        }
        let single_core = format!(
            "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
             Section name=entry base=0x40500000 size=64\n\
             Entry base=0x40500000\n\
             Ring kind=request src=0 dst=1 target=A cap=4 slot=16 bytes=88 base=0x40501000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        assert!(matches!(
            parse_report(&single_core),
            Err(VmmError::MalformedReport(_))
        ));

        let ok = format!(
            "{head}\
             Ring kind=request src=0 dst=1 target=Sink cap=8 slot=24 bytes=216 base=0x40502cb8\n\
             Ring kind=reply src=1 dst=0 target=- cap=2 slot=16 bytes=56 base=0x40502ec0\n\
             Ring kind=request src=2 dst=1 target=Sink cap=8 slot=24 bytes=216 base=0x40502de8\n"
        );
        let parsed = parse_report(&ok).expect("parses");
        let got: Vec<(usize, usize, &str, u64)> = parsed
            .request_rings
            .iter()
            .map(|r| (r.src, r.dst, r.target.as_str(), r.count_addr))
            .collect();
        assert_eq!(
            got,
            vec![
                (0, 1, "Sink", 0x40502cb8 + 8 * 24 + 16),
                (2, 1, "Sink", 0x40502de8 + 8 * 24 + 16),
            ]
        );
    }

    #[test]
    fn parse_report_refuses_placement_forgeries() {
        let head = format!(
            "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
             Section name=entry base=0x40500000 size=64\n\
             Section name=code base=0x40500040 size=0xc0\n\
             Section name=rtcode base=0x40500100 size=0x200\n\
             Section name=rtdata base=0x40501000 size=0x1000\n\
             Entry base=0x40500000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        let cores = "CoreEntry core=1 base=0x40500100\nCoreEntry core=2 base=0x40500200\n";

        {
            let text = format!(
                "{head}CoreEntry core=1 base=0x40501040\nCoreEntry core=2 base=0x40500200\n"
            );
            let err = parse_report(&text).expect_err("CoreEntry in rtdata");
            let msg = err.to_string();
            assert!(
                msg.contains("falls inside `Section name=rtdata`") && msg.contains("must be code"),
                "{msg}"
            );
        }
        {
            let text =
                format!("{head}CoreEntry core=1 base=0x1000\nCoreEntry core=2 base=0x40500200\n");
            let err = parse_report(&text).expect_err("CoreEntry outside sections");
            let msg = err.to_string();
            assert!(
                msg.contains("outside every `Section`") && msg.contains("must be code"),
                "{msg}"
            );
        }
        {
            let text = format!(
                "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
                 Section name=entry base=0x40500000 size=64\n\
                 Section name=rtdata base=0x40500100 size=0x200\n\
                 Entry base=0x40500000\n\
                 CoreEntry core=1 base=0x40500100\n",
                wrela_machine::MACHINE_REVISION_STR
            );
            let err = parse_report(&text).expect_err("CoreEntry only in rtdata");
            assert!(
                err.to_string()
                    .contains("falls inside `Section name=rtdata`"),
                "{err}"
            );
        }
        {
            let text = format!(
                "{head}{cores}\
                 Ring kind=request src=0 dst=1 target=A cap=4 slot=16 bytes=88 base=0x40501000\n\
                 Ring kind=request src=2 dst=1 target=A cap=4 slot=16 bytes=88 base=0x40501020\n"
            );
            let err = parse_report(&text).expect_err("overlapping rings");
            assert!(err.to_string().contains("overlaps"), "{err}");
        }
        {
            let stack = wrela_machine::layout::core_stack_base_n(1, 3);
            let text = format!(
                "{head}{cores}\
                 Ring kind=request src=0 dst=1 target=A cap=4 slot=16 bytes=88 base={stack:#x}\n"
            );
            let err = parse_report(&text).expect_err("ring on stack");
            let msg = err.to_string();
            assert!(
                msg.contains("overlaps") && msg.contains("core 1's stack"),
                "{msg}"
            );
        }
        {
            let text = format!(
                "{head}\
                 CoreEntry core=1 base=0x40500100\n\
                 Actor index=0 type=Sink\n\
                 Placement id=actor#0 type=Sink core=2 source=explicit work=0 work_source=unproved \
                 bytes=1 bytes_state=1 bytes_mailbox=0 bytes_pool=0\n"
            );
            let err = parse_report(&text).expect_err("Placement core >= N");
            let msg = err.to_string();
            assert!(
                msg.contains("Placement id=actor#0 core=2")
                    && msg.contains("core index must be < 2")
                    && msg.contains("Cores count=2"),
                "{msg}"
            );
        }
        {
            let text = format!(
                "{head}{cores}\
                 Actor index=0 type=Sink\n\
                 Placement id=actor#9 type=Ghost core=1 source=explicit work=0 work_source=unproved \
                 bytes=1 bytes_state=1 bytes_mailbox=0 bytes_pool=0\n"
            );
            let err = parse_report(&text).expect_err("Placement of unknown actor");
            let msg = err.to_string();
            assert!(
                msg.contains("Placement id=actor#9") && msg.contains("Actor` lines do not declare"),
                "{msg}"
            );
        }
        {
            let text = format!(
                "{head}CoreEntry core=1 base=0x40500101\nCoreEntry core=2 base=0x40500200\n"
            );
            let err = parse_report(&text).expect_err("unaligned CoreEntry");
            assert!(err.to_string().contains("is not 4-byte aligned"), "{err}");
        }
        {
            let text = format!(
                "{head}CoreEntry core=1 base=0x40500100\nCoreEntry core=2 base=0x40500100\n"
            );
            let err = parse_report(&text).expect_err("shared CoreEntry base");
            assert!(
                err.to_string()
                    .contains("two cores cannot enter at the same address"),
                "{err}"
            );
        }
        {
            let text = format!(
                "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
                 Section name=code base=0x40500050 size=8600\n\
                 Section name=rtcode base=0x40500100 size=0x200\n\
                 Entry base=0x40500000\n\
                 CoreEntry core=1 base=0x40500100\n",
                wrela_machine::MACHINE_REVISION_STR
            );
            let err = parse_report(&text).expect_err("overlapping sections");
            assert!(err.to_string().contains("overlaps `Section name="), "{err}");
        }
        {
            let text = format!(
                "{head}{cores}\
                 Actor index=0 type=Sink\n\
                 Placement id=actor#0 type=Sink core=1 source=explicit work=0 work_source=unproved \
                 bytes=1 bytes_state=1 bytes_mailbox=0 bytes_pool=0\n\
                 Placement id=actor#0 type=Sink core=2 source=explicit work=0 work_source=unproved \
                 bytes=1 bytes_state=1 bytes_mailbox=0 bytes_pool=0\n"
            );
            let err = parse_report(&text).expect_err("duplicate Placement id");
            assert!(
                err.to_string()
                    .contains("Placement id=actor#0` is repeated"),
                "{err}"
            );
        }
        {
            let text = format!(
                "{head}{cores}\
                 Actor index=0 type=Sink\n\
                 Placement id=actor#0 type=Ghost core=1 source=explicit work=0 work_source=unproved \
                 bytes=1 bytes_state=1 bytes_mailbox=0 bytes_pool=0\n"
            );
            let err = parse_report(&text).expect_err("Placement type mismatch");
            assert!(
                err.to_string().contains("disagrees with the declared root"),
                "{err}"
            );
        }
        {
            let text = format!(
                "{head}{cores}\
                 Actor index=0 type=Sink\n\
                 Actor index=1 type=Near\n\
                 Placement id=actor#0 type=Sink core=1 source=explicit work=0 work_source=unproved \
                 bytes=1 bytes_state=1 bytes_mailbox=0 bytes_pool=0\n"
            );
            let err = parse_report(&text).expect_err("missing Placement");
            assert!(err.to_string().contains("has no `Placement` line"), "{err}");
        }
        {
            let text = format!(
                "{head}{cores}\
                 Actor index=0 type=Sink\n\
                 Actor index=1 type=Near\n\
                 Placement id=actor#0 type=Sink core=1 source=explicit work=0 work_source=unproved \
                 bytes=1 bytes_state=1 bytes_mailbox=0 bytes_pool=0\n\
                 Placement id=actor#1 type=Near core=0 source=explicit work=0 work_source=unproved \
                 bytes=1 bytes_state=1 bytes_mailbox=0 bytes_pool=0\n\
                 Ring kind=request src=0 dst=1 target=Sink cap=4 slot=16 bytes=88 base=0x40501000\n"
            );
            parse_report(&text).expect("well-formed Placement set must parse");
        }
        {
            let text = format!(
                "{head}{cores}\
                 Actor index=0 type=Sink\n\
                 Placement id=actor#0 type=Sink core=1 source=explicit work=0 work_source=unproved \
                 bytes=1 bytes_state=1 bytes_mailbox=0 bytes_pool=0\n\
                 Ring kind=request src=0 dst=1 target=Ghost cap=4 slot=16 bytes=88 base=0x40501000\n"
            );
            let err = parse_report(&text).expect_err("ring target names no declared root");
            let msg = err.to_string();
            assert!(
                msg.contains("names a root this report never declares")
                    && msg.contains("known roots: Sink"),
                "{msg}"
            );
        }
        {
            let text = format!(
                "{head}{cores}\
                 Actor index=0 type=Sink\n\
                 Placement id=actor#0 type=Sink core=1 source=explicit work=0 work_source=unproved \
                 bytes=1 bytes_state=1 bytes_mailbox=0 bytes_pool=0\n\
                 Ring kind=request src=0 dst=1 target=Sink cap=4 slot=16 bytes=88 base=0x40501000\n\
                 Ring kind=reply src=1 dst=0 target=- cap=1 slot=16 bytes=40 base=0x40501100\n"
            );
            parse_report(&text).expect("a reply ring's `target=-` is not a root name");
        }
    }

    #[test]
    fn admission_witness_counts_only_the_running_core_s_own_drain() {
        let ring = |src: usize, dst: usize, target: &str, capacity: u64| RequestRing {
            src,
            dst,
            target: target.to_string(),
            data_base: 0,
            count_addr: 0,
            capacity,
        };
        let mut w = AdmissionWitness::new(vec![
            ring(0, 1, "Sink", 8),
            ring(0, 2, "Far", 4),
            ring(2, 1, "Sink", 8),
        ]);
        assert_eq!(
            w.observe(&[2, 1, 0], &[0, 0, 0], 0).expect("ok"),
            Vec::new()
        );
        assert_eq!(
            w.observe(&[2, 0, 1], &[0, 1, 0], 2).expect("ok"),
            vec![("Far".to_string(), "core0".to_string())]
        );
        assert_eq!(
            w.observe(&[0, 0, 0], &[2, 1, 1], 1).expect("ok"),
            vec![
                ("Sink".to_string(), "core0".to_string()),
                ("Sink".to_string(), "core0".to_string()),
                ("Sink".to_string(), "core2".to_string()),
            ]
        );
        assert_eq!(
            w.observe(&[0, 0, 0], &[2, 1, 1], 1).expect("ok"),
            Vec::new()
        );
    }

    #[test]
    fn admission_witness_survives_a_hostile_ring_head() {
        let mut w = AdmissionWitness::new(vec![RequestRing {
            src: 0,
            dst: 1,
            target: "Sink".to_string(),
            data_base: 0,
            count_addr: 0,
            capacity: 8,
        }]);
        let admitted = w.observe(&[1], &[u64::MAX], 1).expect("must not panic");
        assert!(
            admitted.len() <= 8,
            "a ring of 8 slots cannot admit {} messages",
            admitted.len()
        );
        let next = u64::MAX.wrapping_add(1);
        let admitted = w.observe(&[1], &[next], 1).expect("must not panic");
        assert_eq!(admitted.len(), 1, "one head step is one admission");
    }

    #[test]
    fn admission_witness_clamps_a_hostile_count_shrink() {
        let mut w = AdmissionWitness::new(vec![RequestRing {
            src: 0,
            dst: 1,
            target: "Sink".to_string(),
            data_base: 0,
            count_addr: 0,
            capacity: 1,
        }]);
        let _ = w.observe(&[u64::MAX], &[0], 1).expect("must not panic");
        let admitted = w.observe(&[0], &[0], 1).expect("must not panic");
        assert!(
            admitted.len() <= 1,
            "a cap-1 ring cannot admit {} messages in one exit",
            admitted.len()
        );
    }

    #[test]
    fn admission_witness_rejects_a_short_observation() {
        let mut w = AdmissionWitness::new(vec![RequestRing {
            src: 0,
            dst: 1,
            target: "Sink".to_string(),
            data_base: 0,
            count_addr: 0,
            capacity: 8,
        }]);
        let err = w
            .observe(&[], &[], 1)
            .expect_err("length mismatch must fail closed");
        assert!(err.contains("ring(s) declared"), "got {err}");
    }

    #[test]
    fn admission_witness_absorbs_growth_on_a_consumed_ring() {
        let mut w = AdmissionWitness::new(vec![RequestRing {
            src: 0,
            dst: 1,
            target: "Sink".to_string(),
            data_base: 0,
            count_addr: 0,
            capacity: 8,
        }]);
        assert_eq!(w.observe(&[3], &[0], 1).expect("ok"), Vec::new());
        assert_eq!(
            w.observe(&[1], &[2], 1).expect("ok"),
            vec![
                ("Sink".to_string(), "core0".to_string()),
                ("Sink".to_string(), "core0".to_string()),
            ]
        );
    }

    #[test]
    fn parse_report_without_blk_lines_declares_no_device() {
        let text = format!(
            "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nSection name=entry base=0x40500000 size=1\nEntry base=0x40500000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        assert!(parse_report(&text).expect("parses").blk.is_none());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[ignore = "HVF lane: run via `cargo xtask verify-deep`"]
    #[test]
    fn block_count_lane2_agrees_with_host_dram_on_boot_actors() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("crates/wrela-vmm → repo root");
        let src_path = repo_root.join("tests/golden/boot-actors/input.wr");
        let src = std::fs::read_to_string(&src_path).unwrap_or_else(|e| {
            panic!("read {}: {e}", src_path.display());
        });

        wrela_compiler::codegen::set_block_count(true);
        let (image, report) = compile_test_image(&src);
        wrela_compiler::codegen::set_block_count(false);

        let outcome = boot_blob(&image.blob, &report, "lane3-boot-actors");
        assert_eq!(
            outcome.exit_code,
            0,
            "boot-actors under --block-count must exit 0; transcript:\n{}",
            String::from_utf8_lossy(&outcome.transcript)
        );
        let transcript = String::from_utf8_lossy(&outcome.transcript);
        crate::lane3::agree_lane2_vs_host(&transcript, &outcome.lane2_hits).unwrap_or_else(|e| {
            panic!("{e}\nfull transcript:\n{transcript}");
        });
        assert!(
            !outcome.lane2_hits.is_empty(),
            "Lane 3 hit map must be non-empty on boot-actors"
        );

        let line = transcript
            .lines()
            .find(|l| l.starts_with("lane2 hits="))
            .expect("lane2 line");
        let parsed = crate::lane3::parse_lane2_line(line).expect("parse");
        assert!(
            outcome.lane2_hits.len() > wrela_compiler::rtconfig::BLOCK_BOUND_PRINT_PAIRS,
            "boot-actors must exceed the printable pair cap for this oracle to test \
             truncation at all (host pairs: {})",
            outcome.lane2_hits.len()
        );
        assert_eq!(
            parsed.hits.len(),
            wrela_compiler::rtconfig::BLOCK_BOUND_PRINT_PAIRS,
            "a truncating dump must print exactly the cap"
        );
        assert_eq!(
            parsed.truncated,
            Some(
                (outcome.lane2_hits.len() - wrela_compiler::rtconfig::BLOCK_BOUND_PRINT_PAIRS)
                    as u64
            ),
            "`truncated=<N>` must name every dropped pair"
        );
        assert!(
            parsed.truncated.unwrap() > 0,
            "the marker must name a nonzero count on a truncating case"
        );
    }
}
