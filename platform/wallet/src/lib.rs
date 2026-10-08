//! Add server-signed passes to Google Wallet or Apple Wallet.
//!
//! Android accepts a signed [`GoogleWalletJwt`]. One JWT can contain several
//! wallet objects. The save flow runs in `SavePassesActivity`, a library-owned
//! trampoline activity the crate declares for the host application through its
//! Android manifest metadata; it is launched through the shared activity-result
//! bridge, so the published Android context must be an
//! `androidx.activity.ComponentActivity` and hosts do not forward
//! `onActivityResult`.
//!
//! iOS accepts one or more signed [`ApplePass`] archives. Payload constructors
//! validate structure only; Google Wallet or `PassKit` performs signature
//! verification.
//!
//! macOS, Windows, Linux, and other targets report that wallet is unavailable
//! and do not provide an `add` function. `PassKit`'s add-pass review controller
//! is available only on iOS, not macOS.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod payload;
mod sys;

pub use payload::{ApplePass, ApplePasses, GoogleWalletJwt, JwtError, JwtSegment, PkpassError};

use waterkit_core::Capabilities;

/// Whether passes can currently be added through a native wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct WalletCapabilities {
    /// Whether the native wallet flow is available on this device.
    pub available: bool,
}

impl Capabilities for WalletCapabilities {
    fn available(&self) -> bool {
        self.available
    }
}

/// Probes whether this device can add wallet passes.
///
/// Android queries the Google Wallet API asynchronously. A failed probe is
/// returned as an error rather than being reported as unavailable.
///
/// # Errors
///
/// Returns [`WalletError`] if the platform capability query fails.
pub async fn capabilities() -> Result<WalletCapabilities, WalletError> {
    tracing::debug!("wallet capability probe started");
    let capabilities = sys::capabilities().await?;
    tracing::debug!(
        available = capabilities.available,
        "wallet capability probe completed"
    );
    Ok(capabilities)
}

/// Result of presenting a native add-pass flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AddOutcome {
    /// The pass or passes were added.
    Added,
    /// The user cancelled the add-pass flow.
    Cancelled,
}

/// Errors from wallet capability probes or add-pass flows.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WalletError {
    /// Wallet cannot add passes on this device.
    #[error("wallet cannot add passes on this device")]
    Unavailable,
    /// The native layer rejected a pass.
    #[error("native wallet rejected pass: {0}")]
    InvalidPass(String),
    /// Another add flow is still pending.
    #[error("another wallet add flow is already in progress")]
    InProgress,
    /// A platform operation failed.
    #[error("wallet platform error: {0}")]
    Platform(String),
    /// An Android activity-result operation failed.
    #[cfg(target_os = "android")]
    #[error(transparent)]
    ActivityResult(#[from] waterkit_build::ActivityResultError),
}

#[cfg(target_os = "android")]
/// Payload accepted by [`add`] on Android.
pub type PlatformPasses = GoogleWalletJwt;

#[cfg(target_os = "ios")]
/// Payload accepted by [`add`] on iOS.
pub type PlatformPasses = ApplePasses;

/// Presents the platform's native wallet add-pass flow.
///
/// Android takes a single JWT; that token may describe multiple objects.
/// iOS takes one or more validated `.pkpass` archives.
///
/// # Errors
///
/// Returns [`WalletError`] when the platform cannot present or complete the
/// flow.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub async fn add(passes: PlatformPasses) -> Result<AddOutcome, WalletError> {
    tracing::debug!("wallet add flow started");
    let outcome = sys::add(passes).await;
    match &outcome {
        Ok(value) => tracing::debug!(?value, "wallet add flow completed"),
        Err(error) => tracing::debug!(%error, "wallet add flow failed"),
    }
    outcome
}
