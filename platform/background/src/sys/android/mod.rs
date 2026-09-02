//! Android background scheduler backend using `JobScheduler`.

use jni::objects::{JObject, JValue};
use jni::{Env, errors::Error as JniError, jni_sig, jni_str};
use waterkit_build::{AndroidError, jvm_and_context};

use crate::{
    AppRefreshRequest, BackgroundCapabilities, BackgroundError, BootstrapConfig,
    ContinuedProcessingRequest, ContinuedProcessingStrategy, ProcessingRequest, TaskIdentifier,
    TaskKind,
};

impl From<AndroidError> for BackgroundError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

const JOB_SCHEDULER_SERVICE: &str = "jobscheduler";
const JOB_SCHEDULER_RESULT_SUCCESS: i32 = 1;
const NETWORK_TYPE_NONE: i32 = 0;
const NETWORK_TYPE_ANY: i32 = 1;

/// Android runtime state.
#[derive(Debug)]
pub struct BackgroundRuntimeInner {
    job_service_class: String,
}

impl BackgroundRuntimeInner {
    pub fn initialize(_event_ctx: u64, config: &BootstrapConfig) -> Result<Self, BackgroundError> {
        let android_config = config.android_config_ref().ok_or_else(|| {
            BackgroundError::ConfigurationMissing(
                "android_config must be provided with a JobService class when running on Android"
                    .into(),
            )
        })?;

        Ok(Self {
            job_service_class: android_config.job_service_class().to_owned(),
        })
    }

    pub fn submit_app_refresh(&self, request: AppRefreshRequest) -> Result<(), BackgroundError> {
        let AppRefreshRequest {
            identifier,
            earliest_begin_after,
        } = request;
        let spec = JobSpec {
            identifier: &identifier,
            kind: TaskKind::AppRefresh,
            min_latency_ms: duration_ms(earliest_begin_after),
            requires_network_connectivity: false,
            requires_external_power: false,
        };
        schedule_job(&spec, &self.job_service_class)
    }

    pub fn submit_processing(&self, request: ProcessingRequest) -> Result<(), BackgroundError> {
        let ProcessingRequest {
            identifier,
            earliest_begin_after,
            requires_network_connectivity,
            requires_external_power,
        } = request;
        let spec = JobSpec {
            identifier: &identifier,
            kind: TaskKind::Processing,
            min_latency_ms: duration_ms(earliest_begin_after),
            requires_network_connectivity,
            requires_external_power,
        };
        schedule_job(&spec, &self.job_service_class)
    }

    pub fn submit_continued_processing(
        &self,
        request: ContinuedProcessingRequest,
    ) -> Result<(), BackgroundError> {
        let ContinuedProcessingRequest {
            identifier,
            strategy,
            requires_gpu,
            ..
        } = request;
        if requires_gpu {
            return Err(BackgroundError::ConfigurationMissing(
                "android continued processing does not support GPU requirements".into(),
            ));
        }

        if matches!(strategy, ContinuedProcessingStrategy::Fail) {
            return Err(BackgroundError::ConfigurationMissing(
                "android continued processing only supports queue strategy".into(),
            ));
        }

        let spec = JobSpec {
            identifier: &identifier,
            kind: TaskKind::ContinuedProcessing,
            min_latency_ms: 0,
            requires_network_connectivity: false,
            requires_external_power: false,
        };
        schedule_job(&spec, &self.job_service_class)
    }

    #[allow(
        clippy::unused_self,
        reason = "the cross-platform background runtime API is instance-based"
    )]
    pub fn cancel(&self, identifier: &TaskIdentifier) -> Result<(), BackgroundError> {
        let refresh_job_id = job_id_for_identifier(identifier, TaskKind::AppRefresh);
        let processing_job_id = job_id_for_identifier(identifier, TaskKind::Processing);
        let continued_job_id = job_id_for_identifier(identifier, TaskKind::ContinuedProcessing);

        with_env_context(|env, context| {
            let scheduler = get_job_scheduler(env, context)?;
            cancel_job(env, &scheduler, refresh_job_id)?;
            cancel_job(env, &scheduler, processing_job_id)?;
            cancel_job(env, &scheduler, continued_job_id)
        })
    }

    #[allow(
        clippy::unused_self,
        reason = "the cross-platform background runtime API is instance-based"
    )]
    pub fn cancel_all(&self) -> Result<(), BackgroundError> {
        with_env_context(|env, context| {
            let scheduler = get_job_scheduler(env, context)?;
            env.call_method(&scheduler, jni_str!("cancelAll"), jni_sig!("()V"), &[])
                .map_err(jni_error("JobScheduler.cancelAll"))?;
            Ok(())
        })
    }
}

#[must_use]
pub const fn capabilities() -> BackgroundCapabilities {
    BackgroundCapabilities {
        supports_app_refresh: true,
        supports_processing: true,
        supports_continued_processing: true,
        supports_continued_processing_gpu: false,
        supports_launch_events: false,
    }
}

pub fn complete_task(
    _runtime_handle: u64,
    _task_token: u64,
    _success: bool,
) -> Result<(), BackgroundError> {
    unreachable!(
        "waterkit-background: Android backend does not emit launch events and cannot complete tasks"
    )
}

#[derive(Debug)]
struct JobSpec<'a> {
    identifier: &'a TaskIdentifier,
    kind: TaskKind,
    min_latency_ms: i64,
    requires_network_connectivity: bool,
    requires_external_power: bool,
}

