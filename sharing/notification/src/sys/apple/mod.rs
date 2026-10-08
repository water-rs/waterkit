//! Apple (iOS/macOS) notification implementation backed by the
//! `UserNotifications` framework through `objc2`.
//!
//! Authorization and delivery resolve through `block2` completion handlers
//! answering a `futures` oneshot that [`show_notification`] awaits: the
//! delivery callback's own signal carries the `Result`, so there is no
//! deadline. Action responses are handled by a
//! `define_class!` `UNUserNotificationCenterDelegate` that opens action
//! URLs; installing it deliberately retains it for the process lifetime —
//! the center's `delegate` property is weak and responses keep being
//! handled after the caller's handle drops.

use core::cell::RefCell;
use std::rc::Rc;

use block2::{DynBlock, RcBlock};
use futures::channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, ProtocolObject};
use objc2::{AnyThread, ClassType, define_class, msg_send};
use objc2_foundation::{
    NSArray, NSError, NSObjectProtocol, NSOperatingSystemVersion, NSProcessInfo, NSSet, NSString,
    NSURL,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
    UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptionNone,
    UNNotificationDefaultActionIdentifier, UNNotificationDismissActionIdentifier,
    UNNotificationInterruptionLevel, UNNotificationPresentationOptions, UNNotificationRequest,
    UNNotificationResponse, UNNotificationSound, UNTextInputNotificationAction,
    UNTextInputNotificationResponse, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

use crate::{Action, InterruptionLevel, Notification, NotificationError, TextInputAction};

/// Reads an `NSError`'s `localizedDescription` from a completion handler's
/// raw pointer. `None` describes the error when the description is empty.
///
/// SAFETY: `error` must be null or point to a valid `NSError` owned by the
/// invoking completion handler; the description is copied out before the
/// block returns.
fn describe_error(error: *mut NSError) -> String {
    if error.is_null() {
        return "unknown error".into();
    }
    // SAFETY: the pointer is non-null and valid for the block's duration.
    unsafe { (*error).localizedDescription().to_string() }
}

/// The `#available(iOS major, macOS major)`-style OS gates used, as one `NSProcessInfo` version probe: `ios_major` applies on iOS
/// proper, `macos_major` on macOS and Mac Catalyst (where `NSProcessInfo`
/// reports the macOS version — the paired boundary).
fn has_availability(ios_major: isize, macos_major: isize) -> bool {
    let major = if cfg!(all(target_os = "ios", not(target_abi = "macabi"))) {
        ios_major
    } else {
        macos_major
    };
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: major,
        minorVersion: 0,
        patchVersion: 0,
    })
}

/// `UNNotificationDefaultActionIdentifier` — the extern string constant
/// `UserNotifications` exports.
fn default_action_identifier() -> &'static NSString {
    // SAFETY: `UNNotificationDefaultActionIdentifier` is an immutable extern
    // string constant that ships with UserNotifications; read-only access.
    unsafe { UNNotificationDefaultActionIdentifier }
}

/// `UNNotificationDismissActionIdentifier` — the extern string constant
/// `UserNotifications` exports.
fn dismiss_action_identifier() -> &'static NSString {
    // SAFETY: `UNNotificationDismissActionIdentifier` is an immutable extern
    // string constant that ships with UserNotifications; read-only access.
    unsafe { UNNotificationDismissActionIdentifier }
}

/// `UNNotificationPresentationOptions::Alert | .Sound` — the
/// `#available`-gated pre-banner presentation.
#[expect(
    deprecated,
    reason = "the alert presentation option is what the gate's legacy branch selects"
)]
fn legacy_presentation_options() -> UNNotificationPresentationOptions {
    UNNotificationPresentationOptions::Alert | UNNotificationPresentationOptions::Sound
}

/// Opens `url`: re-dispatched to the main
/// queue on iOS, a direct call on macOS.
#[cfg(target_os = "ios")]
fn open_url(url: &NSURL) {
    use dispatch2::DispatchQueue;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, Message};
    use objc2_foundation::NSDictionary;
    use objc2_ui_kit::{UIApplication, UIApplicationOpenExternalURLOptionsKey};

    let url = url.retain();
    DispatchQueue::main().exec_async(move || {
        let mtm = MainThreadMarker::new().expect("the main queue only runs on the main thread");
        // SAFETY: `openURL:options:completionHandler:` is the opener
        // `UIApplication.shared.open(url)` lowers to; `options` is an empty
        // dictionary and there is no completion handler.
        unsafe {
            UIApplication::sharedApplication(mtm).openURL_options_completionHandler(
                &url,
                &NSDictionary::<UIApplicationOpenExternalURLOptionsKey, AnyObject>::new(),
                None,
            );
        }
    });
}

