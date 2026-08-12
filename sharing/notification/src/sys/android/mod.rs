//! Android notification implementation using JNI.

use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JClass, JObject, JString, JValue};
use jni::{Env, EnvUnowned, JavaVM, jni_sig, jni_str};
use serde::Serialize;
use std::sync::OnceLock;

use crate::{InterruptionLevel, Notification, NotificationError};

/// Embedded DEX bytecode containing `NotificationHelper` class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// `waterkit.notification.NotificationHelper`, loaded once from [`DEX_BYTES`].
///
/// A loaded class keeps its defining class loader alive, so caching the class is
/// enough to keep the DEX resident and lets every later call skip the load.
static HELPER_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// One entry of the JSON array the Kotlin helper parses into notification actions.
#[derive(Serialize)]
struct ActionPayload<'a> {
    label: &'a str,
    url: &'a str,
}

fn dispatch_action(notification_id: String, action_url: String) {
    let _ = (notification_id, action_url);
}

/// Handle to a shown notification (Android).
#[derive(Debug)]
pub struct NotificationHandleInner;

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_notification_NotificationHelper_onAction<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    notification_id: JString<'local>,
    action_url: JString<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let notification_id = notification_id.try_to_string(env)?;
        let action_url = action_url.try_to_string(env)?;
        dispatch_action(notification_id, action_url);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

/// Returns the cached `NotificationHelper` class, loading the embedded DEX on
/// first use.
fn helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<&'static Global<JClass<'static>>, NotificationError> {
    if let Some(class) = HELPER_CLASS.get() {
        return Ok(class);
    }

    let class = load_helper_class(env, context)?;
    Ok(HELPER_CLASS.get_or_init(|| class))
}

/// Loads `waterkit.notification.NotificationHelper` from the embedded DEX.
fn load_helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Global<JClass<'static>>, NotificationError> {
    let parent_loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )
        .map_err(|e| NotificationError::Platform(format!("getClassLoader failed: {e}")))?
        .l()
        .map_err(|e| NotificationError::Platform(format!("getClassLoader result: {e}")))?;

    let dex_bytes = env
        .byte_array_from_slice(DEX_BYTES)
        .map_err(|e| NotificationError::Platform(format!("copying the DEX failed: {e}")))?;
    let dex_buffer = env
        .call_static_method(
            jni_str!("java/nio/ByteBuffer"),
            jni_str!("wrap"),
            jni_sig!("([B)Ljava/nio/ByteBuffer;"),
            &[JValue::Object(&dex_bytes)],
        )
        .map_err(|e| NotificationError::Platform(format!("wrapping the DEX failed: {e}")))?
        .l()
        .map_err(|e| NotificationError::Platform(format!("wrapping the DEX result: {e}")))?;
    let class_loader = env
        .new_object(
            jni_str!("dalvik/system/InMemoryDexClassLoader"),
            jni_sig!("(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V"),
            &[JValue::Object(&dex_buffer), JValue::Object(&parent_loader)],
        )
        .map_err(|e| {
            NotificationError::Platform(format!("constructing InMemoryDexClassLoader: {e}"))
        })?;

    let class_name = env
        .new_string("waterkit.notification.NotificationHelper")
        .map_err(|e| NotificationError::Platform(format!("new_string: {e}")))?;
    let class = env
        .call_method(
            &class_loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&class_name)],
        )
        .map_err(|e| NotificationError::Platform(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| NotificationError::Platform(format!("loadClass result: {e}")))?;
    let class = env
        .cast_local::<JClass>(class)
        .map_err(|e| NotificationError::Platform(format!("loadClass returned a non-class: {e}")))?;

    env.new_global_ref(class)
        .map_err(|e| NotificationError::Platform(format!("new_global_ref: {e}")))
}

/// Show a notification with an Android context.
///
/// # Errors
///
/// Returns [`NotificationError::Platform`] if the helper class cannot be loaded
/// or the notification cannot be posted.
pub fn show_notification_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    notification: &Notification,
) -> Result<NotificationHandleInner, NotificationError> {
    let helper = helper_class(env, context)?;

    let jtitle = env
        .new_string(&notification.title)
        .map_err(|e| NotificationError::Platform(format!("new_string: {e}")))?;
    let jbody = env
        .new_string(&notification.body)
        .map_err(|e| NotificationError::Platform(format!("new_string: {e}")))?;

    // Map InterruptionLevel to Android importance
    let importance: i32 = match notification.interruption_level {
        InterruptionLevel::Passive => 2,       // IMPORTANCE_LOW
        InterruptionLevel::Active => 3,        // IMPORTANCE_DEFAULT
        InterruptionLevel::TimeSensitive => 4, // IMPORTANCE_HIGH
        InterruptionLevel::Critical => 5,      // IMPORTANCE_MAX
    };

    // The Kotlin helper treats an empty string as "no actions" and otherwise
    // parses a JSON array.
    let actions_json = if notification.actions.is_empty() {
        String::new()
    } else {
        let actions: Vec<ActionPayload<'_>> = notification
            .actions
            .iter()
            .map(|action| ActionPayload {
                label: &action.label,
                url: &action.url,
            })
            .collect();
        serde_json::to_string(&actions).map_err(|e| {
            NotificationError::Platform(format!("serializing notification actions failed: {e}"))
        })?
    };

    let jactions = env
        .new_string(&actions_json)
        .map_err(|e| NotificationError::Platform(format!("new_string: {e}")))?;
    let notification_id = notification.id.as_deref().ok_or_else(|| {
        NotificationError::Platform("notification id missing in Android backend".into())
    })?;
    let jnotification_id = env
        .new_string(notification_id)
        .map_err(|e| NotificationError::Platform(format!("new_string: {e}")))?;

    env.call_static_method(
        helper,
        jni_str!("showNotification"),
        jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;ILjava/lang/String;Ljava/lang/String;)V"),
        &[
            JValue::Object(context),
            JValue::Object(&jtitle),
            JValue::Object(&jbody),
            JValue::Int(importance),
            JValue::Object(&jactions),
            JValue::Object(&jnotification_id),
        ],
    )
    .map_err(|e| NotificationError::Platform(format!("showNotification call failed: {e}")))?;

    Ok(NotificationHandleInner)
}

/// Show a notification by resolving Android context via `ndk_context`.
///
/// # Errors
///
/// Returns [`NotificationError::Platform`] if the JVM cannot be attached or the
/// notification cannot be posted.
///
/// # Panics
///
/// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet.
pub fn show_notification(
    notification: &Notification,
) -> Result<NotificationHandleInner, NotificationError> {
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    assert!(
        !raw_vm.is_null(),
        "waterkit-notification: ndk_context returned a null JavaVM"
    );
    assert!(
        !raw_context.is_null(),
        "waterkit-notification: ndk_context returned a null Android Context"
    );

    // SAFETY: `ndk_context` publishes the process' JavaVM pointer, which stays
    // valid for the lifetime of the application.
    let vm = unsafe { JavaVM::from_raw(raw_vm) };
    vm.attach_current_thread(
        |env| -> Result<Result<NotificationHandleInner, NotificationError>, jni::errors::Error> {
            // SAFETY: `ndk_context` publishes a global reference to the
            // application `Context` that outlives this attachment, and
            // `as_cast_raw` only borrows it.
            let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
            Ok(show_notification_with_context(env, &context, notification))
        },
    )
    .map_err(|e| NotificationError::Platform(format!("attach_current_thread failed: {e}")))?
}
