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

use serde::Serialize;
use serde::de::DeserializeOwned;

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

    /// Resolves a path under the application's local data directory.
    ///
    /// # Errors
    /// Returns an error when the platform cannot provide a local data directory.
    pub fn data_local_path(path: impl AsRef<Path>) -> io::Result<PathBuf> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            dirs::data_local_dir()
                .map(|root| root.join(path))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "local data directory is unavailable",
                    )
                })
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = path;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "local data directory is unavailable on this platform",
            ))
        }
    }

    /// Loads a JSON store, defaulting to `T::default()` when the file is absent or empty.
    ///
    /// # Errors
    /// Returns an error when reading or decoding the store fails.
    pub fn load_json_store<T>(path: &Path) -> io::Result<T>
    where
        T: Default + DeserializeOwned,
    {
        match std::fs::read(path) {
            Ok(bytes) if bytes.is_empty() => Ok(T::default()),
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
            Err(error) => Err(error),
        }
    }

    /// Writes a JSON store, creating parent directories as needed.
    ///
    /// # Errors
    /// Returns an error when serialization or writing fails.
    pub fn write_json_store<T>(path: &Path, value: &T) -> io::Result<()>
    where
        T: Serialize,
    {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let bytes = serde_json::to_vec_pretty(value)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        std::fs::write(path, bytes)
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

    #[test]
    fn load_json_store_defaults_on_missing_file() {
        #[derive(Default, serde::Deserialize)]
        struct TestStore {
            value: u32,
        }

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("missing.json");
        let store = WaterFs::load_json_store::<TestStore>(&path).expect("missing store defaults");
        assert_eq!(store.value, 0);
    }
}
