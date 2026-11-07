//! What an application reports through Accessibility about where its input
//! goes: whether it believes it is active, and which window is its key
//! window; and its windows, which Accessibility can move.

use core::ffi::c_void;
use core::fmt;
use core::ptr::{self, NonNull};
use core::time::Duration;

use objc2_application_services::{AXError, AXObserver, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFRetained, CFRunLoop, CFRunLoopMode, CFRunLoopSource, CFString, CFType,
    CGPoint, CGRect, CGSize, Type,
};

use crate::Error;
use crate::error::platform;
use crate::framework::Framework;

/// `AXError _AXUIElementGetWindow(AXUIElementRef, CGWindowID *)`.
type GetWindowFn = unsafe extern "C" fn(*const AXUIElement, *mut u32) -> AXError;

/// The notifications after which an application's report is read again.
const NOTIFICATIONS: [&str; 3] = [
    "AXApplicationActivated",
    "AXFocusedWindowChanged",
    "AXMainWindowChanged",
];

/// Maps an Accessibility window to its `WindowServer` window number.
#[derive(Debug)]
pub struct WindowLookup {
    get_window: GetWindowFn,
    // Declared last: the function above stays loaded until it drops.
    _hiservices: Framework,
}

impl WindowLookup {
    pub fn resolve() -> Result<Self, Error> {
        let hiservices = Framework::hiservices()?;
        // SAFETY: the type is the function's signature as `HIServices`
        // exports it; the pointer lives in `Self` beside the handle.
        let get_window = unsafe { hiservices.function(c"_AXUIElementGetWindow")? };
        Ok(Self {
            get_window,
            _hiservices: hiservices,
        })
    }

    fn window_number(&self, window: &AXUIElement) -> Result<u32, Error> {
        let mut number = 0_u32;
        // SAFETY: `window` is a live element and `number` a valid out pointer.
        let status = unsafe { (self.get_window)(window, &raw mut number) };
        if status == AXError::Success {
            Ok(number)
        } else {
            Err(platform("_AXUIElementGetWindow", status))
        }
    }
}

/// What an application reported about its focus: whether it believes it is
/// active, and its key window. `None` is a question it did not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusReport {
    active: Option<bool>,
    key_window: Option<u32>,
}

impl FocusReport {
    /// Whether the application reports itself active (`AXFrontmost`).
    #[must_use]
    pub const fn active(&self) -> Option<bool> {
        self.active
    }

    /// The window number of the window it reports focused
    /// (`AXFocusedWindow`), where its key events go.
    #[must_use]
    pub const fn key_window(&self) -> Option<u32> {
        self.key_window
    }

    /// Whether input the application receives goes to `window`.
    pub(crate) fn settled_on(self, window: u32) -> bool {
        self.active == Some(true) && self.key_window == Some(window)
    }
}

impl fmt::Display for FocusReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.active {
            Some(true) => write!(f, "reports itself active")?,
            Some(false) => write!(f, "reports itself inactive")?,
            None => write!(f, "does not report whether it is active")?,
        }
        match self.key_window {
            Some(window) => write!(f, ", with window {window} focused"),
            None => write!(f, ", with no focused window"),
        }
    }
}

/// One application as Accessibility sees it.
pub struct Application<'a> {
    pid: libc::pid_t,
    element: CFRetained<AXUIElement>,
    lookup: &'a WindowLookup,
}

impl<'a> Application<'a> {
    /// The application `pid`, whose every answer must come within `timeout`.
    pub fn new(
        pid: libc::pid_t,
        lookup: &'a WindowLookup,
        timeout: Duration,
    ) -> Result<Self, Error> {
        // SAFETY: any pid names an application element; a pid without one
        // fails the reads below instead.
        let element = unsafe { AXUIElement::new_application(pid) };
        // SAFETY: `element` is a live application element.
        let status = unsafe { element.set_messaging_timeout(timeout.as_secs_f32()) };
        if status != AXError::Success {
            return Err(platform("AXUIElementSetMessagingTimeout", status));
        }
        Ok(Self {
            pid,
            element,
            lookup,
        })
    }

