//! Waits that service the calling thread's run loop in its default mode.

use core::ptr;
use core::time::Duration;
use std::time::Instant;

use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFRunLoop, CFRunLoopTimer, kCFRunLoopDefaultMode,
};

use crate::Error;

/// Runs this thread's run loop in the default mode until `done` holds or
/// `timeout` passes; whether `done` held.
///
/// `done` is checked first, then again each time the loop returns: after a
/// source it handles, a stop, or at the latest every `recheck`. A wait that
/// is only woken by sources passes its whole timeout as `recheck`.
pub fn spin(
    timeout: Duration,
    recheck: Duration,
    mut done: impl FnMut() -> Result<bool, Error>,
) -> Result<bool, Error> {
    let deadline = Instant::now() + timeout;
    let run_loop = CFRunLoop::current()
        .ok_or_else(|| Error::Platform("CFRunLoopGetCurrent returned NULL".to_owned()))?;
    // A mode with no source or timer makes the loop return at once; the
    // timer keeps it waiting until the deadline.
    // SAFETY: the timer has no callout and no context.
    let timer = unsafe {
        CFRunLoopTimer::new(
            None,
            CFAbsoluteTimeGetCurrent() + timeout.as_secs_f64(),
            0.0,
            0,
            0,
            None,
            ptr::null_mut(),
        )
    }
    .ok_or_else(|| Error::Platform("CFRunLoopTimerCreate returned NULL".to_owned()))?;
    // SAFETY: the default mode is an immutable constant.
    let mode = unsafe { kCFRunLoopDefaultMode };
    run_loop.add_timer(Some(&timer), mode);
    let outcome = loop {
        match done() {
            Ok(false) => {}
            finished => break finished,
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break Ok(false);
        }
        CFRunLoop::run_in_mode(mode, remaining.min(recheck).as_secs_f64(), true);
    };
    timer.invalidate();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_ends_when_its_condition_holds() {
        let mut checks = 0;
        let held = spin(Duration::from_secs(5), Duration::from_millis(1), || {
            checks += 1;
            Ok(checks == 3)
        })
        .expect("the wait runs");
        assert!(held);
        assert_eq!(checks, 3);
    }

    #[test]
    fn a_wait_gives_up_at_its_timeout() {
        let started = Instant::now();
        let held = spin(Duration::from_millis(20), Duration::from_millis(5), || {
            Ok(false)
        })
        .expect("the wait runs");
        assert!(!held);
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn an_error_ends_a_wait() {
        let outcome = spin(Duration::from_secs(5), Duration::from_millis(1), || {
            Err(Error::Platform("failed".to_owned()))
        });
        assert!(matches!(outcome, Err(Error::Platform(message)) if message == "failed"));
    }
}
