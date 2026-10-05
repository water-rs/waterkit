//! Cross-platform clipboard access.
//!
//! This crate provides a unified API for interacting with the system clipboard
//! across macOS, Windows, Linux, Android, and iOS.
//!
//! # Example
//!
//! ```no_run
//! use waterkit_clipboard::Clipboard;
//!
//! # async fn example() -> Result<(), waterkit_clipboard::ClipboardError> {
//! let mut clipboard = Clipboard::new()?;
//!
//! // Check and read text
//! if clipboard.has_text()? {
//!     if let Some(text) = clipboard.text().await? {
//!         println!("Clipboard text: {text}");
//!     }
//! }
//!
//! // Set text
//! clipboard.set_text("Hello, clipboard!")?;
//! # Ok(())
//! # }
//! ```
//!
//! # Clipboard Watching
//!
//! ```no_run
//! use futures::StreamExt;
//! use waterkit_clipboard::Clipboard;
//!
//! # async fn example() -> Result<(), waterkit_clipboard::ClipboardError> {
//! let clipboard = Clipboard::new()?;
//! let mut stream = clipboard.watch()?;
//!
//! while let Some(event) = stream.next().await {
//!     println!("Clipboard changed! has_text={}", event.has_text());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Custom Data Types
//!
//! ```no_run
//! use waterkit_clipboard::{Clipboard, ClipboardData, ClipboardError};
//!
//! struct MyData {
//!     value: i32,
//! }
//!
//! impl ClipboardData for MyData {
//!     const MIME_TYPE: &'static str = "application/x-mydata";
//!
//!     fn encode(&self) -> Result<Vec<u8>, ClipboardError> {
//!         Ok(self.value.to_le_bytes().to_vec())
//!     }
//!
//!     fn decode(bytes: &[u8]) -> Result<Self, ClipboardError> {
//!         let arr: [u8; 4] = bytes.try_into()
//!             .map_err(|_| ClipboardError::Decode("invalid length".into()))?;
//!         Ok(Self { value: i32::from_le_bytes(arr) })
//!     }
//! }
//!
//! # async fn example() -> Result<(), ClipboardError> {
//! let mut clipboard = Clipboard::new()?;
//! clipboard.set_data(&MyData { value: 42 })?;
//! let data: Option<MyData> = clipboard.data().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Primary Selection (Linux only)
//!
//! Linux desktops also have a PRIMARY selection holding the text last
//! selected; it is pasted with the middle mouse button. [`PrimarySelection`]
//! reads and writes it under X11, and under Wayland through the data-control
//! protocol's primary-selection support. Its own documentation carries the
//! example.
//!
//! PRIMARY exists only on Linux desktops, so this API is compiled only for
//! `target_os = "linux"`; other platforms do not get it at all.
//!
//! # Linux Display Server
//!
//! On Linux, [`Clipboard::new`] and [`PrimarySelection::new`] each choose the
//! display server once, from the session, and every later operation on that
//! handle, watching included, uses it:
//!
//! - **Wayland** when `WAYLAND_DISPLAY` is set. The compositor must offer a
//!   data-control protocol (`ext_data_control_manager_v1` or
//!   `zwlr_data_control_manager_v1`); PRIMARY also needs it to provide a
//!   primary selection (`ext_data_control_manager_v1`, or
//!   `zwlr_data_control_manager_v1` version 2+). `new` checks by binding the
//!   compositor's registry. Without what it needs, `new` returns
//!   [`ClipboardError::Platform`] naming the reason. It never falls through to
//!   X11, even when an X server such as Xwayland is reachable: that server's
//!   selections are not the Wayland session's.
//! - **X11** when only `DISPLAY` is set.
//!
//! A variable set to the empty string counts as unset, so
//! `WAYLAND_DISPLAY= app` uses X11.
//!
//! Both display servers carry the same formats. Text is offered as
//! `text/plain;charset=utf-8`, `UTF8_STRING` and `text/plain`, and read from
//! the first of those the owner offers; HTML is `text/html`, images are
//! `image/png`, and files are a `text/uri-list` of `file:` URIs (with
//! `x-special/gnome-copied-files` and the paths as text alongside). Custom
//! data uses its MIME type, which on X11 is the target name.
//!
//! After a write the process owns the selection and keeps answering paste
//! requests from other clients until another client claims it, and:
//!
//! - **Wayland**: until the process exits, independent of the handle's
//!   lifetime; a thread inside the process serves data-control requests.
//! - **X11**: until the handle that wrote it, its clones and the watches
//!   started from them are all dropped; a thread inside the crate answers
//!   `SelectionRequest`s, sending large formats incrementally (`INCR`).
//!
//! # Platform Notes
//!
//! | Feature | Windows | Linux | macOS | iOS | Android |
//! |---------|---------|-------|-------|-----|---------|
//! | Text    | ✓       | ✓     | ✓     | ✓   | ✓       |
//! | HTML    | ✓       | ✓     | ✓     | ✓   | ✓       |
//! | Image   | ✓       | ✓     | ✓     | ✓   | ✓       |
//! | Files   | ✓       | ✓     | ✓     | ✓   | ✓       |
//! | Watch   | ✓       | ✓     | ✓     | ✓   | ✓       |
//! | PRIMARY | —       | ✓     | —     | —   | —       |
//!
//! ## Android
//!
//! On Android, the context is obtained automatically via `ndk_context`.
//! Your app must initialize `ndk_context` before using the clipboard.
//! If the context is not available, `Clipboard::new()` will panic.

