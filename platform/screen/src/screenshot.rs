//! Simple screenshot capture with image encoding.

use crate::{Error, ScreenInfo, sys};
use std::path::Path;

/// Image format for screenshot export.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageFormat {
    /// PNG format (lossless, widely supported).
    #[default]
    Png,
    /// AVIF format (modern, efficient, requires macOS 14+ / iOS 17+).
    Avif,
    /// HEIF format (efficient, requires macOS / iOS).
    Heif,
}

impl ImageFormat {
    /// Get the file extension for this format.
    #[must_use]
    pub const fn extension(&self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Avif => "avif",
            Self::Heif => "heic",
        }
    }

    /// Get the MIME type for this format.
    #[must_use]
    pub const fn mime_type(&self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Avif => "image/avif",
            Self::Heif => "image/heic",
        }
    }
}

/// Captured screenshot with encoded image data.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Screenshot {
    data: Vec<u8>,
    width: u32,
    height: u32,
    format: ImageFormat,
}

impl Screenshot {
    /// Create a new `Screenshot`.
    pub(crate) const fn new(data: Vec<u8>, width: u32, height: u32, format: ImageFormat) -> Self {
        Self {
            data,
            width,
            height,
            format,
        }
    }

    /// Encoded image data.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Consume the screenshot and return the encoded image data.
    #[must_use]
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// Image width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Image format.
    #[must_use]
    pub const fn format(&self) -> ImageFormat {
        self.format
    }

    /// Save the screenshot to a file.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the file cannot be written.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        std::fs::write(path, &self.data)?;
        Ok(())
    }
}

/// Capture a screenshot of the specified display.
///
/// # Arguments
///
/// * `display` - The display to capture.
/// * `format` - The image format to encode as.
///
/// # Errors
///
/// Returns [`Error::Platform`] if capture fails, or [`Error::Unsupported`]
/// if the format is not available on this platform.
#[allow(clippy::missing_const_for_fn)] // Not const on all platforms
pub fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    sys::screenshot(display, format)
}

/// Capture the primary display.
///
/// # Arguments
///
/// * `format` - The image format to encode as.
///
/// # Errors
///
/// Returns [`Error::MonitorNotFound`] if no primary display is found.
pub fn screenshot_primary(format: ImageFormat) -> Result<Screenshot, Error> {
    let displays = crate::screens()?;
    let primary = displays
        .iter()
        .find(|d| d.is_primary())
        .or_else(|| displays.first())
        .ok_or(Error::MonitorNotFound)?;
    screenshot(primary, format)
}
