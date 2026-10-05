use std::path::{Path, PathBuf};

use crate::ClipboardError;
use crate::content::Image;
use wasm_bindgen_futures::wasm_bindgen::JsValue;
use wasm_bindgen_futures::{JsFuture, spawn_local};

#[derive(Debug)]
pub struct ClipboardInner;

#[expect(
    clippy::unused_self,
    reason = "the browser clipboard is global, so this backend keeps no state, but `Clipboard` calls every backend through the same `&self` methods"
)]
impl ClipboardInner {
    pub fn new() -> Result<Self, ClipboardError> {
        let _ = browser_clipboard()?;
        Ok(Self)
    }

    // The browser clipboard lists its formats only asynchronously
    // (`navigator.clipboard.read()`), so no synchronous query can answer.

    pub fn has_text(&self) -> Result<bool, ClipboardError> {
        Err(ClipboardError::UnsupportedType("text/plain".into()))
    }

    pub fn has_html(&self) -> Result<bool, ClipboardError> {
        Err(ClipboardError::UnsupportedType("text/html".into()))
    }

    pub fn has_files(&self) -> Result<bool, ClipboardError> {
        Err(ClipboardError::UnsupportedType("files".into()))
    }

    pub fn has_image(&self) -> Result<bool, ClipboardError> {
        Err(ClipboardError::UnsupportedType("image".into()))
    }

    #[expect(
        clippy::future_not_send,
        reason = "awaits `navigator.clipboard.readText()` through a `JsFuture`, whose shared state is an `Rc<RefCell<_>>` holding JS callbacks"
    )]
    pub async fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        let promise = browser_clipboard()?.read_text();
        let value = JsFuture::from(promise)
            .await
            .map_err(|error| js_error(&error))?;
        let text = value.as_string().ok_or_else(|| {
            ClipboardError::Platform(format!("readText resolved to a non-string: {value:?}"))
        })?;
        Ok(Some(text).filter(|text| !text.is_empty()))
    }

    #[expect(
        clippy::unused_async,
        reason = "`Clipboard` awaits every backend's reads; the browser reads only text, asynchronously"
    )]
    pub async fn get_html(&self) -> Result<Option<String>, ClipboardError> {
        Err(ClipboardError::UnsupportedType("text/html".into()))
    }

    #[expect(
        clippy::unused_async,
        reason = "`Clipboard` awaits every backend's reads; the browser reads only text, asynchronously"
    )]
    pub async fn get_files(&self) -> Result<Vec<PathBuf>, ClipboardError> {
        Err(ClipboardError::UnsupportedType("files".into()))
    }

    #[expect(
        clippy::unused_async,
        reason = "`Clipboard` awaits every backend's reads; the browser reads only text, asynchronously"
    )]
    pub async fn get_image(&self) -> Result<Option<Image>, ClipboardError> {
        Err(ClipboardError::UnsupportedType("image".into()))
    }

    #[expect(
        clippy::unused_async,
        reason = "`Clipboard` awaits every backend's reads; the browser reads only text, asynchronously"
    )]
    pub async fn get_binary(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        Err(ClipboardError::UnsupportedType(mime.to_string()))
    }

    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let promise = browser_clipboard()?.write_text(text);
        spawn_local(async move {
            if let Err(error) = JsFuture::from(promise).await {
                tracing::warn!("waterkit clipboard write_text failed: {:?}", error);
            }
        });
        Ok(())
    }

    pub fn set_html(&self, _html: &str, _alt_text: Option<&str>) -> Result<(), ClipboardError> {
        Err(ClipboardError::UnsupportedType("text/html".into()))
    }

    pub fn set_files(&self, _files: &[PathBuf]) -> Result<(), ClipboardError> {
        Err(ClipboardError::UnsupportedType("files".into()))
    }

    pub fn set_image_from_path(&self, _path: &Path) -> Result<(), ClipboardError> {
        Err(ClipboardError::UnsupportedType("image".into()))
    }

    pub fn set_binary(&self, _data: &[u8], mime: &str) -> Result<(), ClipboardError> {
        Err(ClipboardError::UnsupportedType(mime.to_string()))
    }

    pub fn set_file_promise(
        &self,
        _provider: Box<dyn FnOnce() -> Result<PathBuf, ClipboardError> + Send + 'static>,
    ) -> Result<(), ClipboardError> {
        Err(ClipboardError::UnsupportedType("file-promise".into()))
    }

    pub fn clear(&self) -> Result<(), ClipboardError> {
        self.set_text("")
    }
}

fn browser_clipboard() -> Result<web_sys::Clipboard, ClipboardError> {
    let window = web_sys::window().ok_or(ClipboardError::Unavailable)?;
    Ok(window.navigator().clipboard())
}

fn js_error(error: &JsValue) -> ClipboardError {
    ClipboardError::Platform(format!("{error:?}"))
}
