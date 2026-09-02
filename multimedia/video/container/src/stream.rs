//! Incremental elementary-stream demuxing for segmented playback.

use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU32,
    time::Duration,
};

use broadcast_common::{Parse, Serialize, Unpackage};
use bytes::{Bytes, BytesMut};
use transmux::{
    CodecConfig as TransmuxCodecConfig, DemuxEvent as TransmuxDemuxEvent, Fmp4Demux,
    MovieFragmentBox, ProtectionSchemeInfoBox, ProtectionSystemSpecificHeaderBox,
    SENC_FLAG_USE_SUBSAMPLE_ENCRYPTION, SampleAuxInfoOffsetsBox, SampleAuxInfoSizesBox,
    SampleEncryptionBox, SampleEntryVariant, StblChild, StreamingTsDemux, Track as TransmuxTrack,
    TrackBox, TrackExtendsBox, TrackFragmentBox, TrackFragmentRunBox, TrackSpec, parse_box,
};
use waterkit_video_core::Error;
use waterkit_video_core::VideoColorInfo;

use crate::subtitles::{SubtitleCue, parse_ttml_document};
use crate::{
    CommonEncryptionScheme, EncryptionSubsample, ProtectionInitData, SampleEncryption,
    TrackProtection,
};

const SAMPLE_FLAG_IS_NON_SYNC: u32 = 0x0001_0000;
const TFHD_DEFAULT_BASE_IS_MOOF: u32 = 0x0002_0000;
const MPEG_TIMESTAMP_TIMESCALE: NonZeroU32 = NonZeroU32::new(90_000).unwrap();

/// Stable, non-zero track identity within one presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrackId(NonZeroU32);

impl TrackId {
    /// Creates a track identity.
    ///
    /// # Errors
    ///
    /// Returns a container error when `value` is zero.
    pub fn new(value: u32) -> Result<Self, Error> {
        NonZeroU32::new(value).map(Self).ok_or_else(|| {
            Error::Container(String::from("container track identifiers must be non-zero"))
        })
    }

    /// Returns the integer track identity.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// Exact signed media time with its source time scale retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MediaTime {
    ticks: i64,
    timescale: NonZeroU32,
}

impl MediaTime {
    /// Creates an exact media time.
    #[must_use]
    pub const fn new(ticks: i64, timescale: NonZeroU32) -> Self {
        Self { ticks, timescale }
    }

    /// Returns the signed time-scale ticks.
    #[must_use]
    pub const fn ticks(self) -> i64 {
        self.ticks
    }

    /// Returns the number of ticks per second.
    #[must_use]
    pub const fn timescale(self) -> NonZeroU32 {
        self.timescale
    }

    /// Converts a non-negative media time to a duration.
    ///
    /// # Errors
    ///
    /// Returns a container error for a negative timestamp or an overflowing conversion.
    pub fn to_duration(self) -> Result<Duration, Error> {
        let ticks = u64::try_from(self.ticks).map_err(|_| {
            Error::Container(format!(
                "negative media time {} cannot be represented as a duration",
                self.ticks
            ))
        })?;
        let nanos = u128::from(ticks)
            .checked_mul(1_000_000_000)
            .ok_or_else(|| Error::Container(String::from("media-time conversion overflow")))?
            / u128::from(self.timescale.get());
        let nanos = u64::try_from(nanos)
            .map_err(|_| Error::Container(String::from("media-time duration exceeds u64")))?;
        Ok(Duration::from_nanos(nanos))
    }

    fn shift(self, positive: Duration, negative: Duration) -> Result<Self, Error> {
        let positive = i128::try_from(positive.as_nanos()).map_err(|_| {
            Error::Container(String::from("positive media-time shift exceeds i128"))
        })?;
        let negative = i128::try_from(negative.as_nanos()).map_err(|_| {
            Error::Container(String::from("negative media-time shift exceeds i128"))
        })?;
        let scaled_nanos = positive
            .checked_sub(negative)
            .and_then(|delta| delta.checked_mul(i128::from(self.timescale.get())))
            .ok_or_else(|| Error::Container(String::from("media-time shift overflow")))?;
        if scaled_nanos % 1_000_000_000 != 0 {
            return Err(Error::Container(format!(
                "media-time shift is not exactly representable at timescale {}",
                self.timescale
            )));
        }
        let delta = i64::try_from(scaled_nanos / 1_000_000_000)
            .map_err(|_| Error::Container(String::from("media-time shift exceeds i64 ticks")))?;
        let ticks = self
            .ticks
            .checked_add(delta)
            .ok_or_else(|| Error::Container(String::from("shifted media time exceeds i64")))?;
        Ok(Self::new(ticks, self.timescale))
    }
}

/// Semantic category of one elementary track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackKind {
    /// Coded video samples.
    Video,
    /// Coded audio samples.
    Audio,
    /// Timed text or bitmap subtitles.
    Subtitle,
    /// Timed metadata or private data.
    Metadata,
}

/// Codec carried by one elementary track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Codec {
    /// H.264 / AVC.
    H264,
    /// H.265 / HEVC.
    H265,
    /// H.266 / VVC.
    H266,
    /// AV1.
    Av1,
    /// VP9.
    Vp9,
    /// VP8.
    Vp8,
    /// MPEG-2 / H.262 video.
    Mpeg2Video,
    /// Advanced Audio Coding.
    Aac,
    /// Dolby Digital.
    Ac3,
    /// Dolby Digital Plus.
    Eac3,
    /// Dolby AC-4.
    Ac4,
    /// Opus.
    Opus,
    /// FLAC.
    Flac,
    /// MPEG-H 3D Audio.
    MpegH,
    /// MPEG audio Layer I, II, or III.
    MpegAudio,
    /// DTS audio.
    Dts,
    /// Vorbis.
    Vorbis,
    /// TTML or IMSC timed text.
    Ttml,
    /// ISO BMFF `WebVTT` timed text.
    WebVtt,
    /// `ID3v2` timed metadata carried by MPEG-TS stream type `0x15`.
    Id3,
    /// An opaque data elementary stream.
    Data,
}

/// Coded video dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VideoDimensions {
    /// Coded width in pixels.
    pub width: NonZeroU32,
    /// Coded height in pixels.
    pub height: NonZeroU32,
}

/// Audio stream layout required to configure a decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioLayout {
    /// Channel count.
    pub channels: NonZeroU32,
    /// Samples per second.
    pub sample_rate: NonZeroU32,
    /// Coded sample size in bits when the container declares one.
    pub sample_size: u16,
}

/// Immutable decoder parameters for one track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInfo {
    id: TrackId,
    kind: TrackKind,
    timescale: NonZeroU32,
    codec: Codec,
    decoder_configuration: Bytes,
    video_dimensions: Option<VideoDimensions>,
    video_color_info: Option<VideoColorInfo>,
    audio_layout: Option<AudioLayout>,
    source_pid: Option<u16>,
    protection: Option<TrackProtection>,
}

impl TrackInfo {
    /// Rebinds this descriptor to a presentation-wide track identity.
    ///
    /// Segmented formats such as DASH may place independently numbered tracks
    /// in separate initialization resources. The session layer uses this method
    /// to make those local identifiers unique across the complete presentation.
    #[must_use]
    pub const fn with_id(mut self, id: TrackId) -> Self {
        self.id = id;
        self
    }

    /// Returns the track identity.
    #[must_use]
    pub const fn id(&self) -> TrackId {
        self.id
    }

    /// Returns the semantic track category.
    #[must_use]
    pub const fn kind(&self) -> TrackKind {
        self.kind
    }

    /// Returns the track time scale.
    #[must_use]
    pub const fn timescale(&self) -> NonZeroU32 {
        self.timescale
    }

    /// Returns the coded format.
    #[must_use]
    pub const fn codec(&self) -> Codec {
        self.codec
    }

    /// Returns decoder initialization bytes in the codec's canonical form.
    ///
    /// Video codecs retain their ISOBMFF configuration box. AAC returns the
    /// raw `AudioSpecificConfig` carried by `esds`.
    #[must_use]
    pub fn decoder_configuration(&self) -> &[u8] {
        &self.decoder_configuration
    }

    /// Returns coded video dimensions when this is a video track.
    #[must_use]
    pub const fn video_dimensions(&self) -> Option<VideoDimensions> {
        self.video_dimensions
    }

    /// Returns color and HDR signaling for a video track when the container declares it.
    #[must_use]
    pub const fn video_color_info(&self) -> Option<VideoColorInfo> {
        self.video_color_info
    }

    /// Returns the audio layout when this is an audio track.
    #[must_use]
    pub const fn audio_layout(&self) -> Option<AudioLayout> {
        self.audio_layout
    }

    /// Returns the MPEG-TS packet identifier when the track came from TS.
    #[must_use]
    pub const fn source_pid(&self) -> Option<u16> {
        self.source_pid
    }

    /// Returns Common Encryption defaults when this track is protected.
    #[must_use]
    pub const fn protection(&self) -> Option<&TrackProtection> {
        self.protection.as_ref()
    }
}

/// One decode-order coded access unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedSample {
    track_id: TrackId,
    decode_time: MediaTime,
    presentation_time: MediaTime,
    duration: MediaTime,
    keyframe: bool,
    discontinuity: bool,
    data: Bytes,
    encryption: Option<SampleEncryption>,
}

impl EncodedSample {
    /// Creates one encoded sample with exact decode and presentation timing.
    #[must_use]
    pub const fn new(
        track_id: TrackId,
        decode_time: MediaTime,
        presentation_time: MediaTime,
        duration: MediaTime,
        keyframe: bool,
        data: Bytes,
    ) -> Self {
        Self {
            track_id,
            decode_time,
            presentation_time,
            duration,
            keyframe,
            discontinuity: false,
            data,
            encryption: None,
        }
    }

    /// Rebinds this sample to a presentation-wide track identity.
    #[must_use]
    pub const fn with_track_id(mut self, track_id: TrackId) -> Self {
        self.track_id = track_id;
        self
    }

    /// Maps local media timestamps into a containing presentation timeline.
    ///
    /// `positive` is normally a period start and `negative` is the DASH
    /// presentation-time offset. Both must be exactly representable in the
    /// sample's time scale.
    ///
    /// # Errors
    ///
    /// Returns a container error when the shift is fractional or overflows.
    pub fn shift_timestamps(
        mut self,
        positive: Duration,
        negative: Duration,
    ) -> Result<Self, Error> {
        self.decode_time = self.decode_time.shift(positive, negative)?;
        self.presentation_time = self.presentation_time.shift(positive, negative)?;
        Ok(self)
    }

    /// Sets whether this sample starts a discontinuous timeline region.
    #[must_use]
    pub const fn with_discontinuity(mut self, discontinuity: bool) -> Self {
        self.discontinuity = discontinuity;
        self
    }

    /// Attaches per-sample Common Encryption metadata.
    #[must_use]
    pub fn with_encryption(mut self, encryption: SampleEncryption) -> Self {
        self.encryption = Some(encryption);
        self
    }

    /// Returns the owning track.
    #[must_use]
    pub const fn track_id(&self) -> TrackId {
        self.track_id
    }

    /// Returns the exact decode timestamp.
    #[must_use]
    pub const fn decode_time(&self) -> MediaTime {
        self.decode_time
    }

    /// Returns the exact presentation timestamp.
    #[must_use]
    pub const fn presentation_time(&self) -> MediaTime {
        self.presentation_time
    }

