use std::path::Path;
use std::time::Duration;

use wrela_machine::report::{CoreEntry, CoreStack, ParsedReport};

use crate::devices;
use crate::exit_loop::{
    AdmissionWitness, BlkState, apply_entropy_read, check_core_marks, check_vector_in_range,
    commit_admissions, commit_completions, drain_console, monotonic_ns, observe_admissions,
    raise_vector, read_core_mark, service_blk,
};
use crate::host::{
    DiagnosticRegisters, HostBackend, HostExit, HostMemoryRegion, HostVcpu, HostVm,
    MemoryProtection, MmioAccess, MmioCompletion, MmioDirection, VcpuBootState, VcpuHandle,
};
use crate::record;
use crate::{
    BootOutcome, VmmError, capped_park_deadline_ns, guest_dram_offset, parse_report,
    validate_report_digests,
};

const CORE_SLOTS: usize = wrela_machine::CORE_SLOTS;

#[cfg(all(test, target_os = "linux", target_arch = "aarch64"))]
thread_local! {
    static TEST_WATCHDOG_CAP: std::cell::Cell<Option<Duration>> = const {
        std::cell::Cell::new(None)
    };
}

#[cfg(all(test, target_os = "linux", target_arch = "aarch64"))]
pub(crate) struct WatchdogCapGuard(Option<Duration>);

#[cfg(all(test, target_os = "linux", target_arch = "aarch64"))]
impl Drop for WatchdogCapGuard {
    fn drop(&mut self) {
        TEST_WATCHDOG_CAP.set(self.0);
    }
}

#[cfg(all(test, target_os = "linux", target_arch = "aarch64"))]
pub(crate) fn watchdog_cap_for_test(cap: Duration) -> WatchdogCapGuard {
    let previous = TEST_WATCHDOG_CAP.replace(Some(cap));
    WatchdogCapGuard(previous)
}

pub(crate) fn core_sp_tops_from_report(parsed: &ParsedReport) -> [u64; CORE_SLOTS] {
    use wrela_machine::layout as machine_layout;
    let mut tops = [0u64; CORE_SLOTS];
    if parsed.core_stacks.is_empty() {
        for c in 0..parsed.cores {
            tops[c] = machine_layout::core_stack_base_n(c, parsed.cores)
                + machine_layout::CORE_STACK_SIZE;
        }
    } else {
        for CoreStack { core, base, size } in &parsed.core_stacks {
            if *core < CORE_SLOTS {
                tops[*core] = base + size;
            }
        }
    }
    tops
}

#[cfg(test)]
pub(crate) mod create_inject {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static FAIL_AT: AtomicUsize = AtomicUsize::new(usize::MAX);

    pub fn arm(core: usize) {
        FAIL_AT.store(core, Ordering::SeqCst);
    }

    pub fn clear() {
        FAIL_AT.store(usize::MAX, Ordering::SeqCst);
    }

    pub fn should_fail(core: usize) -> bool {
        FAIL_AT.load(Ordering::SeqCst) == core
    }

    pub struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            clear();
        }
    }
}

