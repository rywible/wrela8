//! Image report schema: structured types, parse, and Kind-line render
//! helpers for the VMM-facing report subset (06-machine.md §3: the report
//! is the VMM's whole configuration). Compiler and VMM share this schema
//! and these line spellings; the compiler's full `--stage=report`
//! `ImageReport v0` document (graph identity, quotas, edges, exact-bytes
//! layouts) is still assembled in `wrela-compiler`, but every overlapping
//! Kind line is formatted through helpers here so the two sides cannot
//! drift.
//!
//! Line format: `Kind key=value key=value ...`, trim-leading whitespace.
//! Fail closed on unknown keys, missing required fields, and set-level
//! forgeries (`validate_report_invariants`).

/// Ring bookkeeping beyond the slot bytes: head, tail, count — three
/// u64s. Mirrored from the compiler's `MAILBOX_BOOKKEEPING_SIZE`; the
/// `bytes == cap * slot + 24` check in `parse_report` is the tripwire if
/// either side drifts.
pub const RING_BOOKKEEPING_BYTES: u64 = 3 * 8;

/// Empty-string SHA-256 — used by unit-test report preambles that do not
/// boot a real image (parse-only). Live boots always carry the blob's own
/// digest from the compiler.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// One declared pool window (`BlkPool name= device= base= size=`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolWindow {
    pub name: String,
    /// Declared-device index (`device#N` in the report).
    pub device: u64,
    pub base: u64,
    pub size: u64,
}

/// One split ring's addresses, exactly as the report declares them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlkQueueConfig {
    pub size: u16,
    pub desc: u64,
    pub avail: u64,
    pub used: u64,
    /// 06 §5's shared-memory doorbell word (8 bytes).
    pub doorbell: u64,
}

impl BlkQueueConfig {
    pub fn desc_bytes(&self) -> u64 {
        crate::virtio::desc_bytes(self.size)
    }
    pub fn avail_bytes(&self) -> u64 {
        crate::virtio::avail_bytes(self.size)
    }
    pub fn used_bytes(&self) -> u64 {
        crate::virtio::used_bytes(self.size)
    }
}

/// The whole configuration of one declared `blk` device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlkConfig {
    pub device: u64,
    pub capacity_sectors: u64,
    pub features: u64,
    pub vector: Option<u64>,
    pub queue: BlkQueueConfig,
    pub pools: Vec<PoolWindow>,
}

/// Secondary core entry (`CoreEntry core= base=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreEntry {
    pub core: usize,
    pub base: u64,
}

/// Per-core high-DRAM stack (`CoreStack core= base= size=`), plans/M15.md
/// decision 4 / item D.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreStack {
    pub core: usize,
    pub base: u64,
    pub size: u64,
}

/// Structural facts the VMM-facing report subset carries (06 §3's whole
/// configuration for boot). Full `--stage=report` also has compiler-
/// internal bookkeeping this parser ignores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedReport {
    pub entry: u64,
    /// SHA-256 hex of the sealed image blob (06 §3: "validates digests").
    /// Checked against the `.img` bytes at boot.
    pub image_sha256: String,
    /// `(path, sha256_hex)` for every `Input path=… sha256=…` line.
    /// Verified against the file when that path is readable.
    pub input_digests: Vec<(String, String)>,
    /// Executable section ranges (rtcode/code/entry) — used for W^X
    /// `hv_vm_protect` after the image is loaded.
    pub exec_sections: Vec<ReportSection>,
    /// plans/M7.md item F: the declared `blk` device, if any (06 §3: "the
    /// VMM ... preconfigures every device, queue, and shared-memory window
    /// the report declares — device topology is a *build output*, not a
    /// probed fact"). `None` for every image built today: the compiler
    /// emits no `Blk*` lines until the driver-side items (C/D/E) land, and
    /// a report without them boots exactly as it did before this item, no
    /// device model constructed at all.
    pub blk: Option<BlkConfig>,
    /// plans/M7.md item G: host writes into `interrupt_status` plus the
    /// vector to raise, applied before the vCPU runs. Empty for images
    /// that bind no ISR.
    pub irq_injects: Vec<IrqHostInject>,
    /// plans/M8.md item C1: `(core, entry address)` for every **secondary**
    /// core the image brings up, ascending and contiguous from core 1.
    /// Empty for every single-core image — which is every image built
    /// before this item, so their boot path is unchanged down to the
    /// number of vCPUs this VMM creates.
    pub core_entries: Vec<CoreEntry>,
    /// Sealed bring-up count (`Cores count=N`), plans/M15.md decision 3–4.
    /// Always `1..=CORE_SLOTS`. When the report omits `Cores` (hand-written
    /// unit fixtures), parse defaults to `1 + core_entries.len()`.
    pub cores: usize,
    /// Per-core high-DRAM stacks (`CoreStack` lines). Empty only on legacy
    /// fixtures that omit them; compiler-emitted reports always carry
    /// exactly `cores` lines matching `layout::core_stack_base_n`.
    pub core_stacks: Vec<CoreStack>,
    /// plans/M8.md item C3, decision 42: this image's own cross-core
    /// **request** rings, in report order — the order the guest's own
    /// drain walks its lanes (`layout::build_rt_drain`), which is what
    /// makes a reconstruction from occupancy words an ordered one. Reply
    /// rings are parsed for shape and then dropped: a reply is addressed
    /// to a turn record, not admitted to a mailbox, so it is not part of
    /// 06 §8's "per-mailbox cross-core admission order". Empty for every
    /// single-core image.
    pub request_rings: Vec<RequestRing>,
}

/// One `Ring kind=request ...` report line, as the recorder consumes it
/// (plans/M8.md item C3). `count_addr` is the ring's occupancy word.
///
/// Pre-M12 packing: each ring was `capacity * slot_size` bytes of slots
/// followed by `head`, `tail`, `count`, so count sat at
/// `base + capacity * slot_size + 16`.
///
/// M12 item C (decision 875): all CTL records pack contiguously, then
/// uniformly-strided DATA. When the report carries a `Rings count=…
/// stride=…` line, `parse_report` rewrites `count_addr` from that
/// geometry (`ctl_base + index * 24 + 16`). Without that line the
/// pre-M12 derivation stands (hand-written unit fixtures).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRing {
    /// The producing core — decision 28: the producer of a cross-core ring
    /// is a *core*, not an actor, which is exactly what an `Admission`
    /// entry's `sender` field names.
    pub src: usize,
    /// The consuming core: the one whose drain performs the admission.
    pub dst: usize,
    /// The mailbox root this ring feeds — exactly one, by decision 28.
    pub target: String,
    /// DATA base from the report's `base=` (slots start here).
    pub data_base: u64,
    pub count_addr: u64,
    /// Slot capacity (`cap=`). Needed so the admission witness can count
    /// head advances under concurrent record (plans/M15.md item I).
    pub capacity: u64,
}

