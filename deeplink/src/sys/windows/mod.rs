use crate::{DeepLink, DeepLinkError};

#[allow(clippy::unused_async)]
pub async fn open_url(url: &str) -> Result<(), DeepLinkError> {
    let uri: windows::core::HSTRING = url.into();
    let win_uri = windows::Foundation::Uri::CreateUri(&uri)
        .map_err(|e| DeepLinkError::PlatformError(e.to_string()))?;
    windows::System::Launcher::LaunchUriAsync(&win_uri)
        .map_err(|e| DeepLinkError::PlatformError(e.to_string()))?
        .await
        .map_err(|e| DeepLinkError::PlatformError(e.to_string()))?;
    Ok(())
}

#[allow(clippy::unused_async)]
pub async fn can_open_url(url: &str) -> Result<bool, DeepLinkError> {
    let uri: windows::core::HSTRING = url.into();
    let win_uri = windows::Foundation::Uri::CreateUri(&uri)
        .map_err(|e| DeepLinkError::PlatformError(e.to_string()))?;
    let result = windows::System::Launcher::QueryUriSupportAsync(
        &win_uri,
        windows::System::LaunchQuerySupportType::Uri,
    )
    .map_err(|e| DeepLinkError::PlatformError(e.to_string()))?
    .await
    .map_err(|e| DeepLinkError::PlatformError(e.to_string()))?;
    Ok(result == windows::System::LaunchQuerySupportStatus::Available)
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
