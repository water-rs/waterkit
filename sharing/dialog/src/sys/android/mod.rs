//! Android dialog implementation using JNI.
//!
//! The async APIs use `ndk-context` to obtain the Android `Context`
//! automatically. The activity-result bridge owns picker lifecycle and result
//! delivery, so host applications do not forward activity results.

use crate::{Dialog, DialogError, FileDialog};
use futures::channel::oneshot;
use jni::objects::{JObject, JObjectArray, JString, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, FromJava, NativeCallback, PeerError, ResultCode,
    decode_optional_string, decode_string, dex_helper, start_activity_for_result,
    with_android_context,
};

/// `waterkit.dialog.DialogHelper`, compiled into the app's DEX by the packager
/// and resolved through the application's `ClassLoader`.
static HELPER: DexHelper = dex_helper!("waterkit.dialog.DialogHelper");

impl From<AndroidError> for DialogError {
    fn from(error: AndroidError) -> Self {
        Self::PlatformError(error.to_string())
    }
}

impl From<PeerError> for DialogError {
    fn from(error: PeerError) -> Self {
        Self::PlatformError(error.to_string())
    }
}

/// The alert carries no answer: the helper completes it with `null` once the
/// user dismisses the dialog.
struct Dismissed;

impl FromJava for Dismissed {
    fn from_java(_env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        if object.is_null() {
            Ok(Self)
        } else {
            Err(AndroidError::from(jni::errors::Error::ParseFailed(
                "expected the alert dialog's null payload".into(),
            )))
        }
    }
}

/// The confirm dialog's answer: the helper completes it with a
/// `java.lang.Boolean`.
struct Confirmed(bool);

impl FromJava for Confirmed {
    fn from_java(env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        let value = env
            .call_method(object, jni_str!("booleanValue"), jni_sig!("()Z"), &[])
            .and_then(JValueOwned::z)?;
        Ok(Self(value))
    }
}

/// Awaits a dialog's `NativeCallback` answer.
async fn answer<T>(
    receiver: oneshot::Receiver<Result<T, PeerError>>,
    dialog: &str,
) -> Result<T, DialogError> {
    receiver
        .await
        .map_err(|_| {
            DialogError::PlatformError(format!("the {dialog} callback was collected unanswered"))
        })?
        .map_err(DialogError::from)
}

/// Opaque handle to a selected media item (URI string).
#[derive(Debug, Clone)]
pub struct Selection(pub String);

fn build_photo_intent<'local>(
    env: &mut Env<'local>,
    context: &JObject<'_>,
    media_type: crate::MediaType,
) -> Result<JObject<'local>, DialogError> {
    let media_type = match media_type {
        crate::MediaType::Image | crate::MediaType::LivePhoto => 0,
        crate::MediaType::Video => 1,
    };
    let helper_class = HELPER.class(env, context)?;
    env.call_static_method(
        helper_class,
        jni_str!("photoPickIntent"),
        jni_sig!("(I)Landroid/content/Intent;"),
        &[JValue::Int(media_type)],
    )
    .map_err(DialogError::from)?
    .l()
    .map_err(DialogError::from)
}

fn build_string_array<'local>(
    env: &mut Env<'local>,
    extensions: &[String],
) -> Result<JObjectArray<'local, JString<'local>>, DialogError> {
    let array = JObjectArray::<JString>::new(env, extensions.len(), JString::null())
        .map_err(DialogError::from)?;
    for (index, extension) in extensions.iter().enumerate() {
        let extension = env.new_string(extension).map_err(DialogError::from)?;
        array
            .set_element(env, index, &extension)
            .map_err(DialogError::from)?;
    }
    Ok(array)
}

fn build_file_intent<'local>(
    env: &mut Env<'local>,
    context: &JObject<'_>,
    dialog: &FileDialog,
    allow_multiple: bool,
) -> Result<JObject<'local>, DialogError> {
    let helper_class = HELPER.class(env, context)?;
    let extensions = crate::collect_filter_extensions(dialog);
    let extensions = build_string_array(env, &extensions)?;
    env.call_static_method(
        helper_class,
        jni_str!("openDocumentIntent"),
        jni_sig!("([Ljava/lang/String;Z)Landroid/content/Intent;"),
        &[JValue::Object(&extensions), JValue::Bool(allow_multiple)],
    )
    .map_err(DialogError::from)?
    .l()
    .map_err(DialogError::from)
}

