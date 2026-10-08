//! Platform-specific native realizations of the barcode request.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::*;

#[cfg(not(target_os = "android"))]
mod unsupported;
#[cfg(not(target_os = "android"))]
pub use unsupported::*;
