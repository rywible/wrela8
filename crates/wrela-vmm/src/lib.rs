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
    /// transcript-so-far shown", never silently discarded).
    Timeout { transcript_so_far: Vec<u8> },
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
            VmmError::Timeout { transcript_so_far } => write!(
                f,
                "timeout after {:?}: {} byte(s) of transcript captured before the forced exit",
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
}

/// The report's own structural facts this VMM actually consumes (module
/// doc's own "whole configuration" — everything else this milestone's
/// report format carries is compiler-internal bookkeeping this VMM never
/// reads). Parsed by `parse_report`, below.
#[derive(Debug)]
struct ParsedReport {
    entry: u64,
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
    let mut has_section = false;
    let mut entry: Option<u64> = None;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("Machine revision=") {
            revision = Some(rest.to_string());
        } else if line.starts_with("Input path=") {
            has_input = true;
        } else if line.starts_with("Section name=") {
            has_section = true;
        } else if let Some(rest) = line.strip_prefix("Entry base=") {
            let digits = rest.trim_start_matches("0x");
            entry = u64::from_str_radix(digits, 16).ok();
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
    if !has_section {
        return Err(VmmError::MalformedReport(
            "no `Section name=` line".to_string(),
        ));
    }
    let entry =
        entry.ok_or_else(|| VmmError::MalformedReport("no `Entry base=0x...` line".to_string()))?;
    Ok(ParsedReport { entry })
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
    use wrela_machine::mmio;

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

    // --- vCPU 0 ---------------------------------------------------------------
    let mut vcpu: u64 = 0;
    let mut exit_ptr: *mut HvVcpuExit = std::ptr::null_mut();
    let r = unsafe { hv_vcpu_create(&mut vcpu, &mut exit_ptr, std::ptr::null_mut()) };
    if r != HV_SUCCESS {
        return Err(VmmError::Hvf {
            call: "hv_vcpu_create",
            code: r,
        });
    }
    struct VcpuGuard(u64);
    impl Drop for VcpuGuard {
        fn drop(&mut self) {
            unsafe {
                hv_vcpu_destroy(self.0);
            }
        }
    }
    let _vcpu_guard = VcpuGuard(vcpu);

    let set_reg = |reg: u32, value: u64| -> Result<(), VmmError> {
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
    // 06 §3: "points x0 at the machine-info page ... and starts vCPU 0 at
    // the image entry." The image's own generated runtime never actually
    // reads x0 today (its own absolute addresses are baked in at compile
    // time — layout.rs's own module doc), but the boot contract is
    // satisfied regardless, for forward compatibility.
    set_reg(hv_reg_xn(0), machine_layout::MACHINE_INFO_BASE)?;
    set_reg(HV_REG_PC, parsed.entry)?;
    // EL1h (`SPSel = 1`), every exception masked (`DAIF = 1111`) — the
    // standard bare-metal AArch64 boot value, plans/M5.md decision text's
    // own "0x3c5".
    set_reg(HV_REG_CPSR, 0x3c5)?;
    let r = unsafe { hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_CPACR_EL1, 0x0030_0000) };
    if r != HV_SUCCESS {
        return Err(VmmError::Hvf {
            call: "hv_vcpu_set_sys_reg(CPACR_EL1)",
            code: r,
        });
    }

    // --- watchdog thread (decision 15's own host-side wall cap) --------------
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let watchdog_vcpu = vcpu;
    let watchdog = std::thread::spawn(move || {
        if done_rx.recv_timeout(WALL_CAP).is_err() {
            // Either a real timeout, or the sender was dropped without a
            // send (an early `return Err` above would drop it) — both
            // cases force the vCPU to exit; a `hv_vcpu_run` that already
            // returned naturally treats this as a no-op per the header's
            // own documented "vcpu not running" behavior.
            let mut vcpus = [watchdog_vcpu];
            unsafe {
                hv_vcpus_exit(vcpus.as_mut_ptr(), 1);
            }
        }
    });
    // Ensures `done_tx` (and therefore the watchdog thread) is always
    // cleaned up on every exit path, including an early `return Err` deep
    // inside the loop below — the same RAII pattern as `VmGuard`/
    // `RamGuard`/`VcpuGuard` above.
    struct WatchdogGuard {
        tx: Option<std::sync::mpsc::Sender<()>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for WatchdogGuard {
        fn drop(&mut self) {
            if let Some(tx) = self.tx.take() {
                let _ = tx.send(());
            }
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }
    let _watchdog_guard = WatchdogGuard {
        tx: Some(done_tx),
        handle: Some(watchdog),
    };

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

    // --- the exit loop ---------------------------------------------------------
    let clock_addr = mmio::CLOCK_MMIO_ADDR;
    let exit_addr = mmio::EXIT_MMIO_ADDR;
    let park_addr = mmio::PARK_MMIO_ADDR;
    let pending_off =
        (wrela_machine::pending::core_word_addr(0) - machine_layout::DRAM_BASE) as usize;
    let deadline_off = (machine_layout::MACHINE_INFO_BASE - machine_layout::DRAM_BASE) as usize
        + machine_info::OFF_NEXT_DEADLINE as usize;
    let mut exits: u64 = 0;
    let exit_code: u64;
    // plans/M6.md item E, decision 9: the single point every
    // nondeterministic decision this loop makes flows through.
    let mut chooser = match replay_choices {
        Some(log) => record::Chooser::replayer(log),
        None => record::Chooser::recorder(),
    };

    loop {
        let r = unsafe { hv_vcpu_run(vcpu) };
        if r != HV_SUCCESS {
            return Err(VmmError::Hvf {
                call: "hv_vcpu_run",
                code: r,
            });
        }
        exits += 1;
        let exit = unsafe { *exit_ptr };
        match exit.reason {
            HV_EXIT_REASON_EXCEPTION => {
                let esr = exit.exception.syndrome;
                let ipa = exit.exception.physical_address;
                if ipa == exit_addr {
                    let Some(da) = decode_data_abort(esr) else {
                        return Err(VmmError::GuestFault(format!(
                            "unhandled access shape at EXIT_MMIO_ADDR (esr={esr:#x})"
                        )));
                    };
                    if !da.write {
                        return Err(VmmError::GuestFault(
                            "a load from EXIT_MMIO_ADDR is not part of the exit protocol"
                                .to_string(),
                        ));
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
                    exit_code = value;
                    break;
                } else if ipa == clock_addr {
                    let Some(da) = decode_data_abort(esr) else {
                        return Err(VmmError::GuestFault(format!(
                            "unhandled access shape at CLOCK_MMIO_ADDR (esr={esr:#x})"
                        )));
                    };
                    if da.write {
                        return Err(VmmError::GuestFault(
                            "a store to CLOCK_MMIO_ADDR is not part of the clock protocol"
                                .to_string(),
                        ));
                    }
                    // plans/M6.md item E, decision 9: the single point of
                    // choice — record produces a fresh live read, replay
                    // consumes the next logged one (never re-reading the
                    // real clock).
                    let entry = chooser.choose_next(record::ChoiceRequest::ClockRead, || {
                        record::ChoiceEntry::ClockRead {
                            value: monotonic_ns(),
                        }
                    });
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
                } else if ipa == park_addr {
                    // plans/M6.md item E, decision 7/06 §5: the park
                    // protocol's own doorbell (`mmio::PARK_MMIO_ADDR`'s
                    // own module doc has the whole contract).
                    let Some(da) = decode_data_abort(esr) else {
                        return Err(VmmError::GuestFault(format!(
                            "unhandled access shape at PARK_MMIO_ADDR (esr={esr:#x})"
                        )));
                    };
                    if !da.write {
                        return Err(VmmError::GuestFault(
                            "a load from PARK_MMIO_ADDR is not part of the park protocol"
                                .to_string(),
                        ));
                    }
                    // Advance PC now — the guest resumes right after its
                    // own trapping store the moment this vCPU is next run,
                    // whether or not this park ends up sleeping at all.
                    advance_pc(vcpu)?;
                    let deadline_ns = unsafe {
                        let mut b = [0u8; 8];
                        std::ptr::copy_nonoverlapping(
                            host_ram.add(deadline_off),
                            b.as_mut_ptr(),
                            8,
                        );
                        u64::from_le_bytes(b)
                    };
                    // The mask-arm-recheck discipline's own "recheck"
                    // half (`mmio::PARK_MMIO_ADDR`'s own doc): a vector
                    // already pending at the moment of this trap means a
                    // wake already happened (or was never needed) — do
                    // not sleep at all, so it is never lost.
                    let already_pending = unsafe {
                        let mut b = [0u8; 8];
                        std::ptr::copy_nonoverlapping(host_ram.add(pending_off), b.as_mut_ptr(), 8);
                        u64::from_le_bytes(b) != 0
                    };
                    if !already_pending {
                        chooser.choose_next(
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
                        chooser
                            .choose_next(record::ChoiceRequest::VectorRaise { vector: 0 }, || {
                                record::ChoiceEntry::VectorRaise { vector: 0 }
                            });
                        // The raise itself (06 §4: "a store-release plus
                        // a wake"): a plain host-side write into this
                        // core's own pending word. No separate wake is
                        // needed — resuming this already-exited vCPU on
                        // the next loop iteration below *is* the wake.
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                1u64.to_le_bytes().as_ptr(),
                                host_ram.add(pending_off),
                                8,
                            );
                        }
                    }
                } else if let Some(imm) = decode_brk(esr) {
                    let pc = read_pc(vcpu).unwrap_or(0);
                    return Err(VmmError::GuestFault(format!(
                        "unexpected `BRK #{imm}` (esr={esr:#x}, ipa={ipa:#x}, pc={pc:#x})"
                    )));
                } else {
                    let pc = read_pc(vcpu).unwrap_or(0);
                    return Err(VmmError::GuestFault(format!(
                        "unhandled exception (esr={esr:#x}, ipa={ipa:#x}, pc={pc:#x})"
                    )));
                }
            }
            HV_EXIT_REASON_CANCELED => {
                let transcript_so_far = drain_console(host_ram);
                return Err(VmmError::Timeout { transcript_so_far });
            }
            other => {
                return Err(VmmError::GuestFault(format!(
                    "unexpected hv_exit_reason_t {other}"
                )));
            }
        }
    }

    // decision 12: the transcript is read from the ring pages only after the
    // guest halts.
    let transcript = drain_console(host_ram);
    let (choices, divergences) = record::finish_chooser(chooser);
    Ok((
        BootOutcome {
            transcript,
            exit_code,
            choices,
            exits,
        },
        divergences,
    ))
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
    epoch.elapsed().as_nanos() as u64
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

pub mod devices {
    //! Device models for the closed machine v1 set (06 §6). At M5, the
    //! console tx-ring drain and the clock MMIO trap live directly in
    //! `boot_image`'s own exit loop (two devices, dumbest-correct, no seam
    //! yet — CLAUDE.md's "no traits with one implementation"); a real
    //! per-device module split arrives with the device milestones, once
    //! there is more than one device model each to justify it.
}

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
    // M5-E way — real HVF boots of hand-assembled guest programs, reusing
    // `wrela_compiler::layout`'s own `build_rt_enqueue`/
    // `build_rt_select_and_run` (the exact machinery a real image will use,
    // not a re-derivation of it) exactly the way this file's own
    // `record_replay_roundtrips...` test above already reuses
    // `wrela_compiler::encode`. No compiled `.wr` source can reach this
    // machinery yet (async lowering is items B/D's own in-flight/future
    // work — `layout.rs`'s own item-C module doc explains why), so every
    // "actor method" here is a tiny hand-assembled stand-in, exactly like
    // this file's own clock test already hand-assembles a whole guest
    // program with no compiler involved at all.

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

    /// Enqueue -> select -> run a sync method end-to-end, FIFO order
    /// across two queued messages, and the ring-full rejection path — all
    /// three of item C's own conformance goals for admission/selection in
    /// one boot (mirrors this file's own `record_replay_...` test's "every
    /// real boot happens sequentially within one `#[test]` fn" reasoning,
    /// one level down: one guest program, several checks, one exit code).
    ///
    /// Two stand-in "actor methods" (`x0 = x1 + 1`/`x0 = x1 + 2`, self in
    /// `x0` unread — the exact scalar-receiver ABI shape a real compiled
    /// method already uses) stand in for `Store.bump`-shaped `pub fn`s.
    /// The guest's own entry: writes `10`/`20`/`30` in turn into one
    /// shared arg-scratch word, calls `rt_enqueue` three times (method 0,
    /// method 1, method 0 again — the third over `capacity=2`; every
    /// admission names a stand-in **waker record**, the park-and-resume
    /// delivery target), then calls `rt_select_and_run` three times
    /// (reading each delivered reply from the waker record's own
    /// `OFF_TURN_REPLY` slot — the placeholder-era `last_result` side
    /// channel no longer exists), and finally folds every
    /// expected-vs-actual comparison into one exit code via
    /// `check_eq_into` (branch-free) before the trapping exit store.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn actor_runtime_enqueue_select_fifo_and_ring_full_over_hvf() {
        use wrela_compiler::codegen::OFF_TURN_REPLY;
        use wrela_compiler::encode;
        use wrela_compiler::layout::{ActorAddrs, build_rt_enqueue, build_rt_select_and_run};
        use wrela_machine::layout as machine_layout;

        let capacity: u64 = 2;
        let slot_size: u64 = 24; // method tag + waker + one 8-byte scalar arg
        let state_size: u64 = 8;
        let sp_top = machine_layout::core_stack_base(0) + machine_layout::CORE_STACK_SIZE;

        let method0 = vec![encode::enc_add_imm(0, 1, 1, true), encode::enc_ret(30)]; // arg + 1
        let method1 = vec![encode::enc_add_imm(0, 1, 2, true), encode::enc_ret(30)]; // arg + 2

        // Builds the whole entry sequence — addr-value-independent in
        // length (every embedded constant is a fixed-width `load_imm` or
        // a single relative `bl`), so it is safe to call once with
        // placeholder indices purely to measure its own word count, then
        // again with the real ones once every other fragment's own
        // position is known (this file's own two-pass approach; `Asm`,
        // the identical technique `layout.rs` itself uses for relocation,
        // is private to that crate — not worth exposing for one test).
        fn build_entry(
            sp_top: u64,
            arg_scratch_addr: u64,
            waker_addr: u64,
            enqueue_word_idx: usize,
            select_word_idx: usize,
        ) -> Vec<u32> {
            let mut w = Vec::new();
            w.extend(load_imm_words(9, sp_top));
            w.push(encode::enc_add_imm(31, 9, 0, true));

            let enqueue_call = |w: &mut Vec<u32>, method_idx: u16, value: u64, save_to: u8| {
                w.extend(load_imm_words(9, arg_scratch_addr));
                w.extend(load_imm_words(10, value));
                w.push(encode::enc_str_x_imm(10, 9, 0));
                w.push(encode::enc_movz(0, method_idx, 0, true));
                w.extend(load_imm_words(1, arg_scratch_addr));
                w.push(encode::enc_movz(2, 1, 0, true));
                w.extend(load_imm_words(3, waker_addr));
                let this = w.len();
                let delta = (enqueue_word_idx as i64 - this as i64) * 4;
                w.push(encode::enc_bl(delta as i32));
                w.push(encode::enc_mov_reg(save_to, 0, true));
            };
            enqueue_call(&mut w, 0, 10, 19);
            enqueue_call(&mut w, 1, 20, 20);
            enqueue_call(&mut w, 0, 30, 21);

            let select_call = |w: &mut Vec<u32>, save_ran_to: u8, save_result_to: u8| {
                let this = w.len();
                let delta = (select_word_idx as i64 - this as i64) * 4;
                w.push(encode::enc_bl(delta as i32));
                w.push(encode::enc_mov_reg(save_ran_to, 0, true));
                w.extend(load_imm_words(9, waker_addr + OFF_TURN_REPLY));
                w.push(encode::enc_ldr_x_imm(save_result_to, 9, 0));
            };
            select_call(&mut w, 22, 23); // x22 = ran1, x23 = delivered reply 1
            select_call(&mut w, 24, 25); // x24 = ran2, x25 = delivered reply 2
            select_call(&mut w, 26, 27); // x26 = ran3 (idle expected); reply unread

            w.push(encode::enc_movz(9, 0, 0, true)); // x9 = fail accumulator
            w.extend(check_eq_into(9, 10, 19, 0, 0)); // outcome0 == 0 (admitted)
            w.extend(check_eq_into(9, 10, 20, 0, 1)); // outcome1 == 0 (admitted)
            w.extend(check_eq_into(9, 10, 21, 1, 2)); // outcome2 == 1 (rejected: ring full)
            w.extend(check_eq_into(9, 10, 22, 1, 3)); // ran1 == 1
            w.extend(check_eq_into(9, 10, 23, 11, 4)); // reply1 == 11 (method 0, arg 10), delivered to the waker
            w.extend(check_eq_into(9, 10, 24, 1, 5)); // ran2 == 1
            w.extend(check_eq_into(9, 10, 25, 22, 6)); // reply2 == 22 (method 1, arg 20) — FIFO order
            w.extend(check_eq_into(9, 10, 26, 0, 7)); // ran3 == 0 (mailbox now empty)

            w.extend(load_imm_words(11, wrela_machine::mmio::EXIT_MMIO_ADDR));
            w.push(encode::enc_str_x_imm(9, 11, 0));
            w.push(encode::enc_brk(0));
            w
        }

        // Pass 1: placeholder indices, to learn `entry`'s own word count
        // (length is provably addr-value-independent, module doc above).
        let entry_len = build_entry(sp_top, 0, 0, 0, 0).len();
        let placeholder = ActorAddrs {
            state: 0,
            ring: 0,
            head: 0,
            tail: 0,
            count: 0,
            turn: 0,
        };
        let enqueue_len = build_rt_enqueue(&placeholder, capacity, slot_size, 0).len();
        let select_len = build_rt_select_and_run(
            &placeholder,
            capacity,
            slot_size,
            &[(0, false), (0, false)],
            0,
        )
        .len();

        let method0_word_idx = entry_len;
        let method1_word_idx = method0_word_idx + method0.len();
        let enqueue_word_idx = method1_word_idx + method1.len();
        let select_word_idx = enqueue_word_idx + enqueue_len;
        let code_words_total = select_word_idx + select_len;

        let rtdata_base = round_up8(machine_layout::IMAGE_BASE + (code_words_total as u64) * 4);
        let addrs = ActorAddrs {
            state: rtdata_base,
            ring: rtdata_base + state_size,
            head: rtdata_base + state_size + capacity * slot_size,
            tail: rtdata_base + state_size + capacity * slot_size + 8,
            count: rtdata_base + state_size + capacity * slot_size + 16,
            turn: rtdata_base + state_size + capacity * slot_size + 24,
        };
        let turn_area_end = addrs.turn + wrela_compiler::codegen::TURN_RECORD_SIZE;
        let waker_addr = turn_area_end; // a detached stand-in waker record
        let arg_scratch_addr = waker_addr + wrela_compiler::codegen::TURN_RECORD_SIZE;
        let rtdata_bytes = (arg_scratch_addr + 8 - rtdata_base) as usize;

        let entry = build_entry(
            sp_top,
            arg_scratch_addr,
            waker_addr,
            enqueue_word_idx,
            select_word_idx,
        );
        assert_eq!(
            entry.len(),
            entry_len,
            "entry's own length must not depend on the real addresses"
        );
        let enqueue_words = build_rt_enqueue(&addrs, capacity, slot_size, enqueue_word_idx);
        let select_words = build_rt_select_and_run(
            &addrs,
            capacity,
            slot_size,
            &[(method0_word_idx, false), (method1_word_idx, false)],
            select_word_idx,
        );

        let mut words = Vec::new();
        words.extend(entry);
        words.extend(method0);
        words.extend(method1);
        words.extend(enqueue_words);
        words.extend(select_words);
        assert_eq!(words.len(), code_words_total);

        let mut img_bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let rtdata_end = (rtdata_base - machine_layout::IMAGE_BASE) as usize + rtdata_bytes;
        img_bytes.resize(rtdata_end, 0);

        let outcome = boot_hand_built_image(&img_bytes, "actor-runtime");
        assert_eq!(
            outcome.exit_code, 0,
            "every check bit must be 0 (a nonzero bit names which check failed, decoded: \
             1=admit#1 2=admit#2 4=ring-full 8=ran#1 16=reply#1 32=ran#2 64=reply#2/FIFO 128=idle#3)"
        );
    }

    /// Decision 12 (abandon = image-fatal at M6): an actor turn that
    /// aborts must never resume — the image exits nonzero, deterministically,
    /// full stop. Since this item's own turn-kind-aware `__wrela_abort`
    /// routing (naming *which* actor faulted on the console, the way a
    /// real compiled program's abort eventually will) is staged follow-up
    /// work once item D's real dispatch exists, this test proves the
    /// structural half directly: a dispatched "method" that performs the
    /// machine's own halt sequence (the identical `push_halt` shape every
    /// M5 abort stub already uses) instead of an ordinary `ret` — the
    /// image exits nonzero and `rt_select_and_run` never regains control
    /// (there is nothing to observe from a resumed turn at all: the guest
    /// is gone the instant the trapping `EXIT_MMIO_ADDR` store lands).
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn actor_turn_abandon_exits_the_image_nonzero_and_never_resumes() {
        use wrela_compiler::encode;
        use wrela_compiler::layout::{ActorAddrs, build_rt_enqueue, build_rt_select_and_run};
        use wrela_machine::layout as machine_layout;

        const ABANDON_EXIT_CODE: u64 = 0x7;
        let capacity: u64 = 1;
        let slot_size: u64 = 16; // no-arg method: tag + waker only
        let state_size: u64 = 8;
        let sp_top = machine_layout::core_stack_base(0) + machine_layout::CORE_STACK_SIZE;

        // The one stand-in "actor method": aborts the turn directly
        // instead of returning (module doc above).
        let mut aborting_method = Vec::new();
        push_halt(&mut aborting_method, 9, 10, ABANDON_EXIT_CODE);

        fn build_entry(
            sp_top: u64,
            arg_scratch_addr: u64,
            enqueue_word_idx: usize,
            select_word_idx: usize,
        ) -> Vec<u32> {
            let mut w = Vec::new();
            w.extend(load_imm_words(9, sp_top));
            w.push(encode::enc_add_imm(31, 9, 0, true));

            // enqueue(method_idx=0, args_ptr=arg_scratch, nargs=0,
            // waker=0 — a one-way message; nothing awaits it)
            w.extend(load_imm_words(0, 0));
            w.extend(load_imm_words(1, arg_scratch_addr));
            w.extend(load_imm_words(2, 0));
            w.extend(load_imm_words(3, 0));
            let this = w.len();
            let delta = (enqueue_word_idx as i64 - this as i64) * 4;
            w.push(encode::enc_bl(delta as i32));

            // select() -> dispatches the aborting method; execution never
            // returns here if the abandon path is real (the trapping
            // store ends the guest mid-call).
            let this = w.len();
            let delta = (select_word_idx as i64 - this as i64) * 4;
            w.push(encode::enc_bl(delta as i32));

            // Unreachable if the abandon path works: a *different*,
            // named exit code proves the guest wrongly resumed instead
            // of the dispatched method's own abort taking effect.
            push_halt(&mut w, 9, 10, 0xBAD);
            w
        }

        let entry_len = build_entry(0, 0, 0, 0).len();
        let placeholder = ActorAddrs {
            state: 0,
            ring: 0,
            head: 0,
            tail: 0,
            count: 0,
            turn: 0,
        };
        let enqueue_len = build_rt_enqueue(&placeholder, capacity, slot_size, 0).len();
        let select_len =
            build_rt_select_and_run(&placeholder, capacity, slot_size, &[(0, false)], 0).len();

        let method0_word_idx = entry_len;
        let enqueue_word_idx = method0_word_idx + aborting_method.len();
        let select_word_idx = enqueue_word_idx + enqueue_len;
        let code_words_total = select_word_idx + select_len;

        let rtdata_base = round_up8(machine_layout::IMAGE_BASE + (code_words_total as u64) * 4);
        let addrs = ActorAddrs {
            state: rtdata_base,
            ring: rtdata_base + state_size,
            head: rtdata_base + state_size + capacity * slot_size,
            tail: rtdata_base + state_size + capacity * slot_size + 8,
            count: rtdata_base + state_size + capacity * slot_size + 16,
            turn: rtdata_base + state_size + capacity * slot_size + 24,
        };
        let arg_scratch_addr = addrs.turn + wrela_compiler::codegen::TURN_RECORD_SIZE;
        let rtdata_bytes = (arg_scratch_addr + 8 - rtdata_base) as usize;

        let entry = build_entry(sp_top, arg_scratch_addr, enqueue_word_idx, select_word_idx);
        assert_eq!(entry.len(), entry_len);
        let enqueue_words = build_rt_enqueue(&addrs, capacity, slot_size, enqueue_word_idx);
        let select_words = build_rt_select_and_run(
            &addrs,
            capacity,
            slot_size,
            &[(method0_word_idx, false)],
            select_word_idx,
        );

        let mut words = Vec::new();
        words.extend(entry);
        words.extend(aborting_method);
        words.extend(enqueue_words);
        words.extend(select_words);
        assert_eq!(words.len(), code_words_total);

        let mut img_bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let rtdata_end = (rtdata_base - machine_layout::IMAGE_BASE) as usize + rtdata_bytes;
        img_bytes.resize(rtdata_end, 0);

        let outcome = boot_hand_built_image(&img_bytes, "actor-abandon");
        assert_eq!(
            outcome.exit_code, ABANDON_EXIT_CODE,
            "the aborting method's own exit code must win — the guest must never resume \
             `rt_select_and_run` (which would instead halt with the unreachable 0xBAD marker)"
        );
    }

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
        use wrela_compiler::{codegen, layout};

        let tokens = wrela_compiler::syntax::lexer::lex(src).expect("conformance source must lex");
        let module = wrela_compiler::syntax::parser::parse(tokens).expect("must parse");
        let program =
            wrela_compiler::sema::check_typed(&module, "<conformance>").expect("must check");
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

        let mut modules = BTreeMap::new();
        modules.insert(module.path.join("."), module.clone());
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
        let codegen_program = codegen::codegen_program_with_async(
            &mwir_program,
            &flow_program,
            &layout_ctx,
            &method_index,
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
        let boot = layout::BootCtx {
            graph: &graph,
            modules: &modules,
            layout_ctx: &layout_ctx,
            async_frames: &async_frames,
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
    /// bounded spinning loop (never parks) calls
    /// `__wrela_checkpoint_service` — the real production routine,
    /// embedded verbatim via `build_checkpoint_and_vector_stub` — at
    /// every back-edge, exactly like a compiled loop's own checkpoint
    /// (`codegen::FnCtx::checkpoint`). A background host thread (this
    /// crate's own `test_delayed_raise` conformance seam — module doc on
    /// `boot_image_core`) raises vector 0 mid-run, entirely independently
    /// of the park protocol (the guest is actively running, never
    /// parked) — modeling "the VMM raises a vector while the guest is
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
        use wrela_compiler::layout::build_checkpoint_and_vector_stub;
        use wrela_machine::{layout as machine_layout, machine_info, pending};

        const LOOP_BOUND: u64 = 200_000_000;
        let sp_top = machine_layout::core_stack_base(0) + machine_layout::CORE_STACK_SIZE;
        let (cp_words, cp_entry_offset) = build_checkpoint_and_vector_stub();

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
}
