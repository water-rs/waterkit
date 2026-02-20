use crate::{DeepLink, DeepLinkError};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

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

type SubscriberList = Vec<(u64, async_channel::Sender<DeepLink>)>;

static SUBSCRIBERS: OnceLock<Mutex<SubscriberList>> = OnceLock::new();
static NEXT_SUBSCRIBER_ID: AtomicU64 = AtomicU64::new(1);
static INITIAL_URL: OnceLock<Option<String>> = OnceLock::new();

fn subscribers() -> &'static Mutex<SubscriberList> {
    SUBSCRIBERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn first_deeplink_arg_from_iter<I>(args: I) -> Option<String>
where
    I: IntoIterator<Item = String>,
{
    args.into_iter()
        .find_map(|arg| DeepLink::parse(&arg).ok().map(|_| arg))
}

fn process_initial_url() -> Option<String> {
    INITIAL_URL
        .get_or_init(|| first_deeplink_arg_from_iter(std::env::args().skip(1)))
        .clone()
}

fn register_subscriber(sender: async_channel::Sender<DeepLink>) -> u64 {
    let subscriber_id = NEXT_SUBSCRIBER_ID.fetch_add(1, Ordering::Relaxed);
    let mut guard = subscribers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.retain(|(_, tx)| !tx.is_closed());
    guard.push((subscriber_id, sender));
    subscriber_id
}

fn unregister_subscriber(subscriber_id: u64) {
    let mut guard = subscribers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.retain(|(id, tx)| *id != subscriber_id && !tx.is_closed());
}

fn broadcast_link(link: &DeepLink) {
    let mut guard = subscribers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.retain(|(_, tx)| !tx.is_closed());
    for (_, tx) in guard.iter() {
        let _ = tx.try_send(link.clone());
    }
}

/// Forward an incoming URL to all active deep-link handlers.
///
/// This is useful on Windows app frameworks that expose protocol activation
/// callbacks, allowing the host to push subsequent deep links into Rust.
///
/// # Errors
/// Returns [`DeepLinkError::InvalidUrl`] when `url` is malformed.
pub fn notify_incoming_url(url: &str) -> Result<(), DeepLinkError> {
    let link = DeepLink::parse(url)?;
    broadcast_link(&link);
    Ok(())
}

#[derive(Debug)]
pub struct DeepLinkHandlerInner {
    subscriber_id: u64,
    initial_url: Option<String>,
    stopped: AtomicBool,
}

impl DeepLinkHandlerInner {
    #[allow(clippy::unused_async)]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        let (link_tx, link_rx) = async_channel::bounded(16);
        let subscriber_id = register_subscriber(link_tx.clone());
        let initial_url = process_initial_url();

        if let Some(url) = initial_url.as_deref()
            && let Ok(link) = DeepLink::parse(url)
        {
            let _ = link_tx.try_send(link);
        }

        Ok((
            Self {
                subscriber_id,
                initial_url,
                stopped: AtomicBool::new(false),
            },
            link_rx,
        ))
    }

    pub fn initial_link(&self) -> Result<Option<DeepLink>, DeepLinkError> {
        self.initial_url.as_deref().map(DeepLink::parse).transpose()
    }

    pub fn stop(&self) {
        if !self.stopped.swap(true, Ordering::Relaxed) {
            unregister_subscriber(self.subscriber_id);
        }
    }
}

impl Drop for DeepLinkHandlerInner {
    fn drop(&mut self) {
        if !self.stopped.swap(true, Ordering::Relaxed) {
            unregister_subscriber(self.subscriber_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::first_deeplink_arg_from_iter;

    #[test]
    fn finds_first_deeplink_argument() {
        let args = vec![
            "--foo=bar".to_owned(),
            "waterkit://open/path?x=1".to_owned(),
            "https://example.com/ignored".to_owned(),
        ];
        let url = first_deeplink_arg_from_iter(args);
        assert_eq!(url.as_deref(), Some("waterkit://open/path?x=1"));
    }

    #[test]
    fn returns_none_without_deeplink_argument() {
        let args = vec!["--foo".to_owned(), "value".to_owned()];
        let url = first_deeplink_arg_from_iter(args);
        assert!(url.is_none());
    }
}
