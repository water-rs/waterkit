//! # waterkit-screen
//!
//! A cross-platform library for screen capture and brightness control.
//!
//! Part of the WaterKit ecosystem, this crate provides a unified API for interacting with screens
//! across Desktop (macOS, Windows, Linux) and Mobile (iOS, Android).
//!
//! ## Features
//!
//! - **Screen Listing**: Enumerate available monitors and their properties.
//! - **Screen Capture**: Capture screenshots as PNG-encoded bytes.
//! - **Brightness Control**: Get and set screen brightness levels.
//! - **System Picker**: (macOS 14.0+) High-privacy screen/window selection via `ScreenCaptureKit`.
//!
//! ## Platform Specifics
//!
//! ### Android
//! On Android, you must initialize the library with a `Context` before calling other methods:
//!
//! ```rust,no_run
//! #[no_mangle]
//! pub extern "C" fn Java_com_example_MainActivity_initScreen(mut env: jni::JNIEnv, _: jni::objects::JClass, context: jni::objects::JObject) {
//!     waterkit_screen::init(&mut env, &context).unwrap();
//! }
//! ```
//!
//! ### macOS
//! Brightness control for macOS is currently a stub due to downstream dependency limitations.
//! Screen capture via `capture_screen` requires the "Screen Recording" permission.
//! `pick_and_capture` uses the system-provided picker and does not require broad permissions.

mod platform;

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

    /// An I/O error occurred during image processing.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
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

/// Capture the screen content as a PNG.
///
/// # Arguments
///
/// * `display_index` - The 0-based index of the screen to capture (corresponds to [screens] order).
///
/// # Returns
///
/// Returns a `Vec<u8>` containing the PNG-encoded image.
///
/// # Platform Behavior
///
/// - **Desktop**: Captures the entire desktop of the specified monitor.
/// - **iOS**: Captures a snapshot of the current application's key window.
/// - **Android**: Currently unsupported.
pub fn capture_screen(display_index: usize) -> Result<Vec<u8>, Error> {
    platform::capture_screen(display_index)
}

/// Raw screen capture result.
#[derive(Debug, Clone)]
pub struct RawCapture {
    /// RGBA pixel data.
    pub data: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Capture the screen content as raw RGBA bytes (no PNG encoding).
///
/// This is faster than [capture_screen] as it skips PNG compression.
/// Useful for real-time encoding pipelines.
///
/// # Arguments
///
/// * `display_index` - The 0-based index of the screen to capture.
pub fn capture_screen_raw(display_index: usize) -> Result<RawCapture, Error> {
    platform::capture_screen_raw(display_index)
}

/// Pick a screen or window using the system-provided picker and capture it.
///
/// This provides a more privacy-conscious way of capturing content as it does not
/// require broad "Screen Recording" permissions.
///
/// # Returns
///
/// Returns a `Vec<u8>` containing the PNG-encoded image of the selected area.
///
/// # Platform Support
///
/// - **macOS**: Supported on macOS 14.0+ via `SCContentSharingPicker`.
/// - **Other Platforms**: Returns [Error::Unsupported].
pub async fn pick_and_capture() -> Result<Vec<u8>, Error> {
    platform::pick_and_capture().await
}

/// Get the current screen brightness level.
///
/// # Returns
///
/// A float between `0.0` (darkest) and `1.0` (brightest).
pub async fn get_brightness() -> Result<f32, Error> {
    platform::get_brightness().await
}

/// Set the screen brightness level.
///
/// # Arguments
///
/// * `val` - A float between `0.0` and `1.0`. Values outside this range will be clamped.
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    platform::set_brightness(val).await
}

/// List all available screens detected by the system.
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    platform::screens()
}

/// Initialize the screen subsystem for Android.
///
/// This must be called from JNI with a valid `Context` before any other functions are used.
#[cfg(target_os = "android")]
pub fn init(env: &mut jni::JNIEnv, context: &jni::objects::JObject) -> Result<(), Error> {
    platform::init(env, context)
}
