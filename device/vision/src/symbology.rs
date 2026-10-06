//! Barcode symbologies the crate can express.

/// A barcode symbology.
///
/// This is the crate's whole vocabulary, shared by every realization: a
/// request for a symbology the serving realization cannot express fails with
/// [`crate::VisionError::Unsupported`], naming it.
#[derive(Debug, enumset::EnumSetType, Hash, PartialOrd, Ord)]
#[enumset(repr = "u32")]
#[non_exhaustive]
pub enum Symbology {
    /// Aztec.
    Aztec,
    /// Codabar.
    Codabar,
    /// Code 39.
    Code39,
    /// Code 93.
    Code93,
    /// Code 128.
    Code128,
    /// Data Matrix.
    DataMatrix,
    /// EAN-8.
    Ean8,
    /// EAN-13.
    Ean13,
    /// GS1 `DataBar`.
    Gs1DataBar,
    /// GS1 `DataBar` Expanded.
    Gs1DataBarExpanded,
    /// GS1 `DataBar` Limited.
    Gs1DataBarLimited,
    /// Interleaved 2 of 5.
    Itf,
    /// ITF-14.
    Itf14,
    /// Micro PDF417.
    MicroPdf417,
    /// Micro QR.
    MicroQr,
    /// MSI Plessey.
    MsiPlessey,
    /// PDF417.
    Pdf417,
    /// QR Code.
    Qr,
    /// UPC-A.
    UpcA,
    /// UPC-E.
    UpcE,
}
