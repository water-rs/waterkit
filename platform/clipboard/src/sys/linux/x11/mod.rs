//! The selections of the X server named by `DISPLAY`.
//!
//! CLIPBOARD goes through `clipboard-rs`, which serves several formats at
//! once, and is watched through `XFixes`; PRIMARY, which only ever holds
//! text, goes through `x11-clipboard`.

mod clipboard;
mod primary;
mod watch;

pub use clipboard::X11Clipboard;
pub use primary::X11Primary;

use x11rb::protocol::xproto::{Atom, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;

use crate::error::ClipboardError;

/// Whether a client owns `selection`.
fn owned(connection: &RustConnection, selection: Atom) -> Result<bool, ClipboardError> {
    let owner = connection
        .get_selection_owner(selection)
        .map_err(|error| platform(&error))?
        .reply()
        .map_err(|error| platform(&error))?
        .owner;
    Ok(owner != x11rb::NONE)
}

fn platform(error: &dyn std::error::Error) -> ClipboardError {
    ClipboardError::Platform(error.to_string())
}
