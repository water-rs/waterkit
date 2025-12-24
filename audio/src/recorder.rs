//! Async audio recording.
//!
//! Uses `cpal` for desktop platforms and native APIs for mobile.

use std::fmt;



/// Audio sample format configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioFormat {
    /// Sample rate in Hz (e.g., 44100, 48000).
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo).
    pub channels: u16,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self {
            sample_rate: 44100,
            channels: 1,
        }
    }
}

/// Information about an audio input device.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InputDevice {
    /// Unique identifier for the device.
    pub id: String,
    /// Human-readable name.
    pub name: String,
}

impl fmt::Display for InputDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// A buffer of recorded audio samples.
#[derive(Clone)]
pub struct AudioBuffer {
    /// Audio samples as f32 (-1.0 to 1.0).
    samples: Vec<f32>,
    /// Format of the audio data.
    format: AudioFormat,
}

impl fmt::Debug for AudioBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioBuffer")
            .field("samples_len", &self.samples.len())
            .field("format", &self.format)
            .finish()
    }
}

impl AudioBuffer {
    /// Create a new audio buffer.
    #[must_use]
    pub const fn new(samples: Vec<f32>, format: AudioFormat) -> Self {
        Self { samples, format }
    }

    /// Get the audio samples.
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Get the audio format.
    #[must_use]
    pub const fn format(&self) -> &AudioFormat {
        &self.format
    }

    /// Get the number of samples.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.samples.len()
    }

    /// Check if the buffer is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Get duration in seconds.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn duration_secs(&self) -> f64 {
        self.samples.len() as f64 / (f64::from(self.format.sample_rate) * f64::from(self.format.channels))
    }
}

/// Errors that can occur during audio recording.
#[derive(Debug, Clone)]
pub enum RecordError {
    /// Recording is not supported on this platform.
    NotSupported,
    /// Failed to enumerate input devices.
    EnumerationFailed(String),
    /// Device not found.
    DeviceNotFound(String),
    /// Failed to open device.
    OpenFailed(String),
    /// Failed to start recording.
    StartFailed(String),
    /// Failed to read audio data.
    ReadFailed(String),
    /// Permission denied.
    PermissionDenied,
    /// Recording is not active.
    NotRecording,
    /// An unknown error occurred.
    Unknown(String),
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSupported => write!(f, "recording not supported on this platform"),
            Self::EnumerationFailed(msg) => write!(f, "failed to enumerate devices: {msg}"),
            Self::DeviceNotFound(id) => write!(f, "device not found: {id}"),
            Self::OpenFailed(msg) => write!(f, "failed to open device: {msg}"),
            Self::StartFailed(msg) => write!(f, "failed to start recording: {msg}"),
            Self::ReadFailed(msg) => write!(f, "failed to read audio: {msg}"),
            Self::PermissionDenied => write!(f, "microphone permission denied"),
            Self::NotRecording => write!(f, "not currently recording"),
            Self::Unknown(msg) => write!(f, "unknown error: {msg}"),
        }
    }
}

impl std::error::Error for RecordError {}

/// Builder for creating an [`AudioRecorder`].
#[derive(Debug, Default)]
pub struct AudioRecorderBuilder {
    device_id: Option<String>,
    sample_rate: Option<u32>,
    channels: Option<u16>,
}

impl AudioRecorderBuilder {
    /// Create a new recorder builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a specific input device (optional, uses default if not set).
    #[must_use]
    pub fn device(mut self, device: &InputDevice) -> Self {
        self.device_id = Some(device.id.clone());
        self
    }

    /// Set the sample rate in Hz.
    #[must_use]
    pub const fn sample_rate(mut self, rate: u32) -> Self {
        self.sample_rate = Some(rate);
        self
    }

    /// Set the number of channels.
    #[must_use]
    pub const fn channels(mut self, channels: u16) -> Self {
        self.channels = Some(channels);
        self
    }

    /// Build the audio recorder.
    ///
    /// # Errors
    ///
    /// Returns an error if the device cannot be opened.
    pub fn build(self) -> Result<AudioRecorder, RecordError> {
        let format = AudioFormat {
            sample_rate: self.sample_rate.unwrap_or(44100),
            channels: self.channels.unwrap_or(1),
        };
        AudioRecorder::new_internal(self.device_id, format)
    }
}

/// Async audio recorder for capturing microphone input.
///
/// # Example
///
/// ```no_run
/// use waterkit_audio::{AudioRecorder, AudioBuffer};
///
/// async fn record() -> Result<(), waterkit_audio::RecordError> {
///     let mut recorder = AudioRecorder::new()
///         .sample_rate(44100)
///         .channels(1)
///         .build()?;
///
///     recorder.start().await?;
///
///     // Read audio buffers
///     let buffer = recorder.read().await?;
///     println!("Captured {} samples", buffer.len());
///
///     recorder.stop().await?;
///     Ok(())
/// }
/// ```
pub struct AudioRecorder {
    inner: crate::sys::AudioRecorderInner,
    format: AudioFormat,
}

impl fmt::Debug for AudioRecorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioRecorder")
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl AudioRecorder {
    /// Create a new audio recorder builder.
    #[must_use]
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> AudioRecorderBuilder {
        AudioRecorderBuilder::new()
    }

    /// List available input devices.
    ///
    /// # Errors
    ///
    /// Returns an error if device enumeration fails.
    pub fn list_devices() -> Result<Vec<InputDevice>, RecordError> {
        crate::sys::AudioRecorderInner::list_devices()
    }

    fn new_internal(device_id: Option<String>, format: AudioFormat) -> Result<Self, RecordError> {
        Ok(Self {
            inner: crate::sys::AudioRecorderInner::new(device_id, format)?,
            format,
        })
    }

    /// # Errors
    ///
    /// Returns an error if recording cannot be started.
    #[allow(clippy::future_not_send)]
    pub async fn start(&mut self) -> Result<(), RecordError> {
        self.inner.start().await
    }

    /// # Errors
    ///
    /// Returns an error if recording cannot be stopped.
    #[allow(clippy::future_not_send)]
    pub async fn stop(&mut self) -> Result<(), RecordError> {
        self.inner.stop().await
    }

    /// # Errors
    ///
    /// Returns an error if reading fails or recording is not active.
    #[allow(clippy::future_not_send)]
    pub async fn read(&mut self) -> Result<AudioBuffer, RecordError> {
        self.inner.read().await
    }

    /// Try to read audio data without waiting.
    ///
    /// Returns `None` if no data is available.
    pub fn try_read(&mut self) -> Option<AudioBuffer> {
        self.inner.try_read()
    }

    /// Check if currently recording.
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.inner.is_recording()
    }

    /// Get the audio format.
    #[must_use]
    pub const fn format(&self) -> &AudioFormat {
        &self.format
    }
}


