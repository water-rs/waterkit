//! CPU-readable `Y'CbCr` 4:2:0 frames for image analysis.
//!
//! [`Camera::analysis_frames`] delivers [`AnalysisFrame`]s when the camera
//! was opened with [`CameraConfig::analysis`]. They are the stream image
//! analysis runs on: where [`Frame`] keeps its pixels in GPU storage that a
//! consumer may not be able to read on the CPU at all — on Android the
//! preview buffers are GPU-sampled `AHardwareBuffer`s — an analysis frame's
//! pixels are always CPU-readable through [`AnalysisFrame::planes`].
//!
//! What the platform payload is:
//!
//! - **Android**: the acquired `android.media.Image` (held as a JNI global,
//!   exposed through `AnalysisFrame::media_image`), closed when the last
//!   clone drops.
//! - **iOS / macOS**: the `CVPixelBuffer` the capture output already
//!   produced, retained (and CPU-locked) for the frame's life — no second
//!   capture output is opened.
//! - **Windows / Linux**: the pixels the capture thread delivered, before
//!   upload.
//!
//! [`Camera::analysis_frames`]: crate::Camera::analysis_frames
//! [`CameraConfig::analysis`]: crate::CameraConfig::analysis

#[cfg(any(target_os = "ios", target_os = "macos"))]
use std::fmt;
#[cfg(any(
    target_os = "android",
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "linux"
))]
use std::sync::Arc;
use std::time::Duration;

use crate::frame::Orientation;
use crate::{CameraError, Resolution, VideoColorInfo};

/// Configuration for the camera's analysis output.
///
/// Analysis is opt-in: [`CameraConfig::analysis`] is `None` by default and
/// a camera opened without it answers
/// [`CameraError::AnalysisNotConfigured`](crate::CameraError::AnalysisNotConfigured)
/// from [`Camera::analysis_frames`](crate::Camera::analysis_frames).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnalysisConfig {
    /// Requested analysis resolution, which may be smaller than the
    /// preview's. The delivered size is the platform's: Android opens a
    /// second `YUV_420_888` stream at the nearest supported size, while
    /// Apple and the desktop backends deliver frames at the capture
    /// resolution.
    pub resolution: Resolution,
}

impl Default for AnalysisConfig {
    /// 1280×720 ([`Resolution::HD`]).
    fn default() -> Self {
        Self {
            resolution: Resolution::HD,
        }
    }
}

/// One plane of an [`AnalysisFrame`]'s samples, CPU-readable.
///
/// `bytes` covers the plane's mapping: reading the sample at `(x, y)` is
/// `bytes[y * row_stride + x * pixel_stride]`. Chroma planes are
/// quarter-resolution (each dimension halved, rounded up); their
/// `pixel_stride` is 2 when `Cb` and `Cr` share storage, as in NV12 and most
/// `YUV_420_888` images.
#[derive(Debug, Clone, Copy)]
pub struct AnalysisPlane<'a> {
    bytes: &'a [u8],
    row_stride: usize,
    pixel_stride: usize,
}

/// Only a platform's analysis frames build planes.
#[cfg(any(
    target_os = "android",
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "linux"
))]
impl<'a> AnalysisPlane<'a> {
    pub(crate) const fn new(bytes: &'a [u8], row_stride: usize, pixel_stride: usize) -> Self {
        Self {
            bytes,
            row_stride,
            pixel_stride,
        }
    }
}

impl AnalysisPlane<'_> {
    /// The plane's mapped bytes, rows `row_stride` apart.
    #[must_use]
    pub const fn bytes(&self) -> &[u8] {
        self.bytes
    }

    /// Bytes from one row's start to the next row's.
    #[must_use]
    pub const fn row_stride(&self) -> usize {
        self.row_stride
    }

    /// Bytes between horizontally adjacent samples in a row.
    #[must_use]
    pub const fn pixel_stride(&self) -> usize {
        self.pixel_stride
    }
}

