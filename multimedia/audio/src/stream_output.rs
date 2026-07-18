//! Incremental PCM playback with a presentation-timeline audio clock.

use std::collections::VecDeque;
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};

use crate::playback_rate::{
    PitchStretchEngine, PlaybackParams, clamp_playback_rate, should_use_pitch_stretch,
    sink_speed_for_playback,
};
use crate::{AudioOutput, DecodedAudioFrame, PlayerError};

struct PresentationClockSource {
    samples: SamplesBuffer<f32>,
    presentation_start_nanos: u64,
    presentation_end_nanos: u64,
    sample_index: u64,
    total_frames: u64,
    channels: u16,
    sample_rate: u32,
    generation: u64,
    active_generation: Arc<AtomicU64>,
    position_nanos: Arc<AtomicU64>,
    buffer_progress: async_channel::Sender<()>,
    completion_reported: bool,
}

impl PresentationClockSource {
    #[cfg(test)]
    fn from_decoded(
        frame: DecodedAudioFrame,
        generation: u64,
        active_generation: Arc<AtomicU64>,
        position_nanos: Arc<AtomicU64>,
        buffer_progress: async_channel::Sender<()>,
    ) -> Result<Self, PlayerError> {
        let presentation_start = frame.presentation_time();
        let presentation_end = presentation_start
            .checked_add(frame.duration())
            .ok_or_else(|| timeline_overflow_error("decoded audio presentation range"))?;
        let channels = frame.channels();
        let sample_rate = frame.sample_rate();
        Self::from_samples(
            presentation_start,
            presentation_end,
            channels,
            sample_rate,
            frame.into_samples().into_vec(),
            generation,
            active_generation,
            position_nanos,
            buffer_progress,
        )
    }

    fn from_processed(
        frame: ProcessedAudioFrame,
        generation: u64,
        active_generation: Arc<AtomicU64>,
        position_nanos: Arc<AtomicU64>,
        buffer_progress: async_channel::Sender<()>,
    ) -> Result<Self, PlayerError> {
        Self::from_samples(
            frame.presentation_start,
            frame.presentation_end,
            frame.channels,
            frame.sample_rate,
            frame.samples,
            generation,
            active_generation,
            position_nanos,
            buffer_progress,
        )
    }

    fn clock_jump(
        presentation_start: Duration,
        presentation_end: Duration,
        generation: u64,
        active_generation: Arc<AtomicU64>,
        position_nanos: Arc<AtomicU64>,
        buffer_progress: async_channel::Sender<()>,
    ) -> Result<Self, PlayerError> {
        assert!(
            presentation_end > presentation_start,
            "audio clock jump must advance presentation time"
        );
        Ok(Self {
            samples: SamplesBuffer::new(1, 1, Vec::new()),
            presentation_start_nanos: duration_to_nanos(presentation_start)?,
            presentation_end_nanos: duration_to_nanos(presentation_end)?,
            sample_index: 0,
            total_frames: 0,
            channels: 1,
            sample_rate: 1,
            generation,
            active_generation,
            position_nanos,
            buffer_progress,
            completion_reported: false,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn from_samples(
        presentation_start: Duration,
        presentation_end: Duration,
        channels: NonZeroU16,
        sample_rate: NonZeroU32,
        samples: Vec<f32>,
        generation: u64,
        active_generation: Arc<AtomicU64>,
        position_nanos: Arc<AtomicU64>,
        buffer_progress: async_channel::Sender<()>,
    ) -> Result<Self, PlayerError> {
        let channels = channels.get();
        assert!(
            !samples.is_empty(),
            "presentation clock source requires at least one PCM frame"
        );
        assert!(
            samples.len().is_multiple_of(usize::from(channels)),
            "presentation clock source received channel-misaligned PCM"
        );
        let total_frames = u64::try_from(samples.len() / usize::from(channels))
            .expect("PCM frame count must fit u64");
        let sample_rate = sample_rate.get();
        let samples = SamplesBuffer::new(channels, sample_rate, samples);
        Ok(Self {
            samples,
            presentation_start_nanos: duration_to_nanos(presentation_start)?,
            presentation_end_nanos: duration_to_nanos(presentation_end)?,
            sample_index: 0,
            total_frames,
            channels,
            sample_rate,
            generation,
            active_generation,
            position_nanos,
            buffer_progress,
            completion_reported: false,
        })
    }

    fn report_completion(&mut self) {
        if self.completion_reported {
            return;
        }
        self.completion_reported = true;
        let _ = self.buffer_progress.try_send(());
    }

    fn update_clock(&self) {
        let consumed_frames = self.sample_index / u64::from(self.channels);
        let presentation_span = self
            .presentation_end_nanos
            .saturating_sub(self.presentation_start_nanos);
        let elapsed_nanos = u64::try_from(
            u128::from(presentation_span) * u128::from(consumed_frames)
                / u128::from(self.total_frames),
        )
        .expect("interpolated presentation time must fit u64 nanoseconds");
        let position = self
            .presentation_start_nanos
            .checked_add(elapsed_nanos)
            .expect("decoded audio presentation timestamp must fit u64 nanoseconds")
            .min(self.presentation_end_nanos);
        self.position_nanos.store(position, Ordering::Release);
    }
}

impl Iterator for PresentationClockSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.active_generation.load(Ordering::Acquire) != self.generation {
            self.report_completion();
            return None;
        }
        if self.total_frames == 0 {
            self.position_nanos
                .store(self.presentation_end_nanos, Ordering::Release);
            self.report_completion();
            return None;
        }
        let sample = self.samples.next();
        if sample.is_some() {
            self.update_clock();
            self.sample_index = self
                .sample_index
                .checked_add(1)
                .expect("decoded audio sample index must fit u64");
        } else {
            self.position_nanos
                .store(self.presentation_end_nanos, Ordering::Release);
            self.report_completion();
        }
        sample
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.samples.size_hint()
    }
}

impl Source for PresentationClockSource {
    fn current_frame_len(&self) -> Option<usize> {
        self.samples.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.samples.total_duration()
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        self.samples.try_seek(position)
    }
}

struct ProcessedAudioFrame {
    presentation_start: Duration,
    presentation_end: Duration,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    samples: Vec<f32>,
}

/// Configuration for shortening sustained digital silence during playback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SilenceSkipping {
    amplitude_threshold: f32,
    minimum_silence: Duration,
    padding: Duration,
}

