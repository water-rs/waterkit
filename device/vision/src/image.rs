use bytes::Bytes;

use crate::Orientation;

/// An image source accepted by vision requests.
#[derive(Debug)]
pub struct Image {
    pixels: Pixels,
}

/// What a realization reads.
#[derive(Debug, Clone)]
pub enum Pixels {
    /// A GPU texture and the orientation of its stored pixels.
    Texture {
        /// The image texture.
        texture: wgpu::Texture,
        /// How the stored pixels relate to upright.
        orientation: Orientation,
    },
    /// Camera frame planes and their orientation.
    #[cfg(feature = "camera")]
    Frame {
        /// Ref-counted camera plane views.
        planes: waterkit_camera::FramePlanes,
        /// How the stored pixels relate to upright.
        orientation: Orientation,
    },
    /// JPEG, PNG, or HEIF data, decoded with its own orientation metadata by
    /// the serving realization.
    Encoded(Bytes),
}

impl Image {
    /// Creates an image from a single 2D texture.
    ///
    /// # Panics
    ///
    /// Panics when `texture` is not a single 2D image (dimension D2 and one
    /// array layer).
    #[must_use]
    pub fn from_texture(texture: wgpu::Texture, orientation: Orientation) -> Self {
        assert!(
            texture.dimension() == wgpu::TextureDimension::D2
                && texture.size().depth_or_array_layers == 1,
            "vision textures must be a single 2D image"
        );
        Self {
            pixels: Pixels::Texture {
                texture,
                orientation,
            },
        }
    }

    /// Creates an image from encoded JPEG, PNG, or HEIF bytes.
    #[must_use]
    pub const fn from_encoded(bytes: Bytes) -> Self {
        Self {
            pixels: Pixels::Encoded(bytes),
        }
    }

    pub(crate) const fn pixels(&self) -> &Pixels {
        &self.pixels
    }

    pub(crate) fn share(&self) -> Self {
        Self {
            pixels: self.pixels.clone(),
        }
    }

    #[cfg(feature = "camera")]
    fn from_planes(planes: &waterkit_camera::FramePlanes, orientation: Orientation) -> Self {
        Self {
            pixels: Pixels::Frame {
                planes: planes.clone(),
                orientation,
            },
        }
    }
}

#[cfg(feature = "camera")]
impl From<&waterkit_camera::Frame> for Image {
    fn from(frame: &waterkit_camera::Frame) -> Self {
        Self::from_planes(frame.planes(), frame.orientation())
    }
}

#[cfg(test)]
mod tests {
    use super::{Image, Pixels};
    use crate::{Orientation, test_support::gpu};

    fn texture(
        device: &wgpu::Device,
        dimension: wgpu::TextureDimension,
        size: wgpu::Extent3d,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vision image test"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    #[test]
    fn texture_image_keeps_its_handle_and_orientation() {
        let (device, _) = gpu();
        let texture = texture(
            &device,
            wgpu::TextureDimension::D2,
            wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
        );
        let image = Image::from_texture(texture.clone(), Orientation::Right);
        assert!(matches!(
            image.pixels(),
            Pixels::Texture {
                texture: actual,
                orientation: Orientation::Right
            } if actual == &texture
        ));
    }

    #[test]
    #[should_panic(expected = "single 2D image")]
    fn texture_image_rejects_3d_textures() {
        let (device, _) = gpu();
        let texture = texture(
            &device,
            wgpu::TextureDimension::D3,
            wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 2,
            },
        );
        let _ = Image::from_texture(texture, Orientation::Up);
    }

    #[test]
    #[should_panic(expected = "single 2D image")]
    fn texture_image_rejects_array_textures() {
        let (device, _) = gpu();
        let texture = texture(
            &device,
            wgpu::TextureDimension::D2,
            wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 2,
            },
        );
        let _ = Image::from_texture(texture, Orientation::Up);
    }

    #[cfg(feature = "camera")]
    #[test]
    fn frame_images_keep_all_camera_plane_views_and_orientation() {
        use waterkit_camera::{FramePlanes, YcbcrEncoding, YcbcrMatrix, YcbcrRange};

        let (device, _) = gpu();
        let make_plane = || {
            texture(
                &device,
                wgpu::TextureDimension::D2,
                wgpu::Extent3d {
                    width: 2,
                    height: 2,
                    depth_or_array_layers: 1,
                },
            )
        };
        let encoding = YcbcrEncoding {
            matrix: YcbcrMatrix::Bt709,
            range: YcbcrRange::Video,
        };

        let rgb_texture = make_plane();
        let rgb_view = rgb_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let rgb = FramePlanes::Rgb(rgb_view.clone());
        let rgb_image = Image::from_planes(&rgb, Orientation::Right);
        match rgb_image.pixels() {
            Pixels::Frame {
                planes: FramePlanes::Rgb(view),
                orientation,
            } => {
                assert_eq!(*orientation, Orientation::Right);
                assert_eq!(view, &rgb_view);
                assert_eq!(view.texture(), &rgb_texture);
            }
            _ => panic!("RGB planes remain RGB"),
        }

        let luma_texture = make_plane();
        let chroma_texture = make_plane();
        let luma = luma_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let chroma = chroma_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let ycbcr420 = FramePlanes::YCbCr420 {
            luma: luma.clone(),
            chroma: chroma.clone(),
            encoding,
        };
        let ycbcr420_image = Image::from_planes(&ycbcr420, Orientation::Right);
        match ycbcr420_image.pixels() {
            Pixels::Frame {
                planes:
                    FramePlanes::YCbCr420 {
                        luma: actual_luma,
                        chroma: actual_chroma,
                        ..
                    },
                orientation,
            } => {
                assert_eq!(*orientation, Orientation::Right);
                assert_eq!(actual_luma, &luma);
                assert_eq!(actual_luma.texture(), &luma_texture);
                assert_eq!(actual_chroma, &chroma);
                assert_eq!(actual_chroma.texture(), &chroma_texture);
            }
            _ => panic!("YCbCr 4:2:0 planes remain YCbCr 4:2:0"),
        }

        let yuyv_texture = make_plane();
        let yuyv = yuyv_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let ycbcr422 = FramePlanes::YCbCr422 {
            yuyv: yuyv.clone(),
            encoding,
        };
        let ycbcr422_image = Image::from_planes(&ycbcr422, Orientation::Right);
        match ycbcr422_image.pixels() {
            Pixels::Frame {
                planes:
                    FramePlanes::YCbCr422 {
                        yuyv: actual_yuyv, ..
                    },
                orientation,
            } => {
                assert_eq!(*orientation, Orientation::Right);
                assert_eq!(actual_yuyv, &yuyv);
                assert_eq!(actual_yuyv.texture(), &yuyv_texture);
            }
            _ => panic!("YCbCr 4:2:2 planes remain YCbCr 4:2:2"),
        }
    }
}
