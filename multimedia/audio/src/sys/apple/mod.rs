//! Apple platform (iOS/macOS) media control implementation using swift-bridge.

#[cfg(target_os = "ios")]
use crate::PlayerError;
use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
use std::{thread::JoinHandle, time::Duration};

#[cfg(feature = "apple-artwork")]
mod host_bridge;

#[swift_bridge::bridge]
mod ffi {
    #[swift_bridge(swift_repr = "struct")]
    struct MediaMetadataFFI {
        title: String,
        artist: String,
        album: String,
        artwork: Vec<u8>,
        duration_secs: f64,
    }

    #[swift_bridge(swift_repr = "struct")]
    struct PlaybackStateFFI {
        status: u8,
        position_secs: f64,
        rate: f64,
        next_enabled: bool,
        previous_enabled: bool,
    }

    enum MediaResultFFI {
        Success,
        InitializationFailed,
        UpdateFailed,
        AudioFocusDenied,
    }

    #[swift_bridge(swift_repr = "struct")]
    struct MediaSessionHandleFFI {
        result: MediaResultFFI,
        session_id: u64,
    }

    #[swift_bridge(swift_repr = "struct")]
    struct MediaCommandFFI {
        kind: u8,
        value_secs: f64,
    }

    enum PlayerResultFFI {
        Success,
        LoadFailed,
        PlaybackFailed,
        UnsupportedFormat,
    }

    #[swift_bridge(swift_repr = "struct")]
    struct PlayerStateFFI {
        state: u8,
        position_secs: f64,
        duration_secs: f64,
    }

    extern "Swift" {
        // Media session functions
        fn media_session_init() -> MediaSessionHandleFFI;
        fn media_session_set_metadata(
            session_id: u64,
            metadata: MediaMetadataFFI,
        ) -> MediaResultFFI;
        fn media_session_set_playback_state(
            session_id: u64,
            state: PlaybackStateFFI,
        ) -> MediaResultFFI;
        fn media_session_request_audio_focus(session_id: u64) -> MediaResultFFI;
        fn media_session_abandon_audio_focus(session_id: u64) -> MediaResultFFI;
        fn media_session_clear(session_id: u64) -> MediaResultFFI;
        fn media_session_wait_command(session_id: u64) -> MediaCommandFFI;

        // Audio player functions
        fn audio_player_init() -> PlayerResultFFI;
        fn audio_player_load_file(path: String) -> PlayerResultFFI;
        fn audio_player_load_url(url: String) -> PlayerResultFFI;
        fn audio_player_pause() -> PlayerResultFFI;
        fn audio_player_resume() -> PlayerResultFFI;
        fn audio_player_stop() -> PlayerResultFFI;
        fn audio_player_seek(position_secs: f64) -> PlayerResultFFI;
        fn audio_player_set_volume(volume: f32) -> PlayerResultFFI;
        fn audio_player_set_playback_rate(rate: f32) -> PlayerResultFFI;
        fn audio_player_set_preserve_pitch(preserve_pitch: bool) -> PlayerResultFFI;
        fn audio_player_get_state() -> PlayerStateFFI;
    }
}

fn convert_result(result: ffi::MediaResultFFI) -> Result<(), MediaError> {
    match result {
        ffi::MediaResultFFI::Success => Ok(()),
        ffi::MediaResultFFI::InitializationFailed => Err(MediaError::InitializationFailed(
            "Apple media session init failed".into(),
        )),
        ffi::MediaResultFFI::UpdateFailed => Err(MediaError::UpdateFailed(
            "Failed to update media state".into(),
        )),
        ffi::MediaResultFFI::AudioFocusDenied => Err(MediaError::AudioFocusDenied),
    }
}

