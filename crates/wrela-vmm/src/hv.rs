//! Raw Hypervisor.framework FFI (plans/M5.md decision 11: "the VMM is raw
//! HVF FFI, no dependency" — CLAUDE.md's own liability rule, and the smoke
//! probe already proved the ~10 calls needed suffice). No bindgen: every
//! signature, struct layout, and constant value below is copied by hand
//! from Apple's own SDK headers (checked directly on this machine —
//! `.../MacOSX.sdk/System/Library/Frameworks/Hypervisor.framework/Versions/
//! A/Headers/{hv_vm.h,hv_vcpu.h,hv_vcpu_types.h}` and
//! `.../MacOSX.sdk/usr/include/arm64/hv/hv_kern_types.h` for the arm64
//! `hv_return_t`/`hv_memory_flags_t` constants, which live in a different
//! header than the x86_64 ones of the same name) — never guessed from
//! memory, since a wrong system-register constant would fail loudly
//! (`HV_BAD_ARGUMENT`) rather than silently, but is worth getting right the
//! first time regardless.
//!
//! Gated to exactly the one target this milestone's flagship VMM host is
//! (macOS on Apple Silicon): `#[cfg(all(target_os = "macos", target_arch =
//! "aarch64"))]`. `lib.rs` supplies a fail-closed stub for every other
//! target — this module never compiles there at all.
//!
//! Types are plain `#[repr(C)]`, matching the ARM ARM/Apple SDK bit-for-bit:
//! - `hv_return_t` is `mach_error_t` (a signed 32-bit `kern_return_t`-shaped
//!   value); `HV_SUCCESS == 0`, every failure code a distinct nonzero value
//!   from `arm64/hv/hv_kern_types.h`.
//! - `hv_reg_t` (`OS_ENUM(..., uint32_t, ...)`): `HV_REG_X0..HV_REG_X30`
//!   are exactly their own index (`0..30`), `HV_REG_PC == 31`,
//!   `HV_REG_CPSR == 34`.
//! - `hv_sys_reg_t` (`OS_ENUM(..., uint16_t, ...)`): only
//!   `HV_SYS_REG_CPACR_EL1 = 0xc082` is needed at M5 (FP/SIMD enable);
//!   `HV_SYS_REG_SP_EL1 = 0xe208` is set at boot from the report's
//!   `CoreStack` lines (plans/M15.md item F / decision 1064) before the
//!   guest entry also installs SP — the report remains the VMM's whole
//!   config; guest install is belt-and-suspenders.

use std::ffi::c_void;

// --- return codes (arm64/hv/hv_kern_types.h) --------------------------------

pub const HV_SUCCESS: i32 = 0;

/// Renders a raw `hv_return_t` for an error message — the six named
/// failure codes this module's own callers can plausibly hit, plus a
/// fallback for anything else (`HV_ILLEGAL_GUEST_STATE`/`HV_EXISTS`
/// included even though nothing here is expected to trigger them, so a
/// future caller's error message is never a bare hex code for a code this
/// module already knows the name of).
pub fn describe_hv_return(code: i32) -> String {
    let code = code as u32;
    match code {
        0xfae9_4001 => "HV_ERROR".to_string(),
        0xfae9_4002 => "HV_BUSY".to_string(),
        0xfae9_4003 => "HV_BAD_ARGUMENT".to_string(),
        0xfae9_4004 => "HV_ILLEGAL_GUEST_STATE".to_string(),
        0xfae9_4005 => "HV_NO_RESOURCES".to_string(),
        0xfae9_4006 => "HV_NO_DEVICE".to_string(),
        0xfae9_4007 => "HV_DENIED".to_string(),
        0xfae9_4008 => "HV_EXISTS".to_string(),
        0xfae9_400f => "HV_UNSUPPORTED".to_string(),
        other => format!("hv_return_t({other:#x})"),
    }
}

// --- memory permissions (hv_vm_map/hv_vm_protect) ---------------------------

