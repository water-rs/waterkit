//! Zero-copy frames from `AVFoundation`'s capture buffers.
//!
//! The capture output delivers biplanar 4:2:0 `CVPixelBuffer`s backed by an
//! `IOSurface`. Each frame imports the surface's luma and chroma planes as
//! textures that alias its memory, so no pixel is copied. The buffer is the
//! import's owner: the plane textures hold it until `wgpu` destroys them,
//! after the last submission that read them has completed, and only then
//! does it go back to the capture pool.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::Duration;

use objc2_core_foundation::{CFRetained, CFString, CFType};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetIOSurface, kCVImageBufferColorPrimaries_EBU_3213,
    kCVImageBufferColorPrimaries_ITU_R_709_2, kCVImageBufferColorPrimaries_ITU_R_2020,
    kCVImageBufferColorPrimaries_P3_D65, kCVImageBufferColorPrimaries_SMPTE_C,
    kCVImageBufferColorPrimariesKey, kCVImageBufferTransferFunction_ITU_R_709_2,
    kCVImageBufferTransferFunction_ITU_R_2020, kCVImageBufferTransferFunction_ITU_R_2100_HLG,
    kCVImageBufferTransferFunction_SMPTE_240M_1995,
    kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ, kCVImageBufferTransferFunction_sRGB,
    kCVImageBufferTransferFunctionKey, kCVImageBufferYCbCrMatrix_ITU_R_601_4,
    kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVImageBufferYCbCrMatrix_ITU_R_2020,
    kCVImageBufferYCbCrMatrixKey,
};
use wgpu_external_frame::YcbcrMatrix;
use wgpu_external_frame::io_surface::{Ycbcr420IoSurfaceFrame, Ycbcr420Plane};

use crate::analysis::AnalysisFrame;
use crate::color::{from_ycbcr_matrix, from_ycbcr_range};
use crate::frame::{Frame, FramePlanes, orientation_from_rotation};
use crate::{ColorPrimaries, MatrixCoefficients, TransferFunction, VideoColorInfo};

/// A retained `CVPixelBuffer` that may cross threads.
///
/// Core Foundation's reference counting is thread-safe, and nothing here
/// reads or writes the buffer's pixels on the CPU: it is only queried for
/// immutable metadata and released.
#[derive(Debug, Clone)]
pub struct CapturedPixelBuffer(pub(crate) CFRetained<CVPixelBuffer>);

// SAFETY: see the type's documentation; the buffer is only retained, released
// and queried for immutable properties, all thread-safe in Core Video.
#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "the Core Foundation reference is exactly what this impl vouches for"
)]
unsafe impl Send for CapturedPixelBuffer {}

impl CapturedPixelBuffer {
    /// Takes ownership of one reference on a pixel buffer.
    ///
    /// # Safety
    ///
    /// `buffer` must be a live `CVPixelBuffer` carrying a reference that the
    /// caller hands over.
    pub unsafe fn from_owned(buffer: NonNull<CVPixelBuffer>) -> Self {
        // SAFETY: the caller transfers one reference on a live buffer.
        Self(unsafe { CFRetained::from_raw(buffer) })
    }
}

/// One captured frame as the capture callback delivered it.
#[derive(Debug)]
pub struct RawFrame {
    pub pixel_buffer: CapturedPixelBuffer,
    pub timestamp: Duration,
    /// Clockwise rotation, in degrees, that makes the unmirrored buffer
    /// upright.
    pub rotation_degrees: u32,
    /// Whether the capture connection mirrored the buffer.
    pub mirrored: bool,
}

