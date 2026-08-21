//! macOS/HVF byte-preserving BGRA8Unorm_sRGB presentation surface.
//!
//! The signed host owns the actual layer/vsync callback.  This adapter owns
//! the texture bytes and exposes the exact callback boundary without moving
//! any renderer decision host-side.

use wrela_machine::pixels::PresentedFrame;

use super::{BackendKind, HostPresentError, PresentationBackend};

#[derive(Debug, Default)]
pub struct MetalDisplayBackend {
    bgra8_unorm_srgb: Vec<u8>,
    last_vsync: Option<u64>,
    last_presented_digest: Option<[u8; 32]>,
    #[cfg(target_os = "macos")]
    native_requested: bool,
    #[cfg(target_os = "macos")]
    native: Option<native::CoreVideoSurface>,
}

impl MetalDisplayBackend {
    pub fn surface_bytes(&self) -> &[u8] {
        &self.bgra8_unorm_srgb
    }

    pub fn last_vsync(&self) -> Option<u64> {
        self.last_vsync
    }

    /// Create the production macOS surface. The ordinary `Default` remains
    /// the deterministic byte sink used by portable device/replay tests.
    #[cfg(target_os = "macos")]
    pub fn native() -> Self {
        Self {
            bgra8_unorm_srgb: Vec::new(),
            last_vsync: None,
            last_presented_digest: None,
            native_requested: true,
            native: None,
        }
    }
}

impl PresentationBackend for MetalDisplayBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Metal
    }

    fn present(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError> {
        if wrela_machine::sha256::sha256(&frame.bgra) != frame.visible_digest {
            return Err(HostPresentError {
                backend: self.kind(),
                message: "BGRA8Unorm_sRGB upload digest differs before present".into(),
                commit_may_have_happened: false,
            });
        }
        self.bgra8_unorm_srgb.clear();
        self.bgra8_unorm_srgb.extend_from_slice(&frame.bgra);
        #[cfg(target_os = "macos")]
        if self.native_requested {
            if self.native.is_none() {
                self.native = Some(native::CoreVideoSurface::new(
                    frame.mode.width,
                    frame.mode.height,
                )?);
            }
            self.native
                .as_mut()
                .expect("initialized above")
                .present(frame)?;
        }
        self.last_vsync = Some(frame.vsync_id);
        self.last_presented_digest = Some(frame.visible_digest);
        Ok(())
    }

    fn presented_digest(&self) -> Option<[u8; 32]> {
        self.last_presented_digest
    }
}

#[cfg(target_os = "macos")]
mod native {
    use std::ffi::c_void;
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    use wrela_machine::pixels::PresentedFrame;

    use super::{BackendKind, HostPresentError};

    const K_CV_PIXEL_FORMAT_TYPE_32_BGRA: u32 = u32::from_be_bytes(*b"BGRA");
    const K_CV_RETURN_SUCCESS: i32 = 0;

    type CVPixelBufferRef = *mut c_void;
    type CVDisplayLinkRef = *mut c_void;
    type CGContextRef = *mut c_void;
    type CGColorSpaceRef = *mut c_void;
    type CGDataProviderRef = *mut c_void;
    type CGImageRef = *mut c_void;
    type CGDirectDisplayId = u32;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreVideo", kind = "framework")]
    unsafe extern "C" {
        fn CVPixelBufferCreate(
            allocator: *const c_void,
            width: usize,
            height: usize,
            pixel_format: u32,
            attributes: *const c_void,
            output: *mut CVPixelBufferRef,
        ) -> i32;
        fn CVPixelBufferLockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
        fn CVPixelBufferUnlockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
        fn CVPixelBufferGetBaseAddress(buffer: CVPixelBufferRef) -> *mut c_void;
        fn CVPixelBufferGetBytesPerRow(buffer: CVPixelBufferRef) -> usize;
        fn CVDisplayLinkCreateWithActiveCGDisplays(output: *mut CVDisplayLinkRef) -> i32;
        fn CVDisplayLinkSetOutputCallback(
            link: CVDisplayLinkRef,
            callback: unsafe extern "C" fn(
                CVDisplayLinkRef,
                *const c_void,
                *const c_void,
                u64,
                *mut u64,
                *mut c_void,
            ) -> i32,
            context: *mut c_void,
        ) -> i32;
        fn CVDisplayLinkStart(link: CVDisplayLinkRef) -> i32;
        fn CVDisplayLinkStop(link: CVDisplayLinkRef) -> i32;
        fn CVDisplayLinkRelease(link: CVDisplayLinkRef);
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        static kCGColorSpaceSRGB: *const c_void;
        fn CGMainDisplayID() -> CGDirectDisplayId;
        fn CGDisplayCapture(display: CGDirectDisplayId) -> i32;
        fn CGDisplayRelease(display: CGDirectDisplayId) -> i32;
        fn CGDisplayGetDrawingContext(display: CGDirectDisplayId) -> CGContextRef;
        fn CGColorSpaceCreateWithName(name: *const c_void) -> CGColorSpaceRef;
        fn CGColorSpaceRelease(space: CGColorSpaceRef);
        fn CGDataProviderCreateWithData(
            info: *mut c_void,
            data: *const c_void,
            size: usize,
            release_data: Option<unsafe extern "C" fn(*mut c_void, *const c_void, usize)>,
        ) -> CGDataProviderRef;
        fn CGDataProviderRelease(provider: CGDataProviderRef);
        fn CGImageCreate(
            width: usize,
            height: usize,
            bits_per_component: usize,
            bits_per_pixel: usize,
            bytes_per_row: usize,
            space: CGColorSpaceRef,
            bitmap_info: u32,
            provider: CGDataProviderRef,
            decode: *const f64,
            should_interpolate: bool,
            intent: u32,
        ) -> CGImageRef;
        fn CGImageRelease(image: CGImageRef);
        fn CGContextDrawImage(context: CGContextRef, rect: CGRect, image: CGImageRef);
        fn CGContextFlush(context: CGContextRef);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }

