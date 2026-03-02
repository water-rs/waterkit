use crate::{DeepLink, DeepLinkError};

#[derive(Debug)]
pub struct DesktopDeepLinkHandlerInner {
    initial_link: Option<DeepLink>,
    _link_tx: async_channel::Sender<DeepLink>,
}

fn parse_initial_link_from_args() -> Option<DeepLink> {
    std::env::args().skip(1).find_map(|arg| {
        if !arg.contains("://") {
            return None;
        }
        DeepLink::parse(&arg).ok()
    })
}

impl DesktopDeepLinkHandlerInner {
    #[allow(clippy::unused_async)]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        let (link_tx, link_rx) = async_channel::bounded(16);
        let initial_link = parse_initial_link_from_args();
        if let Some(link) = initial_link.clone() {
            let _ = link_tx.try_send(link);
        }
        Ok((
            Self {
                initial_link,
                _link_tx: link_tx,
            },
            link_rx,
        ))
    }

    #[must_use]
    pub fn initial_link(&self) -> Option<DeepLink> {
        self.initial_link.clone()
    }

    pub const fn stop(&self) {}
}
