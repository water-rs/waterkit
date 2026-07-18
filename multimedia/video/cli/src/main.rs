//! `WaterKit` video inspection and decode-throughput command-line tool.

use std::{
    io::{self, Write as _},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::Instant,
};

use clap::{Parser, Subcommand, ValueEnum};
use num_traits::ToPrimitive as _;
use serde::Serialize;
use waterkit_codec::{CodecType, DecodedFrameUploader, DecodedPixelLayout};
use waterkit_video_container::VideoReader;
use waterkit_video_player::VideoPlayer;
use waterkit_video_streaming::{AssetCache, ProgressiveDownloadRequest, Url, download};

const DOWNLOAD_PROGRESS_QUANTUM: usize = 1024 * 1024;

#[derive(Debug, Parser)]
#[command(name = "waterkit-video")]
#[command(about = "Inspect media and benchmark the WaterKit decode pipeline")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect MP4/MOV video-track metadata without decoding frames.
    Probe { source: String },
    /// Decode a media source as fast as possible and report measured throughput.
    BenchmarkDecode {
        source: String,
        #[arg(long)]
        max_frames: Option<u64>,
        /// Include upload of every decoded YUV frame into reusable GPU textures.
        #[arg(long)]
        gpu_upload: bool,
        #[arg(long)]
        require_min_fps: Option<f64>,
        #[arg(long)]
        require_width: Option<u32>,
        #[arg(long)]
        require_height: Option<u32>,
        #[arg(long)]
        require_codec: Option<RequiredCodec>,
        #[arg(long)]
        require_layout: Option<RequiredPixelLayout>,
        #[arg(long)]
        require_hdr: bool,
        #[arg(long)]
        max_first_frame_ms: Option<f64>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RequiredCodec {
    H264,
    H265,
    Av1,
}

impl RequiredCodec {
    const fn matches(self, actual: CodecType) -> bool {
        matches!(
            (self, actual),
            (Self::H264, CodecType::H264)
                | (Self::H265, CodecType::H265)
                | (Self::Av1, CodecType::Av1)
        )
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RequiredPixelLayout {
    Nv12,
    P010,
}

impl RequiredPixelLayout {
    const fn matches(self, actual: DecodedPixelLayout) -> bool {
        matches!(
            (self, actual),
            (Self::Nv12, DecodedPixelLayout::Nv12) | (Self::P010, DecodedPixelLayout::P010)
        )
    }
}

#[derive(Debug)]
struct ResolvedSource {
    path: PathBuf,
    downloaded_bytes: usize,
    download_seconds: f64,
}

#[derive(Debug, Serialize)]
struct ProbeReport {
    path: PathBuf,
    width: u32,
    height: u32,
    sample_count: u32,
    timescale: u32,
    has_audio: bool,
    downloaded_bytes: usize,
    download_seconds: f64,
}

#[derive(Debug, Serialize)]
struct DecodeBenchmarkReport {
    path: PathBuf,
    codec: String,
    pixel_layout: String,
    width: u32,
    height: u32,
    source_is_hdr: bool,
    color_matrix: String,
    color_primaries: String,
    transfer_function: String,
    color_range: String,
    max_content_light_nits: Option<u16>,
    decoded_frames: u64,
    presentation_time_regressions: u64,
    first_frame_milliseconds: f64,
    elapsed_seconds: f64,
    decoded_frames_per_second: f64,
    media_duration_seconds: f64,
    gpu_upload: bool,
    downloaded_bytes: usize,
    download_seconds: f64,
}

#[derive(Debug)]
struct BenchmarkRequirements {
    minimum_fps: Option<f64>,
    width: Option<u32>,
    height: Option<u32>,
    codec: Option<RequiredCodec>,
    layout: Option<RequiredPixelLayout>,
    hdr: bool,
    maximum_first_frame_ms: Option<f64>,
}

struct GpuUploadBenchmark {
    device: wgpu::Device,
    queue: wgpu::Queue,
    uploader: DecodedFrameUploader,
}

impl std::fmt::Debug for GpuUploadBenchmark {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GpuUploadBenchmark")
            .finish_non_exhaustive()
    }
}

impl GpuUploadBenchmark {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))?;
        let required_features = adapter.features() & wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("WaterKit video benchmark device"),
                required_features,
                ..Default::default()
            }))?;
        Ok(Self {
            device,
            queue,
            uploader: DecodedFrameUploader::new(),
        })
    }

    fn upload(
        &mut self,
        frame: waterkit_codec::DecodedFrame,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if frame.pixel_layout() == DecodedPixelLayout::P010
            && !self
                .device
                .features()
                .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM)
        {
            return Err(io::Error::other(
                "P010 GPU upload requires TEXTURE_FORMAT_16BIT_NORM on the selected adapter",
            )
            .into());
        }
        let uploaded = self.uploader.upload(frame, &self.device, &self.queue);
        assert_eq!(
            uploaded.pixel_layout(),
            if uploaded.y_texture().format() == wgpu::TextureFormat::R16Unorm {
                DecodedPixelLayout::P010
            } else {
                DecodedPixelLayout::Nv12
            },
            "uploaded texture format must preserve the decoded pixel layout"
        );
        Ok(())
    }

    fn finish(self) -> Result<(), Box<dyn std::error::Error>> {
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = match Arguments::parse().command {
        Command::Probe { source } => {
            let resolved = resolve_source(&source)?;
            let reader = VideoReader::open(&resolved.path)?;
            let (width, height) = reader.dimensions();
            serde_json::to_value(ProbeReport {
                path: resolved.path,
                width,
                height,
                sample_count: reader.sample_count(),
                timescale: reader.timescale(),
                has_audio: reader.has_audio(),
                downloaded_bytes: resolved.downloaded_bytes,
                download_seconds: resolved.download_seconds,
            })?
        }
        Command::BenchmarkDecode {
            source,
            max_frames,
            gpu_upload,
            require_min_fps,
            require_width,
            require_height,
            require_codec,
            require_layout,
            require_hdr,
            max_first_frame_ms,
        } => {
            let requirements = BenchmarkRequirements {
                minimum_fps: require_min_fps,
                width: require_width,
                height: require_height,
                codec: require_codec,
                layout: require_layout,
                hdr: require_hdr,
                maximum_first_frame_ms: max_first_frame_ms,
            };
            serde_json::to_value(benchmark_decode(
                &source,
                max_frames,
                gpu_upload,
                &requirements,
            )?)?
        }
    };

    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer_pretty(&mut output, &report)?;
    output.write_all(b"\n")?;
    Ok(())
}

