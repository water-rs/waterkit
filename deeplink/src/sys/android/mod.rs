use crate::{DeepLink, DeepLinkError};

#[allow(clippy::unused_async)]
pub async fn open_url(_url: &str) -> Result<(), DeepLinkError> {
    Err(DeepLinkError::PlatformError(
        "Android: use JNI context directly".into(),
    ))
}

#[allow(clippy::unused_async)]
pub async fn can_open_url(_url: &str) -> Result<bool, DeepLinkError> {
    Err(DeepLinkError::PlatformError(
        "Android: use JNI context directly".into(),
    ))
}

#[derive(Debug)]
pub struct DeepLinkHandlerInner;

impl DeepLinkHandlerInner {
    #[allow(clippy::unused_async)]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        Err(DeepLinkError::PlatformError(
            "Android: use JNI context directly".into(),
        ))
    }

    pub fn initial_link(&self) -> Result<Option<DeepLink>, DeepLinkError> {
        Err(DeepLinkError::NotSupported)
    }

    pub fn stop(&self) {}
}
