use std::{
    num::{NonZeroU32, NonZeroU64},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use dash_mpd::{
    AdaptationSet, BaseURL, ContentProtection, MPD, Period, Representation, SegmentBase,
    SegmentList, SegmentTemplate,
};
use num_traits::ToPrimitive as _;
use url::Url;
use uuid::Uuid;
use waterkit_video_core::Error;

use crate::{AdaptiveVariant, MediaByteRange, MediaRequest, fetch_media};

/// Static or live behavior declared by a DASH MPD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DashManifestKind {
    /// A finite on-demand presentation.
    Static,
    /// A presentation whose availability window changes over time.
    Dynamic,
}

/// Effective advance notice for a DASH segment relative to its nominal end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DashAvailabilityTimeOffset {
    /// The segment becomes requestable this duration before its nominal end.
    Finite(Duration),
    /// The segment is available from the start of its containing period.
    Infinite,
}

impl DashAvailabilityTimeOffset {
    /// No early segment availability.
    pub const ZERO: Self = Self::Finite(Duration::ZERO);

    fn checked_add(self, other: Self) -> Result<Self, Error> {
        match (self, other) {
            (Self::Infinite, _) | (_, Self::Infinite) => Ok(Self::Infinite),
            (Self::Finite(left), Self::Finite(right)) => {
                left.checked_add(right).map(Self::Finite).ok_or_else(|| {
                    Error::Streaming(String::from("DASH availabilityTimeOffset exceeds Duration"))
                })
            }
        }
    }
}

/// Target live latency constraints from one DASH service description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashLatency {
    /// Producer-reference identifier used by this latency target.
    pub reference_id: Option<String>,
    /// Smallest supported live latency.
    pub min: Option<Duration>,
    /// Preferred live latency.
    pub target: Option<Duration>,
    /// Largest supported live latency.
    pub max: Option<Duration>,
}

/// A positive playback-rate multiplier represented exactly in parts per million.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DashPlaybackRate(NonZeroU32);

impl DashPlaybackRate {
    /// Returns the playback-rate multiplier as a floating-point value.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.0.get()) / 1_000_000.0
    }
}

/// Playback-rate correction bounds for converging on a live latency target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashPlaybackRateRange {
    /// Slowest permitted playback-rate multiplier.
    pub min: Option<DashPlaybackRate>,
    /// Fastest permitted playback-rate multiplier.
    pub max: Option<DashPlaybackRate>,
}

/// Normalized low-latency policy advertised by a DASH service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashServiceDescription {
    /// Optional service-description identifier.
    pub id: Option<String>,
    /// Live latency targets advertised by the service.
    pub latency: Vec<DashLatency>,
    /// Playback-rate correction ranges advertised by the service.
    pub playback_rates: Vec<DashPlaybackRateRange>,
}

/// Mapping between media presentation time and producer wall-clock time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashProducerReferenceTime {
    /// Manifest identifier used by service-description latency references.
    pub id: String,
    /// Whether equivalent producer-reference metadata is carried in-band.
    pub inband: bool,
    /// Media presentation timestamp in `timescale` units.
    pub presentation_time: u64,
    /// Producer wall-clock instant corresponding to `presentation_time`.
    pub wall_clock_time: SystemTime,
    /// Presentation timescale inherited from the representation segment model.
    pub timescale: NonZeroU64,
}

/// One effective DASH content base after hierarchical `BaseURL` resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashBaseUrl {
    /// Fully resolved content base URL.
    pub url: Url,
    /// CDN/service identifier when supplied by the manifest.
    pub service_location: Option<String>,
    /// Hierarchical DVB selection priorities, outermost first.
    pub priorities: Vec<u64>,
    /// Relative selection weight at the innermost level.
    pub weight: NonZeroU64,
    /// Cumulative early-availability offset inherited along this URL path.
    pub availability_time_offset: DashAvailabilityTimeOffset,
    /// Whether a request starts only after the complete segment is available.
    pub availability_time_complete: bool,
}

/// A DASH UTC synchronization source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashUtcTiming {
    /// Timing method URI.
    pub scheme_id_uri: String,
    /// Method-specific timing value.
    pub value: String,
}

/// Parsed and normalized DASH presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashManifest {
    /// Static or dynamic presentation kind.
    pub kind: DashManifestKind,
    /// Wall-clock instant at which the dynamic presentation timeline begins.
    pub availability_start_time: Option<SystemTime>,
    /// Wall-clock instant after which the presentation is no longer available.
    pub availability_end_time: Option<SystemTime>,
    /// Wall-clock publication instant for this MPD revision.
    pub publish_time: Option<SystemTime>,
    /// Total duration when declared.
    pub duration: Option<Duration>,
    /// Minimum client buffer requested by the manifest.
    pub minimum_buffer_time: Option<Duration>,
    /// Live MPD refresh cadence.
    pub minimum_update_period: Option<Duration>,
    /// Live seekable-window depth.
    pub time_shift_buffer_depth: Option<Duration>,
    /// Suggested delay behind the live edge.
    pub suggested_presentation_delay: Option<Duration>,
    /// Clock synchronization sources for live availability calculations.
    pub utc_timing: Vec<DashUtcTiming>,
    /// Presentation-wide low-latency service policies.
    pub service_descriptions: Vec<DashServiceDescription>,
    /// Ordered presentation periods.
    pub periods: Vec<DashPeriod>,
}

/// One contiguous DASH presentation period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashPeriod {
    /// Optional manifest identifier.
    pub id: Option<String>,
    /// Start relative to the presentation timeline.
    pub start: Duration,
    /// Period duration when known.
    pub duration: Option<Duration>,
    /// Period-specific low-latency service policies.
    pub service_descriptions: Vec<DashServiceDescription>,
    /// Media adaptation sets in this period.
    pub adaptation_sets: Vec<DashAdaptationSet>,
}

/// Semantic media kind carried by one adaptation set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DashTrackKind {
    /// Video samples.
    Video,
    /// Audio samples.
    Audio,
    /// Text, `WebVTT`, `TTML`, or image subtitles/captions.
    Subtitle,
}

/// One selectable DASH track group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashAdaptationSet {
    /// Optional manifest identifier.
    pub id: Option<String>,
    /// Media kind shared by its representations.
    pub kind: DashTrackKind,
    /// RFC 5646 language tag when supplied.
    pub language: Option<String>,
    /// DASH role values such as `main`, `alternate`, or `commentary`.
    pub roles: Vec<String>,
    /// Alternative encoded representations.
    pub representations: Vec<DashRepresentation>,
}

/// Common Encryption metadata associated with a representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashContentProtection {
    /// Protection scheme URI, including DRM system UUID URNs.
    pub scheme_id_uri: String,
    /// Scheme-specific value when supplied.
    pub value: Option<String>,
    /// CENC default key identifiers.
    pub default_key_ids: Vec<Uuid>,
    /// Decoded Protection System Specific Header payloads.
    pub pssh: Vec<Vec<u8>>,
    /// Resolved license acquisition endpoints.
    pub license_urls: Vec<Url>,
}

/// One bitrate/codec option inside an adaptation set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashRepresentation {
    /// Representation identifier.
    pub id: String,
    /// Declared average bandwidth in bits per second.
    pub bandwidth: NonZeroU64,
    /// Encoded dimensions when applicable.
    pub dimensions: Option<(u32, u32)>,
    /// Frame rate as a reduced numerator and denominator when supplied.
    pub frame_rate: Option<(u32, NonZeroU64)>,
    /// RFC 6381 codec identifiers.
    pub codecs: Vec<String>,
    /// MIME type inherited from the representation or adaptation set.
    pub mime_type: String,
    /// Fully inherited content-protection metadata.
    pub content_protection: Vec<DashContentProtection>,
    /// Producer wall-clock mappings inherited by this representation.
    pub producer_reference_times: Vec<DashProducerReferenceTime>,
    /// Segment addressing model.
    pub segments: DashSegmentSource,
}

impl AdaptiveVariant for DashRepresentation {
    fn selection_bandwidth(&self) -> NonZeroU64 {
        self.bandwidth
    }

    fn dimensions(&self) -> Option<(u32, u32)> {
        self.dimensions
    }
}

