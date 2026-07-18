//! Zenwave-backed segmented playback sessions.

use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    num::{NonZeroU32, NonZeroU64, NonZeroUsize},
    time::Duration,
};

use aes::cipher::{BlockModeDecrypt as _, KeyIvInit as _, block_padding::Pkcs7};
use waterkit_video_container::{
    CmafDemuxer, CmafInitialization, Codec, EncodedSample, MpegTsDemuxer, MpegTsEvent, SubtitleCue,
    TimedMetadata, TrackId, TrackInfo, TrackKind, decode_cmaf_subtitle_sample,
    parse_hls_webvtt_segment,
};
use waterkit_video_core::Error;
use waterkit_video_core::ProtectionInitData;
use waterkit_video_streaming::{
    AdaptiveSelectionPolicy, AdaptiveTrackSelector, HlsEncryption, HlsEncryptionMethod,
    HlsInitializationSegment, HlsMediaPlaylist, HlsPartialSegment, HlsPlaylist, HlsRendition,
    HlsRenditionKind, HlsSegment, MediaRequest, SegmentLoader, SegmentResource, StreamVariant,
    fetch_hls_playlist, fetch_media,
};

use crate::{
    LiveWindow, audio_track::SelectableAudioTrack, subtitle_track::SelectableSubtitleTrack,
    video_track::SelectableVideoTrack,
};

type Aes128CbcDecryptor = cbc::Decryptor<aes::Aes128>;

/// Audio rendition selection shared by segmented playback sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioTrackSelection {
    /// Use the manifest's default or autoselected audio rendition.
    #[default]
    Auto,
    /// Select the zero-based audio rendition in manifest order.
    Track(usize),
}

/// Subtitle rendition selection shared by segmented playback sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubtitleTrackSelection {
    /// Use the manifest's forced, default, or autoselected subtitle rendition.
    #[default]
    Auto,
    /// Do not download or decode a manifest subtitle rendition.
    Off,
    /// Select the zero-based subtitle rendition in manifest order.
    Track(usize),
}

/// Video representation selection shared by segmented playback sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoTrackSelection {
    /// Let adaptive bitrate policy choose a representation.
    #[default]
    Auto,
    /// Select the zero-based representation in ascending-bandwidth order.
    Track(usize),
}

impl VideoTrackSelection {
    /// Returns the fixed representation index, or `None` while ABR is active.
    #[must_use]
    pub const fn fixed_index(self) -> Option<usize> {
        match self {
            Self::Auto => None,
            Self::Track(index) => Some(index),
        }
    }
}

/// Explicit memory, bandwidth, and viewport policy for segmented playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentedPlaybackOptions {
    maximum_segment_bytes: NonZeroUsize,
    initial_bandwidth: NonZeroU64,
    adaptive_policy: AdaptiveSelectionPolicy,
    viewport: Option<(u32, u32)>,
    audio_track_selection: AudioTrackSelection,
    subtitle_track_selection: SubtitleTrackSelection,
    video_track_selection: VideoTrackSelection,
}

impl SegmentedPlaybackOptions {
    /// Creates playback policy with every resource bound explicit.
    #[must_use]
    pub const fn new(
        maximum_segment_bytes: NonZeroUsize,
        initial_bandwidth: NonZeroU64,
        adaptive_policy: AdaptiveSelectionPolicy,
        viewport: Option<(u32, u32)>,
    ) -> Self {
        Self {
            maximum_segment_bytes,
            initial_bandwidth,
            adaptive_policy,
            viewport,
            audio_track_selection: AudioTrackSelection::Auto,
            subtitle_track_selection: SubtitleTrackSelection::Auto,
            video_track_selection: VideoTrackSelection::Auto,
        }
    }

    /// Selects one manifest audio rendition by zero-based index.
    #[must_use]
    pub const fn audio_track_selection(mut self, selection: AudioTrackSelection) -> Self {
        self.audio_track_selection = selection;
        self
    }

    /// Selects one manifest subtitle rendition or disables manifest subtitles.
    #[must_use]
    pub const fn subtitle_track_selection(mut self, selection: SubtitleTrackSelection) -> Self {
        self.subtitle_track_selection = selection;
        self
    }

    /// Selects a fixed video representation or restores adaptive selection.
    #[must_use]
    pub const fn video_track_selection(mut self, selection: VideoTrackSelection) -> Self {
        self.video_track_selection = selection;
        self
    }

    pub(super) const fn maximum_segment_bytes(self) -> NonZeroUsize {
        self.maximum_segment_bytes
    }

    pub(super) const fn initial_bandwidth(self) -> NonZeroU64 {
        self.initial_bandwidth
    }

    pub(super) const fn adaptive_policy(self) -> AdaptiveSelectionPolicy {
        self.adaptive_policy
    }

    pub(super) const fn viewport(self) -> Option<(u32, u32)> {
        self.viewport
    }

    pub(super) const fn selected_audio_track(self) -> AudioTrackSelection {
        self.audio_track_selection
    }

    pub(super) const fn selected_subtitle_track(self) -> SubtitleTrackSelection {
        self.subtitle_track_selection
    }

    pub(super) const fn selected_video_track(self) -> VideoTrackSelection {
        self.video_track_selection
    }
}

/// One fully downloaded and demuxed HLS segment.
#[derive(Debug)]
pub struct StreamedSegment {
    sequence: usize,
    part_index: Option<usize>,
    duration: Duration,
    tracks: Vec<TrackInfo>,
    samples: Vec<EncodedSample>,
    timed_metadata: Vec<TimedMetadata>,
    protection_init_data: Vec<ProtectionInitData>,
    content_protection: Vec<HlsEncryption>,
    selected_variant: Option<StreamVariant>,
    estimated_bits_per_second: NonZeroU64,
}

/// One fully downloaded and decoded HLS subtitle segment.
#[derive(Debug)]
pub struct StreamedSubtitleSegment {
    sequence: usize,
    part_index: Option<usize>,
    duration: Duration,
    cues: Vec<SubtitleCue>,
    timed_metadata: Vec<TimedMetadata>,
}

impl StreamedSubtitleSegment {
    /// Returns the subtitle media-sequence number.
    #[must_use]
    pub const fn sequence(&self) -> usize {
        self.sequence
    }

    /// Returns the LL-HLS part index, or `None` for a complete media segment.
    #[must_use]
    pub const fn part_index(&self) -> Option<usize> {
        self.part_index
    }

    /// Returns the manifest-declared segment duration.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.duration
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

impl StreamedSegment {
    /// Returns the HLS media-sequence number.
    #[must_use]
    pub const fn sequence(&self) -> usize {
        self.sequence
    }

    /// Returns the LL-HLS part index, or `None` for a complete media segment.
    #[must_use]
    pub const fn part_index(&self) -> Option<usize> {
        self.part_index
    }

    /// Returns the manifest-declared segment duration.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.duration
    }

    /// Returns the elementary tracks declared for this segment.
    #[must_use]
    pub fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }

    /// Returns coded samples in container emission order.
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

    /// Returns DRM initialization data from the CMAF initialization section.
    #[must_use]
    pub fn protection_init_data(&self) -> &[ProtectionInitData] {
        &self.protection_init_data
    }

    /// Returns non-identity HLS key-format declarations active for this segment.
    #[must_use]
    pub fn content_protection(&self) -> &[HlsEncryption] {
        &self.content_protection
    }

    /// Returns the selected multivariant rendition for this segment.
    #[must_use]
    pub const fn selected_variant(&self) -> Option<&StreamVariant> {
        self.selected_variant.as_ref()
    }

    /// Returns the conservative network estimate after downloading this segment.
    #[must_use]
    pub const fn estimated_bits_per_second(&self) -> NonZeroU64 {
        self.estimated_bits_per_second
    }
}

/// Result of polling the next HLS media sequence.
#[derive(Debug)]
#[non_exhaustive]
pub enum HlsSegmentPoll {
    /// A media segment is ready for decode.
    Ready(Box<StreamedSegment>),
    /// A selected alternate-subtitle segment is ready for presentation.
    Subtitles(Box<StreamedSubtitleSegment>),
    /// A live playlist has not published the requested sequence yet.
    AwaitingPlaylist {
        /// Manifest-defined delay before another ordinary playlist reload.
        retry_after: Duration,
    },
    /// A finite playlist has been exhausted.
    EndOfStream,
}

#[derive(Debug)]
struct CmafState {
    initialization: HlsInitializationSegment,
    demuxer: CmafDemuxer,
    protection_init_data: Vec<ProtectionInitData>,
}

#[derive(Debug)]
struct MediaStreamState {
    identity: String,
    request: MediaRequest,
    playlist: HlsMediaPlaylist,
    playlist_start: Duration,
    next_sequence: usize,
    next_part: usize,
    next_start: Duration,
    force_discontinuity: bool,
    cmaf: Option<CmafState>,
}

impl MediaStreamState {
    fn new(identity: String, request: MediaRequest, playlist: HlsMediaPlaylist) -> Self {
        let next_sequence = playlist.media_sequence;
        let mut state = Self {
            identity,
            request,
            playlist,
            playlist_start: Duration::ZERO,
            next_sequence,
            next_part: 0,
            next_start: Duration::ZERO,
            force_discontinuity: false,
            cmaf: None,
        };
        if let Some(window) = state.live_window() {
            state
                .seek_to_position(window.target_position())
                .expect("HLS live target must resolve inside its source playlist");
        }
        state
    }

    fn duration(&self) -> Option<Duration> {
        self.playlist.ended.then(|| self.playlist_duration())
    }

    fn playlist_duration(&self) -> Duration {
        let complete_duration = self
            .playlist
            .segments
            .iter()
            .fold(Duration::ZERO, |duration, segment| {
                duration.saturating_add(segment.duration)
            });
        self.playlist
            .low_latency
            .as_ref()
            .map_or(complete_duration, |low_latency| {
                low_latency
                    .trailing_parts
                    .iter()
                    .fold(complete_duration, |duration, part| {
                        duration.saturating_add(part.duration)
                    })
            })
    }

    fn live_window(&self) -> Option<LiveWindow> {
        if self.playlist.ended || self.playlist_duration().is_zero() {
            return None;
        }
        let live_edge = self.playlist_start.saturating_add(self.playlist_duration());
        let target_offset = self
            .playlist
            .server_control
            .and_then(|control| {
                self.playlist
                    .low_latency
                    .as_ref()
                    .and(control.part_hold_back)
                    .or(control.hold_back)
            })
            .unwrap_or_else(|| self.playlist.target_duration.saturating_mul(3));
        let target_position = live_edge
            .saturating_sub(target_offset)
            .max(self.playlist_start);
        Some(LiveWindow::new(
            self.playlist_start,
            live_edge,
            live_edge,
            target_position,
        ))
    }

