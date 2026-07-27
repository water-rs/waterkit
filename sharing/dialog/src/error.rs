use thiserror::Error;

/// Errors that can occur when using dialogs.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum DialogError {
    /// The user cancelled the dialog or operation.
    #[error("operation cancelled")]
    Cancelled,

    /// An error occurred in the underlying platform implementation.
    #[error("platform error: {0}")]
    PlatformError(String),

    /// An IO error occurred (e.g. during file copy).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A waterkit-fs error (cache directory missing, file name invalid, …).
    #[error(transparent)]
    Fs(#[from] waterkit_fs::FsError),

    /// The requested feature is not supported on this platform.
    #[error("not supported: {0}")]
    Unsupported(String),
}

#[cfg(target_os = "android")]
impl From<jni::errors::Error> for DialogError {
    fn from(error: jni::errors::Error) -> Self {
        Self::PlatformError(error.to_string())
    }
}
