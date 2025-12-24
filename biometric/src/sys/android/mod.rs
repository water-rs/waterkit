use crate::{BiometricError, BiometricType};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jlong};
use std::sync::{Mutex, OnceLock};

/// Embedded DEX bytecode.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Map to store callbacks: pointer -> Sender
/// Note: We cast the raw pointer of the Sender to pass to Java, and cast it back.
/// Using a map might be safer but passing pointer is standard FFI.
/// However, Box::into_raw gives a pointer.
///
/// Type of callback: tokio::sync::oneshot::Sender<Result<(), BiometricError>>
type BiometricSender = tokio::sync::oneshot::Sender<Result<(), BiometricError>>;

/// Initialize the DEX class loader. Must be called with a valid Context.
pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), BiometricError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Write DEX to cache directory (Copied from haptic crate logic)
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|e| BiometricError::PlatformError(format!("getCacheDir: {e}")))?
        .l()
        .map_err(|e| BiometricError::PlatformError(format!("getCacheDir res: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| BiometricError::PlatformError(format!("getAbsolutePath: {e}")))?
        .l()
        .map_err(|e| BiometricError::PlatformError(format!("getAbsolutePath res: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_biometric.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| BiometricError::PlatformError(format!("get_string: {e}")))?
            .to_str()
            .map_err(|e| BiometricError::PlatformError(format!("to_str: {e}")))?
    );

    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| BiometricError::PlatformError(format!("write DEX: {e}")))?;

    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| BiometricError::PlatformError(format!("new_string: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| BiometricError::PlatformError(format!("getClassLoader: {e}")))?
        .l()
        .map_err(|e| BiometricError::PlatformError(format!("getClassLoader res: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| BiometricError::PlatformError(format!("find DexClassLoader: {e}")))?;

    let class_loader = env
        .new_object(
            dex_class_loader_class,
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue::Object(&dex_path_jstring),
                JValue::Object(&cache_path),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|e| BiometricError::PlatformError(format!("new DexClassLoader: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| BiometricError::PlatformError(format!("new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);

    // Register native method
    // We need to register `onResult` in `waterkit.biometric.BiometricHelper`.
    // Since we load the class from our DexClassLoader, we must find it there and register natives.

    // However, standard JNI `RegisterNatives` might be tricky with custom ClassLoader if JNI expects the class to be reachable from system loader?
    // Actually, we can use `JNI_OnLoad` if we were a shared library, but we are statically linked usually?
    // Or we just export `Java_waterkit_biometric_BiometricHelper_onResult`.
    // BUT `BiometricHelper` is in a secondary DEX, so the runtime might not find the symbol automatically if the class is loaded dynamically?
    // Actually, since we load the class dynamically, we MUST manually register natives on the loaded class!

    register_natives(env)?;

    Ok(())
}

fn register_natives(env: &mut JNIEnv) -> Result<(), BiometricError> {
    let class = get_helper_class(env)?;
    let native_methods = [jni::NativeMethod {
        name: "onResult".into(),
        sig: "(JZLjava/lang/String;)V".into(),
        fn_ptr: Java_waterkit_biometric_BiometricHelper_onResult as *mut _,
    }];

    env.register_native_methods(class, &native_methods)
        .map_err(|e| BiometricError::PlatformError(format!("register_native_methods: {e}")))
}

fn get_helper_class<'a>(env: &'a mut JNIEnv) -> Result<JClass<'a>, BiometricError> {
    let class_loader = CLASS_LOADER.get().ok_or(BiometricError::PlatformError(
        "Class loader not initialized".into(),
    ))?;

    let helper_class_name = env
        .new_string("waterkit.biometric.BiometricHelper")
        .map_err(|e| BiometricError::PlatformError(format!("new_string: {e}")))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .map_err(|e| BiometricError::PlatformError(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| BiometricError::PlatformError(format!("loadClass res: {e}")))?;

    Ok(helper_class.into())
}

#[no_mangle]
pub unsafe extern "system" fn Java_waterkit_biometric_BiometricHelper_onResult(
    mut env: JNIEnv,
    _class: JClass,
    callback_ptr: jlong,
    success: jboolean,
    error_msg: JString,
) {
    let sender_ptr = callback_ptr as *mut BiometricSender;
    let sender = Box::from_raw(sender_ptr); // Reconstruct Box to take ownership and drop it

    if success != 0 {
        let _ = sender.send(Ok(()));
    } else {
        let error_str: String = env
            .get_string(&error_msg)
            .map(|s| s.into())
            .unwrap_or_else(|_| "Unknown JNI error".into());
        let _ = sender.send(Err(BiometricError::Failed(error_str)));
    }
}

pub async fn is_available() -> bool {
    // Stub: need context to check availability.
    // In waterkit, we usually don't have a global context available in pure Rust async functions
    // unless it was initialized.
    // For now, return false if we can't get check.
    // In real app usage, one should use `is_available_with_context`.
    false
}

pub async fn get_biometric_type() -> Option<BiometricType> {
    None
}

pub async fn authenticate(_reason: &str) -> Result<(), BiometricError> {
    Err(BiometricError::PlatformError(
        "Android requires authenticate_with_context".into(),
    ))
}

// Public API extending the standard one
pub fn authenticate_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    reason: &str,
) -> Result<tokio::sync::oneshot::Receiver<Result<(), BiometricError>>, BiometricError> {
    init_with_context(env, context)?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender_box = Box::new(tx);
    let sender_ptr = Box::into_raw(sender_box) as jlong;

    let reason_jstr = env
        .new_string(reason)
        .map_err(|e| BiometricError::PlatformError(format!("new_string: {e}")))?;

    let class = get_helper_class(env)?;
    env.call_static_method(
        class,
        "authenticate",
        "(Landroid/content/Context;Ljava/lang/String;J)V",
        &[
            JValue::Object(context),
            JValue::Object(&reason_jstr),
            JValue::Long(sender_ptr),
        ],
    )
    .map_err(|e| {
        // If fail, we must drop the box to avoid leak
        let _ = unsafe { Box::from_raw(sender_ptr as *mut BiometricSender) };
        BiometricError::PlatformError(format!("authenticate call: {e}"))
    })?;

    Ok(rx)
}
