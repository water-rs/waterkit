//! iOS clipboard implementation through `UIPasteboard`.
//!
//! macOS uses `clipboard-rs` instead; this module compiles for iOS only.

use crate::content::{ClipboardEvent, Image};
use crate::error::ClipboardError;
use crate::sys::file_path::unicode_paths;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};

use block2::RcBlock;
use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::AllocAnyThread as _;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSFileManager, NSItemProvider, NSNotification,
    NSNotificationCenter, NSOperationQueue, NSString, NSURL,
};
use objc2_ui_kit::{
    UIApplicationDidBecomeActiveNotification, UIImage, UIPasteboard,
    UIPasteboardChangedNotification,
};
use waterkit_core::apple::on_main;

/// Reinterprets an object as a plain [`AnyObject`] for the untyped
/// pasteboard-item dictionaries.
///
/// # Safety
/// `AnyObject` assumes no specific class; every `'static` Objective-C object
/// can be reinterpreted as one.
fn as_any_object<T: objc2::Message>(obj: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: documented on the function.
    unsafe { Retained::cast_unchecked(obj) }
}

/// The pasteboard type identifiers the queries and writes use.
mod types {
    use objc2::rc::Retained;
    use objc2_foundation::NSString;
    use objc2_uniform_type_identifiers::{UTTypeFileURL, UTTypeHTML, UTTypePlainText};

    /// `public.html`.
    pub fn html() -> Retained<NSString> {
        // SAFETY: `UTTypeHTML` is an immutable framework-exported static.
        unsafe { UTTypeHTML }.identifier()
    }
    /// `public.plain-text`.
    pub fn plain_text() -> Retained<NSString> {
        // SAFETY: `UTTypePlainText` is an immutable framework-exported
        // static.
        unsafe { UTTypePlainText }.identifier()
    }
    /// `public.file-url`.
    pub fn file_url() -> Retained<NSString> {
        // SAFETY: `UTTypeFileURL` is an immutable framework-exported static.
        unsafe { UTTypeFileURL }.identifier()
    }
}

/// iOS clipboard handle.
#[derive(Debug)]
pub struct ClipboardInner;

