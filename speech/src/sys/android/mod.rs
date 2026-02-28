use crate::{RecognitionConfig, RecognitionResult, SpeechError, TtsConfig, Voice};
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use std::collections::BTreeMap;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

type InitTx = tokio::sync::oneshot::Sender<bool>;

static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();
static CONTEXT: OnceLock<GlobalRef> = OnceLock::new();
static VM: OnceLock<Arc<JavaVM>> = OnceLock::new();
static CALLBACK_NATIVES_REGISTERED: OnceLock<()> = OnceLock::new();
static RECOGNITION_SESSIONS: OnceLock<
    Mutex<BTreeMap<i64, async_channel::Sender<RecognitionResult>>>,
> = OnceLock::new();
static NEXT_RECOGNITION_SESSION_ID: AtomicI64 = AtomicI64::new(1);

fn recognition_sessions() -> &'static Mutex<BTreeMap<i64, async_channel::Sender<RecognitionResult>>>
{
    RECOGNITION_SESSIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn ensure_runtime_initialized() -> Result<(), SpeechError> {
    if VM.get().is_some() && CONTEXT.get().is_some() {
        return Ok(());
    }

    let android_context = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android_context.vm().cast()) }
        .map_err(|e| SpeechError::PlatformError(format!("JavaVM::from_raw: {e}")))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| SpeechError::PlatformError(format!("attach_current_thread: {e}")))?;

    let context = ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
    assert!(
        !context.is_null(),
        "waterkit-speech: ndk_context returned null Android Context"
    );

    init_with_context(&mut env, &context)
}

fn get_vm() -> Result<Arc<JavaVM>, SpeechError> {
    if VM.get().is_none() {
        ensure_runtime_initialized()?;
    }

    VM.get().cloned().ok_or_else(|| {
        SpeechError::PlatformError(
            "Android speech runtime initialization failed: VM missing".into(),
        )
    })
}

fn ensure_context() -> Result<GlobalRef, SpeechError> {
    if CONTEXT.get().is_none() {
        ensure_runtime_initialized()?;
    }

    CONTEXT.get().cloned().ok_or_else(|| {
        SpeechError::PlatformError(
            "Android speech runtime initialization failed: Context missing".into(),
        )
    })
}

fn init_dex(env: &mut JNIEnv, context: &JObject) -> Result<(), SpeechError> {
    if CLASS_LOADER.get().is_some() {
        register_callback_natives(env)?;
        return Ok(());
    }

    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| SpeechError::PlatformError(format!("getCacheDir: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| SpeechError::PlatformError(format!("getAbsolutePath: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_speech.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| SpeechError::PlatformError(format!("get_string: {e}")))?
            .to_str()
            .map_err(|e| SpeechError::PlatformError(format!("to_str: {e}")))?
    );

    let _ = std::fs::remove_file(&dex_path);
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| SpeechError::PlatformError(format!("write DEX: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dex_path)
            .map_err(|e| SpeechError::PlatformError(format!("metadata DEX: {e}")))?
            .permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&dex_path, perms)
            .map_err(|e| SpeechError::PlatformError(format!("set_permissions DEX: {e}")))?;
    }

    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| SpeechError::PlatformError(format!("new_string: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| SpeechError::PlatformError(format!("getClassLoader: {e}")))?;

    let dex_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| SpeechError::PlatformError(format!("find DexClassLoader: {e}")))?;

    let loader = env
        .new_object(
            dex_class,
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue::Object(&dex_path_jstring),
                JValue::Object(&cache_path),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|e| SpeechError::PlatformError(format!("new DexClassLoader: {e}")))?;

    let global = env
        .new_global_ref(loader)
        .map_err(|e| SpeechError::PlatformError(format!("global_ref class_loader: {e}")))?;

    let _ = CLASS_LOADER.set(global);
    register_callback_natives(env)?;
    Ok(())
}

fn helper_class<'local>(
    env: &mut JNIEnv<'local>,
) -> Result<jni::objects::JClass<'local>, SpeechError> {
    let loader = CLASS_LOADER
        .get()
        .ok_or_else(|| SpeechError::PlatformError("class loader not initialized".into()))?;

    let helper_name = env
        .new_string("waterkit.speech.SpeechHelper")
        .map_err(|e| SpeechError::PlatformError(format!("new_string helper: {e}")))?;

    let cls_obj = env
        .call_method(
            loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_name)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| SpeechError::PlatformError(format!("loadClass SpeechHelper: {e}")))?;

    Ok(cls_obj.into())
}

