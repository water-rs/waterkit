//! Shared playback-rate and pitch-preservation processing.

use std::collections::VecDeque;
#[cfg(all(feature = "playback", not(target_os = "ios")))]
use std::num::{NonZeroU16, NonZeroU32};
#[cfg(all(feature = "playback", not(target_os = "ios")))]
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
#[cfg(all(feature = "playback", not(target_os = "ios")))]
use std::time::Duration;

use num_traits::ToPrimitive;
#[cfg(all(feature = "playback", not(target_os = "ios")))]
use rodio::Source;
use timestretch::engine::{
    Engine, EngineConfig, EngineController, EngineProcessor, EngineProfile, SourceProducer,
};

const PRESERVE_PITCH_RATE_EPSILON: f32 = 0.001;
const STRETCH_CHUNK_FRAMES: usize = 2048;

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

#[derive(Debug)]
struct RealtimeStretch {
    controller: EngineController,
    processor: EngineProcessor,
    source: SourceProducer,
    channels: usize,
    stretch_ratio: f64,
    expected_output_frames: f64,
    rendered_output_frames: u64,
    pipeline_latency_frames: usize,
    latency_remaining_frames: usize,
    padding: Vec<f32>,
    finished: bool,
}

impl RealtimeStretch {
    fn new(channels: usize, sample_rate: u32, stretch_ratio: f64) -> Self {
        let handles = Engine::build(EngineConfig {
            sample_rate,
            channels,
            profile: EngineProfile::Keylock,
            initial_tempo_rate: 1.0 / stretch_ratio,
            max_block_frames: STRETCH_CHUNK_FRAMES,
            ..EngineConfig::default()
        })
        .expect("pitch-preserving engine configuration must be valid");
        let pipeline_latency_frames = handles.processor.pipeline_latency_frames();
        Self {
            controller: handles.controller,
            processor: handles.processor,
            source: handles.source,
            channels,
            stretch_ratio,
            expected_output_frames: 0.0,
            rendered_output_frames: 0,
            pipeline_latency_frames,
            latency_remaining_frames: pipeline_latency_frames,
            padding: Vec::new(),
            finished: false,
        }
    }

    #[cfg(all(feature = "playback", not(target_os = "ios")))]
    fn set_stretch_ratio(&mut self, stretch_ratio: f64) {
        self.stretch_ratio = stretch_ratio;
        self.controller.set_tempo_rate(1.0 / stretch_ratio);
    }

    fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        assert!(!self.finished, "finished pitch stretcher cannot accept PCM");
        assert!(
            interleaved.len().is_multiple_of(self.channels),
            "real-time pitch stretcher received channel-misaligned PCM"
        );

        let mut output = Vec::new();
        let mut offset = 0;
        while offset < interleaved.len() {
            if self.source.free_frames() == 0 {
                let before = output.len();
                self.drain_available(&mut output);
                assert!(
                    output.len() > before,
                    "pitch stretcher source capacity exhausted before output became available"
                );
            }

            let remaining_frames = (interleaved.len() - offset) / self.channels;
            let pushed_frames = remaining_frames.min(self.source.free_frames());
            let pushed_samples = pushed_frames * self.channels;
            let accepted = self
                .source
                .push(&interleaved[offset..offset + pushed_samples]);
            assert_eq!(
                accepted, pushed_frames,
                "pitch stretcher must accept the advertised source capacity"
            );
            offset += pushed_samples;
            self.expected_output_frames += pushed_frames
                .to_f64()
                .expect("pitch stretcher input frame count must fit f64")
                * self.stretch_ratio;
            self.drain_available(&mut output);
        }
        output
    }

    fn flush(&mut self) -> Vec<f32> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;

        let expected_frames = self
            .expected_output_frames
            .round()
            .to_u64()
            .expect("pitch stretcher output frame count must fit u64");
        let target_rendered = expected_frames
            .checked_add(
                u64::try_from(self.pipeline_latency_frames)
                    .expect("pitch stretcher latency must fit u64"),
            )
            .expect("pitch stretcher output length must fit u64");
        let mut output = Vec::new();
        while self.rendered_output_frames < target_rendered {
            let remaining = usize::try_from(target_rendered - self.rendered_output_frames)
                .unwrap_or(usize::MAX);
            let output_frames = remaining.min(STRETCH_CHUNK_FRAMES);
            self.pad_source_for(output_frames);
            self.render(output_frames, &mut output);
        }

        let emitted_before_flush = self
            .rendered_output_frames
            .saturating_sub(u64::try_from(output.len() / self.channels).expect("fit u64"))
            .saturating_sub(
                u64::try_from(self.pipeline_latency_frames - self.latency_remaining_frames)
                    .expect("fit u64"),
            );
        let remaining_expected = expected_frames.saturating_sub(emitted_before_flush);
        let remaining_samples = usize::try_from(remaining_expected)
            .expect("remaining stretched frame count must fit usize")
            .saturating_mul(self.channels);
        output.truncate(remaining_samples);
        output
    }

    #[cfg(all(feature = "playback", not(target_os = "ios")))]
    fn reset(&mut self) {
        self.processor.reset();
        self.expected_output_frames = 0.0;
        self.rendered_output_frames = 0;
        self.latency_remaining_frames = self.pipeline_latency_frames;
        self.finished = false;
    }

    fn drain_available(&mut self, output: &mut Vec<f32>) {
        let target = self
            .expected_output_frames
            .floor()
            .to_u64()
            .expect("pitch stretcher output frame count must fit u64");
        let remaining = target.saturating_sub(self.rendered_output_frames);
        let remaining = usize::try_from(remaining).unwrap_or(usize::MAX);
        let available = self.available_output_frames(remaining);
        if available > 0 {
            self.render(available, output);
        }
    }

    fn available_output_frames(&self, requested: usize) -> usize {
        let occupied = self.source.occupied_frames();
        let tempo = self.controller.tempo_rate_target();
        let mut low = 0;
        let mut high = requested;
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if self.source.demand_hint(middle, tempo) <= occupied {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        low
    }

    fn pad_source_for(&mut self, output_frames: usize) {
        let required = self
            .source
            .demand_hint(output_frames, self.controller.tempo_rate_target());
        while self.source.occupied_frames() < required {
            let missing = required - self.source.occupied_frames();
            let frames = missing.min(self.source.free_frames());
            assert!(
                frames > 0,
                "pitch stretcher flush padding exceeds source capacity"
            );
            let samples = frames * self.channels;
            self.padding.resize(samples, 0.0);
            let accepted = self.source.push(&self.padding[..samples]);
            assert_eq!(
                accepted, frames,
                "pitch stretcher must accept flush padding"
            );
        }
    }

    fn render(&mut self, output_frames: usize, output: &mut Vec<f32>) {
        let output_start = output.len();
        let rendered_samples = output_frames * self.channels;
        output.resize(output_start + rendered_samples, 0.0);
        self.processor.process(&mut output[output_start..]);
        self.rendered_output_frames = self
            .rendered_output_frames
            .checked_add(u64::try_from(output_frames).expect("output frame count must fit u64"))
            .expect("rendered pitch stretcher frame count must fit u64");
        let skipped_frames = output_frames.min(self.latency_remaining_frames);
        self.latency_remaining_frames -= skipped_frames;
        let skipped_samples = skipped_frames * self.channels;
        if skipped_samples != 0 {
            output.copy_within(
                output_start + skipped_samples..output_start + rendered_samples,
                output_start,
            );
            output.truncate(output.len() - skipped_samples);
        }
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
        if channels <= 8 {
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