#![warn(missing_docs)]

mod content;
mod error;
mod stream;
mod sys;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use content::{ClipboardData, ClipboardEvent, Image};
pub use error::ClipboardError;
pub use stream::ClipboardStream;

/// A handle to the system clipboard.
///
/// This struct provides methods to read and write clipboard content,
/// query available types, and watch for changes.
///
/// On Linux this is the CLIPBOARD selection of the display server
/// [`new`](Self::new) chose; see [Linux Display Server](crate#linux-display-server).
///
/// # Async Reads
///
/// All read operations are async because the clipboard source might be slow
/// (e.g., when using file promises or large data transfers).
///
/// # Sync Writes
///
/// Write operations are sync because we control the data being written.
#[derive(Debug, Clone)]
pub struct Clipboard {
    inner: Arc<sys::ClipboardInner>,
}

impl Clipboard {
    /// Create a new clipboard handle.
    ///
    /// On Linux this chooses the display server, as
    /// [Linux Display Server](crate#linux-display-server) describes.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed. On Linux that is
    /// [`ClipboardError::Platform`] when the session is a Wayland session whose
    /// compositor offers no data-control protocol (whether or not an X server
    /// is also reachable), when the chosen display server cannot be reached,
    /// and when neither `WAYLAND_DISPLAY` nor `DISPLAY` is set.
    ///
    /// # Panics
    ///
    /// On Android, panics if `ndk_context` is not initialized.
    pub fn new() -> Result<Self, ClipboardError> {
        Ok(Self {
            inner: Arc::new(sys::ClipboardInner::new()?),
        })
    }

    // ========== Query (sync - instant metadata checks) ==========

    /// Check if text content is available in the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard's formats cannot be listed, for
    /// example when the platform bridge fails or, on Linux, the display server
    /// is gone. A failure is never reported as an empty clipboard. In a
    /// browser, [`ClipboardError::UnsupportedType`]: the browser clipboard has
    /// no synchronous format query.
    pub fn has_text(&self) -> Result<bool, ClipboardError> {
        self.inner.has_text()
    }

    /// Check if HTML content is available in the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard's formats cannot be listed, as
    /// [`has_text`](Self::has_text) describes. In a browser,
    /// [`ClipboardError::UnsupportedType`].
    pub fn has_html(&self) -> Result<bool, ClipboardError> {
        self.inner.has_html()
    }

