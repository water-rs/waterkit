//! iOS background task backend via `BackgroundTasks` through `objc2`.
//!
//! The runtime is a plain Rust [`RuntimeState`] owned by an `Arc` inside
//! [`BackgroundRuntimeInner`]; the scheduler's launch/expiration blocks
//! capture a `Weak` to it, matching the original `[weak self]` lifecycle.
//! Tasks are registered on the main dispatch queue, so every `BGTask`
//! arrives on the main thread and lives there as a
//! [`MainThreadBound`]-wrapped `Retained` — callers that are not on the
//! main thread hop over with `DispatchQueue::main().exec_async` and await
//! a `futures` oneshot for the result.

use std::sync::{Arc, Mutex};

use block2::RcBlock;
use core::ptr::NonNull;
use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::rc::{Retained, Weak};
use objc2::{AnyThread, MainThreadMarker, Message};
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

/// Everything a running `BackgroundRuntimeInner` shares with the
/// scheduler's blocks. All `BGTask` messaging happens on the main queue.
#[derive(Debug)]
struct RuntimeState {
    /// The launch/expiration event channel into the crate.
    events_tx: async_channel::Sender<crate::BackgroundEvent>,
    /// Every task the scheduler has delivered and not yet completed or
    /// expired. `MainThreadBound` keeps the objects main-thread-only while
    /// letting the map itself be `Sync`.
    pending: Mutex<Vec<Arc<MainThreadBound<Retained<BGTask>>>>>,
}

