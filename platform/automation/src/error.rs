use core::fmt;

use keyboard_types::{Code, Modifiers};

use crate::{DisplayWindows, Point};

/// Failure creating a display or posting input.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A private framework, function, class or selector this crate needs is
    /// missing on this macOS. The value names it.
    #[error("{0} is unavailable on this macOS")]
    Unavailable(&'static str),
    /// A private method's type encoding differs from the one the call is
    /// written against, so calling it is refused.
    #[error("{method} has type encoding {found}, expected {expected}")]
    SignatureMismatch {
        /// The method.
        method: &'static str,
        /// The encoding the runtime reports.
        found: String,
        /// The encoding the call is written against.
        expected: &'static str,
    },
    /// The display's size in pixels (points times backing scale) does not
    /// fit in 32 bits.
    #[error("{width}x{height} points at {scale}x does not fit in 32-bit pixels")]
    SizeOverflow {
        /// Width in points.
        width: u32,
        /// Height in points.
        height: u32,
        /// Backing scale factor.
        scale: u32,
    },
    /// `CoreGraphics` refused to create the virtual display; the value names
    /// the call that refused.
    #[error("CoreGraphics refused the virtual display: {0}")]
    Refused(&'static str),
    /// `-[SLWindowMirroringManager extend:]` refused to bring the display
    /// into the topology.
    #[error("SLWindowMirroringManager refused to extend display {0}")]
    ExtendRefused(u32),
    /// The display did not end up online, active and outside any mirror set.
    #[error("virtual display is not online, active and unmirrored: {0}")]
    NotReady(DisplayState),
    /// No display with this id is online.
    #[error("display {0} is not online")]
    DisplayNotFound(u32),
    /// The point lies outside the display.
    #[error("{at} lies outside display {display}")]
    OutsideDisplay {
        /// The display.
        display: u32,
        /// The point, in the display's points.
        at: Point,
    },
    /// No application window is under the point.
    #[error("no application window is at {at} on {windows}")]
    NoWindow {
        /// The point, in the display's points.
        at: Point,
        /// The display and every window that intersected it.
        windows: DisplayWindows,
    },
    /// No application window is on the display to take key events.
    #[error("no application window can take key events on {0}")]
    NoKeyWindow(DisplayWindows),
    /// The key has no macOS virtual key code.
    #[error("key {0} has no macOS virtual key code")]
    UnmappableKey(Code),
    /// Some of the modifiers have no macOS event flag; the value holds them.
    #[error("modifiers {0:?} have no macOS event flag")]
    UnmappableModifiers(Modifiers),
    /// A platform call failed; the message names it and its status.
    #[error("platform error: {0}")]
    Platform(String),
}

/// What a virtual display looked like when it failed to come up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent predicate observed of the display"
)]
pub struct DisplayState {
    id: u32,
    reported_added: bool,
    registered: bool,
    window_server_online: bool,
    online: bool,
    active: bool,
    mirrored: bool,
    extended: bool,
}

impl DisplayState {
    #[expect(
        clippy::fn_params_excessive_bools,
        clippy::too_many_arguments,
        reason = "every flag is one observed predicate of the display, named at the call site"
    )]
    pub(crate) const fn new(
        id: u32,
        reported_added: bool,
        registered: bool,
        window_server_online: bool,
        online: bool,
        active: bool,
        mirrored: bool,
        extended: bool,
    ) -> Self {
        Self {
            id,
            reported_added,
            registered,
            window_server_online,
            online,
            active,
            mirrored,
            extended,
        }
    }

    /// The display's id.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// Whether a `CoreGraphics` reconfiguration callback reported the display
    /// added.
    #[must_use]
    pub const fn reported_added(&self) -> bool {
        self.reported_added
    }

    /// Whether `WindowServer` has registered the display (`SLSGetDisplayList`).
    #[must_use]
    pub const fn registered(&self) -> bool {
        self.registered
    }

    /// Whether `WindowServer` has brought the display into the topology
    /// (`SLSGetOnlineDisplayList`).
    #[must_use]
    pub const fn window_server_online(&self) -> bool {
        self.window_server_online
    }

    /// Whether `CoreGraphics` reports the display online in this process.
    #[must_use]
    pub const fn online(&self) -> bool {
        self.online
    }

    /// Whether `CoreGraphics` reports the display active.
    #[must_use]
    pub const fn active(&self) -> bool {
        self.active
    }

    /// Whether the display is in a mirror set.
    #[must_use]
    pub const fn mirrored(&self) -> bool {
        self.mirrored
    }

    /// Whether `-[SLWindowMirroringManager extend:]` was called for it.
    #[must_use]
    pub const fn extended(&self) -> bool {
        self.extended
    }
}

impl fmt::Display for DisplayState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "display {}: reported added {}, registered {}, WindowServer online {}, online {}, active {}, mirrored {}, extended {}",
            self.id,
            self.reported_added,
            self.registered,
            self.window_server_online,
            self.online,
            self.active,
            self.mirrored,
            self.extended,
        )
    }
}

/// A platform call's non-success status.
pub fn platform(call: &str, status: impl fmt::Debug) -> Error {
    Error::Platform(format!("{call} failed with {status:?}"))
}
