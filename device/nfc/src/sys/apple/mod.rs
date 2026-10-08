//! NFC via `CoreNFC`, called through `objc2`.

#[cfg(target_os = "ios")]
mod imp {
    use core::cell::{Cell, RefCell};
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use dispatch2::{DispatchQueue, MainThreadBound};
    use futures::channel::oneshot;
    use objc2::rc::{Retained, Weak};
    use objc2::runtime::{NSObject, ProtocolObject};
    use objc2::{
        AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
    };
    use objc2_core_nfc::{
        NFCNDEFMessage, NFCNDEFPayload, NFCNDEFReaderSession, NFCNDEFReaderSessionDelegate,
        NFCNDEFStatus, NFCNDEFTag, NFCReaderSession, NFCReaderSessionProtocol, NFCTypeNameFormat,
    };
    use objc2_foundation::{NSArray, NSData, NSError, NSObjectProtocol, NSString};
    use waterkit_core::apple::on_main;

    use crate::{NdefMessage, NdefRecord, NfcError, NfcTag, NfcTagType};

    /// `NFCNDEFReaderSession.readingAvailable`, one-to-one.
    pub fn nfc_is_available() -> bool {
        // SAFETY: class accessor with no invariants.
        unsafe { NFCReaderSession::readingAvailable() }
    }

    /// A pending `write` request: delivered to the next discovered tag.
    struct PendingWrite {
        message: NdefMessage,
        callback: oneshot::Sender<Result<(), String>>,
    }

    /// ivars of [`NfcSession`].
    pub struct NfcSessionIvars {
        /// Sends tags and errors to the reader's event receiver. Dropped on
        /// session invalidation, which ends `events()`.
        tag_tx: RefCell<Option<async_channel::Sender<Result<NfcTag, NfcError>>>>,
        /// The pending write taken by `didDetectTags` when a tag connects.
        pending_write: RefCell<Option<PendingWrite>>,
        /// Set when the session is invalidated; a later `write` reports "No
        /// active session" like the removed `activeSessions` lookup did.
        invalidated: Cell<bool>,
        /// Set by `NfcReaderInner::stop` before it invalidates the session:
        /// that invalidation closes the stream without an error item.
        stopped: Cell<bool>,
    }

    define_class!(
        /// `NFCNDEFReaderSessionDelegate` owning one session's state — the
        /// global `activeSessions` registry and the raw-pointer `tag_ctx` are
        /// gone. The session retains its delegate, and the delegate callbacks
        /// are dispatched on the main queue the session was created with.
        // SAFETY:
        // - The superclass NSObject does not have any subclassing
        //   requirements.
        // - `NfcSession` does not implement `Drop`.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[ivars = NfcSessionIvars]
        pub struct NfcSession;

        unsafe impl NSObjectProtocol for NfcSession {}

        // SAFETY: `NFCNDEFReaderSessionDelegate`'s contract is followed: the
        // required methods are implemented, and every ivar is only touched
        // from the main queue the session dispatches on.
        unsafe impl NFCNDEFReaderSessionDelegate for NfcSession {
            #[unsafe(method(readerSession:didInvalidateWithError:))]
            fn did_invalidate(&self, _session: &NFCNDEFReaderSession, error: &NSError) {
                self.ivars().invalidated.set(true);
                // Dropping the sender ends `events()`; `stop()` closes the
                // stream cleanly and carries no error item.
                let tx = self.ivars().tag_tx.borrow_mut().take();
                if let Some(tx) = tx
                    && !self.ivars().stopped.get()
                {
                    let _ = tx.try_send(Err(NfcError::Platform(
                        error.localizedDescription().to_string(),
                    )));
                }
            }

            #[unsafe(method(readerSession:didDetectNDEFs:))]
            fn did_detect_ndefs(
                &self,
                _session: &NFCNDEFReaderSession,
                messages: &NSArray<NFCNDEFMessage>,
            ) {
                for message in messages {
                    // The old bridge sent recordsJson even when empty, so this
                    // path always reports `Some` (possibly with no records).
                    let tag = NfcTag {
                        id: Vec::new(),
                        tag_type: NfcTagType::Type4,
                        ndef_message: Some(ndef_message(&message)),
                    };
                    if let Some(tx) = self.ivars().tag_tx.borrow().as_ref() {
                        let _ = tx.try_send(Ok(tag));
                    }
                }
            }

            #[unsafe(method(readerSession:didDetectTags:))]
            fn did_detect_tags(
                &self,
                session: &NFCNDEFReaderSession,
                tags: &NSArray<ProtocolObject<dyn NFCNDEFTag>>,
            ) {
                let Some(tag) = tags.firstObject() else {
                    return;
                };
                let this = Weak::new(self);
                let tag_in_connect = tag.clone();
                // The completion owns the session so it lives until the
                // framework answers `connectToTag:`.
                let session_in_connect = session.retain();
                let connect = RcBlock::new(move |error: *mut NSError| {
                    let _keep_session_alive = &session_in_connect;
                    let Some(this) = this.load() else {
                        return;
                    };
                    if !error.is_null() {
                        // SAFETY: `error` is non-null and valid for the
                        // duration of the callback.
                        this.report_session_error(unsafe { &*error });
                        return;
                    }
                    let this = Weak::new(&*this);
                    let tag_in_query = tag_in_connect.clone();
                    let query = RcBlock::new(
                        move |status: NFCNDEFStatus, _capacity: usize, error: *mut NSError| {
                            let Some(this) = this.load() else {
                                return;
                            };
                            if !error.is_null() {
                                // SAFETY: `error` is non-null and valid for
                                // the duration of the callback.
                                this.report_session_error(unsafe { &*error });
                                return;
                            }
                            let pending = this.ivars().pending_write.borrow_mut().take();
                            if let Some(pending) = pending {
                                if status == NFCNDEFStatus::ReadWrite {
                                    this.write_records(&tag_in_query, pending);
                                } else {
                                    let _ =
                                        pending.callback.send(Err("Tag is read-only".to_string()));
                                }
                            } else {
                                this.read_records(&tag_in_query);
                            }
                        },
                    );
                    // SAFETY: `tag` is live and `query` matches the documented
                    // `queryNDEFStatus:` block signature.
                    unsafe { tag_in_connect.queryNDEFStatusWithCompletionHandler(&query) };
                });
                // SAFETY: `session` and `tag` are live; `connect` matches the
                // documented `connectToTag:` block signature.
                unsafe { session.connectToTag_completionHandler(&tag, &connect) };
            }
        }
    );

