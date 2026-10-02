//! Typed CPU frame views.
//!
//! [`CpuFrame`] is a borrowed, typed description of one or more pixel planes:
//! format, dimensions, per-plane `stride`, timestamp and orientation. It is
//! deliberately *not* a "universal" buffer type — CPU planes stay CPU
//! planes; GPU resources are not part of this build.
//!
//! All coordinates reported by decoders are in the frame's buffer space.
//! [`Orientation`] describes the rotation needed to display the buffer
//! upright so consumers can transform results with
//! [`Quadrilateral::rotated`](crate::Quadrilateral::rotated).

use waterkit_core::Timestamp;

use crate::VisionError;

/// Pixel layout of a CPU frame plane set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FrameFormat {
    /// Single-plane 8-bit luminance.
    Luma8,
    /// Interleaved RGB, 8 bits per channel, 3 bytes per pixel.
    Rgb8,
    /// Interleaved BGR, 8 bits per channel, 3 bytes per pixel.
    Bgr8,
    /// Interleaved RGBA, 8 bits per channel, 4 bytes per pixel.
    Rgba8,
    /// Interleaved BGRA, 8 bits per channel, 4 bytes per pixel.
    Bgra8,
    /// YUV 4:2:0 bi-planar: plane 0 is full-resolution luma, plane 1 is
    /// interleaved UV at half width/height.
    Nv12,
    /// YUV 4:2:0 tri-planar: plane 0 luma, planes 1/2 chroma at half
    /// resolution.
    I420,
}

impl FrameFormat {
    /// Number of planes the format requires.
    #[must_use]
    pub const fn plane_count(self) -> usize {
        match self {
            Self::Luma8 | Self::Rgb8 | Self::Bgr8 | Self::Rgba8 | Self::Bgra8 => 1,
            Self::Nv12 => 2,
            Self::I420 => 3,
        }
    }

    /// Bytes per pixel on plane 0.
    #[must_use]
    pub const fn plane0_bytes_per_pixel(self) -> usize {
        match self {
            Self::Luma8 | Self::Nv12 | Self::I420 => 1,
            Self::Rgb8 | Self::Bgr8 => 3,
            Self::Rgba8 | Self::Bgra8 => 4,
        }
    }

    /// Whether the format's channel order, on the first byte, is red-first.
    pub(crate) const fn is_red_first(self) -> bool {
        matches!(self, Self::Rgb8 | Self::Rgba8)
    }
}

/// Rotation needed to display a buffer's contents upright.
///
/// Decoders always operate in buffer space; use this metadata when mapping
/// reported geometry onto the oriented scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Orientation {
    /// No rotation.
    #[default]
    Up,
    /// Rotate the buffer 90 degrees clockwise to display upright.
    Right,
    /// Rotate the buffer 180 degrees.
    Down,
    /// Rotate the buffer 90 degrees counter-clockwise to display upright.
    Left,
}

impl Orientation {
    /// Rotation in degrees clockwise.
    #[must_use]
    pub const fn degrees_clockwise(self) -> i32 {
        match self {
            Self::Up => 0,
            Self::Right => 90,
            Self::Down => 180,
            Self::Left => 270,
        }
    }
}

/// A borrowed view of one pixel plane.
#[derive(Debug, Clone, Copy)]
pub struct Plane<'a> {
    /// Plane bytes. `data.len()` must cover `stride * (height - 1)` +
    /// `width * bytes_per_pixel`.
    pub data: &'a [u8],
    /// Distance in bytes between the start of consecutive rows.
    pub stride: usize,
    /// Plane width in pixels.
    pub width: u32,
    /// Plane height in pixels.
    pub height: u32,
}

impl Plane<'_> {
    /// Access the pixel byte at `(x, y)` in byte granularity.
    ///
    /// Returns `None` when out of bounds.
    #[must_use]
    pub fn get(&self, x: u32, y: u32) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.data
            .get(y as usize * self.stride + x as usize)
            .copied()
    }
}

/// A borrowed, typed view of a CPU frame.
///
/// Construct via the format-specific constructors ([`CpuFrame::luma`],
/// [`CpuFrame::rgba`], [`CpuFrame::nv12`], ...) or the generic
/// [`CpuFrame::new`]. All constructors validate plane count, strides and
/// buffer lengths and return [`VisionError::InvalidFrame`] on mismatch.
#[derive(Debug, Clone)]
pub struct CpuFrame<'a> {
    planes: Vec<Plane<'a>>,
    format: FrameFormat,
    width: u32,
    height: u32,
    timestamp: Timestamp,
    orientation: Orientation,
}