    /// What the application reports now.
    pub fn report(&self) -> Result<FocusReport, Error> {
        let active = self
            .attribute("AXFrontmost")?
            .and_then(|value| value.downcast::<CFBoolean>().ok())
            .map(|value| value.as_bool());
        let key_window = self
            .attribute("AXFocusedWindow")?
            .and_then(|value| value.downcast::<AXUIElement>().ok())
            .map(|window| self.lookup.window_number(&window))
            .transpose()?;
        Ok(FocusReport { active, key_window })
    }

    /// The value of `name`, or `None` when the application has none or does
    /// not answer in time.
    fn attribute(&self, name: &'static str) -> Result<Option<CFRetained<CFType>>, Error> {
        attribute(&self.element, self.pid, name)
    }

    /// The windows the application reports (`AXWindows`), with their window
    /// numbers; none while it does not answer.
    pub fn windows(&self) -> Result<Vec<Window>, Error> {
        let Some(windows) = self
            .attribute("AXWindows")?
            .and_then(|value| value.downcast::<CFArray>().ok())
        else {
            return Ok(Vec::new());
        };
        // SAFETY: `AXWindows` is an array of window elements.
        let windows: &CFArray<AXUIElement> = unsafe { windows.cast_unchecked() };
        windows
            .iter()
            .map(|element| {
                Ok(Window {
                    id: self.lookup.window_number(&element)?,
                    pid: self.pid,
                    element,
                })
            })
            .collect()
    }

    /// Wakes `run_loop` in `mode` whenever the application changes its
    /// active state or key window, until the observation drops, for a wait
    /// on it to focus `window`.
    pub fn observe(
        &self,
        window: u32,
        run_loop: &CFRunLoop,
        mode: &CFRunLoopMode,
    ) -> Result<Observation, Error> {
        let mut observer: *mut AXObserver = ptr::null_mut();
        // SAFETY: `wake` matches the observer callback signature and
        // `observer` is a valid out pointer.
        let status =
            unsafe { AXObserver::create(self.pid, Some(wake), NonNull::from(&mut observer)) };
        if status != AXError::Success {
            return Err(platform("AXObserverCreate", status));
        }
        let observer = NonNull::new(observer)
            .ok_or_else(|| Error::Platform("AXObserverCreate returned NULL".to_owned()))?;
        // SAFETY: a created observer is +1 and owned here.
        let observer = unsafe { CFRetained::from_raw(observer) };
        for name in NOTIFICATIONS {
            let notification = CFString::from_static_str(name);
            // SAFETY: `wake` reads no refcon.
            let status =
                unsafe { observer.add_notification(&self.element, &notification, ptr::null_mut()) };
            match status {
                AXError::Success => {}
                AXError::NotificationUnsupported => {
                    return Err(Error::NotificationUnsupported {
                        pid: self.pid,
                        window,
                        notification: name,
                    });
                }
                status => return Err(platform("AXObserverAddNotification", status)),
            }
        }
        // SAFETY: the observer is live.
        let source = unsafe { observer.run_loop_source() };
        run_loop.add_source(Some(&source), Some(mode));
        Ok(Observation {
            _observer: observer,
            source,
            run_loop: run_loop.retain(),
            mode: mode.retain(),
        })
    }
}

/// The value of `name` on `element` of `pid`, or `None` when the element has
/// none or its application does not answer in time.
fn attribute(
    element: &AXUIElement,
    pid: libc::pid_t,
    name: &'static str,
) -> Result<Option<CFRetained<CFType>>, Error> {
    let name = CFString::from_static_str(name);
    let mut value: *const CFType = ptr::null();
    // SAFETY: `value` is a valid out pointer; on success it holds a +1
    // reference the caller owns.
    let status = unsafe { element.copy_attribute_value(&name, NonNull::from(&mut value)) };
    match status {
        // SAFETY: the copied value is +1 and owned here.
        AXError::Success => {
            Ok(NonNull::new(value.cast_mut()).map(|value| unsafe { CFRetained::from_raw(value) }))
        }
        AXError::NoValue | AXError::AttributeUnsupported | AXError::CannotComplete => {
            tracing::trace!(pid, %name, ?status, "no answer for the attribute");
            Ok(None)
        }
        status => Err(platform("AXUIElementCopyAttributeValue", status)),
    }
}