impl ClipboardInner {
    /// Create a new clipboard handle.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard constructor can fail on other backends"
    )]
    pub const fn new() -> Result<Self, ClipboardError> {
        Ok(Self)
    }

    // ========== Query (sync) ==========
    //
    // `UIPasteboard`'s presence queries (`hasStrings`, `hasImages`,
    // `contains(pasteboardTypes:)`) cannot fail. The queries keep the
    // cross-platform `Result` signature, which fails on other backends.

    /// Check if text is available.
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard query is fallible and instance-based"
    )]
    pub fn has_text(&self) -> Result<bool, ClipboardError> {
        // SAFETY: `hasStrings` is a read-only query.
        Ok(unsafe { UIPasteboard::generalPasteboard().hasStrings() })
    }

    /// Check if HTML is available.
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard query is fallible and instance-based"
    )]
    pub fn has_html(&self) -> Result<bool, ClipboardError> {
        let types = NSArray::from_retained_slice(&[types::html()]);
        Ok(UIPasteboard::generalPasteboard().containsPasteboardTypes(&types))
    }

    /// Check if files are available.
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard query is fallible and instance-based"
    )]
    pub fn has_files(&self) -> Result<bool, ClipboardError> {
        let types = NSArray::from_retained_slice(&[types::file_url()]);
        Ok(UIPasteboard::generalPasteboard().containsPasteboardTypes(&types))
    }

    /// Check if image is available.
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard query is fallible and instance-based"
    )]
    pub fn has_image(&self) -> Result<bool, ClipboardError> {
        // SAFETY: `hasImages` is a read-only query.
        Ok(unsafe { UIPasteboard::generalPasteboard().hasImages() })
    }

    // ========== Read (sync, called from blocking::unblock) ==========

    /// Get text content.
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard backend API is fallible and instance-based"
    )]
    pub fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        // SAFETY: `string` is a plain property read.
        Ok(unsafe { UIPasteboard::generalPasteboard().string() }.map(|text| text.to_string()))
    }

    /// Get HTML content.
    #[expect(
        clippy::unused_self,
        reason = "the cross-platform clipboard backend API is instance-based"
    )]
    pub fn get_html(&self) -> Result<Option<String>, ClipboardError> {
        let Some(data) = UIPasteboard::generalPasteboard().dataForPasteboardType(&types::html())
        else {
            return Ok(None);
        };
        String::from_utf8(data.to_vec())
            .map(Some)
            .map_err(|error| ClipboardError::Decode(format!("the HTML is not UTF-8: {error}")))
    }

    /// Get the paths of the file URLs on the pasteboard, decoded by
    /// `NSURL.path`.
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard backend API is fallible and instance-based"
    )]
    pub fn get_files(&self) -> Result<Vec<PathBuf>, ClipboardError> {
        // SAFETY: `URLs` is a plain property read.
        let Some(urls) = (unsafe { UIPasteboard::generalPasteboard().URLs() }) else {
            return Ok(Vec::new());
        };
        Ok(urls
            .iter()
            .filter(|url| url.isFileURL())
            .filter_map(|url| url.path().map(|path| PathBuf::from(path.to_string())))
            .collect())
    }

    /// Get image as RGBA.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "pasteboard image dimensions are always valid u32"
    )]
    #[expect(
        clippy::cast_precision_loss,
        reason = "the bitmap rect uses f64 points; pasteboard images are far below 2^52 pixels"
    )]
    #[expect(
        clippy::unused_self,
        reason = "the cross-platform clipboard backend API is instance-based"
    )]
    pub fn get_image(&self) -> Result<Option<Image>, ClipboardError> {
        // SAFETY: `image` is a plain property read.
        let Some(image) = (unsafe { UIPasteboard::generalPasteboard().image() }) else {
            return Ok(None);
        };
        // SAFETY: `CGImage` is a plain property read; it is `None` when the
        // image has no bitmap representation (e.g. a CIImage).
        let Some(cg_image) = (unsafe { image.CGImage() }) else {
            return Err(ClipboardError::InvalidImage(
                "the pasteboard image has no bitmap (CGImage) representation".into(),
            ));
        };

        let width = CGImage::width(Some(&cg_image));
        let height = CGImage::height(Some(&cg_image));
        let bytes_per_row = 4 * width;
        let mut data = vec![0u8; bytes_per_row * height];

        let color_space =
            CGColorSpace::new_device_rgb().expect("the device RGB color space always exists");
        // SAFETY: `data` points to `bytes_per_row * height` writable bytes,
        // matching the premultiplied-RGBA layout declared by the bitmap info.
        let Some(context) = (unsafe {
            CGBitmapContextCreate(
                data.as_mut_ptr().cast(),
                width,
                height,
                8,
                bytes_per_row,
                Some(&color_space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }) else {
            return Err(ClipboardError::InvalidImage(format!(
                "failed to create a {width}x{height} RGBA bitmap context for the pasteboard image"
            )));
        };

        CGContext::draw_image(
            Some(&context),
            CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(width as f64, height as f64),
            ),
            Some(&cg_image),
        );

        Ok(Some(Image::new(width as u32, height as u32, data)))
    }

    /// Get binary data by MIME type.
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard backend API is fallible and instance-based"
    )]
    pub fn get_binary(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        Ok(UIPasteboard::generalPasteboard()
            .dataForPasteboardType(&NSString::from_str(mime))
            .map(|data| data.to_vec()))
    }

    // ========== Write (sync) ==========

    /// Set text content.
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard backend API is fallible and instance-based"
    )]
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        // SAFETY: `setString:` accepts any `NSString`.
        unsafe { UIPasteboard::generalPasteboard().setString(Some(&NSString::from_str(text))) };
        Ok(())
    }

    /// Set HTML content.
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard backend API is fallible and instance-based"
    )]
    pub fn set_html(&self, html: &str, alt_text: Option<&str>) -> Result<(), ClipboardError> {
        let html_data = as_any_object(NSData::with_bytes(html.as_bytes()));
        let html_id = types::html();
        let html_item =
            NSDictionary::<NSString, AnyObject>::from_slices(&[&*html_id], &[&*html_data]);

        let mut items = vec![html_item];
        if let Some(alt_text) = alt_text.filter(|alt| !alt.is_empty()) {
            let alt = as_any_object(NSString::from_str(alt_text));
            let alt_id = types::plain_text();
            let alt_item = NSDictionary::<NSString, AnyObject>::from_slices(&[&*alt_id], &[&*alt]);
            items.push(alt_item);
        }

        let items = NSArray::from_retained_slice(&items);
        // SAFETY: every item is a `{pasteboard-type: data}` dictionary.
        unsafe { UIPasteboard::generalPasteboard().setItems(&items) };
        Ok(())
    }

    /// Set file paths, one pasteboard item per file.
    ///
    /// Each item is an `NSItemProvider` for the file: other apps paste the
    /// file's contents, which they cannot read through a URL into this app's
    /// sandbox, and its file URL, which encodes the path's bytes as they are.
    #[expect(
        clippy::unused_self,
        reason = "the cross-platform clipboard backend API is instance-based"
    )]
    pub fn set_files(&self, files: &[PathBuf]) -> Result<(), ClipboardError> {
        if files.is_empty() {
            return Ok(());
        }
        let mut providers = Vec::with_capacity(files.len());
        for path in unicode_paths(files)? {
            let url = file_url(&path)?;
            // SAFETY: `url` is a file URL.
            let Some(provider) = (unsafe {
                NSItemProvider::initWithContentsOfURL(NSItemProvider::alloc(), Some(&url))
            }) else {
                return Err(ClipboardError::Platform(format!(
                    "NSItemProvider cannot carry the file {}",
                    url.path().map_or_else(String::new, |p| p.to_string())
                )));
            };
            providers.push(provider);
        }
        let providers = NSArray::from_retained_slice(&providers);
        UIPasteboard::generalPasteboard()
            .setItemProviders_localOnly_expirationDate(&providers, false, None);
        Ok(())
    }

    /// Set image from a file path.
    #[expect(
        clippy::unused_self,
        reason = "the cross-platform clipboard backend API is instance-based"
    )]
    pub fn set_image_from_path(&self, path: &Path) -> Result<(), ClipboardError> {
        let path_str = path.to_string_lossy().to_string();
        let Some(image) = UIImage::imageWithContentsOfFile(&NSString::from_str(&path_str)) else {
            return Err(ClipboardError::InvalidImage(
                "failed to load image from path".into(),
            ));
        };
        // SAFETY: `setImage:` accepts any `UIImage`.
        unsafe { UIPasteboard::generalPasteboard().setImage(Some(&image)) };
        Ok(())
    }

    /// Set binary data with MIME type.
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard backend API is fallible and instance-based"
    )]
    pub fn set_binary(&self, data: &[u8], mime: &str) -> Result<(), ClipboardError> {
        UIPasteboard::generalPasteboard()
            .setData_forPasteboardType(&NSData::with_bytes(data), &NSString::from_str(mime));
        Ok(())
    }

    /// Set file promise.
    ///
    /// On iOS, file promises are not fully supported, so this falls back
    /// to immediately calling the provider and setting the file URL.
    pub fn set_file_promise(
        &self,
        provider: Box<dyn FnOnce() -> Result<PathBuf, ClipboardError> + Send>,
    ) -> Result<(), ClipboardError> {
        let path = provider()?;
        self.set_files(&[path])
    }

    /// Clear clipboard.
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the cross-platform clipboard backend API is fallible and instance-based"
    )]
    pub fn clear(&self) -> Result<(), ClipboardError> {
        // SAFETY: an empty item array clears the pasteboard.
        unsafe { UIPasteboard::generalPasteboard().setItems(&NSArray::new()) };
        Ok(())
    }
}

