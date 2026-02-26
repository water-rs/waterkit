//! Cross-platform sensor access.
//!
//! This crate provides access to device sensors (accelerometer, gyroscope,
//! magnetometer, barometer) across iOS, macOS, Android, Windows, and Linux.
//!
//! # Usage
//!
//! ```ignore
//! use waterkit_sensor::{Accelerometer, SensorData};
//!
//! // Check if accelerometer is available
//! if Accelerometer::is_available() {
//!     // One-shot reading
//!     let data = Accelerometer::read().await?;
//!     println!("x={}, y={}, z={}", data.x(), data.y(), data.z());
//!
//!     // Or stream updates
//!     use futures::StreamExt;
//!     let mut stream = Accelerometer::watch(100)?; // 100ms interval
//!     while let Some(data) = stream.next().await {
//!         println!("x={}, y={}, z={}", data.x(), data.y(), data.z());
//!     }
//! }
//! ```

#![warn(missing_docs)]

/// Platform-specific implementations.
mod sys;

use futures::Stream;
use std::pin::Pin;

/// 3-axis sensor data (accelerometer, gyroscope, magnetometer).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SensorData {
    x: f64,
    y: f64,
    z: f64,
    timestamp: u64,
}

impl SensorData {
    /// Creates a new `SensorData` instance.
    #[must_use]
    pub(crate) const fn new(x: f64, y: f64, z: f64, timestamp: u64) -> Self {
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

    /// Timestamp as Unix epoch milliseconds.
    #[must_use]
    pub const fn timestamp(&self) -> u64 {
        self.timestamp
    }
}

/// Single-value sensor data (barometer).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ScalarData {
    value: f64,
    timestamp: u64,
}

impl ScalarData {
    /// Creates a new `ScalarData` instance.
    #[must_use]
    pub(crate) const fn new(value: f64, timestamp: u64) -> Self {
        Self { value, timestamp }
    }

    /// Sensor value.
    #[must_use]
    pub const fn value(&self) -> f64 {
        self.value
    }

    /// Timestamp as Unix epoch milliseconds.
    #[must_use]
    pub const fn timestamp(&self) -> u64 {
        self.timestamp
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
    /// An unknown error occurred.
    #[error("unknown error: {0}")]
    Unknown(String),
}

/// A boxed Stream of sensor data.
pub type SensorStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;

/// Accelerometer sensor (measures linear acceleration in g).
#[derive(Debug)]
pub struct Accelerometer;

impl Accelerometer {
    /// Check if the accelerometer is available.
    #[must_use]
    pub fn is_available() -> bool {
        sys::accelerometer_available()
    }

    /// Read the current sensor data.
    ///
    /// # Errors
    /// Returns a [`SensorError`] if the sensor is not available.
    pub async fn read() -> Result<SensorData, SensorError> {
        sys::accelerometer_read().await
    }

    /// Watch for sensor data updates at a specified interval.
    ///
    /// # Errors
    /// Returns a [`SensorError`] if the sensor is not available.
    pub fn watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
        sys::accelerometer_watch(interval_ms)
    }
}

/// Gyroscope sensor.
#[derive(Debug)]
pub struct Gyroscope;

impl Gyroscope {
    /// Check if the gyroscope is available.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn is_available() -> bool {
        sys::gyroscope_available()
    }

    /// Read the current sensor data.
    ///
    /// # Errors
    /// Returns a [`SensorError`] if the sensor is not available.
    pub async fn read() -> Result<SensorData, SensorError> {
        sys::gyroscope_read().await
    }

    /// Watch for sensor data updates at a specified interval.
    ///
    /// # Errors
    /// Returns a [`SensorError`] if the sensor is not available.
    pub fn watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
        sys::gyroscope_watch(interval_ms)
    }
}

/// Magnetometer sensor.
#[derive(Debug)]
pub struct Magnetometer;

impl Magnetometer {
    /// Check if the magnetometer is available.
    #[must_use]
    pub fn is_available() -> bool {
        sys::magnetometer_available()
    }

    /// Read the current sensor data.
    ///
    /// # Errors
    /// Returns a [`SensorError`] if the sensor is not available.
    pub async fn read() -> Result<SensorData, SensorError> {
        sys::magnetometer_read().await
    }

    /// Watch for sensor data updates at a specified interval.
    ///
    /// # Errors
    /// Returns a [`SensorError`] if the sensor is not available.
    pub fn watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
        sys::magnetometer_watch(interval_ms)
    }
}

/// Barometer sensor.
#[derive(Debug)]
pub struct Barometer;

impl Barometer {
    /// Check if the barometer is available.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn is_available() -> bool {
        sys::barometer_available()
    }

    /// Read the current sensor data.
    ///
    /// # Errors
    /// Returns a [`SensorError`] if the sensor is not available.
    pub async fn read() -> Result<ScalarData, SensorError> {
        sys::barometer_read().await
    }

    /// Watch for sensor data updates at a specified interval.
    ///
    /// # Errors
    /// Returns a [`SensorError`] if the sensor is not available.
    pub fn watch(interval_ms: u32) -> Result<SensorStream<ScalarData>, SensorError> {
        sys::barometer_watch(interval_ms)
    }
}

/// Ambient light sensor.
///
/// Available on macOS (`MacBooks`) and some mobile devices.
#[derive(Debug)]
pub struct AmbientLight;

impl AmbientLight {
    /// Check if the ambient light sensor is available.
    #[must_use]
    pub fn is_available() -> bool {
        sys::ambient_light_available()
    }

    /// Read the current sensor data.
    ///
    /// # Errors
    /// Returns a [`SensorError`] if the sensor is not available.
    pub async fn read() -> Result<ScalarData, SensorError> {
        sys::ambient_light_read().await
    }

    /// Watch for sensor data updates at a specified interval.
    ///
    /// # Errors
    /// Returns a [`SensorError`] if the sensor is not available.
    pub fn watch(interval_ms: u32) -> Result<SensorStream<ScalarData>, SensorError> {
        sys::ambient_light_watch(interval_ms)
    }
}
