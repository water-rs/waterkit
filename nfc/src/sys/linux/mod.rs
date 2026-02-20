use crate::{NdefMessage, NfcError, NfcTag};

pub fn nfc_is_available() -> bool {
    // Linux NFC via neard D-Bus interface
    // Check if neard service is available
    futures::executor::block_on(async {
        let conn = zbus::Connection::system().await.ok();
        conn.is_some()
            && zbus::fdo::DBusProxy::builder(conn.as_ref().unwrap())
                .build()
                .await
                .ok()
                .and_then(|proxy| {
                    futures::executor::block_on(
                        proxy.name_has_owner("org.neard".try_into().unwrap()),
                    )
                    .ok()
                })
                .unwrap_or(false)
    })
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
