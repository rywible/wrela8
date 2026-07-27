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

use std::path::Path;
use std::time::{Duration, Instant};

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

/// The report's own structural facts this VMM actually consumes (module
/// doc's own "whole configuration" — everything else this milestone's
/// report format carries is compiler-internal bookkeeping this VMM never
/// reads). Parsed by `parse_report`, below.
#[derive(Debug)]
struct ParsedReport {
    entry: u64,
    /// plans/M7.md item F: the declared `blk` device, if any (06 §3: "the
    /// VMM ... preconfigures every device, queue, and shared-memory window
    /// the report declares — device topology is a *build output*, not a
    /// probed fact"). `None` for every image built today: the compiler
    /// emits no `Blk*` lines until the driver-side items (C/D/E) land, and
    /// a report without them boots exactly as it did before this item, no
    /// device model constructed at all.
    blk: Option<devices::BlkConfig>,
    /// plans/M7.md item G: host writes into `interrupt_status` plus the
    /// vector to raise, applied before the vCPU runs. Empty for images
    /// that bind no ISR.
    irq_injects: Vec<IrqHostInject>,
    /// plans/M8.md item C1: `(core, entry address)` for every **secondary**
    /// core the image brings up, ascending and contiguous from core 1.
    /// Empty for every single-core image — which is every image built
    /// before this item, so their boot path is unchanged down to the
    /// number of vCPUs this VMM creates.
    core_entries: Vec<(usize, u64)>,
    /// plans/M8.md item C3, decision 42: this image's own cross-core
    /// **request** rings, in report order — the order the guest's own
    /// drain walks its lanes (`layout::build_rt_drain`), which is what
    /// makes a reconstruction from occupancy words an ordered one. Reply
    /// rings are parsed for shape and then dropped: a reply is addressed
    /// to a turn record, not admitted to a mailbox, so it is not part of
    /// 06 §8's "per-mailbox cross-core admission order". Empty for every
    /// single-core image.
    request_rings: Vec<RequestRing>,
}

/// One `Ring kind=request ...` report line, as the recorder consumes it
/// (plans/M8.md item C3). `count_addr` is the ring's occupancy word:
/// `layout::place_runtime_tables` lays each ring out as `capacity *
/// slot_size` bytes of slots followed by `head`, `tail`, `count`, so the
/// third bookkeeping word is `base + capacity * slot_size + 16`. That
/// derivation is the one thing this struct knows that the report line does
/// not spell outright, and it is stated here rather than inline at the
/// read site.
#[derive(Debug, Clone)]
struct RequestRing {
    /// The producing core — decision 28: the producer of a cross-core ring
    /// is a *core*, not an actor, which is exactly what an `Admission`
    /// entry's `sender` field names.
    src: usize,
    /// The consuming core: the one whose drain performs the admission.
    dst: usize,
    /// The mailbox root this ring feeds — exactly one, by decision 28.
    target: String,
    count_addr: u64,
}

/// A ring's declared byte range (`base`..`base+bytes`), request or reply —
/// kept only long enough for the overlap checks in `parse_report`. The
/// three-word head/tail/count bookkeeping is part of `bytes` (same formula
/// the compiler's `RingLayout::bytes` uses: `cap * slot + 24`).
#[derive(Debug, Clone)]
struct RingRange {
    kind: String,
    src: usize,
    dst: usize,
    target: String,
    base: u64,
    bytes: u64,
}

impl RingRange {
    fn end(&self) -> u64 {
        self.base.saturating_add(self.bytes)
    }
}

/// plans/M8.md item H sweep, Target A: a `Section name=... base=... size=`
/// line, parsed fully so a `CoreEntry`/`Ring` that points into the wrong
/// section (or into no section) can be refused by name rather than booted.
#[derive(Debug, Clone)]
struct ReportSection {
    name: String,
    base: u64,
    size: u64,
}

impl ReportSection {
    fn contains(&self, addr: u64) -> bool {
        addr >= self.base && addr < self.base.saturating_add(self.size)
    }

    fn end(&self) -> u64 {
        self.base.saturating_add(self.size)
    }
}

/// One `Placement id=actor#N ... core=C ...` line. Optional in the
/// VMM-facing report (`append_vmm_runtime_lines` does not emit them today),
/// but when present they are configuration and a forgery that places an
/// actor on a core this image never brings up must not boot.
#[derive(Debug, Clone)]
struct ReportPlacement {
    id: String,
    type_name: String,
    core: usize,
}

/// An `Actor index=` / `Driver index=` root the Placement set must cover
/// exactly once (plans/M8.md item H Target A follow-up).
#[derive(Debug, Clone)]
struct DeclaredRoot {
    id: String,
    type_name: String,
}

/// Ring bookkeeping beyond the slot bytes: head, tail, count — three
/// u64s. Mirrored from the compiler's `MAILBOX_BOOKKEEPING_SIZE` rather
/// than imported (that constant lives in `layout.rs`, which this crate
/// does not link for the formula); the `bytes == cap * slot + 24` check
/// below is the tripwire if either side drifts.
const RING_BOOKKEEPING_BYTES: u64 = 3 * 8;

/// One `IrqHostInject` report line (plans/M7.md item G).
#[derive(Debug, Clone)]
struct IrqHostInject {
    base: u64,
    offset: u64,
    status: u32,
    vector: u64,
}

/// One `Kind key=value key=value ...` report line's own fields. Shared by
/// every `Blk*` line below; deliberately strict — a malformed field, a
/// missing required field, or an unknown key fails the whole report closed
/// (`VmmError::MalformedReport`), because a device declaration this VMM
/// half-understands is exactly the configuration it must never boot on.
fn parse_report_fields<'a>(
    kind: &str,
    rest: &'a str,
    allowed: &[&str],
) -> Result<std::collections::BTreeMap<&'a str, &'a str>, VmmError> {
    let mut fields = std::collections::BTreeMap::new();
    for part in rest.split_whitespace() {
        let Some((k, v)) = part.split_once('=') else {
            return Err(VmmError::MalformedReport(format!(
                "`{kind}` field {part:?} has no `=`"
            )));
        };
        if !allowed.contains(&k) {
            return Err(VmmError::MalformedReport(format!(
                "`{kind}` has no field `{k}` (expected one of {allowed:?})"
            )));
        }
        if fields.insert(k, v).is_some() {
            return Err(VmmError::MalformedReport(format!(
                "`{kind}` repeats field `{k}`"
            )));
        }
    }
    Ok(fields)
}

/// A required `key=<integer>` field, decimal or `0x`-prefixed.
fn report_u64(
    kind: &str,
    fields: &std::collections::BTreeMap<&str, &str>,
    key: &str,
) -> Result<u64, VmmError> {
    let raw = fields.get(key).copied().ok_or_else(|| {
        VmmError::MalformedReport(format!("`{kind}` is missing required field `{key}`"))
    })?;
    let parsed = match raw.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => raw.parse::<u64>(),
    };
    parsed.map_err(|e| VmmError::MalformedReport(format!("`{kind}` field `{key}={raw}`: {e}")))
}

/// A required `device=device#<N>` field (plans/M8.md item P). The report
/// spells a declared device the same way everywhere — `device#0` — and
/// this VMM parses that spelling rather than a bare integer so a line that
/// lost its prefix is a malformed report rather than a silently different
/// device.
fn report_device_index(
    kind: &str,
    fields: &std::collections::BTreeMap<&str, &str>,
) -> Result<u64, VmmError> {
    let raw = fields.get("device").copied().ok_or_else(|| {
        VmmError::MalformedReport(format!(
            "`{kind}` is missing required field `device` — 03-hardware.md §3: all memory a device \
             can reach originates from *its* bound pools, which is not a statement this VMM can \
             enforce about an unnamed device"
        ))
    })?;
    let digits = raw.strip_prefix("device#").ok_or_else(|| {
        VmmError::MalformedReport(format!(
            "`{kind}` field `device={raw}`: expected `device#<index>`"
        ))
    })?;
    digits
        .parse::<u64>()
        .map_err(|e| VmmError::MalformedReport(format!("`{kind}` field `device={raw}`: {e}")))
}

