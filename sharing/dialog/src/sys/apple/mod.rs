//! iOS dialogs via `UIKit`, `PhotosUI`, `Photos` and `UniformTypeIdentifiers`
//! through `objc2`.
//!
//! All presentation happens on the main queue, matching the original
//! `waterkit_core::apple::on_main` hops. Delegate objects are `define_class!`
//! ivars-owning Rust senders; a delegate keeps itself alive for its
//! presentation through a `keep_alive` self-retain cleared on completion —
//! the role the old static `activeDelegates` registry filled. A picked
//! photo is handed back as a `Selection` owning the provider, so no global
//! selection table exists either.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use dispatch2::MainThreadBound;
use futures::channel::oneshot;
use objc2::Message as _;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::ProtocolObject;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, extern_class, extern_methods,
    extern_protocol, msg_send,
};
use objc2_foundation::{
    NSArray, NSError, NSFileManager, NSItemProvider, NSObject, NSObjectProtocol,
    NSOperatingSystemVersion, NSProcessInfo, NSString, NSURL, NSUUID,
};
use objc2_photos::{
    PHAsset, PHAssetResource, PHAssetResourceManager, PHAssetResourceRequestOptions,
    PHAssetResourceType,
};
use objc2_photos_ui::{PHPickerConfiguration, PHPickerFilter, PHPickerResult};
use objc2_ui_kit::{
    UIAlertAction, UIAlertActionStyle, UIAlertController, UIAlertControllerStyle, UIApplication,
    UIApplicationDelegate, UIDocumentPickerDelegate, UIDocumentPickerViewController, UIResponder,
    UISceneActivationState, UIViewController, UIWindowScene,
};
use objc2_uniform_type_identifiers::UTType;

use crate::{
    Dialog, DialogError, FileDialog, LoadedLivePhoto, LoadedMedia, MediaType,
    collect_filter_extensions, finalize_selected_file, finalize_selected_files,
};

extern_class!(
    /// `PHPickerViewController` (PhotosUI). `objc2-photos-ui` 0.3 generates
    /// this class for macOS only, so the iOS binding is declared here with
    /// the two members this port needs.
    ///
    /// See also [Apple's documentation](https://developer.apple.com/documentation/photosui/phpickerviewcontroller?language=objc)
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[name = "PHPickerViewController"]
    struct PHPickerViewController;
);

// `extern_protocol!` cannot be invoked inside `impl`; keep the protocol
// declaration next to the class.
extern_protocol!(
    /// `PHPickerViewControllerDelegate`. `objc2-photos-ui` 0.3 generates the
    /// trait for macOS only; on iOS the single required callback is declared
    /// here.
    ///
    /// See also [Apple's documentation](https://developer.apple.com/documentation/photosui/phpickerviewcontrollerdelegate?language=objc)
    /// # Safety
    /// The delegate is stored weakly by `PHPickerViewController`; it must be
    /// a `MainThreadOnly` NSObject subclass conforming to the protocol.
    #[expect(
        clippy::missing_safety_doc,
        reason = "the emitted trait cannot carry a # Safety section the lint can see through extern_protocol!"
    )]
    pub unsafe trait PHPickerViewControllerDelegate:
        NSObjectProtocol + MainThreadOnly
    {
        /// `picker:didFinishPicking:` — fires on selection and on cancel.
        #[unsafe(method(picker:didFinishPicking:))]
        #[unsafe(method_family = none)]
        unsafe fn picker_did_finish_picking(
            &self,
            picker: &PHPickerViewController,
            results: &NSArray<PHPickerResult>,
        );
    }
);

impl PHPickerViewController {
    extern_methods!(
        /// `initWithConfiguration:` — see the class declaration for why this
        /// is declared locally.
        #[unsafe(method(initWithConfiguration:))]
        #[unsafe(method_family = init)]
        pub unsafe fn init_with_configuration(
            this: Allocated<Self>,
            configuration: &PHPickerConfiguration,
        ) -> Retained<Self>;

        /// `setDelegate:` — weak property.
        #[unsafe(method(setDelegate:))]
        #[unsafe(method_family = none)]
        pub unsafe fn set_delegate(
            &self,
            delegate: Option<&ProtocolObject<dyn PHPickerViewControllerDelegate>>,
        );
    );
}