    /// Returns the exact coded-sample duration.
    #[must_use]
    pub const fn duration(&self) -> MediaTime {
        self.duration
    }

    /// Returns whether the sample is a random-access point.
    #[must_use]
    pub const fn is_keyframe(&self) -> bool {
        self.keyframe
    }

    /// Returns whether this sample starts a discontinuous timeline region.
    #[must_use]
    pub const fn is_discontinuity(&self) -> bool {
        self.discontinuity
    }

    /// Returns the coded access-unit bytes.
    #[must_use]
    pub const fn data(&self) -> &Bytes {
        &self.data
    }

    /// Returns Common Encryption metadata when this access unit is protected.
    #[must_use]
    pub const fn encryption(&self) -> Option<&SampleEncryption> {
        self.encryption.as_ref()
    }
}

/// One ISO BMFF event message mapped onto the presentation timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedMetadata {
    scheme_id_uri: String,
    value: String,
    id: u32,
    presentation_time: Duration,
    duration: Duration,
    message_data: Bytes,
}

impl TimedMetadata {
    /// Creates one timed metadata event on a presentation timeline.
    #[must_use]
    pub fn new(
        scheme_id_uri: impl Into<String>,
        value: impl Into<String>,
        id: u32,
        presentation_time: Duration,
        duration: Duration,
        message_data: impl Into<Bytes>,
    ) -> Self {
        Self {
            scheme_id_uri: scheme_id_uri.into(),
            value: value.into(),
            id,
            presentation_time,
            duration,
            message_data: message_data.into(),
        }
    }

    /// Returns the metadata scheme identifier carried by `emsg`.
    #[must_use]
    pub fn scheme_id_uri(&self) -> &str {
        &self.scheme_id_uri
    }

    /// Returns the scheme-specific event value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the scheme-local event identifier.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// Returns the event time on the complete presentation timeline.
    #[must_use]
    pub const fn presentation_time(&self) -> Duration {
        self.presentation_time
    }

    /// Returns the event duration declared by the media container.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.duration
    }

    /// Returns the scheme-owned message payload without interpreting it.
    #[must_use]
    pub const fn message_data(&self) -> &Bytes {
        &self.message_data
    }

    /// Maps a representation-local event onto a containing presentation timeline.
    ///
    /// # Errors
    ///
    /// Returns a container error when the mapped time underflows or overflows.
    pub fn shift_timestamp(
        mut self,
        positive: Duration,
        negative: Duration,
    ) -> Result<Self, Error> {
        self.presentation_time = self
            .presentation_time
            .checked_add(positive)
            .and_then(|time| time.checked_sub(negative))
            .ok_or_else(|| Error::Container(String::from("timed-metadata timestamp overflow")))?;
        Ok(self)
    }
}

/// Elementary samples and timed metadata demuxed from one CMAF media unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmafMediaSegment {
    samples: Vec<EncodedSample>,
    timed_metadata: Vec<TimedMetadata>,
}

impl CmafMediaSegment {
    /// Returns coded elementary samples in container emission order.
    #[must_use]
    pub fn samples(&self) -> &[EncodedSample] {
        &self.samples
    }

    /// Returns timed event messages in container order.
    #[must_use]
    pub fn timed_metadata(&self) -> &[TimedMetadata] {
        &self.timed_metadata
    }

    /// Returns whether neither coded samples nor timed metadata were emitted.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.samples.is_empty() && self.timed_metadata.is_empty()
    }

    /// Consumes the media unit into its coded samples and timed metadata.
    #[must_use]
    pub fn into_parts(self) -> (Vec<EncodedSample>, Vec<TimedMetadata>) {
        (self.samples, self.timed_metadata)
    }
}

/// Decodes one CMAF timed-text sample into presentation subtitle cues.
///
/// TTML sample documents may use sample-local or already mapped presentation
/// timing; both forms are resolved against the sample interval. `WebVTT` cue-box
/// samples take their interval directly from the ISO BMFF sample timeline.
///
/// # Errors
///
/// Returns a container error for track mismatch, unsupported subtitle codec,
/// malformed TTML/XML, malformed `WebVTT` cue boxes, or inconsistent timing.
pub fn decode_cmaf_subtitle_sample(
    track: &TrackInfo,
    sample: &EncodedSample,
) -> Result<Vec<SubtitleCue>, Error> {
    if track.kind() != TrackKind::Subtitle {
        return Err(Error::Container(format!(
            "track {} is not a subtitle track",
            track.id().get()
        )));
    }
    if sample.track_id() != track.id() {
        return Err(Error::Container(format!(
            "subtitle sample track {} does not match decoder track {}",
            sample.track_id().get(),
            track.id().get()
        )));
    }
    let start = sample.presentation_time().to_duration()?;
    let duration = sample.duration().to_duration()?;
    if duration.is_zero() {
        return Err(Error::Container(String::from(
            "subtitle sample duration must be non-zero",
        )));
    }
    match track.codec() {
        Codec::Ttml => {
            let document = std::str::from_utf8(sample.data()).map_err(|error| {
                Error::Container(format!("TTML sample is not valid UTF-8: {error}"))
            })?;
            map_ttml_sample_cues(parse_ttml_document(document)?, start, duration)
        }
        Codec::WebVtt => decode_webvtt_sample(sample.data(), start, duration),
        codec => Err(Error::Unsupported(format!(
            "subtitle track {} uses unsupported codec {codec:?}",
            track.id().get()
        ))),
    }
}

fn map_ttml_sample_cues(
    cues: Vec<SubtitleCue>,
    sample_start: Duration,
    sample_duration: Duration,
) -> Result<Vec<SubtitleCue>, Error> {
    let sample_end = sample_start
        .checked_add(sample_duration)
        .ok_or_else(|| Error::Container(String::from("subtitle sample interval overflow")))?;
    if cues
        .iter()
        .all(|cue| cue.start >= sample_start && cue.end <= sample_end)
    {
        return Ok(cues);
    }
    if cues.iter().all(|cue| cue.end <= sample_duration) {
        return cues
            .into_iter()
            .map(|cue| cue.shift_by(sample_start))
            .collect();
    }
    Err(Error::Container(format!(
        "TTML cue timing is neither sample-local nor within presentation interval {sample_start:?}..{sample_end:?}"
    )))
}

fn decode_webvtt_sample(
    data: &[u8],
    start: Duration,
    duration: Duration,
) -> Result<Vec<SubtitleCue>, Error> {
    let end = start
        .checked_add(duration)
        .ok_or_else(|| Error::Container(String::from("WebVTT sample interval overflow")))?;
    let mut cues = Vec::new();
    for item in parse_top_level_boxes(data)? {
        match item.kind {
            kind if kind == *b"vttc" => {
                let cue = transmux::VttCueBox::bare_parse(&data[item.range])
                    .map_err(|error| Error::Container(error.to_string()))?;
                if !cue.payload.cue_text.trim().is_empty() {
                    cues.push(SubtitleCue {
                        start,
                        end,
                        text: cue.payload.cue_text,
                    });
                }
            }
            kind if kind == *b"vtte" => {}
            kind => {
                return Err(Error::Unsupported(format!(
                    "unsupported WebVTT sample box {:?}",
                    String::from_utf8_lossy(&kind)
                )));
            }
        }
    }
    Ok(cues)
}

/// Parsed CMAF initialization state shared by every segment in a representation.
#[derive(Debug, Clone)]
pub struct CmafInitialization {
    tracks: Vec<TrackInfo>,
    defaults: BTreeMap<TrackId, TrackExtendsBox>,
    protection_init_data: Vec<ProtectionInitData>,
}

impl CmafInitialization {
    /// Parses an initialization segment.
    ///
    /// # Errors
    ///
    /// Returns an error when the segment has no supported tracks, contains invalid
    /// track identities or time scales, or has malformed ISOBMFF boxes.
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let (normalized_data, protections) = normalize_protected_sample_entries(data)?;
        let mut demux = Fmp4Demux::new();
        let media = demux
            .unpackage(normalized_data.as_slice())
            .map_err(|error| Error::Container(error.to_string()))?;
        let mut tracks = media
            .tracks
            .iter()
            .map(track_info_from_transmux)
            .collect::<Result<Vec<_>, _>>()?;
        for track in &mut tracks {
            track.protection = protections.get(&track.id()).cloned();
            if let Some(dimensions) = track.video_dimensions {
                track.video_color_info = Some(super::color::probe_mp4_color_info_bytes(
                    data,
                    Some(dimensions.height.get()),
                )?);
            }
        }
        let movie = parse_movie_box(data)?;
        for track in &movie.tracks {
            let id = TrackId::new(track.tkhd.track_id)?;
            if tracks.iter().any(|known| known.id() == id) {
                continue;
            }
            if let Some(track) = subtitle_track_info(track)? {
                tracks.push(track);
            }
        }
        tracks.sort_by_key(TrackInfo::id);
        if tracks.is_empty() {
            return Err(Error::Unsupported(String::from(
                "CMAF initialization segment contains no supported elementary tracks",
            )));
        }
        let defaults = movie
            .mvex
            .ok_or_else(|| Error::Container(String::from("CMAF initialization has no mvex")))?
            .trex
            .into_iter()
            .map(|defaults| Ok((TrackId::new(defaults.track_id)?, defaults)))
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        for track in &tracks {
            if !defaults.contains_key(&track.id()) {
                return Err(Error::Container(format!(
                    "CMAF initialization has no trex defaults for track {}",
                    track.id().get()
                )));
            }
        }

        for protected_track in protections.keys() {
            if !tracks.iter().any(|track| track.id() == *protected_track) {
                return Err(Error::Unsupported(format!(
                    "protected CMAF track {} uses an unsupported codec",
                    protected_track.get()
                )));
            }
        }

        Ok(Self {
            tracks,
            defaults,
            protection_init_data: parse_protection_init_data(data)?,
        })
    }

    /// Returns every supported elementary track.
    #[must_use]
    pub fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }

    /// Returns DRM initialization payloads declared by ISO BMFF `pssh` boxes.
    #[must_use]
    pub fn protection_init_data(&self) -> &[ProtectionInitData] {
        &self.protection_init_data
    }
}

fn normalize_protected_sample_entries(
    data: &[u8],
) -> Result<(Vec<u8>, BTreeMap<TrackId, TrackProtection>), Error> {
    let top_level = parse_top_level_boxes(data)?;
    let movie = top_level
        .iter()
        .find(|item| item.kind == *b"moov")
        .ok_or_else(|| Error::Container(String::from("initialization segment has no moov")))?;
    let mut normalized = data.to_vec();
    let mut protections = BTreeMap::new();

    for track_box in child_boxes(data, movie)?
        .into_iter()
        .filter(|item| item.kind == *b"trak")
    {
        let Some(entry) = parse_protected_track_entry(data, &track_box)? else {
            continue;
        };
        if protections
            .insert(entry.track_id, entry.protection)
            .is_some()
        {
            return Err(Error::Container(format!(
                "duplicate protected track identifier {}",
                entry.track_id.get()
            )));
        }
        normalized[entry.four_cc_range].copy_from_slice(&entry.original_format);
    }

    Ok((normalized, protections))
}

struct ProtectedTrackEntry {
    track_id: TrackId,
    four_cc_range: std::ops::Range<usize>,
    original_format: [u8; 4],
    protection: TrackProtection,
}

