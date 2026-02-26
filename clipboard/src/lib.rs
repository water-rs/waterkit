//! Cross-platform clipboard access.
//!
//! This crate provides a unified API for interacting with the system clipboard
//! across macOS, Windows, Linux, Android, and iOS.
//!
//! # Example
//!
//! ```no_run
//! use waterkit_clipboard::Clipboard;
//!
//! # async fn example() -> Result<(), waterkit_clipboard::ClipboardError> {
//! let mut clipboard = Clipboard::new()?;
//!
//! // Check and read text
//! if clipboard.has_text() {
//!     if let Some(text) = clipboard.text().await? {
//!         println!("Clipboard text: {text}");
//!     }
//! }
//!
//! // Set text
//! clipboard.set_text("Hello, clipboard!")?;
//! # Ok(())
//! # }
//! ```
//!
//! # Clipboard Watching
//!
//! ```no_run
//! use futures::StreamExt;
//! use waterkit_clipboard::Clipboard;
//!
//! # async fn example() -> Result<(), waterkit_clipboard::ClipboardError> {
//! let clipboard = Clipboard::new()?;
//! let mut stream = clipboard.watch()?;
//!
//! while let Some(event) = stream.next().await {
//!     println!("Clipboard changed! has_text={}", event.has_text());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Custom Data Types
//!
//! ```no_run
//! use waterkit_clipboard::{Clipboard, ClipboardData, ClipboardError};
//!
//! struct MyData {
//!     value: i32,
//! }
//!
//! impl ClipboardData for MyData {
//!     const MIME_TYPE: &'static str = "application/x-mydata";
//!
//!     fn encode(&self) -> Result<Vec<u8>, ClipboardError> {
//!         Ok(self.value.to_le_bytes().to_vec())
//!     }
//!
//!     fn decode(bytes: &[u8]) -> Result<Self, ClipboardError> {
//!         let arr: [u8; 4] = bytes.try_into()
//!             .map_err(|_| ClipboardError::Decode("invalid length".into()))?;
//!         Ok(Self { value: i32::from_le_bytes(arr) })
//!     }
//! }
//!
//! # async fn example() -> Result<(), ClipboardError> {
//! let mut clipboard = Clipboard::new()?;
//! clipboard.set_data(&MyData { value: 42 })?;
//! let data: Option<MyData> = clipboard.data().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Platform Notes
//!
//! | Feature | Windows | Linux | macOS | iOS | Android |
//! |---------|---------|-------|-------|-----|---------|
//! | Text    | ✓       | ✓     | ✓     | ✓   | ✓       |
//! | HTML    | ✓       | ✓     | ✓     | ✓   | ✓       |
//! | Image   | ✓       | ✓     | ✓     | ✓   | ✓       |
//! | Files   | ✓       | ✓     | ✓     | ✓   | ✓       |
//! | Watch   | ✓       | ✓     | ✓     | ✓   | ✓       |
//!
//! ## Android
//!
//! On Android, the context is obtained automatically via `ndk_context`.
//! Your app must initialize `ndk_context` before using the clipboard.
//! If the context is not available, `Clipboard::new()` will panic.

#![warn(missing_docs)]

mod content;
mod error;
mod stream;
mod sys;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use content::{ClipboardData, ClipboardEvent, Image};
pub use error::ClipboardError;
pub use stream::ClipboardStream;

/// A handle to the system clipboard.
///
/// This struct provides methods to read and write clipboard content,
/// query available types, and watch for changes.
///
/// # Async Reads
///
/// All read operations are async because the clipboard source might be slow
/// (e.g., when using file promises or large data transfers).
///
/// # Sync Writes
///
/// Write operations are sync because we control the data being written.
#[derive(Debug, Clone)]
pub struct Clipboard {
    inner: Arc<sys::ClipboardInner>,
}

impl Clipboard {
    /// Create a new clipboard handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    ///
    /// # Panics
    ///
    /// On Android, panics if `ndk_context` is not initialized.
    pub fn new() -> Result<Self, ClipboardError> {
        Ok(Self {
            inner: Arc::new(sys::ClipboardInner::new()?),
        })
    }

    // ========== Query (sync - instant metadata checks) ==========

    /// Check if text content is available in the clipboard.
    #[must_use]
    pub fn has_text(&self) -> bool {
        self.inner.has_text()
    }

    /// Check if HTML content is available in the clipboard.
    #[must_use]
    pub fn has_html(&self) -> bool {
        self.inner.has_html()
    }

    /// Check if file paths are available in the clipboard.
    #[must_use]
    pub fn has_files(&self) -> bool {
        self.inner.has_files()
    }

    /// Check if image data is available in the clipboard.
    #[must_use]
    pub fn has_image(&self) -> bool {
        self.inner.has_image()
    }

    // ========== Read (async - source might be slow) ==========