impl<'a> CpuFrame<'a> {
    /// Construct a frame from explicitly described planes.
    ///
    /// `planes` must contain exactly `format.plane_count()` entries. The
    /// first plane must describe `width` x `height` pixels; chroma planes of
    /// subsampled formats follow the format's subsampling.
    ///
    /// # Errors
    /// [`VisionError::InvalidFrame`] when planes do not match the format or
    /// strides/lengths are inconsistent.
    pub fn new(
        format: FrameFormat,
        planes: Vec<Plane<'a>>,
        width: u32,
        height: u32,
        timestamp: Timestamp,
        orientation: Orientation,
    ) -> Result<Self, VisionError> {
        if width == 0 || height == 0 {
            return Err(VisionError::InvalidFrame("zero-size frame".to_string()));
        }
        if planes.len() != format.plane_count() {
            return Err(VisionError::InvalidFrame(format!(
                "{format:?} requires {} planes, got {}",
                format.plane_count(),
                planes.len()
            )));
        }
        let bpp = format.plane0_bytes_per_pixel();
        for (i, plane) in planes.iter().enumerate() {
            let (min_width, min_height, min_stride) = match (format, i) {
                (FrameFormat::Nv12, 1) => (
                    width.div_ceil(2) * 2,
                    height.div_ceil(2),
                    (width.div_ceil(2) * 2) as usize,
                ),
                (FrameFormat::I420, 1 | 2) => (
                    width.div_ceil(2),
                    height.div_ceil(2),
                    width.div_ceil(2) as usize,
                ),
                _ => (width, height, width as usize * bpp),
            };
            if plane.stride < min_stride {
                return Err(VisionError::InvalidFrame(format!(
                    "plane {i}: stride {} < required {min_stride}",
                    plane.stride
                )));
            }
            if plane.width < min_width || plane.height < min_height {
                return Err(VisionError::InvalidFrame(format!(
                    "plane {i}: {}x{} smaller than required {min_width}x{min_height}",
                    plane.width, plane.height
                )));
            }
            let needed = plane.stride * (plane.height as usize - 1) + plane.width as usize;
            if plane.data.len() < needed {
                return Err(VisionError::InvalidFrame(format!(
                    "plane {i}: {} bytes < required {needed}",
                    plane.data.len()
                )));
            }
        }
        Ok(Self {
            planes,
            format,
            width,
            height,
            timestamp,
            orientation,
        })
    }

    /// Single-plane 8-bit luminance frame with `Timestamp::UNIX_EPOCH` and
    /// [`Orientation::Up`]; refine via [`CpuFrame::with_timestamp`] /
    /// [`CpuFrame::with_orientation`].
    ///
    /// # Errors
    /// [`VisionError::InvalidFrame`] on inconsistent stride/length.
    pub fn luma(
        data: &'a [u8],
        width: u32,
        height: u32,
        stride: usize,
    ) -> Result<Self, VisionError> {
        Self::new(
            FrameFormat::Luma8,
            vec![Plane {
                data,
                stride,
                width,
                height,
            }],
            width,
            height,
            Timestamp::UNIX_EPOCH,
            Orientation::Up,
        )
    }

    /// Convenience constructor for a packed single-plane format
    /// (`Rgb8`, `Bgr8`, `Rgba8`, `Bgra8`).
    ///
    /// # Errors
    /// [`VisionError::InvalidFrame`] on inconsistent stride/length.
    pub fn packed(
        format: FrameFormat,
        data: &'a [u8],
        width: u32,
        height: u32,
        stride: usize,
    ) -> Result<Self, VisionError> {
        if matches!(
            format,
            FrameFormat::Nv12 | FrameFormat::I420 | FrameFormat::Luma8
        ) {
            return Err(VisionError::InvalidFrame(format!(
                "{format:?} is not a packed RGB format; use `luma`, `nv12` or `new`"
            )));
        }
        Self::new(
            format,
            vec![Plane {
                data,
                stride,
                width,
                height,
            }],
            width,
            height,
            Timestamp::UNIX_EPOCH,
            Orientation::Up,
        )
    }

    /// NV12 frame from a luma plane and an interleaved UV plane.
    ///
    /// # Errors
    /// [`VisionError::InvalidFrame`] on inconsistent strides/lengths.
    pub fn nv12(
        y: Plane<'a>,
        uv: Plane<'a>,
        timestamp: Timestamp,
        orientation: Orientation,
    ) -> Result<Self, VisionError> {
        let (w, h) = (y.width, y.height);
        Self::new(FrameFormat::Nv12, vec![y, uv], w, h, timestamp, orientation)
    }

    /// Set the frame timestamp.
    #[must_use]
    pub const fn with_timestamp(mut self, timestamp: Timestamp) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// Set the buffer orientation.
    #[must_use]
    pub const fn with_orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Frame pixel format.
    #[must_use]
    pub const fn format(&self) -> FrameFormat {
        self.format
    }

