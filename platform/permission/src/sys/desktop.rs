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

#[cfg(test)]
mod tests {
    use waterkit_core::permission::HealthDataKind;

    use super::{Permission, has_implicit_desktop_grant};

    #[test]
    fn desktop_implicit_grants_match_unprompted_capabilities() {
        let granted = [
            Permission::Camera,
            Permission::Microphone,
            Permission::Notification,
            Permission::Bluetooth,
            Permission::BluetoothScan,
            Permission::BluetoothConnect,
            Permission::Nfc,
            Permission::HealthRead(HealthDataKind::Steps),
            Permission::HealthWrite(HealthDataKind::Steps),
        ];

        for permission in granted {
            assert!(
                has_implicit_desktop_grant(permission),
                "{permission:?} should be implicitly granted on desktop"
            );
        }
    }

    #[test]
    fn desktop_location_and_user_data_do_not_get_implicit_grants() {
        let not_granted = [
            Permission::Location,
            Permission::LocationWhenInUse,
            Permission::LocationAlways,
            Permission::Photos,
            Permission::Contacts,
            Permission::Calendar,
            Permission::Reminders,
            Permission::SpeechRecognition,
            Permission::Tracking,
            Permission::MediaLibrary,
            Permission::BodySensors,
        ];

        for permission in not_granted {
            assert!(
                !has_implicit_desktop_grant(permission),
                "{permission:?} should not be implicitly granted on desktop"
            );
        }
    }
}
