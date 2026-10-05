//! Zero-copy frames from `AVFoundation`'s capture buffers.
//!
//! The capture output delivers biplanar 4:2:0 `CVPixelBuffer`s backed by an
//! `IOSurface`. Each frame imports the surface's luma and chroma planes as
//! textures that alias its memory, so no pixel is copied, and holds the
//! buffer until the frame drops and the GPU work submitted until then has
//! finished. Only then does the buffer go back to the capture pool.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;

use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetIOSurface, kCVImageBufferYCbCrMatrix_ITU_R_601_4,
    kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVImageBufferYCbCrMatrix_ITU_R_2020,
    kCVImageBufferYCbCrMatrixKey,
};
use wgpu_external_frame::io_surface::{Ycbcr420IoSurfaceFrame, Ycbcr420Plane, YcbcrRange};

use crate::frame::{Frame, FramePlanes, FrameStorage};
use crate::{Orientation, YCbCrEncoding, YCbCrMatrix, YCbCrRange};

/// A retained `CVPixelBuffer` that may cross threads.
///
/// Core Foundation's reference counting is thread-safe, and nothing here
/// reads or writes the buffer's pixels on the CPU: it is only queried for
/// immutable metadata and released.
#[derive(Debug)]
pub struct CapturedPixelBuffer(CFRetained<CVPixelBuffer>);

// SAFETY: see the type's documentation; the buffer is only retained, released
// and queried for immutable properties, all thread-safe in Core Video.
#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "the Core Foundation reference is exactly what this impl vouches for"
)]
unsafe impl Send for CapturedPixelBuffer {}
// SAFETY: as above; no method mutates the buffer.
unsafe impl Sync for CapturedPixelBuffer {}

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

/// The surface and buffer a captured frame's textures alias.
#[derive(Debug)]
struct HeldSurface {
    _surface: Ycbcr420IoSurfaceFrame,
    _pixel_buffer: CapturedPixelBuffer,
}

// SAFETY: the surface is only retained and released, which Core Foundation
// does thread-safely; the buffer is `Send` for the same reason.
#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "the retained IOSurface is exactly what this impl vouches for"
)]
unsafe impl Send for HeldSurface {}
// SAFETY: as above; `HeldSurface` exposes no access at all.
unsafe impl Sync for HeldSurface {}

/// Keeps a captured buffer out of the capture pool while a frame aliases it.
#[derive(Debug)]
pub struct CapturedBuffer {
    held: Option<HeldSurface>,
    queue: Arc<wgpu::Queue>,
}

impl Drop for CapturedBuffer {
    fn drop(&mut self) {
        if let Some(held) = self.held.take() {
            // Work already submitted may still read the planes; the buffer
            // goes back to the pool only once that work has finished.
            self.queue.on_submitted_work_done(move || drop(held));
        }
    }
}

/// Builds a frame whose planes alias `raw`'s `IOSurface`.
///
/// # Panics
///
/// Panics when the buffer has no `IOSurface`, is not biplanar 4:2:0 YCbCr,
/// or carries no YCbCr matrix this crate supports. The capture output is
/// configured for `420f` or `420v`, so each of these is a platform defect.
pub fn build_frame(device: &wgpu::Device, queue: &Arc<wgpu::Queue>, raw: RawFrame) -> Frame {
    let pixel_buffer = &raw.pixel_buffer.0;
    let surface = CVPixelBufferGetIOSurface(Some(pixel_buffer))
        .expect("capture pixel buffers are IOSurface-backed");
    // SAFETY: `surface` is a live IOSurface retained for this call, and the
    // frame takes its own reference on it.
    let surface =
        unsafe { Ycbcr420IoSurfaceFrame::retain(NonNull::from(&*surface).cast::<c_void>()) };
    let luma = surface.import(device, Ycbcr420Plane::Luma);
    let chroma = surface.import(device, Ycbcr420Plane::Chroma);
    let encoding = YCbCrEncoding {
        matrix: ycbcr_matrix(pixel_buffer),
        range: match surface.format().range {
            YcbcrRange::Video => YCbCrRange::Video,
            YcbcrRange::Full => YCbCrRange::Full,
        },
    };
    let (width, height) = (
        surface.width(Ycbcr420Plane::Luma),
        surface.height(Ycbcr420Plane::Luma),
    );
    let planes = FramePlanes::YCbCr420 {
        luma: luma.create_view(&wgpu::TextureViewDescriptor::default()),
        chroma: chroma.create_view(&wgpu::TextureViewDescriptor::default()),
        encoding,
    };
    let storage = FrameStorage::Captured {
        _buffer: CapturedBuffer {
            held: Some(HeldSurface {
                _surface: surface,
                _pixel_buffer: raw.pixel_buffer,
            }),
            queue: Arc::clone(queue),
        },
    };
    Frame::new(
        planes,
        storage,
        width,
        height,
        Orientation::from_rotation(raw.rotation_degrees, raw.mirrored),
        raw.timestamp,
    )
}

