//! Android activity-result contracts shared by `WaterKit` crates.
//!
//! Enable the `activity-result` feature on `waterkit-build`, build an Android
//! [`Intent`](https://developer.android.com/reference/android/content/Intent)
//! or `IntentSender` through JNI, then await the returned
//! [`PendingActivityResult`]. The helper uses the host
//! `ComponentActivity`'s `AndroidX` activity-result registry, so applications do
//! not need to forward request codes through `onActivityResult`.

use super::{android_error_with_pending_exception, decode_string};
use futures_channel::oneshot;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JClass, JObject, JValue};
use jni::sys::{jint, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::task::{Context, Poll};

static HELPER: super::DexHelper = super::DexHelper::new("waterkit.build.ActivityResultHelper");
static NEXT_REQUEST_ID: AtomicI64 = AtomicI64::new(1);

type ResultSender = oneshot::Sender<Result<ActivityResult, ActivityResultError>>;

fn pending_results() -> &'static Mutex<HashMap<i64, ResultSender>> {
    static RESULTS: OnceLock<Mutex<HashMap<i64, ResultSender>>> = OnceLock::new();
    RESULTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The result code returned by an Android activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultCode {
    /// The launched activity completed successfully.
    Ok,
    /// The launched activity was cancelled.
    Canceled,
    /// An application-defined result code.
    Custom(i32),
}

impl From<i32> for ResultCode {
    fn from(value: i32) -> Self {
        match value {
            -1 => Self::Ok,
            0 => Self::Canceled,
            value => Self::Custom(value),
        }
    }
}

/// The result delivered by an Android activity.
#[derive(Debug)]
pub struct ActivityResult {
    code: ResultCode,
    data: Option<Global<JObject<'static>>>,
}

impl ActivityResult {
    /// Returns the Android result code.
    #[must_use]
    pub const fn code(&self) -> ResultCode {
        self.code
    }

    /// Returns the returned intent, if one was supplied.
    #[must_use]
    pub const fn data(&self) -> Option<&Global<JObject<'static>>> {
        self.data.as_ref()
    }

    /// Consumes the result and returns its returned intent, if one was supplied.
    #[must_use]
    pub fn into_data(self) -> Option<Global<JObject<'static>>> {
        self.data
    }
}

/// An error while launching or awaiting an Android activity result.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ActivityResultError {
    /// The context published through `ndk_context` is not a
    /// `ComponentActivity`.
    #[error(
        "activity results need the Context published through ndk_context to be an \
         androidx.activity.ComponentActivity (the host activity); it is {context_class}"
    )]
    NotComponentActivity {
        /// The fully qualified class name of the published context.
        context_class: String,
    },

    /// The host activity was destroyed before the launched activity returned.
    #[error("the host activity was destroyed before the launched activity returned its result")]
    ActivityDestroyed,

    /// Android could not launch the requested activity.
    #[error("could not launch: {0}")]
    Launch(String),

    /// A JNI operation failed.
    #[error(transparent)]
    Android(#[from] super::AndroidError),
}

/// A future for an Android activity result.
#[must_use]
#[derive(Debug)]
pub struct PendingActivityResult(oneshot::Receiver<Result<ActivityResult, ActivityResultError>>);

