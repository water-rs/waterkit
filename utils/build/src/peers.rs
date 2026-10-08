//! The JVM-independent state behind `NativeCallback` / `NativeChannel` peers.
//!
//! The Android glue ([`crate::NativeCallback`], [`crate::NativeChannel`])
//! decodes Java payloads through `FromJava` and hands the results here; these
//! types own the channel endpoints and the deliver/terminate state machine.
//! Because nothing in this module touches JNI, its tests run on the host.

use futures_channel::{mpsc, oneshot};

/// Why a `NativeCallback` result or a `NativeChannel` item failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PeerError {
    /// The Kotlin helper reported failure through `fail`.
    #[error("{0}")]
    Rejected(String),
    /// Decoding the delivered payload failed.
    #[cfg(target_os = "android")]
    #[error(transparent)]
    Decode(#[from] crate::AndroidError),
}

/// Delivery end of a `NativeCallback` — one result, then done.
pub struct CallbackDelivery<T> {
    sender: Option<oneshot::Sender<Result<T, PeerError>>>,
}

impl<T> CallbackDelivery<T> {
    /// The delivery and the receiver that resolves on its `complete`/`fail`.
    pub fn new() -> (Self, oneshot::Receiver<Result<T, PeerError>>) {
        let (sender, receiver) = oneshot::channel();
        (
            Self {
                sender: Some(sender),
            },
            receiver,
        )
    }

    /// Delivers one payload; further calls are ignored.
    pub fn deliver(&mut self, result: Result<T, PeerError>) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(result);
        }
    }

    /// `fail` (`Some`) delivers the rejection; `close`/`release` (`None`)
    /// drops the sender, which is the receiver's cancellation signal.
    pub fn terminate(&mut self, reason: Option<String>) {
        if let Some(reason) = reason {
            self.deliver(Err(PeerError::Rejected(reason)));
        } else {
            self.sender = None;
        }
    }
}

impl<T> Drop for CallbackDelivery<T> {
    fn drop(&mut self) {
        // Dropping the last sender cancels the awaiting receiver.
        self.sender = None;
    }
}

/// Delivery end of a `NativeChannel` — any number of items, then `close`
/// (clean end), `fail` (an error item, then the end), or `release` (a bare
/// end — cancellation).
pub struct ChannelDelivery<T> {
    sender: Option<mpsc::UnboundedSender<Result<T, PeerError>>>,
}

impl<T> ChannelDelivery<T> {
    /// The delivery and the stream end the consumer reads.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<Result<T, PeerError>>) {
        let (sender, receiver) = mpsc::unbounded();
        (
            Self {
                sender: Some(sender),
            },
            receiver,
        )
    }

    /// Streams one item; ignored once the channel terminated.
    pub fn deliver(&self, result: Result<T, PeerError>) {
        if let Some(sender) = &self.sender {
            let _ = sender.unbounded_send(result);
        }
    }

    /// `fail` (`Some`) delivers the rejection item and ends the stream;
    /// `close`/`release` (`None`) just ends it.
    pub fn terminate(&mut self, reason: Option<String>) {
        if let Some(reason) = reason {
            self.deliver(Err(PeerError::Rejected(reason)));
        }
        self.sender = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{CallbackDelivery, ChannelDelivery, PeerError};

    #[test]
    fn callback_delivers_one_result_then_ignores_late_calls() {
        let (mut delivery, mut receiver) = CallbackDelivery::<u8>::new();
        delivery.deliver(Ok(7));
        assert!(matches!(receiver.try_recv().unwrap(), Some(Ok(7))));
        // After the answer, a late terminate must not panic or redeliver.
        delivery.terminate(Some("late".into()));
    }

    #[test]
    fn callback_fail_delivers_rejection() {
        let (mut delivery, mut receiver) = CallbackDelivery::<u8>::new();
        delivery.terminate(Some("denied".into()));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            Some(Err(PeerError::Rejected(reason))) if reason == "denied"
        ));
    }

    #[test]
    fn callback_release_cancels_the_receiver() {
        let (mut delivery, mut receiver) = CallbackDelivery::<u8>::new();
        delivery.terminate(None);
        // Dropping the last sender leaves the oneshot canceled.
        assert!(receiver.try_recv().is_err());
        drop(delivery);
    }

    #[test]
    fn channel_streams_items_then_ends_on_close() {
        let (mut delivery, mut receiver) = ChannelDelivery::<u8>::new();
        delivery.deliver(Ok(1));
        delivery.deliver(Ok(2));
        delivery.terminate(None);
        assert!(matches!(receiver.try_recv().unwrap(), Ok(1)));
        assert!(matches!(receiver.try_recv().unwrap(), Ok(2)));
        assert!(receiver.try_recv().unwrap_err().is_closed());
    }

    #[test]
    fn channel_fail_delivers_error_then_ends() {
        let (mut delivery, mut receiver) = ChannelDelivery::<u8>::new();
        delivery.deliver(Ok(1));
        delivery.terminate(Some("boom".into()));
        assert!(matches!(receiver.try_recv().unwrap(), Ok(1)));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            Err(PeerError::Rejected(reason)) if reason == "boom"
        ));
        assert!(receiver.try_recv().unwrap_err().is_closed());
    }

    #[test]
    fn channel_release_ends_the_stream() {
        let (delivery, mut receiver) = ChannelDelivery::<u8>::new();
        delivery.deliver(Ok(1));
        drop(delivery);
        assert!(matches!(receiver.try_recv().unwrap(), Ok(1)));
        assert!(receiver.try_recv().unwrap_err().is_closed());
    }
}
