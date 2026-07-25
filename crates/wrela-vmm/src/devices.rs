//! Device models for the closed machine v1 set (06-machine.md §6).
//!
//! At M5 there were exactly two "device models", both living directly in
//! `boot_image_core`'s own exit loop: the console tx-ring drain
//! (`crate::drain_console`) and the clock MMIO trap. This module is
//! plans/M7.md item F — the first device model big enough to be its own
//! file: **virtio-blk**, the split ring, the request format, `Flush`, and
//! feature negotiation, per 06 §6's own table row (`blk` — "virtio-blk,
//! split ring, `Flush`, per-queue reset") and 03-hardware.md §4's own
//! queue rules. Still one file, not a directory and not a `trait Device`:
//! there is one model here (CLAUDE.md's "no traits with one
//! implementation"); a second device model splits this into a directory,
//! and not before.
//!
//! ## What this machine deletes, and what is therefore *not* here
//!
//! plans/M7.md decision 2 is explicit that most of virtio is already gone
//! from this machine, and this model implements exactly the remainder:
//!
//! - **No MMIO transport, no discovery** (06 §3: "the VMM ... preconfigures
//!   every device, queue, and shared-memory window the report declares —
//!   device topology is a *build output*, not a probed fact"). There is no
//!   `MagicValue`/`DeviceID`/`QueueSel`/`QueueReady` register file here at
//!   all: `BlkConfig` *is* the transport configuration, parsed out of the
//!   image report by `crate::parse_report`. A driver never probes.
//! - **No trapping notification** (06 §5: "guest→host notification is a
//!   shared-memory doorbell word per queue plus one host-visible wake").
//!   `BlkQueueConfig::doorbell` is an ordinary guest-writable DRAM word;
//!   the guest's store to it does not exit. `crate::boot_image_core`
//!   polls it (see `service`) at every vCPU exit and, crucially, on the
//!   park path *before* the sleep decision — the mask–arm–recheck
//!   discipline 06 §4 already requires for vectors, applied to
//!   completions, so a doorbell rung immediately before a park can never
//!   be lost.
//! - **No interrupt controller** (06 §4). A completion optionally raises a
//!   vector by setting one bit in this core's own pending word
//!   (`BlkConfig::vector`); the guest observes it at its next checkpoint
//!   or park. A device declared with no vector is 03 §7's poll build: the
//!   used ring alone is the completion signal.
//!
//! What remains — and is here in full — is the split ring (descriptor
//! table, available ring, used ring), the virtio-blk request format
//! (type/sector header, data descriptors, status byte), `Flush`, and
//! feature negotiation.
//!
//! ## Validation is the device's job (03 §4, plans/M7.md decision 5)
//!
//! 03 §4: "stale, duplicate, or unknown IDs are driver faults, never
//! unchecked indexes." That rule binds this model at least as hard as it
//! binds the driver, because this side of the ring is the one holding a
//! raw pointer into guest DRAM. Two mechanisms, both structural:
//!
//! 1. **`GuestMem` is the only way to touch guest memory in this file**,
//!    and every one of its accessors takes the same `window_offset` path,
//!    which admits an address only if `[addr, addr+len)` lies entirely
//!    inside one *declared pool window*. Decision 5 ("the VMM maps exactly
//!    the declared pools and nothing else ... it is a *security* property
//!    on the flagship") is therefore enforced by construction rather than
//!    by review: there is no unchecked indexing operation in this module
//!    to audit, so a descriptor whose `addr` names the image's own code,
//!    another actor's state, or an address off the end of DRAM entirely
//!    fails as `BlkFault::OutsidePool` — a diagnosable VMM-side error,
//!    never a panic and never an out-of-bounds read.
//! 2. **Every ring-shaped input is range-checked before it is used as an
//!    index**: `BlkFault` below enumerates each rejection by name.
//!
//! A `BlkFault` is a *driver* fault, so it fails the boot closed with a
//! named diagnostic (`crate::VmmError::GuestFault`); it is never silently
//! skipped and never approximated. It is deliberately distinct from an
//! in-protocol *device error*, which is a real completion carrying
//! `STATUS_IOERR`/`STATUS_UNSUPP` — an out-of-range sector or an unknown
//! request type is something the protocol itself has an answer for, and
//! answering it is not approximating.

use crate::record::digest_hex;
use wrela_machine::layout as machine_layout;

// --- virtio-blk protocol constants (OASIS VIRTIO 1.2, as profiled by 06) ---

/// Descriptor table entry: `addr: u64, len: u32, flags: u16, next: u16`.
pub const DESC_SIZE: u64 = 16;

/// `VIRTQ_DESC_F_NEXT` — this descriptor continues into `next`.
pub const DESC_F_NEXT: u16 = 1;
/// `VIRTQ_DESC_F_WRITE` — device-writable ("write" is from the device's
/// point of view); its absence means device-readable.
pub const DESC_F_WRITE: u16 = 2;
/// `VIRTQ_DESC_F_INDIRECT` — an indirect descriptor table. The
/// corresponding feature is never offered by this device (see
/// `DEVICE_FEATURES`), so a descriptor carrying this flag is a driver
/// fault, not a chain to follow.
pub const DESC_F_INDIRECT: u16 = 4;

/// `VIRTIO_BLK_F_FLUSH` (feature bit 9) — the `Flush` request type 06 §6's
/// own device table names explicitly.
pub const F_BLK_FLUSH: u64 = 1 << 9;
/// `VIRTIO_F_VERSION_1` (feature bit 32). This machine is modern-only:
/// there is no legacy transport to fall back to (there is no transport at
/// all — module doc above), so a driver that does not accept this bit is
/// refused at negotiation rather than quietly served.
pub const F_VERSION_1: u64 = 1 << 32;

/// Everything this model offers. A driver may accept a subset (subject to
/// `F_VERSION_1` being mandatory); it may never accept a bit outside it.
pub const DEVICE_FEATURES: u64 = F_VERSION_1 | F_BLK_FLUSH;

/// `VIRTIO_BLK_T_IN` — read from the device into guest memory.
pub const T_IN: u32 = 0;
/// `VIRTIO_BLK_T_OUT` — write guest memory to the device.
pub const T_OUT: u32 = 1;
/// `VIRTIO_BLK_T_FLUSH`.
pub const T_FLUSH: u32 = 4;

pub const STATUS_OK: u8 = 0;
pub const STATUS_IOERR: u8 = 1;
pub const STATUS_UNSUPP: u8 = 2;

/// The one sector size this machine's `blk` device speaks.
pub const SECTOR_SIZE: u64 = 512;

/// `struct virtio_blk_req`'s own header: `type: u32, reserved: u32,
/// sector: u64`.
pub const REQ_HEADER_SIZE: u64 = 16;

/// The largest in-memory disk this VMM will allocate for a declared `blk`
/// device. The flagship guest profile is 1 GiB of DRAM (06 §2) and the
/// disk is a plain host-side `Vec<u8>` this VMM owns, so a report
/// declaring `capacity_sectors=2^40` must fail closed rather than attempt
/// a 512 TiB allocation. 64 MiB is far past anything M7 needs and small
/// enough that the failure is unmistakable.
pub const MAX_DISK_BYTES: u64 = 64 << 20;

// --- declared pool windows and checked guest memory -------------------------

