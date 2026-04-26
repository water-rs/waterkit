#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "android", target_os = "ios"))
))]
mod desktop;
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "android", target_os = "ios"))
))]
pub use desktop::{
    Selection, load_photo_media, show_alert, show_confirm, show_open_multiple_files,
    show_open_single_file, show_photo_picker,
};

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::{
    Selection, load_photo_media, show_alert, show_confirm, show_open_multiple_files,
    show_open_single_file, show_photo_picker,
};

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub use android::{
    Selection, load_photo_media, show_alert, show_confirm, show_open_multiple_files,
    show_open_single_file, show_photo_picker,
};

#[cfg(target_os = "ios")]
mod apple;
#[cfg(target_os = "ios")]
pub use apple::{
    Selection, load_photo_media, show_alert, show_confirm, show_open_multiple_files,
    show_open_single_file, show_photo_picker,
};
