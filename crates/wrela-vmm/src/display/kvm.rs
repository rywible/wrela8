//! Linux/KVM DRM-dumb-buffer compatible BGRA scanout gather.

use wrela_machine::pixels::PresentedFrame;

use super::{BackendKind, HostPresentError, PresentationBackend};

#[derive(Debug, Default)]
pub struct KvmDisplayBackend {
    drm_dumb_buffer: Vec<u8>,
    last_vsync: Option<u64>,
    last_presented_digest: Option<[u8; 32]>,
    #[cfg(target_os = "linux")]
    native_requested: bool,
    #[cfg(target_os = "linux")]
    native: Option<native::DrmSurface>,
}

impl KvmDisplayBackend {
    pub fn buffer_bytes(&self) -> &[u8] {
        &self.drm_dumb_buffer
    }

    pub fn last_vsync(&self) -> Option<u64> {
        self.last_vsync
    }

    #[cfg(target_os = "linux")]
    pub fn native() -> Self {
        Self {
            drm_dumb_buffer: Vec::new(),
            last_vsync: None,
            last_presented_digest: None,
            native_requested: true,
            native: None,
        }
    }
}

impl PresentationBackend for KvmDisplayBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::LinuxKvm
    }

    fn present(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError> {
        if wrela_machine::sha256::sha256(&frame.bgra) != frame.visible_digest {
            return Err(HostPresentError {
                backend: self.kind(),
                message: "DRM dumb-buffer gather digest differs before present".into(),
                commit_may_have_happened: false,
            });
        }
        self.drm_dumb_buffer.clear();
        self.drm_dumb_buffer.extend_from_slice(&frame.bgra);
        #[cfg(target_os = "linux")]
        if self.native_requested {
            if self.native.is_none() {
                self.native = Some(native::DrmSurface::new(frame)?);
            }
            let native = self.native.as_mut().expect("initialized above");
            native.present(frame)?;
            self.last_presented_digest = Some(native.front_digest(frame)?);
        } else {
            self.last_presented_digest = Some(wrela_machine::sha256::sha256(&self.drm_dumb_buffer));
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.last_presented_digest = Some(wrela_machine::sha256::sha256(&self.drm_dumb_buffer));
        }
        self.last_vsync = Some(frame.vsync_id);
        Ok(())
    }

    fn presented_digest(&self) -> Option<[u8; 32]> {
        self.last_presented_digest
    }
}

#[cfg(target_os = "linux")]
mod native {
    use std::ffi::{c_int, c_uint, c_ulong, c_void};
    use std::fs::{File, OpenOptions};
    use std::os::fd::AsRawFd;
    use std::ptr::NonNull;

    use wrela_machine::pixels::PresentedFrame;

    use super::{BackendKind, HostPresentError};

    const DRM_MODE_CONNECTED: u32 = 1;
    const DRM_MODE_PAGE_FLIP_EVENT: u32 = 1;
    const DRM_EVENT_CONTEXT_VERSION: c_int = 2;
    const PROT_READ: c_int = 1;
    const PROT_WRITE: c_int = 2;
    const MAP_SHARED: c_int = 1;
    const POLLIN: i16 = 1;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct DrmModeModeInfo {
        clock: u32,
        hdisplay: u16,
        hsync_start: u16,
        hsync_end: u16,
        htotal: u16,
        hskew: u16,
        vdisplay: u16,
        vsync_start: u16,
        vsync_end: u16,
        vtotal: u16,
        vscan: u16,
        vrefresh: u32,
        flags: u32,
        type_: u32,
        name: [u8; 32],
    }

    #[repr(C)]
    struct DrmModeRes {
        count_fbs: c_int,
        fbs: *mut u32,
        count_crtcs: c_int,
        crtcs: *mut u32,
        count_connectors: c_int,
        connectors: *mut u32,
        count_encoders: c_int,
        encoders: *mut u32,
        min_width: u32,
        max_width: u32,
        min_height: u32,
        max_height: u32,
    }

    #[repr(C)]
    struct DrmModeConnector {
        connector_id: u32,
        encoder_id: u32,
        connector_type: u32,
        connector_type_id: u32,
        connection: u32,
        mm_width: u32,
        mm_height: u32,
        subpixel: u32,
        count_modes: c_int,
        modes: *mut DrmModeModeInfo,
        count_props: c_int,
        props: *mut u32,
        prop_values: *mut u64,
        count_encoders: c_int,
        encoders: *mut u32,
    }

