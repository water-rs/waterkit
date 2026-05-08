//! Apple platform (iOS/macOS) media control implementation using swift-bridge.

#[cfg(target_os = "ios")]
use crate::PlayerError;
use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
use std::{sync::RwLock, time::Duration};

mod host_bridge;

#[swift_bridge::bridge]
mod ffi {
    #[swift_bridge(swift_repr = "struct")]
    struct MediaMetadataFFI {
        title: String,
        artist: String,
        album: String,
        artwork_url: String,
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
        fn media_session_init() -> MediaResultFFI;
        fn media_session_set_metadata(metadata: MediaMetadataFFI) -> MediaResultFFI;
        fn media_session_set_playback_state(state: PlaybackStateFFI) -> MediaResultFFI;
        fn media_session_request_audio_focus() -> MediaResultFFI;
        fn media_session_abandon_audio_focus() -> MediaResultFFI;
        fn media_session_clear() -> MediaResultFFI;
        fn media_session_register_command_handler();
        fn media_session_run_loop(duration_secs: f64);

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

    extern "Rust" {
        fn rust_on_play();
        fn rust_on_pause();
        fn rust_on_play_pause();
        fn rust_on_stop();
        fn rust_on_next();
        fn rust_on_previous();
        fn rust_on_seek_to(position_secs: f64);
        fn rust_on_seek_forward(secs: f64);
        fn rust_on_seek_backward(secs: f64);
        fn rust_on_audio_focus_gained();
        fn rust_on_audio_focus_lost();
        fn rust_on_audio_focus_lost_transient();
        fn rust_on_audio_focus_lost_duck();
        fn rust_on_audio_becoming_noisy();
    }
}

/// Global command queue for polling
static COMMAND_QUEUE: RwLock<Vec<MediaCommand>> = RwLock::new(Vec::new());

fn dispatch_command(cmd: MediaCommand) {
    if let Ok(mut queue) = COMMAND_QUEUE.write() {
        queue.push(cmd);
    }
}

fn rust_on_play() {
    dispatch_command(MediaCommand::Play);
}

fn rust_on_pause() {
    dispatch_command(MediaCommand::Pause);
}

fn rust_on_play_pause() {
    dispatch_command(MediaCommand::PlayPause);
}

fn rust_on_stop() {
    dispatch_command(MediaCommand::Stop);
}

fn rust_on_next() {
    dispatch_command(MediaCommand::Next);
}

fn rust_on_previous() {
    dispatch_command(MediaCommand::Previous);
}

fn rust_on_seek_to(position_secs: f64) {
    dispatch_command(MediaCommand::Seek(Duration::from_secs_f64(position_secs)));
}

fn rust_on_seek_forward(secs: f64) {
    dispatch_command(MediaCommand::SeekForward(Duration::from_secs_f64(secs)));
}

fn rust_on_seek_backward(secs: f64) {
    dispatch_command(MediaCommand::SeekBackward(Duration::from_secs_f64(secs)));
}

fn rust_on_audio_focus_gained() {
    dispatch_command(MediaCommand::AudioFocusGained);
}

fn rust_on_audio_focus_lost() {
    dispatch_command(MediaCommand::AudioFocusLost);
}

fn rust_on_audio_focus_lost_transient() {
    dispatch_command(MediaCommand::AudioFocusLostTransient);
}

fn rust_on_audio_focus_lost_duck() {
    dispatch_command(MediaCommand::AudioFocusLostDuck);
}

fn rust_on_audio_becoming_noisy() {
    dispatch_command(MediaCommand::AudioBecomingNoisy);
}

fn poll_next_command() -> Option<MediaCommand> {
    COMMAND_QUEUE.write().ok().and_then(|mut queue| {
        if queue.is_empty() {
            None
        } else {
            Some(queue.remove(0))
        }
    })
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
        artwork_url: metadata.artwork_url().unwrap_or_default().to_owned(),
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

#[derive(Debug)]
pub struct MediaSessionInner;

impl MediaSessionInner {
    pub fn new() -> Result<Self, MediaError> {
        convert_result(ffi::media_session_init())?;
        ffi::media_session_register_command_handler();
        Ok(Self)
    }

    #[allow(clippy::unused_self)]
    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        convert_result(ffi::media_session_set_metadata(metadata_to_ffi(metadata)))
    }

    #[allow(clippy::unused_self)]
    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        convert_result(ffi::media_session_set_playback_state(
            playback_state_to_ffi(state),
        ))
    }

    #[allow(clippy::unused_self)]
    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        convert_result(ffi::media_session_request_audio_focus())
    }

    #[allow(clippy::unused_self)]
    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        convert_result(ffi::media_session_abandon_audio_focus())
    }

    #[allow(clippy::unused_self)]
    pub fn clear(&self) -> Result<(), MediaError> {
        convert_result(ffi::media_session_clear())
    }
}

/// Media center integration for Apple platforms.
/// Uses `MPNowPlayingInfoCenter` and `MPRemoteCommandCenter`.
pub struct MediaCenterInner;

impl MediaCenterInner {
    pub fn new() -> Result<Self, MediaError> {
        MediaSessionInner::new()?;
        Ok(Self {})
    }

    #[allow(clippy::unused_self)]
    pub fn update(&self, metadata: &MediaMetadata, state: &PlaybackState) {
        let _ = ffi::media_session_set_metadata(metadata_to_ffi(metadata));
        let _ = ffi::media_session_set_playback_state(playback_state_to_ffi(state));
    }

    #[allow(clippy::unused_self)]
    pub fn clear(&self) {
        let _ = ffi::media_session_clear();
    }

    #[allow(clippy::unused_self)]
    pub fn run_loop(&self, duration: std::time::Duration) {
        // Register command handler to populate the queue
        ffi::media_session_register_command_handler();
        ffi::media_session_run_loop(duration.as_secs_f64());
    }

    #[allow(clippy::unused_self)]
    pub fn poll_command(&self) -> Option<crate::MediaCommand> {
        poll_next_command()
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
