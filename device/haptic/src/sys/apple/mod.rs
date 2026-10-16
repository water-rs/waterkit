//! Apple platform (iOS/macOS) haptic implementation through objc2.
//!
//! Simple taps go through `UIImpactFeedbackGenerator`,
//! `UISelectionFeedbackGenerator`, and `UINotificationFeedbackGenerator` on
//! iOS and through `NSHapticFeedbackManager` on macOS; patterns go through
//! `CHHapticEngine` on iOS. The engine is created per `play_pattern` call
//! and kept alive by its own finished handler rather than by global state.
//! All UIKit/AppKit work hops to the main thread through
//! [`waterkit_core::apple::on_main`].

use crate::{HapticError, HapticPattern, HapticStep, Intensity};

use waterkit_core::apple::on_main;

#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
    NSHapticFeedbackPerformer as _,
};

#[cfg(target_os = "ios")]
use {
    block2::RcBlock,
    objc2::{AnyThread as _, MainThreadOnly as _},
    objc2_core_haptics::{
        CHHapticDeviceCapability as _, CHHapticEngine, CHHapticEngineFinishedAction, CHHapticEvent,
        CHHapticEventParameter, CHHapticEventParameterIDHapticIntensity,
        CHHapticEventParameterIDHapticSharpness, CHHapticEventTypeHapticContinuous,
        CHHapticPattern, CHHapticPatternPlayer as _, CHHapticTimeImmediate,
    },
    objc2_foundation::NSArray,
    objc2_ui_kit::{
        UIImpactFeedbackGenerator, UIImpactFeedbackStyle, UINotificationFeedbackGenerator,
        UINotificationFeedbackType, UISelectionFeedbackGenerator,
    },
};

#[cfg_attr(
    target_os = "macos",
    expect(
        clippy::missing_const_for_fn,
        reason = "the iOS branch queries CHHapticEngine capabilities; only \
            the macOS branch could be const and the signature is shared"
    )
)]
#[must_use]
pub fn is_available() -> bool {
    #[cfg(target_os = "ios")]
    {
        // SAFETY: `capabilitiesForHardware` is a class-level query and
        // `supportsHaptics` has no calling-thread requirement.
        unsafe { CHHapticEngine::capabilitiesForHardware().supportsHaptics() }
    }
    #[cfg(target_os = "macos")]
    {
        true
    }
}

fn ensure_available() -> Result<(), HapticError> {
    is_available().then_some(()).ok_or(HapticError::Unsupported)
}

#[cfg(target_os = "ios")]
#[expect(
    deprecated,
    reason = "initWithStyle: is the only public initializer that takes a \
        UIImpactFeedbackStyle; objc2 marks it deprecated because UIKit \
        prefers feedbackGeneratorWithStyle:forView:, which needs a view we \
        do not have"
)]
async fn impact_sys(intensity: f32) {
    let style = if intensity < 0.35 {
        UIImpactFeedbackStyle::Light
    } else if intensity < 0.65 {
        UIImpactFeedbackStyle::Medium
    } else if intensity < 0.85 {
        UIImpactFeedbackStyle::Heavy
    } else {
        UIImpactFeedbackStyle::Rigid
    };
    on_main(move |mtm| {
        let generator =
            UIImpactFeedbackGenerator::initWithStyle(UIImpactFeedbackGenerator::alloc(mtm), style);
        generator.prepare();
        generator.impactOccurredWithIntensity(f64::from(intensity));
    })
    .await;
}

#[cfg(target_os = "ios")]
async fn selection_sys() {
    on_main(|mtm| {
        let generator = UISelectionFeedbackGenerator::new(mtm);
        generator.prepare();
        generator.selectionChanged();
    })
    .await;
}

#[cfg(target_os = "ios")]
async fn notification_sys(notification_type: i32) {
    let notification_type = match notification_type {
        0 => UINotificationFeedbackType::Success,
        1 => UINotificationFeedbackType::Warning,
        _ => UINotificationFeedbackType::Error,
    };
    on_main(move |mtm| {
        let generator = UINotificationFeedbackGenerator::new(mtm);
        generator.prepare();
        generator.notificationOccurred(notification_type);
    })
    .await;
}

