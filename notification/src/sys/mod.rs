#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub use android::show_notification;

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub mod desktop;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub use desktop::show_notification;

#[cfg(target_os = "ios")]
pub mod apple;
#[cfg(target_os = "ios")]
pub use apple::show_notification;