impl RuntimeState {
    /// `register(_:)` per entry: installs a launch handler on the main
    /// queue that keeps the task in `pending`, wires its expiration
    /// handler, and emits the launch event.
    fn register(self: &Arc<Self>, identifier: &str, kind: TaskKind) -> Result<(), BackgroundError> {
        let weak = Arc::downgrade(self);
        let identifier_string = identifier.to_owned();
        let launch_handler = RcBlock::new(move |task: NonNull<BGTask>| {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            // SAFETY: the scheduler hands the launch handler a live `BGTask`
            // for the duration of the call; the runtime retains it in
            // `pending`.
            let task = unsafe { task.as_ref() };
            let mtm = MainThreadMarker::new()
                .expect("the launch handler is registered on the main queue");
            let task = Arc::new(MainThreadBound::new(task.retain(), mtm));
            runtime
                .pending
                .lock()
                .expect("background pending-tasks lock poisoned")
                .push(Arc::clone(&task));
            runtime.install_expiration_handler(&task, mtm, &identifier_string, kind);
            crate::dispatch_launched_event(
                &runtime.events_tx,
                TaskHandle {
                    task,
                    runtime: Arc::downgrade(&runtime),
                },
                &identifier_string,
                kind,
            );
        });
        // SAFETY: `registerForTaskWithIdentifier:usingQueue:launchHandler:`
        // copies the handler block; the main queue delivers the handler on
        // the main thread.
        let registered = unsafe {
            BGTaskScheduler::sharedScheduler()
                .registerForTaskWithIdentifier_usingQueue_launchHandler(
                    &NSString::from_str(identifier),
                    Some(DispatchQueue::main()),
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

    /// Sets `task.expirationHandler` to a block that drops the task from
    /// `pending` and emits the expired event — the same `[weak self]`
    /// wiring the original used. The task itself is captured weakly: the
    /// block lives on the task, so a strong capture would be a cycle.
    fn install_expiration_handler(
        self: &Arc<Self>,
        task: &Arc<MainThreadBound<Retained<BGTask>>>,
        mtm: MainThreadMarker,
        identifier: &str,
        kind: TaskKind,
    ) {
        let weak = Arc::downgrade(self);
        let identifier = identifier.to_owned();
        let weak_task = Weak::new(&**task.get(mtm));
        let expiration_handler = RcBlock::new(move || {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            let mtm = MainThreadMarker::new()
                .expect("expiration handlers run on the queue the task was delivered on");
            if let Some(task) = weak_task.load() {
                runtime
                    .pending
                    .lock()
                    .expect("background pending-tasks lock poisoned")
                    .retain(|entry| !std::ptr::eq(&raw const **entry.get(mtm), &raw const *task));
            }
            crate::dispatch_expired_event(&runtime.events_tx, &identifier, kind);
        });
        // SAFETY: `setExpirationHandler:` copies the block; the block only
        // weakly owns the runtime and the task, so it cannot keep either
        // alive or dangle.
        unsafe {
            task.get(mtm)
                .setExpirationHandler(Some(&expiration_handler))
        };
    }

    /// `shutdown()` — completes every pending task as failed. Runs on the
    /// main thread.
    fn shutdown(&self, mtm: MainThreadMarker) {
        let pending = {
            let mut pending = self
                .pending
                .lock()
                .expect("background pending-tasks lock poisoned");
            std::mem::take(&mut *pending)
        };
        for task in pending {
            // SAFETY: the tasks were live when the scheduler delivered
            // them; completing an already-finished task is a no-op.
            unsafe { task.get(mtm).setTaskCompletedWithSuccess(false) };
        }
    }
}

/// The Apple half of a launched [`crate::BackgroundTask`]: owns the task
/// object and knows which runtime it belongs to. A task outliving its
/// runtime fails with an error, never a dangling pointer.
#[derive(Debug, Clone)]
pub struct TaskHandle {
    /// The `BGTask` the scheduler delivered, owned and main-thread-bound.
    task: Arc<MainThreadBound<Retained<BGTask>>>,
    /// The runtime that delivered it.
    runtime: std::sync::Weak<RuntimeState>,
}

/// Runs `work` on the main queue: inline when already there, otherwise via
/// `exec_async` with a oneshot carrying the result back to the awaiting
/// caller.
async fn run_on_main_queue<T, F>(work: F) -> T
where
    T: Send + 'static,
    F: FnOnce(MainThreadMarker) -> T + Send + 'static,
{
    if let Some(mtm) = MainThreadMarker::new() {
        return work(mtm);
    }
    let (sender, receiver) = futures_channel::oneshot::channel();
    DispatchQueue::main().exec_async(move || {
        let mtm = MainThreadMarker::new().expect("the main queue only runs on the main thread");
        let _ = sender.send(work(mtm));
    });
    receiver
        .await
        .expect("the main queue dropped the work item before it ran")
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
    pub fn initialize(
        events_tx: async_channel::Sender<crate::BackgroundEvent>,
        config: &BootstrapConfig,
    ) -> Result<Self, BackgroundError> {
        if !has_task_scheduler() {
            return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
        }

        let state = Arc::new(RuntimeState {
            events_tx,
            pending: Mutex::new(Vec::new()),
        });

        let mut registrations: Vec<(&str, TaskKind)> = Vec::new();
        registrations.extend(
            config
                .app_refresh_identifiers()
                .iter()
                .map(|identifier| (identifier.as_str(), TaskKind::AppRefresh)),
        );
        registrations.extend(
            config
                .processing_identifiers()
                .iter()
                .map(|identifier| (identifier.as_str(), TaskKind::Processing)),
        );
        registrations.extend(
            config
                .continued_processing_patterns()
                .iter()
                .map(|pattern| (pattern.as_str(), TaskKind::ContinuedProcessing)),
        );
        for (identifier, kind) in registrations {
            state.register(identifier, kind)?;
        }

        Ok(Self { state })
    }

    /// `submit(_:)` mapped to `BackgroundError`s — the scheduler's own
    /// domain becomes `SchedulerRejected`, anything else `Platform`.
    #[expect(
        clippy::unused_self,
        reason = "the cross-platform background runtime API is instance-based"
    )]
    fn submit_task_request(&self, request: &BGTaskRequest) -> Result<(), BackgroundError> {
        // SAFETY: `submitTaskRequest:error:` only reads the request.
        unsafe {
            BGTaskScheduler::sharedScheduler()
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
            // SAFETY: class property read on an iOS-26-gated path.
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

    #[expect(
        clippy::unused_self,
        reason = "the cross-platform background runtime API is instance-based"
    )]
    pub fn cancel(&self, identifier: &TaskIdentifier) -> Result<(), BackgroundError> {
        if !has_task_scheduler() {
            return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
        }
        // SAFETY: `cancelTaskRequestWithIdentifier:` only reads the string.
        unsafe {
            BGTaskScheduler::sharedScheduler()
                .cancelTaskRequestWithIdentifier(&NSString::from_str(identifier.as_str()));
        }
        Ok(())
    }

    #[expect(
        clippy::unused_self,
        reason = "the cross-platform background runtime API is instance-based"
    )]
    pub fn cancel_all(&self) -> Result<(), BackgroundError> {
        if !has_task_scheduler() {
            return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
        }
        // SAFETY: `cancelAllTaskRequests` takes no arguments.
        unsafe { BGTaskScheduler::sharedScheduler().cancelAllTaskRequests() };
        Ok(())
    }
}

impl Drop for BackgroundRuntimeInner {
    fn drop(&mut self) {
        let state = Arc::clone(&self.state);
        if let Some(mtm) = MainThreadMarker::new() {
            state.shutdown(mtm);
        } else {
            DispatchQueue::main().exec_async(move || {
                let mtm =
                    MainThreadMarker::new().expect("the main queue only runs on the main thread");
                state.shutdown(mtm);
            });
        }
    }
}