    impl NfcSession {
        fn new(
            tag_tx: async_channel::Sender<Result<NfcTag, NfcError>>,
            mtm: MainThreadMarker,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(NfcSessionIvars {
                tag_tx: RefCell::new(Some(tag_tx)),
                pending_write: RefCell::new(None),
                invalidated: Cell::new(false),
                stopped: Cell::new(false),
            });
            // SAFETY: `this` is a freshly allocated `NfcSession` and
            // `NSObject`'s `init` has no additional requirements.
            unsafe { msg_send![super(this), init] }
        }

        /// A session error is an error item on the event stream; the stream
        /// itself ends only on invalidation.
        fn report_session_error(&self, error: &NSError) {
            if let Some(tx) = self.ivars().tag_tx.borrow().as_ref() {
                let _ = tx.try_send(Err(NfcError::Platform(
                    error.localizedDescription().to_string(),
                )));
            }
        }

        /// `tag.readNDEF` from the `didDetectTags` flow.
        fn read_records(&self, tag: &ProtocolObject<dyn NFCNDEFTag>) {
            let this = Weak::new(self);
            // The completion owns the tag so it lives until `readNDEF:`
            // answers.
            let tag_in_read = tag.retain();
            let read = RcBlock::new(move |message: *mut NFCNDEFMessage, error: *mut NSError| {
                let _keep_tag_alive = &tag_in_read;
                let Some(this) = this.load() else {
                    return;
                };
                if !error.is_null() {
                    // SAFETY: `error` is non-null and valid for the
                    // duration of the callback.
                    this.report_session_error(unsafe { &*error });
                    return;
                }
                // The old bridge sent `nil` when there was no message or
                // it had no records.
                let ndef_message = (!message.is_null())
                    // SAFETY: `message` is non-null and valid for the
                    // duration of the callback.
                    .then(|| unsafe { &*message })
                    .and_then(ndef_message_nonempty);
                let tag = NfcTag {
                    id: Vec::new(),
                    tag_type: NfcTagType::Type4,
                    ndef_message,
                };
                if let Some(tx) = this.ivars().tag_tx.borrow().as_ref() {
                    let _ = tx.try_send(Ok(tag));
                }
            });
            // SAFETY: `tag` is live and `read` matches the documented
            // `readNDEF:` block signature.
            unsafe { tag.readNDEFWithCompletionHandler(&read) };
        }