/// Builds a frame whose planes alias `raw`'s `IOSurface`.
///
/// # Panics
///
/// Panics when the buffer has no `IOSurface`, is not biplanar 4:2:0 YCbCr,
/// or carries no supported matrix, primaries, or transfer attachment. The
/// capture output is configured for `420f` or `420v`, so each is a platform
/// defect.
pub fn build_frame(device: &wgpu::Device, raw: RawFrame) -> Frame {
    let color = buffer_color(&raw.pixel_buffer.0);
    let surface = CVPixelBufferGetIOSurface(Some(&raw.pixel_buffer.0))
        .expect("capture pixel buffers are IOSurface-backed");
    // SAFETY: `surface` is a live IOSurface retained for this call, and the
    // import takes its own reference on it.
    let surface =
        unsafe { Ycbcr420IoSurfaceFrame::retain(NonNull::from(&*surface).cast::<c_void>()) }
            .with_owner(raw.pixel_buffer.clone());
    let luma = surface.import(device, Ycbcr420Plane::Luma);
    let chroma = surface.import(device, Ycbcr420Plane::Chroma);
    let (width, height) = (
        surface.width(Ycbcr420Plane::Luma),
        surface.height(Ycbcr420Plane::Luma),
    );
    let planes = FramePlanes::YCbCr420 {
        luma: luma.create_view(&wgpu::TextureViewDescriptor::default()),
        chroma: chroma.create_view(&wgpu::TextureViewDescriptor::default()),
    };
    let mut frame = Frame::new(
        planes,
        color,
        width,
        height,
        orientation_from_rotation(raw.rotation_degrees, raw.mirrored),
        raw.timestamp,
    );
    frame.pixel_buffer = Some(raw.pixel_buffer);
    frame
}

/// Builds an analysis frame over `raw`'s pixel buffer: no capture output of
/// its own, no pixel copy — the buffer's `IOSurface` reads on the CPU, and
/// the frame locks it read-only for its life.
///
/// # Panics
///
/// Panics under the same conditions as [`build_frame`]: the buffer lacks an
/// `IOSurface`, or carries no supported color attachments — all platform
/// defects for a `420f`/`420v` capture output.
pub fn build_analysis_frame(raw: RawFrame) -> AnalysisFrame {
    let color = buffer_color(&raw.pixel_buffer.0);
    let width = objc2_core_video::CVPixelBufferGetWidth(&raw.pixel_buffer.0);
    let height = objc2_core_video::CVPixelBufferGetHeight(&raw.pixel_buffer.0);
    AnalysisFrame::apple(
        raw.pixel_buffer,
        u32::try_from(width).expect("a capture buffer's width fits u32"),
        u32::try_from(height).expect("a capture buffer's height fits u32"),
        orientation_from_rotation(raw.rotation_degrees, raw.mirrored),
        color,
        raw.timestamp,
    )
}

/// The buffer's color description: matrix, primaries and transfer from its
/// attachments, and range from its `IOSurface`'s pixel format.
///
/// # Panics
///
/// Panics when the buffer has no `IOSurface` or carries unsupported
/// attachments — a platform defect for the capture output's `420f`/`420v`.
fn buffer_color(pixel_buffer: &CVPixelBuffer) -> VideoColorInfo {
    let surface = CVPixelBufferGetIOSurface(Some(pixel_buffer))
        .expect("capture pixel buffers are IOSurface-backed");
    // SAFETY: `surface` is a live IOSurface retained for this call; the
    // frame is only queried for its format.
    let surface =
        unsafe { Ycbcr420IoSurfaceFrame::retain(NonNull::from(&*surface).cast::<c_void>()) };
    VideoColorInfo {
        matrix: ycbcr_matrix(pixel_buffer),
        primaries: color_primaries(pixel_buffer),
        transfer: transfer_function(pixel_buffer),
        range: from_ycbcr_range(surface.format().range),
        content_light_level: None,
        dolby_vision: false,
    }
}

