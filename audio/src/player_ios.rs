//! iOS native audio player backend.

use crate::shutdown::ShutdownHandle;
use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
use futures::Stream;
use lofty::prelude::*;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Audio stream format information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioStreamFormat {
    /// Number of interleaved channels.
    pub channels: u16,
    /// Sample rate in Hz.
    pub sample_rate_hz: u32,
}

/// Audio output device.
#[derive(Debug, Clone)]
pub struct AudioDevice {
    name: String,
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

fn clamp_playback_rate(rate: f32) -> f32 {
    if rate.is_finite() {
        rate.clamp(0.25, 4.0)
    } else {
        1.0
    }
}

struct RuntimeHandles {
    player: crate::sys::NativeAudioPlayerInner,
    media_center: Arc<crate::sys::MediaCenterIntegration>,
    shutdown_handle: ShutdownHandle,
    background_thread: JoinHandle<()>,
    command_receiver: async_channel::Receiver<MediaCommand>,
}

/// iOS native audio player with media center integration.
pub struct AudioPlayer {
    player: crate::sys::NativeAudioPlayerInner,
    metadata: MediaMetadata,
    source_format: Option<AudioStreamFormat>,
    output_format: Option<AudioStreamFormat>,
    media_center: Arc<crate::sys::MediaCenterIntegration>,
    metadata_dirty: AtomicBool,
    playback_rate_bits: AtomicU32,
    preserve_pitch: AtomicBool,
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
    fn initialize_runtime() -> Result<RuntimeHandles, PlayerError> {
        let player = crate::sys::NativeAudioPlayerInner::new()?;
        let media_center = Arc::new(
            crate::sys::MediaCenterIntegration::new()
                .map_err(|e| PlayerError::Unknown(format!("media center init failed: {e}")))?,
        );
        let (shutdown_handle, shutdown_rx) = ShutdownHandle::new();
        let (cmd_tx, cmd_rx) = async_channel::unbounded();
        let background_thread = {
            let mc = Arc::clone(&media_center);
            std::thread::spawn(move || {
                while !shutdown_rx.is_shutdown() {
                    mc.run_loop(Duration::from_millis(50));
                    if let Some(cmd) = mc.poll_command() {
                        let _ = cmd_tx.send_blocking(cmd);
                    }
                }
            })
        };

        Ok(RuntimeHandles {
            player,
            media_center,
            shutdown_handle,
            background_thread,
            command_receiver: cmd_rx,
        })
    }

    fn metadata_from_path(path: &Path) -> MediaMetadata {
        let mut metadata = MediaMetadata::default();

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

        metadata
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

    fn playback_rate(&self) -> f32 {
        clamp_playback_rate(f32::from_bits(self.playback_rate_bits.load(Ordering::Acquire)))
    }

    fn set_playback_rate_bits(&self, rate: f32) {
        self.playback_rate_bits
            .store(clamp_playback_rate(rate).to_bits(), Ordering::Release);
    }

    fn preserve_pitch(&self) -> bool {
        self.preserve_pitch.load(Ordering::Acquire)
    }

    fn native_state(&self) -> crate::sys::NativeAudioPlayerState {
        self.player.state()
    }

    fn playback_state(&self) -> PlaybackState {
        let state = self.native_state();
        match state.status {
            PlaybackStatus::Stopped => PlaybackState::stopped(),
            PlaybackStatus::Paused => PlaybackState::paused(state.position.unwrap_or(Duration::ZERO)),
            PlaybackStatus::Playing => PlaybackState::playing(state.position.unwrap_or(Duration::ZERO))
                .with_rate(f64::from(self.playback_rate())),
        }
    }

    fn flush_metadata(&self) {
        if self.metadata_dirty.swap(false, Ordering::AcqRel) {
            self.update_now_playing();
        }
    }

    fn update_now_playing(&self) {
        self.media_center.update(&self.metadata, &self.playback_state());
    }

    fn apply_playback_preferences(&self) -> Result<(), PlayerError> {
        self.player.set_playback_rate(self.playback_rate())?;
        self.player.set_preserve_pitch(self.preserve_pitch())
    }

    /// Open audio from a file path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or the native player fails to load it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PlayerError> {
        let path = path.as_ref();
        let runtime = Self::initialize_runtime()?;
        let path_str = path
            .to_str()
            .expect("waterkit-audio iOS file paths must be valid UTF-8");
        runtime.player.load_file(path_str)?;

        let mut metadata = Self::metadata_from_path(path);
        let state = runtime.player.state();
        if let Some(duration) = state.duration {
            metadata = metadata.with_duration(duration);
        }

        let player = Self {
            player: runtime.player,
            metadata,
            source_format: None,
            output_format: None,
            media_center: runtime.media_center,
            metadata_dirty: AtomicBool::new(false),
            playback_rate_bits: AtomicU32::new(1.0f32.to_bits()),
            preserve_pitch: AtomicBool::new(true),
            shutdown_handle: runtime.shutdown_handle,
            background_thread: Some(runtime.background_thread),
            command_receiver: runtime.command_receiver,
        };
        player.apply_playback_preferences()?;
        player.update_now_playing();
        Ok(player)
    }

