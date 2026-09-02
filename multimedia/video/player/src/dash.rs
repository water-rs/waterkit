//! DASH manifest, adaptation, segment, and CMAF playback session.

use std::{
    collections::{BTreeMap, BTreeSet},
    num::{NonZeroU32, NonZeroU64},
    time::{Duration, SystemTime},
};

use num_traits::ToPrimitive as _;
use waterkit_video_container::{
    CmafChunkDemuxer, CmafDemuxer, CmafInitialization, EncodedSample, SubtitleCue, TimedMetadata,
    TrackId, TrackInfo, TrackKind, decode_cmaf_subtitle_sample, parse_pssh_init_data,
    parse_ttml_document, parse_webvtt_document,
};
use waterkit_video_core::{Error, ProtectionInitData};
use waterkit_video_streaming::{
    AdaptiveTrackSelector, DashAdaptationSet, DashAvailabilityTimeOffset, DashInitialization,
    DashManifest, DashManifestKind, DashPlannedSegment, DashRepresentation, DashSegmentSource,
    DashTrackKind, MediaRequest, SegmentLoader, SegmentResource, SegmentStream,
    fetch_dash_manifest,
};

use crate::streaming::{SubtitleTrackSelection, VideoTrackSelection};
use crate::{
    LivePlaybackRateRange, LiveWindow,
    audio_track::SelectableAudioTrack,
    streaming::{AudioTrackSelection, SegmentedPlaybackOptions},
    subtitle_track::SelectableSubtitleTrack,
    video_track::SelectableVideoTrack,
};

/// One fully downloaded and demuxed DASH representation segment.
#[derive(Debug)]
pub struct DashStreamedSegment {
    period_index: usize,
    start: Duration,
    duration: Duration,
    tracks: Vec<TrackInfo>,
    samples: Vec<EncodedSample>,
    timed_metadata: Vec<TimedMetadata>,
    protection_init_data: Vec<ProtectionInitData>,
    representation: DashRepresentation,
    estimated_bits_per_second: NonZeroU64,
    chunk_index: Option<usize>,
    transport_chunks_consumed: usize,
    response_bytes_consumed: usize,
}

/// One fully downloaded and decoded DASH subtitle segment.
#[derive(Debug)]
pub struct DashStreamedSubtitleSegment {
    period_index: usize,
    start: Duration,
    duration: Duration,
    cues: Vec<SubtitleCue>,
    timed_metadata: Vec<TimedMetadata>,
    chunk_index: Option<usize>,
    transport_chunks_consumed: usize,
    response_bytes_consumed: usize,
}

impl DashStreamedSubtitleSegment {
    /// Returns the containing period index.
    #[must_use]
    pub const fn period_index(&self) -> usize {
        self.period_index
    }

    /// Returns the segment start on the complete presentation timeline.
    #[must_use]
    pub const fn start(&self) -> Duration {
        self.start
    }

    /// Returns the manifest-declared segment duration.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.duration
    }

    /// Returns the zero-based CMAF chunk index for a low-latency response.
    #[must_use]
    pub const fn chunk_index(&self) -> Option<usize> {
        self.chunk_index
    }

    /// Returns the number of transport chunks consumed for this output.
    #[must_use]
    pub const fn transport_chunks_consumed(&self) -> usize {
        self.transport_chunks_consumed
    }

    /// Returns cumulative response bytes consumed for this output.
    #[must_use]
    pub const fn response_bytes_consumed(&self) -> usize {
        self.response_bytes_consumed
    }

    /// Returns decoded presentation cues in this segment.
    #[must_use]
    pub fn cues(&self) -> &[SubtitleCue] {
        &self.cues
    }

    /// Returns timed event messages carried beside the subtitle samples.
    #[must_use]
    pub fn timed_metadata(&self) -> &[TimedMetadata] {
        &self.timed_metadata
    }

    /// Consumes the segment into decoded cues and timed event messages.
    #[must_use]
    pub fn into_media(self) -> (Vec<SubtitleCue>, Vec<TimedMetadata>) {
        (self.cues, self.timed_metadata)
    }
}

impl DashStreamedSegment {
    /// Returns the zero-based containing period index.
    #[must_use]
    pub const fn period_index(&self) -> usize {
        self.period_index
    }

    /// Returns the segment start on the complete presentation timeline.
    #[must_use]
    pub const fn start(&self) -> Duration {
        self.start
    }

    /// Returns the manifest-declared segment duration.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.duration
    }

    /// Returns the zero-based CMAF chunk index for a low-latency response.
    #[must_use]
    pub const fn chunk_index(&self) -> Option<usize> {
        self.chunk_index
    }

    /// Returns the number of transport chunks consumed for this output.
    #[must_use]
    pub const fn transport_chunks_consumed(&self) -> usize {
        self.transport_chunks_consumed
    }

    /// Returns cumulative response bytes consumed for this output.
    #[must_use]
    pub const fn response_bytes_consumed(&self) -> usize {
        self.response_bytes_consumed
    }

    /// Returns presentation-wide elementary track descriptors.
    #[must_use]
    pub fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }

    /// Returns coded samples in decode order.
    #[must_use]
    pub fn samples(&self) -> &[EncodedSample] {
        &self.samples
    }

    /// Returns timed event messages carried beside the coded samples.
    #[must_use]
    pub fn timed_metadata(&self) -> &[TimedMetadata] {
        &self.timed_metadata
    }

    /// Consumes the segment into coded samples and timed event messages.
    #[must_use]
    pub fn into_media(self) -> (Vec<EncodedSample>, Vec<TimedMetadata>) {
        (self.samples, self.timed_metadata)
    }

    /// Returns DRM initialization data from the MPD and CMAF initialization segment.
    #[must_use]
    pub fn protection_init_data(&self) -> &[ProtectionInitData] {
        &self.protection_init_data
    }

    /// Returns the selected DASH representation.
    #[must_use]
    pub const fn representation(&self) -> &DashRepresentation {
        &self.representation
    }

    /// Returns the conservative network estimate after this transfer.
    #[must_use]
    pub const fn estimated_bits_per_second(&self) -> NonZeroU64 {
        self.estimated_bits_per_second
    }
}

/// Result of polling the next DASH representation segment.
#[derive(Debug)]
#[non_exhaustive]
pub enum DashSegmentPoll {
    /// One representation segment is ready for decode.
    Ready(Box<DashStreamedSegment>),
    /// A selected subtitle segment is ready for presentation.
    Subtitles(Box<DashStreamedSubtitleSegment>),
    /// A dynamic MPD currently exposes no segment beyond the playback cursor.
    AwaitingManifest {
        /// MPD-declared delay before an ordinary refresh.
        retry_after: Duration,
    },
    /// Every period in a static presentation has been exhausted.
    EndOfStream,
}

#[derive(Debug)]
struct RepresentationState {
    kind: DashTrackKind,
    representation: DashRepresentation,
    period_start: Duration,
    presentation_time_offset: Duration,
    initialization: Option<DashInitialization>,
    segments: Vec<DashPlannedSegment>,
    next_segment: usize,
    demuxer: Option<CmafDemuxer>,
    track_ids: BTreeMap<TrackId, TrackId>,
    tracks: Vec<TrackInfo>,
    protection_init_data: Vec<ProtectionInitData>,
    discontinuity: bool,
    subtitle_format: Option<SubtitleSegmentFormat>,
    active_chunked: Option<ActiveChunkedSegment>,
    last_polled: u64,
}

#[derive(Debug)]
struct ActiveChunkedSegment {
    segment: DashPlannedSegment,
    stream: SegmentStream,
    demuxer: CmafChunkDemuxer,
    next_chunk_index: usize,
    emitted_samples: bool,
    transport_chunks_consumed: usize,
    response_bytes_consumed: usize,
}

