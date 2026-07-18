//! Cross-platform audio playback and recording.
//!
//! This crate provides a unified API for:
//! - **Playback**: Playing audio files with media center integration
//! - **Recording**: Capturing microphone input (async)
//!
//! Supports iOS, macOS, Android, Windows, and Linux.

#![warn(missing_docs)]

#[cfg(any(feature = "playback", feature = "streaming"))]
mod output;
#[cfg(any(feature = "playback", feature = "streaming"))]
mod playback_rate;
#[cfg(all(feature = "playback", target_os = "ios"))]
#[path = "player_ios.rs"]
mod player;
#[cfg(all(feature = "playback", not(target_os = "ios")))]
mod player;
#[cfg(feature = "recording")]
mod recorder;
#[cfg(feature = "playback")]
mod shutdown;
#[cfg(feature = "streaming")]
mod stream;
#[cfg(feature = "streaming")]
mod stream_output;
#[cfg(any(feature = "media-session", feature = "recording"))]
mod sys;

#[cfg(any(feature = "playback", feature = "streaming"))]
pub use output::{AudioDevice, AudioOutput, AudioStreamFormat, PlayerError};
#[cfg(all(feature = "playback", not(target_os = "ios")))]
pub use player::rodio;
#[cfg(feature = "playback")]
pub use player::{AudioPlayer, ListenerPose, PlaybackMode, SpatialPosition, SpatialScene};
#[cfg(feature = "recording")]
pub use recorder::{AudioBuffer, AudioFormat, AudioRecorder, AudioRecorderBuilder, RecordError};
#[cfg(feature = "playback")]
pub use shutdown::{ShutdownHandle, ShutdownReceiver};
#[cfg(feature = "streaming")]
pub use stream::{
    AacDecoderConfig, AacPacketDecoder, DecodedAudioFrame, EncodedAudioPacket, PacketAudioDecoder,
    PacketAudioError, PcmFrameError,
};
#[cfg(feature = "streaming")]
pub use stream_output::{
    SilenceSkipping, StreamingAudioControl, StreamingAudioPlayer, StreamingAudioProducer,
};

use std::sync::Arc;
use std::time::Duration;

/// Encoded artwork supplied to platform media controls.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaArtwork(Arc<[u8]>);

impl MediaArtwork {
    /// Create artwork from encoded image bytes.
    ///
    /// # Panics
    ///
    /// Panics when `encoded` is empty.
    #[must_use]
    pub fn new(encoded: impl Into<Vec<u8>>) -> Self {
        let encoded = encoded.into();
        assert!(!encoded.is_empty(), "media artwork must not be empty");
        Self(encoded.into())
    }

    /// Return the encoded image bytes.
    #[must_use]
    pub fn encoded(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for MediaArtwork {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MediaArtwork")
            .field("encoded_len", &self.0.len())
            .finish()
    }
}

/// Metadata about the currently playing media.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MediaMetadata {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    artwork: Option<MediaArtwork>,
    duration: Option<Duration>,
}

impl MediaMetadata {
    /// Create new empty metadata.
    #[must_use]
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the title.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the artist.
    #[must_use]
    pub fn with_artist(mut self, artist: impl Into<String>) -> Self {
        self.artist = Some(artist.into());
        self
    }

    /// Set the album.
    #[must_use]
    pub fn with_album(mut self, album: impl Into<String>) -> Self {
        self.album = Some(album.into());
        self
    }

    /// Set encoded artwork.
    #[must_use]
    pub fn with_artwork(mut self, artwork: MediaArtwork) -> Self {
        self.artwork = Some(artwork);
        self
    }

    /// Set the duration.
    #[must_use]
    pub const fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Get the title.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Get the artist.
    #[must_use]
    pub fn artist(&self) -> Option<&str> {
        self.artist.as_deref()
    }

    /// Get the album.
    #[must_use]
    pub fn album(&self) -> Option<&str> {
        self.album.as_deref()
    }

    /// Get encoded artwork.
    #[must_use]
    pub const fn artwork(&self) -> Option<&MediaArtwork> {
        self.artwork.as_ref()
    }

