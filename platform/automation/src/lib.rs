//! Virtual displays and cursor-free input, for driving a desktop that is not
//! the user's.
//!
//! - [`VirtualDisplay`] is a display that exists on none of the user's
//!   monitors: a private-API `CGVirtualDisplay` of a chosen size in points,
//!   backing scale and refresh rate. Dropping it removes the display.
//! - [`Input`] posts typed mouse and keyboard events ([`MouseDown`],
//!   [`MouseUp`], [`MouseMove`], [`Scroll`], [`KeyDown`], [`KeyUp`],
//!   [`Text`]) to windows on one display. Events go straight to the process
//!   that owns the window under the point, never through the HID event tap,
//!   so the user's cursor never moves and the user's front app keeps its
//!   focus.
//!
//! The crate is macOS-only; on every other platform it is empty, so code that
//! uses it fails to compile there.
//!
//! # Permissions
//!
//! Posting events to another process, reading what it reports about its
//! focus and tapping the events routed to it need the Accessibility grant
//! (`Permission::Accessibility`). Request it with
//! `waterkit_permission::request(Permission::Accessibility)` before posting.
//!
//! # Threads and the run loop
//!
//! [`VirtualDisplay::new`] takes a [`MainThreadMarker`]: `CoreGraphics`
//! updates a process's view of the display topology only while
//! reconfiguration callbacks are serviced on the main thread, so a display
//! created anywhere else never becomes usable by the process that made it.
//! Since a library cannot assume the main thread runs a run loop, `new`
//! services one itself: it spins the main run loop in its default mode until
//! `CoreGraphics` reports the display added (each reconfiguration callback
//! stops the spin, so the wait ends as soon as the display appears) or the
//! configured [`online_timeout`](VirtualDisplayConfig::online_timeout)
//! passes. Sources the caller registered in that mode run during the wait.
//! The display's own queue is the main queue, so its termination handler,
//! which logs through `tracing`, runs on the main thread too.
//!
//! [`Input`] holds no thread-bound state and may be used from any thread.
//! While [`Input::post`] waits for its input to arrive, it runs the calling
//! thread's run loop in a mode of its own, so the caller's sources do not
//! run meanwhile. Input into a window of the calling process must come from
//! a thread other than the one running that window's event loop, which has
//! to answer the wait.
//!
//! # Display creation
//!
//! A new display can stay out of the topology: `WindowServer` registers it,
//! but Control Center may hold it as a pending mirroring target. When the
//! display is registered but not online after the timeout, `new` brings it in
//! with `-[SLWindowMirroringManager extend:]`; when it lands in a mirror set,
//! `new` takes it out for the session. A display that is not online, active
//! and unmirrored afterwards fails with [`Error::NotReady`], which carries
//! the state that was observed.
//!
//! # Private API binding
//!
//! Nothing links a private framework or symbol. `SkyLight` and `HIServices`
//! are opened with `dlopen` and every private function is resolved with
//! `dlsym`; the `CGVirtualDisplay` and
//! `SLWindowMirroringManager` classes are looked up through the Objective-C
//! runtime, and `extend:` is called only after its type encoding matches the
//! one the call is written against. A missing symbol, class or selector fails
//! with [`Error::Unavailable`], a changed signature with
//! [`Error::SignatureMismatch`].
//!
//! # Delivery
//!
//! A press goes to the topmost on-screen window under its point that an
//! application owns. Moves and the release while a button is held go to the
//! same window, as drags. Key events go to the window the last press landed
//! on; before any press, to the frontmost application window of the normal
//! window level on the display. When no window takes an event, the error
//! carries every window that intersects the display ([`DisplayWindows`]);
//! [`Input::windows`] lists them all.
//!
//! An application that does not believe it is active drops the key events
//! posted to it, and its views refuse a first click they do not accept. So
//! before a press or a key event, the target application is made to report
//! itself active with the target window focused, without raising the window
//! or changing `WindowServer`'s front process, which stays the user's: an
//! inactive application gets the application-activated event `AppKit` builds
//! for a real activation, and a window that is not focused gets the focus
//! record and the make-key record pair `SLPSPostEventRecordTo` takes.
//! [`Input::post`] then waits, on the application's Accessibility
//! notifications, until it reports both (`AXFrontmost`, `AXFocusedWindow`),
//! and fails with [`Error::NotFocused`] when it does not in time.
//!
//! The events themselves are posted to the owning process with
//! `SLEventPostToPid`, stamped with the target window and its window-local
//! location. For the length of the call, an event tap on that process sees
//! each of them arrive; [`Input::post`] returns once all have, and fails with
//! [`Error::NotDelivered`] when one has not in time. Both waits end after
//! [`Input::confirm_timeout`].

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

#[cfg(target_os = "macos")]
mod accessibility;
#[cfg(target_os = "macos")]
mod delivery;
#[cfg(target_os = "macos")]
mod display;
#[cfg(target_os = "macos")]
mod error;
#[cfg(target_os = "macos")]
mod event;
#[cfg(target_os = "macos")]
mod framework;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod keycode;
#[cfg(target_os = "macos")]
mod skylight;
#[cfg(target_os = "macos")]
mod topology;
#[cfg(target_os = "macos")]
mod window;

#[cfg(target_os = "macos")]
pub use accessibility::FocusReport;
#[cfg(target_os = "macos")]
pub use display::{BackingScale, VirtualDisplay, VirtualDisplayConfig};
#[cfg(target_os = "macos")]
pub use error::{DisplayState, Error};
#[cfg(target_os = "macos")]
pub use event::{
    InputEvent, KeyDown, KeyUp, MouseButton, MouseDown, MouseMove, MouseUp, Point, Scroll, Text,
};
#[cfg(target_os = "macos")]
pub use input::Input;
#[cfg(target_os = "macos")]
pub use keyboard_types::{Code, Modifiers};
#[cfg(target_os = "macos")]
pub use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
pub use waterkit_core::RefreshRate;
#[cfg(target_os = "macos")]
pub use window::{DisplayWindows, ListedWindow, Rect};
