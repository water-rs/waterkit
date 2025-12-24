//! Apple platform (iOS/macOS) haptic implementation using swift-bridge.

use crate::{HapticError, HapticFeedback};

#[swift_bridge::bridge]
mod ffi {
    #[swift_bridge(swift_name = "HapticFeedbackType")]
    enum SwiftHapticFeedback {
        Light,
        Medium,
        Heavy,
        Rigid,
        Soft,
        Selection,
        Success,
        Warning,
        Error,
    }

    extern "Swift" {
        fn trigger_haptic(style: SwiftHapticFeedback);
    }
}

pub async fn feedback(style: HapticFeedback) -> Result<(), HapticError> {
    let swift_style = match style {
        HapticFeedback::Light => ffi::SwiftHapticFeedback::Light,
        HapticFeedback::Medium => ffi::SwiftHapticFeedback::Medium,
        HapticFeedback::Heavy => ffi::SwiftHapticFeedback::Heavy,
        HapticFeedback::Rigid => ffi::SwiftHapticFeedback::Rigid,
        HapticFeedback::Soft => ffi::SwiftHapticFeedback::Soft,
        HapticFeedback::Selection => ffi::SwiftHapticFeedback::Selection,
        HapticFeedback::Success => ffi::SwiftHapticFeedback::Success,
        HapticFeedback::Warning => ffi::SwiftHapticFeedback::Warning,
        HapticFeedback::Error => ffi::SwiftHapticFeedback::Error,
    };

    ffi::trigger_haptic(swift_style);
    Ok(())
}