/// The `iOS 14 / macCatalyst 14 (macOS 11)` gate the original used for
/// `UTType` and the document picker's typed UTIs.
fn has_ut_types() -> bool {
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: if cfg!(target_abi = "macabi") { 11 } else { 14 },
        minorVersion: 0,
        patchVersion: 0,
    })
}

/// `getTopViewController()` — the key window's topmost presented view
/// controller, falling back to the app delegate's window.
fn top_view_controller(mtm: MainThreadMarker) -> Option<Retained<UIViewController>> {
    let app = UIApplication::sharedApplication(mtm);
    let mut top = app
        .connectedScenes()
        .allObjects()
        .iter()
        .filter(|scene| scene.activationState() == UISceneActivationState::ForegroundActive)
        .find_map(|scene| {
            scene
                .downcast_ref::<UIWindowScene>()
                .and_then(|scene| scene.windows().iter().find(|window| window.isKeyWindow()))
        })
        .and_then(|window| window.rootViewController())
        .or_else(|| {
            // SAFETY: `delegate`/`window`/`rootViewController` are reads on
            // live main-thread objects.
            unsafe {
                app.delegate()
                    .and_then(|delegate| delegate.window())
                    .and_then(|window| window.rootViewController())
            }
        });
    while let Some(presented) = top
        .as_deref()
        .and_then(UIViewController::presentedViewController)
    {
        top = Some(presented);
    }
    top
}

/// Builds the `UIAlertController` the original used for both alert and
/// confirm (`.alert` style; the kind string never reached the UI).
fn alert_controller(dialog: &Dialog, mtm: MainThreadMarker) -> Retained<UIAlertController> {
    UIAlertController::alertControllerWithTitle_message_preferredStyle(
        Some(&NSString::from_str(&dialog.title)),
        Some(&NSString::from_str(&dialog.message)),
        UIAlertControllerStyle::Alert,
        mtm,
    )
}

pub async fn show_alert(dialog: Dialog) -> Result<(), DialogError> {
    waterkit_core::apple::on_main(move |mtm| {
        let Some(top) = top_view_controller(mtm) else {
            return;
        };
        let alert = alert_controller(&dialog, mtm);
        let action = UIAlertAction::actionWithTitle_style_handler(
            Some(&NSString::from_str("OK")),
            UIAlertActionStyle::Default,
            None,
            mtm,
        );
        alert.addAction(&action);
        top.presentViewController_animated_completion(&alert, true, None);
    })
    .await;
    Ok(())
}

pub async fn show_confirm(dialog: Dialog) -> Result<bool, DialogError> {
    let (sender, receiver) = oneshot::channel();
    // The sender is shared by the OK and Cancel actions; whichever fires
    // first delivers the answer.
    let sender = Arc::new(Mutex::new(Some(sender)));
    waterkit_core::apple::on_main(move |mtm| {
        let Some(top) = top_view_controller(mtm) else {
            let _ = sender.lock().expect("confirm sender lock").take();
            return;
        };
        let alert = alert_controller(&dialog, mtm);
        for (title, style, answer) in [
            ("OK", UIAlertActionStyle::Default, true),
            ("Cancel", UIAlertActionStyle::Cancel, false),
        ] {
            let sender = Arc::clone(&sender);
            let handler = RcBlock::new(move |_action: core::ptr::NonNull<UIAlertAction>| {
                let sender = sender.lock().expect("confirm sender lock").take();
                if let Some(sender) = sender {
                    let _ = sender.send(answer);
                }
            });
            let action = UIAlertAction::actionWithTitle_style_handler(
                Some(&NSString::from_str(title)),
                style,
                Some(&handler),
                mtm,
            );
            alert.addAction(&action);
        }
        top.presentViewController_animated_completion(&alert, true, None);
    })
    .await;
    receiver.await.map_err(|_| DialogError::Cancelled)
}