    /// Get the duration.
    #[must_use]
    pub const fn duration(&self) -> Option<Duration> {
        self.duration
    }
}

/// Current playback state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum PlaybackStatus {
    /// Media is currently playing.
    Playing,
    /// Media is paused.
    Paused,
    /// Media is stopped (no active playback).
    #[default]
    Stopped,
}

/// Playback state including position information.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct PlaybackState {
    status: PlaybackStatus,
    position: Option<Duration>,
    rate: f64,
    queue_navigation_controls: QueueNavigationControls,
}

impl PlaybackState {
    /// Create a new stopped state.
    #[must_use]
    pub const fn stopped() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            position: None,
            rate: 0.0,
            queue_navigation_controls: QueueNavigationControls::enabled(),
        }
    }

    /// Create a new playing state.
    #[must_use]
    pub const fn playing(position: Duration) -> Self {
        Self {
            status: PlaybackStatus::Playing,
            position: Some(position),
            rate: 1.0,
            queue_navigation_controls: QueueNavigationControls::enabled(),
        }
    }

    /// Create a new paused state.
    #[must_use]
    pub const fn paused(position: Duration) -> Self {
        Self {
            status: PlaybackStatus::Paused,
            position: Some(position),
            rate: 0.0,
            queue_navigation_controls: QueueNavigationControls::enabled(),
        }
    }

    /// Get the playback status.
    #[must_use]
    pub const fn status(&self) -> PlaybackStatus {
        self.status
    }

    /// Get the current playback position.
    #[must_use]
    pub const fn position(&self) -> Option<Duration> {
        self.position
    }

    /// Get the playback rate (1.0 = normal speed).
    #[must_use]
    pub const fn rate(&self) -> f64 {
        self.rate
    }

    /// Get queue navigation control availability.
    #[must_use]
    pub const fn queue_navigation_controls(&self) -> QueueNavigationControls {
        self.queue_navigation_controls
    }

    /// Set playback rate (1.0 = normal speed).
    #[must_use]
    pub const fn with_rate(mut self, rate: f64) -> Self {
        self.rate = rate;
        self
    }

    /// Set queue navigation control availability.
    #[must_use]
    pub const fn with_queue_navigation_controls(
        mut self,
        queue_navigation_controls: QueueNavigationControls,
    ) -> Self {
        self.queue_navigation_controls = queue_navigation_controls;
        self
    }
}

/// Queue navigation controls exposed to system media UIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct QueueNavigationControls {
    next_enabled: bool,
    previous_enabled: bool,
}

impl QueueNavigationControls {
    /// Enable both next and previous actions.
    #[must_use]
    pub const fn enabled() -> Self {
        Self {
            next_enabled: true,
            previous_enabled: true,
        }
    }

    /// Disable both next and previous actions.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            next_enabled: false,
            previous_enabled: false,
        }
    }

    /// Whether next is enabled.
    #[must_use]
    pub const fn next_enabled(self) -> bool {
        self.next_enabled
    }

    /// Whether previous is enabled.
    #[must_use]
    pub const fn previous_enabled(self) -> bool {
        self.previous_enabled
    }

    /// Set next enabled state.
    #[must_use]
    pub const fn with_next_enabled(mut self, next_enabled: bool) -> Self {
        self.next_enabled = next_enabled;
        self
    }

    /// Set previous enabled state.
    #[must_use]
    pub const fn with_previous_enabled(mut self, previous_enabled: bool) -> Self {
        self.previous_enabled = previous_enabled;
        self
    }
}

impl Default for QueueNavigationControls {
    fn default() -> Self {
        Self::enabled()
    }
}