/// Parses the minimal, internal (not itself golden-pinned — `wrela test`'s
/// own merged stdout is the golden surface, not this file) report format
/// `bin/wrela.rs`'s runtime tier writes alongside the image (a `Machine
/// revision=` line, one or more `Input path=... digest=...` lines, one or
/// more `Section name=... base=... size=...` lines, and one `Entry
/// base=0x...` line). Validates exactly what 06 §3/§10 require this VMM to
/// validate: the machine revision (refused, loudly, on any mismatch) and
/// every fact's own *presence* (this VMM has no access to the original
/// source files to re-hash against — only the compiler does, at build
/// time; re-verifying a digest here would require shipping the sources
/// into the image, which nothing in this milestone's surface does).
fn parse_report(text: &str) -> Result<ParsedReport, VmmError> {
    let mut revision: Option<String> = None;
    let mut has_input = false;
    let mut entry: Option<u64> = None;
    // plans/M7.md item F: the declared `blk` device's own three line
    // kinds, accumulated here and assembled into one `BlkConfig` below.
    // Deliberately `Blk`-prefixed rather than reusing `Device`/`Pool`:
    // `report.rs`'s own full `--stage=report` artifact already spells
    // those two words with entirely different fields, and `parse_report`
    // trims indentation away, so a distinct prefix is what keeps the two
    // formats from ever being silently confusable.
    let mut blk_device: Option<(u64, u64, u64, Option<u64>)> = None;
    let mut blk_queue: Option<devices::BlkQueueConfig> = None;
    let mut blk_pools: Vec<devices::PoolWindow> = Vec::new();
    let mut irq_injects: Vec<IrqHostInject> = Vec::new();
    let mut core_entries: Vec<(usize, u64)> = Vec::new();
    let mut request_rings: Vec<RequestRing> = Vec::new();
    // plans/M8.md item H sweep, Target A: semantic checks beyond
    // presence/shape. Sections are parsed fully (a `CoreEntry` must land
    // in `rtcode`); every ring's byte range is retained for overlap;
    // `Placement`/`Actor` lines are optional but, when present, must agree
    // with the core set and with each other — a forged report is an attack
    // surface, not a convenience.
    let mut sections: Vec<ReportSection> = Vec::new();
    let mut ring_ranges: Vec<RingRange> = Vec::new();
    let mut placements: Vec<ReportPlacement> = Vec::new();
    let mut declared_roots: Vec<DeclaredRoot> = Vec::new();
    let mut layout_root_names: Vec<String> = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("Machine revision=") {
            revision = Some(rest.to_string());
        } else if line.starts_with("Input path=") {
            has_input = true;
        } else if let Some(rest) = line.strip_prefix("Section ") {
            let fields = parse_report_fields("Section", rest, &["name", "base", "size"])?;
            let name = fields.get("name").copied().ok_or_else(|| {
                VmmError::MalformedReport("`Section` is missing required field `name`".to_string())
            })?;
            let base = report_u64("Section", &fields, "base")?;
            let size = report_u64("Section", &fields, "size")?;
            if size == 0 {
                return Err(VmmError::MalformedReport(format!(
                    "`Section name={name} base={base:#x} size=0`: a section with no bytes is not \
                     a configuration this VMM can honor"
                )));
            }
            if sections.iter().any(|s| s.name == name) {
                return Err(VmmError::MalformedReport(format!(
                    "`Section name={name}` is repeated"
                )));
            }
            sections.push(ReportSection {
                name: name.to_string(),
                base,
                size,
            });
        } else if let Some(rest) = line.strip_prefix("Entry base=") {
            let digits = rest.trim_start_matches("0x");
            entry = u64::from_str_radix(digits, 16).ok();
        } else if let Some(rest) = line.strip_prefix("CoreEntry ") {
            // plans/M8.md item C1 / 06 §3: where this VMM starts vCPU N once
            // core 0's entry rings the release doorbell. Device topology is
            // a build output and so is the core set — nothing here is
            // probed, defaulted, or guessed.
            let fields = parse_report_fields("CoreEntry", rest, &["core", "base"])?;
            let core = report_u64("CoreEntry", &fields, "core")?;
            let base = report_u64("CoreEntry", &fields, "base")?;
            if core == 0 || core as usize >= wrela_machine::VCPUS {
                return Err(VmmError::MalformedReport(format!(
                    "`CoreEntry core={core}`: secondary cores are 1..{} (06-machine.md §1: the \
                     machine has {} vCPUs, and core 0's entry is the `Entry base=` line)",
                    wrela_machine::VCPUS,
                    wrela_machine::VCPUS
                )));
            }
            core_entries.push((core as usize, base));
        } else if let Some(rest) = line.strip_prefix("Ring ") {
            // plans/M8.md item C3, decision 42: a cross-core ring the
            // recorder must be able to *address*, because 06 §8 makes this
            // VMM the recorder of "per-mailbox cross-core admission order"
            // and the admission itself is performed by guest code in guest
            // memory. Parsed strictly (an unknown field or a missing
            // `base=` fails the report closed) for the same reason every
            // device line is: a ring this VMM half-understands is one it
            // would silently under-record.
            let fields = parse_report_fields(
                "Ring",
                rest,
                &[
                    "kind", "src", "dst", "target", "cap", "slot", "bytes", "base",
                ],
            )?;
            let kind = fields.get("kind").copied().ok_or_else(|| {
                VmmError::MalformedReport("`Ring` is missing required field `kind`".to_string())
            })?;
            let src = report_u64("Ring", &fields, "src")?;
            let dst = report_u64("Ring", &fields, "dst")?;
            let capacity = report_u64("Ring", &fields, "cap")?;
            let slot = report_u64("Ring", &fields, "slot")?;
            let bytes = report_u64("Ring", &fields, "bytes")?;
            let base = report_u64("Ring", &fields, "base")?;
            let target = fields.get("target").copied().ok_or_else(|| {
                VmmError::MalformedReport("`Ring` is missing required field `target`".to_string())
            })?;
            if src as usize >= wrela_machine::VCPUS || dst as usize >= wrela_machine::VCPUS {
                return Err(VmmError::MalformedReport(format!(
                    "`Ring src={src} dst={dst}`: this machine has {} vCPUs (06-machine.md §1)",
                    wrela_machine::VCPUS
                )));
            }
            if src == dst {
                return Err(VmmError::MalformedReport(format!(
                    "`Ring src={src} dst={dst}`: a ring is a *cross*-core edge; same-core edges \
                     keep the mailbox path (04-compiler.md §3)"
                )));
            }
            // plans/M8.md item H Target A: `bytes` is not decorative — it
            // is `cap * slot + 24` (slots plus head/tail/count). A triple
            // that does not add up would make `count_addr` disagree with
            // the range the report claims to reserve.
            let expected_bytes = capacity
                .checked_mul(slot)
                .and_then(|s| s.checked_add(RING_BOOKKEEPING_BYTES))
                .ok_or_else(|| {
                    VmmError::MalformedReport(format!(
                        "`Ring cap={capacity} slot={slot}`: capacity * slot overflows"
                    ))
                })?;
            if bytes != expected_bytes {
                return Err(VmmError::MalformedReport(format!(
                    "`Ring kind={kind} src={src} dst={dst} target={target} cap={capacity} \
                     slot={slot} bytes={bytes}`: bytes must equal cap*slot+{RING_BOOKKEEPING_BYTES} \
                     (={expected_bytes}); a forged triple would point the admission witness at \
                     the wrong occupancy word"
                )));
            }
            match kind {
                "request" => {
                    if target == "-" {
                        return Err(VmmError::MalformedReport(
                            "`Ring kind=request` with no `target=`: a request ring feeds exactly \
                             one mailbox root, which is what names the admission"
                                .to_string(),
                        ));
                    }
                    request_rings.push(RequestRing {
                        src: src as usize,
                        dst: dst as usize,
                        target: target.to_string(),
                        // `place_runtime_tables`'s own layout: slots, then
                        // head, tail, count.
                        count_addr: base + capacity * slot + 16,
                    });
                    ring_ranges.push(RingRange {
                        kind: "request".to_string(),
                        src: src as usize,
                        dst: dst as usize,
                        target: target.to_string(),
                        base,
                        bytes,
                    });
                }
                // A reply is delivered to a turn record, not admitted to a
                // mailbox: retained for overlap checks, then dropped from
                // the admission witness set.
                "reply" => {
                    ring_ranges.push(RingRange {
                        kind: "reply".to_string(),
                        src: src as usize,
                        dst: dst as usize,
                        target: target.to_string(),
                        base,
                        bytes,
                    });
                }
                other => {
                    return Err(VmmError::MalformedReport(format!(
                        "`Ring kind={other}`: the only lanes are `request` and `reply` \
                         (plans/M8.md decision 29)"
                    )));
                }
            }
        } else if let Some(rest) = line.strip_prefix("Placement ") {
            // Full `--stage=report` lines; optional in the VMM-facing
            // subset. When present they must name a real actor and a core
            // this image brings up — otherwise a forged placement would
            // disagree with the CoreEntry set the VMM actually starts.
            let fields = parse_report_fields(
                "Placement",
                rest,
                &[
                    "id",
                    "type",
                    "core",
                    "source",
                    "work",
                    "work_source",
                    "bytes",
                    "bytes_state",
                    "bytes_mailbox",
                    "bytes_pool",
                ],
            )?;
            let id = fields.get("id").copied().ok_or_else(|| {
                VmmError::MalformedReport("`Placement` is missing required field `id`".to_string())
            })?;
            let type_name = fields.get("type").copied().ok_or_else(|| {
                VmmError::MalformedReport(
                    "`Placement` is missing required field `type`".to_string(),
                )
            })?;
            let core = report_u64("Placement", &fields, "core")?;
            if core as usize >= wrela_machine::VCPUS {
                return Err(VmmError::MalformedReport(format!(
                    "`Placement id={id} core={core}`: this machine has {} vCPUs (06-machine.md §1)",
                    wrela_machine::VCPUS
                )));
            }
            placements.push(ReportPlacement {
                id: id.to_string(),
                type_name: type_name.to_string(),
                core: core as usize,
            });
        } else if let Some(rest) = line.strip_prefix("Actor ") {
            // Two spellings reach this VMM: the full report's
            // `Actor index=N type=Name` and the layout section's
            // `Actor name=Name mailbox=...`. Index roots are the Placement
            // set's exact cover; bare names are still accepted as ids.
            let fields = parse_report_fields(
                "Actor",
                rest,
                &["index", "type", "name", "mailbox", "slot", "frame", "state"],
            )?;
            if let Some(index) = fields.get("index").copied() {
                let n: u64 = index.parse().map_err(|e| {
                    VmmError::MalformedReport(format!("`Actor` field `index={index}`: {e}"))
                })?;
                let type_name = fields.get("type").copied().ok_or_else(|| {
                    VmmError::MalformedReport(
                        "`Actor index=` is missing required field `type`".to_string(),
                    )
                })?;
                declared_roots.push(DeclaredRoot {
                    id: format!("actor#{n}"),
                    type_name: type_name.to_string(),
                });
            } else if let Some(name) = fields.get("name").copied() {
                layout_root_names.push(name.to_string());
            } else {
                return Err(VmmError::MalformedReport(
                    "`Actor` line names neither `index=` nor `name=`".to_string(),
                ));
            }
        } else if let Some(rest) = line.strip_prefix("Driver ") {
            let fields = parse_report_fields(
                "Driver",
                rest,
                &["index", "type", "name", "mailbox", "slot", "frame", "state"],
            )?;
            if let Some(index) = fields.get("index").copied() {
                let n: u64 = index.parse().map_err(|e| {
                    VmmError::MalformedReport(format!("`Driver` field `index={index}`: {e}"))
                })?;
                let type_name = fields.get("type").copied().ok_or_else(|| {
                    VmmError::MalformedReport(
                        "`Driver index=` is missing required field `type`".to_string(),
                    )
                })?;
                declared_roots.push(DeclaredRoot {
                    id: format!("driver#{n}"),
                    type_name: type_name.to_string(),
                });
            } else if let Some(name) = fields.get("name").copied() {
                // A messageable driver is a ring target like any mailbox
                // root, so its layout-section name belongs in the same
                // pool invariant (9) checks against.
                layout_root_names.push(name.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("BlkDevice ") {
            let fields = parse_report_fields(
                "BlkDevice",
                rest,
                &["device", "capacity_sectors", "features", "vector"],
            )?;
            if blk_device.is_some() {
                return Err(VmmError::MalformedReport(
                    "more than one `BlkDevice` line (06 §6: the device set is closed and there is no hotplug; machine v1 has exactly one `blk`)".to_string(),
                ));
            }
            let vector = match fields.get("vector") {
                Some(_) => Some(report_u64("BlkDevice", &fields, "vector")?),
                None => None,
            };
            blk_device = Some((
                report_device_index("BlkDevice", &fields)?,
                report_u64("BlkDevice", &fields, "capacity_sectors")?,
                report_u64("BlkDevice", &fields, "features")?,
                vector,
            ));
        } else if let Some(rest) = line.strip_prefix("BlkQueue ") {
            let fields = parse_report_fields(
                "BlkQueue",
                rest,
                &["index", "size", "desc", "avail", "used", "doorbell"],
            )?;
            let index = report_u64("BlkQueue", &fields, "index")?;
            if index != 0 {
                return Err(VmmError::MalformedReport(format!(
                    "`BlkQueue index={index}`: machine v1's `blk` has exactly one queue (index 0)"
                )));
            }
            if blk_queue.is_some() {
                return Err(VmmError::MalformedReport(
                    "more than one `BlkQueue index=0` line".to_string(),
                ));
            }
            let size = report_u64("BlkQueue", &fields, "size")?;
            let size = u16::try_from(size).map_err(|_| {
                VmmError::MalformedReport(format!(
                    "`BlkQueue size={size}` does not fit virtio's own 16-bit queue depth"
                ))
            })?;
            blk_queue = Some(devices::BlkQueueConfig {
                size,
                desc: report_u64("BlkQueue", &fields, "desc")?,
                avail: report_u64("BlkQueue", &fields, "avail")?,
                used: report_u64("BlkQueue", &fields, "used")?,
                doorbell: report_u64("BlkQueue", &fields, "doorbell")?,
            });
        } else if let Some(rest) = line.strip_prefix("BlkPool ") {
            let fields = parse_report_fields("BlkPool", rest, &["name", "device", "base", "size"])?;
            let name = fields.get("name").copied().ok_or_else(|| {
                VmmError::MalformedReport("`BlkPool` is missing required field `name`".to_string())
            })?;
            blk_pools.push(devices::PoolWindow {
                name: name.to_string(),
                device: report_device_index("BlkPool", &fields)?,
                base: report_u64("BlkPool", &fields, "base")?,
                size: report_u64("BlkPool", &fields, "size")?,
            });
        } else if let Some(rest) = line.strip_prefix("IrqHostInject ") {
            // plans/M7.md item G: host `interrupt_status` writer + vector
            // raise. Applied before the vCPU runs so the guest's first
            // checkpoint delivers a status word the guest did not produce.
            let fields = parse_report_fields(
                "IrqHostInject",
                rest,
                &["base", "offset", "status", "vector"],
            )?;
            let status = report_u64("IrqHostInject", &fields, "status")?;
            let status = u32::try_from(status).map_err(|_| {
                VmmError::MalformedReport(format!(
                    "`IrqHostInject status={status:#x}` does not fit a u32 register"
                ))
            })?;
            irq_injects.push(IrqHostInject {
                base: report_u64("IrqHostInject", &fields, "base")?,
                offset: report_u64("IrqHostInject", &fields, "offset")?,
                status,
                vector: report_u64("IrqHostInject", &fields, "vector")?,
            });
        }
    }
    let revision = revision
        .ok_or_else(|| VmmError::MalformedReport("no `Machine revision=` line".to_string()))?;
    if revision != wrela_machine::MACHINE_REVISION_STR {
        return Err(VmmError::MachineRevisionMismatch {
            report: revision,
            vmm: wrela_machine::MACHINE_REVISION_STR,
        });
    }
    if !has_input {
        return Err(VmmError::MalformedReport(
            "no `Input path=` digest line".to_string(),
        ));
    }
    if sections.is_empty() {
        return Err(VmmError::MalformedReport(
            "no `Section name=` line".to_string(),
        ));
    }
    let entry =
        entry.ok_or_else(|| VmmError::MalformedReport("no `Entry base=0x...` line".to_string()))?;
    // The three `Blk*` line kinds are all-or-nothing: a device with no
    // queue, a queue with no device, or either with no pool is a report
    // this VMM refuses outright rather than booting on a device model it
    // would have to guess the shape of.
    let blk = match (blk_device, blk_queue) {
        (None, None) => {
            if !blk_pools.is_empty() {
                return Err(VmmError::MalformedReport(
                    "`BlkPool` line(s) with no `BlkDevice`/`BlkQueue` to bind them to".to_string(),
                ));
            }
            None
        }
        (Some(_), None) => {
            return Err(VmmError::MalformedReport(
                "a `BlkDevice` line with no `BlkQueue index=0` line".to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(VmmError::MalformedReport(
                "a `BlkQueue` line with no `BlkDevice` line".to_string(),
            ));
        }
        (Some((device, capacity_sectors, features, vector)), Some(queue)) => {
            // plans/M8.md item P: per *this* device. A `BlkPool` naming
            // some other device is a declared window this model may not
            // reach — it is carried into `GuestMem` (which is what makes
            // the refusal observable), but it cannot stand in for the
            // device's own bound pool.
            if !blk_pools.iter().any(|p| p.device == device) {
                return Err(VmmError::MalformedReport(format!(
                    "a `BlkDevice device=device#{device}` with no `BlkPool device=device#{device}` \
                     line: all memory a device can reach originates from its bound pools \
                     (03-hardware.md §3)"
                )));
            }
            Some(devices::BlkConfig {
                device,
                capacity_sectors,
                features,
                vector,
                queue,
                pools: blk_pools,
            })
        }
    };
    validate_report_invariants(
        entry,
        &mut core_entries,
        &sections,
        &request_rings,
        &ring_ranges,
        &placements,
        &declared_roots,
        &layout_root_names,
    )?;
    Ok(ParsedReport {
        entry,
        blk,
        irq_injects,
        core_entries,
        request_rings,
    })
}

/// Set-level invariants over the report's own tables (plans/M8.md item H
/// Target A follow-up). Per-line shape checks live in `parse_report`; this
/// function is the one place that validates the **set**. The list below is
/// the contract — keep it and the body in lockstep:
///
/// 1. every `Section` is pairwise disjoint from every other `Section`;
/// 2. `CoreEntry` lines are contiguous from core 1;
/// 3. every `CoreEntry` base is 4-byte aligned (an AArch64 PC is; a report
///    that says otherwise is forged) and distinct from every other core's
///    (including core 0's `Entry base=`);
/// 4. every `CoreEntry` base lands inside an executable section
///    (`rtcode` / `code` / `entry`);
/// 5. every request `Ring` names only cores this image brings up;
/// 6. `Ring` ranges are pairwise disjoint, disjoint from every per-core
///    stack, and wholly inside `rtdata` (so also disjoint from every other
///    `Section`); when the report has no `rtdata`, a ring may not overlap
///    any declared `Section`;
/// 7. declared `Actor index=` / `Driver index=` ids are unique;
/// 8. when `Placement` lines are present: ids are unique (an actor is
///    placed exactly once), each `core=` is brought up, each `type=`
///    agrees with the declared root, and every declared root has exactly
///    one `Placement`.
fn validate_report_invariants(
    entry: u64,
    core_entries: &mut Vec<(usize, u64)>,
    sections: &[ReportSection],
    request_rings: &[RequestRing],
    ring_ranges: &[RingRange],
    placements: &[ReportPlacement],
    declared_roots: &[DeclaredRoot],
    layout_root_names: &[String],
) -> Result<(), VmmError> {
    // (1) Sections are pairwise disjoint.
    for (i, a) in sections.iter().enumerate() {
        for b in sections.iter().skip(i + 1) {
            if a.base < b.end() && b.base < a.end() {
                return Err(VmmError::MalformedReport(format!(
                    "`Section name={} base={:#x} size={}` overlaps `Section name={} base={:#x} \
                     size={}`",
                    a.name, a.base, a.size, b.name, b.base, b.size
                )));
            }
        }
    }

    // (2) Contiguous secondary-core set from core 1.
    core_entries.sort_by_key(|(c, _)| *c);
    for (i, (core, _)) in core_entries.iter().enumerate() {
        if *core != i + 1 {
            return Err(VmmError::MalformedReport(format!(
                "`CoreEntry` lines are not contiguous from core 1 (saw core {core} where core {} \
                 was expected)",
                i + 1
            )));
        }
    }

    // (3) Every CoreEntry base is 4-byte aligned and distinct from every
    // other core's entry (including core 0's `Entry base=`).
    if entry % 4 != 0 {
        return Err(VmmError::MalformedReport(format!(
            "`Entry base={entry:#x}` is not 4-byte aligned (an AArch64 PC must be)"
        )));
    }
    for (core, base) in core_entries.iter() {
        if base % 4 != 0 {
            return Err(VmmError::MalformedReport(format!(
                "`CoreEntry core={core} base={base:#x}` is not 4-byte aligned (an AArch64 PC must \
                 be; a report that says otherwise is forged)"
            )));
        }
        if *base == entry {
            return Err(VmmError::MalformedReport(format!(
                "`CoreEntry core={core} base={base:#x}` equals core 0's `Entry base=` — two cores \
                 cannot enter at the same address"
            )));
        }
    }
    for (i, (c_a, b_a)) in core_entries.iter().enumerate() {
        for (c_b, b_b) in core_entries.iter().skip(i + 1) {
            if b_a == b_b {
                return Err(VmmError::MalformedReport(format!(
                    "`CoreEntry core={c_a} base={b_a:#x}` and `CoreEntry core={c_b} base={b_b:#x}` \
                     name the same entry address — two cores cannot enter at the same address"
                )));
            }
        }
    }

    // (4) Every CoreEntry lands in an executable section.
    const EXEC_SECTIONS: &[&str] = &["rtcode", "code", "entry"];
    for (core, base) in core_entries.iter() {
        let owner = sections.iter().find(|s| s.contains(*base));
        match owner {
            Some(s) if EXEC_SECTIONS.contains(&s.name.as_str()) => {}
            Some(s) => {
                return Err(VmmError::MalformedReport(format!(
                    "`CoreEntry core={core} base={base:#x}` falls inside `Section name={}` — a \
                     secondary entry must be code (`rtcode`, or a test image's `entry`/`code` \
                     harness), not data",
                    s.name
                )));
            }
            None => {
                return Err(VmmError::MalformedReport(format!(
                    "`CoreEntry core={core} base={base:#x}` is outside every `Section` this report \
                     declares — a secondary entry must be code"
                )));
            }
        }
    }

    // (5) Request rings name only brought-up cores.
    for r in request_rings {
        let brought_up = r.dst == 0 || core_entries.iter().any(|(c, _)| *c == r.dst);
        let src_up = r.src == 0 || core_entries.iter().any(|(c, _)| *c == r.src);
        if !brought_up || !src_up {
            return Err(VmmError::MalformedReport(format!(
                "`Ring kind=request src={} dst={} target={}` names a core this image never brings \
                 up (no `CoreEntry` line for it)",
                r.src, r.dst, r.target
            )));
        }
    }

    // (6) Ring ranges: pairwise disjoint; disjoint from stacks; disjoint
    // from every Section other than `rtdata` (and wholly inside `rtdata`
    // when that section exists).
    for (i, a) in ring_ranges.iter().enumerate() {
        for b in ring_ranges.iter().skip(i + 1) {
            if a.base < b.end() && b.base < a.end() {
                return Err(VmmError::MalformedReport(format!(
                    "`Ring kind={} src={} dst={} target={} base={:#x} bytes={}` overlaps \
                     `Ring kind={} src={} dst={} target={} base={:#x} bytes={}`",
                    a.kind,
                    a.src,
                    a.dst,
                    a.target,
                    a.base,
                    a.bytes,
                    b.kind,
                    b.src,
                    b.dst,
                    b.target,
                    b.base,
                    b.bytes
                )));
            }
        }
        for core in 0..wrela_machine::VCPUS {
            let stack_base = wrela_machine::layout::core_stack_base(core);
            let stack_end = stack_base + wrela_machine::layout::CORE_STACK_SIZE;
            if a.base < stack_end && stack_base < a.end() {
                return Err(VmmError::MalformedReport(format!(
                    "`Ring kind={} src={} dst={} target={} base={:#x} bytes={}` overlaps \
                     core {core}'s stack [{stack_base:#x}..{stack_end:#x})",
                    a.kind, a.src, a.dst, a.target, a.base, a.bytes
                )));
            }
        }
        if let Some(rtdata) = sections.iter().find(|s| s.name == "rtdata") {
            if a.base < rtdata.base || a.end() > rtdata.end() {
                return Err(VmmError::MalformedReport(format!(
                    "`Ring kind={} src={} dst={} target={} base={:#x} bytes={}` is not wholly \
                     inside `Section name=rtdata base={:#x} size={}` (rings live in `rtdata` only)",
                    a.kind, a.src, a.dst, a.target, a.base, a.bytes, rtdata.base, rtdata.size
                )));
            }
        } else {
            for s in sections {
                if a.base < s.end() && s.base < a.end() {
                    return Err(VmmError::MalformedReport(format!(
                        "`Ring kind={} src={} dst={} target={} base={:#x} bytes={}` overlaps \
                         `Section name={} base={:#x} size={}` (rings live in `rtdata` only)",
                        a.kind, a.src, a.dst, a.target, a.base, a.bytes, s.name, s.base, s.size
                    )));
                }
            }
        }
    }

    // (7) Declared Actor/Driver index= ids are unique.
    for (i, a) in declared_roots.iter().enumerate() {
        for b in declared_roots.iter().skip(i + 1) {
            if a.id == b.id {
                return Err(VmmError::MalformedReport(format!(
                    "declared root `{}` is repeated",
                    a.id
                )));
            }
        }
    }

    // (9) Every request ring's `target=` names a root this report
    // declares. A ring is the delivery path into a mailbox, so a target
    // no `Actor`/`Driver` line accounts for is a forged edge — the same
    // set-level defect as a repeated `Placement id=`, one field over.
    // A reply ring carries `target=-` (it delivers back to its caller,
    // not into a named mailbox) and is exempt by that spelling.
    if !declared_roots.is_empty() || !layout_root_names.is_empty() {
        for r in ring_ranges {
            if r.target == "-" {
                continue;
            }
            let known = declared_roots.iter().any(|d| d.type_name == r.target)
                || layout_root_names.iter().any(|n| n == &r.target);
            if !known {
                let mut declared: Vec<&str> = declared_roots
                    .iter()
                    .map(|d| d.type_name.as_str())
                    .chain(layout_root_names.iter().map(|s| s.as_str()))
                    .collect();
                declared.sort_unstable();
                declared.dedup();
                return Err(VmmError::MalformedReport(format!(
                    "`Ring kind={} src={} dst={} target={}` names a root this report never \
                     declares (known roots: {}) — a ring is the delivery path into a mailbox, \
                     so a target no `Actor`/`Driver` line accounts for is a forged edge",
                    r.kind,
                    r.src,
                    r.dst,
                    r.target,
                    declared.join(", ")
                )));
            }
        }
    }

    // (8) Placement set — only when Placement lines are present (the
    // VMM-facing subset from `append_vmm_runtime_lines` emits none).
    if placements.is_empty() {
        return Ok(());
    }

    for (i, a) in placements.iter().enumerate() {
        for b in placements.iter().skip(i + 1) {
            if a.id == b.id {
                return Err(VmmError::MalformedReport(format!(
                    "`Placement id={}` is repeated (an actor/driver is placed exactly once; two \
                     lines would put the same root on cores {} and {})",
                    a.id, a.core, b.core
                )));
            }
        }
    }

    for p in placements {
        let core_up = p.core == 0 || core_entries.iter().any(|(c, _)| *c == p.core);
        if !core_up {
            return Err(VmmError::MalformedReport(format!(
                "`Placement id={} core={}` names a core this image never brings up (no \
                 `CoreEntry` line for it; core 0 is the `Entry base=` line)",
                p.id, p.core
            )));
        }

        if let Some(root) = declared_roots.iter().find(|r| r.id == p.id) {
            if root.type_name != p.type_name {
                return Err(VmmError::MalformedReport(format!(
                    "`Placement id={} type={}` disagrees with the declared root's `type={}`",
                    p.id, p.type_name, root.type_name
                )));
            }
        } else if layout_root_names.iter().any(|n| n == &p.id) {
            // Bare-name Placement against a layout-section Actor name=.
        } else if !declared_roots.is_empty() || !layout_root_names.is_empty() {
            let declared: Vec<&str> = declared_roots
                .iter()
                .map(|r| r.id.as_str())
                .chain(layout_root_names.iter().map(|s| s.as_str()))
                .collect();
            return Err(VmmError::MalformedReport(format!(
                "`Placement id={}` names an actor this report's `Actor` lines do not \
                 declare (declared: {declared:?})",
                p.id
            )));
        }
    }

    if !declared_roots.is_empty() {
        for root in declared_roots {
            let n = placements.iter().filter(|p| p.id == root.id).count();
            if n == 0 {
                return Err(VmmError::MalformedReport(format!(
                    "declared root `{}` (type={}) has no `Placement` line — every Actor/Driver \
                     is placed exactly once",
                    root.id, root.type_name
                )));
            }
            let _ = n;
        }
    }

    Ok(())
}

/// Boots `img_path` (a flat blob, loaded at `wrela_machine::layout::
/// IMAGE_BASE`, exactly as `layout::layout_test_image`/`layout_program`
/// emit it) under `report_path`'s own declared configuration, on
/// Hypervisor.framework — the VMM's whole public entry point (plans/M5.md
/// item E).
///
/// **Every one of guest DRAM's `DRAM_SIZE` (1 GiB) bytes is zeroed before
/// the vCPU ever runs** (`std::alloc::alloc_zeroed`) — 06 §3's own "zeroes
/// the declared reservations" boot step, and not merely a tidiness
/// convention: chasing an unrelated test bug during this item's own
/// development (`wrela-compiler/src/layout.rs`'s `harness_jit` module doc)
/// found that a write from freshly-JIT'd/executed code to a never-before-
/// touched anonymous page was not reliably observable without the page
/// having been faulted in first — pre-zeroing the whole reservation here
/// removes any question of whether that same class of issue could ever
/// affect a real guest write to a freshly mapped page.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn boot_image(report_path: &Path, img_path: &Path) -> Result<BootOutcome, VmmError> {
    boot_image_core(report_path, img_path, None, None).map(|(outcome, _divergences)| outcome)
}

/// The shared boot core (plans/M5.md item F, grown by plans/M6.md item E
/// into the choice-sequence shape): identical to `boot_image` above in
/// every respect but two extra parameters.
///
/// `replay_choices`: `None` (live/record mode) or `Some(log)` (replay
/// mode) — every nondeterministic decision this boot's exit loop makes
/// (a clock read, a deadline park's own wake) flows through exactly one
/// `record::Chooser::choose_next` call (decision 9's own single-point-of-
/// choice mandate), which either produces a fresh live value (recording
/// it) or consumes the next tagged entry from `log` (replaying it,
/// diverging loudly — via the second return value — on a tag mismatch or
/// underrun, never a panic and never a silently wrong value). `boot_image`
/// itself is simply this function called with `replay_choices: None`.
///
/// `test_delayed_raise`: `cfg(test)`-only conformance seam (this crate's
/// own tests module is the only caller — plans/M6.md item E's
/// conformance test (a), "vector raise observed at a checkpoint"): after
/// `(delay, vector_bit)`, a background host thread stores `vector_bit`
/// directly into this core's own pending word — a raw host-side memory
/// write, no vCPU exit involved, exactly modeling "the VMM raises a
/// vector while the guest is actively running, not parked" (06 §4's own
/// store-half of "a store-release plus a wake" — no wake is needed here
/// since the vCPU was never parked). This path is **not** itself
/// recorded/replayed (a host-timing-dependent raise cannot be replayed
/// deterministically without a virtual clock this milestone does not
/// have) — it exists purely to prove the checkpoint-service dispatch
/// mechanism honestly, since M6-E's only *real* mid-run vector producer
/// (an expired group's deadline while its target is still running) is
/// item F's own job. Always `None` on every production call site
/// (`boot_image`, `record::record`, `record::replay`).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn boot_image_core(
    report_path: &Path,
    img_path: &Path,
    replay_choices: Option<Vec<record::ChoiceEntry>>,
    test_delayed_raise: Option<(Duration, u64)>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    use hv::*;
    use std::alloc::{Layout, alloc_zeroed, dealloc};
    use std::ffi::c_void;
    use wrela_machine::layout as machine_layout;
    use wrela_machine::machine_info;

    let report_text = std::fs::read_to_string(report_path)
        .map_err(|e| VmmError::Io(format!("read {}: {e}", report_path.display())))?;
    let parsed = parse_report(&report_text)?;
    let img = std::fs::read(img_path)
        .map_err(|e| VmmError::Io(format!("read {}: {e}", img_path.display())))?;

    let image_off = machine_layout::IMAGE_BASE - machine_layout::DRAM_BASE;
    if image_off + (img.len() as u64) > machine_layout::DRAM_SIZE {
        return Err(VmmError::BadImage(format!(
            "image ({} bytes at offset {:#x}) does not fit the {} byte DRAM reservation",
            img.len(),
            image_off,
            machine_layout::DRAM_SIZE
        )));
    }

    // --- hv_vm_create --------------------------------------------------------
    let r = unsafe { hv_vm_create(std::ptr::null_mut()) };
    if r != HV_SUCCESS {
        return Err(VmmError::Hvf {
            call: "hv_vm_create",
            code: r,
        });
    }
    // From here on, every early-return must still run `hv_vm_destroy` — a
    // small RAII guard makes that automatic instead of repeated at every
    // `return Err`.
    struct VmGuard;
    impl Drop for VmGuard {
        fn drop(&mut self) {
            unsafe {
                hv_vm_destroy();
            }
        }
    }
    let _vm_guard = VmGuard;

    // --- host DRAM allocation (zeroed, 16 KiB aligned — Apple Silicon's own
    // page size, plenty for hv_vm_map's own alignment requirement) -----------
    const PAGE_ALIGN: usize = 16 * 1024;
    let dram_size = machine_layout::DRAM_SIZE as usize;
    let layout = Layout::from_size_align(dram_size, PAGE_ALIGN)
        .map_err(|e| VmmError::BadImage(format!("bad DRAM layout: {e}")))?;
    let host_ram = unsafe { alloc_zeroed(layout) };
    if host_ram.is_null() {
        return Err(VmmError::BadImage(
            "failed to allocate the guest DRAM reservation".to_string(),
        ));
    }
    struct RamGuard {
        ptr: *mut u8,
        layout: Layout,
    }
    impl Drop for RamGuard {
        fn drop(&mut self) {
            unsafe {
                dealloc(self.ptr, self.layout);
            }
        }
    }
    let _ram_guard = RamGuard {
        ptr: host_ram,
        layout,
    };

    let r = unsafe {
        hv_vm_map(
            host_ram as *mut c_void,
            machine_layout::DRAM_BASE,
            dram_size,
            HV_MEMORY_READ | HV_MEMORY_WRITE | HV_MEMORY_EXEC,
        )
    };
    if r != HV_SUCCESS {
        return Err(VmmError::Hvf {
            call: "hv_vm_map",
            code: r,
        });
    }

    // --- load the image + machine-info page ----------------------------------
    unsafe {
        std::ptr::copy_nonoverlapping(img.as_ptr(), host_ram.add(image_off as usize), img.len());
    }
    let info_off = (machine_layout::MACHINE_INFO_BASE - machine_layout::DRAM_BASE) as usize;
    unsafe {
        let rev_bytes = wrela_machine::MACHINE_REVISION_STR.as_bytes();
        std::ptr::copy_nonoverlapping(
            rev_bytes.as_ptr(),
            host_ram.add(info_off + machine_info::OFF_REVISION as usize),
            rev_bytes.len(),
        );
        // Wall-clock seed: 0, deterministic at M5 (plans/M5.md item E's
        // own boot-path note — recorded here, not merely implied).
        std::ptr::write_bytes(
            host_ram.add(info_off + machine_info::OFF_WALL_SEED as usize),
            0,
            8,
        );
    }

    // --- device model + boot-time injections, before any vCPU runs --------
    // Establish the monotonic epoch before the guest's first instruction,
    // so `now()` measures from the machine coming up rather than from
    // whichever guest read happened to be first (`monotonic_ns`'s own doc).
    let _ = monotonic_ns();
    // plans/M7.md item F: the declared `blk` device model, preconfigured
    // from the report (06 §3) before the vCPU ever runs. `None` unless the
    // report declares one, which nothing the compiler emits does yet — so
    // every existing image boots down exactly the path it did before.
    let blk: Option<BlkState> = match parsed.blk {
        None => None,
        Some(cfg) => {
            let pools = cfg.pools.clone();
            let vector = cfg.vector;
            let device_index = cfg.device;
            let device = devices::BlkDevice::new(cfg).map_err(VmmError::BadImage)?;
            // plans/M8.md item P: the view is *this device's*. Every
            // declared window goes in; only the ones bound to
            // `device_index` are reachable through it.
            let mem = unsafe { devices::GuestMem::new(host_ram, pools, device_index) }
                .map_err(VmmError::BadImage)?;
            // Completion-time `interrupt_status` writer: same GPA the
            // boot-time `IrqHostInject` names, when this device owns a
            // vector. Plans/M7.md item E4: the ISR masks bit 0 after a
            // real used-ring completion, not only the one-shot boot inject.
            let irq_status_gpa = vector.and_then(|v| {
                parsed
                    .irq_injects
                    .iter()
                    .find_map(|inj| (inj.vector == v).then_some(inj.base.checked_add(inj.offset)?))
            });
            Some(BlkState {
                device,
                mem,
                irq_status_gpa,
            })
        }
    };
    // plans/M7.md item G: write `interrupt_status` then raise the vector
    // before the guest's first instruction. The status value is the
    // compiler's `IRQ_HOST_STATUS_MAGIC` — a word the zeroed reservation
    // cannot produce — so an ISR that asserts equality has proved the
    // host write, not a vacuous zero read.
    for inj in &parsed.irq_injects {
        let guest = inj.base.checked_add(inj.offset).ok_or_else(|| {
            VmmError::BadImage(format!(
                "IrqHostInject base={:#x}+offset={:#x} overflows",
                inj.base, inj.offset
            ))
        })?;
        if guest < machine_layout::DRAM_BASE {
            return Err(VmmError::BadImage(format!(
                "IrqHostInject address {guest:#x} is below DRAM_BASE"
            )));
        }
        let off = (guest - machine_layout::DRAM_BASE) as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(inj.status.to_le_bytes().as_ptr(), host_ram.add(off), 4);
        }
        raise_vector(host_ram, inj.vector);
    }

    // --- plans/M6.md item E's own conformance-only seam: a delayed,
    // host-side raise (module doc above) — absent (`None`) on every
    // production path. `host_ram` is a raw pointer into this fn's own
    // `alloc_zeroed` reservation, alive for the whole fn body (`_ram_guard`
    // frees it only on return) — a plain byte store into it from another
    // host thread is exactly as safe as the main thread's own later
    // `drain_console`/park-handling reads of the identical region, wrapped
    // only so `std::thread::spawn` accepts the raw pointer at all.
    struct SendPtr(*mut u8);
    unsafe impl Send for SendPtr {}
    /// Joins the raiser thread on drop — guarantees it is finished (and
    /// therefore never touches `host_ram` again) before `_ram_guard`
    /// (declared earlier, dropped later — Rust drops in reverse
    /// declaration order) deallocates the reservation this thread writes
    /// into.
    struct RaiseGuard(Option<std::thread::JoinHandle<()>>);
    impl Drop for RaiseGuard {
        fn drop(&mut self) {
            if let Some(h) = self.0.take() {
                let _ = h.join();
            }
        }
    }
    let _raise_guard = RaiseGuard(test_delayed_raise.map(|(delay, vector_bit)| {
        let ptr = SendPtr(host_ram);
        let raise_pending_off =
            (wrela_machine::pending::core_word_addr(0) - machine_layout::DRAM_BASE) as usize;
        std::thread::spawn(move || {
            // `let ptr = ptr;` forces the closure to capture the whole
            // `SendPtr` value (Rust 2021's disjoint-field capture would
            // otherwise capture only the inner `*mut u8` field directly,
            // which is not `Send` on its own — `SendPtr`'s own `unsafe
            // impl Send` only helps if the wrapper itself is what gets
            // captured).
            let ptr = ptr;
            std::thread::sleep(delay);
            let SendPtr(base) = ptr;
            unsafe {
                std::ptr::copy_nonoverlapping(
                    vector_bit.to_le_bytes().as_ptr(),
                    base.add(raise_pending_off),
                    8,
                );
            }
        })
    }));

    // --- the three vCPUs (plans/M8.md item C1, decision 11) ----------------
    //
    // 06-machine.md §1 gives this machine three vCPUs, and §3 makes core 0's
    // own entry "release the other vCPUs". Hypervisor.framework binds a vCPU
    // to the thread that created it, so there are three host threads — but
    // exactly one of them is inside `hv_vcpu_run` at any instant, because
    // they pass a single **baton** whose hand-off order is a pure function of
    // guest-visible state: which cores the guest has released, which have
    // parked, and what their own pending words hold. Nothing in `next_core`
    // below reads a host clock, a thread id, or an address to decide who runs
    // next — otherwise `xtask repro` would be measuring the host's scheduler,
    // and 06 §8's enumerable choice sequence would have quietly become an
    // opaque interleaving trace (decision 11's own rejected alternative).
    //
    // The baton changes hands at exactly two guest actions, both of them
    // things the guest itself does and a recording can therefore replay:
    // the release doorbell (core 0 hands off to each released core in
    // ascending order) and a park (a core with nothing ready hands off).
    // Every other exit keeps the baton, which is why a single-core image —
    // where cores 1 and 2 are never released and the release store is never
    // even emitted — runs down exactly the path it ran before this item.
    const NCORES: usize = wrela_machine::VCPUS;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum CoreState {
        /// Created and register-initialized, never released by the guest.
        Unreleased,
        /// Eligible for the baton.
        Runnable,
        /// Parked at `mmio::PARK_MMIO_ADDR` with nothing of its own to run.
        /// Runnable again only when its own pending word is nonzero — the
        /// mask-arm-recheck discipline, read out of guest memory rather than
        /// remembered host-side.
        Parked,
        /// Its loop has ended (the boot finished, faulted, or timed out).
        Finished,
    }

    struct Sched {
        /// Whose turn it is. Only this core may be inside `hv_vcpu_run`.
        current: usize,
        state: [CoreState; NCORES],
        /// The boot is over (halt, fault, or timeout); every core returns.
        done: bool,
    }

    struct Shared {
        sched: Sched,
        chooser: record::Chooser,
        blk: Option<BlkState>,
        exits: u64,
        exit_code: Option<u64>,
        /// The first failure any core reported — a boot fails closed on the
        /// first one, it never reports a partial transcript as success.
        error: Option<VmmError>,
        /// Live vCPU handles, for the watchdog's own `hv_vcpus_exit`. A core
        /// clears its own slot **under this lock, strictly before**
        /// destroying its vCPU, so the watchdog can never force-exit a
        /// handle that no longer exists.
        vcpus: [u64; NCORES],
        /// Did the guest ring the release doorbell? Recorded rather than
        /// inferred from `sched.state`, which is `Finished` for every core
        /// by the time the marks are checked and so cannot say whether a
        /// core was ever released.
        released: bool,
        /// plans/M8.md item C3: the cross-core admission witness (06 §8).
        /// Empty `rings` for every single-core image, which is what makes
        /// their choice sequences byte-identical to their pre-C3 ones.
        admission: AdmissionWitness,
    }
    // Every field above is touched only by the thread currently holding the
    // baton (or by the main thread, before any core runs and after all have
    // finished); the `Mutex` is what publishes those writes across threads.
    unsafe impl Send for Shared {}

    /// Core `core`'s own pending-vector word, read straight out of guest
    /// memory — the only thing that can make a parked core runnable again,
    /// and guest-visible by construction (06 §4).
    fn pending_word(host_ram: *const u8, core: usize) -> u64 {
        let off =
            (wrela_machine::pending::core_word_addr(core) - machine_layout::DRAM_BASE) as usize;
        let mut b = [0u8; 8];
        unsafe { std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 8) };
        u64::from_le_bytes(b)
    }

    /// The baton's whole hand-off rule: the next core after `from`, in
    /// ascending core order (wrapping, so a lone runnable core hands the
    /// baton back to itself), that guest-visible state says can run.
    fn next_core(sched: &mut Sched, from: usize, host_ram: *const u8) -> Option<usize> {
        for step in 1..=NCORES {
            let c = (from + step) % NCORES;
            match sched.state[c] {
                CoreState::Runnable => return Some(c),
                CoreState::Parked if pending_word(host_ram, c) != 0 => {
                    sched.state[c] = CoreState::Runnable;
                    return Some(c);
                }
                _ => {}
            }
        }
        None
    }

    /// What a handled exit asks of the baton.
    enum Step {
        /// Ordinary exit — this core keeps running.
        Keep,
        /// This core volunteers the machine (release, or a park).
        Yield,
        /// The guest's exit protocol: the image is done.
        Halt(u64),
    }

    let cores_declared = 1 + parsed.core_entries.len();
    let shared = std::sync::Mutex::new(Shared {
        sched: Sched {
            current: 0,
            state: {
                let mut s = [CoreState::Unreleased; NCORES];
                s[0] = CoreState::Runnable;
                s
            },
            done: false,
        },
        released: false,
        chooser: match replay_choices {
            Some(log) => record::Chooser::replayer(log),
            None => record::Chooser::recorder(),
        },
        blk,
        exits: 0,
        exit_code: None,
        error: None,
        vcpus: [0; NCORES],
        admission: AdmissionWitness::new(parsed.request_rings.clone()),
    });
    let baton = std::sync::Condvar::new();

    /// One vCPU exit, decoded and serviced on the core that took it. Every
    /// diagnostic here names its core: with three of them, "unhandled
    /// exception" without a core is a bug report missing its first fact.
    #[allow(clippy::too_many_arguments)]
    fn handle_exit(
        core: usize,
        vcpu: u64,
        exit_ptr: *const HvVcpuExit,
        host_ram: *mut u8,
        cores_declared: usize,
        lock: &std::sync::Mutex<Shared>,
    ) -> Result<Step, VmmError> {
        use wrela_machine::mmio;
        let deadline_off = (machine_layout::MACHINE_INFO_BASE - machine_layout::DRAM_BASE) as usize
            + wrela_machine::machine_info::OFF_NEXT_DEADLINE as usize;
        let exit = unsafe { *exit_ptr };
        match exit.reason {
            HV_EXIT_REASON_EXCEPTION => {
                let esr = exit.exception.syndrome;
                let ipa = exit.exception.physical_address;
                if ipa == mmio::EXIT_MMIO_ADDR {
                    let Some(da) = decode_data_abort(esr) else {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: unhandled access shape at EXIT_MMIO_ADDR (esr={esr:#x})"
                        )));
                    };
                    if !da.write {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: a load from EXIT_MMIO_ADDR is not part of the exit \
                             protocol"
                        )));
                    }
                    let value = match da.reg {
                        Some(reg) => {
                            let mut v = 0u64;
                            let r = unsafe { hv_vcpu_get_reg(vcpu, hv_reg_xn(reg), &mut v) };
                            if r != HV_SUCCESS {
                                return Err(VmmError::Hvf {
                                    call: "hv_vcpu_get_reg",
                                    code: r,
                                });
                            }
                            v
                        }
                        None => 0, // SRT == 31: XZR, architecturally zero.
                    };
                    Ok(Step::Halt(value))
                } else if ipa == mmio::CLOCK_MMIO_ADDR {
                    let Some(da) = decode_data_abort(esr) else {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: unhandled access shape at CLOCK_MMIO_ADDR (esr={esr:#x})"
                        )));
                    };
                    if da.write {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: a store to CLOCK_MMIO_ADDR is not part of the clock \
                             protocol"
                        )));
                    }
                    // plans/M6.md item E, decision 9: the single point of
                    // choice — record produces a fresh live read, replay
                    // consumes the next logged one (never re-reading the
                    // real clock).
                    let entry = {
                        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        g.chooser.choose_next(record::ChoiceRequest::ClockRead, || {
                            record::ChoiceEntry::ClockRead {
                                value: monotonic_ns(),
                            }
                        })
                    };
                    let record::ChoiceEntry::ClockRead { value: ns } = entry else {
                        unreachable!(
                            "choose_next(ClockRead, ..) always returns a ClockRead-shaped entry \
                             (a mismatched replay tag falls back to the request's own shape)"
                        )
                    };
                    if let Some(reg) = da.reg {
                        let r = unsafe { hv_vcpu_set_reg(vcpu, hv_reg_xn(reg), ns) };
                        if r != HV_SUCCESS {
                            return Err(VmmError::Hvf {
                                call: "hv_vcpu_set_reg",
                                code: r,
                            });
                        }
                    }
                    advance_pc(vcpu)?;
                    Ok(Step::Keep)
                } else if ipa == mmio::RELEASE_MMIO_ADDR {
                    // plans/M8.md item C1 / 06 §3: "the entry ... releases
                    // the other vCPUs". Everything about this store is
                    // checked rather than assumed — a machine that starts
                    // cores nobody asked it to start is exactly the
                    // silent-wrong-answer this item exists to remove.
                    let Some(da) = decode_data_abort(esr) else {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: unhandled access shape at RELEASE_MMIO_ADDR \
                             (esr={esr:#x})"
                        )));
                    };
                    if !da.write {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: a load from RELEASE_MMIO_ADDR is not part of the \
                             release protocol"
                        )));
                    }
                    if core != 0 {
                        return Err(VmmError::GuestFault(format!(
                            "core {core} rang the release doorbell: only the boot core releases \
                             the others (06-machine.md §3)"
                        )));
                    }
                    let value = match da.reg {
                        Some(reg) => {
                            let mut v = 0u64;
                            let r = unsafe { hv_vcpu_get_reg(vcpu, hv_reg_xn(reg), &mut v) };
                            if r != HV_SUCCESS {
                                return Err(VmmError::Hvf {
                                    call: "hv_vcpu_get_reg",
                                    code: r,
                                });
                            }
                            v
                        }
                        None => 0,
                    };
                    if value != cores_declared as u64 {
                        return Err(VmmError::GuestFault(format!(
                            "core 0 released {value} core(s) but this image's report declares \
                             {cores_declared} (one `Entry base=` plus {} `CoreEntry` line(s)) — \
                             the image and its report disagree about the machine",
                            cores_declared - 1
                        )));
                    }
                    advance_pc(vcpu)?;
                    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    for c in 1..cores_declared {
                        if g.sched.state[c] != CoreState::Unreleased {
                            return Err(VmmError::GuestFault(format!(
                                "core 0 rang the release doorbell twice (core {c} is already \
                                 {:?}) — release is a one-shot boot step",
                                g.sched.state[c]
                            )));
                        }
                        g.sched.state[c] = CoreState::Runnable;
                    }
                    g.released = true;
                    drop(g);
                    Ok(Step::Yield)
                } else if ipa == mmio::QUIESCE_MMIO_ADDR {
                    // plans/M8.md item F / decision 36, 03-hardware.md §9:
                    // "per-queue reset (when negotiated) or full reset
                    // establishes quiescence, and only then is memory
                    // reclaimed". The device is this VMM, so quiescence is
                    // a thing only this VMM can establish — the guest's
                    // reset traps here first, and the count word it will
                    // later gate a reclaim on is written *by the host*,
                    // after the model has actually stopped using the ring.
                    let Some(da) = decode_data_abort(esr) else {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: unhandled access shape at QUIESCE_MMIO_ADDR \
                             (esr={esr:#x})"
                        )));
                    };
                    if !da.write {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: a load from QUIESCE_MMIO_ADDR is not part of the \
                             quiesce protocol"
                        )));
                    }
                    let named = match da.reg {
                        Some(reg) => {
                            let mut v = 0u64;
                            let r = unsafe { hv_vcpu_get_reg(vcpu, hv_reg_xn(reg), &mut v) };
                            if r != HV_SUCCESS {
                                return Err(VmmError::Hvf {
                                    call: "hv_vcpu_get_reg",
                                    code: r,
                                });
                            }
                            v
                        }
                        None => 0,
                    };
                    {
                        let g = &mut *lock.lock().unwrap_or_else(|e| e.into_inner());
                        let Some(state) = g.blk.as_mut() else {
                            return Err(VmmError::GuestFault(format!(
                                "core {core} rang the quiesce doorbell, but this image declares \
                                 no `blk` device to quiesce (03-hardware.md §9)"
                            )));
                        };
                        let completions =
                            state
                                .device
                                .quiesce(&mut state.mem, named)
                                .map_err(|fault| {
                                    VmmError::GuestFault(format!("virtio-blk: {fault}"))
                                })?;
                        commit_completions(state, &mut g.chooser, &completions, host_ram)?;
                    }
                    advance_pc(vcpu)?;
                    Ok(Step::Keep)
                } else if ipa == mmio::PARK_MMIO_ADDR {
                    // plans/M6.md item E, decision 7/06 §5: the park
                    // protocol's own doorbell (`mmio::PARK_MMIO_ADDR`'s
                    // own module doc has the whole contract).
                    let Some(da) = decode_data_abort(esr) else {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: unhandled access shape at PARK_MMIO_ADDR (esr={esr:#x})"
                        )));
                    };
                    if !da.write {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: a load from PARK_MMIO_ADDR is not part of the park \
                             protocol"
                        )));
                    }
                    // Advance PC now — the guest resumes right after its
                    // own trapping store the moment this vCPU is next run,
                    // whether or not this park ends up sleeping at all.
                    advance_pc(vcpu)?;
                    if core != 0 {
                        // plans/M8.md item C1: a secondary core's park is a
                        // plain "nothing of mine is ready". It never sleeps
                        // on a deadline (`OFF_NEXT_DEADLINE` is the boot
                        // core's own park word, and no turn can arm a
                        // deadline on a core no message can reach yet) and
                        // it never spins: this core stops being scheduled
                        // until its own pending word is raised, which is
                        // item C2's cross-core wake.
                        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        g.sched.state[core] = CoreState::Parked;
                        drop(g);
                        return Ok(Step::Yield);
                    }
                    let deadline_ns = unsafe {
                        let mut b = [0u8; 8];
                        std::ptr::copy_nonoverlapping(
                            host_ram.add(deadline_off),
                            b.as_mut_ptr(),
                            8,
                        );
                        u64::from_le_bytes(b)
                    };
                    // plans/M8.md item C2, decision 31: in a cross-core image,
                    // core 0 parking with **no deadline armed** is exactly a
                    // secondary core's park — "nothing of mine is ready" — and
                    // is treated identically: this core stops being scheduled
                    // until its own pending word is raised. There is nothing to
                    // sleep until, so the deadline path below does not apply.
                    //
                    // **It is deliberately handled before the runnable-sibling
                    // shortcut below**, and that ordering is the whole reason a
                    // lost wake is catchable. Leaving core 0 `Runnable` because
                    // some sibling happened to be runnable would mean core 0
                    // gets the baton back whether or not anything ever woke it,
                    // so an omitted cross-core wake would still boot green — a
                    // decorative mechanism, found by mutating `waker_tag` and
                    // watching every golden pass. Marking it `Parked` puts its
                    // resumption where the machine's own contract puts it: on
                    // its pending word. `next_core` then either finds a runnable
                    // sibling, finds this core's own word already raised, or
                    // fails the boot closed with `no core is runnable` — which
                    // is what replaces the guest-side `DEADLOCK_MSG` for a
                    // cross-core image.
                    //
                    // Unreachable for a single-core image: its entry driver only
                    // parks when a deadline is armed, so every M5-M7 boot takes
                    // the untouched path below.
                    if cores_declared > 1 && deadline_ns == 0 {
                        let blk_completed = {
                            let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                            let g = &mut *g;
                            service_blk(&mut g.blk, &mut g.chooser, host_ram)?
                        };
                        // The mask-arm-recheck "recheck" half: a wake that
                        // landed between the guest's last look and this trap
                        // must not put the core to sleep.
                        if !blk_completed && pending_word(host_ram, core) == 0 {
                            let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                            g.sched.state[core] = CoreState::Parked;
                        }
                        return Ok(Step::Yield);
                    }
                    // Core 0, with a deadline armed. A sibling that can run gets
                    // the machine before this core considers sleeping the host
                    // thread — sleeping while another core is ready would be the
                    // baton deciding scheduling by host timing, which
                    // decision 11 forbids. With no runnable sibling (every
                    // single-core image, always) this is exactly the M6
                    // park path, unchanged.
                    {
                        let g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        let sibling = (1..NCORES).any(|c| {
                            g.sched.state[c] == CoreState::Runnable
                                || (g.sched.state[c] == CoreState::Parked
                                    && pending_word(host_ram, c) != 0)
                        });
                        if sibling {
                            drop(g);
                            return Ok(Step::Yield);
                        }
                    }
                    // plans/M7.md item F: the second doorbell poll site
                    // (06 §5). A completion serviced *here* — after the
                    // guest published and rang, before this VMM decides to
                    // sleep — is exactly the wake the mask-arm-recheck
                    // discipline exists to keep: a doorbell rung between
                    // the driver's last check and its park must never
                    // sleep the core that is waiting for it.
                    let blk_completed = {
                        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        let g = &mut *g;
                        service_blk(&mut g.blk, &mut g.chooser, host_ram)?
                    };
                    // The mask-arm-recheck discipline's own "recheck"
                    // half (`mmio::PARK_MMIO_ADDR`'s own doc): a vector
                    // already pending at the moment of this trap means a
                    // wake already happened (or was never needed) — do
                    // not sleep at all, so it is never lost.
                    let already_pending = pending_word(host_ram, core) != 0;
                    if !already_pending && !blk_completed {
                        {
                            let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                            g.chooser.choose_next(
                                record::ChoiceRequest::DeadlineWake { deadline_ns },
                                || {
                                    // The real, host-side sleep — never
                                    // invoked in replay mode (decision 9:
                                    // "sleep skipped under replay").
                                    let now = monotonic_ns();
                                    if deadline_ns > now {
                                        std::thread::sleep(Duration::from_nanos(deadline_ns - now));
                                    }
                                    record::ChoiceEntry::DeadlineWake { deadline_ns }
                                },
                            );
                            g.chooser.choose_next(
                                record::ChoiceRequest::VectorRaise { vector: 0 },
                                || record::ChoiceEntry::VectorRaise { vector: 0 },
                            );
                        }
                        // The raise itself (06 §4: "a store-release plus
                        // a wake"): a plain host-side write into this
                        // core's own pending word. No separate wake is
                        // needed — resuming this already-exited vCPU on
                        // the next loop iteration below *is* the wake.
                        raise_vector(host_ram, 0);
                    }
                    Ok(Step::Keep)
                } else if let Some(imm) = decode_brk(esr) {
                    let pc = read_pc(vcpu).unwrap_or(0);
                    Err(VmmError::GuestFault(format!(
                        "core {core}: unexpected `BRK #{imm}` (esr={esr:#x}, ipa={ipa:#x}, \
                         pc={pc:#x})"
                    )))
                } else {
                    let pc = read_pc(vcpu).unwrap_or(0);
                    let note = el1_exception_note(vcpu, pc);
                    Err(VmmError::GuestFault(format!(
                        "core {core}: unhandled exception (esr={esr:#x}, ipa={ipa:#x}, \
                         pc={pc:#x}){note}"
                    )))
                }
            }
            HV_EXIT_REASON_CANCELED => {
                // The watchdog force-exited every vCPU; whichever one was
                // inside `hv_vcpu_run` reports the hang, and names itself —
                // "which core hung" is the first thing a three-core hang
                // needs to say.
                let transcript_so_far = drain_console(host_ram);
                Err(VmmError::Timeout {
                    core,
                    transcript_so_far,
                })
            }
            other => Err(VmmError::GuestFault(format!(
                "core {core}: unexpected hv_exit_reason_t {other}"
            ))),
        }
    }

    /// One core's whole life: create nothing (its vCPU is already made on
    /// this thread), take the baton when it is this core's turn, run, service
    /// the exit, and hand the baton on when the guest asks it to.
    fn run_core(
        core: usize,
        vcpu: u64,
        exit_ptr: *const HvVcpuExit,
        host_ram: *mut u8,
        cores_declared: usize,
        lock: &std::sync::Mutex<Shared>,
        baton: &std::sync::Condvar,
    ) {
        loop {
            {
                let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if g.sched.done {
                        g.sched.state[core] = CoreState::Finished;
                        return;
                    }
                    if g.sched.current == core {
                        break;
                    }
                    g = baton.wait(g).unwrap_or_else(|e| e.into_inner());
                }
                // 06 §5: "the VMM's I/O threads poll hot doorbells ... and
                // arm wakes when idle." The device model is polled on the
                // core that owns the device — 04 §3: "a `@driver`'s vectors,
                // pools, permits, and recovery lanes live on its core", and
                // plans/M8.md decision 8 pins virtio-blk to core 0.
                if core == 0 {
                    let s = &mut *g;
                    if let Err(e) = service_blk(&mut s.blk, &mut s.chooser, host_ram) {
                        s.error.get_or_insert(e);
                        s.sched.done = true;
                        s.sched.state[core] = CoreState::Finished;
                        drop(g);
                        baton.notify_all();
                        return;
                    }
                }
            }
            let r = unsafe { hv_vcpu_run(vcpu) };
            if r != HV_SUCCESS {
                let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                g.error.get_or_insert(VmmError::Hvf {
                    call: "hv_vcpu_run",
                    code: r,
                });
                g.sched.done = true;
                g.sched.state[core] = CoreState::Finished;
                drop(g);
                baton.notify_all();
                return;
            }
            {
                let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                guard.exits += 1;
                // plans/M8.md item C3: the admission witness's one and only
                // call site (06 §8). It runs *before* this exit is decoded,
                // so every message the guest admitted during the run that
                // just ended is in the choice sequence ahead of whatever
                // choice this exit itself resolves — and it runs on every
                // exit, so no drain can hide between two of them. A
                // single-core image has no request ring and this returns
                // immediately, which is why every pre-C3 recording is
                // byte-identical.
                let g = &mut *guard;
                if let Err(e) = witness_admissions(&mut g.admission, &mut g.chooser, host_ram, core)
                {
                    g.error.get_or_insert(e);
                    g.sched.done = true;
                    g.sched.state[core] = CoreState::Finished;
                    drop(guard);
                    baton.notify_all();
                    return;
                }
            }
            match handle_exit(core, vcpu, exit_ptr, host_ram, cores_declared, lock) {
                Ok(Step::Keep) => {}
                Ok(Step::Yield) => {
                    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    let g = &mut *guard;
                    match next_core(&mut g.sched, core, host_ram) {
                        Some(next) => g.sched.current = next,
                        None => {
                            // Every core is parked or finished and no
                            // pending word can change that: nothing will
                            // ever run again. Fail closed rather than hang
                            // (CLAUDE.md's own rule) — a hung machine that
                            // prints its transcript as success is the one
                            // outcome a boot must never produce.
                            g.error.get_or_insert(VmmError::GuestFault(format!(
                                "core {core} parked and no core is runnable: every core is \
                                 parked with an empty pending word, so no turn can ever run \
                                 again (04-compiler.md §2)"
                            )));
                            g.sched.done = true;
                        }
                    }
                    if g.sched.done {
                        g.sched.state[core] = CoreState::Finished;
                    }
                    let finished = g.sched.done;
                    drop(guard);
                    baton.notify_all();
                    if finished {
                        return;
                    }
                }
                Ok(Step::Halt(code)) => {
                    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    g.exit_code.get_or_insert(code);
                    g.sched.done = true;
                    g.sched.state[core] = CoreState::Finished;
                    drop(g);
                    baton.notify_all();
                    return;
                }
                Err(e) => {
                    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    g.error.get_or_insert(e);
                    g.sched.done = true;
                    g.sched.state[core] = CoreState::Finished;
                    drop(g);
                    baton.notify_all();
                    return;
                }
            }
        }
    }

    // Core `n`'s own entry address: the report's `Entry base=` for core 0,
    // its own `CoreEntry core=n base=` line for the rest.
    let mut core_entry = [0u64; NCORES];
    core_entry[0] = parsed.entry;
    for (core, base) in &parsed.core_entries {
        core_entry[*core] = *base;
    }

    let (handles_tx, handles_rx) = std::sync::mpsc::channel::<usize>();
    std::thread::scope(|scope| {
        let mut threads = Vec::with_capacity(cores_declared);
        for core in 0..cores_declared {
            let ram = SendPtr(host_ram);
            let tx = handles_tx.clone();
            let shared = &shared;
            let baton = &baton;
            let entry = core_entry[core];
            threads.push(scope.spawn(move || {
                let ram = ram;
                let SendPtr(host_ram) = ram;
                // HVF binds a vCPU to its creating thread: create, register,
                // run and destroy all happen right here.
                let mut vcpu: u64 = 0;
                let mut exit_ptr: *mut HvVcpuExit = std::ptr::null_mut();
                let r = unsafe { hv_vcpu_create(&mut vcpu, &mut exit_ptr, std::ptr::null_mut()) };
                if r != HV_SUCCESS {
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                    g.error.get_or_insert(VmmError::Hvf {
                        call: "hv_vcpu_create",
                        code: r,
                    });
                    g.sched.done = true;
                    g.sched.state[core] = CoreState::Finished;
                    drop(g);
                    baton.notify_all();
                    let _ = tx.send(core);
                    return;
                }
                {
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                    g.vcpus[core] = vcpu;
                }
                let _ = tx.send(core);

                // 06 §3: "points `x0` at the machine-info page ... and starts
                // vCPU 0 at the image entry." Every core gets the identical
                // boot register state at its own entry — there is no
                // per-core discovery register and no MPIDR read: a core
                // knows which core it is because the image gave it its own
                // entry block (06 §3: "no discovery").
                let set = |reg: u32, value: u64| -> Result<(), VmmError> {
                    let r = unsafe { hv_vcpu_set_reg(vcpu, reg, value) };
                    if r == HV_SUCCESS {
                        Ok(())
                    } else {
                        Err(VmmError::Hvf {
                            call: "hv_vcpu_set_reg",
                            code: r,
                        })
                    }
                };
                let init = (|| -> Result<(), VmmError> {
                    set(hv_reg_xn(0), machine_layout::MACHINE_INFO_BASE)?;
                    set(HV_REG_PC, entry)?;
                    // EL1h (`SPSel = 1`), every exception masked
                    // (`DAIF = 1111`) — the standard bare-metal AArch64 boot
                    // value, plans/M5.md decision text's own "0x3c5".
                    set(HV_REG_CPSR, 0x3c5)?;
                    let r = unsafe { hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_CPACR_EL1, 0x0030_0000) };
                    if r != HV_SUCCESS {
                        return Err(VmmError::Hvf {
                            call: "hv_vcpu_set_sys_reg(CPACR_EL1)",
                            code: r,
                        });
                    }
                    Ok(())
                })();
                match init {
                    Ok(()) => run_core(
                        core,
                        vcpu,
                        exit_ptr,
                        host_ram,
                        cores_declared,
                        shared,
                        baton,
                    ),
                    Err(e) => {
                        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                        g.error.get_or_insert(e);
                        g.sched.done = true;
                        g.sched.state[core] = CoreState::Finished;
                        drop(g);
                        baton.notify_all();
                    }
                }
                // Clear this core's handle *under the lock* before
                // destroying it, so the watchdog's own `hv_vcpus_exit` can
                // never name a destroyed vCPU.
                {
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                    g.vcpus[core] = 0;
                }
                unsafe {
                    hv_vcpu_destroy(vcpu);
                }
            }));
        }
        // Every core has registered its handle before the watchdog can name
        // any of them.
        for _ in 0..cores_declared {
            let _ = handles_rx.recv();
        }

        // --- watchdog thread (decision 15's own host-side wall cap) --------
        // With three cores, a hang on *any* of them must still terminate the
        // boot: force-exit every live vCPU, and let whichever core was
        // actually inside `hv_vcpu_run` report the timeout under its own
        // number.
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let watchdog_shared = &shared;
        let watchdog = scope.spawn(move || {
            if done_rx.recv_timeout(WALL_CAP).is_err() {
                let mut g = watchdog_shared.lock().unwrap_or_else(|e| e.into_inner());
                let mut live: Vec<u64> = g.vcpus.iter().copied().filter(|v| *v != 0).collect();
                if !live.is_empty() {
                    unsafe {
                        hv_vcpus_exit(live.as_mut_ptr(), live.len() as u32);
                    }
                }
                g.sched.done = true;
            }
        });
        for t in threads {
            let _ = t.join();
        }
        let _ = done_tx.send(());
        let _ = watchdog.join();
    });
    // Nothing else can hold the lock now (every core thread and the watchdog
    // were joined inside the scope above).
    let shared = shared.into_inner().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = shared.error {
        return Err(e);
    }
    let exit_code = shared.exit_code.ok_or_else(|| {
        VmmError::GuestFault(
            "no core reported the guest exit protocol (`EXIT_MMIO_ADDR`) — the boot ended without \
             the image ever halting"
                .to_string(),
        )
    })?;
    // plans/M10.md item B1 / decision 591: the abort re-entrancy latch must
    // be clear on every green boot. A nonzero value after exit 0 means an
    // abort path ran (or left the latch set) on a boot that claimed success.
    if exit_code == 0 {
        let latch_off = (machine_layout::MACHINE_INFO_BASE - machine_layout::DRAM_BASE
            + machine_info::OFF_ABORT_LATCH) as usize;
        let latch = unsafe { std::ptr::read_unaligned((host_ram.add(latch_off)) as *const u64) };
        if latch != 0 {
            return Err(VmmError::GuestFault(format!(
                "abort re-entrancy latch at machine_info::OFF_ABORT_LATCH is {latch:#x} after a \
                 green boot (exit_code=0); decision 591 requires it never set on any green boot"
            )));
        }
    }
    // plans/M8.md item C1: every core this image released must have
    // executed its own entry block. The mark is guest-written (each core's
    // entry stores `machine_info::core_mark_running(n)` into its own slot),
    // so a core that never ran leaves a zero the zeroed reservation put
    // there — a released-but-dead core is a machine that silently ran an
    // image on fewer cores than it claims, which is the exact failure this
    // item exists to make impossible.
    //
    // Keyed on the release the guest actually rang, not on the report's
    // declared count. A `wrela build` image is the case that forced this:
    // `layout_program`'s entry stub halts with `EXIT_CODE_NO_RUNTIME` and
    // never calls `build_entry_driver`, so its release block — the same one
    // that writes core 0's own mark — is never emitted at all. Such an
    // image still carries `CoreEntry` lines in its report, because it still
    // *contains* the secondary entry blocks; checking the declared count
    // there reported "core 0 was released but never ran its own entry
    // block" about a core that was never released, and turned a clean
    // "no runtime yet" exit (1) into a bad-image fault (2). The marks are
    // evidence of the release, so the release is what decides whether to
    // demand them.
    if shared.released {
        check_core_marks(host_ram, cores_declared)?;
    }

    // decision 12: the transcript is read from the ring pages only after the
    // guest halts.
    let transcript = drain_console(host_ram);
    let core_marks = (0..NCORES)
        .map(|c| read_core_mark(host_ram, c))
        .collect::<Vec<u64>>();
    let (choices, divergences) = record::finish_chooser(shared.chooser);
    Ok((
        BootOutcome {
            transcript,
            exit_code,
            choices,
            exits: shared.exits,
            core_marks,
        },
        divergences,
    ))
}