#[cfg(target_os = "ios")]
async fn play_pattern_sys(timings: &[i32], intensities: &[f32], is_pause: &[bool]) -> bool {
    // SAFETY: `capabilitiesForHardware` is a class-level query and
    // `supportsHaptics` has no calling-thread requirement.
    if !unsafe { CHHapticEngine::capabilitiesForHardware().supportsHaptics() } {
        return false;
    }
    let timings = timings.to_vec();
    let intensities = intensities.to_vec();
    let is_pause = is_pause.to_vec();
    on_main(move |_mtm| {
        // SAFETY: every CoreHaptics call below mirrors the calls the
        // previous implementation issued on the caller's (main) thread:
        // engine init/start, event/pattern/player creation, and a
        // framework-copied finished handler.
        unsafe {
            let Ok(engine) = CHHapticEngine::initAndReturnError(CHHapticEngine::alloc()) else {
                return false;
            };
            if engine.startAndReturnError().is_err() {
                return false;
            }

            let mut events: Vec<objc2::rc::Retained<CHHapticEvent>> =
                Vec::with_capacity(timings.len());
            let mut current_time = 0.0_f64;
            for ((&ms, &intensity), &pause) in timings.iter().zip(&intensities).zip(&is_pause) {
                let duration = f64::from(ms) / 1000.0;
                if !pause {
                    let intensity_param = CHHapticEventParameter::initWithParameterID_value(
                        CHHapticEventParameter::alloc(),
                        CHHapticEventParameterIDHapticIntensity,
                        intensity,
                    );
                    let sharpness_param = CHHapticEventParameter::initWithParameterID_value(
                        CHHapticEventParameter::alloc(),
                        CHHapticEventParameterIDHapticSharpness,
                        0.5,
                    );
                    let parameters = NSArray::from_slice(&[&*intensity_param, &*sharpness_param]);
                    events.push(
                        CHHapticEvent::initWithEventType_parameters_relativeTime_duration(
                            CHHapticEvent::alloc(),
                            CHHapticEventTypeHapticContinuous,
                            &parameters,
                            current_time,
                            duration,
                        ),
                    );
                }
                current_time += duration;
            }
            let event_refs: Vec<&CHHapticEvent> = events.iter().map(|e| &**e).collect();
            let events = NSArray::from_slice(&event_refs);
            let empty = NSArray::<objc2_core_haptics::CHHapticDynamicParameter>::from_slice(&[]);
            let Ok(pattern) = CHHapticPattern::initWithEvents_parameters_error(
                CHHapticPattern::alloc(),
                &events,
                &empty,
            ) else {
                return false;
            };
            let Ok(player) = engine.createPlayerWithPattern_error(&pattern) else {
                return false;
            };

            // The handler holds the engine and the player, so playback
            // keeps going after this call returns; when the framework
            // invokes it once and drops its copy, everything releases.
            let finished = {
                let engine = engine.clone();
                let player = player.clone();
                RcBlock::new(move |_error: *mut objc2_foundation::NSError| {
                    let _keep_alive = (&engine, &player);
                    CHHapticEngineFinishedAction::StopEngine
                })
            };
            engine.notifyWhenPlayersFinished(RcBlock::as_ptr(&finished).cast());

            player.startAtTime_error(CHHapticTimeImmediate).is_ok()
        }
    })
    .await
}

#[cfg(target_os = "macos")]
async fn perform(pattern: NSHapticFeedbackPattern) {
    on_main(move |_mtm| {
        NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
            pattern,
            NSHapticFeedbackPerformanceTime::Default,
        );
    })
    .await;
}

#[cfg(target_os = "macos")]
async fn impact_sys(_intensity: f32) {
    perform(NSHapticFeedbackPattern::Alignment).await;
}

#[cfg(target_os = "macos")]
async fn selection_sys() {
    perform(NSHapticFeedbackPattern::Alignment).await;
}

#[cfg(target_os = "macos")]
async fn notification_sys(_notification_type: i32) {
    perform(NSHapticFeedbackPattern::Generic).await;
}

#[cfg(target_os = "macos")]
async fn play_pattern_sys(_timings: &[i32], _intensities: &[f32], _is_pause: &[bool]) -> bool {
    perform(NSHapticFeedbackPattern::Generic).await;
    true
}

/// # Errors
///
/// Returns [`HapticError::Unsupported`] when the device has no haptic
/// hardware.
pub async fn impact(intensity: Intensity) -> Result<(), HapticError> {
    ensure_available()?;
    impact_sys(intensity.value()).await;
    Ok(())
}

/// # Errors
///
/// Returns [`HapticError::Unsupported`] when the device has no haptic
/// hardware.
pub async fn selection() -> Result<(), HapticError> {
    ensure_available()?;
    selection_sys().await;
    Ok(())
}

/// # Errors
///
/// Returns [`HapticError::Unsupported`] when the device has no haptic
/// hardware.
pub async fn notification_success() -> Result<(), HapticError> {
    ensure_available()?;
    notification_sys(0).await;
    Ok(())
}

/// # Errors
///
/// Returns [`HapticError::Unsupported`] when the device has no haptic
/// hardware.
pub async fn notification_warning() -> Result<(), HapticError> {
    ensure_available()?;
    notification_sys(1).await;
    Ok(())
}

/// # Errors
///
/// Returns [`HapticError::Unsupported`] when the device has no haptic
/// hardware.
pub async fn notification_error() -> Result<(), HapticError> {
    ensure_available()?;
    notification_sys(2).await;
    Ok(())
}

fn duration_ms_i32(duration: std::time::Duration) -> i32 {
    let clamped = duration.as_millis().min(i32::MAX as u128);
    i32::try_from(clamped).expect("clamped haptic duration must fit in i32")
}

/// # Errors
///
/// Returns [`HapticError::Unsupported`] when the device has no haptic
/// hardware, or [`HapticError::Platform`] when playback fails to start.
pub async fn play_pattern(pattern: &HapticPattern) -> Result<(), HapticError> {
    ensure_available()?;
    let mut timings = Vec::with_capacity(pattern.steps().len());
    let mut intensities = Vec::with_capacity(pattern.steps().len());
    let mut is_pause = Vec::with_capacity(pattern.steps().len());

    for step in pattern.steps() {
        match step {
            HapticStep::Vibrate {
                duration,
                intensity,
            } => {
                let ms = duration_ms_i32(*duration);
                timings.push(ms);
                intensities.push(intensity.value());
                is_pause.push(false);
            }
            HapticStep::Pause(duration) => {
                let ms = duration_ms_i32(*duration);
                timings.push(ms);
                intensities.push(0.0);
                is_pause.push(true);
            }
        }
    }

    if play_pattern_sys(&timings, &intensities, &is_pause).await {
        Ok(())
    } else {
        Err(HapticError::Platform("pattern playback failed".into()))
    }
}
