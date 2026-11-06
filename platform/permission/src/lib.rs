//! Cross-platform permission request handling.
//!
//! Type definitions ([`Permission`], [`PermissionStatus`],
//! [`PermissionError`], [`HealthDataKind`]) live in
//! [`waterkit_core::permission`]. This crate provides the platform calls:
//!
//! - [`check`] — async query of the current authorization status.
//! - [`request`] — async runtime request flow.
//!
//! ## Android
//!
//! On Android, the async [`check`] and [`request`] functions automatically
//! use `ndk-context` to resolve the current `Activity`. For advanced JNI
//! integration, see [`android::check_with_activity`] etc.
//!
//! ## macOS privacy grants
//!
//! [`Permission::ScreenRecording`], [`Permission::Accessibility`] and
//! [`Permission::DirectScreenCapture`] run in helper instances of the app
//! itself: macOS caches a negative answer in the process that asked, so
//! status is read in a fresh child process, and a Screen Recording request
//! reaches TCC only from an instance `LaunchServices` launches. The app's
//! `main` must therefore hand those launches to
//! [`run_helper_entry_if_requested`] before doing anything else; without it
//! a status read fails with an error naming that function.
//!
//! Requests also need the app to run from its `.app` bundle, signed as a
//! whole with a certificate: TCC identifies the app by its designated
//! requirement, which an ad hoc signature reduces to a hash that every
//! rebuild changes, and macOS 27 credits Accessibility to no ad hoc bundle.
//! A request from any other process fails with [`PermissionError::Platform`].
//!
//! On every other platform these permissions have no mapping: [`check`]
//! returns [`PermissionStatus::NotDetermined`] and [`request`] returns
//! [`PermissionError::Unsupported`].

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

#[cfg(target_os = "macos")]
mod helper;
mod sys;

use std::process::ExitCode;

#[doc(no_inline)]
pub use waterkit_core::permission::{
    HealthDataKind, Permission, PermissionError, PermissionStatus,
};

/// Android-specific JNI helpers for permission handling.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::android::{check_with_activity, request_with_activity};
}

#[doc = include_str!("../doc/helper_entry.md")]
#[must_use]
#[cfg(target_os = "macos")]
pub fn run_helper_entry_if_requested() -> Option<ExitCode> {
    helper::HelperCommand::parse(std::env::args_os()).map(sys::apple::grants::run)
}

#[doc = include_str!("../doc/helper_entry.md")]
#[must_use]
#[cfg(not(target_os = "macos"))]
pub const fn run_helper_entry_if_requested() -> Option<ExitCode> {
    None
}

/// Checks the current status of a permission without prompting the user.
///
/// # Panics
///
/// On macOS, panics when the status of [`Permission::ScreenRecording`],
/// [`Permission::Accessibility`] or [`Permission::DirectScreenCapture`]
/// cannot be read, most often because the app's `main` does not call
/// [`run_helper_entry_if_requested`].
#[cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "the browser answers through `JsFuture`s and JS callbacks, which are bound to the thread that created them"
    )
)]
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
/// platform mapping; [`PermissionError::Prerequisite`] when it builds on a
/// permission that is not granted yet (macOS direct capture needs Screen
/// Recording); [`PermissionError::Platform`] for OS-specific failures (JNI
/// errors, D-Bus errors, a macOS app not running from a certificate-signed
/// bundle, ...).
///
/// The macOS privacy grants answer asynchronously: their request returns
/// the status in force once the system prompt is up, and the user may
/// still be deciding.
#[cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "the browser answers through `JsFuture`s and JS callbacks, which are bound to the thread that created them"
    )
)]
pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    sys::request(permission).await
}
