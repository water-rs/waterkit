//! iOS Apple Wallet integration through `PassKit` and `UIKit`, via `objc2`.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use dispatch2::DispatchQueue;
use futures::channel::oneshot;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, extern_class,
    extern_methods, extern_protocol, msg_send,
};
use objc2_foundation::{NSArray, NSData, NSObjectProtocol};
use objc2_pass_kit::{PKPass, PKPassLibrary, PKPassLibraryAddPassesStatus};
use objc2_ui_kit::{
    UIApplication, UIResponder, UISceneActivationState, UIViewController, UIWindowScene,
};

use crate::{AddOutcome, ApplePasses, WalletCapabilities, WalletError};

type AddSender = oneshot::Sender<Result<AddOutcome, WalletError>>;

extern_class!(
    /// `PKAddPassesViewController` is missing from `objc2-pass-kit` 0.3.2 (its
    /// generated file is empty, including upstream on `objc2` main), so the
    /// items the wallet flow uses are declared here.
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PKAddPassesViewController"]
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct PKAddPassesViewController;
);

impl PKAddPassesViewController {
    extern_methods!(
        /// `+canAddPasses` reports whether the device can present the add-pass
        /// review UI.
        #[unsafe(method(canAddPasses))]
        #[unsafe(method_family = none)]
        pub fn can_add_passes() -> bool;

        /// `-initWithPass:` presents a single pass for review; returns `nil`
        /// when the pass cannot be added.
        #[unsafe(method(initWithPass:))]
        #[unsafe(method_family = init)]
        pub unsafe fn init_with_pass(
            this: Allocated<Self>,
            pass: &PKPass,
        ) -> Option<Retained<Self>>;

        /// `-initWithPasses:` presents several passes for review; returns `nil`
        /// when the passes cannot be added.
        #[unsafe(method(initWithPasses:))]
        #[unsafe(method_family = init)]
        pub unsafe fn init_with_passes(
            this: Allocated<Self>,
            passes: &NSArray<PKPass>,
        ) -> Option<Retained<Self>>;

        /// `delegate` is an `assign` (non-retaining) property per the PassKit
        /// header; the delegate keeps itself alive through its ivars.
        #[unsafe(method(setDelegate:))]
        #[unsafe(method_family = none)]
        pub unsafe fn set_delegate(
            &self,
            delegate: Option<&ProtocolObject<dyn PKAddPassesViewControllerDelegate>>,
        );
    );
}

extern_protocol!(
    /// `PKAddPassesViewControllerDelegate` is missing from `objc2-pass-kit`
    /// 0.3.2 alongside the controller; declared here.
    ///
    /// # Safety
    ///
    /// Implementations must implement the required
    /// `addPassesViewControllerDidFinish:` callback.
    #[expect(
        clippy::missing_safety_doc,
        reason = "clippy does not pick up doc comments written inside `extern_protocol!` input"
    )]
    pub unsafe trait PKAddPassesViewControllerDelegate: NSObjectProtocol {
        /// Called when the user has finished reviewing the add-pass flow.
        #[unsafe(method(addPassesViewControllerDidFinish:))]
        #[unsafe(method_family = none)]
        unsafe fn add_passes_view_controller_did_finish(
            &self,
            controller: &PKAddPassesViewController,
        );
    }
);

/// ivars of [`WalletAddPassesDelegate`].
pub struct WalletAddPassesDelegateIvars {
    /// The passes the flow was started for; matched against the pass library
    /// after the review UI is dismissed.
    passes: Vec<Retained<PKPass>>,
    sender: RefCell<Option<AddSender>>,
    /// Strong self-retain: `PKAddPassesViewController.delegate` is a weak
    /// (`assign`) property, so the delegate keeps itself alive until
    /// `addPassesViewControllerDidFinish:`'s dismissal completion runs.
    keep_alive: RefCell<Option<Retained<WalletAddPassesDelegate>>>,
}

