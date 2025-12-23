//! Cross-platform clipboard access for WaterUI.
//!
//! This crate provides a unified API for interacting with the system clipboard
//! across macOS, Windows, Linux, Android, and iOS.
//!
//! # Platform Support
//! - **Desktop (Windows, macOS, Linux)**: Uses `arboard` for text and image support.
//! - **Android**: Uses JNI to access `ClipboardManager`. Requires passing `Context`.
//! - **iOS**: Uses `UIPasteboard` via `swift-bridge`.
//!
//! # Example
//! ```rust,no_run
//! // Desktop usage
//! waterkit_clipboard::set_text("Hello".to_string());
//! if let Some(text) = waterkit_clipboard::get_text() {
//!     println!("Clipboard: {}", text);
//! }
//! ```

mod sys;

pub use sys::{get_image, get_text, set_image, set_text};

#[derive(Debug, Clone)]
/// Image data containing width, height, and raw RGBA bytes.
pub struct ImageData {
    /// Width of the image in pixels.
    pub width: usize,
    /// Height of the image in pixels.
    pub height: usize,
    /// Raw RGBA bytes of the image.
    pub bytes: std::borrow::Cow<'static, [u8]>,
}