    #[repr(C)]
    struct DrmModeEncoder {
        encoder_id: u32,
        encoder_type: u32,
        crtc_id: u32,
        possible_crtcs: u32,
        possible_clones: u32,
    }

    #[repr(C)]
    struct DrmModeCrtc {
        crtc_id: u32,
        buffer_id: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        mode_valid: c_int,
        mode: DrmModeModeInfo,
        gamma_size: c_int,
    }

    #[repr(C)]
    #[derive(Default)]
    struct DrmModeCreateDumb {
        height: u32,
        width: u32,
        bpp: u32,
        flags: u32,
        handle: u32,
        pitch: u32,
        size: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct DrmModeMapDumb {
        handle: u32,
        pad: u32,
        offset: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct DrmModeDestroyDumb {
        handle: u32,
    }

    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: i16,
        revents: i16,
    }

    #[repr(C)]
    struct DrmEventContext {
        version: c_int,
        vblank_handler: Option<unsafe extern "C" fn(c_int, c_uint, c_uint, c_uint, *mut c_void)>,
        page_flip_handler: Option<unsafe extern "C" fn(c_int, c_uint, c_uint, c_uint, *mut c_void)>,
    }

    #[link(name = "drm")]
    unsafe extern "C" {
        fn drmModeGetResources(fd: c_int) -> *mut DrmModeRes;
        fn drmModeFreeResources(resources: *mut DrmModeRes);
        fn drmModeGetConnector(fd: c_int, id: u32) -> *mut DrmModeConnector;
        fn drmModeFreeConnector(connector: *mut DrmModeConnector);
        fn drmModeGetEncoder(fd: c_int, id: u32) -> *mut DrmModeEncoder;
        fn drmModeFreeEncoder(encoder: *mut DrmModeEncoder);
        fn drmModeGetCrtc(fd: c_int, id: u32) -> *mut DrmModeCrtc;
        fn drmModeFreeCrtc(crtc: *mut DrmModeCrtc);
        fn drmModeAddFB2(
            fd: c_int,
            width: u32,
            height: u32,
            pixel_format: u32,
            bo_handles: *const u32,
            pitches: *const u32,
            offsets: *const u32,
            buf_id: *mut u32,
            flags: u32,
        ) -> c_int;
        fn drmModeRmFB(fd: c_int, buffer_id: u32) -> c_int;
        fn drmModeSetCrtc(
            fd: c_int,
            crtc_id: u32,
            buffer_id: u32,
            x: u32,
            y: u32,
            connectors: *const u32,
            count: c_int,
            mode: *const DrmModeModeInfo,
        ) -> c_int;
        fn drmModePageFlip(
            fd: c_int,
            crtc_id: u32,
            fb_id: u32,
            flags: u32,
            user_data: *mut c_void,
        ) -> c_int;
        fn drmHandleEvent(fd: c_int, context: *mut DrmEventContext) -> c_int;
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
        fn poll(fds: *mut PollFd, count: usize, timeout_ms: c_int) -> c_int;
    }

    const fn drm_iowr<T>(number: usize) -> usize {
        (3_usize << 30) | (std::mem::size_of::<T>() << 16) | (b'd' as usize) << 8 | number
    }

    const DRM_IOCTL_MODE_CREATE_DUMB: usize = drm_iowr::<DrmModeCreateDumb>(0xb2);
    const DRM_IOCTL_MODE_MAP_DUMB: usize = drm_iowr::<DrmModeMapDumb>(0xb3);
    const DRM_IOCTL_MODE_DESTROY_DUMB: usize = drm_iowr::<DrmModeDestroyDumb>(0xb4);
    const DRM_FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");

    struct DumbBuffer {
        fd: c_int,
        handle: u32,
        framebuffer: u32,
        pitch: u32,
        size: usize,
        mapping: NonNull<u8>,
    }

