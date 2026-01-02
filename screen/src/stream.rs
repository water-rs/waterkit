//! GPU screen capture streaming.

use crate::{Error, ScreenInfo, frame::ScreenFrame, sys};
use std::sync::Arc;
use wgpu::{Device, Queue};

/// Screen capture configuration.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Target frames per second (hint, may not be achieved).
    pub target_fps: u32,
    /// Whether to show cursor in capture.
    pub show_cursor: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            target_fps: 60,
            show_cursor: true,
        }
    }
}

/// Continuous GPU screen capture stream.
///
/// Provides streaming screen capture with GPU texture output.
/// Frames are delivered as [`ScreenFrame`] with zero-copy on supported platforms.
pub struct ScreenStream {
    inner: sys::ScreenStreamInner,
    #[allow(dead_code)]
    device: Arc<Device>,
    #[allow(dead_code)]
    queue: Arc<Queue>,
}

impl std::fmt::Debug for ScreenStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenStream").finish_non_exhaustive()
    }
}

impl ScreenStream {
    /// Start a new screen capture stream for the specified display.
    ///
    /// # Arguments
    ///
    /// * `display` - The display to capture.
    /// * `device` - wgpu device (caller-provided).
    /// * `queue` - wgpu queue (caller-provided).
    /// * `config` - Capture configuration.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Platform`] if capture cannot be started.
    pub fn start(
        display: &ScreenInfo,
        device: Arc<Device>,
        queue: Arc<Queue>,
        config: &StreamConfig,
    ) -> Result<Self, Error> {
        let inner = sys::ScreenStreamInner::new(display, device.clone(), queue.clone(), config)?;
        Ok(Self {
            inner,
            device,
            queue,
        })
    }

    /// Receive the next frame asynchronously.
    ///
    /// Returns `None` if the stream has ended.
    pub async fn next_frame(&self) -> Option<ScreenFrame> {
        self.inner.next_frame().await
    }

    /// Try to receive a frame without blocking.
    ///
    /// Returns `None` if no frame is currently available.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // Not const on all platforms
    pub fn try_next_frame(&self) -> Option<ScreenFrame> {
        self.inner.try_next_frame()
    }

    /// Get the current dimensions of the capture.
    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        self.inner.dimensions()
    }
}