        /// `tag.writeNDEF` from the `didDetectTags` flow.
        #[expect(
            clippy::unused_self,
            reason = "this is part of the session's tag-interaction flow; keeping it a method keeps the grouping"
        )]
        fn write_records(&self, tag: &ProtocolObject<dyn NFCNDEFTag>, pending: PendingWrite) {
            let records = pending
                .message
                .records
                .iter()
                .filter_map(|record| {
                    let format = type_name_format(record.tnf)?;
                    // SAFETY: `initWithFormat:type:identifier:payload:` is the
                    // designated initializer and all data arguments are live.
                    Some(unsafe {
                        NFCNDEFPayload::initWithFormat_type_identifier_payload(
                            NFCNDEFPayload::alloc(),
                            format,
                            &NSData::from_vec(record.record_type.clone()),
                            &NSData::new(),
                            &NSData::from_vec(record.payload.clone()),
                        )
                    })
                })
                .collect::<Vec<_>>();
            let array = NSArray::from_retained_slice(&records);
            // SAFETY: `initWithNDEFRecords:` is the designated initializer and
            // `array` only contains `NFCNDEFPayload` objects.
            let message =
                unsafe { NFCNDEFMessage::initWithNDEFRecords(NFCNDEFMessage::alloc(), &array) };
            // `Mutex` because the block must be `Fn`, not `FnOnce`.
            let callback = Mutex::new(Some(pending.callback));
            // The completion owns the tag so it lives until `writeNDEF:`
            // answers.
            let tag_in_write = tag.retain();
            let write = RcBlock::new(move |error: *mut NSError| {
                let _keep_tag_alive = &tag_in_write;
                let Some(callback) = callback
                    .lock()
                    .expect("nfc write callback mutex poisoned")
                    .take()
                else {
                    return;
                };
                let result = if error.is_null() {
                    Ok(())
                } else {
                    // SAFETY: `error` is non-null and valid for the duration
                    // of the callback.
                    Err(unsafe { &*error }.localizedDescription().to_string())
                };
                let _ = callback.send(result);
            });
            // SAFETY: `tag` is live, `message` is a valid `NFCNDEFMessage`,
            // and `write` matches the documented `writeNDEF:` block signature.
            unsafe { tag.writeNDEF_completionHandler(&message, &write) };
        }
    }

    /// Builds the typed `NdefMessage` from an `NFCNDEFMessage`'s records.
    fn ndef_message(message: &NFCNDEFMessage) -> NdefMessage {
        NdefMessage {
            // SAFETY: `message` is live; `records` is a read-only accessor.
            records: unsafe { message.records() }
                .iter()
                .map(|record| NdefRecord {
                    // SAFETY: read-only accessors on a live record.
                    tnf: unsafe { record.typeNameFormat() }.0,
                    record_type: unsafe { record.r#type() }.to_vec(),
                    payload: unsafe { record.payload() }.to_vec(),
                })
                .collect(),
        }
    }

    /// `None` when the message has no records (the old bridge sent `nil`).
    fn ndef_message_nonempty(message: &NFCNDEFMessage) -> Option<NdefMessage> {
        let message = ndef_message(message);
        (!message.records.is_empty()).then_some(message)
    }

    /// Maps a TNF byte to `NFCTypeNameFormat`, keeping the old `compactMap`
    /// semantics: values outside the defined cases are skipped.
    const fn type_name_format(tnf: u8) -> Option<NFCTypeNameFormat> {
        match tnf {
            0x00 => Some(NFCTypeNameFormat::Empty),
            0x01 => Some(NFCTypeNameFormat::NFCWellKnown),
            0x02 => Some(NFCTypeNameFormat::Media),
            0x03 => Some(NFCTypeNameFormat::AbsoluteURI),
            0x04 => Some(NFCTypeNameFormat::NFCExternal),
            0x05 => Some(NFCTypeNameFormat::Unknown),
            0x06 => Some(NFCTypeNameFormat::Unchanged),
            _ => None,
        }
    }

    /// A running reader session.
    #[derive(Debug)]
    pub struct NfcReaderInner {
        /// The reader session; delegates callbacks to `delegate` on the main
        /// queue.
        session: Arc<MainThreadBound<Retained<NFCNDEFReaderSession>>>,
        /// The delegate owning the session's state; also retained by the
        /// session itself.
        delegate: Arc<MainThreadBound<Retained<NfcSession>>>,
    }

    impl NfcReaderInner {
        pub async fn start_session(
            message: &str,
        ) -> Result<(Self, async_channel::Receiver<Result<NfcTag, NfcError>>), NfcError> {
            if !nfc_is_available() {
                return Err(NfcError::NotAvailable);
            }
            let (tag_tx, tag_rx) = async_channel::unbounded();
            let message = message.to_owned();
            let (session, delegate) = on_main(move |mtm| {
                let delegate = NfcSession::new(tag_tx, mtm);
                // SAFETY: `initWithDelegate:queue:invalidateAfterFirstRead:` is
                // the designated initializer; `delegate` conforms to the
                // protocol and callbacks are dispatched on the main queue,
                // which matches the delegate's `MainThreadOnly` thread kind.
                let session = unsafe {
                    NFCNDEFReaderSession::initWithDelegate_queue_invalidateAfterFirstRead(
                        mtm.alloc(),
                        ProtocolObject::from_ref(&*delegate),
                        Some(DispatchQueue::main()),
                        false,
                    )
                };
                let alert = NSString::from_str(&message);
                // SAFETY: `session` is live and `alertMessage` is a plain
                // property setter.
                unsafe { session.setAlertMessage(&alert) };
                // SAFETY: `session` is live; starting a session has no
                // invariants.
                unsafe { session.beginSession() };
                (
                    Arc::new(MainThreadBound::new(session, mtm)),
                    Arc::new(MainThreadBound::new(delegate, mtm)),
                )
            })
            .await;
            // Any later failure (invalidation, tag I/O) arrives as an error
            // item on `tag_rx`; nothing is sampled synchronously here.
            Ok((Self { session, delegate }, tag_rx))
        }

        pub async fn write(&self, message: NdefMessage) -> Result<(), NfcError> {
            let (tx, rx) = oneshot::channel::<Result<(), String>>();
            let delegate = Arc::clone(&self.delegate);
            on_main(move |mtm| {
                let ivars = delegate.get(mtm).ivars();
                if ivars.invalidated.get() {
                    let _ = tx.send(Err("No active session".to_string()));
                } else {
                    *ivars.pending_write.borrow_mut() = Some(PendingWrite {
                        message,
                        callback: tx,
                    });
                }
            })
            .await;
            rx.await
                .map_err(|_| NfcError::Platform("callback dropped".into()))?
                .map_err(NfcError::WriteFailed)
        }

        pub fn stop(&self) {
            let session = Arc::clone(&self.session);
            let delegate = Arc::clone(&self.delegate);
            DispatchQueue::main().exec_async(move || {
                let mtm = MainThreadMarker::new().expect("exec_async runs on the main queue");
                // `stopped` is recorded on the delegate first, so the ensuing
                // `didInvalidate` callback closes the stream without an item.
                delegate.get(mtm).ivars().stopped.set(true);
                // SAFETY: the session is live; `invalidateSession` ends it and
                // its callbacks.
                unsafe { session.get(mtm).invalidateSession() };
            });
        }
    }
}

