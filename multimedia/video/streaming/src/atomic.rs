use std::path::PathBuf;

#[cfg(feature = "offline")]
use std::ffi::OsStr;
#[cfg(any(feature = "offline", target_os = "windows"))]
use std::path::Path;

use waterkit_video_core::Error;

#[cfg(feature = "offline")]
pub fn is_partial_file_name(name: &OsStr) -> bool {
    let path = Path::new(name);
    name.to_str().is_some_and(|name| name.starts_with('.'))
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("part"))
}

#[cfg(target_os = "windows")]
pub async fn replace(source: PathBuf, destination: PathBuf) -> Result<(), Error> {
    blocking::unblock(move || replace_windows(&source, &destination)).await
}

#[cfg(not(target_os = "windows"))]
pub async fn replace(source: PathBuf, destination: PathBuf) -> Result<(), Error> {
    async_fs::rename(&source, &destination)
        .await
        .map_err(|error| {
            Error::Streaming(format!(
                "failed to atomically replace {} with {}: {error}",
                destination.display(),
                source.display()
            ))
        })?;
    let parent = destination
        .parent()
        .ok_or_else(|| Error::Streaming(String::from("atomic media destination has no parent")))?;
    sync_directory(parent.to_path_buf()).await
}

#[cfg(not(target_os = "windows"))]
async fn sync_directory(directory: PathBuf) -> Result<(), Error> {
    blocking::unblock(move || {
        std::fs::File::open(&directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                Error::Streaming(format!(
                    "failed to synchronize media cache directory {}: {error}",
                    directory.display()
                ))
            })
    })
    .await
}

#[cfg(target_os = "windows")]
fn replace_windows(source: &Path, destination: &Path) -> Result<(), Error> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::PCWSTR,
    };

    // A plain path given to `MoveFileExW` is limited to MAX_PATH (260 UTF-16
    // units), and a cache object's partial-file name alone is 177 characters,
    // so ordinary cache roots exceed it. `std::fs` switches to the verbatim
    // `\\?\` form internally, which is how the partial file was created; this
    // call has to be handed that form explicitly. Both parents exist (the
    // partial file was just written into the destination's directory), so
    // `canonicalize` produces it.
    let verbatim = |path: &Path| {
        std::fs::canonicalize(path).map_err(|error| {
            Error::Streaming(format!(
                "failed to resolve media cache path {}: {error}",
                path.display()
            ))
        })
    };
    let source = verbatim(source)?;
    let destination = match (destination.parent(), destination.file_name()) {
        (Some(parent), Some(file_name)) => verbatim(parent)?.join(file_name),
        _ => {
            return Err(Error::Streaming(format!(
                "atomic media destination {} has no parent directory or file name",
                destination.display()
            )));
        }
    };

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| {
        Error::Streaming(format!(
            "failed to atomically replace {} with {}: {error}",
            destination.display(),
            source.display()
        ))
    })
}
