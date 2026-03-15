//! Cross-platform native dialogs/alerts.
//!
//! This crate provides a unified API for displaying native UI elements:
//! - Alerts ([`Dialog`])
//! - Confirmations ([`Dialog::show_confirm`])
//! - File Open/Save Dialogs ([`FileDialog`])
//! - Photo Picker ([`PhotoPicker`])
//!
//! Platforms supported:
//! - macOS (via `rfd` / `AppKit`)
//! - Android (via JNI / Kotlin)
//! - iOS (via Swift Bridge / `UIKit`)
//!
//! ## Android
//!
//! On Android, the async APIs automatically use `ndk-context` for `Context` lookup.
//! The [`android`] module still exposes explicit JNI entry points for advanced integration.

#![warn(missing_docs)]

// Internal platform-specific implementations.
mod sys;

/// Android-specific JNI functions for dialog handling.
///
/// Use these functions when you already have an Android `Context` via JNI.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::android::{
        Selection, init_with_context, load_media_with_context, show_alert_with_context,
        show_confirm_with_context, show_photo_picker_with_context,
    };
}

mod error;
pub use error::*;

use std::path::{Path, PathBuf};

use waterkit_fs::WaterFs;

#[cfg(any(target_os = "android", target_os = "ios", test))]
pub(crate) const PATH_LIST_SEPARATOR: char = '\0';

#[cfg(any(target_os = "android", target_os = "ios", test))]
pub(crate) fn decode_string_list(encoded: Option<String>) -> Option<Vec<String>> {
    encoded.map(|encoded| {
        encoded
            .split(PATH_LIST_SEPARATOR)
            .map(std::string::ToString::to_string)
            .collect()
    })
}

/// Types of dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogType {
    /// Information dialog.
    Info,
    /// Warning dialog.
    Warning,
    /// Error dialog.
    Error,
}

/// A native dialog.
#[derive(Debug, Clone)]
pub struct Dialog {
    /// Title of the dialog.
    pub title: String,
    /// Message content of the dialog.
    pub message: String,
    /// Type/Icon of the dialog.
    pub type_: DialogType,
}

impl Dialog {
    /// Create a new dialog with default Info type.
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            type_: DialogType::Info,
        }
    }

    /// Set the dialog type.
    #[must_use]
    pub const fn with_type(mut self, type_: DialogType) -> Self {
        self.type_ = type_;
        self
    }

    /// Show the dialog (blocking or modal).
    /// Returns when the user dismisses the dialog.
    ///
    /// # Errors
    /// Returns an error if the native dialog fails to show or is not supported.
    pub async fn show(self) -> Result<(), DialogError> {
        sys::show_alert(self).await
    }

    /// Show a confirmation dialog (Yes/No or OK/Cancel).
    /// Returns true if confirmed (Yes/OK), false otherwise.
    ///
    /// # Errors
    /// Returns an error if the native dialog fails to show or is not supported.
    pub async fn show_confirm(self) -> Result<bool, DialogError> {
        sys::show_confirm(self).await
    }
}

/// A native file dialog (open/save).
#[derive(Debug, Clone)]
pub struct FileDialog {
    /// Title of the dialog
    pub title: Option<String>,
    /// Starting directory
    pub location: Option<std::path::PathBuf>,
    /// File filters name -> `extensions`
    pub filters: Vec<(String, Vec<String>)>,
    /// Import selected files into the application's cache directory subtree before returning.
    pub import_to_cache_subdir: Option<PathBuf>,
}

