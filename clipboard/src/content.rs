//! Clipboard content types and traits.

use crate::ClipboardError;

/// Image data containing width, height, and raw RGBA bytes.
#[derive(Debug, Clone)]
pub struct Image {
    /// Width of the image in pixels.
    pub width: u32,
    /// Height of the image in pixels.
    pub height: u32,
    /// Raw RGBA bytes of the image (4 bytes per pixel).
    pub bytes: Vec<u8>,
}

impl Image {
    /// Creates a new `Image` from raw RGBA bytes.
    ///
    /// # Panics
    ///
    /// Panics if `bytes.len() != width * height * 4`.
    #[must_use]
    pub fn new(width: u32, height: u32, bytes: Vec<u8>) -> Self {
        let expected = (width as usize) * (height as usize) * 4;
        assert_eq!(
            bytes.len(),
            expected,
            "bytes length must equal width * height * 4"
        );
        Self {
            width,
            height,
            bytes,
        }
    }
}

/// Trait for custom data types that can be stored in the clipboard.
///
/// # Errors
///
/// Implementations should return [`ClipboardError::Encode`] if encoding fails
/// and [`ClipboardError::Decode`] if decoding fails.
///
/// Implement this trait to enable automatic encoding/decoding of your types.
///
/// # Example
///
/// ```ignore
/// use waterkit_clipboard::{ClipboardData, ClipboardError};
///
/// struct MyData {
///     name: String,
///     value: i32,
/// }
///
/// impl ClipboardData for MyData {
///     const MIME_TYPE: &'static str = "application/x-myapp-data";
///
///     fn encode(&self) -> Result<Vec<u8>, ClipboardError> {
///         // Use your preferred serialization format
///         Ok(format!("{}:{}", self.name, self.value).into_bytes())
///     }
///
///     fn decode(bytes: &[u8]) -> Result<Self, ClipboardError> {
///         let s = std::str::from_utf8(bytes)
///             .map_err(|e| ClipboardError::Decode(e.to_string()))?;
///         let parts: Vec<&str> = s.split(':').collect();
///         if parts.len() != 2 {
///             return Err(ClipboardError::Decode("invalid format".into()));
///         }
///         Ok(Self {
///             name: parts[0].to_string(),
///             value: parts[1].parse()
///                 .map_err(|e| ClipboardError::Decode(format!("{e}")))?,
///         })
///     }
/// }
/// ```
pub trait ClipboardData: Sized {
    /// The MIME type for this data format.
    const MIME_TYPE: &'static str;

    /// Encode this data to bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ClipboardError::Encode`] if encoding fails.
    fn encode(&self) -> Result<Vec<u8>, ClipboardError>;

    /// Decode bytes into this type.
    ///
    /// # Errors
    ///
    /// Returns [`ClipboardError::Decode`] if decoding fails.
    fn decode(bytes: &[u8]) -> Result<Self, ClipboardError>;
}

/// Event emitted when the clipboard content changes.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct ClipboardEvent {
    /// Whether text content is available.
    pub has_text: bool,
    /// Whether HTML content is available.
    pub has_html: bool,
    /// Whether file paths are available.
    pub has_files: bool,
    /// Whether image data is available.
    pub has_image: bool,
}
