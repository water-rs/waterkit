use crate::sys::desktop_common::DesktopDeepLinkHandlerInner;
use crate::{DeepLink, DeepLinkError};

#[allow(clippy::unused_async)]
pub async fn open_url(url: &str) -> Result<(), DeepLinkError> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map_err(|e| DeepLinkError::Platform(e.to_string()))?;
    Ok(())
}

#[allow(clippy::unused_async)]
pub async fn can_open_url(_url: &str) -> Result<bool, DeepLinkError> {
    // On Linux, xdg-open handles most URL schemes
    Ok(true)
}

#[derive(Debug)]
pub struct DeepLinkHandlerInner {
    inner: DesktopDeepLinkHandlerInner,
}

impl DeepLinkHandlerInner {
    #[allow(clippy::unused_async)]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        let (inner, rx) = DesktopDeepLinkHandlerInner::start().await?;
        Ok((Self { inner }, rx))
    }

    pub fn initial_link(&self) -> Result<Option<DeepLink>, DeepLinkError> {
        self.inner.initial_link()
    }

    pub fn stop(&self) {
        self.inner.stop();
    }
}
