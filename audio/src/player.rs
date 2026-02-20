//! Cross-platform audio player with media center integration.
//!
//! Uses `rodio` for audio playback on all platforms, with platform-specific
//! media center integrations (`MPNowPlayingInfoCenter`, SMTC, MPRIS, `MediaSession`).

use crate::shutdown::ShutdownHandle;
use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState};
use futures::Stream;
use lofty::prelude::*;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source, SpatialSink};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
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
    /// Spatial controls require a spatially configured player.
    #[error("spatial controls are not enabled for this player")]
    SpatialNotEnabled,
    /// Spatial configuration is invalid.
    #[error("invalid spatial configuration: {0}")]
    InvalidSpatialConfiguration(String),
    /// An unknown error occurred.
    #[error("unknown error: {0}")]
    Unknown(String),
}

impl From<MediaError> for PlayerError {
    fn from(err: MediaError) -> Self {
        Self::Unknown(err.to_string())
    }
}

/// Playback mode for audio output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlaybackMode {
    /// Preserve decoded source channels (default).
    #[default]
    PreserveSourceChannels,
    /// Binaural spatial rendering with listener/emitter controls.
    SpatialStereo,
}

/// A position in 3D audio space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SpatialPosition {
    x: f32,
    y: f32,
    z: f32,
}

impl SpatialPosition {
    /// Create a 3D position.
    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// Position at the origin.
    #[must_use]
    pub const fn center() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }

    /// X coordinate.
    #[must_use]
    pub const fn x(self) -> f32 {
        self.x
    }

    /// Y coordinate.
    #[must_use]
    pub const fn y(self) -> f32 {
        self.y
    }

    /// Z coordinate.
    #[must_use]
    pub const fn z(self) -> f32 {
        self.z
    }

    #[must_use]
    const fn as_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    #[must_use]
    const fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

/// Listener pose in 3D space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListenerPose {
    left_ear: SpatialPosition,
    right_ear: SpatialPosition,
}

impl Default for ListenerPose {
    fn default() -> Self {
        Self {
            left_ear: SpatialPosition::new(-1.0, 0.0, 0.0),
            right_ear: SpatialPosition::new(1.0, 0.0, 0.0),
        }
    }
}

impl ListenerPose {
    /// Create a listener pose.
    ///
    /// # Errors
    ///
    /// Returns an error when coordinates are non-finite or both ears overlap.
    pub fn new(left_ear: SpatialPosition, right_ear: SpatialPosition) -> Result<Self, PlayerError> {
        let pose = Self {
            left_ear,
            right_ear,
        };
        pose.validate()?;
        Ok(pose)
    }

    /// Left ear position.
    #[must_use]
    pub const fn left_ear(self) -> SpatialPosition {
        self.left_ear
    }

    /// Right ear position.
    #[must_use]
    pub const fn right_ear(self) -> SpatialPosition {
        self.right_ear
    }

    fn validate(self) -> Result<(), PlayerError> {
        if !self.left_ear.is_finite() {
            return Err(PlayerError::InvalidSpatialConfiguration(
                "left ear contains non-finite coordinates".into(),
            ));
        }
        if !self.right_ear.is_finite() {
            return Err(PlayerError::InvalidSpatialConfiguration(
                "right ear contains non-finite coordinates".into(),
            ));
        }

        let dx = self.left_ear.x() - self.right_ear.x();
        let dy = self.left_ear.y() - self.right_ear.y();
        let dz = self.left_ear.z() - self.right_ear.z();
        let distance_sq = dz.mul_add(dz, dx.mul_add(dx, dy * dy));

        if distance_sq <= f32::EPSILON {
            return Err(PlayerError::InvalidSpatialConfiguration(
                "left and right ear positions must not overlap".into(),
            ));
        }

        Ok(())
    }
}

/// Spatial scene parameters used for 3D audio rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialScene {
    emitter: SpatialPosition,
    listener: ListenerPose,
}

impl Default for SpatialScene {
    fn default() -> Self {
        Self {
            emitter: SpatialPosition::center(),
            listener: ListenerPose::default(),
        }
    }
}

impl SpatialScene {
    /// Create a spatial scene.
    ///
    /// # Errors
    ///
    /// Returns an error when emitter/listener coordinates are invalid.
    pub fn new(emitter: SpatialPosition, listener: ListenerPose) -> Result<Self, PlayerError> {
        let scene = Self { emitter, listener };
        scene.validate()?;
        Ok(scene)
    }

