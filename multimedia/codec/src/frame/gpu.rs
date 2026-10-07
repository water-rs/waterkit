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
    #[cfg_attr(
        not(waterkit_any_codec),
        expect(
            clippy::missing_const_for_fn,
            clippy::needless_pass_by_value,
            reason = "a build without a codec has no decoded frame to upload: `DecodedFrame` is uninhabited there, so the body reduces to an empty match"
        )
    )]
    pub fn upload(
        &mut self,
        decoded: DecodedFrame,
        device: &Device,
        #[cfg_attr(
            all(waterkit_any_codec, not(waterkit_software_frames)),
            expect(
                unused_variables,
                reason = "hardware-only builds import IOSurface frames in place and never upload through the queue"
            )
        )]
        queue: &Queue,
    ) -> GpuFrame {
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
            usage: TextureUsages::STORAGE_BINDING
                | TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_SRC,
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

#[cfg(all(test, waterkit_software_frames))]
mod tests {
    use std::time::Duration;

    use half::f16;
    use waterkit_video_core::{
        ColorPrimaries, ColorRange, MatrixCoefficients, TransferFunction, VideoColorInfo,
    };

    use super::{GpuFrame, LinearRgbaConverter};
    use crate::DecodedPixelLayout;

    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 64;
    const MAX_CODE: u32 = 1023;

    /// One 10-bit code step of tolerance. Comparing in R'G'B' keeps rgba16f
    /// rounding of linear values (which reach ~4 for out-of-gamut chroma,
    /// where a half f16 step is already ~1/1023) from swamping it; the inverse
    /// transfer shrinks that rounding below a quarter step.
    #[test]
    fn p010_frames_convert_every_code_value_within_one_step() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("the codec P010 GPU test needs a GPU adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: shaderloom::required_features(adapter.features()),
            ..Default::default()
        }))
        .expect("the codec P010 GPU test needs a GPU device");

        let mut bytes = Vec::with_capacity(DecodedPixelLayout::P010.packed_len(WIDTH, HEIGHT));
        for index in 0..WIDTH * HEIGHT {
            bytes.extend_from_slice(&p010_element(index % (MAX_CODE + 1)));
        }
        for index in 0..(WIDTH / 2) * (HEIGHT / 2) {
            bytes.extend_from_slice(&p010_element(index));
            bytes.extend_from_slice(&p010_element(MAX_CODE - index));
        }
        let frame = GpuFrame::uploaded(
            &device,
            &queue,
            WIDTH,
            HEIGHT,
            DecodedPixelLayout::P010,
            &bytes,
            0,
        );
        let converter = LinearRgbaConverter::new(&device);
        let matrices = [
            MatrixCoefficients::Bt601,
            MatrixCoefficients::Bt709,
            MatrixCoefficients::Bt2020NonConstantLuminance,
        ];
        let ranges = [ColorRange::Limited, ColorRange::Full];

        for matrix in matrices {
            for range in ranges {
                let output = converter.convert(
                    &device,
                    &queue,
                    &frame,
                    VideoColorInfo {
                        matrix,
                        primaries: ColorPrimaries::Bt709,
                        transfer: TransferFunction::Sdr,
                        range,
                        content_light_level: None,
                        dolby_vision: false,
                    },
                );
                let pixels = read_rgba16f(&device, &queue, &output);
                for y in 0..HEIGHT {
                    for x in 0..WIDTH {
                        let index = (y * WIDTH + x) as usize;
                        let y_code = (y * WIDTH + x) % (MAX_CODE + 1);
                        let chroma_index = (y / 2) * (WIDTH / 2) + x / 2;
                        let blue_chroma_code = chroma_index;
                        let red_chroma_code = MAX_CODE - chroma_index;
                        let expected = reference_rgb(
                            [y_code, blue_chroma_code, red_chroma_code],
                            matrix,
                            range,
                        );
                        let got = pixels[index].map(bt709_from_linear);
                        for (channel, (got, expected)) in got.into_iter().zip(expected).enumerate()
                        {
                            let error = (got - expected).abs();
                            assert!(
                                error <= 1.0 / f64::from(MAX_CODE),
                                "{matrix:?} {range:?} pixel ({x}, {y}) Y'={y_code} Cb={blue_chroma_code} Cr={red_chroma_code} channel {channel}: got {got}, expected {expected}, error {error}"
                            );
                        }
                    }
                }
            }
        }
    }

    fn p010_element(code: u32) -> [u8; 2] {
        u16::try_from(code << 6)
            .expect("10-bit P010 codes fit in the high 10 bits of u16")
            .to_le_bytes()
    }

    fn read_rgba16f(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
    ) -> Vec<[f64; 3]> {
        let size = texture.size();
        let row_bytes = size.width * 8;
        let padded_row_bytes = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("codec P010 test readback"),
            size: u64::from(padded_row_bytes * size.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(size.height),
                },
            },
            size,
        );
        queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).expect("the readback receiver is alive");
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(30)),
            })
            .expect("the GPU completes P010 test readback");
        receiver
            .recv()
            .expect("the readback callback completes")
            .expect("the readback buffer maps");
        let mapped = slice
            .get_mapped_range()
            .expect("the mapped readback buffer exposes its full range");
        let pixels = (0..size.height)
            .flat_map(|y| {
                let row_start = (y * padded_row_bytes) as usize;
                let row = &mapped[row_start..row_start + row_bytes as usize];
                row.as_chunks::<8>().0.iter().map(|pixel| {
                    [0, 1, 2].map(|channel| {
                        let offset = channel * 2;
                        let bits = u16::from_ne_bytes([pixel[offset], pixel[offset + 1]]);
                        f64::from(f16::from_bits(bits).to_f32())
                    })
                })
            })
            .collect();
        drop(mapped);
        buffer.unmap();
        pixels
    }

    fn reference_rgb(
        [y, cb, cr]: [u32; 3],
        matrix: MatrixCoefficients,
        range: ColorRange,
    ) -> [f64; 3] {
        let [y, cb, cr] = [y, cb, cr].map(f64::from);
        let (y, cb, cr) = match range {
            ColorRange::Limited => (
                (y - 64.0) / 876.0,
                (cb - 512.0) / 896.0,
                (cr - 512.0) / 896.0,
            ),
            ColorRange::Full => (
                y / f64::from(MAX_CODE),
                (cb - 512.0) / 1023.0,
                (cr - 512.0) / 1023.0,
            ),
        };
        let (kr, kb): (f64, f64) = match matrix {
            MatrixCoefficients::Bt601 => (0.299, 0.114),
            MatrixCoefficients::Bt709 => (0.2126, 0.0722),
            MatrixCoefficients::Bt2020NonConstantLuminance => (0.2627, 0.0593),
            MatrixCoefficients::Bt2020ConstantLuminance => {
                unreachable!("the test only uses non-constant-luminance matrices")
            }
        };
        let red = (2.0 * (1.0 - kr)).mul_add(cr, y);
        let blue = (2.0 * (1.0 - kb)).mul_add(cb, y);
        let green = kb.mul_add(-blue, kr.mul_add(-red, y)) / (1.0 - kr - kb);
        [red, green, blue].map(|channel| channel.max(0.0))
    }

    fn bt709_from_linear(linear: f64) -> f64 {
        if linear < 0.081 / 4.5 {
            4.5 * linear
        } else {
            1.099_f64.mul_add(linear.powf(0.45), -0.099)
        }
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
