//! Exit-loop helpers for the HVF boot path: console drain, vector raise,
//! blk service/completions, admission witness, and core-mark checks.
//! Called from `boot::boot_image_core`'s vCPU loop.

use std::time::Instant;

use wrela_machine::report::RequestRing;

use crate::devices;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::hv;
use crate::record;
use crate::{VmmError, guest_dram_offset};

/// Core `core`'s own guest-written bring-up mark (plans/M8.md item C1,
/// `machine_info::OFF_CORE_MARK`).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn read_core_mark(host_ram: *const u8, core: usize) -> u64 {
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

/// plans/M7.md item F: one declared `blk` device model plus the checked
/// view of guest memory it is allowed to touch. A pair rather than one
/// type because `devices::GuestMem` is deliberately the *only* thing in
/// this VMM holding both a raw DRAM pointer and the declared pool windows
/// (`devices.rs`'s own module doc: decision 5's security boundary is
/// enforced by there being no other way to turn a guest address into a
/// host offset).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) struct BlkState {
    pub(crate) device: devices::BlkDevice,
    pub(crate) mem: devices::GuestMem,
    /// Guest GPA of `interrupt_status` (devregs + 0x60), when the image
    /// declared a vector and bound an ISR. `None` for poll builds.
    pub(crate) irq_status_gpa: Option<u64>,
}

/// 06 §4's raise, both producers' one implementation: set bit `vector` in
/// core 0's own pending word. **An OR, never a store**, since M7 gives
/// this machine a second raiser (a `blk` completion) alongside M6's
/// deadline service — a plain store of `1` would silently drop a
/// completion vector raised moments earlier in the same park.
/// Pending-word vectors are bits `0..63`. Masking with `& 63` used to
/// silently alias high values onto low bits — refuse instead.
pub(crate) fn check_vector_in_range(vector: u64) -> Result<(), VmmError> {
    if vector >= 64 {
        return Err(VmmError::BadImage(format!(
            "vector={vector} is out of range (pending word has 64 bits; `& 63` would alias)"
        )));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn raise_vector(host_ram: *mut u8, vector: u64) -> Result<(), VmmError> {
    check_vector_in_range(vector)?;
    let off = guest_dram_offset(wrela_machine::pending::core_word_addr(0), 8, "pending word")?;
    unsafe {
        let mut b = [0u8; 8];
        std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 8);
        let raised = u64::from_le_bytes(b) | (1u64 << vector);
        std::ptr::copy_nonoverlapping(raised.to_le_bytes().as_ptr(), host_ram.add(off), 8);
    }
    Ok(())
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
pub(crate) fn service_blk(
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
pub(crate) fn commit_completions(
    state: &mut BlkState,
    chooser: &mut record::Chooser,
    completions: &[devices::Completion],
    host_ram: *mut u8,
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
            let off = guest_dram_offset(gpa, 4, "interrupt_status")?;
            unsafe {
                let mut b = [0u8; 4];
                std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 4);
                let status = u32::from_le_bytes(b) | 1;
                std::ptr::copy_nonoverlapping(status.to_le_bytes().as_ptr(), host_ram.add(off), 4);
            }
        }
        chooser.choose_checked(record::ChoiceRequest::VectorRaise { vector }, || {
            record::ChoiceEntry::VectorRaise { vector }
        })?;
        raise_vector(host_ram, vector)?;
    }
    Ok(true)
}

