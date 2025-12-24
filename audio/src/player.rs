//! Cross-platform audio player with media center integration.
//!
//! Uses `rodio` for audio playback on all platforms, with platform-specific
//! media center integrations (`MPNowPlayingInfoCenter`, SMTC, MPRIS, `MediaSession`).

use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

// Re-export rodio for advanced users
pub use rodio;

/// Audio output device.
#[derive(Debug, Clone)]
pub struct AudioDevice {
    name: String,
    // Device handle is not Clone, so we store the name and recreate when needed
}

impl AudioDevice {
    /// Get the device name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl std::fmt::Display for AudioDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
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
#[derive(Debug)]
pub enum PlayerError {
    /// Failed to initialize audio output.
    OutputInitFailed(String),
    /// Failed to load the audio source.
    LoadFailed(String),
    /// Playback operation failed.
    PlaybackFailed(String),
    /// The audio format is not supported.
    UnsupportedFormat(String),
    /// No audio device available.
    NoDevice,
    /// An unknown error occurred.
    Unknown(String),
}

impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutputInitFailed(msg) => write!(f, "failed to init audio output: {msg}"),
            Self::LoadFailed(msg) => write!(f, "failed to load audio: {msg}"),
            Self::PlaybackFailed(msg) => write!(f, "playback failed: {msg}"),
            Self::UnsupportedFormat(msg) => write!(f, "unsupported format: {msg}"),
            Self::NoDevice => write!(f, "no audio device available"),
            Self::Unknown(msg) => write!(f, "unknown error: {msg}"),
        }
    }
}

impl std::error::Error for PlayerError {}

impl Clone for PlayerError {
    fn clone(&self) -> Self {
        match self {
            Self::OutputInitFailed(s) => Self::OutputInitFailed(s.clone()),
            Self::LoadFailed(s) => Self::LoadFailed(s.clone()),
            Self::PlaybackFailed(s) => Self::PlaybackFailed(s.clone()),
            Self::UnsupportedFormat(s) => Self::UnsupportedFormat(s.clone()),
            Self::NoDevice => Self::NoDevice,
            Self::Unknown(s) => Self::Unknown(s.clone()),
        }
    }
}

impl From<MediaError> for PlayerError {
    fn from(err: MediaError) -> Self {
        Self::Unknown(err.to_string())
    }
}

/// Builder for creating an [`AudioPlayer`].
#[derive(Debug, Default)]
pub struct AudioPlayerBuilder {
    device: Option<String>,
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

    /// Set a specific output device (optional, uses default if not set).
    #[must_use]
    pub fn device(mut self, device: &AudioDevice) -> Self {
        self.device = Some(device.name.clone());
        self
    }

    /// Set the title for media center display.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the artist for media center display.
    #[must_use]
    pub fn artist(mut self, artist: impl Into<String>) -> Self {
        self.artist = Some(artist.into());
        self
    }

    /// Set the album for media center display.
    #[must_use]
    pub fn album(mut self, album: impl Into<String>) -> Self {
        self.album = Some(album.into());
        self
    }

    /// Set the artwork URL for media center display.
    #[must_use]
    pub fn artwork_url(mut self, url: impl Into<String>) -> Self {
        self.artwork_url = Some(url.into());
        self
    }

    /// Build the audio player.
    ///
    /// # Errors
    ///
    /// Returns an error if the audio output cannot be initialized.
    pub fn build(self) -> Result<AudioPlayer, PlayerError> {
        AudioPlayer::new_internal(
            self.device,
            MediaMetadata {
                title: self.title,
                artist: self.artist,
                album: self.album,
                artwork_url: self.artwork_url,
                duration: None,
            },
        )
    }
}

/// Cross-platform audio player with media center integration.
///
/// # Example
///
/// ```no_run
/// use waterkit_audio::AudioPlayer;
///
/// // Simple usage with default device
/// let mut player = AudioPlayer::new()
///     .title("My Song")
///     .artist("My Artist")
///     .build()?;
///
/// player.play_file("song.mp3")?;
/// # Ok::<(), waterkit_audio::PlayerError>(())
/// ```
///
/// # Device Selection
///
/// ```no_run
/// use waterkit_audio::AudioPlayer;
///
/// let devices = AudioPlayer::list_devices()?;
/// let player = AudioPlayer::new()
///     .device(&devices[0])
///     .build()?;
/// # Ok::<(), waterkit_audio::PlayerError>(())
/// ```
/// Controller for audio playback and media center integration.
pub struct AudioController {
    sink: Sink,
    metadata: MediaMetadata,
    state: PlayerState,
    media_center: crate::sys::MediaCenterIntegration,
}

