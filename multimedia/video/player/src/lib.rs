//! UI-independent video decoding, playback timing, and platform media services.

#![warn(missing_docs)]

#[cfg(target_os = "android")]
mod android_offload;
#[cfg(target_os = "android")]
mod android_surface;
#[cfg(target_os = "android")]
mod android_tunneling;
mod audio_track;
#[cfg(feature = "streaming")]
mod dash;
mod decode;
mod live;
mod picture_in_picture;
#[cfg(all(target_os = "android", feature = "streaming"))]
mod protected;
#[cfg(feature = "streaming")]
mod streaming;
mod subtitle_track;
mod video_track;

#[cfg(target_os = "android")]
pub use android_offload::{AndroidOffloadAudioController, AndroidOffloadAudioPlayback};
#[cfg(target_os = "android")]
pub use android_surface::{AndroidPlaybackContext, AndroidVideoSurface};
#[cfg(target_os = "android")]
pub use android_tunneling::AndroidTunneledPlayback;
pub use audio_track::SelectableAudioTrack;
#[cfg(feature = "streaming")]
pub use dash::{
    DashPlaybackSession, DashSegmentPoll, DashStreamedSegment, DashStreamedSubtitleSegment,
};
pub use decode::{
    AudioTrackDecoder, DecodedVideoFrame, VideoPlayer, VideoTrackDecoder, detect_codec_type,
};
pub use live::{LivePlaybackRateRange, LiveWindow};
pub use picture_in_picture::{
    PictureInPictureCommand, PictureInPictureCommandStream, PictureInPictureController,
    PictureInPictureControllerState, PictureInPictureHostId,
};
#[cfg(all(target_os = "android", feature = "streaming"))]
pub use protected::{
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
#[cfg(feature = "streaming")]
pub use streaming::{
    AudioTrackSelection, HlsPlaybackSession, HlsSegmentPoll, SegmentedPlaybackOptions,
    StreamedSegment, StreamedSubtitleSegment, SubtitleTrackSelection, VideoTrackSelection,
};
pub use subtitle_track::SelectableSubtitleTrack;
pub use video_track::SelectableVideoTrack;
pub use waterkit_audio::DecodedAudioFrame;
pub use waterkit_video_core::Error;
#[cfg(feature = "streaming")]
pub use waterkit_video_streaming::{
    AnyLicenseServer, LicenseRequest, LicenseResponse, LicenseServer, ZenwaveLicenseServer,
    acquire_license,
};
