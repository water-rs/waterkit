//! Shared build utilities for waterkit crates.
//!
//! This crate provides common functionality for:
//! - Apple: Swift bridge generation and Swift compilation
//! - Android: runtime helpers that resolve Kotlin classes the packager
//!   compiled into the application
//!
//! # Usage
//!
//! In your `build.rs`:
//!
//! ```ignore
//! use waterkit_build::{SwiftBridge, SwiftBridges};
//!
//! fn main() {
//!     let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
//!
//!     if target_os == "ios" || target_os == "macos" {
//!         SwiftBridges::new()
//!             .bridge(
//!                 SwiftBridge::new("src/sys/apple/mod.rs")
//!                     .swift_source("src/sys/apple/Feature.swift")
//!                     .framework("Foundation"),
//!             )
//!             .compile();
//!     }
//! }
//! ```

#![warn(missing_docs)]

#[cfg(target_os = "android")]
mod android_runtime;
#[cfg(not(target_os = "android"))]
mod apple;

#[cfg(not(target_os = "android"))]
pub use apple::{SwiftBridge, SwiftBridges};

#[cfg(target_os = "android")]
pub use android_runtime::{
    AndroidError, DexHelper, decode_optional_string, decode_string, describe_jni_error,
    jvm_and_context, with_android_context,
};

#[cfg(all(target_os = "android", feature = "activity-result"))]
pub use android_runtime::{
    ActivityResult, ActivityResultError, PendingActivityResult, ResultCode,
    start_activity_for_result, start_intent_sender_for_result,
};

#[cfg(all(target_os = "android", feature = "native-callback"))]
pub use android_runtime::{FromJava, NativeCallback, NativeChannel, PeerError};
