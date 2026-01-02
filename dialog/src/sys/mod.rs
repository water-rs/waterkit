#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use desktop::{
    Selection, load_media, show_alert, show_confirm, show_open_single_file, show_photo_picker,
};

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::{Selection, load_media, show_alert, show_confirm, show_photo_picker};

#[cfg(target_os = "android")]
pub async fn show_open_single_file(
    _: crate::FileDialog,
) -> Result<Option<std::path::PathBuf>, String> {
    Err("File picker not supported on Android yet".to_string())
}

#[cfg(target_os = "ios")]
mod apple;
#[cfg(target_os = "ios")]
pub use apple::{NativeHandle, load_media, show_alert, show_confirm, show_photo_picker};

#[cfg(target_os = "ios")]
pub async fn show_open_single_file(
    _: crate::FileDialog,
) -> Result<Option<std::path::PathBuf>, String> {
    Err("File picker not supported on iOS yet".to_string())
}
