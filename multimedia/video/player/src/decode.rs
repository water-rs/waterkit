//! Container-to-codec decode pipeline.

use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    time::Duration,
};

use futures::Stream;
use num_traits::ToPrimitive;
use waterkit_audio::{
    AacDecoderConfig, AacPacketDecoder, DecodedAudioFrame, EncodedAudioPacket, PacketAudioDecoder,
};
use waterkit_codec::{CodecType, DecodePacket, DecodedFrame, Decoder};
use waterkit_video_container::{
    Codec as ContainerCodec, EncodedSample, TrackId, TrackInfo, TrackKind, VideoReader,
    probe_mp4_color_info,
};
use waterkit_video_core::{Error, FrameTiming, VideoColorInfo};

/// Persistent decoder for presentation-wide encoded audio samples.
///
/// The container/session layer owns buffering and track selection. This type
/// validates the selected track, translates exact container timestamps, and
/// resets codec state at discontinuities.
pub struct AudioTrackDecoder {
    track: TrackInfo,
    decoder: Box<dyn PacketAudioDecoder>,
}

impl std::fmt::Debug for AudioTrackDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AudioTrackDecoder")
            .field("track", &self.track)
            .finish_non_exhaustive()
    }
}

impl AudioTrackDecoder {
    /// Creates a decoder for one configured audio track.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-audio track, missing layout, unsupported codec,
    /// malformed decoder configuration, or a channel count that exceeds `u16`.
    pub fn new(track: TrackInfo) -> Result<Self, Error> {
        let decoder = create_audio_track_decoder(&track)?;
        Ok(Self { track, decoder })
    }

    /// Returns the presentation-wide identity of the active track.
    #[must_use]
    pub const fn track_id(&self) -> TrackId {
        self.track.id()
    }

    /// Returns the active immutable track configuration.
    #[must_use]
    pub const fn track_info(&self) -> &TrackInfo {
        &self.track
    }

    /// Reconfigures the decoder for an adaptive or period track change.
    ///
    /// # Errors
    ///
    /// Returns an error when the next track configuration is invalid or unsupported.
    pub fn reconfigure(&mut self, track: TrackInfo) -> Result<(), Error> {
        let decoder = create_audio_track_decoder(&track)?;
        self.track = track;
        self.decoder = decoder;
        Ok(())
    }

    /// Submits one audio access unit and returns every available PCM frame.
    ///
    /// # Errors
    ///
    /// Returns an error for the wrong track, an invalid timestamp, or codec failure.
    pub fn decode(&mut self, sample: &EncodedSample) -> Result<Vec<DecodedAudioFrame>, Error> {
        if sample.track_id() != self.track.id() {
            return Err(Error::Codec(format!(
                "audio decoder for track {} received track {}",
                self.track.id().get(),
                sample.track_id().get()
            )));
        }
        if sample.encryption().is_some() {
            return Err(Error::Unsupported(format!(
                "encrypted audio sample for track {} requires a platform CDM decoder",
                self.track.id().get()
            )));
        }
        let mut packet = EncodedAudioPacket::new(
            sample.presentation_time().to_duration()?,
            sample.duration().to_duration()?,
            sample.data().to_vec(),
        );
        if sample.is_discontinuity() {
            packet = packet.with_discontinuity();
        }
        self.decoder
            .decode(packet)
            .map_err(|error| Error::Codec(error.to_string()))
    }

    /// Signals end-of-stream and returns every delayed PCM frame.
    ///
    /// # Errors
    ///
    /// Returns an error when the codec cannot drain its buffered access units.
    pub fn finish(&mut self) -> Result<Vec<DecodedAudioFrame>, Error> {
        self.decoder
            .finish()
            .map_err(|error| Error::Codec(error.to_string()))
    }

    /// Resets codec state before an externally signaled discontinuity.
    pub fn reset(&mut self) {
        self.decoder.reset();
    }
}

