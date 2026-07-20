//! Cross-platform haptic feedback.
//!
//! This crate provides a unified API for triggering haptic feedback
//! (vibration) across iOS, macOS, Android, Windows, and Linux.
//!
//! All trigger functions are synchronous: the underlying platform calls
//! return immediately after dispatching to the OS haptic engine. They
//! must be called on the main / UI thread on Apple platforms.
//!
//! # Example
//!
//! ```no_run
//! use waterkit_haptic::{Haptic, HapticPattern, Intensity};
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), waterkit_haptic::HapticError> {
//! Haptic::impact(Intensity::MEDIUM)?;
//! Haptic::selection()?;
//! Haptic::notification_success()?;
//!
//! let pattern = HapticPattern::builder()
//!     .add(Duration::from_millis(100), Intensity::MAX)
//!     .pause(Duration::from_millis(50))
//!     .add(Duration::from_millis(200), Intensity::MEDIUM)
//!     .build();
//!
//! Haptic::play(&pattern)?;
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod sys;

use std::time::Duration;
use waterkit_core::{Capabilities, OutOfRange};

/// Haptic feedback intensity.
///
/// Internal value is guaranteed to be in `0.0..=1.0`.
///
/// # Errors
///
/// [`Intensity::new`] returns [`OutOfRange`] if the input is `NaN` or outside
/// `0.0..=1.0`.
///
/// # Example
///
/// ```
/// use waterkit_haptic::Intensity;
///
/// # fn main() -> Result<(), waterkit_core::OutOfRange> {
/// // Use predefined levels
/// let _low = Intensity::LOW;
/// let _medium = Intensity::MEDIUM;
///
/// // Or create custom intensity
/// let custom = Intensity::new(0.7)?;
/// assert_eq!(custom.value(), 0.7);
///
/// // Invalid values are rejected
/// assert!(Intensity::new(1.5).is_err());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Intensity(f32);

impl Intensity {
    /// Low intensity (0.25).
    pub const LOW: Self = Self(0.25);

    /// Medium intensity (0.5).
    pub const MEDIUM: Self = Self(0.5);

    /// High intensity (0.75).
    pub const HIGH: Self = Self(0.75);

    /// Maximum intensity (1.0).
    pub const MAX: Self = Self(1.0);

    /// Create a custom intensity value.
    ///
    /// # Errors
    ///
    /// Returns [`OutOfRange`] if `value` is `NaN` or outside `0.0..=1.0`.
    pub fn new(value: f32) -> Result<Self, OutOfRange> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(OutOfRange {
                value: f64::from(value),
                range: "0.0..=1.0",
            });
        }
        Ok(Self(value))
    }

    /// Constructs without validation.
    ///
    /// The caller must ensure `value` lies within `0.0..=1.0`.
    #[must_use]
    pub const fn new_unchecked(value: f32) -> Self {
        Self(value)
    }

    /// Get the raw intensity value (`0.0..=1.0`).
    #[must_use]
    pub const fn value(&self) -> f32 {
        self.0
    }
}

impl Default for Intensity {
    fn default() -> Self {
        Self::MEDIUM
    }
}

#[cfg(test)]
mod intensity_tests {
    use super::Intensity;

    #[test]
    fn new_accepts_boundaries() {
        assert_eq!(
            Intensity::new(0.0)
                .expect("valid intensity")
                .value()
                .to_bits(),
            0.0_f32.to_bits()
        );
        assert_eq!(
            Intensity::new(1.0)
                .expect("valid intensity")
                .value()
                .to_bits(),
            1.0_f32.to_bits()
        );
        assert_eq!(
            Intensity::new(0.7)
                .expect("valid intensity")
                .value()
                .to_bits(),
            0.7_f32.to_bits()
        );
    }

    #[test]
    fn new_rejects_nan_and_out_of_range() {
        assert!(Intensity::new(f32::NAN).is_err());
        assert!(Intensity::new(f32::INFINITY).is_err());
        assert!(Intensity::new(-0.1).is_err());
        assert!(Intensity::new(1.1).is_err());
    }
}

/// A single step in a haptic pattern.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum HapticStep {
    /// Vibrate for a duration at a given intensity.
    Vibrate {
        /// Duration of the vibration.
        duration: Duration,
        /// Intensity of the vibration.
        intensity: Intensity,
    },
    /// Pause (no vibration) for a duration.
    Pause(Duration),
}

/// A custom haptic pattern composed of multiple steps.
///
/// Use [`HapticPattern::builder()`] to create a builder.
///
/// # Example
///
/// ```
/// use waterkit_haptic::{HapticPattern, Intensity};
/// use std::time::Duration;
///
/// let pattern = HapticPattern::builder()
///     .add(Duration::from_millis(100), Intensity::MAX)
///     .pause(Duration::from_millis(50))
///     .add(Duration::from_millis(200), Intensity::MEDIUM)
///     .build();
///
/// assert_eq!(pattern.steps().len(), 3);
/// assert_eq!(pattern.total_duration(), Duration::from_millis(350));
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HapticPattern {
    steps: Vec<HapticStep>,
}

