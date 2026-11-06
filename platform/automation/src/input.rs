//! Cursor-free input into windows on one display.

use core::sync::atomic::{AtomicU32, Ordering};
use core::time::Duration;

use objc2_core_foundation::{CFRetained, CGPoint, CGRect};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGEvent, CGEventFlags, CGEventType, CGKeyCode,
    CGMouseButton, CGScrollEventUnit, CGWindowListOption, kCGNullWindowID,
};

use crate::accessibility::{Application, WindowLookup};
use crate::delivery::{APPKIT_DEFINED, Watch};
use crate::keycode::{event_flags, virtual_key};
use crate::skylight::EventSymbols;
use crate::topology;
use crate::window::{self, DisplayWindows, ListedWindow, Rect, Window, contains, intersects};
use crate::{
    Error, InputEvent, KeyDown, KeyUp, MouseButton, MouseDown, MouseMove, MouseUp, Point, Scroll,
    Text,
};

/// `kCGMouseEventClickState`.
const FIELD_CLICK_STATE: u32 = 1;
/// `kCGMouseEventButtonNumber`.
const FIELD_BUTTON_NUMBER: u32 = 3;
/// `kCGEventTargetUnixProcessID`.
const FIELD_TARGET_PID: u32 = 40;
/// The window number `AppKit` reads as the event's window.
const FIELD_WINDOW_NUMBER: u32 = 51;
/// The sender's `WindowServer` connection, which `AppKit` stamps on the
/// window events it builds.
const FIELD_WINDOW_CONTEXT: u32 = 52;
/// The subtype of an `AppKit`-defined event.
const FIELD_APPKIT_SUBTYPE: u32 = 83;
/// `kCGMouseEventWindowUnderMousePointer`.
const FIELD_WINDOW_UNDER_POINTER: u32 = 91;
/// `kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent`.
const FIELD_WINDOW_HANDLING_EVENT: u32 = 92;

/// `NSEventSubtypeApplicationActivated`.
const APPLICATION_ACTIVATED: i64 = 1;
/// The flags `AppKit` carries on an application-activated event; they are
/// not held modifiers.
const ACTIVATION_FLAGS: CGEventFlags = CGEventFlags(0xC_0000);

/// Where the press that makes a window key lands, from the window's top-left:
/// just outside its frame, so no view takes it.
const MAKE_KEY_PRESS: CGPoint = CGPoint::new(-1.0, -1.0);

/// The window level of ordinary application windows (`kCGNormalWindowLevel`).
const NORMAL_LAYER: i32 = 0;

/// The most UTF-16 units one keyboard event carries; the system truncates
/// longer strings on delivery.
const UNITS_PER_EVENT: usize = 20;

/// How long, by default, a target application gets to report a window
/// focused and `WindowServer` to route an event into it.
const DEFAULT_CONFIRM_TIMEOUT: Duration = Duration::from_secs(1);

/// Posts input to the windows of one display without moving the user's
/// cursor or focusing anything on the user's displays.
///
/// See the crate documentation for where each event goes and how its arrival
/// is confirmed.
#[derive(Debug)]
pub struct Input {
    symbols: EventSymbols,
    windows: WindowLookup,
    display: CGDirectDisplayID,
    confirm_timeout: Duration,
    /// The next event's marker, unique among this process's posts.
    markers: AtomicU32,
    held: Option<Held>,
    /// The window this input last made its application's key window, where
    /// key events go.
    key_target: Option<Target>,
}

/// An application window events are delivered to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    pid: libc::pid_t,
    window: u32,
}

/// The button a press holds down, and the window it landed on.
#[derive(Debug, Clone, Copy)]
struct Held {
    button: MouseButton,
    target: Target,
}

impl Input {
    /// Input into the windows of the display `display`.
    ///
    /// # Errors
    ///
    /// [`Error::DisplayNotFound`] when the display is not online, and
    /// [`Error::Unavailable`] when a private function is missing.
    pub fn new(display: CGDirectDisplayID) -> Result<Self, Error> {
        if !topology::online()?.contains(&display) {
            return Err(Error::DisplayNotFound(display));
        }
        Ok(Self {
            symbols: EventSymbols::resolve()?,
            windows: WindowLookup::resolve()?,
            display,
            confirm_timeout: DEFAULT_CONFIRM_TIMEOUT,
            markers: AtomicU32::new(0),
            held: None,
            key_target: None,
        })
    }

