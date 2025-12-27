//! # Waterkit
//!
//! A comprehensive, cross-platform utility kit for building modern applications with WaterUI.
//!
//! Waterkit provides a unified API for common system functionalities, including audio, video,
//! camera, location, permissions, and more, across macOS, iOS, Android, Windows, and Linux.
//!
//! ## Features
//!
//! Waterkit is highly modular. You can enable only the features you need to keep your
//! dependencies minimal.
//!
//! - `audio`: Audio playback and recording.
//! - `video`: Video playback and muxing/demuxing.
//! - `camera`: Camera access and photo capture.
//! - `location`: GPS and geolocation services.
//! - `permission`: Unified permission request handling.
//! - `haptic`: Haptic feedback for mobile and desktop.
//! - `notification`: Local notifications.
//! - `dialog`: Native system dialogs (alerts, file pickers).
//! - `biometric`: Biometric authentication (FaceID, Fingerprint).
//! - `clipboard`: System clipboard access (text and images).
//! - `fs`: File system utilities and sandboxed access.
//! - `secret`: Secure storage for sensitive information.
//! - `sensor`: Device sensors (accelerometer, light, etc.).
//! - `codec`: Hardware-accelerated video codecs.
//! - `screen`: Screen capture and display information.
//! - `system`: System information and power management.
//!
//! Use the `full` feature to enable everything.
//!
//! ## Example
//!
//! ```toml
//! [dependencies]
//! waterkit = { version = "0.1", features = ["location", "notification"] }
//! ```
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
