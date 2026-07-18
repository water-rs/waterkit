use std::{collections::BTreeMap, num::NonZeroU64, time::Duration};

use quick_m3u8::{
    HlsLine, Reader,
    config::ParsingOptions,
    tag::{
        KnownTag,
        hls::{self as quick_hls, EnumeratedString, Method, PreloadHintType},
    },
};
use url::Url;
use waterkit_video_core::Error;

use crate::{MediaByteRange, MediaRequest, StreamVariant, fetch_media};

/// Parsed HTTP Live Streaming playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HlsPlaylist {
    /// Multivariant playlist containing bitrate and rendition choices.
    Master(HlsMasterPlaylist),
    /// Media playlist containing ordered media segments.
    Media(Box<HlsMediaPlaylist>),
}

/// Parsed HLS multivariant playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsMasterPlaylist {
    /// Regular audio/video variants available for playback.
    pub variants: Vec<StreamVariant>,
    /// I-frame-only variants available for scrubbing and trick play.
    pub i_frame_variants: Vec<StreamVariant>,
    /// Alternate audio, video, subtitle, and closed-caption renditions.
    pub renditions: Vec<HlsRendition>,
    /// Whether every segment starts with independently decodable media.
    pub independent_segments: bool,
}

/// Kind of one alternate HLS rendition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HlsRenditionKind {
    /// Alternate audio rendition.
    Audio,
    /// Alternate video rendition.
    Video,
    /// `WebVTT` or `IMSC` subtitle rendition.
    Subtitles,
    /// In-band closed captions.
    ClosedCaptions,
}

/// One alternate rendition declared by `EXT-X-MEDIA`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsRendition {
    /// Rendition kind.
    pub kind: HlsRenditionKind,
    /// Resolved rendition playlist URL, absent for in-band renditions.
    pub url: Option<Url>,
    /// Group identifier referenced by variants.
    pub group_id: String,
    /// Human-readable rendition name.
    pub name: String,
    /// BCP-47 language tag when supplied.
    pub language: Option<String>,
    /// Whether this rendition is the default choice.
    pub is_default: bool,
    /// Whether automatic selection is permitted.
    pub is_autoselect: bool,
    /// Whether the rendition contains forced subtitles.
    pub is_forced: bool,
}

/// Parsed HLS media playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsMediaPlaylist {
    /// Maximum declared media-segment duration.
    pub target_duration: Duration,
    /// Sequence number of the first listed segment.
    pub media_sequence: usize,
    /// Discontinuity sequence of the first listed segment.
    pub discontinuity_sequence: usize,
    /// Ordered media segments.
    pub segments: Vec<HlsSegment>,
    /// Whether no further segments will be appended.
    pub ended: bool,
    /// Whether every segment is independently decodable.
    pub independent_segments: bool,
    /// Delta-update metadata when `EXT-X-SKIP` omitted older segments.
    pub delta_update: Option<HlsDeltaUpdate>,
    /// Origin reload and target-latency policy.
    pub server_control: Option<HlsServerControl>,
    /// Low-latency controls and currently published partial segments.
    pub low_latency: Option<HlsLowLatency>,
}

/// One HLS delta playlist update declared by `EXT-X-SKIP`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsDeltaUpdate {
    /// Number of media segments omitted from the beginning of the update.
    pub skipped_segments: usize,
    /// Recently removed date-range identifiers, exactly as declared by the origin.
    pub recently_removed_dateranges: Option<String>,
}

/// Server capabilities and latency policy declared by `EXT-X-SERVER-CONTROL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HlsServerControl {
    /// Maximum duration an origin may omit from a delta update.
    pub can_skip_until: Option<Duration>,
    /// Whether delta updates may omit recently removed date ranges.
    pub can_skip_dateranges: bool,
    /// Recommended distance from the live edge for complete-segment playback.
    pub hold_back: Option<Duration>,
    /// Recommended distance from the live edge for partial-segment playback.
    pub part_hold_back: Option<Duration>,
    /// Whether the origin accepts blocking playlist reload queries.
    pub can_block_reload: bool,
}

/// Low-Latency HLS state attached to one media-playlist snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsLowLatency {
    /// Maximum duration of one partial segment.
    pub part_target: Duration,
    /// Published parts for the in-progress media sequence.
    pub trailing_parts: Vec<HlsPartialSegment>,
    /// Resource the origin expects to publish next, when declared.
    pub preload_hint: Option<HlsPreloadHint>,
    /// Latest positions reported for sibling renditions.
    pub rendition_reports: Vec<HlsRenditionReport>,
}

