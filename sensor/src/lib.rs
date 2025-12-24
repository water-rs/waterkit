//! Cross-platform sensor access.
//!
//! This crate provides access to device sensors (accelerometer, gyroscope,
//! magnetometer, barometer) across iOS, macOS, Android, Windows, and Linux.
//!
//! # Usage
//!
//! ```no_run
//! use waterkit_sensor::{Accelerometer, SensorData};
//!
//! // Check if accelerometer is available
//! if Accelerometer::is_available() {
//!     // One-shot reading
//!     let data = Accelerometer::read().await?;
//!     println!("x={}, y={}, z={}", data.x, data.y, data.z);
//!
//!     // Or stream updates
//!     use futures::StreamExt;
//!     let mut stream = Accelerometer::watch(100)?; // 100ms interval
//!     while let Some(data) = stream.next().await {
//!         println!("x={}, y={}, z={}", data.x, data.y, data.z);
//!     }
//! }
//! ```

#![warn(missing_docs)]

/// Platform-specific implementations.
pub mod sys;

use futures::Stream;
use std::pin::Pin;

/// 3-axis sensor data (accelerometer, gyroscope, magnetometer).
#[derive(Debug, Clone, PartialEq)]
pub struct SensorData {
    /// X-axis value.
    pub x: f64,
    /// Y-axis value.
    pub y: f64,
    /// Z-axis value.
    pub z: f64,
    /// Timestamp as Unix epoch milliseconds.
    pub timestamp: u64,
}

/// Single-value sensor data (barometer).
#[derive(Debug, Clone, PartialEq)]
pub struct ScalarData {
    /// Sensor value.
    pub value: f64,
    /// Timestamp as Unix epoch milliseconds.
    pub timestamp: u64,
}

/// Errors that can occur when accessing sensors.
#[derive(Debug, Clone)]
pub enum SensorError {
    /// Sensor is not available on this device.
    NotAvailable,
    /// Sensor access permission denied.
    PermissionDenied,
    /// Sensor read timed out.
    Timeout,
    /// An unknown error occurred.
    Unknown(String),
}

impl std::fmt::Display for SensorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAvailable => write!(f, "sensor not available"),
            Self::PermissionDenied => write!(f, "sensor permission denied"),
            Self::Timeout => write!(f, "sensor read timed out"),
            Self::Unknown(msg) => write!(f, "unknown error: {msg}"),
        }
    }
}

impl std::error::Error for SensorError {}

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
