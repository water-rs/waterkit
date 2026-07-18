use super::MediaSessionInner;
use crate::{
    MediaArtwork, MediaCommand, MediaError, MediaMetadata, PlaybackState, QueueNavigationControls,
};
use futures::{FutureExt, select_biased};
use std::{
    ffi::{CStr, c_char},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

#[repr(C)]
pub struct WaterKitAppleMediaCommandFFI {
    pub kind: i32,
    pub value_secs: f64,
}

pub struct WaterKitAppleMediaSessionHandle {
    session: Arc<MediaSessionInner>,
    artwork_generation: Arc<AtomicU64>,
    artwork_cancel: Option<async_channel::Sender<()>>,
    artwork_workers: Vec<JoinHandle<()>>,
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

const fn ffi_result_code(result: &Result<(), MediaError>) -> i32 {
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
    if duration_secs >= 0.0 {
        metadata = metadata.with_duration(Duration::from_secs_f64(duration_secs));
    }

    metadata
}

impl WaterKitAppleMediaSessionHandle {
    fn cancel_artwork_resolution(&mut self) {
        self.artwork_generation
            .fetch_add(1, Ordering::AcqRel)
            .checked_add(1)
            .expect("Apple artwork generation must fit u64");
        if let Some(cancel) = self.artwork_cancel.take() {
            cancel.close();
        }
    }

    fn reap_artwork_workers(&mut self) {
        let mut index = 0;
        while index < self.artwork_workers.len() {
            if self.artwork_workers[index].is_finished() {
                self.artwork_workers
                    .swap_remove(index)
                    .join()
                    .expect("Apple artwork resolver thread must not panic");
            } else {
                index += 1;
            }
        }
    }

    fn resolve_artwork(&mut self, artwork_url: Option<String>, metadata: MediaMetadata) {
        self.cancel_artwork_resolution();
        self.reap_artwork_workers();
        let generation = self
            .artwork_generation
            .fetch_add(1, Ordering::AcqRel)
            .checked_add(1)
            .expect("Apple artwork generation must fit u64");
        let Some(artwork_url) = artwork_url else {
            return;
        };

        let (cancel, cancel_receiver) = async_channel::bounded(1);
        self.artwork_cancel = Some(cancel);
        let session = Arc::clone(&self.session);
        let current_generation = Arc::clone(&self.artwork_generation);
        self.artwork_workers.push(std::thread::spawn(move || {
            futures::executor::block_on(async move {
                let fetch = async {
                    let response = zenwave::get(&artwork_url).await.map_err(|error| {
                        MediaError::UpdateFailed(format!(
                            "failed to fetch Apple media artwork through Zenwave: {error}"
                        ))
                    })?;
                    let bytes = response.into_body().into_bytes().await.map_err(|error| {
                        MediaError::UpdateFailed(format!(
                            "failed to read Apple media artwork through Zenwave: {error}"
                        ))
                    })?;
                    Ok::<_, MediaError>(MediaArtwork::new(bytes.to_vec()))
                }
                .fuse();
                let cancelled = cancel_receiver.recv().fuse();
                futures::pin_mut!(fetch, cancelled);

                let artwork = select_biased! {
                    _ = cancelled => return,
                    result = fetch => match result {
                        Ok(artwork) => artwork,
                        Err(error) => {
                            tracing::error!(%error, %artwork_url, "Apple media artwork resolution failed");
                            return;
                        }
                    },
                };
                if current_generation.load(Ordering::Acquire) != generation {
                    return;
                }
                let resolved = metadata.with_artwork(artwork);
                if let Err(error) = session.set_metadata(&resolved) {
                    tracing::error!(%error, "failed to apply resolved Apple media artwork");
                }
            });
        }));
    }

    fn shutdown_artwork_resolution(&mut self) {
        self.cancel_artwork_resolution();
        for worker in self.artwork_workers.drain(..) {
            worker
                .join()
                .expect("Apple artwork resolver thread must not panic");
        }
    }
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

const fn media_command_to_ffi(command: Option<&MediaCommand>) -> WaterKitAppleMediaCommandFFI {
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
pub unsafe extern "C" fn waterkit_audio_apple_media_session_init(
    result_out: *mut i32,
) -> *mut WaterKitAppleMediaSessionHandle {
    match MediaSessionInner::new() {
        Ok(session) => {
            unsafe { *result_out = APPLE_MEDIA_RESULT_SUCCESS };
            Box::into_raw(Box::new(WaterKitAppleMediaSessionHandle {
                session: Arc::new(session),
                artwork_generation: Arc::new(AtomicU64::new(0)),
                artwork_cancel: None,
                artwork_workers: Vec::new(),
            }))
        }
        Err(error) => {
            unsafe { *result_out = ffi_result_code(&Err(error)) };
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_set_metadata(
    handle: *mut WaterKitAppleMediaSessionHandle,
    title: *const c_char,
    artist: *const c_char,
    album: *const c_char,
    artwork_url: *const c_char,
    duration_secs: f64,
) -> i32 {
    let metadata = media_metadata_from_raw(title, artist, album, duration_secs);
    let handle = unsafe { &mut *handle };
    let result = handle.session.set_metadata(&metadata);
    if result.is_ok() {
        handle.resolve_artwork(optional_c_string(artwork_url), metadata);
    }
    ffi_result_code(&result)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_set_playback_state(
    handle: *mut WaterKitAppleMediaSessionHandle,
    status: u8,
    position_secs: f64,
    rate: f64,
    next_enabled: bool,
    previous_enabled: bool,
) -> i32 {
    let state =
        playback_state_from_raw(status, position_secs, rate, next_enabled, previous_enabled);
    let session = unsafe { &(*handle).session };
    ffi_result_code(&session.set_playback_state(&state))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_request_audio_focus(
    handle: *mut WaterKitAppleMediaSessionHandle,
) -> i32 {
    let session = unsafe { &(*handle).session };
    ffi_result_code(&session.request_audio_focus())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_abandon_audio_focus(
    handle: *mut WaterKitAppleMediaSessionHandle,
) -> i32 {
    let session = unsafe { &(*handle).session };
    ffi_result_code(&session.abandon_audio_focus())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_clear(
    handle: *mut WaterKitAppleMediaSessionHandle,
) -> i32 {
    let handle = unsafe { &mut *handle };
    handle.cancel_artwork_resolution();
    ffi_result_code(&handle.session.clear())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_wait_command(
    handle: *mut WaterKitAppleMediaSessionHandle,
) -> WaterKitAppleMediaCommandFFI {
    let session = unsafe { &(*handle).session };
    let command = session.command_receiver().recv_blocking().ok();
    media_command_to_ffi(command.as_ref())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_audio_apple_media_session_destroy(
    handle: *mut WaterKitAppleMediaSessionHandle,
) {
    let mut handle = unsafe { Box::from_raw(handle) };
    handle.shutdown_artwork_resolution();
    drop(handle);
}
