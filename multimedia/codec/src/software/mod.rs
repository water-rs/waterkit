//! Software codec fallback implementations.
//!
//! These are used when hardware acceleration is unavailable.
//! Not exposed in public API - users interact only with the unified Decoder/Encoder.

// Software fallback only available on desktop platforms (where rav1e/rav1d are enabled)
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
pub mod av1;
