//! Cross-platform health data access.
//!
//! Provides read/write access to health and fitness data.
//! - iOS: `HealthKit`
//! - Android: `Health Connect`
//! - Desktop: Not supported

#![warn(missing_docs)]

mod sys;

/// Type of health data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HealthDataType {
    /// Step count.
    Steps,
    /// Heart rate (bpm).
    HeartRate,
    /// Active energy burned (kcal).
    ActiveEnergy,
    /// Distance walked/run (meters).
    Distance,
    /// Body weight (kg).
    Weight,
    /// Body height (meters).
    Height,
    /// Blood oxygen saturation (%).
    BloodOxygen,
    /// Sleep analysis.
    Sleep,
}

/// A health data sample.
#[derive(Debug, Clone)]
pub struct HealthSample {
    /// The data type.
    pub data_type: HealthDataType,
    /// Numeric value (interpretation depends on data type).
    pub value: f64,
    /// Unit string (e.g., "count", "bpm", "kcal", "m", "kg").
    pub unit: String,
    /// Start date (ISO 8601).
    pub start_date: String,
    /// End date (ISO 8601).
    pub end_date: String,
    /// Source app/device name.
    pub source: Option<String>,
}

/// Check if health data is available on this device.
#[must_use]
pub fn is_available() -> bool {
    sys::is_available()
}

/// Request authorization to read/write health data types.
///
/// # Errors
/// Returns error if authorization fails.
pub async fn request_authorization(
    read_types: &[HealthDataType],
    write_types: &[HealthDataType],
) -> Result<(), HealthError> {
    sys::request_authorization(read_types, write_types).await
}

/// Query health samples within a date range.
///
/// # Errors
/// Returns error if the query fails.
pub async fn query_samples(
    data_type: HealthDataType,
    start_date: &str,
    end_date: &str,
) -> Result<Vec<HealthSample>, HealthError> {
    sys::query_samples(data_type, start_date, end_date).await
}

/// Write a health sample.
///
/// # Errors
/// Returns error if writing fails.
pub async fn write_sample(sample: HealthSample) -> Result<(), HealthError> {
    sys::write_sample(sample).await
}

/// Errors in health operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum HealthError {
    /// Health data not available.
    #[error("health data not available")]
    NotAvailable,
    /// Permission denied.
    #[error("health permission denied")]
    PermissionDenied,
    /// Not supported on this platform.
    #[error("not supported")]
    NotSupported,
    /// Platform error.
    #[error("platform error: {0}")]
    PlatformError(String),
}
