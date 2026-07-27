//! Android location implementation using JNI.

use crate::{Location, LocationError, Timestamp};
use jni::{
    Env, JavaVM, jni_sig, jni_str,
    objects::{JClass, JDoubleArray, JObject, JValue},
};

/// Embedded DEX bytecode containing `LocationHelper`.
///
/// Generated at build time by `kotlinc` and D8.
const DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
const LOCATION_HELPER_CLASS: &str = "waterkit.location.LocationHelper";

fn helper_class<'local>(
    env: &mut Env<'local>,
    context: &JObject<'_>,
) -> Result<JClass<'local>, LocationError> {
    let parent_loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )
        .map_err(|error| {
            LocationError::Platform(format!("get Android location class loader failed: {error}"))
        })?
        .l()
        .map_err(|error| {
            LocationError::Platform(format!(
                "get Android location class loader result failed: {error}"
            ))
        })?;

    let dex_bytes = env.byte_array_from_slice(DEX_BYTES).map_err(|error| {
        LocationError::Platform(format!("create Android location DEX array failed: {error}"))
    })?;
    let byte_buffer = env
        .call_static_method(
            jni_str!("java/nio/ByteBuffer"),
            jni_str!("wrap"),
            jni_sig!("([B)Ljava/nio/ByteBuffer;"),
            &[JValue::Object(&dex_bytes)],
        )
        .map_err(|error| {
            LocationError::Platform(format!("wrap Android location DEX failed: {error}"))
        })?
        .l()
        .map_err(|error| {
            LocationError::Platform(format!("wrap Android location DEX result failed: {error}"))
        })?;
    let class_loader = env
        .new_object(
            jni_str!("dalvik/system/InMemoryDexClassLoader"),
            jni_sig!("(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V"),
            &[JValue::Object(&byte_buffer), JValue::Object(&parent_loader)],
        )
        .map_err(|error| {
            LocationError::Platform(format!(
                "create Android location in-memory class loader failed: {error}"
            ))
        })?;
    let helper_name = env.new_string(LOCATION_HELPER_CLASS).map_err(|error| {
        LocationError::Platform(format!(
            "create Android location helper class name failed: {error}"
        ))
    })?;
    let helper = env
        .call_method(
            &class_loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&helper_name)],
        )
        .map_err(|error| {
            LocationError::Platform(format!("load Android location helper failed: {error}"))
        })?
        .l()
        .map_err(|error| {
            LocationError::Platform(format!(
                "load Android location helper result failed: {error}"
            ))
        })?;
    env.cast_local::<JClass>(helper).map_err(|error| {
        LocationError::Platform(format!(
            "cast Android location helper class failed: {error}"
        ))
    })
}

/// Get the last known location using an Android `Context`.
///
/// # Errors
///
/// Returns [`LocationError::NotAvailable`] when Android has no last known
/// location, or [`LocationError::Platform`] when JNI returns invalid data.
pub fn get_location_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Location, LocationError> {
    let helper_class = helper_class(env, context)?;
    let result = env
        .call_static_method(
            helper_class,
            jni_str!("getLastKnownLocation"),
            jni_sig!("(Landroid/content/Context;)[D"),
            &[JValue::Object(context)],
        )
        .map_err(|error| {
            LocationError::Platform(format!("get Android last known location failed: {error}"))
        })?
        .l()
        .map_err(|error| {
            LocationError::Platform(format!(
                "get Android last known location result failed: {error}"
            ))
        })?;
    let result = env.cast_local::<JDoubleArray>(result).map_err(|error| {
        LocationError::Platform(format!("cast Android location result failed: {error}"))
    })?;
    let len = result.len(env).map_err(|error| {
        LocationError::Platform(format!(
            "read Android location result length failed: {error}"
        ))
    })?;
    if len == 0 {
        return Err(LocationError::NotAvailable);
    }

    let mut values = vec![0.0_f64; len];
    result.get_region(env, 0, &mut values).map_err(|error| {
        LocationError::Platform(format!("read Android location result failed: {error}"))
    })?;
    if values[0] < 0.5 {
        return Err(LocationError::NotAvailable);
    }
    let [
        _,
        latitude,
        longitude,
        altitude,
        horizontal_accuracy,
        timestamp_ms,
        ..,
    ] = values.as_slice()
    else {
        return Err(LocationError::Platform(String::from(
            "Android location result omitted required fields",
        )));
    };

    #[expect(
        clippy::cast_possible_truncation,
        reason = "Android Location timestamps are represented as integral milliseconds in a double array"
    )]
    let timestamp = Timestamp::from_millisecond(*timestamp_ms as i64)
        .map_err(|error| LocationError::Platform(error.to_string()))?;
    Ok(Location::from_degrees(*latitude, *longitude, timestamp)?
        .with_altitude(*altitude)
        .with_horizontal_accuracy(*horizontal_accuracy))
}

#[derive(Debug)]
struct AttachedLocationError(LocationError);

impl std::fmt::Display for AttachedLocationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for AttachedLocationError {}

impl From<jni::errors::Error> for AttachedLocationError {
    fn from(error: jni::errors::Error) -> Self {
        Self(LocationError::Platform(error.to_string()))
    }
}

pub async fn get_location() -> Result<Location, LocationError> {
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    assert!(
        !raw_vm.is_null(),
        "waterkit-location: ndk_context returned null JavaVM"
    );
    assert!(
        !raw_context.is_null(),
        "waterkit-location: ndk_context returned null Android Context"
    );

    let vm = unsafe { JavaVM::from_raw(raw_vm) };
    vm.attach_current_thread(|env| -> Result<Location, AttachedLocationError> {
        let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
        get_location_with_context(env, &context).map_err(AttachedLocationError)
    })
    .map_err(|error| error.0)
}