impl DashRepresentation {
    /// Returns how early this representation's media segments may be requested.
    #[must_use]
    pub const fn availability_time_offset(&self) -> DashAvailabilityTimeOffset {
        match &self.segments {
            DashSegmentSource::Template(template) => template.availability_time_offset,
            DashSegmentSource::List(list) => list.availability_time_offset,
            DashSegmentSource::Base(base) => base.availability_time_offset,
        }
    }

    /// Returns whether media requests start only after a complete segment is available.
    #[must_use]
    pub const fn availability_time_complete(&self) -> bool {
        match &self.segments {
            DashSegmentSource::Template(template) => template.availability_time_complete,
            DashSegmentSource::List(list) => list.availability_time_complete,
            DashSegmentSource::Base(base) => base.availability_time_complete,
        }
    }

    /// Returns the media timestamp mapped to the containing period start.
    #[must_use]
    pub fn presentation_time_offset(&self) -> Duration {
        match &self.segments {
            DashSegmentSource::Template(template) => {
                ticks_to_duration(template.presentation_time_offset, template.timescale)
            }
            DashSegmentSource::List(list) => {
                ticks_to_duration(list.presentation_time_offset, list.timescale)
            }
            DashSegmentSource::Base(base) => {
                ticks_to_duration(base.presentation_time_offset, base.timescale)
            }
        }
    }

    /// Resolves this representation's initialization resource.
    ///
    /// # Errors
    ///
    /// Returns an error when an initialization template is malformed.
    pub fn initialization(&self) -> Result<Option<DashInitialization>, Error> {
        match &self.segments {
            DashSegmentSource::Template(template) => template
                .initialization
                .as_ref()
                .map(|initialization| {
                    let relative =
                        render_template(initialization, &self.id, self.bandwidth, None, None)?;
                    Ok(DashInitialization {
                        urls: resolve_candidates(&template.base_urls, &relative)?,
                        byte_range: None,
                    })
                })
                .transpose(),
            DashSegmentSource::List(list) => Ok(list.initialization.clone()),
            DashSegmentSource::Base(base) => Ok(base.initialization.clone()),
        }
    }

    /// Expands finite segment addressing into an ordered playback plan.
    ///
    /// `period_duration` is required for constant-duration templates and for
    /// open-ended timeline repeats without a following explicit time. Live
    /// callers pass the currently available window duration.
    ///
    /// # Errors
    ///
    /// Returns an error for unresolved durations, malformed templates,
    /// timeline overflow, or a SegmentList/timeline count mismatch.
    pub fn plan_segments(
        &self,
        period_duration: Option<Duration>,
    ) -> Result<Vec<DashPlannedSegment>, Error> {
        match &self.segments {
            DashSegmentSource::Template(template) => {
                plan_template(template, &self.id, self.bandwidth, period_duration)
            }
            DashSegmentSource::List(list) => plan_list(list, period_duration),
            DashSegmentSource::Base(base) => {
                let duration = period_duration.ok_or_else(|| {
                    Error::Streaming(String::from(
                        "DASH SegmentBase requires a known period duration",
                    ))
                })?;
                Ok(vec![DashPlannedSegment {
                    number: 1,
                    start: Duration::ZERO,
                    duration,
                    resource: DashSegmentReference {
                        urls: base.media_urls.clone(),
                        byte_range: None,
                        index_urls: Vec::new(),
                        index_range: base.index_range,
                    },
                }])
            }
        }
    }

    /// Expands only segments intersecting a presentation-time window.
    ///
    /// Constant-duration templates are indexed directly, so a long-running
    /// live presentation does not allocate a plan for every expired segment.
    ///
    /// # Errors
    ///
    /// Returns an error for an inverted window or malformed segment addressing.
    pub fn plan_segments_in_window(
        &self,
        window_start: Duration,
        window_end: Duration,
    ) -> Result<Vec<DashPlannedSegment>, Error> {
        if window_start > window_end {
            return Err(Error::Streaming(String::from(
                "DASH segment window starts after it ends",
            )));
        }
        if window_start == window_end {
            return Ok(Vec::new());
        }
        if let DashSegmentSource::Template(template) = &self.segments
            && template.timeline.is_empty()
        {
            return plan_constant_template_window(
                template,
                &self.id,
                self.bandwidth,
                window_start,
                window_end,
            );
        }
        let mut segments = self.plan_segments(Some(window_end))?;
        segments.retain(|segment| {
            segment.start < window_end
                && segment
                    .start
                    .checked_add(segment.duration)
                    .is_some_and(|end| end > window_start)
        });
        Ok(segments)
    }
}

/// Segment addressing model selected after DASH inheritance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashSegmentSource {
    /// URL templates driven by number or presentation time.
    Template(DashSegmentTemplate),
    /// Explicit ordered segment URLs.
    List(DashSegmentList),
    /// A single resource with optional index ranges.
    Base(DashSegmentBase),
}

/// Initialization resource for a DASH representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashInitialization {
    /// Candidate URLs ordered by manifest priority.
    pub urls: Vec<Url>,
    /// Optional byte range within each candidate resource.
    pub byte_range: Option<MediaByteRange>,
}

/// One explicit DASH media segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashSegmentReference {
    /// Candidate media URLs ordered by manifest priority.
    pub urls: Vec<Url>,
    /// Optional byte range within the resource.
    pub byte_range: Option<MediaByteRange>,
    /// Optional index resource candidates.
    pub index_urls: Vec<Url>,
    /// Optional index byte range.
    pub index_range: Option<MediaByteRange>,
}

/// One fully resolved DASH media segment on the presentation timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashPlannedSegment {
    /// Template or list sequence number.
    pub number: u64,
    /// Start relative to the containing period.
    pub start: Duration,
    /// Segment duration.
    pub duration: Duration,
    /// Network resource and optional byte/index ranges.
    pub resource: DashSegmentReference,
}

/// One `SegmentTimeline/S` run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DashTimelineEntry {
    /// Explicit first presentation time, or the previous run end when absent.
    pub start: Option<u64>,
    /// Non-zero duration in timescale ticks.
    pub duration: NonZeroU64,
    /// Additional repetitions. `-1` means repeat to the next run or period end.
    pub repeat: i64,
}

/// DASH `SegmentTemplate` inherited to representation scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashSegmentTemplate {
    /// Effective candidate content bases.
    pub base_urls: Vec<DashBaseUrl>,
    /// Media URL template.
    pub media: String,
    /// Initialization URL template when required.
    pub initialization: Option<String>,
    /// Optional segment-index URL template.
    pub index: Option<String>,
    /// Template timescale.
    pub timescale: NonZeroU64,
    /// Constant segment duration when no timeline is present.
    pub duration: Option<NonZeroU64>,
    /// First `$Number$` value.
    pub start_number: u64,
    /// Presentation timestamp offset.
    pub presentation_time_offset: u64,
    /// Variable-duration segment timeline.
    pub timeline: Vec<DashTimelineEntry>,
    /// Effective early-availability offset after `BaseURL` and template inheritance.
    pub availability_time_offset: DashAvailabilityTimeOffset,
    /// Whether the segment is complete when its request begins.
    pub availability_time_complete: bool,
}

/// DASH `SegmentList` inherited to representation scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashSegmentList {
    /// Timescale used by duration and timeline values.
    pub timescale: NonZeroU64,
    /// Constant duration when supplied.
    pub duration: Option<NonZeroU64>,
    /// Media timestamp mapped to the containing period start.
    pub presentation_time_offset: u64,
    /// Optional initialization resource.
    pub initialization: Option<DashInitialization>,
    /// Explicit ordered media references.
    pub segments: Vec<DashSegmentReference>,
    /// Optional timeline matching the explicit segment order.
    pub timeline: Vec<DashTimelineEntry>,
    /// Effective early-availability offset inherited from `BaseURL`.
    pub availability_time_offset: DashAvailabilityTimeOffset,
    /// Whether the segment is complete when its request begins.
    pub availability_time_complete: bool,
}

/// DASH `SegmentBase` single-resource addressing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashSegmentBase {
    /// Candidate representation resource URLs.
    pub media_urls: Vec<Url>,
    /// Optional initialization resource.
    pub initialization: Option<DashInitialization>,
    /// Optional segment-index range in the media resource.
    pub index_range: Option<MediaByteRange>,
    /// Timescale for presentation metadata.
    pub timescale: NonZeroU64,
    /// Presentation timestamp offset.
    pub presentation_time_offset: u64,
    /// Effective early-availability offset after `BaseURL` and `SegmentBase` inheritance.
    pub availability_time_offset: DashAvailabilityTimeOffset,
    /// Whether the segment is complete when its request begins.
    pub availability_time_complete: bool,
}

