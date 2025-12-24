//! Cross-platform file system utilities.
//!
//! This crate provides a unified API for accessing common platform directories
//! such as documents and cache folders across iOS, macOS, Android, Windows, and Linux.

/// Platform-specific implementations.
pub mod sys;

use std::path::PathBuf;

/// Cross-platform File System Utilities
///
/// This struct provides access to file system operations like finding sandbox paths.
#[derive(Debug, Clone, Copy, Default)]
pub struct WaterFs;

impl WaterFs {
    /// Gets the application's documents directory.
    #[must_use]
    pub fn documents_dir() -> Option<PathBuf> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            dirs::document_dir()
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            sys::documents_dir()
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "ios",
            target_os = "android"
        )))]
        {
            None
        }
    }

    /// Gets the application's cache directory.
    #[must_use]
    pub fn cache_dir() -> Option<PathBuf> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            dirs::cache_dir()
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            sys::cache_dir()
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "ios",
            target_os = "android"
        )))]
        {
            None
        }
    }
}