    fn reload_delay(&self) -> Duration {
        self.playlist
            .low_latency
            .as_ref()
            .map_or(self.playlist.target_duration, |low_latency| {
                low_latency.part_target
            })
    }

    fn reload_request(&self) -> MediaRequest {
        let Some(server_control) = self.playlist.server_control else {
            return self.request.clone();
        };
        let mut url = self.request.url().clone();
        let mut query = url
            .query_pairs()
            .filter(|(name, _)| !matches!(name.as_ref(), "_HLS_msn" | "_HLS_part" | "_HLS_skip"))
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        if server_control.can_block_reload {
            query.push((String::from("_HLS_msn"), self.next_sequence.to_string()));
            if self.playlist.low_latency.is_some() {
                query.push((String::from("_HLS_part"), self.next_part.to_string()));
            }
        }
        if server_control.can_skip_until.is_some() {
            query.push((
                String::from("_HLS_skip"),
                if server_control.can_skip_dateranges {
                    String::from("v2")
                } else {
                    String::from("YES")
                },
            ));
        }
        url.query_pairs_mut().clear().extend_pairs(query);
        self.request.related(url)
    }

    fn position_for_sequence(&self, sequence: usize) -> Option<Duration> {
        if sequence < self.playlist.media_sequence {
            return None;
        }
        let mut position = self.playlist_start;
        for segment in &self.playlist.segments {
            if segment.sequence == sequence {
                return Some(position);
            }
            position = position.saturating_add(segment.duration);
        }
        if self
            .playlist
            .low_latency
            .as_ref()
            .and_then(|low_latency| low_latency.trailing_parts.first())
            .is_some_and(|part| part.sequence == sequence)
        {
            return Some(position);
        }
        let one_past_end = self
            .playlist
            .segments
            .last()
            .and_then(|segment| segment.sequence.checked_add(1));
        (one_past_end == Some(sequence)).then_some(position)
    }

    fn offset_for_sequence(&self, sequence: usize) -> Option<Duration> {
        self.position_for_sequence(sequence)
            .map(|position| position.saturating_sub(self.playlist_start))
    }

    fn reconstruct_delta_playlist(
        &self,
        mut playlist: HlsMediaPlaylist,
    ) -> Result<HlsMediaPlaylist, Error> {
        let Some(delta) = &playlist.delta_update else {
            return Ok(playlist);
        };
        let declared_start = playlist
            .media_sequence
            .checked_sub(delta.skipped_segments)
            .ok_or_else(|| {
                Error::Streaming(String::from(
                    "HLS delta update skipped more segments than its media sequence",
                ))
            })?;
        let retained = self
            .playlist
            .segments
            .iter()
            .filter(|segment| {
                segment.sequence >= declared_start && segment.sequence < playlist.media_sequence
            })
            .cloned()
            .collect::<Vec<_>>();
        if retained.len() != delta.skipped_segments {
            return Err(Error::Streaming(format!(
                "HLS delta update skipped {} segments but only {} are retained locally from sequence {declared_start}",
                delta.skipped_segments,
                retained.len()
            )));
        }
        let mut segments = retained;
        segments.append(&mut playlist.segments);
        playlist.media_sequence = declared_start;
        playlist.segments = segments;
        Ok(playlist)
    }

    fn replace_playlist(&mut self, playlist: HlsMediaPlaylist) -> Result<(), Error> {
        let playlist = self.reconstruct_delta_playlist(playlist)?;
        if playlist.media_sequence < self.playlist.media_sequence {
            return Err(Error::Streaming(format!(
                "HLS media sequence regressed from {} to {}",
                self.playlist.media_sequence, playlist.media_sequence
            )));
        }
        let playlist_start = self
            .position_for_sequence(playlist.media_sequence)
            .ok_or_else(|| {
                Error::Streaming(format!(
                    "HLS playlist skipped from sequence {} beyond known timeline ending at {}",
                    self.playlist.media_sequence,
                    self.playlist
                        .segments
                        .last()
                        .map_or(self.playlist.media_sequence, |segment| segment.sequence)
                ))
            })?;
        self.playlist = playlist;
        self.playlist_start = playlist_start;
        Ok(())
    }

    fn seek_to_progress(&mut self, progress: f64) -> Result<Duration, Error> {
        if !self.playlist.ended {
            return Err(Error::Unsupported(String::from(
                "normalized seek is undefined for a live HLS playlist",
            )));
        }
        let duration = self.duration().ok_or_else(|| {
            Error::Streaming(String::from("finite HLS playlist duration is unavailable"))
        })?;
        if self.playlist.segments.is_empty() {
            return Err(Error::Streaming(String::from(
                "finite HLS playlist has no segments",
            )));
        }
        self.seek_to_position(duration.mul_f64(progress.clamp(0.0, 1.0)))
    }

    fn seek_to_position(&mut self, target: Duration) -> Result<Duration, Error> {
        if let Some(window) = self.live_window()
            && !window.contains(target)
        {
            return Err(Error::Streaming(format!(
                "HLS live seek target {target:?} is outside {:?}..={:?}",
                window.seekable_start(),
                window.seekable_end()
            )));
        }
        let mut start = self.playlist_start;
        for segment in &self.playlist.segments {
            let end = start.saturating_add(segment.duration);
            if target < end {
                self.next_sequence = segment.sequence;
                self.next_part = 0;
                self.next_start = start;
                self.force_discontinuity = true;
                self.cmaf = None;
                return Ok(start);
            }
            start = end;
        }
        if let Some(low_latency) = &self.playlist.low_latency {
            for part in &low_latency.trailing_parts {
                let end = start.saturating_add(part.duration);
                if target < end {
                    self.next_sequence = part.sequence;
                    self.next_part = part.part_index;
                    self.next_start = start;
                    self.force_discontinuity = true;
                    self.cmaf = None;
                    return Ok(start);
                }
                start = end;
            }
            if target == start
                && let Some(part) = low_latency.trailing_parts.last()
            {
                self.next_sequence = part.sequence;
                self.next_part = part.part_index;
                self.next_start = start.saturating_sub(part.duration);
                self.force_discontinuity = true;
                self.cmaf = None;
                return Ok(self.next_start);
            }
        }
        if target == start
            && let Some(segment) = self.playlist.segments.last()
        {
            self.next_sequence = segment.sequence;
            self.next_part = 0;
            self.next_start = start.saturating_sub(segment.duration);
            self.force_discontinuity = true;
            self.cmaf = None;
            return Ok(self.next_start);
        }
        Err(Error::Streaming(String::from(
            "HLS playlist does not contain the requested position",
        )))
    }

