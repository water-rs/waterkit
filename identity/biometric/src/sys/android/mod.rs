//! Android biometric authentication implementation using JNI.

use crate::{BiometricError, BiometricType};
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jlong};
use jni::{Env, EnvUnowned, JavaVM, NativeMethod, jni_sig, jni_str};
use std::sync::OnceLock;

/// Embedded DEX bytecode.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// `waterkit.biometric.BiometricHelper`, loaded once from [`DEX_BYTES`] with its
/// native method registered.
static HELPER_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Map to store callbacks: pointer -> Sender.
/// Note: We cast the raw pointer of the Sender to pass to Java, and cast it back.
/// Using a map might be safer but passing pointer is standard FFI.
/// However, `Box::into_raw` gives a pointer.
///
/// Type of callback: `tokio::sync::oneshot::Sender<Result<(), BiometricError>>`
type BiometricSender = tokio::sync::oneshot::Sender<Result<(), BiometricError>>;

fn with_android_context<T, F>(f: F) -> Result<T, BiometricError>
where
    F: FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, BiometricError>,
{
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    assert!(
        !raw_vm.is_null(),
        "waterkit-biometric: ndk_context returned a null JavaVM"
    );
    assert!(
        !raw_context.is_null(),
        "waterkit-biometric: ndk_context returned a null Android Context"
    );

    // SAFETY: `ndk_context` publishes the process' JavaVM pointer, which stays
    // valid for the lifetime of the application.
    let vm = unsafe { JavaVM::from_raw(raw_vm) };
    vm.attach_current_thread(
        |env| -> Result<Result<T, BiometricError>, jni::errors::Error> {
            // SAFETY: `ndk_context` publishes a global reference to the application
            // `Context` that outlives this attachment, and `as_cast_raw` only
            // borrows it.
            let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
            Ok(f(env, &context))
        },
    )
    .map_err(|e| BiometricError::Platform(format!("attach_current_thread: {e}")))?
}

/// Initialize Android biometric support by loading helper classes and JNI bindings.
///
/// # Errors
///
/// Returns [`BiometricError`] when JNI setup, DEX loading, or native registration fails.
pub fn init(env: &mut Env<'_>, context: &JObject<'_>) -> Result<(), BiometricError> {
    helper_class(env, context)?;
    Ok(())
}

/// Returns the cached helper class, loading the embedded DEX and registering its
/// native method on first use.
///
/// `BiometricHelper` lives in a secondary DEX loaded at runtime, so the JVM
/// cannot resolve `onResult` by symbol name - it has to be registered against
/// the loaded class explicitly.
fn helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<&'static Global<JClass<'static>>, BiometricError> {
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
) -> Result<Global<JClass<'static>>, BiometricError> {
    let parent_loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )
        .map_err(|e| BiometricError::Platform(format!("getClassLoader: {e}")))?
        .l()
        .map_err(|e| BiometricError::Platform(format!("getClassLoader res: {e}")))?;

    let dex_bytes = env
        .byte_array_from_slice(DEX_BYTES)
        .map_err(|e| BiometricError::Platform(format!("copy DEX: {e}")))?;
    let dex_buffer = env
        .call_static_method(
            jni_str!("java/nio/ByteBuffer"),
            jni_str!("wrap"),
            jni_sig!("([B)Ljava/nio/ByteBuffer;"),
            &[JValue::Object(&dex_bytes)],
        )
        .map_err(|e| BiometricError::Platform(format!("wrap DEX: {e}")))?
        .l()
        .map_err(|e| BiometricError::Platform(format!("wrap DEX res: {e}")))?;
    let class_loader = env
        .new_object(
            jni_str!("dalvik/system/InMemoryDexClassLoader"),
            jni_sig!("(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V"),
            &[JValue::Object(&dex_buffer), JValue::Object(&parent_loader)],
        )
        .map_err(|e| BiometricError::Platform(format!("new InMemoryDexClassLoader: {e}")))?;

    let class_name = env
        .new_string("waterkit.biometric.BiometricHelper")
        .map_err(|e| BiometricError::Platform(format!("new_string: {e}")))?;
    let class = env
        .call_method(
            &class_loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&class_name)],
        )
        .map_err(|e| BiometricError::Platform(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| BiometricError::Platform(format!("loadClass res: {e}")))?;
    let class = env
        .cast_local::<JClass>(class)
        .map_err(|e| BiometricError::Platform(format!("loadClass returned a non-class: {e}")))?;

    env.new_global_ref(class)
        .map_err(|e| BiometricError::Platform(format!("new_global_ref: {e}")))
}

