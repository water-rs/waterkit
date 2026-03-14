use super::{MediaSessionInner, poll_next_command};
use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState, QueueNavigationControls};
use std::{
    ffi::{CStr, c_char},
    time::Duration,
};

#[repr(C)]
pub struct WaterKitAppleMediaCommandFFI {
    pub kind: i32,
    pub value_secs: f64,
}

const APPLE_MEDIA_RESULT_SUCCESS: i32 = 0;
const APPLE_MEDIA_RESULT_INITIALIZATION_FAILED: i32 = 1;
const APPLE_MEDIA_RESULT_UPDATE_FAILED: i32 = 2;
const APPLE_MEDIA_RESULT_AUDIO_FOCUS_DENIED: i32 = 3;
const APPLE_MEDIA_RESULT_UNKNOWN: i32 = 4;

const APPLE_MEDIA_COMMAND_NONE: i32 = 0;
const APPLE_MEDIA_COMMAND_PLAY: i32 = 1;
const APPLE_MEDIA_COMMAND_PAUSE: i32 = 2;
const APPLE_MEDIA_COMMAND_PLAY_PAUSE: i32 = 3;
const APPLE_MEDIA_COMMAND_STOP: i32 = 4;
const APPLE_MEDIA_COMMAND_NEXT: i32 = 5;
const APPLE_MEDIA_COMMAND_PREVIOUS: i32 = 6;
const APPLE_MEDIA_COMMAND_SEEK: i32 = 7;
const APPLE_MEDIA_COMMAND_SEEK_FORWARD: i32 = 8;
const APPLE_MEDIA_COMMAND_SEEK_BACKWARD: i32 = 9;
const APPLE_MEDIA_COMMAND_AUDIO_FOCUS_GAINED: i32 = 10;
const APPLE_MEDIA_COMMAND_AUDIO_FOCUS_LOST: i32 = 11;
const APPLE_MEDIA_COMMAND_AUDIO_FOCUS_LOST_TRANSIENT: i32 = 12;
const APPLE_MEDIA_COMMAND_AUDIO_FOCUS_LOST_DUCK: i32 = 13;
const APPLE_MEDIA_COMMAND_AUDIO_BECOMING_NOISY: i32 = 14;

fn ffi_result_code(result: Result<(), MediaError>) -> i32 {
    match result {
        Ok(()) => APPLE_MEDIA_RESULT_SUCCESS,
        Err(MediaError::InitializationFailed(_)) => APPLE_MEDIA_RESULT_INITIALIZATION_FAILED,
        Err(MediaError::UpdateFailed(_)) => APPLE_MEDIA_RESULT_UPDATE_FAILED,
        Err(MediaError::AudioFocusDenied) => APPLE_MEDIA_RESULT_AUDIO_FOCUS_DENIED,
        Err(MediaError::Unsupported | MediaError::Unknown(_)) => APPLE_MEDIA_RESULT_UNKNOWN,
    }
}

fn optional_c_string(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }

    let value = unsafe {
        CStr::from_ptr(value)
            .to_str()
            .expect("waterkit-audio apple media session strings must be valid UTF-8")
    };
    (!value.is_empty()).then(|| value.to_owned())
}

fn media_metadata_from_raw(
    title: *const c_char,
    artist: *const c_char,
    album: *const c_char,
    artwork_url: *const c_char,
    duration_secs: f64,
) -> MediaMetadata {
    let mut metadata = MediaMetadata::new();

    if let Some(title) = optional_c_string(title) {
        metadata = metadata.with_title(title);
    }
    if let Some(artist) = optional_c_string(artist) {
        metadata = metadata.with_artist(artist);
    }
    if let Some(album) = optional_c_string(album) {
        metadata = metadata.with_album(album);
    }
    if let Some(artwork_url) = optional_c_string(artwork_url) {
        metadata = metadata.with_artwork_url(artwork_url);
    }
    if duration_secs >= 0.0 {
        metadata = metadata.with_duration(Duration::from_secs_f64(duration_secs));
    }

    metadata
}

