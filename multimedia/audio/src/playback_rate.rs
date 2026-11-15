//! Shared playback-rate and pitch-preservation processing.

use std::collections::VecDeque;
#[cfg(all(feature = "playback", not(target_os = "ios")))]
use std::num::{NonZeroU16, NonZeroU32};
#[cfg(all(feature = "playback", not(target_os = "ios")))]
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
#[cfg(all(feature = "playback", not(target_os = "ios")))]
use std::time::Duration;

#[cfg(all(feature = "playback", not(target_os = "ios")))]
use rodio::Source;
use timestretch::{
    Parameters, PreparedSession, ProcessStatus, SessionConfig, StreamSession, TempoRate,
};

const PRESERVE_PITCH_RATE_EPSILON: f32 = 0.001;
const STRETCH_CHUNK_FRAMES: usize = 2048;
const OUTPUT_BLOCK_FRAMES: usize = 256;

pub const fn clamp_playback_rate(rate: f32) -> f32 {
    if rate.is_finite() {
        if rate < 0.25 {
            0.25
        } else if rate > 4.0 {
            4.0
        } else {
            rate
        }
    } else {
        1.0
    }
}

pub fn should_use_pitch_stretch(rate: f32, preserve_pitch: bool) -> bool {
    preserve_pitch && (rate - 1.0).abs() > PRESERVE_PITCH_RATE_EPSILON
}

pub fn sink_speed_for_playback(rate: f32, preserve_pitch: bool) -> f32 {
    if should_use_pitch_stretch(rate, preserve_pitch) {
        1.0
    } else {
        rate
    }
}

#[cfg(all(feature = "playback", not(target_os = "ios")))]
pub fn duration_mul_rate(duration: Duration, rate: f32) -> Duration {
    duration.mul_f64(f64::from(rate))
}

#[cfg(all(feature = "playback", not(target_os = "ios")))]
pub fn duration_div_rate(duration: Duration, rate: f32) -> Duration {
    duration.mul_f64(1.0 / f64::from(rate))
}

#[derive(Debug)]
pub struct PlaybackParams {
    rate_bits: AtomicU32,
    preserve_pitch: AtomicBool,
}

impl PlaybackParams {
    pub const fn new() -> Self {
        Self {
            rate_bits: AtomicU32::new(1.0f32.to_bits()),
            preserve_pitch: AtomicBool::new(true),
        }
    }

    pub fn rate(&self) -> f32 {
        let encoded = f32::from_bits(self.rate_bits.load(Ordering::Acquire));
        clamp_playback_rate(encoded)
    }

    pub fn set_rate(&self, rate: f32) {
        let clamped = clamp_playback_rate(rate);
        self.rate_bits.store(clamped.to_bits(), Ordering::Release);
    }

    pub fn preserve_pitch(&self) -> bool {
        self.preserve_pitch.load(Ordering::Acquire)
    }

    pub fn set_preserve_pitch(&self, preserve_pitch: bool) {
        self.preserve_pitch.store(preserve_pitch, Ordering::Release);
    }
}

#[derive(Debug)]
enum StretchCore {
    Interleaved {
        processor: Box<RealtimeStretch>,
    },
    PerChannel {
        processors: Vec<RealtimeStretch>,
        pending: Vec<VecDeque<f32>>,
    },
}

struct RealtimeStretch {
    session: StreamSession,
    pending_input: Vec<f32>,
    channels: usize,
    sample_rate: u32,
    stretch_ratio: f64,
    command_id: u64,
    finished: bool,
}

impl std::fmt::Debug for RealtimeStretch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RealtimeStretch")
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .field("stretch_ratio", &self.stretch_ratio)
            .finish_non_exhaustive()
    }
}

impl RealtimeStretch {
    fn new(channels: usize, sample_rate: u32, stretch_ratio: f64) -> Self {
        Self {
            session: Self::build_session(channels, sample_rate, stretch_ratio),
            pending_input: Vec::new(),
            channels,
            sample_rate,
            stretch_ratio,
            command_id: 1,
            finished: false,
        }
    }

