//! Linux/aarch64 KVM adapter. All rust-vmm and KVM run-state details stop here.

use std::mem::size_of;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, OnceLock};

use kvm_bindings::{
    KVM_API_VERSION, KVM_MEM_READONLY, KVM_REG_ARM_CORE, KVM_REG_ARM64, KVM_REG_ARM64_SYSREG,
    KVM_REG_SIZE_U32, KVM_REG_SIZE_U64, KVM_REG_SIZE_U128, kvm_regs, kvm_userspace_memory_region,
    kvm_vcpu_init, user_fpsimd_state, user_pt_regs,
};
use kvm_ioctls::{Cap, Kvm, VcpuExit, VcpuFd, VmFd};

use super::{
    BackendKind, DiagnosticRegisters, HostBackend, HostCapabilities, HostExit, HostExitDiagnostic,
    HostMemoryRegion, HostVcpu, HostVm, IsaCapabilities, MemoryProtection, MmioAccess,
    MmioCompletion, MmioDirection, MmioToken, TranslationBootState, VcpuBootState, VcpuHandle,
    host_error,
};
use crate::VmmError;
use crate::guest_memory::GuestMemory;

const ID_AA64PFR0_EL1: u16 = 0xc020;
const ID_AA64PFR1_EL1: u16 = 0xc021;
const ID_AA64ISAR0_EL1: u16 = 0xc030;
const ID_AA64ISAR1_EL1: u16 = 0xc031;
const ID_AA64ISAR2_EL1: u16 = 0xc032;
const CPACR_EL1: u16 = 0xc082;
const SCTLR_EL1: u16 = 0xc080;
const TTBR0_EL1: u16 = 0xc100;
const TCR_EL1: u16 = 0xc102;
const MAIR_EL1: u16 = 0xc510;
const VBAR_EL1: u16 = 0xc600;
const ESR_EL1: u16 = 0xc290;
const ELR_EL1: u16 = 0xc201;
const FAR_EL1: u16 = 0xc300;
const SPSR_EL1: u16 = 0xc200;

pub(crate) struct KvmBackend;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct KvmCapabilityProbe {
    api_version: bool,
    immediate_exit: bool,
    ipa_size: bool,
    isa_baseline: bool,
    one_reg: bool,
    readonly_mem: bool,
    target: bool,
    user_memory: bool,
    vcpu_count: bool,
    writable_id_regs: bool,
    ipa_bits: u8,
    max_vcpus: usize,
    isa: Option<IsaCapabilities>,
}

impl KvmCapabilityProbe {
    fn required_ok(self) -> bool {
        self.api_version
            && self.immediate_exit
            && self.ipa_size
            && self.isa_baseline
            && self.one_reg
            && self.target
            && self.user_memory
            && self.vcpu_count
            && self.writable_id_regs
    }

    pub(crate) fn render(self) -> String {
        format!(
            "api_version:{},immediate_exit:{},ipa_size:{},isa_baseline:{},one_reg:{},readonly_mem:{},target:{},user_memory:{},vcpu_count:{},writable_id_regs:{}",
            yes_no(self.api_version),
            yes_no(self.immediate_exit),
            yes_no(self.ipa_size),
            yes_no(self.isa_baseline),
            yes_no(self.one_reg),
            yes_no(self.readonly_mem),
            yes_no(self.target),
            yes_no(self.user_memory),
            yes_no(self.vcpu_count),
            yes_no(self.writable_id_regs),
        )
    }
}

pub(crate) struct KvmVm {
    fd: VmFd,
    preferred: kvm_vcpu_init,
    mapped: Vec<HostMemoryRegion>,
}

pub(crate) struct KvmVcpu {
    fd: VcpuFd,
    immediate_exit: Arc<ImmediateExit>,
    sequence: u64,
    pending: Option<KvmPendingMmio>,
    cancel_generation: u64,
}

struct ImmediateExit(Mutex<ImmediateExitState>);
struct ImmediateExitState {
    ptr: Option<NonNull<u8>>,
    generation: u64,
    /// The `pthread_t` of the vCPU's owning thread, captured while that thread
    /// creates the vCPU. Cleared with `ptr` before the run mapping dies.
    thread: usize,
}
// The pointer is confined to the mutex and is cleared by `KvmVcpu::drop`
// while the KVM run mapping is still live. A cancellation therefore either
// holds the lock across its volatile store or observes `None`.
unsafe impl Send for ImmediateExit {}
unsafe impl Sync for ImmediateExit {}

/// `SIGUSR1`. The kick handler does nothing: its only job is to make the
/// kernel's `signal_pending()` check fire so an in-guest `KVM_RUN` returns
/// `EINTR`. `KVM_RUN` reports `-EINTR` rather than `-ERESTARTSYS`, so the
/// handler's `SA_RESTART` disposition cannot silently resume the guest.
const VCPU_KICK_SIGNAL: i32 = 10;

unsafe extern "C" {
    fn signal(number: i32, handler: usize) -> usize;
    fn pthread_self() -> usize;
    fn pthread_kill(thread: usize, number: i32) -> i32;
}

extern "C" fn vcpu_kick_handler(_number: i32) {}

