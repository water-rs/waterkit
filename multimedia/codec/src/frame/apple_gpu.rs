//! Apple IOSurface-to-wgpu interop without CPU readback.

use super::DecodedPixelLayout;
use metal_wgpu::{
    MTLOrigin, MTLPixelFormat, MTLSize, Texture,
    foreign_types::{ForeignType, ForeignTypeRef},
};
use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use wgpu_hal::api::Metal;

unsafe extern "C" {
    fn CVMetalTextureCacheCreate(
        allocator: *const c_void,
        cache_attributes: *const c_void,
        metal_device: *mut c_void,
        texture_attributes: *const c_void,
        cache_out: *mut *mut c_void,
    ) -> i32;
    fn CVMetalTextureCacheCreateTextureFromImage(
        allocator: *const c_void,
        texture_cache: *mut c_void,
        source_image: *const c_void,
        texture_attributes: *const c_void,
        pixel_format: u64,
        width: usize,
        height: usize,
        plane: usize,
        texture_out: *mut *mut c_void,
    ) -> i32;
    fn CVMetalTextureGetTexture(image: *mut c_void) -> *mut metal_wgpu::MTLTexture;
    fn CFRelease(value: *const c_void);
    fn objc_retain(value: *mut c_void) -> *mut c_void;
}

pub(super) struct AppleFrameUploader {
    texture_cache: NonNull<c_void>,
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
            let command_queue = hal_queue.as_raw().lock();
            CVMetalTextureCacheCreate(
                ptr::null(),
                ptr::null(),
                command_queue.device().as_ptr().cast(),
                ptr::null(),
                &raw mut texture_cache,
            )
        };
        assert_eq!(cache_status, 0, "CVMetalTextureCacheCreate failed");
        let texture_cache =
            NonNull::new(texture_cache).expect("Core Video returned a null Metal texture cache");
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
            self.texture_cache.as_ptr(),
            copy.pixel_buffer,
            0,
            copy.width,
            copy.height,
            y_format,
        );
        let uv_width = (copy.width / 2).max(1);
        let uv_height = (copy.height / 2).max(1);
        let uv_source = create_pixel_buffer_plane_texture(
            self.texture_cache.as_ptr(),
            copy.pixel_buffer,
            1,
            uv_width,
            uv_height,
            uv_format,
        );

        let y_destination = {
            let hal = unsafe { copy.y_target.as_hal::<Metal>() }
                .expect("Apple decoded frame target must use the wgpu Metal backend");
            unsafe { hal.raw_handle() }.to_owned()
        };
        let uv_destination = {
            let hal = unsafe { copy.uv_target.as_hal::<Metal>() }
                .expect("Apple decoded frame target must use the wgpu Metal backend");
            unsafe { hal.raw_handle() }.to_owned()
        };

        let command_buffer = {
            let command_queue = hal_queue.as_raw().lock();
            command_queue.new_command_buffer().to_owned()
        };
        let encoder = command_buffer.new_blit_command_encoder();
        copy_plane(
            encoder,
            &y_source,
            &y_destination,
            copy.width.into(),
            copy.height.into(),
        );
        copy_plane(
            encoder,
            &uv_source,
            &uv_destination,
            uv_width.into(),
            uv_height.into(),
        );
        encoder.end_encoding();
        command_buffer.commit();
    }
}

impl Drop for AppleFrameUploader {
    fn drop(&mut self) {
        unsafe { CFRelease(self.texture_cache.as_ptr()) };
    }
}

fn create_pixel_buffer_plane_texture(
    texture_cache: *mut c_void,
    pixel_buffer: &CFRetained<CVPixelBuffer>,
    plane: usize,
    width: u32,
    height: u32,
    format: MTLPixelFormat,
) -> Texture {
    let mut cv_texture = ptr::null_mut();
    let status = unsafe {
        CVMetalTextureCacheCreateTextureFromImage(
            ptr::null(),
            texture_cache,
            CFRetained::as_ptr(pixel_buffer).as_ptr().cast(),
            ptr::null(),
            format as u64,
            width as usize,
            height as usize,
            plane,
            &raw mut cv_texture,
        )
    };
    assert_eq!(
        status, 0,
        "Core Video could not map the decoded pixel-buffer plane"
    );
    assert!(
        !cv_texture.is_null(),
        "Core Video returned a null Metal texture"
    );
    let raw = unsafe { CVMetalTextureGetTexture(cv_texture) };
    assert!(!raw.is_null(), "Metal rejected the decoded IOSurface plane");
    unsafe {
        objc_retain(raw.cast());
        CFRelease(cv_texture);
    }
    unsafe { Texture::from_ptr(raw) }
}

fn copy_plane(
    encoder: &metal_wgpu::BlitCommandEncoderRef,
    source: &metal_wgpu::TextureRef,
    destination: &metal_wgpu::TextureRef,
    width: u64,
    height: u64,
) {
    encoder.copy_from_texture(
        source,
        0,
        0,
        MTLOrigin { x: 0, y: 0, z: 0 },
        MTLSize {
            width,
            height,
            depth: 1,
        },
        destination,
        0,
        0,
        MTLOrigin { x: 0, y: 0, z: 0 },
    );
}
