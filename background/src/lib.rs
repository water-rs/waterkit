//! Cross-platform background task scheduling for mobile platforms.
//!
//! This crate provides:
//! - iOS background refresh (`BGAppRefreshTaskRequest`)
//! - iOS background processing (`BGProcessingTaskRequest`)
//! - iOS 26 continued processing (`BGContinuedProcessingTaskRequest`)
//! - Android background scheduling via `JobScheduler`
//!
//! Tasks that are launched by the platform are delivered through [`BackgroundRuntime::subscribe`].

#![warn(missing_docs)]

mod error;
mod sys;

use std::fmt;
use std::time::Duration;

#[cfg(target_os = "ios")]
use serde::Serialize;

pub use error::BackgroundError;

/// Background task kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TaskKind {
    /// A short app refresh task.
    AppRefresh = 1,
    /// A longer processing task.
    Processing = 2,
    /// A user-initiated continued processing task.
    ContinuedProcessing = 3,
}

impl TaskKind {
    #[cfg(target_os = "ios")]
    pub(crate) fn from_raw(value: u8) -> Result<Self, BackgroundError> {
        match value {
            1 => Ok(Self::AppRefresh),
            2 => Ok(Self::Processing),
            3 => Ok(Self::ContinuedProcessing),
            other => Err(BackgroundError::Platform(format!(
                "unknown task kind value: {other}"
            ))),
        }
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub(crate) const fn as_raw(self) -> u8 {
        self as u8
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AppRefresh => "app refresh",
            Self::Processing => "processing",
            Self::ContinuedProcessing => "continued processing",
        }
    }
}

/// Capability flags for the current platform backend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct BackgroundCapabilities {
    /// Whether app refresh scheduling is supported.
    pub supports_app_refresh: bool,
    /// Whether heavy processing scheduling is supported.
    pub supports_processing: bool,
    /// Whether iOS-26-style continued processing is supported.
    pub supports_continued_processing: bool,
    /// Whether continued processing GPU resources are supported.
    pub supports_continued_processing_gpu: bool,
    /// Whether the backend can emit launch/expiration events to Rust.
    pub supports_launch_events: bool,
}

/// A validated background task identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskIdentifier(String);

impl TaskIdentifier {
    /// Create a new validated task identifier.
    ///
    /// # Errors
    /// Returns an error if the identifier is malformed.
    pub fn new(identifier: impl Into<String>) -> Result<Self, BackgroundError> {
        let identifier = identifier.into();
        validate_identifier(&identifier)?;
        Ok(Self(identifier))
    }

    /// Borrow the identifier string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A continued-processing wildcard pattern (must end with `.*`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContinuedTaskPattern(String);

impl ContinuedTaskPattern {
    /// Create a new validated continued-processing pattern.
    ///
    /// Example: `com.example.app.continued.*`
    ///
    /// # Errors
    /// Returns an error if the pattern is malformed.
    pub fn new(pattern: impl Into<String>) -> Result<Self, BackgroundError> {
        let pattern = pattern.into();
        let Some(prefix) = pattern.strip_suffix(".*") else {
            return Err(BackgroundError::InvalidContinuedPattern {
                pattern,
                reason: "pattern must end with `.*`".into(),
            });
        };
        if prefix.is_empty() {
            return Err(BackgroundError::InvalidContinuedPattern {
                pattern,
                reason: "pattern prefix cannot be empty".into(),
            });
        }

        validate_identifier(prefix).map_err(|error| match error {
            BackgroundError::InvalidIdentifier { reason, .. } => {
                BackgroundError::InvalidContinuedPattern {
                    pattern: pattern.clone(),
                    reason,
                }
            }
            other => other,
        })?;

        Ok(Self(pattern))
    }

    /// Borrow the pattern string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn matches_identifier(&self, identifier: &TaskIdentifier) -> bool {
        let prefix = self
            .0
            .strip_suffix('*')
            .expect("continued pattern suffix already validated");
        identifier.as_str().starts_with(prefix) && identifier.as_str().len() > prefix.len()
    }
}

/// Android-specific bootstrap settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidConfig {
    job_service_class: String,
}