impl SilenceSkipping {
    /// Creates a validated silence-skipping policy.
    ///
    /// # Panics
    ///
    /// Panics unless `amplitude_threshold` is finite and in `0.0..=1.0`,
    /// `minimum_silence` is non-zero, and twice the padding is strictly less
    /// than the minimum silence duration.
    #[must_use]
    pub fn new(amplitude_threshold: f32, minimum_silence: Duration, padding: Duration) -> Self {
        assert!(
            amplitude_threshold.is_finite() && (0.0..=1.0).contains(&amplitude_threshold),
            "silence amplitude threshold must be finite and within 0.0..=1.0"
        );
        assert!(
            !minimum_silence.is_zero(),
            "minimum silence duration must be non-zero"
        );
        assert!(
            padding.saturating_mul(2) < minimum_silence,
            "twice the silence padding must be below the minimum silence duration"
        );
        Self {
            amplitude_threshold,
            minimum_silence,
            padding,
        }
    }

    /// Balanced defaults: 100 ms minimum silence, 20 ms edge padding, and a
    /// normalized amplitude threshold equivalent to 1024 in signed 16-bit PCM.
    #[must_use]
    pub const fn standard() -> Self {
        Self {
            amplitude_threshold: 1024.0 / 32768.0,
            minimum_silence: Duration::from_millis(100),
            padding: Duration::from_millis(20),
        }
    }
}

impl Default for SilenceSkipping {
    fn default() -> Self {
        Self::standard()
    }
}

enum SilenceOutput {
    Pcm(DecodedAudioFrame),
    ClockJump {
        presentation_start: Duration,
        presentation_end: Duration,
    },
}

struct SilenceSkippingProcessor {
    policy: SilenceSkipping,
    enabled: bool,
    candidate: VecDeque<DecodedAudioFrame>,
    candidate_duration: Duration,
    trailing: VecDeque<DecodedAudioFrame>,
    trailing_duration: Duration,
    skipping: bool,
}

impl SilenceSkippingProcessor {
    const fn new(policy: SilenceSkipping) -> Self {
        Self {
            policy,
            enabled: false,
            candidate: VecDeque::new(),
            candidate_duration: Duration::ZERO,
            trailing: VecDeque::new(),
            trailing_duration: Duration::ZERO,
            skipping: false,
        }
    }

    fn process(&mut self, frame: DecodedAudioFrame) -> Vec<SilenceOutput> {
        if !self.enabled {
            return vec![SilenceOutput::Pcm(frame)];
        }
        if !self.is_silent(&frame) {
            let mut output = self.flush_pending();
            output.push(SilenceOutput::Pcm(frame));
            return output;
        }
        if self.skipping {
            return self.push_trailing(frame);
        }

        self.candidate_duration = self.candidate_duration.saturating_add(frame.duration());
        self.candidate.push_back(frame);
        if self.candidate_duration < self.policy.minimum_silence {
            return Vec::new();
        }

        self.skipping = true;
        let mut leading_duration = Duration::ZERO;
        let mut output = Vec::new();
        while let Some(candidate) = self.candidate.pop_front() {
            self.candidate_duration = self.candidate_duration.saturating_sub(candidate.duration());
            if leading_duration < self.policy.padding {
                leading_duration = leading_duration.saturating_add(candidate.duration());
                output.push(SilenceOutput::Pcm(candidate));
            } else {
                output.extend(self.push_trailing(candidate));
            }
        }
        output
    }

    fn set_enabled(&mut self, enabled: bool) -> Vec<SilenceOutput> {
        if self.enabled == enabled {
            return Vec::new();
        }
        let pending = self.flush_pending();
        self.enabled = enabled;
        pending
    }

    fn finish(&mut self) -> Vec<SilenceOutput> {
        self.flush_pending()
    }

    fn reset(&mut self) {
        self.candidate.clear();
        self.candidate_duration = Duration::ZERO;
        self.trailing.clear();
        self.trailing_duration = Duration::ZERO;
        self.skipping = false;
    }

