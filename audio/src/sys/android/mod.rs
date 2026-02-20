//! Android media control implementation using JNI and `MediaSession`.

#![allow(dead_code)] // Functions and statics are exported for Android app usage

use crate::{
    MediaCommand, MediaCommandHandler, MediaError, MediaMetadata, PlaybackState, PlaybackStatus,
};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use std::sync::OnceLock;

/// Embedded DEX bytecode containing `MediaSessionHelper` class.
/// Generated at build time by kotlinc + D8.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Initialize the DEX class loader. Must be called with a valid Context.
///
/// # Safety
///
/// The `context` must be a valid Android Context `JObject`.
pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), MediaError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Write DEX to cache directory
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|e| MediaError::InitializationFailed(format!("getCacheDir failed: {e}")))?
        .l()
        .map_err(|e| MediaError::InitializationFailed(format!("getCacheDir result: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| MediaError::InitializationFailed(format!("getAbsolutePath failed: {e}")))?
        .l()
        .map_err(|e| MediaError::InitializationFailed(format!("getAbsolutePath result: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_media.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| MediaError::InitializationFailed(format!("get_string failed: {e}")))?
            .to_str()
            .map_err(|e| MediaError::InitializationFailed(format!("to_str failed: {e}")))?
    );

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| MediaError::InitializationFailed(format!("write DEX failed: {e}")))?;

    // Create DexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| MediaError::InitializationFailed(format!("new_string failed: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| MediaError::InitializationFailed(format!("getClassLoader failed: {e}")))?
        .l()
        .map_err(|e| MediaError::InitializationFailed(format!("getClassLoader result: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| MediaError::InitializationFailed(format!("find DexClassLoader: {e}")))?;

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
        .map_err(|e| MediaError::InitializationFailed(format!("new DexClassLoader: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| MediaError::InitializationFailed(format!("new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

use jni::objects::JClass;

/// Get the `MediaSessionHelper` class.
fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<JClass<'a>, MediaError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| MediaError::InitializationFailed("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.media.MediaSessionHelper")
        .map_err(|e| MediaError::Unknown(format!("new_string: {e}")))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .map_err(|e| MediaError::Unknown(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| MediaError::Unknown(format!("loadClass result: {e}")))?;

    // helper_class is a JObject representing a Class. Convert to JClass.
    // Ensure we import JClass.
    Ok(helper_class.into())
}

/// Create a media session using the Context.
pub fn create_session_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), MediaError> {
    init_with_context(env, context)?;

    let helper_class = get_helper_class(env)?;

    env.call_static_method::<&JClass, _, _>(
        &helper_class,
        "createSession",
        "(Landroid/content/Context;)V",
        &[JValue::Object(context)],
    )
    .map_err(|e| MediaError::InitializationFailed(format!("createSession: {e}")))?;

    Ok(())
}

/// Set metadata using the Context.
pub fn set_metadata_with_context(
    env: &mut JNIEnv,
    metadata: &MediaMetadata,
) -> Result<(), MediaError> {
    let helper_class = get_helper_class(env)?;

    let title = env
        .new_string(metadata.title.as_deref().unwrap_or(""))
        .map_err(|e| MediaError::UpdateFailed(format!("new_string title: {e}")))?;
    let artist = env
        .new_string(metadata.artist.as_deref().unwrap_or(""))
        .map_err(|e| MediaError::UpdateFailed(format!("new_string artist: {e}")))?;
    let album = env
        .new_string(metadata.album.as_deref().unwrap_or(""))
        .map_err(|e| MediaError::UpdateFailed(format!("new_string album: {e}")))?;
    let artwork_url = env
        .new_string(metadata.artwork_url.as_deref().unwrap_or(""))
        .map_err(|e| MediaError::UpdateFailed(format!("new_string artwork_url: {e}")))?;

    #[allow(clippy::cast_possible_truncation)]
    let duration_ms = metadata.duration.map_or(-1, |d| d.as_millis() as i64);

    env.call_static_method::<&JClass, _, _>(
        &helper_class,
        "setMetadata",
        "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;J)V",
        &[
            JValue::Object(&title),
            JValue::Object(&artist),
            JValue::Object(&album),
            JValue::Object(&artwork_url),
            JValue::Long(duration_ms),
        ],
    )
    .map_err(|e| MediaError::UpdateFailed(format!("setMetadata: {e}")))?;

    Ok(())
}

/// Set playback state.
pub fn set_playback_state_with_context(
    env: &mut JNIEnv,
    state: &PlaybackState,
) -> Result<(), MediaError> {
    let helper_class = get_helper_class(env)?;

    let status = match state.status {
        PlaybackStatus::Stopped => 0,
        PlaybackStatus::Paused => 1,
        PlaybackStatus::Playing => 2,
    };

    #[allow(clippy::cast_possible_truncation)]
    let position_ms = state.position.map_or(-1, |d| d.as_millis() as i64);

    env.call_static_method::<&JClass, _, _>(
        &helper_class,
        "setPlaybackState",
        "(IJF)V",
        &[
            JValue::Int(status),
            JValue::Long(position_ms),
            #[allow(clippy::cast_possible_truncation)]
            JValue::Float(state.rate as f32),
        ],
    )
    .map_err(|e| MediaError::UpdateFailed(format!("setPlaybackState: {e}")))?;

    Ok(())
}

/// Request audio focus.
pub fn request_audio_focus_with_context(env: &mut JNIEnv) -> Result<(), MediaError> {
    let helper_class = get_helper_class(env)?;

    let result = env
        .call_static_method::<&JClass, _, _>(&helper_class, "requestAudioFocus", "()Z", &[])
        .map_err(|e| MediaError::Unknown(format!("requestAudioFocus: {e}")))?
        .z()
        .map_err(|e| MediaError::Unknown(format!("requestAudioFocus result: {e}")))?;

    if result {
        Ok(())
    } else {
        Err(MediaError::AudioFocusDenied)
    }
}

/// Abandon audio focus.
pub fn abandon_audio_focus_with_context(env: &mut JNIEnv) -> Result<(), MediaError> {
    let helper_class = get_helper_class(env)?;

    env.call_static_method::<&JClass, _, _>(&helper_class, "abandonAudioFocus", "()V", &[])
        .map_err(|e| MediaError::Unknown(format!("abandonAudioFocus: {e}")))?;

    Ok(())
}

/// Clear the media session.
pub fn clear_session(env: &mut JNIEnv) -> Result<(), MediaError> {
    let helper_class = get_helper_class(env)?;

    env.call_static_method::<&JClass, _, _>(&helper_class, "clearSession", "()V", &[])
        .map_err(|e| MediaError::Unknown(format!("clearSession: {e}")))?;

    Ok(())
}

#[derive(Debug)]
pub struct MediaCenterInner {
    vm: jni::JavaVM,
    context: GlobalRef,
}

impl MediaCenterInner {
    fn with_attached_env<T>(
        &self,
        op: impl FnOnce(&mut JNIEnv, &JObject) -> Result<T, MediaError>,
    ) -> Result<T, MediaError> {
        let mut env = self
            .vm
            .attach_current_thread()
            .map_err(|e| MediaError::Unknown(format!("attach_current_thread failed: {e}")))?;
        op(&mut env, self.context.as_obj())
    }

    pub fn new() -> Result<Self, MediaError> {
        let android_context =
            std::panic::catch_unwind(ndk_context::android_context).map_err(|_| {
                MediaError::InitializationFailed("ndk context is not initialized".into())
            })?;

        let vm = unsafe { jni::JavaVM::from_raw(android_context.vm().cast()) }.map_err(|e| {
            MediaError::InitializationFailed(format!("JavaVM::from_raw failed: {e}"))
        })?;

        let context = {
            let mut env = vm.attach_current_thread().map_err(|e| {
                MediaError::InitializationFailed(format!("attach_current_thread failed: {e}"))
            })?;

            let context_local = unsafe { JObject::from_raw(android_context.context().cast()) };
            let context = env.new_global_ref(&context_local).map_err(|e| {
                MediaError::InitializationFailed(format!("new_global_ref context failed: {e}"))
            })?;

            create_session_with_context(&mut env, context.as_obj())?;
            context
        };

        Ok(Self { vm, context })
    }

    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        self.with_attached_env(|env, _context| set_metadata_with_context(env, metadata))
    }

    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        self.with_attached_env(|env, _context| set_playback_state_with_context(env, state))
    }

    #[allow(clippy::unused_self)]
    pub fn set_command_handler(
        &self,
        _handler: Box<dyn MediaCommandHandler>,
    ) -> Result<(), MediaError> {
        Err(MediaError::NotSupported)
    }

    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        self.with_attached_env(|env, _context| request_audio_focus_with_context(env))
    }

    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        self.with_attached_env(|env, _context| abandon_audio_focus_with_context(env))
    }

    pub fn clear(&self) -> Result<(), MediaError> {
        self.with_attached_env(|env, _context| clear_session(env))
    }

    pub fn update(&self, metadata: &MediaMetadata, state: &PlaybackState) {
        let _ = self.set_metadata(metadata);
        let _ = self.set_playback_state(state);
    }

    #[allow(clippy::unused_self)]
    pub const fn run_loop(&self, _duration: std::time::Duration) {}

    #[allow(clippy::unused_self)]
    pub const fn poll_command(&self) -> Option<MediaCommand> {
        None
    }
}

impl Drop for MediaCenterInner {
    fn drop(&mut self) {
        let _ = self.clear();
    }
}

// Valid for backwards compat if needed, otherwise just this struct
pub type MediaSessionInner = MediaCenterInner;