impl AndroidConfig {
    /// Create Android scheduler settings.
    ///
    /// `job_service_class` must be a fully-qualified class name of a `JobService`
    /// declared in `AndroidManifest.xml`.
    ///
    /// # Errors
    /// Returns an error if `job_service_class` is malformed.
    pub fn new(job_service_class: impl Into<String>) -> Result<Self, BackgroundError> {
        let job_service_class = job_service_class.into();
        if job_service_class.is_empty() {
            return Err(BackgroundError::ConfigurationMissing(
                "android job service class cannot be empty".into(),
            ));
        }
        if job_service_class.chars().any(char::is_whitespace) {
            return Err(BackgroundError::ConfigurationMissing(
                "android job service class cannot contain whitespace".into(),
            ));
        }
        if !job_service_class.contains('.') {
            return Err(BackgroundError::ConfigurationMissing(
                "android job service class must be fully-qualified".into(),
            ));
        }

        Ok(Self { job_service_class })
    }

    /// Borrow the configured job service class name.
    #[must_use]
    pub fn job_service_class(&self) -> &str {
        &self.job_service_class
    }
}

/// Bootstrap configuration used during [`initialize`].
#[derive(Debug, Clone, Default)]
pub struct BootstrapConfig {
    app_refresh_identifiers: Vec<TaskIdentifier>,
    processing_identifiers: Vec<TaskIdentifier>,
    continued_processing_patterns: Vec<ContinuedTaskPattern>,
    android: Option<AndroidConfig>,
}

impl BootstrapConfig {
    /// Create an empty bootstrap configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an app refresh task identifier.
    ///
    /// # Errors
    /// Returns an error on duplicate registration.
    pub fn register_app_refresh(
        mut self,
        identifier: TaskIdentifier,
    ) -> Result<Self, BackgroundError> {
        if self.app_refresh_identifiers.contains(&identifier) {
            return Err(BackgroundError::ConfigurationMissing(format!(
                "duplicate app refresh identifier `{identifier}`"
            )));
        }
        self.app_refresh_identifiers.push(identifier);
        Ok(self)
    }

    /// Register a processing task identifier.
    ///
    /// # Errors
    /// Returns an error on duplicate registration.
    pub fn register_processing(
        mut self,
        identifier: TaskIdentifier,
    ) -> Result<Self, BackgroundError> {
        if self.processing_identifiers.contains(&identifier) {
            return Err(BackgroundError::ConfigurationMissing(format!(
                "duplicate processing identifier `{identifier}`"
            )));
        }
        self.processing_identifiers.push(identifier);
        Ok(self)
    }

    /// Register a continued-processing wildcard pattern.
    ///
    /// # Errors
    /// Returns an error on duplicate registration.
    pub fn register_continued_processing(
        mut self,
        pattern: ContinuedTaskPattern,
    ) -> Result<Self, BackgroundError> {
        if self.continued_processing_patterns.contains(&pattern) {
            return Err(BackgroundError::ConfigurationMissing(format!(
                "duplicate continued-processing pattern `{}`",
                pattern.as_str()
            )));
        }
        self.continued_processing_patterns.push(pattern);
        Ok(self)
    }

    /// Attach Android-specific configuration.
    #[must_use]
    pub fn android_config(mut self, config: AndroidConfig) -> Self {
        self.android = Some(config);
        self
    }

