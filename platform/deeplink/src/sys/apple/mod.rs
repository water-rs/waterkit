//! Apple platform (iOS/macOS) deep link implementation through objc2.
//!
//! `open_url` goes through `UIApplication.shared.open` on iOS and
//! `NSWorkspace.shared.open` on macOS. Incoming links on macOS arrive as
//! `kAEGetURL` Apple events; the handler object's ivars own the delivery
//! sender and the pending initial URL instead of the globals the previous
//! implementation used.

use crate::{DeepLink, DeepLinkError};

use objc2_foundation::{NSString, NSURL};
use waterkit_core::apple::on_main;
#[cfg(target_os = "ios")]
use {
    block2::RcBlock,
    objc2_foundation::{NSDictionary, NSNotification, NSNotificationCenter, NSOperationQueue},
    objc2_ui_kit::{UIApplication, UISceneWillConnectNotification},
};
#[cfg(target_os = "macos")]
use {
    objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, rc::Retained, sel},
    objc2_app_kit::NSWorkspace,
    objc2_core_services::{AEEventClass, AEEventID, keyDirectObject},
    objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSObject},
    std::cell::RefCell,
};

/// # Errors
///
/// Returns [`DeepLinkError::Platform`] when the URL cannot be opened or the
/// completion callback is dropped.
pub async fn open_url(url: &str) -> Result<(), DeepLinkError> {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return Err(DeepLinkError::Platform("failed to open URL".into()));
    };
    let (tx, rx) = futures::channel::oneshot::channel();

    #[cfg(target_os = "ios")]
    {
        let tx = std::sync::Mutex::new(Some(tx));
        on_main(move |mtm| {
            let application = UIApplication::sharedApplication(mtm);
            // The completion fires on an internal queue, so it is a `Fn`
            // block and the one-shot sender is taken through the mutex. The
            // block also retains the application, so the object the request
            // was issued on lives until the framework answers.
            let completion = RcBlock::new({
                let application = application.clone();
                move |success: objc2::runtime::Bool| {
                    let _application = &application;
                    let _ = tx.lock().expect("sender mutex poisoned").take().map(|tx| {
                        tx.send(if success.as_bool() {
                            Ok(())
                        } else {
                            Err(DeepLinkError::Platform("failed to open URL".into()))
                        })
                    });
                }
            });
            // SAFETY: `openURL:options:completionHandler:` runs the
            // completion on a private queue inside UIKit; the block's
            // `Send` captures are channel state only — the retained
            // application is never touched off the main thread.
            unsafe {
                application.openURL_options_completionHandler(
                    &url,
                    &NSDictionary::new(),
                    Some(&completion),
                );
            }
        })
        .await;
    }
    #[cfg(target_os = "macos")]
    {
        let success = on_main(move |_mtm| NSWorkspace::sharedWorkspace().openURL(&url)).await;
        let _ = tx.send(if success {
            Ok(())
        } else {
            Err(DeepLinkError::Platform("failed to open URL".into()))
        });
    }

    rx.await
        .map_err(|_| DeepLinkError::Platform("callback dropped".into()))?
}

/// # Errors
///
/// Infallible today; returns [`DeepLinkError::InvalidUrl`] when the URL
/// string cannot be parsed.
#[cfg_attr(
    target_os = "macos",
    expect(
        clippy::unused_async,
        reason = "the facade calls every platform's backend through the same async signature; on iOS it awaits the main-queue hop and only the macOS branch has nothing to await"
    )
)]
pub async fn can_open_url(url: &str) -> Result<bool, DeepLinkError> {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return Ok(false);
    };
    #[cfg(target_os = "ios")]
    {
        Ok(on_main(move |mtm| UIApplication::sharedApplication(mtm).canOpenURL(&url)).await)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = url;
        Ok(true)
    }
}

#[cfg(target_os = "macos")]
struct DeepLinkIvars {
    /// The channel incoming links are delivered on while listening.
    sender: RefCell<Option<async_channel::Sender<DeepLink>>>,
    /// A link that arrived with no active listener (e.g. during `stop`).
    initial_url: RefCell<Option<String>>,
}

#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "WaterkitDeepLinkHandler"]
    #[ivars = DeepLinkIvars]
    struct DeepLinkHandler;

    impl DeepLinkHandler {
        /// `kInternetEventClass` / `kAEGetURL` Apple event handler
        /// (`handleGetURLEvent:withReplyEvent:`).
        #[unsafe(method(handleGetURL:reply:))]
        fn handle_get_url(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            let Some(url) = event
                .paramDescriptorForKeyword(keyDirectObject)
                .and_then(|descriptor| descriptor.stringValue())
                .map(|value| value.to_string())
            else {
                return;
            };
            let ivars = self.ivars();
            if let Some(sender) = ivars.sender.borrow().as_ref() {
                if let Ok(link) = DeepLink::parse(&url) {
                    let _ = sender.try_send(link);
                }
            } else {
                *ivars.initial_url.borrow_mut() = Some(url);
            }
        }
    }
);

