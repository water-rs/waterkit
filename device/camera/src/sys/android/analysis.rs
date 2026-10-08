//! CPU-readable analysis frames from the camera's `YUV_420_888` stream.
//!
//! The capture session runs a second `ImageReader` at the analysis size next
//! to the GPU preview reader. Each acquired `android.media.Image` is held as
//! a JNI global with its planes' direct-buffer addresses and strides
//! resolved at acquisition; dropping the image calls `Image.close`,
//! returning it to the reader — the same lease shape as [`super::frames`]
//! uses for preview frames.

use std::sync::Arc;
use std::time::Duration;

use jni::objects::{Global, JObject};

use super::frames::frame_color;
use super::{AndroidBridge, SensorMounting};
use crate::analysis::{AnalysisFrame, AnalysisPlane, AnalysisPlanes};
use crate::frame::orientation_from_camera2;
use crate::{CameraError, DynamicRangeProfile};

/// An acquired `android.media.Image` and its three `YUV_420_888` planes'
/// direct-buffer addresses and strides.
///
/// The image is held through a JNI global for the frame's life and closed on
/// drop, returning it to the `ImageReader`.
#[derive(Debug)]
pub struct AnalysisImage {
    bridge: Arc<AndroidBridge>,
    image: Global<JObject<'static>>,
    planes: [ImagePlane; 3],
    width: u32,
    height: u32,
}

/// One `Image.Plane`'s direct buffer: address, capacity, and strides.
#[derive(Debug, Clone, Copy)]
pub struct ImagePlane {
    pub address: usize,
    pub len: usize,
    pub row_stride: usize,
    pub pixel_stride: usize,
}

impl ImagePlane {
    /// The plane's mapped bytes as an [`AnalysisPlane`] borrowing the frame.
    const fn plane(&self) -> AnalysisPlane<'_> {
        // SAFETY: `address` is the direct `ByteBuffer` of a plane of the
        // image this frame holds a JNI global on, so it stays mapped at
        // least as long as the returned borrow lives.
        AnalysisPlane::new(
            unsafe { std::slice::from_raw_parts(self.address as *const u8, self.len) },
            self.row_stride,
            self.pixel_stride,
        )
    }
}

impl AnalysisImage {
    pub(super) const fn new(
        bridge: Arc<AndroidBridge>,
        image: Global<JObject<'static>>,
        planes: [ImagePlane; 3],
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            bridge,
            image,
            planes,
            width,
            height,
        }
    }

    /// The `android.media.Image` the frame holds.
    pub const fn image(&self) -> &Global<JObject<'static>> {
        &self.image
    }

    /// The image's width in pixels.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// The image's height in pixels.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The image's `Y'`, `Cb`, `Cr` planes. `Cb` and `Cr` commonly share
    /// storage with a `pixel_stride` of 2.
    pub const fn planes(&self) -> AnalysisPlanes<'_> {
        let [luma, cb, cr] = &self.planes;
        AnalysisPlanes {
            luma: luma.plane(),
            cb: cb.plane(),
            cr: cr.plane(),
        }
    }
}

impl Drop for AnalysisImage {
    fn drop(&mut self) {
        self.bridge.release_image(&self.image);
    }
}

/// One analysis image as the reader thread takes it from the helper queue.
#[derive(Debug)]
pub struct RawAnalysisFrame {
    /// The acquired image and its planes.
    pub image: AnalysisImage,
    /// Display rotation in degrees when the image arrived.
    pub display_rotation: u32,
    /// The image's data space; `DATASPACE_UNKNOWN` (0) below API 33.
    pub data_space: i32,
    /// The sensor timestamp (start of exposure) through the stream clock.
    pub timestamp: Duration,
}

impl RawAnalysisFrame {
    /// The frame the analysis stream yields.
    ///
    /// The analysis output runs the session's standard dynamic-range
    /// profile, so the color description starts from SDR where the image's
    /// data space is unspecified.
    pub fn into_frame(self, mounting: SensorMounting) -> Result<AnalysisFrame, CameraError> {
        let color = frame_color(self.data_space, DynamicRangeProfile::Sdr, None)?;
        Ok(AnalysisFrame::android(
            self.image,
            orientation_from_camera2(
                mounting.sensor_orientation,
                mounting.lens_faces_back,
                self.display_rotation,
            ),
            color,
            self.timestamp,
        ))
    }
}
