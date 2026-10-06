//! Reading and writing the versioned, uncompressed `WKRV` frame stream.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::time::Duration;

use thiserror::Error;
use waterkit_video_core::{ColorRange, MatrixCoefficients};
use yuv::{YuvBiPlanarImage, YuvConversionMode, YuvRange, YuvStandardMatrix};

const MAGIC: &[u8; 4] = b"WKRV";
const VERSION: u8 = 2;
const PIXEL_FORMAT_RGBA8: u8 = 2;
const PIXEL_FORMAT_NV12: u8 = 3;

/// Pixel layout and YCbCr encoding of a raw-video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawVideoLayout {
    /// Non-linear R′G′B′ samples in RGBA byte order.
    Rgba8,
    /// Biplanar 4:2:0 samples: luma rows followed by interleaved `CbCr` rows.
    Nv12 {
        /// Matrix coefficients used to decode the samples.
        matrix: MatrixCoefficients,
        /// Encoded component range.
        range: ColorRange,
    },
}

/// Fixed metadata stored in a `WKRV` file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawVideoHeader {
    /// Pixel layout and, for NV12, its matrix and range.
    pub layout: RawVideoLayout,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Nominal frames per second, or zero when unavailable.
    pub fps: u32,
}

impl RawVideoHeader {
    /// Header length in bytes.
    pub const LEN: usize = 20;

    /// Serializes this header as WKRV version 2.
    ///
    /// # Errors
    ///
    /// Returns [`RawVideoError::InvalidMatrix`] for a matrix without a
    /// supported NV12 decoder.
    pub fn to_bytes(&self) -> Result<[u8; Self::LEN], RawVideoError> {
        let (pixel_format, matrix, range) = match self.layout {
            RawVideoLayout::Rgba8 => (PIXEL_FORMAT_RGBA8, 0, 1),
            RawVideoLayout::Nv12 { matrix, range } => {
                let matrix_code = matrix.cicp();
                if matrix == MatrixCoefficients::Bt2020ConstantLuminance {
                    return Err(RawVideoError::InvalidMatrix {
                        pixel_format: PIXEL_FORMAT_NV12,
                        code: matrix_code,
                    });
                }
                let range_code = match range {
                    ColorRange::Limited => 0,
                    ColorRange::Full => 1,
                };
                (PIXEL_FORMAT_NV12, matrix_code, range_code)
            }
        };

        let mut bytes = [0; Self::LEN];
        bytes[..4].copy_from_slice(MAGIC);
        bytes[4] = VERSION;
        bytes[5] = pixel_format;
        bytes[6] = matrix;
        bytes[7] = range;
        bytes[8..12].copy_from_slice(&self.width.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.height.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.fps.to_le_bytes());
        Ok(bytes)
    }

    /// Parses and validates a fixed-length WKRV header.
    ///
    /// Version 1 is rejected because it did not record the YCbCr matrix.
    ///
    /// # Errors
    ///
    /// Returns an error for bad magic, unsupported versions, unknown pixel
    /// formats, or invalid matrix and range codes.
    pub fn parse(bytes: &[u8; Self::LEN]) -> Result<Self, RawVideoError> {
        if &bytes[..4] != MAGIC {
            return Err(RawVideoError::BadMagic);
        }
        if bytes[4] != VERSION {
            return Err(RawVideoError::UnsupportedVersion(bytes[4]));
        }

        let layout = match bytes[5] {
            PIXEL_FORMAT_RGBA8 => {
                if bytes[6] != 0 {
                    return Err(RawVideoError::InvalidMatrix {
                        pixel_format: PIXEL_FORMAT_RGBA8,
                        code: bytes[6],
                    });
                }
                if bytes[7] != 1 {
                    return Err(RawVideoError::InvalidRange {
                        pixel_format: PIXEL_FORMAT_RGBA8,
                        code: bytes[7],
                    });
                }
                RawVideoLayout::Rgba8
            }
            PIXEL_FORMAT_NV12 => {
                let code = bytes[6];
                let matrix = MatrixCoefficients::from_cicp(code)
                    .filter(|matrix| {
                        *matrix != MatrixCoefficients::Bt2020ConstantLuminance && code != 5
                    })
                    .ok_or(RawVideoError::InvalidMatrix {
                        pixel_format: PIXEL_FORMAT_NV12,
                        code,
                    })?;
                let range = match bytes[7] {
                    0 => ColorRange::Limited,
                    1 => ColorRange::Full,
                    code => {
                        return Err(RawVideoError::InvalidRange {
                            pixel_format: PIXEL_FORMAT_NV12,
                            code,
                        });
                    }
                };
                RawVideoLayout::Nv12 { matrix, range }
            }
            pixel_format => return Err(RawVideoError::UnknownPixelFormat(pixel_format)),
        };

        Ok(Self {
            layout,
            width: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            height: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            fps: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
        })
    }

