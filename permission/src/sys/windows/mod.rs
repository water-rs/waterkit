//! Windows permission implementation using `WinRT`.

use crate::{Permission, PermissionError, PermissionStatus};

pub async fn check(permission: Permission) -> PermissionStatus {
    match permission {
        Permission::Location | Permission::LocationWhenInUse | Permission::LocationAlways => {
            check_location().await
        }
        // Most other permissions are implicit on classic Windows desktop;
        // capability crates that need a stricter gate (Bluetooth runtime
        // capability, etc.) will refine this match in their own dedicated
        // platform code.
        _ => PermissionStatus::Granted,
    }
}

pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    match permission {
        Permission::Location | Permission::LocationWhenInUse | Permission::LocationAlways => {
            request_location().await
        }
        _ => Ok(PermissionStatus::Granted),
    }
}

async fn check_location() -> PermissionStatus {
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator};

    let Ok(op) = Geolocator::RequestAccessAsync() else {
        return PermissionStatus::NotDetermined;
    };

    op.await
        .map_or(PermissionStatus::NotDetermined, |status| match status {
            GeolocationAccessStatus::Allowed => PermissionStatus::Granted,
            GeolocationAccessStatus::Denied => PermissionStatus::Denied,
            _ => PermissionStatus::NotDetermined,
        })
}

async fn request_location() -> Result<PermissionStatus, PermissionError> {
    // On Windows, `RequestAccessAsync` both checks and requests if needed
    Ok(check_location().await)
}
