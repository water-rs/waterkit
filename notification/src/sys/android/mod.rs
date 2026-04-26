//! Android notification implementation using JNI.

use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use std::mem::ManuallyDrop;
use std::sync::OnceLock;

use crate::{InterruptionLevel, Notification, NotificationError};

/// Embedded DEX bytecode containing `NotificationHelper` class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

fn dispatch_action(notification_id: String, action_url: String) {
    let _ = (notification_id, action_url);
}

/// Handle to a shown notification (Android).
#[derive(Debug)]
pub struct NotificationHandleInner;

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_notification_NotificationHelper_onAction<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    notification_id: JString<'local>,
    action_url: JString<'local>,
) {
    let notification_id = env
        .get_string(&notification_id)
        .unwrap_or_else(|error| {
            panic!("waterkit-notification: decode notification id failed: {error}")
        })
        .to_str()
        .unwrap_or_else(|error| {
            panic!("waterkit-notification: invalid UTF-8 notification id: {error}")
        })
        .to_owned();
    let action_url = env
        .get_string(&action_url)
        .unwrap_or_else(|error| panic!("waterkit-notification: decode action url failed: {error}"))
        .to_str()
        .unwrap_or_else(|error| panic!("waterkit-notification: invalid UTF-8 action url: {error}"))
        .to_owned();
    dispatch_action(notification_id, action_url);
}

/// Initialize the DEX class loader. Must be called with a valid Context.
fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), NotificationError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Write DEX to cache directory
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|e| NotificationError::Platform(format!("getCacheDir failed: {e}")))?
        .l()
        .map_err(|e| NotificationError::Platform(format!("getCacheDir result: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| NotificationError::Platform(format!("getAbsolutePath failed: {e}")))?
        .l()
        .map_err(|e| NotificationError::Platform(format!("getAbsolutePath result: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_notification.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| NotificationError::Platform(format!("get_string failed: {e}")))?
            .to_str()
            .map_err(|e| NotificationError::Platform(format!("to_str failed: {e}")))?
    );

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| NotificationError::Platform(format!("write DEX failed: {e}")))?;

    // Create DexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| NotificationError::Platform(format!("new_string failed: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| NotificationError::Platform(format!("getClassLoader failed: {e}")))?
        .l()
        .map_err(|e| NotificationError::Platform(format!("getClassLoader result: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| NotificationError::Platform(format!("find DexClassLoader: {e}")))?;

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
        .map_err(|e| NotificationError::Platform(format!("new DexClassLoader: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| NotificationError::Platform(format!("new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

/// Show a notification with an Android context.
pub fn show_notification_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    notification: &Notification,
) -> Result<NotificationHandleInner, NotificationError> {
    init_with_context(env, context)?;

    let class_loader = CLASS_LOADER
        .get()
        .ok_or(NotificationError::ServiceUnavailable)?;

    let helper_class_name = env
        .new_string("waterkit.notification.NotificationHelper")
        .map_err(|e| NotificationError::Platform(format!("new_string: {e}")))?;

    let loaded_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .map_err(|e| NotificationError::Platform(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| NotificationError::Platform(format!("loadClass result: {e}")))?;

    let helper_jclass: jni::objects::JClass = loaded_class.into();

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

    // Serialize actions as JSON: [{"label": "...", "url": "..."}, ...]
    let actions_json = if notification.actions.is_empty() {
        String::new()
    } else {
        let actions: Vec<String> = notification
            .actions
            .iter()
            .map(|a| format!(r#"{{"label":"{}","url":"{}"}}"#, a.label, a.url))
            .collect();
        format!("[{}]", actions.join(","))
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
        helper_jclass,
        "showNotification",
        "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;ILjava/lang/String;Ljava/lang/String;)V",
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
pub fn show_notification(
    notification: &Notification,
) -> Result<NotificationHandleInner, NotificationError> {
    let android_ctx = ndk_context::android_context();
    let vm = unsafe {
        jni::JavaVM::from_raw(android_ctx.vm().cast())
            .expect("waterkit-notification: ndk_context did not provide a valid JavaVM")
    };

    let context = ManuallyDrop::new(unsafe { JObject::from_raw(android_ctx.context().cast()) });
    assert!(
        !context.is_null(),
        "waterkit-notification: ndk_context returned a null Context"
    );

    let mut env = vm
        .attach_current_thread()
        .map_err(|e| NotificationError::Platform(format!("attach_current_thread failed: {e}")))?;

    show_notification_with_context(&mut env, &context, notification)
}