    impl DumbBuffer {
        fn new(fd: c_int, width: u32, height: u32) -> Result<Self, HostPresentError> {
            let mut create = DrmModeCreateDumb {
                width,
                height,
                bpp: 32,
                ..Default::default()
            };
            if unsafe { ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB as c_ulong, &mut create) } != 0 {
                return Err(error("DRM_IOCTL_MODE_CREATE_DUMB failed"));
            }
            let mut map = DrmModeMapDumb {
                handle: create.handle,
                ..Default::default()
            };
            if unsafe { ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB as c_ulong, &mut map) } != 0 {
                destroy(fd, create.handle);
                return Err(error("DRM_IOCTL_MODE_MAP_DUMB failed"));
            }
            let mapped = unsafe {
                mmap(
                    std::ptr::null_mut(),
                    create.size as usize,
                    PROT_READ | PROT_WRITE,
                    MAP_SHARED,
                    fd,
                    map.offset as i64,
                )
            };
            let Some(mapping) =
                NonNull::new(mapped.cast::<u8>()).filter(|ptr| ptr.as_ptr() as isize != -1)
            else {
                destroy(fd, create.handle);
                return Err(error("mmap DRM dumb buffer failed"));
            };
            let handles = [create.handle, 0, 0, 0];
            let pitches = [create.pitch, 0, 0, 0];
            let offsets = [0_u32; 4];
            let mut framebuffer = 0;
            if unsafe {
                drmModeAddFB2(
                    fd,
                    width,
                    height,
                    DRM_FORMAT_ARGB8888,
                    handles.as_ptr(),
                    pitches.as_ptr(),
                    offsets.as_ptr(),
                    &mut framebuffer,
                    0,
                )
            } != 0
            {
                unsafe { munmap(mapping.as_ptr().cast(), create.size as usize) };
                destroy(fd, create.handle);
                return Err(error("drmModeAddFB2(BGRA8) failed"));
            }
            Ok(Self {
                fd,
                handle: create.handle,
                framebuffer,
                pitch: create.pitch,
                size: create.size as usize,
                mapping,
            })
        }