fn media_command_from_ffi(
    command: &ffi::MediaCommandFFI,
) -> Result<Option<MediaCommand>, MediaError> {
    let duration = || {
        if command.value_secs.is_finite() && command.value_secs >= 0.0 {
            Ok(Duration::from_secs_f64(command.value_secs))
        } else {
            Err(MediaError::Unknown(format!(
                "Apple media command {} has invalid duration {}",
                command.kind, command.value_secs
            )))
        }
    };
    match command.kind {
        0 => Ok(None),
        1 => Ok(Some(MediaCommand::Play)),
        2 => Ok(Some(MediaCommand::Pause)),
        3 => Ok(Some(MediaCommand::PlayPause)),
        4 => Ok(Some(MediaCommand::Stop)),
        5 => Ok(Some(MediaCommand::Next)),
        6 => Ok(Some(MediaCommand::Previous)),
        7 => duration().map(MediaCommand::Seek).map(Some),
        8 => duration().map(MediaCommand::SeekForward).map(Some),
        9 => duration().map(MediaCommand::SeekBackward).map(Some),
        10 => Ok(Some(MediaCommand::AudioFocusGained)),
        11 => Ok(Some(MediaCommand::AudioFocusLost)),
        12 => Ok(Some(MediaCommand::AudioFocusLostTransient)),
        13 => Ok(Some(MediaCommand::AudioFocusLostDuck)),
        14 => Ok(Some(MediaCommand::AudioBecomingNoisy)),
        kind => Err(MediaError::Unknown(format!(
            "Apple media session emitted unknown command kind {kind}"
        ))),
    }
}

#[cfg(target_os = "ios")]
fn convert_player_result(result: ffi::PlayerResultFFI) -> Result<(), PlayerError> {
    match result {
        ffi::PlayerResultFFI::Success => Ok(()),
        ffi::PlayerResultFFI::LoadFailed => Err(PlayerError::LoadFailed(
            "Apple audio player failed to load media".into(),
        )),
        ffi::PlayerResultFFI::PlaybackFailed => Err(PlayerError::PlaybackFailed(
            "Apple audio player operation failed".into(),
        )),
        ffi::PlayerResultFFI::UnsupportedFormat => Err(PlayerError::UnsupportedFormat(
            "Apple audio player does not support this format".into(),
        )),
    }
}

#[cfg(target_os = "ios")]
fn player_state_from_ffi(state: &ffi::PlayerStateFFI) -> NativeAudioPlayerState {
    let status = match state.state {
        0 => PlaybackStatus::Stopped,
        1 => PlaybackStatus::Paused,
        2 => PlaybackStatus::Playing,
        value => panic!("waterkit-audio apple received unsupported player state {value}"),
    };

    NativeAudioPlayerState {
        status,
        position: (state.position_secs >= 0.0)
            .then(|| Duration::from_secs_f64(state.position_secs)),
        duration: (state.duration_secs >= 0.0)
            .then(|| Duration::from_secs_f64(state.duration_secs)),
    }
}

fn metadata_to_ffi(metadata: &MediaMetadata) -> ffi::MediaMetadataFFI {
    ffi::MediaMetadataFFI {
        title: metadata.title().unwrap_or_default().to_owned(),
        artist: metadata.artist().unwrap_or_default().to_owned(),
        album: metadata.album().unwrap_or_default().to_owned(),
        artwork: metadata
            .artwork()
            .map_or_else(Vec::new, |artwork| artwork.encoded().to_vec()),
        duration_secs: metadata
            .duration()
            .map_or(-1.0, |duration| duration.as_secs_f64()),
    }
}

fn playback_state_to_ffi(state: &PlaybackState) -> ffi::PlaybackStateFFI {
    let status = match state.status() {
        PlaybackStatus::Stopped => 0,
        PlaybackStatus::Paused => 1,
        PlaybackStatus::Playing => 2,
    };

    ffi::PlaybackStateFFI {
        status,
        position_secs: state
            .position()
            .map_or(-1.0, |position| position.as_secs_f64()),
        rate: state.rate(),
        next_enabled: state.queue_navigation_controls().next_enabled(),
        previous_enabled: state.queue_navigation_controls().previous_enabled(),
    }
}

pub struct MediaSessionInner {
    session_id: u64,
    command_receiver: async_channel::Receiver<MediaCommand>,
    command_worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for MediaSessionInner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MediaSessionInner")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl MediaSessionInner {
    pub fn new() -> Result<Self, MediaError> {
        let handle = ffi::media_session_init();
        convert_result(handle.result)?;
        if handle.session_id == 0 {
            return Err(MediaError::InitializationFailed(String::from(
                "Apple media session returned reserved identifier zero",
            )));
        }
        let (command_sender, command_receiver) = async_channel::unbounded();
        let session_id = handle.session_id;
        let command_worker = std::thread::spawn(move || {
            loop {
                match media_command_from_ffi(&ffi::media_session_wait_command(session_id)) {
                    Ok(Some(command)) => {
                        if command_sender.send_blocking(command).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        tracing::error!(%error, session_id, "invalid Apple media command");
                        break;
                    }
                }
            }
        });
        Ok(Self {
            session_id,
            command_receiver,
            command_worker: Some(command_worker),
        })
    }

    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        convert_result(ffi::media_session_set_metadata(
            self.session_id,
            metadata_to_ffi(metadata),
        ))
    }

    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        convert_result(ffi::media_session_set_playback_state(
            self.session_id,
            playback_state_to_ffi(state),
        ))
    }

    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        convert_result(ffi::media_session_request_audio_focus(self.session_id))
    }

    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        convert_result(ffi::media_session_abandon_audio_focus(self.session_id))
    }

    pub fn clear(&self) -> Result<(), MediaError> {
        convert_result(ffi::media_session_clear(self.session_id))
    }

    pub fn command_receiver(&self) -> async_channel::Receiver<MediaCommand> {
        self.command_receiver.clone()
    }
}

