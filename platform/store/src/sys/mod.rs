//! Platform dispatch: Android goes through a Kotlin helper over JNI, Apple
//! through a `StoreKit` 2 Swift bridge, and everything else is unsupported.

#[cfg(any(target_os = "android", target_os = "ios", target_os = "macos", test))]
pub mod wire;

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

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
pub use unsupported::*;
