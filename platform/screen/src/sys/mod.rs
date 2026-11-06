//! Platform-specific implementations.

#[cfg(all(
    any(target_os = "macos", target_os = "windows", target_os = "linux"),
    any(feature = "screens", feature = "brightness")
))]
mod desktop;

#[cfg(all(
    any(target_os = "ios", target_os = "macos"),
    any(
        feature = "screens",
        feature = "brightness",
        feature = "gpu",
        feature = "display-stream"
    )
))]
pub mod apple;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(any(feature = "screens", feature = "brightness"))]
use crate::Error;
#[cfg(feature = "screens")]
use crate::ScreenInfo;
#[cfg(feature = "screenshot")]
use crate::{Screenshot, screenshot::ImageFormat};

// ============================================================================
// screens()
// ============================================================================

#[cfg(all(
    feature = "screens",
    any(target_os = "macos", target_os = "windows", target_os = "linux")
))]
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    desktop::screens()
}

#[cfg(all(feature = "screens", target_os = "ios"))]
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    apple::screens()
}

#[cfg(all(feature = "screens", target_os = "android"))]
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    android::screens()
}

#[cfg(all(
    feature = "screens",
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "ios",
        target_os = "android"
    ))
))]
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

#[cfg(all(
    feature = "screens",
    any(target_os = "macos", target_os = "windows", target_os = "linux")
))]
pub fn max_refresh_rate_hz() -> Result<f32, Error> {
    desktop::max_refresh_rate_hz()
}

#[cfg(all(feature = "screens", target_os = "ios"))]
pub const fn max_refresh_rate_hz() -> Result<f32, Error> {
    Err(Error::Unsupported)
}

#[cfg(all(feature = "screens", target_os = "android"))]
pub fn max_refresh_rate_hz() -> Result<f32, Error> {
    android::max_refresh_rate_hz()
}

#[cfg(all(
    feature = "screens",
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "ios",
        target_os = "android"
    ))
))]
pub const fn max_refresh_rate_hz() -> Result<f32, Error> {
    Err(Error::Unsupported)
}

// ============================================================================
// brightness
// ============================================================================

#[cfg(all(
    feature = "brightness",
    any(target_os = "macos", target_os = "windows", target_os = "linux")
))]
pub async fn get_brightness() -> Result<f32, Error> {
    desktop::get_brightness().await
}

#[cfg(all(
    feature = "brightness",
    any(target_os = "macos", target_os = "windows", target_os = "linux")
))]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    desktop::set_brightness(val).await
}

#[cfg(all(feature = "brightness", target_os = "ios"))]
pub async fn get_brightness() -> Result<f32, Error> {
    apple::get_brightness().await
}

#[cfg(all(feature = "brightness", target_os = "ios"))]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    apple::set_brightness(val).await
}

#[cfg(all(feature = "brightness", target_os = "android"))]
pub async fn get_brightness() -> Result<f32, Error> {
    android::get_brightness().await
}

#[cfg(all(feature = "brightness", target_os = "android"))]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    android::set_brightness(val).await
}

#[cfg(all(
    feature = "brightness",
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "ios",
        target_os = "android"
    ))
))]
#[expect(clippy::unused_async, reason = "the facade is async on every platform")]
pub async fn get_brightness() -> Result<f32, Error> {
    Err(Error::Unsupported)
}

#[cfg(all(
    feature = "brightness",
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "ios",
        target_os = "android"
    ))
))]
#[expect(clippy::unused_async, reason = "the facade is async on every platform")]
pub async fn set_brightness(_val: f32) -> Result<(), Error> {
    Err(Error::Unsupported)
}

// ============================================================================
// screenshot
// ============================================================================

#[cfg(all(
    feature = "screenshot",
    any(target_os = "macos", target_os = "windows", target_os = "linux")
))]
pub async fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    desktop::screenshot(display, format).await
}

#[cfg(all(feature = "screenshot", target_os = "ios"))]
pub async fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    apple::screenshot(display, format).await
}

#[cfg(all(feature = "screenshot", target_os = "android"))]
pub async fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    android::screenshot(display, format).await
}

#[cfg(all(
    feature = "screenshot",
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "ios",
        target_os = "android"
    ))
))]
#[expect(clippy::unused_async, reason = "the facade is async on every platform")]
pub async fn screenshot(_display: &ScreenInfo, _format: ImageFormat) -> Result<Screenshot, Error> {
    Err(Error::Unsupported)
}

// ============================================================================
// DisplayStreamInner / PixelBufferInner
// ============================================================================

#[cfg(all(feature = "display-stream", target_os = "macos"))]
pub use apple::{DisplayStreamInner, PixelBufferInner};

#[cfg(all(feature = "display-stream", not(target_os = "macos")))]
pub use unsupported_display::{DisplayStreamInner, PixelBufferInner};

/// Display streams exist only on macOS; elsewhere neither type can be
/// constructed.
#[cfg(all(feature = "display-stream", not(target_os = "macos")))]
mod unsupported_display {
    use core::convert::Infallible;
    use core::time::Duration;

    use crate::Error;
    use crate::display_stream::{DisplayStreamConfig, PixelBuffer, PixelFormat};

    #[derive(Debug)]
    pub struct DisplayStreamInner(Infallible);

    impl DisplayStreamInner {
        #[expect(
            clippy::unused_async,
            reason = "the facade is async; macOS awaits ScreenCaptureKit here"
        )]
        pub async fn new(_display_id: u32, config: DisplayStreamConfig) -> Result<Self, Error> {
            // The size, rate and format only matter to a backend that captures.
            let DisplayStreamConfig {
                width: _width,
                height: _height,
                frame_rate: _frame_rate,
                pixel_format: _pixel_format,
            } = config;
            Err(Error::Unsupported)
        }

        pub const fn frames(&self) -> async_channel::Receiver<PixelBuffer> {
            match self.0 {}
        }

        #[expect(
            clippy::unused_async,
            reason = "the facade is async; macOS awaits ScreenCaptureKit here"
        )]
        pub async fn resume(&self) -> Result<(), Error> {
            match self.0 {}
        }

        #[expect(
            clippy::unused_async,
            reason = "the facade is async; macOS awaits ScreenCaptureKit here"
        )]
        pub async fn pause(&self) -> Result<(), Error> {
            match self.0 {}
        }
    }

    #[derive(Debug)]
    pub struct PixelBufferInner(Infallible);

    impl PixelBufferInner {
        pub const fn width(&self) -> u32 {
            match self.0 {}
        }

        pub const fn height(&self) -> u32 {
            match self.0 {}
        }

        pub const fn pixel_format(&self) -> PixelFormat {
            match self.0 {}
        }

        pub const fn presentation_time(&self) -> Duration {
            match self.0 {}
        }
    }
}

// ============================================================================
// ScreenStreamInner
// ============================================================================

#[cfg(all(feature = "gpu", any(target_os = "macos", target_os = "ios")))]
pub use apple::ScreenStreamInner;

#[cfg(all(feature = "gpu", any(target_os = "windows", target_os = "linux")))]
pub use desktop::ScreenStreamInner;

#[cfg(all(feature = "gpu", target_os = "android"))]
pub use android::ScreenStreamInner;

#[cfg(all(
    feature = "gpu",
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "ios",
        target_os = "android"
    ))
))]
pub use unsupported_stream::ScreenStreamInner;

#[cfg(all(
    feature = "gpu",
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "ios",
        target_os = "android"
    ))
))]
mod unsupported_stream {
    use crate::{Error, ScreenInfo};

    pub struct ScreenStreamInner;

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
}
