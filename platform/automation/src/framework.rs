//! Frameworks opened at run time, whose private functions are resolved by
//! name.
//!
//! Nothing links a private framework or symbol: the framework is opened with
//! `dlopen` and each function is resolved with `dlsym`, so a library user
//! needs no private-framework search path and a missing symbol is an
//! [`Error::Unavailable`] at the call that needs it.

use core::ffi::{CStr, c_void};
use core::ptr::NonNull;

use crate::Error;

const SKYLIGHT: &CStr = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
const HISERVICES: &CStr = c"/System/Library/Frameworks/ApplicationServices.framework/Frameworks/HIServices.framework/HIServices";

/// A reference on a loaded framework image, released on drop.
#[derive(Debug)]
pub struct Framework(NonNull<c_void>);

// SAFETY: the handle is an opaque reference count on a loaded image; dyld's
// `dlsym` and `dlclose` take it from any thread.
unsafe impl Send for Framework {}
// SAFETY: as above; nothing is reached through a shared handle but `dlsym`.
unsafe impl Sync for Framework {}

impl Framework {
    /// The private `SkyLight` framework, `WindowServer`'s client library.
    pub fn skylight() -> Result<Self, Error> {
        Self::open(SKYLIGHT, "SkyLight.framework")
    }

    /// `HIServices`, which holds Accessibility and its private window lookup.
    pub fn hiservices() -> Result<Self, Error> {
        Self::open(HISERVICES, "HIServices.framework")
    }

    fn open(path: &CStr, name: &'static str) -> Result<Self, Error> {
        // SAFETY: `path` is NUL-terminated; `dlopen` has no other requirement.
        let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        NonNull::new(handle)
            .map(Self)
            .ok_or(Error::Unavailable(name))
    }

    /// Resolves the function `name`.
    ///
    /// # Safety
    ///
    /// `F` must be the `unsafe extern "C" fn` pointer type of the symbol's
    /// real signature, and the pointer must not be called after `self` drops.
    pub unsafe fn function<F: Copy>(&self, name: &'static CStr) -> Result<F, Error> {
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

impl Drop for Framework {
    fn drop(&mut self) {
        // SAFETY: the handle came from `dlopen` and is closed once.
        let status = unsafe { libc::dlclose(self.0.as_ptr()) };
        assert_eq!(status, 0, "dlclose of a framework failed");
    }
}
