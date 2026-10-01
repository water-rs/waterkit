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
//! use waterkit_build::build_apple_bridge;
//!
//! fn main() {
//!     let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
//!
//!     if target_os == "ios" || target_os == "macos" {
//!         build_apple_bridge(&["src/sys/apple/mod.rs"]);
//!     }
//! }
//! ```

#![warn(missing_docs)]

#[cfg(target_os = "android")]
mod android_runtime;
#[cfg(not(target_os = "android"))]
mod apple;

#[cfg(not(target_os = "android"))]
pub use apple::{
    AppleSwiftConfig, SwiftBridgeCrate, build_apple_bridge, compile_multi_swift, compile_swift,
};

#[cfg(target_os = "android")]
pub use android_runtime::{
    AndroidError, DexHelper, decode_optional_string, decode_string, jvm_and_context,
    with_android_context,
};
