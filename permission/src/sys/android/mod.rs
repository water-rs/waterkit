//! Android permission implementation using JNI.
//!
//! The async APIs use `ndk-context` to obtain the current Activity automatically.
//! For advanced JNI integration, `*_with_activity` APIs are also available.

use crate::{Permission, PermissionError, PermissionStatus};
use jni::objects::{GlobalRef, JClass, JObject, JValue};
use jni::sys::jint;
use jni::{JNIEnv, JavaVM};
use std::sync::OnceLock;

/// Embedded DEX bytecode containing `PermissionHelper` class.
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
const STATUS_RESTRICTED: jint = 1;
const STATUS_DENIED: jint = 2;
const STATUS_GRANTED: jint = 3;
const REQUEST_CODE_BASE: jint = 0x57A0;

const fn permission_to_jint(permission: Permission) -> jint {
    match permission {
        Permission::Location => PERMISSION_LOCATION,
        Permission::Camera => PERMISSION_CAMERA,
        Permission::Microphone => PERMISSION_MICROPHONE,
        Permission::Photos => PERMISSION_PHOTOS,
        Permission::Contacts => PERMISSION_CONTACTS,
        Permission::Calendar => PERMISSION_CALENDAR,
    }
}

const fn status_from_jint(status: jint) -> PermissionStatus {
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
/// The `activity` must be a valid Android Activity `JObject`.
///
/// # Errors
/// Returns a `PermissionError::Unknown` if DEX loading or class loader creation fails.
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

    // Remove if exists to handle previous read-only setting
    match std::fs::remove_file(&dex_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(PermissionError::Unknown(format!(
                "remove stale DEX failed: {e}"
            )));
        }
    }

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| PermissionError::Unknown(format!("write DEX failed: {e}")))?;

    // Make DEX read-only as required by modern Android security
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dex_path)
            .map_err(|e| PermissionError::Unknown(format!("metadata DEX failed: {e}")))?
            .permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&dex_path, perms)
            .map_err(|e| PermissionError::Unknown(format!("set_permissions DEX failed: {e}")))?;
    }

    // Create InMemoryDexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| PermissionError::Unknown(format!("new_string failed: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
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

    if CLASS_LOADER.set(global_ref).is_err() {
        debug_assert!(
            CLASS_LOADER.get().is_some(),
            "Class loader set failed but loader is still uninitialized"
        );
    }
    Ok(())
}

fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<JClass<'a>, PermissionError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| PermissionError::Unknown("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.permission.PermissionHelper")
        .map_err(|e| PermissionError::Unknown(format!("new_string: {e}")))?;

    let loaded_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .map_err(|e| PermissionError::Unknown(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| PermissionError::Unknown(format!("loadClass result: {e}")))?;

    Ok(loaded_class.into())
}

fn with_activity<T>(
    op: impl FnOnce(&mut JNIEnv, &JObject) -> Result<T, PermissionError>,
) -> Result<T, PermissionError> {
    let android_ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android_ctx.vm().cast()) }
        .expect("waterkit-permission: ndk_context did not provide a valid JavaVM");

    let activity = unsafe { JObject::from_raw(android_ctx.context().cast()) };
    assert!(
        !activity.is_null(),
        "waterkit-permission: ndk_context returned a null Activity"
    );

    let mut env = vm
        .attach_current_thread()
        .expect("waterkit-permission: failed to attach current thread to JVM");
    let activity_global = env
        .new_global_ref(&activity)
        .expect("waterkit-permission: failed to promote Activity to global ref");

    op(&mut env, activity_global.as_obj())
}

/// Check permission using the Activity context.
///
/// # Errors
/// Returns a `PermissionError::Unknown` if JNI method calls fail.
pub fn check_with_activity(
    env: &mut JNIEnv,
    activity: &JObject,
    permission: Permission,
) -> Result<PermissionStatus, PermissionError> {
    init_with_activity(env, activity)?;
    let helper_class = get_helper_class(env)?;

    let result = env
        .call_static_method(
            helper_class,
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

/// Request permission using the Activity context.
///
/// This only starts the Android runtime permission flow. The final result is delivered
/// asynchronously to the host Activity callback.
///
/// # Errors
/// Returns a `PermissionError::Unknown` if JNI method calls fail.
pub fn request_with_activity(
    env: &mut JNIEnv,
    activity: &JObject,
    permission: Permission,
) -> Result<(), PermissionError> {
    init_with_activity(env, activity)?;
    let helper_class = get_helper_class(env)?;
    let permission_type = permission_to_jint(permission);
    let request_code = REQUEST_CODE_BASE + permission_type;

    env.call_static_method(
        helper_class,
        "requestPermission",
        "(Landroid/app/Activity;II)V",
        &[
            JValue::Object(activity),
            JValue::Int(permission_type),
            JValue::Int(request_code),
        ],
    )
    .map_err(|e| PermissionError::Unknown(format!("requestPermission: {e}")))?;

    Ok(())
}

// Async wrappers for the public API (use ndk-context).
pub async fn check(permission: Permission) -> PermissionStatus {
    with_activity(|env, activity| check_with_activity(env, activity, permission))
        .expect("waterkit-permission: Android permission check failed")
}

pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    with_activity(|env, activity| {
        let current = check_with_activity(env, activity, permission)?;
        if current == PermissionStatus::Granted {
            return Ok(PermissionStatus::Granted);
        }

        request_with_activity(env, activity, permission)?;
        Ok(PermissionStatus::NotDetermined)
    })
}