/// The matrix named by the buffer's `kCVImageBufferYCbCrMatrixKey`
/// attachment, which `AVFoundation` sets on every YCbCr capture buffer.
fn ycbcr_matrix(pixel_buffer: &CVPixelBuffer) -> MatrixCoefficients {
    // SAFETY: the keys and values are Core Video's own immutable constants.
    let (key, bt601, bt709, bt2020) = unsafe {
        (
            kCVImageBufferYCbCrMatrixKey,
            kCVImageBufferYCbCrMatrix_ITU_R_601_4,
            kCVImageBufferYCbCrMatrix_ITU_R_709_2,
            kCVImageBufferYCbCrMatrix_ITU_R_2020,
        )
    };
    let attachment = string_attachment(pixel_buffer, key, "kCVImageBufferYCbCrMatrixKey");
    let name = attachment.downcast_ref::<CFString>().unwrap_or_else(|| {
        panic!("kCVImageBufferYCbCrMatrixKey attachment is not a string: {attachment:?}")
    });
    if name == bt601 {
        from_ycbcr_matrix(YcbcrMatrix::Bt601)
    } else if name == bt709 {
        from_ycbcr_matrix(YcbcrMatrix::Bt709)
    } else if name == bt2020 {
        from_ycbcr_matrix(YcbcrMatrix::Bt2020)
    } else {
        panic!("unsupported kCVImageBufferYCbCrMatrixKey value {name}")
    }
}

fn color_primaries(pixel_buffer: &CVPixelBuffer) -> ColorPrimaries {
    // SAFETY: the keys and values are Core Video's own immutable constants.
    let (key, bt709, smpte_c, ebu_3213, p3_d65, bt2020) = unsafe {
        (
            kCVImageBufferColorPrimariesKey,
            kCVImageBufferColorPrimaries_ITU_R_709_2,
            kCVImageBufferColorPrimaries_SMPTE_C,
            kCVImageBufferColorPrimaries_EBU_3213,
            kCVImageBufferColorPrimaries_P3_D65,
            kCVImageBufferColorPrimaries_ITU_R_2020,
        )
    };
    let attachment = string_attachment(pixel_buffer, key, "kCVImageBufferColorPrimariesKey");
    let name = attachment.downcast_ref::<CFString>().unwrap_or_else(|| {
        panic!("kCVImageBufferColorPrimariesKey attachment is not a string: {attachment:?}")
    });
    if name == bt709 {
        ColorPrimaries::Bt709
    } else if name == smpte_c || name == ebu_3213 {
        ColorPrimaries::Bt601
    } else if name == p3_d65 {
        ColorPrimaries::DisplayP3
    } else if name == bt2020 {
        ColorPrimaries::Bt2020
    } else {
        panic!("unsupported kCVImageBufferColorPrimariesKey value {name}")
    }
}

fn transfer_function(pixel_buffer: &CVPixelBuffer) -> TransferFunction {
    // SAFETY: the keys and values are Core Video's own immutable constants.
    let (key, bt709, smpte_240m, bt2020, srgb, pq, hlg) = unsafe {
        (
            kCVImageBufferTransferFunctionKey,
            kCVImageBufferTransferFunction_ITU_R_709_2,
            kCVImageBufferTransferFunction_SMPTE_240M_1995,
            kCVImageBufferTransferFunction_ITU_R_2020,
            kCVImageBufferTransferFunction_sRGB,
            kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ,
            kCVImageBufferTransferFunction_ITU_R_2100_HLG,
        )
    };
    let attachment = string_attachment(pixel_buffer, key, "kCVImageBufferTransferFunctionKey");
    let name = attachment.downcast_ref::<CFString>().unwrap_or_else(|| {
        panic!("kCVImageBufferTransferFunctionKey attachment is not a string: {attachment:?}")
    });
    if name == bt709 || name == smpte_240m || name == bt2020 || name == srgb || is_smpte_c(name) {
        TransferFunction::Sdr
    } else if name == pq {
        TransferFunction::Pq
    } else if name == hlg {
        TransferFunction::Hlg
    } else {
        panic!("unsupported kCVImageBufferTransferFunctionKey value {name}")
    }
}

