//! Cross-platform NFC tag reading and writing.
//!
//! This crate provides unified APIs for:
//! - **Tag Discovery**: Scanning for nearby NFC tags
//! - **NDEF Read/Write**: Reading and writing NDEF messages
//! - **Tag Information**: Identifying tag types and capabilities

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod sys;

use futures_core::Stream;

/// Android-specific JNI helpers that require an `Env` and `Context`.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::jni_api::is_available;
}

/// NFC tag type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NfcTagType {
    /// NFC Forum Type 1 Tag.
    Type1,
    /// NFC Forum Type 2 Tag.
    Type2,
    /// NFC Forum Type 3 Tag (`FeliCa`).
    Type3,
    /// NFC Forum Type 4 Tag (ISO-DEP).
    Type4,
    /// NFC Forum Type 5 Tag (ISO 15693).
    Type5,
    /// MIFARE Classic.
    MifareClassic,
    /// Unknown tag type.
    Unknown,
}

/// An NDEF record.
#[derive(Debug, Clone)]
pub struct NdefRecord {
    /// TNF (Type Name Format).
    pub tnf: u8,
    /// Record type bytes.
    pub record_type: Vec<u8>,
    /// Payload bytes.
    pub payload: Vec<u8>,
}

impl NdefRecord {
    /// Create a well-known text record.
    #[must_use]
    pub fn text(text: &str) -> Self {
        let lang = b"en";
        #[allow(clippy::cast_possible_truncation)]
        let mut payload = vec![lang.len() as u8];
        payload.extend_from_slice(lang);
        payload.extend_from_slice(text.as_bytes());
        Self {
            tnf: 0x01,
            record_type: b"T".to_vec(),
            payload,
        }
    }

    /// Create a well-known URI record.
    #[must_use]
    pub fn uri(uri: &str) -> Self {
        let mut payload = vec![0x00];
        payload.extend_from_slice(uri.as_bytes());
        Self {
            tnf: 0x01,
            record_type: b"U".to_vec(),
            payload,
        }
    }
}

/// An NDEF message (collection of records).
#[derive(Debug, Clone, Default)]
pub struct NdefMessage {
    /// Records in the message.
    pub records: Vec<NdefRecord>,
}

impl NdefMessage {
    /// Create a new empty NDEF message.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a record to the message.
    #[must_use]
    pub fn with_record(mut self, record: NdefRecord) -> Self {
        self.records.push(record);
        self
    }
}

/// An NFC tag that was discovered.
#[derive(Debug, Clone)]
pub struct NfcTag {
    /// Tag identifier bytes.
    pub id: Vec<u8>,
    /// Tag type.
    pub tag_type: NfcTagType,
    /// NDEF message read from the tag (if available).
    pub ndef_message: Option<NdefMessage>,
}

/// Check if NFC is available on this device.
#[must_use]
pub fn is_available() -> bool {
    sys::nfc_is_available()
}

/// NFC reader for discovering and interacting with NFC tags.
#[derive(Debug)]
pub struct NfcReader {
    inner: sys::NfcReaderInner,
    events: async_channel::Receiver<NfcTag>,
}

impl NfcReader {
    /// Starts an NFC reading session with a user-visible message.
    ///
    /// # Errors
    ///
    /// Returns [`NfcError`] if NFC is not available or the session
    /// cannot be started.
    pub async fn new(message: &str) -> Result<Self, NfcError> {
        let (inner, events) = sys::NfcReaderInner::start_session(message).await?;
        Ok(Self { inner, events })
    }

    /// Returns a `Stream<Item = NfcTag>` of discovered tags.
    pub fn events(&self) -> impl Stream<Item = NfcTag> + Send + 'static {
        self.events.clone()
    }

    /// Writes an NDEF message to the next discovered tag.
    ///
    /// # Errors
    ///
    /// Returns [`NfcError`] when the write fails.
    pub async fn write(&self, message: NdefMessage) -> Result<(), NfcError> {
        self.inner.write(message).await
    }

    /// Stops the NFC session.
    #[allow(clippy::missing_const_for_fn)]
    pub fn stop(&self) {
        self.inner.stop();
    }
}

/// Errors that can occur in NFC operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum NfcError {
    /// NFC is not available on this device.
    #[error("NFC not available")]
    NotAvailable,
    /// NFC permission not granted.
    #[error("NFC permission denied")]
    PermissionDenied,
    /// Tag connection lost.
    #[error("tag connection lost")]
    ConnectionLost,
    /// Write failed.
    #[error("write failed: {0}")]
    WriteFailed(String),
    /// Tag is read-only.
    #[error("tag is read-only")]
    ReadOnly,
    /// Not supported on this platform.
    #[error("not supported")]
    Unsupported,
    /// Platform-specific error.
    #[error("platform error: {0}")]
    Platform(String),
}