fn parse_protected_track_entry(
    data: &[u8],
    track_box: &TopLevelBox,
) -> Result<Option<ProtectedTrackEntry>, Error> {
    let track = TrackBox::parse(&data[track_box.range.clone()])
        .map_err(|error| Error::Container(error.to_string()))?;
    let track_id = TrackId::new(track.tkhd.track_id)?;
    let media = required_child_box(data, track_box, *b"mdia", "trak has no mdia")?;
    let information = required_child_box(data, &media, *b"minf", "mdia has no minf")?;
    let table = required_child_box(data, &information, *b"stbl", "minf has no stbl")?;
    let description = required_child_box(data, &table, *b"stsd", "stbl has no stsd")?;
    let protected_entries = sample_description_entries(data, &description)?
        .into_iter()
        .filter(|entry| matches!(&entry.kind, b"encv" | b"enca"))
        .collect::<Vec<_>>();
    if protected_entries.len() > 1 {
        return Err(Error::Unsupported(format!(
            "protected CMAF track {} declares multiple sample descriptions",
            track_id.get()
        )));
    }
    let Some(entry) = protected_entries.into_iter().next() else {
        return Ok(None);
    };
    let fixed_sample_entry_size = if entry.kind == *b"encv" { 78 } else { 28 };
    let children_start = entry
        .body_range
        .start
        .checked_add(fixed_sample_entry_size)
        .ok_or_else(|| Error::Container(String::from("sample-entry range overflow")))?;
    if children_start > entry.body_range.end {
        return Err(Error::Container(format!(
            "protected sample entry for track {} is shorter than its fixed header",
            track_id.get()
        )));
    }
    let entry_children =
        parse_box_sequence(&data[children_start..entry.body_range.end], children_start)?;
    let scheme_information = entry_children
        .iter()
        .find(|child| child.kind == *b"sinf")
        .ok_or_else(|| {
            Error::Container(format!(
                "protected sample entry for track {} has no sinf",
                track_id.get()
            ))
        })?;
    let scheme_information =
        ProtectionSchemeInfoBox::parse(&data[scheme_information.range.clone()])
            .map_err(|error| Error::Container(error.to_string()))?;
    let protection = parse_track_protection(&scheme_information, track_id)?;
    Ok(Some(ProtectedTrackEntry {
        track_id,
        four_cc_range: entry.range.start + 4..entry.range.start + 8,
        original_format: scheme_information.original_format.data_format,
        protection,
    }))
}

fn parse_track_protection(
    scheme_information: &ProtectionSchemeInfoBox,
    track_id: TrackId,
) -> Result<TrackProtection, Error> {
    let scheme_type = scheme_information
        .scheme_type
        .as_ref()
        .ok_or_else(|| Error::Container(String::from("CENC sinf has no schm box")))?
        .scheme_type;
    let scheme = match &scheme_type {
        b"cenc" => CommonEncryptionScheme::Cenc,
        b"cbcs" => CommonEncryptionScheme::Cbcs,
        other => {
            return Err(Error::Unsupported(format!(
                "unsupported Common Encryption scheme {:?}",
                String::from_utf8_lossy(other)
            )));
        }
    };
    let encryption = scheme_information
        .scheme_info
        .as_ref()
        .and_then(|information| information.tenc.as_ref())
        .ok_or_else(|| Error::Container(String::from("CENC sinf has no tenc box")))?;
    if encryption.default_is_protected != 1 {
        return Err(Error::Container(format!(
            "protected sample entry for track {} has default_is_protected {}",
            track_id.get(),
            encryption.default_is_protected
        )));
    }
    TrackProtection::new(
        scheme,
        encryption.default_kid,
        encryption.default_per_sample_iv_size,
        encryption.default_constant_iv.clone(),
        encryption.default_crypt_byte_block,
        encryption.default_skip_byte_block,
    )
}

fn parse_protection_init_data(data: &[u8]) -> Result<Vec<ProtectionInitData>, Error> {
    let top_level = parse_top_level_boxes(data)?;
    let movie_children = top_level
        .iter()
        .find(|item| item.kind == *b"moov")
        .map(|movie| child_boxes(data, movie))
        .transpose()?
        .unwrap_or_default();
    top_level
        .iter()
        .filter(|item| item.kind == *b"pssh")
        .chain(movie_children.iter().filter(|item| item.kind == *b"pssh"))
        .map(|item| parse_pssh_init_data(&data[item.range.start..item.range.end]))
        .collect()
}

/// Parses one complete ISO BMFF `pssh` box into platform-CDM initialization data.
///
/// # Errors
///
/// Returns a container error for a malformed box, system identifier, key list, or payload.
pub fn parse_pssh_init_data(init_data: &[u8]) -> Result<ProtectionInitData, Error> {
    let parsed = ProtectionSystemSpecificHeaderBox::parse_box(init_data)
        .map_err(|error| Error::Container(error.to_string()))?;
    Ok(ProtectionInitData::new(
        parsed.system_id,
        parsed.kids,
        init_data.to_vec(),
        parsed.data,
    ))
}

fn sample_description_entries(
    data: &[u8],
    description: &TopLevelBox,
) -> Result<Vec<TopLevelBox>, Error> {
    if description.body_range.len() < 8 {
        return Err(Error::Container(String::from(
            "stsd is shorter than its full-box header and entry count",
        )));
    }
    let count_offset = description.body_range.start + 4;
    let entry_count = u32::from_be_bytes(
        data[count_offset..count_offset + 4]
            .try_into()
            .expect("validated stsd entry count contains four bytes"),
    ) as usize;
    let entries_start = count_offset + 4;
    let entries = parse_box_sequence(
        &data[entries_start..description.body_range.end],
        entries_start,
    )?;
    if entries.len() != entry_count {
        return Err(Error::Container(format!(
            "stsd declares {entry_count} entries but contains {}",
            entries.len()
        )));
    }
    Ok(entries)
}

fn child_boxes(data: &[u8], parent: &TopLevelBox) -> Result<Vec<TopLevelBox>, Error> {
    parse_box_sequence(&data[parent.body_range.clone()], parent.body_range.start)
}

fn required_child_box(
    data: &[u8],
    parent: &TopLevelBox,
    kind: [u8; 4],
    message: &str,
) -> Result<TopLevelBox, Error> {
    child_boxes(data, parent)?
        .into_iter()
        .find(|item| item.kind == kind)
        .ok_or_else(|| Error::Container(String::from(message)))
}

fn subtitle_track_info(track: &TrackBox) -> Result<Option<TrackInfo>, Error> {
    let Some(media) = track.mdia.as_ref() else {
        return Ok(None);
    };
    let Some(header) = media.mdhd.as_ref() else {
        return Ok(None);
    };
    let Some(information) = media.minf.as_ref() else {
        return Ok(None);
    };
    let Some(table) = information.stbl.as_ref() else {
        return Ok(None);
    };
    let Some(description) = table.children.iter().find_map(|child| match child {
        StblChild::Stsd(description) => Some(description),
        _ => None,
    }) else {
        return Ok(None);
    };
    let Some(entry) = description.entries.first() else {
        return Ok(None);
    };
    let (codec, decoder_configuration) = match entry {
        SampleEntryVariant::Stpp(entry) => (Codec::Ttml, serialize_subtitle_entry(entry.as_ref())?),
        SampleEntryVariant::Wvtt(entry) => {
            (Codec::WebVtt, serialize_subtitle_entry(entry.as_ref())?)
        }
        _ => return Ok(None),
    };
    let timescale = NonZeroU32::new(header.timescale).ok_or_else(|| {
        Error::Container(format!(
            "subtitle track {} has a zero timescale",
            track.tkhd.track_id
        ))
    })?;
    Ok(Some(TrackInfo {
        id: TrackId::new(track.tkhd.track_id)?,
        kind: TrackKind::Subtitle,
        timescale,
        codec,
        decoder_configuration: decoder_configuration.into(),
        video_dimensions: None,
        video_color_info: None,
        audio_layout: None,
        source_pid: None,
        protection: None,
    }))
}

fn serialize_subtitle_entry<T>(entry: &T) -> Result<Vec<u8>, Error>
where
    T: Serialize,
    T::Error: core::fmt::Display,
{
    let mut bytes = vec![0_u8; entry.serialized_len()];
    entry.serialize_into(&mut bytes).map_err(|error| {
        Error::Container(format!("failed to serialize subtitle entry: {error}"))
    })?;
    Ok(bytes)
}

/// Stateful CMAF/fMP4 media-segment demuxer.
#[derive(Debug)]
pub struct CmafDemuxer {
    initialization: CmafInitialization,
    next_decode_time: BTreeMap<TrackId, u64>,
}

impl CmafDemuxer {
    /// Creates a demuxer for media segments sharing `initialization`.
    #[must_use]
    pub const fn new(initialization: CmafInitialization) -> Self {
        Self {
            initialization,
            next_decode_time: BTreeMap::new(),
        }
    }

    /// Returns the current representation's tracks.
    #[must_use]
    pub fn tracks(&self) -> &[TrackInfo] {
        self.initialization.tracks()
    }

    /// Demuxes one independently fetched CMAF media segment.
    ///
    /// The returned payloads are zero-copy slices of `segment`. `discontinuity`
    /// is attached to the first sample of every track in this segment.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed fragment boxes, unresolved sample defaults,
    /// unknown tracks, invalid byte ranges, or overflowing media timestamps.
    pub fn demux_segment(
        &mut self,
        segment: &Bytes,
        discontinuity: bool,
    ) -> Result<CmafMediaSegment, Error> {
        let boxes = parse_top_level_boxes(segment)?;
        let media_ranges = boxes
            .iter()
            .filter(|item| item.kind == *b"mdat")
            .map(|item| item.body_range.clone())
            .collect::<Vec<_>>();
        if media_ranges.is_empty() {
            return Err(Error::Container(String::from(
                "CMAF media segment contains no mdat payload",
            )));
        }

        let mut samples = Vec::new();
        let mut discontinuity_tracks = BTreeSet::new();
        let mut previous_traf_data_end = None;

        for item in boxes.iter().filter(|item| item.kind == *b"moof") {
            let moof = MovieFragmentBox::parse_body(&segment[item.body_range.clone()])
                .map_err(|error| Error::Container(error.to_string()))?;
            let fragment_encryption = self.parse_fragment_encryption(segment, item, &moof)?;
            let moof_offset = u64::try_from(item.range.start).map_err(|_| {
                Error::Container(String::from(
                    "CMAF moof offset exceeds the current architecture",
                ))
            })?;

            for (traf, encryption) in moof.traf.iter().zip(&fragment_encryption) {
                let track_id = TrackId::new(traf.tfhd.track_id)?;
                let starts_discontinuity = discontinuity && discontinuity_tracks.insert(track_id);
                let (mut track_samples, data_end) = self.demux_track_fragment(
                    segment,
                    &media_ranges,
                    traf,
                    CmafFragmentLocation {
                        moof_offset,
                        previous_track_data_end: previous_traf_data_end,
                        starts_discontinuity,
                    },
                    encryption.as_deref(),
                )?;
                samples.append(&mut track_samples);
                previous_traf_data_end = data_end.or(previous_traf_data_end);
            }
        }
        if samples.is_empty() {
            return Err(Error::Container(String::from(
                "CMAF media segment contains no coded samples",
            )));
        }
        let mut presentation_times = samples.iter();
        let first_presentation_time = presentation_times
            .next()
            .ok_or_else(|| Error::Container(String::from("CMAF sample set became empty")))?
            .presentation_time()
            .to_duration()?;
        let earliest_presentation_time =
            presentation_times.try_fold(first_presentation_time, |earliest, sample| {
                Ok::<_, Error>(earliest.min(sample.presentation_time().to_duration()?))
            })?;
        let timed_metadata = parse_timed_metadata(segment, &boxes, earliest_presentation_time)?;
        Ok(CmafMediaSegment {
            samples,
            timed_metadata,
        })
    }

