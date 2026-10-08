//! GPU conversion of any camera [`Frame`] to an upright RGBA texture.

use std::collections::HashMap;

use shaderloom::{CompiledShader, ShaderStage};
use waterkit_video_core::{MatrixCoefficients, TransferFunction, VideoColorInfo};
use wgpu::util::DeviceExt as _;

use crate::CameraError;
use crate::frame::{Frame, FramePlanes};

const CONVERT_RGB: CompiledShader = include!(concat!(env!("OUT_DIR"), "/frame_convert_rgb.rs"));
const CONVERT_YCBCR420: CompiledShader =
    include!(concat!(env!("OUT_DIR"), "/frame_convert_ycbcr420.rs"));
const CONVERT_YCBCR422: CompiledShader =
    include!(concat!(env!("OUT_DIR"), "/frame_convert_ycbcr422.rs"));

/// Workgroup edge of every converter entry point.
const WORKGROUP_SIZE: u32 = 8;

/// The texture format [`FrameConverter`] writes.
pub const UPRIGHT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Renders camera frames of any plane layout and orientation to upright
/// [`UPRIGHT_FORMAT`] textures on the GPU.
///
/// The output (`UPRIGHT_FORMAT`) is non-linear R'G'B' in the frame's own
/// primaries and SDR transfer. The converter removes the YCbCr matrix/range
/// and orientation only; it does not convert primaries or transfer.
///
/// It accepts exactly frames with `transfer == TransferFunction::Sdr`,
/// `dolby_vision == false`, and a matrix in
/// `{Bt601, Bt709, Bt2020NonConstantLuminance}`. Any primaries and range are
/// accepted. RGB planes do not use matrix or range when converting, but the
/// description must still be one the converter accepts. PQ, HLG,
/// `Bt2020ConstantLuminance`, and Dolby Vision fail fast with
/// [`CameraError::UnsupportedColor`].
///
/// To sample the output linearized, create it from
/// [`Self::output_descriptor`] with `Rgba8UnormSrgb` among its view formats,
/// on devices whose downlevel capabilities include view formats.
///
/// The converter's shaders are compiled ahead of time, so the device must be
/// created with [`FrameConverter::required_features`];
/// [`FrameConverter::check_device`] says whether it was.
///
/// The converter keeps one uniform buffer per distinct set of conversion
/// parameters it has seen (orientation, matrix/range, sample depth and size,
/// which a camera rarely changes) and the view of the last output texture.
/// The bind group is made per frame, since it names the frame's own planes.
#[derive(Debug)]
pub struct FrameConverter {
    rgb: ConvertPass,
    ycbcr420: ConvertPass,
    ycbcr422: ConvertPass,
    params: HashMap<[u8; 32], wgpu::Buffer>,
    output: Option<(wgpu::Texture, wgpu::TextureView)>,
}

#[derive(Debug)]
struct ConvertPass {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl ConvertPass {
    fn new(device: &wgpu::Device, shader: &CompiledShader, entry_point: &str) -> Self {
        let module = shader.create_entry_point(device, ShaderStage::Compute, entry_point);
        let layout = shader
            .create_bind_group_layouts(device)
            .into_iter()
            .next()
            .expect("every converter shader declares bind group 0");
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(entry_point),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry_point),
            layout: Some(&pipeline_layout),
            module: module.module(),
            entry_point: Some(module.entry_point()),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }
}

/// The shader's `ConvertParams` uniform, in its 32-byte WGSL layout.
#[derive(Debug, Clone, Copy)]
struct ConvertParams {
    orientation: u32,
    matrix_mode: u32,
    range_mode: u32,
    bit_depth: u32,
    element_bits: u32,
    stored_width: u32,
    stored_height: u32,
}

impl ConvertParams {
    /// Parameters placing `frame`'s stored pixels upright; the YCbCr fields
    /// stay zero, which the RGB entry point never reads.
    const fn upright(frame: &Frame) -> Self {
        Self {
            orientation: frame.orientation().exif() as u32,
            matrix_mode: 0,
            range_mode: 0,
            bit_depth: 0,
            element_bits: 0,
            stored_width: frame.width(),
            stored_height: frame.height(),
        }
    }

