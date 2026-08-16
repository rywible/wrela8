//! Linux/aarch64 KVM boot path.
//!
//! This intentionally uses the stable kernel UAPI directly. The VMM crate
//! has no Cargo dependency on a KVM wrapper, and the display device remains
//! the same portable model used by HVF and headless replay.

use std::ffi::{c_int, c_ulong, c_void};
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::path::Path;
use std::ptr::NonNull;
use std::time::Duration;

use wrela_machine::{layout, machine_info, mmio, pixels};

use crate::display::{DisplayBackendSelection, RuntimeDisplay};
use crate::exit_loop::{
    AdmissionWitness, apply_entropy_read, check_core_marks, commit_admissions, drain_console,
    observe_admissions, read_core_mark,
};
use crate::record::{self, ChoiceEntry, ChoiceRequest};
use crate::{BootOutcome, VmmError, parse_report, validate_report_digests};

const KVMIO: usize = 0xae;
const KVM_API_VERSION: c_int = 12;
const KVM_EXIT_MMIO: u32 = 6;
const KVM_EXIT_SYSTEM_EVENT: u32 = 24;
const KVM_SYSTEM_EVENT_SHUTDOWN: u32 = 1;
const KVM_SYSTEM_EVENT_RESET: u32 = 2;
const KVM_RUN_EXIT_REASON_OFFSET: usize = 8;
const KVM_RUN_UNION_OFFSET: usize = 32;
const KVM_RUN_MMIO_DATA_OFFSET: usize = 40;
const KVM_RUN_MMIO_LEN_OFFSET: usize = 48;
const KVM_RUN_MMIO_IS_WRITE_OFFSET: usize = 52;
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const MAP_PRIVATE: c_int = 2;
const MAP_ANONYMOUS: c_int = 0x20;

const fn io(number: usize) -> usize {
    (KVMIO << 8) | number
}

const fn iow<T>(number: usize) -> usize {
    (1_usize << 30) | (std::mem::size_of::<T>() << 16) | io(number)
}

const fn ior<T>(number: usize) -> usize {
    (2_usize << 30) | (std::mem::size_of::<T>() << 16) | io(number)
}

const KVM_GET_API_VERSION: usize = io(0x00);
const KVM_CREATE_VM: usize = io(0x01);
const KVM_GET_VCPU_MMAP_SIZE: usize = io(0x04);
const KVM_CREATE_VCPU: usize = io(0x41);
const KVM_SET_USER_MEMORY_REGION: usize = iow::<KvmUserspaceMemoryRegion>(0x46);
const KVM_RUN: usize = io(0x80);
const KVM_SET_ONE_REG: usize = iow::<KvmOneReg>(0xac);
const KVM_ARM_VCPU_INIT: usize = iow::<KvmVcpuInit>(0xae);
const KVM_ARM_PREFERRED_TARGET: usize = ior::<KvmVcpuInit>(0xaf);

const KVM_REG_ARM64: u64 = 0x6000_0000_0000_0000;
const KVM_REG_SIZE_U64: u64 = 0x0030_0000_0000_0000;
// `KVM_REG_ARM_COPROC_SHIFT` is 16 in the Linux arm64 UAPI.
const KVM_REG_ARM_CORE: u64 = 0x1000_0000;
const KVM_REG_ARM64_SYSREG: u64 = 0x1300_0000;
const REG_X0: u64 = KVM_REG_ARM64 | KVM_REG_SIZE_U64 | KVM_REG_ARM_CORE;
const REG_PC: u64 = REG_X0 | 64;
const REG_PSTATE: u64 = REG_X0 | 66;
// PSTATE 0x3c5 starts in EL1h, which consumes SP_EL1. Core-register index
// 62 is SP_EL0; SP_EL1 follows `user_pt_regs` at index 68.
const REG_SP_EL1: u64 = REG_X0 | 68;
// S3_0_C1_C0_2, the architectural CPACR_EL1 encoding.
const REG_CPACR_EL1: u64 = KVM_REG_ARM64 | KVM_REG_SIZE_U64 | KVM_REG_ARM64_SYSREG | 0xc082;
// S3_3_C4_C4_0, the architectural FPCR encoding.
const REG_FPCR: u64 = KVM_REG_ARM64 | KVM_REG_SIZE_U64 | KVM_REG_ARM64_SYSREG | 0xda20;