fn resolve_source(source: &str) -> Result<ResolvedSource, Box<dyn std::error::Error>> {
    let local_path = Path::new(source);
    if local_path.exists() {
        return Ok(ResolvedSource {
            path: local_path.to_owned(),
            downloaded_bytes: 0,
            download_seconds: 0.0,
        });
    }

    let url = Url::parse(source)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(io::Error::other(format!(
            "unsupported video source scheme '{}'; expected a local path, HTTP, or HTTPS",
            url.scheme()
        ))
        .into());
    }
    let cache_root = dirs::cache_dir()
        .ok_or_else(|| io::Error::other("video benchmark requires a platform cache directory"))?
        .join("waterkit")
        .join("video-benchmark");
    let destination = AssetCache::new(cache_root).path_for(&url, "mp4");
    if destination.exists() {
        return Ok(ResolvedSource {
            path: destination,
            downloaded_bytes: 0,
            download_seconds: 0.0,
        });
    }

    let started = Instant::now();
    let request = ProgressiveDownloadRequest::new_cached(
        url,
        destination,
        NonZeroUsize::new(DOWNLOAD_PROGRESS_QUANTUM)
            .expect("download progress quantum must be non-zero"),
    )?;
    let receipt = pollster::block_on(download(request, |_| {}))?;
    Ok(ResolvedSource {
        path: receipt.destination().to_owned(),
        downloaded_bytes: receipt.bytes_written(),
        download_seconds: started.elapsed().as_secs_f64(),
    })
}