/// One declared, device-reachable window of guest DRAM (05-library.md §9's
/// `img.pool`/`img.dma_pool`, as it reaches this VMM through the report).
/// plans/M7.md decision 5: "the VMM maps exactly the declared pools and
/// nothing else" — on the flagship there is no IOMMU, so this list *is*
/// the mapping, and `GuestMem` below is what enforces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolWindow {
    pub name: String,
    pub base: u64,
    pub size: u64,
}

/// Guest DRAM, reachable only through the declared pool windows.
///
/// Holds a raw pointer into `boot_image_core`'s own `alloc_zeroed` DRAM
/// reservation (exactly like `crate::drain_console`, and for the same
/// reason: this VMM never forms a `&mut [u8]` over the whole guest
/// reservation, so it never has to reason about aliasing it with the
/// vCPU's own view). Every accessor below goes through `window_offset`,
/// which is the single enforcement point for decision 5.
pub struct GuestMem {
    base: *mut u8,
    windows: Vec<PoolWindow>,
}

impl GuestMem {
    /// `base` must point at a `machine_layout::DRAM_SIZE`-byte host
    /// allocation mapped at `machine_layout::DRAM_BASE`. Every window is
    /// validated here, once: non-empty, wholly inside guest DRAM, and
    /// disjoint from every other window (a duplicated or overlapping pool
    /// declaration is a configuration bug, and a silent overlap would
    /// quietly widen exactly the boundary this type exists to keep
    /// narrow).
    ///
    /// # Safety
    /// `base` must be valid for reads and writes of `DRAM_SIZE` bytes for
    /// as long as this `GuestMem` lives.
    pub unsafe fn new(base: *mut u8, windows: Vec<PoolWindow>) -> Result<GuestMem, String> {
        for (i, w) in windows.iter().enumerate() {
            if w.size == 0 {
                return Err(format!("pool `{}` declares a zero-byte window", w.name));
            }
            let end = w
                .base
                .checked_add(w.size)
                .ok_or_else(|| format!("pool `{}` window overflows a u64", w.name))?;
            if w.base < machine_layout::DRAM_BASE
                || end > machine_layout::DRAM_BASE + machine_layout::DRAM_SIZE
            {
                return Err(format!(
                    "pool `{}` window [{:#x}, {end:#x}) is not inside guest DRAM [{:#x}, {:#x})",
                    w.name,
                    w.base,
                    machine_layout::DRAM_BASE,
                    machine_layout::DRAM_BASE + machine_layout::DRAM_SIZE
                ));
            }
            for other in &windows[..i] {
                let other_end = other.base + other.size;
                if w.base < other_end && other.base < end {
                    return Err(format!(
                        "pools `{}` and `{}` declare overlapping windows",
                        other.name, w.name
                    ));
                }
            }
        }
        Ok(GuestMem { base, windows })
    }

    /// THE enforcement point (module doc above): the host-side byte offset
    /// of `[addr, addr+len)`, or `BlkFault::OutsidePool` if that range is
    /// not wholly inside one declared pool window. Nothing in this file
    /// converts a guest address to a host offset any other way.
    fn window_offset(&self, addr: u64, len: u64) -> Result<usize, BlkFault> {
        let end = addr.checked_add(len).ok_or(BlkFault::OutsidePool {
            addr,
            len,
            why: "address + length overflows a u64",
        })?;
        for w in &self.windows {
            if addr >= w.base && end <= w.base + w.size {
                return Ok((addr - machine_layout::DRAM_BASE) as usize);
            }
        }
        Err(BlkFault::OutsidePool {
            addr,
            len,
            why: "no declared pool window contains this range",
        })
    }

    fn read(&self, addr: u64, buf: &mut [u8]) -> Result<(), BlkFault> {
        let off = self.window_offset(addr, buf.len() as u64)?;
        unsafe {
            std::ptr::copy_nonoverlapping(self.base.add(off), buf.as_mut_ptr(), buf.len());
        }
        Ok(())
    }

    fn write(&mut self, addr: u64, bytes: &[u8]) -> Result<(), BlkFault> {
        let off = self.window_offset(addr, bytes.len() as u64)?;
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.base.add(off), bytes.len());
        }
        Ok(())
    }

    fn read_u16(&self, addr: u64) -> Result<u16, BlkFault> {
        let mut b = [0u8; 2];
        self.read(addr, &mut b)?;
        Ok(u16::from_le_bytes(b))
    }

    fn read_u32(&self, addr: u64) -> Result<u32, BlkFault> {
        let mut b = [0u8; 4];
        self.read(addr, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    fn read_u64(&self, addr: u64) -> Result<u64, BlkFault> {
        let mut b = [0u8; 8];
        self.read(addr, &mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    fn write_u16(&mut self, addr: u64, v: u16) -> Result<(), BlkFault> {
        self.write(addr, &v.to_le_bytes())
    }

    fn write_u32(&mut self, addr: u64, v: u32) -> Result<(), BlkFault> {
        self.write(addr, &v.to_le_bytes())
    }

    fn write_u64(&mut self, addr: u64, v: u64) -> Result<(), BlkFault> {
        self.write(addr, &v.to_le_bytes())
    }
}

/// `true` if `[addr, addr+len)` lies wholly inside one of `windows` — the
/// same containment rule `GuestMem::window_offset` enforces at access
/// time, reused by `BlkDevice::new` to reject a *configuration* whose ring
/// or doorbell does not live in a declared pool before any boot happens.
fn window_contains(windows: &[PoolWindow], addr: u64, len: u64) -> bool {
    match addr.checked_add(len) {
        None => false,
        Some(end) => windows
            .iter()
            .any(|w| addr >= w.base && end <= w.base + w.size),
    }
}

// --- faults -----------------------------------------------------------------

/// Every way this model refuses a ring (03 §4: "stale, duplicate, or
/// unknown IDs are driver faults, never unchecked indexes"). Each is named
/// exactly rather than collapsed into one opaque "bad ring" string, so a
/// post-mortem says which rule the driver broke. None of these is ever a
/// panic, an approximation, or an out-of-bounds access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlkFault {
    /// A guest address (a descriptor's `addr`, a ring word, the doorbell)
    /// whose range is not wholly inside a declared pool window — decision
    /// 5's security boundary, refused.
    OutsidePool {
        addr: u64,
        len: u64,
        why: &'static str,
    },
    /// `avail.idx` advanced by more than the queue is deep: the driver
    /// published more entries than can exist, or the index is stale/
    /// garbage.
    AvailIndexJump {
        last: u16,
        now: u16,
        queue_size: u16,
    },
    /// An `avail.ring[]` entry, or a chain's `next`, naming a descriptor
    /// slot that does not exist — 03 §4's "unknown ID", refused before it
    /// is ever used as an index.
    DescriptorIndexOutOfRange { index: u16, queue_size: u16 },
    /// A chain that revisits a descriptor it already used — 03 §4's
    /// "duplicate", and the shape a malicious ring would take to make a
    /// naive walker loop forever.
    DescriptorChainLoop { index: u16 },
    /// A chain longer than the queue is deep (the same loop, caught by
    /// length even where the visited set would not).
    DescriptorChainTooLong { queue_size: u16 },
    /// `VIRTQ_DESC_F_INDIRECT` on a device that never offered
    /// `VIRTIO_F_INDIRECT_DESC`.
    IndirectNotNegotiated { index: u16 },
    /// Fewer than the two descriptors (header + status) every virtio-blk
    /// request needs.
    ChainTooShort { len: usize },
    /// The head descriptor is not a `REQ_HEADER_SIZE`-byte device-readable
    /// buffer.
    BadRequestHeader { len: u32, device_writable: bool },
    /// The last descriptor is not a device-writable status byte.
    BadStatusDescriptor { len: u32, device_writable: bool },
    /// A data descriptor pointing the wrong way for its request type (a
    /// read into a device-readable buffer, or a write out of a
    /// device-writable one).
    DataDirectionMismatch {
        request_type: u32,
        device_writable: bool,
    },
    /// Data length that is not a whole number of sectors.
    UnalignedDataLength { len: u64 },
    /// A `Flush` carrying data descriptors (`struct virtio_blk_req` has
    /// none for `T_FLUSH`).
    FlushWithData { len: u64 },
}

impl std::fmt::Display for BlkFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlkFault::OutsidePool { addr, len, why } => write!(
                f,
                "guest range [{addr:#x}, +{len}) is not device-reachable: {why} \
                 (plans/M7.md decision 5: the device model may touch declared pool pages only)"
            ),
            BlkFault::AvailIndexJump {
                last,
                now,
                queue_size,
            } => write!(
                f,
                "avail.idx jumped from {last} to {now}, more than the queue depth {queue_size}"
            ),
            BlkFault::DescriptorIndexOutOfRange { index, queue_size } => write!(
                f,
                "descriptor index {index} is outside a {queue_size}-deep queue (03 §4: an unknown ID is a driver fault, never an unchecked index)"
            ),
            BlkFault::DescriptorChainLoop { index } => write!(
                f,
                "descriptor chain revisits descriptor {index} (03 §4: a duplicate ID is a driver fault)"
            ),
            BlkFault::DescriptorChainTooLong { queue_size } => write!(
                f,
                "descriptor chain is longer than the {queue_size}-deep queue"
            ),
            BlkFault::IndirectNotNegotiated { index } => write!(
                f,
                "descriptor {index} sets VIRTQ_DESC_F_INDIRECT, which this device never offered"
            ),
            BlkFault::ChainTooShort { len } => write!(
                f,
                "descriptor chain has {len} descriptor(s); a virtio-blk request needs at least a header and a status byte"
            ),
            BlkFault::BadRequestHeader {
                len,
                device_writable,
            } => write!(
                f,
                "request header descriptor is {len} byte(s) and {} — expected exactly {REQ_HEADER_SIZE} device-readable bytes",
                if *device_writable {
                    "device-writable"
                } else {
                    "device-readable"
                }
            ),
            BlkFault::BadStatusDescriptor {
                len,
                device_writable,
            } => write!(
                f,
                "status descriptor is {len} byte(s) and {} — expected at least one device-writable byte",
                if *device_writable {
                    "device-writable"
                } else {
                    "device-readable"
                }
            ),
            BlkFault::DataDirectionMismatch {
                request_type,
                device_writable,
            } => write!(
                f,
                "request type {request_type} has a {} data descriptor pointing the wrong way",
                if *device_writable {
                    "device-writable"
                } else {
                    "device-readable"
                }
            ),
            BlkFault::UnalignedDataLength { len } => write!(
                f,
                "data length {len} is not a whole number of {SECTOR_SIZE}-byte sectors"
            ),
            BlkFault::FlushWithData { len } => {
                write!(f, "a Flush request carries {len} byte(s) of data")
            }
        }
    }
}

