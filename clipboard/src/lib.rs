//! Cross-platform clipboard access.
//!
//! This crate provides a unified API for interacting with the system clipboard
//! across macOS, Windows, Linux, Android, and iOS.

#![warn(missing_docs)]

mod sys;

pub use sys::{get_image, get_text, set_image, set_text};

/// Image data containing width, height, and raw RGBA bytes.
#[derive(Debug, Clone)]
pub struct ImageData {
    /// Width of the image in pixels.
    pub width: usize,
    /// Height of the image in pixels.
    pub height: usize,
    /// Raw RGBA bytes of the image.
    pub bytes: std::borrow::Cow<'static, [u8]>,
}
