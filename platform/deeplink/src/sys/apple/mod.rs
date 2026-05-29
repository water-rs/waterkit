use crate::{DeepLink, DeepLinkError};

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn deeplink_open_url(url: &str, callback: Box<dyn FnOnce(bool) -> ()>);
        fn deeplink_can_open_url(url: &str) -> bool;
        fn deeplink_start_listener(link_ctx: u64);
        fn deeplink_stop_listener();
        fn deeplink_get_initial_link() -> Option<String>;
    }

    extern "Rust" {
        fn on_deeplink_received_raw(link_ctx: u64, url: &str);
    }
}

#[allow(clippy::cast_possible_truncation)]
fn on_deeplink_received_raw(link_ctx: u64, url: &str) {
    let tx = unsafe { &*(link_ctx as usize as *const async_channel::Sender<DeepLink>) };
    if let Ok(link) = DeepLink::parse(url) {
        let _ = tx.try_send(link);
    }
}

pub async fn open_url(url: &str) -> Result<(), DeepLinkError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::deeplink_open_url(
        url,
        Box::new(move |success: bool| {
            if success {
                let _ = tx.send(Ok(()));
            } else {
                let _ = tx.send(Err(DeepLinkError::Platform("failed to open URL".into())));
            }
        }),
    );
    rx.await
        .map_err(|_| DeepLinkError::Platform("callback dropped".into()))?
}

#[allow(clippy::unused_async)]
pub async fn can_open_url(url: &str) -> Result<bool, DeepLinkError> {
    Ok(ffi::deeplink_can_open_url(url))
}

#[derive(Debug)]
pub struct DeepLinkHandlerInner {
    _link_tx: Box<async_channel::Sender<DeepLink>>,
}

impl DeepLinkHandlerInner {
    #[allow(clippy::unused_async)]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        let (link_tx, link_rx) = async_channel::bounded(16);
        let link_tx = Box::new(link_tx);
        let link_ctx = (&raw const *link_tx) as usize as u64;
        ffi::deeplink_start_listener(link_ctx);
        Ok((Self { _link_tx: link_tx }, link_rx))
    }

    #[allow(clippy::unused_self)]
    pub fn initial_link(&self) -> Result<Option<DeepLink>, DeepLinkError> {
        ffi::deeplink_get_initial_link()
            .map(|url| DeepLink::parse(&url))
            .transpose()
    }

    #[allow(clippy::unused_self)]
    pub fn stop(&self) {
        ffi::deeplink_stop_listener();
    }
}