// --- configuration ----------------------------------------------------------

/// One split ring's own addresses, exactly as the report declares them
/// (06 §3: "preconfigures every device, queue, and shared-memory window
/// the report declares"). Deliberately three independent addresses rather
/// than one base plus the legacy contiguous-with-padding layout: there is
/// no transport here to negotiate a layout with, so the declaration says
/// where each part is and nothing is implied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlkQueueConfig {
    pub size: u16,
    pub desc: u64,
    pub avail: u64,
    pub used: u64,
    /// 06 §5's shared-memory doorbell word (8 bytes). The guest stores a
    /// nonzero value here after publishing; no trap, no exit.
    pub doorbell: u64,
}

impl BlkQueueConfig {
    fn desc_bytes(&self) -> u64 {
        self.size as u64 * DESC_SIZE
    }
    /// `flags: u16, idx: u16, ring: [u16; size]`.
    fn avail_bytes(&self) -> u64 {
        4 + 2 * self.size as u64
    }
    /// `flags: u16, idx: u16, ring: [(id: u32, len: u32); size]`.
    fn used_bytes(&self) -> u64 {
        4 + 8 * self.size as u64
    }
}

/// The whole configuration of one declared `blk` device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlkConfig {
    pub capacity_sectors: u64,
    /// The feature bits the image declares (03 §9: "The image declares
    /// required features ...; boot still negotiates the real device") —
    /// run through `negotiate` at construction, so a build asking for a
    /// bit this model does not implement fails the boot closed instead of
    /// silently running without it.
    pub features: u64,
    /// The vector bit a completion raises in core 0's own pending word
    /// (06 §4). `None` is 03 §7's poll build: no vector exists, and the
    /// used ring alone is the completion signal.
    pub vector: Option<u64>,
    pub queue: BlkQueueConfig,
    pub pools: Vec<PoolWindow>,
}

/// 03 §9's negotiation, device side: the driver's requested set against
/// what this model offers. Returns the accepted set, or names exactly
/// which bits were refused and why — never a silent intersection (a
/// driver that believes it has `Flush` and does not is precisely the bug
/// this refusal exists to prevent).
pub fn negotiate(requested: u64) -> Result<u64, String> {
    let unknown = requested & !DEVICE_FEATURES;
    if unknown != 0 {
        return Err(format!(
            "the image requires virtio-blk feature bits {unknown:#x}, which this device model does not offer (offered: {DEVICE_FEATURES:#x})"
        ));
    }
    if requested & F_VERSION_1 == 0 {
        return Err(format!(
            "the image does not accept VIRTIO_F_VERSION_1 ({F_VERSION_1:#x}); this machine has no legacy virtio transport to fall back to (06-machine.md §3)"
        ));
    }
    Ok(requested)
}

// --- the model --------------------------------------------------------------

/// One completed operation, as the device reports it (and as the recorder
/// logs it — plans/M7.md decision 7: "device completions join the choice
/// sequence").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The chain's own head descriptor index — virtio's operation ID, and
    /// the `id` the used ring reports.
    pub head: u16,
    /// The virtio-blk status byte written into the guest's status
    /// descriptor.
    pub status: u8,
    /// The used ring's own `len`: bytes this operation wrote into
    /// device-writable guest buffers (payload for a read, plus the status
    /// byte always).
    pub len: u32,
    /// A digest (`record::digest_hex`, FNV-1a) over **every byte this
    /// operation wrote**, in write order: guest-memory writes first (read
    /// payload, then the status byte), then disk writes (a write
    /// request's own payload). 06 §8 asks the recorder for "every device
    /// completion and DMA-written byte range" plus "digests of every
    /// output (block writes, ...)"; one digest per completion covers both
    /// halves without putting payload bytes in the log.
    pub digest: String,
}

/// The virtio-blk device model. Owns its disk (an in-memory `Vec<u8>` —
/// there is no host file behind it, so 06 §8's "replay ... suppresses real
/// outputs" is satisfied by construction: a block write has no output to
/// suppress) and the ring's own device-side state.
pub struct BlkDevice {
    pub config: BlkConfig,
    /// Accepted feature bits (`negotiate`'s own result).
    pub negotiated: u64,
    disk: Vec<u8>,
    /// The `avail.idx` value this model has already consumed up to
    /// (virtio's `last_avail_idx`), wrapping like the guest's own.
    last_avail_idx: u16,
    /// This model's own `used.idx`.
    used_idx: u16,
}