/// One independently addressable Low-Latency HLS partial segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsPartialSegment {
    /// Media sequence containing this part.
    pub sequence: usize,
    /// Zero-based part index within the media sequence.
    pub part_index: usize,
    /// Resolved partial-segment URL.
    pub url: Url,
    /// Declared part duration.
    pub duration: Duration,
    /// Optional byte range within the partial-segment resource.
    pub byte_range: Option<HlsSegmentRange>,
    /// Active Media Initialization Section.
    pub initialization: Option<HlsInitializationSegment>,
    /// Active encryption/key descriptions.
    pub encryption: Vec<HlsEncryption>,
    /// Whether the part begins with an independently decodable sample.
    pub independent: bool,
    /// Whether the part begins a timeline or codec discontinuity.
    pub discontinuity: bool,
    /// Whether the part is intentionally absent and only advances time.
    pub gap: bool,
}

/// One `EXT-X-PRELOAD-HINT` for a future partial segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsPreloadHint {
    /// Kind of resource expected at the hinted URL.
    pub kind: HlsPreloadHintKind,
    /// Resolved resource URL.
    pub url: Url,
    /// Optional known byte range within the resource.
    pub byte_range: Option<HlsSegmentRange>,
}

/// Resource kind declared by an HLS preload hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HlsPreloadHintKind {
    /// Future partial media segment.
    Part,
    /// Future Media Initialization Section.
    Map,
}

/// Last published position for one sibling rendition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsRenditionReport {
    /// Resolved rendition playlist URL.
    pub url: Url,
    /// Last media sequence available in that rendition.
    pub last_sequence: usize,
    /// Last part index available within `last_sequence`.
    pub last_part: Option<usize>,
}

/// One half-open byte range inside an HLS resource.
pub type HlsSegmentRange = MediaByteRange;

/// HLS segment encryption method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HlsEncryptionMethod {
    /// Whole-segment AES-128 CBC encryption.
    Aes128,
    /// Sample-level HLS encryption.
    SampleAes,
    /// Sample-level Common Encryption using AES-CTR.
    SampleAesCtr,
}

/// One encryption/key option attached to an HLS segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsEncryption {
    /// Encryption method.
    pub method: HlsEncryptionMethod,
    /// Resolved key or license URL.
    pub key_url: Url,
    /// Explicit initialization vector; absent means derive it from sequence.
    pub initialization_vector: Option<[u8; 16]>,
    /// HLS key format identifier, such as identity, `FairPlay`, `Widevine`, or `PlayReady`.
    pub key_format: String,
}

/// Media Initialization Section referenced by an HLS media segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsInitializationSegment {
    /// Resolved initialization resource URL.
    pub url: Url,
    /// Optional byte range within the resource.
    pub byte_range: Option<HlsSegmentRange>,
}

/// One ordered HLS media segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsSegment {
    /// Media sequence number.
    pub sequence: usize,
    /// Resolved media URL.
    pub url: Url,
    /// Segment duration on the media timeline.
    pub duration: Duration,
    /// Optional byte range within the resource.
    pub byte_range: Option<HlsSegmentRange>,
    /// Optional Media Initialization Section for fragmented MP4.
    pub initialization: Option<HlsInitializationSegment>,
    /// Available encryption/key descriptions.
    pub encryption: Vec<HlsEncryption>,
    /// Whether this segment starts a timeline/codec discontinuity.
    pub discontinuity: bool,
    /// Whether the segment is intentionally absent and only advances time.
    pub gap: bool,
}

/// Parses an HLS multivariant or media playlist and resolves every relative URL.
///
/// # Errors
///
/// Returns a streaming error for malformed manifests, invalid relative URLs,
/// zero bandwidth variants, overflowing dimensions/ranges, or ambiguous
/// implicit byte ranges.
pub fn parse_hls_playlist(base_url: &Url, input: &str) -> Result<HlsPlaylist, Error> {
    if input.lines().any(|line| {
        line.starts_with("#EXT-X-STREAM-INF:")
            || line.starts_with("#EXT-X-I-FRAME-STREAM-INF:")
            || line.starts_with("#EXT-X-MEDIA:")
    }) {
        return parse_master(base_url, input).map(HlsPlaylist::Master);
    }
    if input
        .lines()
        .any(|line| line.starts_with("#EXT-X-TARGETDURATION:"))
    {
        return parse_media(base_url, input)
            .map(Box::new)
            .map(HlsPlaylist::Media);
    }
    Err(Error::Streaming(String::from(
        "HLS playlist is neither multivariant nor media",
    )))
}

