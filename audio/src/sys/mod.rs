//! Platform-specific integrations.
//!
//! - Audio playback: handled by rodio across all platforms
//! - Media center: platform-specific "Now Playing" integration
//! - Recording: cpal on desktop, native on mobile

use crate::{MediaCommand, MediaMetadata, PlaybackState};
use std::time::Duration;

// Recording - use cpal on all desktop platforms
mod desktop_record;
pub use desktop_record::AudioRecorderInner;

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;

#[cfg(target_os = "android")]
mod android;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

// Keep MediaSessionInner for backwards compatibility
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::MediaSessionInner;

#[cfg(target_os = "android")]
pub(crate) use android::MediaSessionInner;

#[cfg(target_os = "windows")]
pub(crate) use windows::MediaSessionInner;

#[cfg(target_os = "linux")]
pub(crate) use linux::MediaSessionInner;

/// Platform-specific media center integration.
///
/// Handles "Now Playing" display and media command callbacks.
pub struct MediaCenterIntegration {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    inner: apple::MediaCenterInner,

    #[cfg(target_os = "windows")]
    inner: windows::MediaCenterInner,

    #[cfg(target_os = "linux")]
    inner: linux::MediaCenterInner,

    #[cfg(target_os = "android")]
    inner: android::MediaCenterInner,

    #[cfg(not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "android",
        target_os = "windows",
        target_os = "linux"
    )))]
    inner: FallbackMediaCenter,
}

impl MediaCenterIntegration {
    pub fn new() -> Result<Self, String> {
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        let inner = apple::MediaCenterInner::new().map_err(|e| e.to_string())?;

        #[cfg(target_os = "windows")]
        let inner = windows::MediaCenterInner::new().map_err(|e| e.to_string())?;

        #[cfg(target_os = "linux")]
        let inner = linux::MediaCenterInner::new().map_err(|e| e.to_string())?;

        #[cfg(target_os = "android")]
        let inner = android::MediaCenterInner::new().map_err(|e| e.to_string())?;

        #[cfg(not(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "android",
            target_os = "windows",
            target_os = "linux"
        )))]
        let inner = FallbackMediaCenter;

        Ok(Self { inner })
    }

    pub fn update(&self, metadata: &MediaMetadata, state: &PlaybackState) {
        self.inner.update(metadata, state);
    }

    pub fn clear(&self) {
        self.inner.clear();
    }

    pub fn run_loop(&self, duration: Duration) {
        self.inner.run_loop(duration);
    }

    pub fn poll_command(&self) -> Option<MediaCommand> {
        self.inner.poll_command()
    }
}

// Fallback for unsupported platforms
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
struct FallbackMediaCenter;

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
impl FallbackMediaCenter {
    fn update(&self, _metadata: &MediaMetadata, _state: &PlaybackState) {}
    fn clear(&self) {}
    fn run_loop(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
    fn poll_command(&self) -> Option<MediaCommand> {
        None
    }
}

// Also keep fallback MediaSessionInner for backwards compatibility
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