#[must_use]
pub fn capabilities() -> BackgroundCapabilities {
    let scheduler = has_task_scheduler();
    BackgroundCapabilities {
        supports_app_refresh: scheduler,
        supports_processing: scheduler,
        supports_launch_events: scheduler,
        supports_continued_processing: has_continued_processing(),
        supports_continued_processing_gpu: has_continued_processing()
            // SAFETY: class property read on an iOS-26-gated path.
            && unsafe { BGTaskScheduler::supportedResources() }
                .contains(BGContinuedProcessingTaskRequestResources::GPU),
    }
}

/// `complete(_:)` — drops the task from `pending` and completes it with
/// the given success flag.
pub async fn complete_task(handle: &TaskHandle, success: bool) -> Result<(), BackgroundError> {
    let Some(runtime) = handle.runtime.upgrade() else {
        return Err(BackgroundError::InvalidTaskToken(
            "the task's runtime is no longer alive".into(),
        ));
    };
    let task = Arc::clone(&handle.task);
    run_on_main_queue(move |mtm| {
        let task = {
            let mut pending = runtime
                .pending
                .lock()
                .expect("background pending-tasks lock poisoned");
            let Some(index) = pending.iter().position(|entry| Arc::ptr_eq(entry, &task)) else {
                return Err(BackgroundError::InvalidTaskToken(
                    "the task is no longer pending; it may already be completed or expired".into(),
                ));
            };
            pending.remove(index)
        };
        // SAFETY: the task was live when the scheduler delivered it.
        unsafe { task.get(mtm).setTaskCompletedWithSuccess(success) };
        Ok(())
    })
    .await
}

/// `updateContinuedStatus(_:_:)` / `updateContinuedProgress(_:_:)`'s
/// shared lookup: the task must still be pending and must be a
/// `BGContinuedProcessingTask`, matching the original's single
/// `InvalidTaskToken` message.
fn pending_continued_task(
    runtime: &RuntimeState,
    handle: &TaskHandle,
    mtm: MainThreadMarker,
) -> Result<Retained<BGContinuedProcessingTask>, BackgroundError> {
    let error = || {
        BackgroundError::InvalidTaskToken(
            "the task does not reference a pending continued processing task".into(),
        )
    };
    let is_pending = {
        let pending = runtime
            .pending
            .lock()
            .expect("background pending-tasks lock poisoned");
        pending.iter().any(|entry| Arc::ptr_eq(entry, &handle.task))
    };
    if !is_pending {
        return Err(error());
    }
    handle
        .task
        .get(mtm)
        .downcast_ref::<BGContinuedProcessingTask>()
        .map(Message::retain)
        .ok_or_else(error)
}

/// `updateContinuedStatus` — retitles a live continued-processing task.
pub async fn update_continued_processing_status(
    handle: &TaskHandle,
    title: &str,
    subtitle: &str,
) -> Result<(), BackgroundError> {
    if !has_continued_processing() {
        return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
    }
    let Some(runtime) = handle.runtime.upgrade() else {
        return Err(BackgroundError::InvalidTaskToken(
            "the task's runtime is no longer alive".into(),
        ));
    };
    let title = title.to_owned();
    let subtitle = subtitle.to_owned();
    let task = Arc::clone(&handle.task);
    run_on_main_queue(move |mtm| {
        let task = pending_continued_task(
            &runtime,
            &TaskHandle {
                task,
                runtime: Arc::downgrade(&runtime),
            },
            mtm,
        )?;
        // SAFETY: `updateTitle:subtitle:` only reads the strings.
        unsafe {
            task.updateTitle_subtitle(&NSString::from_str(&title), &NSString::from_str(&subtitle));
        }
        Ok(())
    })
    .await
}

/// `updateContinuedProgress` — updates a live continued-processing task's
/// `NSProgress` counters.
pub async fn update_continued_processing_progress(
    handle: &TaskHandle,
    completed: u64,
    total: u64,
) -> Result<(), BackgroundError> {
    if !has_continued_processing() {
        return Err(BackgroundError::Platform(NOT_SUPPORTED.into()));
    }
    let Some(runtime) = handle.runtime.upgrade() else {
        return Err(BackgroundError::InvalidTaskToken(
            "the task's runtime is no longer alive".into(),
        ));
    };
    let task = Arc::clone(&handle.task);
    run_on_main_queue(move |mtm| {
        let task = pending_continued_task(
            &runtime,
            &TaskHandle {
                task,
                runtime: Arc::downgrade(&runtime),
            },
            mtm,
        )?;
        // `progress` is the task's own `NSProgress` (it conforms to
        // `NSProgressReporting`); setting unit counts is a plain property
        // write.
        task.progress()
            .setTotalUnitCount(i64::try_from(total).unwrap_or(i64::MAX));
        task.progress()
            .setCompletedUnitCount(i64::try_from(completed).unwrap_or(i64::MAX));
        Ok(())
    })
    .await
}
