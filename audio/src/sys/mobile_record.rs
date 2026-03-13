//! Mobile audio recording placeholder.

use crate::recorder::{AudioBuffer, AudioFormat, InputDevice, RecordError};

/// Mobile audio recorder inner.
pub struct AudioRecorderInner;

impl AudioRecorderInner {
    /// List available input devices.
    pub fn list_devices() -> Result<Vec<InputDevice>, RecordError> {
        Err(RecordError::Unsupported)
    }

    /// Create a new audio recorder.
    pub fn new(_device_id: Option<String>, _format: AudioFormat) -> Result<Self, RecordError> {
        Err(RecordError::Unsupported)
    }

    /// Start recording.
    #[allow(clippy::future_not_send, clippy::unused_async)]
    pub async fn start(&mut self) -> Result<(), RecordError> {
        Err(RecordError::Unsupported)
    }

    /// Stop recording.
    #[allow(clippy::future_not_send, clippy::unused_async)]
    pub async fn stop(&mut self) -> Result<(), RecordError> {
        Err(RecordError::Unsupported)
    }

    /// Read audio buffer (async).
    #[allow(clippy::future_not_send)]
    pub async fn read(&self) -> Result<AudioBuffer, RecordError> {
        Err(RecordError::Unsupported)
    }

    /// Try to read without waiting.
    pub fn try_read(&self) -> Option<AudioBuffer> {
        None
    }

    /// Read audio buffer synchronously (blocking).
    pub fn read_blocking(&self) -> Result<AudioBuffer, RecordError> {
        Err(RecordError::Unsupported)
    }

    /// Check if recording.
    pub fn is_recording(&self) -> bool {
        false
    }

    pub fn receiver(&self) -> async_channel::Receiver<AudioBuffer> {
        let (_, receiver) = async_channel::unbounded();
        receiver
    }
}
