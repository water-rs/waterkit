//! Watching an X selection through `XFixes` selection notifications.
//!
//! The watch subscribes before [`watch`] returns, so no change made after it
//! returns is missed. Its thread blocks on the connection until the X server
//! reports an event; stopping it sends that thread a client message.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::thread;

use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{EventMask, Window};
use x11rb::rust_connection::RustConnection;

use super::{connect, intern, platform, wake};
use crate::error::ClipboardError;
use crate::sys::linux::{StopWatch, WatchGuard};

/// The `XFixes` version whose selection notifications this uses.
const XFIXES_VERSION: (u32, u32) = (5, 0);

/// Call `on_change` on a thread of its own every time a client claims the
/// selection named `selection`, or its owner goes away, until `on_change`
/// breaks, the X server connection fails, or the returned guard is dropped.
pub fn watch(
    selection: &'static str,
    mut on_change: impl FnMut() -> ControlFlow<()> + Send + 'static,
) -> Result<WatchGuard, ClipboardError> {
    // The notifications and the stop message are delivered to this window.
    let (connection, window) = connect(EventMask::NO_EVENT)?;
    let selection_atom = intern(&connection, selection)?;
    connection
        .xfixes_query_version(XFIXES_VERSION.0, XFIXES_VERSION.1)
        .map_err(|error| platform(&error))?
        .reply()
        .map_err(|error| platform(&error))?;
    connection
        .xfixes_select_selection_input(
            window,
            selection_atom,
            xfixes::SelectionEventMask::SET_SELECTION_OWNER
                | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
                | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )
        .map_err(|error| platform(&error))?
        .check()
        .map_err(|error| platform(&error))?;

    let connection = Arc::new(connection);
    let watched = Arc::clone(&connection);
    thread::Builder::new()
        .name(format!("waterkit {selection} watch"))
        .spawn(move || {
            loop {
                match watched.wait_for_event() {
                    Ok(Event::XfixesSelectionNotify(_)) => {
                        if on_change().is_break() {
                            return;
                        }
                    }
                    Ok(Event::ClientMessage(message)) if message.window == window => return,
                    Ok(_) => {}
                    Err(error) => {
                        tracing::error!(
                            selection,
                            %error,
                            "watching the X11 selection failed; the watch stream ends"
                        );
                        return;
                    }
                }
            }
        })
        .map_err(|error| platform(&error))?;
    Ok(WatchGuard::new(Stop { connection, window }))
}

/// Stops an X11 watch by waking its thread with a client message.
struct Stop {
    connection: Arc<RustConnection>,
    window: Window,
}

impl StopWatch for Stop {
    fn stop(&self) {
        if let Err(error) = wake(&self.connection, self.window) {
            // The connection is gone, and with it the watch thread.
            tracing::debug!(%error, "the X11 watch connection closed before the watch was stopped");
        }
    }
}
