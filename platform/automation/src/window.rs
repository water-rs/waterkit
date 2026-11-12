//! Windows as `WindowServer` lists them.

use core::fmt;

use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect,
};
use objc2_core_graphics::{
    CGDirectDisplayID, CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo,
    CGWindowListOption, kCGWindowAlpha, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer,
    kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
};

use crate::Error;

/// A rectangle in global display coordinates: points from the main display's
/// top-left corner, x to the right and y down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    pub(crate) const fn from_cg(rect: CGRect) -> Self {
        Self {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        }
    }

    /// Left edge.
    #[must_use]
    pub const fn x(&self) -> f64 {
        self.x
    }

    /// Top edge.
    #[must_use]
    pub const fn y(&self) -> f64 {
        self.y
    }

    /// Width in points.
    #[must_use]
    pub const fn width(&self) -> f64 {
        self.width
    }

    /// Height in points.
    #[must_use]
    pub const fn height(&self) -> f64 {
        self.height
    }
}

impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}x{} at ({}, {})",
            self.width, self.height, self.x, self.y
        )
    }
}

/// One window `WindowServer` lists, and whether [`Input`](crate::Input)
/// treats it as an application window.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ListedWindow {
    id: u32,
    pid: libc::pid_t,
    owner: Option<String>,
    layer: i32,
    bounds: Rect,
    alpha: f64,
    on_screen: bool,
    application: bool,
}

impl ListedWindow {
    /// The window number.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// The owning process.
    #[must_use]
    pub const fn pid(&self) -> libc::pid_t {
        self.pid
    }

    /// The owning application's name, when `WindowServer` reports one.
    #[must_use]
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    /// The window layer; ordinary application windows are on layer 0.
    #[must_use]
    pub const fn layer(&self) -> i32 {
        self.layer
    }

    /// The window's frame.
    #[must_use]
    pub const fn bounds(&self) -> Rect {
        self.bounds
    }

    /// The window's opacity; a window at 0 never takes input.
    #[must_use]
    pub const fn alpha(&self) -> f64 {
        self.alpha
    }

    /// Whether the window is ordered in on a visible Space.
    #[must_use]
    pub const fn on_screen(&self) -> bool {
        self.on_screen
    }

    /// Whether the owner's connection has a process serial number, which
    /// makes the window a delivery target.
    #[must_use]
    pub const fn application(&self) -> bool {
        self.application
    }
}

impl fmt::Display for ListedWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "window {} of pid {} ({}), layer {}, {}, alpha {}, {}, {}",
            self.id,
            self.pid,
            self.owner.as_deref().unwrap_or("no owner name"),
            self.layer,
            self.bounds,
            self.alpha,
            if self.on_screen {
                "on screen"
            } else {
                "off screen"
            },
            if self.application {
                "owner has a process serial number"
            } else {
                "owner has no process serial number"
            },
        )
    }
}

/// Every window over one display, front to back, as [`Input`](crate::Input)
/// saw it when no window there could take an event.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DisplayWindows {
    display: CGDirectDisplayID,
    bounds: Rect,
    windows: Vec<ListedWindow>,
}

impl DisplayWindows {
    pub(crate) const fn new(
        display: CGDirectDisplayID,
        bounds: Rect,
        windows: Vec<ListedWindow>,
    ) -> Self {
        Self {
            display,
            bounds,
            windows,
        }
    }

    /// The display.
    #[must_use]
    pub const fn display(&self) -> CGDirectDisplayID {
        self.display
    }

    /// The display's bounds.
    #[must_use]
    pub const fn bounds(&self) -> Rect {
        self.bounds
    }

    /// The windows that intersect the display, on screen or not, front to
    /// back.
    #[must_use]
    pub fn windows(&self) -> &[ListedWindow] {
        &self.windows
    }
}

impl fmt::Display for DisplayWindows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "display {} ({})", self.display, self.bounds)?;
        if self.windows.is_empty() {
            return write!(f, ", which no window intersects");
        }
        write!(f, ", intersected front to back by")?;
        for window in &self.windows {
            write!(f, "; {window}")?;
        }
        Ok(())
    }
}

