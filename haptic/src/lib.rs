//! Cross-platform haptic feedback.
//!
//! This crate provides a unified API for triggering haptic feedback (vibration)
//! across iOS, macOS, Android, Windows, and Linux platforms.
//!
//! # Example
//!
//! ```no_run
//! use waterkit_haptic::{Haptic, HapticPattern, Intensity};
//! use std::time::Duration;
//!
//! # fn example() -> Result<(), waterkit_haptic::HapticError> {
//! // Simple feedback with preset intensity
//! Haptic::impact(Intensity::MEDIUM)?;
//! Haptic::selection()?;
//! Haptic::notification_success()?;
//!
//! // Custom pattern
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

mod sys;

use std::time::Duration;

/// Haptic feedback intensity.
///
/// Internal value is clamped to `0.0..=1.0`.
///
/// # Example
///
/// ```
/// use waterkit_haptic::Intensity;
///
/// // Use predefined levels
/// let low = Intensity::LOW;
/// let medium = Intensity::MEDIUM;
///
/// // Or create custom intensity
/// let custom = Intensity::new(0.7);
/// assert_eq!(custom.value(), 0.7);
///
/// // Values are clamped
/// let clamped = Intensity::new(1.5);
/// assert_eq!(clamped.value(), 1.0);
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
    /// The value is clamped to `0.0..=1.0`.
    #[must_use]
    pub const fn new(value: f32) -> Self {
        // Manual clamping since f32::clamp is not const fn
        let v = if value < 0.0 {
            0.0
        } else if value > 1.0 {
            1.0
        } else {
            value
        };
        Self(v)
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

/// A single step in a haptic pattern.
#[derive(Debug, Clone, Copy, PartialEq)]
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
        self.steps
            .push(HapticStep::Vibrate { duration, intensity });
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
pub enum HapticError {
    /// Haptic feedback is not supported on this device.
    #[error("haptic feedback not supported")]
    NotSupported,

    /// The haptic pattern is invalid (e.g., empty).
    #[error("invalid haptic pattern: {0}")]
    InvalidPattern(String),

    /// Failed to initialize the haptic engine.
    #[error("haptic engine initialization failed: {0}")]
    InitFailed(String),

    /// An unknown error occurred.
    #[error("unknown error: {0}")]
    Unknown(String),
}

/// ZST namespace for haptic feedback operations.
///
/// All methods are static and synchronous (fire-and-forget).
///
/// # Example
///
/// ```no_run
/// use waterkit_haptic::{Haptic, Intensity};
///
/// # fn example() -> Result<(), waterkit_haptic::HapticError> {
/// // Check availability
/// if Haptic::is_available() {
///     Haptic::impact(Intensity::MEDIUM)?;
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Haptic;

impl Haptic {
    /// Check if haptic feedback is available on this device.
    #[must_use]
    pub fn is_available() -> bool {
        sys::is_available()
    }

    /// Trigger an impact feedback with the specified intensity.
    ///
    /// # Errors
    ///
    /// Returns a [`HapticError`] if haptics are not supported.
    pub fn impact(intensity: Intensity) -> Result<(), HapticError> {
        sys::impact(intensity)
    }

    /// Trigger a selection feedback (light tap for UI selection changes).
    ///
    /// # Errors
    ///
    /// Returns a [`HapticError`] if haptics are not supported.
    pub fn selection() -> Result<(), HapticError> {
        sys::selection()
    }

    /// Trigger a success notification feedback.
    ///
    /// # Errors
    ///
    /// Returns a [`HapticError`] if haptics are not supported.
    pub fn notification_success() -> Result<(), HapticError> {
        sys::notification_success()
    }

    /// Trigger a warning notification feedback.
    ///
    /// # Errors
    ///
    /// Returns a [`HapticError`] if haptics are not supported.
    pub fn notification_warning() -> Result<(), HapticError> {
        sys::notification_warning()
    }

    /// Trigger an error notification feedback.
    ///
    /// # Errors
    ///
    /// Returns a [`HapticError`] if haptics are not supported.
    pub fn notification_error() -> Result<(), HapticError> {
        sys::notification_error()
    }

    /// Play a custom haptic pattern.
    ///
    /// # Errors
    ///
    /// Returns a [`HapticError`] if haptics are not supported or the pattern is invalid.
    pub fn play(pattern: &HapticPattern) -> Result<(), HapticError> {
        if pattern.steps.is_empty() {
            return Err(HapticError::InvalidPattern("pattern is empty".into()));
        }
        sys::play_pattern(pattern)
    }
}