fn create_audio_track_decoder(track: &TrackInfo) -> Result<Box<dyn PacketAudioDecoder>, Error> {
    if track.kind() != TrackKind::Audio {
        return Err(Error::Codec(format!(
            "track {} is not an audio track",
            track.id().get()
        )));
    }
    if track.protection().is_some() {
        return Err(Error::Unsupported(format!(
            "protected audio track {} requires a platform CDM decoder",
            track.id().get()
        )));
    }
    let layout = track.audio_layout().ok_or_else(|| {
        Error::Container(format!(
            "audio track {} has no channel or sample-rate layout",
            track.id().get()
        ))
    })?;
    let channels = u16::try_from(layout.channels.get()).map_err(|_| {
        Error::Unsupported(format!(
            "audio track {} declares {} channels, exceeding u16",
            track.id().get(),
            layout.channels
        ))
    })?;
    let channels = std::num::NonZeroU16::new(channels).ok_or_else(|| {
        Error::Container(format!(
            "audio track {} declares zero channels",
            track.id().get()
        ))
    })?;

    match track.codec() {
        ContainerCodec::Aac => AacPacketDecoder::new(AacDecoderConfig::new(
            track.decoder_configuration(),
            channels,
            layout.sample_rate,
        ))
        .map(|decoder| Box::new(decoder) as Box<dyn PacketAudioDecoder>)
        .map_err(|error| Error::Codec(error.to_string())),
        codec => Err(Error::Unsupported(format!(
            "audio track {} uses unsupported codec {codec:?}",
            track.id().get()
        ))),
    }
}

/// One decoded frame with deterministic media timing.
pub struct DecodedVideoFrame {
    frame: DecodedFrame,
    timing: FrameTiming,
    color_info: VideoColorInfo,
    progress: f64,
}

impl DecodedVideoFrame {
    /// Returns the opaque platform-backed decoded frame.
    #[must_use]
    pub const fn frame(&self) -> &DecodedFrame {
        &self.frame
    }

    /// Consumes the wrapper and returns its opaque decoded frame.
    #[must_use]
    pub fn into_frame(self) -> DecodedFrame {
        self.frame
    }

    /// Returns deterministic media timing for this frame.
    #[must_use]
    pub const fn timing(&self) -> FrameTiming {
        self.timing
    }

    /// Returns the color description that applies to this decoded frame.
    #[must_use]
    pub const fn color_info(&self) -> VideoColorInfo {
        self.color_info
    }

    /// Returns normalized progress within a seekable file.
    #[must_use]
    pub const fn progress(&self) -> f64 {
        self.progress
    }

    /// Returns the coded frame width.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.frame.width()
    }

    /// Returns the coded frame height.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.frame.height()
    }
}

impl std::fmt::Debug for DecodedVideoFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DecodedVideoFrame")
            .field("timing", &self.timing)
            .field("color_info", &self.color_info)
            .field("progress", &self.progress)
            .field("width", &self.width())
            .field("height", &self.height())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SampleMetadata {
    pts: u64,
    duration: u32,
    is_keyframe: bool,
}

#[derive(Debug)]
struct PendingDecodedFrame {
    frame: DecodedFrame,
    presentation_time: Duration,
    duration: Duration,
    progress: f64,
}

#[derive(Debug, Clone, Copy)]
struct SubmittedTiming {
    duration: Duration,
    progress: f64,
}

#[derive(Debug, Default)]
struct DecodedFrameQueue {
    pending: VecDeque<PendingDecodedFrame>,
    submitted: BTreeMap<Duration, VecDeque<SubmittedTiming>>,
    sequence: u64,
}

impl DecodedFrameQueue {
    fn submit(&mut self, presentation_time: Duration, duration: Duration, progress: f64) {
        self.submitted
            .entry(presentation_time)
            .or_default()
            .push_back(SubmittedTiming { duration, progress });
    }

