//! Windows haptic implementation.

use crate::{HapticError, HapticFeedback};
use windows::Devices::Haptics::{
    KnownSimpleHapticsControllerWaveforms, VibrationAccessStatus, VibrationDevice,
};

pub(crate) async fn feedback(style: HapticFeedback) -> Result<(), HapticError> {
    // Check access
    let access = VibrationDevice::RequestAccessAsync()
        .map_err(|e| HapticError::Unknown(e.to_string()))?
        .await
        .map_err(|e| HapticError::Unknown(e.to_string()))?;

    if access != VibrationAccessStatus::Allowed {
        return Err(HapticError::NotSupported);
    }

    // Get default device
    let device = VibrationDevice::GetDefaultAsync()
        .map_err(|e| HapticError::Unknown(e.to_string()))?
        .await
        .map_err(|e| HapticError::Unknown(e.to_string()))?;

    let device = match device {
        Some(d) => d,
        None => return Err(HapticError::NotSupported),
    };

    let controller = device
        .SimpleHapticsController()
        .map_err(|e| HapticError::Unknown(e.to_string()))?;

    // Find supported feedback matching our style
    let waveform_id = match style {
        HapticFeedback::Light => KnownSimpleHapticsControllerWaveforms::Click()?,
        HapticFeedback::Medium => KnownSimpleHapticsControllerWaveforms::Click()?,
        HapticFeedback::Heavy => KnownSimpleHapticsControllerWaveforms::Press()?,
        HapticFeedback::Rigid => KnownSimpleHapticsControllerWaveforms::Click()?,
        HapticFeedback::Soft => KnownSimpleHapticsControllerWaveforms::Click()?,
        HapticFeedback::Selection => KnownSimpleHapticsControllerWaveforms::Click()?,
        HapticFeedback::Success => KnownSimpleHapticsControllerWaveforms::Click()?, // Double click?
        HapticFeedback::Warning => KnownSimpleHapticsControllerWaveforms::BuzzContinuous()?,
        HapticFeedback::Error => KnownSimpleHapticsControllerWaveforms::BuzzContinuous()?,
    };

    let supported_feedbacks = controller
        .SupportedFeedback()
        .map_err(|e| HapticError::Unknown(e.to_string()))?;

    for feedback in supported_feedbacks {
        let waveform = feedback
            .Waveform()
            .map_err(|e| HapticError::Unknown(e.to_string()))?;
        
        if waveform == waveform_id {
            controller
                .SendHapticFeedback(&feedback)
                .map_err(|e| HapticError::Unknown(e.to_string()))?;
            return Ok(());
        }
    }

    // Fallback or ignore if exact waveform not supported
    Ok(())
}