    /// Frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Capture timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Buffer orientation.
    #[must_use]
    pub const fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// All planes.
    #[must_use]
    pub fn planes(&self) -> &[Plane<'a>] {
        &self.planes
    }

    /// The primary (luma / packed) plane.
    ///
    /// # Panics
    /// Never: constructors guarantee at least one plane.
    #[must_use]
    pub fn plane0(&self) -> Plane<'a> {
        self.planes[0]
    }

    /// Copy the frame into an owned [`FrameBuf`].
    ///
    /// Planes are compacted: the owned copy has `stride ==
    /// width * bytes_per_pixel` (no padding) unless a plane's declared
    /// dimensions require more.
    #[must_use]
    pub fn to_owned_frame(&self) -> FrameBuf {
        let mut planes = Vec::with_capacity(self.planes.len());
        for (i, plane) in self.planes.iter().enumerate() {
            // Chroma planes describe their width in bytes-as-pixels (bpp 1).
            let bpp = if i == 0 {
                format_plane_bpp(self.format)
            } else {
                1
            };
            let row_bytes = plane.width as usize * bpp;
            let mut data = Vec::with_capacity(row_bytes * plane.height as usize);
            for y in 0..plane.height as usize {
                let start = y * plane.stride;
                data.extend_from_slice(&plane.data[start..start + row_bytes]);
            }
            planes.push(OwnedPlane {
                data,
                stride: row_bytes,
                width: plane.width,
                height: plane.height,
            });
        }
        FrameBuf {
            planes,
            format: self.format,
            width: self.width,
            height: self.height,
            timestamp: self.timestamp,
            orientation: self.orientation,
        }
    }
}

const fn format_plane_bpp(format: FrameFormat) -> usize {
    match format {
        FrameFormat::Luma8 | FrameFormat::Nv12 | FrameFormat::I420 => 1,
        FrameFormat::Rgb8 | FrameFormat::Bgr8 => 3,
        FrameFormat::Rgba8 | FrameFormat::Bgra8 => 4,
    }
}

#[derive(Debug, Clone)]
struct OwnedPlane {
    data: Vec<u8>,
    stride: usize,
    width: u32,
    height: u32,
}

/// An owned CPU frame: the storage counterpart of [`CpuFrame`], used by the
/// stream API and GPU readback.
#[derive(Debug, Clone)]
pub struct FrameBuf {
    planes: Vec<OwnedPlane>,
    format: FrameFormat,
    width: u32,
    height: u32,
    timestamp: Timestamp,
    orientation: Orientation,
}

impl FrameBuf {
    /// Wrap an existing tightly-packed single plane buffer.
    ///
    /// `stride` must be at least `width * bytes_per_pixel`; excess trailing
    /// bytes are ignored.
    ///
    /// # Errors
    /// [`VisionError::InvalidFrame`] on inconsistent stride/length.
    pub fn single_plane(
        format: FrameFormat,
        data: Vec<u8>,
        width: u32,
        height: u32,
        stride: usize,
    ) -> Result<Self, VisionError> {
        // Validate before wrapping: build the borrowed view up front.
        CpuFrame::new(
            format,
            vec![Plane {
                data: data.as_slice(),
                stride,
                width,
                height,
            }],
            width,
            height,
            Timestamp::UNIX_EPOCH,
            Orientation::Up,
        )?;
        Ok(Self {
            planes: vec![OwnedPlane {
                data,
                stride,
                width,
                height,
            }],
            format,
            width,
            height,
            timestamp: Timestamp::UNIX_EPOCH,
            orientation: Orientation::Up,
        })
    }

    /// Borrow the owned frame as a [`CpuFrame`].
    ///
    /// # Panics
    /// Never: constructors validate invariants.
    #[must_use]
    pub fn as_cpu_frame(&self) -> CpuFrame<'_> {
        CpuFrame::new(
            self.format,
            self.planes
                .iter()
                .map(|p| Plane {
                    data: p.data.as_slice(),
                    stride: p.stride,
                    width: p.width,
                    height: p.height,
                })
                .collect(),
            self.width,
            self.height,
            self.timestamp,
            self.orientation,
        )
        .expect("FrameBuf invariants were validated at construction")
    }

    /// Frame pixel format.
    #[must_use]
    pub const fn format(&self) -> FrameFormat {
        self.format
    }

    /// Frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Capture timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Buffer orientation.
    #[must_use]
    pub const fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// Set the timestamp.
    #[must_use]
    pub const fn with_timestamp(mut self, timestamp: Timestamp) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// Set the orientation.
    #[must_use]
    pub const fn with_orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = orientation;
        self
    }
}
