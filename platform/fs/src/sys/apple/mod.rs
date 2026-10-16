//! Apple platform (iOS/macOS) file system implementation backed by
//! Foundation's `NSFileManager` search-path lookup.

use std::path::PathBuf;

use objc2_foundation::{NSFileManager, NSSearchPathDirectory, NSSearchPathDomainMask};

/// The first user-domain URL Foundation reports for `directory`, mirroring
/// `FileManager.default.urls(for:in:).first?.path`.
fn search_path(directory: NSSearchPathDirectory) -> Option<PathBuf> {
    NSFileManager::defaultManager()
        .URLsForDirectory_inDomains(directory, NSSearchPathDomainMask::UserDomainMask)
        .firstObject()
        .and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
}

/// Gets the application's documents directory on Apple platforms.
#[must_use]
pub fn documents_dir() -> Option<PathBuf> {
    search_path(NSSearchPathDirectory::DocumentDirectory)
}

/// Gets the application's cache directory on Apple platforms.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    search_path(NSSearchPathDirectory::CachesDirectory)
}
