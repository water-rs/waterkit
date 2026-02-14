use crate::{ShareError, ShareItem, ShareResult, ShareSheet};

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn share_show_sheet(
            items_json: &str,
            subject: Option<String>,
            callback: Box<dyn FnOnce(bool) -> ()>,
        );
    }
}

pub async fn show_share_sheet(sheet: ShareSheet) -> Result<ShareResult, ShareError> {
    let items_json = serialize_items(&sheet.items)?;
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::share_show_sheet(
        &items_json,
        sheet.subject,
        Box::new(move |shared: bool| {
            let _ = tx.send(if shared {
                ShareResult::Shared
            } else {
                ShareResult::Cancelled
            });
        }),
    );
    rx.await
        .map_err(|_| ShareError::PlatformError("callback dropped".into()))
}

fn serialize_items(items: &[ShareItem]) -> Result<String, ShareError> {
    let mut parts = Vec::new();
    for item in items {
        match item {
            ShareItem::Text(t) => parts.push(format!("text:{t}")),
            ShareItem::Url(u) => parts.push(format!("url:{u}")),
            ShareItem::Image(p) => {
                let path = p
                    .to_str()
                    .ok_or_else(|| ShareError::PlatformError("invalid path encoding".into()))?;
                parts.push(format!("image:{path}"));
            }
            ShareItem::File(p) => {
                let path = p
                    .to_str()
                    .ok_or_else(|| ShareError::PlatformError("invalid path encoding".into()))?;
                parts.push(format!("file:{path}"));
            }
        }
    }
    Ok(parts.join("\n"))
}
