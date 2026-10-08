use bytes::Bytes;

use crate::{Quad, Symbology};

/// A decoded barcode with its symbology, payload and geometry — the output
/// of a `DetectBarcodes` request, where the bounds always exist.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Barcode {
    pub(crate) symbology: Symbology,
    pub(crate) payload: Payload,
    pub(crate) bounds: Quad,
}

impl Barcode {
    /// The symbology the payload was decoded as.
    #[must_use]
    pub const fn symbology(&self) -> Symbology {
        self.symbology
    }

    /// The decoded payload.
    #[must_use]
    pub const fn payload(&self) -> &Payload {
        &self.payload
    }

    /// The detected code's geometry, normalized to the analyzed image.
    #[must_use]
    pub const fn bounds(&self) -> Quad {
        self.bounds
    }
}

/// The decoded content of a barcode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub(crate) bytes: Bytes,
}

impl Payload {
    /// The decoded bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The payload as UTF-8 text, when the bytes are valid UTF-8.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}

impl AsRef<[u8]> for Payload {
    fn as_ref(&self) -> &[u8] {
        self.bytes()
    }
}