    /// Check if file paths are available in the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard's formats cannot be listed, as
    /// [`has_text`](Self::has_text) describes. In a browser,
    /// [`ClipboardError::UnsupportedType`].
    pub fn has_files(&self) -> Result<bool, ClipboardError> {
        self.inner.has_files()
    }

    /// Check if image data is available in the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard's formats cannot be listed, as
    /// [`has_text`](Self::has_text) describes. In a browser,
    /// [`ClipboardError::UnsupportedType`].
    pub fn has_image(&self) -> Result<bool, ClipboardError> {
        self.inner.has_image()
    }

    // ========== Read (async - source might be slow) ==========

    /// Get text content from the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub async fn text(&self) -> Result<Option<String>, ClipboardError> {
        #[cfg(target_arch = "wasm32")]
        {
            self.inner.get_text().await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let inner = Arc::clone(&self.inner);
            blocking::unblock(move || inner.get_text()).await
        }
    }

    /// Get HTML content from the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub async fn html(&self) -> Result<Option<String>, ClipboardError> {
        #[cfg(target_arch = "wasm32")]
        {
            self.inner.get_html().await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let inner = Arc::clone(&self.inner);
            blocking::unblock(move || inner.get_html()).await
        }
    }

    /// Get file paths from the clipboard.
    ///
    /// On Linux the paths come from the clipboard's `text/uri-list`; URIs of a
    /// scheme other than `file` name no local file and are left out. On iOS
    /// they are the pasteboard's file URLs. On Android they are the clip's
    /// `file://` URIs and the `content://` URIs this app's
    /// `ClipboardFileProvider` serves; other `content://` URIs name no path.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed. On Linux,
    /// [`ClipboardError::Decode`] when the URI list is malformed or names a
    /// file on another host.
    pub async fn files(&self) -> Result<Vec<PathBuf>, ClipboardError> {
        #[cfg(target_arch = "wasm32")]
        {
            self.inner.get_files().await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let inner = Arc::clone(&self.inner);
            blocking::unblock(move || inner.get_files()).await
        }
    }

    /// Get image content from the clipboard as raw RGBA pixels.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed or if
    /// image conversion fails.
    pub async fn image(&self) -> Result<Option<Image>, ClipboardError> {
        #[cfg(target_arch = "wasm32")]
        {
            self.inner.get_image().await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let inner = Arc::clone(&self.inner);
            blocking::unblock(move || inner.get_image()).await
        }
    }

    /// Get custom data from the clipboard.
    ///
    /// The type must implement [`ClipboardData`] to define its MIME type
    /// and encoding/decoding logic.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed or if
    /// decoding fails.
    pub async fn data<T: ClipboardData + Send + 'static>(
        &self,
    ) -> Result<Option<T>, ClipboardError> {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(bytes) = self.inner.get_binary(T::MIME_TYPE).await? {
                Ok(Some(T::decode(&bytes)?))
            } else {
                Ok(None)
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let inner = Arc::clone(&self.inner);
            blocking::unblock(move || {
                if let Some(bytes) = inner.get_binary(T::MIME_TYPE)? {
                    Ok(Some(T::decode(&bytes)?))
                } else {
                    Ok(None)
                }
            })
            .await
        }
    }

    /// Get raw binary data from the clipboard by MIME type.
    ///
    /// Use this when you want to handle encoding/decoding yourself.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub async fn binary(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        #[cfg(target_arch = "wasm32")]
        {
            self.inner.get_binary(mime).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let inner = Arc::clone(&self.inner);
            let mime = mime.to_string();
            blocking::unblock(move || inner.get_binary(&mime)).await
        }
    }

    // ========== Write (sync - we control the data) ==========

    /// Set text content to the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.inner.set_text(text)
    }

    /// Set HTML content to the clipboard.
    ///
    /// # Arguments
    ///
    /// * `html` - The HTML content to set.
    /// * `alt_text` - Optional plain text fallback for applications that don't support HTML.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub fn set_html(&mut self, html: &str, alt_text: Option<&str>) -> Result<(), ClipboardError> {
        self.inner.set_html(html, alt_text)
    }

    /// Set file paths to the clipboard.
    ///
    /// # Platform Notes
    ///
    /// - **iOS**: Each file is an item provider carrying its contents, which
    ///   other apps paste, and its file URL.
    /// - **Android**: Each file is a `content://` URI that other apps open
    ///   through the read access the clipboard grants them. The URI is served
    ///   by `waterkit.clipboard.ClipboardFileProvider`, which the app's
    ///   manifest must declare, under any authority:
    ///
    ///   ```xml
    ///   <provider
    ///       android:name="waterkit.clipboard.ClipboardFileProvider"
    ///       android:authorities="${applicationId}.waterkit.clipboard"
    ///       android:exported="false"
    ///       android:grantUriPermissions="true" />
    ///   ```
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed. On Linux, iOS and
    /// Android, [`ClipboardError::Encode`] when a path is not absolute, which a
    /// file URL needs, or on iOS and Android is not Unicode. On Android,
    /// [`ClipboardError::Platform`] when the manifest does not declare the
    /// provider as above.
    pub fn set_files(&mut self, files: &[PathBuf]) -> Result<(), ClipboardError> {
        self.inner.set_files(files)
    }

    /// Set image content to the clipboard from a file path.
    ///
    /// The image format is detected from the file extension.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed or if
    /// the image cannot be read. On Android the image goes on the clipboard
    /// as a file, which needs the provider [`set_files`](Self::set_files)
    /// describes.
    pub fn set_image(&mut self, path: &Path) -> Result<(), ClipboardError> {
        self.inner.set_image_from_path(path)
    }

    /// Set custom data to the clipboard.
    ///
    /// The type must implement [`ClipboardData`] to define its MIME type
    /// and encoding logic.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed or if
    /// encoding fails. On Android the data goes on the clipboard as a file,
    /// which needs the provider [`set_files`](Self::set_files) describes.
    pub fn set_data<T: ClipboardData>(&mut self, data: &T) -> Result<(), ClipboardError> {
        let bytes = data.encode()?;
        self.inner.set_binary(&bytes, T::MIME_TYPE)
    }

    /// Set raw binary data to the clipboard with a MIME type.
    ///
    /// Use this when you want to handle encoding yourself.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed. On Android the
    /// data goes on the clipboard as a file, which needs the provider
    /// [`set_files`](Self::set_files) describes.
    pub fn set_binary(&mut self, data: &[u8], mime: &str) -> Result<(), ClipboardError> {
        self.inner.set_binary(data, mime)
    }

    /// Set a file promise to the clipboard.
    ///
    /// The provider closure is called when the recipient requests the data.
    /// This is useful for lazy generation of clipboard content.
    ///
    /// # Platform Notes
    ///
    /// - **macOS/iOS**: Uses `NSFilePromiseProvider`
    /// - **Other platforms**: Falls back to immediate file copy
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub fn set_file_promise<F>(&mut self, provider: F) -> Result<(), ClipboardError>
    where
        F: FnOnce() -> Result<PathBuf, ClipboardError> + Send + 'static,
    {
        self.inner.set_file_promise(Box::new(provider))
    }

    // ========== Control ==========

    /// Clear all clipboard content.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be accessed.
    pub fn clear(&mut self) -> Result<(), ClipboardError> {
        self.inner.clear()
    }

    /// Watch for clipboard changes.
    ///
    /// Returns a stream that yields [`ClipboardEvent`]s whenever the
    /// clipboard content changes.
    ///
    /// # Platform Notes
    ///
    /// - **Windows/macOS**: Uses native clipboard change notifications.
    /// - **Linux**: Watches the display server this handle chose: the
    ///   compositor's data-control selection events on Wayland, `XFixes`
    ///   selection notifications on X11. The selection held when the watch
    ///   starts is not reported, only later changes. A failure of the display
    ///   server ends the stream and is logged through `tracing`.
    /// - **iOS**: Uses polling with `UIPasteboard.changeCount` (500ms interval).
    /// - **Android**: Uses `ClipboardManager.OnPrimaryClipChangedListener`; every clip
    ///   notification emits an event, including same-type content updates.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard watcher cannot be started.
    pub fn watch(&self) -> Result<ClipboardStream, ClipboardError> {
        let (receiver, shutdown) = sys::start_watch(&self.inner)?;
        Ok(ClipboardStream::new(receiver, shutdown))
    }
}

