#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use desktop::{show_alert, show_confirm};

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::{show_alert, show_confirm};

#[cfg(target_os = "ios")]
mod apple;
#[cfg(target_os = "ios")]
pub use apple::{show_alert, show_confirm};
