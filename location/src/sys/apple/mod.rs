//! Apple platform (iOS/macOS) location implementation using swift-bridge.

use crate::{Location, LocationError};

#[swift_bridge::bridge]
mod ffi {
    // Shared struct for location data
    #[swift_bridge(swift_repr = "struct")]
    struct LocationData {
        latitude: f64,
        longitude: f64,
        altitude: f64,
        horizontal_accuracy: f64,
        vertical_accuracy: f64,
        timestamp_ms: u64,
    }

    // Result type for location requests
    enum LocationResult {
        Success(LocationData),
        PermissionDenied,
        ServiceDisabled,
        Timeout,
        NotAvailable,
    }

    extern "Swift" {
        fn get_current_location() -> LocationResult;
    }
}

/// Get the current location on Apple platforms.
///
/// # Errors
/// Returns a `LocationError` if the location cannot be retrieved.
pub async fn get_location() -> Result<Location, LocationError> {
    match ffi::get_current_location() {
        ffi::LocationResult::Success(data) => Ok(Location {
            latitude: data.latitude,
            longitude: data.longitude,
            altitude: if data.altitude.is_nan() {
                None
            } else {
                Some(data.altitude)
            },
            horizontal_accuracy: if data.horizontal_accuracy < 0.0 {
                None
            } else {
                Some(data.horizontal_accuracy)
            },
            vertical_accuracy: if data.vertical_accuracy < 0.0 {
                None
            } else {
                Some(data.vertical_accuracy)
            },
            timestamp: data.timestamp_ms,
        }),
        ffi::LocationResult::PermissionDenied => Err(LocationError::PermissionDenied),
        ffi::LocationResult::ServiceDisabled => Err(LocationError::ServiceDisabled),
        ffi::LocationResult::Timeout => Err(LocationError::Timeout),
        ffi::LocationResult::NotAvailable => Err(LocationError::NotAvailable),
    }
}
