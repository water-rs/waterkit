//! Recycled GPU storage for frames whose pixels arrive in CPU memory.
//!
//! A [`FramePool`] uploads each frame's planes into textures it reuses: a
//! frame keeps its textures through a [`PoolLease`], and dropping the frame
//! hands them back over a channel for the next upload of the same layout. The
//! pool therefore holds as many texture sets as frames are alive at once,
//! instead of allocating a set per frame.

use std::sync::Arc;
use std::time::Duration;

#[cfg(any(target_os = "windows", target_os = "linux", test))]
use crate::frame::YCbCrEncoding;
use crate::frame::{Frame, FramePlanes, FrameStorage, Orientation};

/// One frame's pixels in CPU memory, in the layout the platform delivered.
pub enum CpuPlanes<'a> {
    /// Interleaved 8-bit pixels in `format` (`Rgba8Unorm` or `Bgra8Unorm`),
    /// rows `stride` bytes apart.
    Rgb {
        format: wgpu::TextureFormat,
        data: &'a [u8],
        stride: u32,
    },
    /// NV12: the full-size luma plane, then the half-size interleaved chroma
    /// plane, each tightly packed.
    #[cfg(any(target_os = "windows", target_os = "linux", test))]
    Nv12 {
        data: &'a [u8],
        encoding: YCbCrEncoding,
    },
    /// Packed YUYV 4:2:2, tightly packed rows.
    #[cfg(any(target_os = "windows", target_os = "linux", test))]
    Yuyv {
        data: &'a [u8],
        encoding: YCbCrEncoding,
    },
    /// P010: NV12's layout with 16-bit little-endian samples holding 10-bit
    /// codes in their top bits. Only the converter tests upload it, on the
    /// Apple GPUs that all offer 16-bit planes; 10-bit platform frames arrive
    /// as imported surfaces, not through CPU memory.
    #[cfg(all(test, target_vendor = "apple"))]
    P010 {
        data: &'a [u8],
        encoding: YCbCrEncoding,
    },
}

/// One plane to upload: its texture shape and the bytes that fill it.
struct PlaneUpload<'a> {
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    stride: u32,
    data: &'a [u8],
}

/// Textures a live frame holds; dropping it returns them to the pool.
#[derive(Debug)]
pub struct PoolLease {
    textures: Vec<wgpu::Texture>,
    free: async_channel::Sender<Vec<wgpu::Texture>>,
}

impl Drop for PoolLease {
    fn drop(&mut self) {
        // A closed channel means the pool is gone; the textures drop with the
        // lease.
        let _ = self.free.try_send(std::mem::take(&mut self.textures));
    }
}

/// Uploads CPU frames into recycled textures on one device.
#[derive(Debug)]
pub struct FramePool {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    free_tx: async_channel::Sender<Vec<wgpu::Texture>>,
    free_rx: async_channel::Receiver<Vec<wgpu::Texture>>,
}

impl FramePool {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        let (free_tx, free_rx) = async_channel::unbounded();
        Self {
            device,
            queue,
            free_tx,
            free_rx,
        }
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
        match *pixels {
            CpuPlanes::Rgb {
                format,
                data,
                stride,
            } => {
                let textures = self.write(&[PlaneUpload {
                    format,
                    width,
                    height,
                    stride,
                    data,
                }]);
                let planes = FramePlanes::Rgb(view(&textures[0]));
                self.frame(planes, textures, width, height, orientation, timestamp)
            }
            #[cfg(any(target_os = "windows", target_os = "linux", test))]
            CpuPlanes::Nv12 { data, encoding } => {
                let luma_len = width as usize * height as usize;
                let (luma, chroma) = data.split_at(luma_len);
                let textures = self.write(&[
                    PlaneUpload {
                        format: wgpu::TextureFormat::R8Unorm,
                        width,
                        height,
                        stride: width,
                        data: luma,
                    },
                    PlaneUpload {
                        format: wgpu::TextureFormat::Rg8Unorm,
                        width: width / 2,
                        height: height / 2,
                        stride: width,
                        data: chroma,
                    },
                ]);
                let planes = FramePlanes::YCbCr420 {
                    luma: view(&textures[0]),
                    chroma: view(&textures[1]),
                    encoding,
                };
                self.frame(planes, textures, width, height, orientation, timestamp)
            }
            #[cfg(any(target_os = "windows", target_os = "linux", test))]
            CpuPlanes::Yuyv { data, encoding } => {
                let textures = self.write(&[PlaneUpload {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    width: width / 2,
                    height,
                    stride: width * 2,
                    data,
                }]);
                let planes = FramePlanes::YCbCr422 {
                    yuyv: view(&textures[0]),
                    encoding,
                };
                self.frame(planes, textures, width, height, orientation, timestamp)
            }
            #[cfg(all(test, target_vendor = "apple"))]
            CpuPlanes::P010 { data, encoding } => {
                let (luma, chroma) = data.split_at(width as usize * height as usize * 2);
                let textures = self.write(&[
                    PlaneUpload {
                        format: wgpu::TextureFormat::R16Unorm,
                        width,
                        height,
                        stride: width * 2,
                        data: luma,
                    },
                    PlaneUpload {
                        format: wgpu::TextureFormat::Rg16Unorm,
                        width: width / 2,
                        height: height / 2,
                        stride: width * 2,
                        data: chroma,
                    },
                ]);
                let planes = FramePlanes::YCbCr420 {
                    luma: view(&textures[0]),
                    chroma: view(&textures[1]),
                    encoding,
                };
                self.frame(planes, textures, width, height, orientation, timestamp)
            }
        }
    }

    fn frame(
        &self,
        planes: FramePlanes,
        textures: Vec<wgpu::Texture>,
        width: u32,
        height: u32,
        orientation: Orientation,
        timestamp: Duration,
    ) -> Frame {
        let lease = PoolLease {
            textures,
            free: self.free_tx.clone(),
        };
        Frame::new(
            planes,
            FrameStorage::Pooled { _lease: lease },
            width,
            height,
            orientation,
            timestamp,
        )
    }

    /// Writes `planes` into a free texture set of the same shape, or into a
    /// new set when none is free. Free sets of another shape belong to a
    /// layout the camera no longer delivers and are released.
    fn write(&self, planes: &[PlaneUpload<'_>]) -> Vec<wgpu::Texture> {
        let textures = std::iter::from_fn(|| self.free_rx.try_recv().ok())
            .find(|set| fits(set, planes))
            .unwrap_or_else(|| self.allocate(planes));
        for (texture, plane) in textures.iter().zip(planes) {
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
        }
        textures
    }

    fn allocate(&self, planes: &[PlaneUpload<'_>]) -> Vec<wgpu::Texture> {
        planes
            .iter()
            .map(|plane| {
                self.device.create_texture(&wgpu::TextureDescriptor {
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
                })
            })
            .collect()
    }
}

fn fits(set: &[wgpu::Texture], planes: &[PlaneUpload<'_>]) -> bool {
    set.len() == planes.len()
        && set.iter().zip(planes).all(|(texture, plane)| {
            texture.format() == plane.format
                && texture.width() == plane.width
                && texture.height() == plane.height
        })
}

fn view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}