/// A picked photo item handed to `load_photo_media` later, replacing the
/// static `activeSelections` table: the request owns its state.
#[derive(Clone)]
pub struct Selection {
    /// The item provider of the picked `PHPickerResult`, main-thread-bound
    /// like the picker that produced it.
    provider: Arc<MainThreadBound<Retained<NSItemProvider>>>,
    /// `PHPickerResult.assetIdentifier` — `None` when the picker was
    /// configured with the plain `init` (the original's shape), so live
    /// photo loads fail fast exactly as before.
    asset_identifier: Option<String>,
}

impl std::fmt::Debug for Selection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Selection")
            .field("asset_identifier", &self.asset_identifier)
            .finish_non_exhaustive()
    }
}

/// The `ObjC` ivars of [`PhotoPickerDelegate`]: the pending result sender
/// plus the self-retain that keeps the delegate alive for the picker's
/// lifetime.
#[derive(Debug)]
struct PhotoPickerDelegateIvars {
    sender: RefCell<Option<oneshot::Sender<Option<Selection>>>>,
    keep_alive: RefCell<Option<Retained<PhotoPickerDelegate>>>,
}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `PhotoPickerDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WaterkitDialogPhotoPickerDelegate"]
    #[ivars = PhotoPickerDelegateIvars]
    #[derive(Debug)]
    struct PhotoPickerDelegate;

    unsafe impl NSObjectProtocol for PhotoPickerDelegate {}

    unsafe impl PHPickerViewControllerDelegate for PhotoPickerDelegate {
        #[unsafe(method(picker:didFinishPicking:))]
        unsafe fn picker_did_finish_picking(
            &self,
            picker: &PHPickerViewController,
            results: &NSArray<PHPickerResult>,
        ) {
            let mtm = MainThreadMarker::new()
                .expect("PHPickerViewControllerDelegate is main-thread bound");
            picker.dismissViewControllerAnimated_completion(true, None);
            let selection = results.firstObject().map(|result| {
                // SAFETY: `itemProvider`/`assetIdentifier` are reads on a
                // live picker result.
                let (provider, identifier) =
                    unsafe { (result.itemProvider(), result.assetIdentifier()) };
                Selection {
                    provider: Arc::new(MainThreadBound::new(provider, mtm)),
                    asset_identifier: identifier.as_deref().map(ToString::to_string),
                }
            });
            self.finish(selection);
        }
    }
);

impl PhotoPickerDelegate {
    fn new(sender: oneshot::Sender<Option<Selection>>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PhotoPickerDelegateIvars {
            sender: RefCell::new(Some(sender)),
            keep_alive: RefCell::new(None),
        });
        // SAFETY: `this` is a freshly allocated delegate and `NSObject`'s
        // `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }

    /// Self-retains for the presentation duration; `finish` clears it.
    fn start(&self) {
        *self.ivars().keep_alive.borrow_mut() = Some(self.retain());
    }

    /// Delivers the result once and releases the self-retain.
    fn finish(&self, selection: Option<Selection>) {
        if let Some(sender) = self.ivars().sender.borrow_mut().take() {
            let _ = sender.send(selection);
        }
        self.ivars().keep_alive.borrow_mut().take();
    }
}

