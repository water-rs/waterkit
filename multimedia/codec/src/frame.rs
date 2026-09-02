//! Decoded video frame in its native bi-planar YUV layout.
//!
//! The types here are backend-independent: they describe decoded pixels held in
//! native decoder storage or CPU memory. Uploading them to `wgpu` textures lives
//! in the `gpu` submodule behind the `gpu` feature.

#[cfg(target_vendor = "apple")]
use {objc2_core_foundation::CFRetained, objc2_io_surface::IOSurfaceRef, std::ptr};

#[cfg(all(target_vendor = "apple", feature = "gpu"))]
use objc2_core_video::CVPixelBuffer;

#[cfg(feature = "gpu")]
mod gpu;

#[cfg(feature = "gpu")]
pub use gpu::{DecodedFrameUploader, GpuFrame, LinearRgbaConverter};

/// Decoded frame - opaque type hiding platform details.
///
/// This represents a decoded video frame that has not yet been converted to GPU textures.
/// With the `gpu` feature, `to_gpu_frame` creates GPU textures on the user's device;
/// otherwise read the pixels out with [`copy_to_buffer`](Self::copy_to_buffer).
pub struct DecodedFrame {
    inner: DecodedFrameInner,
}

// SAFETY: Apple IOSurfaces are explicitly cross-thread shareable allocations and
// the retained CF ownership keeps their storage alive until the frame is dropped.
#[cfg(target_vendor = "apple")]
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for DecodedFrame {}

/// Native decoded YUV storage layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedPixelLayout {
    /// 8-bit bi-planar 4:2:0 video-range YUV (`420v`).
    Nv12,
    /// 10-bit bi-planar 4:2:0 video-range YUV in 16-bit lanes (`x420`).
    P010,
}

impl DecodedPixelLayout {
    /// Number of bytes required for a tightly packed frame.
    #[must_use]
    pub const fn packed_len(self, width: u32, height: u32) -> usize {
        let width = width as usize;
        let height = height as usize;
        let luma_samples = width * height;
        let chroma_samples = width.div_ceil(2) * height.div_ceil(2) * 2;
        let samples = luma_samples + chroma_samples;
        match self {
            Self::Nv12 => samples,
            Self::P010 => samples * 2,
        }
    }

    /// Number of bytes in one luma or interleaved chroma row.
    #[must_use]
    pub const fn bytes_per_row(self, width: u32) -> usize {
        match self {
            Self::Nv12 => width as usize,
            Self::P010 => width as usize * 2,
        }
    }
}

/// Private enum holding platform-specific frame data.
enum DecodedFrameInner {
    /// Hardware-decoded frame backed by `IOSurface` (Apple only).
    #[cfg(target_vendor = "apple")]
    Hardware {
        surface: CFRetained<IOSurfaceRef>,
        /// Retained Core Video buffer used for Metal texture-cache interop.
        #[cfg(feature = "gpu")]
        pixel_buffer: CFRetained<CVPixelBuffer>,
        width: u32,
        height: u32,
        timestamp_ns: u64,
        layout: DecodedPixelLayout,
    },
    /// Software-decoded frame with NV12 data.
    /// Available on non-Apple platforms, or desktop Apple platforms with software-fallback.
    #[cfg(waterkit_software_frames)]
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            dead_code,
            reason = "WebCodecs frame construction is owned by the browser adapter"
        )
    )]
    Software {
        data: Vec<u8>,
        width: u32,
        height: u32,
        timestamp_ns: u64,
        layout: DecodedPixelLayout,
    },
}

impl std::fmt::Debug for DecodedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedFrame")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("timestamp", &self.timestamp())
            .finish_non_exhaustive()
    }
}

impl DecodedFrame {
    /// Create a decoded frame from a hardware `IOSurface` output (Apple only).
    #[cfg(target_vendor = "apple")]
    pub(crate) fn from_iosurface(frame: &crate::sys::apple::IOSurfaceFrame) -> Self {
        Self {
            inner: DecodedFrameInner::Hardware {
                surface: frame.surface.clone(),
                #[cfg(feature = "gpu")]
                pixel_buffer: frame.pixel_buffer.clone(),
                width: frame.width,
                height: frame.height,
                timestamp_ns: frame.timestamp_ns,
                layout: frame.layout,
            },
        }
    }