    fn is_silent(&self, frame: &DecodedAudioFrame) -> bool {
        frame
            .samples()
            .iter()
            .all(|sample| sample.abs() <= self.policy.amplitude_threshold)
    }

    fn push_trailing(&mut self, frame: DecodedAudioFrame) -> Vec<SilenceOutput> {
        self.trailing_duration = self.trailing_duration.saturating_add(frame.duration());
        self.trailing.push_back(frame);
        let mut output = Vec::new();
        while self.trailing.len() > 1 && self.trailing_duration > self.policy.padding {
            let skipped = self
                .trailing
                .pop_front()
                .expect("non-empty silence trailing queue must pop");
            self.trailing_duration = self.trailing_duration.saturating_sub(skipped.duration());
            output.push(SilenceOutput::ClockJump {
                presentation_start: skipped.presentation_time(),
                presentation_end: skipped
                    .presentation_time()
                    .checked_add(skipped.duration())
                    .expect("decoded silence presentation range must fit Duration"),
            });
        }
        output
    }

    fn flush_pending(&mut self) -> Vec<SilenceOutput> {
        let frames = if self.skipping {
            self.trailing.drain(..).collect::<Vec<_>>()
        } else {
            self.candidate.drain(..).collect::<Vec<_>>()
        };
        self.candidate_duration = Duration::ZERO;
        self.trailing_duration = Duration::ZERO;
        self.skipping = false;
        frames.into_iter().map(SilenceOutput::Pcm).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RateMode {
    rate: f32,
    preserve_pitch: bool,
}

struct StreamingRateProcessor {
    mode: Option<RateMode>,
    engine: Option<PitchStretchEngine>,
    channels: Option<NonZeroU16>,
    sample_rate: Option<NonZeroU32>,
    presentation_cursor: Option<Duration>,
    accepted_end: Option<Duration>,
}

impl StreamingRateProcessor {
    const fn new() -> Self {
        Self {
            mode: None,
            engine: None,
            channels: None,
            sample_rate: None,
            presentation_cursor: None,
            accepted_end: None,
        }
    }

    fn process(
        &mut self,
        frame: DecodedAudioFrame,
        params: &PlaybackParams,
    ) -> Result<Vec<ProcessedAudioFrame>, PlayerError> {
        let mode = RateMode {
            rate: params.rate(),
            preserve_pitch: params.preserve_pitch(),
        };
        let channels = frame.channels();
        let sample_rate = frame.sample_rate();
        let format_changed = self.channels.is_some_and(|current| current != channels)
            || self
                .sample_rate
                .is_some_and(|current| current != sample_rate);
        let mode_changed = self.mode.is_some_and(|current| current != mode);
        let mut output = Vec::with_capacity(2);
        if format_changed || mode_changed {
            if let Some(flushed) = self.flush()? {
                output.push(flushed);
            }
            self.reset();
        }

        self.mode = Some(mode);
        self.channels = Some(channels);
        self.sample_rate = Some(sample_rate);

        if !should_use_pitch_stretch(mode.rate, mode.preserve_pitch) {
            output.push(Self::unprocessed(frame)?);
            return Ok(output);
        }

        let presentation_start = frame.presentation_time();
        let presentation_end = presentation_start
            .checked_add(frame.duration())
            .ok_or_else(|| timeline_overflow_error("decoded audio presentation range"))?;
        let samples = frame.into_samples().into_vec();
        self.presentation_cursor.get_or_insert(presentation_start);
        self.accepted_end = Some(presentation_end);
        let engine = self.engine.get_or_insert_with(|| {
            PitchStretchEngine::new(
                usize::from(channels.get()),
                sample_rate.get(),
                1.0 / mode.rate,
            )
        });
        let stretched = engine.process(&samples);
        if let Some(processed) = self.map_stretched_samples(stretched, false)? {
            output.push(processed);
        }
        Ok(output)
    }

    fn unprocessed(frame: DecodedAudioFrame) -> Result<ProcessedAudioFrame, PlayerError> {
        let presentation_start = frame.presentation_time();
        let presentation_end = presentation_start
            .checked_add(frame.duration())
            .ok_or_else(|| timeline_overflow_error("decoded audio presentation range"))?;
        let channels = frame.channels();
        let sample_rate = frame.sample_rate();
        Ok(ProcessedAudioFrame {
            presentation_start,
            presentation_end,
            channels,
            sample_rate,
            samples: frame.into_samples().into_vec(),
        })
    }

    fn flush(&mut self) -> Result<Option<ProcessedAudioFrame>, PlayerError> {
        let Some(engine) = self.engine.as_mut() else {
            return Ok(None);
        };
        let samples = engine.flush();
        self.map_stretched_samples(samples, true)
    }

    fn map_stretched_samples(
        &mut self,
        samples: Vec<f32>,
        final_output: bool,
    ) -> Result<Option<ProcessedAudioFrame>, PlayerError> {
        if samples.is_empty() {
            return Ok(None);
        }
        let mode = self
            .mode
            .expect("stretched output must have an active playback mode");
        let channels = self
            .channels
            .expect("stretched output must have an active channel count");
        let sample_rate = self
            .sample_rate
            .expect("stretched output must have an active sample rate");
        assert!(
            samples.len().is_multiple_of(usize::from(channels.get())),
            "pitch stretcher emitted channel-misaligned PCM: channels={} samples={}",
            channels,
            samples.len()
        );
        let presentation_start = self
            .presentation_cursor
            .expect("stretched output must have a presentation cursor");
        let accepted_end = self
            .accepted_end
            .expect("stretched output must have an accepted presentation end");
        let output_frames = u32::try_from(samples.len() / usize::from(channels.get()))
            .expect("stretched PCM frame count must fit u32");
        let output_seconds = f64::from(output_frames) / f64::from(sample_rate.get());
        let represented_duration = Duration::from_secs_f64(output_seconds * f64::from(mode.rate));
        let estimated_end = presentation_start
            .checked_add(represented_duration)
            .ok_or_else(|| timeline_overflow_error("stretched audio presentation range"))?;
        let presentation_end = if final_output {
            accepted_end
        } else {
            estimated_end.min(accepted_end)
        };
        assert!(
            presentation_end > presentation_start,
            "pitch stretcher emitted PCM without advancing the presentation timeline"
        );
        self.presentation_cursor = Some(presentation_end);
        Ok(Some(ProcessedAudioFrame {
            presentation_start,
            presentation_end,
            channels,
            sample_rate,
            samples,
        }))
    }

    fn reset(&mut self) {
        self.mode = None;
        self.engine = None;
        self.channels = None;
        self.sample_rate = None;
        self.presentation_cursor = None;
        self.accepted_end = None;
    }
}

enum OutputCommand {
    Enqueue(DecodedAudioFrame),
    Reset,
    Play,
    Pause,
    SetVolume(f32),
    SetPlaybackRate(f32),
    SetPreservePitch(bool),
    SetSkipSilence(bool),
    Finish,
    Shutdown,
}

struct OutputState {
    sink: Sink,
    params: PlaybackParams,
    rate_processor: StreamingRateProcessor,
    silence_skipper: SilenceSkippingProcessor,
    generation: Arc<AtomicU64>,
    position_nanos: Arc<AtomicU64>,
    buffer_progress: async_channel::Sender<()>,
}

impl OutputState {
    fn new(
        handle: &OutputStreamHandle,
        generation: Arc<AtomicU64>,
        position_nanos: Arc<AtomicU64>,
        buffer_progress: async_channel::Sender<()>,
    ) -> Result<Self, PlayerError> {
        let sink = Sink::try_new(handle)
            .map_err(|error| PlayerError::OutputInitFailed(error.to_string()))?;
        sink.pause();
        Ok(Self {
            sink,
            params: PlaybackParams::new(),
            rate_processor: StreamingRateProcessor::new(),
            silence_skipper: SilenceSkippingProcessor::new(SilenceSkipping::standard()),
            generation,
            position_nanos,
            buffer_progress,
        })
    }

    fn handle(&mut self, command: OutputCommand) -> Result<bool, PlayerError> {
        match command {
            OutputCommand::Enqueue(frame) => self.enqueue(frame)?,
            OutputCommand::Reset => {
                self.sink.clear();
                self.rate_processor.reset();
                self.silence_skipper.reset();
            }
            OutputCommand::Play => self.sink.play(),
            OutputCommand::Pause => self.sink.pause(),
            OutputCommand::SetVolume(volume) => self.sink.set_volume(volume),
            OutputCommand::SetPlaybackRate(rate) => {
                self.params.set_rate(rate);
                self.sync_sink_speed();
            }
            OutputCommand::SetPreservePitch(preserve_pitch) => {
                self.params.set_preserve_pitch(preserve_pitch);
                self.sync_sink_speed();
            }
            OutputCommand::SetSkipSilence(enabled) => {
                let pending = self.silence_skipper.set_enabled(enabled);
                self.append_silence_outputs(pending)?;
            }
            OutputCommand::Finish => self.finish()?,
            OutputCommand::Shutdown => {
                self.sink.stop();
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn enqueue(&mut self, frame: DecodedAudioFrame) -> Result<(), PlayerError> {
        let output = self.silence_skipper.process(frame);
        self.append_silence_outputs(output)
    }

    fn finish(&mut self) -> Result<(), PlayerError> {
        let pending = self.silence_skipper.finish();
        self.append_silence_outputs(pending)?;
        let generation = self.generation.load(Ordering::Acquire);
        if let Some(processed) = self.rate_processor.flush()? {
            self.append_processed(processed, generation)?;
        }
        Ok(())
    }

    fn append_silence_outputs(&mut self, outputs: Vec<SilenceOutput>) -> Result<(), PlayerError> {
        let generation = self.generation.load(Ordering::Acquire);
        for output in outputs {
            match output {
                SilenceOutput::Pcm(frame) => {
                    for processed in self.rate_processor.process(frame, &self.params)? {
                        self.append_processed(processed, generation)?;
                    }
                }
                SilenceOutput::ClockJump {
                    presentation_start,
                    presentation_end,
                } => self.append_clock_jump(presentation_start, presentation_end, generation)?,
            }
        }
        Ok(())
    }

    fn append_clock_jump(
        &self,
        presentation_start: Duration,
        presentation_end: Duration,
        generation: u64,
    ) -> Result<(), PlayerError> {
        let source = PresentationClockSource::clock_jump(
            presentation_start,
            presentation_end,
            generation,
            Arc::clone(&self.generation),
            Arc::clone(&self.position_nanos),
            self.buffer_progress.clone(),
        )?;
        self.sink.append(source);
        Ok(())
    }

    fn append_processed(
        &self,
        processed: ProcessedAudioFrame,
        generation: u64,
    ) -> Result<(), PlayerError> {
        let source = PresentationClockSource::from_processed(
            processed,
            generation,
            Arc::clone(&self.generation),
            Arc::clone(&self.position_nanos),
            self.buffer_progress.clone(),
        )?;
        self.sink.append(source);
        Ok(())
    }

    fn sync_sink_speed(&self) {
        self.sink.set_speed(sink_speed_for_playback(
            self.params.rate(),
            self.params.preserve_pitch(),
        ));
    }
}

/// Incremental PCM player used by segmented media pipelines.
///
/// Decoded frames are queued without concatenating or rewriting them. The
/// reported position comes from PCM samples actually pulled by the platform
/// output callback, so video can synchronize against audio hardware progress.
pub struct StreamingAudioPlayer {
    control: StreamingAudioControl,
    buffer_progress: async_channel::Receiver<()>,
    output_thread: Option<JoinHandle<()>>,
}

/// Cloneable decoded-PCM producer for one owned streaming audio output.
///
/// The [`StreamingAudioPlayer`] owner may move to a playback coordinator while
/// a decoder thread retains this handle. Dropping the producer never stops the
/// platform stream; the output lifetime remains owned by the player.
#[derive(Clone)]
pub struct StreamingAudioProducer {
    control: StreamingAudioControl,
    buffer_progress: async_channel::Receiver<()>,
}

/// Cloneable control and presentation-clock handle for incremental PCM output.
///
/// The owning [`StreamingAudioPlayer`] remains with the single decoded-PCM
/// producer. Playback coordination and rendering may clone this handle without
/// gaining a second PCM enqueue path.
#[derive(Clone)]
pub struct StreamingAudioControl {
    commands: Sender<OutputCommand>,
    generation: Arc<AtomicU64>,
    position_nanos: Arc<AtomicU64>,
    queued_end_nanos: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
}

impl std::fmt::Debug for StreamingAudioControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StreamingAudioControl")
            .field("position", &self.position())
            .field("buffered_duration", &self.buffered_duration())
            .field("is_paused", &self.is_paused())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for StreamingAudioPlayer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StreamingAudioPlayer")
            .field("position", &self.position())
            .field("buffered_duration", &self.buffered_duration())
            .field("is_paused", &self.is_paused())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for StreamingAudioProducer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StreamingAudioProducer")
            .field("buffered_duration", &self.buffered_duration())
            .finish_non_exhaustive()
    }
}

impl StreamingAudioPlayer {
    /// Opens the platform's default audio output in a paused state.
    ///
    /// # Errors
    ///
    /// Returns an error when no output device or stream is available.
    pub fn new() -> Result<Self, PlayerError> {
        Self::new_with_output(&AudioOutput::system_default())
    }

    /// Opens the selected audio output in a paused state.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected device or output stream cannot be opened.
    pub fn new_with_output(output: &AudioOutput) -> Result<Self, PlayerError> {
        Self::open_output(output.clone())
    }

    fn open_output(output: AudioOutput) -> Result<Self, PlayerError> {
        let generation = Arc::new(AtomicU64::new(0));
        let position_nanos = Arc::new(AtomicU64::new(0));
        let (commands, command_receiver) = mpsc::channel();
        let (buffer_progress_sender, buffer_progress) = async_channel::bounded(1);
        let (initialization_tx, initialization_rx) = mpsc::sync_channel(1);
        let output_generation = Arc::clone(&generation);
        let output_position = Arc::clone(&position_nanos);
        let output_thread = std::thread::Builder::new()
            .name(String::from("waterkit-pcm-output"))
            .spawn(move || {
                let output = open_stream(&output).and_then(|(stream, handle)| {
                    let mut state = OutputState::new(
                        &handle,
                        Arc::clone(&output_generation),
                        Arc::clone(&output_position),
                        buffer_progress_sender,
                    )?;
                    initialization_tx.send(Ok(())).map_err(|_| {
                        PlayerError::OutputInitFailed(String::from(
                            "PCM output owner was dropped during initialization",
                        ))
                    })?;
                    for command in command_receiver {
                        if !state.handle(command)? {
                            break;
                        }
                    }
                    drop(stream);
                    Ok(())
                });
                if let Err(error) = output {
                    let _ = initialization_tx.send(Err(error));
                }
            })
            .map_err(|error| PlayerError::OutputInitFailed(error.to_string()))?;
        initialization_rx.recv().map_err(|_| {
            PlayerError::OutputInitFailed(String::from(
                "PCM output thread exited before initialization",
            ))
        })??;

        Ok(Self {
            control: StreamingAudioControl {
                commands,
                generation,
                position_nanos,
                queued_end_nanos: Arc::new(AtomicU64::new(0)),
                paused: Arc::new(AtomicBool::new(true)),
            },
            buffer_progress,
            output_thread: Some(output_thread),
        })
    }

    /// Returns a cloneable playback-control and presentation-clock handle.
    #[must_use]
    pub fn control(&self) -> StreamingAudioControl {
        self.control.clone()
    }

    /// Returns a cloneable decoded-PCM producer that does not own output lifetime.
    #[must_use]
    pub fn producer(&self) -> StreamingAudioProducer {
        StreamingAudioProducer {
            control: self.control.clone(),
            buffer_progress: self.buffer_progress.clone(),
        }
    }

    /// Returns the single-consumer notification source for PCM consumption.
    ///
    /// One notification means that at least one queued frame completed or was
    /// invalidated. The producer must re-read [`Self::buffered_duration`] after
    /// every notification instead of assuming a fixed amount of capacity.
    #[must_use]
    pub fn buffer_progress_receiver(&self) -> async_channel::Receiver<()> {
        self.buffer_progress.clone()
    }

    /// Queues one decoded PCM frame.
    ///
    /// # Errors
    ///
    /// Returns an error for an out-of-order frame, a timeline outside the
    /// representable audio clock range, or an unavailable output thread.
    pub fn enqueue(&self, frame: DecodedAudioFrame) -> Result<(), PlayerError> {
        self.control.enqueue(frame)
    }

    /// Invalidates queued PCM and reanchors the audio clock without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested position exceeds the audio clock range
    /// or the output thread is unavailable.
    pub fn reset_to(&self, position: Duration) -> Result<(), PlayerError> {
        self.control.reset_to(position)
    }

    /// Flushes pitch-preservation latency after the final decoded frame.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn finish(&self) -> Result<(), PlayerError> {
        self.control.finish()
    }

    /// Starts or resumes platform audio output.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn play(&self) -> Result<(), PlayerError> {
        self.control.play()
    }

    /// Pauses platform audio output while retaining queued PCM.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn pause(&self) -> Result<(), PlayerError> {
        self.control.pause()
    }

