//! Android permission implementation using JNI.

use crate::{Permission, PermissionError, PermissionStatus};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jint;
use jni::JNIEnv;
use std::sync::OnceLock;

/// Embedded DEX bytecode containing PermissionHelper class.
/// Generated at build time by kotlinc + D8.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Permission type constants (must match Kotlin).
const PERMISSION_LOCATION: jint = 0;
const PERMISSION_CAMERA: jint = 1;
const PERMISSION_MICROPHONE: jint = 2;
const PERMISSION_PHOTOS: jint = 3;
const PERMISSION_CONTACTS: jint = 4;
const PERMISSION_CALENDAR: jint = 5;

/// Status constants (must match Kotlin).
const STATUS_NOT_DETERMINED: jint = 0;
const STATUS_RESTRICTED: jint = 1;
const STATUS_DENIED: jint = 2;
const STATUS_GRANTED: jint = 3;

fn permission_to_jint(permission: Permission) -> jint {
    match permission {
        Permission::Location => PERMISSION_LOCATION,
        Permission::Camera => PERMISSION_CAMERA,
        Permission::Microphone => PERMISSION_MICROPHONE,
        Permission::Photos => PERMISSION_PHOTOS,
        Permission::Contacts => PERMISSION_CONTACTS,
        Permission::Calendar => PERMISSION_CALENDAR,
    }
}

fn status_from_jint(status: jint) -> PermissionStatus {
    match status {
        STATUS_GRANTED => PermissionStatus::Granted,
        STATUS_DENIED => PermissionStatus::Denied,
        STATUS_RESTRICTED => PermissionStatus::Restricted,
        _ => PermissionStatus::NotDetermined,
    }
}

/// Initialize the DEX class loader. Must be called with a valid Activity context.
///
/// # Safety
/// The `activity` must be a valid Android Activity JObject.
pub fn init_with_activity(env: &mut JNIEnv, activity: &JObject) -> Result<(), PermissionError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Write DEX to cache directory
    let context = activity;
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|e| PermissionError::Unknown(format!("getCacheDir failed: {e}")))?
        .l()
        .map_err(|e| PermissionError::Unknown(format!("getCacheDir result: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| PermissionError::Unknown(format!("getAbsolutePath failed: {e}")))?
        .l()
        .map_err(|e| PermissionError::Unknown(format!("getAbsolutePath result: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_permission.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| PermissionError::Unknown(format!("get_string failed: {e}")))?
            .to_str()
            .map_err(|e| PermissionError::Unknown(format!("to_str failed: {e}")))?
    );

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| PermissionError::Unknown(format!("write DEX failed: {e}")))?;

    // Create InMemoryDexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| PermissionError::Unknown(format!("new_string failed: {e}")))?;

    let parent_loader = env
        .call_method(
            context,
            "getClassLoader",
            "()Ljava/lang/ClassLoader;",
            &[],
        )
        .map_err(|e| PermissionError::Unknown(format!("getClassLoader failed: {e}")))?
        .l()
        .map_err(|e| PermissionError::Unknown(format!("getClassLoader result: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| PermissionError::Unknown(format!("find DexClassLoader: {e}")))?;

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
        .map_err(|e| PermissionError::Unknown(format!("new DexClassLoader: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| PermissionError::Unknown(format!("new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

/// Check permission using the Activity context.
pub fn check_with_activity(
    env: &mut JNIEnv,
    activity: &JObject,
    permission: Permission,
) -> Result<PermissionStatus, PermissionError> {
    init_with_activity(env, activity)?;

    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| PermissionError::Unknown("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.permission.PermissionHelper")
        .map_err(|e| PermissionError::Unknown(format!("new_string: {e}")))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .map_err(|e| PermissionError::Unknown(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| PermissionError::Unknown(format!("loadClass result: {e}")))?;

    let helper_jclass: jni::objects::JClass = helper_class.into();
    let result = env
        .call_static_method(
            helper_jclass,
            "checkPermission",
            "(Landroid/app/Activity;I)I",
            &[
                JValue::Object(activity),
                JValue::Int(permission_to_jint(permission)),
            ],
        )
        .map_err(|e| PermissionError::Unknown(format!("checkPermission: {e}")))?
        .i()
        .map_err(|e| PermissionError::Unknown(format!("checkPermission result: {e}")))?;

    Ok(status_from_jint(result))
}

// Async wrappers for the public API (require runtime context)
pub(crate) async fn check(permission: Permission) -> PermissionStatus {
    // Without JNI context, we can't check permissions
    // The application must call check_with_activity directly
    let _ = permission;
    PermissionStatus::NotDetermined
}

pub(crate) async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    // Without JNI context, we can't request permissions  
    // The application must use the Android Activity API directly
    let _ = permission;
    Err(PermissionError::Unknown(
        "Android: use check_with_activity() with Activity context".into(),
    ))
}
