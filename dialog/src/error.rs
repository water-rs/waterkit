use thiserror::Error;

/// Errors that can occur when using dialogs.
#[derive(Error, Debug)]
pub enum DialogError {
    /// The user cancelled the dialog or operation.
    #[error("Operation cancelled")]
    Cancelled,

    /// An error occurred in the underlying platform implementation.
    #[error("Platform error: {0}")]
    PlatformError(String),

    /// An IO error occurred (e.g. during file copy).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// The requested feature is not supported on this platform.
    #[error("Not supported: {0}")]
    NotSupported(String),
}
