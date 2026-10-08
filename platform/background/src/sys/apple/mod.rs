//! iOS background task backend via `BackgroundTasks` through `objc2`.
//!
//! The runtime is a plain Rust [`RuntimeState`] owned by an `Arc` inside
//! [`BackgroundRuntimeInner`]; the scheduler's launch/expiration blocks
//! capture a `Weak` to it, matching the original `[weak self]` lifecycle.
//! `runtime_handle` values are that `Arc`'s address — opaque to callers and
//! only meaningful while the runtime is alive, the same contract the
//! `Unmanaged` handle carried.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use core::ptr::NonNull;
use objc2::rc::Retained;
use objc2::{AnyThread, Message};
use objc2_background_tasks::{
    BGAppRefreshTaskRequest, BGContinuedProcessingTask, BGContinuedProcessingTaskRequest,
    BGContinuedProcessingTaskRequestResources, BGContinuedProcessingTaskRequestSubmissionStrategy,
    BGProcessingTaskRequest, BGTask, BGTaskRequest, BGTaskScheduler, BGTaskSchedulerErrorDomain,
};
use objc2_foundation::{
    NSDate, NSError, NSOperatingSystemVersion, NSProcessInfo, NSProgressReporting, NSString,
};

use crate::{
    AppRefreshRequest, BackgroundCapabilities, BackgroundError, BootstrapConfig,
    ContinuedProcessingRequest, ContinuedProcessingStrategy, ProcessingRequest, TaskIdentifier,
    TaskKind,
};

const CAP_APP_REFRESH: u8 = 1 << 0;
const CAP_PROCESSING: u8 = 1 << 1;
const CAP_CONTINUED_PROCESSING: u8 = 1 << 2;
const CAP_LAUNCH_EVENTS: u8 = 1 << 3;
const CAP_CONTINUED_GPU: u8 = 1 << 4;

/// The message the original `notSupported` bridge error mapped to.
const NOT_SUPPORTED: &str = "requested background operation is unavailable on this iOS runtime";

/// The `#available(iOS _, macOS _)`-style gates of the original
/// implementation: `ios` applies on iOS proper, `macos` on Mac Catalyst
/// (where `NSProcessInfo` reports the macOS version — the paired boundary).
fn has_availability(ios: (isize, isize, isize), macos: (isize, isize, isize)) -> bool {
    let version = if cfg!(all(target_os = "ios", not(target_abi = "macabi"))) {
        ios
    } else {
        macos
    };
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: version.0,
        minorVersion: version.1,
        patchVersion: version.2,
    })
}

/// iOS 13 / macOS 10.15 (Catalyst) — the `BGTaskScheduler` availability gate.
fn has_task_scheduler() -> bool {
    has_availability((13, 0, 0), (10, 15, 0))
}

/// iOS 26 / macOS 26 — the continued-processing availability gate. The
/// original guarded these paths with a compile-time SDK flag too; the
/// extern declarations resolve classes lazily, so the runtime check alone
/// preserves the behavior on both old and new SDKs.
fn has_continued_processing() -> bool {
    has_availability((26, 0, 0), (26, 0, 0))
}

/// A `BGTask` held by the pending map. The generated bindings do not mark
/// the class `Send`/`Sync`, but tasks are delivered on the registered queue
/// and `setTaskCompletedWithSuccess:` is the documented way to finish one
/// from there; every access here is additionally serialized by the
/// `Mutex<PendingTasks>`.
#[derive(Debug)]
struct PendingTask(Retained<BGTask>);

// SAFETY: `BGTask` messages are sent under the pending map's mutex, and
// task completion is usable from the queue the task was delivered on.
#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "BGTask messaging is serialized through the pending map's mutex"
)]
unsafe impl Send for PendingTask {}
// SAFETY: see `Send`.
unsafe impl Sync for PendingTask {}

/// The process-wide `BGTaskScheduler`. The generated bindings do not mark
/// it `Send`/`Sync`, but `sharedScheduler` is a singleton documented for
/// use from the thread that schedules work, and the Rust surface may be
/// called from any thread.
#[derive(Debug)]
struct SharedScheduler(Retained<BGTaskScheduler>);

