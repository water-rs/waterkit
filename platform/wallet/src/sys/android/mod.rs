//! Android Google Wallet integration through the app-classpath Kotlin helper.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};

use futures::channel::oneshot;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong};
use jni::{jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, decode_optional_string, dex_helper, with_android_context,
};

use crate::{AddOutcome, GoogleWalletJwt, WalletCapabilities, WalletError};

static HELPER: DexHelper = dex_helper!("waterkit.wallet.WalletHelper");

struct Pending<T> {
    next: AtomicI64,
    senders: Mutex<HashMap<i64, oneshot::Sender<T>>>,
}

impl<T> Pending<T> {
    fn new() -> Self {
        Self {
            next: AtomicI64::new(1),
            senders: Mutex::new(HashMap::new()),
        }
    }

    fn register(&self) -> (i64, oneshot::Receiver<T>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        assert!(id > 0, "waterkit-wallet: pending request id overflowed");
        let (sender, receiver) = oneshot::channel();
        let previous = self
            .senders
            .lock()
            .unwrap_or_else(|error| {
                panic!("waterkit-wallet: pending request map poisoned: {error}")
            })
            .insert(id, sender);
        assert!(
            previous.is_none(),
            "waterkit-wallet: duplicate pending request id {id}"
        );
        (id, receiver)
    }

    fn complete(&self, id: i64, value: T) {
        let sender = self
            .senders
            .lock()
            .unwrap_or_else(|error| {
                panic!("waterkit-wallet: pending request map poisoned: {error}")
            })
            .remove(&id)
            .unwrap_or_else(|| panic!("waterkit-wallet: unknown pending request id {id}"));
        let _ = sender.send(value);
    }

    fn cancel(&self, id: i64) {
        self.senders
            .lock()
            .unwrap_or_else(|error| {
                panic!("waterkit-wallet: pending request map poisoned: {error}")
            })
            .remove(&id);
    }
}

#[derive(Debug)]
struct SaveResult {
    result_code: i32,
    message: Option<String>,
}

static AVAILABILITY: LazyLock<Pending<Result<bool, String>>> = LazyLock::new(Pending::new);
static SAVES: LazyLock<Pending<Result<SaveResult, String>>> = LazyLock::new(Pending::new);

impl From<AndroidError> for WalletError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

pub async fn capabilities() -> Result<WalletCapabilities, WalletError> {
    let receiver = with_android_context(|env, context| {
        let helper = HELPER.class(env, context)?;
        let (id, receiver) = AVAILABILITY.register();
        if let Err(error) = env.call_static_method(
            helper,
            jni_str!("requestAvailability"),
            jni_sig!("(Landroid/content/Context;J)V"),
            &[JValue::Object(context), JValue::Long(id)],
        ) {
            AVAILABILITY.cancel(id);
            return Err(WalletError::Platform(format!(
                "WalletHelper.requestAvailability failed: {error}"
            )));
        }
        Ok(receiver)
    })?;

    let available = receiver.await.map_err(|_| {
        WalletError::Platform("Android availability callback channel closed".into())
    })?;
    let available = available.map_err(|message| {
        WalletError::Platform(format!(
            "Google Wallet availability probe failed: {message}"
        ))
    })?;
    Ok(WalletCapabilities { available })
}

pub async fn add(passes: GoogleWalletJwt) -> Result<AddOutcome, WalletError> {
    let receiver = with_android_context(|env, context| {
        let helper = HELPER.class(env, context)?;
        let jwt = env.new_string(passes.as_str()).map_err(|error| {
            WalletError::Platform(format!("creating JWT string failed: {error}"))
        })?;
        let (id, receiver) = SAVES.register();
        let launch = env
            .call_static_method(
                helper,
                jni_str!("savePassesJwt"),
                jni_sig!("(Landroid/content/Context;JLjava/lang/String;)Z"),
                &[
                    JValue::Object(context),
                    JValue::Long(id),
                    JValue::Object(&jwt),
                ],
            )
            .map_err(|error| {
                SAVES.cancel(id);
                WalletError::Platform(format!("WalletHelper.savePassesJwt failed: {error}"))
            })?
            .z()
            .map_err(|error| {
                SAVES.cancel(id);
                WalletError::Platform(format!("savePassesJwt return conversion failed: {error}"))
            })?;
        if !launch {
            SAVES.cancel(id);
            return Err(WalletError::InProgress);
        }
        Ok(receiver)
    })?;

    let result = receiver
        .await
        .map_err(|_| WalletError::Platform("Android wallet result channel closed".into()))?
        .map_err(|message| {
            WalletError::Platform(format!(
                "launching the Google Wallet save flow failed: {message}"
            ))
        })?;
    match result.result_code {
        -1 => Ok(AddOutcome::Added),
        0 => Ok(AddOutcome::Cancelled),
        1 => Err(WalletError::Unavailable),
        2 => Err(WalletError::Platform(format!(
            "save error: {}",
            result.message.as_deref().unwrap_or("unknown error")
        ))),
        3 => Err(WalletError::Platform(format!(
            "internal error: {}",
            result.message.as_deref().unwrap_or("unknown error")
        ))),
        code => Err(WalletError::Platform(format!(
            "unexpected wallet result code {code}: {}",
            result.message.as_deref().unwrap_or("no error message")
        ))),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_wallet_WalletHelper_onAvailability<'local>(
    mut env: jni::EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    available: jboolean,
) {
    env.with_env(|_| -> jni::errors::Result<()> {
        AVAILABILITY.complete(request_id, Ok(available));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_wallet_WalletHelper_onAvailabilityError<'local>(
    mut env: jni::EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    message: JString<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let message = message.try_to_string(env)?;
        AVAILABILITY.complete(request_id, Err(message));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_wallet_WalletHelper_onSaveResult<'local>(
    mut env: jni::EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    result_code: jint,
    error_message: JObject<'local>,
) {
    env.with_env(|env| -> Result<(), AndroidError> {
        let message = decode_optional_string(env, &error_message)?;
        SAVES.complete(
            request_id,
            Ok(SaveResult {
                result_code,
                message,
            }),
        );
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_wallet_WalletHelper_onSaveFailed<'local>(
    mut env: jni::EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    message: JString<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let message = message.try_to_string(env)?;
        SAVES.complete(request_id, Err(message));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
