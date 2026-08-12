//! Android passkey backend via dynamically loaded Kotlin helper.

use std::sync::OnceLock;

use async_trait::async_trait;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jlong};
use jni::{Env, EnvUnowned, NativeMethod, jni_sig, jni_str};

use crate::{
    AuthenticateOptions, AuthenticationResult, Availability, PasskeyError, RegisterOptions,
    RegistrationResult, authenticate_request_json, parse_authentication_response_json,
    parse_registration_response_json, register_request_json,
};

use super::PasskeyBackend;

const DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// `waterkit.passkey.PasskeyHelper`, loaded once from [`DEX_BYTES`] with its
/// native callbacks registered.
static HELPER_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

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

fn with_android_context<T, F>(f: F) -> Result<T, PasskeyError>
where
    F: FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, PasskeyError>,
{
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    assert!(
        !raw_vm.is_null(),
        "waterkit-passkey: ndk_context returned a null JavaVM"
    );
    assert!(
        !raw_context.is_null(),
        "waterkit-passkey: ndk_context returned a null Android Context"
    );

    // SAFETY: `ndk_context` publishes the process' JavaVM pointer, which stays
    // valid for the lifetime of the application.
    let vm = unsafe { jni::JavaVM::from_raw(raw_vm) };
    vm.attach_current_thread(
        |env| -> Result<Result<T, PasskeyError>, jni::errors::Error> {
            // SAFETY: `ndk_context` publishes a global reference to the application
            // `Context` that outlives this attachment, and `as_cast_raw` only
            // borrows it.
            let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
            Ok(f(env, &context))
        },
    )
    .map_err(|error| PasskeyError::Platform(format!("attach_current_thread failed: {error}")))?
}

/// Returns the cached helper class, loading the embedded DEX and registering its
/// native callbacks on first use.
fn helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<&'static Global<JClass<'static>>, PasskeyError> {
    if let Some(class) = HELPER_CLASS.get() {
        return Ok(class);
    }

    let class = load_helper_class(env, context)?;
    let class = HELPER_CLASS.get_or_init(|| class);
    register_natives(env, class)?;
    Ok(class)
}

fn load_helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Global<JClass<'static>>, PasskeyError> {
    let parent_loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )
        .map_err(|error| PasskeyError::Platform(format!("getClassLoader failed: {error}")))?
        .l()
        .map_err(|error| {
            PasskeyError::Platform(format!("getClassLoader result invalid: {error}"))
        })?;

    let dex_bytes = env
        .byte_array_from_slice(DEX_BYTES)
        .map_err(|error| PasskeyError::Platform(format!("copy DEX failed: {error}")))?;
    let dex_buffer = env
        .call_static_method(
            jni_str!("java/nio/ByteBuffer"),
            jni_str!("wrap"),
            jni_sig!("([B)Ljava/nio/ByteBuffer;"),
            &[JValue::Object(&dex_bytes)],
        )
        .map_err(|error| PasskeyError::Platform(format!("wrap DEX failed: {error}")))?
        .l()
        .map_err(|error| PasskeyError::Platform(format!("wrap DEX result invalid: {error}")))?;
    let loader = env
        .new_object(
            jni_str!("dalvik/system/InMemoryDexClassLoader"),
            jni_sig!("(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V"),
            &[JValue::Object(&dex_buffer), JValue::Object(&parent_loader)],
        )
        .map_err(|error| {
            PasskeyError::Platform(format!("new InMemoryDexClassLoader failed: {error}"))
        })?;

    let class_name = env
        .new_string("waterkit.passkey.PasskeyHelper")
        .map_err(|error| {
            PasskeyError::Platform(format!("new helper class string failed: {error}"))
        })?;
    let loaded_class = env
        .call_method(
            &loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&class_name)],
        )
        .map_err(|error| PasskeyError::Platform(format!("loadClass failed: {error}")))?
        .l()
        .map_err(|error| PasskeyError::Platform(format!("loadClass result invalid: {error}")))?;
    let loaded_class = env.cast_local::<JClass>(loaded_class).map_err(|error| {
        PasskeyError::Platform(format!("loadClass returned a non-class: {error}"))
    })?;

    env.new_global_ref(loaded_class)
        .map_err(|error| PasskeyError::Platform(format!("new_global_ref failed: {error}")))
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
