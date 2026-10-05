//! PRIMARY through an X server, via `x11-clipboard`.
//!
//! The handle keeps two connections to the X server named by `DISPLAY`: one
//! reads selections, and a worker thread on the other serves the
//! `SelectionRequest`s for text this handle wrote, until another client claims
//! PRIMARY or the handle is dropped.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use x11rb::protocol::xproto::ConnectionExt as _;

use crate::error::ClipboardError;

/// How long a read waits for the owner of PRIMARY to answer. The ICCCM sets no
/// deadline, so an owner that never answers would otherwise block the read
/// forever.
const READ_TIMEOUT: Duration = Duration::from_secs(4);

/// PRIMARY of an X server.
pub struct X11Primary {
    /// Reads share one connection, whose events answer one conversion at a
    /// time, so they are serialized.
    clipboard: Mutex<x11_clipboard::Clipboard>,
}

impl std::fmt::Debug for X11Primary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Primary").finish_non_exhaustive()
    }
}

impl X11Primary {
    /// Connect to the X server named by `DISPLAY`.
    pub fn connect() -> Result<Self, ClipboardError> {
        let clipboard = x11_clipboard::Clipboard::new().map_err(|error| platform(&error))?;
        Ok(Self {
            clipboard: Mutex::new(clipboard),
        })
    }

    fn lock(&self) -> MutexGuard<'_, x11_clipboard::Clipboard> {
        self.clipboard
            .lock()
            .expect("a PRIMARY operation panicked while it held the X11 connection")
    }

    /// Read the PRIMARY text, or `None` when no client owns PRIMARY.
    ///
    /// An owner that cannot convert PRIMARY to `UTF8_STRING` refuses the
    /// conversion, which `x11-clipboard` reports as empty data; that reads as
    /// an empty string.
    pub fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        let bytes = {
            let clipboard = self.lock();
            let reader = &clipboard.getter;
            let owner = reader
                .connection
                .get_selection_owner(reader.atoms.primary)
                .map_err(|error| platform(&error))?
                .reply()
                .map_err(|error| platform(&error))?
                .owner;
            if owner == x11rb::NONE {
                return Ok(None);
            }
            clipboard
                .load(
                    reader.atoms.primary,
                    reader.atoms.utf8_string,
                    reader.atoms.property,
                    READ_TIMEOUT,
                )
                .map_err(|error| match error {
                    x11_clipboard::error::Error::Timeout => ClipboardError::Platform(format!(
                        "the owner of PRIMARY did not answer within {READ_TIMEOUT:?}"
                    )),
                    error => platform(&error),
                })?
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| ClipboardError::Decode(error.to_string()))
    }

    /// Claim PRIMARY with `text`.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let clipboard = self.lock();
        let atoms = &clipboard.setter.atoms;
        clipboard
            .store(atoms.primary, atoms.utf8_string, text.as_bytes())
            .map_err(|error| platform(&error))
    }
}

fn platform(error: &dyn std::error::Error) -> ClipboardError {
    ClipboardError::Platform(error.to_string())
}