/// The three CPU planes of an [`AnalysisFrame`]: full-resolution luma and
/// quarter-resolution `Cb` and `Cr`.
#[derive(Debug, Clone, Copy)]
pub struct AnalysisPlanes<'a> {
    /// The full-resolution luma (`Y'`) plane; `pixel_stride` is 1.
    pub luma: AnalysisPlane<'a>,
    /// The quarter-resolution `Cb` plane.
    pub cb: AnalysisPlane<'a>,
    /// The quarter-resolution `Cr` plane.
    pub cr: AnalysisPlane<'a>,
}

/// A CPU-readable `Y'CbCr` 4:2:0 camera frame for image analysis.
///
/// The frame's samples read through [`Self::planes`]; its width, height,
/// [`Orientation`], [`VideoColorInfo`] and timestamp follow [`Frame`]'s
/// contract — the timestamp is measured on the platform's capture clock from
/// the first analysis frame the camera delivered.
///
/// Analysis frames are cheap to clone: every platform payload is a shared
/// handle. On Android dropping the last clone closes the
/// `android.media.Image`, returning it to the `ImageReader`; on Apple
/// platforms it releases the `CVPixelBuffer` back to the capture pool.
/// A consumer that holds frames empties that pool the same way it does for
/// [`Frame`]: take each frame, read it, drop it.
#[derive(Debug, Clone)]
pub struct AnalysisFrame {
    width: u32,
    height: u32,
    timestamp: Duration,
    orientation: Orientation,
    color: VideoColorInfo,
    /// The acquired `android.media.Image`; closed when the last clone drops.
    #[cfg(target_os = "android")]
    image: Arc<crate::sys::android::AnalysisImage>,
    /// The capture output's `CVPixelBuffer`, locked read-only for the
    /// frame's life so the CPU can read its planes.
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    pixel_buffer: Arc<LockedPixelBuffer>,
    /// NV12 pixels captured before upload, tightly packed.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pixels: Arc<Vec<u8>>,
}

impl AnalysisFrame {
    /// The `android.media.Image` backing this frame, a `YUV_420_888` image
    /// acquired from the analysis `ImageReader`.
    ///
    /// The image stays acquired for the frame's life; dropping the last
    /// frame clone closes it. Passing it to ML Kit's
    /// `InputImage.fromMediaImage` needs no pixel copy.
    #[cfg(target_os = "android")]
    #[must_use]
    pub fn media_image(&self) -> &jni::objects::Global<jni::objects::JObject<'static>> {
        self.image.image()
    }

    /// The `CVPixelBuffer` the capture output delivered, which the frame
    /// retains.
    ///
    /// Retaining the buffer costs no pixel copy: Apple's capture buffers are
    /// `IOSurface`-backed and read by CPU and GPU alike.
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    #[must_use]
    pub fn pixel_buffer(
        &self,
    ) -> objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer> {
        self.pixel_buffer.buffer()
    }

    /// The frame's `Y'CbCr` 4:2:0 samples as three CPU planes: luma, `Cb`,
    /// `Cr`.
    ///
    /// The returned slices borrow the frame: its pixels stay mapped for the
    /// frame's life, so the borrow checker keeps them valid.
    #[must_use]
    pub fn planes(&self) -> AnalysisPlanes<'_> {
        #[cfg(target_os = "android")]
        {
            self.image.planes()
        }
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            self.pixel_buffer.planes()
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            nv12_planes(&self.pixels, self.width, self.height)
        }
        #[cfg(not(any(
            target_os = "android",
            target_os = "ios",
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        )))]
        {
            unreachable!("analysis frames exist only on platforms with a camera backend")
        }
    }

    /// Stored width in pixels, before [`Self::orientation`] is applied.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Stored height in pixels, before [`Self::orientation`] is applied.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// When the frame was captured, on the same capture clock
    /// [`Frame::timestamp`](crate::Frame::timestamp) reads: measured from the
    /// first analysis frame delivered since the camera opened. The first
    /// analysis frame reads [`Duration::ZERO`].
    #[must_use]
    pub const fn timestamp(&self) -> Duration {
        self.timestamp
    }

    /// How the stored pixels relate to upright.
    #[must_use]
    pub const fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// The color description of the frame's `Y'CbCr` samples: `matrix` and
    /// `range` decode them; `primaries` and `transfer` describe them.
    #[must_use]
    pub const fn color(&self) -> VideoColorInfo {
        self.color
    }
}

