//! Android media control implementation using JNI and `MediaSession`.

use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JObject, JValue};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

/// Embedded DEX bytecode containing `MediaSessionHelper` class.
/// Generated at build time by kotlinc + D8.
const DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

struct DexArtifact(PathBuf);

impl DexArtifact {
    fn create(env: &mut JNIEnv, cache_dir: &JObject) -> Result<Self, MediaError> {
        let prefix = env.new_string("waterkit-media-").map_err(|error| {
            MediaError::InitializationFailed(format!("DEX prefix string failed: {error}"))
        })?;
        let suffix = env.new_string(".dex").map_err(|error| {
            MediaError::InitializationFailed(format!("DEX suffix string failed: {error}"))
        })?;
        let file_class = env.find_class("java/io/File").map_err(|error| {
            MediaError::InitializationFailed(format!("java.io.File lookup failed: {error}"))
        })?;
        let file = env
            .call_static_method(
                file_class,
                "createTempFile",
                "(Ljava/lang/String;Ljava/lang/String;Ljava/io/File;)Ljava/io/File;",
                &[
                    JValue::Object(&prefix),
                    JValue::Object(&suffix),
                    JValue::Object(cache_dir),
                ],
            )
            .map_err(|error| {
                MediaError::InitializationFailed(format!("temporary DEX creation failed: {error}"))
            })?
            .l()
            .map_err(|error| {
                MediaError::InitializationFailed(format!(
                    "temporary DEX file result failed: {error}"
                ))
            })?;
        let absolute_path = env
            .call_method(&file, "getAbsolutePath", "()Ljava/lang/String;", &[])
            .map_err(|error| {
                MediaError::InitializationFailed(format!(
                    "temporary DEX absolute path failed: {error}"
                ))
            })?
            .l()
            .map_err(|error| {
                MediaError::InitializationFailed(format!(
                    "temporary DEX absolute path result failed: {error}"
                ))
            })?;
        let path: String = env
            .get_string((&absolute_path).into())
            .map_err(|error| {
                MediaError::InitializationFailed(format!(
                    "temporary DEX path string failed: {error}"
                ))
            })?
            .into();
        let artifact = Self(PathBuf::from(path));
        std::fs::write(artifact.path(), DEX_BYTES).map_err(|error| {
            MediaError::InitializationFailed(format!("temporary DEX write failed: {error}"))
        })?;
        Ok(artifact)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for DexArtifact {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0) {
            tracing::error!(%error, path = %self.0.display(), "failed to remove temporary Android media DEX");
        }
    }
}

/// Initialize the DEX class loader. Must be called with a valid Context.
///
/// # Safety
///
/// The `context` must be a valid Android Context `JObject`.
fn create_class_loader(
    env: &mut JNIEnv,
    context: &JObject,
) -> Result<(GlobalRef, DexArtifact), MediaError> {
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

    let artifact = DexArtifact::create(env, &cache_dir)?;

    // Create DexClassLoader
    let dex_path_jstring = env
        .new_string(artifact.path().to_string_lossy())
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

    let class_loader = env
        .new_global_ref(class_loader)
        .map_err(|e| MediaError::InitializationFailed(format!("new_global_ref: {e}")))?;
    Ok((class_loader, artifact))
}

use jni::objects::JClass;

/// Get the `MediaSessionHelper` class.
fn get_helper_class<'a>(
    env: &mut JNIEnv<'a>,
    class_loader: &GlobalRef,
) -> Result<JClass<'a>, MediaError> {
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

/// Create an instance-owned media session helper using the Context.
fn create_session_with_context(
    env: &mut JNIEnv,
    context: &JObject,
) -> Result<(GlobalRef, DexArtifact), MediaError> {
    let (class_loader, artifact) = create_class_loader(env, context)?;
    let helper_class = get_helper_class(env, &class_loader)?;
    let helper = env
        .new_object(
            helper_class,
            "(Landroid/content/Context;)V",
            &[JValue::Object(context)],
        )
        .map_err(|e| MediaError::InitializationFailed(format!("create MediaSessionHelper: {e}")))?;

    let helper = env.new_global_ref(helper).map_err(|e| {
        MediaError::InitializationFailed(format!("new_global_ref MediaSessionHelper: {e}"))
    })?;
    Ok((helper, artifact))
}