/// Core `core`'s own guest-written bring-up mark (plans/M8.md item C1,
/// `machine_info::OFF_CORE_MARK`).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn read_core_mark(host_ram: *const u8, core: usize) -> u64 {
    use wrela_machine::layout as machine_layout;
    let off =
        (wrela_machine::machine_info::core_mark_addr(core) - machine_layout::DRAM_BASE) as usize;
    let mut b = [0u8; 8];
    unsafe { std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 8) };
    u64::from_le_bytes(b)
}

/// plans/M8.md item C1's own acceptance check, run after every multicore
/// boot: each of the `cores` cores this image brought up wrote its own mark,
/// and wrote *its own* (never another core's — that would be a mis-wired
/// `CoreEntry` address, a real and otherwise silent bug).
///
/// A single-core image (`cores == 1`) writes no mark at all and is not
/// checked: it releases nothing, so there is nothing to have gone missing,
/// and that is also what keeps every M5-M7 boot byte-identical.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn check_core_marks(host_ram: *const u8, cores: usize) -> Result<(), VmmError> {
    use wrela_machine::machine_info;
    if cores <= 1 {
        return Ok(());
    }
    for core in 0..cores {
        let want = machine_info::core_mark_running(core);
        let got = read_core_mark(host_ram, core);
        if got != want {
            return Err(VmmError::GuestFault(format!(
                "core {core} was released but never ran its own entry block: its bring-up mark is \
                 {got:#x}, expected {want:#x} (06-machine.md §3: the entry releases the other \
                 vCPUs and every core enters its own event loop)"
            )));
        }
    }
    Ok(())
}

