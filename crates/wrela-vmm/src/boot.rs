use std::path::Path;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::time::Duration;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use wrela_machine::report::{CoreEntry, CoreStack, ParsedReport, ReportSection};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::devices;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::exit_loop::{
    AdmissionWitness, BlkState, advance_pc, apply_entropy_read, check_core_marks,
    check_vector_in_range, commit_admissions, commit_completions, drain_console,
    el1_exception_note, monotonic_ns, observe_admissions, raise_vector, read_core_mark, read_pc,
    service_blk,
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::hv;
use crate::record;
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
use crate::{BootOutcome, VmmError};

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
#[path = "boot_kvm.rs"]
mod kvm_boot;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::{
    BootOutcome, VmmError, capped_park_deadline_ns, guest_dram_offset, parse_report,
    validate_report_digests,
};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const CORE_SLOTS: usize = wrela_machine::CORE_SLOTS;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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

pub(crate) fn host_cores_refuse(requested: usize, failed_at: usize, code: i32) -> VmmError {
    VmmError::HostCoresRefuse {
        requested,
        failed_at,
        code,
    }
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
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

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn apply_wx_exec_protections(exec_sections: &[ReportSection]) -> Result<(), VmmError> {
    use hv::{HV_MEMORY_EXEC, HV_MEMORY_READ, HV_SUCCESS, hv_vm_protect};
    const PAGE: u64 = 16 * 1024;
    for s in exec_sections {
        let start = s.base & !(PAGE - 1);
        let end = s
            .base
            .checked_add(s.size)
            .and_then(|e| e.checked_add(PAGE - 1))
            .map(|e| e & !(PAGE - 1))
            .ok_or_else(|| {
                VmmError::BadImage(format!(
                    "exec section `{}` base={:#x} size={} overflows when page-aligning for W^X",
                    s.name, s.base, s.size
                ))
            })?;
        let size = (end - start) as usize;
        if size == 0 {
            continue;
        }
        let r = unsafe { hv_vm_protect(start, size, HV_MEMORY_READ | HV_MEMORY_EXEC) };
        if r != HV_SUCCESS {
            return Err(VmmError::Hvf {
                call: "hv_vm_protect",
                code: r,
            });
        }
    }
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn boot_image(report_path: &Path, img_path: &Path) -> Result<BootOutcome, VmmError> {
    boot_image_with_display(
        report_path,
        img_path,
        crate::display::DisplayBackendSelection::Headless,
    )
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn boot_image_with_display(
    report_path: &Path,
    img_path: &Path,
    display: crate::display::DisplayBackendSelection,
) -> Result<BootOutcome, VmmError> {
    boot_image_core_inner(report_path, img_path, None, None, display)
        .map(|(outcome, _divergences)| outcome)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn boot_image_core(
    report_path: &Path,
    img_path: &Path,
    replay_choices: Option<Vec<record::ChoiceEntry>>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    boot_image_core_inner(
        report_path,
        img_path,
        replay_choices,
        None,
        crate::display::DisplayBackendSelection::Headless,
    )
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn boot_image_core_with_delayed_raise(
    report_path: &Path,
    img_path: &Path,
    replay_choices: Option<Vec<record::ChoiceEntry>>,
    test_delayed_raise: Option<(Duration, u64)>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    boot_image_core_inner(
        report_path,
        img_path,
        replay_choices,
        test_delayed_raise,
        crate::display::DisplayBackendSelection::Headless,
    )
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn boot_image_core_inner(
    report_path: &Path,
    img_path: &Path,
    replay_choices: Option<Vec<record::ChoiceEntry>>,
    test_delayed_raise: Option<(Duration, u64)>,
    display_selection: crate::display::DisplayBackendSelection,
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
    validate_report_digests(&parsed, &img)?;

    let image_off = machine_layout::IMAGE_BASE - machine_layout::DRAM_BASE;
    if image_off + (img.len() as u64) > machine_layout::DRAM_SIZE {
        return Err(VmmError::BadImage(format!(
            "image ({} bytes at offset {:#x}) does not fit the {} byte DRAM reservation",
            img.len(),
            image_off,
            machine_layout::DRAM_SIZE
        )));
    }

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

    let r = unsafe { hv_vm_create(std::ptr::null_mut()) };
    if r != HV_SUCCESS {
        return Err(VmmError::Hvf {
            call: "hv_vm_create",
            code: r,
        });
    }
    struct VmGuard;
    impl Drop for VmGuard {
        fn drop(&mut self) {
            unsafe {
                hv_vm_destroy();
            }
        }
    }
    let _vm_guard = VmGuard;

    let r = unsafe {
        hv_vm_map(
            host_ram as *mut c_void,
            machine_layout::DRAM_BASE,
            dram_size,
            HV_MEMORY_READ | HV_MEMORY_WRITE,
        )
    };
    if r != HV_SUCCESS {
        return Err(VmmError::Hvf {
            call: "hv_vm_map",
            code: r,
        });
    }

    unsafe {
        std::ptr::copy_nonoverlapping(img.as_ptr(), host_ram.add(image_off as usize), img.len());
    }
    apply_wx_exec_protections(&parsed.exec_sections)?;
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

    let _ = monotonic_ns();
    let sp_tops = core_sp_tops_from_report(&parsed);
    let blk: Option<BlkState> = match parsed.blk {
        None => None,
        Some(cfg) => {
            let pools = cfg.pools.clone();
            let vector = cfg.vector;
            let device_index = cfg.device;
            let device = devices::BlkDevice::new(cfg).map_err(VmmError::BadImage)?;
            let mem = unsafe { devices::GuestMem::new(host_ram, pools, device_index) }
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
        let off = guest_dram_offset(guest, 4, "IrqHostInject")?;
        check_vector_in_range(inj.vector)?;
        unsafe {
            std::ptr::copy_nonoverlapping(inj.status.to_le_bytes().as_ptr(), host_ram.add(off), 4);
        }
        raise_vector(host_ram, inj.vector)?;
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
        let ptr = SendPtr(host_ram);
        let raise_pending_off =
            (wrela_machine::pending::core_word_addr(0) - machine_layout::DRAM_BASE) as usize;
        std::thread::spawn(move || {
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

    struct Shared {
        sched: Sched,
        chooser: record::Chooser,
        blk: Option<BlkState>,
        exits: u64,
        exit_code: Option<u64>,
        error: Option<VmmError>,
        vcpus: [u64; CORE_SLOTS],
        released: bool,
        admission: AdmissionWitness,
        admission_buf: [Vec<(String, String)>; CORE_SLOTS],
        display: crate::display::RuntimeDisplay,
    }
    unsafe impl Send for Shared {}

    fn pending_word(host_ram: *const u8, core: usize) -> u64 {
        let off =
            (wrela_machine::pending::core_word_addr(core) - machine_layout::DRAM_BASE) as usize;
        let mut b = [0u8; 8];
        unsafe { std::ptr::copy_nonoverlapping(host_ram.add(off), b.as_mut_ptr(), 8) };
        u64::from_le_bytes(b)
    }

    fn next_core(
        sched: &mut Sched,
        from: usize,
        host_ram: *const u8,
        cores_declared: usize,
    ) -> Option<usize> {
        for step in 1..=cores_declared {
            let c = (from + step) % cores_declared;
            if sched.state[c] == CoreState::Parked && pending_word(host_ram, c) != 0 {
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

    let cores_declared = parsed.cores;
    if !(1..=CORE_SLOTS).contains(&cores_declared) {
        return Err(VmmError::MalformedReport(format!(
            "`Cores count={cores_declared}` must satisfy 1..=CORE_SLOTS ({CORE_SLOTS})"
        )));
    }
    let display = crate::display::runtime_display(display_selection)?;
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
        vcpus: [0; CORE_SLOTS],
        admission: AdmissionWitness::new(parsed.request_rings.clone()),
        admission_buf: std::array::from_fn(|_| Vec::new()),
        display,
    });
    let wake = std::sync::Condvar::new();

    fn sleep_until_park_deadline(
        lock: &std::sync::Mutex<Shared>,
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

    fn require_mmword(da: &DataAbort, core: usize, what: &str) -> Result<(), VmmError> {
        if da.size_bytes != 8 {
            return Err(VmmError::GuestFault(format!(
                "core {core}: {what} requires an 8-byte access, got a {}-byte access",
                da.size_bytes
            )));
        }
        Ok(())
    }

    fn mmio_access(
        esr: u64,
        core: usize,
        what: &str,
        protocol: &str,
        want_write: bool,
    ) -> Result<DataAbort, VmmError> {
        let Some(da) = decode_data_abort(esr) else {
            return Err(VmmError::GuestFault(format!(
                "core {core}: unhandled access shape at {what} (esr={esr:#x})"
            )));
        };
        require_mmword(&da, core, what)?;
        if want_write && !da.write {
            return Err(VmmError::GuestFault(format!(
                "core {core}: a load from {what} is not part of the {protocol} protocol"
            )));
        }
        if !want_write && da.write {
            return Err(VmmError::GuestFault(format!(
                "core {core}: a store to {what} is not part of the {protocol} protocol"
            )));
        }
        Ok(da)
    }

    fn mmio_src_value(vcpu: u64, da: &DataAbort) -> Result<u64, VmmError> {
        match da.reg {
            Some(reg) => {
                let mut v = 0u64;
                let r = unsafe { hv_vcpu_get_reg(vcpu, hv_reg_xn(reg), &mut v) };
                if r != HV_SUCCESS {
                    return Err(VmmError::Hvf {
                        call: "hv_vcpu_get_reg",
                        code: r,
                    });
                }
                Ok(v)
            }
            None => Ok(0),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_exit(
        core: usize,
        vcpu: u64,
        exit_ptr: *const HvVcpuExit,
        host_ram: *mut u8,
        cores_declared: usize,
        lock: &std::sync::Mutex<Shared>,
        wake: &std::sync::Condvar,
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
                    let da = mmio_access(esr, core, "EXIT_MMIO_ADDR", "exit", true)?;
                    let value = mmio_src_value(vcpu, &da)?;
                    Ok(Step::Halt(value))
                } else if ipa == mmio::CLOCK_MMIO_ADDR {
                    let da = mmio_access(esr, core, "CLOCK_MMIO_ADDR", "clock", false)?;
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
                } else if ipa == mmio::ENTROPY_MMIO_ADDR {
                    mmio_access(esr, core, "ENTROPY_MMIO_ADDR", "entropy", true)?;
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
                    advance_pc(vcpu)?;
                    Ok(Step::Keep)
                } else if wrela_machine::pixels::is_display_doorbell_addr(ipa) {
                    let da = mmio_access(esr, core, "DISPLAY_DOORBELL_ADDR", "display", true)?;
                    let control_addr = mmio_src_value(vcpu, &da)?;
                    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    // SAFETY: `host_ram` is the live DRAM mapping for this
                    // boot. The display doorbell synchronously transfers the
                    // referenced records to the host display model.
                    let submission = unsafe {
                        g.display.consume_volatile_from(
                            ipa,
                            host_ram,
                            machine_layout::DRAM_SIZE as usize,
                            control_addr,
                        )
                    };
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
                    advance_pc(vcpu)?;
                    Ok(Step::Keep)
                } else if ipa == mmio::RELEASE_MMIO_ADDR {
                    let da = mmio_access(esr, core, "RELEASE_MMIO_ADDR", "release", true)?;
                    if core != 0 {
                        return Err(VmmError::GuestFault(format!(
                            "core {core} rang the release doorbell: only the boot core releases \
                             the others (06-machine.md §3)"
                        )));
                    }
                    let value = mmio_src_value(vcpu, &da)?;
                    if value != cores_declared as u64 {
                        return Err(VmmError::GuestFault(format!(
                            "core 0 released {value} core(s) but this image's report declares \
                             Cores count={cores_declared} — the image and its report disagree \
                             about the machine"
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
                    Ok(Step::Yield {
                        release: true,
                        park: false,
                    })
                } else if ipa == mmio::QUIESCE_MMIO_ADDR {
                    let da = mmio_access(esr, core, "QUIESCE_MMIO_ADDR", "quiesce", true)?;
                    let named = mmio_src_value(vcpu, &da)?;
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
                    mmio_access(esr, core, "PARK_MMIO_ADDR", "park", true)?;
                    advance_pc(vcpu)?;
                    if core != 0 {
                        if pending_word(host_ram, core) != 0 {
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
                            service_blk(&mut g.blk, &mut g.chooser, host_ram)?
                        };
                        return Ok(Step::Yield {
                            release: false,
                            park: !blk_completed && pending_word(host_ram, core) == 0,
                        });
                    }
                    {
                        let g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        let sibling = (1..cores_declared).any(|c| {
                            g.sched.state[c] == CoreState::Runnable
                                || (g.sched.state[c] == CoreState::Parked
                                    && pending_word(host_ram, c) != 0)
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
                        service_blk(&mut g.blk, &mut g.chooser, host_ram)?
                    };
                    let already_pending = pending_word(host_ram, core) != 0;
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
                        raise_vector(host_ram, 0)?;
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

    fn run_core(
        core: usize,
        vcpu: u64,
        exit_ptr: *const HvVcpuExit,
        host_ram: *mut u8,
        cores_declared: usize,
        lock: &std::sync::Mutex<Shared>,
        wake: &std::sync::Condvar,
    ) {
        loop {
            {
                let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if g.sched.done {
                        g.sched.state[core] = CoreState::Finished;
                        return;
                    }
                    if g.sched.state[core] == CoreState::Parked && pending_word(host_ram, core) != 0
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
                    if let Err(e) = service_blk(&mut s.blk, &mut s.chooser, host_ram) {
                        s.error.get_or_insert(e);
                        s.sched.done = true;
                        s.sched.state[core] = CoreState::Finished;
                        drop(g);
                        wake.notify_all();
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
                wake.notify_all();
                return;
            }
            {
                let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                guard.exits += 1;
                let g = &mut *guard;
                match observe_admissions(&mut g.admission, host_ram, core) {
                    Ok(admitted) => g.admission_buf[core].extend(admitted),
                    Err(e) => {
                        g.error.get_or_insert(e);
                        g.sched.done = true;
                        g.sched.state[core] = CoreState::Finished;
                        drop(guard);
                        wake.notify_all();
                        return;
                    }
                }
            }
            match handle_exit(core, vcpu, exit_ptr, host_ram, cores_declared, lock, wake) {
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
                            CoreState::Parked => pending_word(host_ram, cur) == 0,
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
                        g.error.get_or_insert(e);
                        g.sched.done = true;
                        g.sched.state[core] = CoreState::Finished;
                        drop(guard);
                        wake.notify_all();
                        return;
                    }
                    let index = g.chooser.resolved_count();
                    let live_next = next_core(&mut g.sched, core, host_ram, cores_declared);
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
                            g.error.get_or_insert(e);
                            g.sched.done = true;
                            g.sched.state[core] = CoreState::Finished;
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
                                g.error.get_or_insert(e);
                                g.sched.done = true;
                                g.sched.state[core] = CoreState::Finished;
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
                                            || pending_word(host_ram, forced_usize) != 0)
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
                                        g.error.get_or_insert(e);
                                        g.sched.done = true;
                                        g.sched.state[core] = CoreState::Finished;
                                        drop(guard);
                                        wake.notify_all();
                                        return;
                                    }
                                }
                                g.sched.current = use_core.min(cores_declared.saturating_sub(1));
                                if g.sched.state[g.sched.current] == CoreState::Parked
                                    && pending_word(host_ram, g.sched.current) != 0
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
                    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    g.error.get_or_insert(e);
                    g.sched.done = true;
                    g.sched.state[core] = CoreState::Finished;
                    drop(g);
                    wake.notify_all();
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

    let (handles_tx, handles_rx) = std::sync::mpsc::channel::<usize>();
    std::thread::scope(|scope| {
        let mut threads = Vec::with_capacity(cores_declared);
        for core in 0..cores_declared {
            let ram = SendPtr(host_ram);
            let tx = handles_tx.clone();
            let shared = &shared;
            let wake = &wake;
            let entry = core_entry[core];
            let sp_top = sp_tops[core];
            threads.push(scope.spawn(move || {
                let ram = ram;
                let SendPtr(host_ram) = ram;
                let mut vcpu: u64 = 0;
                let mut exit_ptr: *mut HvVcpuExit = std::ptr::null_mut();
                #[cfg(test)]
                let create_code = if create_inject::should_fail(core) {
                    HV_NO_RESOURCES
                } else {
                    unsafe { hv_vcpu_create(&mut vcpu, &mut exit_ptr, std::ptr::null_mut()) }
                };
                #[cfg(not(test))]
                let create_code =
                    unsafe { hv_vcpu_create(&mut vcpu, &mut exit_ptr, std::ptr::null_mut()) };
                if create_code != HV_SUCCESS {
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                    g.error
                        .get_or_insert(host_cores_refuse(cores_declared, core, create_code));
                    g.sched.done = true;
                    g.sched.state[core] = CoreState::Finished;
                    drop(g);
                    wake.notify_all();
                    let _ = tx.send(core);
                    return;
                }
                {
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                    g.vcpus[core] = vcpu;
                }
                let _ = tx.send(core);

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
                    set(HV_REG_CPSR, 0x3c5)?;
                    let r = unsafe { hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_SP_EL1, sp_top) };
                    if r != HV_SUCCESS {
                        return Err(VmmError::Hvf {
                            call: "hv_vcpu_set_sys_reg(SP_EL1)",
                            code: r,
                        });
                    }
                    let r = unsafe { hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_CPACR_EL1, 0x0030_0000) };
                    if r != HV_SUCCESS {
                        return Err(VmmError::Hvf {
                            call: "hv_vcpu_set_sys_reg(CPACR_EL1)",
                            code: r,
                        });
                    }
                    set(HV_REG_FPCR, crate::GUEST_FPCR)?;
                    Ok(())
                })();
                match init {
                    Ok(()) => {
                        run_core(core, vcpu, exit_ptr, host_ram, cores_declared, shared, wake)
                    }
                    Err(e) => {
                        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                        g.error.get_or_insert(e);
                        g.sched.done = true;
                        g.sched.state[core] = CoreState::Finished;
                        drop(g);
                        wake.notify_all();
                    }
                }
                {
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                    g.vcpus[core] = 0;
                }
                unsafe {
                    hv_vcpu_destroy(vcpu);
                }
            }));
        }
        for _ in 0..cores_declared {
            let _ = handles_rx.recv();
        }

        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let watchdog_shared = &shared;
        let watchdog_wake = &wake;
        // Exact Pixels proof workloads are intentionally not a
        // performance-admission gate before P12. Keep the ordinary boot hang
        // detector tight while allowing a finite from-scratch visibility
        // proof to complete on slower signed runners.
        let watchdog_cap = crate::boot_wall_cap(!parsed.renderer_placements.is_empty());
        let watchdog = scope.spawn(move || {
            if done_rx.recv_timeout(watchdog_cap).is_err() {
                let mut g = watchdog_shared.lock().unwrap_or_else(|e| e.into_inner());
                let mut live: Vec<u64> = g.vcpus.iter().copied().filter(|v| *v != 0).collect();
                if !live.is_empty() {
                    unsafe {
                        hv_vcpus_exit(live.as_mut_ptr(), live.len() as u32);
                    }
                }
                g.sched.done = true;
                drop(g);
                watchdog_wake.notify_all();
            }
        });
        for t in threads {
            let _ = t.join();
        }
        let _ = done_tx.send(());
        let _ = watchdog.join();
    });
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
            .filter(|end| *end <= dram_size)
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
    let lane2_hits = crate::lane3::read_lane2_hits(host_ram);
    let (choices, divergences) = record::finish_chooser(shared.chooser)?;
    Ok((
        BootOutcome {
            transcript,
            exit_code,
            choices,
            exits: shared.exits,
            core_marks,
            lane2_hits,
            frames: shared.display.frames().to_vec(),
            frame_buffer_digests: shared.display.backend_digests().to_vec(),
        },
        divergences,
    ))
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub fn boot_image(report_path: &Path, img_path: &Path) -> Result<BootOutcome, VmmError> {
    boot_image_with_display(
        report_path,
        img_path,
        crate::display::DisplayBackendSelection::Headless,
    )
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub fn boot_image_with_display(
    report_path: &Path,
    img_path: &Path,
    display: crate::display::DisplayBackendSelection,
) -> Result<BootOutcome, VmmError> {
    Ok(kvm_boot::boot(report_path, img_path, display, None)?.0)
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub(crate) fn boot_image_core(
    report_path: &Path,
    img_path: &Path,
    replay_choices: Option<Vec<record::ChoiceEntry>>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    kvm_boot::boot(
        report_path,
        img_path,
        crate::display::DisplayBackendSelection::Headless,
        replay_choices,
    )
}

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "aarch64")
)))]
pub fn boot_image(_report_path: &Path, _img_path: &Path) -> Result<BootOutcome, VmmError> {
    Err(VmmError::Unsupported(
        "the wrela VMM needs Hypervisor.framework (macOS/aarch64 at M5); no other host is implemented yet",
    ))
}

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "aarch64")
)))]
pub fn boot_image_with_display(
    _report_path: &Path,
    _img_path: &Path,
    _display: crate::display::DisplayBackendSelection,
) -> Result<BootOutcome, VmmError> {
    Err(VmmError::Unsupported(
        "the wrela VMM needs Hypervisor.framework (macOS/aarch64 at M5); no other host is implemented yet",
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
        "the wrela VMM needs Hypervisor.framework (macOS/aarch64 at M5); no other host is implemented yet",
    ))
}