/// macOS action URLs go through `NSWorkspace`, callable on any thread.
#[cfg(target_os = "macos")]
fn open_url(url: &NSURL) {
    use objc2_app_kit::NSWorkspace;

    let _ = NSWorkspace::sharedWorkspace().openURL(url);
}

/// The `ObjC` ivars of [`NotificationDelegate`]: none — the delegate only
/// translates action responses into URL opens.
#[derive(Debug)]
struct NotificationDelegateIvars {}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `NotificationDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "WaterkitNotificationDelegate"]
    #[ivars = NotificationDelegateIvars]
    #[derive(Debug)]
    struct NotificationDelegate;

    unsafe impl NSObjectProtocol for NotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        /// URL actions carry their URL as the action identifier; a tap opens
        /// it through the platform's URL opener.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive_response(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &DynBlock<dyn Fn()>,
        ) {
            if response
                .downcast_ref::<UNTextInputNotificationResponse>()
                .is_some()
            {
                tracing::debug!("Text input notification action received");
            } else {
                let action_id = response.actionIdentifier();
                if !action_id.isEqualToString(default_action_identifier())
                    && !action_id.isEqualToString(dismiss_action_identifier())
                    && let Some(url) = NSURL::URLWithString(&action_id)
                {
                    open_url(&url);
                }
            }
            completion_handler.call(());
        }

        /// Foreground presentation mirrors the `#available` split:
        /// banners where the OS has them, alerts below.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present_notification(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            let options = if has_availability(14, 11) {
                UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::Sound
            } else {
                legacy_presentation_options()
            };
            completion_handler.call((options,));
        }
    }
);

impl NotificationDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(NotificationDelegateIvars {});
        // SAFETY: `this` is a freshly allocated `NotificationDelegate` and
        // `NSObject`'s `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }
}

/// Installs the action delegate once per process.
///
/// The center owns exactly one `delegate` (a weak property), so the
/// installed `NotificationDelegate` is deliberately retained for the
/// process lifetime — it is the center's one delegate, the same residency
/// the process-shared delegate had. An installed
/// delegate of our class is kept; a delegate of any other class means the
/// host already owns the notification center's delegate, which is an
/// error — silently keeping it would stop action taps from opening their
/// URLs with no signal.
fn install_delegate(center: &UNUserNotificationCenter) -> Result<(), NotificationError> {
    if let Some(delegate) = center.delegate() {
        return delegate.downcast::<NotificationDelegate>().map_or_else(
            |_| {
                Err(NotificationError::Platform(
                    "UNUserNotificationCenter already has a delegate owned by the host; the host owns the notification center's delegate and waterkit cannot install its action handler"
                        .into(),
                ))
            },
            |_| Ok(()),
        );
    }
    let delegate = NotificationDelegate::new();
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    std::mem::forget(delegate);
    Ok(())
}

/// The owned fields of a [`Notification`] the completion blocks build
/// content from (an `&Notification` cannot cross into the `'static` block).
struct NotificationParams {
    id: String,
    title: String,
    body: String,
    subtitle: String,
    interruption_level: InterruptionLevel,
    actions: Vec<Action>,
    text_input_actions: Vec<TextInputAction>,
}

/// `UNMutableNotificationContent` populated with the original behavior:
/// sound always, subtitle only when non-empty, the interruption level where
/// the OS supports it, and a `waterkit_<id>` category carrying every action
/// when the notification has any.
fn notification_content(
    center: &UNUserNotificationCenter,
    notification: &NotificationParams,
) -> Retained<UNMutableNotificationContent> {
    // SAFETY: `[[UNMutableNotificationContent alloc] init]`.
    let content: Retained<UNMutableNotificationContent> =
        unsafe { msg_send![UNMutableNotificationContent::class(), new] };
    content.setTitle(&NSString::from_str(&notification.title));
    content.setBody(&NSString::from_str(&notification.body));
    if !notification.subtitle.is_empty() {
        content.setSubtitle(&NSString::from_str(&notification.subtitle));
    }
    content.setSound(Some(&UNNotificationSound::defaultSound()));

    // `#available(iOS 15.0, macOS 12.0, *)` — the property does not exist
    // below the gate.
    if has_availability(15, 12) {
        content.setInterruptionLevel(match notification.interruption_level {
            InterruptionLevel::Passive => UNNotificationInterruptionLevel::Passive,
            InterruptionLevel::Active => UNNotificationInterruptionLevel::Active,
            InterruptionLevel::TimeSensitive => UNNotificationInterruptionLevel::TimeSensitive,
            InterruptionLevel::Critical => UNNotificationInterruptionLevel::Critical,
        });
    }

    let mut all_actions: Vec<Retained<UNNotificationAction>> = notification
        .actions
        .iter()
        .map(|action: &Action| {
            // The URL the delegate opens on a tap is the action identifier.
            UNNotificationAction::actionWithIdentifier_title_options(
                &NSString::from_str(&action.url),
                &NSString::from_str(&action.label),
                UNNotificationActionOptions::Foreground,
            )
        })
        .collect();
    all_actions.extend(notification.text_input_actions.iter().map(|action: &TextInputAction| {
        Retained::into_super(UNTextInputNotificationAction::actionWithIdentifier_title_options_textInputButtonTitle_textInputPlaceholder(
            &NSString::from_str(&action.id),
            &NSString::from_str(&action.label),
            UNNotificationActionOptions::Foreground,
            &NSString::from_str(&action.submit_label),
            &NSString::from_str(&action.placeholder),
        ))
    }));

    if !all_actions.is_empty() {
        let category_id = NSString::from_str(&format!("waterkit_{}", notification.id));
        let category =
            UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                &category_id,
                &NSArray::from_retained_slice(&all_actions),
                &NSArray::new(),
                UNNotificationCategoryOptionNone,
            );
        center.setNotificationCategories(&NSSet::setWithObject(&*category));
        content.setCategoryIdentifier(&category_id);
    }

    content
}

