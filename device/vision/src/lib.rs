//! # waterkit-vision
//!
//! On-device vision primitives for the Waterkit ecosystem.
//!
//! ## Implemented
//!
//! * **Linear barcodes** — EAN-13 and UPC-A (as the EAN-13-with-leading-zero
//!   subset) decoded by an in-crate engine: adaptive binarization, guard
//!   pattern and digit decoding with per-element tolerance, quiet-zone and
//!   mod-10 checksum validation, multi-code results deterministically
//!   ordered by source geometry. No heuristic fallbacks: a row that fails
//!   validation is rejected with a structured [`RejectReason`].
//! * **Frame types** — [`CpuFrame`]: a borrowed, typed CPU plane view with
//!   stride, format, dimensions, timestamp and orientation metadata.
//! * **Stream API** — [`BarcodeScanner`]: bounded in-flight work (latest
//!   frame wins), explicit pacing, deterministic cancellation and temporal
//!   deduplication/tracking of individual codes.
//!
//! ## Not implemented (explicit)
//!
//! QR Code, Data Matrix, Aztec, PDF417, EAN-8, UPC-E, Code 128, Code 39,
//! ITF and all other symbologies are **not** decoded: they are absent from
//! [`Formats`], so they cannot be requested. GPU frame input, the
//! `waterkit-camera` adapter and the OCR pipeline are not part of this
//! build.
//!
//! ## Threading
//!
//! [`BarcodeEngine`] is an immutable value type safe to share across
//! threads. [`BarcodeScanner`] owns a dedicated worker thread; frame
//! submission and event delivery go through bounded `async-channel`
//! queues — the public API never blocks an async executor and never
//! exposes `Arc<Mutex>` state.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![forbid(unsafe_code)]

pub mod barcode;
mod error;
mod frame;
mod geometry;
mod image;
mod stream;

pub use barcode::{
    Barcode, BarcodeEngine, DecodeAttempt, DecodeOptions, DecodeReport, Evidence, Formats,
    RejectReason, ScanAxes, Symbology,
};
pub use error::VisionError;
pub use frame::{CpuFrame, FrameBuf, FrameFormat, Orientation, Plane};
pub use geometry::{Homography, Point, Quadrilateral};
pub use stream::{BarcodeScanner, ScanConfig, ScanEvent, TrackId, TrackedBarcode};
