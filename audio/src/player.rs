//! Cross-platform audio player with media center integration.
//!
//! Uses `rodio` for audio playback on all platforms, with platform-specific
//! media center integrations (`MPNowPlayingInfoCenter`, SMTC, MPRIS, `MediaSession`).

use crate::shutdown::ShutdownHandle;
use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState};
use futures::Stream;
use lofty::prelude::*;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::cell::Cell;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;
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

/// Errors that can occur during audio playback.
#[derive(Debug, thiserror::Error, Clone)]
pub enum PlayerError {
    /// Failed to initialize audio output.
    #[error("failed to init audio output: {0}")]
    OutputInitFailed(String),
    /// Failed to load the audio source.
    #[error("failed to load audio: {0}")]
    LoadFailed(String),
    /// Playback operation failed.
    #[error("playback failed: {0}")]
    PlaybackFailed(String),
    /// The audio format is not supported.
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    /// No audio device available.
    #[error("no audio device available")]
    NoDevice,
    /// An unknown error occurred.
    #[error("unknown error: {0}")]
    Unknown(String),
}

impl From<MediaError> for PlayerError {
    fn from(err: MediaError) -> Self {
        Self::Unknown(err.to_string())
    }
}

/// Cross-platform audio player with media center integration.
///
/// # Example
///
/// ```no_run
/// use waterkit_audio::AudioPlayer;
///
/// // Metadata is automatically extracted from the file
/// let mut player = AudioPlayer::open("song.mp3").unwrap();
/// player.play();
///
/// // Override metadata if needed
/// let mut player = AudioPlayer::open("song.mp3").unwrap()
///     .title("Custom Title")
///     .artist("Custom Artist");
/// ```
pub struct AudioPlayer {
    // Keep internal stream handle alive via sink, but we don't hold OutputStream directly
    // (it lives in the background thread)
    #[allow(dead_code)]
    stream_handle: OutputStreamHandle,
    sink: Arc<Sink>,

    // State
    metadata: MediaMetadata,
    media_center: Arc<crate::sys::MediaCenterIntegration>,

    // Deferred metadata updates: builder methods set this flag,
    // first action (play/pause/seek) flushes to media center
    metadata_dirty: Cell<bool>,

    // Background worker
    shutdown_handle: ShutdownHandle,
    background_thread: Option<JoinHandle<()>>,
    command_receiver: async_channel::Receiver<MediaCommand>,
}

impl std::fmt::Debug for AudioPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioPlayer")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

unsafe impl Send for AudioPlayer {}
unsafe impl Sync for AudioPlayer {}

impl AudioPlayer {
    /// Open audio from a file path.
    ///
    /// This automatically extracts metadata (title, artist, album, artwork)
    /// from the file using `lofty`.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened or the audio output fails.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PlayerError> {
        let path = path.as_ref();

        // 1. Initialize audio output in background thread (to keep OutputStream !Send contained)
        let (handle_tx, handle_rx) = std::sync::mpsc::channel();
        let (shutdown_handle, shutdown_rx) = ShutdownHandle::new();

        let media_center = Arc::new(
            crate::sys::MediaCenterIntegration::new()
                .map_err(|e| PlayerError::Unknown(format!("media center init failed: {e}")))?,
        );

        let (cmd_tx, cmd_rx) = async_channel::unbounded();

