//! Graceful shutdown utilities for background tasks.
//!
//! Provides a channel-based shutdown mechanism that signals via channel close,
//! allowing background tasks to detect shutdown without polling atomic flags.

use async_channel::{Receiver, Sender};

/// A handle that signals shutdown when dropped.
///
/// When this handle is dropped or [`shutdown()`](Self::shutdown) is called,
/// the associated [`ShutdownReceiver`] will detect the shutdown signal.
///
/// # Example
///
/// ```ignore
/// let (handle, receiver) = ShutdownHandle::new();
///
/// std::thread::spawn(move || {
///     while !receiver.is_shutdown() {
///         // Do work...
///         std::thread::sleep(Duration::from_millis(50));
///     }
/// });
///
/// // When handle is dropped, the background thread exits
/// drop(handle);
/// ```
#[derive(Debug)]
pub struct ShutdownHandle {
    sender: Sender<()>,
}

impl ShutdownHandle {
    /// Create a new shutdown handle and receiver pair.
    #[must_use]
    pub fn new() -> (Self, ShutdownReceiver) {
        let (sender, receiver) = async_channel::bounded(1);
        (Self { sender }, ShutdownReceiver { receiver })
    }

    /// Explicitly signal shutdown.
    ///
    /// This is automatically called on drop, but can be called earlier
    /// if explicit shutdown timing is needed.
    pub fn shutdown(&self) {
        self.sender.close();
    }
}

impl Default for ShutdownHandle {
    fn default() -> Self {
        Self::new().0
    }
}

impl Drop for ShutdownHandle {
    fn drop(&mut self) {
        self.sender.close();
    }
}

/// Receiver that background tasks use to detect shutdown.
///
/// Check [`is_shutdown()`](Self::is_shutdown) periodically or use
/// [`wait()`](Self::wait) / [`wait_blocking()`](Self::wait_blocking)
/// to block until shutdown is signaled.
#[derive(Debug, Clone)]
pub struct ShutdownReceiver {
    receiver: Receiver<()>,
}

impl ShutdownReceiver {
    /// Check if shutdown was signaled (non-blocking).
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        self.receiver.is_closed()
    }

    /// Wait for shutdown signal (async).
    ///
    /// Returns when the [`ShutdownHandle`] is dropped or
    /// [`shutdown()`](ShutdownHandle::shutdown) is called.
    pub async fn wait(&self) {
        // When sender is dropped/closed, recv will error with Closed
        let _ = self.receiver.recv().await;
    }

    /// Wait for shutdown signal (blocking).
    ///
    /// Use this in non-async contexts (e.g., background threads).
    pub fn wait_blocking(&self) {
        let _ = self.receiver.recv_blocking();
    }
}
