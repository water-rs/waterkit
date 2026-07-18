//! Incremental elementary-track access for progressive MP4 and MOV files.

use std::collections::BTreeMap;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use broadcast_common::Unpackage as _;
use transmux::Fmp4Demux;
use waterkit_video_core::Error;

use crate::demuxer::{SampleMeta, build_sample_metas};
use crate::isobmff::read_top_level_box;
use crate::stream::{
    EncodedSample, MediaTime, TrackId, TrackInfo, TrackKind, track_info_from_spec,
};

/// Metadata for one selectable elementary track in a progressive MP4 presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressiveTrack {
    info: TrackInfo,
    language: String,
    duration: Duration,
    sample_count: u32,
}

impl ProgressiveTrack {
    /// Returns the decoder configuration and stable container track identity.
    #[must_use]
    pub const fn info(&self) -> &TrackInfo {
        &self.info
    }

    /// Returns the ISO 639 language code declared by the media header.
    #[must_use]
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Returns the presentation duration of this track.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.duration
    }

    /// Returns the number of encoded access units in this track.
    #[must_use]
    pub const fn sample_count(&self) -> u32 {
        self.sample_count
    }
}

/// Incremental, seekable elementary-track reader for progressive MP4 and MOV files.
///
/// Only the `moov` metadata box is materialized. Encoded access units are read
/// on demand through the container's sample tables, so opening a large video
/// never copies its media payload into memory.
pub struct ProgressiveTrackReader {
    reader: mp4::Mp4Reader<BufReader<std::fs::File>>,
    kind: TrackKind,
    tracks: Vec<ProgressiveTrack>,
    sample_metadata: BTreeMap<TrackId, Vec<SampleMeta>>,
    selected_track: Option<usize>,
    sample_index: usize,
    discontinuity: bool,
}

impl std::fmt::Debug for ProgressiveTrackReader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProgressiveTrackReader")
            .field("kind", &self.kind)
            .field("tracks", &self.tracks)
            .field("selected_track", &self.selected_track)
            .field("sample_index", &self.sample_index)
            .finish_non_exhaustive()
    }
}

impl ProgressiveTrackReader {
    /// Opens a progressive MP4 or MOV and discovers supported tracks of `kind`.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed ISO BMFF metadata or a sample table
    /// whose codec parameters cannot be represented by the `WaterKit` contract.
    pub fn open(path: impl AsRef<Path>, kind: TrackKind) -> Result<Self, Error> {
        let mp4_kind = match kind {
            TrackKind::Audio => mp4::TrackType::Audio,
            TrackKind::Video => mp4::TrackType::Video,
            TrackKind::Subtitle | TrackKind::Metadata => {
                return Err(Error::Unsupported(format!(
                    "progressive {kind:?} sample reading is not defined"
                )));
            }
        };
        let path = path.as_ref();
        let movie = read_top_level_box(path, *b"moov")?
            .ok_or_else(|| Error::Container(String::from("progressive media has no moov box")))?;
        let media = Fmp4Demux::new()
            .unpackage(&movie)
            .map_err(|error| Error::Container(error.to_string()))?;
        let configured_tracks = media
            .tracks
            .iter()
            .map(|track| track_info_from_spec(&track.spec).map(|info| (info.id(), info)))
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        let file = std::fs::File::open(path)?;
        let file_len = file.metadata()?.len();
        let reader = mp4::Mp4Reader::read_header(BufReader::new(file), file_len)
            .map_err(|error| Error::Container(error.to_string()))?;
        let mut track_ids = reader
            .tracks()
            .values()
            .filter_map(|track| {
                track
                    .track_type()
                    .ok()
                    .filter(|candidate| *candidate == mp4_kind)
                    .map(|_| track.track_id())
            })
            .collect::<Vec<_>>();
        track_ids.sort_unstable();

        let mut tracks = Vec::with_capacity(track_ids.len());
        let mut sample_metadata = BTreeMap::new();
        for track_id in track_ids {
            let id = TrackId::new(track_id)?;
            let info = configured_tracks.get(&id).cloned().ok_or_else(|| {
                Error::Unsupported(format!(
                    "MP4 {kind:?} track {track_id} has no supported decoder configuration"
                ))
            })?;
            if info.kind() != kind {
                return Err(Error::Container(format!(
                    "MP4 track {track_id} is marked as {kind:?} but its decoder configuration is {:?}",
                    info.kind()
                )));
            }
            let mp4_track = reader.tracks().get(&track_id).ok_or_else(|| {
                Error::Container(format!(
                    "MP4 {kind:?} track {track_id} disappeared after probing"
                ))
            })?;
            let sample_count = mp4_track.sample_count();
            let metadata = build_sample_metas(mp4_track, sample_count)?;
            let presentation_end = metadata.iter().fold(0_u64, |end, sample| {
                end.max(
                    sample
                        .presentation_time
                        .saturating_add(u64::from(sample.duration)),
                )
            });
            let duration = media_duration(presentation_end, info.timescale());
            tracks.push(ProgressiveTrack {
                info,
                language: mp4_track.language().to_owned(),
                duration,
                sample_count,
            });
            sample_metadata.insert(id, metadata);
        }

        Ok(Self {
            reader,
            kind,
            selected_track: (!tracks.is_empty()).then_some(0),
            tracks,
            sample_metadata,
            sample_index: 0,
            discontinuity: false,
        })
    }

