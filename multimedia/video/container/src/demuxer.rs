//! Video demuxer and frame representation.

use broadcast_common::Parse;
use mp4::WriteBox;
use std::{
    io::{BufReader, Cursor, Read, Seek, SeekFrom},
    path::Path,
    time::Duration,
};
use transmux::{MovieFragmentBox, TrackExtendsBox, TrackFragmentBox};
use waterkit_video_core::Error;

use crate::isobmff::{TopLevelBoxSpan, read_top_level_box, scan_top_level_boxes};
use crate::stream::{SAMPLE_FLAG_IS_NON_SYNC, TFHD_DEFAULT_BASE_IS_MOOF};

type VideoError = Error;

#[derive(Debug, Clone, Copy)]
pub struct SampleMeta {
    pub decode_time: u64,
    pub presentation_time: u64,
    pub duration: u32,
    pub is_keyframe: bool,
}

/// Embedded subtitle codec carried inside the MP4 container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedSubtitleCodec {
    /// MPEG-4 Timed Text (`tx3g`).
    Tx3g,
}

/// Metadata describing one embedded subtitle track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedSubtitleTrack {
    /// MP4 track id.
    pub track_id: u32,
    /// Track language from `mdhd`.
    pub language: String,
    /// Subtitle sample entry codec.
    pub codec: EmbeddedSubtitleCodec,
}

/// One decoded embedded subtitle cue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedSubtitleCue {
    /// Cue start presentation time.
    pub start: Duration,
    /// Cue end presentation time.
    pub end: Duration,
    /// Decoded cue text payload.
    pub text: String,
}

/// The coded video format a [`VideoReader`]'s track carries.
///
/// Read from the track's sample entry, which is where the container states it.
/// Sniffing it back out of the codec-configuration bytes cannot tell an `av1C`
/// from anything else, and guesses `avcC` from a profile byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264 / AVC, configured by an `avcC` box.
    H264,
    /// H.265 / HEVC, configured by an `hvcC` box.
    H265,
    /// AV1, configured by an `av1C` box.
    Av1,
}

/// Video reader for MP4/MOV files.
#[derive(Debug)]
pub struct VideoReader {
    reader: mp4::Mp4Reader<BufReader<std::fs::File>>,
    video_track_id: u32,
    width: u32,
    height: u32,
    sample_metas: Vec<SampleMeta>,
    fragmented_samples: Option<FragmentedSamples>,
    codec: Option<VideoCodec>,
    codec_config: Option<Vec<u8>>,
    current_index: usize,
    timescale: u32,
    has_audio: bool,
}

/// Payload byte ranges for a fragmented track plus a handle to read them with.
///
/// `mp4::Mp4Reader` cannot resolve fragmented sample offsets (it returns the
/// `tfhd` base offset for every sample and only honours a single `trun` per
/// `traf`), so fragmented payloads are read through this index instead.
#[derive(Debug)]
struct FragmentedSamples {
    reader: BufReader<std::fs::File>,
    locations: Vec<SampleLocation>,
}

impl VideoReader {
    /// Probe a media file and return `Ok(())` when it can be opened as video.
    ///
    /// # Errors
    ///
    /// Returns the same error as [`VideoReader::open`] when probing fails.
    pub fn probe<P: AsRef<Path>>(path: P) -> Result<(), VideoError> {
        Self::open(path).map(|_| ())
    }

    /// Open a video file for reading.
    ///
    /// # Errors
    /// Returns [`VideoError::Io`] if the file cannot be opened.
    #[allow(clippy::cast_possible_truncation)]
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, VideoError> {
        let path_ref = path.as_ref();
        let file = std::fs::File::open(path_ref)?;
        let size = file.metadata()?.len();
        let reader = mp4::Mp4Reader::read_header(BufReader::new(file), size)
            .map_err(|e| VideoError::Container(e.to_string()))?;

        // Find video track
        let mut video_track_id = 0;
        let mut width = 0u32;
        let mut height = 0u32;
        let mut codec: Option<VideoCodec> = None;
        let mut codec_config: Option<Vec<u8>> = None;
        let mut timescale = 0u32;
        let mut has_audio = false;

        for track in reader.tracks().values() {
            let track_type = track
                .track_type()
                .map_err(|e| VideoError::Container(e.to_string()))?;
            if track_type == mp4::TrackType::Audio {
                has_audio = true;
            } else if video_track_id == 0 && track_type == mp4::TrackType::Video {
                video_track_id = track.track_id();
                width = u32::from(track.width());
                height = u32::from(track.height());
                timescale = track.timescale();

                let stsd = &track.trak.mdia.minf.stbl.stsd;

                if let Some(avc1) = &stsd.avc1 {
                    let avcc = &avc1.avcc;
                    let mut buf = Vec::new();
                    let mut cursor = Cursor::new(&mut buf);
                    if avcc.write_box(&mut cursor).is_ok() {
                        codec = Some(VideoCodec::H264);
                        codec_config = Some(buf);
                    }
                } else if let Some(av1c) = extract_box_from_file(path_ref, *b"av1C")? {
                    // The mp4 crate has no `av01` sample entry, so it silently
                    // skips the one this track has; read the `av1C` box out of
                    // the file the same way HEVC's is read below.
                    codec = Some(VideoCodec::Av1);
                    codec_config = Some(av1c);
                } else {
                    // For HEVC (hvc1/hev1), mp4 crate cannot reliably expose raw hvcC bytes.
                    // Extract hvcC atom directly from file for decoder initialization.
                    codec = Some(VideoCodec::H265);
                    codec_config = extract_box_from_file(path_ref, *b"hvcC")?;
                }
            }
        }

        if video_track_id == 0 {
            return Err(VideoError::Container("No video track found".into()));
        }

        let track = reader.tracks().get(&video_track_id).ok_or_else(|| {
            VideoError::Container(format!(
                "missing video track metadata for track {video_track_id}"
            ))
        })?;
        let indexed = index_track_samples(path_ref, track)?;
        let fragmented_samples = indexed
            .locations
            .map(|locations| {
                Ok::<_, VideoError>(FragmentedSamples {
                    reader: BufReader::new(std::fs::File::open(path_ref)?),
                    locations,
                })
            })
            .transpose()?;
        let sample_metas = indexed.metas;

        Ok(Self {
            reader,
            video_track_id,
            width,
            height,
            sample_metas,
            fragmented_samples,
            codec,
            codec_config,
            current_index: 0,
            timescale,
            has_audio,
        })
    }

    /// Get timescale.
    #[must_use]
    pub const fn timescale(&self) -> u32 {
        self.timescale
    }

    /// Get video dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Returns whether the container declares at least one audio track.
    #[must_use]
    pub const fn has_audio(&self) -> bool {
        self.has_audio
    }

