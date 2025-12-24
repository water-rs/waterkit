//! Android haptic implementation using JNI.

use crate::{HapticError, HapticFeedback};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use std::sync::OnceLock;

/// Embedded DEX bytecode containing HapticHelper class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

// Haptic style constants matching Kotlin side
const STYLE_LIGHT: i32 = 0;
const STYLE_MEDIUM: i32 = 1;
const STYLE_HEAVY: i32 = 2;
const STYLE_RIGID: i32 = 3;
const STYLE_SOFT: i32 = 4;
const STYLE_SELECTION: i32 = 5;
const STYLE_SUCCESS: i32 = 6;
const STYLE_WARNING: i32 = 7;
const STYLE_ERROR: i32 = 8;

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

/// Trigger haptic feedback using the Context.
pub fn feedback_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    style: HapticFeedback,
) -> Result<(), HapticError> {
    init_with_context(env, context)?;

    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| HapticError::Unknown("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.haptic.HapticHelper")
        .map_err(|e| HapticError::Unknown(format!("new_string: {e}")))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .map_err(|e| HapticError::Unknown(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| HapticError::Unknown(format!("loadClass result: {e}")))?;

    let style_id = match style {
        HapticFeedback::Light => STYLE_LIGHT,
        HapticFeedback::Medium => STYLE_MEDIUM,
        HapticFeedback::Heavy => STYLE_HEAVY,
        HapticFeedback::Rigid => STYLE_RIGID,
        HapticFeedback::Soft => STYLE_SOFT,
        HapticFeedback::Selection => STYLE_SELECTION,
        HapticFeedback::Success => STYLE_SUCCESS,
        HapticFeedback::Warning => STYLE_WARNING,
        HapticFeedback::Error => STYLE_ERROR,
    };

    let helper_jclass: jni::objects::JClass = helper_class.into();
    env.call_static_method(
        helper_jclass,
        "feedback",
        "(Landroid/content/Context;I)V",
        &[JValue::Object(context), JValue::Int(style_id)],
    )
    .map_err(|e| HapticError::Unknown(format!("feedback call failed: {e}")))?;

    Ok(())
}

// Async wrapper for the public API (stub)
pub(crate) async fn feedback(_style: HapticFeedback) -> Result<(), HapticError> {
    Err(HapticError::Unknown(
        "Android: use feedback_with_context() with Context".into(),
    ))
}
