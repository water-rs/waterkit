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
mod sys;

pub use player::{AudioDevice, AudioPlayer, AudioPlayerBuilder, PlayerError, PlayerState, rodio};
pub use recorder::{AudioBuffer, AudioFormat, AudioRecorder, AudioRecorderBuilder, RecordError};

use std::time::Duration;

/// Metadata about the currently playing media.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaMetadata {
    /// Title of the media (e.g., song name).
    pub title: Option<String>,
    /// Artist or creator name.
    pub artist: Option<String>,
    /// Album name.
    pub album: Option<String>,
    /// URL to artwork image.
    pub artwork_url: Option<String>,
    /// Total duration of the media.
    pub duration: Option<Duration>,
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
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the artist.
    #[must_use]
    pub fn artist(mut self, artist: impl Into<String>) -> Self {
        self.artist = Some(artist.into());
        self
    }

    /// Set the album.
    #[must_use]
    pub fn album(mut self, album: impl Into<String>) -> Self {
        self.album = Some(album.into());
        self
    }

    /// Set the artwork URL.
    #[must_use]
    pub fn artwork_url(mut self, url: impl Into<String>) -> Self {
        self.artwork_url = Some(url.into());
        self
    }

    /// Set the duration.
    #[must_use]
    pub const fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }
}

/// Current playback state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
pub struct PlaybackState {
    /// Current playback status.
    pub status: PlaybackStatus,
    /// Current playback position.
    pub position: Option<Duration>,
    /// Playback rate (1.0 = normal speed).
    pub rate: f64,
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
#[derive(Debug, Clone)]
pub enum MediaError {
    /// Media control is not supported on this platform.
    NotSupported,
    /// Failed to initialize media session.
    InitializationFailed(String),
    /// Failed to update media state.
    UpdateFailed(String),
    /// Audio focus was not granted.
    AudioFocusDenied,
    /// An unknown error occurred.
    Unknown(String),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSupported => write!(f, "media control not supported on this platform"),
            Self::InitializationFailed(msg) => {
                write!(f, "failed to initialize media session: {msg}")
            }
            Self::UpdateFailed(msg) => write!(f, "failed to update media state: {msg}"),
            Self::AudioFocusDenied => write!(f, "audio focus was denied"),
            Self::Unknown(msg) => write!(f, "unknown error: {msg}"),
        }
    }
}

impl std::error::Error for MediaError {}

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

    /// Run the main event loop for the specified duration.
    ///
    /// On macOS, this runs `CFRunLoop` which is required for
    /// `MPRemoteCommandCenter` to receive and dispatch events in CLI apps.
    /// GUI apps using `AppKit` or `SwiftUI` do not need this.
    ///
    /// On other platforms, this simply sleeps for the duration.
    #[cfg(target_os = "macos")]
    pub fn run_loop(&self, duration: std::time::Duration) {
        self.inner.run_loop(duration);
    }

    /// Run the main event loop for the specified duration.
    ///
    /// On non-macOS platforms, this simply sleeps for the duration.
    #[cfg(not(target_os = "macos"))]
    pub fn run_loop(&self, duration: std::time::Duration) {
        std::thread::sleep(duration);
    }
}
