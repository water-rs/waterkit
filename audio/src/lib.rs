//! Cross-platform audio playback and recording.
//!
//! This crate provides a unified API for:
//! - **Playback**: Playing audio files with media center integration
//! - **Recording**: Capturing microphone input (async)
//!
//! Supports iOS, macOS, Android, Windows, and Linux.

#![warn(missing_docs)]

mod player;
mod recorder;
mod shutdown;
mod sys;

pub use player::{
    AudioDevice, AudioPlayer, ListenerPose, PlaybackMode, PlayerError, SpatialPosition,
    SpatialScene, rodio,
};
pub use recorder::{AudioBuffer, AudioFormat, AudioRecorder, AudioRecorderBuilder, RecordError};
pub use shutdown::{ShutdownHandle, ShutdownReceiver};

use std::time::Duration;

/// Metadata about the currently playing media.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MediaMetadata {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    artwork_url: Option<String>,
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

    /// Set the artwork URL.
    #[must_use]
    pub fn with_artwork_url(mut self, url: impl Into<String>) -> Self {
        self.artwork_url = Some(url.into());
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

    /// Get the artwork URL.
    #[must_use]
    pub fn artwork_url(&self) -> Option<&str> {
        self.artwork_url.as_deref()
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
}

impl PlaybackState {
    /// Create a new stopped state.
    #[must_use]
    pub const fn stopped() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            position: None,
            rate: 0.0,
        }
    }

    /// Create a new playing state.
    #[must_use]
    pub const fn playing(position: Duration) -> Self {
        Self {
            status: PlaybackStatus::Playing,
            position: Some(position),
            rate: 1.0,
        }
    }

    /// Create a new paused state.
    #[must_use]
    pub const fn paused(position: Duration) -> Self {
        Self {
            status: PlaybackStatus::Paused,
            position: Some(position),
            rate: 0.0,
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
}

/// Errors that can occur with media control.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum MediaError {
    /// Media control is not supported on this platform.
    #[error("media control not supported on this platform")]
    NotSupported,
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

/// Handler for media commands from system controls.
pub trait MediaCommandHandler: Send + Sync {
    /// Handle a media command.
    fn on_command(&self, command: MediaCommand);
}

/// Manager for media control and "Now Playing" information.
#[derive(Debug)]
pub struct MediaSession {
    inner: sys::MediaSessionInner,
}

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

    // run_loop is now handled automatically in the background
}
