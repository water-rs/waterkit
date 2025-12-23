//! Cross-platform audio player with media center integration.
//!
//! This module provides an ergonomic API for playing audio files and URLs
//! with automatic "Now Playing" integration on all supported platforms.

use crate::{MediaCommand, MediaCommandHandler, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Audio source to play.
#[derive(Debug, Clone)]
pub enum AudioSource {
    /// Local file path.
    File(std::path::PathBuf),
    /// Remote URL.
    Url(String),
}

impl<P: AsRef<Path>> From<P> for AudioSource {
    fn from(path: P) -> Self {
        Self::File(path.as_ref().to_path_buf())
    }
}

/// Current state of the audio player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlayerState {
    /// Player is stopped (no audio loaded).
    #[default]
    Stopped,
    /// Audio is currently playing.
    Playing,
    /// Audio is paused.
    Paused,
}

/// Errors that can occur during audio playback.
#[derive(Debug, Clone)]
pub enum PlayerError {
    /// Failed to load the audio source.
    LoadFailed(String),
    /// Playback operation failed.
    PlaybackFailed(String),
    /// The audio format is not supported.
    UnsupportedFormat(String),
    /// Media session error.
    MediaSessionError(MediaError),
    /// An unknown error occurred.
    Unknown(String),
}

impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoadFailed(msg) => write!(f, "failed to load audio: {msg}"),
            Self::PlaybackFailed(msg) => write!(f, "playback failed: {msg}"),
            Self::UnsupportedFormat(msg) => write!(f, "unsupported audio format: {msg}"),
            Self::MediaSessionError(err) => write!(f, "media session error: {err}"),
            Self::Unknown(msg) => write!(f, "unknown error: {msg}"),
        }
    }
}

impl std::error::Error for PlayerError {}

impl From<MediaError> for PlayerError {
    fn from(err: MediaError) -> Self {
        Self::MediaSessionError(err)
    }
}

/// Builder for creating an [`AudioPlayer`] with metadata.
#[derive(Debug, Clone, Default)]
pub struct AudioPlayerBuilder {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    artwork_url: Option<String>,
}

impl AudioPlayerBuilder {
    /// Create a new audio player builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the title for the audio.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the artist for the audio.
    #[must_use]
    pub fn artist(mut self, artist: impl Into<String>) -> Self {
        self.artist = Some(artist.into());
        self
    }

    /// Set the album for the audio.
    #[must_use]
    pub fn album(mut self, album: impl Into<String>) -> Self {
        self.album = Some(album.into());
        self
    }

    /// Set the artwork URL for the audio.
    #[must_use]
    pub fn artwork_url(mut self, url: impl Into<String>) -> Self {
        self.artwork_url = Some(url.into());
        self
    }

    /// Build the audio player.
    ///
    /// # Errors
    ///
    /// Returns an error if the audio player fails to initialize.
    pub fn build(self) -> Result<AudioPlayer, PlayerError> {
        AudioPlayer::with_metadata(MediaMetadata {
            title: self.title,
            artist: self.artist,
            album: self.album,
            artwork_url: self.artwork_url,
            duration: None,
        })
    }
}

/// Cross-platform audio player with media center integration.
///
/// # Example
///
/// ```no_run
/// use waterkit_media::AudioPlayer;
/// use std::time::Duration;
///
/// let player = AudioPlayer::new()
///     .title("My Song")
///     .artist("My Artist")
///     .build()?;
///
/// player.play_file("/path/to/song.mp3")?;
///
/// // Run event loop for media key support (required on macOS CLI apps)
/// player.run_loop(Duration::from_secs(30));
/// # Ok::<(), waterkit_media::PlayerError>(())
/// ```
pub struct AudioPlayer {
    inner: crate::sys::AudioPlayerInner,
    metadata: Arc<RwLock<MediaMetadata>>,
    #[allow(dead_code)]
    command_handler: Arc<RwLock<Option<Box<dyn MediaCommandHandler>>>>,
}

impl std::fmt::Debug for AudioPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioPlayer")
            .field("inner", &self.inner)
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

impl AudioPlayer {
    /// Create a new audio player builder.
    #[must_use]
    pub fn new() -> AudioPlayerBuilder {
        AudioPlayerBuilder::new()
    }

    /// Create an audio player with the given metadata.
    fn with_metadata(metadata: MediaMetadata) -> Result<Self, PlayerError> {
        let inner = crate::sys::AudioPlayerInner::new()?;
        Ok(Self {
            inner,
            metadata: Arc::new(RwLock::new(metadata)),
            command_handler: Arc::new(RwLock::new(None)),
        })
    }

