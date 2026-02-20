//! Cross-platform share sheet and social sharing.
//!
//! Provides a unified API for sharing content via the system share sheet.

#![warn(missing_docs)]

mod sys;

use std::path::PathBuf;

/// Android-specific JNI helpers that require `JNIEnv` and `Context`.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::jni_api::share_with_context;
}

/// Content that can be shared.
#[derive(Debug, Clone)]
pub enum ShareItem {
    /// Plain text content.
    Text(String),
    /// A URL.
    Url(String),
    /// An image file path.
    Image(PathBuf),
    /// A generic file path.
    File(PathBuf),
}

/// A share request with one or more items.
#[derive(Debug, Clone)]
pub struct ShareSheet {
    /// Items to share.
    pub items: Vec<ShareItem>,
    /// Optional subject line (for email sharing).
    pub subject: Option<String>,
}

impl ShareSheet {
    /// Create a new share sheet with a single text item.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            items: vec![ShareItem::Text(text.into())],
            subject: None,
        }
    }

    /// Create a new share sheet with a URL.
    #[must_use]
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            items: vec![ShareItem::Url(url.into())],
            subject: None,
        }
    }

    /// Create a new share sheet with an image.
    #[must_use]
    pub fn image(path: impl Into<PathBuf>) -> Self {
        Self {
            items: vec![ShareItem::Image(path.into())],
            subject: None,
        }
    }

    /// Create a new share sheet with a file.
    #[must_use]
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            items: vec![ShareItem::File(path.into())],
            subject: None,
        }
    }

    /// Add another item to share.
    #[must_use]
    pub fn with_item(mut self, item: ShareItem) -> Self {
        self.items.push(item);
        self
    }

    /// Set the subject line.
    #[must_use]
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Show the share sheet and wait for the result.
    ///
    /// # Errors
    /// Returns error if sharing fails or is not supported.
    pub async fn show(self) -> Result<ShareResult, ShareError> {
        if self.items.is_empty() {
            return Err(ShareError::EmptyContent);
        }
        sys::show_share_sheet(self).await
    }
}

/// Result of a share operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareResult {
    /// Content was shared successfully.
    Shared,
    /// User cancelled the share sheet.
    Cancelled,
}

/// Errors that can occur during sharing.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ShareError {
    /// Sharing is not supported on this platform.
    #[error("sharing not supported")]
    NotSupported,
    /// No content to share.
    #[error("no content to share")]
    EmptyContent,
    /// File not found.
    #[error("file not found: {0}")]
    FileNotFound(String),
    /// Platform-specific error.
    #[error("platform error: {0}")]
    PlatformError(String),
}
