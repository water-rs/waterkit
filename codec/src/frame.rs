//! GPU-backed video frame with YUV texture representation.

use std::sync::Arc;
use wgpu::{
    BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, ComputePipeline, ComputePipelineDescriptor,
    Device, Extent3d, PipelineLayoutDescriptor, Queue, ShaderStages, StorageTextureAccess, Texture,
    TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
    TextureViewDimension,
};

#[cfg(target_vendor = "apple")]
use {objc2_core_foundation::CFRetained, objc2_io_surface::IOSurfaceRef, std::ptr};

/// Decoded frame - opaque type hiding platform details.
///
/// This represents a decoded video frame that has not yet been converted to GPU textures.
/// Use [`to_gpu_frame`](Self::to_gpu_frame) to create GPU textures on the user's device.
pub struct DecodedFrame {
    inner: DecodedFrameInner,
}

/// Private enum holding platform-specific frame data.
enum DecodedFrameInner {
    /// Hardware-decoded frame backed by `IOSurface` (Apple only).
    #[cfg(target_vendor = "apple")]
    Hardware {
        surface: CFRetained<IOSurfaceRef>,
        width: u32,
        height: u32,
        timestamp_ns: u64,
    },
    /// Software-decoded frame with NV12 data.
    /// Available on non-Apple platforms, or desktop Apple platforms with software-fallback.
    #[cfg(any(
        not(target_vendor = "apple"),
        all(
            target_vendor = "apple",
            not(any(target_os = "ios", target_os = "tvos", target_os = "watchos"))
        )
    ))]
    Software {
        data: Vec<u8>,
        width: u32,
        height: u32,
        timestamp_ns: u64,
    },
}

impl std::fmt::Debug for DecodedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedFrame")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("timestamp_ns", &self.timestamp_ns())
            .finish_non_exhaustive()
    }
}

impl DecodedFrame {
    /// Create a decoded frame from hardware `IOSurface` (Apple only).
    #[cfg(target_vendor = "apple")]
    pub(crate) const fn from_iosurface(
        surface: CFRetained<IOSurfaceRef>,
        width: u32,
        height: u32,
        timestamp_ns: u64,
    ) -> Self {
        Self {
            inner: DecodedFrameInner::Hardware {
                surface,
                width,
                height,
                timestamp_ns,
            },
        }
    }

    /// Create a decoded frame from NV12 software decode output.
    #[cfg(any(
        not(target_vendor = "apple"),
        all(
            target_vendor = "apple",
            not(any(target_os = "ios", target_os = "tvos", target_os = "watchos"))
        )
    ))]
    pub(crate) const fn from_nv12_data(
        data: Vec<u8>,
        width: u32,
        height: u32,
        timestamp_ns: u64,
    ) -> Self {
        Self {
            inner: DecodedFrameInner::Software {
                data,
                width,
                height,
                timestamp_ns,
            },
        }
    }

    /// Get the frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { width, .. } => *width,
            #[cfg(any(
                not(target_vendor = "apple"),
                all(
                    target_vendor = "apple",
                    not(any(target_os = "ios", target_os = "tvos", target_os = "watchos"))
                )
            ))]
            DecodedFrameInner::Software { width, .. } => *width,
        }
    }

    /// Get the frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { height, .. } => *height,
            #[cfg(any(
                not(target_vendor = "apple"),
                all(
                    target_vendor = "apple",
                    not(any(target_os = "ios", target_os = "tvos", target_os = "watchos"))
                )
            ))]
            DecodedFrameInner::Software { height, .. } => *height,
        }
    }

    /// Get the presentation timestamp in nanoseconds.
    #[must_use]
    pub const fn timestamp_ns(&self) -> u64 {
        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { timestamp_ns, .. } => *timestamp_ns,
            #[cfg(any(
                not(target_vendor = "apple"),
                all(
                    target_vendor = "apple",
                    not(any(target_os = "ios", target_os = "tvos", target_os = "watchos"))
                )
            ))]
            DecodedFrameInner::Software { timestamp_ns, .. } => *timestamp_ns,
        }
    }

    /// Convert to GPU frame by uploading to the user's device.
    ///
    /// This consumes the decoded frame and creates GPU textures on the provided device.
    #[must_use]
    pub fn to_gpu_frame(self, device: &Device, queue: &Queue) -> GpuFrame {
        match self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware {
                surface,
                width,
                height,
                timestamp_ns,
            } => {
                // Copy IOSurface to NV12, then upload to GPU
                let nv12_data = Self::iosurface_to_nv12(&surface, width, height);
                GpuFrame::from_nv12(device, queue, &nv12_data, width, height, timestamp_ns)
            }
            #[cfg(any(
                not(target_vendor = "apple"),
                all(
                    target_vendor = "apple",
                    not(any(target_os = "ios", target_os = "tvos", target_os = "watchos"))
                )
            ))]
            DecodedFrameInner::Software {
                data,
                width,
                height,
                timestamp_ns,
            } => GpuFrame::from_nv12(device, queue, &data, width, height, timestamp_ns),
        }
    }

    /// Copy NV12 data to a provided buffer slice.
    ///
    /// Returns the number of bytes written. The buffer must be large enough
    /// to hold the NV12 data (width * height * 3 / 2 bytes).
    ///
    /// # Panics
    ///
    /// Panics if the buffer is too small.
    pub fn copy_to_buffer(&self, output: &mut [u8]) -> usize {
        let width = self.width();
        let height = self.height();
        let required_size = (width * height * 3 / 2) as usize;
        assert!(
            output.len() >= required_size,
            "buffer too small: need {required_size}, got {}",
            output.len()
        );

        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { surface, .. } => {
                Self::copy_iosurface_to_buffer(surface, width, height, output);
            }
            #[cfg(any(
                not(target_vendor = "apple"),
                all(
                    target_vendor = "apple",
                    not(any(target_os = "ios", target_os = "tvos", target_os = "watchos"))
                )
            ))]
            DecodedFrameInner::Software { data, .. } => {
                output[..data.len()].copy_from_slice(data);
            }
        }

        required_size
    }

    /// Copy `IOSurface` data to NV12 format.
    #[cfg(target_vendor = "apple")]
    fn iosurface_to_nv12(surface: &CFRetained<IOSurfaceRef>, width: u32, height: u32) -> Vec<u8> {
        let y_size = (width * height) as usize;
        let uv_size = y_size / 2;
        let mut nv12_data = vec![0u8; y_size + uv_size];

        Self::copy_iosurface_to_buffer(surface, width, height, &mut nv12_data);
        nv12_data
    }

    /// Copy `IOSurface` data to a buffer.
    #[cfg(target_vendor = "apple")]
    fn copy_iosurface_to_buffer(
        surface: &CFRetained<IOSurfaceRef>,
        width: u32,
        height: u32,
        output: &mut [u8],
    ) {
        use objc2_io_surface::IOSurfaceLockOptions;

        let y_size = (width * height) as usize;

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
                    output.as_mut_ptr().add(row * width as usize),
                    width as usize,
                );
            }

            // Copy UV plane
            let uv_base = surface_ref.base_address_of_plane(1).as_ptr().cast::<u8>();
            let uv_stride = surface_ref.bytes_per_row_of_plane(1);
            let uv_height = height as usize / 2;
            for row in 0..uv_height {
                ptr::copy_nonoverlapping(
                    uv_base.add(row * uv_stride),
                    output.as_mut_ptr().add(y_size + row * width as usize),
                    width as usize,
                );
            }

            surface_ref.unlock(IOSurfaceLockOptions::ReadOnly, ptr::null_mut());
        }
    }
}