impl FileDialog {
    /// Create a new file dialog.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            title: None,
            location: None,
            filters: Vec::new(),
            import_to_cache_subdir: None,
        }
    }

    /// Set the title of the dialog.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the starting location.
    #[must_use]
    pub fn set_location(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.location = Some(path.into());
        self
    }

    /// Add a file extension filter.
    /// Usage: `add_filter("Image", &["png", "jpg"])`
    #[must_use]
    pub fn add_filter(mut self, name: impl Into<String>, extensions: &[&str]) -> Self {
        self.filters.push((
            name.into(),
            extensions
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
        ));
        self
    }

    /// Import selected files into the application's cache directory subtree before returning.
    #[must_use]
    pub fn import_to_cache_subdir(mut self, cache_subdir: impl Into<PathBuf>) -> Self {
        self.import_to_cache_subdir = Some(cache_subdir.into());
        self
    }

    /// Show the dialog to select a single file to open.
    ///
    /// # Errors
    /// Returns an error if the native dialog fails to show or is not supported.
    pub async fn show_open_single_file(self) -> Result<Option<std::path::PathBuf>, DialogError> {
        sys::show_open_single_file(self).await
    }

    /// Show the dialog to select multiple files to open.
    ///
    /// # Errors
    /// Returns an error if the native dialog fails to show or is not supported.
    pub async fn show_open_multiple_files(
        self,
    ) -> Result<Option<Vec<std::path::PathBuf>>, DialogError> {
        sys::show_open_multiple_files(self).await
    }

    // Future: show_save_single_file
}

impl Default for FileDialog {
    fn default() -> Self {
        Self::new()
    }
}

/// Type of media to pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    /// Images only.
    Image,
    /// Videos only.
    Video,
    /// Live Photos only (iOS). Other platforms report this as unsupported.
    LivePhoto,
}

/// The resolved kind of a loaded media selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadedMediaKind {
    /// A still image asset.
    Image,
    /// A video asset.
    Video,
    /// A live photo asset composed of paired still and motion resources.
    LivePhoto,
}

/// A loaded live photo asset composed of paired still and motion resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedLivePhoto {
    image: PathBuf,
    video: PathBuf,
}

impl LoadedLivePhoto {
    #[cfg(any(target_os = "ios", test))]
    fn new(image: PathBuf, video: PathBuf) -> Self {
        Self { image, video }
    }

    /// Returns the local still-image path for the live photo.
    #[must_use]
    pub fn image(&self) -> &Path {
        &self.image
    }

    /// Returns the local paired-video path for the live photo.
    #[must_use]
    pub fn video(&self) -> &Path {
        &self.video
    }

    /// Consumes the live photo and returns its paired file paths.
    #[must_use]
    pub fn into_parts(self) -> (PathBuf, PathBuf) {
        (self.image, self.video)
    }
}

/// A loaded media asset copied or resolved to local file paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadedMedia {
    /// A still image asset.
    Image(PathBuf),
    /// A video asset.
    Video(PathBuf),
    /// A live photo asset with paired still and motion resources.
    LivePhoto(LoadedLivePhoto),
}

impl LoadedMedia {
    /// Returns the local file path for the loaded media asset.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Image(path) | Self::Video(path) => path,
            Self::LivePhoto(live_photo) => live_photo.image(),
        }
    }

    /// Returns the resolved media kind.
    #[must_use]
    pub const fn kind(&self) -> LoadedMediaKind {
        match self {
            Self::Image(_) => LoadedMediaKind::Image,
            Self::Video(_) => LoadedMediaKind::Video,
            Self::LivePhoto(_) => LoadedMediaKind::LivePhoto,
        }
    }

    /// Returns the paired live photo payload when the asset is a live photo.
    #[must_use]
    pub fn as_live_photo(&self) -> Option<&LoadedLivePhoto> {
        match self {
            Self::LivePhoto(live_photo) => Some(live_photo),
            Self::Image(_) | Self::Video(_) => None,
        }
    }
}

/// A handle to a selected photo/media.
///
/// Use `load()` to download or copy the media to a local temporary file.
#[derive(Debug, Clone)]
pub struct PhotoHandle {
    handle: sys::Selection,
    requested_media_type: MediaType,
}

