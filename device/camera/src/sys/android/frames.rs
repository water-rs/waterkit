//! Zero-copy frames from the camera's GPU-sampled `AHardwareBuffer`s.
//!
//! Each preview image's buffer is handed to `wgpu-external-frame`'s importer
//! with a lease on the captured frame. The importer either aliases the buffer
//! or, for a driver-private (external) YCbCr format, converts it into two
//! plane textures on the GPU; either way it releases the lease, closing the
//! image and returning its slot to the `ImageReader`, as soon as the GPU no
//! longer reads it. No pixel is read on the CPU.

use std::sync::Arc;
use std::time::Duration;

use jni::objects::{Global, JObject};
use ndk::data_space::{DataSpace, DataSpaceRange, DataSpaceStandard, DataSpaceTransfer};
use wgpu_external_frame::YcbcrEncoding;
use wgpu_external_frame::ahardware_buffer::{
    DEVICE_EXTENSIONS, HardwareBuffer, HardwareBufferFrame, HardwareBufferImporter,
    HardwareBufferLease, ImportedHardwareBuffer,
};

use super::{AndroidBridge, SensorMounting};
use crate::color::from_ycbcr_encoding;
use crate::frame::{Frame, FramePlanes, orientation_from_camera2};
use crate::{
    CameraError, ColorPrimaries, ColorRange, DynamicRangeProfile, MatrixCoefficients,
    TransferFunction, VideoColorInfo,
};

/// Fails unless `device` was opened with the extensions every
/// `AHardwareBuffer` import needs, which `wgpu` never enables on its own, as
/// the importer itself checks, and with `TEXTURE_FORMAT_NV12`.
///
/// Whether the driver aliases the camera's buffers or converts them from an
/// external format shows only on the first buffer, since the reader's
/// `PRIVATE` format is the camera HAL's choice; the conversion needs nothing
/// of the device beyond what [`DeviceRequirements`] enables for every import.
///
/// [`DeviceRequirements`]: wgpu_external_frame::ahardware_buffer::DeviceRequirements
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

/// A preview frame the camera handed out; closing it returns the image to
/// the `ImageReader` and frees one of the reader's in-flight slots, which
/// re-arms acquisition on the camera thread. Dropping the lease closes it
/// too, as the importer requires of an abandoned import.
#[derive(Debug)]
pub struct FrameLease {
    bridge: Arc<AndroidBridge>,
    frame: Option<Global<JObject<'static>>>,
}

impl FrameLease {
    pub const fn new(bridge: Arc<AndroidBridge>, frame: Global<JObject<'static>>) -> Self {
        Self {
            bridge,
            frame: Some(frame),
        }
    }
}

impl HardwareBufferLease for FrameLease {
    fn presented(&mut self) {}

    fn release(self: Box<Self>) {
        // Dropping closes the frame.
    }
}