#[derive(Debug, Clone, Copy)]
struct SegmentTransfer {
    estimated_bits_per_second: NonZeroU64,
    chunk_index: Option<usize>,
    transport_chunks_consumed: usize,
    response_bytes_consumed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubtitleSegmentFormat {
    Cmaf,
    Ttml,
    WebVtt,
}

impl RepresentationState {
    fn new(
        kind: DashTrackKind,
        representation: DashRepresentation,
        period_start: Duration,
        window_start: Duration,
        window_end: Duration,
        cursor: Duration,
        discontinuity: bool,
    ) -> Result<Self, Error> {
        let protection_init_data = manifest_protection_init_data(&representation)?;
        let subtitle_format = representation_subtitle_format(kind, &representation)?;
        if matches!(
            subtitle_format,
            Some(SubtitleSegmentFormat::Ttml | SubtitleSegmentFormat::WebVtt)
        ) && !representation.availability_time_complete()
        {
            return Err(Error::Unsupported(format!(
                "low-latency DASH subtitle representation {:?} must use CMAF chunks",
                representation.id
            )));
        }
        if kind != DashTrackKind::Subtitle && !representation.mime_type.ends_with("/mp4") {
            return Err(Error::Unsupported(format!(
                "DASH representation {:?} uses unsupported MIME type {:?}",
                representation.id, representation.mime_type
            )));
        }
        let presentation_time_offset = representation.presentation_time_offset();
        let initialization = representation.initialization()?;
        let lookahead = availability_lookahead(&representation, window_start, window_end)?;
        let plan_end = window_end.checked_add(lookahead).ok_or_else(|| {
            Error::Streaming(String::from("DASH availability planning window overflow"))
        })?;
        let mut segments = representation.plan_segments_in_window(window_start, plan_end)?;
        segments.retain(|segment| {
            segment_is_available(
                segment,
                window_end,
                representation.availability_time_offset(),
            )
        });
        let local_cursor = cursor.saturating_sub(period_start);
        let next_segment = segments
            .iter()
            .position(|segment| {
                segment
                    .start
                    .checked_add(segment.duration)
                    .is_some_and(|end| end > local_cursor)
            })
            .unwrap_or(segments.len());
        Ok(Self {
            kind,
            representation,
            period_start,
            presentation_time_offset,
            initialization,
            segments,
            next_segment,
            demuxer: None,
            track_ids: BTreeMap::new(),
            tracks: Vec::new(),
            protection_init_data,
            discontinuity,
            subtitle_format,
            active_chunked: None,
            last_polled: 0,
        })
    }

    fn next(&self) -> Option<&DashPlannedSegment> {
        self.segments.get(self.next_segment)
    }
}

/// Stateful DASH period, adaptation, ABR, network, and CMAF session.
#[derive(Debug)]
pub struct DashPlaybackSession {
    manifest_request: MediaRequest,
    manifest: DashManifest,
    options: SegmentedPlaybackOptions,
    loader: SegmentLoader,
    period_index: usize,
    cursor: Duration,
    representations: Vec<RepresentationState>,
    video_selector: Option<AdaptiveTrackSelector<DashRepresentation>>,
    selected_video_id: Option<String>,
    next_track_id: Option<NonZeroU32>,
    poll_sequence: u64,
}

impl DashPlaybackSession {
    /// Returns whether the MPD describes a dynamic live presentation.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.manifest.kind == DashManifestKind::Dynamic
    }

