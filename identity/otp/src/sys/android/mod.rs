//! Android OTP backend via the app-classpath Kotlin helper.

use futures::channel::oneshot;
use futures::{Stream, StreamExt};
use jni::objects::{Global, JByteArray, JClass, JObject, JString, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, FromJava, NativeChannel, decode_string, describe_jni_error,
    dex_helper, with_android_context,
};

use crate::{AddressedRealization, AppToken, OtpCapabilities, OtpError, Sender, derive_app_token};

use super::{Event, Request};

/// `waterkit.otp.OtpHelper`, compiled into the app's DEX by the packager and
/// resolved through the application's `ClassLoader`.
static HELPER: DexHelper = dex_helper!("waterkit.otp.OtpHelper");

/// The `OtpEvent` subclasses, resolved through the same application class
/// loader as `OtpHelper` so dispatch is by type, never by name.
static EVENT_STARTED: DexHelper = dex_helper!("waterkit.otp.OtpEvent$Started");
static EVENT_MESSAGE: DexHelper = dex_helper!("waterkit.otp.OtpEvent$Message");
static EVENT_TIMEOUT: DexHelper = dex_helper!("waterkit.otp.OtpEvent$Timeout");
static EVENT_DENIED: DexHelper = dex_helper!("waterkit.otp.OtpEvent$Denied");
static EVENT_FAILED: DexHelper = dex_helper!("waterkit.otp.OtpEvent$Failed");

/// `OtpHelper.capabilities` bit for Google Play services availability.
const CAPABILITY_PLAY_SERVICES: i32 = 1 << 0;
/// `OtpHelper.capabilities` bit for telephony messaging support.
const CAPABILITY_TELEPHONY_MESSAGING: i32 = 1 << 1;

impl From<AndroidError> for OtpError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

/// Decodes the Kotlin `OtpEvent` sealed-class instance the helper sends. The
/// event variant is the object's own type, so dispatch is `isInstance` checks
/// against the subclasses resolved through the application's class loader —
/// a renamed or obfuscated class can never silently misfire — and the payload
/// field (`text` / `error`) when the variant carries one.
impl FromJava for Event {
    fn from_java(env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        let (_vm, context) = waterkit_build::jvm_and_context()?;
        let context = context.as_obj();
        let started = EVENT_STARTED.class(env, context)?;
        let message = EVENT_MESSAGE.class(env, context)?;
        let timeout = EVENT_TIMEOUT.class(env, context)?;
        let denied = EVENT_DENIED.class(env, context)?;
        let failed = EVENT_FAILED.class(env, context)?;
        if env.is_instance_of(object, started)? {
            Ok(Self::Started)
        } else if env.is_instance_of(object, message)? {
            Ok(Self::Message(read_field(env, object, jni_str!("getText"))?))
        } else if env.is_instance_of(object, timeout)? {
            Ok(Self::Timeout)
        } else if env.is_instance_of(object, denied)? {
            Ok(Self::Denied)
        } else if env.is_instance_of(object, failed)? {
            Ok(Self::Failed(read_field(env, object, jni_str!("getError"))?))
        } else {
            Err(unexpected_event_class(env, object)?)
        }
    }
}

