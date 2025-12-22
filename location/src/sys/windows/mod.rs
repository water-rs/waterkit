//! Windows location implementation using WinRT Geolocator.

use crate::{Location, LocationError};

pub(crate) async fn get_location() -> Result<Location, LocationError> {
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator};

    // Request access (this also serves as permission check on Windows)
    let access = Geolocator::RequestAccessAsync()
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?
        .get()
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?;

    match access {
        GeolocationAccessStatus::Denied => return Err(LocationError::PermissionDenied),
        GeolocationAccessStatus::Unspecified => return Err(LocationError::NotAvailable),
        GeolocationAccessStatus::Allowed => {}
        _ => return Err(LocationError::NotAvailable),
    }

    let geolocator = Geolocator::new().map_err(|e| LocationError::Unknown(e.message().to_string()))?;

    let position = geolocator
        .GetGeopositionAsync()
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?
        .get()
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

    let timestamp = coord
        .Timestamp()
        .map_err(|e| LocationError::Unknown(e.message().to_string()))?
        .UniversalTime()
        .unwrap_or(0) as u64;

    let accuracy = coord.Accuracy().ok().map(|a| a.GetDouble().unwrap_or(0.0));

    Ok(Location {
        latitude: pos.Latitude,
        longitude: pos.Longitude,
        altitude: Some(pos.Altitude),
        horizontal_accuracy: accuracy,
        vertical_accuracy: None,
        timestamp,
    })
}
