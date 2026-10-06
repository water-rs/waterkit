use crate::{NdefMessage, NfcError, NfcTag};

#[expect(
    clippy::missing_const_for_fn,
    reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
)]
pub fn nfc_is_available() -> bool {
    false
}

#[derive(Debug)]
pub struct NfcReaderInner;

#[expect(
    clippy::unused_self,
    clippy::missing_const_for_fn,
    reason = "this unsupported-platform shim keeps no state and computes nothing, but the facade calls every platform's backend through the same non-const `&self` methods"
)]
impl NfcReaderInner {
    #[allow(clippy::unused_async)]
    pub async fn start_session(
        _message: &str,
    ) -> Result<(Self, async_channel::Receiver<NfcTag>), NfcError> {
        Err(NfcError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn write(&self, _message: NdefMessage) -> Result<(), NfcError> {
        Err(NfcError::Unsupported)
    }

    pub fn stop(&self) {}
}
