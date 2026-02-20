use crate::{
    AppRefreshRequest, BackgroundCapabilities, BackgroundError, BootstrapConfig,
    ContinuedProcessingRequest, ProcessingRequest, TaskIdentifier,
};

/// Background runtime backend for unsupported platforms.
#[derive(Debug)]
pub struct BackgroundRuntimeInner;

#[allow(clippy::unused_self)]
impl BackgroundRuntimeInner {
    pub const fn initialize(
        _event_ctx: u64,
        _config: &BootstrapConfig,
    ) -> Result<Self, BackgroundError> {
        Err(BackgroundError::NotSupported)
    }

    pub fn submit_app_refresh(&self, _request: AppRefreshRequest) -> Result<(), BackgroundError> {
        Err(BackgroundError::NotSupported)
    }

    pub fn submit_processing(&self, _request: ProcessingRequest) -> Result<(), BackgroundError> {
        Err(BackgroundError::NotSupported)
    }

    pub fn submit_continued_processing(
        &self,
        _request: ContinuedProcessingRequest,
    ) -> Result<(), BackgroundError> {
        Err(BackgroundError::NotSupported)
    }

    pub const fn cancel(&self, _identifier: &TaskIdentifier) -> Result<(), BackgroundError> {
        Err(BackgroundError::NotSupported)
    }

    pub const fn cancel_all(&self) -> Result<(), BackgroundError> {
        Err(BackgroundError::NotSupported)
    }
}

#[must_use]
pub fn capabilities() -> BackgroundCapabilities {
    BackgroundCapabilities::default()
}

pub const fn complete_task(
    _runtime_handle: u64,
    _task_token: u64,
    _success: bool,
) -> Result<(), BackgroundError> {
    Err(BackgroundError::NotSupported)
}
