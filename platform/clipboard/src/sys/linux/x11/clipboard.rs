//! CLIPBOARD through an X server, via `clipboard-rs`.
//!
//! `clipboard-rs` keeps two connections to the X server named by `DISPLAY`:
//! one reads, and a thread on the other serves the formats this process wrote
//! until another client claims CLIPBOARD. That thread outlives the handle and
//! ends with the process. A third connection, this module's own, asks who
//! owns CLIPBOARD, which `clipboard-rs` does not expose.
//!
//! `clipboard-rs` is built without its `wayland` feature, which would make its
//! constructor fall back from Wayland to X11 on its own. The irrefutable
//! pattern on its context in [`X11Clipboard::connect`] is the proof: with the
//! feature on, the context gains a `Wayland` variant and this module stops
//! compiling.
//!
//! Changes are watched through `XFixes` directly ([`super::watch`]) rather
//! than through `clipboard-rs`'s watcher, which subscribes only after its
//! thread starts, so a change right after the watch was created could be
//! missed.

use std::ops::ControlFlow;
use std::sync::{Arc, Mutex, MutexGuard};

use clipboard_rs::{Clipboard as _, ClipboardContent, ClipboardContext};
use x11rb::protocol::xproto::{Atom, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;

use super::{owned, platform, watch};
use crate::content::ClipboardEvent;
use crate::error::ClipboardError;
use crate::sys::linux::formats::{Offered, Representation};
use crate::sys::linux::{Backend, WatchGuard};

/// CLIPBOARD of an X server.
///
/// Clones share the connections, so a watch reads through the handle that
/// started it instead of opening connections whose serving thread would
/// outlive the watch.
#[derive(Clone)]
pub struct X11Clipboard {
    /// Reads share one connection, whose events answer one conversion at a
    /// time, so they are serialized.
    context: Arc<Mutex<ClipboardContext>>,
    /// Asks who owns CLIPBOARD: converting an unowned selection fails in
    /// `clipboard-rs` the same way as a refused conversion.
    owner: Arc<Owner>,
}

/// A connection that asks who owns CLIPBOARD.
struct Owner {
    connection: RustConnection,
    clipboard: Atom,
}

impl std::fmt::Debug for X11Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Clipboard").finish_non_exhaustive()
    }
}

impl X11Clipboard {
    /// Connect to the X server named by `DISPLAY`.
    pub fn connect() -> Result<Self, ClipboardError> {
        let context = ClipboardContext::new().map_err(|error| platform(&*error))?;
        // Irrefutable only while `clipboard-rs`'s `wayland` feature is off;
        // see the module docs.
        let ClipboardContext::X11(_) = &context;
        let (connection, _) = x11rb::connect(None).map_err(|error| platform(&error))?;
        let clipboard = connection
            .intern_atom(false, b"CLIPBOARD")
            .map_err(|error| platform(&error))?
            .reply()
            .map_err(|error| platform(&error))?
            .atom;
        Ok(Self {
            context: Arc::new(Mutex::new(context)),
            owner: Arc::new(Owner {
                connection,
                clipboard,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, ClipboardContext> {
        self.context
            .lock()
            .expect("a CLIPBOARD operation panicked while it held the X11 connection")
    }
}

impl Backend for X11Clipboard {
    fn mime_types(&self) -> Result<Vec<String>, ClipboardError> {
        if !owned(&self.owner.connection, self.owner.clipboard)? {
            return Ok(Vec::new());
        }
        self.lock()
            .available_formats()
            .map_err(|error| platform(&*error))
    }

    fn read(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        self.lock()
            .get_buffer(mime)
            .map(Some)
            .map_err(|error| platform(&*error))
    }

    fn offer(&self, representations: Vec<Representation>) -> Result<(), ClipboardError> {
        let contents = representations
            .into_iter()
            .map(|representation| {
                ClipboardContent::Other(representation.mime, representation.bytes)
            })
            .collect();
        self.lock().set(contents).map_err(|error| platform(&*error))
    }

    fn clear(&self) -> Result<(), ClipboardError> {
        self.lock().clear().map_err(|error| platform(&*error))
    }

    fn watch(
        &self,
        sender: async_channel::Sender<ClipboardEvent>,
    ) -> Result<WatchGuard, ClipboardError> {
        let clipboard = self.clone();
        watch::watch("CLIPBOARD", move || match clipboard.mime_types() {
            Ok(mime_types) => match sender.try_send(Offered::new(mime_types).event()) {
                Ok(()) => ControlFlow::Continue(()),
                // The stream was dropped.
                Err(_) => ControlFlow::Break(()),
            },
            Err(error) => {
                tracing::error!(
                    %error,
                    "listing the X11 CLIPBOARD formats after a change failed; the watch stream ends"
                );
                ControlFlow::Break(())
            }
        })
    }
}
