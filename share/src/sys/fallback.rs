use crate::{ShareError, ShareResult, ShareSheet};

#[allow(clippy::unused_async)]
pub async fn show_share_sheet(_sheet: ShareSheet) -> Result<ShareResult, ShareError> {
    Err(ShareError::NotSupported)
}