/// The `ObjC` ivars of [`FilePickerDelegate`].
#[derive(Debug)]
struct FilePickerDelegateIvars {
    sender: RefCell<Option<oneshot::Sender<Option<Vec<PathBuf>>>>>,
    multiple: bool,
    keep_alive: RefCell<Option<Retained<FilePickerDelegate>>>,
}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `FilePickerDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WaterkitDialogFilePickerDelegate"]
    #[ivars = FilePickerDelegateIvars]
    #[derive(Debug)]
    struct FilePickerDelegate;

    unsafe impl NSObjectProtocol for FilePickerDelegate {}

    unsafe impl UIDocumentPickerDelegate for FilePickerDelegate {
        #[unsafe(method(documentPickerWasCancelled:))]
        fn document_picker_was_cancelled(&self, _controller: &UIDocumentPickerViewController) {
            self.finish(None);
        }

        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn document_picker_did_pick_documents(
            &self,
            _controller: &UIDocumentPickerViewController,
            urls: &NSArray<NSURL>,
        ) {
            let urls: Vec<Retained<NSURL>> = if self.ivars().multiple {
                urls.iter().collect()
            } else {
                urls.firstObject().into_iter().collect()
            };
            let paths: Vec<PathBuf> = urls
                .iter()
                .filter_map(|url| copy_to_temporary_location(url, None, None))
                .collect();
            if paths.is_empty() {
                self.finish(None);
            } else {
                self.finish(Some(paths));
            }
        }
    }
);

impl FilePickerDelegate {
    fn new(
        sender: oneshot::Sender<Option<Vec<PathBuf>>>,
        multiple: bool,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FilePickerDelegateIvars {
            sender: RefCell::new(Some(sender)),
            multiple,
            keep_alive: RefCell::new(None),
        });
        // SAFETY: `this` is a freshly allocated delegate and `NSObject`'s
        // `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }

    /// Self-retains for the presentation duration; `finish` clears it.
    fn start(&self) {
        *self.ivars().keep_alive.borrow_mut() = Some(self.retain());
    }

    /// Delivers the result once and releases the self-retain.
    fn finish(&self, paths: Option<Vec<PathBuf>>) {
        if let Some(sender) = self.ivars().sender.borrow_mut().take() {
            let _ = sender.send(paths);
        }
        self.ivars().keep_alive.borrow_mut().take();
    }
}

pub async fn show_photo_picker(media_type: MediaType) -> Result<Option<Selection>, DialogError> {
    let (sender, receiver) = oneshot::channel();
    waterkit_core::apple::on_main(move |mtm| {
        let Some(top) = top_view_controller(mtm) else {
            let _ = sender.send(None);
            return;
        };
        // SAFETY: `PHPickerConfiguration::new` builds the system-library
        // configuration; setters only store values.
        let configuration = unsafe { PHPickerConfiguration::new() };
        unsafe { configuration.setSelectionLimit(1) };
        // SAFETY: `*Filter` are class factory accessors.
        let filter = unsafe {
            match media_type {
                MediaType::Image => PHPickerFilter::imagesFilter(),
                MediaType::Video => PHPickerFilter::videosFilter(),
                MediaType::LivePhoto => PHPickerFilter::livePhotosFilter(),
            }
        };
        // `objc2-photos-ui` 0.3 emits `setFilter:` only on the macOS
        // `PHPickerUpdateConfiguration`; the same selector exists on the
        // iOS `PHPickerConfiguration`.
        let () = unsafe { msg_send![&configuration, setFilter: Some(&*filter)] };

        let delegate = PhotoPickerDelegate::new(sender, mtm);
        let picker = unsafe {
            PHPickerViewController::init_with_configuration(
                PHPickerViewController::alloc(mtm),
                &configuration,
            )
        };
        // SAFETY: `setDelegate:` stores a weak reference; `delegate` is
        // kept alive by its own `keep_alive` ivar until it fires.
        unsafe {
            picker.set_delegate(Some(ProtocolObject::from_ref(&*delegate)));
        }
        delegate.start();
        top.presentViewController_animated_completion(&picker, true, None);
    })
    .await;
    receiver.await.map_err(|_| DialogError::Cancelled)
}

/// The document-type identifier list the original built: `UTType`
/// identifiers when available, `public.data` otherwise.
fn document_type_identifiers(extensions: &[String]) -> Vec<String> {
    if !has_ut_types() {
        return vec!["public.data".into()];
    }
    let mapped: Vec<String> = extensions
        .iter()
        .filter_map(|extension| {
            UTType::typeWithFilenameExtension(&NSString::from_str(extension))
                .map(|ty| ty.identifier().to_string())
        })
        .collect();
    if mapped.is_empty() {
        // The original used UTType.item's identifier.
        vec!["public.item".into()]
    } else {
        mapped
    }
}