    fn demux_track_fragment(
        &mut self,
        segment: &Bytes,
        media_ranges: &[std::ops::Range<usize>],
        fragment: &TrackFragmentBox,
        location: CmafFragmentLocation,
        encryption: Option<&[SampleEncryption]>,
    ) -> Result<(Vec<EncodedSample>, Option<u64>), Error> {
        let track_id = TrackId::new(fragment.tfhd.track_id)?;
        let track = self
            .initialization
            .tracks
            .iter()
            .find(|track| track.id() == track_id)
            .ok_or_else(|| {
                Error::Container(format!(
                    "CMAF fragment references undeclared track {}",
                    track_id.get()
                ))
            })?;
        let defaults = self.initialization.defaults.get(&track_id).ok_or_else(|| {
            Error::Container(format!(
                "CMAF fragment has no defaults for track {}",
                track_id.get()
            ))
        })?;
        let mut decode_time = fragment
            .tfdt
            .as_ref()
            .map(transmux::TrackFragmentBaseMediaDecodeTimeBox::base_media_decode_time)
            .or_else(|| self.next_decode_time.get(&track_id).copied())
            .ok_or_else(|| {
                Error::Container(format!(
                    "first CMAF fragment for track {} has no tfdt",
                    track_id.get()
                ))
            })?;
        let base_data_offset = fragment.tfhd.base_data_offset.unwrap_or_else(|| {
            if fragment.tfhd.flags & TFHD_DEFAULT_BASE_IS_MOOF != 0 {
                location.moof_offset
            } else {
                location
                    .previous_track_data_end
                    .unwrap_or(location.moof_offset)
            }
        });
        let sample_defaults = CmafSampleDefaults {
            duration: fragment
                .tfhd
                .default_sample_duration
                .unwrap_or(defaults.default_sample_duration),
            size: fragment
                .tfhd
                .default_sample_size
                .unwrap_or(defaults.default_sample_size),
            flags: fragment
                .tfhd
                .default_sample_flags
                .unwrap_or(defaults.default_sample_flags),
        };
        let context = CmafRunContext {
            segment,
            media_ranges,
            track_id,
            timescale: track.timescale(),
            defaults: sample_defaults,
        };
        let mut samples = Vec::new();
        let mut run_data_end = None;
        let mut next_sample_starts_discontinuity = location.starts_discontinuity;
        let expected_sample_count = fragment
            .trun
            .iter()
            .map(|run| run.samples.len())
            .sum::<usize>();
        if encryption.is_some_and(|entries| entries.len() != expected_sample_count) {
            return Err(Error::Container(format!(
                "CMAF track {} has {expected_sample_count} samples but its senc describes {}",
                track_id.get(),
                encryption.map_or(0, <[SampleEncryption]>::len)
            )));
        }
        let mut encryption_offset = 0_usize;
        for run in &fragment.trun {
            let data_offset = match run.data_offset {
                Some(relative) => checked_add_signed(base_data_offset, relative)?,
                None => run_data_end.unwrap_or(base_data_offset),
            };
            let decoded = context.decode_run(
                run,
                data_offset,
                decode_time,
                &mut next_sample_starts_discontinuity,
                encryption.map(|entries| {
                    &entries[encryption_offset..encryption_offset + run.samples.len()]
                }),
            )?;
            encryption_offset += run.samples.len();
            run_data_end = Some(decoded.data_end);
            decode_time = decoded.next_decode_time;
            samples.extend(decoded.samples);
        }
        self.next_decode_time.insert(track_id, decode_time);
        Ok((samples, run_data_end))
    }

    fn parse_fragment_encryption(
        &self,
        segment: &Bytes,
        item: &TopLevelBox,
        fragment: &MovieFragmentBox,
    ) -> Result<Vec<Option<Vec<SampleEncryption>>>, Error> {
        let raw_tracks = child_boxes(segment, item)?
            .into_iter()
            .filter(|child| child.kind == *b"traf")
            .collect::<Vec<_>>();
        if raw_tracks.len() != fragment.traf.len() {
            return Err(Error::Container(format!(
                "CMAF moof parser found {} typed traf boxes but {} raw traf boxes",
                fragment.traf.len(),
                raw_tracks.len()
            )));
        }
        fragment
            .traf
            .iter()
            .zip(raw_tracks)
            .map(|(track_fragment, raw_track)| {
                let track_id = TrackId::new(track_fragment.tfhd.track_id)?;
                let track = self
                    .initialization
                    .tracks
                    .iter()
                    .find(|track| track.id() == track_id)
                    .ok_or_else(|| {
                        Error::Container(format!(
                            "CMAF fragment references undeclared track {}",
                            track_id.get()
                        ))
                    })?;
                parse_track_fragment_encryption(segment, &raw_track, track)
            })
            .collect()
    }
}

fn parse_track_fragment_encryption(
    segment: &[u8],
    track_fragment: &TopLevelBox,
    track: &TrackInfo,
) -> Result<Option<Vec<SampleEncryption>>, Error> {
    let children = child_boxes(segment, track_fragment)?;
    let sample_encryption = children.iter().find(|child| child.kind == *b"senc");
    let Some(protection) = track.protection() else {
        if sample_encryption.is_some() {
            return Err(Error::Container(format!(
                "clear CMAF track {} carries a senc box without tenc defaults",
                track.id().get()
            )));
        }
        return Ok(None);
    };
    let sample_encryption = sample_encryption.ok_or_else(|| {
        Error::Container(format!(
            "protected CMAF track {} fragment has no senc box",
            track.id().get()
        ))
    })?;
    let body = &segment[sample_encryption.body_range.clone()];
    if body.len() < 4 {
        return Err(Error::Container(String::from(
            "senc is shorter than its full-box header",
        )));
    }
    let version = body[0];
    let flags = u32::from_be_bytes([0, body[1], body[2], body[3]]);
    if flags & !SENC_FLAG_USE_SUBSAMPLE_ENCRYPTION != 0 {
        return Err(Error::Unsupported(format!(
            "CENC senc flags {flags:#08x} require track-encryption overrides not supported by this container version"
        )));
    }
    let parsed = SampleEncryptionBox::parse_body(
        &body[4..],
        version,
        flags,
        protection.per_sample_iv_size(),
    )
    .map_err(|error| Error::Container(error.to_string()))?;
    let expected_iv_size = usize::from(protection.per_sample_iv_size());
    let entries = parsed
        .entries
        .into_iter()
        .map(|entry| {
            if entry.initialization_vector.len() != expected_iv_size {
                return Err(Error::Container(format!(
                    "CENC sample IV has {} bytes but track {} declares {expected_iv_size}",
                    entry.initialization_vector.len(),
                    track.id().get()
                )));
            }
            Ok(SampleEncryption::new(
                entry.initialization_vector,
                entry
                    .subsamples
                    .into_iter()
                    .map(|subsample| {
                        EncryptionSubsample::new(
                            subsample.bytes_of_clear_data,
                            subsample.bytes_of_protected_data,
                        )
                    })
                    .collect(),
            ))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    validate_sample_auxiliary_information(segment, &children, &entries, flags)?;
    Ok(Some(entries))
}

fn validate_sample_auxiliary_information(
    segment: &[u8],
    boxes: &[TopLevelBox],
    entries: &[SampleEncryption],
    senc_flags: u32,
) -> Result<(), Error> {
    if let Some(sizes_box) = boxes.iter().find(|item| item.kind == *b"saiz") {
        let bytes = &segment[sizes_box.range.clone()];
        let sizes = SampleAuxInfoSizesBox::parse_box(bytes)
            .map_err(|error| Error::Container(error.to_string()))?;
        let sample_count = sample_auxiliary_information_count(bytes, sizes.flags)?;
        if sample_count != entries.len() {
            return Err(Error::Container(format!(
                "saiz describes {sample_count} samples but senc describes {}",
                entries.len()
            )));
        }
        let use_subsamples = senc_flags & SENC_FLAG_USE_SUBSAMPLE_ENCRYPTION != 0;
        let expected_sizes = entries
            .iter()
            .map(|entry| {
                let size = entry.initialization_vector().len()
                    + usize::from(use_subsamples) * 2
                    + entry.subsamples().len() * 6;
                u8::try_from(size).map_err(|_| {
                    Error::Container(String::from("CENC sample auxiliary data exceeds 255 bytes"))
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        if sizes.default_sample_info_size == 0 {
            if sizes.sample_info_sizes != expected_sizes {
                return Err(Error::Container(String::from(
                    "saiz sample sizes do not match senc auxiliary data",
                )));
            }
        } else if expected_sizes
            .iter()
            .any(|size| *size != sizes.default_sample_info_size)
        {
            return Err(Error::Container(String::from(
                "saiz default sample size does not match senc auxiliary data",
            )));
        }
    }
    if let Some(offsets_box) = boxes.iter().find(|item| item.kind == *b"saio") {
        let offsets = SampleAuxInfoOffsetsBox::parse_box(&segment[offsets_box.range.clone()])
            .map_err(|error| Error::Container(error.to_string()))?;
        if offsets.offsets.len() != 1 {
            return Err(Error::Unsupported(format!(
                "CENC saio declares {} auxiliary ranges; exactly one inline senc range is required",
                offsets.offsets.len()
            )));
        }
    }
    Ok(())
}

fn sample_auxiliary_information_count(bytes: &[u8], flags: u32) -> Result<usize, Error> {
    let optional_fields = usize::from(flags & 1 != 0) * 8;
    let count_offset = 12_usize
        .checked_add(optional_fields)
        .and_then(|offset| offset.checked_add(1))
        .ok_or_else(|| Error::Container(String::from("saiz count offset overflow")))?;
    let count_end = count_offset
        .checked_add(4)
        .ok_or_else(|| Error::Container(String::from("saiz count range overflow")))?;
    let count = bytes
        .get(count_offset..count_end)
        .ok_or_else(|| Error::Container(String::from("saiz has no complete sample count field")))?;
    Ok(u32::from_be_bytes(
        count
            .try_into()
            .expect("validated saiz count contains four bytes"),
    ) as usize)
}

fn validate_sample_encryption(
    encryption: &SampleEncryption,
    sample_size: u32,
    track_id: TrackId,
) -> Result<(), Error> {
    if encryption.subsamples().is_empty() {
        return Ok(());
    }
    let described_size = encryption
        .subsamples()
        .iter()
        .try_fold(0_u64, |total, subsample| {
            total
                .checked_add(u64::from(subsample.clear_bytes()))
                .and_then(|total| total.checked_add(u64::from(subsample.encrypted_bytes())))
                .ok_or_else(|| Error::Container(String::from("CENC subsample size overflow")))
        })?;
    if described_size != u64::from(sample_size) {
        return Err(Error::Container(format!(
            "CENC subsamples describe {described_size} bytes for track {} sample with {sample_size} bytes",
            track_id.get()
        )));
    }
    Ok(())
}

fn parse_timed_metadata(
    segment: &Bytes,
    boxes: &[TopLevelBox],
    earliest_presentation_time: Duration,
) -> Result<Vec<TimedMetadata>, Error> {
    boxes
        .iter()
        .filter(|item| item.kind == *b"emsg")
        .map(|item| parse_event_message(segment, item, earliest_presentation_time))
        .collect()
}

fn parse_event_message(
    segment: &Bytes,
    item: &TopLevelBox,
    earliest_presentation_time: Duration,
) -> Result<TimedMetadata, Error> {
    let body = &segment[item.body_range.clone()];
    let mut cursor = EventMessageCursor::new(body);
    let version = cursor.u8("emsg version")?;
    let flags = cursor.take(3, "emsg flags")?;
    if flags != [0, 0, 0] {
        return Err(Error::Unsupported(format!(
            "emsg version {version} uses unsupported flags {flags:02x?}"
        )));
    }
    let (scheme_id_uri, value, timescale, presentation_time, event_duration, id) = match version {
        0 => {
            let scheme_id_uri = cursor.utf8_z("emsg scheme_id_uri")?.to_owned();
            let value = cursor.utf8_z("emsg value")?.to_owned();
            let timescale = cursor.non_zero_u32("emsg timescale")?;
            let presentation_time_delta = cursor.u32("emsg presentation_time_delta")?;
            let event_duration = cursor.u32("emsg event_duration")?;
            let id = cursor.u32("emsg id")?;
            let presentation_time = earliest_presentation_time
                .checked_add(duration_from_ticks(
                    u64::from(presentation_time_delta),
                    timescale,
                    "emsg presentation_time_delta",
                )?)
                .ok_or_else(|| {
                    Error::Container(String::from("emsg presentation timestamp overflow"))
                })?;
            (
                scheme_id_uri,
                value,
                timescale,
                presentation_time,
                event_duration,
                id,
            )
        }
        1 => {
            let timescale = cursor.non_zero_u32("emsg timescale")?;
            let presentation_time = cursor.u64("emsg presentation_time")?;
            let event_duration = cursor.u32("emsg event_duration")?;
            let id = cursor.u32("emsg id")?;
            let scheme_id_uri = cursor.utf8_z("emsg scheme_id_uri")?.to_owned();
            let value = cursor.utf8_z("emsg value")?.to_owned();
            (
                scheme_id_uri,
                value,
                timescale,
                duration_from_ticks(presentation_time, timescale, "emsg presentation_time")?,
                event_duration,
                id,
            )
        }
        other => {
            return Err(Error::Unsupported(format!(
                "unsupported emsg version {other}"
            )));
        }
    };
    let message_start = item
        .body_range
        .start
        .checked_add(cursor.position())
        .ok_or_else(|| Error::Container(String::from("emsg payload range overflow")))?;
    Ok(TimedMetadata::new(
        scheme_id_uri,
        value,
        id,
        presentation_time,
        duration_from_ticks(u64::from(event_duration), timescale, "emsg event_duration")?,
        segment.slice(message_start..item.body_range.end),
    ))
}

fn duration_from_ticks(ticks: u64, timescale: NonZeroU32, field: &str) -> Result<Duration, Error> {
    let nanos = u128::from(ticks)
        .checked_mul(1_000_000_000)
        .ok_or_else(|| Error::Container(format!("{field} conversion overflow")))?
        / u128::from(timescale.get());
    let nanos = u64::try_from(nanos)
        .map_err(|_| Error::Container(format!("{field} exceeds duration range")))?;
    Ok(Duration::from_nanos(nanos))
}

struct EventMessageCursor<'a> {
    body: &'a [u8],
    position: usize,
}

impl<'a> EventMessageCursor<'a> {
    const fn new(body: &'a [u8]) -> Self {
        Self { body, position: 0 }
    }

    const fn position(&self) -> usize {
        self.position
    }

    fn take(&mut self, length: usize, field: &str) -> Result<&'a [u8], Error> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.body.len())
            .ok_or_else(|| Error::Container(format!("truncated {field}")))?;
        let value = &self.body[self.position..end];
        self.position = end;
        Ok(value)
    }

    fn u8(&mut self, field: &str) -> Result<u8, Error> {
        Ok(self.take(1, field)?[0])
    }

    fn u32(&mut self, field: &str) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(
            self.take(4, field)?
                .try_into()
                .expect("a four-byte cursor result forms a u32"),
        ))
    }

    fn non_zero_u32(&mut self, field: &str) -> Result<NonZeroU32, Error> {
        NonZeroU32::new(self.u32(field)?)
            .ok_or_else(|| Error::Container(format!("{field} must be non-zero")))
    }

    fn u64(&mut self, field: &str) -> Result<u64, Error> {
        Ok(u64::from_be_bytes(
            self.take(8, field)?
                .try_into()
                .expect("an eight-byte cursor result forms a u64"),
        ))
    }

    fn utf8_z(&mut self, field: &str) -> Result<&'a str, Error> {
        let remaining = &self.body[self.position..];
        let length = remaining
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| Error::Container(format!("{field} is not null terminated")))?;
        let value = self.take(length + 1, field)?;
        std::str::from_utf8(&value[..length])
            .map_err(|error| Error::Container(format!("{field} is not UTF-8: {error}")))
    }
}

