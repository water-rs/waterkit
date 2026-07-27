//! Android permission implementation using JNI.
//!
//! The async APIs use `ndk-context` to obtain the current Activity automatically.
//! For advanced JNI integration, `*_with_activity` APIs are also available.

use crate::{Permission, PermissionError, PermissionStatus};
use jni::objects::{Global, JClass, JObject, JString, JValue};
use jni::sys::jint;
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::sync::OnceLock;

/// Embedded DEX bytecode containing `PermissionHelper` class.
/// Generated at build time by kotlinc + D8.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<Global<JObject<'static>>> = OnceLock::new();

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

const fn permission_to_jint(permission: Permission) -> Option<jint> {
    Some(match permission {
        Permission::Location | Permission::LocationWhenInUse | Permission::LocationAlways => {
            PERMISSION_LOCATION
        }
        Permission::Camera => PERMISSION_CAMERA,
        Permission::Microphone => PERMISSION_MICROPHONE,
        Permission::Photos => PERMISSION_PHOTOS,
        Permission::Contacts => PERMISSION_CONTACTS,
        Permission::Calendar => PERMISSION_CALENDAR,
        // Reminders, Bluetooth*, Nfc, Notification, SpeechRecognition,
        // Tracking, MediaLibrary, BodySensors, HealthRead/Write — not yet
        // bridged through PermissionHelper.kt; falls through with `None`.
        // The wildcard also catches any future `Permission` variants
        // added to waterkit-core before they are wired up.
        _ => return None,
    })
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
/// Returns a `PermissionError::Platform` if DEX loading or class loader creation fails.
pub fn init_with_activity(env: &mut Env<'_>, activity: &JObject) -> Result<(), PermissionError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Write DEX to cache directory
    let context = activity;
    let cache_dir = env
        .call_method(
            context,
            jni_str!("getCacheDir"),
            jni_sig!("()Ljava/io/File;"),
            &[],
        )
        .map_err(|e| PermissionError::Platform(format!("getCacheDir failed: {e}")))?
        .l()
        .map_err(|e| PermissionError::Platform(format!("getCacheDir result: {e}")))?;

    let cache_path = env
        .call_method(
            &cache_dir,
            jni_str!("getAbsolutePath"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .map_err(|e| PermissionError::Platform(format!("getAbsolutePath failed: {e}")))?
        .l()
        .map_err(|e| PermissionError::Platform(format!("getAbsolutePath result: {e}")))?;

    let cache_path_string = env
        .as_cast::<JString>(&cache_path)
        .and_then(|path| path.try_to_string(env))
        .map_err(|e| PermissionError::Platform(format!("decode cache path failed: {e}")))?;
    let dex_path = format!("{cache_path_string}/waterkit_permission.dex");

    // Remove if exists to handle previous read-only setting
    match std::fs::remove_file(&dex_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(PermissionError::Platform(format!(
                "remove stale DEX failed: {e}"
            )));
        }
    }

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| PermissionError::Platform(format!("write DEX failed: {e}")))?;

    // Make DEX read-only as required by modern Android security
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dex_path)
            .map_err(|e| PermissionError::Platform(format!("metadata DEX failed: {e}")))?
            .permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&dex_path, perms)
            .map_err(|e| PermissionError::Platform(format!("set_permissions DEX failed: {e}")))?;
    }

    // Create InMemoryDexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| PermissionError::Platform(format!("new_string failed: {e}")))?;

    let parent_loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )
        .map_err(|e| PermissionError::Platform(format!("getClassLoader failed: {e}")))?
        .l()
        .map_err(|e| PermissionError::Platform(format!("getClassLoader result: {e}")))?;

    let dex_class_loader_class = env
        .find_class(jni_str!("dalvik/system/DexClassLoader"))
        .map_err(|e| PermissionError::Platform(format!("find DexClassLoader: {e}")))?;

    let class_loader = env
        .new_object(
            dex_class_loader_class,
            jni_sig!(
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V"
            ),
            &[
                JValue::Object(&dex_path_jstring),
                JValue::Object(&cache_path),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|e| PermissionError::Platform(format!("new DexClassLoader: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| PermissionError::Platform(format!("new_global_ref: {e}")))?;

    if CLASS_LOADER.set(global_ref).is_err() {
        debug_assert!(
            CLASS_LOADER.get().is_some(),
            "Class loader set failed but loader is still uninitialized"
        );
    }
    Ok(())
}

fn get_helper_class<'local>(env: &mut Env<'local>) -> Result<JClass<'local>, PermissionError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| PermissionError::Platform("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.permission.PermissionHelper")
        .map_err(|e| PermissionError::Platform(format!("new_string: {e}")))?;

    let loaded_class = env
        .call_method(
            class_loader.as_obj(),
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&helper_class_name)],
        )
        .map_err(|e| PermissionError::Platform(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| PermissionError::Platform(format!("loadClass result: {e}")))?;

    env.cast_local::<JClass>(loaded_class)
        .map_err(|error| PermissionError::Platform(error.to_string()))
}

