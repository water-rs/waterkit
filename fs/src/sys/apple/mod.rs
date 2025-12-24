//! Apple platform (iOS/macOS) file system implementation using swift-bridge.

use std::path::PathBuf;

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn documents_dir() -> Option<String>;
        fn cache_dir() -> Option<String>;
    }
}

/// Gets the application's documents directory on Apple platforms.
#[must_use]
pub fn documents_dir() -> Option<PathBuf> {
    ffi::documents_dir().map(PathBuf::from)
}

/// Gets the application's cache directory on Apple platforms.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    ffi::cache_dir().map(PathBuf::from)
}