/// Whether `name` is the SMPTE C transfer, an SDR curve.
///
/// Only macOS exports the deprecated `kCVImageBufferTransferFunction_SMPTE_C`;
/// iOS has no such symbol, so no iOS buffer carries it.
#[cfg(target_os = "macos")]
#[expect(
    deprecated,
    reason = "Core Video deprecated the SMPTE C transfer name, but macOS buffers may still carry it"
)]
fn is_smpte_c(name: &CFString) -> bool {
    // SAFETY: the value is Core Video's own immutable constant.
    name == unsafe { objc2_core_video::kCVImageBufferTransferFunction_SMPTE_C }
}

/// Whether `name` is the SMPTE C transfer, an SDR curve.
///
/// iOS exports no SMPTE C transfer name, so no iOS buffer carries it.
#[cfg(not(target_os = "macos"))]
const fn is_smpte_c(_name: &CFString) -> bool {
    false
}

#[expect(
    deprecated,
    reason = "CVBufferCopyAttachment needs iOS 15, above this crate's iOS 14 deployment target"
)]
fn string_attachment(
    pixel_buffer: &CVPixelBuffer,
    key: &CFString,
    attachment_name: &str,
) -> CFRetained<CFType> {
    // SAFETY: a null attachment mode pointer is allowed.
    unsafe { pixel_buffer.get_attachment(key, std::ptr::null_mut()) }
        .unwrap_or_else(|| panic!("capture pixel buffers carry no {attachment_name} attachment"))
}

#[cfg(test)]
mod tests {
    use std::ptr::NonNull;
    use std::time::Duration;

