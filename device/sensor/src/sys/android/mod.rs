//! Android sensor implementation using JNI.

use crate::{ScalarData, SensorData, SensorError};
use futures::channel::oneshot;
use futures::{StreamExt, stream};
use jni::objects::{JDoubleArray, JObject, JValue};
use jni::signature::MethodSignature;
use jni::strings::JNIStr;
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, FromJava, NativeCallback, PeerError, dex_helper, with_android_context,
};
use waterkit_core::Timestamp;

/// `waterkit.sensor.SensorHelper`, embedded as a DEX by this crate's build script and
/// loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.sensor.SensorHelper");

impl From<AndroidError> for SensorError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

/// The `[D` payload a `NativeCallback` delivers: sensor values followed by
/// the epoch-millisecond timestamp.
struct RawReading(Vec<f64>);

impl FromJava for RawReading {
    fn from_java(env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        let array = env.as_cast::<JDoubleArray>(object)?;
        let mut values = vec![0.0f64; array.len(env)?];
        array.get_region(env, 0, &mut values)?;
        Ok(Self(values))
    }
}

impl From<PeerError> for SensorError {
    fn from(error: PeerError) -> Self {
        Self::Platform(error.to_string())
    }
}

fn parse_sensor_reading(values: &[f64]) -> Result<SensorData, SensorError> {
    let [x, y, z, timestamp] = values else {
        return Err(SensorError::Platform("invalid sensor reading".into()));
    };
    Ok(SensorData::new(
        *x,
        *y,
        *z,
        timestamp_from_jni_double(*timestamp)?,
    ))
}

fn parse_scalar_reading(values: &[f64]) -> Result<ScalarData, SensorError> {
    let [value, timestamp] = values else {
        return Err(SensorError::Platform("invalid scalar reading".into()));
    };
    Ok(ScalarData::new(
        *value,
        timestamp_from_jni_double(*timestamp)?,
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
/// Returns [`SensorError`] when the sensor is unavailable, DEX initialization,
/// helper loading, JNI access, or payload decoding fails, or the read reports
/// failure.
pub async fn read_sensor_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    sensor_type: i32,
) -> Result<SensorData, SensorError> {
    let receiver = begin_read(
        env,
        context,
        jni_str!("readSensor"),
        jni_sig!("(Landroid/content/Context;ILwaterkit/build/NativeCallback;)V"),
        &[JValue::Int(sensor_type)],
    )?;
    let reading = finish_read(receiver).await?;
    parse_sensor_reading(&reading.0)
}

/// Read pressure data with an explicit Android `Context`.
///
/// # Errors
/// Returns [`SensorError`] when the sensor is unavailable, DEX initialization,
/// helper loading, JNI access, or payload decoding fails, or the read reports
/// failure.
pub async fn read_pressure_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<ScalarData, SensorError> {
    let receiver = begin_read(
        env,
        context,
        jni_str!("readPressure"),
        jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeCallback;)V"),
        &[],
    )?;
    let reading = finish_read(receiver).await?;
    parse_scalar_reading(&reading.0)
}

/// Read ambient light data with an explicit Android `Context`.
///
/// # Errors
/// Returns [`SensorError`] when the sensor is unavailable, DEX initialization,
/// helper loading, JNI access, or payload decoding fails, or the read reports
/// failure.
pub async fn read_light_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<ScalarData, SensorError> {
    let receiver = begin_read(
        env,
        context,
        jni_str!("readLight"),
        jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeCallback;)V"),
        &[],
    )?;
    let reading = finish_read(receiver).await?;
    parse_scalar_reading(&reading.0)
}

/// Creates a `NativeCallback<RawReading>` and hands it to the helper's `method`
/// entry point, returning the receiver the reading resolves on.
fn begin_read(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    method: &'static JNIStr,
    signature: MethodSignature<'_, '_>,
    args: &[JValue],
) -> Result<oneshot::Receiver<Result<RawReading, PeerError>>, SensorError> {
    let helper = HELPER.class(env, context)?;
    let (callback, receiver) = NativeCallback::<RawReading>::new(env)?;
    let all_args: Vec<JValue> = std::iter::once(JValue::Object(context))
        .chain(args.iter().copied())
        .chain(std::iter::once(JValue::Object(callback.as_obj())))
        .collect();
    env.call_static_method(helper, method, signature, &all_args)
        .map_err(|e| SensorError::Platform(format!("{method}: {e}")))?;
    Ok(receiver)
}

