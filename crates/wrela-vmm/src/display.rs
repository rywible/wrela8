//! Machine-v1 display device and byte-preserving host presentation boundary.

use std::collections::BTreeMap;

pub mod drm;
pub mod headless;
pub mod metal;

use wrela_machine::pixels::{
    CONTROL_BYTES, DISPLAY_TILE_DESC_BYTES_V1, DisplayError, DisplayQueue, DisplayTileDescV1,
    PresentControl, PresentedFrame, TILE_ALLOCATION_BYTES,
};

use crate::guest_memory::GuestMemoryHandle;
use crate::{VmmError, guest_dram_offset};

pub use headless::{HeadlessBackend, HeadlessDisplay};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Headless,
    Metal,
    Drm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPresentError {
    pub backend: BackendKind,
    pub message: String,
    /// True only when the host accepted a display-changing commit but could
    /// not prove its completion. Such a failure is terminal for the VMM: it
    /// must never be reported to the guest as an ordinary rejected frame.
    pub commit_may_have_happened: bool,
}

impl std::fmt::Display for HostPresentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.backend, self.message)
    }
}

pub trait PresentationBackend: std::fmt::Debug {
    fn kind(&self) -> BackendKind;
    fn present(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError>;
    fn presented_digest(&self) -> Option<[u8; 32]>;
}

impl<T: PresentationBackend + ?Sized> PresentationBackend for Box<T> {
    fn kind(&self) -> BackendKind {
        (**self).kind()
    }