/// Fetches and parses a DASH MPD exclusively through Zenwave.
///
/// # Errors
///
/// Returns a streaming error when the request fails, the response is not
/// UTF-8, or the MPD violates normalized `WaterKit` invariants.
pub async fn fetch_dash_manifest(request: MediaRequest) -> Result<DashManifest, Error> {
    let response = fetch_media(request).await?;
    let input = std::str::from_utf8(response.bytes())
        .map_err(|error| Error::Streaming(format!("DASH MPD is not UTF-8: {error}")))?;
    parse_dash_manifest(response.effective_url(), input)
}

/// Parses and normalizes a DASH MPD using its final network URL as the base.
///
/// # Errors
///
/// Returns a streaming or unsupported error for malformed MPDs, unresolved
/// external `XLinks`, invalid ranges, invalid `CENC` metadata, or incomplete
/// representation/segment declarations.
pub fn parse_dash_manifest(base_url: &Url, input: &str) -> Result<DashManifest, Error> {
    let mpd = dash_mpd::parse(input)
        .map_err(|error| Error::Streaming(format!("invalid DASH MPD: {error}")))?;
    normalize_manifest(base_url, &mpd)
}

fn normalize_manifest(base_url: &Url, mpd: &MPD) -> Result<DashManifest, Error> {
    if mpd.periods.is_empty() {
        return Err(Error::Streaming(String::from("DASH MPD has no periods")));
    }
    let kind = match mpd.mpdtype.as_deref() {
        None | Some("static") => DashManifestKind::Static,
        Some("dynamic") => DashManifestKind::Dynamic,
        Some(kind) => {
            return Err(Error::Unsupported(format!(
                "unsupported DASH presentation type {kind:?}"
            )));
        }
    };
    let root = vec![DashBaseUrl {
        url: base_url.clone(),
        service_location: None,
        priorities: Vec::new(),
        weight: NonZeroU64::MIN,
        availability_time_offset: DashAvailabilityTimeOffset::ZERO,
        availability_time_complete: true,
    }];
    let bases = inherit_base_urls(&root, &mpd.base_url)?;
    let global_protection = normalize_protection(base_url, &mpd.ContentProtection)?;
    let periods = mpd
        .periods
        .iter()
        .enumerate()
        .map(|(index, period)| normalize_period(index, period, &bases, &global_protection, mpd))
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(DashManifest {
        kind,
        availability_start_time: mpd
            .availabilityStartTime
            .as_ref()
            .map(xs_datetime_to_system_time)
            .transpose()?,
        availability_end_time: mpd
            .availabilityEndTime
            .as_ref()
            .map(xs_datetime_to_system_time)
            .transpose()?,
        publish_time: mpd
            .publishTime
            .as_ref()
            .map(xs_datetime_to_system_time)
            .transpose()?,
        duration: mpd.mediaPresentationDuration,
        minimum_buffer_time: mpd.minBufferTime,
        minimum_update_period: mpd.minimumUpdatePeriod,
        time_shift_buffer_depth: mpd.timeShiftBufferDepth,
        suggested_presentation_delay: mpd.suggestedPresentationDelay,
        utc_timing: mpd
            .UTCTiming
            .iter()
            .filter_map(|timing| {
                timing.value.as_ref().map(|value| DashUtcTiming {
                    scheme_id_uri: timing.schemeIdUri.clone(),
                    value: value.clone(),
                })
            })
            .collect(),
        service_descriptions: normalize_service_descriptions(&mpd.ServiceDescription)?,
        periods,
    })
}

fn xs_datetime_to_system_time(value: &dash_mpd::XsDatetime) -> Result<SystemTime, Error> {
    let seconds = value.timestamp();
    let nanos = value.timestamp_subsec_nanos();
    let subsecond = Duration::from_nanos(u64::from(nanos));
    if seconds >= 0 {
        UNIX_EPOCH
            .checked_add(Duration::from_secs(seconds.unsigned_abs()))
            .and_then(|time| time.checked_add(subsecond))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs(seconds.unsigned_abs()))
            .and_then(|time| time.checked_add(subsecond))
    }
    .ok_or_else(|| Error::Streaming(String::from("DASH wall-clock timestamp exceeds SystemTime")))
}

fn normalize_period(
    index: usize,
    period: &Period,
    parent_bases: &[DashBaseUrl],
    parent_protection: &[DashContentProtection],
    mpd: &MPD,
) -> Result<DashPeriod, Error> {
    reject_external_xlink("period", period.href.as_deref())?;
    let bases = inherit_base_urls(parent_bases, &period.BaseURL)?;
    let mut protection = parent_protection.to_vec();
    extend_unique(
        &mut protection,
        normalize_protection(&bases[0].url, &period.ContentProtection)?,
    );
    let start = period.start.unwrap_or_else(|| {
        if index == 0 {
            Duration::ZERO
        } else {
            mpd.periods[index - 1]
                .start
                .zip(mpd.periods[index - 1].duration)
                .map_or(Duration::ZERO, |(start, duration)| start + duration)
        }
    });
    let duration = period.duration.or_else(|| {
        mpd.periods
            .get(index + 1)
            .and_then(|next| next.start)
            .and_then(|next| next.checked_sub(start))
            .or_else(|| mpd.mediaPresentationDuration?.checked_sub(start))
    });
    let adaptation_sets = period
        .adaptations
        .iter()
        .map(|adaptation| normalize_adaptation(adaptation, period, &bases, &protection, duration))
        .collect::<Result<Vec<_>, Error>>()?;
    if adaptation_sets.is_empty() {
        return Err(Error::Streaming(format!(
            "DASH period {} has no adaptation sets",
            period.id.as_deref().unwrap_or("<unnamed>")
        )));
    }
    Ok(DashPeriod {
        id: period.id.clone(),
        start,
        duration,
        service_descriptions: normalize_service_descriptions(&period.service_description)?,
        adaptation_sets,
    })
}