    fn payload_len(self) -> u128 {
        let width = u128::from(self.width);
        let height = u128::from(self.height);
        match self.layout {
            RawVideoLayout::Rgba8 => width * height * 4,
            RawVideoLayout::Nv12 { .. } => {
                let chroma_width = width.div_ceil(2);
                let chroma_height = height.div_ceil(2);
                width * height + chroma_width * chroma_height * 2
            }
        }
    }
}

/// Errors while reading, validating, or converting a WKRV stream.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RawVideoError {
    /// The underlying file or stream could not be read.
    #[error("raw video I/O error: {0}")]
    Io(#[from] io::Error),
    /// The file does not start with the WKRV magic bytes.
    #[error("bad WKRV magic")]
    BadMagic,
    /// The WKRV version is unsupported.
    #[error(
        "unsupported WKRV version {0}; version 1 does not record its YCbCr matrix and cannot be decoded unambiguously, so re-record with version 2"
    )]
    UnsupportedVersion(u8),
    /// The pixel-format byte is not supported.
    #[error("unknown WKRV pixel format {0}")]
    UnknownPixelFormat(u8),
    /// The matrix code is invalid for the pixel format.
    #[error("invalid WKRV matrix code {code} for pixel format {pixel_format}")]
    InvalidMatrix {
        /// Pixel-format byte from the header.
        pixel_format: u8,
        /// Unsupported H.273 matrix code.
        code: u8,
    },
    /// The range code is invalid for the pixel format.
    #[error("invalid WKRV range code {code} for pixel format {pixel_format}")]
    InvalidRange {
        /// Pixel-format byte from the header.
        pixel_format: u8,
        /// Unsupported range code.
        code: u8,
    },
    /// The frame payload length does not match the dimensions and layout.
    #[error("WKRV payload size mismatch: expected {expected} bytes, got {actual}")]
    PayloadSize {
        /// Payload length required by the header.
        expected: u128,
        /// Payload length recorded in the stream or provided by the caller.
        actual: u128,
    },
    /// The stream ended partway through a header or record.
    #[error("truncated WKRV stream")]
    Truncated,
    /// A requested frame buffer could not be allocated.
    #[error("failed to allocate raw video frame: {0}")]
    Allocation(String),
    /// The YUV conversion library rejected the declared frame layout.
    #[error("raw video color conversion failed: {0}")]
    Conversion(String),
}

/// One frame read from a WKRV stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawVideoFrame {
    /// Presentation timestamp relative to the stream start.
    pub timestamp: Duration,
    /// Uncompressed pixels in the layout declared by the file header.
    pub data: Vec<u8>,
}

impl RawVideoFrame {
    /// Converts this frame to non-linear R′G′B′ in RGBA byte order.
    ///
    /// The conversion applies only the stored YCbCr matrix and range. WKRV
    /// does not record primaries or transfer characteristics, so this method
    /// does not convert either.
    ///
    /// # Errors
    ///
    /// Returns an error if the payload does not match the header or its NV12
    /// matrix cannot be converted.
    pub fn to_rgba(&self, header: &RawVideoHeader) -> Result<Vec<u8>, RawVideoError> {
        let expected_payload = header.payload_len();
        if self.data.len() as u128 != expected_payload {
            return Err(RawVideoError::PayloadSize {
                expected: expected_payload,
                actual: self.data.len() as u128,
            });
        }

        let (matrix, range) = match header.layout {
            RawVideoLayout::Rgba8 => return Ok(self.data.clone()),
            RawVideoLayout::Nv12 { matrix, range } => (matrix, range),
        };
        let Some((yuv_range, yuv_matrix)) = yuv_encoding(matrix, range) else {
            return Err(RawVideoError::InvalidMatrix {
                pixel_format: PIXEL_FORMAT_NV12,
                code: matrix.cicp(),
            });
        };

        let width = header.width;
        let height = header.height;
        let y_len = usize::try_from(u128::from(width) * u128::from(height))
            .map_err(|error| RawVideoError::Allocation(error.to_string()))?;
        let rgba_len = usize::try_from(u128::from(width) * u128::from(height) * 4)
            .map_err(|error| RawVideoError::Allocation(error.to_string()))?;
        let uv_stride = u32::try_from(u128::from(width.div_ceil(2)) * 2)
            .map_err(|error| RawVideoError::Conversion(error.to_string()))?;
        let rgba_stride = width
            .checked_mul(4)
            .ok_or_else(|| RawVideoError::Conversion("RGBA row stride overflow".into()))?;
        let (y_plane, uv_plane) = self.data.split_at(y_len);
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(rgba_len)
            .map_err(|error| RawVideoError::Allocation(error.to_string()))?;
        rgba.resize(rgba_len, 0);

        yuv::yuv_nv12_to_rgba(
            &YuvBiPlanarImage {
                y_plane,
                y_stride: width,
                uv_plane,
                uv_stride,
                width,
                height,
            },
            &mut rgba,
            rgba_stride,
            yuv_range,
            yuv_matrix,
            YuvConversionMode::Balanced,
        )
        .map_err(|error| RawVideoError::Conversion(error.to_string()))?;
        Ok(rgba)
    }
}

