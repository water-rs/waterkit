//! Clipboard event stream.

use crate::content::ClipboardEvent;
use crate::sys::WatcherShutdown;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// A stream of clipboard change events.
///
/// This stream yields [`ClipboardEvent`]s whenever the clipboard content changes.
/// The stream will continue until the clipboard watcher is stopped or an error occurs.
///
/// # Example
///
/// ```no_run
/// use futures::StreamExt;
/// use waterkit_clipboard::Clipboard;
///
/// # async fn example() -> Result<(), waterkit_clipboard::ClipboardError> {
/// let clipboard = Clipboard::new()?;
/// let mut stream = clipboard.watch()?;
///
/// while let Some(event) = stream.next().await {
///     println!("Clipboard changed! has_text={}", event.has_text);
/// }
/// # Ok(())
/// # }
/// ```
pub struct ClipboardStream {
    receiver: Pin<Box<async_channel::Receiver<ClipboardEvent>>>,
    shutdown: Arc<WatcherShutdown>,
}

impl ClipboardStream {
    /// Create a new clipboard stream from a receiver and shutdown handle.
    pub(crate) fn new(
        receiver: async_channel::Receiver<ClipboardEvent>,
        shutdown: Arc<WatcherShutdown>,
    ) -> Self {
        Self {
            receiver: Box::pin(receiver),
            shutdown,
        }
    }

    /// Stop watching for clipboard changes.
    ///
    /// After calling this, the stream will terminate.
    pub fn stop(&self) {
        self.shutdown.stop();
    }
}

impl Stream for ClipboardStream {
    type Item = ClipboardEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.as_mut().poll_next(cx)
    }
}

impl std::fmt::Debug for ClipboardStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardStream").finish_non_exhaustive()
    }
}
