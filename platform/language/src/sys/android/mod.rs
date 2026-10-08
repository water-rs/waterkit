//! Android `TranslationManager` implementation.

use std::{
    collections::HashMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicI64, Ordering},
    },
};

use futures::channel::oneshot;
use jni::{
    Env, EnvUnowned,
    errors::ThrowRuntimeExAndDefault,
    jni_sig, jni_str,
    objects::{Global, JClass, JObject, JString, JValue},
    sys::jlong,
};
use waterkit_build::{
    AndroidError, DexHelper, decode_string, describe_jni_error, dex_helper, with_android_context,
};

use crate::{
    sys::wire,
    translation::{AssetStatus, LanguagePair, TranslationCapabilities, TranslationError},
};

static HELPER: DexHelper = dex_helper!("waterkit.language.TranslationHelper");
static NEXT_CALL_ID: AtomicI64 = AtomicI64::new(1);
static PENDING_CALLS: OnceLock<Mutex<HashMap<i64, PendingCall>>> = OnceLock::new();

enum PendingCall {
    Json(oneshot::Sender<Result<String, TranslationError>>),
    Translator(oneshot::Sender<Result<Global<JObject<'static>>, TranslationError>>),
}

struct PendingCallGuard {
    id: i64,
    cancellation_signal: Option<Global<JObject<'static>>>,
}

impl PendingCallGuard {
    const fn new(id: i64) -> Self {
        Self {
            id,
            cancellation_signal: None,
        }
    }
}

impl Drop for PendingCallGuard {
    fn drop(&mut self) {
        if remove_pending_call(self.id).is_some()
            && let Some(cancellation_signal) = self.cancellation_signal.take()
        {
            let result: Result<(), TranslationError> = with_android_context(|env, _context| {
                env.call_method(
                    cancellation_signal.as_obj(),
                    jni_str!("cancel"),
                    jni_sig!("()V"),
                    &[],
                )
                .map_err(|error| jni_error(env, "android.os.CancellationSignal.cancel", error))?;
                Ok(())
            });
            if let Err(error) = result {
                tracing::error!(?error, "failed to cancel Android translation request");
            }
        }
    }
}

/// One change reported by Android's translation capability listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityUpdate {
    pair: LanguagePair,
    status: Option<AssetStatus>,
}

type CapabilityUpdateResult = Result<CapabilityUpdate, TranslationError>;
type CapabilityListener = async_channel::Sender<CapabilityUpdateResult>;
type CapabilityListeners = Mutex<HashMap<i64, CapabilityListener>>;

static CAPABILITY_LISTENERS: OnceLock<CapabilityListeners> = OnceLock::new();

impl CapabilityUpdate {
    /// The source and target languages that changed.
    #[must_use]
    pub const fn pair(&self) -> &LanguagePair {
        &self.pair
    }

    /// The new asset status, or `None` when the pair is no longer available.
    #[must_use]
    pub const fn status(&self) -> Option<AssetStatus> {
        self.status
    }
}

impl From<AndroidError> for TranslationError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

fn pending_calls() -> &'static Mutex<HashMap<i64, PendingCall>> {
    PENDING_CALLS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn capability_listeners() -> &'static CapabilityListeners {
    CAPABILITY_LISTENERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_call_id() -> i64 {
    NEXT_CALL_ID.fetch_add(1, Ordering::Relaxed)
}

fn helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<&'static Global<JClass<'static>>, TranslationError> {
    HELPER.class(env, context).map_err(Into::into)
}

fn jni_error(env: &Env<'_>, call: &str, error: jni::errors::Error) -> TranslationError {
    TranslationError::Platform(format!("{call}: {}", describe_jni_error(env, error)))
}

fn begin_json_call() -> (
    PendingCallGuard,
    oneshot::Receiver<Result<String, TranslationError>>,
) {
    let id = next_call_id();
    let (sender, receiver) = oneshot::channel();
    pending_calls()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id, PendingCall::Json(sender));
    (PendingCallGuard::new(id), receiver)
}

fn begin_translator_call() -> (
    PendingCallGuard,
    oneshot::Receiver<Result<Global<JObject<'static>>, TranslationError>>,
) {
    let id = next_call_id();
    let (sender, receiver) = oneshot::channel();
    pending_calls()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id, PendingCall::Translator(sender));
    (PendingCallGuard::new(id), receiver)
}

fn remove_pending_call(id: i64) -> Option<PendingCall> {
    pending_calls()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&id)
}

fn decode_callback_string(env: &Env<'_>, value: &JString<'_>) -> Result<String, TranslationError> {
    decode_string(env, value).map_err(Into::into)
}

fn check_api_level() -> Result<bool, TranslationError> {
    with_android_context(|env, context| {
        let helper = helper_class(env, context)?;
        env.call_static_method(helper, jni_str!("isApiSupported"), jni_sig!("()Z"), &[])
            .map_err(|error| jni_error(env, "TranslationHelper.isApiSupported", error))?
            .z()
            .map_err(|error| jni_error(env, "TranslationHelper.isApiSupported result", error))
    })
}

