//! Cross-platform location access.
//!
//! Provides a unified async API for the device's geographic location.
//! Callers must arrange for [`Permission::Location`] to be granted before
//! calling [`Location::get`]; the recommended flow is
//! `waterkit_permission::request(Permission::Location).await` then
//! `Location::get().await`.
//!
//! # Example
//!
//! ```no_run
//! use waterkit_location::Location;
//! use waterkit_permission::{request, Permission};
//!
//! # async fn example() -> Result<(), waterkit_location::LocationError> {
//! let _ = request(Permission::Location).await;
//! let location = Location::get().await?;
//! let _latitude = location.latitude().get();
//! let _longitude = location.longitude().get();
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

pub use jiff::Timestamp;
pub use waterkit_core::{Latitude, Longitude, OutOfRange};

mod sys;

pub use waterkit_permission::{Permission, PermissionStatus};

/// Android-specific JNI helpers that require a `JNIEnv` and `Context`/`Activity`.
///
/// These are intentionally separate from the async public API because Android
/// permission/location flows require an app-owned JNI context.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::android::get_location_with_context;
}

/// A geographic location with coordinates and metadata.
///
/// All fields are private to allow future API evolution without breaking changes.
/// Use the accessor methods to retrieve location data.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    latitude: Latitude,
    longitude: Longitude,
    altitude: Option<f64>,
    horizontal_accuracy: Option<f64>,
    vertical_accuracy: Option<f64>,
    timestamp: Timestamp,
}

impl Location {
    /// Creates a new `Location` with validated coordinates.
    ///
    /// # Arguments
    ///
    /// * `latitude` - Latitude in degrees (-90 to 90).
    /// * `longitude` - Longitude in degrees (-180 to 180).
    /// * `timestamp` - When this location was recorded
    #[must_use]
    pub const fn new(latitude: Latitude, longitude: Longitude, timestamp: Timestamp) -> Self {
        Self {
            latitude,
            longitude,
            altitude: None,
            horizontal_accuracy: None,
            vertical_accuracy: None,
            timestamp,
        }
    }

    /// Creates a new `Location` from raw coordinate degrees.
    ///
    /// # Errors
    ///
    /// Returns [`LocationError::InvalidCoordinate`] if either coordinate is
    /// `NaN` or outside its valid range.
    pub fn from_degrees(
        latitude: f64,
        longitude: f64,
        timestamp: Timestamp,
    ) -> Result<Self, LocationError> {
        Ok(Self::new(
            Latitude::new(latitude)?,
            Longitude::new(longitude)?,
            timestamp,
        ))
    }

    /// Returns the current device location.
    ///
    /// **Precondition**: callers must ensure [`Permission::Location`] is
    /// granted; this function does not trigger the runtime prompt.
    /// Use `waterkit_permission::request(Permission::Location)` first.
    ///
    /// # Errors
    ///
    /// Returns [`LocationError::PermissionDenied`] when access is denied,
    /// [`LocationError::ServiceDisabled`] when location services are off,
    /// [`LocationError::Timeout`] when the request times out,
    /// [`LocationError::InvalidCoordinate`] when the OS returns invalid
    /// coordinates, or [`LocationError::Platform`] for other OS failures.
    pub async fn get() -> Result<Self, LocationError> {
        sys::get_location().await
    }

    /// Sets the altitude in meters above sea level.
    ///
    /// `with_*` prefix is preserved here because [`Location`] also has
    /// getters with bare field names (`altitude()` returns the current
    /// value); the workspace-wide naming rule is "bare method name =
    /// setter unless a getter already takes that name."
    #[must_use]
    pub const fn with_altitude(mut self, altitude: f64) -> Self {
        self.altitude = Some(altitude);
        self
    }

    /// Sets the horizontal accuracy in meters.
    #[must_use]
    pub const fn with_horizontal_accuracy(mut self, accuracy: f64) -> Self {
        self.horizontal_accuracy = Some(accuracy);
        self
    }

    /// Sets the vertical accuracy in meters.
    #[must_use]
    pub const fn with_vertical_accuracy(mut self, accuracy: f64) -> Self {
        self.vertical_accuracy = Some(accuracy);
        self
    }

    /// Returns the latitude in degrees (-90 to 90).
    #[must_use]
    pub const fn latitude(&self) -> Latitude {
        self.latitude
    }

    /// Returns the longitude in degrees (-180 to 180).
    #[must_use]
    pub const fn longitude(&self) -> Longitude {
        self.longitude
    }

    /// Returns the altitude in meters above sea level, if available.
    #[must_use]
    pub const fn altitude(&self) -> Option<f64> {
        self.altitude
    }

    /// Returns the horizontal accuracy in meters, if available.
    ///
    /// Lower values indicate more precise location data.
    #[must_use]
    pub const fn horizontal_accuracy(&self) -> Option<f64> {
        self.horizontal_accuracy
    }

    /// Returns the vertical accuracy in meters, if available.
    ///
    /// Lower values indicate more precise altitude data.
    #[must_use]
    pub const fn vertical_accuracy(&self) -> Option<f64> {
        self.vertical_accuracy
    }

    /// Returns the timestamp when this location was recorded.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }
}

/// Errors that can occur when accessing location.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum LocationError {
    /// Location permission was not granted.
    #[error("location permission denied")]
    PermissionDenied,
    /// Location services are disabled on the device.
    #[error("location services disabled")]
    ServiceDisabled,
    /// Location request timed out.
    #[error("location request timed out")]
    Timeout,
    /// Location is not available.
    #[error("location not available")]
    NotAvailable,
    /// Location coordinates were outside their valid geographic ranges.
    #[error("invalid location coordinate: {0}")]
    InvalidCoordinate(#[from] OutOfRange),
    /// Platform-level failure with a message.
    #[error("platform error: {0}")]
    Platform(String),
}

#[cfg(test)]
mod tests {
    use super::{Latitude, Location, LocationError, Longitude, Timestamp};

    #[test]
    fn new_stores_validated_coordinates() {
        let location = Location::new(
            Latitude::new(35.0).expect("valid latitude"),
            Longitude::new(139.0).expect("valid longitude"),
            Timestamp::from_second(0).expect("valid timestamp"),
        );

        assert!((location.latitude().get() - 35.0).abs() < f64::EPSILON);
        assert!((location.longitude().get() - 139.0).abs() < f64::EPSILON);
    }

    #[test]
    fn from_degrees_rejects_invalid_coordinates() {
        let err = Location::from_degrees(
            91.0,
            139.0,
            Timestamp::from_second(0).expect("valid timestamp"),
        )
        .expect_err("latitude should be rejected");

        assert!(matches!(err, LocationError::InvalidCoordinate(_)));
    }
}