/// Presents a `UIDocumentPickerViewController` and resolves the picked
/// copies.
async fn present_document_picker(
    dialog: &FileDialog,
    multiple: bool,
) -> Result<Option<Vec<PathBuf>>, DialogError> {
    let extensions = collect_filter_extensions(dialog);
    let (sender, receiver) = oneshot::channel();
    waterkit_core::apple::on_main(move |mtm| {
        let Some(top) = top_view_controller(mtm) else {
            let _ = sender.send(None);
            return;
        };
        let types: Vec<Retained<NSString>> = document_type_identifiers(&extensions)
            .iter()
            .map(|identifier| NSString::from_str(identifier))
            .collect();
        let delegate = FilePickerDelegate::new(sender, multiple, mtm);
        // SAFETY: fresh alloc; `initWithDocumentTypes:inMode:` is
        // deprecated, but it is the initializer the original selected for
        // iOS 13 support (`initForOpeningContentTypes:asCopy:` is iOS 18).
        #[expect(deprecated)]
        let picker = UIDocumentPickerViewController::initWithDocumentTypes_inMode(
            UIDocumentPickerViewController::alloc(mtm),
            &NSArray::from_retained_slice(&types),
            objc2_ui_kit::UIDocumentPickerMode::Import,
        );
        picker.setAllowsMultipleSelection(multiple);
        picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        delegate.start();
        top.presentViewController_animated_completion(&picker, true, None);
    })
    .await;
    receiver.await.map_err(|_| DialogError::Cancelled)
}

pub async fn show_open_single_file(dialog: FileDialog) -> Result<Option<PathBuf>, DialogError> {
    let Some(mut paths) = present_document_picker(&dialog, false).await? else {
        return Ok(None);
    };
    debug_assert_eq!(paths.len(), 1);
    paths
        .pop()
        .map(|path| finalize_selected_file(&dialog, path))
        .transpose()
}

pub async fn show_open_multiple_files(
    dialog: FileDialog,
) -> Result<Option<Vec<PathBuf>>, DialogError> {
    let Some(paths) = present_document_picker(&dialog, true).await? else {
        return Ok(None);
    };
    finalize_selected_files(&dialog, paths).map(Some)
}

/// `temporaryDestinationURL` — `<tmp>/<uuid>[.<ext>]`.
fn temporary_destination_url(
    source_url: Option<&NSURL>,
    preferred_filename: Option<&str>,
    type_identifier: Option<&str>,
) -> Retained<NSURL> {
    let extension = resolved_filename_extension(source_url, preferred_filename, type_identifier);
    let uuid = NSUUID::UUID().UUIDString();
    let file_name = if extension.is_empty() {
        uuid.to_string()
    } else {
        format!("{uuid}.{extension}")
    };
    NSFileManager::defaultManager()
        .temporaryDirectory()
        .URLByAppendingPathComponent(&NSString::from_str(&file_name))
        .expect("appending a file name to the temporary directory must succeed")
}

/// `resolvedFilenameExtension` — source extension, then the provider's
/// suggested name's extension, then the type identifier's preferred
/// extension.
fn resolved_filename_extension(
    source_url: Option<&NSURL>,
    preferred_filename: Option<&str>,
    type_identifier: Option<&str>,
) -> String {
    if let Some(url) = source_url
        && let Some(extension) = url.pathExtension()
        && !extension.is_empty()
    {
        return extension.to_string();
    }
    if let Some(name) = preferred_filename {
        let extension = NSURL::fileURLWithPath(&NSString::from_str(name)).pathExtension();
        if let Some(extension) = extension
            && !extension.is_empty()
        {
            return extension.to_string();
        }
    }
    if let Some(identifier) = type_identifier
        && has_ut_types()
        && let Some(extension) = UTType::importedTypeWithIdentifier(&NSString::from_str(identifier))
            .preferredFilenameExtension()
    {
        return extension.to_string();
    }
    String::new()
}

/// `copyToTemporaryLocation` — security-scoped copy into the temp dir;
/// returns the destination path. `None` on any failure, matching the
/// original's `nil` result.
fn copy_to_temporary_location(
    source_url: &NSURL,
    preferred_filename: Option<&str>,
    type_identifier: Option<&str>,
) -> Option<PathBuf> {
    // SAFETY: `startAccessingSecurityScopedResource` may only be paired with
    // `stopAccessing…` when it returned true.
    let scoped = unsafe { source_url.startAccessingSecurityScopedResource() };
    let result = (|| {
        let destination =
            temporary_destination_url(Some(source_url), preferred_filename, type_identifier);
        let destination_path = destination.path()?.to_string();
        let manager = NSFileManager::defaultManager();
        if manager.fileExistsAtPath(&NSString::from_str(&destination_path)) {
            manager.removeItemAtURL_error(&destination).ok()?;
        }
        manager
            .copyItemAtURL_toURL_error(source_url, &destination)
            .ok()?;
        Some(PathBuf::from(destination_path))
    })();
    if scoped {
        // SAFETY: paired with the successful `startAccessing…` above.
        unsafe { source_url.stopAccessingSecurityScopedResource() };
    }
    result
}

