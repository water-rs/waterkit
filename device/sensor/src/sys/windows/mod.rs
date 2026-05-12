//! Windows sensor implementation using `WinRT`.
//!
//! The `*_read()` functions are async to match the cross-platform interface,
//! even though `WinRT` sensor reads are synchronous.

use crate::sys::SensorStream;
use crate::{ScalarData, SensorData, SensorError};
use futures::stream;
use waterkit_core::Timestamp;
use windows::Devices::Sensors::{
    Accelerometer as WinAccelerometer, Barometer as WinBarometer, Gyrometer as WinGyrometer,
    LightSensor as WinLightSensor, Magnetometer as WinMagnetometer,
};

fn timestamp_now() -> Timestamp {
    Timestamp::now()
}

// Accelerometer
pub fn accelerometer_available() -> bool {
    WinAccelerometer::GetDefault().is_ok()
}

#[allow(clippy::unused_async)]
pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
    let sensor = WinAccelerometer::GetDefault().map_err(|_| SensorError::NotAvailable)?;

    let reading = sensor
        .GetCurrentReading()
        .map_err(|e| SensorError::Platform(e.to_string()))?;

    Ok(SensorData::new(
        reading.AccelerationX().unwrap_or(0.0),
        reading.AccelerationY().unwrap_or(0.0),
        reading.AccelerationZ().unwrap_or(0.0),
        timestamp_now(),
    ))
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

#[allow(clippy::unused_async)]
pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
    let sensor = WinGyrometer::GetDefault().map_err(|_| SensorError::NotAvailable)?;

    let reading = sensor
        .GetCurrentReading()
        .map_err(|e| SensorError::Platform(e.to_string()))?;

    Ok(SensorData::new(
        reading.AngularVelocityX().unwrap_or(0.0),
        reading.AngularVelocityY().unwrap_or(0.0),
        reading.AngularVelocityZ().unwrap_or(0.0),
        timestamp_now(),
    ))
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

#[allow(clippy::unused_async)]
pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
    let sensor = WinMagnetometer::GetDefault().map_err(|_| SensorError::NotAvailable)?;

    let reading = sensor
        .GetCurrentReading()
        .map_err(|e| SensorError::Platform(e.to_string()))?;

    Ok(SensorData::new(
        f64::from(reading.MagneticFieldX().unwrap_or(0.0)),
        f64::from(reading.MagneticFieldY().unwrap_or(0.0)),
        f64::from(reading.MagneticFieldZ().unwrap_or(0.0)),
        timestamp_now(),
    ))
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

#[allow(clippy::unused_async)]
pub async fn barometer_read() -> Result<ScalarData, SensorError> {
    let sensor = WinBarometer::GetDefault().map_err(|_| SensorError::NotAvailable)?;

    let reading = sensor
        .GetCurrentReading()
        .map_err(|e| SensorError::Platform(e.to_string()))?;

    Ok(ScalarData::new(
        reading.StationPressureInHectopascals().unwrap_or(0.0),
        timestamp_now(),
    ))
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

// Ambient Light
pub fn ambient_light_available() -> bool {
    WinLightSensor::GetDefault().is_ok()
}

#[allow(clippy::unused_async)]
pub async fn ambient_light_read() -> Result<ScalarData, SensorError> {
    let sensor = WinLightSensor::GetDefault().map_err(|_| SensorError::NotAvailable)?;

    let reading = sensor
        .GetCurrentReading()
        .map_err(|e| SensorError::Platform(e.to_string()))?;

    Ok(ScalarData::new(
        f64::from(reading.IlluminanceInLux().unwrap_or(0.0)),
        timestamp_now(),
    ))
}

pub fn ambient_light_watch(interval_ms: u32) -> Result<SensorStream<ScalarData>, SensorError> {
    if !ambient_light_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        ambient_light_read().await.ok().map(|data| (data, ()))
    })))
}
