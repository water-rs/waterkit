//! Camera frames: the GPU planes a frame holds, how they are encoded, and how
//! the stored pixels relate to the upright image.

use std::time::Duration;

use wgpu_external_frame::YcbcrEncoding;

pub use waterkit_core::Orientation;

/// The GPU planes of one camera frame, in the layout the platform delivered.
///
/// Every view samples the stored code values: no plane uses an sRGB view
/// format, so sampling returns the camera's non-linear values unchanged.
/// [`FrameConverter`](crate::FrameConverter) turns any layout into one
/// upright RGBA texture.
#[derive(Debug, Clone)]
pub enum FramePlanes {
    /// One 8-bit RGBA- or BGRA-ordered texture; `format()` of the view's
    /// texture says which.
    Rgb(wgpu::TextureView),
    /// Biplanar 4:2:0 YCbCr: full-size luma (R8/R16 or plane 0 of an NV12
    /// texture) and half-size interleaved chroma (RG8/RG16 or plane 1).
    ///
    /// 16-bit planes hold 10-bit codes in their most significant bits, the
    /// P010 layout.
    YCbCr420 {
        /// Full-resolution Y' plane.
        luma: wgpu::TextureView,
        /// Half-resolution interleaved Cb/Cr plane.
        chroma: wgpu::TextureView,
        /// How the samples map to R'G'B'.
        encoding: YcbcrEncoding,
    },
    /// Packed 4:2:2 YCbCr in YUYV byte order: one `Rgba8Unorm` texture half
    /// the frame's width, each texel holding two horizontally adjacent pixels
    /// as (Y'0, Cb, Y'1, Cr).
    ///
    /// Desktop webcams deliver this when uncompressed YUYV is the format that
    /// best matches the requested resolution and frame rate, which is the
    /// common case for UVC cameras on Linux.
    YCbCr422 {
        /// The packed YUYV texture.
        yuyv: wgpu::TextureView,
        /// How the samples map to R'G'B'.
        encoding: YcbcrEncoding,
    },
}

/// The orientation of stored pixels that become upright when they are
/// first mirrored horizontally (if `mirrored`) and then rotated
/// `clockwise_degrees` clockwise.
///
/// # Panics
///
/// Panics when `clockwise_degrees` is not a multiple of 90: every
/// platform reports its rotations in quarter turns, so anything else is a
/// backend defect.
#[cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "android", test)),
    expect(
        dead_code,
        reason = "desktop frames are delivered upright; only the mobile backends and the tests rotate"
    )
)]
pub fn orientation_from_rotation(clockwise_degrees: u32, mirrored: bool) -> Orientation {
    assert!(
        clockwise_degrees.is_multiple_of(90),
        "camera rotation of {clockwise_degrees}° is not a quarter turn"
    );
    match ((clockwise_degrees / 90) % 4, mirrored) {
        (0, false) => Orientation::Up,
        (1, false) => Orientation::Right,
        (2, false) => Orientation::Down,
        (3, false) => Orientation::Left,
        (0, true) => Orientation::UpMirrored,
        (1, true) => Orientation::RightMirrored,
        (2, true) => Orientation::DownMirrored,
        (_, true) => Orientation::LeftMirrored,
        (_, false) => unreachable!("quarter turns are reduced modulo 4"),
    }
}

/// Orientation of a Camera2 frame: the sensor's mounting angle combined
/// with the display rotation, whose sign flips for a lens that does not
/// face away from the display, because that lens sees the world mirrored
/// relative to it. Camera2 buffers themselves are never mirrored.
#[cfg(any(target_os = "android", test))]
pub fn orientation_from_camera2(
    sensor_orientation: u32,
    lens_faces_back: bool,
    display_rotation: u32,
) -> Orientation {
    let rotation = if lens_faces_back {
        (sensor_orientation + 360 - display_rotation) % 360
    } else {
        (sensor_orientation + display_rotation) % 360
    };
    orientation_from_rotation(rotation, false)
}

