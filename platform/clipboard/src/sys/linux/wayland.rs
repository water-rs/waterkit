//! A selection through a Wayland data-control protocol, via `wl-clipboard-rs`.
//!
//! Every operation opens its own connection to the compositor named by
//! `WAYLAND_DISPLAY`. A write hands its formats to a thread that
//! `wl-clipboard-rs` spawns; it answers paste requests until another client
//! claims the selection or the process exits. A watch keeps one connection on
//! a thread of its own, which the compositor's selection events wake.

use std::error::Error;
use std::io::Read;
use std::marker::PhantomData;
use std::thread;

use wl_clipboard_rs::{copy, paste, utils, watch};

use super::formats::{Offered, Representation};
use super::session::DataControl;
use super::{Backend, Selection, StopWatch, WatchGuard};
use crate::content::ClipboardEvent;
use crate::error::ClipboardError;

/// Bind the compositor's registry and report the data-control protocol it
/// offers, or why it offers none.
pub fn probe() -> Result<DataControl, String> {
    match utils::is_primary_selection_supported() {
        Ok(true) => Ok(DataControl::WithPrimary),
        Ok(false) => Ok(DataControl::Regular),
        Err(error) => Err(error_chain(&error)),
    }
}

/// The selection `S` of the compositor named by `WAYLAND_DISPLAY`.
///
/// `S` only names the selection and is never held, hence `fn() -> S`: the
/// handle is `Send` and `Sync` whatever `S` is.
pub struct WaylandSelection<S>(PhantomData<fn() -> S>);

impl<S> std::fmt::Debug for WaylandSelection<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaylandSelection").finish_non_exhaustive()
    }
}

impl<S: Selection> WaylandSelection<S> {
    /// The selection `S`. Data-control must offer it, which
    /// [`select`](super::session::select) checked.
    pub const fn new() -> Self {
        Self(PhantomData)
    }

    const fn copy_type() -> copy::ClipboardType {
        match S::WAYLAND {
            paste::ClipboardType::Regular => copy::ClipboardType::Regular,
            paste::ClipboardType::Primary => copy::ClipboardType::Primary,
        }
    }
}

impl<S: Selection> Backend for WaylandSelection<S> {
    fn mime_types(&self) -> Result<Vec<String>, ClipboardError> {
        match paste::get_mime_types_ordered(S::WAYLAND, paste::Seat::Unspecified) {
            Ok(mime_types) => Ok(mime_types),
            Err(paste::Error::ClipboardEmpty) => Ok(Vec::new()),
            Err(error) => Err(platform(&error)),
        }
    }

    fn read(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        let (mut pipe, _) = match paste::get_contents(
            S::WAYLAND,
            paste::Seat::Unspecified,
            paste::MimeType::Specific(mime),
        ) {
            Ok(contents) => contents,
            // The selection changed after its formats were listed.
            Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(None),
            Err(error) => return Err(platform(&error)),
        };
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)
            .map_err(|error| platform(&error))?;
        Ok(Some(bytes))
    }

    fn offer(&self, representations: Vec<Representation>) -> Result<(), ClipboardError> {
        let mut options = copy::Options::new();
        // Offer exactly the given formats. By default `wl-clipboard-rs` also
        // offers the first text-like one (such as `text/html`) under every
        // plain-text MIME type.
        options
            .clipboard(Self::copy_type())
            .omit_additional_text_mime_types(true);
        let sources = representations
            .into_iter()
            .map(|representation| copy::MimeSource {
                source: copy::Source::Bytes(representation.bytes.into()),
                mime_type: copy::MimeType::Specific(representation.mime),
            })
            .collect();
        options
            .copy_multi(sources)
            .map_err(|error| platform(&error))
    }

    fn clear(&self) -> Result<(), ClipboardError> {
        copy::clear(Self::copy_type(), copy::Seat::All).map_err(|error| platform(&error))
    }

    fn watch(
        &self,
        sender: async_channel::Sender<ClipboardEvent>,
    ) -> Result<WatchGuard, ClipboardError> {
        let mut watcher = watch::Watcher::new(S::WAYLAND.into(), paste::Seat::Unspecified)
            .map_err(|error| platform(&error))?;
        let cancel = watcher.cancel_handle();
        thread::Builder::new()
            .name(format!("waterkit {} watch", S::NAME))
            .spawn(move || forward_changes::<S>(&mut watcher, &sender))
            .map_err(|error| platform(&error))?;
        Ok(WatchGuard::new(cancel))
    }
}

impl StopWatch for watch::CancelHandle {
    fn stop(&self) {
        self.cancel();
    }
}

/// Send an event for every selection change `watcher` reports, until it is
/// cancelled, fails, or `sender`'s receiver is dropped.
fn forward_changes<S: Selection>(
    watcher: &mut watch::Watcher,
    sender: &async_channel::Sender<ClipboardEvent>,
) {
    // The first event reports the selection held when the watch started; it
    // is not a change.
    if next_selection::<S>(watcher).is_none() {
        return;
    }
    while let Some(offered) = next_selection::<S>(watcher) {
        if sender.try_send(offered.event()).is_err() {
            return;
        }
    }
}

/// The formats of the next selection `watcher` reports, or `None` once it is
/// cancelled or fails.
fn next_selection<S: Selection>(watcher: &mut watch::Watcher) -> Option<Offered> {
    match watcher.next_event() {
        Ok(Some(watch::ClipboardEvent::Changed { mime_types, .. })) => {
            Some(Offered::new(mime_types))
        }
        Ok(Some(watch::ClipboardEvent::Cleared { .. })) => Some(Offered::default()),
        Ok(None) => None,
        Err(error) => {
            tracing::error!(
                selection = S::NAME,
                error = error_chain(&error),
                "watching the Wayland selection failed; the watch stream ends"
            );
            None
        }
    }
}

fn platform(error: &(dyn Error + 'static)) -> ClipboardError {
    ClipboardError::Platform(error_chain(error))
}

/// `error` and its sources, outermost first. `wl-clipboard-rs` keeps the
/// detail of a failure (which connection, which I/O error) in the sources.
fn error_chain(error: &(dyn Error + 'static)) -> String {
    std::iter::successors(Some(error), |&error| error.source())
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}
