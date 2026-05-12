//! Cross-platform permission request handling.
//!
//! Type definitions ([`Permission`], [`PermissionStatus`],
//! [`PermissionError`], [`HealthDataKind`]) live in
//! [`waterkit_core::permission`]. This crate provides the platform calls:
//!
//! - [`check`] — async query of the current authorization status.
//! - [`request`] — async runtime request flow.
//! - [`status`] — reactive [`Subscribed<PermissionStatus>`] seeded by
//!   the current status.
//!
//! ## Android
//!
//! On Android, the async [`check`] and [`request`] functions automatically
//! use `ndk-context` to resolve the current `Activity`. For advanced JNI
//! integration, see [`android::init_with_activity`] etc.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod sys;

#[doc(no_inline)]
pub use waterkit_core::permission::{HealthDataKind, Permission, PermissionError, PermissionStatus};
use waterkit_core::{Subscribed, subscribed};

/// Android-specific JNI helpers for permission handling.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::android::{check_with_activity, init_with_activity, request_with_activity};
}

/// Checks the current status of a permission without prompting the user.
pub async fn check(permission: Permission) -> PermissionStatus {
    sys::check(permission).await
}

/// Triggers the runtime permission request flow.
///
/// If the permission has already been granted or denied, returns the
/// current status without prompting again. On Android the result may be
/// [`PermissionStatus::NotDetermined`] until the host Activity receives
/// and applies the permission callback result.
///
/// # Errors
///
/// Returns [`PermissionError::Unsupported`] if the permission has no
/// platform mapping; [`PermissionError::Platform`] for OS-specific
/// failures (JNI errors, D-Bus errors, ...).
pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    sys::request(permission).await
}

/// Reactive view of a permission's authorization status.
///
/// Returns a [`Subscribed<PermissionStatus>`] seeded with the current
/// status from [`check`]. The returned signal is intended to be the
/// long-lived input to UI code that toggles its presentation based on the
/// authorization state. The associated sink is held privately; future work
/// (a platform-specific change-listener registration) will push updates
/// when the user toggles the permission outside the app.
#[allow(clippy::missing_panics_doc)]
pub async fn status(permission: Permission) -> Subscribed<PermissionStatus> {
    let initial = check(permission).await;
    let (sub, _sink) = subscribed(initial);
    // TODO(waterkit-permission #1): wire platform-specific change listeners
    //   (Apple `CLLocationManager` delegate, Android registerReceiver,
    //   Windows `WatchPermissions`) and forward via `_sink`. For now the
    //   signal carries only the initial probe.
    sub
}