pub const HV_MEMORY_READ: u64 = 1 << 0;
pub const HV_MEMORY_WRITE: u64 = 1 << 1;
pub const HV_MEMORY_EXEC: u64 = 1 << 2;

// --- register IDs (hv_vcpu_types.h's `hv_reg_t`) ----------------------------

pub const HV_REG_PC: u32 = 31;
pub const HV_REG_CPSR: u32 = 34;

/// `HV_REG_X0..HV_REG_X30` are exactly their own index; `31` denotes `PC`
/// in this enum's numbering (never a general register), so callers must
/// never pass `31` here — the one place that matters is the ESR decoder's
/// own `SRT == 31` case (architectural `XZR`, not a real register), which
/// `hv.rs`'s own exit-loop caller special-cases before ever reaching this
/// function.
pub fn hv_reg_xn(n: u32) -> u32 {
    debug_assert!(n <= 30, "x{n} is out of range for a plain GPR");
    n
}

// --- system register IDs (hv_vcpu_types.h's `hv_sys_reg_t`) -----------------

pub const HV_SYS_REG_CPACR_EL1: u16 = 0xc082;
/// `SP_EL1` — boot SP init from report `CoreStack` (plans/M15.md item F).
/// CPSR boots as EL1h (`SPSel = 1`), so this is the architectural SP the
/// guest sees at entry.
pub const HV_SYS_REG_SP_EL1: u16 = 0xe208;
/// Synthetic `hv_return_t` used by the create-failure inject (item F
/// host-refuse unit test). Matches Apple's `HV_NO_RESOURCES`.
pub const HV_NO_RESOURCES: i32 = 0xfae9_4005u32 as i32;
/// plans/M6.md item F: the three EL1 exception-state registers a guest's
/// own *first* fault leaves behind when it vectors into an unmapped
/// vector table (`lib.rs::el1_exception_note`'s own doc comment has the
/// whole mechanism). Read-only diagnostics — nothing here ever writes
/// them, and the VMM's own model of the machine is unchanged by their
/// existence (06-machine.md §4: there is no emulated GIC and no VBAR the
/// guest is expected to install).
pub const HV_SYS_REG_ESR_EL1: u16 = 0xc290;
pub const HV_SYS_REG_ELR_EL1: u16 = 0xc201;
pub const HV_SYS_REG_FAR_EL1: u16 = 0xc300;
pub const HV_SYS_REG_VBAR_EL1: u16 = 0xc600;

// --- exit reason (hv_vcpu_types.h's `hv_exit_reason_t`) ---------------------

pub const HV_EXIT_REASON_CANCELED: u32 = 0;
pub const HV_EXIT_REASON_EXCEPTION: u32 = 1;
pub const HV_EXIT_REASON_VTIMER_ACTIVATED: u32 = 2;
pub const HV_EXIT_REASON_UNKNOWN: u32 = 3;

// --- exit info struct (hv_vcpu_types.h) -------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HvVcpuExitException {
    pub syndrome: u64,
    pub virtual_address: u64,
    pub physical_address: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HvVcpuExit {
    pub reason: u32,
    pub exception: HvVcpuExitException,
}

// --- raw FFI surface (hv_vm.h, hv_vcpu.h) -----------------------------------
//
// Every one of these is the exact ~10-call surface plans/M5.md decision 11
// names: vm create/destroy, memory map, vcpu create/destroy/run/exit-info
// (folded into `hv_vcpu_create`'s own out-param, per the real header —
// there is no separate `hv_vcpu_exit_info` call), register get/set
// (`hv_vcpu_get_reg`/`hv_vcpu_set_reg`/`hv_vcpu_set_sys_reg`/, from
// plans/M6.md item F, `hv_vcpu_get_sys_reg` — read-only, diagnostics
// only), plus `hv_vcpus_exit` (decision 15's own watchdog force-exit
// call). Linked via the `Hypervisor` framework (the smoke probe's own
// exact recipe).