#[cfg(target_os = "ios")]
pub use imp::{NfcReaderInner, nfc_is_available};

/// NFC exists nowhere but iOS; non-iOS Apple keeps the same "not available"
/// surface the conditional-compiled version had.
#[cfg(not(target_os = "ios"))]
#[allow(clippy::missing_const_for_fn)]
pub fn nfc_is_available() -> bool {
    false
}

/// Placeholder on Apple platforms other than iOS, where `nfc_is_available`
/// already reports `false`.
#[cfg(not(target_os = "ios"))]
#[derive(Debug)]
pub struct NfcReaderInner {
    _private: (),
}

#[cfg(not(target_os = "ios"))]
impl NfcReaderInner {
    /// Only reachable if `nfc_is_available` lied, which it cannot.
    #[expect(
        clippy::unused_async,
        reason = "the async signature is part of the crate API surface and other platforms await here"
    )]
    pub async fn start_session(
        _message: &str,
    ) -> Result<
        (
            Self,
            async_channel::Receiver<Result<crate::NfcTag, crate::NfcError>>,
        ),
        crate::NfcError,
    > {
        Err(crate::NfcError::NotAvailable)
    }

    #[expect(
        clippy::unused_async,
        reason = "the async signature is part of the crate API surface and other platforms await here"
    )]
    pub async fn write(&self, _message: crate::NdefMessage) -> Result<(), crate::NfcError> {
        Err(crate::NfcError::NotAvailable)
    }

    #[expect(
        clippy::unused_self,
        reason = "there is no session state on non-iOS platforms"
    )]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "the iOS implementation invalidates the session on the main queue"
    )]
    pub fn stop(&self) {}
}