// SAFETY: `BGTaskScheduler` is a process-wide singleton; messaging it is
// safe from any thread the scheduler delivers work to.
#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "sharedScheduler is a process-wide singleton usable from the scheduling thread"
)]
unsafe impl Send for SharedScheduler {}
// SAFETY: see `Send`.
unsafe impl Sync for SharedScheduler {}

/// Live tasks by token, plus the monotonically increasing token counter.
#[derive(Debug)]
struct PendingTasks {
    map: HashMap<u64, PendingTask>,
    next_token: u64,
}

impl PendingTasks {
    /// `allocateTaskToken` — hands the task the next token and keeps it.
    fn allocate(&mut self, task: &BGTask) -> u64 {
        let token = self.next_token;
        self.next_token = self.next_token.wrapping_add(1);
        self.map.insert(token, PendingTask(task.retain()));
        token
    }
}

/// Everything a running `BackgroundRuntimeInner` shares with the
/// scheduler's blocks.
#[derive(Debug)]
struct RuntimeState {
    /// The crate's event-sender pointer, forwarded verbatim into
    /// [`crate::dispatch_launched_event`] / [`crate::dispatch_expired_event`].
    event_ctx: u64,
    /// `Arc::as_ptr` of this very state — the opaque handle `BackgroundTask`
    /// echoes back into the sys functions.
    handle: u64,
    /// The process-wide task scheduler.
    scheduler: SharedScheduler,
    pending: Mutex<PendingTasks>,
}

impl RuntimeState {
    /// `register(_:)` per entry: installs a launch handler that allocates a
    /// task token, wires the expiration handler, and dispatches the launch
    /// event.
    fn register(self: &Arc<Self>, identifier: &str, kind: u8) -> Result<(), BackgroundError> {
        let weak = Arc::downgrade(self);
        let identifier_string = identifier.to_owned();
        let launch_handler = RcBlock::new(move |task: NonNull<BGTask>| {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            // SAFETY: the scheduler hands the launch handler a live `BGTask`
            // for the duration of the call; the runtime keeps it in the
            // pending map.
            let task = unsafe { task.as_ref() };
            let task_token = runtime
                .pending
                .lock()
                .expect("background pending-tasks lock poisoned")
                .allocate(task);
            runtime.install_expiration_handler(task, task_token, &identifier_string, kind);
            crate::dispatch_launched_event(
                runtime.event_ctx,
                runtime.handle,
                task_token,
                &identifier_string,
                kind,
            );
        });
        // SAFETY: `registerForTaskWithIdentifier:usingQueue:launchHandler:`
        // copies the handler block; `nil` queue means the system picks it.
        let registered = unsafe {
            self.scheduler
                .0
                .registerForTaskWithIdentifier_usingQueue_launchHandler(
                    &NSString::from_str(identifier),
                    None,
                    &launch_handler,
                )
        };
        if !registered {
            return Err(BackgroundError::ConfigurationMissing(format!(
                "failed to register task identifier `{identifier}`; ensure it exists in BGTaskSchedulerPermittedIdentifiers"
            )));
        }
        Ok(())
    }

    /// Sets `task.expirationHandler` to a block that frees the token and
    /// dispatches the expired event — the same `[weak self]` wiring the
    /// original used.
    fn install_expiration_handler(
        self: &Arc<Self>,
        task: &BGTask,
        task_token: u64,
        identifier: &str,
        kind: u8,
    ) {
        let weak = Arc::downgrade(self);
        let identifier = identifier.to_owned();
        let expiration_handler = RcBlock::new(move || {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            runtime
                .pending
                .lock()
                .expect("background pending-tasks lock poisoned")
                .map
                .remove(&task_token);
            crate::dispatch_expired_event(runtime.event_ctx, &identifier, kind);
        });
        // SAFETY: `setExpirationHandler:` copies the block; the block only
        // weakly owns the runtime so it outlives the task without cycles.
        unsafe { task.setExpirationHandler(Some(&expiration_handler)) };
    }

