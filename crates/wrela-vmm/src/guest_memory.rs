//! Owned guest DRAM and the only checked GPA-to-host translation boundary.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::VmmError;

pub(crate) const COMMON_PROTECTION_GRANULE: usize = 16 * 1024;

pub(crate) struct GuestMemory {
    ptr: NonNull<u8>,
    len: usize,
    layout: Layout,
    locked: bool,
}

impl std::fmt::Debug for GuestMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestMemory")
            .field("len", &self.len)
            .field("alignment", &self.layout.align())
            .finish_non_exhaustive()
    }
}

impl GuestMemory {
    pub(crate) fn allocate(host_granule: usize, committed: bool) -> Result<Self, VmmError> {
        Self::allocate_len(
            wrela_machine::layout::DRAM_SIZE as usize,
            host_granule,
            committed,
        )
    }

    fn allocate_len(len: usize, host_granule: usize, committed: bool) -> Result<Self, VmmError> {
        if host_granule == 0 || !host_granule.is_power_of_two() {
            return Err(VmmError::BadImage(format!(
                "host mapping granule {host_granule} is not a nonzero power of two"
            )));
        }
        let alignment = host_granule
            .max(COMMON_PROTECTION_GRANULE)
            .max(wrela_machine::stage1::GRANULE as usize)
            .max(wrela_machine::layout::PIXELS_REGION_ALIGNMENT as usize);
        let rounded = len
            .checked_add(alignment - 1)
            .map(|value| value & !(alignment - 1))
            .ok_or_else(|| {
                VmmError::BadImage("guest DRAM allocation size overflows while aligning".into())
            })?;
        let layout = Layout::from_size_align(rounded, alignment)
            .map_err(|error| VmmError::BadImage(format!("bad guest DRAM layout: {error}")))?;
        let raw = unsafe { alloc_zeroed(layout) };
        let ptr = NonNull::new(raw).ok_or_else(|| {
            VmmError::BadImage("failed to allocate the guest DRAM reservation".into())
        })?;
        let mut memory = Self {
            ptr,
            len,
            layout,
            locked: false,
        };
        if committed {
            memory.commit(host_granule)?;
        }
        Ok(memory)
    }

    fn commit(&mut self, host_granule: usize) -> Result<(), VmmError> {
        for offset in (0..self.len).step_by(host_granule) {
            unsafe { std::ptr::write_volatile(self.ptr.as_ptr().add(offset), 0) };
        }
        if self.len != 0 {
            unsafe { std::ptr::write_volatile(self.ptr.as_ptr().add(self.len - 1), 0) };
        }
        unsafe extern "C" {
            fn mlock(address: *const std::ffi::c_void, length: usize) -> i32;
        }
        if unsafe { mlock(self.ptr.as_ptr().cast(), self.len) } != 0 {
            return Err(VmmError::HostProfile(format!(
                "prefaulted {} bytes but could not lock the complete guest DRAM reservation: {}",
                self.len,
                std::io::Error::last_os_error()
            )));
        }
        self.locked = true;
        Ok(())
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    fn alignment(&self) -> usize {
        self.layout.align()
    }

    pub(crate) fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    pub(crate) fn offset(&self, gpa: u64, len: usize, what: &str) -> Result<usize, VmmError> {
        let relative = gpa
            .checked_sub(wrela_machine::layout::DRAM_BASE)
            .ok_or_else(|| {
                VmmError::GuestFault(format!("{what} address {gpa:#x} is below guest DRAM"))
            })?;
        let offset = usize::try_from(relative)
            .map_err(|_| VmmError::GuestFault(format!("{what} offset does not fit usize")))?;
        offset
            .checked_add(len)
            .filter(|end| *end <= self.len)
            .map(|_| offset)
            .ok_or_else(|| {
                VmmError::GuestFault(format!(
                    "{what} range [{gpa:#x}, +{len}) is outside guest DRAM"
                ))
            })
    }

    pub(crate) fn initialize(
        &mut self,
        gpa: u64,
        bytes: &[u8],
        what: &str,
    ) -> Result<(), VmmError> {
        let offset = self.offset(gpa, bytes.len(), what)?;
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.ptr.as_ptr().add(offset),
                bytes.len(),
            )
        };
        Ok(())
    }

    #[cfg(test)]
    fn read_volatile(&self, gpa: u64, out: &mut [u8], what: &str) -> Result<(), VmmError> {
        let offset = self.offset(gpa, out.len(), what)?;
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = unsafe { std::ptr::read_volatile(self.ptr.as_ptr().add(offset + index)) };
        }
        Ok(())
    }
}

impl Drop for GuestMemory {
    fn drop(&mut self) {
        if self.locked {
            unsafe extern "C" {
                fn munlock(address: *const std::ffi::c_void, length: usize) -> i32;
            }
            let _ = unsafe { munlock(self.ptr.as_ptr().cast(), self.len) };
        }
        unsafe { dealloc(self.ptr.as_ptr(), self.layout) };
    }
}

#[derive(Debug)]
pub(crate) struct PublishedRegion {
    offset: usize,
    len: usize,
    what: String,
}