    /// Emitter position.
    #[must_use]
    pub const fn emitter(self) -> SpatialPosition {
        self.emitter
    }

    /// Listener pose.
    #[must_use]
    pub const fn listener(self) -> ListenerPose {
        self.listener
    }

    /// Return a new scene with updated emitter position.
    ///
    /// # Errors
    ///
    /// Returns an error when emitter coordinates are invalid.
    pub fn with_emitter(self, emitter: SpatialPosition) -> Result<Self, PlayerError> {
        Self::new(emitter, self.listener)
    }

    /// Return a new scene with updated listener pose.
    ///
    /// # Errors
    ///
    /// Returns an error when listener coordinates are invalid.
    pub fn with_listener(self, listener: ListenerPose) -> Result<Self, PlayerError> {
        Self::new(self.emitter, listener)
    }

    fn validate(self) -> Result<(), PlayerError> {
        if !self.emitter.is_finite() {
            return Err(PlayerError::InvalidSpatialConfiguration(
                "emitter contains non-finite coordinates".into(),
            ));
        }
        self.listener.validate()
    }
}

#[derive(Debug, Clone, Copy)]
enum SinkInit {
    Standard,
    Spatial(SpatialScene),
}

enum SinkBackend {
    Standard(Arc<Sink>),
    Spatial {
        sink: Arc<SpatialSink>,
        scene: Arc<RwLock<SpatialScene>>,
    },
}

impl SinkBackend {
    fn new(stream_handle: &OutputStreamHandle, init: SinkInit) -> Result<Self, PlayerError> {
        match init {
            SinkInit::Standard => {
                let sink = Sink::try_new(stream_handle)
                    .map_err(|e| PlayerError::OutputInitFailed(e.to_string()))?;
                Ok(Self::Standard(Arc::new(sink)))
            }
            SinkInit::Spatial(scene) => {
                scene.validate()?;
                let sink = SpatialSink::try_new(
                    stream_handle,
                    scene.emitter().as_array(),
                    scene.listener().left_ear().as_array(),
                    scene.listener().right_ear().as_array(),
                )
                .map_err(|e| PlayerError::OutputInitFailed(e.to_string()))?;
                Ok(Self::Spatial {
                    sink: Arc::new(sink),
                    scene: Arc::new(RwLock::new(scene)),
                })
            }
        }
    }

    const fn mode(&self) -> PlaybackMode {
        match self {
            Self::Standard(_) => PlaybackMode::PreserveSourceChannels,
            Self::Spatial { .. } => PlaybackMode::SpatialStereo,
        }
    }

    fn append<S>(&self, source: S)
    where
        S: Source + Send + 'static,
        f32: rodio::cpal::FromSample<S::Item>,
        S::Item: rodio::Sample + Send,
    {
        match self {
            Self::Standard(sink) => sink.append(source),
            Self::Spatial { sink, .. } => sink.append(source),
        }
    }

    fn play(&self) {
        match self {
            Self::Standard(sink) => sink.play(),
            Self::Spatial { sink, .. } => sink.play(),
        }
    }

    fn pause(&self) {
        match self {
            Self::Standard(sink) => sink.pause(),
            Self::Spatial { sink, .. } => sink.pause(),
        }
    }

    fn stop(&self) {
        match self {
            Self::Standard(sink) => sink.stop(),
            Self::Spatial { sink, .. } => sink.stop(),
        }
    }

    fn set_volume(&self, volume: f32) {
        match self {
            Self::Standard(sink) => sink.set_volume(volume),
            Self::Spatial { sink, .. } => sink.set_volume(volume),
        }
    }

    fn is_paused(&self) -> bool {
        match self {
            Self::Standard(sink) => sink.is_paused(),
            Self::Spatial { sink, .. } => sink.is_paused(),
        }
    }

    fn empty(&self) -> bool {
        match self {
            Self::Standard(sink) => sink.empty(),
            Self::Spatial { sink, .. } => sink.empty(),
        }
    }

    fn get_pos(&self) -> Duration {
        match self {
            Self::Standard(sink) => sink.get_pos(),
            Self::Spatial { sink, .. } => sink.get_pos(),
        }
    }