/// plans/M7.md item F: one declared `blk` device model plus the checked
/// view of guest memory it is allowed to touch. A pair rather than one
/// type because `devices::GuestMem` is deliberately the *only* thing in
/// this VMM holding both a raw DRAM pointer and the declared pool windows
/// (`devices.rs`'s own module doc: decision 5's security boundary is
/// enforced by there being no other way to turn a guest address into a
/// host offset).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct BlkState {
    device: devices::BlkDevice,
    mem: devices::GuestMem,
    /// Guest GPA of `interrupt_status` (devregs + 0x60), when the image
    /// declared a vector and bound an ISR. `None` for poll builds.
    irq_status_gpa: Option<u64>,
}

/// 06 §4's raise, both producers' one implementation: set bit `vector` in
/// core 0's own pending word. **An OR, never a store**, since M7 gives
/// this machine a second raiser (a `blk` completion) alongside M6's
/// deadline service — a plain store of `1` would silently drop a
/// completion vector raised moments earlier in the same park.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn raise_vector(host_ram: *mut u8, vector: u64) {
    use wrela_machine::layout as machine_layout;
    let off = (wrela_machine::pending::core_word_addr(0) - machine_layout::DRAM_BASE) as usize;
    unsafe {
        let mut b = [0u8; 8];
        std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 8);
        let raised = u64::from_le_bytes(b) | (1u64 << (vector & 63));
        std::ptr::copy_nonoverlapping(raised.to_le_bytes().as_ptr(), host_ram.add(off), 8);
    }
}

