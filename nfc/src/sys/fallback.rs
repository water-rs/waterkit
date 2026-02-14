use crate::{NdefMessage, NfcError, NfcTag};

pub fn nfc_is_available() -> bool {
    false
}

#[derive(Debug)]
pub struct NfcReaderInner;

impl NfcReaderInner {
    #[allow(clippy::unused_async)]
    pub async fn start_session(
        _message: &str,
    ) -> Result<(Self, async_channel::Receiver<NfcTag>), NfcError> {
        Err(NfcError::NotSupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn write(&self, _message: NdefMessage) -> Result<(), NfcError> {
        Err(NfcError::NotSupported)
    }

    pub fn stop(&self) {}
}
