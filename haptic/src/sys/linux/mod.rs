//! Linux haptic implementation.

use crate::{HapticError, HapticFeedback};

pub(crate) async fn feedback(_style: HapticFeedback) -> Result<(), HapticError> {
    // TODO: Implement via UPower or other mechanism
    Err(HapticError::NotSupported)
}