/// The matrix named by the buffer's `kCVImageBufferYCbCrMatrixKey`
/// attachment, which `AVFoundation` sets on every YCbCr capture buffer.
fn ycbcr_matrix(pixel_buffer: &CVPixelBuffer) -> YCbCrMatrix {
    // SAFETY: the keys and values are Core Video's own immutable constants.
    let (key, bt601, bt709, bt2020) = unsafe {
        (
            kCVImageBufferYCbCrMatrixKey,
            kCVImageBufferYCbCrMatrix_ITU_R_601_4,
            kCVImageBufferYCbCrMatrix_ITU_R_709_2,
            kCVImageBufferYCbCrMatrix_ITU_R_2020,
        )
    };
    #[expect(
        deprecated,
        reason = "CVBufferCopyAttachment needs iOS 15, above this crate's iOS 14 deployment target"
    )]
    // SAFETY: a null attachment mode pointer is allowed.
    let attachment = unsafe { pixel_buffer.get_attachment(key, std::ptr::null_mut()) }
        .expect("capture pixel buffers carry a YCbCr matrix attachment");
    let name = attachment
        .downcast_ref::<CFString>()
        .expect("the YCbCr matrix attachment is a string");
    if name == bt601 {
        YCbCrMatrix::Bt601
    } else if name == bt709 {
        YCbCrMatrix::Bt709
    } else if name == bt2020 {
        YCbCrMatrix::Bt2020
    } else {
        panic!("capture buffer uses the unsupported YCbCr matrix {name}")
    }
}

#[cfg(test)]
mod tests {
    use std::ptr::NonNull;
    use std::time::Duration;

    use objc2_core_foundation::{CFDictionary, CFRetained, CFString, CFType};
    use objc2_core_video::{
        CVAttachmentMode, CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVImageBufferYCbCrMatrix_ITU_R_601_4,
        kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVImageBufferYCbCrMatrixKey,
        kCVPixelBufferIOSurfacePropertiesKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    };

    use super::{CapturedPixelBuffer, RawFrame, build_frame};
    use crate::test_support::{gpu, read_texture, wait_idle};
    use crate::{FramePlanes, Orientation, YCbCrEncoding, YCbCrMatrix, YCbCrRange};

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

    /// An IOSurface-backed 4:2:0 pixel buffer with known samples and a YCbCr
    /// matrix attachment, as the capture output delivers.
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

        // SAFETY: the key and value are Core Foundation strings, the types the
        // attachment carries.
        unsafe {
            buffer.set_attachment(
                kCVImageBufferYCbCrMatrixKey,
                matrix,
                CVAttachmentMode::ShouldPropagate,
            );
        }
        buffer
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
                YCbCrRange::Video,
                bt709,
                YCbCrMatrix::Bt709,
            ),
            (
                kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                YCbCrRange::Full,
                bt601,
                YCbCrMatrix::Bt601,
            ),
        ] {
            let buffer = capture_buffer(pixel_format, matrix_name);
            let baseline = buffer.retain_count();
            let frame = build_frame(
                &device,
                &queue,
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
                encoding,
            } = frame.planes()
            else {
                panic!("a 4:2:0 capture buffer imports as YCbCr420");
            };
            assert_eq!(*encoding, YCbCrEncoding { matrix, range });
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
}