    use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};
    use objc2_core_video::{
        CVAttachmentMode, CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferPool, CVPixelBufferUnlockBaseAddress,
        kCVImageBufferColorPrimaries_ITU_R_709_2, kCVImageBufferColorPrimariesKey,
        kCVImageBufferTransferFunction_ITU_R_709_2, kCVImageBufferTransferFunctionKey,
        kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
        kCVImageBufferYCbCrMatrixKey, kCVPixelBufferHeightKey,
        kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferPixelFormatTypeKey,
        kCVPixelBufferPoolAllocationThresholdKey, kCVPixelBufferWidthKey,
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVReturnSuccess,
        kCVReturnWouldExceedAllocationThreshold,
    };

    use super::{CapturedPixelBuffer, RawFrame, build_frame};
    use crate::test_support::{gpu, read_texture, wait_idle};
    use crate::{
        ColorPrimaries, ColorRange, FrameConverter, FramePlanes, MatrixCoefficients, Orientation,
        TransferFunction, VideoColorInfo,
    };

    const WIDTH: usize = 64;
    const HEIGHT: usize = 48;

    fn luma(x: usize, y: usize) -> u8 {
        u8::try_from((x * 3 + y * 5) % 256).expect("below 256")
    }

    fn chroma(cx: usize, cy: usize) -> [u8; 2] {
        [
            u8::try_from((cx * 7 + cy * 2 + 40) % 256).expect("below 256"),
            u8::try_from((cx * 2 + cy * 11 + 90) % 256).expect("below 256"),
        ]
    }

    /// An IOSurface-backed 4:2:0 pixel buffer with known samples and color
    /// attachments, as the capture output delivers.
    fn capture_buffer(pixel_format: u32, matrix: &CFString) -> CFRetained<CVPixelBuffer> {
        // SAFETY: Core Video's immutable key constant.
        let io_surface_key = unsafe { kCVPixelBufferIOSurfacePropertiesKey };
        let no_properties = CFDictionary::<CFString, CFType>::empty();
        let attributes = CFDictionary::<CFString, CFType>::from_slices(
            &[io_surface_key],
            &[no_properties.as_ref()],
        );
        let mut buffer: *mut CVPixelBuffer = std::ptr::null_mut();
        // SAFETY: the out pointer is valid for the call, and a created buffer
        // carries one reference the `CFRetained` below takes over.
        let buffer = unsafe {
            let status = CVPixelBufferCreate(
                None,
                WIDTH,
                HEIGHT,
                pixel_format,
                Some(attributes.as_opaque()),
                NonNull::from(&mut buffer),
            );
            assert_eq!(status, 0, "CVPixelBufferCreate failed");
            CFRetained::from_raw(NonNull::new(buffer).expect("a created pixel buffer"))
        };

        // SAFETY: the buffer is live and unlocked.
        let locked = unsafe { CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags(0)) };
        assert_eq!(locked, 0, "locking the test buffer");
        for (plane, rows, row_bytes) in [(0, HEIGHT, WIDTH), (1, HEIGHT / 2, WIDTH)] {
            let base = CVPixelBufferGetBaseAddressOfPlane(&buffer, plane).cast::<u8>();
            let stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, plane);
            for row in 0..rows {
                // SAFETY: the plane is locked and holds `rows` rows of
                // `stride >= row_bytes` bytes.
                let line =
                    unsafe { std::slice::from_raw_parts_mut(base.add(row * stride), row_bytes) };
                for (column, byte) in line.iter_mut().enumerate() {
                    *byte = if plane == 0 {
                        luma(column, row)
                    } else {
                        chroma(column / 2, row)[column % 2]
                    };
                }
            }
        }
        // SAFETY: the buffer was locked above with the same flags.
        let unlocked =
            unsafe { CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0)) };
        assert_eq!(unlocked, 0, "unlocking the test buffer");

        tag_sdr_capture_colour(&buffer, matrix);
        buffer
    }

    /// Attaches `matrix` with BT.709 primaries and transfer, the colour
    /// attachments `AVFoundation` puts on every SDR capture buffer.
    fn tag_sdr_capture_colour(buffer: &CVPixelBuffer, matrix: &CFString) {
        // SAFETY: the keys and values are Core Video's immutable
        // Core Foundation strings, the types the attachments carry.
        unsafe {
            for (key, value) in [
                (kCVImageBufferYCbCrMatrixKey, matrix),
                (
                    kCVImageBufferColorPrimariesKey,
                    kCVImageBufferColorPrimaries_ITU_R_709_2,
                ),
                (
                    kCVImageBufferTransferFunctionKey,
                    kCVImageBufferTransferFunction_ITU_R_709_2,
                ),
            ] {
                buffer.set_attachment(key, value, CVAttachmentMode::ShouldPropagate);
            }
        }
    }

    /// A real `420v` or `420f` capture buffer becomes a frame whose planes
    /// are its `IOSurface` planes, with the buffer's range and matrix, and the
    /// buffer is released once the frame drops and the GPU is done.
    #[test]
    fn capture_buffers_import_as_ycbcr420_planes() {
        let (device, queue) = gpu(wgpu::Features::empty());
        // SAFETY: Core Video's immutable value constants.
        let (bt601, bt709) = unsafe {
            (
                kCVImageBufferYCbCrMatrix_ITU_R_601_4,
                kCVImageBufferYCbCrMatrix_ITU_R_709_2,
            )
        };
        let expected_luma: Vec<u8> = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| luma(x, y)))
            .collect();
        let expected_chroma: Vec<u8> = (0..HEIGHT / 2)
            .flat_map(|cy| (0..WIDTH / 2).flat_map(move |cx| chroma(cx, cy)))
            .collect();

        for (pixel_format, range, matrix_name, matrix) in [
            (
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                ColorRange::Limited,
                bt709,
                MatrixCoefficients::Bt709,
            ),
            (
                kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                ColorRange::Full,
                bt601,
                MatrixCoefficients::Bt601,
            ),
        ] {
            let buffer = capture_buffer(pixel_format, matrix_name);
            let baseline = buffer.retain_count();
            let frame = build_frame(
                &device,
                RawFrame {
                    pixel_buffer: CapturedPixelBuffer(buffer.clone()),
                    timestamp: Duration::from_millis(5),
                    rotation_degrees: 90,
                    mirrored: false,
                },
            );
            assert!(
                buffer.retain_count() > baseline,
                "the frame holds the buffer"
            );

            assert_eq!((frame.width(), frame.height()), (64, 48));
            assert_eq!(frame.orientation(), Orientation::Right);
            assert_eq!(frame.timestamp(), Duration::from_millis(5));
            let FramePlanes::YCbCr420 {
                luma: luma_view,
                chroma: chroma_view,
            } = frame.planes()
            else {
                panic!("a 4:2:0 capture buffer imports as YCbCr420");
            };
            assert_eq!(
                frame.color(),
                VideoColorInfo {
                    matrix,
                    primaries: ColorPrimaries::Bt709,
                    transfer: TransferFunction::Sdr,
                    range,
                    content_light_level: None,
                    dolby_vision: false,
                }
            );
            let (luma_texture, chroma_texture) = (luma_view.texture(), chroma_view.texture());
            assert_eq!(luma_texture.format(), wgpu::TextureFormat::R8Unorm);
            assert_eq!((luma_texture.width(), luma_texture.height()), (64, 48));
            assert_eq!(chroma_texture.format(), wgpu::TextureFormat::Rg8Unorm);
            assert_eq!((chroma_texture.width(), chroma_texture.height()), (32, 24));
            // The textures alias the surface: they read back exactly what was
            // written into the pixel buffer, which no code path copied.
            assert_eq!(read_texture(&device, &queue, luma_texture), expected_luma);
            assert_eq!(
                read_texture(&device, &queue, chroma_texture),
                expected_chroma
            );

            drop(frame);
            wait_idle(&device);
            assert_eq!(
                buffer.retain_count(),
                baseline,
                "the buffer goes back once the frame drops and the GPU is done"
            );
        }
    }

    /// How many buffers the test pool lends at once, like the capture
    /// output's small pool.
    const POOL_BUFFERS: usize = 2;

    /// A pool of IOSurface-backed `420v` buffers that refuses a buffer while
    /// [`POOL_BUFFERS`] are lent out.
    struct CapturePool {
        pool: CFRetained<CVPixelBufferPool>,
        threshold: CFRetained<CFDictionary<CFString, CFType>>,
    }

    impl CapturePool {
        fn new() -> Self {
            // SAFETY: Core Video's immutable key constants.
            let keys = unsafe {
                [
                    kCVPixelBufferPixelFormatTypeKey,
                    kCVPixelBufferWidthKey,
                    kCVPixelBufferHeightKey,
                    kCVPixelBufferIOSurfacePropertiesKey,
                ]
            };
            let format = CFNumber::new_i32(
                i32::try_from(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)
                    .expect("a four-character code fits i32"),
            );
            let width = CFNumber::new_i32(i32::try_from(WIDTH).expect("small"));
            let height = CFNumber::new_i32(i32::try_from(HEIGHT).expect("small"));
            let no_properties = CFDictionary::<CFString, CFType>::empty();
            let attributes = CFDictionary::<CFString, CFType>::from_slices(
                &keys,
                &[
                    format.as_ref(),
                    width.as_ref(),
                    height.as_ref(),
                    no_properties.as_ref(),
                ],
            );
            let mut pool: *mut CVPixelBufferPool = std::ptr::null_mut();
            // SAFETY: the out pointer is valid for the call, and a created
            // pool carries one reference the `CFRetained` below takes over.
            let pool = unsafe {
                let status = CVPixelBufferPool::create(
                    None,
                    None,
                    Some(attributes.as_opaque()),
                    NonNull::from(&mut pool),
                );
                assert_eq!(status, kCVReturnSuccess, "CVPixelBufferPoolCreate failed");
                CFRetained::from_raw(NonNull::new(pool).expect("a created pool"))
            };
            // SAFETY: Core Video's immutable key constant.
            let threshold_key = unsafe { kCVPixelBufferPoolAllocationThresholdKey };
            let limit = CFNumber::new_i32(i32::try_from(POOL_BUFFERS).expect("small"));
            let threshold =
                CFDictionary::<CFString, CFType>::from_slices(&[threshold_key], &[limit.as_ref()]);
            Self { pool, threshold }
        }

        /// The next free buffer, tagged BT.709 SDR as a capture buffer is, or
        /// `None` while every buffer is lent out.
        fn take(&self) -> Option<CFRetained<CVPixelBuffer>> {
            let mut buffer: *mut CVPixelBuffer = std::ptr::null_mut();
            // SAFETY: the out pointer is valid for the call, and a created
            // buffer carries one reference the `CFRetained` below takes over.
            let status = unsafe {
                CVPixelBufferPool::create_pixel_buffer_with_aux_attributes(
                    None,
                    &self.pool,
                    Some(self.threshold.as_opaque()),
                    NonNull::from(&mut buffer),
                )
            };
            if status == kCVReturnWouldExceedAllocationThreshold {
                return None;
            }
            assert_eq!(status, kCVReturnSuccess, "taking a pool buffer failed");
            // SAFETY: as above.
            let buffer = unsafe {
                CFRetained::from_raw(NonNull::new(buffer).expect("a created pixel buffer"))
            };
            // SAFETY: Core Video's immutable constant.
            tag_sdr_capture_colour(&buffer, unsafe { kCVImageBufferYCbCrMatrix_ITU_R_709_2 });
            Some(buffer)
        }

        /// How many buffers can be taken right now, up to [`POOL_BUFFERS`].
        fn free(&self) -> usize {
            // Each buffer is held until all are taken: one returned at once
            // would be taken again.
            let mut held = Vec::new();
            while let Some(buffer) = self.take() {
                held.push(buffer);
            }
            held.len()
        }
    }

    fn frame_from(device: &wgpu::Device, buffer: CFRetained<CVPixelBuffer>) -> crate::Frame {
        build_frame(
            device,
            RawFrame {
                pixel_buffer: CapturedPixelBuffer(buffer),
                timestamp: Duration::ZERO,
                rotation_degrees: 0,
                mirrored: false,
            },
        )
    }

    /// Frames dropped without any GPU work give their buffers straight back,
    /// so a consumer that skips frames never starves the capture pool, and a
    /// frame whose conversion is recorded keeps its buffer until that work
    /// has been submitted and has completed.
    #[test]
    fn capture_buffers_return_to_the_pool_once_the_gpu_is_done() {
        let (device, queue) = gpu(wgpu::Features::empty());
        let pool = CapturePool::new();

        // Held frames keep their buffers out, and the pool runs dry.
        let held: Vec<_> = (0..POOL_BUFFERS)
            .map(|_| frame_from(&device, pool.take().expect("a free buffer")))
            .collect();
        assert_eq!(pool.free(), 0, "every buffer is lent to a held frame");
        drop(held);
        assert_eq!(
            pool.free(),
            POOL_BUFFERS,
            "dropped frames return their buffers"
        );

        // Many more frames than the pool holds, each dropped untouched with
        // no submission at all.
        for taken in 0..POOL_BUFFERS * 4 {
            let buffer = pool.take().unwrap_or_else(|| {
                panic!("frame {taken} starved: a dropped frame kept its buffer")
            });
            drop(frame_from(&device, buffer));
        }
        assert_eq!(pool.free(), POOL_BUFFERS);

        // Skip frames, then convert the last one: the conversion recorded
        // before the frame drops holds its buffer until the GPU finishes it.
        let mut last = None;
        for _ in 0..5 {
            last = Some(frame_from(&device, pool.take().expect("a free buffer")));
        }
        let last = last.expect("five frames were taken");
        let mut converter = FrameConverter::new(&device);
        let upright = FrameConverter::create_output(&device, &last);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        converter
            .encode(&device, &mut encoder, &last, &upright)
            .expect("test frame has supported color");
        drop(last);
        assert_eq!(
            pool.free(),
            POOL_BUFFERS - 1,
            "recorded work keeps the converted frame's buffer"
        );
        queue.submit([encoder.finish()]);
        wait_idle(&device);
        assert_eq!(
            pool.free(),
            POOL_BUFFERS,
            "the buffer returns once the conversion has completed"
        );
    }
}