fn benchmark_decode(
    source: &str,
    max_frames: Option<u64>,
    gpu_upload: bool,
    requirements: &BenchmarkRequirements,
) -> Result<DecodeBenchmarkReport, Box<dyn std::error::Error>> {
    validate_requirement_values(requirements)?;
    let resolved = resolve_source(source)?;
    let mut player = VideoPlayer::open(&resolved.path)?;
    let (width, height) = player.dimensions();
    let codec = player.codec_type();
    let color = player.color_info();
    let duration = player.duration();
    let mut gpu = gpu_upload.then(GpuUploadBenchmark::new).transpose()?;
    let started = Instant::now();
    let mut first_frame_milliseconds = None;
    let mut decoded_frames = 0u64;
    let mut presentation_time_regressions = 0u64;
    let mut previous_presentation_time = None;
    let mut pixel_layout = None;

    while let Some(frame) = player.next_frame()? {
        first_frame_milliseconds.get_or_insert_with(|| started.elapsed().as_secs_f64() * 1000.0);
        let layout = frame.frame().pixel_layout();
        if let Some(existing) = pixel_layout {
            assert_eq!(
                existing, layout,
                "decoded pixel layout changed within one video stream"
            );
        } else {
            pixel_layout = Some(layout);
        }
        let presentation_time = frame.timing().presentation_time();
        if previous_presentation_time.is_some_and(|previous| presentation_time < previous) {
            presentation_time_regressions = presentation_time_regressions.saturating_add(1);
        }
        previous_presentation_time = Some(presentation_time);
        if let Some(gpu) = gpu.as_mut() {
            gpu.upload(frame.into_frame())?;
        }
        decoded_frames = decoded_frames.saturating_add(1);
        if max_frames.is_some_and(|limit| decoded_frames >= limit) {
            break;
        }
    }
    if let Some(gpu) = gpu {
        gpu.finish()?;
    }
    let elapsed_seconds = started.elapsed().as_secs_f64();
    let first_frame_milliseconds = first_frame_milliseconds
        .ok_or_else(|| io::Error::other("video benchmark decoded no frames"))?;
    let pixel_layout = pixel_layout.expect("a decoded frame must establish its pixel layout");
    let decoded_frames_per_second = decoded_frames
        .to_f64()
        .expect("u64 frame count must convert to a finite f64")
        / elapsed_seconds;
    let report = DecodeBenchmarkReport {
        path: resolved.path,
        codec: format!("{codec:?}"),
        pixel_layout: format!("{pixel_layout:?}"),
        width,
        height,
        source_is_hdr: color.is_hdr(),
        color_matrix: format!("{:?}", color.matrix),
        color_primaries: format!("{:?}", color.primaries),
        transfer_function: format!("{:?}", color.transfer),
        color_range: format!("{:?}", color.range),
        max_content_light_nits: color
            .content_light_level
            .map(waterkit_video_core::ContentLightLevel::max_content_light_level),
        decoded_frames,
        presentation_time_regressions,
        first_frame_milliseconds,
        elapsed_seconds,
        decoded_frames_per_second,
        media_duration_seconds: duration.as_secs_f64(),
        gpu_upload,
        downloaded_bytes: resolved.downloaded_bytes,
        download_seconds: resolved.download_seconds,
    };
    validate_report(&report, codec, pixel_layout, requirements)?;
    Ok(report)
}

fn validate_requirement_values(
    requirements: &BenchmarkRequirements,
) -> Result<(), Box<dyn std::error::Error>> {
    if requirements
        .minimum_fps
        .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        return Err(io::Error::other("required minimum FPS must be finite and positive").into());
    }
    if requirements
        .maximum_first_frame_ms
        .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        return Err(
            io::Error::other("maximum first-frame time must be finite and positive").into(),
        );
    }
    Ok(())
}

fn validate_report(
    report: &DecodeBenchmarkReport,
    codec: CodecType,
    layout: DecodedPixelLayout,
    requirements: &BenchmarkRequirements,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut failures = Vec::new();
    if requirements
        .minimum_fps
        .is_some_and(|minimum| report.decoded_frames_per_second < minimum)
    {
        failures.push(format!(
            "decoded throughput {:.2} fps is below required {:.2} fps",
            report.decoded_frames_per_second,
            requirements.minimum_fps.expect("minimum FPS was checked")
        ));
    }
    if requirements
        .width
        .is_some_and(|required| report.width != required)
    {
        failures.push(format!(
            "decoded width {} does not match required {}",
            report.width,
            requirements.width.expect("required width was checked")
        ));
    }
    if requirements
        .height
        .is_some_and(|required| report.height != required)
    {
        failures.push(format!(
            "decoded height {} does not match required {}",
            report.height,
            requirements.height.expect("required height was checked")
        ));
    }
    if requirements
        .codec
        .is_some_and(|required| !required.matches(codec))
    {
        failures.push(format!(
            "decoded codec {codec:?} does not match required {:?}",
            requirements.codec.expect("required codec was checked")
        ));
    }
    if requirements
        .layout
        .is_some_and(|required| !required.matches(layout))
    {
        failures.push(format!(
            "decoded layout {layout:?} does not match required {:?}",
            requirements.layout.expect("required layout was checked")
        ));
    }
    if requirements.hdr && !report.source_is_hdr {
        failures.push(String::from("decoded source is not HDR"));
    }
    if requirements
        .maximum_first_frame_ms
        .is_some_and(|maximum| report.first_frame_milliseconds > maximum)
    {
        failures.push(format!(
            "first frame {:.2} ms exceeds required maximum {:.2} ms",
            report.first_frame_milliseconds,
            requirements
                .maximum_first_frame_ms
                .expect("maximum first-frame time was checked")
        ));
    }
    if report.presentation_time_regressions != 0 {
        failures.push(format!(
            "decoded presentation timeline regressed {} times",
            report.presentation_time_regressions
        ));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")).into())
    }
}
