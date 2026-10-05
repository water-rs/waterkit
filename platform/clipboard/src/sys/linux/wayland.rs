//! PRIMARY through a Wayland data-control protocol, via `wl-clipboard-rs`.
//!
//! Every operation opens its own connection to the compositor named by
//! `WAYLAND_DISPLAY`. A write hands the text to a thread that `wl-clipboard-rs`
//! spawns; it answers paste requests until another client claims PRIMARY or the
//! process exits.

use std::error::Error;
use std::io::Read;

use wl_clipboard_rs::{copy, paste, utils};

use crate::error::ClipboardError;

/// Bind the compositor's registry and report whether it offers a data-control
/// protocol with a primary selection (`ext_data_control_manager_v1`, or
/// `zwlr_data_control_manager_v1` version 2+), or why it does not.
pub fn probe_data_control() -> Result<(), String> {
    match utils::is_primary_selection_supported() {
        Ok(true) => Ok(()),
        Ok(false) => Err(
            "the compositor's data-control protocol does not provide a primary selection".into(),
        ),
        Err(error) => Err(error_chain(&error)),
    }
}

/// Read the PRIMARY text, or `None` when PRIMARY holds no text.
pub fn get_text() -> Result<Option<String>, ClipboardError> {
    let (mut pipe, _) = match paste::get_contents(
        paste::ClipboardType::Primary,
        paste::Seat::Unspecified,
        paste::MimeType::Text,
    ) {
        Ok(contents) => contents,
        Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(None),
        Err(error) => return Err(platform(&error)),
    };
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)
        .map_err(|error| platform(&error))?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| ClipboardError::Decode(error.to_string()))
}

/// Claim PRIMARY with `text`.
pub fn set_text(text: &str) -> Result<(), ClipboardError> {
    let mut options = copy::Options::new();
    options.clipboard(copy::ClipboardType::Primary);
    options
        .copy(
            copy::Source::Bytes(text.as_bytes().into()),
            copy::MimeType::Text,
        )
        .map_err(|error| platform(&error))
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
