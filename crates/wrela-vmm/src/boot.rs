//! Image boot entry points (`boot_image` / `boot_image_core`) and the
//! HVF setup path. The vCPU exit-loop helpers live in `exit_loop`.

use std::path::Path;
use std::time::Duration;

use wrela_machine::report::{CoreEntry, CoreStack, ParsedReport, ReportSection};

use crate::devices;
use crate::exit_loop::{
    AdmissionWitness, BlkState, advance_pc, apply_entropy_read, check_core_marks,
    check_vector_in_range, commit_admissions, commit_completions, drain_console,
    el1_exception_note, monotonic_ns, observe_admissions, raise_vector, read_core_mark, read_pc,
    service_blk,
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::hv;
use crate::record;
use crate::{
    BootOutcome, VmmError, WALL_CAP, capped_park_deadline_ns, guest_dram_offset, parse_report,
    validate_report_digests,
};

/// Array capacity for per-core sched state / vCPU handles: packing ceiling
/// (`CORE_SLOTS`), not the sealed bring-up N. Live loops use
/// `cores_declared` (plans/M15.md item F / decision 1060).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const CORE_SLOTS: usize = wrela_machine::CORE_SLOTS;

/// plans/M15.md item F / decision 1064: SP top for each sealed core from
/// report `CoreStack` lines (`base + size`). Legacy fixtures that omit
/// the lines fall back to the high-DRAM formula — never the retired low
/// `STACKS_BASE` map.
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

/// Named mapping for a failed `hv_vcpu_create` on a sealed-N bring-up
/// (plans/M15.md item F / decision 1062). Kept as a free function so the
/// unit test can pin the variant without needing a short host.
pub(crate) fn host_cores_refuse(requested: usize, failed_at: usize, code: i32) -> VmmError {
    VmmError::HostCoresRefuse {
        requested,
        failed_at,
        code,
    }
}

/// `cfg(test)` inject: force `hv_vcpu_create` for core `n` to fail with
/// `HV_NO_RESOURCES`, so host-refuse is pinned without a 32-core host.
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

/// Raise report-declared executable sections to RX (page-granular). The
/// DRAM reservation was mapped RW-only; this is the only place execute
/// permission is granted.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn apply_wx_exec_protections(exec_sections: &[ReportSection]) -> Result<(), VmmError> {
    use hv::{HV_MEMORY_EXEC, HV_MEMORY_READ, HV_SUCCESS, hv_vm_protect};
    // Apple Silicon HVF page size matches the host DRAM allocation align.
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
    boot_image_core(report_path, img_path, None).map(|(outcome, _divergences)| outcome)
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
/// Production boot core. The `cfg(test)`-only delayed vector-raise seam
/// lives on [`boot_image_core_with_delayed_raise`].
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn boot_image_core(
    report_path: &Path,
    img_path: &Path,
    replay_choices: Option<Vec<record::ChoiceEntry>>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    boot_image_core_inner(report_path, img_path, replay_choices, None)
}

/// `cfg(test)`-only wrapper for the delayed vector-raise conformance seam
/// (plans/M6.md item E). Production callers use [`boot_image_core`].
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn boot_image_core_with_delayed_raise(
    report_path: &Path,
    img_path: &Path,
    replay_choices: Option<Vec<record::ChoiceEntry>>,
    test_delayed_raise: Option<(Duration, u64)>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    boot_image_core_inner(report_path, img_path, replay_choices, test_delayed_raise)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn boot_image_core_inner(
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

    // W^X: map the whole reservation RW (never RWX). Executable sections
    // from the report are raised to RX via `hv_vm_protect` after the
    // image bytes are copied in — a guest that later stores into those
    // pages faults rather than executing its own write.
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

    // --- load the image + machine-info page ----------------------------------
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
    // Capture stack tops before `parsed.blk` is moved into the device model.
    let sp_tops = core_sp_tops_from_report(&parsed);
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
        let off = guest_dram_offset(guest, 4, "IrqHostInject")?;
        check_vector_in_range(inj.vector)?;
        unsafe {
            std::ptr::copy_nonoverlapping(inj.status.to_le_bytes().as_ptr(), host_ram.add(off), 4);
        }
        raise_vector(host_ram, inj.vector)?;
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

    // --- N vCPUs, overlapping hv_vcpu_run (plans/M15.md item I) ------------
    //
    // 06-machine.md §1: sealed image `Cores count=N`; the VMM creates
    // exactly those N vCPUs. §3 makes core 0's entry "release the other
    // vCPUs". Hypervisor.framework binds a vCPU to the thread that created
    // it, so there are N host threads. plans/M15.md item I deleted the
    // baton: every released Runnable core may enter `hv_vcpu_run`
    // concurrently under record **and** replay. Host `Shared` bookkeeping
    // stays behind one mutex (dumb); park/release/watchdog stay.
    // Yield-`Progress` forces the choice-log cursor (`sched.current`) at
    // park/release (decision 7) — that is the serial hand-off. Enter itself
    // is not gated on the cursor: enter-serializing replay of a concurrent-
    // record Progress tape deadlocks cross-core awaits. Yield-flush recovers
    // a stuck cursor (Progress target Parked with empty pending). Admission
    // replay checks a multiset bag (decision 8); cap-1 under-count leftovers
    // / extras are tolerated.
    //
    // Progress is appended at exactly the two guest actions that used to
    // move the baton: the release doorbell and a park (Yield). Every other
    // exit keeps the same core running. A single-core image — where
    // secondaries are never released — never emits Progress and runs down
    // exactly the path it ran before M8.
    // Array capacity = packing ceiling (`CORE_SLOTS`); live bring-up is
    // `cores_declared` from the report (item F creates exactly N vCPUs).

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum CoreState {
        /// Created and register-initialized, never released by the guest.
        Unreleased,
        /// Eligible to enter `hv_vcpu_run` (concurrently under record).
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
        /// Log-serializer cursor: whose buffered Admissions+Progress may
        /// flush next. Progress at Yield forces this under record and
        /// replay (decision 7). Enter does not wait on it — Runnable cores
        /// overlap in `hv_vcpu_run` under both modes. Yield-flush recovers
        /// the cursor when the holder is Parked with an empty pending word
        /// (otherwise Progress can plant a stuck cursor under overlap).
        current: usize,
        state: [CoreState; CORE_SLOTS],
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
        vcpus: [u64; CORE_SLOTS],
        /// Did the guest ring the release doorbell? Recorded rather than
        /// inferred from `sched.state`, which is `Finished` for every core
        /// by the time the marks are checked and so cannot say whether a
        /// core was ever released.
        released: bool,
        /// plans/M8.md item C3: the cross-core admission witness (06 §8).
        /// Empty `rings` for every single-core image, which is what makes
        /// their choice sequences byte-identical to their pre-C3 ones.
        admission: AdmissionWitness,
        /// plans/M15.md item I: Admissions buffered until Yield-`Progress`.
        /// Choice-log flush waits on `sched.current`; enter does not.
        admission_buf: [Vec<(String, String)>; CORE_SLOTS],
    }
    // Every field above is touched only under this mutex (dumb host
    // bookkeeping — plans/M15.md item I). Guest rings stay SPSC + DMB.
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

    /// Guest-visible next-core pick: prefer a parked core whose pending
    /// word is already set (a delivered wake — unblocks cross-core await
    /// under Progress-serial replay), then an idle Runnable, wrapping in
    /// ascending core order among the sealed N. Under record this feeds
    /// the Progress log; under replay Progress forces the hand-off and a
    /// mismatch is a named divergence.
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

    /// What a handled exit asks of the scheduler.
    enum Step {
        /// Ordinary exit — this core keeps running.
        Keep,
        /// Release doorbell or park / deadline hand-off.
        /// `release`: Progress stays on this core (bring-up continues).
        /// `park`: mark `Parked` after Progress (deferred for next_core).
        Yield { release: bool, park: bool },
        /// The guest's exit protocol: the image is done.
        Halt(u64),
    }

    // plans/M15.md item D/F: bring-up count is the report's `Cores count=N`
    // (parse defaults to `1 + core_entries.len()` when the line is absent).
    // Decision 1061: refuse anything outside `1..=CORE_SLOTS`.
    let cores_declared = parsed.cores;
    if !(1..=CORE_SLOTS).contains(&cores_declared) {
        return Err(VmmError::MalformedReport(format!(
            "`Cores count={cores_declared}` must satisfy 1..=CORE_SLOTS ({CORE_SLOTS})"
        )));
    }
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
            // Live replay fails closed on the first divergence (06 §8);
            // unit tests of the chooser itself leave `.strict()` off.
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
    });
    // Park-sleep / eligibility / replay-wait / watchdog wake — not a baton.
    let wake = std::sync::Condvar::new();

    /// Sleep until `deadline_ns` (clamped to [`WALL_CAP`] past now), or
    /// until the watchdog/`sched.done` wakes us — **never** while holding
    /// `lock`. Uses `wake.wait_timeout` so the mutex is released for the
    /// whole wait slice and the watchdog can still force-exit.
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

    /// MMIO protocol words are 64-bit only. A sub-word LDRB/LDRH/STRH
    /// against a doorbell or clock is a guest fault, never a silent
    /// truncation/zero-extend (adversarial audit, 2026-07-27).
    fn require_mmword(da: &DataAbort, core: usize, what: &str) -> Result<(), VmmError> {
        if da.size_bytes != 8 {
            return Err(VmmError::GuestFault(format!(
                "core {core}: {what} requires an 8-byte access, got a {}-byte access",
                da.size_bytes
            )));
        }
        Ok(())
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
                    let Some(da) = decode_data_abort(esr) else {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: unhandled access shape at EXIT_MMIO_ADDR (esr={esr:#x})"
                        )));
                    };
                    require_mmword(&da, core, "EXIT_MMIO_ADDR")?;
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
                    require_mmword(&da, core, "CLOCK_MMIO_ADDR")?;
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
                    // plans/M17.md item D / freeze 3+7+8: park-shaped
                    // entropy fill. Guest staged dest/len at
                    // machine_info::OFF_ENTROPY_DEST / OFF_ENTROPY_LEN
                    // (ordinary stores), then this trapping store. VMM
                    // fills [dest, dest+len) via host_ram — not GuestMem
                    // (stack scratch is not a DMA pool window).
                    let Some(da) = decode_data_abort(esr) else {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: unhandled access shape at ENTROPY_MMIO_ADDR \
                             (esr={esr:#x})"
                        )));
                    };
                    require_mmword(&da, core, "ENTROPY_MMIO_ADDR")?;
                    if !da.write {
                        return Err(VmmError::GuestFault(format!(
                            "core {core}: a load from ENTROPY_MMIO_ADDR is not part of the \
                             entropy protocol"
                        )));
                    }
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
                    require_mmword(&da, core, "RELEASE_MMIO_ADDR")?;
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
                    // Decision 1061: release value must equal report
                    // `Cores count` (already in `1..=CORE_SLOTS`).
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
                    require_mmword(&da, core, "QUIESCE_MMIO_ADDR")?;
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
                    require_mmword(&da, core, "PARK_MMIO_ADDR")?;
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
                        //
                        // plans/M15.md item I: under overlapping
                        // `hv_vcpu_run` the producer may raise pending
                        // between the guest's last look and this trap —
                        // the same mask-arm-recheck core 0 already did.
                        // Recheck before parking or a lost wake deadlocks
                        // the machine.
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
                    // gets to run again whether or not anything ever woke it,
                    // so an omitted cross-core wake would still boot green — a
                    // decorative mechanism, found by mutating `waker_tag` and
                    // watching every golden pass. Marking it `Parked` puts its
                    // resumption where the machine's own contract puts it: on
                    // its pending word. Under concurrent record a sibling may
                    // already be running; under Progress replay `next_core`
                    // either finds a runnable sibling, finds this core's own
                    // word already raised, or fails the boot closed with
                    // `no core is runnable` — which is what replaces the
                    // guest-side `DEADLOCK_MSG` for a cross-core image.
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
                        return Ok(Step::Yield {
                            release: false,
                            park: !blk_completed && pending_word(host_ram, core) == 0,
                        });
                    }
                    // Core 0, with a deadline armed. A sibling that can run is
                    // already overlapping under concurrent record; under
                    // Progress replay this Yield hands off before sleeping so
                    // host timing does not decide the schedule (decision 7).
                    // With no runnable sibling (every single-core image,
                    // always) this is exactly the M6 park path, unchanged.
                    {
                        let g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        let sibling = (1..cores_declared).any(|c| {
                            g.sched.state[c] == CoreState::Runnable
                                || (g.sched.state[c] == CoreState::Parked
                                    && pending_word(host_ram, c) != 0)
                        });
                        if sibling {
                            drop(g);
                            // Deadline armed + runnable sibling: hand off
                            // without parking (core 0 stays Runnable).
                            return Ok(Step::Yield {
                                release: false,
                                park: false,
                            });
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
                        // Sleep OUTSIDE the scheduler mutex. Holding the
                        // lock across `thread::sleep(deadline - now)` let a
                        // guest-written `u64::MAX` deadline defeat the
                        // WALL_CAP watchdog (mutex never releasable) —
                        // adversarial audit, 2026-07-27. Replay still skips
                        // the sleep (decision 9): only record mode calls
                        // `live`, and we sleep before recording so the
                        // choice log stays sleep-free under replay.
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
                                // Watchdog fired during the park sleep.
                                // This core is not inside `hv_vcpu_run`
                                // (it is servicing the park trap), so
                                // `hv_vcpus_exit` cannot deliver
                                // `HV_EXIT_REASON_CANCELED` here — name
                                // the timeout ourselves rather than
                                // falling through to "no core reported
                                // the guest exit protocol".
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
                        // The raise itself (06 §4: "a store-release plus
                        // a wake"): a plain host-side write into this
                        // core's own pending word. No separate wake is
                        // needed — resuming this already-exited vCPU on
                        // the next loop iteration below *is* the wake.
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
    /// this thread). plans/M15.md item I: overlapping `hv_vcpu_run` for
    /// every Runnable core; Yield-`Progress` serializes the choice-log
    /// cursor only (`next_core` forced from the log under replay).
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
                    // Decision 7: every Runnable core may enter `hv_vcpu_run`
                    // without waiting on `sched.current` (record and replay).
                    // Enter-serializing replay of a concurrent-record Progress
                    // tape deadlocks cross-core awaits; Progress forces the
                    // Yield choice-log cursor only. Admission replay is a
                    // multiset bag (decision 8). Cap-1 rings may under-count
                    // under overlap — leftover / extra Admissions at finish
                    // are tolerated like leftover Progress (park count is
                    // host-schedule-dependent). Yield-flush recovers a stuck
                    // cursor (Parked holder, empty pending).
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
                    // Decision 7: Progress is a single global tape. Yield
                    // holds the log cursor before Admissions+Progress flush
                    // (record and replay). Enter does not wait on it.
                    //
                    // Cursor recovery (mailbox-depth hang under overlap):
                    // Progress may force `current` onto a Parked core whose
                    // pending word is still clear. Enter stays concurrent, so
                    // a sibling that could raise that wake may itself need to
                    // Yield-flush — and would deadlock waiting on that stuck
                    // cursor until WALL_CAP (no EXIT_MMIO_ADDR). If the holder
                    // cannot reach a Yield, take the cursor.
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
                    // Release: Progress stays on the releasing core so replay
                    // keeps publishing; secondaries overlap under record and
                    // take the cursor on a later park Progress.
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
                                    // Do not plant a stuck cursor: a Progress
                                    // target that is Parked with an empty
                                    // pending word cannot Yield-flush, and
                                    // under concurrent enter that deadlocks
                                    // siblings waiting on the log cursor
                                    // (boot-cross-core-mailbox-depth).
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

    // Core `n`'s own entry address: the report's `Entry base=` for core 0,
    // its own `CoreEntry core=n base=` line for the rest.
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
                // HVF binds a vCPU to its creating thread: create, register,
                // run and destroy all happen right here.
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
                    // Decision 1062: named host-refuse, not a bare Hvf.
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
                    // Decision 1064: SP from report `CoreStack` (top =
                    // base+size). Guest entry also installs SP; the report
                    // is still the VMM's whole config.
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
        let watchdog_wake = &wake;
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
                drop(g);
                // Wake any core parked in `sleep_until_park_deadline`'s
                // Condvar wait — without this, a force-exit alone leaves
                // the sleeper waiting out its current slice.
                watchdog_wake.notify_all();
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
    // Decision 1065: evidence length is the sealed N, not CORE_SLOTS.
    // Marks pack 0x68..0x168; line buf lives at 0x218 — no overlap.
    // Length N (not CORE_SLOTS) is the sealed bring-up count.
    let core_marks = (0..cores_declared)
        .map(|c| read_core_mark(host_ram, c))
        .collect::<Vec<u64>>();
    // Integrity Phase 2 Item N: Lane 3 host snapshot of the placed LANE2
    // page — compared to the guest `lane2 hits=` dump by the agreement
    // oracle (`lane3::agree_lane2_vs_host`).
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
        },
        divergences,
    ))
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
pub(crate) fn boot_image_core(
    _report_path: &Path,
    _img_path: &Path,
    _replay_choices: Option<Vec<record::ChoiceEntry>>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    Err(VmmError::Unsupported(
        "the wrela VMM needs Hypervisor.framework (macOS/aarch64 at M5); no other host is implemented yet",
    ))
}