/// `immediate_exit` alone is only examined on `KVM_RUN` entry, so it cannot
/// eject a vCPU that is already executing guest code. Every cancellation
/// therefore pairs it with this signal.
fn install_vcpu_kick_handler() -> Result<(), VmmError> {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    // SIG_ERR is `(void (*)(int)) -1`.
    let installed = *INSTALLED.get_or_init(
        || unsafe {
            signal(
                VCPU_KICK_SIGNAL,
                vcpu_kick_handler as *const () as usize,
            )
        } != usize::MAX,
    );
    if installed {
        Ok(())
    } else {
        Err(kvm_error(
            "install vCPU kick handler",
            0,
            "the host refused the reserved vCPU cancellation signal",
        ))
    }
}

#[derive(Clone)]
pub(crate) struct KvmVcpuHandle(Arc<ImmediateExit>);

#[derive(Debug, Clone, Copy)]
struct KvmPendingMmio {
    sequence: u64,
    direction: MmioDirection,
    width: u8,
}

impl HostBackend for KvmBackend {
    type Vm = KvmVm;
    #[cfg(test)]
    const KIND: BackendKind = BackendKind::Kvm;

    fn capabilities() -> Result<HostCapabilities, VmmError> {
        let probe = probe_capabilities()?;
        if !probe.required_ok() {
            return Err(kvm_error(
                "validate KVM capability profile",
                0,
                probe.render(),
            ));
        }
        Ok(HostCapabilities {
            backend: BackendKind::Kvm,
            page_size: host_page_size()?,
            max_vcpus: probe.max_vcpus,
            ipa_bits: probe.ipa_bits,
            readonly_memory: probe.readonly_mem,
            immediate_exit: probe.immediate_exit,
            isa: probe.isa.expect("required_ok includes the ISA probe"),
        })
    }

    fn create_vm(
        memory: &GuestMemory,
        ipa_bits: u8,
        regions: &[HostMemoryRegion],
    ) -> Result<Self::Vm, VmmError> {
        let kvm = open_kvm()?;
        if !kvm.check_extension(Cap::UserMemory) {
            return Err(kvm_error(
                "query KVM_CAP_USER_MEMORY",
                0,
                "capability is absent",
            ));
        }
        if !kvm.check_extension(Cap::OneReg) {
            return Err(kvm_error(
                "query KVM_CAP_ONE_REG",
                0,
                "capability is absent",
            ));
        }
        if !kvm.check_extension(Cap::ImmediateExit) {
            return Err(kvm_error(
                "query KVM_CAP_IMMEDIATE_EXIT",
                0,
                "capability is absent",
            ));
        }
        // The Wrela profile masks architectural `ID_AA64*` feature registers,
        // never the implementation-defined MIDR/REVIDR/AIDR set that
        // `KVM_CAP_ARM_WRITABLE_IMP_ID_REGS` advertises. Requiring that
        // capability here would refuse a conforming kernel for an unrelated
        // reason. The enforceable proof is the masking itself, which
        // `probe_capabilities` performs before any VM exists and
        // `create_vcpu` repeats fail-closed on every vCPU.
        let host_ipa = require_positive(
            "query KVM_CAP_ARM_VM_IPA_SIZE",
            kvm.check_extension_int(Cap::ArmVmIPASize),
        )? as u8;
        if ipa_bits > host_ipa {
            return Err(kvm_error(
                "select VM IPA width",
                0,
                format!("requested {ipa_bits} bits, host limit is {host_ipa}"),
            ));
        }
        validate_regions(memory, regions, host_page_size()?)?;
        if regions.len() > kvm.get_nr_memslots() {
            return Err(kvm_error(
                "validate KVM memslot count",
                0,
                format!(
                    "{} regions exceed {} memslots",
                    regions.len(),
                    kvm.get_nr_memslots()
                ),
            ));
        }
        let readonly = kvm.check_extension(Cap::ReadonlyMem);
        let vm = kvm
            .create_vm_with_ipa_size(ipa_bits.into())
            .map_err(|error| errno_error("KVM_CREATE_VM", error))?;
        for (slot, region) in regions.iter().enumerate() {
            let offset = memory.offset(region.gpa, region.len, "KVM memory region")?;
            let flags = if readonly && region.protection != MemoryProtection::ReadWrite {
                KVM_MEM_READONLY
            } else {
                0
            };
            let mapping = kvm_userspace_memory_region {
                slot: slot as u32,
                flags,
                guest_phys_addr: region.gpa,
                memory_size: region.len as u64,
                userspace_addr: unsafe { memory.as_ptr().add(offset) } as u64,
            };
            // SAFETY: the region is page-aligned, checked inside the one live
            // owned GuestMemory reservation, and the VM cannot outlive it in
            // the engine's lexical ownership order.
            unsafe { vm.set_user_memory_region(mapping) }
                .map_err(|error| errno_error("KVM_SET_USER_MEMORY_REGION", error))?;
        }
        let mut preferred = kvm_vcpu_init::default();
        vm.get_preferred_target(&mut preferred)
            .map_err(|error| errno_error("KVM_ARM_PREFERRED_TARGET", error))?;
        Ok(KvmVm {
            fd: vm,
            preferred,
            mapped: regions.to_vec(),
        })
    }
}

impl HostVm for KvmVm {
    type Backend = KvmBackend;
    type Vcpu = KvmVcpu;
    type Handle = KvmVcpuHandle;