/// Incremental demuxer for a chunked CMAF HTTP response.
///
/// Complete `moof`/`mdat` pairs are emitted as soon as their bytes arrive;
/// the full media segment never needs to be buffered before decode begins.
#[derive(Debug)]
pub struct CmafChunkDemuxer {
    demuxer: CmafDemuxer,
    pending: BytesMut,
    discontinuity: bool,
}

impl CmafChunkDemuxer {
    /// Creates an incremental demuxer sharing one CMAF initialization section.
    #[must_use]
    pub fn new(initialization: CmafInitialization) -> Self {
        Self::from_demuxer(CmafDemuxer::new(initialization))
    }

    /// Continues incremental demuxing from an existing representation state.
    #[must_use]
    pub fn from_demuxer(demuxer: CmafDemuxer) -> Self {
        Self {
            demuxer,
            pending: BytesMut::new(),
            discontinuity: false,
        }
    }

    /// Returns the current representation's tracks.
    #[must_use]
    pub fn tracks(&self) -> &[TrackInfo] {
        self.demuxer.tracks()
    }

    /// Appends response bytes and emits every newly completed CMAF chunk.
    ///
    /// `discontinuity` is attached to the first sample of each track emitted
    /// after the flag is observed, even when a network chunk divides box headers.
    ///
    /// # Errors
    ///
    /// Returns a container error for malformed ISO BMFF box sizes or invalid
    /// completed CMAF chunks.
    pub fn feed(&mut self, data: &[u8], discontinuity: bool) -> Result<CmafMediaSegment, Error> {
        self.discontinuity |= discontinuity;
        self.pending.extend_from_slice(data);
        let mut samples = Vec::new();
        let mut timed_metadata = Vec::new();
        while let Some(chunk_length) = complete_cmaf_chunk_length(&self.pending)? {
            let chunk = self.pending.split_to(chunk_length).freeze();
            let demuxed = self
                .demuxer
                .demux_segment(&chunk, std::mem::take(&mut self.discontinuity))?;
            let (mut chunk_samples, mut chunk_metadata) = demuxed.into_parts();
            samples.append(&mut chunk_samples);
            timed_metadata.append(&mut chunk_metadata);
        }
        Ok(CmafMediaSegment {
            samples,
            timed_metadata,
        })
    }

    /// Completes one chunked media response.
    ///
    /// # Errors
    ///
    /// Returns a container error when the response ends inside a box or before
    /// a `moof` receives its matching `mdat`.
    pub fn finish(self) -> Result<CmafDemuxer, Error> {
        if !self.pending.is_empty() {
            return Err(Error::Container(format!(
                "chunked CMAF response ended with {} incomplete bytes",
                self.pending.len()
            )));
        }
        Ok(self.demuxer)
    }
}

fn complete_cmaf_chunk_length(data: &[u8]) -> Result<Option<usize>, Error> {
    let mut offset = 0_usize;
    let mut saw_movie_fragment = false;
    while offset < data.len() {
        let remaining = &data[offset..];
        if remaining.len() < 8 {
            return Ok(None);
        }
        let size32 = u32::from_be_bytes(
            remaining[..4]
                .try_into()
                .expect("validated box header contains four size bytes"),
        );
        let kind: [u8; 4] = remaining[4..8]
            .try_into()
            .expect("validated box header contains four type bytes");
        let (header_length, box_length) = match size32 {
            0 => {
                return Err(Error::Unsupported(String::from(
                    "chunked CMAF requires explicit top-level box sizes",
                )));
            }
            1 => {
                if remaining.len() < 16 {
                    return Ok(None);
                }
                let extended = u64::from_be_bytes(
                    remaining[8..16]
                        .try_into()
                        .expect("validated extended header contains eight size bytes"),
                );
                (
                    16_usize,
                    usize::try_from(extended).map_err(|_| {
                        Error::Container(String::from(
                            "CMAF extended box size exceeds the current architecture",
                        ))
                    })?,
                )
            }
            size => (8_usize, size as usize),
        };
        if box_length < header_length {
            return Err(Error::Container(format!(
                "CMAF top-level box {:?} is smaller than its header",
                String::from_utf8_lossy(&kind)
            )));
        }
        if remaining.len() < box_length {
            return Ok(None);
        }
        offset = offset.checked_add(box_length).ok_or_else(|| {
            Error::Container(String::from("CMAF chunk byte range overflowed usize"))
        })?;
        if kind == *b"moof" {
            saw_movie_fragment = true;
        } else if kind == *b"mdat" && saw_movie_fragment {
            return Ok(Some(offset));
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, Copy)]
struct CmafSampleDefaults {
    duration: u32,
    size: u32,
    flags: u32,
}

#[derive(Debug, Clone, Copy)]
struct CmafFragmentLocation {
    moof_offset: u64,
    previous_track_data_end: Option<u64>,
    starts_discontinuity: bool,
}

struct CmafRunContext<'a> {
    segment: &'a Bytes,
    media_ranges: &'a [std::ops::Range<usize>],
    track_id: TrackId,
    timescale: NonZeroU32,
    defaults: CmafSampleDefaults,
}

struct DecodedCmafRun {
    samples: Vec<EncodedSample>,
    data_end: u64,
    next_decode_time: u64,
}

