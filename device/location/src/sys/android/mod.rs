//! Android location implementation using JNI.

use crate::{Location, LocationError, Timestamp};
use jni::{
    Env, jni_sig, jni_str,
    objects::{JObject, JValue},
    strings::JNIStr,
};
use waterkit_build::{AndroidError, DexHelper, dex_helper, with_android_context};

/// `waterkit.location.LocationHelper`, embedded as a DEX by this crate's build
/// script and loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.location.LocationHelper");

/// How long the platform may take to produce a fix before the request fails
/// with [`LocationError::Timeout`]. Matches the Apple implementation's
/// `locationRequestTimeout`.
const LOCATION_REQUEST_TIMEOUT_MS: i64 = 10_000;

// Status codes mirrored from `LocationHelper.Result`.
const STATUS_SUCCESS: i32 = 0;
const STATUS_PERMISSION_DENIED: i32 = 1;
const STATUS_SERVICE_DISABLED: i32 = 2;
const STATUS_UNAVAILABLE: i32 = 3;
const STATUS_TIMEOUT: i32 = 4;

impl From<AndroidError> for LocationError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

fn double_field(
    env: &mut Env<'_>,
    result: &JObject<'_>,
    name: &JNIStr,
) -> Result<f64, LocationError> {
    env.get_field(result, name, jni_sig!("D"))
        .and_then(jni::objects::JValueOwned::d)
        .map_err(|error| {
            LocationError::Platform(format!(
                "read Android location field {name} failed: {error}"
            ))
        })
}

fn flag_field(
    env: &mut Env<'_>,
    result: &JObject<'_>,
    name: &JNIStr,
) -> Result<bool, LocationError> {
    env.get_field(result, name, jni_sig!("Z"))
        .and_then(jni::objects::JValueOwned::z)
        .map_err(|error| {
            LocationError::Platform(format!(
                "read Android location field {name} failed: {error}"
            ))
        })
}

/// Requests a fresh location fix using an Android `Context`.
///
/// Blocks the calling thread until the platform delivers a fix or the request
/// times out, so this must not run on the Android main thread — the helper
/// delivers its callback on the main looper.
///
/// # Errors
///
/// Returns [`LocationError::PermissionDenied`] when neither fine nor coarse
/// location permission is granted, [`LocationError::ServiceDisabled`] when no
/// location provider is enabled, [`LocationError::Timeout`] when no fix
/// arrives in time, [`LocationError::NotAvailable`] when the platform reports
/// no location, or [`LocationError::Platform`] when JNI fails.
pub fn get_location_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Location, LocationError> {
    let helper_class = HELPER.class(env, context)?;
    let result = env
        .call_static_method(
            helper_class,
            jni_str!("getCurrentLocation"),
            jni_sig!("(Landroid/content/Context;J)Lwaterkit/location/LocationHelper$Result;"),
            &[
                JValue::Object(context),
                JValue::Long(LOCATION_REQUEST_TIMEOUT_MS),
            ],
        )
        .map_err(|error| {
            LocationError::Platform(format!("request Android location failed: {error}"))
        })?
        .l()
        .map_err(|error| {
            LocationError::Platform(format!("read Android location result failed: {error}"))
        })?;

    let status = env
        .get_field(&result, jni_str!("status"), jni_sig!("I"))
        .and_then(jni::objects::JValueOwned::i)
        .map_err(|error| {
            LocationError::Platform(format!("read Android location status failed: {error}"))
        })?;
    match status {
        STATUS_SUCCESS => {}
        STATUS_PERMISSION_DENIED => return Err(LocationError::PermissionDenied),
        STATUS_SERVICE_DISABLED => return Err(LocationError::ServiceDisabled),
        STATUS_UNAVAILABLE => return Err(LocationError::NotAvailable),
        STATUS_TIMEOUT => return Err(LocationError::Timeout),
        other => {
            return Err(LocationError::Platform(format!(
                "Android location helper returned unknown status {other}"
            )));
        }
    }

    let latitude = double_field(env, &result, jni_str!("latitude"))?;
    let longitude = double_field(env, &result, jni_str!("longitude"))?;
    let time_millis = env
        .get_field(&result, jni_str!("timeMillis"), jni_sig!("J"))
        .and_then(jni::objects::JValueOwned::j)
        .map_err(|error| {
            LocationError::Platform(format!("read Android location timestamp failed: {error}"))
        })?;
    let timestamp = Timestamp::from_millisecond(time_millis)
        .map_err(|error| LocationError::Platform(error.to_string()))?;

    let mut location = Location::from_degrees(latitude, longitude, timestamp)?;
    if flag_field(env, &result, jni_str!("hasAltitude"))? {
        location = location.with_altitude(double_field(env, &result, jni_str!("altitude"))?);
    }
    if flag_field(env, &result, jni_str!("hasHorizontalAccuracy"))? {
        location = location.with_horizontal_accuracy(double_field(
            env,
            &result,
            jni_str!("horizontalAccuracy"),
        )?);
    }
    if flag_field(env, &result, jni_str!("hasVerticalAccuracy"))? {
        location = location.with_vertical_accuracy(double_field(
            env,
            &result,
            jni_str!("verticalAccuracy"),
        )?);
    }
    Ok(location)
}

pub async fn get_location() -> Result<Location, LocationError> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    std::thread::Builder::new()
        .name(String::from("waterkit-location"))
        .spawn(move || {
            let result = with_android_context(get_location_with_context);
            let _ = sender.send(result);
        })
        .map_err(|error| {
            LocationError::Platform(format!("spawn Android location thread failed: {error}"))
        })?;
    receiver
        .await
        .map_err(|_| LocationError::Platform(String::from("Android location thread died")))?
}