impl BlkDevice {
    /// Validates the whole declared configuration up front (06 §3's
    /// "preconfigures" step) and allocates the disk. Every failure here is
    /// a build/report bug, reported as a plain string the caller turns
    /// into `VmmError::BadImage`; none of them is reachable from guest
    /// code.
    pub fn new(config: BlkConfig) -> Result<BlkDevice, String> {
        let negotiated = negotiate(config.features)?;
        let q = &config.queue;
        if q.size == 0 || !q.size.is_power_of_two() {
            return Err(format!(
                "blk queue size {} must be a nonzero power of two (VIRTIO 1.2 §2.6)",
                q.size
            ));
        }
        let disk_bytes = config
            .capacity_sectors
            .checked_mul(SECTOR_SIZE)
            .ok_or_else(|| {
                format!(
                    "blk capacity {} sector(s) overflows a u64 byte count",
                    config.capacity_sectors
                )
            })?;
        if disk_bytes > MAX_DISK_BYTES {
            return Err(format!(
                "blk capacity {} sector(s) ({disk_bytes} bytes) exceeds this VMM's own {MAX_DISK_BYTES}-byte in-memory disk ceiling",
                config.capacity_sectors
            ));
        }
        for (what, addr, len) in [
            ("descriptor table", q.desc, q.desc_bytes()),
            ("available ring", q.avail, q.avail_bytes()),
            ("used ring", q.used, q.used_bytes()),
            ("doorbell word", q.doorbell, 8),
        ] {
            if !window_contains(&config.pools, addr, len) {
                return Err(format!(
                    "the blk {what} at [{addr:#x}, +{len}) is not inside any declared pool window \
                     (plans/M7.md decision 5: shared control memory lives in a declared pool, and \
                     the model may reach nothing else)"
                ));
            }
        }
        Ok(BlkDevice {
            config,
            negotiated,
            disk: vec![0u8; disk_bytes as usize],
            last_avail_idx: 0,
            used_idx: 0,
        })
    }

    /// Test/inspection seam: replaces the zero-filled disk with known
    /// content. Never used on a boot path (a declared device's disk starts
    /// zeroed; there is no report syntax for preloading one at M7).
    pub fn set_disk(&mut self, bytes: Vec<u8>) {
        self.disk = bytes;
    }

    pub fn disk(&self) -> &[u8] {
        &self.disk
    }

    /// 06 §5's doorbell poll. Reads the doorbell word; if it is zero,
    /// nothing was published and this returns an empty list without
    /// touching the ring at all. Otherwise the doorbell is cleared and
    /// every newly available chain is executed, in `avail.ring` order.
    ///
    /// **What this does and does not do**: it performs the operation's own
    /// DMA (read payload into guest memory, status byte, disk writes) —
    /// the deterministic half, a pure function of the ring contents and
    /// the disk — and returns the resulting `Completion`s *without*
    /// publishing them in the used ring. Publication is `commit_used`,
    /// called separately by `boot_image_core` once each completion has
    /// been through the recorder's own `Chooser` (plans/M7.md decision 7:
    /// under replay the *used ring* is fed from the log, not from this
    /// model, and any disagreement between the two is a named divergence
    /// rather than a silently different answer).
    pub fn service(&mut self, mem: &mut GuestMem) -> Result<Vec<Completion>, BlkFault> {
        let q = self.config.queue.clone();
        if mem.read_u64(q.doorbell)? == 0 {
            return Ok(Vec::new());
        }
        mem.write_u64(q.doorbell, 0)?;

        let avail_idx = mem.read_u16(q.avail + 2)?;
        let pending = avail_idx.wrapping_sub(self.last_avail_idx);
        if pending > q.size {
            return Err(BlkFault::AvailIndexJump {
                last: self.last_avail_idx,
                now: avail_idx,
                queue_size: q.size,
            });
        }
        let mut out = Vec::new();
        for _ in 0..pending {
            let slot = self.last_avail_idx % q.size;
            let head = mem.read_u16(q.avail + 4 + 2 * slot as u64)?;
            if head >= q.size {
                return Err(BlkFault::DescriptorIndexOutOfRange {
                    index: head,
                    queue_size: q.size,
                });
            }
            let chain = self.walk_chain(mem, head)?;
            let completion = self.execute(mem, head, &chain)?;
            self.last_avail_idx = self.last_avail_idx.wrapping_add(1);
            out.push(completion);
        }
        Ok(out)
    }

    /// Publishes one completion in the used ring and bumps `used.idx`
    /// (release-ordered by construction: the entry's own bytes are written
    /// before the index that makes them visible — 03 §3's "payload writes
    /// before publication").
    pub fn commit_used(&mut self, mem: &mut GuestMem, head: u16, len: u32) -> Result<(), BlkFault> {
        let q = self.config.queue.clone();
        if head >= q.size {
            return Err(BlkFault::DescriptorIndexOutOfRange {
                index: head,
                queue_size: q.size,
            });
        }
        let slot = self.used_idx % q.size;
        let entry = q.used + 4 + 8 * slot as u64;
        mem.write_u32(entry, head as u32)?;
        mem.write_u32(entry + 4, len)?;
        self.used_idx = self.used_idx.wrapping_add(1);
        mem.write_u16(q.used + 2, self.used_idx)?;
        Ok(())
    }

    /// One descriptor, already read and range-checked.
    fn read_desc(&self, mem: &GuestMem, index: u16) -> Result<Descriptor, BlkFault> {
        let q = &self.config.queue;
        if index >= q.size {
            return Err(BlkFault::DescriptorIndexOutOfRange {
                index,
                queue_size: q.size,
            });
        }
        let at = q.desc + index as u64 * DESC_SIZE;
        let addr = mem.read_u64(at)?;
        let len = mem.read_u32(at + 8)?;
        let flags = mem.read_u16(at + 12)?;
        let next = mem.read_u16(at + 14)?;
        if flags & DESC_F_INDIRECT != 0 {
            return Err(BlkFault::IndirectNotNegotiated { index });
        }
        // The buffer itself must be device-reachable *before* anything
        // reads or writes a byte of it — the check is here, at the one
        // place a descriptor becomes usable, as well as inside every
        // `GuestMem` accessor.
        mem.window_offset(addr, len as u64)?;
        Ok(Descriptor {
            index,
            addr,
            len,
            flags,
            next,
        })
    }

    /// Walks a chain from `head`, bounded by the queue depth and checked
    /// for revisits — 03 §4's stale/duplicate/unknown trio, all three
    /// refused before any byte is touched.
    fn walk_chain(&self, mem: &GuestMem, head: u16) -> Result<Vec<Descriptor>, BlkFault> {
        let q = &self.config.queue;
        let mut chain = Vec::new();
        let mut seen = vec![false; q.size as usize];
        let mut index = head;
        loop {
            if chain.len() >= q.size as usize {
                return Err(BlkFault::DescriptorChainTooLong { queue_size: q.size });
            }
            if index < q.size && seen[index as usize] {
                return Err(BlkFault::DescriptorChainLoop { index });
            }
            let d = self.read_desc(mem, index)?;
            seen[index as usize] = true;
            let more = d.flags & DESC_F_NEXT != 0;
            let next = d.next;
            chain.push(d);
            if !more {
                return Ok(chain);
            }
            index = next;
        }
    }