pub async fn capabilities() -> Result<TranslationCapabilities, TranslationError> {
    let (call, receiver) = begin_json_call();
    let started: Result<(), TranslationError> = with_android_context(|env, context| {
        let helper = helper_class(env, context)?;
        env.call_static_method(
            helper,
            jni_str!("getCapabilities"),
            jni_sig!("(Landroid/content/Context;J)V"),
            &[JValue::Object(context), JValue::Long(call.id)],
        )
        .map_err(|error| jni_error(env, "TranslationHelper.getCapabilities", error))?;
        Ok(())
    });
    started?;
    let json = receiver.await.map_err(|_| {
        TranslationError::Platform("Android capability callback was dropped".into())
    })??;
    wire::decode_capabilities(&json)
}

pub async fn pair_status(pair: &LanguagePair) -> Result<Option<AssetStatus>, TranslationError> {
    if !check_api_level()? {
        return Err(TranslationError::Unavailable);
    }
    Ok(capabilities().await?.status(pair))
}

#[derive(Debug)]
pub struct Translator {
    inner: Global<JObject<'static>>,
    pair: LanguagePair,
}

impl Translator {
    pub async fn create(pair: &LanguagePair) -> Result<Self, TranslationError> {
        let (call, receiver) = begin_translator_call();
        let started: Result<(), TranslationError> = with_android_context(|env, context| {
            let helper = helper_class(env, context)?;
            let source = env
                .new_string(pair.source().to_string())
                .map_err(|error| jni_error(env, "create source language tag", error))?;
            let target = env
                .new_string(pair.target().to_string())
                .map_err(|error| jni_error(env, "create target language tag", error))?;
            env.call_static_method(
                helper,
                jni_str!("createTranslator"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;J)V"),
                &[
                    JValue::Object(context),
                    JValue::Object(&source),
                    JValue::Object(&target),
                    JValue::Long(call.id),
                ],
            )
            .map_err(|error| jni_error(env, "TranslationHelper.createTranslator", error))?;
            Ok(())
        });
        started?;
        let inner = receiver.await.map_err(|_| {
            TranslationError::Platform("Android translator callback was dropped".into())
        })??;
        Ok(Self {
            inner,
            pair: pair.clone(),
        })
    }

    pub async fn translate(&self, texts: &[&str]) -> Result<Vec<String>, TranslationError> {
        let texts_json = wire::encode_texts(texts)?;
        let (mut call, receiver) = begin_json_call();
        let cancellation_signal = with_android_context(|env, context| {
            let helper = helper_class(env, context)?;
            let texts_json = env
                .new_string(texts_json)
                .map_err(|error| jni_error(env, "encode Android translation request", error))?;
            let cancellation_signal = env
                .call_static_method(
                helper,
                jni_str!("translate"),
                jni_sig!(
                    "(Landroid/view/translation/Translator;JLjava/lang/String;)Landroid/os/CancellationSignal;"
                ),
                &[
                    JValue::Object(self.inner.as_obj()),
                    JValue::Long(call.id),
                    JValue::Object(&texts_json),
                ],
            )
                .map_err(|error| jni_error(env, "TranslationHelper.translate", error))?
                .l()
                .map_err(|error| jni_error(env, "TranslationHelper.translate result", error))?;
            if cancellation_signal.is_null() {
                return Err(TranslationError::Platform(
                    "TranslationHelper.translate returned a null cancellation signal".into(),
                ));
            }
            env.new_global_ref(cancellation_signal).map_err(|error| {
                jni_error(env, "retain Android translation cancellation signal", error)
            })
        });
        call.cancellation_signal = Some(cancellation_signal?);
        let json = receiver.await.map_err(|_| {
            TranslationError::Platform("Android translation callback was dropped".into())
        })??;
        wire::decode_translations(&json, &self.pair, texts.len())
    }
}

impl Drop for Translator {
    fn drop(&mut self) {
        let result: Result<(), TranslationError> = with_android_context(|env, _context| {
            env.call_method(
                self.inner.as_obj(),
                jni_str!("destroy"),
                jni_sig!("()V"),
                &[],
            )
            .map_err(|error| {
                jni_error(env, "android.view.translation.Translator.destroy", error)
            })?;
            Ok(())
        });
        match result {
            Ok(()) => {}
            Err(error) => {
                tracing::error!(%error, "failed to destroy Android translation translator");
            }
        }
    }
}

