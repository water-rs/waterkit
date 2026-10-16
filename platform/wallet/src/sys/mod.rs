#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod apple;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod unsupported;

#[cfg(target_os = "android")]
pub use android::{add, capabilities};
#[cfg(target_os = "ios")]
pub use apple::{add, capabilities};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use unsupported::capabilities;