fn register_natives(
    env: &mut Env<'_>,
    class: &Global<JClass<'static>>,
) -> Result<(), BiometricError> {
    // SAFETY: `onResult` is a static native method, so its Rust counterpart
    // takes `EnvUnowned` and `JClass` as its first two parameters, and the
    // remaining parameters match the descriptor below.
    let native_methods = [unsafe {
        NativeMethod::from_raw_parts(
            jni_str!("onResult"),
            jni_str!("(JZLjava/lang/String;)V"),
            Java_waterkit_biometric_BiometricHelper_onResult as *mut _,
        )
    }];

    // SAFETY: the descriptor above matches the exported function's signature.
    unsafe { env.register_native_methods(class, &native_methods) }
        .map_err(|e| BiometricError::Platform(format!("register_native_methods: {e}")))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_biometric_BiometricHelper_onResult<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    callback_ptr: jlong,
    success: jboolean,
    error_msg: JString<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let sender_ptr = callback_ptr as *mut BiometricSender;
        // SAFETY: `authenticate_with_context` leaked exactly one `Box` per call
        // and Java hands that same pointer back exactly once.
        let sender = unsafe { Box::from_raw(sender_ptr) };

        if success {
            let error = error_msg
                .try_to_string(env)
                .unwrap_or_else(|_| String::from("Unknown JNI error"));
            let _ = sender.send(Err(BiometricError::Failed(error)));
        } else {
            let _ = sender.send(Ok(()));
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[allow(clippy::unused_async)]
pub async fn is_available() -> bool {
    with_android_context(|env, context| {
        let class = helper_class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("isAvailable"),
            jni_sig!("(Landroid/content/Context;)Z"),
            &[JValue::Object(context)],
        )
        .map_err(|e| BiometricError::Platform(format!("isAvailable call: {e}")))?
        .z()
        .map_err(|e| BiometricError::Platform(format!("isAvailable result: {e}")))
    })
    .unwrap_or(false)
}

#[allow(clippy::unused_async)]
pub async fn get_biometric_type() -> Option<BiometricType> {
    with_android_context(|env, context| {
        let class = helper_class(env, context)?;
        let biometric_type = env
            .call_static_method(
                class,
                jni_str!("getBiometricType"),
                jni_sig!("(Landroid/content/Context;)I"),
                &[JValue::Object(context)],
            )
            .map_err(|e| BiometricError::Platform(format!("getBiometricType call: {e}")))?
            .i()
            .map_err(|e| BiometricError::Platform(format!("getBiometricType result: {e}")))?;

        Ok(match biometric_type {
            1 => Some(BiometricType::Fingerprint),
            2 => Some(BiometricType::Face),
            3 => Some(BiometricType::Iris),
            _ => None,
        })
    })
    .ok()
    .flatten()
}

#[allow(clippy::unused_async)]
pub async fn authenticate(reason: &str) -> Result<(), BiometricError> {
    let rx = with_android_context(|env, context| authenticate_with_context(env, context, reason))?;
    rx.await.unwrap_or_else(|_| {
        Err(BiometricError::Platform(
            "Biometric result channel closed".into(),
        ))
    })
}

/// Authenticate using an explicit Android `Context`.
///
/// Returns a oneshot receiver that resolves to the authentication outcome.
///
/// # Errors
///
/// Returns [`BiometricError`] when JNI calls fail or helper initialization cannot be completed.
pub fn authenticate_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    reason: &str,
) -> Result<tokio::sync::oneshot::Receiver<Result<(), BiometricError>>, BiometricError> {
    let class = helper_class(env, context)?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender_box = Box::new(tx);
    let sender_ptr = Box::into_raw(sender_box) as jlong;

    let reason_jstr = env
        .new_string(reason)
        .map_err(|e| BiometricError::Platform(format!("new_string: {e}")))?;

    env.call_static_method(
        class,
        jni_str!("authenticate"),
        jni_sig!("(Landroid/content/Context;Ljava/lang/String;J)V"),
        &[
            JValue::Object(context),
            JValue::Object(&reason_jstr),
            JValue::Long(sender_ptr),
        ],
    )
    .map_err(|e| {
        // If fail, we must drop the box to avoid leak
        // SAFETY: Java never saw the pointer, so this reclaims the only copy.
        let _ = unsafe { Box::from_raw(sender_ptr as *mut BiometricSender) };
        BiometricError::Platform(format!("authenticate call: {e}"))
    })?;

    Ok(rx)
}
