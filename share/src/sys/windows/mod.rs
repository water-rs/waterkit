use crate::{ShareError, ShareItem, ShareResult, ShareSheet};

#[allow(clippy::unused_async)]
pub async fn show_share_sheet(sheet: ShareSheet) -> Result<ShareResult, ShareError> {
    // Windows DataTransferManager requires a CoreWindow context.
    // Fallback to clipboard + open for basic sharing.
    for item in &sheet.items {
        match item {
            ShareItem::Url(url) => {
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "start", url])
                    .spawn()
                    .map_err(|e| ShareError::PlatformError(e.to_string()))?;
                return Ok(ShareResult::Shared);
            }
            ShareItem::File(path) | ShareItem::Image(path) => {
                let path_str = path
                    .to_str()
                    .ok_or_else(|| ShareError::PlatformError("invalid path".into()))?;
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "start", "", path_str])
                    .spawn()
                    .map_err(|e| ShareError::PlatformError(e.to_string()))?;
                return Ok(ShareResult::Shared);
            }
            ShareItem::Text(text) => {
                let mailto = crate::mailto_url(sheet.subject.as_deref(), text);
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "start", "", &mailto])
                    .spawn()
                    .map_err(|e| ShareError::PlatformError(e.to_string()))?;
                return Ok(ShareResult::Shared);
            }
        }
    }
    unreachable!("waterkit-share: non-empty share request had no supported item variant")
}