    fn receive(
        &mut self,
        decoded: DecodedFrame,
        discard_before: Option<Duration>,
    ) -> Result<(), Error> {
        let presentation_time = decoded.timestamp();
        let timing_queue = self.submitted.get_mut(&presentation_time).ok_or_else(|| {
            Error::Codec(format!(
                "decoder returned PTS {presentation_time:?} that was not attached to a submitted packet"
            ))
        })?;
        let timing = timing_queue.pop_front().ok_or_else(|| {
            Error::Codec(format!(
                "decoder returned more than one frame for submitted PTS {presentation_time:?}"
            ))
        })?;
        if timing_queue.is_empty() {
            self.submitted.remove(&presentation_time);
        }
        if discard_before.is_some_and(|discard_before| presentation_time < discard_before) {
            return Ok(());
        }
        self.pending.push_back(PendingDecodedFrame {
            frame: decoded,
            presentation_time,
            duration: timing.duration,
            progress: timing.progress,
        });
        Ok(())
    }

    fn pop(&mut self, color_info: VideoColorInfo) -> Option<DecodedVideoFrame> {
        let pending = self.pending.pop_front()?;
        let timing = FrameTiming::new(pending.presentation_time, pending.duration, self.sequence);
        self.sequence = self.sequence.saturating_add(1);
        Some(DecodedVideoFrame {
            frame: pending.frame,
            timing,
            color_info,
            progress: pending.progress,
        })
    }

    fn reset(&mut self, sequence: u64) {
        self.pending.clear();
        self.submitted.clear();
        self.sequence = sequence;
    }

    fn reset_timing(&mut self) {
        self.submitted.clear();
    }
}

/// Persistent decoder for presentation-wide encoded video samples.
///
/// The demux/session layer owns buffering and track selection. This type owns
/// codec reordering and translates exact container timing into decoded frames
/// without depending on a network protocol or graphics device.
pub struct VideoTrackDecoder {
    track: TrackInfo,
    codec_type: CodecType,
    decoder: Decoder,
    frames: DecodedFrameQueue,
    color_info: VideoColorInfo,
}

impl std::fmt::Debug for VideoTrackDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VideoTrackDecoder")
            .field("track", &self.track)
            .field("codec_type", &self.codec_type)
            .field("color_info", &self.color_info)
            .finish_non_exhaustive()
    }
}

impl VideoTrackDecoder {
    /// Creates a decoder for one configured video track.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-video track, missing dimensions, unsupported
    /// codec, or platform decoder initialization failure.
    pub fn new(track: TrackInfo) -> Result<Self, Error> {
        let (codec_type, decoder, color_info) = create_track_decoder(&track)?;
        Ok(Self {
            track,
            codec_type,
            decoder,
            frames: DecodedFrameQueue::default(),
            color_info,
        })
    }

    /// Returns the presentation-wide identity of the active track.
    #[must_use]
    pub const fn track_id(&self) -> TrackId {
        self.track.id()
    }

    /// Returns the active immutable track configuration.
    #[must_use]
    pub const fn track_info(&self) -> &TrackInfo {
        &self.track
    }

    /// Reconfigures the decoder for an adaptive or period track change.
    ///
    /// Delayed frames from the previous decoder are returned before the new
    /// configuration becomes active.
    ///
    /// # Errors
    ///
    /// Returns an error when draining or initializing either configuration fails.
    pub fn reconfigure(&mut self, track: TrackInfo) -> Result<Vec<DecodedVideoFrame>, Error> {
        let drained = self.finish()?;
        let (codec_type, decoder, color_info) = create_track_decoder(&track)?;
        self.track = track;
        self.codec_type = codec_type;
        self.decoder = decoder;
        self.color_info = color_info;
        self.frames.reset_timing();
        Ok(drained)
    }

