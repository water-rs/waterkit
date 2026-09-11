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

use super::{DecodedFrame, DecodedFrameInner, DecodedPixelLayout};
use crate::{ColorOutputTarget, video_color_uniform};
use shaderloom::{CompiledShader, ShaderStage};

#[cfg(target_vendor = "apple")]
mod apple;

const YUV_COLOR_SHADER: CompiledShader = include!(concat!(env!("OUT_DIR"), "/yuv_color.rs"));

impl DecodedFrame {
    /// Convert to GPU frame by uploading to the user's device.
    ///
    /// This consumes the decoded frame and creates GPU textures on the provided device.
    #[must_use]
    pub fn to_gpu_frame(self, device: &Device, queue: &Queue) -> GpuFrame {
        DecodedFrameUploader::new().upload(self, device, queue)
    }
}

/// Reusable decoded-frame uploader that retains GPU plane textures across frames.
#[derive(Debug)]
pub struct DecodedFrameUploader {
    cached: Option<GpuFrame>,
    #[cfg(target_vendor = "apple")]
    apple: Option<apple::AppleFrameUploader>,
}

impl DecodedFrameUploader {
    /// Creates an uploader with no allocated textures.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cached: None,
            #[cfg(target_vendor = "apple")]
            apple: None,
        }
    }

    /// Uploads a decoded frame, reusing textures while dimensions and layout remain stable.
    #[must_use]
    pub fn upload(&mut self, decoded: DecodedFrame, device: &Device, queue: &Queue) -> GpuFrame {
        let width = decoded.width();
        let height = decoded.height();
        let layout = decoded.pixel_layout();
        let replace = self.cached.as_ref().is_some_and(|cached| {
            cached.width != width || cached.height != height || cached.layout != layout
        });
        if replace {
            self.cached = None;
        }
        let cached = self
            .cached
            .get_or_insert_with(|| GpuFrame::initialized(device, queue, width, height, layout));
        let timestamp_ns = match decoded.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware {
                pixel_buffer,
                timestamp_ns,
                ..
            } => {
                self.apple
                    .get_or_insert_with(|| apple::AppleFrameUploader::new(queue))
                    .copy_surface_planes(
                        queue,
                        apple::SurfacePlaneCopy {
                            pixel_buffer: &pixel_buffer,
                            y_target: &cached.y_texture,
                            uv_target: &cached.uv_texture,
                            width,
                            height,
                            layout,
                        },
                    );
                timestamp_ns
            }
            #[cfg(waterkit_software_frames)]
            DecodedFrameInner::Software {
                data, timestamp_ns, ..
            } => {
                cached.write_biplanar(queue, &data);
                timestamp_ns
            }
        };
        cached.timestamp_ns = timestamp_ns;
        cached.clone()
    }
}

impl Default for DecodedFrameUploader {
    fn default() -> Self {
        Self::new()
    }
}

/// A decoded video frame backed by YUV textures on GPU.
///
/// The frame is stored in its native bi-planar NV12 or P010 layout.
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
    fn initialized(
        device: &Device,
        queue: &Queue,
        width: u32,
        height: u32,
        layout: DecodedPixelLayout,
    ) -> Self {
        let (y_texture, uv_texture) = Self::create_biplanar_textures(device, width, height, layout);
        let zero_pixel = match layout {
            DecodedPixelLayout::Nv12 => &[0_u8; 2][..],
            DecodedPixelLayout::P010 => &[0_u8; 4][..],
        };
        for texture in [&y_texture, &uv_texture] {
            queue.write_texture(
                texture.as_image_copy(),
                zero_pixel,
                wgpu::TexelCopyBufferLayout::default(),
                Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        queue.submit([]);
        Self {
            y_texture: Arc::new(y_texture),
            uv_texture: Arc::new(uv_texture),
            width,
            height,
            timestamp_ns: 0,
            layout,
        }
    }

    fn create_biplanar_textures(
        device: &Device,
        width: u32,
        height: u32,
        layout: DecodedPixelLayout,
    ) -> (Texture, Texture) {
        let (y_format, uv_format) = match layout {
            DecodedPixelLayout::Nv12 => (TextureFormat::R8Unorm, TextureFormat::Rg8Unorm),
            DecodedPixelLayout::P010 => (TextureFormat::R16Unorm, TextureFormat::Rg16Unorm),
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
        (
            texture("GpuFrame Y", width, height, y_format),
            texture(
                "GpuFrame UV",
                (width / 2).max(1),
                (height / 2).max(1),
                uv_format,
            ),
        )
    }

    #[cfg(waterkit_software_frames)]
    fn write_biplanar(&self, queue: &Queue, data: &[u8]) {
        let row_bytes = self.layout.bytes_per_row(self.width);
        let y_size = row_bytes * self.height as usize;
        queue.write_texture(
            self.y_texture.as_image_copy(),
            &data[..y_size],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(u32::try_from(row_bytes).expect("row bytes must fit in u32")),
                rows_per_image: Some(self.height),
            },
            self.y_texture.size(),
        );
        queue.write_texture(
            self.uv_texture.as_image_copy(),
            &data[y_size..],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(u32::try_from(row_bytes).expect("row bytes must fit in u32")),
                rows_per_image: Some((self.height / 2).max(1)),
            },
            self.uv_texture.size(),
        );
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
                        sample_type: TextureSampleType::Float { filterable: false },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: false },
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
        let mut mapped = buffer.slice(..).get_mapped_range_mut();
        mapped.copy_from_slice(&bytes);
    }
    buffer.unmap();
    buffer
}