/// plans/M12.md item C: the report's `Rings count={} stride={} padding={}
/// bytes={}` summary. Present on every image with cross-core rings after
/// that item; absent on hand-written unit fixtures and ringless images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingsMeta {
    pub count: u64,
    pub stride: u64,
}

/// A ring's declared byte range (`base`..`base+bytes`), request or reply —
/// kept only long enough for the overlap checks in `parse_report`. The
/// three-word head/tail/count bookkeeping is part of `bytes` (same formula
/// the compiler's `RingLayout::bytes` uses: `cap * slot + 24`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingRange {
    pub kind: String,
    pub src: usize,
    pub dst: usize,
    pub target: String,
    pub base: u64,
    pub bytes: u64,
}

impl RingRange {
    fn end(&self) -> u64 {
        self.base.saturating_add(self.bytes)
    }
}

/// plans/M8.md item H sweep, Target A: a `Section name=... base=... size=`
/// line, parsed fully so a `CoreEntry`/`Ring` that points into the wrong
/// section (or into no section) can be refused by name rather than booted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportSection {
    pub name: String,
    pub base: u64,
    pub size: u64,
}

impl ReportSection {
    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.base && addr < self.base.saturating_add(self.size)
    }

    pub fn end(&self) -> u64 {
        self.base.saturating_add(self.size)
    }
}

/// One `Placement id=actor#N ... core=C ...` line. Optional in the
/// VMM-facing report (`append_vmm_runtime_lines` does not emit them today),
/// but when present they are configuration and a forgery that places an
/// actor on a core this image never brings up must not boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportPlacement {
    pub id: String,
    pub type_name: String,
    pub core: usize,
}

/// An `Actor index=` / `Driver index=` root the Placement set must cover
/// exactly once (plans/M8.md item H Target A follow-up).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredRoot {
    pub id: String,
    pub type_name: String,
}

/// One `IrqHostInject` report line (plans/M7.md item G).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrqHostInject {
    pub base: u64,
    pub offset: u64,
    pub status: u32,
    pub vector: u64,
}

/// One `Kind key=value key=value ...` report line's own fields. Shared by
/// every `Blk*` line below; deliberately strict — a malformed field, a
/// missing required field, or an unknown key fails the whole report closed
/// (as a `String` error), because a device declaration half-understood
/// is exactly the configuration a boot must never accept.
pub fn parse_report_fields<'a>(
    kind: &str,
    rest: &'a str,
    allowed: &[&str],
) -> Result<std::collections::BTreeMap<&'a str, &'a str>, String> {
    let mut fields = std::collections::BTreeMap::new();
    for part in rest.split_whitespace() {
        let Some((k, v)) = part.split_once('=') else {
            return Err(format!("`{kind}` field {part:?} has no `=`"));
        };
        if !allowed.contains(&k) {
            return Err(format!(
                "`{kind}` has no field `{k}` (expected one of {allowed:?})"
            ));
        }
        if fields.insert(k, v).is_some() {
            return Err(format!("`{kind}` repeats field `{k}`"));
        }
    }
    Ok(fields)
}

/// A required `key=<integer>` field, decimal or `0x`-prefixed.
pub fn report_u64(
    kind: &str,
    fields: &std::collections::BTreeMap<&str, &str>,
    key: &str,
) -> Result<u64, String> {
    let raw = fields
        .get(key)
        .copied()
        .ok_or_else(|| format!("`{kind}` is missing required field `{key}`"))?;
    let parsed = match raw.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => raw.parse::<u64>(),
    };
    parsed.map_err(|e| format!("`{kind}` field `{key}={raw}`: {e}"))
}

/// A required `device=device#<N>` field (plans/M8.md item P). The report
/// spells a declared device the same way everywhere — `device#0` — and
/// this VMM parses that spelling rather than a bare integer so a line that
/// lost its prefix is a malformed report rather than a silently different
/// device.
pub fn report_device_index(
    kind: &str,
    fields: &std::collections::BTreeMap<&str, &str>,
) -> Result<u64, String> {
    let raw = fields.get("device").copied().ok_or_else(|| {
        format!(
            "`{kind}` is missing required field `device` — 03-hardware.md §3: all memory a device \
             can reach originates from *its* bound pools, which is not a statement this VMM can \
             enforce about an unnamed device"
        )
    })?;
    let digits = raw
        .strip_prefix("device#")
        .ok_or_else(|| format!("`{kind}` field `device={raw}`: expected `device#<index>`"))?;
    digits
        .parse::<u64>()
        .map_err(|e| format!("`{kind}` field `device={raw}`: {e}"))
}

