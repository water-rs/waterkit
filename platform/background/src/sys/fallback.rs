use crate::{
    AppRefreshRequest, BackgroundCapabilities, BackgroundError, BackgroundEvent, BootstrapConfig,
    ContinuedProcessingRequest, ProcessingRequest, TaskIdentifier,
};

/// The fallback task handle: this backend never launches tasks.
#[derive(Debug, Clone)]
pub struct TaskHandle;

/// Background runtime backend for unsupported platforms.
#[derive(Debug)]
pub struct BackgroundRuntimeInner;

#[allow(clippy::unused_self)]
impl BackgroundRuntimeInner {
    pub fn initialize(
        _events_tx: async_channel::Sender<BackgroundEvent>,
        _config: &BootstrapConfig,
    ) -> Result<Self, BackgroundError> {
        Err(BackgroundError::Unsupported)
    }

    pub fn submit_app_refresh(&self, _request: AppRefreshRequest) -> Result<(), BackgroundError> {
        Err(BackgroundError::Unsupported)
    }

    pub fn submit_processing(&self, _request: ProcessingRequest) -> Result<(), BackgroundError> {
        Err(BackgroundError::Unsupported)
    }

    pub fn submit_continued_processing(
        &self,
        _request: ContinuedProcessingRequest,
    ) -> Result<(), BackgroundError> {
        Err(BackgroundError::Unsupported)
    }

    pub const fn cancel(&self, _identifier: &TaskIdentifier) -> Result<(), BackgroundError> {
        Err(BackgroundError::Unsupported)
    }

    pub const fn cancel_all(&self) -> Result<(), BackgroundError> {
        Err(BackgroundError::Unsupported)
    }
}

#[must_use]
pub fn capabilities() -> BackgroundCapabilities {
    BackgroundCapabilities::default()
}

pub fn complete_task(
    _handle: &TaskHandle,
    _success: bool,
) -> impl core::future::Future<Output = Result<(), BackgroundError>> {
    core::future::ready(Err(BackgroundError::Unsupported))
}