    fn protect(&self, gpa: u64, len: usize, protection: MemoryProtection) -> Result<(), VmmError> {
        if self
            .mapped
            .iter()
            .any(|region| region.gpa == gpa && region.len == len && region.protection == protection)
        {
            Ok(())
        } else {
            Err(kvm_error(
                "protect guest memory",
                0,
                "KVM memslot protections are immutable after VM construction",
            ))
        }
    }

    fn create_vcpu(
        &self,
        index: usize,
        state: &VcpuBootState,
    ) -> Result<(Self::Vcpu, Self::Handle), VmmError> {
        let mut fd = self
            .fd
            .create_vcpu(index as u64)
            .map_err(|error| errno_error("KVM_CREATE_VCPU", error))?;
        fd.vcpu_init(&self.preferred)
            .map_err(|error| errno_error("KVM_ARM_VCPU_INIT", error))?;
        mask_and_validate_id_registers(&fd)?;
        initialize_vcpu(&fd, state)?;
        install_vcpu_kick_handler()?;
        let immediate = NonNull::from(&mut fd.get_kvm_run().immediate_exit);
        // `create_vcpu` runs on the thread that will own every `KVM_RUN` for
        // this vCPU, so `pthread_self` here is the thread a kick must target.
        let immediate_exit = Arc::new(ImmediateExit(Mutex::new(ImmediateExitState {
            ptr: Some(immediate),
            generation: 0,
            thread: unsafe { pthread_self() },
        })));
        Ok((
            KvmVcpu {
                fd,
                immediate_exit: Arc::clone(&immediate_exit),
                sequence: 0,
                pending: None,
                cancel_generation: 0,
            },
            KvmVcpuHandle(immediate_exit),
        ))
    }
}

impl HostVcpu for KvmVcpu {
    type Handle = KvmVcpuHandle;

    fn run(&mut self) -> Result<HostExit, VmmError> {
        if self.pending.is_some() {
            return Err(kvm_error(
                "KVM_RUN",
                0,
                "the preceding MMIO exit was not completed exactly once",
            ));
        }
        let guard = self.immediate_exit.0.lock().map_err(|_| {
            kvm_error(
                "reset KVM immediate_exit",
                0,
                "cancellation lock is poisoned",
            )
        })?;
        let immediate = guard.ptr.ok_or_else(|| {
            kvm_error(
                "reset KVM immediate_exit",
                0,
                "the vCPU run mapping is no longer live",
            )
        })?;
        if guard.generation != self.cancel_generation {
            self.cancel_generation = guard.generation;
            return Ok(HostExit::Canceled);
        }
        // SAFETY: the mutex excludes both cancellation and `KvmVcpu::drop`,
        // so the KVM run mapping remains live for this store.
        unsafe { std::ptr::write_volatile(immediate.as_ptr(), 0) };
        drop(guard);
        let exit = loop {
            match self.fd.run() {
                Ok(VcpuExit::Intr) => {
                    if self.observe_cancellation()? {
                        return Ok(HostExit::Canceled);
                    }
                }
                Ok(exit) => break exit,
                Err(error) if error.errno() == 4 => {
                    if self.observe_cancellation()? {
                        return Ok(HostExit::Canceled);
                    }
                }
                Err(error) => return Err(errno_error("KVM_RUN", error)),
            }
        };
        match exit {
            VcpuExit::MmioRead(address, bytes) => {
                let width = checked_mmio_width(address, bytes.len())?;
                self.sequence = next_sequence(self.sequence)?;
                self.pending = Some(KvmPendingMmio {
                    sequence: self.sequence,
                    direction: MmioDirection::Read,
                    width,
                });
                Ok(HostExit::Mmio(MmioAccess {
                    address,
                    width,
                    direction: MmioDirection::Read,
                    write_value: None,
                    token: MmioToken::new(self.sequence),
                }))
            }
            VcpuExit::MmioWrite(address, bytes) => {
                let width = checked_mmio_width(address, bytes.len())?;
                let mut value = [0_u8; 8];
                value[..bytes.len()].copy_from_slice(bytes);
                self.sequence = next_sequence(self.sequence)?;
                self.pending = Some(KvmPendingMmio {
                    sequence: self.sequence,
                    direction: MmioDirection::Write,
                    width,
                });
                Ok(HostExit::Mmio(MmioAccess {
                    address,
                    width,
                    direction: MmioDirection::Write,
                    write_value: Some(u64::from_le_bytes(value)),
                    token: MmioToken::new(self.sequence),
                }))
            }
            VcpuExit::Intr => unreachable!("KVM interrupt exits are classified in the retry loop"),
            VcpuExit::Debug(_) => Ok(HostExit::Breakpoint { immediate: None }),
            other => Ok(HostExit::Unexpected(HostExitDiagnostic {
                class: "kvm-exit",
                raw_reason: 0,
                syndrome: None,
                physical_address: None,
                detail: format!("{other:?}"),
            })),
        }
    }

