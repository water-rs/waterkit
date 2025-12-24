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
    }
}

const fn permission_to_ffi(permission: Permission) -> ffi::PermissionType {
    match permission {
        Permission::Location => ffi::PermissionType::Location,
        Permission::Camera => ffi::PermissionType::Camera,
        Permission::Microphone => ffi::PermissionType::Microphone,
        Permission::Photos => ffi::PermissionType::Photos,
        Permission::Contacts => ffi::PermissionType::Contacts,
        Permission::Calendar => ffi::PermissionType::Calendar,
    }
}

const fn status_from_ffi(result: ffi::PermissionResult) -> PermissionStatus {
    match result {
        ffi::PermissionResult::NotDetermined => PermissionStatus::NotDetermined,
        ffi::PermissionResult::Restricted => PermissionStatus::Restricted,
        ffi::PermissionResult::Denied => PermissionStatus::Denied,
        ffi::PermissionResult::Granted => PermissionStatus::Granted,
    }
}

/// Check the status of a permission on Apple platforms.
pub async fn check(permission: Permission) -> PermissionStatus {
    let result = ffi::check_permission(permission_to_ffi(permission));
    status_from_ffi(result)
}

/// Request a permission on Apple platforms.
///
/// # Errors
/// Always returns `Ok` as Apple's request API returns the status directly.
pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    let result = ffi::request_permission(permission_to_ffi(permission));
    Ok(status_from_ffi(result))
}