    /// How long a target application gets to report the target window
    /// focused, and `WindowServer` to route each posted event into it,
    /// before [`post`](Self::post) fails. One second unless set.
    #[must_use]
    pub const fn confirm_timeout(&self) -> Duration {
        self.confirm_timeout
    }

    /// Sets [`confirm_timeout`](Self::confirm_timeout).
    pub const fn set_confirm_timeout(&mut self, timeout: Duration) {
        self.confirm_timeout = timeout;
    }

    /// Delivers `event`, and returns once every event it posted has entered
    /// the target process's event stream.
    ///
    /// That is the last step the system reports on: no interface tells
    /// another process whether an application then dispatched an event to a
    /// view. Before a press or a key event, the target window has been made
    /// key ahead of the input, and its application has reported itself
    /// active with that window focused.
    ///
    /// # Errors
    ///
    /// - [`Error::OutsideDisplay`] when a mouse event's point lies outside
    ///   the display;
    /// - [`Error::NoWindow`] when no application window is under it;
    /// - [`Error::NoKeyWindow`] when a key event has no window to go to;
    ///   both carry every window that intersects the display;
    /// - [`Error::UnmappableKey`] or [`Error::UnmappableModifiers`] when a key
    ///   or modifier has no macOS equivalent;
    /// - [`Error::NotFocused`] when the application owning a press's or key
    ///   event's window does not report itself active with that window
    ///   focused in time, which it needs to act on the input;
    /// - [`Error::NotDelivered`] when `WindowServer` does not route every
    ///   event into the target process in time;
    /// - [`Error::Platform`] when `CoreGraphics`, `SkyLight` or
    ///   Accessibility fails a call.
    pub fn post(&mut self, event: impl InputEvent) -> Result<(), Error> {
        event.deliver(self)
    }

    /// Every window `WindowServer` lists, on any display and on screen or
    /// not, front to back.
    ///
    /// # Errors
    ///
    /// [`Error::Platform`] when `CoreGraphics` or `SkyLight` fails a call.
    pub fn windows(&self) -> Result<Vec<ListedWindow>, Error> {
        window::list(CGWindowListOption::OptionAll, kCGNullWindowID)?
            .iter()
            .map(|window| self.listed(window))
            .collect()
    }

    fn listed(&self, window: &Window) -> Result<ListedWindow, Error> {
        let application = self.symbols.application_owns(window.id)?;
        Ok(window.listed(application))
    }

    /// The windows that intersect the display, on screen or not, for an
    /// error that found none there to take an event.
    fn display_windows(&self) -> Result<DisplayWindows, Error> {
        let bounds = CGDisplayBounds(self.display);
        let windows = window::list(CGWindowListOption::OptionAll, kCGNullWindowID)?
            .iter()
            .filter(|window| intersects(window.bounds, bounds))
            .map(|window| self.listed(window))
            .collect::<Result<_, _>>()?;
        Ok(DisplayWindows::new(
            self.display,
            Rect::from_cg(bounds),
            windows,
        ))
    }

    pub(crate) fn mouse_down(&mut self, event: MouseDown) -> Result<(), Error> {
        let flags = event_flags(event.modifiers)?;
        let point = self.global(event.at)?;
        let (window, target) = self.window_at(event.at, point)?;
        let mut watch = Watch::open(target.pid, self.confirm_timeout)?;
        self.focus(&mut watch, target, window.bounds)?;
        // A move first: AppKit hit-tests a press against the tracking state
        // the window last saw.
        let moved = mouse_event(CGEventType::MouseMoved, point, MouseButton::Left)?;
        self.post_mouse(&mut watch, &moved, target, window.bounds, point, flags, 0);

        let down = mouse_event(down_type(event.button), point, event.button)?;
        self.post_mouse(
            &mut watch,
            &down,
            target,
            window.bounds,
            point,
            flags,
            event.clicks.get(),
        );
        watch.confirm(target.window)?;
        self.held = Some(Held {
            button: event.button,
            target,
        });
        Ok(())
    }

