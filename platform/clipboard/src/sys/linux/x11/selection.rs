//! An X selection, read, owned and served in any number of formats as the
//! ICCCM describes.
//!
//! A handle keeps two connections to the X server named by `DISPLAY`:
//!
//! - The reader converts the selection to a format on a window of its own
//!   and waits a bounded time for the owner's answer, taking a large answer
//!   in increments (`INCR`). An owner that refuses the conversion reads as not
//!   offering the format.
//! - The owner claims the selection, and a thread on its connection answers
//!   the conversion requests of other clients, sending a large format in
//!   increments, until another client claims the selection or the last clone
//!   of the handle is dropped.
//!
//! Losing the selection is ordered against claiming it by request sequence
//! number: a `SelectionClear` the X server generated before the handle's
//! latest claim is stale and leaves the newly claimed formats in place.

use std::marker::PhantomData;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec};
use x11rb::connection::{Connection as _, RequestConnection as _, SequenceNumber};
use x11rb::errors::ConnectionError;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, PropMode, Property,
    SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

use super::{connect, intern, owned, platform, wake, watch};
use crate::content::ClipboardEvent;
use crate::error::ClipboardError;
use crate::sys::linux::formats::{Offered, Representation};
use crate::sys::linux::{Backend, Selection, WatchGuard};

/// How long a read waits for each answer of the selection's owner. The ICCCM
/// sets no deadline, so an owner that never answers would otherwise block the
/// read forever.
const READ_TIMEOUT: Duration = Duration::from_secs(4);

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        TARGETS,
        INCR,
        MULTIPLE,
        TIMESTAMP,
        WATERKIT_SELECTION,
    }
}

/// The selection `S` of the X server named by `DISPLAY`.
///
/// Clones share the connections and the claim.
pub struct X11Selection<S> {
    shared: Arc<Shared>,
    selection: PhantomData<fn() -> S>,
}

impl<S> Clone for X11Selection<S> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            selection: PhantomData,
        }
    }
}

impl<S> std::fmt::Debug for X11Selection<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Selection").finish_non_exhaustive()
    }
}

struct Shared {
    selection: Atom,
    atoms: Atoms,
    /// Conversions answer through the reader window's events, one at a time.
    reader: Mutex<Reader>,
    owner: Owner,
}

impl<S: Selection> X11Selection<S> {
    /// Connect to the X server named by `DISPLAY` and start serving the
    /// selection's conversion requests.
    pub fn connect() -> Result<Self, ClipboardError> {
        // The reader takes incremental answers through property changes.
        let (reader, reader_window) = connect(EventMask::PROPERTY_CHANGE)?;
        let (owner, owner_window) = connect(EventMask::NO_EVENT)?;
        let atoms = Atoms::new(&owner)
            .map_err(|error| platform(&error))?
            .reply()
            .map_err(|error| platform(&error))?;
        let selection = intern(&owner, S::NAME)?;
        // The ICCCM's bound on a property written at once; larger formats go
        // in increments of this size.
        let increment = owner.maximum_request_bytes() / 4;
        let owner = Arc::new(owner);
        let claim = Arc::new(Mutex::new(None));
        let server = Server {
            name: S::NAME,
            connection: Arc::clone(&owner),
            window: owner_window,
            selection,
            atoms,
            claim: Arc::clone(&claim),
            increment,
            transfers: Vec::new(),
        };
        thread::Builder::new()
            .name(format!("waterkit {} owner", S::NAME))
            .spawn(move || server.run())
            .map_err(|error| platform(&error))?;
        Ok(Self {
            shared: Arc::new(Shared {
                selection,
                atoms,
                reader: Mutex::new(Reader {
                    connection: reader,
                    window: reader_window,
                }),
                owner: Owner {
                    connection: owner,
                    window: owner_window,
                    claim,
                },
            }),
            selection: PhantomData,
        })
    }
}

impl Shared {
    fn reader(&self) -> MutexGuard<'_, Reader> {
        self.reader
            .lock()
            .expect("a selection read panicked while it held the X11 reader")
    }

    /// Claim the selection with `formats`, or with `None` give it up and
    /// leave it without an owner.
    fn claim(&self, formats: Option<Vec<(Atom, Arc<[u8]>)>>) -> Result<(), ClipboardError> {
        let owner = &self.owner;
        let window = if formats.is_some() {
            owner.window
        } else {
            NONE
        };
        let cookie = {
            // Held until the claim is recorded, so the serving thread compares
            // a `SelectionClear` with this claim and not the one before it.
            let mut claim = owner.claim();
            let cookie = owner
                .connection
                .set_selection_owner(window, self.selection, CURRENT_TIME)
                .map_err(|error| platform(&error))?;
            *claim = formats.map(|formats| Claim {
                sequence: cookie.sequence_number(),
                formats,
            });
            cookie
        };
        cookie.check().map_err(|error| platform(&error))?;
        if window == NONE {
            return Ok(());
        }
        let current = owner
            .connection
            .get_selection_owner(self.selection)
            .map_err(|error| platform(&error))?
            .reply()
            .map_err(|error| platform(&error))?
            .owner;
        if current == window {
            Ok(())
        } else {
            Err(ClipboardError::Platform(
                "another client claimed the selection at the same moment".into(),
            ))
        }
    }
}

