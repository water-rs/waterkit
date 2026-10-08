//! Runtime half of the Android bridge.
//!
//! A crate's Kotlin helper ships on the application's classpath: the packager
//! compiles the crate's declared Kotlin sources into the app's DEX, and this
//! module resolves the helper class through the application `ClassLoader` at
//! run time.
//!
//! Only compiled when the crate is built *for* Android. Build scripts compile
//! for the host, so a `[build-dependencies]` copy of this crate never sees this
//! module - which is also why `swift-bridge-build` is not in an Android
//! application's dependency closure.

use jni::objects::{Global, JClass, JObject, JString, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::sync::OnceLock;

#[cfg(feature = "activity-result")]
mod activity_result;
#[cfg(feature = "native-callback")]
mod native_callback;

#[cfg(feature = "activity-result")]
pub use activity_result::{
    ActivityResult, ActivityResultError, PendingActivityResult, ResultCode,
    start_activity_for_result, start_intent_sender_for_result,
};
#[cfg(feature = "native-callback")]
pub use native_callback::{FromJava, NativeCallback, NativeChannel, PeerError};

/// Failure while bridging into the Android platform.
///
/// Capability crates convert this into their own `Platform` variant.
#[derive(Debug, thiserror::Error)]
#[error("Android JNI call failed: {}", summarize(.0))]
pub struct AndroidError(#[from] jni::errors::Error);

/// Renders a JNI error on one line; a caught Java exception is its class name
/// and message rather than the multi-line report with the stack trace.
fn summarize(error: &jni::errors::Error) -> String {
    match error {
        jni::errors::Error::CaughtJavaException { name, msg, .. } => format!("{name}: {msg}"),
        other => other.to_string(),
    }
}

/// Describes a failed JNI call, taking the Java exception it left pending.
///
/// `error` is what the call returned. When the call threw, the throwable is
/// still pending on the thread: it is caught and cleared, so the thread stays
/// usable for the next JNI call, and the description is its class name and
/// message. Otherwise `error` describes itself.
#[must_use]
pub fn describe_jni_error(env: &Env<'_>, error: jni::errors::Error) -> String {
    summarize(&take_pending_exception(env, error))
}

/// Replaces `error` with the Java exception pending on the thread, as a
/// [`jni::errors::Error::CaughtJavaException`], and clears it. Without a pending
/// exception, `error` is returned as is.
fn take_pending_exception(env: &Env<'_>, error: jni::errors::Error) -> jni::errors::Error {
    if !env.exception_check() {
        return error;
    }
    let caught = env
        .exception_catch()
        .expect_err("exception_check reported a pending throwable");
    // `exception_catch` clears the throwable before inspecting it, and the
    // inspection can throw in turn; leave nothing pending.
    env.exception_clear();
    caught
}

#[cfg(any(feature = "activity-result", feature = "native-callback"))]
fn android_error_with_pending_exception(env: &Env<'_>, error: jni::errors::Error) -> AndroidError {
    AndroidError::from(take_pending_exception(env, error))
}

/// Returns the application's JVM together with a global reference to its Android
/// `Context`, both published by `ndk_context`.
///
/// Use this when a handle has to outlive one call - a worker thread, or a struct
/// that keeps talking to the JVM. For a single call [`with_android_context`] is
/// simpler and does not allocate a global reference.
///
/// # Errors
///
/// Returns [`AndroidError`] if the global reference cannot be created.
///
/// # Panics
///
/// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet.
pub fn jvm_and_context() -> Result<(JavaVM, Global<JObject<'static>>), AndroidError> {
    let (vm, raw_context) = published_vm_and_context();
    let context = vm.attach_current_thread(|env| -> jni::errors::Result<_> {
        // SAFETY: `ndk_context` publishes a global reference to the application
        // `Context` that outlives this attachment, and `as_cast_raw` only
        // borrows it.
        let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
        env.new_global_ref(&*context)
    })?;
    Ok((vm, context))
}

/// Reads the process' `JavaVM` and Android `Context` out of `ndk_context`.
fn published_vm_and_context() -> (JavaVM, jni::sys::jobject) {
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    assert!(
        !raw_vm.is_null(),
        "waterkit: ndk_context returned a null JavaVM"
    );
    assert!(
        !raw_context.is_null(),
        "waterkit: ndk_context returned a null Android Context"
    );

    // SAFETY: `ndk_context` publishes the process' JavaVM pointer, which stays
    // valid for the lifetime of the application.
    (unsafe { JavaVM::from_raw(raw_vm) }, raw_context)
}

/// Runs `f` with the calling thread attached to the application's JVM, passing
/// it the application `Context` published by `ndk_context`.
///
/// # Errors
///
/// Returns whatever `f` returns, or an attachment failure converted through
/// [`AndroidError`].
///
/// # Panics
///
/// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet, which means
/// the caller ran before the application object was initialized.
pub fn with_android_context<T, E, F>(f: F) -> Result<T, E>
where
    F: FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, E>,
    E: From<AndroidError>,
{
    let (vm, raw_context) = published_vm_and_context();

    let attached = vm.attach_current_thread(|env| -> Result<Result<T, E>, AndroidError> {
        // SAFETY: `ndk_context` publishes a global reference to the application
        // `Context` that outlives this attachment, and `as_cast_raw` only
        // borrows it.
        let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
        let result = f(env, &context);
        if result.is_err() && env.exception_check() {
            // A Java exception throws by staying pending on the thread; a
            // failure converted into `E` must not leave it behind or the next
            // JNI call on this thread misreports.
            env.exception_clear();
        }
        Ok(result)
    });

    match attached {
        Ok(result) => result,
        Err(error) => Err(E::from(error)),
    }
}

/// A Kotlin helper class shipped with a crate on the application's classpath.
///
/// Declare one with [`dex_helper!`](crate::dex_helper) and reach for
/// [`DexHelper::class`] at each call site. The class is resolved through the
/// application `Context`'s `ClassLoader` - the same loader the app's own code
/// uses - and kept as a global reference, so every later call is a plain
/// lookup.
#[derive(Debug)]
pub struct DexHelper {
    class_name: &'static str,
    class: OnceLock<Global<JClass<'static>>>,
}

impl DexHelper {
    /// Declares the helper class a crate resolves at run time.
    ///
    /// Prefer [`dex_helper!`](crate::dex_helper).
    #[must_use]
    pub const fn new(class_name: &'static str) -> Self {
        Self {
            class_name,
            class: OnceLock::new(),
        }
    }

    /// Returns the helper class, loading it on first use.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidError`] if the class is not on the application's
    /// classpath.
    pub fn class(
        &self,
        env: &mut Env<'_>,
        context: &JObject<'_>,
    ) -> Result<&Global<JClass<'static>>, AndroidError> {
        if let Some(class) = self.class.get() {
            return Ok(class);
        }

        let class = self.load(env, context)?;
        Ok(self.class.get_or_init(|| class))
    }

    fn load(
        &self,
        env: &mut Env<'_>,
        context: &JObject<'_>,
    ) -> Result<Global<JClass<'static>>, AndroidError> {
        let loaded = (|| -> jni::errors::Result<Global<JClass<'static>>> {
            let class_loader = env
                .call_method(
                    context,
                    jni_str!("getClassLoader"),
                    jni_sig!("()Ljava/lang/ClassLoader;"),
                    &[],
                )?
                .l()?;

            let class_name = env.new_string(self.class_name)?;
            let class = env
                .call_method(
                    &class_loader,
                    jni_str!("loadClass"),
                    jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
                    &[JValue::Object(&class_name)],
                )?
                .l()?;
            let class = env.cast_local::<JClass>(class)?;
            env.new_global_ref(class)
        })();
        match loaded {
            Ok(class) => Ok(class),
            // `ClassLoader.loadClass` throws a `ClassNotFoundException` for an
            // un-staged helper, and the exception stays pending on the thread.
            // Take it off the thread with its class and message: the callers
            // map this to a Rust error, so the thread must stay usable for the
            // next JNI call and the error must say why.
            Err(error) => Err(take_pending_exception(env, error).into()),
        }
    }
}

