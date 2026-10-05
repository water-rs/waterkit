use crate::{DeepLink, DeepLinkError};

#[allow(clippy::unused_async)]
pub async fn open_url(_url: &str) -> Result<(), DeepLinkError> {
    Err(DeepLinkError::Unsupported)
}

#[allow(clippy::unused_async)]
pub async fn can_open_url(_url: &str) -> Result<bool, DeepLinkError> {
    Err(DeepLinkError::Unsupported)
}

#[derive(Debug)]
pub struct DeepLinkHandlerInner;

#[expect(
    clippy::unused_self,
    clippy::missing_const_for_fn,
    reason = "this unsupported-platform shim keeps no state and computes nothing, but the facade calls every platform's backend through the same non-const `&self` methods"
)]
impl DeepLinkHandlerInner {
    #[allow(clippy::unused_async)]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        Err(DeepLinkError::Unsupported)
    }

    pub fn initial_link(&self) -> Result<Option<DeepLink>, DeepLinkError> {
        Err(DeepLinkError::Unsupported)
    }

    pub fn stop(&self) {}
}