    /// Returns the supported tracks in stable MP4 track-id order.
    #[must_use]
    pub fn tracks(&self) -> &[ProgressiveTrack] {
        &self.tracks
    }

    /// Selects one track by zero-based index and resets its sample cursor.
    ///
    /// # Errors
    ///
    /// Returns an error when `index` is outside the discovered track list.
    pub fn select_track(&mut self, index: usize) -> Result<(), Error> {
        if index >= self.tracks.len() {
            return Err(Error::Container(format!(
                "progressive {:?} track {index} is outside the presentation's {} tracks",
                self.kind,
                self.tracks.len()
            )));
        }
        self.selected_track = Some(index);
        self.sample_index = 0;
        self.discontinuity = false;
        Ok(())
    }

    /// Returns the selected track, or `None` when no track of this kind exists.
    ///
    /// # Panics
    ///
    /// Panics if internal track selection no longer refers to the immutable
    /// track list established while opening the reader.
    #[must_use]
    pub fn selected_track(&self) -> Option<&ProgressiveTrack> {
        self.selected_track.map(|index| &self.tracks[index])
    }

    /// Seeks the selected track to its first access unit at or after `position`.
    ///
    /// The next returned sample is marked discontinuous so a persistent decoder
    /// can flush state before accepting it.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected track metadata is inconsistent.
    pub fn seek_to(&mut self, position: Duration) -> Result<Duration, Error> {
        let Some(track) = self.selected_track() else {
            return Ok(Duration::ZERO);
        };
        let track_id = track.info.id();
        let timescale = track.info.timescale();
        let track_duration = track.duration;
        let target_ticks = duration_to_ticks(position, timescale)?;
        let metadata = self.sample_metadata.get(&track_id).ok_or_else(|| {
            Error::Container(format!(
                "progressive {:?} track {} has no sample metadata",
                self.kind,
                track_id.get()
            ))
        })?;
        self.sample_index =
            metadata.partition_point(|sample| sample.presentation_time < target_ticks);
        self.discontinuity = self.sample_index < metadata.len();
        Ok(metadata
            .get(self.sample_index)
            .map_or(track_duration, |sample| {
                media_duration(sample.presentation_time, timescale)
            }))
    }

    /// Seeks a video track to the closest preceding random-access sample.
    ///
    /// # Errors
    ///
    /// Returns an error when this reader does not own video or its sample
    /// metadata is inconsistent.
    pub fn seek_to_keyframe(&mut self, position: Duration) -> Result<Duration, Error> {
        if self.kind != TrackKind::Video {
            return Err(Error::Container(format!(
                "keyframe seek requires a video reader, got {:?}",
                self.kind
            )));
        }
        let Some(track) = self.selected_track() else {
            return Err(Error::Container(String::from(
                "keyframe seek requires a selected video track",
            )));
        };
        let track_id = track.info.id();
        let timescale = track.info.timescale();
        let track_duration = track.duration;
        let target_ticks = duration_to_ticks(position, timescale)?;
        let metadata = self.sample_metadata.get(&track_id).ok_or_else(|| {
            Error::Container(format!(
                "progressive video track {} has no sample metadata",
                track_id.get()
            ))
        })?;
        let after = metadata.partition_point(|sample| sample.presentation_time <= target_ticks);
        self.sample_index = metadata[..after]
            .iter()
            .rposition(|sample| sample.is_keyframe)
            .unwrap_or(0);
        self.discontinuity = self.sample_index < metadata.len();
        Ok(metadata
            .get(self.sample_index)
            .map_or(track_duration, |sample| {
                media_duration(sample.presentation_time, timescale)
            }))
    }

