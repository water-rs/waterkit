//! Choosing the display server that serves a selection.
//!
//! The choice is made once, before any operation, from what the session
//! provides. It never falls from one display server to the other: in a
//! Wayland session the selections are the compositor's, and an X server
//! reachable beside it (such as Xwayland) holds different ones.

use std::ffi::OsStr;

use wl_clipboard_rs::paste;

use super::Selection;
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

/// The data-control protocol a Wayland compositor offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataControl {
    /// Data-control for the regular clipboard only (`zwlr_data_control_manager_v1`
    /// version 1).
    Regular,
    /// Data-control with a primary selection (`ext_data_control_manager_v1`,
    /// or `zwlr_data_control_manager_v1` version 2+).
    WithPrimary,
}

/// The display server a selection is read and written through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayServer {
    /// A Wayland compositor's data-control protocol.
    Wayland,
    /// An X server.
    X11,
}

/// Choose the display server that serves the selection `S` in `session`.
///
/// `probe_data_control` binds the Wayland compositor's registry and reports
/// the data-control protocol it offers, or why it offers none. It runs only in
/// a Wayland session.
///
/// # Errors
///
/// [`ClipboardError::Platform`] when the session is a Wayland session whose
/// compositor does not offer `S` to data-control clients, whether or not an X
/// server is also reachable, and when the session has no display server at
/// all.
pub fn select<S: Selection>(
    session: Session,
    probe_data_control: impl FnOnce() -> Result<DataControl, String>,
) -> Result<DisplayServer, ClipboardError> {
    if session.wayland {
        return probe_data_control()
            .and_then(|data_control| {
                if S::WAYLAND == paste::ClipboardType::Primary
                    && data_control == DataControl::Regular
                {
                    Err(
                        "the compositor's data-control protocol does not provide a primary \
                         selection"
                            .to_owned(),
                    )
                } else {
                    Ok(DisplayServer::Wayland)
                }
            })
            .map_err(|reason| {
                ClipboardError::Platform(format!(
                    "{} is unavailable in this Wayland session: {reason}; X11 is never used \
                     while WAYLAND_DISPLAY is set",
                    S::NAME
                ))
            });
    }
    if session.x11 {
        return Ok(DisplayServer::X11);
    }
    Err(ClipboardError::Platform(format!(
        "{} needs a display server, but neither WAYLAND_DISPLAY nor DISPLAY is set",
        S::NAME
    )))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{DataControl, DisplayServer, Session, select};
    use crate::error::ClipboardError;
    use crate::sys::linux::{Clipboard, Primary, Selection};

    const WAYLAND: Option<&str> = Some("wayland-0");
    const X11: Option<&str> = Some(":0");
    const EMPTY: Option<&str> = Some("");

    fn session(wayland_display: Option<&str>, display: Option<&str>) -> Session {
        Session::new(wayland_display.map(OsStr::new), display.map(OsStr::new))
    }

    fn no_data_control() -> Result<DataControl, String> {
        Err("no data-control protocol".into())
    }

    fn not_probed() -> Result<DataControl, String> {
        panic!("the Wayland compositor was probed outside a Wayland session")
    }

    fn platform_reason(result: Result<DisplayServer, ClipboardError>) -> String {
        match result {
            Err(ClipboardError::Platform(reason)) => reason,
            other => panic!("expected ClipboardError::Platform, got {other:?}"),
        }
    }

    /// Declare a test that runs `$check` for every selection.
    macro_rules! for_each_selection {
        ($name:ident, $check:ident) => {
            #[test]
            fn $name() {
                $check::<Clipboard>();
                $check::<Primary>();
            }
        };
    }

    fn wayland_with_full_data_control<S: Selection>() {
        for display in [None, X11] {
            let selected = select::<S>(session(WAYLAND, display), || Ok(DataControl::WithPrimary));
            assert_eq!(selected.unwrap(), DisplayServer::Wayland);
        }
    }
    for_each_selection!(
        wayland_with_full_data_control_selects_wayland,
        wayland_with_full_data_control
    );

    fn wayland_without_data_control<S: Selection>() {
        for display in [None, X11] {
            let reason = platform_reason(select::<S>(session(WAYLAND, display), no_data_control));
            assert!(reason.starts_with(S::NAME), "{reason}");
            assert!(reason.contains("no data-control protocol"), "{reason}");
        }
    }
    for_each_selection!(
        wayland_without_data_control_never_selects_x11,
        wayland_without_data_control
    );

    #[test]
    fn data_control_without_primary_serves_only_clipboard() {
        let regular_only = || Ok(DataControl::Regular);
        for display in [None, X11] {
            let session = session(WAYLAND, display);
            assert_eq!(
                select::<Clipboard>(session, regular_only).unwrap(),
                DisplayServer::Wayland
            );
            let reason = platform_reason(select::<Primary>(session, regular_only));
            assert!(reason.starts_with("PRIMARY"), "{reason}");
            assert!(
                reason.contains("does not provide a primary selection"),
                "{reason}"
            );
        }
    }

    fn x11_alone<S: Selection>() {
        for wayland_display in [None, EMPTY] {
            let selected = select::<S>(session(wayland_display, X11), not_probed);
            assert_eq!(selected.unwrap(), DisplayServer::X11);
        }
    }
    for_each_selection!(x11_alone_selects_x11_without_probing_wayland, x11_alone);

    fn no_display_server<S: Selection>() {
        for (wayland_display, display) in [(None, None), (EMPTY, EMPTY), (None, EMPTY)] {
            let reason =
                platform_reason(select::<S>(session(wayland_display, display), not_probed));
            assert!(reason.starts_with(S::NAME), "{reason}");
            assert!(
                reason.contains("neither WAYLAND_DISPLAY nor DISPLAY"),
                "{reason}"
            );
        }
    }
    for_each_selection!(no_display_server_is_an_error, no_display_server);
}
