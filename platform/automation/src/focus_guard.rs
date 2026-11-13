// Copyright (c) 2026 Cua AI, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// Derived from cua-driver (https://github.com/trycua/cua):
// libs/cua-driver/rust/crates/platform-macos/src/focus_steal.rs,
// libs/cua-driver/rust/crates/platform-macos/src/focus_arbiter.rs and
// libs/cua-driver/rust/crates/platform-macos/src/tools/launch_app.rs.

//! Putting the user's application back in front when a launched one
//! activates itself.
//!
//! `LaunchServices` honors `activates = false`, but an application such as
//! Chrome, Safari or an Electron app calls `NSApp.activate` itself when it
//! finishes launching or opens a URL another process handed it. A
//! [`FocusGuard`] watches `NSWorkspace.didActivateApplicationNotification`
//! and, when the launched application becomes the front one, activates the
//! application that was in front when the guard was armed. An activation
//! the user may have caused, a mouse button or a modifier key pressed just
//! before it, is left alone, and so is every later one: the guard has then
//! seen the user switch.
//!
//! The guard is owned by the launch and stops watching when it is dropped.
//! It also stops on its own deadline, checked whenever an activation is
//! reported, so a guard that outlives its launch never keeps reverting.

use core::time::Duration;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification,
};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSOperationQueue};

use crate::{Error, placement, run_loop};

/// A hardware mouse-down or modifier change within this long before an
/// activation means the user may have caused it. The window is cua-driver's.
const USER_SWITCH_WINDOW: Duration = Duration::from_secs(1);

/// What a guard decides about one activation.
#[derive(Debug)]
struct Watch {
    /// The application to put back in front.
    restore_to: libc::pid_t,
    /// The launched application, once its process is known.
    target: Option<libc::pid_t>,
    /// After this the guard reverts nothing.
    deadline: Instant,
    /// Whether the user has been seen to switch applications. Final: a
    /// later check, with the input no longer recent, does not undo it.
    left: bool,
}

impl Watch {
    /// The application to activate for `activated` having become the front
    /// one at `now`, or `None` to leave it. `user_switched` is asked only
    /// when there is a change to undo.
    fn decide(
        &mut self,
        activated: libc::pid_t,
        now: Instant,
        user_switched: impl FnOnce() -> bool,
    ) -> Option<libc::pid_t> {
        if now >= self.deadline || self.left {
            return None;
        }
        // The restore activation reports itself too; it is not a steal.
        if self.target != Some(activated) || activated == self.restore_to {
            return None;
        }
        if user_switched() {
            self.left = true;
            return None;
        }
        Some(self.restore_to)
    }
}

/// State the activation observer shares with its guard.
#[derive(Debug)]
struct Shared(Mutex<Watch>);

impl Shared {
    /// Reverts `activated` becoming the front application when the watch
    /// says to.
    fn activated(&self, activated: libc::pid_t) {
        let restore = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .decide(activated, Instant::now(), user_switched_recently);
        if let Some(restore) = restore {
            restore_front(activated, restore);
        }
    }
}

/// Activates `restore`, if it still runs, over `activated`.
fn restore_front(activated: libc::pid_t, restore: libc::pid_t) {
    let Some(application) = NSRunningApplication::runningApplicationWithProcessIdentifier(restore)
    else {
        tracing::debug!(restore, "the application to put back in front quit");
        return;
    };
    // Documented thread-safe, so it runs on the observer's thread.
    let sent = application.activateWithOptions(NSApplicationActivationOptions(0));
    tracing::debug!(activated, restore, sent, "put the front application back");
}

/// Whether the user pressed a mouse button or a modifier key on the real
/// hardware just now. Events this crate posts to a process do not count.
fn user_switched_recently() -> bool {
    [
        CGEventType::LeftMouseDown,
        CGEventType::RightMouseDown,
        CGEventType::FlagsChanged,
    ]
    .into_iter()
    .any(|kind| {
        let since = CGEventSource::seconds_since_last_event_type(
            CGEventSourceStateID::HIDSystemState,
            kind,
        );
        since.is_finite() && since < USER_SWITCH_WINDOW.as_secs_f64()
    })
}

/// The front application's process.
fn frontmost_pid() -> Option<libc::pid_t> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|application| application.processIdentifier())
}

/// A watch that puts the application that was in front when it was armed
/// back in front when the launched application activates itself.
pub struct FocusGuard {
    shared: Arc<Shared>,
    center: Retained<NSNotificationCenter>,
    observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    // The observer's block runs on this queue's own thread, so activations
    // are handled whatever run loop the calling thread services.
    _queue: Retained<NSOperationQueue>,
}

