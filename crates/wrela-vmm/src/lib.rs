//! The wrela VMM: the product implementation of the wrela machine
//! (docs/language/06-machine.md). Firecracker-class, userspace, two host
//! backends behind one internal seam:
//!
//!   - `kvm` (Linux): the Raspberry Pi 5 flagship host (unimplemented until
//!     the Pi milestone).
//!   - `hv`/`boot_image` (macOS / Hypervisor.framework): development and
//!     Mac hosts — **live** as of plans/M5.md item E.
//!
//! The VMM consumes the compiler's own emitted image + report as its
//! entire configuration (06 §3): `boot_image` reads both, validates the
//! machine revision and the report's own structural shape, loads the
//! image at the fixed base, zeroes the declared reservations (the whole
//! guest DRAM allocation is zeroed up front — see `boot_image`'s own doc
//! comment for why this is load-bearing, not merely tidy), points `x0` at
//! the machine-info page, and starts vCPU 0 at the image's own entry.
//! Devices at M5 are exactly two: the console tx ring (decision 12,
//! drained once, after halt) and the clock MMIO trap (decision 13, logged
//! every read). Everything else fails closed.

use std::time::Duration;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod hv;

/// Host-side wall-clock cap per boot (plans/M5.md decision 15): a guest
/// stuck with no exits at all (an infinite loop between checkpoints) is
/// force-exited via `hv_vcpus_exit` from a watchdog thread — a hang is
/// reported as `VmmError::Timeout`, transcript-so-far included, never a
/// silent stall.
pub const WALL_CAP: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum VmmError {
    /// The requested capability is not implemented yet (a non-macOS/
    /// non-aarch64 host at M5 — the flagship KVM backend is a later
    /// milestone). Fail closed, never a silent skip.
    Unsupported(&'static str),
    /// The report's own `Machine revision=` line does not name this
    /// build's `wrela_machine::MACHINE_REVISION_STR` (06 §10: "the VMM
    /// refuses an image built for another revision").
    MachineRevisionMismatch { report: String, vmm: &'static str },
    /// The report text is missing a structural fact this VMM's whole
    /// configuration depends on (no `Machine revision=` line at all, no
    /// `Input path=` digest line, no `Section name=`/`Entry base=` line) —
    /// 06 §3's "the VMM reads the sealed image and its report, validates
    /// digests" half, at the presence-check granularity this milestone's
    /// report format actually carries (the VMM has no access to the
    /// original source files to re-hash against; only the compiler does).
    MalformedReport(String),
    /// A file (`report_path`/`img_path`) could not be read.
    Io(String),
    /// A raw Hypervisor.framework call returned a non-`HV_SUCCESS` code.
    Hvf { call: &'static str, code: i32 },
    /// The image does not fit the declared DRAM reservation, or a section
    /// address the report claims is inconsistent with the machine's own
    /// fixed layout contract — an internal-consistency failure, never an
    /// ordinary boot outcome.
    BadImage(String),
    /// The guest trapped in a way this VMM has no handler for: an
    /// unexpected MMIO address, a non-load/store instruction shape at an
    /// MMIO address (`ISV == 0`), a bare `BRK`, or any other exception
    /// class/exit reason — reported with the guest's own PC and the raw
    /// `ESR` for a post-mortem.
    GuestFault(String),
    /// `--replay` found a determinism disagreement mid-boot (strict
    /// chooser abort: choice-log underrun/overrun/tag mismatch, etc.).
    /// Distinct from [`VmmError::GuestFault`]: the process exit contract
    /// maps this to `EXIT_REPLAY_DIVERGENCE` (3), never `EXIT_VMM_FAILURE`
    /// (2) — a caller checking `$?` alone must never confuse a
    /// determinism finding with a boot that never produced an answer.
    ReplayDivergence(String),
    /// The host-side wall-clock cap (`WALL_CAP`) elapsed with the guest
    /// still running; `transcript_so_far` is whatever the console ring
    /// held at the moment of the forced exit (decision 15: "the
    /// transcript-so-far shown", never silently discarded). `core` is the
    /// vCPU that was actually inside `hv_vcpu_run` when the watchdog
    /// force-exited every core (plans/M8.md item C1: with three cores, a
    /// hang that does not say *which* core hung is a bug report missing
    /// its first fact).
    Timeout {
        core: usize,
        transcript_so_far: Vec<u8>,
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
        }
    }
}

impl std::error::Error for VmmError {}

/// The whole result of one boot (plans/M5.md item E's own public API,
/// `boot_image`'s return value): the captured console transcript, the
/// guest's own reported exit code (`0`/`1`, `machine_info::OFF_EXIT_CODE`'s
/// own convention, mirrored via the trapping `EXIT_MMIO_ADDR` store), the
/// ordered log of every clock read's returned value (06 §8's own
/// record-boundary subset; empty at M5 since nothing in the generated
/// runtime issues one yet — see layout.rs's own module doc), and the total
/// vCPU exit count (a `bench guest`/`profile` fact, item F).
#[derive(Debug, Clone, Default)]
pub struct BootOutcome {
    pub transcript: Vec<u8>,
    pub exit_code: u64,
    /// plans/M6.md item E: the whole ordered choice sequence this boot
    /// resolved (decision 9) — clock reads, deadline wakes, and vector
    /// raises alike, in the order `Chooser::choose_next` (`record.rs`)
    /// saw them. M5's own `clock_log: Vec<u64>` is exactly the
    /// `ChoiceEntry::ClockRead` subsequence of this, now generalized.
    pub choices: Vec<record::ChoiceEntry>,
    pub exits: u64,
    /// plans/M8.md item C1: each core's own guest-written bring-up mark
    /// (`machine_info::OFF_CORE_MARK`), `VCPUS` of them, in core order —
    /// `core_mark_running(n)` for a core that reached its own event loop,
    /// `0` for one that never ran. A single-core image leaves all three at
    /// `0` (it releases nothing and writes no mark); `check_core_marks`
    /// has already refused any boot where a *released* core is missing its
    /// own mark, so this field is evidence for a test to read, never a
    /// condition a caller has to remember to check.
    pub core_marks: Vec<u64>,
}

/// Re-exports of the shared image-report schema (`wrela_machine::report`).
/// Parse logic lives in the machine crate; this module keeps VMM-specific
/// digest checks, W^X, and boot wiring.
pub use wrela_machine::report::{
    BlkConfig, BlkQueueConfig, CoreEntry, EMPTY_SHA256, IrqHostInject, ParsedReport, PoolWindow,
    ReportSection, RequestRing,
};

/// Parse the VMM-facing report text, mapping machine `String` errors into
/// `VmmError` (including the distinct machine-revision mismatch variant).
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

/// 06 §3: re-check the sealed image blob and every readable `Input` file
/// against the digests the report declares. An unreadably-named input
/// (unit-test placeholders like `<conformance>`) is skipped — presence of
/// a well-formed digest is still required at parse time.
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
    Ok(())
}

/// Host offset into the DRAM reservation for a guest-physical range, or a
/// `BadImage`/`MalformedReport` when the range is not wholly inside
/// `[DRAM_BASE, DRAM_BASE + DRAM_SIZE)`. Callers that only have a point
/// use `nbytes = 1` (or the access width).
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
mod exit_loop;

pub use boot::boot_image;
pub(crate) use boot::boot_image_core;
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) use boot::boot_image_core_with_delayed_raise;

#[cfg(test)]
pub(crate) use exit_loop::{
    AdmissionWitness, check_vector_in_range, drain_console, raise_vector, read_core_mark,
};

