//! Cross-platform location access.
//!
//! This crate provides a unified API for accessing device location across
//! iOS, macOS, Android, Windows, and Linux platforms.
//!
//! # Example
//!
//! ```no_run
//! use waterkit_location::Location;
//!
//! # async fn example() -> Result<(), waterkit_location::LocationError> {
//! let location = Location::get().await?;
//!
//! println!("Latitude: {}", location.latitude());
//! println!("Longitude: {}", location.longitude());
//!
//! if let Some(altitude) = location.altitude() {
//!     println!("Altitude: {} meters", altitude);
//! }
//!
//! println!("Timestamp: {:?}", location.timestamp());
//! # Ok(())
//! # }
//! ```

pub use jiff::Timestamp;

/// Platform-specific implementations.
mod sys;

pub use waterkit_permission::{Permission, PermissionStatus};

/// A geographic location with coordinates and metadata.
///
/// All fields are private to allow future API evolution without breaking changes.
/// Use the accessor methods to retrieve location data.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    latitude: f64,
    longitude: f64,
    altitude: Option<f64>,
    horizontal_accuracy: Option<f64>,
    vertical_accuracy: Option<f64>,
    timestamp: Timestamp,
}

impl Location {
    /// Creates a new `Location` with the given coordinates.
    ///
    /// # Arguments
    ///
    /// * `latitude` - Latitude in degrees (-90 to 90)
    /// * `longitude` - Longitude in degrees (-180 to 180)
    /// * `timestamp` - When this location was recorded
    #[must_use]
    pub const fn new(latitude: f64, longitude: f64, timestamp: Timestamp) -> Self {
        Self {
            latitude,
            longitude,
            altitude: None,
            horizontal_accuracy: None,
            vertical_accuracy: None,
            timestamp,
        }
    }

    /// Get the current device location.
    ///
    /// This will request location permission if not already granted.
    ///
    /// # Errors
    /// Returns a `LocationError` if:
    /// - Permission is denied.
    /// - Location services are disabled.
    /// - The request times out.
    /// - Location is not available.
    pub async fn get() -> Result<Self, LocationError> {
        // Check/request permission first
        Self::ask_permission().await?;

        sys::get_location().await
    }

    /// Ask for location permission.
    ///
    /// # Errors
    /// Returns a `LocationError` if:
    /// - Permission is denied.
    /// - Location services are disabled.
    /// - The request times out.
    pub async fn ask_permission() -> Result<(), LocationError> {
        let status = waterkit_permission::request(Permission::Location)
            .await
            .map_err(|e| LocationError::Unknown(e.to_string()))?;

        if status != PermissionStatus::Granted {
            return Err(LocationError::PermissionDenied);
        }

        Ok(())
    }

    /// Sets the altitude in meters above sea level.
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
    pub const fn latitude(&self) -> f64 {
        self.latitude
    }

    /// Returns the longitude in degrees (-180 to 180).
    #[must_use]
    pub const fn longitude(&self) -> f64 {
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
    /// An unknown error occurred.
    #[error("unknown error: {0}")]
    Unknown(String),
}
