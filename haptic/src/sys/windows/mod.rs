//! Windows haptic implementation.

use crate::{HapticError, HapticPattern, HapticStep, Intensity};
use futures::executor::block_on;
use std::thread;
use std::time::Duration;
use windows::Devices::Haptics::{
    KnownSimpleHapticsControllerWaveforms, SimpleHapticsController,
    SimpleHapticsControllerFeedback, VibrationAccessStatus, VibrationDevice,
};

fn spawn_haptic_task(
    task: impl FnOnce() -> Result<(), HapticError> + Send + 'static,
) -> Result<(), HapticError> {
    thread::Builder::new()
        .name("waterkit-haptic-win".into())
        .spawn(move || {
            let _ = task();
        })
        .map(|_| ())
        .map_err(|error| HapticError::InitFailed(format!("failed to spawn haptic worker: {error}")))
}

fn get_controller_blocking() -> Result<SimpleHapticsController, HapticError> {
    let access = block_on(async {
        VibrationDevice::RequestAccessAsync()
            .map_err(|e| HapticError::Platform(e.to_string()))?
            .await
            .map_err(|e| HapticError::Platform(e.to_string()))
    })?;

    if access != VibrationAccessStatus::Allowed {
        return Err(HapticError::InitFailed(format!(
            "vibration access not allowed: {access:?}"
        )));
    }

    let device = block_on(async {
        VibrationDevice::GetDefaultAsync()
            .map_err(|e| HapticError::Platform(e.to_string()))?
            .await
            .map_err(|e| HapticError::Platform(e.to_string()))
    })?;

    device
        .SimpleHapticsController()
        .map_err(|e| HapticError::Platform(e.to_string()))
}

fn find_feedback(
    controller: &SimpleHapticsController,
    waveform_id: u16,
) -> Option<SimpleHapticsControllerFeedback> {
    let supported = controller.SupportedFeedback().ok()?;

    for feedback in supported {
        if let Ok(waveform) = feedback.Waveform()
            && waveform == waveform_id
        {
            return Some(feedback);
        }
    }
    None
}

fn send_waveform(
    controller: &SimpleHapticsController,
    waveform_id: u16,
    intensity: f64,
) -> Result<(), HapticError> {
    if let Some(feedback) = find_feedback(controller, waveform_id) {
        controller
            .SendHapticFeedbackWithIntensity(&feedback, intensity)
            .map_err(|e| HapticError::Platform(e.to_string()))?;
    }
    Ok(())
}

fn run_click_for(duration: Duration, intensity: f64) -> Result<(), HapticError> {
    let controller = get_controller_blocking()?;
    let click_id = KnownSimpleHapticsControllerWaveforms::Click()
        .map_err(|e| HapticError::Platform(e.to_string()))?;
    send_waveform(&controller, click_id, intensity)?;
    thread::sleep(duration);
    Ok(())
}

pub fn is_available() -> bool {
    VibrationDevice::RequestAccessAsync().is_ok() && VibrationDevice::GetDefaultAsync().is_ok()
}

pub fn impact(intensity: Intensity) -> Result<(), HapticError> {
    let waveform_id = if intensity.value() > 0.6 {
        KnownSimpleHapticsControllerWaveforms::Press()
    } else {
        KnownSimpleHapticsControllerWaveforms::Click()
    }
    .map_err(|e| HapticError::Platform(e.to_string()))?;

    spawn_haptic_task(move || {
        let controller = get_controller_blocking()?;
        send_waveform(&controller, waveform_id, f64::from(intensity.value()))
    })
}

pub fn selection() -> Result<(), HapticError> {
    let waveform_id = KnownSimpleHapticsControllerWaveforms::Click()
        .map_err(|e| HapticError::Platform(e.to_string()))?;

    spawn_haptic_task(move || {
        let controller = get_controller_blocking()?;
        send_waveform(&controller, waveform_id, 0.3)
    })
}

pub fn notification_success() -> Result<(), HapticError> {
    impact(Intensity::MEDIUM)
}

pub fn notification_warning() -> Result<(), HapticError> {
    let waveform_id = KnownSimpleHapticsControllerWaveforms::BuzzContinuous()
        .map_err(|e| HapticError::Platform(e.to_string()))?;

    spawn_haptic_task(move || {
        let controller = get_controller_blocking()?;
        send_waveform(&controller, waveform_id, 0.6)
    })
}

pub fn notification_error() -> Result<(), HapticError> {
    let waveform_id = KnownSimpleHapticsControllerWaveforms::BuzzContinuous()
        .map_err(|e| HapticError::Platform(e.to_string()))?;

    spawn_haptic_task(move || {
        let controller = get_controller_blocking()?;
        send_waveform(&controller, waveform_id, 1.0)
    })
}

pub fn play_pattern(pattern: &HapticPattern) -> Result<(), HapticError> {
    if pattern.steps().is_empty() {
        return Err(HapticError::InvalidPattern(
            "pattern must contain at least one step".into(),
        ));
    }

    let steps = pattern.steps().to_vec();
    spawn_haptic_task(move || {
        for step in steps {
            match step {
                HapticStep::Vibrate {
                    duration,
                    intensity,
                } => run_click_for(duration, f64::from(intensity.value()))?,
                HapticStep::Pause(duration) => thread::sleep(duration),
            }
        }
        Ok(())
    })
}