    fn build_session(channels: usize, sample_rate: u32, stretch_ratio: f64) -> StreamSession {
        let tempo =
            TempoRate::new(1.0 / stretch_ratio).expect("pitch-preserving tempo rate must be valid");
        PreparedSession::new(SessionConfig {
            channels,
            source_sample_rate: sample_rate,
            output_sample_rate: sample_rate,
            parameters: Parameters::keylock(tempo),
            ..SessionConfig::default()
        })
        .expect("pitch-preserving session configuration must be valid")
        .activate()
    }

    #[cfg(all(feature = "playback", not(target_os = "ios")))]
    fn set_stretch_ratio(&mut self, stretch_ratio: f64) {
        self.stretch_ratio = stretch_ratio;
        let tempo =
            TempoRate::new(1.0 / stretch_ratio).expect("pitch-preserving tempo rate must be valid");
        self.command_id = self.command_id.checked_add(1).expect("command id overflow");
        self.session
            .schedule(
                self.command_id,
                self.session.counters().collected_frames,
                Parameters::keylock(tempo),
            )
            .expect("pitch-preserving tempo command must be accepted");
    }

    fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        assert!(!self.finished, "finished pitch stretcher cannot accept PCM");
        assert!(
            interleaved.len().is_multiple_of(self.channels),
            "real-time pitch stretcher received channel-misaligned PCM"
        );
        self.pending_input.extend_from_slice(interleaved);
        self.pump(false)
    }

    fn flush(&mut self) -> Vec<f32> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        self.session
            .finish_input()
            .expect("pitch stretcher input must finish");
        self.pump(true)
    }

    #[cfg(all(feature = "playback", not(target_os = "ios")))]
    fn reset(&mut self) {
        self.session = Self::build_session(self.channels, self.sample_rate, self.stretch_ratio);
        self.pending_input.clear();
        self.command_id = 1;
        self.finished = false;
    }

    fn pump(&mut self, until_finished: bool) -> Vec<f32> {
        let mut output = Vec::new();
        let mut block = vec![0.0; OUTPUT_BLOCK_FRAMES * self.channels];
        loop {
            let report = self.session.advance(8);
            match report.status {
                ProcessStatus::NeedSource => {
                    let Some(request) = self.session.source_request() else {
                        panic!("pitch stretcher requested source without a request");
                    };
                    let available_frames = self.pending_input.len() / self.channels;
                    if available_frames == 0 {
                        assert!(
                            !until_finished,
                            "pitch stretcher requested source after input finished"
                        );
                        break;
                    }
                    let frames = available_frames.min(request.frames);
                    let samples = frames * self.channels;
                    let chunk = self.pending_input.drain(..samples).collect::<Vec<_>>();
                    self.session
                        .supply(request, &chunk)
                        .expect("pitch stretcher must accept supplied PCM");
                }
                ProcessStatus::Fault => {
                    panic!(
                        "pitch stretcher fault: {:?}",
                        self.session.fault().expect("fault status carries an error")
                    );
                }
                ProcessStatus::Finished => break,
                ProcessStatus::Paused => {
                    panic!("pitch stretcher session is paused")
                }
                ProcessStatus::NeedAcknowledgement
                | ProcessStatus::NeedOutput
                | ProcessStatus::Priming
                | ProcessStatus::Progress
                | ProcessStatus::Rendered => {}
            }

            let stamp = self
                .session
                .read_output(&mut block)
                .expect("pitch stretcher output must be readable");
            if stamp.frames == 0 {
                if self.session.finished() {
                    break;
                }
                if !until_finished && self.pending_input.len() < self.channels {
                    break;
                }
                continue;
            }
            output.extend_from_slice(&block[..stamp.frames * self.channels]);
            let receipt = stamp
                .consumed(stamp.frames)
                .expect("consumed frames cannot exceed the stamp");
            self.session
                .acknowledge(receipt)
                .expect("pitch stretcher output receipt must be accepted");
        }
        output
    }
}

#[derive(Debug)]
pub struct PitchStretchEngine {
    channels: usize,
    core: StretchCore,
    input_channels: Vec<Vec<f32>>,
}

