//! Modular, UI-independent video framework for Rust.
//!
//! `waterkit-video` is an umbrella: applications enable only the reusable
//! layers they need. Containers, Zenwave networking, frame processing, and the
//! playback engine remain separate crates so tools and libraries do not inherit
//! an unrelated graphics or UI dependency graph.

#![warn(missing_docs)]

pub use waterkit_video_core as core;
pub use waterkit_video_core::{
    ColorPrimaries, ColorRange, CommonEncryptionScheme, ContentLightLevel, EncryptionSubsample,
    Error as VideoError, FrameRate, FrameSize, FrameTiming, MatrixCoefficients, ProtectionInitData,
    SampleEncryption, TrackProtection, TransferFunction, VideoColorInfo,
};

#[cfg(feature = "container")]
pub use waterkit_video_container as container;
#[cfg(feature = "container")]
pub use waterkit_video_container::{
    EmbeddedSubtitleCodec, EmbeddedSubtitleCue, EmbeddedSubtitleTrack, MuxerCodecType, SubtitleCue,
    TimedMetadata, VideoFormat, VideoReader, VideoWriter, active_subtitle_text,
    decode_cmaf_subtitle_sample, embedded_subtitle_tracks, parse_hls_webvtt_segment,
    parse_subrip_document, parse_subtitles_from_path, parse_ttml_document, parse_webvtt_document,
    probe_mp4_color_info, read_embedded_subtitle_cues,
};

#[cfg(feature = "streaming")]
pub use waterkit_video_streaming as streaming;

#[cfg(feature = "processing")]
pub use waterkit_video_processing as processing;

#[cfg(feature = "player")]
pub use waterkit_video_player as player;
#[cfg(feature = "player")]
pub use waterkit_video_player::{
    AudioTrackDecoder, DecodedAudioFrame, DecodedVideoFrame, LivePlaybackRateRange, LiveWindow,
    PictureInPictureCommand, PictureInPictureCommandStream, PictureInPictureController,
    PictureInPictureControllerState, PictureInPictureHostId, SelectableAudioTrack,
    SelectableSubtitleTrack, SelectableVideoTrack, VideoPlayer, VideoTrackDecoder,
    detect_codec_type,
};

#[cfg(all(target_os = "android", feature = "streaming"))]
pub use waterkit_video_player::{
    AndroidAudioDecoderTarget, AndroidAudioDrmBootstrap, AndroidDrmBootstrap, AndroidDrmContext,
    AndroidKeyDuration, AndroidKeyRequestType, AndroidKeyStatus, AndroidLicenseChallenge,
    AndroidOfflineKeySet, AndroidOfflineLicenseAcquisition,
    AndroidOfflineLicenseAcquisitionBootstrap, AndroidOfflineLicenseBootstrap,
    AndroidOfflineLicenseProvisioning, AndroidOfflineLicenseRelease,
    AndroidOfflineLicenseReleaseBootstrap, AndroidOfflineLicenseRenewal,
    AndroidOfflineLicenseRenewalBootstrap, AndroidPendingAudioDecoder, AndroidPendingDecoder,
    AndroidPendingOfflineLicense, AndroidPendingVideoDecoder, AndroidProtectedAudioDecoder,
    AndroidProtectedSurface, AndroidProtectedVideoDecoder, AndroidProtectedVideoOutput,
    AndroidProvisionRequest, AndroidProvisioningAudioDecoder, AndroidProvisioningVideoDecoder,
    AndroidReadyAudioDecoder, AndroidReadyDecoder, AndroidReadyVideoDecoder,
    AndroidVideoDecoderTarget,
};

#[cfg(all(target_os = "android", feature = "player"))]
pub use waterkit_video_player::{
    AndroidOffloadAudioController, AndroidOffloadAudioPlayback, AndroidPlaybackContext,
    AndroidTunneledPlayback, AndroidVideoSurface,
};

#[cfg(all(feature = "player", feature = "streaming"))]
pub use waterkit_video_player::{
    AnyLicenseServer, AudioTrackSelection, DashPlaybackSession, DashSegmentPoll,
    DashStreamedSegment, DashStreamedSubtitleSegment, HlsPlaybackSession, HlsSegmentPoll,
    LicenseRequest, LicenseResponse, LicenseServer, SegmentedPlaybackOptions, StreamedSegment,
    StreamedSubtitleSegment, SubtitleTrackSelection, VideoTrackSelection, ZenwaveLicenseServer,
    acquire_license,
};
