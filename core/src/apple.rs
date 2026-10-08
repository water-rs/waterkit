//! Main-thread access for the Apple realizations.
//!
//! `AppKit`, `UIKit` and most framework objects the capability crates drive
//! may only be touched on the main thread. [`on_main`] is the one way those
//! crates get there: it never blocks the calling thread, so it is safe to
//! await from any executor, including one the main thread itself is driving.

use dispatch2::DispatchQueue;
use futures::channel::oneshot;
use objc2::MainThreadMarker;

/// Runs `work` on the main thread and returns its result.
///
/// On the main thread `work` runs inline. Elsewhere it is dispatched
/// asynchronously to the main queue and the returned future resolves once it
/// has run. The closure is `'static` because the dispatched block can outlive
/// a dropped future; state it needs crosses as owned values, such as an
/// `Arc<MainThreadBound<Retained<T>>>`.
///
/// # Panics
///
/// Panics if the main queue drops the dispatched block without running it,
/// which libdispatch never does.
pub async fn on_main<R, F>(work: F) -> R
where
    F: FnOnce(MainThreadMarker) -> R + Send + 'static,
    R: Send + 'static,
{
    if let Some(mtm) = MainThreadMarker::new() {
        return work(mtm);
    }
    let (sender, receiver) = oneshot::channel();
    DispatchQueue::main().exec_async(move || {
        let mtm = MainThreadMarker::new().expect("the main queue runs on the main thread");
        // The caller may have dropped the future; its result is then unwanted.
        let _ = sender.send(work(mtm));
    });
    receiver
        .await
        .expect("the main queue runs every block dispatched to it")
}