    #[derive(Clone, Copy)]
    struct PendingCommit {
        context: CGContextRef,
        image: CGImageRef,
        provider: CGDataProviderRef,
        rect: CGRect,
        requested_vsync: u64,
    }

    // The VMM owns every pointee until the display-link callback consumes the
    // pending commit, and `present`/`Drop` stop the link before releasing any
    // unconsumed object.
    unsafe impl Send for PendingCommit {}

    #[derive(Default)]
    struct PresentationState {
        tick: u64,
        pending: Option<PendingCommit>,
        completed_vsync: Option<u64>,
        commit_order: u64,
        notify_order: u64,
        next_order: u64,
    }

    struct VsyncState {
        presentation: Mutex<PresentationState>,
        wake: Condvar,
    }

    pub(super) struct CoreVideoSurface {
        buffer: CVPixelBufferRef,
        display_link: CVDisplayLinkRef,
        state: Box<VsyncState>,
        width: u32,
        height: u32,
        display: CGDirectDisplayId,
        context: CGContextRef,
        srgb: CGColorSpaceRef,
        last_staged_tick: Option<u64>,
        last_completed_tick: Option<u64>,
    }

    // Display access is serialized by the VMM's `Shared` mutex. The callback
    // consumes exactly one staged CoreGraphics image while `present` keeps
    // its guest bytes alive and waits for that requested-vsync commit.
    unsafe impl Send for CoreVideoSurface {}