    fn present(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError> {
        (**self).present(frame)
    }

    fn presented_digest(&self) -> Option<[u8; 32]> {
        (**self).presented_digest()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayBackendSelection {
    #[default]
    Headless,
    Native,
}

fn runtime_backend(
    selection: DisplayBackendSelection,
) -> Result<Box<dyn PresentationBackend + Send>, VmmError> {
    let backend: Box<dyn PresentationBackend + Send> = match selection {
        DisplayBackendSelection::Headless => Box::new(HeadlessBackend::default()),
        DisplayBackendSelection::Native => {
            #[cfg(target_os = "macos")]
            {
                Box::new(metal::MetalDisplayBackend::native())
            }
            #[cfg(target_os = "linux")]
            {
                Box::new(drm::DrmDisplayBackend::native())
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                return Err(VmmError::GuestFault(
                    "native display backend is unavailable on this host".into(),
                ));
            }
        }
    };
    Ok(backend)
}

#[derive(Debug)]
pub struct RuntimeDisplay {
    selection: DisplayBackendSelection,
    spare_backend: Option<Box<dyn PresentationBackend + Send>>,
    devices: BTreeMap<u64, DisplayDevice<Box<dyn PresentationBackend + Send>>>,
    frames: Vec<PresentedFrame>,
    events: Vec<DisplayEvent>,
    backend_digests: Vec<[u8; 32]>,
    last_completion_status: u32,
}

pub fn runtime_display(selection: DisplayBackendSelection) -> Result<RuntimeDisplay, VmmError> {
    // Validate availability now; each sealed transport receives an independent
    // backend instance lazily on its first submission.
    let spare_backend = Some(runtime_backend(selection)?);
    Ok(RuntimeDisplay {
        selection,
        spare_backend,
        devices: BTreeMap::new(),
        frames: Vec::new(),
        events: Vec::new(),
        backend_digests: Vec::new(),
        last_completion_status: wrela_machine::pixels::COMPLETION_PENDING,
    })
}

impl RuntimeDisplay {
    pub fn frames(&self) -> &[PresentedFrame] {
        &self.frames
    }

    pub fn events(&self) -> &[DisplayEvent] {
        &self.events
    }

    pub fn backend_digests(&self) -> &[[u8; 32]] {
        &self.backend_digests
    }

    pub fn last_completion_status(&self) -> u32 {
        self.last_completion_status
    }

    /// Route one live submission to the backend owned by its sealed display
    /// transport.
    ///
    /// # Safety
    ///
    /// `host_ram` must point to `dram_len` mapped bytes for this call.
    #[cfg(test)]
    pub unsafe fn consume_volatile_from(
        &mut self,
        doorbell_addr: u64,
        host_ram: *const u8,
        dram_len: usize,
        control_addr: u64,
    ) -> Result<PresentedFrame, VmmError> {
        if !wrela_machine::pixels::is_display_doorbell_addr(doorbell_addr) {
            return Err(VmmError::GuestFault(format!(
                "display submission used unsupported doorbell {doorbell_addr:#x}"
            )));
        }
        if !self.devices.contains_key(&doorbell_addr) {
            let backend = match self.spare_backend.take() {
                Some(backend) => backend,
                None => runtime_backend(self.selection)?,
            };
            self.devices.insert(
                doorbell_addr,
                DisplayDevice::new(DisplayQueue::negotiate_first_mode(), backend),
            );
        }
        let device = self
            .devices
            .get_mut(&doorbell_addr)
            .expect("inserted display device exists");
        let frame_count = device.frames().len();
        let event_count = device.events().len();
        let digest_count = device.backend_digests().len();
        let result = unsafe {
            device.consume_volatile_from(doorbell_addr, host_ram, dram_len, control_addr)
        };
        self.last_completion_status = device.last_completion_status();
        self.frames
            .extend_from_slice(&device.frames()[frame_count..]);
        self.events
            .extend_from_slice(&device.events()[event_count..]);
        self.backend_digests
            .extend_from_slice(&device.backend_digests()[digest_count..]);
        result
    }

    pub(crate) fn consume_published_from(
        &mut self,
        doorbell_addr: u64,
        memory: GuestMemoryHandle,
        control_addr: u64,
    ) -> Result<PresentedFrame, VmmError> {
        if !wrela_machine::pixels::is_display_doorbell_addr(doorbell_addr) {
            return Err(VmmError::GuestFault(format!(
                "display submission used unsupported doorbell {doorbell_addr:#x}"
            )));
        }
        if !self.devices.contains_key(&doorbell_addr) {
            let backend = match self.spare_backend.take() {
                Some(backend) => backend,
                None => runtime_backend(self.selection)?,
            };
            self.devices.insert(
                doorbell_addr,
                DisplayDevice::new(DisplayQueue::negotiate_first_mode(), backend),
            );
        }
        let device = self
            .devices
            .get_mut(&doorbell_addr)
            .expect("display inserted");
        let frame_count = device.frames().len();
        let event_count = device.events().len();
        let digest_count = device.backend_digests().len();
        let result = device.consume_published_from(doorbell_addr, memory, control_addr);
        self.last_completion_status = device.last_completion_status();
        self.frames
            .extend_from_slice(&device.frames()[frame_count..]);
        self.events
            .extend_from_slice(&device.events()[event_count..]);
        self.backend_digests
            .extend_from_slice(&device.backend_digests()[digest_count..]);
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayEvent {
    Presented {
        renderer_index: u16,
        sequence: u64,
        generation: u8,
        visible_digest: [u8; 32],
        raw_tile_digest: [u8; 32],
    },
    Rejected {
        sequence: Option<u64>,
        error: String,
    },
}

#[derive(Debug)]
pub struct DisplayDevice<B> {
    initial_queue: DisplayQueue,
    queues: BTreeMap<u16, DisplayQueue>,
    doorbell_bindings: BTreeMap<u64, u16>,
    backend: B,
    frames: Vec<PresentedFrame>,
    events: Vec<DisplayEvent>,
    backend_digests: Vec<[u8; 32]>,
    last_completion_status: u32,
}

impl<B: PresentationBackend> DisplayDevice<B> {
    pub fn new(queue: DisplayQueue, backend: B) -> Self {
        Self {
            initial_queue: queue,
            queues: BTreeMap::new(),
            doorbell_bindings: BTreeMap::new(),
            backend,
            frames: Vec::new(),
            events: Vec::new(),
            backend_digests: Vec::new(),
            last_completion_status: wrela_machine::pixels::COMPLETION_PENDING,
        }
    }

    pub fn frames(&self) -> &[PresentedFrame] {
        &self.frames
    }

    pub fn events(&self) -> &[DisplayEvent] {
        &self.events
    }

    pub fn backend_digests(&self) -> &[[u8; 32]] {
        &self.backend_digests
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn last_completion_status(&self) -> u32 {
        self.last_completion_status
    }

    pub fn consume(&mut self, dram: &[u8], control_addr: u64) -> Result<PresentedFrame, VmmError> {
        self.consume_from(wrela_machine::pixels::DOORBELL_ADDR, dram, control_addr)
    }

    pub fn consume_from(
        &mut self,
        doorbell_addr: u64,
        dram: &[u8],
        control_addr: u64,
    ) -> Result<PresentedFrame, VmmError> {
        self.consume_with(doorbell_addr, control_addr, |addr, len, what| {
            let offset = display_dram_offset(addr, len as u64, what)?;
            let end = offset
                .checked_add(len)
                .filter(|end| *end <= dram.len())
                .ok_or_else(|| {
                    VmmError::GuestFault(format!(
                        "{what} address {addr:#x}+{len} exceeds the supplied DRAM snapshot"
                    ))
                })?;
            Ok(dram[offset..end].to_vec())
        })
    }

    /// Consume display-owned bytes using volatile reads from live guest DRAM.
    ///
    /// # Safety
    ///
    /// `host_ram` must point to `dram_len` mapped bytes for the duration of
    /// this call.  The guest release-publishes the complete chain before the
    /// synchronous doorbell.  Volatile byte reads avoid a Rust shared slice
    /// over RAM another guest vCPU could otherwise mutate.
    #[cfg(test)]
    pub unsafe fn consume_volatile(
        &mut self,
        host_ram: *const u8,
        dram_len: usize,
        control_addr: u64,
    ) -> Result<PresentedFrame, VmmError> {
        unsafe {
            self.consume_volatile_from(
                wrela_machine::pixels::DOORBELL_ADDR,
                host_ram,
                dram_len,
                control_addr,
            )
        }
    }

    /// Consume one submission from a specific sealed display doorbell.
    ///
    /// # Safety
    ///
    /// The safety contract is identical to [`Self::consume_volatile`].
    #[cfg(test)]
    pub unsafe fn consume_volatile_from(
        &mut self,
        doorbell_addr: u64,
        host_ram: *const u8,
        dram_len: usize,
        control_addr: u64,
    ) -> Result<PresentedFrame, VmmError> {
        self.consume_with(doorbell_addr, control_addr, |addr, len, what| {
            let offset = display_dram_offset(addr, len as u64, what)?;
            let end = offset
                .checked_add(len)
                .filter(|end| *end <= dram_len)
                .ok_or_else(|| {
                    VmmError::GuestFault(format!(
                        "{what} address {addr:#x}+{len} exceeds mapped guest DRAM"
                    ))
                })?;
            // Read in machine words rather than one byte at a time: a 1080p
            // frame is ~8 MB of tile payload per doorbell, and the byte loop
            // was the dominant cost of consuming one submission. The volatile
            // word reads keep the same guarantee — no Rust shared slice is ever
            // formed over guest RAM another vCPU could mutate.
            let mut bytes = Vec::with_capacity(len);
            let mut index = offset;
            while index + std::mem::size_of::<u64>() <= end {
                // SAFETY: the caller guarantees the mapping, and the checked
                // range covers these eight bytes. `read_unaligned` tolerates
                // any guest alignment; the read is volatile so it is never
                // cached, duplicated, or reordered by the optimizer.
                let word =
                    unsafe { std::ptr::read_volatile(host_ram.add(index).cast::<[u8; 8]>()) };
                bytes.extend_from_slice(&word);
                index += std::mem::size_of::<u64>();
            }
            for tail in index..end {
                // SAFETY: as above, for the sub-word tail.
                bytes.push(unsafe { std::ptr::read_volatile(host_ram.add(tail)) });
            }
            Ok(bytes)
        })
    }

    pub(crate) fn consume_published_from(
        &mut self,
        doorbell_addr: u64,
        memory: GuestMemoryHandle,
        control_addr: u64,
    ) -> Result<PresentedFrame, VmmError> {
        self.consume_with(doorbell_addr, control_addr, |addr, len, what| {
            // SAFETY: `consume_with` validates the doorbell/control sequence
            // that release-published these bytes before invoking this reader.
            let region = unsafe { memory.claim_published(addr, len, what) }.map_err(|error| {
                VmmError::GuestFault(format!("display claim rejected: {error}"))
            })?;
            memory
                .copy_published(region)
                .map_err(|error| VmmError::GuestFault(format!("display present rejected: {error}")))
        })
    }

    fn consume_with(
        &mut self,
        doorbell_addr: u64,
        control_addr: u64,
        mut read: impl FnMut(u64, usize, &str) -> Result<Vec<u8>, VmmError>,
    ) -> Result<PresentedFrame, VmmError> {
        self.last_completion_status = wrela_machine::pixels::COMPLETION_REJECTED;
        let result = self.prepare_frame(doorbell_addr, control_addr, &mut read);
        let (candidate_queue, frame) = match result {
            Ok(value) => value,
            Err(error) => {
                self.events.push(DisplayEvent::Rejected {
                    sequence: read_sequence_best_effort(control_addr, &mut read),
                    error: error.to_string(),
                });
                return Err(error);
            }
        };
        if let Some(path) = std::env::var_os("WRELA_P8_FRAME_DUMP") {
            std::fs::write(&path, &frame.bgra).map_err(|error| {
                VmmError::Io(format!(
                    "write requested P8 pre-presentation frame dump {}: {error}",
                    std::path::Path::new(&path).display()
                ))
            })?;
        }
        if let Some(path) = std::env::var_os("WRELA_P9_FRAME_SEQUENCE_DUMP") {
            use std::io::Write as _;
            let mut output = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|error| {
                    VmmError::Io(format!(
                        "open requested P9 frame-sequence dump {}: {error}",
                        std::path::Path::new(&path).display()
                    ))
                })?;
            output
                .write_all(b"WRELAP9F")
                .and_then(|()| output.write_all(&frame.sequence.to_le_bytes()))
                .and_then(|()| output.write_all(&(frame.bgra.len() as u64).to_le_bytes()))
                .and_then(|()| output.write_all(&frame.bgra))
                .map_err(|error| {
                    VmmError::Io(format!(
                        "append requested P9 frame-sequence dump {}: {error}",
                        std::path::Path::new(&path).display()
                    ))
                })?;
        }
        if let Err(error) = self.backend.present(&frame) {
            if error.commit_may_have_happened {
                return Err(VmmError::Io(format!(
                    "display completion became unknown after host commit: {error}"
                )));
            }
            self.events.push(DisplayEvent::Rejected {
                sequence: Some(frame.sequence),
                error: error.to_string(),
            });
            return Err(VmmError::GuestFault(format!(
                "display present rejected: {error}"
            )));
        }
        let backend_digest = self.backend.presented_digest().ok_or_else(|| {
            VmmError::Io("display backend omitted its presented-byte digest".into())
        })?;
        if backend_digest != frame.visible_digest {
            return Err(VmmError::Io(format!(
                "display backend committed bytes with digest {backend_digest:x?}, expected {:x?}",
                frame.visible_digest,
            )));
        }
        self.queues.insert(frame.renderer_index, candidate_queue);
        self.doorbell_bindings
            .insert(doorbell_addr, frame.renderer_index);
        self.backend_digests.push(backend_digest);
        self.events.push(DisplayEvent::Presented {
            renderer_index: frame.renderer_index,
            sequence: frame.sequence,
            generation: frame.generation,
            visible_digest: frame.visible_digest,
            raw_tile_digest: frame.raw_tile_digest,
        });
        // Replay owns the digest record, not an extra copy of potentially
        // multi-megabyte visible bytes.
        let mut recorded = frame.clone();
        recorded.bgra.clear();
        self.frames.push(recorded);
        self.last_completion_status = wrela_machine::pixels::COMPLETION_PRESENTED;
        Ok(frame)
    }

    fn prepare_frame(
        &self,
        doorbell_addr: u64,
        control_addr: u64,
        read: &mut impl FnMut(u64, usize, &str) -> Result<Vec<u8>, VmmError>,
    ) -> Result<(DisplayQueue, PresentedFrame), VmmError> {
        let control_bytes = read(control_addr, CONTROL_BYTES, "display control")?;
        let control = PresentControl::decode(&control_bytes).map_err(display_error)?;
        if self
            .doorbell_bindings
            .get(&doorbell_addr)
            .is_some_and(|renderer| *renderer != control.renderer_index)
            || self.doorbell_bindings.iter().any(|(address, renderer)| {
                *renderer == control.renderer_index && *address != doorbell_addr
            })
        {
            return Err(VmmError::GuestFault(format!(
                "display present rejected: doorbell {doorbell_addr:#x} is not bound to renderer {}",
                control.renderer_index
            )));
        }
        let queue = self
            .queues
            .get(&control.renderer_index)
            .unwrap_or(&self.initial_queue);
        queue.validate_control(control).map_err(display_error)?;
        let descriptor_bytes = usize::try_from(control.tile_count)
            .ok()
            .and_then(|count| count.checked_mul(DISPLAY_TILE_DESC_BYTES_V1))
            .ok_or_else(|| VmmError::GuestFault("display tile-list byte count overflow".into()))?;
        let tile_records = read(control.tiles_addr, descriptor_bytes, "display tile list")?;
        let mut tiles = Vec::with_capacity(control.tile_count as usize);
        for bytes in tile_records.chunks_exact(DISPLAY_TILE_DESC_BYTES_V1) {
            tiles.push(DisplayTileDescV1::decode(bytes).map_err(display_error)?);
        }
        // Descriptor geometry/address validation happens inside `present`
        // before this callback is invoked, so malformed lists cannot cause a
        // pixel read at an attacker-selected address.
        let mut candidate = queue.clone();
        let frame = candidate
            .present(control, &tiles, |tile| {
                read(
                    tile.guest_addr,
                    TILE_ALLOCATION_BYTES,
                    "display tile pixels",
                )
                .ok()
            })
            .map_err(display_error)?;
        Ok((candidate, frame))
    }
}

/// Publish the device's terminal completion status into the guest control
/// block before the synchronous doorbell returns.
///
/// The guest coordinator branches on exactly this word: `COMPLETION_PRESENTED`
/// swaps the back generation to front and advances the frame sequence, while
/// any other terminal code must reclaim the back generation and surface
/// `RenderError::Display`. Both host backends publish it through this one
/// function so a rejected frame cannot recover on one host and wedge on the
/// other.
///
/// # Safety
///
/// `dram` must point to the live guest DRAM mapping for the duration of the
/// call. The address is range-checked against the machine's DRAM window before
/// any write, so a malformed control address fails closed instead of writing
/// outside the mapping.
pub(crate) unsafe fn publish_completion_status(
    dram: *mut u8,
    control_addr: u64,
    status: u32,
) -> Result<(), VmmError> {
    let status_addr = control_addr
        .checked_add(wrela_machine::pixels::COMPLETION_STATUS_OFFSET)
        .ok_or_else(|| {
            VmmError::GuestFault("display completion status address overflow".to_string())
        })?;
    let offset = guest_dram_offset(status_addr, 4, "display completion status")?;
    unsafe {
        std::sync::atomic::AtomicU32::from_ptr(dram.add(offset).cast())
            .store(status, std::sync::atomic::Ordering::Release)
    };
    Ok(())
}

fn read_sequence_best_effort(
    control_addr: u64,
    read: &mut impl FnMut(u64, usize, &str) -> Result<Vec<u8>, VmmError>,
) -> Option<u64> {
    read(control_addr, CONTROL_BYTES, "display rejected control")
        .ok()
        .and_then(|bytes| PresentControl::decode(&bytes).ok())
        .map(|control| control.sequence)
}

fn display_dram_offset(guest: u64, nbytes: u64, what: &str) -> Result<usize, VmmError> {
    guest_dram_offset(guest, nbytes, what)
        .map_err(|error| VmmError::GuestFault(format!("display present rejected: {error}")))
}

fn display_error(error: DisplayError) -> VmmError {
    VmmError::GuestFault(format!("display present rejected: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wrela_machine::{layout, pixels};

    fn seal_guest_digests(dram: &mut [u8], control_addr: u64) {
        let at = |addr: u64| (addr - layout::DRAM_BASE) as usize;
        let mut control = PresentControl::decode(
            &dram[at(control_addr)..at(control_addr) + pixels::CONTROL_BYTES],
        )
        .unwrap();
        let mut visible = vec![0_u8; control.mode.visible_bytes().unwrap()];
        let mut raw = Vec::new();
        let mut descriptor_bytes = Vec::new();
        for ordinal in 0..control.tile_count as usize {
            let descriptor_at =
                at(control.tiles_addr) + ordinal * pixels::DISPLAY_TILE_DESC_BYTES_V1;
            let encoded = &dram[descriptor_at..descriptor_at + pixels::DISPLAY_TILE_DESC_BYTES_V1];
            let descriptor = DisplayTileDescV1::decode(encoded).unwrap();
            descriptor_bytes.extend_from_slice(encoded);
            let payload = &dram[at(descriptor.guest_addr)
                ..at(descriptor.guest_addr) + pixels::TILE_ALLOCATION_BYTES];
            raw.extend_from_slice(payload);
            for row in 0..u32::from(descriptor.height) {
                let source = row as usize * pixels::TILE_STRIDE_BYTES as usize;
                let target = ((u32::from(descriptor.y) + row) * control.mode.width
                    + u32::from(descriptor.x)) as usize
                    * pixels::BYTES_PER_PIXEL as usize;
                let count = descriptor.width as usize * pixels::BYTES_PER_PIXEL as usize;
                visible[target..target + count].copy_from_slice(&payload[source..source + count]);
            }
        }
        control.guest_visible_digest = pixels::guest_bounded_digest(&visible);
        control.guest_raw_tile_digest = pixels::guest_bounded_digest(&raw);
        control.guest_tile_descriptor_digest = pixels::guest_bounded_digest(&descriptor_bytes);
        dram[at(control_addr)..at(control_addr) + pixels::CONTROL_BYTES]
            .copy_from_slice(&control.encode());
    }

    fn valid_dram() -> Vec<u8> {
        let at = |addr: u64| (addr - layout::DRAM_BASE) as usize;
        let end = at(pixels::FRAMEBUFFER_BASE) + pixels::TILE_ALLOCATION_BYTES;
        let mut dram = vec![0_u8; end];
        let control = PresentControl {
            abi_version: pixels::ABI_VERSION,
            format: pixels::FORMAT_BGRA8_SRGB,
            generation: 1,
            renderer_index: 0,
            mode: pixels::DisplayModeV1::SEED,
            tile_count: 1,
            sequence: 0,
            tiles_addr: pixels::TILES_BASE,
            vsync_id: 0,
            checkpoint: 0,
            guest_visible_digest: [0; 4],
            guest_raw_tile_digest: [0; 4],
            guest_tile_descriptor_digest: [0; 4],
            completion_status: pixels::COMPLETION_PENDING,
        };
        dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES]
            .copy_from_slice(&control.encode());
        let tile = DisplayTileDescV1 {
            guest_addr: pixels::FRAMEBUFFER_BASE,
            x: 0,
            y: 0,
            width: pixels::WIDTH as u16,
            height: pixels::HEIGHT as u16,
            stride_bytes: pixels::TILE_STRIDE_BYTES as u16,
            format: pixels::FORMAT_BGRA8_SRGB,
            reserved: [0; 5],
        };
        dram[at(pixels::TILES_BASE)..at(pixels::TILES_BASE) + pixels::DISPLAY_TILE_DESC_BYTES_V1]
            .copy_from_slice(&tile.encode());
        for pixel in dram[at(pixels::FRAMEBUFFER_BASE)..end].chunks_exact_mut(4) {
            pixel.copy_from_slice(&[1, 2, 3, 255]);
        }
        seal_guest_digests(&mut dram, pixels::CONTROL_BASE);
        dram
    }

    fn partial_mode_dram() -> Vec<u8> {
        let mode = pixels::DisplayModeV1 {
            width: 65,
            height: 33,
            refresh_hz: 75,
        };
        let at = |addr: u64| (addr - layout::DRAM_BASE) as usize;
        let tile_count = mode.tile_count().unwrap();
        let end =
            at(pixels::FRAMEBUFFER_BASE) + tile_count as usize * pixels::TILE_ALLOCATION_BYTES;
        let mut dram = vec![0_u8; end];
        let control = PresentControl {
            abi_version: pixels::ABI_VERSION,
            format: pixels::FORMAT_BGRA8_SRGB,
            generation: 1,
            renderer_index: 0,
            mode,
            tile_count,
            sequence: 0,
            tiles_addr: pixels::TILES_BASE,
            vsync_id: 3,
            checkpoint: 5,
            guest_visible_digest: [0; 4],
            guest_raw_tile_digest: [0; 4],
            guest_tile_descriptor_digest: [0; 4],
            completion_status: pixels::COMPLETION_PENDING,
        };
        dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES]
            .copy_from_slice(&control.encode());
        let mut ordinal = 0_u32;
        for tile_y in 0..mode.height.div_ceil(pixels::TILE_HEIGHT) {
            for tile_x in 0..mode.width.div_ceil(pixels::TILE_WIDTH) {
                let x = tile_x * pixels::TILE_WIDTH;
                let y = tile_y * pixels::TILE_HEIGHT;
                let descriptor = DisplayTileDescV1 {
                    guest_addr: pixels::FRAMEBUFFER_BASE
                        + u64::from(ordinal) * pixels::TILE_ALLOCATION_BYTES as u64,
                    x: x as u16,
                    y: y as u16,
                    width: (mode.width - x).min(pixels::TILE_WIDTH) as u16,
                    height: (mode.height - y).min(pixels::TILE_HEIGHT) as u16,
                    stride_bytes: pixels::TILE_STRIDE_BYTES as u16,
                    format: pixels::FORMAT_BGRA8_SRGB,
                    reserved: [0; 5],
                };
                let descriptor_at =
                    at(pixels::TILES_BASE) + ordinal as usize * pixels::DISPLAY_TILE_DESC_BYTES_V1;
                dram[descriptor_at..descriptor_at + pixels::DISPLAY_TILE_DESC_BYTES_V1]
                    .copy_from_slice(&descriptor.encode());
                let payload_at = at(descriptor.guest_addr);
                for row in 0..usize::from(descriptor.height) {
                    for column in 0..usize::from(descriptor.width) {
                        let pixel =
                            payload_at + row * pixels::TILE_STRIDE_BYTES as usize + column * 4;
                        dram[pixel..pixel + 4].copy_from_slice(&[
                            ordinal as u8,
                            row as u8,
                            column as u8,
                            255,
                        ]);
                    }
                }
                ordinal += 1;
            }
        }
        seal_guest_digests(&mut dram, pixels::CONTROL_BASE);
        dram
    }

    #[test]
    fn consumes_guest_owned_bytes_without_modification_and_one_event() {
        let dram = valid_dram();
        let before = dram.clone();
        let mut sink = HeadlessDisplay::default();
        let frame = sink
            .consume(&dram, wrela_machine::pixels::CONTROL_BASE)
            .unwrap();
        assert_eq!(&frame.bgra[..4], &[1, 2, 3, 255]);
        assert!(sink.frames()[0].bgra.is_empty());
        assert_eq!(sink.events().len(), 1);
        assert_eq!(dram, before);
    }

    #[test]
    fn guest_vmm_path_negotiates_partial_multitile_mode() {
        let dram = partial_mode_dram();
        let mut sink = HeadlessDisplay::default();
        let frame = sink.consume(&dram, pixels::CONTROL_BASE).unwrap();
        assert_eq!(frame.mode.width, 65);
        assert_eq!(frame.mode.height, 33);
        assert_eq!(frame.mode.refresh_hz, 75);
        assert_eq!(frame.bgra.len(), 65 * 33 * 4);
        assert_eq!(&frame.bgra[..4], &[0, 0, 0, 255]);
        let last = (65 * 33 - 1) * 4;
        assert_eq!(&frame.bgra[last..last + 4], &[3, 0, 0, 255]);
        assert_eq!(sink.events().len(), 1);
    }

    #[test]
    fn interleaved_renderers_keep_independent_sequence_and_generation_state() {
        let mut dram = valid_dram();
        let at = |addr: u64| (addr - layout::DRAM_BASE) as usize;
        let mut sink = HeadlessDisplay::default();

        let first = sink
            .consume_from(pixels::DOORBELL_ADDR, &dram, pixels::CONTROL_BASE)
            .unwrap();
        assert_eq!(
            (first.renderer_index, first.sequence, first.generation),
            (0, 0, 1)
        );

        let mut control = PresentControl::decode(
            &dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES],
        )
        .unwrap();
        control.renderer_index = 1;
        dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES]
            .copy_from_slice(&control.encode());
        seal_guest_digests(&mut dram, pixels::CONTROL_BASE);
        let second = sink
            .consume_from(pixels::DOORBELL_ADDR + 0x1000, &dram, pixels::CONTROL_BASE)
            .unwrap();
        assert_eq!(
            (second.renderer_index, second.sequence, second.generation),
            (1, 0, 1)
        );

        control.renderer_index = 0;
        control.sequence = 1;
        control.generation = 0;
        dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES]
            .copy_from_slice(&control.encode());
        seal_guest_digests(&mut dram, pixels::CONTROL_BASE);
        let third = sink
            .consume_from(pixels::DOORBELL_ADDR, &dram, pixels::CONTROL_BASE)
            .unwrap();
        assert_eq!(
            (third.renderer_index, third.sequence, third.generation),
            (0, 1, 0)
        );

        control.renderer_index = 1;
        control.sequence = 1;
        control.generation = 0;
        dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES]
            .copy_from_slice(&control.encode());
        seal_guest_digests(&mut dram, pixels::CONTROL_BASE);
        let error = sink
            .consume_from(pixels::DOORBELL_ADDR, &dram, pixels::CONTROL_BASE)
            .expect_err("a renderer cannot switch to another display's doorbell");
        assert!(error.to_string().contains("is not bound to renderer 1"));
    }

    #[test]
    fn runtime_router_allocates_one_queue_and_backend_per_display_doorbell() {
        let mut dram = valid_dram();
        let at = |addr: u64| (addr - layout::DRAM_BASE) as usize;
        let mut runtime = runtime_display(DisplayBackendSelection::Headless).unwrap();
        let first = unsafe {
            runtime.consume_volatile_from(
                pixels::DOORBELL_ADDR,
                dram.as_ptr(),
                dram.len(),
                pixels::CONTROL_BASE,
            )
        }
        .unwrap();
        assert_eq!(first.renderer_index, 0);

        let mut control = PresentControl::decode(
            &dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES],
        )
        .unwrap();
        control.renderer_index = 1;
        dram[at(pixels::CONTROL_BASE)..at(pixels::CONTROL_BASE) + pixels::CONTROL_BYTES]
            .copy_from_slice(&control.encode());
        seal_guest_digests(&mut dram, pixels::CONTROL_BASE);
        let second = unsafe {
            runtime.consume_volatile_from(
                pixels::DOORBELL_ADDR + 0x1000,
                dram.as_ptr(),
                dram.len(),
                pixels::CONTROL_BASE,
            )
        }
        .unwrap();
        assert_eq!(second.renderer_index, 1);
        assert_eq!(runtime.devices.len(), 2);
        assert_eq!(runtime.frames().len(), 2);
        assert_eq!(runtime.backend_digests().len(), 2);
    }

