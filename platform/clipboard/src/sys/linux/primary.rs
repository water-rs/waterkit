//! The PRIMARY selection.

use super::formats;
use super::session::{self, DisplayServer, Session};
use super::wayland::{self, WaylandSelection};
use super::x11::X11Primary;
use super::{Backend as _, Primary};
use crate::error::ClipboardError;

/// Handle to the Linux PRIMARY selection.
#[derive(Debug)]
pub struct PrimaryInner {
    backend: PrimaryBackend,
}

#[derive(Debug)]
enum PrimaryBackend {
    /// Each operation connects to the compositor on its own.
    Wayland(WaylandSelection<Primary>),
    /// Boxed: the X11 handle is large, the Wayland variant empty.
    X11(Box<X11Primary>),
}

impl PrimaryInner {
    /// Choose the display server from the session and connect to it.
    pub fn new() -> Result<Self, ClipboardError> {
        let display_server = session::select::<Primary>(Session::current(), wayland::probe)?;
        tracing::debug!(?display_server, "PRIMARY selection backend chosen");
        let backend = match display_server {
            DisplayServer::Wayland => PrimaryBackend::Wayland(WaylandSelection::new()),
            DisplayServer::X11 => PrimaryBackend::X11(Box::new(X11Primary::connect()?)),
        };
        Ok(Self { backend })
    }

    /// Read the PRIMARY selection text, or `None` if it holds none.
    pub fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        match &self.backend {
            PrimaryBackend::Wayland(selection) => selection.text(),
            PrimaryBackend::X11(primary) => primary.get_text(),
        }
    }

    /// Write the PRIMARY selection text.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        match &self.backend {
            PrimaryBackend::Wayland(selection) => selection.offer(formats::text(text)),
            PrimaryBackend::X11(primary) => primary.set_text(text),
        }
    }
}
