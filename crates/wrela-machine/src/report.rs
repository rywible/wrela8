use std::fmt::Write as _;

pub const RING_BOOKKEEPING_BYTES: u64 = 3 * 8;

pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolWindow {
    pub name: String,
    pub device: u64,
    pub base: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlkQueueConfig {
    pub size: u16,
    pub desc: u64,
    pub avail: u64,
    pub used: u64,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlkConfig {
    pub device: u64,
    pub capacity_sectors: u64,
    pub features: u64,
    pub vector: Option<u64>,
    pub queue: BlkQueueConfig,
    pub pools: Vec<PoolWindow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreEntry {
    pub core: usize,
    pub base: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreStack {
    pub core: usize,
    pub base: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedReport {
    pub entry: u64,
    pub image_sha256: String,
    pub input_digests: Vec<(String, String)>,
    pub exec_sections: Vec<ReportSection>,
    pub frameprog_sections: Vec<ReportSection>,
    pub renderer_placements: Vec<ReportRendererPlacement>,
    pub blk: Option<BlkConfig>,
    pub irq_injects: Vec<IrqHostInject>,
    pub core_entries: Vec<CoreEntry>,
    pub cores: usize,
    pub core_stacks: Vec<CoreStack>,
    pub request_rings: Vec<RequestRing>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportRendererPlacement {
    pub index: usize,
    pub frameprog_base: u64,
    pub frameprog_size: u64,
    pub state_base: u64,
    pub state_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRing {
    pub src: usize,
    pub dst: usize,
    pub target: String,
    pub data_base: u64,
    pub count_addr: u64,
    pub capacity: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingsMeta {
    pub count: u64,
    pub stride: u64,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportPlacement {
    pub id: String,
    pub type_name: String,
    pub core: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredRoot {
    pub id: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrqHostInject {
    pub base: u64,
    pub offset: u64,
    pub status: u32,
    pub vector: u64,
}

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

pub fn parse_report(text: &str) -> Result<ParsedReport, String> {
    let mut revision: Option<String> = None;
    let mut input_digests: Vec<(String, String)> = Vec::new();
    let mut image_sha256: Option<String> = None;
    let mut entry: Option<u64> = None;
    let mut blk_device: Option<(u64, u64, u64, Option<u64>)> = None;
    let mut blk_queue: Option<BlkQueueConfig> = None;
    let mut blk_pools: Vec<PoolWindow> = Vec::new();
    let mut irq_injects: Vec<IrqHostInject> = Vec::new();
    let mut core_entries: Vec<CoreEntry> = Vec::new();
    let mut cores_line: Option<usize> = None;
    let mut core_stacks: Vec<CoreStack> = Vec::new();
    let mut request_rings: Vec<RequestRing> = Vec::new();
    let mut sections: Vec<ReportSection> = Vec::new();
    let mut renderer_placements: Vec<ReportRendererPlacement> = Vec::new();
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
        } else if let Some(rest) = line.strip_prefix("RendererPlacement ") {
            let fields = parse_report_fields(
                "RendererPlacement",
                rest,
                &[
                    "index",
                    "frameprog_base",
                    "frameprog_bytes",
                    "state_base",
                    "state_bytes",
                    "coordinator",
                    "coordinator_core",
                ],
            )?;
            let index = usize::try_from(report_u64("RendererPlacement", &fields, "index")?)
                .map_err(|_| "`RendererPlacement index=` exceeds usize".to_string())?;
            let frameprog_base = report_u64("RendererPlacement", &fields, "frameprog_base")?;
            let frameprog_size = report_u64("RendererPlacement", &fields, "frameprog_bytes")?;
            let state_base = report_u64("RendererPlacement", &fields, "state_base")?;
            let state_size = report_u64("RendererPlacement", &fields, "state_bytes")?;
            if frameprog_size == 0 {
                return Err(format!(
                    "`RendererPlacement index={index}` has zero frameprog bytes"
                ));
            }
            renderer_placements.push(ReportRendererPlacement {
                index,
                frameprog_base,
                frameprog_size,
                state_base,
                state_size,
            });
        } else if let Some(rest) = line.strip_prefix("Entry base=") {
            let rest = rest.trim();
            if entry.is_some() {
                return Err("more than one `Entry base=` line".to_string());
            }
            let digits = rest.strip_prefix("0x").ok_or_else(|| {
                format!("`Entry base={rest}`: expected a `0x`-prefixed hex address")
            })?;
            let parsed =
                u64::from_str_radix(digits, 16).map_err(|e| format!("`Entry base={rest}`: {e}"))?;
            entry = Some(parsed);
        } else if let Some(rest) = line.strip_prefix("Cores count=") {
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
            if capacity == 0 || slot == 0 {
                return Err(format!(
                    "`Ring cap={capacity} slot={slot}`: a live ring has at least one slot of at \
                     least one byte; a zero here makes the `bytes = cap*slot+{RING_BOOKKEEPING_BYTES}` \
                     check vacuous and would admit any capacity at all"
                ));
            }
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
                        count_addr: base
                            .checked_add(expected_bytes.saturating_sub(8))
                            .ok_or_else(|| {
                                format!(
                                    "`Ring kind={kind} base={base:#x} bytes={bytes}`: the \
                                     occupancy word's address overflows a u64"
                                )
                            })?,
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
    let frameprog_sections: Vec<ReportSection> = sections
        .iter()
        .filter(|section| section.name == "frameprog")
        .cloned()
        .collect();
    renderer_placements.sort_unstable_by_key(|placement| placement.index);
    for (expected, placement) in renderer_placements.iter().enumerate() {
        if placement.index != expected {
            return Err(format!(
                "`RendererPlacement index={}` is not the dense canonical index {expected}",
                placement.index
            ));
        }
    }
    if !renderer_placements.is_empty() {
        let [section] = frameprog_sections.as_slice() else {
            return Err(format!(
                "{} renderer placement(s) require exactly one `Section name=frameprog` line",
                renderer_placements.len()
            ));
        };
        let first = renderer_placements
            .first()
            .expect("nonempty renderer placements");
        let last = renderer_placements
            .last()
            .expect("nonempty renderer placements");
        let placement_end = last
            .frameprog_base
            .checked_add(last.frameprog_size)
            .ok_or_else(|| "last renderer frameprog range overflows".to_string())?;
        if first.frameprog_base != section.base || placement_end != section.end() {
            return Err(
                "`RendererPlacement` frameprog ranges do not exactly span the canonical \
                 `frameprog` section"
                    .to_string(),
            );
        }
    } else if !frameprog_sections.is_empty() {
        return Err(
            "a `Section name=frameprog` line requires at least one `RendererPlacement` line"
                .to_string(),
        );
    }
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
        frameprog_sections,
        renderer_placements,
        blk,
        irq_injects,
        core_entries,
        cores,
        core_stacks,
        request_rings,
    })
}

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

    for (i, a) in declared_roots.iter().enumerate() {
        for b in declared_roots.iter().skip(i + 1) {
            if a.id == b.id {
                return Err(format!("declared root `{}` is repeated", a.id));
            }
        }
    }

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

pub fn blk_device_line(
    device: u64,
    capacity_sectors: u64,
    features: u64,
    vector: Option<u64>,
) -> String {
    let mut s = format!(
        "BlkDevice device=device#{device} capacity_sectors={capacity_sectors} \
         features={features:#x}"
    );
    if let Some(v) = vector {
        s.push_str(&format!(" vector={v}"));
    }
    s
}

pub fn blk_queue_line(
    index: u16,
    size: u16,
    desc: u64,
    avail: u64,
    used: u64,
    doorbell: u64,
) -> String {
    format!(
        "BlkQueue index={index} size={size} desc={desc:#x} avail={avail:#x} \
         used={used:#x} doorbell={doorbell:#x}"
    )
}

pub fn blk_pool_line(name: &str, device: u64, base: u64, size: u64) -> String {
    format!("BlkPool name={name} device=device#{device} base={base:#x} size={size:#x}")
}

pub fn line_blk_device(blk: &BlkConfig) -> String {
    blk_device_line(blk.device, blk.capacity_sectors, blk.features, blk.vector)
}

pub fn line_blk_queue(q: &BlkQueueConfig) -> String {
    blk_queue_line(0, q.size, q.desc, q.avail, q.used, q.doorbell)
}

pub fn line_blk_pool(p: &PoolWindow) -> String {
    blk_pool_line(&p.name, p.device, p.base, p.size)
}

pub fn render_runtime_tail(parsed: &ParsedReport) -> String {
    let mut out = String::new();
    out.push_str(&line_cores(parsed.cores));
    out.push('\n');
    if parsed.core_stacks.is_empty() {
        for core in 0..parsed.cores {
            let base = crate::layout::core_stack_base_n(core, parsed.cores);
            out.push_str(&line_core_stack(core, base, crate::layout::CORE_STACK_SIZE));
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
    for s in &parsed.frameprog_sections {
        out.push_str(&line_section(&s.name, s.base, s.size));
        out.push('\n');
    }
    for placement in &parsed.renderer_placements {
        let _ = writeln!(
            out,
            "RendererPlacement index={} frameprog_base={:#x} frameprog_bytes={} state_base={:#x} state_bytes={}",
            placement.index,
            placement.frameprog_base,
            placement.frameprog_size,
            placement.state_base,
            placement.state_size,
        );
    }
    out.push_str(&line_entry(parsed.entry));
    out.push('\n');
    out.push_str(&render_runtime_tail(parsed));
    for r in &parsed.request_rings {
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
        assert_eq!(parsed.frameprog_sections, again.frameprog_sections);
        assert_eq!(parsed.renderer_placements, again.renderer_placements);
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

    mod forged {
        use super::*;

        fn ring_report(cap: u64, slot: u64, bytes: u64, base: u64) -> String {
            let n = 2usize;
            format!(
                "Machine revision={}\n\
                 Input path=input.wr sha256={EMPTY_SHA256}\n\
                 Image sha256={EMPTY_SHA256}\n\
                 Section name=rtcode base=0x40500000 size=0x1000\n\
                 Section name=rtdata base={rtdata:#x} size=0x1000\n\
                 Entry base=0x40500000\n\
                 Cores count=2\n\
                 CoreEntry core=1 base=0x40500040\n\
                 CoreStack core=0 base={s0:#x} size={stack:#x}\n\
                 CoreStack core=1 base={s1:#x} size={stack:#x}\n\
                 Actor name=Root\n\
                 Ring kind=request src=1 dst=0 target=Root cap={cap} slot={slot} \
                 bytes={bytes} base={base:#x}\n",
                crate::MACHINE_REVISION_STR,
                rtdata = crate::layout::RTDATA_BASE,
                s0 = crate::layout::core_stack_base_n(0, n),
                s1 = crate::layout::core_stack_base_n(1, n),
                stack = crate::layout::CORE_STACK_SIZE,
            )
        }

        #[test]
        fn a_zero_slot_cannot_smuggle_an_unbounded_capacity() {
            let text = ring_report(u64::MAX, 0, 24, crate::layout::RTDATA_BASE);
            let err = parse_report(&text).expect_err("slot=0 must be refused");
            assert!(err.contains("at least one slot"), "got {err}");
            let text = ring_report(0, 16, 24, crate::layout::RTDATA_BASE);
            assert!(parse_report(&text).is_err(), "cap=0 must be refused");
            let text = ring_report(4, 16, 4 * 16 + 24, crate::layout::RTDATA_BASE);
            let parsed = parse_report(&text).expect("a well-formed ring still parses");
            assert_eq!(parsed.request_rings.len(), 1);
            assert_eq!(parsed.request_rings[0].capacity, 4);
        }

        #[test]
        fn a_ring_base_at_the_top_of_the_address_space_does_not_overflow() {
            let text = ring_report(4, 16, 4 * 16 + 24, u64::MAX);
            let err = parse_report(&text).expect_err("must be refused, never panic");
            assert!(
                err.contains("overflows") || err.contains("outside"),
                "got {err}"
            );
        }

        #[test]
        fn a_second_entry_line_cannot_silently_redirect_the_boot() {
            let base = format!(
                "Machine revision={}\n\
                 Input path=input.wr sha256={EMPTY_SHA256}\n\
                 Image sha256={EMPTY_SHA256}\n\
                 Section name=rtcode base=0x40500000 size=0x1000\n\
                 Entry base=0x40500000\n",
                crate::MACHINE_REVISION_STR
            );
            assert_eq!(
                parse_report(&base).expect("baseline parses").entry,
                0x4050_0000
            );
            let forged = format!("{base}Entry base=0x40500abc\n");
            let err = parse_report(&forged).expect_err("a duplicate Entry must be refused");
            assert!(err.contains("more than one `Entry base=`"), "got {err}");
        }

        #[test]
        fn entry_requires_exactly_one_0x_prefix() {
            for spelling in ["0x0x40500000", "40500000", ""] {
                let text = format!(
                    "Machine revision={}\n\
                     Input path=input.wr sha256={EMPTY_SHA256}\n\
                     Image sha256={EMPTY_SHA256}\n\
                     Section name=rtcode base=0x40500000 size=0x1000\n\
                     Entry base={spelling}\n",
                    crate::MACHINE_REVISION_STR
                );
                assert!(
                    parse_report(&text).is_err(),
                    "`Entry base={spelling}` must be refused"
                );
            }
        }
    }
}
