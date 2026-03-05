use crate::{NdefMessage, NdefRecord, NfcError, NfcTag, NfcTagType};
use std::sync::Mutex;
use windows::Networking::Proximity::{MessageReceivedHandler, ProximityDevice};

pub fn nfc_is_available() -> bool {
    // Windows Proximity API (NFC) - check if device supports it
    windows::Networking::Proximity::ProximityDevice::GetDefault().is_ok()
}

#[derive(Debug)]
pub struct NfcReaderInner {
    device: ProximityDevice,
    subscription_id: i64,
    publish_id: Mutex<Option<i64>>,
}

fn decode_uri_payload(payload: &[u8]) -> Option<String> {
    let (&prefix, rest) = payload.split_first()?;
    let prefix_str = match prefix {
        0x01 => "http://www.",
        0x02 => "https://www.",
        0x03 => "http://",
        0x04 => "https://",
        _ => "",
    };
    let suffix = std::str::from_utf8(rest).ok()?;
    Some(format!("{prefix_str}{suffix}"))
}

fn decode_text_payload(payload: &[u8]) -> Option<String> {
    let (&status, rest) = payload.split_first()?;
    let lang_len = usize::from(status & 0x3F);
    if rest.len() < lang_len {
        return None;
    }
    std::str::from_utf8(&rest[lang_len..])
        .ok()
        .map(std::string::ToString::to_string)
}

impl NfcReaderInner {
    #[allow(clippy::unused_async)]
    pub async fn start_session(
        _message: &str,
    ) -> Result<(Self, async_channel::Receiver<NfcTag>), NfcError> {
        let device = ProximityDevice::GetDefault().map_err(|_| NfcError::NotAvailable)?;
        let (tx, rx) = async_channel::bounded(16);
        let subscription_id = device
            .SubscribeForMessage(
                &windows::core::HSTRING::from("NDEF"),
                &MessageReceivedHandler::new(move |_device, message| {
                    let Some(message) = message.as_ref() else {
                        return Ok(());
                    };
                    let data_string = message
                        .DataAsString()
                        .map(|value| value.to_string())
                        .unwrap_or_default();
                    let ndef_message = if data_string.is_empty() {
                        None
                    } else {
                        Some(NdefMessage {
                            records: vec![NdefRecord::text(&data_string)],
                        })
                    };
                    let _ = tx.try_send(NfcTag {
                        id: Vec::new(),
                        tag_type: NfcTagType::Unknown,
                        ndef_message,
                    });
                    Ok(())
                }),
            )
            .map_err(|e| NfcError::PlatformError(format!("SubscribeForMessage failed: {e}")))?;

        Ok((
            Self {
                device,
                subscription_id,
                publish_id: Mutex::new(None),
            },
            rx,
        ))
    }

    #[allow(clippy::unused_async)]
    pub async fn write(&self, message: NdefMessage) -> Result<(), NfcError> {
        let record = message
            .records
            .first()
            .ok_or_else(|| NfcError::WriteFailed("NDEF message has no records".into()))?;

        let next_publish_id = if record.record_type.as_slice() == b"U" {
            let uri = decode_uri_payload(&record.payload)
                .ok_or_else(|| NfcError::WriteFailed("invalid URI record payload".into()))?;
            let uri_obj = windows::Foundation::Uri::CreateUri(&windows::core::HSTRING::from(uri))
                .map_err(|e| NfcError::WriteFailed(format!("CreateUri failed: {e}")))?;
            self.device
                .PublishUriMessage(&uri_obj)
                .map_err(|e| NfcError::WriteFailed(format!("PublishUriMessage failed: {e}")))?
        } else if record.record_type.as_slice() == b"T" {
            let text = decode_text_payload(&record.payload)
                .ok_or_else(|| NfcError::WriteFailed("invalid text record payload".into()))?;
            self.device
                .PublishMessage(
                    &windows::core::HSTRING::from("Windows.NDEF"),
                    &windows::core::HSTRING::from(text),
                )
                .map_err(|e| NfcError::WriteFailed(format!("PublishMessage failed: {e}")))?
        } else {
            return Err(NfcError::WriteFailed(
                "only text/uri NDEF records are supported on Windows backend".into(),
            ));
        };

        let mut publish_id = self
            .publish_id
            .lock()
            .map_err(|_| NfcError::PlatformError("publish_id mutex poisoned".into()))?;
        if let Some(old_id) = publish_id.replace(next_publish_id) {
            let _ = self.device.StopPublishingMessage(old_id);
        }
        drop(publish_id);
        Ok(())
    }

    pub fn stop(&self) {
        let _ = self.device.StopSubscribingForMessage(self.subscription_id);
        if let Ok(mut publish_id) = self.publish_id.lock()
            && let Some(id) = publish_id.take()
        {
            let _ = self.device.StopPublishingMessage(id);
        }
    }
}

impl Drop for NfcReaderInner {
    fn drop(&mut self) {
        self.stop();
    }
}
