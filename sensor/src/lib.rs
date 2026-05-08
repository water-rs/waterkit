//! Cross-platform sensor access.
//!
//! Provides access to device sensors (accelerometer, gyroscope,
//! magnetometer, barometer, ambient light) across iOS, macOS, Android,
//! Windows, and Linux.
//!
//! # Usage
//!
//! ```ignore
//! use waterkit_sensor::Accelerometer;
//! use futures::StreamExt;
//!
//! if Accelerometer::capabilities().available {
//!     let data = Accelerometer::read().await?;
//!     println!("x={}, y={}, z={}", data.x(), data.y(), data.z());
//!
//!     let mut stream = Accelerometer::watch(100)?;
//!     while let Some(data) = stream.next().await {
//!         println!("x={}, y={}, z={}", data.x(), data.y(), data.z());
//!     }
//! }
//! ```

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod sys;

/// Android-specific JNI helpers that require an explicit `Context`.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::android::{
        is_sensor_available_with_context, read_light_with_context, read_pressure_with_context,
        read_sensor_with_context,
    };
}

use futures_core::Stream;
use waterkit_core::{Capabilities, Timestamp};

/// 3-axis sensor data (accelerometer, gyroscope, magnetometer).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SensorData {
    x: f64,
    y: f64,
    z: f64,
    timestamp: Timestamp,
}

impl SensorData {
    /// Creates a new `SensorData` instance.
    #[must_use]
    pub(crate) const fn new(x: f64, y: f64, z: f64, timestamp: Timestamp) -> Self {
        Self { x, y, z, timestamp }
    }

    /// X-axis value.
    #[must_use]
    pub const fn x(&self) -> f64 {
        self.x
    }

    /// Y-axis value.
    #[must_use]
    pub const fn y(&self) -> f64 {
        self.y
    }

    /// Z-axis value.
    #[must_use]
    pub const fn z(&self) -> f64 {
        self.z
    }

    /// Sample timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }
}

/// Single-value sensor data (barometer, ambient light).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ScalarData {
    value: f64,
    timestamp: Timestamp,
}

impl ScalarData {
    /// Creates a new `ScalarData` instance.
    #[must_use]
    pub(crate) const fn new(value: f64, timestamp: Timestamp) -> Self {
        Self { value, timestamp }
    }

    /// Sensor value.
    #[must_use]
    pub const fn value(&self) -> f64 {
        self.value
    }

    /// Sample timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }
}

/// Capability probe for a single sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SensorCapabilities {
    /// Whether the sensor is available on this device.
    pub available: bool,
}

impl Capabilities for SensorCapabilities {
    fn available(&self) -> bool {
        self.available
    }
}

/// Errors that can occur when accessing sensors.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum SensorError {
    /// Sensor is not available on this device.
    #[error("sensor not available")]
    NotAvailable,
    /// Sensor access permission denied.
    #[error("sensor permission denied")]
    PermissionDenied,
    /// Sensor read timed out.
    #[error("sensor read timed out")]
    Timeout,
    /// Platform-level failure with a message.
    #[error("platform error: {0}")]
    Platform(String),
}

/// Accelerometer sensor (measures linear acceleration in g).
#[derive(Debug)]
pub struct Accelerometer;

impl Accelerometer {
    /// Probes whether the accelerometer is available.
    #[must_use]
    pub fn capabilities() -> SensorCapabilities {
        SensorCapabilities {
            available: sys::accelerometer_available(),
        }
    }

    /// Reads the current sensor data.
    ///
    /// # Errors
    ///
    /// Returns [`SensorError`] when the sensor cannot be read.
    pub async fn read() -> Result<SensorData, SensorError> {
        sys::accelerometer_read().await
    }

    /// Subscribes to sensor data updates at the given interval.
    ///
    /// # Errors
    ///
    /// Returns [`SensorError`] when the sensor cannot be subscribed to.
    pub fn watch(
        interval_ms: u32,
    ) -> Result<impl Stream<Item = SensorData> + Send + 'static, SensorError> {
        sys::accelerometer_watch(interval_ms)
    }
}

