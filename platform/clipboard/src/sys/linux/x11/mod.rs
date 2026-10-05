//! The selections of the X server named by `DISPLAY`, via `x11rb`.
//!
//! [`X11Selection`] reads, owns and serves CLIPBOARD and PRIMARY alike, in
//! every format a write offers; [`watch`] follows a selection through
//! `XFixes`.

mod selection;
mod watch;

pub use selection::X11Selection;

use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection as _;
use x11rb::errors::ConnectionError;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::error::ClipboardError;

/// Connect to the X server named by `DISPLAY` and create an input-only window
/// that receives `events`. Selection events, and the client message that
/// [`wake`] sends, reach the window whatever `events` holds.
fn connect(events: EventMask) -> Result<(RustConnection, Window), ClipboardError> {
    let (connection, screen) = x11rb::connect(None).map_err(|error| platform(&error))?;
    let root = connection.setup().roots[screen].root;
    let window = connection.generate_id().map_err(|error| platform(&error))?;
    connection
        .create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(events),
        )
        .map_err(|error| platform(&error))?
        .check()
        .map_err(|error| platform(&error))?;
    Ok((connection, window))
}

/// Intern the atom named `name`.
fn intern(connection: &RustConnection, name: &str) -> Result<Atom, ClipboardError> {
    Ok(connection
        .intern_atom(false, name.as_bytes())
        .map_err(|error| platform(&error))?
        .reply()
        .map_err(|error| platform(&error))?
        .atom)
}

/// Send `window`, which `connection` created, a client message. A thread
/// blocked reading `connection`'s events wakes up with it.
fn wake(connection: &RustConnection, window: Window) -> Result<(), ConnectionError> {
    let message = ClientMessageEvent::new(32, window, AtomEnum::NONE, [0; 5]);
    connection.send_event(false, window, EventMask::NO_EVENT, message)?;
    connection.flush()
}

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
