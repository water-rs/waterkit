//! Linux haptic implementation.
//!
//! Desktop Linux typically lacks haptic hardware, so this returns `NotSupported`.

use crate::{HapticError, HapticPattern, Intensity};

pub fn is_available() -> bool {
    false
}

pub fn impact(_intensity: Intensity) -> Result<(), HapticError> {
    Err(HapticError::NotSupported)
}

pub fn selection() -> Result<(), HapticError> {
    Err(HapticError::NotSupported)
}

pub fn notification_success() -> Result<(), HapticError> {
    Err(HapticError::NotSupported)
}

pub fn notification_warning() -> Result<(), HapticError> {
    Err(HapticError::NotSupported)
}

pub fn notification_error() -> Result<(), HapticError> {
    Err(HapticError::NotSupported)
}

pub fn play_pattern(_pattern: &HapticPattern) -> Result<(), HapticError> {
    Err(HapticError::NotSupported)
}