    fn complete_mmio(
        &mut self,
        token: MmioToken,
        completion: MmioCompletion,
    ) -> Result<(), VmmError> {
        let pending = self
            .pending
            .take()
            .ok_or_else(|| kvm_error("complete KVM MMIO", 0, "missing or duplicate completion"))?;
        if token.0 != pending.sequence {
            self.pending = Some(pending);
            return Err(kvm_error("complete KVM MMIO", 0, "stale completion token"));
        }
        match (pending.direction, completion) {
            (MmioDirection::Write, MmioCompletion::WriteAccepted) => Ok(()),
            (MmioDirection::Read, MmioCompletion::ReadValue(value)) => {
                let run = self.fd.get_kvm_run();
                // SAFETY: run() established KVM_EXIT_MMIO and the pending
                // token prevents any intervening KVM_RUN or union reuse.
                let mmio = unsafe { &mut run.__bindgen_anon_1.mmio };
                if mmio.len != pending.width as u32 || mmio.is_write != 0 {
                    return Err(kvm_error(
                        "complete KVM MMIO",
                        0,
                        "KVM run-state no longer matches the pending read",
                    ));
                }
                mmio.data[..pending.width as usize]
                    .copy_from_slice(&value.to_le_bytes()[..pending.width as usize]);
                Ok(())
            }
            _ => Err(kvm_error(
                "complete KVM MMIO",
                0,
                "completion direction does not match the pending access",
            )),
        }
    }

    fn diagnostics(&self) -> Result<DiagnosticRegisters, VmmError> {
        Ok(DiagnosticRegisters {
            pc: get_u64(&self.fd, core_reg_id(pc_offset(), KVM_REG_SIZE_U64))?,
            pstate: get_u64(&self.fd, core_reg_id(pstate_offset(), KVM_REG_SIZE_U64))?,
            esr_el1: get_u64(&self.fd, sys_reg_id(ESR_EL1))?,
            far_el1: get_u64(&self.fd, sys_reg_id(FAR_EL1))?,
            elr_el1: get_u64(&self.fd, sys_reg_id(ELR_EL1))?,
            spsr_el1: get_u64(&self.fd, sys_reg_id(SPSR_EL1))?,
        })
    }
}

impl KvmVcpu {
    fn observe_cancellation(&mut self) -> Result<bool, VmmError> {
        let generation = self
            .immediate_exit
            .0
            .lock()
            .map_err(|_| kvm_error("classify KVM interrupt", 0, "cancellation lock is poisoned"))?
            .generation;
        let canceled = generation != self.cancel_generation;
        self.cancel_generation = generation;
        Ok(canceled)
    }
}

impl VcpuHandle for KvmVcpuHandle {
    fn cancel(&self) -> Result<(), VmmError> {
        let mut guard = self
            .0
            .0
            .lock()
            .map_err(|_| kvm_error("cancel KVM vCPU", 0, "cancellation lock is poisoned"))?;
        let Some(immediate) = guard.ptr else {
            return Ok(());
        };
        guard.generation = guard
            .generation
            .checked_add(1)
            .ok_or_else(|| kvm_error("cancel KVM vCPU", 0, "cancellation generation overflow"))?;
        // SAFETY: `KvmVcpu::drop` must acquire this mutex before invalidating
        // the KVM run mapping, so the pointer stays valid through the store.
        //
        // The flag is published before the signal so a vCPU that has not yet
        // entered `KVM_RUN` refuses at its next entry, and the signal then
        // ejects a vCPU that is already executing guest code. `run` takes this
        // same mutex at entry, so it cannot miss both.
        unsafe { std::ptr::write_volatile(immediate.as_ptr(), 1) };
        let result = unsafe { pthread_kill(guard.thread, VCPU_KICK_SIGNAL) };
        if result != 0 {
            return Err(kvm_error(
                "cancel KVM vCPU",
                i64::from(result),
                "the vCPU kick signal could not be delivered to its owning thread",
            ));
        }
        Ok(())
    }
}

impl Drop for KvmVcpu {
    fn drop(&mut self) {
        // Clear the borrowed pointer before `fd` drops and unmaps kvm_run, and
        // forget the thread so a late cancellation cannot signal a thread id
        // the host may already have reused. Recovering a poisoned mutex is safe
        // here because invalidating the pointer is the fail-closed operation.
        let mut state = self
            .immediate_exit
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.ptr = None;
        state.thread = 0;
    }
}