impl<S: Selection> Backend for X11Selection<S> {
    fn mime_types(&self) -> Result<Vec<String>, ClipboardError> {
        let Shared {
            selection, atoms, ..
        } = *self.shared;
        let answer = {
            let reader = self.shared.reader();
            if !owned(&reader.connection, selection)? {
                return Ok(Vec::new());
            }
            reader.convert(selection, atoms.TARGETS, &atoms)?
        };
        let Some(answer) = answer else {
            return Err(ClipboardError::Platform(format!(
                "the owner of {} refused to list its formats (TARGETS)",
                S::NAME
            )));
        };
        if answer.format != 32 {
            return Err(ClipboardError::Platform(format!(
                "the owner of {} listed its formats as {}-bit values, not atoms",
                S::NAME,
                answer.format
            )));
        }
        let meta = [NONE, atoms.TARGETS, atoms.MULTIPLE, atoms.TIMESTAMP];
        let connection = &self.shared.owner.connection;
        let cookies = answer
            .value
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&atom| u32::from_ne_bytes(atom))
            .filter(|atom| !meta.contains(atom))
            .map(|atom| connection.get_atom_name(atom))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| platform(&error))?;
        cookies
            .into_iter()
            .map(|cookie| {
                let name = cookie.reply().map_err(|error| platform(&error))?.name;
                String::from_utf8(name).map_err(|error| ClipboardError::Decode(error.to_string()))
            })
            .collect()
    }

    fn read(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        let reader = self.shared.reader();
        let target = intern(&reader.connection, mime)?;
        Ok(reader
            .convert(self.shared.selection, target, &self.shared.atoms)?
            .map(|answer| answer.value))
    }

    fn offer(&self, representations: Vec<Representation>) -> Result<(), ClipboardError> {
        let connection = &self.shared.owner.connection;
        let cookies = representations
            .iter()
            .map(|representation| connection.intern_atom(false, representation.mime.as_bytes()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| platform(&error))?;
        let formats = cookies
            .into_iter()
            .zip(representations)
            .map(|(cookie, representation)| {
                let atom = cookie.reply().map_err(|error| platform(&error))?.atom;
                Ok((atom, Arc::from(representation.bytes)))
            })
            .collect::<Result<Vec<_>, ClipboardError>>()?;
        self.shared.claim(Some(formats))
    }

    fn clear(&self) -> Result<(), ClipboardError> {
        self.shared.claim(None)
    }

    fn watch(
        &self,
        sender: async_channel::Sender<ClipboardEvent>,
    ) -> Result<WatchGuard, ClipboardError> {
        let selection = self.clone();
        watch::watch(S::NAME, move || match selection.mime_types() {
            Ok(mime_types) => match sender.try_send(Offered::new(mime_types).event()) {
                Ok(()) => ControlFlow::Continue(()),
                // The stream was dropped.
                Err(_) => ControlFlow::Break(()),
            },
            Err(error) => {
                tracing::error!(
                    selection = S::NAME,
                    %error,
                    "listing the X11 selection's formats after a change failed; the watch \
                     stream ends"
                );
                ControlFlow::Break(())
            }
        })
    }
}

/// The reading side: a connection and the window conversions are delivered
/// to.
struct Reader {
    connection: RustConnection,
    window: Window,
}

/// An owner's answer to a conversion.
struct Answer {
    format: u8,
    value: Vec<u8>,
}