impl PhotoHandle {
    /// Load the media to a local file.
    ///
    /// This may involve downloading from the cloud (e.g., iCloud, Google Photos).
    /// Returns the path to the local file.
    ///
    /// # Errors
    /// Returns an error if loading fails.
    pub async fn load(self) -> Result<std::path::PathBuf, DialogError> {
        match self.load_media().await? {
            LoadedMedia::Image(path) | LoadedMedia::Video(path) => Ok(path),
            LoadedMedia::LivePhoto(_) => Err(DialogError::Unsupported(
                "live photo selections require PhotoHandle::load_media()".into(),
            )),
        }
    }

    /// Load the media to a local file and resolve its concrete media kind.
    ///
    /// # Errors
    /// Returns an error if loading fails.
    pub async fn load_media(self) -> Result<LoadedMedia, DialogError> {
        sys::load_photo_media(self.handle, self.requested_media_type).await
    }
}

/// A native photo picker.
#[derive(Debug, Clone)]
pub struct PhotoPicker {
    /// Type of media to pick.
    pub media_type: MediaType,
}

impl PhotoPicker {
    /// Create a new photo picker.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            media_type: MediaType::Image,
        }
    }

    /// Set the media type to pick.
    #[must_use]
    pub const fn with_media_type(mut self, media_type: MediaType) -> Self {
        self.media_type = media_type;
        self
    }

    /// Show the photo picker and return a handle to the selected media.
    ///
    /// # Errors
    /// Returns an error if the picker fails to show or is not supported.
    pub async fn pick(self) -> Result<Option<PhotoHandle>, DialogError> {
        (sys::show_photo_picker(self.media_type).await?).map_or(Ok(None), |handle| {
            Ok(Some(PhotoHandle {
                handle,
                requested_media_type: self.media_type,
            }))
        })
    }
}

impl Default for PhotoPicker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "android", target_os = "ios", test))]
pub(crate) fn collect_filter_extensions(dialog: &FileDialog) -> Vec<String> {
    let mut extensions = Vec::new();
    for (_, filter_extensions) in &dialog.filters {
        for ext in filter_extensions {
            let normalized = ext.trim().trim_start_matches('.').to_ascii_lowercase();
            if normalized.is_empty() {
                continue;
            }
            if !extensions
                .iter()
                .any(|existing: &String| existing == &normalized)
            {
                extensions.push(normalized);
            }
        }
    }
    extensions
}

pub(crate) fn finalize_selected_file(
    dialog: &FileDialog,
    path: PathBuf,
) -> Result<PathBuf, DialogError> {
    dialog
        .import_to_cache_subdir
        .as_deref()
        .map_or(Ok(path.clone()), |cache_subdir| {
            WaterFs::import_file_to_cache(&path, cache_subdir).map_err(DialogError::from)
        })
}

pub(crate) fn finalize_selected_files(
    dialog: &FileDialog,
    paths: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, DialogError> {
    assert!(
        !paths.is_empty(),
        "waterkit-dialog picker returned an empty file selection"
    );

    paths
        .into_iter()
        .map(|path| finalize_selected_file(dialog, path))
        .collect()
}

#[cfg(any(not(target_os = "ios"), test))]
fn infer_loaded_media_kind(path: &Path) -> LoadedMediaKind {
    let extension = path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase);

    if extension.as_deref().is_some_and(|extension| {
        matches!(
            extension,
            "mp4" | "mov" | "avi" | "mkv" | "webm" | "m4v" | "3gp" | "hevc"
        )
    }) {
        LoadedMediaKind::Video
    } else {
        LoadedMediaKind::Image
    }
}

#[cfg(any(not(target_os = "ios"), test))]
pub(crate) fn loaded_media_from_path(path: PathBuf) -> LoadedMedia {
    match infer_loaded_media_kind(&path) {
        LoadedMediaKind::Image => LoadedMedia::Image(path),
        LoadedMediaKind::Video => LoadedMedia::Video(path),
        LoadedMediaKind::LivePhoto => {
            panic!("single-path media classification must not resolve to LivePhoto")
        }
    }
}