fn playback_state_from_raw(
    status: u8,
    position_secs: f64,
    rate: f64,
    next_enabled: bool,
    previous_enabled: bool,
) -> PlaybackState {
    assert!(
        rate.is_finite(),
        "waterkit-audio apple playback rate must be finite"
    );

    let position = (position_secs >= 0.0).then(|| Duration::from_secs_f64(position_secs));
    let controls = QueueNavigationControls::disabled()
        .with_next_enabled(next_enabled)
        .with_previous_enabled(previous_enabled);

    match status {
        0 => PlaybackState::stopped().with_queue_navigation_controls(controls),
        1 => PlaybackState::paused(position.unwrap_or(Duration::ZERO))
            .with_queue_navigation_controls(controls),
        2 => PlaybackState::playing(position.unwrap_or(Duration::ZERO))
            .with_rate(rate)
            .with_queue_navigation_controls(controls),
        _ => panic!("waterkit-audio apple received unsupported playback status {status}"),
    }
}

fn media_command_to_ffi(command: Option<MediaCommand>) -> WaterKitAppleMediaCommandFFI {
    match command {
        None => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_NONE,
            value_secs: 0.0,
        },
        Some(MediaCommand::Play) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_PLAY,
            value_secs: 0.0,
        },
        Some(MediaCommand::Pause) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_PAUSE,
            value_secs: 0.0,
        },
        Some(MediaCommand::PlayPause) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_PLAY_PAUSE,
            value_secs: 0.0,
        },
        Some(MediaCommand::Stop) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_STOP,
            value_secs: 0.0,
        },
        Some(MediaCommand::Next) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_NEXT,
            value_secs: 0.0,
        },
        Some(MediaCommand::Previous) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_PREVIOUS,
            value_secs: 0.0,
        },
        Some(MediaCommand::Seek(position)) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_SEEK,
            value_secs: position.as_secs_f64(),
        },
        Some(MediaCommand::SeekForward(delta)) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_SEEK_FORWARD,
            value_secs: delta.as_secs_f64(),
        },
        Some(MediaCommand::SeekBackward(delta)) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_SEEK_BACKWARD,
            value_secs: delta.as_secs_f64(),
        },
        Some(MediaCommand::AudioFocusGained) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_AUDIO_FOCUS_GAINED,
            value_secs: 0.0,
        },
        Some(MediaCommand::AudioFocusLost) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_AUDIO_FOCUS_LOST,
            value_secs: 0.0,
        },
        Some(MediaCommand::AudioFocusLostTransient) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_AUDIO_FOCUS_LOST_TRANSIENT,
            value_secs: 0.0,
        },
        Some(MediaCommand::AudioFocusLostDuck) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_AUDIO_FOCUS_LOST_DUCK,
            value_secs: 0.0,
        },
        Some(MediaCommand::AudioBecomingNoisy) => WaterKitAppleMediaCommandFFI {
            kind: APPLE_MEDIA_COMMAND_AUDIO_BECOMING_NOISY,
            value_secs: 0.0,
        },
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_init() -> i32 {
    ffi_result_code(MediaSessionInner::new().map(|_| ()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_set_metadata(
    title: *const c_char,
    artist: *const c_char,
    album: *const c_char,
    artwork_url: *const c_char,
    duration_secs: f64,
) -> i32 {
    let session = MediaSessionInner;
    let metadata = media_metadata_from_raw(title, artist, album, artwork_url, duration_secs);
    ffi_result_code(session.set_metadata(&metadata))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_set_playback_state(
    status: u8,
    position_secs: f64,
    rate: f64,
    next_enabled: bool,
    previous_enabled: bool,
) -> i32 {
    let session = MediaSessionInner;
    let state =
        playback_state_from_raw(status, position_secs, rate, next_enabled, previous_enabled);
    ffi_result_code(session.set_playback_state(&state))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_request_audio_focus() -> i32 {
    let session = MediaSessionInner;
    ffi_result_code(session.request_audio_focus())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_abandon_audio_focus() -> i32 {
    let session = MediaSessionInner;
    ffi_result_code(session.abandon_audio_focus())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_clear() -> i32 {
    let session = MediaSessionInner;
    ffi_result_code(session.clear())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_poll_command()
-> WaterKitAppleMediaCommandFFI {
    media_command_to_ffi(poll_next_command())
}