/// One entry of `CGWindowListCopyWindowInfo`.
#[derive(Debug, Clone)]
pub struct Window {
    pub id: u32,
    pub pid: libc::pid_t,
    pub bounds: CGRect,
    pub alpha: f64,
    pub layer: i32,
    on_screen: bool,
    owner: Option<CFRetained<CFString>>,
}

impl Window {
    /// The window as listed publicly, with whether it is an application
    /// window.
    pub fn listed(&self, application: bool) -> ListedWindow {
        ListedWindow {
            id: self.id,
            pid: self.pid,
            owner: self.owner.as_ref().map(ToString::to_string),
            layer: self.layer,
            bounds: Rect::from_cg(self.bounds),
            alpha: self.alpha,
            on_screen: self.on_screen,
            application,
        }
    }
}

/// The windows `option` selects, front to back.
pub fn list(option: CGWindowListOption, relative_to: u32) -> Result<Vec<Window>, Error> {
    let list = CGWindowListCopyWindowInfo(option, relative_to)
        .ok_or_else(|| Error::Platform("CGWindowListCopyWindowInfo returned NULL".to_owned()))?;
    // SAFETY: the list holds one dictionary per window, keyed by strings.
    let list: &CFArray<CFDictionary<CFString, CFType>> = unsafe { list.cast_unchecked() };
    // SAFETY: the keys are immutable constants `CoreGraphics` exports.
    let (number_key, owner_key, name_key, layer_key, bounds_key, alpha_key, on_screen_key) = unsafe {
        (
            kCGWindowNumber,
            kCGWindowOwnerPID,
            kCGWindowOwnerName,
            kCGWindowLayer,
            kCGWindowBounds,
            kCGWindowAlpha,
            kCGWindowIsOnscreen,
        )
    };
    let number = |info: &CFDictionary<CFString, CFType>, key: &CFString| {
        info.get(key)
            .and_then(|value| value.downcast::<CFNumber>().ok())
    };
    let mut windows = Vec::with_capacity(list.len());
    for info in list {
        // Required keys of every listed window (`CGWindow.h`).
        let (Some(id), Some(pid), Some(layer), Some(alpha), Some(bounds)) = (
            number(&info, number_key).and_then(|number| number.as_i64()),
            number(&info, owner_key).and_then(|owner| owner.as_i32()),
            number(&info, layer_key).and_then(|layer| layer.as_i32()),
            number(&info, alpha_key).and_then(|alpha| alpha.as_f64()),
            info.get(bounds_key)
                .and_then(|value| value.downcast::<CFDictionary>().ok()),
        ) else {
            return Err(Error::Platform(
                "CGWindowListCopyWindowInfo listed a window without its required keys".to_owned(),
            ));
        };
        // Optional keys: an absent on-screen flag means off screen.
        let owner = info
            .get(name_key)
            .and_then(|value| value.downcast::<CFString>().ok());
        let on_screen = info
            .get(on_screen_key)
            .and_then(|value| value.downcast::<CFBoolean>().ok())
            .is_some_and(|flag| flag.as_bool());
        let mut rect = CGRect::default();
        // SAFETY: `rect` is a valid out pointer.
        if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &raw mut rect) } {
            return Err(Error::Platform(format!("window {id} has malformed bounds")));
        }
        let id = u32::try_from(id)
            .map_err(|_| Error::Platform(format!("window number {id} is out of range")))?;
        windows.push(Window {
            id,
            pid,
            bounds: rect,
            alpha,
            layer,
            on_screen,
            owner,
        });
    }
    Ok(windows)
}

/// Whether `rect` holds `point`, edges half-open.
pub fn contains(rect: CGRect, point: CGPoint) -> bool {
    (rect.origin.x..rect.origin.x + rect.size.width).contains(&point.x)
        && (rect.origin.y..rect.origin.y + rect.size.height).contains(&point.y)
}

/// Whether `a` and `b` overlap, edges half-open.
pub fn intersects(a: CGRect, b: CGRect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}