pub(crate) fn probe_capabilities() -> Result<KvmCapabilityProbe, VmmError> {
    let kvm = Kvm::new().map_err(|error| errno_error("open /dev/kvm", error))?;
    let mut probe = KvmCapabilityProbe {
        api_version: kvm.get_api_version() == KVM_API_VERSION as i32,
        immediate_exit: kvm.check_extension(Cap::ImmediateExit),
        one_reg: kvm.check_extension(Cap::OneReg),
        readonly_mem: kvm.check_extension(Cap::ReadonlyMem),
        user_memory: kvm.check_extension(Cap::UserMemory),
        writable_id_regs: false,
        ..KvmCapabilityProbe::default()
    };
    let ipa = kvm.check_extension_int(Cap::ArmVmIPASize);
    if ipa > 0 {
        probe.ipa_bits = ipa as u8;
        probe.ipa_size = probe.ipa_bits >= 40;
    }
    let max_vcpus = kvm.check_extension_int(Cap::MaxVcpus);
    if max_vcpus > 0 {
        probe.max_vcpus = max_vcpus as usize;
        probe.vcpu_count = probe.max_vcpus >= 3;
    }
    if !probe.api_version || !probe.ipa_size || !probe.one_reg {
        return Ok(probe);
    }
    let Ok(vm) = kvm.create_vm_with_ipa_size(40) else {
        return Ok(probe);
    };
    let mut preferred = kvm_vcpu_init::default();
    if vm.get_preferred_target(&mut preferred).is_err() {
        return Ok(probe);
    }
    let Ok(vcpu) = vm.create_vcpu(0) else {
        return Ok(probe);
    };
    if vcpu.vcpu_init(&preferred).is_err() {
        return Ok(probe);
    }
    probe.target = true;
    let registers = [
        get_u64(&vcpu, sys_reg_id(ID_AA64PFR0_EL1)),
        get_u64(&vcpu, sys_reg_id(ID_AA64PFR1_EL1)),
        get_u64(&vcpu, sys_reg_id(ID_AA64ISAR0_EL1)),
        get_u64(&vcpu, sys_reg_id(ID_AA64ISAR1_EL1)),
        get_u64(&vcpu, sys_reg_id(ID_AA64ISAR2_EL1)),
    ];
    let Ok(registers) = registers.into_iter().collect::<Result<Vec<_>, _>>() else {
        return Ok(probe);
    };
    let registers: [u64; 5] = registers
        .try_into()
        .expect("the probe reads exactly five ID registers");
    let pfr0 = registers[0];
    let isar0 = registers[2];
    let fp = (pfr0 >> 16) & 0xf;
    let asimd = (pfr0 >> 20) & 0xf;
    let atomics = (isar0 >> 20) & 0xf;
    let isa = IsaCapabilities {
        armv8_2: atomics >= 2,
        fp: fp != 0xf,
        asimd: asimd != 0xf,
        fp16: false,
        sve: false,
        pointer_auth: false,
        mte: false,
    };
    probe.isa_baseline = baseline_id_registers(registers).is_ok();
    probe.isa = probe.isa_baseline.then_some(isa);
    // Performing the reduction is the capability. A kernel that advertises a
    // masking extension but rejects the writes Wrela actually issues is not
    // conforming, and one that accepts them is, whatever it advertises.
    probe.writable_id_regs = mask_and_validate_id_registers(&vcpu).is_ok();
    Ok(probe)
}

fn open_kvm() -> Result<Kvm, VmmError> {
    let kvm = Kvm::new().map_err(|error| errno_error("open /dev/kvm", error))?;
    if kvm.get_api_version() != KVM_API_VERSION as i32 {
        return Err(kvm_error(
            "validate KVM API version",
            kvm.get_api_version() as i64,
            format!("expected {KVM_API_VERSION}"),
        ));
    }
    Ok(kvm)
}

const fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn host_page_size() -> Result<usize, VmmError> {
    // Ask the kernel directly. Parsing `/proc/self/smaps` returned whichever
    // mapping happened to be listed first, which is an incidental choice, and
    // it needs a readable procfs inside the product jail.
    const SC_PAGESIZE: i32 = 30;
    unsafe extern "C" {
        fn sysconf(name: i32) -> std::ffi::c_long;
    }
    let raw = unsafe { sysconf(SC_PAGESIZE) };
    usize::try_from(raw)
        .ok()
        .filter(|value| *value != 0 && value.is_power_of_two())
        .ok_or_else(|| {
            kvm_error(
                "query host page size",
                i64::from(raw as i32),
                "the host reported a page size that is not a nonzero power of two",
            )
        })
}

fn validate_regions(
    memory: &GuestMemory,
    regions: &[HostMemoryRegion],
    page_size: usize,
) -> Result<(), VmmError> {
    if regions.is_empty() {
        return Err(kvm_error(
            "validate memory regions",
            0,
            "region list is empty",
        ));
    }
    let mut cursor = wrela_machine::layout::DRAM_BASE;
    for region in regions {
        if region.gpa != cursor
            || region.len == 0
            || region.gpa % page_size as u64 != 0
            || region.len % page_size != 0
        {
            return Err(kvm_error(
                "validate memory regions",
                0,
                format!(
                    "non-canonical region at {:#x} length {}",
                    region.gpa, region.len
                ),
            ));
        }
        memory.offset(region.gpa, region.len, "host memory protection")?;
        cursor = cursor
            .checked_add(region.len as u64)
            .ok_or_else(|| kvm_error("validate memory regions", 0, "range overflow"))?;
    }
    if cursor != wrela_machine::layout::DRAM_BASE + wrela_machine::layout::DRAM_SIZE {
        return Err(kvm_error(
            "validate memory regions",
            0,
            format!("regions end at {cursor:#x}, not at the end of DRAM"),
        ));
    }
    Ok(())
}

fn mask_and_validate_id_registers(vcpu: &VcpuFd) -> Result<(), VmmError> {
    let original_pfr0 = get_u64(vcpu, sys_reg_id(ID_AA64PFR0_EL1))?;
    let original_pfr1 = get_u64(vcpu, sys_reg_id(ID_AA64PFR1_EL1))?;
    let original_isar0 = get_u64(vcpu, sys_reg_id(ID_AA64ISAR0_EL1))?;
    let original_isar1 = get_u64(vcpu, sys_reg_id(ID_AA64ISAR1_EL1))?;
    let original_isar2 = get_u64(vcpu, sys_reg_id(ID_AA64ISAR2_EL1))?;
    let masked = baseline_id_registers([
        original_pfr0,
        original_pfr1,
        original_isar0,
        original_isar1,
        original_isar2,
    ])?;
    for (name, register, original, value) in [
        ("ID_AA64PFR0_EL1", ID_AA64PFR0_EL1, original_pfr0, masked[0]),
        ("ID_AA64PFR1_EL1", ID_AA64PFR1_EL1, original_pfr1, masked[1]),
        (
            "ID_AA64ISAR0_EL1",
            ID_AA64ISAR0_EL1,
            original_isar0,
            masked[2],
        ),
        (
            "ID_AA64ISAR1_EL1",
            ID_AA64ISAR1_EL1,
            original_isar1,
            masked[3],
        ),
        (
            "ID_AA64ISAR2_EL1",
            ID_AA64ISAR2_EL1,
            original_isar2,
            masked[4],
        ),
    ] {
        // KVM rejects writes to ID registers with no mutable fields, even if
        // the value is unchanged. Avoid such writes while still applying and
        // validating every baseline reduction that is actually required.
        if value != original {
            set_u64(vcpu, sys_reg_id(register), value, "mask KVM ID register").map_err(
                |error| match error {
                    VmmError::Host {
                        backend,
                        operation,
                        code,
                        detail,
                    } => VmmError::Host {
                        backend,
                        operation,
                        code,
                        detail: format!("{name}: {detail}"),
                    },
                    other => other,
                },
            )?;
        }
    }
    Ok(())
}