    /// Sets linear output gain in the inclusive range `0.0..=1.0`.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn set_volume(&self, volume: f32) -> Result<(), PlayerError> {
        self.control.set_volume(volume)
    }

    /// Sets playback rate in the inclusive range `0.25..=4.0`.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn set_playback_rate(&self, rate: f32) -> Result<(), PlayerError> {
        self.control.set_playback_rate(rate)
    }

    /// Enables or disables pitch preservation during rate changes.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn set_preserve_pitch(&self, preserve_pitch: bool) -> Result<(), PlayerError> {
        self.control.set_preserve_pitch(preserve_pitch)
    }

    /// Enables or disables shortening sustained digital silence.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn set_skip_silence(&self, enabled: bool) -> Result<(), PlayerError> {
        self.control.set_skip_silence(enabled)
    }

    /// Returns whether platform audio output is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.control.is_paused()
    }

    /// Returns the presentation position of the latest PCM frame consumed by the output callback.
    #[must_use]
    pub fn position(&self) -> Duration {
        self.control.position()
    }

    /// Returns decoded PCM queued ahead of the consumed audio position.
    #[must_use]
    pub fn buffered_duration(&self) -> Duration {
        self.control.buffered_duration()
    }
}

impl StreamingAudioProducer {
    /// Returns the single-consumer PCM progress notification source.
    #[must_use]
    pub fn buffer_progress_receiver(&self) -> async_channel::Receiver<()> {
        self.buffer_progress.clone()
    }