impl CmafRunContext<'_> {
    fn decode_run(
        &self,
        run: &TrackFragmentRunBox,
        mut data_offset: u64,
        mut decode_time: u64,
        next_sample_starts_discontinuity: &mut bool,
        encryption: Option<&[SampleEncryption]>,
    ) -> Result<DecodedCmafRun, Error> {
        let mut samples = Vec::with_capacity(run.samples.len());
        for (sample_index, sample) in run.samples.iter().enumerate() {
            let duration = sample.sample_duration.unwrap_or(self.defaults.duration);
            if duration == 0 {
                return Err(Error::Container(format!(
                    "CMAF sample in track {} has no duration",
                    self.track_id.get()
                )));
            }
            let size = sample.sample_size.unwrap_or(self.defaults.size);
            let flags = sample
                .sample_flags
                .or_else(|| {
                    (sample_index == 0)
                        .then_some(run.first_sample_flags)
                        .flatten()
                })
                .unwrap_or(self.defaults.flags);
            let data_end = data_offset
                .checked_add(u64::from(size))
                .ok_or_else(|| Error::Container(String::from("CMAF sample byte range overflow")))?;
            validate_media_range(data_offset..data_end, self.media_ranges)?;
            let data_range = usize_range(data_offset..data_end, "CMAF sample")?;
            let decode_ticks = i64::try_from(decode_time)
                .map_err(|_| Error::Container(String::from("CMAF decode time exceeds i64")))?;
            let presentation_ticks = decode_ticks
                .checked_add(i64::from(
                    sample.sample_composition_time_offset.unwrap_or(0),
                ))
                .ok_or_else(|| {
                    Error::Container(String::from("CMAF presentation timestamp overflow"))
                })?;
            let encryption = encryption.map(|entries| entries[sample_index].clone());
            if let Some(encryption) = encryption.as_ref() {
                validate_sample_encryption(encryption, size, self.track_id)?;
            }
            samples.push(EncodedSample {
                track_id: self.track_id,
                decode_time: MediaTime::new(decode_ticks, self.timescale),
                presentation_time: MediaTime::new(presentation_ticks, self.timescale),
                duration: MediaTime::new(i64::from(duration), self.timescale),
                keyframe: flags & SAMPLE_FLAG_IS_NON_SYNC == 0,
                discontinuity: std::mem::take(next_sample_starts_discontinuity),
                data: self.segment.slice(data_range),
                encryption,
            });
            data_offset = data_end;
            decode_time = decode_time
                .checked_add(u64::from(duration))
                .ok_or_else(|| Error::Container(String::from("CMAF decode timeline overflow")))?;
        }
        Ok(DecodedCmafRun {
            samples,
            data_end: data_offset,
            next_decode_time: decode_time,
        })
    }
}

/// Incremental MPEG-2 TS event emitted by [`MpegTsDemuxer`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MpegTsEvent {
    /// A fully configured elementary track became available.
    Track(TrackInfo),
    /// A coded sample became available.
    Sample(EncodedSample),
    /// Every track currently declared by the program map is configured.
    TracksResolved,
}

/// Incremental MPEG-2 Transport Stream demuxer.
pub struct MpegTsDemuxer {
    inner: StreamingTsDemux,
    tracks: BTreeMap<TrackId, TrackInfo>,
    next_decode_time: BTreeMap<TrackId, i64>,
    discontinuous_pids: BTreeSet<u16>,
}

impl std::fmt::Debug for MpegTsDemuxer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MpegTsDemuxer")
            .field("tracks", &self.tracks)
            .field("next_decode_time", &self.next_decode_time)
            .field("discontinuous_pids", &self.discontinuous_pids)
            .finish_non_exhaustive()
    }
}

impl Default for MpegTsDemuxer {
    fn default() -> Self {
        Self::new()
    }
}

impl MpegTsDemuxer {
    /// Creates an empty incremental TS demuxer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: StreamingTsDemux::new(),
            tracks: BTreeMap::new(),
            next_decode_time: BTreeMap::new(),
            discontinuous_pids: BTreeSet::new(),
        }
    }

    /// Feeds arbitrarily aligned TS bytes and returns every newly available event.
    ///
    /// # Errors
    ///
    /// Returns an error when a discovered track or sample cannot be represented by
    /// the `WaterKit` container contract.
    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<MpegTsEvent>, Error> {
        self.inner.feed(data);
        self.drain_events()
    }

    /// Flushes the end of the transport stream and returns trailing events.
    ///
    /// # Errors
    ///
    /// Returns an error when a trailing sample cannot be represented.
    pub fn finish(&mut self) -> Result<Vec<MpegTsEvent>, Error> {
        self.inner.finish();
        self.drain_events()
    }

    fn drain_events(&mut self) -> Result<Vec<MpegTsEvent>, Error> {
        let mut events = Vec::new();
        while let Some(event) = self.inner.poll_event() {
            match event {
                TransmuxDemuxEvent::TrackAdded(track) | TransmuxDemuxEvent::TrackUpdated(track) => {
                    let track = track_info_from_transmux(&track)?;
                    self.tracks.insert(track.id(), track.clone());
                    events.push(MpegTsEvent::Track(track));
                }
                TransmuxDemuxEvent::Sample { track_id, sample } => {
                    let track_id = TrackId::new(track_id)?;
                    let track = self.tracks.get(&track_id).ok_or_else(|| {
                        Error::Container(format!(
                            "MPEG-TS emitted a sample before track {} was configured",
                            track_id.get()
                        ))
                    })?;
                    let (decode_time, presentation_time) =
                        if let Some(timing) = sample.source_timing {
                            (
                                MediaTime::new(
                                    u64_to_i64(timing.dts, "MPEG-TS DTS")?,
                                    MPEG_TIMESTAMP_TIMESCALE,
                                ),
                                MediaTime::new(
                                    u64_to_i64(timing.pts, "MPEG-TS PTS")?,
                                    MPEG_TIMESTAMP_TIMESCALE,
                                ),
                            )
                        } else {
                            let ticks = self.next_decode_time.get(&track_id).copied().unwrap_or(0);
                            let presentation = ticks
                                .checked_add(i64::from(sample.composition_offset))
                                .ok_or_else(|| {
                                    Error::Container(String::from("MPEG-TS PTS overflow"))
                                })?;
                            (
                                MediaTime::new(ticks, track.timescale()),
                                MediaTime::new(presentation, track.timescale()),
                            )
                        };
                    let next = if let Some(timing) = sample.source_timing {
                        u64_to_i64(timing.dts, "MPEG-TS DTS")?
                    } else {
                        decode_time.ticks()
                    }
                    .checked_add(i64::from(sample.duration))
                    .ok_or_else(|| Error::Container(String::from("MPEG-TS timeline overflow")))?;
                    self.next_decode_time.insert(track_id, next);
                    let discontinuity = track
                        .source_pid()
                        .is_some_and(|pid| self.discontinuous_pids.remove(&pid));
                    events.push(MpegTsEvent::Sample(EncodedSample {
                        track_id,
                        decode_time,
                        presentation_time,
                        duration: MediaTime::new(i64::from(sample.duration), track.timescale()),
                        keyframe: sample.is_sync,
                        discontinuity,
                        data: Bytes::from(sample.data),
                        encryption: None,
                    }));
                }
                TransmuxDemuxEvent::Discontinuity { pid } => {
                    self.discontinuous_pids.insert(pid);
                }
                TransmuxDemuxEvent::TracksResolved => {
                    events.push(MpegTsEvent::TracksResolved);
                }
                TransmuxDemuxEvent::Pcr(_) => {}
                _ => {
                    return Err(Error::Unsupported(String::from(
                        "transmux emitted an unknown MPEG-TS event",
                    )));
                }
            }
        }
        Ok(events)
    }
}

#[derive(Debug, Clone)]
struct TopLevelBox {
    kind: [u8; 4],
    range: std::ops::Range<usize>,
    body_range: std::ops::Range<usize>,
}

fn parse_top_level_boxes(data: &[u8]) -> Result<Vec<TopLevelBox>, Error> {
    parse_box_sequence(data, 0)
}

fn parse_box_sequence(data: &[u8], base_offset: usize) -> Result<Vec<TopLevelBox>, Error> {
    let mut boxes = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        let (item, consumed) =
            parse_box(&data[offset..]).map_err(|error| Error::Container(error.to_string()))?;
        if consumed == 0 {
            return Err(Error::Container(String::from(
                "ISOBMFF parser made no progress",
            )));
        }
        let local_end = offset.checked_add(consumed).ok_or_else(|| {
            Error::Container(String::from("ISOBMFF top-level box range overflow"))
        })?;
        let start = base_offset
            .checked_add(offset)
            .ok_or_else(|| Error::Container(String::from("ISOBMFF box start range overflow")))?;
        let end = base_offset
            .checked_add(local_end)
            .ok_or_else(|| Error::Container(String::from("ISOBMFF box end range overflow")))?;
        let body_start = end
            .checked_sub(item.body.len())
            .ok_or_else(|| Error::Container(String::from("ISOBMFF body range underflow")))?;
        boxes.push(TopLevelBox {
            kind: item.header.box_type.0,
            range: start..end,
            body_range: body_start..end,
        });
        offset = local_end;
    }
    Ok(boxes)
}

fn parse_movie_box(data: &[u8]) -> Result<transmux::MovieBox, Error> {
    let boxes = parse_top_level_boxes(data)?;
    let movie = boxes
        .iter()
        .find(|item| item.kind == *b"moov")
        .ok_or_else(|| Error::Container(String::from("initialization segment has no moov")))?;
    transmux::MovieBox::parse(&data[movie.range.clone()])
        .map_err(|error| Error::Container(error.to_string()))
}

fn track_info_from_transmux(track: &TransmuxTrack) -> Result<TrackInfo, Error> {
    track_info_from_spec(&track.spec)
}

pub fn track_info_from_spec(spec: &TrackSpec) -> Result<TrackInfo, Error> {
    let id = TrackId::new(spec.track_id)?;
    let timescale = NonZeroU32::new(spec.timescale)
        .ok_or_else(|| Error::Container(format!("track {} declares a zero timescale", id.get())))?;
    let (kind, codec, decoder_configuration, video_dimensions, audio_layout) =
        normalize_codec(&spec.config)?;
    Ok(TrackInfo {
        id,
        kind,
        timescale,
        codec,
        decoder_configuration,
        video_dimensions,
        video_color_info: video_dimensions
            .map(|dimensions| super::color::inferred_sdr_color_info(Some(dimensions.height.get()))),
        audio_layout,
        source_pid: spec.source_pid,
        protection: None,
    })
}

type NormalizedCodec = (
    TrackKind,
    Codec,
    Bytes,
    Option<VideoDimensions>,
    Option<AudioLayout>,
);

