//! Android biometric authentication implementation using JNI.

use crate::{BiometricError, BiometricType};
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};
use waterkit_build::{AndroidError, DexHelper, dex_helper, with_android_context};

/// `waterkit.biometric.BiometricHelper`, compiled into the app's DEX by the
/// packager and resolved through the application's `ClassLoader`.
static HELPER: DexHelper = dex_helper!("waterkit.biometric.BiometricHelper");

impl From<AndroidError> for BiometricError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

/// Map to store callbacks: pointer -> Sender.
/// Note: We cast the raw pointer of the Sender to pass to Java, and cast it back.
/// Using a map might be safer but passing pointer is standard FFI.
/// However, `Box::into_raw` gives a pointer.
///
/// Type of callback: `tokio::sync::oneshot::Sender<Result<(), BiometricError>>`
type BiometricSender = tokio::sync::oneshot::Sender<Result<(), BiometricError>>;

/// Initialize Android biometric support by resolving the helper class.
///
/// # Errors
///
/// Returns [`BiometricError`] when JNI setup or helper class resolution fails.
pub fn init(env: &mut Env<'_>, context: &JObject<'_>) -> Result<(), BiometricError> {
    helper_class(env, context)?;
    Ok(())
}

/// Returns the helper class from the application's class loader.
///
/// The helper is on the application's classpath, so its `external fun`s bind
/// to the `Java_waterkit_biometric_*` exports by symbol name.
fn helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<&'static Global<JClass<'static>>, BiometricError> {
    Ok(HELPER.class(env, context)?)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_biometric_BiometricHelper_onResult<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    callback_ptr: jlong,
    success: jboolean,
    error_msg: JString<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let sender_ptr = callback_ptr as *mut BiometricSender;
        // SAFETY: `authenticate_with_context` leaked exactly one `Box` per call
        // and Java hands that same pointer back exactly once.
        let sender = unsafe { Box::from_raw(sender_ptr) };

        if success {
            let error = error_msg
                .try_to_string(env)
                .unwrap_or_else(|_| String::from("Unknown JNI error"));
            let _ = sender.send(Err(BiometricError::Failed(error)));
        } else {
            let _ = sender.send(Ok(()));
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[allow(clippy::unused_async)]
pub async fn is_available() -> bool {
    with_android_context(|env, context| -> Result<bool, BiometricError> {
        let class = helper_class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("isAvailable"),
            jni_sig!("(Landroid/content/Context;)Z"),
            &[JValue::Object(context)],
        )
        .map_err(|e| BiometricError::Platform(format!("isAvailable call: {e}")))?
        .z()
        .map_err(|e| BiometricError::Platform(format!("isAvailable result: {e}")))
    })
    .unwrap_or(false)
}

#[allow(clippy::unused_async)]
pub async fn get_biometric_type() -> Option<BiometricType> {
    with_android_context(
        |env, context| -> Result<Option<BiometricType>, BiometricError> {
            let class = helper_class(env, context)?;
            let biometric_type = env
                .call_static_method(
                    class,
                    jni_str!("getBiometricType"),
                    jni_sig!("(Landroid/content/Context;)I"),
                    &[JValue::Object(context)],
                )
                .map_err(|e| BiometricError::Platform(format!("getBiometricType call: {e}")))?
                .i()
                .map_err(|e| BiometricError::Platform(format!("getBiometricType result: {e}")))?;

            Ok(match biometric_type {
                1 => Some(BiometricType::Fingerprint),
                2 => Some(BiometricType::Face),
                3 => Some(BiometricType::Iris),
                _ => None,
            })
        },
    )
    .ok()
    .flatten()
}

#[allow(clippy::unused_async)]
pub async fn authenticate(reason: &str) -> Result<(), BiometricError> {
    let rx = with_android_context(|env, context| authenticate_with_context(env, context, reason))?;
    rx.await.unwrap_or_else(|_| {
        Err(BiometricError::Platform(
            "Biometric result channel closed".into(),
        ))
    })
}

/// Authenticate using an explicit Android `Context`.
///
/// Returns a oneshot receiver that resolves to the authentication outcome.
///
/// # Errors
///
/// Returns [`BiometricError`] when JNI calls fail or helper initialization cannot be completed.
pub fn authenticate_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    reason: &str,
) -> Result<tokio::sync::oneshot::Receiver<Result<(), BiometricError>>, BiometricError> {
    let class = helper_class(env, context)?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender_box = Box::new(tx);
    let sender_ptr = Box::into_raw(sender_box) as jlong;

    let reason_jstr = env
        .new_string(reason)
        .map_err(|e| BiometricError::Platform(format!("new_string: {e}")))?;

    env.call_static_method(
        class,
        jni_str!("authenticate"),
        jni_sig!("(Landroid/content/Context;Ljava/lang/String;J)V"),
        &[
            JValue::Object(context),
            JValue::Object(&reason_jstr),
            JValue::Long(sender_ptr),
        ],
    )
    .map_err(|e| {
        // If fail, we must drop the box to avoid leak
        // SAFETY: Java never saw the pointer, so this reclaims the only copy.
        let _ = unsafe { Box::from_raw(sender_ptr as *mut BiometricSender) };
        BiometricError::Platform(format!("authenticate call: {e}"))
    })?;

    Ok(rx)
}
