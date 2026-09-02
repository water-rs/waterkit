//! Android passkey backend via dynamically loaded Kotlin helper.

use std::sync::OnceLock;

use async_trait::async_trait;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jlong};
use jni::{Env, EnvUnowned, NativeMethod, jni_sig, jni_str};
use waterkit_build::{AndroidError, DexHelper, dex_helper, with_android_context};

use crate::{
    AuthenticateOptions, AuthenticationResult, Availability, PasskeyError, RegisterOptions,
    RegistrationResult, authenticate_request_json, parse_authentication_response_json,
    parse_registration_response_json, register_request_json,
};

use super::PasskeyBackend;

/// `waterkit.passkey.PasskeyHelper`, embedded as a DEX by this crate's build
/// script and loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.passkey.PasskeyHelper");

impl From<AndroidError> for PasskeyError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

type RegisterSender = tokio::sync::oneshot::Sender<Result<RegistrationResult, PasskeyError>>;
type AuthenticateSender = tokio::sync::oneshot::Sender<Result<AuthenticationResult, PasskeyError>>;

pub struct PlatformBackend;

#[async_trait]
impl PasskeyBackend for PlatformBackend {
    async fn is_available(&self) -> Result<Availability, PasskeyError> {
        let available = with_android_context(|env, context| {
            let helper_class = helper_class(env, context)?;
            env.call_static_method(
                helper_class,
                jni_str!("isAvailable"),
                jni_sig!("(Landroid/content/Context;)Z"),
                &[JValue::Object(context)],
            )
            .map_err(|error| PasskeyError::Platform(format!("isAvailable call failed: {error}")))?
            .z()
            .map_err(|error| {
                PasskeyError::Platform(format!("isAvailable return conversion failed: {error}"))
            })
        })
        .map_err(|error| {
            PasskeyError::Platform(format!("android availability check failed: {error}"))
        })?;

        if available {
            Ok(Availability::supported())
        } else {
            Ok(Availability::unavailable())
        }
    }

    async fn register(
        &self,
        options: &RegisterOptions,
    ) -> Result<RegistrationResult, PasskeyError> {
        let request_json = register_request_json(options)?;
        let rx = with_android_context(|env, context| {
            register_with_context(env, context, &request_json)
        })?;

        rx.await.unwrap_or_else(|_| {
            Err(PasskeyError::Platform(
                "android registration callback channel closed".into(),
            ))
        })
    }

    async fn authenticate(
        &self,
        options: &AuthenticateOptions,
    ) -> Result<AuthenticationResult, PasskeyError> {
        let request_json = authenticate_request_json(options)?;
        let rx = with_android_context(|env, context| {
            authenticate_with_context(env, context, &request_json)
        })?;

        rx.await.unwrap_or_else(|_| {
            Err(PasskeyError::Platform(
                "android authentication callback channel closed".into(),
            ))
        })
    }
}

/// Returns the helper class, registering its native callbacks on first use.
///
/// The helper lives in a DEX loaded at runtime, so the JVM cannot resolve its
/// native methods by symbol name - they have to be registered against the loaded
/// class explicitly. `RegisterNatives` just re-sets the same function pointers,
/// so a racing second registration is harmless.
fn helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<&'static Global<JClass<'static>>, PasskeyError> {
    static NATIVES_REGISTERED: OnceLock<()> = OnceLock::new();

    let class = HELPER.class(env, context)?;
    if NATIVES_REGISTERED.get().is_none() {
        register_natives(env, class)?;
        let _ = NATIVES_REGISTERED.set(());
    }
    Ok(class)
}

fn register_natives(
    env: &mut Env<'_>,
    helper_class: &Global<JClass<'static>>,
) -> Result<(), PasskeyError> {
    // SAFETY: both callbacks are static native methods, so their Rust
    // counterparts take `EnvUnowned` and `JClass` as the first two parameters,
    // and the remaining parameters match the descriptors below.
    let methods = unsafe {
        [
            NativeMethod::from_raw_parts(
                jni_str!("onRegisterResult"),
                jni_str!("(JZLjava/lang/String;Ljava/lang/String;)V"),
                Java_waterkit_passkey_PasskeyHelper_onRegisterResult as *mut _,
            ),
            NativeMethod::from_raw_parts(
                jni_str!("onAuthenticateResult"),
                jni_str!("(JZLjava/lang/String;Ljava/lang/String;)V"),
                Java_waterkit_passkey_PasskeyHelper_onAuthenticateResult as *mut _,
            ),
        ]
    };

    // SAFETY: the descriptors above match the exported functions' signatures.
    unsafe { env.register_native_methods(helper_class, &methods) }
        .map_err(|error| PasskeyError::Platform(format!("register_native_methods failed: {error}")))
}