/// Set metadata on the media session helper.
pub fn set_metadata_with_context(
    env: &mut JNIEnv,
    helper: &GlobalRef,
    metadata: &MediaMetadata,
) -> Result<(), MediaError> {
    let title = env
        .new_string(metadata.title().unwrap_or(""))
        .map_err(|e| MediaError::UpdateFailed(format!("new_string title: {e}")))?;
    let artist = env
        .new_string(metadata.artist().unwrap_or(""))
        .map_err(|e| MediaError::UpdateFailed(format!("new_string artist: {e}")))?;
    let album = env
        .new_string(metadata.album().unwrap_or(""))
        .map_err(|e| MediaError::UpdateFailed(format!("new_string album: {e}")))?;
    let artwork = env
        .byte_array_from_slice(
            metadata
                .artwork()
                .map_or(&[][..], |artwork| artwork.encoded()),
        )
        .map_err(|e| MediaError::UpdateFailed(format!("new_byte_array artwork: {e}")))?;

    #[allow(clippy::cast_possible_truncation)]
    let duration_ms = metadata.duration().map_or(-1, |d| d.as_millis() as i64);

    env.call_method(
        helper.as_obj(),
        "setMetadata",
        "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[BJ)V",
        &[
            JValue::Object(&title),
            JValue::Object(&artist),
            JValue::Object(&album),
            JValue::Object(&artwork),
            JValue::Long(duration_ms),
        ],
    )
    .map_err(|e| MediaError::UpdateFailed(format!("setMetadata: {e}")))?;

    Ok(())
}

/// Set playback state.
pub fn set_playback_state_with_context(
    env: &mut JNIEnv,
    helper: &GlobalRef,
    state: &PlaybackState,
) -> Result<(), MediaError> {
    let status = match state.status() {
        PlaybackStatus::Stopped => 0,
        PlaybackStatus::Paused => 1,
        PlaybackStatus::Playing => 2,
    };

    #[allow(clippy::cast_possible_truncation)]
    let position_ms = state.position().map_or(-1, |d| d.as_millis() as i64);

    env.call_method(
        helper.as_obj(),
        "setPlaybackState",
        "(IJFZZ)V",
        &[
            JValue::Int(status),
            JValue::Long(position_ms),
            #[allow(clippy::cast_possible_truncation)]
            JValue::Float(state.rate() as f32),
            JValue::Bool(u8::from(state.queue_navigation_controls().next_enabled())),
            JValue::Bool(u8::from(
                state.queue_navigation_controls().previous_enabled(),
            )),
        ],
    )
    .map_err(|e| MediaError::UpdateFailed(format!("setPlaybackState: {e}")))?;

    Ok(())
}

/// Request audio focus.
pub fn request_audio_focus_with_context(
    env: &mut JNIEnv,
    helper: &GlobalRef,
) -> Result<(), MediaError> {
    let result = env
        .call_method(helper.as_obj(), "requestAudioFocus", "()Z", &[])
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
pub fn abandon_audio_focus_with_context(
    env: &mut JNIEnv,
    helper: &GlobalRef,
) -> Result<(), MediaError> {
    env.call_method(helper.as_obj(), "abandonAudioFocus", "()V", &[])
        .map_err(|e| MediaError::Unknown(format!("abandonAudioFocus: {e}")))?;

    Ok(())
}

/// Clear the media session.
pub fn clear_session(env: &mut JNIEnv, helper: &GlobalRef) -> Result<(), MediaError> {
    env.call_method(helper.as_obj(), "clearSession", "()V", &[])
        .map_err(|e| MediaError::Unknown(format!("clearSession: {e}")))?;

    Ok(())
}

fn parse_media_command(raw: &str) -> Result<MediaCommand, MediaError> {
    match raw {
        "play" => Ok(MediaCommand::Play),
        "pause" => Ok(MediaCommand::Pause),
        "play_pause" => Ok(MediaCommand::PlayPause),
        "stop" => Ok(MediaCommand::Stop),
        "next" => Ok(MediaCommand::Next),
        "previous" => Ok(MediaCommand::Previous),
        "audio_focus_gained" => Ok(MediaCommand::AudioFocusGained),
        "audio_focus_lost" => Ok(MediaCommand::AudioFocusLost),
        "audio_focus_lost_transient" => Ok(MediaCommand::AudioFocusLostTransient),
        "audio_focus_lost_duck" => Ok(MediaCommand::AudioFocusLostDuck),
        "audio_becoming_noisy" => Ok(MediaCommand::AudioBecomingNoisy),
        _ if raw.starts_with("seek:") => {
            let millis = raw
                .split_once(':')
                .expect("seek command must contain ':' separator")
                .1
                .parse::<u64>()
                .map_err(|e| MediaError::Unknown(format!("invalid seek command `{raw}`: {e}")))?;
            Ok(MediaCommand::Seek(Duration::from_millis(millis)))
        }
        _ if raw.starts_with("seek_forward:") => {
            let millis = raw
                .split_once(':')
                .expect("seek_forward command must contain ':' separator")
                .1
                .parse::<u64>()
                .map_err(|e| {
                    MediaError::Unknown(format!("invalid seek_forward command `{raw}`: {e}"))
                })?;
            Ok(MediaCommand::SeekForward(Duration::from_millis(millis)))
        }
        _ if raw.starts_with("seek_backward:") => {
            let millis = raw
                .split_once(':')
                .expect("seek_backward command must contain ':' separator")
                .1
                .parse::<u64>()
                .map_err(|e| {
                    MediaError::Unknown(format!("invalid seek_backward command `{raw}`: {e}"))
                })?;
            Ok(MediaCommand::SeekBackward(Duration::from_millis(millis)))
        }
        _ => Err(MediaError::Unknown(format!(
            "unknown media command from Android helper: {raw}"
        ))),
    }
}

fn take_command_with_context(
    env: &mut JNIEnv,
    helper: &GlobalRef,
) -> Result<Option<MediaCommand>, MediaError> {
    let command_obj = env
        .call_method(helper.as_obj(), "takeCommand", "()Ljava/lang/String;", &[])
        .map_err(|e| MediaError::Unknown(format!("takeCommand: {e}")))?
        .l()
        .map_err(|e| MediaError::Unknown(format!("takeCommand result: {e}")))?;

    let command: String = env
        .get_string((&command_obj).into())
        .map_err(|e| MediaError::Unknown(format!("takeCommand get_string: {e}")))?
        .into();
    if command == "shutdown" {
        Ok(None)
    } else {
        parse_media_command(&command).map(Some)
    }
}

pub struct MediaSessionInner {
    vm: JavaVM,
    context: GlobalRef,
    helper: GlobalRef,
    dex_artifact: DexArtifact,
    command_receiver: async_channel::Receiver<MediaCommand>,
    command_worker: Option<JoinHandle<()>>,
}

impl core::fmt::Debug for MediaSessionInner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MediaSessionInner")
            .field("dex_path", &self.dex_artifact.path())
            .finish_non_exhaustive()
    }
}

