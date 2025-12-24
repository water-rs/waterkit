use crate::SecretError;
use jni::JNIEnv;
use jni::objects::{JObject, JString, JValue};

/// Helper to attach thread and get JNIEnv, but since our API is async and typically
/// waterkit passes context explicitly or assumes a thread-local JNI env is not available,
/// we need to follow waterkit's pattern.
///
/// However, `waterkit` modules usually expose `*_with_context` for Android.
/// The standard `set`/`get` in `lib.rs` don't take context.
///
/// This implies `waterkit-secret` for Android might need `init` or `with_context`.
/// But to fit the `SecretManager` trait-like static API using just `set(service, account, password)`,
/// we have a problem: we need a Context.
///
/// Solution: We will implement `set_with_context`, `get_with_context` here,
/// and the top-level `set`/`get` will error if called on Android without using the Android-specific API,
/// OR we rely on `ndk_context` if the app uses it.
///
/// For now, we'll implement `*_with_context` and let `lib.rs` (which calls `sys::set`) fail
/// or we try to grab a global context if one was set.
///
/// Given `waterkit` modules usually have `sys::android::function_with_context`,
/// we follow that pattern.

pub async fn set(_service: &str, _account: &str, _password: &str) -> Result<(), SecretError> {
    // On Android, we cannot simply run this without context.
    Err(SecretError::System(
        "On Android, use `waterkit_secret::android::set_with_context`".into(),
    ))
}

/// Retrieve a secret (stub, use `get_with_context`).
pub async fn get(_service: &str, _account: &str) -> Result<String, SecretError> {
    Err(SecretError::System(
        "On Android, use `waterkit_secret::android::get_with_context`".into(),
    ))
}

/// Delete a secret (stub, use `delete_with_context`).
pub async fn delete(_service: &str, _account: &str) -> Result<(), SecretError> {
    Err(SecretError::System(
        "On Android, use `waterkit_secret::android::delete_with_context`".into(),
    ))
}

/// Android-specific API
pub fn set_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    service: &str,
    account: &str,
    password: &str,
) -> Result<(), SecretError> {
    let ctx = context;

    // context.getSharedPreferences("waterkit_secrets", Context.MODE_PRIVATE)
    let prefs_name = env
        .new_string("waterkit_secrets")
        .map_err(|e| SecretError::System(e.to_string()))?;

    let prefs = env
        .call_method(
            ctx,
            "getSharedPreferences",
            "(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
            &[JValue::Object(&prefs_name), JValue::Int(0)], // MODE_PRIVATE = 0
        )
        .map_err(|e| SecretError::System(e.to_string()))?
        .l()
        .map_err(|e| SecretError::System(e.to_string()))?;

    // editor = prefs.edit()
    let editor = env
        .call_method(
            &prefs,
            "edit",
            "()Landroid/content/SharedPreferences$Editor;",
            &[],
        )
        .map_err(|e| SecretError::System(e.to_string()))?
        .l()
        .map_err(|e| SecretError::System(e.to_string()))?;

    // key = service + ":" + account
    let key_str = format!("{}:{}", service, account);
    let key = env
        .new_string(key_str)
        .map_err(|e| SecretError::System(e.to_string()))?;
    let val = env
        .new_string(password)
        .map_err(|e| SecretError::System(e.to_string()))?;

    // editor.putString(key, val)
    env.call_method(
        &editor,
        "putString",
        "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;",
        &[JValue::Object(&key), JValue::Object(&val)],
    )
    .map_err(|e| SecretError::System(e.to_string()))?;

    // editor.apply()
    env.call_method(&editor, "apply", "()V", &[])
        .map_err(|e| SecretError::System(e.to_string()))?;

    Ok(())
}

/// Retrieve a secret using Android Context.
/// Note: This implementation uses SharedPreferences which is application-private but does not use hardware-backed KeyStore.
pub fn get_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    service: &str,
    account: &str,
) -> Result<String, SecretError> {
    let prefs_name = env
        .new_string("waterkit_secrets")
        .map_err(|e| SecretError::System(e.to_string()))?;

    let prefs = env
        .call_method(
            context,
            "getSharedPreferences",
            "(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
            &[JValue::Object(&prefs_name), JValue::Int(0)],
        )
        .map_err(|e| SecretError::System(e.to_string()))?
        .l()
        .map_err(|e| SecretError::System(e.to_string()))?;

    let key_str = format!("{}:{}", service, account);
    let key = env
        .new_string(key_str)
        .map_err(|e| SecretError::System(e.to_string()))?;

    // prefs.getString(key, null)
    let val_j = env
        .call_method(
            &prefs,
            "getString",
            "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(&key), JValue::Object(&JObject::null())],
        )
        .map_err(|e| SecretError::System(e.to_string()))?;

    let val_obj = val_j.l().map_err(|e| SecretError::System(e.to_string()))?;

    if val_obj.is_null() {
        return Err(SecretError::NotFound);
    }

    let val_jstr: JString = val_obj.into();
    let val_str: String = env
        .get_string(&val_jstr)
        .map_err(|e| SecretError::System(e.to_string()))?
        .into();

    Ok(val_str)
}

/// Delete a secret using Android Context.
pub fn delete_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    service: &str,
    account: &str,
) -> Result<(), SecretError> {
    let prefs_name = env
        .new_string("waterkit_secrets")
        .map_err(|e| SecretError::System(e.to_string()))?;

    let prefs = env
        .call_method(
            context,
            "getSharedPreferences",
            "(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
            &[JValue::Object(&prefs_name), JValue::Int(0)],
        )
        .map_err(|e| SecretError::System(e.to_string()))?
        .l()
        .map_err(|e| SecretError::System(e.to_string()))?;

    let editor = env
        .call_method(
            &prefs,
            "edit",
            "()Landroid/content/SharedPreferences$Editor;",
            &[],
        )
        .map_err(|e| SecretError::System(e.to_string()))?
        .l()
        .map_err(|e| SecretError::System(e.to_string()))?;

    let key_str = format!("{}:{}", service, account);
    let key = env
        .new_string(key_str)
        .map_err(|e| SecretError::System(e.to_string()))?;

    // editor.remove(key)
    env.call_method(
        &editor,
        "remove",
        "(Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;",
        &[JValue::Object(&key)],
    )
    .map_err(|e| SecretError::System(e.to_string()))?;

    // editor.apply()
    env.call_method(&editor, "apply", "()V", &[])
        .map_err(|e| SecretError::System(e.to_string()))?;

    Ok(())
}
