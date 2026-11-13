//! Cross-cutting permission types.
//!
//! These types are shared across every capability crate that interacts with
//! the platform's runtime permission system. The actual platform calls
//! (`check`, `request`, reactive `status`) live in `waterkit-permission`,
//! which depends on this module for types.

use thiserror::Error;

/// Permission kinds that may be requested at runtime.
///
/// Most platforms cover only a subset; an unsupported request returns
/// [`PermissionError::Unsupported`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Permission {
    /// Live geolocation in the foreground (Apple `WhenInUse` is preferred
    /// when both apply).
    Location,
    /// Foreground-only geolocation. Apple-specific distinction.
    LocationWhenInUse,
    /// Background geolocation as well as foreground.
    LocationAlways,
    /// Camera capture.
    Camera,
    /// Microphone capture.
    Microphone,
    /// Photo library access.
    Photos,
    /// Address-book / contacts access.
    Contacts,
    /// Calendar events access.
    Calendar,
    /// Reminders access (Apple).
    Reminders,
    /// Bluetooth runtime permission (Android 12+ umbrella).
    Bluetooth,
    /// Bluetooth scanning permission (Android 12+).
    BluetoothScan,
    /// Bluetooth connect permission (Android 12+).
    BluetoothConnect,
    /// NFC tag reading.
    Nfc,
    /// Local notification posting permission.
    Notification,
    /// Speech recognition.
    SpeechRecognition,
    /// App Tracking Transparency (Apple).
    Tracking,
    /// Media library / Apple Music.
    MediaLibrary,
    /// Body sensors (Android).
    BodySensors,
    /// `HealthKit` / Health Connect read access for a data type.
    HealthRead(HealthDataKind),
    /// `HealthKit` / Health Connect write access for a data type.
    HealthWrite(HealthDataKind),
    /// Reading the pixels of other apps' windows and of whole displays
    /// (macOS Screen Recording).
    ScreenRecording,
    /// Driving other apps' UI: synthesized input and the accessibility
    /// API (macOS Accessibility).
    Accessibility,
    /// Capturing the screen without the system's window picker (the
    /// one-time consent macOS 15 and later asks for at an app's first such
    /// capture). It builds on [`Permission::ScreenRecording`].
    DirectScreenCapture,
}

/// Discriminator for [`Permission::HealthRead`] / [`Permission::HealthWrite`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HealthDataKind {
    /// Step count.
    Steps,
    /// Heart rate.
    HeartRate,
    /// Active energy burned.
    ActiveEnergy,
    /// Distance walked or run.
    Distance,
    /// Body weight.
    Weight,
    /// Body height.
    Height,
    /// Blood oxygen saturation.
    BloodOxygen,
    /// Sleep records.
    Sleep,
}

/// Current authorization status of a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PermissionStatus {
    /// User has granted the permission.
    Granted,
    /// User has explicitly denied the permission.
    Denied,
    /// Permission is restricted by policy (parental controls, MDM, ...).
    Restricted,
    /// User has not been prompted yet.
    NotDetermined,
    /// The permission is not granted, and the platform does not say whether
    /// the user declined it or was never asked.
    NotGranted,
    /// The platform keeps the answer where the app cannot read it; only the
    /// operation the permission guards reveals whether it is allowed.
    Unknown,
}

/// Errors returned from permission operations.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum PermissionError {
    /// Permission kind has no platform mapping.
    #[error("permission not supported on this platform")]
    Unsupported,
    /// The permission can be requested only once this other one is granted.
    #[error("{0:?} must be granted first")]
    Prerequisite(Permission),
    /// Platform-level failure with a message from the OS.
    #[error("platform error: {0}")]
    Platform(String),
}