/// Awaits one reading: cancellation means the callback object was released
/// unanswered.
async fn finish_read(
    receiver: oneshot::Receiver<Result<RawReading, PeerError>>,
) -> Result<RawReading, SensorError> {
    receiver
        .await
        .map_err(|_| SensorError::Platform("sensor read was abandoned".into()))?
        .map_err(SensorError::from)
}

// --- Parameter-less API Implementation using ndk-context ---

fn is_sensor_available_internal(sensor_type: i32) -> bool {
    with_android_context(|env, context| is_sensor_available_with_context(env, context, sensor_type))
        .unwrap_or(false)
}

async fn read_sensor_internal(sensor_type: i32) -> Result<SensorData, SensorError> {
    let receiver = with_android_context(|env, context| {
        begin_read(
            env,
            context,
            jni_str!("readSensor"),
            jni_sig!("(Landroid/content/Context;ILwaterkit/build/NativeCallback;)V"),
            &[JValue::Int(sensor_type)],
        )
    })?;
    let reading = finish_read(receiver).await?;
    parse_sensor_reading(&reading.0)
}

async fn read_pressure_internal() -> Result<ScalarData, SensorError> {
    let receiver = with_android_context(|env, context| {
        begin_read(
            env,
            context,
            jni_str!("readPressure"),
            jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeCallback;)V"),
            &[],
        )
    })?;
    let reading = finish_read(receiver).await?;
    parse_scalar_reading(&reading.0)
}

async fn read_light_internal() -> Result<ScalarData, SensorError> {
    let receiver = with_android_context(|env, context| {
        begin_read(
            env,
            context,
            jni_str!("readLight"),
            jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeCallback;)V"),
            &[],
        )
    })?;
    let reading = finish_read(receiver).await?;
    parse_scalar_reading(&reading.0)
}

pub fn accelerometer_available() -> bool {
    is_sensor_available_internal(1)
}

pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
    read_sensor_internal(1).await
}

#[expect(
    clippy::unused_async,
    reason = "keeps the sys-impl signature uniform across platforms"
)]
pub async fn accelerometer_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = Result<SensorData, SensorError>> + Send, SensorError> {
    if !accelerometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        Some((accelerometer_read().await, ()))
    }))
}

pub fn gyroscope_available() -> bool {
    is_sensor_available_internal(4)
}

pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
    read_sensor_internal(4).await
}

#[expect(
    clippy::unused_async,
    reason = "keeps the sys-impl signature uniform across platforms"
)]
pub async fn gyroscope_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = Result<SensorData, SensorError>> + Send, SensorError> {
    if !gyroscope_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        Some((gyroscope_read().await, ()))
    }))
}

pub fn magnetometer_available() -> bool {
    is_sensor_available_internal(2)
}

pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
    read_sensor_internal(2).await
}

#[expect(
    clippy::unused_async,
    reason = "keeps the sys-impl signature uniform across platforms"
)]
pub async fn magnetometer_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = Result<SensorData, SensorError>> + Send, SensorError> {
    if !magnetometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        Some((magnetometer_read().await, ()))
    }))
}

pub fn barometer_available() -> bool {
    is_sensor_available_internal(6)
}

pub async fn barometer_read() -> Result<ScalarData, SensorError> {
    read_pressure_internal().await
}

#[expect(
    clippy::unused_async,
    reason = "keeps the sys-impl signature uniform across platforms"
)]
pub async fn barometer_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = Result<ScalarData, SensorError>> + Send, SensorError> {
    if !barometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        Some((barometer_read().await, ()))
    }))
}

pub fn ambient_light_available() -> bool {
    is_sensor_available_internal(5)
}

pub async fn ambient_light_read() -> Result<ScalarData, SensorError> {
    read_light_internal().await
}

#[expect(
    clippy::unused_async,
    reason = "keeps the sys-impl signature uniform across platforms"
)]
pub async fn ambient_light_watch(
    interval_ms: u32,
) -> Result<impl futures_core::Stream<Item = Result<ScalarData, SensorError>> + Send, SensorError> {
    if !ambient_light_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        Some((ambient_light_read().await, ()))
    }))
}
