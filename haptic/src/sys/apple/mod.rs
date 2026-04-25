//! Apple platform (iOS/macOS) haptic implementation using swift-bridge.

use crate::{HapticError, HapticPattern, HapticStep, Intensity};

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn haptic_is_available() -> bool;
        fn haptic_impact(intensity: f32);
        fn haptic_selection();
        fn haptic_notification(notification_type: i32);
        fn haptic_play_pattern(
            timings: Vec<i32>,
            intensities: Vec<f32>,
            is_pause: Vec<bool>,
        ) -> bool;
    }
}

pub fn is_available() -> bool {
    ffi::haptic_is_available()
}

fn ensure_available() -> Result<(), HapticError> {
    is_available().then_some(()).ok_or(HapticError::Unsupported)
}

pub fn impact(intensity: Intensity) -> Result<(), HapticError> {
    ensure_available()?;
    ffi::haptic_impact(intensity.value());
    Ok(())
}

pub fn selection() -> Result<(), HapticError> {
    ensure_available()?;
    ffi::haptic_selection();
    Ok(())
}

pub fn notification_success() -> Result<(), HapticError> {
    ensure_available()?;
    ffi::haptic_notification(0);
    Ok(())
}

pub fn notification_warning() -> Result<(), HapticError> {
    ensure_available()?;
    ffi::haptic_notification(1);
    Ok(())
}

pub fn notification_error() -> Result<(), HapticError> {
    ensure_available()?;
    ffi::haptic_notification(2);
    Ok(())
}

fn duration_ms_i32(duration: std::time::Duration) -> i32 {
    let clamped = duration.as_millis().min(i32::MAX as u128);
    i32::try_from(clamped).expect("clamped haptic duration must fit in i32")
}

pub fn play_pattern(pattern: &HapticPattern) -> Result<(), HapticError> {
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

    if ffi::haptic_play_pattern(timings, intensities, is_pause) {
        Ok(())
    } else {
        Err(HapticError::Unknown("pattern playback failed".into()))
    }
}