#[derive(Clone, Copy)]
pub(crate) struct GuestMemoryHandle {
    ptr: NonNull<u8>,
    len: usize,
}

impl GuestMemoryHandle {
    pub(crate) fn from_memory(memory: &GuestMemory) -> Self {
        Self {
            ptr: memory.ptr,
            len: memory.len,
        }
    }

    /// Rebuild the non-owning handle inside a scoped vCPU callback.
    ///
    /// # Safety
    /// `ptr..ptr+len` must be the live `GuestMemory` allocation and the
    /// callback must not outlive its owner.
    #[cfg(test)]
    pub(crate) unsafe fn from_raw(ptr: *mut u8, len: usize) -> Self {
        Self {
            ptr: NonNull::new(ptr).expect("live GuestMemory pointer is non-null"),
            len,
        }
    }

    /// Mint a one-use proof after a device model has validated the protocol
    /// transition that transfers this complete range to the host.
    ///
    /// # Safety
    /// The caller must be the device-model consume path immediately following
    /// its release-publish/acquire-consume boundary. The guest compiler's
    /// ownership rules must prevent writes until the host publishes completion.
    pub(crate) unsafe fn claim_published(
        self,
        gpa: u64,
        len: usize,
        what: &str,
    ) -> Result<PublishedRegion, VmmError> {
        let relative = gpa
            .checked_sub(wrela_machine::layout::DRAM_BASE)
            .ok_or_else(|| {
                VmmError::GuestFault(format!("{what} address {gpa:#x} is below guest DRAM"))
            })?;
        let offset = usize::try_from(relative)
            .map_err(|_| VmmError::GuestFault(format!("{what} offset exceeds usize")))?;
        offset
            .checked_add(len)
            .filter(|end| *end <= self.len)
            .ok_or_else(|| VmmError::GuestFault(format!("{what} range exceeds guest DRAM")))?;
        std::sync::atomic::fence(Ordering::Acquire);
        Ok(PublishedRegion {
            offset,
            len,
            what: what.to_string(),
        })
    }

    pub(crate) fn copy_published(self, region: PublishedRegion) -> Result<Vec<u8>, VmmError> {
        let end = region.offset.checked_add(region.len).ok_or_else(|| {
            VmmError::GuestFault(format!("{} claimed range overflows", region.what))
        })?;
        if end > self.len {
            return Err(VmmError::GuestFault(format!(
                "{} claimed range exceeds guest DRAM",
                region.what
            )));
        }
        let mut out = vec![0_u8; region.len];
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.ptr.as_ptr().add(region.offset),
                out.as_mut_ptr(),
                region.len,
            )
        };
        Ok(out)
    }

    pub(crate) fn copy_into_owned(
        self,
        gpa: u64,
        bytes: &[u8],
        what: &str,
    ) -> Result<(), VmmError> {
        let relative = gpa
            .checked_sub(wrela_machine::layout::DRAM_BASE)
            .ok_or_else(|| {
                VmmError::GuestFault(format!("{what} address {gpa:#x} is below guest DRAM"))
            })?;
        let offset = usize::try_from(relative)
            .map_err(|_| VmmError::GuestFault(format!("{what} offset exceeds usize")))?;
        offset
            .checked_add(bytes.len())
            .filter(|end| *end <= self.len)
            .ok_or_else(|| VmmError::GuestFault(format!("{what} range exceeds guest DRAM")))?;
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.ptr.as_ptr().add(offset),
                bytes.len(),
            )
        };
        std::sync::atomic::fence(Ordering::Release);
        Ok(())
    }

    pub(crate) fn fetch_or_u64(
        self,
        gpa: u64,
        value: u64,
        ordering: Ordering,
        what: &str,
    ) -> Result<u64, VmmError> {
        let ptr = self.atomic_ptr::<AtomicU64>(gpa, what)?;
        Ok(unsafe { (&*ptr).fetch_or(value, ordering) })
    }

    pub(crate) fn load_u64(
        self,
        gpa: u64,
        ordering: Ordering,
        what: &str,
    ) -> Result<u64, VmmError> {
        let ptr = self.atomic_ptr::<AtomicU64>(gpa, what)?;
        Ok(unsafe { (&*ptr).load(ordering) })
    }

    pub(crate) fn swap_u64(
        self,
        gpa: u64,
        value: u64,
        ordering: Ordering,
        what: &str,
    ) -> Result<u64, VmmError> {
        let ptr = self.atomic_ptr::<AtomicU64>(gpa, what)?;
        Ok(unsafe { (&*ptr).swap(value, ordering) })
    }

    pub(crate) fn fetch_or_u32(
        self,
        gpa: u64,
        value: u32,
        ordering: Ordering,
        what: &str,
    ) -> Result<u32, VmmError> {
        let ptr = self.atomic_ptr::<AtomicU32>(gpa, what)?;
        Ok(unsafe { (&*ptr).fetch_or(value, ordering) })
    }

    #[cfg(test)]
    pub(crate) fn load_u32(
        self,
        gpa: u64,
        ordering: Ordering,
        what: &str,
    ) -> Result<u32, VmmError> {
        let ptr = self.atomic_ptr::<AtomicU32>(gpa, what)?;
        Ok(unsafe { (&*ptr).load(ordering) })
    }

    #[cfg(test)]
    pub(crate) fn swap_u32(
        self,
        gpa: u64,
        value: u32,
        ordering: Ordering,
        what: &str,
    ) -> Result<u32, VmmError> {
        let ptr = self.atomic_ptr::<AtomicU32>(gpa, what)?;
        Ok(unsafe { (&*ptr).swap(value, ordering) })
    }

    fn atomic_ptr<T>(self, gpa: u64, what: &str) -> Result<*mut T, VmmError> {
        let relative = gpa
            .checked_sub(wrela_machine::layout::DRAM_BASE)
            .ok_or_else(|| {
                VmmError::GuestFault(format!("{what} address {gpa:#x} is below guest DRAM"))
            })?;
        let offset = usize::try_from(relative)
            .map_err(|_| VmmError::GuestFault(format!("{what} offset does not fit usize")))?;
        if gpa % align_of::<T>() as u64 != 0
            || offset
                .checked_add(size_of::<T>())
                .is_none_or(|end| end > self.len)
        {
            return Err(VmmError::GuestFault(format!(
                "{what} address {gpa:#x} is out of bounds or unaligned for atomic access"
            )));
        }
        Ok(unsafe { self.ptr.as_ptr().add(offset).cast() })
    }
}

