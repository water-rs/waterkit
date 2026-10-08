//! Platform-specific translation implementations.

#[cfg(any(target_os = "android", target_os = "ios", target_os = "macos", test))]
mod wire;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
mod fallback;

#[cfg(target_os = "android")]
pub use android::{Translator, capabilities, pair_status};
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::{Translator, capabilities, pair_status};
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
pub use fallback::{Translator, capabilities, pair_status};
