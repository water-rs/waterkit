//! Android background scheduler backend using `JobScheduler`.
#![allow(
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value,
    clippy::unused_self
)]

use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use jni::{JavaVM, errors::Error as JniError};

use crate::{
    AppRefreshRequest, BackgroundCapabilities, BackgroundError, BootstrapConfig,
    ContinuedProcessingRequest, ProcessingRequest, TaskIdentifier, TaskKind,
};

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
        let spec = JobSpec {
            identifier: request.identifier(),
            kind: TaskKind::AppRefresh,
            min_latency_ms: duration_ms(request.earliest_begin_after_value()),
            requires_network_connectivity: false,
            requires_external_power: false,
        };
        schedule_job(spec, &self.job_service_class)
    }

    pub fn submit_processing(&self, request: ProcessingRequest) -> Result<(), BackgroundError> {
        let spec = JobSpec {
            identifier: request.identifier(),
            kind: TaskKind::Processing,
            min_latency_ms: duration_ms(request.earliest_begin_after_value()),
            requires_network_connectivity: request.requires_network_connectivity_value(),
            requires_external_power: request.requires_external_power_value(),
        };
        schedule_job(spec, &self.job_service_class)
    }

    pub fn submit_continued_processing(
        &self,
        _request: ContinuedProcessingRequest,
    ) -> Result<(), BackgroundError> {
        Err(BackgroundError::NotSupported)
    }

    pub fn cancel(&self, identifier: &TaskIdentifier) -> Result<(), BackgroundError> {
        let refresh_job_id = job_id_for_identifier(identifier, TaskKind::AppRefresh);
        let processing_job_id = job_id_for_identifier(identifier, TaskKind::Processing);

        with_env_context(|env, context| {
            let scheduler = get_job_scheduler(env, context)?;
            cancel_job(env, &scheduler, refresh_job_id)?;
            cancel_job(env, &scheduler, processing_job_id)
        })
    }

    pub fn cancel_all(&self) -> Result<(), BackgroundError> {
        with_env_context(|env, context| {
            let scheduler = get_job_scheduler(env, context)?;
            env.call_method(&scheduler, "cancelAll", "()V", &[])
                .map_err(jni_error("JobScheduler.cancelAll"))?;
            Ok(())
        })
    }
}

#[must_use]
pub fn capabilities() -> BackgroundCapabilities {
    BackgroundCapabilities {
        supports_app_refresh: true,
        supports_processing: true,
        supports_continued_processing: false,
        supports_continued_processing_gpu: false,
        supports_launch_events: false,
    }
}

pub fn complete_task(
    _runtime_handle: u64,
    _task_token: u64,
    _success: bool,
) -> Result<(), BackgroundError> {
    Err(BackgroundError::NotSupported)
}

#[derive(Debug)]
struct JobSpec<'a> {
    identifier: &'a TaskIdentifier,
    kind: TaskKind,
    min_latency_ms: i64,
    requires_network_connectivity: bool,
    requires_external_power: bool,
}

fn schedule_job(spec: JobSpec<'_>, job_service_class: &str) -> Result<(), BackgroundError> {
    let job_id = job_id_for_identifier(spec.identifier, spec.kind);

    with_env_context(|env, context| {
        let scheduler = get_job_scheduler(env, context)?;
        let job_info = build_job_info(env, context, job_id, &spec, job_service_class)?;

        let result = env
            .call_method(
                &scheduler,
                "schedule",
                "(Landroid/app/job/JobInfo;)I",
                &[JValue::Object(&job_info)],
            )
            .map_err(jni_error("JobScheduler.schedule"))?
            .i()
            .map_err(jni_error("JobScheduler.schedule result"))?;

        if result != JOB_SCHEDULER_RESULT_SUCCESS {
            return Err(BackgroundError::SchedulerRejected {
                code: result,
                message: format!(
                    "JobScheduler.schedule returned {result} for `{}`",
                    spec.identifier
                ),
            });
        }

        Ok(())
    })
}

