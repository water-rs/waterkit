use crate::{RecognitionConfig, RecognitionResult, SpeechError, TtsConfig, Voice};
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JClass, JObject, JObjectArray, JString, JValue};
use jni::{Env, EnvUnowned, JavaVM, NativeMethod, jni_sig, jni_str};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use waterkit_build::{AndroidError, DexHelper, dex_helper, jvm_and_context};

impl From<AndroidError> for SpeechError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

type InitTx = tokio::sync::oneshot::Sender<bool>;

static HELPER: DexHelper = dex_helper!("waterkit.speech.SpeechHelper");
static CALLBACK: DexHelper = dex_helper!("waterkit.speech.SpeechInitCallback");
static NATIVES_REGISTERED: OnceLock<()> = OnceLock::new();
static CONTEXT: OnceLock<Global<JObject<'static>>> = OnceLock::new();
static VM: OnceLock<Arc<JavaVM>> = OnceLock::new();
static RECOGNITION_SESSIONS: OnceLock<
    Mutex<BTreeMap<i64, async_channel::Sender<RecognitionResult>>>,
> = OnceLock::new();
static NEXT_RECOGNITION_SESSION_ID: AtomicI64 = AtomicI64::new(1);

fn recognition_sessions() -> &'static Mutex<BTreeMap<i64, async_channel::Sender<RecognitionResult>>>
{
    RECOGNITION_SESSIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn ensure_runtime_initialized() -> Result<(), SpeechError> {
    if VM.get().is_some() && CONTEXT.get().is_some() && NATIVES_REGISTERED.get().is_some() {
        return Ok(());
    }
    let (vm, context) = jvm_and_context()?;
    vm.attach_current_thread(
        |env| -> Result<Result<(), SpeechError>, jni::errors::Error> {
            Ok(init_with_context(env, context.as_obj()))
        },
    )
    .map_err(|e| SpeechError::Platform(format!("attach_current_thread: {e}")))?
}

fn get_vm() -> Result<Arc<JavaVM>, SpeechError> {
    if VM.get().is_none() {
        ensure_runtime_initialized()?;
    }

    VM.get().cloned().ok_or_else(|| {
        SpeechError::Platform("Android speech runtime initialization failed: VM missing".into())
    })
}

fn ensure_context() -> Result<&'static Global<JObject<'static>>, SpeechError> {
    if CONTEXT.get().is_none() {
        ensure_runtime_initialized()?;
    }

    CONTEXT.get().ok_or_else(|| {
        SpeechError::Platform(
            "Android speech runtime initialization failed: Context missing".into(),
        )
    })
}

/// Resolves the helper classes on the application's classpath and registers
/// their native callbacks. Runs at most once - `SpeechHelper` and
/// `SpeechInitCallback` come from the same loader (the app's own), which is
/// what `initTts` requires of the callback type.
fn init_helpers(env: &mut Env<'_>, context: &JObject<'_>) -> Result<(), SpeechError> {
    if NATIVES_REGISTERED.get().is_some() {
        return Ok(());
    }
    register_callback_natives(env, context)?;
    let _ = NATIVES_REGISTERED.set(());
    Ok(())
}

fn helper_class(env: &mut Env<'_>) -> Result<&'static Global<JClass<'static>>, SpeechError> {
    let context = ensure_context()?;
    Ok(HELPER.class(env, context.as_obj())?)
}

fn register_callback_natives(env: &mut Env<'_>, context: &JObject<'_>) -> Result<(), SpeechError> {
    let callback = CALLBACK.class(env, context)?;
    let helper = HELPER.class(env, context)?;
    // SAFETY: `onTtsInit` is an instance native method, so its Rust counterpart
    // takes `EnvUnowned` and the receiver `JObject` as the first two parameters.
    let callback_natives = [unsafe {
        NativeMethod::from_raw_parts(
            jni_str!("onTtsInit"),
            jni_str!("(Z)V"),
            Java_waterkit_speech_SpeechInitCallback_onTtsInit as *mut _,
        )
    }];
    // SAFETY: the descriptor above matches the exported function's signature.
    unsafe { env.register_native_methods(callback, &callback_natives) }.map_err(|e| {
        SpeechError::Platform(format!(
            "register_native_methods SpeechInitCallback failed: {e}"
        ))
    })?;

    // SAFETY: both recognition callbacks are static native methods, so their
    // Rust counterparts take `EnvUnowned` and `JClass` as the first two
    // parameters.
    let helper_natives = unsafe {
        [
            NativeMethod::from_raw_parts(
                jni_str!("onRecognitionResult"),
                jni_str!("(JLjava/lang/String;ZF)V"),
                Java_waterkit_speech_SpeechHelper_onRecognitionResult as *mut _,
            ),
            NativeMethod::from_raw_parts(
                jni_str!("onRecognitionError"),
                jni_str!("(JI)V"),
                Java_waterkit_speech_SpeechHelper_onRecognitionError as *mut _,
            ),
        ]
    };
    // SAFETY: the descriptors above match the exported functions' signatures.
    unsafe { env.register_native_methods(helper, &helper_natives) }.map_err(|e| {
        SpeechError::Platform(format!("register_native_methods SpeechHelper failed: {e}"))
    })?;

    Ok(())
}

/// Initialize Android speech runtime with JNI environment and app context.
///
/// # Errors
///
/// Returns `SpeechError::Platform` if JVM/context caching, helper class
/// resolution or JNI native registration fails.
pub fn init_with_context(env: &mut Env<'_>, context: &JObject<'_>) -> Result<(), SpeechError> {
    if VM.get().is_none() {
        let vm = env
            .get_java_vm()
            .map_err(|e| SpeechError::Platform(format!("get_java_vm: {e}")))?;
        let _ = VM.set(Arc::new(vm));
    }

    if CONTEXT.get().is_none() {
        let global = env
            .new_global_ref(context)
            .map_err(|e| SpeechError::Platform(format!("new_global_ref context: {e}")))?;
        let _ = CONTEXT.set(global);
    }

    init_helpers(env, context)
}

#[derive(Debug)]
pub struct TtsInner {
    vm: Arc<JavaVM>,
    context: &'static Global<JObject<'static>>,
}

impl TtsInner {
    pub async fn new() -> Result<Self, SpeechError> {
        let context = ensure_context()?;
        let vm = get_vm()?;

        let rx = vm
            .attach_current_thread(
                |env| -> Result<
                    Result<tokio::sync::oneshot::Receiver<bool>, SpeechError>,
                    jni::errors::Error,
                > { Ok(start_tts_init(env, context)) },
            )
            .map_err(|e| SpeechError::Platform(format!("attach_current_thread: {e}")))??;

        let initialized = tokio::select! {
            result = rx => result.unwrap_or(false),
            () = futures_timer::Delay::new(std::time::Duration::from_secs(5)) => false,
        };

        if !initialized {
            return Err(SpeechError::NotAvailable);
        }

        Ok(Self { vm, context })
    }

    pub fn available_voices(&self) -> Result<Vec<Voice>, SpeechError> {
        self.vm
            .attach_current_thread(
                |env| -> Result<Result<Vec<Voice>, SpeechError>, jni::errors::Error> {
                    Ok(read_available_voices(env, self.context.as_obj()))
                },
            )
            .map_err(|e| SpeechError::Platform(format!("attach_current_thread: {e}")))?
    }

    pub async fn speak(&self, text: &str, config: &TtsConfig) -> Result<(), SpeechError> {
        futures::future::ready(
            self.vm
                .attach_current_thread(
                    |env| -> Result<Result<(), SpeechError>, jni::errors::Error> {
                        Ok(speak_with_env(env, self.context.as_obj(), text, config))
                    },
                )
                .map_err(|e| SpeechError::Platform(format!("attach_current_thread: {e}")))?,
        )
        .await
    }

    pub fn stop(&self) {
        self.vm
            .attach_current_thread(|env| -> jni::errors::Result<()> {
                let helper = HELPER.class(env, self.context.as_obj()).unwrap_or_else(|e| {
                    panic!("Android speech bridge invariant violated: HELPER.class failed in TtsInner::stop: {e}")
                });
                env.call_static_method(helper, jni_str!("stop"), jni_sig!("()V"), &[])
                    .unwrap_or_else(|e| {
                        panic!(
                            "Android speech bridge invariant violated: SpeechHelper.stop failed in TtsInner::stop: {e}"
                        )
                    });
                Ok(())
            })
            .unwrap_or_else(|e| {
                panic!("Android speech bridge invariant violated: attach_current_thread failed in TtsInner::stop: {e}")
            });
    }

    pub fn is_speaking(&self) -> bool {
        self.vm
            .attach_current_thread(|env| -> jni::errors::Result<bool> {
                let helper = HELPER.class(env, self.context.as_obj()).unwrap_or_else(|e| {
                    panic!("Android speech bridge invariant violated: HELPER.class failed in TtsInner::is_speaking: {e}")
                });
                let result = env
                    .call_static_method(helper, jni_str!("isSpeaking"), jni_sig!("()Z"), &[])
                    .unwrap_or_else(|e| {
                        panic!(
                            "Android speech bridge invariant violated: SpeechHelper.isSpeaking call failed in TtsInner::is_speaking: {e}"
                        )
                    });
                Ok(result.z().unwrap_or_else(|e| {
                    panic!(
                        "Android speech bridge invariant violated: SpeechHelper.isSpeaking return decode failed in TtsInner::is_speaking: {e}"
                    )
                }))
            })
            .unwrap_or_else(|e| {
                panic!("Android speech bridge invariant violated: attach_current_thread failed in TtsInner::is_speaking: {e}")
            })
    }
}

/// Constructs the init callback, stores the completion sender on it and asks the
/// helper to bring up the platform TTS engine.
fn start_tts_init(
    env: &mut Env<'_>,
    context: &Global<JObject<'static>>,
) -> Result<tokio::sync::oneshot::Receiver<bool>, SpeechError> {
    let helper = HELPER.class(env, context.as_obj())?;
    let callback_cls = CALLBACK.class(env, context.as_obj())?;

    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();

    let callback = env
        .new_object(callback_cls, jni_sig!("()V"), &[])
        .map_err(|e| SpeechError::Platform(format!("new SpeechInitCallback: {e}")))?;

    // SAFETY: the field is a `jlong` on a freshly created callback instance that
    // nothing else has a handle to yet.
    unsafe { env.set_rust_field(&callback, jni_str!("waterkit_tts_init_tx"), Some(tx)) }
        .map_err(|e| SpeechError::Platform(format!("set_rust_field callback: {e}")))?;

    env.call_static_method(
        helper,
        jni_str!("initTts"),
        jni_sig!("(Landroid/content/Context;Lwaterkit/speech/SpeechInitCallback;)V"),
        &[JValue::Object(context.as_obj()), JValue::Object(&callback)],
    )
    .map_err(|e| SpeechError::Platform(format!("initTts: {e}")))?;

    Ok(rx)
}

fn read_available_voices(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Vec<Voice>, SpeechError> {
    let helper = HELPER.class(env, context)?;
    let voices = env
        .call_static_method(
            helper,
            jni_str!("getAvailableVoices"),
            jni_sig!("()[Ljava/lang/String;"),
            &[],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|e| SpeechError::Platform(format!("getAvailableVoices: {e}")))?;

    let array = env
        .cast_local::<JObjectArray>(voices)
        .map_err(|e| SpeechError::Platform(format!("voices is not an array: {e}")))?;
    let len = array
        .len(env)
        .map_err(|e| SpeechError::Platform(format!("voices len: {e}")))?;

    let mut out = Vec::with_capacity(len);
    for idx in 0..len {
        let item = array
            .get_element(env, idx)
            .map_err(|e| SpeechError::Platform(format!("voice[{idx}]: {e}")))?;
        let text = env
            .as_cast::<JString>(&item)
            .and_then(|value| value.try_to_string(env))
            .map_err(|e| SpeechError::Platform(format!("voice str[{idx}]: {e}")))?;
        let mut parts = text.split('|');
        let id = parts.next().unwrap_or_default().to_string();
        let name = parts.next().unwrap_or_default().to_string();
        let language = parts.next().unwrap_or_default().to_string();
        out.push(Voice { id, name, language });
    }

    Ok(out)
}

fn speak_with_env(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    text: &str,
    config: &TtsConfig,
) -> Result<(), SpeechError> {
    let helper = HELPER.class(env, context)?;
    let text_j = env
        .new_string(text)
        .map_err(|e| SpeechError::Platform(format!("new_string text: {e}")))?;
    let language_tag = config
        .voice
        .as_ref()
        .map_or("", |voice| voice.language.as_str());
    let language_j = env
        .new_string(language_tag)
        .map_err(|e| SpeechError::Platform(format!("new_string language: {e}")))?;

    env.call_static_method(
        helper,
        jni_str!("speak"),
        jni_sig!("(Ljava/lang/String;FFFLjava/lang/String;)V"),
        &[
            JValue::Object(&text_j),
            JValue::Float(config.rate),
            JValue::Float(config.pitch),
            JValue::Float(config.volume),
            JValue::Object(&language_j),
        ],
    )
    .map_err(|e| SpeechError::Platform(format!("speak: {e}")))?;

    Ok(())
}

impl Drop for TtsInner {
    fn drop(&mut self) {
        let _ = self
            .vm
            .attach_current_thread(|env| -> jni::errors::Result<()> {
                if let Ok(helper) = HELPER.class(env, self.context.as_obj()) {
                    let _ =
                        env.call_static_method(helper, jni_str!("shutdown"), jni_sig!("()V"), &[]);
                }
                Ok(())
            });
        let _ = &self.context;
    }
}

pub fn recognition_is_available() -> bool {
    let context = ensure_context().unwrap_or_else(|error| {
        panic!("waterkit-speech: failed to obtain Android context for recognition availability: {error}")
    });
    let vm = get_vm().unwrap_or_else(|error| {
        panic!("waterkit-speech: failed to obtain JavaVM for recognition availability: {error}")
    });

    vm.attach_current_thread(|env| -> jni::errors::Result<bool> {
        let helper = HELPER.class(env, context.as_obj()).unwrap_or_else(|error| {
            panic!(
                "waterkit-speech: failed to load SpeechHelper for recognition availability: {error}"
            )
        });
        let available = env
            .call_static_method(
                helper,
                jni_str!("isRecognitionAvailable"),
                jni_sig!("(Landroid/content/Context;)Z"),
                &[JValue::Object(context.as_obj())],
            )
            .unwrap_or_else(|error| {
                panic!("waterkit-speech: SpeechHelper.isRecognitionAvailable call failed: {error}")
            })
            .z()
            .unwrap_or_else(|error| {
                panic!(
                    "waterkit-speech: SpeechHelper.isRecognitionAvailable result decode failed: {error}"
                )
            });
        Ok(available)
    })
    .unwrap_or_else(|error| {
        panic!(
            "waterkit-speech: attach_current_thread failed for recognition availability: {error}"
        )
    })
}

#[derive(Debug)]
pub struct SpeechRecognizerInner {
    vm: Arc<JavaVM>,
    session_id: i64,
}

impl SpeechRecognizerInner {
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    #[allow(clippy::unused_async)]
    pub async fn start(
        config: RecognitionConfig,
    ) -> Result<(Self, async_channel::Receiver<RecognitionResult>), SpeechError> {
        let context = ensure_context()?;
        let vm = get_vm()?;

        let (tx, rx) = async_channel::bounded(32);
        let session_id = NEXT_RECOGNITION_SESSION_ID.fetch_add(1, Ordering::Relaxed);
        if session_id <= 0 {
            return Err(SpeechError::Platform(format!(
                "invalid recognition session id generated: {session_id}"
            )));
        }

        {
            let mut sessions = recognition_sessions().lock().map_err(|e| {
                SpeechError::Platform(format!("recognition session map lock poisoned: {e}"))
            })?;
            sessions.insert(session_id, tx);
        }

        let language = config.language.unwrap_or_default();
        let started = vm
            .attach_current_thread(
                |env| -> Result<Result<bool, SpeechError>, jni::errors::Error> {
                    Ok(start_recognition(
                        env,
                        context,
                        &language,
                        config.partial_results,
                        session_id,
                    ))
                },
            )
            .map_err(|e| SpeechError::Platform(format!("attach_current_thread: {e}")))??;

        if !started {
            recognition_sessions()
                .lock()
                .map_err(|e| {
                    SpeechError::Platform(format!("recognition session map lock poisoned: {e}"))
                })?
                .remove(&session_id);
            return Err(SpeechError::NotAvailable);
        }

        Ok((Self { vm, session_id }, rx))
    }

    pub fn stop(&self) {
        self.vm
            .attach_current_thread(|env| -> jni::errors::Result<()> {
                let helper = helper_class(env).unwrap_or_else(|error| {
                    panic!(
                        "waterkit-speech: failed to load SpeechHelper in SpeechRecognizerInner::stop: {error}"
                    )
                });
                env.call_static_method(helper, jni_str!("stopRecognition"), jni_sig!("()V"), &[])
                    .unwrap_or_else(|error| {
                        panic!(
                            "waterkit-speech: SpeechHelper.stopRecognition failed in SpeechRecognizerInner::stop: {error}"
                        )
                    });
                Ok(())
            })
            .unwrap_or_else(|error| {
                panic!("waterkit-speech: attach_current_thread failed in SpeechRecognizerInner::stop: {error}")
            });

        let mut sessions = recognition_sessions().lock().unwrap_or_else(|error| {
            panic!("waterkit-speech: recognition session map lock poisoned in stop: {error}")
        });
        sessions.remove(&self.session_id);
    }
}

fn start_recognition(
    env: &mut Env<'_>,
    context: &Global<JObject<'static>>,
    language: &str,
    partial_results: bool,
    session_id: i64,
) -> Result<bool, SpeechError> {
    let helper = helper_class(env)?;
    let language_j = env
        .new_string(language)
        .map_err(|e| SpeechError::Platform(format!("new_string language: {e}")))?;

    env.call_static_method(
        helper,
        jni_str!("startRecognition"),
        jni_sig!("(Landroid/content/Context;Ljava/lang/String;ZJ)Z"),
        &[
            JValue::Object(context.as_obj()),
            JValue::Object(&language_j),
            JValue::Bool(partial_results),
            JValue::Long(session_id),
        ],
    )
    .map_err(|e| SpeechError::Platform(format!("startRecognition: {e}")))?
    .z()
    .map_err(|e| SpeechError::Platform(format!("startRecognition result: {e}")))
}

impl Drop for SpeechRecognizerInner {
    fn drop(&mut self) {
        if let Ok(mut sessions) = recognition_sessions().lock() {
            sessions.remove(&self.session_id);
        }

        let _ = self
            .vm
            .attach_current_thread(|env| -> jni::errors::Result<()> {
                if let Ok(helper) = helper_class(env) {
                    let _ = env.call_static_method(
                        helper,
                        jni_str!("stopRecognition"),
                        jni_sig!("()V"),
                        &[],
                    );
                }
                Ok(())
            });
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_speech_SpeechInitCallback_onTtsInit<'local>(
    mut env: EnvUnowned<'local>,
    callback: JObject<'local>,
    success: jni::sys::jboolean,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        // SAFETY: the field was populated by `start_tts_init` on this same
        // object and is taken exactly once.
        let tx = unsafe {
            env.take_rust_field::<_, _, Option<InitTx>>(&callback, jni_str!("waterkit_tts_init_tx"))
        }
        .unwrap_or_else(|error| {
            panic!(
                "waterkit-speech: failed to extract init callback sender from Rust field: {error}"
            )
        });

        if let Some(tx) = tx {
            let _ = tx.send(success);
        } else {
            debug_assert!(
                false,
                "waterkit-speech: SpeechInitCallback invoked without stored sender"
            );
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_speech_SpeechHelper_onRecognitionResult<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    session_id: jni::sys::jlong,
    text: JString<'local>,
    is_final: jni::sys::jboolean,
    confidence: jni::sys::jfloat,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        assert!(
            session_id > 0,
            "waterkit-speech: invalid recognition session id in onRecognitionResult: {session_id}"
        );

        let text = text.try_to_string(env).unwrap_or_else(|error| {
            panic!("waterkit-speech: failed to decode recognition text from JNI: {error}")
        });
        let result = RecognitionResult {
            text,
            is_final,
            confidence: (confidence >= 0.0).then_some(confidence),
        };

        let sender = {
            let sessions = recognition_sessions().lock().unwrap_or_else(|error| {
                panic!("waterkit-speech: recognition session map lock poisoned: {error}")
            });
            sessions.get(&session_id).cloned()
        };
        if let Some(sender) = sender {
            sender.try_send(result).unwrap_or_else(|error| {
                panic!("waterkit-speech: failed to send recognition result: {error}")
            });
        } else {
            debug_assert!(
                false,
                "waterkit-speech: received recognition result for unknown session: {session_id}"
            );
        }

        if is_final {
            let mut sessions = recognition_sessions().lock().unwrap_or_else(|error| {
                panic!(
                    "waterkit-speech: recognition session map lock poisoned on final result: {error}"
                )
            });
            sessions.remove(&session_id);
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_speech_SpeechHelper_onRecognitionError<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    session_id: jni::sys::jlong,
    error_code: jni::sys::jint,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        assert!(
            session_id > 0,
            "waterkit-speech: invalid recognition session id in onRecognitionError: {session_id}"
        );

        let removed = recognition_sessions()
            .lock()
            .unwrap_or_else(|error| {
                panic!(
                    "waterkit-speech: recognition session map lock poisoned on error callback: {error}"
                )
            })
            .remove(&session_id);
        if removed.is_none() {
            debug_assert!(
                false,
                "waterkit-speech: received recognition error for unknown session: id={session_id}, code={error_code}"
            );
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
