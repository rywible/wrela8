//! Optional guest-only PMU group owned by one Linux/KVM vCPU thread.

use crate::VmmError;

pub(crate) const COUNTER_NAMES: [&str; 7] = [
    "br_mis_pred",
    "cpu_cycles",
    "inst_retired",
    "l1d_cache_refill",
    "l2d_cache_refill",
    "stall_backend",
    "stall_frontend",
];
const ATTR_DISABLED: u64 = 1 << 0;
const ATTR_PINNED: u64 = 1 << 2;
const ATTR_EXCLUDE_HOST: u64 = 1 << 19;

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
mod platform {
    use std::ffi::{c_int, c_long, c_ulong, c_void};
    use std::os::fd::RawFd;

    use super::*;

    const PERF_TYPE_HARDWARE: u32 = 0;
    const PERF_TYPE_RAW: u32 = 4;
    const PERF_COUNT_HW_CPU_CYCLES: u64 = 0;
    const PERF_COUNT_HW_INSTRUCTIONS: u64 = 1;
    const PERF_COUNT_HW_BRANCH_MISSES: u64 = 5;
    const PERF_COUNT_HW_STALLED_CYCLES_FRONTEND: u64 = 7;
    const PERF_COUNT_HW_STALLED_CYCLES_BACKEND: u64 = 8;
    const ARMV8_PMUV3_L1D_CACHE_REFILL: u64 = 0x03;
    const ARMV8_PMUV3_L2D_CACHE_REFILL: u64 = 0x17;
    const PERF_FLAG_FD_CLOEXEC: c_ulong = 1 << 3;
    const PERF_FORMAT_TOTAL_TIME_ENABLED: u64 = 1 << 0;
    const PERF_FORMAT_TOTAL_TIME_RUNNING: u64 = 1 << 1;
    const PERF_EVENT_IOC_ENABLE: c_ulong = 0x2400;
    const PERF_EVENT_IOC_DISABLE: c_ulong = 0x2401;
    const PERF_EVENT_IOC_RESET: c_ulong = 0x2403;
    const PERF_IOC_FLAG_GROUP: c_ulong = 1;
    const SYS_PERF_EVENT_OPEN: c_long = 241;

    // Linux UAPI PERF_ATTR_SIZE_VER6 (120 bytes). The kernel reads `size` and
    // zero-fills any newer tail fields, so this layout stays valid on the
    // pinned Rasputin kernel without tracking later revisions.
    #[repr(C)]
    #[derive(Default)]
    struct PerfEventAttr {
        type_: u32,
        size: u32,
        config: u64,
        sample_period: u64,
        sample_type: u64,
        read_format: u64,
        flags: u64,
        wakeup_events: u32,
        bp_type: u32,
        config1: u64,
        config2: u64,
        branch_sample_type: u64,
        sample_regs_user: u64,
        sample_stack_user: u32,
        clockid: i32,
        sample_regs_intr: u64,
        aux_watermark: u32,
        sample_max_stack: u16,
        reserved2: u16,
        aux_sample_size: u32,
        reserved3: u32,
    }

    // ABI facts, not test expectations: the required Linux/aarch64 build lane
    // fails if this layout or the guest-only filter ever drifts.
    const PERF_ATTR_SIZE_VER6: usize = 120;
    const _: () = {
        assert!(size_of::<PerfEventAttr>() == PERF_ATTR_SIZE_VER6);
        assert!(
            super::ATTR_DISABLED | super::ATTR_PINNED | super::ATTR_EXCLUDE_HOST == 0x0008_0005
        );
        // `exclude_guest` is bit 20 and must stay clear, or the group would
        // count host execution instead of the guest cycles the proxy predicts.
        assert!(
            (super::ATTR_DISABLED | super::ATTR_PINNED | super::ATTR_EXCLUDE_HOST) & (1 << 20) == 0
        );
    };

    unsafe extern "C" {
        fn syscall(number: c_long, ...) -> c_long;
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
        fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
        fn close(fd: c_int) -> c_int;
        fn __errno_location() -> *mut c_int;
    }

    pub(crate) struct GuestPmu {
        fds: [RawFd; 7],
    }

    impl GuestPmu {
        pub(crate) fn open() -> Result<Self, VmmError> {
            // Storage order matches COUNTER_NAMES; the cycle event is opened
            // first as group leader and then placed in its canonical row.
            let leader = open_event(PERF_TYPE_HARDWARE, PERF_COUNT_HW_CPU_CYCLES, -1, true)?;
            let mut opened = vec![leader];
            let result = (|| {
                let branch = open_event(
                    PERF_TYPE_HARDWARE,
                    PERF_COUNT_HW_BRANCH_MISSES,
                    leader,
                    false,
                )?;
                opened.push(branch);
                let instructions = open_event(
                    PERF_TYPE_HARDWARE,
                    PERF_COUNT_HW_INSTRUCTIONS,
                    leader,
                    false,
                )?;
                opened.push(instructions);
                let l1d = open_event(PERF_TYPE_RAW, ARMV8_PMUV3_L1D_CACHE_REFILL, leader, false)?;
                opened.push(l1d);
                let l2d = open_event(PERF_TYPE_RAW, ARMV8_PMUV3_L2D_CACHE_REFILL, leader, false)?;
                opened.push(l2d);
                let backend = open_event(
                    PERF_TYPE_HARDWARE,
                    PERF_COUNT_HW_STALLED_CYCLES_BACKEND,
                    leader,
                    false,
                )?;
                opened.push(backend);
                let frontend = open_event(
                    PERF_TYPE_HARDWARE,
                    PERF_COUNT_HW_STALLED_CYCLES_FRONTEND,
                    leader,
                    false,
                )?;
                opened.push(frontend);
                Ok(Self {
                    fds: [branch, leader, instructions, l1d, l2d, backend, frontend],
                })
            })();
            if result.is_err() {
                for fd in opened {
                    unsafe {
                        let _ = close(fd);
                    }
                }
            }
            result
        }