fn baseline_id_registers(original: [u64; 5]) -> Result<[u64; 5], VmmError> {
    let [mut pfr0, mut pfr1, mut isar0, mut isar1, mut isar2] = original;
    let fp = (pfr0 >> 16) & 0xf;
    let asimd = (pfr0 >> 20) & 0xf;
    let atomics = (isar0 >> 20) & 0xf;
    if fp == 0xf || asimd == 0xf || atomics < 2 {
        return Err(kvm_error(
            "validate Wrela ISA baseline",
            0,
            format!("FP={fp:#x} ASIMD={asimd:#x} Atomics={atomics:#x}"),
        ));
    }
    // Linux exposes FP/ASIMD version fields as immutable on Rasputin. Retain
    // those host values after proving the required minimum; the executable
    // certificate prevents guest discovery. SVE is a mutable opt-in feature
    // and was not requested in KVM_ARM_VCPU_INIT, so reduce its field to zero.
    pfr0 &= !(0xf << 32);
    isar0 = (isar0 & !(0xf << 20)) | (2 << 20);
    pfr1 &= !(0xf << 8);
    isar1 &= !((0xf << 4) | (0xf << 8) | (0xf << 24) | (0xf << 28));
    isar2 &= !((0xf << 12) | (0xf << 16));
    Ok([pfr0, pfr1, isar0, isar1, isar2])
}

fn initialize_vcpu(vcpu: &VcpuFd, state: &VcpuBootState) -> Result<(), VmmError> {
    for (register, value) in state.x.iter().copied().enumerate() {
        set_u64(
            vcpu,
            core_reg_id(x_offset(register), KVM_REG_SIZE_U64),
            value,
            "initialize x register",
        )?;
    }
    set_u64(
        vcpu,
        core_reg_id(sp_el0_offset(), KVM_REG_SIZE_U64),
        state.sp_el0,
        "initialize SP_EL0",
    )?;
    set_u64(
        vcpu,
        core_reg_id(pc_offset(), KVM_REG_SIZE_U64),
        state.pc,
        "initialize PC",
    )?;
    set_u64(
        vcpu,
        core_reg_id(pstate_offset(), KVM_REG_SIZE_U64),
        state.pstate,
        "initialize PSTATE",
    )?;
    set_u64(
        vcpu,
        core_reg_id(sp_el1_offset(), KVM_REG_SIZE_U64),
        state.sp_el1,
        "initialize SP_EL1",
    )?;
    set_u64(
        vcpu,
        sys_reg_id(CPACR_EL1),
        state.cpacr_el1,
        "initialize CPACR_EL1",
    )?;
    for (register, value) in state.simd.iter().enumerate() {
        let offset = simd_offset(register);
        set_bytes(
            vcpu,
            core_reg_id(offset, KVM_REG_SIZE_U128),
            value,
            "initialize SIMD register",
        )?;
    }
    set_u32(
        vcpu,
        core_reg_id(fpsr_offset(), KVM_REG_SIZE_U32 as u64),
        state.fpsr as u32,
        "initialize FPSR",
    )?;
    set_u32(
        vcpu,
        core_reg_id(fpcr_offset(), KVM_REG_SIZE_U32 as u64),
        state.fpcr as u32,
        "initialize FPCR",
    )?;
    match state.stage1 {
        TranslationBootState::DisabledDiagnostic => {
            let reset = get_u64(vcpu, sys_reg_id(SCTLR_EL1))?;
            set_u64(
                vcpu,
                sys_reg_id(SCTLR_EL1),
                reset & !1,
                "initialize diagnostic MMU-off SCTLR_EL1",
            )?;
        }
        TranslationBootState::Enabled(stage1) => {
            set_u64(
                vcpu,
                sys_reg_id(TTBR0_EL1),
                stage1.ttbr0_el1,
                "initialize TTBR0_EL1",
            )?;
            set_u64(
                vcpu,
                sys_reg_id(TCR_EL1),
                stage1.tcr_el1,
                "initialize TCR_EL1",
            )?;
            set_u64(
                vcpu,
                sys_reg_id(MAIR_EL1),
                stage1.mair_el1,
                "initialize MAIR_EL1",
            )?;
            set_u64(
                vcpu,
                sys_reg_id(VBAR_EL1),
                stage1.vbar_el1,
                "initialize VBAR_EL1",
            )?;
            set_u64(
                vcpu,
                sys_reg_id(SCTLR_EL1),
                stage1.sctlr_el1,
                "initialize SCTLR_EL1",
            )?;
        }
    }
    Ok(())
}

