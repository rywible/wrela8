use std::io::Read;
use std::time::Instant;

use wrela_machine::report::RequestRing;

use crate::VmmError;
use crate::devices;
#[cfg(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux")))]
use crate::guest_dram_offset;
use crate::guest_memory::GuestMemoryHandle;
use crate::record;

pub(crate) fn host_entropy(buf: &mut [u8]) -> Result<(), VmmError> {
    let mut f = std::fs::File::open("/dev/urandom")
        .map_err(|e| VmmError::Io(format!("open /dev/urandom: {e}")))?;
    f.read_exact(buf)
        .map_err(|e| VmmError::Io(format!("read /dev/urandom: {e}")))?;
    Ok(())
}

pub(crate) fn fill_entropy_into_dram(
    host_ram: *mut u8,
    dest: u64,
    bytes: &[u8],
) -> Result<(), VmmError> {
    use wrela_machine::layout as machine_layout;
    let len = bytes.len() as u64;
    let end = dest.checked_add(len).ok_or_else(|| {
        VmmError::GuestFault(format!(
            "entropy fill destination {dest:#x}+{len} overflows a u64"
        ))
    })?;
    let dram_base = machine_layout::DRAM_BASE;
    let dram_end = dram_base + machine_layout::DRAM_SIZE;
    if dest < dram_base || end > dram_end {
        return Err(VmmError::GuestFault(format!(
            "entropy fill destination [{dest:#x}..{end:#x}) is outside guest DRAM \
             [{dram_base:#x}..{dram_end:#x})"
        )));
    }
    let off = (dest - dram_base) as usize;
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), host_ram.add(off), bytes.len());
    }
    Ok(())
}

pub(crate) fn apply_entropy_read(
    chooser: &mut record::Chooser,
    host_ram: *mut u8,
    dest: u64,
    len: u64,
) -> Result<(), VmmError> {
    use wrela_machine::layout as machine_layout;
    use wrela_machine::machine_info;

    if len == 0 || len > machine_info::ENTROPY_LEN_MAX {
        return Err(VmmError::GuestFault(format!(
            "entropy length {len} is not in 1..={}",
            machine_info::ENTROPY_LEN_MAX
        )));
    }
    let end = dest.checked_add(len).ok_or_else(|| {
        VmmError::GuestFault(format!(
            "entropy fill destination {dest:#x}+{len} overflows a u64"
        ))
    })?;
    let dram_base = machine_layout::DRAM_BASE;
    let dram_end = dram_base + machine_layout::DRAM_SIZE;
    if dest < dram_base || end > dram_end {
        return Err(VmmError::GuestFault(format!(
            "entropy fill destination [{dest:#x}..{end:#x}) is outside guest DRAM \
             [{dram_base:#x}..{dram_end:#x})"
        )));
    }

    let entry = if chooser.is_recording() {
        let mut buf = vec![0u8; len as usize];
        host_entropy(&mut buf)?;
        chooser.choose_checked(record::ChoiceRequest::EntropyRead { len }, move || {
            record::ChoiceEntry::EntropyRead { bytes: buf }
        })?
    } else {
        chooser.choose_checked(record::ChoiceRequest::EntropyRead { len }, || {
            unreachable!("Chooser::choose_next never invokes `live` under replay")
        })?
    };
    let record::ChoiceEntry::EntropyRead { bytes } = entry else {
        unreachable!(
            "choose_checked(EntropyRead, ..) always returns an EntropyRead-shaped entry \
             (a mismatched replay tag falls back to the request's own shape)"
        )
    };
    if bytes.len() as u64 != len {
        return Err(VmmError::GuestFault(format!(
            "entropy choice returned {} byte(s), guest asked for {len}",
            bytes.len()
        )));
    }
    fill_entropy_into_dram(host_ram, dest, &bytes)
}

pub(crate) fn read_core_mark(host_ram: *const u8, core: usize) -> u64 {
    use wrela_machine::layout as machine_layout;
    let off =
        (wrela_machine::machine_info::core_mark_addr(core) - machine_layout::DRAM_BASE) as usize;
    let mut b = [0u8; 8];
    unsafe { std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 8) };
    u64::from_le_bytes(b)
}

