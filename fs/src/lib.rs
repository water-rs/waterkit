//! Cross-platform file system utilities.
//!
//! This crate provides a unified API for accessing common platform directories
//! such as documents and cache folders across iOS, macOS, Android, Windows, and Linux.

/// Platform-specific implementations.
#[cfg(any(target_os = "ios", target_os = "android"))]
mod sys;
#[cfg(target_arch = "wasm32")]
mod web;

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

    /// Reads a file into memory.
    ///
    /// # Errors
    /// Returns an error when the file cannot be read.
    pub async fn read(path: &Path) -> io::Result<Vec<u8>> {
        #[cfg(target_arch = "wasm32")]
        {
            web::read(path).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            async_fs::read(path).await
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
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            let _ = cache_subdir;
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "import_file_to_cache is unavailable on wasm; import bytes via WaterFs::import_bytes_to_cache",
            ));
        }

        #[cfg(not(target_arch = "wasm32"))]
        let file_name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "selected path has no file name",
            )
        })?;
        #[cfg(not(target_arch = "wasm32"))]
        let cache_root = Self::cache_dir().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "cache directory is unavailable")
        })?;
        #[cfg(not(target_arch = "wasm32"))]
        let base_dir = cache_root.join(cache_subdir);
        #[cfg(not(target_arch = "wasm32"))]
        std::fs::create_dir_all(&base_dir)?;
        #[cfg(not(target_arch = "wasm32"))]
        let destination = next_available_cache_path(&base_dir, file_name, Path::exists);
        #[cfg(not(target_arch = "wasm32"))]
        std::fs::copy(path, &destination)?;
        #[cfg(not(target_arch = "wasm32"))]
        Ok(destination)
    }

    /// Imports raw bytes into the application's cache directory subtree.
    ///
    /// If a file with the same name already exists, a numeric suffix is added.
    ///
    /// # Errors
    /// Returns an error when the file name is invalid, the cache directory is
    /// unavailable, or the write operation fails.
    pub async fn import_bytes_to_cache(
        bytes: &[u8],
        file_name: &str,
        cache_subdir: &Path,
    ) -> io::Result<PathBuf> {
        #[cfg(target_arch = "wasm32")]
        {
            web::import_bytes_to_cache(bytes, file_name, cache_subdir).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let file_name = validate_file_name(Path::new(file_name))?;
            let cache_root = Self::cache_dir().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "cache directory is unavailable")
            })?;
            let base_dir = cache_root.join(cache_subdir);
            async_fs::create_dir_all(&base_dir).await?;

            let mut index = 0usize;
            loop {
                let candidate = cache_path_candidate(&base_dir, file_name, index);
                match async_fs::metadata(&candidate).await {
                    Ok(_) => {
                        index += 1;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        async_fs::write(&candidate, bytes).await?;
                        return Ok(candidate);
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
}

fn validate_file_name(path: &Path) -> io::Result<&OsStr> {
    path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "selected path has no file name",
        )
    })
}

fn next_available_cache_path(
    base_dir: &Path,
    file_name: &OsStr,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let mut index = 0usize;
    loop {
        let candidate = cache_path_candidate(base_dir, file_name, index);
        if !exists(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

pub(crate) fn cache_path_candidate(base_dir: &Path, file_name: &OsStr, index: usize) -> PathBuf {
    if index == 0 {
        return base_dir.join(file_name);
    }

    let file_path = Path::new(file_name);
    let stem = file_path
        .file_stem()
        .map_or_else(|| Cow::Borrowed("file"), |stem| stem.to_string_lossy());
    let extension = file_path
        .extension()
        .map(|extension| extension.to_string_lossy().to_string());

    extension.as_ref().map_or_else(
        || base_dir.join(format!("{stem}-{index}")),
        |extension| base_dir.join(format!("{stem}-{index}.{extension}")),
    )
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