    /// Adds how the YCbCr samples decode.
    const fn ycbcr(self, color: VideoColorInfo, sample: SampleDepth) -> Self {
        Self {
            // YCBCR_MATRIX_* / YCBCR_RANGE_* in waterkit-video-core's ycbcr.wgsl.
            matrix_mode: color
                .matrix
                .ycbcr_mode()
                .expect("color validation rejects constant-luminance BT.2020"),
            range_mode: color.range.ycbcr_mode(),
            bit_depth: sample.bit_depth,
            element_bits: sample.element_bits,
            ..self
        }
    }

    fn to_bytes(self) -> [u8; 32] {
        let words = [
            self.orientation,
            self.matrix_mode,
            self.range_mode,
            self.bit_depth,
            self.element_bits,
            self.stored_width,
            self.stored_height,
            0,
        ];
        let mut bytes = [0_u8; 32];
        for (chunk, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            *chunk = word.to_ne_bytes();
        }
        bytes
    }
}

/// How the significant YCbCr code bits occupy each sampled plane element.
#[derive(Debug, Clone, Copy)]
struct SampleDepth {
    bit_depth: u32,
    element_bits: u32,
}

impl SampleDepth {
    const EIGHT_BIT: Self = Self {
        bit_depth: 8,
        element_bits: 8,
    };

    /// A 16-bit plane holding 10-bit codes in its top bits (P010).
    const TEN_BIT_MSB: Self = Self {
        bit_depth: 10,
        element_bits: 16,
    };

    fn of_luma(luma: &wgpu::TextureView) -> Self {
        match luma.texture().format() {
            wgpu::TextureFormat::R8Unorm | wgpu::TextureFormat::NV12 => Self::EIGHT_BIT,
            wgpu::TextureFormat::R16Unorm | wgpu::TextureFormat::P010 => Self::TEN_BIT_MSB,
            format => panic!("camera frame luma plane has unsupported format {format:?}"),
        }
    }
}

impl FrameConverter {
    /// Checks whether the converter accepts a frame's color description.
    ///
    /// The converter accepts SDR transfer, no Dolby Vision, and BT.601,
    /// BT.709 or non-constant-luminance BT.2020 matrix coefficients. Primaries
    /// and range may be any supported values. RGB planes do not use matrix or
    /// range when converting, but the description must still be one the
    /// converter accepts.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::UnsupportedColor`] when a field is outside the
    /// accepted description.
    pub fn check_color(color: &VideoColorInfo) -> Result<(), CameraError> {
        let acceptance = "the converter accepts SDR transfer, no Dolby Vision, and BT.601, BT.709, or \
             non-constant-luminance BT.2020 matrix coefficients";
        let unsupported = |field: &str| {
            CameraError::UnsupportedColor(format!("field `{field}` in {color:?}; {acceptance}"))
        };
        if color.transfer != TransferFunction::Sdr {
            return Err(unsupported("transfer"));
        }
        if color.dolby_vision {
            return Err(unsupported("dolby_vision"));
        }
        if color.matrix == MatrixCoefficients::Bt2020ConstantLuminance {
            return Err(unsupported("matrix"));
        }
        Ok(())
    }

    /// The device features the converter's precompiled shaders need, out of
    /// what `adapter_features` offers. Request them when creating the device.
    #[must_use]
    pub fn required_features(adapter_features: wgpu::Features) -> wgpu::Features {
        shaderloom::required_features(adapter_features)
    }

    /// Whether the converter can run on `device`: Metal, Vulkan and Direct3D
    /// 12 devices load its precompiled shaders, which needs
    /// `PASSTHROUGH_SHADERS`, one of [`Self::required_features`]; GL and
    /// WebGPU devices compile its WGSL and need nothing.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::GpuError`] naming the missing feature.
    pub fn check_device(device: &wgpu::Device) -> Result<(), CameraError> {
        let backend = device.adapter_info().backend;
        let loads_native_shaders = matches!(
            backend,
            wgpu::Backend::Metal | wgpu::Backend::Vulkan | wgpu::Backend::Dx12
        );
        if loads_native_shaders
            && !device
                .features()
                .contains(wgpu::Features::PASSTHROUGH_SHADERS)
        {
            return Err(CameraError::GpuError(format!(
                "the frame converter loads precompiled shaders on {backend:?}, which needs a device \
                 created with PASSTHROUGH_SHADERS; request `FrameConverter::required_features`"
            )));
        }
        Ok(())
    }

