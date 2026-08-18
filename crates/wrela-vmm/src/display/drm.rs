//! Linux/KVM DRM-dumb-buffer compatible BGRA scanout gather.

use wrela_machine::pixels::PresentedFrame;

use super::{BackendKind, HostPresentError, PresentationBackend};

#[derive(Debug, Default)]
pub struct DrmDisplayBackend {
    drm_dumb_buffer: Vec<u8>,
    last_vsync: Option<u64>,
    last_presented_digest: Option<[u8; 32]>,
    #[cfg(target_os = "linux")]
    native_requested: bool,
    #[cfg(target_os = "linux")]
    native_readback_validation: bool,
    #[cfg(all(target_os = "linux", feature = "native-presentation"))]
    native: Option<native::DrmSurface>,
}

impl DrmDisplayBackend {
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
            // Reading a write-combined dumb-buffer mapping is intentionally a
            // validation-only operation. Ordinary presentation trusts the
            // staged bytes that `copy_frame` copies verbatim.
            native_readback_validation: std::env::var_os("WRELA_DRM_VALIDATE_READBACK")
                .is_some_and(|value| value == "1"),
            #[cfg(feature = "native-presentation")]
            native: None,
        }
    }
}

impl PresentationBackend for DrmDisplayBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Drm
    }

    fn present(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError> {
        let staged_digest = wrela_machine::sha256::sha256(&frame.bgra);
        if staged_digest != frame.visible_digest {
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
            #[cfg(not(feature = "native-presentation"))]
            return Err(HostPresentError {
                backend: self.kind(),
                message: "native DRM presentation is not present in this headless artifact".into(),
                commit_may_have_happened: false,
            });
            #[cfg(feature = "native-presentation")]
            {
                if self.native.is_none() {
                    self.native = Some(native::DrmSurface::new(frame)?);
                }
                let native = self.native.as_mut().expect("initialized above");
                native.present(frame)?;
                self.last_presented_digest = Some(if self.native_readback_validation {
                    // Validation proves each buffer's current extent once;
                    // sustained sequences never pay an uncached read per
                    // frame even when the option is enabled.
                    native.verify_front_once(frame)?.unwrap_or(staged_digest)
                } else {
                    staged_digest
                });
            }
        } else {
            self.last_presented_digest = Some(staged_digest);
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.last_presented_digest = Some(staged_digest);
        }
        self.last_vsync = Some(frame.vsync_id);
        Ok(())
    }

    fn presented_digest(&self) -> Option<[u8; 32]> {
        self.last_presented_digest
    }
}

#[cfg(all(target_os = "linux", feature = "native-presentation"))]
mod native {
    use std::ffi::{c_int, c_ulong, c_void};
    use std::fs::{File, OpenOptions};
    use std::os::fd::AsRawFd;
    use std::ptr::NonNull;

    use wrela_machine::pixels::PresentedFrame;

    use super::{BackendKind, HostPresentError};

