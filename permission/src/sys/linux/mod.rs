//! Linux permission implementation.
//!
//! On Linux, most permissions are handled at the system level via:
//! - File permissions (camera/microphone devices in /dev)
//! - Desktop portal systems (Flatpak/Snap sandboxing)
//! - User groups (e.g., 'video' group for camera access)
//!
//! For GeoClue (location), the application just needs to connect to the D-Bus service.

use crate::{Permission, PermissionError, PermissionStatus};

pub(crate) async fn check(_permission: Permission) -> PermissionStatus {
    // Linux permissions are generally handled at the OS/container level
    // Applications typically have access unless sandboxed
    PermissionStatus::Granted
}

pub(crate) async fn request(_permission: Permission) -> Result<PermissionStatus, PermissionError> {
    // No runtime permission prompts on traditional Linux
    // Sandboxed apps (Flatpak/Snap) use portals which handle this differently
    Ok(PermissionStatus::Granted)
}
