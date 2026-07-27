//! Android dialog implementation using JNI.
//!
//! The async APIs use `ndk-context` to obtain the Android `Context` automatically.
//! For advanced JNI integration, `*_with_context` APIs are also available.

use crate::{Dialog, DialogError, FileDialog};
use futures::channel::oneshot;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JClass, JObject, JString, JValue};
use jni::sys::jlong;
use jni::{Env, EnvUnowned, JavaVM, jni_sig, jni_str};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Embedded DEX bytecode containing `DialogHelper` class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader.
static CLASS_LOADER: OnceLock<Global<JObject<'static>>> = OnceLock::new();

/// Opaque handle to a selected media item (URI string).
#[derive(Debug, Clone)]
pub struct Selection(pub String);

type PickerCallback = oneshot::Sender<Option<String>>;
type MultiPickerCallback = oneshot::Sender<Option<Vec<String>>>;

static NEXT_PICKER_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn photo_picker_callbacks() -> &'static Mutex<HashMap<u64, PickerCallback>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, PickerCallback>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

fn file_picker_callbacks() -> &'static Mutex<HashMap<u64, PickerCallback>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, PickerCallback>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

fn multiple_file_picker_callbacks() -> &'static Mutex<HashMap<u64, MultiPickerCallback>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, MultiPickerCallback>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

fn decode_optional_string(
    env: &mut Env<'_>,
    uri: &JObject<'_>,
) -> jni::errors::Result<Option<String>> {
    if uri.is_null() {
        return Ok(None);
    }
    let uri_jstring = env.as_cast::<JString>(uri)?;
    uri_jstring.try_to_string(env).map(Some)
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
pub extern "system" fn Java_waterkit_dialog_DialogHelper_onPhotoPickerResult<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    request_id: jlong,
    uri: JObject<'caller>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        assert!(
            request_id > 0,
            "waterkit-dialog: invalid photo picker request id: {request_id}"
        );
        let uri = decode_optional_string(env, &uri)?;
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
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_dialog_DialogHelper_onFilePickerResult<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    request_id: jlong,
    uri: JObject<'caller>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        assert!(
            request_id > 0,
            "waterkit-dialog: invalid file picker request id: {request_id}"
        );
        let uri = decode_optional_string(env, &uri)?;
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
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_dialog_DialogHelper_onFilePickerMultipleResult<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    request_id: jlong,
    uris: JObject<'caller>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        assert!(
            request_id > 0,
            "waterkit-dialog: invalid multiple file picker request id: {request_id}"
        );
        let encoded = decode_optional_string(env, &uris)?;
        let uris = crate::decode_string_list(encoded);
        let tx = multiple_file_picker_callbacks()
            .lock()
            .unwrap_or_else(|error| {
                panic!("waterkit-dialog: multiple file picker callback map lock poisoned: {error}")
            })
            .remove(&request_id_from_jlong(request_id))
            .unwrap_or_else(|| {
                panic!(
                    "waterkit-dialog: unknown multiple file picker request id in callback: {request_id}"
                )
            });
        let _ = tx.send(uris);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

/// Initialize the DEX class loader. Must be called with a valid Context.
///
/// # Errors
/// Returns an error if JNI operations fail.
pub fn init_with_context(env: &mut Env<'_>, context: &JObject) -> Result<(), DialogError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    let cache_dir = env
        .call_method(
            context,
            jni_str!("getCacheDir"),
            jni_sig!("()Ljava/io/File;"),
            &[],
        )?
        .l()
        .map_err(|e| DialogError::PlatformError(format!("getCacheDir: {e}")))?;

    let cache_path = env
        .call_method(
            &cache_dir,
            jni_str!("getAbsolutePath"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )?
        .l()
        .map_err(|e| DialogError::PlatformError(format!("getAbsolutePath: {e}")))?;

    let cache_path_string = env
        .as_cast::<JString>(&cache_path)
        .and_then(|path| path.try_to_string(env))
        .map_err(|e| DialogError::PlatformError(format!("decode cache path: {e}")))?;
    let dex_path = format!("{}/waterkit_dialog.dex", cache_path_string);

    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| DialogError::PlatformError(format!("write DEX: {e}")))?;

    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| DialogError::PlatformError(format!("new_string: {e}")))?;

    let parent_loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )?
        .l()
        .map_err(|e| DialogError::PlatformError(format!("getClassLoader: {e}")))?;

    let dex_class_loader_class = env
        .find_class(jni_str!("dalvik/system/DexClassLoader"))
        .map_err(|e| DialogError::PlatformError(format!("find_class: {e}")))?;

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