/// A handle to the Linux PRIMARY selection.
///
/// PRIMARY holds the text last selected and is pasted with the middle mouse
/// button. This type exists only on Linux (`cfg(target_os = "linux")`): the
/// selection has no equivalent on other platforms and is never emulated with
/// CLIPBOARD.
///
/// [`new`](Self::new) chooses the display server once, from the session, as
/// [Linux Display Server](crate#linux-display-server) describes; a Wayland
/// compositor must offer a primary selection to data-control clients.
///
/// After [`set_text`](Self::set_text) the process owns PRIMARY and serves it
/// for as long as [Linux Display Server](crate#linux-display-server)
/// describes: on X11 while this handle or a clone lives, on Wayland until the
/// process exits.
///
/// # Example
///
/// ```no_run
/// # async fn example() -> Result<(), waterkit_clipboard::ClipboardError> {
/// let mut primary = waterkit_clipboard::PrimarySelection::new()?;
/// primary.set_text("selected text")?;
/// assert_eq!(primary.text().await?.as_deref(), Some("selected text"));
/// # Ok(())
/// # }
/// ```
#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
pub struct PrimarySelection {
    inner: Arc<sys::PrimaryInner>,
}

#[cfg(target_os = "linux")]
impl PrimarySelection {
    /// Create a new PRIMARY selection handle.
    ///
    /// Chooses the display server as
    /// [Linux Display Server](crate#linux-display-server) describes: the
    /// Wayland compositor when `WAYLAND_DISPLAY` is set, the X server when
    /// only `DISPLAY` is.
    ///
    /// # Errors
    ///
    /// Returns [`ClipboardError::Platform`] when the session is a Wayland
    /// session whose compositor offers no primary selection to data-control
    /// clients (whether or not an X server is also reachable), when the
    /// chosen display server cannot be reached, and when neither
    /// `WAYLAND_DISPLAY` nor `DISPLAY` is set.
    pub fn new() -> Result<Self, ClipboardError> {
        Ok(Self {
            inner: Arc::new(sys::PrimaryInner::new()?),
        })
    }

    /// Get text content from the PRIMARY selection.
    ///
    /// Returns `None` when no client currently owns a PRIMARY selection, and
    /// on Wayland also when its owner offers no plain-text type. On X11 an owner
    /// that refuses to convert PRIMARY to `UTF8_STRING` reads as an empty
    /// string.
    ///
    /// # Errors
    ///
    /// Returns [`ClipboardError::Platform`] if the display server fails the
    /// read or, on X11, the owner does not answer in time, and
    /// [`ClipboardError::Decode`] if the text is not UTF-8.
    pub async fn text(&self) -> Result<Option<String>, ClipboardError> {
        let inner = Arc::clone(&self.inner);
        blocking::unblock(move || inner.get_text()).await
    }

    /// Set the PRIMARY selection text.
    ///
    /// The crate owns and serves the selection afterwards; see the type-level
    /// docs for the per-display-server lifetime.
    ///
    /// # Errors
    ///
    /// Returns [`ClipboardError::Platform`] if the display server refuses the
    /// selection or cannot be reached.
    pub fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.inner.set_text(text)
    }
}
