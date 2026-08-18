//! Narrow, backend-neutral host hypervisor boundary.

use crate::VmmError;
use crate::guest_memory::GuestMemory;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) mod hvf;
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub(crate) mod kvm;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) type SelectedBackend = hvf::HvfBackend;
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub(crate) type SelectedBackend = kvm::KvmBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Hvf,
    Kvm,
    Synthetic,
}

impl BackendKind {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Hvf => "hvf",
            Self::Kvm => "kvm",
            Self::Synthetic => "synthetic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IsaCapabilities {
    pub armv8_2: bool,
    pub fp: bool,
    pub asimd: bool,
    pub fp16: bool,
    pub sve: bool,
    pub pointer_auth: bool,
    pub mte: bool,
}

impl IsaCapabilities {
    pub(crate) fn validate_wrela_baseline(self) -> Result<(), VmmError> {
        if !(self.armv8_2 && self.fp && self.asimd) {
            return Err(VmmError::Host {
                backend: BackendKind::Synthetic,
                operation: "validate Wrela ISA baseline",
                code: 0,
                detail: format!(
                    "host is missing baseline feature(s): armv8.2={} fp={} asimd={}",
                    self.armv8_2, self.fp, self.asimd
                ),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostCapabilities {
    pub backend: BackendKind,
    pub page_size: usize,
    pub max_vcpus: usize,
    pub ipa_bits: u8,
    pub readonly_memory: bool,
    pub immediate_exit: bool,
    pub isa: IsaCapabilities,
}

impl HostCapabilities {
    pub(crate) fn validate(&self, cores: usize) -> Result<(), VmmError> {
        if self.page_size == 0 || !self.page_size.is_power_of_two() {
            return Err(host_error(
                self.backend,
                "validate mapping granule",
                0,
                format!("host reported invalid page size {}", self.page_size),
            ));
        }
        if self.page_size > crate::guest_memory::COMMON_PROTECTION_GRANULE {
            return Err(host_error(
                self.backend,
                "validate mapping granule",
                0,
                format!(
                    "host page size {} exceeds the sealed {} byte common protection granule",
                    self.page_size,
                    crate::guest_memory::COMMON_PROTECTION_GRANULE
                ),
            ));
        }
        if cores == 0 || cores > self.max_vcpus {
            return Err(host_error(
                self.backend,
                "validate vCPU count",
                0,
                format!("requested {cores} vCPU(s), host permits {}", self.max_vcpus),
            ));
        }
        if self.ipa_bits < 32 {
            return Err(host_error(
                self.backend,
                "validate IPA width",
                0,
                format!(
                    "{} IPA bits cannot address the Wrela machine",
                    self.ipa_bits
                ),
            ));
        }
        self.isa.validate_wrela_baseline().map_err(|_| {
            host_error(
                self.backend,
                "validate Wrela ISA baseline",
                0,
                format!(
                    "host is missing baseline feature(s): armv8.2={} fp={} asimd={}",
                    self.isa.armv8_2, self.isa.fp, self.isa.asimd
                ),
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stage1BootState {
    pub ttbr0_el1: u64,
    pub tcr_el1: u64,
    pub mair_el1: u64,
    pub sctlr_el1: u64,
    pub vbar_el1: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranslationBootState {
    DisabledDiagnostic,
    Enabled(Stage1BootState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VcpuBootState {
    pub x: [u64; 31],
    pub pc: u64,
    pub pstate: u64,
    pub sp_el0: u64,
    pub sp_el1: u64,
    pub cpacr_el1: u64,
    pub fpcr: u64,
    pub fpsr: u64,
    pub simd: [[u8; 16]; 32],
    pub stage1: TranslationBootState,
}

impl VcpuBootState {
    pub(crate) fn wrela(entry: u64, stack: u64, stage1: TranslationBootState) -> Self {
        let mut x = [0; 31];
        x[0] = wrela_machine::layout::MACHINE_INFO_BASE;
        Self {
            x,
            pc: entry,
            pstate: 0x3c5,
            sp_el0: 0,
            sp_el1: stack,
            cpacr_el1: 0x0030_0000,
            fpcr: crate::GUEST_FPCR,
            fpsr: 0,
            simd: [[0; 16]; 32],
            stage1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MmioDirection {
    Read,
    Write,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MmioToken(u64);

impl MmioToken {
    pub(crate) fn new(sequence: u64) -> Self {
        Self(sequence)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MmioAccess {
    pub address: u64,
    pub width: u8,
    pub direction: MmioDirection,
    pub write_value: Option<u64>,
    pub token: MmioToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MmioCompletion {
    WriteAccepted,
    ReadValue(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostExitDiagnostic {
    pub class: &'static str,
    pub raw_reason: u64,
    pub syndrome: Option<u64>,
    pub physical_address: Option<u64>,
    pub detail: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HostExit {
    Mmio(MmioAccess),
    Breakpoint { immediate: Option<u16> },
    Canceled,
    Unexpected(HostExitDiagnostic),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryProtection {
    ReadWrite,
    ReadExecute,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HostMemoryRegion {
    pub gpa: u64,
    pub len: usize,
    pub protection: MemoryProtection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiagnosticRegisters {
    pub pc: u64,
    pub pstate: u64,
    pub esr_el1: u64,
    pub far_el1: u64,
    pub elr_el1: u64,
    pub spsr_el1: u64,
}

pub(crate) trait HostBackend: Sized + Send + Sync + 'static {
    type Vm: HostVm<Backend = Self>;
    #[cfg(test)]
    const KIND: BackendKind;

    fn capabilities() -> Result<HostCapabilities, VmmError>;
    fn create_vm(
        memory: &GuestMemory,
        ipa_bits: u8,
        regions: &[HostMemoryRegion],
    ) -> Result<Self::Vm, VmmError>;
}

pub(crate) trait HostVm: Send + Sync + 'static {
    type Backend: HostBackend<Vm = Self>;
    type Vcpu: HostVcpu<Handle = Self::Handle>;
    type Handle: VcpuHandle;

    fn protect(&self, gpa: u64, len: usize, protection: MemoryProtection) -> Result<(), VmmError>;
    fn create_vcpu(
        &self,
        index: usize,
        state: &VcpuBootState,
    ) -> Result<(Self::Vcpu, Self::Handle), VmmError>;
}

pub(crate) trait HostVcpu: Send + 'static {
    type Handle: VcpuHandle;

    fn run(&mut self) -> Result<HostExit, VmmError>;
    fn complete_mmio(
        &mut self,
        token: MmioToken,
        completion: MmioCompletion,
    ) -> Result<(), VmmError>;
    fn diagnostics(&self) -> Result<DiagnosticRegisters, VmmError>;
}

pub(crate) trait VcpuHandle: Clone + Send + Sync + 'static {
    fn cancel(&self) -> Result<(), VmmError>;
}

pub(crate) fn host_error(
    backend: BackendKind,
    operation: &'static str,
    code: i64,
    detail: impl Into<String>,
) -> VmmError {
    VmmError::Host {
        backend,
        operation,
        code,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct FakeHandle;
    impl VcpuHandle for FakeHandle {
        fn cancel(&self) -> Result<(), VmmError> {
            Ok(())
        }
    }

    struct FakeVcpu {
        pending: Option<u64>,
        completed: bool,
    }

    impl HostVcpu for FakeVcpu {
        type Handle = FakeHandle;

        fn run(&mut self) -> Result<HostExit, VmmError> {
            if self.pending.is_some() && !self.completed {
                return Err(host_error(
                    BackendKind::Synthetic,
                    "run vCPU",
                    0,
                    "the preceding MMIO exit was not completed exactly once",
                ));
            }
            self.completed = false;
            self.pending = Some(1);
            Ok(HostExit::Mmio(MmioAccess {
                address: 0x800_0000,
                width: 8,
                direction: MmioDirection::Read,
                write_value: None,
                token: MmioToken::new(1),
            }))
        }

        fn complete_mmio(
            &mut self,
            token: MmioToken,
            completion: MmioCompletion,
        ) -> Result<(), VmmError> {
            if self.pending.take() != Some(token.0) || self.completed {
                return Err(host_error(
                    BackendKind::Synthetic,
                    "complete MMIO",
                    0,
                    "missing or duplicate MMIO completion",
                ));
            }
            if !matches!(completion, MmioCompletion::ReadValue(_)) {
                return Err(host_error(
                    BackendKind::Synthetic,
                    "complete MMIO",
                    0,
                    "read exit requires ReadValue completion",
                ));
            }
            self.completed = true;
            Ok(())
        }

        fn diagnostics(&self) -> Result<DiagnosticRegisters, VmmError> {
            Ok(DiagnosticRegisters {
                pc: 0,
                pstate: 0,
                esr_el1: 0,
                far_el1: 0,
                elr_el1: 0,
                spsr_el1: 0,
            })
        }
    }

    #[test]
    fn mmio_completion_is_consumed_exactly_once() {
        let mut vcpu = FakeVcpu {
            pending: None,
            completed: false,
        };
        let HostExit::Mmio(access) = vcpu.run().unwrap() else {
            panic!("expected synthetic MMIO")
        };
        vcpu.complete_mmio(access.token, MmioCompletion::ReadValue(7))
            .unwrap();
        assert!(
            vcpu.complete_mmio(MmioToken::new(1), MmioCompletion::ReadValue(8))
                .is_err()
        );
        let _ = vcpu.run().unwrap();
        assert!(
            vcpu.run()
                .unwrap_err()
                .to_string()
                .contains("not completed")
        );
    }

    #[test]
    fn capability_validation_fails_closed() {
        let mut caps = HostCapabilities {
            backend: BackendKind::Synthetic,
            page_size: 4096,
            max_vcpus: 3,
            ipa_bits: 40,
            readonly_memory: true,
            immediate_exit: true,
            isa: IsaCapabilities {
                armv8_2: true,
                fp: true,
                asimd: true,
                fp16: false,
                sve: false,
                pointer_auth: false,
                mte: false,
            },
        };
        caps.validate(3).unwrap();
        caps.max_vcpus = 2;
        assert!(
            caps.validate(3)
                .unwrap_err()
                .to_string()
                .contains("requested 3")
        );
        caps.max_vcpus = 3;
        caps.isa.asimd = false;
        assert!(
            caps.validate(1)
                .unwrap_err()
                .to_string()
                .contains("asimd=false")
        );
    }

    #[test]
    fn timer_and_unknown_reasons_remain_unexpected() {
        let exit = HostExit::Unexpected(HostExitDiagnostic {
            class: "virtual-timer",
            raw_reason: 3,
            syndrome: None,
            physical_address: None,
            detail: "timer exits are outside the Wrela machine".into(),
        });
        assert!(matches!(exit, HostExit::Unexpected(_)));
    }

    #[test]
    fn boot_state_pins_every_writable_visibility_row() {
        let stage1 = TranslationBootState::Enabled(Stage1BootState {
            ttbr0_el1: 1,
            tcr_el1: 2,
            mair_el1: 3,
            sctlr_el1: 4,
            vbar_el1: 5,
        });
        let state = VcpuBootState::wrela(0x4060_0000, 0x4050_0000, stage1);
        assert_eq!(state.x[0], wrela_machine::layout::MACHINE_INFO_BASE);
        assert_eq!(state.x[1..], [0; 30]);
        assert_eq!(state.pc, 0x4060_0000);
        assert_eq!(state.pstate, 0x3c5);
        assert_eq!(state.sp_el0, 0);
        assert_eq!(state.sp_el1, 0x4050_0000);
        assert_eq!(state.cpacr_el1, 0x0030_0000);
        assert_eq!(state.fpcr, crate::GUEST_FPCR);
        assert_eq!(state.fpsr, 0);
        assert_eq!(state.simd, [[0; 16]; 32]);
        assert_eq!(state.stage1, stage1);
    }
}