/// One window of an application as Accessibility sees it.
pub struct Window {
    id: u32,
    pid: libc::pid_t,
    element: CFRetained<AXUIElement>,
}

impl Window {
    /// The window's `WindowServer` number.
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// The window's frame (`AXPosition`, `AXSize`), in global display
    /// coordinates.
    pub fn frame(&self) -> Result<CGRect, Error> {
        let origin = self.value("AXPosition", AXValueType::CGPoint, CGPoint::ZERO)?;
        let size = self.value("AXSize", AXValueType::CGSize, CGSize::ZERO)?;
        Ok(CGRect::new(origin, size))
    }

    /// Moves the window's top-left corner to `origin`.
    pub fn set_origin(&self, origin: CGPoint) -> Result<(), Error> {
        self.set_value("AXPosition", AXValueType::CGPoint, &origin)
    }

    /// Resizes the window to `size`.
    pub fn set_size(&self, size: CGSize) -> Result<(), Error> {
        self.set_value("AXSize", AXValueType::CGSize, &size)
    }

    /// The `kind` value of `name`, decoded over `empty`.
    fn value<T: Copy>(&self, name: &'static str, kind: AXValueType, empty: T) -> Result<T, Error> {
        let value = attribute(&self.element, self.pid, name)?
            .and_then(|value| value.downcast::<AXValue>().ok())
            .ok_or_else(|| {
                Error::Platform(format!(
                    "window {} of pid {} reports no {name}",
                    self.id, self.pid
                ))
            })?;
        let mut decoded = empty;
        // SAFETY: `decoded` is the structure `kind` encodes.
        if unsafe { value.value(kind, NonNull::from(&mut decoded).cast()) } {
            Ok(decoded)
        } else {
            Err(Error::Platform(format!(
                "window {} of pid {} reports {name} of another type",
                self.id, self.pid
            )))
        }
    }

    /// Sets `name` to `value`, a structure of kind `kind`.
    fn set_value<T>(&self, name: &'static str, kind: AXValueType, value: &T) -> Result<(), Error> {
        // SAFETY: `value` is the structure `kind` encodes; `AXValueCreate`
        // only reads it.
        let encoded = unsafe { AXValue::new(kind, NonNull::from(value).cast()) }
            .ok_or_else(|| Error::Platform("AXValueCreate returned NULL".to_owned()))?;
        let attribute = CFString::from_static_str(name);
        // SAFETY: `encoded` holds the type the attribute takes.
        let status = unsafe { self.element.set_attribute_value(&attribute, &encoded) };
        if status == AXError::Success {
            Ok(())
        } else {
            Err(Error::Platform(format!(
                "setting {name} of window {} of pid {} failed with {status:?}",
                self.id, self.pid
            )))
        }
    }
}

/// An application's notifications waking a run loop mode.
pub struct Observation {
    _observer: CFRetained<AXObserver>,
    source: CFRetained<CFRunLoopSource>,
    run_loop: CFRetained<CFRunLoop>,
    mode: CFRetained<CFRunLoopMode>,
}

impl Drop for Observation {
    fn drop(&mut self) {
        self.run_loop
            .remove_source(Some(&self.source), Some(&self.mode));
    }
}

/// Handling the notification is the wake-up: the waiter reads the report
/// again.
const unsafe extern "C-unwind" fn wake(
    _observer: NonNull<AXObserver>,
    _element: NonNull<AXUIElement>,
    _notification: NonNull<CFString>,
    _refcon: *mut c_void,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_lookup_resolves() {
        WindowLookup::resolve().expect("_AXUIElementGetWindow resolves");
    }

    #[test]
    fn a_report_describes_itself() {
        let settled = FocusReport {
            active: Some(true),
            key_window: Some(7),
        };
        assert!(settled.settled_on(7));
        assert!(!settled.settled_on(8));
        assert_eq!(
            settled.to_string(),
            "reports itself active, with window 7 focused"
        );
        let background = FocusReport {
            active: Some(false),
            key_window: Some(7),
        };
        assert!(!background.settled_on(7));
        let silent = FocusReport {
            active: None,
            key_window: None,
        };
        assert_eq!(
            silent.to_string(),
            "does not report whether it is active, with no focused window"
        );
    }
}