/// Opens the system translation asset settings.
pub fn open_download_settings() -> Result<(), TranslationError> {
    let json = with_android_context(|env, context| {
        let helper = helper_class(env, context)?;
        let result = env
            .call_static_method(
                helper,
                jni_str!("openDownloadSettings"),
                jni_sig!("(Landroid/content/Context;)Ljava/lang/String;"),
                &[JValue::Object(context)],
            )
            .map_err(|error| jni_error(env, "TranslationHelper.openDownloadSettings", error))?
            .l()
            .map_err(|error| {
                jni_error(env, "TranslationHelper.openDownloadSettings result", error)
            })?;
        let result = env
            .as_cast::<JString>(&result)
            .map_err(|error| jni_error(env, "cast download settings response", error))?;
        decode_callback_string(env, &result)
    })?;
    wire::decode::<serde_json::Value>(&json, None).map(|_| ())
}

/// Starts listening for changes to Android translation capabilities.
pub fn capability_updates()
-> Result<crate::translation::android::CapabilityUpdates, TranslationError> {
    let id = next_call_id();
    let (sender, receiver) = async_channel::unbounded();
    capability_listeners()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id, sender);

    let result = with_android_context(|env, context| {
        let helper = helper_class(env, context)?;
        let result = env
            .call_static_method(
                helper,
                jni_str!("registerCapabilityUpdates"),
                jni_sig!("(Landroid/content/Context;J)Ljava/lang/String;"),
                &[JValue::Object(context), JValue::Long(id)],
            )
            .map_err(|error| jni_error(env, "TranslationHelper.registerCapabilityUpdates", error))?
            .l()
            .map_err(|error| {
                jni_error(
                    env,
                    "TranslationHelper.registerCapabilityUpdates result",
                    error,
                )
            })?;
        let result = env
            .as_cast::<JString>(&result)
            .map_err(|error| jni_error(env, "cast capability registration response", error))?;
        decode_callback_string(env, &result)
    });

    let json = match result {
        Ok(json) => json,
        Err(error) => {
            capability_listeners()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
            return Err(error);
        }
    };
    if let Err(error) = wire::decode::<serde_json::Value>(&json, None) {
        capability_listeners()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        return Err(error);
    }

    Ok(crate::translation::android::CapabilityUpdates::new(
        id, receiver,
    ))
}

pub fn remove_capability_listener(id: i64) {
    capability_listeners()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&id);
    let result = with_android_context(|env, context| {
        let helper = helper_class(env, context)?;
        let result = env
            .call_static_method(
                helper,
                jni_str!("removeCapabilityUpdates"),
                jni_sig!("(Landroid/content/Context;J)Ljava/lang/String;"),
                &[JValue::Object(context), JValue::Long(id)],
            )
            .map_err(|error| jni_error(env, "TranslationHelper.removeCapabilityUpdates", error))?
            .l()
            .map_err(|error| {
                jni_error(
                    env,
                    "TranslationHelper.removeCapabilityUpdates result",
                    error,
                )
            })?;
        let result = env
            .as_cast::<JString>(&result)
            .map_err(|error| jni_error(env, "cast capability removal response", error))?;
        decode_callback_string(env, &result)
    });
    match result {
        Ok(json) => {
            if let Err(error) = wire::decode::<serde_json::Value>(&json, None) {
                tracing::error!(%error, "failed to remove Android translation capability listener");
            }
        }
        Err(error) => {
            tracing::error!(%error, "failed to remove Android translation capability listener");
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_language_TranslationHelper_onResult<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    call_id: jlong,
    json: JString<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let result = decode_callback_string(env, &json);
        if let Some(PendingCall::Json(sender)) = remove_pending_call(call_id) {
            let _ = sender.send(result);
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_language_TranslationHelper_onTranslatorCreated<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    call_id: jlong,
    translator: JObject<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        if let Some(PendingCall::Translator(sender)) = remove_pending_call(call_id) {
            let result = if translator.is_null() {
                Err(TranslationError::Platform(
                    "system translation service returned a null translator".into(),
                ))
            } else {
                env.new_global_ref(translator)
                    .map_err(|error| jni_error(env, "create global translator reference", error))
            };
            let _ = sender.send(result);
        } else if !translator.is_null() {
            env.call_method(&translator, jni_str!("destroy"), jni_sig!("()V"), &[])?;
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_language_TranslationHelper_onTranslatorFailed<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    call_id: jlong,
    message: JString<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let message =
            decode_callback_string(env, &message).unwrap_or_else(|error| error.to_string());
        if let Some(PendingCall::Translator(sender)) = remove_pending_call(call_id) {
            let _ = sender.send(Err(TranslationError::Platform(message)));
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_language_TranslationHelper_onCapabilityUpdate<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    listener_id: jlong,
    json: JString<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let message = match decode_callback_string(env, &json) {
            Ok(json) => wire::decode_capability_update(&json)
                .map(|(pair, status)| CapabilityUpdate { pair, status }),
            Err(error) => Err(error),
        };
        let sender = capability_listeners()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&listener_id)
            .cloned();
        if let Some(sender) = sender {
            let _ = sender.try_send(message);
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