/// `loadSingleMedia` — `loadFileRepresentation` for the first conforming
/// type (video preferred over image, as before), then a security-scoped
/// copy into the temp dir.
async fn load_single_media(
    provider: Arc<MainThreadBound<Retained<NSItemProvider>>>,
) -> Option<LoadedMedia> {
    let receiver = waterkit_core::apple::on_main(move |mtm| {
        let provider = provider.get(mtm);
        let movie_identifier = UTType::typeWithIdentifier(&NSString::from_str("public.movie"))
            .map(|ty| ty.identifier().to_string());
        let image_identifier = UTType::typeWithIdentifier(&NSString::from_str("public.image"))
            .map(|ty| ty.identifier().to_string());
        let (type_identifier, video) = if movie_identifier.as_deref().is_some_and(|identifier| {
            provider.hasItemConformingToTypeIdentifier(&NSString::from_str(identifier))
        }) {
            (movie_identifier, true)
        } else if image_identifier.as_deref().is_some_and(|identifier| {
            provider.hasItemConformingToTypeIdentifier(&NSString::from_str(identifier))
        }) {
            (image_identifier, false)
        } else {
            return None;
        };
        let (sender, receiver) = oneshot::channel();
        let sender = RefCell::new(Some(sender));
        let suggested_name = provider.suggestedName().as_deref().map(ToString::to_string);
        let type_identifier_ns = NSString::from_str(type_identifier.as_deref().unwrap_or_default());
        let provider_clone = provider.clone();
        // The block owns a clone of the provider so the request's target
        // outlives the framework call; the completion fires on an
        // NSItemProvider-private queue and only Send data crosses it (the
        // sender and the copied path).
        let handler = RcBlock::new(move |url: *mut NSURL, error: *mut NSError| {
            let _provider = &provider_clone;
            let path = (!url.is_null() && error.is_null())
                .then(|| {
                    // SAFETY: `url` is non-null for the duration of the
                    // completion call.
                    copy_to_temporary_location(
                        unsafe { &*url },
                        suggested_name.as_deref(),
                        type_identifier.as_deref(),
                    )
                })
                .flatten();
            if let Some(sender) = sender.borrow_mut().take() {
                let _ = sender.send(path);
            }
        });
        // SAFETY: `loadFileRepresentation` may run its handler on a private
        // queue; the block captures only Send state besides the provider
        // clone it owns.
        unsafe {
            provider.loadFileRepresentationForTypeIdentifier_completionHandler(
                &type_identifier_ns,
                &handler,
            );
        }
        Some((receiver, video))
    })
    .await?;
    let (receiver, video) = receiver;
    receiver.await.ok()?.map(|path| {
        if video {
            LoadedMedia::Video(path)
        } else {
            LoadedMedia::Image(path)
        }
    })
}

