//! Apple platform (iOS/macOS) media control implementation using swift-bridge.

use crate::{MediaCommandHandler, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
use std::sync::RwLock;

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
        fn audio_player_play_file(path: String) -> PlayerResultFFI;
        fn audio_player_play_url(url: String) -> PlayerResultFFI;
        fn audio_player_pause() -> PlayerResultFFI;
        fn audio_player_resume() -> PlayerResultFFI;
        fn audio_player_stop() -> PlayerResultFFI;
        fn audio_player_seek(position_secs: f64) -> PlayerResultFFI;
        fn audio_player_set_volume(volume: f32) -> PlayerResultFFI;
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
    }
}

/// Global command handler (set via `set_command_handler`)
static COMMAND_HANDLER: RwLock<Option<Box<dyn MediaCommandHandler>>> = RwLock::new(None);

fn dispatch_command(cmd: crate::MediaCommand) {
    if let Ok(guard) = COMMAND_HANDLER.read() {
        if let Some(handler) = guard.as_ref() {
            handler.on_command(cmd);
        }
    }
}

fn rust_on_play() {
    dispatch_command(crate::MediaCommand::Play);
}

fn rust_on_pause() {
    dispatch_command(crate::MediaCommand::Pause);
}

fn rust_on_play_pause() {
    dispatch_command(crate::MediaCommand::PlayPause);
}

fn rust_on_stop() {
    dispatch_command(crate::MediaCommand::Stop);
}

fn rust_on_next() {
    dispatch_command(crate::MediaCommand::Next);
}

fn rust_on_previous() {
    dispatch_command(crate::MediaCommand::Previous);
}

fn rust_on_seek_to(position_secs: f64) {
    dispatch_command(crate::MediaCommand::Seek(std::time::Duration::from_secs_f64(position_secs)));
}

fn rust_on_seek_forward(secs: f64) {
    dispatch_command(crate::MediaCommand::SeekForward(std::time::Duration::from_secs_f64(secs)));
}

fn rust_on_seek_backward(secs: f64) {
    dispatch_command(crate::MediaCommand::SeekBackward(std::time::Duration::from_secs_f64(secs)));
}

fn convert_result(result: ffi::MediaResultFFI) -> Result<(), MediaError> {
    match result {
        ffi::MediaResultFFI::Success => Ok(()),
        ffi::MediaResultFFI::InitializationFailed => {
            Err(MediaError::InitializationFailed("Apple media session init failed".into()))
        }
        ffi::MediaResultFFI::UpdateFailed => {
            Err(MediaError::UpdateFailed("Failed to update media state".into()))
        }
        ffi::MediaResultFFI::AudioFocusDenied => Err(MediaError::AudioFocusDenied),
    }
}

#[derive(Debug)]
pub struct MediaSessionInner;

impl MediaSessionInner {
    pub fn new() -> Result<Self, MediaError> {
        convert_result(ffi::media_session_init())?;
        Ok(Self)
    }

    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        let ffi_metadata = ffi::MediaMetadataFFI {
            title: metadata.title.clone().unwrap_or_default(),
            artist: metadata.artist.clone().unwrap_or_default(),
            album: metadata.album.clone().unwrap_or_default(),
            artwork_url: metadata.artwork_url.clone().unwrap_or_default(),
            duration_secs: metadata
                .duration
                .map(|d| d.as_secs_f64())
                .unwrap_or(-1.0),
        };
        convert_result(ffi::media_session_set_metadata(ffi_metadata))
    }

    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        let status = match state.status {
            PlaybackStatus::Stopped => 0,
            PlaybackStatus::Paused => 1,
            PlaybackStatus::Playing => 2,
        };
        let ffi_state = ffi::PlaybackStateFFI {
            status,
            position_secs: state.position.map(|d| d.as_secs_f64()).unwrap_or(-1.0),
            rate: state.rate,
        };
        convert_result(ffi::media_session_set_playback_state(ffi_state))
    }

    pub fn set_command_handler(
        &self,
        handler: Box<dyn MediaCommandHandler>,
    ) -> Result<(), MediaError> {
        {
            let mut guard = COMMAND_HANDLER
                .write()
                .map_err(|e| MediaError::Unknown(format!("Lock poisoned: {e}")))?;
            *guard = Some(handler);
        }
        ffi::media_session_register_command_handler();
        Ok(())
    }

    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        convert_result(ffi::media_session_request_audio_focus())
    }

    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        convert_result(ffi::media_session_abandon_audio_focus())
    }

    pub fn clear(&self) -> Result<(), MediaError> {
        convert_result(ffi::media_session_clear())
    }

    /// Run the macOS run loop for the specified duration.
    /// This is required for `MPRemoteCommandCenter` to receive events in CLI apps.
    pub fn run_loop(&self, duration: std::time::Duration) {
        ffi::media_session_run_loop(duration.as_secs_f64());
    }
}



/// Media center integration for Apple platforms.
/// Uses MPNowPlayingInfoCenter and MPRemoteCommandCenter.
pub struct MediaCenterInner;

impl MediaCenterInner {
    pub fn new() -> Result<Self, String> {
        convert_result(ffi::media_session_init())
            .map_err(|e| e.to_string())?;
        Ok(Self)
    }
    
    pub fn update(&self, metadata: &MediaMetadata, state: &PlaybackState) {
        let ffi_metadata = ffi::MediaMetadataFFI {
            title: metadata.title.clone().unwrap_or_default(),
            artist: metadata.artist.clone().unwrap_or_default(),
            album: metadata.album.clone().unwrap_or_default(),
            artwork_url: metadata.artwork_url.clone().unwrap_or_default(),
            duration_secs: metadata.duration.map(|d| d.as_secs_f64()).unwrap_or(-1.0),
        };
        let _ = ffi::media_session_set_metadata(ffi_metadata);
        
        let status = match state.status {
            PlaybackStatus::Stopped => 0,
            PlaybackStatus::Paused => 1,
            PlaybackStatus::Playing => 2,
        };
        let ffi_state = ffi::PlaybackStateFFI {
            status,
            position_secs: state.position.map(|d| d.as_secs_f64()).unwrap_or(-1.0),
            rate: state.rate,
        };
        let _ = ffi::media_session_set_playback_state(ffi_state);
    }
    
    pub fn clear(&self) {
        let _ = ffi::media_session_clear();
    }
    
    pub fn set_command_handler(&self, handler: Box<dyn MediaCommandHandler>) {
        if let Ok(mut guard) = COMMAND_HANDLER.write() {
            *guard = Some(handler);
        }
        ffi::media_session_register_command_handler();
    }
    
    pub fn run_loop(&self, duration: std::time::Duration) {
        ffi::media_session_run_loop(duration.as_secs_f64());
    }
}
