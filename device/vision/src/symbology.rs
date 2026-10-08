/// A barcode symbology a vision request may ask for or report.
///
/// This is the crate's shared vocabulary: `DetectBarcodes` realizations and
/// [`CodeScanner`] alike name their formats with it. Each platform maps the
/// symbologies it can express — Apple `Vision` recognizes all of them, the
/// Google code scanner carries a subset — and a request for a symbology the
/// serving realization cannot express fails with
/// [`VisionError::Unsupported`], naming it.
///
/// [`CodeScanner`]: crate::CodeScanner
/// [`VisionError::Unsupported`]: crate::VisionError::Unsupported
#[derive(Debug, enumset::EnumSetType, Hash, PartialOrd, Ord)]
#[enumset(repr = "u32")]
#[non_exhaustive]
pub enum Symbology {
    /// Aztec Code.
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
    /// GS1 `DataBar` (RSS-14).
    Gs1DataBar,
    /// GS1 `DataBar` Expanded.
    Gs1DataBarExpanded,
    /// GS1 `DataBar` Limited.
    Gs1DataBarLimited,
    /// Interleaved 2 of 5.
    Itf,
    /// ITF-14, the 14-digit GS1 Interleaved 2 of 5.
    Itf14,
    /// Micro PDF417.
    MicroPdf417,
    /// Micro QR Code.
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
