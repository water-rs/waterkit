//! Android permission implementation using JNI.
//!
//! The async APIs use `ndk-context` to obtain the current Activity automatically.
//! For advanced JNI integration, `*_with_activity` APIs are also available.

use crate::{Permission, PermissionError, PermissionStatus};
use jni::objects::{JObject, JValue};
use jni::sys::jint;
use jni::{Env, JavaVM, jni_sig, jni_str};
use waterkit_build::{DexHelper, dex_helper};

/// `waterkit.permission.PermissionHelper`, compiled into the app's DEX by the
/// packager and resolved through the application's `ClassLoader`.
static HELPER: DexHelper = dex_helper!("waterkit.permission.PermissionHelper");

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

    let helper_class = HELPER
        .class(env, activity)
        .map_err(|e| PermissionError::Platform(e.to_string()))?;

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

    let helper_class = HELPER
        .class(env, activity)
        .map_err(|e| PermissionError::Platform(e.to_string()))?;
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