impl HapticPattern {
    /// Create a new pattern builder.
    #[must_use]
    pub fn builder() -> HapticPatternBuilder {
        HapticPatternBuilder::new()
    }

    /// Get the steps in this pattern.
    #[must_use]
    pub fn steps(&self) -> &[HapticStep] {
        &self.steps
    }

    /// Get the total duration of this pattern.
    #[must_use]
    pub fn total_duration(&self) -> Duration {
        self.steps.iter().fold(Duration::ZERO, |acc, step| {
            acc + match step {
                HapticStep::Vibrate { duration, .. } => *duration,
                HapticStep::Pause(d) => *d,
            }
        })
    }
}

/// Builder for [`HapticPattern`].
#[derive(Debug, Clone, Default)]
pub struct HapticPatternBuilder {
    steps: Vec<HapticStep>,
}

impl HapticPatternBuilder {
    /// Create a new pattern builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a vibration step.
    #[must_use]
    pub fn add(mut self, duration: Duration, intensity: Intensity) -> Self {
        self.steps.push(HapticStep::Vibrate {
            duration,
            intensity,
        });
        self
    }

    /// Add a pause step.
    #[must_use]
    pub fn pause(mut self, duration: Duration) -> Self {
        self.steps.push(HapticStep::Pause(duration));
        self
    }

    /// Build the pattern.
    #[must_use]
    pub fn build(self) -> HapticPattern {
        HapticPattern { steps: self.steps }
    }
}

/// Errors that can occur when triggering haptic feedback.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum HapticError {
    /// Haptic feedback is not supported on this device.
    #[error("haptic feedback not supported")]
    Unsupported,

    /// The haptic pattern is invalid (e.g., empty).
    #[error("invalid haptic pattern: {0}")]
    InvalidPattern(String),

    /// Failed to initialize the haptic engine.
    #[error("haptic engine initialization failed: {0}")]
    InitFailed(String),

    /// Platform-level failure with a message.
    #[error("platform error: {0}")]
    Platform(String),
}

/// Capability probe result for the haptic subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct HapticCapabilities {
    /// Whether haptic feedback is available on this device.
    pub available: bool,
}

impl Capabilities for HapticCapabilities {
    fn available(&self) -> bool {
        self.available
    }
}

/// ZST namespace for haptic feedback operations.
///
/// All methods are synchronous and dispatch to the platform's haptic
/// engine. On Apple platforms the trigger calls must run on the main
/// thread.
///
/// # Example
///
/// ```no_run
/// use waterkit_haptic::{Haptic, Intensity};
///
/// # fn example() -> Result<(), waterkit_haptic::HapticError> {
/// if Haptic::capabilities().available {
///     Haptic::impact(Intensity::MEDIUM)?;
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Haptic;

impl Haptic {
    /// Probes the haptic subsystem.
    #[must_use]
    pub fn capabilities() -> HapticCapabilities {
        HapticCapabilities {
            available: sys::is_available(),
        }
    }

    /// Triggers an impact feedback with the specified intensity.
    ///
    /// # Errors
    ///
    /// Returns a [`HapticError`] if haptics are not supported.
    pub fn impact(intensity: Intensity) -> Result<(), HapticError> {
        sys::impact(intensity)
    }

    /// Triggers a selection feedback (light tap for UI selection changes).
    ///
    /// # Errors
    ///
    /// Returns a [`HapticError`] if haptics are not supported.
    pub fn selection() -> Result<(), HapticError> {
        sys::selection()
    }

    /// Triggers a success notification feedback.
    ///
    /// # Errors
    ///
    /// Returns a [`HapticError`] if haptics are not supported.
    pub fn notification_success() -> Result<(), HapticError> {
        sys::notification_success()
    }

    /// Triggers a warning notification feedback.
    ///
    /// # Errors
    ///
    /// Returns a [`HapticError`] if haptics are not supported.
    pub fn notification_warning() -> Result<(), HapticError> {
        sys::notification_warning()
    }

    /// Triggers an error notification feedback.
    ///
    /// # Errors
    ///
    /// Returns a [`HapticError`] if haptics are not supported.
    pub fn notification_error() -> Result<(), HapticError> {
        sys::notification_error()
    }

    /// Plays a custom haptic pattern.
    ///
    /// # Errors
    ///
    /// Returns [`HapticError::InvalidPattern`] for an empty pattern, or
    /// [`HapticError::Unsupported`] / [`HapticError::Platform`] from the
    /// platform engine.
    pub fn play(pattern: &HapticPattern) -> Result<(), HapticError> {
        if pattern.steps.is_empty() {
            return Err(HapticError::InvalidPattern("pattern is empty".into()));
        }
        sys::play_pattern(pattern)
    }
}