const fn core_reg_id(byte_offset: usize, size: u64) -> u64 {
    KVM_REG_ARM64 | size | KVM_REG_ARM_CORE as u64 | (byte_offset as u64 / 4)
}

const fn x_offset(register: usize) -> usize {
    std::mem::offset_of!(kvm_regs, regs)
        + std::mem::offset_of!(user_pt_regs, regs)
        + register * size_of::<u64>()
}

const fn sp_el0_offset() -> usize {
    std::mem::offset_of!(kvm_regs, regs) + std::mem::offset_of!(user_pt_regs, sp)
}

const fn pc_offset() -> usize {
    std::mem::offset_of!(kvm_regs, regs) + std::mem::offset_of!(user_pt_regs, pc)
}

const fn pstate_offset() -> usize {
    std::mem::offset_of!(kvm_regs, regs) + std::mem::offset_of!(user_pt_regs, pstate)
}

const fn sp_el1_offset() -> usize {
    std::mem::offset_of!(kvm_regs, sp_el1)
}

const fn simd_offset(register: usize) -> usize {
    std::mem::offset_of!(kvm_regs, fp_regs)
        + std::mem::offset_of!(user_fpsimd_state, vregs)
        + register * size_of::<u128>()
}

const fn fpsr_offset() -> usize {
    std::mem::offset_of!(kvm_regs, fp_regs) + std::mem::offset_of!(user_fpsimd_state, fpsr)
}

const fn fpcr_offset() -> usize {
    std::mem::offset_of!(kvm_regs, fp_regs) + std::mem::offset_of!(user_fpsimd_state, fpcr)
}

const fn sys_reg_id(encoding: u16) -> u64 {
    KVM_REG_ARM64 | KVM_REG_SIZE_U64 | KVM_REG_ARM64_SYSREG as u64 | encoding as u64
}

// Kernel ABI facts, not test expectations. `KVM_REG_ARM_CORE` ids are indexes
// into `kvm_regs`, so a binding revision that reordered or repadded that
// struct would silently retarget every boot register. The required
// Linux/aarch64 build lane fails here rather than at run time on Rasputin.
const _: () = {
    assert!(x_offset(0) + 30 * size_of::<u64>() == x_offset(30));
    assert!(x_offset(30) + size_of::<u64>() == sp_el0_offset());
    assert!(sp_el0_offset() + size_of::<u64>() == pc_offset());
    assert!(pc_offset() + size_of::<u64>() == pstate_offset());
    assert!(pstate_offset() + size_of::<u64>() == sp_el1_offset());
    assert!(simd_offset(0) + 31 * size_of::<u128>() == simd_offset(31));
    assert!(simd_offset(31) + size_of::<u128>() == fpsr_offset());
    assert!(fpsr_offset() + size_of::<u32>() == fpcr_offset());
    assert!(sys_reg_id(CPACR_EL1) & 0xffff == CPACR_EL1 as u64);
};

fn get_u64(vcpu: &VcpuFd, id: u64) -> Result<u64, VmmError> {
    let mut bytes = [0_u8; 8];
    vcpu.get_one_reg(id, &mut bytes)
        .map_err(|error| errno_error("KVM_GET_ONE_REG", error))?;
    Ok(u64::from_ne_bytes(bytes))
}

fn set_u64(vcpu: &VcpuFd, id: u64, value: u64, operation: &'static str) -> Result<(), VmmError> {
    set_bytes(vcpu, id, &value.to_ne_bytes(), operation)
}

fn set_u32(vcpu: &VcpuFd, id: u64, value: u32, operation: &'static str) -> Result<(), VmmError> {
    set_bytes(vcpu, id, &value.to_ne_bytes(), operation)
}

fn set_bytes(
    vcpu: &VcpuFd,
    id: u64,
    bytes: &[u8],
    operation: &'static str,
) -> Result<(), VmmError> {
    vcpu.set_one_reg(id, bytes)
        .map(|_| ())
        .map_err(|error| errno_error(operation, error))
}

fn checked_mmio_width(address: u64, width: usize) -> Result<u8, VmmError> {
    if matches!(width, 1 | 2 | 4 | 8) {
        Ok(width as u8)
    } else {
        Err(kvm_error(
            "normalize KVM MMIO",
            0,
            format!("access at {address:#x} has invalid width {width}"),
        ))
    }
}

fn next_sequence(sequence: u64) -> Result<u64, VmmError> {
    sequence
        .checked_add(1)
        .ok_or_else(|| kvm_error("normalize KVM MMIO", 0, "MMIO token overflow"))
}

fn require_positive(operation: &'static str, value: i32) -> Result<i32, VmmError> {
    if value > 0 {
        Ok(value)
    } else {
        Err(kvm_error(
            operation,
            value as i64,
            "required capability is absent",
        ))
    }
}

fn errno_error(operation: &'static str, error: kvm_ioctls::Error) -> VmmError {
    kvm_error(operation, error.errno() as i64, error.to_string())
}

