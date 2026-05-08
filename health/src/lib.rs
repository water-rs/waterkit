//! Cross-platform health data access.
//!
//! Provides read/write access to health and fitness data.
//! - iOS: `HealthKit`
//! - Android: `Health Connect`
//! - Desktop: persistent local data store
//!
//! Permissions are requested via `waterkit_permission::request` with
//! [`waterkit_core::Permission::HealthRead`] /
//! [`waterkit_core::Permission::HealthWrite`]; this crate no longer ships
//! its own `request_authorization`.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod sys;

use waterkit_core::Capabilities;
use waterkit_core::Timestamp;

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
    start: Timestamp,
    end: Timestamp,
    source: Option<String>,
}

impl HealthSample {
    /// Creates a new health sample.
    #[must_use]
    pub fn new(
        data_type: HealthDataType,
        value: f64,
        unit: impl Into<String>,
        start: Timestamp,
        end: Timestamp,
    ) -> Self {
        Self {
            data_type,
            value,
            unit: unit.into(),
            start,
            end,
            source: None,
        }
    }

    /// Sets the source app/device name.
    #[must_use]
    pub fn source(mut self, source: impl Into<String>) -> Self {
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

    /// Sample start instant.
    #[must_use]
    pub const fn start(&self) -> Timestamp {
        self.start
    }

    /// Sample end instant.
    #[must_use]
    pub const fn end(&self) -> Timestamp {
        self.end
    }

    /// Source app/device name.
    #[must_use]
    pub fn source_name(&self) -> Option<&str> {
        self.source.as_deref()
    }
}

/// Capability probe result for the health subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct HealthCapabilities {
    /// Whether health data is accessible at runtime.
    pub available: bool,
}

impl Capabilities for HealthCapabilities {
    fn available(&self) -> bool {
        self.available
    }
}

/// Probes the health subsystem.
#[must_use]
pub fn capabilities() -> HealthCapabilities {
    HealthCapabilities {
        available: sys::is_available(),
    }
}

/// Queries health samples within a date range.
///
/// # Errors
///
/// Returns [`HealthError`] when the query cannot be executed.
pub async fn query_samples(
    data_type: HealthDataType,
    start: Timestamp,
    end: Timestamp,
) -> Result<Vec<HealthSample>, HealthError> {
    sys::query_samples(data_type, start, end).await
}

/// Writes a health sample.
///
/// # Errors
///
/// Returns [`HealthError`] when the write cannot be performed.
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
    Unsupported,
    /// Platform error.
    #[error("platform error: {0}")]
    Platform(String),
}
