//! Linux PRIMARY selection.
//!
//! The display server is chosen once, when the handle is created (see
//! [`session`]): a Wayland compositor's data-control protocol through
//! `wl-clipboard-rs` in a Wayland session, an X server through `x11-clipboard`
//! otherwise. A Wayland session whose compositor offers no primary selection
//! to data-control clients is an error; it never falls through to X11.

mod session;
mod wayland;
mod x11;

use session::{DisplayServer, Session};
use x11::X11Primary;

use crate::error::ClipboardError;

/// Handle to the Linux PRIMARY selection.
#[derive(Debug)]
pub struct Primary {
    backend: Backend,
}

#[derive(Debug)]
enum Backend {
    /// Each operation connects to the compositor on its own.
    Wayland,
    /// Boxed: the X11 handle is large, the Wayland variant empty.
    X11(Box<X11Primary>),
}

impl Primary {
    /// Choose the display server from the session and connect to it.
    pub fn new() -> Result<Self, ClipboardError> {
        let display_server = session::select(Session::current(), wayland::probe_data_control)?;
        tracing::debug!(?display_server, "PRIMARY selection backend chosen");
        let backend = match display_server {
            DisplayServer::Wayland => Backend::Wayland,
            DisplayServer::X11 => Backend::X11(Box::new(X11Primary::connect()?)),
        };
        Ok(Self { backend })
    }

    /// Read the PRIMARY selection text, or `None` if it holds none.
    pub fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        match &self.backend {
            Backend::Wayland => wayland::get_text(),
            Backend::X11(primary) => primary.get_text(),
        }
    }

    /// Write the PRIMARY selection text.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        match &self.backend {
            Backend::Wayland => wayland::set_text(text),
            Backend::X11(primary) => primary.set_text(text),
        }
    }
}