    /// Creates the converter's pipelines on `device`.
    ///
    /// # Panics
    ///
    /// Panics when [`Self::check_device`] rejects `device`.
    #[must_use]
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            rgb: ConvertPass::new(device, &CONVERT_RGB, "convert_rgb"),
            ycbcr420: ConvertPass::new(device, &CONVERT_YCBCR420, "convert_ycbcr420"),
            ycbcr422: ConvertPass::new(device, &CONVERT_YCBCR422, "convert_ycbcr422"),
            params: HashMap::new(),
            output: None,
        }
    }

    /// The upright size of `frame`: its stored size, with width and height
    /// exchanged for the orientations that turn the image a quarter.
    #[must_use]
    pub const fn upright_size(frame: &Frame) -> wgpu::Extent3d {
        let (width, height) = if frame.orientation().swaps_dimensions() {
            (frame.height(), frame.width())
        } else {
            (frame.width(), frame.height())
        };
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        }
    }

    /// Describes a texture that [`Self::encode`] can write `frame` into: the
    /// upright size in [`UPRIGHT_FORMAT`], usable as a storage target,
    /// sampled texture and copy source, with no extra view formats.
    #[must_use]
    pub const fn output_descriptor(frame: &Frame) -> wgpu::TextureDescriptor<'static> {
        wgpu::TextureDescriptor {
            label: Some("waterkit-camera upright frame"),
            size: Self::upright_size(frame),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: UPRIGHT_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                .union(wgpu::TextureUsages::TEXTURE_BINDING)
                .union(wgpu::TextureUsages::COPY_SRC),
            view_formats: &[],
        }
    }

    /// Creates a texture from [`Self::output_descriptor`]. Reuse it for every
    /// frame of the same upright size.
    #[must_use]
    pub fn create_output(device: &wgpu::Device, frame: &Frame) -> wgpu::Texture {
        device.create_texture(&Self::output_descriptor(frame))
    }

    /// Records the conversion of `frame` into `output` on `encoder`.
    ///
    /// # Panics
    ///
    /// Panics when `output` is not an [`UPRIGHT_FORMAT`] storage texture of
    /// [`Self::upright_size`].
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::UnsupportedColor`] when the frame's color
    /// description is not supported by this converter.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
        output: &wgpu::Texture,
    ) -> Result<(), CameraError> {
        Self::check_color(&frame.color())?;
        assert_eq!(
            output.format(),
            UPRIGHT_FORMAT,
            "frame conversion output must be {UPRIGHT_FORMAT:?}"
        );
        assert!(
            output
                .usage()
                .contains(wgpu::TextureUsages::STORAGE_BINDING),
            "frame conversion output must allow STORAGE_BINDING"
        );
        let size = Self::upright_size(frame);
        assert_eq!(
            output.size(),
            size,
            "frame conversion output must have the frame's upright size"
        );

        let (pass, params, sources): (
            &ConvertPass,
            ConvertParams,
            [Option<&wgpu::TextureView>; 2],
        ) = match frame.planes() {
            FramePlanes::Rgb(rgb) => (&self.rgb, ConvertParams::upright(frame), [Some(rgb), None]),
            FramePlanes::YCbCr420 { luma, chroma } => (
                &self.ycbcr420,
                ConvertParams::upright(frame).ycbcr(frame.color(), SampleDepth::of_luma(luma)),
                [Some(luma), Some(chroma)],
            ),
            FramePlanes::YCbCr422 { yuyv } => (
                &self.ycbcr422,
                ConvertParams::upright(frame).ycbcr(frame.color(), SampleDepth::EIGHT_BIT),
                [Some(yuyv), None],
            ),
        };

        let params = self
            .params
            .entry(params.to_bytes())
            .or_insert_with_key(|bytes| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("waterkit-camera frame conversion parameters"),
                    contents: bytes,
                    usage: wgpu::BufferUsages::UNIFORM,
                })
            });
        if self
            .output
            .as_ref()
            .is_none_or(|(texture, _)| texture != output)
        {
            let view = output.create_view(&wgpu::TextureViewDescriptor::default());
            self.output = Some((output.clone(), view));
        }
        let (_, output_view) = self
            .output
            .as_ref()
            .expect("the output view was just cached");
        let mut entries = [
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(output_view),
            },
        ]
        .to_vec();
        entries.extend(sources.into_iter().zip(0..).filter_map(|(view, binding)| {
            view.map(|view| wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            })
        }));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waterkit-camera frame conversion"),
            layout: &pass.layout,
            entries: &entries,
        });

        let mut compute = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("waterkit-camera frame conversion"),
            timestamp_writes: None,
        });
        compute.set_pipeline(&pass.pipeline);
        compute.set_bind_group(0, &bind_group, &[]);
        compute.dispatch_workgroups(
            size.width.div_ceil(WORKGROUP_SIZE),
            size.height.div_ceil(WORKGROUP_SIZE),
            1,
        );
        Ok(())
    }

    /// Converts `frame` into a new upright texture and submits the work.
    ///
    /// A stream of frames is better served by one [`Self::create_output`]
    /// texture reused through [`Self::encode`].
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::UnsupportedColor`] when the frame's color
    /// description is not supported by this converter.
    pub fn convert(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &Frame,
    ) -> Result<wgpu::Texture, CameraError> {
        Self::check_color(&frame.color())?;
        let output = Self::create_output(device, frame);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("waterkit-camera frame conversion"),
        });
        self.encode(device, &mut encoder, frame, &output)?;
        queue.submit([encoder.finish()]);
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use image::{DynamicImage, RgbaImage};
    use waterkit_video_core::{
        ColorPrimaries, MatrixCoefficients, TransferFunction, VideoColorInfo,
    };

    use super::FrameConverter;
    use crate::CameraError;
    use crate::frame::{Frame, Orientation};
    use crate::upload::{CpuPlanes, FrameUploader};
    use crate::wgpu_external_frame::{YcbcrEncoding, YcbcrMatrix, YcbcrRange};

    /// Stored frame size: even, as 4:2:2 needs, and not square, so a wrong
    /// quarter turn changes the output size.
    const WIDTH: u32 = 8;
    const HEIGHT: u32 = 6;
    /// An odd stored size, whose 4:2:0 chroma plane rounds up to cover the
    /// last column and row.
    const ODD: (u32, u32) = (7, 5);

    /// Largest per-channel difference from the f64 reference, in 8-bit codes,
    /// for YCbCr frames: the converter evaluates in f32 with coefficients
    /// rounded to six digits, then rounds once to 8 bits.
    const YCBCR_TOLERANCE: u8 = 1;

    const ORIENTATIONS: [Orientation; 8] = [
        Orientation::Up,
        Orientation::UpMirrored,
        Orientation::Down,
        Orientation::DownMirrored,
        Orientation::LeftMirrored,
        Orientation::Right,
        Orientation::RightMirrored,
        Orientation::Left,
    ];

    fn encodings() -> impl Iterator<Item = YcbcrEncoding> {
        [YcbcrMatrix::Bt601, YcbcrMatrix::Bt709, YcbcrMatrix::Bt2020]
            .into_iter()
            .flat_map(|matrix| {
                [YcbcrRange::Video, YcbcrRange::Full]
                    .into_iter()
                    .map(move |range| YcbcrEncoding { matrix, range })
            })
    }

    fn color_info(encoding: YcbcrEncoding) -> VideoColorInfo {
        let (matrix, range) = crate::color::from_ycbcr_encoding(encoding);
        VideoColorInfo {
            matrix,
            range,
            ..VideoColorInfo::default()
        }
    }

    struct Gpu {
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        converter: FrameConverter,
        uploader: FrameUploader,
    }

    impl Gpu {
        fn new(extra_features: wgpu::Features) -> Self {
            let (device, queue) = crate::test_support::gpu(extra_features);
            Self {
                converter: FrameConverter::new(&device),
                uploader: FrameUploader::new(Arc::clone(&device), Arc::clone(&queue)),
                device,
                queue,
            }
        }

        fn upload(
            &self,
            pixels: &CpuPlanes<'_>,
            color: VideoColorInfo,
            orientation: Orientation,
        ) -> Frame {
            self.upload_sized(pixels, color, (WIDTH, HEIGHT), orientation)
        }

        fn upload_sized(
            &self,
            pixels: &CpuPlanes<'_>,
            color: VideoColorInfo,
            (width, height): (u32, u32),
            orientation: Orientation,
        ) -> Frame {
            self.uploader
                .upload(pixels, color, width, height, orientation, Duration::ZERO)
        }

        /// Converts `frame` and reads the upright result back.
        fn convert(&mut self, frame: &Frame) -> RgbaImage {
            let texture = self
                .converter
                .convert(&self.device, &self.queue, frame)
                .expect("test frame color is supported");
            let pixels = crate::test_support::read_texture(&self.device, &self.queue, &texture);
            RgbaImage::from_raw(texture.width(), texture.height(), pixels).expect("readback size")
        }
    }

    /// The stored image turned upright by the `image` crate's EXIF handling.
    fn upright_reference(stored: RgbaImage, orientation: Orientation) -> RgbaImage {
        let mut image = DynamicImage::ImageRgba8(stored);
        image.apply_orientation(
            image::metadata::Orientation::from_exif(orientation.exif())
                .expect("orientation values are EXIF 1-8"),
        );
        image.into_rgba8()
    }

    fn assert_matches(actual: &RgbaImage, expected: &RgbaImage, tolerance: u8, context: &str) {
        assert_eq!(
            actual.dimensions(),
            expected.dimensions(),
            "{context}: upright size"
        );
        for (x, y, pixel) in actual.enumerate_pixels() {
            let reference = expected.get_pixel(x, y);
            let close = pixel
                .0
                .iter()
                .zip(reference.0)
                .all(|(a, b)| a.abs_diff(b) <= tolerance);
            assert!(
                close,
                "{context}: pixel ({x}, {y}) is {:?}, reference {:?}",
                pixel.0, reference.0
            );
        }
    }

    /// Distinct 8-bit luma per pixel, within video range.
    fn luma(x: u32, y: u32) -> u8 {
        u8::try_from(16 + (x * 37 + y * 53) % 220).expect("below 256")
    }

    /// Distinct Cb and Cr per 2x2 (or 2x1) chroma site, within video range.
    fn chroma(cx: u32, cy: u32) -> [u8; 2] {
        [
            u8::try_from(16 + (cx * 71 + cy * 29) % 225).expect("below 256"),
            u8::try_from(16 + (cx * 43 + cy * 97 + 50) % 225).expect("below 256"),
        ]
    }

    /// The 8-bit Y', Cb, Cr that pixel (x, y) of a 4:2:0 test frame holds.
    fn eight_bit_sample(x: u32, y: u32) -> [u16; 3] {
        let [cb, cr] = chroma(x / 2, y / 2);
        [luma(x, y), cb, cr].map(u16::from)
    }

    fn nv12((width, height): (u32, u32)) -> Vec<u8> {
        let mut data: Vec<u8> = (0..height)
            .flat_map(|y| (0..width).map(move |x| luma(x, y)))
            .collect();
        data.extend(
            (0..height.div_ceil(2))
                .flat_map(|cy| (0..width.div_ceil(2)).flat_map(move |cx| chroma(cx, cy))),
        );
        assert_eq!(data.len(), crate::upload::nv12_len(width, height));
        data
    }

    fn yuyv() -> Vec<u8> {
        (0..HEIGHT)
            .flat_map(|y| {
                (0..WIDTH / 2).flat_map(move |cx| {
                    let [cb, cr] = chroma(cx, y);
                    [luma(cx * 2, y), cb, luma(cx * 2 + 1, y), cr]
                })
            })
            .collect()
    }

    /// 10-bit codes that use their two low bits, so a converter reading them
    /// as 8-bit, or least-significant-aligned, misses the reference.
    fn p010_codes() -> Vec<u16> {
        nv12((WIDTH, HEIGHT))
            .into_iter()
            .enumerate()
            .map(|(index, code)| u16::from(code) * 4 + u16::try_from(index % 4).expect("below 4"))
            .collect()
    }

    /// Kr and Kb of each matrix, as ITU-R BT.601, BT.709 and BT.2020 define
    /// them; the reference derives every coefficient from these.
    const fn luma_weights(matrix: YcbcrMatrix) -> (f64, f64) {
        match matrix {
            YcbcrMatrix::Bt601 => (0.299, 0.114),
            YcbcrMatrix::Bt709 => (0.2126, 0.0722),
            YcbcrMatrix::Bt2020 => (0.2627, 0.0593),
        }
    }

    /// The 8-bit RGB of one `bits`-deep YCbCr sample, computed in f64 from
    /// the recommendations' definitions of range and matrix.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is clamped to 0..=255 and rounded before the cast"
    )]
    fn reference_pixel(
        [y, cb, cr]: [u16; 3],
        bits: u32,
        encoding: YcbcrEncoding,
    ) -> image::Rgba<u8> {
        let max_code = f64::from((1_u32 << bits) - 1);
        let step = f64::from(1_u32 << (bits - 8));
        let (y, cb, cr) = (f64::from(y), f64::from(cb), f64::from(cr));
        let (luma, blue, red) = match encoding.range {
            YcbcrRange::Video => (
                16.0f64.mul_add(-step, y) / (219.0 * step),
                128.0f64.mul_add(-step, cb) / (224.0 * step),
                128.0f64.mul_add(-step, cr) / (224.0 * step),
            ),
            YcbcrRange::Full => (
                y / max_code,
                128.0f64.mul_add(-step, cb) / max_code,
                128.0f64.mul_add(-step, cr) / max_code,
            ),
        };
        let (kr, kb) = luma_weights(encoding.matrix);
        let r = 2.0f64.mul_add(red * (1.0 - kr), luma);
        let b = 2.0f64.mul_add(blue * (1.0 - kb), luma);
        let g = kb.mul_add(-b, kr.mul_add(-r, luma)) / (1.0 - kr - kb);
        let code = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        image::Rgba([code(r), code(g), code(b), 255])
    }

    /// The stored image of a 4:2:0 frame, chroma shared by each 2x2 block.
    fn reference_420(
        (width, height): (u32, u32),
        samples: impl Fn(u32, u32) -> [u16; 3],
        bits: u32,
        encoding: YcbcrEncoding,
    ) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            reference_pixel(samples(x, y), bits, encoding)
        })
    }

    #[test]
    fn rgb_frames_turn_upright_in_every_orientation() {
        let mut gpu = Gpu::new(wgpu::Features::empty());
        let stored = RgbaImage::from_fn(WIDTH, HEIGHT, |x, y| {
            image::Rgba([
                u8::try_from(x * 30 + 5).expect("below 256"),
                u8::try_from(y * 40 + 7).expect("below 256"),
                u8::try_from((x * y * 11 + 3) % 256).expect("below 256"),
                255,
            ])
        });
        let bgra: Vec<u8> = stored
            .pixels()
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
            .collect();
        for orientation in ORIENTATIONS {
            let expected = upright_reference(stored.clone(), orientation);
            for (format, data) in [
                (wgpu::TextureFormat::Rgba8Unorm, stored.as_raw().as_slice()),
                (wgpu::TextureFormat::Bgra8Unorm, bgra.as_slice()),
            ] {
                let frame = gpu.upload(
                    &CpuPlanes::Rgb {
                        format,
                        data,
                        stride: WIDTH * 4,
                    },
                    VideoColorInfo::default(),
                    orientation,
                );
                assert_matches(
                    &gpu.convert(&frame),
                    &expected,
                    0,
                    &format!("{format:?} {orientation:?}"),
                );
            }
        }
    }

    #[test]
    fn ycbcr420_frames_decode_every_encoding_in_every_orientation() {
        let mut gpu = Gpu::new(wgpu::Features::empty());
        for size in [(WIDTH, HEIGHT), ODD] {
            let data = nv12(size);
            for encoding in encodings() {
                let stored = reference_420(size, eight_bit_sample, 8, encoding);
                for orientation in ORIENTATIONS {
                    let frame = gpu.upload_sized(
                        &CpuPlanes::Nv12 { data: &data },
                        color_info(encoding),
                        size,
                        orientation,
                    );
                    assert_matches(
                        &gpu.convert(&frame),
                        &upright_reference(stored.clone(), orientation),
                        YCBCR_TOLERANCE,
                        &format!("NV12 {size:?} {encoding:?} {orientation:?}"),
                    );
                }
            }
        }
    }

    #[test]
    fn ycbcr422_frames_decode_every_encoding_in_every_orientation() {
        let mut gpu = Gpu::new(wgpu::Features::empty());
        let data = yuyv();
        for encoding in encodings() {
            // 4:2:2 shares chroma across a pixel pair on its own row.
            let stored = RgbaImage::from_fn(WIDTH, HEIGHT, |x, y| {
                let [cb, cr] = chroma(x / 2, y);
                reference_pixel([luma(x, y), cb, cr].map(u16::from), 8, encoding)
            });
            for orientation in ORIENTATIONS {
                let frame = gpu.upload(
                    &CpuPlanes::Yuyv { data: &data },
                    color_info(encoding),
                    orientation,
                );
                assert_matches(
                    &gpu.convert(&frame),
                    &upright_reference(stored.clone(), orientation),
                    YCBCR_TOLERANCE,
                    &format!("YUYV {encoding:?} {orientation:?}"),
                );
            }
        }
    }

    /// 16-bit planes need `TEXTURE_FORMAT_16BIT_NORM`; the test GPU asserts
    /// the adapter offers it, so a host without it fails instead of skipping.
    #[test]
    fn ten_bit_ycbcr420_frames_decode_every_encoding() {
        let mut gpu = Gpu::new(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM);
        let codes = p010_codes();
        let bytes: Vec<u8> = codes
            .iter()
            .flat_map(|code| (code << 6).to_le_bytes())
            .collect();
        let (luma_codes, chroma_codes) = codes.split_at((WIDTH * HEIGHT) as usize);
        let sample = |x: u32, y: u32| {
            let chroma = ((y / 2) * WIDTH + (x / 2) * 2) as usize;
            [
                luma_codes[(y * WIDTH + x) as usize],
                chroma_codes[chroma],
                chroma_codes[chroma + 1],
            ]
        };
        for encoding in encodings() {
            let stored = reference_420((WIDTH, HEIGHT), sample, 10, encoding);
            for orientation in [Orientation::Up, Orientation::Right] {
                let frame = gpu.upload(
                    &CpuPlanes::P010 { data: &bytes },
                    color_info(encoding),
                    orientation,
                );
                assert_matches(
                    &gpu.convert(&frame),
                    &upright_reference(stored.clone(), orientation),
                    YCBCR_TOLERANCE,
                    &format!("P010 {encoding:?} {orientation:?}"),
                );
            }
        }
    }

    #[test]
    fn unsupported_color_descriptions_fail_before_conversion() {
        let mut gpu = Gpu::new(wgpu::Features::empty());
        let data = [0_u8, 0, 0, 255].repeat((WIDTH * HEIGHT) as usize);
        let invalid_colors = [
            (
                VideoColorInfo {
                    transfer: TransferFunction::Pq,
                    ..VideoColorInfo::default()
                },
                "transfer",
            ),
            (
                VideoColorInfo {
                    transfer: TransferFunction::Hlg,
                    ..VideoColorInfo::default()
                },
                "transfer",
            ),
            (
                VideoColorInfo {
                    matrix: MatrixCoefficients::Bt2020ConstantLuminance,
                    ..VideoColorInfo::default()
                },
                "matrix",
            ),
            (
                VideoColorInfo {
                    dolby_vision: true,
                    ..VideoColorInfo::default()
                },
                "dolby_vision",
            ),
        ];
        for (color, field) in invalid_colors {
            let Err(CameraError::UnsupportedColor(message)) = FrameConverter::check_color(&color)
            else {
                panic!("expected UnsupportedColor for {color:?}");
            };
            assert!(message.contains(field), "{message}");
            assert!(message.contains(&format!("{color:?}")), "{message}");
            assert!(message.contains("accepts"), "{message}");

            let frame = gpu.upload(
                &CpuPlanes::Rgb {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    data: &data,
                    stride: WIDTH * 4,
                },
                color,
                Orientation::Up,
            );
            let output = FrameConverter::create_output(&gpu.device, &frame);
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("unsupported color test"),
                });
            assert!(matches!(
                gpu.converter
                    .encode(&gpu.device, &mut encoder, &frame, &output),
                Err(CameraError::UnsupportedColor(_))
            ));
            assert!(matches!(
                gpu.converter.convert(&gpu.device, &gpu.queue, &frame),
                Err(CameraError::UnsupportedColor(_))
            ));
        }
    }

    #[test]
    fn primaries_do_not_change_converted_code_values() {
        let mut gpu = Gpu::new(wgpu::Features::empty());
        let data = nv12((WIDTH, HEIGHT));
        let base_color = color_info(YcbcrEncoding {
            matrix: YcbcrMatrix::Bt709,
            range: YcbcrRange::Video,
        });
        let reference = gpu.convert(&gpu.upload(
            &CpuPlanes::Nv12 { data: &data },
            VideoColorInfo {
                primaries: ColorPrimaries::Bt709,
                ..base_color
            },
            Orientation::Up,
        ));
        for primaries in [ColorPrimaries::DisplayP3, ColorPrimaries::Bt2020] {
            let actual = gpu.convert(&gpu.upload(
                &CpuPlanes::Nv12 { data: &data },
                VideoColorInfo {
                    primaries,
                    ..base_color
                },
                Orientation::Up,
            ));
            assert_eq!(actual, reference, "{primaries:?}");
        }
    }
}
