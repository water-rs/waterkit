//! Android Google Wallet integration through the app-classpath Kotlin helpers.
//!
//! The save flow runs in `SavePassesActivity`, a library-owned trampoline the
//! crate's manifest metadata declares: the Wallet API delivers its result to
//! that activity, which forwards it as its own result, so the shared
//! `waterkit-build` activity-result bridge that launched it receives the
//! result on the host `ComponentActivity`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};

use futures::channel::oneshot;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jlong};
use jni::{jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, ResultCode, decode_optional_string, dex_helper,
    start_activity_for_result, with_android_context,
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

static AVAILABILITY: LazyLock<Pending<Result<bool, String>>> = LazyLock::new(Pending::new);

/// Guards against overlapping save flows: Google Wallet resolves one save
/// request at a time.
static SAVE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The token that frees the save-flow slot when dropped.
struct SaveSlot;

impl SaveSlot {
    fn acquire() -> Result<Self, WalletError> {
        SAVE_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| WalletError::InProgress)?;
        Ok(Self)
    }
}

impl Drop for SaveSlot {
    fn drop(&mut self) {
        SAVE_ACTIVE.store(false, Ordering::SeqCst);
    }
}

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
    let _slot = SaveSlot::acquire()?;

    let pending = with_android_context(|env, context| {
        let helper = HELPER.class(env, context)?;
        let jwt = env.new_string(passes.as_str()).map_err(|error| {
            WalletError::Platform(format!("creating JWT string failed: {error}"))
        })?;
        let intent = env
            .call_static_method(
                helper,
                jni_str!("savePassesIntent"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;)Landroid/content/Intent;"),
                &[JValue::Object(context), JValue::Object(&jwt)],
            )
            .map_err(|error| {
                WalletError::Platform(format!("WalletHelper.savePassesIntent failed: {error}"))
            })?
            .l()
            .map_err(|error| {
                WalletError::Platform(format!("savePassesIntent conversion failed: {error}"))
            })?;
        start_activity_for_result(env, &intent).map_err(WalletError::from)
    })?;

    let result = pending.await?;
    let message = match result.data() {
        Some(data) => api_error_message(data)?,
        None => None,
    };
    save_outcome(raw_result_code(result.code()), message.as_deref())
}

/// Reads the save flow's `EXTRA_API_ERROR_MESSAGE` out of its result intent.
fn api_error_message(data: &Global<JObject<'static>>) -> Result<Option<String>, WalletError> {
    with_android_context(|env, context| {
        let helper = HELPER.class(env, context)?;
        let message = env
            .call_static_method(
                helper,
                jni_str!("apiErrorMessage"),
                jni_sig!("(Landroid/content/Intent;)Ljava/lang/String;"),
                &[JValue::Object(data.as_obj())],
            )
            .map_err(|error| {
                WalletError::Platform(format!("WalletHelper.apiErrorMessage failed: {error}"))
            })?
            .l()
            .map_err(|error| {
                WalletError::Platform(format!("apiErrorMessage conversion failed: {error}"))
            })?;
        Ok(decode_optional_string(env, &message)?)
    })
}

const fn raw_result_code(code: ResultCode) -> i32 {
    match code {
        ResultCode::Ok => -1,
        ResultCode::Canceled => 0,
        ResultCode::Custom(code) => code,
    }
}

fn save_outcome(result_code: i32, message: Option<&str>) -> Result<AddOutcome, WalletError> {
    match result_code {
        -1 => Ok(AddOutcome::Added),
        0 => Ok(AddOutcome::Cancelled),
        1 => Err(WalletError::Unavailable),
        2 => Err(WalletError::Platform(format!(
            "save error: {}",
            message.unwrap_or("unknown error")
        ))),
        3 => Err(WalletError::Platform(format!(
            "internal error: {}",
            message.unwrap_or("unknown error")
        ))),
        code => Err(WalletError::Platform(format!(
            "unexpected wallet result code {code}: {}",
            message.unwrap_or("no error message")
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