fn kvm_error(operation: &'static str, code: i64, detail: impl Into<String>) -> VmmError {
    host_error(BackendKind::Kvm, operation, code, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_acceptance_requires_every_real_prerequisite_but_not_readonly_memory() {
        let complete = KvmCapabilityProbe {
            api_version: true,
            immediate_exit: true,
            ipa_size: true,
            isa_baseline: true,
            one_reg: true,
            readonly_mem: false,
            target: true,
            user_memory: true,
            vcpu_count: true,
            writable_id_regs: true,
            ipa_bits: 40,
            max_vcpus: 3,
            isa: Some(IsaCapabilities {
                armv8_2: true,
                fp: true,
                asimd: true,
                fp16: false,
                sve: false,
                pointer_auth: false,
                mte: false,
            }),
        };
        assert!(complete.required_ok());
        assert!(
            KvmCapabilityProbe {
                readonly_mem: true,
                ..complete
            }
            .required_ok()
        );
        for refused in [
            KvmCapabilityProbe {
                target: false,
                ..complete
            },
            KvmCapabilityProbe {
                user_memory: false,
                ..complete
            },
            KvmCapabilityProbe {
                one_reg: false,
                ..complete
            },
            KvmCapabilityProbe {
                writable_id_regs: false,
                ..complete
            },
            KvmCapabilityProbe {
                isa_baseline: false,
                ..complete
            },
        ] {
            assert!(!refused.required_ok());
        }
    }

    #[test]
    fn interrupt_classification_requires_an_explicit_cancel_generation() {
        let classify = |observed: u64, current: u64| current != observed;
        assert!(!classify(7, 7), "incidental EINTR must be retried");
        assert!(
            classify(7, 8),
            "an explicit simultaneous cancel must terminate"
        );
    }

    /// `immediate_exit` is read only at `KVM_RUN` entry, so a watchdog that
    /// set it alone could never stop a vCPU already executing guest code: the
    /// boot would hang until the lab agent's deadline killed the process,
    /// destroying the transcript the timeout is supposed to carry. The kick
    /// must interrupt a thread that is blocked inside a syscall.
    #[test]
    fn a_cancel_ejects_a_thread_that_is_already_blocked_in_a_syscall() {
        #[repr(C)]
        struct TimeSpec {
            seconds: i64,
            nanoseconds: i64,
        }
        unsafe extern "C" {
            fn nanosleep(request: *const TimeSpec, remaining: *mut TimeSpec) -> i32;
        }

        let flag: &'static mut u8 = Box::leak(Box::new(0_u8));
        let state = Arc::new(ImmediateExit(Mutex::new(ImmediateExitState {
            ptr: Some(NonNull::from(flag)),
            generation: 0,
            thread: 0,
        })));
        let handle = KvmVcpuHandle(Arc::clone(&state));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            install_vcpu_kick_handler().expect("kick handler installs on the owning thread");
            state.0.lock().expect("fresh mutex").thread = unsafe { pthread_self() };
            ready_tx.send(()).expect("handshake");
            let request = TimeSpec {
                seconds: 60,
                nanoseconds: 0,
            };
            let mut remaining = TimeSpec {
                seconds: 0,
                nanoseconds: 0,
            };
            let started = std::time::Instant::now();
            let result = unsafe { nanosleep(&request, &mut remaining) };
            (result, started.elapsed())
        });
        ready_rx.recv().expect("worker is blocked");
        // Give the worker time to actually enter the syscall, so the kick has
        // to eject it rather than being observed at a boundary.
        std::thread::sleep(std::time::Duration::from_millis(50));
        handle.cancel().expect("cancel delivers the kick");
        let (result, elapsed) = worker.join().expect("worker returns");
        assert_eq!(result, -1, "the blocked syscall must report interruption");
        assert!(
            elapsed < std::time::Duration::from_secs(30),
            "the kick must eject the blocked thread promptly, took {elapsed:?}"
        );
        assert_eq!(
            unsafe { std::ptr::read_volatile(handle.0.0.lock().unwrap().ptr.unwrap().as_ptr()) },
            1,
            "the entry-side immediate_exit flag is still published"
        );
    }

    #[test]
    fn mmio_width_is_strict() {
        for width in [1, 2, 4, 8] {
            assert_eq!(checked_mmio_width(0, width).unwrap(), width as u8);
        }
        for width in [0, 3, 16] {
            assert!(checked_mmio_width(0, width).is_err());
        }
    }

    #[test]
    fn id_mask_preserves_immutable_fp_versions_and_reduces_mutable_features() {
        let pfr0 = (1 << 16) | (1 << 20) | (1 << 32);
        let pfr1 = 1 << 8;
        let isar0 = 3 << 20;
        let isar1 = (1 << 4) | (1 << 8) | (1 << 24) | (1 << 28);
        let isar2 = (1 << 12) | (1 << 16);
        let masked = baseline_id_registers([pfr0, pfr1, isar0, isar1, isar2]).unwrap();
        assert_eq!(masked[0], (1 << 16) | (1 << 20));
        assert_eq!(masked[1], 0);
        assert_eq!(masked[2], 2 << 20);
        assert_eq!(masked[3], 0);
        assert_eq!(masked[4], 0);
    }

    #[test]
    fn id_mask_refuses_missing_baseline_features() {
        let error = baseline_id_registers([0xf << 16, 0, 2 << 20, 0, 0]).unwrap_err();
        assert!(error.to_string().contains("validate Wrela ISA baseline"));
        let error = baseline_id_registers([0, 0, 1 << 20, 0, 0]).unwrap_err();
        assert!(error.to_string().contains("Atomics=0x1"));
    }
}
