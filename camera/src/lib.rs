//! Cross-platform camera streaming.
//!
//! This crate provides a unified API for camera enumeration and streaming
//! across iOS, macOS, Android, Windows, and Linux platforms with efficient
//! WGPU texture integration.

#![warn(missing_docs)]

mod sys;

use std::fmt;

/// Re-export wgpu for texture integration.
pub use wgpu;

// ... (CameraInfo, FrameFormat, CameraFrame impls unchanged but I need to be careful with line numbers)
// Note: I will only replace `pub mod sys` and `CameraError` definition and impls.

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
