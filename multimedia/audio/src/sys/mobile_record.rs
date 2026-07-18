//! Mobile audio recording placeholder.

use crate::recorder::{AudioBuffer, AudioFormat, AudioFormatRequest, InputDevice, RecordError};

/// Mobile audio recorder inner.
pub struct AudioRecorderInner;

impl AudioRecorderInner {
    /// List available input devices.
    pub const fn list_devices() -> Result<Vec<InputDevice>, RecordError> {
        Err(RecordError::Unsupported)
    }

    /// Create a new audio recorder.
    pub fn new(
        _device_id: Option<String>,
        _format: AudioFormatRequest,
    ) -> Result<Self, RecordError> {
        Err(RecordError::Unsupported)
    }

    /// Start recording.
    #[allow(
        clippy::future_not_send,
        clippy::unused_async,
        clippy::unused_self,
        reason = "the cross-platform recorder API is async and instance-based"
    )]
    pub async fn start(&self) -> Result<(), RecordError> {
        Err(RecordError::Unsupported)
    }

    /// Stop recording.
    #[allow(
        clippy::future_not_send,
        clippy::unused_async,
        clippy::unused_self,
        reason = "the cross-platform recorder API is async and instance-based"
    )]
    pub async fn stop(&self) -> Result<(), RecordError> {
        Err(RecordError::Unsupported)
    }

    /// Check if recording.
    #[allow(
        clippy::unused_self,
        reason = "the cross-platform recorder API is instance-based"
    )]
    pub const fn is_recording(&self) -> bool {
        false
    }

    #[allow(
        clippy::unused_self,
        reason = "the cross-platform recorder API is instance-based"
    )]
    pub fn receiver(&self) -> async_channel::Receiver<AudioBuffer> {
        let (_, receiver) = async_channel::unbounded();
        receiver
    }

    #[allow(
        clippy::unused_self,
        reason = "the cross-platform recorder exposes negotiated format through an instance"
    )]
    pub const fn format(&self) -> AudioFormat {
        AudioFormat {
            sample_rate: 44100,
            channels: 1,
        }
    }
}