#[cfg(any(target_os = "ios", test))]
pub(crate) fn decode_loaded_media_payload(
    encoded: Option<String>,
) -> Result<LoadedMedia, DialogError> {
    let parts = decode_string_list(encoded).ok_or(DialogError::Cancelled)?;
    assert!(
        !parts.is_empty(),
        "waterkit-dialog loaded media payload must contain at least a kind tag"
    );

    match parts.as_slice() {
        [kind, path] if kind == "image" => Ok(LoadedMedia::Image(PathBuf::from(path))),
        [kind, path] if kind == "video" => Ok(LoadedMedia::Video(PathBuf::from(path))),
        [kind, image, video] if kind == "livephoto" => Ok(LoadedMedia::LivePhoto(
            LoadedLivePhoto::new(PathBuf::from(image), PathBuf::from(video)),
        )),
        [kind, ..] => Err(DialogError::PlatformError(format!(
            "unknown loaded media payload kind: {kind}"
        ))),
        [] => panic!("loaded media payload unexpectedly empty"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FileDialog, LoadedLivePhoto, LoadedMedia, LoadedMediaKind, PATH_LIST_SEPARATOR,
        collect_filter_extensions, decode_loaded_media_payload, decode_string_list,
        finalize_selected_files, infer_loaded_media_kind, loaded_media_from_path,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn decode_string_list_roundtrips_nul_separated_payload() {
        let encoded = format!("a{PATH_LIST_SEPARATOR}b{PATH_LIST_SEPARATOR}c");
        assert_eq!(
            decode_string_list(Some(encoded)),
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
        assert_eq!(decode_string_list(None), None);
    }

    #[test]
    fn collect_filter_extensions_normalizes_and_deduplicates() {
        let dialog = FileDialog::new()
            .add_filter("Images", &[".PNG", "jpg", ""])
            .add_filter("More", &["png", " gif "]);
        assert_eq!(
            collect_filter_extensions(&dialog),
            vec!["png".to_string(), "jpg".to_string(), "gif".to_string()]
        );
    }

    #[test]
    fn infer_loaded_media_kind_uses_video_extensions() {
        assert_eq!(
            infer_loaded_media_kind(Path::new("/tmp/example.mov")),
            LoadedMediaKind::Video
        );
        assert_eq!(
            infer_loaded_media_kind(Path::new("/tmp/example.heic")),
            LoadedMediaKind::Image
        );
    }

    #[test]
    fn loaded_media_from_path_preserves_video_classification() {
        assert_eq!(
            loaded_media_from_path(PathBuf::from("/tmp/example.mov")),
            LoadedMedia::Video(PathBuf::from("/tmp/example.mov"))
        );
    }

    #[test]
    fn decode_loaded_media_payload_handles_live_photo_pairs() {
        let encoded =
            format!("livephoto{PATH_LIST_SEPARATOR}/tmp/a.heic{PATH_LIST_SEPARATOR}/tmp/a.mov");
        assert_eq!(
            decode_loaded_media_payload(Some(encoded)).unwrap(),
            LoadedMedia::LivePhoto(LoadedLivePhoto::new(
                PathBuf::from("/tmp/a.heic"),
                PathBuf::from("/tmp/a.mov"),
            ))
        );
    }

    #[test]
    fn finalize_selected_files_rejects_empty_selection() {
        let result =
            std::panic::catch_unwind(|| finalize_selected_files(&FileDialog::new(), Vec::new()));
        assert!(result.is_err(), "empty file selections must fail fast");
    }

    #[test]
    fn finalize_selected_files_preserves_paths_without_import_policy() {
        let paths = vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")];
        assert_eq!(
            finalize_selected_files(&FileDialog::new(), paths.clone()).unwrap(),
            paths
        );
    }
}
