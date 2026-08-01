use crate::record::digest_hex;
use wrela_machine::layout as machine_layout;
use wrela_machine::virtio::quiesce_count_addr as virtio_quiesce_count_addr;

pub use wrela_machine::report::{BlkConfig, BlkQueueConfig, PoolWindow};
pub use wrela_machine::virtio::{
    DESC_F_NEXT, DESC_F_WRITE, DESC_SIZE, DEVICE_FEATURES, F_BLK_FLUSH, F_VERSION_1,
    REQ_HEADER_SIZE,
};

pub const DESC_F_INDIRECT: u16 = 4;

pub const T_IN: u32 = 0;
pub const T_OUT: u32 = 1;
pub const T_FLUSH: u32 = 4;

pub const STATUS_OK: u8 = 0;
pub const STATUS_IOERR: u8 = 1;
pub const STATUS_UNSUPP: u8 = 2;

pub const SECTOR_SIZE: u64 = 512;

pub const MAX_DISK_BYTES: u64 = 64 << 20;

pub const MAX_BLK_QUEUE_SIZE: u16 = 1024;

pub struct GuestMem {
    base: *mut u8,
    windows: Vec<PoolWindow>,
    device: u64,
}

impl GuestMem {
    pub unsafe fn new(
        base: *mut u8,
        windows: Vec<PoolWindow>,
        device: u64,
    ) -> Result<GuestMem, String> {
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
        Ok(GuestMem {
            base,
            windows,
            device,
        })
    }

