use async_trait::async_trait;

use crate::{
    AuthenticateOptions, AuthenticationResult, Availability, PasskeyError, RegisterOptions,
    RegistrationResult,
};

#[async_trait]
pub trait PasskeyBackend {
    async fn is_available(&self) -> Result<Availability, PasskeyError>;
    async fn register(&self, options: &RegisterOptions)
    -> Result<RegistrationResult, PasskeyError>;
    async fn authenticate(
        &self,
        options: &AuthenticateOptions,
    ) -> Result<AuthenticationResult, PasskeyError>;
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::PlatformBackend;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::PlatformBackend;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::PlatformBackend;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::PlatformBackend;

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
mod unsupported;
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub use unsupported::PlatformBackend;

pub const fn platform_backend() -> PlatformBackend {
    PlatformBackend
}
