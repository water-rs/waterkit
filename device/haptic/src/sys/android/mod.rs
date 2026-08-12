//! Android haptic implementation using JNI.

use crate::{HapticError, HapticPattern, HapticStep, Intensity};
use jni::objects::{JIntArray, JLongArray, JObject, JValue};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{AndroidError, DexHelper, dex_helper, with_android_context};

/// `waterkit.haptic.HapticHelper`, embedded as a DEX by this crate's build
/// script and loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.haptic.HapticHelper");

impl From<AndroidError> for HapticError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

impl From<jni::errors::Error> for HapticError {
    fn from(error: jni::errors::Error) -> Self {
        Self::Platform(error.to_string())
    }
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
    let helper = HELPER.class(env, context)?;

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
    let helper = HELPER.class(env, context)?;

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
    let helper = HELPER.class(env, context)?;

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
    let helper = HELPER.class(env, context)?;

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

    let helper = HELPER.class(env, context)?;

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