/// The `OtpEvent` dispatch hit no known subclass: throw
/// `IllegalArgumentException` naming the actual class, so the helper sees a
/// descriptive failure instead of a silent drop.
fn unexpected_event_class(
    env: &mut Env<'_>,
    object: &JObject<'_>,
) -> Result<AndroidError, AndroidError> {
    let class = env
        .call_method(
            object,
            jni_str!("getClass"),
            jni_sig!("()Ljava/lang/Class;"),
            &[],
        )
        .and_then(JValueOwned::l)?;
    let name = env
        .call_method(
            &class,
            jni_str!("getName"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .and_then(JValueOwned::l)
        .map(|name| decode_string(env, &name).unwrap_or_else(|_| "<unreadable class>".into()))?;
    let message = std::ffi::CString::new(format!("expected an OtpEvent subclass, got {name}"))
        .expect("a Java class name contains no NUL");
    let message = jni::strings::JNIStr::from_cstr(&message)
        .expect("a Java class name is valid modified UTF-8");
    env.throw_new(jni_str!("java/lang/IllegalArgumentException"), message)?;
    Ok(AndroidError::from(jni::errors::Error::JavaException))
}

/// Reads a `String`-typed getter off a Kotlin `OtpEvent` data class.
fn read_field(
    env: &mut Env<'_>,
    object: &JObject<'_>,
    getter: &'static jni::strings::JNIStr,
) -> Result<String, AndroidError> {
    let value = env
        .call_method(object, getter, jni_sig!("()Ljava/lang/String;"), &[])
        .and_then(JValueOwned::l)?;
    decode_string(env, &value)
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

/// A request whose events arrive through the helper's `NativeChannel`. Peer
/// rejections surface as `Event::Failed` with their message.
fn request() -> Result<(NativeChannel<Event>, impl Stream<Item = Event>), OtpError> {
    let (channel, receiver) =
        with_android_context(|env, _context| Ok::<_, OtpError>(NativeChannel::<Event>::new(env)?))?;
    Ok((
        channel,
        receiver.map(|item| item.unwrap_or_else(|error| Event::Failed(error.to_string()))),
    ))
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

/// Cancels the request behind `channel`: unregisters its Kotlin listener and
/// closes the stream.
pub fn cancel(channel: &JObject<'_>) -> Result<(), OtpError> {
    with_android_context(|env, context| {
        let class = helper_class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("cancel"),
            &jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeChannel;)V"),
            &[JValue::Object(context), JValue::Object(channel)],
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
    let (channel, events) = request()?;
    let mut request = Request::new(channel, events);

    let token = with_android_context(|env, context| -> Result<AppToken, OtpError> {
        let token = retriever_token(env, context)?;
        let class = helper_class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("startSmsRetriever"),
            &jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeChannel;)V"),
            &[
                JValue::Object(context),
                JValue::Object(request_channel(&request)),
            ],
        )
        .map_err(|error| jni_error(env, "OtpHelper.startSmsRetriever", error))?;
        Ok(token)
    })?;

    await_started(&mut request).await?;
    Ok((token, request))
}

/// The `NativeChannel` inside the request, for calls that hand it to Kotlin.
fn request_channel(request: &Request) -> &JObject<'_> {
    request.channel().as_obj()
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
    let (channel, events) = request()?;
    let request = Request::new(channel, events);

    let (sender, receiver) = oneshot::channel();
    std::thread::Builder::new()
        .name("waterkit-otp-token".into())
        .spawn(move || {
            let request = request;
            let result = with_android_context(|env, context| {
                let class = helper_class(env, context)?;
                let token = env
                    .call_static_method(
                        class,
                        jni_str!("createAppSpecificSmsToken"),
                        &jni_sig!(
                            "(Landroid/content/Context;Lwaterkit/build/NativeChannel;)Ljava/lang/String;"
                        ),
                        &[
                            JValue::Object(context),
                            JValue::Object(request_channel(&request)),
                        ],
                    )
                    .and_then(JValueOwned::l)
                    .map_err(|error| {
                        jni_error(env, "OtpHelper.createAppSpecificSmsToken", error)
                    })?;
                AppToken::new(decode_string(env, &token)?)
            });
            // The request (and its channel) travels back through the same
            // oneshot so the returned handle still owns them.
            let _ = sender.send((result, request));
        })
        .map_err(|error| {
            OtpError::Platform(format!("failed to start OTP token worker: {error}"))
        })?;
    let (result, request) = receiver
        .await
        .map_err(|_| OtpError::Platform("Android OTP token worker stopped".into()))?;
    let token = result?;

    // `createAppSpecificSmsToken` returns once the receiver and the token
    // exist, so the system is already listening here.
    Ok((token, request))
}

pub async fn start_consent(sender: Option<Sender>) -> Result<Request, OtpError> {
    if !capabilities()?.consent {
        return Err(OtpError::Unavailable);
    }

    let (channel, events) = request()?;
    let mut request = Request::new(channel, events);

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
            &jni_sig!(
                "(Landroid/content/Context;Ljava/lang/String;Lwaterkit/build/NativeChannel;)V"
            ),
            &[
                JValue::Object(context),
                JValue::Object(&sender),
                JValue::Object(request_channel(&request)),
            ],
        )
        .map(|_| ())
        .map_err(|error| jni_error(env, "OtpHelper.startSmsUserConsent", error))
    })?;

    await_started(&mut request).await?;
    Ok(request)
}
