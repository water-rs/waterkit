use crate::{ShareError, ShareItem, ShareResult, ShareSheet};

#[allow(clippy::unused_async)]
pub async fn show_share_sheet(sheet: ShareSheet) -> Result<ShareResult, ShareError> {
    // Use xdg-open for URLs and files via the freedesktop portal
    for item in &sheet.items {
        match item {
            ShareItem::Url(url) => {
                std::process::Command::new("xdg-open")
                    .arg(url)
                    .spawn()
                    .map_err(|e| ShareError::PlatformError(e.to_string()))?;
                return Ok(ShareResult::Shared);
            }
            ShareItem::File(path) | ShareItem::Image(path) => {
                let path_str = path
                    .to_str()
                    .ok_or_else(|| ShareError::PlatformError("invalid path".into()))?;
                std::process::Command::new("xdg-open")
                    .arg(path_str)
                    .spawn()
                    .map_err(|e| ShareError::PlatformError(e.to_string()))?;
                return Ok(ShareResult::Shared);
            }
            ShareItem::Text(text) => {
                let mailto = crate::mailto_url(sheet.subject.as_deref(), text);
                std::process::Command::new("xdg-open")
                    .arg(mailto)
                    .spawn()
                    .map_err(|e| ShareError::PlatformError(e.to_string()))?;
                return Ok(ShareResult::Shared);
            }
        }
    }
    Err(ShareError::NotSupported)
}
