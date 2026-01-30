//! Desktop clipboard implementation using clipboard-rs.
//!
//! Supports Windows, Linux, and macOS.

#![allow(clippy::unnecessary_wraps)] // API consistency with other platforms
#![allow(clippy::needless_pass_by_ref_mut)] // API consistency with other platforms

use crate::content::{ClipboardEvent, Image};
use crate::error::ClipboardError;
use clipboard_rs::common::RustImage;
use clipboard_rs::{
    Clipboard, ClipboardContext, ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext,
    WatcherShutdown,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;

/// Desktop clipboard handle.
///
/// Note: `ClipboardContext` is not Send, so we wrap operations in a Mutex
/// and create new contexts as needed for thread safety.
pub struct ClipboardInner {
    ctx: Mutex<ClipboardContext>,
}

impl std::fmt::Debug for ClipboardInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardInner").finish_non_exhaustive()
    }
}

impl ClipboardInner {
    /// Create a new clipboard handle.
    pub fn new() -> Result<Self, ClipboardError> {
        let ctx = ClipboardContext::new().map_err(|e| ClipboardError::Platform(e.to_string()))?;
        Ok(Self {
            ctx: Mutex::new(ctx),
        })
    }

    // ========== Query (sync) ==========

    /// Check if text is available.
    pub fn has_text(&self) -> bool {
        let ctx = self.ctx.lock().unwrap();
        ctx.has(clipboard_rs::ContentFormat::Text)
    }

    /// Check if HTML is available.
    pub fn has_html(&self) -> bool {
        let ctx = self.ctx.lock().unwrap();
        ctx.has(clipboard_rs::ContentFormat::Html)
    }

    /// Check if files are available.
    pub fn has_files(&self) -> bool {
        let ctx = self.ctx.lock().unwrap();
        ctx.available_formats()
            .is_ok_and(|formats| formats.iter().any(|f| f.to_lowercase().contains("file")))
    }

    /// Check if image is available.
    pub fn has_image(&self) -> bool {
        let ctx = self.ctx.lock().unwrap();
        ctx.has(clipboard_rs::ContentFormat::Image)
    }

    // ========== Read (sync, called from blocking::unblock) ==========