    /// `shutdown()` — completes every pending task as failed.
    fn shutdown(&self) {
        let pending = {
            let mut pending = self
                .pending
                .lock()
                .expect("background pending-tasks lock poisoned");
            std::mem::take(&mut pending.map)
        };
        for (_, task) in pending {
            // SAFETY: the tasks were live when the scheduler delivered them.
            unsafe { task.0.setTaskCompletedWithSuccess(false) };
        }
    }
}

/// `mapSchedulerError` — `BGTaskScheduler` errors carry their raw code.
fn map_scheduler_error(error: &NSError) -> BackgroundError {
    // SAFETY: an immutable extern string constant of BackgroundTasks;
    // read-only access.
    if &*error.domain() == unsafe { BGTaskSchedulerErrorDomain } {
        return BackgroundError::SchedulerRejected {
            code: i32::try_from(error.code()).unwrap_or(-1),
            message: error.localizedDescription().to_string(),
        };
    }
    BackgroundError::Platform(error.localizedDescription().to_string())
}

/// iOS runtime state.
#[derive(Debug)]
pub struct BackgroundRuntimeInner {
    state: Arc<RuntimeState>,
}

impl BackgroundRuntimeInner {
    pub fn initialize(event_ctx: u64, config: &BootstrapConfig) -> Result<Self, BackgroundError> {
        if !has_task_scheduler() {
            return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
        }

        // SAFETY: `sharedScheduler` is a process-wide singleton accessor.
        let scheduler = unsafe { SharedScheduler(BGTaskScheduler::sharedScheduler()) };
        let mut state = Arc::new(RuntimeState {
            event_ctx,
            handle: 0,
            scheduler,
            pending: Mutex::new(PendingTasks {
                map: HashMap::new(),
                next_token: 1,
            }),
        });
        let handle = Arc::as_ptr(&state) as usize as u64;
        Arc::get_mut(&mut state)
            .expect("the only strong reference is ours")
            .handle = handle;

        let mut registrations: Vec<(&str, u8)> = Vec::new();
        registrations.extend(
            config
                .app_refresh_identifiers()
                .iter()
                .map(|identifier| (identifier.as_str(), TaskKind::AppRefresh.as_raw())),
        );
        registrations.extend(
            config
                .processing_identifiers()
                .iter()
                .map(|identifier| (identifier.as_str(), TaskKind::Processing.as_raw())),
        );
        registrations.extend(
            config
                .continued_processing_patterns()
                .iter()
                .map(|pattern| (pattern.as_str(), TaskKind::ContinuedProcessing.as_raw())),
        );
        for (identifier, kind) in registrations {
            state.register(identifier, kind)?;
        }

        Ok(Self { state })
    }

    /// `submit(_:)` mapped to `BackgroundError`s — the scheduler's own
    /// domain becomes `SchedulerRejected`, anything else `Platform`.
    fn submit_task_request(&self, request: &BGTaskRequest) -> Result<(), BackgroundError> {
        // SAFETY: `submitTaskRequest:error:` only reads the request.
        unsafe {
            self.state
                .scheduler
                .0
                .submitTaskRequest_error(request)
                .map_err(|error| map_scheduler_error(&error))
        }
    }

    pub fn submit_app_refresh(&self, request: AppRefreshRequest) -> Result<(), BackgroundError> {
        if !has_task_scheduler() {
            return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
        }
        let AppRefreshRequest {
            identifier,
            earliest_begin_after,
        } = request;
        // SAFETY: `initWithIdentifier:` on a fresh alloc.
        let task_request = unsafe {
            BGAppRefreshTaskRequest::initWithIdentifier(
                BGAppRefreshTaskRequest::alloc(),
                &NSString::from_str(identifier.as_str()),
            )
        };
        if let Some(begin_after) = earliest_begin_after {
            // SAFETY: property setter on a live request.
            unsafe {
                task_request.setEarliestBeginDate(Some(&NSDate::dateWithTimeIntervalSinceNow(
                    begin_after.as_secs_f64(),
                )));
            }
        }
        // SAFETY: `BGAppRefreshTaskRequest` is-a `BGTaskRequest`.
        self.submit_task_request(&Retained::into_super(task_request))
    }