/// Gyroscope sensor.
#[derive(Debug)]
pub struct Gyroscope;

impl Gyroscope {
    /// Probes whether the gyroscope is available.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn capabilities() -> SensorCapabilities {
        SensorCapabilities {
            available: sys::gyroscope_available(),
        }
    }

    /// Reads the current sensor data.
    ///
    /// # Errors
    ///
    /// Returns [`SensorError`] when the sensor cannot be read.
    pub async fn read() -> Result<SensorData, SensorError> {
        sys::gyroscope_read().await
    }

    /// Subscribes to sensor data updates at the given interval.
    ///
    /// # Errors
    ///
    /// Returns [`SensorError`] when the sensor cannot be subscribed to.
    pub fn watch(
        interval_ms: u32,
    ) -> Result<impl Stream<Item = SensorData> + Send + 'static, SensorError> {
        sys::gyroscope_watch(interval_ms)
    }
}

/// Magnetometer sensor.
#[derive(Debug)]
pub struct Magnetometer;

impl Magnetometer {
    /// Probes whether the magnetometer is available.
    #[must_use]
    pub fn capabilities() -> SensorCapabilities {
        SensorCapabilities {
            available: sys::magnetometer_available(),
        }
    }

    /// Reads the current sensor data.
    ///
    /// # Errors
    ///
    /// Returns [`SensorError`] when the sensor cannot be read.
    pub async fn read() -> Result<SensorData, SensorError> {
        sys::magnetometer_read().await
    }

    /// Subscribes to sensor data updates at the given interval.
    ///
    /// # Errors
    ///
    /// Returns [`SensorError`] when the sensor cannot be subscribed to.
    pub fn watch(
        interval_ms: u32,
    ) -> Result<impl Stream<Item = SensorData> + Send + 'static, SensorError> {
        sys::magnetometer_watch(interval_ms)
    }
}

/// Barometer sensor (atmospheric pressure).
#[derive(Debug)]
pub struct Barometer;

impl Barometer {
    /// Probes whether the barometer is available.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn capabilities() -> SensorCapabilities {
        SensorCapabilities {
            available: sys::barometer_available(),
        }
    }

    /// Reads the current sensor data.
    ///
    /// # Errors
    ///
    /// Returns [`SensorError`] when the sensor cannot be read.
    pub async fn read() -> Result<ScalarData, SensorError> {
        sys::barometer_read().await
    }

    /// Subscribes to sensor data updates at the given interval.
    ///
    /// # Errors
    ///
    /// Returns [`SensorError`] when the sensor cannot be subscribed to.
    pub fn watch(
        interval_ms: u32,
    ) -> Result<impl Stream<Item = ScalarData> + Send + 'static, SensorError> {
        sys::barometer_watch(interval_ms)
    }
}

/// Ambient light sensor.
///
/// Available on macOS (`MacBooks`) and some mobile devices.
#[derive(Debug)]
pub struct AmbientLight;

impl AmbientLight {
    /// Probes whether the ambient light sensor is available.
    #[must_use]
    pub fn capabilities() -> SensorCapabilities {
        SensorCapabilities {
            available: sys::ambient_light_available(),
        }
    }

    /// Reads the current sensor data.
    ///
    /// # Errors
    ///
    /// Returns [`SensorError`] when the sensor cannot be read.
    pub async fn read() -> Result<ScalarData, SensorError> {
        sys::ambient_light_read().await
    }

    /// Subscribes to sensor data updates at the given interval.
    ///
    /// # Errors
    ///
    /// Returns [`SensorError`] when the sensor cannot be subscribed to.
    pub fn watch(
        interval_ms: u32,
    ) -> Result<impl Stream<Item = ScalarData> + Send + 'static, SensorError> {
        sys::ambient_light_watch(interval_ms)
    }
}
