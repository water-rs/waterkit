//! Platform-specific integrations.
//!
//! - Audio playback: handled by rodio across all platforms
//! - Media center: platform-specific "Now Playing" integration
//! - Recording: cpal on desktop, native on mobile

use crate::{MediaCommand, MediaMetadata, PlaybackState};
use std::time::Duration;

// Recording - use cpal on desktop platforms, explicit mobile inner elsewhere
#[cfg(not(target_os = "ios"))]
mod desktop_record;
#[cfg(not(target_os = "ios"))]
pub use desktop_record::AudioRecorderInner;

#[cfg(target_os = "ios")]
mod mobile_record;
#[cfg(target_os = "ios")]
pub use mobile_record::AudioRecorderInner;

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;

#[cfg(target_os = "android")]
mod android;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
compile_error!("waterkit-audio supports only macOS, iOS, Android, Windows, and Linux.");

// Keep MediaSessionInner for backwards compatibility
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::MediaSessionInner;

#[cfg(target_os = "ios")]
pub use apple::{NativeAudioPlayerInner, NativeAudioPlayerState};

#[cfg(target_os = "android")]
pub use android::MediaSessionInner;

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
    inner: UnsupportedMediaCenter,
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
        let inner =
            android::MediaCenterInner::new().map_err(|e: crate::MediaError| e.to_string())?;

        #[cfg(not(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "android",
            target_os = "windows",
            target_os = "linux"
        )))]
        let inner = UnsupportedMediaCenter;

        Ok(Self { inner })
    }

    #[allow(clippy::missing_const_for_fn)] // Not const on all platforms
    pub fn update(&self, metadata: &MediaMetadata, state: &PlaybackState) {
        self.inner.update(metadata, state);
    }

    #[allow(
        clippy::let_unit_value,
        clippy::ignored_unit_patterns,
        let_underscore_drop
    )]
    pub fn clear(&self) {
        let _ = self.inner.clear();
    }

    // run_loop is now handled internally by platform implementations
    #[allow(clippy::missing_const_for_fn)] // Not const on all platforms
    pub(crate) fn run_loop(&self, duration: Duration) {
        self.inner.run_loop(duration);
    }

    #[allow(clippy::missing_const_for_fn)] // Not const on all platforms
    pub fn poll_command(&self) -> Option<MediaCommand> {
        self.inner.poll_command()
    }
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
#[derive(Debug)]
struct UnsupportedMediaCenter;

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
impl UnsupportedMediaCenter {
    fn update(&self, _metadata: &MediaMetadata, _state: &PlaybackState) {
        panic!("waterkit-audio supports only macOS, iOS, Android, Windows, and Linux.")
    }

    fn clear(&self) {
        panic!("waterkit-audio supports only macOS, iOS, Android, Windows, and Linux.")
    }

    fn run_loop(&self, _duration: Duration) {
        panic!("waterkit-audio supports only macOS, iOS, Android, Windows, and Linux.")
    }

    fn poll_command(&self) -> Option<MediaCommand> {
        panic!("waterkit-audio supports only macOS, iOS, Android, Windows, and Linux.")
    }
}