    /// Create a decoded frame from tightly packed bi-planar software output.
    #[cfg(waterkit_software_frames)]
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            dead_code,
            reason = "WebCodecs frame construction is owned by the browser adapter"
        )
    )]
    pub(crate) const fn from_biplanar_data(
        data: Vec<u8>,
        width: u32,
        height: u32,
        timestamp_ns: u64,
        layout: DecodedPixelLayout,
    ) -> Self {
        Self {
            inner: DecodedFrameInner::Software {
                data,
                width,
                height,
                timestamp_ns,
                layout,
            },
        }
    }

    /// Returns the native decoded pixel layout.
    #[must_use]
    pub const fn pixel_layout(&self) -> DecodedPixelLayout {
        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { layout, .. } => *layout,
            #[cfg(waterkit_software_frames)]
            DecodedFrameInner::Software { layout, .. } => *layout,
        }
    }

    /// Get the frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { width, .. } => *width,
            #[cfg(waterkit_software_frames)]
            DecodedFrameInner::Software { width, .. } => *width,
        }
    }

    /// Get the frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { height, .. } => *height,
            #[cfg(waterkit_software_frames)]
            DecodedFrameInner::Software { height, .. } => *height,
        }
    }

    /// Returns the presentation timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> std::time::Duration {
        let ns = match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware { timestamp_ns, .. } => *timestamp_ns,
            #[cfg(waterkit_software_frames)]
            DecodedFrameInner::Software { timestamp_ns, .. } => *timestamp_ns,
        };
        std::time::Duration::from_nanos(ns)
    }

    /// Copy the native bi-planar data to a provided buffer slice.
    ///
    /// Returns the number of bytes written. The buffer must be large enough for
    /// [`DecodedPixelLayout::packed_len`] bytes at this frame's dimensions.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is too small.
    pub fn copy_to_buffer(&self, output: &mut [u8]) -> usize {
        let width = self.width();
        let height = self.height();
        let required_size = self.pixel_layout().packed_len(width, height);
        assert!(
            output.len() >= required_size,
            "buffer too small: need {required_size}, got {}",
            output.len()
        );

        match &self.inner {
            #[cfg(target_vendor = "apple")]
            DecodedFrameInner::Hardware {
                surface, layout, ..
            } => {
                Self::copy_iosurface_to_buffer(surface, width, height, *layout, output);
            }
            #[cfg(waterkit_software_frames)]
            DecodedFrameInner::Software { data, .. } => {
                output[..data.len()].copy_from_slice(data);
            }
        }

        required_size
    }

    /// Copy `IOSurface` data to a buffer.
    #[cfg(target_vendor = "apple")]
    fn copy_iosurface_to_buffer(
        surface: &CFRetained<IOSurfaceRef>,
        width: u32,
        height: u32,
        layout: DecodedPixelLayout,
        output: &mut [u8],
    ) {
        use objc2_io_surface::IOSurfaceLockOptions;

        let row_bytes = layout.bytes_per_row(width);
        let y_size = row_bytes * height as usize;

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
                    output.as_mut_ptr().add(row * row_bytes),
                    row_bytes,
                );
            }

            // Copy UV plane
            let uv_base = surface_ref.base_address_of_plane(1).as_ptr().cast::<u8>();
            let uv_stride = surface_ref.bytes_per_row_of_plane(1);
            let uv_height = height as usize / 2;
            for row in 0..uv_height {
                ptr::copy_nonoverlapping(
                    uv_base.add(row * uv_stride),
                    output.as_mut_ptr().add(y_size + row * row_bytes),
                    row_bytes,
                );
            }

            surface_ref.unlock(IOSurfaceLockOptions::ReadOnly, ptr::null_mut());
        }
    }
}
