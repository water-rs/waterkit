//! Network delivery for `WaterKit` video, built exclusively on Zenwave.
//!
//! Protocol implementations live as modules in this crate rather than one
//! crate per protocol. The crate owns transport, cache identity, redirects,
//! progress reporting, and protocol policy while leaving presentation to a
//! player or application.

#![warn(missing_docs)]

mod abr;
mod atomic;
mod cache;
mod dash;
mod fetch;
mod hls;
mod license;
#[cfg(feature = "offline")]
mod offline;
mod progressive;
mod segment;
mod transport;

pub use abr::{
    AdaptiveSelectionPolicy, AdaptiveTrackSelector, AdaptiveVariant, BandwidthEstimator,
    StreamVariant,
};
pub use cache::AssetCache;
#[cfg(feature = "offline")]
pub use cache::{CacheCoverage, CacheResourceKey, CachedObject, PersistentMediaCache};
pub use dash::{
    DashAdaptationSet, DashAvailabilityTimeOffset, DashBaseUrl, DashContentProtection,
    DashInitialization, DashLatency, DashManifest, DashManifestKind, DashPeriod,
    DashPlannedSegment, DashPlaybackRate, DashPlaybackRateRange, DashProducerReferenceTime,
    DashRepresentation, DashSegmentBase, DashSegmentList, DashSegmentReference, DashSegmentSource,
    DashSegmentTemplate, DashServiceDescription, DashTimelineEntry, DashTrackKind, DashUtcTiming,
    fetch_dash_manifest, parse_dash_manifest,
};
pub use fetch::{
    MediaByteRange, MediaRequest, MediaResponse, MediaRevalidation, MediaStream,
    MediaStreamReceipt, MediaValidator, fetch_media, open_media_stream, revalidate_media,
};
pub use hls::{
    HlsDeltaUpdate, HlsEncryption, HlsEncryptionMethod, HlsInitializationSegment, HlsLowLatency,
    HlsMasterPlaylist, HlsMediaPlaylist, HlsPartialSegment, HlsPlaylist, HlsPreloadHint,
    HlsPreloadHintKind, HlsRendition, HlsRenditionKind, HlsRenditionReport, HlsSegment,
    HlsSegmentRange, HlsServerControl, fetch_hls_playlist, parse_hls_playlist,
};
pub use license::{
    AnyLicenseServer, LicenseRequest, LicenseResponse, LicenseServer, ZenwaveLicenseServer,
    acquire_license,
};
#[cfg(feature = "offline")]
pub use offline::{
    OfflineDownload, OfflineDownloadController, OfflineDownloadEvent, OfflineDownloadEvents,
    OfflineDownloadOutcome, OfflineDownloadProgress, OfflineManager, OfflinePackage,
    OfflinePackageId, OfflinePackagePlan, OfflineResource, OfflineResourceId,
};
pub use progressive::{
    DownloadEvent, DownloadProgress, DownloadReceipt, ProgressiveDownloadRequest, download,
};
pub use segment::{
    FetchedSegment, SegmentLoader, SegmentResource, SegmentStream, StreamedSegmentReceipt,
};
pub use url::Url;
pub use waterkit_video_core::Error;