#[repr(C)]
struct KvmUserspaceMemoryRegion {
    slot: u32,
    flags: u32,
    guest_phys_addr: u64,
    memory_size: u64,
    userspace_addr: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KvmVcpuInit {
    target: u32,
    features: [u32; 7],
}

#[repr(C)]
struct KvmOneReg {
    id: u64,
    addr: u64,
}

struct Mapping {
    ptr: NonNull<u8>,
    len: usize,
}

impl Mapping {
    fn anonymous(len: usize) -> Result<Self, VmmError> {
        let raw = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        let Some(ptr) = NonNull::new(raw.cast::<u8>()).filter(|ptr| ptr.as_ptr() as isize != -1)
        else {
            return Err(io_error("mmap guest DRAM"));
        };
        Ok(Self { ptr, len })
    }

    fn shared(fd: RawFd, len: usize) -> Result<Self, VmmError> {
        let raw = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        let Some(ptr) = NonNull::new(raw.cast::<u8>()).filter(|ptr| ptr.as_ptr() as isize != -1)
        else {
            return Err(io_error("mmap KVM run structure"));
        };
        Ok(Self { ptr, len })
    }

    fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            let _ = munmap(self.ptr.as_ptr().cast(), self.len);
        }
    }
}

struct Vcpu {
    _fd: File,
    run: Mapping,
}

impl Vcpu {
    fn run(&mut self) -> Result<MmioExit, VmmError> {
        let result = unsafe { ioctl(self._fd.as_raw_fd(), KVM_RUN as c_ulong) };
        if result != 0 {
            return Err(io_error("KVM_RUN"));
        }
        let bytes = self.run.bytes();
        let exit_reason = read_u32(bytes, KVM_RUN_EXIT_REASON_OFFSET)?;
        match exit_reason {
            KVM_EXIT_MMIO => {
                let addr = read_u64(bytes, KVM_RUN_UNION_OFFSET)?;
                let len = read_u32(bytes, KVM_RUN_MMIO_LEN_OFFSET)? as usize;
                let write = *bytes
                    .get(KVM_RUN_MMIO_IS_WRITE_OFFSET)
                    .ok_or_else(|| VmmError::GuestFault("short KVM MMIO exit".into()))?
                    != 0;
                if len == 0 || len > 8 {
                    return Err(VmmError::GuestFault(format!(
                        "KVM MMIO at {addr:#x} has unsupported width {len}"
                    )));
                }
                let mut data = [0_u8; 8];
                data[..len].copy_from_slice(
                    bytes
                        .get(KVM_RUN_MMIO_DATA_OFFSET..KVM_RUN_MMIO_DATA_OFFSET + len)
                        .ok_or_else(|| VmmError::GuestFault("short KVM MMIO data".into()))?,
                );
                Ok(MmioExit::Access {
                    addr,
                    write,
                    len,
                    data,
                })
            }
            KVM_EXIT_SYSTEM_EVENT => {
                let kind = read_u32(bytes, KVM_RUN_UNION_OFFSET)?;
                if matches!(kind, KVM_SYSTEM_EVENT_SHUTDOWN | KVM_SYSTEM_EVENT_RESET) {
                    Ok(MmioExit::SystemEvent(kind))
                } else {
                    Err(VmmError::GuestFault(format!(
                        "unhandled KVM system event {kind}"
                    )))
                }
            }
            other => Err(VmmError::GuestFault(format!(
                "unhandled KVM exit reason {other}"
            ))),
        }
    }

    fn complete_read(&mut self, len: usize, value: u64) -> Result<(), VmmError> {
        let target = unsafe { std::slice::from_raw_parts_mut(self.run.ptr.as_ptr(), self.run.len) };
        target
            .get_mut(KVM_RUN_MMIO_DATA_OFFSET..KVM_RUN_MMIO_DATA_OFFSET + len)
            .ok_or_else(|| VmmError::GuestFault("short KVM MMIO read buffer".into()))?
            .copy_from_slice(&value.to_le_bytes()[..len]);
        Ok(())
    }
}

