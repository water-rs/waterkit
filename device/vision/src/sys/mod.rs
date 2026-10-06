//! Platform implementations of vision capabilities.

#[cfg(all(feature = "scanner", target_os = "android"))]
mod android;
#[cfg(all(feature = "scanner", target_os = "ios"))]
mod apple;
#[cfg(all(
    feature = "scanner",
    not(any(target_os = "android", target_os = "ios"))
))]
mod unsupported;

#[cfg(all(feature = "scanner", target_os = "android"))]
pub use android::{scan, scanner_available};
#[cfg(all(feature = "scanner", target_os = "ios"))]
pub use apple::{scan, scanner_available};
#[cfg(all(
    feature = "scanner",
    not(any(target_os = "android", target_os = "ios"))
))]
pub use unsupported::{scan, scanner_available};
