//! Android dialog implementation using JNI.
//!
//! For Android, the async functions return errors since they lack JNI context.
//! Use the `*_with_context` functions with a valid `JNIEnv` and Context.

use crate::{Dialog, DialogError};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use std::sync::OnceLock;

/// Embedded DEX bytecode containing `DialogHelper` class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Opaque handle to a selected media item (URI string).
#[derive(Debug, Clone)]
pub struct Selection(pub String);

/// Initialize the DEX class loader. Must be called with a valid Context.
///
/// # Errors
/// Returns an error if JNI operations fail.
pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), DialogError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| DialogError::PlatformError(format!("getCacheDir: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| DialogError::PlatformError(format!("getAbsolutePath: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_dialog.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| DialogError::PlatformError(format!("get_string: {e}")))?
            .to_str()
            .map_err(|e| DialogError::PlatformError(format!("to_str: {e}")))?
    );

    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| DialogError::PlatformError(format!("write DEX: {e}")))?;

    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| DialogError::PlatformError(format!("new_string: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| DialogError::PlatformError(format!("getClassLoader: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| DialogError::PlatformError(format!("find_class: {e}")))?;

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
        .map_err(|e| DialogError::PlatformError(format!("new_object: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| DialogError::PlatformError(format!("new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<jni::objects::JClass<'a>, DialogError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| DialogError::PlatformError("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.dialog.DialogHelper")
        .map_err(|e| DialogError::PlatformError(format!("new_string: {e}")))?;

    let loaded_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| DialogError::PlatformError(format!("loadClass: {e}")))?;

    Ok(loaded_class.into())
}

/// Show an alert dialog with JNI context.
///
/// # Errors
/// Returns an error if JNI operations fail.
pub fn show_alert_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    dialog: &Dialog,
) -> Result<(), DialogError> {
    init_with_context(env, context)?;

    let helper_class = get_helper_class(env)?;

    let title = env
        .new_string(&dialog.title)
        .map_err(|e| DialogError::PlatformError(e.to_string()))?;
    let message = env
        .new_string(&dialog.message)
        .map_err(|e| DialogError::PlatformError(e.to_string()))?;

    env.call_static_method(
        helper_class,
        "showDialog",
        "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V",
        &[
            JValue::Object(context),
            JValue::Object(&title),
            JValue::Object(&message),
        ],
    )
    .map_err(|e| DialogError::PlatformError(format!("showDialog: {e}")))?;

    Ok(())
}

/// Show a confirmation dialog with JNI context.
///
/// # Errors
/// Returns an error if JNI operations fail.
pub fn show_confirm_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    dialog: &Dialog,
) -> Result<bool, DialogError> {
    init_with_context(env, context)?;

    let helper_class = get_helper_class(env)?;

    let title = env
        .new_string(&dialog.title)
        .map_err(|e| DialogError::PlatformError(e.to_string()))?;
    let message = env
        .new_string(&dialog.message)
        .map_err(|e| DialogError::PlatformError(e.to_string()))?;

    let result = env
        .call_static_method(
            helper_class,
            "showConfirm",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Z",
            &[
                JValue::Object(context),
                JValue::Object(&title),
                JValue::Object(&message),
            ],
        )
        .map_err(|e| DialogError::PlatformError(format!("showConfirm: {e}")))?
        .z()
        .map_err(|e| DialogError::PlatformError(format!("return value: {e}")))?;

    Ok(result)
}

/// Show a photo picker with JNI context.
///
/// Note: The current implementation returns None as photo picking requires
/// app-level Activity integration. Use `preparePhotoPick` and `handleActivityResult`
/// from the Kotlin helper for full functionality.
///
/// # Errors
/// Returns an error if JNI operations fail.
pub fn show_photo_picker_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    picker: &crate::PhotoPicker,
) -> Result<Option<Selection>, DialogError> {
    init_with_context(env, context)?;

    let helper_class = get_helper_class(env)?;

    let type_int = match picker.media_type {
        crate::MediaType::Image | crate::MediaType::LivePhoto => 0,
        crate::MediaType::Video => 1,
    };

    let result = env
        .call_static_method(
            helper_class,
            "pickPhoto",
            "(Landroid/content/Context;I)Ljava/lang/String;",
            &[JValue::Object(context), JValue::Int(type_int)],
        )
        .map_err(|e| DialogError::PlatformError(format!("pickPhoto: {e}")))?
        .l()
        .map_err(|e| DialogError::PlatformError(format!("pickPhoto return: {e}")))?;

    if result.is_null() {
        Ok(None)
    } else {
        let uri = env
            .get_string((&result).into())
            .map_err(|e| DialogError::PlatformError(format!("get_string: {e}")))?;
        Ok(Some(Selection(uri.into())))
    }
}

/// Load media from a selection handle with JNI context.
///
/// # Errors
/// Returns an error if JNI operations fail or media loading fails.
pub fn load_media_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    handle: &Selection,
) -> Result<std::path::PathBuf, DialogError> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;

    let uri_jstr = env
        .new_string(&handle.0)
        .map_err(|e| DialogError::PlatformError(format!("new_string: {e}")))?;

    let result = env
        .call_static_method(
            helper_class,
            "loadMedia",
            "(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(context), JValue::Object(&uri_jstr)],
        )
        .map_err(|e| DialogError::PlatformError(format!("loadMedia: {e}")))?
        .l()
        .map_err(|e| DialogError::PlatformError(format!("loadMedia return: {e}")))?;

    if result.is_null() {
        Err(DialogError::PlatformError(
            "Failed to load media (returned null)".into(),
        ))
    } else {
        let path_str = env
            .get_string((&result).into())
            .map_err(|e| DialogError::PlatformError(format!("get_string path: {e}")))?;
        Ok(std::path::PathBuf::from(String::from(path_str)))
    }
}

// Public async API stubs - require JNI context

/// Show an alert dialog.
///
/// # Errors
/// Always returns an error on Android. Use `show_alert_with_context` instead.
#[allow(clippy::unused_async)]
pub async fn show_alert(_dialog: Dialog) -> Result<(), DialogError> {
    Err(DialogError::PlatformError(
        "Android: use show_alert_with_context() with JNIEnv and Context".into(),
    ))
}

/// Show a confirmation dialog.
///
/// # Errors
/// Always returns an error on Android. Use `show_confirm_with_context` instead.
#[allow(clippy::unused_async)]
pub async fn show_confirm(_dialog: Dialog) -> Result<bool, DialogError> {
    Err(DialogError::PlatformError(
        "Android: use show_confirm_with_context() with JNIEnv and Context".into(),
    ))
}

/// Show a photo picker.
///
/// # Errors
/// Always returns an error on Android. Use `show_photo_picker_with_context` instead.
#[allow(clippy::unused_async)]
pub async fn show_photo_picker(
    _picker: crate::PhotoPicker,
) -> Result<Option<Selection>, DialogError> {
    Err(DialogError::PlatformError(
        "Android: use show_photo_picker_with_context() with JNIEnv and Context".into(),
    ))
}

/// Load media from a selection handle.
///
/// # Errors
/// Always returns an error on Android. Use `load_media_with_context` instead.
#[allow(clippy::unused_async)]
pub async fn load_media(_handle: Selection) -> Result<std::path::PathBuf, DialogError> {
    Err(DialogError::PlatformError(
        "Android: use load_media_with_context() with JNIEnv and Context".into(),
    ))
}
