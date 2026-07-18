//! Cross-platform audio player with media center integration.
//!
//! Uses `rodio` for audio playback on all platforms, with platform-specific
//! media center integrations (`MPNowPlayingInfoCenter`, SMTC, MPRIS, `MediaSession`).

use crate::playback_rate::{
    AdaptivePlaybackSource, PlaybackParams, clamp_playback_rate, duration_div_rate,
    duration_mul_rate, sink_speed_for_playback,
};
#[cfg(test)]
use crate::playback_rate::{PitchStretchEngine, should_use_pitch_stretch};
use crate::shutdown::ShutdownHandle;
use crate::{
    AudioDevice, AudioOutput, AudioStreamFormat, MediaArtwork, MediaCommand, MediaMetadata,
    MediaSession, PlaybackState, PlayerError,
};
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

    fn set_speed(&self, speed: f32) {
        match self {
            Self::Standard(sink) => sink.set_speed(speed),
            Self::Spatial { sink, .. } => sink.set_speed(speed),
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

#[derive(Debug, Clone, Copy)]
struct PlaybackClock {
    rate: f32,
    sink_anchor: Duration,
    source_anchor: Duration,
}

impl PlaybackClock {
    #[cfg(test)]
    const fn new() -> Self {
        Self {
            rate: 1.0,
            sink_anchor: Duration::ZERO,
            source_anchor: Duration::ZERO,
        }
    }

    const fn at_start(rate: f32) -> Self {
        Self {
            rate,
            sink_anchor: Duration::ZERO,
            source_anchor: Duration::ZERO,
        }
    }

    fn source_position(self, sink_position: Duration) -> Duration {
        if sink_position >= self.sink_anchor {
            let delta = sink_position.saturating_sub(self.sink_anchor);
            self.source_anchor
                .saturating_add(duration_mul_rate(delta, self.rate))
        } else {
            let delta = self.sink_anchor.saturating_sub(sink_position);
            self.source_anchor
                .saturating_sub(duration_mul_rate(delta, self.rate))
        }
    }

    fn sink_position(self, source_position: Duration) -> Duration {
        if source_position >= self.source_anchor {
            let delta = source_position.saturating_sub(self.source_anchor);
            self.sink_anchor
                .saturating_add(duration_div_rate(delta, self.rate))
        } else {
            let delta = self.source_anchor.saturating_sub(source_position);
            self.sink_anchor
                .saturating_sub(duration_div_rate(delta, self.rate))
        }
    }

    fn reanchor_for_rate_change(&mut self, sink_position: Duration, next_rate: f32) {
        let source_position = self.source_position(sink_position);
        self.sink_anchor = sink_position;
        self.source_anchor = source_position;
        self.rate = next_rate;
    }

    const fn reanchor_after_seek(&mut self, sink_position: Duration, source_position: Duration) {
        self.sink_anchor = sink_position;
        self.source_anchor = source_position;
    }
}

struct RuntimeHandles {
    stream_handle: OutputStreamHandle,
    media_session: Arc<MediaSession>,
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
    source_format: Option<AudioStreamFormat>,
    output_format: Option<AudioStreamFormat>,
    playback_params: Arc<PlaybackParams>,
    playback_clock: RwLock<PlaybackClock>,
    media_session: Arc<MediaSession>,

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
            .field("source_format", &self.source_format)
            .field("output_format", &self.output_format)
            .finish_non_exhaustive()
    }
}

impl AudioPlayer {
    fn initialize_runtime(output: &AudioOutput) -> Result<RuntimeHandles, PlayerError> {
        let (handle_tx, handle_rx) = std::sync::mpsc::channel();
        let (shutdown_handle, shutdown_rx) = ShutdownHandle::new();
        let selected_device = output.selected_device().cloned();

        let media_session = Arc::new(MediaSession::new()?);

        let command_receiver = media_session.command_receiver();

        let background_thread = {
            std::thread::spawn(move || {
                let stream = selected_device.map_or_else(OutputStream::try_default, |device| {
                    OutputStream::try_from_device(&device.handle)
                });
                let (_stream, stream_handle) = match stream {
                    Ok(pair) => pair,
                    Err(e) => {
                        let _ = handle_tx.send(Err(PlayerError::OutputInitFailed(e.to_string())));
                        return;
                    }
                };

                if handle_tx.send(Ok(stream_handle)).is_err() {
                    return;
                }

                shutdown_rx.wait_blocking();
            })
        };

        let stream_handle = handle_rx
            .recv()
            .map_err(|_| PlayerError::OutputInitFailed("audio thread failed to start".into()))??;

        Ok(RuntimeHandles {
            stream_handle,
            media_session,
            shutdown_handle,
            background_thread,
            command_receiver,
        })
    }

    fn open_with_sink(
        path: impl AsRef<Path>,
        sink_init: SinkInit,
        output: &AudioOutput,
    ) -> Result<Self, PlayerError> {
        let path = path.as_ref();
        let runtime = Self::initialize_runtime(output)?;
        let sink = SinkBackend::new(&runtime.stream_handle, sink_init)?;

        let file = File::open(path)
            .map_err(|e| PlayerError::LoadFailed(format!("{}: {e}", path.display())))?;
        let reader = BufReader::new(file);

        let source =
            Decoder::new(reader).map_err(|e| PlayerError::UnsupportedFormat(e.to_string()))?;
        let source_format = Some(AudioStreamFormat {
            channels: source.channels(),
            sample_rate_hz: source.sample_rate(),
        });

        let mut metadata = MediaMetadata::default();

        if let Some(d) = source.total_duration() {
            metadata = metadata.with_duration(d);
        }

        if let Ok(tagged_file) = lofty::read_from_path(path)
            && let Some(tag) = tagged_file.primary_tag()
        {
            if let Some(title) = tag.title() {
                metadata = metadata.with_title(title.to_string());
            }
            if let Some(artist) = tag.artist() {
                metadata = metadata.with_artist(artist.to_string());
            }
            if let Some(album) = tag.album() {
                metadata = metadata.with_album(album.to_string());
            }
        }

        if metadata.title().is_none()
            && let Some(stem) = path.file_stem()
        {
            metadata = metadata.with_title(stem.to_string_lossy().into_owned());
        }

        let playback_params = Arc::new(PlaybackParams::new());
        let playback_source = AdaptivePlaybackSource::new(
            source.convert_samples::<f32>(),
            Arc::clone(&playback_params),
        );

        sink.append(playback_source);
        sink.pause();

        runtime.media_session.set_metadata(&metadata)?;
        runtime
            .media_session
            .set_playback_state(&PlaybackState::paused(Duration::ZERO))?;

        Ok(Self {
            stream_handle: runtime.stream_handle,
            sink,
            metadata,
            source_format,
            output_format: source_format,
            playback_params,
            playback_clock: RwLock::new(PlaybackClock::at_start(1.0)),
            media_session: runtime.media_session,
            metadata_dirty: AtomicBool::new(false),
            shutdown_handle: runtime.shutdown_handle,
            background_thread: Some(runtime.background_thread),
            command_receiver: runtime.command_receiver,
        })
    }

    #[allow(clippy::future_not_send)]
    async fn open_url_with_sink(
        url: &str,
        sink_init: SinkInit,
        output: &AudioOutput,
    ) -> Result<Self, PlayerError> {
        let response = zenwave::get(url)
            .await
            .map_err(|e| PlayerError::LoadFailed(format!("HTTP request failed: {e}")))?;

        let bytes =
            response.into_body().into_bytes().await.map_err(|e| {
                PlayerError::LoadFailed(format!("Failed to read response body: {e}"))
            })?;

        let runtime = Self::initialize_runtime(output)?;
        let sink = SinkBackend::new(&runtime.stream_handle, sink_init)?;

        let source = Decoder::new(std::io::Cursor::new(bytes))
            .map_err(|e| PlayerError::UnsupportedFormat(e.to_string()))?;
        let source_format = Some(AudioStreamFormat {
            channels: source.channels(),
            sample_rate_hz: source.sample_rate(),
        });

        let mut metadata = MediaMetadata::default();
        if let Some(d) = source.total_duration() {
            metadata = metadata.with_duration(d);
        }
        metadata = metadata.with_title(Self::title_from_url(url));

        let playback_params = Arc::new(PlaybackParams::new());
        let playback_source = AdaptivePlaybackSource::new(
            source.convert_samples::<f32>(),
            Arc::clone(&playback_params),
        );

        sink.append(playback_source);
        sink.pause();

        runtime.media_session.set_metadata(&metadata)?;
        runtime
            .media_session
            .set_playback_state(&PlaybackState::paused(Duration::ZERO))?;

        Ok(Self {
            stream_handle: runtime.stream_handle,
            sink,
            metadata,
            source_format,
            output_format: source_format,
            playback_params,
            playback_clock: RwLock::new(PlaybackClock::at_start(1.0)),
            media_session: runtime.media_session,
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
        Self::open_with_sink(path, SinkInit::Standard, &AudioOutput::system_default())
    }

    /// Open audio from a file path on the selected output.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened or the selected audio output fails.
    pub fn open_with_output(
        path: impl AsRef<Path>,
        output: &AudioOutput,
    ) -> Result<Self, PlayerError> {
        Self::open_with_sink(path, SinkInit::Standard, output)
    }

    /// Open audio from a file path with spatial rendering enabled.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened, audio output fails,
    /// or the spatial scene is invalid.
    pub fn open_spatial(path: impl AsRef<Path>, scene: SpatialScene) -> Result<Self, PlayerError> {
        Self::open_with_sink(
            path,
            SinkInit::Spatial(scene),
            &AudioOutput::system_default(),
        )
    }

    /// Open audio from a file path with spatial rendering on the selected output.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened, the selected audio output fails,
    /// or the spatial scene is invalid.
    pub fn open_spatial_with_output(
        path: impl AsRef<Path>,
        scene: SpatialScene,
        output: &AudioOutput,
    ) -> Result<Self, PlayerError> {
        Self::open_with_sink(path, SinkInit::Spatial(scene), output)
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
        Self::open_url_with_sink(url, SinkInit::Standard, &AudioOutput::system_default()).await
    }

    /// Open audio from a URL on the selected output.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL cannot be fetched, its format is unsupported,
    /// or the selected audio output fails.
    #[allow(clippy::future_not_send)]
    pub async fn open_url_with_output(
        url: &str,
        output: &AudioOutput,
    ) -> Result<Self, PlayerError> {
        Self::open_url_with_sink(url, SinkInit::Standard, output).await
    }

    /// Open audio from a URL (async) with spatial rendering enabled.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL cannot be fetched, the audio format is unsupported,
    /// or the spatial scene is invalid.
    #[allow(clippy::future_not_send)]
    pub async fn open_url_spatial(url: &str, scene: SpatialScene) -> Result<Self, PlayerError> {
        Self::open_url_with_sink(
            url,
            SinkInit::Spatial(scene),
            &AudioOutput::system_default(),
        )
        .await
    }

    /// Open audio from a URL with spatial rendering on the selected output.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL cannot be fetched, its format is unsupported,
    /// the selected output fails, or the spatial scene is invalid.
    #[allow(clippy::future_not_send)]
    pub async fn open_url_spatial_with_output(
        url: &str,
        scene: SpatialScene,
        output: &AudioOutput,
    ) -> Result<Self, PlayerError> {
        Self::open_url_with_sink(url, SinkInit::Spatial(scene), output).await
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
        self.metadata = std::mem::take(&mut self.metadata).with_title(title);
        self.metadata_dirty.store(true, Ordering::Release);
        self
    }

    /// Set the artist.
    #[must_use]
    pub fn artist(mut self, artist: impl Into<String>) -> Self {
        self.metadata = std::mem::take(&mut self.metadata).with_artist(artist);
        self.metadata_dirty.store(true, Ordering::Release);
        self
    }

    /// Set the album.
    #[must_use]
    pub fn album(mut self, album: impl Into<String>) -> Self {
        self.metadata = std::mem::take(&mut self.metadata).with_album(album);
        self.metadata_dirty.store(true, Ordering::Release);
        self
    }

    /// Set encoded artwork.
    #[must_use]
    pub fn artwork(mut self, artwork: MediaArtwork) -> Self {
        self.metadata = std::mem::take(&mut self.metadata).with_artwork(artwork);
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
    ///
    /// # Panics
    ///
    /// Panics if the platform refuses audio focus or system media state cannot be updated.
    pub fn play(&self) {
        self.flush_metadata();
        self.media_session
            .request_audio_focus()
            .unwrap_or_else(|error| {
                panic!("waterkit-audio: failed to acquire audio focus before playback: {error}")
            });
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
    ///
    /// # Panics
    ///
    /// Panics if the platform media session cannot be cleared or audio focus cannot be released.
    pub fn stop(&self) {
        self.flush_metadata();
        self.sink.stop();
        let current_rate = self.playback_params.rate();
        if let Ok(mut clock) = self.playback_clock.write() {
            *clock = PlaybackClock::at_start(current_rate);
        }
        self.media_session.clear().unwrap_or_else(|error| {
            panic!("waterkit-audio: failed to clear media session after stop: {error}")
        });
        self.media_session
            .abandon_audio_focus()
            .unwrap_or_else(|error| {
                panic!("waterkit-audio: failed to abandon audio focus after stop: {error}")
            });
        self.update_now_playing();
    }

    /// Seek to a specific position.
    pub fn seek(&self, position: Duration) {
        self.flush_metadata();
        let target_sink_position = self
            .playback_clock
            .read()
            .map_or(position, |clock| clock.sink_position(position));
        if self.sink.try_seek(target_sink_position).is_ok()
            && let Ok(mut clock) = self.playback_clock.write()
        {
            clock.reanchor_after_seek(target_sink_position, position);
        }
        self.update_now_playing();
    }

    /// Set volume (0.0 to 1.0).
    pub fn set_volume(&self, volume: f32) {
        self.sink.set_volume(volume.clamp(0.0, 1.0));
    }

    /// Set playback rate (1.0 = normal speed).
    pub fn set_playback_rate(&self, rate: f32) {
        let clamped = clamp_playback_rate(rate);
        let sink_position = self.sink.get_pos();
        if let Ok(mut clock) = self.playback_clock.write() {
            clock.reanchor_for_rate_change(sink_position, clamped);
        }
        self.playback_params.set_rate(clamped);
        let sink_speed = sink_speed_for_playback(clamped, self.playback_params.preserve_pitch());
        self.sink.set_speed(sink_speed);
        self.update_now_playing();
    }

    /// Enable/disable pitch preservation during rate changes.
    pub fn set_preserve_pitch(&self, preserve_pitch: bool) {
        self.playback_params.set_preserve_pitch(preserve_pitch);
        let rate = self.playback_params.rate();
        let sink_speed = sink_speed_for_playback(rate, preserve_pitch);
        self.sink.set_speed(sink_speed);
        self.update_now_playing();
    }

    /// Source audio format reported by the decoder.
    #[must_use]
    pub const fn source_format(&self) -> Option<AudioStreamFormat> {
        self.source_format
    }

    /// Current output format.
    #[must_use]
    pub const fn output_format(&self) -> Option<AudioStreamFormat> {
        self.output_format
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
        let sink_position = self.sink.get_pos();
        self.playback_clock
            .read()
            .map_or(sink_position, |clock| clock.source_position(sink_position))
    }

    /// Get total duration.
    #[must_use]
    pub const fn duration(&self) -> Option<Duration> {
        self.metadata.duration()
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
        let position = self.position();
        let base_state = if self.is_playing() {
            PlaybackState::playing(position)
        } else if self.sink.empty() {
            PlaybackState::stopped()
        } else {
            PlaybackState::paused(position)
        };
        let rate = self
            .playback_clock
            .read()
            .map_or(1.0_f64, |clock| f64::from(clock.rate));
        let state = base_state.with_rate(rate);

        self.media_session
            .set_metadata(&self.metadata)
            .unwrap_or_else(|error| {
                panic!("waterkit-audio: failed to update system media metadata: {error}")
            });
        self.media_session
            .set_playback_state(&state)
            .unwrap_or_else(|error| {
                panic!("waterkit-audio: failed to update system playback state: {error}")
            });
    }

    /// List available audio output devices.
    ///
    /// # Errors
    /// Returns an error if the audio host cannot enumerate output devices.
    pub fn list_devices() -> Result<Vec<AudioDevice>, PlayerError> {
        AudioDevice::list()
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

        if let Err(error) = self.media_session.clear() {
            tracing::error!(%error, "failed to clear system media session during audio player teardown");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdaptivePlaybackSource, ListenerPose, PitchStretchEngine, PlaybackClock, PlaybackParams,
        SpatialPosition, SpatialScene, should_use_pitch_stretch, sink_speed_for_playback,
    };
    use rodio::Source;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    const NANOS_PER_SECOND: u128 = 1_000_000_000;

    #[derive(Debug)]
    struct TrackingSource {
        samples: Vec<f32>,
        channels: u16,
        sample_rate: u32,
        cursor: usize,
        last_seek_nanos: Arc<AtomicU64>,
    }

    impl TrackingSource {
        fn new(
            channels: u16,
            sample_rate: u32,
            frames: usize,
            last_seek_nanos: Arc<AtomicU64>,
        ) -> Self {
            assert!(
                channels > 0,
                "tracking source requires at least one channel"
            );
            assert!(
                sample_rate > 0,
                "tracking source requires a non-zero sample rate"
            );

            let total_samples = frames.saturating_mul(usize::from(channels));
            let samples = (0..total_samples)
                .map(|index| {
                    let normalized = f32::from(
                        u16::try_from(index % 257)
                            .expect("sample fixture modulo should fit into u16"),
                    ) / 257.0;
                    normalized.mul_add(2.0, -1.0)
                })
                .collect();

            Self {
                samples,
                channels,
                sample_rate,
                cursor: 0,
                last_seek_nanos,
            }
        }
    }

    impl Iterator for TrackingSource {
        type Item = f32;

        fn next(&mut self) -> Option<Self::Item> {
            let sample = self.samples.get(self.cursor).copied();
            if sample.is_some() {
                self.cursor = self.cursor.saturating_add(1);
            }
            sample
        }
    }

    impl Source for TrackingSource {
        fn current_frame_len(&self) -> Option<usize> {
            None
        }

        fn channels(&self) -> u16 {
            self.channels
        }

        fn sample_rate(&self) -> u32 {
            self.sample_rate
        }

        fn total_duration(&self) -> Option<Duration> {
            let total_frames = self.samples.len() / usize::from(self.channels);
            let total_frames =
                u32::try_from(total_frames).expect("tracking source fixture should fit u32 frames");
            Some(Duration::from_secs_f64(
                f64::from(total_frames) / f64::from(self.sample_rate),
            ))
        }

        fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
            let target_frames = position
                .as_nanos()
                .checked_mul(u128::from(self.sample_rate))
                .expect("tracking source seek position should fit u128 frame-nanos");
            let target_frames = (target_frames + (NANOS_PER_SECOND / 2)) / NANOS_PER_SECOND;
            let target_frames =
                usize::try_from(target_frames).expect("tracking source seek should fit usize");
            let total_frames = self.samples.len() / usize::from(self.channels);
            let clamped_frames = target_frames.min(total_frames);
            self.cursor = clamped_frames.saturating_mul(usize::from(self.channels));
            let recorded = u64::try_from(position.as_nanos().min(u128::from(u64::MAX)))
                .expect("recorded seek position is clamped to u64");
            self.last_seek_nanos.store(recorded, Ordering::Release);
            Ok(())
        }
    }

    fn assert_f32_close(actual: f32, expected: f32) {
        let delta = (actual - expected).abs();
        assert!(
            delta <= f32::EPSILON,
            "f32 mismatch: actual={actual} expected={expected} delta={delta}"
        );
    }

    fn assert_duration_close(actual: Duration, expected: Duration, tolerance_ms: u64) {
        let delta = actual.abs_diff(expected);
        assert!(
            delta <= Duration::from_millis(tolerance_ms),
            "duration mismatch: actual={actual:?} expected={expected:?} delta={delta:?}"
        );
    }

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

    #[test]
    fn playback_clock_preserves_continuity_across_rate_change() {
        let mut clock = PlaybackClock::new();
        let sink_before_change = Duration::from_secs(4);
        let source_before_change = clock.source_position(sink_before_change);
        assert_eq!(source_before_change, Duration::from_secs(4));

        clock.reanchor_for_rate_change(sink_before_change, 2.0);

        let sink_after_change = Duration::from_secs(5);
        let source_after_change = clock.source_position(sink_after_change);
        assert_eq!(source_after_change, Duration::from_secs(6));
    }

    #[test]
    fn playback_clock_seek_mapping_round_trips() {
        let mut clock = PlaybackClock::new();
        clock.reanchor_for_rate_change(Duration::from_millis(2250), 1.5);
        let source_target = Duration::from_secs(12);
        let sink_target = clock.sink_position(source_target);
        let mapped_back = clock.source_position(sink_target);
        assert_duration_close(mapped_back, source_target, 1);
    }

    #[test]
    fn preserve_pitch_policy_selects_expected_sink_speed() {
        assert!(should_use_pitch_stretch(1.25, true));
        assert_f32_close(sink_speed_for_playback(1.25, true), 1.0);

        assert!(!should_use_pitch_stretch(1.25, false));
        assert_f32_close(sink_speed_for_playback(1.25, false), 1.25);

        assert!(!should_use_pitch_stretch(1.0, true));
        assert_f32_close(sink_speed_for_playback(1.0, true), 1.0);
    }

    #[test]
    fn multichannel_pitch_stretch_preserves_channel_alignment() {
        let channels = 6usize;
        let mut engine = PitchStretchEngine::new(channels, 48_000, 0.75);
        let input: Vec<f32> = (0..(4096 * channels))
            .map(|index| {
                let normalized = f32::from(
                    u16::try_from(index % 97).expect("sample fixture modulo should fit into u16"),
                ) / 97.0;
                normalized.mul_add(2.0, -1.0)
            })
            .collect();

        let output = engine.process(&input);
        assert!(output.len().is_multiple_of(channels));

        let flushed = engine.flush();
        assert!(flushed.len().is_multiple_of(channels));
    }

    #[test]
    fn adaptive_source_seek_tracks_preserve_pitch_mode() {
        let seek_probe = Arc::new(AtomicU64::new(0));
        let source = TrackingSource::new(2, 48_000, 48_000, Arc::clone(&seek_probe));
        let params = Arc::new(PlaybackParams::new());
        params.set_rate(1.5);
        params.set_preserve_pitch(true);

        let mut adaptive = AdaptivePlaybackSource::new(source, Arc::clone(&params));
        adaptive
            .try_seek(Duration::from_millis(1500))
            .expect("seek with preserve-pitch enabled should succeed");
        let sought_source_pos = Duration::from_nanos(seek_probe.load(Ordering::Acquire));
        assert_duration_close(sought_source_pos, Duration::from_millis(2250), 2);

        params.set_preserve_pitch(false);
        adaptive
            .try_seek(Duration::from_millis(900))
            .expect("seek with preserve-pitch disabled should succeed");
        let sought_source_pos = Duration::from_nanos(seek_probe.load(Ordering::Acquire));
        assert_duration_close(sought_source_pos, Duration::from_millis(900), 2);
    }

    #[test]
    fn adaptive_source_keeps_multichannel_alignment_across_mode_switches() {
        let channels = 6u16;
        let source = TrackingSource::new(channels, 48_000, 8192, Arc::new(AtomicU64::new(0)));
        let params = Arc::new(PlaybackParams::new());
        params.set_rate(1.25);
        params.set_preserve_pitch(true);

        let adaptive = AdaptivePlaybackSource::new(source, Arc::clone(&params));
        let mut output = Vec::new();
        let mut emitted_samples = 0usize;

        for sample in adaptive {
            if emitted_samples == 3000 {
                params.set_preserve_pitch(false);
            }
            if emitted_samples == 6000 {
                params.set_preserve_pitch(true);
                params.set_rate(0.8);
            }
            if emitted_samples == 9000 {
                params.set_rate(1.0);
            }
            output.push(sample);
            emitted_samples = emitted_samples.saturating_add(1);
        }

        assert!(!output.is_empty(), "adaptive playback must produce output");
        assert!(
            output.len().is_multiple_of(usize::from(channels)),
            "adaptive output lost channel alignment: channels={} samples={}",
            channels,
            output.len()
        );
    }
}