    pub(crate) fn mouse_up(&mut self, event: MouseUp) -> Result<(), Error> {
        let flags = event_flags(event.modifiers)?;
        let point = self.global(event.at)?;
        let (bounds, target) = match self.held {
            Some(held) if held.button == event.button => {
                (window_bounds(held.target.window)?, held.target)
            }
            _ => {
                let (window, target) = self.window_at(event.at, point)?;
                (window.bounds, target)
            }
        };
        let mut watch = Watch::open(target.pid, self.confirm_timeout)?;
        let up = mouse_event(up_type(event.button), point, event.button)?;
        self.post_mouse(
            &mut watch,
            &up,
            target,
            bounds,
            point,
            flags,
            event.clicks.get(),
        );
        if self.held.is_some_and(|held| held.button == event.button) {
            self.held = None;
        }
        watch.confirm(target.window)
    }

    pub(crate) fn mouse_move(&self, event: MouseMove) -> Result<(), Error> {
        let flags = event_flags(event.modifiers)?;
        let point = self.global(event.at)?;
        let (moved, bounds, target) = if let Some(held) = self.held {
            let dragged = mouse_event(drag_type(held.button), point, held.button)?;
            (dragged, window_bounds(held.target.window)?, held.target)
        } else {
            let (window, target) = self.window_at(event.at, point)?;
            let moved = mouse_event(CGEventType::MouseMoved, point, MouseButton::Left)?;
            (moved, window.bounds, target)
        };
        let mut watch = Watch::open(target.pid, self.confirm_timeout)?;
        self.post_mouse(&mut watch, &moved, target, bounds, point, flags, 0);
        watch.confirm(target.window)
    }

    pub(crate) fn scroll(&self, event: Scroll) -> Result<(), Error> {
        let flags = event_flags(event.modifiers)?;
        let point = self.global(event.at)?;
        let (window, target) = self.window_at(event.at, point)?;
        // A wheel turned up is positive; moving down through content is a
        // negative wheel delta.
        let scrolled = CGEvent::new_scroll_wheel_event2(
            None,
            CGScrollEventUnit::Pixel,
            2,
            event.dy.saturating_neg(),
            event.dx.saturating_neg(),
            0,
        )
        .ok_or_else(|| null_event("CGEventCreateScrollWheelEvent2"))?;
        CGEvent::set_location(Some(&scrolled), point);
        let mut watch = Watch::open(target.pid, self.confirm_timeout)?;
        self.post_mouse(
            &mut watch,
            &scrolled,
            target,
            window.bounds,
            point,
            flags,
            0,
        );
        watch.confirm(target.window)
    }

    pub(crate) fn key_down(&mut self, event: KeyDown) -> Result<(), Error> {
        let key = virtual_key(event.code)?;
        let flags = event_flags(event.modifiers)?;
        self.keys(|input, watch, target| input.post_key(watch, target, key, true, flags, &[]))
    }

    pub(crate) fn key_up(&mut self, event: KeyUp) -> Result<(), Error> {
        let key = virtual_key(event.code)?;
        let flags = event_flags(event.modifiers)?;
        self.keys(|input, watch, target| input.post_key(watch, target, key, false, flags, &[]))
    }

    pub(crate) fn text(&mut self, event: &Text) -> Result<(), Error> {
        let chunks = utf16_chunks(&event.text);
        if chunks.is_empty() {
            return Ok(());
        }
        self.keys(|input, watch, target| {
            for chunk in &chunks {
                input.post_key(watch, target, 0, true, CGEventFlags::empty(), chunk)?;
                input.post_key(watch, target, 0, false, CGEventFlags::empty(), chunk)?;
            }
            Ok(())
        })
    }

    /// Focuses the key target, posts key events with `post`, and confirms
    /// they reached it.
    fn keys(
        &mut self,
        post: impl FnOnce(&Self, &mut Watch, Target) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let target = self.key_target()?;
        let bounds = window_bounds(target.window)?;
        let mut watch = Watch::open(target.pid, self.confirm_timeout)?;
        self.focus(&mut watch, target, bounds)?;
        post(self, &mut watch, target)?;
        watch.confirm(target.window)
    }

    /// `at` in global display coordinates, checked to lie on the display.
    fn global(&self, at: Point) -> Result<CGPoint, Error> {
        let bounds = CGDisplayBounds(self.display);
        let inside = (0.0..bounds.size.width).contains(&at.x())
            && (0.0..bounds.size.height).contains(&at.y());
        if !inside {
            return Err(Error::OutsideDisplay {
                display: self.display,
                at,
            });
        }
        Ok(CGPoint::new(
            bounds.origin.x + at.x(),
            bounds.origin.y + at.y(),
        ))
    }

