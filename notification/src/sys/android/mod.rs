//! Android notification implementation using JNI.

use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use std::sync::OnceLock;

/// Embedded DEX bytecode containing NotificationHelper class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Initialize the DEX class loader. Must be called with a valid Context.
pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), String> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Write DEX to cache directory
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|e| format!("getCacheDir failed: {e}"))?
        .l()
        .map_err(|e| format!("getCacheDir result: {e}"))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| format!("getAbsolutePath failed: {e}"))?
        .l()
        .map_err(|e| format!("getAbsolutePath result: {e}"))?;

    let dex_path = format!(
        "{}/waterkit_notification.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| format!("get_string failed: {e}"))?
            .to_str()
            .map_err(|e| format!("to_str failed: {e}"))?
    );

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES).map_err(|e| format!("write DEX failed: {e}"))?;

    // Create DexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| format!("new_string failed: {e}"))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| format!("getClassLoader failed: {e}"))?
        .l()
        .map_err(|e| format!("getClassLoader result: {e}"))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| format!("find DexClassLoader: {e}"))?;

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
        .map_err(|e| format!("new DexClassLoader: {e}"))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| format!("new_global_ref: {e}"))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

pub fn show_notification_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    title: &str,
    body: &str,
) -> Result<(), String> {
    init_with_context(env, context)?;

    let class_loader = CLASS_LOADER.get().ok_or("Class loader not initialized")?;

    let helper_class_name = env
        .new_string("waterkit.notification.NotificationHelper")
        .map_err(|e| format!("new_string: {e}"))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .map_err(|e| format!("loadClass: {e}"))?
        .l()
        .map_err(|e| format!("loadClass result: {e}"))?;

    let helper_jclass: jni::objects::JClass = helper_class.into();

    let jtitle = env
        .new_string(title)
        .map_err(|e| format!("new_string: {e}"))?;
    let jbody = env
        .new_string(body)
        .map_err(|e| format!("new_string: {e}"))?;

    env.call_static_method(
        helper_jclass,
        "showNotification",
        "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V",
        &[
            JValue::Object(context),
            JValue::Object(&jtitle),
            JValue::Object(&jbody),
        ],
    )
    .map_err(|e| format!("showNotification call failed: {e}"))?;

    Ok(())
}

// Stub for the default trait method trying to find context or fail
pub fn show_notification(_title: &str, _body: &str) {
    eprintln!("Android notification requires generic show_with_context call.");
}
