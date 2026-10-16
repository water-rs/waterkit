use crate::{WalletCapabilities, WalletError};

pub async fn capabilities() -> Result<WalletCapabilities, WalletError> {
    Ok(WalletCapabilities { available: false })
}