    /// Executes one already-validated chain as a virtio-blk request.
    fn execute(
        &mut self,
        mem: &mut GuestMem,
        head: u16,
        chain: &[Descriptor],
    ) -> Result<Completion, BlkFault> {
        if chain.len() < 2 {
            return Err(BlkFault::ChainTooShort { len: chain.len() });
        }
        let header = &chain[0];
        if header.len as u64 != REQ_HEADER_SIZE || header.is_device_writable() {
            return Err(BlkFault::BadRequestHeader {
                len: header.len,
                device_writable: header.is_device_writable(),
            });
        }
        let status_desc = &chain[chain.len() - 1];
        if status_desc.len < 1 || !status_desc.is_device_writable() {
            return Err(BlkFault::BadStatusDescriptor {
                len: status_desc.len,
                device_writable: status_desc.is_device_writable(),
            });
        }
        let data = &chain[1..chain.len() - 1];

        let request_type = mem.read_u32(header.addr)?;
        let sector = mem.read_u64(header.addr + 8)?;
        let data_len: u64 = data.iter().map(|d| d.len as u64).sum();

        for d in data {
            let want_writable = request_type == T_IN;
            if d.is_device_writable() != want_writable {
                return Err(BlkFault::DataDirectionMismatch {
                    request_type,
                    device_writable: d.is_device_writable(),
                });
            }
        }
        if request_type == T_FLUSH && data_len != 0 {
            return Err(BlkFault::FlushWithData { len: data_len });
        }
        if (request_type == T_IN || request_type == T_OUT) && data_len % SECTOR_SIZE != 0 {
            return Err(BlkFault::UnalignedDataLength { len: data_len });
        }

        // `written` accumulates exactly the bytes this operation writes,
        // in write order, for `Completion::digest` (its own doc has the
        // rule).
        let mut written: Vec<u8> = Vec::new();
        let mut payload_written: u32 = 0;
        let status = match request_type {
            T_IN | T_OUT => {
                let start = sector.checked_mul(SECTOR_SIZE);
                let end = start.and_then(|s| s.checked_add(data_len));
                match end {
                    Some(end) if end <= self.disk.len() as u64 => {
                        let start = start.expect("checked above");
                        let mut cursor = start;
                        for d in data {
                            let lo = cursor as usize;
                            let hi = lo + d.len as usize;
                            if request_type == T_IN {
                                let bytes = self.disk[lo..hi].to_vec();
                                mem.write(d.addr, &bytes)?;
                                written.extend_from_slice(&bytes);
                                payload_written += d.len;
                            } else {
                                let mut bytes = vec![0u8; d.len as usize];
                                mem.read(d.addr, &mut bytes)?;
                                self.disk[lo..hi].copy_from_slice(&bytes);
                            }
                            cursor += d.len as u64;
                        }
                        STATUS_OK
                    }
                    // In range for the protocol, out of range for the
                    // disk: a real device error with a real answer, not a
                    // driver fault (module doc's own distinction).
                    _ => STATUS_IOERR,
                }
            }
            // `Flush` (06 §6's own device row). The disk is an in-memory
            // `Vec<u8>` this VMM owns, so every prior write is already
            // durable to exactly the extent anything here is durable:
            // flush is an ordered no-op that completes OK, never a
            // silently ignored request.
            T_FLUSH if self.negotiated & F_BLK_FLUSH != 0 => STATUS_OK,
            // The driver sent `Flush` without the feature negotiated, or
            // a request type this device does not implement. The protocol
            // has its own answer for both.
            _ => STATUS_UNSUPP,
        };
        if request_type == T_OUT && status == STATUS_OK {
            // The disk write is this operation's own *output* (06 §8:
            // "digests of every output (block writes, ...)").
            let mut cursor = sector * SECTOR_SIZE;
            for d in data {
                let lo = cursor as usize;
                written.extend_from_slice(&self.disk[lo..lo + d.len as usize]);
                cursor += d.len as u64;
            }
        }
        mem.write(status_desc.addr, &[status])?;
        written.push(status);
        Ok(Completion {
            head,
            status,
            len: payload_written + 1,
            digest: digest_hex(&written),
        })
    }
}

/// Hand-written rather than derived: a derived `Debug` would print the
/// whole disk (up to `MAX_DISK_BYTES`) into any diagnostic that formats a
/// device, which is exactly the kind of unreadable output that gets a
/// diagnostic ignored.
impl std::fmt::Debug for BlkDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlkDevice")
            .field("config", &self.config)
            .field("negotiated", &format_args!("{:#x}", self.negotiated))
            .field("disk_bytes", &self.disk.len())
            .field("last_avail_idx", &self.last_avail_idx)
            .field("used_idx", &self.used_idx)
            .finish()
    }
}

/// Likewise hand-written: the raw DRAM pointer is host bookkeeping nobody
/// reading a diagnostic can act on; the declared windows are the fact that
/// matters.
impl std::fmt::Debug for GuestMem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestMem")
            .field("windows", &self.windows)
            .finish_non_exhaustive()
    }
}

