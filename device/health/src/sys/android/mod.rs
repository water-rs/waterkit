use crate::{HealthDataType, HealthError, HealthSample};
use futures::future;
use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use std::mem::ManuallyDrop;
use std::sync::OnceLock;
use waterkit_core::Timestamp;

const HELPER_CLASS_NAME: &str = "waterkit.health.HealthHelper";
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

fn with_android_context<T, F>(f: F) -> Result<T, HealthError>
where
    F: for<'local> FnOnce(&mut JNIEnv<'local>, &JObject<'local>) -> Result<T, HealthError>,
{
    let android_context = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(android_context.vm().cast()) }
        .map_err(|error| HealthError::Platform(format!("JavaVM::from_raw failed: {error}")))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| HealthError::Platform(format!("attach_current_thread failed: {error}")))?;

    let context = ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
    assert!(
        !context.is_null(),
        "waterkit-health: ndk_context returned null Android Context"
    );

    f(&mut env, &context)
}

fn init_dex(env: &mut JNIEnv, context: &JObject) -> Result<(), HealthError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| HealthError::Platform(format!("Context.getCacheDir failed: {error}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| HealthError::Platform(format!("File.getAbsolutePath failed: {error}")))?;

    let cache_path_string: String = env
        .get_string(&JString::from(cache_path))
        .map_err(|error| HealthError::Platform(format!("cache path decode failed: {error}")))?
        .into();

    let dex_path = format!("{cache_path_string}/waterkit_health.dex");
    let _ = std::fs::remove_file(&dex_path);
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|error| HealthError::Platform(format!("write DEX failed: {error}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(&dex_path)
            .map_err(|error| HealthError::Platform(format!("dex metadata failed: {error}")))?
            .permissions();
        permissions.set_mode(0o444);
        std::fs::set_permissions(&dex_path, permissions).map_err(|error| {
            HealthError::Platform(format!("set dex permissions failed: {error}"))
        })?;
    }

    let dex_path_java = env
        .new_string(dex_path)
        .map_err(|error| HealthError::Platform(format!("new dex path string failed: {error}")))?;
    let cache_path_java = env
        .new_string(cache_path_string)
        .map_err(|error| HealthError::Platform(format!("new cache path string failed: {error}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| {
            HealthError::Platform(format!("Context.getClassLoader failed: {error}"))
        })?;

    let dex_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|error| HealthError::Platform(format!("find DexClassLoader failed: {error}")))?;

    let class_loader = env
        .new_object(
            dex_loader_class,
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue::Object(&dex_path_java),
                JValue::Object(&cache_path_java),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|error| HealthError::Platform(format!("new DexClassLoader failed: {error}")))?;

    let class_loader_global = env
        .new_global_ref(class_loader)
        .map_err(|error| HealthError::Platform(format!("new_global_ref failed: {error}")))?;

    if CLASS_LOADER.set(class_loader_global).is_err() {
        assert!(
            CLASS_LOADER.get().is_some(),
            "waterkit-health: class loader initialization race left loader unset"
        );
    }

    Ok(())
}

fn get_helper_class<'local>(env: &mut JNIEnv<'local>) -> Result<JClass<'local>, HealthError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| HealthError::Platform("class loader not initialized".into()))?;

    let helper_name = env.new_string(HELPER_CLASS_NAME).map_err(|error| {
        HealthError::Platform(format!("new helper class string failed: {error}"))
    })?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_name)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| HealthError::Platform(format!("ClassLoader.loadClass failed: {error}")))?;

    Ok(helper_class.into())
}

fn is_available_with_context(env: &mut JNIEnv, context: &JObject) -> Result<bool, HealthError> {
    init_dex(env, context)?;
    let helper_class = get_helper_class(env)?;
    env.call_static_method(&helper_class, "isAvailable", "()Z", &[])
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
        init_dex(env, context)?;
        let helper_class = get_helper_class(env)?;

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
                "querySamples",
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
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

        let payload: String = env
            .get_string(&JString::from(payload))
            .map_err(|error| {
                HealthError::Platform(format!("decode querySamples payload failed: {error}"))
            })?
            .into();
        Ok(parse_samples(data_type, &payload))
    }))
    .await
}

pub async fn write_sample(sample: HealthSample) -> Result<(), HealthError> {
    let data_type_name = type_to_str(sample.data_type());
    let start_str = sample.start().to_string();
    let end_str = sample.end().to_string();
    future::ready(with_android_context(|env, context| {
        init_dex(env, context)?;
        let helper_class = get_helper_class(env)?;

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
                "writeSample",
                "(Landroid/content/Context;Ljava/lang/String;DLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
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
            let message: String = env
                .get_string(&JString::from(error))
                .map_err(|error| {
                    HealthError::Platform(format!(
                        "decode writeSample error failed: {error}"
                    ))
                })?
                .into();
            Err(map_android_health_error(message))
        }
    }))
    .await
}