    #[test]
    fn hostile_control_is_recorded_and_never_reads_tile_payload() {
        let mut dram = valid_dram();
        let at = |addr: u64| (addr - layout::DRAM_BASE) as usize;
        dram[at(pixels::CONTROL_BASE) + 20..at(pixels::CONTROL_BASE) + 24]
            .copy_from_slice(&u32::MAX.to_le_bytes());
        let mut reads = Vec::new();
        let mut sink = HeadlessDisplay::default();
        let error = sink
            .consume_with(
                pixels::DOORBELL_ADDR,
                pixels::CONTROL_BASE,
                |addr, len, _| {
                    reads.push((addr, len));
                    let start = at(addr);
                    dram.get(start..start + len)
                        .map(<[u8]>::to_vec)
                        .ok_or_else(|| VmmError::GuestFault("range".into()))
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("TileCount"));
        assert!(
            reads
                .iter()
                .all(|(address, _)| *address == pixels::CONTROL_BASE)
        );
        assert!(matches!(sink.events(), [DisplayEvent::Rejected { .. }]));
    }

    #[derive(Debug, Default)]
    struct FailOnceBackend {
        failed: bool,
        digest: Option<[u8; 32]>,
    }

    impl PresentationBackend for FailOnceBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Headless
        }

        fn present(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError> {
            if !self.failed {
                self.failed = true;
                return Err(HostPresentError {
                    backend: self.kind(),
                    message: "injected present failure".into(),
                    commit_may_have_happened: false,
                });
            }
            self.digest = Some(wrela_machine::sha256::sha256(&frame.bgra));
            Ok(())
        }

        fn presented_digest(&self) -> Option<[u8; 32]> {
            self.digest
        }
    }

