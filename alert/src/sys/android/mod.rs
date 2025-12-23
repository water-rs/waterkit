//! Android alert implementation using JNI.

use crate::{Alert, AlertType};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;
use std::sync::OnceLock;

/// Embedded DEX bytecode containing AlertHelper class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Initialize the DEX class loader. Must be called with a valid Context.
pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), String> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Standard DEX loading boilerplate (same as haptic/location)
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error getCacheDir: {e}"))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error getAbsolutePath: {e}"))?;

    let dex_path = format!(
        "{}/waterkit_alert.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| format!("JNI error get_string: {e}"))?
            .to_str()
            .map_err(|e| format!("JNI error to_str: {e}"))?
    );

    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| format!("Failed to write DEX: {e}"))?;

    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| format!("JNI error new_string: {e}"))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error getClassLoader: {e}"))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| format!("JNI error find_class: {e}"))?;

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
        .map_err(|e| format!("JNI error new_object: {e}"))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| format!("JNI error new_global_ref: {e}"))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<jni::objects::JClass<'a>, String> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| "Class loader not initialized".to_string())?;

    let helper_class_name = env
        .new_string("waterkit.alert.AlertHelper")
        .map_err(|e| format!("JNI error new_string name: {e}"))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error loadClass: {e}"))?;

    Ok(helper_class.into())
}

pub fn show_alert_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    alert: &Alert,
) -> Result<(), String> {
    init_with_context(env, context)?;

    let helper_jclass = get_helper_class(env)?;
    
    let title = env.new_string(&alert.title).map_err(|e| e.to_string())?;
    let message = env.new_string(&alert.message).map_err(|e| e.to_string())?;

    env.call_static_method(
        helper_jclass,
        "showAlert",
        "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V",
        &[
            JValue::Object(context),
            JValue::Object(&title),
            JValue::Object(&message),
        ],
    )
    .map_err(|e| format!("JNI error showAlert: {e}"))?;

    Ok(())
}

pub fn show_confirm_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    alert: &Alert,
) -> Result<bool, String> {
    init_with_context(env, context)?;

    let helper_jclass = get_helper_class(env)?;
    
    let title = env.new_string(&alert.title).map_err(|e| e.to_string())?;
    let message = env.new_string(&alert.message).map_err(|e| e.to_string())?;

    let result = env.call_static_method(
        helper_jclass,
        "showConfirm",
        "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Z",
        &[
            JValue::Object(context),
            JValue::Object(&title),
            JValue::Object(&message),
        ],
    )
    .map_err(|e| format!("JNI error showConfirm: {e}"))?
    .z()
    .map_err(|e| format!("JNI error return value: {e}"))?;

    Ok(result)
}

// Public API stubs calling for context
pub async fn show_alert(_alert: Alert) -> Result<(), String> {
    Err("Android: use show_alert_with_context() with JNIEnv and Context".into())
}

pub async fn show_confirm(_alert: Alert) -> Result<bool, String> {
    Err("Android: use show_confirm_with_context() with JNIEnv and Context".into())
}