    /// Queues one decoded PCM frame.
    ///
    /// # Errors
    ///
    /// Returns an error for an out-of-order frame, an overflowing timeline, or
    /// an unavailable output owner.
    pub fn enqueue(&self, frame: DecodedAudioFrame) -> Result<(), PlayerError> {
        self.control.enqueue(frame)
    }

    /// Invalidates queued PCM and reanchors the producer timeline.
    ///
    /// # Errors
    ///
    /// Returns an error for an overflowing position or unavailable output owner.
    pub fn reset_to(&self, position: Duration) -> Result<(), PlayerError> {
        self.control.reset_to(position)
    }

    /// Flushes pending producer-side processing at end of input.
    ///
    /// # Errors
    ///
    /// Returns an error when the output owner is unavailable.
    pub fn finish(&self) -> Result<(), PlayerError> {
        self.control.finish()
    }

    /// Returns decoded PCM queued ahead of the consumed output position.
    #[must_use]
    pub fn buffered_duration(&self) -> Duration {
        self.control.buffered_duration()
    }
}

impl StreamingAudioControl {
    fn enqueue(&self, frame: DecodedAudioFrame) -> Result<(), PlayerError> {
        let start = duration_to_nanos(frame.presentation_time())?;
        let end = duration_to_nanos(
            frame
                .presentation_time()
                .checked_add(frame.duration())
                .ok_or_else(|| timeline_overflow_error("decoded audio presentation range"))?,
        )?;
        let queued_end = self.queued_end_nanos.load(Ordering::Acquire);
        if start < queued_end {
            return Err(PlayerError::PlaybackFailed(format!(
                "decoded audio frame at {:?} precedes queued end {:?}",
                frame.presentation_time(),
                Duration::from_nanos(queued_end)
            )));
        }
        self.send(OutputCommand::Enqueue(frame))?;
        self.queued_end_nanos.store(end, Ordering::Release);
        Ok(())
    }

