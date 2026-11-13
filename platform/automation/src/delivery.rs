//! Confirmation that `WindowServer` routed posted events into the target
//! process.
//!
//! An event posted to a process enters its event stream just before the
//! event taps for that process and passes through them (`CGEventPostToPid`,
//! `CGEvent.h`). For the length of one [`Input::post`](crate::Input::post)
//! call, a tap on the target process sees the events that reach it, and each
//! event the call posts carries a marker in its source user data. An event
//! whose marker the tap does not see in time was dropped before the process
//! got it.

use core::cell::{Cell, RefCell};
use core::ffi::c_void;
use core::ptr::NonNull;
use core::time::Duration;
use std::time::Instant;

use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopMode, CFRunLoopSource, CFString,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventMask, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy,
    CGEventType,
};

use crate::Error;
use crate::skylight::EventSymbols;

/// The run loop mode a post waits in: only its own tap and observers run.
pub const MODE: &str = "dev.waterkit.automation.delivery";

/// Every event type [`Input`](crate::Input) posts.
const POSTED_TYPES: [CGEventType; 15] = [
    CGEventType::LeftMouseDown,
    CGEventType::LeftMouseUp,
    CGEventType::RightMouseDown,
    CGEventType::RightMouseUp,
    CGEventType::MouseMoved,
    CGEventType::LeftMouseDragged,
    CGEventType::RightMouseDragged,
    CGEventType::OtherMouseDown,
    CGEventType::OtherMouseUp,
    CGEventType::OtherMouseDragged,
    CGEventType::ScrollWheel,
    CGEventType::KeyDown,
    CGEventType::KeyUp,
    CGEventType::FlagsChanged,
    APPKIT_DEFINED,
];

/// `NSEventTypeAppKitDefined`, which has no named Quartz constant.
pub const APPKIT_DEFINED: CGEventType = CGEventType(13);

/// What the tap saw: each event's marker, and whether `WindowServer` turned
/// the tap off.
#[derive(Debug, Default)]
struct Routed {
    markers: RefCell<Vec<i64>>,
    disabled: Cell<Option<CGEventType>>,
}

/// One post call's tap on its target process, and the markers of the events
/// the call posted.
pub struct Watch {
    pid: libc::pid_t,
    timeout: Duration,
    run_loop: CFRetained<CFRunLoop>,
    mode: CFRetained<CFRunLoopMode>,
    tap: CFRetained<CFMachPort>,
    source: CFRetained<CFRunLoopSource>,
    // Dropped after the tap is invalidated, which ends its callbacks.
    routed: Box<Routed>,
    posted: Vec<i64>,
}

impl Watch {
    /// Taps the events routed to `pid`; every wait gives up after `timeout`.
    ///
    /// The tap filters rather than listens, which takes the Accessibility
    /// grant posting already needs; it hands every event on unchanged.
    pub fn open(pid: libc::pid_t, timeout: Duration) -> Result<Self, Error> {
        let routed = Box::<Routed>::default();
        let mask = POSTED_TYPES
            .iter()
            .fold(0 as CGEventMask, |mask, kind| mask | (1 << kind.0));
        // SAFETY: `on_routed` matches the tap callback signature and reads
        // `routed`, which outlives the tap (see `Drop`).
        let tap = unsafe {
            CGEvent::tap_create_for_pid(
                pid,
                CGEventTapPlacement::TailAppendEventTap,
                CGEventTapOptions::Default,
                mask,
                Some(on_routed),
                (&raw const *routed).cast_mut().cast::<c_void>(),
            )
        }
        .ok_or_else(|| {
            Error::Platform(format!(
                "CGEventTapCreateForPid refused a tap on pid {pid}; posting needs the Accessibility grant"
            ))
        })?;
        let source = CFMachPort::new_run_loop_source(None, Some(&tap), 0).ok_or_else(|| {
            Error::Platform("CFMachPortCreateRunLoopSource returned NULL".to_owned())
        })?;
        let run_loop = CFRunLoop::current()
            .ok_or_else(|| Error::Platform("CFRunLoopGetCurrent returned NULL".to_owned()))?;
        let mode = CFString::from_static_str(MODE);
        run_loop.add_source(Some(&source), Some(&mode));
        CGEvent::tap_enable(&tap, true);
        Ok(Self {
            pid,
            timeout,
            run_loop,
            mode,
            tap,
            source,
            routed,
            posted: Vec::new(),
        })
    }

    pub fn run_loop(&self) -> &CFRunLoop {
        &self.run_loop
    }

    pub fn mode(&self) -> &CFRunLoopMode {
        &self.mode
    }

    /// Marks `event` with `marker` and posts it to the watched process.
    pub fn post(&mut self, symbols: &EventSymbols, event: &CGEvent, marker: i64) {
        CGEvent::set_integer_value_field(Some(event), CGEventField::EventSourceUserData, marker);
        self.posted.push(marker);
        symbols.post_to_pid(self.pid, event);
    }

    /// Runs this thread's run loop in the watch's mode, so the tap and the
    /// observers registered in it are served, until `done` holds or the
    /// timeout passes; whether `done` held.
    pub fn until(&self, mut done: impl FnMut() -> Result<bool, Error>) -> Result<bool, Error> {
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Some(kind) = self.routed.disabled.get() {
                return Err(Error::Platform(format!(
                    "WindowServer turned off the delivery tap on pid {} ({kind:?})",
                    self.pid
                )));
            }
            if done()? {
                return Ok(true);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            CFRunLoop::run_in_mode(Some(&self.mode), remaining.as_secs_f64(), true);
        }
    }

    /// Waits until every event posted through the watch has reached the
    /// process; `window` names the target in the error.
    pub fn confirm(self, window: u32) -> Result<(), Error> {
        let arrived = |routed: &Routed| {
            let seen = routed.markers.borrow();
            self.posted
                .iter()
                .filter(|marker| seen.contains(marker))
                .count()
        };
        let all = self.until(|| Ok(arrived(&self.routed) == self.posted.len()))?;
        if all {
            return Ok(());
        }
        Err(Error::NotDelivered {
            pid: self.pid,
            window,
            posted: self.posted.len(),
            routed: arrived(&self.routed),
            timeout: self.timeout,
        })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        CGEvent::tap_enable(&self.tap, false);
        self.run_loop
            .remove_source(Some(&self.source), Some(&self.mode));
        self.tap.invalidate();
    }
}

/// Records the marker of each event routed to the watched process and hands
/// the event on.
unsafe extern "C-unwind" fn on_routed(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    routed: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `routed` is the boxed state the watch registered, which lives
    // until the tap is invalidated; callbacks run on the watch's thread.
    let routed = unsafe { &*routed.cast::<Routed>() };
    if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput {
        routed.disabled.set(Some(kind));
    } else {
        // SAFETY: the tap hands its callback a live event.
        let event_ref = unsafe { event.as_ref() };
        let marker =
            CGEvent::integer_value_field(Some(event_ref), CGEventField::EventSourceUserData);
        routed.markers.borrow_mut().push(marker);
    }
    event.as_ptr()
}