    /// Open audio from a file path with spatial rendering enabled.
    ///
    /// # Errors
    ///
    /// Always returns [`PlayerError::SpatialNotEnabled`] on iOS.
    pub fn open_spatial(
        _path: impl AsRef<Path>,
        _scene: SpatialScene,
    ) -> Result<Self, PlayerError> {
        Err(PlayerError::SpatialNotEnabled)
    }

    /// Open audio from a URL (async).
    ///
    /// # Errors
    ///
    /// Returns an error if the URL cannot be loaded by the native player.
    #[allow(clippy::future_not_send)]
    pub async fn open_url(url: &str) -> Result<Self, PlayerError> {
        let runtime = Self::initialize_runtime()?;
        runtime.player.load_url(url)?;

        let mut metadata = MediaMetadata::default().with_title(Self::title_from_url(url));
        let state = runtime.player.state();
        if let Some(duration) = state.duration {
            metadata = metadata.with_duration(duration);
        }

        let player = Self {
            player: runtime.player,
            metadata,
            source_format: None,
            output_format: None,
            media_center: runtime.media_center,
            metadata_dirty: AtomicBool::new(false),
            playback_rate_bits: AtomicU32::new(1.0f32.to_bits()),
            preserve_pitch: AtomicBool::new(true),
            shutdown_handle: runtime.shutdown_handle,
            background_thread: Some(runtime.background_thread),
            command_receiver: runtime.command_receiver,
        };
        player.apply_playback_preferences()?;
        player.update_now_playing();
        Ok(player)
    }

    /// Open audio from a URL (async) with spatial rendering enabled.
    ///
    /// # Errors
    ///
    /// Always returns [`PlayerError::SpatialNotEnabled`] on iOS.
    #[allow(clippy::future_not_send)]
    pub async fn open_url_spatial(_url: &str, _scene: SpatialScene) -> Result<Self, PlayerError> {
        Err(PlayerError::SpatialNotEnabled)
    }

    /// Get the active playback mode.
    #[must_use]
    pub const fn mode(&self) -> PlaybackMode {
        PlaybackMode::PreserveSourceChannels
    }

    /// Get the current spatial scene.
    ///
    /// # Errors
    ///
    /// Always returns [`PlayerError::SpatialNotEnabled`] on iOS.
    pub fn spatial_scene(&self) -> Result<SpatialScene, PlayerError> {
        Err(PlayerError::SpatialNotEnabled)
    }

    /// Set the full spatial scene.
    ///
    /// # Errors
    ///
    /// Always returns [`PlayerError::SpatialNotEnabled`] on iOS.
    pub fn set_spatial_scene(&self, _scene: SpatialScene) -> Result<(), PlayerError> {
        Err(PlayerError::SpatialNotEnabled)
    }

