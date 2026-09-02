//! Apple platform (iOS/macOS) permission implementation using swift-bridge.

use crate::{Permission, PermissionError, PermissionStatus};

#[swift_bridge::bridge]
mod ffi {
    // Shared enum bridged between Rust and Swift
    enum PermissionType {
        Location,
        Camera,
        Microphone,
        Photos,
        Contacts,
        Calendar,
    }

    enum PermissionResult {
        NotDetermined,
        Restricted,
        Denied,
        Granted,
    }

    extern "Swift" {
        fn check_permission(permission: PermissionType) -> PermissionResult;
        fn request_permission(permission: PermissionType) -> PermissionResult;
        fn request_location_permission(callback: Box<dyn FnOnce(PermissionResult) -> ()>);
    }
}

/// Maps a [`Permission`] to its Swift counterpart, returning `None` for
/// variants that have no Apple mapping yet (Bluetooth runtime permission
/// is implicit on Apple, NFC has no equivalent gating, etc.).
const fn permission_to_ffi(permission: Permission) -> Option<ffi::PermissionType> {
    Some(match permission {
        Permission::Location | Permission::LocationWhenInUse | Permission::LocationAlways => {
            ffi::PermissionType::Location
        }
        Permission::Camera => ffi::PermissionType::Camera,
        Permission::Microphone => ffi::PermissionType::Microphone,
        Permission::Photos => ffi::PermissionType::Photos,
        Permission::Contacts => ffi::PermissionType::Contacts,
        Permission::Calendar => ffi::PermissionType::Calendar,
        // Reminders, Bluetooth*, Nfc, Notification, SpeechRecognition,
        // Tracking, MediaLibrary, BodySensors, HealthRead/Write — not yet
        // bridged through Permission.swift; falls through with `None`.
        // The wildcard also catches any future `Permission` variants
        // added to waterkit-core before they are wired up.
        _ => return None,
    })
}

const fn status_from_ffi(result: ffi::PermissionResult) -> PermissionStatus {
    match result {
        ffi::PermissionResult::NotDetermined => PermissionStatus::NotDetermined,
        ffi::PermissionResult::Restricted => PermissionStatus::Restricted,
        ffi::PermissionResult::Denied => PermissionStatus::Denied,
        ffi::PermissionResult::Granted => PermissionStatus::Granted,
    }
}

/// Checks the status of a permission on Apple platforms.
///
/// Returns [`PermissionStatus::NotDetermined`] for permissions that have
/// no Apple bridge yet — callers should pair this with [`request`] which
/// returns a typed error.
pub async fn check(permission: Permission) -> PermissionStatus {
    permission_to_ffi(permission).map_or(PermissionStatus::NotDetermined, |p| {
        status_from_ffi(ffi::check_permission(p))
    })
}

/// Requests a permission on Apple platforms.
///
/// # Errors
///
/// Returns [`PermissionError::Unsupported`] for permissions that have no
/// Apple bridge.
pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    if matches!(
        permission,
        Permission::Location | Permission::LocationWhenInUse | Permission::LocationAlways
    ) {
        let (sender, receiver) = futures::channel::oneshot::channel();
        ffi::request_location_permission(Box::new(move |result| {
            let _ = sender.send(result);
        }));
        return receiver
            .await
            .map(status_from_ffi)
            .map_err(|_| PermissionError::Platform("location permission callback dropped".into()));
    }
    let permission_type = permission_to_ffi(permission).ok_or(PermissionError::Unsupported)?;
    Ok(status_from_ffi(ffi::request_permission(permission_type)))
}
