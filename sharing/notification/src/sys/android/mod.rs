//! Android notification implementation using JNI.

use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::{Env, EnvUnowned, JavaVM, jni_sig, jni_str};
use serde::Serialize;

use crate::{InterruptionLevel, Notification, NotificationError};
use waterkit_build::{AndroidError, DexHelper, dex_helper};

/// `waterkit.notification.NotificationHelper`, embedded as a DEX by this crate's build script and
/// loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.notification.NotificationHelper");

impl From<AndroidError> for NotificationError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

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
    let helper = HELPER.class(env, context)?;

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
