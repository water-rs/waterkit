//! The paths a file write accepts.
//!
//! Every backend that writes files builds one URL per path: Linux with
//! `url`, iOS with `URL(fileURLWithFileSystemRepresentation:)`, Android with
//! `Uri.Builder`. A URL names a file only by its absolute path, and the iOS
//! and Android APIs take the path as a Unicode string.

use std::path::Path;
#[cfg(any(target_os = "ios", target_os = "android", test))]
use std::path::PathBuf;

use crate::error::ClipboardError;

/// The error for writing `path`, which is not absolute.
pub fn not_absolute(path: &Path) -> ClipboardError {
    ClipboardError::Encode(format!(
        "{} is not an absolute path, which a file URL needs",
        path.display()
    ))
}

/// `paths` as the absolute Unicode strings a platform file-URL API takes.
///
/// # Errors
///
/// [`ClipboardError::Encode`] when a path is not absolute or not Unicode.
#[cfg(any(target_os = "ios", target_os = "android", test))]
pub fn unicode_paths(paths: &[PathBuf]) -> Result<Vec<String>, ClipboardError> {
    paths
        .iter()
        .map(|path| {
            if !path.is_absolute() {
                return Err(not_absolute(path));
            }
            path.to_str().map(str::to_owned).ok_or_else(|| {
                ClipboardError::Encode(format!(
                    "{} is not Unicode, which the platform's file URL API needs",
                    path.display()
                ))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::unicode_paths;
    use crate::error::ClipboardError;

    #[test]
    fn keeps_spaces_non_ascii_and_url_delimiters_verbatim() {
        let paths = vec![
            PathBuf::from("/tmp/a file.txt"),
            PathBuf::from("/tmp/n\u{e4}me #1?.txt"),
        ];
        assert_eq!(
            unicode_paths(&paths).unwrap(),
            ["/tmp/a file.txt", "/tmp/n\u{e4}me #1?.txt"]
        );
    }

    #[test]
    fn rejects_a_relative_path() {
        assert!(matches!(
            unicode_paths(&[PathBuf::from("relative.txt")]),
            Err(ClipboardError::Encode(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_path_that_is_not_unicode() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = PathBuf::from(OsStr::from_bytes(b"/tmp/\xff.txt"));
        assert!(matches!(
            unicode_paths(&[path]),
            Err(ClipboardError::Encode(_))
        ));
    }
}
