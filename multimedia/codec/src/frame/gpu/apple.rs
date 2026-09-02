//! Apple IOSurface-to-wgpu interop without CPU readback.

use crate::DecodedPixelLayout;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer, kCVReturnSuccess,
};
use objc2_metal::{
    MTLBlitCommandEncoder, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLOrigin,
    MTLPixelFormat, MTLSize, MTLTexture,
};
use std::ptr::{self, NonNull};
use wgpu_hal::api::Metal;

pub(super) struct AppleFrameUploader {
    texture_cache: CFRetained<CVMetalTextureCache>,
}

#[derive(Clone, Copy)]
pub(super) struct SurfacePlaneCopy<'a> {
    pub(super) pixel_buffer: &'a CFRetained<CVPixelBuffer>,
    pub(super) y_target: &'a wgpu::Texture,
    pub(super) uv_target: &'a wgpu::Texture,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) layout: DecodedPixelLayout,
}

impl std::fmt::Debug for AppleFrameUploader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleFrameUploader").finish_non_exhaustive()
    }
}

impl AppleFrameUploader {
    pub(super) fn new(queue: &wgpu::Queue) -> Self {
        let hal_queue = unsafe { queue.as_hal::<Metal>() }
            .expect("Apple decoded frames require the wgpu Metal backend");
        let mut texture_cache = ptr::null_mut();
        let cache_status = unsafe {
            let metal_device = hal_queue.as_raw().device();
            CVMetalTextureCache::create(
                None,
                None,
                &metal_device,
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

    pub(super) fn copy_surface_planes(&self, queue: &wgpu::Queue, copy: SurfacePlaneCopy<'_>) {
        let (y_format, uv_format) = match copy.layout {
            DecodedPixelLayout::Nv12 => (MTLPixelFormat::R8Unorm, MTLPixelFormat::RG8Unorm),
            DecodedPixelLayout::P010 => (MTLPixelFormat::R16Unorm, MTLPixelFormat::RG16Unorm),
        };

        let hal_queue = unsafe { queue.as_hal::<Metal>() }
            .expect("Apple decoded frames require the wgpu Metal backend");
        let y_source = create_pixel_buffer_plane_texture(
            &self.texture_cache,
            copy.pixel_buffer,
            0,
            copy.width,
            copy.height,
            y_format,
        );
        let uv_width = (copy.width / 2).max(1);
        let uv_height = (copy.height / 2).max(1);
        let uv_source = create_pixel_buffer_plane_texture(
            &self.texture_cache,
            copy.pixel_buffer,
            1,
            uv_width,
            uv_height,
            uv_format,
        );

        let y_destination = retain_metal_texture(copy.y_target);
        let uv_destination = retain_metal_texture(copy.uv_target);

        let command_buffer = hal_queue
            .as_raw()
            .commandBuffer()
            .expect("Metal command queue failed to create a command buffer");
        let encoder = command_buffer
            .blitCommandEncoder()
            .expect("Metal command buffer failed to create a blit encoder");
        copy_plane(
            &encoder,
            &y_source,
            &y_destination,
            copy.width.into(),
            copy.height.into(),
        );
        copy_plane(
            &encoder,
            &uv_source,
            &uv_destination,
            uv_width.into(),
            uv_height.into(),
        );
        encoder.endEncoding();
        command_buffer.commit();
    }
}

fn retain_metal_texture(texture: &wgpu::Texture) -> Retained<ProtocolObject<dyn MTLTexture>> {
    // Keep each HAL guard scoped to one texture; wgpu forbids recursively
    // acquiring the internal texture lock while another guard is alive.
    let texture = unsafe { texture.as_hal::<Metal>() }
        .expect("Apple decoded frame target must use the wgpu Metal backend");
    let raw = texture.raw_handle();
    let raw = std::ptr::from_ref(raw).cast_mut();
    unsafe { Retained::retain(raw) }.expect("wgpu returned a null Metal texture")
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
    CVMetalTextureGetTexture(&cv_texture).expect("Metal rejected the decoded IOSurface plane")
}

fn copy_plane(
    encoder: &ProtocolObject<dyn MTLBlitCommandEncoder>,
    source: &ProtocolObject<dyn MTLTexture>,
    destination: &ProtocolObject<dyn MTLTexture>,
    width: u64,
    height: u64,
) {
    unsafe {
        encoder.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
        source,
        0,
        0,
        MTLOrigin { x: 0, y: 0, z: 0 },
        MTLSize {
            width: usize::try_from(width).expect("Metal plane width must fit usize"),
            height: usize::try_from(height).expect("Metal plane height must fit usize"),
            depth: 1,
        },
        destination,
        0,
        0,
        MTLOrigin { x: 0, y: 0, z: 0 },
        );
    }
}