/// Declares the [`DexHelper`] for a Kotlin helper class the crate ships on the
/// application's classpath.
///
/// The crate's manifest lists the helper's sources under
/// `[package.metadata.waterui.android]`; the packager compiles them into the
/// application's DEX, and this macro names the class to resolve.
///
/// ```ignore
/// use waterkit_build::{DexHelper, dex_helper};
///
/// static HELPER: DexHelper = dex_helper!("waterkit.haptic.HapticHelper");
/// ```
#[macro_export]
macro_rules! dex_helper {
    ($class_name:literal) => {
        $crate::DexHelper::new($class_name)
    };
}

/// Decodes a `java.lang.String` into a Rust `String`.
///
/// # Errors
///
/// Returns [`AndroidError`] if `value` is null or is not a string.
pub fn decode_string(env: &Env<'_>, value: &JObject<'_>) -> Result<String, AndroidError> {
    let text = env.as_cast::<JString>(value)?;
    Ok(text.try_to_string(env)?)
}

/// Decodes a nullable `java.lang.String` into a Rust `String`.
///
/// # Errors
///
/// Returns [`AndroidError`] if `value` is non-null and is not a string.
pub fn decode_optional_string(
    env: &Env<'_>,
    value: &JObject<'_>,
) -> Result<Option<String>, AndroidError> {
    if value.is_null() {
        return Ok(None);
    }
    decode_string(env, value).map(Some)
}
