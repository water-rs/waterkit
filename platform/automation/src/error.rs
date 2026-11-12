use core::fmt;
use core::time::Duration;
use std::path::PathBuf;

use keyboard_types::{Code, Modifiers};

use crate::{DisplayWindows, FocusReport, ListedWindow, Point, Rect};

/// Failure creating a display, posting input or launching an application.
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
    /// The system's headless fallback display runs a mode a virtual display
    /// cannot reproduce, so no stand-in can take its place.
    #[error(
        "the headless fallback display {display} runs {width}x{height} points on {pixels_wide}x{pixels_high} pixels at {refresh_rate} Hz, which no virtual display reproduces"
    )]
    UnmatchableMode {
        /// The fallback display.
        display: u32,
        /// Width in points.
        width: usize,
        /// Height in points.
        height: usize,
        /// Width in pixels.
        pixels_wide: usize,
        /// Height in pixels.
        pixels_high: usize,
        /// Refresh rate in hertz.
        refresh_rate: f64,
    },
    /// The system kept its headless fallback display online after the
    /// stand-in that replaces it came up.
    #[error("the headless fallback display {fallback} stayed online beside stand-in {stand_in}")]
    FallbackStayed {
        /// The fallback display.
        fallback: u32,
        /// The stand-in display.
        stand_in: u32,
    },
    /// The display that is to stay the user's main display while a virtual
    /// display comes up did not become main.
    #[error("display {display} did not become the main display; display {main} is")]
    NotMain {
        /// The display that is to be main.
        display: u32,
        /// The display that is main.
        main: u32,
    },
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
    /// No window takes a press at the point.
    #[error("no window takes a press at {at} on {windows}")]
    NoWindow {
        /// The point, in the display's points.
        at: Point,
        /// The display and every window that intersected it.
        windows: DisplayWindows,
    },
    /// The display, or the window under the point, kept moving: no reading
    /// within the timeout saw the display stay put while `WindowServer`
    /// listed the window that takes a press at the point around it.
    #[error("display {display} or the window under {at} on it kept moving for {timeout:?}")]
    Unsettled {
        /// The display.
        display: u32,
        /// The point, in the display's points.
        at: Point,
        /// How long the geometry was given to settle.
        timeout: Duration,
    },
    /// The window that takes a press at the point belongs to no application,
    /// such as `WindowServer`'s menu bar, so no process can be handed the
    /// event.
    #[error("the window that takes a press at {at} is {window}, which no application owns")]
    NotApplicationWindow {
        /// The point, in the display's points.
        at: Point,
        /// The window.
        window: ListedWindow,
    },
    /// No application window is on the display to take key events.
    #[error("no application window can take key events on {0}")]
    NoKeyWindow(DisplayWindows),
    /// The application owning the target window did not come to report
    /// itself active with that window focused, so it would drop the input;
    /// nothing but the activation event and the press that makes the window
    /// key was sent.
    #[error(
        "pid {pid} did not make window {window} its focused window within {timeout:?}: it {report}"
    )]
    NotFocused {
        /// The owning process.
        pid: libc::pid_t,
        /// The target window.
        window: u32,
        /// How long the application was given.
        timeout: Duration,
        /// What the application reported last.
        report: FocusReport,
    },
    /// The application owning the target window does not post an
    /// Accessibility notification the focus wait is woken by, so it cannot
    /// tell when the application has focused the window; nothing was sent.
    #[error(
        "pid {pid} does not support the Accessibility notification {notification}, which the wait for it to focus window {window} needs"
    )]
    NotificationUnsupported {
        /// The owning process.
        pid: libc::pid_t,
        /// The target window.
        window: u32,
        /// The notification it refused.
        notification: &'static str,
    },
    /// `WindowServer` did not route every posted event into the target
    /// process; some of them may have arrived.
    #[error(
        "{routed} of {posted} events posted for window {window} reached pid {pid} within {timeout:?}"
    )]
    NotDelivered {
        /// The target process.
        pid: libc::pid_t,
        /// The target window.
        window: u32,
        /// Events posted.
        posted: usize,
        /// Events that reached the process.
        routed: usize,
        /// How long they were given.
        timeout: Duration,
    },
    /// The key has no macOS virtual key code.
    #[error("key {0} has no macOS virtual key code")]
    UnmappableKey(Code),
    /// Some of the modifiers have no macOS event flag; the value holds them.
    #[error("modifiers {0:?} have no macOS event flag")]
    UnmappableModifiers(Modifiers),
    /// `LaunchServices` knows no application with this bundle id.
    #[error("no application with bundle id {0} is installed")]
    AppNotFound(String),
    /// No installed application ships this name.
    #[error("no installed application is named {name:?}{}", suggested(.suggestions))]
    AppNameNotFound {
        /// The name that was looked up.
        name: String,
        /// The closest names of installed applications, best first, at most
        /// five; empty when none comes close.
        suggestions: Vec<String>,
    },
    /// Several installed applications match this name equally well.
    #[error("{name:?} matches several installed applications{}", suggested(.suggestions))]
    AppNameAmbiguous {
        /// The name that was looked up.
        name: String,
        /// The names of the applications that match, best first, at most
        /// five.
        suggestions: Vec<String>,
    },
    /// The path names no bundle with a bundle identifier, so it is not an
    /// application bundle.
    #[error("{} is not an application bundle", .0.display())]
    NotAnAppBundle(PathBuf),
    /// A file to launch or to hand to the application does not exist.
    #[error("{} does not exist", .0.display())]
    FileNotFound(PathBuf),
    /// A URL to hand to the application does not parse or has no scheme.
    #[error("{0:?} is not an absolute URL")]
    InvalidUrl(String),
    /// `LaunchServices` reported the launch failed.
    #[error("launching {bundle_id} failed: {reason}")]
    LaunchFailed {
        /// The application.
        bundle_id: String,
        /// What `LaunchServices` reported.
        reason: String,
    },
    /// `LaunchServices` neither reported the launch done nor showed a process
    /// of the application in time; the launch may still be under way.
    #[error("{bundle_id} neither reported its launch nor started a process within {timeout:?}")]
    LaunchTimedOut {
        /// The application.
        bundle_id: String,
        /// How long the launch was given.
        timeout: Duration,
    },
    /// The application is running but reported no window on screen in time.
    #[error("{bundle_id} (pid {pid}) showed no new window on screen within {timeout:?}")]
    NoAppWindow {
        /// The application's process.
        pid: libc::pid_t,
        /// The application.
        bundle_id: String,
        /// How long the launch was given.
        timeout: Duration,
    },
    /// A window that was moved onto a display is not listed within its
    /// bounds.
    #[error(
        "window {window} of pid {pid} {} rather than within display {display} ({bounds})",
        .frame.map_or_else(|| "is no longer listed".to_owned(), |frame| format!("lies at {frame}"))
    )]
    NotPlaced {
        /// The application's process.
        pid: libc::pid_t,
        /// The window.
        window: u32,
        /// Where `WindowServer` lists it last, or `None` when it no longer
        /// lists it.
        frame: Option<Rect>,
        /// The display.
        display: u32,
        /// The display's bounds.
        bounds: Rect,
    },
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

/// The closest names, as a suffix of an error message; nothing when there
/// are none.
fn suggested(suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        String::new()
    } else {
        format!(": {}", suggestions.join(", "))
    }
}

/// A platform call's non-success status.
pub fn platform(call: &str, status: impl fmt::Debug) -> Error {
    Error::Platform(format!("{call} failed with {status:?}"))
}
