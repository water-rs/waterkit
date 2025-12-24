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

mod android;
mod apple;

pub use android::{AndroidConfig, build_kotlin, find_android_jar, find_d8_jar};
pub use apple::{AppleSwiftConfig, build_apple_bridge, compile_swift};