        fn copy_frame(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError> {
            let row_bytes = frame.mode.width as usize * 4;
            if row_bytes > self.pitch as usize {
                return Err(error("frame row exceeds DRM dumb-buffer pitch"));
            }
            unsafe { std::ptr::write_bytes(self.mapping.as_ptr(), 0, self.size) };
            for row in 0..frame.mode.height as usize {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        frame.bgra[row * row_bytes..(row + 1) * row_bytes].as_ptr(),
                        self.mapping.as_ptr().add(row * self.pitch as usize),
                        row_bytes,
                    );
                }
            }
            Ok(())
        }

        fn visible_digest(&self, width: u32, height: u32) -> Result<[u8; 32], HostPresentError> {
            let width = usize::try_from(width)
                .map_err(|_| error("DRM visible width does not fit the host"))?;
            let height = usize::try_from(height)
                .map_err(|_| error("DRM visible height does not fit the host"))?;
            let row_bytes = width
                .checked_mul(4)
                .ok_or_else(|| error("DRM visible row byte count overflow"))?;
            if row_bytes > self.pitch as usize {
                return Err(error("DRM visible row exceeds dumb-buffer pitch"));
            }
            let mut bytes = Vec::with_capacity(
                row_bytes
                    .checked_mul(height)
                    .ok_or_else(|| error("DRM visible byte count overflow"))?,
            );
            for row in 0..height {
                let source = unsafe {
                    std::slice::from_raw_parts(
                        self.mapping.as_ptr().add(row * self.pitch as usize),
                        row_bytes,
                    )
                };
                bytes.extend_from_slice(source);
            }
            Ok(wrela_machine::sha256::sha256(&bytes))
        }
    }

    impl Drop for DumbBuffer {
        fn drop(&mut self) {
            unsafe {
                let _ = drmModeRmFB(self.fd, self.framebuffer);
                let _ = munmap(self.mapping.as_ptr().cast(), self.size);
            }
            destroy(self.fd, self.handle);
        }
    }

    fn destroy(fd: c_int, handle: u32) {
        let mut destroy = DrmModeDestroyDumb { handle };
        unsafe {
            let _ = ioctl(fd, DRM_IOCTL_MODE_DESTROY_DUMB as c_ulong, &mut destroy);
        }
    }

    pub(super) struct DrmSurface {
        file: File,
        connector: u32,
        crtc: u32,
        mode: DrmModeModeInfo,
        original: OriginalCrtc,
        buffers: [DumbBuffer; 2],
        front: Option<usize>,
    }

    struct OriginalCrtc {
        framebuffer: u32,
        x: u32,
        y: u32,
        mode: Option<DrmModeModeInfo>,
    }

    // The VMM serializes display submission. The mapped dumb buffers never
    // escape this owner and page-flip callbacks only toggle a stack-local
    // completion flag while `present` is blocked in `drmHandleEvent`.
    unsafe impl Send for DrmSurface {}

    impl std::fmt::Debug for DrmSurface {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("DrmSurface")
                .field("connector", &self.connector)
                .field("crtc", &self.crtc)
                .field("scanout", &(self.mode.hdisplay, self.mode.vdisplay))
                .field("front", &self.front)
                .finish()
        }
    }

    impl DrmSurface {
        pub(super) fn new(frame: &PresentedFrame) -> Result<Self, HostPresentError> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/dri/card0")
                .map_err(|cause| error(format!("open /dev/dri/card0: {cause}")))?;
            let fd = file.as_raw_fd();
            let resources = NonNull::new(unsafe { drmModeGetResources(fd) })
                .ok_or_else(|| error("drmModeGetResources failed"))?;
            let result = (|| {
                let resources = unsafe { resources.as_ref() };
                let connectors = unsafe {
                    std::slice::from_raw_parts(
                        resources.connectors,
                        resources.count_connectors.max(0) as usize,
                    )
                };
                for connector_id in connectors {
                    let Some(connector_ptr) =
                        NonNull::new(unsafe { drmModeGetConnector(fd, *connector_id) })
                    else {
                        continue;
                    };
                    let connector = unsafe { connector_ptr.as_ref() };
                    let usable = connector.connection == DRM_MODE_CONNECTED
                        && connector.count_modes > 0
                        && connector.encoder_id != 0;
                    if !usable {
                        unsafe { drmModeFreeConnector(connector_ptr.as_ptr()) };
                        continue;
                    }
                    let modes = unsafe {
                        std::slice::from_raw_parts(connector.modes, connector.count_modes as usize)
                    };
                    let mode = modes.iter().copied().find(|mode| {
                        u32::from(mode.hdisplay) >= frame.mode.width
                            && u32::from(mode.vdisplay) >= frame.mode.height
                    });
                    let Some(mode) = mode else {
                        unsafe { drmModeFreeConnector(connector_ptr.as_ptr()) };
                        continue;
                    };
                    let encoder =
                        NonNull::new(unsafe { drmModeGetEncoder(fd, connector.encoder_id) });
                    let crtc = encoder
                        .map(|encoder| unsafe { encoder.as_ref().crtc_id })
                        .filter(|crtc| *crtc != 0)
                        .or_else(|| unsafe {
                            (resources.count_crtcs > 0).then(|| *resources.crtcs)
                        });
                    if let Some(encoder) = encoder {
                        unsafe { drmModeFreeEncoder(encoder.as_ptr()) };
                    }
                    unsafe { drmModeFreeConnector(connector_ptr.as_ptr()) };
                    let Some(crtc) = crtc else { continue };
                    let original_ptr = NonNull::new(unsafe { drmModeGetCrtc(fd, crtc) })
                        .ok_or_else(|| error("drmModeGetCrtc failed"))?;
                    let original_ref = unsafe { original_ptr.as_ref() };
                    let original = OriginalCrtc {
                        framebuffer: original_ref.buffer_id,
                        x: original_ref.x,
                        y: original_ref.y,
                        mode: (original_ref.mode_valid != 0).then_some(original_ref.mode),
                    };
                    unsafe { drmModeFreeCrtc(original_ptr.as_ptr()) };
                    let buffers = [
                        DumbBuffer::new(fd, u32::from(mode.hdisplay), u32::from(mode.vdisplay))?,
                        DumbBuffer::new(fd, u32::from(mode.hdisplay), u32::from(mode.vdisplay))?,
                    ];
                    return Ok(Self {
                        file,
                        connector: *connector_id,
                        crtc,
                        mode,
                        original,
                        buffers,
                        front: None,
                    });
                }
                Err(error(
                    "no connected DRM connector can contain the guest frame",
                ))
            })();
            unsafe { drmModeFreeResources(resources.as_ptr()) };
            result
        }

        pub(super) fn present(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError> {
            let back = self.front.map_or(0, |front| 1 - front);
            self.buffers[back].copy_frame(frame)?;
            let fd = self.file.as_raw_fd();
            if self.front.is_none() {
                // The successful modeset is the first frame's atomic commit.
                // Do not follow it with a fallible operation that could make
                // the device report rejection after changing scanout.
                if unsafe {
                    drmModeSetCrtc(
                        fd,
                        self.crtc,
                        self.buffers[back].framebuffer,
                        0,
                        0,
                        &self.connector,
                        1,
                        &self.mode,
                    )
                } != 0
                {
                    return Err(error("drmModeSetCrtc failed"));
                }
                self.front = Some(back);
                return Ok(());
            }
            let mut completed = false;
            if unsafe {
                drmModePageFlip(
                    fd,
                    self.crtc,
                    self.buffers[back].framebuffer,
                    DRM_MODE_PAGE_FLIP_EVENT,
                    (&mut completed as *mut bool).cast(),
                )
            } != 0
            {
                return Err(error("drmModePageFlip failed"));
            }
            unsafe extern "C" fn page_flip(
                _fd: c_int,
                _sequence: c_uint,
                _seconds: c_uint,
                _microseconds: c_uint,
                data: *mut c_void,
            ) {
                unsafe { *data.cast::<bool>() = true };
            }
            let mut context = DrmEventContext {
                version: DRM_EVENT_CONTEXT_VERSION,
                vblank_handler: None,
                page_flip_handler: Some(page_flip),
            };
            let mut poll_fd = PollFd {
                fd,
                events: POLLIN,
                revents: 0,
            };
            if unsafe { poll(&mut poll_fd, 1, 1000) } <= 0
                || unsafe { drmHandleEvent(fd, &mut context) } != 0
                || !completed
            {
                return Err(committed_error(
                    "DRM accepted the page flip but its completion became unknown",
                ));
            }
            self.front = Some(back);
            Ok(())
        }

        pub(super) fn front_digest(
            &self,
            frame: &PresentedFrame,
        ) -> Result<[u8; 32], HostPresentError> {
            let front = self
                .front
                .ok_or_else(|| error("DRM has no committed front buffer"))?;
            self.buffers[front].visible_digest(frame.mode.width, frame.mode.height)
        }
    }

    impl Drop for DrmSurface {
        fn drop(&mut self) {
            let mode = self
                .original
                .mode
                .as_ref()
                .map_or(std::ptr::null(), |mode| mode);
            let (connector, count) = if self.original.mode.is_some() {
                (&self.connector as *const u32, 1)
            } else {
                (std::ptr::null(), 0)
            };
            unsafe {
                let _ = drmModeSetCrtc(
                    self.file.as_raw_fd(),
                    self.crtc,
                    self.original.framebuffer,
                    self.original.x,
                    self.original.y,
                    connector,
                    count,
                    mode,
                );
            }
        }
    }

    fn error(message: impl Into<String>) -> HostPresentError {
        HostPresentError {
            backend: BackendKind::LinuxKvm,
            message: message.into(),
            commit_may_have_happened: false,
        }
    }

    fn committed_error(message: impl Into<String>) -> HostPresentError {
        HostPresentError {
            backend: BackendKind::LinuxKvm,
            message: message.into(),
            commit_may_have_happened: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{PresentationBackend, headless::HeadlessBackend, hvf::HvfDisplayBackend};

    fn frame() -> PresentedFrame {
        let bgra = vec![1, 2, 3, 255, 5, 6, 7, 255];
        let digest = wrela_machine::sha256::sha256(&bgra);
        PresentedFrame {
            renderer_index: 0,
            sequence: 0,
            generation: 1,
            released_generation: None,
            mode: wrela_machine::pixels::DisplayModeV1 {
                width: 2,
                height: 1,
                refresh_hz: 60,
            },
            format: wrela_machine::pixels::FORMAT_BGRA8_SRGB,
            tile_descriptor_digest: [2; 32],
            visible_digest: digest,
            raw_tile_digest: [3; 32],
            digest: wrela_machine::sha256::sha256_hex(&bgra),
            vsync_id: 9,
            checkpoint: 11,
            bgra,
        }
    }

    #[test]
    fn all_backends_accept_identical_visible_bytes_before_presentation() {
        let frame = frame();
        let mut headless = HeadlessBackend::default();
        let mut hvf = HvfDisplayBackend::default();
        let mut kvm = KvmDisplayBackend::default();
        headless.present(&frame).unwrap();
        hvf.present(&frame).unwrap();
        kvm.present(&frame).unwrap();
        assert_eq!(headless.digests(), &[frame.visible_digest]);
        assert_eq!(hvf.surface_bytes(), frame.bgra);
        assert_eq!(kvm.buffer_bytes(), frame.bgra);
        assert_eq!(hvf.last_vsync(), Some(9));
        assert_eq!(kvm.last_vsync(), Some(9));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires DRM master on /dev/dri/card0; run in the Linux/KVM hardware lane"]
    fn native_drm_surface_page_flips_exact_bgra_on_vsync() {
        let frame = frame();
        let mut backend = KvmDisplayBackend::native();
        backend.present(&frame).expect("DRM page flip");
        assert_eq!(backend.buffer_bytes(), frame.bgra);
        assert_eq!(backend.last_vsync(), Some(9));
    }
}
