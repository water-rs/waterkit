//! Cursor-free input into windows on one display.

use objc2_core_foundation::{CFRetained, CGPoint, CGRect};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGEvent, CGEventFlags, CGEventType, CGKeyCode,
    CGMouseButton, CGScrollEventUnit, CGWindowListOption, kCGNullWindowID,
};
use objc2_foundation::NSProcessInfo;

use crate::keycode::{event_flags, virtual_key};
use crate::skylight::{EventSymbols, ProcessSerialNumber, RECORD_LEN};
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
/// `kCGMouseEventWindowUnderMousePointer`.
const FIELD_WINDOW_UNDER_POINTER: u32 = 91;
/// `kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent`.
const FIELD_WINDOW_HANDLING_EVENT: u32 = 92;

/// The first macOS release that reads a pid-routed event's window location as
/// window-local; earlier ones take the global point.
const WINDOW_LOCAL_SINCE_MAJOR: isize = 27;

/// The most UTF-16 units one keyboard event carries; the system truncates
/// longer strings on delivery.
const UNITS_PER_EVENT: usize = 20;

/// Posts input to the windows of one display without moving the user's
/// cursor or focusing anything on the user's displays.
///
/// See the crate documentation for where each event goes.
#[derive(Debug)]
pub struct Input {
    symbols: EventSymbols,
    display: CGDirectDisplayID,
    window_local: bool,
    held: Option<Held>,
    key_target: Option<Target>,
}

/// An application window events are delivered to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    pid: libc::pid_t,
    psn: ProcessSerialNumber,
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
    /// [`Error::Unavailable`] when a `SkyLight` function is missing.
    pub fn new(display: CGDirectDisplayID) -> Result<Self, Error> {
        if !topology::online()?.contains(&display) {
            return Err(Error::DisplayNotFound(display));
        }
        let version = NSProcessInfo::processInfo().operatingSystemVersion();
        Ok(Self {
            symbols: EventSymbols::resolve()?,
            display,
            window_local: version.majorVersion >= WINDOW_LOCAL_SINCE_MAJOR,
            held: None,
            key_target: None,
        })
    }

    /// Delivers `event`.
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
    /// - [`Error::Platform`] when `CoreGraphics` or `SkyLight` fails a call.
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
        let application = self.symbols.application_psn(window.id)?.is_some();
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
        self.focus(target)?;
        // A move first: AppKit hit-tests a press against the tracking state
        // the window last saw.
        let moved = mouse_event(CGEventType::MouseMoved, point, MouseButton::Left)?;
        self.post_mouse(&moved, target, window.bounds, point, flags, 0);

        let down = mouse_event(down_type(event.button), point, event.button)?;
        self.post_mouse(
            &down,
            target,
            window.bounds,
            point,
            flags,
            event.clicks.get(),
        );
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
        let up = mouse_event(up_type(event.button), point, event.button)?;
        self.post_mouse(&up, target, bounds, point, flags, event.clicks.get());
        if self.held.is_some_and(|held| held.button == event.button) {
            self.held = None;
        }
        Ok(())
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
        self.post_mouse(&moved, target, bounds, point, flags, 0);
        Ok(())
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
        self.post_mouse(&scrolled, target, window.bounds, point, flags, 0);
        Ok(())
    }

    pub(crate) fn key_down(&mut self, event: KeyDown) -> Result<(), Error> {
        let key = virtual_key(event.code)?;
        let flags = event_flags(event.modifiers)?;
        let target = self.key_target()?;
        self.post_key(target, key, true, flags, &[])
    }

    pub(crate) fn key_up(&mut self, event: KeyUp) -> Result<(), Error> {
        let key = virtual_key(event.code)?;
        let flags = event_flags(event.modifiers)?;
        let target = self.key_target()?;
        self.post_key(target, key, false, flags, &[])
    }

    pub(crate) fn text(&mut self, event: &Text) -> Result<(), Error> {
        let chunks = utf16_chunks(&event.text);
        if chunks.is_empty() {
            return Ok(());
        }
        let target = self.key_target()?;
        for chunk in &chunks {
            self.post_key(target, 0, true, CGEventFlags::empty(), chunk)?;
            self.post_key(target, 0, false, CGEventFlags::empty(), chunk)?;
        }
        Ok(())
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
    /// application window on the display, which is focused first.
    fn key_target(&mut self) -> Result<Target, Error> {
        if let Some(target) = self.key_target {
            return Ok(target);
        }
        let display = CGDisplayBounds(self.display);
        for window in on_screen_windows()? {
            if intersects(window.bounds, display)
                && let Some(target) = self.application_target(&window)?
            {
                self.focus(target)?;
                return Ok(target);
            }
        }
        Err(Error::NoKeyWindow(self.display_windows()?))
    }

    /// `window` as a delivery target, when an application owns it: its
    /// owner's connection has a process serial number. `WindowServer`'s own
    /// windows have none.
    fn application_target(&self, window: &Window) -> Result<Option<Target>, Error> {
        Ok(self.symbols.application_psn(window.id)?.map(|psn| Target {
            pid: window.pid,
            psn,
            window: window.id,
        }))
    }

    /// Makes `target` the key window of its application, without raising it
    /// or activating the application.
    fn focus(&mut self, target: Target) -> Result<(), Error> {
        if self.key_target == Some(target) {
            return Ok(());
        }
        self.symbols
            .post_record(target.psn, &focus_record(target.window))?;
        for kind in MAKE_KEY_KINDS {
            self.symbols
                .post_record(target.psn, &make_key_record(target.window, kind))?;
        }
        tracing::debug!(
            pid = target.pid,
            psn = ?target.psn,
            window = target.window,
            "posted the records that make the window its application's key window"
        );
        self.key_target = Some(target);
        Ok(())
    }

    /// Stamps the routing of a mouse event to `target`, whose window has the
    /// frame `window`, and posts it.
    fn post_mouse(
        &self,
        event: &CGEvent,
        target: Target,
        window: CGRect,
        point: CGPoint,
        flags: CGEventFlags,
        clicks: u32,
    ) {
        CGEvent::set_flags(Some(event), flags);
        let local = CGPoint::new(point.x - window.origin.x, point.y - window.origin.y);
        let location = if self.window_local { local } else { point };
        self.symbols.set_window_location(event, location);
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
            psn = ?target.psn,
            window = target.window,
            kind = ?CGEvent::r#type(Some(event)),
            ?flags,
            clicks,
            global = ?point,
            window_local = ?local,
            stamped = if self.window_local { "window-local" } else { "global" },
            "posting a mouse event"
        );
        self.symbols.post_to_pid(target.pid, event);
    }

    fn stamp_window(&self, event: &CGEvent, target: Target) {
        self.symbols
            .set_field(event, FIELD_TARGET_PID, i64::from(target.pid));
        self.symbols
            .set_field(event, FIELD_WINDOW_NUMBER, i64::from(target.window));
    }

    fn post_key(
        &self,
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
            psn = ?target.psn,
            window = target.window,
            key,
            down,
            ?flags,
            units = text.len(),
            "posting a keyboard event"
        );
        self.symbols.post_to_pid(target.pid, &event);
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

