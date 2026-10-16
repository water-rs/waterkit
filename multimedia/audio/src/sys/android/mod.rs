//! Android media control implementation using JNI and `MediaSession`.

use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
use jni::objects::{Global, JObject, JString, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::thread::JoinHandle;
use std::time::Duration;
use waterkit_build::{AndroidError, DexHelper, dex_helper};

/// `waterkit.media.MediaSessionHelper`, compiled into the app's DEX by the
/// packager and loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.media.MediaSessionHelper");

impl From<AndroidError> for MediaError {
    fn from(error: AndroidError) -> Self {
        Self::InitializationFailed(error.to_string())
    }
}

/// Create an instance-owned media session helper using the Context.
fn create_session_with_context(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<Global<JObject<'static>>, MediaError> {
    let helper_class = HELPER.class(env, context)?;
    let helper = env
        .new_object(
            helper_class,
            jni_sig!("(Landroid/content/Context;)V"),
            &[JValue::Object(context)],
        )
        .map_err(|e| MediaError::InitializationFailed(format!("create MediaSessionHelper: {e}")))?;

    env.new_global_ref(helper).map_err(|e| {
        MediaError::InitializationFailed(format!("new_global_ref MediaSessionHelper: {e}"))
    })
}

/// Set metadata on the media session helper.
pub fn set_metadata_with_context(
    env: &mut Env<'_>,
    helper: &Global<JObject<'static>>,
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
        jni_str!("setMetadata"),
        jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[BJ)V"),
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
    env: &mut Env<'_>,
    helper: &Global<JObject<'static>>,
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
        jni_str!("setPlaybackState"),
        jni_sig!("(IJFZZ)V"),
        &[
            JValue::Int(status),
            JValue::Long(position_ms),
            #[allow(clippy::cast_possible_truncation)]
            JValue::Float(state.rate() as f32),
            JValue::Bool(state.queue_navigation_controls().next_enabled()),
            JValue::Bool(state.queue_navigation_controls().previous_enabled()),
        ],
    )
    .map_err(|e| MediaError::UpdateFailed(format!("setPlaybackState: {e}")))?;

    Ok(())
}

/// Request audio focus.
pub fn request_audio_focus_with_context(
    env: &mut Env<'_>,
    helper: &Global<JObject<'static>>,
) -> Result<(), MediaError> {
    let result = env
        .call_method(
            helper.as_obj(),
            jni_str!("requestAudioFocus"),
            jni_sig!("()Z"),
            &[],
        )
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
    env: &mut Env<'_>,
    helper: &Global<JObject<'static>>,
) -> Result<(), MediaError> {
    env.call_method(
        helper.as_obj(),
        jni_str!("abandonAudioFocus"),
        jni_sig!("()V"),
        &[],
    )
    .map_err(|e| MediaError::Unknown(format!("abandonAudioFocus: {e}")))?;

    Ok(())
}

/// Clear the media session.
pub fn clear_session(
    env: &mut Env<'_>,
    helper: &Global<JObject<'static>>,
) -> Result<(), MediaError> {
    env.call_method(
        helper.as_obj(),
        jni_str!("clearSession"),
        jni_sig!("()V"),
        &[],
    )
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
    env: &mut Env<'_>,
    helper: &Global<JObject<'static>>,
) -> Result<Option<MediaCommand>, MediaError> {
    let command_obj = env
        .call_method(
            helper.as_obj(),
            jni_str!("takeCommand"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .map_err(|e| MediaError::Unknown(format!("takeCommand: {e}")))?
        .l()
        .map_err(|e| MediaError::Unknown(format!("takeCommand result: {e}")))?;

    let command = env
        .as_cast::<JString>(&command_obj)
        .and_then(|command| command.try_to_string(env))
        .map_err(|e| MediaError::Unknown(format!("takeCommand decode string: {e}")))?;
    if command == "shutdown" {
        Ok(None)
    } else {
        parse_media_command(&command).map(Some)
    }
}

pub struct MediaSessionInner {
    vm: JavaVM,
    context: Global<JObject<'static>>,
    helper: Global<JObject<'static>>,
    command_receiver: async_channel::Receiver<MediaCommand>,
    command_worker: Option<JoinHandle<()>>,
}

impl core::fmt::Debug for MediaSessionInner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MediaSessionInner").finish_non_exhaustive()
    }
}

impl MediaSessionInner {
    fn with_attached_env<T>(
        &self,
        op: impl FnOnce(&mut Env<'_>, &JObject) -> Result<T, MediaError>,
    ) -> Result<T, MediaError> {
        self.vm
            .attach_current_thread(|env| op(env, self.context.as_obj()))
    }

    pub fn new() -> Result<Self, MediaError> {
        let android_context = ndk_context::android_context();
        let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
        let raw_context: jni::sys::jobject = android_context.context().cast();
        assert!(
            !raw_vm.is_null(),
            "waterkit-audio: ndk_context returned null JavaVM"
        );
        assert!(
            !raw_context.is_null(),
            "waterkit-audio: ndk_context returned null Android Context"
        );
        let vm = unsafe { JavaVM::from_raw(raw_vm) };

        let (context, helper) = vm.attach_current_thread(|env| -> Result<_, MediaError> {
            let context_ref = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
            let context = env.new_global_ref(&*context_ref).map_err(|e| {
                MediaError::InitializationFailed(format!("new_global_ref context failed: {e}"))
            })?;

            let helper = create_session_with_context(env, context.as_obj())?;
            Ok((context, helper))
        })?;
        let command_vm = vm.clone();
        let command_helper = vm.attach_current_thread(|env| -> Result<_, MediaError> {
            env.new_global_ref(helper.as_obj()).map_err(|error| {
                MediaError::InitializationFailed(format!(
                    "new_global_ref command helper failed: {error}"
                ))
            })
        })?;
        let (command_sender, command_receiver) = async_channel::unbounded();
        let command_worker = std::thread::spawn(move || {
            let result = command_vm.attach_current_thread(|env| -> Result<(), MediaError> {
                loop {
                    match take_command_with_context(env, &command_helper) {
                        Ok(Some(command)) => {
                            if command_sender.send_blocking(command).is_err() {
                                return Ok(());
                            }
                        }
                        Ok(None) => return Ok(()),
                        Err(error) => {
                            tracing::error!(%error, "Android media command delivery failed");
                            return Err(error);
                        }
                    }
                }
            });
            if let Err(error) = result {
                tracing::error!(%error, "Android media command thread failed");
            }
        });

        Ok(Self {
            vm,
            context,
            helper,
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
