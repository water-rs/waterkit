//! Shared build utilities for waterkit crates.
//!
//! This crate provides common functionality for:
//! - Apple: Swift bridge generation and Swift compilation
//! - Android: Kotlin → DEX compilation
//!
//! # Usage
//!
//! In your `build.rs`:
//!
//! ```ignore
//! use waterkit_build::{build_apple_bridge, build_kotlin, AppleConfig, AndroidConfig};
//!
//! fn main() {
//!     let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
//!
//!     if target_os == "ios" || target_os == "macos" {
//!         build_apple_bridge(&["src/sys/apple/mod.rs"]);
//!     }
//!
//!     if target_os == "android" {
//!         build_kotlin(&["src/sys/android/Helper.kt"]);
//!     }
//! }
//! ```

#![warn(missing_docs)]

#[cfg(not(target_os = "android"))]
mod android;
#[cfg(target_os = "android")]
mod android_runtime;
#[cfg(not(target_os = "android"))]
mod apple;

#[cfg(not(target_os = "android"))]
pub use android::{
    AndroidConfig, build_kotlin, build_kotlin_with_config, find_android_jar, find_d8_jar,
};
#[cfg(not(target_os = "android"))]
pub use apple::{
    AppleSwiftConfig, SwiftBridgeCrate, build_apple_bridge, compile_multi_swift, compile_swift,
};

#[cfg(target_os = "android")]
pub use android_runtime::{
    AndroidError, DexHelper, decode_optional_string, decode_string, jvm_and_context,
    with_android_context,
};
