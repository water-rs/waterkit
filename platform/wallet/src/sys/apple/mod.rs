//! iOS Apple Wallet integration through `PassKit`.

use futures::channel::oneshot;

use crate::{AddOutcome, ApplePasses, WalletCapabilities, WalletError};

#[swift_bridge::bridge]
mod ffi {
    extern "Rust" {
        type AddRequest;
        fn pass_count(self: &AddRequest) -> usize;
        fn pass_data(self: &AddRequest, index: usize) -> &[u8];

        type AddCallback;
        fn on_added(self: AddCallback);
        fn on_cancelled(self: AddCallback);
        fn on_unavailable(self: AddCallback);
        fn on_invalid_pass(self: AddCallback, index: usize, message: String);
        fn on_error(self: AddCallback, message: String);
    }

    extern "Swift" {
        fn wallet_is_available() -> bool;
        fn wallet_add(request: AddRequest, callback: AddCallback);
    }
}

pub struct AddRequest {
    passes: ApplePasses,
}

impl AddRequest {
    const fn pass_count(&self) -> usize {
        self.passes.len()
    }

    fn pass_data(&self, index: usize) -> &[u8] {
        self.passes
            .iter()
            .nth(index)
            .unwrap_or_else(|| panic!("waterkit-wallet: pass index out of range: {index}"))
            .as_bytes()
    }
}

pub struct AddCallback {
    sender: oneshot::Sender<Result<AddOutcome, WalletError>>,
}

impl AddCallback {
    fn on_added(self) {
        let _ = self.sender.send(Ok(AddOutcome::Added));
    }

    fn on_cancelled(self) {
        let _ = self.sender.send(Ok(AddOutcome::Cancelled));
    }

    fn on_unavailable(self) {
        let _ = self.sender.send(Err(WalletError::Unavailable));
    }

    fn on_invalid_pass(self, index: usize, mut message: String) {
        message.insert_str(0, &format!("pass at index {index}: "));
        let _ = self.sender.send(Err(WalletError::InvalidPass(message)));
    }

    fn on_error(self, message: String) {
        let _ = self.sender.send(Err(WalletError::Platform(message)));
    }
}

pub async fn capabilities() -> Result<WalletCapabilities, WalletError> {
    Ok(WalletCapabilities {
        available: ffi::wallet_is_available(),
    })
}

pub async fn add(passes: ApplePasses) -> Result<AddOutcome, WalletError> {
    let (sender, receiver) = oneshot::channel();
    ffi::wallet_add(AddRequest { passes }, AddCallback { sender });
    receiver
        .await
        .map_err(|_| WalletError::Platform("iOS wallet callback channel closed".into()))?
}