    #[test]
    fn backend_failure_retains_front_sequence_and_next_submission_recovers() {
        let dram = valid_dram();
        let mut device = DisplayDevice::new(
            DisplayQueue::new(pixels::DisplayModeV1::SEED).unwrap(),
            FailOnceBackend::default(),
        );
        assert!(device.consume(&dram, pixels::CONTROL_BASE).is_err());
        assert_eq!(device.last_completion_status(), pixels::COMPLETION_REJECTED);
        assert!(device.frames().is_empty());
        assert!(matches!(device.events(), [DisplayEvent::Rejected { .. }]));

        let recovered = device.consume(&dram, pixels::CONTROL_BASE).unwrap();
        assert_eq!(
            recovered.sequence, 0,
            "failed present must not advance sequence"
        );
        assert_eq!(
            device.last_completion_status(),
            pixels::COMPLETION_PRESENTED
        );
        assert_eq!(device.frames().len(), 1);
        assert!(matches!(device.events()[1], DisplayEvent::Presented { .. }));
    }

    #[test]
    fn a_rejected_frame_publishes_recoverable_status_and_keeps_the_front_generation() {
        // The guest coordinator can only reclaim its back generation and raise
        // `RenderError::Display` if the device leaves a terminal `rejected`
        // code in the control block, keeps the frame sequence where it was, and
        // still accepts the next well-formed submission. Nothing else in the
        // suite drives that whole sequence, so a device that forgot to publish
        // the status — or advanced the sequence anyway — would wedge the guest
        // on the next frame with every other test still green.
        let mut dram = valid_dram();
        let at = |addr: u64| (addr - layout::DRAM_BASE) as usize;
        let mut device = DisplayDevice::new(
            DisplayQueue::new(pixels::DisplayModeV1::SEED).unwrap(),
            FailOnceBackend::default(),
        );

        assert!(device.consume(&dram, pixels::CONTROL_BASE).is_err());
        let status = device.last_completion_status();
        assert_eq!(status, pixels::COMPLETION_REJECTED);
        // SAFETY: `dram` is a live, sufficiently large host buffer standing in
        // for the guest mapping.
        unsafe {
            publish_completion_status(dram.as_mut_ptr(), pixels::CONTROL_BASE, status).unwrap();
        }
        let published = at(pixels::CONTROL_BASE) + pixels::COMPLETION_STATUS_OFFSET as usize;
        assert_eq!(
            u32::from_le_bytes(dram[published..published + 4].try_into().unwrap()),
            pixels::COMPLETION_REJECTED,
            "the guest must observe a terminal rejected code, not the pending one"
        );

        // Re-arming the status word is the guest's job: generated `p8_present`
        // rewrites the control block (status back to pending) for every
        // attempt, and the device refuses to reuse a control block still
        // carrying a terminal code.
        assert!(
            matches!(
                device.consume(&dram, pixels::CONTROL_BASE),
                Err(VmmError::GuestFault(ref message)) if message.contains("ReservedNonzero")
            ),
            "a control block still holding a terminal status must be refused"
        );
        dram[published..published + 4].copy_from_slice(&pixels::COMPLETION_PENDING.to_le_bytes());

        let recovered = device
            .consume(&dram, pixels::CONTROL_BASE)
            .expect("the next submission recovers after a rejection");
        assert_eq!(
            recovered.sequence, 0,
            "a rejected frame must not consume a frame sequence number"
        );
        assert_eq!(recovered.generation, 1);
        let status = device.last_completion_status();
        assert_eq!(status, pixels::COMPLETION_PRESENTED);
        // SAFETY: as above.
        unsafe {
            publish_completion_status(dram.as_mut_ptr(), pixels::CONTROL_BASE, status).unwrap();
        }
        assert_eq!(
            u32::from_le_bytes(dram[published..published + 4].try_into().unwrap()),
            pixels::COMPLETION_PRESENTED,
        );
        // Both refusals are recorded as diagnostics and only the accepted frame
        // becomes an output event, so replay never sees a rejected submission
        // as a displayed frame.
        assert!(matches!(
            device.events(),
            [
                DisplayEvent::Rejected { .. },
                DisplayEvent::Rejected { .. },
                DisplayEvent::Presented { .. }
            ]
        ));
        assert_eq!(device.frames().len(), 1);
    }