enum MmioExit {
    Access {
        addr: u64,
        write: bool,
        len: usize,
        data: [u8; 8],
    },
    SystemEvent(u32),
}

pub(super) fn boot(
    report_path: &Path,
    img_path: &Path,
    display_selection: DisplayBackendSelection,
    replay_choices: Option<Vec<record::ChoiceEntry>>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    let report_text = std::fs::read_to_string(report_path)
        .map_err(|error| VmmError::Io(format!("read {}: {error}", report_path.display())))?;
    let parsed = parse_report(&report_text)?;
    if parsed.blk.is_some() {
        return Err(VmmError::Unsupported(
            "Linux/KVM block-device MMIO is not implemented",
        ));
    }
    let image = std::fs::read(img_path)
        .map_err(|error| VmmError::Io(format!("read {}: {error}", img_path.display())))?;
    validate_report_digests(&parsed, &image)?;
    let image_offset = usize::try_from(layout::IMAGE_BASE - layout::DRAM_BASE)
        .map_err(|_| VmmError::BadImage("image offset does not fit usize".into()))?;
    let image_end = image_offset
        .checked_add(image.len())
        .filter(|end| *end <= layout::DRAM_SIZE as usize)
        .ok_or_else(|| VmmError::BadImage("image exceeds machine-v1 DRAM".into()))?;

    let kvm = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .map_err(|error| VmmError::Io(format!("open /dev/kvm: {error}")))?;
    if ioctl_noarg(kvm.as_raw_fd(), KVM_GET_API_VERSION)? != KVM_API_VERSION {
        return Err(VmmError::Unsupported("host KVM API version is not 12"));
    }
    let vm = ioctl_fd(kvm.as_raw_fd(), KVM_CREATE_VM, 0, "KVM_CREATE_VM")?;
    let memory = Mapping::anonymous(layout::DRAM_SIZE as usize)?;
    unsafe {
        std::ptr::copy_nonoverlapping(
            image.as_ptr(),
            memory.ptr.as_ptr().add(image_offset),
            image.len(),
        );
    }
    debug_assert!(image_end <= memory.len);
    let region = KvmUserspaceMemoryRegion {
        slot: 0,
        flags: 0,
        guest_phys_addr: layout::DRAM_BASE,
        memory_size: layout::DRAM_SIZE,
        userspace_addr: memory.ptr.as_ptr() as u64,
    };
    ioctl_ptr(
        vm.as_raw_fd(),
        KVM_SET_USER_MEMORY_REGION,
        (&region as *const KvmUserspaceMemoryRegion)
            .cast_mut()
            .cast(),
        "KVM_SET_USER_MEMORY_REGION",
    )?;
    initialize_machine_info(memory.ptr.as_ptr())?;

    let mmap_size = usize::try_from(ioctl_noarg(kvm.as_raw_fd(), KVM_GET_VCPU_MMAP_SIZE)?)
        .map_err(|_| VmmError::Unsupported("KVM vCPU mmap size does not fit usize"))?;
    let cores = parsed.cores;
    if !(1..=wrela_machine::CORE_SLOTS).contains(&cores) {
        return Err(VmmError::MalformedReport(format!(
            "Cores count={cores} is outside machine-v1 slots"
        )));
    }
    let mut vcpus = Vec::with_capacity(cores);
    for core in 0..cores {
        let fd = ioctl_fd(
            vm.as_raw_fd(),
            KVM_CREATE_VCPU,
            core as c_ulong,
            "KVM_CREATE_VCPU",
        )?;
        let mut init = KvmVcpuInit::default();
        ioctl_ptr(
            vm.as_raw_fd(),
            KVM_ARM_PREFERRED_TARGET,
            (&mut init as *mut KvmVcpuInit).cast(),
            "KVM_ARM_PREFERRED_TARGET",
        )?;
        ioctl_ptr(
            fd.as_raw_fd(),
            KVM_ARM_VCPU_INIT,
            (&mut init as *mut KvmVcpuInit).cast(),
            "KVM_ARM_VCPU_INIT",
        )?;
        let entry = parsed
            .core_entries
            .iter()
            .find_map(|entry| (entry.core == core).then_some(entry.base))
            .unwrap_or(parsed.entry);
        let stack = if let Some(stack) = parsed.core_stacks.iter().find(|stack| stack.core == core)
        {
            stack.base.checked_add(stack.size).ok_or_else(|| {
                VmmError::MalformedReport(format!("core {core} stack top overflows u64"))
            })?
        } else {
            layout::core_stack_base_n(core, cores) + layout::CORE_STACK_SIZE
        };
        set_one_reg(fd.as_raw_fd(), REG_X0, layout::MACHINE_INFO_BASE)?;
        set_one_reg(fd.as_raw_fd(), REG_PC, entry)?;
        set_one_reg(fd.as_raw_fd(), REG_SP_EL1, stack)?;
        set_one_reg(fd.as_raw_fd(), REG_PSTATE, 0x3c5)?;
        set_one_reg(fd.as_raw_fd(), REG_CPACR_EL1, 0x0030_0000)?;
        set_one_reg(fd.as_raw_fd(), REG_FPCR, crate::GUEST_FPCR)?;
        vcpus.push(Vcpu {
            run: Mapping::shared(fd.as_raw_fd(), mmap_size)?,
            _fd: fd,
        });
    }

    let mut display = crate::display::runtime_display(display_selection)?;
    let mut chooser = match replay_choices {
        Some(log) => record::Chooser::replayer(log).strict(),
        None => record::Chooser::recorder(),
    };
    let mut admission = AdmissionWitness::new(parsed.request_rings.clone());
    let mut active = vec![false; cores];
    active[0] = true;
    let mut released = false;
    let mut current = 0_usize;
    let mut exits = 0_u64;
    let exit_code = loop {
        if !active[current] {
            current = (current + 1) % cores;
            continue;
        }
        exits = exits
            .checked_add(1)
            .ok_or_else(|| VmmError::GuestFault("KVM exit counter overflow".into()))?;
        match vcpus[current].run()? {
            MmioExit::SystemEvent(kind) => {
                return Err(VmmError::GuestFault(format!(
                    "KVM guest terminated through system event {kind} instead of the machine-v1 exit doorbell"
                )));
            }
            MmioExit::Access {
                addr,
                write,
                len,
                data,
            } => {
                if addr == mmio::CLOCK_MMIO_ADDR {
                    require_access(addr, write, len, false)?;
                    let choice = chooser.choose_checked(ChoiceRequest::ClockRead, || {
                        ChoiceEntry::ClockRead {
                            value: crate::exit_loop::monotonic_ns(),
                        }
                    })?;
                    let ChoiceEntry::ClockRead { value } = choice else {
                        unreachable!("clock request returns a clock choice")
                    };
                    vcpus[current].complete_read(len, value)?;
                } else {
                    require_access(addr, write, len, true)?;
                    let value = u64::from_le_bytes(data);
                    if addr == mmio::EXIT_MMIO_ADDR {
                        break value;
                    } else if addr == mmio::RELEASE_MMIO_ADDR {
                        if current != 0 || value != cores as u64 || released {
                            return Err(VmmError::GuestFault(format!(
                                "invalid or repeated KVM release from core {current}: {value}"
                            )));
                        }
                        active.fill(true);
                        released = true;
                    } else if addr == mmio::PARK_MMIO_ADDR {
                        std::thread::sleep(Duration::from_micros(50));
                    } else if addr == mmio::ENTROPY_MMIO_ADDR {
                        service_entropy(&mut chooser, memory.ptr.as_ptr())?;
                    } else if pixels::is_display_doorbell_addr(addr) {
                        service_display(
                            &mut display,
                            &mut chooser,
                            addr,
                            memory.ptr.as_ptr(),
                            value,
                        )?;
                    } else if addr == mmio::QUIESCE_MMIO_ADDR {
                        return Err(VmmError::GuestFault(
                            "KVM guest rang a block quiesce doorbell without a block device".into(),
                        ));
                    } else {
                        return Err(VmmError::GuestFault(format!(
                            "unhandled KVM MMIO write at {addr:#x} from core {current}"
                        )));
                    }
                }
            }
        }
        let admitted = observe_admissions(&mut admission, memory.ptr.as_ptr(), current)?;
        commit_admissions(&mut chooser, &admitted)?;
        let live_next = (1..=cores)
            .map(|step| (current + step) % cores)
            .find(|core| active[*core])
            .ok_or_else(|| VmmError::GuestFault("KVM scheduler has no active core".into()))?;
        let index = chooser.resolved_count();
        let choice = chooser.choose_checked(
            ChoiceRequest::Progress {
                core: live_next as u32,
            },
            || ChoiceEntry::Progress {
                core: live_next as u32,
            },
        )?;
        let ChoiceEntry::Progress { core: forced } = choice else {
            unreachable!("progress request returns a progress choice")
        };
        let forced = usize::try_from(forced).unwrap_or(usize::MAX);
        if forced >= cores || !active[forced] {
            chooser.note_divergence_checked(record::Divergence::ProgressMismatch {
                index,
                recorded: u32::try_from(forced).unwrap_or(u32::MAX),
                actual: live_next as u32,
            })?;
            current = live_next;
        } else {
            current = forced;
        }
    };

    if released {
        check_core_marks(memory.ptr.as_ptr(), cores)?;
    }
    if exit_code == 0 {
        let latch_offset = usize::try_from(
            layout::MACHINE_INFO_BASE - layout::DRAM_BASE + machine_info::OFF_ABORT_LATCH,
        )
        .map_err(|_| VmmError::GuestFault("abort latch offset does not fit usize".into()))?;
        let latch = unsafe {
            std::ptr::read_unaligned(memory.ptr.as_ptr().add(latch_offset).cast::<u64>())
        };
        if latch != 0 {
            return Err(VmmError::GuestFault(format!(
                "abort re-entrancy latch is {latch:#x} after a green KVM boot"
            )));
        }
    }
    if let Some(path) = std::env::var_os("WRELA_P8_STATE_DUMP") {
        let placement = parsed.renderer_placements.first().ok_or_else(|| {
            VmmError::MalformedReport("P8 state dump requested without a renderer".into())
        })?;
        let offset = usize::try_from(placement.state_base - layout::DRAM_BASE)
            .map_err(|_| VmmError::BadImage("renderer state offset does not fit usize".into()))?;
        let len = usize::try_from(placement.state_size)
            .map_err(|_| VmmError::BadImage("renderer state size does not fit usize".into()))?;
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= memory.len)
            .ok_or_else(|| VmmError::BadImage("renderer state range exceeds DRAM".into()))?;
        let bytes =
            unsafe { std::slice::from_raw_parts(memory.ptr.as_ptr().add(offset), end - offset) };
        std::fs::write(&path, bytes).map_err(|error| {
            VmmError::Io(format!(
                "write requested P8 renderer-state dump {}: {error}",
                std::path::Path::new(&path).display()
            ))
        })?;
    }
    let mut transcript = drain_console(memory.ptr.as_ptr());
    transcript.extend_from_slice(&crate::replay::frame_log_bytes(display.frames()));
    transcript.extend_from_slice(&crate::replay::rejected_display_event_log_bytes(
        display.events(),
    ));
    let (choices, divergences) = record::finish_chooser(chooser)?;
    Ok((
        BootOutcome {
            transcript,
            exit_code,
            choices,
            exits,
            core_marks: (0..cores)
                .map(|core| read_core_mark(memory.ptr.as_ptr(), core))
                .collect(),
            lane2_hits: crate::lane3::read_lane2_hits(memory.ptr.as_ptr()),
            frames: display.frames().to_vec(),
            frame_buffer_digests: display.backend_digests().to_vec(),
        },
        divergences,
    ))
}