    /// Get text content.
    pub fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        let ctx = self.ctx.lock().unwrap();
        Ok(ctx.get_text().ok())
    }

    /// Get HTML content.
    pub fn get_html(&self) -> Result<Option<String>, ClipboardError> {
        let ctx = self.ctx.lock().unwrap();
        Ok(ctx.get_html().ok())
    }

    /// Get file paths.
    pub fn get_files(&self) -> Result<Vec<PathBuf>, ClipboardError> {
        let ctx = self.ctx.lock().unwrap();
        Ok(ctx
            .get_files()
            .map(|files| files.into_iter().map(PathBuf::from).collect())
            .unwrap_or_default())
    }

    /// Get image as RGBA.
    pub fn get_image(&self) -> Result<Option<Image>, ClipboardError> {
        let ctx = self.ctx.lock().unwrap();
        match ctx.get_image() {
            Ok(img) => {
                let (width, height) = img.get_size();
                let rgba = img
                    .to_rgba8()
                    .map_err(|e| ClipboardError::InvalidImage(format!("{e}")))?;
                Ok(Some(Image {
                    width,
                    height,
                    bytes: rgba.into_raw(),
                }))
            }
            Err(_) => Ok(None),
        }
    }

    /// Get binary data by MIME type.
    pub fn get_binary(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        let ctx = self.ctx.lock().unwrap();
        Ok(ctx.get_buffer(mime).ok())
    }

    // ========== Write (sync) ==========

    /// Set text content.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let ctx = self.ctx.lock().unwrap();
        ctx.set_text(text.to_string())
            .map_err(|e| ClipboardError::Platform(e.to_string()))
    }

    /// Set HTML content.
    pub fn set_html(&self, html: &str, alt_text: Option<&str>) -> Result<(), ClipboardError> {
        let ctx = self.ctx.lock().unwrap();
        // Set plain text first if alt_text provided
        if let Some(alt) = alt_text {
            let _ = ctx.set_text(alt.to_string());
        }
        ctx.set_html(html.to_string())
            .map_err(|e| ClipboardError::Platform(e.to_string()))
    }

    /// Set file paths.
    pub fn set_files(&self, files: &[PathBuf]) -> Result<(), ClipboardError> {
        let ctx = self.ctx.lock().unwrap();
        let file_strings: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        ctx.set_files(file_strings)
            .map_err(|e| ClipboardError::Platform(e.to_string()))
    }

    /// Set image from a file path.
    pub fn set_image_from_path(&self, path: &Path) -> Result<(), ClipboardError> {
        use image::DynamicImage;

        // Load image from file
        let img = image::open(path)
            .map_err(|e| ClipboardError::InvalidImage(format!("failed to load image: {e}")))?;

        // Convert to RGBA8
        let rgba = img.to_rgba8();

        // Create RgbaImage and convert to clipboard format
        let dynamic_image = DynamicImage::ImageRgba8(rgba);
        let rust_image = clipboard_rs::common::RustImageData::from_dynamic_image(dynamic_image);

        let ctx = self.ctx.lock().unwrap();
        ctx.set_image(rust_image)
            .map_err(|e| ClipboardError::Platform(e.to_string()))
    }

    /// Set binary data with MIME type.
    pub fn set_binary(&self, data: &[u8], mime: &str) -> Result<(), ClipboardError> {
        let ctx = self.ctx.lock().unwrap();
        ctx.set_buffer(mime, data.to_vec())
            .map_err(|e| ClipboardError::Platform(e.to_string()))
    }

    /// Set file promise.
    ///
    /// On desktop, this falls back to immediately calling the provider
    /// since clipboard-rs doesn't support lazy providers.
    pub fn set_file_promise(
        &self,
        provider: Box<dyn FnOnce() -> Result<PathBuf, ClipboardError> + Send>,
    ) -> Result<(), ClipboardError> {
        // Desktop fallback: call provider immediately
        let path = provider()?;
        self.set_files(&[path])
    }

    /// Clear clipboard.
    pub fn clear(&self) -> Result<(), ClipboardError> {
        let ctx = self.ctx.lock().unwrap();
        ctx.clear()
            .map_err(|e| ClipboardError::Platform(e.to_string()))
    }
}

/// Handler for clipboard change events that sends to an async channel.
struct WatchHandler {
    sender: async_channel::Sender<ClipboardEvent>,
    ctx: ClipboardContext,
}

impl ClipboardHandler for WatchHandler {
    fn on_clipboard_change(&mut self) {
        let event = ClipboardEvent {
            has_text: self.ctx.has(clipboard_rs::ContentFormat::Text),
            has_html: self.ctx.has(clipboard_rs::ContentFormat::Html),
            has_files: self
                .ctx
                .available_formats()
                .is_ok_and(|f| f.iter().any(|s| s.to_lowercase().contains("file"))),
            has_image: self.ctx.has(clipboard_rs::ContentFormat::Image),
        };
        // Try to send, ignore if receiver dropped
        let _ = self.sender.try_send(event);
    }
}

/// Start watching clipboard changes.
///
/// Returns a receiver that yields `ClipboardEvent`s and a shutdown handle.
pub fn start_watch()
-> Result<(async_channel::Receiver<ClipboardEvent>, WatcherShutdown), ClipboardError> {
    let (sender, receiver) = async_channel::unbounded();

    let ctx = ClipboardContext::new().map_err(|e| ClipboardError::Platform(e.to_string()))?;
    let handler = WatchHandler { sender, ctx };

    let mut watcher =
        ClipboardWatcherContext::new().map_err(|e| ClipboardError::Platform(e.to_string()))?;

    let shutdown = watcher.add_handler(handler).get_shutdown_channel();

    // Spawn watcher thread
    thread::spawn(move || {
        watcher.start_watch();
    });

    Ok((receiver, shutdown))
}