impl Reader {
    /// Convert `selection` to `target`, or `None` when its owner refuses.
    fn convert(
        &self,
        selection: Atom,
        target: Atom,
        atoms: &Atoms,
    ) -> Result<Option<Answer>, ClipboardError> {
        let property = atoms.WATERKIT_SELECTION;
        self.connection
            .convert_selection(self.window, selection, target, property, CURRENT_TIME)
            .map_err(|error| platform(&error))?;
        self.connection.flush().map_err(|error| platform(&error))?;
        let notify = self.wait(|event| match event {
            Event::SelectionNotify(notify)
                if notify.requestor == self.window
                    && notify.selection == selection
                    && notify.target == target =>
            {
                Some(notify)
            }
            _ => None,
        })?;
        let answered = notify.property;
        if answered == NONE {
            return Ok(None);
        }
        let (kind, answer) = self.take(answered)?;
        if kind != atoms.INCR {
            return Ok(Some(answer));
        }
        // Taking the `INCR` property deleted it, which asks the owner for the
        // first increment. Each increment arrives as a new value of the
        // property, taking it asks for the next, and an empty one ends them.
        let mut value = Vec::new();
        loop {
            self.wait(|event| match event {
                Event::PropertyNotify(changed)
                    if changed.window == self.window
                        && changed.atom == answered
                        && changed.state == Property::NEW_VALUE =>
                {
                    Some(())
                }
                _ => None,
            })?;
            let (_, increment) = self.take(answered)?;
            if increment.value.is_empty() {
                return Ok(Some(Answer {
                    format: increment.format,
                    value,
                }));
            }
            value.extend_from_slice(&increment.value);
        }
    }

    /// Read and delete `property` of the reader window, with its type.
    fn take(&self, property: Atom) -> Result<(Atom, Answer), ClipboardError> {
        let reply = self
            .connection
            .get_property(true, self.window, property, AtomEnum::ANY, 0, u32::MAX)
            .map_err(|error| platform(&error))?
            .reply()
            .map_err(|error| platform(&error))?;
        Ok((
            reply.type_,
            Answer {
                format: reply.format,
                value: reply.value,
            },
        ))
    }

    /// Discard events until `matches` picks one, for at most [`READ_TIMEOUT`].
    fn wait<T>(&self, mut matches: impl FnMut(Event) -> Option<T>) -> Result<T, ClipboardError> {
        let deadline = Instant::now() + READ_TIMEOUT;
        loop {
            while let Some(event) = self
                .connection
                .poll_for_event()
                .map_err(|error| platform(&error))?
            {
                if let Some(found) = matches(event) {
                    return Ok(found);
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ClipboardError::Platform(format!(
                    "the owner of the selection did not answer within {READ_TIMEOUT:?}"
                )));
            }
            let timeout = Timespec::try_from(remaining).expect("the read timeout fits a timespec");
            let mut readable = [PollFd::new(self.connection.stream(), PollFlags::IN)];
            match rustix::event::poll(&mut readable, Some(&timeout)) {
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Err(platform(&error)),
            }
        }
    }
}

/// The owning side, shared with the thread that serves the selection.
struct Owner {
    connection: Arc<RustConnection>,
    window: Window,
    claim: Arc<Mutex<Option<Claim>>>,
}

impl Owner {
    fn claim(&self) -> MutexGuard<'_, Option<Claim>> {
        self.claim
            .lock()
            .expect("serving the selection panicked while it held the claim")
    }
}

impl Drop for Owner {
    /// Stop the serving thread; its connection closes with it, which gives up
    /// the selection.
    fn drop(&mut self) {
        if let Err(error) = wake(&self.connection, self.window) {
            // The connection is gone, and with it the serving thread.
            tracing::debug!(%error, "the X11 owner connection closed before the handle");
        }
    }
}

/// The formats a handle offers while it owns the selection.
struct Claim {
    /// The sequence number of the `SetSelectionOwner` request that made it.
    sequence: SequenceNumber,
    /// Each format's target and bytes.
    formats: Vec<(Atom, Arc<[u8]>)>,
}

/// A format sent in increments to a requestor.
struct Transfer {
    requestor: Window,
    property: Atom,
    target: Atom,
    data: Arc<[u8]>,
    /// How much of `data` has been sent.
    sent: usize,
}

/// Answers other clients' conversion requests on the owner connection.
struct Server {
    name: &'static str,
    connection: Arc<RustConnection>,
    window: Window,
    selection: Atom,
    atoms: Atoms,
    claim: Arc<Mutex<Option<Claim>>>,
    increment: usize,
    transfers: Vec<Transfer>,
}

