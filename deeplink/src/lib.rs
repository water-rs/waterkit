//! Cross-platform deep linking and URL scheme handling.
//!
//! Provides APIs for:
//! - Receiving incoming deep links (custom URL schemes and universal links)
//! - Opening URLs in external apps
//! - Checking if a URL can be handled

#![warn(missing_docs)]

mod sys;

use std::collections::HashMap;

/// A parsed deep link.
#[derive(Debug, Clone)]
pub struct DeepLink {
    /// The full URL string.
    pub url: String,
    /// The URL scheme (e.g., "myapp", "https").
    pub scheme: String,
    /// The host component.
    pub host: Option<String>,
    /// The path component.
    pub path: String,
    /// Query parameters.
    pub query_params: HashMap<String, String>,
}

impl DeepLink {
    /// Parse a URL string into a `DeepLink`.
    ///
    /// # Errors
    /// Returns error if the URL is malformed.
    pub fn parse(url: &str) -> Result<Self, DeepLinkError> {
        let parts: Vec<&str> = url.splitn(2, "://").collect();
        if parts.len() < 2 {
            return Err(DeepLinkError::InvalidUrl(url.to_string()));
        }

        let scheme = parts[0].to_string();
        let rest = parts[1];
        let (host_and_path, query_string) = rest.split_once('?').unwrap_or((rest, ""));
        let (host, path) = host_and_path.find('/').map_or_else(
            || (Some(host_and_path.to_string()), String::new()),
            |idx| {
                (
                    Some(host_and_path[..idx].to_string()),
                    host_and_path[idx..].to_string(),
                )
            },
        );

        let query_params = if query_string.is_empty() {
            HashMap::new()
        } else {
            query_string
                .split('&')
                .filter_map(|pair| {
                    let mut kv = pair.splitn(2, '=');
                    Some((kv.next()?.to_string(), kv.next().unwrap_or("").to_string()))
                })
                .collect()
        };

        Ok(Self {
            url: url.to_string(),
            scheme,
            host,
            path,
            query_params,
        })
    }
}

/// Deep link handler that receives incoming links.
#[derive(Debug)]
pub struct DeepLinkHandler {
    inner: sys::DeepLinkHandlerInner,
}

impl DeepLinkHandler {
    /// Start listening for incoming deep links.
    ///
    /// # Errors
    /// Returns error if the handler cannot be initialized.
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        let (inner, rx) = sys::DeepLinkHandlerInner::start().await?;
        Ok((Self { inner }, rx))
    }

    /// Get the initial deep link that launched the app (if any).
    ///
    /// # Errors
    /// Returns error if the initial link cannot be retrieved.
    pub fn initial_link(&self) -> Result<Option<DeepLink>, DeepLinkError> {
        self.inner.initial_link()
    }

    /// Stop listening for deep links.
    pub fn stop(&self) {
        self.inner.stop();
    }
}

/// Open a URL in the default handler (browser, app, etc.).
///
/// # Errors
/// Returns error if the URL cannot be opened.
pub async fn open_url(url: &str) -> Result<(), DeepLinkError> {
    sys::open_url(url).await
}

/// Check if a URL scheme can be handled by an installed app.
///
/// # Errors
/// Returns error if the check fails.
pub async fn can_open_url(url: &str) -> Result<bool, DeepLinkError> {
    sys::can_open_url(url).await
}

/// Errors in deep linking operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DeepLinkError {
    /// Invalid URL.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    /// Not supported.
    #[error("not supported")]
    NotSupported,
    /// Permission denied.
    #[error("permission denied")]
    PermissionDenied,
    /// Platform error.
    #[error("platform error: {0}")]
    PlatformError(String),
}