#[link(name = "Hypervisor", kind = "framework")]
unsafe extern "C" {
    pub fn hv_vm_create(config: *mut c_void) -> i32;
    pub fn hv_vm_destroy() -> i32;
    pub fn hv_vm_map(addr: *mut c_void, ipa: u64, size: usize, flags: u64) -> i32;
    /// Change permissions on an already-mapped GPA range (`hv_vm.h`).
    pub fn hv_vm_protect(ipa: u64, size: usize, flags: u64) -> i32;

    pub fn hv_vcpu_create(vcpu: *mut u64, exit: *mut *mut HvVcpuExit, config: *mut c_void) -> i32;
    pub fn hv_vcpu_destroy(vcpu: u64) -> i32;
    pub fn hv_vcpu_get_reg(vcpu: u64, reg: u32, value: *mut u64) -> i32;
    pub fn hv_vcpu_set_reg(vcpu: u64, reg: u32, value: u64) -> i32;
    pub fn hv_vcpu_set_sys_reg(vcpu: u64, reg: u16, value: u64) -> i32;
    pub fn hv_vcpu_get_sys_reg(vcpu: u64, reg: u16, value: *mut u64) -> i32;
    /// Raw FFI. Callers go through [`hv_vcpu_run`], which tracks overlap
    /// depth as a test metric (plans/M15.md item I).
    #[link_name = "hv_vcpu_run"]
    fn hv_vcpu_run_raw(vcpu: u64) -> i32;
    pub fn hv_vcpus_exit(vcpus: *mut u64, vcpu_count: u32) -> i32;
}

/// plans/M15.md item I: count of host threads currently inside the raw
/// `hv_vcpu_run`. Under overlapping execution this is a test metric only
/// (depth > 1 is expected and required proof of concurrency); there is no
/// live assert that it stay at 0-or-1.
static HV_VCPU_RUN_DEPTH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Peak value of [`hv_vcpu_run_depth`] since process start / last reset.
/// Concurrent boots must observe a peak > 1 (item I overlap proof).
static HV_VCPU_RUN_DEPTH_MAX: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// How many host threads are inside [`hv_vcpu_run`] right now.
pub fn hv_vcpu_run_depth() -> usize {
    HV_VCPU_RUN_DEPTH.load(std::sync::atomic::Ordering::SeqCst)
}

/// Peak overlap depth observed so far.
pub fn hv_vcpu_run_depth_max() -> usize {
    HV_VCPU_RUN_DEPTH_MAX.load(std::sync::atomic::Ordering::SeqCst)
}

/// Reset the peak counter (unit tests that need a clean overlap window).
pub fn hv_vcpu_run_depth_max_reset() {
    HV_VCPU_RUN_DEPTH_MAX.store(0, std::sync::atomic::Ordering::SeqCst);
}

/// Enter the guest on `vcpu`. Plans/M15.md item I deleted the baton: every
/// released Runnable core may be inside this call at once. The depth
/// counter remains so tests can prove overlap (peak > 1) and quiescence
/// (depth == 0 outside any call).
///
/// # Safety
/// Same contract as the Hypervisor.framework `hv_vcpu_run`: `vcpu` must
/// be a live handle created on this thread.
pub unsafe fn hv_vcpu_run(vcpu: u64) -> i32 {
    use std::sync::atomic::Ordering;
    let prev = HV_VCPU_RUN_DEPTH.fetch_add(1, Ordering::SeqCst);
    let depth = prev + 1;
    HV_VCPU_RUN_DEPTH_MAX.fetch_max(depth, Ordering::SeqCst);
    let r = unsafe { hv_vcpu_run_raw(vcpu) };
    let left = HV_VCPU_RUN_DEPTH.fetch_sub(1, Ordering::SeqCst);
    debug_assert!(
        left >= 1,
        "hv_vcpu_run depth counter underflowed"
    );
    r
}