    /// Invalidates queued PCM and reanchors the audio clock without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested position exceeds the audio clock
    /// range or the output thread is unavailable.
    pub fn reset_to(&self, position: Duration) -> Result<(), PlayerError> {
        let position_nanos = duration_to_nanos(position)?;
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.position_nanos.store(position_nanos, Ordering::Release);
        self.queued_end_nanos
            .store(position_nanos, Ordering::Release);
        self.send(OutputCommand::Reset)
    }

    /// Flushes pitch-preservation latency after the final decoded frame.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn finish(&self) -> Result<(), PlayerError> {
        self.send(OutputCommand::Finish)
    }

    /// Starts or resumes platform audio output.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn play(&self) -> Result<(), PlayerError> {
        self.paused.store(false, Ordering::Release);
        self.send(OutputCommand::Play)
    }

    /// Pauses platform audio output while retaining queued PCM.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn pause(&self) -> Result<(), PlayerError> {
        self.paused.store(true, Ordering::Release);
        self.send(OutputCommand::Pause)
    }

    /// Sets linear output gain in the inclusive range `0.0..=1.0`.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn set_volume(&self, volume: f32) -> Result<(), PlayerError> {
        self.send(OutputCommand::SetVolume(volume.clamp(0.0, 1.0)))
    }

    /// Sets playback rate in the inclusive range `0.25..=4.0`.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn set_playback_rate(&self, rate: f32) -> Result<(), PlayerError> {
        self.send(OutputCommand::SetPlaybackRate(clamp_playback_rate(rate)))
    }

    /// Enables or disables pitch preservation during rate changes.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn set_preserve_pitch(&self, preserve_pitch: bool) -> Result<(), PlayerError> {
        self.send(OutputCommand::SetPreservePitch(preserve_pitch))
    }

    /// Enables or disables shortening sustained digital silence.
    ///
    /// The audio presentation clock advances across removed samples, allowing
    /// a synchronized video renderer to discard the corresponding video time.
    ///
    /// # Errors
    ///
    /// Returns an error when the output thread is unavailable.
    pub fn set_skip_silence(&self, enabled: bool) -> Result<(), PlayerError> {
        self.send(OutputCommand::SetSkipSilence(enabled))
    }

    /// Returns whether platform audio output is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Returns the presentation position consumed by platform audio output.
    #[must_use]
    pub fn position(&self) -> Duration {
        Duration::from_nanos(self.position_nanos.load(Ordering::Acquire))
    }

    /// Returns decoded PCM queued ahead of the consumed audio position.
    #[must_use]
    pub fn buffered_duration(&self) -> Duration {
        let position = self.position_nanos.load(Ordering::Acquire);
        let queued_end = self.queued_end_nanos.load(Ordering::Acquire);
        Duration::from_nanos(queued_end.saturating_sub(position))
    }

    fn send(&self, command: OutputCommand) -> Result<(), PlayerError> {
        self.commands.send(command).map_err(|_| {
            PlayerError::PlaybackFailed(String::from("PCM output thread is unavailable"))
        })
    }
}

