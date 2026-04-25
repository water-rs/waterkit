//! Android filesystem implementation using JNI.

use jni::JNIEnv;
use jni::objects::{JObject, JValue};
use std::path::PathBuf;

fn string_object_to_path(
    env: &mut JNIEnv,
    value: &JObject,
) -> jni::errors::Result<Option<PathBuf>> {
    if value.is_null() {
        return Ok(None);
    }
    let path: String = env.get_string(value.into())?.into();
    Ok(Some(PathBuf::from(path)))
}

fn file_object_to_path(env: &mut JNIEnv, file: &JObject) -> jni::errors::Result<Option<PathBuf>> {
    if file.is_null() {
        return Ok(None);
    }
    let absolute_path = env
        .call_method(file, "getAbsolutePath", "()Ljava/lang/String;", &[])?
        .l()?;
    string_object_to_path(env, &absolute_path)
}

fn call_context_file_method(
    env: &mut JNIEnv,
    context: &JObject,
    method_name: &str,
    signature: &str,
    args: &[JValue],
) -> jni::errors::Result<Option<PathBuf>> {
    let file = env
        .call_method(context, method_name, signature, args)?
        .l()?;
    file_object_to_path(env, &file)
}

/// Gets the application's documents directory using an Android `Context`.
pub fn documents_dir_with_context(env: &mut JNIEnv, context: &JObject) -> Option<PathBuf> {
    let documents_value = env.get_static_field(
        "android/os/Environment",
        "DIRECTORY_DOCUMENTS",
        "Ljava/lang/String;",
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
        "getExternalFilesDir",
        "(Ljava/lang/String;)Ljava/io/File;",
        &[JValue::Object(&documents_kind)],
    )
    .unwrap_or_else(|error| {
        tracing::error!("waterkit-fs: failed to resolve Android documents dir: {error}");
        None
    })
}

/// Gets the application's cache directory using an Android `Context`.
pub fn cache_dir_with_context(env: &mut JNIEnv, context: &JObject) -> Option<PathBuf> {
    call_context_file_method(env, context, "getCacheDir", "()Ljava/io/File;", &[]).unwrap_or_else(
        |error| {
            tracing::error!("waterkit-fs: failed to resolve Android cache dir: {error}");
            None
        },
    )
}

pub fn documents_dir() -> Option<PathBuf> {
    tracing::warn!("waterkit-fs: Android documents_dir requires Context");
    None
}

pub fn cache_dir() -> Option<PathBuf> {
    tracing::warn!("waterkit-fs: Android cache_dir requires Context");
    None
}