impl PitchStretchEngine {
    pub fn new(channels: usize, sample_rate: u32, ratio: f32) -> Self {
        let clamped_ratio = f64::from(ratio.clamp(0.25, 4.0));
        if channels <= 2 {
            return Self {
                channels,
                core: StretchCore::Interleaved {
                    processor: Box::new(RealtimeStretch::new(channels, sample_rate, clamped_ratio)),
                },
                input_channels: Vec::new(),
            };
        }

        let mut processors = Vec::with_capacity(channels);
        let mut pending = Vec::with_capacity(channels);
        for _ in 0..channels {
            processors.push(RealtimeStretch::new(1, sample_rate, clamped_ratio));
            pending.push(VecDeque::new());
        }
        let input_channels = (0..channels)
            .map(|_| Vec::with_capacity(STRETCH_CHUNK_FRAMES))
            .collect();
        Self {
            channels,
            core: StretchCore::PerChannel {
                processors,
                pending,
            },
            input_channels,
        }
    }

    #[cfg(all(feature = "playback", not(target_os = "ios")))]
    fn set_ratio(&mut self, ratio: f32) {
        let clamped_ratio = f64::from(ratio.clamp(0.25, 4.0));
        match &mut self.core {
            StretchCore::Interleaved { processor } => {
                processor.set_stretch_ratio(clamped_ratio);
            }
            StretchCore::PerChannel { processors, .. } => {
                for processor in processors {
                    processor.set_stretch_ratio(clamped_ratio);
                }
            }
        }
    }

    pub fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        assert!(
            interleaved.len().is_multiple_of(self.channels),
            "pitch stretcher received channel-misaligned input: channels={} samples={}",
            self.channels,
            interleaved.len()
        );

        match &mut self.core {
            StretchCore::Interleaved { processor } => processor.process(interleaved),
            StretchCore::PerChannel {
                processors,
                pending,
            } => {
                for channel_input in &mut self.input_channels {
                    channel_input.clear();
                }
                for frame in interleaved.chunks_exact(self.channels) {
                    for (index, sample) in frame.iter().enumerate() {
                        self.input_channels[index].push(*sample);
                    }
                }

                for (index, processor) in processors.iter_mut().enumerate() {
                    let input = &self.input_channels[index];
                    let output = processor.process(input);
                    pending[index].extend(output.iter().copied());
                }

                Self::drain_interleaved_pending(pending)
            }
        }
    }

    pub fn flush(&mut self) -> Vec<f32> {
        match &mut self.core {
            StretchCore::Interleaved { processor } => processor.flush(),
            StretchCore::PerChannel {
                processors,
                pending,
            } => {
                for (index, processor) in processors.iter_mut().enumerate() {
                    let output = processor.flush();
                    pending[index].extend(output.iter().copied());
                }
                Self::drain_interleaved_pending(pending)
            }
        }
    }

    #[cfg(all(feature = "playback", not(target_os = "ios")))]
    fn reset(&mut self) {
        match &mut self.core {
            StretchCore::Interleaved { processor } => {
                processor.reset();
            }
            StretchCore::PerChannel {
                processors,
                pending,
            } => {
                for processor in processors {
                    processor.reset();
                }
                for channel_pending in pending {
                    channel_pending.clear();
                }
            }
        }
    }

    fn drain_interleaved_pending(pending: &mut [VecDeque<f32>]) -> Vec<f32> {
        if pending.is_empty() {
            return Vec::new();
        }

        let mut output = Vec::new();
        while pending.iter().all(|channel| !channel.is_empty()) {
            for channel in pending.iter_mut() {
                output.push(
                    channel
                        .pop_front()
                        .expect("pending channel queue must contain a sample"),
                );
            }
        }
        output
    }
}

#[cfg(all(feature = "playback", not(target_os = "ios")))]
#[derive(Debug)]
pub struct AdaptivePlaybackSource<S>
where
    S: Source<Item = f32>,
{
    inner: S,
    params: Arc<PlaybackParams>,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    chunk_buffer: Vec<f32>,
    output_buffer: VecDeque<f32>,
    stretch_engine: Option<PitchStretchEngine>,
    last_stretch_active: bool,
    input_finished: bool,
}

