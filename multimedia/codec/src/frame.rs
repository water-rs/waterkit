//! GPU-backed video frame with YUV texture representation.

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

use crate::{ColorOutputTarget, YUV_COLOR_SHADER, video_color_uniform};
use shaderloom::ShaderStage;

#[cfg(target_vendor = "apple")]
use {
    objc2_core_foundation::CFRetained, objc2_core_video::CVPixelBuffer,
    objc2_io_surface::IOSurfaceRef, std::ptr,
};

#[cfg(target_vendor = "apple")]
mod apple_gpu;

/// Decoded frame - opaque type hiding platform details.
///
/// This represents a decoded video frame that has not yet been converted to GPU textures.
/// Use [`to_gpu_frame`](Self::to_gpu_frame) to create GPU textures on the user's device.
pub struct DecodedFrame {
    inner: DecodedFrameInner,
}

/// Reusable decoded-frame uploader that retains GPU plane textures across frames.
#[derive(Debug)]
pub struct DecodedFrameUploader {
    cached: Option<GpuFrame>,
    #[cfg(target_vendor = "apple")]
    apple: Option<apple_gpu::AppleFrameUploader>,
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
                    .get_or_insert_with(|| apple_gpu::AppleFrameUploader::new(queue))
                    .copy_surface_planes(
                        queue,
                        apple_gpu::SurfacePlaneCopy {
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

// SAFETY: Apple IOSurfaces are explicitly cross-thread shareable allocations and
// the retained CF ownership keeps their storage alive until the frame is dropped.
#[cfg(target_vendor = "apple")]
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for DecodedFrame {}

/// Native decoded YUV storage layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedPixelLayout {
    /// 8-bit bi-planar 4:2:0 video-range YUV (`420v`).
    Nv12,
    /// 10-bit bi-planar 4:2:0 video-range YUV in 16-bit lanes (`x420`).
    P010,
}

impl DecodedPixelLayout {
    /// Number of bytes required for a tightly packed frame.
    #[must_use]
    pub const fn packed_len(self, width: u32, height: u32) -> usize {
        let samples = (width as usize) * (height as usize) * 3 / 2;
        match self {
            Self::Nv12 => samples,
            Self::P010 => samples * 2,
        }
    }

    /// Number of bytes in one luma or interleaved chroma row.
    #[must_use]
    pub const fn bytes_per_row(self, width: u32) -> usize {
        match self {
            Self::Nv12 => width as usize,
            Self::P010 => width as usize * 2,
        }
    }
}

/// Private enum holding platform-specific frame data.
enum DecodedFrameInner {
    /// Hardware-decoded frame backed by `IOSurface` (Apple only).
    #[cfg(target_vendor = "apple")]
    Hardware {
        surface: CFRetained<IOSurfaceRef>,
        pixel_buffer: CFRetained<CVPixelBuffer>,
        width: u32,
        height: u32,
        timestamp_ns: u64,
        layout: DecodedPixelLayout,
    },
    /// Software-decoded frame with NV12 data.
    /// Available on non-Apple platforms, or desktop Apple platforms with software-fallback.
    #[cfg(waterkit_software_frames)]
    Software {
        data: Vec<u8>,
        width: u32,
        height: u32,
        timestamp_ns: u64,
        layout: DecodedPixelLayout,
    },
}

impl std::fmt::Debug for DecodedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedFrame")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("timestamp", &self.timestamp())
            .finish_non_exhaustive()
    }
}

impl DecodedFrame {
    /// Create a decoded frame from hardware `IOSurface` (Apple only).
    #[cfg(target_vendor = "apple")]
    pub(crate) const fn from_iosurface(
        surface: CFRetained<IOSurfaceRef>,
        pixel_buffer: CFRetained<CVPixelBuffer>,
        width: u32,
        height: u32,
        timestamp_ns: u64,
        layout: DecodedPixelLayout,
    ) -> Self {
        Self {
            inner: DecodedFrameInner::Hardware {
                surface,
                pixel_buffer,
                width,
                height,
                timestamp_ns,
                layout,
            },
        }
    }

    /// Create a decoded frame from tightly packed bi-planar software output.
    #[cfg(waterkit_software_frames)]
    pub(crate) const fn from_biplanar_data(
        data: Vec<u8>,
        width: u32,
        height: u32,
        timestamp_ns: u64,
        layout: DecodedPixelLayout,
    ) -> Self {
        Self {
            inner: DecodedFrameInner::Software {
                data,
                width,
                height,
                timestamp_ns,
                layout,
            },
        }
    }