/// The file URL of `path`, whose bytes it encodes as they are.
///
/// `fileURLWithFileSystemRepresentation:isDirectory:relativeToURL:` keeps
/// the path's bytes verbatim — unlike `fileURLWithPath:`, which converts to
/// the decomposed (NFD) form Darwin's file-system representation uses — so a
/// name written precomposed reads back as the same bytes. Like
/// `fileURLWithPath:`, the URL ends in a slash when `path` is a directory.
///
/// # Errors
///
/// Returns [`ClipboardError::Platform`] when `path` contains an interior NUL
/// byte, which a C file-system representation cannot carry.
fn file_url(path: &str) -> Result<Retained<NSURL>, ClipboardError> {
    let mut is_directory = Bool::new(false);
    // SAFETY: `is_directory` points at a live `Bool` for the call's duration.
    let exists = unsafe {
        NSFileManager::defaultManager()
            .fileExistsAtPath_isDirectory(&NSString::from_str(path), &raw mut is_directory)
    };
    let c_path = CString::new(path)
        .map_err(|_| ClipboardError::Platform("path contains an interior NUL".into()))?;
    let c_ptr =
        NonNull::new(c_path.as_ptr().cast_mut()).expect("a CString's pointer is never null");
    // SAFETY: `c_ptr` is a NUL-terminated UTF-8 file-system representation
    // valid for the call's duration.
    Ok(unsafe {
        NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
            c_ptr,
            exists && is_directory.as_bool(),
            None,
        )
    })
}

