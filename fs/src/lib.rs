//! Cross-platform file system utilities.
//!
//! This crate provides a unified API for accessing common platform directories
//! such as documents and cache folders across iOS, macOS, Android, Windows, and Linux.

/// Platform-specific implementations.
#[cfg(any(target_os = "ios", target_os = "android"))]
mod sys;

use std::borrow::Cow;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

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

    /// Imports a file into the application's cache directory subtree.
    ///
    /// If a file with the same name already exists, a numeric suffix is added.
    ///
    /// # Errors
    /// Returns an error when the source path has no file name, the cache
    /// directory is unavailable, or the copy operation fails.
    pub fn import_file_to_cache(path: &Path, cache_subdir: &Path) -> io::Result<PathBuf> {
        let file_name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "selected path has no file name",
            )
        })?;
        let cache_root = Self::cache_dir().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "cache directory is unavailable")
        })?;
        let base_dir = cache_root.join(cache_subdir);
        std::fs::create_dir_all(&base_dir)?;

        let mut destination = base_dir.join(file_name);
        if destination.exists() {
            let stem = path
                .file_stem()
                .map_or_else(|| Cow::Borrowed("file"), |s: &OsStr| s.to_string_lossy());
            let extension = path
                .extension()
                .map(|e: &OsStr| e.to_string_lossy().to_string());

            let mut index = 1usize;
            loop {
                let candidate = extension.as_ref().map_or_else(
                    || base_dir.join(format!("{stem}-{index}")),
                    |ext| base_dir.join(format!("{stem}-{index}.{ext}")),
                );
                if !candidate.exists() {
                    destination = candidate;
                    break;
                }
                index += 1;
            }
        }

        std::fs::copy(path, &destination)?;
        Ok(destination)
    }
}

#[cfg(test)]
mod tests {
    use super::WaterFs;

    #[test]
    fn import_file_to_cache_rejects_paths_without_filename() {
        let error = WaterFs::import_file_to_cache(
            std::path::Path::new("/"),
            std::path::Path::new("waterui-tests"),
        )
        .expect_err("root path should not have importable file name");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
