//! Runtime half of the Android bridge.
//!
//! [`build_kotlin`](crate::build_kotlin) compiles a crate's Kotlin helper into
//! `$OUT_DIR/classes.dex` at build time; this module loads that DEX back into
//! the JVM at run time and hands out the helper class. Both halves of that
//! contract live here so the `$OUT_DIR` layout stays an implementation detail
//! of this crate rather than something every capability crate has to restate.
//!
//! Only compiled when the crate is built *for* Android. Build scripts compile
//! for the host, so a `[build-dependencies]` copy of this crate never sees this
//! module - which is also why `swift-bridge-build` is not in an Android
//! application's dependency closure.

use jni::objects::{Global, JClass, JObject, JString, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::sync::OnceLock;

/// Failure while bridging into the Android platform.
///
/// Capability crates convert this into their own `Platform` variant.
#[derive(Debug, thiserror::Error)]
#[error("Android JNI call failed: {0}")]
pub struct AndroidError(#[from] jni::errors::Error);

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

/// Reads the process' JavaVM and Android `Context` out of `ndk_context`.
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
        Ok(f(env, &context))
    });

    match attached {
        Ok(result) => result,
        Err(error) => Err(E::from(error)),
    }
}

/// A Kotlin helper class shipped with a crate as an embedded DEX.
///
/// Declare one with [`dex_helper!`](crate::dex_helper) and reach for
/// [`DexHelper::class`] at each call site. On first use the DEX goes to an
/// `InMemoryDexClassLoader` - nothing is written to disk - and the loaded class
/// is kept as a global reference, which also keeps its defining loader alive, so
/// every later call is a plain lookup.
#[derive(Debug)]
pub struct DexHelper {
    dex: &'static [u8],
    class_name: &'static str,
    class: OnceLock<Global<JClass<'static>>>,
}

impl DexHelper {
    /// Declares the helper class a crate loads from its embedded DEX.
    ///
    /// Prefer [`dex_helper!`](crate::dex_helper), which fills in the DEX that
    /// [`build_kotlin`](crate::build_kotlin) produced.
    #[must_use]
    pub const fn new(dex: &'static [u8], class_name: &'static str) -> Self {
        Self {
            dex,
            class_name,
            class: OnceLock::new(),
        }
    }

    /// Returns the helper class, loading the embedded DEX on first use.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidError`] if the DEX cannot be loaded or does not contain
    /// the class.
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
        let parent_loader = env
            .call_method(
                context,
                jni_str!("getClassLoader"),
                jni_sig!("()Ljava/lang/ClassLoader;"),
                &[],
            )?
            .l()?;

        let dex_bytes = JObject::from(env.byte_array_from_slice(self.dex)?);
        let dex_buffer = env
            .call_static_method(
                jni_str!("java/nio/ByteBuffer"),
                jni_str!("wrap"),
                jni_sig!("([B)Ljava/nio/ByteBuffer;"),
                &[JValue::Object(&dex_bytes)],
            )?
            .l()?;
        let class_loader = env.new_object(
            jni_str!("dalvik/system/InMemoryDexClassLoader"),
            jni_sig!("(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V"),
            &[JValue::Object(&dex_buffer), JValue::Object(&parent_loader)],
        )?;

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
        Ok(env.new_global_ref(class)?)
    }
}

/// Declares the [`DexHelper`] for the DEX this crate's build script produced.
///
/// Pairs with [`build_kotlin`](crate::build_kotlin): the build script writes
/// `$OUT_DIR/classes.dex`, and this macro embeds exactly that file, so the path
/// convention never leaks into capability crates.
///
/// ```ignore
/// use waterkit_build::{DexHelper, dex_helper};
///
/// static HELPER: DexHelper = dex_helper!("waterkit.haptic.HapticHelper");
/// ```
#[macro_export]
macro_rules! dex_helper {
    ($class_name:literal) => {
        $crate::DexHelper::new(
            include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex")),
            $class_name,
        )
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