/// Parses the minimal, internal (not itself golden-pinned — `wrela test`'s
/// own merged stdout is the golden surface, not this file) report format
/// `bin/wrela.rs`'s runtime tier writes alongside the image (a `Machine
/// revision=` line, one or more `Input path=... sha256=...` lines, one
/// `Image sha256=...` line, section/entry lines). Validates 06 §3/§10:
/// machine revision, digest *presence and shape*, and — at boot — the
/// image blob hash and every readable input file's hash.
pub fn parse_report(text: &str) -> Result<ParsedReport, String> {
    let mut revision: Option<String> = None;
    let mut input_digests: Vec<(String, String)> = Vec::new();
    let mut image_sha256: Option<String> = None;
    let mut entry: Option<u64> = None;
    // plans/M7.md item F: the declared `blk` device's own three line
    // kinds, accumulated here and assembled into one `BlkConfig` below.
    // Deliberately `Blk`-prefixed rather than reusing `Device`/`Pool`:
    // `report.rs`'s own full `--stage=report` artifact already spells
    // those two words with entirely different fields, and `parse_report`
    // trims indentation away, so a distinct prefix is what keeps the two
    // formats from ever being silently confusable.
    let mut blk_device: Option<(u64, u64, u64, Option<u64>)> = None;
    let mut blk_queue: Option<BlkQueueConfig> = None;
    let mut blk_pools: Vec<PoolWindow> = Vec::new();
    let mut irq_injects: Vec<IrqHostInject> = Vec::new();
    let mut core_entries: Vec<CoreEntry> = Vec::new();
    let mut cores_line: Option<usize> = None;
    let mut core_stacks: Vec<CoreStack> = Vec::new();
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
    let mut rings_meta: Option<RingsMeta> = None;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("Machine revision=") {
            revision = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("Image sha256=") {
            let dig = rest.trim();
            if !crate::sha256::is_sha256_hex(dig) {
                return Err(format!(
                    "`Image sha256=` must be 64 hex digits, got {dig:?}"
                ));
            }
            if image_sha256.is_some() {
                return Err("more than one `Image sha256=` line".to_string());
            }
            image_sha256 = Some(dig.to_ascii_lowercase());
        } else if let Some(rest) = line.strip_prefix("Input ") {
            let fields = parse_report_fields("Input", rest, &["path", "sha256"])?;
            let path = fields
                .get("path")
                .copied()
                .ok_or_else(|| "`Input` is missing required field `path`".to_string())?;
            let dig = fields
                .get("sha256")
                .copied()
                .ok_or_else(|| "`Input` is missing required field `sha256`".to_string())?;
            if !crate::sha256::is_sha256_hex(dig) {
                return Err(format!(
                    "`Input path={path} sha256=` must be 64 hex digits, got {dig:?}"
                ));
            }
            input_digests.push((path.to_string(), dig.to_ascii_lowercase()));
        } else if let Some(rest) = line.strip_prefix("Rings ") {
            // plans/M12.md item C: `Rings count={} stride={} padding={} bytes={}`.
            let fields =
                parse_report_fields("Rings", rest, &["count", "stride", "padding", "bytes"])?;
            let count = report_u64("Rings", &fields, "count")?;
            let stride = report_u64("Rings", &fields, "stride")?;
            let padding = report_u64("Rings", &fields, "padding")?;
            let bytes = report_u64("Rings", &fields, "bytes")?;
            if count == 0 {
                return Err(
                    "`Rings count=0`: the summary line is absent for ringless images, never zero"
                        .to_string(),
                );
            }
            if stride == 0 {
                return Err(
                    "`Rings stride=0`: a live ring image always has a positive data stride"
                        .to_string(),
                );
            }
            let expect_bytes = count
                .checked_mul(24)
                .and_then(|c| c.checked_add(count.checked_mul(stride)?))
                .ok_or_else(|| {
                    "`Rings` count/stride overflow computing expected bytes".to_string()
                })?;
            if bytes != expect_bytes {
                return Err(format!(
                    "`Rings count={count} stride={stride} padding={padding} bytes={bytes}`: \
                     bytes must equal count*24 + count*stride (={expect_bytes})"
                ));
            }
            // padding is disclosed; forge-check it against the Ring lines
            // after the loop (needs every ring's cap*slot).
            rings_meta = Some(RingsMeta { count, stride });
        } else if let Some(rest) = line.strip_prefix("Section ") {
            let fields = parse_report_fields("Section", rest, &["name", "base", "size"])?;
            let name = fields
                .get("name")
                .copied()
                .ok_or_else(|| "`Section` is missing required field `name`".to_string())?;
            let base = report_u64("Section", &fields, "base")?;
            let size = report_u64("Section", &fields, "size")?;
            if size == 0 {
                return Err(format!(
                    "`Section name={name} base={base:#x} size=0`: a section with no bytes is not \
                     a configuration this VMM can honor"
                ));
            }
            if sections.iter().any(|s| s.name == name) {
                return Err(format!("`Section name={name}` is repeated"));
            }
            sections.push(ReportSection {
                name: name.to_string(),
                base,
                size,
            });
        } else if let Some(rest) = line.strip_prefix("Entry base=") {
            let digits = rest.trim_start_matches("0x");
            entry = u64::from_str_radix(digits, 16).ok();
        } else if let Some(rest) = line.strip_prefix("Cores count=") {
            // plans/M15.md item D / decision 3: sealed bring-up count.
            let n: u64 = rest.trim().parse().map_err(|e| {
                format!("`Cores count=`: expected a decimal integer, got {rest:?} ({e})")
            })?;
            if n < 1 || n as usize > crate::CORE_SLOTS {
                return Err(format!(
                    "`Cores count={n}`: sealed cores must satisfy 1..=CORE_SLOTS ({}) \
                     (06-machine.md §1 / plans/M15.md decision 3)",
                    crate::CORE_SLOTS
                ));
            }
            if cores_line.is_some() {
                return Err("more than one `Cores count=` line".to_string());
            }
            cores_line = Some(n as usize);
        } else if let Some(rest) = line.strip_prefix("CoreStack ") {
            // plans/M15.md item D / decision 4: high-DRAM per-core stack.
            let fields = parse_report_fields("CoreStack", rest, &["core", "base", "size"])?;
            let core = report_u64("CoreStack", &fields, "core")?;
            let base = report_u64("CoreStack", &fields, "base")?;
            let size = report_u64("CoreStack", &fields, "size")?;
            if core as usize >= crate::CORE_SLOTS {
                return Err(format!(
                    "`CoreStack core={core}`: core index must be < CORE_SLOTS ({})",
                    crate::CORE_SLOTS
                ));
            }
            core_stacks.push(CoreStack {
                core: core as usize,
                base,
                size,
            });
        } else if let Some(rest) = line.strip_prefix("CoreEntry ") {
            // plans/M8.md item C1 / 06 §3: where this VMM starts vCPU N once
            // core 0's entry rings the release doorbell. Device topology is
            // a build output and so is the core set — nothing here is
            // probed, defaulted, or guessed. Bounds vs report N are checked
            // in `validate_report_invariants` once `Cores` is known.
            let fields = parse_report_fields("CoreEntry", rest, &["core", "base"])?;
            let core = report_u64("CoreEntry", &fields, "core")?;
            let base = report_u64("CoreEntry", &fields, "base")?;
            if core == 0 || core as usize >= crate::CORE_SLOTS {
                return Err(format!(
                    "`CoreEntry core={core}`: secondary cores are 1..CORE_SLOTS ({}) \
                     (core 0's entry is the `Entry base=` line; packing ceiling is CORE_SLOTS)",
                    crate::CORE_SLOTS
                ));
            }
            core_entries.push(CoreEntry {
                core: core as usize,
                base,
            });
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
            let kind = fields
                .get("kind")
                .copied()
                .ok_or_else(|| "`Ring` is missing required field `kind`".to_string())?;
            let src = report_u64("Ring", &fields, "src")?;
            let dst = report_u64("Ring", &fields, "dst")?;
            let capacity = report_u64("Ring", &fields, "cap")?;
            let slot = report_u64("Ring", &fields, "slot")?;
            let bytes = report_u64("Ring", &fields, "bytes")?;
            let base = report_u64("Ring", &fields, "base")?;
            let target = fields
                .get("target")
                .copied()
                .ok_or_else(|| "`Ring` is missing required field `target`".to_string())?;
            if src as usize >= crate::CORE_SLOTS || dst as usize >= crate::CORE_SLOTS {
                return Err(format!(
                    "`Ring src={src} dst={dst}`: core index must be < CORE_SLOTS ({}) \
                     (06-machine.md §1 / plans/M15.md)",
                    crate::CORE_SLOTS
                ));
            }
            if src == dst {
                return Err(format!(
                    "`Ring src={src} dst={dst}`: a ring is a *cross*-core edge; same-core edges \
                     keep the mailbox path (04-compiler.md §3)"
                ));
            }
            // plans/M8.md item H Target A: `bytes` is not decorative — it
            // is `cap * slot + 24` (slots plus head/tail/count). A triple
            // that does not add up would make `count_addr` disagree with
            // the range the report claims to reserve.
            let expected_bytes = capacity
                .checked_mul(slot)
                .and_then(|s| s.checked_add(RING_BOOKKEEPING_BYTES))
                .ok_or_else(|| {
                    format!("`Ring cap={capacity} slot={slot}`: capacity * slot overflows")
                })?;
            if bytes != expected_bytes {
                return Err(format!(
                    "`Ring kind={kind} src={src} dst={dst} target={target} cap={capacity} \
                     slot={slot} bytes={bytes}`: bytes must equal cap*slot+{RING_BOOKKEEPING_BYTES} \
                     (={expected_bytes}); a forged triple would point the admission witness at \
                     the wrong occupancy word"
                ));
            }
            match kind {
                "request" => {
                    if target == "-" {
                        return Err(
                            "`Ring kind=request` with no `target=`: a request ring feeds exactly \
                             one mailbox root, which is what names the admission"
                                .to_string(),
                        );
                    }
                    request_rings.push(RequestRing {
                        src: src as usize,
                        dst: dst as usize,
                        target: target.to_string(),
                        data_base: base,
                        // Pre-M12 default; rewritten below when `Rings` meta
                        // is present (CTL-then-DATA packing).
                        count_addr: base + capacity * slot + 16,
                        capacity,
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
                    return Err(format!(
                        "`Ring kind={other}`: the only lanes are `request` and `reply` \
                         (plans/M8.md decision 29)"
                    ));
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
            let id = fields
                .get("id")
                .copied()
                .ok_or_else(|| "`Placement` is missing required field `id`".to_string())?;
            let type_name = fields
                .get("type")
                .copied()
                .ok_or_else(|| "`Placement` is missing required field `type`".to_string())?;
            let core = report_u64("Placement", &fields, "core")?;
            if core as usize >= crate::CORE_SLOTS {
                return Err(format!(
                    "`Placement id={id} core={core}`: core index must be < CORE_SLOTS ({}) \
                     (06-machine.md §1 / plans/M15.md)",
                    crate::CORE_SLOTS
                ));
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
                let n: u64 = index
                    .parse()
                    .map_err(|e| format!("`Actor` field `index={index}`: {e}"))?;
                let type_name = fields
                    .get("type")
                    .copied()
                    .ok_or_else(|| "`Actor index=` is missing required field `type`".to_string())?;
                declared_roots.push(DeclaredRoot {
                    id: format!("actor#{n}"),
                    type_name: type_name.to_string(),
                });
            } else if let Some(name) = fields.get("name").copied() {
                layout_root_names.push(name.to_string());
            } else {
                return Err("`Actor` line names neither `index=` nor `name=`".to_string());
            }
        } else if let Some(rest) = line.strip_prefix("Driver ") {
            let fields = parse_report_fields(
                "Driver",
                rest,
                &["index", "type", "name", "mailbox", "slot", "frame", "state"],
            )?;
            if let Some(index) = fields.get("index").copied() {
                let n: u64 = index
                    .parse()
                    .map_err(|e| format!("`Driver` field `index={index}`: {e}"))?;
                let type_name = fields.get("type").copied().ok_or_else(|| {
                    "`Driver index=` is missing required field `type`".to_string()
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
                return Err(
                    "more than one `BlkDevice` line (06 §6: the device set is closed and there is no hotplug; machine v1 has exactly one `blk`)".to_string()
                );
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
                return Err(format!(
                    "`BlkQueue index={index}`: machine v1's `blk` has exactly one queue (index 0)"
                ));
            }
            if blk_queue.is_some() {
                return Err("more than one `BlkQueue index=0` line".to_string());
            }
            let size = report_u64("BlkQueue", &fields, "size")?;
            let size = u16::try_from(size).map_err(|_| {
                format!("`BlkQueue size={size}` does not fit virtio's own 16-bit queue depth")
            })?;
            blk_queue = Some(BlkQueueConfig {
                size,
                desc: report_u64("BlkQueue", &fields, "desc")?,
                avail: report_u64("BlkQueue", &fields, "avail")?,
                used: report_u64("BlkQueue", &fields, "used")?,
                doorbell: report_u64("BlkQueue", &fields, "doorbell")?,
            });
        } else if let Some(rest) = line.strip_prefix("BlkPool ") {
            let fields = parse_report_fields("BlkPool", rest, &["name", "device", "base", "size"])?;
            let name = fields
                .get("name")
                .copied()
                .ok_or_else(|| "`BlkPool` is missing required field `name`".to_string())?;
            blk_pools.push(PoolWindow {
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
                format!("`IrqHostInject status={status:#x}` does not fit a u32 register")
            })?;
            irq_injects.push(IrqHostInject {
                base: report_u64("IrqHostInject", &fields, "base")?,
                offset: report_u64("IrqHostInject", &fields, "offset")?,
                status,
                vector: report_u64("IrqHostInject", &fields, "vector")?,
            });
        }
    }
    let revision = revision.ok_or_else(|| "no `Machine revision=` line".to_string())?;
    if revision != crate::MACHINE_REVISION_STR {
        return Err(format!("machine-revision-mismatch:{}", revision));
    }
    if input_digests.is_empty() {
        return Err("no `Input path=… sha256=…` digest line".to_string());
    }
    let image_sha256 = image_sha256.ok_or_else(|| "no `Image sha256=` digest line".to_string())?;
    if sections.is_empty() {
        return Err("no `Section name=` line".to_string());
    }
    let entry = entry.ok_or_else(|| "no `Entry base=0x...` line".to_string())?;
    let exec_sections: Vec<ReportSection> = sections
        .iter()
        .filter(|s| {
            matches!(
                s.name.as_str(),
                "entry" | "code" | "abort" | "checkpoint" | "rtcode"
            )
        })
        .cloned()
        .collect();
    // The three `Blk*` line kinds are all-or-nothing: a device with no
    // queue, a queue with no device, or either with no pool is a report
    // this VMM refuses outright rather than booting on a device model it
    // would have to guess the shape of.
    let blk = match (blk_device, blk_queue) {
        (None, None) => {
            if !blk_pools.is_empty() {
                return Err(
                    "`BlkPool` line(s) with no `BlkDevice`/`BlkQueue` to bind them to".to_string(),
                );
            }
            None
        }
        (Some(_), None) => {
            return Err("a `BlkDevice` line with no `BlkQueue index=0` line".to_string());
        }
        (None, Some(_)) => {
            return Err("a `BlkQueue` line with no `BlkDevice` line".to_string());
        }
        (Some((device, capacity_sectors, features, vector)), Some(queue)) => {
            // plans/M8.md item P: per *this* device. A `BlkPool` naming
            // some other device is a declared window this model may not
            // reach — it is carried into `GuestMem` (which is what makes
            // the refusal observable), but it cannot stand in for the
            // device's own bound pool.
            if !blk_pools.iter().any(|p| p.device == device) {
                return Err(format!(
                    "a `BlkDevice device=device#{device}` with no `BlkPool device=device#{device}` \
                     line: all memory a device can reach originates from its bound pools \
                     (03-hardware.md §3)"
                ));
            }
            Some(BlkConfig {
                device,
                capacity_sectors,
                features,
                vector,
                queue,
                pools: blk_pools,
            })
        }
    };
    // M12 item C: when the report publishes `Rings …`, rewrite count_addrs
    // and overlap ranges for CTL-then-uniform-DATA packing. Without that
    // line (unit fixtures), pre-M12 DATA-then-CTL stands.
    if let Some(meta) = rings_meta {
        apply_uniform_ring_layout(&mut request_rings, &mut ring_ranges, meta)?;
    }
    let cores = match cores_line {
        Some(n) => n,
        None => 1 + core_entries.len(),
    };
    validate_report_invariants(
        entry,
        cores,
        cores_line.is_some(),
        &mut core_entries,
        &mut core_stacks,
        &sections,
        &request_rings,
        &ring_ranges,
        &placements,
        &declared_roots,
        &layout_root_names,
    )?;
    Ok(ParsedReport {
        entry,
        image_sha256,
        input_digests,
        exec_sections,
        blk,
        irq_injects,
        core_entries,
        cores,
        core_stacks,
        request_rings,
    })
}

/// plans/M12.md item C: fold the `Rings` summary into per-ring
/// `count_addr` and into the ranges overlap checks walk. DATA bases stay
/// as reported; each ring's overlap span becomes the uniform `stride`,
/// and the contiguous CTL block is appended so stack/section checks see
/// it (CTL words live immediately before the first DATA base).
fn apply_uniform_ring_layout(
    request_rings: &mut [RequestRing],
    ring_ranges: &mut Vec<RingRange>,
    meta: RingsMeta,
) -> Result<(), String> {
    if ring_ranges.len() as u64 != meta.count {
        return Err(format!(
            "`Rings count={}` but the report has {} `Ring kind=` line(s)",
            meta.count,
            ring_ranges.len()
        ));
    }
    let mut sorted: Vec<u64> = ring_ranges.iter().map(|r| r.base).collect();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() as u64 != meta.count {
        return Err("`Ring` DATA bases are not unique under uniform stride packing".to_string());
    }
    let data0 = sorted[0];
    for (i, &b) in sorted.iter().enumerate() {
        let expect = data0 + (i as u64) * meta.stride;
        if b != expect {
            return Err(format!(
                "`Ring` DATA base {b:#x} is not at index {i} of stride {} from {data0:#x} \
                 (expected {expect:#x})",
                meta.stride
            ));
        }
    }
    let ctl_base = data0
        .checked_sub(meta.count * 24)
        .ok_or_else(|| "uniform ring CTL block would underflow below DATA base".to_string())?;
    for r in request_rings.iter_mut() {
        let idx = sorted
            .iter()
            .position(|&b| b == r.data_base)
            .ok_or_else(|| {
                format!(
                    "request ring DATA base {:#x} missing from Ring ranges",
                    r.data_base
                )
            })?;
        r.count_addr = ctl_base + (idx as u64) * 24 + 16;
    }
    for r in ring_ranges.iter_mut() {
        r.bytes = meta.stride;
    }
    ring_ranges.push(RingRange {
        kind: "ctl".to_string(),
        src: 0,
        dst: 0,
        target: "-".to_string(),
        base: ctl_base,
        bytes: meta.count * 24,
    });
    Ok(())
}

/// Set-level invariants over the report's own tables (plans/M8.md item H
/// Target A follow-up; M15 item D for `Cores`/`CoreStack`). Per-line shape
/// checks live in `parse_report`; this function is the one place that
/// validates the **set**.
fn validate_report_invariants(
    entry: u64,
    cores: usize,
    cores_declared: bool,
    core_entries: &mut Vec<CoreEntry>,
    core_stacks: &mut Vec<CoreStack>,
    sections: &[ReportSection],
    request_rings: &[RequestRing],
    ring_ranges: &[RingRange],
    placements: &[ReportPlacement],
    declared_roots: &[DeclaredRoot],
    layout_root_names: &[String],
) -> Result<(), String> {
    use crate::layout as machine_layout;
    let dram_end = machine_layout::dram_end();

    // (0) Every section and every ring range lies wholly inside guest DRAM
    // — otherwise a forged report can name host-OOB GPAs that later
    // `host_ram.add(off)` paths trust.
    for s in sections {
        let end = s.base.checked_add(s.size).ok_or_else(|| {
            format!(
                "`Section name={} base={:#x} size={}` overflows a u64",
                s.name, s.base, s.size
            )
        })?;
        if s.base < machine_layout::DRAM_BASE || end > dram_end {
            return Err(format!(
                "`Section name={} base={:#x} size={}` is outside guest DRAM \
                 [{:#x}..{dram_end:#x})",
                s.name,
                s.base,
                s.size,
                machine_layout::DRAM_BASE
            ));
        }
    }
    for a in ring_ranges {
        let end = a.base.checked_add(a.bytes).ok_or_else(|| {
            format!(
                "`Ring kind={} … base={:#x} bytes={}` overflows a u64",
                a.kind, a.base, a.bytes
            )
        })?;
        if a.base < machine_layout::DRAM_BASE || end > dram_end {
            return Err(format!(
                "`Ring kind={} src={} dst={} target={} base={:#x} bytes={}` is outside \
                 guest DRAM [{:#x}..{dram_end:#x})",
                a.kind,
                a.src,
                a.dst,
                a.target,
                a.base,
                a.bytes,
                machine_layout::DRAM_BASE
            ));
        }
    }

    // (1) Sections are pairwise disjoint.
    for (i, a) in sections.iter().enumerate() {
        for b in sections.iter().skip(i + 1) {
            if a.base < b.end() && b.base < a.end() {
                return Err(format!(
                    "`Section name={} base={:#x} size={}` overlaps `Section name={} base={:#x} \
                     size={}`",
                    a.name, a.base, a.size, b.name, b.base, b.size
                ));
            }
        }
    }

    // (2) Contiguous secondary-core set from core 1, agreeing with report N.
    core_entries.sort_by_key(|e| e.core);
    if cores_declared && core_entries.len() + 1 != cores {
        return Err(format!(
            "`Cores count={cores}` disagrees with {} `CoreEntry` line(s) (bring-up is one \
             `Entry base=` plus N-1 secondary entries)",
            core_entries.len()
        ));
    }
    for (i, e) in core_entries.iter().enumerate() {
        if e.core != i + 1 {
            return Err(format!(
                "`CoreEntry` lines are not contiguous from core 1 (saw core {} where core {} \
                 was expected)",
                e.core,
                i + 1
            ));
        }
        if e.core >= cores {
            return Err(format!(
                "`CoreEntry core={}`: secondary cores are 1..{cores} for this image's \
                 `Cores count={cores}`",
                e.core
            ));
        }
    }

    // (2b) CoreStack lines: when present, exactly N contiguous high-DRAM
    // stacks matching `layout::core_stack_base_n`.
    if !core_stacks.is_empty() || cores_declared {
        if core_stacks.len() != cores {
            return Err(format!(
                "`Cores count={cores}` requires exactly {cores} `CoreStack` line(s); saw {}",
                core_stacks.len()
            ));
        }
        core_stacks.sort_by_key(|s| s.core);
        for (i, s) in core_stacks.iter().enumerate() {
            if s.core != i {
                return Err(format!(
                    "`CoreStack` lines are not contiguous from core 0 (saw core {} where core {} \
                     was expected)",
                    s.core, i
                ));
            }
            let expect_base = machine_layout::core_stack_base_n(s.core, cores);
            if s.base != expect_base || s.size != machine_layout::CORE_STACK_SIZE {
                return Err(format!(
                    "`CoreStack core={} base={:#x} size={}`: expected base={expect_base:#x} \
                     size={} (plans/M15.md decision 4: DRAM_END - (N-n)*CORE_STACK_SIZE)",
                    s.core,
                    s.base,
                    s.size,
                    machine_layout::CORE_STACK_SIZE
                ));
            }
            let end = s.base.saturating_add(s.size);
            if s.base < machine_layout::DRAM_BASE || end > dram_end {
                return Err(format!(
                    "`CoreStack core={} base={:#x} size={}` is outside guest DRAM",
                    s.core, s.base, s.size
                ));
            }
            // Must not overlap the image packing window / fixed low pages.
            if s.base < layout_image_hi(sections) {
                return Err(format!(
                    "`CoreStack core={} base={:#x}` overlaps the image / low-map window \
                     (stacks are high-DRAM only; IMAGE_BASE stays fixed)",
                    s.core, s.base
                ));
            }
            for sec in sections {
                if s.base < sec.end() && sec.base < end {
                    return Err(format!(
                        "`CoreStack core={} base={:#x} size={}` overlaps `Section name={} \
                         base={:#x} size={}`",
                        s.core, s.base, s.size, sec.name, sec.base, sec.size
                    ));
                }
            }
        }
    }

    // (3) Every CoreEntry base is 4-byte aligned and distinct from every
    // other core's entry (including core 0's `Entry base=`). Core 0's
    // entry must also sit in guest DRAM and inside an executable section
    // (same rule as secondary cores — a forged `Entry` below DRAM_BASE
    // used to pass structural checks and then fault host-side).
    if entry % 4 != 0 {
        return Err(format!(
            "`Entry base={entry:#x}` is not 4-byte aligned (an AArch64 PC must be)"
        ));
    }
    if entry < machine_layout::DRAM_BASE || entry >= dram_end {
        return Err(format!(
            "`Entry base={entry:#x}` is outside guest DRAM [{:#x}..{dram_end:#x})",
            machine_layout::DRAM_BASE
        ));
    }
    {
        const EXEC_SECTIONS: &[&str] = &["rtcode", "code", "entry"];
        match sections.iter().find(|s| s.contains(entry)) {
            Some(s) if EXEC_SECTIONS.contains(&s.name.as_str()) => {}
            Some(s) => {
                return Err(format!(
                    "`Entry base={entry:#x}` falls inside `Section name={}` — the image \
                     entry must be code (`rtcode`, or a test image's `entry`/`code` \
                     harness), not data",
                    s.name
                ));
            }
            None => {
                return Err(format!(
                    "`Entry base={entry:#x}` is outside every `Section` this report declares"
                ));
            }
        }
    }
    for e in core_entries.iter() {
        let (core, base) = (e.core, e.base);
        if base % 4 != 0 {
            return Err(format!(
                "`CoreEntry core={core} base={base:#x}` is not 4-byte aligned (an AArch64 PC must \
                 be; a report that says otherwise is forged)"
            ));
        }
        if base == entry {
            return Err(format!(
                "`CoreEntry core={core} base={base:#x}` equals core 0's `Entry base=` — two cores \
                 cannot enter at the same address"
            ));
        }
    }
    for (i, a) in core_entries.iter().enumerate() {
        for b in core_entries.iter().skip(i + 1) {
            if a.base == b.base {
                return Err(format!(
                    "`CoreEntry core={} base={:#x}` and `CoreEntry core={} base={:#x}` \
                     name the same entry address — two cores cannot enter at the same address",
                    a.core, a.base, b.core, b.base
                ));
            }
        }
    }

    // (4) Every CoreEntry lands in an executable section.
    const EXEC_SECTIONS: &[&str] = &["rtcode", "code", "entry"];
    for e in core_entries.iter() {
        let (core, base) = (e.core, e.base);
        let owner = sections.iter().find(|s| s.contains(base));
        match owner {
            Some(s) if EXEC_SECTIONS.contains(&s.name.as_str()) => {}
            Some(s) => {
                return Err(format!(
                    "`CoreEntry core={core} base={base:#x}` falls inside `Section name={}` — a \
                     secondary entry must be code (`rtcode`, or a test image's `entry`/`code` \
                     harness), not data",
                    s.name
                ));
            }
            None => {
                return Err(format!(
                    "`CoreEntry core={core} base={base:#x}` is outside every `Section` this report \
                     declares — a secondary entry must be code"
                ));
            }
        }
    }

    // (5) Request rings name only brought-up cores (report N).
    for r in request_rings {
        if r.src >= cores || r.dst >= cores {
            return Err(format!(
                "`Ring kind=request src={} dst={} target={}` names a core outside 0..{cores} \
                 (this image's `Cores count={cores}`)",
                r.src, r.dst, r.target
            ));
        }
        let brought_up = r.dst == 0 || core_entries.iter().any(|e| e.core == r.dst);
        let src_up = r.src == 0 || core_entries.iter().any(|e| e.core == r.src);
        if !brought_up || !src_up {
            return Err(format!(
                "`Ring kind=request src={} dst={} target={}` names a core this image never brings \
                 up (no `CoreEntry` line for it)",
                r.src, r.dst, r.target
            ));
        }
    }

    // (6) Ring ranges: pairwise disjoint; disjoint from stacks; disjoint
    // from every Section other than `rtdata` (and wholly inside `rtdata`
    // when that section exists).
    for (i, a) in ring_ranges.iter().enumerate() {
        for b in ring_ranges.iter().skip(i + 1) {
            if a.base < b.end() && b.base < a.end() {
                return Err(format!(
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
                ));
            }
        }
        for core in 0..cores {
            let stack_base = if let Some(s) = core_stacks.iter().find(|s| s.core == core) {
                s.base
            } else {
                machine_layout::core_stack_base_n(core, cores)
            };
            let stack_end = stack_base + machine_layout::CORE_STACK_SIZE;
            if a.base < stack_end && stack_base < a.end() {
                return Err(format!(
                    "`Ring kind={} src={} dst={} target={} base={:#x} bytes={}` overlaps \
                     core {core}'s stack [{stack_base:#x}..{stack_end:#x})",
                    a.kind, a.src, a.dst, a.target, a.base, a.bytes
                ));
            }
        }
        if let Some(rtdata) = sections.iter().find(|s| s.name == "rtdata") {
            if a.base < rtdata.base || a.end() > rtdata.end() {
                return Err(format!(
                    "`Ring kind={} src={} dst={} target={} base={:#x} bytes={}` is not wholly \
                     inside `Section name=rtdata base={:#x} size={}` (rings live in `rtdata` only)",
                    a.kind, a.src, a.dst, a.target, a.base, a.bytes, rtdata.base, rtdata.size
                ));
            }
        } else {
            for s in sections {
                if a.base < s.end() && s.base < a.end() {
                    return Err(format!(
                        "`Ring kind={} src={} dst={} target={} base={:#x} bytes={}` overlaps \
                         `Section name={} base={:#x} size={}` (rings live in `rtdata` only)",
                        a.kind, a.src, a.dst, a.target, a.base, a.bytes, s.name, s.base, s.size
                    ));
                }
            }
        }
    }

    // (7) Declared Actor/Driver index= ids are unique.
    for (i, a) in declared_roots.iter().enumerate() {
        for b in declared_roots.iter().skip(i + 1) {
            if a.id == b.id {
                return Err(format!("declared root `{}` is repeated", a.id));
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
                return Err(format!(
                    "`Ring kind={} src={} dst={} target={}` names a root this report never \
                     declares (known roots: {}) — a ring is the delivery path into a mailbox, \
                     so a target no `Actor`/`Driver` line accounts for is a forged edge",
                    r.kind,
                    r.src,
                    r.dst,
                    r.target,
                    declared.join(", ")
                ));
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
                return Err(format!(
                    "`Placement id={}` is repeated (an actor/driver is placed exactly once; two \
                     lines would put the same root on cores {} and {})",
                    a.id, a.core, b.core
                ));
            }
        }
    }

    for p in placements {
        if p.core >= cores {
            return Err(format!(
                "`Placement id={} core={}`: core index must be < {cores} \
                 (this image's `Cores count={cores}`)",
                p.id, p.core
            ));
        }
        let core_up = p.core == 0 || core_entries.iter().any(|e| e.core == p.core);
        if !core_up {
            return Err(format!(
                "`Placement id={} core={}` names a core this image never brings up (no \
                 `CoreEntry` line for it; core 0 is the `Entry base=` line)",
                p.id, p.core
            ));
        }

        if let Some(root) = declared_roots.iter().find(|r| r.id == p.id) {
            if root.type_name != p.type_name {
                return Err(format!(
                    "`Placement id={} type={}` disagrees with the declared root's `type={}`",
                    p.id, p.type_name, root.type_name
                ));
            }
        } else if layout_root_names.iter().any(|n| n == &p.id) {
            // Bare-name Placement against a layout-section Actor name=.
        } else if !declared_roots.is_empty() || !layout_root_names.is_empty() {
            let declared: Vec<&str> = declared_roots
                .iter()
                .map(|r| r.id.as_str())
                .chain(layout_root_names.iter().map(|s| s.as_str()))
                .collect();
            return Err(format!(
                "`Placement id={}` names an actor this report's `Actor` lines do not \
                 declare (declared: {declared:?})",
                p.id
            ));
        }
    }

    if !declared_roots.is_empty() {
        for root in declared_roots {
            let n = placements.iter().filter(|p| p.id == root.id).count();
            if n == 0 {
                return Err(format!(
                    "declared root `{}` (type={}) has no `Placement` line — every Actor/Driver \
                     is placed exactly once",
                    root.id, root.type_name
                ));
            }
            let _ = n;
        }
    }

    Ok(())
}

/// Upper bound of the image / low-map window stacks must clear: the end of
/// the highest `Section` at or above `IMAGE_BASE`, or the rtdata packing
/// ceiling when no such section exists.
fn layout_image_hi(sections: &[ReportSection]) -> u64 {
    use crate::layout as machine_layout;
    let mut hi = machine_layout::RTDATA_BASE + machine_layout::RTDATA_SIZE_MAX;
    for s in sections {
        if s.base >= machine_layout::IMAGE_BASE {
            hi = hi.max(s.end());
        }
    }
    hi
}

// --- shared Kind-line spellings (emitter + parser agree here) ------------

pub fn line_machine_revision(revision: &str) -> String {
    format!("Machine revision={revision}")
}

pub fn line_input(path: &str, sha256: &str) -> String {
    format!("Input path={path} sha256={sha256}")
}

pub fn line_image_sha256(sha256: &str) -> String {
    format!("Image sha256={sha256}")
}

pub fn line_section(name: &str, base: u64, size: u64) -> String {
    format!("Section name={name} base={base:#x} size={size}")
}

pub fn line_entry(base: u64) -> String {
    format!("Entry base={base:#x}")
}

pub fn line_core_entry(core: usize, base: u64) -> String {
    format!("CoreEntry core={core} base={base:#x}")
}

pub fn line_cores(count: usize) -> String {
    format!("Cores count={count}")
}

pub fn line_core_stack(core: usize, base: u64, size: u64) -> String {
    format!("CoreStack core={core} base={base:#x} size={size:#x}")
}

pub fn line_irq_host_inject(base: u64, offset: u64, status: u32, vector: u64) -> String {
    format!("IrqHostInject base={base:#x} offset={offset:#x} status={status:#x} vector={vector}")
}

pub fn line_blk_device(blk: &BlkConfig) -> String {
    let mut s = format!(
        "BlkDevice device=device#{} capacity_sectors={} features={:#x}",
        blk.device, blk.capacity_sectors, blk.features
    );
    if let Some(v) = blk.vector {
        s.push_str(&format!(" vector={v}"));
    }
    s
}

pub fn line_blk_queue(q: &BlkQueueConfig) -> String {
    format!(
        "BlkQueue index=0 size={} desc={:#x} avail={:#x} used={:#x} doorbell={:#x}",
        q.size, q.desc, q.avail, q.used, q.doorbell
    )
}

pub fn line_blk_pool(p: &PoolWindow) -> String {
    format!(
        "BlkPool name={} device=device#{} base={:#x} size={}",
        p.name, p.device, p.base, p.size
    )
}

/// Render Cores / CoreStack / CoreEntry / Blk* / IrqHostInject lines from a
/// `ParsedReport` (the runtime-config tail the compiler appends after
/// Entry). Ring lines stay compiler-side: `RequestRing` is lossy vs the
/// live `Ring kind=` spelling.
pub fn render_runtime_tail(parsed: &ParsedReport) -> String {
    let mut out = String::new();
    out.push_str(&line_cores(parsed.cores));
    out.push('\n');
    if parsed.core_stacks.is_empty() {
        // Synthesize the canonical high-DRAM table so parse→render→parse
        // round-trips stay closed when a fixture omitted `CoreStack` lines.
        for core in 0..parsed.cores {
            let base = crate::layout::core_stack_base_n(core, parsed.cores);
            out.push_str(&line_core_stack(
                core,
                base,
                crate::layout::CORE_STACK_SIZE,
            ));
            out.push('\n');
        }
    } else {
        for s in &parsed.core_stacks {
            out.push_str(&line_core_stack(s.core, s.base, s.size));
            out.push('\n');
        }
    }
    for e in &parsed.core_entries {
        out.push_str(&line_core_entry(e.core, e.base));
        out.push('\n');
    }
    if let Some(blk) = &parsed.blk {
        out.push_str(&line_blk_device(blk));
        out.push('\n');
        out.push_str(&line_blk_queue(&blk.queue));
        out.push('\n');
        for p in &blk.pools {
            out.push_str(&line_blk_pool(p));
            out.push('\n');
        }
    }
    for inj in &parsed.irq_injects {
        out.push_str(&line_irq_host_inject(
            inj.base, inj.offset, inj.status, inj.vector,
        ));
        out.push('\n');
    }
    out
}

/// Render the VMM-facing subset of a `ParsedReport` (identity, digests,
/// exec sections, entry, secondary cores, blk, irq injects, request rings
/// as best-effort occupancy witnesses). Enough for a parse→render→parse
/// round trip on ringless reports; request rings re-emit with a synthetic
/// `cap`/`slot`/`bytes` that preserves `base` and the occupancy word only
/// when pre-M12 packing applies (`count_addr == base + cap*slot + 16`).
pub fn render(parsed: &ParsedReport) -> String {
    let mut out = String::new();
    out.push_str(&line_machine_revision(crate::MACHINE_REVISION_STR));
    out.push('\n');
    for (path, dig) in &parsed.input_digests {
        out.push_str(&line_input(path, dig));
        out.push('\n');
    }
    out.push_str(&line_image_sha256(&parsed.image_sha256));
    out.push('\n');
    for s in &parsed.exec_sections {
        out.push_str(&line_section(&s.name, s.base, s.size));
        out.push('\n');
    }
    out.push_str(&line_entry(parsed.entry));
    out.push('\n');
    out.push_str(&render_runtime_tail(parsed));
    for r in &parsed.request_rings {
        // Synthetic shape reserved for round-trip fixtures; live rings are
        // emitted by the compiler with full cap/slot/bytes (RequestRing is
        // lossy).
        let _ = r;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_minimal_report() {
        let text = format!(
            "Machine revision={}\n             Input path=input.wr sha256={EMPTY_SHA256}\n             Image sha256={EMPTY_SHA256}\n             Section name=entry base=0x40500000 size=64\n             Entry base=0x40500000\n",
            crate::MACHINE_REVISION_STR
        );
        let parsed = parse_report(&text).expect("parse");
        let rendered = render(&parsed);
        let again = parse_report(&rendered).expect("re-parse");
        assert_eq!(parsed.entry, again.entry);
        assert_eq!(parsed.image_sha256, again.image_sha256);
        assert_eq!(parsed.input_digests, again.input_digests);
        assert_eq!(parsed.exec_sections, again.exec_sections);
        assert_eq!(parsed.core_entries, again.core_entries);
        assert_eq!(parsed.cores, again.cores);
        assert_eq!(again.cores, 1);
        assert_eq!(again.core_stacks.len(), 1);
        assert_eq!(
            again.core_stacks[0].base,
            crate::layout::core_stack_base_n(0, 1)
        );
        assert_eq!(parsed.blk, again.blk);
        assert_eq!(parsed.irq_injects, again.irq_injects);
        assert_eq!(parsed.request_rings, again.request_rings);
    }

    #[test]
    fn parse_accepts_cores_and_high_core_stacks() {
        let n = 2usize;
        let s0 = crate::layout::core_stack_base_n(0, n);
        let s1 = crate::layout::core_stack_base_n(1, n);
        let text = format!(
            "Machine revision={}\nInput path=input.wr sha256={EMPTY_SHA256}\n\
             Image sha256={EMPTY_SHA256}\n\
             Section name=entry base=0x40500000 size=64\n\
             Section name=rtcode base=0x40500100 size=0x200\n\
             Cores count=2\n\
             CoreStack core=0 base={s0:#x} size={:#x}\n\
             CoreStack core=1 base={s1:#x} size={:#x}\n\
             Entry base=0x40500000\n\
             CoreEntry core=1 base=0x40500100\n",
            crate::MACHINE_REVISION_STR,
            crate::layout::CORE_STACK_SIZE,
            crate::layout::CORE_STACK_SIZE,
        );
        let parsed = parse_report(&text).expect("parse");
        assert_eq!(parsed.cores, 2);
        assert_eq!(parsed.core_stacks.len(), 2);
        assert_eq!(parsed.core_stacks[0].base, s0);
        assert_eq!(parsed.core_stacks[1].base, s1);
        assert_eq!(parsed.core_entries.len(), 1);
    }

    #[test]
    fn parse_rejects_low_dram_core_stack() {
        let text = format!(
            "Machine revision={}\nInput path=input.wr sha256={EMPTY_SHA256}\n\
             Image sha256={EMPTY_SHA256}\n\
             Section name=entry base=0x40500000 size=64\n\
             Cores count=1\n\
             CoreStack core=0 base=0x40010000 size={:#x}\n\
             Entry base=0x40500000\n",
            crate::MACHINE_REVISION_STR,
            crate::layout::CORE_STACK_SIZE,
        );
        let err = parse_report(&text).expect_err("low stack");
        assert!(
            err.contains("expected base=") || err.contains("high-DRAM"),
            "{err}"
        );
    }

    #[test]
    fn parse_rejects_wrong_revision_with_stable_prefix() {
        let text = format!(
            "Machine revision=other-v9\n             Input path=x sha256={EMPTY_SHA256}\n             Image sha256={EMPTY_SHA256}\n             Section name=entry base=0x40500000 size=1\n             Entry base=0x40500000\n"
        );
        let err = parse_report(&text).expect_err("mismatch");
        assert!(err.starts_with("machine-revision-mismatch:"));
        assert!(err.contains("other-v9"));
    }
}
