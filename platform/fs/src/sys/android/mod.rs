//! Android filesystem implementation using JNI.

use jni::objects::{JObject, JString, JValue};
use jni::signature::MethodSignature;
use jni::strings::JNIStr;
use jni::{Env, jni_sig, jni_str};
use std::path::PathBuf;

const GET_ABSOLUTE_PATH: MethodSignature<'static, 'static> = jni_sig!(() -> JString);
const GET_EXTERNAL_FILES_DIR: MethodSignature<'static, 'static> =
    jni_sig!((JString) -> java.io.File);
const GET_CACHE_DIR: MethodSignature<'static, 'static> = jni_sig!(() -> java.io.File);

fn string_object_to_path(env: &Env<'_>, value: &JObject) -> jni::errors::Result<Option<PathBuf>> {
    if value.is_null() {
        return Ok(None);
    }
    let path = env.as_cast::<JString>(value)?.try_to_string(env)?;
    Ok(Some(PathBuf::from(path)))
}

fn file_object_to_path(env: &mut Env<'_>, file: &JObject) -> jni::errors::Result<Option<PathBuf>> {
    if file.is_null() {
        return Ok(None);
    }
    let absolute_path = env
        .call_method(file, jni_str!("getAbsolutePath"), &GET_ABSOLUTE_PATH, &[])?
        .l()?;
    string_object_to_path(env, &absolute_path)
}

fn call_context_file_method(
    env: &mut Env<'_>,
    context: &JObject,
    method_name: &JNIStr,
    signature: &MethodSignature<'_, '_>,
    args: &[JValue],
) -> jni::errors::Result<Option<PathBuf>> {
    let file = env
        .call_method(context, method_name, signature, args)?
        .l()?;
    file_object_to_path(env, &file)
}

/// Gets the application's documents directory using an Android `Context`.
pub fn documents_dir_with_context(env: &mut Env<'_>, context: &JObject) -> Option<PathBuf> {
    let documents_value = env.get_static_field(
        jni_str!("android/os/Environment"),
        jni_str!("DIRECTORY_DOCUMENTS"),
        jni_sig!(JString),
    );
    let documents_kind = match documents_value {
        Ok(value) => match value.l() {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(
                    "waterkit-fs: failed to convert Android documents constant: {error}"
                );
                return None;
            }
        },
        Err(error) => {
            tracing::error!("waterkit-fs: failed to resolve Android documents constant: {error}");
            return None;
        }
    };

    call_context_file_method(
        env,
        context,
        jni_str!("getExternalFilesDir"),
        &GET_EXTERNAL_FILES_DIR,
        &[JValue::Object(&documents_kind)],
    )
    .unwrap_or_else(|error| {
        tracing::error!("waterkit-fs: failed to resolve Android documents dir: {error}");
        None
    })
}

/// Gets the application's cache directory using an Android `Context`.
pub fn cache_dir_with_context(env: &mut Env<'_>, context: &JObject) -> Option<PathBuf> {
    call_context_file_method(env, context, jni_str!("getCacheDir"), &GET_CACHE_DIR, &[])
        .unwrap_or_else(|error| {
            tracing::error!("waterkit-fs: failed to resolve Android cache dir: {error}");
            None
        })
}

pub fn documents_dir() -> Option<PathBuf> {
    tracing::warn!("waterkit-fs: Android documents_dir requires Context");
    None
}

pub fn cache_dir() -> Option<PathBuf> {
    tracing::warn!("waterkit-fs: Android cache_dir requires Context");
    None
}