/// Fetches and parses an HLS playlist exclusively through Zenwave.
///
/// Relative references are resolved against the final response URL after all
/// redirects, rather than the originally requested URL.
///
/// # Errors
///
/// Returns a streaming error when the request fails, the response is not
/// UTF-8, or the playlist is malformed.
pub async fn fetch_hls_playlist(request: MediaRequest) -> Result<HlsPlaylist, Error> {
    let response = fetch_media(request).await?;
    let input = std::str::from_utf8(response.bytes())
        .map_err(|error| Error::Streaming(format!("HLS playlist is not UTF-8: {error}")))?;
    parse_hls_playlist(response.effective_url(), input)
}

fn parse_master(base_url: &Url, input: &str) -> Result<HlsMasterPlaylist, Error> {
    let mut reader = Reader::from_str(input, ParsingOptions::default());
    let mut variants = Vec::new();
    let mut i_frame_variants = Vec::new();
    let mut renditions = Vec::new();
    let mut independent_segments = false;
    let mut pending_variant = None;
    while let Some(line) = read_hls_line(&mut reader)? {
        match line {
            HlsLine::KnownTag(KnownTag::Hls(tag)) => match tag {
                quick_hls::Tag::StreamInf(stream) => {
                    if pending_variant.replace(stream).is_some() {
                        return Err(Error::Streaming(String::from(
                            "HLS variant metadata is missing its URI",
                        )));
                    }
                }
                quick_hls::Tag::IFrameStreamInf(stream) => {
                    i_frame_variants.push(stream_variant(
                        base_url,
                        stream.uri(),
                        &stream,
                        VariantGroups {
                            audio: None,
                            video: stream.video().map(ToOwned::to_owned),
                            subtitles: None,
                            closed_captions: None,
                        },
                    )?);
                }
                quick_hls::Tag::Media(media) => {
                    renditions.push(HlsRendition {
                        kind: rendition_kind(media.media_type())?,
                        url: media
                            .uri()
                            .map(|url| resolve_url(base_url, url))
                            .transpose()?,
                        group_id: media.group_id().to_owned(),
                        name: media.name().to_owned(),
                        language: media.language().map(ToOwned::to_owned),
                        is_default: media.default(),
                        is_autoselect: media.autoselect(),
                        is_forced: media.forced(),
                    });
                }
                quick_hls::Tag::IndependentSegments(_) => independent_segments = true,
                _ => {}
            },
            HlsLine::Uri(uri) => {
                let stream = pending_variant.take().ok_or_else(|| {
                    Error::Streaming(format!(
                        "HLS multivariant playlist has URI {uri:?} without EXT-X-STREAM-INF"
                    ))
                })?;
                variants.push(stream_variant(
                    base_url,
                    &uri,
                    &stream,
                    VariantGroups {
                        audio: stream.audio().map(ToOwned::to_owned),
                        video: stream.video().map(ToOwned::to_owned),
                        subtitles: stream.subtitles().map(ToOwned::to_owned),
                        closed_captions: stream
                            .closed_captions()
                            .filter(|group| !group.eq_ignore_ascii_case("NONE"))
                            .map(ToOwned::to_owned),
                    },
                )?);
            }
            HlsLine::KnownTag(_)
            | HlsLine::UnknownTag(_)
            | HlsLine::Comment(_)
            | HlsLine::Blank => {}
        }
    }
    if pending_variant.is_some() {
        return Err(Error::Streaming(String::from(
            "HLS variant metadata is missing its URI at end of playlist",
        )));
    }
    if variants.is_empty() && i_frame_variants.is_empty() {
        return Err(Error::Streaming(String::from(
            "HLS multivariant playlist contains no variants",
        )));
    }

    Ok(HlsMasterPlaylist {
        variants,
        i_frame_variants,
        renditions,
        independent_segments,
    })
}

struct VariantGroups {
    audio: Option<String>,
    video: Option<String>,
    subtitles: Option<String>,
    closed_captions: Option<String>,
}

trait VariantAttributes {
    fn bandwidth(&self) -> u64;
    fn average_bandwidth(&self) -> Option<u64>;
    fn resolution(&self) -> Option<quick_m3u8::tag::DecimalResolution>;
    fn codecs(&self) -> Option<&str>;
}

impl VariantAttributes for quick_hls::StreamInf<'_> {
    fn bandwidth(&self) -> u64 {
        self.bandwidth()
    }

    fn average_bandwidth(&self) -> Option<u64> {
        self.average_bandwidth()
    }

    fn resolution(&self) -> Option<quick_m3u8::tag::DecimalResolution> {
        self.resolution()
    }

    fn codecs(&self) -> Option<&str> {
        self.codecs()
    }
}