    /// Reads the next encoded access unit from the selected track.
    ///
    /// # Errors
    ///
    /// Returns an error when the MP4 sample table cannot resolve the payload.
    pub fn read_sample(&mut self) -> Result<Option<EncodedSample>, Error> {
        let Some(track_index) = self.selected_track else {
            return Ok(None);
        };
        let track = self.tracks.get(track_index).ok_or_else(|| {
            Error::Container(String::from(
                "selected progressive track index became invalid",
            ))
        })?;
        let track_id = track.info.id();
        let timescale = track.info.timescale();
        let metadata = self.sample_metadata.get(&track_id).ok_or_else(|| {
            Error::Container(format!(
                "progressive {:?} track {} has no sample metadata",
                self.kind,
                track_id.get()
            ))
        })?;
        let Some(sample_metadata) = metadata.get(self.sample_index).copied() else {
            return Ok(None);
        };
        let sample_id = u32::try_from(self.sample_index)
            .map_err(|_| Error::Container(String::from("MP4 sample index exceeds u32")))?
            .checked_add(1)
            .ok_or_else(|| Error::Container(String::from("MP4 sample id exceeds u32")))?;
        let sample = self
            .reader
            .read_sample(track_id.get(), sample_id)
            .map_err(|error| Error::Container(error.to_string()))?
            .ok_or_else(|| {
                Error::Container(format!(
                    "MP4 {:?} track {} is missing declared sample {sample_id}",
                    self.kind,
                    track_id.get()
                ))
            })?;
        self.sample_index = self.sample_index.saturating_add(1);
        let encoded = EncodedSample::new(
            track_id,
            MediaTime::new(
                i64::try_from(sample_metadata.decode_time).map_err(|_| {
                    Error::Container(String::from("MP4 decode timestamp exceeds i64"))
                })?,
                timescale,
            ),
            MediaTime::new(
                i64::try_from(sample_metadata.presentation_time).map_err(|_| {
                    Error::Container(String::from("MP4 presentation timestamp exceeds i64"))
                })?,
                timescale,
            ),
            MediaTime::new(i64::from(sample_metadata.duration), timescale),
            sample_metadata.is_keyframe,
            sample.bytes,
        )
        .with_discontinuity(std::mem::take(&mut self.discontinuity));
        Ok(Some(encoded))
    }
}

fn media_duration(ticks: u64, timescale: std::num::NonZeroU32) -> Duration {
    Duration::from_nanos(ticks.saturating_mul(1_000_000_000) / u64::from(timescale.get()))
}

