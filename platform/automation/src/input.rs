//! Cursor-free input into windows on one display.

use core::sync::atomic::{AtomicU32, Ordering};
use core::time::Duration;

use objc2_core_foundation::{CFRetained, CFString, CGPoint, CGRect};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGEvent, CGEventFlags, CGEventType, CGKeyCode,
    CGMouseButton, CGScrollEventUnit, CGWindowListOption, kCGNullWindowID,
};

use crate::accessibility::{Application, WindowLookup};
use crate::delivery::{self, APPKIT_DEFINED, Watch};
use crate::keycode::{event_flags, virtual_key};
use crate::skylight::EventSymbols;
use crate::window::{self, DisplayWindows, ListedWindow, Rect, Window, intersects};
use crate::{
    Error, InputEvent, KeyDown, KeyUp, MouseButton, MouseDown, MouseMove, MouseUp, Point, Scroll,
    Text,
};
use crate::{placement, run_loop, topology};

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

/// One reading of a mouse event's point and the window under it.
#[derive(Debug)]
struct Reading {
    /// The point in global display coordinates.
    point: CGPoint,
    /// The window the hit test hands a press at the point to, as the window
    /// list has it.
    hit: Option<Window>,
}

/// Where a mouse event goes: its global point, and the target window with
/// the frame it had when the point was read.
#[derive(Debug, Clone, Copy)]
struct Located {
    point: CGPoint,
    frame: CGRect,
    target: Target,
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
    /// focused, `WindowServer` to route each posted event into it, and the
    /// display and the window under a mouse event's point to hold still for
    /// one reading, before [`post`](Self::post) fails. One second unless set.
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
    /// - [`Error::Unsettled`] when the display, or the window under a mouse
    ///   event's point, keeps moving for the confirm timeout;
    /// - [`Error::NoWindow`] when no window takes a press at it;
    /// - [`Error::NoKeyWindow`] when a key event has no window to go to;
    ///   both carry every window that intersects the display;
    /// - [`Error::NotApplicationWindow`] when the window that takes a press
    ///   at the point belongs to no application;
    /// - [`Error::UnmappableKey`] or [`Error::UnmappableModifiers`] when a key
    ///   or modifier has no macOS equivalent;
    /// - [`Error::NotFocused`] when the application owning a press's or key
    ///   event's window does not report itself active with that window
    ///   focused in time, which it needs to act on the input;
    /// - [`Error::NotificationUnsupported`] when that application does not
    ///   post the Accessibility notifications the wait is woken by;
    /// - [`Error::NotDelivered`] when `WindowServer` does not route every
    ///   event into the target process in time;
    /// - [`Error::Platform`] when `CoreGraphics`, `SkyLight` or
    ///   Accessibility fails a call.
    pub fn post(&mut self, event: impl InputEvent) -> Result<(), Error> {
        event.deliver(self)
    }

    /// The display's current bounds in global display coordinates. A point
    /// [`post`](Self::post) takes is relative to their origin, so a listed
    /// window's [`bounds`](ListedWindow::bounds) minus that origin is where
    /// the window lies in the points events take.
    #[must_use]
    pub fn bounds(&self) -> Rect {
        Rect::from_cg(CGDisplayBounds(self.display))
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
            .filter_map(|window| self.listed(window).transpose())
            .collect()
    }

    /// `window` with its ownership, or `None` when it closed since it was
    /// listed.
    fn listed(&self, window: &Window) -> Result<Option<ListedWindow>, Error> {
        Ok(self
            .symbols
            .application_owns(window.id)?
            .map(|application| window.listed(application)))
    }

    /// The windows that intersect the display, on screen or not, for an
    /// error that found none there to take an event.
    fn display_windows(&self) -> Result<DisplayWindows, Error> {
        let bounds = CGDisplayBounds(self.display);
        let windows = window::list(CGWindowListOption::OptionAll, kCGNullWindowID)?
            .iter()
            .filter(|window| intersects(window.bounds, bounds))
            .filter_map(|window| self.listed(window).transpose())
            .collect::<Result<_, _>>()?;
        Ok(DisplayWindows::new(
            self.display,
            Rect::from_cg(bounds),
            windows,
        ))
    }

