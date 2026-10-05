//! Platform-specific haptic implementations.

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

// Re-export platform implementations
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::{
    impact, is_available, notification_error, notification_success, notification_warning,
    play_pattern, selection,
};

#[cfg(target_os = "android")]
pub use android::{
    impact, is_available, notification_error, notification_success, notification_warning,
    play_pattern, selection,
};

#[cfg(target_os = "windows")]
pub use windows::{
    impact, is_available, notification_error, notification_success, notification_warning,
    play_pattern, selection,
};

#[cfg(target_os = "linux")]
pub use linux::{
    impact, is_available, notification_error, notification_success, notification_warning,
    play_pattern, selection,
};

// Fallback for unsupported platforms
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
mod fallback {
    use crate::{HapticError, HapticPattern, Intensity};

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn is_available() -> bool {
        false
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn impact(_intensity: Intensity) -> Result<(), HapticError> {
        Err(HapticError::Unsupported)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn selection() -> Result<(), HapticError> {
        Err(HapticError::Unsupported)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn notification_success() -> Result<(), HapticError> {
        Err(HapticError::Unsupported)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn notification_warning() -> Result<(), HapticError> {
        Err(HapticError::Unsupported)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn notification_error() -> Result<(), HapticError> {
        Err(HapticError::Unsupported)
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
    )]
    pub fn play_pattern(_pattern: &HapticPattern) -> Result<(), HapticError> {
        Err(HapticError::Unsupported)
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
