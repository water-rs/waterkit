//! Android sensor implementation using JNI.

use crate::{ScalarData, SensorData, SensorError};
use futures::stream;
use jni::objects::{JDoubleArray, JObject, JValue};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{AndroidError, DexHelper, dex_helper, with_android_context};
use waterkit_core::Timestamp;

/// `waterkit.sensor.SensorHelper`, embedded as a DEX by this crate's build script and
/// loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.sensor.SensorHelper");

impl From<AndroidError> for SensorError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

/// Reads the `[D` payload the helper returns, rejecting the "unavailable"
/// marker in slot 0.
fn read_result_values(
    env: &Env<'_>,
    result: JObject<'_>,
    minimum_len: usize,
) -> Result<Vec<f64>, SensorError> {
    let array = env
        .cast_local::<JDoubleArray>(result)
        .map_err(|e| SensorError::Platform(format!("sensor result is not a double array: {e}")))?;
    let len = array
        .len(env)
        .map_err(|e| SensorError::Platform(format!("sensor result length: {e}")))?;

    if len < 1 {
        return Err(SensorError::NotAvailable);
    }

    let mut values = vec![0.0f64; len];
    array
        .get_region(env, 0, &mut values)
        .map_err(|e| SensorError::Platform(format!("sensor result read: {e}")))?;

    if values[0] < 0.5 {
        return Err(SensorError::NotAvailable);
    }

    if len < minimum_len {
        return Err(SensorError::Platform("Invalid result array".into()));
    }

    Ok(values)
}

fn parse_sensor_result(env: &Env<'_>, result: JObject<'_>) -> Result<SensorData, SensorError> {
    let values = read_result_values(env, result, 5)?;
    Ok(SensorData::new(
        values[1],
        values[2],
        values[3],
        timestamp_from_jni_double(values[4])?,
    ))
}

fn parse_scalar_result(env: &Env<'_>, result: JObject<'_>) -> Result<ScalarData, SensorError> {
    let values = read_result_values(env, result, 3)?;
    Ok(ScalarData::new(
        values[1],
        timestamp_from_jni_double(values[2])?,
    ))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Android sensor helper returns epoch milliseconds as a non-negative finite double"
)]
fn timestamp_from_jni_double(value: f64) -> Result<Timestamp, SensorError> {
    if !value.is_finite() || value < 0.0 {
        return Err(SensorError::Platform(format!(
            "invalid Android sensor timestamp: {value}"
        )));
    }
    Timestamp::from_millisecond(value as i64)
        .map_err(|e| SensorError::Platform(format!("Android sensor timestamp out of range: {e}")))
}

/// Check sensor availability with an explicit Android `Context`.
///
/// # Errors
/// Returns [`SensorError`] when DEX initialization, helper loading, or the JNI call fails.
pub fn is_sensor_available_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    sensor_type: i32,
) -> Result<bool, SensorError> {
    let helper = HELPER.class(env, context)?;

    env.call_static_method(
        helper,
        jni_str!("isSensorAvailable"),
        jni_sig!("(Landroid/content/Context;I)Z"),
        &[JValue::Object(context), JValue::Int(sensor_type)],
    )
    .map_err(|e| SensorError::Platform(format!("isSensorAvailable: {e}")))?
    .z()
    .map_err(|e| SensorError::Platform(format!("isSensorAvailable result: {e}")))
}

/// Read a sensor with an explicit Android `Context`.
///
/// # Errors
/// Returns [`SensorError`] when DEX initialization, helper loading, JNI access, or payload
/// decoding fails.
pub fn read_sensor_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    sensor_type: i32,
) -> Result<SensorData, SensorError> {
    let helper = HELPER.class(env, context)?;

    let result = env
        .call_static_method(
            helper,
            jni_str!("readSensor"),
            jni_sig!("(Landroid/content/Context;I)[D"),
            &[JValue::Object(context), JValue::Int(sensor_type)],
        )
        .map_err(|e| SensorError::Platform(format!("readSensor: {e}")))?
        .l()
        .map_err(|e| SensorError::Platform(format!("readSensor result: {e}")))?;

    parse_sensor_result(env, result)
}