// The allocation itself stays owned by `GuestMemory`; this non-owning handle
// is copied only into vCPU/device threads while scoped thread lifetimes prove
// the owner outlives them. Guest writes are accessed through volatile or
// aligned atomic operations, never through shared Rust references.
unsafe impl Send for GuestMemoryHandle {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_memory() -> GuestMemory {
        GuestMemory::allocate_len(64 * 1024, 4096, false).unwrap()
    }

    #[test]
    fn allocation_bounds_alignment_and_zeroing() {
        let memory = test_memory();
        assert!(memory.alignment() >= wrela_machine::layout::PIXELS_REGION_ALIGNMENT as usize);
        let base = wrela_machine::layout::DRAM_BASE;
        let mut bytes = [1_u8; 8];
        memory.read_volatile(base, &mut bytes, "test").unwrap();
        assert_eq!(bytes, [0; 8]);
        assert!(memory.offset(base - 1, 1, "below").is_err());
        assert!(memory.offset(base + 64 * 1024 - 1, 2, "past end").is_err());
    }

    #[test]
    fn atomic_raises_do_not_lose_bits() {
        let memory = test_memory();
        let handle = GuestMemoryHandle::from_memory(&memory);
        let word = wrela_machine::layout::DRAM_BASE + 8;
        std::thread::scope(|scope| {
            for bit in 0..32 {
                let handle = handle;
                scope.spawn(move || {
                    handle
                        .fetch_or_u64(word, 1 << bit, Ordering::Release, "pending")
                        .unwrap();
                });
            }
        });
        assert_eq!(
            handle.load_u64(word, Ordering::Acquire, "pending").unwrap(),
            u32::MAX as u64
        );
        assert_eq!(
            handle
                .swap_u64(word, 0, Ordering::Acquire, "pending")
                .unwrap(),
            u32::MAX as u64
        );
        assert_eq!(
            handle.load_u64(word, Ordering::Relaxed, "pending").unwrap(),
            0
        );
        assert!(
            handle
                .load_u64(word + 1, Ordering::Acquire, "unaligned")
                .is_err()
        );
    }

    #[test]
    fn u32_status_raise_racing_ack_keeps_a_complete_raise() {
        let memory = test_memory();
        let handle = GuestMemoryHandle::from_memory(&memory);
        let word = wrela_machine::layout::DRAM_BASE + 16;
        handle
            .fetch_or_u32(word, 1, Ordering::Release, "status")
            .unwrap();
        let other = handle;
        let raise = std::thread::spawn(move || {
            other
                .fetch_or_u32(word, 2, Ordering::Release, "status")
                .unwrap();
        });
        let claimed = handle
            .swap_u32(word, 0, Ordering::Acquire, "status")
            .unwrap();
        raise.join().unwrap();
        let after = handle.load_u32(word, Ordering::Acquire, "status").unwrap();
        assert_eq!((claimed | after) & 0b11, 0b11);
    }

    #[test]
    fn bulk_copy_requires_an_exact_consumed_token() {
        let mut memory = test_memory();
        let base = wrela_machine::layout::DRAM_BASE + 32;
        memory.initialize(base, b"frame", "frame init").unwrap();
        let handle = GuestMemoryHandle::from_memory(&memory);
        let token = unsafe { handle.claim_published(base, 5, "frame") }.unwrap();
        let out = handle.copy_published(token).unwrap();
        assert_eq!(&out, b"frame");
        assert!(
            unsafe {
                handle.claim_published(wrela_machine::layout::DRAM_BASE - 1, 5, "pre-publish")
            }
            .is_err()
        );
    }
}