/// A GPU-backed camera frame.
///
/// The plane textures are the frame's whole storage: each keeps what its
/// pixels live in alive, and `wgpu` releases it once the texture, with every
/// view and bind group of it, has dropped and the last submission that read
/// it has completed. Work recorded before the frame drops therefore always
/// reads valid pixels, whether it is submitted before or after.
///
/// On Apple platforms and Android the planes alias the captured buffer
/// itself, which comes from a small pool the camera owns, and the buffer
/// goes back to the camera once those textures are released. Every frame a
/// consumer holds keeps one of those buffers out of the pool, and so does a
/// plane texture or view kept past its frame; when none is left the camera
/// drops new frames until one comes back. Drop each frame as soon as its
/// work is submitted, and convert or copy the pixels to keep them longer.
#[derive(Debug)]
pub struct Frame {
    planes: FramePlanes,
    orientation: Orientation,
    width: u32,
    height: u32,
    timestamp: Duration,
}

impl Frame {
    pub(crate) const fn new(
        planes: FramePlanes,
        width: u32,
        height: u32,
        orientation: Orientation,
        timestamp: Duration,
    ) -> Self {
        Self {
            planes,
            orientation,
            width,
            height,
            timestamp,
        }
    }

    /// The GPU planes holding the frame's pixels.
    #[must_use]
    pub const fn planes(&self) -> &FramePlanes {
        &self.planes
    }

    /// How the stored pixels relate to upright.
    #[must_use]
    pub const fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// Stored width in pixels, before [`Self::orientation`] is applied.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Stored height in pixels, before [`Self::orientation`] is applied.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Presentation timestamp since the camera started.
    #[must_use]
    pub const fn timestamp(&self) -> Duration {
        self.timestamp
    }
}

#[cfg(test)]
mod tests {
    use super::{Orientation, orientation_from_camera2, orientation_from_rotation};

    #[test]
    fn rotation_and_mirroring_map_to_exif_orientations() {
        let cases = [
            (0, false, Orientation::Up),
            (90, false, Orientation::Right),
            (180, false, Orientation::Down),
            (270, false, Orientation::Left),
            (360, false, Orientation::Up),
            (0, true, Orientation::UpMirrored),
            (90, true, Orientation::RightMirrored),
            (180, true, Orientation::DownMirrored),
            (270, true, Orientation::LeftMirrored),
        ];
        for (degrees, mirrored, expected) in cases {
            assert_eq!(
                orientation_from_rotation(degrees, mirrored),
                expected,
                "{degrees}° mirrored={mirrored}"
            );
        }
    }

    #[test]
    fn camera2_rotation_flips_sign_for_front_lenses() {
        // A back sensor mounted at 90° in a portrait device needs a quarter
        // turn clockwise; turning the display to landscape (90°) cancels it.
        assert_eq!(orientation_from_camera2(90, true, 0), Orientation::Right);
        assert_eq!(orientation_from_camera2(90, true, 90), Orientation::Up);
        assert_eq!(orientation_from_camera2(90, true, 270), Orientation::Down);
        // A front sensor at 270° adds the display rotation instead.
        assert_eq!(orientation_from_camera2(270, false, 0), Orientation::Left);
        assert_eq!(orientation_from_camera2(270, false, 90), Orientation::Up);
        assert_eq!(orientation_from_camera2(270, false, 270), Orientation::Down);
    }

    #[test]
    fn quarter_turn_orientations_swap_dimensions() {
        for orientation in [
            Orientation::LeftMirrored,
            Orientation::Right,
            Orientation::RightMirrored,
            Orientation::Left,
        ] {
            assert!(orientation.swaps_dimensions());
        }
        for orientation in [
            Orientation::Up,
            Orientation::UpMirrored,
            Orientation::Down,
            Orientation::DownMirrored,
        ] {
            assert!(!orientation.swaps_dimensions());
        }
    }
}
