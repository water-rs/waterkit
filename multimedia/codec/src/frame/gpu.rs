//! GPU texture output for decoded video frames.
//!
//! This module holds everything that needs a `wgpu` device: plane-texture
//! upload and the native-YUV to linear-RGBA compute conversion. It is compiled
//! only with the `gpu` feature, so a decode-only consumer links no `wgpu`.

use std::sync::Arc;
use waterkit_video_core::VideoColorInfo;
use wgpu::{
    BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, Buffer, BufferBindingType,
    BufferDescriptor, BufferUsages, ComputePipeline, ComputePipelineDescriptor, Device, Extent3d,
    PipelineLayoutDescriptor, Queue, ShaderStages, StorageTextureAccess, Texture,
    TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
    TextureViewDimension,
};

#[cfg(waterkit_any_codec)]
use super::DecodedFrameInner;
use super::{DecodedFrame, DecodedPixelLayout};
use crate::{ColorOutputTarget, video_color_uniform};
use shaderloom::{CompiledShader, ShaderStage};

#[cfg(waterkit_hw_codec_apple)]
mod apple;

const YUV_COLOR_SHADER: CompiledShader = include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/shaders/compiled/yuv_color.rs"
));

impl DecodedFrame {
    /// Convert to GPU frame by uploading to the user's device.
    ///
    /// This consumes the decoded frame and creates GPU textures on the provided device.
    #[must_use]
    pub fn to_gpu_frame(self, device: &Device, queue: &Queue) -> GpuFrame {
        DecodedFrameUploader::new().upload(self, device, queue)
    }
}

/// Reusable decoded-frame uploader.
///
/// Every call produces a fresh [`GpuFrame`]: hardware frames are imported in
/// place from their `IOSurface` planes and software frames are uploaded once
/// into their own textures, so a frame's planes stay valid for as long as a
/// consumer retains them.
#[derive(Debug)]
pub struct DecodedFrameUploader {
    #[cfg(waterkit_hw_codec_apple)]
    apple: Option<apple::AppleFrameUploader>,
    imported_frames: u64,
    uploaded_frames: u64,
}

impl DecodedFrameUploader {
    /// Creates an uploader.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            #[cfg(waterkit_hw_codec_apple)]
            apple: None,
            imported_frames: 0,
            uploaded_frames: 0,
        }
    }

    /// Frames imported in place from native storage with no copy.
    #[must_use]
    pub const fn imported_frames(&self) -> u64 {
        self.imported_frames
    }

    /// Software frames written into newly created textures.
    #[must_use]
    pub const fn uploaded_frames(&self) -> u64 {
        self.uploaded_frames
    }

    /// Turns a decoded frame into a [`GpuFrame`] on `device`.
    ///
    /// Apple hardware frames are imported in place — the returned textures
    /// share the `IOSurface` storage, nothing is copied. Software frames are
    /// written to freshly created textures exactly once.
    #[must_use]
    pub fn upload(&mut self, decoded: DecodedFrame, device: &Device, queue: &Queue) -> GpuFrame {
        #[cfg(waterkit_any_codec)]
        {
            let width = decoded.width();
            let height = decoded.height();
            let layout = decoded.pixel_layout();
            match decoded.inner {
                #[cfg(waterkit_hw_codec_apple)]
                DecodedFrameInner::Hardware {
                    pixel_buffer,
                    timestamp_ns,
                    ..
                } => {
                    self.imported_frames += 1;
                    self.apple
                        .get_or_insert_with(|| apple::AppleFrameUploader::new(device))
                        .import_frame(device, &pixel_buffer, width, height, layout, timestamp_ns)
                }
                #[cfg(waterkit_software_frames)]
                DecodedFrameInner::Software {
                    data, timestamp_ns, ..
                } => {
                    self.uploaded_frames += 1;
                    GpuFrame::uploaded(device, queue, width, height, layout, &data, timestamp_ns)
                }
            }
        }
        #[cfg(not(waterkit_any_codec))]
        {
            let _ = (device, queue);
            match decoded.inner {}
        }
    }
}

impl Default for DecodedFrameUploader {
    fn default() -> Self {
        Self::new()
    }
}

/// A decoded video frame backed by YUV textures on GPU.
///
/// The frame is stored in its native bi-planar NV12 or P010 layout, as
/// `R8Uint`/`Rg8Uint` or `R16Uint`/`Rg16Uint` integer textures — the formats
/// the engine's external-frame contract takes. Hardware-decoded frames are
/// imported in place: their textures share the `IOSurface` storage, so
/// dropping this frame releases the planes back to the decoder's pool.
/// Use [`to_linear_rgba`](Self::to_linear_rgba) to convert to RGBA via compute shader.
#[derive(Clone)]
pub struct GpuFrame {
    y_texture: Arc<Texture>,
    uv_texture: Arc<Texture>,
    width: u32,
    height: u32,
    timestamp_ns: u64,
    layout: DecodedPixelLayout,
}

impl std::fmt::Debug for GpuFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("timestamp", &self.timestamp())
            .finish_non_exhaustive()
    }
}

impl GpuFrame {
    /// Wraps plane textures imported from a decoded frame's native storage.
    #[cfg(waterkit_hw_codec_apple)]
    pub(super) fn imported(
        y_texture: Texture,
        uv_texture: Texture,
        width: u32,
        height: u32,
        layout: DecodedPixelLayout,
        timestamp_ns: u64,
    ) -> Self {
        Self {
            y_texture: Arc::new(y_texture),
            uv_texture: Arc::new(uv_texture),
            width,
            height,
            timestamp_ns,
            layout,
        }
    }