fn callback_class<'local>(env: &mut JNIEnv<'local>) -> Result<JClass<'local>, SpeechError> {
    let loader = CLASS_LOADER
        .get()
        .ok_or_else(|| SpeechError::PlatformError("class loader not initialized".into()))?;

    let callback_name = env
        .new_string("waterkit.speech.SpeechInitCallback")
        .map_err(|e| SpeechError::PlatformError(format!("new_string callback: {e}")))?;

    let cls_obj = env
        .call_method(
            loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&callback_name)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| SpeechError::PlatformError(format!("loadClass SpeechInitCallback: {e}")))?;

    Ok(cls_obj.into())
}

fn register_callback_natives(env: &mut JNIEnv) -> Result<(), SpeechError> {
    if CALLBACK_NATIVES_REGISTERED.get().is_some() {
        return Ok(());
    }

    let callback = callback_class(env)?;
    let callback_natives = [jni::NativeMethod {
        name: "onTtsInit".into(),
        sig: "(Z)V".into(),
        fn_ptr: Java_waterkit_speech_SpeechInitCallback_onTtsInit as *mut _,
    }];
    env.register_native_methods(callback, &callback_natives)
        .map_err(|e| {
            SpeechError::PlatformError(format!(
                "register_native_methods SpeechInitCallback failed: {e}"
            ))
        })?;

    let helper = helper_class(env)?;
    let helper_natives = [
        jni::NativeMethod {
            name: "onRecognitionResult".into(),
            sig: "(JLjava/lang/String;ZF)V".into(),
            fn_ptr: Java_waterkit_speech_SpeechHelper_onRecognitionResult as *mut _,
        },
        jni::NativeMethod {
            name: "onRecognitionError".into(),
            sig: "(JI)V".into(),
            fn_ptr: Java_waterkit_speech_SpeechHelper_onRecognitionError as *mut _,
        },
    ];
    env.register_native_methods(helper, &helper_natives)
        .map_err(|e| {
            SpeechError::PlatformError(format!("register_native_methods SpeechHelper failed: {e}"))
        })?;

    let _ = CALLBACK_NATIVES_REGISTERED.set(());
    Ok(())
}

/// Initialize Android speech runtime with JNI environment and app context.
///
/// # Errors
///
/// Returns `SpeechError::PlatformError` if JVM/context caching, DEX loading,
/// or JNI native registration fails.
pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), SpeechError> {
    if VM.get().is_none() {
        let vm = env
            .get_java_vm()
            .map_err(|e| SpeechError::PlatformError(format!("get_java_vm: {e}")))?;
        let _ = VM.set(Arc::new(vm));
    }

    if CONTEXT.get().is_none() {
        let global = env
            .new_global_ref(context)
            .map_err(|e| SpeechError::PlatformError(format!("new_global_ref context: {e}")))?;
        let _ = CONTEXT.set(global);
    }

    init_dex(env, context)
}

#[derive(Debug)]
pub struct TtsInner {
    vm: Arc<JavaVM>,
    context: GlobalRef,
}