/// Commands received from system media controls.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MediaCommand {
    /// Play command.
    Play,
    /// Pause command.
    Pause,
    /// Toggle play/pause.
    PlayPause,
    /// Stop command.
    Stop,
    /// Skip to next track.
    Next,
    /// Skip to previous track.
    Previous,
    /// Seek to a specific position.
    Seek(Duration),
    /// Seek forward by an amount.
    SeekForward(Duration),
    /// Seek backward by an amount.
    SeekBackward(Duration),
    /// Audio focus was regained after a previous interruption.
    AudioFocusGained,
    /// Audio focus was lost permanently.
    AudioFocusLost,
    /// Audio focus was lost temporarily; playback may resume on focus gain.
    AudioFocusLostTransient,
    /// Audio focus was lost temporarily and playback should duck.
    AudioFocusLostDuck,
    /// Audio output became noisy (for example, headphones were disconnected).
    AudioBecomingNoisy,
}

/// Errors that can occur with media control.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum MediaError {
    /// Media control is not supported on this platform.
    #[error("media control not supported on this platform")]
    Unsupported,
    /// Failed to initialize media session.
    #[error("failed to initialize media session: {0}")]
    InitializationFailed(String),
    /// Failed to update media state.
    #[error("failed to update media state: {0}")]
    UpdateFailed(String),
    /// Audio focus was not granted.
    #[error("audio focus was denied")]
    AudioFocusDenied,
    /// An unknown error occurred.
    #[error("unknown error: {0}")]
    Unknown(String),
}

/// Manager for media control and "Now Playing" information.
#[cfg(feature = "media-session")]
#[derive(Debug)]
pub struct MediaSession {
    inner: sys::MediaSessionInner,
}

#[cfg(feature = "media-session")]
impl MediaSession {
    /// Create a new media session.
    ///
    /// This registers the application with the system's media controls.
    ///
    /// # Errors
    /// Returns [`MediaError::InitializationFailed`] if the session cannot be created.
    pub fn new() -> Result<Self, MediaError> {
        Ok(Self {
            inner: sys::MediaSessionInner::new()?,
        })
    }

    /// Update the currently playing media metadata.
    ///
    /// # Errors
    /// Returns [`MediaError::UpdateFailed`] if the metadata update fails.
    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        self.inner.set_metadata(metadata)
    }

    /// Update the current playback state.
    ///
    /// # Errors
    /// Returns [`MediaError::UpdateFailed`] if the state update fails.
    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        self.inner.set_playback_state(state)
    }

    /// Request audio focus.
    ///
    /// Call this before starting playback. On some platforms (Android),
    /// this is required to properly integrate with other audio apps.
    ///
    /// # Errors
    /// Returns [`MediaError::AudioFocusDenied`] if focus is refused.
    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        self.inner.request_audio_focus()
    }

    /// Abandon audio focus.
    ///
    /// Call this when stopping playback to allow other apps to play audio.
    ///
    /// # Errors
    /// Returns [`MediaError::UpdateFailed`] if focus cannot be abandoned.
    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        self.inner.abandon_audio_focus()
    }

    /// Clear the current media session.
    ///
    /// This removes "Now Playing" information from system controls.
    ///
    /// # Errors
    /// Returns [`MediaError::UpdateFailed`] if the session cannot be cleared.
    pub fn clear(&self) -> Result<(), MediaError> {
        self.inner.clear()
    }

    /// Returns the event-driven stream of commands from system media controls.
    ///
    /// Cloned receivers distribute commands among consumers; one player should
    /// therefore designate exactly one command handler.
    #[must_use]
    pub fn command_receiver(&self) -> async_channel::Receiver<MediaCommand> {
        self.inner.command_receiver()
    }
}

#[cfg(test)]
mod tests {
    use super::{PlaybackState, QueueNavigationControls};
    use std::time::Duration;

    #[test]
    fn playback_state_enables_queue_navigation_by_default() {
        let state = PlaybackState::playing(Duration::from_secs(1));

        assert!(state.queue_navigation_controls().next_enabled());
        assert!(state.queue_navigation_controls().previous_enabled());
    }

    #[test]
    fn playback_state_can_disable_queue_navigation() {
        let state = PlaybackState::paused(Duration::from_secs(1))
            .with_queue_navigation_controls(QueueNavigationControls::disabled());

        assert!(!state.queue_navigation_controls().next_enabled());
        assert!(!state.queue_navigation_controls().previous_enabled());
    }
}
