//! Cross-thread reactive state.
//!
//! [`Subscribed<T>`] is a read-only view of a value that may be updated from
//! any thread. It is a thin wrapper around `nami::Binding<T>`:
//!
//! - Readers on the binding's home thread observe via [`Subscribed::get`],
//!   [`Subscribed::watch`], or [`Subscribed::stream`] — the type implements
//!   `nami::Signal<Output = T>` directly, so any nami combinator (`map`,
//!   `zip`, `distinct`, `cached`, …) composes natively.
//! - Producers on any thread push values through [`SubscribedSink<T>`], a
//!   thin wrapper over `nami::binding::BindingMailbox<T>`. The sink is
//!   `Send + Sync + Clone`; calling [`SubscribedSink::set`] enqueues an
//!   `FnOnce(&mut Binding<T>)` closure that the home-thread mailbox worker
//!   drains and executes.
//!
//! ## Threading model
//!
//! The home thread is the thread that calls [`subscribed`]. A
//! `LocalExecutor` (from `executor-core`) must be polling on that thread for
//! the mailbox worker to drain pushes. Inside a waterui app, the UI thread
//! satisfies this automatically. Headless tests can pass any
//! `LocalExecutor` to [`subscribed_with_executor`].
//!
//! `Subscribed<T>` itself is `Clone + !Send` because the underlying
//! `Binding<T>` is `Rc<…>`. To hand a sink to a background thread, use the
//! `SubscribedSink<T>` returned alongside it.

use alloc::sync::Arc;
use core::fmt;
use executor_core::LocalExecutor;
use nami::binding::BindingMailbox;
use nami::stream::SignalStream;
use nami::watcher::Context as WatcherContext;
use nami::{Binding, Signal, binding as nami_binding};

extern crate alloc;

/// Read-only reactive view of a value updated from any thread.
///
/// See the [module-level documentation](self) for usage.
pub struct Subscribed<T: Clone + 'static> {
    binding: Binding<T>,
}

impl<T: Clone + 'static> Subscribed<T> {
    /// Constructs a [`Subscribed`] from an existing `nami::Binding<T>`.
    ///
    /// Useful when adapter code already holds a binding (e.g. inside a
    /// view's local state) and wants to expose it as the `Subscribed` API.
    /// The returned view shares the binding — `set` calls on the binding
    /// notify watchers of this view, and vice versa.
    #[must_use]
    pub const fn from_binding(binding: Binding<T>) -> Self {
        Self { binding }
    }

    /// Borrows the underlying binding.
    ///
    /// Most consumers do not need this; prefer the `Signal` impl. Provided
    /// for adapters that need to plug a `Subscribed<T>` into APIs that
    /// specifically take a `&Binding<T>`.
    #[must_use]
    pub const fn as_binding(&self) -> &Binding<T> {
        &self.binding
    }
}

impl<T: Clone + 'static> Clone for Subscribed<T> {
    fn clone(&self) -> Self {
        Self {
            binding: self.binding.clone(),
        }
    }
}

impl<T: Clone + 'static> Subscribed<T> {
    /// Synchronous snapshot of the current value.
    ///
    /// Equivalent to `<Self as nami::Signal>::get(&self)`.
    #[must_use]
    pub fn get(&self) -> T {
        Signal::get(&self.binding)
    }

    /// Wraps as a `futures::Stream<Item = T>`.
    ///
    /// Each call returns a fresh stream; multiple stream consumers are
    /// independent and each see every update after the call.
    #[must_use]
    pub fn stream(&self) -> SignalStream<Binding<T>> {
        SignalStream::new(self.binding.clone())
    }

    /// Lazy map: derives a new `Subscribed<U>` whose value is `f(upstream)`.
    ///
    /// Zero spawn — uses `nami::Binding::mapping` directly. Composing
    /// `a.map(f).map(g)` does not allocate forwarding tasks.
    #[must_use]
    pub fn map<U, F>(&self, f: F) -> Subscribed<U>
    where
        U: Clone + 'static,
        F: Fn(T) -> U + Clone + 'static,
    {
        Subscribed {
            binding: Binding::mapping(&self.binding, f, |_, _| {}),
        }
    }
}

impl<T: Clone + 'static> Signal for Subscribed<T> {
    type Output = T;
    type Guard = <Binding<T> as Signal>::Guard;

    fn get(&self) -> T {
        Signal::get(&self.binding)
    }

    fn watch(&self, watcher: impl Fn(WatcherContext<T>) + 'static) -> Self::Guard {
        self.binding.watch(watcher)
    }
}