impl FocusGuard {
    /// Starts watching, for `lifetime` at most, to keep the application that
    /// is in front now in front; `None` when no application is.
    pub fn arm(lifetime: Duration) -> Option<Self> {
        let restore_to = frontmost_pid()?;
        let shared = Arc::new(Shared(Mutex::new(Watch {
            restore_to,
            target: None,
            deadline: Instant::now() + lifetime,
            left: false,
        })));
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let queue = NSOperationQueue::new();
        // Serial, so two activations in a row are put back in order.
        queue.setMaxConcurrentOperationCount(1);
        let for_block = Arc::clone(&shared);
        let block = RcBlock::new(move |notification: core::ptr::NonNull<NSNotification>| {
            // SAFETY: AppKit hands the block a live notification for the
            // duration of the call.
            let notification = unsafe { notification.as_ref() };
            if let Some(pid) = activated_pid(notification) {
                for_block.activated(pid);
            }
        });
        // SAFETY: the name is the notification's constant, the block is
        // `Send` through its `Arc`, and the queue outlives the observer.
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidActivateApplicationNotification),
                None,
                Some(&queue),
                &block,
            )
        };
        Some(Self {
            shared,
            center,
            observer,
            _queue: queue,
        })
    }

    /// Names the launched application's process, and puts the previous front
    /// application back at once if that process is already in front.
    pub fn follow(&self, pid: libc::pid_t) {
        self.shared
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .target = Some(pid);
        if let Some(front) = frontmost_pid() {
            self.shared.activated(front);
        }
    }

    /// Keeps watching for `window`, servicing this thread's run loop and
    /// checking the front application every [`placement::RECHECK`] in case an
    /// activation went unreported.
    pub fn settle(&self, window: Duration) -> Result<(), Error> {
        run_loop::spin(window, placement::RECHECK, || {
            if let Some(front) = frontmost_pid() {
                self.shared.activated(front);
            }
            Ok(false)
        })?;
        Ok(())
    }
}

impl Drop for FocusGuard {
    fn drop(&mut self) {
        // SAFETY: the observer is the one `arm` registered with this center.
        unsafe { self.center.removeObserver(self.observer.as_ref()) };
    }
}

/// The process of the application a `didActivateApplication` notification
/// reports.
fn activated_pid(notification: &NSNotification) -> Option<libc::pid_t> {
    let info = notification.userInfo()?;
    // SAFETY: `NSWorkspaceApplicationKey` is the key whose value is the
    // `NSRunningApplication`.
    let application = unsafe { info.objectForKey(NSWorkspaceApplicationKey) }?;
    let application = application.downcast::<NSRunningApplication>().ok()?;
    Some(application.processIdentifier())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch(lifetime: Duration) -> Watch {
        Watch {
            restore_to: 7,
            target: Some(42),
            deadline: Instant::now() + lifetime,
            left: false,
        }
    }

    #[test]
    fn the_launched_application_activating_is_put_back() {
        let mut watch = watch(Duration::from_secs(5));
        assert_eq!(watch.decide(42, Instant::now(), || false), Some(7));
        // Another steal is put back too.
        assert_eq!(watch.decide(42, Instant::now(), || false), Some(7));
    }

    #[test]
    fn another_application_activating_is_left() {
        let mut watch = watch(Duration::from_secs(5));
        assert_eq!(watch.decide(99, Instant::now(), || true), None);
        assert!(
            !watch.left,
            "an activation that is not the target's is not a switch"
        );
        assert_eq!(watch.decide(7, Instant::now(), || false), None);
    }

    #[test]
    fn an_application_not_yet_launched_is_not_followed() {
        let mut watch = watch(Duration::from_secs(5));
        watch.target = None;
        assert_eq!(watch.decide(42, Instant::now(), || false), None);
    }

    #[test]
    fn a_switch_by_the_user_is_left_for_good() {
        let mut watch = watch(Duration::from_secs(5));
        assert_eq!(watch.decide(42, Instant::now(), || true), None);
        // The input is old by the next check, yet the switch stands.
        assert_eq!(watch.decide(42, Instant::now(), || false), None);
    }

    #[test]
    fn input_is_not_asked_for_when_there_is_nothing_to_undo() {
        let mut watch = watch(Duration::from_secs(5));
        let outcome = watch.decide(99, Instant::now(), || {
            panic!("the input is read only for the launched application")
        });
        assert_eq!(outcome, None);
    }

    #[test]
    fn a_guard_past_its_deadline_reverts_nothing() {
        let mut watch = watch(Duration::from_secs(5));
        let later = Instant::now() + Duration::from_secs(6);
        assert_eq!(watch.decide(42, later, || false), None);
    }
}