    fn try_seek(&self, position: Duration) -> Result<(), PlayerError> {
        let seek_result = match self {
            Self::Standard(sink) => sink.try_seek(position),
            Self::Spatial { sink, .. } => sink.try_seek(position),
        };

        seek_result.map_err(|e| PlayerError::PlaybackFailed(format!("seek failed: {e}")))
    }

    fn scene(&self) -> Result<SpatialScene, PlayerError> {
        match self {
            Self::Standard(_) => Err(PlayerError::SpatialNotEnabled),
            Self::Spatial { scene, .. } => scene
                .read()
                .map(|guard| *guard)
                .map_err(|_| PlayerError::PlaybackFailed("spatial scene lock poisoned".into())),
        }
    }

    fn set_scene(&self, next_scene: SpatialScene) -> Result<(), PlayerError> {
        next_scene.validate()?;

        match self {
            Self::Standard(_) => Err(PlayerError::SpatialNotEnabled),
            Self::Spatial { sink, scene } => {
                sink.set_emitter_position(next_scene.emitter().as_array());
                sink.set_left_ear_position(next_scene.listener().left_ear().as_array());
                sink.set_right_ear_position(next_scene.listener().right_ear().as_array());

                let mut guard = scene.write().map_err(|_| {
                    PlayerError::PlaybackFailed("spatial scene lock poisoned".into())
                })?;
                *guard = next_scene;
                drop(guard);
                Ok(())
            }
        }
    }
}

struct RuntimeHandles {
    stream_handle: OutputStreamHandle,
    media_center: Arc<crate::sys::MediaCenterIntegration>,
    shutdown_handle: ShutdownHandle,
    background_thread: JoinHandle<()>,
    command_receiver: async_channel::Receiver<MediaCommand>,
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
    sink: SinkBackend,

    // State
    metadata: MediaMetadata,
    media_center: Arc<crate::sys::MediaCenterIntegration>,

    // Deferred metadata updates: builder methods set this flag,
    // first action (play/pause/seek) flushes to media center
    metadata_dirty: AtomicBool,

    // Background worker
    shutdown_handle: ShutdownHandle,
    background_thread: Option<JoinHandle<()>>,
    command_receiver: async_channel::Receiver<MediaCommand>,
}

impl std::fmt::Debug for AudioPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioPlayer")
            .field("mode", &self.mode())
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

