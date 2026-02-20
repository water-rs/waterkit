use crate::{DeepLink, DeepLinkError};

#[allow(clippy::unused_async)]
pub async fn open_url(_url: &str) -> Result<(), DeepLinkError> {
    Err(DeepLinkError::NotSupported)
}

#[allow(clippy::unused_async)]
pub async fn can_open_url(_url: &str) -> Result<bool, DeepLinkError> {
    Err(DeepLinkError::NotSupported)
}

#[derive(Debug)]
pub struct DeepLinkHandlerInner;

impl DeepLinkHandlerInner {
    #[allow(clippy::unused_async)]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        Err(DeepLinkError::NotSupported)
    }

    pub fn initial_link(&self) -> Result<Option<DeepLink>, DeepLinkError> {
        Err(DeepLinkError::NotSupported)
    }

    pub fn stop(&self) {}
}