fn normalize_adaptation(
    adaptation: &AdaptationSet,
    period: &Period,
    parent_bases: &[DashBaseUrl],
    parent_protection: &[DashContentProtection],
    period_duration: Option<Duration>,
) -> Result<DashAdaptationSet, Error> {
    reject_external_xlink("adaptation set", adaptation.href.as_deref())?;
    if adaptation.representations.is_empty() {
        return Err(Error::Streaming(String::from(
            "DASH adaptation set has no representations",
        )));
    }
    let bases = inherit_base_urls(parent_bases, &adaptation.BaseURL)?;
    let kind = infer_track_kind(adaptation)?;
    let mut protection = parent_protection.to_vec();
    extend_unique(
        &mut protection,
        normalize_protection(&bases[0].url, &adaptation.ContentProtection)?,
    );
    let representations = adaptation
        .representations
        .iter()
        .map(|representation| {
            normalize_representation(
                representation,
                adaptation,
                period,
                &bases,
                &protection,
                period_duration,
            )
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(DashAdaptationSet {
        id: adaptation.id.clone(),
        kind,
        language: adaptation.lang.clone(),
        roles: adaptation
            .Role
            .iter()
            .filter_map(|role| role.value.clone())
            .collect(),
        representations,
    })
}

fn normalize_representation(
    representation: &Representation,
    adaptation: &AdaptationSet,
    period: &Period,
    parent_bases: &[DashBaseUrl],
    parent_protection: &[DashContentProtection],
    period_duration: Option<Duration>,
) -> Result<DashRepresentation, Error> {
    reject_external_xlink("representation", representation.href.as_deref())?;
    let id = representation
        .id
        .clone()
        .ok_or_else(|| Error::Streaming(String::from("DASH representation has no identifier")))?;
    let bandwidth = NonZeroU64::new(representation.bandwidth.unwrap_or(0)).ok_or_else(|| {
        Error::Streaming(format!("DASH representation {id:?} has zero bandwidth"))
    })?;
    let bases = inherit_base_urls(parent_bases, &representation.BaseURL)?;
    let mut protection = parent_protection.to_vec();
    extend_unique(
        &mut protection,
        normalize_protection(&bases[0].url, &representation.ContentProtection)?,
    );
    let mime_type = representation
        .mimeType
        .as_ref()
        .or(adaptation.mimeType.as_ref())
        .cloned()
        .ok_or_else(|| Error::Streaming(format!("DASH representation {id:?} has no MIME type")))?;
    let width = representation.width.or(adaptation.width);
    let height = representation.height.or(adaptation.height);
    let dimensions = match (width, height) {
        (Some(width), Some(height)) => Some((
            u32::try_from(width).map_err(|_| {
                Error::Streaming(format!("DASH representation {id:?} width exceeds u32"))
            })?,
            u32::try_from(height).map_err(|_| {
                Error::Streaming(format!("DASH representation {id:?} height exceeds u32"))
            })?,
        )),
        (None, None) => None,
        _ => {
            return Err(Error::Streaming(format!(
                "DASH representation {id:?} declares only one dimension"
            )));
        }
    };
    let frame_rate = representation
        .frameRate
        .as_ref()
        .or(adaptation.frameRate.as_ref())
        .map(|rate| parse_frame_rate(rate))
        .transpose()?;
    let codecs = representation
        .codecs
        .as_ref()
        .or(adaptation.codecs.as_ref())
        .map(|codecs| {
            codecs
                .split(',')
                .map(str::trim)
                .filter(|codec| !codec.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let segments = normalize_segments(
        representation,
        adaptation,
        period,
        &bases,
        &id,
        bandwidth,
        period_duration,
    )?;
    let timescale = segment_timescale(&segments);
    let mut producer_reference_times =
        normalize_producer_reference_times(&adaptation.ProducerReferenceTime, timescale)?;
    extend_unique(
        &mut producer_reference_times,
        normalize_producer_reference_times(&representation.ProducerReferenceTime, timescale)?,
    );
    Ok(DashRepresentation {
        id,
        bandwidth,
        dimensions,
        frame_rate,
        codecs,
        mime_type,
        content_protection: protection,
        producer_reference_times,
        segments,
    })
}

fn normalize_segments(
    representation: &Representation,
    adaptation: &AdaptationSet,
    period: &Period,
    bases: &[DashBaseUrl],
    id: &str,
    bandwidth: NonZeroU64,
    _period_duration: Option<Duration>,
) -> Result<DashSegmentSource, Error> {
    let templates = [
        period.SegmentTemplate.as_ref(),
        adaptation.SegmentTemplate.as_ref(),
        representation.SegmentTemplate.as_ref(),
    ];
    let lists = [
        period.SegmentList.as_ref(),
        adaptation.SegmentList.as_ref(),
        representation.SegmentList.as_ref(),
    ];
    let segment_bases = [
        period.SegmentBase.as_ref(),
        adaptation.SegmentBase.as_ref(),
        representation.SegmentBase.as_ref(),
    ];
    let has_templates = templates.into_iter().flatten().next().is_some();
    let has_lists = lists.into_iter().flatten().next().is_some();
    let has_segment_bases = segment_bases.into_iter().flatten().next().is_some();
    let present =
        usize::from(has_templates) + usize::from(has_lists) + usize::from(has_segment_bases);
    if present != 1 {
        return Err(Error::Streaming(format!(
            "DASH representation {id:?} must inherit exactly one segment addressing model"
        )));
    }
    if has_templates {
        return normalize_template(templates, bases, id, bandwidth)
            .map(DashSegmentSource::Template);
    }
    if let Some(list) = lists.into_iter().rev().flatten().next() {
        return normalize_list(list, bases).map(DashSegmentSource::List);
    }
    let segment_base = segment_bases
        .into_iter()
        .rev()
        .flatten()
        .next()
        .expect("validated segment source must exist");
    normalize_segment_base(segment_base, bases).map(DashSegmentSource::Base)
}

fn normalize_template(
    levels: [Option<&SegmentTemplate>; 3],
    bases: &[DashBaseUrl],
    id: &str,
    bandwidth: NonZeroU64,
) -> Result<DashSegmentTemplate, Error> {
    let (base_offset, base_complete) = common_base_availability(bases)?;
    let media = inherited(&levels, |template| template.media.as_ref())
        .cloned()
        .ok_or_else(|| {
            Error::Streaming(format!("DASH representation {id:?} has no media template"))
        })?;
    let initialization = inherited(&levels, |template| template.initialization.as_ref()).cloned();
    let index = inherited(&levels, |template| template.index.as_ref()).cloned();
    let timescale = NonZeroU64::new(
        inherited(&levels, |template| template.timescale.as_ref())
            .copied()
            .unwrap_or(1),
    )
    .ok_or_else(|| Error::Streaming(String::from("DASH template timescale must be non-zero")))?;
    let duration = inherited(&levels, |template| template.duration.as_ref())
        .copied()
        .map(parse_template_duration)
        .transpose()?;
    let start_number = inherited(&levels, |template| template.startNumber.as_ref())
        .copied()
        .unwrap_or(1);
    let presentation_time_offset =
        inherited(&levels, |template| template.presentationTimeOffset.as_ref())
            .copied()
            .unwrap_or(0);
    let timeline = inherited(&levels, |template| template.SegmentTimeline.as_ref())
        .map(normalize_timeline)
        .transpose()?
        .unwrap_or_default();
    let template_offset = inherited(&levels, |template| template.availabilityTimeOffset.as_ref())
        .copied()
        .map(parse_availability_time_offset)
        .transpose()?
        .unwrap_or(DashAvailabilityTimeOffset::ZERO);
    let availability_time_offset = base_offset.checked_add(template_offset)?;
    let availability_time_complete = inherited(&levels, |template| {
        template.availabilityTimeComplete.as_ref()
    })
    .copied()
    .unwrap_or(base_complete);
    if duration.is_none() && timeline.is_empty() {
        return Err(Error::Streaming(format!(
            "DASH representation {id:?} template has neither duration nor timeline"
        )));
    }
    validate_template_tokens(&media, id, bandwidth, true)?;
    if let Some(initialization) = &initialization {
        validate_template_tokens(initialization, id, bandwidth, false)?;
    }
    if let Some(index) = &index {
        validate_template_tokens(index, id, bandwidth, true)?;
    }
    Ok(DashSegmentTemplate {
        base_urls: bases.to_vec(),
        media,
        initialization,
        index,
        timescale,
        duration,
        start_number,
        presentation_time_offset,
        timeline,
        availability_time_offset,
        availability_time_complete,
    })
}

fn normalize_list(list: &SegmentList, bases: &[DashBaseUrl]) -> Result<DashSegmentList, Error> {
    reject_external_xlink("segment list", list.href.as_deref())?;
    if list.segment_urls.is_empty() {
        return Err(Error::Streaming(String::from(
            "DASH SegmentList has no media segments",
        )));
    }
    let timescale = NonZeroU64::new(list.timescale.unwrap_or(1))
        .ok_or_else(|| Error::Streaming(String::from("DASH SegmentList timescale is zero")))?;
    let duration = list
        .duration
        .map(|duration| {
            NonZeroU64::new(duration)
                .ok_or_else(|| Error::Streaming(String::from("DASH SegmentList duration is zero")))
        })
        .transpose()?;
    let presentation_time_offset = 0;
    let initialization = list
        .Initialization
        .as_ref()
        .map(|initialization| normalize_initialization(initialization, bases))
        .transpose()?;
    let segments = list
        .segment_urls
        .iter()
        .map(|segment| {
            let media = segment.media.as_deref().ok_or_else(|| {
                Error::Streaming(String::from("DASH SegmentURL has no media URL"))
            })?;
            Ok(DashSegmentReference {
                urls: resolve_candidates(bases, media)?,
                byte_range: segment
                    .mediaRange
                    .as_deref()
                    .map(parse_byte_range)
                    .transpose()?,
                index_urls: segment
                    .index
                    .as_deref()
                    .map(|index| resolve_candidates(bases, index))
                    .transpose()?
                    .unwrap_or_default(),
                index_range: segment
                    .indexRange
                    .as_deref()
                    .map(parse_byte_range)
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let timeline = list
        .SegmentTimeline
        .as_ref()
        .map(normalize_timeline)
        .transpose()?
        .unwrap_or_default();
    let (availability_time_offset, availability_time_complete) = common_base_availability(bases)?;
    Ok(DashSegmentList {
        timescale,
        duration,
        presentation_time_offset,
        initialization,
        segments,
        timeline,
        availability_time_offset,
        availability_time_complete,
    })
}

fn normalize_segment_base(
    segment: &SegmentBase,
    bases: &[DashBaseUrl],
) -> Result<DashSegmentBase, Error> {
    let (base_offset, base_complete) = common_base_availability(bases)?;
    let segment_offset = segment
        .availabilityTimeOffset
        .map(parse_availability_time_offset)
        .transpose()?
        .unwrap_or(DashAvailabilityTimeOffset::ZERO);
    let timescale = NonZeroU64::new(segment.timescale.unwrap_or(1))
        .ok_or_else(|| Error::Streaming(String::from("DASH SegmentBase timescale is zero")))?;
    Ok(DashSegmentBase {
        media_urls: bases.iter().map(|base| base.url.clone()).collect(),
        initialization: segment
            .Initialization
            .as_ref()
            .map(|initialization| normalize_initialization(initialization, bases))
            .transpose()?,
        index_range: segment
            .indexRange
            .as_deref()
            .map(parse_byte_range)
            .transpose()?,
        timescale,
        presentation_time_offset: segment.presentationTimeOffset.unwrap_or(0),
        availability_time_offset: base_offset.checked_add(segment_offset)?,
        availability_time_complete: segment.availabilityTimeComplete.unwrap_or(base_complete),
    })
}

fn normalize_initialization(
    initialization: &dash_mpd::Initialization,
    bases: &[DashBaseUrl],
) -> Result<DashInitialization, Error> {
    let urls = match initialization.sourceURL.as_deref() {
        Some(source) => resolve_candidates(bases, source)?,
        None => bases.iter().map(|base| base.url.clone()).collect(),
    };
    Ok(DashInitialization {
        urls,
        byte_range: initialization
            .range
            .as_deref()
            .map(parse_byte_range)
            .transpose()?,
    })
}

fn normalize_timeline(
    timeline: &dash_mpd::SegmentTimeline,
) -> Result<Vec<DashTimelineEntry>, Error> {
    timeline
        .segments
        .iter()
        .map(|segment| {
            if segment.r.unwrap_or(0) < -1 {
                return Err(Error::Streaming(format!(
                    "DASH timeline repeat {} is below -1",
                    segment.r.unwrap_or(0)
                )));
            }
            Ok(DashTimelineEntry {
                start: segment.t,
                duration: NonZeroU64::new(segment.d).ok_or_else(|| {
                    Error::Streaming(String::from("DASH timeline segment duration is zero"))
                })?,
                repeat: segment.r.unwrap_or(0),
            })
        })
        .collect()
}

fn normalize_protection(
    base_url: &Url,
    protection: &[ContentProtection],
) -> Result<Vec<DashContentProtection>, Error> {
    protection
        .iter()
        .map(|item| {
            let default_key_ids = item
                .default_KID
                .as_deref()
                .map(|ids| {
                    ids.split_whitespace()
                        .map(|id| {
                            Uuid::parse_str(id).map_err(|error| {
                                Error::Streaming(format!(
                                    "invalid DASH default_KID {id:?}: {error}"
                                ))
                            })
                        })
                        .collect::<Result<Vec<_>, Error>>()
                })
                .transpose()?
                .unwrap_or_default();
            let pssh = item
                .cenc_pssh
                .iter()
                .filter_map(|pssh| pssh.content.as_deref())
                .map(|pssh| {
                    STANDARD.decode(pssh.trim()).map_err(|error| {
                        Error::Streaming(format!("invalid DASH CENC PSSH: {error}"))
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let license_urls = [item.laurl.as_ref(), item.clearkey_laurl.as_ref()]
                .into_iter()
                .flatten()
                .filter_map(|license| license.content.as_deref())
                .map(|license| {
                    base_url.join(license).map_err(|error| {
                        Error::Streaming(format!("invalid DASH license URL {license:?}: {error}"))
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            Ok(DashContentProtection {
                scheme_id_uri: item.schemeIdUri.clone(),
                value: item.value.clone(),
                default_key_ids,
                pssh,
                license_urls,
            })
        })
        .collect()
}

fn normalize_service_descriptions(
    descriptions: &[dash_mpd::ServiceDescription],
) -> Result<Vec<DashServiceDescription>, Error> {
    descriptions
        .iter()
        .map(|description| {
            let latency = description
                .Latency
                .iter()
                .map(|latency| {
                    let min = latency
                        .min
                        .map(|value| parse_milliseconds("latency min", value))
                        .transpose()?;
                    let target = latency
                        .target
                        .map(|value| parse_milliseconds("latency target", value))
                        .transpose()?;
                    let max = latency
                        .max
                        .map(|value| parse_milliseconds("latency max", value))
                        .transpose()?;
                    validate_ordered_bounds("DASH latency", min, target, max)?;
                    Ok(DashLatency {
                        reference_id: latency.referenceId.clone(),
                        min,
                        target,
                        max,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let playback_rates = description
                .PlaybackRate
                .iter()
                .map(|rate| {
                    let min = rate.min.map(parse_playback_rate).transpose()?;
                    let max = rate.max.map(parse_playback_rate).transpose()?;
                    if min.zip(max).is_some_and(|(min, max)| min > max) {
                        return Err(Error::Streaming(String::from(
                            "DASH playback-rate minimum exceeds maximum",
                        )));
                    }
                    Ok(DashPlaybackRateRange { min, max })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            Ok(DashServiceDescription {
                id: description.id.clone(),
                latency,
                playback_rates,
            })
        })
        .collect()
}

fn normalize_producer_reference_times(
    references: &[dash_mpd::ProducerReferenceTime],
    timescale: NonZeroU64,
) -> Result<Vec<DashProducerReferenceTime>, Error> {
    references
        .iter()
        .map(|reference| {
            let id = reference.id.clone().ok_or_else(|| {
                Error::Streaming(String::from("DASH ProducerReferenceTime has no id"))
            })?;
            let presentation_time = reference.presentationTime.ok_or_else(|| {
                Error::Streaming(format!(
                    "DASH ProducerReferenceTime {id:?} has no presentationTime"
                ))
            })?;
            let wall_clock_time = reference.wallClockTime.as_ref().ok_or_else(|| {
                Error::Streaming(format!(
                    "DASH ProducerReferenceTime {id:?} has no wallClockTime"
                ))
            })?;
            Ok(DashProducerReferenceTime {
                id,
                inband: reference.inband.unwrap_or(false),
                presentation_time,
                wall_clock_time: xs_datetime_to_system_time(wall_clock_time)?,
                timescale,
            })
        })
        .collect()
}

const fn segment_timescale(segments: &DashSegmentSource) -> NonZeroU64 {
    match segments {
        DashSegmentSource::Template(template) => template.timescale,
        DashSegmentSource::List(list) => list.timescale,
        DashSegmentSource::Base(base) => base.timescale,
    }
}

fn common_base_availability(
    bases: &[DashBaseUrl],
) -> Result<(DashAvailabilityTimeOffset, bool), Error> {
    let first = bases.first().ok_or_else(|| {
        Error::Streaming(String::from("DASH representation has no effective BaseURL"))
    })?;
    if bases.iter().skip(1).any(|base| {
        base.availability_time_offset != first.availability_time_offset
            || base.availability_time_complete != first.availability_time_complete
    }) {
        return Err(Error::Streaming(String::from(
            "DASH alternative BaseURLs disagree on availability timing",
        )));
    }
    Ok((
        first.availability_time_offset,
        first.availability_time_complete,
    ))
}

fn parse_availability_time_offset(value: f64) -> Result<DashAvailabilityTimeOffset, Error> {
    if value == f64::INFINITY {
        return Ok(DashAvailabilityTimeOffset::Infinite);
    }
    parse_seconds("availabilityTimeOffset", value).map(DashAvailabilityTimeOffset::Finite)
}

fn parse_milliseconds(kind: &str, value: f64) -> Result<Duration, Error> {
    parse_seconds(kind, value / 1_000.0)
}

fn parse_seconds(kind: &str, value: f64) -> Result<Duration, Error> {
    if !value.is_finite() || value < 0.0 {
        return Err(Error::Streaming(format!(
            "DASH {kind} must be finite and non-negative"
        )));
    }
    Duration::try_from_secs_f64(value)
        .map_err(|error| Error::Streaming(format!("DASH {kind} exceeds Duration: {error}")))
}

fn parse_playback_rate(value: f64) -> Result<DashPlaybackRate, Error> {
    if !value.is_finite() || value <= 0.0 {
        return Err(Error::Streaming(String::from(
            "DASH playback rate must be finite and positive",
        )));
    }
    let parts_per_million = (value * 1_000_000.0).round().to_u32().ok_or_else(|| {
        Error::Streaming(format!(
            "DASH playback rate {value} exceeds supported precision"
        ))
    })?;
    NonZeroU32::new(parts_per_million)
        .map(DashPlaybackRate)
        .ok_or_else(|| Error::Streaming(String::from("DASH playback rate rounds to zero")))
}

fn validate_ordered_bounds(
    kind: &str,
    min: Option<Duration>,
    target: Option<Duration>,
    max: Option<Duration>,
) -> Result<(), Error> {
    if min.zip(target).is_some_and(|(min, target)| min > target)
        || target.zip(max).is_some_and(|(target, max)| target > max)
        || min.zip(max).is_some_and(|(min, max)| min > max)
    {
        return Err(Error::Streaming(format!(
            "{kind} constraints are not monotonically ordered"
        )));
    }
    Ok(())
}

fn inherit_base_urls(
    parents: &[DashBaseUrl],
    children: &[BaseURL],
) -> Result<Vec<DashBaseUrl>, Error> {
    if children.is_empty() {
        return Ok(parents.to_vec());
    }
    let mut bases = Vec::with_capacity(parents.len().saturating_mul(children.len()));
    for parent in parents {
        for child in children {
            let weight = u64::try_from(child.weight.unwrap_or(1))
                .ok()
                .and_then(NonZeroU64::new)
                .ok_or_else(|| {
                    Error::Streaming(String::from("DASH BaseURL weight must be positive"))
                })?;
            let mut priorities = parent.priorities.clone();
            priorities.push(child.priority.unwrap_or(1));
            let child_offset = child
                .availability_time_offset
                .map(parse_availability_time_offset)
                .transpose()?
                .unwrap_or(DashAvailabilityTimeOffset::ZERO);
            bases.push(DashBaseUrl {
                url: parent.url.join(&child.base).map_err(|error| {
                    Error::Streaming(format!("invalid DASH BaseURL {:?}: {error}", child.base))
                })?,
                service_location: child
                    .serviceLocation
                    .clone()
                    .or_else(|| parent.service_location.clone()),
                priorities,
                weight,
                availability_time_offset: parent
                    .availability_time_offset
                    .checked_add(child_offset)?,
                availability_time_complete: child
                    .availability_time_complete
                    .unwrap_or(parent.availability_time_complete),
            });
        }
    }
    bases.sort_by(|left, right| left.priorities.cmp(&right.priorities));
    Ok(bases)
}

fn resolve_candidates(bases: &[DashBaseUrl], relative: &str) -> Result<Vec<Url>, Error> {
    bases
        .iter()
        .map(|base| {
            base.url.join(relative).map_err(|error| {
                Error::Streaming(format!("invalid DASH media URL {relative:?}: {error}"))
            })
        })
        .collect()
}

fn infer_track_kind(adaptation: &AdaptationSet) -> Result<DashTrackKind, Error> {
    let declared = adaptation
        .contentType
        .as_deref()
        .or_else(|| {
            adaptation
                .mimeType
                .as_deref()
                .and_then(|mime| mime.split_once('/').map(|v| v.0))
        })
        .or_else(|| {
            adaptation
                .representations
                .iter()
                .find_map(|representation| {
                    representation.contentType.as_deref().or_else(|| {
                        representation
                            .mimeType
                            .as_deref()
                            .and_then(|mime| mime.split_once('/').map(|v| v.0))
                    })
                })
        });
    match declared {
        Some("video") => Ok(DashTrackKind::Video),
        Some("audio") => Ok(DashTrackKind::Audio),
        Some("text" | "application" | "image") => Ok(DashTrackKind::Subtitle),
        Some(kind) => Err(Error::Unsupported(format!(
            "unsupported DASH content type {kind:?}"
        ))),
        None => Err(Error::Streaming(String::from(
            "DASH adaptation set has no inferable content type",
        ))),
    }
}

fn parse_frame_rate(input: &str) -> Result<(u32, NonZeroU64), Error> {
    let (numerator, denominator) = input.split_once('/').unwrap_or((input, "1"));
    let numerator = numerator
        .parse::<u32>()
        .map_err(|error| Error::Streaming(format!("invalid DASH frame rate {input:?}: {error}")))?;
    if numerator == 0 {
        return Err(Error::Streaming(String::from("DASH frame rate is zero")));
    }
    let denominator = denominator
        .parse::<u64>()
        .map_err(|error| Error::Streaming(format!("invalid DASH frame rate {input:?}: {error}")))?;
    Ok((
        numerator,
        NonZeroU64::new(denominator)
            .ok_or_else(|| Error::Streaming(String::from("DASH frame-rate denominator is zero")))?,
    ))
}

fn parse_byte_range(input: &str) -> Result<MediaByteRange, Error> {
    let (start, inclusive_end) = input
        .split_once('-')
        .ok_or_else(|| Error::Streaming(format!("malformed DASH byte range {input:?}")))?;
    let start = start
        .parse::<u64>()
        .map_err(|error| Error::Streaming(format!("invalid DASH byte-range start: {error}")))?;
    let inclusive_end = inclusive_end
        .parse::<u64>()
        .map_err(|error| Error::Streaming(format!("invalid DASH byte-range end: {error}")))?;
    let end_exclusive = inclusive_end
        .checked_add(1)
        .ok_or_else(|| Error::Streaming(String::from("DASH byte-range end overflowed u64")))?;
    MediaByteRange::new(start, end_exclusive)
}

fn parse_template_duration(duration: f64) -> Result<NonZeroU64, Error> {
    if !duration.is_finite() || duration <= 0.0 || duration.fract() != 0.0 {
        return Err(Error::Streaming(format!(
            "DASH template duration {duration} is not a positive integer"
        )));
    }
    NonZeroU64::new(duration.to_u64().ok_or_else(|| {
        Error::Streaming(format!("DASH template duration {duration} exceeds u64"))
    })?)
    .ok_or_else(|| Error::Streaming(String::from("DASH template duration is zero")))
}

fn plan_template(
    template: &DashSegmentTemplate,
    representation_id: &str,
    bandwidth: NonZeroU64,
    period_duration: Option<Duration>,
) -> Result<Vec<DashPlannedSegment>, Error> {
    let ticks = if template.timeline.is_empty() {
        let duration = template
            .duration
            .expect("validated template duration must exist");
        let period_duration = period_duration.ok_or_else(|| {
            Error::Streaming(String::from(
                "constant-duration DASH template requires a known period or availability-window duration",
            ))
        })?;
        let total = duration_to_ticks_ceil(period_duration, template.timescale)?;
        let count = total.div_ceil(duration.get());
        (0..count)
            .map(|index| {
                index
                    .checked_mul(duration.get())
                    .map(|start| (start, duration.get()))
                    .ok_or_else(|| {
                        Error::Streaming(String::from("DASH template timeline overflow"))
                    })
            })
            .collect::<Result<Vec<_>, Error>>()?
    } else {
        expand_timeline(
            &template.timeline,
            template.timescale,
            period_duration,
            None,
        )?
    };
    ticks
        .into_iter()
        .enumerate()
        .map(|(index, (start, duration))| {
            let index = u64::try_from(index)
                .map_err(|_| Error::Streaming(String::from("DASH segment count exceeds u64")))?;
            render_template_segment(
                template,
                representation_id,
                bandwidth,
                index,
                start,
                duration,
            )
        })
        .collect()
}

fn plan_constant_template_window(
    template: &DashSegmentTemplate,
    representation_id: &str,
    bandwidth: NonZeroU64,
    window_start: Duration,
    window_end: Duration,
) -> Result<Vec<DashPlannedSegment>, Error> {
    let segment_duration = template
        .duration
        .ok_or_else(|| Error::Streaming(String::from("DASH template duration is missing")))?;
    let first_index =
        duration_to_ticks_floor(window_start, template.timescale)? / segment_duration.get();
    let end_ticks = duration_to_ticks_ceil(window_end, template.timescale)?;
    let end_index = end_ticks.div_ceil(segment_duration.get());
    (first_index..end_index)
        .map(|index| {
            let start = index
                .checked_mul(segment_duration.get())
                .ok_or_else(|| Error::Streaming(String::from("DASH template start overflow")))?;
            render_template_segment(
                template,
                representation_id,
                bandwidth,
                index,
                start,
                segment_duration.get(),
            )
        })
        .collect()
}

fn render_template_segment(
    template: &DashSegmentTemplate,
    representation_id: &str,
    bandwidth: NonZeroU64,
    index: u64,
    start: u64,
    duration: u64,
) -> Result<DashPlannedSegment, Error> {
    let number = template
        .start_number
        .checked_add(index)
        .ok_or_else(|| Error::Streaming(String::from("DASH segment number overflowed u64")))?;
    let media = render_template(
        &template.media,
        representation_id,
        bandwidth,
        Some(number),
        Some(start),
    )?;
    let index_urls = template
        .index
        .as_ref()
        .map(|index| {
            let index = render_template(
                index,
                representation_id,
                bandwidth,
                Some(number),
                Some(start),
            )?;
            resolve_candidates(&template.base_urls, &index)
        })
        .transpose()?
        .unwrap_or_default();
    Ok(DashPlannedSegment {
        number,
        start: ticks_to_duration(start, template.timescale),
        duration: ticks_to_duration(duration, template.timescale),
        resource: DashSegmentReference {
            urls: resolve_candidates(&template.base_urls, &media)?,
            byte_range: None,
            index_urls,
            index_range: None,
        },
    })
}

fn plan_list(
    list: &DashSegmentList,
    period_duration: Option<Duration>,
) -> Result<Vec<DashPlannedSegment>, Error> {
    let ticks = if list.timeline.is_empty() {
        let duration = list.duration.ok_or_else(|| {
            Error::Streaming(String::from(
                "DASH SegmentList has neither duration nor timeline",
            ))
        })?;
        (0..list.segments.len())
            .map(|index| {
                u64::try_from(index)
                    .ok()
                    .and_then(|index| index.checked_mul(duration.get()))
                    .map(|start| (start, duration.get()))
                    .ok_or_else(|| {
                        Error::Streaming(String::from("DASH SegmentList timeline overflow"))
                    })
            })
            .collect::<Result<Vec<_>, Error>>()?
    } else {
        expand_timeline(
            &list.timeline,
            list.timescale,
            period_duration,
            Some(list.segments.len()),
        )?
    };
    list.segments
        .iter()
        .cloned()
        .zip(ticks)
        .enumerate()
        .map(|(index, (resource, (start, duration)))| {
            let number = u64::try_from(index)
                .map_err(|_| Error::Streaming(String::from("DASH segment count exceeds u64")))?
                .checked_add(1)
                .ok_or_else(|| {
                    Error::Streaming(String::from("DASH segment number overflowed u64"))
                })?;
            Ok(DashPlannedSegment {
                number,
                start: ticks_to_duration(start, list.timescale),
                duration: ticks_to_duration(duration, list.timescale),
                resource,
            })
        })
        .collect()
}

fn expand_timeline(
    timeline: &[DashTimelineEntry],
    timescale: NonZeroU64,
    period_duration: Option<Duration>,
    maximum_count: Option<usize>,
) -> Result<Vec<(u64, u64)>, Error> {
    let period_end = period_duration
        .map(|duration| duration_to_ticks_ceil(duration, timescale))
        .transpose()?;
    let mut expanded = Vec::new();
    let mut cursor = 0_u64;
    for (index, entry) in timeline.iter().enumerate() {
        let start = entry.start.unwrap_or(cursor);
        if start < cursor {
            return Err(Error::Streaming(String::from(
                "DASH timeline moves backwards",
            )));
        }
        let count = if entry.repeat >= 0 {
            u64::try_from(entry.repeat)
                .expect("non-negative timeline repeat must fit u64")
                .checked_add(1)
                .expect("timeline repeat count must fit u64")
        } else if let Some(boundary) = timeline.get(index + 1).and_then(|next| next.start) {
            repeated_count_to_boundary(start, entry.duration, boundary)?
        } else if let Some(maximum_count) = maximum_count {
            u64::try_from(maximum_count.saturating_sub(expanded.len()))
                .map_err(|_| Error::Streaming(String::from("DASH segment count exceeds u64")))?
        } else {
            repeated_count_to_boundary(
                start,
                entry.duration,
                period_end.ok_or_else(|| {
                    Error::Streaming(String::from(
                        "open-ended DASH timeline requires a following start or known period duration",
                    ))
                })?,
            )?
        };
        if count == 0 {
            return Err(Error::Streaming(String::from(
                "DASH timeline run expands to zero segments",
            )));
        }
        for repetition in 0..count {
            let offset = repetition
                .checked_mul(entry.duration.get())
                .ok_or_else(|| {
                    Error::Streaming(String::from("DASH timeline offset overflowed u64"))
                })?;
            let segment_start = start.checked_add(offset).ok_or_else(|| {
                Error::Streaming(String::from("DASH timeline start overflowed u64"))
            })?;
            expanded.push((segment_start, entry.duration.get()));
        }
        cursor = start
            .checked_add(count.checked_mul(entry.duration.get()).ok_or_else(|| {
                Error::Streaming(String::from("DASH timeline duration overflowed u64"))
            })?)
            .ok_or_else(|| Error::Streaming(String::from("DASH timeline end overflowed u64")))?;
    }
    if let Some(maximum_count) = maximum_count
        && expanded.len() != maximum_count
    {
        return Err(Error::Streaming(format!(
            "DASH timeline describes {} segments but SegmentList contains {maximum_count}",
            expanded.len()
        )));
    }
    Ok(expanded)
}

fn repeated_count_to_boundary(
    start: u64,
    duration: NonZeroU64,
    boundary: u64,
) -> Result<u64, Error> {
    let span = boundary.checked_sub(start).ok_or_else(|| {
        Error::Streaming(String::from(
            "DASH timeline repeat boundary precedes its start",
        ))
    })?;
    if span % duration.get() != 0 {
        return Err(Error::Streaming(format!(
            "DASH timeline repeat span {span} is not divisible by duration {}",
            duration.get()
        )));
    }
    Ok(span / duration.get())
}

fn duration_to_ticks_ceil(duration: Duration, timescale: NonZeroU64) -> Result<u64, Error> {
    let scale = u128::from(timescale.get());
    let seconds = u128::from(duration.as_secs())
        .checked_mul(scale)
        .ok_or_else(|| Error::Streaming(String::from("DASH duration overflowed timescale")))?;
    let fractional = u128::from(duration.subsec_nanos())
        .checked_mul(scale)
        .ok_or_else(|| Error::Streaming(String::from("DASH duration overflowed timescale")))?
        .div_ceil(1_000_000_000);
    u64::try_from(seconds + fractional)
        .map_err(|_| Error::Streaming(String::from("DASH duration ticks exceed u64")))
}

fn duration_to_ticks_floor(duration: Duration, timescale: NonZeroU64) -> Result<u64, Error> {
    let scale = u128::from(timescale.get());
    let seconds = u128::from(duration.as_secs())
        .checked_mul(scale)
        .ok_or_else(|| Error::Streaming(String::from("DASH duration overflowed timescale")))?;
    let fractional = u128::from(duration.subsec_nanos())
        .checked_mul(scale)
        .ok_or_else(|| Error::Streaming(String::from("DASH duration overflowed timescale")))?
        / 1_000_000_000;
    u64::try_from(seconds + fractional)
        .map_err(|_| Error::Streaming(String::from("DASH duration ticks exceed u64")))
}

fn ticks_to_duration(ticks: u64, timescale: NonZeroU64) -> Duration {
    let seconds = ticks / timescale.get();
    let remainder = ticks % timescale.get();
    let nanos = u128::from(remainder) * 1_000_000_000 / u128::from(timescale.get());
    let nanos = u32::try_from(nanos)
        .expect("fractional timescale remainder is always below one billion nanoseconds");
    Duration::new(seconds, nanos)
}

fn validate_template_tokens(
    template: &str,
    representation_id: &str,
    bandwidth: NonZeroU64,
    accepts_segment_tokens: bool,
) -> Result<(), Error> {
    let (number, time) = if accepts_segment_tokens {
        (Some(1), Some(0))
    } else {
        (None, None)
    };
    render_template(template, representation_id, bandwidth, number, time).map(drop)
}

fn render_template(
    template: &str,
    representation_id: &str,
    bandwidth: NonZeroU64,
    number: Option<u64>,
    time: Option<u64>,
) -> Result<String, Error> {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((literal, after_dollar)) = rest.split_once('$') {
        rendered.push_str(literal);
        if let Some(after_escape) = after_dollar.strip_prefix('$') {
            rendered.push('$');
            rest = after_escape;
            continue;
        }
        let (token, after_token) = after_dollar.split_once('$').ok_or_else(|| {
            Error::Streaming(format!("unterminated DASH template token in {template:?}"))
        })?;
        let value = match token {
            "RepresentationID" => representation_id.to_owned(),
            "Bandwidth" => bandwidth.to_string(),
            "Number" => number
                .map(|value| value.to_string())
                .ok_or_else(|| Error::Streaming(String::from("DASH template requires Number")))?,
            "Time" => time
                .map(|value| value.to_string())
                .ok_or_else(|| Error::Streaming(String::from("DASH template requires Time")))?,
            token => render_formatted_token(token, number, time)?,
        };
        rendered.push_str(&value);
        rest = after_token;
    }
    rendered.push_str(rest);
    Ok(rendered)
}

fn render_formatted_token(
    token: &str,
    number: Option<u64>,
    time: Option<u64>,
) -> Result<String, Error> {
    let (name, format) = token
        .split_once('%')
        .ok_or_else(|| Error::Unsupported(format!("unsupported DASH template token ${token}$")))?;
    let width = format
        .strip_prefix('0')
        .and_then(|format| format.strip_suffix('d'))
        .ok_or_else(|| Error::Unsupported(format!("unsupported DASH template format %{format}")))?
        .parse::<usize>()
        .map_err(|error| Error::Streaming(format!("invalid DASH template width: {error}")))?;
    let value = match name {
        "Number" => number,
        "Time" => time,
        _ => None,
    }
    .ok_or_else(|| Error::Unsupported(format!("unsupported DASH template token ${token}$")))?;
    Ok(format!("{value:0width$}"))
}

fn inherited<'a, T>(
    levels: &'a [Option<&SegmentTemplate>; 3],
    field: impl Fn(&'a SegmentTemplate) -> Option<&'a T>,
) -> Option<&'a T> {
    levels.iter().rev().flatten().find_map(|level| field(level))
}

fn reject_external_xlink(kind: &str, href: Option<&str>) -> Result<(), Error> {
    if let Some(href) = href {
        return Err(Error::Unsupported(format!(
            "external DASH {kind} XLink {href:?} must be resolved before normalization"
        )));
    }
    Ok(())
}

fn extend_unique<T: PartialEq>(target: &mut Vec<T>, values: Vec<T>) {
    for value in values {
        if !target.contains(&value) {
            target.push(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, time::Duration};

    use url::Url;

    use super::{
        DashAvailabilityTimeOffset, DashBaseUrl, DashManifestKind, DashPlaybackRate,
        DashRepresentation, DashSegmentSource, DashSegmentTemplate, DashTrackKind,
        parse_dash_manifest,
    };

    #[test]
    fn parses_tracks_templates_ranges_and_cenc() {
        let base = Url::parse("https://waterui.dev/video/manifest.mpd")
            .expect("test base URL must be valid");
        let manifest = parse_dash_manifest(&base, include_str!("../tests/assets/dash_static.mpd"))
            .expect("test DASH manifest must parse");

        assert_eq!(manifest.kind, DashManifestKind::Static);
        assert_eq!(manifest.periods.len(), 1);
        let video = manifest.periods[0]
            .adaptation_sets
            .iter()
            .find(|adaptation| adaptation.kind == DashTrackKind::Video)
            .expect("video adaptation must exist");
        assert_eq!(video.representations.len(), 2);
        let DashSegmentSource::Template(template) = &video.representations[1].segments else {
            panic!("video representation must use a template");
        };
        assert_eq!(template.timeline.len(), 2);
        assert_eq!(
            template.base_urls[0].url.as_str(),
            "https://waterui.dev/video/cdn/video/"
        );
        assert_eq!(
            video.representations[0].content_protection[0].pssh[0],
            b"pssh"
        );
        let segments = video.representations[1]
            .plan_segments(manifest.periods[0].duration)
            .expect("template must expand");
        assert_eq!(segments.len(), 6);
        assert_eq!(segments[5].start, std::time::Duration::from_secs(10));
        assert_eq!(
            segments[5].resource.urls[0].as_str(),
            "https://waterui.dev/video/cdn/video/1080p-0000010000.m4s"
        );
        assert!(
            video.representations[1]
                .initialization()
                .expect("initialization must resolve")
                .expect("initialization must exist")
                .urls[0]
                .as_str()
                .ends_with("init-1080p.mp4")
        );

        let audio = manifest.periods[0]
            .adaptation_sets
            .iter()
            .find(|adaptation| adaptation.kind == DashTrackKind::Audio)
            .expect("audio adaptation must exist");
        let DashSegmentSource::List(list) = &audio.representations[0].segments else {
            panic!("audio representation must use a list");
        };
        assert_eq!(
            list.segments[0].byte_range.expect("range must exist").len(),
            1_000
        );
        assert_eq!(
            audio.representations[0]
                .plan_segments(manifest.periods[0].duration)
                .expect("segment list must plan")
                .len(),
            2
        );
    }

    #[test]
    fn normalizes_low_latency_dash_timing_policy() {
        let base = Url::parse("https://waterui.dev/video/manifest.mpd")
            .expect("test base URL must be valid");
        let manifest =
            parse_dash_manifest(&base, include_str!("../tests/assets/dash_low_latency.mpd"))
                .expect("low-latency DASH manifest must parse");

        let global = &manifest.service_descriptions[0];
        assert_eq!(global.latency[0].min, Some(Duration::from_millis(1_500)));
        assert_eq!(global.latency[0].target, Some(Duration::from_secs(2)));
        assert_eq!(global.latency[0].max, Some(Duration::from_millis(3_500)));
        assert_eq!(
            global.playback_rates[0].min.map(DashPlaybackRate::as_f64),
            Some(0.95)
        );
        assert_eq!(
            global.playback_rates[0].max.map(DashPlaybackRate::as_f64),
            Some(1.05)
        );
        assert_eq!(
            manifest.periods[0].service_descriptions[0].latency[0].target,
            Some(Duration::from_millis(1_800))
        );

        let representation = &manifest.periods[0].adaptation_sets[0].representations[0];
        assert_eq!(
            representation.availability_time_offset(),
            DashAvailabilityTimeOffset::Finite(Duration::from_secs(2))
        );
        assert!(!representation.availability_time_complete());
        assert_eq!(representation.producer_reference_times.len(), 2);
        assert_eq!(
            representation.producer_reference_times[0].timescale,
            NonZeroU64::new(90_000).expect("test timescale is non-zero")
        );
        assert!(representation.producer_reference_times[0].inband);
    }

    #[test]
    fn plans_only_the_requested_constant_duration_live_window() {
        let representation = DashRepresentation {
            id: String::from("video"),
            bandwidth: NonZeroU64::new(4_000_000).expect("test bandwidth is non-zero"),
            dimensions: Some((3_840, 2_160)),
            frame_rate: Some((60, NonZeroU64::MIN)),
            codecs: vec![String::from("hvc1.2.4.L153.B0")],
            mime_type: String::from("video/mp4"),
            content_protection: Vec::new(),
            producer_reference_times: Vec::new(),
            segments: DashSegmentSource::Template(DashSegmentTemplate {
                base_urls: vec![DashBaseUrl {
                    url: Url::parse("https://waterui.dev/live/").expect("test base URL must parse"),
                    service_location: None,
                    priorities: Vec::new(),
                    weight: NonZeroU64::MIN,
                    availability_time_offset: DashAvailabilityTimeOffset::ZERO,
                    availability_time_complete: true,
                }],
                media: String::from("segment-$Number$.m4s"),
                initialization: Some(String::from("init.mp4")),
                index: None,
                timescale: NonZeroU64::MIN,
                duration: NonZeroU64::new(2),
                start_number: 1,
                presentation_time_offset: 0,
                timeline: Vec::new(),
                availability_time_offset: DashAvailabilityTimeOffset::ZERO,
                availability_time_complete: true,
            }),
        };
        let segments = representation
            .plan_segments_in_window(Duration::from_hours(24), Duration::from_secs(86_406))
            .expect("live window must plan");

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].number, 43_201);
        assert_eq!(segments[0].start, Duration::from_hours(24));
        assert!(
            segments[0].resource.urls[0]
                .path()
                .ends_with("segment-43201.m4s")
        );
    }
}
