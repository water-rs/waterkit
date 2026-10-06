//! Android location implementation using JNI.

use crate::{Location, LocationCapabilities, LocationError, LocationProvider, Timestamp};
use jni::{
    Env, jni_sig, jni_str,
    objects::{JObject, JValue},
    strings::JNIStr,
};
use std::sync::OnceLock;
use waterkit_build::{
    AndroidError, DexHelper, describe_jni_error, dex_helper, with_android_context,
};

/// `waterkit.location.LocationHelper`, embedded as a DEX by this crate's build
/// script and loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.location.LocationHelper");

/// How long the platform may take to produce a fix before the request fails
/// with [`LocationError::Timeout`]. Matches the Apple implementation's
/// `locationRequestTimeout`.
const LOCATION_REQUEST_TIMEOUT_MS: i64 = 10_000;

/// The realization serving location on this device, chosen once per process
/// from whether Google Play services is usable.
#[derive(Debug, Clone, Copy)]
enum Realization {
    /// The Fused Location Provider of Google Play services.
    Fused,
    /// The framework `LocationManager`.
    Framework,
}

impl Realization {
    /// The `LocationHelper` method that requests a fix through this realization.
    const fn request_method(self) -> &'static JNIStr {
        match self {
            Self::Fused => jni_str!("getFusedLocation"),
            Self::Framework => jni_str!("getFrameworkLocation"),
        }
    }
}

impl From<Realization> for LocationProvider {
    fn from(realization: Realization) -> Self {
        match realization {
            Realization::Fused => Self::FusedLocationProvider,
            Realization::Framework => Self::AndroidLocationManager,
        }
    }
}

static REALIZATION: OnceLock<Realization> = OnceLock::new();

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

fn realization_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Realization, LocationError> {
    if let Some(realization) = REALIZATION.get() {
        return Ok(*realization);
    }
    let helper_class = HELPER.class(env, context)?;
    let has_play_services = env
        .call_static_method(
            helper_class,
            jni_str!("hasGooglePlayServices"),
            jni_sig!("(Landroid/content/Context;)Z"),
            &[JValue::Object(context)],
        )
        .and_then(jni::objects::JValueOwned::z)
        .map_err(|error| {
            LocationError::Platform(format!(
                "probe Google Play services failed: {}",
                describe_jni_error(env, error)
            ))
        })?;
    let realization = if has_play_services {
        Realization::Fused
    } else {
        Realization::Framework
    };
    tracing::debug!(
        ?realization,
        "waterkit-location: chose the Android location realization"
    );
    Ok(*REALIZATION.get_or_init(|| realization))
}

/// Reports which [`LocationProvider`] serves location on this device, using
/// an Android `Context`.
///
/// The first call asks `GoogleApiAvailability` whether Google Play services
/// is usable and fixes the answer for the process: its Fused Location
/// Provider when it is, the framework `LocationManager` otherwise. Every
/// request then goes through that provider only.
///
/// # Errors
///
/// Returns [`LocationError::Platform`] when JNI fails.
pub fn provider_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<LocationProvider, LocationError> {
    realization_with_context(env, context).map(LocationProvider::from)
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

/// Requests a fresh location fix using an Android `Context`, through the
/// provider [`provider_with_context`] reports.
///
/// Blocks the calling thread until the platform delivers a fix or the request
/// times out, so this must not run on the Android main thread — the helper
/// waits for callbacks the main looper may deliver.
///
/// # Errors
///
/// Returns [`LocationError::PermissionDenied`] when neither fine nor coarse
/// location permission is granted, [`LocationError::ServiceDisabled`] when no
/// location provider is enabled, [`LocationError::Timeout`] when no fix
/// arrives in time, [`LocationError::NotAvailable`] when the platform reports
/// no location, or [`LocationError::Platform`] when JNI or Google Play
/// services fails.
pub fn get_location_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Location, LocationError> {
    let realization = realization_with_context(env, context)?;
    let helper_class = HELPER.class(env, context)?;
    let result = env
        .call_static_method(
            helper_class,
            realization.request_method(),
            jni_sig!("(Landroid/content/Context;J)Lwaterkit/location/LocationHelper$Result;"),
            &[
                JValue::Object(context),
                JValue::Long(LOCATION_REQUEST_TIMEOUT_MS),
            ],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| {
            LocationError::Platform(format!(
                "request Android location failed: {}",
                describe_jni_error(env, error)
            ))
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

/// Runs `work` with the Android context on a dedicated thread, so neither the
/// JNI calls nor the helper's wait for a fix block the awaiting task.
async fn on_location_thread<T: Send + 'static>(
    work: fn(&mut Env<'_>, &JObject<'_>) -> Result<T, LocationError>,
) -> Result<T, LocationError> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    std::thread::Builder::new()
        .name(String::from("waterkit-location"))
        .spawn(move || {
            let _ = sender.send(with_android_context(work));
        })
        .map_err(|error| {
            LocationError::Platform(format!("spawn Android location thread failed: {error}"))
        })?;
    receiver
        .await
        .map_err(|_| LocationError::Platform(String::from("Android location thread died")))?
}

pub async fn capabilities() -> LocationCapabilities {
    let realization = match REALIZATION.get() {
        Some(realization) => *realization,
        None => on_location_thread(realization_with_context)
            .await
            .unwrap_or_else(|error| {
                panic!("waterkit-location: failed to choose the Android location provider: {error}")
            }),
    };
    LocationCapabilities {
        provider: Some(realization.into()),
    }
}

pub async fn get_location() -> Result<Location, LocationError> {
    on_location_thread(get_location_with_context).await
}