impl<T: fmt::Debug + Clone + 'static> fmt::Debug for Subscribed<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscribed")
            .field("value", &self.get())
            .finish()
    }
}

/// Cross-thread push handle for a [`Subscribed<T>`].
///
/// `Send + Sync + Clone`. Drop the sink (or all clones) when the producer
/// stops; the binding's mailbox worker exits when the channel closes.
///
/// Internally an `Arc<BindingMailbox<T>>`; clones share one mailbox.
pub struct SubscribedSink<T: 'static> {
    mailbox: Arc<BindingMailbox<T>>,
}

impl<T: 'static> SubscribedSink<T> {
    /// Pushes a new value. Returns immediately; the actual `binding.set`
    /// runs asynchronously on the binding's home thread.
    pub fn set(&self, value: T)
    where
        T: Send + 'static,
    {
        self.mailbox.handle(move |b| b.set(value));
    }

    /// Runs an arbitrary closure with mutable access to the binding on its
    /// home thread. Useful for atomic read-modify-write patterns where
    /// `set` of a clone would race with another producer.
    pub fn handle(&self, job: impl FnOnce(&mut Binding<T>) + Send + 'static) {
        self.mailbox.handle(job);
    }
}

impl<T: 'static> Clone for SubscribedSink<T> {
    fn clone(&self) -> Self {
        Self {
            mailbox: Arc::clone(&self.mailbox),
        }
    }
}

impl<T: 'static> fmt::Debug for SubscribedSink<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubscribedSink").finish_non_exhaustive()
    }
}

/// Constructs a fresh `Subscribed<T>` with an initial value, paired with a
/// cross-thread [`SubscribedSink<T>`].
///
/// The binding lives on the calling thread; the mailbox worker is spawned
/// on `executor_core::DefaultExecutor`.
///
/// # Precondition
///
/// A `LocalExecutor` must be polling on the calling thread for the mailbox
/// worker to drain. Inside a waterui app this is guaranteed on the UI
/// thread; for headless contexts use [`subscribed_with_executor`].
#[must_use]
pub fn subscribed<T: Clone + Send + 'static>(initial: T) -> (Subscribed<T>, SubscribedSink<T>) {
    let binding = nami_binding(initial);
    let mailbox = binding.mailbox();
    (
        Subscribed { binding },
        SubscribedSink {
            mailbox: Arc::new(mailbox),
        },
    )
}

/// Variant of [`subscribed`] that takes an explicit `LocalExecutor`.
///
/// Useful for tests or for crates that need to run the mailbox worker on
/// a non-default executor (rare; consult the nami docs).
#[must_use]
pub fn subscribed_with_executor<T, E>(
    initial: T,
    executor: E,
) -> (Subscribed<T>, SubscribedSink<T>)
where
    T: Clone + Send + 'static,
    E: LocalExecutor,
{
    let binding = nami_binding(initial);
    let mailbox = binding.mailbox_with_executor(executor);
    (
        Subscribed { binding },
        SubscribedSink {
            mailbox: Arc::new(mailbox),
        },
    )
}

#[cfg(test)]
mod tests {
    //! Unit tests cover the bare reactive view (`Subscribed::from_binding`).
    //! Mailbox-based pushing requires a `LocalExecutor`; that path is
    //! exercised in integration tests where a runtime is available.

    use super::*;
    use core::cell::Cell;
    use std::rc::Rc;

    fn make<T: Clone + 'static>(value: T) -> Subscribed<T> {
        let binding: Binding<T> = nami_binding(value);
        Subscribed::from_binding(binding)
    }

    #[test]
    fn snapshot_returns_initial_value() {
        let sub = make(7_u32);
        assert_eq!(sub.get(), 7);
    }

    #[test]
    fn map_derives_value() {
        let sub = make(3_u32);
        let doubled = sub.map(|x: u32| x * 2);
        assert_eq!(doubled.get(), 6);
    }

    #[test]
    fn watch_fires_on_local_set() {
        let sub = make(0_u32);
        let captured = Rc::new(Cell::new(0_u32));
        let captured_for_watcher = Rc::clone(&captured);
        let _guard = sub.watch(move |ctx| {
            captured_for_watcher.set(*ctx.value());
        });
        sub.as_binding().set(42);
        assert_eq!(captured.get(), 42);
        assert_eq!(sub.get(), 42);
    }

    #[test]
    fn map_propagates_upstream_set() {
        let sub = make(10_i32);
        let plus_one = sub.map(|x: i32| x + 1);
        assert_eq!(plus_one.get(), 11);
        sub.as_binding().set(20);
        assert_eq!(plus_one.get(), 21);
    }
}
