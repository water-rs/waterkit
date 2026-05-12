//! Video demuxer and frame representation.

use crate::VideoError;
use mp4::WriteBox;
use std::{
    io::{BufReader, Cursor, Read},
    path::Path,
    time::Duration,
};

#[derive(Debug, Clone, Copy)]
struct SampleMeta {
    start_time: u64,
    is_keyframe: bool,
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

/// A decoded video frame.
#[derive(Clone)]
pub struct VideoFrame {
    /// Raw pixel data (BGRA format).
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Presentation timestamp.
    pub pts: std::time::Duration,
}

impl VideoFrame {
    /// Write frame data to a wgpu texture.
    pub fn write_to_texture(&self, queue: &wgpu::Queue, texture: &wgpu::Texture) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * 4),
                rows_per_image: Some(self.height),
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Create a wgpu texture suitable for this frame.
    #[must_use]
    pub fn create_texture(&self, device: &wgpu::Device) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("VideoFrame"),
            size: wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pts", &self.pts)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// Video reader for MP4/MOV files.
#[derive(Debug)]
pub struct VideoReader {
    reader: mp4::Mp4Reader<BufReader<std::fs::File>>,
    video_track_id: u32,
    width: u32,
    height: u32,
    sample_metas: Vec<SampleMeta>,
    codec_config: Option<Vec<u8>>,
    current_index: usize,
    timescale: u32,
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
        let mut sample_count = 0u32;
        let mut codec_config: Option<Vec<u8>> = None;
        let mut timescale = 0u32;

        for track in reader.tracks().values() {
            let track_type = track
                .track_type()
                .map_err(|e| VideoError::Container(e.to_string()))?;
            if track_type == mp4::TrackType::Video {
                video_track_id = track.track_id();
                width = u32::from(track.width());
                height = u32::from(track.height());
                sample_count = track.sample_count();
                timescale = track.timescale();

                let stsd = &track.trak.mdia.minf.stbl.stsd;

                if let Some(avc1) = &stsd.avc1 {
                    let avcc = &avc1.avcc;
                    let mut buf = Vec::new();
                    let mut cursor = Cursor::new(&mut buf);
                    if avcc.write_box(&mut cursor).is_ok() {
                        codec_config = Some(buf);
                    }
                } else {
                    // For HEVC (hvc1/hev1), mp4 crate cannot reliably expose raw hvcC bytes.
                    // Extract hvcC atom directly from file for decoder initialization.
                    codec_config = extract_box_from_file(path_ref, *b"hvcC")?;
                }
                break;
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
        let sample_metas = build_sample_metas(track, sample_count)?;

        Ok(Self {
            reader,
            video_track_id,
            width,
            height,
            sample_metas,
            codec_config,
            current_index: 0,
            timescale,
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
    /// Returns `(pts, is_keyframe)` when the sample exists.
    #[must_use]
    pub fn sample_info(&self, index: usize) -> Option<(u64, bool)> {
        self.sample_metas
            .get(index)
            .map(|meta| (meta.start_time, meta.is_keyframe))
    }

    /// Return estimated stream duration from the last sample PTS.
    #[must_use]
    pub fn duration(&self) -> Option<std::time::Duration> {
        let (last_pts, _) = self
            .sample_metas
            .last()
            .map(|meta| (meta.start_time, meta.is_keyframe))?;
        if self.timescale == 0 {
            return Some(std::time::Duration::ZERO);
        }
        Some(std::time::Duration::from_nanos(
            last_pts.saturating_mul(1_000_000_000) / u64::from(self.timescale),
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
        let sample_id = u32::try_from(sample_index + 1).map_err(|_| {
            VideoError::Container("sample index exceeds mp4 reader range".to_string())
        })?;
        let sample = self
            .reader
            .read_sample(self.video_track_id, sample_id)
            .map_err(|error| VideoError::Container(error.to_string()))?;
        let meta = self.sample_metas[sample_index];
        Ok(sample.map(|sample| (sample.bytes.to_vec(), meta.start_time, meta.is_keyframe)))
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

    /// Get codec configuration (avcC or hvcC raw data).
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

fn build_sample_metas(
    track: &mp4::Mp4Track,
    sample_count: u32,
) -> Result<Vec<SampleMeta>, VideoError> {
    let mut metas = Vec::with_capacity(sample_count as usize);
    for sample_id in 1..=sample_count {
        metas.push(SampleMeta {
            start_time: sample_start_time(track, sample_id)?,
            is_keyframe: is_sync_sample(track, sample_id),
        });
    }
    Ok(metas)
}

fn sample_start_time(track: &mp4::Mp4Track, sample_id: u32) -> Result<u64, VideoError> {
    if !track.trafs.is_empty() {
        let offset = u64::from(sample_id.saturating_sub(1));
        return Ok(offset.saturating_mul(u64::from(track.default_sample_duration)));
    }

    let stts = &track.trak.mdia.minf.stbl.stts;
    let mut first_sample = 1_u32;
    let mut elapsed = 0_u64;
    for entry in &stts.entries {
        let next_first = first_sample
            .checked_add(entry.sample_count)
            .ok_or_else(|| {
                VideoError::Container(
                    "stts sample_count overflow while building video timeline".into(),
                )
            })?;
        if sample_id < next_first {
            let sample_offset = u64::from(sample_id.saturating_sub(first_sample));
            let delta = u64::from(entry.sample_delta);
            return Ok(elapsed.saturating_add(sample_offset.saturating_mul(delta)));
        }
        first_sample = next_first;
        elapsed =
            elapsed.saturating_add(u64::from(entry.sample_count) * u64::from(entry.sample_delta));
    }

    Err(VideoError::Container(format!(
        "stts entry missing for video sample {sample_id}"
    )))
}

fn is_sync_sample(track: &mp4::Mp4Track, sample_id: u32) -> bool {
    if !track.trafs.is_empty() {
        let traf_count = u32::try_from(track.trafs.len()).unwrap_or(0);
        if traf_count == 0 {
            return true;
        }
        let sample_sizes_count = track.sample_count() / traf_count;
        if sample_sizes_count == 0 {
            return sample_id == 1;
        }
        return sample_id == 1 || sample_id.is_multiple_of(sample_sizes_count);
    }

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
    let mut file = std::fs::File::open(path)?;
    let file_len = usize::try_from(file.metadata()?.len()).map_err(|_| {
        VideoError::Container("video file length exceeds current architecture limits".to_string())
    })?;
    let mut bytes = vec![0_u8; file_len];
    file.read_exact(&mut bytes)?;
    Ok(extract_box_from_bytes(&bytes, box_type))
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
        .chunks_exact(2)
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
}
