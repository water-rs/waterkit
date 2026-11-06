//! The private `SkyLight` functions this crate calls, `WindowServer`'s client
//! library, resolved at run time ([`Framework`]).

use core::marker::PhantomData;

use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{CGError, CGEvent};

use crate::error::platform;
use crate::framework::Framework;
use crate::{Error, topology};

type DisplayListFn = unsafe extern "C" fn(u32, *mut u32, *mut u32) -> CGError;

/// `WindowServer`'s own display lists, which `CoreGraphics`' per-process
/// view can lag behind.
#[derive(Debug)]
pub struct DisplayLists<'a> {
    registered: DisplayListFn,
    online: DisplayListFn,
    _skylight: PhantomData<&'a Framework>,
}

impl<'a> DisplayLists<'a> {
    pub fn resolve(skylight: &'a Framework) -> Result<Self, Error> {
        // SAFETY: both are `CGError (uint32_t max, CGDirectDisplayID *list,
        // uint32_t *count)`, used only while `skylight` lives.
        unsafe {
            Ok(Self {
                registered: skylight.function(c"SLSGetDisplayList")?,
                online: skylight.function(c"SLSGetOnlineDisplayList")?,
                _skylight: PhantomData,
            })
        }
    }

    /// The displays `WindowServer` has registered, online or not.
    pub fn registered(&self) -> Result<Vec<u32>, Error> {
        // SAFETY: `read_list` passes a buffer of at least `max` entries, or a
        // null buffer with a zero maximum, and a valid count pointer.
        topology::read_list("SLSGetDisplayList", |max, list, count| unsafe {
            (self.registered)(max, list, count)
        })
    }

    /// The displays `WindowServer` has brought into the topology.
    pub fn online(&self) -> Result<Vec<u32>, Error> {
        // SAFETY: as in `registered`.
        topology::read_list("SLSGetOnlineDisplayList", |max, list, count| unsafe {
            (self.online)(max, list, count)
        })
    }
}

/// The address `WindowServer` delivers a process's events to.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessSerialNumber {
    high: u32,
    low: u32,
}

type MainConnectionFn = unsafe extern "C" fn() -> u32;
type WindowOwnerFn = unsafe extern "C" fn(u32, u32, *mut u32) -> CGError;
type ConnectionPsnFn = unsafe extern "C" fn(u32, *mut ProcessSerialNumber) -> CGError;
type PostToPidFn = unsafe extern "C" fn(libc::pid_t, *const CGEvent);
type SetFieldFn = unsafe extern "C" fn(*const CGEvent, u32, i64);
type SetWindowLocationFn = unsafe extern "C" fn(*const CGEvent, CGPoint);

/// The functions [`Input`](crate::Input) posts through. They hold their own
/// reference on `SkyLight`.
#[derive(Debug)]
pub struct EventSymbols {
    main_connection: MainConnectionFn,
    window_owner: WindowOwnerFn,
    connection_psn: ConnectionPsnFn,
    post_to_pid: PostToPidFn,
    set_field: SetFieldFn,
    set_window_location: SetWindowLocationFn,
    // Declared last: the functions above stay loaded until it drops.
    _skylight: Framework,
}

impl EventSymbols {
    pub fn resolve() -> Result<Self, Error> {
        let skylight = Framework::skylight()?;
        // SAFETY: each type is the function's signature as `SkyLight`
        // exports it; the pointers live in `Self` beside the handle.
        unsafe {
            Ok(Self {
                main_connection: skylight.function(c"SLSMainConnectionID")?,
                window_owner: skylight.function(c"SLSGetWindowOwner")?,
                connection_psn: skylight.function(c"SLSGetConnectionPSN")?,
                post_to_pid: skylight.function(c"SLEventPostToPid")?,
                set_field: skylight.function(c"SLEventSetIntegerValueField")?,
                set_window_location: skylight.function(c"CGEventSetWindowLocation")?,
                _skylight: skylight,
            })
        }
    }

    /// Whether an application owns `window`: its owner's connection has a
    /// process serial number, which `WindowServer`'s own connection has not.
    pub fn application_owns(&self, window: u32) -> Result<bool, Error> {
        let connection = self.main_connection();
        let mut owner = 0_u32;
        // SAFETY: `owner` is a valid out pointer.
        let status = unsafe { (self.window_owner)(connection, window, &raw mut owner) };
        if status != CGError::Success {
            return Err(platform("SLSGetWindowOwner", status));
        }
        let mut psn = ProcessSerialNumber { high: 0, low: 0 };
        // SAFETY: `psn` is a valid out pointer of the declared layout.
        let status = unsafe { (self.connection_psn)(owner, &raw mut psn) };
        let no_process = ProcessSerialNumber { high: 0, low: 0 };
        if status != CGError::Success || psn == no_process {
            tracing::trace!(window, ?status, "window owner has no process serial number");
            return Ok(false);
        }
        Ok(true)
    }

    /// This process's `WindowServer` connection.
    pub fn main_connection(&self) -> u32 {
        // SAFETY: the function takes no arguments.
        unsafe { (self.main_connection)() }
    }

    /// Posts `event` to the process `pid`.
    pub fn post_to_pid(&self, pid: libc::pid_t, event: &CGEvent) {
        // SAFETY: `event` is a live event.
        unsafe { (self.post_to_pid)(pid, event) }
    }

    /// Sets the raw event field `field`.
    pub fn set_field(&self, event: &CGEvent, field: u32, value: i64) {
        // SAFETY: `event` is a live event; any field index is accepted.
        unsafe { (self.set_field)(event, field, value) }
    }

    /// Sets the location the receiving window reads the event at.
    pub fn set_window_location(&self, event: &CGEvent, location: CGPoint) {
        // SAFETY: `event` is a live event.
        unsafe { (self.set_window_location)(event, location) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolves every function on this macOS. Opening `SkyLight` and looking
    /// up symbols creates, posts and shows nothing.
    #[test]
    fn every_function_resolves() {
        let skylight = Framework::skylight().expect("SkyLight opens");
        let lists = DisplayLists::resolve(&skylight).expect("the display lists resolve");
        assert!(
            !lists
                .registered()
                .expect("the registered list reads")
                .is_empty(),
            "WindowServer registers at least one display"
        );
        EventSymbols::resolve().expect("the event functions resolve");
    }
}
