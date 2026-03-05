use crate::{ShareError, ShareItem, ShareResult, ShareSheet};
use std::borrow::Cow;

#[allow(clippy::unused_async)]
pub async fn show_share_sheet(sheet: ShareSheet) -> Result<ShareResult, ShareError> {
    // Windows DataTransferManager requires a CoreWindow context.
    // Fallback to clipboard + open for basic sharing.
    let Some(item) = sheet.items.first() else {
        unreachable!("waterkit-share: non-empty share request had no supported item variant");
    };
    let target: Cow<'_, str> = match item {
        ShareItem::Url(url) => Cow::Borrowed(url.as_str()),
        ShareItem::File(path) | ShareItem::Image(path) => Cow::Borrowed(
            path.to_str()
                .ok_or_else(|| ShareError::PlatformError("invalid path".into()))?,
        ),
        ShareItem::Text(text) => Cow::Owned(crate::mailto_url(sheet.subject.as_deref(), text)),
    };
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", target.as_ref()])
        .spawn()
        .map_err(|e| ShareError::PlatformError(e.to_string()))?;
    Ok(ShareResult::Shared)
}