fn get_helper_class<'local>(env: &mut Env<'local>) -> Result<JClass<'local>, DialogError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| DialogError::PlatformError("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.dialog.DialogHelper")
        .map_err(|e| DialogError::PlatformError(format!("new_string: {e}")))?;

    let loaded_class = env
        .call_method(
            class_loader.as_obj(),
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&helper_class_name)],
        )?
        .l()
        .map_err(|e| DialogError::PlatformError(format!("loadClass: {e}")))?;

    env.cast_local::<JClass>(loaded_class)
        .map_err(DialogError::from)
}

fn ensure_context_global() -> Result<(JavaVM, Global<JObject<'static>>), DialogError> {
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    if raw_vm.is_null() {
        return Err(DialogError::PlatformError(
            "ndk_context returned null JavaVM".into(),
        ));
    }
    if raw_context.is_null() {
        return Err(DialogError::PlatformError(
            "ndk_context returned null Context".into(),
        ));
    }

    let vm = unsafe { JavaVM::from_raw(raw_vm) };
    let global = vm.attach_current_thread(|env| {
        let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
        env.new_global_ref(&*context).map_err(DialogError::from)
    })?;
    Ok((vm, global))
}

fn launch_photo_picker_with_context(
    env: &mut Env<'_>,
    context: &JObject,
    media_type: crate::MediaType,
) -> Result<oneshot::Receiver<Option<String>>, DialogError> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;

    let type_int = match media_type {
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
        jni_str!("pickPhoto"),
        jni_sig!("(Landroid/content/Context;IJ)V"),
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
    env: &mut Env<'_>,
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

    let filters_csv = crate::collect_filter_extensions(dialog).join(",");
    let filters_jstr = env
        .new_string(filters_csv)
        .map_err(|e| DialogError::PlatformError(format!("new_string filters: {e}")))?;
    let launch_result = env.call_static_method(
        helper_class,
        jni_str!("pickFile"),
        jni_sig!("(Landroid/content/Context;Ljava/lang/String;J)V"),
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

fn launch_multiple_file_picker_with_context(
    env: &mut Env<'_>,
    context: &JObject,
    dialog: &FileDialog,
) -> Result<oneshot::Receiver<Option<Vec<String>>>, DialogError> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;

    let request_id = NEXT_PICKER_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let request_id_jlong = jlong_from_request_id(request_id)?;
    let (tx, rx) = oneshot::channel();
    multiple_file_picker_callbacks()
        .lock()
        .map_err(|e| {
            DialogError::PlatformError(format!("multiple file picker callback map lock: {e}"))
        })?
        .insert(request_id, tx);

    let filters_csv = crate::collect_filter_extensions(dialog).join(",");
    let filters_jstr = env
        .new_string(filters_csv)
        .map_err(|e| DialogError::PlatformError(format!("new_string filters: {e}")))?;
    let launch_result = env.call_static_method(
        helper_class,
        jni_str!("pickMultipleFiles"),
        jni_sig!("(Landroid/content/Context;Ljava/lang/String;J)V"),
        &[
            JValue::Object(context),
            JValue::Object(&filters_jstr),
            JValue::Long(request_id_jlong),
        ],
    );
    if let Err(error) = launch_result {
        multiple_file_picker_callbacks()
            .lock()
            .map_err(|e| {
                DialogError::PlatformError(format!(
                    "multiple file picker callback map lock cleanup: {e}"
                ))
            })?
            .remove(&request_id);
        return Err(DialogError::PlatformError(format!(
            "pickMultipleFiles: {error}"
        )));
    }
    Ok(rx)
}

/// Show an alert dialog with JNI context.
///
/// # Errors
/// Returns an error if JNI operations fail.
pub fn show_alert_with_context(
    env: &mut Env<'_>,
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
        jni_str!("showDialog"),
        jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V"),
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
    env: &mut Env<'_>,
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
            jni_str!("showConfirm"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Z"),
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
    _env: &mut Env<'_>,
    _context: &JObject,
    _media_type: crate::MediaType,
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
    env: &mut Env<'_>,
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
            jni_str!("loadMedia"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;"),
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
        let path = env
            .as_cast::<JString>(&result)
            .and_then(|path| path.try_to_string(env))
            .map_err(|e| DialogError::PlatformError(format!("decode media path: {e}")))?;
        Ok(std::path::PathBuf::from(path))
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
        vm.attach_current_thread(|env| show_alert_with_context(env, context.as_obj(), &dialog))
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
        vm.attach_current_thread(|env| show_confirm_with_context(env, context.as_obj(), &dialog))
    })
    .await
}

/// Show a photo picker.
///
/// # Errors
/// Returns an error if `ndk-context` is unavailable or JNI operations fail.
pub async fn show_photo_picker(
    media_type: crate::MediaType,
) -> Result<Option<Selection>, DialogError> {
    let (vm, context) = ensure_context_global()?;
    let rx = vm.attach_current_thread(|env| {
        launch_photo_picker_with_context(env, context.as_obj(), media_type)
    })?;
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
    let rx = vm.attach_current_thread(|env| {
        launch_file_picker_with_context(env, context.as_obj(), &dialog)
    })?;
    let picked_uri = rx.await.map_err(|_| DialogError::Cancelled)?;
    match picked_uri {
        Some(uri) => {
            let path = load_media(Selection(uri)).await?;
            crate::finalize_selected_file(&dialog, path).map(Some)
        }
        None => Ok(None),
    }
}

/// Show a file picker and copy the selected files into app cache.
///
/// # Errors
/// Returns an error if `ndk-context` is unavailable or JNI operations fail.
pub async fn show_open_multiple_files(
    dialog: FileDialog,
) -> Result<Option<Vec<std::path::PathBuf>>, DialogError> {
    let (vm, context) = ensure_context_global()?;
    let rx = vm.attach_current_thread(|env| {
        launch_multiple_file_picker_with_context(env, context.as_obj(), &dialog)
    })?;
    let picked_uris = rx.await.map_err(|_| DialogError::Cancelled)?;
    match picked_uris {
        Some(uris) => {
            let mut paths = Vec::with_capacity(uris.len());
            for uri in uris {
                paths.push(load_media(Selection(uri)).await?);
            }
            crate::finalize_selected_files(&dialog, paths).map(Some)
        }
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
        vm.attach_current_thread(|env| load_media_with_context(env, context.as_obj(), &handle))
    })
    .await
}

pub async fn load_photo_media(
    handle: Selection,
    requested_media_type: crate::MediaType,
) -> Result<crate::LoadedMedia, DialogError> {
    let path = load_media(handle).await?;
    if let Some(live_photo) = crate::motion_photo::load_live_photo_from_motion_photo(&path)? {
        return Ok(crate::LoadedMedia::LivePhoto(live_photo));
    }

    match requested_media_type {
        crate::MediaType::Image => Ok(crate::LoadedMedia::Image(path)),
        crate::MediaType::Video => Ok(crate::LoadedMedia::Video(path)),
        crate::MediaType::LivePhoto => Err(DialogError::Unsupported(
            "the selected Android asset is not a Motion Photo".into(),
        )),
    }
}
