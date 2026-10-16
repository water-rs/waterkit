//! Platform-specific hardware codec implementations.

#[cfg(waterkit_hw_codec_apple)]
pub mod apple;

#[cfg(waterkit_hw_codec_android)]
pub mod android;

#[cfg(waterkit_hw_codec_windows)]
pub mod windows;

#[cfg(waterkit_hw_codec_vaapi)]
pub mod linux;
