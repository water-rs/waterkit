//! Platform dispatch: Android goes through a Kotlin helper over JNI, Apple
//! through a `StoreKit` 2 Swift bridge, Windows through
//! `Windows.Services.Store.StoreContext`, and everything else is
//! unsupported.

#[cfg(any(target_os = "android", target_os = "ios", target_os = "macos", test))]
pub mod wire;

/// Pure mappings for the Windows backend, host-tested everywhere.
#[cfg(any(target_os = "windows", test))]
pub mod mapping;

/// The transaction feed a backend streams from `Store::connect`; the public
/// [`crate::StoreEvents`] wraps it.
pub type EventStream =
    futures::stream::BoxStream<'static, Result<crate::Purchase, crate::StoreError>>;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::*;

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "macos",
    target_os = "windows"
)))]
mod unsupported;
#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "macos",
    target_os = "windows"
)))]
pub use unsupported::*;