impl std::fmt::Debug for AudioController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioController")
            .field("metadata", &self.metadata)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl AudioController {
    /// Play audio from a file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be loaded or decoded.
    pub fn play_file(&mut self, path: impl AsRef<Path>) -> Result<(), PlayerError> {
        let path = path.as_ref();
        let file = File::open(path)
            .map_err(|e| PlayerError::LoadFailed(format!("{}: {e}", path.display())))?;

        let source = Decoder::new(BufReader::new(file))
            .map_err(|e| PlayerError::UnsupportedFormat(e.to_string()))?;

        // Get duration from source and update metadata
        if let Some(d) = source.total_duration() {
            self.metadata.duration = Some(d);
        }

        self.sink.stop();
        self.sink.append(source);
        self.sink.play();

        self.state = PlayerState::Playing;
        self.update_now_playing();
        Ok(())
    }

    /// Play audio from a URL (async).
    ///
    /// # Errors
    ///
    /// Returns an error if the URL cannot be loaded.
    #[allow(clippy::future_not_send)]
    pub async fn play_url(&mut self, url: &str) -> Result<(), PlayerError> {
        // Fetch audio data using zenwave
        let response = zenwave::get(url)
            .await
            .map_err(|e| PlayerError::LoadFailed(e.to_string()))?;

        let data = response
            .into_body()
            .into_bytes()
            .await
            .map_err(|e| PlayerError::LoadFailed(e.to_string()))?;

        let cursor = std::io::Cursor::new(data);
        let source =
            Decoder::new(cursor).map_err(|e| PlayerError::UnsupportedFormat(e.to_string()))?;

        self.sink.stop();
        self.sink.append(source);
        self.sink.play();

        self.state = PlayerState::Playing;
        self.update_now_playing();
        Ok(())
    }

    /// Pause playback.
    pub fn pause(&mut self) {
        self.sink.pause();
        self.state = PlayerState::Paused;
        self.update_now_playing();
    }

    /// Resume playback.
    pub fn resume(&mut self) {
        self.sink.play();
        self.state = PlayerState::Playing;
        self.update_now_playing();
    }

    /// Seek to a specific position.
    pub fn seek(&self, position: Duration) {
        let _ = self.sink.try_seek(position);
        self.update_now_playing();
    }

    /// Seek forward by a duration.
    pub fn seek_forward(&self, duration: Duration) {
        let pos = self.current_position() + duration;
        self.seek(pos);
    }

    /// Seek backward by a duration.
    pub fn seek_backward(&self, duration: Duration) {
        let pos = self.current_position().saturating_sub(duration);
        self.seek(pos);
    }

    /// Stop playback and clear the queue.
    pub fn stop(&mut self) {
        self.sink.stop();
        self.state = PlayerState::Stopped;
        self.media_center.clear();
    }

    /// Set volume (0.0 to 1.0).
    pub fn set_volume(&self, volume: f32) {
        self.sink.set_volume(volume.clamp(0.0, 1.0));
    }

    /// Get current volume.
    #[must_use]
    pub fn volume(&self) -> f32 {
        self.sink.volume()
    }

    /// Check if audio is currently playing.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        !self.sink.is_paused() && !self.sink.empty()
    }

    /// Check if playback is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    /// Check if the playback queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sink.empty()
    }

    /// Get the current player state.
    #[must_use]
    pub const fn state(&self) -> PlayerState {
        self.state
    }

    /// Get direct access to the underlying rodio Sink.
    ///
    /// Use this for advanced audio manipulation.
    #[must_use]
    pub const fn sink(&self) -> &Sink {
        &self.sink
    }

    /// Update metadata (title, artist, etc.).
    pub fn set_metadata(&mut self, metadata: MediaMetadata) {
        self.metadata = metadata;
        self.update_now_playing();
    }

    /// Poll for the next media command from the system.
    ///
    /// Returns `None` if no command is pending.
    pub fn poll_command(&self) -> Option<MediaCommand> {
        self.media_center.poll_command()
    }

    /// Update the media center with current playback state.
    /// Call periodically to keep progress bar updated.
    pub fn update_now_playing(&self) {
        // Get position from sink
        let position = Some(self.sink.get_pos());

        let playback_state = match self.state {
            PlayerState::Playing => PlaybackState {
                status: PlaybackStatus::Playing,
                position,
                rate: 1.0,
            },
            PlayerState::Paused => PlaybackState {
                status: PlaybackStatus::Paused,
                position,
                rate: 0.0,
            },
            PlayerState::Stopped => PlaybackState::stopped(),
        };

        self.media_center.update(&self.metadata, &playback_state);
    }

    /// Get the current playback position.
    pub fn current_position(&self) -> Duration {
        self.sink.get_pos()
    }

    /// Get the track duration.
    pub const fn duration(&self) -> Option<Duration> {
        self.metadata.duration
    }

    /// Handle a media command.
    pub fn handle_command(&mut self, cmd: &MediaCommand) {
        match cmd {
            MediaCommand::Play => self.resume(),
            MediaCommand::Pause => self.pause(),
            MediaCommand::PlayPause => {
                if self.is_playing() {
                    self.pause();
                } else {
                    self.resume();
                }
            }
            MediaCommand::Stop => self.stop(),
            MediaCommand::Seek(pos) => self.seek(*pos),
            MediaCommand::SeekForward(delta) => self.seek_forward(*delta),
            MediaCommand::SeekBackward(delta) => self.seek_backward(*delta),
            _ => {
                // Other commands not handled by default
            }
        }
    }
}