    fn align_after_switch(
        &mut self,
        presentation_position: Duration,
        preferred_sequence: usize,
        preferred_part: usize,
    ) -> Result<(), Error> {
        if self.playlist.ended {
            self.seek_to_position(presentation_position)?;
        } else {
            let next_sequence = preferred_sequence.max(self.playlist.media_sequence);
            let offset = self.offset_for_sequence(next_sequence).ok_or_else(|| {
                Error::Streaming(format!(
                    "HLS rendition does not contain aligned sequence {next_sequence}"
                ))
            })?;
            self.playlist_start = presentation_position.checked_sub(offset).ok_or_else(|| {
                Error::Streaming(String::from(
                    "HLS rendition alignment precedes the presentation timeline origin",
                ))
            })?;
            self.next_sequence = next_sequence;
            self.next_part = preferred_part;
            self.next_start = presentation_position;
            self.force_discontinuity = true;
            self.cmaf = None;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct TrackNamespace {
    mappings: BTreeMap<String, BTreeMap<u32, TrackId>>,
    next_id: Option<NonZeroU32>,
}

impl TrackNamespace {
    const fn new() -> Self {
        Self {
            mappings: BTreeMap::new(),
            next_id: Some(NonZeroU32::MIN),
        }
    }

    fn remap(
        &mut self,
        identity: &str,
        tracks: Vec<TrackInfo>,
        samples: Vec<EncodedSample>,
    ) -> Result<(Vec<TrackInfo>, Vec<EncodedSample>), Error> {
        let mappings = self.mappings.entry(identity.to_owned()).or_default();
        for track in &tracks {
            let local_id = track.id().get();
            if let Entry::Vacant(entry) = mappings.entry(local_id) {
                let global_id = allocate_hls_track_id(&mut self.next_id)?;
                entry.insert(global_id);
            }
        }
        let remapped_tracks = tracks
            .into_iter()
            .map(|track| {
                let id = mappings[&track.id().get()];
                track.with_id(id)
            })
            .collect();
        let remapped_samples = samples
            .into_iter()
            .map(|sample| {
                let id = mappings.get(&sample.track_id().get()).copied().ok_or_else(|| {
                    Error::Container(format!(
                        "HLS rendition {identity} emitted a sample for undeclared local track {}",
                        sample.track_id().get()
                    ))
                })?;
                Ok(sample.with_track_id(id))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok((remapped_tracks, remapped_samples))
    }
}

/// Stateful HLS playlist, ABR, network, and demux session.
#[derive(Debug)]
pub struct HlsPlaybackSession {
    request_context: MediaRequest,
    main: MediaStreamState,
    alternate_audio: Option<MediaStreamState>,
    alternate_subtitles: Option<MediaStreamState>,
    renditions: Vec<HlsRendition>,
    selector: Option<AdaptiveTrackSelector>,
    selected_variant: Option<StreamVariant>,
    loader: SegmentLoader,
    maximum_segment_bytes: NonZeroUsize,
    viewport: Option<(u32, u32)>,
    audio_track_selection: AudioTrackSelection,
    subtitle_track_selection: SubtitleTrackSelection,
    encryption_keys: BTreeMap<String, [u8; 16]>,
    track_namespace: TrackNamespace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HlsStreamKind {
    Main,
    AlternateAudio,
    AlternateSubtitles,
}

enum HlsStreamPoll {
    Ready(Box<StreamedSegment>),
    Subtitles(Box<StreamedSubtitleSegment>),
    AwaitingPlaylist { retry_after: Duration },
    EndOfStream,
}

enum HlsMediaUnit {
    Segment(HlsSegment),
    Partial(HlsPartialSegment),
}

impl HlsMediaUnit {
    const fn sequence(&self) -> usize {
        match self {
            Self::Segment(segment) => segment.sequence,
            Self::Partial(part) => part.sequence,
        }
    }

    const fn part_index(&self) -> Option<usize> {
        match self {
            Self::Segment(_) => None,
            Self::Partial(part) => Some(part.part_index),
        }
    }

    const fn duration(&self) -> Duration {
        match self {
            Self::Segment(segment) => segment.duration,
            Self::Partial(part) => part.duration,
        }
    }

    const fn initialization(&self) -> Option<&HlsInitializationSegment> {
        match self {
            Self::Segment(segment) => segment.initialization.as_ref(),
            Self::Partial(part) => part.initialization.as_ref(),
        }
    }

    fn encryption(&self) -> &[HlsEncryption] {
        match self {
            Self::Segment(segment) => &segment.encryption,
            Self::Partial(part) => &part.encryption,
        }
    }

    const fn discontinuity(&self) -> bool {
        match self {
            Self::Segment(segment) => segment.discontinuity,
            Self::Partial(part) => part.discontinuity,
        }
    }

    const fn gap(&self) -> bool {
        match self {
            Self::Segment(segment) => segment.gap,
            Self::Partial(part) => part.gap,
        }
    }

    fn resource(&self, maximum_segment_bytes: NonZeroUsize) -> Result<SegmentResource, Error> {
        match self {
            Self::Segment(segment) => SegmentResource::for_hls(segment, maximum_segment_bytes),
            Self::Partial(part) => SegmentResource::for_hls_partial(part, maximum_segment_bytes),
        }
    }

    fn advance(&self, state: &mut MediaStreamState) -> Result<(), Error> {
        match self {
            Self::Segment(segment) => {
                state.next_sequence = segment.sequence.checked_add(1).ok_or_else(|| {
                    Error::Streaming(String::from("HLS media sequence overflowed usize"))
                })?;
                state.next_part = 0;
            }
            Self::Partial(part) => {
                state.next_part = part.part_index.checked_add(1).ok_or_else(|| {
                    Error::Streaming(String::from("HLS partial index overflowed usize"))
                })?;
            }
        }
        state.next_start = state.next_start.saturating_add(self.duration());
        Ok(())
    }
}

impl HlsPlaybackSession {
    /// Opens an HLS media or multivariant playlist through Zenwave.
    ///
    /// `supports` rejects variants the active codec/platform stack cannot decode.
    ///
    /// # Errors
    ///
    /// Returns an error for network, manifest, variant-selection, or media-playlist
    /// failures. The response size bound is taken from `manifest_request`.
    pub async fn open(
        manifest_request: MediaRequest,
        options: SegmentedPlaybackOptions,
        mut supports: impl FnMut(&StreamVariant) -> bool,
    ) -> Result<Self, Error> {
        let playlist = fetch_hls_playlist(manifest_request.clone()).await?;
        let loader = SegmentLoader::new(options.initial_bandwidth);
        let (main, alternate_audio, alternate_subtitles, renditions, selector, selected_variant) =
            match playlist {
                HlsPlaylist::Media(media) => (
                    MediaStreamState::new(String::from("main"), manifest_request.clone(), *media),
                    None,
                    None,
                    Vec::new(),
                    None,
                    None,
                ),
                HlsPlaylist::Master(master) => {
                    let mut selector =
                        AdaptiveTrackSelector::new(master.variants, options.adaptive_policy)?;
                    selector.set_manual_selection(options.selected_video_track().fixed_index())?;
                    let selected = selector
                        .select(
                            options.initial_bandwidth,
                            Duration::ZERO,
                            options.viewport,
                            &mut supports,
                        )?
                        .clone();
                    let media_request = manifest_request.related(selected.url.clone());
                    let media = fetch_media_playlist(media_request.clone()).await?;
                    let alternate_audio = open_audio_rendition(
                        &manifest_request,
                        &master.renditions,
                        &selected,
                        options.audio_track_selection,
                    )
                    .await?;
                    let alternate_subtitles = open_subtitle_rendition(
                        &manifest_request,
                        &master.renditions,
                        &selected,
                        options.selected_subtitle_track(),
                    )
                    .await?;
                    (
                        MediaStreamState::new(String::from("main"), media_request, media),
                        alternate_audio,
                        alternate_subtitles,
                        master.renditions,
                        Some(selector),
                        Some(selected),
                    )
                }
            };
        Ok(Self {
            request_context: manifest_request,
            main,
            alternate_audio,
            alternate_subtitles,
            renditions,
            selector,
            selected_variant,
            loader,
            maximum_segment_bytes: options.maximum_segment_bytes,
            viewport: options.viewport,
            audio_track_selection: options.audio_track_selection,
            subtitle_track_selection: options.selected_subtitle_track(),
            encryption_keys: BTreeMap::new(),
            track_namespace: TrackNamespace::new(),
        })
    }

    /// Changes the pixel viewport used by future adaptive selections.
    pub const fn set_viewport(&mut self, viewport: Option<(u32, u32)>) {
        self.viewport = viewport;
    }

    /// Returns whether the active media playlist is live.
    #[must_use]
    pub const fn is_live(&self) -> bool {
        !self.main.playlist.ended
    }

    /// Returns the complete duration for a finite media playlist.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        self.main.duration()
    }

    /// Returns the current live seek window, or `None` for finite media.
    #[must_use]
    pub fn live_window(&self) -> Option<LiveWindow> {
        self.main.live_window()
    }

    /// Returns selectable alternate-audio renditions for the active variant.
    ///
    /// The returned order is identical to [`AudioTrackSelection::Track`]. A
    /// media playlist or muxed-audio variant returns an empty list because it
    /// exposes no independently selectable audio rendition.
    #[must_use]
    pub fn audio_tracks(&self) -> Vec<SelectableAudioTrack> {
        let Some(group_id) = self
            .selected_variant
            .as_ref()
            .and_then(|variant| variant.audio_group_id.as_deref())
        else {
            return Vec::new();
        };
        audio_renditions(&self.renditions, group_id)
            .enumerate()
            .map(|(index, rendition)| {
                let label = if rendition.name.trim().is_empty() {
                    format!("Audio {}", index + 1)
                } else {
                    rendition.name.clone()
                };
                SelectableAudioTrack::new(label, rendition.language.clone(), Vec::new())
            })
            .collect()
    }

    /// Returns selectable alternate-subtitle renditions for the active variant.
    ///
    /// The returned order is identical to [`SubtitleTrackSelection::Track`].
    #[must_use]
    pub fn subtitle_tracks(&self) -> Vec<SelectableSubtitleTrack> {
        let Some(group_id) = self
            .selected_variant
            .as_ref()
            .and_then(|variant| variant.subtitle_group_id.as_deref())
        else {
            return Vec::new();
        };
        subtitle_renditions(&self.renditions, group_id)
            .enumerate()
            .map(|(index, rendition)| {
                let label = if rendition.name.trim().is_empty() {
                    format!("Subtitles {}", index + 1)
                } else {
                    rendition.name.clone()
                };
                SelectableSubtitleTrack::new(
                    label,
                    rendition.language.clone(),
                    Vec::new(),
                    rendition.is_forced,
                )
            })
            .collect()
    }

    /// Returns video representations in ascending-bandwidth selection order.
    ///
    /// The returned order is identical to [`VideoTrackSelection::Track`]. A
    /// direct media playlist exposes no manifest-level representation choices.
    #[must_use]
    pub fn video_tracks(&self) -> Vec<SelectableVideoTrack> {
        self.selector.as_ref().map_or_else(Vec::new, |selector| {
            selector
                .variants()
                .iter()
                .map(|variant| {
                    SelectableVideoTrack::new(
                        variant.url.as_str(),
                        variant.selection_bandwidth(),
                        variant.dimensions,
                        variant.codecs.clone(),
                        false,
                    )
                })
                .collect()
        })
    }

    /// Selects a fixed video representation or restores adaptive selection.
    ///
    /// The new representation is applied at the next segment boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when a direct media playlist has no selectable quality
    /// or the requested index is outside the advertised representation list.
    pub fn set_video_track_selection(
        &mut self,
        selection: VideoTrackSelection,
    ) -> Result<(), Error> {
        match &mut self.selector {
            Some(selector) => selector.set_manual_selection(selection.fixed_index()),
            None if selection == VideoTrackSelection::Auto => Ok(()),
            None => Err(Error::Unsupported(String::from(
                "direct HLS media playlist has no selectable video representations",
            ))),
        }
    }

    /// Seeks a finite playlist to the segment containing normalized progress.
    ///
    /// The next decode sample is marked discontinuous so the codec layer drains
    /// and restarts at the selected segment's random-access point.
    ///
    /// # Errors
    ///
    /// Returns an error for a live playlist, an empty finite playlist, or a
    /// segment timeline that overflows.
    pub fn seek_to_progress(&mut self, progress: f64) -> Result<Duration, Error> {
        let selected_start = self.main.seek_to_progress(progress)?;
        if let Some(audio) = &mut self.alternate_audio {
            audio.seek_to_progress(progress)?;
        }
        if let Some(subtitles) = &mut self.alternate_subtitles {
            subtitles.seek_to_progress(progress)?;
        }
        Ok(selected_start)
    }

    /// Seeks a live playlist to an absolute presentation position.
    ///
    /// The next decode sample is marked discontinuous so decoders restart at
    /// the selected segment boundary.
    ///
    /// # Errors
    ///
    /// Returns an error for finite media, an empty playlist, or a position
    /// outside the current live seek window.
    pub fn seek_to_live_position(&mut self, position: Duration) -> Result<Duration, Error> {
        if !self.is_live() {
            return Err(Error::Unsupported(String::from(
                "live seek requires an HLS live playlist",
            )));
        }
        let selected_start = self.main.seek_to_position(position)?;
        if let Some(audio) = &mut self.alternate_audio {
            audio.seek_to_position(position)?;
        }
        if let Some(subtitles) = &mut self.alternate_subtitles {
            subtitles.seek_to_position(position)?;
        }
        Ok(selected_start)
    }

    /// Downloads and demuxes the next available media sequence.
    ///
    /// ABR changes happen only at this segment boundary. A finite playlist returns
    /// [`HlsSegmentPoll::EndOfStream`]; a live playlist returns a manifest-derived
    /// retry duration instead of performing an unobservable fixed sleep.
    ///
    /// # Errors
    ///
    /// Returns an error for network, playlist, encryption, initialization,
    /// container, or track configuration failures.
    pub async fn next_segment(
        &mut self,
        buffered: Duration,
        mut supports: impl FnMut(&StreamVariant) -> bool,
    ) -> Result<HlsSegmentPoll, Error> {
        self.select_variant(buffered, &mut supports).await?;
        let mut stream_order = [
            HlsStreamKind::Main,
            HlsStreamKind::AlternateAudio,
            HlsStreamKind::AlternateSubtitles,
        ];
        stream_order.sort_by_key(|kind| {
            let separate_track_priority = match kind {
                HlsStreamKind::AlternateSubtitles => 0_u8,
                HlsStreamKind::AlternateAudio => 1,
                HlsStreamKind::Main => 2,
            };
            (self.stream_start(*kind), separate_track_priority)
        });
        let mut retry_after = None;
        for kind in stream_order {
            if kind == HlsStreamKind::AlternateAudio && self.alternate_audio.is_none() {
                continue;
            }
            if kind == HlsStreamKind::AlternateSubtitles && self.alternate_subtitles.is_none() {
                continue;
            }
            match self.poll_stream(kind).await? {
                HlsStreamPoll::Ready(segment) => return Ok(HlsSegmentPoll::Ready(segment)),
                HlsStreamPoll::Subtitles(segment) => {
                    return Ok(HlsSegmentPoll::Subtitles(segment));
                }
                HlsStreamPoll::AwaitingPlaylist { retry_after: delay } => {
                    retry_after =
                        Some(retry_after.map_or(delay, |current: Duration| current.min(delay)));
                }
                HlsStreamPoll::EndOfStream => {}
            }
        }
        Ok(
            retry_after.map_or(HlsSegmentPoll::EndOfStream, |retry_after| {
                HlsSegmentPoll::AwaitingPlaylist { retry_after }
            }),
        )
    }

    fn stream_start(&self, kind: HlsStreamKind) -> Duration {
        match kind {
            HlsStreamKind::Main => self.main.next_start,
            HlsStreamKind::AlternateAudio => self
                .alternate_audio
                .as_ref()
                .map_or(Duration::MAX, |stream| stream.next_start),
            HlsStreamKind::AlternateSubtitles => self
                .alternate_subtitles
                .as_ref()
                .map_or(Duration::MAX, |stream| stream.next_start),
        }
    }

    async fn select_variant(
        &mut self,
        buffered: Duration,
        supports: &mut impl FnMut(&StreamVariant) -> bool,
    ) -> Result<(), Error> {
        let Some(selector) = &mut self.selector else {
            return Ok(());
        };
        let selected = selector
            .select(
                self.loader.estimated_bits_per_second(),
                buffered,
                self.viewport,
                supports,
            )?
            .clone();
        if self
            .selected_variant
            .as_ref()
            .is_some_and(|current| current.url == selected.url)
        {
            return Ok(());
        }
        let main_position = self.main.next_start;
        let main_sequence = self.main.next_sequence;
        let main_part = self.main.next_part;
        let request = self.request_context.related(selected.url.clone());
        let playlist = fetch_media_playlist(request.clone()).await?;
        let mut main = MediaStreamState::new(String::from("main"), request, playlist);
        main.align_after_switch(main_position, main_sequence, main_part)?;

        let audio_group_changed = self
            .selected_variant
            .as_ref()
            .and_then(|variant| variant.audio_group_id.as_deref())
            != selected.audio_group_id.as_deref();
        if audio_group_changed {
            let mut alternate_audio = open_audio_rendition(
                &self.request_context,
                &self.renditions,
                &selected,
                self.audio_track_selection,
            )
            .await?;
            if let Some(audio) = &mut alternate_audio {
                let initial_sequence = audio.next_sequence;
                let initial_part = audio.next_part;
                audio.align_after_switch(main.next_start, initial_sequence, initial_part)?;
            }
            self.alternate_audio = alternate_audio;
        }
        let subtitle_group_changed = self
            .selected_variant
            .as_ref()
            .and_then(|variant| variant.subtitle_group_id.as_deref())
            != selected.subtitle_group_id.as_deref();
        if subtitle_group_changed {
            let mut alternate_subtitles = open_subtitle_rendition(
                &self.request_context,
                &self.renditions,
                &selected,
                self.subtitle_track_selection,
            )
            .await?;
            if let Some(subtitles) = &mut alternate_subtitles {
                let initial_sequence = subtitles.next_sequence;
                let initial_part = subtitles.next_part;
                subtitles.align_after_switch(main.next_start, initial_sequence, initial_part)?;
            }
            self.alternate_subtitles = alternate_subtitles;
        }
        self.main = main;
        self.selected_variant = Some(selected);
        Ok(())
    }

    async fn poll_stream(&mut self, kind: HlsStreamKind) -> Result<HlsStreamPoll, Error> {
        if kind == HlsStreamKind::AlternateSubtitles {
            let stream = self.alternate_subtitles.as_mut().ok_or_else(|| {
                Error::Streaming(String::from(
                    "HLS subtitle scheduler selected an absent rendition",
                ))
            })?;
            return poll_hls_subtitle_stream(
                stream,
                &mut self.loader,
                self.maximum_segment_bytes,
                &self.request_context,
                &mut self.encryption_keys,
            )
            .await;
        }
        let selected_variant = self.selected_variant.clone();
        let stream = match kind {
            HlsStreamKind::Main => &mut self.main,
            HlsStreamKind::AlternateAudio => self.alternate_audio.as_mut().ok_or_else(|| {
                Error::Streaming(String::from(
                    "HLS alternate-audio scheduler selected an absent rendition",
                ))
            })?,
            HlsStreamKind::AlternateSubtitles => unreachable!(
                "alternate subtitles are dispatched before selecting an audiovisual stream"
            ),
        };
        poll_hls_stream(
            stream,
            &mut self.loader,
            self.maximum_segment_bytes,
            &self.request_context,
            &mut self.encryption_keys,
            &mut self.track_namespace,
            selected_variant,
        )
        .await
    }
}

async fn open_subtitle_rendition(
    manifest_request: &MediaRequest,
    renditions: &[HlsRendition],
    variant: &StreamVariant,
    selection: SubtitleTrackSelection,
) -> Result<Option<MediaStreamState>, Error> {
    if selection == SubtitleTrackSelection::Off {
        return Ok(None);
    }
    let Some(group_id) = variant.subtitle_group_id.as_deref() else {
        return Ok(None);
    };
    let candidates = subtitle_renditions(renditions, group_id).collect::<Vec<_>>();
    let rendition = match selection {
        SubtitleTrackSelection::Auto => candidates.iter().copied().min_by_key(|rendition| {
            match (
                rendition.is_forced,
                rendition.is_default,
                rendition.is_autoselect,
            ) {
                (true, _, _) => 0_u8,
                (false, true, _) => 1,
                (false, false, true) => 2,
                (false, false, false) => 3,
            }
        }),
        SubtitleTrackSelection::Track(index) => candidates.get(index).copied(),
        SubtitleTrackSelection::Off => None,
    }
    .ok_or_else(|| {
        Error::Streaming(match selection {
            SubtitleTrackSelection::Auto => {
                format!("HLS variant references absent subtitle rendition group {group_id}")
            }
            SubtitleTrackSelection::Track(index) => format!(
                "HLS subtitle track {index} is outside rendition group {group_id} with {} tracks",
                candidates.len()
            ),
            SubtitleTrackSelection::Off => {
                String::from("disabled HLS subtitles unexpectedly reached rendition selection")
            }
        })
    })?;
    let url = rendition.url.clone().ok_or_else(|| {
        Error::Streaming(format!(
            "HLS subtitle rendition {:?} has no media-playlist URI",
            rendition.name
        ))
    })?;
    let request = manifest_request.related(url);
    let playlist = fetch_media_playlist(request.clone()).await?;
    Ok(Some(MediaStreamState::new(
        format!("subtitles:{group_id}:{}", rendition.name),
        request,
        playlist,
    )))
}

fn subtitle_renditions<'a>(
    renditions: &'a [HlsRendition],
    group_id: &'a str,
) -> impl Iterator<Item = &'a HlsRendition> + 'a {
    renditions.iter().filter(move |rendition| {
        rendition.kind == HlsRenditionKind::Subtitles && rendition.group_id == group_id
    })
}

async fn open_audio_rendition(
    manifest_request: &MediaRequest,
    renditions: &[HlsRendition],
    variant: &StreamVariant,
    selection: AudioTrackSelection,
) -> Result<Option<MediaStreamState>, Error> {
    let Some(group_id) = variant.audio_group_id.as_deref() else {
        return Ok(None);
    };
    let candidates = audio_renditions(renditions, group_id).collect::<Vec<_>>();
    let rendition = select_audio_rendition(&candidates, group_id, selection)?;
    let Some(url) = rendition.url.clone() else {
        return Ok(None);
    };
    let request = manifest_request.related(url);
    let playlist = fetch_media_playlist(request.clone()).await?;
    Ok(Some(MediaStreamState::new(
        format!("audio:{group_id}:{}", rendition.name),
        request,
        playlist,
    )))
}

fn select_audio_rendition<'a>(
    candidates: &'a [&HlsRendition],
    group_id: &str,
    selection: AudioTrackSelection,
) -> Result<&'a HlsRendition, Error> {
    match selection {
        AudioTrackSelection::Auto => candidates.iter().copied().min_by_key(|rendition| {
            match (rendition.is_default, rendition.is_autoselect) {
                (true, _) => 0_u8,
                (false, true) => 1,
                (false, false) => 2,
            }
        }),
        AudioTrackSelection::Track(index) => candidates.get(index).copied(),
    }
    .ok_or_else(|| {
        Error::Streaming(match selection {
            AudioTrackSelection::Auto => {
                format!("HLS variant references absent audio rendition group {group_id}")
            }
            AudioTrackSelection::Track(index) => format!(
                "HLS audio track {index} is outside rendition group {group_id} with {} tracks",
                candidates.len()
            ),
        })
    })
}

fn audio_renditions<'a>(
    renditions: &'a [HlsRendition],
    group_id: &'a str,
) -> impl Iterator<Item = &'a HlsRendition> + 'a {
    renditions.iter().filter(move |rendition| {
        rendition.kind == HlsRenditionKind::Audio && rendition.group_id == group_id
    })
}

async fn poll_hls_stream(
    state: &mut MediaStreamState,
    loader: &mut SegmentLoader,
    maximum_segment_bytes: NonZeroUsize,
    request_context: &MediaRequest,
    encryption_keys: &mut BTreeMap<String, [u8; 16]>,
    track_namespace: &mut TrackNamespace,
    selected_variant: Option<StreamVariant>,
) -> Result<HlsStreamPoll, Error> {
    let Some(unit) = locate_playable_hls_media_unit(state).await? else {
        return if state.playlist.ended {
            Ok(HlsStreamPoll::EndOfStream)
        } else {
            Ok(HlsStreamPoll::AwaitingPlaylist {
                retry_after: state.reload_delay(),
            })
        };
    };
    let sequence = unit.sequence();
    let part_index = unit.part_index();
    let duration = unit.duration();
    let content_protection = protected_hls_options(unit.encryption())?;
    if !content_protection.is_empty() && unit.initialization().is_none() {
        return Err(Error::Unsupported(format!(
            "protected HLS sequence {sequence} uses MPEG-TS; platform-CDM playback requires CMAF"
        )));
    }
    let discontinuity = std::mem::take(&mut state.force_discontinuity) || unit.discontinuity();
    let resource = unit
        .resource(maximum_segment_bytes)?
        .with_request_context(&state.request);
    let fetched = loader.load(&resource).await?;
    let estimated_bits_per_second = fetched.estimated_bits_per_second();
    let bytes = decrypt_hls_media(
        sequence,
        unit.encryption(),
        fetched.into_bytes(),
        request_context,
        encryption_keys,
    )
    .await?;
    let (tracks, samples, timed_metadata, protection_init_data) = match unit.initialization() {
        Some(initialization) => {
            ensure_hls_cmaf(state, initialization, loader, maximum_segment_bytes).await?;
            let cmaf = state.cmaf.as_mut().ok_or_else(|| {
                Error::Container(String::from(
                    "CMAF initialization completed without creating a demuxer",
                ))
            })?;
            let tracks = cmaf.demuxer.tracks().to_vec();
            let (samples, timed_metadata) = cmaf
                .demuxer
                .demux_segment(&bytes, discontinuity)?
                .into_parts();
            (
                tracks,
                samples,
                timed_metadata,
                cmaf.protection_init_data.clone(),
            )
        }
        None => match unit {
            HlsMediaUnit::Segment(_) => {
                let DemuxedTransportStream {
                    tracks,
                    samples,
                    timed_metadata,
                } = demux_transport_stream(&bytes, discontinuity)?;
                (tracks, samples, timed_metadata, Vec::new())
            }
            HlsMediaUnit::Partial(_) => {
                return Err(Error::Unsupported(String::from(
                    "Low-Latency HLS partial segments require a CMAF initialization section",
                )));
            }
        },
    };
    if !content_protection.is_empty() && tracks.iter().all(|track| track.protection().is_none()) {
        return Err(Error::Container(format!(
            "protected HLS sequence {sequence} declares a DRM key format but its CMAF initialization has no tenc metadata"
        )));
    }
    let (tracks, samples) = track_namespace.remap(&state.identity, tracks, samples)?;
    unit.advance(state)?;
    Ok(HlsStreamPoll::Ready(Box::new(StreamedSegment {
        sequence,
        part_index,
        duration,
        tracks,
        samples,
        timed_metadata,
        protection_init_data,
        content_protection,
        selected_variant,
        estimated_bits_per_second,
    })))
}

async fn poll_hls_subtitle_stream(
    state: &mut MediaStreamState,
    loader: &mut SegmentLoader,
    maximum_segment_bytes: NonZeroUsize,
    request_context: &MediaRequest,
    encryption_keys: &mut BTreeMap<String, [u8; 16]>,
) -> Result<HlsStreamPoll, Error> {
    let Some(unit) = locate_playable_hls_media_unit(state).await? else {
        return if state.playlist.ended {
            Ok(HlsStreamPoll::EndOfStream)
        } else {
            Ok(HlsStreamPoll::AwaitingPlaylist {
                retry_after: state.reload_delay(),
            })
        };
    };
    let segment_start = state.next_start;
    let sequence = unit.sequence();
    let part_index = unit.part_index();
    let duration = unit.duration();
    let discontinuity = std::mem::take(&mut state.force_discontinuity) || unit.discontinuity();
    if !protected_hls_options(unit.encryption())?.is_empty() {
        return Err(Error::Unsupported(format!(
            "protected HLS subtitle sequence {sequence} cannot be exposed outside the platform CDM"
        )));
    }
    let resource = unit
        .resource(maximum_segment_bytes)?
        .with_request_context(&state.request);
    let bytes = decrypt_hls_media(
        sequence,
        unit.encryption(),
        loader.load(&resource).await?.into_bytes(),
        request_context,
        encryption_keys,
    )
    .await?;
    let (cues, timed_metadata) = if let Some(initialization) = unit.initialization() {
        ensure_hls_cmaf(state, initialization, loader, maximum_segment_bytes).await?;
        let cmaf = state.cmaf.as_mut().ok_or_else(|| {
            Error::Container(String::from(
                "HLS subtitle initialization did not create a CMAF demuxer",
            ))
        })?;
        let tracks = cmaf.demuxer.tracks().to_vec();
        let (samples, timed_metadata) = cmaf
            .demuxer
            .demux_segment(&bytes, discontinuity)?
            .into_parts();
        let mut cues = Vec::new();
        for track in tracks
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
        if tracks
            .iter()
            .all(|track| track.kind() != TrackKind::Subtitle)
        {
            return Err(Error::Container(String::from(
                "HLS subtitle initialization has no timed-text track",
            )));
        }
        (cues, timed_metadata)
    } else {
        if matches!(unit, HlsMediaUnit::Partial(_)) {
            return Err(Error::Unsupported(String::from(
                "Low-Latency HLS subtitle parts require a CMAF initialization section",
            )));
        }
        let document = std::str::from_utf8(&bytes).map_err(|error| {
            Error::Container(format!("HLS WebVTT segment is not valid UTF-8: {error}"))
        })?;
        (
            parse_hls_webvtt_segment(document, segment_start)?,
            Vec::new(),
        )
    };
    unit.advance(state)?;
    Ok(HlsStreamPoll::Subtitles(Box::new(
        StreamedSubtitleSegment {
            sequence,
            part_index,
            duration,
            cues,
            timed_metadata,
        },
    )))
}

async fn locate_playable_hls_media_unit(
    state: &mut MediaStreamState,
) -> Result<Option<HlsMediaUnit>, Error> {
    loop {
        let Some(unit) = locate_hls_media_unit(state).await? else {
            return Ok(None);
        };
        if !unit.gap() {
            return Ok(Some(unit));
        }
        state.force_discontinuity |= unit.discontinuity();
        unit.advance(state)?;
    }
}

async fn locate_hls_media_unit(
    state: &mut MediaStreamState,
) -> Result<Option<HlsMediaUnit>, Error> {
    if let Some(unit) = locate_published_hls_media_unit(state)? {
        return Ok(Some(unit));
    }
    if state.playlist.ended {
        return Ok(None);
    }
    let playlist = fetch_media_playlist(state.reload_request()).await?;
    state.replace_playlist(playlist)?;
    locate_published_hls_media_unit(state)
}

fn locate_published_hls_media_unit(
    state: &mut MediaStreamState,
) -> Result<Option<HlsMediaUnit>, Error> {
    if state.next_sequence < state.playlist.media_sequence {
        state.next_sequence = state.playlist.media_sequence;
        state.next_part = 0;
        state.next_start = state.playlist_start;
        state.force_discontinuity = true;
        state.cmaf = None;
    }
    loop {
        let complete = state
            .playlist
            .segments
            .iter()
            .find(|segment| segment.sequence == state.next_sequence)
            .cloned();
        if state.next_part == 0 {
            if let Some(segment) = complete {
                return Ok(Some(HlsMediaUnit::Segment(segment)));
            }
        } else if let Some(segment) = complete {
            let segment_start = state
                .position_for_sequence(segment.sequence)
                .ok_or_else(|| {
                    Error::Streaming(format!(
                        "HLS completed sequence {} has no presentation position",
                        segment.sequence
                    ))
                })?;
            state.next_sequence = segment.sequence.checked_add(1).ok_or_else(|| {
                Error::Streaming(String::from("HLS media sequence overflowed usize"))
            })?;
            state.next_part = 0;
            state.next_start = segment_start.saturating_add(segment.duration);
            continue;
        }
        let part = state
            .playlist
            .low_latency
            .as_ref()
            .and_then(|low_latency| {
                low_latency.trailing_parts.iter().find(|part| {
                    part.sequence == state.next_sequence && part.part_index == state.next_part
                })
            })
            .cloned();
        return Ok(part.map(HlsMediaUnit::Partial));
    }
}

async fn ensure_hls_cmaf(
    state: &mut MediaStreamState,
    initialization: &HlsInitializationSegment,
    loader: &mut SegmentLoader,
    maximum_segment_bytes: NonZeroUsize,
) -> Result<(), Error> {
    if state
        .cmaf
        .as_ref()
        .is_some_and(|cmaf| cmaf.initialization == *initialization)
    {
        return Ok(());
    }
    let resource = SegmentResource::for_hls_initialization(initialization, maximum_segment_bytes)?
        .with_request_context(&state.request);
    let bytes = loader.load(&resource).await?.into_bytes();
    let parsed = CmafInitialization::parse(&bytes)?;
    let protection_init_data = parsed.protection_init_data().to_vec();
    state.cmaf = Some(CmafState {
        initialization: initialization.clone(),
        demuxer: CmafDemuxer::new(parsed),
        protection_init_data,
    });
    Ok(())
}

async fn decrypt_hls_media(
    sequence: usize,
    encryption_options: &[HlsEncryption],
    ciphertext: bytes::Bytes,
    request_context: &MediaRequest,
    encryption_keys: &mut BTreeMap<String, [u8; 16]>,
) -> Result<bytes::Bytes, Error> {
    if encryption_options.is_empty() {
        return Ok(ciphertext);
    }
    let encryption = encryption_options
        .iter()
        .find(|encryption| {
            encryption.method == HlsEncryptionMethod::Aes128
                && encryption.key_format.eq_ignore_ascii_case("identity")
        })
        .cloned();
    let Some(encryption) = encryption else {
        protected_hls_options(encryption_options)?;
        return Ok(ciphertext);
    };
    let key = load_hls_identity_key(&encryption, request_context, encryption_keys).await?;
    let initialization_vector = encryption.initialization_vector.unwrap_or_else(|| {
        u128::try_from(sequence)
            .expect("HLS media sequence must fit the 128-bit default initialization vector")
            .to_be_bytes()
    });
    let mut plaintext = ciphertext.to_vec();
    let plaintext_length = Aes128CbcDecryptor::new(&key.into(), &initialization_vector.into())
        .decrypt_padded::<Pkcs7>(&mut plaintext)
        .map_err(|error| {
            Error::Streaming(format!(
                "HLS AES-128 sequence {sequence} has invalid CBC padding: {error}"
            ))
        })?
        .len();
    plaintext.truncate(plaintext_length);
    Ok(plaintext.into())
}

fn protected_hls_options(
    encryption_options: &[HlsEncryption],
) -> Result<Vec<HlsEncryption>, Error> {
    if encryption_options.iter().any(|encryption| {
        encryption.method == HlsEncryptionMethod::Aes128
            && encryption.key_format.eq_ignore_ascii_case("identity")
    }) {
        return Ok(Vec::new());
    }
    encryption_options
        .iter()
        .map(|encryption| {
            if encryption.key_format.eq_ignore_ascii_case("identity") {
                return Err(Error::Unsupported(format!(
                    "HLS identity key uses unsupported sample-level method {:?}",
                    encryption.method
                )));
            }
            if !matches!(
                encryption.method,
                HlsEncryptionMethod::SampleAes | HlsEncryptionMethod::SampleAesCtr
            ) {
                return Err(Error::Unsupported(format!(
                    "HLS key format {:?} uses whole-segment encryption that cannot be delegated to a platform CDM",
                    encryption.key_format
                )));
            }
            Ok(encryption.clone())
        })
        .collect()
}

async fn load_hls_identity_key(
    encryption: &HlsEncryption,
    request_context: &MediaRequest,
    encryption_keys: &mut BTreeMap<String, [u8; 16]>,
) -> Result<[u8; 16], Error> {
    let cache_key = encryption.key_url.as_str();
    if let Some(key) = encryption_keys.get(cache_key) {
        return Ok(*key);
    }
    let maximum_key_bytes = NonZeroUsize::new(16).expect("AES-128 key size must be non-zero");
    let request = request_context
        .related(encryption.key_url.clone())
        .with_maximum_response_bytes(maximum_key_bytes);
    let response = fetch_media(request).await?;
    let key = <[u8; 16]>::try_from(response.bytes()).map_err(|_| {
        Error::Streaming(format!(
            "HLS identity key {} must contain exactly 16 bytes",
            encryption.key_url
        ))
    })?;
    encryption_keys.insert(cache_key.to_owned(), key);
    Ok(key)
}

fn allocate_hls_track_id(next: &mut Option<NonZeroU32>) -> Result<TrackId, Error> {
    let value = next.take().ok_or_else(|| {
        Error::Container(String::from(
            "HLS presentation exhausted 32-bit track identities",
        ))
    })?;
    *next = value.get().checked_add(1).and_then(NonZeroU32::new);
    TrackId::new(value.get())
}

async fn fetch_media_playlist(request: MediaRequest) -> Result<HlsMediaPlaylist, Error> {
    match fetch_hls_playlist(request).await? {
        HlsPlaylist::Media(media) => Ok(*media),
        HlsPlaylist::Master(_) => Err(Error::Streaming(String::from(
            "HLS variant URL resolved to another multivariant playlist",
        ))),
    }
}

struct DemuxedTransportStream {
    tracks: Vec<TrackInfo>,
    samples: Vec<EncodedSample>,
    timed_metadata: Vec<TimedMetadata>,
}

fn demux_transport_stream(
    bytes: &[u8],
    discontinuity: bool,
) -> Result<DemuxedTransportStream, Error> {
    let mut demuxer = MpegTsDemuxer::new();
    let mut events = demuxer.feed(bytes)?;
    events.extend(demuxer.finish()?);
    let mut tracks = Vec::new();
    let mut samples = Vec::new();
    let mut discontinuous_tracks = BTreeSet::<TrackId>::new();
    for event in events {
        match event {
            MpegTsEvent::Track(track) => tracks.push(track),
            MpegTsEvent::Sample(sample) => {
                let starts_discontinuity =
                    discontinuity && discontinuous_tracks.insert(sample.track_id());
                let sample_discontinuity = sample.is_discontinuity() || starts_discontinuity;
                samples.push(sample.with_discontinuity(sample_discontinuity));
            }
            MpegTsEvent::TracksResolved => {}
            _ => {
                return Err(Error::Unsupported(String::from(
                    "container emitted an unknown MPEG-TS event",
                )));
            }
        }
    }
    if tracks.is_empty() || samples.is_empty() {
        return Err(Error::Container(String::from(
            "HLS MPEG-TS segment did not contain configured media samples",
        )));
    }
    let codecs = tracks
        .iter()
        .map(|track| (track.id(), track.codec()))
        .collect::<BTreeMap<_, _>>();
    let mut media_samples = Vec::with_capacity(samples.len());
    let mut timed_metadata = Vec::new();
    for sample in samples {
        match codecs.get(&sample.track_id()) {
            Some(Codec::Id3) => timed_metadata.push(decode_mpeg_ts_id3_metadata(&sample)?),
            Some(_) => media_samples.push(sample),
            None => {
                return Err(Error::Container(format!(
                    "MPEG-TS sample references undeclared track {}",
                    sample.track_id().get()
                )));
            }
        }
    }
    if media_samples.is_empty() {
        return Err(Error::Container(String::from(
            "HLS MPEG-TS segment contains metadata but no playable media samples",
        )));
    }
    Ok(DemuxedTransportStream {
        tracks,
        samples: media_samples,
        timed_metadata,
    })
}

const ID3_TIMED_METADATA_SCHEME_ID_URI: &str = "https://aomedia.org/emsg/ID3";

fn decode_mpeg_ts_id3_metadata(sample: &EncodedSample) -> Result<TimedMetadata, Error> {
    validate_id3_tag(sample.data())?;
    Ok(TimedMetadata::new(
        ID3_TIMED_METADATA_SCHEME_ID_URI,
        "mpegts",
        0,
        sample.presentation_time().to_duration()?,
        sample.duration().to_duration()?,
        sample.data().clone(),
    ))
}

fn validate_id3_tag(data: &[u8]) -> Result<(), Error> {
    let header = data.get(..10).ok_or_else(|| {
        Error::Container(String::from(
            "HLS timed ID3 sample is shorter than its 10-byte tag header",
        ))
    })?;
    if &header[..3] != b"ID3" {
        return Err(Error::Container(String::from(
            "HLS timed ID3 sample does not start with an ID3v2 tag",
        )));
    }
    let version = header[3];
    if !(2..=4).contains(&version) || header[4] == 0xff {
        return Err(Error::Unsupported(format!(
            "unsupported HLS timed ID3 version {version}.{}",
            header[4]
        )));
    }
    let size_bytes: [u8; 4] = header[6..10]
        .try_into()
        .expect("validated ID3 header has four size bytes");
    if size_bytes.iter().any(|byte| byte & 0x80 != 0) {
        return Err(Error::Container(String::from(
            "HLS timed ID3 tag size is not sync-safe",
        )));
    }
    let payload_size = size_bytes
        .into_iter()
        .fold(0_usize, |size, byte| (size << 7) | usize::from(byte));
    let footer_size = usize::from(version == 4 && header[5] & 0x10 != 0) * 10;
    let declared_size = 10_usize
        .checked_add(payload_size)
        .and_then(|size| size.checked_add(footer_size))
        .ok_or_else(|| Error::Container(String::from("HLS timed ID3 tag size overflow")))?;
    if declared_size != data.len() {
        return Err(Error::Container(format!(
            "HLS timed ID3 tag declares {declared_size} bytes but its sample contains {}",
            data.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        num::{NonZeroU32, NonZeroU64, NonZeroUsize},
        thread,
        time::Duration,
    };

    use aes::cipher::{BlockModeEncrypt as _, KeyIvInit as _, block_padding::Pkcs7};
    use bytes::Bytes;
    use transmux::{
        AVCConfigurationBox, AVCDecoderConfigurationRecord, AvcPps, AvcSps, CodecConfig,
        DecoderConfigDescriptor, DecoderSpecificInfo, ESDescriptor, EsdsBox, FragmentTrackData,
        ObjectTypeIndication, SLConfigDescriptor, Sample, StreamType, TrackSpec,
        build_init_segment, build_media_segment,
    };
    use waterkit_video_container::{EncodedSample, MediaTime, TrackId, TrackKind};
    use waterkit_video_streaming::{
        AdaptiveSelectionPolicy, HlsMediaPlaylist, HlsRendition, HlsRenditionKind, HlsSegment,
        MediaRequest, Url,
    };

    use super::{
        AudioTrackSelection, HlsPlaybackSession, HlsSegmentPoll, MediaStreamState,
        SegmentedPlaybackOptions, VideoTrackSelection, decode_mpeg_ts_id3_metadata,
        select_audio_rendition, validate_id3_tag,
    };

    #[test]
    fn mpeg_ts_id3_sample_maps_to_timed_metadata() {
        let mut tag = b"ID3\x04\x00\x00\x00\x00\x00\x04".to_vec();
        tag.extend_from_slice(b"test");
        let timescale = NonZeroU32::new(90_000).expect("test timescale is non-zero");
        let sample = EncodedSample::new(
            TrackId::new(3).expect("test track identifier is non-zero"),
            MediaTime::new(180_000, timescale),
            MediaTime::new(180_000, timescale),
            MediaTime::new(45_000, timescale),
            true,
            Bytes::from(tag.clone()),
        );

        let metadata = decode_mpeg_ts_id3_metadata(&sample).expect("valid timed ID3 must decode");

        assert_eq!(metadata.scheme_id_uri(), "https://aomedia.org/emsg/ID3");
        assert_eq!(metadata.value(), "mpegts");
        assert_eq!(metadata.presentation_time(), Duration::from_secs(2));
        assert_eq!(metadata.duration(), Duration::from_millis(500));
        assert_eq!(metadata.message_data().as_ref(), tag.as_slice());
    }

    #[test]
    fn timed_id3_rejects_non_sync_safe_tag_size() {
        let malformed = b"ID3\x04\x00\x00\x80\x00\x00\x00";
        let error = validate_id3_tag(malformed).expect_err("invalid ID3 size must fail");
        assert!(error.to_string().contains("not sync-safe"));
    }

    #[test]
    fn live_hls_window_preserves_time_across_playlist_slides() {
        let request = MediaRequest::new(
            Url::parse("https://storage.googleapis.com/waterui-media/live.m3u8")
                .expect("test URL must parse"),
            NonZeroUsize::new(16 * 1_024).expect("test manifest limit is non-zero"),
        );
        let mut state = MediaStreamState::new(String::from("main"), request, live_playlist(100, 3));

        let initial = state.live_window().expect("playlist must be live");
        assert_eq!(initial.seekable_start(), Duration::ZERO);
        assert_eq!(initial.live_edge(), Duration::from_secs(12));

        state
            .replace_playlist(live_playlist(102, 3))
            .expect("overlapping playlist slide must preserve its timeline");
        let slid = state.live_window().expect("playlist must remain live");
        assert_eq!(slid.seekable_start(), Duration::from_secs(8));
        assert_eq!(slid.live_edge(), Duration::from_secs(20));
        assert_eq!(slid.target_position(), Duration::from_secs(8));
        assert_eq!(
            state
                .seek_to_position(Duration::from_secs(16))
                .expect("DVR position must be seekable"),
            Duration::from_secs(16)
        );
        assert_eq!(state.next_sequence, 104);
    }

    fn live_playlist(media_sequence: usize, segment_count: usize) -> HlsMediaPlaylist {
        let base = Url::parse("https://storage.googleapis.com/waterui-media/")
            .expect("test base URL must parse");
        HlsMediaPlaylist {
            target_duration: Duration::from_secs(4),
            media_sequence,
            discontinuity_sequence: 0,
            segments: (0..segment_count)
                .map(|offset| HlsSegment {
                    sequence: media_sequence + offset,
                    url: base
                        .join(&format!("segment-{}.m4s", media_sequence + offset))
                        .expect("test segment URL must resolve"),
                    duration: Duration::from_secs(4),
                    byte_range: None,
                    initialization: None,
                    encryption: Vec::new(),
                    discontinuity: false,
                    gap: false,
                })
                .collect(),
            ended: false,
            independent_segments: true,
            delta_update: None,
            server_control: None,
            low_latency: None,
        }
    }

    #[test]
    fn hls_session_fetches_init_and_demuxes_media_without_copying_protocol_layers() {
        let track = video_track();
        let init = build_init_segment(std::slice::from_ref(&track), 1_000)
            .expect("test initialization must build");
        let samples = [Sample::new(vec![0, 0, 0, 1, 0x65], 3_000, true, 0)];
        let segment = build_media_segment(
            1,
            &[FragmentTrackData {
                track_id: track.track_id,
                base_media_decode_time: 0,
                samples: &samples,
            }],
        )
        .expect("test media segment must build");
        let server = TestHlsServer::start(init, segment);
        let manifest_url = Url::parse(&format!("http://{}/master.m3u8", server.address))
            .expect("test manifest URL must parse");
        let request = MediaRequest::new(
            manifest_url,
            NonZeroUsize::new(16 * 1_024).expect("test manifest limit is non-zero"),
        );
        let options = SegmentedPlaybackOptions::new(
            NonZeroUsize::new(1024 * 1_024).expect("test segment limit is non-zero"),
            NonZeroU64::new(8_000_000).expect("test bandwidth is non-zero"),
            AdaptiveSelectionPolicy::default(),
            Some((1_920, 1_080)),
        );
        let segment = futures::executor::block_on(async {
            let mut session = HlsPlaybackSession::open(request, options, |_| true)
                .await
                .expect("test session must open");
            let tracks = session.video_tracks();
            assert_eq!(tracks.len(), 1);
            assert_eq!(tracks[0].dimensions(), Some((1920, 1080)));
            session
                .set_video_track_selection(VideoTrackSelection::Track(0))
                .expect("advertised video track must be selectable");
            match session
                .next_segment(Duration::from_secs(12), |_| true)
                .await
                .expect("test segment must load")
            {
                HlsSegmentPoll::Ready(segment) => segment,
                other => panic!("expected a ready segment, got {other:?}"),
            }
        });
        assert_eq!(segment.sequence(), 0);
        assert_eq!(segment.tracks().len(), 1);
        assert_eq!(segment.samples().len(), 1);
        assert_eq!(segment.samples()[0].data().as_ref(), samples[0].data);
        server.finish();
    }

    #[test]
    fn low_latency_hls_demuxes_parts_and_merges_blocking_delta_reload() {
        let track = video_track();
        let init = build_init_segment(std::slice::from_ref(&track), 1_000)
            .expect("test initialization must build");
        let sample_data = [
            vec![0, 0, 0, 1, 0x65, 0],
            vec![0, 0, 0, 1, 0x41, 1],
            vec![0, 0, 0, 1, 0x65, 2],
        ];
        let parts = sample_data
            .iter()
            .enumerate()
            .map(|(index, data)| {
                let sample = Sample::new(data.clone(), 500, index != 1, 0);
                build_media_segment(
                    u32::try_from(index + 1).expect("test sequence must fit u32"),
                    &[FragmentTrackData {
                        track_id: track.track_id,
                        base_media_decode_time: 4_000
                            + u64::try_from(index).expect("test index must fit u64") * 500,
                        samples: std::slice::from_ref(&sample),
                    }],
                )
                .expect("test partial segment must build")
            })
            .collect::<Vec<_>>();
        let server = TestLowLatencyHlsServer::start(init, parts);
        let request = MediaRequest::new(
            Url::parse(&format!("http://{}/master.m3u8", server.address))
                .expect("test manifest URL must parse"),
            NonZeroUsize::new(16 * 1_024).expect("test manifest limit is non-zero"),
        );
        let options = SegmentedPlaybackOptions::new(
            NonZeroUsize::new(1024 * 1_024).expect("test segment limit is non-zero"),
            NonZeroU64::new(8_000_000).expect("test bandwidth is non-zero"),
            AdaptiveSelectionPolicy::default(),
            Some((1_920, 1_080)),
        );

        let streamed = futures::executor::block_on(async {
            let mut session = HlsPlaybackSession::open(request, options, |_| true)
                .await
                .expect("test LL-HLS session must open");
            let window = session
                .live_window()
                .expect("LL-HLS must expose a live window");
            assert_eq!(window.live_edge(), Duration::from_secs(5));
            assert_eq!(window.target_position(), Duration::from_secs(4));
            let mut streamed = Vec::new();
            for _ in 0..3 {
                match session
                    .next_segment(Duration::from_secs(2), |_| true)
                    .await
                    .expect("published LL-HLS part must load")
                {
                    HlsSegmentPoll::Ready(segment) => streamed.push(segment),
                    other => panic!("expected a ready LL-HLS part, got {other:?}"),
                }
            }
            streamed
        });

        assert_eq!(streamed[0].sequence(), 101);
        assert_eq!(streamed[0].part_index(), Some(0));
        assert_eq!(streamed[1].sequence(), 101);
        assert_eq!(streamed[1].part_index(), Some(1));
        assert_eq!(streamed[2].sequence(), 102);
        assert_eq!(streamed[2].part_index(), Some(0));
        for (segment, expected) in streamed.iter().zip(sample_data) {
            assert_eq!(segment.samples().len(), 1);
            assert_eq!(segment.samples()[0].data().as_ref(), expected);
        }
        server.finish();
    }

    #[test]
    fn hls_session_decrypts_identity_aes128_before_cmaf_demux() {
        let track = video_track();
        let init = build_init_segment(std::slice::from_ref(&track), 1_000)
            .expect("test initialization must build");
        let samples = [Sample::new(vec![0, 0, 0, 1, 0x65], 3_000, true, 0)];
        let plaintext = build_media_segment(
            1,
            &[FragmentTrackData {
                track_id: track.track_id,
                base_media_decode_time: 0,
                samples: &samples,
            }],
        )
        .expect("test media segment must build");
        let key = [0x42_u8; 16];
        let initialization_vector = [0_u8; 16];
        let ciphertext =
            cbc::Encryptor::<aes::Aes128>::new(&key.into(), &initialization_vector.into())
                .encrypt_padded_vec::<Pkcs7>(&plaintext);
        let server = TestHlsServer::start_encrypted(init, ciphertext, key);
        let request = MediaRequest::new(
            Url::parse(&format!("http://{}/master.m3u8", server.address))
                .expect("test manifest URL must parse"),
            NonZeroUsize::new(16 * 1_024).expect("test manifest limit is non-zero"),
        );
        let options = SegmentedPlaybackOptions::new(
            NonZeroUsize::new(1024 * 1_024).expect("test segment limit is non-zero"),
            NonZeroU64::new(8_000_000).expect("test bandwidth is non-zero"),
            AdaptiveSelectionPolicy::default(),
            Some((1_920, 1_080)),
        );

        let streamed = futures::executor::block_on(async {
            let mut session = HlsPlaybackSession::open(request, options, |_| true)
                .await
                .expect("encrypted test session must open");
            match session
                .next_segment(Duration::from_secs(12), |_| true)
                .await
                .expect("encrypted test segment must load")
            {
                HlsSegmentPoll::Ready(segment) => segment,
                other => panic!("expected a decrypted ready segment, got {other:?}"),
            }
        });
        assert_eq!(streamed.samples().len(), 1);
        assert_eq!(streamed.samples()[0].data().as_ref(), samples[0].data);
        server.finish();
    }

    #[test]
    fn hls_session_interleaves_independently_numbered_alternate_audio() {
        let video_track = video_track();
        let video_init = build_init_segment(std::slice::from_ref(&video_track), 1_000)
            .expect("video initialization must build");
        let video_sample = Sample::new(vec![0, 0, 0, 1, 0x65], 3_000, true, 0);
        let video_segment = build_media_segment(
            1,
            &[FragmentTrackData {
                track_id: video_track.track_id,
                base_media_decode_time: 0,
                samples: std::slice::from_ref(&video_sample),
            }],
        )
        .expect("video media segment must build");
        let audio_track = audio_track();
        let audio_init = build_init_segment(std::slice::from_ref(&audio_track), 1_000)
            .expect("audio initialization must build");
        let audio_sample = Sample::new(vec![0x21, 0x10, 0x04, 0x60], 1_024, true, 0);
        let audio_segment = build_media_segment(
            100,
            &[FragmentTrackData {
                track_id: audio_track.track_id,
                base_media_decode_time: 0,
                samples: std::slice::from_ref(&audio_sample),
            }],
        )
        .expect("audio media segment must build");
        let server = TestHlsServer::start_with_alternate_audio(
            video_init,
            video_segment,
            audio_init,
            audio_segment,
        );
        let request = MediaRequest::new(
            Url::parse(&format!("http://{}/master.m3u8", server.address))
                .expect("test manifest URL must parse"),
            NonZeroUsize::new(16 * 1_024).expect("test manifest limit is non-zero"),
        );
        let options = SegmentedPlaybackOptions::new(
            NonZeroUsize::new(1024 * 1_024).expect("test segment limit is non-zero"),
            NonZeroU64::new(8_000_000).expect("test bandwidth is non-zero"),
            AdaptiveSelectionPolicy::default(),
            Some((1_920, 1_080)),
        );

        let (audio_tracks, audio, video) = futures::executor::block_on(async {
            let mut session = HlsPlaybackSession::open(request, options, |_| true)
                .await
                .expect("alternate-audio session must open");
            let audio_tracks = session.audio_tracks();
            let audio = match session
                .next_segment(Duration::from_secs(12), |_| true)
                .await
                .expect("audio segment must load")
            {
                HlsSegmentPoll::Ready(segment) => segment,
                other => panic!("expected an alternate-audio segment, got {other:?}"),
            };
            let video = match session
                .next_segment(Duration::from_secs(12), |_| true)
                .await
                .expect("video segment must load")
            {
                HlsSegmentPoll::Ready(segment) => segment,
                other => panic!("expected a main video segment, got {other:?}"),
            };
            (audio_tracks, audio, video)
        });
        assert_eq!(audio_tracks.len(), 1);
        assert_eq!(audio_tracks[0].label(), "English");
        assert_eq!(audio_tracks[0].language(), Some("en"));
        assert_eq!(audio.sequence(), 100);
        assert_eq!(audio.tracks()[0].kind(), TrackKind::Audio);
        assert_eq!(audio.samples()[0].data().as_ref(), audio_sample.data);
        assert_eq!(video.sequence(), 0);
        assert_eq!(video.tracks()[0].kind(), TrackKind::Video);
        assert_eq!(video.samples()[0].data().as_ref(), video_sample.data);
        assert_ne!(audio.tracks()[0].id(), video.tracks()[0].id());
        server.finish();
    }

    #[test]
    fn hls_session_decodes_selected_webvtt_rendition() {
        let track = video_track();
        let init = build_init_segment(std::slice::from_ref(&track), 1_000)
            .expect("video initialization must build");
        let sample = Sample::new(vec![0, 0, 0, 1, 0x65], 3_000, true, 0);
        let media = build_media_segment(
            1,
            &[FragmentTrackData {
                track_id: track.track_id,
                base_media_decode_time: 0,
                samples: std::slice::from_ref(&sample),
            }],
        )
        .expect("video media segment must build");
        let server = TestHlsServer::start_with_subtitles(init, media);
        let request = MediaRequest::new(
            Url::parse(&format!("http://{}/master.m3u8", server.address))
                .expect("test manifest URL must parse"),
            NonZeroUsize::new(16 * 1_024).expect("test manifest limit is non-zero"),
        );
        let options = SegmentedPlaybackOptions::new(
            NonZeroUsize::new(1024 * 1_024).expect("test segment limit is non-zero"),
            NonZeroU64::new(8_000_000).expect("test bandwidth is non-zero"),
            AdaptiveSelectionPolicy::default(),
            Some((1_920, 1_080)),
        );

        let (tracks, subtitles, video) = futures::executor::block_on(async {
            let mut session = HlsPlaybackSession::open(request, options, |_| true)
                .await
                .expect("subtitle session must open");
            let tracks = session.subtitle_tracks();
            let subtitles = match session
                .next_segment(Duration::from_secs(12), |_| true)
                .await
                .expect("subtitle segment must load")
            {
                HlsSegmentPoll::Subtitles(segment) => segment,
                other => panic!("expected a subtitle segment, got {other:?}"),
            };
            let video = match session
                .next_segment(Duration::from_secs(12), |_| true)
                .await
                .expect("video segment must load")
            {
                HlsSegmentPoll::Ready(segment) => segment,
                other => panic!("expected a video segment, got {other:?}"),
            };
            (tracks, subtitles, video)
        });

        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].label(), "English");
        assert_eq!(tracks[0].language(), Some("en"));
        assert!(!tracks[0].is_forced());
        assert_eq!(subtitles.sequence(), 50);
        assert_eq!(subtitles.cues().len(), 1);
        assert_eq!(subtitles.cues()[0].start, Duration::from_millis(500));
        assert_eq!(subtitles.cues()[0].text, "WaterKit subtitles");
        assert_eq!(video.samples()[0].data().as_ref(), sample.data);
        server.finish();
    }

    #[test]
    fn explicit_hls_audio_selection_rejects_out_of_range_track() {
        let rendition = HlsRendition {
            kind: HlsRenditionKind::Audio,
            url: None,
            group_id: String::from("stereo"),
            name: String::from("English"),
            language: Some(String::from("en")),
            is_default: true,
            is_autoselect: true,
            is_forced: false,
        };
        let candidates = [&rendition];

        let error = select_audio_rendition(&candidates, "stereo", AudioTrackSelection::Track(2))
            .expect_err("out-of-range HLS audio selection must fail");

        assert!(error.to_string().contains("with 1 tracks"));
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

    fn audio_track() -> TrackSpec {
        TrackSpec::new(
            1,
            44_100,
            CodecConfig::Aac {
                esds: EsdsBox {
                    es_descriptor: ESDescriptor {
                        es_id: 1,
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
                sample_rate: 44_100,
                channel_count: 2,
                sample_size: 16,
            },
        )
    }

    struct TestHlsServer {
        address: std::net::SocketAddr,
        worker: thread::JoinHandle<()>,
    }

    struct TestLowLatencyHlsServer {
        address: std::net::SocketAddr,
        worker: thread::JoinHandle<()>,
    }

    struct TestHlsAssets {
        master_playlist: &'static [u8],
        init: Vec<u8>,
        segment: Vec<u8>,
        media_playlist: &'static [u8],
        key: Option<[u8; 16]>,
        alternate_audio: Option<TestHlsAlternateAudioAssets>,
        alternate_subtitles: bool,
    }

    struct TestHlsAlternateAudioAssets {
        init: Vec<u8>,
        segment: Vec<u8>,
    }

    impl TestLowLatencyHlsServer {
        fn start(init: Vec<u8>, parts: Vec<Vec<u8>>) -> Self {
            assert_eq!(parts.len(), 3);
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test server must bind");
            let address = listener.local_addr().expect("test address must exist");
            let worker = thread::spawn(move || {
                let mut media_requests = 0_usize;
                for _ in 0..7 {
                    let (mut socket, _) = listener.accept().expect("test request must arrive");
                    let mut request = [0_u8; 4_096];
                    let read = socket.read(&mut request).expect("test request must read");
                    let request =
                        std::str::from_utf8(&request[..read]).expect("test request must be UTF-8");
                    let path = request
                        .split_ascii_whitespace()
                        .nth(1)
                        .expect("test request path must exist");
                    let body = if path == "/master.m3u8" {
                        include_bytes!("../tests/assets/hls_session_low_latency_master.m3u8")
                            .as_slice()
                    } else if path.starts_with("/media.m3u8") {
                        let body = if media_requests == 0 {
                            include_bytes!("../tests/assets/hls_session_low_latency_initial.m3u8")
                                .as_slice()
                        } else {
                            assert!(path.contains("_HLS_msn=101"));
                            assert!(path.contains("_HLS_part=2"));
                            assert!(path.contains("_HLS_skip=YES"));
                            include_bytes!("../tests/assets/hls_session_low_latency_delta.m3u8")
                                .as_slice()
                        };
                        media_requests += 1;
                        body
                    } else if path == "/init.mp4" {
                        init.as_slice()
                    } else if path == "/part-101-0.m4s" {
                        parts[0].as_slice()
                    } else if path == "/part-101-1.m4s" {
                        parts[1].as_slice()
                    } else if path == "/part-102-0.m4s" {
                        parts[2].as_slice()
                    } else {
                        panic!("unexpected LL-HLS test request path {path}")
                    };
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
                assert_eq!(media_requests, 2);
            });
            Self { address, worker }
        }

        fn finish(self) {
            self.worker.join().expect("test server must finish");
        }
    }

    impl TestHlsServer {
        fn start(init: Vec<u8>, segment: Vec<u8>) -> Self {
            Self::start_with_assets(TestHlsAssets {
                master_playlist: include_bytes!("../tests/assets/hls_session_master.m3u8"),
                init,
                segment,
                media_playlist: include_bytes!("../tests/assets/hls_session_media.m3u8"),
                key: None,
                alternate_audio: None,
                alternate_subtitles: false,
            })
        }

        fn start_encrypted(init: Vec<u8>, segment: Vec<u8>, key: [u8; 16]) -> Self {
            Self::start_with_assets(TestHlsAssets {
                master_playlist: include_bytes!("../tests/assets/hls_session_master.m3u8"),
                init,
                segment,
                media_playlist: include_bytes!("../tests/assets/hls_session_encrypted_media.m3u8"),
                key: Some(key),
                alternate_audio: None,
                alternate_subtitles: false,
            })
        }

        fn start_with_alternate_audio(
            init: Vec<u8>,
            segment: Vec<u8>,
            audio_init: Vec<u8>,
            audio_segment: Vec<u8>,
        ) -> Self {
            Self::start_with_assets(TestHlsAssets {
                master_playlist: include_bytes!(
                    "../tests/assets/hls_session_alternate_master.m3u8"
                ),
                init,
                segment,
                media_playlist: include_bytes!("../tests/assets/hls_session_media.m3u8"),
                key: None,
                alternate_audio: Some(TestHlsAlternateAudioAssets {
                    init: audio_init,
                    segment: audio_segment,
                }),
                alternate_subtitles: false,
            })
        }

        fn start_with_subtitles(init: Vec<u8>, segment: Vec<u8>) -> Self {
            Self::start_with_assets(TestHlsAssets {
                master_playlist: include_bytes!("../tests/assets/hls_session_subtitle_master.m3u8"),
                init,
                segment,
                media_playlist: include_bytes!("../tests/assets/hls_session_media.m3u8"),
                key: None,
                alternate_audio: None,
                alternate_subtitles: true,
            })
        }

        fn start_with_assets(assets: TestHlsAssets) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test server must bind");
            let address = listener.local_addr().expect("test address must exist");
            let worker = thread::spawn(move || {
                let request_count = 4
                    + usize::from(assets.key.is_some())
                    + 3 * usize::from(assets.alternate_audio.is_some())
                    + 2 * usize::from(assets.alternate_subtitles);
                for _ in 0..request_count {
                    let (mut socket, _) = listener.accept().expect("test request must arrive");
                    let mut request = [0_u8; 4_096];
                    let read = socket.read(&mut request).expect("test request must read");
                    let request =
                        std::str::from_utf8(&request[..read]).expect("test request must be UTF-8");
                    let path = request
                        .split_ascii_whitespace()
                        .nth(1)
                        .expect("test request path must exist");
                    let body = if path.ends_with("/master.m3u8") {
                        assets.master_playlist
                    } else if path.ends_with("/media.m3u8") {
                        assets.media_playlist
                    } else if path.ends_with("/audio.m3u8") {
                        include_bytes!("../tests/assets/hls_session_audio.m3u8").as_slice()
                    } else if path.ends_with("/subtitles.m3u8") {
                        include_bytes!("../tests/assets/hls_session_subtitles.m3u8").as_slice()
                    } else if path.ends_with("/key.bin") {
                        assets
                            .key
                            .as_ref()
                            .expect("only encrypted tests request an HLS key")
                            .as_slice()
                    } else if path.ends_with("/init.mp4") {
                        assets.init.as_slice()
                    } else if path.ends_with("/audio-init.mp4") {
                        assets
                            .alternate_audio
                            .as_ref()
                            .expect("only alternate-audio tests request its initialization")
                            .init
                            .as_slice()
                    } else if path.ends_with("/segment.m4s") {
                        assets.segment.as_slice()
                    } else if path.ends_with("/audio-segment.m4s") {
                        assets
                            .alternate_audio
                            .as_ref()
                            .expect("only alternate-audio tests request its segment")
                            .segment
                            .as_slice()
                    } else if path.ends_with("/subtitles-50.vtt") {
                        include_bytes!("../tests/assets/hls_session_subtitles.vtt").as_slice()
                    } else {
                        panic!("unexpected test request path {path}")
                    };
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
            });
            Self { address, worker }
        }

        fn finish(self) {
            self.worker.join().expect("test server must finish");
        }
    }
}