impl TtsInner {
    pub async fn new() -> Result<Self, SpeechError> {
        let context = ensure_context()?;
        let vm = get_vm()?;

        let rx = {
            let mut env = vm
                .attach_current_thread()
                .map_err(|e| SpeechError::PlatformError(format!("attach_current_thread: {e}")))?;

            init_dex(&mut env, context.as_obj())?;
            let helper = helper_class(&mut env)?;

            let (tx, rx) = tokio::sync::oneshot::channel::<bool>();

            let callback_cls = callback_class(&mut env)?;
            let callback = env
                .new_object(callback_cls, "()V", &[])
                .map_err(|e| SpeechError::PlatformError(format!("new SpeechInitCallback: {e}")))?;

            unsafe { env.set_rust_field(&callback, "waterkit_tts_init_tx", Some(tx)) }
                .map_err(|e| SpeechError::PlatformError(format!("set_rust_field callback: {e}")))?;

            env.call_static_method(
                helper,
                "initTts",
                "(Landroid/content/Context;Lwaterkit/speech/SpeechInitCallback;)V",
                &[JValue::Object(context.as_obj()), JValue::Object(&callback)],
            )
            .map_err(|e| SpeechError::PlatformError(format!("initTts: {e}")))?;
            rx
        };

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
        let mut env = self
            .vm
            .attach_current_thread()
            .map_err(|e| SpeechError::PlatformError(format!("attach_current_thread: {e}")))?;

        let helper = helper_class(&mut env)?;
        let voices = env
            .call_static_method(helper, "getAvailableVoices", "()[Ljava/lang/String;", &[])
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| SpeechError::PlatformError(format!("getAvailableVoices: {e}")))?;

        let array = jni::objects::JObjectArray::from(voices);
        let len_i32 = env
            .get_array_length(&array)
            .map_err(|e| SpeechError::PlatformError(format!("voices len: {e}")))?;
        let len = usize::try_from(len_i32)
            .map_err(|_| SpeechError::PlatformError(format!("negative voices len: {len_i32}")))?;

        let mut out = Vec::with_capacity(len);
        for idx in 0..len_i32 {
            let item = env
                .get_object_array_element(&array, idx)
                .map_err(|e| SpeechError::PlatformError(format!("voice[{idx}]: {e}")))?;
            let text: String = env
                .get_string(&JString::from(item))
                .map_err(|e| SpeechError::PlatformError(format!("voice str[{idx}]: {e}")))?
                .into();
            let mut parts = text.split('|');
            let id = parts.next().unwrap_or_default().to_string();
            let name = parts.next().unwrap_or_default().to_string();
            let language = parts.next().unwrap_or_default().to_string();
            out.push(Voice { id, name, language });
        }

        Ok(out)
    }

    pub async fn speak(&self, text: &str, config: &TtsConfig) -> Result<(), SpeechError> {
        futures::future::ready({
            let mut env = self
                .vm
                .attach_current_thread()
                .map_err(|e| SpeechError::PlatformError(format!("attach_current_thread: {e}")))?;

            let helper = helper_class(&mut env)?;
            let text_j = env
                .new_string(text)
                .map_err(|e| SpeechError::PlatformError(format!("new_string text: {e}")))?;
            let language_tag = config
                .voice
                .as_ref()
                .map_or("", |voice| voice.language.as_str());
            let language_j = env
                .new_string(language_tag)
                .map_err(|e| SpeechError::PlatformError(format!("new_string language: {e}")))?;

            env.call_static_method(
                helper,
                "speak",
                "(Ljava/lang/String;FFFLjava/lang/String;)V",
                &[
                    JValue::Object(&text_j),
                    JValue::Float(config.rate),
                    JValue::Float(config.pitch),
                    JValue::Float(config.volume),
                    JValue::Object(&language_j),
                ],
            )
            .map_err(|e| SpeechError::PlatformError(format!("speak: {e}")))?;

            Ok(())
        })
        .await
    }

    pub fn stop(&self) {
        let mut env = self.vm.attach_current_thread().unwrap_or_else(|e| {
            panic!("Android speech bridge invariant violated: attach_current_thread failed in TtsInner::stop: {e}")
        });
        let helper = helper_class(&mut env).unwrap_or_else(|e| {
            panic!("Android speech bridge invariant violated: helper_class failed in TtsInner::stop: {e}")
        });
        env.call_static_method(helper, "stop", "()V", &[])
            .unwrap_or_else(|e| {
                panic!(
                    "Android speech bridge invariant violated: SpeechHelper.stop failed in TtsInner::stop: {e}"
                )
            });
    }

    pub fn is_speaking(&self) -> bool {
        let mut env = self.vm.attach_current_thread().unwrap_or_else(|e| {
            panic!("Android speech bridge invariant violated: attach_current_thread failed in TtsInner::is_speaking: {e}")
        });
        let helper = helper_class(&mut env).unwrap_or_else(|e| {
            panic!("Android speech bridge invariant violated: helper_class failed in TtsInner::is_speaking: {e}")
        });
        let result = env
            .call_static_method(helper, "isSpeaking", "()Z", &[])
            .unwrap_or_else(|e| {
                panic!(
                    "Android speech bridge invariant violated: SpeechHelper.isSpeaking call failed in TtsInner::is_speaking: {e}"
                )
            });
        result.z().unwrap_or_else(|e| {
            panic!(
                "Android speech bridge invariant violated: SpeechHelper.isSpeaking return decode failed in TtsInner::is_speaking: {e}"
            )
        })
    }
}

