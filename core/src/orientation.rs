//! Image orientation shared by camera and vision inputs.

/// How the stored pixels relate to the upright image (EXIF orientations 1–8).
///
/// Upright is the scene as the lens sees it, unmirrored, with the horizon
/// level relative to the display. Each variant is the EXIF orientation of the
/// stored pixels: [`Right`](Self::Right) (EXIF 6) means the stored image must
/// be rotated 90° clockwise to be upright.
/// The mirrored variants appear only when the platform mirrored the pixels;
/// showing a front camera mirrored, as a selfie preview, is a presentation
/// choice left to the consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Orientation {
    /// EXIF 1: stored upright.
    Up = 1,
    /// EXIF 2: mirrored horizontally.
    UpMirrored = 2,
    /// EXIF 3: rotated 180°.
    Down = 3,
    /// EXIF 4: mirrored vertically.
    DownMirrored = 4,
    /// EXIF 5: transposed; mirror horizontally, then rotate 90° counter-clockwise
    /// to be upright.
    LeftMirrored = 5,
    /// EXIF 6: rotate 90° clockwise to be upright.
    Right = 6,
    /// EXIF 7: transversed; mirror horizontally, then rotate 90° clockwise to be
    /// upright.
    RightMirrored = 7,
    /// EXIF 8: rotate 90° counter-clockwise to be upright.
    Left = 8,
}

impl Orientation {
    /// The EXIF orientation value, 1–8.
    #[must_use]
    pub const fn exif(self) -> u8 {
        self as u8
    }

    /// Whether the upright image's width is the stored height, which is the
    /// case for the four orientations that turn the image a quarter.
    #[must_use]
    pub const fn swaps_dimensions(self) -> bool {
        matches!(
            self,
            Self::LeftMirrored | Self::Right | Self::RightMirrored | Self::Left
        )
    }
}