/// The display, among `displays` with their bounds, that `window` lies on:
/// the one holding the largest part of it, the earliest on a tie, as `AppKit`
/// picks a window's screen; `None` when it lies on none. A window spanning
/// several displays, such as a system overlay over the whole desktop, lies
/// on one of them only.
pub fn display_of(
    window: CGRect,
    displays: impl IntoIterator<Item = (CGDirectDisplayID, CGRect)>,
) -> Option<CGDirectDisplayID> {
    let mut best: Option<(CGDirectDisplayID, f64)> = None;
    for (display, bounds) in displays {
        let area = overlap(window, bounds);
        if area > 0.0 && best.is_none_or(|(_, most)| area > most) {
            best = Some((display, area));
        }
    }
    best.map(|(display, _)| display)
}

/// The area `a` and `b` share.
fn overlap(a: CGRect, b: CGRect) -> f64 {
    let width =
        (a.origin.x + a.size.width).min(b.origin.x + b.size.width) - a.origin.x.max(b.origin.x);
    let height =
        (a.origin.y + a.size.height).min(b.origin.y + b.size.height) - a.origin.y.max(b.origin.y);
    width.max(0.0) * height.max(0.0)
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGSize};
    use objc2_core_graphics::kCGNullWindowID;

    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
    }

    #[test]
    fn rectangles_are_half_open() {
        let window = rect(10.0, 20.0, 100.0, 50.0);
        assert!(intersects(window, rect(109.0, 69.0, 5.0, 5.0)));
        assert!(!intersects(window, rect(110.0, 20.0, 5.0, 5.0)));
        assert!(contains(window, CGPoint::new(10.0, 20.0)));
        assert!(contains(window, CGPoint::new(109.5, 69.5)));
        assert!(!contains(window, CGPoint::new(110.0, 20.0)));
        assert!(!contains(window, CGPoint::new(10.0, 70.0)));
    }

    /// A main display with a virtual display to its right, as on the Mac
    /// mini: a window on either lies on that one, a window straddling them
    /// on the one holding more of it, and one spanning the whole desktop,
    /// as a system overlay does, on the main display only.
    #[test]
    fn a_window_lies_on_the_display_holding_most_of_it() {
        let main = (1, rect(0.0, 0.0, 1920.0, 1080.0));
        let virtual_display = (5, rect(1920.0, 0.0, 1280.0, 800.0));
        let on = |window| display_of(window, [main, virtual_display]);
        assert_eq!(on(rect(2020.0, 100.0, 400.0, 300.0)), Some(5));
        assert_eq!(on(rect(100.0, 100.0, 400.0, 300.0)), Some(1));
        assert_eq!(on(rect(1820.0, 100.0, 400.0, 300.0)), Some(5));
        assert_eq!(on(rect(1720.0, 100.0, 400.0, 300.0)), Some(1));
        assert_eq!(on(rect(0.0, 0.0, 3200.0, 1080.0)), Some(1));
        assert_eq!(on(rect(-500.0, 0.0, 100.0, 100.0)), None);
        // Touching an edge is no overlap; an even split goes to the earlier.
        assert_eq!(on(rect(1920.0, 900.0, 100.0, 100.0)), None);
        let twin = (5, rect(1920.0, 0.0, 1920.0, 1080.0));
        assert_eq!(
            display_of(rect(1820.0, 0.0, 200.0, 100.0), [main, twin]),
            Some(1)
        );
    }

    /// Reads the window list only; nothing is created or shown.
    #[test]
    fn every_listed_window_parses() {
        list(CGWindowListOption::OptionAll, kCGNullWindowID).expect("the window list parses");
    }

    #[test]
    fn a_listed_window_describes_itself() {
        let window = ListedWindow {
            id: 7,
            pid: 42,
            owner: Some("Finder".to_owned()),
            layer: 0,
            bounds: Rect::from_cg(rect(100.0, 100.0, 400.0, 300.0)),
            alpha: 1.0,
            on_screen: false,
            application: true,
        };
        assert_eq!(
            window.to_string(),
            "window 7 of pid 42 (Finder), layer 0, 400x300 at (100, 100), alpha 1, off screen, owner has a process serial number"
        );
        let display =
            DisplayWindows::new(2, Rect::from_cg(rect(1920.0, 0.0, 1280.0, 800.0)), vec![]);
        assert_eq!(
            display.to_string(),
            "display 2 (1280x800 at (1920, 0)), which no window intersects"
        );
    }
}
