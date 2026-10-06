//! Platform realization dispatch.
//!
//! Every platform module defines the same surface: `capabilities()` plus a
//! `plan_*` function and plan type per request feature.

#[cfg(all(target_os = "android", any(feature = "barcode", feature = "text")))]
mod android;
#[cfg(any(
    not(target_os = "android"),
    not(any(feature = "barcode", feature = "text"))
))]
mod unsupported;

#[cfg(all(target_os = "android", any(feature = "barcode", feature = "text")))]
pub use android::*;
#[cfg(any(
    not(target_os = "android"),
    not(any(feature = "barcode", feature = "text"))
))]
pub use unsupported::*;