define_class!(
    /// Delegate for the add-pass review controller. The request object owns
    /// its state; the old global `retainedDelegates` registry is replaced by
    /// the `keep_alive` ivar.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WalletAddPassesDelegate"]
    #[ivars = WalletAddPassesDelegateIvars]
    pub struct WalletAddPassesDelegate;

    unsafe impl NSObjectProtocol for WalletAddPassesDelegate {}

    unsafe impl PKAddPassesViewControllerDelegate for WalletAddPassesDelegate {
        /// Dismisses the review UI and reports Added when every pass reached
        /// the pass library, Cancelled otherwise.
        #[unsafe(method(addPassesViewControllerDidFinish:))]
        fn add_passes_view_controller_did_finish(&self, controller: &PKAddPassesViewController) {
            let this = self.retain();
            let completion = RcBlock::new(move || {
                let ivars = this.ivars();
                let sender = ivars
                    .sender
                    .borrow_mut()
                    .take()
                    .expect("waterkit-wallet: add-pass callback was already completed");
                // SAFETY: `new` is a convenience constructor with no invariants
                // to uphold.
                let library = unsafe { PKPassLibrary::new() };
                // SAFETY: `library` and every element are live objects.
                let added = ivars
                    .passes
                    .iter()
                    .all(|pass| unsafe { library.containsPass(pass) });
                let _ = sender.send(if added {
                    Ok(AddOutcome::Added)
                } else {
                    Ok(AddOutcome::Cancelled)
                });
                *ivars.keep_alive.borrow_mut() = None;
            });
            controller.dismissViewControllerAnimated_completion(true, Some(&completion));
        }
    }
);

impl WalletAddPassesDelegate {
    fn new(
        passes: Vec<Retained<PKPass>>,
        sender: AddSender,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WalletAddPassesDelegateIvars {
            passes,
            sender: RefCell::new(Some(sender)),
            keep_alive: RefCell::new(None),
        });
        // SAFETY: `this` is a freshly allocated `WalletAddPassesDelegate` and
        // `NSObject`'s `init` has no additional requirements.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.ivars().keep_alive.replace(Some(this.clone()));
        this
    }
}

pub async fn capabilities() -> Result<WalletCapabilities, WalletError> {
    Ok(WalletCapabilities {
        available: wallet_is_available(),
    })
}

pub async fn add(passes: ApplePasses) -> Result<AddOutcome, WalletError> {
    let receiver = add_inner(&passes);
    receiver
        .await
        .map_err(|_| WalletError::Platform("iOS wallet callback channel closed".into()))?
}

/// Hands the raw pass bytes to the main queue. Only `Send` data (the
/// `Vec<Vec<u8>>` and the oneshot's sender) crosses the queue hop; the `PKPass`
/// objects — `!Send` — are built on the main queue.
fn add_inner(passes: &ApplePasses) -> oneshot::Receiver<Result<AddOutcome, WalletError>> {
    let bytes = passes
        .iter()
        .map(|pass| pass.as_bytes().to_vec())
        .collect::<Vec<Vec<u8>>>();
    let (sender, receiver) = oneshot::channel();
    DispatchQueue::main().exec_async(move || add_on_main(bytes, sender));
    receiver
}

/// Builds the `PKPass` objects from their serialized bytes, mapping a parse
/// failure to `WalletError::InvalidPass` with the pass's index.
fn build_pk_passes(bytes: &[Vec<u8>]) -> Result<Vec<Retained<PKPass>>, WalletError> {
    let mut passes = Vec::with_capacity(bytes.len());
    for (index, pass_bytes) in bytes.iter().enumerate() {
        let data = NSData::from_vec(pass_bytes.clone());
        // SAFETY: `initWithData:error:` is the designated initializer; `data`
        // is a live `NSData`.
        let pass =
            unsafe { PKPass::initWithData_error(PKPass::alloc(), &data) }.map_err(|error| {
                WalletError::InvalidPass(format!(
                    "pass at index {index}: {}",
                    error.localizedDescription()
                ))
            })?;
        passes.push(pass);
    }
    Ok(passes)
}

