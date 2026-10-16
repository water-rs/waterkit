//! Android location implementation using JNI.

use crate::{Location, LocationCapabilities, LocationError, LocationProvider, Timestamp};
use futures::channel::oneshot;
use jni::{
    Env, jni_sig, jni_str,
    objects::{JObject, JValue},
    strings::JNIStr,
};
use std::sync::OnceLock;
use waterkit_build::{
    AndroidError, DexHelper, FromJava, NativeCallback, PeerError, describe_jni_error, dex_helper,
    with_android_context,
};

/// `waterkit.location.LocationHelper`, embedded as a DEX by this crate's build
/// script and loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.location.LocationHelper");

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
) -> Result<f64, jni::errors::Error> {
    env.get_field(result, name, jni_sig!("D"))
        .and_then(jni::objects::JValueOwned::d)
}

fn flag_field(
    env: &mut Env<'_>,
    result: &JObject<'_>,
    name: &JNIStr,
) -> Result<bool, jni::errors::Error> {
    env.get_field(result, name, jni_sig!("Z"))
        .and_then(jni::objects::JValueOwned::z)
}

/// What the Kotlin helper completes the request's `NativeCallback` with:
/// the `LocationHelper.Result` read field-by-field.
struct LocationOutcome {
    status: i32,
    latitude: f64,
    longitude: f64,
    has_altitude: bool,
    altitude: f64,
    has_horizontal_accuracy: bool,
    horizontal_accuracy: f64,
    has_vertical_accuracy: bool,
    vertical_accuracy: f64,
    time_millis: i64,
}

impl FromJava for LocationOutcome {
    fn from_java(env: &mut Env<'_>, result: &JObject<'_>) -> Result<Self, AndroidError> {
        Ok(Self {
            status: env
                .get_field(result, jni_str!("status"), jni_sig!("I"))
                .and_then(jni::objects::JValueOwned::i)?,
            latitude: double_field(env, result, jni_str!("latitude"))?,
            longitude: double_field(env, result, jni_str!("longitude"))?,
            has_altitude: flag_field(env, result, jni_str!("hasAltitude"))?,
            altitude: double_field(env, result, jni_str!("altitude"))?,
            has_horizontal_accuracy: flag_field(env, result, jni_str!("hasHorizontalAccuracy"))?,
            horizontal_accuracy: double_field(env, result, jni_str!("horizontalAccuracy"))?,
            has_vertical_accuracy: flag_field(env, result, jni_str!("hasVerticalAccuracy"))?,
            vertical_accuracy: double_field(env, result, jni_str!("verticalAccuracy"))?,
            time_millis: env
                .get_field(result, jni_str!("timeMillis"), jni_sig!("J"))
                .and_then(jni::objects::JValueOwned::j)?,
        })
    }
}

impl LocationOutcome {
    /// The outcome's typed `Location`, or the status's [`LocationError`].
    fn into_location(self) -> Result<Location, LocationError> {
        match self.status {
            STATUS_SUCCESS => {}
            STATUS_PERMISSION_DENIED => return Err(LocationError::PermissionDenied),
            STATUS_SERVICE_DISABLED => return Err(LocationError::ServiceDisabled),
            STATUS_UNAVAILABLE => return Err(LocationError::NotAvailable),
            other => {
                return Err(LocationError::Platform(format!(
                    "Android location helper returned unknown status {other}"
                )));
            }
        }

        let timestamp = Timestamp::from_millisecond(self.time_millis)
            .map_err(|error| LocationError::Platform(error.to_string()))?;

        let mut location = Location::from_degrees(self.latitude, self.longitude, timestamp)?;
        if self.has_altitude {
            location = location.with_altitude(self.altitude);
        }
        if self.has_horizontal_accuracy {
            location = location.with_horizontal_accuracy(self.horizontal_accuracy);
        }
        if self.has_vertical_accuracy {
            location = location.with_vertical_accuracy(self.vertical_accuracy);
        }
        Ok(location)
    }
}

/// Starts the provider's fix and returns the receiver the `NativeCallback`
/// answers. The Kotlin listener completes the callback on the platform's
/// delivery thread; nothing is parked waiting.
fn request_fix(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<oneshot::Receiver<Result<LocationOutcome, PeerError>>, LocationError> {
    let realization = realization_with_context(env, context)?;
    let helper_class = HELPER.class(env, context)?;
    let (callback, rx) = NativeCallback::<LocationOutcome>::new(env)?;
    env.call_static_method(
        helper_class,
        realization.request_method(),
        jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeCallback;)V"),
        &[JValue::Object(context), JValue::Object(callback.as_obj())],
    )
    .map_err(|error| {
        LocationError::Platform(format!(
            "request Android location failed: {}",
            describe_jni_error(env, error)
        ))
    })?;
    Ok(rx)
}

/// Awaits the fix's `NativeCallback` answer and decodes it into the
/// request's [`Location`].
async fn await_fix(
    rx: oneshot::Receiver<Result<LocationOutcome, PeerError>>,
) -> Result<Location, LocationError> {
    rx.await
        .map_err(|_| {
            LocationError::Platform(String::from(
                "the location callback was collected unanswered",
            ))
        })?
        .map_err(|error| LocationError::Platform(error.to_string()))?
        .into_location()
}

/// Requests a fresh location fix using an Android `Context`, through the
/// provider [`provider_with_context`] reports.
///
/// Every JNI call runs before this returns; the future it hands back only
/// awaits the request's `NativeCallback` receiver, so it is `Send` and the
/// caller bounds its wait itself.
///
/// # Errors
///
/// Returns [`LocationError::Platform`] when the JNI calls fail. The
/// returned future yields [`LocationError::PermissionDenied`] when neither
/// fine nor coarse location permission is granted,
/// [`LocationError::ServiceDisabled`] when no location provider is enabled,
/// [`LocationError::NotAvailable`] when the platform reports no location,
/// or [`LocationError::Platform`] when the callback's answer fails to
/// decode.
pub fn get_location_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<
    impl std::future::Future<Output = Result<Location, LocationError>> + Send + use<>,
    LocationError,
> {
    request_fix(env, context).map(await_fix)
}

pub async fn capabilities() -> LocationCapabilities {
    let realization = REALIZATION.get().copied().unwrap_or_else(|| {
        with_android_context(realization_with_context).unwrap_or_else(|error| {
            panic!("waterkit-location: failed to choose the Android location provider: {error}")
        })
    });
    LocationCapabilities {
        provider: Some(realization.into()),
    }
}

pub async fn get_location() -> Result<Location, LocationError> {
    await_fix(with_android_context(request_fix)?).await
}
