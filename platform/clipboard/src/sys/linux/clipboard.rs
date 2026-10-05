//! The regular clipboard, CLIPBOARD.

use std::path::{Path, PathBuf};

use super::formats::{self, Offered, Representation};
use super::session::{self, DisplayServer, Session};
use super::wayland::{self, WaylandSelection};
use super::x11::X11Clipboard;
use super::{Backend, Clipboard, WatchGuard};
use crate::content::{ClipboardEvent, Image};
use crate::error::ClipboardError;

/// Handle to the Linux CLIPBOARD.
#[derive(Debug)]
pub struct ClipboardInner {
    backend: Box<dyn Backend>,
}

impl ClipboardInner {
    /// Choose the display server from the session and connect to it.
    pub fn new() -> Result<Self, ClipboardError> {
        let display_server = session::select::<Clipboard>(Session::current(), wayland::probe)?;
        tracing::debug!(?display_server, "CLIPBOARD backend chosen");
        let backend: Box<dyn Backend> = match display_server {
            DisplayServer::Wayland => Box::new(WaylandSelection::<Clipboard>::new()),
            DisplayServer::X11 => Box::new(X11Clipboard::connect()?),
        };
        Ok(Self { backend })
    }

    // ========== Query (sync) ==========

    /// The formats CLIPBOARD offers. The queries answer with a `bool`, so a
    /// failure to list them is logged and reads as offering none.
    fn offered(&self) -> Offered {
        self.backend.offered().unwrap_or_else(|error| {
            tracing::error!(%error, "listing the CLIPBOARD formats failed; reporting none");
            Offered::default()
        })
    }

    /// Check if text is available.
    pub fn has_text(&self) -> bool {
        self.offered().has_text()
    }

    /// Check if HTML is available.
    pub fn has_html(&self) -> bool {
        self.offered().has_html()
    }

    /// Check if files are available.
    pub fn has_files(&self) -> bool {
        self.offered().has_files()
    }

    /// Check if image is available.
    pub fn has_image(&self) -> bool {
        self.offered().has_image()
    }

    // ========== Read (sync, called from blocking::unblock) ==========

    /// Get text content.
    pub fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        self.backend.text()
    }

    /// Get HTML content.
    pub fn get_html(&self) -> Result<Option<String>, ClipboardError> {
        self.backend
            .read_if_offered(formats::HTML)?
            .map(formats::decode_text)
            .transpose()
    }

    /// Get file paths.
    pub fn get_files(&self) -> Result<Vec<PathBuf>, ClipboardError> {
        self.backend
            .read_if_offered(formats::URI_LIST)?
            .map_or_else(|| Ok(Vec::new()), |list| formats::decode_uri_list(&list))
    }

    /// Get image as RGBA.
    pub fn get_image(&self) -> Result<Option<Image>, ClipboardError> {
        self.backend
            .read_if_offered(formats::PNG)?
            .map(|png| formats::decode_png(&png))
            .transpose()
    }

    /// Get binary data by MIME type.
    pub fn get_binary(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        self.backend.read_if_offered(mime)
    }

    // ========== Write (sync) ==========

    /// Set text content.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        self.backend.offer(formats::text(text))
    }

    /// Set HTML content, with `alt_text` as its plain-text form.
    pub fn set_html(&self, html: &str, alt_text: Option<&str>) -> Result<(), ClipboardError> {
        self.backend.offer(formats::html(html, alt_text))
    }

    /// Set file paths.
    pub fn set_files(&self, files: &[PathBuf]) -> Result<(), ClipboardError> {
        self.backend.offer(formats::files(files)?)
    }

    /// Set image from a file path.
    pub fn set_image_from_path(&self, path: &Path) -> Result<(), ClipboardError> {
        let image = image::open(path).map_err(|error| {
            ClipboardError::InvalidImage(format!("failed to load {}: {error}", path.display()))
        })?;
        self.backend.offer(formats::png(&image)?)
    }

    /// Set binary data with MIME type.
    pub fn set_binary(&self, data: &[u8], mime: &str) -> Result<(), ClipboardError> {
        self.backend.offer(vec![Representation::new(mime, data)])
    }

    /// Set file promise.
    ///
    /// Neither display server has lazy file providers, so the provider runs
    /// now and its file is set.
    pub fn set_file_promise(
        &self,
        provider: Box<dyn FnOnce() -> Result<PathBuf, ClipboardError> + Send>,
    ) -> Result<(), ClipboardError> {
        let path = provider()?;
        self.set_files(&[path])
    }

    /// Clear clipboard.
    pub fn clear(&self) -> Result<(), ClipboardError> {
        self.backend.clear()
    }

    /// Watch CLIPBOARD for changes on this handle's display server.
    pub fn watch(
        &self,
    ) -> Result<(async_channel::Receiver<ClipboardEvent>, WatchGuard), ClipboardError> {
        let (sender, receiver) = async_channel::unbounded();
        let guard = self.backend.watch(sender)?;
        Ok((receiver, guard))
    }
}