#[cfg(target_os = "linux")]
pub mod kvm {
    //! Unimplemented until the Raspberry Pi flagship host milestone.
    //! Hardcoded KVM backend when it lands — no rust-vmm dependency.
}

pub mod devices;

pub mod record;

#[cfg(test)]
mod tests {
    use super::*;

    /// VMM-facing report identity lines for unit fixtures. `Image sha256=`
    /// hashes `img` when provided; parse-only fixtures pass `&[]` and get
    /// the empty digest.
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

    /// The M5-G adversarial-sweep find/fix, at `drain_console`'s own
    /// level: `wrela-compiler/src/layout.rs`'s module doc has the whole
    /// story, but the bug's *observable* shape was always here — a
    /// transcript silently truncated once `console::QUEUE_SIZE`
    /// descriptors were spent. `drain_console`'s own `count =
    /// avail_idx.min(console::QUEUE_SIZE)` line never itself hard-coded
    /// the old `16`, so no code here needed to change for the fix — but
    /// nothing golden-covered ever exercised more than a handful of
    /// descriptors either, so this proves the parser genuinely reads
    /// past the *old* bound (20 > 16) now that `QUEUE_SIZE` is 256: a
    /// synthetic guest-RAM buffer (a plain heap `Vec<u8>`, not a real
    /// mmap — `drain_console` only ever does pointer-offset reads, no
    /// page-fault-timing subtlety like `layout.rs`'s own JIT self-tests
    /// need) with 20 one-byte descriptors published directly (no VMM,
    /// no HVF, no guest code at all — purely `drain_console`'s own
    /// parsing).
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn drain_console_reads_more_than_the_old_16_descriptor_limit() {
        use wrela_machine::{console, layout as machine_layout};

        let buf_len =
            (console::DATA_BASE + console::DATA_SIZE - machine_layout::DRAM_BASE) as usize + 64;
        let mut buf = vec![0u8; buf_len];

        let ring_off = (console::RING_BASE - machine_layout::DRAM_BASE) as usize;
        let data_off = (console::DATA_BASE - machine_layout::DRAM_BASE) as usize;

        let n: usize = 20; // > the old QUEUE_SIZE == 16
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

    /// plans/M5.md item F: "a hand-built image in wrela-vmm's tests that
    /// DOES read CLOCK_MMIO twice" — this milestone's only exerciser of
    /// the clock trap and the whole `record`/`replay` machinery, since no
    /// real `@test(runtime)` program can issue a clock read yet
    /// (`machine.clock.trap-logged`'s own gap note). A ~12-word
    /// hand-assembled guest program, reusing `wrela-compiler::encode`'s
    /// own pinned A76 encodings (this crate's one test-only dependency —
    /// see `Cargo.toml`'s own comment) rather than re-deriving the same
    /// bit patterns a second time by hand:
    ///
    /// ```text
    /// movz x9,  #lo16(CLOCK_MMIO_ADDR)
    /// movk x9,  #bits[16:31](CLOCK_MMIO_ADDR), lsl #16
    /// movk x9,  #0, lsl #32
    /// movk x9,  #0, lsl #48
    /// ldr  x0,  [x9]                 ; clock read #1 (discarded)
    /// ldr  x1,  [x9]                 ; clock read #2 (discarded)
    /// movz x2,  #0                   ; exit code = 0
    /// movz x10, #lo16(EXIT_MMIO_ADDR)
    /// movk x10, #bits[16:31](EXIT_MMIO_ADDR), lsl #16
    /// movk x10, #0, lsl #32
    /// movk x10, #0, lsl #48
    /// str  x2,  [x10]                ; trapping store: guest is done
    /// ```
    ///
    /// One `#[test]` fn, not several: every real boot below happens
    /// sequentially within it (record once, then five replays reusing the
    /// same recording/image), so no two calls into Hypervisor.framework's
    /// one process-wide VM context are ever concurrent — the same reason
    /// `boot_image`'s own `VmGuard`/`VcpuGuard` tear down fully on every
    /// return path, `Drop`-ordered, before the next call in this fn can
    /// begin.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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

        // --- record: one live boot -------------------------------------
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

        // --- replay with the real recording: no divergence --------------
        let divergences = record::replay(&report_path, &img_path, &recorded).expect("replay boot");
        assert!(
            divergences.is_empty(),
            "expected no divergence, got {divergences:?}"
        );

        // --- tampered transcript digest: caught --------------------------
        let mut bad_digest = recorded.clone();
        bad_digest.transcript_digest = "not-the-real-digest".to_string();
        let divergences =
            record::replay(&report_path, &img_path, &bad_digest).expect("replay boot");
        assert!(matches!(
            divergences.as_slice(),
            [record::Divergence::TranscriptDigestMismatch { .. }]
        ));

        // --- tampered exit code: caught -----------------------------------
        let mut bad_exit = recorded.clone();
        bad_exit.exit_code = 99;
        let divergences = record::replay(&report_path, &img_path, &bad_exit).expect("replay boot");
        assert!(divergences.contains(&record::Divergence::ExitCodeMismatch {
            expected: 99,
            actual: 0,
        }));

        // --- truncated choice log: an underrun aborts (strict replay) ------
        let mut short_log = recorded.clone();
        short_log.choices.truncate(1);
        let err = record::replay(&report_path, &img_path, &short_log).expect_err("strict underrun");
        assert!(err.to_string().contains("replay divergence"), "got {err}");

        // --- padded choice log: an overrun aborts (strict replay) ----------
        let mut long_log = recorded.clone();
        long_log
            .choices
            .push(record::ChoiceEntry::ClockRead { value: 424242 });
        let err = record::replay(&report_path, &img_path, &long_log).expect_err("strict overrun");
        assert!(err.to_string().contains("replay divergence"), "got {err}");

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    // =======================================================================
    // plans/M6.md item C: the guest runtime core, conformance-tested the
    // M5-E way — real HVF boots of hand-assembled guest programs. M11 J
    // deleted the enqueue/select hand-built HVF oracles with the
    // ImageStatic emitters; boot transcripts own that surface now.

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

    /// Writes `img_bytes` to disk under `tag` and returns the
    /// `(report_path, img_path)` pair, without booting — shared by every
    /// test below that needs to call `boot_image_core`/`record::record`/
    /// `record::replay` directly (rather than through the plain
    /// `boot_image` wrapper `boot_hand_built_image` above uses).
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

    /// Rounds `n` up to the next multiple of 8 — every `ActorAddrs` field
    /// this item's own tests place is a `u64`, so the `rtdata` region's
    /// own base must be 8-byte aligned (an unaligned 64-bit `LDR`/`STR`
    /// can fault): a real image already gets this via `layout.rs`'s own
    /// `round_up(cursor, 8)`; this test harness needs the identical
    /// rounding since it lays out its own hand-built blob rather than
    /// going through `layout_program`/`layout_test_image`.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn round_up8(n: u64) -> u64 {
        n.div_ceil(8) * 8
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

    /// Pre-M11-I empty irq/wake checkpoint block (vector0 observed++ /
    /// pending whole-word clear), frozen for the HVF conformance guest.
    /// Production images use the floor trampoline + wrela body instead.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn hand_built_simple_checkpoint() -> (Vec<u32>, usize) {
        use wrela_compiler::encode;
        use wrela_machine::{layout as machine_layout, machine_info, pending};
        let mut w = Vec::new();
        let observed = machine_layout::MACHINE_INFO_BASE + machine_info::OFF_VECTOR0_OBSERVED;
        let pending_addr = pending::core_word_addr(0);
        // vector0 @ 0
        w.extend(load_imm_words(9, observed));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        w.push(encode::enc_add_imm(10, 10, 1, true));
        w.push(encode::enc_str_x_imm(10, 9, 0));
        w.push(encode::enc_ret(30));
        let service = w.len();
        // floor cat2 save
        w.push(encode::enc_sub_imm(31, 31, 16, true));
        w.push(encode::enc_str_x_imm(30, 31, 0));
        let loop_top = w.len();
        w.extend(load_imm_words(9, pending_addr));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        let cbz_at = w.len();
        w.push(0); // cbz placeholder
        let bl_at = w.len();
        w.push(encode::enc_bl(((0isize - bl_at as isize) * 4) as i32)); // → vector0
        w.extend(load_imm_words(9, pending_addr));
        w.push(encode::enc_str_x_imm(31, 9, 0)); // str xzr — x31 encodes zr in ldr/str
        // Actually str xzr uses rt=31; enc_str_x_imm(31, ...) is xz. Good.
        let b_at = w.len();
        w.push(encode::enc_b(((loop_top as i64 - b_at as i64) * 4) as i32));
        let done = w.len();
        w[cbz_at] = encode::enc_cbz(10, ((done as i64 - cbz_at as i64) * 4) as i32, true);
        // floor cat2 restore
        w.push(encode::enc_ldr_x_imm(30, 31, 0));
        w.push(encode::enc_add_imm(31, 31, 16, true));
        w.push(encode::enc_ret(30));
        (w, service)
    }

