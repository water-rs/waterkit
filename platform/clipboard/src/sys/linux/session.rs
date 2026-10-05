//! Choosing the display server that serves PRIMARY.
//!
//! The choice is made once, before any operation, from what the session
//! provides. It never falls from one display server to the other: in a
//! Wayland session PRIMARY is the compositor's, and an X server reachable
//! beside it (such as Xwayland) holds a different selection.

use std::ffi::OsStr;

use crate::error::ClipboardError;

/// The display-server variables of a process environment.
///
/// A variable that is set to the empty string counts as unset, so
/// `WAYLAND_DISPLAY= app` runs an application as an X11 client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    wayland: bool,
    x11: bool,
}

impl Session {
    /// The session this process runs in.
    pub fn current() -> Self {
        Self::new(
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
            std::env::var_os("DISPLAY").as_deref(),
        )
    }

    /// A session with the given `WAYLAND_DISPLAY` and `DISPLAY` values.
    pub fn new(wayland_display: Option<&OsStr>, display: Option<&OsStr>) -> Self {
        let is_set = |value: Option<&OsStr>| value.is_some_and(|value| !value.is_empty());
        Self {
            wayland: is_set(wayland_display),
            x11: is_set(display),
        }
    }
}

/// The display server PRIMARY is read and written through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayServer {
    /// A Wayland compositor's data-control protocol.
    Wayland,
    /// An X server.
    X11,
}

/// Choose the display server for `session`.
///
/// `probe_data_control` binds the Wayland compositor's registry and reports
/// whether it offers a data-control protocol with a primary selection, or why
/// it does not. It runs only in a Wayland session.
///
/// # Errors
///
/// [`ClipboardError::Platform`] when the session is a Wayland session whose
/// compositor offers no primary selection to data-control clients, whether or
/// not an X server is also reachable, and when the session has no display
/// server at all.
pub fn select(
    session: Session,
    probe_data_control: impl FnOnce() -> Result<(), String>,
) -> Result<DisplayServer, ClipboardError> {
    if session.wayland {
        return probe_data_control()
            .map(|()| DisplayServer::Wayland)
            .map_err(|reason| {
                ClipboardError::Platform(format!(
                    "PRIMARY is unavailable in this Wayland session: {reason}; X11 is never \
                     used while WAYLAND_DISPLAY is set"
                ))
            });
    }
    if session.x11 {
        return Ok(DisplayServer::X11);
    }
    Err(ClipboardError::Platform(
        "PRIMARY needs a display server, but neither WAYLAND_DISPLAY nor DISPLAY is set".into(),
    ))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{DisplayServer, Session, select};
    use crate::error::ClipboardError;

    const WAYLAND: Option<&str> = Some("wayland-0");
    const X11: Option<&str> = Some(":0");
    const EMPTY: Option<&str> = Some("");

    fn session(wayland_display: Option<&str>, display: Option<&str>) -> Session {
        Session::new(wayland_display.map(OsStr::new), display.map(OsStr::new))
    }

    fn no_data_control() -> Result<(), String> {
        Err("no data-control protocol".into())
    }

    fn not_probed() -> Result<(), String> {
        panic!("the Wayland compositor was probed outside a Wayland session")
    }

    fn platform_reason(result: Result<DisplayServer, ClipboardError>) -> String {
        match result {
            Err(ClipboardError::Platform(reason)) => reason,
            other => panic!("expected ClipboardError::Platform, got {other:?}"),
        }
    }

    #[test]
    fn wayland_with_data_control_selects_wayland() {
        for display in [None, X11] {
            let session = session(WAYLAND, display);
            assert_eq!(select(session, || Ok(())).unwrap(), DisplayServer::Wayland);
        }
    }

    #[test]
    fn wayland_without_data_control_never_selects_x11() {
        for display in [None, X11] {
            let reason = platform_reason(select(session(WAYLAND, display), no_data_control));
            assert!(reason.contains("no data-control protocol"), "{reason}");
        }
    }

    #[test]
    fn x11_alone_selects_x11_without_probing_wayland() {
        for wayland_display in [None, EMPTY] {
            let session = session(wayland_display, X11);
            assert_eq!(select(session, not_probed).unwrap(), DisplayServer::X11);
        }
    }

    #[test]
    fn no_display_server_is_an_error() {
        for (wayland_display, display) in [(None, None), (EMPTY, EMPTY), (None, EMPTY)] {
            let reason = platform_reason(select(session(wayland_display, display), not_probed));
            assert!(
                reason.contains("neither WAYLAND_DISPLAY nor DISPLAY"),
                "{reason}"
            );
        }
    }
}
