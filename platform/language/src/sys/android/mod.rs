//! Android `TranslationManager` implementation.

use futures::StreamExt;
use jni::{
    Env, jni_sig, jni_str,
    objects::{Global, JClass, JObject, JString, JValue},
};
use waterkit_build::{
    AndroidError, DexHelper, NativeCallback, NativeChannel, PeerError, decode_string,
    describe_jni_error, dex_helper, with_android_context,
};

use crate::{
    sys::wire,
    translation::{AssetStatus, LanguagePair, TranslationCapabilities, TranslationError},
};

static HELPER: DexHelper = dex_helper!("waterkit.language.TranslationHelper");

/// Cancels the `CancellationSignal` of an in-flight translation when its
/// caller is dropped before the result arrives. [`CallGuard::disarm`]
/// clears the signal once the helper has answered.
struct CallGuard {
    cancellation_signal: Option<Global<JObject<'static>>>,
}

impl CallGuard {
    const fn new() -> Self {
        Self {
            cancellation_signal: None,
        }
    }

    /// Records the call's `CancellationSignal`.
    fn arm(&mut self, signal: Global<JObject<'static>>) {
        self.cancellation_signal = Some(signal);
    }

    /// The helper answered; nothing left to cancel.
    fn disarm(&mut self) {
        self.cancellation_signal = None;
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        if let Some(cancellation_signal) = self.cancellation_signal.take() {
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

impl From<PeerError> for TranslationError {
    fn from(error: PeerError) -> Self {
        Self::Platform(error.to_string())
    }
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

/// A one-shot helper call: the `NativeCallback` handed to Kotlin plus the
/// receiver its answer lands on.
type HelperCall<T> = (
    NativeCallback<T>,
    futures::channel::oneshot::Receiver<Result<T, PeerError>>,
);

fn begin_json_call() -> Result<HelperCall<String>, TranslationError> {
    with_android_context(|env, _context| Ok(NativeCallback::<String>::new(env)?))
}

fn begin_translator_call() -> Result<HelperCall<Global<JObject<'static>>>, TranslationError> {
    with_android_context(|env, _context| Ok(NativeCallback::<Global<JObject<'static>>>::new(env)?))
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
    let (callback, receiver) = begin_json_call()?;
    let started: Result<(), TranslationError> = with_android_context(|env, context| {
        let helper = helper_class(env, context)?;
        env.call_static_method(
            helper,
            jni_str!("getCapabilities"),
            jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeCallback;)V"),
            &[JValue::Object(context), JValue::Object(callback.as_obj())],
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
        let (callback, receiver) = begin_translator_call()?;
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
                jni_sig!(
                    "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Lwaterkit/build/NativeCallback;)V"
                ),
                &[
                    JValue::Object(context),
                    JValue::Object(&source),
                    JValue::Object(&target),
                    JValue::Object(callback.as_obj()),
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
        let (callback, receiver) = begin_json_call()?;
        let mut call = CallGuard::new();
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
                    "(Landroid/view/translation/Translator;Lwaterkit/build/NativeCallback;Ljava/lang/String;)Landroid/os/CancellationSignal;"
                ),
                &[
                    JValue::Object(self.inner.as_obj()),
                    JValue::Object(callback.as_obj()),
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
        call.arm(cancellation_signal?);
        let result = receiver.await.map_err(|_| {
            TranslationError::Platform("Android translation callback was dropped".into())
        });
        call.disarm();
        let json = result??;
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

/// Starts listening for changes to Android translation capabilities. The
/// helper returns a `CapabilityUpdates` object owning the platform listener;
/// the returned handle drops it through `close()`.
pub fn capability_updates()
-> Result<crate::translation::android::CapabilityUpdates, TranslationError> {
    with_android_context(|env, context| {
        let (channel, receiver) = NativeChannel::<String>::new(env)?;
        let helper = helper_class(env, context)?;
        let registration = env
            .call_static_method(
                helper,
                jni_str!("registerCapabilityUpdates"),
                jni_sig!(
                    "(Landroid/content/Context;Lwaterkit/build/NativeChannel;)Lwaterkit/language/CapabilityUpdates;"
                ),
                &[JValue::Object(context), JValue::Object(channel.as_obj())],
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
        let registration = env
            .new_global_ref(registration)
            .map_err(|error| jni_error(env, "retain capability registration", error))?;

        let updates = receiver.map(|item| {
            item.map_err(TranslationError::from).and_then(|json| {
                wire::decode_capability_update(&json)
                    .map(|(pair, status)| CapabilityUpdate { pair, status })
            })
        });
        Ok(crate::translation::android::CapabilityUpdates::new(
            registration,
            updates,
        ))
    })
}

/// Ends a capability registration: removes the platform listener and closes
/// its channel.
pub fn remove_capability_listener(registration: &JObject<'_>) {
    if let Err(error) = with_android_context(|env, _context| {
        env.call_method(registration, jni_str!("close"), jni_sig!("()V"), &[])
            .map(|_| ())
            .map_err(|error| jni_error(env, "CapabilityUpdates.close", error))
    }) {
        tracing::error!(%error, "failed to remove Android translation capability listener");
    }
}
