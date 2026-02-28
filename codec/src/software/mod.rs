//! Software codec fallback implementations.
//!
//! These are used when hardware acceleration is unavailable.
//! Not exposed in public API - users interact only with the unified Decoder/Encoder.

#[cfg(feature = "software-fallback")]
pub mod av1;
