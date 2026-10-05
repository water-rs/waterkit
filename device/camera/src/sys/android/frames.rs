//! Zero-copy frames from the camera's GPU-sampled `AHardwareBuffer`s.
//!
//! Each preview image's buffer is handed to `wgpu-external-frame`'s importer
//! with a lease on the image. The importer either aliases the buffer or, for a
//! driver-private (external) YCbCr format, converts it into two plane textures
//! on the GPU; either way it releases the lease, closing the image and
//! returning the buffer to the `ImageReader`, as soon as the GPU no longer
//! reads it. No pixel is read on the CPU.

use std::sync::Arc;
use std::time::Duration;

use jni::objects::{Global, JObject};
use wgpu_external_frame::ahardware_buffer::{
    DEVICE_EXTENSIONS, HardwareBuffer, HardwareBufferFrame, HardwareBufferImporter,
    HardwareBufferLease, ImportedHardwareBuffer,
};

use super::{AndroidBridge, SensorMounting};
use crate::frame::{Frame, FramePlanes, FrameStorage};
use crate::{CameraError, Orientation};

/// Fails unless `device` was opened with the extensions an `AHardwareBuffer`
/// import needs, which `wgpu` never enables on its own.
pub fn check_device(device: &wgpu::Device) -> Result<(), CameraError> {
    // SAFETY: the guard names the device's real backend or yields `None`, and
    // is only read for its enabled extensions.
    let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }.ok_or_else(|| {
        CameraError::GpuError("Android camera frames need a Vulkan wgpu device".into())
    })?;
    let enabled = hal_device.enabled_device_extensions();
    DEVICE_EXTENSIONS
        .into_iter()
        .find(|extension| !enabled.contains(extension))
        .map_or(Ok(()), |missing| {
            Err(CameraError::GpuError(format!(
                "the wgpu device does not enable {missing:?}, which camera frames need; open it \
                 with `wgpu_external_frame::ahardware_buffer::request_device`, or apply \
                 `ahardware_buffer::DeviceRequirements` when opening it yourself"
            )))
        })
}

/// A preview image the camera handed out; closing it returns its buffer to
/// the `ImageReader`. Dropping the lease closes it too, as the importer
/// requires of an abandoned import.
#[derive(Debug)]
pub struct ImageLease {
    bridge: Arc<AndroidBridge>,
    image: Option<Global<JObject<'static>>>,
}

impl ImageLease {
    pub const fn new(bridge: Arc<AndroidBridge>, image: Global<JObject<'static>>) -> Self {
        Self {
            bridge,
            image: Some(image),
        }
    }
}

impl HardwareBufferLease for ImageLease {
    fn presented(&mut self) {}

    fn release(self: Box<Self>) {
        // Dropping closes the image.
    }
}

impl Drop for ImageLease {
    fn drop(&mut self) {
        if let Some(image) = self.image.take() {
            self.bridge.close_image(&image);
        }
    }
}

/// One preview image ready for import.
#[derive(Debug)]
pub struct RawFrame {
    frame: HardwareBufferFrame,
    /// Display rotation in degrees when the frame arrived.
    display_rotation: u32,
    timestamp: Duration,
}

impl RawFrame {
    /// Takes a reference on `buffer`, leased from the image `lease` closes.
    pub fn new(
        buffer: &HardwareBuffer,
        lease: ImageLease,
        display_rotation: u32,
        timestamp: Duration,
    ) -> Self {
        Self {
            frame: HardwareBufferFrame::new(buffer, None).with_lease(Box::new(lease)),
            display_rotation,
            timestamp,
        }
    }

    /// Imports the buffer's planes on the importer's device.
    ///
    /// # Panics
    ///
    /// Panics when the importer rejects the buffer. The reader is configured
    /// for GPU-sampled `PRIVATE` buffers, which every Camera2 device delivers
    /// as importable YCbCr, so a rejection is a platform defect.
    pub fn import(self, importer: &mut HardwareBufferImporter, mounting: SensorMounting) -> Frame {
        let buffer = importer
            .import(self.frame)
            .unwrap_or_else(|error| panic!("a camera frame could not be imported: {error}"));
        let (planes, size) = match buffer {
            ImportedHardwareBuffer::Ycbcr420(ycbcr) => {
                let size = ycbcr.luma.texture().size();
                (
                    FramePlanes::YCbCr420 {
                        luma: ycbcr.luma,
                        chroma: ycbcr.chroma,
                        encoding: ycbcr.encoding,
                    },
                    size,
                )
            }
            ImportedHardwareBuffer::Rgba(texture) => {
                let size = texture.size();
                (
                    FramePlanes::Rgb(texture.create_view(&wgpu::TextureViewDescriptor::default())),
                    size,
                )
            }
        };
        Frame::new(
            planes,
            FrameStorage::Imported,
            size.width,
            size.height,
            Orientation::from_camera2(
                mounting.sensor_orientation,
                mounting.lens_faces_back,
                self.display_rotation,
            ),
            self.timestamp,
        )
    }
}
