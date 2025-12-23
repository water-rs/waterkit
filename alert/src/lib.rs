//! Cross-platform native alerts/popups.

#![warn(missing_docs)]

/// Platform-specific implementations.
pub mod sys;

/// Types of alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertType {
    /// Information alert.
    Info,
    /// Warning alert.
    Warning,
    /// Error alert.
    Error,
}

/// A native alert dialog.
#[derive(Debug, Clone)]
pub struct Alert {
    /// Title of the alert.
    pub title: String,
    /// Message content of the alert.
    pub message: String,
    /// Type/Icon of the alert.
    pub type_: AlertType,
}

impl Alert {
    /// Create a new alert with default Info type.
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            type_: AlertType::Info,
        }
    }

    /// Set the alert type.
    pub fn with_type(mut self, type_: AlertType) -> Self {
        self.type_ = type_;
        self
    }

    /// Show the alert (blocking or modal).
    /// Returns when the user dismisses the alert.
    pub async fn show(self) -> Result<(), String> {
        sys::show_alert(self).await
    }

    /// Show a confirmation dialog (Yes/No or OK/Cancel).
    /// Returns true if confirmed (Yes/OK), false otherwise.
    pub async fn show_confirm(self) -> Result<bool, String> {
        sys::show_confirm(self).await
    }
}
