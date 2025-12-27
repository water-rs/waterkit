#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use desktop::{
    load_media, show_alert, show_confirm, show_open_single_file, show_photo_picker, Selection,
};

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::{load_media, show_alert, show_confirm, show_photo_picker, Selection};

#[cfg(target_os = "android")]
pub async fn show_open_single_file(
    _: crate::FileDialog,
) -> Result<Option<std::path::PathBuf>, String> {
    Err("File picker not supported on Android yet".to_string())
}

#[cfg(target_os = "ios")]
mod apple;
#[cfg(target_os = "ios")]
pub use apple::{load_media, show_alert, show_confirm, show_photo_picker, NativeHandle};

#[cfg(target_os = "ios")]
pub async fn show_open_single_file(
    _: crate::FileDialog,
) -> Result<Option<std::path::PathBuf>, String> {
    Err("File picker not supported on iOS yet".to_string())
}
