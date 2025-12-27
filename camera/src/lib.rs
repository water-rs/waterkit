//! Cross-platform camera streaming.
//!
//! This crate provides a unified API for camera enumeration and streaming
//! across iOS, macOS, Android, Windows, and Linux platforms with efficient
//! WGPU texture integration.

#![warn(missing_docs)]

mod sys;

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use sys::apple::IOSurfaceHandle;

/// Information about a camera device.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CameraInfo {
    /// Unique identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Whether the camera is front-facing.
    pub is_front_facing: bool,
}

/// Pixel format of a camera frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameFormat {
    /// RGB 8-bit.
    Rgb,
    /// RGBA 8-bit.
    Rgba,
    /// BGRA 8-bit.
    Bgra,
    /// NV12 (YUV 4:2:0 bi-planar).
    Nv12,
    /// YUY2 (YUV 4:2:2).
    Yuy2,
    /// JPEG compressed.
    Jpeg,
}

impl FrameFormat {
    /// Get bytes per pixel (approximate for planar formats).
    #[must_use]
    pub const fn bytes_per_pixel(&self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba | Self::Bgra => 4,
            Self::Nv12 => 1, // 1.5 actually, handled specially
            Self::Yuy2 => 2,
            Self::Jpeg => 0, // Variable
        }
    }
}

/// A captured camera frame.
#[derive(Debug, Clone)]
pub struct CameraFrame {
    /// Raw pixel data.
    pub data: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: FrameFormat,
    /// Optional platform-specific handle (e.g. `IOSurface`).
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub iosurface: Option<IOSurfaceHandle>,
}

impl CameraFrame {
    /// Create a new frame.
    #[must_use]
    pub const fn new(
        data: Vec<u8>,
        width: u32,
        height: u32,
        format: FrameFormat,
        #[cfg(any(target_os = "macos", target_os = "ios"))] iosurface: Option<IOSurfaceHandle>,
    ) -> Self {
        Self {
            data,
            width,
            height,
            format,
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            iosurface,
        }
    }

    /// Convert frame data to RGBA.
    ///
    /// Currently only a stub for non-RGB/RGBA formats.
    #[must_use]
    pub fn to_rgba(&self) -> Vec<u8> {
        // TODO: Implement actual conversion for NV12, YUY2, JPEG
        #[allow(clippy::match_same_arms)]
        match self.format {
            FrameFormat::Rgba => self.data.clone(),
            _ => self.data.clone(),
        }
    }
}

// ... skipping to CameraError ...

/// Errors that can occur with camera operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CameraError {
    /// Camera is not supported on this platform.
    #[error("camera not supported on this platform")]
    NotSupported,
    /// Failed to enumerate cameras.
    #[error("failed to enumerate cameras: {0}")]
    EnumerationFailed(String),
    /// Camera not found.
    #[error("camera not found: {0}")]
    NotFound(String),
    /// Failed to open camera.
    #[error("failed to open camera: {0}")]
    OpenFailed(String),
    /// Failed to start camera.
    #[error("failed to start camera: {0}")]
    StartFailed(String),
    /// Failed to capture frame.
    #[error("failed to capture frame: {0}")]
    CaptureFailed(String),
    /// Permission denied.
    #[error("camera permission denied")]
    PermissionDenied,
    /// Camera is already in use.
    #[error("camera is already in use")]
    AlreadyInUse,
    /// An unknown error occurred.
    #[error("unknown error: {0}")]
    Unknown(String),
}

/// Camera resolution configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Resolution {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Resolution {
    /// Standard 720p resolution.
    pub const HD: Self = Self {
        width: 1280,
        height: 720,
    };

    /// Standard 1080p resolution.
    pub const FULL_HD: Self = Self {
        width: 1920,
        height: 1080,
    };

    /// Standard 4K resolution.
    pub const UHD: Self = Self {
        width: 3840,
        height: 2160,
    };
}

/// Camera controller.
#[derive(Debug)]
pub struct Camera {
    inner: sys::CameraInner,
}

