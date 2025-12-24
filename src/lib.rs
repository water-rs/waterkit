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
//! ```rust
//! use waterkit::location;
//!
//! async fn get_coords() {
//!     if let Ok(pos) = location::get_current_position().await {
//!         println!("Latitude: {}, Longitude: {}", pos.latitude, pos.longitude);
//!     }
//! }
//! ```

#[cfg(feature = "audio")]
pub use waterkit_audio as audio;

#[cfg(feature = "biometric")]
pub use waterkit_biometric as biometric;

#[cfg(feature = "camera")]
pub use waterkit_camera as camera;

#[cfg(feature = "clipboard")]
pub use waterkit_clipboard as clipboard;

#[cfg(feature = "codec")]
pub use waterkit_codec as codec;

#[cfg(feature = "dialog")]
pub use waterkit_dialog as dialog;

#[cfg(feature = "fs")]
pub use waterkit_fs as fs;

#[cfg(feature = "haptic")]
pub use waterkit_haptic as haptic;

#[cfg(feature = "location")]
pub use waterkit_location as location;

#[cfg(feature = "notification")]
pub use waterkit_notification as notification;

#[cfg(feature = "permission")]
pub use waterkit_permission as permission;

#[cfg(feature = "screen")]
pub use waterkit_screen as screen;

#[cfg(feature = "secret")]
pub use waterkit_secret as secret;

#[cfg(feature = "sensor")]
pub use waterkit_sensor as sensor;

#[cfg(feature = "system")]
pub use waterkit_system as system;

#[cfg(feature = "video")]
pub use waterkit_video as video;
