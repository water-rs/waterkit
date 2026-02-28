use crate::{DeepLink, DeepLinkError};

#[allow(clippy::unused_async)]
pub async fn open_url(url: &str) -> Result<(), DeepLinkError> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map_err(|e| DeepLinkError::PlatformError(e.to_string()))?;
    Ok(())
}

#[allow(clippy::unused_async)]
pub async fn can_open_url(_url: &str) -> Result<bool, DeepLinkError> {
    // On Linux, xdg-open handles most URL schemes
    Ok(true)
}

#[derive(Debug)]
pub struct DeepLinkHandlerInner;

impl DeepLinkHandlerInner {
    #[allow(clippy::unused_async)]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        Err(DeepLinkError::NotSupported)
    }

    pub const fn initial_link(&self) -> Result<Option<DeepLink>, DeepLinkError> {
        let _ = self;
        Err(DeepLinkError::NotSupported)
    }

    pub const fn stop(&self) {
        let _ = self;
    }
}
