#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use apple::*;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::*;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod desktop;

#[cfg(any(test, target_os = "windows", target_os = "linux"))]
mod network;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use desktop::*;

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
compile_error!("waterkit-system supports only macOS, iOS, Android, Windows, and Linux.");

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub(crate) fn connectivity() -> Result<crate::ConnectivityInfo, crate::SystemError> {
    panic!("waterkit-system supports only macOS, iOS, Android, Windows, and Linux.")
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub(crate) fn thermal_state() -> Result<Option<crate::ThermalState>, crate::SystemError> {
    panic!("waterkit-system supports only macOS, iOS, Android, Windows, and Linux.")
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub(crate) fn load() -> Result<crate::SystemLoad, crate::SystemError> {
    panic!("waterkit-system supports only macOS, iOS, Android, Windows, and Linux.")
}
