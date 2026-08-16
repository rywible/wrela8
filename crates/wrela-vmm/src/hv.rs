use std::ffi::c_void;

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
pub const HV_REG_CPSR: u32 = 34;

pub fn hv_reg_xn(n: u32) -> u32 {
    debug_assert!(n <= 30, "x{n} is out of range for a plain GPR");
    n
}

pub const HV_SYS_REG_CPACR_EL1: u16 = 0xc082;
pub const HV_SYS_REG_SP_EL1: u16 = 0xe208;
pub const HV_NO_RESOURCES: i32 = 0xfae9_4005u32 as i32;
pub const HV_SYS_REG_ESR_EL1: u16 = 0xc290;
pub const HV_SYS_REG_ELR_EL1: u16 = 0xc201;
pub const HV_SYS_REG_FAR_EL1: u16 = 0xc300;
pub const HV_SYS_REG_VBAR_EL1: u16 = 0xc600;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn architectural_system_register_encodings_are_pinned() {
        assert_eq!(HV_SYS_REG_CPACR_EL1, 0xc082);
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
