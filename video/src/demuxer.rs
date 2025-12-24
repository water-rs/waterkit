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
    /// Open a video file for reading.
    ///
    /// # Errors
    /// Returns [`VideoError::Io`] if the file cannot be opened.
    #[allow(clippy::cast_possible_truncation)]
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, VideoError> {
        let file = std::fs::File::open(path.as_ref())?;
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

                // Check for HEVC (hev1) - mp4 crate's HvcCBox is broken (discards all data)
                // We must read raw hvcC bytes directly from the file
                if stsd.hev1.is_some() {
                    // Read raw hvcC by scanning file for the atom
                    let mut file = std::fs::File::open(&path)?;
                    let mut buf = vec![0u8; file.metadata()?.len() as usize];
                    file.read_exact(&mut buf)?;

                    // Find hvcC box in file (search for 'hvcC' signature)
                    if let Some(pos) = buf.windows(4).position(|w| w == b"hvcC") {
                        // hvcC box starts 4 bytes before (that's the size field)
                        if pos >= 4 {
                            let size_pos = pos - 4;
                            let box_size = u32::from_be_bytes([
                                buf[size_pos],
                                buf[size_pos + 1],
                                buf[size_pos + 2],
                                buf[size_pos + 3],
                            ]) as usize;
                            if size_pos + box_size <= buf.len() && box_size > 8 {
                                // Extract the full box (including header) for decoder compatibility
                                codec_config = Some(buf[size_pos..size_pos + box_size].to_vec());
                            }
                        }
                    }
                }
                // Check for AVC (avc1)
                else if let Some(avc1) = &stsd.avc1 {
                    let avcc = &avc1.avcc;
                    let mut buf = Vec::new();
                    let mut cursor = Cursor::new(&mut buf);
                    if avcc.write_box(&mut cursor).is_ok() {
                        codec_config = Some(buf);
                    }
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
