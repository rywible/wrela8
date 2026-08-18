use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use super::{
    BackendKind, DiagnosticRegisters, HostBackend, HostCapabilities, HostExit, HostExitDiagnostic,
    HostMemoryRegion, HostVcpu, HostVm, IsaCapabilities, MemoryProtection, MmioAccess,
    MmioCompletion, MmioDirection, MmioToken, TranslationBootState, VcpuBootState, VcpuHandle,
    host_error,
};
use crate::VmmError;
use crate::guest_memory::GuestMemory;

pub const HV_SUCCESS: i32 = 0;

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

pub const HV_MEMORY_READ: u64 = 1 << 0;
pub const HV_MEMORY_WRITE: u64 = 1 << 1;
pub const HV_MEMORY_EXEC: u64 = 1 << 2;

pub const HV_REG_PC: u32 = 31;
pub const HV_REG_FPCR: u32 = 32;
pub const HV_REG_FPSR: u32 = 33;
pub const HV_REG_CPSR: u32 = 34;

pub fn hv_reg_xn(n: u32) -> u32 {
    debug_assert!(n <= 30, "x{n} is out of range for a plain GPR");
    n
}

pub const HV_SYS_REG_CPACR_EL1: u16 = 0xc082;
pub const HV_SYS_REG_SP_EL0: u16 = 0xc208;
pub const HV_SYS_REG_SP_EL1: u16 = 0xe208;
pub const HV_SYS_REG_SCTLR_EL1: u16 = 0xc080;
pub const HV_SYS_REG_TTBR0_EL1: u16 = 0xc100;
pub const HV_SYS_REG_TCR_EL1: u16 = 0xc102;
pub const HV_SYS_REG_MAIR_EL1: u16 = 0xc510;
pub const HV_NO_RESOURCES: i32 = 0xfae9_4005u32 as i32;
pub const HV_SYS_REG_ESR_EL1: u16 = 0xc290;
pub const HV_SYS_REG_ELR_EL1: u16 = 0xc201;
pub const HV_SYS_REG_FAR_EL1: u16 = 0xc300;
pub const HV_SYS_REG_VBAR_EL1: u16 = 0xc600;
pub const HV_SYS_REG_SPSR_EL1: u16 = 0xc200;

pub const HV_EXIT_REASON_CANCELED: u32 = 0;
pub const HV_EXIT_REASON_EXCEPTION: u32 = 1;
pub const HV_EXIT_REASON_VTIMER_ACTIVATED: u32 = 2;
pub const HV_EXIT_REASON_UNKNOWN: u32 = 3;

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

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HvSimdFpUchar16(pub [u8; 16]);

#[link(name = "Hypervisor", kind = "framework")]
unsafe extern "C" {
    pub fn hv_vm_create(config: *mut c_void) -> i32;
    pub fn hv_vm_destroy() -> i32;
    pub fn hv_vm_map(addr: *mut c_void, ipa: u64, size: usize, flags: u64) -> i32;
    pub fn hv_vm_protect(ipa: u64, size: usize, flags: u64) -> i32;

    pub fn hv_vcpu_create(vcpu: *mut u64, exit: *mut *mut HvVcpuExit, config: *mut c_void) -> i32;
    pub fn hv_vcpu_destroy(vcpu: u64) -> i32;
    pub fn hv_vcpu_get_reg(vcpu: u64, reg: u32, value: *mut u64) -> i32;
    pub fn hv_vcpu_set_reg(vcpu: u64, reg: u32, value: u64) -> i32;
    pub fn hv_vcpu_set_sys_reg(vcpu: u64, reg: u16, value: u64) -> i32;
    pub fn hv_vcpu_get_sys_reg(vcpu: u64, reg: u16, value: *mut u64) -> i32;
    pub fn hv_vcpu_set_simd_fp_reg(vcpu: u64, reg: u32, value: HvSimdFpUchar16) -> i32;
    #[link_name = "hv_vcpu_run"]
    fn hv_vcpu_run_raw(vcpu: u64) -> i32;
    pub fn hv_vcpus_exit(vcpus: *mut u64, vcpu_count: u32) -> i32;
}

static HV_VCPU_RUN_DEPTH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