fn service_display(
    display: &mut RuntimeDisplay,
    chooser: &mut record::Chooser,
    doorbell_addr: u64,
    dram: *mut u8,
    control_addr: u64,
) -> Result<(), VmmError> {
    let frame = unsafe {
        display.consume_volatile_from(
            doorbell_addr,
            dram,
            layout::DRAM_SIZE as usize,
            control_addr,
        )
    };
    if matches!(&frame, Err(VmmError::Io(_))) {
        // A host-service failure after an accepted native commit has no safe
        // guest-visible rejection state. Abort the VMM without publishing a
        // false COMPLETION_REJECTED result.
        return frame.map(|_| ());
    }
    let status = display.last_completion_status();
    // SAFETY: `dram` is the live guest mapping for this boot.
    unsafe { crate::display::publish_completion_status(dram, control_addr, status)? };
    if let Ok(frame) = frame {
        chooser.check_frame_output(&frame)?;
    }
    // Descriptor/backend rejection is a terminal device completion, not a
    // host boot failure. Resume the guest so the coordinator can reclaim the
    // back generation and surface RenderError::Display.
    Ok(())
}

fn service_entropy(chooser: &mut record::Chooser, dram: *mut u8) -> Result<(), VmmError> {
    let info = (layout::MACHINE_INFO_BASE - layout::DRAM_BASE) as usize;
    let read = |offset: u64| unsafe {
        std::ptr::read_unaligned(dram.add(info + offset as usize).cast::<u64>())
    };
    let destination = read(machine_info::OFF_ENTROPY_DEST);
    let length = usize::try_from(read(machine_info::OFF_ENTROPY_LEN))
        .map_err(|_| VmmError::GuestFault("entropy length does not fit usize".into()))?;
    if length > machine_info::ENTROPY_LEN_MAX as usize {
        return Err(VmmError::GuestFault(format!(
            "entropy request length {length} exceeds machine-v1 maximum {}",
            machine_info::ENTROPY_LEN_MAX,
        )));
    }
    apply_entropy_read(chooser, dram, destination, length as u64)
}

