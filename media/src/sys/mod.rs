//! Platform-specific media control implementations.

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;

#[cfg(target_os = "android")]
mod android;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

// Re-export platform implementations
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub(crate) use apple::{AudioPlayerInner, MediaSessionInner};

#[cfg(target_os = "android")]
pub(crate) use android::{AudioPlayerInner, MediaSessionInner};

#[cfg(target_os = "windows")]
pub(crate) use windows::{AudioPlayerInner, MediaSessionInner};

#[cfg(target_os = "linux")]
pub(crate) use linux::{AudioPlayerInner, MediaSessionInner};

// Fallback for unsupported platforms
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
mod fallback {
    use crate::player::PlayerState;
    use crate::{MediaCommandHandler, MediaError, MediaMetadata, PlaybackState, PlayerError};
    use std::path::Path;
    use std::sync::{Arc, RwLock};
    use std::time::Duration;

    #[derive(Debug)]
    pub struct MediaSessionInner;

    impl MediaSessionInner {
        pub fn new() -> Result<Self, MediaError> {
            Err(MediaError::NotSupported)
        }

        pub fn set_metadata(&self, _metadata: &MediaMetadata) -> Result<(), MediaError> {
            Err(MediaError::NotSupported)
        }

        pub fn set_playback_state(&self, _state: &PlaybackState) -> Result<(), MediaError> {
            Err(MediaError::NotSupported)
        }

        pub fn set_command_handler(
            &self,
            _handler: Box<dyn MediaCommandHandler>,
        ) -> Result<(), MediaError> {
            Err(MediaError::NotSupported)
        }

        pub fn request_audio_focus(&self) -> Result<(), MediaError> {
            Err(MediaError::NotSupported)
        }

        pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
            Err(MediaError::NotSupported)
        }

        pub fn clear(&self) -> Result<(), MediaError> {
            Err(MediaError::NotSupported)
        }
    }

    #[derive(Debug)]
    pub struct AudioPlayerInner;

    impl AudioPlayerInner {
        pub fn new() -> Result<Self, PlayerError> {
            Err(PlayerError::Unknown("platform not supported".into()))
        }

        pub fn play_file(&self, _path: &Path) -> Result<(), PlayerError> {
            Err(PlayerError::Unknown("platform not supported".into()))
        }

        pub fn play_url(&self, _url: &str) -> Result<(), PlayerError> {
            Err(PlayerError::Unknown("platform not supported".into()))
        }

        pub fn pause(&self) -> Result<(), PlayerError> {
            Err(PlayerError::Unknown("platform not supported".into()))
        }

        pub fn resume(&self) -> Result<(), PlayerError> {
            Err(PlayerError::Unknown("platform not supported".into()))
        }

        pub fn stop(&self) -> Result<(), PlayerError> {
            Err(PlayerError::Unknown("platform not supported".into()))
        }

        pub fn seek(&self, _position: Duration) -> Result<(), PlayerError> {
            Err(PlayerError::Unknown("platform not supported".into()))
        }

        pub fn position(&self) -> Option<Duration> {
            None
        }

        pub fn duration(&self) -> Option<Duration> {
            None
        }

        pub fn state(&self) -> PlayerState {
            PlayerState::Stopped
        }

        pub fn set_volume(&self, _volume: f32) -> Result<(), PlayerError> {
            Err(PlayerError::Unknown("platform not supported".into()))
        }

        pub fn update_now_playing(
            &self,
            _metadata: &MediaMetadata,
            _state: &PlaybackState,
        ) -> Result<(), PlayerError> {
            Err(PlayerError::Unknown("platform not supported".into()))
        }

        pub fn clear_now_playing(&self) -> Result<(), PlayerError> {
            Err(PlayerError::Unknown("platform not supported".into()))
        }

        pub fn register_command_handler(
            &self,
            _handler: Arc<RwLock<Option<Box<dyn MediaCommandHandler>>>>,
        ) {
        }

        pub fn run_loop(&self, duration: Duration) {
            std::thread::sleep(duration);
        }
    }
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub(crate) use fallback::{AudioPlayerInner, MediaSessionInner};

