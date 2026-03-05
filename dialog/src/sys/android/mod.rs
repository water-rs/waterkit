//! Android dialog implementation using JNI.
//!
//! The async APIs use `ndk-context` to obtain the Android `Context` automatically.
//! For advanced JNI integration, `*_with_context` APIs are also available.

use crate::{Dialog, DialogError, FileDialog};
use futures::channel::oneshot;
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::jlong;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Embedded DEX bytecode containing `DialogHelper` class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Opaque handle to a selected media item (URI string).
#[derive(Debug, Clone)]
pub struct Selection(pub String);

type PickerCallback = oneshot::Sender<Option<String>>;

static NEXT_PICKER_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn photo_picker_callbacks() -> &'static Mutex<HashMap<u64, PickerCallback>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, PickerCallback>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

fn file_picker_callbacks() -> &'static Mutex<HashMap<u64, PickerCallback>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, PickerCallback>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

fn decode_optional_string(env: &mut JNIEnv, uri: JObject) -> Option<String> {
    if uri.is_null() {
        return None;
    }
    let uri_jstring = JString::from(uri);
    Some(
        env.get_string(&uri_jstring)
            .unwrap_or_else(|error| {
                panic!("waterkit-dialog: failed to decode picker URI from JNI: {error}")
            })
            .into(),
    )
}

fn request_id_from_jlong(request_id: jlong) -> u64 {
    u64::try_from(request_id).unwrap_or_else(|_| {
        panic!("waterkit-dialog: request id conversion from jlong failed: {request_id}")
    })
}

fn jlong_from_request_id(request_id: u64) -> Result<jlong, DialogError> {
    jlong::try_from(request_id).map_err(|_| {
        DialogError::PlatformError(format!(
            "picker request id exceeds jlong range: {request_id}"
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_dialog_DialogHelper_onPhotoPickerResult(
    mut env: JNIEnv,
    _class: JClass,
    request_id: jlong,
    uri: JObject,
) {
    assert!(
        request_id > 0,
        "waterkit-dialog: invalid photo picker request id: {request_id}"
    );
    let uri = decode_optional_string(&mut env, uri);
    let tx = photo_picker_callbacks()
        .lock()
        .unwrap_or_else(|error| {
            panic!("waterkit-dialog: photo picker callback map lock poisoned: {error}")
        })
        .remove(&request_id_from_jlong(request_id))
        .unwrap_or_else(|| {
            panic!("waterkit-dialog: unknown photo picker request id in callback: {request_id}")
        });
    let _ = tx.send(uri);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_dialog_DialogHelper_onFilePickerResult(
    mut env: JNIEnv,
    _class: JClass,
    request_id: jlong,
    uri: JObject,
) {
    assert!(
        request_id > 0,
        "waterkit-dialog: invalid file picker request id: {request_id}"
    );
    let uri = decode_optional_string(&mut env, uri);
    let tx = file_picker_callbacks()
        .lock()
        .unwrap_or_else(|error| {
            panic!("waterkit-dialog: file picker callback map lock poisoned: {error}")
        })
        .remove(&request_id_from_jlong(request_id))
        .unwrap_or_else(|| {
            panic!("waterkit-dialog: unknown file picker request id in callback: {request_id}")
        });
    let _ = tx.send(uri);
}

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

    if CLASS_LOADER.set(global_ref).is_err() {
        debug_assert!(
            CLASS_LOADER.get().is_some(),
            "Class loader set failed but loader is still uninitialized"
        );
    }
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

fn get_vm_and_context() -> Result<(JavaVM, JObject<'static>), DialogError> {
    let android_ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android_ctx.vm().cast()) }
        .map_err(|e| DialogError::PlatformError(format!("from_raw vm: {e}")))?;
    let context = unsafe { JObject::from_raw(android_ctx.context().cast()) };
    if context.is_null() {
        return Err(DialogError::PlatformError(
            "ndk_context returned null Context".into(),
        ));
    }
    Ok((vm, context))
}

fn ensure_context_global() -> Result<(JavaVM, GlobalRef), DialogError> {
    let (vm, context) = get_vm_and_context()?;
    let global = {
        let env = vm
            .attach_current_thread()
            .map_err(|e| DialogError::PlatformError(format!("attach_current_thread: {e}")))?;
        env.new_global_ref(&context)
            .map_err(|e| DialogError::PlatformError(format!("new_global_ref context: {e}")))?
    };
    Ok((vm, global))
}

fn build_filters_csv(dialog: &FileDialog) -> String {
    let mut extensions = Vec::new();
    for (_, filter_extensions) in &dialog.filters {
        for ext in filter_extensions {
            let normalized = ext.trim().trim_start_matches('.').to_ascii_lowercase();
            if normalized.is_empty() {
                continue;
            }
            if !extensions
                .iter()
                .any(|existing: &String| existing == &normalized)
            {
                extensions.push(normalized);
            }
        }
    }
    extensions.join(",")
}

fn launch_photo_picker_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    picker: &crate::PhotoPicker,
) -> Result<oneshot::Receiver<Option<String>>, DialogError> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;

    let type_int = match picker.media_type {
        crate::MediaType::Image | crate::MediaType::LivePhoto => 0,
        crate::MediaType::Video => 1,
    };
    let request_id = NEXT_PICKER_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let request_id_jlong = jlong_from_request_id(request_id)?;
    let (tx, rx) = oneshot::channel();
    photo_picker_callbacks()
        .lock()
        .map_err(|e| DialogError::PlatformError(format!("photo picker callback map lock: {e}")))?
        .insert(request_id, tx);

    let launch_result = env.call_static_method(
        helper_class,
        "pickPhoto",
        "(Landroid/content/Context;IJ)V",
        &[
            JValue::Object(context),
            JValue::Int(type_int),
            JValue::Long(request_id_jlong),
        ],
    );
    if let Err(error) = launch_result {
        photo_picker_callbacks()
            .lock()
            .map_err(|e| {
                DialogError::PlatformError(format!("photo picker callback map lock cleanup: {e}"))
            })?
            .remove(&request_id);
        return Err(DialogError::PlatformError(format!("pickPhoto: {error}")));
    }
    Ok(rx)
}

fn launch_file_picker_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    dialog: &FileDialog,
) -> Result<oneshot::Receiver<Option<String>>, DialogError> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;

    let request_id = NEXT_PICKER_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let request_id_jlong = jlong_from_request_id(request_id)?;
    let (tx, rx) = oneshot::channel();
    file_picker_callbacks()
        .lock()
        .map_err(|e| DialogError::PlatformError(format!("file picker callback map lock: {e}")))?
        .insert(request_id, tx);

    let filters_csv = build_filters_csv(dialog);
    let filters_jstr = env
        .new_string(filters_csv)
        .map_err(|e| DialogError::PlatformError(format!("new_string filters: {e}")))?;
    let launch_result = env.call_static_method(
        helper_class,
        "pickFile",
        "(Landroid/content/Context;Ljava/lang/String;J)V",
        &[
            JValue::Object(context),
            JValue::Object(&filters_jstr),
            JValue::Long(request_id_jlong),
        ],
    );
    if let Err(error) = launch_result {
        file_picker_callbacks()
            .lock()
            .map_err(|e| {
                DialogError::PlatformError(format!("file picker callback map lock cleanup: {e}"))
            })?
            .remove(&request_id);
        return Err(DialogError::PlatformError(format!("pickFile: {error}")));
    }
    Ok(rx)
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
/// This context-bound API cannot return a result without blocking. Use
/// [`show_photo_picker`] which is async and non-blocking.
///
/// # Errors
/// Always returns an error to preserve non-blocking API semantics.
pub fn show_photo_picker_with_context(
    _env: &mut JNIEnv,
    _context: &JObject,
    _picker: &crate::PhotoPicker,
) -> Result<Option<Selection>, DialogError> {
    Err(DialogError::PlatformError(
        "show_photo_picker_with_context is unavailable in non-blocking mode; use show_photo_picker()"
            .into(),
    ))
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

// Public async API with implicit Android context.

/// Show an alert dialog.
///
/// # Errors
/// Returns an error if `ndk-context` is unavailable or JNI operations fail.
pub async fn show_alert(dialog: Dialog) -> Result<(), DialogError> {
    futures::future::ready({
        let (vm, context) = ensure_context_global()?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| DialogError::PlatformError(format!("attach_current_thread: {e}")))?;
        show_alert_with_context(&mut env, context.as_obj(), &dialog)
    })
    .await
}

/// Show a confirmation dialog.
///
/// # Errors
/// Returns an error if `ndk-context` is unavailable or JNI operations fail.
pub async fn show_confirm(dialog: Dialog) -> Result<bool, DialogError> {
    futures::future::ready({
        let (vm, context) = ensure_context_global()?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| DialogError::PlatformError(format!("attach_current_thread: {e}")))?;
        show_confirm_with_context(&mut env, context.as_obj(), &dialog)
    })
    .await
}

