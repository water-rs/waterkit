//! Runtime binding of the private `SkyLight` framework.
//!
//! Nothing links `SkyLight`: the framework is opened with `dlopen` and each
//! function is resolved by name, so a library user needs no private-framework
//! search path and a missing symbol is an [`Error::Unavailable`] at the call
//! that needs it.

use core::ffi::{CStr, c_void};
use core::marker::PhantomData;
use core::ptr::NonNull;

use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{CGError, CGEvent};

use crate::error::platform;
use crate::{Error, topology};

const PATH: &CStr = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";

/// A reference on the loaded `SkyLight` image, released on drop.
#[derive(Debug)]
pub struct SkyLight(NonNull<c_void>);

// SAFETY: the handle is an opaque reference count on a loaded image; dyld's
// `dlsym` and `dlclose` take it from any thread.
unsafe impl Send for SkyLight {}
// SAFETY: as above; nothing is reached through a shared handle but `dlsym`.
unsafe impl Sync for SkyLight {}

impl SkyLight {
    pub fn open() -> Result<Self, Error> {
        // SAFETY: `PATH` is NUL-terminated; `dlopen` has no other requirement.
        let handle = unsafe { libc::dlopen(PATH.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        NonNull::new(handle)
            .map(Self)
            .ok_or(Error::Unavailable("SkyLight.framework"))
    }

    /// Resolves the function `name`.
    ///
    /// # Safety
    ///
    /// `F` must be the `unsafe extern "C" fn` pointer type of the symbol's
    /// real signature, and the pointer must not be called after `self` drops.
    unsafe fn function<F: Copy>(&self, name: &'static CStr) -> Result<F, Error> {
        const { assert!(size_of::<F>() == size_of::<*mut c_void>()) };
        // SAFETY: the handle is open and `name` is NUL-terminated.
        let symbol = unsafe { libc::dlsym(self.0.as_ptr(), name.as_ptr()) };
        if symbol.is_null() {
            return Err(Error::Unavailable(
                name.to_str().expect("symbol names are ASCII"),
            ));
        }
        // SAFETY: the caller names the symbol's function pointer type, which
        // has the size of a data pointer (checked above).
        Ok(unsafe { core::mem::transmute_copy::<*mut c_void, F>(&symbol) })
    }
}

impl Drop for SkyLight {
    fn drop(&mut self) {
        // SAFETY: the handle came from `dlopen` and is closed once.
        let status = unsafe { libc::dlclose(self.0.as_ptr()) };
        assert_eq!(status, 0, "dlclose of SkyLight failed");
    }
}

type DisplayListFn = unsafe extern "C" fn(u32, *mut u32, *mut u32) -> CGError;

/// `WindowServer`'s own display lists, which `CoreGraphics`' per-process
/// view can lag behind.
#[derive(Debug)]
pub struct DisplayLists<'a> {
    registered: DisplayListFn,
    online: DisplayListFn,
    _skylight: PhantomData<&'a SkyLight>,
}

impl<'a> DisplayLists<'a> {
    pub fn resolve(skylight: &'a SkyLight) -> Result<Self, Error> {
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
pub struct ProcessSerialNumber {
    high: u32,
    low: u32,
}

type MainConnectionFn = unsafe extern "C" fn() -> u32;
type WindowOwnerFn = unsafe extern "C" fn(u32, u32, *mut u32) -> CGError;
type ConnectionPsnFn = unsafe extern "C" fn(u32, *mut ProcessSerialNumber) -> CGError;
type PostRecordFn = unsafe extern "C" fn(*const ProcessSerialNumber, *const u8) -> CGError;
type PostToPidFn = unsafe extern "C" fn(libc::pid_t, *const CGEvent);
type SetFieldFn = unsafe extern "C" fn(*const CGEvent, u32, i64);
type SetWindowLocationFn = unsafe extern "C" fn(*const CGEvent, CGPoint);

/// Length of a `WindowServer` event record, which its own length field
/// repeats.
pub const RECORD_LEN: usize = 0xF8;

/// The functions [`Input`](crate::Input) posts through. They hold their own
/// reference on `SkyLight`.
#[derive(Debug)]
pub struct EventSymbols {
    main_connection: MainConnectionFn,
    window_owner: WindowOwnerFn,
    connection_psn: ConnectionPsnFn,
    post_record: PostRecordFn,
    post_to_pid: PostToPidFn,
    set_field: SetFieldFn,
    set_window_location: SetWindowLocationFn,
    // Declared last: the functions above stay loaded until it drops.
    _skylight: SkyLight,
}

impl EventSymbols {
    pub fn resolve() -> Result<Self, Error> {
        let skylight = SkyLight::open()?;
        // SAFETY: each type is the function's signature as `SkyLight`
        // exports it; the pointers live in `Self` beside the handle.
        unsafe {
            Ok(Self {
                main_connection: skylight.function(c"SLSMainConnectionID")?,
                window_owner: skylight.function(c"SLSGetWindowOwner")?,
                connection_psn: skylight.function(c"SLSGetConnectionPSN")?,
                post_record: skylight.function(c"SLPSPostEventRecordTo")?,
                post_to_pid: skylight.function(c"SLEventPostToPid")?,
                set_field: skylight.function(c"SLEventSetIntegerValueField")?,
                set_window_location: skylight.function(c"CGEventSetWindowLocation")?,
                _skylight: skylight,
            })
        }
    }

    /// The process serial number of the process owning `window`, or `None`
    /// when its owner is no application: a connection without a process
    /// serial number, such as `WindowServer`'s own.
    pub fn application_psn(&self, window: u32) -> Result<Option<ProcessSerialNumber>, Error> {
        // SAFETY: the function takes no arguments.
        let connection = unsafe { (self.main_connection)() };
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
            return Ok(None);
        }
        Ok(Some(psn))
    }

    /// Queues `record` in the event queue of the process `psn` names.
    pub fn post_record(
        &self,
        psn: ProcessSerialNumber,
        record: &[u8; RECORD_LEN],
    ) -> Result<(), Error> {
        // SAFETY: `record` is a full-length event record.
        let status = unsafe { (self.post_record)(&raw const psn, record.as_ptr()) };
        if status == CGError::Success {
            Ok(())
        } else {
            Err(platform("SLPSPostEventRecordTo", status))
        }
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
        let skylight = SkyLight::open().expect("SkyLight opens");
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