    /// Creates the plane textures and writes a software-decoded frame into
    /// them exactly once.
    #[cfg(waterkit_software_frames)]
    fn uploaded(
        device: &Device,
        queue: &Queue,
        width: u32,
        height: u32,
        layout: DecodedPixelLayout,
        data: &[u8],
        timestamp_ns: u64,
    ) -> Self {
        let (y_format, uv_format) = match layout {
            DecodedPixelLayout::Nv12 => (TextureFormat::R8Uint, TextureFormat::Rg8Uint),
            DecodedPixelLayout::P010 => (TextureFormat::R16Uint, TextureFormat::Rg16Uint),
        };
        let texture = |label, texture_width, texture_height, format| {
            device.create_texture(&TextureDescriptor {
                label: Some(label),
                size: Extent3d {
                    width: texture_width,
                    height: texture_height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let y_texture = texture("GpuFrame Y", width, height, y_format);
        let uv_width = width.div_ceil(2).max(1);
        let uv_height = height.div_ceil(2).max(1);
        let uv_texture = texture("GpuFrame UV", uv_width, uv_height, uv_format);

        let row_bytes = layout.bytes_per_row(width);
        let y_size = row_bytes * height as usize;
        queue.write_texture(
            y_texture.as_image_copy(),
            &data[..y_size],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(u32::try_from(row_bytes).expect("row bytes must fit in u32")),
                rows_per_image: Some(height),
            },
            y_texture.size(),
        );
        queue.write_texture(
            uv_texture.as_image_copy(),
            &data[y_size..],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(u32::try_from(row_bytes).expect("row bytes must fit in u32")),
                rows_per_image: Some(uv_height),
            },
            uv_texture.size(),
        );

        Self {
            y_texture: Arc::new(y_texture),
            uv_texture: Arc::new(uv_texture),
            width,
            height,
            timestamp_ns,
            layout,
        }
    }

    /// Get the Y plane texture.
    #[must_use]
    pub fn y_texture(&self) -> &Texture {
        &self.y_texture
    }

    /// Get the UV plane texture (interleaved, half resolution).
    #[must_use]
    pub fn uv_texture(&self) -> &Texture {
        &self.uv_texture
    }

    /// Get the frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Get the frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Returns the presentation timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(self.timestamp_ns)
    }

    /// Returns the decoded pixel layout represented by the textures.
    #[must_use]
    pub const fn pixel_layout(&self) -> DecodedPixelLayout {
        self.layout
    }

    /// Converts native YUV into linear extended-range RGBA16F on the GPU.
    ///
    /// Use [`LinearRgbaConverter`] for repeated conversions so the compute
    /// pipeline is created once.
    #[must_use]
    pub fn to_linear_rgba(&self, device: &Device, queue: &Queue, color: VideoColorInfo) -> Texture {
        let converter = LinearRgbaConverter::new(device);
        converter.convert(device, queue, self, color)
    }
}

/// Reusable native-YUV to linear RGBA16F converter pipeline.
///
/// The output uses sRGB/BT.709 primaries and linear light relative to a
/// 203-nit reference white. HDR values intentionally remain above `1.0`.
pub struct LinearRgbaConverter {
    pipeline: ComputePipeline,
    bind_group_layout: BindGroupLayout,
}

impl std::fmt::Debug for LinearRgbaConverter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinearRgbaConverter")
            .finish_non_exhaustive()
    }
}

impl LinearRgbaConverter {
    /// Creates a reusable linear RGBA16F converter.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        let shader = YUV_COLOR_SHADER.create_entry_point(
            device,
            ShaderStage::Compute,
            "convert_to_linear_rgba",
        );

        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("YUV converter bind group layout"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Uint,
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Uint,
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 3,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: Some(std::num::NonZeroU64::MIN.saturating_add(31)),
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 4,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageTexture {
                        access: StorageTextureAccess::WriteOnly,
                        format: TextureFormat::Rgba16Float,
                        view_dimension: TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("YUV converter pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
            label: Some("YUV to RGBA pipeline"),
            layout: Some(&pipeline_layout),
            module: shader.module(),
            entry_point: Some(shader.entry_point()),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
        }
    }

    /// Converts one YUV frame to linear RGBA16F while preserving HDR range.
    #[must_use]
    pub fn convert(
        &self,
        device: &Device,
        queue: &Queue,
        frame: &GpuFrame,
        color: VideoColorInfo,
    ) -> Texture {
        let output = device.create_texture(&TextureDescriptor {
            label: Some("Linear RGBA16F video frame"),
            size: Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba16Float,
            usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let y_view = frame
            .y_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let uv_view = frame
            .uv_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let uniform = create_video_color_uniform_buffer(device, frame.layout, color);

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("YUV converter bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&y_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(&uv_view),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: uniform.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: BindingResource::TextureView(&output_view),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(frame.width.div_ceil(8), frame.height.div_ceil(8), 1);
        }
        queue.submit(Some(encoder.finish()));

        output
    }
}

fn create_video_color_uniform_buffer(
    device: &Device,
    layout: DecodedPixelLayout,
    color: VideoColorInfo,
) -> Buffer {
    let uniform = video_color_uniform(color, layout, ColorOutputTarget::LinearHdr);
    let bytes = uniform.to_bytes();
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("WaterKit video color uniform"),
        size: 32,
        usage: BufferUsages::UNIFORM,
        mapped_at_creation: true,
    });
    {
        let mut mapped = buffer
            .slice(..)
            .get_mapped_range_mut()
            .expect("a mapped-at-creation buffer exposes its full range");
        mapped.copy_from_slice(&bytes);
    }
    buffer.unmap();
    buffer
}