pub(crate) fn check_core_marks(host_ram: *const u8, cores: usize) -> Result<(), VmmError> {
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

pub(crate) struct BlkState {
    pub(crate) device: devices::BlkDevice,
    pub(crate) mem: devices::GuestMem,
    pub(crate) irq_status_gpa: Option<u64>,
}

pub(crate) fn check_vector_in_range(vector: u64) -> Result<(), VmmError> {
    if vector >= 64 {
        return Err(VmmError::BadImage(format!(
            "vector={vector} is out of range (pending word has 64 bits; `& 63` would alias)"
        )));
    }
    Ok(())
}

pub(crate) fn raise_vector(memory: GuestMemoryHandle, vector: u64) -> Result<(), VmmError> {
    check_vector_in_range(vector)?;
    memory.fetch_or_u64(
        wrela_machine::pending::core_word_addr(0),
        1u64 << vector,
        std::sync::atomic::Ordering::Release,
        "pending word",
    )?;
    Ok(())
}

pub(crate) fn service_blk(
    blk: &mut Option<BlkState>,
    chooser: &mut record::Chooser,
    memory: GuestMemoryHandle,
) -> Result<bool, VmmError> {
    let Some(state) = blk.as_mut() else {
        return Ok(false);
    };
    let completions = state
        .device
        .service(&mut state.mem)
        .map_err(|fault| VmmError::GuestFault(format!("virtio-blk: {fault}")))?;
    commit_completions(state, chooser, &completions, memory)
}

pub(crate) fn commit_completions(
    state: &mut BlkState,
    chooser: &mut record::Chooser,
    completions: &[devices::Completion],
    memory: GuestMemoryHandle,
) -> Result<bool, VmmError> {
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
            chooser.choose_checked(request, move || observed)?
        };
        if chosen != observed {
            chooser.note_divergence_checked(record::Divergence::DeviceCompletionMismatch {
                index,
                recorded: chosen.to_text_fields(),
                actual: observed.to_text_fields(),
            })?;
        }
        let len = match &chosen {
            record::ChoiceEntry::DeviceCompletion { len, .. } => *len,
            _ => c.len,
        };
        state
            .device
            .commit_used(&mut state.mem, c.head, len)
            .map_err(|fault| VmmError::GuestFault(format!("virtio-blk: {fault}")))?;
    }
    if let Some(vector) = state.device.config.vector {
        if let Some(gpa) = state.irq_status_gpa {
            memory.fetch_or_u32(
                gpa,
                1,
                std::sync::atomic::Ordering::Release,
                "interrupt_status",
            )?;
        }
        chooser.choose_checked(record::ChoiceRequest::VectorRaise { vector }, || {
            record::ChoiceEntry::VectorRaise { vector }
        })?;
        raise_vector(memory, vector)?;
    }
    Ok(true)
}

#[derive(Debug, Default)]
pub(crate) struct AdmissionWitness {
    rings: Vec<RequestRing>,
    last_count: Vec<u64>,
    last_head: Vec<u64>,
}

impl AdmissionWitness {
    pub(crate) fn new(rings: Vec<RequestRing>) -> AdmissionWitness {
        let n = rings.len();
        AdmissionWitness {
            rings,
            last_count: vec![0; n],
            last_head: vec![0; n],
        }
    }

    pub(crate) fn observe(
        &mut self,
        counts: &[u64],
        heads: &[u64],
        core: usize,
    ) -> Result<Vec<(String, String)>, String> {
        if counts.len() != self.rings.len() || heads.len() != self.rings.len() {
            return Err(format!(
                "admission witness: {} ring(s) declared but {} count word(s) and {} head word(s) \
                 were read",
                self.rings.len(),
                counts.len(),
                heads.len()
            ));
        }
        let mut admitted = Vec::new();
        for (i, ring) in self.rings.iter().enumerate() {
            let now_c = counts[i];
            let was_c = self.last_count[i];
            let now_h = heads[i];
            let was_h = self.last_head[i];
            if ring.dst == core {
                let cap = ring.capacity.max(1);
                let n = if ring.capacity > 1 {
                    let now_r = now_h % cap;
                    let was_r = was_h % cap;
                    let mut d = if now_r >= was_r {
                        now_r - was_r
                    } else {
                        cap - (was_r - now_r)
                    };
                    if d == 0 && was_c > now_c && now_h == was_h {
                        d = was_c - now_c;
                    }
                    d
                } else if now_c < was_c {
                    was_c - now_c
                } else {
                    0
                };
                let n = n.min(cap);
                for _ in 0..n {
                    admitted.push((ring.target.clone(), format!("core{}", ring.src)));
                }
                self.last_head[i] = now_h;
            }
            self.last_count[i] = now_c;
        }
        Ok(admitted)
    }
}

#[cfg(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux")))]
pub(crate) fn observe_admissions(
    witness: &mut AdmissionWitness,
    host_ram: *const u8,
    core: usize,
) -> Result<Vec<(String, String)>, VmmError> {
    if witness.rings.is_empty() {
        return Ok(Vec::new());
    }
    let mut counts = Vec::with_capacity(witness.rings.len());
    let mut heads = Vec::with_capacity(witness.rings.len());
    for r in &witness.rings {
        let off = guest_dram_offset(r.count_addr, 8, "admission count_addr")?;
        let mut b = [0u8; 8];
        unsafe { std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 8) };
        counts.push(u64::from_le_bytes(b));
        let head_addr = r.count_addr.saturating_sub(16);
        let hoff = guest_dram_offset(head_addr, 8, "admission head_addr")?;
        let mut hb = [0u8; 8];
        unsafe { std::ptr::copy_nonoverlapping(host_ram.add(hoff), hb.as_mut_ptr(), 8) };
        heads.push(u64::from_le_bytes(hb));
    }
    witness
        .observe(&counts, &heads, core)
        .map_err(VmmError::GuestFault)
}