/// The two `NSNotificationCenter` observer tokens a live watch holds,
/// `Send`-wrapped: they were created on the main thread and are dropped
/// there.
type ObserverTokens = MainThreadBound<(Retained<AnyObject>, Retained<AnyObject>)>;

/// Owns the notifications a clipboard watch is registered for.
///
/// Dropping the guard removes both observers; the channel the watch stream
/// reads then closes once its queued events drain.
pub struct AppleWatchGuard {
    tokens: Option<ObserverTokens>,
}

impl std::fmt::Debug for AppleWatchGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleWatchGuard").finish_non_exhaustive()
    }
}

impl Drop for AppleWatchGuard {
    fn drop(&mut self) {
        let Some(tokens) = self.tokens.take() else {
            return;
        };
        DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("the main queue runs on the main thread");
            let (changed, became_active) = tokens.into_inner(mtm);
            let center = NSNotificationCenter::defaultCenter();
            // SAFETY: `changed` and `became_active` are this watch's two
            // live observer registrations.
            unsafe {
                center.removeObserver(&changed);
                center.removeObserver(&became_active);
            }
        });
    }
}

/// Emit an event when the pasteboard's `changeCount` moved past `last`.
/// Runs on the main queue inside the observer blocks.
fn emit_if_changed(last: &AtomicIsize, sender: &async_channel::Sender<ClipboardEvent>) {
    // SAFETY: the read-only `changeCount` query runs on the main queue,
    // where the process-global pasteboard is valid to query.
    let pasteboard = UIPasteboard::generalPasteboard();
    let current = unsafe { pasteboard.changeCount() };
    if current == last.swap(current, Ordering::SeqCst) {
        return;
    }
    let event = ClipboardEvent::new(
        // SAFETY: same read-only queries.
        unsafe { pasteboard.hasStrings() },
        pasteboard.containsPasteboardTypes(&NSArray::from_retained_slice(&[types::html()])),
        pasteboard.containsPasteboardTypes(&NSArray::from_retained_slice(&[types::file_url()])),
        unsafe { pasteboard.hasImages() },
    );
    // A failed send only means the stream is gone; the guard's drop
    // unregisters the observer behind it.
    let _ = sender.try_send(event);
}

/// Start watching clipboard changes.
///
/// Registers two `NSNotificationCenter` observers whose blocks run on the
/// main queue: `UIPasteboardChangedNotification` covers changes made while
/// the app is active, and `UIApplicationDidBecomeActiveNotification`
/// triggers one `changeCount` comparison on returning to the foreground,
/// which is where changes made by other apps become visible. Both feed the
/// unbounded channel the watch stream yields from; nothing runs while
/// nothing changes.
///
/// # Errors
///
/// Infallible today; returns [`ClipboardError`] for parity with other
/// backends.
pub async fn start_watch()
-> Result<(async_channel::Receiver<ClipboardEvent>, AppleWatchGuard), ClipboardError> {
    let (sender, receiver) = async_channel::unbounded();
    let sender_clone = sender.clone();

    let tokens = on_main(move |mtm| {
        // SAFETY: the read-only `changeCount` query runs on the main thread
        // and only seeds the deduplication counter.
        let last = Arc::new(AtomicIsize::new(unsafe {
            UIPasteboard::generalPasteboard().changeCount()
        }));

        let changed_block = RcBlock::new({
            let sender = sender_clone.clone();
            let last = Arc::clone(&last);
            move |_notification: NonNull<NSNotification>| emit_if_changed(&last, &sender)
        });
        let became_active_block = RcBlock::new({
            let sender = sender_clone;
            let last = Arc::clone(&last);
            move |_notification: NonNull<NSNotification>| emit_if_changed(&last, &sender)
        });

        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: both blocks capture only `Send` state (the channel and
        // the counter) and run on the main queue, where the pasteboard
        // queries are valid.
        let changed = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(UIPasteboardChangedNotification),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &changed_block,
            )
        };
        let became_active = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(UIApplicationDidBecomeActiveNotification),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &became_active_block,
            )
        };

        MainThreadBound::new((as_any_object(changed), as_any_object(became_active)), mtm)
    })
    .await;

    Ok((
        receiver,
        AppleWatchGuard {
            tokens: Some(tokens),
        },
    ))
}