impl MediaSessionInner {
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
        let android_context = ndk_context::android_context();

        let vm = unsafe { JavaVM::from_raw(android_context.vm().cast()) }.map_err(|e| {
            MediaError::InitializationFailed(format!("JavaVM::from_raw failed: {e}"))
        })?;

        let (context, helper, dex_artifact) = {
            let mut env = vm.attach_current_thread().map_err(|e| {
                MediaError::InitializationFailed(format!("attach_current_thread failed: {e}"))
            })?;

            let context_local =
                ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
            assert!(
                !context_local.is_null(),
                "waterkit-audio: ndk_context returned null Android Context"
            );
            let context = env.new_global_ref(&*context_local).map_err(|e| {
                MediaError::InitializationFailed(format!("new_global_ref context failed: {e}"))
            })?;

            let (helper, dex_artifact) = create_session_with_context(&mut env, context.as_obj())?;
            (context, helper, dex_artifact)
        };
        let command_vm = unsafe { JavaVM::from_raw(android_context.vm().cast()) }.map_err(|e| {
            MediaError::InitializationFailed(format!("command JavaVM::from_raw failed: {e}"))
        })?;
        let command_helper = helper.clone();
        let (command_sender, command_receiver) = async_channel::unbounded();
        let command_worker = std::thread::spawn(move || {
            let mut env = match command_vm.attach_current_thread() {
                Ok(env) => env,
                Err(error) => {
                    tracing::error!(%error, "Android media command thread could not attach to JVM");
                    return;
                }
            };
            loop {
                match take_command_with_context(&mut env, &command_helper) {
                    Ok(Some(command)) => {
                        if command_sender.send_blocking(command).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        tracing::error!(%error, "Android media command delivery failed");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            vm,
            context,
            helper,
            dex_artifact,
            command_receiver,
            command_worker: Some(command_worker),
        })
    }

    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        self.with_attached_env(|env, _context| {
            set_metadata_with_context(env, &self.helper, metadata)
        })
    }

    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        self.with_attached_env(|env, _context| {
            set_playback_state_with_context(env, &self.helper, state)
        })
    }

    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        self.with_attached_env(|env, _context| request_audio_focus_with_context(env, &self.helper))
    }

    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        self.with_attached_env(|env, _context| abandon_audio_focus_with_context(env, &self.helper))
    }

    pub fn clear(&self) -> Result<(), MediaError> {
        self.with_attached_env(|env, _context| clear_session(env, &self.helper))
    }

    pub fn command_receiver(&self) -> async_channel::Receiver<MediaCommand> {
        self.command_receiver.clone()
    }
}

impl Drop for MediaSessionInner {
    fn drop(&mut self) {
        if let Err(error) = self.clear() {
            tracing::error!(%error, "failed to clear Android media session during shutdown");
        }
        if let Some(command_worker) = self.command_worker.take() {
            command_worker
                .join()
                .expect("Android media command worker must not panic during shutdown");
        }
    }
}