    /// The topmost application window under `point`.
    fn window_at(&self, at: Point, point: CGPoint) -> Result<(Window, Target), Error> {
        for window in on_screen_windows()? {
            if contains(window.bounds, point)
                && let Some(target) = self.application_target(&window)?
            {
                return Ok((window, target));
            }
        }
        tracing::debug!(display = self.display, %at, "no application window under the point");
        Err(Error::NoWindow {
            at,
            windows: self.display_windows()?,
        })
    }

    /// The window key events go to: the last press's, else the frontmost
    /// ordinary application window on the display. Menu bar items and other
    /// windows above the normal level are skipped: on the main display they
    /// cover every application's windows.
    fn key_target(&self) -> Result<Target, Error> {
        if let Some(target) = self.key_target {
            return Ok(target);
        }
        let display = CGDisplayBounds(self.display);
        for window in on_screen_windows()? {
            if window.layer == NORMAL_LAYER
                && intersects(window.bounds, display)
                && let Some(target) = self.application_target(&window)?
            {
                return Ok(target);
            }
        }
        Err(Error::NoKeyWindow(self.display_windows()?))
    }

    /// `window` as a delivery target, when an application owns it.
    fn application_target(&self, window: &Window) -> Result<Option<Target>, Error> {
        Ok(self.symbols.application_owns(window.id)?.then_some(Target {
            pid: window.pid,
            window: window.id,
        }))
    }

    /// Makes `target`'s window, whose frame is `bounds`, its application's
    /// key window in an application that believes it is active, without
    /// changing `WindowServer`'s front process, and waits until the
    /// application reports both.
    ///
    /// An application that does not believe it is active drops the key
    /// events posted to it, and `AppKit` hands the press that makes a window
    /// key only to a view that accepts first mouse: a text view, like
    /// `TextEdit`'s, does not. An inactive application is sent the
    /// application-activated event `AppKit` builds for a real activation,
    /// then the window is sent a press and release just outside its frame,
    /// which make it key and reach no view. They go through the same
    /// per-process queue as the input after them, which `AppKit` handles in
    /// order, so the window is key before the input arrives.
    ///
    /// An inactive application reports as `AXFocusedWindow` a window it did
    /// not make key, so the press is skipped only for the window this input
    /// made key last, while the application still reports it focused and
    /// itself active.
    fn focus(&mut self, watch: &mut Watch, target: Target, bounds: CGRect) -> Result<(), Error> {
        let app = Application::new(target.pid, &self.windows, self.confirm_timeout)?;
        // Before anything is sent, so no change goes unnoticed.
        let _observation = app.observe(watch.run_loop(), watch.mode())?;
        let mut report = app.report()?;
        if self.key_target == Some(target) && report.settled_on(target.window) {
            return Ok(());
        }
        if report.active() != Some(true) {
            let activated = self.activation(target)?;
            watch.post(&self.symbols, &activated, self.marker());
        }
        self.make_key(watch, target, bounds)?;
        tracing::debug!(
            pid = target.pid,
            window = target.window,
            %report,
            "asked the application to take input in the window"
        );
        let settled = watch.until(|| {
            report = app.report()?;
            Ok(report.settled_on(target.window))
        })?;
        if !settled {
            return Err(Error::NotFocused {
                pid: target.pid,
                window: target.window,
                timeout: self.confirm_timeout,
                report,
            });
        }
        tracing::debug!(
            pid = target.pid,
            window = target.window,
            "the application reports the window focused"
        );
        self.key_target = Some(target);
        Ok(())
    }

    /// Posts the press and release that make `target`'s window, whose frame
    /// is `bounds`, its application's key window, at [`MAKE_KEY_PRESS`].
    fn make_key(&self, watch: &mut Watch, target: Target, bounds: CGRect) -> Result<(), Error> {
        let point = CGPoint::new(
            bounds.origin.x + MAKE_KEY_PRESS.x,
            bounds.origin.y + MAKE_KEY_PRESS.y,
        );
        for kind in [CGEventType::LeftMouseDown, CGEventType::LeftMouseUp] {
            let event = mouse_event(kind, point, MouseButton::Left)?;
            self.post_mouse(
                watch,
                &event,
                target,
                bounds,
                point,
                CGEventFlags::empty(),
                1,
            );
        }
        Ok(())
    }

