//! Error type for all fallible public APIs in this crate.

/// Errors produced by `waterkit-vision`.
///
/// Variants are grouped by subsystem. `#[non_exhaustive]` keeps new failure
/// modes additive in future releases.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum VisionError {
    /// A [`crate::CpuFrame`] description was inconsistent or unsupported:
    /// stride shorter than the pixel width, a plane count that does not
    /// match the format, or zero-size dimensions.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),

    /// An invalid argument was passed to a constructor or method.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// The pixel format of a supplied frame cannot be consumed by the
    /// requested operation.
    #[error("unsupported pixel format: {0}")]
    UnsupportedFormat(String),

    /// A scanner operation was requested after the scanner was stopped.
    #[error("scanner is closed")]
    ScannerClosed,

    /// Escape hatch for platform or IO failures surfaced through vision APIs.
    #[error("platform error: {0}")]
    Platform(String),
}
