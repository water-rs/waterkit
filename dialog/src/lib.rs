//! Cross-platform native dialogs/alerts.

#![warn(missing_docs)]

/// Platform-specific implementations.
pub mod sys;

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
    pub fn with_type(mut self, type_: DialogType) -> Self {
        self.type_ = type_;
        self
    }

    /// Show the dialog (blocking or modal).
    /// Returns when the user dismisses the dialog.
    pub async fn show(self) -> Result<(), String> {
        sys::show_alert(self).await
    }

    /// Show a confirmation dialog (Yes/No or OK/Cancel).
    /// Returns true if confirmed (Yes/OK), false otherwise.
    pub async fn show_confirm(self) -> Result<bool, String> {
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
    /// File filters name -> [extensions]
    pub filters: Vec<(String, Vec<String>)>,
}

impl FileDialog {
    /// Create a new file dialog.
    pub fn new() -> Self {
        Self {
            title: None,
            location: None,
            filters: Vec::new(),
        }
    }

    /// Set the title of the dialog.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the starting location.
    pub fn set_location(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.location = Some(path.into());
        self
    }

    /// Add a file extension filter.
    /// Usage: .add_filter("Image", &["png", "jpg"])
    pub fn add_filter(mut self, name: impl Into<String>, extensions: &[&str]) -> Self {
        self.filters.push((
            name.into(),
            extensions.iter().map(|s| s.to_string()).collect(),
        ));
        self
    }

    /// Show the dialog to select a single file to open.
    pub async fn show_open_single_file(self) -> Result<Option<std::path::PathBuf>, String> {
        sys::show_open_single_file(self).await
    }
    
    // Future: show_open_multiple_files, show_save_single_file
}

impl Default for FileDialog {
    fn default() -> Self {
        Self::new()
    }
}
