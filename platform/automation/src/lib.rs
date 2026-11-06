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
//! Posting events to another process needs the Accessibility grant
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
//! Nothing links a private framework. `SkyLight` is opened with `dlopen` and
//! every function is resolved with `dlsym`; the `CGVirtualDisplay` and
//! `SLWindowMirroringManager` classes are looked up through the Objective-C
//! runtime, and `extend:` is called only after its type encoding matches the
//! one the call is written against. A missing symbol, class or selector fails
//! with [`Error::Unavailable`], a changed signature with
//! [`Error::SignatureMismatch`].
//!
//! # Delivery
//!
//! A press goes to the topmost on-screen window under its point that an
//! application owns, and makes that window the key window of its app without
//! raising it or activating the app: the focus record `SLPSPostEventRecordTo`
//! takes. Moves and the release while a button is held go to the same window,
//! as drags. Key events go to the window the last press landed on; before
//! any press, to the frontmost application window on the display, which is
//! then focused the same way. The events themselves are posted to the
//! owning process with `SLEventPostToPid`, stamped with the target window and
//! its window-local location. When no window takes an event, the error
//! carries every window that intersects the display ([`DisplayWindows`]);
//! [`Input::windows`] lists them all.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

#[cfg(target_os = "macos")]
mod display;
#[cfg(target_os = "macos")]
mod error;
#[cfg(target_os = "macos")]
mod event;
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