/// plans/M8.md item C3, decision 42 — the recorder's witness on 06 §8's
/// "per-mailbox cross-core admission order", which is the one scheduling
/// nondeterminism 04 §2 gives this machine.
///
/// Under plans/M15.md item I, concurrent record may overlap producers and
/// consumers, so occupancy Δcount alone can miss a produce+consume pair
/// that nets to zero between exits. The ring's **head** only advances on
/// drain, so head deltas are the concurrent-safe admission count (with
/// `capacity` for wrap). Cap-1 rings keep head at 0 always; for those we
/// still fall back to count shrinks on the consumer's exit (and accept
/// that a fully overlapped produce+consume can under-count — Progress
/// replay re-checks under serial hand-off).
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

    /// Pure observation rule (unit-tested via `admission_witness_*`).
    pub(crate) fn observe(
        &mut self,
        counts: &[u64],
        heads: &[u64],
        core: usize,
    ) -> Result<Vec<(String, String)>, String> {
        debug_assert_eq!(counts.len(), self.rings.len());
        debug_assert_eq!(heads.len(), self.rings.len());
        let mut admitted = Vec::new();
        for (i, ring) in self.rings.iter().enumerate() {
            let now_c = counts[i];
            let was_c = self.last_count[i];
            let now_h = heads[i];
            let was_h = self.last_head[i];
            if ring.dst == core {
                let n = if ring.capacity > 1 {
                    let cap = ring.capacity;
                    let mut d = (now_h + cap - was_h) % cap;
                    // Full-wrap of exactly `cap` leaves head unchanged.
                    if d == 0 && was_c > now_c && now_h == was_h {
                        d = was_c - now_c;
                        if d > cap {
                            d = cap;
                        }
                    }
                    d
                } else if now_c < was_c {
                    was_c - now_c
                } else {
                    0
                };
                for _ in 0..n {
                    admitted.push((ring.target.clone(), format!("core{}", ring.src)));
                }
                // Head only moves on drain — update it only on the consumer
                // so an unrelated exit cannot steal a concurrent head advance.
                self.last_head[i] = now_h;
            }
            self.last_count[i] = now_c;
        }
        Ok(admitted)
    }

    /// Align witness baselines to the live ring words without emitting
    /// Admissions — used when ending the bring-up overlap window so entry
    /// parks that ran un-witnessed do not create a false head delta later.
    pub(crate) fn sync_baselines(&mut self, counts: &[u64], heads: &[u64]) {
        debug_assert_eq!(counts.len(), self.rings.len());
        debug_assert_eq!(heads.len(), self.rings.len());
        self.last_count.clear();
        self.last_count.extend_from_slice(counts);
        self.last_head.clear();
        self.last_head.extend_from_slice(heads);
    }
}

/// Reads every request ring's occupancy word and head out of guest memory
/// and returns one `(mailbox, sender)` per message core `core`'s drain
/// just admitted. Callers buffer these until the core's next Yield so the
/// choice-log interleaving matches Progress-serialized replay
/// (plans/M15.md item I).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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

/// Snap witness baselines to the live ring without emitting Admissions.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn sync_admission_baselines(
    witness: &mut AdmissionWitness,
    host_ram: *const u8,
) -> Result<(), VmmError> {
    if witness.rings.is_empty() {
        return Ok(());
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
    witness.sync_baselines(&counts, &heads);
    Ok(())
}

/// Appends buffered `(mailbox, sender)` pairs as `Admission` choices and
/// checks them under replay.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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

/// Reads the vCPU's own current `PC` for a fault diagnostic — best-effort
/// (`Ok(0)` is never returned; a real HVF failure here still surfaces as
/// `None` to the caller, which substitutes `0` rather than compounding
/// one failure with another).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn read_pc(vcpu: u64) -> Option<u64> {
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
pub(crate) fn el1_exception_note(vcpu: u64, pc: u64) -> String {
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
pub(crate) fn advance_pc(vcpu: u64) -> Result<(), VmmError> {
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
pub(crate) fn monotonic_ns() -> u64 {
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
pub(crate) fn drain_console(host_ram: *const u8) -> Vec<u8> {
    use wrela_machine::console;
    use wrela_machine::layout as machine_layout;

    // Only the console data window is transcript material — never the rest
    // of guest DRAM (a forged descriptor must not leak arbitrary guest
    // memory into the host-side transcript / choice-log digest).
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