    /// `x_reg` already holds an actual value; compares against `expect`,
    /// sets `scratch` to `1` if they differ, shifts it into `bit` (a power
    /// of two, `1 << shift`), and ORs it into `acc` — a branch-free
    /// "assert" this test's own entry sequence composes one call per
    /// checked fact, entirely so the boot's own single observable exit
    /// code (`BootOutcome::exit_code`) can carry every check's own
    /// pass/fail bit at once, since this hand-built harness has no
    /// console/report machinery of its own to print through (unlike a
    /// real compiled program's `@test(runtime)` — this fn's own module
    /// doc explains why no compiled program can reach this code yet).
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

    // M11 J: hand-built HVF enqueue/select oracles deleted with
    // `build_rt_enqueue` / `build_rt_select_and_run` (emitters gone).
    // Boot transcripts (`boot-*/expected/test.txt`) are the oracle
    // (decision 705 / 837).

    // =======================================================================
    // Park-and-resume conformance (the M6 turn-suspension mandate): real
    // `.wr` sources compiled through the identical pipeline
    // `bin/wrela.rs::test_cmd` runs (sema -> image graph -> mwir + FlowWir
    // -> codegen -> `layout_test_image`), booted for real on
    // Hypervisor.framework, transcript asserted. These are the semantic
    // witnesses 04-compiler.md §2 demands structurally: awaiting lets ALL
    // ready actors run; one turn owns an actor until completion; FIFO per
    // mailbox; replies land in the awaiting turn's own reply slot; the
    // root test turn parks/resumes through the same machinery; and
    // nothing-ready + root-incomplete aborts with the named deadlock
    // diagnostic.

    /// Compiles `src` exactly the way `test_cmd` does and returns the
    /// laid-out image + the report text `boot_image` needs. `patch_rtdata`
    /// lets the deadlock test corrupt the (normally all-zero) rtdata
    /// bytes before boot — a hand-built table state, disclosed as such.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn compile_test_image(src: &str) -> (wrela_compiler::layout::ImageLayout, String) {
        use std::collections::{BTreeMap, BTreeSet};
        use wrela_compiler::sema::typed::TestKind;
        use wrela_compiler::sema::types::{Type, TypeArg};
        use wrela_compiler::{layout, loader};

