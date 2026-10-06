use bytes::Bytes;

use crate::{Quad, Symbology};

/// A decoded barcode with its symbology, payload and geometry.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Barcode {
    pub(crate) symbology: Symbology,
    pub(crate) payload: Payload,
    pub(crate) bounds: Option<Quad>,
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

    /// The detected code's geometry, when the serving realization reports
    /// it.
    ///
    /// Frame-scanning realizations report the quad normalized to the
    /// analyzed image. The Android system scanner's corner points are in
    /// the coordinates of an internal camera frame the caller never sees,
    /// so it reports `None`; the iOS scanner normalizes to the presented
    /// view and reports `Some`.
    #[must_use]
    pub const fn bounds(&self) -> Option<Quad> {
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