/// Read pressure data with an explicit Android `Context`.
///
/// # Errors
/// Returns [`SensorError`] when DEX initialization, helper loading, JNI access, or payload
/// decoding fails.
pub fn read_pressure_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<ScalarData, SensorError> {
    let helper = HELPER.class(env, context)?;

    let result = env
        .call_static_method(
            helper,
            jni_str!("readPressure"),
            jni_sig!("(Landroid/content/Context;)[D"),
            &[JValue::Object(context)],
        )
        .map_err(|e| SensorError::Platform(format!("readPressure: {e}")))?
        .l()
        .map_err(|e| SensorError::Platform(format!("readPressure result: {e}")))?;

    parse_scalar_result(env, result)
}

/// Read ambient light data with an explicit Android `Context`.
///
/// # Errors
/// Returns [`SensorError`] when DEX initialization, helper loading, JNI access, or payload
/// decoding fails.
pub fn read_light_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<ScalarData, SensorError> {
    let helper = HELPER.class(env, context)?;

    let result = env
        .call_static_method(
            helper,
            jni_str!("readLight"),
            jni_sig!("(Landroid/content/Context;)[D"),
            &[JValue::Object(context)],
        )
        .map_err(|e| SensorError::Platform(format!("readLight: {e}")))?
        .l()
        .map_err(|e| SensorError::Platform(format!("readLight result: {e}")))?;

    parse_scalar_result(env, result)
}

// --- Parameter-less API Implementation using ndk-context ---

fn is_sensor_available_internal(sensor_type: i32) -> bool {
    with_android_context(|env, context| is_sensor_available_with_context(env, context, sensor_type))
        .unwrap_or(false)
}

fn read_sensor_internal(sensor_type: i32) -> Result<SensorData, SensorError> {
    with_android_context(|env, context| read_sensor_with_context(env, context, sensor_type))
}

fn read_pressure_internal() -> Result<ScalarData, SensorError> {
    with_android_context(read_pressure_with_context)
}

fn read_light_internal() -> Result<ScalarData, SensorError> {
    with_android_context(read_light_with_context)
}

pub fn accelerometer_available() -> bool {
    is_sensor_available_internal(1)
}

#[allow(clippy::unused_async)]
pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
    read_sensor_internal(1)
}

pub fn accelerometer_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = SensorData> + Send, SensorError> {
    if !accelerometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        (accelerometer_read().await).map_or(None, |data| Some((data, ())))
    }))
}

pub fn gyroscope_available() -> bool {
    is_sensor_available_internal(4)
}

#[allow(clippy::unused_async)]
pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
    read_sensor_internal(4)
}

pub fn gyroscope_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = SensorData> + Send, SensorError> {
    if !gyroscope_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        (gyroscope_read().await).map_or(None, |data| Some((data, ())))
    }))
}

pub fn magnetometer_available() -> bool {
    is_sensor_available_internal(2)
}

#[allow(clippy::unused_async)]
pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
    read_sensor_internal(2)
}

pub fn magnetometer_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = SensorData> + Send, SensorError> {
    if !magnetometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        (magnetometer_read().await).map_or(None, |data| Some((data, ())))
    }))
}

pub fn barometer_available() -> bool {
    is_sensor_available_internal(6)
}

#[allow(clippy::unused_async)]
pub async fn barometer_read() -> Result<ScalarData, SensorError> {
    read_pressure_internal()
}

pub fn barometer_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = ScalarData> + Send, SensorError> {
    if !barometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        (barometer_read().await).map_or(None, |data| Some((data, ())))
    }))
}

pub fn ambient_light_available() -> bool {
    is_sensor_available_internal(5)
}

#[allow(clippy::unused_async)]
pub async fn ambient_light_read() -> Result<ScalarData, SensorError> {
    read_light_internal()
}

pub fn ambient_light_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = ScalarData> + Send, SensorError> {
    if !ambient_light_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        (ambient_light_read().await).map_or(None, |data| Some((data, ())))
    }))
}
