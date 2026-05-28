//! Shared desktop permission semantics.

use crate::Permission;

pub(super) const fn has_implicit_desktop_grant(permission: Permission) -> bool {
    matches!(
        permission,
        Permission::Camera
            | Permission::Microphone
            | Permission::Notification
            | Permission::Bluetooth
            | Permission::BluetoothScan
            | Permission::BluetoothConnect
            | Permission::Nfc
            | Permission::HealthRead(_)
            | Permission::HealthWrite(_)
    )
}