        let tokens = wrela_compiler::syntax::lexer::lex(src).expect("conformance source must lex");
        let module = wrela_compiler::syntax::parser::parse(tokens).expect("must parse");
        // M10 B2 / B3 / A2d: mirror `bin/wrela.rs::test_cmd` — auto-load
        // `core.runtime` and force-root its helpers so harness
        // `bl_call_key("__wrela_line_*")` / `__wrela_fmt_dec` resolve.
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
            gen_key,
            wrela_compiler::rtconfig::GENERATED_INPUT_PATH.to_string(),
        );
        // Mirror `bin/wrela.rs::load_runtime_bearing_singleton`: time prelude
        // for `seconds(...)` etc., then drop `core.time` from the maps.
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
        let mut programs_vec =
            wrela_compiler::sema::check_program_typed(&modules_vec, &paths).expect("must check");
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
        // Prefer the user root's TypedProgram for test discovery / image fn.
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
        // The unique-instance resolution `bin/wrela.rs::resolve_runtime_test_args`
        // performs, at the subset these conformance sources need (every
        // param is `Actor[T]` with exactly one declared `T` instance).
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
            &runtime_tests,
            &async_tests,
        )
        .expect("lower_and_codegen_image");
        let boot = layout::BootCtx {
            graph: &graph,
            modules: &compiled.modules,
            programs: &compiled.programs,
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
        // plans/M8.md item C1: the same `CoreEntry` lines `bin/wrela.rs`
        // writes for `wrela test`, so a conformance image built here boots
        // the identical way a real one does (absent for a single-core
        // image, which is every conformance image before this item).
        for (core, base) in &image.core_entries {
            report.push_str(&format!("CoreEntry core={core} base={base:#x}\n"));
        }
        (image, report)
    }

    /// The `wrela build` flavor of `compile_test_image`: the same pipeline
    /// up to layout, then `layout_program` instead of `layout_test_image`.
    /// Deliberately a separate fn rather than a flag on the other one —
    /// it needs no runtime tests, no async-test set and no test args, and
    /// threading three unused parameters through would obscure the one
    /// difference that matters (which layout fn runs).
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
            &[],
            &empty_async,
        )
        .expect("lower_and_codegen_image");
        let boot = layout::BootCtx {
            graph: &graph,
            modules: &compiled.modules,
            programs: &compiled.programs,
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

    /// Rewrite `Image sha256=` to match `img` — tests that patch a blob
    /// after `compile_test_image` still present a self-consistent report.
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

    /// (a) The two-hop await chain: root -> Outer -> Inner. The root
    /// turn parks awaiting `Outer.relay`; Outer's turn parks awaiting
    /// `Inner.get` (a nested suspension — Outer's own waker chain is
    /// root's area, Inner's message carries Outer's); Inner's sync turn
    /// completes; Outer resumes and completes; root resumes and asserts.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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
        assert_eq!(
            String::from_utf8_lossy(&outcome.transcript),
            "test chain: ok\n1 passed, 0 failed\n"
        );
        assert_eq!(outcome.exit_code, 0);
    }

    /// (b) FIFO + non-reentrancy under suspension (decision 4's flip
    /// witness shape — item F pins the golden; this proves it now): two
    /// one-way messages queue to Worker while its first turn is
    /// busy-SUSPENDED awaiting Stamper; the second turn starts only
    /// after the first fully completes. The log encodings prove
    /// completion order: Worker's own log writes happen *after* each
    /// turn's await resumes, so `job(1)`'s write preceding `job(2)`'s —
    /// and Stamper's log seeing 1 before 2 — pins turn-at-a-time FIFO.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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
        assert_eq!(
            String::from_utf8_lossy(&outcome.transcript),
            "test fifo: ok\n1 passed, 0 failed\n"
        );
        assert_eq!(outcome.exit_code, 0);
    }

    /// (c) Interleaving — the property the deleted nested-drain
    /// placeholder faked: while ChainActor's turn is parked awaiting the
    /// Log, a `send`-queued turn on Third RUNS (scheduler-mediated,
    /// 04 §2's "awaiting a dependency lets other actors run"), proven by
    /// its stamp (99) landing in the Log BETWEEN the chain's own two
    /// stamps (10, then 20): final log = ((10)*100+99)*100+20 = 109920.
    /// Under the old synchronous drain, Third's turn could never run
    /// during the chain's suspension, and the 99 could not interleave.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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
        assert_eq!(
            String::from_utf8_lossy(&outcome.transcript),
            "test interleave: ok\n1 passed, 0 failed\n"
        );
        assert_eq!(outcome.exit_code, 0);
    }

    /// (d) The deadlock diagnostic. NOT constructible from source at
    /// M6's surface (the report records why: `@image` wiring is the only
    /// way to hand out `Actor[T]` handles, its arguments are evaluated
    /// in declaration order with no post-hoc rebinding, so the handle
    /// graph is acyclic and every await chain bottoms out; messages
    /// cannot carry handles; groups fail closed until item F) — so the
    /// diagnostic is pinned via a hand-built table state, exactly as the
    /// mandate's fallback prescribes: a REAL compiled image (root awaits
    /// Stuck.nudge) whose rtdata is patched pre-boot to mark Stuck's
    /// turn record busy+suspended with no reply ever coming. The root's
    /// message queues behind the phantom parked turn; nothing is ever
    /// ready; the driver prints the named line and the image exits
    /// nonzero.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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
        // The hand-built state: Stuck's one turn is parked forever (busy
        // + suspended, resume_ready never set — as if awaiting a reply
        // that cannot come). boot_init only zero-fills actor STATE
        // slots, so the patched record survives boot untouched.
        patch(&mut blob, stuck.turn + OFF_TURN_BUSY, 1);
        patch(&mut blob, stuck.turn + OFF_TURN_SUSPENDED, 1);

        let outcome = boot_blob(&blob, &report, "deadlock");
        let transcript = String::from_utf8_lossy(&outcome.transcript).into_owned();
        assert_eq!(
            transcript,
            format!("test stuck: FAILED {DEADLOCK_MSG}\n0 passed, 1 failed\n"),
            "the named deadlock line, on the failing root turn's own test line"
        );
        assert_eq!(outcome.exit_code, 1, "fail closed: the image exits nonzero");
    }

    // =======================================================================
    // plans/M6.md item E: pending words, deadline wakes, and the choice-
    // sequence recorder — conformance tests over real HVF, hand-assembled
    // guests exactly like this file's own item-C/clock-test precedent
    // (module docs above): no `.wr` source can exercise a real deadline
    // yet (groups are item F's own job), so these hand-build the minimal
    // guest each mechanism needs, reusing `wrela_compiler::layout::
    // build_checkpoint_and_vector_stub`/`encode` — the exact production
    // routine, never a re-derivation of it.

    /// (b) Park + deadline wake: a hand-assembled guest reads the real
    /// clock once, writes `now + 3ms` to `OFF_NEXT_DEADLINE`, and parks
    /// (the trapping store to `PARK_MMIO_ADDR`). The VMM must sleep real
    /// wall time until (approximately) that deadline, raise vector 0 (a
    /// plain host-side write into this core's own pending word), and
    /// resume the vCPU with PC advanced past the trapping store — the
    /// guest then reads its own pending word back and asserts it reads
    /// `1` (the VMM's own raise, left uncleared since this hand-built
    /// guest never calls `__wrela_checkpoint_service`), proving "guest
    /// resumes and completes" (the task's own conformance wording) rather
    /// than hanging past `WALL_CAP`.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn park_conformance_wakes_at_the_deadline_and_resumes_over_hvf() {
        use wrela_compiler::encode;
        use wrela_machine::{layout as machine_layout, machine_info, mmio, pending};

        const DELTA_NS: u64 = 3_000_000; // 3ms — short, but a real, observable sleep.
        let sp_top = machine_layout::core_stack_base(0) + machine_layout::CORE_STACK_SIZE;

        let mut w = Vec::new();
        w.extend(load_imm_words(9, sp_top));
        w.push(encode::enc_add_imm(31, 9, 0, true)); // mov sp, x9

        // x1 = now_ns (a real clock read — CLOCK_MMIO_ADDR trap #1).
        w.extend(load_imm_words(9, mmio::CLOCK_MMIO_ADDR));
        w.push(encode::enc_ldr_x_imm(1, 9, 0));

        // x1 = deadline = now_ns + DELTA_NS
        w.extend(load_imm_words(2, DELTA_NS));
        w.push(encode::enc_add_reg(1, 1, 2, true));

        // OFF_NEXT_DEADLINE = deadline (an ordinary, non-trapping store —
        // mmio::PARK_MMIO_ADDR's own module doc: "the guest writes its
        // own next deadline ... and only then performs the trapping
        // store").
        w.extend(load_imm_words(
            9,
            machine_layout::MACHINE_INFO_BASE + machine_info::OFF_NEXT_DEADLINE,
        ));
        w.push(encode::enc_str_x_imm(1, 9, 0));

        // Park: the trapping store to PARK_MMIO_ADDR.
        w.extend(load_imm_words(9, mmio::PARK_MMIO_ADDR));
        w.push(encode::enc_str_x_imm(1, 9, 0));

        // Resumed: the pending word must now read 1.
        w.extend(load_imm_words(9, pending::core_word_addr(0)));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        w.push(encode::enc_movz(11, 0, 0, true)); // fail accumulator
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

    /// (a) Vector raise observed at a checkpoint: a hand-assembled,
    /// bounded spinning loop (never parks) calls a self-contained
    /// checkpoint fixture (`hand_built_simple_checkpoint` — the pre-I
    /// empty irq/wake shape) at every back-edge, exactly like a compiled
    /// loop's own checkpoint (`codegen::FnCtx::checkpoint`). A background
    /// host thread (this crate's own `test_delayed_raise` conformance seam
    /// — module doc on `boot_image_core`) raises vector 0 mid-run, entirely
    /// independently of the park protocol (the guest is actively running,
    /// never parked) — modeling "the VMM raises a vector while the guest is
    /// somewhere in a bounded loop" (06 §4) honestly, since M6-E's only
    /// *real* producer of a mid-run raise (an expired group's deadline)
    /// is item F's own job. The loop's own bound is large enough that the
    /// raise's own short delay reliably lands inside it (not after), so
    /// the checkpoint service dispatches the vector-0 routine, which
    /// increments the observation counter — asserted `== 1` after the
    /// loop completes (never lost, never double-counted), alongside the
    /// loop's own counter reaching exactly zero (the raise never
    /// corrupts ordinary control flow). Deliberately not claimed as
    /// replay-exact (the raise's own timing is real wall-clock, not part
    /// of the choice sequence — disclosed in `boot_image_core`'s own doc,
    /// not silently narrowed): replay-stability is (c)'s own job, over
    /// the deadline-wake path (b), which *is* choice-sequence-covered.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn vector_raise_observed_at_a_checkpoint_over_hvf() {
        use wrela_compiler::encode;
        use wrela_machine::{layout as machine_layout, machine_info, pending};

        const LOOP_BOUND: u64 = 200_000_000;
        let sp_top = machine_layout::core_stack_base(0) + machine_layout::CORE_STACK_SIZE;
        // M11 I: production checkpoint section is a floor trampoline + wrela
        // body in `code`. This conformance guest is hand-built, so embed a
        // self-contained simple-path clone (observed++ / pending clear) —
        // the pre-I empty irq/wake shape, frozen here as a VMM fixture.
        let (cp_words, cp_entry_offset) = hand_built_simple_checkpoint();

        let mut w = Vec::new();
        w.extend(load_imm_words(9, sp_top));
        w.push(encode::enc_add_imm(31, 9, 0, true)); // mov sp, x9
        w.extend(load_imm_words(19, LOOP_BOUND)); // x19 = loop counter

        let loop_top = w.len();
        // checkpoint: load pending word, skip the BL if zero.
        w.extend(load_imm_words(9, pending::core_word_addr(0)));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        w.push(encode::enc_cbz(10, 8, true)); // cbz x10, +2 words (skip the bl)
        let bl_word = w.len();
        w.push(0); // placeholder, patched below once cp_words' own base is known
        w.push(encode::enc_subs_imm(19, 19, 1, true));
        {
            let this = w.len() as i64;
            let delta = (loop_top as i64 - this) * 4;
            w.push(encode::enc_cbnz(19, delta as i32, true));
        }

        // The loop's own `cbnz` above falls straight through to here once
        // `x19` hits zero — an unconditional `B` over `cp_words` is
        // required so that fall-through never executes the checkpoint
        // routine itself as if it were this test's own post-loop code
        // (its own trailing `ret` would then return through whatever
        // garbage `x30` happens to hold, not a `BL`'s own fresh value).
        let skip_cp_word = w.len();
        w.push(0); // placeholder `B`, patched once `cp_words`' own end is known

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

        // Post-loop checks: observed count == 1, loop counter == 0.
        w.extend(load_imm_words(
            9,
            machine_layout::MACHINE_INFO_BASE + machine_info::OFF_VECTOR0_OBSERVED,
        ));
        w.push(encode::enc_ldr_x_imm(10, 9, 0));
        w.push(encode::enc_movz(11, 0, 0, true)); // fail accumulator
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

    /// (c) Record -> replay of the park/deadline-wake scenario (b),
    /// byte-stable, with divergence detection on tamper — the choice-
    /// sequence recorder's own conformance evidence (decision 9): a real
    /// recorded boot of the identical park-wake guest, replayed, must
    /// reproduce the exact same transcript digest/exit code (replay's own
    /// sleep-skipped, virtual-time-fed wake, per `Chooser::choose_next`'s
    /// own doc), and a tampered choice log must be caught, named, by
    /// `record::Divergence`.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    /// plans/M6.md item F: the EL1-exception note (`el1_exception_note`'s
    /// own doc comment has the whole mechanism). A four-instruction
    /// hand-built guest performs one unaligned 64-bit load — the MMU is
    /// off, so every access is Device-nGnRnE and naturally
    /// alignment-checked — which the CPU takes as an EL1 synchronous
    /// exception, vectoring to `VBAR_EL1 + 0x200` with `VBAR_EL1` never
    /// installed (06-machine.md §4: this machine has no vector table at
    /// all). The bare fault the VMM sees is therefore an instruction
    /// abort at `0x200`, which says nothing; this test pins that the
    /// diagnostic *also* names the mechanism and reports the original
    /// `ESR_EL1` (EC `0x25` — data abort, same EL) and the real faulting
    /// address in `FAR_EL1`. Exactly the diagnostic that would have named
    /// `golden/boot-group-join`'s own first-boot failure outright.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn an_el1_fault_into_the_absent_vector_table_names_the_original_esr_over_hvf() {
        use wrela_compiler::encode;
        use wrela_machine::layout as machine_layout;

        // A deliberately 4-aligned (never 8-aligned) DRAM address.
        let bad = machine_layout::DRAM_BASE + 0x8004;
        let mut w = Vec::new();
        w.extend(load_imm_words(9, bad));
        w.push(encode::enc_ldr_x_imm(10, 9, 0)); // 64-bit load at a 4-aligned address
        w.push(encode::enc_brk(0)); // never reached

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

    #[test]
    fn record_replay_of_the_park_wake_scenario_is_byte_stable_and_detects_tamper() {
        use wrela_compiler::encode;
        use wrela_machine::{layout as machine_layout, machine_info, mmio, pending};

        const DELTA_NS: u64 = 2_000_000; // 2ms
        let sp_top = machine_layout::core_stack_base(0) + machine_layout::CORE_STACK_SIZE;

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
        // Exactly one ClockRead, then a DeadlineWake, then the vector-0
        // raise — the park-wake scenario's own choice sequence, pinned
        // structurally (values are real wall-clock/monotonic-ns, never
        // compared here — only the tag shape, per the format's own house
        // rule).
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

        // --- replay with the real recording: byte-stable, no divergence ---
        let divergences = record::replay(&report_path, &img_path, &recorded).expect("replay boot");
        assert!(
            divergences.is_empty(),
            "expected no divergence, got {divergences:?}"
        );

        // --- tampered choice tag: aborts (strict replay) -------------------
        let mut bad_tag = recorded.clone();
        bad_tag.choices[1] = record::ChoiceEntry::ClockRead { value: 0 };
        let err =
            record::replay(&report_path, &img_path, &bad_tag).expect_err("strict tag mismatch");
        assert!(err.to_string().contains("replay divergence"), "got {err}");

        // --- tampered exit code: caught -------------------------------------
        let mut bad_exit = recorded.clone();
        bad_exit.exit_code = 7;
        let divergences = record::replay(&report_path, &img_path, &bad_exit).expect("replay boot");
        assert!(divergences.contains(&record::Divergence::ExitCodeMismatch {
            expected: 7,
            actual: 0,
        }));

        let _ = std::fs::remove_dir_all(report_path.parent().unwrap());
    }

    /// plans/M6.md item E, verification's own fail-closed finding: the
    /// process-level exit-code contract `main.rs`'s own module doc names
    /// — `record::replay`'s own returned `Vec<Divergence>` (already
    /// exercised directly, above) is only half the contract; the other
    /// half is `main.rs`'s own mapping from that list to *this process's*
    /// exit code, which had no test coverage of its own before this item.
    /// Builds + codesigns the real `wrela-vmm` *binary* itself (this
    /// crate's every other test exercises the library directly; this one
    /// specifically needs the compiled binary's own `main` to run) and
    /// spawns it as a real subprocess for every documented outcome:
    /// - a clean replay of a guest that reported a **nonzero** exit code
    ///   must itself exit `1` (guest-authored, mirroring a plain boot) —
    ///   not unconditionally `0`, the real bug this test was written to
    ///   catch (a clean replay previously always returned
    ///   `ExitCode::SUCCESS` regardless of the guest's own outcome, fixed
    ///   in the same commit as this test);
    /// - a replay whose recorded `exit_code=` line is tampered must exit
    ///   `EXIT_REPLAY_DIVERGENCE` (`3`), **never** `0` — the exact
    ///   fail-closed violation the coordinator's own verification probe
    ///   named;
    /// - `--replay` against an unparseable record file, and `--record` to
    ///   an unwritable destination, must each exit `EXIT_VMM_FAILURE`
    ///   (`2`).
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn replay_divergence_and_record_failures_exit_nonzero_through_the_real_binary() {
        use std::process::Command;

        use wrela_machine::vmm_process::{EXIT_REPLAY_DIVERGENCE, EXIT_VMM_FAILURE};
        const GUEST_EXIT_CODE: u64 = 5; // nonzero — must collapse to process exit `1`, never `0`.

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

        // A minimal hand-assembled guest (this file's own clock-test
        // precedent): halts immediately with `GUEST_EXIT_CODE` — no sp
        // setup needed, `push_halt` alone is a complete, valid program.
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

        // --- record: a plain boot's own guest-authored exit code --------
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

        // --- clean replay: must ALSO exit 1, never unconditionally 0 ----
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

        // --- tampered exit_code=: must exit EXIT_REPLAY_DIVERGENCE, never 0
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

        // --- malformed record file on --replay: must exit EXIT_VMM_FAILURE
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

        // --- --record to an unwritable path: must exit EXIT_VMM_FAILURE -
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

    // =======================================================================
    // plans/M7.md item F: the virtio-blk device model, its doorbell, and its
    // completions in the recorded choice sequence.
    //
    // `devices.rs`'s own `#[cfg(test)]` module drives the *model* directly
    // (ring shapes, request format, `Flush`, and every malformed-ring
    // rejection by name) with no VMM and no HVF involved at all. What
    // follows is the other half — a **real boot** of a hand-assembled guest
    // that plays the driver's role, exactly the way M5/M6 both established
    // is the only oracle that catches register-level and protocol-level
    // bugs a dump review misses (plans/M7.md's own A–H note: "a real HVF
    // boot as the behavioral oracle wherever the item produces
    // guest-visible behavior"). No compiled `.wr` source can reach a device
    // yet — capabilities, layouts, DMA pools, queues, and receipts are
    // items A/B/C/D/E — so the driver here is hand-assembled, exactly like
    // this file's own clock/park/actor-runtime conformance guests.

    /// Everything one blk conformance boot needs: the image blob (code +
    /// a prefilled ring and buffers), the report declaring the device, and
    /// the expected payload words the guest checks against.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    struct BlkImage {
        img_bytes: Vec<u8>,
        report_text: String,
    }

    /// Builds the hand-assembled blk driver + its ring.
    ///
    /// The ring, the two request headers, the source payload, and the
    /// destination buffer all live in the image's own trailing data region
    /// — plain image bytes, loaded into DRAM by the ordinary boot path —
    /// and one declared pool window covers exactly that region and nothing
    /// else. The descriptor chains are prefilled by this builder (the
    /// harness playing the driver's build-time role); the guest program
    /// itself does the two runtime acts a real driver does: publish an
    /// available entry, and ring the doorbell.
    ///
    /// Two operations, in order:
    /// 1. `T_OUT` sector 0, 512 bytes from `SRC` (chain 0 -> 1 -> 2);
    /// 2. `T_IN` sector 0, 512 bytes into `DST` (chain 3 -> 4 -> 5).
    ///
    /// The read-back therefore proves the write actually reached the
    /// model's own disk, without the guest ever seeing the disk.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn build_blk_conformance_image() -> BlkImage {
        use wrela_machine::layout as machine_layout;

        const QUEUE_SIZE: u64 = 8;
        // Offsets within the data region.
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
        /// The vector bit a completion raises (06 §4). Deliberately not
        /// bit 0: that is M6's deadline vector, and this test asserts the
        /// pending word ends up holding *only* the blk bit — which is what
        /// proves the completion suppressed the park's own sleep rather
        /// than merely racing it.
        const BLK_VECTOR: u64 = 1;

        let payload: Vec<u8> = (0..512u32).map(|i| ((i * 7 + 3) % 256) as u8).collect();
        let expect_first = u64::from_le_bytes(payload[0..8].try_into().unwrap());
        let expect_last = u64::from_le_bytes(payload[504..512].try_into().unwrap());

        let sp_top = machine_layout::core_stack_base(0) + machine_layout::CORE_STACK_SIZE;

        // Pass 1 with placeholder addresses purely to measure the entry
        // sequence's own word count — its length is addr-value-independent
        // (every embedded constant is a fixed-width `load_imm_words`), the
        // identical two-pass technique this file's own actor-runtime test
        // uses.
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
            w.push(encode::enc_add_imm(31, 9, 0, true)); // mov sp, x9

            // One aligned 64-bit store publishes the whole avail header:
            // `flags: u16 = 0, idx: u16, ring[0]: u16, ring[1]: u16`. No
            // 16-bit store encoding is needed, and the guest's own view of
            // the ring stays exactly the little-endian layout the model
            // reads.
            let publish = |w: &mut Vec<u32>, idx: u64| {
                w.extend(load_imm_words(9, avail));
                w.extend(load_imm_words(10, (idx << 16) | (0 << 32) | (3 << 48)));
                w.push(encode::enc_str_x_imm(10, 9, 0));
                // 06 §5's doorbell: an ordinary store to ordinary DRAM.
                // No trap, no exit — the VMM polls this word.
                w.extend(load_imm_words(9, doorbell));
                w.push(encode::enc_movz(10, 1, 0, true));
                w.push(encode::enc_str_x_imm(10, 9, 0));
            };
            // A park is what gives the VMM a chance to poll at all (this
            // guest has no checkpoint loop of its own). The deadline is
            // deliberately real and short: if the doorbell path were
            // broken, this boot would still finish and fail its checks
            // loudly rather than hanging to `WALL_CAP`.
            let park = |w: &mut Vec<u32>| {
                w.extend(load_imm_words(9, mmio::CLOCK_MMIO_ADDR));
                w.push(encode::enc_ldr_x_imm(11, 9, 0));
                w.extend(load_imm_words(12, 20_000_000)); // 20ms
                w.push(encode::enc_add_reg(11, 11, 12, true));
                w.extend(load_imm_words(
                    9,
                    machine_layout::MACHINE_INFO_BASE + machine_info::OFF_NEXT_DEADLINE,
                ));
                w.push(encode::enc_str_x_imm(11, 9, 0));
                w.extend(load_imm_words(9, mmio::PARK_MMIO_ADDR));
                w.push(encode::enc_str_x_imm(11, 9, 0));
            };

            publish(&mut w, 1); // chain 0: the write
            park(&mut w);
            publish(&mut w, 2); // chain 3: the read-back
            park(&mut w);

            // --- checks -------------------------------------------------
            // used[0..8]  = flags(0) | idx(2)<<16 | ring[0].id(0)<<32
            // used[8..16] = ring[0].len(1) | ring[1].id(3)<<32
            // used[16..24]= ring[1].len(513) | ring[2].id(0)<<32
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

            w.push(encode::enc_movz(1, 0, 0, true)); // x1 = fail accumulator
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
            // Only the blk vector, never the deadline's own bit 0: a park
            // that slept and woke on its deadline would leave bit 0 set
            // too, so this pins that the completion itself suppressed the
            // sleep (06 §4's "a wake between test and park cannot be
            // lost", applied to completions).
            check(&mut w, 26, 1u64 << BLK_VECTOR, 7);

            w.extend(load_imm_words(15, mmio::EXIT_MMIO_ADDR));
            w.push(encode::enc_str_x_imm(1, 15, 0));
            w.push(encode::enc_brk(0));
            w
        }

        let entry_len = build_entry(sp_top, 0, 0, 0).len();
        let code_bytes = (entry_len as u64) * 4;
        // Data must sit outside the page-granular RX window applied to
        // `Section name=entry` (16 KiB HVF pages). Padding code up to a
        // page and starting the ring/doorbell there keeps doorbell stores
        // on RW DRAM.
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

        // Chain 0: write 512 bytes of SRC to sector 0.
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
        // Chain 3: read sector 0 back into DST.
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
        // A status byte that is never written would read as the 0 the
        // whole image is padded with, which is also `STATUS_OK` — so
        // pre-poison both, and the checks above only pass if the model
        // genuinely wrote them.
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

    /// The doorbell path end to end, over real HVF: a hand-assembled
    /// driver publishes a `T_OUT` chain, rings the shared-memory doorbell
    /// (an ordinary store — **no trap**, 06 §5), parks, and the VMM's own
    /// park-path poll services the ring, writes the disk, publishes the
    /// used entry and raises the completion vector *without ever
    /// sleeping*; then the same guest reads the sector back through a
    /// second chain and checks every byte of it.
    ///
    /// Eight independent checks fold into the exit code (bit N names which
    /// one failed), so a regression says what broke rather than merely
    /// that something did.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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
        // The two completions and their two vector raises, in order — and
        // nothing else from the park path, since neither park slept.
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
                assert_eq!((*status, *len), (0, 1)); // a write writes only the status byte
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

    /// plans/M7.md decision 7 + 06 §8: device completions join the
    /// recorded choice sequence, and a replay reproduces the boot exactly.
    /// Record -> replay clean -> tamper each field of a completion ->
    /// named divergence, all over the identical real blk boot above.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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
        // The record file's own text round-trips with the new tag in it.
        let text = recorded.to_text();
        assert!(text.contains("DeviceCompletion device=blk queue=0 head=0 status=0 len=1 digest="));
        assert_eq!(
            record::RecordFile::parse(&text).expect("parses"),
            recorded,
            "the completion tag must survive the record file's own text format"
        );

        // --- replay with the real recording: no divergence --------------
        let divergences = record::replay(&report_path, &img_path, &recorded).expect("replay boot");
        assert!(
            divergences.is_empty(),
            "expected a clean replay, got {divergences:?}"
        );

        // --- tamper each field of the first completion ------------------
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

    /// A malformed ring is a *diagnosable VMM-side error*, over a real
    /// boot — not a panic, not an out-of-bounds read, and not a silently
    /// skipped operation (03 §4). The identical conformance image, with
    /// one descriptor's `addr` repointed at the machine-info page, which
    /// no declared pool covers (plans/M7.md decision 5).
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn a_descriptor_outside_every_declared_pool_fails_the_boot_closed_over_hvf() {
        use wrela_machine::layout as machine_layout;
        let built = build_blk_conformance_image();
        // Find descriptor 1's own `addr` word (chain 0's data descriptor)
        // by re-deriving the data region's base from the report itself —
        // no second copy of the layout constants.
        let data_base = built
            .report_text
            .lines()
            .find_map(|l| l.strip_prefix("BlkPool name=BlockControl device=device#0 base="))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok())
            .expect("the report declares the pool this image uses");
        let mut img = built.img_bytes.clone();
        let desc1 = (data_base - machine_layout::IMAGE_BASE) as usize + devices::DESC_SIZE as usize; // descriptor index 1
        img[desc1..desc1 + 8].copy_from_slice(&machine_layout::DRAM_BASE.to_le_bytes());
        // Digest is checked before the device model runs — rewrite the
        // report's Image sha256 so the deliberate descriptor forgery is
        // what fails closed, not the digest gate.
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

    // --- report parsing for the declared device ---------------------------

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
        // plans/M8.md item P: **every** declared window is carried, each
        // with the device it is bound to — the foreign one is what
        // `GuestMem` refuses rather than never hears about.
        assert_eq!(blk.pools.len(), 2);
        assert_eq!(blk.pools[0].name, "BlockControl");
        assert_eq!(blk.pools[0].device, 0);
        assert_eq!(blk.pools[1].name, "Foreign");
        assert_eq!(blk.pools[1].device, 1);
    }

    /// Every half-declared, misspelled, or contradictory device
    /// declaration fails the whole report closed — a device this VMM only
    /// half-understands is exactly the configuration it must never boot.
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
            // plans/M8.md item P: the device field is required on both
            // line kinds, and only in the `device#<n>` spelling.
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

    // --- plans/M8.md item C1: three vCPUs actually execute ----------------

    /// The cross-core conformance source both C1 boot tests below use: one
    /// actor on core 0 (messaged by the root turn), one on core 1 (reachable
    /// by nothing until item C2's rings), and core 2 with nothing placed on
    /// it at all — the "a core with no placed actor must still come up, find
    /// nothing, and park" arm.
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
    img = Image(name="cross-core-conf", target=Target.wrela_machine_v1)
    home = img.actor(Home, mailbox=4, core=0)
    away = img.actor(Away, mailbox=2, core=1)
    img.on_failure(policy=Failure.Halt)
    return img.seal()
