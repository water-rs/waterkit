//! Windows location implementation using `WinRT` Geolocator.

use crate::{Location, LocationError, Timestamp};

// FILETIME epoch offset: 11644473600 seconds between 1601-01-01 and 1970-01-01
const FILETIME_UNIX_DIFF: i64 = 11_644_473_600;

/// Windows `E_ACCESSDENIED`, which `GetGeopositionAsync` returns when the user
/// has denied location access.
const E_ACCESSDENIED: windows::core::HRESULT =
    windows::core::HRESULT(0x8007_0005_u32.cast_signed());

pub async fn get_location() -> Result<Location, LocationError> {
    use windows::Devices::Geolocation::Geolocator;

    // The crate contract says this function never triggers the runtime
    // prompt — waterkit-permission owns that flow — so no RequestAccessAsync
    // here. A denied grant surfaces as E_ACCESSDENIED from the position call.
    let map_err = |e: windows::core::Error| {
        if e.code() == E_ACCESSDENIED {
            LocationError::PermissionDenied
        } else {
            LocationError::Platform(e.to_string())
        }
    };

    let geolocator = Geolocator::new().map_err(map_err)?;

    let position = geolocator
        .GetGeopositionAsync()
        .map_err(map_err)?
        .await
        .map_err(map_err)?;

    let coord = position.Coordinate().map_err(map_err)?;

    let point = coord.Point().map_err(map_err)?;

    let pos = point.Position().map_err(map_err)?;

    // Windows DateTime.UniversalTime is 100-nanosecond intervals since
    // 1601-01-01; keep the full precision instead of truncating to seconds.
    let filetime = coord.Timestamp().map_err(map_err)?.UniversalTime;
    let unix_100ns = i128::from(filetime) - i128::from(FILETIME_UNIX_DIFF) * 10_000_000;
    let timestamp = Timestamp::from_nanosecond(unix_100ns * 100)
        .map_err(|e| LocationError::Platform(e.to_string()))?;

    let mut location = Location::from_degrees(pos.Latitude, pos.Longitude, timestamp)?;

    if let Ok(accuracy) = coord.Accuracy() {
        location = location.with_horizontal_accuracy(accuracy);
    }

    // BasicGeoposition.Altitude is always populated, 0.0 included, so its
    // validity comes from AltitudeAccuracy being present.
    if let Ok(reference) = coord.AltitudeAccuracy()
        && let Ok(vertical_accuracy) = reference.Value()
    {
        location = location
            .with_altitude(pos.Altitude)
            .with_vertical_accuracy(vertical_accuracy);
    }

    Ok(location)
}
