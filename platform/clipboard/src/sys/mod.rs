//! Platform-specific clipboard backend implementations.

use crate::content::ClipboardEvent;
use crate::error::ClipboardError;
use std::sync::Arc;

// Desktop platforms (Windows, Linux, macOS) use clipboard-rs
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
mod desktop;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub use desktop::ClipboardInner;

// iOS uses Swift bridge
#[cfg(target_os = "ios")]
mod apple;
#[cfg(target_os = "ios")]
pub use apple::ClipboardInner;

// Android uses JNI
#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub use android::ClipboardInner;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::ClipboardInner;

/// Shutdown handle for the clipboard watcher.
pub struct WatcherShutdown {
    inner: ShutdownInner,
}

impl WatcherShutdown {
    /// Stop the clipboard watcher.
    pub fn stop(&self) {
        match &self.inner {
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
            ShutdownInner::Desktop(shutdown) => {
                let shutdown = match shutdown.lock() {
                    Ok(mut guard) => guard.take(),
                    Err(poisoned) => poisoned.into_inner().take(),
                };
                if let Some(s) = shutdown {
                    s.stop();
                }
            }
            #[cfg(target_os = "ios")]
            ShutdownInner::Apple(stop_flag) => {
                stop_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            #[cfg(target_os = "android")]
            ShutdownInner::Android(stop_flag) => {
                stop_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            #[cfg(target_arch = "wasm32")]
            ShutdownInner::Web => {}
        }
    }
}

/// Platform-specific shutdown mechanism.
enum ShutdownInner {
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    Desktop(std::sync::Mutex<Option<clipboard_rs::WatcherShutdown>>),
    #[cfg(target_os = "ios")]
    Apple(Arc<std::sync::atomic::AtomicBool>),
    #[cfg(target_os = "android")]
    Android(Arc<std::sync::atomic::AtomicBool>),
    #[cfg(target_arch = "wasm32")]
    #[expect(
        dead_code,
        reason = "browser clipboard watching returns Unsupported before constructing a shutdown handle"
    )]
    Web,
}

/// Start watching for clipboard changes.
///
/// Returns a receiver that yields `ClipboardEvent`s and a shutdown handle.
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub fn start_watch() -> Result<
    (
        async_channel::Receiver<ClipboardEvent>,
        Arc<WatcherShutdown>,
    ),
    ClipboardError,
> {
    let (receiver, shutdown) = desktop::start_watch()?;
    Ok((
        receiver,
        Arc::new(WatcherShutdown {
            inner: ShutdownInner::Desktop(std::sync::Mutex::new(Some(shutdown))),
        }),
    ))
}

/// Start watching for clipboard changes.
#[cfg(target_arch = "wasm32")]
pub fn start_watch() -> Result<
    (
        async_channel::Receiver<ClipboardEvent>,
        Arc<WatcherShutdown>,
    ),
    ClipboardError,
> {
    Err(ClipboardError::UnsupportedType("watch".into()))
}

/// Start watching for clipboard changes.
#[cfg(target_os = "ios")]
pub fn start_watch() -> Result<
    (
        async_channel::Receiver<ClipboardEvent>,
        Arc<WatcherShutdown>,
    ),
    ClipboardError,
> {
    let (receiver, stop_flag) = apple::start_watch()?;
    Ok((
        receiver,
        Arc::new(WatcherShutdown {
            inner: ShutdownInner::Apple(stop_flag),
        }),
    ))
}

/// Start watching for clipboard changes.
#[cfg(target_os = "android")]
#[allow(
    clippy::unnecessary_wraps,
    reason = "The platform-neutral clipboard watcher API is fallible; Android's polling watcher currently cannot fail during construction."
)]
pub fn start_watch() -> Result<
    (
        async_channel::Receiver<ClipboardEvent>,
        Arc<WatcherShutdown>,
    ),
    ClipboardError,
> {
    let (receiver, stop_flag) = android::start_watch();
    Ok((
        receiver,
        Arc::new(WatcherShutdown {
            inner: ShutdownInner::Android(stop_flag),
        }),
    ))
}