#[cfg(all(feature = "playback", not(target_os = "ios")))]
impl<S> AdaptivePlaybackSource<S>
where
    S: Source<Item = f32>,
{
    pub fn new(inner: S, params: Arc<PlaybackParams>) -> Self {
        let channels = inner.channels();
        let sample_rate = inner.sample_rate();

        Self {
            inner,
            params,
            channels,
            sample_rate,
            chunk_buffer: Vec::with_capacity(STRETCH_CHUNK_FRAMES * usize::from(channels.get())),
            output_buffer: VecDeque::with_capacity(
                STRETCH_CHUNK_FRAMES * usize::from(channels.get()),
            ),
            stretch_engine: None,
            last_stretch_active: false,
            input_finished: false,
        }
    }

    fn current_rate(&self) -> f32 {
        self.params.rate()
    }

    fn stretch_active(&self) -> bool {
        should_use_pitch_stretch(self.current_rate(), self.params.preserve_pitch())
    }

    fn ensure_stretch_engine(&mut self, rate: f32) -> &mut PitchStretchEngine {
        let expected_channels = usize::from(self.channels.get());
        let stretch_ratio = 1.0 / rate;
        if self
            .stretch_engine
            .as_ref()
            .is_none_or(|engine| engine.channels != expected_channels)
        {
            self.stretch_engine = Some(PitchStretchEngine::new(
                expected_channels,
                self.sample_rate.get(),
                stretch_ratio,
            ));
        }
        let engine = self
            .stretch_engine
            .as_mut()
            .expect("stretch engine must be initialized");
        engine.set_ratio(stretch_ratio);
        engine
    }

    fn read_chunk(&mut self) {
        self.chunk_buffer.clear();
        let target_samples = STRETCH_CHUNK_FRAMES.saturating_mul(usize::from(self.channels.get()));
        while self.chunk_buffer.len() < target_samples {
            if let Some(sample) = self.inner.next() {
                self.chunk_buffer.push(sample);
            } else {
                self.input_finished = true;
                break;
            }
        }

        assert!(
            self.chunk_buffer.is_empty()
                || self
                    .chunk_buffer
                    .len()
                    .is_multiple_of(usize::from(self.channels.get())),
            "audio source emitted channel-misaligned sample count: channels={} samples={}",
            self.channels,
            self.chunk_buffer.len()
        );
    }

    fn on_mode_switch(&mut self, stretch_active: bool) {
        if self.last_stretch_active == stretch_active {
            return;
        }
        self.output_buffer.clear();
        if let Some(engine) = self.stretch_engine.as_mut() {
            engine.reset();
        }
        self.last_stretch_active = stretch_active;
    }

    fn refill_output(&mut self) {
        let stretch_active = self.stretch_active();
        self.on_mode_switch(stretch_active);

        if stretch_active {
            let rate = self.current_rate();
            self.read_chunk();
            let mut input_chunk = std::mem::take(&mut self.chunk_buffer);
            let output = {
                let input_finished = self.input_finished;
                let engine = self.ensure_stretch_engine(rate);
                if input_chunk.is_empty() {
                    if input_finished {
                        engine.flush()
                    } else {
                        Vec::new()
                    }
                } else {
                    engine.process(&input_chunk)
                }
            };
            self.output_buffer.extend(output);
            input_chunk.clear();
            self.chunk_buffer = input_chunk;
            return;
        }

        self.read_chunk();
        self.output_buffer.extend(self.chunk_buffer.iter().copied());
    }
}

#[cfg(all(feature = "playback", not(target_os = "ios")))]
impl<S> Iterator for AdaptivePlaybackSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(sample) = self.output_buffer.pop_front() {
                return Some(sample);
            }

            if self.input_finished {
                self.refill_output();
                return self.output_buffer.pop_front();
            }

            self.refill_output();
        }
    }
}

#[cfg(all(feature = "playback", not(target_os = "ios")))]
impl<S> Source for AdaptivePlaybackSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> NonZeroU16 {
        self.channels
    }

    fn sample_rate(&self) -> NonZeroU32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let seek_source_position = if self.stretch_active() {
            duration_mul_rate(pos, self.current_rate())
        } else {
            pos
        };

        self.inner.try_seek(seek_source_position)?;
        self.output_buffer.clear();
        if let Some(engine) = self.stretch_engine.as_mut() {
            engine.reset();
        }
        self.input_finished = false;
        Ok(())
    }
}