fn initialize_machine_info(dram: *mut u8) -> Result<(), VmmError> {
    let info = (layout::MACHINE_INFO_BASE - layout::DRAM_BASE) as usize;
    let revision = wrela_machine::MACHINE_REVISION_STR.as_bytes();
    unsafe {
        std::ptr::copy_nonoverlapping(
            revision.as_ptr(),
            dram.add(info + machine_info::OFF_REVISION as usize),
            revision.len(),
        );
        std::ptr::write_bytes(dram.add(info + machine_info::OFF_WALL_SEED as usize), 0, 8);
    }
    Ok(())
}

fn require_access(addr: u64, write: bool, len: usize, want_write: bool) -> Result<(), VmmError> {
    if len != 8 || write != want_write {
        return Err(VmmError::GuestFault(format!(
            "KVM MMIO at {addr:#x} expected {} 8-byte access, got {} {len}-byte access",
            if want_write { "a write" } else { "a read" },
            if write { "a write" } else { "a read" },
        )));
    }
    Ok(())
}

fn set_one_reg(fd: RawFd, id: u64, mut value: u64) -> Result<(), VmmError> {
    let register = KvmOneReg {
        id,
        addr: (&mut value as *mut u64) as u64,
    };
    ioctl_ptr(
        fd,
        KVM_SET_ONE_REG,
        (&register as *const KvmOneReg).cast_mut().cast(),
        "KVM_SET_ONE_REG",
    )
}