/// `preferredImageResource`/`preferredVideoResource` — first resource
/// matching the original's preference order.
fn preferred_resource(
    resources: &NSArray<PHAssetResource>,
    kinds: &[PHAssetResourceType],
) -> Option<Retained<PHAssetResource>> {
    for kind in kinds {
        // SAFETY: `type` is a read on a live resource.
        if let Some(resource) = resources
            .iter()
            .find(|resource| unsafe { resource.r#type() == *kind })
        {
            return Some(resource);
        }
    }
    None
}

/// `writeAssetResourceToTemporaryLocation` — returns a receiver for the
/// destination path (`None` on failure).
fn write_asset_resource(
    resource: &PHAssetResource,
    options: &PHAssetResourceRequestOptions,
) -> oneshot::Receiver<Option<PathBuf>> {
    let (sender, receiver) = oneshot::channel();
    let sender = RefCell::new(Some(sender));
    // SAFETY: reads on a live resource.
    let (file_name, type_identifier) = unsafe {
        (
            resource.originalFilename().to_string(),
            // `contentType` is not exposed by objc2-photos 0.3; the
            // deprecated accessor it replaced is still the selector.
            #[expect(deprecated)]
            resource.uniformTypeIdentifier().to_string(),
        )
    };
    let destination = temporary_destination_url(None, Some(&file_name), Some(&type_identifier));
    let destination_path = destination.path().map(|path| path.to_string().into());
    // SAFETY: `defaultManager` returns the shared process manager.
    let manager = unsafe { PHAssetResourceManager::defaultManager() };
    // The block owns a clone of the manager so the request's target lives
    // until the framework answers; the completion fires on a private queue
    // and only Send data crosses it (the sender and the destination path).
    let handler = {
        let manager = manager.clone();
        RcBlock::new(move |error: *mut NSError| {
            let _manager = &manager;
            if let Some(sender) = sender.borrow_mut().take() {
                let _ = sender.send(if error.is_null() {
                    destination_path.clone()
                } else {
                    None
                });
            }
        })
    };
    // SAFETY: `writeDataForAssetResource:toFile:options:completionHandler:`
    // runs its handler on a private queue; the block captures only Send
    // state besides the manager clone it owns.
    unsafe {
        manager.writeDataForAssetResource_toFile_options_completionHandler(
            resource,
            &destination,
            Some(options),
            &handler,
        );
    }
    receiver
}

/// `loadLivePhoto` — fetches the asset by local identifier, picks the
/// preferred still/video resources, writes both to temp files, and joins
/// the two completions into the `LoadedLivePhoto` pair.
async fn load_live_photo(selection: &Selection) -> Option<LoadedMedia> {
    let asset_identifier = selection.asset_identifier.clone()?;
    let (image_rx, video_rx) = waterkit_core::apple::on_main(move |mtm| {
        let _ = mtm;
        let identifiers = NSArray::from_retained_slice(&[NSString::from_str(&asset_identifier)]);
        // SAFETY: `fetchAssetsWithLocalIdentifiers:options:` is a read-only
        // query.
        let assets =
            unsafe { PHAsset::fetchAssetsWithLocalIdentifiers_options(&identifiers, None) };
        // SAFETY: `firstObject` is a read on the live fetch result.
        let asset = unsafe { assets.firstObject() }?;
        // SAFETY: `assetResourcesForAsset` returns the asset's resources.
        let resources = unsafe { PHAssetResource::assetResourcesForAsset(&asset) };
        let image_resource = preferred_resource(
            &resources,
            &[
                PHAssetResourceType::FullSizePhoto,
                PHAssetResourceType::Photo,
                PHAssetResourceType::AlternatePhoto,
            ],
        )?;
        let video_resource = preferred_resource(
            &resources,
            &[
                PHAssetResourceType::FullSizePairedVideo,
                PHAssetResourceType::PairedVideo,
                PHAssetResourceType::Video,
            ],
        )?;
        // SAFETY: `new` + `setNetworkAccessAllowed` on a fresh options
        // object.
        let options = unsafe { PHAssetResourceRequestOptions::new() };
        unsafe { options.setNetworkAccessAllowed(true) };
        Some((
            write_asset_resource(&image_resource, &options),
            write_asset_resource(&video_resource, &options),
        ))
    })
    .await?;
    let (image, video) = futures::future::join(image_rx, video_rx).await;
    match (image.ok()?, video.ok()?) {
        (Some(image), Some(video)) => {
            Some(LoadedMedia::LivePhoto(LoadedLivePhoto::new(image, video)))
        }
        _ => None,
    }
}

pub async fn load_photo_media(
    selection: Selection,
    media_type: MediaType,
) -> Result<LoadedMedia, DialogError> {
    let loaded = if media_type == MediaType::LivePhoto {
        load_live_photo(&selection).await
    } else {
        load_single_media(selection.provider.clone()).await
    };
    loaded.ok_or(DialogError::Cancelled)
}
