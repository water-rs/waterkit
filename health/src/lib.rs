//! Cross-platform health data access.
//!
//! Provides read/write access to health and fitness data.
//! - iOS: `HealthKit`
//! - Android: `Health Connect`
//! - Desktop: persistent local data store

#![warn(missing_docs)]

mod sys;

/// Type of health data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct HealthSample {
    data_type: HealthDataType,
    value: f64,
    unit: String,
    start_date: String,
    end_date: String,
    source: Option<String>,
}

impl HealthSample {
    /// Creates a new health sample.
    #[must_use]
    pub fn new(
        data_type: HealthDataType,
        value: f64,
        unit: impl Into<String>,
        start_date: impl Into<String>,
        end_date: impl Into<String>,
    ) -> Self {
        Self {
            data_type,
            value,
            unit: unit.into(),
            start_date: start_date.into(),
            end_date: end_date.into(),
            source: None,
        }
    }

    /// Sets the source app/device name.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// The data type.
    #[must_use]
    pub const fn data_type(&self) -> HealthDataType {
        self.data_type
    }

    /// Numeric value (interpretation depends on data type).
    #[must_use]
    pub const fn value(&self) -> f64 {
        self.value
    }

    /// Unit string (e.g., "count", "bpm", "kcal", "m", "kg").
    #[must_use]
    pub fn unit(&self) -> &str {
        &self.unit
    }

    /// Start date (ISO 8601).
    #[must_use]
    pub fn start_date(&self) -> &str {
        &self.start_date
    }

    /// End date (ISO 8601).
    #[must_use]
    pub fn end_date(&self) -> &str {
        &self.end_date
    }

    /// Source app/device name.
    #[must_use]
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
}

/// Check if health data is available on this device.
#[must_use]
#[allow(clippy::missing_const_for_fn)]
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
#[non_exhaustive]
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
