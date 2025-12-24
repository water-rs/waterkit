//! Platform-specific sensor implementations.

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;

/// Android platform implementation.
#[cfg(target_os = "android")]
pub mod android;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

// Re-export platform implementations
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub(crate) use apple::*;

#[cfg(target_os = "android")]
pub(crate) use android::*;

#[cfg(target_os = "windows")]
pub(crate) use windows::*;

#[cfg(target_os = "linux")]
pub(crate) use linux::*;

// Fallback for unsupported platforms
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
mod fallback {
    use crate::{ScalarData, SensorData, SensorError, SensorStream};

    pub fn accelerometer_available() -> bool {
        false
    }
    pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
        Err(SensorError::NotAvailable)
    }
    pub fn accelerometer_watch(_interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
        Err(SensorError::NotAvailable)
    }

    pub fn gyroscope_available() -> bool {
        false
    }
    pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
        Err(SensorError::NotAvailable)
    }
    pub fn gyroscope_watch(_interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
        Err(SensorError::NotAvailable)
    }

    pub fn magnetometer_available() -> bool {
        false
    }
    pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
        Err(SensorError::NotAvailable)
    }
    pub fn magnetometer_watch(_interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
        Err(SensorError::NotAvailable)
    }

    pub fn barometer_available() -> bool {
        false
    }
    pub async fn barometer_read() -> Result<ScalarData, SensorError> {
        Err(SensorError::NotAvailable)
    }
    pub fn barometer_watch(_interval_ms: u32) -> Result<SensorStream<ScalarData>, SensorError> {
        Err(SensorError::NotAvailable)
    }

    pub fn ambient_light_available() -> bool {
        false
    }
    pub async fn ambient_light_read() -> Result<ScalarData, SensorError> {
        Err(SensorError::NotAvailable)
    }
    pub fn ambient_light_watch(_interval_ms: u32) -> Result<SensorStream<ScalarData>, SensorError> {
        Err(SensorError::NotAvailable)
    }
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub(crate) use fallback::*;
