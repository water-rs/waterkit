//! Desktop audio recording using cpal.
//!
//! Works on macOS, Windows, and Linux.

use crate::recorder::{AudioBuffer, AudioFormat, AudioFormatRequest, InputDevice, RecordError};
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
    pub fn new(device_id: Option<String>, format: AudioFormatRequest) -> Result<Self, RecordError> {
        let host = cpal::default_host();

        let device = if let Some(id) = device_id {
            let devices = host
                .input_devices()
                .map_err(|e| RecordError::EnumerationFailed(e.to_string()))?;

            devices
                .into_iter()
                .find(|d| d.name().is_ok_and(|n| n == id))
                .ok_or(RecordError::DeviceNotFound(id))?
        } else {
            host.default_input_device()
                .ok_or_else(|| RecordError::DeviceNotFound("no default device".into()))?
        };

        let format = resolve_format(&device, format)?;

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
            return Err(RecordError::StartFailed(
                "Recoder is in invalid state".into(),
            ));
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
                    tracing::error!("Audio input error: {err}");
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

    /// Check if recording.
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    #[allow(dead_code)]
    pub fn split(self) -> (Self, async_channel::Receiver<AudioBuffer>) {
        let receiver = self.receiver.clone();
        (self, receiver)
    }

    pub fn receiver(&self) -> async_channel::Receiver<AudioBuffer> {
        self.receiver.clone()
    }

    pub const fn format(&self) -> AudioFormat {
        self.format
    }
}

fn resolve_format(
    device: &cpal::Device,
    request: AudioFormatRequest,
) -> Result<AudioFormat, RecordError> {
    let default = device
        .default_input_config()
        .map_err(|e| RecordError::OpenFailed(e.to_string()))?;

    let sample_rate = request
        .sample_rate
        .unwrap_or_else(|| default.sample_rate().0);
    let channels = request.channels.unwrap_or_else(|| default.channels());

    let supported = device
        .supported_input_configs()
        .map_err(|e| RecordError::EnumerationFailed(e.to_string()))?
        .any(|range| {
            range.channels() == channels
                && range.min_sample_rate().0 <= sample_rate
                && sample_rate <= range.max_sample_rate().0
        });

    if !supported {
        return Err(RecordError::OpenFailed(format!(
            "input device does not support {sample_rate} Hz with {channels} channel(s)"
        )));
    }

    Ok(AudioFormat {
        sample_rate,
        channels,
    })
}