// --- ESR decoder (item E's own "keep it tiny + unit-tested" requirement) --
//
// ARM ARM "ESR_ELx" layout: `EC` (Exception Class) is bits `[31:26]`, `IL`
// (instruction length: 1 = the trapping instruction was 32 bits, always
// true for this machine's own A64-only ISA) is bit `[25]`, and the
// remaining `[24:0]` is `ISS` (Instruction-Specific Syndrome), whose own
// shape depends on `EC`. Two `EC` values matter to this VMM: `0x24`/`0x25`
// (Data Abort, from a lower EL / without a change in EL — both routed to
// the host under HVF's stage-2 trap model) and `0x3C` (`BRK` instruction
// execution in AArch64 state).

fn exception_class(esr: u64) -> u32 {
    ((esr >> 26) & 0x3F) as u32
}

const EC_DATA_ABORT_LOWER_EL: u32 = 0x24;
const EC_DATA_ABORT_SAME_EL: u32 = 0x25;
const EC_BRK: u32 = 0x3C;

/// A decoded Data Abort ISS (`EC` `0x24`/`0x25`) whose "instruction
/// syndrome" half is valid (`ISV == 1` — true for every plain `LDR`/`STR`
/// this machine's own codegen ever emits against MMIO, since those are
/// exactly the single-register load/store forms the ARM ARM guarantees
/// `ISV` for). `reg` is the ARM ARM's own `SRT` field: the register that
/// held the store's value (a write) or receives the load's result (a
/// read) — **already resolved to `None` when `SRT == 31`**, the
/// architectural `XZR` case (never a request to read/write `PC`, which
/// `hv_reg_t`'s own numbering would otherwise collide with at index `31`;
/// `hv.rs`'s own exit loop must never call `hv_vcpu_get_reg`/`set_reg`
/// with an `SRT`-derived `31`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataAbort {
    pub write: bool,
    /// `None` when `SRT == 31` (`XZR`): a store's value is architecturally
    /// zero; a load's destination is simply discarded.
    pub reg: Option<u32>,
    pub size_bytes: u32,
}

/// `None` when `esr` is not a Data Abort, or when `ISV == 0` (the trapping
/// instruction was not a plain single-register load/store — this VMM has
/// no other MMIO-touching instruction shape to decode, so this case is
/// reported as an unhandled exception by the caller, never guessed at).
pub fn decode_data_abort(esr: u64) -> Option<DataAbort> {
    let ec = exception_class(esr);
    if ec != EC_DATA_ABORT_LOWER_EL && ec != EC_DATA_ABORT_SAME_EL {
        return None;
    }
    let iss = esr & 0x01FF_FFFF;
    let isv = (iss >> 24) & 1 != 0;
    if !isv {
        return None;
    }
    let sas = (iss >> 22) & 0b11;
    let srt = ((iss >> 16) & 0b1_1111) as u32;
    let wnr = (iss >> 6) & 1 != 0;
    let size_bytes = match sas {
        0b00 => 1,
        0b01 => 2,
        0b10 => 4,
        0b11 => 8,
        _ => unreachable!("2-bit field"),
    };
    Some(DataAbort {
        write: wnr,
        reg: if srt == 31 { None } else { Some(srt) },
        size_bytes,
    })
}

