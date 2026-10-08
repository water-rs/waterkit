//! Android OTP backend via the app-classpath Kotlin helper.

use futures::channel::{mpsc, oneshot};
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JByteArray, JClass, JObject, JString, JValue, JValueOwned};
use jni::sys::jlong;
use jni::{Env, EnvUnowned, jni_sig, jni_str};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use waterkit_build::{
    AndroidError, DexHelper, decode_string, describe_jni_error, dex_helper, with_android_context,
};

use crate::{AddressedRealization, AppToken, OtpCapabilities, OtpError, Sender, derive_app_token};

use super::{Event, Request};

/// `waterkit.otp.OtpHelper`, compiled into the app's DEX by the packager and
/// resolved through the application's `ClassLoader`.
static HELPER: DexHelper = dex_helper!("waterkit.otp.OtpHelper");

/// `OtpHelper.capabilities` bit for Google Play services availability.
const CAPABILITY_PLAY_SERVICES: i32 = 1 << 0;
/// `OtpHelper.capabilities` bit for telephony messaging support.
const CAPABILITY_TELEPHONY_MESSAGING: i32 = 1 << 1;

impl From<AndroidError> for OtpError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

/// The event senders of the requests the Kotlin helper can still call back.
///
/// Kotlin only ever carries the request id, so a dropped request simply loses
/// its registry entry and its late callbacks are ignored.
fn requests() -> &'static Mutex<HashMap<u64, mpsc::UnboundedSender<Event>>> {
    static REQUESTS: OnceLock<Mutex<HashMap<u64, mpsc::UnboundedSender<Event>>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn request_registry()
-> Result<MutexGuard<'static, HashMap<u64, mpsc::UnboundedSender<Event>>>, OtpError> {
    requests()
        .lock()
        .map_err(|_| OtpError::Platform("Android OTP request registry mutex poisoned".into()))
}

fn next_request_id() -> Result<u64, OtpError> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_ID
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| OtpError::Platform("Android OTP request id space exhausted".into()))
}

/// Delivers `event` to the request `id`, ignoring an unknown id.
fn dispatch(id: u64, event: Event) {
    let sender = match request_registry() {
        Ok(registry) => registry.get(&id).cloned(),
        Err(error) => {
            tracing::error!(%error, "failed to dispatch Android OTP callback");
            return;
        }
    };
    if let Some(sender) = sender {
        let _ = sender.unbounded_send(event);
    }
}

fn jni_error(env: &Env<'_>, call: &str, error: jni::errors::Error) -> OtpError {
    OtpError::Platform(format!("{call}: {}", describe_jni_error(env, error)))
}

fn helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<&'static Global<JClass<'static>>, OtpError> {
    Ok(HELPER.class(env, context)?)
}

fn capability_bits() -> Result<i32, OtpError> {
    with_android_context(|env, context| {
        let class = helper_class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("capabilities"),
            &jni_sig!("(Landroid/content/Context;)I"),
            &[JValue::Object(context)],
        )
        .and_then(JValueOwned::i)
        .map_err(|error| jni_error(env, "OtpHelper.capabilities", error))
    })
}

pub fn capabilities() -> Result<OtpCapabilities, OtpError> {
    let bits = capability_bits()?;
    let addressed = if bits & CAPABILITY_PLAY_SERVICES != 0 {
        Some(AddressedRealization::SmsRetriever)
    } else if bits & CAPABILITY_TELEPHONY_MESSAGING != 0 {
        Some(AddressedRealization::AppSpecificToken)
    } else {
        None
    };
    let consent = bits & CAPABILITY_PLAY_SERVICES != 0;

    Ok(OtpCapabilities {
        available: addressed.is_some() || consent,
        addressed,
        consent,
        one_time_code_autofill: false,
    })
}

/// Registers a request id and returns its handle, cancelling on failure.
fn register(id: u64) -> Result<Request, OtpError> {
    let (sender, receiver) = mpsc::unbounded();
    request_registry()?.insert(id, sender);
    Ok(Request::new(id, receiver))
}

pub fn cancel(id: u64) -> Result<(), OtpError> {
    request_registry()?.remove(&id);

    with_android_context(|env, context| {
        let class = helper_class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("cancel"),
            &jni_sig!("(Landroid/content/Context;J)V"),
            &[JValue::Object(context), JValue::Long(id.cast_signed())],
        )
        .map(|_| ())
        .map_err(|error| jni_error(env, "OtpHelper.cancel", error))
    })
}

/// Waits until the Kotlin helper reports that the system is listening.
async fn await_started(request: &mut Request) -> Result<(), OtpError> {
    match request.next_event().await {
        Some(Event::Started) => Ok(()),
        Some(Event::Timeout) => Err(OtpError::Timeout),
        Some(Event::Denied) => Err(OtpError::ConsentDenied),
        Some(Event::Failed(message)) => Err(OtpError::Platform(message)),
        Some(Event::Message(_)) | None => Err(OtpError::Platform(
            "Android OTP request did not report that it started listening".into(),
        )),
    }
}

pub async fn start_addressed() -> Result<(AppToken, Request), OtpError> {
    // The realization is decided from the capabilities before any listener is
    // installed; a failing realization is never retried with the other one.
    match capabilities()?.addressed {
        Some(AddressedRealization::SmsRetriever) => start_sms_retriever().await,
        Some(AddressedRealization::AppSpecificToken) => start_app_specific_token().await,
        None => Err(OtpError::Unavailable),
    }
}