/// Kinds of the record pair that makes a window its application's key window.
const MAKE_KEY_KINDS: [u8; 2] = [0x01, 0x02];

/// The record that tells an application it holds input focus in `window`,
/// without raising the window or activating the application.
fn focus_record(window: u32) -> [u8; RECORD_LEN] {
    let mut record = [0_u8; RECORD_LEN];
    record[0x04] = 0xF8;
    record[0x08] = 0x0D;
    record[0x3C..0x40].copy_from_slice(&window.to_le_bytes());
    record[0x8A] = 0x01;
    record
}

/// One record of the pair that makes `window` its application's key window.
fn make_key_record(window: u32, kind: u8) -> [u8; RECORD_LEN] {
    let mut record = [0_u8; RECORD_LEN];
    record[0x04] = 0xF8;
    record[0x08] = kind;
    record[0x20..0x30].fill(0xFF);
    record[0x3A] = 0x10;
    record[0x3C..0x40].copy_from_slice(&window.to_le_bytes());
    record
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
    fn records_carry_their_length_kind_and_window() {
        let focus = focus_record(0x0102_0304);
        assert_eq!(focus.len(), 0xF8);
        assert_eq!(focus[0x04], 0xF8);
        assert_eq!(focus[0x08], 0x0D);
        assert_eq!(focus[0x3C..0x40], [0x04, 0x03, 0x02, 0x01]);
        assert_eq!(focus[0x8A], 0x01);

        let make_key = make_key_record(7, 0x02);
        assert_eq!(make_key[0x08], 0x02);
        assert_eq!(make_key[0x3A], 0x10);
        assert!(make_key[0x20..0x30].iter().all(|byte| *byte == 0xFF));
        assert_eq!(make_key[0x3C..0x40], [7, 0, 0, 0]);
    }

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
}
