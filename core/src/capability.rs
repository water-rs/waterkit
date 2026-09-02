//! Capability-probe pattern.
//!
//! Every capability crate exposes a `Capabilities` struct describing what
//! the platform supports (e.g. `BiometricCapabilities`,
//! `SensorCapabilities`). This trait is the lowest common denominator —
//! a single `available` bit so generic code can ask "is this capability
//! present at all?" without knowing the rich shape.

/// Lowest common denominator across capability-probe structs.
pub trait Capabilities {
    /// Returns `true` if the capability is available on this platform at
    /// runtime. Equivalent to the legacy `is_available()` helper.
    fn available(&self) -> bool;
}