/// One descriptor-table entry, already range-checked by `read_desc`.
#[derive(Debug, Clone, Copy)]
struct Descriptor {
    #[allow(dead_code)]
    index: u16,
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

impl Descriptor {
    fn is_device_writable(&self) -> bool {
        self.flags & DESC_F_WRITE != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A guest-memory stand-in: a plain heap buffer the size of the whole
    /// DRAM reservation's *used* prefix, addressed exactly as guest DRAM
    /// is (`crate::tests::drain_console_reads_more_than_the_old_16_
    /// descriptor_limit`'s own established technique — `GuestMem` only
    /// ever does pointer-offset reads/writes, so no real mapping is
    /// needed, and none of these tests needs HVF at all).
    struct Harness {
        ram: Vec<u8>,
        cfg: BlkConfig,
    }

    /// Everything this harness places sits in one declared pool window
    /// starting at `POOL_BASE`; addresses outside it are exactly what the
    /// out-of-pool tests use.
    const POOL_BASE: u64 = machine_layout::DRAM_BASE + 0x10_0000;
    const POOL_SIZE: u64 = 0x10_0000;
    const QUEUE_SIZE: u16 = 8;

    const DESC_ADDR: u64 = POOL_BASE;
    const AVAIL_ADDR: u64 = POOL_BASE + 0x400;
    const USED_ADDR: u64 = POOL_BASE + 0x800;
    const DOORBELL_ADDR: u64 = POOL_BASE + 0xC00;
    const HEADER_ADDR: u64 = POOL_BASE + 0x1000;
    const DATA_ADDR: u64 = POOL_BASE + 0x2000;
    const STATUS_ADDR: u64 = POOL_BASE + 0x4000;

    impl Harness {
        fn new() -> Harness {
            let cfg = BlkConfig {
                capacity_sectors: 16,
                features: DEVICE_FEATURES,
                vector: None,
                queue: BlkQueueConfig {
                    size: QUEUE_SIZE,
                    desc: DESC_ADDR,
                    avail: AVAIL_ADDR,
                    used: USED_ADDR,
                    doorbell: DOORBELL_ADDR,
                },
                pools: vec![PoolWindow {
                    name: "BlockControl".to_string(),
                    base: POOL_BASE,
                    size: POOL_SIZE,
                }],
            };
            Harness {
                ram: vec![0u8; (POOL_BASE + POOL_SIZE - machine_layout::DRAM_BASE) as usize],
                cfg,
            }
        }

        fn mem(&mut self) -> GuestMem {
            unsafe { GuestMem::new(self.ram.as_mut_ptr(), self.cfg.pools.clone()) }
                .expect("windows")
        }

        fn device(&self) -> BlkDevice {
            BlkDevice::new(self.cfg.clone()).expect("a well-formed configuration")
        }

        fn off(&self, addr: u64) -> usize {
            (addr - machine_layout::DRAM_BASE) as usize
        }

        fn put(&mut self, addr: u64, bytes: &[u8]) {
            let off = self.off(addr);
            self.ram[off..off + bytes.len()].copy_from_slice(bytes);
        }

        fn get(&self, addr: u64, len: usize) -> Vec<u8> {
            let off = self.off(addr);
            self.ram[off..off + len].to_vec()
        }

        fn desc(&mut self, index: u16, addr: u64, len: u32, flags: u16, next: u16) {
            let at = DESC_ADDR + index as u64 * DESC_SIZE;
            self.put(at, &addr.to_le_bytes());
            self.put(at + 8, &len.to_le_bytes());
            self.put(at + 12, &flags.to_le_bytes());
            self.put(at + 14, &next.to_le_bytes());
        }

        /// Publishes descriptor chain head `head` and rings the doorbell.
        fn publish(&mut self, head: u16, avail_idx: u16) {
            self.put(
                AVAIL_ADDR + 4 + 2 * ((avail_idx - 1) % QUEUE_SIZE) as u64,
                &head.to_le_bytes(),
            );
            self.put(AVAIL_ADDR + 2, &avail_idx.to_le_bytes());
            self.put(DOORBELL_ADDR, &1u64.to_le_bytes());
        }

        /// The standard three-descriptor request: header (0), data (1),
        /// status (2).
        fn build_request(&mut self, request_type: u32, sector: u64, data_len: u32, write: bool) {
            self.put(HEADER_ADDR, &request_type.to_le_bytes());
            self.put(HEADER_ADDR + 4, &0u32.to_le_bytes());
            self.put(HEADER_ADDR + 8, &sector.to_le_bytes());
            self.desc(0, HEADER_ADDR, REQ_HEADER_SIZE as u32, DESC_F_NEXT, 1);
            if data_len == 0 {
                self.desc(0, HEADER_ADDR, REQ_HEADER_SIZE as u32, DESC_F_NEXT, 2);
            } else {
                self.desc(
                    1,
                    DATA_ADDR,
                    data_len,
                    DESC_F_NEXT | if write { DESC_F_WRITE } else { 0 },
                    2,
                );
            }
            self.desc(2, STATUS_ADDR, 1, DESC_F_WRITE, 0);
        }

        fn used_idx(&self) -> u16 {
            u16::from_le_bytes(self.get(USED_ADDR + 2, 2).try_into().unwrap())
        }

        fn used_entry(&self, slot: u16) -> (u32, u32) {
            let at = USED_ADDR + 4 + 8 * slot as u64;
            (
                u32::from_le_bytes(self.get(at, 4).try_into().unwrap()),
                u32::from_le_bytes(self.get(at + 4, 4).try_into().unwrap()),
            )
        }

        /// `service` + `commit_used` for every completion — what
        /// `boot_image_core` does on a live (recording) boot.
        fn run(&mut self, dev: &mut BlkDevice) -> Result<Vec<Completion>, BlkFault> {
            let mut mem = self.mem();
            let completions = dev.service(&mut mem)?;
            for c in &completions {
                dev.commit_used(&mut mem, c.head, c.len)?;
            }
            Ok(completions)
        }
    }

    // --- feature negotiation (03 §9, decision 2's "what remains") --------

    #[test]
    fn negotiation_accepts_the_offered_set_and_refuses_anything_else() {
        assert_eq!(negotiate(DEVICE_FEATURES), Ok(DEVICE_FEATURES));
        assert_eq!(negotiate(F_VERSION_1), Ok(F_VERSION_1));
        let err = negotiate(F_VERSION_1 | (1 << 5)).expect_err("bit 5 is not offered");
        assert!(err.contains("does not offer"), "{err}");
        let err = negotiate(F_BLK_FLUSH).expect_err("VERSION_1 is mandatory");
        assert!(err.contains("VIRTIO_F_VERSION_1"), "{err}");
    }

    #[test]
    fn a_device_declared_with_an_unoffered_feature_fails_construction_closed() {
        let mut h = Harness::new();
        h.cfg.features = DEVICE_FEATURES | (1 << 12);
        let err = BlkDevice::new(h.cfg.clone()).expect_err("bit 12 is not offered");
        assert!(err.contains("0x1000"), "{err}");
    }

    // --- configuration validation ----------------------------------------

    #[test]
    fn a_queue_size_that_is_not_a_power_of_two_is_refused() {
        let mut h = Harness::new();
        h.cfg.queue.size = 7;
        assert!(
            BlkDevice::new(h.cfg.clone())
                .expect_err("7 is not a power of two")
                .contains("power of two")
        );
        h.cfg.queue.size = 0;
        assert!(BlkDevice::new(h.cfg.clone()).is_err());
    }

    #[test]
    fn a_ring_outside_every_declared_pool_is_refused_before_any_boot() {
        for mangle in [
            |c: &mut BlkConfig| c.queue.desc = machine_layout::DRAM_BASE,
            |c: &mut BlkConfig| c.queue.avail = POOL_BASE + POOL_SIZE,
            |c: &mut BlkConfig| c.queue.used = POOL_BASE + POOL_SIZE - 4,
            |c: &mut BlkConfig| c.queue.doorbell = POOL_BASE - 8,
        ] {
            let h = Harness::new();
            let mut cfg = h.cfg.clone();
            mangle(&mut cfg);
            let err = BlkDevice::new(cfg).expect_err("outside the declared pool");
            assert!(err.contains("declared pool window"), "{err}");
        }
    }

    #[test]
    fn an_absurd_capacity_is_refused_rather_than_allocated() {
        let mut h = Harness::new();
        h.cfg.capacity_sectors = 1 << 40;
        assert!(
            BlkDevice::new(h.cfg.clone())
                .expect_err("512 TiB")
                .contains("ceiling")
        );
        h.cfg.capacity_sectors = u64::MAX;
        assert!(
            BlkDevice::new(h.cfg.clone())
                .expect_err("overflow")
                .contains("overflows")
        );
    }

    #[test]
    fn overlapping_or_out_of_dram_pool_windows_are_refused() {
        let mut h = Harness::new();
        h.cfg.pools.push(PoolWindow {
            name: "Second".to_string(),
            base: POOL_BASE + 0x1000,
            size: 0x1000,
        });
        let pools = h.cfg.pools.clone();
        let err = unsafe { GuestMem::new(h.ram.as_mut_ptr(), pools) }.expect_err("overlap");
        assert!(err.contains("overlapping"), "{err}");

        let err = unsafe {
            GuestMem::new(
                h.ram.as_mut_ptr(),
                vec![PoolWindow {
                    name: "BelowDram".to_string(),
                    base: 0x1000,
                    size: 0x1000,
                }],
            )
        }
        .expect_err("below DRAM");
        assert!(err.contains("not inside guest DRAM"), "{err}");
    }

    // --- the happy paths --------------------------------------------------

    #[test]
    fn a_write_then_a_read_round_trips_through_the_disk() {
        let mut h = Harness::new();
        let mut dev = h.device();
        let payload: Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
        h.put(DATA_ADDR, &payload);
        h.build_request(T_OUT, 3, 512, false);
        h.publish(0, 1);
        let completions = h.run(&mut dev).expect("a well-formed write");
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].head, 0);
        assert_eq!(completions[0].status, STATUS_OK);
        // A write writes only the status byte into guest memory.
        assert_eq!(completions[0].len, 1);
        assert_eq!(h.get(STATUS_ADDR, 1), vec![STATUS_OK]);
        assert_eq!(h.used_idx(), 1);
        assert_eq!(h.used_entry(0), (0, 1));
        assert_eq!(&dev.disk()[3 * 512..4 * 512], &payload[..]);