    /// Get text content from the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub async fn text(&self) -> Result<Option<String>, ClipboardError> {
        let inner = Arc::clone(&self.inner);
        blocking::unblock(move || inner.get_text()).await
    }

    /// Get HTML content from the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub async fn html(&self) -> Result<Option<String>, ClipboardError> {
        let inner = Arc::clone(&self.inner);
        blocking::unblock(move || inner.get_html()).await
    }

    /// Get file paths from the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub async fn files(&self) -> Result<Vec<PathBuf>, ClipboardError> {
        let inner = Arc::clone(&self.inner);
        blocking::unblock(move || inner.get_files()).await
    }

    /// Get image content from the clipboard as raw RGBA pixels.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed or if
    /// image conversion fails.
    pub async fn image(&self) -> Result<Option<Image>, ClipboardError> {
        let inner = Arc::clone(&self.inner);
        blocking::unblock(move || inner.get_image()).await
    }

    /// Get custom data from the clipboard.
    ///
    /// The type must implement [`ClipboardData`] to define its MIME type
    /// and encoding/decoding logic.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed or if
    /// decoding fails.
    pub async fn data<T: ClipboardData + Send + 'static>(
        &self,
    ) -> Result<Option<T>, ClipboardError> {
        let inner = Arc::clone(&self.inner);
        blocking::unblock(move || {
            if let Some(bytes) = inner.get_binary(T::MIME_TYPE)? {
                Ok(Some(T::decode(&bytes)?))
            } else {
                Ok(None)
            }
        })
        .await
    }

    /// Get raw binary data from the clipboard by MIME type.
    ///
    /// Use this when you want to handle encoding/decoding yourself.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub async fn binary(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        let inner = Arc::clone(&self.inner);
        let mime = mime.to_string();
        blocking::unblock(move || inner.get_binary(&mime)).await
    }

    // ========== Write (sync - we control the data) ==========

    /// Set text content to the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.inner.set_text(text)
    }

    /// Set HTML content to the clipboard.
    ///
    /// # Arguments
    ///
    /// * `html` - The HTML content to set.
    /// * `alt_text` - Optional plain text fallback for applications that don't support HTML.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub fn set_html(&mut self, html: &str, alt_text: Option<&str>) -> Result<(), ClipboardError> {
        self.inner.set_html(html, alt_text)
    }

    /// Set file paths to the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub fn set_files(&mut self, files: &[PathBuf]) -> Result<(), ClipboardError> {
        self.inner.set_files(files)
    }

    /// Set image content to the clipboard from a file path.
    ///
    /// The image format is detected from the file extension.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed or if
    /// the image cannot be read.
    pub fn set_image(&mut self, path: &Path) -> Result<(), ClipboardError> {
        self.inner.set_image_from_path(path)
    }

    /// Set custom data to the clipboard.
    ///
    /// The type must implement [`ClipboardData`] to define its MIME type
    /// and encoding logic.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed or if
    /// encoding fails.
    pub fn set_data<T: ClipboardData>(&mut self, data: &T) -> Result<(), ClipboardError> {
        let bytes = data.encode()?;
        self.inner.set_binary(&bytes, T::MIME_TYPE)
    }

    /// Set raw binary data to the clipboard with a MIME type.
    ///
    /// Use this when you want to handle encoding yourself.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub fn set_binary(&mut self, data: &[u8], mime: &str) -> Result<(), ClipboardError> {
        self.inner.set_binary(data, mime)
    }

    /// Set a file promise to the clipboard.
    ///
    /// The provider closure is called when the recipient requests the data.
    /// This is useful for lazy generation of clipboard content.
    ///
    /// # Platform Notes
    ///
    /// - **macOS/iOS**: Uses `NSFilePromiseProvider`
    /// - **Other platforms**: Falls back to immediate file copy
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub fn set_file_promise<F>(&mut self, provider: F) -> Result<(), ClipboardError>
    where
        F: FnOnce() -> Result<PathBuf, ClipboardError> + Send + 'static,
    {
        self.inner.set_file_promise(Box::new(provider))
    }

    // ========== Control ==========

    /// Clear all clipboard content.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub fn clear(&mut self) -> Result<(), ClipboardError> {
        self.inner.clear()
    }

    /// Watch for clipboard changes.
    ///
    /// Returns a stream that yields [`ClipboardEvent`]s whenever the
    /// clipboard content changes.
    ///
    /// # Platform Notes
    ///
    /// - **Desktop (Windows/Linux/macOS)**: Uses native clipboard change notifications.
    /// - **iOS**: Uses polling with `UIPasteboard.changeCount` (500ms interval).
    /// - **Android**: Uses polling with `ClipboardManager.getPrimaryClipDescription` (500ms interval).
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard watcher cannot be started.
    pub fn watch(&self) -> Result<ClipboardStream, ClipboardError> {
        let (receiver, shutdown) = sys::start_watch()?;
        Ok(ClipboardStream::new(receiver, shutdown))
    }
}
