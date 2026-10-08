//! Platform-specific clipboard backend implementations.

use crate::content::ClipboardEvent;
use crate::error::ClipboardError;
use std::sync::{Arc, Mutex};

// The paths a file write accepts, for every backend that builds file URLs
#[cfg(any(
    target_os = "ios",
    target_os = "android",
    target_os = "linux",
    all(test, unix)
))]
mod file_path;

// Windows and macOS use clipboard-rs
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod desktop;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use desktop::ClipboardInner;

// Linux chooses Wayland (data-control) or X11 per session, for CLIPBOARD and
// for the PRIMARY selection only Linux has
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{ClipboardInner, PrimaryInner};

// iOS uses UIPasteboard through objc2
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
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::missing_const_for_fn,
            reason = "the browser's only arm is the never-constructed `Web` shim; every other platform stops its watcher at run time"
        )
    )]
    pub fn stop(&self) {
        match &self.inner {
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            ShutdownInner::Desktop(shutdown) => {
                if let Some(shutdown) = take(shutdown) {
                    shutdown.stop();
                }
            }
            // Dropping the guard stops the watch.
            #[cfg(target_os = "linux")]
            ShutdownInner::Linux(guard) => drop(take(guard)),
            // Dropping the guard removes the watch's observers.
            #[cfg(target_os = "ios")]
            ShutdownInner::Apple(guard) => drop(take(guard)),
            #[cfg(target_os = "android")]
            ShutdownInner::Android(session) => {
                if let Some(session) = take(session) {
                    session.stop();
                }
            }
            #[cfg(target_arch = "wasm32")]
            ShutdownInner::Web => {}
        }
    }
}

/// Take the stopper out of `slot`, so that only the first stop runs it.
#[cfg_attr(
    target_arch = "wasm32",
    expect(dead_code, reason = "the browser watcher stops without a slot")
)]
fn take<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    match slot.lock() {
        Ok(mut guard) => guard.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    }
}

/// Platform-specific shutdown mechanism.
enum ShutdownInner {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    Desktop(Mutex<Option<clipboard_rs::WatcherShutdown>>),
    #[cfg(target_os = "linux")]
    Linux(Mutex<Option<linux::WatchGuard>>),
    #[cfg(target_os = "ios")]
    Apple(Mutex<Option<apple::AppleWatchGuard>>),
    #[cfg(target_os = "android")]
    Android(Mutex<Option<android::WatchSession>>),
    #[cfg(target_arch = "wasm32")]
    #[expect(
        dead_code,
        reason = "browser clipboard watching returns Unsupported before constructing a shutdown handle"
    )]
    Web,
}

/// Receiver of a watch's events, and the handle that stops it.
type Watch = (
    async_channel::Receiver<ClipboardEvent>,
    Arc<WatcherShutdown>,
);

/// Start watching for clipboard changes.
///
/// Returns a receiver that yields `ClipboardEvent`s and a shutdown handle.
#[cfg(any(target_os = "windows", target_os = "macos"))]
#[expect(
    clippy::unused_async,
    reason = "the facade calls every platform's backend through the same async signature; this backend registers its watcher synchronously"
)]
pub async fn start_watch(_clipboard: &ClipboardInner) -> Result<Watch, ClipboardError> {
    let (receiver, shutdown) = desktop::start_watch()?;
    Ok((
        receiver,
        Arc::new(WatcherShutdown {
            inner: ShutdownInner::Desktop(Mutex::new(Some(shutdown))),
        }),
    ))
}

/// Start watching for clipboard changes on the display server `clipboard`
/// chose.
#[cfg(target_os = "linux")]
#[expect(
    clippy::unused_async,
    reason = "the facade calls every platform's backend through the same async signature; this backend registers its watcher synchronously"
)]
pub async fn start_watch(clipboard: &ClipboardInner) -> Result<Watch, ClipboardError> {
    let (receiver, guard) = clipboard.watch()?;
    Ok((
        receiver,
        Arc::new(WatcherShutdown {
            inner: ShutdownInner::Linux(Mutex::new(Some(guard))),
        }),
    ))
}

/// Start watching for clipboard changes.
#[cfg(target_arch = "wasm32")]
#[expect(
    clippy::unused_async,
    reason = "the facade calls every platform's backend through the same async signature; this shim has nothing to await"
)]
pub async fn start_watch(_clipboard: &ClipboardInner) -> Result<Watch, ClipboardError> {
    Err(ClipboardError::UnsupportedType("watch".into()))
}

/// Start watching for clipboard changes.
#[cfg(target_os = "ios")]
pub async fn start_watch(_clipboard: &ClipboardInner) -> Result<Watch, ClipboardError> {
    let (receiver, guard) = apple::start_watch().await?;
    Ok((
        receiver,
        Arc::new(WatcherShutdown {
            inner: ShutdownInner::Apple(Mutex::new(Some(guard))),
        }),
    ))
}

/// Start watching for clipboard changes.
#[cfg(target_os = "android")]
#[expect(
    clippy::unused_async,
    reason = "the facade calls every platform's backend through the same async signature; this backend registers its watcher synchronously"
)]
pub async fn start_watch(_clipboard: &ClipboardInner) -> Result<Watch, ClipboardError> {
    let (receiver, session) = android::start_watch()?;
    Ok((
        receiver,
        Arc::new(WatcherShutdown {
            inner: ShutdownInner::Android(Mutex::new(Some(session))),
        }),
    ))
}
