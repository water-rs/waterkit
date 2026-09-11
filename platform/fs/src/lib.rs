//! Cross-platform file system utilities.
//!
//! Provides a uniform API for the directories every app needs (documents,
//! cache, local data) and for cache-imported user files. Every fallible
//! call returns [`FsError`] so callers see typed reasons (no documents
//! dir on this platform, missing file name, unsupported on wasm, ...) on
//! top of an embedded `std::io::Error`.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

#[cfg(any(target_os = "ios", target_os = "android"))]
mod sys;
#[cfg(target_arch = "wasm32")]
mod web;

/// Android-specific JNI helpers that require an explicit `Context`.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::{cache_dir_with_context, documents_dir_with_context};
}

use std::borrow::Cow;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

/// Errors returned by the `waterkit-fs` API.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FsError {
    /// Underlying I/O failure.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The platform exposes no documents directory.
    #[error("documents directory is unavailable on this platform")]
    NoDocumentsDir,
    /// The platform exposes no cache directory.
    #[error("cache directory is unavailable on this platform")]
    NoCacheDir,
    /// The platform exposes no local-data directory.
    #[error("local data directory is unavailable on this platform")]
    NoDataLocalDir,
    /// Selected path has no file name component.
    #[error("path has no file name")]
    NoFileName,
    /// The requested operation is unsupported on the current platform.
    #[error("unsupported on this platform: {0}")]
    Unsupported(&'static str),
    /// JSON encoding or decoding failed.
    #[error("json {operation}: {source}")]
    Json {
        /// Whether the failure came from encoding or decoding.
        operation: &'static str,
        /// Underlying serde error.
        source: serde_json::Error,
    },
}

/// Convenience alias.
pub type Result<T, E = FsError> = core::result::Result<T, E>;

/// Cross-platform file system utilities.
#[derive(Debug, Clone, Copy, Default)]
pub struct WaterFs;

impl WaterFs {
    /// Returns the application's documents directory from an Android `Context`.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::NoDocumentsDir`] when the JNI call returns no
    /// path.
    #[cfg(target_os = "android")]
    pub fn documents_dir_with_context(
        env: &mut jni::Env<'_>,
        context: &jni::objects::JObject,
    ) -> Result<PathBuf> {
        sys::documents_dir_with_context(env, context).ok_or(FsError::NoDocumentsDir)
    }

    /// Returns the application's cache directory from an Android `Context`.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::NoCacheDir`] when the JNI call returns no path.
    #[cfg(target_os = "android")]
    pub fn cache_dir_with_context(
        env: &mut jni::Env<'_>,
        context: &jni::objects::JObject,
    ) -> Result<PathBuf> {
        sys::cache_dir_with_context(env, context).ok_or(FsError::NoCacheDir)
    }