/// Sequential reader for versioned WKRV raw-video files.
#[derive(Debug)]
pub struct RawVideoReader<R: Read> {
    reader: R,
    header: RawVideoHeader,
}

impl RawVideoReader<File> {
    /// Opens a WKRV file from disk.
    ///
    /// # Errors
    ///
    /// Returns an I/O or header-validation error.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RawVideoError> {
        Self::new(File::open(path)?)
    }
}

impl<R: Read> RawVideoReader<R> {
    /// Creates a reader and parses its WKRV header.
    ///
    /// # Errors
    ///
    /// Returns an I/O, truncation, or header-validation error.
    pub fn new(mut reader: R) -> Result<Self, RawVideoError> {
        let mut bytes = [0; RawVideoHeader::LEN];
        read_exact(&mut reader, &mut bytes)?;
        let header = RawVideoHeader::parse(&bytes)?;
        Ok(Self { reader, header })
    }

    /// Returns the validated stream header.
    #[must_use]
    pub const fn header(&self) -> &RawVideoHeader {
        &self.header
    }

    /// Reads the next complete frame, returning `None` only at clean EOF
    /// between records.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O failures, truncated records, or payload
    /// lengths that disagree with the header.
    pub fn next_frame(&mut self) -> Result<Option<RawVideoFrame>, RawVideoError> {
        let mut record_header = [0; 12];
        loop {
            match self.reader.read(&mut record_header[..1]) {
                Ok(0) => return Ok(None),
                Ok(1) => break,
                Ok(_) => unreachable!("a one-byte buffer cannot return multiple bytes"),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(RawVideoError::Io(error)),
            }
        }
        read_exact(&mut self.reader, &mut record_header[1..])?;

        let timestamp_ns = u64::from_le_bytes([
            record_header[0],
            record_header[1],
            record_header[2],
            record_header[3],
            record_header[4],
            record_header[5],
            record_header[6],
            record_header[7],
        ]);
        let payload_len = u32::from_le_bytes([
            record_header[8],
            record_header[9],
            record_header[10],
            record_header[11],
        ]);
        let expected = self.header.payload_len();
        if u128::from(payload_len) != expected {
            return Err(RawVideoError::PayloadSize {
                expected,
                actual: u128::from(payload_len),
            });
        }

        let payload_len = usize::try_from(payload_len)
            .map_err(|error| RawVideoError::Allocation(error.to_string()))?;
        let mut data = Vec::new();
        data.try_reserve_exact(payload_len)
            .map_err(|error| RawVideoError::Allocation(error.to_string()))?;
        data.resize(payload_len, 0);
        read_exact(&mut self.reader, &mut data)?;

        Ok(Some(RawVideoFrame {
            timestamp: Duration::from_nanos(timestamp_ns),
            data,
        }))
    }
}

fn read_exact(reader: &mut impl Read, bytes: &mut [u8]) -> Result<(), RawVideoError> {
    match reader.read_exact(bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Err(RawVideoError::Truncated),
        Err(error) => Err(RawVideoError::Io(error)),
    }
}

