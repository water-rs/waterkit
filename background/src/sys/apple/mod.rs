//! iOS background task backend using `BackgroundTasks`.

use crate::{
    AppRefreshRequest, BackgroundCapabilities, BackgroundError, BootstrapConfig,
    ContinuedProcessingRequest, ProcessingRequest, TaskIdentifier,
};

const CAP_APP_REFRESH: u8 = 1 << 0;
const CAP_PROCESSING: u8 = 1 << 1;
const CAP_CONTINUED_PROCESSING: u8 = 1 << 2;
const CAP_LAUNCH_EVENTS: u8 = 1 << 3;
const CAP_CONTINUED_GPU: u8 = 1 << 4;

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn ios_background_initialize(event_ctx: u64, registrations_json: &str) -> String;
        fn ios_background_shutdown(runtime_handle: u64);
        fn ios_background_capabilities() -> u8;

        fn ios_background_submit_app_refresh(
            runtime_handle: u64,
            identifier: &str,
            earliest_begin_seconds: u64,
        ) -> Option<String>;

        fn ios_background_submit_processing(
            runtime_handle: u64,
            identifier: &str,
            earliest_begin_seconds: u64,
            requires_network_connectivity: bool,
            requires_external_power: bool,
        ) -> Option<String>;

        fn ios_background_submit_continued_processing(
            runtime_handle: u64,
            identifier: &str,
            title: &str,
            subtitle: &str,
            strategy: u8,
            requires_gpu: bool,
        ) -> Option<String>;

        fn ios_background_cancel(runtime_handle: u64, identifier: &str) -> Option<String>;
        fn ios_background_cancel_all(runtime_handle: u64) -> Option<String>;

        fn ios_background_complete_task(
            runtime_handle: u64,
            task_token: u64,
            success: bool,
        ) -> Option<String>;

        fn ios_background_update_continued_status(
            runtime_handle: u64,
            task_token: u64,
            title: &str,
            subtitle: &str,
        ) -> Option<String>;

        fn ios_background_update_continued_progress(
            runtime_handle: u64,
            task_token: u64,
            completed: u64,
            total: u64,
        ) -> Option<String>;
    }

    extern "Rust" {
        fn on_background_task_launched_raw(
            event_ctx: u64,
            runtime_handle: u64,
            task_token: u64,
            identifier: &str,
            kind_raw: u8,
        );
        fn on_background_task_expired_raw(
            event_ctx: u64,
            runtime_handle: u64,
            task_token: u64,
            identifier: &str,
            kind_raw: u8,
        );
    }
}

fn on_background_task_launched_raw(
    event_ctx: u64,
    runtime_handle: u64,
    task_token: u64,
    identifier: &str,
    kind_raw: u8,
) {
    crate::dispatch_launched_event(event_ctx, runtime_handle, task_token, identifier, kind_raw);
}

#[allow(clippy::unused_self)]
fn on_background_task_expired_raw(
    event_ctx: u64,
    _runtime_handle: u64,
    _task_token: u64,
    identifier: &str,
    kind_raw: u8,
) {
    crate::dispatch_expired_event(event_ctx, identifier, kind_raw);
}

/// iOS runtime state.
#[derive(Debug)]
pub struct BackgroundRuntimeInner {
    runtime_handle: u64,
}

impl BackgroundRuntimeInner {
    pub fn initialize(event_ctx: u64, config: &BootstrapConfig) -> Result<Self, BackgroundError> {
        let registrations_json = config.registrations_json()?;
        let response = ffi::ios_background_initialize(event_ctx, &registrations_json);
        parse_initialize_response(&response).map(|runtime_handle| Self { runtime_handle })
    }

    pub fn submit_app_refresh(&self, request: AppRefreshRequest) -> Result<(), BackgroundError> {
        map_swift_result(ffi::ios_background_submit_app_refresh(
            self.runtime_handle,
            request.identifier().as_str(),
            duration_seconds(request.earliest_begin_after_value()),
        ))
    }

    pub fn submit_processing(&self, request: ProcessingRequest) -> Result<(), BackgroundError> {
        map_swift_result(ffi::ios_background_submit_processing(
            self.runtime_handle,
            request.identifier().as_str(),
            duration_seconds(request.earliest_begin_after_value()),
            request.requires_network_connectivity_value(),
            request.requires_external_power_value(),
        ))
    }