impl Drop for TtsInner {
    fn drop(&mut self) {
        if let Ok(mut env) = self.vm.attach_current_thread()
            && let Ok(helper) = helper_class(&mut env)
        {
            let _ = env.call_static_method(helper, "shutdown", "()V", &[]);
        }
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
    let mut env = vm.attach_current_thread().unwrap_or_else(|error| {
        panic!(
            "waterkit-speech: attach_current_thread failed for recognition availability: {error}"
        )
    });
    init_dex(&mut env, context.as_obj()).unwrap_or_else(|error| {
        panic!("waterkit-speech: init_dex failed for recognition availability: {error}")
    });
    let helper = helper_class(&mut env).unwrap_or_else(|error| {
        panic!("waterkit-speech: failed to load SpeechHelper for recognition availability: {error}")
    });
    env.call_static_method(
        helper,
        "isRecognitionAvailable",
        "(Landroid/content/Context;)Z",
        &[JValue::Object(context.as_obj())],
    )
    .unwrap_or_else(|error| {
        panic!("waterkit-speech: SpeechHelper.isRecognitionAvailable call failed: {error}")
    })
    .z()
    .unwrap_or_else(|error| {
        panic!("waterkit-speech: SpeechHelper.isRecognitionAvailable result decode failed: {error}")
    })
}

#[derive(Debug)]
pub struct SpeechRecognizerInner {
    vm: Arc<JavaVM>,
    session_id: i64,
}

impl SpeechRecognizerInner {
    #[allow(clippy::unused_async)]
    pub async fn start(
        config: RecognitionConfig,
    ) -> Result<(Self, async_channel::Receiver<RecognitionResult>), SpeechError> {
        let context = ensure_context()?;
        let vm = get_vm()?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| SpeechError::PlatformError(format!("attach_current_thread: {e}")))?;

        init_dex(&mut env, context.as_obj())?;
        let helper = helper_class(&mut env)?;

        let (tx, rx) = async_channel::bounded(32);
        let session_id = NEXT_RECOGNITION_SESSION_ID.fetch_add(1, Ordering::Relaxed);
        if session_id <= 0 {
            return Err(SpeechError::PlatformError(format!(
                "invalid recognition session id generated: {session_id}"
            )));
        }

        {
            let mut sessions = recognition_sessions().lock().map_err(|e| {
                SpeechError::PlatformError(format!("recognition session map lock poisoned: {e}"))
            })?;
            sessions.insert(session_id, tx);
        }

