//! GPU-first zero-copy screen capture with wgpu texture output.
//!
//! This crate provides two capture modes:
//!
//! 1. **Simple Screenshot** - No GPU device required, returns encoded image data (PNG/AVIF/HEIF)
//! 2. **GPU Streaming** - Zero-copy screen capture with wgpu texture output for real-time processing
//!
//! # Simple Screenshot
//!
//! ```ignore
//! use waterkit_screen::{screens, screenshot, ImageFormat};
//!
//! let displays = screens()?;
//! let shot = screenshot(&displays[0], ImageFormat::Png)?;
//! shot.save("screenshot.png")?;
//! ```
//!
//! # GPU Streaming
//!
//! ```ignore
//! use waterkit_screen::{screens, ScreenStream, StreamConfig};
//! use std::sync::Arc;
//!
//! let displays = screens()?;
//! let stream = ScreenStream::start(
//!     &displays[0],
//!     device.clone(),
//!     queue.clone(),
//!     StreamConfig::default(),
//! )?;
//!
//! while let Some(frame) = stream.try_next_frame() {
//!     let view = frame.create_view();
//!     // Use view in render pipeline...
//! }
//! ```

mod frame;
mod screenshot;
mod stream;
mod sys;

pub use frame::ScreenFrame;
pub use screenshot::{ImageFormat, Screenshot, screenshot, screenshot_primary};
pub use stream::{ScreenStream, StreamConfig};
pub use waterkit_core::{Brightness, RefreshRate};

/// Errors returned by screen operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An error occurred in the underlying platform implementation.
    #[error("Platform error: {0}")]
    Platform(String),

    /// The requested feature is not supported on the current platform.
    #[error("Unsupported platform or feature")]
    Unsupported,

    /// The specified monitor index was not found.
    #[error("Monitor not found")]
    MonitorNotFound,

    /// An I/O error occurred.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to create GPU resources.
    #[error("GPU error: {0}")]
    Gpu(String),

    /// The capture stream was stopped.
    #[error("Stream stopped")]
    StreamStopped,

    /// Permission denied for screen capture.
    #[error("Permission denied")]
    PermissionDenied,

    /// Image encoding failed.
    #[error("Encoding error: {0}")]
    Encoding(String),
}

/// Information about a display/screen.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ScreenInfo {
    id: u32,
    name: String,
    width: u32,
    height: u32,
    scale_factor: f32,
    is_primary: bool,
}

impl ScreenInfo {
    /// Create a new `ScreenInfo`.
    pub(crate) const fn new(
        id: u32,
        name: String,
        width: u32,
        height: u32,
        scale_factor: f32,
        is_primary: bool,
    ) -> Self {
        Self {
            id,
            name,
            width,
            height,
            scale_factor,
            is_primary,
        }
    }

    /// A platform-specific unique identifier for the screen.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// A human-readable name for the display.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The scale factor (e.g., 2.0 for Retina/HiDPI displays).
    #[must_use]
    pub const fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// Whether this is the primary system display.
    #[must_use]
    pub const fn is_primary(&self) -> bool {
        self.is_primary
    }
}

/// List all available screens detected by the system.
///
/// # Errors
///
/// Returns [`Error::Platform`] if screen enumeration fails.
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    sys::screens()
}

/// Returns the maximum refresh rate across available displays.
///
/// # Errors
///
/// Returns [`Error::Unsupported`] when the platform cannot report refresh-rate metadata.
/// Returns [`Error::MonitorNotFound`] when no displays are available.
pub fn max_refresh_rate() -> Result<RefreshRate, Error> {
    let raw = sys::max_refresh_rate_hz()?;
    RefreshRate::new(raw).map_err(|error| Error::Platform(error.to_string()))
}

/// Returns the current screen brightness.
///
/// # Errors
///
/// Returns [`Error::Platform`] if the brightness cannot be retrieved.
pub async fn brightness() -> Result<Brightness, Error> {
    let raw = sys::get_brightness().await?;
    Brightness::new(raw).map_err(|error| Error::Platform(error.to_string()))
}

/// Sets the screen brightness.
///
/// # Errors
///
/// Returns [`Error::Platform`] if the brightness cannot be set.
pub async fn set_brightness(value: Brightness) -> Result<(), Error> {
    sys::set_brightness(value.get()).await
}

/// Initialize the screen subsystem for Android.
///
/// This must be called from JNI with a valid `Context` before any other functions are used.
///
/// # Errors
///
/// Returns `Error::Platform` if initialization fails or if called more than once.
#[cfg(target_os = "android")]
pub fn init(env: &mut jni::JNIEnv, context: &jni::objects::JObject) -> Result<(), Error> {
    sys::android::init(env, context)
}
