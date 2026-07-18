//! Shared audio-output types used by file and streaming playback.

use crate::MediaError;

/// Audio stream format information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioStreamFormat {
    /// Number of interleaved channels.
    pub channels: u16,
    /// Sample rate in Hz.
    pub sample_rate_hz: u32,
}

/// Audio output device returned by [`AudioDevice::list`].
///
/// The value owns the platform device handle; selection never relies on a
/// display name, because multiple outputs may legitimately have the same name.
#[derive(Clone)]
pub struct AudioDevice {
    name: String,
    #[cfg(not(target_os = "ios"))]
    pub(crate) handle: rodio::cpal::Device,
}

impl AudioDevice {
    /// Enumerates the currently available audio output devices.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform cannot enumerate output devices or
    /// when an enumerated device does not expose a valid name. Explicit output
    /// selection is unavailable on iOS; route selection there belongs to the
    /// system audio-session route picker.
    #[cfg(target_os = "ios")]
    pub const fn list() -> Result<Vec<Self>, PlayerError> {
        Err(PlayerError::OutputDeviceSelectionUnavailable)
    }

    /// Enumerates the currently available audio output devices.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform cannot enumerate output devices or
    /// when an enumerated device does not expose a valid name.
    #[cfg(not(target_os = "ios"))]
    pub fn list() -> Result<Vec<Self>, PlayerError> {
        use rodio::cpal::traits::{DeviceTrait as _, HostTrait as _};

        rodio::cpal::default_host()
            .output_devices()
            .map_err(|error| PlayerError::OutputDeviceEnumerationFailed(error.to_string()))?
            .map(|handle| {
                let name = handle.name().map_err(|error| {
                    PlayerError::OutputDeviceEnumerationFailed(error.to_string())
                })?;
                Ok(Self { name, handle })
            })
            .collect()
    }

    /// Get the device name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl std::fmt::Debug for AudioDevice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AudioDevice")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for AudioDevice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.name)
    }
}

/// Audio output selection for a player instance.
///
/// The default value follows the platform's current system output. Use
/// [`AudioOutput::on_device`] with a device returned by [`AudioDevice::list`]
/// to pin a player to one explicit output.
#[derive(Debug, Clone, Default)]
pub struct AudioOutput {
    device: Option<AudioDevice>,
}

impl AudioOutput {
    /// Follow the platform's current default output route.
    #[must_use]
    pub const fn system_default() -> Self {
        Self { device: None }
    }

    /// Select one enumerated output device.
    #[must_use]
    pub fn on_device(device: &AudioDevice) -> Self {
        Self {
            device: Some(device.clone()),
        }
    }

    /// Returns the explicitly selected device, or `None` when following the
    /// system default route.
    #[must_use]
    pub const fn selected_device(&self) -> Option<&AudioDevice> {
        self.device.as_ref()
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
    /// Audio output devices could not be enumerated.
    #[error("failed to enumerate audio output devices: {0}")]
    OutputDeviceEnumerationFailed(String),
    /// The platform owns output-route selection for this player type.
    #[error("explicit audio output device selection is unavailable on this platform")]
    OutputDeviceSelectionUnavailable,
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
    fn from(error: MediaError) -> Self {
        Self::Unknown(error.to_string())
    }
}
