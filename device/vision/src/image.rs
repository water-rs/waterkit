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
    /// Camera frame planes, their colour description, and their orientation.
    #[cfg(feature = "camera")]
    Frame {
        /// Ref-counted camera plane views.
        planes: waterkit_camera::FramePlanes,
        /// How the plane samples decode to colour.
        color: waterkit_camera::VideoColorInfo,
        /// How the stored pixels relate to upright.
        orientation: Orientation,
        /// The captured `CVPixelBuffer`, when the frame carries one.
        ///
        /// Native realizations serve straight from it; frames uploaded from
        /// memory carry no buffer and prepare through their planes.
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        pixel_buffer: Option<crate::sys::PixelBuffer>,
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

    /// Mutable access for tests that need to construct a `Pixels` variant no
    /// public constructor produces, like a frame carrying its pixel buffer.
    #[cfg(all(
        test,
        feature = "camera",
        feature = "barcode",
        any(target_os = "ios", target_os = "macos")
    ))]
    pub(crate) const fn pixels_mut(&mut self) -> &mut Pixels {
        &mut self.pixels
    }

    #[cfg(feature = "camera")]
    pub(crate) fn from_planes(
        planes: &waterkit_camera::FramePlanes,
        color: waterkit_camera::VideoColorInfo,
        orientation: Orientation,
    ) -> Self {
        Self {
            pixels: Pixels::Frame {
                planes: planes.clone(),
                color,
                orientation,
                #[cfg(any(target_os = "ios", target_os = "macos"))]
                pixel_buffer: None,
            },
        }
    }
}

#[cfg(feature = "camera")]
impl From<&waterkit_camera::Frame> for Image {
    fn from(frame: &waterkit_camera::Frame) -> Self {
        let mut image = Self::from_planes(frame.planes(), frame.color(), frame.orientation());
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        if let Pixels::Frame { pixel_buffer, .. } = &mut image.pixels {
            *pixel_buffer = frame.pixel_buffer().map(crate::sys::PixelBuffer);
        }
        image
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
    fn frame_images_keep_all_camera_plane_views_color_and_orientation() {
        use waterkit_camera::{
            ColorPrimaries, ColorRange, FramePlanes, MatrixCoefficients, TransferFunction,
            VideoColorInfo,
        };

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
        let color = VideoColorInfo {
            matrix: MatrixCoefficients::Bt2020NonConstantLuminance,
            primaries: ColorPrimaries::Bt2020,
            transfer: TransferFunction::Hlg,
            range: ColorRange::Limited,
            content_light_level: None,
            dolby_vision: false,
        };

        let rgb_texture = make_plane();
        let rgb_view = rgb_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let rgb = FramePlanes::Rgb(rgb_view.clone());
        let rgb_image = Image::from_planes(&rgb, color, Orientation::Right);
        match rgb_image.pixels() {
            Pixels::Frame {
                planes: FramePlanes::Rgb(view),
                color: actual_color,
                orientation,
                ..
            } => {
                assert_eq!(*actual_color, color);
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
        };
        let ycbcr420_image = Image::from_planes(&ycbcr420, color, Orientation::Right);
        match ycbcr420_image.pixels() {
            Pixels::Frame {
                planes:
                    FramePlanes::YCbCr420 {
                        luma: actual_luma,
                        chroma: actual_chroma,
                    },
                color: actual_color,
                orientation,
                ..
            } => {
                assert_eq!(*actual_color, color);
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
        let ycbcr422 = FramePlanes::YCbCr422 { yuyv: yuyv.clone() };
        let ycbcr422_image = Image::from_planes(&ycbcr422, color, Orientation::Right);
        match ycbcr422_image.pixels() {
            Pixels::Frame {
                planes: FramePlanes::YCbCr422 { yuyv: actual_yuyv },
                color: actual_color,
                orientation,
                ..
            } => {
                assert_eq!(*actual_color, color);
                assert_eq!(*orientation, Orientation::Right);
                assert_eq!(actual_yuyv, &yuyv);
                assert_eq!(actual_yuyv.texture(), &yuyv_texture);
            }
            _ => panic!("YCbCr 4:2:2 planes remain YCbCr 4:2:2"),
        }
    }
}
