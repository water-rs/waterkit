//! Platform-specific integrations.
//!
//! - Audio playback: handled by rodio across all platforms
//! - Media center: platform-specific "Now Playing" integration
//! - Recording: cpal on every supported platform

#[cfg(feature = "recording")]
mod desktop_record;
#[cfg(feature = "recording")]
pub use desktop_record::AudioRecorderInner;

#[cfg(all(feature = "media-session", any(target_os = "ios", target_os = "macos")))]
mod apple;

#[cfg(all(feature = "media-session", target_os = "android"))]
mod android;

#[cfg(all(feature = "media-session", target_os = "windows"))]
mod windows;

#[cfg(all(feature = "media-session", target_os = "linux"))]
mod linux;

#[cfg(all(feature = "media-session", target_arch = "wasm32"))]
mod web;

#[cfg(all(
    feature = "media-session",
    not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "android",
        target_os = "windows",
        target_os = "linux",
        target_arch = "wasm32"
    ))
))]
compile_error!("waterkit-audio supports only macOS, iOS, Android, Windows, and Linux.");

#[cfg(all(feature = "media-session", any(target_os = "ios", target_os = "macos")))]
pub use apple::MediaSessionInner;

#[cfg(all(feature = "playback", target_os = "ios"))]
pub use apple::{NativeAudioPlayerInner, NativeAudioPlayerState};

#[cfg(all(feature = "media-session", target_os = "android"))]
pub use android::MediaSessionInner;

#[cfg(all(feature = "media-session", target_os = "windows"))]
pub use windows::MediaSessionInner;

#[cfg(all(feature = "media-session", target_os = "linux"))]
pub use linux::MediaSessionInner;

#[cfg(all(feature = "media-session", target_arch = "wasm32"))]
pub use web::MediaSessionInner;
