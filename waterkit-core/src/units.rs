//! Typed scalar wrappers with validated value ranges.
//!
//! Each newtype enforces its bounds at construction time and exposes a
//! plain getter. Use these instead of bare `f32` / `f64` for parameters
//! that have a real source of truth (display brightness in `0..=1`, audio
//! pan in `-1..=1`, geographic latitude in `-90..=90`, etc.).
//!
//! All wrappers in this module:
//! - implement `Debug`, `Clone`, `Copy`, `PartialEq`, `PartialOrd`;
//! - expose `MIN` / `MAX` consts;
//! - return `Err(OutOfRange)` from `new` for `NaN` / out-of-range input;
//! - expose `new_unchecked` for compile-time constants.

use core::fmt;
use thiserror::Error;

/// Error returned by unit constructors when the input is out of range.
#[derive(Debug, Clone, Error)]
#[error("value {value} is out of range {range}")]
pub struct OutOfRange {
    /// The offending value (widened to `f64` for uniform display).
    pub value: f64,
    /// Human description of the valid range.
    pub range: &'static str,
}

macro_rules! ranged_unit {
    (
        $(#[$meta:meta])*
        name = $name:ident,
        repr = $repr:ty,
        min = $min:expr,
        max = $max:expr,
        range_doc = $range_doc:literal $(,)?
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
        pub struct $name($repr);

        impl $name {
            /// Lower bound of the valid range.
            pub const MIN: Self = Self($min);
            /// Upper bound of the valid range.
            pub const MAX: Self = Self($max);

            /// Constructs a new value, validating the range.
            ///
            /// # Errors
            ///
            /// Returns [`OutOfRange`] if `value` is `NaN` or outside the
            /// valid range.
            pub fn new(value: $repr) -> Result<Self, OutOfRange> {
                if !value.is_finite() || value < $min || value > $max {
                    return Err(OutOfRange {
                        value: f64::from(value),
                        range: $range_doc,
                    });
                }
                Ok(Self(value))
            }

            /// Constructs without validation.
            ///
            /// The caller must ensure `value` lies within the valid range.
            #[must_use]
            pub const fn new_unchecked(value: $repr) -> Self {
                Self(value)
            }

            /// Inner numeric value.
            #[must_use]
            pub const fn get(self) -> $repr {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

ranged_unit! {
    /// Display brightness, normalized `0.0..=1.0`.
    name = Brightness,
    repr = f32,
    min = 0.0,
    max = 1.0,
    range_doc = "0.0..=1.0",
}

ranged_unit! {
    /// Audio output volume, normalized `0.0..=1.0`.
    name = Volume,
    repr = f32,
    min = 0.0,
    max = 1.0,
    range_doc = "0.0..=1.0",
}

ranged_unit! {
    /// Stereo panning, `-1.0` (full left) ..= `1.0` (full right).
    name = Pan,
    repr = f32,
    min = -1.0,
    max = 1.0,
    range_doc = "-1.0..=1.0",
}

ranged_unit! {
    /// Audio playback rate multiplier; `1.0` is normal speed. Values
    /// `0.25..=4.0` cover all reasonable use cases on every platform.
    name = PlaybackRate,
    repr = f32,
    min = 0.25,
    max = 4.0,
    range_doc = "0.25..=4.0",
}

ranged_unit! {
    /// Pitch multiplier; `1.0` is unchanged.
    name = Pitch,
    repr = f32,
    min = 0.5,
    max = 2.0,
    range_doc = "0.5..=2.0",
}

ranged_unit! {
    /// Camera or display zoom factor; `1.0` is no zoom. Upper bound `100.0`
    /// covers known device caps.
    name = Zoom,
    repr = f32,
    min = 1.0,
    max = 100.0,
    range_doc = "1.0..=100.0",
}

ranged_unit! {
    /// Geographic latitude in degrees, `-90.0..=90.0`.
    name = Latitude,
    repr = f64,
    min = -90.0,
    max = 90.0,
    range_doc = "-90.0..=90.0",
}

ranged_unit! {
    /// Geographic longitude in degrees, `-180.0..=180.0`.
    name = Longitude,
    repr = f64,
    min = -180.0,
    max = 180.0,
    range_doc = "-180.0..=180.0",
}

ranged_unit! {
    /// Display refresh rate in Hz, `1.0..=480.0`. Upper bound exceeds any
    /// shipping consumer panel.
    name = RefreshRate,
    repr = f32,
    min = 1.0,
    max = 480.0,
    range_doc = "1.0..=480.0",
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_accepts_in_range() {
        assert!(Brightness::new(0.0).is_ok());
        assert!(Brightness::new(1.0).is_ok());
        assert!(Brightness::new(0.5).is_ok());
    }

    #[test]
    fn brightness_rejects_out_of_range() {
        assert!(Brightness::new(-0.1).is_err());
        assert!(Brightness::new(1.1).is_err());
        assert!(Brightness::new(f32::NAN).is_err());
        assert!(Brightness::new(f32::INFINITY).is_err());
    }

    #[test]
    fn pan_accepts_negative_range() {
        assert!(Pan::new(-1.0).is_ok());
        assert!(Pan::new(0.0).is_ok());
        assert!(Pan::new(1.0).is_ok());
        assert!(Pan::new(-1.001).is_err());
    }

    #[test]
    fn min_max_consts_are_consistent() {
        assert!((Brightness::MIN.get() - 0.0).abs() < f32::EPSILON);
        assert!((Brightness::MAX.get() - 1.0).abs() < f32::EPSILON);
        assert!((Pan::MIN.get() - -1.0).abs() < f32::EPSILON);
        assert!((Pan::MAX.get() - 1.0).abs() < f32::EPSILON);
    }
}