    /// Decodes one access unit and returns every frame currently emitted.
    ///
    /// `progress` is presentation-level progress supplied by the session. A
    /// discontinuity must begin with a random-access sample; the decoder is
    /// drained and rebuilt before that sample is submitted.
    ///
    /// # Errors
    ///
    /// Returns an error for the wrong track, an invalid discontinuity, timing
    /// conversion, or codec failure.
    pub fn decode(
        &mut self,
        sample: &EncodedSample,
        progress: f64,
    ) -> Result<Vec<DecodedVideoFrame>, Error> {
        if sample.track_id() != self.track.id() {
            return Err(Error::Codec(format!(
                "video decoder for track {} received track {}",
                self.track.id().get(),
                sample.track_id().get()
            )));
        }
        if sample.encryption().is_some() {
            return Err(Error::Unsupported(format!(
                "encrypted video sample for track {} requires a protected platform surface",
                self.track.id().get()
            )));
        }
        let mut output = if sample.is_discontinuity() {
            if !sample.is_keyframe() {
                return Err(Error::Container(format!(
                    "video discontinuity on track {} does not start at a keyframe",
                    sample.track_id().get()
                )));
            }
            let drained = self.finish()?;
            let (_, decoder, _) = create_track_decoder(&self.track)?;
            self.decoder = decoder;
            self.frames.reset_timing();
            drained
        } else {
            Vec::new()
        };
        let presentation_time = sample.presentation_time().to_duration()?;
        let duration = sample.duration().to_duration()?;
        self.frames
            .submit(presentation_time, duration, progress.clamp(0.0, 1.0));
        for decoded in self
            .decoder
            .decode(DecodePacket::new(sample.data(), presentation_time))
        {
            self.frames.receive(
                decoded.map_err(|error| Error::Codec(error.to_string()))?,
                None,
            )?;
        }
        while let Some(frame) = self.frames.pop(self.color_info) {
            output.push(frame);
        }
        Ok(output)
    }

    /// Drains delayed frames at end of a representation or presentation.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform decoder fails or emits unmatched timing.
    pub fn finish(&mut self) -> Result<Vec<DecodedVideoFrame>, Error> {
        for decoded in self.decoder.drain() {
            self.frames.receive(
                decoded.map_err(|error| Error::Codec(error.to_string()))?,
                None,
            )?;
        }
        let mut output = Vec::new();
        while let Some(frame) = self.frames.pop(self.color_info) {
            output.push(frame);
        }
        Ok(output)
    }
}

fn create_track_decoder(track: &TrackInfo) -> Result<(CodecType, Decoder, VideoColorInfo), Error> {
    if track.kind() != TrackKind::Video {
        return Err(Error::Codec(format!(
            "track {} is not a video track",
            track.id().get()
        )));
    }
    if track.protection().is_some() {
        return Err(Error::Unsupported(format!(
            "protected video track {} requires a protected platform surface",
            track.id().get()
        )));
    }
    let dimensions = track.video_dimensions().ok_or_else(|| {
        Error::Container(format!(
            "video track {} has no coded dimensions",
            track.id().get()
        ))
    })?;
    let codec_type = match track.codec() {
        ContainerCodec::H264 => CodecType::H264,
        ContainerCodec::H265 => CodecType::H265,
        ContainerCodec::Av1 => CodecType::Av1,
        codec => {
            return Err(Error::Unsupported(format!(
                "video track {} uses unsupported codec {codec:?}",
                track.id().get()
            )));
        }
    };
    let decoder = Decoder::new(
        codec_type,
        Some(track.decoder_configuration()),
        dimensions.width.get(),
        dimensions.height.get(),
    )
    .map_err(|error| Error::Codec(error.to_string()))?;
    Ok((
        codec_type,
        decoder,
        track.video_color_info().unwrap_or_default(),
    ))
}

/// Synchronous demux-and-decode engine for a seekable video file.
///
/// This type owns no graphics device and performs no texture upload. Callers
/// decide whether decoded frames are presented, processed, transcoded, or
/// uploaded through `waterkit-codec`'s GPU adapter.
pub struct VideoPlayer {
    reader: VideoReader,
    decoder: Decoder,
    frames: DecodedFrameQueue,
    width: u32,
    height: u32,
    timescale: u32,
    total_samples: u32,
    duration: Duration,
    codec_type: CodecType,
    codec_config: Option<Vec<u8>>,
    sample_metadata: Vec<SampleMetadata>,
    has_audio: bool,
    color_info: VideoColorInfo,
    drained: bool,
    discard_before: Option<Duration>,
}

