//! GPU textures for frames whose pixels arrive in CPU memory.
//!
//! Each upload writes a frame's planes into textures created for that frame.
//! The textures are not recycled: a consumer may record work that samples a
//! frame, drop the frame, and submit the work afterwards, and only `wgpu`
//! knows when that work has finished. A texture dropped with the frame is
//! freed by `wgpu` exactly then, while a recycled one would be overwritten
//! by the next upload, which `wgpu` orders ahead of that later submission.
//! `wgpu`'s safe API offers no signal for that moment that a pool could wait
//! on.

use std::sync::Arc;
use std::time::Duration;

#[cfg(any(target_os = "windows", target_os = "linux", test))]
use crate::YcbcrEncoding;
use crate::frame::{Frame, FramePlanes, Orientation};

/// One frame's pixels in CPU memory, in the layout the platform delivered.
pub enum CpuPlanes<'a> {
    /// Interleaved 8-bit pixels in `format` (`Rgba8Unorm` or `Bgra8Unorm`),
    /// rows `stride` bytes apart.
    Rgb {
        format: wgpu::TextureFormat,
        data: &'a [u8],
        stride: u32,
    },
    /// NV12: the full-size luma plane, then the interleaved chroma plane of
    /// `ceil(width / 2)` x `ceil(height / 2)` Cb/Cr pairs, each tightly packed.
    #[cfg(any(target_os = "windows", target_os = "linux", test))]
    Nv12 {
        data: &'a [u8],
        encoding: YcbcrEncoding,
    },
    /// Packed YUYV 4:2:2 of an even width, tightly packed rows.
    #[cfg(any(target_os = "windows", target_os = "linux", test))]
    Yuyv {
        data: &'a [u8],
        encoding: YcbcrEncoding,
    },
    /// P010: NV12's layout with 16-bit little-endian samples holding 10-bit
    /// codes in their top bits. Only the converter tests upload it, on the
    /// Apple GPUs that all offer 16-bit planes; 10-bit platform frames arrive
    /// as imported surfaces, not through CPU memory.
    #[cfg(all(test, target_vendor = "apple"))]
    P010 {
        data: &'a [u8],
        encoding: YcbcrEncoding,
    },
}

/// Bytes of a tightly packed NV12 frame of `width` x `height` pixels.
#[cfg(any(target_os = "windows", target_os = "linux", test))]
pub const fn nv12_len(width: u32, height: u32) -> usize {
    let (chroma_width, chroma_height) = chroma_extent(width, height);
    width as usize * height as usize + 2 * chroma_width as usize * chroma_height as usize
}

/// The chroma plane's extent for a 4:2:0 frame, rounded up for odd sizes.
const fn chroma_extent(width: u32, height: u32) -> (u32, u32) {
    (width.div_ceil(2), height.div_ceil(2))
}

/// One plane to upload: its texture shape and the bytes that fill it.
struct PlaneUpload<'a> {
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    stride: u32,
    data: &'a [u8],
}

/// Uploads CPU frames into textures on one device.
#[derive(Debug)]
pub struct FrameUploader {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

impl FrameUploader {
    pub const fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self { device, queue }
    }

    /// Uploads one frame of `width` x `height` pixels.
    pub fn upload(
        &self,
        pixels: &CpuPlanes<'_>,
        width: u32,
        height: u32,
        orientation: Orientation,
        timestamp: Duration,
    ) -> Frame {
        let planes = match *pixels {
            CpuPlanes::Rgb {
                format,
                data,
                stride,
            } => {
                let [rgb] = self.write([PlaneUpload {
                    format,
                    width,
                    height,
                    stride,
                    data,
                }]);
                FramePlanes::Rgb(view(&rgb))
            }
            #[cfg(any(target_os = "windows", target_os = "linux", test))]
            CpuPlanes::Nv12 { data, encoding } => {
                let (chroma_width, chroma_height) = chroma_extent(width, height);
                let (luma, chroma) = data.split_at(width as usize * height as usize);
                let [luma, chroma] = self.write([
                    PlaneUpload {
                        format: wgpu::TextureFormat::R8Unorm,
                        width,
                        height,
                        stride: width,
                        data: luma,
                    },
                    PlaneUpload {
                        format: wgpu::TextureFormat::Rg8Unorm,
                        width: chroma_width,
                        height: chroma_height,
                        stride: chroma_width * 2,
                        data: chroma,
                    },
                ]);
                FramePlanes::YCbCr420 {
                    luma: view(&luma),
                    chroma: view(&chroma),
                    encoding,
                }
            }
            #[cfg(any(target_os = "windows", target_os = "linux", test))]
            CpuPlanes::Yuyv { data, encoding } => {
                let [yuyv] = self.write([PlaneUpload {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    width: width / 2,
                    height,
                    stride: width * 2,
                    data,
                }]);
                FramePlanes::YCbCr422 {
                    yuyv: view(&yuyv),
                    encoding,
                }
            }
            #[cfg(all(test, target_vendor = "apple"))]
            CpuPlanes::P010 { data, encoding } => {
                let (chroma_width, chroma_height) = chroma_extent(width, height);
                let (luma, chroma) = data.split_at(width as usize * height as usize * 2);
                let [luma, chroma] = self.write([
                    PlaneUpload {
                        format: wgpu::TextureFormat::R16Unorm,
                        width,
                        height,
                        stride: width * 2,
                        data: luma,
                    },
                    PlaneUpload {
                        format: wgpu::TextureFormat::Rg16Unorm,
                        width: chroma_width,
                        height: chroma_height,
                        stride: chroma_width * 4,
                        data: chroma,
                    },
                ]);
                FramePlanes::YCbCr420 {
                    luma: view(&luma),
                    chroma: view(&chroma),
                    encoding,
                }
            }
        };
        Frame::new(
            planes,
            width,
            height,
            orientation,
            timestamp,
        )
    }

    /// Creates a texture for each plane and writes the plane into it.
    fn write<const N: usize>(&self, planes: [PlaneUpload<'_>; N]) -> [wgpu::Texture; N] {
        planes.map(|plane| {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("waterkit-camera frame plane"),
                size: wgpu::Extent3d {
                    width: plane.width,
                    height: plane.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: plane.format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            self.queue.write_texture(
                texture.as_image_copy(),
                plane.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(plane.stride),
                    rows_per_image: Some(plane.height),
                },
                texture.size(),
            );
            texture
        })
    }
}

fn view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}