/// Show a photo picker.
///
/// # Errors
/// Returns an error if `ndk-context` is unavailable or JNI operations fail.
pub async fn show_photo_picker(
    picker: crate::PhotoPicker,
) -> Result<Option<Selection>, DialogError> {
    let (vm, context) = ensure_context_global()?;
    let rx = {
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| DialogError::PlatformError(format!("attach_current_thread: {e}")))?;
        launch_photo_picker_with_context(&mut env, context.as_obj(), &picker)?
    };
    let picked_uri = rx.await.map_err(|_| DialogError::Cancelled)?;
    Ok(picked_uri.map(Selection))
}

/// Show a file picker and copy the selected file into app cache.
///
/// # Errors
/// Returns an error if `ndk-context` is unavailable or JNI operations fail.
pub async fn show_open_single_file(
    dialog: FileDialog,
) -> Result<Option<std::path::PathBuf>, DialogError> {
    let (vm, context) = ensure_context_global()?;
    let rx = {
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| DialogError::PlatformError(format!("attach_current_thread: {e}")))?;
        launch_file_picker_with_context(&mut env, context.as_obj(), &dialog)?
    };
    let picked_uri = rx.await.map_err(|_| DialogError::Cancelled)?;
    match picked_uri {
        Some(uri) => load_media(Selection(uri)).await.map(Some),
        None => Ok(None),
    }
}

/// Load media from a selection handle.
///
/// # Errors
/// Returns an error if `ndk-context` is unavailable or JNI operations fail.
pub async fn load_media(handle: Selection) -> Result<std::path::PathBuf, DialogError> {
    futures::future::ready({
        let (vm, context) = ensure_context_global()?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| DialogError::PlatformError(format!("attach_current_thread: {e}")))?;
        load_media_with_context(&mut env, context.as_obj(), &handle)
    })
    .await
}