    /// Get total sample count.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub const fn sample_count(&self) -> u32 {
        self.sample_metas.len() as u32
    }

    /// Get the current sample cursor index.
    #[must_use]
    pub const fn current_index(&self) -> usize {
        self.current_index
    }

    /// Return sample timing metadata at `index`.
    ///
    /// Returns `(pts, duration, is_keyframe)` when the sample exists.
    #[must_use]
    pub fn sample_info(&self, index: usize) -> Option<(u64, u32, bool)> {
        self.sample_metas
            .get(index)
            .map(|meta| (meta.presentation_time, meta.duration, meta.is_keyframe))
    }

    /// Return estimated stream duration from the last sample PTS.
    #[must_use]
    pub fn duration(&self) -> Option<std::time::Duration> {
        let presentation_end = self.sample_metas.iter().fold(0_u64, |end, meta| {
            end.max(
                meta.presentation_time
                    .saturating_add(u64::from(meta.duration)),
            )
        });
        if self.timescale == 0 {
            return Some(std::time::Duration::ZERO);
        }
        Some(std::time::Duration::from_nanos(
            presentation_end.saturating_mul(1_000_000_000) / u64::from(self.timescale),
        ))
    }

    /// Find nearest keyframe index at or before `index`.
    #[must_use]
    pub fn nearest_keyframe_at_or_before(&self, index: usize) -> usize {
        if self.sample_metas.is_empty() {
            return 0;
        }
        let clamped = index.min(self.sample_metas.len().saturating_sub(1));
        for candidate in (0..=clamped).rev() {
            if self.sample_metas[candidate].is_keyframe {
                return candidate;
            }
        }
        0
    }

    /// Seek the internal cursor to a sample index.
    pub fn seek_to_sample(&mut self, index: usize) {
        self.current_index = index.min(self.sample_metas.len());
    }

    /// Read the next video sample (encoded data).
    /// Returns `(data, pts_in_timescale_units, is_keyframe)` or None if
    /// at end. Convert the raw `pts` to `Duration` via the reader's
    /// [`timescale`](Self::timescale).
    ///
    /// # Errors
    ///
    /// Returns an error when the sample index exceeds the MP4 reader range or
    /// when the underlying container reader fails to load the sample.
    pub fn read_sample(&mut self) -> Result<Option<(Vec<u8>, u64, bool)>, VideoError> {
        if self.current_index >= self.sample_metas.len() {
            return Ok(None);
        }

        let sample_index = self.current_index;
        self.current_index += 1;
        let meta = self.sample_metas[sample_index];
        if let Some(fragmented) = self.fragmented_samples.as_mut() {
            let location = fragmented.locations.get(sample_index).ok_or_else(|| {
                VideoError::Container(String::from(
                    "fragmented sample index exceeds the parsed fragment layout",
                ))
            })?;
            let mut bytes = vec![
                0_u8;
                usize::try_from(location.size).map_err(|_| {
                    VideoError::Container(String::from(
                        "fragmented sample size exceeds the current architecture",
                    ))
                })?
            ];
            fragmented.reader.seek(SeekFrom::Start(location.offset))?;
            fragmented.reader.read_exact(&mut bytes)?;
            return Ok(Some((bytes, meta.presentation_time, meta.is_keyframe)));
        }
        let sample_id = u32::try_from(sample_index + 1).map_err(|_| {
            VideoError::Container("sample index exceeds mp4 reader range".to_string())
        })?;
        let sample = self
            .reader
            .read_sample(self.video_track_id, sample_id)
            .map_err(|error| VideoError::Container(error.to_string()))?;
        Ok(sample.map(|sample| {
            (
                sample.bytes.to_vec(),
                meta.presentation_time,
                meta.is_keyframe,
            )
        }))
    }

    /// Iterate over samples from the current position.
    pub fn samples(
        &mut self,
    ) -> impl Iterator<Item = Result<(Vec<u8>, u64, bool), VideoError>> + '_ {
        std::iter::from_fn(move || match self.read_sample() {
            Ok(Some(sample)) => Some(Ok(sample)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
    }

    /// The coded video format this file's video track carries.
    ///
    /// `None` when the track declares a sample entry this reader does not
    /// recognize.
    #[must_use]
    pub const fn codec(&self) -> Option<VideoCodec> {
        self.codec
    }

    /// Get codec configuration (avcC, hvcC or av1C raw data).
    #[must_use]
    pub fn codec_config(&self) -> Option<&[u8]> {
        self.codec_config.as_deref()
    }

    /// Reset to beginning.
    pub const fn reset(&mut self) {
        self.current_index = 0;
    }
}

/// Read embedded subtitle track metadata from an MP4/MOV file.
///
/// # Errors
///
/// Returns an error when the file cannot be opened or the MP4 header cannot be parsed.
pub fn embedded_subtitle_tracks<P: AsRef<Path>>(
    path: P,
) -> Result<Vec<EmbeddedSubtitleTrack>, VideoError> {
    let reader = open_mp4_reader(path)?;
    let mut tracks = Vec::new();

    for track in reader.tracks().values() {
        let track_type = track
            .track_type()
            .map_err(|error| VideoError::Container(error.to_string()))?;
        if track_type != mp4::TrackType::Subtitle {
            continue;
        }

        let codec = match track
            .media_type()
            .map_err(|error| VideoError::Container(error.to_string()))?
        {
            mp4::MediaType::TTXT => EmbeddedSubtitleCodec::Tx3g,
            _ => continue,
        };

        tracks.push(EmbeddedSubtitleTrack {
            track_id: track.track_id(),
            language: track.language().to_owned(),
            codec,
        });
    }

    Ok(tracks)
}

/// Decode all cues from one embedded subtitle track.
///
/// # Errors
///
/// Returns an error when the track does not exist, is not a supported subtitle codec,
/// or one of its samples cannot be decoded.
pub fn read_embedded_subtitle_cues<P: AsRef<Path>>(
    path: P,
    track_id: u32,
) -> Result<Vec<EmbeddedSubtitleCue>, VideoError> {
    let mut reader = open_mp4_reader(path)?;
    let track = reader.tracks().get(&track_id).ok_or_else(|| {
        VideoError::Container(format!("embedded subtitle track {track_id} not found"))
    })?;
    let track_type = track
        .track_type()
        .map_err(|error| VideoError::Container(error.to_string()))?;
    if track_type != mp4::TrackType::Subtitle {
        return Err(VideoError::Container(format!(
            "track {track_id} is not a subtitle track"
        )));
    }

    match track
        .media_type()
        .map_err(|error| VideoError::Container(error.to_string()))?
    {
        mp4::MediaType::TTXT => {}
        media_type => {
            return Err(VideoError::Unsupported(format!(
                "embedded subtitle codec {media_type:?} is not supported"
            )));
        }
    }

    let timescale = track.timescale();
    let sample_count = track.sample_count();
    let mut cues = Vec::with_capacity(sample_count as usize);

    for sample_id in 1..=sample_count {
        let Some(sample) = reader
            .read_sample(track_id, sample_id)
            .map_err(|error| VideoError::Container(error.to_string()))?
        else {
            continue;
        };

        let text = parse_tx3g_sample_text(sample.bytes.as_ref())?;
        let start = timescaled_value_to_duration(sample.start_time, timescale);
        let end = timescaled_value_to_duration(
            sample.start_time.saturating_add(u64::from(sample.duration)),
            timescale,
        );
        cues.push(EmbeddedSubtitleCue { start, end, text });
    }

    Ok(cues)
}

/// Absolute byte range of one coded sample inside a fragmented MP4 file.
#[derive(Debug, Clone, Copy)]
pub struct SampleLocation {
    /// File offset of the first payload byte.
    pub offset: u64,
    /// Payload length in bytes.
    pub size: u32,
}

/// Per-sample index for one track.
///
/// `metas` are in decode order. `locations` carries each sample's payload byte
/// range and is only populated for fragmented tracks; unfragmented payloads
/// stay resolved through the `mp4` sample tables.
#[derive(Debug)]
pub struct IndexedTrackSamples {
    /// Decode-order sample metadata.
    pub metas: Vec<SampleMeta>,
    /// Decode-order payload byte ranges for fragmented tracks.
    pub locations: Option<Vec<SampleLocation>>,
}

/// Indexes one track's samples.
///
/// Unfragmented tracks resolve timing from `stts`/`ctts`/`stss`. Fragmented
/// tracks are parsed from the file's `moof`/`mvex` boxes because `mp4` keeps
/// only one `trun` per `traf` and one `trex` for the whole movie, which loses
/// fragment boundaries and per-track defaults.
///
/// # Errors
///
/// Returns a container error when the sample metadata is malformed.
pub fn index_track_samples(
    path: &Path,
    track: &mp4::Mp4Track,
) -> Result<IndexedTrackSamples, VideoError> {
    if track.trafs.is_empty() {
        return index_progressive_samples(track);
    }
    index_fragment_samples(path, track)
}

fn index_progressive_samples(track: &mp4::Mp4Track) -> Result<IndexedTrackSamples, VideoError> {
    // `stsz` counts only `moov`-declared samples; `track.sample_count()`
    // adds fragment sample counts that the `stts`/`ctts` tables do not cover.
    let sample_count = track.trak.mdia.minf.stbl.stsz.sample_count;
    let capacity = usize::try_from(sample_count).map_err(|_| {
        VideoError::Container(format!(
            "sample count {sample_count} exceeds the current architecture"
        ))
    })?;
    let mut metas = Vec::with_capacity(capacity);
    let mut decode_time = 0_u64;
    for entry in &track.trak.mdia.minf.stbl.stts.entries {
        for _ in 0..entry.sample_count {
            let sample_id = u32::try_from(metas.len())
                .map_err(|_| {
                    VideoError::Container(String::from(
                        "MP4 sample metadata length exceeds the declared u32 sample count",
                    ))
                })?
                .saturating_add(1);
            metas.push(SampleMeta {
                decode_time,
                presentation_time: decode_time,
                duration: entry.sample_delta,
                is_keyframe: is_sync_sample(track, sample_id),
            });
            decode_time = decode_time
                .checked_add(u64::from(entry.sample_delta))
                .ok_or_else(|| {
                    VideoError::Container(String::from("MP4 decode timeline exceeds u64 ticks"))
                })?;
        }
    }
    if metas.len() != capacity {
        return Err(VideoError::Container(format!(
            "MP4 stts declares {} samples while stsz declares {sample_count}",
            metas.len()
        )));
    }

    if let Some(composition_offsets) = &track.trak.mdia.minf.stbl.ctts {
        let mut sample_index = 0_usize;
        for entry in &composition_offsets.entries {
            let offset =
                composition_time_offset_i64(entry.sample_offset, composition_offsets.version);
            for _ in 0..entry.sample_count {
                let meta = metas.get_mut(sample_index).ok_or_else(|| {
                    VideoError::Container(String::from(
                        "MP4 ctts declares more samples than the sample table",
                    ))
                })?;
                meta.presentation_time = meta
                    .decode_time
                    .checked_add_signed(offset)
                    .ok_or_else(|| {
                        VideoError::Container(format!(
                            "MP4 sample {sample_index} has a negative or overflowing presentation timestamp"
                        ))
                    })?;
                sample_index = sample_index.saturating_add(1);
            }
        }
        if sample_index != metas.len() {
            return Err(VideoError::Container(format!(
                "MP4 ctts declares {sample_index} samples while the sample table declares {}",
                metas.len()
            )));
        }
    }

    Ok(IndexedTrackSamples {
        metas,
        locations: None,
    })
}

/// Resolves the byte ranges of `moov`-declared samples from the `stsz`,
/// `stsc`, and `stco`/`co64` chunk tables.
///
/// `mp4` cannot do this once a track carries `traf`s — it routes every
/// `sample_offset` lookup through fragment data — so the chunk tables are
/// walked here directly.
fn stbl_sample_locations(track: &mp4::Mp4Track) -> Result<Vec<SampleLocation>, VideoError> {
    let stbl = &track.trak.mdia.minf.stbl;
    let stsz = &stbl.stsz;
    let sample_count = usize::try_from(stsz.sample_count).map_err(|_| {
        VideoError::Container(String::from(
            "stsz sample count exceeds the current architecture",
        ))
    })?;
    let chunk_offsets: Vec<u64> = stbl.stco.as_ref().map_or_else(
        || {
            stbl.co64
                .as_ref()
                .map_or_else(Vec::new, |co64| co64.entries.clone())
        },
        |stco| {
            stco.entries
                .iter()
                .map(|offset| u64::from(*offset))
                .collect()
        },
    );

    let mut locations = Vec::with_capacity(sample_count);
    for (chunk_index, chunk_offset) in chunk_offsets.iter().enumerate() {
        let chunk_id = u32::try_from(chunk_index + 1)
            .map_err(|_| VideoError::Container(String::from("chunk index exceeds u32")))?;
        let stsc_index = stbl
            .stsc
            .entries
            .partition_point(|entry| entry.first_chunk <= chunk_id)
            .saturating_sub(1);
        let samples_per_chunk = stbl
            .stsc
            .entries
            .get(stsc_index)
            .map_or(0, |entry| entry.samples_per_chunk);
        let mut offset = *chunk_offset;
        for _ in 0..samples_per_chunk {
            if locations.len() == sample_count {
                break;
            }
            let size = if stsz.sample_size > 0 {
                stsz.sample_size
            } else {
                stsz.sample_sizes
                    .get(locations.len())
                    .copied()
                    .ok_or_else(|| {
                        VideoError::Container(String::from(
                            "stsz declares fewer sizes than its sample count",
                        ))
                    })?
            };
            locations.push(SampleLocation { offset, size });
            offset = offset.checked_add(u64::from(size)).ok_or_else(|| {
                VideoError::Container(String::from("stbl sample byte range overflow"))
            })?;
        }
    }
    if locations.len() != sample_count {
        return Err(VideoError::Container(format!(
            "chunk tables cover {} of {sample_count} declared samples",
            locations.len()
        )));
    }
    Ok(locations)
}

fn index_fragment_samples(
    path: &Path,
    track: &mp4::Mp4Track,
) -> Result<IndexedTrackSamples, VideoError> {
    let track_id = track.track_id();
    let spans = scan_top_level_boxes(path)?;
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();

    let movie_span = spans
        .iter()
        .find(|span| span.kind == *b"moov")
        .ok_or_else(|| VideoError::Container(String::from("fragmented media has no moov box")))?;
    let movie_bytes = read_span_bytes(&mut file, movie_span)?;
    let movie = transmux::MovieBox::parse(&movie_bytes)
        .map_err(|error| VideoError::Container(error.to_string()))?;
    let extends = movie
        .mvex
        .as_ref()
        .ok_or_else(|| VideoError::Container(String::from("fragmented media has no mvex box")))?;
    let track_defaults = extends
        .trex
        .iter()
        .find(|defaults| defaults.track_id == track_id)
        .ok_or_else(|| {
            VideoError::Container(format!("fragmented track {track_id} has no trex defaults"))
        })?;

    // `moov`-declared samples come first in decode order; fragments without
    // `tfdt` continue the timeline from their end.
    let mut metas = index_progressive_samples(track)?.metas;
    let mut locations = stbl_sample_locations(track)?;
    let mut next_decode_time = metas
        .last()
        .map(|meta| meta.decode_time + u64::from(meta.duration));
    for span in spans.iter().filter(|span| span.kind == *b"moof") {
        let fragment = MovieFragmentBox::parse_body(&read_span_body(&mut file, span)?)
            .map_err(|error| VideoError::Container(error.to_string()))?;
        let mut traf_data_end = None;
        for traf in &fragment.traf {
            let base_data_offset = traf.tfhd.base_data_offset.unwrap_or_else(|| {
                if traf.tfhd.flags & TFHD_DEFAULT_BASE_IS_MOOF != 0 {
                    span.offset
                } else {
                    traf_data_end.unwrap_or(span.offset)
                }
            });
            if traf.tfhd.track_id == track_id {
                let traf_samples = collect_traf_samples(
                    traf,
                    extends,
                    track_defaults,
                    base_data_offset,
                    next_decode_time,
                    file_len,
                )?;
                next_decode_time = Some(traf_samples.next_decode_time);
                traf_data_end = traf_samples.data_end.or(traf_data_end);
                for sample in traf_samples.samples {
                    metas.push(sample.meta);
                    locations.push(sample.location);
                }
            } else {
                traf_data_end =
                    foreign_traf_data_end(traf, extends, base_data_offset)?.or(traf_data_end);
            }
        }
    }
    if metas.is_empty() {
        return Err(VideoError::Container(format!(
            "fragmented media declares no samples for track {track_id}"
        )));
    }
    Ok(IndexedTrackSamples {
        metas,
        locations: Some(locations),
    })
}

struct IndexedTrafSamples {
    samples: Vec<IndexedSample>,
    data_end: Option<u64>,
    next_decode_time: u64,
}

struct IndexedSample {
    meta: SampleMeta,
    location: SampleLocation,
}

/// Decodes a composition-time offset according to its box version: version 0
/// stores an unsigned `u32` (which both `mp4`'s `ctts` and `transmux`'s `trun`
/// expose through an `i32` field), while version 1 is a signed `i32`.
fn composition_time_offset_i64(raw: i32, version: u8) -> i64 {
    if version == 0 {
        i64::from(raw.cast_unsigned())
    } else {
        i64::from(raw)
    }
}

/// Resolves a `traf`'s default sample size — `tfhd` first, then that track's
/// `trex` — treating zero defaults as absent.
fn traf_default_size(traf: &TrackFragmentBox, extends: &transmux::MovieExtendsBox) -> Option<u32> {
    traf.tfhd
        .default_sample_size
        .or_else(|| {
            extends
                .trex
                .iter()
                .find(|defaults| defaults.track_id == traf.tfhd.track_id)
                .map(|defaults| defaults.default_sample_size)
        })
        .filter(|size| *size != 0)
}

/// Resolves every run's per-sample sizes: `trun` fields, falling back to the
/// resolved default.
fn traf_run_sizes(
    traf: &TrackFragmentBox,
    default_size: Option<u32>,
) -> Result<Vec<Vec<u32>>, VideoError> {
    traf.trun
        .iter()
        .map(|run| {
            run.samples
                .iter()
                .map(|sample| {
                    sample.sample_size.or(default_size).ok_or_else(|| {
                        VideoError::Container(format!(
                            "fragmented sample in track {} has no size",
                            traf.tfhd.track_id
                        ))
                    })
                })
                .collect()
        })
        .collect()
}

/// Resolves one `traf`'s samples in decode order.
///
/// Per-sample `trun` fields take precedence over `tfhd` defaults, which take
/// precedence over `trex` defaults. `tfdt` resets the decode-time base when
/// present; otherwise the timeline continues from the preceding samples
/// (`moov`-declared or earlier fragments), starting at zero.
fn collect_traf_samples(
    traf: &TrackFragmentBox,
    extends: &transmux::MovieExtendsBox,
    defaults: &TrackExtendsBox,
    base_data_offset: u64,
    next_decode_time: Option<u64>,
    file_len: u64,
) -> Result<IndexedTrafSamples, VideoError> {
    let mut decode_time = traf
        .tfdt
        .as_ref()
        .map(transmux::TrackFragmentBaseMediaDecodeTimeBox::base_media_decode_time)
        .or(next_decode_time)
        .unwrap_or(0);
    let default_duration = traf
        .tfhd
        .default_sample_duration
        .unwrap_or(defaults.default_sample_duration);
    let default_flags = traf
        .tfhd
        .default_sample_flags
        .unwrap_or(defaults.default_sample_flags);
    let run_sizes = traf_run_sizes(traf, traf_default_size(traf, extends))?;

    let mut samples = Vec::new();
    let mut run_data_end = None;
    for (run, sizes) in traf.trun.iter().zip(run_sizes) {
        let mut data_offset = match run.data_offset {
            Some(relative) => checked_add_signed(base_data_offset, relative)?,
            None => run_data_end.unwrap_or(base_data_offset),
        };
        for (index, (sample, size)) in run.samples.iter().zip(sizes).enumerate() {
            let duration = sample.sample_duration.unwrap_or(default_duration);
            if duration == 0 {
                return Err(VideoError::Container(format!(
                    "fragmented sample in track {} has no duration",
                    traf.tfhd.track_id
                )));
            }
            let flags = sample
                .sample_flags
                .or_else(|| (index == 0).then_some(run.first_sample_flags).flatten())
                .unwrap_or(default_flags);
            let data_end = data_offset.checked_add(u64::from(size)).ok_or_else(|| {
                VideoError::Container(String::from("fragmented sample byte range overflow"))
            })?;
            if data_end > file_len {
                return Err(VideoError::Container(format!(
                    "fragmented sample in track {} ends at byte {data_end}, beyond the {file_len}-byte file",
                    traf.tfhd.track_id
                )));
            }
            let presentation_time = decode_time
                .checked_add_signed(composition_time_offset_i64(
                    sample.sample_composition_time_offset.unwrap_or(0),
                    run.version,
                ))
                .ok_or_else(|| {
                    VideoError::Container(String::from(
                        "fragmented sample has a negative or overflowing presentation timestamp",
                    ))
                })?;
            samples.push(IndexedSample {
                meta: SampleMeta {
                    decode_time,
                    presentation_time,
                    duration,
                    is_keyframe: flags & SAMPLE_FLAG_IS_NON_SYNC == 0,
                },
                location: SampleLocation {
                    offset: data_offset,
                    size,
                },
            });
            data_offset = data_end;
            decode_time = decode_time
                .checked_add(u64::from(duration))
                .ok_or_else(|| {
                    VideoError::Container(String::from(
                        "fragmented decode timeline exceeds u64 ticks",
                    ))
                })?;
        }
        run_data_end = Some(data_offset);
    }
    Ok(IndexedTrafSamples {
        samples,
        data_end: run_data_end,
        next_decode_time: decode_time,
    })
}

/// Computes the end of a foreign track's fragment data so a later `traf` in
/// the same `moof` without an explicit `tfhd` base can chain off it.
///
/// # Errors
///
/// Returns a container error when the foreign fragment's sample sizes or
/// offsets are malformed — an unresolvable size or an overflowing offset
/// cannot be treated as "no data" without silently corrupting later bases.
fn foreign_traf_data_end(
    traf: &TrackFragmentBox,
    extends: &transmux::MovieExtendsBox,
    base_data_offset: u64,
) -> Result<Option<u64>, VideoError> {
    let run_sizes = traf_run_sizes(traf, traf_default_size(traf, extends))?;
    let mut run_data_end = None;
    for (run, sizes) in traf.trun.iter().zip(run_sizes) {
        let mut data_offset = match run.data_offset {
            Some(relative) => checked_add_signed(base_data_offset, relative)?,
            None => run_data_end.unwrap_or(base_data_offset),
        };
        for size in sizes {
            data_offset = data_offset.checked_add(u64::from(size)).ok_or_else(|| {
                VideoError::Container(String::from("fragmented sample byte range overflow"))
            })?;
        }
        run_data_end = Some(data_offset);
    }
    Ok(run_data_end)
}

fn checked_add_signed(base: u64, relative: i32) -> Result<u64, VideoError> {
    base.checked_add_signed(i64::from(relative)).ok_or_else(|| {
        VideoError::Container(format!(
            "fragmented data offset {relative} is invalid relative to base {base}"
        ))
    })
}

fn read_span_bytes(
    file: &mut std::fs::File,
    span: &TopLevelBoxSpan,
) -> Result<Vec<u8>, VideoError> {
    file.seek(SeekFrom::Start(span.offset))?;
    read_file_bytes(file, span.size)
}

fn read_span_body(file: &mut std::fs::File, span: &TopLevelBoxSpan) -> Result<Vec<u8>, VideoError> {
    file.seek(SeekFrom::Start(span.body_offset))?;
    read_file_bytes(file, span.offset + span.size - span.body_offset)
}

fn read_file_bytes(file: &mut std::fs::File, len: u64) -> Result<Vec<u8>, VideoError> {
    let allocation = usize::try_from(len).map_err(|_| {
        VideoError::Container(String::from(
            "ISO BMFF box exceeds the current architecture",
        ))
    })?;
    let mut bytes = vec![0_u8; allocation];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn is_sync_sample(track: &mp4::Mp4Track, sample_id: u32) -> bool {
    track
        .trak
        .mdia
        .minf
        .stbl
        .stss
        .as_ref()
        .is_none_or(|stss| stss.entries.binary_search(&sample_id).is_ok())
}

fn extract_box_from_file(path: &Path, box_type: [u8; 4]) -> Result<Option<Vec<u8>>, VideoError> {
    let movie = read_top_level_box(path, *b"moov")?
        .ok_or_else(|| VideoError::Container(String::from("video file has no moov box")))?;
    Ok(extract_box_from_bytes(&movie, box_type))
}

fn extract_box_from_bytes(bytes: &[u8], box_type: [u8; 4]) -> Option<Vec<u8>> {
    let pos = bytes.windows(4).position(|window| window == box_type)?;
    if pos < 4 {
        return None;
    }

    let size_pos = pos - 4;
    let box_size = usize::try_from(u32::from_be_bytes([
        bytes[size_pos],
        bytes[size_pos + 1],
        bytes[size_pos + 2],
        bytes[size_pos + 3],
    ]))
    .ok()?;
    if box_size <= 8 || size_pos.saturating_add(box_size) > bytes.len() {
        return None;
    }

    Some(bytes[size_pos..size_pos + box_size].to_vec())
}

fn open_mp4_reader<P: AsRef<Path>>(
    path: P,
) -> Result<mp4::Mp4Reader<BufReader<std::fs::File>>, VideoError> {
    let file = std::fs::File::open(path.as_ref())?;
    let size = file.metadata()?.len();
    mp4::Mp4Reader::read_header(BufReader::new(file), size)
        .map_err(|error| VideoError::Container(error.to_string()))
}

fn parse_tx3g_sample_text(bytes: &[u8]) -> Result<String, VideoError> {
    if bytes.len() < 2 {
        return Err(VideoError::Container(
            "tx3g subtitle sample is missing the length prefix".to_string(),
        ));
    }

    let text_len = usize::from(u16::from_be_bytes([bytes[0], bytes[1]]));
    let text_end = 2usize
        .checked_add(text_len)
        .ok_or_else(|| VideoError::Container("tx3g subtitle sample length overflow".to_string()))?;
    if bytes.len() < text_end {
        return Err(VideoError::Container(format!(
            "tx3g subtitle sample truncated: expected {text_end} bytes, got {}",
            bytes.len()
        )));
    }

    let payload = &bytes[2..text_end];
    if payload.is_empty() {
        return Ok(String::new());
    }

    if payload.starts_with(&[0xfe, 0xff]) {
        return decode_utf16(&payload[2..], false);
    }
    if payload.starts_with(&[0xff, 0xfe]) {
        return decode_utf16(&payload[2..], true);
    }

    String::from_utf8(payload.to_vec()).map_err(|error| {
        VideoError::Container(format!("tx3g subtitle sample is not valid UTF-8: {error}"))
    })
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, VideoError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(VideoError::Container(
            "tx3g UTF-16 subtitle payload must have an even byte length".to_string(),
        ));
    }

    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| {
            if little_endian {
                u16::from_le_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]])
            }
        })
        .collect::<Vec<_>>();

    String::from_utf16(&units).map_err(|error| {
        VideoError::Container(format!(
            "tx3g subtitle payload is not valid UTF-16: {error}"
        ))
    })
}