        pub(crate) fn start(&self) -> Result<(), VmmError> {
            ioctl_group(self.fds[1], PERF_EVENT_IOC_RESET, "reset guest PMU group")?;
            ioctl_group(self.fds[1], PERF_EVENT_IOC_ENABLE, "enable guest PMU group")
        }

        pub(crate) fn stop_and_read(&self) -> Result<[u64; 7], VmmError> {
            ioctl_group(
                self.fds[1],
                PERF_EVENT_IOC_DISABLE,
                "disable guest PMU group",
            )?;
            let mut values = [0_u64; 7];
            for (index, fd) in self.fds.iter().copied().enumerate() {
                let mut row = [0_u64; 3];
                let got = unsafe {
                    read(
                        fd,
                        row.as_mut_ptr().cast::<c_void>(),
                        std::mem::size_of_val(&row),
                    )
                };
                if got != std::mem::size_of_val(&row) as isize {
                    return Err(error("read guest PMU counter"));
                }
                if row[1] == 0 || row[1] != row[2] {
                    return Err(VmmError::HostProfile(format!(
                        "guest PMU counter {} was multiplexed (enabled {}, running {})",
                        super::COUNTER_NAMES[index],
                        row[1],
                        row[2]
                    )));
                }
                values[index] = row[0];
            }
            Ok(values)
        }
    }

    impl Drop for GuestPmu {
        fn drop(&mut self) {
            for fd in self.fds {
                unsafe {
                    let _ = close(fd);
                }
            }
        }
    }

    fn open_event(type_: u32, config: u64, group: RawFd, pinned: bool) -> Result<RawFd, VmmError> {
        let attr = PerfEventAttr {
            type_,
            size: std::mem::size_of::<PerfEventAttr>() as u32,
            config,
            read_format: PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING,
            // disabled=1, exclude_host=1, exclude_guest=0. The group counts
            // only guest execution while KVM_RUN owns this host thread.
            flags: super::ATTR_DISABLED
                | (u64::from(pinned) * super::ATTR_PINNED)
                | super::ATTR_EXCLUDE_HOST,
            ..Default::default()
        };
        let fd = unsafe {
            syscall(
                SYS_PERF_EVENT_OPEN,
                &attr as *const PerfEventAttr,
                0 as c_int,
                -1 as c_int,
                group,
                PERF_FLAG_FD_CLOEXEC,
            )
        };
        if fd < 0 {
            Err(error("open guest-only PMU event"))
        } else {
            Ok(fd as RawFd)
        }
    }

    fn ioctl_group(fd: RawFd, request: c_ulong, operation: &'static str) -> Result<(), VmmError> {
        if unsafe { ioctl(fd, request, PERF_IOC_FLAG_GROUP) } == 0 {
            Ok(())
        } else {
            Err(error(operation))
        }
    }

    fn error(operation: &'static str) -> VmmError {
        let errno = unsafe { *__errno_location() };
        VmmError::HostProfile(format!("{operation}: errno {errno}"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn perf_attr_abi_is_the_declared_uapi_revision() {
            // The binding assertions are compile-time; this names the revision
            // the `size` field reports to the kernel.
            assert_eq!(size_of::<PerfEventAttr>(), PERF_ATTR_SIZE_VER6);
        }

        #[test]
        fn named_raw_events_match_armv8_pmuv3() {
            assert_eq!(ARMV8_PMUV3_L1D_CACHE_REFILL, 0x03);
            assert_eq!(ARMV8_PMUV3_L2D_CACHE_REFILL, 0x17);
            assert_eq!(super::super::COUNTER_NAMES.len(), 7);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_only_filter_uses_exclude_host_not_exclude_guest() {
        let flags = ATTR_DISABLED | ATTR_PINNED | ATTR_EXCLUDE_HOST;
        assert_eq!(flags, 0x0008_0005);
        assert_eq!(flags & (1 << 20), 0, "exclude_guest must remain clear");
        assert_eq!(COUNTER_NAMES.len(), 7);
    }
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub(crate) use platform::GuestPmu;

#[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
pub(crate) struct GuestPmu;

#[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
impl GuestPmu {
    pub(crate) fn open() -> Result<Self, VmmError> {
        Err(VmmError::Unsupported(
            "guest-only PMU measurement is available only on Linux/aarch64",
        ))
    }

    pub(crate) fn start(&self) -> Result<(), VmmError> {
        unreachable!("unsupported PMU cannot be constructed")
    }

    pub(crate) fn stop_and_read(&self) -> Result<[u64; 7], VmmError> {
        unreachable!("unsupported PMU cannot be constructed")
    }
}