impl Server {
    fn run(mut self) {
        loop {
            let (event, sequence) = match self.connection.wait_for_event_with_sequence() {
                Ok(event) => event,
                Err(error) => {
                    tracing::error!(
                        selection = self.name,
                        %error,
                        "the X11 connection serving the selection failed; it is no longer served"
                    );
                    return;
                }
            };
            let served = match event {
                Event::SelectionRequest(request) => self.answer(&request),
                Event::SelectionClear(clear) if clear.selection == self.selection => {
                    self.lost(sequence);
                    Ok(())
                }
                Event::PropertyNotify(deleted) if deleted.state == Property::DELETE => {
                    self.send_increment(deleted.window, deleted.atom)
                }
                Event::DestroyNotify(destroyed) => {
                    self.transfers
                        .retain(|transfer| transfer.requestor != destroyed.window);
                    Ok(())
                }
                Event::ClientMessage(message) if message.window == self.window => return,
                // A request to a requestor that has gone away fails; the
                // requestor no longer waits for it.
                Event::Error(error) => {
                    tracing::debug!(?error, "answering a selection request failed");
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(error) = served {
                tracing::error!(
                    selection = self.name,
                    %error,
                    "the X11 connection serving the selection failed; it is no longer served"
                );
                return;
            }
        }
    }

    /// Forget the claim when the selection was lost after it was made.
    /// `sequence` is the last request of this connection the X server had
    /// processed when it reported the loss.
    fn lost(&self, sequence: SequenceNumber) {
        let mut claim = self.claim();
        if claim
            .as_ref()
            .is_some_and(|claim| sequence >= claim.sequence)
        {
            *claim = None;
        }
    }

    /// Answer a conversion request, or refuse it.
    fn answer(&mut self, request: &SelectionRequestEvent) -> Result<(), ConnectionError> {
        // Obsolete clients name no property; the ICCCM has the target serve
        // as the property then.
        let property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let answered = request.selection == self.selection && self.write(request, property)?;
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if answered { property } else { NONE },
        };
        self.connection
            .send_event(false, request.requestor, EventMask::NO_EVENT, notify)?;
        self.connection.flush()
    }

    /// Write the requested format to `property` of the requestor; `false`
    /// when the claim does not offer it.
    fn write(
        &mut self,
        request: &SelectionRequestEvent,
        property: Atom,
    ) -> Result<bool, ConnectionError> {
        if request.target == self.atoms.TARGETS {
            let Some(targets) = self.targets() else {
                return Ok(false);
            };
            self.connection.change_property32(
                PropMode::REPLACE,
                request.requestor,
                property,
                AtomEnum::ATOM,
                &targets,
            )?;
            return Ok(true);
        }
        let Some(data) = self.format(request.target) else {
            return Ok(false);
        };
        if data.len() <= self.increment {
            self.connection.change_property8(
                PropMode::REPLACE,
                request.requestor,
                property,
                request.target,
                &data,
            )?;
            return Ok(true);
        }
        // Too large for one property: announce the size, and send an
        // increment each time the requestor deletes the property.
        self.connection.change_window_attributes(
            request.requestor,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        let size = u32::try_from(data.len()).unwrap_or(u32::MAX);
        self.connection.change_property32(
            PropMode::REPLACE,
            request.requestor,
            property,
            self.atoms.INCR,
            &[size],
        )?;
        self.transfers.push(Transfer {
            requestor: request.requestor,
            property,
            target: request.target,
            data,
            sent: 0,
        });
        Ok(true)
    }

    fn claim(&self) -> MutexGuard<'_, Option<Claim>> {
        self.claim
            .lock()
            .expect("a selection write panicked while it held the claim")
    }

    /// The targets the claim offers, `TARGETS` first, or `None` without one.
    fn targets(&self) -> Option<Vec<Atom>> {
        self.claim().as_ref().map(|claim| {
            std::iter::once(self.atoms.TARGETS)
                .chain(claim.formats.iter().map(|&(target, _)| target))
                .collect()
        })
    }

    /// The bytes of `target`, or `None` when the claim does not offer it.
    fn format(&self, target: Atom) -> Option<Arc<[u8]>> {
        self.claim().as_ref().and_then(|claim| {
            claim
                .formats
                .iter()
                .find(|&&(offered, _)| offered == target)
                .map(|(_, data)| Arc::clone(data))
        })
    }

    /// Send the next increment of the transfer to `property` of `requestor`,
    /// which the requestor just deleted; an empty increment ends it.
    fn send_increment(&mut self, requestor: Window, property: Atom) -> Result<(), ConnectionError> {
        let Some(index) = self
            .transfers
            .iter()
            .position(|transfer| transfer.requestor == requestor && transfer.property == property)
        else {
            return Ok(());
        };
        let transfer = &mut self.transfers[index];
        let end = (transfer.sent + self.increment).min(transfer.data.len());
        let increment = &transfer.data[transfer.sent..end];
        self.connection.change_property8(
            PropMode::REPLACE,
            requestor,
            property,
            transfer.target,
            increment,
        )?;
        if increment.is_empty() {
            self.transfers.swap_remove(index);
            if !self
                .transfers
                .iter()
                .any(|transfer| transfer.requestor == requestor)
            {
                self.connection.change_window_attributes(
                    requestor,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
                )?;
            }
        } else {
            transfer.sent = end;
        }
        self.connection.flush()
    }
}