    /// Set emitter position in spatial mode.
    ///
    /// # Errors
    ///
    /// Always returns [`PlayerError::SpatialNotEnabled`] on iOS.
    pub fn set_emitter_position(&self, _position: SpatialPosition) -> Result<(), PlayerError> {
        Err(PlayerError::SpatialNotEnabled)
    }

    /// Set listener ear positions in spatial mode.
    ///
    /// # Errors
    ///
    /// Always returns [`PlayerError::SpatialNotEnabled`] on iOS.
    pub fn set_listener_pose(&self, _pose: ListenerPose) -> Result<(), PlayerError> {
        Err(PlayerError::SpatialNotEnabled)
    }

    /// Set stereo pan in spatial mode.
    ///
    /// # Errors
    ///
    /// Always returns [`PlayerError::SpatialNotEnabled`] on iOS.
    pub fn set_pan(&self, _pan: f32) -> Result<(), PlayerError> {
        Err(PlayerError::SpatialNotEnabled)
    }

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

    /// Set the artwork URL.
    #[must_use]
    pub fn artwork_url(mut self, url: impl Into<String>) -> Self {
        self.metadata = std::mem::take(&mut self.metadata).with_artwork_url(url);
        self.metadata_dirty.store(true, Ordering::Release);
        self
    }

    /// Start playback.
    pub fn play(&self) {
        self.flush_metadata();
        if self.player.play().is_ok() {
            self.update_now_playing();
        }
    }

    /// Pause playback.
    pub fn pause(&self) {
        self.flush_metadata();
        if self.player.pause().is_ok() {
            self.update_now_playing();
        }
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
        if self.player.stop().is_ok() {
            self.media_center.clear();
            self.update_now_playing();
        }
    }

    /// Seek to a specific position.
    pub fn seek(&self, position: Duration) {
        self.flush_metadata();
        if self.player.seek(position).is_ok() {
            self.update_now_playing();
        }
    }

    /// Set volume (0.0 to 1.0).
    pub fn set_volume(&self, volume: f32) {
        let _ = self.player.set_volume(volume.clamp(0.0, 1.0));
    }

    /// Set playback rate (1.0 = normal speed).
    pub fn set_playback_rate(&self, rate: f32) {
        let clamped = clamp_playback_rate(rate);
        self.set_playback_rate_bits(clamped);
        let _ = self.player.set_playback_rate(clamped);
        self.update_now_playing();
    }

    /// Enable/disable pitch preservation during rate changes.
    pub fn set_preserve_pitch(&self, preserve_pitch: bool) {
        self.preserve_pitch.store(preserve_pitch, Ordering::Release);
        let _ = self.player.set_preserve_pitch(preserve_pitch);
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

    /// Check if audio is currently playing.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.native_state().status == PlaybackStatus::Playing
    }

    /// Check if audio is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.native_state().status == PlaybackStatus::Paused
    }

    /// Check if the playlist is empty (playback finished).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.native_state().status == PlaybackStatus::Stopped
    }

    /// Get current playback position.
    pub fn position(&self) -> Duration {
        self.native_state().position.unwrap_or(Duration::ZERO)
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

    /// Get a stream of media commands (Play, Pause, Next, etc.).
    pub fn commands(&self) -> impl Stream<Item = MediaCommand> + '_ {
        self.command_receiver.clone()
    }

    /// Handle a standard media command.
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
            _ => {}
        }
    }

    /// List available audio output devices.
    ///
    /// # Errors
    ///
    /// Always returns an error on iOS because explicit output device selection is unavailable.
    pub fn list_devices() -> Result<Vec<AudioDevice>, PlayerError> {
        Err(PlayerError::Unknown(
            "audio device enumeration is unavailable on iOS".into(),
        ))
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        let _ = self.player.stop();
        drop(std::mem::take(&mut self.shutdown_handle));
        if let Some(handle) = self.background_thread.take() {
            let _ = handle.join();
        }
        self.media_center.clear();
    }
}
