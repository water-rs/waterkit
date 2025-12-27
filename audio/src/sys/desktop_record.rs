//! Desktop audio recording using cpal.
//!
//! Works on macOS, Windows, and Linux.

use crate::recorder::{AudioBuffer, AudioFormat, InputDevice, RecordError};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Desktop audio recorder using cpal.
pub struct AudioRecorderInner {
    device: cpal::Device,
    format: AudioFormat,
    stream: Option<cpal::Stream>,
    // Channel for streaming audio data
    sender: Option<async_channel::Sender<AudioBuffer>>,
    receiver: async_channel::Receiver<AudioBuffer>,
    recording: Arc<AtomicBool>,
}

impl AudioRecorderInner {
    /// List available input devices.
    #[allow(deprecated)]
    pub fn list_devices() -> Result<Vec<InputDevice>, RecordError> {
        let host = cpal::default_host();
        let devices = host
            .input_devices()
            .map_err(|e| RecordError::EnumerationFailed(e.to_string()))?;

        let mut result = Vec::new();
        for device in devices {
            if let Ok(name) = device.name() {
                result.push(InputDevice {
                    id: name.clone(),
                    name,
                });
            }
        }
        Ok(result)
    }

    /// Create a new audio recorder.
    #[allow(deprecated)]
    pub fn new(device_id: Option<String>, format: AudioFormat) -> Result<Self, RecordError> {
        let host = cpal::default_host();

        let device = if let Some(id) = device_id {
            let devices = host
                .input_devices()
                .map_err(|e| RecordError::EnumerationFailed(e.to_string()))?;

            devices
                .into_iter()
                .find(|d| d.name().map(|n| n == id).unwrap_or(false))
                .ok_or(RecordError::DeviceNotFound(id))?
        } else {
            host.default_input_device()
                .ok_or_else(|| RecordError::DeviceNotFound("no default device".into()))?
        };

        // Create unbound channel for audio data
        let (sender, receiver) = async_channel::unbounded();

        Ok(Self {
            device,
            format,
            stream: None,
            sender: Some(sender),
            receiver,
            recording: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Start recording.
    #[allow(clippy::future_not_send, clippy::unused_async)]
    pub async fn start(&mut self) -> Result<(), RecordError> {
        if self.stream.is_some() {
            return Ok(()); // Already recording
        }

        let config = cpal::StreamConfig {
            channels: self.format.channels,
            sample_rate: cpal::SampleRate(self.format.sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let recording = Arc::clone(&self.recording);
        
        // We need a sender for the callback
        let sender = if let Some(s) = &self.sender {
            s.clone()
        } else {
            return Err(RecordError::StartFailed("Recoder is in invalid state".into()));
        };
        
        let format = self.format;

        let stream = self
            .device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if recording.load(Ordering::Relaxed) {
                        let samples = data.to_vec();
                        let buffer = AudioBuffer::new(samples, format);
                        // Ignore errors if receiver is dropped
                        let _ = sender.try_send(buffer);
                    }
                },
                |err| {
                    eprintln!("Audio input error: {err}");
                },
                None,
            )
            .map_err(|e| RecordError::StartFailed(e.to_string()))?;

        stream
            .play()
            .map_err(|e| RecordError::StartFailed(e.to_string()))?;

        self.recording.store(true, Ordering::Relaxed);
        self.stream = Some(stream);

        Ok(())
    }

    /// Stop recording.
    #[allow(clippy::future_not_send, clippy::unused_async)]
    pub async fn stop(&mut self) -> Result<(), RecordError> {
        self.recording.store(false, Ordering::Relaxed);

        if let Some(stream) = self.stream.take() {
            drop(stream);
        }

        Ok(())
    }

    /// Read audio buffer (async).
    #[allow(clippy::future_not_send)]
    pub async fn read(&self) -> Result<AudioBuffer, RecordError> {
        if !self.recording.load(Ordering::Relaxed) {
            return Err(RecordError::NotRecording);
        }

        self.receiver.recv().await.map_err(|e| RecordError::ReadFailed(e.to_string()))
    }

    /// Try to read without waiting.
    pub fn try_read(&self) -> Option<AudioBuffer> {
        self.receiver.try_recv().ok()
    }

    /// Read audio buffer synchronously (blocking).
    /// 
    /// Use this method when calling from a non-async context (e.g., a dedicated thread).
    /// This is more reliable than using `pollster::block_on(read())` as it doesn't
    /// depend on async runtime waker semantics.
    pub fn read_blocking(&self) -> Result<AudioBuffer, RecordError> {
        if !self.recording.load(Ordering::Relaxed) {
            return Err(RecordError::NotRecording);
        }

        self.receiver.recv_blocking().map_err(|e| RecordError::ReadFailed(e.to_string()))
    }

    /// Check if recording.
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    pub fn split(self) -> (crate::sys::AudioRecorderInner, async_channel::Receiver<AudioBuffer>) {
        let receiver = self.receiver.clone();
        (self, receiver)
    }

    pub fn receiver(&self) -> async_channel::Receiver<AudioBuffer> {
        self.receiver.clone()
    }
}