fn ioctl_noarg(fd: RawFd, request: usize) -> Result<c_int, VmmError> {
    let result = unsafe { ioctl(fd, request as c_ulong) };
    if result < 0 {
        Err(io_error("KVM ioctl"))
    } else {
        Ok(result)
    }
}

fn ioctl_fd(fd: RawFd, request: usize, argument: c_ulong, what: &str) -> Result<File, VmmError> {
    let result = unsafe { ioctl(fd, request as c_ulong, argument) };
    if result < 0 {
        Err(io_error(what))
    } else {
        Ok(unsafe { File::from_raw_fd(result) })
    }
}

fn ioctl_ptr(fd: RawFd, request: usize, argument: *mut c_void, what: &str) -> Result<(), VmmError> {
    if unsafe { ioctl(fd, request as c_ulong, argument) } != 0 {
        Err(io_error(what))
    } else {
        Ok(())
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, VmmError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_ne_bytes)
        .ok_or_else(|| VmmError::GuestFault("short KVM run structure".into()))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, VmmError> {
    bytes
        .get(offset..offset + 8)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_ne_bytes)
        .ok_or_else(|| VmmError::GuestFault("short KVM run structure".into()))
}

fn io_error(what: &str) -> VmmError {
    VmmError::Io(format!("{what}: {}", std::io::Error::last_os_error()))
}

unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> c_int;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm64_kvm_uapi_constants_match_linux_headers() {
        assert_eq!(KVM_SET_USER_MEMORY_REGION, 0x4020_ae46);
        assert_eq!(KVM_SET_ONE_REG, 0x4010_aeac);
        assert_eq!(KVM_ARM_VCPU_INIT, 0x4020_aeae);
        assert_eq!(KVM_ARM_PREFERRED_TARGET, 0x8020_aeaf);
        assert_eq!(REG_X0, 0x6030_0000_1000_0000);
        assert_eq!(REG_SP_EL1, 0x6030_0000_1000_0044);
        assert_eq!(REG_PC, 0x6030_0000_1000_0040);
        assert_eq!(REG_PSTATE, 0x6030_0000_1000_0042);
        assert_eq!(REG_CPACR_EL1, 0x6030_0000_1300_c082);
        assert_eq!(REG_FPCR, 0x6030_0000_1300_da20);
        assert_eq!(crate::GUEST_FPCR, 0x0200_0000);
        assert_eq!(KVM_RUN_EXIT_REASON_OFFSET, 8);
        assert_eq!(KVM_RUN_UNION_OFFSET, 32);
        assert_eq!(KVM_RUN_MMIO_DATA_OFFSET, 40);
        assert_eq!(KVM_RUN_MMIO_LEN_OFFSET, 48);
        assert_eq!(KVM_RUN_MMIO_IS_WRITE_OFFSET, 52);
    }

    #[test]
    #[ignore = "Linux/KVM+DRM lane: set WRELA_KVM_REPORT and WRELA_KVM_IMAGE to a verified one-frame Pixels guest"]
    fn pixels_guest_doorbell_reaches_native_drm_and_preserves_bgra() {
        let report = std::env::var_os("WRELA_KVM_REPORT")
            .map(std::path::PathBuf::from)
            .expect("WRELA_KVM_REPORT");
        let image = std::env::var_os("WRELA_KVM_IMAGE")
            .map(std::path::PathBuf::from)
            .expect("WRELA_KVM_IMAGE");
        let (outcome, divergences) = boot(&report, &image, DisplayBackendSelection::Native, None)
            .expect("KVM guest and native DRM presentation complete");
        assert!(divergences.is_empty());
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(
            outcome.frames.len(),
            1,
            "one guest doorbell produces one frame"
        );
        assert_eq!(
            outcome.frame_buffer_digests,
            vec![outcome.frames[0].visible_digest],
            "native DRM mapping preserves the guest's exact BGRA bytes",
        );
    }

    #[test]
    #[ignore = "Linux/KVM lane: set WRELA_KVM_REPORT and WRELA_KVM_IMAGE to a verified one-frame Pixels guest"]
    fn pixels_guest_record_replays_frame_output_and_schedule() {
        let report = std::env::var_os("WRELA_KVM_REPORT")
            .map(std::path::PathBuf::from)
            .expect("WRELA_KVM_REPORT");
        let image = std::env::var_os("WRELA_KVM_IMAGE")
            .map(std::path::PathBuf::from)
            .expect("WRELA_KVM_IMAGE");
        let (recorded, divergences) =
            boot(&report, &image, DisplayBackendSelection::Headless, None)
                .expect("KVM recording boot");
        assert!(divergences.is_empty());
        assert!(
            recorded
                .choices
                .iter()
                .any(|choice| { matches!(choice, record::ChoiceEntry::FrameOutputV1(_)) })
        );
        let (replayed, divergences) = boot(
            &report,
            &image,
            DisplayBackendSelection::Headless,
            Some(recorded.choices.clone()),
        )
        .expect("KVM replay boot");
        assert!(divergences.is_empty());
        assert_eq!(replayed.frames, recorded.frames);
        assert_eq!(replayed.transcript, recorded.transcript);
    }
}