    pub fn submit_processing(&self, request: ProcessingRequest) -> Result<(), BackgroundError> {
        if !has_task_scheduler() {
            return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
        }
        let ProcessingRequest {
            identifier,
            earliest_begin_after,
            requires_network_connectivity,
            requires_external_power,
        } = request;
        // SAFETY: `initWithIdentifier:` on a fresh alloc.
        let task_request = unsafe {
            BGProcessingTaskRequest::initWithIdentifier(
                BGProcessingTaskRequest::alloc(),
                &NSString::from_str(identifier.as_str()),
            )
        };
        // SAFETY: property setters on a live request.
        unsafe {
            if let Some(begin_after) = earliest_begin_after {
                task_request.setEarliestBeginDate(Some(&NSDate::dateWithTimeIntervalSinceNow(
                    begin_after.as_secs_f64(),
                )));
            }
            task_request.setRequiresNetworkConnectivity(requires_network_connectivity);
            task_request.setRequiresExternalPower(requires_external_power);
        }
        // SAFETY: `BGProcessingTaskRequest` is-a `BGTaskRequest`.
        self.submit_task_request(&Retained::into_super(task_request))
    }

    pub fn submit_continued_processing(
        &self,
        request: ContinuedProcessingRequest,
    ) -> Result<(), BackgroundError> {
        if !has_continued_processing() {
            return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
        }
        let ContinuedProcessingRequest {
            identifier,
            title,
            subtitle,
            strategy,
            requires_gpu,
        } = request;
        // SAFETY: `initWithIdentifier:title:subtitle:` on a fresh alloc.
        let task_request = unsafe {
            BGContinuedProcessingTaskRequest::initWithIdentifier_title_subtitle(
                BGContinuedProcessingTaskRequest::alloc(),
                &NSString::from_str(identifier.as_str()),
                &NSString::from_str(&title),
                &NSString::from_str(&subtitle),
            )
        };
        let strategy = match strategy {
            ContinuedProcessingStrategy::Fail => {
                BGContinuedProcessingTaskRequestSubmissionStrategy::Fail
            }
            ContinuedProcessingStrategy::Queue => {
                BGContinuedProcessingTaskRequestSubmissionStrategy::Queue
            }
        };
        // SAFETY: property setter on a live request.
        unsafe { task_request.setStrategy(strategy) };

        if requires_gpu {
            // SAFETY: class property read.
            if !unsafe { BGTaskScheduler::supportedResources() }
                .contains(BGContinuedProcessingTaskRequestResources::GPU)
            {
                return Err(BackgroundError::SchedulerRejected {
                    code: 0,
                    message:
                        "device does not support background GPU continued-processing resources"
                            .into(),
                });
            }
            // SAFETY: property setter on a live request.
            unsafe {
                task_request.setRequiredResources(BGContinuedProcessingTaskRequestResources::GPU);
            }
        }
        // SAFETY: `BGContinuedProcessingTaskRequest` is-a `BGTaskRequest`.
        self.submit_task_request(&Retained::into_super(task_request))
    }

    pub fn cancel(&self, identifier: &TaskIdentifier) -> Result<(), BackgroundError> {
        if !has_task_scheduler() {
            return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
        }
        // SAFETY: `cancelTaskRequestWithIdentifier:` only reads the string.
        unsafe {
            self.state
                .scheduler
                .0
                .cancelTaskRequestWithIdentifier(&NSString::from_str(identifier.as_str()));
        }
        Ok(())
    }

    pub fn cancel_all(&self) -> Result<(), BackgroundError> {
        if !has_task_scheduler() {
            return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
        }
        // SAFETY: `cancelAllTaskRequests` takes no arguments.
        unsafe { self.state.scheduler.0.cancelAllTaskRequests() };
        Ok(())
    }
}

impl Drop for BackgroundRuntimeInner {
    fn drop(&mut self) {
        self.state.shutdown();
    }
}

