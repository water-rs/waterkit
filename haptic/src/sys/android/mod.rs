//! Android haptic implementation using JNI.

use crate::{HapticError, HapticPattern, HapticStep, Intensity};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use std::sync::OnceLock;

/// Embedded DEX bytecode containing HapticHelper class.
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
        .map_err(|e| HapticError::Unknown(format!("getCacheDir failed: {e}")))?
        .l()
        .map_err(|e| HapticError::Unknown(format!("getCacheDir result: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| HapticError::Unknown(format!("getAbsolutePath failed: {e}")))?
        .l()
        .map_err(|e| HapticError::Unknown(format!("getAbsolutePath result: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_haptic.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| HapticError::Unknown(format!("get_string failed: {e}")))?
            .to_str()
            .map_err(|e| HapticError::Unknown(format!("to_str failed: {e}")))?
    );

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| HapticError::Unknown(format!("write DEX failed: {e}")))?;

    // Create DexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| HapticError::Unknown(format!("new_string failed: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| HapticError::Unknown(format!("getClassLoader failed: {e}")))?
        .l()
        .map_err(|e| HapticError::Unknown(format!("getClassLoader result: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| HapticError::Unknown(format!("find DexClassLoader: {e}")))?;

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
        .map_err(|e| HapticError::Unknown(format!("new DexClassLoader: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| HapticError::Unknown(format!("new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>, HapticError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| HapticError::InitFailed("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.haptic.HapticHelper")
        .map_err(|e| HapticError::Unknown(format!("new_string: {e}")))?;

    env.call_method(
        class_loader.as_obj(),
        "loadClass",
        "(Ljava/lang/String;)Ljava/lang/Class;",
        &[JValue::Object(&helper_class_name)],
    )
    .map_err(|e| HapticError::Unknown(format!("loadClass: {e}")))?
    .l()
    .map_err(|e| HapticError::Unknown(format!("loadClass result: {e}")))
}

/// Check if haptic feedback is available (requires context).
pub fn is_available_with_context(env: &mut JNIEnv, context: &JObject) -> Result<bool, HapticError> {
    init_with_context(env, context)?;

    let helper_class = get_helper_class(env)?;
    let helper_jclass: jni::objects::JClass = helper_class.into();

    let result = env
        .call_static_method(
            helper_jclass,
            "isAvailable",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(context)],
        )
        .map_err(|e| HapticError::Unknown(format!("isAvailable call failed: {e}")))?
        .z()
        .map_err(|e| HapticError::Unknown(format!("isAvailable result: {e}")))?;

    Ok(result)
}

/// Trigger impact feedback with context.
pub fn impact_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    intensity: Intensity,
) -> Result<(), HapticError> {
    init_with_context(env, context)?;

    let helper_class = get_helper_class(env)?;
    let helper_jclass: jni::objects::JClass = helper_class.into();

    env.call_static_method(
        helper_jclass,
        "impact",
        "(Landroid/content/Context;F)V",
        &[JValue::Object(context), JValue::Float(intensity.value())],
    )
    .map_err(|e| HapticError::Unknown(format!("impact call failed: {e}")))?;

    Ok(())
}

/// Trigger selection feedback with context.
pub fn selection_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), HapticError> {
    init_with_context(env, context)?;

    let helper_class = get_helper_class(env)?;
    let helper_jclass: jni::objects::JClass = helper_class.into();

    env.call_static_method(
        helper_jclass,
        "selection",
        "(Landroid/content/Context;)V",
        &[JValue::Object(context)],
    )
    .map_err(|e| HapticError::Unknown(format!("selection call failed: {e}")))?;

    Ok(())
}

/// Trigger notification feedback with context.
pub fn notification_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    notification_type: i32,
) -> Result<(), HapticError> {
    init_with_context(env, context)?;

    let helper_class = get_helper_class(env)?;
    let helper_jclass: jni::objects::JClass = helper_class.into();

    env.call_static_method(
        helper_jclass,
        "notification",
        "(Landroid/content/Context;I)V",
        &[JValue::Object(context), JValue::Int(notification_type)],
    )
    .map_err(|e| HapticError::Unknown(format!("notification call failed: {e}")))?;

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
                timings.push(duration.as_millis() as i64);
                amplitudes.push((intensity.value() * 255.0) as i32);
            }
            HapticStep::Pause(duration) => {
                timings.push(duration.as_millis() as i64);
                amplitudes.push(0);
            }
        }
    }

    let helper_class = get_helper_class(env)?;
    let helper_jclass: jni::objects::JClass = helper_class.into();

    // Create Java long[] for timings
    let timings_array = env
        .new_long_array(timings.len() as i32)
        .map_err(|e| HapticError::Unknown(format!("new_long_array: {e}")))?;
    env.set_long_array_region(&timings_array, 0, &timings)
        .map_err(|e| HapticError::Unknown(format!("set_long_array_region: {e}")))?;

    // Create Java int[] for amplitudes
    let amplitudes_array = env
        .new_int_array(amplitudes.len() as i32)
        .map_err(|e| HapticError::Unknown(format!("new_int_array: {e}")))?;
    env.set_int_array_region(&amplitudes_array, 0, &amplitudes)
        .map_err(|e| HapticError::Unknown(format!("set_int_array_region: {e}")))?;

    let result = env
        .call_static_method(
            helper_jclass,
            "playPattern",
            "(Landroid/content/Context;[J[I)Z",
            &[
                JValue::Object(context),
                JValue::Object(&timings_array),
                JValue::Object(&amplitudes_array),
            ],
        )
        .map_err(|e| HapticError::Unknown(format!("playPattern call failed: {e}")))?
        .z()
        .map_err(|e| HapticError::Unknown(format!("playPattern result: {e}")))?;

    if result {
        Ok(())
    } else {
        Err(HapticError::Unknown("pattern playback failed".into()))
    }
}

// Public API stubs - Android requires Context for all haptic operations
pub fn is_available() -> bool {
    // Cannot check without context, assume true
    true
}

pub fn impact(_intensity: Intensity) -> Result<(), HapticError> {
    Err(HapticError::Unknown(
        "Android: use impact_with_context() with Context".into(),
    ))
}

pub fn selection() -> Result<(), HapticError> {
    Err(HapticError::Unknown(
        "Android: use selection_with_context() with Context".into(),
    ))
}

pub fn notification_success() -> Result<(), HapticError> {
    Err(HapticError::Unknown(
        "Android: use notification_with_context() with Context".into(),
    ))
}

pub fn notification_warning() -> Result<(), HapticError> {
    Err(HapticError::Unknown(
        "Android: use notification_with_context() with Context".into(),
    ))
}

pub fn notification_error() -> Result<(), HapticError> {
    Err(HapticError::Unknown(
        "Android: use notification_with_context() with Context".into(),
    ))
}

pub fn play_pattern(_pattern: &HapticPattern) -> Result<(), HapticError> {
    Err(HapticError::Unknown(
        "Android: use play_pattern_with_context() with Context".into(),
    ))
}
