/// A barcode symbology a vision request may ask for or report.
///
/// This is the crate's shared vocabulary: every variant maps onto both
/// system code scanners — the Google code scanner on Android and
/// `VisionKit`'s `DataScannerViewController` on iOS — so a [`CodeScanner`]
/// request is never split between formats it can and cannot express.
///
/// [`CodeScanner`]: crate::CodeScanner
#[derive(Debug, enumset::EnumSetType, Hash, PartialOrd, Ord)]
#[enumset(repr = "u16")]
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
    /// Interleaved 2 of 5, including ITF-14.
    Itf,
    /// PDF417.
    Pdf417,
    /// QR Code.
    Qr,
    /// UPC-A.
    UpcA,
    /// UPC-E.
    UpcE,
}