fn schedule_job(spec: &JobSpec<'_>, job_service_class: &str) -> Result<(), BackgroundError> {
    let JobSpec {
        identifier,
        kind,
        min_latency_ms,
        requires_network_connectivity,
        requires_external_power,
    } = *spec;
    let job_id = job_id_for_identifier(identifier, kind);

    with_env_context(|env, context| {
        let scheduler = get_job_scheduler(env, context)?;
        let spec = JobSpec {
            identifier,
            kind,
            min_latency_ms,
            requires_network_connectivity,
            requires_external_power,
        };
        let job_info = build_job_info(env, context, job_id, &spec, job_service_class)?;

        let result = env
            .call_method(
                &scheduler,
                jni_str!("schedule"),
                jni_sig!("(Landroid/app/job/JobInfo;)I"),
                &[JValue::Object(&job_info)],
            )
            .map_err(jni_error("JobScheduler.schedule"))?
            .i()
            .map_err(jni_error("JobScheduler.schedule result"))?;

        if result != JOB_SCHEDULER_RESULT_SUCCESS {
            return Err(BackgroundError::SchedulerRejected {
                code: result,
                message: format!("JobScheduler.schedule returned {result} for `{identifier}`"),
            });
        }

        Ok(())
    })
}

fn build_job_info<'local>(
    env: &mut Env<'local>,
    context: &JObject<'_>,
    job_id: i32,
    spec: &JobSpec<'_>,
    job_service_class: &str,
) -> Result<JObject<'local>, BackgroundError> {
    let service_class_name = env
        .new_string(job_service_class)
        .map_err(jni_error("new_string service class"))?;

    let component_name = env
        .new_object(
            jni_str!("android/content/ComponentName"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)V"),
            &[JValue::Object(context), JValue::Object(&service_class_name)],
        )
        .map_err(jni_error("new ComponentName"))?;

    let builder = env
        .new_object(
            jni_str!("android/app/job/JobInfo$Builder"),
            jni_sig!("(ILandroid/content/ComponentName;)V"),
            &[JValue::Int(job_id), JValue::Object(&component_name)],
        )
        .map_err(jni_error("new JobInfo.Builder"))?;

    let network_type = if spec.requires_network_connectivity {
        NETWORK_TYPE_ANY
    } else {
        NETWORK_TYPE_NONE
    };

    env.call_method(
        &builder,
        jni_str!("setRequiredNetworkType"),
        jni_sig!("(I)Landroid/app/job/JobInfo$Builder;"),
        &[JValue::Int(network_type)],
    )
    .map_err(jni_error("JobInfo.Builder.setRequiredNetworkType"))?;

    env.call_method(
        &builder,
        jni_str!("setRequiresCharging"),
        jni_sig!("(Z)Landroid/app/job/JobInfo$Builder;"),
        &[JValue::Bool(spec.requires_external_power)],
    )
    .map_err(jni_error("JobInfo.Builder.setRequiresCharging"))?;

    if spec.min_latency_ms > 0 {
        env.call_method(
            &builder,
            jni_str!("setMinimumLatency"),
            jni_sig!("(J)Landroid/app/job/JobInfo$Builder;"),
            &[JValue::Long(spec.min_latency_ms)],
        )
        .map_err(jni_error("JobInfo.Builder.setMinimumLatency"))?;
    }

    env.call_method(
        &builder,
        jni_str!("build"),
        jni_sig!("()Landroid/app/job/JobInfo;"),
        &[],
    )
    .map_err(jni_error("JobInfo.Builder.build"))?
    .l()
    .map_err(jni_error("JobInfo.Builder.build result"))
}

fn with_env_context<T>(
    operation: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, BackgroundError>,
) -> Result<T, BackgroundError> {
    let (vm, context) = jvm_and_context()?;
    vm.attach_current_thread(
        |env| -> Result<Result<T, BackgroundError>, jni::errors::Error> {
            Ok(operation(env, context.as_obj()))
        },
    )
    .map_err(|error| BackgroundError::Platform(format!("attach_current_thread: {error}")))?
}

fn get_job_scheduler<'local>(
    env: &mut Env<'local>,
    context: &JObject<'_>,
) -> Result<JObject<'local>, BackgroundError> {
    let service_name = env
        .new_string(JOB_SCHEDULER_SERVICE)
        .map_err(jni_error("new_string jobscheduler"))?;

    let scheduler = env
        .call_method(
            context,
            jni_str!("getSystemService"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
            &[JValue::Object(&service_name)],
        )
        .map_err(jni_error("Context.getSystemService"))?
        .l()
        .map_err(jni_error("Context.getSystemService result"))?;

    if scheduler.is_null() {
        return Err(BackgroundError::ConfigurationMissing(
            "Context.getSystemService(".to_owned() + JOB_SCHEDULER_SERVICE + ") returned null",
        ));
    }

    Ok(scheduler)
}

fn cancel_job(
    env: &mut Env<'_>,
    scheduler: &JObject<'_>,
    job_id: i32,
) -> Result<(), BackgroundError> {
    env.call_method(
        scheduler,
        jni_str!("cancel"),
        jni_sig!("(I)V"),
        &[JValue::Int(job_id)],
    )
    .map_err(jni_error("JobScheduler.cancel"))?;
    Ok(())
}

fn duration_ms(duration: Option<std::time::Duration>) -> i64 {
    duration
        .map(|value| value.as_millis())
        .and_then(|value| i64::try_from(value).ok())
        .unwrap_or(0)
}

fn jni_error(context: &'static str) -> impl FnOnce(JniError) -> BackgroundError {
    move |error| BackgroundError::Platform(format!("{context}: {error}"))
}

fn job_id_for_identifier(identifier: &TaskIdentifier, kind: TaskKind) -> i32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in identifier.as_str().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash ^= u32::from(kind.as_raw());

    let mut job_id = i32::try_from(hash & 0x7fff_ffff).unwrap_or(1);
    if job_id == 0 {
        job_id = 1;
    }
    job_id
}