    #[test]
    fn completion_status_publication_fails_closed_outside_guest_dram() {
        let mut dram = valid_dram();
        for control in [u64::MAX - 8, layout::DRAM_BASE - 4096, 0] {
            // SAFETY: every address here is rejected before any write, which is
            // exactly what the assertion checks.
            let result = unsafe {
                publish_completion_status(dram.as_mut_ptr(), control, pixels::COMPLETION_PENDING)
            };
            assert!(
                result.is_err(),
                "control address {control:#x} must not reach a host write"
            );
        }
    }

    #[derive(Debug)]
    struct CommitBecameUnknown;

    impl PresentationBackend for CommitBecameUnknown {
        fn kind(&self) -> BackendKind {
            BackendKind::Drm
        }

        fn present(&mut self, _frame: &PresentedFrame) -> Result<(), HostPresentError> {
            Err(HostPresentError {
                backend: self.kind(),
                message: "injected post-commit completion loss".into(),
                commit_may_have_happened: true,
            })
        }

        fn presented_digest(&self) -> Option<[u8; 32]> {
            None
        }
    }

    #[test]
    fn post_commit_failure_is_terminal_and_never_published_as_rejected() {
        let dram = valid_dram();
        let mut device = DisplayDevice::new(
            DisplayQueue::new(pixels::DisplayModeV1::SEED).unwrap(),
            CommitBecameUnknown,
        );
        let error = device
            .consume(&dram, pixels::CONTROL_BASE)
            .expect_err("post-commit uncertainty is terminal");
        assert!(matches!(error, VmmError::Io(_)));
        assert!(device.frames().is_empty());
        assert!(device.events().is_empty());
    }
}