fn timescaled_value_to_duration(value: u64, timescale: u32) -> Duration {
    if timescale == 0 {
        return Duration::ZERO;
    }

    Duration::from_nanos(value.saturating_mul(1_000_000_000) / u64::from(timescale))
}

#[cfg(test)]
mod tests {
    use super::{decode_utf16, extract_box_from_bytes, parse_tx3g_sample_text};

    #[test]
    fn extracts_hvcc_box_from_payload() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0, 0, 0, 4, b'f', b't', b'y', b'p']);
        bytes.extend_from_slice(&[0, 0, 0, 12, b'h', b'v', b'c', b'C', 1, 2, 3, 4]);

        let hvcc = extract_box_from_bytes(&bytes, *b"hvcC").expect("expected hvcC");
        assert_eq!(hvcc, vec![0, 0, 0, 12, b'h', b'v', b'c', b'C', 1, 2, 3, 4]);
    }

    #[test]
    fn ignores_invalid_box_size() {
        let bytes = [0, 0, 0, 2, b'h', b'v', b'c', b'C'];
        assert!(extract_box_from_bytes(&bytes, *b"hvcC").is_none());
    }

    #[test]
    fn parses_utf8_tx3g_sample_payload() {
        let bytes = [0, 5, b'H', b'e', b'l', b'l', b'o'];
        let text = parse_tx3g_sample_text(&bytes).expect("tx3g parse must succeed");
        assert_eq!(text, "Hello");
    }

    #[test]
    fn parses_utf16be_tx3g_sample_payload_with_bom() {
        let text = decode_utf16(&[0, b'H', 0, b'i'], false).expect("utf16 decode must succeed");
        assert_eq!(text, "Hi");

        let bytes = [0, 6, 0xfe, 0xff, 0, b'H', 0, b'i'];
        let parsed = parse_tx3g_sample_text(&bytes).expect("tx3g parse must succeed");
        assert_eq!(parsed, "Hi");
    }

    const TRACK_ID: u32 = 1;
    const TIMESCALE: u32 = 1_000;
    const SYNC_FLAGS: u32 = 0x0200_0000;
    const NON_SYNC_FLAGS: u32 = 0x0101_0000;

    use std::io::Write as _;

    use broadcast_common::{Package as _, Serialize as _};
    use tempfile::NamedTempFile;
    use transmux::movie_fragment::{
        TFHD_DEFAULT_BASE_IS_MOOF, TFHD_DEFAULT_SAMPLE_DURATION_PRESENT,
        TFHD_DEFAULT_SAMPLE_FLAGS_PRESENT, TRUN_DATA_OFFSET_PRESENT,
        TRUN_FIRST_SAMPLE_FLAGS_PRESENT, TRUN_SAMPLE_COMPOSITION_TIME_OFFSET_PRESENT,
        TRUN_SAMPLE_DURATION_PRESENT, TRUN_SAMPLE_FLAGS_PRESENT, TRUN_SAMPLE_SIZE_PRESENT,
    };
    use transmux::{
        AVCConfigurationBox, AVCDecoderConfigurationRecord, AvcPps, AvcSps, CodecConfig, Media,
        MovieFragmentBox, MovieFragmentHeaderBox, ProgressiveMux, Sample, Track,
        TrackFragmentBaseMediaDecodeTimeBox, TrackFragmentBox, TrackFragmentHeaderBox,
        TrackFragmentRunBox, TrackSpec, TrunSample, build_init_segment,
    };

    use super::VideoReader;

    fn video_spec(track_id: u32) -> TrackSpec {
        TrackSpec::new(
            track_id,
            TIMESCALE,
            CodecConfig::Avc {
                config: AVCConfigurationBox::new(AVCDecoderConfigurationRecord {
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
                }),
                width: 1_920,
                height: 1_080,
            },
        )
    }

    fn traf(
        tfhd_flags: u32,
        tfdt: Option<TrackFragmentBaseMediaDecodeTimeBox>,
        default_sample_duration: Option<u32>,
        default_sample_size: Option<u32>,
        default_sample_flags: Option<u32>,
        runs: Vec<TrackFragmentRunBox>,
    ) -> TrackFragmentBox {
        TrackFragmentBox {
            tfhd: TrackFragmentHeaderBox {
                flags: tfhd_flags,
                track_id: TRACK_ID,
                base_data_offset: None,
                sample_description_index: None,
                default_sample_duration,
                default_sample_size,
                default_sample_flags,
            },
            tfdt,
            trun: runs,
        }
    }

    fn trun(
        version: u8,
        tr_flags: u32,
        first_sample_flags: Option<u32>,
        samples: Vec<TrunSample>,
    ) -> TrackFragmentRunBox {
        TrackFragmentRunBox {
            version,
            tr_flags,
            data_offset: None,
            first_sample_flags,
            samples,
        }
    }

    fn sample(
        duration: Option<u32>,
        size: Option<u32>,
        flags: Option<u32>,
        composition_offset: Option<i32>,
    ) -> TrunSample {
        TrunSample {
            sample_duration: duration,
            sample_size: size,
            sample_flags: flags,
            sample_composition_time_offset: composition_offset,
        }
    }

    /// Serializes a `moof` + `mdat` pair, pointing the first `trun`'s
    /// `data_offset` at the mdat payload that follows the `moof`.
    fn media_segment(sequence_number: u32, traf: TrackFragmentBox, payload: &[u8]) -> Vec<u8> {
        media_segment_multi(sequence_number, vec![traf], payload)
    }

    /// Serializes a `moof` + `mdat` pair holding several `traf`s. The first
    /// `trun` of the first `traf` gets an explicit `data_offset` pointing at
    /// the mdat payload; later runs or `traf`s resolve implicitly.
    fn media_segment_multi(
        sequence_number: u32,
        trafs: Vec<TrackFragmentBox>,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut moof = MovieFragmentBox {
            mfhd: MovieFragmentHeaderBox::new(sequence_number),
            traf: trafs,
        };
        let moof_len = i32::try_from(moof.serialized_len()).expect("moof fits in i32");
        moof.traf[0].trun[0].data_offset = Some(moof_len + 8);
        let mut bytes = moof.to_bytes();
        bytes.extend_from_slice(
            &u32::try_from(payload.len() + 8)
                .expect("mdat fits in u32")
                .to_be_bytes(),
        );
        bytes.extend_from_slice(b"mdat");
        bytes.extend_from_slice(payload);
        bytes
    }

    /// Rewrites the `trex` defaults of a generated init segment.
    fn patch_trex(init: &mut [u8], track_id: u32, duration: u32, size: u32, flags: u32) {
        let mut offset = 0;
        while let Some(position) = init[offset..]
            .windows(4)
            .position(|window| window == b"trex")
        {
            let body = offset + position + 4;
            if u32::from_be_bytes(init[body + 4..body + 8].try_into().expect("trex track id"))
                == track_id
            {
                init[body + 12..body + 16].copy_from_slice(&duration.to_be_bytes());
                init[body + 16..body + 20].copy_from_slice(&size.to_be_bytes());
                init[body + 20..body + 24].copy_from_slice(&flags.to_be_bytes());
                return;
            }
            offset = body;
        }
        panic!("trex for track {track_id} not found in init segment");
    }

    fn write_fixture(bytes: &[u8]) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("temporary media file must open");
        file.write_all(bytes)
            .expect("fixture bytes must be written");
        file
    }

    /// Two fragments: `tfhd`-level duration overriding a zero `trex`,
    /// `tfdt` base decode times, signed `trun` v1 composition offsets that
    /// reorder presentation order, `first_sample_flags`, unequal fragment
    /// lengths, and a second `trun` whose data chains after the first.
    fn fragmented_fixture() -> NamedTempFile {
        let mut bytes = build_init_segment(&[video_spec(TRACK_ID)], TIMESCALE)
            .expect("init segment must serialize");

        let first_traf = traf(
            TFHD_DEFAULT_BASE_IS_MOOF | TFHD_DEFAULT_SAMPLE_DURATION_PRESENT,
            Some(TrackFragmentBaseMediaDecodeTimeBox::new_v1(48_000)),
            Some(1_000),
            None,
            None,
            vec![trun(
                1,
                TRUN_DATA_OFFSET_PRESENT
                    | TRUN_SAMPLE_SIZE_PRESENT
                    | TRUN_SAMPLE_FLAGS_PRESENT
                    | TRUN_SAMPLE_COMPOSITION_TIME_OFFSET_PRESENT,
                None,
                vec![
                    sample(None, Some(4), Some(NON_SYNC_FLAGS), Some(2_000)),
                    sample(None, Some(4), Some(SYNC_FLAGS), Some(0)),
                ],
            )],
        );
        bytes.extend_from_slice(&media_segment(
            1,
            first_traf,
            &[0xA0; 4].into_iter().chain([0xA1; 4]).collect::<Vec<_>>(),
        ));

        let second_traf = traf(
            TFHD_DEFAULT_BASE_IS_MOOF | TFHD_DEFAULT_SAMPLE_FLAGS_PRESENT,
            Some(TrackFragmentBaseMediaDecodeTimeBox::new_v1(50_000)),
            None,
            None,
            Some(NON_SYNC_FLAGS),
            vec![
                trun(
                    0,
                    TRUN_DATA_OFFSET_PRESENT
                        | TRUN_FIRST_SAMPLE_FLAGS_PRESENT
                        | TRUN_SAMPLE_DURATION_PRESENT
                        | TRUN_SAMPLE_SIZE_PRESENT,
                    Some(SYNC_FLAGS),
                    vec![
                        sample(Some(1_500), Some(4), None, None),
                        sample(Some(1_500), Some(4), None, None),
                        sample(Some(1_000), Some(4), None, None),
                    ],
                ),
                trun(
                    0,
                    TRUN_SAMPLE_DURATION_PRESENT
                        | TRUN_SAMPLE_SIZE_PRESENT
                        | TRUN_SAMPLE_FLAGS_PRESENT,
                    None,
                    vec![
                        sample(Some(1_000), Some(4), Some(SYNC_FLAGS), None),
                        sample(Some(1_000), Some(4), Some(NON_SYNC_FLAGS), None),
                    ],
                ),
            ],
        );
        bytes.extend_from_slice(&media_segment(
            2,
            second_traf,
            &[0xB0; 4]
                .into_iter()
                .chain([0xB1; 4])
                .chain([0xB2; 4])
                .chain([0xC0; 4])
                .chain([0xC1; 4])
                .collect::<Vec<_>>(),
        ));
        write_fixture(&bytes)
    }

    /// A single fragment whose samples rely entirely on `trex` defaults
    /// (duration, size, flags), with only `first_sample_flags` overriding the
    /// first sample's sync status and a version-0 `tfdt`.
    fn trex_defaults_fixture() -> NamedTempFile {
        let mut bytes = build_init_segment(&[video_spec(TRACK_ID)], TIMESCALE)
            .expect("init segment must serialize");
        patch_trex(&mut bytes, TRACK_ID, 900, 4, NON_SYNC_FLAGS);

        let traf = traf(
            TFHD_DEFAULT_BASE_IS_MOOF,
            Some(TrackFragmentBaseMediaDecodeTimeBox::new_v0(3_000)),
            None,
            None,
            None,
            vec![trun(
                0,
                TRUN_DATA_OFFSET_PRESENT | TRUN_FIRST_SAMPLE_FLAGS_PRESENT,
                Some(SYNC_FLAGS),
                vec![TrunSample::new(), TrunSample::new()],
            )],
        );
        bytes.extend_from_slice(&media_segment(
            1,
            traf,
            &[0xD0; 4].into_iter().chain([0xD1; 4]).collect::<Vec<_>>(),
        ));
        write_fixture(&bytes)
    }

    /// Two `moof`s without `default-base-is-moof`: each first `traf`
    /// resolves its implicit base to the enclosing `moof`, and a second
    /// `traf` inside one `moof` chains off the first `traf`'s data end.
    /// The first fragment carries no `tfdt` (timeline starts at zero); the
    /// second `moof`'s first `traf` resets it via `tfdt`.
    fn implicit_base_fixture() -> NamedTempFile {
        let mut bytes = build_init_segment(&[video_spec(TRACK_ID)], TIMESCALE)
            .expect("init segment must serialize");

        bytes.extend_from_slice(&media_segment(
            1,
            traf(
                TFHD_DEFAULT_SAMPLE_DURATION_PRESENT,
                None,
                Some(1_000),
                None,
                None,
                vec![trun(
                    0,
                    TRUN_DATA_OFFSET_PRESENT | TRUN_SAMPLE_SIZE_PRESENT | TRUN_SAMPLE_FLAGS_PRESENT,
                    None,
                    vec![sample(None, Some(4), Some(SYNC_FLAGS), None)],
                )],
            ),
            &[0xF0; 4],
        ));

        // An intervening `free` box keeps the second moof's offset from
        // equalling the first segment's data end, so a stale cross-moof base
        // would read the wrong bytes.
        bytes.extend_from_slice(&8_u32.to_be_bytes());
        bytes.extend_from_slice(b"free");

        bytes.extend_from_slice(&media_segment_multi(
            2,
            vec![
                traf(
                    TFHD_DEFAULT_SAMPLE_DURATION_PRESENT,
                    Some(TrackFragmentBaseMediaDecodeTimeBox::new_v1(5_000)),
                    Some(1_000),
                    None,
                    None,
                    vec![trun(
                        0,
                        TRUN_DATA_OFFSET_PRESENT
                            | TRUN_SAMPLE_SIZE_PRESENT
                            | TRUN_SAMPLE_FLAGS_PRESENT,
                        None,
                        vec![sample(None, Some(4), Some(NON_SYNC_FLAGS), None)],
                    )],
                ),
                traf(
                    TFHD_DEFAULT_SAMPLE_DURATION_PRESENT,
                    None,
                    Some(1_000),
                    None,
                    None,
                    vec![trun(
                        0,
                        TRUN_SAMPLE_SIZE_PRESENT | TRUN_SAMPLE_FLAGS_PRESENT,
                        None,
                        vec![sample(None, Some(4), Some(SYNC_FLAGS), None)],
                    )],
                ),
            ],
            &[0xF1; 4].into_iter().chain([0xF2; 4]).collect::<Vec<_>>(),
        ));
        write_fixture(&bytes)
    }

    /// Appends an `mvex` box holding one zeroed `trex` for `track_id` to the
    /// `moov` box inside a generated file.
    fn inject_mvex(bytes: &mut Vec<u8>, track_id: u32) {
        let moov_position = bytes
            .windows(4)
            .position(|window| window == b"moov")
            .expect("fixture must contain a moov box");
        let size_position = moov_position - 4;
        let moov_size = usize::try_from(u32::from_be_bytes(
            bytes[size_position..size_position + 4]
                .try_into()
                .expect("moov size"),
        ))
        .expect("moov size fits usize");
        let mut mvex = Vec::with_capacity(40);
        mvex.extend_from_slice(&40_u32.to_be_bytes());
        mvex.extend_from_slice(b"mvex");
        mvex.extend_from_slice(&32_u32.to_be_bytes());
        mvex.extend_from_slice(b"trex");
        mvex.extend_from_slice(&0_u32.to_be_bytes());
        mvex.extend_from_slice(&track_id.to_be_bytes());
        mvex.extend_from_slice(&1_u32.to_be_bytes());
        mvex.extend_from_slice(&0_u32.to_be_bytes());
        mvex.extend_from_slice(&0_u32.to_be_bytes());
        mvex.extend_from_slice(&0_u32.to_be_bytes());
        let body_end = size_position + moov_size;
        bytes.splice(body_end..body_end, mvex);
        let new_size = u32::try_from(moov_size + 40).expect("moov fits in u32");
        bytes[size_position..size_position + 4].copy_from_slice(&new_size.to_be_bytes());

        // The injected box pushes `mdat` back by 40 bytes, so every chunk
        // offset declared in `stco`/`co64` moves with it.
        for marker in [b"stco", b"co64"] {
            let mut offset = 0;
            while let Some(position) = bytes[offset..]
                .windows(4)
                .position(|window| window == marker.as_slice())
            {
                let table = offset + position + 4;
                let entry_count = u32::from_be_bytes(
                    bytes[table + 4..table + 8].try_into().expect("entry count"),
                ) as usize;
                let entry_width = if marker == b"stco" { 4 } else { 8 };
                for entry in 0..entry_count {
                    let start = table + 8 + entry * entry_width;
                    let range = start..start + entry_width;
                    let value = match entry_width {
                        4 => u64::from(u32::from_be_bytes(
                            bytes[range.clone()].try_into().expect("stco entry"),
                        )),
                        _ => {
                            u64::from_be_bytes(bytes[range.clone()].try_into().expect("co64 entry"))
                        }
                    };
                    bytes[range].copy_from_slice(&(value + 40).to_be_bytes()[8 - entry_width..]);
                }
                offset = table;
            }
        }
    }

    /// A `moov` that declares one stbl sample followed by a `moof` fragment
    /// without `tfdt`, so the fragment's decode timeline continues from the
    /// initial sample tables.
    fn mixed_stbl_fragment_fixture() -> NamedTempFile {
        let media = Media::new(
            vec![Track::new(
                video_spec(TRACK_ID),
                vec![Sample::new(vec![0xE8; 4], 1_000, true, 0)],
            )],
            TIMESCALE,
        );
        let mut bytes = ProgressiveMux::new(true)
            .package(&media)
            .expect("progressive fixture must mux");
        inject_mvex(&mut bytes, TRACK_ID);

        bytes.extend_from_slice(&media_segment(
            1,
            traf(
                TFHD_DEFAULT_BASE_IS_MOOF | TFHD_DEFAULT_SAMPLE_DURATION_PRESENT,
                None,
                Some(1_000),
                None,
                None,
                vec![trun(
                    0,
                    TRUN_DATA_OFFSET_PRESENT | TRUN_SAMPLE_SIZE_PRESENT | TRUN_SAMPLE_FLAGS_PRESENT,
                    None,
                    vec![sample(None, Some(4), Some(SYNC_FLAGS), None)],
                )],
            ),
            &[0xE9; 4],
        ));
        write_fixture(&bytes)
    }

    /// One fragment with a version-0 `trun` (unsigned composition offsets)
    /// and a version-1 `trun` (signed) sharing a `tfdt` decode base.
    fn composition_offset_version_fixture() -> NamedTempFile {
        let mut bytes = build_init_segment(&[video_spec(TRACK_ID)], TIMESCALE)
            .expect("init segment must serialize");
        bytes.extend_from_slice(&media_segment(
            1,
            traf(
                TFHD_DEFAULT_BASE_IS_MOOF
                    | TFHD_DEFAULT_SAMPLE_DURATION_PRESENT
                    | TFHD_DEFAULT_SAMPLE_FLAGS_PRESENT,
                Some(TrackFragmentBaseMediaDecodeTimeBox::new_v1(1_000)),
                Some(500),
                None,
                Some(NON_SYNC_FLAGS),
                vec![
                    trun(
                        0,
                        TRUN_DATA_OFFSET_PRESENT
                            | TRUN_SAMPLE_SIZE_PRESENT
                            | TRUN_SAMPLE_COMPOSITION_TIME_OFFSET_PRESENT,
                        None,
                        vec![sample(None, Some(4), None, Some(-2_147_483_648))],
                    ),
                    trun(
                        1,
                        TRUN_SAMPLE_SIZE_PRESENT | TRUN_SAMPLE_COMPOSITION_TIME_OFFSET_PRESENT,
                        None,
                        vec![sample(None, Some(4), None, Some(-50))],
                    ),
                ],
            ),
            &[0xC7; 4].into_iter().chain([0xC8; 4]).collect::<Vec<_>>(),
        ));
        write_fixture(&bytes)
    }

    #[test]
    fn fragmented_reader_respects_fragment_timing_flags_and_locations() {
        let file = fragmented_fixture();
        let mut reader = VideoReader::open(file.path()).expect("fragmented file must open");

        assert_eq!(reader.sample_count(), 7);
        assert_eq!(reader.duration(), Some(std::time::Duration::from_secs(56)));

        let expected = [
            (48_000_u64, 50_000_u64, 1_000_u32, false, 0xA0_u8),
            (49_000, 49_000, 1_000, true, 0xA1),
            (50_000, 50_000, 1_500, true, 0xB0),
            (51_500, 51_500, 1_500, false, 0xB1),
            (53_000, 53_000, 1_000, false, 0xB2),
            (54_000, 54_000, 1_000, true, 0xC0),
            (55_000, 55_000, 1_000, false, 0xC1),
        ];
        for (index, &(decode_time, pts, duration, is_keyframe, byte)) in expected.iter().enumerate()
        {
            let (meta_pts, meta_duration, meta_keyframe) = reader
                .sample_info(index)
                .expect("sample metadata must exist");
            assert_eq!(
                (meta_pts, meta_duration, meta_keyframe),
                (pts, duration, is_keyframe)
            );
            let (data, read_pts, read_keyframe) = reader
                .read_sample()
                .expect("sample must read")
                .expect("sample must exist");
            assert_eq!(data, vec![byte; 4]);
            assert_eq!((read_pts, read_keyframe), (pts, is_keyframe));
            let _ = decode_time;
        }
        assert!(reader.read_sample().expect("eof read").is_none());
    }

    #[test]
    fn fragmented_reader_seeks_by_sample_index() {
        let file = fragmented_fixture();
        let mut reader = VideoReader::open(file.path()).expect("fragmented file must open");

        assert_eq!(reader.nearest_keyframe_at_or_before(4), 2);
        reader.seek_to_sample(4);
        let (data, pts, is_keyframe) = reader
            .read_sample()
            .expect("sample must read")
            .expect("sample must exist");
        assert_eq!((data, pts, is_keyframe), (vec![0xB2; 4], 53_000, false));
    }

    #[test]
    fn fragmented_reader_falls_back_to_trex_defaults() {
        let file = trex_defaults_fixture();
        let mut reader = VideoReader::open(file.path()).expect("fragmented file must open");

        assert_eq!(reader.sample_count(), 2);
        let expected = [(3_000_u64, true, 0xD0_u8), (3_900, false, 0xD1)];
        for (index, &(pts, is_keyframe, byte)) in expected.iter().enumerate() {
            let (meta_pts, meta_duration, meta_keyframe) = reader
                .sample_info(index)
                .expect("sample metadata must exist");
            assert_eq!(
                (meta_pts, meta_duration, meta_keyframe),
                (pts, 900, is_keyframe)
            );
            let (data, read_pts, read_keyframe) = reader
                .read_sample()
                .expect("sample must read")
                .expect("sample must exist");
            assert_eq!(data, vec![byte; 4]);
            assert_eq!((read_pts, read_keyframe), (pts, is_keyframe));
        }
    }

    #[test]
    fn progressive_reader_keeps_sample_table_timing() {
        let media = Media::new(
            vec![Track::new(
                video_spec(TRACK_ID),
                vec![
                    Sample::new(vec![0xE0; 4], 1_000, true, 0),
                    Sample::new(vec![0xE1; 4], 1_000, false, 500),
                    Sample::new(vec![0xE2; 4], 1_000, false, 1_000),
                ],
            )],
            TIMESCALE,
        );
        let bytes = ProgressiveMux::new(true)
            .package(&media)
            .expect("progressive fixture must mux");
        let file = write_fixture(&bytes);

        let mut reader = VideoReader::open(file.path()).expect("progressive file must open");
        assert_eq!(reader.sample_count(), 3);
        let expected = [
            (0_u64, 1_000_u32, true, 0xE0_u8),
            (1_500, 1_000, false, 0xE1),
            (3_000, 1_000, false, 0xE2),
        ];
        for (index, &(pts, duration, is_keyframe, byte)) in expected.iter().enumerate() {
            let (meta_pts, meta_duration, meta_keyframe) = reader
                .sample_info(index)
                .expect("sample metadata must exist");
            assert_eq!(
                (meta_pts, meta_duration, meta_keyframe),
                (pts, duration, is_keyframe)
            );
            let (data, read_pts, read_keyframe) = reader
                .read_sample()
                .expect("sample must read")
                .expect("sample must exist");
            assert_eq!(data, vec![byte; 4]);
            assert_eq!((read_pts, read_keyframe), (pts, is_keyframe));
        }
    }

    #[test]
    fn fragmented_reader_resolves_implicit_bases_per_moof() {
        let file = implicit_base_fixture();
        let mut reader = VideoReader::open(file.path()).expect("fragmented file must open");

        assert_eq!(reader.sample_count(), 3);
        // First moof has no tfdt (decode starts at zero); moof two's first
        // traf resets to 5000 via tfdt, and its second traf continues the
        // decode timeline while its bytes chain inside the same mdat.
        let expected = [
            (0_u64, 0_u64, 1_000_u32, true, 0xF0_u8),
            (5_000, 5_000, 1_000, false, 0xF1),
            (6_000, 6_000, 1_000, true, 0xF2),
        ];
        for (index, &(decode_time, pts, duration, is_keyframe, byte)) in expected.iter().enumerate()
        {
            let (meta_pts, meta_duration, meta_keyframe) = reader
                .sample_info(index)
                .expect("sample metadata must exist");
            assert_eq!(
                (meta_pts, meta_duration, meta_keyframe),
                (pts, duration, is_keyframe)
            );
            let (data, read_pts, read_keyframe) = reader
                .read_sample()
                .expect("sample must read")
                .expect("sample must exist");
            assert_eq!(data, vec![byte; 4]);
            assert_eq!((read_pts, read_keyframe), (pts, is_keyframe));
            let _ = decode_time;
        }
    }

    #[test]
    fn fragmented_reader_indexes_initial_samples_before_fragments() {
        let file = mixed_stbl_fragment_fixture();
        let mut reader = VideoReader::open(file.path()).expect("fragmented file must open");

        assert_eq!(reader.sample_count(), 2);
        let expected = [(0_u64, true, 0xE8_u8), (1_000, true, 0xE9)];
        for (index, &(pts, is_keyframe, byte)) in expected.iter().enumerate() {
            let (meta_pts, meta_duration, meta_keyframe) = reader
                .sample_info(index)
                .expect("sample metadata must exist");
            assert_eq!(
                (meta_pts, meta_duration, meta_keyframe),
                (pts, 1_000, is_keyframe)
            );
            let (data, read_pts, read_keyframe) = reader
                .read_sample()
                .expect("sample must read")
                .expect("sample must exist");
            assert_eq!(data, vec![byte; 4]);
            assert_eq!((read_pts, read_keyframe), (pts, is_keyframe));
        }
    }

    #[test]
    fn fragmented_reader_decodes_composition_offsets_by_trun_version() {
        let file = composition_offset_version_fixture();
        let mut reader = VideoReader::open(file.path()).expect("fragmented file must open");

        assert_eq!(reader.sample_count(), 2);
        // Version 0 trun stores an unsigned u32 (0x8000_0000 widens to
        // 2147483648, not a negative offset); version 1 stores a signed i32.
        let expected = [(2_147_484_648_u64, 0xC7_u8), (1_450, 0xC8)];
        for (index, &(pts, byte)) in expected.iter().enumerate() {
            let (meta_pts, _, _) = reader
                .sample_info(index)
                .expect("sample metadata must exist");
            assert_eq!(meta_pts, pts);
            let (data, read_pts, _) = reader
                .read_sample()
                .expect("sample must read")
                .expect("sample must exist");
            assert_eq!(data, vec![byte; 4]);
            assert_eq!(read_pts, pts);
        }
    }

    #[test]
    fn progressive_reader_seeks_by_pts_across_decode_reordering() {
        let file = fragmented_fixture();
        let mut reader =
            crate::progressive::ProgressiveTrackReader::open(file.path(), crate::TrackKind::Video)
                .expect("fragmented file must open");

        // Decode-order PTS is 50000, 49000, 50000, ... so a target of 49 s
        // must land on the reordered sample (decode index 1), not index 0.
        let landed = reader
            .seek_to(std::time::Duration::from_secs(49))
            .expect("seek must succeed");
        assert_eq!(landed, std::time::Duration::from_secs(49));
        let sample = reader
            .read_sample()
            .expect("sample must read")
            .expect("sample must exist");
        assert_eq!(sample.data()[..], [0xA1; 4]);
        assert_eq!(sample.presentation_time().ticks(), 49_000);
        assert!(sample.is_keyframe());
    }

    #[test]
    fn progressive_reader_seeks_keyframe_by_pts_across_decode_reordering() {
        let file = fragmented_fixture();
        let mut reader =
            crate::progressive::ProgressiveTrackReader::open(file.path(), crate::TrackKind::Video)
                .expect("fragmented file must open");

        // The only sample presented at or before 49 s is decode index 1
        // (a keyframe); decode index 0 presents at 50 s and is not a sync
        // sample, so it must not be selected.
        let landed = reader
            .seek_to_keyframe(std::time::Duration::from_secs(49))
            .expect("seek must succeed");
        assert_eq!(landed, std::time::Duration::from_secs(49));
        let sample = reader
            .read_sample()
            .expect("sample must read")
            .expect("sample must exist");
        assert_eq!(sample.data()[..], [0xA1; 4]);
        assert_eq!(sample.presentation_time().ticks(), 49_000);
        assert!(sample.is_keyframe());
    }
}