impl Camera {
    /// List available cameras on the system.
    ///
    /// # Errors
    /// Returns [`CameraError::EnumerationFailed`] if camera enumeration fails.
    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        sys::CameraInner::list()
    }

    /// Open a camera by its ID.
    ///
    /// # Errors
    /// Returns [`CameraError::OpenFailed`] if the camera cannot be opened.
    pub fn open(camera_id: &str) -> Result<Self, CameraError> {
        Ok(Self {
            inner: sys::CameraInner::open(camera_id)?,
        })
    }

    /// Open the default camera.
    ///
    /// On desktop, this is typically the first webcam.
    /// On mobile, this is typically the back camera.
    ///
    /// # Errors
    /// Returns [`CameraError::NotFound`] if no camera is available.
    pub fn open_default() -> Result<Self, CameraError> {
        let cameras = Self::list()?;
        let camera = cameras
            .first()
            .ok_or_else(|| CameraError::NotFound("no cameras available".into()))?;
        Self::open(&camera.id)
    }

    /// Start capturing frames.
    ///
    /// # Errors
    /// Returns [`CameraError::StartFailed`] if the camera cannot be started.
    pub fn start(&mut self) -> Result<(), CameraError> {
        self.inner.start()
    }

    /// Stop capturing frames.
    ///
    /// # Errors
    /// Returns [`CameraError::Unknown`] if the camera cannot be stopped.
    pub fn stop(&mut self) -> Result<(), CameraError> {
        self.inner.stop()
    }

    /// Get the next captured frame.
    ///
    /// This may block until a frame is available.
    ///
    /// # Errors
    /// Returns [`CameraError::CaptureFailed`] if frame capture fails.
    pub fn get_frame(&mut self) -> Result<CameraFrame, CameraError> {
        self.inner.get_frame()
    }

    /// Set the desired resolution.
    ///
    /// The actual resolution may differ based on camera capabilities.
    ///
    /// # Errors
    /// Returns [`CameraError::Unknown`] if the resolution cannot be set.
    pub fn set_resolution(&mut self, resolution: Resolution) -> Result<(), CameraError> {
        self.inner.set_resolution(resolution)
    }

    /// Get the current resolution.
    #[must_use]
    pub fn resolution(&self) -> Resolution {
        self.inner.resolution()
    }

    /// Get the number of dropped frames since start.
    #[must_use]
    pub fn dropped_frame_count(&self) -> u64 {
        self.inner.dropped_frame_count()
    }

    /// Enable or disable HDR mode.
    ///
    /// # Errors
    /// Returns [`CameraError::NotSupported`] if the camera or backend does not support HDR/HLG.
    pub fn set_hdr(&self, enabled: bool) -> Result<(), CameraError> {
        self.inner.set_hdr(enabled)
    }

    /// Check if HDR mode is currently enabled.
    #[must_use]
    pub fn hdr_enabled(&self) -> bool {
        self.inner.hdr_enabled()
    }

    /// Take a high-quality photo.
    ///
    /// On mobile, this uses the system's computational photography pipeline.
    /// On desktop, this returns the next available frame.
    ///
    /// The result format may be `FrameFormat::Jpeg` on mobile.
    ///
    /// # Errors
    /// Returns [`CameraError::CaptureFailed`] if the photo cannot be taken.
    pub fn take_photo(&mut self) -> Result<CameraFrame, CameraError> {
        self.inner.take_photo()
    }

    /// Start recording video to the specified file path.
    ///
    /// # Arguments
    /// * `path` - content file path to save the video.
    ///
    /// # Errors
    /// Returns [`CameraError::StartFailed`] if the recording cannot be started.
    pub fn start_recording(&mut self, path: &str) -> Result<(), CameraError> {
        self.inner.start_recording(path)
    }

    /// Stop the current video recording.
    ///
    /// # Errors
    /// Returns [`CameraError::Unknown`] if the recording cannot be stopped.
    pub fn stop_recording(&mut self) -> Result<(), CameraError> {
        self.inner.stop_recording()
    }
}

#[cfg(feature = "codec")]
impl TryFrom<CameraFrame> for waterkit_codec::Frame {
    type Error = waterkit_codec::CodecError;

    fn try_from(frame: CameraFrame) -> Result<Self, Self::Error> {
        use std::sync::Arc;
        use waterkit_codec::{CodecError, PixelFormat};

        let format = match frame.format {
            FrameFormat::Rgba => PixelFormat::Rgba,
            FrameFormat::Bgra => PixelFormat::Bgra,
            FrameFormat::Nv12 => PixelFormat::Nv12,
            _ => {
                return Err(CodecError::Unsupported(format!(
                    "Unsupported format for codec: {:?}",
                    frame.format
                )));
            }
        };

        Ok(Self {
            data: Arc::new(frame.data),
            width: frame.width,
            height: frame.height,
            format,
            timestamp_ns: 0, // Todo: Propagate timestamp if available
        })
    }
}
