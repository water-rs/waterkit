//! Apple `IOSurface`-to-wgpu interop without CPU readback.

use crate::DecodedPixelLayout;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer, kCVReturnSuccess,
};
use objc2_metal::{MTLPixelFormat, MTLTexture};
use std::ptr::{self, NonNull};
use wgpu_hal::api::Metal;

use super::GpuFrame;

/// Imports decoded `IOSurface` planes as wgpu textures in place.
///
/// The `CVMetalTextureCache` hands out `MTLTexture`s that view the decoded
/// frame's `IOSurface` planes; importing them with
/// [`wgpu::Device::create_texture_from_hal`] wraps that storage without
/// copying a pixel.
pub(super) struct AppleFrameUploader {
    texture_cache: CFRetained<CVMetalTextureCache>,
}

impl std::fmt::Debug for AppleFrameUploader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleFrameUploader").finish_non_exhaustive()
    }
}

impl AppleFrameUploader {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let hal_device = unsafe { device.as_hal::<Metal>() }
            .expect("Apple decoded frames require the wgpu Metal backend");
        let mut texture_cache = ptr::null_mut();
        let cache_status = unsafe {
            let metal_device = hal_device.raw_device();
            CVMetalTextureCache::create(
                None,
                None,
                metal_device,
                None,
                NonNull::from(&mut texture_cache),
            )
        };
        assert_eq!(
            cache_status, kCVReturnSuccess,
            "CVMetalTextureCacheCreate failed"
        );
        let texture_cache =
            NonNull::new(texture_cache).expect("Core Video returned a null Metal texture cache");
        let texture_cache = unsafe { CFRetained::from_raw(texture_cache) };
        Self { texture_cache }
    }

    /// Imports `pixel_buffer`'s luma and chroma planes as wgpu textures that
    /// share the decoded frame's `IOSurface` storage.
    pub(super) fn import_frame(
        &self,
        device: &wgpu::Device,
        pixel_buffer: &CFRetained<CVPixelBuffer>,
        width: u32,
        height: u32,
        layout: DecodedPixelLayout,
        timestamp_ns: u64,
    ) -> GpuFrame {
        let (y_format, uv_format) = match layout {
            DecodedPixelLayout::Nv12 => (MTLPixelFormat::R8Uint, MTLPixelFormat::RG8Uint),
            DecodedPixelLayout::P010 => (MTLPixelFormat::R16Uint, MTLPixelFormat::RG16Uint),
        };
        let (y_wgpu_format, uv_wgpu_format) = match layout {
            DecodedPixelLayout::Nv12 => (wgpu::TextureFormat::R8Uint, wgpu::TextureFormat::Rg8Uint),
            DecodedPixelLayout::P010 => {
                (wgpu::TextureFormat::R16Uint, wgpu::TextureFormat::Rg16Uint)
            }
        };

        let y_raw = create_pixel_buffer_plane_texture(
            &self.texture_cache,
            pixel_buffer,
            0,
            width,
            height,
            y_format,
        );
        let uv_raw = create_pixel_buffer_plane_texture(
            &self.texture_cache,
            pixel_buffer,
            1,
            width.div_ceil(2),
            height.div_ceil(2),
            uv_format,
        );

        GpuFrame::imported(
            import_texture(device, y_raw, y_wgpu_format),
            import_texture(device, uv_raw, uv_wgpu_format),
            width,
            height,
            layout,
            timestamp_ns,
        )
    }
}

/// Wraps a raw `MTLTexture` in a `wgpu::Texture` that can sample the plane.
///
/// The returned texture retains `raw` until it is dropped; the plane's
/// `MTLTexture` keeps the decoded frame's `IOSurface` alive for the texture's
/// lifetime, so no additional frame retention is needed.
fn import_texture(
    device: &wgpu::Device,
    raw: Retained<ProtocolObject<dyn MTLTexture>>,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    let extent = wgpu::Extent3d {
        width: u32::try_from(raw.width()).expect("plane width fits u32"),
        height: u32::try_from(raw.height()).expect("plane height fits u32"),
        depth_or_array_layers: 1,
    };
    let hal_texture = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            raw,
            format,
            objc2_metal::MTLTextureType::Type2D,
            1,
            1,
            extent.into(),
            None,
        )
    };
    unsafe {
        device.create_texture_from_hal::<wgpu::hal::metal::Api>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("decoded frame plane"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::wgt::TextureUses::RESOURCE,
        )
    }
}

fn create_pixel_buffer_plane_texture(
    texture_cache: &CVMetalTextureCache,
    pixel_buffer: &CFRetained<CVPixelBuffer>,
    plane: usize,
    width: u32,
    height: u32,
    format: MTLPixelFormat,
) -> Retained<ProtocolObject<dyn MTLTexture>> {
    let mut cv_texture = ptr::null_mut();
    let status = unsafe {
        CVMetalTextureCache::create_texture_from_image(
            None,
            texture_cache,
            pixel_buffer,
            None,
            format,
            width as usize,
            height as usize,
            plane,
            NonNull::from(&mut cv_texture),
        )
    };
    assert_eq!(
        status, kCVReturnSuccess,
        "Core Video could not map the decoded pixel-buffer plane"
    );
    let cv_texture = NonNull::<CVMetalTexture>::new(cv_texture)
        .expect("Core Video returned a null Metal texture");
    let cv_texture = unsafe { CFRetained::from_raw(cv_texture) };
    CVMetalTextureGetTexture(&cv_texture).expect("Metal rejected the decoded `IOSurface` plane")
}
