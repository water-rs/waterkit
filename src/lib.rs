//! # Waterkit
//!
//! A comprehensive, cross-platform utility kit for building modern applications.
//!
//! Waterkit provides a unified API for common system functionalities, including audio, video,
//! camera, location, permissions, and more, across macOS, iOS, Android, Windows, and Linux.
//!
//! By default, all features are enabled. If you want to disable some features, you can disable
//! them by adding `default = []` to your `Cargo.toml`.
//!
//! ## Example
//!
//! ```rust, ignore
//! use waterkit::location;
//!
//! async fn get_coords() {
//!     if let Ok(pos) = location::get_current_position().await {
//!         println!("Latitude: {}, Longitude: {}", pos.latitude, pos.longitude);
//!     }
//! }
//! ```

#[cfg(feature = "audio")]
#[doc(inline)]
pub use waterkit_audio as audio;

#[cfg(feature = "biometric")]
#[doc(inline)]
pub use waterkit_biometric as biometric;

#[cfg(feature = "camera")]
#[doc(inline)]
pub use waterkit_camera as camera;

#[cfg(feature = "clipboard")]
#[doc(inline)]
pub use waterkit_clipboard as clipboard;

#[cfg(feature = "codec")]
#[doc(inline)]
pub use waterkit_codec as codec;

#[cfg(feature = "dialog")]
#[doc(inline)]
pub use waterkit_dialog as dialog;

#[cfg(feature = "fs")]
#[doc(inline)]
pub use waterkit_fs as fs;

#[cfg(feature = "haptic")]
#[doc(inline)]
pub use waterkit_haptic as haptic;

#[cfg(feature = "location")]
#[doc(inline)]
pub use waterkit_location as location;

#[cfg(feature = "notification")]
#[doc(inline)]
pub use waterkit_notification as notification;

#[cfg(feature = "permission")]
#[doc(inline)]
pub use waterkit_permission as permission;

#[cfg(feature = "screen")]
#[doc(inline)]
pub use waterkit_screen as screen;

#[cfg(feature = "secret")]
#[doc(inline)]
pub use waterkit_secret as secret;

#[cfg(feature = "sensor")]
#[doc(inline)]
pub use waterkit_sensor as sensor;

#[cfg(feature = "system")]
#[doc(inline)]
pub use waterkit_system as system;

#[cfg(feature = "video")]
#[doc(inline)]
pub use waterkit_video as video;

#[cfg(feature = "bluetooth")]
#[doc(inline)]
pub use waterkit_bluetooth as bluetooth;

#[cfg(feature = "nfc")]
#[doc(inline)]
pub use waterkit_nfc as nfc;

#[cfg(feature = "share")]
#[doc(inline)]
pub use waterkit_share as share;

#[cfg(feature = "speech")]
#[doc(inline)]
pub use waterkit_speech as speech;

#[cfg(feature = "contacts")]
#[doc(inline)]
pub use waterkit_contacts as contacts;

#[cfg(feature = "calendar")]
#[doc(inline)]
pub use waterkit_calendar as calendar;

#[cfg(feature = "health")]
#[doc(inline)]
pub use waterkit_health as health;

#[cfg(feature = "deeplink")]
#[doc(inline)]
pub use waterkit_deeplink as deeplink;
