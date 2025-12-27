//! Apple platform (iOS/macOS) haptic implementation using swift-bridge.

#![allow(clippy::unnecessary_wraps)] // API requires Result for cross-platform consistency
#![allow(clippy::cast_possible_truncation)] // Durations are clamped to i32::MAX before cast

use crate::{HapticError, HapticPattern, HapticStep, Intensity};

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn haptic_is_available() -> bool;
        fn haptic_impact(intensity: f32);
        fn haptic_selection();
        fn haptic_notification(notification_type: i32);
        fn haptic_play_pattern(timings: Vec<i32>, intensities: Vec<f32>, is_pause: Vec<bool>)
            -> bool;
    }
}

pub fn is_available() -> bool {
    ffi::haptic_is_available()
}

pub fn impact(intensity: Intensity) -> Result<(), HapticError> {
    ffi::haptic_impact(intensity.value());
    Ok(())
}

pub fn selection() -> Result<(), HapticError> {
    ffi::haptic_selection();
    Ok(())
}

pub fn notification_success() -> Result<(), HapticError> {
    ffi::haptic_notification(0);
    Ok(())
}

pub fn notification_warning() -> Result<(), HapticError> {
    ffi::haptic_notification(1);
    Ok(())
}

pub fn notification_error() -> Result<(), HapticError> {
    ffi::haptic_notification(2);
    Ok(())
}

pub fn play_pattern(pattern: &HapticPattern) -> Result<(), HapticError> {
    let mut timings = Vec::with_capacity(pattern.steps().len());
    let mut intensities = Vec::with_capacity(pattern.steps().len());
    let mut is_pause = Vec::with_capacity(pattern.steps().len());

    for step in pattern.steps() {
        match step {
            HapticStep::Vibrate { duration, intensity } => {
                // Saturate at i32::MAX for extremely long durations
                let ms = duration.as_millis().min(i32::MAX as u128) as i32;
                timings.push(ms);
                intensities.push(intensity.value());
                is_pause.push(false);
            }
            HapticStep::Pause(duration) => {
                let ms = duration.as_millis().min(i32::MAX as u128) as i32;
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
