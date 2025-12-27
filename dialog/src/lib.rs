//! Cross-platform native dialogs/alerts for `WaterUI`.
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

#![warn(missing_docs)]

// Internal platform-specific implementations.
mod sys;

mod error;
pub use error::*;

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
}

impl FileDialog {
    /// Create a new file dialog.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            title: None,
            location: None,
            filters: Vec::new(),
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

    /// Show the dialog to select a single file to open.
    ///
    /// # Errors
    /// Returns an error if the native dialog fails to show or is not supported.
    pub async fn show_open_single_file(self) -> Result<Option<std::path::PathBuf>, DialogError> {
        sys::show_open_single_file(self).await
    }

    // Future: show_open_multiple_files, show_save_single_file
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
    /// Live Photos only (iOS). Falls back to Image on other platforms.
    LivePhoto,
}

/// A handle to a selected photo/media.
///
/// Use `load()` to download or copy the media to a local temporary file.
#[derive(Debug, Clone)]
pub struct PhotoHandle {
    handle: sys::Selection,
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
        sys::load_media(self.handle).await
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
        (sys::show_photo_picker(self).await?)
            .map_or(Ok(None), |handle| Ok(Some(PhotoHandle { handle })))
    }
}

impl Default for PhotoPicker {
    fn default() -> Self {
        Self::new()
    }
}