/// 06 §5's doorbell poll, plans/M7.md decision 7's recording, and the
/// completion's own vector raise — the whole guest-visible half of the
/// `blk` device model, in one place, called from exactly two sites in
/// `boot_image_core`'s loop (every vCPU exit, and the park path before the
/// sleep decision). Returns whether anything completed.
///
/// The record/replay split (decision 7, 06 §8) is the subtle part, so it
/// is spelled out rather than implied:
///
/// - The **model always runs**, in both modes. A completion is not a value
///   this VMM invents — it is a deterministic function of the ring the
///   guest published and the disk this VMM owns, and the *payload bytes*
///   have to be written into guest memory for a replayed guest to see
///   anything at all. 06 §8's "replay feeds the log from virtual device
///   models" is exactly this: the models are still there.
/// - The **used-ring `len` and the status the driver branches on come from
///   the log** under replay, never from the fresh model run.
/// - The **`head` always comes from the model**, never the log: a chain's
///   head is the identity the *guest itself* published in `avail.ring`, so
///   feeding it from an untrusted log would be precisely the unchecked
///   index 03 §4 forbids. A log whose head disagrees is a divergence, not
///   an index.
/// - A disagreement in any field is `Divergence::DeviceCompletionMismatch`
///   — named, never silently taken.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn service_blk(
    blk: &mut Option<BlkState>,
    chooser: &mut record::Chooser,
    host_ram: *mut u8,
) -> Result<bool, VmmError> {
    let Some(state) = blk.as_mut() else {
        return Ok(false);
    };
    let completions = state
        .device
        .service(&mut state.mem)
        .map_err(|fault| VmmError::GuestFault(format!("virtio-blk: {fault}")))?;
    commit_completions(state, chooser, &completions, host_ram)
}