    /// Returns the native decoded pixel layout.
    #[must_use]
    pub const fn pixel_layout(&self) -> DecodedPixelLayout {
        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { layout, .. } => *layout,
            #[cfg(waterkit_software_frames)]
            DecodedFrameInner::Software { layout, .. } => *layout,
        }
    }

    /// Get the frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { width, .. } => *width,
            #[cfg(waterkit_software_frames)]
            DecodedFrameInner::Software { width, .. } => *width,
        }
    }

    /// Get the frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { height, .. } => *height,
            #[cfg(waterkit_software_frames)]
            DecodedFrameInner::Software { height, .. } => *height,
        }
    }

    /// Returns the presentation timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> std::time::Duration {
        let ns = match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { timestamp_ns, .. } => *timestamp_ns,
            #[cfg(waterkit_software_frames)]
            DecodedFrameInner::Software { timestamp_ns, .. } => *timestamp_ns,
        };
        std::time::Duration::from_nanos(ns)
    }

    /// Convert to GPU frame by uploading to the user's device.
    ///
    /// This consumes the decoded frame and creates GPU textures on the provided device.
    #[must_use]
    pub fn to_gpu_frame(self, device: &Device, queue: &Queue) -> GpuFrame {
        DecodedFrameUploader::new().upload(self, device, queue)
    }

    /// Copy the native bi-planar data to a provided buffer slice.
    ///
    /// Returns the number of bytes written. The buffer must be large enough for
    /// [`DecodedPixelLayout::packed_len`] bytes at this frame's dimensions.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is too small.
    pub fn copy_to_buffer(&self, output: &mut [u8]) -> usize {
        let width = self.width();
        let height = self.height();
        let required_size = self.pixel_layout().packed_len(width, height);
        assert!(
            output.len() >= required_size,
            "buffer too small: need {required_size}, got {}",
            output.len()
        );

        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware {
                surface, layout, ..
            } => {
                Self::copy_iosurface_to_buffer(surface, width, height, *layout, output);
            }
            #[cfg(waterkit_software_frames)]
            DecodedFrameInner::Software { data, .. } => {
                output[..data.len()].copy_from_slice(data);
            }
        }

        required_size
    }

    /// Copy `IOSurface` data to a buffer.
    #[cfg(target_vendor = "apple")]
    fn copy_iosurface_to_buffer(
        surface: &CFRetained<IOSurfaceRef>,
        width: u32,
        height: u32,
        layout: DecodedPixelLayout,
        output: &mut [u8],
    ) {
        use objc2_io_surface::IOSurfaceLockOptions;

        let row_bytes = layout.bytes_per_row(width);
        let y_size = row_bytes * height as usize;

        unsafe {
            let surface_ref = CFRetained::as_ptr(surface).as_ref();

            // Read-only lock
            surface_ref.lock(IOSurfaceLockOptions::ReadOnly, ptr::null_mut());

            // Copy Y plane
            let y_base = surface_ref.base_address_of_plane(0).as_ptr().cast::<u8>();
            let y_stride = surface_ref.bytes_per_row_of_plane(0);
            for row in 0..height as usize {
                ptr::copy_nonoverlapping(
                    y_base.add(row * y_stride),
                    output.as_mut_ptr().add(row * row_bytes),
                    row_bytes,
                );
            }

            // Copy UV plane
            let uv_base = surface_ref.base_address_of_plane(1).as_ptr().cast::<u8>();
            let uv_stride = surface_ref.bytes_per_row_of_plane(1);
            let uv_height = height as usize / 2;
            for row in 0..uv_height {
                ptr::copy_nonoverlapping(
                    uv_base.add(row * uv_stride),
                    output.as_mut_ptr().add(y_size + row * row_bytes),
                    row_bytes,
                );
            }

            surface_ref.unlock(IOSurfaceLockOptions::ReadOnly, ptr::null_mut());
        }
    }
}

/// A decoded video frame backed by YUV textures on GPU.
///
/// The frame is stored in its native bi-planar NV12 or P010 layout.
/// Use [`to_rgba`](Self::to_rgba) to convert to RGBA via compute shader.
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
/// 100-nit reference white. HDR values intentionally remain above `1.0`.
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