impl Drop for StreamingAudioPlayer {
    fn drop(&mut self) {
        let _ = self.control.commands.send(OutputCommand::Shutdown);
        if let Some(output_thread) = self.output_thread.take() {
            output_thread
                .join()
                .expect("PCM output thread must not panic during shutdown");
        }
    }
}

fn open_stream(output: &AudioOutput) -> Result<(OutputStream, OutputStreamHandle), PlayerError> {
    #[cfg(target_os = "ios")]
    if output.selected_device().is_some() {
        return Err(PlayerError::OutputDeviceSelectionUnavailable);
    }

    #[cfg(not(target_os = "ios"))]
    if let Some(device) = output.selected_device() {
        return OutputStream::try_from_device(&device.handle)
            .map_err(|error| PlayerError::OutputInitFailed(error.to_string()));
    }

    OutputStream::try_default().map_err(|error| PlayerError::OutputInitFailed(error.to_string()))
}

fn timeline_overflow_error(label: &str) -> PlayerError {
    PlayerError::PlaybackFailed(format!("{label} exceeds Duration"))
}

fn duration_to_nanos(duration: Duration) -> Result<u64, PlayerError> {
    u64::try_from(duration.as_nanos()).map_err(|_| {
        PlayerError::PlaybackFailed(format!(
            "audio timestamp {duration:?} exceeds the u64 nanosecond clock range"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::{
        PresentationClockSource, SilenceOutput, SilenceSkipping, SilenceSkippingProcessor,
        StreamingRateProcessor,
    };
    use crate::DecodedAudioFrame;
    use crate::playback_rate::PlaybackParams;

    fn decoded_frame() -> DecodedAudioFrame {
        DecodedAudioFrame::from_interleaved_pcm(
            Duration::from_secs(3),
            NonZeroU16::new(2).expect("two channels must be non-zero"),
            NonZeroU32::new(2).expect("sample rate must be non-zero"),
            vec![0.0; 8],
        )
        .expect("aligned PCM fixture must be valid")
    }

    fn mono_frame(index: u32, amplitude: f32) -> DecodedAudioFrame {
        DecodedAudioFrame::from_interleaved_pcm(
            Duration::from_millis(u64::from(index) * 25),
            NonZeroU16::MIN,
            NonZeroU32::new(400).expect("sample rate must be non-zero"),
            vec![amplitude; 10],
        )
        .expect("mono PCM fixture must be aligned")
    }

    fn output_timeline(outputs: Vec<SilenceOutput>) -> Vec<(Duration, bool)> {
        outputs
            .into_iter()
            .map(|output| match output {
                SilenceOutput::Pcm(frame) => (frame.presentation_time(), true),
                SilenceOutput::ClockJump {
                    presentation_start, ..
                } => (presentation_start, false),
            })
            .collect()
    }

    #[test]
    fn sustained_silence_keeps_edge_padding_and_emits_clock_jumps() {
        let mut processor = SilenceSkippingProcessor::new(SilenceSkipping::new(
            0.01,
            Duration::from_millis(100),
            Duration::from_millis(25),
        ));
        assert!(processor.set_enabled(true).is_empty());
        let mut output = Vec::new();
        for index in 0..10 {
            output.extend(processor.process(mono_frame(index, 0.0)));
        }
        output.extend(processor.process(mono_frame(10, 0.5)));

        assert_eq!(
            output_timeline(output),
            vec![
                (Duration::ZERO, true),
                (Duration::from_millis(25), false),
                (Duration::from_millis(50), false),
                (Duration::from_millis(75), false),
                (Duration::from_millis(100), false),
                (Duration::from_millis(125), false),
                (Duration::from_millis(150), false),
                (Duration::from_millis(175), false),
                (Duration::from_millis(200), false),
                (Duration::from_millis(225), true),
                (Duration::from_millis(250), true),
            ]
        );
    }

    #[test]
    fn short_silence_is_not_removed() {
        let mut processor = SilenceSkippingProcessor::new(SilenceSkipping::new(
            0.01,
            Duration::from_millis(100),
            Duration::from_millis(25),
        ));
        assert!(processor.set_enabled(true).is_empty());
        let mut output = Vec::new();
        for index in 0..3 {
            output.extend(processor.process(mono_frame(index, 0.0)));
        }
        output.extend(processor.process(mono_frame(3, 0.5)));

        assert_eq!(
            output_timeline(output),
            vec![
                (Duration::ZERO, true),
                (Duration::from_millis(25), true),
                (Duration::from_millis(50), true),
                (Duration::from_millis(75), true),
            ]
        );
    }

    #[test]
    fn source_clock_tracks_pcm_consumed_by_the_output_callback() {
        let generation = Arc::new(AtomicU64::new(7));
        let position = Arc::new(AtomicU64::new(0));
        let (buffer_progress, _buffer_progress_receiver) = async_channel::bounded(1);
        let mut source = PresentationClockSource::from_decoded(
            decoded_frame(),
            7,
            Arc::clone(&generation),
            Arc::clone(&position),
            buffer_progress,
        )
        .expect("PCM source must initialize");

        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(position.load(Ordering::Acquire), 3_000_000_000);
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(position.load(Ordering::Acquire), 3_500_000_000);
    }

    #[test]
    fn generation_change_discards_stale_queued_pcm() {
        let generation = Arc::new(AtomicU64::new(1));
        let position = Arc::new(AtomicU64::new(0));
        let (buffer_progress, buffer_progress_receiver) = async_channel::bounded(1);
        let mut source = PresentationClockSource::from_decoded(
            decoded_frame(),
            1,
            Arc::clone(&generation),
            position,
            buffer_progress,
        )
        .expect("PCM source must initialize");

        generation.store(2, Ordering::Release);
        assert_eq!(source.next(), None);
        buffer_progress_receiver
            .try_recv()
            .expect("invalidating queued PCM must notify the producer");
    }

    #[test]
    fn pitch_preserving_stream_keeps_a_continuous_presentation_timeline() {
        let params = PlaybackParams::new();
        params.set_rate(1.5);
        params.set_preserve_pitch(true);
        let mut processor = StreamingRateProcessor::new();
        let channels = NonZeroU16::new(2).expect("two channels must be non-zero");
        let sample_rate = NonZeroU32::new(48_000).expect("sample rate must be non-zero");
        let mut output = Vec::new();

        for index in 0..8u32 {
            let start = Duration::from_secs_f64(f64::from(index) * 1024.0 / 48_000.0);
            let frame = DecodedAudioFrame::from_interleaved_pcm(
                start,
                channels,
                sample_rate,
                vec![0.25; 2048],
            )
            .expect("PCM fixture must be valid");
            output.extend(
                processor
                    .process(frame, &params)
                    .expect("pitch-preserving processing must succeed"),
            );
        }
        if let Some(flushed) = processor
            .flush()
            .expect("pitch-preserving flush must succeed")
        {
            output.push(flushed);
        }

        assert!(!output.is_empty());
        for pair in output.windows(2) {
            assert_eq!(pair[0].presentation_end, pair[1].presentation_start);
        }
        assert!(
            output
                .last()
                .expect("processed output must have a final frame")
                .presentation_end
                .abs_diff(Duration::from_secs_f64(8192.0 / 48_000.0))
                <= Duration::from_nanos(1)
        );
    }
}