impl Drop for MediaSessionInner {
    fn drop(&mut self) {
        if let Err(error) = self.clear() {
            tracing::error!(%error, "failed to clear Apple media session during shutdown");
        }
        if let Some(worker) = self.command_worker.take() {
            worker
                .join()
                .expect("Apple media command worker must not panic during shutdown");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(target_os = "ios")]
pub struct NativeAudioPlayerState {
    pub status: PlaybackStatus,
    pub position: Option<Duration>,
    pub duration: Option<Duration>,
}

#[derive(Debug, Clone, Copy, Default)]
#[cfg(target_os = "ios")]
pub struct NativeAudioPlayerInner;

#[cfg(target_os = "ios")]
impl NativeAudioPlayerInner {
    pub fn new() -> Result<Self, PlayerError> {
        convert_player_result(ffi::audio_player_init())?;
        Ok(Self)
    }

    #[allow(
        clippy::unused_self,
        reason = "the zero-sized iOS player handle represents a process-wide native player"
    )]
    pub fn load_file(self, path: &str) -> Result<(), PlayerError> {
        convert_player_result(ffi::audio_player_load_file(path.to_owned()))
    }

    #[allow(
        clippy::unused_self,
        reason = "the zero-sized iOS player handle represents a process-wide native player"
    )]
    pub fn load_url(self, url: &str) -> Result<(), PlayerError> {
        convert_player_result(ffi::audio_player_load_url(url.to_owned()))
    }

    #[allow(
        clippy::unused_self,
        reason = "the zero-sized iOS player handle represents a process-wide native player"
    )]
    pub fn play(self) -> Result<(), PlayerError> {
        convert_player_result(ffi::audio_player_resume())
    }

    #[allow(
        clippy::unused_self,
        reason = "the zero-sized iOS player handle represents a process-wide native player"
    )]
    pub fn pause(self) -> Result<(), PlayerError> {
        convert_player_result(ffi::audio_player_pause())
    }

    #[allow(
        clippy::unused_self,
        reason = "the zero-sized iOS player handle represents a process-wide native player"
    )]
    pub fn stop(self) -> Result<(), PlayerError> {
        convert_player_result(ffi::audio_player_stop())
    }

    #[allow(
        clippy::unused_self,
        reason = "the zero-sized iOS player handle represents a process-wide native player"
    )]
    pub fn seek(self, position: Duration) -> Result<(), PlayerError> {
        convert_player_result(ffi::audio_player_seek(position.as_secs_f64()))
    }

    #[allow(
        clippy::unused_self,
        reason = "the zero-sized iOS player handle represents a process-wide native player"
    )]
    pub fn set_volume(self, volume: f32) -> Result<(), PlayerError> {
        convert_player_result(ffi::audio_player_set_volume(volume))
    }

    #[allow(
        clippy::unused_self,
        reason = "the zero-sized iOS player handle represents a process-wide native player"
    )]
    pub fn set_playback_rate(self, rate: f32) -> Result<(), PlayerError> {
        convert_player_result(ffi::audio_player_set_playback_rate(rate))
    }

    #[allow(
        clippy::unused_self,
        reason = "the zero-sized iOS player handle represents a process-wide native player"
    )]
    pub fn set_preserve_pitch(self, preserve_pitch: bool) -> Result<(), PlayerError> {
        convert_player_result(ffi::audio_player_set_preserve_pitch(preserve_pitch))
    }

    #[allow(
        clippy::unused_self,
        reason = "the zero-sized iOS player handle represents a process-wide native player"
    )]
    pub fn state(self) -> NativeAudioPlayerState {
        player_state_from_ffi(&ffi::audio_player_get_state())
    }
}