    impl std::fmt::Debug for CoreVideoSurface {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("CoreVideoSurface")
                .field("width", &self.width)
                .field("height", &self.height)
                .finish_non_exhaustive()
        }
    }

    impl CoreVideoSurface {
        #[cfg(test)]
        pub(super) fn presentation_trace(&self) -> (Option<u64>, Option<u64>, u64, u64) {
            let state = self
                .state
                .presentation
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            (
                self.last_staged_tick,
                self.last_completed_tick,
                state.commit_order,
                state.notify_order,
            )
        }

        pub(super) fn new(width: u32, height: u32) -> Result<Self, HostPresentError> {
            if width == 0 || height == 0 {
                return Err(error("CoreVideo surface dimensions must be positive"));
            }
            let mut buffer = std::ptr::null_mut();
            let create = unsafe {
                CVPixelBufferCreate(
                    std::ptr::null(),
                    width as usize,
                    height as usize,
                    K_CV_PIXEL_FORMAT_TYPE_32_BGRA,
                    std::ptr::null(),
                    &mut buffer,
                )
            };
            if create != K_CV_RETURN_SUCCESS || buffer.is_null() {
                return Err(error(format!(
                    "CVPixelBufferCreate(BGRA8Unorm_sRGB) failed with {create}"
                )));
            }
            let mut display_link = std::ptr::null_mut();
            let link_create = unsafe { CVDisplayLinkCreateWithActiveCGDisplays(&mut display_link) };
            if link_create != K_CV_RETURN_SUCCESS || display_link.is_null() {
                unsafe { CFRelease(buffer) };
                return Err(error(format!(
                    "CVDisplayLinkCreateWithActiveCGDisplays failed with {link_create}"
                )));
            }
            let mut state = Box::new(VsyncState {
                presentation: Mutex::new(PresentationState::default()),
                wake: Condvar::new(),
            });
            let callback = unsafe {
                CVDisplayLinkSetOutputCallback(
                    display_link,
                    display_link_callback,
                    (&mut *state as *mut VsyncState).cast(),
                )
            };
            if callback != K_CV_RETURN_SUCCESS {
                unsafe {
                    CVDisplayLinkRelease(display_link);
                    CFRelease(buffer);
                }
                return Err(error(format!(
                    "CVDisplayLinkSetOutputCallback failed with {callback}"
                )));
            }
            let display = unsafe { CGMainDisplayID() };
            let capture = unsafe { CGDisplayCapture(display) };
            if capture != 0 {
                unsafe {
                    CVDisplayLinkRelease(display_link);
                    CFRelease(buffer);
                }
                return Err(error(format!(
                    "CGDisplayCapture failed with {capture}; native presentation requires display capture permission"
                )));
            }
            let context = unsafe { CGDisplayGetDrawingContext(display) };
            let srgb = unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) };
            if context.is_null() || srgb.is_null() {
                unsafe {
                    if !srgb.is_null() {
                        CGColorSpaceRelease(srgb);
                    }
                    let _ = CGDisplayRelease(display);
                    CVDisplayLinkRelease(display_link);
                    CFRelease(buffer);
                }
                return Err(error(
                    "CoreGraphics did not provide a captured sRGB drawing surface",
                ));
            }
            Ok(Self {
                buffer,
                display_link,
                state,
                width,
                height,
                display,
                context,
                srgb,
                last_staged_tick: None,
                last_completed_tick: None,
            })
        }

        pub(super) fn present(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError> {
            if frame.mode.width != self.width || frame.mode.height != self.height {
                return Err(error(format!(
                    "frame mode {}x{} differs from CoreVideo surface {}x{}",
                    frame.mode.width, frame.mode.height, self.width, self.height
                )));
            }
            let row_bytes = usize::try_from(self.width)
                .ok()
                .and_then(|width| width.checked_mul(4))
                .ok_or_else(|| error("CoreVideo row byte count overflow"))?;
            let lock = unsafe { CVPixelBufferLockBaseAddress(self.buffer, 0) };
            if lock != K_CV_RETURN_SUCCESS {
                return Err(error(format!(
                    "CVPixelBufferLockBaseAddress failed with {lock}"
                )));
            }
            let result = (|| {
                let base = unsafe { CVPixelBufferGetBaseAddress(self.buffer) }.cast::<u8>();
                let stride = unsafe { CVPixelBufferGetBytesPerRow(self.buffer) };
                if base.is_null() || stride < row_bytes {
                    return Err(error("CoreVideo returned an invalid BGRA row mapping"));
                }
                for row in 0..self.height as usize {
                    let source = &frame.bgra[row * row_bytes..(row + 1) * row_bytes];
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            source.as_ptr(),
                            base.add(row * stride),
                            row_bytes,
                        )
                    };
                }
                Ok(())
            })();
            let unlock = unsafe { CVPixelBufferUnlockBaseAddress(self.buffer, 0) };
            result?;
            if unlock != K_CV_RETURN_SUCCESS {
                return Err(error(format!(
                    "CVPixelBufferUnlockBaseAddress failed with {unlock}"
                )));
            }

            let provider = unsafe {
                CGDataProviderCreateWithData(
                    std::ptr::null_mut(),
                    frame.bgra.as_ptr().cast(),
                    frame.bgra.len(),
                    None,
                )
            };
            if provider.is_null() {
                return Err(error("CGDataProviderCreateWithData failed"));
            }
            // BGRA bytes, opaque alpha, and an explicit sRGB color space.
            // CoreGraphics sees little-endian premultiplied-first words; with
            // alpha fixed at 255 this is byte-preserving BGRA8Unorm_sRGB.
            const BITMAP_INFO: u32 = 0x2000 | 2;
            let image = unsafe {
                CGImageCreate(
                    self.width as usize,
                    self.height as usize,
                    8,
                    32,
                    row_bytes,
                    self.srgb,
                    BITMAP_INFO,
                    provider,
                    std::ptr::null(),
                    false,
                    0,
                )
            };
            if image.is_null() {
                unsafe { CGDataProviderRelease(provider) };
                return Err(error("CGImageCreate(BGRA8Unorm_sRGB) failed"));
            }
            let mut state = self
                .state
                .presentation
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if state.pending.is_some() {
                unsafe {
                    CGImageRelease(image);
                    CGDataProviderRelease(provider);
                }
                return Err(error("CoreVideo already has a pending display commit"));
            }
            let before = state.tick;
            self.last_staged_tick = Some(before);
            state.completed_vsync = None;
            state.commit_order = 0;
            state.notify_order = 0;
            state.pending = Some(PendingCommit {
                context: self.context,
                image,
                provider,
                rect: CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: f64::from(self.width),
                        height: f64::from(self.height),
                    },
                },
                requested_vsync: frame.vsync_id,
            });
            let start = unsafe { CVDisplayLinkStart(self.display_link) };
            if start != K_CV_RETURN_SUCCESS {
                if let Some(pending) = state.pending.take() {
                    unsafe {
                        CGImageRelease(pending.image);
                        CGDataProviderRelease(pending.provider);
                    }
                }
                return Err(error(format!("CVDisplayLinkStart failed with {start}")));
            }
            let (state, timeout) = self
                .state
                .wake
                .wait_timeout_while(state, Duration::from_millis(250), |state| {
                    state.completed_vsync != Some(frame.vsync_id)
                })
                .unwrap_or_else(|poison| poison.into_inner());
            let initially_committed = state.completed_vsync == Some(frame.vsync_id);
            drop(state);
            let stop = unsafe { CVDisplayLinkStop(self.display_link) };
            let mut state = self
                .state
                .presentation
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let committed = initially_committed || state.completed_vsync == Some(frame.vsync_id);
            if stop != K_CV_RETURN_SUCCESS {
                return Err(committed_error(format!(
                    "CVDisplayLinkStop failed with {stop} after the requested commit became uncertain"
                )));
            }
            if !committed {
                if let Some(pending) = state.pending.take() {
                    unsafe {
                        CGImageRelease(pending.image);
                        CGDataProviderRelease(pending.provider);
                    }
                }
                let reason = if timeout.timed_out() {
                    "CoreVideo display link missed the requested vsync"
                } else {
                    "CoreVideo display link woke without committing the requested vsync"
                };
                return Err(error(reason));
            }
            self.last_completed_tick = Some(state.tick);
            Ok(())
        }
    }

    impl Drop for CoreVideoSurface {
        fn drop(&mut self) {
            unsafe {
                let _ = CVDisplayLinkStop(self.display_link);
                if let Some(pending) = self
                    .state
                    .presentation
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .pending
                    .take()
                {
                    CGImageRelease(pending.image);
                    CGDataProviderRelease(pending.provider);
                }
                CGColorSpaceRelease(self.srgb);
                let _ = CGDisplayRelease(self.display);
                CVDisplayLinkRelease(self.display_link);
                CFRelease(self.buffer);
            }
        }
    }

    unsafe extern "C" fn display_link_callback(
        _link: CVDisplayLinkRef,
        _now: *const c_void,
        _output: *const c_void,
        _flags_in: u64,
        _flags_out: *mut u64,
        context: *mut c_void,
    ) -> i32 {
        let state = unsafe { &*(context.cast::<VsyncState>()) };
        let mut presentation = state
            .presentation
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        presentation.tick = presentation.tick.wrapping_add(1);
        if let Some(pending) = presentation.pending.take() {
            unsafe {
                CGContextDrawImage(pending.context, pending.rect, pending.image);
                CGContextFlush(pending.context);
                CGImageRelease(pending.image);
                CGDataProviderRelease(pending.provider);
            }
            presentation.completed_vsync = Some(pending.requested_vsync);
            presentation.next_order = presentation.next_order.wrapping_add(1);
            presentation.commit_order = presentation.next_order;
        }
        presentation.next_order = presentation.next_order.wrapping_add(1);
        presentation.notify_order = presentation.next_order;
        drop(presentation);
        state.wake.notify_all();
        K_CV_RETURN_SUCCESS
    }

    fn error(message: impl Into<String>) -> HostPresentError {
        HostPresentError {
            backend: BackendKind::Metal,
            message: message.into(),
            commit_may_have_happened: false,
        }
    }

    fn committed_error(message: impl Into<String>) -> HostPresentError {
        HostPresentError {
            backend: BackendKind::Metal,
            message: message.into(),
            commit_may_have_happened: true,
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires interactive display-capture permission; verify-deep runs this on the signed host"]
    fn native_coregraphics_surface_accepts_exact_srgb_bgra_on_vsync() {
        let bgra = vec![1, 2, 3, 255, 4, 5, 6, 255];
        let frame = PresentedFrame {
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
            visible_digest: wrela_machine::sha256::sha256(&bgra),
            raw_tile_digest: [3; 32],
            digest: wrela_machine::sha256::sha256_hex(&bgra),
            vsync_id: 7,
            checkpoint: 9,
            bgra,
        };
        let mut backend = MetalDisplayBackend::native();
        backend.present(&frame).expect("present on display link");
        assert_eq!(backend.surface_bytes(), frame.bgra);
        assert_eq!(backend.last_vsync(), Some(7));
        let surface = backend.native.as_ref().expect("native surface initialized");
        let (staged, completed, commit_order, notify_order) = surface.presentation_trace();
        assert!(
            staged < completed,
            "the frame must be staged before its matching display-link completion"
        );
        assert!(
            commit_order != 0 && commit_order < notify_order,
            "the display-link callback must commit before it publishes completion"
        );
    }
}