/// A decoded video frame backed by YUV textures on GPU.
///
/// The frame is stored in NV12 format (Y plane + interleaved UV plane).
/// Use [`to_rgba`](Self::to_rgba) to convert to RGBA via compute shader.
#[derive(Clone)]
pub struct GpuFrame {
    y_texture: Arc<Texture>,
    uv_texture: Arc<Texture>,
    width: u32,
    height: u32,
    timestamp_ns: u64,
}

impl std::fmt::Debug for GpuFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("timestamp_ns", &self.timestamp_ns)
            .finish_non_exhaustive()
    }
}

impl GpuFrame {
    /// Create a GPU frame from NV12 data (Y plane followed by interleaved UV).
    pub(crate) fn from_nv12(
        device: &Device,
        queue: &Queue,
        data: &[u8],
        width: u32,
        height: u32,
        timestamp_ns: u64,
    ) -> Self {
        let y_size = (width * height) as usize;

        let y_texture = device.create_texture(&TextureDescriptor {
            label: Some("GpuFrame Y"),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::R8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let uv_texture = device.create_texture(&TextureDescriptor {
            label: Some("GpuFrame UV"),
            size: Extent3d {
                width: width / 2,
                height: height / 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rg8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload Y plane
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &y_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data[..y_size],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        // Upload UV plane
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &uv_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data[y_size..],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width), // UV is interleaved, so same width in bytes
                rows_per_image: Some(height / 2),
            },
            Extent3d {
                width: width / 2,
                height: height / 2,
                depth_or_array_layers: 1,
            },
        );

        Self {
            y_texture: Arc::new(y_texture),
            uv_texture: Arc::new(uv_texture),
            width,
            height,
            timestamp_ns,
        }
    }

    /// Create a GPU frame wrapping existing YUV textures (zero-copy).
    #[allow(dead_code)]
    pub(crate) fn from_yuv_textures(
        y_texture: Texture,
        uv_texture: Texture,
        width: u32,
        height: u32,
        timestamp_ns: u64,
    ) -> Self {
        Self {
            y_texture: Arc::new(y_texture),
            uv_texture: Arc::new(uv_texture),
            width,
            height,
            timestamp_ns,
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

    /// Get the presentation timestamp in nanoseconds.
    #[must_use]
    pub const fn timestamp_ns(&self) -> u64 {
        self.timestamp_ns
    }

    /// Convert YUV to RGBA using a compute shader.
    ///
    /// Returns a new RGBA texture. Use [`YuvConverter`] for batch conversions
    /// to avoid recreating the pipeline each time.
    #[must_use]
    pub fn to_rgba(&self, device: &Device, queue: &Queue) -> Texture {
        let converter = YuvConverter::new(device);
        converter.convert(device, queue, self)
    }
}

/// Reusable YUV to RGBA converter pipeline.
///
/// Create once and reuse for multiple frames to avoid pipeline recreation overhead.
pub struct YuvConverter {
    pipeline: ComputePipeline,
    bind_group_layout: BindGroupLayout,
}

impl std::fmt::Debug for YuvConverter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YuvConverter").finish_non_exhaustive()
    }
}

impl YuvConverter {
    /// Create a new YUV to RGBA converter.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("YUV to RGBA shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("yuv_to_rgba.wgsl").into()),
        });

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
                    binding: 2,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageTexture {
                        access: StorageTextureAccess::WriteOnly,
                        format: TextureFormat::Rgba8Unorm,
                        view_dimension: TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("YUV converter pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            ..Default::default()
        });

        let pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
            label: Some("YUV to RGBA pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
        }
    }

    /// Convert a YUV frame to RGBA.
    #[must_use]
    pub fn convert(&self, device: &Device, queue: &Queue, frame: &GpuFrame) -> Texture {
        let output = device.create_texture(&TextureDescriptor {
            label: Some("RGBA output"),
            size: Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
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
                    binding: 2,
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