impl AnalysisFrame {
    /// Builds a frame over the Android analysis image.
    #[cfg(target_os = "android")]
    pub(crate) fn android(
        image: crate::sys::android::AnalysisImage,
        orientation: Orientation,
        color: VideoColorInfo,
        timestamp: Duration,
    ) -> Self {
        let (width, height) = (image.width(), image.height());
        Self {
            width,
            height,
            timestamp,
            orientation,
            color,
            image: Arc::new(image),
        }
    }

    /// Builds a frame over the capture output's `CVPixelBuffer`, locking it
    /// read-only for the frame's life.
    ///
    /// # Panics
    ///
    /// Panics when the buffer cannot be locked; the buffer comes from the
    /// camera's own capture pool, so a lock failure is a platform defect.
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    pub(crate) fn apple(
        pixel_buffer: crate::sys::apple::CapturedPixelBuffer,
        width: u32,
        height: u32,
        orientation: Orientation,
        color: VideoColorInfo,
        timestamp: Duration,
    ) -> Self {
        Self {
            width,
            height,
            timestamp,
            orientation,
            color,
            pixel_buffer: Arc::new(LockedPixelBuffer::new(pixel_buffer)),
        }
    }

    /// Builds a frame over tightly packed NV12 pixels captured before
    /// upload.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn nv12(
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        color: VideoColorInfo,
        timestamp: Duration,
    ) -> Self {
        debug_assert_eq!(
            pixels.len(),
            crate::upload::nv12_len(width, height),
            "an analysis frame's pixels are NV12"
        );
        Self {
            width,
            height,
            timestamp,
            orientation: Orientation::Up,
            color,
            pixels: Arc::new(pixels),
        }
    }
}

/// A `CVPixelBuffer` locked read-only while the frame holding it lives.
#[cfg(any(target_os = "ios", target_os = "macos"))]
struct LockedPixelBuffer {
    buffer: crate::sys::apple::CapturedPixelBuffer,
}

// SAFETY: Core Foundation reference counting is thread-safe, the buffer is
// locked read-only for the frame's life and `planes` only reads its mapped
// memory — sharing it between threads is safe.
#[cfg(any(target_os = "ios", target_os = "macos"))]
unsafe impl Send for LockedPixelBuffer {}
// SAFETY: as above.
#[cfg(any(target_os = "ios", target_os = "macos"))]
unsafe impl Sync for LockedPixelBuffer {}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl LockedPixelBuffer {
    fn new(buffer: crate::sys::apple::CapturedPixelBuffer) -> Self {
        // SAFETY: the buffer is live and not locked.
        let status = unsafe {
            objc2_core_video::CVPixelBufferLockBaseAddress(
                &buffer.0,
                objc2_core_video::CVPixelBufferLockFlags::ReadOnly,
            )
        };
        assert_eq!(
            status,
            objc2_core_video::kCVReturnSuccess,
            "a capture buffer's pixels could not be locked for reading"
        );
        Self { buffer }
    }

    fn buffer(&self) -> objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer> {
        self.buffer.0.clone()
    }

