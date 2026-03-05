use crate::{DeepLink, DeepLinkError};

#[derive(Debug)]
pub struct DesktopDeepLinkHandlerInner {
    initial_link: Option<String>,
    link_tx: async_channel::Sender<DeepLink>,
}

fn parse_initial_link_from_args() -> Option<String> {
    std::env::args().skip(1).find_map(|arg| {
        if !arg.contains("://") {
            return None;
        }
        Some(arg)
    })
}

impl DesktopDeepLinkHandlerInner {
    #[allow(clippy::unused_async)]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        let (link_tx, link_rx) = async_channel::bounded(16);
        let initial_link = parse_initial_link_from_args();
        if let Some(raw_link) = initial_link.as_deref() {
            let link = DeepLink::parse(raw_link)?;
            let _ = link_tx.try_send(link);
        }
        Ok((
            Self {
                initial_link,
                link_tx,
            },
            link_rx,
        ))
    }

    pub fn initial_link(&self) -> Result<Option<DeepLink>, DeepLinkError> {
        self.initial_link
            .as_deref()
            .map(DeepLink::parse)
            .transpose()
    }

    pub fn stop(&self) {
        let _ = self.link_tx.is_closed();
    }
}