        // Read it back into a different buffer.
        h.put(DATA_ADDR, &vec![0u8; 512]);
        h.build_request(T_IN, 3, 512, true);
        h.publish(0, 2);
        let completions = h.run(&mut dev).expect("a well-formed read");
        assert_eq!(completions[0].status, STATUS_OK);
        assert_eq!(completions[0].len, 513); // 512 payload + the status byte
        assert_eq!(h.get(DATA_ADDR, 512), payload);
        assert_eq!(h.used_idx(), 2);
        assert_eq!(h.used_entry(1), (0, 513));
    }

    #[test]
    fn a_multi_descriptor_read_scatters_across_the_chain() {
        let mut h = Harness::new();
        let mut dev = h.device();
        let mut disk = vec![0u8; 16 * 512];
        for (i, b) in disk.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        dev.set_disk(disk.clone());

        // header(0) -> data(1, 512B) -> data(3, 512B) -> status(2)
        h.put(HEADER_ADDR, &T_IN.to_le_bytes());
        h.put(HEADER_ADDR + 8, &1u64.to_le_bytes());
        h.desc(0, HEADER_ADDR, 16, DESC_F_NEXT, 1);
        h.desc(1, DATA_ADDR, 512, DESC_F_NEXT | DESC_F_WRITE, 3);
        h.desc(3, DATA_ADDR + 512, 512, DESC_F_NEXT | DESC_F_WRITE, 2);
        h.desc(2, STATUS_ADDR, 1, DESC_F_WRITE, 0);
        h.publish(0, 1);

        let completions = h.run(&mut dev).expect("a well-formed scattered read");
        assert_eq!(completions[0].len, 1025);
        assert_eq!(h.get(DATA_ADDR, 1024), disk[512..512 + 1024].to_vec());
    }

    #[test]
    fn two_chains_published_under_one_doorbell_both_complete_in_order() {
        let mut h = Harness::new();
        let mut dev = h.device();
        // chain A: header(0) data(1) status(2); chain B: header(3) status(4)
        // (a Flush, which needs no data).
        h.put(HEADER_ADDR, &T_OUT.to_le_bytes());
        h.put(HEADER_ADDR + 8, &0u64.to_le_bytes());
        h.desc(0, HEADER_ADDR, 16, DESC_F_NEXT, 1);
        h.desc(1, DATA_ADDR, 512, DESC_F_NEXT, 2);
        h.desc(2, STATUS_ADDR, 1, DESC_F_WRITE, 0);
        h.put(HEADER_ADDR + 32, &T_FLUSH.to_le_bytes());
        h.put(HEADER_ADDR + 40, &0u64.to_le_bytes());
        h.desc(3, HEADER_ADDR + 32, 16, DESC_F_NEXT, 4);
        h.desc(4, STATUS_ADDR + 1, 1, DESC_F_WRITE, 0);

        h.put(AVAIL_ADDR + 4, &0u16.to_le_bytes());
        h.put(AVAIL_ADDR + 6, &3u16.to_le_bytes());
        h.put(AVAIL_ADDR + 2, &2u16.to_le_bytes());
        h.put(DOORBELL_ADDR, &1u64.to_le_bytes());

        let completions = h.run(&mut dev).expect("both chains");
        assert_eq!(completions.len(), 2);
        assert_eq!(completions[0].head, 0);
        assert_eq!(completions[1].head, 3);
        assert!(completions.iter().all(|c| c.status == STATUS_OK));
        assert_eq!(h.used_idx(), 2);
        assert_eq!(h.used_entry(0).0, 0);
        assert_eq!(h.used_entry(1).0, 3);
        // The doorbell is cleared by the service that consumed it.
        assert_eq!(h.get(DOORBELL_ADDR, 8), vec![0u8; 8]);
    }

    #[test]
    fn an_unrung_doorbell_services_nothing_at_all() {
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        // Publish without ringing.
        h.put(AVAIL_ADDR + 4, &0u16.to_le_bytes());
        h.put(AVAIL_ADDR + 2, &1u16.to_le_bytes());
        assert_eq!(h.run(&mut dev).expect("no doorbell"), Vec::new());
        assert_eq!(h.used_idx(), 0);
    }

    #[test]
    fn flush_completes_ok_when_negotiated_and_unsupported_when_not() {
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_FLUSH, 0, 0, false);
        h.publish(0, 1);
        assert_eq!(h.run(&mut dev).expect("flush")[0].status, STATUS_OK);