    fn validate(&self) -> Result<(), BackgroundError> {
        for refresh_id in &self.app_refresh_identifiers {
            if self.processing_identifiers.contains(refresh_id) {
                return Err(BackgroundError::ConfigurationMissing(format!(
                    "identifier `{refresh_id}` cannot be registered for both app refresh and processing"
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn app_refresh_identifiers(&self) -> &[TaskIdentifier] {
        &self.app_refresh_identifiers
    }

    pub(crate) fn processing_identifiers(&self) -> &[TaskIdentifier] {
        &self.processing_identifiers
    }

    pub(crate) fn continued_processing_patterns(&self) -> &[ContinuedTaskPattern] {
        &self.continued_processing_patterns
    }

    #[cfg(target_os = "android")]
    pub(crate) const fn android_config_ref(&self) -> Option<&AndroidConfig> {
        self.android.as_ref()
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn registrations_json(&self) -> Result<String, BackgroundError> {
        #[derive(Serialize)]
        struct Registration<'a> {
            identifier: &'a str,
            kind: u8,
        }

        let mut registrations = Vec::new();
        registrations.extend(
            self.app_refresh_identifiers
                .iter()
                .map(|identifier| Registration {
                    identifier: identifier.as_str(),
                    kind: TaskKind::AppRefresh.as_raw(),
                }),
        );
        registrations.extend(
            self.processing_identifiers
                .iter()
                .map(|identifier| Registration {
                    identifier: identifier.as_str(),
                    kind: TaskKind::Processing.as_raw(),
                }),
        );
        registrations.extend(self.continued_processing_patterns.iter().map(|pattern| {
            Registration {
                identifier: pattern.as_str(),
                kind: TaskKind::ContinuedProcessing.as_raw(),
            }
        }));

        serde_json::to_string(&registrations)
            .map_err(|error| BackgroundError::Platform(format!("serialize registrations: {error}")))
    }
}

/// Request to schedule an app refresh task.
#[derive(Debug, Clone)]
pub struct AppRefreshRequest {
    identifier: TaskIdentifier,
    earliest_begin_after: Option<Duration>,
}

impl AppRefreshRequest {
    /// Build an app refresh request.
    #[must_use]
    pub const fn new(identifier: TaskIdentifier) -> Self {
        Self {
            identifier,
            earliest_begin_after: None,
        }
    }

    /// Delay scheduling until after this duration.
    #[must_use]
    pub const fn earliest_begin_after(mut self, delay: Duration) -> Self {
        self.earliest_begin_after = Some(delay);
        self
    }

    pub(crate) const fn identifier(&self) -> &TaskIdentifier {
        &self.identifier
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub(crate) const fn earliest_begin_after_value(&self) -> Option<Duration> {
        self.earliest_begin_after
    }
}

/// Request to schedule a processing task.
#[derive(Debug, Clone)]
pub struct ProcessingRequest {
    identifier: TaskIdentifier,
    earliest_begin_after: Option<Duration>,
    requires_network_connectivity: bool,
    requires_external_power: bool,
}

impl ProcessingRequest {
    /// Build a processing request.
    #[must_use]
    pub const fn new(identifier: TaskIdentifier) -> Self {
        Self {
            identifier,
            earliest_begin_after: None,
            requires_network_connectivity: false,
            requires_external_power: false,
        }
    }

    /// Delay scheduling until after this duration.
    #[must_use]
    pub const fn earliest_begin_after(mut self, delay: Duration) -> Self {
        self.earliest_begin_after = Some(delay);
        self
    }

    /// Require network connectivity.
    #[must_use]
    pub const fn requires_network_connectivity(mut self, required: bool) -> Self {
        self.requires_network_connectivity = required;
        self
    }

    /// Require external power.
    #[must_use]
    pub const fn requires_external_power(mut self, required: bool) -> Self {
        self.requires_external_power = required;
        self
    }

    pub(crate) const fn identifier(&self) -> &TaskIdentifier {
        &self.identifier
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub(crate) const fn earliest_begin_after_value(&self) -> Option<Duration> {
        self.earliest_begin_after
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub(crate) const fn requires_network_connectivity_value(&self) -> bool {
        self.requires_network_connectivity
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub(crate) const fn requires_external_power_value(&self) -> bool {
        self.requires_external_power
    }
}

/// iOS continued-processing submission strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuedProcessingStrategy {
    /// Fail immediately when immediate execution is not possible.
    Fail,
    /// Queue and run when resources become available.
    Queue,
}

impl ContinuedProcessingStrategy {
    #[cfg(target_os = "ios")]
    pub(crate) const fn as_raw(self) -> u8 {
        match self {
            Self::Fail => 0,
            Self::Queue => 1,
        }
    }
}

/// Request to schedule a continued processing task (iOS 26+).
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct ContinuedProcessingRequest {
    identifier: TaskIdentifier,
    title: String,
    subtitle: String,
    strategy: ContinuedProcessingStrategy,
    requires_gpu: bool,
}

impl ContinuedProcessingRequest {
    /// Build a continued processing request.
    ///
    /// # Errors
    /// Returns an error if `title` or `subtitle` is empty.
    pub fn new(
        identifier: TaskIdentifier,
        title: impl Into<String>,
        subtitle: impl Into<String>,
    ) -> Result<Self, BackgroundError> {
        let title = title.into();
        let subtitle = subtitle.into();
        if title.is_empty() {
            return Err(BackgroundError::ConfigurationMissing(
                "continued processing title cannot be empty".into(),
            ));
        }
        if subtitle.is_empty() {
            return Err(BackgroundError::ConfigurationMissing(
                "continued processing subtitle cannot be empty".into(),
            ));
        }

        Ok(Self {
            identifier,
            title,
            subtitle,
            strategy: ContinuedProcessingStrategy::Queue,
            requires_gpu: false,
        })
    }

    /// Set submission strategy.
    #[must_use]
    pub const fn strategy(mut self, strategy: ContinuedProcessingStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Require background GPU resources.
    #[must_use]
    pub const fn requires_gpu(mut self, requires_gpu: bool) -> Self {
        self.requires_gpu = requires_gpu;
        self
    }

    pub(crate) const fn identifier(&self) -> &TaskIdentifier {
        &self.identifier
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn subtitle(&self) -> &str {
        &self.subtitle
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub(crate) const fn strategy_value(&self) -> ContinuedProcessingStrategy {
        self.strategy
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub(crate) const fn requires_gpu_value(&self) -> bool {
        self.requires_gpu
    }
}

/// Event emitted by a background runtime.
#[derive(Debug, Clone)]
pub enum BackgroundEvent {
    /// A task launch callback from the platform scheduler.
    Launched(BackgroundTask),
    /// A task expiration callback from the platform scheduler.
    Expired(BackgroundTaskExpiration),
}

/// A launched background task that must be completed.
#[derive(Debug, Clone)]
pub struct BackgroundTask {
    identifier: TaskIdentifier,
    kind: TaskKind,
    runtime_handle: u64,
    token: u64,
}

impl BackgroundTask {
    /// Borrow the launched task identifier.
    #[must_use]
    pub const fn identifier(&self) -> &TaskIdentifier {
        &self.identifier
    }

    /// Get the launched task kind.
    #[must_use]
    pub const fn kind(&self) -> TaskKind {
        self.kind
    }

    /// Mark the task as completed.
    ///
    /// This consumes the task to prevent duplicate completion calls.
    ///
    /// # Errors
    /// Returns an error if the completion callback fails.
    pub fn complete(self, success: bool) -> Result<(), BackgroundError> {
        sys::complete_task(self.runtime_handle, self.token, success)
    }

    /// Update the user-visible title and subtitle for a continued processing task.
    ///
    /// # Errors
    /// Returns an error if this is not a continued processing task or if the backend rejects it.
    #[cfg(target_os = "ios")]
    pub fn update_status(
        &self,
        title: impl Into<String>,
        subtitle: impl Into<String>,
    ) -> Result<(), BackgroundError> {
        if self.kind != TaskKind::ContinuedProcessing {
            return Err(BackgroundError::InvalidTaskKind(self.kind.as_str()));
        }
        let title = title.into();
        let subtitle = subtitle.into();
        if title.is_empty() || subtitle.is_empty() {
            return Err(BackgroundError::ConfigurationMissing(
                "continued task title/subtitle cannot be empty".into(),
            ));
        }
        sys::update_continued_processing_status(self.runtime_handle, self.token, &title, &subtitle)
    }

    /// Update progress for a continued processing task.
    ///
    /// # Errors
    /// Returns an error if this is not a continued processing task or if progress values are invalid.
    #[cfg(target_os = "ios")]
    pub fn update_progress(&self, completed: u64, total: u64) -> Result<(), BackgroundError> {
        if self.kind != TaskKind::ContinuedProcessing {
            return Err(BackgroundError::InvalidTaskKind(self.kind.as_str()));
        }
        if total == 0 {
            return Err(BackgroundError::ConfigurationMissing(
                "continued task progress total must be > 0".into(),
            ));
        }
        if completed > total {
            return Err(BackgroundError::ConfigurationMissing(
                "continued task progress completed cannot exceed total".into(),
            ));
        }
        sys::update_continued_processing_progress(self.runtime_handle, self.token, completed, total)
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn from_raw(
        runtime_handle: u64,
        token: u64,
        identifier: &str,
        kind_raw: u8,
    ) -> Result<Self, BackgroundError> {
        let kind = TaskKind::from_raw(kind_raw)?;
        let identifier = TaskIdentifier::new(identifier.to_owned())?;
        Ok(Self {
            identifier,
            kind,
            runtime_handle,
            token,
        })
    }
}

/// Expiration payload for a task whose allotted background time ended.
#[derive(Debug, Clone)]
pub struct BackgroundTaskExpiration {
    identifier: TaskIdentifier,
    kind: TaskKind,
}

impl BackgroundTaskExpiration {
    /// Borrow the expired task identifier.
    #[must_use]
    pub const fn identifier(&self) -> &TaskIdentifier {
        &self.identifier
    }

    /// Get the expired task kind.
    #[must_use]
    pub const fn kind(&self) -> TaskKind {
        self.kind
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn from_raw(identifier: &str, kind_raw: u8) -> Result<Self, BackgroundError> {
        let kind = TaskKind::from_raw(kind_raw)?;
        let identifier = TaskIdentifier::new(identifier.to_owned())?;
        Ok(Self { identifier, kind })
    }
}

/// A runtime instance for receiving task callbacks and scheduling work.
#[derive(Debug)]
pub struct BackgroundRuntime {
    inner: sys::BackgroundRuntimeInner,
    registrations: BootstrapConfig,
    events: async_channel::Receiver<BackgroundEvent>,
    _events_tx_guard: Box<async_channel::Sender<BackgroundEvent>>,
}

impl BackgroundRuntime {
    /// Subscribe to background task events.
    #[must_use]
    pub fn subscribe(&self) -> async_channel::Receiver<BackgroundEvent> {
        self.events.clone()
    }

    /// Submit an app refresh task request.
    ///
    /// # Errors
    /// Returns an error if the identifier was not registered at bootstrap or if scheduling fails.
    pub fn submit_app_refresh(&self, request: AppRefreshRequest) -> Result<(), BackgroundError> {
        if !self
            .registrations
            .app_refresh_identifiers()
            .contains(request.identifier())
        {
            return Err(BackgroundError::HandlerNotRegistered {
                identifier: request.identifier().to_string(),
                kind: TaskKind::AppRefresh.as_str(),
            });
        }

        self.inner.submit_app_refresh(request)
    }

    /// Submit a processing task request.
    ///
    /// # Errors
    /// Returns an error if the identifier was not registered at bootstrap or if scheduling fails.
    pub fn submit_processing(&self, request: ProcessingRequest) -> Result<(), BackgroundError> {
        if !self
            .registrations
            .processing_identifiers()
            .contains(request.identifier())
        {
            return Err(BackgroundError::HandlerNotRegistered {
                identifier: request.identifier().to_string(),
                kind: TaskKind::Processing.as_str(),
            });
        }

        self.inner.submit_processing(request)
    }

    /// Submit an iOS 26 continued processing request.
    ///
    /// # Errors
    /// Returns an error if no registered wildcard pattern matches this identifier,
    /// or if scheduling fails.
    pub fn submit_continued_processing(
        &self,
        request: ContinuedProcessingRequest,
    ) -> Result<(), BackgroundError> {
        if !self
            .registrations
            .continued_processing_patterns()
            .iter()
            .any(|pattern| pattern.matches_identifier(request.identifier()))
        {
            return Err(BackgroundError::HandlerNotRegistered {
                identifier: request.identifier().to_string(),
                kind: TaskKind::ContinuedProcessing.as_str(),
            });
        }

        self.inner.submit_continued_processing(request)
    }

    /// Cancel a previously submitted request by identifier.
    ///
    /// # Errors
    /// Returns an error if cancellation fails.
    #[allow(clippy::missing_const_for_fn)]
    pub fn cancel(&self, identifier: &TaskIdentifier) -> Result<(), BackgroundError> {
        self.inner.cancel(identifier)
    }

    /// Cancel all pending requests.
    ///
    /// # Errors
    /// Returns an error if cancellation fails.
    #[allow(clippy::missing_const_for_fn)]
    pub fn cancel_all(&self) -> Result<(), BackgroundError> {
        self.inner.cancel_all()
    }
}

/// Initialize background scheduling and callback runtime.
///
/// # Errors
/// Returns an error if bootstrap configuration is invalid or backend initialization fails.
pub fn initialize(config: BootstrapConfig) -> Result<BackgroundRuntime, BackgroundError> {
    config.validate()?;

    let (events_tx, events_rx) = async_channel::bounded(128);
    let events_tx = Box::new(events_tx);
    #[allow(clippy::cast_possible_truncation)]
    let event_ctx = (&raw const *events_tx) as usize as u64;

    let inner = sys::BackgroundRuntimeInner::initialize(event_ctx, &config)?;

    Ok(BackgroundRuntime {
        inner,
        registrations: config,
        events: events_rx,
        _events_tx_guard: events_tx,
    })
}

/// Query background capabilities on the current platform.
#[must_use]
pub fn capabilities() -> BackgroundCapabilities {
    sys::capabilities()
}

fn validate_identifier(identifier: &str) -> Result<(), BackgroundError> {
    if identifier.is_empty() {
        return Err(BackgroundError::InvalidIdentifier {
            identifier: identifier.to_owned(),
            reason: "identifier cannot be empty".into(),
        });
    }

    if identifier.len() > 128 {
        return Err(BackgroundError::InvalidIdentifier {
            identifier: identifier.to_owned(),
            reason: "identifier length must be <= 128".into(),
        });
    }

    if identifier.starts_with('.') || identifier.ends_with('.') {
        return Err(BackgroundError::InvalidIdentifier {
            identifier: identifier.to_owned(),
            reason: "identifier cannot start or end with '.'".into(),
        });
    }

    if identifier.contains("..") {
        return Err(BackgroundError::InvalidIdentifier {
            identifier: identifier.to_owned(),
            reason: "identifier cannot contain consecutive '.' characters".into(),
        });
    }

    if let Some(invalid) = identifier
        .chars()
        .find(|ch| !ch.is_ascii_alphanumeric() && *ch != '.' && *ch != '_' && *ch != '-')
    {
        return Err(BackgroundError::InvalidIdentifier {
            identifier: identifier.to_owned(),
            reason: format!("identifier contains invalid character '{invalid}'"),
        });
    }

    Ok(())
}

#[cfg(target_os = "ios")]
pub(crate) fn dispatch_launched_event(
    event_ctx: u64,
    runtime_handle: u64,
    task_token: u64,
    identifier: &str,
    kind_raw: u8,
) {
    let task = match BackgroundTask::from_raw(runtime_handle, task_token, identifier, kind_raw) {
        Ok(task) => task,
        Err(error) => {
            eprintln!("invalid launched task callback payload: {error}");
            return;
        }
    };

    #[allow(clippy::cast_possible_truncation)]
    let sender = unsafe { &*(event_ctx as usize as *const async_channel::Sender<BackgroundEvent>) };
    let _ = sender.try_send(BackgroundEvent::Launched(task));
}

#[cfg(target_os = "ios")]
pub(crate) fn dispatch_expired_event(event_ctx: u64, identifier: &str, kind_raw: u8) {
    let expiration = match BackgroundTaskExpiration::from_raw(identifier, kind_raw) {
        Ok(expiration) => expiration,
        Err(error) => {
            eprintln!("invalid expired task callback payload: {error}");
            return;
        }
    };

    #[allow(clippy::cast_possible_truncation)]
    let sender = unsafe { &*(event_ctx as usize as *const async_channel::Sender<BackgroundEvent>) };
    let _ = sender.try_send(BackgroundEvent::Expired(expiration));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_identifier() {
        assert!(TaskIdentifier::new("com.example.refresh").is_ok());
        assert!(TaskIdentifier::new("com..example").is_err());
        assert!(TaskIdentifier::new("com.example.*").is_err());
    }

    #[test]
    fn validates_continued_pattern() {
        assert!(ContinuedTaskPattern::new("com.example.continued.*").is_ok());
        assert!(ContinuedTaskPattern::new("com.example.continued").is_err());
    }

    #[test]
    fn continued_pattern_matches_identifier_suffix() {
        let pattern =
            ContinuedTaskPattern::new("com.example.continued.*").expect("pattern should be valid");
        let matching = TaskIdentifier::new("com.example.continued.42").expect("id should be valid");
        let not_matching =
            TaskIdentifier::new("com.example.continued").expect("id should be valid");

        assert!(pattern.matches_identifier(&matching));
        assert!(!pattern.matches_identifier(&not_matching));
    }
}