    /// The application-activated event `AppKit` builds for a real
    /// activation, addressed to `target`'s window.
    fn activation(&self, target: Target) -> Result<CFRetained<CGEvent>, Error> {
        let event = CGEvent::new(None).ok_or_else(|| null_event("CGEventCreate"))?;
        CGEvent::set_type(Some(&event), APPKIT_DEFINED);
        CGEvent::set_flags(Some(&event), ACTIVATION_FLAGS);
        self.stamp_window(&event, target);
        self.symbols.set_field(
            &event,
            FIELD_WINDOW_CONTEXT,
            i64::from(self.symbols.main_connection()),
        );
        self.symbols
            .set_field(&event, FIELD_APPKIT_SUBTYPE, APPLICATION_ACTIVATED);
        Ok(event)
    }

    /// A marker no other event this process posts carries.
    fn marker(&self) -> i64 {
        let sequence = self.markers.fetch_add(1, Ordering::Relaxed);
        (i64::from(std::process::id()) << 32) | i64::from(sequence)
    }

    /// Stamps the routing of a mouse event to `target`, whose window has the
    /// frame `window`, and posts it. The receiving window reads the stamped
    /// location as its own top-left-origin point, so the stamp is
    /// window-local.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is one field of the posted event"
    )]
    fn post_mouse(
        &self,
        watch: &mut Watch,
        event: &CGEvent,
        target: Target,
        window: CGRect,
        point: CGPoint,
        flags: CGEventFlags,
        clicks: u32,
    ) {
        CGEvent::set_flags(Some(event), flags);
        let local = CGPoint::new(point.x - window.origin.x, point.y - window.origin.y);
        self.symbols.set_window_location(event, local);
        self.symbols
            .set_field(event, FIELD_CLICK_STATE, i64::from(clicks));
        self.stamp_window(event, target);
        let number = i64::from(target.window);
        self.symbols
            .set_field(event, FIELD_WINDOW_UNDER_POINTER, number);
        self.symbols
            .set_field(event, FIELD_WINDOW_HANDLING_EVENT, number);
        tracing::debug!(
            pid = target.pid,
            window = target.window,
            kind = ?CGEvent::r#type(Some(event)),
            ?flags,
            clicks,
            global = ?point,
            window_local = ?local,
            "posting a mouse event"
        );
        watch.post(&self.symbols, event, self.marker());
    }

    fn stamp_window(&self, event: &CGEvent, target: Target) {
        self.symbols
            .set_field(event, FIELD_TARGET_PID, i64::from(target.pid));
        self.symbols
            .set_field(event, FIELD_WINDOW_NUMBER, i64::from(target.window));
    }

    fn post_key(
        &self,
        watch: &mut Watch,
        target: Target,
        key: CGKeyCode,
        down: bool,
        flags: CGEventFlags,
        text: &[u16],
    ) -> Result<(), Error> {
        let event = CGEvent::new_keyboard_event(None, key, down)
            .ok_or_else(|| null_event("CGEventCreateKeyboardEvent"))?;
        CGEvent::set_flags(Some(&event), flags);
        if !text.is_empty() {
            let length = u64::try_from(text.len()).expect("a chunk holds at most 20 units");
            // SAFETY: `text` holds `length` UTF-16 units.
            unsafe { CGEvent::keyboard_set_unicode_string(Some(&event), length, text.as_ptr()) };
        }
        self.stamp_window(&event, target);
        tracing::debug!(
            pid = target.pid,
            window = target.window,
            key,
            down,
            ?flags,
            units = text.len(),
            "posting a keyboard event"
        );
        watch.post(&self.symbols, &event, self.marker());
        Ok(())
    }
}

fn null_event(call: &str) -> Error {
    Error::Platform(format!("{call} returned NULL"))
}

fn mouse_event(
    kind: CGEventType,
    point: CGPoint,
    button: MouseButton,
) -> Result<CFRetained<CGEvent>, Error> {
    let event = CGEvent::new_mouse_event(None, kind, point, cg_button(button))
        .ok_or_else(|| null_event("CGEventCreateMouseEvent"))?;
    CGEvent::set_integer_value_field(
        Some(&event),
        objc2_core_graphics::CGEventField(FIELD_BUTTON_NUMBER),
        i64::from(cg_button(button).0),
    );
    Ok(event)
}

const fn cg_button(button: MouseButton) -> CGMouseButton {
    match button {
        MouseButton::Left => CGMouseButton::Left,
        MouseButton::Right => CGMouseButton::Right,
        MouseButton::Middle => CGMouseButton::Center,
    }
}