fn duration_to_ticks(duration: Duration, timescale: std::num::NonZeroU32) -> Result<u64, Error> {
    let ticks = duration
        .as_nanos()
        .checked_mul(u128::from(timescale.get()))
        .ok_or_else(|| Error::Container(String::from("media seek timestamp overflow")))?
        / 1_000_000_000;
    u64::try_from(ticks)
        .map_err(|_| Error::Container(String::from("media seek timestamp exceeds u64 ticks")))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::time::Duration;

    use broadcast_common::Package as _;
    use tempfile::NamedTempFile;
    use transmux::{
        AVCConfigurationBox, AVCDecoderConfigurationRecord, AvcPps, AvcSps, CodecConfig,
        DecoderConfigDescriptor, DecoderSpecificInfo, ESDescriptor, EsdsBox, Media,
        ObjectTypeIndication, ProgressiveMux, SLConfigDescriptor, Sample, StreamType, Track,
        TrackSpec,
    };

    use super::ProgressiveTrackReader;
    use crate::TrackKind;

    #[test]
    fn reads_and_selects_progressive_audio_without_materializing_media_payloads() {
        let media = Media::new(
            vec![
                audio_track(2, vec![0x11, 0x22]),
                audio_track(3, vec![0x33, 0x44]),
            ],
            1_000,
        );
        let bytes = ProgressiveMux::new(true)
            .package(&media)
            .expect("progressive audio fixture must mux");
        let mut file = NamedTempFile::new().expect("temporary media file must open");
        file.write_all(&bytes)
            .expect("temporary media fixture must write");

        let mut reader = ProgressiveTrackReader::open(file.path(), TrackKind::Audio)
            .expect("progressive audio fixture must open");
        assert_eq!(reader.tracks().len(), 2);
        assert_eq!(reader.tracks()[0].info().id().get(), 2);
        assert_eq!(reader.tracks()[1].info().id().get(), 3);

        reader
            .select_track(1)
            .expect("second audio track must select");
        let first = reader
            .read_sample()
            .expect("selected sample must read")
            .expect("selected track must contain a sample");
        assert_eq!(first.track_id().get(), 3);
        assert_eq!(first.data().as_ref(), [0x33, 0x44]);
        assert!(!first.is_discontinuity());

        let seeked = reader
            .seek_to(Duration::ZERO)
            .expect("audio seek must succeed");
        assert_eq!(seeked, Duration::ZERO);
        assert!(
            reader
                .read_sample()
                .expect("sample after seek must read")
                .expect("sample after seek must exist")
                .is_discontinuity()
        );
    }

    #[test]
    fn reads_progressive_video_and_seeks_to_preceding_keyframe() {
        let media = Media::new(
            vec![video_track(
                7,
                vec![
                    Sample::new(vec![0x01], 1_000, true, 0),
                    Sample::new(vec![0x02], 1_000, false, 0),
                    Sample::new(vec![0x03], 1_000, true, 0),
                ],
            )],
            1_000,
        );
        let bytes = ProgressiveMux::new(true)
            .package(&media)
            .expect("progressive video fixture must mux");
        let mut file = NamedTempFile::new().expect("temporary media file must open");
        file.write_all(&bytes)
            .expect("temporary media fixture must write");

        let mut reader = ProgressiveTrackReader::open(file.path(), TrackKind::Video)
            .expect("progressive video fixture must open");
        assert_eq!(reader.tracks()[0].info().id().get(), 7);
        assert_eq!(
            reader.tracks()[0]
                .info()
                .video_dimensions()
                .expect("video track must retain dimensions")
                .width
                .get(),
            1_920
        );

        let resolved = reader
            .seek_to_keyframe(Duration::from_millis(1_500))
            .expect("video keyframe seek must succeed");
        assert_eq!(resolved, Duration::ZERO);
        let sample = reader
            .read_sample()
            .expect("keyframe sample must read")
            .expect("keyframe sample must exist");
        assert!(sample.is_keyframe());
        assert!(sample.is_discontinuity());
        assert_eq!(sample.data().as_ref(), [0x01]);

        let resolved = reader
            .seek_to_keyframe(Duration::from_millis(2_500))
            .expect("later keyframe seek must succeed");
        assert_eq!(resolved, Duration::from_secs(2));
        assert_eq!(
            reader
                .read_sample()
                .expect("later keyframe sample must read")
                .expect("later keyframe sample must exist")
                .data()
                .as_ref(),
            [0x03]
        );
    }

    fn audio_track(track_id: u32, data: Vec<u8>) -> Track {
        let spec = TrackSpec::new(
            track_id,
            48_000,
            CodecConfig::Aac {
                esds: EsdsBox {
                    es_descriptor: ESDescriptor {
                        es_id: u16::try_from(track_id).expect("test track id must fit u16"),
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
                            max_bitrate: 128_000,
                            avg_bitrate: 128_000,
                            decoder_specific_info: Some(DecoderSpecificInfo {
                                data: vec![0x11, 0x90],
                            }),
                        }),
                        sl_config: Some(SLConfigDescriptor { body: vec![0x02] }),
                    },
                },
                channel_count: 2,
                sample_rate: 48_000,
                sample_size: 16,
            },
        );
        Track::new(spec, vec![Sample::new(data, 1_024, true, 0)])
    }

    fn video_track(track_id: u32, samples: Vec<Sample>) -> Track {
        let spec = TrackSpec::new(
            track_id,
            1_000,
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
        );
        Track::new(spec, samples)
    }
}