impl std::fmt::Debug for VideoPlayer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VideoPlayer")
            .field("dimensions", &self.dimensions())
            .field("codec_type", &self.codec_type)
            .field("sample_count", &self.total_samples)
            .field("duration", &self.duration)
            .finish_non_exhaustive()
    }
}

impl VideoPlayer {
    /// Opens a seekable MP4 or MOV file and initializes its decoder.
    ///
    /// # Errors
    ///
    /// Returns an error when the container cannot be read, no video track is
    /// present, the codec is unsupported, or decoder initialization fails.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let reader = VideoReader::open(path)?;
        let (width, height) = reader.dimensions();
        let color_info = probe_mp4_color_info(path, Some(height))?;
        let codec_config = reader.codec_config().map(<[u8]>::to_vec);
        let codec_type = detect_codec_type(codec_config.as_deref())?;
        let decoder = Decoder::new(codec_type, codec_config.as_deref(), width, height)
            .map_err(|error| Error::Codec(error.to_string()))?;
        let timescale = reader.timescale();
        let total_samples = reader.sample_count();
        let sample_capacity = usize::try_from(total_samples).map_err(|_| {
            Error::Container(format!(
                "video sample count {total_samples} exceeds the current architecture"
            ))
        })?;
        let mut sample_metadata = Vec::with_capacity(sample_capacity);
        for index in 0..sample_capacity {
            let (pts, duration, is_keyframe) = reader.sample_info(index).ok_or_else(|| {
                Error::Container(format!(
                    "missing video sample metadata for zero-based sample {index}"
                ))
            })?;
            sample_metadata.push(SampleMetadata {
                pts,
                duration,
                is_keyframe,
            });
        }
        let duration = estimated_video_duration(&sample_metadata, timescale);
        let has_audio = reader.has_audio();

