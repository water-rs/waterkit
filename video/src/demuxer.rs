//! Video demuxer and frame representation.

use crate::VideoError;
use mp4::WriteBox;
use std::io::{Cursor, Read};
use std::path::Path;

/// A decoded video frame.
#[derive(Clone)]
pub struct VideoFrame {
    /// Raw pixel data (BGRA format).
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Presentation timestamp in milliseconds.
    pub pts_ms: u64,
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
            .field("pts_ms", &self.pts_ms)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// Video reader for MP4/MOV files.
#[derive(Debug)]
pub struct VideoReader {
    width: u32,
    height: u32,
    samples: Vec<(Vec<u8>, u64, bool)>, // (data, pts, is_keyframe)
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
        let reader = mp4::Mp4Reader::read_header(std::io::BufReader::new(file), size)
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

        // Read all samples
        let mut samples = Vec::new();
        let mut reader = reader;
        for i in 1..=sample_count {
            if let Ok(Some(sample)) = reader.read_sample(video_track_id, i) {
                samples.push((sample.bytes.to_vec(), sample.start_time, sample.is_sync));
            }
        }

        Ok(Self {
            width,
            height,
            samples,
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
        self.samples.len() as u32
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
        self.samples
            .get(index)
            .map(|(_, pts, is_keyframe)| (*pts, *is_keyframe))
    }

    /// Return estimated stream duration from the last sample PTS.
    #[must_use]
    pub fn duration(&self) -> Option<std::time::Duration> {
        let (last_pts, _) = self
            .samples
            .last()
            .map(|(_, pts, is_keyframe)| (*pts, *is_keyframe))?;
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
        if self.samples.is_empty() {
            return 0;
        }
        let clamped = index.min(self.samples.len().saturating_sub(1));
        for candidate in (0..=clamped).rev() {
            if self.samples[candidate].2 {
                return candidate;
            }
        }
        0
    }

    /// Seek the internal cursor to a sample index.
    pub fn seek_to_sample(&mut self, index: usize) {
        self.current_index = index.min(self.samples.len());
    }

    /// Read the next video sample (encoded data).
    /// Returns (data, `pts_ms`, `is_keyframe`) or None if at end.
    pub fn read_sample(&mut self) -> Option<(Vec<u8>, u64, bool)> {
        if self.current_index >= self.samples.len() {
            return None;
        }

        let sample = self.samples[self.current_index].clone();
        self.current_index += 1;
        Some(sample)
    }

    /// Read the next sample by reference without cloning sample bytes.
    ///
    /// Returns `(sample_data, pts, is_keyframe)` or `None` when at EOF.
    pub fn read_sample_ref(&mut self) -> Option<(&[u8], u64, bool)> {
        let index = self.current_index;
        let (sample_data, pts, is_keyframe) = self.samples.get(index)?;
        self.current_index += 1;
        Some((sample_data.as_slice(), *pts, *is_keyframe))
    }

    /// Iterate over samples from the current position.
    pub fn samples(&mut self) -> impl Iterator<Item = (Vec<u8>, u64, bool)> + '_ {
        std::iter::from_fn(move || self.read_sample())
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

#[cfg(test)]
mod tests {
    use super::extract_box_from_bytes;

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
}
