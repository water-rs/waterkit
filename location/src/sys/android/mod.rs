//! Android location implementation using JNI.

use crate::{Location, LocationError};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;
use std::sync::OnceLock;

/// Embedded DEX bytecode containing LocationHelper class.
/// Generated at build time by kotlinc + D8.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Initialize the DEX class loader. Must be called with a valid Context.
///
/// # Safety
/// The `context` must be a valid Android Context JObject.
pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), LocationError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Write DEX to cache directory
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|e| LocationError::Unknown(format!("getCacheDir failed: {e}")))?
        .l()
        .map_err(|e| LocationError::Unknown(format!("getCacheDir result: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| LocationError::Unknown(format!("getAbsolutePath failed: {e}")))?
        .l()
        .map_err(|e| LocationError::Unknown(format!("getAbsolutePath result: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_location.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| LocationError::Unknown(format!("get_string failed: {e}")))?
            .to_str()
            .map_err(|e| LocationError::Unknown(format!("to_str failed: {e}")))?
    );

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| LocationError::Unknown(format!("write DEX failed: {e}")))?;

    // Create DexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| LocationError::Unknown(format!("new_string failed: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| LocationError::Unknown(format!("getClassLoader failed: {e}")))?
        .l()
        .map_err(|e| LocationError::Unknown(format!("getClassLoader result: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| LocationError::Unknown(format!("find DexClassLoader: {e}")))?;

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
        .map_err(|e| LocationError::Unknown(format!("new DexClassLoader: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| LocationError::Unknown(format!("new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

/// Get location using the Context.
pub fn get_location_with_context(
    env: &mut JNIEnv,
    context: &JObject,
) -> Result<Location, LocationError> {
    init_with_context(env, context)?;

    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| LocationError::Unknown("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.location.LocationHelper")
        .map_err(|e| LocationError::Unknown(format!("new_string: {e}")))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .map_err(|e| LocationError::Unknown(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| LocationError::Unknown(format!("loadClass result: {e}")))?;

    let result = env
        .call_static_method(
            (&helper_class).into(),
            "getLastKnownLocation",
            "(Landroid/content/Context;)[D",
            &[JValue::Object(context)],
        )
        .map_err(|e| LocationError::Unknown(format!("getLastKnownLocation: {e}")))?
        .l()
        .map_err(|e| LocationError::Unknown(format!("getLastKnownLocation result: {e}")))?;

    // Parse double array result
    let array = env
        .get_double_array_elements(result.as_raw().cast(), jni::objects::ReleaseMode::NoCopyBack)
        .map_err(|e| LocationError::Unknown(format!("get_double_array: {e}")))?;

    let len = array.len();
    if len < 1 {
        return Err(LocationError::NotAvailable);
    }

    let success = unsafe { *array.as_ptr() };
    if success < 0.5 {
        return Err(LocationError::NotAvailable);
    }

    if len < 6 {
        return Err(LocationError::Unknown("Invalid result array".into()));
    }

    unsafe {
        let ptr = array.as_ptr();
        Ok(Location {
            latitude: *ptr.add(1),
            longitude: *ptr.add(2),
            altitude: Some(*ptr.add(3)),
            horizontal_accuracy: Some(*ptr.add(4)),
            vertical_accuracy: None,
            timestamp: *ptr.add(5) as u64,
        })
    }
}

// Async wrapper for the public API (requires runtime context)
pub(crate) async fn get_location() -> Result<Location, LocationError> {
    // Without JNI context, we can't get location
    // The application must call get_location_with_context directly
    Err(LocationError::Unknown(
        "Android: use get_location_with_context() with Context".into(),
    ))
}