    const DRM_MODE_CONNECTED: u32 = 1;
    const DRM_MODE_PAGE_FLIP_EVENT: u32 = 1;
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
    #[derive(Default)]
    struct DrmModeCardRes {
        fb_id_ptr: u64,
        crtc_id_ptr: u64,
        connector_id_ptr: u64,
        encoder_id_ptr: u64,
        count_fbs: u32,
        count_crtcs: u32,
        count_connectors: u32,
        count_encoders: u32,
        min_width: u32,
        max_width: u32,
        min_height: u32,
        max_height: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct DrmModeConnector {
        encoders_ptr: u64,
        modes_ptr: u64,
        props_ptr: u64,
        prop_values_ptr: u64,
        count_modes: u32,
        count_props: u32,
        count_encoders: u32,
        encoder_id: u32,
        connector_id: u32,
        connector_type: u32,
        connector_type_id: u32,
        connection: u32,
        mm_width: u32,
        mm_height: u32,
        subpixel: u32,
        pad: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct DrmModeEncoder {
        encoder_id: u32,
        encoder_type: u32,
        crtc_id: u32,
        possible_crtcs: u32,
        possible_clones: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct DrmModeCrtc {
        set_connectors_ptr: u64,
        count_connectors: u32,
        crtc_id: u32,
        fb_id: u32,
        x: u32,
        y: u32,
        gamma_size: u32,
        mode_valid: u32,
        mode: DrmModeModeInfo,
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
    #[derive(Default)]
    struct DrmModeFbCmd2 {
        fb_id: u32,
        width: u32,
        height: u32,
        pixel_format: u32,
        flags: u32,
        handles: [u32; 4],
        pitches: [u32; 4],
        offsets: [u32; 4],
        modifier: [u64; 4],
    }

    #[repr(C)]
    struct DrmModeCrtcPageFlip {
        crtc_id: u32,
        fb_id: u32,
        flags: u32,
        reserved: u32,
        user_data: u64,
    }

    const _: () = {
        assert!(std::mem::size_of::<DrmModeCrtcPageFlip>() == 24);
        assert!(std::mem::offset_of!(DrmModeCrtcPageFlip, crtc_id) == 0);
        assert!(std::mem::offset_of!(DrmModeCrtcPageFlip, fb_id) == 4);
        assert!(std::mem::offset_of!(DrmModeCrtcPageFlip, flags) == 8);
        assert!(std::mem::offset_of!(DrmModeCrtcPageFlip, reserved) == 12);
        assert!(std::mem::offset_of!(DrmModeCrtcPageFlip, user_data) == 16);
    };

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct DrmEventVblank {
        type_: u32,
        length: u32,
        user_data: u64,
        tv_sec: u32,
        tv_usec: u32,
        sequence: u32,
        crtc_id: u32,
    }

    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: i16,
        revents: i16,
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
        fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
    }

    const fn drm_iowr<T>(number: usize) -> usize {
        (3_usize << 30) | (std::mem::size_of::<T>() << 16) | (b'd' as usize) << 8 | number
    }

    const DRM_IOCTL_MODE_CREATE_DUMB: usize = drm_iowr::<DrmModeCreateDumb>(0xb2);
    const DRM_IOCTL_MODE_MAP_DUMB: usize = drm_iowr::<DrmModeMapDumb>(0xb3);
    const DRM_IOCTL_MODE_DESTROY_DUMB: usize = drm_iowr::<DrmModeDestroyDumb>(0xb4);
    const DRM_IOCTL_MODE_GETRESOURCES: usize = drm_iowr::<DrmModeCardRes>(0xa0);
    const DRM_IOCTL_MODE_GETCRTC: usize = drm_iowr::<DrmModeCrtc>(0xa1);
    const DRM_IOCTL_MODE_SETCRTC: usize = drm_iowr::<DrmModeCrtc>(0xa2);
    const DRM_IOCTL_MODE_GETENCODER: usize = drm_iowr::<DrmModeEncoder>(0xa6);
    const DRM_IOCTL_MODE_GETCONNECTOR: usize = drm_iowr::<DrmModeConnector>(0xa7);
    const DRM_IOCTL_MODE_RMFB: usize = drm_iowr::<u32>(0xaf);
    const DRM_IOCTL_MODE_PAGE_FLIP: usize = drm_iowr::<DrmModeCrtcPageFlip>(0xb0);
    const DRM_IOCTL_MODE_ADDFB2: usize = drm_iowr::<DrmModeFbCmd2>(0xb8);
    const DRM_EVENT_FLIP_COMPLETE: u32 = 0x02;
    const DRM_FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");

    struct DumbBuffer {
        fd: c_int,
        handle: u32,
        framebuffer: u32,
        pitch: u32,
        size: usize,
        mapping: NonNull<u8>,
        /// Extent last drawn into this buffer. Only a change of extent can
        /// leave stale pixels outside the drawn region.
        cleared_extent: Option<(u32, u32)>,
        /// Whether the scanout mapping has been proven byte-exact once.
        verified: bool,
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
            let mut add = DrmModeFbCmd2 {
                width,
                height,
                pixel_format: DRM_FORMAT_ARGB8888,
                handles: [create.handle, 0, 0, 0],
                pitches: [create.pitch, 0, 0, 0],
                ..Default::default()
            };
            if unsafe { ioctl(fd, DRM_IOCTL_MODE_ADDFB2 as c_ulong, &mut add) } != 0 {
                unsafe { munmap(mapping.as_ptr().cast(), create.size as usize) };
                destroy(fd, create.handle);
                return Err(error("DRM_IOCTL_MODE_ADDFB2(BGRA8) failed"));
            }
            Ok(Self {
                fd,
                handle: create.handle,
                framebuffer: add.fb_id,
                pitch: create.pitch,
                size: create.size as usize,
                mapping,
                cleared_extent: None,
                verified: false,
            })
        }

        fn copy_frame(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError> {
            let width = frame.mode.width as usize;
            let height = frame.mode.height as usize;
            let row_bytes = width
                .checked_mul(4)
                .ok_or_else(|| error("DRM frame row byte count overflow"))?;
            if row_bytes > self.pitch as usize {
                return Err(error("frame row exceeds DRM dumb-buffer pitch"));
            }
            // The surface picks its mode from the first frame, so a later
            // frame declaring a taller mode would otherwise write past the
            // mapping. Refuse rather than scribble outside the dumb buffer.
            if height
                .checked_mul(self.pitch as usize)
                .is_none_or(|end| end > self.size)
            {
                return Err(error("frame exceeds the DRM dumb-buffer extent"));
            }
            if frame
                .bgra
                .len()
                .checked_div(row_bytes.max(1))
                .is_none_or(|rows| rows < height)
            {
                return Err(error("frame carries fewer bytes than its declared mode"));
            }
            // Clearing the whole buffer every frame costs a full write-combined
            // pass over megabytes that the copy below immediately overwrites.
            // Only a change of drawn extent can expose stale pixels.
            let extent = (frame.mode.width, frame.mode.height);
            if self.cleared_extent != Some(extent) {
                unsafe { std::ptr::write_bytes(self.mapping.as_ptr(), 0, self.size) };
                self.cleared_extent = Some(extent);
                // The margin changed, so the proof of an earlier extent no
                // longer describes what is now on screen.
                self.verified = false;
            }
            for row in 0..height {
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

        /// Prove once per buffer that the bytes actually on the scanout are the
        /// frame's visible bytes. Reading back a write-combined mapping is
        /// uncached and very slow — megabytes per frame at 1080p — so it runs
        /// only until each buffer's current extent has been verified.
        fn verify_scanout_once(
            &mut self,
            width: u32,
            height: u32,
        ) -> Result<Option<[u8; 32]>, HostPresentError> {
            if self.verified {
                return Ok(None);
            }
            let digest = self.visible_digest(width, height)?;
            self.verified = true;
            Ok(Some(digest))
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
                let mut framebuffer = self.framebuffer;
                let _ = ioctl(self.fd, DRM_IOCTL_MODE_RMFB as c_ulong, &mut framebuffer);
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

    struct Resources {
        crtcs: Vec<u32>,
        connectors: Vec<u32>,
    }

    struct Connector {
        encoder_id: u32,
        connection: u32,
        modes: Vec<DrmModeModeInfo>,
    }

    fn bounded_count(value: u32, what: &str) -> Result<usize, HostPresentError> {
        if value > 4096 {
            Err(error(format!(
                "DRM {what} count {value} exceeds the 4096 ceiling"
            )))
        } else {
            Ok(value as usize)
        }
    }

    fn get_resources(fd: c_int) -> Result<Resources, HostPresentError> {
        let mut raw = DrmModeCardRes::default();
        if unsafe { ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES as c_ulong, &mut raw) } != 0 {
            return Err(error("DRM_IOCTL_MODE_GETRESOURCES sizing failed"));
        }
        let mut crtcs = vec![0_u32; bounded_count(raw.count_crtcs, "CRTC")?];
        let mut connectors = vec![0_u32; bounded_count(raw.count_connectors, "connector")?];
        let mut fbs = vec![0_u32; bounded_count(raw.count_fbs, "framebuffer")?];
        let mut encoders = vec![0_u32; bounded_count(raw.count_encoders, "encoder")?];
        raw.crtc_id_ptr = crtcs.as_mut_ptr() as u64;
        raw.connector_id_ptr = connectors.as_mut_ptr() as u64;
        raw.fb_id_ptr = fbs.as_mut_ptr() as u64;
        raw.encoder_id_ptr = encoders.as_mut_ptr() as u64;
        if unsafe { ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES as c_ulong, &mut raw) } != 0
            || raw.count_crtcs as usize > crtcs.len()
            || raw.count_connectors as usize > connectors.len()
        {
            return Err(error("DRM_IOCTL_MODE_GETRESOURCES fill failed or raced"));
        }
        crtcs.truncate(raw.count_crtcs as usize);
        connectors.truncate(raw.count_connectors as usize);
        Ok(Resources { crtcs, connectors })
    }

    fn get_connector(fd: c_int, id: u32) -> Result<Connector, HostPresentError> {
        let mut raw = DrmModeConnector {
            connector_id: id,
            ..Default::default()
        };
        if unsafe { ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR as c_ulong, &mut raw) } != 0 {
            return Err(error("DRM_IOCTL_MODE_GETCONNECTOR sizing failed"));
        }
        let mut modes = vec![DrmModeModeInfo::default(); bounded_count(raw.count_modes, "mode")?];
        let mut props = vec![0_u32; bounded_count(raw.count_props, "property")?];
        let mut values = vec![0_u64; props.len()];
        let mut encoders = vec![0_u32; bounded_count(raw.count_encoders, "connector encoder")?];
        raw.modes_ptr = modes.as_mut_ptr() as u64;
        raw.props_ptr = props.as_mut_ptr() as u64;
        raw.prop_values_ptr = values.as_mut_ptr() as u64;
        raw.encoders_ptr = encoders.as_mut_ptr() as u64;
        if unsafe { ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR as c_ulong, &mut raw) } != 0
            || raw.count_modes as usize > modes.len()
        {
            return Err(error("DRM_IOCTL_MODE_GETCONNECTOR fill failed or raced"));
        }
        modes.truncate(raw.count_modes as usize);
        Ok(Connector {
            encoder_id: raw.encoder_id,
            connection: raw.connection,
            modes,
        })
    }

    fn get_encoder(fd: c_int, id: u32) -> Result<DrmModeEncoder, HostPresentError> {
        let mut raw = DrmModeEncoder {
            encoder_id: id,
            ..Default::default()
        };
        if unsafe { ioctl(fd, DRM_IOCTL_MODE_GETENCODER as c_ulong, &mut raw) } == 0 {
            Ok(raw)
        } else {
            Err(error("DRM_IOCTL_MODE_GETENCODER failed"))
        }
    }

    fn get_crtc(fd: c_int, id: u32) -> Result<DrmModeCrtc, HostPresentError> {
        let mut raw = DrmModeCrtc {
            crtc_id: id,
            ..Default::default()
        };
        if unsafe { ioctl(fd, DRM_IOCTL_MODE_GETCRTC as c_ulong, &mut raw) } == 0 {
            Ok(raw)
        } else {
            Err(error("DRM_IOCTL_MODE_GETCRTC failed"))
        }
    }

    fn set_crtc(
        fd: c_int,
        crtc: u32,
        framebuffer: u32,
        x: u32,
        y: u32,
        connector: Option<u32>,
        mode: Option<&DrmModeModeInfo>,
    ) -> Result<(), HostPresentError> {
        let connector_storage = connector.unwrap_or(0);
        let mut raw = DrmModeCrtc {
            set_connectors_ptr: connector.map_or(0, |_| (&connector_storage as *const u32) as u64),
            count_connectors: u32::from(connector.is_some()),
            crtc_id: crtc,
            fb_id: framebuffer,
            x,
            y,
            mode_valid: u32::from(mode.is_some()),
            mode: mode.copied().unwrap_or_default(),
            ..Default::default()
        };
        if unsafe { ioctl(fd, DRM_IOCTL_MODE_SETCRTC as c_ulong, &mut raw) } == 0 {
            Ok(())
        } else {
            Err(error("DRM_IOCTL_MODE_SETCRTC failed"))
        }
    }

    fn open_card() -> Result<File, HostPresentError> {
        let mut failures = Vec::new();
        for path in ["/dev/dri/card1", "/dev/dri/card0"] {
            match OpenOptions::new().read(true).write(true).open(path) {
                Ok(file) => return Ok(file),
                Err(cause) => failures.push(format!("{path}: {cause}")),
            }
        }
        Err(error(format!("open DRM card: {}", failures.join("; "))))
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
            let file = open_card()?;
            let fd = file.as_raw_fd();
            let resources = get_resources(fd)?;
            for connector_id in &resources.connectors {
                let connector = match get_connector(fd, *connector_id) {
                    Ok(connector) => connector,
                    Err(_) => continue,
                };
                let usable = connector.connection == DRM_MODE_CONNECTED
                    && !connector.modes.is_empty()
                    && connector.encoder_id != 0;
                if !usable {
                    continue;
                }
                let mode = connector.modes.iter().copied().find(|mode| {
                    u32::from(mode.hdisplay) >= frame.mode.width
                        && u32::from(mode.vdisplay) >= frame.mode.height
                });
                let Some(mode) = mode else { continue };
                let crtc = get_encoder(fd, connector.encoder_id)
                    .ok()
                    .map(|encoder| encoder.crtc_id)
                    .filter(|crtc| *crtc != 0)
                    .or_else(|| resources.crtcs.first().copied());
                let Some(crtc) = crtc else { continue };
                let original_ref = get_crtc(fd, crtc)?;
                let original = OriginalCrtc {
                    framebuffer: original_ref.fb_id,
                    x: original_ref.x,
                    y: original_ref.y,
                    mode: (original_ref.mode_valid != 0).then_some(original_ref.mode),
                };
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
        }

        pub(super) fn present(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError> {
            let back = self.front.map_or(0, |front| 1 - front);
            self.buffers[back].copy_frame(frame)?;
            let fd = self.file.as_raw_fd();
            if self.front.is_none() {
                // The successful modeset is the first frame's atomic commit.
                // Do not follow it with a fallible operation that could make
                // the device report rejection after changing scanout.
                set_crtc(
                    fd,
                    self.crtc,
                    self.buffers[back].framebuffer,
                    0,
                    0,
                    Some(self.connector),
                    Some(&self.mode),
                )?;
                self.front = Some(back);
                return Ok(());
            }
            let token = (&self.buffers[back] as *const DumbBuffer) as u64;
            let mut flip = DrmModeCrtcPageFlip {
                crtc_id: self.crtc,
                fb_id: self.buffers[back].framebuffer,
                flags: DRM_MODE_PAGE_FLIP_EVENT,
                reserved: 0,
                user_data: token,
            };
            if unsafe { ioctl(fd, DRM_IOCTL_MODE_PAGE_FLIP as c_ulong, &mut flip) } != 0 {
                return Err(error("DRM_IOCTL_MODE_PAGE_FLIP failed"));
            }
            let mut poll_fd = PollFd {
                fd,
                events: POLLIN,
                revents: 0,
            };
            let mut event = DrmEventVblank::default();
            let got = if unsafe { poll(&mut poll_fd, 1, 1000) } > 0 {
                unsafe {
                    read(
                        fd,
                        (&mut event as *mut DrmEventVblank).cast(),
                        std::mem::size_of::<DrmEventVblank>(),
                    )
                }
            } else {
                -1
            };
            if got != std::mem::size_of::<DrmEventVblank>() as isize
                || event.type_ != DRM_EVENT_FLIP_COMPLETE
                || event.length as usize != std::mem::size_of::<DrmEventVblank>()
                || event.user_data != token
            {
                return Err(committed_error(
                    "DRM accepted the page flip but its completion became unknown",
                ));
            }
            self.front = Some(back);
            Ok(())
        }

        pub(super) fn verify_front_once(
            &mut self,
            frame: &PresentedFrame,
        ) -> Result<Option<[u8; 32]>, HostPresentError> {
            let front = self
                .front
                .ok_or_else(|| error("DRM has no committed front buffer"))?;
            self.buffers[front].verify_scanout_once(frame.mode.width, frame.mode.height)
        }
    }

    impl Drop for DrmSurface {
        fn drop(&mut self) {
            let _ = set_crtc(
                self.file.as_raw_fd(),
                self.crtc,
                self.original.framebuffer,
                self.original.x,
                self.original.y,
                self.original.mode.as_ref().map(|_| self.connector),
                self.original.mode.as_ref(),
            );
        }
    }

    fn error(message: impl Into<String>) -> HostPresentError {
        HostPresentError {
            backend: BackendKind::Drm,
            message: message.into(),
            commit_may_have_happened: false,
        }
    }

    fn committed_error(message: impl Into<String>) -> HostPresentError {
        HostPresentError {
            backend: BackendKind::Drm,
            message: message.into(),
            commit_may_have_happened: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{
        PresentationBackend, headless::HeadlessBackend, metal::MetalDisplayBackend,
    };

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
        let mut hvf = MetalDisplayBackend::default();
        let mut kvm = DrmDisplayBackend::default();
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
        let first = frame();
        let mut backend = DrmDisplayBackend::native();
        backend.present(&first).expect("initial DRM modeset");
        assert_eq!(backend.presented_digest(), Some(first.visible_digest));

        let mut second = first.clone();
        second.sequence = 1;
        second.generation = 2;
        second.vsync_id = 10;
        second.bgra = vec![9, 8, 7, 255, 6, 5, 4, 255];
        second.visible_digest = wrela_machine::sha256::sha256(&second.bgra);
        second.digest = wrela_machine::sha256::sha256_hex(&second.bgra);
        backend
            .present(&second)
            .expect("second DRM page flip acknowledgement");
        assert_eq!(backend.buffer_bytes(), second.bgra);
        assert_eq!(backend.presented_digest(), Some(second.visible_digest));
        assert_eq!(backend.last_vsync(), Some(10));
    }
}