#[cfg(target_os = "macos")]
impl DeepLinkHandler {
    fn new() -> Retained<Self> {
        // SAFETY: `alloc`/`init` pair on a plain NSObject subclass.
        let this = Self::alloc().set_ivars(DeepLinkIvars {
            sender: RefCell::new(None),
            initial_url: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }
}

/// `kInternetEventClass` (`'GURL'`), not exported by `objc2-core-services`.
#[cfg(target_os = "macos")]
const K_INTERNET_EVENT_CLASS: AEEventClass = 0x4755_524c;
/// `kAEGetURL` (`'GURL'`), not exported by `objc2-core-services`.
#[cfg(target_os = "macos")]
const K_AE_GET_URL: AEEventID = 0x4755_524c;

pub struct DeepLinkHandlerInner {
    /// Keeps the channel open and, on macOS, owns the Apple-event handler
    /// object (which owns the sender).
    #[cfg(target_os = "macos")]
    handler: Retained<DeepLinkHandler>,
    #[cfg(target_os = "ios")]
    _link_tx: async_channel::Sender<DeepLink>,
}

impl std::fmt::Debug for DeepLinkHandlerInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepLinkHandlerInner")
            .finish_non_exhaustive()
    }
}

impl DeepLinkHandlerInner {
    /// # Errors
    ///
    /// Infallible today; reserved for parity with other platforms.
    #[expect(
        clippy::unused_async,
        reason = "the facade calls every platform's backend through the same async signature; only this platform's implementation is synchronous"
    )]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        let (link_tx, link_rx) = async_channel::bounded(16);

        #[cfg(target_os = "macos")]
        {
            let handler = DeepLinkHandler::new();
            *handler.ivars().sender.borrow_mut() = Some(link_tx);
            // SAFETY: installs `handler` for `kAEGetURL` Apple events; the
            // shared event manager retains it until `stop` removes it.
            unsafe {
                NSAppleEventManager::sharedAppleEventManager()
                    .setEventHandler_andSelector_forEventClass_andEventID(
                        &handler,
                        sel!(handleGetURL:reply:),
                        K_INTERNET_EVENT_CLASS,
                        K_AE_GET_URL,
                    );
            }
            Ok((Self { handler }, link_rx))
        }

        #[cfg(target_os = "ios")]
        {
            // Mirrors the previous implementation exactly: an observer for
            // `UIScene.willConnectNotification` whose body is inert (scene
            // URL contexts are delivered by the app's scene delegate, which
            // this crate does not provide). The token is dropped; the
            // registration lives forever, as before.
            let observer =
                RcBlock::new(move |_notification: core::ptr::NonNull<NSNotification>| {});
            // SAFETY: the block is sendable (it captures nothing) and the
            // observer is registered on the main queue.
            unsafe {
                let center = NSNotificationCenter::defaultCenter();
                let _ = center.addObserverForName_object_queue_usingBlock(
                    Some(UISceneWillConnectNotification),
                    None,
                    Some(&NSOperationQueue::mainQueue()),
                    &observer,
                );
            }
            Ok((Self { _link_tx: link_tx }, link_rx))
        }
    }

    /// # Errors
    ///
    /// Returns [`DeepLinkError`] when a stored initial URL fails to parse.
    #[cfg_attr(
        target_os = "ios",
        expect(
            clippy::missing_const_for_fn,
            reason = "the facade calls every platform's backend through the same non-const signature"
        )
    )]
    #[cfg_attr(
        target_os = "ios",
        expect(
            clippy::unnecessary_wraps,
            reason = "the facade signature returns Result on every platform"
        )
    )]
    #[cfg_attr(
        target_os = "ios",
        expect(
            clippy::unused_self,
            reason = "the facade calls every platform's backend through the same &self signature; only the iOS branch does not read it"
        )
    )]
    pub fn initial_link(&self) -> Result<Option<DeepLink>, DeepLinkError> {
        #[cfg(target_os = "macos")]
        {
            self.handler
                .ivars()
                .initial_url
                .borrow()
                .as_deref()
                .map(DeepLink::parse)
                .transpose()
        }
        #[cfg(target_os = "ios")]
        {
            Ok(None)
        }
    }

    #[cfg_attr(
        target_os = "ios",
        expect(
            clippy::missing_const_for_fn,
            reason = "the facade calls every platform's backend through the same non-const signature"
        )
    )]
    #[cfg_attr(
        target_os = "ios",
        expect(
            clippy::unused_self,
            reason = "the facade calls every platform's backend through the same &self signature; only the iOS branch does not read it"
        )
    )]
    pub fn stop(&self) {
        #[cfg(target_os = "macos")]
        {
            self.handler.ivars().sender.borrow_mut().take();
            NSAppleEventManager::sharedAppleEventManager()
                .removeEventHandlerForEventClass_andEventID(K_INTERNET_EVENT_CLASS, K_AE_GET_URL);
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for DeepLinkHandlerInner {
    fn drop(&mut self) {
        self.stop();
    }
}
