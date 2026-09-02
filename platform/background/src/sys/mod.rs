//! Platform-specific background task implementations.

#[cfg(target_os = "ios")]
mod apple;
#[cfg(target_os = "ios")]
pub use apple::*;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::*;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod fallback;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub use fallback::*;
