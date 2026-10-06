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
    CONVERSION_DEVICE_EXTENSIONS, DEVICE_EXTENSIONS, HardwareBuffer, HardwareBufferFrame,
    HardwareBufferImportError, HardwareBufferImporter, HardwareBufferLease, ImportedHardwareBuffer,
};

use super::{AndroidBridge, SensorMounting};
use crate::CameraError;
use crate::frame::{Frame, FramePlanes, orientation_from_camera2};

/// Fails unless `device` was opened with the extensions every
/// `AHardwareBuffer` import needs, which `wgpu` never enables on its own, as
/// the importer itself checks.
///
/// Whether the camera's buffers also need the conversion, and with it the
/// [`CONVERSION_DEVICE_EXTENSIONS`], is not knowable here: the reader's
/// `PRIVATE` format is the camera HAL's choice, and only the driver's reading
/// of the first buffer says whether it maps to a Vulkan format. A device
/// without those extensions is therefore accepted, with a warning, and a
/// buffer that needs the conversion ends the stream with the importer's error
/// naming the missing extension.
pub fn check_device(device: &wgpu::Device) -> Result<(), CameraError> {
    // SAFETY: the guard names the device's real backend or yields `None`, and
    // is only read for its enabled extensions.
    let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }.ok_or_else(|| {
        CameraError::GpuError("Android camera frames need a Vulkan wgpu device".into())
    })?;
    // A driver that maps the camera's buffers to a Vulkan format has them
    // aliased as NV12 textures, which the device must be able to create.
    if !device
        .features()
        .contains(wgpu::Features::TEXTURE_FORMAT_NV12)
    {
        return Err(CameraError::GpuError(
            "the wgpu device lacks TEXTURE_FORMAT_NV12, which camera frames need; request it when \
             opening the device"
                .into(),
        ));
    }
    let enabled = hal_device.enabled_device_extensions();
    if let Some(missing) = CONVERSION_DEVICE_EXTENSIONS
        .into_iter()
        .find(|extension| !enabled.contains(extension))
    {
        tracing::warn!(
            "the wgpu device does not enable {missing:?}; camera buffers the driver describes \
             only through an external format cannot be imported on it"
        );
    }
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

/// A preview image the camera handed out.
///
/// The handle is shared between the frame's imported planes and a consumer
/// that reads the `Image` itself, such as a native vision realization fed
/// through `InputImage.fromMediaImage`. The last handle's drop closes the
/// image and returns its buffer to the `ImageReader`.
#[derive(Debug)]
pub struct MediaImage {
    bridge: Arc<AndroidBridge>,
    image: Global<JObject<'static>>,
}

impl MediaImage {
    /// Shares `image` under a handle whose last drop closes it through the
    /// app's Kotlin bridge. Only `sys::android` builds these, on the
    /// frame-producing thread.
    pub(super) const fn new(bridge: Arc<AndroidBridge>, image: Global<JObject<'static>>) -> Self {
        Self { bridge, image }
    }

    /// The `android.media.Image`, still open while any `MediaImage` or the
    /// importer's lease on it lives.
    #[must_use]
    pub const fn image(&self) -> &Global<JObject<'static>> {
        &self.image
    }
}

impl Drop for MediaImage {
    fn drop(&mut self) {
        self.bridge.close_image(&self.image);
    }
}

/// The importer's share of a [`MediaImage`]. Releasing it drops one handle;
/// the image closes only once every handle is gone.
#[derive(Debug)]
struct MediaImageLease(Arc<MediaImage>);

impl HardwareBufferLease for MediaImageLease {
    fn presented(&mut self) {}

    fn release(self: Box<Self>) {
        drop(self.0); // hand this share of the image back
    }
}

/// One preview image ready for import.
#[derive(Debug)]
pub struct RawFrame {
    frame: HardwareBufferFrame,
    /// The `Image` behind `frame`, kept open for the built `Frame`.
    media: Arc<MediaImage>,
    /// Display rotation in degrees when the frame arrived.
    display_rotation: u32,
    timestamp: Duration,
}

impl RawFrame {
    /// Takes a reference on `buffer`, leased from the image `media` closes.
    pub fn new(
        buffer: &HardwareBuffer,
        media: Arc<MediaImage>,
        display_rotation: u32,
        timestamp: Duration,
    ) -> Self {
        Self {
            frame: HardwareBufferFrame::new(buffer, None)
                .with_lease(Box::new(MediaImageLease(Arc::clone(&media)))),
            media,
            display_rotation,
            timestamp,
        }
    }

    /// Imports the buffer's planes on the importer's device.
    ///
    /// # Errors
    ///
    /// Returns the importer's error when the device cannot import the
    /// buffer, such as an external-format buffer on a device opened without
    /// the [`CONVERSION_DEVICE_EXTENSIONS`] its conversion needs.
    pub fn import(
        self,
        importer: &mut HardwareBufferImporter,
        mounting: SensorMounting,
    ) -> Result<Frame, HardwareBufferImportError> {
        let buffer = importer.import(self.frame)?;
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
        Ok(Frame::new(
            planes,
            size.width,
            size.height,
            orientation_from_camera2(
                mounting.sensor_orientation,
                mounting.lens_faces_back,
                self.display_rotation,
            ),
            self.timestamp,
            self.media,
        ))
    }
}