impl AudioPlayer {
    fn initialize_runtime() -> Result<RuntimeHandles, PlayerError> {
        let (handle_tx, handle_rx) = std::sync::mpsc::channel();
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
                        let _ = handle_tx.send(Err(PlayerError::OutputInitFailed(e.to_string())));
                        return;
                    }
                };

                if handle_tx.send(Ok(stream_handle)).is_err() {
                    return;
                }

                while !shutdown_rx.is_shutdown() {
                    mc.run_loop(Duration::from_millis(50));
                    if let Some(cmd) = mc.poll_command() {
                        let _ = cmd_tx.send_blocking(cmd);
                    }
                }
            })
        };

        let stream_handle = handle_rx
            .recv()
            .map_err(|_| PlayerError::OutputInitFailed("audio thread failed to start".into()))??;

        Ok(RuntimeHandles {
            stream_handle,
            media_center,
            shutdown_handle,
            background_thread,
            command_receiver: cmd_rx,
        })
    }

    fn open_with_sink(path: impl AsRef<Path>, sink_init: SinkInit) -> Result<Self, PlayerError> {
        let path = path.as_ref();
        let runtime = Self::initialize_runtime()?;
        let sink = SinkBackend::new(&runtime.stream_handle, sink_init)?;

        let file = File::open(path)
            .map_err(|e| PlayerError::LoadFailed(format!("{}: {e}", path.display())))?;
        let reader = BufReader::new(file);

        let source =
            Decoder::new(reader).map_err(|e| PlayerError::UnsupportedFormat(e.to_string()))?;

        let mut metadata = MediaMetadata::default();

        if let Some(d) = source.total_duration() {
            metadata.duration = Some(d);
        }

        if let Ok(tagged_file) = lofty::read_from_path(path)
            && let Some(tag) = tagged_file.primary_tag()
        {
            metadata.title = tag.title().map(String::from);
            metadata.artist = tag.artist().map(String::from);
            metadata.album = tag.album().map(String::from);
        }

        if metadata.title.is_none() {
            metadata.title = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        }

        sink.append(source);
        sink.pause();

        runtime
            .media_center
            .update(&metadata, &PlaybackState::paused(Duration::ZERO));

        Ok(Self {
            stream_handle: runtime.stream_handle,
            sink,
            metadata,
            media_center: runtime.media_center,
            metadata_dirty: AtomicBool::new(false),
            shutdown_handle: runtime.shutdown_handle,
            background_thread: Some(runtime.background_thread),
            command_receiver: runtime.command_receiver,
        })
    }

    #[allow(clippy::future_not_send)]
    async fn open_url_with_sink(url: &str, sink_init: SinkInit) -> Result<Self, PlayerError> {
        let response = zenwave::get(url)
            .await
            .map_err(|e| PlayerError::LoadFailed(format!("HTTP request failed: {e}")))?;

        let bytes =
            response.into_body().into_bytes().await.map_err(|e| {
                PlayerError::LoadFailed(format!("Failed to read response body: {e}"))
            })?;

        let runtime = Self::initialize_runtime()?;
        let sink = SinkBackend::new(&runtime.stream_handle, sink_init)?;

        let source = Decoder::new(std::io::Cursor::new(bytes))
            .map_err(|e| PlayerError::UnsupportedFormat(e.to_string()))?;

        let mut metadata = MediaMetadata::default();
        if let Some(d) = source.total_duration() {
            metadata.duration = Some(d);
        }
        metadata.title = Some(Self::title_from_url(url));

        sink.append(source);
        sink.pause();

        runtime
            .media_center
            .update(&metadata, &PlaybackState::paused(Duration::ZERO));

        Ok(Self {
            stream_handle: runtime.stream_handle,
            sink,
            metadata,
            media_center: runtime.media_center,
            metadata_dirty: AtomicBool::new(false),
            shutdown_handle: runtime.shutdown_handle,
            background_thread: Some(runtime.background_thread),
            command_receiver: runtime.command_receiver,
        })
    }

    fn title_from_url(url: &str) -> String {
        let path_without_query = url.split('?').next().unwrap_or(url);
        let last_segment = path_without_query
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .unwrap_or("Stream");

        if last_segment.is_empty() {
            "Stream".to_string()
        } else {
            last_segment.to_string()
        }
    }

    /// Open audio from a file path.
    ///
    /// This automatically extracts metadata (title, artist, album, artwork)
    /// from the file using `lofty`.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened or the audio output fails.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PlayerError> {
        Self::open_with_sink(path, SinkInit::Standard)
    }

    /// Open audio from a file path with spatial rendering enabled.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened, audio output fails,
    /// or the spatial scene is invalid.
    pub fn open_spatial(path: impl AsRef<Path>, scene: SpatialScene) -> Result<Self, PlayerError> {
        Self::open_with_sink(path, SinkInit::Spatial(scene))
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
        Self::open_url_with_sink(url, SinkInit::Standard).await
    }

    /// Open audio from a URL (async) with spatial rendering enabled.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL cannot be fetched, the audio format is unsupported,
    /// or the spatial scene is invalid.
    #[allow(clippy::future_not_send)]
    pub async fn open_url_spatial(url: &str, scene: SpatialScene) -> Result<Self, PlayerError> {
        Self::open_url_with_sink(url, SinkInit::Spatial(scene)).await
    }

    /// Get the active playback mode.
    #[must_use]
    pub const fn mode(&self) -> PlaybackMode {
        self.sink.mode()
    }

    /// Get the current spatial scene.
    ///
    /// # Errors
    ///
    /// Returns [`PlayerError::SpatialNotEnabled`] when the player was not opened in spatial mode.
    pub fn spatial_scene(&self) -> Result<SpatialScene, PlayerError> {
        self.sink.scene()
    }

    /// Set the full spatial scene.
    ///
    /// # Errors
    ///
    /// Returns [`PlayerError::SpatialNotEnabled`] when spatial mode is not enabled,
    /// or [`PlayerError::InvalidSpatialConfiguration`] when scene values are invalid.
    pub fn set_spatial_scene(&self, scene: SpatialScene) -> Result<(), PlayerError> {
        self.sink.set_scene(scene)
    }

    /// Set emitter position in spatial mode.
    ///
    /// # Errors
    ///
    /// Returns [`PlayerError::SpatialNotEnabled`] when spatial mode is not enabled,
    /// or [`PlayerError::InvalidSpatialConfiguration`] when coordinates are invalid.
    pub fn set_emitter_position(&self, position: SpatialPosition) -> Result<(), PlayerError> {
        let scene = self.sink.scene()?.with_emitter(position)?;
        self.sink.set_scene(scene)
    }

    /// Set listener ear positions in spatial mode.
    ///
    /// # Errors
    ///
    /// Returns [`PlayerError::SpatialNotEnabled`] when spatial mode is not enabled,
    /// or [`PlayerError::InvalidSpatialConfiguration`] when listener values are invalid.
    pub fn set_listener_pose(&self, pose: ListenerPose) -> Result<(), PlayerError> {
        let scene = self.sink.scene()?.with_listener(pose)?;
        self.sink.set_scene(scene)
    }

    /// Set stereo pan in spatial mode.
    ///
    /// `pan = -1.0` is full left, `pan = 1.0` is full right.
    ///
    /// # Errors
    ///
    /// Returns [`PlayerError::SpatialNotEnabled`] when spatial mode is not enabled,
    /// or [`PlayerError::InvalidSpatialConfiguration`] when `pan` is non-finite or out of range.
    pub fn set_pan(&self, pan: f32) -> Result<(), PlayerError> {
        if !pan.is_finite() {
            return Err(PlayerError::InvalidSpatialConfiguration(
                "pan must be a finite number".into(),
            ));
        }
        if !(-1.0..=1.0).contains(&pan) {
            return Err(PlayerError::InvalidSpatialConfiguration(
                "pan must be within [-1.0, 1.0]".into(),
            ));
        }

        let current_scene = self.sink.scene()?;
        let emitter = SpatialPosition::new(
            pan,
            current_scene.emitter().y(),
            current_scene.emitter().z(),
        );

        let next_scene = current_scene.with_emitter(emitter)?;
        self.sink.set_scene(next_scene)
    }

    // --- Builder Methods ---
    // These methods defer media center updates until the first action (play, pause, etc.)

    /// Set the title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.metadata.title = Some(title.into());
        self.metadata_dirty.store(true, Ordering::Release);
        self
    }

    /// Set the artist.
    #[must_use]
    pub fn artist(mut self, artist: impl Into<String>) -> Self {
        self.metadata.artist = Some(artist.into());
        self.metadata_dirty.store(true, Ordering::Release);
        self
    }

    /// Set the album.
    #[must_use]
    pub fn album(mut self, album: impl Into<String>) -> Self {
        self.metadata.album = Some(album.into());
        self.metadata_dirty.store(true, Ordering::Release);
        self
    }

    /// Set the artwork URL.
    #[must_use]
    pub fn artwork_url(mut self, url: impl Into<String>) -> Self {
        self.metadata.artwork_url = Some(url.into());
        self.metadata_dirty.store(true, Ordering::Release);
        self
    }

    // --- Playback Control ---

    /// Flush pending metadata updates to the media center.
    ///
    /// Called automatically before playback actions.
    fn flush_metadata(&self) {
        if self.metadata_dirty.swap(false, Ordering::AcqRel) {
            self.update_now_playing();
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
    pub const fn metadata(&self) -> &MediaMetadata {
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
    ///
    /// # Errors
    /// Returns an error if the audio host cannot enumerate output devices.
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
        drop(std::mem::take(&mut self.shutdown_handle));

        // Wait for background thread to exit cleanly
        if let Some(handle) = self.background_thread.take() {
            let _ = handle.join();
        }

        self.media_center.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{ListenerPose, SpatialPosition, SpatialScene};

    #[test]
    fn listener_pose_rejects_overlapping_ears() {
        let ear = SpatialPosition::new(0.0, 0.0, 0.0);
        let result = ListenerPose::new(ear, ear);
        assert!(result.is_err());
    }

    #[test]
    fn spatial_scene_rejects_non_finite_emitter() {
        let listener = ListenerPose::default();
        let emitter = SpatialPosition::new(f32::NAN, 0.0, 0.0);
        let result = SpatialScene::new(emitter, listener);
        assert!(result.is_err());
    }

    #[test]
    fn title_from_url_uses_last_path_segment() {
        let title =
            super::AudioPlayer::title_from_url("https://example.com/audio/track.mp3?token=abc");
        assert_eq!(title, "track.mp3");
    }
}