#[derive(Debug)]
struct AttachedPermissionError(PermissionError);

impl std::fmt::Display for AttachedPermissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for AttachedPermissionError {}

impl From<jni::errors::Error> for AttachedPermissionError {
    fn from(error: jni::errors::Error) -> Self {
        Self(PermissionError::Platform(error.to_string()))
    }
}

fn with_activity<T>(
    op: impl FnOnce(&mut Env<'_>, &JObject) -> Result<T, PermissionError>,
) -> Result<T, PermissionError> {
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_activity: jni::sys::jobject = android_context.context().cast();
    if raw_vm.is_null() {
        return Err(PermissionError::Platform(
            "Android JavaVM is null from ndk_context".into(),
        ));
    }
    if raw_activity.is_null() {
        return Err(PermissionError::Platform(
            "Android Activity is null from ndk_context".into(),
        ));
    }

    let vm = unsafe { JavaVM::from_raw(raw_vm) };
    vm.attach_current_thread(|env| -> Result<T, AttachedPermissionError> {
        let activity = unsafe { env.as_cast_raw::<JObject>(&raw_activity)? };
        op(env, &activity).map_err(AttachedPermissionError)
    })
    .map_err(|error| error.0)
}

/// Check permission using the Activity context.
///
/// # Errors
///
/// Returns [`PermissionError::Unsupported`] if the permission has no
/// Android mapping; [`PermissionError::Platform`] if a JNI call fails.
pub fn check_with_activity(
    env: &mut Env<'_>,
    activity: &JObject,
    permission: Permission,
) -> Result<PermissionStatus, PermissionError> {
    let permission_type = permission_to_jint(permission).ok_or(PermissionError::Unsupported)?;

    init_with_activity(env, activity)?;
    let helper_class = get_helper_class(env)?;

    let result = env
        .call_static_method(
            helper_class,
            jni_str!("checkPermission"),
            jni_sig!("(Landroid/app/Activity;I)I"),
            &[JValue::Object(activity), JValue::Int(permission_type)],
        )
        .map_err(|e| PermissionError::Platform(format!("checkPermission: {e}")))?
        .i()
        .map_err(|e| PermissionError::Platform(format!("checkPermission result: {e}")))?;

    Ok(status_from_jint(result))
}

/// Request permission using the Activity context.
///
/// This only starts the Android runtime permission flow. The final result
/// is delivered asynchronously to the host Activity callback.
///
/// # Errors
///
/// Returns [`PermissionError::Unsupported`] if the permission has no
/// Android mapping; [`PermissionError::Platform`] if a JNI call fails.
pub fn request_with_activity(
    env: &mut Env<'_>,
    activity: &JObject,
    permission: Permission,
) -> Result<(), PermissionError> {
    let permission_type = permission_to_jint(permission).ok_or(PermissionError::Unsupported)?;

    init_with_activity(env, activity)?;
    let helper_class = get_helper_class(env)?;
    let request_code = REQUEST_CODE_BASE + permission_type;

    env.call_static_method(
        helper_class,
        jni_str!("requestPermission"),
        jni_sig!("(Landroid/app/Activity;II)V"),
        &[
            JValue::Object(activity),
            JValue::Int(permission_type),
            JValue::Int(request_code),
        ],
    )
    .map_err(|e| PermissionError::Platform(format!("requestPermission: {e}")))?;

    Ok(())
}

// Async wrappers for the public API (use ndk-context).
pub async fn check(permission: Permission) -> PermissionStatus {
    with_activity(|env, activity| check_with_activity(env, activity, permission))
        .unwrap_or(PermissionStatus::NotDetermined)
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
