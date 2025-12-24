//! Apple platform (iOS/macOS) sensor implementation using swift-bridge.

use crate::{ScalarData, SensorData, SensorError, SensorStream};
use futures::stream;

#[swift_bridge::bridge]
mod ffi {
    #[swift_bridge(swift_repr = "struct")]
    struct SensorReading {
        x: f64,
        y: f64,
        z: f64,
        timestamp_ms: u64,
    }

    #[swift_bridge(swift_repr = "struct")]
    struct ScalarReading {
        value: f64,
        timestamp_ms: u64,
    }

    enum SensorResult {
        Success(SensorReading),
        NotAvailable,
        PermissionDenied,
        Timeout,
    }

    enum ScalarResult {
        Success(ScalarReading),
        NotAvailable,
        PermissionDenied,
        Timeout,
    }

    extern "Swift" {
        fn is_accelerometer_available() -> bool;
        fn read_accelerometer() -> SensorResult;

        fn is_gyroscope_available() -> bool;
        fn read_gyroscope() -> SensorResult;

        fn is_magnetometer_available() -> bool;
        fn read_magnetometer() -> SensorResult;

        fn is_barometer_available() -> bool;
        fn read_barometer() -> ScalarResult;

        fn is_ambient_light_available() -> bool;
        fn read_ambient_light() -> ScalarResult;
    }
}

fn convert_reading(reading: ffi::SensorReading) -> SensorData {
    SensorData {
        x: reading.x,
        y: reading.y,
        z: reading.z,
        timestamp: reading.timestamp_ms,
    }
}

fn convert_scalar(reading: ffi::ScalarReading) -> ScalarData {
    ScalarData {
        value: reading.value,
        timestamp: reading.timestamp_ms,
    }
}

fn convert_result(result: ffi::SensorResult) -> Result<SensorData, SensorError> {
    match result {
        ffi::SensorResult::Success(r) => Ok(convert_reading(r)),
        ffi::SensorResult::NotAvailable => Err(SensorError::NotAvailable),
        ffi::SensorResult::PermissionDenied => Err(SensorError::PermissionDenied),
        ffi::SensorResult::Timeout => Err(SensorError::Timeout),
    }
}

fn convert_scalar_result(result: ffi::ScalarResult) -> Result<ScalarData, SensorError> {
    match result {
        ffi::ScalarResult::Success(r) => Ok(convert_scalar(r)),
        ffi::ScalarResult::NotAvailable => Err(SensorError::NotAvailable),
        ffi::ScalarResult::PermissionDenied => Err(SensorError::PermissionDenied),
        ffi::ScalarResult::Timeout => Err(SensorError::Timeout),
    }
}

// Accelerometer
pub fn accelerometer_available() -> bool {
    ffi::is_accelerometer_available()
}

pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
    convert_result(ffi::read_accelerometer())
}

pub fn accelerometer_watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    if !accelerometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        match ffi::read_accelerometer() {
            ffi::SensorResult::Success(r) => Some((convert_reading(r), ())),
            _ => None,
        }
    })))
}

// Gyroscope
pub fn gyroscope_available() -> bool {
    ffi::is_gyroscope_available()
}

pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
    convert_result(ffi::read_gyroscope())
}

pub fn gyroscope_watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    if !gyroscope_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        match ffi::read_gyroscope() {
            ffi::SensorResult::Success(r) => Some((convert_reading(r), ())),
            _ => None,
        }
    })))
}

// Magnetometer
pub fn magnetometer_available() -> bool {
    ffi::is_magnetometer_available()
}

pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
    convert_result(ffi::read_magnetometer())
}

pub fn magnetometer_watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    if !magnetometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        match ffi::read_magnetometer() {
            ffi::SensorResult::Success(r) => Some((convert_reading(r), ())),
            _ => None,
        }
    })))
}

// Barometer
pub fn barometer_available() -> bool {
    ffi::is_barometer_available()
}

pub async fn barometer_read() -> Result<ScalarData, SensorError> {
    convert_scalar_result(ffi::read_barometer())
}

pub fn barometer_watch(interval_ms: u32) -> Result<SensorStream<ScalarData>, SensorError> {
    if !barometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        match ffi::read_barometer() {
            ffi::ScalarResult::Success(r) => Some((convert_scalar(r), ())),
            _ => None,
        }
    })))
}

// Ambient Light
pub fn ambient_light_available() -> bool {
    ffi::is_ambient_light_available()
}

pub async fn ambient_light_read() -> Result<ScalarData, SensorError> {
    convert_scalar_result(ffi::read_ambient_light())
}

pub fn ambient_light_watch(interval_ms: u32) -> Result<SensorStream<ScalarData>, SensorError> {
    if !ambient_light_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        match ffi::read_ambient_light() {
            ffi::ScalarResult::Success(r) => Some((convert_scalar(r), ())),
            _ => None,
        }
    })))
}
