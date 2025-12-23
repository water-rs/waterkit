//! Cross-platform location access.
//!
//! This crate provides a unified API for accessing device location across
//! iOS, macOS, Android, Windows, and Linux platforms.

#![warn(missing_docs)]

/// Platform-specific implementations.
pub mod sys;

pub use waterkit_permission::{Permission, PermissionStatus};

/// A geographic location with coordinates and metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    /// Latitude in degrees (-90 to 90).
    pub latitude: f64,
    /// Longitude in degrees (-180 to 180).
    pub longitude: f64,
    /// Altitude in meters above sea level, if available.
    pub altitude: Option<f64>,
    /// Horizontal accuracy in meters, if available.
    pub horizontal_accuracy: Option<f64>,
    /// Vertical accuracy in meters, if available.
    pub vertical_accuracy: Option<f64>,
    /// Timestamp as Unix epoch milliseconds.
    pub timestamp: u64,
}

/// Errors that can occur when accessing location.
#[derive(Debug, Clone)]
pub enum LocationError {
    /// Location permission was not granted.
    PermissionDenied,
    /// Location services are disabled on the device.
    ServiceDisabled,
    /// Location request timed out.
    Timeout,
    /// Location is not available.
    NotAvailable,
    /// An unknown error occurred.
    Unknown(String),
}

impl std::fmt::Display for LocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(f, "location permission denied"),
            Self::ServiceDisabled => write!(f, "location services disabled"),
            Self::Timeout => write!(f, "location request timed out"),
            Self::NotAvailable => write!(f, "location not available"),
            Self::Unknown(msg) => write!(f, "unknown error: {msg}"),
        }
    }
}

impl std::error::Error for LocationError {}

/// Manager for accessing device location.
#[derive(Debug)]
pub struct LocationManager;

impl LocationManager {
    /// Get the current device location.
    ///
    /// This will request location permission if not already granted.
    pub async fn get_location() -> Result<Location, LocationError> {
        // Check/request permission first
        let status = waterkit_permission::request(Permission::Location)
            .await
            .map_err(|e| LocationError::Unknown(e.to_string()))?;

        if status != PermissionStatus::Granted {
            return Err(LocationError::PermissionDenied);
        }

        sys::get_location().await
    }

    /// Get the current location without checking permissions.
    ///
    /// Use this if you've already verified permission status.
    pub async fn get_location_unchecked() -> Result<Location, LocationError> {
        sys::get_location().await
    }
}
