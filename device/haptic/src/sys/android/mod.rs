//! Android haptic implementation using JNI.

use crate::{HapticError, HapticPattern, HapticStep, Intensity};
use jni::objects::{Global, JClass, JIntArray, JLongArray, JObject, JString, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::sync::OnceLock;

/// Embedded DEX bytecode containing the `HapticHelper` class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// `waterkit.haptic.HapticHelper`, loaded once from [`DEX_BYTES`].
///
/// A loaded class keeps its defining `DexClassLoader` alive, so caching the
/// class is enough to keep the whole DEX resident and lets every later call
/// skip both the file write and `ClassLoader.loadClass`.
static HELPER_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

impl From<jni::errors::Error> for HapticError {
    fn from(error: jni::errors::Error) -> Self {
        Self::Platform(error.to_string())
    }
}

/// Returns the cached `HapticHelper` class, loading the embedded DEX on first use.
fn helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<&'static Global<JClass<'static>>, HapticError> {
    if let Some(class) = HELPER_CLASS.get() {
        return Ok(class);
    }

    let class = load_helper_class(env, context)?;
    Ok(HELPER_CLASS.get_or_init(|| class))
}

/// Writes [`DEX_BYTES`] into the application cache directory and loads
/// `waterkit.haptic.HapticHelper` from it through a `DexClassLoader`.
fn load_helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Global<JClass<'static>>, HapticError> {
    let cache_dir = env
        .call_method(
            context,
            jni_str!("getCacheDir"),
            jni_sig!("()Ljava/io/File;"),
            &[],
        )
        .map_err(|e| HapticError::InitFailed(format!("Context.getCacheDir failed: {e}")))?
        .l()?;

    let cache_path = env
        .call_method(
            &cache_dir,
            jni_str!("getAbsolutePath"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .map_err(|e| HapticError::InitFailed(format!("File.getAbsolutePath failed: {e}")))?
        .l()?;

    let cache_path_string = env
        .as_cast::<JString>(&cache_path)
        .and_then(|path| path.try_to_string(env))
        .map_err(|e| HapticError::InitFailed(format!("decoding the cache path failed: {e}")))?;
    let dex_path = format!("{cache_path_string}/waterkit_haptic.dex");

    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| HapticError::InitFailed(format!("writing {dex_path} failed: {e}")))?;

    let dex_path_string = env.new_string(&dex_path)?;
    let parent_loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )
        .map_err(|e| HapticError::InitFailed(format!("Context.getClassLoader failed: {e}")))?
        .l()?;

    let class_loader = env
        .new_object(
            jni_str!("dalvik/system/DexClassLoader"),
            jni_sig!(
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V"
            ),
            &[
                JValue::Object(&dex_path_string),
                JValue::Object(&cache_path),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|e| HapticError::InitFailed(format!("constructing DexClassLoader failed: {e}")))?;

    let helper_name = env.new_string("waterkit.haptic.HapticHelper")?;
    let helper = env
        .call_method(
            &class_loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&helper_name)],
        )
        .map_err(|e| HapticError::InitFailed(format!("loading HapticHelper failed: {e}")))?
        .l()?;

    let helper = env.cast_local::<JClass>(helper)?;
    Ok(env.new_global_ref(helper)?)
}

fn duration_millis_i64(duration: std::time::Duration) -> Result<i64, HapticError> {
    i64::try_from(duration.as_millis()).map_err(|_| {
        HapticError::Platform(format!(
            "haptic duration exceeds i64 milliseconds: {duration:?}"
        ))
    })
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "Intensity is constrained to 0.0..=1.0 and Android amplitudes are integer values in 0..=255."
)]
fn amplitude_i32(intensity: Intensity) -> i32 {
    (intensity.value() * 255.0).round() as i32
}

/// Check if haptic feedback is available (requires context).
pub fn is_available_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<bool, HapticError> {
    let helper = helper_class(env, context)?;

    let available = env
        .call_static_method(
            helper,
            jni_str!("isAvailable"),
            jni_sig!("(Landroid/content/Context;)Z"),
            &[JValue::Object(context)],
        )
        .map_err(|e| HapticError::Platform(format!("isAvailable call failed: {e}")))?
        .z()?;

    Ok(available)
}

/// Trigger impact feedback with context.
pub fn impact_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    intensity: Intensity,
) -> Result<(), HapticError> {
    let helper = helper_class(env, context)?;

    env.call_static_method(
        helper,
        jni_str!("impact"),
        jni_sig!("(Landroid/content/Context;F)V"),
        &[JValue::Object(context), JValue::Float(intensity.value())],
    )
    .map_err(|e| HapticError::Platform(format!("impact call failed: {e}")))?;

    Ok(())
}