    pub(crate) fn mouse_down(&mut self, event: MouseDown) -> Result<(), Error> {
        let flags = event_flags(event.modifiers)?;
        let mut located = self.locate(event.at)?;
        let (mut watch, pressed) = loop {
            let mut watch = Watch::open(located.target.pid, self.confirm_timeout)?;
            self.focus(&mut watch, located.target, located.frame)?;
            // Focusing waits on the application, and the display can move
            // meanwhile, taking the window with it: the press is converted
            // with the window's frame as of now.
            let now = self.locate(event.at)?;
            if now.target == located.target {
                break (watch, now);
            }
            tracing::debug!(
                focused = located.target.window,
                now = now.target.window,
                %event.at,
                "another window took the point while the first was focused"
            );
            watch.confirm(located.target.window)?;
            located = now;
        };
        let Located {
            point,
            frame,
            target,
        } = pressed;
        // A move first: AppKit hit-tests a press against the tracking state
        // the window last saw.
        let moved = mouse_event(CGEventType::MouseMoved, point, MouseButton::Left)?;
        self.post_mouse(&mut watch, &moved, target, frame, point, flags, 0);

        let down = mouse_event(down_type(event.button), point, event.button)?;
        self.post_mouse(
            &mut watch,
            &down,
            target,
            frame,
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
        let held = self.held.filter(|held| held.button == event.button);
        let Located {
            point,
            frame,
            target,
        } = match held {
            Some(held) => self.locate_held(event.at, held.target)?,
            None => self.locate(event.at)?,
        };
        let mut watch = Watch::open(target.pid, self.confirm_timeout)?;
        let up = mouse_event(up_type(event.button), point, event.button)?;
        self.post_mouse(
            &mut watch,
            &up,
            target,
            frame,
            point,
            flags,
            event.clicks.get(),
        );
        if held.is_some() {
            self.held = None;
        }
        watch.confirm(target.window)
    }

    pub(crate) fn mouse_move(&self, event: MouseMove) -> Result<(), Error> {
        let flags = event_flags(event.modifiers)?;
        let (located, kind, button) = match self.held {
            Some(held) => (
                self.locate_held(event.at, held.target)?,
                drag_type(held.button),
                held.button,
            ),
            None => (
                self.locate(event.at)?,
                CGEventType::MouseMoved,
                MouseButton::Left,
            ),
        };
        let Located {
            point,
            frame,
            target,
        } = located;
        let moved = mouse_event(kind, point, button)?;
        let mut watch = Watch::open(target.pid, self.confirm_timeout)?;
        self.post_mouse(&mut watch, &moved, target, frame, point, flags, 0);
        watch.confirm(target.window)
    }

    pub(crate) fn scroll(&self, event: Scroll) -> Result<(), Error> {
        let flags = event_flags(event.modifiers)?;
        let Located {
            point,
            frame,
            target,
        } = self.locate(event.at)?;
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
        self.post_mouse(&mut watch, &scrolled, target, frame, point, flags, 0);
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

    /// The point `at` on the display, and the window `WindowServer` hands a
    /// press there to, read from one geometry.
    ///
    /// The display's position (`CGDisplayBounds`), the hit test and the
    /// window list are separate reads, and the system moves a display, and
    /// every window on it, when the arrangement changes, such as when
    /// another display goes away. A reading counts only when the display
    /// stayed put across it and the window list has the hit window where
    /// the hit test found it, around the point; until one does, the reads
    /// are taken again. A frame from a reading that disagrees would put the
    /// window-local point outside the window, where no view takes it.
    fn read(&self, at: Point) -> Result<Reading, Error> {
        let mode = CFString::from_static_str(delivery::MODE);
        let mut reading = None;
        let settled = run_loop::spin_in(
            Some(&mode),
            self.confirm_timeout,
            placement::RECHECK,
            || {
                let before = CGDisplayBounds(self.display);
                let point = global(before, at).ok_or(Error::OutsideDisplay {
                    display: self.display,
                    at,
                })?;
                let hit = self
                    .symbols
                    .window_at(point)?
                    .map(listed_window)
                    .transpose()?;
                let after = CGDisplayBounds(self.display);
                let frame = hit.as_ref().map(|window| window.bounds);
                let agrees = one_geometry(before, after, frame, point);
                if !agrees {
                    tracing::debug!(
                        display = self.display,
                        %at,
                        ?before,
                        ?after,
                        window = hit.as_ref().map(|window| window.id),
                        ?frame,
                        "the display or the window under the point moved during the reading"
                    );
                }
                reading = Some(Reading { point, hit });
                Ok(agrees)
            },
        )?;
        if !settled {
            return Err(Error::Unsettled {
                display: self.display,
                at,
                timeout: self.confirm_timeout,
            });
        }
        Ok(reading.expect("a settled wait read the geometry"))
    }

    /// The window `WindowServer` hands a press at `at` to, as a delivery
    /// target, with its frame as of the reading ([`read`](Self::read)). Its
    /// hit test follows each window's shape and passes through windows that
    /// ignore mouse events, which on the main display include system
    /// overlays the size of the display.
    fn locate(&self, at: Point) -> Result<Located, Error> {
        let Reading { point, hit } = self.read(at)?;
        let Some(window) = hit else {
            tracing::debug!(display = self.display, %at, "no window takes a press at the point");
            return Err(Error::NoWindow {
                at,
                windows: self.display_windows()?,
            });
        };
        match self.symbols.application_owns(window.id)? {
            Some(true) => Ok(Located {
                point,
                frame: window.bounds,
                target: Target {
                    pid: window.pid,
                    window: window.id,
                },
            }),
            Some(false) => Err(Error::NotApplicationWindow {
                at,
                window: window.listed(false),
            }),
            None => Err(Error::Platform(format!(
                "window {}, which takes a press at {at}, closed before the press",
                window.id
            ))),
        }
    }

    /// The point `at` for an event that goes to `held`, the window a held
    /// button pressed, wherever the point lies. While the point is over that
    /// window, its frame is the reading's; past its edge, the window list's.
    fn locate_held(&self, at: Point, held: Target) -> Result<Located, Error> {
        let Reading { point, hit } = self.read(at)?;
        let frame = match hit {
            Some(window) if window.id == held.window => window.bounds,
            _ => window_bounds(held.window)?,
        };
        Ok(Located {
            point,
            frame,
            target: held,
        })
    }

    /// The window key events go to: the last press's, else the frontmost
    /// ordinary application window on the display that takes mouse events.
    /// Menu bar items and other windows above the normal level are skipped:
    /// on the main display they cover every application's windows. So are
    /// windows that let mouse events through, such as overlays drawn over
    /// other applications, which no click could have made key.
    fn key_target(&self) -> Result<Target, Error> {
        if let Some(target) = self.key_target {
            return Ok(target);
        }
        let display = CGDisplayBounds(self.display);
        for window in on_screen_windows()? {
            if window.layer == NORMAL_LAYER
                && intersects(window.bounds, display)
                && !self.symbols.ignores_mouse_events(window.id)?
                && let Some(target) = self.application_target(&window)?
            {
                return Ok(target);
            }
        }
        Err(Error::NoKeyWindow(self.display_windows()?))
    }

    /// `window` as a delivery target, when an application owns it and it is
    /// still open.
    fn application_target(&self, window: &Window) -> Result<Option<Target>, Error> {
        Ok(
            (self.symbols.application_owns(window.id)? == Some(true)).then_some(Target {
                pid: window.pid,
                window: window.id,
            }),
        )
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
        let _observation = app.observe(target.window, watch.run_loop(), watch.mode())?;
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

/// `at`, a point on a display whose bounds are `display`, in global display
/// coordinates; `None` when it lies outside the display.
fn global(display: CGRect, at: Point) -> Option<CGPoint> {
    let inside =
        (0.0..display.size.width).contains(&at.x()) && (0.0..display.size.height).contains(&at.y());
    inside.then(|| CGPoint::new(display.origin.x + at.x(), display.origin.y + at.y()))
}

/// Whether the reads of one reading saw one geometry: the display stayed at
/// `before` until `after`, and the window the hit test handed `point` to,
/// listed at `frame`, lies under the point, as every window the hit test
/// finds does.
fn one_geometry(before: CGRect, after: CGRect, frame: Option<CGRect>, point: CGPoint) -> bool {
    before == after && frame.is_none_or(|frame| window::contains(frame, point))
}

/// How `WindowServer` lists `window` now.
fn listed_window(window: u32) -> Result<Window, Error> {
    window::list(CGWindowListOption::OptionIncludingWindow, window)?
        .into_iter()
        .find(|listed| listed.id == window)
        .ok_or_else(|| Error::Platform(format!("window {window} is no longer listed")))
}

/// The current bounds of `window`.
fn window_bounds(window: u32) -> Result<CGRect, Error> {
    listed_window(window).map(|listed| listed.bounds)
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

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(
            CGPoint::new(x, y),
            objc2_core_foundation::CGSize::new(width, height),
        )
    }

    #[test]
    fn a_point_is_global_only_on_its_display() {
        let display = rect(-1280.0, 0.0, 1280.0, 800.0);
        assert_eq!(
            global(display, Point::new(140.0, 130.0)),
            Some(CGPoint::new(-1140.0, 130.0))
        );
        assert_eq!(global(display, Point::new(1280.0, 0.0)), None);
        assert_eq!(global(display, Point::new(0.0, -1.0)), None);
    }

    /// The reading of water-rs/waterkit#424: the display had moved from
    /// x = -2560 to -1280, the hit test found the window at its new place,
    /// and the window list still had it at the old one, which put the
    /// window-local point 1280 points right of the 400x300 window.
    #[test]
    fn a_reading_counts_only_when_its_reads_agree() {
        let display = rect(-1280.0, 0.0, 1280.0, 800.0);
        let point = global(display, Point::new(140.0, 130.0)).expect("on the display");
        let moved_window = rect(-1180.0, 100.0, 400.0, 300.0);
        let stale_window = rect(-2460.0, 100.0, 400.0, 300.0);

        assert!(one_geometry(display, display, Some(moved_window), point));
        assert!(
            !one_geometry(display, display, Some(stale_window), point),
            "a frame that does not hold the hit point is from another geometry"
        );
        assert!(
            !one_geometry(
                rect(-2560.0, 0.0, 1280.0, 800.0),
                display,
                Some(moved_window),
                point
            ),
            "the display moved during the reading"
        );
        assert!(
            one_geometry(display, display, None, point),
            "no window under the point is a reading too"
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