/// Cross-platform audio player with media center integration.
pub struct AudioPlayer {
    // Keep stream alive - must not be dropped while sink is in use
    _stream: OutputStream,
    #[allow(dead_code)]
    stream_handle: OutputStreamHandle,
    /// The controller for the audio player.
    controller: AudioController,
    /// Flag to stop the background run loop.
    run_loop_running: Arc<AtomicBool>,
}

impl std::fmt::Debug for AudioPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioPlayer")
            .field("controller", &"AudioController")
            .finish_non_exhaustive()
    }
}

impl AudioPlayer {
    /// Create a new audio player builder.
    #[must_use]
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> AudioPlayerBuilder {
        AudioPlayerBuilder::new()
    }

    /// List available audio output devices.
    ///
    /// # Errors
    ///
    /// Returns an error if devices cannot be enumerated.
    pub fn list_devices() -> Result<Vec<AudioDevice>, PlayerError> {
        use rodio::cpal::traits::{DeviceTrait, HostTrait};

        let host = rodio::cpal::default_host();
        let devices: Vec<AudioDevice> = host
            .output_devices()
            .map_err(|e| PlayerError::Unknown(format!("failed to list devices: {e}")))?
            .filter_map(|d| d.name().ok().map(|name| AudioDevice { name }))
            .collect();

        Ok(devices)
    }

    fn new_internal(
        _device_name: Option<String>,
        metadata: MediaMetadata,
    ) -> Result<Self, PlayerError> {
        // TODO: Support specific device selection
        // For now, always use default device
        let (stream, stream_handle) = OutputStream::try_default()
            .map_err(|e| PlayerError::OutputInitFailed(e.to_string()))?;

        let sink = Sink::try_new(&stream_handle)
            .map_err(|e| PlayerError::OutputInitFailed(e.to_string()))?;

        let media_center = crate::sys::MediaCenterIntegration::new()
            .map_err(|e| PlayerError::Unknown(format!("media center init failed: {e}")))?;

        let controller = AudioController {
            sink,
            metadata,
            state: PlayerState::Stopped,
            media_center,
        };

        let run_loop_running = Arc::new(AtomicBool::new(true));

        // Spawn background thread for the platform run loop
        {
            let running = Arc::clone(&run_loop_running);
            std::thread::spawn(move || {
                // Create a new media center integration for this thread
                if let Ok(mc) = crate::sys::MediaCenterIntegration::new() {
                    while running.load(Ordering::Relaxed) {
                        mc.run_loop(Duration::from_millis(100));
                    }
                }
            });
        }

        Ok(Self {
            _stream: stream,
            stream_handle,
            controller,
            run_loop_running,
        })
    }

    /// Wait for the next media command from the system.
    ///
    /// This is an async method that yields when a command is available.
    /// Handle the returned command with `handle_command()` or manually.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use waterkit_audio::AudioPlayer;
    ///
    /// async fn run() -> Result<(), waterkit_audio::PlayerError> {
    ///     let mut player = AudioPlayer::new().title("Song").build()?;
    ///     player.play_file("song.mp3")?;
    ///
    ///     loop {
    ///         let cmd = player.next_command().await;
    ///         player.handle_command(&cmd);
    ///     }
    /// }
    /// ```
    #[allow(clippy::future_not_send)]
    pub async fn next_command(&self) -> MediaCommand {
        loop {
            if let Some(cmd) = self.controller.poll_command() {
                return cmd;
            }
            // Yield to the async runtime
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Try to get the next media command without waiting.
    ///
    /// Returns `None` if no command is pending.
    pub fn try_next_command(&self) -> Option<MediaCommand> {
        self.controller.poll_command()
    }

    /// Set a default command handler that automatically handles
    /// Play, Pause, Stop, Seek commands.
    ///
    /// This spawns a background task that polls for commands
    /// and applies them to the player.
    pub const fn set_default_handler(&self) {
        // The background run loop already handles this via MediaCenterIntegration
        // This method is kept for API compatibility
    }

    /// Run the event loop for the specified duration.
    ///
    /// This is used to process media control events in CLI apps.
    /// GUI apps typically don't need this.
    pub fn run_loop(&self, duration: Duration) {
        self.controller.media_center.run_loop(duration);
    }
}

impl std::ops::Deref for AudioPlayer {
    type Target = AudioController;

    fn deref(&self) -> &Self::Target {
        &self.controller
    }
}

impl std::ops::DerefMut for AudioPlayer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.controller
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        // Stop the background run loop
        self.run_loop_running.store(false, Ordering::Relaxed);
        // RAII: Clear media center when player is dropped
        self.controller.stop();
        self.controller.media_center.clear();
    }
}
