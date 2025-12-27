use crate::{Dialog, DialogType, DialogError};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use std::sync::OnceLock;

// ... (DEX_BYTES, CLASS_LOADER, init_with_context, get_helper_class unchanged) ...
// Wait, I cannot leave them unchanged if I replace the whole file or large chunk.
// I will target specific functions or blocks.
// `init_with_context` implementation is long. I will skip editing it if possible.
// I will edit `show_alert_with_context`, `show_confirm_with_context`, `show_photo_picker_with_context`, `load_media_with_context` and stubs.

pub fn show_alert_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    dialog: &Dialog,
) -> Result<(), DialogError> {
    init_with_context(env, context).map_err(DialogError::PlatformError)?;

    let helper_jclass = get_helper_class(env).map_err(DialogError::PlatformError)?;

    let title = env.new_string(&dialog.title).map_err(|e| DialogError::PlatformError(e.to_string()))?;
    let message = env.new_string(&dialog.message).map_err(|e| DialogError::PlatformError(e.to_string()))?;

    env.call_static_method(
        helper_jclass,
        "showDialog",
        "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V",
        &[
            JValue::Object(context),
            JValue::Object(&title),
            JValue::Object(&message),
        ],
    )
    .map_err(|e| DialogError::PlatformError(format!("JNI error showDialog: {e}")))?;

    Ok(())
}

pub fn show_confirm_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    dialog: &Dialog,
) -> Result<bool, DialogError> {
    init_with_context(env, context).map_err(DialogError::PlatformError)?;

    let helper_jclass = get_helper_class(env).map_err(DialogError::PlatformError)?;

    let title = env.new_string(&dialog.title).map_err(|e| DialogError::PlatformError(e.to_string()))?;
    let message = env.new_string(&dialog.message).map_err(|e| DialogError::PlatformError(e.to_string()))?;

    let result = env
        .call_static_method(
            helper_jclass,
            "showConfirm",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Z",
            &[
                JValue::Object(context),
                JValue::Object(&title),
                JValue::Object(&message),
            ],
        )
        .map_err(|e| DialogError::PlatformError(format!("JNI error showConfirm: {e}")))?
        .z()
        .map_err(|e| DialogError::PlatformError(format!("JNI error return value: {e}")))?;

    Ok(result)
}

#[derive(Debug, Clone)]
pub struct Selection(pub String);

pub fn show_photo_picker_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    picker: &crate::PhotoPicker,
) -> Result<Option<Selection>, DialogError> {
    init_with_context(env, context).map_err(DialogError::PlatformError)?;

    let helper_jclass = get_helper_class(env).map_err(DialogError::PlatformError)?;

    let type_int = match picker.media_type {
        crate::MediaType::Image | crate::MediaType::LivePhoto => 0, // Image
        crate::MediaType::Video => 1,                               // Video
    };

    let result = env
        .call_static_method(
            helper_jclass,
            "pickPhoto",
            "(Landroid/content/Context;I)Ljava/lang/String;",
            &[JValue::Object(context), JValue::Int(type_int)],
        )
        .map_err(|e| DialogError::PlatformError(format!("JNI error pickPhoto: {e}")))?
        .l()
        .map_err(|e| DialogError::PlatformError(format!("JNI error pickPhoto return: {e}")))?;

    if result.is_null() {
        Ok(None)
    } else {
        let uri = env
            .get_string((&result).into())
            .map_err(|e| DialogError::PlatformError(format!("JNI error get_string: {e}")))?;
        Ok(Some(Selection(uri.into())))
    }
}

pub fn load_media_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    handle: Selection,
) -> Result<std::path::PathBuf, DialogError> {
    init_with_context(env, context).map_err(DialogError::PlatformError)?;
    let helper_jclass = get_helper_class(env).map_err(DialogError::PlatformError)?;

    let uri_jstr = env
        .new_string(&handle.0)
        .map_err(|e| DialogError::PlatformError(format!("JNI error new_string: {e}")))?;

    let result = env
        .call_static_method(
            helper_jclass,
            "loadMedia",
            "(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(context), JValue::Object(&uri_jstr)],
        )
        .map_err(|e| DialogError::PlatformError(format!("JNI error loadMedia: {e}")))?
        .l()
        .map_err(|e| DialogError::PlatformError(format!("JNI error loadMedia return: {e}")))?;

    if result.is_null() {
        Err(DialogError::PlatformError("Failed to load media (returned null)".to_string()))
    } else {
        let path_str = env
            .get_string((&result).into())
            .map_err(|e| DialogError::PlatformError(format!("JNI error get_string path: {e}")))?;
        Ok(std::path::PathBuf::from(String::from(path_str)))
    }
}

// Public API stubs calling for context
pub async fn show_alert(_dialog: Dialog) -> Result<(), DialogError> {
    Err(DialogError::PlatformError("Android: use show_alert_with_context() with JNIEnv and Context".into()))
}

pub async fn show_confirm(_dialog: Dialog) -> Result<bool, DialogError> {
    Err(DialogError::PlatformError("Android: use show_confirm_with_context() with JNIEnv and Context".into()))
}

pub async fn show_photo_picker(
    _picker: crate::PhotoPicker,
) -> Result<Option<Selection>, DialogError> {
    Err(DialogError::PlatformError("Android: use show_photo_picker_with_context() with JNIEnv and Context".into()))
}

