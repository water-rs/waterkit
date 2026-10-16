//! Platform-specific native realizations of the document request.

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::*;

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
pub use unsupported::*;
