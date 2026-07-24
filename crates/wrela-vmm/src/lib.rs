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
    pub clock_log: Vec<u64>,
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

    // --- the exit loop ---------------------------------------------------------
    let clock_addr = mmio::CLOCK_MMIO_ADDR;
    let exit_addr = mmio::EXIT_MMIO_ADDR;
    let mut clock_log = Vec::new();
    let mut exits: u64 = 0;
    let exit_code: u64;

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
                    let ns = monotonic_ns();
                    clock_log.push(ns);
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
    Ok(BootOutcome {
        transcript,
        exit_code,
        clock_log,
        exits,
    })
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

pub mod record {
    //! Recorder/replayer for the determinism boundary (06 §8).
    //! `BootOutcome::clock_log` is this milestone's own down payment (the
    //! clock-read log half of the record boundary); a real replay harness
    //! arrives with plans/M5.md item F.
}

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
}
