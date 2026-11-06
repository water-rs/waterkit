//! Display capture delivering the platform's own pixel buffers.
//!
//! [`DisplayStream`] captures one display, picked by its platform id, and
//! hands every complete frame over as a [`PixelBuffer`]: on macOS the
//! `IOSurface`-backed `CVPixelBuffer` `ScreenCaptureKit` produced, which a
//! hardware encoder takes without a copy. Capture requires the macOS Screen
//! Recording privacy grant for the process responsible for the caller.
//!
//! The stream is only available on macOS; every other platform reports
//! [`Error::Unsupported`].

use core::num::NonZeroU32;
use core::time::Duration;

use futures::Stream;

use crate::{Error, sys};

/// Pixel layout of the buffers a [`DisplayStream`] delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PixelFormat {
    /// 8-bit blue, green, red and alpha, packed little-endian (`BGRA`).
    Bgra8,
    /// 10-bit blue, green and red with 2-bit alpha, packed little-endian
    /// (`l10r`).
    Bgr10a2,
    /// Bi-planar 4:2:0 `Y'CbCr` in video range (`420v`).
    Ycbcr420Video,
    /// Bi-planar 4:2:0 `Y'CbCr` in full range (`420f`).
    Ycbcr420Full,
}

/// The frame rate a [`DisplayStreamConfig`] starts with.
const DEFAULT_FRAME_RATE: NonZeroU32 = NonZeroU32::new(60).expect("60 is non-zero");

/// What a [`DisplayStream`] captures: output size, frame rate and pixel
/// format.
#[derive(Debug, Clone)]
pub struct DisplayStreamConfig {
    pub(crate) width: NonZeroU32,
    pub(crate) height: NonZeroU32,
    pub(crate) frame_rate: NonZeroU32,
    pub(crate) pixel_format: PixelFormat,
}

impl DisplayStreamConfig {
    /// Frames of `width`x`height` pixels, scaled from the display's content,
    /// at up to 60 per second in [`PixelFormat::Bgra8`].
    #[must_use]
    pub const fn new(width: NonZeroU32, height: NonZeroU32) -> Self {
        Self {
            width,
            height,
            frame_rate: DEFAULT_FRAME_RATE,
            pixel_format: PixelFormat::Bgra8,
        }
    }

    /// The most frames per second the stream delivers. A display whose
    /// content does not change delivers none.
    #[must_use]
    pub const fn frame_rate(mut self, frames_per_second: NonZeroU32) -> Self {
        self.frame_rate = frames_per_second;
        self
    }

    /// The pixel format of every delivered buffer.
    #[must_use]
    pub const fn pixel_format(mut self, format: PixelFormat) -> Self {
        self.pixel_format = format;
        self
    }
}

/// One captured frame, held by the caller until it drops it.
///
/// While held, the frame keeps its surface out of the capture's pool, so a
/// caller holding many frames stalls the stream until it releases some.
#[derive(Debug)]
pub struct PixelBuffer {
    inner: sys::PixelBufferInner,
}

impl PixelBuffer {
    #[cfg(target_os = "macos")]
    pub(crate) const fn new(inner: sys::PixelBufferInner) -> Self {
        Self { inner }
    }

    /// Width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.inner.width()
    }

    /// Height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.inner.height()
    }

    /// The buffer's pixel layout.
    #[must_use]
    pub fn pixel_format(&self) -> PixelFormat {
        self.inner.pixel_format()
    }

    /// When the frame was presented, on the host clock (on macOS,
    /// `mach_absolute_time` converted to nanoseconds).
    #[must_use]
    pub const fn presentation_time(&self) -> Duration {
        self.inner.presentation_time()
    }

    /// The `CVPixelBuffer` holding the frame, for an encoder that takes it
    /// as it is.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn cv_pixel_buffer(&self) -> &objc2_core_video::CVPixelBuffer {
        self.inner.cv_pixel_buffer()
    }

    /// The `IOSurface` backing [`Self::cv_pixel_buffer`].
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn io_surface(&self) -> &objc2_io_surface::IOSurfaceRef {
        self.inner.io_surface()
    }
}

/// A capture of one display, delivering [`PixelBuffer`]s.
///
/// The stream starts paused: [`resume`](Self::resume) starts capture, whose
/// first frame follows at once, and [`pause`](Self::pause) stops it.
/// Dropping the stream stops capture and ends [`frames`](Self::frames).
#[derive(Debug)]
pub struct DisplayStream {
    inner: sys::DisplayStreamInner,
}

impl DisplayStream {
    /// Prepares a paused capture of the display whose platform id is
    /// `display_id` (on macOS its `CGDirectDisplayID`, as
    /// [`ScreenInfo::id`](crate::ScreenInfo::id) reports it), including a
    /// virtual display the application created itself.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MonitorNotFound`] when no display has that id,
    /// [`Error::PermissionDenied`] without the Screen Recording permission,
    /// [`Error::Unsupported`] on platforms without display streams, and
    /// [`Error::Platform`] when the capture cannot be set up.
    pub async fn new(display_id: u32, config: DisplayStreamConfig) -> Result<Self, Error> {
        let inner = sys::DisplayStreamInner::new(display_id, config).await?;
        Ok(Self { inner })
    }

    /// Captured frames, oldest first. While frames wait unread, the stream
    /// keeps the most recent ones and drops older ones, so capture never
    /// stalls on a slow reader. Every call returns a receiver of the same
    /// frames; each frame reaches one of them. The stream ends when this
    /// `DisplayStream` drops or capture stops with an error.
    pub fn frames(&self) -> impl Stream<Item = PixelBuffer> + Send + 'static {
        self.inner.frames()
    }

    /// Starts capture; the first frame follows at once. Resuming a running
    /// stream does nothing.
    ///
    /// # Errors
    ///
    /// Returns [`Error::PermissionDenied`] without the Screen Recording
    /// permission and [`Error::Platform`] when capture cannot start.
    pub async fn resume(&mut self) -> Result<(), Error> {
        self.inner.resume().await
    }

    /// Stops capture until [`resume`](Self::resume). Pausing a paused stream
    /// does nothing.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Platform`] when capture cannot stop.
    pub async fn pause(&mut self) -> Result<(), Error> {
        self.inner.pause().await
    }
}
