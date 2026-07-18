use std::{num::NonZeroUsize, path::PathBuf};

use async_fs::File;
use futures::{AsyncWriteExt as _, StreamExt as _};
use url::Url;
use uuid::Uuid;
use waterkit_video_core::Error;
use zenwave::{Client as _, Method, redirect::FollowRedirect};

use crate::atomic;

/// Parameters for one progressive HTTP media download.
#[derive(Debug, Clone)]
pub struct ProgressiveDownloadRequest {
    url: Url,
    destination: PathBuf,
    commit_destination: Option<PathBuf>,
    progress_quantum: NonZeroUsize,
}

impl ProgressiveDownloadRequest {
    /// Creates a request with an explicit progress reporting quantum.
    #[must_use]
    pub const fn new(url: Url, destination: PathBuf, progress_quantum: NonZeroUsize) -> Self {
        Self {
            url,
            destination,
            commit_destination: None,
            progress_quantum,
        }
    }

    /// Creates a cache download whose growing file is private and whose final
    /// destination appears atomically only after a complete transfer.
    ///
    /// # Errors
    ///
    /// Returns a streaming error when the final destination has no parent or
    /// file name.
    pub fn new_cached(
        url: Url,
        commit_destination: PathBuf,
        progress_quantum: NonZeroUsize,
    ) -> Result<Self, Error> {
        let parent = commit_destination.parent().ok_or_else(|| {
            Error::Streaming(String::from("cached download destination has no parent"))
        })?;
        let file_name = commit_destination
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                Error::Streaming(String::from(
                    "cached download destination must have a UTF-8 file name",
                ))
            })?;
        let destination = parent.join(format!(".{file_name}.{}.part", Uuid::new_v4()));
        Ok(Self {
            url,
            destination,
            commit_destination: Some(commit_destination),
            progress_quantum,
        })
    }

    /// Returns the remote media URL.
    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }

    /// Returns the local destination path.
    #[must_use]
    pub fn destination(&self) -> &std::path::Path {
        &self.destination
    }

    /// Returns the atomically committed destination for a cache download.
    #[must_use]
    pub fn commit_destination(&self) -> Option<&std::path::Path> {
        self.commit_destination.as_deref()
    }

    /// Returns the minimum byte delta between progress callbacks.
    #[must_use]
    pub const fn progress_quantum(&self) -> NonZeroUsize {
        self.progress_quantum
    }
}

/// Byte progress for a network media transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    /// Bytes written and flushed to the growing destination file.
    pub bytes_written: usize,
    /// HTTP content length when supplied by the origin.
    pub total_bytes: Option<usize>,
}

/// Observable lifecycle event for a progressive download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadEvent {
    /// The response has been accepted and the destination file is open.
    Started(DownloadProgress),
    /// At least one configured progress quantum has been written.
    Progress(DownloadProgress),
    /// The response body is complete and the destination has been flushed.
    Finished(DownloadProgress),
}

/// Result of a completed progressive download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadReceipt {
    destination: PathBuf,
    bytes_written: usize,
    total_bytes: Option<usize>,
}

impl DownloadReceipt {
    /// Returns the completed local asset path.
    #[must_use]
    pub fn destination(&self) -> &std::path::Path {
        &self.destination
    }

    /// Returns the number of bytes written.
    #[must_use]
    pub const fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    /// Returns the origin content length when supplied.
    #[must_use]
    pub const fn total_bytes(&self) -> Option<usize> {
        self.total_bytes
    }
}

/// Downloads one HTTP media object with Zenwave.
///
/// The destination remains visible while it grows so a container-aware caller
/// can probe progressive readiness. On any failure the incomplete destination
/// is removed; callers never mistake failed partial data for cached media.
///
/// # Errors
///
/// Returns a streaming error for request, response, body, or destination I/O
/// failures. Non-success HTTP status codes fail before a destination is created.
pub async fn download(
    request: ProgressiveDownloadRequest,
    mut observe: impl FnMut(DownloadEvent),
) -> Result<DownloadReceipt, Error> {
    let mut client = FollowRedirect::new(zenwave::raw_client());
    let response = client
        .method(Method::GET, request.url.as_str())
        .map_err(|error| Error::Streaming(error.to_string()))?
        .await
        .map_err(|error| Error::Streaming(error.to_string()))?;
    if !response.status().is_success() {
        return Err(Error::Streaming(format!(
            "GET {} returned HTTP {}",
            request.url,
            response.status()
        )));
    }

    let total_bytes = response
        .headers()
        .get("content-length")
        .map(|value| {
            value
                .to_str()
                .map_err(|error| Error::Streaming(format!("invalid Content-Length: {error}")))?
                .parse::<usize>()
                .map_err(|error| Error::Streaming(format!("invalid Content-Length: {error}")))
        })
        .transpose()?;
    let Some(parent) = request.destination.parent() else {
        return Err(Error::Streaming(String::from(
            "progressive download destination has no parent directory",
        )));
    };
    async_fs::create_dir_all(parent)
        .await
        .map_err(|error| Error::Streaming(error.to_string()))?;
    let mut file = File::create(&request.destination)
        .await
        .map_err(|error| Error::Streaming(error.to_string()))?;
    let initial = DownloadProgress {
        bytes_written: 0,
        total_bytes,
    };
    observe(DownloadEvent::Started(initial));

    let result = async {
        let mut body = response.into_body();
        let mut bytes_written = 0usize;
        let mut last_reported = 0usize;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|error| Error::Streaming(error.to_string()))?;
            file.write_all(&chunk)
                .await
                .map_err(|error| Error::Streaming(error.to_string()))?;
            bytes_written = bytes_written.checked_add(chunk.len()).ok_or_else(|| {
                Error::Streaming(String::from("progressive download length overflowed usize"))
            })?;
            if bytes_written.saturating_sub(last_reported) >= request.progress_quantum.get() {
                file.flush()
                    .await
                    .map_err(|error| Error::Streaming(error.to_string()))?;
                last_reported = bytes_written;
                observe(DownloadEvent::Progress(DownloadProgress {
                    bytes_written,
                    total_bytes,
                }));
            }
        }
        file.sync_all()
            .await
            .map_err(|error| Error::Streaming(error.to_string()))?;
        drop(file);
        let destination = if let Some(commit_destination) = &request.commit_destination {
            atomic::replace(request.destination.clone(), commit_destination.clone()).await?;
            commit_destination.clone()
        } else {
            request.destination.clone()
        };
        let progress = DownloadProgress {
            bytes_written,
            total_bytes,
        };
        observe(DownloadEvent::Finished(progress));
        Ok(DownloadReceipt {
            destination,
            bytes_written,
            total_bytes,
        })
    }
    .await;

    if result.is_err() {
        let _ = async_fs::remove_file(&request.destination).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroUsize, path::Path};

    use url::Url;

    use super::ProgressiveDownloadRequest;

    #[test]
    fn cached_request_keeps_growing_file_private_until_commit() {
        let final_path = Path::new("media-cache").join("trailer.mp4");
        let request = ProgressiveDownloadRequest::new_cached(
            Url::parse("https://waterui.dev/assets/trailer.mp4").expect("test URL must be valid"),
            final_path.clone(),
            NonZeroUsize::new(1024).expect("test progress quantum must be non-zero"),
        )
        .expect("cached request must be valid");

        assert_ne!(request.destination(), final_path);
        assert_eq!(request.commit_destination(), Some(final_path.as_path()));
        assert_eq!(
            request
                .destination()
                .extension()
                .and_then(|value| value.to_str()),
            Some("part")
        );
    }
}