        Ok(Self {
            reader,
            decoder,
            frames: DecodedFrameQueue::default(),
            width,
            height,
            timescale,
            total_samples,
            duration,
            codec_type,
            codec_config,
            sample_metadata,
            has_audio,
            color_info,
            drained: false,
            discard_before: None,
        })
    }

    /// Returns coded video dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Returns the selected decoder codec.
    #[must_use]
    pub const fn codec_type(&self) -> CodecType {
        self.codec_type
    }

    /// Returns the number of encoded video samples.
    #[must_use]
    pub const fn sample_count(&self) -> u32 {
        self.total_samples
    }

    /// Returns the container time scale.
    #[must_use]
    pub const fn timescale(&self) -> u32 {
        self.timescale
    }

    /// Returns the estimated playable video duration.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.duration
    }

    /// Returns whether the container includes at least one audio track.
    #[must_use]
    pub const fn has_audio(&self) -> bool {
        self.has_audio
    }

    /// Returns the source color description used for decoded frames.
    #[must_use]
    pub const fn color_info(&self) -> VideoColorInfo {
        self.color_info
    }

    /// Decodes the next presentation frame.
    ///
    /// Decoder reordering is retained across calls, so B-frame output is not
    /// discarded when one encoded sample produces multiple decoded frames.
    ///
    /// # Errors
    ///
    /// Returns a codec or container error when decoding cannot continue.
    pub fn next_frame(&mut self) -> Result<Option<DecodedVideoFrame>, Error> {
        if let Some(frame) = self.frames.pop(self.color_info) {
            return Ok(Some(frame));
        }

        loop {
            let sample_index = self.reader.current_index();
            let Some((sample_data, pts, _)) = self.reader.read_sample()? else {
                if !self.drained {
                    self.drained = true;
                    for decoded in self.decoder.drain() {
                        let decoded = decoded.map_err(|error| Error::Codec(error.to_string()))?;
                        self.handle_decoded_frame(decoded)?;
                    }
                    if let Some(frame) = self.frames.pop(self.color_info) {
                        return Ok(Some(frame));
                    }
                }
                return Ok(None);
            };
            let packet = DecodePacket::new(&sample_data, ticks_to_duration(pts, self.timescale));
            let presentation_time = packet.presentation_time();
            let duration = sample_duration(&self.sample_metadata, self.timescale, sample_index);
            let progress = normalized_progress(sample_index, self.total_samples);
            self.frames.submit(presentation_time, duration, progress);

            for decoded in self.decoder.decode(packet) {
                let decoded = decoded.map_err(|error| Error::Codec(error.to_string()))?;
                self.handle_decoded_frame(decoded)?;
            }

            if let Some(frame) = self.frames.pop(self.color_info) {
                return Ok(Some(frame));
            }
        }
    }

    /// Seeks to normalized progress, restarting decode at the closest preceding keyframe.
    ///
    /// # Errors
    ///
    /// Returns an error when decoder reinitialization or preroll fails.
    pub fn seek_to_progress(&mut self, progress: f64) -> Result<Duration, Error> {
        if self.total_samples == 0 {
            return Ok(Duration::ZERO);
        }

        let target_index = target_sample_index(progress, self.total_samples);
        let keyframe_index =
            nearest_keyframe_index_at_or_before(&self.sample_metadata, target_index);

        self.rebuild_decoder()?;
        self.reader.seek_to_sample(keyframe_index);
        self.drained = false;
        let target_time =
            sample_presentation_time(&self.sample_metadata, self.timescale, target_index);
        self.discard_before = Some(target_time);
        let sequence = u64::try_from(target_index).map_err(|_| {
            Error::Container(format!(
                "video sample index {target_index} exceeds a 64-bit sequence number"
            ))
        })?;
        self.frames.reset(sequence);

        while self.reader.current_index() < target_index {
            let sample_index = self.reader.current_index();
            let Some((sample_data, pts, _)) = self.reader.read_sample()? else {
                break;
            };
            let packet = DecodePacket::new(&sample_data, ticks_to_duration(pts, self.timescale));
            self.frames.submit(
                packet.presentation_time(),
                sample_duration(&self.sample_metadata, self.timescale, sample_index),
                normalized_progress(sample_index, self.total_samples),
            );
            for decoded in self.decoder.decode(packet) {
                let decoded = decoded.map_err(|error| Error::Codec(error.to_string()))?;
                self.handle_decoded_frame(decoded)?;
            }
        }

        Ok(target_time)
    }

    /// Creates a demand-driven asynchronous stream over decoded frames.
    pub fn frames(self) -> impl Stream<Item = Result<DecodedVideoFrame, Error>> {
        futures::stream::unfold(Some(self), |player| async move {
            let mut player = player?;
            match player.next_frame() {
                Ok(Some(frame)) => Some((Ok(frame), Some(player))),
                Ok(None) => None,
                Err(error) => Some((Err(error), None)),
            }
        })
    }

    fn rebuild_decoder(&mut self) -> Result<(), Error> {
        self.decoder = Decoder::new(
            self.codec_type,
            self.codec_config.as_deref(),
            self.width,
            self.height,
        )
        .map_err(|error| Error::Codec(error.to_string()))?;
        Ok(())
    }

    fn handle_decoded_frame(&mut self, decoded: DecodedFrame) -> Result<(), Error> {
        self.frames.receive(decoded, self.discard_before)
    }
}

/// Detects the decoder codec from an AVC or HEVC configuration record.
///
/// # Errors
///
/// Returns [`Error::Unsupported`] when no recognized configuration is present.
pub fn detect_codec_type(config: Option<&[u8]>) -> Result<CodecType, Error> {
    let Some(config) = config else {
        return Err(Error::Unsupported(String::from(
            "video track has no codec configuration",
        )));
    };

    if config.len() >= 8 && &config[4..8] == b"avcC" {
        return Ok(CodecType::H264);
    }
    if config.len() >= 7 && config[0] == 0x01 {
        let profile = config[1];
        if matches!(profile, 66 | 77 | 88 | 100 | 110 | 122 | 144) {
            return Ok(CodecType::H264);
        }
    }
    if config.len() >= 8 && &config[4..8] == b"hvcC" {
        return Ok(CodecType::H265);
    }
    if config.len() >= 23 && config[0] == 0x01 {
        let level = config[12];
        if level > 0 && level <= 186 {
            return Ok(CodecType::H265);
        }
    }

    Err(Error::Unsupported(String::from(
        "video codec configuration is neither H.264/AVC nor H.265/HEVC",
    )))
}

