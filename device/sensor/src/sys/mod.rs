//! Platform-specific sensor implementations.
//!
//! Each platform returns its concrete stream type so subscription does not
//! require an otherwise unnecessary stream box.

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;

/// Android platform implementation.
#[cfg(target_os = "android")]
pub mod android;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

// Re-export platform implementations
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::*;

#[cfg(target_os = "android")]
pub use android::*;

#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "linux")]
pub use linux::*;

// Fallback for unsupported platforms
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
mod fallback {
    use crate::{ScalarData, SensorData, SensorError};
    use futures::stream;

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn accelerometer_available() -> bool {
        false
    }
    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
        Err(SensorError::NotAvailable)
    }
    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn accelerometer_watch(
        _interval_ms: u32,
    ) -> Result<stream::Empty<Result<SensorData, SensorError>>, SensorError> {
        Err(SensorError::NotAvailable)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn gyroscope_available() -> bool {
        false
    }
    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
        Err(SensorError::NotAvailable)
    }
    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn gyroscope_watch(
        _interval_ms: u32,
    ) -> Result<stream::Empty<Result<SensorData, SensorError>>, SensorError> {
        Err(SensorError::NotAvailable)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn magnetometer_available() -> bool {
        false
    }
    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
        Err(SensorError::NotAvailable)
    }
    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn magnetometer_watch(
        _interval_ms: u32,
    ) -> Result<stream::Empty<Result<SensorData, SensorError>>, SensorError> {
        Err(SensorError::NotAvailable)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn barometer_available() -> bool {
        false
    }
    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn barometer_read() -> Result<ScalarData, SensorError> {
        Err(SensorError::NotAvailable)
    }
    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn barometer_watch(
        _interval_ms: u32,
    ) -> Result<stream::Empty<Result<ScalarData, SensorError>>, SensorError> {
        Err(SensorError::NotAvailable)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn ambient_light_available() -> bool {
        false
    }
    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn ambient_light_read() -> Result<ScalarData, SensorError> {
        Err(SensorError::NotAvailable)
    }
    #[expect(
        clippy::unused_async,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn ambient_light_watch(
        _interval_ms: u32,
    ) -> Result<stream::Empty<Result<ScalarData, SensorError>>, SensorError> {
        Err(SensorError::NotAvailable)
    }
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub use fallback::*;
