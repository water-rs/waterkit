/// Errors produced while selecting or running a vision request.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VisionError {
    /// No available realization can serve the request.
    #[error("unsupported request: {0}")]
    Unsupported(String),
    /// Portable model weights are missing or failed verification (#133).
    #[error("model unavailable: {0}")]
    ModelUnavailable(String),
    /// A GPU operation failed.
    #[error("GPU error: {0}")]
    Gpu(String),
    /// A platform framework or service failed.
    #[error("platform error: {0}")]
    Platform(String),
}
