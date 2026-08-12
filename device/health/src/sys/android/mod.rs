use crate::{HealthDataType, HealthError, HealthSample};
use futures::future;
use jni::objects::{Global, JClass, JObject, JString, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::sync::OnceLock;
use waterkit_core::Timestamp;

const HELPER_CLASS_NAME: &str = "waterkit.health.HealthHelper";
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// `waterkit.health.HealthHelper`, loaded once from [`DEX_BYTES`].
static HELPER_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

fn with_android_context<T, F>(f: F) -> Result<T, HealthError>
where
    F: FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, HealthError>,
{
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    assert!(
        !raw_vm.is_null(),
        "waterkit-health: ndk_context returned a null JavaVM"
    );
    assert!(
        !raw_context.is_null(),
        "waterkit-health: ndk_context returned a null Android Context"
    );

    // SAFETY: `ndk_context` publishes the process' JavaVM pointer, which stays
    // valid for the lifetime of the application.
    let vm = unsafe { JavaVM::from_raw(raw_vm) };
    vm.attach_current_thread(
        |env| -> Result<Result<T, HealthError>, jni::errors::Error> {
            // SAFETY: `ndk_context` publishes a global reference to the application
            // `Context` that outlives this attachment, and `as_cast_raw` only
            // borrows it.
            let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
            Ok(f(env, &context))
        },
    )
    .map_err(|error| HealthError::Platform(format!("attach_current_thread failed: {error}")))?
}

/// Returns the cached helper class, loading the embedded DEX on first use.
fn helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<&'static Global<JClass<'static>>, HealthError> {
    if let Some(class) = HELPER_CLASS.get() {
        return Ok(class);
    }

    let class = load_helper_class(env, context)?;
    Ok(HELPER_CLASS.get_or_init(|| class))
}

fn load_helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Global<JClass<'static>>, HealthError> {
    let parent_loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| {
            HealthError::Platform(format!("Context.getClassLoader failed: {error}"))
        })?;

    let dex_bytes = env
        .byte_array_from_slice(DEX_BYTES)
        .map_err(|error| HealthError::Platform(format!("copy DEX failed: {error}")))?;
    let dex_buffer = env
        .call_static_method(
            jni_str!("java/nio/ByteBuffer"),
            jni_str!("wrap"),
            jni_sig!("([B)Ljava/nio/ByteBuffer;"),
            &[JValue::Object(&dex_bytes)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| HealthError::Platform(format!("wrap DEX failed: {error}")))?;
    let class_loader = env
        .new_object(
            jni_str!("dalvik/system/InMemoryDexClassLoader"),
            jni_sig!("(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V"),
            &[JValue::Object(&dex_buffer), JValue::Object(&parent_loader)],
        )
        .map_err(|error| {
            HealthError::Platform(format!("new InMemoryDexClassLoader failed: {error}"))
        })?;

    let helper_name = env.new_string(HELPER_CLASS_NAME).map_err(|error| {
        HealthError::Platform(format!("new helper class string failed: {error}"))
    })?;
    let helper_class = env
        .call_method(
            &class_loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&helper_name)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| HealthError::Platform(format!("ClassLoader.loadClass failed: {error}")))?;
    let helper_class = env.cast_local::<JClass>(helper_class).map_err(|error| {
        HealthError::Platform(format!("loadClass returned a non-class: {error}"))
    })?;

    env.new_global_ref(helper_class)
        .map_err(|error| HealthError::Platform(format!("new_global_ref failed: {error}")))
}

fn decode_string(env: &Env<'_>, value: &JObject<'_>) -> Result<String, HealthError> {
    env.as_cast::<JString>(value)
        .and_then(|text| text.try_to_string(env))
        .map_err(|error| HealthError::Platform(format!("string decode failed: {error}")))
}

fn is_available_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<bool, HealthError> {
    let helper_class = helper_class(env, context)?;
    env.call_static_method(helper_class, jni_str!("isAvailable"), jni_sig!("()Z"), &[])
        .map_err(|error| {
            HealthError::Platform(format!("HealthHelper.isAvailable failed: {error}"))
        })?
        .z()
        .map_err(|error| {
            HealthError::Platform(format!("isAvailable result decode failed: {error}"))
        })
}

pub fn is_available() -> bool {
    with_android_context(is_available_with_context).unwrap_or_else(|error| {
        panic!("waterkit-health: failed to query availability with Android context: {error}")
    })
}