fn build_job_info<'a>(
    env: &mut JNIEnv<'a>,
    context: &JObject<'static>,
    job_id: i32,
    spec: &JobSpec<'_>,
    job_service_class: &str,
) -> Result<JObject<'a>, BackgroundError> {
    let component_name_class = env
        .find_class("android/content/ComponentName")
        .map_err(jni_error("find ComponentName"))?;

    let service_class_name = env
        .new_string(job_service_class)
        .map_err(jni_error("new_string service class"))?;

    let component_name = env
        .new_object(
            component_name_class,
            "(Landroid/content/Context;Ljava/lang/String;)V",
            &[JValue::Object(context), JValue::Object(&service_class_name)],
        )
        .map_err(jni_error("new ComponentName"))?;

    let builder_class = env
        .find_class("android/app/job/JobInfo$Builder")
        .map_err(jni_error("find JobInfo$Builder"))?;

    let builder = env
        .new_object(
            builder_class,
            "(ILandroid/content/ComponentName;)V",
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
        "setRequiredNetworkType",
        "(I)Landroid/app/job/JobInfo$Builder;",
        &[JValue::Int(network_type)],
    )
    .map_err(jni_error("JobInfo.Builder.setRequiredNetworkType"))?;

    env.call_method(
        &builder,
        "setRequiresCharging",
        "(Z)Landroid/app/job/JobInfo$Builder;",
        &[JValue::Bool(bool_to_jni(spec.requires_external_power))],
    )
    .map_err(jni_error("JobInfo.Builder.setRequiresCharging"))?;

    if spec.min_latency_ms > 0 {
        env.call_method(
            &builder,
            "setMinimumLatency",
            "(J)Landroid/app/job/JobInfo$Builder;",
            &[JValue::Long(spec.min_latency_ms)],
        )
        .map_err(jni_error("JobInfo.Builder.setMinimumLatency"))?;
    }

    env.call_method(&builder, "build", "()Landroid/app/job/JobInfo;", &[])
        .map_err(jni_error("JobInfo.Builder.build"))?
        .l()
        .map_err(jni_error("JobInfo.Builder.build result"))
}

fn with_env_context<T>(
    operation: impl FnOnce(&mut JNIEnv<'_>, &JObject<'static>) -> Result<T, BackgroundError>,
) -> Result<T, BackgroundError> {
    let (vm, context_ref) = ensure_context_global()?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| BackgroundError::Platform(format!("attach_current_thread: {error}")))?;

    operation(&mut env, context_ref.as_obj())
}

fn ensure_context_global() -> Result<(JavaVM, GlobalRef), BackgroundError> {
    let android_context = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android_context.vm().cast()) }
        .map_err(|error| BackgroundError::Platform(format!("from_raw vm: {error}")))?;

    let context = unsafe { JObject::from_raw(android_context.context().cast()) };

    let context_ref = {
        let env = vm.attach_current_thread().map_err(|error| {
            BackgroundError::Platform(format!("attach_current_thread: {error}"))
        })?;
        env.new_global_ref(&context)
            .map_err(jni_error("new_global_ref context"))?
    };

    Ok((vm, context_ref))
}

fn get_job_scheduler<'a>(
    env: &mut JNIEnv<'a>,
    context: &JObject<'static>,
) -> Result<JObject<'a>, BackgroundError> {
    let service_name = env
        .new_string(JOB_SCHEDULER_SERVICE)
        .map_err(jni_error("new_string jobscheduler"))?;

    let scheduler = env
        .call_method(
            context,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
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
    env: &mut JNIEnv<'_>,
    scheduler: &JObject<'_>,
    job_id: i32,
) -> Result<(), BackgroundError> {
    env.call_method(scheduler, "cancel", "(I)V", &[JValue::Int(job_id)])
        .map_err(jni_error("JobScheduler.cancel"))?;
    Ok(())
}

fn duration_ms(duration: Option<std::time::Duration>) -> i64 {
    duration
        .map(|value| value.as_millis())
        .and_then(|value| i64::try_from(value).ok())
        .unwrap_or(0)
}

fn bool_to_jni(value: bool) -> u8 {
    u8::from(value)
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
