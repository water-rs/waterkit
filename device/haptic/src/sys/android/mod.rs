//! Android haptic implementation using JNI.

use crate::{HapticError, HapticPattern, HapticStep, Intensity};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JValue};
use std::mem::ManuallyDrop;
use std::sync::OnceLock;

/// Embedded DEX bytecode containing `HapticHelper` class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Initialize the DEX class loader. Must be called with a valid Context.
pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), HapticError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Write DEX to cache directory
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|e| HapticError::Platform(format!("getCacheDir failed: {e}")))?
        .l()
        .map_err(|e| HapticError::Platform(format!("getCacheDir result: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| HapticError::Platform(format!("getAbsolutePath failed: {e}")))?
        .l()
        .map_err(|e| HapticError::Platform(format!("getAbsolutePath result: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_haptic.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| HapticError::Platform(format!("get_string failed: {e}")))?
            .to_str()
            .map_err(|e| HapticError::Platform(format!("to_str failed: {e}")))?
    );

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| HapticError::Platform(format!("write DEX failed: {e}")))?;

    // Create DexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| HapticError::Platform(format!("new_string failed: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| HapticError::Platform(format!("getClassLoader failed: {e}")))?
        .l()
        .map_err(|e| HapticError::Platform(format!("getClassLoader result: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| HapticError::Platform(format!("find DexClassLoader: {e}")))?;

    let class_loader = env
        .new_object(
            dex_class_loader_class,
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue::Object(&dex_path_jstring),
                JValue::Object(&cache_path),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|e| HapticError::Platform(format!("new DexClassLoader: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| HapticError::Platform(format!("new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>, HapticError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| HapticError::InitFailed("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.haptic.HapticHelper")
        .map_err(|e| HapticError::Platform(format!("new_string: {e}")))?;

    env.call_method(
        class_loader.as_obj(),
        "loadClass",
        "(Ljava/lang/String;)Ljava/lang/Class;",
        &[JValue::Object(&helper_class_name)],
    )
    .map_err(|e| HapticError::Platform(format!("loadClass: {e}")))?
    .l()
    .map_err(|e| HapticError::Platform(format!("loadClass result: {e}")))
}

fn duration_millis_i64(duration: std::time::Duration) -> Result<i64, HapticError> {
    i64::try_from(duration.as_millis()).map_err(|_| {
        HapticError::Platform(format!(
            "haptic duration exceeds i64 milliseconds: {duration:?}"
        ))
    })
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "Intensity is constrained to 0.0..=1.0 and Android amplitudes are integer values in 0..=255."
)]
fn amplitude_i32(intensity: Intensity) -> i32 {
    (intensity.value() * 255.0).round() as i32
}

/// Check if haptic feedback is available (requires context).
pub fn is_available_with_context(env: &mut JNIEnv, context: &JObject) -> Result<bool, HapticError> {
    init_with_context(env, context)?;

    let helper: JClass = get_helper_class(env)?.into();

    let result = env
        .call_static_method(
            helper,
            "isAvailable",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(context)],
        )
        .map_err(|e| HapticError::Platform(format!("isAvailable call failed: {e}")))?
        .z()
        .map_err(|e| HapticError::Platform(format!("isAvailable result: {e}")))?;

    Ok(result)
}

/// Trigger impact feedback with context.
pub fn impact_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    intensity: Intensity,
) -> Result<(), HapticError> {
    init_with_context(env, context)?;

    let helper: JClass = get_helper_class(env)?.into();

    env.call_static_method(
        helper,
        "impact",
        "(Landroid/content/Context;F)V",
        &[JValue::Object(context), JValue::Float(intensity.value())],
    )
    .map_err(|e| HapticError::Platform(format!("impact call failed: {e}")))?;

    Ok(())
}

/// Trigger selection feedback with context.
pub fn selection_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), HapticError> {
    init_with_context(env, context)?;

    let helper: JClass = get_helper_class(env)?.into();

    env.call_static_method(
        helper,
        "selection",
        "(Landroid/content/Context;)V",
        &[JValue::Object(context)],
    )
    .map_err(|e| HapticError::Platform(format!("selection call failed: {e}")))?;

    Ok(())
}

/// Trigger notification feedback with context.
pub fn notification_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    notification_type: i32,
) -> Result<(), HapticError> {
    init_with_context(env, context)?;

    let helper: JClass = get_helper_class(env)?.into();

    env.call_static_method(
        helper,
        "notification",
        "(Landroid/content/Context;I)V",
        &[JValue::Object(context), JValue::Int(notification_type)],
    )
    .map_err(|e| HapticError::Platform(format!("notification call failed: {e}")))?;

    Ok(())
}

/// Play a custom haptic pattern with context.
pub fn play_pattern_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    pattern: &HapticPattern,
) -> Result<(), HapticError> {
    init_with_context(env, context)?;

    // Convert pattern to timings and amplitudes arrays
    let mut timings = Vec::with_capacity(pattern.steps().len());
    let mut amplitudes = Vec::with_capacity(pattern.steps().len());

    for step in pattern.steps() {
        match step {
            HapticStep::Vibrate {
                duration,
                intensity,
            } => {
                timings.push(duration_millis_i64(*duration)?);
                amplitudes.push(amplitude_i32(*intensity));
            }
            HapticStep::Pause(duration) => {
                timings.push(duration_millis_i64(*duration)?);
                amplitudes.push(0);
            }
        }
    }

    let helper: JClass = get_helper_class(env)?.into();
    let timings_len = i32::try_from(timings.len()).map_err(|_| {
        HapticError::Platform(format!(
            "haptic timing count exceeds i32: {}",
            timings.len()
        ))
    })?;
    let amplitudes_len = i32::try_from(amplitudes.len()).map_err(|_| {
        HapticError::Platform(format!(
            "haptic amplitude count exceeds i32: {}",
            amplitudes.len()
        ))
    })?;

    // Create Java long[] for timings
    let timings_array = env
        .new_long_array(timings_len)
        .map_err(|e| HapticError::Platform(format!("new_long_array: {e}")))?;
    env.set_long_array_region(&timings_array, 0, &timings)
        .map_err(|e| HapticError::Platform(format!("set_long_array_region: {e}")))?;

    // Create Java int[] for amplitudes
    let amplitudes_array = env
        .new_int_array(amplitudes_len)
        .map_err(|e| HapticError::Platform(format!("new_int_array: {e}")))?;
    env.set_int_array_region(&amplitudes_array, 0, &amplitudes)
        .map_err(|e| HapticError::Platform(format!("set_int_array_region: {e}")))?;

    let result = env
        .call_static_method(
            helper,
            "playPattern",
            "(Landroid/content/Context;[J[I)Z",
            &[
                JValue::Object(context),
                JValue::Object(&timings_array),
                JValue::Object(&amplitudes_array),
            ],
        )
        .map_err(|e| HapticError::Platform(format!("playPattern call failed: {e}")))?
        .z()
        .map_err(|e| HapticError::Platform(format!("playPattern result: {e}")))?;

    if result {
        Ok(())
    } else {
        Err(HapticError::Platform("pattern playback failed".into()))
    }
}

fn with_android_context<T, F>(f: F) -> Result<T, HapticError>
where
    F: for<'local> FnOnce(&mut JNIEnv<'local>, &JObject<'local>) -> Result<T, HapticError>,
{
    let android_context = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(android_context.vm().cast()) }
        .map_err(|e| HapticError::Platform(format!("JavaVM::from_raw failed: {e}")))?;

    let mut env = vm
        .attach_current_thread()
        .map_err(|e| HapticError::Platform(format!("attach_current_thread failed: {e}")))?;

    let context = ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
    assert!(
        !context.is_null(),
        "waterkit-haptic: ndk_context returned null Android Context"
    );

    f(&mut env, &context)
}

pub fn is_available() -> bool {
    with_android_context(is_available_with_context).unwrap_or_else(|error| {
        panic!("waterkit-haptic: failed to query availability with Android context: {error}")
    })
}

pub fn impact(intensity: Intensity) -> Result<(), HapticError> {
    with_android_context(|env, context| impact_with_context(env, context, intensity))
}

pub fn selection() -> Result<(), HapticError> {
    with_android_context(selection_with_context)
}

pub fn notification_success() -> Result<(), HapticError> {
    with_android_context(|env, context| notification_with_context(env, context, 0))
}

pub fn notification_warning() -> Result<(), HapticError> {
    with_android_context(|env, context| notification_with_context(env, context, 1))
}

pub fn notification_error() -> Result<(), HapticError> {
    with_android_context(|env, context| notification_with_context(env, context, 2))
}

pub fn play_pattern(pattern: &HapticPattern) -> Result<(), HapticError> {
    with_android_context(|env, context| play_pattern_with_context(env, context, pattern))
}
