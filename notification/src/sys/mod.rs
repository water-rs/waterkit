#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub use android::{show_notification, NotificationHandleInner};

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod desktop;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use desktop::{show_notification, NotificationHandleInner};

// iOS: Always use native UserNotifications framework
#[cfg(target_os = "ios")]
mod apple;
#[cfg(target_os = "ios")]
pub use apple::{show_notification, NotificationHandleInner};

// macOS: Use native UserNotifications for bundled apps (supports actions),
// fall back to notify-rust for CLI/unbundled apps (no actions)
#[cfg(target_os = "macos")]
mod apple;
#[cfg(target_os = "macos")]
mod desktop;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{show_notification, NotificationHandleInner};
