//! On-device translation through the operating system's translation service.
//!
//! Apple platforms require iOS or macOS 26 and the Xcode 26 SDK. When
//! translation assets are missing, they are reported as needing download;
//! presenting the consent and download sheet is the responsibility of `WaterUI`
//! ([water-rs/waterui#1879]).
//!
//! Android requires API 31 or later and a system translation service, such as
//! Android System Intelligence on Pixel devices. Windows, Linux, and WebAssembly
//! do not currently provide an implementation.
//!
//! Language identification and entity extraction are planned as sibling
//! modules for water-rs/waterkit#160.
//!
//! [water-rs/waterui#1879]: https://github.com/water-rs/waterui/issues/1879

mod sys;
pub mod translation;

pub use icu_locale_core::{LanguageIdentifier, langid};
