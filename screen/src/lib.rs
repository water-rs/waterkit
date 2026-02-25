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

/// Errors returned by screen operations.
#[derive(Debug, thiserror::Error)]
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
pub struct ScreenInfo {
    /// A platform-specific unique identifier for the screen.
    pub id: u32,
    /// A human-readable name for the display.
    pub name: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The scale factor (e.g., 2.0 for Retina/HiDPI displays).
    pub scale_factor: f32,
    /// Whether this is the primary system display.
    pub is_primary: bool,
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
pub fn max_refresh_rate_hz() -> Result<f32, Error> {
    sys::max_refresh_rate_hz()
}

/// Get the current screen brightness level.
///
/// Returns a value between 0.0 and 1.0.
///
/// # Errors
///
/// Returns [`Error::Platform`] if the brightness level cannot be retrieved.
pub async fn get_brightness() -> Result<f32, Error> {
    sys::get_brightness().await
}

/// Set the screen brightness level.
///
/// # Arguments
///
/// * `val` - A float between 0.0 and 1.0. Values outside this range will be clamped.
///
/// # Errors
///
/// Returns [`Error::Platform`] if the brightness level cannot be set.
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    sys::set_brightness(val).await
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
