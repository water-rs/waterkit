//! Linux PRIMARY selection via `arboard`.
//!
//! `arboard` covers both display servers: on X11 it claims the PRIMARY atom
//! and serves `SelectionRequest`s from a worker thread for as long as its
//! handle is alive; on Wayland (the `wayland-data-control` feature) it hands
//! the data to a forked child that answers data-control requests on its own.
//! Compositors without a data-control manager offering a primary selection
//! (`zwlr_data_control_manager_v1` version 2+, or `ext_data_control_manager_v1`)
//! do not expose PRIMARY to Wayland clients at all, and reads and writes
//! return an error there.

use std::sync::Mutex;

use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};

use crate::error::ClipboardError;

/// Handle to the Linux PRIMARY selection.
pub struct Primary {
    ctx: Mutex<arboard::Clipboard>,
}

impl std::fmt::Debug for Primary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Primary").finish_non_exhaustive()
    }
}

impl Primary {
    /// Connect to the selection backend (X11, or a Wayland data-control
    /// protocol when `WAYLAND_DISPLAY` is set and the compositor supports it).
    pub fn new() -> Result<Self, ClipboardError> {
        let ctx = arboard::Clipboard::new().map_err(|e| ClipboardError::Platform(e.to_string()))?;
        Ok(Self {
            ctx: Mutex::new(ctx),
        })
    }

    fn lock_ctx(&self) -> std::sync::MutexGuard<'_, arboard::Clipboard> {
        match self.ctx.lock() {
            Ok(ctx) => ctx,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Read the PRIMARY selection text, or `None` if it is empty.
    pub fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        let mut ctx = self.lock_ctx();
        match ctx.get().clipboard(LinuxClipboardKind::Primary).text() {
            Ok(text) => Ok(Some(text)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(ClipboardError::Platform(error.to_string())),
        }
    }

    /// Write the PRIMARY selection text.
    ///
    /// The crate keeps owning and serving the selection afterwards (see the
    /// module docs for the per-display-server lifetime).
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let mut ctx = self.lock_ctx();
        ctx.set()
            .clipboard(LinuxClipboardKind::Primary)
            .text(text.to_string())
            .map_err(|error| ClipboardError::Platform(error.to_string()))
    }
}