    pub fn submit_continued_processing(
        &self,
        request: ContinuedProcessingRequest,
    ) -> Result<(), BackgroundError> {
        map_swift_result(ffi::ios_background_submit_continued_processing(
            self.runtime_handle,
            request.identifier().as_str(),
            request.title(),
            request.subtitle(),
            request.strategy_value().as_raw(),
            request.requires_gpu_value(),
        ))
    }

    pub fn cancel(&self, identifier: &TaskIdentifier) -> Result<(), BackgroundError> {
        map_swift_result(ffi::ios_background_cancel(
            self.runtime_handle,
            identifier.as_str(),
        ))
    }

    pub fn cancel_all(&self) -> Result<(), BackgroundError> {
        map_swift_result(ffi::ios_background_cancel_all(self.runtime_handle))
    }
}

impl Drop for BackgroundRuntimeInner {
    fn drop(&mut self) {
        ffi::ios_background_shutdown(self.runtime_handle);
    }
}

#[must_use]
pub fn capabilities() -> BackgroundCapabilities {
    let bits = ffi::ios_background_capabilities();
    BackgroundCapabilities {
        supports_app_refresh: bits & CAP_APP_REFRESH != 0,
        supports_processing: bits & CAP_PROCESSING != 0,
        supports_continued_processing: bits & CAP_CONTINUED_PROCESSING != 0,
        supports_continued_processing_gpu: bits & CAP_CONTINUED_GPU != 0,
        supports_launch_events: bits & CAP_LAUNCH_EVENTS != 0,
    }
}

pub fn complete_task(
    runtime_handle: u64,
    task_token: u64,
    success: bool,
) -> Result<(), BackgroundError> {
    map_swift_result(ffi::ios_background_complete_task(
        runtime_handle,
        task_token,
        success,
    ))
}

pub fn update_continued_processing_status(
    runtime_handle: u64,
    task_token: u64,
    title: &str,
    subtitle: &str,
) -> Result<(), BackgroundError> {
    map_swift_result(ffi::ios_background_update_continued_status(
        runtime_handle,
        task_token,
        title,
        subtitle,
    ))
}

pub fn update_continued_processing_progress(
    runtime_handle: u64,
    task_token: u64,
    completed: u64,
    total: u64,
) -> Result<(), BackgroundError> {
    map_swift_result(ffi::ios_background_update_continued_progress(
        runtime_handle,
        task_token,
        completed,
        total,
    ))
}

fn parse_initialize_response(response: &str) -> Result<u64, BackgroundError> {
    if let Some(handle) = response.strip_prefix("ok:") {
        return handle.parse::<u64>().map_err(|error| {
            BackgroundError::Platform(format!("invalid runtime handle: {error}"))
        });
    }

    if let Some(error_message) = response.strip_prefix("err:") {
        return Err(map_swift_error(error_message.to_owned()));
    }

    Err(BackgroundError::Platform(format!(
        "invalid initialize response: {response}"
    )))
}

fn map_swift_result(result: Option<String>) -> Result<(), BackgroundError> {
    result.map_or(Ok(()), |error| Err(map_swift_error(error)))
}

fn map_swift_error(error: String) -> BackgroundError {
    if let Some(message) = error.strip_prefix("late_init:") {
        return BackgroundError::LateInitialization(message.to_owned());
    }

    if let Some(message) = error.strip_prefix("config_missing:") {
        return BackgroundError::ConfigurationMissing(message.to_owned());
    }

    if let Some(payload) = error.strip_prefix("scheduler_rejected:") {
        let (code_str, message) = payload.split_once(':').unwrap_or(("-1", payload));
        let code = code_str.parse::<i32>().unwrap_or(-1);
        return BackgroundError::SchedulerRejected {
            code,
            message: message.to_owned(),
        };
    }

    if let Some(message) = error.strip_prefix("invalid_token:") {
        return BackgroundError::InvalidTaskToken(message.to_owned());
    }

    if error == "not_supported" {
        return BackgroundError::NotSupported;
    }

    BackgroundError::Platform(error)
}

fn duration_seconds(value: Option<std::time::Duration>) -> u64 {
    value.map_or(0, |duration| duration.as_secs())
}