pub async fn load_media(_handle: Selection) -> Result<std::path::PathBuf, DialogError> {
    Err(DialogError::PlatformError("Android: use load_media_with_context() with JNIEnv and Context".into()))
}

/// Embedded DEX bytecode containing DialogHelper class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Initialize the DEX class loader. Must be called with a valid Context.
pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), String> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Standard DEX loading boilerplate
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error getCacheDir: {e}"))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error getAbsolutePath: {e}"))?;

    let dex_path = format!(
        "{}/waterkit_dialog.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| format!("JNI error get_string: {e}"))?
            .to_str()
            .map_err(|e| format!("JNI error to_str: {e}"))?
    );

    std::fs::write(&dex_path, DEX_BYTES).map_err(|e| format!("Failed to write DEX: {e}"))?;

    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| format!("JNI error new_string: {e}"))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error getClassLoader: {e}"))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| format!("JNI error find_class: {e}"))?;

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
        .map_err(|e| format!("JNI error new_object: {e}"))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| format!("JNI error new_global_ref: {e}"))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<jni::objects::JClass<'a>, String> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| "Class loader not initialized".to_string())?;

    let helper_class_name = env
        .new_string("waterkit.dialog.DialogHelper")
        .map_err(|e| format!("JNI error new_string name: {e}"))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error loadClass: {e}"))?;

    Ok(helper_class.into())
}

pub fn show_alert_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    dialog: &Dialog,
) -> Result<(), String> {
    init_with_context(env, context)?;

    let helper_jclass = get_helper_class(env)?;

    let title = env.new_string(&dialog.title).map_err(|e| e.to_string())?;
    let message = env.new_string(&dialog.message).map_err(|e| e.to_string())?;

    env.call_static_method(
        helper_jclass,
        "showDialog",
        "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V",
        &[
            JValue::Object(context),
            JValue::Object(&title),
            JValue::Object(&message),
        ],
    )
    .map_err(|e| format!("JNI error showDialog: {e}"))?;

    Ok(())
}

pub fn show_confirm_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    dialog: &Dialog,
) -> Result<bool, String> {
    init_with_context(env, context)?;

    let helper_jclass = get_helper_class(env)?;

    let title = env.new_string(&dialog.title).map_err(|e| e.to_string())?;
    let message = env.new_string(&dialog.message).map_err(|e| e.to_string())?;

    let result = env
        .call_static_method(
            helper_jclass,
            "showConfirm",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Z",
            &[
                JValue::Object(context),
                JValue::Object(&title),
                JValue::Object(&message),
            ],
        )
        .map_err(|e| format!("JNI error showConfirm: {e}"))?
        .z()
        .map_err(|e| format!("JNI error return value: {e}"))?;


    Ok(result)
}

#[derive(Debug, Clone)]
pub struct Selection(pub String);

pub fn show_photo_picker_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    picker: &crate::PhotoPicker,
) -> Result<Option<Selection>, String> {
    init_with_context(env, context)?;

    let helper_jclass = get_helper_class(env)?;

    let type_int = match picker.media_type {
        crate::MediaType::Image | crate::MediaType::LivePhoto => 0, // Image
        crate::MediaType::Video => 1,                               // Video
    };

    let result = env
        .call_static_method(
            helper_jclass,
            "pickPhoto",
            "(Landroid/content/Context;I)Ljava/lang/String;",
            &[JValue::Object(context), JValue::Int(type_int)],
        )
        .map_err(|e| format!("JNI error pickPhoto: {e}"))?
        .l()
        .map_err(|e| format!("JNI error pickPhoto return: {e}"))?;

    if result.is_null() {
        Ok(None)
    } else {
        let uri = env
            .get_string((&result).into())
            .map_err(|e| format!("JNI error get_string: {e}"))?;
        Ok(Some(Selection(uri.into())))
    }
}

pub fn load_media_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    handle: Selection,
) -> Result<std::path::PathBuf, String> {
    init_with_context(env, context)?;
    let helper_jclass = get_helper_class(env)?;

    let uri_jstr = env
        .new_string(&handle.0)
        .map_err(|e| format!("JNI error new_string: {e}"))?;

    let result = env
        .call_static_method(
            helper_jclass,
            "loadMedia",
            "(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(context), JValue::Object(&uri_jstr)],
        )
        .map_err(|e| format!("JNI error loadMedia: {e}"))?
        .l()
        .map_err(|e| format!("JNI error loadMedia return: {e}"))?;

    if result.is_null() {
        Err("Failed to load media (returned null)".to_string())
    } else {
        let path_str = env
            .get_string((&result).into())
            .map_err(|e| format!("JNI error get_string path: {e}"))?;
        Ok(std::path::PathBuf::from(String::from(path_str)))
    }
}

// Public API stubs calling for context
pub async fn show_alert(_dialog: Dialog) -> Result<(), String> {
    Err("Android: use show_alert_with_context() with JNIEnv and Context".into())
}

pub async fn show_confirm(_dialog: Dialog) -> Result<bool, String> {
    Err("Android: use show_confirm_with_context() with JNIEnv and Context".into())
}

pub async fn show_photo_picker(
    _picker: crate::PhotoPicker,
) -> Result<Option<Selection>, String> {
    Err("Android: use show_photo_picker_with_context() with JNIEnv and Context".into())
}

pub async fn load_media(_handle: Selection) -> Result<std::path::PathBuf, String> {
    Err("Android: use load_media_with_context() with JNIEnv and Context".into())
}
