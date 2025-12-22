//! Windows permission implementation using WinRT.

use crate::{Permission, PermissionError, PermissionStatus};

pub(crate) async fn check(permission: Permission) -> PermissionStatus {
    match permission {
        Permission::Location => check_location().await,
        _ => PermissionStatus::Granted, // Most permissions are implicit on Windows
    }
}

pub(crate) async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    match permission {
        Permission::Location => request_location().await,
        _ => Ok(PermissionStatus::Granted),
    }
}

async fn check_location() -> PermissionStatus {
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator};

    match Geolocator::RequestAccessAsync() {
        Ok(op) => match op.get() {
            Ok(status) => match status {
                GeolocationAccessStatus::Allowed => PermissionStatus::Granted,
                GeolocationAccessStatus::Denied => PermissionStatus::Denied,
                GeolocationAccessStatus::Unspecified => PermissionStatus::NotDetermined,
                _ => PermissionStatus::NotDetermined,
            },
            Err(_) => PermissionStatus::NotDetermined,
        },
        Err(_) => PermissionStatus::NotDetermined,
    }
}

async fn request_location() -> Result<PermissionStatus, PermissionError> {
    // On Windows, RequestAccessAsync both checks and requests if needed
    Ok(check_location().await)
}
