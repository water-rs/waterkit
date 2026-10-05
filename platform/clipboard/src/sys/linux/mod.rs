//! Linux selections: CLIPBOARD and PRIMARY.
//!
//! Each handle chooses its display server once, when it is created (see
//! [`session`]): a Wayland compositor's data-control protocol through
//! `wl-clipboard-rs` in a Wayland session, an X server otherwise. A Wayland
//! session whose compositor does not offer the selection to data-control
//! clients is an error; neither selection ever falls through to X11.
//!
//! On each display server both selections share one implementation, generic
//! over the [`Selection`]: [`wayland`] through `wl-clipboard-rs`, [`x11`]
//! through `x11rb`. Both implement [`Backend`].
//!
//! The formats a write offers and how a read is decoded live in [`formats`],
//! once for both display servers.

mod clipboard;
mod formats;
mod primary;
mod session;
mod wayland;
mod x11;

pub use clipboard::ClipboardInner;
pub use primary::PrimaryInner;

use wl_clipboard_rs::paste;

use crate::content::ClipboardEvent;
use crate::error::ClipboardError;
use formats::{Offered, Representation};

/// A Linux selection, as the type parameter of everything shared between
/// CLIPBOARD and PRIMARY.
pub trait Selection: 'static {
    /// The selection's name in messages.
    const NAME: &'static str;
    /// The selection on a Wayland data-control protocol.
    const WAYLAND: paste::ClipboardType;
}

/// The regular clipboard, written by copy and read by paste.
#[derive(Debug)]
pub struct Clipboard;

impl Selection for Clipboard {
    const NAME: &'static str = "CLIPBOARD";
    const WAYLAND: paste::ClipboardType = paste::ClipboardType::Regular;
}

/// The PRIMARY selection, written by selecting text and read by a middle
/// click.
#[derive(Debug)]
pub struct Primary;

impl Selection for Primary {
    const NAME: &'static str = "PRIMARY";
    const WAYLAND: paste::ClipboardType = paste::ClipboardType::Primary;
}

/// A selection on one display server, offering and reading formats by MIME
/// type (on X11, by target name).
pub trait Backend: std::fmt::Debug + Send + Sync {
    /// The formats the selection's owner offers, empty when it has no owner.
    fn mime_types(&self) -> Result<Vec<String>, ClipboardError>;

    /// Read the selection as `mime`, which its owner offered. `None` when the
    /// selection changed since and no longer offers it.
    fn read(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError>;

    /// Claim the selection with `representations`, each offered under its
    /// own MIME type.
    fn offer(&self, representations: Vec<Representation>) -> Result<(), ClipboardError>;

    /// Empty the selection.
    fn clear(&self) -> Result<(), ClipboardError>;

    /// Send an event to `sender` every time the selection changes, until the
    /// returned guard stops the watch or the receiver is dropped.
    fn watch(
        &self,
        sender: async_channel::Sender<ClipboardEvent>,
    ) -> Result<WatchGuard, ClipboardError>;

    /// The formats the selection offers.
    fn offered(&self) -> Result<Offered, ClipboardError> {
        self.mime_types().map(Offered::new)
    }

    /// Read the selection as `mime`, or `None` when it does not offer it.
    fn read_if_offered(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        if self.offered()?.has(mime) {
            self.read(mime)
        } else {
            Ok(None)
        }
    }

    /// Read the selection's text, or `None` when it offers no plain text.
    fn text(&self) -> Result<Option<String>, ClipboardError> {
        let offered = self.offered()?;
        let Some(target) = offered.text_target() else {
            return Ok(None);
        };
        self.read(target)?.map(formats::decode_text).transpose()
    }
}

/// Ends a running selection watch.
pub trait StopWatch: Send {
    /// End the watch. Its thread exits; a watch that already ended is left as
    /// it is.
    fn stop(&self);
}

/// Stops a selection watch when dropped.
pub struct WatchGuard(Box<dyn StopWatch>);

impl WatchGuard {
    /// A guard that runs `stop` when dropped.
    pub fn new(stop: impl StopWatch + 'static) -> Self {
        Self(Box::new(stop))
    }
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        self.0.stop();
    }
}