    /// Returns the complete static presentation duration when resolved.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        self.manifest.duration.or_else(|| {
            self.manifest.periods.last().and_then(|period| {
                period
                    .duration
                    .and_then(|duration| period.start.checked_add(duration))
            })
        })
    }

    /// Returns the current live seek window at `now`, or `None` for static media.
    ///
    /// # Errors
    ///
    /// Returns an error when the dynamic MPD has no valid availability clock.
    pub fn live_window_at(&self, now: SystemTime) -> Result<Option<LiveWindow>, Error> {
        let period = self
            .manifest
            .periods
            .get(self.period_index)
            .ok_or_else(|| Error::Streaming(String::from("DASH period index is out of bounds")))?;
        live_window(&self.manifest, period, now)
    }

    /// Returns manifest-authorized playback-rate bounds for live catch-up.
    ///
    /// Period-level service descriptions take precedence over presentation-wide
    /// descriptions. A missing lower or upper bound means normal speed on that
    /// side, so a service can authorize only catch-up or only slow-down.
    ///
    /// # Errors
    ///
    /// Returns an error when the active service description does not define a
    /// valid correction interval containing normal playback speed.
    pub fn live_playback_rate_range(&self) -> Result<Option<LivePlaybackRateRange>, Error> {
        let period = self
            .manifest
            .periods
            .get(self.period_index)
            .ok_or_else(|| Error::Streaming(String::from("DASH period index is out of bounds")))?;
        live_playback_rate_range(&self.manifest, period)
    }

    /// Returns selectable audio adaptation sets in the active period.
    ///
    /// The returned order is identical to [`AudioTrackSelection::Track`].
    #[must_use]
    pub fn audio_tracks(&self) -> Vec<SelectableAudioTrack> {
        self.manifest.periods[self.period_index]
            .adaptation_sets
            .iter()
            .filter(|adaptation| adaptation.kind == DashTrackKind::Audio)
            .enumerate()
            .map(|(index, adaptation)| dash_audio_track(adaptation, index))
            .collect()
    }

    /// Returns selectable subtitle adaptation sets in the active period.
    ///
    /// The returned order is identical to [`SubtitleTrackSelection::Track`].
    #[must_use]
    pub fn subtitle_tracks(&self) -> Vec<SelectableSubtitleTrack> {
        self.manifest.periods[self.period_index]
            .adaptation_sets
            .iter()
            .filter(|adaptation| adaptation.kind == DashTrackKind::Subtitle)
            .enumerate()
            .map(|(index, adaptation)| dash_subtitle_track(adaptation, index))
            .collect()
    }

    /// Returns video representations in ascending-bandwidth selection order.
    ///
    /// The returned order is identical to [`VideoTrackSelection::Track`] for
    /// the active DASH period.
    #[must_use]
    pub fn video_tracks(&self) -> Vec<SelectableVideoTrack> {
        self.video_selector
            .as_ref()
            .map_or_else(Vec::new, |selector| {
                selector
                    .variants()
                    .iter()
                    .map(|representation| {
                        SelectableVideoTrack::new(
                            representation.id.clone(),
                            representation.bandwidth,
                            representation.dimensions,
                            representation.codecs.clone(),
                            false,
                        )
                    })
                    .collect()
            })
    }

    /// Selects a fixed video representation or restores adaptive selection.
    ///
    /// The active period switches at the next segment boundary and subsequent
    /// periods apply the same ascending-bandwidth index.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested index is outside the active period.
    pub fn set_video_track_selection(
        &mut self,
        selection: VideoTrackSelection,
    ) -> Result<(), Error> {
        self.video_selector
            .as_mut()
            .ok_or_else(|| Error::Streaming(String::from("DASH video selector is unavailable")))?
            .set_manual_selection(selection.fixed_index())?;
        self.options = self.options.video_track_selection(selection);
        Ok(())
    }

    /// Opens a DASH presentation using the current wall clock for live availability.
    ///
    /// `supports` rejects representations the active codec/platform stack cannot decode.
    ///
    /// # Errors
    ///
    /// Returns an error for network, MPD, availability, selection, or addressing failures.
    pub async fn open(
        manifest_request: MediaRequest,
        options: SegmentedPlaybackOptions,
        supports: impl FnMut(&DashRepresentation) -> bool,
    ) -> Result<Self, Error> {
        Self::open_at(manifest_request, options, SystemTime::now(), supports).await
    }

    /// Opens a DASH presentation at an explicit wall clock instant.
    ///
    /// This deterministic entry point is intended for simulations, tests, and
    /// callers that synchronize against a trusted UTC source.
    ///
    /// # Errors
    ///
    /// Returns an error for network, MPD, availability, selection, or addressing failures.
    pub async fn open_at(
        manifest_request: MediaRequest,
        options: SegmentedPlaybackOptions,
        now: SystemTime,
        mut supports: impl FnMut(&DashRepresentation) -> bool,
    ) -> Result<Self, Error> {
        let manifest = fetch_dash_manifest(manifest_request.clone()).await?;
        validate_manifest_clock(&manifest, now)?;
        let loader = SegmentLoader::new(options.initial_bandwidth());
        let mut session = Self {
            manifest_request,
            manifest,
            options,
            loader,
            period_index: 0,
            cursor: Duration::ZERO,
            representations: Vec::new(),
            video_selector: None,
            selected_video_id: None,
            next_track_id: Some(NonZeroU32::MIN),
            poll_sequence: 1,
        };
        session.configure_period(now, &mut supports, false)?;
        Ok(session)
    }

    /// Downloads and demuxes the next presentation-time-ordered segment.
    ///
    /// For separate audio and video adaptation sets, consecutive polls may
    /// return segments with the same start time. Track identifiers are remapped
    /// into one presentation-wide namespace before samples are returned.
    ///
    /// # Errors
    ///
    /// Returns an error for MPD refresh, ABR, network, initialization, CMAF,
    /// timestamp mapping, or track-identity failures.
    pub async fn next_segment_at(
        &mut self,
        now: SystemTime,
        buffered: Duration,
        mut supports: impl FnMut(&DashRepresentation) -> bool,
    ) -> Result<DashSegmentPoll, Error> {
        validate_manifest_clock(&self.manifest, now)?;
        self.switch_video_if_needed(now, buffered, &mut supports)?;
        loop {
            if let Some(state_index) = self.next_representation_index() {
                if let Some(poll) = self.load_representation_segment(state_index).await? {
                    return Ok(poll);
                }
                continue;
            }
            match self.manifest.kind {
                DashManifestKind::Static => {
                    if self.period_index + 1 >= self.manifest.periods.len() {
                        return Ok(DashSegmentPoll::EndOfStream);
                    }
                    self.period_index += 1;
                    self.cursor = self.manifest.periods[self.period_index].start;
                    self.configure_period(now, &mut supports, true)?;
                }
                DashManifestKind::Dynamic => {
                    let retry_after = self.manifest.minimum_update_period.ok_or_else(|| {
                        Error::Streaming(String::from(
                            "dynamic DASH MPD must declare minimumUpdatePeriod",
                        ))
                    })?;
                    let previous_publish_time = self.manifest.publish_time;
                    let refreshed = fetch_dash_manifest(self.manifest_request.clone()).await?;
                    validate_manifest_clock(&refreshed, now)?;
                    if refreshed.publish_time == previous_publish_time {
                        return Ok(DashSegmentPoll::AwaitingManifest { retry_after });
                    }
                    self.manifest = refreshed;
                    if self.period_index >= self.manifest.periods.len() {
                        self.period_index =
                            self.manifest.periods.len().checked_sub(1).ok_or_else(|| {
                                Error::Streaming(String::from("refreshed DASH MPD has no periods"))
                            })?;
                    }
                    self.configure_period(now, &mut supports, true)?;
                    if self.next_representation_index().is_none() {
                        return Ok(DashSegmentPoll::AwaitingManifest { retry_after });
                    }
                }
            }
        }
    }

    /// Seeks a static presentation to the segment containing normalized progress.
    ///
    /// # Errors
    ///
    /// Returns an error for a dynamic MPD, unresolved duration, period gap, or
    /// representation that cannot be selected at the target position.
    pub fn seek_to_progress_at(
        &mut self,
        progress: f64,
        now: SystemTime,
        mut supports: impl FnMut(&DashRepresentation) -> bool,
    ) -> Result<Duration, Error> {
        if self.manifest.kind == DashManifestKind::Dynamic {
            return Err(Error::Unsupported(String::from(
                "normalized seek is undefined for a dynamic DASH presentation",
            )));
        }
        let duration = self
            .duration()
            .ok_or_else(|| Error::Streaming(String::from("static DASH duration is unresolved")))?;
        let mut target = duration.mul_f64(progress.clamp(0.0, 1.0));
        if target == duration {
            target = duration.saturating_sub(Duration::from_nanos(1));
        }
        let final_period_index = self.manifest.periods.len().checked_sub(1).ok_or_else(|| {
            Error::Streaming(String::from("static DASH presentation has no periods"))
        })?;
        self.period_index = self
            .manifest
            .periods
            .iter()
            .enumerate()
            .find_map(|(index, period)| {
                let end = period
                    .duration
                    .and_then(|duration| period.start.checked_add(duration))?;
                (target >= period.start
                    && (target < end || (index == final_period_index && target == end)))
                    .then_some(index)
            })
            .ok_or_else(|| {
                Error::Streaming(format!(
                    "DASH seek target {target:?} is outside every presentation period"
                ))
            })?;
        self.cursor = target;
        self.configure_period(now, &mut supports, true)?;
        let selected_start = self
            .representations
            .iter()
            .filter_map(|state| {
                state
                    .next()
                    .and_then(|segment| state.period_start.checked_add(segment.start))
            })
            .min()
            .ok_or_else(|| {
                Error::Streaming(String::from("DASH seek target has no containing segment"))
            })?;
        self.cursor = selected_start;
        Ok(selected_start)
    }

    /// Seeks a dynamic presentation to an absolute position in its live window.
    ///
    /// # Errors
    ///
    /// Returns an error for static media, an invalid availability clock, a
    /// target outside the current live window, or an unaddressable segment.
    pub fn seek_to_live_position_at(
        &mut self,
        position: Duration,
        now: SystemTime,
        mut supports: impl FnMut(&DashRepresentation) -> bool,
    ) -> Result<Duration, Error> {
        let window = self.live_window_at(now)?.ok_or_else(|| {
            Error::Unsupported(String::from(
                "live seek requires a dynamic DASH presentation",
            ))
        })?;
        if !window.contains(position) {
            return Err(Error::Streaming(format!(
                "DASH live seek target {position:?} is outside {:?}..={:?}",
                window.seekable_start(),
                window.seekable_end()
            )));
        }
        self.cursor = if position == window.seekable_end() {
            position.saturating_sub(Duration::from_nanos(1))
        } else {
            position
        };
        self.configure_period(now, &mut supports, true)?;
        let selected_start = self
            .representations
            .iter()
            .filter_map(|state| {
                state
                    .next()
                    .and_then(|segment| state.period_start.checked_add(segment.start))
            })
            .min()
            .ok_or_else(|| {
                Error::Streaming(String::from(
                    "DASH live seek target has no containing segment",
                ))
            })?;
        self.cursor = selected_start;
        Ok(selected_start)
    }

    fn configure_period(
        &mut self,
        now: SystemTime,
        supports: &mut impl FnMut(&DashRepresentation) -> bool,
        discontinuity: bool,
    ) -> Result<(), Error> {
        let period = self
            .manifest
            .periods
            .get(self.period_index)
            .ok_or_else(|| Error::Streaming(String::from("DASH period index is out of bounds")))?
            .clone();
        let (window_start, window_end) = presentation_window(&self.manifest, &period, now)?;
        if let Some(window) = live_window(&self.manifest, &period, now)?
            && (self.representations.is_empty()
                || self.cursor < window.seekable_start()
                || self.cursor > window.seekable_end())
        {
            self.cursor = window.target_position();
        }
        let video = preferred_adaptation(&period.adaptation_sets, DashTrackKind::Video)
            .ok_or_else(|| {
                Error::Unsupported(String::from("DASH period has no video adaptation"))
            })?;
        let mut selector = AdaptiveTrackSelector::new(
            video.representations.clone(),
            self.options.adaptive_policy(),
        )?;
        selector.set_manual_selection(self.options.selected_video_track().fixed_index())?;
        let selected_video = selector
            .select(
                self.loader.estimated_bits_per_second(),
                Duration::ZERO,
                self.options.viewport(),
                &mut *supports,
            )?
            .clone();
        let mut representations = vec![RepresentationState::new(
            DashTrackKind::Video,
            selected_video.clone(),
            period.start,
            window_start,
            window_end,
            self.cursor,
            discontinuity,
        )?];
        if let Some(audio) =
            selected_audio_adaptation(&period.adaptation_sets, self.options.selected_audio_track())?
        {
            let selected_audio = audio
                .representations
                .iter()
                .filter(|representation| supports(representation))
                .min_by_key(|representation| representation.bandwidth)
                .cloned()
                .ok_or_else(|| {
                    Error::Unsupported(String::from(
                        "no DASH audio representation satisfies codec constraints",
                    ))
                })?;
            representations.push(RepresentationState::new(
                DashTrackKind::Audio,
                selected_audio,
                period.start,
                window_start,
                window_end,
                self.cursor,
                discontinuity,
            )?);
        }
        if let Some(subtitles) = selected_subtitle_adaptation(
            &period.adaptation_sets,
            self.options.selected_subtitle_track(),
        )? {
            let selected_subtitles = subtitles
                .representations
                .iter()
                .min_by_key(|representation| representation.bandwidth)
                .cloned()
                .ok_or_else(|| {
                    Error::Unsupported(String::from(
                        "selected DASH subtitle adaptation has no representation",
                    ))
                })?;
            representations.push(RepresentationState::new(
                DashTrackKind::Subtitle,
                selected_subtitles,
                period.start,
                window_start,
                window_end,
                self.cursor,
                discontinuity,
            )?);
        }
        self.representations = representations;
        self.selected_video_id = Some(selected_video.id);
        self.video_selector = Some(selector);
        Ok(())
    }

    fn switch_video_if_needed(
        &mut self,
        now: SystemTime,
        buffered: Duration,
        supports: &mut impl FnMut(&DashRepresentation) -> bool,
    ) -> Result<(), Error> {
        let Some(selector) = &mut self.video_selector else {
            return Ok(());
        };
        if self
            .representations
            .iter()
            .any(|state| state.kind == DashTrackKind::Video && state.active_chunked.is_some())
        {
            return Ok(());
        }
        let selected = selector
            .select(
                self.loader.estimated_bits_per_second(),
                buffered,
                self.options.viewport(),
                supports,
            )?
            .clone();
        if self.selected_video_id.as_deref() == Some(selected.id.as_str()) {
            return Ok(());
        }
        let period = &self.manifest.periods[self.period_index];
        let (window_start, window_end) = presentation_window(&self.manifest, period, now)?;
        let state = RepresentationState::new(
            DashTrackKind::Video,
            selected.clone(),
            period.start,
            window_start,
            window_end,
            self.cursor,
            true,
        )?;
        let video = self
            .representations
            .iter_mut()
            .find(|state| state.kind == DashTrackKind::Video)
            .ok_or_else(|| Error::Streaming(String::from("DASH video state is missing")))?;
        *video = state;
        self.selected_video_id = Some(selected.id);
        Ok(())
    }

    fn next_representation_index(&self) -> Option<usize> {
        self.representations
            .iter()
            .enumerate()
            .filter_map(|(index, state)| {
                state.next().map(|segment| {
                    let audio_first = match state.kind {
                        DashTrackKind::Audio => 0_u8,
                        DashTrackKind::Video => 1,
                        DashTrackKind::Subtitle => 2,
                    };
                    (index, segment.start, state.last_polled, audio_first)
                })
            })
            .min_by_key(|(_, start, last_polled, audio_first)| (*start, *last_polled, *audio_first))
            .map(|(index, _, _, _)| index)
    }

    async fn load_representation_segment(
        &mut self,
        state_index: usize,
    ) -> Result<Option<DashSegmentPoll>, Error> {
        let segment = self.representations[state_index]
            .next()
            .cloned()
            .ok_or_else(|| Error::Streaming(String::from("DASH segment state is exhausted")))?;
        ensure_initialization(
            &mut self.loader,
            &self.manifest_request,
            self.options.maximum_segment_bytes(),
            &mut self.next_track_id,
            &mut self.representations[state_index],
        )
        .await?;
        if !self.representations[state_index]
            .representation
            .availability_time_complete()
        {
            return self
                .load_chunked_representation_segment(state_index, segment)
                .await;
        }
        self.load_complete_representation_segment(state_index, segment)
            .await
    }

    async fn load_chunked_representation_segment(
        &mut self,
        state_index: usize,
        segment: DashPlannedSegment,
    ) -> Result<Option<DashSegmentPoll>, Error> {
        let state = &mut self.representations[state_index];
        if state.active_chunked.is_none() {
            let resource =
                SegmentResource::for_dash(&segment.resource, self.options.maximum_segment_bytes())?
                    .with_request_context(&self.manifest_request);
            let stream = self.loader.open_stream(&resource).await?;
            let demuxer = state.demuxer.take().ok_or_else(|| {
                Error::Container(String::from(
                    "DASH initialization did not create a CMAF demuxer",
                ))
            })?;
            state.active_chunked = Some(ActiveChunkedSegment {
                segment,
                stream,
                demuxer: CmafChunkDemuxer::from_demuxer(demuxer),
                next_chunk_index: 0,
                emitted_samples: false,
                transport_chunks_consumed: 0,
                response_bytes_consumed: 0,
            });
        }
        loop {
            let active = state.active_chunked.as_mut().ok_or_else(|| {
                Error::Streaming(String::from("DASH chunked response state is missing"))
            })?;
            let Some(bytes) = active.stream.next_chunk().await? else {
                let active = state.active_chunked.take().ok_or_else(|| {
                    Error::Streaming(String::from("DASH chunked response state is missing"))
                })?;
                if !active.emitted_samples {
                    return Err(Error::Container(String::from(
                        "chunked DASH response ended without a complete CMAF chunk",
                    )));
                }
                self.loader.finish_stream(active.stream)?;
                state.demuxer = Some(active.demuxer.finish()?);
                state.next_segment += 1;
                return Ok(None);
            };
            active.transport_chunks_consumed += 1;
            active.response_bytes_consumed = active
                .response_bytes_consumed
                .checked_add(bytes.len())
                .ok_or_else(|| {
                    Error::Streaming(String::from("DASH response byte count overflow"))
                })?;
            let demuxed = active.demuxer.feed(&bytes, state.discontinuity)?;
            if demuxed.is_empty() {
                continue;
            }
            let (samples, timed_metadata) = demuxed.into_parts();
            let segment = active.segment.clone();
            let transfer = SegmentTransfer {
                estimated_bits_per_second: self.loader.estimated_bits_per_second(),
                chunk_index: Some(active.next_chunk_index),
                transport_chunks_consumed: active.transport_chunks_consumed,
                response_bytes_consumed: active.response_bytes_consumed,
            };
            active.next_chunk_index += 1;
            active.emitted_samples = true;
            let start = mapped_segment_start(state, segment.start)?;
            let poll = cmaf_segment_poll(
                state,
                &segment,
                samples,
                timed_metadata,
                self.period_index,
                transfer,
            )?;
            self.cursor = self.cursor.max(start);
            state.last_polled = self.poll_sequence;
            self.poll_sequence = self.poll_sequence.checked_add(1).ok_or_else(|| {
                Error::Streaming(String::from("DASH poll sequence exhausted u64"))
            })?;
            return Ok(Some(poll));
        }
    }

    async fn load_complete_representation_segment(
        &mut self,
        state_index: usize,
        segment: DashPlannedSegment,
    ) -> Result<Option<DashSegmentPoll>, Error> {
        let state = &mut self.representations[state_index];
        let resource =
            SegmentResource::for_dash(&segment.resource, self.options.maximum_segment_bytes())?
                .with_request_context(&self.manifest_request);
        let fetched = self.loader.load(&resource).await?;
        let estimated_bits_per_second = fetched.estimated_bits_per_second();
        let bytes = fetched.into_bytes();
        let response_bytes = bytes.len();
        if let Some(format @ (SubtitleSegmentFormat::Ttml | SubtitleSegmentFormat::WebVtt)) =
            state.subtitle_format
        {
            let (start, poll) =
                decode_direct_subtitle_segment(state, &segment, &bytes, format, self.period_index)?;
            self.cursor = self.cursor.max(start);
            state.last_polled = self.poll_sequence;
            self.poll_sequence = self.poll_sequence.checked_add(1).ok_or_else(|| {
                Error::Streaming(String::from("DASH poll sequence exhausted u64"))
            })?;
            return Ok(Some(poll));
        }
        if state.demuxer.is_none() {
            install_initialization(
                state,
                CmafInitialization::parse(&bytes)?,
                &mut self.next_track_id,
            )?;
        }
        let demuxer = state.demuxer.as_mut().ok_or_else(|| {
            Error::Container(String::from(
                "DASH initialization did not create a CMAF demuxer",
            ))
        })?;
        let (samples, timed_metadata) = demuxer
            .demux_segment(&bytes, state.discontinuity)?
            .into_parts();
        state.next_segment += 1;
        let start = mapped_segment_start(state, segment.start)?;
        let poll = cmaf_segment_poll(
            state,
            &segment,
            samples,
            timed_metadata,
            self.period_index,
            SegmentTransfer {
                estimated_bits_per_second,
                chunk_index: None,
                transport_chunks_consumed: 1,
                response_bytes_consumed: response_bytes,
            },
        )?;
        self.cursor = self.cursor.max(start);
        state.last_polled = self.poll_sequence;
        self.poll_sequence = self
            .poll_sequence
            .checked_add(1)
            .ok_or_else(|| Error::Streaming(String::from("DASH poll sequence exhausted u64")))?;
        Ok(Some(poll))
    }
}

