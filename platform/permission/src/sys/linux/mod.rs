//! Linux permission implementation.
//!
//! On Linux, most permissions are handled at the system level via:
//! - File permissions (camera/microphone devices in /dev)
//! - Desktop portal systems (Flatpak/Snap sandboxing)
//! - User groups (e.g., 'video' group for camera access)
//!
//! For `GeoClue` (location), the application just needs to connect to the D-Bus service.

use crate::{Permission, PermissionError, PermissionStatus};

use super::desktop::has_implicit_desktop_grant;

pub async fn check(permission: Permission) -> PermissionStatus {
    if has_implicit_desktop_grant(permission) {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotDetermined
    }
}

pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    if has_implicit_desktop_grant(permission) {
        Ok(PermissionStatus::Granted)
    } else {
        Err(PermissionError::Unsupported)
    }
}
