//! iOS clipboard implementation using swift-bridge.

#![allow(clippy::unnecessary_wraps)] // API consistency with other platforms
#![allow(clippy::needless_pass_by_ref_mut)] // API consistency with other platforms
#![allow(clippy::unused_self)] // Methods use global UIPasteboard, &self is for API consistency
#![allow(clippy::missing_const_for_fn)] // FFI calls cannot be const

use crate::content::{ClipboardEvent, Image};
use crate::error::ClipboardError;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

#[swift_bridge::bridge]
mod ffi {
    #[swift_bridge(swift_repr = "struct")]
    struct SwiftImageData {
        width: usize,
        height: usize,
        bytes: Vec<u8>,
        is_valid: bool,
    }

    #[swift_bridge(swift_repr = "struct")]
    struct SwiftBinaryData {
        bytes: Vec<u8>,
        is_valid: bool,
    }

    extern "Swift" {
        // Query
        fn clipboard_has_text() -> bool;
        fn clipboard_has_html() -> bool;
        fn clipboard_has_image() -> bool;
        fn clipboard_has_files() -> bool;

        // Read
        fn clipboard_get_text() -> Option<String>;
        fn clipboard_get_html() -> Option<String>;
        fn clipboard_get_image() -> SwiftImageData;
        fn clipboard_get_file_url() -> Option<String>;
        fn clipboard_get_binary(mime: String) -> SwiftBinaryData;

        // Write
        fn clipboard_set_text(text: String);
        fn clipboard_set_html(html: String, alt_text: String);
        fn clipboard_set_image_from_path(path: String) -> bool;
        fn clipboard_set_file_url(url: String);
        fn clipboard_set_binary(data: Vec<u8>, mime: String);

        // Control
        fn clipboard_clear();
        fn clipboard_get_change_count() -> i64;
    }
}

/// iOS clipboard handle.
#[derive(Debug)]
pub struct ClipboardInner;

impl ClipboardInner {
    /// Create a new clipboard handle.
    pub fn new() -> Result<Self, ClipboardError> {
        Ok(Self)
    }

    // ========== Query (sync) ==========

    /// Check if text is available.
    pub fn has_text(&self) -> bool {
        ffi::clipboard_has_text()
    }

    /// Check if HTML is available.
    pub fn has_html(&self) -> bool {
        ffi::clipboard_has_html()
    }

    /// Check if files are available.
    pub fn has_files(&self) -> bool {
        ffi::clipboard_has_files()
    }

    /// Check if image is available.
    pub fn has_image(&self) -> bool {
        ffi::clipboard_has_image()
    }

    // ========== Read (sync, called from blocking::unblock) ==========

    /// Get text content.
    pub fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        Ok(ffi::clipboard_get_text())
    }

    /// Get HTML content.
    pub fn get_html(&self) -> Result<Option<String>, ClipboardError> {
        Ok(ffi::clipboard_get_html())
    }

    /// Get file paths.
    pub fn get_files(&self) -> Result<Vec<PathBuf>, ClipboardError> {
        if let Some(url) = ffi::clipboard_get_file_url()
            && let Some(path) = url.strip_prefix("file://")
        {
            // URL decode the path
            let decoded = percent_encoding::percent_decode_str(path)
                .decode_utf8()
                .map_err(|e| ClipboardError::Platform(format!("Invalid URL encoding: {e}")))?;
            return Ok(vec![PathBuf::from(decoded.into_owned())]);
        }
        Ok(Vec::new())
    }

    /// Get image as RGBA.
    #[allow(clippy::cast_possible_truncation)] // Image dimensions from Swift are always valid u32
    pub fn get_image(&self) -> Result<Option<Image>, ClipboardError> {
        let image = ffi::clipboard_get_image();
        if !image.is_valid {
            return Ok(None);
        }
        Ok(Some(Image {
            width: image.width as u32,
            height: image.height as u32,
            bytes: image.bytes,
        }))
    }

    /// Get binary data by MIME type.
    pub fn get_binary(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        let data = ffi::clipboard_get_binary(mime.to_string());
        if !data.is_valid {
            return Ok(None);
        }
        Ok(Some(data.bytes))
    }

    // ========== Write (sync) ==========

    /// Set text content.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        ffi::clipboard_set_text(text.to_string());
        Ok(())
    }

    /// Set HTML content.
    pub fn set_html(&self, html: &str, alt_text: Option<&str>) -> Result<(), ClipboardError> {
        ffi::clipboard_set_html(html.to_string(), alt_text.unwrap_or("").to_string());
        Ok(())
    }

    /// Set file paths.
    pub fn set_files(&self, files: &[PathBuf]) -> Result<(), ClipboardError> {
        if files.is_empty() {
            return Ok(());
        }
        // iOS only supports single file URL
        let path = &files[0];
        let url = format!(
            "file://{}",
            percent_encoding::utf8_percent_encode(
                path.to_string_lossy().as_ref(),
                percent_encoding::NON_ALPHANUMERIC
            )
        );
        ffi::clipboard_set_file_url(url);
        Ok(())
    }

    /// Set image from a file path.
    pub fn set_image_from_path(&self, path: &Path) -> Result<(), ClipboardError> {
        let path_str = path.to_string_lossy().to_string();
        if !ffi::clipboard_set_image_from_path(path_str) {
            return Err(ClipboardError::InvalidImage(
                "failed to load image from path".into(),
            ));
        }
        Ok(())
    }

    /// Set binary data with MIME type.
    pub fn set_binary(&self, data: &[u8], mime: &str) -> Result<(), ClipboardError> {
        ffi::clipboard_set_binary(data.to_vec(), mime.to_string());
        Ok(())
    }

    /// Set file promise.
    ///
    /// On iOS, file promises are not fully supported, so this falls back
    /// to immediately calling the provider and setting the file URL.
    pub fn set_file_promise(
        &self,
        provider: Box<dyn FnOnce() -> Result<PathBuf, ClipboardError> + Send>,
    ) -> Result<(), ClipboardError> {
        let path = provider()?;
        self.set_files(&[path])
    }

    /// Clear clipboard.
    pub fn clear(&self) -> Result<(), ClipboardError> {
        ffi::clipboard_clear();
        Ok(())
    }
}

/// Start watching clipboard changes.
///
/// Uses polling with `UIPasteboard.changeCount`.
/// Returns a receiver and a stop flag.
pub fn start_watch()
-> Result<(async_channel::Receiver<ClipboardEvent>, Arc<AtomicBool>), ClipboardError> {
    let (sender, receiver) = async_channel::unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);

    thread::spawn(move || {
        let mut last_count = ffi::clipboard_get_change_count();

        while !stop_clone.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(500));

            let current_count = ffi::clipboard_get_change_count();
            if current_count != last_count {
                last_count = current_count;

                let event = ClipboardEvent {
                    has_text: ffi::clipboard_has_text(),
                    has_html: ffi::clipboard_has_html(),
                    has_files: ffi::clipboard_has_files(),
                    has_image: ffi::clipboard_has_image(),
                };
                if sender.try_send(event).is_err() {
                    break;
                }
            }
        }
    });

    Ok((receiver, stop))
}