fn selected_uri(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    data: &JObject<'_>,
) -> Result<Option<String>, DialogError> {
    let helper_class = HELPER.class(env, context)?;
    let result = env
        .call_static_method(
            helper_class,
            jni_str!("selectedUri"),
            jni_sig!("(Landroid/content/Intent;)Ljava/lang/String;"),
            &[JValue::Object(data)],
        )
        .map_err(DialogError::from)?
        .l()
        .map_err(DialogError::from)?;
    Ok(decode_optional_string(env, &result)?)
}

fn selected_uris(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    data: &JObject<'_>,
) -> Result<Vec<String>, DialogError> {
    let helper_class = HELPER.class(env, context)?;
    let result = env
        .call_static_method(
            helper_class,
            jni_str!("selectedUris"),
            jni_sig!("(Landroid/content/Intent;)[Ljava/lang/String;"),
            &[JValue::Object(data)],
        )
        .map_err(DialogError::from)?
        .l()
        .map_err(DialogError::from)?;
    let result = env
        .cast_local::<JObjectArray<JString>>(result)
        .map_err(DialogError::from)?;
    let count = result.len(env).map_err(DialogError::from)?;
    let mut uris = Vec::with_capacity(count);
    for index in 0..count {
        let uri = result.get_element(env, index).map_err(DialogError::from)?;
        uris.push(decode_string(env, &uri)?);
    }
    Ok(uris)
}

fn result_code_error(code: ResultCode) -> DialogError {
    DialogError::PlatformError(format!(
        "activity result returned unexpected code: {code:?}"
    ))
}

fn no_selection_error(kind: &str) -> DialogError {
    DialogError::PlatformError(format!(
        "activity result returned RESULT_OK without a {kind} selection"
    ))
}

/// Posts the alert dialog to the main looper; the returned receiver resolves
/// when the user dismisses it.
fn post_alert_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    dialog: &Dialog,
) -> Result<oneshot::Receiver<Result<Dismissed, PeerError>>, DialogError> {
    let helper_class = HELPER.class(env, context)?;
    let title = env.new_string(&dialog.title).map_err(DialogError::from)?;
    let message = env.new_string(&dialog.message).map_err(DialogError::from)?;
    let (callback, receiver) = NativeCallback::<Dismissed>::new(env)?;

    env.call_static_method(
        helper_class,
        jni_str!("showDialog"),
        jni_sig!(
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Lwaterkit/build/NativeCallback;)V"
        ),
        &[
            JValue::Object(context),
            JValue::Object(&title),
            JValue::Object(&message),
            JValue::Object(callback.as_obj()),
        ],
    )
    .map_err(DialogError::from)?;
    Ok(receiver)
}

/// Posts the confirmation dialog to the main looper; the returned receiver
/// resolves to the user's answer.
fn post_confirm_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    dialog: &Dialog,
) -> Result<oneshot::Receiver<Result<Confirmed, PeerError>>, DialogError> {
    let helper_class = HELPER.class(env, context)?;
    let title = env.new_string(&dialog.title).map_err(DialogError::from)?;
    let message = env.new_string(&dialog.message).map_err(DialogError::from)?;
    let (callback, receiver) = NativeCallback::<Confirmed>::new(env)?;

    env.call_static_method(
        helper_class,
        jni_str!("showConfirm"),
        jni_sig!(
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Lwaterkit/build/NativeCallback;)V"
        ),
        &[
            JValue::Object(context),
            JValue::Object(&title),
            JValue::Object(&message),
            JValue::Object(callback.as_obj()),
        ],
    )
    .map_err(DialogError::from)?;
    Ok(receiver)
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
    let helper_class = HELPER.class(env, context)?;
    let uri = env.new_string(&handle.0).map_err(DialogError::from)?;
    let result = env
        .call_static_method(
            helper_class,
            jni_str!("loadMedia"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;"),
            &[JValue::Object(context), JValue::Object(&uri)],
        )
        .map_err(DialogError::from)?
        .l()
        .map_err(DialogError::from)?;

    if result.is_null() {
        return Err(DialogError::PlatformError(
            "failed to load media (returned null)".into(),
        ));
    }
    Ok(std::path::PathBuf::from(decode_string(env, &result)?))
}