    /// Play audio from a file path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be loaded or played.
    pub fn play_file(&self, path: impl AsRef<Path>) -> Result<(), PlayerError> {
        let path = path.as_ref();
        self.inner.play_file(path)?;
        self.update_now_playing()?;
        Ok(())
    }

    /// Play audio from a URL.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL cannot be loaded or played.
    pub fn play_url(&self, url: &str) -> Result<(), PlayerError> {
        self.inner.play_url(url)?;
        self.update_now_playing()?;
        Ok(())
    }

    /// Pause playback.
    ///
    /// # Errors
    ///
    /// Returns an error if pausing fails.
    pub fn pause(&self) -> Result<(), PlayerError> {
        self.inner.pause()?;
        self.update_now_playing()?;
        Ok(())
    }

    /// Resume playback.
    ///
    /// # Errors
    ///
    /// Returns an error if resuming fails.
    pub fn resume(&self) -> Result<(), PlayerError> {
        self.inner.resume()?;
        self.update_now_playing()?;
        Ok(())
    }

    /// Stop playback.
    ///
    /// # Errors
    ///
    /// Returns an error if stopping fails.
    pub fn stop(&self) -> Result<(), PlayerError> {
        self.inner.stop()?;
        self.inner.clear_now_playing()?;
        Ok(())
    }

    /// Seek to a specific position.
    ///
    /// # Errors
    ///
    /// Returns an error if seeking fails.
    pub fn seek(&self, position: Duration) -> Result<(), PlayerError> {
        self.inner.seek(position)?;
        self.update_now_playing()?;
        Ok(())
    }

    /// Get the current playback position.
    #[must_use]
    pub fn position(&self) -> Option<Duration> {
        self.inner.position()
    }

    /// Get the total duration of the current audio.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        self.inner.duration()
    }

    /// Check if audio is currently playing.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.inner.state() == PlayerState::Playing
    }

    /// Get the current player state.
    #[must_use]
    pub fn state(&self) -> PlayerState {
        self.inner.state()
    }

    /// Set the volume (0.0 to 1.0).
    ///
    /// # Errors
    ///
    /// Returns an error if setting volume fails.
    pub fn set_volume(&self, volume: f32) -> Result<(), PlayerError> {
        self.inner.set_volume(volume.clamp(0.0, 1.0))
    }

    /// Set the handler for media commands (from media keys, Control Center, etc.).
    ///
    /// The handler is called when the user interacts with system media controls.
    pub fn set_command_handler<F>(&self, handler: F)
    where
        F: Fn(MediaCommand) + Send + Sync + 'static,
    {
        let handler = ClosureHandler(handler);
        if let Ok(mut guard) = self.command_handler.write() {
            *guard = Some(Box::new(handler));
        }
        self.inner.register_command_handler(self.command_handler.clone());
    }

    /// Run the event loop for the specified duration.
    ///
    /// On macOS, this runs `CFRunLoop` which is required for media key events
    /// in CLI apps. GUI apps using AppKit/SwiftUI do not need this.
    ///
    /// On other platforms, this simply sleeps for the duration.
    pub fn run_loop(&self, duration: Duration) {
        self.inner.run_loop(duration);
    }

    /// Update the "Now Playing" information.
    fn update_now_playing(&self) -> Result<(), PlayerError> {
        let mut metadata = self.metadata.write().map_err(|e| {
            PlayerError::Unknown(format!("lock poisoned: {e}"))
        })?;
        
        // Update duration from the player
        metadata.duration = self.inner.duration();
        
        let state = match self.inner.state() {
            PlayerState::Playing => PlaybackState {
                status: PlaybackStatus::Playing,
                position: self.inner.position(),
                rate: 1.0,
            },
            PlayerState::Paused => PlaybackState {
                status: PlaybackStatus::Paused,
                position: self.inner.position(),
                rate: 0.0,
            },
            PlayerState::Stopped => PlaybackState::stopped(),
        };
        
        self.inner.update_now_playing(&metadata, &state)?;
        Ok(())
    }
}

/// Internal handler that wraps a closure.
struct ClosureHandler<F>(F);

impl<F> MediaCommandHandler for ClosureHandler<F>
where
    F: Fn(MediaCommand) + Send + Sync,
{
    fn on_command(&self, command: MediaCommand) {
        (self.0)(command);
    }
}

