//! Windows location implementation using `WinRT` Geolocator.

use crate::{Location, LocationError, Timestamp};

pub async fn get_location() -> Result<Location, LocationError> {
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator};

    // Request access (this also serves as permission check on Windows)
    let access = Geolocator::RequestAccessAsync()
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?
        .await
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?;

    match access {
        GeolocationAccessStatus::Denied => return Err(LocationError::PermissionDenied),
        GeolocationAccessStatus::Unspecified => return Err(LocationError::NotAvailable),
        GeolocationAccessStatus::Allowed => {}
        _ => return Err(LocationError::NotAvailable),
    }

    let geolocator =
        Geolocator::new().map_err(|e| LocationError::Unknown(e.message().to_string()))?;

    let position = geolocator
        .GetGeopositionAsync()
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?
        .await
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?;

    let coord = position
        .Coordinate()
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?;

    let point = coord
        .Point()
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?;

    let pos = point
        .Position()
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?;

    // Windows DateTime.UniversalTime is 100-nanosecond intervals since 1601-01-01
    // Convert to Unix timestamp (seconds since 1970-01-01)
    let filetime = coord
        .Timestamp()
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?
        .UniversalTime;

    // FILETIME epoch offset: 11644473600 seconds between 1601-01-01 and 1970-01-01
    const FILETIME_UNIX_DIFF: i64 = 11_644_473_600;
    let unix_seconds = (filetime / 10_000_000) - FILETIME_UNIX_DIFF;

    let timestamp =
        Timestamp::from_second(unix_seconds).map_err(|e| LocationError::Unknown(e.to_string()))?;

    let accuracy = coord.Accuracy().ok().map(|a| a.Value().unwrap_or(0.0));

    let mut location =
        Location::new(pos.Latitude, pos.Longitude, timestamp).with_altitude(pos.Altitude);

    if let Some(acc) = accuracy {
        location = location.with_horizontal_accuracy(acc);
    }

    Ok(location)
}