fn estimated_video_duration(samples: &[SampleMetadata], timescale: u32) -> Duration {
    let presentation_end = samples.iter().fold(0_u64, |end, sample| {
        end.max(sample.pts.saturating_add(u64::from(sample.duration)))
    });
    ticks_to_duration(presentation_end, timescale)
}

fn nearest_keyframe_index_at_or_before(samples: &[SampleMetadata], target: usize) -> usize {
    samples
        .iter()
        .take(target.saturating_add(1))
        .enumerate()
        .rev()
        .find_map(|(index, sample)| sample.is_keyframe.then_some(index))
        .unwrap_or(0)
}

fn sample_presentation_time(samples: &[SampleMetadata], timescale: u32, index: usize) -> Duration {
    samples.get(index).map_or(Duration::ZERO, |sample| {
        ticks_to_duration(sample.pts, timescale)
    })
}

fn sample_duration(samples: &[SampleMetadata], timescale: u32, index: usize) -> Duration {
    samples.get(index).map_or(Duration::ZERO, |sample| {
        ticks_to_duration(u64::from(sample.duration), timescale)
    })
}

fn ticks_to_duration(ticks: u64, timescale: u32) -> Duration {
    if timescale == 0 {
        return Duration::ZERO;
    }
    Duration::from_nanos(ticks.saturating_mul(1_000_000_000) / u64::from(timescale))
}

fn normalized_progress(sample_index: usize, total_samples: u32) -> f64 {
    if total_samples <= 1 {
        return 0.0;
    }
    let bounded_index = u32::try_from(sample_index)
        .expect("video sample index must fit into u32")
        .min(total_samples - 1);
    f64::from(bounded_index) / f64::from(total_samples - 1)
}

fn target_sample_index(progress: f64, total_samples: u32) -> usize {
    let last = total_samples.saturating_sub(1);
    (progress.clamp(0.0, 1.0) * f64::from(last))
        .round()
        .to_usize()
        .expect("normalized seek target must fit into the current architecture")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        SampleMetadata, estimated_video_duration, nearest_keyframe_index_at_or_before,
        sample_duration, target_sample_index,
    };

    #[test]
    fn duration_includes_the_last_sample_span() {
        let samples = [
            SampleMetadata {
                pts: 0,
                duration: 40,
                is_keyframe: true,
            },
            SampleMetadata {
                pts: 40,
                duration: 40,
                is_keyframe: false,
            },
            SampleMetadata {
                pts: 80,
                duration: 40,
                is_keyframe: false,
            },
        ];
        assert_eq!(
            estimated_video_duration(&samples, 1_000),
            Duration::from_millis(120)
        );
        assert_eq!(
            sample_duration(&samples, 1_000, 2),
            Duration::from_millis(40)
        );
    }

    #[test]
    fn seek_starts_from_the_latest_preceding_keyframe() {
        let samples = [
            SampleMetadata {
                pts: 0,
                duration: 40,
                is_keyframe: true,
            },
            SampleMetadata {
                pts: 40,
                duration: 40,
                is_keyframe: false,
            },
            SampleMetadata {
                pts: 80,
                duration: 40,
                is_keyframe: true,
            },
            SampleMetadata {
                pts: 120,
                duration: 40,
                is_keyframe: false,
            },
        ];
        assert_eq!(nearest_keyframe_index_at_or_before(&samples, 3), 2);
    }

    #[test]
    fn normalized_seek_is_clamped_to_the_sample_range() {
        assert_eq!(target_sample_index(-1.0, 100), 0);
        assert_eq!(target_sample_index(0.5, 100), 50);
        assert_eq!(target_sample_index(2.0, 100), 99);
    }
}
