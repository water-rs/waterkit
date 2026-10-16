//! Platform-specific implementations.

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod desktop;

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple;

#[cfg(target_os = "android")]
pub mod android;

use crate::{Error, ScreenInfo, Screenshot, screenshot::ImageFormat};

// ============================================================================
// screens()
// ============================================================================

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    desktop::screens()
}

#[cfg(target_os = "ios")]
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    apple::screens()
}

#[cfg(target_os = "android")]
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    android::screens()
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "ios",
    target_os = "android"
)))]
#[expect(
    clippy::missing_const_for_fn,
    reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
)]
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    Err(Error::Unsupported)
}

// ============================================================================
// refresh rate
// ============================================================================

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub fn max_refresh_rate_hz() -> Result<f32, Error> {
    desktop::max_refresh_rate_hz()
}

#[cfg(target_os = "ios")]
pub const fn max_refresh_rate_hz() -> Result<f32, Error> {
    Err(Error::Unsupported)
}

#[cfg(target_os = "android")]
pub fn max_refresh_rate_hz() -> Result<f32, Error> {
    android::max_refresh_rate_hz()
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "ios",
    target_os = "android"
)))]
pub const fn max_refresh_rate_hz() -> Result<f32, Error> {
    Err(Error::Unsupported)
}

// ============================================================================
// brightness
// ============================================================================

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub async fn get_brightness() -> Result<f32, Error> {
    desktop::get_brightness().await
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    desktop::set_brightness(val).await
}

#[cfg(target_os = "ios")]
pub async fn get_brightness() -> Result<f32, Error> {
    apple::get_brightness().await
}

#[cfg(target_os = "ios")]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    apple::set_brightness(val).await
}

#[cfg(target_os = "android")]
pub async fn get_brightness() -> Result<f32, Error> {
    android::get_brightness().await
}

#[cfg(target_os = "android")]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    android::set_brightness(val).await
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "ios",
    target_os = "android"
)))]
#[expect(clippy::unused_async, reason = "the facade is async on every platform")]
pub async fn get_brightness() -> Result<f32, Error> {
    Err(Error::Unsupported)
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "ios",
    target_os = "android"
)))]
#[expect(clippy::unused_async, reason = "the facade is async on every platform")]
pub async fn set_brightness(_val: f32) -> Result<(), Error> {
    Err(Error::Unsupported)
}

// ============================================================================
// screenshot
// ============================================================================

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub async fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    desktop::screenshot(display, format).await
}

#[cfg(target_os = "ios")]
pub async fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    apple::screenshot(display, format).await
}

#[cfg(target_os = "android")]
pub async fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    android::screenshot(display, format).await
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "ios",
    target_os = "android"
)))]
#[expect(clippy::unused_async, reason = "the facade is async on every platform")]
pub async fn screenshot(_display: &ScreenInfo, _format: ImageFormat) -> Result<Screenshot, Error> {
    Err(Error::Unsupported)
}

// ============================================================================
// ScreenStreamInner
// ============================================================================

#[cfg(target_os = "macos")]
pub use apple::ScreenStreamInner;

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use desktop::ScreenStreamInner;

#[cfg(target_os = "ios")]
pub use apple::ScreenStreamInner;

#[cfg(target_os = "android")]
pub use android::ScreenStreamInner;

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "ios",
    target_os = "android"
)))]
pub struct ScreenStreamInner;

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "ios",
    target_os = "android"
)))]
#[expect(
    clippy::unused_self,
    clippy::missing_const_for_fn,
    reason = "this unsupported-platform shim keeps no state and computes nothing, but the facade calls every platform's backend through the same non-const `&self` methods"
)]
impl ScreenStreamInner {
    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "wgpu's WebGPU device is not Send on wasm"
        )
    )]
    pub async fn new(
        _display: &ScreenInfo,
        _device: std::sync::Arc<wgpu::Device>,
        _queue: std::sync::Arc<wgpu::Queue>,
        _config: &crate::stream::StreamConfig,
    ) -> Result<Self, Error> {
        Err(Error::Unsupported)
    }

    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn next_frame(&self) -> Option<crate::frame::ScreenFrame> {
        None
    }

    pub fn try_next_frame(&self) -> Option<crate::frame::ScreenFrame> {
        None
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        (0, 0)
    }
}