/// `runtimeFromHandle` — borrows the state behind a `runtime_handle`.
///
/// # Safety
/// `handle` must be a live runtime's handle — the original `Unmanaged`
/// borrow carried the same contract.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the handle is a host pointer value; supported Apple targets are 64-bit"
)]
fn runtime_from_handle(runtime_handle: u64) -> Result<&'static RuntimeState, BackgroundError> {
    if runtime_handle == 0 {
        return Err(BackgroundError::InvalidTaskToken(
            "runtime handle is invalid".into(),
        ));
    }
    // SAFETY: a non-zero handle was produced by `initialize` as the address
    // of the runtime's `Arc`; the `Arc` outlives every use of the handle
    // (task tokens die with the runtime).
    Ok(unsafe { &*(runtime_handle as usize as *const RuntimeState) })
}

#[must_use]
pub fn capabilities() -> BackgroundCapabilities {
    if !has_task_scheduler() {
        return BackgroundCapabilities {
            supports_app_refresh: false,
            supports_processing: false,
            supports_continued_processing: false,
            supports_continued_processing_gpu: false,
            supports_launch_events: false,
        };
    }
    let mut bits = CAP_APP_REFRESH | CAP_PROCESSING | CAP_LAUNCH_EVENTS;
    if has_continued_processing() {
        bits |= CAP_CONTINUED_PROCESSING;
        // SAFETY: class property read.
        if unsafe { BGTaskScheduler::supportedResources() }
            .contains(BGContinuedProcessingTaskRequestResources::GPU)
        {
            bits |= CAP_CONTINUED_GPU;
        }
    }
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
    let task = runtime_from_handle(runtime_handle)?
        .pending
        .lock()
        .expect("background pending-tasks lock poisoned")
        .map
        .remove(&task_token)
        .ok_or_else(|| {
            BackgroundError::InvalidTaskToken(format!(
                "unknown task token {task_token}; it may already be completed or expired"
            ))
        })?;
    // SAFETY: the task was live when the scheduler delivered it.
    unsafe { task.0.setTaskCompletedWithSuccess(success) };
    Ok(())
}

/// The pending continued-processing task for `task_token`, or the same
/// `InvalidTaskToken` message the original used.
fn pending_continued_task(
    state: &RuntimeState,
    task_token: u64,
) -> Result<Retained<BGContinuedProcessingTask>, BackgroundError> {
    let task = {
        let pending = state
            .pending
            .lock()
            .expect("background pending-tasks lock poisoned");
        pending
            .map
            .get(&task_token)
            .and_then(|task| task.0.downcast_ref::<BGContinuedProcessingTask>())
            .map(Message::retain)
    };
    task.ok_or_else(|| {
        BackgroundError::InvalidTaskToken(format!(
            "task token {task_token} does not reference a continued processing task"
        ))
    })
}

pub fn update_continued_processing_status(
    runtime_handle: u64,
    task_token: u64,
    title: &str,
    subtitle: &str,
) -> Result<(), BackgroundError> {
    if !has_continued_processing() {
        return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
    }
    let state = runtime_from_handle(runtime_handle)?;
    let task = pending_continued_task(state, task_token)?;
    // SAFETY: `updateTitle:subtitle:` only reads the strings.
    unsafe {
        task.updateTitle_subtitle(&NSString::from_str(title), &NSString::from_str(subtitle));
    }
    Ok(())
}

pub fn update_continued_processing_progress(
    runtime_handle: u64,
    task_token: u64,
    completed: u64,
    total: u64,
) -> Result<(), BackgroundError> {
    if !has_continued_processing() {
        return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
    }
    let state = runtime_from_handle(runtime_handle)?;
    let task = pending_continued_task(state, task_token)?;
    // `progress` is the task's own `NSProgress` (it conforms to
    // `NSProgressReporting`); setting unit counts is a plain property write.
    task.progress()
        .setTotalUnitCount(i64::try_from(total).unwrap_or(i64::MAX));
    task.progress()
        .setCompletedUnitCount(i64::try_from(completed).unwrap_or(i64::MAX));
    Ok(())
}