impl Future for PendingActivityResult {
    type Output = Result<ActivityResult, ActivityResultError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.get_mut().0).poll(context) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => {
                panic!("waterkit-build activity-result sender dropped before delivering a result")
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Starts an Android activity for result.
///
/// The `intent` is launched through the `ComponentActivity` published by
/// `ndk_context`. The returned future resolves when the activity returns,
/// when the host activity is destroyed, or when launch fails.
///
/// # Errors
///
/// Returns an error if the Android context or helper cannot be resolved, or
/// if the host context is not a `ComponentActivity`.
pub fn start_activity_for_result(
    env: &mut Env<'_>,
    intent: &JObject<'_>,
) -> Result<PendingActivityResult, ActivityResultError> {
    let (_, context) = super::jvm_and_context()?;
    launch(env, context.as_obj(), intent, LaunchKind::Activity)
}

/// Starts an Android intent sender for result.
///
/// The `intent_sender` is launched through the `ComponentActivity` published
/// by `ndk_context`. The returned future resolves when the sender returns,
/// when the host activity is destroyed, or when launch fails.
///
/// # Errors
///
/// Returns an error if the Android context or helper cannot be resolved, or
/// if the host context is not a `ComponentActivity`.
pub fn start_intent_sender_for_result(
    env: &mut Env<'_>,
    intent_sender: &JObject<'_>,
) -> Result<PendingActivityResult, ActivityResultError> {
    let (_, context) = super::jvm_and_context()?;
    launch(
        env,
        context.as_obj(),
        intent_sender,
        LaunchKind::IntentSender,
    )
}

#[derive(Clone, Copy)]
enum LaunchKind {
    Activity,
    IntentSender,
}

fn launch(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    input: &JObject<'_>,
    kind: LaunchKind,
) -> Result<PendingActivityResult, ActivityResultError> {
    let helper_class = HELPER.class(env, context)?;
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = oneshot::channel();
    pending_results()
        .lock()
        .unwrap_or_else(|error| panic!("waterkit-build activity-result map poisoned: {error}"))
        .insert(request_id, sender);

    let launch_result = match kind {
        LaunchKind::Activity => env.call_static_method(
            helper_class,
            jni_str!("startActivityForResult"),
            jni_sig!("(Landroid/content/Context;JLandroid/content/Intent;)Z"),
            &[
                JValue::Object(context),
                JValue::Long(request_id),
                JValue::Object(input),
            ],
        ),
        LaunchKind::IntentSender => env.call_static_method(
            helper_class,
            jni_str!("startIntentSenderForResult"),
            jni_sig!("(Landroid/content/Context;JLandroid/content/IntentSender;)Z"),
            &[
                JValue::Object(context),
                JValue::Long(request_id),
                JValue::Object(input),
            ],
        ),
    };

    let launched = match launch_result {
        Ok(value) => match value.z() {
            Ok(launched) => launched,
            Err(error) => {
                remove_pending(request_id);
                return Err(ActivityResultError::Android(super::AndroidError::from(
                    error,
                )));
            }
        },
        Err(error) => {
            remove_pending(request_id);
            return Err(ActivityResultError::Android(
                android_error_with_pending_exception(env, error),
            ));
        }
    };

    if !launched {
        remove_pending(request_id);
        return Err(ActivityResultError::NotComponentActivity {
            context_class: context_class_name(env, context)?,
        });
    }

    Ok(PendingActivityResult(receiver))
}

fn context_class_name(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<String, ActivityResultError> {
    let class_name = (|| -> jni::errors::Result<JObject<'_>> {
        let class = env
            .call_method(
                context,
                jni_str!("getClass"),
                jni_sig!("()Ljava/lang/Class;"),
                &[],
            )?
            .l()?;
        env.call_method(
            &class,
            jni_str!("getName"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )?
        .l()
    })();

    let class_name = match class_name {
        Ok(value) => value,
        Err(error) => {
            return Err(ActivityResultError::Android(
                android_error_with_pending_exception(env, error),
            ));
        }
    };
    Ok(decode_string(env, &class_name)?)
}

fn remove_pending(request_id: i64) -> ResultSender {
    pending_results()
        .lock()
        .unwrap_or_else(|error| panic!("waterkit-build activity-result map poisoned: {error}"))
        .remove(&request_id)
        .unwrap_or_else(|| {
            panic!("waterkit-build: unknown activity-result request id: {request_id}")
        })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_build_ActivityResultHelper_deliverResult<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    request_id: jlong,
    result_code: jint,
    data: JObject<'caller>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let sender = remove_pending(request_id);
        let data = if data.is_null() {
            None
        } else {
            match env.new_global_ref(&data) {
                Ok(data) => Some(data),
                Err(error) => {
                    let _ = sender.send(Err(ActivityResultError::Android(
                        android_error_with_pending_exception(env, error),
                    )));
                    return Ok(());
                }
            }
        };
        let _ = sender.send(Ok(ActivityResult {
            code: result_code.into(),
            data,
        }));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_build_ActivityResultHelper_deliverDestroyed<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    request_id: jlong,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        let sender = remove_pending(request_id);
        let _ = sender.send(Err(ActivityResultError::ActivityDestroyed));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_build_ActivityResultHelper_deliverLaunchFailure<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    request_id: jlong,
    description: JObject<'caller>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let description = decode_string(env, &description).map_err(|error| error.0)?;
        let sender = remove_pending(request_id);
        let _ = sender.send(Err(ActivityResultError::Launch(description)));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