/// Trigger selection feedback with context.
pub fn selection_with_context(env: &mut Env<'_>, context: &JObject<'_>) -> Result<(), HapticError> {
    let helper = helper_class(env, context)?;

    env.call_static_method(
        helper,
        jni_str!("selection"),
        jni_sig!("(Landroid/content/Context;)V"),
        &[JValue::Object(context)],
    )
    .map_err(|e| HapticError::Platform(format!("selection call failed: {e}")))?;

    Ok(())
}

/// Trigger notification feedback with context.
pub fn notification_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    notification_type: i32,
) -> Result<(), HapticError> {
    let helper = helper_class(env, context)?;

    env.call_static_method(
        helper,
        jni_str!("notification"),
        jni_sig!("(Landroid/content/Context;I)V"),
        &[JValue::Object(context), JValue::Int(notification_type)],
    )
    .map_err(|e| HapticError::Platform(format!("notification call failed: {e}")))?;

    Ok(())
}

/// Play a custom haptic pattern with context.
pub fn play_pattern_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    pattern: &HapticPattern,
) -> Result<(), HapticError> {
    // Convert pattern to timings and amplitudes arrays
    let mut timings = Vec::with_capacity(pattern.steps().len());
    let mut amplitudes = Vec::with_capacity(pattern.steps().len());

    for step in pattern.steps() {
        match step {
            HapticStep::Vibrate {
                duration,
                intensity,
            } => {
                timings.push(duration_millis_i64(*duration)?);
                amplitudes.push(amplitude_i32(*intensity));
            }
            HapticStep::Pause(duration) => {
                timings.push(duration_millis_i64(*duration)?);
                amplitudes.push(0);
            }
        }
    }

    let helper = helper_class(env, context)?;

    let timings_array = JLongArray::new(env, timings.len())?;
    timings_array.set_region(env, 0, &timings)?;

    let amplitudes_array = JIntArray::new(env, amplitudes.len())?;
    amplitudes_array.set_region(env, 0, &amplitudes)?;

    let played = env
        .call_static_method(
            helper,
            jni_str!("playPattern"),
            jni_sig!("(Landroid/content/Context;[J[I)Z"),
            &[
                JValue::Object(context),
                JValue::Object(&timings_array),
                JValue::Object(&amplitudes_array),
            ],
        )
        .map_err(|e| HapticError::Platform(format!("playPattern call failed: {e}")))?
        .z()?;

    if played {
        Ok(())
    } else {
        Err(HapticError::Platform("pattern playback failed".into()))
    }
}

/// Runs `f` against the current thread's JNI environment and the application
/// `Context` published by `ndk_context`.
fn with_android_context<T, F>(f: F) -> Result<T, HapticError>
where
    F: for<'local> FnOnce(&mut Env<'local>, &JObject<'local>) -> Result<T, HapticError>,
{
    let android_context = ndk_context::android_context();
    // SAFETY: `ndk_context` publishes the process' JavaVM pointer, which stays
    // valid for the lifetime of the application.
    let vm = unsafe { JavaVM::from_raw(android_context.vm().cast()) };

    vm.attach_current_thread(|env| {
        // SAFETY: `ndk_context` publishes a global reference to the application
        // `Context` that outlives this attachment. `JObject` only borrows it -
        // it never deletes the reference on drop.
        let context = unsafe { JObject::from_raw(env, android_context.context().cast()) };
        assert!(
            !context.is_null(),
            "waterkit-haptic: ndk_context returned null Android Context"
        );

        f(env, &context)
    })
}

pub fn is_available() -> bool {
    with_android_context(is_available_with_context).unwrap_or_else(|error| {
        panic!("waterkit-haptic: failed to query availability with Android context: {error}")
    })
}

pub fn impact(intensity: Intensity) -> Result<(), HapticError> {
    with_android_context(|env, context| impact_with_context(env, context, intensity))
}

pub fn selection() -> Result<(), HapticError> {
    with_android_context(selection_with_context)
}

pub fn notification_success() -> Result<(), HapticError> {
    with_android_context(|env, context| notification_with_context(env, context, 0))
}

pub fn notification_warning() -> Result<(), HapticError> {
    with_android_context(|env, context| notification_with_context(env, context, 1))
}

pub fn notification_error() -> Result<(), HapticError> {
    with_android_context(|env, context| notification_with_context(env, context, 2))
}

pub fn play_pattern(pattern: &HapticPattern) -> Result<(), HapticError> {
    with_android_context(|env, context| play_pattern_with_context(env, context, pattern))
}