/// Show an alert dialog.
///
/// # Errors
/// Returns an error if `ndk-context` is unavailable or JNI operations fail.
pub async fn show_alert(dialog: Dialog) -> Result<(), DialogError> {
    let receiver =
        with_android_context(|env, context| post_alert_with_context(env, context, &dialog))?;
    answer(receiver, "alert dialog").await.map(|_| ())
}

/// Show a confirmation dialog.
///
/// # Errors
/// Returns an error if `ndk-context` is unavailable or JNI operations fail.
pub async fn show_confirm(dialog: Dialog) -> Result<bool, DialogError> {
    let receiver =
        with_android_context(|env, context| post_confirm_with_context(env, context, &dialog))?;
    Ok(answer(receiver, "confirm dialog").await?.0)
}

/// Show a photo picker.
///
/// # Errors
/// Returns an error if the picker fails to launch or returns invalid data.
pub async fn show_photo_picker(
    media_type: crate::MediaType,
) -> Result<Option<Selection>, DialogError> {
    let pending = with_android_context(|env, context| {
        let intent = build_photo_intent(env, context, media_type)?;
        start_activity_for_result(env, &intent).map_err(DialogError::from)
    })?;
    let result = pending.await.map_err(DialogError::from)?;
    match result.code() {
        ResultCode::Ok => {
            let data = result
                .into_data()
                .ok_or_else(|| no_selection_error("photo"))?;
            let uri =
                with_android_context(|env, context| selected_uri(env, context, data.as_obj()))?
                    .ok_or_else(|| no_selection_error("photo"))?;
            Ok(Some(Selection(uri)))
        }
        ResultCode::Canceled => Ok(None),
        code @ ResultCode::Custom(_) => Err(result_code_error(code)),
    }
}

/// Show a file picker and copy the selected file into app cache.
///
/// # Errors
/// Returns an error if the picker fails to launch or media loading fails.
pub async fn show_open_single_file(
    dialog: FileDialog,
) -> Result<Option<std::path::PathBuf>, DialogError> {
    let pending = with_android_context(|env, context| {
        let intent = build_file_intent(env, context, &dialog, false)?;
        start_activity_for_result(env, &intent).map_err(DialogError::from)
    })?;
    let result = pending.await.map_err(DialogError::from)?;
    match result.code() {
        ResultCode::Ok => {
            let data = result
                .into_data()
                .ok_or_else(|| no_selection_error("file"))?;
            let uri =
                with_android_context(|env, context| selected_uri(env, context, data.as_obj()))?
                    .ok_or_else(|| no_selection_error("file"))?;
            let selection = Selection(uri);
            let path = load_media(&selection)?;
            crate::finalize_selected_file(&dialog, path).map(Some)
        }
        ResultCode::Canceled => Ok(None),
        code @ ResultCode::Custom(_) => Err(result_code_error(code)),
    }
}

/// Show a file picker and copy the selected files into app cache.
///
/// # Errors
/// Returns an error if the picker fails to launch or media loading fails.
pub async fn show_open_multiple_files(
    dialog: FileDialog,
) -> Result<Option<Vec<std::path::PathBuf>>, DialogError> {
    let pending = with_android_context(|env, context| {
        let intent = build_file_intent(env, context, &dialog, true)?;
        start_activity_for_result(env, &intent).map_err(DialogError::from)
    })?;
    let result = pending.await.map_err(DialogError::from)?;
    match result.code() {
        ResultCode::Ok => {
            let data = result
                .into_data()
                .ok_or_else(|| no_selection_error("file"))?;
            let uris =
                with_android_context(|env, context| selected_uris(env, context, data.as_obj()))?;
            if uris.is_empty() {
                return Err(no_selection_error("file"));
            }
            let mut paths = Vec::with_capacity(uris.len());
            for uri in uris {
                paths.push(load_media(&Selection(uri))?);
            }
            crate::finalize_selected_files(&dialog, paths).map(Some)
        }
        ResultCode::Canceled => Ok(None),
        code @ ResultCode::Custom(_) => Err(result_code_error(code)),
    }
}

/// Load media from a selection handle.
///
/// # Errors
/// Returns an error if `ndk-context` is unavailable or media loading fails.
pub fn load_media(handle: &Selection) -> Result<std::path::PathBuf, DialogError> {
    with_android_context(|env, context| load_media_with_context(env, context, handle))
}

pub async fn load_photo_media(
    handle: Selection,
    requested_media_type: crate::MediaType,
) -> Result<crate::LoadedMedia, DialogError> {
    let path = load_media(&handle)?;
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
