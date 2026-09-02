//! Clipboard content types and traits.

use crate::ClipboardError;

/// Image data containing width, height, and raw RGBA bytes.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Image {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
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

    /// Width of the image in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height of the image in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Raw RGBA bytes of the image (4 bytes per pixel).
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume the image and return the raw RGBA bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
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
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)]
pub struct ClipboardEvent {
    has_text: bool,
    has_html: bool,
    has_files: bool,
    has_image: bool,
}

impl ClipboardEvent {
    /// Create a new clipboard event.
    #[must_use]
    #[allow(clippy::fn_params_excessive_bools)]
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            dead_code,
            reason = "browser clipboard change observation is unavailable"
        )
    )]
    pub(crate) const fn new(
        has_text: bool,
        has_html: bool,
        has_files: bool,
        has_image: bool,
    ) -> Self {
        Self {
            has_text,
            has_html,
            has_files,
            has_image,
        }
    }

    /// Whether text content is available.
    #[must_use]
    pub const fn has_text(&self) -> bool {
        self.has_text
    }

    /// Whether HTML content is available.
    #[must_use]
    pub const fn has_html(&self) -> bool {
        self.has_html
    }

    /// Whether file paths are available.
    #[must_use]
    pub const fn has_files(&self) -> bool {
        self.has_files
    }

    /// Whether image data is available.
    #[must_use]
    pub const fn has_image(&self) -> bool {
        self.has_image
    }
}
