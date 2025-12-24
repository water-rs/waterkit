//! Windows sensor implementation using WinRT.

use crate::{ScalarData, SensorData, SensorError, SensorStream};
use futures::stream;
use windows::Devices::Sensors::{
    Accelerometer as WinAccelerometer, Barometer as WinBarometer, Gyrometer as WinGyrometer,
    Magnetometer as WinMagnetometer,
};

fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// Accelerometer
pub fn accelerometer_available() -> bool {
    WinAccelerometer::GetDefault().is_ok()
}

pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
    let sensor = WinAccelerometer::GetDefault().map_err(|_| SensorError::NotAvailable)?;

    let reading = sensor
        .GetCurrentReading()
        .map_err(|e| SensorError::Unknown(e.to_string()))?;

    Ok(SensorData {
        x: reading.AccelerationX().unwrap_or(0.0),
        y: reading.AccelerationY().unwrap_or(0.0),
        z: reading.AccelerationZ().unwrap_or(0.0),
        timestamp: timestamp_now(),
    })
}

pub fn accelerometer_watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    if !accelerometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        accelerometer_read().await.ok().map(|data| (data, ()))
    })))
}

// Gyroscope
pub fn gyroscope_available() -> bool {
    WinGyrometer::GetDefault().is_ok()
}

pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
    let sensor = WinGyrometer::GetDefault().map_err(|_| SensorError::NotAvailable)?;

    let reading = sensor
        .GetCurrentReading()
        .map_err(|e| SensorError::Unknown(e.to_string()))?;

    Ok(SensorData {
        x: reading.AngularVelocityX().unwrap_or(0.0),
        y: reading.AngularVelocityY().unwrap_or(0.0),
        z: reading.AngularVelocityZ().unwrap_or(0.0),
        timestamp: timestamp_now(),
    })
}

pub fn gyroscope_watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    if !gyroscope_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        gyroscope_read().await.ok().map(|data| (data, ()))
    })))
}

// Magnetometer
pub fn magnetometer_available() -> bool {
    WinMagnetometer::GetDefault().is_ok()
}

pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
    let sensor = WinMagnetometer::GetDefault().map_err(|_| SensorError::NotAvailable)?;

    let reading = sensor
        .GetCurrentReading()
        .map_err(|e| SensorError::Unknown(e.to_string()))?;

    Ok(SensorData {
        x: reading.MagneticFieldX().unwrap_or(0.0),
        y: reading.MagneticFieldY().unwrap_or(0.0),
        z: reading.MagneticFieldZ().unwrap_or(0.0),
        timestamp: timestamp_now(),
    })
}

pub fn magnetometer_watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    if !magnetometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        magnetometer_read().await.ok().map(|data| (data, ()))
    })))
}

// Barometer
pub fn barometer_available() -> bool {
    WinBarometer::GetDefault().is_ok()
}

pub async fn barometer_read() -> Result<ScalarData, SensorError> {
    let sensor = WinBarometer::GetDefault().map_err(|_| SensorError::NotAvailable)?;

    let reading = sensor
        .GetCurrentReading()
        .map_err(|e| SensorError::Unknown(e.to_string()))?;

    Ok(ScalarData {
        value: reading.StationPressureInHectopascals().unwrap_or(0.0),
        timestamp: timestamp_now(),
    })
}

pub fn barometer_watch(interval_ms: u32) -> Result<SensorStream<ScalarData>, SensorError> {
    if !barometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        barometer_read().await.ok().map(|data| (data, ()))
    })))
}
