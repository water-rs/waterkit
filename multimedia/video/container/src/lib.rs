//! Media-container support for `WaterKit` video.
//!
//! Container parsing remains independent from codecs, networking, graphics,
//! audio output, and UI so media tooling can reuse it without linking a player.

#![warn(missing_docs)]

mod color;
mod demuxer;
mod isobmff;
mod muxer;
mod progressive;
mod stream;
mod subtitles;

pub use color::{probe_mp4_color_info, probe_mp4_color_info_bytes};
pub use demuxer::{
    EmbeddedSubtitleCodec, EmbeddedSubtitleCue, EmbeddedSubtitleTrack, VideoCodec, VideoReader,
    embedded_subtitle_tracks, read_embedded_subtitle_cues,
};
pub use muxer::{CodecType as MuxerCodecType, VideoFormat, VideoWriter};
pub use progressive::{ProgressiveTrack, ProgressiveTrackReader};
pub use stream::{
    AudioLayout, CmafChunkDemuxer, CmafDemuxer, CmafInitialization, CmafMediaSegment, Codec,
    EncodedSample, MediaTime, MpegTsDemuxer, MpegTsEvent, TimedMetadata, TrackId, TrackInfo,
    TrackKind, VideoDimensions, decode_cmaf_subtitle_sample, parse_pssh_init_data,
};
pub use subtitles::{
    SubtitleCue, active_subtitle_text, parse_hls_webvtt_segment, parse_subrip_document,
    parse_subtitles_from_path, parse_ttml_document, parse_webvtt_document,
};
pub use waterkit_video_core::{
    CommonEncryptionScheme, EncryptionSubsample, Error, ProtectionInitData, SampleEncryption,
    TrackProtection,
};