const fn type_to_str(data_type: HealthDataType) -> &'static str {
    match data_type {
        HealthDataType::Steps => "steps",
        HealthDataType::HeartRate => "heartRate",
        HealthDataType::ActiveEnergy => "activeEnergy",
        HealthDataType::Distance => "distance",
        HealthDataType::Weight => "weight",
        HealthDataType::Height => "height",
        HealthDataType::BloodOxygen => "bloodOxygen",
        HealthDataType::Sleep => "sleep",
    }
}

fn parse_samples(data_type: HealthDataType, payload: &str) -> Vec<HealthSample> {
    payload
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(5, '\t').collect();
            if parts.len() < 4 {
                return None;
            }
            let start = parts[2].parse::<Timestamp>().ok()?;
            let end = parts[3].parse::<Timestamp>().ok()?;
            let mut sample = HealthSample::new(
                data_type,
                parts[0].parse().unwrap_or(0.0),
                parts[1],
                start,
                end,
            );
            if let Some(src) = parts.get(4).filter(|value| !value.is_empty()) {
                sample = sample.source(*src);
            }
            Some(sample)
        })
        .collect()
}

fn map_android_health_error(error: String) -> HealthError {
    if error.to_ascii_lowercase().contains("permission") {
        HealthError::PermissionDenied
    } else {
        HealthError::Platform(error)
    }
}

pub async fn query_samples(
    data_type: HealthDataType,
    start: Timestamp,
    end: Timestamp,
) -> Result<Vec<HealthSample>, HealthError> {
    let data_type_name = type_to_str(data_type);
    let start_str = start.to_string();
    let end_str = end.to_string();
    future::ready(with_android_context(|env, context| {
        let helper_class = helper_class(env, context)?;

        let data_type_java = env.new_string(data_type_name).map_err(|error| {
            HealthError::Platform(format!("new_string data_type failed: {error}"))
        })?;
        let start = env.new_string(&start_str).map_err(|error| {
            HealthError::Platform(format!("new_string start failed: {error}"))
        })?;
        let end = env.new_string(&end_str).map_err(|error| {
            HealthError::Platform(format!("new_string end failed: {error}"))
        })?;

        let payload = env
            .call_static_method(
                helper_class,
                jni_str!("querySamples"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
                &[
                    JValue::Object(context),
                    JValue::Object(&data_type_java),
                    JValue::Object(&start),
                    JValue::Object(&end),
                ],
            )
            .map_err(|error| {
                HealthError::Platform(format!("HealthHelper.querySamples failed: {error}"))
            })?
            .l()
            .map_err(|error| {
                HealthError::Platform(format!("querySamples return decode failed: {error}"))
            })?;
        if payload.is_null() {
            return Ok(Vec::new());
        }

        let payload = decode_string(env, &payload)?;
        Ok(parse_samples(data_type, &payload))
    }))
    .await
}

pub async fn write_sample(sample: HealthSample) -> Result<(), HealthError> {
    let data_type_name = type_to_str(sample.data_type());
    let start_str = sample.start().to_string();
    let end_str = sample.end().to_string();
    future::ready(with_android_context(|env, context| {
        let helper_class = helper_class(env, context)?;

        let data_type = env.new_string(data_type_name).map_err(|error| {
            HealthError::Platform(format!("new_string data_type failed: {error}"))
        })?;
        let unit = env.new_string(sample.unit()).map_err(|error| {
            HealthError::Platform(format!("new_string unit failed: {error}"))
        })?;
        let start = env.new_string(&start_str).map_err(|error| {
            HealthError::Platform(format!("new_string start_date failed: {error}"))
        })?;
        let end = env.new_string(&end_str).map_err(|error| {
            HealthError::Platform(format!("new_string end_date failed: {error}"))
        })?;

        let error = env
            .call_static_method(
                helper_class,
                jni_str!("writeSample"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;DLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
                &[
                    JValue::Object(context),
                    JValue::Object(&data_type),
                    JValue::Double(sample.value()),
                    JValue::Object(&unit),
                    JValue::Object(&start),
                    JValue::Object(&end),
                ],
            )
            .map_err(|error| {
                HealthError::Platform(format!("HealthHelper.writeSample failed: {error}"))
            })?
            .l()
            .map_err(|error| {
                HealthError::Platform(format!("writeSample return decode failed: {error}"))
            })?;

        if error.is_null() {
            Ok(())
        } else {
            Err(map_android_health_error(decode_string(env, &error)?))
        }
    }))
    .await
}