        let mut h = Harness::new();
        h.cfg.features = F_VERSION_1; // no VIRTIO_BLK_F_FLUSH
        let mut dev = h.device();
        h.build_request(T_FLUSH, 0, 0, false);
        h.publish(0, 1);
        assert_eq!(h.run(&mut dev).expect("flush")[0].status, STATUS_UNSUPP);
    }

    #[test]
    fn an_unknown_request_type_completes_unsupported_rather_than_faulting() {
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(9999, 0, 0, false);
        h.publish(0, 1);
        let c = h.run(&mut dev).expect("an in-protocol answer");
        assert_eq!(c[0].status, STATUS_UNSUPP);
        assert_eq!(h.get(STATUS_ADDR, 1), vec![STATUS_UNSUPP]);
    }

    #[test]
    fn a_sector_past_the_end_of_the_disk_is_an_io_error_not_a_fault() {
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 15, 1024, true); // 16-sector disk, 2 sectors from 15
        h.publish(0, 1);
        let c = h.run(&mut dev).expect("an in-protocol answer");
        assert_eq!(c[0].status, STATUS_IOERR);
        assert_eq!(c[0].len, 1, "no payload is transferred on an IO error");

        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, u64::MAX, 512, true); // sector * 512 overflows
        h.publish(0, 1);
        assert_eq!(h.run(&mut dev).expect("no panic")[0].status, STATUS_IOERR);
    }

    #[test]
    fn the_completion_digest_covers_every_written_byte_and_changes_with_them() {
        let mut h = Harness::new();
        let mut dev = h.device();
        let mut disk = vec![0u8; 16 * 512];
        disk[0] = 7;
        dev.set_disk(disk.clone());
        h.build_request(T_IN, 0, 512, true);
        h.publish(0, 1);
        let a = h.run(&mut dev).expect("read")[0].digest.clone();

        let mut h2 = Harness::new();
        let mut dev2 = h2.device();
        disk[0] = 8;
        dev2.set_disk(disk);
        h2.build_request(T_IN, 0, 512, true);
        h2.publish(0, 1);
        let b = h2.run(&mut dev2).expect("read")[0].digest.clone();
        assert_ne!(a, b, "one differing disk byte must change the digest");
    }

    // --- malformed rings: every rejection, by name ------------------------

    /// The heart of item F's own security claim: a malformed ring is a
    /// named, diagnosable VMM-side error — never a panic, never an
    /// out-of-bounds read, never a silently truncated operation.
    #[test]
    fn every_malformed_ring_shape_is_rejected_by_name() {
        // (a) An avail.ring entry naming a descriptor slot that does not
        //     exist — 03 §4's "unknown ID".
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.publish(QUEUE_SIZE, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::DescriptorIndexOutOfRange {
                index: QUEUE_SIZE,
                queue_size: QUEUE_SIZE
            })
        );

        // (b) A `next` naming a slot that does not exist.
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.desc(1, DATA_ADDR, 512, DESC_F_NEXT | DESC_F_WRITE, 200);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::DescriptorIndexOutOfRange {
                index: 200,
                queue_size: QUEUE_SIZE
            })
        );

        // (c) A chain that loops back on itself — 03 §4's "duplicate".
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.desc(1, DATA_ADDR, 512, DESC_F_NEXT | DESC_F_WRITE, 0);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::DescriptorChainLoop { index: 0 })
        );

        // (d) A chain longer than the queue is deep (every slot distinct,
        //     so the visited set alone would not catch it).
        let mut h = Harness::new();
        let mut dev = h.device();
        for i in 0..QUEUE_SIZE {
            h.desc(
                i,
                DATA_ADDR,
                16,
                DESC_F_NEXT,
                (i + 1) % QUEUE_SIZE, // slot 7 -> slot 0, all distinct until then
            );
        }
        h.publish(0, 1);
        let got = h.run(&mut dev);
        assert!(
            matches!(
                got,
                Err(BlkFault::DescriptorChainTooLong { .. })
                    | Err(BlkFault::DescriptorChainLoop { .. })
            ),
            "a chain that never terminates must be refused, got {got:?}"
        );

        // (e) A descriptor pointing outside every declared pool — decision
        //     5's boundary, and the one rejection that is a *security*
        //     property rather than a protocol one.
        for bad_addr in [
            machine_layout::DRAM_BASE, // the machine-info page
            POOL_BASE - 1,             // one byte before the pool
            POOL_BASE + POOL_SIZE - 8, // straddling the end
            machine_layout::DRAM_BASE + machine_layout::DRAM_SIZE, // past DRAM entirely
            u64::MAX - 4,              // overflowing addr + len
        ] {
            let mut h = Harness::new();
            let mut dev = h.device();
            h.build_request(T_IN, 0, 512, true);
            h.desc(1, bad_addr, 512, DESC_F_NEXT | DESC_F_WRITE, 2);
            h.publish(0, 1);
            let got = h.run(&mut dev);
            assert!(
                matches!(got, Err(BlkFault::OutsidePool { .. })),
                "descriptor addr {bad_addr:#x} must be refused, got {got:?}"
            );
        }

        // (f) VIRTQ_DESC_F_INDIRECT, never offered.
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.desc(1, DATA_ADDR, 512, DESC_F_INDIRECT | DESC_F_WRITE, 2);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::IndirectNotNegotiated { index: 1 })
        );

        // (g) A one-descriptor chain — no room for a header and a status.
        let mut h = Harness::new();
        let mut dev = h.device();
        h.desc(0, HEADER_ADDR, 16, 0, 0);
        h.publish(0, 1);
        assert_eq!(h.run(&mut dev), Err(BlkFault::ChainTooShort { len: 1 }));

        // (h) A header of the wrong size, and a device-writable header.
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.desc(0, HEADER_ADDR, 8, DESC_F_NEXT, 1);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::BadRequestHeader {
                len: 8,
                device_writable: false
            })
        );
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.desc(0, HEADER_ADDR, 16, DESC_F_NEXT | DESC_F_WRITE, 1);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::BadRequestHeader {
                len: 16,
                device_writable: true
            })
        );

        // (i) A status descriptor that is device-readable, or empty.
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.desc(2, STATUS_ADDR, 1, 0, 0);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::BadStatusDescriptor {
                len: 1,
                device_writable: false
            })
        );
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.desc(2, STATUS_ADDR, 0, DESC_F_WRITE, 0);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::BadStatusDescriptor {
                len: 0,
                device_writable: true
            })
        );

        // (j) A read into a device-readable buffer (the device would have
        //     to write where the driver said "read only").
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, false);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::DataDirectionMismatch {
                request_type: T_IN,
                device_writable: false
            })
        );

        // (k) A write out of a device-writable buffer.
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_OUT, 0, 512, true);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::DataDirectionMismatch {
                request_type: T_OUT,
                device_writable: true
            })
        );

        // (l) A partial-sector transfer.
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 500, true);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::UnalignedDataLength { len: 500 })
        );

        // (m) A Flush carrying data.
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_FLUSH, 0, 512, false);
        h.publish(0, 1);
        assert_eq!(h.run(&mut dev), Err(BlkFault::FlushWithData { len: 512 }));

        // (n) An avail.idx that jumps further than the queue is deep —
        //     03 §4's "stale".
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.put(AVAIL_ADDR + 2, &(QUEUE_SIZE + 1).to_le_bytes());
        h.put(DOORBELL_ADDR, &1u64.to_le_bytes());
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::AvailIndexJump {
                last: 0,
                now: QUEUE_SIZE + 1,
                queue_size: QUEUE_SIZE
            })
        );
    }

    /// The ring itself is guest-writable memory, so a *ring word* — not
    /// just a descriptor's `addr` — can name anything at all. This is the
    /// proof that even the ring's own bookkeeping reads go through the
    /// window check: a device whose declared pool is deliberately too
    /// small to hold its own used ring is refused at construction, so no
    /// path exists that reads or writes ring bytes outside a window.
    #[test]
    fn no_ring_access_can_escape_the_declared_windows() {
        let h = Harness::new();
        let mut cfg = h.cfg.clone();
        // A pool that covers the descriptor table but stops before the
        // used ring.
        cfg.pools = vec![PoolWindow {
            name: "TooSmall".to_string(),
            base: POOL_BASE,
            size: 0x600,
        }];
        let err = BlkDevice::new(cfg).expect_err("the used ring is outside the window");
        assert!(err.contains("used ring"), "{err}");
    }

    /// Every `BlkFault` renders a distinct, non-empty diagnostic (a fault
    /// nobody can read is a fault nobody can act on).
    #[test]
    fn every_fault_renders_a_distinct_diagnostic() {
        let faults = vec![
            BlkFault::OutsidePool {
                addr: 1,
                len: 2,
                why: "x",
            },
            BlkFault::AvailIndexJump {
                last: 0,
                now: 9,
                queue_size: 8,
            },
            BlkFault::DescriptorIndexOutOfRange {
                index: 9,
                queue_size: 8,
            },
            BlkFault::DescriptorChainLoop { index: 1 },
            BlkFault::DescriptorChainTooLong { queue_size: 8 },
            BlkFault::IndirectNotNegotiated { index: 1 },
            BlkFault::ChainTooShort { len: 1 },
            BlkFault::BadRequestHeader {
                len: 8,
                device_writable: false,
            },
            BlkFault::BadStatusDescriptor {
                len: 0,
                device_writable: true,
            },
            BlkFault::DataDirectionMismatch {
                request_type: 0,
                device_writable: false,
            },
            BlkFault::UnalignedDataLength { len: 500 },
            BlkFault::FlushWithData { len: 512 },
        ];
        let mut seen = std::collections::BTreeSet::new();
        for f in &faults {
            let s = f.to_string();
            assert!(!s.is_empty());
            assert!(seen.insert(s.clone()), "duplicate diagnostic: {s}");
        }
    }
}