fn cmaf_segment_poll(
    state: &mut RepresentationState,
    segment: &DashPlannedSegment,
    samples: Vec<EncodedSample>,
    timed_metadata: Vec<TimedMetadata>,
    period_index: usize,
    transfer: SegmentTransfer,
) -> Result<DashSegmentPoll, Error> {
    let mut discontinuous_tracks = BTreeSet::new();
    let samples = samples
        .into_iter()
        .map(|sample| {
            let track_id = *state.track_ids.get(&sample.track_id()).ok_or_else(|| {
                Error::Container(format!(
                    "DASH sample references unknown local track {}",
                    sample.track_id().get()
                ))
            })?;
            let starts_discontinuity = state.discontinuity && discontinuous_tracks.insert(track_id);
            let discontinuity = sample.is_discontinuity() || starts_discontinuity;
            sample
                .with_track_id(track_id)
                .with_discontinuity(discontinuity)
                .shift_timestamps(state.period_start, state.presentation_time_offset)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let timed_metadata = timed_metadata
        .into_iter()
        .map(|metadata| {
            metadata.shift_timestamp(state.period_start, state.presentation_time_offset)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    state.discontinuity = false;
    let start = mapped_segment_start(state, segment.start)?;
    if state.kind == DashTrackKind::Subtitle {
        let mut cues = Vec::new();
        for track in state
            .tracks
            .iter()
            .filter(|track| track.kind() == TrackKind::Subtitle)
        {
            for sample in samples
                .iter()
                .filter(|sample| sample.track_id() == track.id())
            {
                cues.extend(decode_cmaf_subtitle_sample(track, sample)?);
            }
        }
        return Ok(DashSegmentPoll::Subtitles(Box::new(
            DashStreamedSubtitleSegment {
                period_index,
                start,
                duration: segment.duration,
                cues,
                timed_metadata,
                chunk_index: transfer.chunk_index,
                transport_chunks_consumed: transfer.transport_chunks_consumed,
                response_bytes_consumed: transfer.response_bytes_consumed,
            },
        )));
    }
    Ok(DashSegmentPoll::Ready(Box::new(DashStreamedSegment {
        period_index,
        start,
        duration: segment.duration,
        tracks: state.tracks.clone(),
        samples,
        timed_metadata,
        protection_init_data: state.protection_init_data.clone(),
        representation: state.representation.clone(),
        estimated_bits_per_second: transfer.estimated_bits_per_second,
        chunk_index: transfer.chunk_index,
        transport_chunks_consumed: transfer.transport_chunks_consumed,
        response_bytes_consumed: transfer.response_bytes_consumed,
    })))
}

fn decode_direct_subtitle_segment(
    state: &mut RepresentationState,
    segment: &DashPlannedSegment,
    bytes: &[u8],
    format: SubtitleSegmentFormat,
    period_index: usize,
) -> Result<(Duration, DashSegmentPoll), Error> {
    let start = mapped_segment_start(state, segment.start)?;
    let document = std::str::from_utf8(bytes).map_err(|error| {
        Error::Container(format!(
            "DASH subtitle representation {:?} is not valid UTF-8: {error}",
            state.representation.id
        ))
    })?;
    let cues = match format {
        SubtitleSegmentFormat::Ttml => parse_ttml_document(document)?,
        SubtitleSegmentFormat::WebVtt => parse_webvtt_document(document)?,
        SubtitleSegmentFormat::Cmaf => {
            return Err(Error::Container(String::from(
                "CMAF subtitle representation entered the direct text decoder",
            )));
        }
    };
    let cues = map_direct_subtitle_cues(cues, start, segment.duration)?;
    state.next_segment += 1;
    Ok((
        start,
        DashSegmentPoll::Subtitles(Box::new(DashStreamedSubtitleSegment {
            period_index,
            start,
            duration: segment.duration,
            cues,
            timed_metadata: Vec::new(),
            chunk_index: None,
            transport_chunks_consumed: 1,
            response_bytes_consumed: bytes.len(),
        })),
    ))
}

fn representation_subtitle_format(
    kind: DashTrackKind,
    representation: &DashRepresentation,
) -> Result<Option<SubtitleSegmentFormat>, Error> {
    if kind != DashTrackKind::Subtitle {
        return Ok(None);
    }
    let mime_type = representation
        .mime_type
        .split(';')
        .next()
        .expect("MIME type split must yield its first component")
        .trim();
    match mime_type {
        "application/mp4" | "text/mp4" => Ok(Some(SubtitleSegmentFormat::Cmaf)),
        "application/ttml+xml" | "application/ttaf+xml" => Ok(Some(SubtitleSegmentFormat::Ttml)),
        "text/vtt" => Ok(Some(SubtitleSegmentFormat::WebVtt)),
        _ => Err(Error::Unsupported(format!(
            "DASH subtitle representation {:?} uses unsupported MIME type {:?}",
            representation.id, representation.mime_type
        ))),
    }
}

fn mapped_segment_start(
    state: &RepresentationState,
    segment_start: Duration,
) -> Result<Duration, Error> {
    state
        .period_start
        .checked_add(segment_start)
        .and_then(|start| start.checked_sub(state.presentation_time_offset))
        .ok_or_else(|| Error::Streaming(String::from("DASH segment start mapping overflow")))
}

fn map_direct_subtitle_cues(
    cues: Vec<SubtitleCue>,
    segment_start: Duration,
    segment_duration: Duration,
) -> Result<Vec<SubtitleCue>, Error> {
    let segment_end = segment_start
        .checked_add(segment_duration)
        .ok_or_else(|| Error::Streaming(String::from("DASH subtitle interval overflow")))?;
    if cues
        .iter()
        .all(|cue| cue.start >= segment_start && cue.end <= segment_end)
    {
        return Ok(cues);
    }
    if cues.iter().all(|cue| cue.end <= segment_duration) {
        return cues
            .into_iter()
            .map(|cue| cue.shift_by(segment_start))
            .collect();
    }
    Err(Error::Container(format!(
        "DASH subtitle cue timing is neither segment-local nor inside {segment_start:?}..{segment_end:?}"
    )))
}

fn selected_audio_adaptation(
    adaptations: &[DashAdaptationSet],
    selection: AudioTrackSelection,
) -> Result<Option<&DashAdaptationSet>, Error> {
    match selection {
        AudioTrackSelection::Auto => Ok(preferred_adaptation(adaptations, DashTrackKind::Audio)),
        AudioTrackSelection::Track(index) => {
            let candidates = adaptations
                .iter()
                .filter(|adaptation| adaptation.kind == DashTrackKind::Audio)
                .collect::<Vec<_>>();
            candidates.get(index).copied().map(Some).ok_or_else(|| {
                Error::Streaming(format!(
                    "DASH audio track {index} is outside the manifest's {} audio adaptations",
                    candidates.len()
                ))
            })
        }
    }
}

fn selected_subtitle_adaptation(
    adaptations: &[DashAdaptationSet],
    selection: SubtitleTrackSelection,
) -> Result<Option<&DashAdaptationSet>, Error> {
    match selection {
        SubtitleTrackSelection::Auto => Ok(adaptations
            .iter()
            .filter(|adaptation| adaptation.kind == DashTrackKind::Subtitle)
            .max_by_key(|adaptation| {
                let forced = adaptation
                    .roles
                    .iter()
                    .any(|role| role == "forced-subtitle");
                let main = adaptation.roles.iter().any(|role| role == "main");
                (forced, main)
            })),
        SubtitleTrackSelection::Off => Ok(None),
        SubtitleTrackSelection::Track(index) => {
            let candidates = adaptations
                .iter()
                .filter(|adaptation| adaptation.kind == DashTrackKind::Subtitle)
                .collect::<Vec<_>>();
            candidates.get(index).copied().map(Some).ok_or_else(|| {
                Error::Streaming(format!(
                    "DASH subtitle track {index} is outside the manifest's {} subtitle adaptations",
                    candidates.len()
                ))
            })
        }
    }
}

fn dash_audio_track(adaptation: &DashAdaptationSet, index: usize) -> SelectableAudioTrack {
    let language = adaptation
        .language
        .as_deref()
        .filter(|language| !language.trim().is_empty());
    let role = adaptation
        .roles
        .iter()
        .find(|role| !role.trim().is_empty())
        .map(String::as_str);
    let label = match (language, role) {
        (Some(language), Some(role)) => format!("{language} ({role})"),
        (Some(language), None) => language.to_owned(),
        (None, Some(role)) => role.to_owned(),
        (None, None) => adaptation
            .id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .map_or_else(|| format!("Audio {}", index + 1), str::to_owned),
    };
    SelectableAudioTrack::new(label, adaptation.language.clone(), adaptation.roles.clone())
}

fn dash_subtitle_track(adaptation: &DashAdaptationSet, index: usize) -> SelectableSubtitleTrack {
    let language = adaptation
        .language
        .as_deref()
        .filter(|language| !language.trim().is_empty());
    let role = adaptation
        .roles
        .iter()
        .find(|role| !role.trim().is_empty())
        .map(String::as_str);
    let label = match (language, role) {
        (Some(language), Some(role)) => format!("{language} ({role})"),
        (Some(language), None) => language.to_owned(),
        (None, Some(role)) => role.to_owned(),
        (None, None) => adaptation
            .id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .map_or_else(|| format!("Subtitles {}", index + 1), str::to_owned),
    };
    let forced = adaptation
        .roles
        .iter()
        .any(|role| role == "forced-subtitle");
    SelectableSubtitleTrack::new(
        label,
        adaptation.language.clone(),
        adaptation.roles.clone(),
        forced,
    )
}

async fn ensure_initialization(
    loader: &mut SegmentLoader,
    request: &MediaRequest,
    maximum_segment_bytes: std::num::NonZeroUsize,
    next_track_id: &mut Option<NonZeroU32>,
    state: &mut RepresentationState,
) -> Result<(), Error> {
    if state.demuxer.is_some() || state.active_chunked.is_some() || state.initialization.is_none() {
        return Ok(());
    }
    let initialization = state
        .initialization
        .as_ref()
        .ok_or_else(|| Error::Container(String::from("DASH initialization state is missing")))?;
    let resource = SegmentResource::for_dash_initialization(initialization, maximum_segment_bytes)?
        .with_request_context(request);
    let bytes = loader.load(&resource).await?.into_bytes();
    install_initialization(state, CmafInitialization::parse(&bytes)?, next_track_id)
}

fn install_initialization(
    state: &mut RepresentationState,
    initialization: CmafInitialization,
    next_track_id: &mut Option<NonZeroU32>,
) -> Result<(), Error> {
    let expected_kind = match state.kind {
        DashTrackKind::Video => TrackKind::Video,
        DashTrackKind::Audio => TrackKind::Audio,
        DashTrackKind::Subtitle => TrackKind::Subtitle,
    };
    if !initialization
        .tracks()
        .iter()
        .any(|track| track.kind() == expected_kind)
    {
        return Err(Error::Container(format!(
            "DASH {:?} initialization has no matching elementary track",
            state.representation.id
        )));
    }
    let mut track_ids = BTreeMap::new();
    let mut tracks = Vec::with_capacity(initialization.tracks().len());
    for track in initialization.tracks() {
        let global_id = allocate_track_id(next_track_id)?;
        track_ids.insert(track.id(), global_id);
        tracks.push(track.clone().with_id(global_id));
    }
    state.track_ids = track_ids;
    state.tracks = tracks;
    for init_data in initialization.protection_init_data() {
        if !state.protection_init_data.contains(init_data) {
            state.protection_init_data.push(init_data.clone());
        }
    }
    state.demuxer = Some(CmafDemuxer::new(initialization));
    Ok(())
}

fn manifest_protection_init_data(
    representation: &DashRepresentation,
) -> Result<Vec<ProtectionInitData>, Error> {
    let mut init_data = Vec::new();
    for protection in &representation.content_protection {
        for pssh in &protection.pssh {
            let parsed = parse_pssh_init_data(pssh)?;
            if !init_data.contains(&parsed) {
                init_data.push(parsed);
            }
        }
    }
    Ok(init_data)
}

fn allocate_track_id(next: &mut Option<NonZeroU32>) -> Result<TrackId, Error> {
    let value = next.take().ok_or_else(|| {
        Error::Container(String::from(
            "DASH presentation exhausted 32-bit track identities",
        ))
    })?;
    *next = value.get().checked_add(1).and_then(NonZeroU32::new);
    TrackId::new(value.get())
}

fn preferred_adaptation(
    adaptations: &[DashAdaptationSet],
    kind: DashTrackKind,
) -> Option<&DashAdaptationSet> {
    adaptations
        .iter()
        .filter(|adaptation| adaptation.kind == kind)
        .max_by_key(|adaptation| adaptation.roles.iter().any(|role| role == "main"))
}

fn availability_lookahead(
    representation: &DashRepresentation,
    window_start: Duration,
    window_end: Duration,
) -> Result<Duration, Error> {
    match representation.availability_time_offset() {
        DashAvailabilityTimeOffset::Finite(offset) => Ok(offset),
        DashAvailabilityTimeOffset::Infinite => maximum_declared_segment_duration(representation)
            .or_else(|_| {
                let window = window_end.saturating_sub(window_start);
                if window.is_zero() {
                    Err(Error::Streaming(String::from(
                        "infinite DASH availability requires a derivable segment horizon",
                    )))
                } else {
                    Ok(window)
                }
            }),
    }
}

fn maximum_declared_segment_duration(
    representation: &DashRepresentation,
) -> Result<Duration, Error> {
    let (ticks, timescale) = match &representation.segments {
        DashSegmentSource::Template(template) => (
            template
                .timeline
                .iter()
                .map(|entry| entry.duration)
                .max()
                .or(template.duration),
            template.timescale,
        ),
        DashSegmentSource::List(list) => (
            list.timeline
                .iter()
                .map(|entry| entry.duration)
                .max()
                .or(list.duration),
            list.timescale,
        ),
        DashSegmentSource::Base(_) => (None, NonZeroU64::MIN),
    };
    let ticks = ticks.ok_or_else(|| {
        Error::Streaming(String::from(
            "DASH segment model has no declared duration for infinite availability",
        ))
    })?;
    scaled_duration(ticks, timescale)
}

fn scaled_duration(ticks: NonZeroU64, timescale: NonZeroU64) -> Result<Duration, Error> {
    let nanos = u128::from(ticks.get())
        .checked_mul(1_000_000_000)
        .ok_or_else(|| Error::Streaming(String::from("DASH segment duration overflow")))?
        / u128::from(timescale.get());
    let nanos = u64::try_from(nanos)
        .map_err(|_| Error::Streaming(String::from("DASH segment duration exceeds u64")))?;
    Ok(Duration::from_nanos(nanos))
}

fn segment_is_available(
    segment: &DashPlannedSegment,
    live_edge: Duration,
    offset: DashAvailabilityTimeOffset,
) -> bool {
    match offset {
        DashAvailabilityTimeOffset::Infinite => true,
        DashAvailabilityTimeOffset::Finite(offset) => segment
            .start
            .checked_add(segment.duration)
            .is_some_and(|end| end <= live_edge.saturating_add(offset)),
    }
}

fn presentation_window(
    manifest: &DashManifest,
    period: &waterkit_video_streaming::DashPeriod,
    now: SystemTime,
) -> Result<(Duration, Duration), Error> {
    if manifest.kind == DashManifestKind::Static {
        let duration = period.duration.ok_or_else(|| {
            Error::Streaming(String::from("static DASH period has no resolved duration"))
        })?;
        return Ok((Duration::ZERO, duration));
    }
    let window = live_window(manifest, period, now)?.ok_or_else(|| {
        Error::Streaming(String::from(
            "dynamic DASH manifest did not produce a live window",
        ))
    })?;
    Ok((window.seekable_start(), window.seekable_end()))
}

fn live_window(
    manifest: &DashManifest,
    period: &waterkit_video_streaming::DashPeriod,
    now: SystemTime,
) -> Result<Option<LiveWindow>, Error> {
    if manifest.kind == DashManifestKind::Static {
        return Ok(None);
    }
    let availability_start = manifest.availability_start_time.ok_or_else(|| {
        Error::Streaming(String::from(
            "dynamic DASH MPD must declare availabilityStartTime",
        ))
    })?;
    let elapsed = now.duration_since(availability_start).map_err(|_| {
        Error::Streaming(String::from(
            "DASH availabilityStartTime is later than the synchronized clock",
        ))
    })?;
    let live_edge = elapsed.saturating_sub(period.start);
    let seekable_end = period
        .duration
        .map_or(live_edge, |duration| duration.min(live_edge));
    let seekable_start = manifest
        .time_shift_buffer_depth
        .map_or(Duration::ZERO, |depth| seekable_end.saturating_sub(depth));
    let target_latency = period
        .service_descriptions
        .iter()
        .flat_map(|description| &description.latency)
        .find_map(|latency| latency.target)
        .or_else(|| {
            manifest
                .service_descriptions
                .iter()
                .flat_map(|description| &description.latency)
                .find_map(|latency| latency.target)
        })
        .or(manifest.suggested_presentation_delay)
        .unwrap_or(Duration::ZERO);
    let target_position = seekable_end
        .saturating_sub(target_latency)
        .max(seekable_start);
    Ok(Some(LiveWindow::new(
        seekable_start,
        seekable_end,
        live_edge.max(seekable_end),
        target_position,
    )))
}

fn live_playback_rate_range(
    manifest: &DashManifest,
    period: &waterkit_video_streaming::DashPeriod,
) -> Result<Option<LivePlaybackRateRange>, Error> {
    if manifest.kind == DashManifestKind::Static {
        return Ok(None);
    }
    let range = period
        .service_descriptions
        .iter()
        .flat_map(|description| &description.playback_rates)
        .next()
        .or_else(|| {
            manifest
                .service_descriptions
                .iter()
                .flat_map(|description| &description.playback_rates)
                .next()
        });
    let Some(range) = range else {
        return Ok(None);
    };
    let minimum = range
        .min
        .map_or(1.0, waterkit_video_streaming::DashPlaybackRate::as_f64);
    let maximum = range
        .max
        .map_or(1.0, waterkit_video_streaming::DashPlaybackRate::as_f64);
    let minimum = minimum.to_f32().ok_or_else(|| {
        Error::Streaming(String::from(
            "DASH minimum playback rate does not fit an f32 multiplier",
        ))
    })?;
    let maximum = maximum.to_f32().ok_or_else(|| {
        Error::Streaming(String::from(
            "DASH maximum playback rate does not fit an f32 multiplier",
        ))
    })?;
    LivePlaybackRateRange::new(minimum, maximum).map(Some)
}

fn validate_manifest_clock(manifest: &DashManifest, now: SystemTime) -> Result<(), Error> {
    if manifest
        .availability_end_time
        .is_some_and(|availability_end| now > availability_end)
    {
        return Err(Error::Streaming(String::from(
            "DASH presentation availability window has ended",
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        num::{NonZeroU64, NonZeroUsize},
        sync::mpsc,
        thread,
        time::{Duration, SystemTime},
    };

    use transmux::{
        AVCConfigurationBox, AVCDecoderConfigurationRecord, AvcPps, AvcSps, CodecConfig,
        FragmentTrackData, Sample, TrackSpec, build_init_segment, build_media_segment,
    };
    use waterkit_video_streaming::{
        AdaptiveSelectionPolicy, DashAdaptationSet, DashLatency, DashManifest, DashManifestKind,
        DashPeriod, DashServiceDescription, DashTrackKind, MediaRequest, Url,
    };

    use super::{
        DashPlaybackSession, DashSegmentPoll, dash_audio_track, live_window,
        selected_audio_adaptation,
    };
    use crate::streaming::{AudioTrackSelection, SegmentedPlaybackOptions, VideoTrackSelection};

    #[test]
    fn dynamic_dash_window_uses_service_latency_before_presentation_delay() {
        let mut period = DashPeriod {
            id: Some(String::from("live")),
            start: Duration::from_secs(10),
            duration: None,
            service_descriptions: Vec::new(),
            adaptation_sets: Vec::new(),
        };
        let manifest = DashManifest {
            kind: DashManifestKind::Dynamic,
            availability_start_time: Some(SystemTime::UNIX_EPOCH),
            availability_end_time: None,
            publish_time: None,
            duration: None,
            minimum_buffer_time: None,
            minimum_update_period: Some(Duration::from_secs(2)),
            time_shift_buffer_depth: Some(Duration::from_secs(30)),
            suggested_presentation_delay: Some(Duration::from_secs(6)),
            utc_timing: Vec::new(),
            service_descriptions: Vec::new(),
            periods: vec![period.clone()],
        };

        let window = live_window(
            &manifest,
            &period,
            SystemTime::UNIX_EPOCH + Duration::from_secs(70),
        )
        .expect("live window must resolve")
        .expect("dynamic manifest must expose a live window");

        assert_eq!(window.seekable_start(), Duration::from_secs(30));
        assert_eq!(window.seekable_end(), Duration::from_secs(60));
        assert_eq!(window.live_edge(), Duration::from_secs(60));
        assert_eq!(window.target_position(), Duration::from_secs(54));
        assert_eq!(window.target_live_offset(), Duration::from_secs(6));

        period.service_descriptions.push(DashServiceDescription {
            id: Some(String::from("low-latency")),
            latency: vec![DashLatency {
                reference_id: None,
                min: Some(Duration::from_secs(1)),
                target: Some(Duration::from_secs(2)),
                max: Some(Duration::from_secs(4)),
            }],
            playback_rates: Vec::new(),
        });
        let service_window = live_window(
            &manifest,
            &period,
            SystemTime::UNIX_EPOCH + Duration::from_secs(70),
        )
        .expect("service-defined live window must resolve")
        .expect("dynamic manifest must expose a live window");
        assert_eq!(service_window.target_position(), Duration::from_secs(58));
        assert_eq!(service_window.target_live_offset(), Duration::from_secs(2));
    }

    #[test]
    fn dash_session_fetches_and_demuxes_a_static_cmaf_representation() {
        let track = video_track();
        let init = build_init_segment(std::slice::from_ref(&track), 1_000)
            .expect("test initialization must build");
        let samples = [Sample::new(vec![0, 0, 0, 1, 0x65], 3_000, true, 0)];
        let media = build_media_segment(
            1,
            &[FragmentTrackData {
                track_id: track.track_id,
                base_media_decode_time: 0,
                samples: &samples,
            }],
        )
        .expect("test media segment must build");
        let server = TestDashServer::start(init, media);
        let request = MediaRequest::new(
            Url::parse(&format!("http://{}/manifest.mpd", server.address))
                .expect("test manifest URL must parse"),
            NonZeroUsize::new(16 * 1_024).expect("test manifest limit is non-zero"),
        );
        let options = SegmentedPlaybackOptions::new(
            NonZeroUsize::new(1_024 * 1_024).expect("test segment limit is non-zero"),
            NonZeroU64::new(8_000_000).expect("test bandwidth is non-zero"),
            AdaptiveSelectionPolicy::default(),
            Some((1_920, 1_080)),
        );
        let segment = futures::executor::block_on(async {
            let mut session =
                DashPlaybackSession::open_at(request, options, SystemTime::UNIX_EPOCH, |_| true)
                    .await
                    .expect("test DASH session must open");
            let tracks = session.video_tracks();
            assert_eq!(tracks.len(), 1);
            assert_eq!(tracks[0].dimensions(), Some((1920, 1080)));
            session
                .set_video_track_selection(VideoTrackSelection::Track(0))
                .expect("advertised video track must be selectable");
            match session
                .next_segment_at(SystemTime::UNIX_EPOCH, Duration::from_secs(12), |_| true)
                .await
                .expect("test DASH segment must load")
            {
                DashSegmentPoll::Ready(segment) => segment,
                other => panic!("expected ready DASH segment, got {other:?}"),
            }
        });
        assert_eq!(segment.start(), Duration::ZERO);
        assert_eq!(segment.tracks().len(), 1);
        assert_eq!(segment.samples().len(), 1);
        assert_eq!(segment.samples()[0].data().as_ref(), samples[0].data);
        server.finish();
    }

    #[test]
    fn low_latency_dash_emits_cmaf_chunks_before_the_http_response_completes() {
        let track = video_track();
        let init = build_init_segment(std::slice::from_ref(&track), 1_000)
            .expect("test initialization must build");
        let first_samples = [Sample::new(vec![0, 0, 0, 1, 0x65, 1], 3_000, true, 0)];
        let second_samples = [Sample::new(vec![0, 0, 0, 1, 0x41, 2], 3_000, false, 0)];
        let first_chunk = build_media_segment(
            1,
            &[FragmentTrackData {
                track_id: track.track_id,
                base_media_decode_time: 0,
                samples: &first_samples,
            }],
        )
        .expect("first CMAF chunk must build");
        let second_chunk = build_media_segment(
            2,
            &[FragmentTrackData {
                track_id: track.track_id,
                base_media_decode_time: 3_000,
                samples: &second_samples,
            }],
        )
        .expect("second CMAF chunk must build");
        let server = TestLowLatencyDashServer::start(init, first_chunk, second_chunk);
        let session = open_low_latency_dash_session(server.address);
        let live_rates = session
            .live_playback_rate_range()
            .expect("low-latency playback-rate policy must resolve")
            .expect("test MPD must advertise playback-rate bounds");
        assert!((live_rates.minimum() - 0.95).abs() <= f32::EPSILON);
        assert!((live_rates.maximum() - 1.05).abs() <= f32::EPSILON);
        let (first_poll_tx, first_poll_rx) = mpsc::sync_channel(1);
        let first_poll_worker = thread::spawn(move || {
            let mut session = session;
            let result = futures::executor::block_on(session.next_segment_at(
                SystemTime::UNIX_EPOCH,
                Duration::ZERO,
                |_| true,
            ));
            first_poll_tx
                .send((session, result))
                .expect("first DASH poll result must send");
        });
        server
            .first_chunk_written
            .recv()
            .expect("server must send the first HTTP chunk");
        let (mut session, first_result) = match first_poll_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(error) => {
                server
                    .release_second_chunk
                    .send(())
                    .expect("timed-out test must release the second HTTP chunk");
                let (_, result) = first_poll_rx
                    .recv()
                    .expect("released first DASH poll must complete");
                first_poll_worker
                    .join()
                    .expect("first DASH poll worker must finish");
                server.finish();
                panic!(
                    "first DASH chunk was buffered until response completion ({error}); released poll result: {result:?}"
                );
            }
        };
        let first = match first_result.expect("first CMAF chunk must load") {
            DashSegmentPoll::Ready(segment) => segment,
            other => panic!("expected first ready DASH chunk, got {other:?}"),
        };
        assert_eq!(first.chunk_index(), Some(0));
        assert_eq!(first.samples().len(), 1);
        assert_eq!(first.samples()[0].data().as_ref(), first_samples[0].data);
        server
            .release_second_chunk
            .send(())
            .expect("test must release the second HTTP chunk");

        let second = match futures::executor::block_on(session.next_segment_at(
            SystemTime::UNIX_EPOCH,
            Duration::ZERO,
            |_| true,
        ))
        .expect("second CMAF chunk must load")
        {
            DashSegmentPoll::Ready(segment) => segment,
            other => panic!("expected second ready DASH chunk, got {other:?}"),
        };
        assert_eq!(second.chunk_index(), Some(1));
        assert_eq!(second.samples().len(), 1);
        assert_eq!(second.samples()[0].data().as_ref(), second_samples[0].data);
        first_poll_worker
            .join()
            .expect("first DASH poll worker must finish");
        server.finish();
    }

    #[test]
    fn dash_session_decodes_selected_ttml_representation() {
        let track = video_track();
        let init = build_init_segment(std::slice::from_ref(&track), 1_000)
            .expect("test initialization must build");
        let samples = [Sample::new(vec![0, 0, 0, 1, 0x65], 3_000, true, 0)];
        let media = build_media_segment(
            1,
            &[FragmentTrackData {
                track_id: track.track_id,
                base_media_decode_time: 0,
                samples: &samples,
            }],
        )
        .expect("test media segment must build");
        let server = TestDashServer::start_with_subtitles(init, media);
        let request = MediaRequest::new(
            Url::parse(&format!("http://{}/manifest.mpd", server.address))
                .expect("test manifest URL must parse"),
            NonZeroUsize::new(16 * 1_024).expect("test manifest limit is non-zero"),
        );
        let options = SegmentedPlaybackOptions::new(
            NonZeroUsize::new(1_024 * 1_024).expect("test segment limit is non-zero"),
            NonZeroU64::new(8_000_000).expect("test bandwidth is non-zero"),
            AdaptiveSelectionPolicy::default(),
            Some((1_920, 1_080)),
        );

        let (tracks, subtitles) = futures::executor::block_on(async {
            let mut session =
                DashPlaybackSession::open_at(request, options, SystemTime::UNIX_EPOCH, |_| true)
                    .await
                    .expect("test DASH subtitle session must open");
            let tracks = session.subtitle_tracks();
            let first = session
                .next_segment_at(SystemTime::UNIX_EPOCH, Duration::from_secs(12), |_| true)
                .await
                .expect("video segment must load before subtitles");
            assert!(matches!(first, DashSegmentPoll::Ready(_)));
            let subtitles = match session
                .next_segment_at(SystemTime::UNIX_EPOCH, Duration::from_secs(12), |_| true)
                .await
                .expect("subtitle segment must load")
            {
                DashSegmentPoll::Subtitles(segment) => segment,
                other => panic!("expected DASH subtitles, got {other:?}"),
            };
            (tracks, subtitles)
        });

        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].label(), "en (main)");
        assert_eq!(tracks[0].language(), Some("en"));
        assert_eq!(subtitles.cues().len(), 1);
        assert_eq!(subtitles.cues()[0].start, Duration::from_millis(500));
        assert_eq!(subtitles.cues()[0].end, Duration::from_millis(2500));
        assert_eq!(subtitles.cues()[0].text, "WaterKit DASH subtitles");
        server.finish();
    }

    #[test]
    fn dash_audio_track_preserves_language_and_roles() {
        let adaptation = DashAdaptationSet {
            id: Some(String::from("commentary-en")),
            kind: DashTrackKind::Audio,
            language: Some(String::from("en")),
            roles: vec![String::from("commentary")],
            representations: Vec::new(),
        };

        let track = dash_audio_track(&adaptation, 0);

        assert_eq!(track.label(), "en (commentary)");
        assert_eq!(track.language(), Some("en"));
        assert_eq!(track.roles(), [String::from("commentary")]);
    }

    #[test]
    fn explicit_dash_audio_selection_rejects_out_of_range_track() {
        let adaptations = [DashAdaptationSet {
            id: Some(String::from("main-audio")),
            kind: DashTrackKind::Audio,
            language: Some(String::from("en")),
            roles: vec![String::from("main")],
            representations: Vec::new(),
        }];

        let error = selected_audio_adaptation(&adaptations, AudioTrackSelection::Track(3))
            .expect_err("out-of-range DASH audio selection must fail");

        assert!(error.to_string().contains("1 audio adaptations"));
    }

    fn video_track() -> TrackSpec {
        TrackSpec::new(
            1,
            90_000,
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

    fn test_playback_options() -> SegmentedPlaybackOptions {
        SegmentedPlaybackOptions::new(
            NonZeroUsize::new(1_024 * 1_024).expect("test segment limit is non-zero"),
            NonZeroU64::new(8_000_000).expect("test bandwidth is non-zero"),
            AdaptiveSelectionPolicy::default(),
            Some((1_920, 1_080)),
        )
    }

    fn open_low_latency_dash_session(address: std::net::SocketAddr) -> DashPlaybackSession {
        let request = MediaRequest::new(
            Url::parse(&format!("http://{address}/manifest.mpd"))
                .expect("test manifest URL must parse"),
            NonZeroUsize::new(16 * 1_024).expect("test manifest limit is non-zero"),
        );
        futures::executor::block_on(async {
            DashPlaybackSession::open_at(
                request,
                test_playback_options(),
                SystemTime::UNIX_EPOCH,
                |_| true,
            )
            .await
            .expect("low-latency DASH session must open")
        })
    }

    fn read_request_path(socket: &mut std::net::TcpStream) -> String {
        let mut request = [0_u8; 4_096];
        let mut filled = 0_usize;
        loop {
            let read = socket
                .read(&mut request[filled..])
                .expect("test request must read");
            assert_ne!(read, 0, "test request ended before its HTTP header");
            filled += read;
            if request[..filled]
                .windows(4)
                .any(|window| window == b"\r\n\r\n")
            {
                break;
            }
            assert!(
                filled < request.len(),
                "test request exceeded its explicit header bound"
            );
        }
        std::str::from_utf8(&request[..filled])
            .expect("test request must be UTF-8")
            .split_ascii_whitespace()
            .nth(1)
            .expect("test request path must exist")
            .to_owned()
    }

    fn write_complete_response(socket: &mut std::net::TcpStream, body: &[u8]) {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket
            .write_all(header.as_bytes())
            .expect("test response header must write");
        socket
            .write_all(body)
            .expect("test response body must write");
    }

    fn write_http_chunk(socket: &mut std::net::TcpStream, body: &[u8]) {
        let header = format!("{:X}\r\n", body.len());
        socket
            .write_all(header.as_bytes())
            .expect("HTTP chunk header must write");
        socket.write_all(body).expect("HTTP chunk body must write");
        socket
            .write_all(b"\r\n")
            .expect("HTTP chunk delimiter must write");
    }

    struct TestDashServer {
        address: std::net::SocketAddr,
        worker: thread::JoinHandle<()>,
    }

    struct TestLowLatencyDashServer {
        address: std::net::SocketAddr,
        first_chunk_written: mpsc::Receiver<()>,
        release_second_chunk: mpsc::Sender<()>,
        worker: thread::JoinHandle<()>,
    }

    impl TestLowLatencyDashServer {
        fn start(init: Vec<u8>, first_chunk: Vec<u8>, second_chunk: Vec<u8>) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test server must bind");
            let address = listener.local_addr().expect("test address must exist");
            let (first_chunk_written_tx, first_chunk_written) = mpsc::channel();
            let (release_second_chunk, release_second_chunk_rx) = mpsc::channel();
            let worker = thread::spawn(move || {
                for _ in 0..3 {
                    let (mut socket, _) = listener.accept().expect("test request must arrive");
                    socket
                        .set_nodelay(true)
                        .expect("low-latency test socket must disable Nagle buffering");
                    let path = read_request_path(&mut socket);
                    match path.as_str() {
                        "/manifest.mpd" => write_complete_response(
                            &mut socket,
                            include_bytes!("../tests/assets/dash_session_low_latency.mpd"),
                        ),
                        "/init.mp4" => write_complete_response(&mut socket, &init),
                        "/segment-1.m4s" => {
                            socket
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                                )
                                .expect("streaming response header must write");
                            write_http_chunk(&mut socket, &first_chunk);
                            socket.flush().expect("first HTTP chunk must flush");
                            first_chunk_written_tx
                                .send(())
                                .expect("first-chunk signal must send");
                            release_second_chunk_rx
                                .recv()
                                .expect("second HTTP chunk must be explicitly released");
                            write_http_chunk(&mut socket, &second_chunk);
                            socket
                                .write_all(b"0\r\n\r\n")
                                .expect("HTTP chunked response terminator must write");
                        }
                        _ => panic!("unexpected low-latency DASH request path {path}"),
                    }
                }
            });
            Self {
                address,
                first_chunk_written,
                release_second_chunk,
                worker,
            }
        }

        fn finish(self) {
            self.worker
                .join()
                .expect("low-latency test server must finish");
        }
    }

    impl TestDashServer {
        fn start(init: Vec<u8>, media: Vec<u8>) -> Self {
            Self::start_with_assets(
                include_bytes!("../tests/assets/dash_session_static.mpd"),
                init,
                media,
                None,
            )
        }

        fn start_with_subtitles(init: Vec<u8>, media: Vec<u8>) -> Self {
            Self::start_with_assets(
                include_bytes!("../tests/assets/dash_session_subtitles.mpd"),
                init,
                media,
                Some(include_bytes!(
                    "../tests/assets/dash_session_subtitles.ttml"
                )),
            )
        }

        fn start_with_assets(
            manifest: &'static [u8],
            init: Vec<u8>,
            media: Vec<u8>,
            subtitles: Option<&'static [u8]>,
        ) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test server must bind");
            let address = listener.local_addr().expect("test address must exist");
            let worker = thread::spawn(move || {
                for _ in 0..3 + usize::from(subtitles.is_some()) {
                    let (mut socket, _) = listener.accept().expect("test request must arrive");
                    let path = read_request_path(&mut socket);
                    let body = if path.ends_with("/manifest.mpd") {
                        manifest
                    } else if path.ends_with("/init.mp4") {
                        init.as_slice()
                    } else if path.ends_with("/segment-1.m4s") {
                        media.as_slice()
                    } else if path.ends_with("/subtitles-1.ttml") {
                        subtitles.expect("only subtitle tests request a TTML segment")
                    } else {
                        panic!("unexpected test request path {path}")
                    };
                    write_complete_response(&mut socket, body);
                }
            });
            Self { address, worker }
        }

        fn finish(self) {
            self.worker.join().expect("test server must finish");
        }
    }
}