    /// Returns the application's documents directory.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::NoDocumentsDir`] on platforms / OS configs
    /// where no documents directory is exposed.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::missing_const_for_fn,
            reason = "the wasm body is a constant `Err`; the native bodies do real I/O"
        )
    )]
    pub fn documents_dir() -> Result<PathBuf> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            dirs::document_dir().ok_or(FsError::NoDocumentsDir)
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            sys::documents_dir().ok_or(FsError::NoDocumentsDir)
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "ios",
            target_os = "android"
        )))]
        {
            Err(FsError::NoDocumentsDir)
        }
    }

    /// Returns the application's cache directory.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::NoCacheDir`] on platforms / OS configs where no
    /// cache directory is exposed.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::missing_const_for_fn,
            reason = "the wasm body is a constant `Err`; the native bodies do real I/O"
        )
    )]
    pub fn cache_dir() -> Result<PathBuf> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            dirs::cache_dir().ok_or(FsError::NoCacheDir)
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            sys::cache_dir().ok_or(FsError::NoCacheDir)
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "ios",
            target_os = "android"
        )))]
        {
            Err(FsError::NoCacheDir)
        }
    }

    /// Reads a file into memory.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] on read failure.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "the wasm backend awaits IndexedDB handles, which are bound to the browser thread"
        )
    )]
    pub async fn read(path: &Path) -> Result<Vec<u8>> {
        #[cfg(target_arch = "wasm32")]
        {
            web::read(path).await.map_err(FsError::Io)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            async_fs::read(path).await.map_err(FsError::Io)
        }
    }

    /// Resolves a path under the application's local data directory.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::NoDataLocalDir`] on platforms / OS configs
    /// where no local data directory is exposed.
    pub fn data_local_path(path: impl AsRef<Path>) -> Result<PathBuf> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            dirs::data_local_dir()
                .map(|root| root.join(path))
                .ok_or(FsError::NoDataLocalDir)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = path;
            Err(FsError::NoDataLocalDir)
        }
    }

    /// Loads a JSON store, returning `T::default()` when the file is
    /// absent or empty.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] for filesystem errors other than
    /// `NotFound`, or [`FsError::Json`] for malformed content.
    pub fn load_json_store<T>(path: &Path) -> Result<T>
    where
        T: Default + DeserializeOwned,
    {
        match std::fs::read(path) {
            Ok(bytes) if bytes.is_empty() => Ok(T::default()),
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| FsError::Json {
                operation: "decode",
                source,
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
            Err(error) => Err(FsError::Io(error)),
        }
    }

    /// Writes a JSON store, creating parent directories as needed.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if the write fails, or [`FsError::Json`]
    /// for serialization failures.
    pub fn write_json_store<T>(path: &Path, value: &T) -> Result<()>
    where
        T: Serialize,
    {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(value).map_err(|source| FsError::Json {
            operation: "encode",
            source,
        })?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// Imports a file into the application's cache directory subtree.
    ///
    /// If a file with the same name already exists, a numeric suffix is
    /// added.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::NoFileName`] when `path` has no file name,
    /// [`FsError::NoCacheDir`] when the platform has no cache directory,
    /// [`FsError::Unsupported`] on wasm, or [`FsError::Io`] for the copy.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::missing_const_for_fn,
            reason = "the wasm body is a constant `Err`; the native bodies do real I/O"
        )
    )]
    pub fn import_file_to_cache(path: &Path, cache_subdir: &Path) -> Result<PathBuf> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (path, cache_subdir);
            Err(FsError::Unsupported(
                "import_file_to_cache: use WaterFs::import_bytes_to_cache on wasm",
            ))
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let file_name = path.file_name().ok_or(FsError::NoFileName)?;
            let base_dir = Self::cache_dir()?.join(cache_subdir);
            std::fs::create_dir_all(&base_dir)?;
            let destination = next_available_cache_path(&base_dir, file_name, Path::exists);
            std::fs::copy(path, &destination)?;
            Ok(destination)
        }
    }

    /// Imports raw bytes into the application's cache directory subtree.
    ///
    /// If a file with the same name already exists, a numeric suffix is
    /// added.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::NoFileName`] when `file_name` is invalid,
    /// [`FsError::NoCacheDir`] when no cache directory is available, or
    /// [`FsError::Io`] for write failures.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "the wasm backend awaits IndexedDB handles, which are bound to the browser thread"
        )
    )]
    pub async fn import_bytes_to_cache(
        bytes: &[u8],
        file_name: &str,
        cache_subdir: &Path,
    ) -> Result<PathBuf> {
        #[cfg(target_arch = "wasm32")]
        {
            web::import_bytes_to_cache(bytes, file_name, cache_subdir)
                .await
                .map_err(FsError::Io)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let file_name = validate_file_name(Path::new(file_name))?;
            let base_dir = Self::cache_dir()?.join(cache_subdir);
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
                    Err(error) => return Err(FsError::Io(error)),
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn validate_file_name(path: &Path) -> Result<&OsStr> {
    path.file_name().ok_or(FsError::NoFileName)
}

#[cfg(not(target_arch = "wasm32"))]
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
    use super::{FsError, WaterFs};

    #[test]
    fn import_file_to_cache_rejects_paths_without_filename() {
        let error = WaterFs::import_file_to_cache(
            std::path::Path::new("/"),
            std::path::Path::new("waterui-tests"),
        )
        .expect_err("root path should not have importable file name");
        assert!(matches!(error, FsError::NoFileName));
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