async fn start_sms_retriever() -> Result<(AppToken, Request), OtpError> {
    let id = next_request_id()?;
    let mut request = register(id)?;

    let token = with_android_context(|env, context| -> Result<AppToken, OtpError> {
        let token = retriever_token(env, context)?;
        let class = helper_class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("startSmsRetriever"),
            &jni_sig!("(Landroid/content/Context;J)V"),
            &[JValue::Object(context), JValue::Long(id.cast_signed())],
        )
        .map_err(|error| jni_error(env, "OtpHelper.startSmsRetriever", error))?;
        Ok(token)
    })?;

    await_started(&mut request).await?;
    Ok((token, request))
}

/// Derives the 11-character SMS Retriever hash of the running application.
fn retriever_token(env: &mut Env<'_>, context: &JObject<'_>) -> Result<AppToken, OtpError> {
    let class = helper_class(env, context)?;
    let certificate = env
        .call_static_method(
            class,
            jni_str!("signingCertificate"),
            &jni_sig!("(Landroid/content/Context;)[B"),
            &[JValue::Object(context)],
        )
        .and_then(JValueOwned::l)
        .map_err(|error| jni_error(env, "OtpHelper.signingCertificate", error))?;
    let certificate = env
        .cast_local::<JByteArray>(certificate)
        .map_err(|error| jni_error(env, "cast signing certificate to byte[]", error))?;
    let certificate = env
        .convert_byte_array(&certificate)
        .map_err(|error| jni_error(env, "convert signing certificate", error))?;

    let package_name = env
        .call_method(
            context,
            jni_str!("getPackageName"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .and_then(JValueOwned::l)
        .map_err(|error| jni_error(env, "Context.getPackageName", error))?;
    let package_name = decode_string(env, &package_name)?;

    derive_app_token(&package_name, &certificate)
}

async fn start_app_specific_token() -> Result<(AppToken, Request), OtpError> {
    let id = next_request_id()?;
    let request = register(id)?;

    let (sender, receiver) = oneshot::channel();
    std::thread::Builder::new()
        .name("waterkit-otp-token".into())
        .spawn(move || {
            let result = with_android_context(|env, context| {
                let class = helper_class(env, context)?;
                let token = env
                    .call_static_method(
                        class,
                        jni_str!("createAppSpecificSmsToken"),
                        &jni_sig!("(Landroid/content/Context;J)Ljava/lang/String;"),
                        &[JValue::Object(context), JValue::Long(id.cast_signed())],
                    )
                    .and_then(JValueOwned::l)
                    .map_err(|error| {
                        jni_error(env, "OtpHelper.createAppSpecificSmsToken", error)
                    })?;
                AppToken::new(decode_string(env, &token)?)
            });
            let _ = sender.send(result);
        })
        .map_err(|error| {
            OtpError::Platform(format!("failed to start OTP token worker: {error}"))
        })?;
    let token = receiver
        .await
        .map_err(|_| OtpError::Platform("Android OTP token worker stopped".into()))??;

    // `createAppSpecificSmsToken` returns once the receiver and the token
    // exist, so the system is already listening here.
    Ok((token, request))
}

pub async fn start_consent(sender: Option<Sender>) -> Result<Request, OtpError> {
    if !capabilities()?.consent {
        return Err(OtpError::Unavailable);
    }

    let id = next_request_id()?;
    let mut request = register(id)?;

    with_android_context(|env, context| {
        let sender = match sender.as_ref() {
            Some(sender) => env
                .new_string(sender.as_str())
                .map_err(|error| jni_error(env, "new sender string", error))?,
            None => JString::null(),
        };
        let class = helper_class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("startSmsUserConsent"),
            &jni_sig!("(Landroid/content/Context;Ljava/lang/String;J)V"),
            &[
                JValue::Object(context),
                JValue::Object(&sender),
                JValue::Long(id.cast_signed()),
            ],
        )
        .map(|_| ())
        .map_err(|error| jni_error(env, "OtpHelper.startSmsUserConsent", error))
    })?;

    await_started(&mut request).await?;
    Ok(request)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_otp_OtpHelper_onStarted<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    id: jlong,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        dispatch(id.cast_unsigned(), Event::Started);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_otp_OtpHelper_onMessage<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    id: jlong,
    text: JString<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        match text.try_to_string(env) {
            Ok(text) => dispatch(id.cast_unsigned(), Event::Message(text)),
            Err(error) => dispatch(
                id.cast_unsigned(),
                Event::Failed(format!("SMS message decode failed: {error}")),
            ),
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_otp_OtpHelper_onTimeout<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    id: jlong,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        dispatch(id.cast_unsigned(), Event::Timeout);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_otp_OtpHelper_onDenied<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    id: jlong,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        dispatch(id.cast_unsigned(), Event::Denied);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_otp_OtpHelper_onFailed<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    id: jlong,
    message: JString<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        match message.try_to_string(env) {
            Ok(message) => dispatch(id.cast_unsigned(), Event::Failed(message)),
            Err(error) => dispatch(
                id.cast_unsigned(),
                Event::Failed(format!("SMS platform error decode failed: {error}")),
            ),
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