const fn down_type(button: MouseButton) -> CGEventType {
    match button {
        MouseButton::Left => CGEventType::LeftMouseDown,
        MouseButton::Right => CGEventType::RightMouseDown,
        MouseButton::Middle => CGEventType::OtherMouseDown,
    }
}

const fn up_type(button: MouseButton) -> CGEventType {
    match button {
        MouseButton::Left => CGEventType::LeftMouseUp,
        MouseButton::Right => CGEventType::RightMouseUp,
        MouseButton::Middle => CGEventType::OtherMouseUp,
    }
}

const fn drag_type(button: MouseButton) -> CGEventType {
    match button {
        MouseButton::Left => CGEventType::LeftMouseDragged,
        MouseButton::Right => CGEventType::RightMouseDragged,
        MouseButton::Middle => CGEventType::OtherMouseDragged,
    }
}

/// The on-screen windows with any opacity, front to back.
fn on_screen_windows() -> Result<Vec<Window>, Error> {
    let mut windows = window::list(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    )?;
    windows.retain(|window| window.alpha > 0.0);
    Ok(windows)
}

/// The current bounds of `window`.
fn window_bounds(window: u32) -> Result<CGRect, Error> {
    window::list(CGWindowListOption::OptionIncludingWindow, window)?
        .into_iter()
        .find(|listed| listed.id == window)
        .map(|listed| listed.bounds)
        .ok_or_else(|| Error::Platform(format!("window {window} is no longer listed")))
}

/// `text` as UTF-16, cut between characters into pieces one keyboard event
/// carries.
fn utf16_chunks(text: &str) -> Vec<Vec<u16>> {
    let mut chunks = Vec::new();
    let mut chunk = Vec::with_capacity(UNITS_PER_EVENT);
    let mut units = [0_u16; 2];
    for character in text.chars() {
        let encoded = character.encode_utf16(&mut units);
        if chunk.len() + encoded.len() > UNITS_PER_EVENT {
            chunks.push(core::mem::replace(
                &mut chunk,
                Vec::with_capacity(UNITS_PER_EVENT),
            ));
        }
        chunk.extend_from_slice(encoded);
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cut_between_characters() {
        assert_eq!(utf16_chunks(""), Vec::<Vec<u16>>::new());
        assert_eq!(
            utf16_chunks("hi"),
            vec![vec![u16::from(b'h'), u16::from(b'i')]]
        );

        let long = "a".repeat(45);
        let lengths: Vec<usize> = utf16_chunks(&long).iter().map(Vec::len).collect();
        assert_eq!(lengths, [20, 20, 5]);

        // 19 units, then a surrogate pair that does not fit after them.
        let text = format!("{}\u{1F600}", "a".repeat(19));
        let chunks = utf16_chunks(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 19);
        assert_eq!(
            String::from_utf16(&chunks[1]).ok().as_deref(),
            Some("\u{1F600}")
        );
    }

    /// The activation event carries what `AppKit` routes an
    /// application-activated event by. Building an event posts nothing.
    #[test]
    fn the_activation_event_names_the_window_and_connection() {
        let input = Input {
            symbols: EventSymbols::resolve().expect("the event functions resolve"),
            windows: WindowLookup::resolve().expect("the window lookup resolves"),
            display: 0,
            confirm_timeout: DEFAULT_CONFIRM_TIMEOUT,
            markers: AtomicU32::new(0),
            held: None,
            key_target: None,
        };
        let target = Target { pid: 42, window: 7 };
        let event = input.activation(target).expect("the event is built");
        let field = |field: u32| {
            CGEvent::integer_value_field(Some(&event), objc2_core_graphics::CGEventField(field))
        };
        assert_eq!(CGEvent::r#type(Some(&event)), APPKIT_DEFINED);
        assert_eq!(CGEvent::flags(Some(&event)), ACTIVATION_FLAGS);
        assert_eq!(field(FIELD_WINDOW_NUMBER), 7);
        assert_eq!(field(FIELD_TARGET_PID), 42);
        assert_eq!(field(FIELD_APPKIT_SUBTYPE), APPLICATION_ACTIVATED);
        assert_eq!(
            field(FIELD_WINDOW_CONTEXT),
            i64::from(input.symbols.main_connection())
        );

        let first = input.marker();
        assert_ne!(first, input.marker(), "every marker is new");
        assert_eq!(first >> 32, i64::from(std::process::id()));
    }
}