"#;

    /// plans/M8.md item C1's own acceptance test, stated as the plan states
    /// it: **three cores run**, and the evidence is guest-written.
    ///
    /// Every core's mark is written by that core's own entry block
    /// (`machine_info::core_mark_addr`), so a boot where cores 1 and 2 never
    /// executed leaves the zeroed reservation's own zeros there. The
    /// single-core half of the assertion matters just as much: an image that
    /// brings up one core writes **no** mark at all and releases nothing,
    /// which is the mechanical reason every M5-M7 transcript is unchanged.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn three_cores_come_up_on_a_cross_core_image_over_hvf() {
        let outcome = boot_source(CROSS_CORE_SRC, "c1-three-cores");
        assert_eq!(
            String::from_utf8_lossy(&outcome.transcript),
            "test boots: ok\n1 passed, 0 failed\n"
        );
        assert_eq!(outcome.exit_code, 0);
        // Core 0 by the entry driver, cores 1 and 2 by their own entry
        // blocks — each its own value, so a core running another core's
        // block cannot pass this.
        assert_eq!(outcome.core_marks, vec![1, 2, 3]);

        // The single-core control: same shape, no `core=` anywhere, so
        // nothing is released and no core marks itself.
        let single = boot_source(
            &CROSS_CORE_SRC
                .replace(", core=0", "")
                .replace(", core=1", ""),
            "c1-single-core",
        );
        assert_eq!(
            String::from_utf8_lossy(&single.transcript),
            "test boots: ok\n1 passed, 0 failed\n"
        );
        assert_eq!(single.core_marks, vec![0, 0, 0]);
    }

    /// The mark is `core + 1`, never a bare `1`, for one reason: a
    /// mis-wired `CoreEntry` address would otherwise look exactly like a
    /// correct boot. Here core 2's declared entry is pointed at core 1's
    /// entry block — every core still runs, every core still parks, the
    /// transcript would still have said `ok` — and the boot fails closed
    /// naming core 2, because core 2's own mark was never written.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn a_miswired_core_entry_fails_the_boot_closed_over_hvf() {
        // Two cores sharing an entry address is refused at parse
        // (`validate_report_invariants`); that is the durable close for this
        // forgery. The pre-set-level shape (boot, then "never ran its mark")
        // is no longer reachable.
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

    /// A `wrela build` (production) image of a **cross-core** program halts
    /// with `EXIT_CODE_NO_RUNTIME`, exactly like its single-core twin —
    /// it does not fail with a bring-up fault about cores it never
    /// released.
    ///
    /// The sibling above pins the mark check; this one pins its *scope*.
    /// `layout_program`'s entry stub halts before any release, so
    /// `build_entry_driver`'s release block — the same code that writes
    /// core 0's own mark — is never emitted at all. But such an image
    /// still carries `CoreEntry` lines, because it still *contains* the
    /// secondary entry blocks, and the check used to key off that declared
    /// count: the boot then died with "core 0 was released but never ran
    /// its own entry block", naming a release that never happened and
    /// turning a clean exit 1 into a bad-image exit 2. Found by probing a
    /// `wrela build` of the cross-core golden by hand.
    ///
    /// `sched.state` cannot answer this question — every core is
    /// `Finished` by the time the marks are checked — which is why
    /// `Shared::released` records the doorbell instead of inferring it.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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

    /// A guest fault on a **secondary** core names that core and fails the
    /// boot closed — never a silent hang, never a partial transcript
    /// reported as success.
    ///
    /// Same-address and unaligned forgeries are refused by
    /// `validate_report_invariants` (pinned in
    /// `parse_report_refuses_placement_forgeries`). This oracle retargets
    /// core 1 to the start of the `entry` section — still executable,
    /// 4-byte aligned, distinct — so the vCPU runs, then fails closed
    /// naming core 1.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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

    /// The report is the whole configuration (06 §3), so a `CoreEntry` line
    /// this VMM cannot honor is refused before any vCPU is created — never
    /// defaulted, never guessed at.
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
                "a core outside 0..VCPUS",
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
        // The well-formed pair parses, ascending and contiguous.
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

    /// plans/M8.md item C3: the `Ring` lines the admission recorder reads.
    /// Parsed strictly for the same reason every device line is — a ring
    /// this VMM half-understands is one whose admissions it would silently
    /// under-record, and 06 §8 makes it the recorder of exactly those.
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
        // A ring naming a core the image never brings up: the `CoreEntry`
        // set is the machine, and an admission nothing can ever perform is
        // a report this VMM refuses rather than carries.
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

        // The well-formed set: request rings kept in report order, reply
        // rings shape-checked and dropped (a reply is delivered to a turn
        // record, never admitted to a mailbox).
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
                // count = base + cap * slot + 16 (slots, head, tail, count)
                (0, 1, "Sink", 0x40502cb8 + 8 * 24 + 16),
                (2, 1, "Sink", 0x40502de8 + 8 * 24 + 16),
            ]
        );
    }

    /// plans/M8.md item H Target A — placement / report forgery. Every row
    /// was a semantic gap `parse_report` previously accepted; each is now
    /// refused by name. The report is this VMM's whole configuration
    /// (AGENTS.md / 06 §3), so a forged line must never boot.
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

        // (1) CoreEntry base inside a non-executable section (rtdata).
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
        // (2) CoreEntry base outside every section.
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
        // (3) CoreEntry whose only owning section is data (no executable
        // section contains the address) — the production-image shape of
        // "not code", distinct from a test image's `entry`/`code` harness.
        // Core 0's `Entry` still needs a real exec section (DRAM + code).
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
        // (4) Ring base overlapping another ring.
        {
            let text = format!(
                "{head}{cores}\
                 Ring kind=request src=0 dst=1 target=A cap=4 slot=16 bytes=88 base=0x40501000\n\
                 Ring kind=request src=2 dst=1 target=A cap=4 slot=16 bytes=88 base=0x40501020\n"
            );
            let err = parse_report(&text).expect_err("overlapping rings");
            assert!(err.to_string().contains("overlaps"), "{err}");
        }
        // (5) Ring base overlapping a per-core stack.
        {
            let stack = wrela_machine::layout::core_stack_base(1);
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
        // (6) Placement core disagrees with the CoreEntry set.
        {
            let text = format!(
                "{head}\
                 CoreEntry core=1 base=0x40500100\n\
                 Actor index=0 type=Sink\n\
                 Placement id=actor#0 type=Sink core=2 source=explicit work=0 work_source=unproved \
                 bytes=1 bytes_state=1 bytes_mailbox=0 bytes_pool=0\n"
            );
            let err = parse_report(&text).expect_err("Placement on undeclared core");
            let msg = err.to_string();
            assert!(
                msg.contains("Placement id=actor#0 core=2") && msg.contains("never brings up"),
                "{msg}"
            );
        }
        // (7) Placement naming an actor the Actor lines do not declare.
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
        // (8) Unaligned CoreEntry base.
        {
            let text = format!(
                "{head}CoreEntry core=1 base=0x40500101\nCoreEntry core=2 base=0x40500200\n"
            );
            let err = parse_report(&text).expect_err("unaligned CoreEntry");
            assert!(err.to_string().contains("is not 4-byte aligned"), "{err}");
        }
        // (9) Two cores entering at the same address.
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
        // (10) Overlapping Sections (forged size swallows a neighbour).
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
        // (11) Duplicate Placement id (same actor on two cores).
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
        // (12) Placement type disagrees with Actor type.
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
        // (13) Declared Actor with no Placement.
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
        // Well-formed control: Placement agrees with Actor + CoreEntry.
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
        // (14) A request ring whose `target=` names no declared root.
        // Found by orchestrator spot-probe after (8)–(13) landed: the
        // set-level pass validated the Placement set but left the ring's
        // own delivery target unaccounted for, one field over.
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
        // (15) A reply ring carries `target=-` and must stay exempt —
        // it delivers back to its caller, not into a named mailbox.
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

    /// plans/M8.md item C3, decision 42: the whole counting rule of the
    /// admission witness, exercised directly — the guest-memory read
    /// around it is three lines, this is the part that can be wrong.
    #[test]
    fn admission_witness_counts_only_the_running_core_s_own_drain() {
        let ring = |src: usize, dst: usize, target: &str| RequestRing {
            src,
            dst,
            target: target.to_string(),
            data_base: 0,
            count_addr: 0,
        };
        let mut w = AdmissionWitness::new(vec![
            ring(0, 1, "Sink"),
            ring(0, 2, "Far"),
            ring(2, 1, "Sink"),
        ]);
        // Core 0 published two messages into ring 0 and one into ring 1.
        // It is the *producer* of both, so nothing was admitted.
        assert_eq!(w.observe(&[2, 1, 0], 0).expect("ok"), Vec::new());
        // Core 2 runs: it drains its own inbound ring (index 1) and
        // publishes into ring 2 in the same hold. Only the drain counts,
        // and it is named by the ring's target and its *producing* core.
        assert_eq!(
            w.observe(&[2, 0, 1], 2).expect("ok"),
            vec![("Far".to_string(), "core0".to_string())]
        );
        // Core 1 runs and drains both of its inbound lanes — in ring
        // order, which is the order `build_rt_drain` walks them.
        assert_eq!(
            w.observe(&[0, 0, 0], 1).expect("ok"),
            vec![
                ("Sink".to_string(), "core0".to_string()),
                ("Sink".to_string(), "core0".to_string()),
                ("Sink".to_string(), "core2".to_string()),
            ]
        );
        // A core that ran and touched nothing admits nothing.
        assert_eq!(w.observe(&[0, 0, 0], 1).expect("ok"), Vec::new());
    }

    /// The invariant the exact (non-modular) count rests on: a ring whose
    /// *consuming* core is the one that just ran cannot have grown,
    /// because its producer is a different core and the baton
    /// (plans/M8.md decision 11) means no other core ran. If that ever
    /// stops being true the witness fails closed rather than silently
    /// under-recording an admission order.
    #[test]
    fn admission_witness_fails_closed_if_a_consumed_ring_grows() {
        let mut w = AdmissionWitness::new(vec![RequestRing {
            src: 0,
            dst: 1,
            target: "Sink".to_string(),
            data_base: 0,
            count_addr: 0,
        }]);
        let err = w.observe(&[3], 1).expect_err("must fail closed");
        assert!(err.contains("SPSC producer/consumer split"), "{err}");
    }

    /// A report with no `Blk*` lines at all constructs no device model —
    /// the state every image built today is in, and the reason this item
    /// moves no existing golden.
    #[test]
    fn parse_report_without_blk_lines_declares_no_device() {
        let text = format!(
            "Machine revision={}\nInput path=x sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nImage sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nSection name=entry base=0x40500000 size=1\nEntry base=0x40500000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        assert!(parse_report(&text).expect("parses").blk.is_none());
    }
}