/// The recorder-and-used-ring half of `service_blk`, shared verbatim with
/// the quiesce path (plans/M8.md item F). A completion produced while
/// establishing quiescence is an ordinary completion in every respect —
/// same choice entry, same divergence check, same used-ring publication,
/// same vector raise — and factoring it out is what makes that true by
/// construction rather than by two copies agreeing.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn commit_completions(
    state: &mut BlkState,
    chooser: &mut record::Chooser,
    completions: &[devices::Completion],
    host_ram: *mut u8,
) -> Result<bool, VmmError> {
    use wrela_machine::layout as machine_layout;
    if completions.is_empty() {
        return Ok(false);
    }
    for c in completions {
        let request = record::ChoiceRequest::DeviceCompletion {
            device: "blk".to_string(),
            queue: 0,
            head: c.head as u32,
            status: c.status as u32,
            len: c.len,
            digest: c.digest.clone(),
        };
        let observed = request.fallback();
        let index = chooser.resolved_count();
        let chosen = {
            let observed = observed.clone();
            chooser.choose_next(request, move || observed)
        };
        if chosen != observed {
            chooser.note_divergence(record::Divergence::DeviceCompletionMismatch {
                index,
                recorded: chosen.to_text_fields(),
                actual: observed.to_text_fields(),
            });
        }
        let len = match &chosen {
            record::ChoiceEntry::DeviceCompletion { len, .. } => *len,
            // A replayed tag mismatch already fell back to `observed`
            // (`ChoiceRequest::fallback`), so this arm is unreachable;
            // it fails closed rather than inventing a length.
            _ => c.len,
        };
        state
            .device
            .commit_used(&mut state.mem, c.head, len)
            .map_err(|fault| VmmError::GuestFault(format!("virtio-blk: {fault}")))?;
    }
    // 06 §4: a completion optionally raises this driver's own vector. A
    // device declared with none is 03 §7's poll build — the used ring
    // alone is the signal, and nothing is raised or recorded.
    //
    // plans/M7.md item E4: also OR `INT_VRING` (bit 0) into the guest's
    // `interrupt_status` so the ISR's mask-against-declared-bits path
    // sees a real level after a used-ring publish (boot-time
    // `IrqHostInject` only covers the pre-first-instruction oracle).
    if let Some(vector) = state.device.config.vector {
        if let Some(gpa) = state.irq_status_gpa {
            if gpa >= machine_layout::DRAM_BASE {
                let off = (gpa - machine_layout::DRAM_BASE) as usize;
                unsafe {
                    let mut b = [0u8; 4];
                    std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 4);
                    let status = u32::from_le_bytes(b) | 1;
                    std::ptr::copy_nonoverlapping(
                        status.to_le_bytes().as_ptr(),
                        host_ram.add(off),
                        4,
                    );
                }
            }
        }
        chooser.choose_next(record::ChoiceRequest::VectorRaise { vector }, || {
            record::ChoiceEntry::VectorRaise { vector }
        });
        raise_vector(host_ram, vector);
    }
    Ok(true)
}