impl VariantAttributes for quick_hls::IFrameStreamInf<'_> {
    fn bandwidth(&self) -> u64 {
        self.bandwidth()
    }

    fn average_bandwidth(&self) -> Option<u64> {
        self.average_bandwidth()
    }

    fn resolution(&self) -> Option<quick_m3u8::tag::DecimalResolution> {
        self.resolution()
    }

    fn codecs(&self) -> Option<&str> {
        self.codecs()
    }
}

fn stream_variant(
    base_url: &Url,
    uri: &str,
    stream_data: &impl VariantAttributes,
    groups: VariantGroups,
) -> Result<StreamVariant, Error> {
    let peak_bandwidth = NonZeroU64::new(stream_data.bandwidth())
        .ok_or_else(|| Error::Streaming(String::from("HLS variant bandwidth must be non-zero")))?;
    let average_bandwidth = stream_data
        .average_bandwidth()
        .map(|value| {
            NonZeroU64::new(value).ok_or_else(|| {
                Error::Streaming(String::from(
                    "HLS variant average bandwidth must be non-zero",
                ))
            })
        })
        .transpose()?;
    let dimensions = stream_data
        .resolution()
        .map(|resolution| -> Result<(u32, u32), Error> {
            Ok((
                u32::try_from(resolution.width)
                    .map_err(|_| Error::Streaming(String::from("HLS variant width exceeds u32")))?,
                u32::try_from(resolution.height).map_err(|_| {
                    Error::Streaming(String::from("HLS variant height exceeds u32"))
                })?,
            ))
        })
        .transpose()?;
    let codecs = stream_data
        .codecs()
        .map(|codecs| {
            codecs
                .split(',')
                .map(str::trim)
                .filter(|codec| !codec.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();

    Ok(StreamVariant {
        url: resolve_url(base_url, uri)?,
        peak_bandwidth,
        average_bandwidth,
        dimensions,
        codecs,
        audio_group_id: groups.audio,
        video_group_id: groups.video,
        subtitle_group_id: groups.subtitles,
        closed_caption_group_id: groups.closed_captions,
    })
}

fn rendition_kind(
    kind: EnumeratedString<'_, quick_hls::MediaType>,
) -> Result<HlsRenditionKind, Error> {
    match kind {
        EnumeratedString::Known(quick_hls::MediaType::Audio) => Ok(HlsRenditionKind::Audio),
        EnumeratedString::Known(quick_hls::MediaType::Video) => Ok(HlsRenditionKind::Video),
        EnumeratedString::Known(quick_hls::MediaType::Subtitles) => Ok(HlsRenditionKind::Subtitles),
        EnumeratedString::Known(quick_hls::MediaType::ClosedCaptions) => {
            Ok(HlsRenditionKind::ClosedCaptions)
        }
        EnumeratedString::Unknown(value) => Err(Error::Unsupported(format!(
            "unsupported future HLS rendition type {value}"
        ))),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the ordered HLS tag state machine stays centralized so segment-scoped tags cannot be applied out of order"
)]
fn parse_media(base_url: &Url, input: &str) -> Result<HlsMediaPlaylist, Error> {
    let mut reader = Reader::from_str(input, ParsingOptions::default());
    let mut target_duration = None;
    let mut declared_media_sequence = None;
    let mut discontinuity_sequence = None;
    let mut ended = false;
    let mut independent_segments = false;
    let mut segments = Vec::new();
    let mut next_duration = None;
    let mut next_range = None;
    let mut next_discontinuity = false;
    let mut next_gap = false;
    let mut previous_segment_range = None::<(Url, u64)>;
    let mut part_target = None;
    let mut server_control = None;
    let mut delta_update = None;
    let mut trailing_parts = Vec::new();
    let mut preload_hint = None;
    let mut rendition_reports = Vec::new();
    let mut initialization = None;
    let mut encryption = BTreeMap::<String, HlsEncryption>::new();
    let mut previous_part_range = None::<(Url, u64)>;

    while let Some(line) = read_hls_line(&mut reader)? {
        match line {
            HlsLine::KnownTag(KnownTag::Hls(tag)) => match tag {
                quick_hls::Tag::Targetduration(tag) => set_once(
                    &mut target_duration,
                    Duration::from_secs(tag.target_duration()),
                    "EXT-X-TARGETDURATION",
                )?,
                quick_hls::Tag::MediaSequence(tag) => set_once(
                    &mut declared_media_sequence,
                    usize::try_from(tag.media_sequence()).map_err(|_| {
                        Error::Streaming(String::from("HLS media sequence exceeds usize"))
                    })?,
                    "EXT-X-MEDIA-SEQUENCE",
                )?,
                quick_hls::Tag::DiscontinuitySequence(tag) => set_once(
                    &mut discontinuity_sequence,
                    usize::try_from(tag.discontinuity_sequence()).map_err(|_| {
                        Error::Streaming(String::from("HLS discontinuity sequence exceeds usize"))
                    })?,
                    "EXT-X-DISCONTINUITY-SEQUENCE",
                )?,
                quick_hls::Tag::Endlist(_) => ended = true,
                quick_hls::Tag::IndependentSegments(_) => independent_segments = true,
                quick_hls::Tag::Inf(tag) => set_once(
                    &mut next_duration,
                    positive_duration(tag.duration(), "EXTINF duration")?,
                    "EXTINF",
                )?,
                quick_hls::Tag::Byterange(tag) => set_once(
                    &mut next_range,
                    quick_m3u8::tag::DecimalIntegerRange {
                        length: tag.length(),
                        offset: tag.offset(),
                    },
                    "EXT-X-BYTERANGE",
                )?,
                quick_hls::Tag::Discontinuity(_) => next_discontinuity = true,
                quick_hls::Tag::Gap(_) => next_gap = true,
                quick_hls::Tag::PartInf(tag) => {
                    set_once(
                        &mut part_target,
                        positive_duration(tag.part_target(), "PART-TARGET")?,
                        "EXT-X-PART-INF",
                    )?;
                }
                quick_hls::Tag::ServerControl(tag) => {
                    set_once(
                        &mut server_control,
                        HlsServerControl {
                            can_skip_until: tag
                                .can_skip_until()
                                .map(|value| positive_duration(value, "CAN-SKIP-UNTIL"))
                                .transpose()?,
                            can_skip_dateranges: tag.can_skip_dateranges(),
                            hold_back: tag
                                .hold_back()
                                .map(|value| positive_duration(value, "HOLD-BACK"))
                                .transpose()?,
                            part_hold_back: tag
                                .part_hold_back()
                                .map(|value| positive_duration(value, "PART-HOLD-BACK"))
                                .transpose()?,
                            can_block_reload: tag.can_block_reload(),
                        },
                        "EXT-X-SERVER-CONTROL",
                    )?;
                }
                quick_hls::Tag::Skip(tag) => {
                    let skipped_segments =
                        usize::try_from(tag.skipped_segments()).map_err(|_| {
                            Error::Streaming(String::from(
                                "HLS skipped-segment count exceeds usize",
                            ))
                        })?;
                    if skipped_segments == 0 {
                        return Err(Error::Streaming(String::from(
                            "HLS delta update must skip at least one segment",
                        )));
                    }
                    set_once(
                        &mut delta_update,
                        HlsDeltaUpdate {
                            skipped_segments,
                            recently_removed_dateranges: tag
                                .recently_removed_dateranges()
                                .map(ToOwned::to_owned),
                        },
                        "EXT-X-SKIP",
                    )?;
                }
                quick_hls::Tag::Map(tag) => {
                    initialization = Some(parse_quick_initialization(base_url, &tag)?);
                }
                quick_hls::Tag::Key(tag) => {
                    update_quick_encryption(base_url, &tag, &mut encryption)?;
                }
                quick_hls::Tag::Part(tag) => {
                    let url = resolve_url(base_url, tag.uri())?;
                    let byte_range = tag
                        .byterange()
                        .map(|range| {
                            resolve_quick_byte_range(range, &url, previous_part_range.as_ref())
                        })
                        .transpose()?;
                    if let Some(range) = byte_range {
                        previous_part_range = Some((url.clone(), range.end_exclusive()));
                    } else {
                        previous_part_range = None;
                    }
                    trailing_parts.push(HlsPartialSegment {
                        sequence: 0,
                        part_index: trailing_parts.len(),
                        url,
                        duration: positive_duration(tag.duration(), "partial-segment DURATION")?,
                        byte_range,
                        initialization: initialization.clone(),
                        encryption: encryption.values().cloned().collect(),
                        independent: tag.independent(),
                        discontinuity: next_discontinuity && trailing_parts.is_empty(),
                        gap: tag.gap(),
                    });
                }
                quick_hls::Tag::PreloadHint(tag) => {
                    let kind = match tag.hint_type() {
                        EnumeratedString::Known(PreloadHintType::Part) => HlsPreloadHintKind::Part,
                        EnumeratedString::Known(PreloadHintType::Map) => HlsPreloadHintKind::Map,
                        EnumeratedString::Unknown(value) => {
                            return Err(Error::Unsupported(format!(
                                "unsupported HLS preload-hint type {value}"
                            )));
                        }
                    };
                    let byte_range = tag
                        .byterange_length()
                        .map(|length| {
                            let end =
                                tag.byterange_start().checked_add(length).ok_or_else(|| {
                                    Error::Streaming(String::from(
                                        "HLS preload-hint byte range overflowed u64",
                                    ))
                                })?;
                            MediaByteRange::new(tag.byterange_start(), end)
                        })
                        .transpose()?;
                    preload_hint = Some(HlsPreloadHint {
                        kind,
                        url: resolve_url(base_url, tag.uri())?,
                        byte_range,
                    });
                }
                quick_hls::Tag::RenditionReport(tag) => {
                    rendition_reports.push(HlsRenditionReport {
                        url: resolve_url(base_url, tag.uri())?,
                        last_sequence: usize::try_from(tag.last_msn()).map_err(|_| {
                            Error::Streaming(String::from(
                                "HLS rendition-report sequence exceeds usize",
                            ))
                        })?,
                        last_part: tag
                            .last_part()
                            .map(|part| {
                                usize::try_from(part).map_err(|_| {
                                    Error::Streaming(String::from(
                                        "HLS rendition-report part exceeds usize",
                                    ))
                                })
                            })
                            .transpose()?,
                    });
                }
                _ => {}
            },
            HlsLine::Uri(uri) => {
                let duration = next_duration.take().ok_or_else(|| {
                    Error::Streaming(format!("HLS media-segment URI {uri:?} is missing EXTINF"))
                })?;
                let url = resolve_url(base_url, &uri)?;
                let byte_range = next_range
                    .take()
                    .map(|range| {
                        resolve_quick_byte_range(range, &url, previous_segment_range.as_ref())
                    })
                    .transpose()?;
                previous_segment_range =
                    byte_range.map(|range| (url.clone(), range.end_exclusive()));
                segments.push(HlsSegment {
                    sequence: 0,
                    url,
                    duration,
                    byte_range,
                    initialization: initialization.clone(),
                    encryption: encryption.values().cloned().collect(),
                    discontinuity: std::mem::take(&mut next_discontinuity),
                    gap: std::mem::take(&mut next_gap),
                });
                trailing_parts.clear();
                previous_part_range = None;
            }
            HlsLine::UnknownTag(_)
            | HlsLine::Comment(_)
            | HlsLine::Blank
            | HlsLine::KnownTag(KnownTag::Custom(_)) => {}
        }
    }

    if next_duration.is_some() {
        return Err(Error::Streaming(String::from(
            "HLS playlist ends with EXTINF but no media-segment URI",
        )));
    }
    let target_duration = target_duration.ok_or_else(|| {
        Error::Streaming(String::from(
            "HLS media playlist is missing EXT-X-TARGETDURATION",
        ))
    })?;
    if target_duration.is_zero() {
        return Err(Error::Streaming(String::from(
            "HLS target duration must be greater than zero",
        )));
    }
    let skipped_segments = delta_update
        .as_ref()
        .map_or(0, |update| update.skipped_segments);
    let media_sequence = declared_media_sequence
        .unwrap_or(0)
        .checked_add(skipped_segments)
        .ok_or_else(|| Error::Streaming(String::from("HLS media sequence overflowed usize")))?;
    for (offset, segment) in segments.iter_mut().enumerate() {
        segment.sequence = media_sequence.checked_add(offset).ok_or_else(|| {
            Error::Streaming(String::from("HLS segment sequence overflowed usize"))
        })?;
    }
    let trailing_sequence = media_sequence
        .checked_add(segments.len())
        .ok_or_else(|| Error::Streaming(String::from("HLS partial sequence overflowed usize")))?;
    for part in &mut trailing_parts {
        part.sequence = trailing_sequence;
    }

    let has_low_latency_tags = part_target.is_some()
        || !trailing_parts.is_empty()
        || preload_hint.is_some()
        || !rendition_reports.is_empty();
    let low_latency = if has_low_latency_tags {
        if server_control.is_none() {
            return Err(Error::Streaming(String::from(
                "Low-Latency HLS playlist is missing EXT-X-SERVER-CONTROL",
            )));
        }
        Some(HlsLowLatency {
            part_target: part_target.ok_or_else(|| {
                Error::Streaming(String::from(
                    "Low-Latency HLS playlist is missing EXT-X-PART-INF",
                ))
            })?,
            trailing_parts,
            preload_hint,
            rendition_reports,
        })
    } else {
        None
    };
    Ok(HlsMediaPlaylist {
        target_duration,
        media_sequence,
        discontinuity_sequence: discontinuity_sequence.unwrap_or(0),
        segments,
        ended,
        independent_segments,
        delta_update,
        server_control,
        low_latency,
    })
}

fn read_hls_line<'a>(
    reader: &mut Reader<&'a str, quick_m3u8::tag::NoCustomTag>,
) -> Result<Option<HlsLine<'a>>, Error> {
    reader
        .read_line()
        .map_err(|error| Error::Streaming(format!("invalid HLS playlist line: {error}")))
}

fn set_once<T>(slot: &mut Option<T>, value: T, tag: &str) -> Result<(), Error> {
    if slot.replace(value).is_some() {
        return Err(Error::Streaming(format!(
            "HLS playlist contains duplicate {tag}"
        )));
    }
    Ok(())
}

fn positive_duration(value: f64, attribute: &str) -> Result<Duration, Error> {
    let duration = Duration::try_from_secs_f64(value).map_err(|error| {
        Error::Streaming(format!(
            "HLS {attribute} must be finite and non-negative: {error}"
        ))
    })?;
    if duration.is_zero() {
        return Err(Error::Streaming(format!(
            "HLS {attribute} must be greater than zero"
        )));
    }
    Ok(duration)
}

fn parse_quick_initialization(
    base_url: &Url,
    map: &quick_hls::Map<'_>,
) -> Result<HlsInitializationSegment, Error> {
    let byte_range = map
        .byterange()
        .map(|range| {
            let end = range.offset.checked_add(range.length).ok_or_else(|| {
                Error::Streaming(String::from("HLS map byte range overflowed u64"))
            })?;
            MediaByteRange::new(range.offset, end)
        })
        .transpose()?;
    Ok(HlsInitializationSegment {
        url: resolve_url(base_url, map.uri())?,
        byte_range,
    })
}

fn update_quick_encryption(
    base_url: &Url,
    key: &quick_hls::Key<'_>,
    encryption: &mut BTreeMap<String, HlsEncryption>,
) -> Result<(), Error> {
    let key_format = key.keyformat().to_owned();
    let method = match key.method() {
        EnumeratedString::Known(Method::None) => {
            encryption.remove(&key_format);
            return Ok(());
        }
        EnumeratedString::Known(Method::Aes128) => HlsEncryptionMethod::Aes128,
        EnumeratedString::Known(Method::SampleAes) => HlsEncryptionMethod::SampleAes,
        EnumeratedString::Known(Method::SampleAesCtr) => HlsEncryptionMethod::SampleAesCtr,
        EnumeratedString::Unknown(value) => {
            return Err(Error::Unsupported(format!(
                "unsupported HLS encryption method {value}"
            )));
        }
    };
    let key_url = key.uri().ok_or_else(|| {
        Error::Streaming(format!(
            "HLS encryption method {method:?} requires a key URI"
        ))
    })?;
    let initialization_vector = key.iv().map(parse_initialization_vector).transpose()?;
    encryption.insert(
        key_format.clone(),
        HlsEncryption {
            method,
            key_url: resolve_url(base_url, key_url)?,
            initialization_vector,
            key_format,
        },
    );
    Ok(())
}

fn parse_initialization_vector(value: &str) -> Result<[u8; 16], Error> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .ok_or_else(|| Error::Streaming(String::from("HLS IV must begin with 0x")))?;
    let bytes = hex::decode(digits)
        .map_err(|error| Error::Streaming(format!("invalid HLS IV hexadecimal: {error}")))?;
    <[u8; 16]>::try_from(bytes)
        .map_err(|_| Error::Streaming(String::from("HLS IV must contain exactly 128 bits")))
}

