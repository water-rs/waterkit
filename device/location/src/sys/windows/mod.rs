//! Windows location implementation using `WinRT` Geolocator.

use crate::{Location, LocationError, Timestamp};

// FILETIME epoch offset: 11644473600 seconds between 1601-01-01 and 1970-01-01
const FILETIME_UNIX_DIFF: i64 = 11_644_473_600;

pub async fn get_location() -> Result<Location, LocationError> {
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator};

    let map_err = |e: windows::core::Error| LocationError::Platform(e.to_string());

    // Request access (this also serves as permission check on Windows)
    let access = Geolocator::RequestAccessAsync()
        .map_err(map_err)?
        .await
        .map_err(map_err)?;

    if access == GeolocationAccessStatus::Denied {
        return Err(LocationError::PermissionDenied);
    }
    if access != GeolocationAccessStatus::Allowed {
        return Err(LocationError::NotAvailable);
    }

    let geolocator = Geolocator::new().map_err(map_err)?;

    let position = geolocator
        .GetGeopositionAsync()
        .map_err(map_err)?
        .await
        .map_err(map_err)?;

    let coord = position.Coordinate().map_err(map_err)?;

    let point = coord.Point().map_err(map_err)?;

    let pos = point.Position().map_err(map_err)?;

    // Windows DateTime.UniversalTime is 100-nanosecond intervals since 1601-01-01
    // Convert to Unix timestamp (seconds since 1970-01-01)
    let filetime = coord.Timestamp().map_err(map_err)?.UniversalTime;

    let unix_seconds = (filetime / 10_000_000) - FILETIME_UNIX_DIFF;

    let timestamp =
        Timestamp::from_second(unix_seconds).map_err(|e| LocationError::Platform(e.to_string()))?;

    let accuracy = coord.Accuracy().ok();

    let mut location =
        Location::from_degrees(pos.Latitude, pos.Longitude, timestamp)?.with_altitude(pos.Altitude);

    if let Some(acc) = accuracy {
        location = location.with_horizontal_accuracy(acc);
    }

    Ok(location)
}