/// plans/M8.md item C3, decision 42 — the recorder's witness on 06 §8's
/// "per-mailbox cross-core admission order", which is the one scheduling
/// nondeterminism 04 §2 gives this machine.
///
/// **Why a witness can be exact here, stated as the invariant it rests
/// on.** A cross-core request ring's producer is core `src` and its
/// consumer is core `dst`, and `src != dst` by construction
/// (`parse_report` refuses otherwise). Decision 11's baton means exactly
/// one vCPU is inside `hv_vcpu_run` at any instant. So between two
/// consecutive vCPU exits **at most one core ran**, and for any one ring
/// that core is either its producer or its consumer, never both:
/// a ring whose `dst` just ran can only have *shrunk*, and by exactly the
/// number of messages that core's drain admitted. The occupancy word is
/// therefore an exact counter, not a sampled one — no modular head
/// arithmetic, no lost wrap.
///
/// **The order is exact too, for the same reason.**
/// `layout::build_rt_drain` walks its request lanes in `RuntimeTables::
/// rings` order and drains each lane to empty before starting the next,
/// and no other core can produce into any of them meanwhile. Walking the
/// report's `Ring` lines in that same order reconstructs the order the
/// guest actually admitted in.
#[derive(Debug, Default)]
struct AdmissionWitness {
    rings: Vec<RequestRing>,
    /// Each ring's occupancy word as of the last observation, parallel to
    /// `rings`. Zero-initialized, which is the value guest DRAM's own
    /// zeroed reservation puts there before the first instruction runs.
    last_count: Vec<u64>,
}

impl AdmissionWitness {
    fn new(rings: Vec<RequestRing>) -> AdmissionWitness {
        let last_count = vec![0; rings.len()];
        AdmissionWitness { rings, last_count }
    }

    /// The whole counting rule, as a pure function of (this observation's
    /// occupancy words, which core just ran) — separated from the guest
    /// memory read above it so it can be unit-tested directly
    /// (`admission_witness_*`, below).
    ///
    /// Returns one `(mailbox, sender)` pair per message admitted, in the
    /// order they were admitted. Fails closed rather than guessing if a
    /// ring the running core *consumes* somehow grew: that would mean the
    /// SPSC producer/consumer split this reconstruction rests on is not
    /// true of the running image, and a silently under-recorded admission
    /// order is the exact thing 06 §8 exists to prevent.
    fn observe(&mut self, counts: &[u64], core: usize) -> Result<Vec<(String, String)>, String> {
        debug_assert_eq!(counts.len(), self.rings.len());
        let mut admitted = Vec::new();
        for (i, ring) in self.rings.iter().enumerate() {
            let now = counts[i];
            let was = self.last_count[i];
            if ring.dst == core {
                if now > was {
                    return Err(format!(
                        "cross-core ring src={} dst={} target={} grew from {was} to {now} while \
                         its own consuming core {core} was the only core running — the SPSC \
                         producer/consumer split the admission recorder rests on does not hold",
                        ring.src, ring.dst, ring.target
                    ));
                }
                for _ in 0..(was - now) {
                    admitted.push((ring.target.clone(), format!("core{}", ring.src)));
                }
            }
            self.last_count[i] = now;
        }
        Ok(admitted)
    }
}

/// Reads every request ring's occupancy word out of guest memory and
/// pushes one `ChoiceEntry::Admission` per message core `core`'s drain
/// just admitted (`AdmissionWitness` above has the whole argument).
///
/// **Witness, not injection** — the honest claim, in the words plans/M8.md
/// item C3 asks for. The drain is guest code and the mailbox is guest
/// memory; nothing here writes either, in record mode or replay mode. So
/// replay does not *feed* the recorded admission order back to the guest;
/// it re-witnesses and **checks**, and a disagreement is
/// `Divergence::AdmissionMismatch`, named exactly like a device
/// completion's. Under decision 11's baton there is no alternative order
/// to feed, which is why the checking form is the honest one — and why a
/// later schedule enumerator, which would vary the baton hand-off, is the
/// thing that would make injection mean something.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn witness_admissions(
    witness: &mut AdmissionWitness,
    chooser: &mut record::Chooser,
    host_ram: *const u8,
    core: usize,
) -> Result<(), VmmError> {
    use wrela_machine::layout as machine_layout;
    if witness.rings.is_empty() {
        return Ok(());
    }
    let counts: Vec<u64> = witness
        .rings
        .iter()
        .map(|r| {
            let off = (r.count_addr - machine_layout::DRAM_BASE) as usize;
            let mut b = [0u8; 8];
            unsafe { std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 8) };
            u64::from_le_bytes(b)
        })
        .collect();
    let admitted = witness
        .observe(&counts, core)
        .map_err(VmmError::GuestFault)?;
    for (mailbox, sender) in admitted {
        let request = record::ChoiceRequest::Admission { mailbox, sender };
        let observed = request.fallback();
        let index = chooser.resolved_count();
        let chosen = {
            let observed = observed.clone();
            chooser.choose_next(request, move || observed)
        };
        if chosen != observed {
            chooser.note_divergence(record::Divergence::AdmissionMismatch {
                index,
                recorded: chosen.to_text_fields(),
                actual: observed.to_text_fields(),
            });
        }
    }
    Ok(())
}

/// Reads the vCPU's own current `PC` for a fault diagnostic — best-effort
/// (`Ok(0)` is never returned; a real HVF failure here still surfaces as
/// `None` to the caller, which substitutes `0` rather than compounding
/// one failure with another).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn read_pc(vcpu: u64) -> Option<u64> {
    use hv::{HV_REG_PC, HV_SUCCESS, hv_vcpu_get_reg};
    let mut pc = 0u64;
    let r = unsafe { hv_vcpu_get_reg(vcpu, HV_REG_PC, &mut pc) };
    if r == HV_SUCCESS { Some(pc) } else { None }
}

/// plans/M6.md item F: the fault diagnostic's own second half, and the
/// single highest-value debugging tool this milestone added.
///
/// 06-machine.md §4 gives this machine **no** exception vector table:
/// there is no emulated GIC, the guest never installs a `VBAR_EL1`, and
/// every interrupt is a checkpoint-observed pending word instead. So an
/// EL1 synchronous exception the guest takes *itself* — an unaligned
/// 64-bit access (the MMU is off, so every access is Device-nGnRnE and
/// alignment-checked), a misaligned `sp`, an undefined instruction — is
/// not routed to the host at all: the CPU vectors to `VBAR_EL1 + <slot>`,
/// which is guest-physical `0x000..0x780` with `VBAR_EL1` still zero,
/// which is not mapped, which *then* exits to this VMM as an instruction
/// abort at that vector address. The reported `esr`/`ipa`/`pc` therefore
/// describe the **second** fault, and say nothing at all about the first.
///
/// The original fault's own state is still sitting in `ESR_EL1`/
/// `ELR_EL1`/`FAR_EL1`, untouched (nothing at the vector address ran to
/// clobber it) — so whenever `pc` lands on a `VBAR_EL1` vector slot,
/// report those too, and name the mechanism. Item F's own
/// `golden/boot-group-join` cost a full debugging session to a bare
/// `pc=0x200`; with this note the same failure reads out its real cause
/// (`ESR_EL1` EC `0x25`, DFSC `0b100001` — an alignment fault) and its
/// real faulting instruction directly.
///
/// Best-effort by construction: a register read that fails is simply
/// omitted, never turned into a second error on top of the first.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn el1_exception_note(vcpu: u64, pc: u64) -> String {
    use hv::*;
    let sys = |reg: u16| -> Option<u64> {
        let mut v = 0u64;
        let r = unsafe { hv_vcpu_get_sys_reg(vcpu, reg, &mut v) };
        if r == HV_SUCCESS { Some(v) } else { None }
    };
    let Some(vbar) = sys(HV_SYS_REG_VBAR_EL1) else {
        return String::new();
    };
    // The AArch64 vector table is 16 slots of 0x80 bytes (ARM ARM
    // D1.10.2): four groups of four (current EL with SP0, current EL with
    // SPx, lower EL AArch64, lower EL AArch32) x (sync, IRQ, FIQ, SError).
    if pc < vbar || pc >= vbar + 0x800 || (pc - vbar) % 0x80 != 0 {
        return String::new();
    }
    let slot = pc - vbar;
    let (esr1, elr1, far1) = (
        sys(HV_SYS_REG_ESR_EL1),
        sys(HV_SYS_REG_ELR_EL1),
        sys(HV_SYS_REG_FAR_EL1),
    );
    let mut note = format!(
        "; pc is VBAR_EL1({vbar:#x}) + {slot:#x} — the guest took an EL1 exception into a \
         vector table this machine never installs (06-machine.md §4), so the fault above is \
         only the resulting instruction abort. The original fault:"
    );
    match esr1 {
        Some(e) => {
            note.push_str(&format!(" ESR_EL1={e:#x} (EC={:#x})", (e >> 26) & 0x3F));
        }
        None => note.push_str(" ESR_EL1=<unreadable>"),
    }
    if let Some(v) = elr1 {
        note.push_str(&format!(" ELR_EL1={v:#x}"));
    }
    if let Some(v) = far1 {
        note.push_str(&format!(" FAR_EL1={v:#x}"));
    }
    note
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn advance_pc(vcpu: u64) -> Result<(), VmmError> {
    use hv::*;
    let mut pc = 0u64;
    let r = unsafe { hv_vcpu_get_reg(vcpu, HV_REG_PC, &mut pc) };
    if r != HV_SUCCESS {
        return Err(VmmError::Hvf {
            call: "hv_vcpu_get_reg(PC)",
            code: r,
        });
    }
    let r = unsafe { hv_vcpu_set_reg(vcpu, HV_REG_PC, pc + 4) };
    if r != HV_SUCCESS {
        return Err(VmmError::Hvf {
            call: "hv_vcpu_set_reg(PC)",
            code: r,
        });
    }
    Ok(())
}

/// Monotonic nanoseconds since an arbitrary, process-local epoch — decision
/// 13's own "the VMM ... returns monotonic ns"; `std::time::Instant`
/// already is exactly this on every platform Rust supports, so no host
/// syscall FFI is needed for it (unlike the guest-facing MMIO trap itself,
/// which is HVF's own exit mechanism).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn monotonic_ns() -> u64 {
    use std::sync::OnceLock;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    // plans/M6.md item F: never `0`. `0` is the deadline protocol's own
    // "no deadline" sentinel everywhere it appears — `machine_info::
    // OFF_NEXT_DEADLINE` (item E's park contract with this VMM) and the
    // group arena's own `deadline_ns` word — so a guest that computed
    // `now() + ms(0)` at the very first instant of the epoch would
    // otherwise arm a deadline indistinguishable from "none". Clamping the
    // clock's own floor to 1ns is the smallest coherent fix: the machine's
    // monotonic clock is defined to start at 1, the sentinel keeps its one
    // meaning, and no arithmetic anywhere needs a second sentinel value.
    (epoch.elapsed().as_nanos() as u64).max(1)
}

/// Reads the console ring's own `avail.idx` and walks descriptors
/// `0..avail.idx` **directly by index** (decision 12's own disclosed
/// simplification, `layout.rs`'s module doc: this producer never
/// populates `avail.ring[]` at all, since it never reorders or skips an
/// index) — the used ring is never read either (nothing here tracks
/// completions). Every offset is clamped to the DRAM reservation so a
/// malformed/adversarial descriptor can only truncate the transcript,
/// never read out of bounds.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn drain_console(host_ram: *const u8) -> Vec<u8> {
    use wrela_machine::console;
    use wrela_machine::layout as machine_layout;

    let dram_size = machine_layout::DRAM_SIZE;
    let ring_off = (console::RING_BASE - machine_layout::DRAM_BASE) as usize;

    let read_u16 = |off: usize| -> u16 {
        let mut b = [0u8; 2];
        unsafe { std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 2) };
        u16::from_le_bytes(b)
    };
    let read_u32 = |off: usize| -> u32 {
        let mut b = [0u8; 4];
        unsafe { std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 4) };
        u32::from_le_bytes(b)
    };
    let read_u64 = |off: usize| -> u64 {
        let mut b = [0u8; 8];
        unsafe { std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 8) };
        u64::from_le_bytes(b)
    };

    let avail_idx = read_u16(ring_off + console::AVAIL_OFFSET as usize + 2);
    let mut out = Vec::new();
    let count = (avail_idx as u64).min(console::QUEUE_SIZE);
    for i in 0..count {
        let desc_off =
            ring_off + (console::DESC_TABLE_OFFSET + i * console::DESC_ENTRY_SIZE) as usize;
        let addr = read_u64(desc_off);
        let len = read_u32(desc_off + 8) as u64;
        if addr < machine_layout::DRAM_BASE {
            continue;
        }
        let src_off = addr - machine_layout::DRAM_BASE;
        if src_off >= dram_size {
            continue;
        }
        let clamped_len = len.min(dram_size - src_off) as usize;
        let mut buf = vec![0u8; clamped_len];
        unsafe {
            std::ptr::copy_nonoverlapping(
                host_ram.add(src_off as usize),
                buf.as_mut_ptr(),
                clamped_len,
            );
        }
        out.extend_from_slice(&buf);
    }
    out
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
pub fn boot_image(_report_path: &Path, _img_path: &Path) -> Result<BootOutcome, VmmError> {
    Err(VmmError::Unsupported(
        "the wrela VMM needs Hypervisor.framework (macOS/aarch64 at M5); no other host is implemented yet",
    ))
}

/// Non-HVF-host stub for `boot_image_core` (`record::replay`'s own
/// dependency) — fails closed exactly like `boot_image` above, so
/// `record::replay` is callable (and fails the same honest way) on every
/// host, not only the one this milestone actually boots on.
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn boot_image_core(
    _report_path: &Path,
    _img_path: &Path,
    _replay_choices: Option<Vec<record::ChoiceEntry>>,
    _test_delayed_raise: Option<(Duration, u64)>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    Err(VmmError::Unsupported(
        "the wrela VMM needs Hypervisor.framework (macOS/aarch64 at M5); no other host is implemented yet",
    ))
}