/// `showNotificationAsync`'s port: the work rides the same
/// completion-handler chain, and the delivery callback's send on the
/// oneshot is the only signal the caller waits on.
async fn show(notification: &Notification) -> Result<(), NotificationError> {
    // On macOS, `UNUserNotificationCenter` requires a valid app bundle.
    #[cfg(target_os = "macos")]
    if objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .is_none()
    {
        return Err(NotificationError::Platform(
            "UNUserNotificationCenter requires a valid app bundle".into(),
        ));
    }

    let notification = NotificationParams {
        id: notification.id.clone().unwrap_or_default(),
        title: notification.title.clone(),
        body: notification.body.clone(),
        subtitle: notification.subtitle.clone().unwrap_or_default(),
        interruption_level: notification.interruption_level,
        actions: notification.actions.clone(),
        text_input_actions: notification.text_input_actions.clone(),
    };

    let receiver = {
        let center = UNUserNotificationCenter::currentNotificationCenter();
        install_delegate(&center)?;
        let (sender, receiver) = oneshot::channel();
        // A completion handler can fire the authorization callback at most
        // once, but the `RefCell` is what lets the send move into the
        // delivery block that runs after it.
        let sender = Rc::new(RefCell::new(Some(sender)));
        let deliver_center = center.clone();
        let authorize = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            let Some(result_sender) = sender.borrow_mut().take() else {
                return;
            };
            if !(granted.as_bool() && error.is_null()) {
                let _ = result_sender.send(Err(if granted.as_bool() {
                    NotificationError::Platform(describe_error(error))
                } else {
                    NotificationError::PermissionDenied
                }));
                return;
            }
            let content = notification_content(&deliver_center, &notification);
            // The request reuses the notification's id: posting the same
            // identifier replaces the delivered notification.
            let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
                &NSString::from_str(&notification.id),
                &Retained::into_super(content),
                None,
            );
            // The `Rc` keeps the sender reachable on later (unexpected) calls so
            // the block stays `Fn`.
            let sender = Rc::new(RefCell::new(Some(result_sender)));
            let deliver = RcBlock::new(move |error: *mut NSError| {
                if let Some(sender) = sender.borrow_mut().take() {
                    let _ = sender.send(if error.is_null() {
                        Ok(())
                    } else {
                        Err(NotificationError::Platform(describe_error(error)))
                    });
                }
            });
            deliver_center.addNotificationRequest_withCompletionHandler(&request, Some(&deliver));
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &authorize,
        );
        // `requestAuthorizationWithOptions:completionHandler:` copies the
        // block, so the `RcBlock`s can drop here — before the future is
        // awaited — keeping `show`'s future `Send`.
        receiver
    };

    // The receiver resolves when the delivery handler fires; the sender
    // only drops early if the center never invokes its completion blocks.
    receiver.await.map_err(|_| {
        NotificationError::Platform(
            "the notification delivery completion handler was never invoked".into(),
        )
    })?
}

/// Handle to a shown notification (iOS/macOS).
#[derive(Debug)]
pub struct NotificationHandleInner;

/// Show a notification using `UserNotifications`.
pub async fn show_notification(
    notification: &Notification,
) -> Result<NotificationHandleInner, NotificationError> {
    show(notification).await?;
    Ok(NotificationHandleInner)
}