impl Drop for FrameLease {
    fn drop(&mut self) {
        if let Some(frame) = self.frame.take() {
            self.bridge.release_frame(&frame);
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
    data_space: i32,
    dynamic_range_profile: DynamicRangeProfile,
}

impl RawFrame {
    /// Takes a reference on `buffer`, leased from the frame `lease` closes.
    pub fn new(
        buffer: &HardwareBuffer,
        lease: FrameLease,
        display_rotation: u32,
        data_space: i32,
        dynamic_range_profile: DynamicRangeProfile,
        timestamp: Duration,
    ) -> Self {
        Self {
            frame: HardwareBufferFrame::new(buffer, None).with_lease(Box::new(lease)),
            display_rotation,
            timestamp,
            data_space,
            dynamic_range_profile,
        }
    }

    /// Imports the buffer's planes on the importer's device.
    ///
    /// # Errors
    ///
    /// Returns the importer's error when the buffer cannot be imported, such
    /// as an external format other than 8-bit 4:2:0 YCbCr, or a driver that
    /// cannot allocate the conversion's descriptor set, and a color-description
    /// error when the buffer's color description is unsupported.
    pub fn import(
        self,
        importer: &mut HardwareBufferImporter,
        mounting: SensorMounting,
    ) -> Result<Frame, CameraError> {
        let data_space = self.data_space;
        let dynamic_range_profile = self.dynamic_range_profile;
        let display_rotation = self.display_rotation;
        let timestamp = self.timestamp;
        let buffer = importer
            .import(self.frame)
            .map_err(|error| CameraError::FrameImport(Arc::new(error)))?;
        let (planes, size, ycbcr) = match buffer {
            ImportedHardwareBuffer::Ycbcr420(ycbcr) => {
                let size = ycbcr.luma.texture().size();
                (
                    FramePlanes::YCbCr420 {
                        luma: ycbcr.luma,
                        chroma: ycbcr.chroma,
                    },
                    size,
                    Some(ycbcr.encoding),
                )
            }
            ImportedHardwareBuffer::Rgba(texture) => {
                let size = texture.size();
                (
                    FramePlanes::Rgb(texture.create_view(&wgpu::TextureViewDescriptor::default())),
                    size,
                    None,
                )
            }
        };
        let color = frame_color(data_space, dynamic_range_profile, ycbcr)?;
        Ok(Frame::new(
            planes,
            color,
            size.width,
            size.height,
            orientation_from_camera2(
                mounting.sensor_orientation,
                mounting.lens_faces_back,
                display_rotation,
            ),
            timestamp,
        ))
    }
}

/// Dataspace components take precedence when specified; unspecified
/// components use the dynamic-range profile captured with this image. Below
/// API 33, the Kotlin bridge reports `DATASPACE_UNKNOWN` (zero), so each
/// unspecified data-space component uses that profile's default.
fn frame_color(
    data_space: i32,
    profile: DynamicRangeProfile,
    ycbcr: Option<YcbcrEncoding>,
) -> Result<VideoColorInfo, CameraError> {
    let data_space = DataSpace::from(data_space);
    let (default_primaries, default_transfer, profile_matrix, dolby_vision) = match profile {
        DynamicRangeProfile::Sdr => (
            ColorPrimaries::Bt709,
            TransferFunction::Sdr,
            MatrixCoefficients::Bt709,
            false,
        ),
        DynamicRangeProfile::Hlg10 => (
            ColorPrimaries::Bt2020,
            TransferFunction::Hlg,
            MatrixCoefficients::Bt2020NonConstantLuminance,
            false,
        ),
        DynamicRangeProfile::Hdr10 => (
            ColorPrimaries::Bt2020,
            TransferFunction::Pq,
            MatrixCoefficients::Bt2020NonConstantLuminance,
            false,
        ),
        DynamicRangeProfile::DolbyVision => (
            ColorPrimaries::Bt2020,
            TransferFunction::Pq,
            MatrixCoefficients::Bt2020NonConstantLuminance,
            true,
        ),
    };
    let standard = data_space.standard();
    let primaries = match standard {
        DataSpaceStandard::Bt709 => ColorPrimaries::Bt709,
        DataSpaceStandard::Bt601_625
        | DataSpaceStandard::Bt601_625Unadjusted
        | DataSpaceStandard::Bt601_525
        | DataSpaceStandard::Bt601_525Unadjusted => ColorPrimaries::Bt601,
        DataSpaceStandard::Bt2020 | DataSpaceStandard::Bt2020ConstantLuminance => {
            ColorPrimaries::Bt2020
        }
        DataSpaceStandard::DciP3 => ColorPrimaries::DisplayP3,
        DataSpaceStandard::Unspecified => default_primaries,
        _ => return Err(unsupported_data_space("standard", data_space, profile)),
    };
    let transfer = match data_space.transfer() {
        DataSpaceTransfer::Smpte170M
        | DataSpaceTransfer::Srgb
        | DataSpaceTransfer::Gamma2_2
        | DataSpaceTransfer::Gamma2_6
        | DataSpaceTransfer::Gamma2_8 => TransferFunction::Sdr,
        DataSpaceTransfer::St2084 => TransferFunction::Pq,
        DataSpaceTransfer::HLG => TransferFunction::Hlg,
        DataSpaceTransfer::Unspecified => default_transfer,
        _ => return Err(unsupported_data_space("transfer", data_space, profile)),
    };
    let (matrix, range) = if let Some(ycbcr) = ycbcr {
        from_ycbcr_encoding(ycbcr)
    } else {
        let matrix = match standard {
            DataSpaceStandard::Bt601_625
            | DataSpaceStandard::Bt601_625Unadjusted
            | DataSpaceStandard::Bt601_525
            | DataSpaceStandard::Bt601_525Unadjusted => MatrixCoefficients::Bt601,
            DataSpaceStandard::Bt709 | DataSpaceStandard::DciP3 => MatrixCoefficients::Bt709,
            DataSpaceStandard::Bt2020 => MatrixCoefficients::Bt2020NonConstantLuminance,
            DataSpaceStandard::Bt2020ConstantLuminance => {
                MatrixCoefficients::Bt2020ConstantLuminance
            }
            DataSpaceStandard::Unspecified => profile_matrix,
            _ => return Err(unsupported_data_space("standard", data_space, profile)),
        };
        let range = match data_space.range() {
            DataSpaceRange::Full | DataSpaceRange::Unspecified => ColorRange::Full,
            DataSpaceRange::Limited => ColorRange::Limited,
            _ => return Err(unsupported_data_space("range", data_space, profile)),
        };
        (matrix, range)
    };
    Ok(VideoColorInfo {
        matrix,
        primaries,
        transfer,
        range,
        content_light_level: None,
        dolby_vision,
    })
}

fn unsupported_data_space(
    field: &str,
    data_space: DataSpace,
    profile: DynamicRangeProfile,
) -> CameraError {
    CameraError::UnsupportedColor(format!(
        "unsupported data space {field} in {data_space:?} for dynamic-range profile {profile:?}"
    ))
}

#[cfg(test)]
mod tests {
    use ndk::data_space::{DataSpace, DataSpaceRange, DataSpaceStandard, DataSpaceTransfer};

    use super::frame_color;
    use crate::{
        ColorPrimaries, ColorRange, DynamicRangeProfile, MatrixCoefficients, TransferFunction,
        VideoColorInfo,
    };

    fn data_space(
        standard: DataSpaceStandard,
        transfer: DataSpaceTransfer,
        range: DataSpaceRange,
    ) -> i32 {
        i32::from(DataSpace::from_parts(standard, transfer, range))
    }

    #[test]
    fn specified_dataspace_components_override_profile_defaults() {
        let color = frame_color(
            data_space(
                DataSpaceStandard::DciP3,
                DataSpaceTransfer::Srgb,
                DataSpaceRange::Full,
            ),
            DynamicRangeProfile::Hdr10,
            None,
        )
        .expect("supported dataspace");
        assert_eq!(
            color,
            VideoColorInfo {
                matrix: MatrixCoefficients::Bt709,
                primaries: ColorPrimaries::DisplayP3,
                transfer: TransferFunction::Sdr,
                range: ColorRange::Full,
                content_light_level: None,
                dolby_vision: false,
            }
        );
    }

    #[test]
    fn unspecified_dataspace_components_use_profile_defaults() {
        for (profile, primaries, transfer, matrix, dolby_vision) in [
            (
                DynamicRangeProfile::Sdr,
                ColorPrimaries::Bt709,
                TransferFunction::Sdr,
                MatrixCoefficients::Bt709,
                false,
            ),
            (
                DynamicRangeProfile::Hlg10,
                ColorPrimaries::Bt2020,
                TransferFunction::Hlg,
                MatrixCoefficients::Bt2020NonConstantLuminance,
                false,
            ),
            (
                DynamicRangeProfile::Hdr10,
                ColorPrimaries::Bt2020,
                TransferFunction::Pq,
                MatrixCoefficients::Bt2020NonConstantLuminance,
                false,
            ),
            (
                DynamicRangeProfile::DolbyVision,
                ColorPrimaries::Bt2020,
                TransferFunction::Pq,
                MatrixCoefficients::Bt2020NonConstantLuminance,
                true,
            ),
        ] {
            let color = frame_color(
                data_space(
                    DataSpaceStandard::Unspecified,
                    DataSpaceTransfer::Unspecified,
                    DataSpaceRange::Unspecified,
                ),
                profile,
                None,
            )
            .expect("unspecified components use the active profile");
            assert_eq!(
                color,
                VideoColorInfo {
                    matrix,
                    primaries,
                    transfer,
                    range: ColorRange::Full,
                    content_light_level: None,
                    dolby_vision,
                }
            );
        }
    }

    #[test]
    fn imported_ycbcr_encoding_sets_matrix_and_range() {
        let encoding = wgpu_external_frame::YcbcrEncoding {
            matrix: wgpu_external_frame::YcbcrMatrix::Bt2020,
            range: wgpu_external_frame::YcbcrRange::Video,
        };
        let color = frame_color(
            data_space(
                DataSpaceStandard::Bt709,
                DataSpaceTransfer::Smpte170M,
                DataSpaceRange::Full,
            ),
            DynamicRangeProfile::Sdr,
            Some(encoding),
        )
        .expect("supported YCbCr color");
        assert_eq!(color.matrix, MatrixCoefficients::Bt2020NonConstantLuminance);
        assert_eq!(color.range, ColorRange::Limited);
        assert_eq!(color.primaries, ColorPrimaries::Bt709);
        assert_eq!(color.transfer, TransferFunction::Sdr);
    }

    #[test]
    fn rgba_dataspace_maps_primaries_transfer_matrix_and_range() {
        for (standard, transfer, range, primaries, function, matrix, color_range) in [
            (
                DataSpaceStandard::Bt601_525Unadjusted,
                DataSpaceTransfer::Gamma2_8,
                DataSpaceRange::Limited,
                ColorPrimaries::Bt601,
                TransferFunction::Sdr,
                MatrixCoefficients::Bt601,
                ColorRange::Limited,
            ),
            (
                DataSpaceStandard::Bt2020,
                DataSpaceTransfer::St2084,
                DataSpaceRange::Limited,
                ColorPrimaries::Bt2020,
                TransferFunction::Pq,
                MatrixCoefficients::Bt2020NonConstantLuminance,
                ColorRange::Limited,
            ),
            (
                DataSpaceStandard::Bt2020ConstantLuminance,
                DataSpaceTransfer::HLG,
                DataSpaceRange::Full,
                ColorPrimaries::Bt2020,
                TransferFunction::Hlg,
                MatrixCoefficients::Bt2020ConstantLuminance,
                ColorRange::Full,
            ),
        ] {
            let color = frame_color(
                data_space(standard, transfer, range),
                DynamicRangeProfile::Sdr,
                None,
            )
            .expect("supported RGB dataspace");
            assert_eq!(color.primaries, primaries);
            assert_eq!(color.transfer, function);
            assert_eq!(color.matrix, matrix);
            assert_eq!(color.range, color_range);
        }
    }

    #[test]
    fn unsupported_standard_transfer_and_range_are_errors() {
        for (data_space, field) in [
            (
                data_space(
                    DataSpaceStandard::AdobeRgb,
                    DataSpaceTransfer::Srgb,
                    DataSpaceRange::Full,
                ),
                "standard",
            ),
            (
                data_space(
                    DataSpaceStandard::Bt709,
                    DataSpaceTransfer::Linear,
                    DataSpaceRange::Full,
                ),
                "transfer",
            ),
            (
                data_space(
                    DataSpaceStandard::Bt709,
                    DataSpaceTransfer::Srgb,
                    DataSpaceRange::Extended,
                ),
                "range",
            ),
        ] {
            let error = frame_color(data_space, DynamicRangeProfile::Sdr, None)
                .expect_err("unsupported dataspace component");
            assert!(error.to_string().contains(field), "{error}");
        }
    }
}
