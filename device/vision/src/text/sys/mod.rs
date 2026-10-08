//! Platform-specific native realizations of the text request.

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::*;

#[cfg(not(any(target_os = "windows", target_os = "android")))]
mod unsupported;
#[cfg(not(any(target_os = "windows", target_os = "android")))]
pub use unsupported::*;