/// Runs on the main queue.
fn add_on_main(pass_bytes: Vec<Vec<u8>>, sender: AddSender) {
    // SAFETY: this function executes on the main queue.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let passes = match build_pk_passes(&pass_bytes) {
        Ok(passes) => passes,
        Err(error) => {
            let _ = sender.send(Err(error));
            return;
        }
    };
    if !wallet_is_available() {
        let _ = sender.send(Err(WalletError::Unavailable));
        return;
    }
    let Some(view_controller) = foreground_top_view_controller(mtm) else {
        let _ = sender.send(Err(WalletError::Platform(
            "no foreground window to present Wallet from".into(),
        )));
        return;
    };
    if passes.len() == 1 {
        present_review(passes, &view_controller, sender, mtm);
        return;
    }
    // SAFETY: `new` is a convenience constructor with no invariants to
    // uphold.
    let library = unsafe { PKPassLibrary::new() };
    let passes_array = NSArray::from_retained_slice(&passes);
    let sender = Arc::new(Mutex::new(Some(sender)));
    let completion = RcBlock::new(move |status: PKPassLibraryAddPassesStatus| {
        let report = |result: Result<AddOutcome, WalletError>| {
            if let Ok(mut guard) = sender.lock()
                && let Some(sender) = guard.take()
            {
                let _ = sender.send(result);
            }
        };
        match status {
            PKPassLibraryAddPassesStatus::DidAddPasses => report(Ok(AddOutcome::Added)),
            PKPassLibraryAddPassesStatus::DidCancelAddPasses => {
                report(Ok(AddOutcome::Cancelled));
            }
            PKPassLibraryAddPassesStatus::ShouldReviewPasses => {
                // Only `Send` data crosses back to the main queue: the pass
                // bytes and the shared sender. The `PKPass` objects and the
                // presenting view controller are rebuilt/re-resolved there —
                // this completion may run on an arbitrary queue.
                let pass_bytes = pass_bytes.clone();
                let sender = Arc::clone(&sender);
                DispatchQueue::main().exec_async(move || {
                    let sender = sender
                        .lock()
                        .expect("wallet add sender mutex poisoned")
                        .take()
                        .expect("waterkit-wallet: add-pass callback was already completed");
                    // SAFETY: this closure executes on the main queue.
                    let mtm = unsafe { MainThreadMarker::new_unchecked() };
                    let passes = match build_pk_passes(&pass_bytes) {
                        Ok(passes) => passes,
                        Err(error) => {
                            let _ = sender.send(Err(error));
                            return;
                        }
                    };
                    let Some(view_controller) = foreground_top_view_controller(mtm) else {
                        let _ = sender.send(Err(WalletError::Platform(
                            "no foreground window to present Wallet from".into(),
                        )));
                        return;
                    };
                    present_review(passes, &view_controller, sender, mtm);
                });
            }
            _ => report(Err(WalletError::Platform(format!(
                "unknown PassKit add-pass status {}",
                status.0
            )))),
        }
    });
    // SAFETY: `library` is live, the array only contains `PKPass` objects,
    // and `completion` matches the documented `addPasses:` block signature.
    unsafe { library.addPasses_withCompletionHandler(&passes_array, Some(&completion)) };
}

fn present_review(
    passes: Vec<Retained<PKPass>>,
    view_controller: &UIViewController,
    sender: AddSender,
    mtm: MainThreadMarker,
) {
    let allocated = mtm.alloc();
    // SAFETY: `initWithPass:`/`initWithPasses:` are the designated
    // initializers; the pass and array arguments are live objects.
    let controller = if passes.len() == 1 {
        unsafe { PKAddPassesViewController::init_with_pass(allocated, &passes[0]) }
    } else {
        let array = NSArray::from_retained_slice(&passes);
        unsafe { PKAddPassesViewController::init_with_passes(allocated, &array) }
    };
    let Some(controller) = controller else {
        let _ = sender.send(Err(WalletError::Unavailable));
        return;
    };
    let delegate = WalletAddPassesDelegate::new(passes, sender, mtm);
    // SAFETY: `controller` is live and `delegate` conforms to the protocol; the
    // delegate keeps itself alive through its `keep_alive` ivar because the
    // property is `assign`.
    unsafe { controller.set_delegate(Some(ProtocolObject::from_ref(&*delegate))) };
    view_controller.presentViewController_animated_completion(&controller, true, None);
}

fn wallet_is_available() -> bool {
    // SAFETY: class methods with no invariants to uphold.
    unsafe {
        PKPassLibrary::isPassLibraryAvailable() && PKAddPassesViewController::can_add_passes()
    }
}

fn foreground_top_view_controller(mtm: MainThreadMarker) -> Option<Retained<UIViewController>> {
    let application = UIApplication::sharedApplication(mtm);
    let window_scene = application.connectedScenes().iter().find_map(|scene| {
        let scene = scene.downcast_ref::<UIWindowScene>()?;
        (scene.activationState() == UISceneActivationState::ForegroundActive)
            .then(|| scene.retain())
    });
    let window = window_scene.and_then(|scene| {
        scene
            .windows()
            .iter()
            .find(|window| window.isKeyWindow())
            .map(|window| window.retain())
    })?;
    let mut top = window.rootViewController();
    while let Some(presented) = top.as_ref().and_then(|vc| vc.presentedViewController()) {
        top = Some(presented);
    }
    top
}