#[allow(clippy::too_many_lines)]
fn normalize_codec(config: &TransmuxCodecConfig) -> Result<NormalizedCodec, Error> {
    let video = |codec, bytes, width: u16, height: u16| {
        Ok((
            TrackKind::Video,
            codec,
            Bytes::from(bytes),
            Some(VideoDimensions {
                width: non_zero_dimension(width, "width")?,
                height: non_zero_dimension(height, "height")?,
            }),
            None,
        ))
    };
    let audio = |codec, bytes, channels: u16, sample_rate: u32, sample_size: u16| {
        Ok((
            TrackKind::Audio,
            codec,
            Bytes::from(bytes),
            None,
            Some(AudioLayout {
                channels: NonZeroU32::new(u32::from(channels)).ok_or_else(|| {
                    Error::Container(String::from("audio track declares zero channels"))
                })?,
                sample_rate: NonZeroU32::new(sample_rate).ok_or_else(|| {
                    Error::Container(String::from("audio track declares a zero sample rate"))
                })?,
                sample_size,
            }),
        ))
    };

    match config {
        TransmuxCodecConfig::Avc {
            config,
            width,
            height,
        } => video(Codec::H264, serialize_box(config)?, *width, *height),
        TransmuxCodecConfig::Hevc {
            config,
            width,
            height,
        } => video(Codec::H265, serialize_box(config)?, *width, *height),
        TransmuxCodecConfig::Vvc {
            config,
            width,
            height,
        } => video(Codec::H266, serialize_box(config)?, *width, *height),
        TransmuxCodecConfig::Av1 {
            config,
            width,
            height,
        } => video(Codec::Av1, serialize_box(config)?, *width, *height),
        TransmuxCodecConfig::Vp9 {
            config,
            width,
            height,
        } => video(Codec::Vp9, serialize_box(config)?, *width, *height),
        TransmuxCodecConfig::Vp8 { width, height } => {
            video(Codec::Vp8, Vec::new(), *width, *height)
        }
        TransmuxCodecConfig::Mpeg2Video {
            esds,
            width,
            height,
        } => video(Codec::Mpeg2Video, serialize_box(esds)?, *width, *height),
        TransmuxCodecConfig::Aac {
            esds,
            channel_count,
            sample_rate,
            sample_size,
        } => audio(
            Codec::Aac,
            esds.es_descriptor
                .decoder_config
                .as_ref()
                .and_then(|config| config.decoder_specific_info.as_ref())
                .map(|info| info.data.clone())
                .ok_or_else(|| {
                    Error::Container(String::from(
                        "AAC esds has no AudioSpecificConfig decoder data",
                    ))
                })?,
            *channel_count,
            *sample_rate,
            *sample_size,
        ),
        TransmuxCodecConfig::Ac3 {
            config,
            channel_count,
            sample_rate,
            sample_size,
        } => audio(
            Codec::Ac3,
            serialize_box(config)?,
            *channel_count,
            *sample_rate,
            *sample_size,
        ),
        TransmuxCodecConfig::Eac3 {
            config,
            channel_count,
            sample_rate,
            sample_size,
        } => audio(
            Codec::Eac3,
            serialize_box(config)?,
            *channel_count,
            *sample_rate,
            *sample_size,
        ),
        TransmuxCodecConfig::Ac4 {
            config,
            channel_count,
            sample_rate,
            sample_size,
        } => audio(
            Codec::Ac4,
            serialize_box(config)?,
            *channel_count,
            *sample_rate,
            *sample_size,
        ),
        TransmuxCodecConfig::Opus {
            config,
            channel_count,
            sample_rate,
            sample_size,
        } => audio(
            Codec::Opus,
            serialize_box(config)?,
            *channel_count,
            *sample_rate,
            *sample_size,
        ),
        TransmuxCodecConfig::Flac {
            config,
            channel_count,
            sample_rate,
            sample_size,
        } => audio(
            Codec::Flac,
            serialize_box(config)?,
            *channel_count,
            *sample_rate,
            *sample_size,
        ),
        TransmuxCodecConfig::MpegH {
            config,
            channel_count,
            sample_rate,
            sample_size,
        } => audio(
            Codec::MpegH,
            serialize_box(config)?,
            *channel_count,
            *sample_rate,
            *sample_size,
        ),
        TransmuxCodecConfig::MpegAudio {
            esds,
            channel_count,
            sample_rate,
            sample_size,
            ..
        } => audio(
            Codec::MpegAudio,
            serialize_box(esds)?,
            *channel_count,
            *sample_rate,
            *sample_size,
        ),
        TransmuxCodecConfig::Dts {
            config,
            channel_count,
            sample_rate,
            sample_size,
            ..
        } => audio(
            Codec::Dts,
            serialize_box(config)?,
            *channel_count,
            *sample_rate,
            *sample_size,
        ),
        TransmuxCodecConfig::Vorbis {
            codec_private,
            channels,
            sample_rate,
        } => audio(
            Codec::Vorbis,
            codec_private.clone(),
            *channels,
            *sample_rate,
            0,
        ),
        TransmuxCodecConfig::Data { stream_type, .. } => Ok((
            TrackKind::Metadata,
            mpeg_ts_data_codec(*stream_type),
            Bytes::new(),
            None,
            None,
        )),
        _ => Err(Error::Unsupported(String::from(
            "container emitted a codec unknown to this WaterKit version",
        ))),
    }
}

const fn mpeg_ts_data_codec(stream_type: u8) -> Codec {
    if stream_type == 0x15 {
        Codec::Id3
    } else {
        Codec::Data
    }
}

