//! Desktop audio recording using cpal.
//!
//! Works on macOS, Windows, and Linux.

use crate::recorder::{AudioBuffer, AudioFormat, InputDevice, RecordError};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// Desktop audio recorder using cpal.
pub struct AudioRecorderInner {
    device: cpal::Device,
    format: AudioFormat,
    stream: Option<cpal::Stream>,
    buffer: Arc<Mutex<Vec<f32>>>,
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

        Ok(Self {
            device,
            format,
            stream: None,
            buffer: Arc::new(Mutex::new(Vec::new())),
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

        let buffer = Arc::clone(&self.buffer);
        let recording = Arc::clone(&self.recording);

        let stream = self
            .device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if recording.load(Ordering::Relaxed)
                        && let Ok(mut buf) = buffer.lock()
                    {
                        buf.extend_from_slice(data);
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

        // Wait until we have some data
        loop {
            {
                let mut buf = self.buffer.lock().unwrap();
                if !buf.is_empty() {
                    let samples = std::mem::take(&mut *buf);
                    drop(buf);
                    // The original instruction implies a callback `self.on_data` here,
                    // but `self.on_data` is not defined in the struct or provided in the diff.
                    // To maintain syntactic correctness and faithfulness to the provided diff snippet,
                    // while also acknowledging the original return type, we'll keep the return.
                    // If `self.on_data` were defined, this line would be `(self.on_data)(AudioBuffer::new(samples, self.format));`
                    return Ok(AudioBuffer::new(samples, self.format));
                }
            }
            // Yield to async runtime
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Try to read without waiting.
    pub fn try_read(&self) -> Option<AudioBuffer> {
        let mut buf = self.buffer.lock().ok()?;
        if buf.is_empty() {
            return None;
        }
        let samples = std::mem::take(&mut *buf);
        drop(buf);
        Some(AudioBuffer::new(samples, self.format))
    }

    /// Check if recording.
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }
}