pub(crate) const fn yuv_encoding(
    matrix: MatrixCoefficients,
    range: ColorRange,
) -> Option<(YuvRange, YuvStandardMatrix)> {
    let matrix = match matrix {
        MatrixCoefficients::Bt601 => YuvStandardMatrix::Bt601,
        MatrixCoefficients::Bt709 => YuvStandardMatrix::Bt709,
        MatrixCoefficients::Bt2020NonConstantLuminance => YuvStandardMatrix::Bt2020,
        MatrixCoefficients::Bt2020ConstantLuminance => return None,
    };
    let range = match range {
        ColorRange::Limited => YuvRange::Limited,
        ColorRange::Full => YuvRange::Full,
    };
    Some((range, matrix))
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, time::Duration};

    use super::{RawVideoError, RawVideoFrame, RawVideoHeader, RawVideoLayout, RawVideoReader};
    use crate::{ColorRange, MatrixCoefficients};

    fn header(layout: RawVideoLayout) -> RawVideoHeader {
        RawVideoHeader {
            layout,
            width: 2,
            height: 2,
            fps: 30,
        }
    }

    fn encoded_stream(header: RawVideoHeader, timestamp_ns: u64, payload: &[u8]) -> Vec<u8> {
        let mut bytes = header.to_bytes().expect("supported header").to_vec();
        bytes.extend_from_slice(&timestamp_ns.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn header_roundtrips_all_supported_layouts() {
        let rgba = header(RawVideoLayout::Rgba8);
        assert_eq!(
            RawVideoHeader::parse(&rgba.to_bytes().unwrap()).unwrap(),
            rgba
        );

        for matrix in [
            MatrixCoefficients::Bt601,
            MatrixCoefficients::Bt709,
            MatrixCoefficients::Bt2020NonConstantLuminance,
        ] {
            for range in [ColorRange::Limited, ColorRange::Full] {
                let header = header(RawVideoLayout::Nv12 { matrix, range });
                assert_eq!(
                    RawVideoHeader::parse(&header.to_bytes().unwrap()).unwrap(),
                    header
                );
            }
        }
    }

    #[test]
    fn header_rejects_legacy_and_invalid_signals() {
        let rgba = header(RawVideoLayout::Rgba8).to_bytes().unwrap();
        let mut legacy = rgba;
        legacy[4] = 1;
        let error = RawVideoHeader::parse(&legacy).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not record its YCbCr matrix")
        );
        assert!(error.to_string().contains("re-record with version 2"));

        let mut invalid = rgba;
        invalid[..4].copy_from_slice(b"BAD!");
        assert!(matches!(
            RawVideoHeader::parse(&invalid),
            Err(RawVideoError::BadMagic)
        ));

        let mut invalid = rgba;
        invalid[5] = 1;
        assert!(matches!(
            RawVideoHeader::parse(&invalid),
            Err(RawVideoError::UnknownPixelFormat(1))
        ));

        for (matrix, range, expected_error) in [(1, 1, "matrix"), (0, 0, "range")] {
            let mut invalid = rgba;
            invalid[6] = matrix;
            invalid[7] = range;
            let error = RawVideoHeader::parse(&invalid).unwrap_err();
            assert!(error.to_string().contains(expected_error));
        }

        for code in [0, 2, 5, 10, 255] {
            let mut invalid = header(RawVideoLayout::Nv12 {
                matrix: MatrixCoefficients::Bt601,
                range: ColorRange::Limited,
            })
            .to_bytes()
            .unwrap();
            invalid[6] = code;
            assert!(matches!(
                RawVideoHeader::parse(&invalid),
                Err(RawVideoError::InvalidMatrix { code: actual, .. }) if actual == code
            ));
        }

        let mut invalid_range = header(RawVideoLayout::Nv12 {
            matrix: MatrixCoefficients::Bt709,
            range: ColorRange::Limited,
        })
        .to_bytes()
        .unwrap();
        invalid_range[7] = 2;
        assert!(matches!(
            RawVideoHeader::parse(&invalid_range),
            Err(RawVideoError::InvalidRange { code: 2, .. })
        ));
    }

    #[test]
    fn nv12_writer_rejects_constant_luminance() {
        let header = header(RawVideoLayout::Nv12 {
            matrix: MatrixCoefficients::Bt2020ConstantLuminance,
            range: ColorRange::Limited,
        });
        assert!(matches!(
            header.to_bytes(),
            Err(RawVideoError::InvalidMatrix {
                pixel_format: 3,
                code: 10
            })
        ));
    }

    #[test]
    fn reader_decodes_nv12_with_its_stored_matrix_and_range() {
        let samples = [81, 81, 81, 81, 90, 240];
        let cases = [
            (MatrixCoefficients::Bt601, ColorRange::Full),
            (MatrixCoefficients::Bt709, ColorRange::Limited),
            (
                MatrixCoefficients::Bt2020NonConstantLuminance,
                ColorRange::Limited,
            ),
        ];
        let mut results = Vec::new();
        for (matrix, range) in cases {
            let header = header(RawVideoLayout::Nv12 { matrix, range });
            let mut reader =
                RawVideoReader::new(Cursor::new(encoded_stream(header, 1234, &samples)))
                    .expect("valid NV12 header");
            let frame = reader.next_frame().unwrap().unwrap();
            assert_eq!(frame.timestamp, Duration::from_nanos(1234));
            let rgba = frame.to_rgba(reader.header()).expect("NV12 converts");
            let expected = expected_rgb(matrix, range, 81, 90, 240);
            let (pixels, remainder) = rgba.as_chunks::<4>();
            assert_eq!(remainder, []);
            for pixel in pixels {
                assert!(expected[0].mul_add(-255.0, f64::from(pixel[0])).abs() <= 1.0);
                assert!(expected[1].mul_add(-255.0, f64::from(pixel[1])).abs() <= 1.0);
                assert!(expected[2].mul_add(-255.0, f64::from(pixel[2])).abs() <= 1.0);
                assert_eq!(pixel[3], 255);
            }
            results.push(rgba);
            assert!(reader.next_frame().unwrap().is_none());
        }
        assert_ne!(results[0], results[1]);
        assert_ne!(results[1], results[2]);
        assert_ne!(results[0], results[2]);
    }

    #[test]
    fn reader_rejects_truncated_and_wrong_sized_payloads() {
        let rgba_header = header(RawVideoLayout::Rgba8);
        let mut truncated = rgba_header.to_bytes().unwrap().to_vec();
        truncated.extend_from_slice(&0u64.to_le_bytes());
        truncated.extend_from_slice(&16u32.to_le_bytes());
        truncated.extend_from_slice(&[1, 2]);
        let mut reader = RawVideoReader::new(Cursor::new(truncated)).unwrap();
        assert!(matches!(reader.next_frame(), Err(RawVideoError::Truncated)));

        let mut wrong_size = rgba_header.to_bytes().unwrap().to_vec();
        wrong_size.extend_from_slice(&0u64.to_le_bytes());
        wrong_size.extend_from_slice(&15u32.to_le_bytes());
        let mut reader = RawVideoReader::new(Cursor::new(wrong_size)).unwrap();
        assert!(matches!(
            reader.next_frame(),
            Err(RawVideoError::PayloadSize {
                expected: 16,
                actual: 15
            })
        ));
    }

    #[test]
    fn rgba_frame_returns_pixels_unchanged() {
        let header = header(RawVideoLayout::Rgba8);
        let data = [1, 2, 3, 4].repeat(4);
        let frame = RawVideoFrame {
            timestamp: Duration::ZERO,
            data: data.clone(),
        };
        assert_eq!(frame.to_rgba(&header).unwrap(), data);
    }

    fn expected_rgb(
        matrix: MatrixCoefficients,
        range: ColorRange,
        y: u8,
        cb: u8,
        cr: u8,
    ) -> [f64; 3] {
        let (luma, blue_difference, red_difference) = match range {
            ColorRange::Limited => (
                (f64::from(y) - 16.0) / 219.0,
                (f64::from(cb) - 128.0) / 224.0,
                (f64::from(cr) - 128.0) / 224.0,
            ),
            ColorRange::Full => (
                f64::from(y) / 255.0,
                (f64::from(cb) - 128.0) / 255.0,
                (f64::from(cr) - 128.0) / 255.0,
            ),
        };
        let (red_cr_factor, green_blue_factor, green_red_factor, blue_cb_factor): (
            f64,
            f64,
            f64,
            f64,
        ) = match matrix {
            MatrixCoefficients::Bt601 => (1.402, 0.344_136, 0.714_136, 1.772),
            MatrixCoefficients::Bt709 => (1.5748, 0.187_324, 0.468_124, 1.8556),
            MatrixCoefficients::Bt2020NonConstantLuminance => {
                (1.4746, 0.164_553, 0.571_353, 1.8814)
            }
            MatrixCoefficients::Bt2020ConstantLuminance => unreachable!(),
        };
        [
            red_cr_factor.mul_add(red_difference, luma),
            green_red_factor.mul_add(
                -red_difference,
                green_blue_factor.mul_add(-blue_difference, luma),
            ),
            blue_cb_factor.mul_add(blue_difference, luma),
        ]
        .map(|channel| channel.clamp(0.0, 1.0))
    }
}