fn serialize_box<T>(value: &T) -> Result<Vec<u8>, Error>
where
    T: Serialize<Error = transmux::Error>,
{
    let mut bytes = vec![0; value.serialized_len()];
    let written = value
        .serialize_into(&mut bytes)
        .map_err(|error| Error::Container(error.to_string()))?;
    if written != bytes.len() {
        return Err(Error::Container(format!(
            "codec configuration declared {} bytes but wrote {written}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

fn non_zero_dimension(value: u16, name: &str) -> Result<NonZeroU32, Error> {
    NonZeroU32::new(u32::from(value))
        .ok_or_else(|| Error::Container(format!("video track declares a zero coded {name}")))
}

fn checked_add_signed(base: u64, relative: i32) -> Result<u64, Error> {
    base.checked_add_signed(i64::from(relative)).ok_or_else(|| {
        Error::Container(format!(
            "CMAF data offset {relative} is invalid relative to base {base}"
        ))
    })
}

fn validate_media_range(
    sample: std::ops::Range<u64>,
    media_ranges: &[std::ops::Range<usize>],
) -> Result<(), Error> {
    let valid = media_ranges.iter().any(|media| {
        u64::try_from(media.start).is_ok_and(|start| start <= sample.start)
            && u64::try_from(media.end).is_ok_and(|end| sample.end <= end)
    });
    if valid {
        Ok(())
    } else {
        Err(Error::Container(format!(
            "CMAF sample byte range {}..{} is outside every mdat payload",
            sample.start, sample.end
        )))
    }
}

fn usize_range(
    range: std::ops::Range<u64>,
    description: &str,
) -> Result<std::ops::Range<usize>, Error> {
    let start = usize::try_from(range.start).map_err(|_| {
        Error::Container(format!(
            "{description} offset exceeds the current architecture"
        ))
    })?;
    let end = usize::try_from(range.end).map_err(|_| {
        Error::Container(format!(
            "{description} end exceeds the current architecture"
        ))
    })?;
    Ok(start..end)
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, Error> {
    i64::try_from(value)
        .map_err(|_| Error::Container(format!("{field} exceeds signed media-time range")))
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU32, time::Duration};

    use broadcast_common::{Parse, Serialize};
    use bytes::Bytes;
    use transmux::{
        AVCConfigurationBox, AVCDecoderConfigurationRecord, AvcPps, AvcSps, CencScheme,
        CodecConfig, DecoderConfigDescriptor, DecoderSpecificInfo, ESDescriptor, EsdsBox,
        FragmentTrackData, ObjectTypeIndication, SLConfigDescriptor, Sample, SampleEncryptionEntry,
        StreamType, SubSampleEntry, TrackEncryption, TrackEncryptionBox, TrackSpec,
        build_init_segment, build_media_segment,
        init_segment::protect_init_segment,
        movie_fragment::{FragmentProtection, protect_media_segment},
    };

    use super::{
        CmafChunkDemuxer, CmafDemuxer, CmafInitialization, Codec, EncodedSample, MediaTime,
        TrackId, TrackInfo, TrackKind, decode_cmaf_subtitle_sample, mpeg_ts_data_codec,
    };

    fn subtitle_track(codec: Codec) -> TrackInfo {
        TrackInfo {
            id: TrackId::new(3).expect("test track identifier is non-zero"),
            kind: TrackKind::Subtitle,
            timescale: NonZeroU32::new(1_000).expect("test timescale is non-zero"),
            codec,
            decoder_configuration: Bytes::new(),
            video_dimensions: None,
            video_color_info: None,
            audio_layout: None,
            source_pid: None,
            protection: None,
        }
    }

    fn subtitle_sample(data: Vec<u8>) -> EncodedSample {
        let timescale = NonZeroU32::new(1_000).expect("test timescale is non-zero");
        EncodedSample::new(
            TrackId::new(3).expect("test track identifier is non-zero"),
            MediaTime::new(5_000, timescale),
            MediaTime::new(5_000, timescale),
            MediaTime::new(2_000, timescale),
            true,
            Bytes::from(data),
        )
    }

    fn video_track() -> TrackSpec {
        let record = AVCDecoderConfigurationRecord {
            configuration_version: 1,
            profile_indication: 66,
            profile_compatibility: 0,
            level_indication: 30,
            length_size_minus_one: 3,
            sps: vec![AvcSps(vec![0x67, 0x42, 0x00, 0x1e, 0xe9, 0x01, 0x40])],
            pps: vec![AvcPps(vec![0x68, 0xce, 0x06, 0xe2])],
            chroma_format: None,
            bit_depth_luma_minus8: None,
            bit_depth_chroma_minus8: None,
            sps_ext: Vec::new(),
        };
        TrackSpec::new(
            1,
            90_000,
            CodecConfig::Avc {
                config: AVCConfigurationBox::new(record),
                width: 1_920,
                height: 1_080,
            },
        )
    }

    fn audio_track() -> TrackSpec {
        TrackSpec::new(
            2,
            44_100,
            CodecConfig::Aac {
                esds: EsdsBox {
                    es_descriptor: ESDescriptor {
                        es_id: 2,
                        stream_dependence_flag: false,
                        url_flag: false,
                        ocr_stream_flag: false,
                        stream_priority: 0,
                        depends_on_es_id: None,
                        url: None,
                        ocr_es_id: None,
                        decoder_config: Some(DecoderConfigDescriptor {
                            object_type_indication: ObjectTypeIndication(0x40),
                            stream_type: StreamType(5),
                            up_stream: false,
                            buffer_size_db: 0,
                            max_bitrate: 96_000,
                            avg_bitrate: 96_000,
                            decoder_specific_info: Some(DecoderSpecificInfo {
                                data: vec![0x12, 0x10],
                            }),
                        }),
                        sl_config: Some(SLConfigDescriptor { body: vec![0x02] }),
                    },
                },
                channel_count: 2,
                sample_rate: 44_100,
                sample_size: 16,
            },
        )
    }

    fn event_message_box(version: u8, fields: &[u8], message_data: &[u8]) -> Vec<u8> {
        let size = 12_usize
            .checked_add(fields.len())
            .and_then(|size| size.checked_add(message_data.len()))
            .and_then(|size| u32::try_from(size).ok())
            .expect("test emsg size fits u32");
        let mut event = Vec::with_capacity(size as usize);
        event.extend_from_slice(&size.to_be_bytes());
        event.extend_from_slice(b"emsg");
        event.extend_from_slice(&[version, 0, 0, 0]);
        event.extend_from_slice(fields);
        event.extend_from_slice(message_data);
        event
    }

    fn event_message_v0() -> Vec<u8> {
        let mut fields = Vec::new();
        fields.extend_from_slice(b"urn:mpeg:dash:event:2012\0");
        fields.extend_from_slice(b"campaign-marker\0");
        fields.extend_from_slice(&1_000_u32.to_be_bytes());
        fields.extend_from_slice(&250_u32.to_be_bytes());
        fields.extend_from_slice(&1_500_u32.to_be_bytes());
        fields.extend_from_slice(&7_u32.to_be_bytes());
        event_message_box(0, &fields, b"v0-payload")
    }

    fn event_message_v1() -> Vec<u8> {
        let mut fields = Vec::new();
        fields.extend_from_slice(&90_000_u32.to_be_bytes());
        fields.extend_from_slice(&180_000_u64.to_be_bytes());
        fields.extend_from_slice(&45_000_u32.to_be_bytes());
        fields.extend_from_slice(&9_u32.to_be_bytes());
        fields.extend_from_slice(b"https://aomedia.org/emsg/ID3\0");
        fields.extend_from_slice(b"id3\0");
        event_message_box(1, &fields, b"v1-payload")
    }

    #[test]
    fn mpeg_ts_metadata_stream_type_maps_to_id3_codec() {
        assert_eq!(mpeg_ts_data_codec(0x15), Codec::Id3);
        assert_eq!(mpeg_ts_data_codec(0x06), Codec::Data);
    }

    #[test]
    fn cmaf_demux_preserves_tfdt_cts_duration_flags_and_payload() {
        let track = video_track();
        let init = build_init_segment(std::slice::from_ref(&track), 1_000)
            .expect("CMAF initialization must build");
        let samples = [
            Sample::new(vec![0, 0, 0, 1, 0x65], 3_003, true, 1_001),
            Sample::new(vec![0, 0, 0, 1, 0x41], 3_003, false, -1_001),
        ];
        let fragment = FragmentTrackData {
            track_id: track.track_id,
            base_media_decode_time: 180_000,
            samples: &samples,
        };
        let segment = build_media_segment(7, &[fragment]).expect("CMAF media segment must build");
        let initialization = CmafInitialization::parse(&init).expect("initialization must parse");
        assert_eq!(initialization.tracks()[0].codec(), Codec::H264);
        let mut demux = CmafDemuxer::new(initialization);
        let demuxed = demux
            .demux_segment(&Bytes::from(segment), true)
            .expect("media segment must demux");

        let demuxed = demuxed.samples();
        assert_eq!(demuxed.len(), 2);
        assert_eq!(demuxed[0].decode_time().ticks(), 180_000);
        assert_eq!(demuxed[0].presentation_time().ticks(), 181_001);
        assert_eq!(demuxed[0].duration().ticks(), 3_003);
        assert!(demuxed[0].is_keyframe());
        assert!(demuxed[0].is_discontinuity());
        assert_eq!(demuxed[0].data().as_ref(), samples[0].data.as_slice());
        assert_eq!(demuxed[1].decode_time().ticks(), 183_003);
        assert_eq!(demuxed[1].presentation_time().ticks(), 182_002);
        assert!(!demuxed[1].is_keyframe());
        assert!(!demuxed[1].is_discontinuity());
    }

    #[test]
    fn cmaf_demux_preserves_v0_and_v1_timed_event_messages() {
        let track = video_track();
        let initialization = CmafInitialization::parse(
            &build_init_segment(std::slice::from_ref(&track), 1_000)
                .expect("CMAF initialization must build"),
        )
        .expect("CMAF initialization must parse");
        let sample = Sample::new(vec![0, 0, 0, 1, 0x65], 3_000, true, 0);
        let mut segment = event_message_v0();
        segment.extend_from_slice(&event_message_v1());
        segment.extend_from_slice(
            &build_media_segment(
                8,
                &[FragmentTrackData {
                    track_id: track.track_id,
                    base_media_decode_time: 450_000,
                    samples: std::slice::from_ref(&sample),
                }],
            )
            .expect("CMAF media segment must build"),
        );

        let demuxed = CmafDemuxer::new(initialization)
            .demux_segment(&Bytes::from(segment), false)
            .expect("CMAF event messages must demux");
        let metadata = demuxed.timed_metadata();

        assert_eq!(metadata.len(), 2);
        assert_eq!(metadata[0].scheme_id_uri(), "urn:mpeg:dash:event:2012");
        assert_eq!(metadata[0].value(), "campaign-marker");
        assert_eq!(metadata[0].id(), 7);
        assert_eq!(
            metadata[0].presentation_time(),
            Duration::from_millis(5_250)
        );
        assert_eq!(metadata[0].duration(), Duration::from_millis(1_500));
        assert_eq!(metadata[0].message_data().as_ref(), b"v0-payload");
        assert_eq!(metadata[1].scheme_id_uri(), "https://aomedia.org/emsg/ID3");
        assert_eq!(metadata[1].value(), "id3");
        assert_eq!(metadata[1].id(), 9);
        assert_eq!(metadata[1].presentation_time(), Duration::from_secs(2));
        assert_eq!(metadata[1].duration(), Duration::from_millis(500));
        assert_eq!(metadata[1].message_data().as_ref(), b"v1-payload");
    }

    #[test]
    fn protected_cmaf_preserves_track_and_sample_encryption_metadata() {
        let track = video_track();
        let samples = [
            Sample::new(vec![0, 0, 0, 1, 0x65], 3_003, true, 0),
            Sample::new(vec![0, 0, 0, 1, 0x41], 3_003, false, 0),
        ];
        let encryption_entries = vec![
            SampleEncryptionEntry {
                initialization_vector: vec![0x11; 8],
                subsamples: vec![SubSampleEntry {
                    bytes_of_clear_data: 1,
                    bytes_of_protected_data: 4,
                }],
            },
            SampleEncryptionEntry {
                initialization_vector: vec![0x22; 8],
                subsamples: vec![
                    SubSampleEntry {
                        bytes_of_clear_data: 1,
                        bytes_of_protected_data: 1,
                    },
                    SubSampleEntry {
                        bytes_of_clear_data: 1,
                        bytes_of_protected_data: 2,
                    },
                ],
            },
        ];
        let encryption = TrackEncryption {
            scheme: CencScheme::Cenc,
            tenc: TrackEncryptionBox {
                version: 0,
                default_crypt_byte_block: 0,
                default_skip_byte_block: 0,
                default_is_protected: 1,
                default_per_sample_iv_size: 8,
                default_kid: [0x44; 16],
                default_constant_iv: None,
            },
            samples: encryption_entries.clone(),
        };
        let init = protect_init_segment(
            &build_init_segment(std::slice::from_ref(&track), 1_000)
                .expect("CMAF initialization must build"),
            track.track_id,
            &encryption,
        )
        .expect("CMAF initialization must become protected");
        let segment = protect_media_segment(
            &build_media_segment(
                7,
                &[FragmentTrackData {
                    track_id: track.track_id,
                    base_media_decode_time: 180_000,
                    samples: &samples,
                }],
            )
            .expect("CMAF media segment must build"),
            &[FragmentProtection {
                track_id: track.track_id,
                entries: &encryption_entries,
                per_sample_iv_size: 8,
            }],
        )
        .expect("CMAF media segment must become protected");

        let initialization =
            CmafInitialization::parse(&init).expect("protected CMAF initialization must parse");
        let protection = initialization.tracks()[0]
            .protection()
            .expect("track protection must be retained");
        assert_eq!(protection.default_key_id(), &[0x44; 16]);
        assert_eq!(protection.per_sample_iv_size(), 8);
        let demuxed = CmafDemuxer::new(initialization)
            .demux_segment(&Bytes::from(segment), false)
            .expect("protected CMAF samples must demux");

        let demuxed = demuxed.samples();
        assert_eq!(demuxed.len(), 2);
        assert_eq!(
            demuxed[0]
                .encryption()
                .expect("first sample must remain protected")
                .initialization_vector(),
            &[0x11; 8]
        );
        let second = demuxed[1]
            .encryption()
            .expect("second sample must remain protected");
        assert_eq!(second.initialization_vector(), &[0x22; 8]);
        assert_eq!(second.subsamples().len(), 2);
        assert_eq!(second.subsamples()[1].encrypted_bytes(), 2);
    }

    #[test]
    fn chunked_cmaf_emits_each_fragment_before_the_complete_segment_arrives() {
        let track = video_track();
        let initialization = CmafInitialization::parse(
            &build_init_segment(std::slice::from_ref(&track), 1_000)
                .expect("CMAF initialization must build"),
        )
        .expect("CMAF initialization must parse");
        let first_sample = Sample::new(vec![0, 0, 0, 1, 0x65], 3_000, true, 0);
        let second_sample = Sample::new(vec![0, 0, 0, 1, 0x41], 3_000, false, 0);
        let first = build_media_segment(
            1,
            &[FragmentTrackData {
                track_id: track.track_id,
                base_media_decode_time: 0,
                samples: std::slice::from_ref(&first_sample),
            }],
        )
        .expect("first CMAF chunk must build");
        let second = build_media_segment(
            2,
            &[FragmentTrackData {
                track_id: track.track_id,
                base_media_decode_time: 3_000,
                samples: std::slice::from_ref(&second_sample),
            }],
        )
        .expect("second CMAF chunk must build");
        let split = first.len() / 2;
        let mut demuxer = CmafChunkDemuxer::new(initialization);

        assert!(
            demuxer
                .feed(&first[..split], true)
                .expect("partial network chunk must buffer")
                .is_empty()
        );
        let first_decoded = demuxer
            .feed(&first[split..], false)
            .expect("completed first CMAF chunk must demux");
        assert_eq!(first_decoded.samples().len(), 1);
        assert!(first_decoded.samples()[0].is_discontinuity());
        let second_decoded = demuxer
            .feed(&second, false)
            .expect("second CMAF chunk must demux independently");
        assert_eq!(second_decoded.samples().len(), 1);
        assert_eq!(second_decoded.samples()[0].decode_time().ticks(), 3_000);
        demuxer.finish().expect("complete response must finish");
    }

    #[test]
    fn cmaf_aac_track_exposes_raw_audio_specific_config() {
        let initialization = CmafInitialization::parse(
            &build_init_segment(&[audio_track()], 1_000)
                .expect("AAC CMAF initialization must build"),
        )
        .expect("AAC CMAF initialization must parse");
        let track = &initialization.tracks()[0];

        assert_eq!(track.codec(), Codec::Aac);
        assert_eq!(track.decoder_configuration(), [0x12, 0x10]);
        let layout = track.audio_layout().expect("AAC track must declare layout");
        assert_eq!(layout.channels.get(), 2);
        assert_eq!(layout.sample_rate.get(), 44_100);
    }

    #[test]
    fn cmaf_ttml_sample_maps_local_cues_to_presentation_time() {
        let track = subtitle_track(Codec::Ttml);
        let sample = subtitle_sample(
            b"<tt><body><div><p begin=\"500ms\" end=\"1500ms\">Mapped</p></div></body></tt>"
                .to_vec(),
        );

        let cues =
            decode_cmaf_subtitle_sample(&track, &sample).expect("sample-local TTML must decode");

        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start, std::time::Duration::from_millis(5_500));
        assert_eq!(cues[0].end, std::time::Duration::from_millis(6_500));
        assert_eq!(cues[0].text, "Mapped");
    }

    #[test]
    fn cmaf_webvtt_sample_uses_sample_interval() {
        let track = subtitle_track(Codec::WebVtt);
        let cue_box = transmux::VttCueBox::new("CMAF WebVTT");
        let sample = subtitle_sample(cue_box.to_bytes());

        let cues =
            decode_cmaf_subtitle_sample(&track, &sample).expect("WebVTT cue box must decode");

        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start, std::time::Duration::from_secs(5));
        assert_eq!(cues[0].end, std::time::Duration::from_secs(7));
        assert_eq!(cues[0].text, "CMAF WebVTT");
    }

    #[test]
    fn media_time_retains_exact_broadcast_scale() {
        let time = MediaTime::new(
            90_000,
            NonZeroU32::new(90_000).expect("test timescale is non-zero"),
        );
        assert_eq!(
            time.to_duration().expect("positive time"),
            std::time::Duration::from_secs(1)
        );
    }

    #[test]
    fn avc_record_fixture_is_structurally_valid() {
        let track = video_track();
        let CodecConfig::Avc { config, .. } = track.config else {
            unreachable!("test track is AVC")
        };
        let bytes = broadcast_common::Serialize::to_bytes(&config.config);
        AVCDecoderConfigurationRecord::parse(&bytes).expect("AVC configuration must round-trip");
    }
}