        let language = config.language.unwrap_or_default();
        let language_j = env
            .new_string(language)
            .map_err(|e| SpeechError::PlatformError(format!("new_string language: {e}")))?;
        let started = env
            .call_static_method(
                helper,
                "startRecognition",
                "(Landroid/content/Context;Ljava/lang/String;ZJ)Z",
                &[
                    JValue::Object(context.as_obj()),
                    JValue::Object(&language_j),
                    JValue::Bool(u8::from(config.partial_results)),
                    JValue::Long(session_id),
                ],
            )
            .map_err(|e| SpeechError::PlatformError(format!("startRecognition: {e}")))?
            .z()
            .map_err(|e| SpeechError::PlatformError(format!("startRecognition result: {e}")))?;

        if !started {
            recognition_sessions()
                .lock()
                .map_err(|e| {
                    SpeechError::PlatformError(format!(
                        "recognition session map lock poisoned: {e}"
                    ))
                })?
                .remove(&session_id);
            return Err(SpeechError::NotAvailable);
        }

        drop(env);
        Ok((Self { vm, session_id }, rx))
    }

    pub fn stop(&self) {
        let mut env = self.vm.attach_current_thread().unwrap_or_else(|error| {
            panic!("waterkit-speech: attach_current_thread failed in SpeechRecognizerInner::stop: {error}")
        });
        let helper = helper_class(&mut env).unwrap_or_else(|error| {
            panic!(
                "waterkit-speech: failed to load SpeechHelper in SpeechRecognizerInner::stop: {error}"
            )
        });
        env.call_static_method(helper, "stopRecognition", "()V", &[])
            .unwrap_or_else(|error| {
                panic!(
                    "waterkit-speech: SpeechHelper.stopRecognition failed in SpeechRecognizerInner::stop: {error}"
                )
            });

        let mut sessions = recognition_sessions().lock().unwrap_or_else(|error| {
            panic!("waterkit-speech: recognition session map lock poisoned in stop: {error}")
        });
        sessions.remove(&self.session_id);
    }
}

impl Drop for SpeechRecognizerInner {
    fn drop(&mut self) {
        if let Ok(mut sessions) = recognition_sessions().lock() {
            sessions.remove(&self.session_id);
        }

        if let Ok(mut env) = self.vm.attach_current_thread()
            && let Ok(helper) = helper_class(&mut env)
        {
            let _ = env.call_static_method(helper, "stopRecognition", "()V", &[]);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_speech_SpeechInitCallback_onTtsInit(
    mut env: JNIEnv,
    callback: JObject,
    success: jni::sys::jboolean,
) {
    let tx = unsafe {
        env.take_rust_field::<_, _, Option<InitTx>>(&callback, "waterkit_tts_init_tx")
    }
    .unwrap_or_else(|error| {
        panic!("waterkit-speech: failed to extract init callback sender from Rust field: {error}")
    });

    if let Some(tx) = tx {
        let _ = tx.send(success != 0);
    } else {
        debug_assert!(
            false,
            "waterkit-speech: SpeechInitCallback invoked without stored sender"
        );
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_speech_SpeechHelper_onRecognitionResult(
    mut env: JNIEnv,
    _class: JClass,
    session_id: jni::sys::jlong,
    text: JString,
    is_final: jni::sys::jboolean,
    confidence: jni::sys::jfloat,
) {
    assert!(
        session_id > 0,
        "waterkit-speech: invalid recognition session id in onRecognitionResult: {session_id}"
    );

    let text: String = env
        .get_string(&text)
        .unwrap_or_else(|error| {
            panic!("waterkit-speech: failed to decode recognition text from JNI: {error}")
        })
        .into();
    let result = RecognitionResult {
        text,
        is_final: is_final != 0,
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

    if is_final != 0 {
        let mut sessions = recognition_sessions().lock().unwrap_or_else(|error| {
            panic!(
                "waterkit-speech: recognition session map lock poisoned on final result: {error}"
            )
        });
        sessions.remove(&session_id);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_speech_SpeechHelper_onRecognitionError(
    _env: JNIEnv,
    _class: JClass,
    session_id: jni::sys::jlong,
    error_code: jni::sys::jint,
) {
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
}
