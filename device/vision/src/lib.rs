//! Hardware-accelerated on-device vision requests over camera frames and still images.
//!
//! A request is served by one realization: either a native OS framework or
//! preinstalled system service, or a portable implementation. Native
//! realizations are thin clients for services such as Apple Vision, Play
//! services ML Kit, or Windows.Media.Ocr. Portable realizations combine a
//! `wgpu` pipeline with pure-Rust decoders or open models compiled into the
//! application by `portable-*` features selected by the `water` CLI from
//! `Water.toml`.
//!
//! Selection follows four rules:
//!
//! 1. Native serves only when it is present on this device and supports the
//!    request exactly, including every requested symbology, language, and
//!    option.
//! 2. Otherwise, portable serves if the application carries it; if not, the
//!    request returns [`VisionError::Unsupported`] naming what was missing.
//!    A request is never split across realizations.
//! 3. If a selected native realization fails, its error is returned without a
//!    retry through the portable path.
//! 4. Every selection is logged through `tracing` and can be queried up front
//!    with [`Vision::capabilities`].
//!
//! [`Policy::PortableOnly`] forces portable selection. In this policy,
//! [`Vision::with_policy`] panics if any enabled capability's portable
//! realization is not carried by the application.
//!
//! Request futures are Send on native targets (`wgpu::WasmNotSend`); wasm32
//! does not impose a `Send` requirement.
//!
//! Requests compose as tuples, including nested tuples:
//!
//! ```ignore
//! let (barcodes, text) =
//!     vision.perform(&image, &(detect_barcodes, recognize_text)).await?;
//! ```
//!
//! These request types arrive with their corresponding capabilities.
//!
//! For a live stream, keep only the newest frames while inference is busy:
//!
//! ```ignore
//! camera.frames().then(|f| vision.perform(&Image::from(&f), &request))
//! ```
//!
//! Camera frame channels are already newest-wins (`bounded(1)` plus
//! `force_send`), so this `then` pattern never queues stale frames and needs no
//! extra channel.
//!
//! The crate also ships the one-shot system code scanner behind the `scanner`
//! feature: [`CodeScanner`] presents the platform's own scanning UI — the
//! Google code scanner of Google Play services on Android (no camera
//! permission required) and `VisionKit`'s `DataScannerViewController` on iOS —
//! and resolves to the decoded [`Barcode`]. macOS, Windows and Linux have no
//! system scanner; [`CodeScanner::capabilities`] reports it unavailable there
//! and [`CodeScanner::scan`] is an error, never a fallback: `WaterUI` owns
//! the fallback scanning view.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
// Apple bridges reach `wgpu`'s hal handles and mark `CVPixelBuffer`
// thread-boundaries; Android's scanner JNI export is the one unsafe
// attribute a `no_mangle` bridge cannot avoid. Every other target stays
// forbidding unsafe code.
#![cfg_attr(
    not(any(
        target_os = "ios",
        target_os = "macos",
        all(target_os = "android", feature = "scanner")
    )),
    forbid(unsafe_code)
)]

mod barcode;
mod capability;
#[cfg(feature = "document")]
mod document;
mod error;
mod geometry;
mod image;
mod request;
#[cfg(feature = "scanner")]
mod scanner;
mod sealed;
mod selection;
mod symbology;
mod sys;
#[cfg(test)]
mod test_support;
#[cfg(feature = "text")]
mod text;
mod vision;

#[cfg(feature = "barcode")]
pub use barcode::DetectBarcodes;
pub use barcode::{Barcode, Payload};
pub use capability::{Portable, RealizationSet, VisionCapabilities};
#[cfg(feature = "document")]
pub use document::{
    Block, DataKind, DetectedData, Document, Formula, List, ListItem, Paragraph, RecognizeDocument,
    Table, TableCell,
};
pub use enumset::EnumSet;
pub use error::VisionError;
pub use geometry::{Point, Quad};
pub use image::Image;
pub use request::Request;
#[cfg(feature = "scanner")]
pub use scanner::{CodeScanner, ScannedCode, ScannerCapabilities};
pub use symbology::Symbology;
#[cfg(feature = "text")]
pub use text::{RecognitionLevel, RecognizeText, TextLine, TextWord};
pub use vision::{Policy, Vision};
pub use waterkit_core::Orientation;
pub use wgpu;