pub(crate) fn boot_image<H: HostBackend>(
    report_path: &Path,
    img_path: &Path,
    replay_choices: Option<Vec<record::ChoiceEntry>>,
    display_selection: crate::display::DisplayBackendSelection,
    test_delayed_raise: Option<(Duration, u64)>,
    translation_mode: crate::boot::TranslationMode,
    guest_counters: bool,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    use wrela_machine::layout as machine_layout;
    use wrela_machine::machine_info;

    let report_text = std::fs::read_to_string(report_path)
        .map_err(|e| VmmError::Io(format!("read {}: {e}", report_path.display())))?;
    let parsed = parse_report(&report_text)?;
    let img = std::fs::read(img_path)
        .map_err(|e| VmmError::Io(format!("read {}: {e}", img_path.display())))?;
    validate_report_digests(&parsed, &img)?;
    let host_profile = crate::hardening::HostProfile::selected()?;
    if translation_mode == crate::boot::TranslationMode::DiagnosticMmuOff
        && host_profile == crate::hardening::HostProfile::Product
    {
        return Err(VmmError::HostProfile(
            "the diagnostic MMU-off fixture is never product-conforming".into(),
        ));
    }
    host_profile.validate_process()?;
    crate::hardening::pin_host(host_profile)?;
    let translation_boot = match translation_mode {
        crate::boot::TranslationMode::SealedStage1 => {
            let stage1_report = parsed.stage1.as_ref().ok_or_else(|| {
                VmmError::BadImage(
                    "ordinary sealed-image execution requires the compiler-owned Stage1 contract"
                        .into(),
                )
            })?;
            crate::host::TranslationBootState::Enabled(crate::host::Stage1BootState {
                ttbr0_el1: stage1_report.ttbr0_el1,
                tcr_el1: stage1_report.tcr_el1,
                mair_el1: stage1_report.mair_el1,
                sctlr_el1: stage1_report.sctlr_el1,
                vbar_el1: stage1_report.vbar_el1,
            })
        }
        crate::boot::TranslationMode::DiagnosticMmuOff => {
            crate::host::TranslationBootState::DisabledDiagnostic
        }
    };

    let image_off = machine_layout::IMAGE_BASE - machine_layout::DRAM_BASE;
    if image_off + (img.len() as u64) > machine_layout::DRAM_SIZE {
        return Err(VmmError::BadImage(format!(
            "image ({} bytes at offset {:#x}) does not fit the {} byte DRAM reservation",
            img.len(),
            image_off,
            machine_layout::DRAM_SIZE
        )));
    }

    let capabilities = H::capabilities()?;
    capabilities.validate(parsed.cores)?;
    const MACHINE_IPA_BITS: u8 = 40;
    if capabilities.ipa_bits < MACHINE_IPA_BITS {
        return Err(crate::host::host_error(
            capabilities.backend,
            "validate machine IPA width",
            0,
            format!(
                "the Wrela machine requires {MACHINE_IPA_BITS} IPA bits, host exposes {}",
                capabilities.ipa_bits
            ),
        ));
    }
    let mut guest_memory = crate::guest_memory::GuestMemory::allocate(
        capabilities.page_size,
        host_profile == crate::hardening::HostProfile::Product,
    )?;
    let host_ram = guest_memory.as_ptr();
    let memory_handle = crate::guest_memory::GuestMemoryHandle::from_memory(&guest_memory);
    guest_memory.initialize(machine_layout::IMAGE_BASE, &img, "sealed image")?;
    let regions = parsed
        .protections
        .iter()
        .map(|range| {
            let protection = match range.class.as_str() {
                "normal-ro-rx" => MemoryProtection::ReadExecute,
                "normal-rw-nx" | "device-rw-nx" => MemoryProtection::ReadWrite,
                "normal-ro-nx" | "invalid" => MemoryProtection::ReadOnly,
                other => {
                    return Err(VmmError::BadImage(format!(
                        "unknown sealed protection class `{other}`"
                    )));
                }
            };
            let len = usize::try_from(range.size).map_err(|_| {
                VmmError::BadImage(format!("protection size {} does not fit usize", range.size))
            })?;
            Ok(HostMemoryRegion {
                gpa: range.base,
                len,
                protection,
            })
        })
        .collect::<Result<Vec<_>, VmmError>>()?;
    let info_off = (machine_layout::MACHINE_INFO_BASE - machine_layout::DRAM_BASE) as usize;
    unsafe {
        let rev_bytes = wrela_machine::MACHINE_REVISION_STR.as_bytes();
        std::ptr::copy_nonoverlapping(
            rev_bytes.as_ptr(),
            host_ram.add(info_off + machine_info::OFF_REVISION as usize),
            rev_bytes.len(),
        );
        std::ptr::write_bytes(
            host_ram.add(info_off + machine_info::OFF_WALL_SEED as usize),
            0,
            8,
        );
    }

    // Declared after GuestMemory so every vCPU and the VM are destroyed before
    // the backing reservation is released.
    let vm = H::create_vm(&guest_memory, MACHINE_IPA_BITS, &regions)?;

    let _ = monotonic_ns();
    let sp_tops = core_sp_tops_from_report(&parsed);
    let blk: Option<BlkState> = match parsed.blk {
        None => None,
        Some(cfg) => {
            let pools = cfg.pools.clone();
            let vector = cfg.vector;
            let device_index = cfg.device;
            let device = devices::BlkDevice::new(cfg).map_err(VmmError::BadImage)?;
            let mem = devices::GuestMem::from_memory(memory_handle, pools, device_index)
                .map_err(VmmError::BadImage)?;
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
    for inj in &parsed.irq_injects {
        let guest = inj.base.checked_add(inj.offset).ok_or_else(|| {
            VmmError::BadImage(format!(
                "IrqHostInject base={:#x}+offset={:#x} overflows",
                inj.base, inj.offset
            ))
        })?;
        check_vector_in_range(inj.vector)?;
        memory_handle.fetch_or_u32(
            guest,
            inj.status,
            std::sync::atomic::Ordering::Release,
            "IrqHostInject status",
        )?;
        raise_vector(memory_handle, inj.vector)?;
    }

    struct SendPtr(*mut u8);
    unsafe impl Send for SendPtr {}
    struct RaiseGuard(Option<std::thread::JoinHandle<()>>);
    impl Drop for RaiseGuard {
        fn drop(&mut self) {
            if let Some(h) = self.0.take() {
                let _ = h.join();
            }
        }
    }
    let _raise_guard = RaiseGuard(test_delayed_raise.map(|(delay, vector_bit)| {
        let memory = memory_handle;
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            memory
                .fetch_or_u64(
                    wrela_machine::pending::core_word_addr(0),
                    vector_bit,
                    std::sync::atomic::Ordering::Release,
                    "delayed pending raise",
                )
                .expect("validated delayed pending-word raise");
        })
    }));

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum CoreState {
        Unreleased,
        Runnable,
        Parked,
        Finished,
    }

    struct Sched {
        current: usize,
        state: [CoreState; CORE_SLOTS],
        done: bool,
    }

    struct Shared<V: VcpuHandle> {
        sched: Sched,
        chooser: record::Chooser,
        blk: Option<BlkState>,
        exits: u64,
        exit_code: Option<u64>,
        error: Option<VmmError>,
        vcpus: Vec<Option<V>>,
        released: bool,
        admission: AdmissionWitness,
        admission_buf: [Vec<(String, String)>; CORE_SLOTS],
        display: crate::display::RuntimeDisplay,
        input: crate::input::InputDevice,
    }

    fn pending_word(memory: crate::guest_memory::GuestMemoryHandle, core: usize) -> u64 {
        memory
            .load_u64(
                wrela_machine::pending::core_word_addr(core),
                std::sync::atomic::Ordering::Acquire,
                "pending word",
            )
            .expect("pending words are sealed aligned DRAM addresses")
    }

    fn next_core(
        sched: &mut Sched,
        from: usize,
        memory: crate::guest_memory::GuestMemoryHandle,
        cores_declared: usize,
    ) -> Option<usize> {
        for step in 1..=cores_declared {
            let c = (from + step) % cores_declared;
            if sched.state[c] == CoreState::Parked && pending_word(memory, c) != 0 {
                sched.state[c] = CoreState::Runnable;
                return Some(c);
            }
        }
        for step in 1..=cores_declared {
            let c = (from + step) % cores_declared;
            if sched.state[c] == CoreState::Runnable {
                return Some(c);
            }
        }
        None
    }

    enum Step {
        Keep,
        Yield { release: bool, park: bool },
        Halt(u64),
    }

    /// Record the first failure and stop every core. Kept in one place because
    /// a partially applied version of this transition — an error without
    /// `done`, or `done` without marking this core `Finished` — either hangs
    /// the scoped threads or loses the diagnosis.
    fn mark_failed<V: VcpuHandle>(shared: &mut Shared<V>, core: usize, error: VmmError) {
        shared.error.get_or_insert(error);
        shared.sched.done = true;
        shared.sched.state[core] = CoreState::Finished;
    }

    /// `mark_failed` for the callers that do not already hold the lock.
    fn fail_core<V: VcpuHandle>(
        lock: &std::sync::Mutex<Shared<V>>,
        wake: &std::sync::Condvar,
        core: usize,
        error: VmmError,
    ) {
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        mark_failed(&mut guard, core, error);
        drop(guard);
        wake.notify_all();
    }

    let cores_declared = parsed.cores;
    if !(1..=CORE_SLOTS).contains(&cores_declared) {
        return Err(VmmError::MalformedReport(format!(
            "`Cores count={cores_declared}` must satisfy 1..=CORE_SLOTS ({CORE_SLOTS})"
        )));
    }
    let display = crate::display::runtime_display(display_selection)?;
    let input = crate::input::InputDevice::from_process(replay_choices.is_some())
        .map_err(VmmError::GuestFault)?;
    let shared = std::sync::Mutex::new(Shared {
        sched: Sched {
            current: 0,
            state: {
                let mut s = [CoreState::Unreleased; CORE_SLOTS];
                s[0] = CoreState::Runnable;
                s
            },
            done: false,
        },
        released: false,
        chooser: match replay_choices {
            Some(log) => record::Chooser::replayer(log).strict(),
            None => record::Chooser::recorder(),
        },
        blk,
        exits: 0,
        exit_code: None,
        error: None,
        vcpus: vec![None; CORE_SLOTS],
        admission: AdmissionWitness::new(parsed.request_rings.clone()),
        admission_buf: std::array::from_fn(|_| Vec::new()),
        display,
        input,
    });
    let wake = std::sync::Condvar::new();

    fn sleep_until_park_deadline<V: VcpuHandle>(
        lock: &std::sync::Mutex<Shared<V>>,
        wake: &std::sync::Condvar,
        deadline_ns: u64,
    ) {
        const SLICE: Duration = Duration::from_millis(100);
        loop {
            let g = lock.lock().unwrap_or_else(|e| e.into_inner());
            if g.sched.done {
                return;
            }
            let now = monotonic_ns();
            let capped = capped_park_deadline_ns(now, deadline_ns);
            if now >= capped {
                return;
            }
            let remaining = Duration::from_nanos(capped - now);
            let slice = if remaining < SLICE { remaining } else { SLICE };
            let (g2, _) = wake
                .wait_timeout(g, slice)
                .unwrap_or_else(|e| e.into_inner());
            drop(g2);
        }
    }

    fn mmio_access(
        access: &MmioAccess,
        core: usize,
        what: &str,
        protocol: &str,
        want_write: bool,
    ) -> Result<(), VmmError> {
        if access.width != 8 {
            return Err(VmmError::GuestFault(format!(
                "core {core}: {what} requires an 8-byte access, got a {}-byte access",
                access.width
            )));
        }
        if want_write && access.direction != MmioDirection::Write {
            return Err(VmmError::GuestFault(format!(
                "core {core}: a load from {what} is not part of the {protocol} protocol"
            )));
        }
        if !want_write && access.direction != MmioDirection::Read {
            return Err(VmmError::GuestFault(format!(
                "core {core}: a store to {what} is not part of the {protocol} protocol"
            )));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_exit<V: HostVcpu>(
        core: usize,
        vcpu: &mut V,
        exit: HostExit,
        host_ram: *mut u8,
        memory_handle: crate::guest_memory::GuestMemoryHandle,
        cores_declared: usize,
        lock: &std::sync::Mutex<Shared<V::Handle>>,
        wake: &std::sync::Condvar,
        wall_cap: Duration,
    ) -> Result<Step, VmmError> {
        use wrela_machine::mmio;
        let deadline_off = (machine_layout::MACHINE_INFO_BASE - machine_layout::DRAM_BASE) as usize
            + wrela_machine::machine_info::OFF_NEXT_DEADLINE as usize;
        match exit {
            HostExit::Mmio(access) => {
                let ipa = access.address;
                if ipa == mmio::FAULT_MMIO_ADDR {
                    mmio_access(&access, core, "FAULT_MMIO_ADDR", "stage1 fault", true)?;
                    vcpu.complete_mmio(access.token, MmioCompletion::WriteAccepted)?;
                    let record = wrela_machine::layout::STAGE1_FAULT_BASE
                        + wrela_machine::stage1::COMMON_PROTECTION_GRANULE;
                    let offset = guest_dram_offset(record, 32, "stage1 fault record")?;
                    let mut words = [0_u64; 4];
                    for (index, word) in words.iter_mut().enumerate() {
                        let mut bytes = [0_u8; 8];
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                host_ram.add(offset + index * 8),
                                bytes.as_mut_ptr(),
                                8,
                            )
                        };
                        *word = u64::from_le_bytes(bytes);
                    }
                    // Classify from the recorded ESR. The guest vector catches
                    // every synchronous exception, not only protection faults,
                    // so a fixed label here would report an unmapped-page bug
                    // or a guest BRK as a successful W^X enforcement.
                    let class = wrela_machine::fault::class(words[0]);
                    Err(VmmError::GuestFault(format!(
                        "core {core}: {class} esr={:#x} far={:#x} elr={:#x} spsr={:#x}",
                        words[0], words[1], words[2], words[3]
                    )))
                } else if ipa == mmio::EXIT_MMIO_ADDR {
                    mmio_access(&access, core, "EXIT_MMIO_ADDR", "exit", true)?;
                    let value = access.write_value.unwrap_or(0);
                    vcpu.complete_mmio(access.token, MmioCompletion::WriteAccepted)?;
                    Ok(Step::Halt(value))
                } else if ipa == mmio::CLOCK_MMIO_ADDR {
                    mmio_access(&access, core, "CLOCK_MMIO_ADDR", "clock", false)?;
                    let entry = {
                        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        g.chooser
                            .choose_checked(record::ChoiceRequest::ClockRead, || {
                                record::ChoiceEntry::ClockRead {
                                    value: monotonic_ns(),
                                }
                            })?
                    };
                    let record::ChoiceEntry::ClockRead { value: ns } = entry else {
                        unreachable!(
                            "choose_checked(ClockRead, ..) always returns a ClockRead-shaped entry \
                             (a mismatched replay tag falls back to the request's own shape)"
                        )
                    };
                    vcpu.complete_mmio(access.token, MmioCompletion::ReadValue(ns))?;
                    Ok(Step::Keep)
                } else if ipa == mmio::ENTROPY_MMIO_ADDR {
                    mmio_access(&access, core, "ENTROPY_MMIO_ADDR", "entropy", true)?;
                    let info_off =
                        (machine_layout::MACHINE_INFO_BASE - machine_layout::DRAM_BASE) as usize;
                    let dest = unsafe {
                        let mut b = [0u8; 8];
                        std::ptr::copy_nonoverlapping(
                            host_ram.add(
                                info_off + wrela_machine::machine_info::OFF_ENTROPY_DEST as usize,
                            ),
                            b.as_mut_ptr(),
                            8,
                        );
                        u64::from_le_bytes(b)
                    };
                    let len = unsafe {
                        let mut b = [0u8; 8];
                        std::ptr::copy_nonoverlapping(
                            host_ram.add(
                                info_off + wrela_machine::machine_info::OFF_ENTROPY_LEN as usize,
                            ),
                            b.as_mut_ptr(),
                            8,
                        );
                        u64::from_le_bytes(b)
                    };
                    {
                        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        apply_entropy_read(&mut g.chooser, host_ram, dest, len)?;
                    }
                    vcpu.complete_mmio(access.token, MmioCompletion::WriteAccepted)?;
                    Ok(Step::Keep)
                } else if ipa == mmio::INPUT_STATUS_MMIO_ADDR {
                    mmio_access(&access, core, "INPUT_STATUS_MMIO_ADDR", "input", false)?;
                    let pending = {
                        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        if g.chooser.is_replaying() {
                            g.chooser.replay_input_pending()
                        } else {
                            let Shared { chooser, input, .. } = &mut *g;
                            input.pending(chooser).map_err(VmmError::GuestFault)?
                        }
                    };
                    vcpu.complete_mmio(
                        access.token,
                        MmioCompletion::ReadValue(u64::from(pending)),
                    )?;
                    Ok(Step::Keep)
                } else if ipa == mmio::INPUT_EVENT_MMIO_ADDR {
                    mmio_access(&access, core, "INPUT_EVENT_MMIO_ADDR", "input", false)?;
                    let event = {
                        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        let Shared { chooser, input, .. } = &mut *g;
                        input.pop(chooser)?
                    }
                    .ok_or_else(|| {
                        VmmError::GuestFault(format!(
                            "core {core}: input event read while the queue is empty"
                        ))
                    })?;
                    let sequence = u32::try_from(event.sequence).map_err(|_| {
                        VmmError::GuestFault("input event sequence exceeds the v1 ABI".into())
                    })?;
                    let packed = u64::from(event.value as u16)
                        | (u64::from(wrela_machine::input::control_code(event.control)) << 16)
                        | (u64::from(event.player) << 24)
                        | (u64::from(sequence) << 32);
                    vcpu.complete_mmio(access.token, MmioCompletion::ReadValue(packed))?;
                    Ok(Step::Keep)
                } else if wrela_machine::pixels::is_display_doorbell_addr(ipa) {
                    mmio_access(&access, core, "DISPLAY_DOORBELL_ADDR", "display", true)?;
                    let control_addr = access.write_value.unwrap_or(0);
                    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    let submission =
                        g.display
                            .consume_published_from(ipa, memory_handle, control_addr);
                    if matches!(submission, Err(VmmError::Io(_))) {
                        return submission.map(|_| Step::Keep);
                    }
                    let status = g.display.last_completion_status();
                    // SAFETY: `host_ram` is the live DRAM mapping for this boot.
                    unsafe {
                        crate::display::publish_completion_status(host_ram, control_addr, status)?;
                    }
                    if let Ok(frame) = submission {
                        g.chooser.check_frame_output(&frame)?;
                    }
                    drop(g);
                    vcpu.complete_mmio(access.token, MmioCompletion::WriteAccepted)?;
                    Ok(Step::Keep)
                } else if ipa == mmio::RELEASE_MMIO_ADDR {
                    mmio_access(&access, core, "RELEASE_MMIO_ADDR", "release", true)?;
                    if core != 0 {
                        return Err(VmmError::GuestFault(format!(
                            "core {core} rang the release doorbell: only the boot core releases \
                             the others (06-machine.md §3)"
                        )));
                    }
                    let value = access.write_value.unwrap_or(0);
                    if value != cores_declared as u64 {
                        return Err(VmmError::GuestFault(format!(
                            "core 0 released {value} core(s) but this image's report declares \
                             Cores count={cores_declared} — the image and its report disagree \
                             about the machine"
                        )));
                    }
                    vcpu.complete_mmio(access.token, MmioCompletion::WriteAccepted)?;
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
                    Ok(Step::Yield {
                        release: true,
                        park: false,
                    })
                } else if ipa == mmio::QUIESCE_MMIO_ADDR {
                    mmio_access(&access, core, "QUIESCE_MMIO_ADDR", "quiesce", true)?;
                    let named = access.write_value.unwrap_or(0);
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
                        commit_completions(state, &mut g.chooser, &completions, memory_handle)?;
                    }
                    vcpu.complete_mmio(access.token, MmioCompletion::WriteAccepted)?;
                    Ok(Step::Keep)
                } else if ipa == mmio::PARK_MMIO_ADDR {
                    mmio_access(&access, core, "PARK_MMIO_ADDR", "park", true)?;
                    vcpu.complete_mmio(access.token, MmioCompletion::WriteAccepted)?;
                    if core != 0 {
                        if pending_word(memory_handle, core) != 0 {
                            return Ok(Step::Keep);
                        }
                        return Ok(Step::Yield {
                            release: false,
                            park: true,
                        });
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
                    if cores_declared > 1 && deadline_ns == 0 {
                        let blk_completed = {
                            let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                            let g = &mut *g;
                            service_blk(&mut g.blk, &mut g.chooser, memory_handle)?
                        };
                        return Ok(Step::Yield {
                            release: false,
                            park: !blk_completed && pending_word(memory_handle, core) == 0,
                        });
                    }
                    {
                        let g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        let sibling = (1..cores_declared).any(|c| {
                            g.sched.state[c] == CoreState::Runnable
                                || (g.sched.state[c] == CoreState::Parked
                                    && pending_word(memory_handle, c) != 0)
                        });
                        if sibling {
                            drop(g);
                            return Ok(Step::Yield {
                                release: false,
                                park: false,
                            });
                        }
                    }
                    let blk_completed = {
                        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        let g = &mut *g;
                        service_blk(&mut g.blk, &mut g.chooser, memory_handle)?
                    };
                    let already_pending = pending_word(memory_handle, core) != 0;
                    if !already_pending && !blk_completed {
                        let should_sleep = {
                            let g = lock.lock().unwrap_or_else(|e| e.into_inner());
                            g.chooser.is_recording()
                        };
                        if should_sleep {
                            sleep_until_park_deadline(lock, wake, deadline_ns);
                        }
                        {
                            let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                            if g.sched.done {
                                if g.error.is_none() && g.exit_code.is_none() {
                                    let transcript_so_far = drain_console(host_ram);
                                    return Err(VmmError::Timeout {
                                        core,
                                        cap: wall_cap,
                                        transcript_so_far,
                                    });
                                }
                                return Ok(Step::Keep);
                            }
                            g.chooser.choose_checked(
                                record::ChoiceRequest::DeadlineWake { deadline_ns },
                                || record::ChoiceEntry::DeadlineWake { deadline_ns },
                            )?;
                            g.chooser.choose_checked(
                                record::ChoiceRequest::VectorRaise { vector: 0 },
                                || record::ChoiceEntry::VectorRaise { vector: 0 },
                            )?;
                        }
                        raise_vector(memory_handle, 0)?;
                    }
                    Ok(Step::Keep)
                } else {
                    Err(VmmError::GuestFault(format!(
                        "core {core}: MMIO address {ipa:#x} is not part of the Wrela machine"
                    )))
                }
            }
            HostExit::Canceled => {
                let transcript_so_far = drain_console(host_ram);
                Err(VmmError::Timeout {
                    core,
                    cap: wall_cap,
                    transcript_so_far,
                })
            }
            HostExit::Breakpoint { immediate } => {
                let registers = vcpu.diagnostics()?;
                // A host that traps BRK itself and a host that lets the guest
                // vector take it must name the same class; only the backend's
                // private detail differs. See `wrela_machine::fault`.
                Err(VmmError::GuestFault(format!(
                    "core {core}: {} immediate={immediate:?} pc={:#x}",
                    wrela_machine::fault::GUEST_BRK,
                    registers.pc
                )))
            }
            HostExit::Unexpected(raw) => {
                let registers = vcpu.diagnostics().unwrap_or(DiagnosticRegisters {
                    pc: 0,
                    pstate: 0,
                    esr_el1: 0,
                    far_el1: 0,
                    elr_el1: 0,
                    spsr_el1: 0,
                });
                Err(VmmError::GuestFault(format!(
                    "core {core}: unexpected host exit class={} reason={:#x} syndrome={:?} \
                     physical_address={:?} pc={:#x} detail={}",
                    raw.class,
                    raw.raw_reason,
                    raw.syndrome,
                    raw.physical_address,
                    registers.pc,
                    raw.detail
                )))
            }
        }
    }

    fn run_core<V: HostVcpu>(
        core: usize,
        mut vcpu: V,
        host_ram: *mut u8,
        memory_handle: crate::guest_memory::GuestMemoryHandle,
        cores_declared: usize,
        lock: &std::sync::Mutex<Shared<V::Handle>>,
        wake: &std::sync::Condvar,
        run_ns_out: &std::sync::atomic::AtomicU64,
        guest_counters: bool,
        counter_out: &[std::sync::atomic::AtomicU64; 7],
        wall_cap: Duration,
    ) {
        struct PublishRunNs<'a> {
            total: std::cell::Cell<u64>,
            out: &'a std::sync::atomic::AtomicU64,
        }
        impl Drop for PublishRunNs<'_> {
            fn drop(&mut self) {
                self.out
                    .store(self.total.get(), std::sync::atomic::Ordering::Relaxed);
            }
        }
        let run_ns = PublishRunNs {
            total: std::cell::Cell::new(0),
            out: run_ns_out,
        };
        let mut pmu = if guest_counters {
            match crate::guest_pmu::GuestPmu::open() {
                Ok(pmu) => Some(pmu),
                Err(error) => {
                    fail_core(lock, wake, core, error);
                    return;
                }
            }
        } else {
            None
        };
        let mut counter_totals = [0_u64; 7];
        loop {
            {
                let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if g.sched.done {
                        g.sched.state[core] = CoreState::Finished;
                        return;
                    }
                    if g.sched.state[core] == CoreState::Parked
                        && pending_word(memory_handle, core) != 0
                    {
                        g.sched.state[core] = CoreState::Runnable;
                    }
                    match g.sched.state[core] {
                        CoreState::Runnable => break,
                        CoreState::Unreleased | CoreState::Parked => {
                            let (g2, _) = wake
                                .wait_timeout(g, Duration::from_millis(1))
                                .unwrap_or_else(|e| e.into_inner());
                            g = g2;
                        }
                        CoreState::Finished => return,
                    }
                }
                if core == 0 {
                    let s = &mut *g;
                    if let Err(e) = service_blk(&mut s.blk, &mut s.chooser, memory_handle) {
                        mark_failed(s, core, e);
                        drop(g);
                        wake.notify_all();
                        return;
                    }
                }
            }
            if let Some(pmu) = &pmu
                && let Err(error) = pmu.start()
            {
                fail_core(lock, wake, core, error);
                return;
            }
            let entered = std::time::Instant::now();
            let result = vcpu.run();
            let elapsed = u64::try_from(entered.elapsed().as_nanos()).unwrap_or(u64::MAX);
            run_ns.total.set(run_ns.total.get().saturating_add(elapsed));
            if let Some(pmu) = &mut pmu {
                match pmu.stop_and_read() {
                    Ok(values) => {
                        for index in 0..values.len() {
                            counter_totals[index] =
                                counter_totals[index].saturating_add(values[index]);
                            counter_out[index]
                                .store(counter_totals[index], std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Err(error) => {
                        fail_core(lock, wake, core, error);
                        return;
                    }
                }
            }
            let exit = match result {
                Ok(exit) => exit,
                Err(error) => {
                    fail_core(lock, wake, core, error);
                    return;
                }
            };
            {
                let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                guard.exits += 1;
                let g = &mut *guard;
                match observe_admissions(&mut g.admission, host_ram, core) {
                    Ok(admitted) => g.admission_buf[core].extend(admitted),
                    Err(e) => {
                        mark_failed(g, core, e);
                        drop(guard);
                        wake.notify_all();
                        return;
                    }
                }
            }
            match handle_exit(
                core,
                &mut vcpu,
                exit,
                host_ram,
                memory_handle,
                cores_declared,
                lock,
                wake,
                wall_cap,
            ) {
                Ok(Step::Keep) => {
                    wake.notify_all();
                }
                Ok(Step::Yield { release, park }) => {
                    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if guard.sched.done {
                            guard.sched.state[core] = CoreState::Finished;
                            drop(guard);
                            wake.notify_all();
                            return;
                        }
                        if guard.sched.current == core {
                            break;
                        }
                        let cur = guard.sched.current;
                        let holder_stuck = match guard.sched.state[cur] {
                            CoreState::Finished | CoreState::Unreleased => true,
                            CoreState::Parked => pending_word(memory_handle, cur) == 0,
                            CoreState::Runnable => false,
                        };
                        if holder_stuck {
                            guard.sched.current = core;
                            break;
                        }
                        guard = wake.wait(guard).unwrap_or_else(|e| e.into_inner());
                    }
                    let g = &mut *guard;
                    let buffered = std::mem::take(&mut g.admission_buf[core]);
                    if let Err(e) = commit_admissions(&mut g.chooser, &buffered) {
                        mark_failed(g, core, e);
                        drop(guard);
                        wake.notify_all();
                        return;
                    }
                    let index = g.chooser.resolved_count();
                    let live_next = next_core(&mut g.sched, core, memory_handle, cores_declared);
                    let progress_core = if release {
                        core as u32
                    } else {
                        match live_next {
                            Some(n) => n as u32,
                            None => core as u32,
                        }
                    };
                    let chosen = match g.chooser.choose_checked(
                        record::ChoiceRequest::Progress {
                            core: progress_core,
                        },
                        || record::ChoiceEntry::Progress {
                            core: progress_core,
                        },
                    ) {
                        Ok(e) => e,
                        Err(e) => {
                            mark_failed(g, core, e);
                            drop(guard);
                            wake.notify_all();
                            return;
                        }
                    };
                    let record::ChoiceEntry::Progress { core: forced } = chosen else {
                        unreachable!("Progress request returns Progress");
                    };
                    if release {
                        if (forced as usize) >= cores_declared {
                            if let Err(e) = g.chooser.note_divergence_checked(
                                record::Divergence::ProgressMismatch {
                                    index,
                                    recorded: forced,
                                    actual: core as u32,
                                },
                            ) {
                                mark_failed(g, core, e);
                                drop(guard);
                                wake.notify_all();
                                return;
                            }
                        }
                        g.sched.current = if g.chooser.is_replaying() {
                            (forced as usize).min(cores_declared.saturating_sub(1))
                        } else {
                            core
                        };
                    } else {
                        match live_next {
                            Some(next) => {
                                let use_core = if g.chooser.is_replaying() {
                                    let forced_usize = forced as usize;
                                    if forced_usize < cores_declared
                                        && (g.sched.state[forced_usize] != CoreState::Parked
                                            || pending_word(memory_handle, forced_usize) != 0)
                                    {
                                        forced_usize
                                    } else {
                                        next
                                    }
                                } else {
                                    next
                                };
                                if (forced as usize) >= cores_declared {
                                    if let Err(e) = g.chooser.note_divergence_checked(
                                        record::Divergence::ProgressMismatch {
                                            index,
                                            recorded: forced,
                                            actual: next as u32,
                                        },
                                    ) {
                                        mark_failed(g, core, e);
                                        drop(guard);
                                        wake.notify_all();
                                        return;
                                    }
                                }
                                g.sched.current = use_core.min(cores_declared.saturating_sub(1));
                                if g.sched.state[g.sched.current] == CoreState::Parked
                                    && pending_word(memory_handle, g.sched.current) != 0
                                {
                                    g.sched.state[g.sched.current] = CoreState::Runnable;
                                }
                            }
                            None => {
                                if g.exit_code.is_some() || g.sched.done {
                                    g.sched.done = true;
                                } else {
                                    g.error.get_or_insert(VmmError::GuestFault(format!(
                                        "core {core} parked and no core is runnable: every core is \
                                         parked with an empty pending word, so no turn can ever run \
                                         again (04-compiler.md §2)"
                                    )));
                                    g.sched.done = true;
                                }
                            }
                        }
                    }
                    if park {
                        g.sched.state[core] = CoreState::Parked;
                    }
                    if g.sched.done {
                        g.sched.state[core] = CoreState::Finished;
                    }
                    let finished = g.sched.done;
                    drop(guard);
                    wake.notify_all();
                    if finished {
                        return;
                    }
                }
                Ok(Step::Halt(code)) => {
                    {
                        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        g.exit_code.get_or_insert(code);
                        for c in 0..cores_declared {
                            let buffered = std::mem::take(&mut g.admission_buf[c]);
                            if let Err(e) = commit_admissions(&mut g.chooser, &buffered) {
                                g.error.get_or_insert(e);
                                g.sched.done = true;
                                g.sched.state[core] = CoreState::Finished;
                                drop(g);
                                wake.notify_all();
                                return;
                            }
                        }
                        let released = g.released;
                        drop(g);
                        if released {
                            let grace = std::time::Instant::now() + Duration::from_millis(50);
                            while std::time::Instant::now() < grace {
                                let mut ok = true;
                                for c in 0..cores_declared {
                                    if read_core_mark(host_ram, c)
                                        != wrela_machine::machine_info::core_mark_running(c)
                                    {
                                        ok = false;
                                        break;
                                    }
                                }
                                if ok {
                                    break;
                                }
                                wake.notify_all();
                                std::thread::sleep(Duration::from_micros(200));
                            }
                        }
                    }
                    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    g.sched.done = true;
                    g.sched.state[core] = CoreState::Finished;
                    drop(g);
                    wake.notify_all();
                    return;
                }
                Err(e) => {
                    fail_core(lock, wake, core, e);
                    return;
                }
            }
        }
    }

    let mut core_entry = [0u64; CORE_SLOTS];
    core_entry[0] = parsed.entry;
    for CoreEntry { core, base } in &parsed.core_entries {
        core_entry[*core] = *base;
    }

    // Exact Pixels proof workloads are intentionally not a
    // performance-admission gate before P12. Keep the ordinary boot hang
    // detector tight while allowing a finite from-scratch visibility
    // proof to complete on slower signed runners. The cores are told which
    // deadline applies so a timeout names the bound that actually elapsed.
    let watchdog_cap = crate::boot_wall_cap(!parsed.renderer_placements.is_empty());
    #[cfg(all(test, target_os = "linux", target_arch = "aarch64"))]
    let watchdog_cap = TEST_WATCHDOG_CAP.get().unwrap_or(watchdog_cap);
    let (handles_tx, handles_rx) = std::sync::mpsc::channel::<usize>();
    let vcpu_run_ns: [std::sync::atomic::AtomicU64; CORE_SLOTS] =
        std::array::from_fn(|_| std::sync::atomic::AtomicU64::new(0));
    let vcpu_guest_counters: [[std::sync::atomic::AtomicU64; 7]; CORE_SLOTS] =
        std::array::from_fn(|_| std::array::from_fn(|_| std::sync::atomic::AtomicU64::new(0)));
    std::thread::scope(|scope| {
        let mut threads = Vec::with_capacity(cores_declared);
        for core in 0..cores_declared {
            let ram = SendPtr(host_ram);
            let tx = handles_tx.clone();
            let shared = &shared;
            let wake = &wake;
            let entry = core_entry[core];
            let sp_top = sp_tops[core];
            let vm = &vm;
            let run_ns_out = &vcpu_run_ns[core];
            let counter_out = &vcpu_guest_counters[core];
            threads.push(scope.spawn(move || {
                let ram = ram;
                let SendPtr(host_ram) = ram;
                if let Err(error) = crate::hardening::pin_vcpu(core, host_profile) {
                    fail_core(shared, wake, core, error);
                    let _ = tx.send(core);
                    return;
                }
                #[cfg(test)]
                if create_inject::should_fail(core) {
                    fail_core(
                        shared,
                        wake,
                        core,
                        crate::host::host_error(
                            H::KIND,
                            "create vCPU",
                            0xfae9_4005u32 as i32 as i64,
                            format!(
                                "host refused sealed Cores count={cores_declared} at core {core}"
                            ),
                        ),
                    );
                    let _ = tx.send(core);
                    return;
                }
                let mut state = VcpuBootState::wrela(entry, sp_top, translation_boot);
                state.x[18] = wrela_machine::lane2::BASE
                    + wrela_machine::lane2::HITS_OFFSET
                    + (core as u64) * wrela_machine::lane2::CORE_STRIDE;
                let (vcpu, handle) = match vm.create_vcpu(core, &state) {
                    Ok(pair) => pair,
                    Err(error) => {
                        fail_core(shared, wake, core, error);
                        let _ = tx.send(core);
                        return;
                    }
                };
                {
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                    g.vcpus[core] = Some(handle);
                }
                let _ = tx.send(core);
                run_core(
                    core,
                    vcpu,
                    host_ram,
                    memory_handle,
                    cores_declared,
                    shared,
                    wake,
                    run_ns_out,
                    guest_counters,
                    counter_out,
                    watchdog_cap,
                );
                {
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                    g.vcpus[core] = None;
                }
            }));
        }
        for _ in 0..cores_declared {
            let _ = handles_rx.recv();
        }

        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let watchdog_shared = &shared;
        let watchdog_wake = &wake;
        let watchdog = scope.spawn(move || {
            if done_rx.recv_timeout(watchdog_cap).is_err() {
                let mut g = watchdog_shared.lock().unwrap_or_else(|e| e.into_inner());
                let live: Vec<_> = g.vcpus.iter().filter_map(Clone::clone).collect();
                g.sched.done = true;
                drop(g);
                for handle in live {
                    let _ = handle.cancel();
                }
                watchdog_wake.notify_all();
            }
        });
        for t in threads {
            let _ = t.join();
        }
        let _ = done_tx.send(());
        let _ = watchdog.join();
    });
    let mut shared = shared.into_inner().unwrap_or_else(|e| e.into_inner());
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
    if shared.released {
        check_core_marks(host_ram, cores_declared)?;
    }
    if let Some(path) = std::env::var_os("WRELA_P8_STATE_DUMP") {
        let placement = parsed.renderer_placements.first().ok_or_else(|| {
            VmmError::MalformedReport("P8 state dump requested without a renderer".into())
        })?;
        let offset = usize::try_from(placement.state_base - machine_layout::DRAM_BASE)
            .map_err(|_| VmmError::BadImage("renderer state offset does not fit usize".into()))?;
        let len = usize::try_from(placement.state_size)
            .map_err(|_| VmmError::BadImage("renderer state size does not fit usize".into()))?;
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= guest_memory.len())
            .ok_or_else(|| VmmError::BadImage("renderer state range exceeds DRAM".into()))?;
        let bytes = unsafe { std::slice::from_raw_parts(host_ram.add(offset), end - offset) };
        std::fs::write(&path, bytes).map_err(|error| {
            VmmError::Io(format!(
                "write requested P8 renderer-state dump {}: {error}",
                std::path::Path::new(&path).display()
            ))
        })?;
    }

    let mut transcript = drain_console(host_ram);
    transcript.extend_from_slice(&crate::replay::frame_log_bytes(shared.display.frames()));
    transcript.extend_from_slice(&crate::replay::rejected_display_event_log_bytes(
        shared.display.events(),
    ));
    let core_marks = (0..cores_declared)
        .map(|c| read_core_mark(host_ram, c))
        .collect::<Vec<u64>>();
    let mut lane2_hits_per_core = crate::lane3::read_lane2_hits_per_core(host_ram);
    lane2_hits_per_core.truncate(cores_declared);
    let lane2_hits = crate::lane3::read_lane2_hits(host_ram);
    shared.input.finalize().map_err(VmmError::GuestFault)?;
    let (choices, divergences) = record::finish_chooser(shared.chooser)?;
    Ok((
        BootOutcome {
            transcript,
            exit_code,
            choices,
            exits: shared.exits,
            core_marks,
            lane2_hits,
            lane2_hits_per_core,
            frames: shared.display.frames().to_vec(),
            frame_buffer_digests: shared.display.backend_digests().to_vec(),
            vcpu_run_ns: vcpu_run_ns[..cores_declared]
                .iter()
                .map(|value| value.load(std::sync::atomic::Ordering::Relaxed))
                .collect(),
            vcpu_guest_counters: vcpu_guest_counters[..cores_declared]
                .iter()
                .map(|row| {
                    std::array::from_fn(|index| {
                        row[index].load(std::sync::atomic::Ordering::Relaxed)
                    })
                })
                .collect(),
            host_profile: host_profile.name(),
            translation_profile: match translation_mode {
                crate::boot::TranslationMode::SealedStage1 => "sealed-stage1",
                crate::boot::TranslationMode::DiagnosticMmuOff => "diagnostic-mmu-off",
            },
        },
        divergences,
    ))
}

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "aarch64")
)))]
pub(crate) fn boot_image_core(
    _report_path: &Path,
    _img_path: &Path,
    _replay_choices: Option<Vec<record::ChoiceEntry>>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    Err(VmmError::Unsupported(
        "the Wrela VMM requires macOS/aarch64 Hypervisor.framework or Linux/aarch64 KVM",
    ))
}
