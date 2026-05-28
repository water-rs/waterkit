//! Windows permission implementation using `WinRT`.

use crate::{Permission, PermissionError, PermissionStatus};

use super::desktop::has_implicit_desktop_grant;

pub async fn check(permission: Permission) -> PermissionStatus {
    match permission {
        Permission::Location | Permission::LocationWhenInUse | Permission::LocationAlways => {
            check_location().await
        }
        permission if has_implicit_desktop_grant(permission) => PermissionStatus::Granted,
        _ => PermissionStatus::NotDetermined,
    }
}

pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    match permission {
        Permission::Location | Permission::LocationWhenInUse | Permission::LocationAlways => {
            request_location().await
        }
        permission if has_implicit_desktop_grant(permission) => Ok(PermissionStatus::Granted),
        _ => Err(PermissionError::Unsupported),
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