        let background_thread = {
            let mc = Arc::clone(&media_center);
            let tx = cmd_tx;

            std::thread::spawn(move || {
                // Create stream on this thread
                let (_stream, stream_handle) = match OutputStream::try_default() {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = handle_tx.send(Err(PlayerError::OutputInitFailed(e.to_string())));
                        return;
                    }
                };

                // Send handle back
                if handle_tx.send(Ok(stream_handle)).is_err() {
                    return;
                }

                // Run loop until shutdown is signaled
                if let Ok(local_mc) = crate::sys::MediaCenterIntegration::new() {
                    while !shutdown_rx.is_shutdown() {
                        // Run platform loop step
                        local_mc.run_loop(Duration::from_millis(50));

                        // Check for commands
                        if let Some(cmd) = mc.poll_command().or_else(|| local_mc.poll_command()) {
                            let _ = tx.send_blocking(cmd);
                        }
                    }
                }

                // _stream dropped here
            })
        };

        // Receive handle
        let stream_handle = handle_rx
            .recv()
            .map_err(|_| PlayerError::OutputInitFailed("audio thread failed to start".into()))??;

        let sink = Sink::try_new(&stream_handle)
            .map_err(|e| PlayerError::OutputInitFailed(e.to_string()))?;

        // 2. Load audio file
        let file = File::open(path)
            .map_err(|e| PlayerError::LoadFailed(format!("{}: {e}", path.display())))?;
        let reader = BufReader::new(file);

        let source =
            Decoder::new(reader).map_err(|e| PlayerError::UnsupportedFormat(e.to_string()))?;

        // 3. Extract metadata
        let mut metadata = MediaMetadata::default();

        // Get duration from decoder
        if let Some(d) = source.total_duration() {
            metadata.duration = Some(d);
        }

        // Try extracting tags with lofty
        if let Ok(tagged_file) = lofty::read_from_path(path)
            && let Some(tag) = tagged_file.primary_tag()
        {
            metadata.title = tag.title().map(String::from);
            metadata.artist = tag.artist().map(String::from);
            metadata.album = tag.album().map(String::from);
        }

        // Fallback to filename if title is missing
        if metadata.title.is_none() {
            metadata.title = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        }

        // 4. Setup playback
        sink.append(source);
        sink.pause(); // Start paused

        // Initial update
        media_center.update(&metadata, &PlaybackState::paused(Duration::ZERO));

        Ok(Self {
            stream_handle,
            sink: Arc::new(sink),
            metadata,
            media_center,
            metadata_dirty: Cell::new(false),
            shutdown_handle,
            background_thread: Some(background_thread),
            command_receiver: cmd_rx,
        })
    }

    /// Open audio from a URL (async).
    ///
    /// Fetches audio data from the URL and creates a player.
    /// Note: Metadata extraction from URL streams is limited compared to local files.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL cannot be fetched or the audio format is unsupported.
    #[allow(clippy::future_not_send)]
    pub async fn open_url(url: &str) -> Result<Self, PlayerError> {
        // Fetch audio data
        let response = zenwave::get(url)
            .await
            .map_err(|e| PlayerError::LoadFailed(format!("HTTP request failed: {e}")))?;

        let bytes =
            response.into_body().into_bytes().await.map_err(|e| {
                PlayerError::LoadFailed(format!("Failed to read response body: {e}"))
            })?;

        // Create a cursor for in-memory decoding
        let cursor = std::io::Cursor::new(bytes);

        // Initialize audio output and media center in background thread
        let (stream_handle_tx, stream_handle_rx) = std::sync::mpsc::channel();
        let (shutdown_handle, shutdown_rx) = ShutdownHandle::new();

        let media_center = Arc::new(
            crate::sys::MediaCenterIntegration::new()
                .map_err(|e| PlayerError::Unknown(format!("media center init failed: {e}")))?,
        );

        let (cmd_tx, cmd_rx) = async_channel::unbounded();

        let background_thread = {
            let mc = Arc::clone(&media_center);

            std::thread::spawn(move || {
                let (_stream, stream_handle) = match OutputStream::try_default() {
                    Ok(pair) => pair,
                    Err(e) => {
                        let _ = stream_handle_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let _ = stream_handle_tx.send(Ok(stream_handle));

                // Run loop until shutdown is signaled (fixes thread leak)
                if let Ok(local_mc) = crate::sys::MediaCenterIntegration::new() {
                    while !shutdown_rx.is_shutdown() {
                        local_mc.run_loop(Duration::from_millis(50));
                        if let Some(cmd) = mc.poll_command().or_else(|| local_mc.poll_command()) {
                            let _ = cmd_tx.send_blocking(cmd);
                        }
                    }
                }
                // _stream dropped here, thread exits cleanly
            })
        };

        let stream_handle = stream_handle_rx
            .recv()
            .map_err(|_| PlayerError::OutputInitFailed("Background thread died".into()))?
            .map_err(PlayerError::OutputInitFailed)?;

        let sink = Sink::try_new(&stream_handle)
            .map_err(|e| PlayerError::OutputInitFailed(e.to_string()))?;

        // Decode audio
        let source =
            Decoder::new(cursor).map_err(|e| PlayerError::UnsupportedFormat(e.to_string()))?;

        // Get duration if available
        let mut metadata = MediaMetadata::default();
        if let Some(d) = source.total_duration() {
            metadata.duration = Some(d);
        }

        // Use URL as fallback title
        metadata.title = Some(
            url.rsplit('/')
                .next()
                .unwrap_or("Stream")
                .split('?')
                .next()
                .unwrap_or("Stream")
                .to_string(),
        );

        // Setup playback
        sink.append(source);
        sink.pause(); // Start paused

        media_center.update(&metadata, &PlaybackState::paused(Duration::ZERO));

        Ok(Self {
            stream_handle,
            sink: Arc::new(sink),
            metadata,
            media_center,
            metadata_dirty: Cell::new(false),
            shutdown_handle,
            background_thread: Some(background_thread),
            command_receiver: cmd_rx,
        })
    }

    // --- Builder Methods ---
    // These methods defer media center updates until the first action (play, pause, etc.)

    /// Set the title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.metadata.title = Some(title.into());
        self.metadata_dirty.set(true);
        self
    }

    /// Set the artist.
    #[must_use]
    pub fn artist(mut self, artist: impl Into<String>) -> Self {
        self.metadata.artist = Some(artist.into());
        self.metadata_dirty.set(true);
        self
    }

    /// Set the album.
    #[must_use]
    pub fn album(mut self, album: impl Into<String>) -> Self {
        self.metadata.album = Some(album.into());
        self.metadata_dirty.set(true);
        self
    }

    /// Set the artwork URL.
    #[must_use]
    pub fn artwork_url(mut self, url: impl Into<String>) -> Self {
        self.metadata.artwork_url = Some(url.into());
        self.metadata_dirty.set(true);
        self
    }

    // --- Playback Control ---

    /// Flush pending metadata updates to the media center.
    ///
    /// Called automatically before playback actions.
    fn flush_metadata(&self) {
        if self.metadata_dirty.get() {
            self.update_now_playing();
            self.metadata_dirty.set(false);
        }
    }

    /// Start playback.
    pub fn play(&self) {
        self.flush_metadata();
        self.sink.play();
        self.update_now_playing();
    }

    /// Pause playback.
    pub fn pause(&self) {
        self.flush_metadata();
        self.sink.pause();
        self.update_now_playing();
    }

    /// Toggle playback state.
    pub fn toggle_play_pause(&self) {
        self.flush_metadata();
        if self.is_playing() {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Stop playback.
    pub fn stop(&self) {
        self.flush_metadata();
        self.sink.stop();
        self.media_center.clear();
        self.update_now_playing();
    }

    /// Seek to a specific position.
    pub fn seek(&self, position: Duration) {
        self.flush_metadata();
        let _ = self.sink.try_seek(position);
        self.update_now_playing();
    }

    /// Set volume (0.0 to 1.0).
    pub fn set_volume(&self, volume: f32) {
        self.sink.set_volume(volume.clamp(0.0, 1.0));
    }

    // --- State Queries ---

    /// Check if audio is currently playing.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        !self.sink.is_paused() && !self.sink.empty()
    }

    /// Check if audio is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    /// Check if the playlist is empty (playback finished).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sink.empty()
    }

    /// Get current playback position.
    pub fn position(&self) -> Duration {
        self.sink.get_pos()
    }

    /// Get total duration.
    #[must_use]
    pub const fn duration(&self) -> Option<Duration> {
        self.metadata.duration
    }

    /// Get the current metadata.
    pub fn metadata(&self) -> &MediaMetadata {
        &self.metadata
    }

    // --- Events ---

    /// Get a stream of media commands (Play, Pause, Next, etc.).
    ///
    /// This is runtime-agnostic and can be used with any async executor.
    pub fn commands(&self) -> impl Stream<Item = MediaCommand> + '_ {
        self.command_receiver.clone()
    }

    /// Handle a standard media command.
    ///
    /// Automatically performs the action (Play, Pause, Seek) for standard commands.
    /// You should call this when processing the command stream if you want default behavior.
    pub fn handle(&self, cmd: &MediaCommand) {
        match cmd {
            MediaCommand::Play => self.play(),
            MediaCommand::Pause => self.pause(),
            MediaCommand::PlayPause => self.toggle_play_pause(),
            MediaCommand::Stop => self.stop(),
            MediaCommand::Seek(pos) => self.seek(*pos),
            MediaCommand::SeekForward(delta) => {
                self.seek(self.position() + *delta);
            }
            MediaCommand::SeekBackward(delta) => {
                self.seek(self.position().saturating_sub(*delta));
            }
            _ => {} // Next/Prev handled by app
        }
    }

    // --- Internal ---

    fn update_now_playing(&self) {
        let state = if self.is_playing() {
            PlaybackState::playing(self.sink.get_pos())
        } else if self.sink.empty() {
            PlaybackState::stopped()
        } else {
            PlaybackState::paused(self.sink.get_pos())
        };

        self.media_center.update(&self.metadata, &state);
    }

    /// List available audio output devices.
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
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        // ShutdownHandle is dropped automatically, signaling background thread to exit.
        // We explicitly drop it first to ensure the signal is sent before we try to join.
        drop(std::mem::replace(&mut self.shutdown_handle, ShutdownHandle::default()));

        // Wait for background thread to exit cleanly
        if let Some(handle) = self.background_thread.take() {
            let _ = handle.join();
        }

        self.media_center.clear();
    }
}