fn resolve_quick_byte_range(
    range: quick_m3u8::tag::DecimalIntegerRange,
    url: &Url,
    previous: Option<&(Url, u64)>,
) -> Result<HlsSegmentRange, Error> {
    let start = match range.offset {
        Some(offset) => offset,
        None => previous
            .filter(|(previous_url, _)| previous_url == url)
            .map(|(_, end)| *end)
            .ok_or_else(|| {
                Error::Streaming(format!(
                    "implicit HLS byte range for {url} has no preceding range on the same resource"
                ))
            })?,
    };
    let end = start
        .checked_add(range.length)
        .ok_or_else(|| Error::Streaming(String::from("HLS byte range overflowed u64")))?;
    MediaByteRange::new(start, end)
}

fn resolve_url(base_url: &Url, relative: &str) -> Result<Url, Error> {
    base_url
        .join(relative)
        .map_err(|error| Error::Streaming(format!("invalid HLS URL {relative:?}: {error}")))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use url::Url;

    use super::{
        HlsEncryptionMethod, HlsPlaylist, HlsPreloadHintKind, HlsRenditionKind, parse_hls_playlist,
    };

    #[test]
    fn parses_multivariant_tracks_and_resolves_urls() {
        let base = Url::parse("https://waterui.dev/video/master.m3u8")
            .expect("test base URL must be valid");
        let playlist = parse_hls_playlist(&base, include_str!("../tests/assets/hls_master.m3u8"))
            .expect("test master playlist must parse");
        let HlsPlaylist::Master(master) = playlist else {
            panic!("test manifest must be a master playlist");
        };
        assert_eq!(master.variants.len(), 2);
        assert_eq!(master.variants[1].dimensions, Some((1920, 1080)));
        assert_eq!(master.variants[0].audio_group_id.as_deref(), Some("audio"));
        assert_eq!(
            master.variants[0].subtitle_group_id.as_deref(),
            Some("subs")
        );
        assert_eq!(
            master.variants[0].url.as_str(),
            "https://waterui.dev/video/720p/index.m3u8"
        );
        assert!(master.renditions.iter().any(|rendition| {
            rendition.kind == HlsRenditionKind::Audio && rendition.language.as_deref() == Some("en")
        }));
    }

    #[test]
    fn parses_ranges_initialization_encryption_and_discontinuity() {
        let base = Url::parse("https://waterui.dev/video/720p/index.m3u8")
            .expect("test base URL must be valid");
        let playlist = parse_hls_playlist(&base, include_str!("../tests/assets/hls_media.m3u8"))
            .expect("test media playlist must parse");
        let HlsPlaylist::Media(media) = playlist else {
            panic!("test manifest must be a media playlist");
        };
        assert_eq!(media.segments.len(), 2);
        assert_eq!(
            media.segments[0]
                .byte_range
                .expect("range must exist")
                .start(),
            0
        );
        assert_eq!(
            media.segments[1]
                .byte_range
                .expect("range must exist")
                .start(),
            1_000
        );
        assert!(media.segments[1].discontinuity);
        assert_eq!(
            media.segments[0].encryption[0].method,
            HlsEncryptionMethod::Aes128
        );
        assert_eq!(
            media.segments[0]
                .initialization
                .as_ref()
                .expect("map must exist")
                .url
                .as_str(),
            "https://waterui.dev/video/720p/init.mp4"
        );
    }

    #[test]
    fn parses_low_latency_parts_delta_updates_and_reload_policy() {
        let base = Url::parse("https://waterui.dev/video/1080p/index.m3u8")
            .expect("test base URL must be valid");
        let playlist =
            parse_hls_playlist(&base, include_str!("../tests/assets/hls_low_latency.m3u8"))
                .expect("test Low-Latency HLS playlist must parse");
        let HlsPlaylist::Media(media) = playlist else {
            panic!("test manifest must be a media playlist");
        };
        assert_eq!(media.media_sequence, 102);
        assert_eq!(media.segments[0].sequence, 102);
        assert_eq!(
            media
                .delta_update
                .as_ref()
                .expect("delta metadata must exist")
                .skipped_segments,
            2
        );
        let low_latency = media
            .low_latency
            .as_ref()
            .expect("low-latency metadata must exist");
        assert_eq!(low_latency.part_target, Duration::from_millis(500));
        let server_control = media
            .server_control
            .expect("server-control metadata must exist");
        assert_eq!(
            server_control.part_hold_back,
            Some(Duration::from_millis(1_500))
        );
        assert!(server_control.can_block_reload);
        assert_eq!(low_latency.trailing_parts.len(), 2);
        assert_eq!(low_latency.trailing_parts[0].sequence, 103);
        assert_eq!(low_latency.trailing_parts[1].part_index, 1);
        assert_eq!(
            low_latency.trailing_parts[1]
                .byte_range
                .expect("implicit range must resolve")
                .start(),
            400
        );
        assert_eq!(
            low_latency.trailing_parts[0].encryption[0].method,
            HlsEncryptionMethod::SampleAes
        );
        let preload = low_latency
            .preload_hint
            .as_ref()
            .expect("preload hint must exist");
        assert_eq!(preload.kind, HlsPreloadHintKind::Part);
        assert_eq!(
            preload
                .byte_range
                .expect("preload range must exist")
                .start(),
            1_000
        );
        assert_eq!(low_latency.rendition_reports[0].last_sequence, 103);
        assert_eq!(low_latency.rendition_reports[0].last_part, Some(1));
    }
}
