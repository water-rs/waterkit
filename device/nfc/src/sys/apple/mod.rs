use crate::{NdefMessage, NdefRecord, NfcError, NfcTag, NfcTagType};

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn nfc_is_available_bridge() -> bool;
        fn nfc_start_session(
            message: &str,
            tag_ctx: u64,
            error_callback: Box<dyn FnOnce(String) -> ()>,
        );
        fn nfc_stop_session(tag_ctx: u64);
        fn nfc_write_message(
            tag_ctx: u64,
            records_json: &str,
            callback: Box<dyn FnOnce(String) -> ()>,
        );
    }

    extern "Rust" {
        fn on_nfc_tag_discovered_raw(
            tag_ctx: u64,
            tag_id_hex: &str,
            tag_type: &str,
            records_json: Option<String>,
        );
    }
}

/// Called from Swift when an NFC tag is discovered.
/// `tag_ctx` is a raw pointer to an `async_channel::Sender<NfcTag>`.
#[allow(clippy::cast_possible_truncation)]
fn on_nfc_tag_discovered_raw(
    tag_ctx: u64,
    tag_id_hex: &str,
    tag_type: &str,
    records_json: Option<String>,
) {
    let tx = unsafe { &*(tag_ctx as usize as *const async_channel::Sender<NfcTag>) };
    let id = hex_decode(tag_id_hex);
    let tag_type = match tag_type {
        "1" => NfcTagType::Type1,
        "2" => NfcTagType::Type2,
        "3" => NfcTagType::Type3,
        "4" => NfcTagType::Type4,
        "5" => NfcTagType::Type5,
        "mifare" => NfcTagType::MifareClassic,
        _ => NfcTagType::Unknown,
    };
    let ndef_message = records_json.map(|json| parse_ndef_records(&json));
    let tag = NfcTag {
        id,
        tag_type,
        ndef_message,
    };
    let _ = tx.try_send(tag);
}

fn parse_ndef_records(json: &str) -> NdefMessage {
    let mut records = Vec::new();
    for rec_str in json.split(';').filter(|s| !s.is_empty()) {
        let parts: Vec<&str> = rec_str.splitn(3, ':').collect();
        if parts.len() == 3 {
            let tnf = parts[0].parse().unwrap_or(0);
            let record_type = hex_decode(parts[1]);
            let payload = hex_decode(parts[2]);
            records.push(NdefRecord {
                tnf,
                record_type,
                payload,
            });
        }
    }
    NdefMessage { records }
}

fn hex_decode(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

fn hex_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn nfc_is_available() -> bool {
    ffi::nfc_is_available_bridge()
}

#[derive(Debug)]
pub struct NfcReaderInner {
    /// Boxed sender kept alive while the session is active.
    _tag_tx: Box<async_channel::Sender<NfcTag>>,
    tag_ctx: u64,
}

impl NfcReaderInner {
    pub async fn start_session(
        message: &str,
    ) -> Result<(Self, async_channel::Receiver<NfcTag>), NfcError> {
        if !nfc_is_available() {
            return Err(NfcError::NotAvailable);
        }
        let (tag_tx, tag_rx) = async_channel::bounded(16);
        let tag_tx = Box::new(tag_tx);
        let tag_ctx = (&raw const *tag_tx) as usize as u64;

        let (err_tx, mut err_rx) = futures::channel::oneshot::channel::<String>();
        ffi::nfc_start_session(
            message,
            tag_ctx,
            Box::new(move |error: String| {
                let _ = err_tx.send(error);
            }),
        );
        if let Ok(Some(err)) = futures::future::poll_fn(|cx| {
            use std::pin::Pin;
            use std::task::Poll;
            match Pin::new(&mut err_rx).poll(cx) {
                Poll::Ready(Ok(e)) if !e.is_empty() => Poll::Ready(Ok::<
                    Option<String>,
                    futures::channel::oneshot::Canceled,
                >(Some(e))),
                _ => Poll::Ready(Ok(None)),
            }
        })
        .await
        {
            return Err(NfcError::Platform(err));
        }

        Ok((
            Self {
                _tag_tx: tag_tx,
                tag_ctx,
            },
            tag_rx,
        ))
    }

    pub async fn write(&self, message: NdefMessage) -> Result<(), NfcError> {
        let (tx, rx) = futures::channel::oneshot::channel();
        let records_json: String = message
            .records
            .iter()
            .map(|r| {
                format!(
                    "{}:{}:{}",
                    r.tnf,
                    hex_encode(&r.record_type),
                    hex_encode(&r.payload)
                )
            })
            .collect::<Vec<_>>()
            .join(";");
        ffi::nfc_write_message(
            self.tag_ctx,
            &records_json,
            Box::new(move |error: String| {
                if error.is_empty() {
                    let _ = tx.send(Ok(()));
                } else {
                    let _ = tx.send(Err(NfcError::WriteFailed(error)));
                }
            }),
        );
        rx.await
            .map_err(|_| NfcError::Platform("callback dropped".into()))?
    }

    pub fn stop(&self) {
        ffi::nfc_stop_session(self.tag_ctx);
    }
}