    /// The locked buffer's luma and interleaved chroma planes as `Y'`, `Cb`,
    /// `Cr` views; the chroma `pixel_stride` is 2.
    fn planes(&self) -> AnalysisPlanes<'_> {
        use objc2_core_video::{
            CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
            CVPixelBufferGetHeightOfPlane, CVPixelBufferGetWidthOfPlane,
        };
        let buffer = &*self.buffer.0;
        // SAFETY: the buffer is locked for `self`'s borrow, so each plane's
        // base address is valid for its mapped extent.
        unsafe {
            let luma_width = CVPixelBufferGetWidthOfPlane(buffer, 0);
            let luma_height = CVPixelBufferGetHeightOfPlane(buffer, 0);
            let luma_stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, 0);
            let luma = AnalysisPlane::new(
                std::slice::from_raw_parts(
                    CVPixelBufferGetBaseAddressOfPlane(buffer, 0).cast::<u8>(),
                    luma_stride * (luma_height - 1) + luma_width,
                ),
                luma_stride,
                1,
            );
            let chroma_width = CVPixelBufferGetWidthOfPlane(buffer, 1);
            let chroma_height = CVPixelBufferGetHeightOfPlane(buffer, 1);
            let chroma_stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, 1);
            let chroma_base = CVPixelBufferGetBaseAddressOfPlane(buffer, 1).cast::<u8>();
            let chroma_len = chroma_stride * (chroma_height - 1) + chroma_width * 2;
            AnalysisPlanes {
                luma,
                cb: AnalysisPlane::new(
                    std::slice::from_raw_parts(chroma_base, chroma_len - 1),
                    chroma_stride,
                    2,
                ),
                cr: AnalysisPlane::new(
                    std::slice::from_raw_parts(chroma_base.add(1), chroma_len - 1),
                    chroma_stride,
                    2,
                ),
            }
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl Drop for LockedPixelBuffer {
    fn drop(&mut self) {
        // SAFETY: the buffer was locked `ReadOnly` at construction and is
        // unlocked once, here, with the same flags.
        unsafe {
            objc2_core_video::CVPixelBufferUnlockBaseAddress(
                &self.buffer.0,
                objc2_core_video::CVPixelBufferLockFlags::ReadOnly,
            );
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl fmt::Debug for LockedPixelBuffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LockedPixelBuffer")
            .finish_non_exhaustive()
    }
}

/// Tightly packed NV12 pixels as `Y'`, `Cb`, `Cr` planes: the chroma shares
/// one interleaved region, so each chroma plane's `pixel_stride` is 2 and
/// `Cb` starts one byte before `Cr`.
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn nv12_planes(pixels: &[u8], width: u32, height: u32) -> AnalysisPlanes<'_> {
    let (luma, chroma) = pixels.split_at(width as usize * height as usize);
    let chroma_width = width.div_ceil(2) as usize;
    AnalysisPlanes {
        luma: AnalysisPlane::new(luma, width as usize, 1),
        cb: AnalysisPlane::new(&chroma[..chroma.len() - 1], chroma_width * 2, 2),
        cr: AnalysisPlane::new(&chroma[1..], chroma_width * 2, 2),
    }
}

/// The shared `Camera::analysis_frames` stream shape.
///
/// `receiver` is `None` when the camera was opened without an
/// [`AnalysisConfig`]: the stream then yields
/// [`CameraError::AnalysisNotConfigured`] once and ends. Otherwise each raw
/// frame maps through `map`; the channels are newest-wins, so a consumer
/// that falls behind drops pending frames rather than the camera.
pub fn stream<T, F>(
    receiver: Option<async_channel::Receiver<T>>,
    map: F,
) -> impl futures::Stream<Item = Result<AnalysisFrame, CameraError>>
where
    F: FnMut(T) -> Result<AnalysisFrame, CameraError>,
{
    // The state is `None` once the stream ends: after the not-configured
    // error, and after the channel closes following the capture's teardown.
    futures::stream::unfold(Some((receiver, map)), |state| async move {
        let (receiver, mut map) = state?;
        match receiver {
            None => Some((Err(CameraError::AnalysisNotConfigured), None)),
            Some(receiver) => receiver.recv().await.ok().map(|raw| {
                let item = map(raw);
                (item, Some((Some(receiver), map)))
            }),
        }
    })
}
