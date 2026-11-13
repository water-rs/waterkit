//! An application's windows: waiting for it to show one, and moving them onto
//! a display through Accessibility, which neither activates the application
//! nor moves the cursor.

use core::time::Duration;

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGWindowListOption, kCGNullWindowID,
};

use crate::accessibility::{Application, Window};
use crate::window::{self, Rect};
use crate::{Error, run_loop};

/// How often a wait on another process's windows looks at them again: no
/// notification reaches a process that did not create the window.
pub const RECHECK: Duration = Duration::from_millis(50);

/// The windows `app` reports that `WindowServer` lists on screen, other than
/// the `existing` ones, once there is one; empty when `timeout` passes first.
pub fn shown_windows(
    app: &Application,
    existing: &[u32],
    timeout: Duration,
) -> Result<Vec<Window>, Error> {
    let mut shown = Vec::new();
    run_loop::spin(timeout, RECHECK, || {
        let listed = window::list(
            CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
            kCGNullWindowID,
        )?;
        shown = app.windows()?;
        shown.retain(|window| {
            !existing.contains(&window.id()) && listed.iter().any(|listed| listed.id == window.id())
        });
        Ok(!shown.is_empty())
    })?;
    Ok(shown)
}

/// Moves `windows` of `pid` onto `display`, each centred and shrunk to fit
/// it, and waits until `WindowServer` lists every one within the display.
pub fn place(
    pid: libc::pid_t,
    windows: &[Window],
    display: CGDirectDisplayID,
    timeout: Duration,
) -> Result<(), Error> {
    let bounds = CGDisplayBounds(display);
    for window in windows {
        let frame = window.frame()?;
        let target = fit(frame.size, bounds);
        if target.size != frame.size {
            window.set_size(target.size)?;
        }
        window.set_origin(target.origin)?;
        // The system keeps a window's title bar below a display's menu bar,
        // which can push a window as tall as the display past its bottom.
        if let Some(size) = shrunk(window.frame()?, bounds) {
            window.set_size(size)?;
        }
        // `tracing`'s macros bring a `display` function into scope.
        let display_id = display;
        tracing::debug!(
            pid,
            window = window.id(),
            ?frame,
            ?target,
            display_id,
            "moved a window"
        );
    }

    let mut stray = None;
    let placed = run_loop::spin(timeout, RECHECK, || {
        let bounds = CGDisplayBounds(display);
        let listed = window::list(CGWindowListOption::OptionAll, kCGNullWindowID)?;
        stray = windows.iter().find_map(|window| {
            let frame = listed
                .iter()
                .find(|listed| listed.id == window.id())
                .map(|listed| listed.bounds);
            match frame {
                Some(frame) if contains(bounds, frame) => None,
                frame => Some((window.id(), frame)),
            }
        });
        Ok(stray.is_none())
    })?;
    if placed {
        return Ok(());
    }
    let (window, frame) = stray.expect("a failed wait found a window outside the display");
    Err(Error::NotPlaced {
        pid,
        window,
        frame: frame.map(Rect::from_cg),
        display,
        bounds: Rect::from_cg(CGDisplayBounds(display)),
    })
}

/// Where a window of `size` goes on a display with `bounds`: shrunk to fit
/// it, centred on whole points.
const fn fit(size: CGSize, bounds: CGRect) -> CGRect {
    let size = CGSize::new(
        size.width.min(bounds.size.width),
        size.height.min(bounds.size.height),
    );
    let origin = CGPoint::new(
        bounds.origin.x + ((bounds.size.width - size.width) / 2.0).floor(),
        bounds.origin.y + ((bounds.size.height - size.height) / 2.0).floor(),
    );
    CGRect::new(origin, size)
}

/// The size that ends `frame`, whose top-left corner lies on a display with
/// `bounds`, at the display's right and bottom edges; `None` when the frame
/// already fits or its corner lies off the display.
fn shrunk(frame: CGRect, bounds: CGRect) -> Option<CGSize> {
    let right = bounds.origin.x + bounds.size.width;
    let bottom = bounds.origin.y + bounds.size.height;
    let corner_inside = (bounds.origin.x..right).contains(&frame.origin.x)
        && (bounds.origin.y..bottom).contains(&frame.origin.y);
    if !corner_inside || contains(bounds, frame) {
        return None;
    }
    Some(CGSize::new(
        frame.size.width.min(right - frame.origin.x),
        frame.size.height.min(bottom - frame.origin.y),
    ))
}

/// Whether `outer` holds all of `inner`.
const fn contains(outer: CGRect, inner: CGRect) -> bool {
    inner.origin.x >= outer.origin.x
        && inner.origin.y >= outer.origin.y
        && inner.origin.x + inner.size.width <= outer.origin.x + outer.size.width
        && inner.origin.y + inner.size.height <= outer.origin.y + outer.size.height
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
    }

    /// A 1280x800 display arranged right of a 1920-point main display.
    const DISPLAY: CGRect = CGRect::new(CGPoint::new(1920.0, 0.0), CGSize::new(1280.0, 800.0));

    #[test]
    fn a_window_that_fits_is_centred_on_whole_points() {
        assert_eq!(
            fit(CGSize::new(231.0, 401.0), DISPLAY),
            rect(1920.0 + 524.0, 199.0, 231.0, 401.0)
        );
    }

    #[test]
    fn a_window_larger_than_the_display_is_shrunk_to_it() {
        assert_eq!(
            fit(CGSize::new(2000.0, 600.0), DISPLAY),
            rect(1920.0, 100.0, 1280.0, 600.0)
        );
        assert_eq!(fit(CGSize::new(3000.0, 2000.0), DISPLAY), DISPLAY);
    }

    #[test]
    fn a_window_pushed_below_the_menu_bar_is_shrunk_to_the_bottom_edge() {
        assert_eq!(
            shrunk(rect(1920.0, 24.0, 1280.0, 800.0), DISPLAY),
            Some(CGSize::new(1280.0, 776.0))
        );
        assert_eq!(shrunk(rect(2000.0, 24.0, 400.0, 300.0), DISPLAY), None);
        assert_eq!(shrunk(rect(0.0, 24.0, 400.0, 300.0), DISPLAY), None);
    }

    #[test]
    fn containment_includes_the_edges() {
        assert!(contains(DISPLAY, DISPLAY));
        assert!(contains(DISPLAY, rect(1920.0, 0.0, 10.0, 10.0)));
        assert!(!contains(DISPLAY, rect(1919.0, 0.0, 10.0, 10.0)));
        assert!(!contains(DISPLAY, rect(3100.0, 700.0, 101.0, 10.0)));
    }
}