#[cfg(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux")))]
pub(crate) fn commit_admissions(
    chooser: &mut record::Chooser,
    admitted: &[(String, String)],
) -> Result<(), VmmError> {
    for (mailbox, sender) in admitted {
        let request = record::ChoiceRequest::Admission {
            mailbox: mailbox.clone(),
            sender: sender.clone(),
        };
        let observed = request.fallback();
        let index = chooser.resolved_count();
        let chosen = {
            let observed = observed.clone();
            chooser.choose_checked(request, move || observed)?
        };
        if chosen != observed {
            chooser.note_divergence_checked(record::Divergence::AdmissionMismatch {
                index,
                recorded: chosen.to_text_fields(),
                actual: observed.to_text_fields(),
            })?;
        }
    }
    Ok(())
}

pub(crate) fn monotonic_ns() -> u64 {
    use std::sync::OnceLock;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    (epoch.elapsed().as_nanos() as u64).max(1)
}

pub(crate) fn drain_console(host_ram: *const u8) -> Vec<u8> {
    use wrela_machine::console;
    use wrela_machine::layout as machine_layout;

    let data_base = console::DATA_BASE;
    let data_end = console::DATA_BASE + console::DATA_SIZE;
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
        if addr < data_base || addr >= data_end {
            continue;
        }
        let max_len = data_end - addr;
        let clamped_len = len.min(max_len) as usize;
        if clamped_len == 0 {
            continue;
        }
        let src_off = (addr - machine_layout::DRAM_BASE) as usize;
        let mut buf = vec![0u8; clamped_len];
        unsafe {
            std::ptr::copy_nonoverlapping(host_ram.add(src_off), buf.as_mut_ptr(), clamped_len);
        }
        out.extend_from_slice(&buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wrela_machine::layout as machine_layout;
    use wrela_machine::machine_info;

    #[test]
    fn host_entropy_fills_buffer_from_urandom() {
        let mut a = [0xAAu8; 32];
        let mut b = [0xAAu8; 32];
        host_entropy(&mut a).expect("urandom open/read");
        host_entropy(&mut b).expect("urandom open/read");
        assert_ne!(a, [0xAAu8; 32], "host_entropy must overwrite the buffer");
        assert_ne!(a, b, "two live draws should disagree");
    }

    #[test]
    fn apply_entropy_read_records_and_replays_into_dram() {
        let dest = machine_layout::DRAM_BASE + 0x400;
        let len = 8u64;
        let mut ram = vec![0u8; 0x800];

        let mut recorder = record::Chooser::recorder();
        apply_entropy_read(&mut recorder, ram.as_mut_ptr(), dest, len).expect("record fill");
        let off = (dest - machine_layout::DRAM_BASE) as usize;
        let recorded = ram[off..off + len as usize].to_vec();
        assert_ne!(
            recorded,
            vec![0u8; len as usize],
            "live fill must not be silent zeros"
        );

        let (choices, divergences) = record::finish_chooser(recorder).expect("finish");
        assert!(divergences.is_empty());
        assert_eq!(choices.len(), 1);
        assert_eq!(
            choices[0],
            record::ChoiceEntry::EntropyRead {
                bytes: recorded.clone()
            }
        );

        let mut ram2 = vec![0u8; 0x800];
        let mut replayer = record::Chooser::replayer(choices);
        apply_entropy_read(&mut replayer, ram2.as_mut_ptr(), dest, len).expect("replay fill");
        assert_eq!(&ram2[off..off + len as usize], recorded.as_slice());
        let (_, divergences) = record::finish_chooser(replayer).expect("finish");
        assert!(divergences.is_empty());
    }

    #[test]
    fn apply_entropy_read_refuses_bad_len_and_oob_dest() {
        let mut ram = vec![0u8; 0x1000];
        let mut c = record::Chooser::recorder();
        let err = apply_entropy_read(&mut c, ram.as_mut_ptr(), machine_layout::DRAM_BASE, 0)
            .expect_err("len 0");
        assert!(
            matches!(&err, VmmError::GuestFault(m) if m.contains("entropy length")),
            "got {err:?}"
        );
        let err = apply_entropy_read(
            &mut c,
            ram.as_mut_ptr(),
            machine_layout::DRAM_BASE,
            machine_info::ENTROPY_LEN_MAX + 1,
        )
        .expect_err("len too large");
        assert!(matches!(&err, VmmError::GuestFault(_)), "got {err:?}");
        let err = apply_entropy_read(&mut c, ram.as_mut_ptr(), machine_layout::DRAM_BASE - 1, 8)
            .expect_err("below DRAM");
        assert!(
            matches!(&err, VmmError::GuestFault(m) if m.contains("outside guest DRAM")),
            "got {err:?}"
        );
        assert_eq!(
            c.resolved_count(),
            0,
            "a refused request must not burn a choice"
        );
    }

    #[test]
    fn fill_entropy_into_dram_copies_bytes() {
        let dest = machine_layout::DRAM_BASE + 0x10;
        let mut ram = vec![0u8; 0x40];
        fill_entropy_into_dram(ram.as_mut_ptr(), dest, &[0xde, 0xad, 0xbe, 0xef])
            .expect("in-range");
        assert_eq!(&ram[0x10..0x14], &[0xde, 0xad, 0xbe, 0xef]);
    }
}
