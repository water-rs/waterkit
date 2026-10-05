//! The PRIMARY selection.

use super::formats;
use super::session::{self, DisplayServer, Session};
use super::wayland::{self, WaylandSelection};
use super::x11::X11Selection;
use super::{Backend, Primary};
use crate::error::ClipboardError;

/// Handle to the Linux PRIMARY selection.
#[derive(Debug)]
pub struct PrimaryInner {
    backend: Box<dyn Backend>,
}

impl PrimaryInner {
    /// Choose the display server from the session and connect to it.
    pub fn new() -> Result<Self, ClipboardError> {
        let display_server = session::select::<Primary>(Session::current(), wayland::probe)?;
        tracing::debug!(?display_server, "PRIMARY selection backend chosen");
        let backend: Box<dyn Backend> = match display_server {
            DisplayServer::Wayland => Box::new(WaylandSelection::<Primary>::new()),
            DisplayServer::X11 => Box::new(X11Selection::<Primary>::connect()?),
        };
        Ok(Self { backend })
    }

    /// Read the PRIMARY selection text, or `None` if it holds none.
    pub fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        self.backend.text()
    }

    /// Write the PRIMARY selection text.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        self.backend.offer(formats::text(text))
    }
}