    fn window_offset(&self, addr: u64, len: u64) -> Result<usize, BlkFault> {
        let end = addr.checked_add(len).ok_or(BlkFault::OutsidePool {
            addr,
            len,
            why: "address + length overflows a u64",
        })?;
        for w in &self.windows {
            if addr >= w.base && end <= w.base + w.size {
                if w.device != self.device {
                    return Err(BlkFault::ForeignPool {
                        offset: addr - w.base,
                        len,
                        pool: w.name.clone(),
                        owner: w.device,
                        device: self.device,
                    });
                }
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

fn window_contains(windows: &[PoolWindow], device: u64, addr: u64, len: u64) -> bool {
    match addr.checked_add(len) {
        None => false,
        Some(end) => windows
            .iter()
            .any(|w| w.device == device && addr >= w.base && end <= w.base + w.size),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlkFault {
    OutsidePool {
        addr: u64,
        len: u64,
        why: &'static str,
    },
    ForeignPool {
        offset: u64,
        len: u64,
        pool: String,
        owner: u64,
        device: u64,
    },
    AvailIndexJump {
        last: u16,
        now: u16,
        queue_size: u16,
    },
    DescriptorIndexOutOfRange {
        index: u16,
        queue_size: u16,
    },
    DescriptorChainLoop {
        index: u16,
    },
    DescriptorChainTooLong {
        queue_size: u16,
    },
    IndirectNotNegotiated {
        index: u16,
    },
    ChainTooShort {
        len: usize,
    },
    BadRequestHeader {
        len: u32,
        device_writable: bool,
    },
    BadStatusDescriptor {
        len: u32,
        device_writable: bool,
    },
    DataDirectionMismatch {
        request_type: u32,
        device_writable: bool,
    },
    UnalignedDataLength {
        len: u64,
    },
    FlushWithData {
        len: u64,
    },
    QuiesceWrongWord {
        named: u64,
        expected: u64,
        device: u64,
    },
    DescTooLarge {
        len: u64,
    },
}

impl std::fmt::Display for BlkFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlkFault::OutsidePool { addr, len, why } => write!(
                f,
                "guest range [{addr:#x}, +{len}) is not device-reachable: {why} \
                 (plans/M7.md decision 5: the device model may touch declared pool pages only)"
            ),
            BlkFault::ForeignPool {
                offset,
                len,
                pool,
                owner,
                device,
            } => write!(
                f,
                "guest range [offset {offset:#x}, +{len}) lies in pool `{pool}`, which is bound to \
                 device#{owner}, but this access is device#{device}'s \
                 (03-hardware.md §3: all memory a device can reach originates from *its* bound \
                 pools)"
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
            BlkFault::QuiesceWrongWord {
                named,
                expected,
                device,
            } => write!(
                f,
                "a quiesce named {named:#x} as device#{device}'s quiesce-count word, but this \
                 queue's own is {expected:#x} (03-hardware.md §9: reclaim is gated on the count \
                 this VMM writes, so it may only ever be the queue's own)"
            ),
            BlkFault::DescTooLarge { len } => write!(
                f,
                "virtio data descriptor length {len} exceeds this VMM's {MAX_DISK_BYTES}-byte \
                 host-allocation ceiling (a driver fault, never an unbounded host alloc)"
            ),
        }
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub head: u16,
    pub status: u8,
    pub len: u32,
    pub digest: String,
}

pub struct BlkDevice {
    pub config: BlkConfig,
    pub negotiated: u64,
    disk: Vec<u8>,
    last_avail_idx: u16,
    used_idx: u16,
}

impl BlkDevice {
    pub fn new(config: BlkConfig) -> Result<BlkDevice, String> {
        let negotiated = negotiate(config.features)?;
        let q = &config.queue;
        if q.size == 0 || !q.size.is_power_of_two() {
            return Err(format!(
                "blk queue size {} must be a nonzero power of two (VIRTIO 1.2 §2.6)",
                q.size
            ));
        }
        if q.size > MAX_BLK_QUEUE_SIZE {
            return Err(format!(
                "blk queue size {} exceeds this VMM's own {MAX_BLK_QUEUE_SIZE}-deep ceiling \
                 (a forged report must not force unbounded memcpy per doorbell poll)",
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
            if !window_contains(&config.pools, config.device, addr, len) {
                return Err(format!(
                    "the blk {what} at [{addr:#x}, +{len}) is not inside a pool window bound to \
                     device#{} (plans/M7.md decision 5: shared control memory lives in a declared \
                     pool, and the model may reach nothing else; plans/M8.md item P: the pool must \
                     be one of *this* device's — 03-hardware.md §3)",
                    config.device
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

    pub fn set_disk(&mut self, bytes: Vec<u8>) {
        self.disk = bytes;
    }

    pub fn disk(&self) -> &[u8] {
        &self.disk
    }

    pub fn service(&mut self, mem: &mut GuestMem) -> Result<Vec<Completion>, BlkFault> {
        let q = self.config.queue.clone();
        if mem.read_u64(q.doorbell)? == 0 {
            return Ok(Vec::new());
        }
        mem.write_u64(q.doorbell, 0)?;
        self.execute_available(mem)
    }

    fn execute_available(&mut self, mem: &mut GuestMem) -> Result<Vec<Completion>, BlkFault> {
        let q = self.config.queue.clone();
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

    pub fn quiesce(&mut self, mem: &mut GuestMem, named: u64) -> Result<Vec<Completion>, BlkFault> {
        let expected = self.quiesce_count_addr();
        if named != expected {
            return Err(BlkFault::QuiesceWrongWord {
                named,
                expected,
                device: self.config.device,
            });
        }
        let completions = self.execute_available(mem)?;
        mem.write_u64(self.config.queue.doorbell, 0)?;
        let count = mem.read_u64(expected)?.wrapping_add(1);
        mem.write_u64(expected, count)?;
        Ok(completions)
    }

    pub fn quiesce_count_addr(&self) -> u64 {
        virtio_quiesce_count_addr(self.config.queue.doorbell)
    }

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
        mem.window_offset(addr, len as u64)?;
        Ok(Descriptor {
            addr,
            len,
            flags,
            next,
        })
    }

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
        let mut data_len: u64 = 0;
        for d in data {
            if (d.len as u64) > MAX_DISK_BYTES {
                return Err(BlkFault::DescTooLarge { len: d.len as u64 });
            }
            data_len = data_len
                .checked_add(d.len as u64)
                .ok_or(BlkFault::DescTooLarge { len: d.len as u64 })?;
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
                    _ => STATUS_IOERR,
                }
            }
            T_FLUSH if self.negotiated & F_BLK_FLUSH != 0 => STATUS_OK,
            _ => STATUS_UNSUPP,
        };
        if request_type == T_OUT && status == STATUS_OK {
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

impl std::fmt::Debug for GuestMem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestMem")
            .field("windows", &self.windows)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy)]
struct Descriptor {
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

    struct Harness {
        ram: Vec<u8>,
        cfg: BlkConfig,
    }

    const POOL_BASE: u64 = machine_layout::DRAM_BASE + 0x10_0000;
    const POOL_SIZE: u64 = 0x10_0000;
    const QUEUE_SIZE: u16 = 8;
    const HARNESS_DEVICE: u64 = 0;

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
                device: HARNESS_DEVICE,
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
                    device: HARNESS_DEVICE,
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
            unsafe {
                GuestMem::new(
                    self.ram.as_mut_ptr(),
                    self.cfg.pools.clone(),
                    HARNESS_DEVICE,
                )
            }
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

        fn publish(&mut self, head: u16, avail_idx: u16) {
            self.put(
                AVAIL_ADDR + 4 + 2 * ((avail_idx - 1) % QUEUE_SIZE) as u64,
                &head.to_le_bytes(),
            );
            self.put(AVAIL_ADDR + 2, &avail_idx.to_le_bytes());
            self.put(DOORBELL_ADDR, &1u64.to_le_bytes());
        }

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

        fn run(&mut self, dev: &mut BlkDevice) -> Result<Vec<Completion>, BlkFault> {
            let mut mem = self.mem();
            let completions = dev.service(&mut mem)?;
            for c in &completions {
                dev.commit_used(&mut mem, c.head, c.len)?;
            }
            Ok(completions)
        }
    }

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
    fn a_queue_size_above_the_vmm_ceiling_is_refused() {
        let mut h = Harness::new();
        h.cfg.queue.size = 32768;
        let err = BlkDevice::new(h.cfg.clone()).expect_err("32768 exceeds the VMM ceiling");
        assert!(
            err.contains(&MAX_BLK_QUEUE_SIZE.to_string()),
            "must name the ceiling: {err}"
        );
    }

    #[test]
    fn a_ring_outside_every_declared_pool_is_refused_before_any_boot() {
        for mangle in [
            |c: &mut BlkConfig| c.queue.desc = machine_layout::DRAM_BASE,
            |c: &mut BlkConfig| c.queue.avail = POOL_BASE + POOL_SIZE,
            |c: &mut BlkConfig| c.queue.used = POOL_BASE + POOL_SIZE - 4,
            |c: &mut BlkConfig| c.queue.doorbell = POOL_BASE - 8,
            |c: &mut BlkConfig| {
                c.pools.push(PoolWindow {
                    name: "Foreign".to_string(),
                    device: HARNESS_DEVICE + 1,
                    base: POOL_BASE + POOL_SIZE,
                    size: 0x1000,
                });
                c.queue.doorbell = POOL_BASE + POOL_SIZE;
            },
        ] {
            let h = Harness::new();
            let mut cfg = h.cfg.clone();
            mangle(&mut cfg);
            let err = BlkDevice::new(cfg).expect_err("outside this device's declared pools");
            assert!(err.contains("pool window bound to device#0"), "{err}");
        }
    }

    #[test]
    fn a_window_bound_to_another_device_is_refused_by_name() {
        let mut h = Harness::new();
        const FOREIGN_BASE: u64 = POOL_BASE + POOL_SIZE;
        const FOREIGN_SIZE: u64 = 0x1000;
        h.ram.resize(
            (FOREIGN_BASE + FOREIGN_SIZE - machine_layout::DRAM_BASE) as usize,
            0,
        );
        let windows = vec![
            h.cfg.pools[0].clone(),
            PoolWindow {
                name: "Foreign".to_string(),
                device: HARNESS_DEVICE + 1,
                base: FOREIGN_BASE,
                size: FOREIGN_SIZE,
            },
        ];
        let mine = unsafe {
            GuestMem::new(h.ram.as_mut_ptr(), windows.clone(), HARNESS_DEVICE).expect("windows")
        };
        assert!(mine.window_offset(POOL_BASE, 8).is_ok());
        assert!(matches!(
            mine.window_offset(FOREIGN_BASE, 8),
            Err(BlkFault::ForeignPool {
                owner: 1,
                device: 0,
                ..
            })
        ));
        assert!(matches!(
            mine.window_offset(FOREIGN_BASE + 8, 8),
            Err(BlkFault::ForeignPool { offset: 8, .. })
        ));
        let theirs = unsafe {
            GuestMem::new(h.ram.as_mut_ptr(), windows, HARNESS_DEVICE + 1).expect("windows")
        };
        assert!(theirs.window_offset(FOREIGN_BASE, 8).is_ok());
        assert!(matches!(
            theirs.window_offset(POOL_BASE, 8),
            Err(BlkFault::ForeignPool {
                owner: 0,
                device: 1,
                ..
            })
        ));
        assert!(matches!(
            mine.window_offset(machine_layout::DRAM_BASE, 8),
            Err(BlkFault::OutsidePool { .. })
        ));
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
            device: HARNESS_DEVICE + 1,
            base: POOL_BASE + 0x1000,
            size: 0x1000,
        });
        let pools = h.cfg.pools.clone();
        let err = unsafe { GuestMem::new(h.ram.as_mut_ptr(), pools, HARNESS_DEVICE) }
            .expect_err("overlap");
        assert!(err.contains("overlapping"), "{err}");

        let err = unsafe {
            GuestMem::new(
                h.ram.as_mut_ptr(),
                vec![PoolWindow {
                    name: "BelowDram".to_string(),
                    device: HARNESS_DEVICE,
                    base: 0x1000,
                    size: 0x1000,
                }],
                HARNESS_DEVICE,
            )
        }
        .expect_err("below DRAM");
        assert!(err.contains("not inside guest DRAM"), "{err}");
    }

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
        assert_eq!(completions[0].len, 1);
        assert_eq!(h.get(STATUS_ADDR, 1), vec![STATUS_OK]);
        assert_eq!(h.used_idx(), 1);
        assert_eq!(h.used_entry(0), (0, 1));
        assert_eq!(&dev.disk()[3 * 512..4 * 512], &payload[..]);

        h.put(DATA_ADDR, &vec![0u8; 512]);
        h.build_request(T_IN, 3, 512, true);
        h.publish(0, 2);
        let completions = h.run(&mut dev).expect("a well-formed read");
        assert_eq!(completions[0].status, STATUS_OK);
        assert_eq!(completions[0].len, 513);
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
        assert_eq!(h.get(DOORBELL_ADDR, 8), vec![0u8; 8]);
    }

    #[test]
    fn an_unrung_doorbell_services_nothing_at_all() {
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.put(AVAIL_ADDR + 4, &0u16.to_le_bytes());
        h.put(AVAIL_ADDR + 2, &1u16.to_le_bytes());
        assert_eq!(h.run(&mut dev).expect("no doorbell"), Vec::new());
        assert_eq!(h.used_idx(), 0);
    }

    #[test]
    fn a_quiesce_finishes_outstanding_work_and_bumps_the_host_written_count() {
        let mut h = Harness::new();
        let mut dev = h.device();
        dev.set_disk(vec![0xAB; 16 * SECTOR_SIZE as usize]);
        h.build_request(T_IN, 0, 512, true);
        h.publish(0, 1);
        let count_addr = dev.quiesce_count_addr();
        let completions = {
            let mut mem = h.mem();
            dev.quiesce(&mut mem, count_addr)
                .expect("a well-formed quiesce")
        };
        assert_eq!(completions.len(), 1, "the outstanding read was finished");
        assert_eq!(completions[0].status, STATUS_OK);
        assert_eq!(
            h.get(DATA_ADDR, 4),
            vec![0xAB; 4],
            "the read's payload landed before the quiesce returned"
        );
        assert_eq!(
            u64::from_le_bytes(h.get(count_addr, 8).try_into().unwrap()),
            1,
            "the host-written quiesce count is the driver's only evidence"
        );
        assert_eq!(
            u64::from_le_bytes(h.get(DOORBELL_ADDR, 8).try_into().unwrap()),
            0,
            "the doorbell is clear afterwards"
        );
    }

    #[test]
    fn no_chain_published_before_a_quiesce_is_ever_executed_after_it() {
        let mut h = Harness::new();
        let mut dev = h.device();
        dev.set_disk(vec![0xAB; 16 * SECTOR_SIZE as usize]);
        h.build_request(T_IN, 0, 512, true);
        h.publish(0, 1);
        let count_addr = dev.quiesce_count_addr();
        {
            let mut mem = h.mem();
            dev.quiesce(&mut mem, count_addr).expect("quiesce");
        }
        h.put(DATA_ADDR, &[0u8; 512]);
        h.put(DOORBELL_ADDR, &1u64.to_le_bytes());
        assert_eq!(
            h.run(&mut dev).expect("nothing outstanding"),
            Vec::new(),
            "a pre-quiesce chain is not re-executed"
        );
        assert_eq!(
            h.get(DATA_ADDR, 512),
            vec![0u8; 512],
            "no post-quiesce write reached the reclaimed buffer"
        );
    }

    #[test]
    fn a_quiesce_naming_any_other_word_is_refused_by_name() {
        let mut h = Harness::new();
        let mut dev = h.device();
        let count_addr = dev.quiesce_count_addr();
        let mut mem = h.mem();
        let err = dev
            .quiesce(&mut mem, count_addr + 8)
            .expect_err("a foreign word is refused");
        assert!(matches!(err, BlkFault::QuiesceWrongWord { .. }), "{err:?}");
        let text = err.to_string();
        assert!(text.contains("quiesce-count word"), "{text}");
        drop(mem);
        assert_eq!(
            u64::from_le_bytes(h.get(count_addr, 8).try_into().unwrap()),
            0
        );
    }

    #[test]
    fn a_second_quiesce_bumps_the_count_twice_and_is_not_refused() {
        let mut h = Harness::new();
        let mut dev = h.device();
        dev.set_disk(vec![0xAB; 16 * SECTOR_SIZE as usize]);
        h.build_request(T_IN, 0, 512, true);
        h.publish(0, 1);
        let count_addr = dev.quiesce_count_addr();
        let first = {
            let mut mem = h.mem();
            dev.quiesce(&mut mem, count_addr).expect("first quiesce")
        };
        assert_eq!(
            first.len(),
            1,
            "first quiesce finishes the outstanding chain"
        );
        assert_eq!(
            u64::from_le_bytes(h.get(count_addr, 8).try_into().unwrap()),
            1
        );
        let second = {
            let mut mem = h.mem();
            dev.quiesce(&mut mem, count_addr).expect("second quiesce")
        };
        assert_eq!(second.len(), 0, "second quiesce has nothing left to finish");
        assert_eq!(
            u64::from_le_bytes(h.get(count_addr, 8).try_into().unwrap()),
            2,
            "the host-written count moved exactly once per quiesce"
        );
    }

    #[test]
    fn double_quiesce_before_quarantine_cannot_satisfy_a_reclaim_that_should_refuse() {
        use wrela_compiler::virtqueue::{ReclaimGate, SLOT_FLAG_QUARANTINED, reclaim_gate};
        let mut h = Harness::new();
        let mut dev = h.device();
        let count_addr = dev.quiesce_count_addr();
        {
            let mut mem = h.mem();
            dev.quiesce(&mut mem, count_addr).expect("q1");
            dev.quiesce(&mut mem, count_addr).expect("q2");
        }
        let quiesced = u64::from_le_bytes(h.get(count_addr, 8).try_into().unwrap());
        assert_eq!(quiesced, 2);
        assert_eq!(
            reclaim_gate(SLOT_FLAG_QUARANTINED, quiesced, quiesced),
            ReclaimGate::NotQuiesced,
            "stamp==count after double-quiesce-then-quarantine must refuse reclaim"
        );
        {
            let mut mem = h.mem();
            dev.quiesce(&mut mem, count_addr).expect("q3");
        }
        let quiesced_after = u64::from_le_bytes(h.get(count_addr, 8).try_into().unwrap());
        assert_eq!(quiesced_after, 3);
        assert_eq!(
            reclaim_gate(SLOT_FLAG_QUARANTINED, quiesced_after, quiesced),
            ReclaimGate::Reclaim
        );
    }

    #[test]
    fn a_quiesce_finishes_every_outstanding_chain_before_bumping() {
        let mut h = Harness::new();
        let mut dev = h.device();
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
        h.put(DOORBELL_ADDR, &0u64.to_le_bytes());

        let count_addr = dev.quiesce_count_addr();
        let completions = {
            let mut mem = h.mem();
            dev.quiesce(&mut mem, count_addr).expect("quiesce")
        };
        assert_eq!(
            completions.len(),
            2,
            "both outstanding chains finished inside the quiesce"
        );
        assert_eq!(
            u64::from_le_bytes(h.get(count_addr, 8).try_into().unwrap()),
            1,
            "count moves once after both are finished"
        );
    }

    #[test]
    fn quiesce_count_addr_is_the_shared_machine_formula() {
        let h = Harness::new();
        let dev = h.device();
        assert_eq!(
            dev.quiesce_count_addr(),
            wrela_machine::virtio::quiesce_count_addr(h.cfg.queue.doorbell)
        );
        assert_eq!(
            dev.quiesce_count_addr(),
            h.cfg.queue.doorbell
                + wrela_machine::virtio::DOORBELL_BYTES
                + wrela_machine::virtio::SLOT_BOOK_QUIESCED
        );
        assert_eq!(
            wrela_machine::virtio::avail_bytes(h.cfg.queue.size),
            4 + 2 * h.cfg.queue.size as u64
        );
        assert_eq!(
            wrela_machine::virtio::used_bytes(h.cfg.queue.size),
            4 + 8 * h.cfg.queue.size as u64
        );
        assert_eq!(
            wrela_machine::virtio::desc_bytes(h.cfg.queue.size),
            h.cfg.queue.size as u64 * wrela_machine::virtio::DESC_SIZE
        );
    }

    #[test]
    fn flush_completes_ok_when_negotiated_and_unsupported_when_not() {
        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_FLUSH, 0, 0, false);
        h.publish(0, 1);
        assert_eq!(h.run(&mut dev).expect("flush")[0].status, STATUS_OK);

        let mut h = Harness::new();
        h.cfg.features = F_VERSION_1;
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
        h.build_request(T_IN, 15, 1024, true);
        h.publish(0, 1);
        let c = h.run(&mut dev).expect("an in-protocol answer");
        assert_eq!(c[0].status, STATUS_IOERR);
        assert_eq!(c[0].len, 1, "no payload is transferred on an IO error");

        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, u64::MAX, 512, true);
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

    #[test]
    fn every_malformed_ring_shape_is_rejected_by_name() {
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

        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.desc(1, DATA_ADDR, 512, DESC_F_NEXT | DESC_F_WRITE, 0);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::DescriptorChainLoop { index: 0 })
        );

        let mut h = Harness::new();
        let mut dev = h.device();
        for i in 0..QUEUE_SIZE {
            h.desc(i, DATA_ADDR, 16, DESC_F_NEXT, (i + 1) % QUEUE_SIZE);
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

        for bad_addr in [
            machine_layout::DRAM_BASE,
            POOL_BASE - 1,
            POOL_BASE + POOL_SIZE - 8,
            machine_layout::DRAM_BASE + machine_layout::DRAM_SIZE,
            u64::MAX - 4,
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

        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 512, true);
        h.desc(1, DATA_ADDR, 512, DESC_F_INDIRECT | DESC_F_WRITE, 2);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::IndirectNotNegotiated { index: 1 })
        );

        let mut h = Harness::new();
        let mut dev = h.device();
        h.desc(0, HEADER_ADDR, 16, 0, 0);
        h.publish(0, 1);
        assert_eq!(h.run(&mut dev), Err(BlkFault::ChainTooShort { len: 1 }));

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

        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_IN, 0, 500, true);
        h.publish(0, 1);
        assert_eq!(
            h.run(&mut dev),
            Err(BlkFault::UnalignedDataLength { len: 500 })
        );

        let mut h = Harness::new();
        let mut dev = h.device();
        h.build_request(T_FLUSH, 0, 512, false);
        h.publish(0, 1);
        assert_eq!(h.run(&mut dev), Err(BlkFault::FlushWithData { len: 512 }));

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

    #[test]
    fn no_ring_access_can_escape_the_declared_windows() {
        let h = Harness::new();
        let mut cfg = h.cfg.clone();
        cfg.pools = vec![PoolWindow {
            name: "TooSmall".to_string(),
            device: HARNESS_DEVICE,
            base: POOL_BASE,
            size: 0x600,
        }];
        let err = BlkDevice::new(cfg).expect_err("the used ring is outside the window");
        assert!(err.contains("used ring"), "{err}");
    }

    #[test]
    fn every_fault_renders_a_distinct_diagnostic() {
        let faults = vec![
            BlkFault::OutsidePool {
                addr: 1,
                len: 2,
                why: "x",
            },
            BlkFault::ForeignPool {
                offset: 1,
                len: 2,
                pool: "Other".to_string(),
                owner: 1,
                device: 0,
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
            BlkFault::DescTooLarge {
                len: MAX_DISK_BYTES + 1,
            },
            BlkFault::QuiesceWrongWord {
                named: 1,
                expected: 2,
                device: 0,
            },
        ];
        let mut seen = std::collections::BTreeSet::new();
        for f in &faults {
            let s = f.to_string();
            assert!(!s.is_empty());
            assert!(seen.insert(s.clone()), "duplicate diagnostic: {s}");
        }
    }
}
