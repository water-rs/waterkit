//! Windows haptic implementation.

use crate::{HapticError, HapticPattern, HapticStep, Intensity};
use futures::executor::block_on;
use windows::Devices::Haptics::{
    KnownSimpleHapticsControllerWaveforms, SimpleHapticsController,
    SimpleHapticsControllerFeedback, VibrationAccessStatus, VibrationDevice,
};

fn get_controller() -> Result<SimpleHapticsController, HapticError> {
    let access = block_on(async {
        VibrationDevice::RequestAccessAsync()
            .map_err(|e| HapticError::Unknown(e.to_string()))?
            .await
            .map_err(|e| HapticError::Unknown(e.to_string()))
    })?;

    if access != VibrationAccessStatus::Allowed {
        return Err(HapticError::NotSupported);
    }

    let device = block_on(async {
        VibrationDevice::GetDefaultAsync()
            .map_err(|e| HapticError::Unknown(e.to_string()))?
            .await
            .map_err(|e| HapticError::Unknown(e.to_string()))
    })?;

    device
        .SimpleHapticsController()
        .map_err(|e| HapticError::Unknown(e.to_string()))
}

fn find_feedback(
    controller: &SimpleHapticsController,
    waveform_id: u16,
) -> Option<SimpleHapticsControllerFeedback> {
    let supported = controller.SupportedFeedback().ok()?;

    for feedback in supported {
        if let Ok(waveform) = feedback.Waveform() {
            if waveform == waveform_id {
                return Some(feedback);
            }
        }
    }
    None
}

pub fn is_available() -> bool {
    get_controller().is_ok()
}

pub fn impact(intensity: Intensity) -> Result<(), HapticError> {
    let controller = get_controller()?;

    let waveform_id = if intensity.value() > 0.6 {
        KnownSimpleHapticsControllerWaveforms::Press()
    } else {
        KnownSimpleHapticsControllerWaveforms::Click()
    }
    .map_err(|e| HapticError::Unknown(e.to_string()))?;

    if let Some(feedback) = find_feedback(&controller, waveform_id) {
        controller
            .SendHapticFeedbackWithIntensity(&feedback, intensity.value().into())
            .map_err(|e| HapticError::Unknown(e.to_string()))?;
    }
    Ok(())
}

pub fn selection() -> Result<(), HapticError> {
    let controller = get_controller()?;

    let waveform_id = KnownSimpleHapticsControllerWaveforms::Click()
        .map_err(|e| HapticError::Unknown(e.to_string()))?;

    if let Some(feedback) = find_feedback(&controller, waveform_id) {
        controller
            .SendHapticFeedbackWithIntensity(&feedback, 0.3)
            .map_err(|e| HapticError::Unknown(e.to_string()))?;
    }
    Ok(())
}

pub fn notification_success() -> Result<(), HapticError> {
    impact(Intensity::MEDIUM)
}

pub fn notification_warning() -> Result<(), HapticError> {
    let controller = get_controller()?;

    let waveform_id = KnownSimpleHapticsControllerWaveforms::BuzzContinuous()
        .map_err(|e| HapticError::Unknown(e.to_string()))?;

    if let Some(feedback) = find_feedback(&controller, waveform_id) {
        controller
            .SendHapticFeedbackWithIntensity(&feedback, 0.6)
            .map_err(|e| HapticError::Unknown(e.to_string()))?;
    }
    Ok(())
}

pub fn notification_error() -> Result<(), HapticError> {
    let controller = get_controller()?;

    let waveform_id = KnownSimpleHapticsControllerWaveforms::BuzzContinuous()
        .map_err(|e| HapticError::Unknown(e.to_string()))?;

    if let Some(feedback) = find_feedback(&controller, waveform_id) {
        controller
            .SendHapticFeedbackWithIntensity(&feedback, 1.0)
            .map_err(|e| HapticError::Unknown(e.to_string()))?;
    }
    Ok(())
}

pub fn play_pattern(pattern: &HapticPattern) -> Result<(), HapticError> {
    let controller = get_controller()?;

    let click_id = KnownSimpleHapticsControllerWaveforms::Click()
        .map_err(|e| HapticError::Unknown(e.to_string()))?;

    // Windows SimpleHapticsController doesn't support custom patterns well
    // Fall back to playing sequential feedbacks with delays
    for step in pattern.steps() {
        match step {
            HapticStep::Vibrate {
                duration,
                intensity,
            } => {
                if let Some(feedback) = find_feedback(&controller, click_id) {
                    controller
                        .SendHapticFeedbackWithIntensity(&feedback, intensity.value().into())
                        .map_err(|e| HapticError::Unknown(e.to_string()))?;
                }
                std::thread::sleep(*duration);
            }
            HapticStep::Pause(duration) => {
                std::thread::sleep(*duration);
            }
        }
    }
    Ok(())
}
