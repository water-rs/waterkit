//! Android activity-result contracts shared by `WaterKit` crates.
//!
//! Enable the `activity-result` feature on `waterkit-build`, build an Android
//! [`Intent`](https://developer.android.com/reference/android/content/Intent)
//! or `IntentSender` through JNI, then await the returned
//! [`PendingActivityResult`]. The helper uses the host
//! `ComponentActivity`'s `AndroidX` activity-result registry, so applications do
//! not need to forward request codes through `onActivityResult`.
//!
//! The launched request resolves through a
//! [`NativeCallback`](super::native_callback::NativeCallback) the Kotlin helper
//! receives as an argument: `complete` carries the
//! `androidx.activity.result.ActivityResult` object (or `null` when the host
//! activity was destroyed without one), and `fail` carries a launch failure.

use super::{
    AndroidError, FromJava, NativeCallback, PeerError, android_error_with_pending_exception,
};
use futures_channel::oneshot;
use jni::objects::{Global, JObject, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

static HELPER: super::DexHelper = super::DexHelper::new("waterkit.build.ActivityResultHelper");

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

/// What `NativeCallback.complete` carries for an activity launch:
/// `Some` for a delivered `androidx.activity.result.ActivityResult`, `None`
/// when the host activity was destroyed before the result arrived.
impl FromJava for Option<ActivityResult> {
    fn from_java(env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        if object.is_null() {
            return Ok(None);
        }
        let code = env
            .call_method(object, jni_str!("getResultCode"), jni_sig!("()I"), &[])
            .and_then(JValueOwned::i)?;
        let data = env
            .call_method(
                object,
                jni_str!("getData"),
                jni_sig!("()Landroid/content/Intent;"),
                &[],
            )
            .and_then(JValueOwned::l)?;
        let data = if data.is_null() {
            None
        } else {
            Some(env.new_global_ref(&data)?)
        };
        Ok(Some(ActivityResult {
            code: code.into(),
            data,
        }))
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

    /// The host activity was destroyed before the launched activity returned,
    /// or the request was otherwise collected unanswered.
    #[error("the host activity was destroyed before the launched activity returned its result")]
    ActivityDestroyed,

    /// Android could not launch the requested activity.
    #[error("could not launch: {0}")]
    Launch(String),

    /// A JNI operation failed.
    #[error(transparent)]
    Android(#[from] AndroidError),
}

/// A future for an Android activity result.
#[must_use]
#[derive(Debug)]
pub struct PendingActivityResult(oneshot::Receiver<Result<Option<ActivityResult>, PeerError>>);

impl Future for PendingActivityResult {
    type Output = Result<ActivityResult, ActivityResultError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.get_mut().0).poll(context) {
            Poll::Ready(Ok(Ok(Some(result)))) => Poll::Ready(Ok(result)),
            // `complete(null)` means the host activity was destroyed, and a
            // collected-unanswered peer cancels the oneshot the same way.
            Poll::Ready(Ok(Ok(None)) | Err(_)) => {
                Poll::Ready(Err(ActivityResultError::ActivityDestroyed))
            }
            Poll::Ready(Ok(Err(PeerError::Rejected(message)))) => {
                Poll::Ready(Err(ActivityResultError::Launch(message)))
            }
            Poll::Ready(Ok(Err(PeerError::Decode(error)))) => {
                Poll::Ready(Err(ActivityResultError::Android(error)))
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
    let (callback, receiver) =
        NativeCallback::<Option<ActivityResult>>::new(env).map_err(ActivityResultError::from)?;

    let launch_result = match kind {
        LaunchKind::Activity => env.call_static_method(
            helper_class,
            jni_str!("startActivityForResult"),
            jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeCallback;Landroid/content/Intent;)Z"),
            &[
                JValue::Object(context),
                JValue::Object(callback.as_obj()),
                JValue::Object(input),
            ],
        ),
        LaunchKind::IntentSender => env.call_static_method(
            helper_class,
            jni_str!("startIntentSenderForResult"),
            jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeCallback;Landroid/content/IntentSender;)Z"),
            &[
                JValue::Object(context),
                JValue::Object(callback.as_obj()),
                JValue::Object(input),
            ],
        ),
    };

    let launched = match launch_result {
        Ok(value) => value.z().map_err(AndroidError::from)?,
        Err(error) => {
            return Err(ActivityResultError::Android(
                android_error_with_pending_exception(env, error),
            ));
        }
    };

    if !launched {
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
    Ok(super::decode_string(env, &class_name)?)
}
