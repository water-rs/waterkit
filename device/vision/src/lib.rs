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

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![forbid(unsafe_code)]

mod capability;
mod error;
mod geometry;
mod image;
mod request;
mod sealed;
mod selection;
#[cfg(test)]
mod test_support;
mod vision;

pub use capability::{Portable, RealizationSet, VisionCapabilities};
pub use error::VisionError;
pub use geometry::{Point, Quad};
pub use image::Image;
pub use request::Request;
pub use vision::{Policy, Vision};
pub use waterkit_core::Orientation;
pub use wgpu;
