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
pub(crate) use apple::MediaSessionInner;

#[cfg(target_os = "android")]
pub(crate) use android::MediaSessionInner;

#[cfg(target_os = "windows")]
pub(crate) use windows::MediaSessionInner;

#[cfg(target_os = "linux")]
pub(crate) use linux::MediaSessionInner;

// Fallback for unsupported platforms
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
mod fallback {
    use crate::{MediaCommandHandler, MediaError, MediaMetadata, PlaybackState};

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
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub(crate) use fallback::MediaSessionInner;
