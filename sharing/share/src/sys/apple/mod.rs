use crate::{ShareError, ShareItem, ShareResult, ShareSheet};

use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_foundation::{NSArray, NSString, NSURL};
use waterkit_core::apple::on_main;

#[cfg(target_os = "ios")]
use {
    block2::RcBlock,
    objc2::MainThreadOnly as _,
    objc2::runtime::Bool,
    objc2_foundation::{NSError, NSObjectNSKeyValueCoding as _, ns_string},
    objc2_ui_kit::{
        UIActivity, UIActivityType, UIActivityViewController, UIApplication,
        UIApplicationDelegate as _, UIImage, UISceneActivationState, UIViewController,
        UIWindowScene,
    },
    std::sync::Mutex,
};
#[cfg(target_os = "macos")]
use {
    objc2::AllocAnyThread as _,
    objc2_app_kit::{NSApplication, NSImage, NSSharingServicePicker},
    objc2_foundation::{NSRect, NSRectEdge},
};

/// Reinterprets an object as a plain [`AnyObject`] for the untyped
/// `activityItems` arrays both presenters take.
///
/// # Safety
/// Every Foundation/AppKit/UIKit object is a valid `AnyObject`.
fn as_any_object<T: objc2::Message>(obj: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: `AnyObject` assumes no specific class; every `'static`
    // Objective-C object can be reinterpreted as one.
    unsafe { Retained::cast_unchecked(obj) }
}

/// Turns the sheet's items into Objective-C activity items, skipping any
/// value the OS cannot construct exactly as before.
///
/// # Errors
///
/// Returns [`ShareError::Platform`] when an image or file path is not valid
/// UTF-8.
fn activity_items(
    mtm: MainThreadMarker,
    items: &[ShareItem],
) -> Result<Retained<NSArray<AnyObject>>, ShareError> {
    let _ = mtm;
    let mut objects: Vec<Retained<AnyObject>> = Vec::with_capacity(items.len());
    for item in items {
        match item {
            ShareItem::Text(text) => objects.push(as_any_object(NSString::from_str(text))),
            ShareItem::Url(url) => {
                if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
                    objects.push(as_any_object(url));
                }
            }
            ShareItem::Image(path) => {
                let path = path
                    .to_str()
                    .ok_or_else(|| ShareError::Platform("invalid path encoding".into()))?;
                let path = NSString::from_str(path);
                #[cfg(target_os = "ios")]
                {
                    if let Some(image) = UIImage::imageWithContentsOfFile(&path) {
                        objects.push(as_any_object(image));
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    // A `None` result means unreadable image data, which is
                    // skipped below.
                    if let Some(image) = NSImage::initWithContentsOfFile(NSImage::alloc(), &path) {
                        objects.push(as_any_object(image));
                    }
                }
            }
            ShareItem::File(path) => {
                let path = path
                    .to_str()
                    .ok_or_else(|| ShareError::Platform("invalid path encoding".into()))?;
                objects.push(as_any_object(NSURL::fileURLWithPath(&NSString::from_str(
                    path,
                ))));
            }
        }
    }
    Ok(NSArray::from_retained_slice(&objects))
}

/// # Errors
///
/// Returns [`ShareError::Platform`] when activity items cannot be built or
/// the completion callback is dropped.
pub async fn show_share_sheet(sheet: ShareSheet) -> Result<ShareResult, ShareError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    on_main(move |mtm| -> Result<(), ShareError> {
        let items = activity_items(mtm, &sheet.items)?;

        #[cfg(target_os = "ios")]
        {
            // SAFETY: `activityItems` accepts the Foundation objects built
            // above and no application activities are supplied.
            let controller = unsafe {
                UIActivityViewController::initWithActivityItems_applicationActivities(
                    UIActivityViewController::alloc(mtm),
                    &items,
                    None::<&NSArray<UIActivity>>,
                )
            };
            if let Some(subject) = sheet.subject {
                let subject = as_any_object(NSString::from_str(&subject));
                // SAFETY: `"subject"` is the KVC key UIActivityViewController
                // reads for the shared subject line; the value is an
                // `NSString`, the type that key expects.
                unsafe { controller.setValue_forKey(Some(&subject), ns_string!("subject")) };
            }

            match top_view_controller(mtm) {
                Some(top) => {
                    let tx = Mutex::new(Some(tx));
                    // The handler fires later from UIKit, so it is a `Fn`
                    // block and the one-shot sender is taken through the
                    // mutex. The block also retains the controller the
                    // request was issued on, so it lives until UIKit
                    // answers.
                    let completion = RcBlock::new({
                        let controller = controller.clone();
                        move |_activity: *mut UIActivityType,
                              completed: Bool,
                              _items: *mut NSArray,
                              _error: *mut NSError| {
                            let _controller = &controller;
                            let _ = tx.lock().expect("sender mutex poisoned").take().map(|tx| {
                                tx.send(if completed.as_bool() {
                                    ShareResult::Shared
                                } else {
                                    ShareResult::Cancelled
                                })
                            });
                        }
                    });
                    // SAFETY: the block is copied by the setter and outlives
                    // the local `RcBlock`.
                    unsafe {
                        controller
                            .setCompletionWithItemsHandler(RcBlock::as_ptr(&completion).cast());
                    }
                    top.presentViewController_animated_completion(&controller, true, None);
                }
                None => {
                    let _ = tx.send(ShareResult::Cancelled);
                }
            }
        }
        #[cfg(target_os = "macos")]
        {
            let _ = sheet;
            // SAFETY: the items are all objects an `NSSharingServicePicker`
            // accepts (strings, URLs, images).
            let picker = unsafe {
                NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items)
            };
            if let Some(view) = NSApplication::sharedApplication(mtm)
                .mainWindow()
                .and_then(|window| window.contentView())
            {
                picker.showRelativeToRect_ofView_preferredEdge(
                    NSRect::ZERO,
                    &view,
                    NSRectEdge::MinY,
                );
            }
            // macOS has no completion signal on the picker; the previous
            // implementation reported success unconditionally.
            let _ = tx.send(ShareResult::Shared);
        }

        Ok(())
    })
    .await?;

    rx.await
        .map_err(|_| ShareError::Platform("callback dropped".into()))
}

/// The frontmost scene's key window's top-most presented view controller,
/// falling back to the app delegate's window — the controller the sheet is
/// presented from.
#[cfg(target_os = "ios")]
fn top_view_controller(mtm: MainThreadMarker) -> Option<Retained<UIViewController>> {
    let app = UIApplication::sharedApplication(mtm);
    let mut top = app
        .connectedScenes()
        .iter()
        .filter(|scene| scene.activationState() == UISceneActivationState::ForegroundActive)
        .find_map(|scene| {
            scene
                .downcast_ref::<UIWindowScene>()
                .and_then(|scene| scene.windows().iter().find(|window| window.isKeyWindow()))
        })
        .and_then(|window| window.rootViewController())
        .or_else(|| {
            // SAFETY: `delegate` is read on the main thread.
            unsafe { app.delegate() }
                .and_then(|delegate| delegate.window())
                .and_then(|window| window.rootViewController())
        });
    while let Some(presented) = top.as_ref().and_then(|vc| vc.presentedViewController()) {
        top = Some(presented);
    }
    top
}