static HV_VCPU_RUN_DEPTH_MAX: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub fn hv_vcpu_run_depth() -> usize {
    HV_VCPU_RUN_DEPTH.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn hv_vcpu_run_depth_max() -> usize {
    HV_VCPU_RUN_DEPTH_MAX.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn hv_vcpu_run_depth_max_reset() {
    HV_VCPU_RUN_DEPTH_MAX.store(0, std::sync::atomic::Ordering::SeqCst);
}

pub unsafe fn hv_vcpu_run(vcpu: u64) -> i32 {
    use std::sync::atomic::Ordering;
    let prev = HV_VCPU_RUN_DEPTH.fetch_add(1, Ordering::SeqCst);
    let depth = prev + 1;
    HV_VCPU_RUN_DEPTH_MAX.fetch_max(depth, Ordering::SeqCst);
    let r = unsafe { hv_vcpu_run_raw(vcpu) };
    let left = HV_VCPU_RUN_DEPTH.fetch_sub(1, Ordering::SeqCst);
    debug_assert!(left >= 1, "hv_vcpu_run depth counter underflowed");
    r
}

fn exception_class(esr: u64) -> u32 {
    ((esr >> 26) & 0x3F) as u32
}

const EC_DATA_ABORT_LOWER_EL: u32 = 0x24;
const EC_DATA_ABORT_SAME_EL: u32 = 0x25;
const EC_BRK: u32 = 0x3C;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataAbort {
    pub write: bool,
    pub reg: Option<u32>,
    pub size_bytes: u32,
}

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

pub fn decode_brk(esr: u64) -> Option<u16> {
    if exception_class(esr) != EC_BRK {
        return None;
    }
    Some((esr & 0xFFFF) as u16)
}

pub(crate) struct HvfBackend;

pub(crate) struct HvfVm;

struct LiveVcpu {
    id: Mutex<Option<u64>>,
}

#[derive(Clone)]
pub(crate) struct HvfVcpuHandle(Arc<LiveVcpu>);

pub(crate) struct HvfVcpu {
    id: u64,
    exit: *mut HvVcpuExit,
    live: Arc<LiveVcpu>,
    sequence: u64,
    pending: Option<HvfPendingMmio>,
}

unsafe impl Send for HvfVcpu {}

#[derive(Debug, Clone, Copy)]
struct HvfPendingMmio {
    sequence: u64,
    direction: MmioDirection,
    register: Option<u32>,
}

impl HostBackend for HvfBackend {
    type Vm = HvfVm;
    #[cfg(test)]
    const KIND: BackendKind = BackendKind::Hvf;

    fn capabilities() -> Result<HostCapabilities, VmmError> {
        Ok(HostCapabilities {
            backend: BackendKind::Hvf,
            page_size: 16 * 1024,
            max_vcpus: wrela_machine::CORE_SLOTS,
            ipa_bits: 40,
            readonly_memory: true,
            immediate_exit: true,
            isa: IsaCapabilities {
                armv8_2: true,
                fp: true,
                asimd: true,
                fp16: true,
                sve: false,
                pointer_auth: false,
                mte: false,
            },
        })
    }

    fn create_vm(
        memory: &GuestMemory,
        _ipa_bits: u8,
        regions: &[HostMemoryRegion],
    ) -> Result<Self::Vm, VmmError> {
        let result = unsafe { hv_vm_create(std::ptr::null_mut()) };
        if result != HV_SUCCESS {
            return Err(hvf_error("hv_vm_create", result));
        }
        let result = unsafe {
            hv_vm_map(
                memory.as_ptr().cast::<c_void>(),
                wrela_machine::layout::DRAM_BASE,
                memory.len(),
                HV_MEMORY_READ | HV_MEMORY_WRITE,
            )
        };
        if result != HV_SUCCESS {
            unsafe {
                let _ = hv_vm_destroy();
            }
            return Err(hvf_error("hv_vm_map", result));
        }
        let vm = HvfVm;
        for region in regions {
            if let Err(error) = vm.protect(region.gpa, region.len, region.protection) {
                drop(vm);
                return Err(error);
            }
        }
        Ok(vm)
    }
}

impl HostVm for HvfVm {
    type Backend = HvfBackend;
    type Vcpu = HvfVcpu;
    type Handle = HvfVcpuHandle;

    fn protect(&self, gpa: u64, len: usize, protection: MemoryProtection) -> Result<(), VmmError> {
        let flags = match protection {
            MemoryProtection::ReadWrite => HV_MEMORY_READ | HV_MEMORY_WRITE,
            MemoryProtection::ReadExecute => HV_MEMORY_READ | HV_MEMORY_EXEC,
            MemoryProtection::ReadOnly => HV_MEMORY_READ,
        };
        let result = unsafe { hv_vm_protect(gpa, len, flags) };
        if result == HV_SUCCESS {
            Ok(())
        } else {
            Err(hvf_error("hv_vm_protect", result))
        }
    }

    fn create_vcpu(
        &self,
        index: usize,
        state: &VcpuBootState,
    ) -> Result<(Self::Vcpu, Self::Handle), VmmError> {
        let mut id = 0;
        let mut exit = std::ptr::null_mut();
        let result = unsafe { hv_vcpu_create(&mut id, &mut exit, std::ptr::null_mut()) };
        if result != HV_SUCCESS {
            return Err(host_error(
                BackendKind::Hvf,
                "create vCPU",
                result as i64,
                format!("core {index}: {}", describe_hv_return(result)),
            ));
        }
        let init = initialize_vcpu(id, state);
        if let Err(error) = init {
            unsafe {
                let _ = hv_vcpu_destroy(id);
            }
            return Err(error);
        }
        let live = Arc::new(LiveVcpu {
            id: Mutex::new(Some(id)),
        });
        Ok((
            HvfVcpu {
                id,
                exit,
                live: Arc::clone(&live),
                sequence: 0,
                pending: None,
            },
            HvfVcpuHandle(live),
        ))
    }
}

impl Drop for HvfVm {
    fn drop(&mut self) {
        unsafe {
            let _ = hv_vm_destroy();
        }
    }
}

fn initialize_vcpu(id: u64, state: &VcpuBootState) -> Result<(), VmmError> {
    for (register, value) in state.x.iter().copied().enumerate() {
        set_reg(
            id,
            hv_reg_xn(register as u32),
            value,
            "initialize x register",
        )?;
    }
    set_reg(id, HV_REG_PC, state.pc, "initialize PC")?;
    set_reg(id, HV_REG_CPSR, state.pstate, "initialize PSTATE")?;
    set_reg(id, HV_REG_FPCR, state.fpcr, "initialize FPCR")?;
    set_reg(id, HV_REG_FPSR, state.fpsr, "initialize FPSR")?;
    set_sys_reg(id, HV_SYS_REG_SP_EL0, state.sp_el0, "initialize SP_EL0")?;
    set_sys_reg(id, HV_SYS_REG_SP_EL1, state.sp_el1, "initialize SP_EL1")?;
    set_sys_reg(
        id,
        HV_SYS_REG_CPACR_EL1,
        state.cpacr_el1,
        "initialize CPACR_EL1",
    )?;
    for (register, value) in state.simd.iter().copied().enumerate() {
        let result =
            unsafe { hv_vcpu_set_simd_fp_reg(id, register as u32, HvSimdFpUchar16(value)) };
        if result != HV_SUCCESS {
            return Err(hvf_error("initialize SIMD register", result));
        }
    }
    match state.stage1 {
        TranslationBootState::DisabledDiagnostic => {
            let reset = get_sys_reg(id, HV_SYS_REG_SCTLR_EL1)?;
            set_sys_reg(
                id,
                HV_SYS_REG_SCTLR_EL1,
                reset & !1,
                "initialize diagnostic MMU-off SCTLR_EL1",
            )?;
        }
        TranslationBootState::Enabled(stage1) => {
            set_sys_reg(
                id,
                HV_SYS_REG_TTBR0_EL1,
                stage1.ttbr0_el1,
                "initialize TTBR0_EL1",
            )?;
            set_sys_reg(id, HV_SYS_REG_TCR_EL1, stage1.tcr_el1, "initialize TCR_EL1")?;
            set_sys_reg(
                id,
                HV_SYS_REG_MAIR_EL1,
                stage1.mair_el1,
                "initialize MAIR_EL1",
            )?;
            set_sys_reg(
                id,
                HV_SYS_REG_VBAR_EL1,
                stage1.vbar_el1,
                "initialize VBAR_EL1",
            )?;
            // Enabling translation is deliberately the final register write.
            set_sys_reg(
                id,
                HV_SYS_REG_SCTLR_EL1,
                stage1.sctlr_el1,
                "initialize SCTLR_EL1",
            )?;
        }
    }
    Ok(())
}

impl HostVcpu for HvfVcpu {
    type Handle = HvfVcpuHandle;

    fn run(&mut self) -> Result<HostExit, VmmError> {
        if self.pending.is_some() {
            return Err(host_error(
                BackendKind::Hvf,
                "run vCPU",
                0,
                "the preceding MMIO exit was not completed exactly once",
            ));
        }
        let result = unsafe { hv_vcpu_run(self.id) };
        if result != HV_SUCCESS {
            return Err(hvf_error("hv_vcpu_run", result));
        }
        let exit = unsafe { *self.exit };
        match exit.reason {
            HV_EXIT_REASON_EXCEPTION => {
                if let Some(abort) = decode_data_abort(exit.exception.syndrome) {
                    self.sequence = self.sequence.checked_add(1).ok_or_else(|| {
                        host_error(BackendKind::Hvf, "normalize MMIO", 0, "MMIO token overflow")
                    })?;
                    let write_value = if abort.write {
                        Some(match abort.reg {
                            Some(register) => get_reg(self.id, hv_reg_xn(register))?,
                            None => 0,
                        })
                    } else {
                        None
                    };
                    let direction = if abort.write {
                        MmioDirection::Write
                    } else {
                        MmioDirection::Read
                    };
                    self.pending = Some(HvfPendingMmio {
                        sequence: self.sequence,
                        direction,
                        register: abort.reg,
                    });
                    Ok(HostExit::Mmio(MmioAccess {
                        address: exit.exception.physical_address,
                        width: abort.size_bytes as u8,
                        direction,
                        write_value,
                        token: MmioToken::new(self.sequence),
                    }))
                } else if let Some(immediate) = decode_brk(exit.exception.syndrome) {
                    Ok(HostExit::Breakpoint {
                        immediate: Some(immediate),
                    })
                } else {
                    Ok(HostExit::Unexpected(HostExitDiagnostic {
                        class: "exception",
                        raw_reason: exit.reason as u64,
                        syndrome: Some(exit.exception.syndrome),
                        physical_address: Some(exit.exception.physical_address),
                        detail: "exception was neither an ISV-valid data abort nor BRK".into(),
                    }))
                }
            }
            HV_EXIT_REASON_CANCELED => Ok(HostExit::Canceled),
            HV_EXIT_REASON_VTIMER_ACTIVATED => Ok(HostExit::Unexpected(HostExitDiagnostic {
                class: "virtual-timer",
                raw_reason: exit.reason as u64,
                syndrome: None,
                physical_address: None,
                detail: "the Wrela machine has no raw host virtual-timer exit".into(),
            })),
            other => Ok(HostExit::Unexpected(HostExitDiagnostic {
                class: "unknown",
                raw_reason: other as u64,
                syndrome: None,
                physical_address: None,
                detail: "unrecognized Hypervisor.framework exit".into(),
            })),
        }
    }

    fn complete_mmio(
        &mut self,
        token: MmioToken,
        completion: MmioCompletion,
    ) -> Result<(), VmmError> {
        let pending = self.pending.take().ok_or_else(|| {
            host_error(
                BackendKind::Hvf,
                "complete MMIO",
                0,
                "missing or duplicate completion",
            )
        })?;
        if token.0 != pending.sequence {
            self.pending = Some(pending);
            return Err(host_error(
                BackendKind::Hvf,
                "complete MMIO",
                0,
                "stale completion token",
            ));
        }
        match (pending.direction, completion) {
            (MmioDirection::Write, MmioCompletion::WriteAccepted) => {}
            (MmioDirection::Read, MmioCompletion::ReadValue(value)) => {
                if let Some(register) = pending.register {
                    set_reg(self.id, hv_reg_xn(register), value, "complete MMIO read")?;
                }
            }
            _ => {
                return Err(host_error(
                    BackendKind::Hvf,
                    "complete MMIO",
                    0,
                    "completion direction does not match the pending access",
                ));
            }
        }
        let pc = get_reg(self.id, HV_REG_PC)?;
        set_reg(
            self.id,
            HV_REG_PC,
            pc.wrapping_add(4),
            "advance PC after MMIO",
        )
    }

    fn diagnostics(&self) -> Result<DiagnosticRegisters, VmmError> {
        Ok(DiagnosticRegisters {
            pc: get_reg(self.id, HV_REG_PC)?,
            pstate: get_reg(self.id, HV_REG_CPSR)?,
            esr_el1: get_sys_reg(self.id, HV_SYS_REG_ESR_EL1)?,
            far_el1: get_sys_reg(self.id, HV_SYS_REG_FAR_EL1)?,
            elr_el1: get_sys_reg(self.id, HV_SYS_REG_ELR_EL1)?,
            spsr_el1: get_sys_reg(self.id, HV_SYS_REG_SPSR_EL1)?,
        })
    }
}

impl Drop for HvfVcpu {
    fn drop(&mut self) {
        if let Ok(mut live) = self.live.id.lock() {
            *live = None;
        }
        unsafe {
            let _ = hv_vcpu_destroy(self.id);
        }
    }
}

impl VcpuHandle for HvfVcpuHandle {
    fn cancel(&self) -> Result<(), VmmError> {
        let live = self.0.id.lock().unwrap_or_else(|error| error.into_inner());
        let Some(id) = *live else {
            return Ok(());
        };
        let mut ids = [id];
        let result = unsafe { hv_vcpus_exit(ids.as_mut_ptr(), 1) };
        if result == HV_SUCCESS {
            Ok(())
        } else {
            Err(hvf_error("hv_vcpus_exit", result))
        }
    }
}

fn set_reg(id: u64, register: u32, value: u64, operation: &'static str) -> Result<(), VmmError> {
    let result = unsafe { hv_vcpu_set_reg(id, register, value) };
    if result == HV_SUCCESS {
        Ok(())
    } else {
        Err(hvf_error(operation, result))
    }
}

fn get_reg(id: u64, register: u32) -> Result<u64, VmmError> {
    let mut value = 0;
    let result = unsafe { hv_vcpu_get_reg(id, register, &mut value) };
    if result == HV_SUCCESS {
        Ok(value)
    } else {
        Err(hvf_error("read diagnostic register", result))
    }
}

fn set_sys_reg(
    id: u64,
    register: u16,
    value: u64,
    operation: &'static str,
) -> Result<(), VmmError> {
    let result = unsafe { hv_vcpu_set_sys_reg(id, register, value) };
    if result == HV_SUCCESS {
        Ok(())
    } else {
        Err(hvf_error(operation, result))
    }
}

fn get_sys_reg(id: u64, register: u16) -> Result<u64, VmmError> {
    let mut value = 0;
    let result = unsafe { hv_vcpu_get_sys_reg(id, register, &mut value) };
    if result == HV_SUCCESS {
        Ok(value)
    } else {
        Err(hvf_error("read diagnostic system register", result))
    }
}

fn hvf_error(operation: &'static str, code: i32) -> VmmError {
    host_error(
        BackendKind::Hvf,
        operation,
        code as i64,
        describe_hv_return(code),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn architectural_system_register_encodings_are_pinned() {
        assert_eq!(HV_SYS_REG_CPACR_EL1, 0xc082);
        assert_eq!(HV_SYS_REG_SCTLR_EL1, 0xc080);
        assert_eq!(HV_SYS_REG_TTBR0_EL1, 0xc100);
        assert_eq!(HV_SYS_REG_TCR_EL1, 0xc102);
        assert_eq!(HV_SYS_REG_MAIR_EL1, 0xc510);
        assert_eq!(HV_REG_FPCR, 32);
    }

    fn build_esr_data_abort(ec: u32, write: bool, srt: u32, sas: u32) -> u64 {
        let mut iss: u64 = 0;
        iss |= 1 << 24;
        iss |= (sas as u64 & 0b11) << 22;
        iss |= (srt as u64 & 0b1_1111) << 16;
        iss |= 1 << 15;
        if write {
            iss |= 1 << 6;
        }
        iss |= 0b000100;
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
        for (sas, bytes) in [(0b00u32, 1u32), (0b01, 2), (0b10, 4), (0b11, 8)] {
            let esr = build_esr_data_abort(EC_DATA_ABORT_LOWER_EL, false, 1, sas);
            let da = decode_data_abort(esr).unwrap();
            assert_eq!(da.size_bytes == 8, bytes == 8, "sas={sas}");
        }
    }

    #[test]
    fn isv_zero_is_not_decodable() {
        let esr = build_esr_data_abort(EC_DATA_ABORT_LOWER_EL, true, 5, 0b11) & !(1 << 24);
        assert_eq!(decode_data_abort(esr), None);
    }

    #[test]
    fn non_data_abort_ec_is_not_decoded_as_one() {
        let esr = (0x3Cu64) << 26;
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
        assert_eq!(hv_vcpu_run_depth(), 0);
    }
}
