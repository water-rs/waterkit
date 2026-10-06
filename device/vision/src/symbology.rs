//! The barcode symbology vocabulary shared by every realization.
//!
//! The enum is the full vocabulary the crate names; each platform
//! realization expresses the subset it serves. A request for a symbology the
//! serving realization cannot express fails with
//! [`VisionError::Unsupported`](crate::VisionError), naming it.

/// A barcode symbology.
// `repr` is the bitset storage of `EnumSet<Symbology>`: `u16` only holds 16
// variants; the vocabulary already has 20, so `u32` is the smallest that fits.
#[derive(Debug, enumset::EnumSetType, Hash, PartialOrd, Ord)]
#[enumset(repr = "u32")]
#[non_exhaustive]
pub enum Symbology {
    /// Aztec 2D.
    Aztec,
    /// Codabar.
    Codabar,
    /// Code 39.
    Code39,
    /// Code 93.
    Code93,
    /// Code 128.
    Code128,
    /// Data Matrix 2D.
    DataMatrix,
    /// EAN-8.
    Ean8,
    /// EAN-13.
    Ean13,
    /// `GS1 DataBar` (`RSS-14`).
    Gs1DataBar,
    /// `GS1 DataBar` expanded.
    Gs1DataBarExpanded,
    /// `GS1 DataBar` limited.
    Gs1DataBarLimited,
    /// Interleaved 2 of 5.
    Itf,
    /// ITF-14.
    Itf14,
    /// `MicroPDF417`.
    MicroPdf417,
    /// Micro QR.
    MicroQr,
    /// MSI Plessey.
    MsiPlessey,
    /// PDF417.
    Pdf417,
    /// QR code.
    Qr,
    /// UPC-A.
    UpcA,
    /// UPC-E.
    UpcE,
}