/// `Some(imm16)` when `esr` is a `BRK #imm16` exception (`EC == 0x3C`,
/// `ISS[15:0]` the immediate) — decision E's own "a bare BRK = guest
/// fault, report it" case: reachable only if guest execution ever
/// continues past the exit-code `STR` trap without the host tearing the
/// vCPU down (this VMM always does, so this is a defensive diagnostic
/// path, not a normal exit).
pub fn decode_brk(esr: u64) -> Option<u16> {
    if exception_class(esr) != EC_BRK {
        return None;
    }
    Some((esr & 0xFFFF) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-built ESR word for a 64-bit `STR` (write) with `SRT = 5`,
    /// `SAS = 0b11` (8 bytes), `EC = 0x24` — every field placed by its own
    /// documented bit position, mirroring `encode.rs`'s own "hand-verified
    /// bit-by-bit" methodology (no external tool, no copied hex).
    fn build_esr_data_abort(ec: u32, write: bool, srt: u32, sas: u32) -> u64 {
        let mut iss: u64 = 0;
        iss |= 1 << 24; // ISV
        iss |= (sas as u64 & 0b11) << 22; // SAS
        iss |= (srt as u64 & 0b1_1111) << 16; // SRT
        iss |= 1 << 15; // SF (64-bit register)
        if write {
            iss |= 1 << 6; // WnR
        }
        iss |= 0b000100; // DFSC (an arbitrary translation-fault code)
        ((ec as u64) << 26) | (1 << 25) | iss
    }

    #[test]
    fn decodes_a_64_bit_store() {
        let esr = build_esr_data_abort(EC_DATA_ABORT_LOWER_EL, true, 5, 0b11);
        let da = decode_data_abort(esr).expect("data abort");
        assert!(da.write);
        assert_eq!(da.reg, Some(5));
        assert_eq!(da.size_bytes, 8);
    }

    #[test]
    fn decodes_a_64_bit_load() {
        let esr = build_esr_data_abort(EC_DATA_ABORT_SAME_EL, false, 12, 0b11);
        let da = decode_data_abort(esr).expect("data abort");
        assert!(!da.write);
        assert_eq!(da.reg, Some(12));
        assert_eq!(da.size_bytes, 8);
    }

    #[test]
    fn srt_31_is_xzr_not_a_real_register() {
        let esr = build_esr_data_abort(EC_DATA_ABORT_LOWER_EL, true, 31, 0b11);
        let da = decode_data_abort(esr).expect("data abort");
        assert_eq!(da.reg, None);
    }

    #[test]
    fn every_access_size_decodes() {
        for (sas, expect) in [(0b00u32, 1u32), (0b01, 2), (0b10, 4), (0b11, 8)] {
            let esr = build_esr_data_abort(EC_DATA_ABORT_LOWER_EL, true, 0, sas);
            assert_eq!(decode_data_abort(esr).unwrap().size_bytes, expect);
        }
    }

    #[test]
    fn mmio_protocol_words_are_eight_bytes_only() {
        // boot.rs rejects anything but size_bytes == 8 at every MMIO
        // address; pin the decode side so a 1/2/4-byte LDRB/LDRH/LDR
        // is distinguishable from an 8-byte LDR (adversarial audit).
        for (sas, bytes) in [(0b00u32, 1u32), (0b01, 2), (0b10, 4), (0b11, 8)] {
            let esr = build_esr_data_abort(EC_DATA_ABORT_LOWER_EL, false, 1, sas);
            let da = decode_data_abort(esr).unwrap();
            assert_eq!(da.size_bytes == 8, bytes == 8, "sas={sas}");
        }
    }

    #[test]
    fn isv_zero_is_not_decodable() {
        // ISV bit cleared: the same word as a valid store, but with bit 24
        // masked off.
        let esr = build_esr_data_abort(EC_DATA_ABORT_LOWER_EL, true, 5, 0b11) & !(1 << 24);
        assert_eq!(decode_data_abort(esr), None);
    }

    #[test]
    fn non_data_abort_ec_is_not_decoded_as_one() {
        let esr = (0x3Cu64) << 26; // BRK's own EC, not a data abort's.
        assert_eq!(decode_data_abort(esr), None);
    }

    #[test]
    fn decodes_a_brk_immediate() {
        let esr = (EC_BRK as u64) << 26 | (1 << 25) | 0x0042;
        assert_eq!(decode_brk(esr), Some(0x0042));
    }

    #[test]
    fn non_brk_ec_is_not_decoded_as_one() {
        let esr = (EC_DATA_ABORT_LOWER_EL as u64) << 26;
        assert_eq!(decode_brk(esr), None);
    }

    #[test]
    fn overlap_run_depth_is_quiescent_outside_run() {
        // plans/M15.md item I: depth is a test metric only. Outside any
        // `hv_vcpu_run` it must read zero; overlapping boots separately
        // prove the peak rises above 1 (`lib` concurrent boot test).
        assert_eq!(hv_vcpu_run_depth(), 0);
    }
}