fn register_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    request_json: &str,
) -> Result<tokio::sync::oneshot::Receiver<Result<RegistrationResult, PasskeyError>>, PasskeyError>
{
    let helper_class = helper_class(env, context)?;

    let request_json_java = env.new_string(request_json).map_err(|error| {
        PasskeyError::Platform(format!("new registration JSON string failed: {error}"))
    })?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender_ptr = Box::into_raw(Box::new(tx)) as jlong;

    if let Err(error) = env.call_static_method(
        helper_class,
        jni_str!("register"),
        jni_sig!("(Landroid/content/Context;Ljava/lang/String;J)V"),
        &[
            JValue::Object(context),
            JValue::Object(&request_json_java),
            JValue::Long(sender_ptr),
        ],
    ) {
        // SAFETY: Java never saw the pointer, so this reclaims the only copy.
        let _ = unsafe { Box::from_raw(sender_ptr as *mut RegisterSender) };
        return Err(PasskeyError::Platform(format!(
            "PasskeyHelper.register invocation failed: {error}"
        )));
    }

    Ok(rx)
}

fn authenticate_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    request_json: &str,
) -> Result<tokio::sync::oneshot::Receiver<Result<AuthenticationResult, PasskeyError>>, PasskeyError>
{
    let helper_class = helper_class(env, context)?;

    let request_json_java = env
        .new_string(request_json)
        .map_err(|error| PasskeyError::Platform(format!("new auth JSON string failed: {error}")))?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender_ptr = Box::into_raw(Box::new(tx)) as jlong;

    if let Err(error) = env.call_static_method(
        helper_class,
        jni_str!("authenticate"),
        jni_sig!("(Landroid/content/Context;Ljava/lang/String;J)V"),
        &[
            JValue::Object(context),
            JValue::Object(&request_json_java),
            JValue::Long(sender_ptr),
        ],
    ) {
        // SAFETY: Java never saw the pointer, so this reclaims the only copy.
        let _ = unsafe { Box::from_raw(sender_ptr as *mut AuthenticateSender) };
        return Err(PasskeyError::Platform(format!(
            "PasskeyHelper.authenticate invocation failed: {error}"
        )));
    }

    Ok(rx)
}

fn decode_optional_string(env: &Env<'_>, object: &JObject<'_>) -> Option<String> {
    if object.is_null() {
        return None;
    }

    env.as_cast::<JString>(object)
        .and_then(|text| text.try_to_string(env))
        .ok()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_passkey_PasskeyHelper_onRegisterResult<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    callback_ptr: jlong,
    success: jboolean,
    error_message: JObject<'local>,
    response_json: JObject<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        if callback_ptr == 0 {
            return Ok(());
        }

        // SAFETY: `register_with_context` leaked exactly one `Box` per call and
        // Java hands that same pointer back exactly once.
        let sender = unsafe { Box::from_raw(callback_ptr as *mut RegisterSender) };

        if !success {
            let error = decode_optional_string(env, &error_message).map_or_else(
                || {
                    PasskeyError::Platform(
                        "android registration failed without error message".into(),
                    )
                },
                PasskeyError::from_platform_error,
            );
            let _ = sender.send(Err(error));
            return Ok(());
        }

        let payload = decode_optional_string(env, &response_json).ok_or_else(|| {
            PasskeyError::Platform("android registration callback missing response payload".into())
        });
        let _ = sender.send(payload.and_then(|json| parse_registration_response_json(&json)));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_passkey_PasskeyHelper_onAuthenticateResult<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    callback_ptr: jlong,
    success: jboolean,
    error_message: JObject<'local>,
    response_json: JObject<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        if callback_ptr == 0 {
            return Ok(());
        }

        // SAFETY: `authenticate_with_context` leaked exactly one `Box` per call
        // and Java hands that same pointer back exactly once.
        let sender = unsafe { Box::from_raw(callback_ptr as *mut AuthenticateSender) };

        if !success {
            let error = decode_optional_string(env, &error_message).map_or_else(
                || {
                    PasskeyError::Platform(
                        "android authentication failed without error message".into(),
                    )
                },
                PasskeyError::from_platform_error,
            );
            let _ = sender.send(Err(error));
            return Ok(());
        }

        let payload = decode_optional_string(env, &response_json).ok_or_else(|| {
            PasskeyError::Platform(
                "android authentication callback missing response payload".into(),
            )
        });
        let _ = sender.send(payload.and_then(|json| parse_authentication_response_json(&json)));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