#[cfg(target_os = "linux")]
pub mod kvm {
    //! Linux/KVM backend. May build on the rust-vmm crates. Unimplemented
    //! until the Raspberry Pi flagship host milestone.
}

pub mod devices;

pub mod record;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_report_accepts_a_well_formed_report() {
        let text = format!(
            "Machine revision={}\nInput path=input.wr digest=abc123\nSection name=entry base=0x40500000 size=64\nEntry base=0x40500000\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        let parsed = parse_report(&text).unwrap();
        assert_eq!(parsed.entry, 0x40500000);
    }

    #[test]
    fn parse_report_rejects_a_wrong_revision() {
        let text = "Machine revision=some-other-machine-v9\nInput path=x digest=y\nSection name=entry base=0x0 size=1\nEntry base=0x0\n";
        match parse_report(text) {
            Err(VmmError::MachineRevisionMismatch { report, .. }) => {
                assert_eq!(report, "some-other-machine-v9");
            }
            other => panic!("expected a machine-revision mismatch, got {other:?}"),
        }
    }

    #[test]
    fn parse_report_rejects_a_missing_revision_line() {
        let text = "Input path=x digest=y\nSection name=entry base=0x0 size=1\nEntry base=0x0\n";
        assert!(matches!(
            parse_report(text),
            Err(VmmError::MalformedReport(_))
        ));
    }

    #[test]
    fn parse_report_rejects_a_missing_input_line() {
        let text = format!(
            "Machine revision={}\nSection name=entry base=0x0 size=1\nEntry base=0x0\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        assert!(matches!(
            parse_report(&text),
            Err(VmmError::MalformedReport(_))
        ));
    }

    #[test]
    fn parse_report_rejects_a_missing_section_line() {
        let text = format!(
            "Machine revision={}\nInput path=x digest=y\nEntry base=0x0\n",
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
            "Machine revision={}\nInput path=x digest=y\nSection name=entry base=0x0 size=1\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        assert!(matches!(
            parse_report(&text),
            Err(VmmError::MalformedReport(_))
        ));
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
            "Machine revision={}\nInput path=clock-test.wr digest=testdigest\nSection name=entry base={:#x} size={}\nEntry base={:#x}\n",
            wrela_machine::MACHINE_REVISION_STR,
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

        // --- truncated choice log: an underrun, caught ---------------------
        let mut short_log = recorded.clone();
        short_log.choices.truncate(1);
        let divergences = record::replay(&report_path, &img_path, &short_log).expect("replay boot");
        assert!(divergences.iter().any(|d| matches!(
            d,
            record::Divergence::ChoiceLogUnderrun {
                index: 1,
                recorded: 1
            }
        )));

        // --- padded choice log: an overrun, caught -------------------------
        let mut long_log = recorded.clone();
        long_log
            .choices
            .push(record::ChoiceEntry::ClockRead { value: 424242 });
        let divergences = record::replay(&report_path, &img_path, &long_log).expect("replay boot");
        assert!(divergences.iter().any(|d| matches!(
            d,
            record::Divergence::ChoiceLogOverrun {
                consumed: 2,
                recorded: 3
            }
        )));

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
            "Machine revision={}\nInput path={tag}.wr digest=testdigest\nSection name=entry base={:#x} size={}\nEntry base={:#x}\n",
            wrela_machine::MACHINE_REVISION_STR,
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
            "Machine revision={}\nInput path={tag}.wr digest=testdigest\nSection name=entry base={:#x} size={}\nEntry base={:#x}\n",
            wrela_machine::MACHINE_REVISION_STR,
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
        use wrela_compiler::{codegen, layout, loader, lower};

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

        let reachable =
            lower::guest_reachable_keys_closure(&programs, &lower::LowerOpts::default());
        let lower_opts = lower::LowerOpts {
            emit_comptime_tests: false,
            only: Some(reachable),
        };
        let mut mwir_programs = Vec::new();
        let mut flow_fns = BTreeMap::new();
        for typed in programs.values() {
            mwir_programs.push(lower::lower_program_with(typed, &lower_opts).expect("sync lower"));
            flow_fns.extend(
                wrela_compiler::flowwir_lower::lower_program_with(typed, &lower_opts)
                    .expect("flowwir lower")
                    .fns,
            );
        }
        let mwir_program = layout::merge_mwir_programs(mwir_programs);
        let flow_program = wrela_compiler::flowwir::FlowWirProgram { fns: flow_fns };
        let mut layout_ctx = layout::merge_layout_ctx(&modules).expect("layout ctx");
        layout::enrich_layout_ctx_with_instantiations(&mut layout_ctx, &programs);
        let graph = match &program.image_fn {
            Some(fn_name) => {
                wrela_compiler::eval::interp::eval_image(program, fn_name).expect("image graph")
            }
            None => Default::default(),
        };
        let method_index =
            layout::actor_method_index_tables(&modules, &layout_ctx).expect("method index");
        let group_arena_capacity = layout::count_with_group_sites(&modules);
        let enqueue_specs =
            layout::mailbox_enqueue_specs(&graph, &modules, &layout_ctx).expect("enqueue specs");
        let codegen_program = codegen::codegen_program_with_async(
            &mwir_program,
            &flow_program,
            &layout_ctx,
            &method_index,
            group_arena_capacity,
            &enqueue_specs,
        )
        .expect("codegen");
        let async_frames =
            codegen::async_frame_sizes(&flow_program, &layout_ctx).expect("async frames");
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
        let group_child_index =
            codegen::compute_group_child_indices(&flow_program).expect("group child index");
        let boot = layout::BootCtx {
            graph: &graph,
            modules: &modules,
            programs: &programs,
            layout_ctx: &layout_ctx,
            async_frames: &async_frames,
            group_child_index: &group_child_index,
        };
        let image = layout::layout_test_image(
            &codegen_program,
            &runtime_tests,
            &async_tests,
            Some(boot),
            &test_args,
        )
        .expect("layout_test_image");

        let mut report = format!(
            "Machine revision={}\nInput path=<conformance> digest=deadbeef\n",
            wrela_machine::MACHINE_REVISION_STR
        );
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
        use std::collections::BTreeMap;
        use wrela_compiler::{codegen, layout};

        let tokens = wrela_compiler::syntax::lexer::lex(src).expect("conformance source must lex");
        let module = wrela_compiler::syntax::parser::parse(tokens).expect("must parse");
        let program =
            wrela_compiler::sema::check_typed(&module, "<conformance>").expect("must check");
        let mut modules = BTreeMap::new();
        modules.insert(module.path.join("."), module.clone());
        let mut programs = BTreeMap::new();
        programs.insert(module.path.join("."), program.clone());
        let layout_ctx = layout::merge_layout_ctx(&modules).expect("layout ctx");
        let mwir_program = wrela_compiler::lower::lower_program(&program).expect("sync lower");
        let flow_program =
            wrela_compiler::flowwir_lower::lower_program(&program).expect("flowwir lower");
        let graph = match &program.image_fn {
            Some(fn_name) => {
                wrela_compiler::eval::interp::eval_image(&program, fn_name).expect("image graph")
            }
            None => Default::default(),
        };
        let method_index =
            layout::actor_method_index_tables(&modules, &layout_ctx).expect("method index");
        let group_arena_capacity = layout::count_with_group_sites(&modules);
        let enqueue_specs =
            layout::mailbox_enqueue_specs(&graph, &modules, &layout_ctx).expect("enqueue specs");
        let codegen_program = codegen::codegen_program_with_async(
            &mwir_program,
            &flow_program,
            &layout_ctx,
            &method_index,
            group_arena_capacity,
            &enqueue_specs,
        )
        .expect("codegen");
        let async_frames =
            codegen::async_frame_sizes(&flow_program, &layout_ctx).expect("async frames");
        let group_child_index =
            codegen::compute_group_child_indices(&flow_program).expect("group child index");
        let boot = layout::BootCtx {
            graph: &graph,
            modules: &modules,
            programs: &programs,
            layout_ctx: &layout_ctx,
            async_frames: &async_frames,
            group_child_index: &group_child_index,
        };
        let image = layout::layout_program(&codegen_program, Some(boot)).expect("layout_program");

        let mut report = format!(
            "Machine revision={}\nInput path=<conformance> digest=deadbeef\n",
            wrela_machine::MACHINE_REVISION_STR
        );
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

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn boot_blob(blob: &[u8], report: &str, tag: &str) -> BootOutcome {
        let dir = std::env::temp_dir().join(format!("wrela-vmm-conf-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let img_path = dir.join("test.img");
        let report_path = dir.join("test.report.txt");
        std::fs::write(&img_path, blob).expect("write img");
        std::fs::write(&report_path, report).expect("write report");
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
        match v:
            case .Ok(n):
                return n + 1
            case .Err(_):
                return 0

@test(runtime)
async fn chain(outer: Actor[Outer]):
    v = await outer.relay()
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
    img.supervise(children=[inner, outer], strategy=Restart.OneForOne,
                  intensity=RestartIntensity(max=3, within=seconds(10)))
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
    match r1:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, "send 1 rejected"
    r2 = send worker.job(v=2)
    match r2:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, "send 2 rejected"
    wl = await worker.log_value()
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
    img.supervise(children=[stamps, worker], strategy=Restart.OneForOne,
                  intensity=RestartIntensity(max=3, within=seconds(10)))
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
        match a:
            case .Ok(_):
                pass
            case .Err(_):
                return 0
        b = await self.log.mark(v=20)
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
        match r:
            case .Ok(_):
                pass
            case .Err(_):
                pass

@test(runtime)
async fn interleave(chain: Actor[ChainActor], third: Actor[Third], log: Actor[Log]):
    s = send third.poke()
    match s:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, "send rejected"
    r = await chain.chain()
    match r:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, "chain rejected"
    v = await log.value()
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
    img.supervise(children=[log, chain, third], strategy=Restart.OneForOne,
                  intensity=RestartIntensity(max=3, within=seconds(10)))
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
    match v:
        case .Ok(_):
            pass
        case .Err(_):
            assert false, "rejected"

@image
pub fn build() -> Image:
    img = Image(name="deadlock", target=Target.wrela_machine_v1)
    s = img.actor(Stuck, mailbox=4)
    img.supervise(children=[s], strategy=Restart.OneForOne,
                  intensity=RestartIntensity(max=3, within=seconds(10)))
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
        let (outcome, divergences) = boot_image_core(
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
            "Machine revision={}\nInput path=el1-vector.wr digest=testdigest\nSection name=entry base={:#x} size={}\nEntry base={:#x}\n",
            wrela_machine::MACHINE_REVISION_STR,
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

        // --- tampered choice tag: caught -----------------------------------
        let mut bad_tag = recorded.clone();
        bad_tag.choices[1] = record::ChoiceEntry::ClockRead { value: 0 };
        let divergences = record::replay(&report_path, &img_path, &bad_tag).expect("replay boot");
        assert!(
            divergences
                .iter()
                .any(|d| matches!(d, record::Divergence::ChoiceTagMismatch { index: 1, .. }))
        );

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

        const EXIT_VMM_FAILURE: i32 = 2;
        const EXIT_REPLAY_DIVERGENCE: i32 = 3;
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
            "Machine revision={}\nInput path=exit-code-contract.wr digest=testdigest\nSection name=entry base={:#x} size={}\nEntry base={:#x}\n",
            wrela_machine::MACHINE_REVISION_STR,
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
        let data_base = {
            let after_code = machine_layout::IMAGE_BASE + (entry_len as u64) * 4;
            after_code.div_ceil(16) * 16
        };
        let words = build_entry(sp_top, data_base, expect_first, expect_last);
        assert_eq!(
            words.len(),
            entry_len,
            "the entry sequence's own length must not depend on the real addresses"
        );

        let mut img: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        img.resize(
            (data_base - machine_layout::IMAGE_BASE + DATA_REGION_SIZE) as usize,
            0,
        );
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
            "Machine revision={}\n\
             Input path=blk-conformance.wr digest=testdigest\n\
             Section name=entry base={:#x} size={}\n\
             Entry base={:#x}\n\
             BlkDevice device=device#0 capacity_sectors=16 features={:#x} vector={BLK_VECTOR}\n\
             BlkQueue index=0 size={QUEUE_SIZE} desc={:#x} avail={:#x} used={:#x} doorbell={:#x}\n\
             BlkPool name=BlockControl device=device#0 base={:#x} size={:#x}\n",
            wrela_machine::MACHINE_REVISION_STR,
            machine_layout::IMAGE_BASE,
            img.len(),
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
            let divergences = record::replay(&report_path, &img_path, &bad).expect("replay boot");
            assert!(
                divergences.iter().any(|d| matches!(
                    d,
                    record::Divergence::DeviceCompletionMismatch { index, .. } if *index == idx
                )),
                "tampering `{}` must be caught by name, got {divergences:?}",
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

        let tmp_dir =
            std::env::temp_dir().join(format!("wrela-vmm-blk-oob-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let img_path = tmp_dir.join("blk-oob.img");
        let report_path = tmp_dir.join("blk-oob.report.txt");
        std::fs::write(&img_path, &img).expect("write image");
        std::fs::write(&report_path, &built.report_text).expect("write report");
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
            "Machine revision={}\nInput path=x digest=y\nSection name=entry base=0x0 size=1\nEntry base=0x40500000\n\
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
            "Machine revision={}\nInput path=x digest=y\nSection name=entry base=0x0 size=1\nEntry base=0x0\n",
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
    img.supervise(children=[home, away], strategy=Restart.OneForOne,
                  intensity=RestartIntensity(max=3, within=seconds(10)))
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
            "Machine revision={}\nInput path=x digest=y\n\
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
        assert_eq!(parsed.core_entries, vec![(1, 0x40500100), (2, 0x40500200)]);
    }

    /// plans/M8.md item C3: the `Ring` lines the admission recorder reads.
    /// Parsed strictly for the same reason every device line is — a ring
    /// this VMM half-understands is one whose admissions it would silently
    /// under-record, and 06 §8 makes it the recorder of exactly those.
    #[test]
    fn parse_report_reads_request_rings_and_refuses_malformed_ones() {
        let head = format!(
            "Machine revision={}\nInput path=x digest=y\n\
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
            "Machine revision={}\nInput path=x digest=y\n\
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
            "Machine revision={}\nInput path=x digest=y\n\
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
        {
            let text = format!(
                "Machine revision={}\nInput path=x digest=y\n\
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
                "Machine revision={}\nInput path=x digest=y\n\
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
            "Machine revision={}\nInput path=x digest=y\nSection name=entry base=0x0 size=1\nEntry base=0x0\n",
            wrela_machine::MACHINE_REVISION_STR
        );
        assert!(parse_report(&text).expect("parses").blk.is_none());
    }
}
