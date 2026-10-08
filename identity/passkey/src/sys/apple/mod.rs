//! Apple passkey backend via `AuthenticationServices` through `objc2`.
//!
//! Each ceremony is a `define_class!` delegate object that owns its state —
//! the oneshot sender, the `ASAuthorizationController` (the controller's
//! `delegate`/`presentationContextProvider` properties are weak, so the
//! delegate self-retains until the ceremony finishes). Requests run on the
//! main thread; callers hop through [`waterkit_core::apple::on_main`].

use async_trait::async_trait;
use core::cell::RefCell;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_authentication_services::{
    ASAuthorization, ASAuthorizationController, ASAuthorizationControllerDelegate,
    ASAuthorizationControllerPresentationContextProviding,
    ASAuthorizationPlatformPublicKeyCredentialAssertion,
    ASAuthorizationPlatformPublicKeyCredentialDescriptor,
    ASAuthorizationPlatformPublicKeyCredentialProvider,
    ASAuthorizationPlatformPublicKeyCredentialRegistration,
    ASAuthorizationPublicKeyCredentialAssertion,
    ASAuthorizationPublicKeyCredentialAssertionRequest,
    ASAuthorizationPublicKeyCredentialAttestationKindDirect,
    ASAuthorizationPublicKeyCredentialAttestationKindEnterprise,
    ASAuthorizationPublicKeyCredentialAttestationKindIndirect,
    ASAuthorizationPublicKeyCredentialAttestationKindNone,
    ASAuthorizationPublicKeyCredentialRegistration,
    ASAuthorizationPublicKeyCredentialRegistrationRequest,
    ASAuthorizationPublicKeyCredentialUserVerificationPreferenceDiscouraged,
    ASAuthorizationPublicKeyCredentialUserVerificationPreferencePreferred,
    ASAuthorizationPublicKeyCredentialUserVerificationPreferenceRequired, ASAuthorizationRequest,
    ASPublicKeyCredential,
};
use objc2_foundation::{
    NSArray, NSData, NSError, NSObjectProtocol, NSOperatingSystemVersion, NSProcessInfo, NSString,
};
use tokio::sync::oneshot;

use crate::{
    AttestationPreference, AuthenticateOptions, AuthenticationResult, Availability, PasskeyError,
    RegisterOptions, RegistrationResult, UserVerificationRequirement,
};

use super::PasskeyBackend;

/// The `#available(iOS major, macOS major)`-style OS gates of the original
/// implementation, as one `NSProcessInfo` version probe: `ios_major` applies on iOS
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

/// `userVerificationPreference(from:)` — the extern string constants
/// `AuthenticationServices` exports.
fn user_verification_preference(value: UserVerificationRequirement) -> &'static NSString {
    match value {
        UserVerificationRequirement::Discouraged => {
            // SAFETY: an immutable extern string constant of
            // AuthenticationServices; read-only access.
            unsafe { ASAuthorizationPublicKeyCredentialUserVerificationPreferenceDiscouraged }
                .expect("ASAuthorizationPublicKeyCredentialUserVerificationPreferenceDiscouraged requires the passkey API (iOS 16/macOS 13)")
        }
        UserVerificationRequirement::Preferred => {
            // SAFETY: an immutable extern string constant of
            // AuthenticationServices; read-only access.
            unsafe { ASAuthorizationPublicKeyCredentialUserVerificationPreferencePreferred }
                .expect("ASAuthorizationPublicKeyCredentialUserVerificationPreferencePreferred requires the passkey API (iOS 16/macOS 13)")
        }
        UserVerificationRequirement::Required => {
            // SAFETY: an immutable extern string constant of
            // AuthenticationServices; read-only access.
            unsafe { ASAuthorizationPublicKeyCredentialUserVerificationPreferenceRequired }
                .expect("ASAuthorizationPublicKeyCredentialUserVerificationPreferenceRequired requires the passkey API (iOS 16/macOS 13)")
        }
    }
}

/// `attestationPreference(from:)` — the extern string constants
/// `AuthenticationServices` exports.
fn attestation_preference(value: AttestationPreference) -> &'static NSString {
    match value {
        AttestationPreference::Direct => {
            // SAFETY: an immutable extern string constant of
            // AuthenticationServices; read-only access.
            unsafe { ASAuthorizationPublicKeyCredentialAttestationKindDirect }
                .expect("ASAuthorizationPublicKeyCredentialAttestationKindDirect requires the passkey API (iOS 16/macOS 13)")
        }
        AttestationPreference::Indirect => {
            // SAFETY: an immutable extern string constant of
            // AuthenticationServices; read-only access.
            unsafe { ASAuthorizationPublicKeyCredentialAttestationKindIndirect }
                .expect("ASAuthorizationPublicKeyCredentialAttestationKindIndirect requires the passkey API (iOS 16/macOS 13)")
        }
        AttestationPreference::Enterprise => {
            // SAFETY: an immutable extern string constant of
            // AuthenticationServices; read-only access.
            unsafe { ASAuthorizationPublicKeyCredentialAttestationKindEnterprise }
                .expect("ASAuthorizationPublicKeyCredentialAttestationKindEnterprise requires the passkey API (iOS 16/macOS 13)")
        }
        AttestationPreference::None => {
            // SAFETY: an immutable extern string constant of
            // AuthenticationServices; read-only access.
            unsafe { ASAuthorizationPublicKeyCredentialAttestationKindNone }
                .expect("ASAuthorizationPublicKeyCredentialAttestationKindNone requires the passkey API (iOS 16/macOS 13)")
        }
    }
}

/// `resolvePresentationAnchor`'s iOS half: the key window of any connected
/// `UIWindowScene`, or a fresh off-screen `UIWindow` as the last resort.
/// (`ASPresentationAnchor` is an untyped `id`; every anchor is really a
/// `UIWindow`.)
#[cfg(target_os = "ios")]
fn presentation_anchor(mtm: MainThreadMarker) -> Retained<objc2_ui_kit::UIWindow> {
    use objc2_ui_kit::{UIApplication, UIWindowScene};

    let scenes = UIApplication::sharedApplication(mtm).connectedScenes();
    for scene in &scenes {
        if let Some(window_scene) = scene.downcast_ref::<UIWindowScene>() {
            for window in &window_scene.windows() {
                if window.isKeyWindow() {
                    return window.retain();
                }
            }
        }
    }

    // `UIWindow(frame: .zero)` — an empty window still anchors the sheet.
    objc2_ui_kit::UIWindow::new(mtm)
}

/// `resolvePresentationAnchor`'s macOS half: the app's key window, else its
/// first window, else a fresh `NSWindow`. (`ASPresentationAnchor` is an
/// untyped `id`; every anchor is really an `NSWindow`.)
#[cfg(target_os = "macos")]
fn presentation_anchor(mtm: MainThreadMarker) -> Retained<objc2_app_kit::NSWindow> {
    use objc2_app_kit::NSApplication;

    let app = NSApplication::sharedApplication(mtm);
    if let Some(window) = app.keyWindow() {
        return window;
    }
    if let Some(window) = app.windows().firstObject() {
        return window.retain();
    }
    // `NSWindow()` — an empty window still anchors the sheet.
    // SAFETY: `[[NSWindow alloc] init]`, main thread held by `mtm`.
    unsafe { objc2_app_kit::NSWindow::new(mtm) }
}

/// The delegate's ceremony state: which oneshot the completion answers.
#[derive(Debug)]
enum PasskeyState {
    Register(oneshot::Sender<Result<RegistrationResult, PasskeyError>>),
    Authenticate(oneshot::Sender<Result<AuthenticationResult, PasskeyError>>),
}

/// The `ObjC` ivars of [`WaterkitPasskeyDelegate`].
#[derive(Debug)]
struct WaterkitPasskeyDelegateIvars {
    /// The oneshot the delegate resolves once; `Option` so finish can take
    /// it. (`oneshot::Sender` has no `Debug`, so the ivars stay undebuggable
    /// only through this field.)
    state: RefCell<Option<PasskeyState>>,
    /// Keeps the live `ASAuthorizationController` alive for the ceremony.
    controller: RefCell<Option<Retained<ASAuthorizationController>>>,
    /// Strong self-retain: the controller's delegate references are weak, so
    /// nothing else holds the delegate while a ceremony is in flight.
    keep_alive: RefCell<Option<Retained<WaterkitPasskeyDelegate>>>,
}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `WaterkitPasskeyDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WaterkitPasskeyDelegate"]
    #[ivars = WaterkitPasskeyDelegateIvars]
    #[derive(Debug)]
    struct WaterkitPasskeyDelegate;

    unsafe impl NSObjectProtocol for WaterkitPasskeyDelegate {}

    unsafe impl ASAuthorizationControllerDelegate for WaterkitPasskeyDelegate {
        /// `didCompleteWithAuthorization`: downcasts the credential to the
        /// concrete registration/assertion type the ceremony expects and
        /// resolves the oneshot with typed data.
        #[unsafe(method(authorizationController:didCompleteWithAuthorization:))]
        unsafe fn did_complete_with_authorization(
            &self,
            _controller: &ASAuthorizationController,
            authorization: &ASAuthorization,
        ) {
            let Some(state) = self.ivars().state.borrow_mut().take() else {
                return;
            };
            self.ivars().keep_alive.borrow_mut().take();
            self.ivars().controller.borrow_mut().take();

            // SAFETY: `credential` is the credential the completed ceremony
            // produced; it is only read.
            let credential = unsafe { authorization.credential() };
            let credential: &AnyObject = (*credential).as_ref();

            match state {
                PasskeyState::Register(sender) => {
                    let result = registration_result(credential);
                    let _ = sender.send(result);
                }
                PasskeyState::Authenticate(sender) => {
                    let result = authentication_result(credential);
                    let _ = sender.send(result);
                }
            }
        }

        /// `didCompleteWithError`: forwards the localized description through
        /// the crate's shared `from_platform_error` mapping.
        #[unsafe(method(authorizationController:didCompleteWithError:))]
        unsafe fn did_complete_with_error(
            &self,
            _controller: &ASAuthorizationController,
            error: &NSError,
        ) {
            let Some(state) = self.ivars().state.borrow_mut().take() else {
                return;
            };
            self.ivars().keep_alive.borrow_mut().take();
            self.ivars().controller.borrow_mut().take();

            let error = PasskeyError::from_platform_error(
                error.localizedDescription().to_string(),
            );
            match state {
                PasskeyState::Register(sender) => {
                    let _ = sender.send(Err(error));
                }
                PasskeyState::Authenticate(sender) => {
                    let _ = sender.send(Err(error));
                }
            }
        }
    }

    unsafe impl ASAuthorizationControllerPresentationContextProviding
        for WaterkitPasskeyDelegate
    {
        /// `presentationAnchorForAuthorizationController:` returns an
        /// autoreleased anchor: under method family `none` the caller does
        /// not take ownership, so the anchor goes out as a raw object pointer
        /// (the selector signature encodes an object return either way).
        #[cfg(target_os = "macos")]
        #[unsafe(method(presentationAnchorForAuthorizationController:))]
        unsafe fn presentation_anchor_for_controller(
            &self,
            _controller: &ASAuthorizationController,
        ) -> *mut AnyObject {
            Retained::autorelease_ptr(presentation_anchor(
                MainThreadMarker::new().expect("presentation anchors resolve on the main thread"),
            ))
            .cast()
        }
    }

    impl WaterkitPasskeyDelegate {
        /// On iOS `presentationAnchorForAuthorizationController:` is missing
        /// from the generated protocol, so it is declared directly on the
        /// class — the runtime calls it all the same. It returns an
        /// autoreleased anchor for the same reason as the macOS impl.
        #[cfg(target_os = "ios")]
        #[unsafe(method(presentationAnchorForAuthorizationController:))]
        unsafe fn presentation_anchor_for_controller(
            &self,
            _controller: &ASAuthorizationController,
        ) -> *mut AnyObject {
            Retained::autorelease_ptr(presentation_anchor(
                MainThreadMarker::new().expect("presentation anchors resolve on the main thread"),
            ))
            .cast()
        }
    }
);

/// `RegisterControllerDelegate.didCompleteWithAuthorization`'s typed build:
/// the credential must be a platform public-key registration with its
/// attestation object.
fn registration_result(credential: &AnyObject) -> Result<RegistrationResult, PasskeyError> {
    let Some(credential) =
        credential.downcast_ref::<ASAuthorizationPlatformPublicKeyCredentialRegistration>()
    else {
        return Err(PasskeyError::from_platform_error(
            "unexpected passkey registration credential type",
        ));
    };
    // SAFETY: the credential accessors read immutable ceremony results.
    let credential_id = crate::CredentialId::new(unsafe { credential.credentialID() }.to_vec())?;
    // SAFETY: `rawAttestationObject` may legitimately be nil; the original
    // implementation treated that as an error rather than substituting anything.
    let Some(attestation_object) = (unsafe { credential.rawAttestationObject() }) else {
        return Err(PasskeyError::from_platform_error(
            "registration attestation object is missing",
        ));
    };
    // SAFETY: the credential accessors read immutable ceremony results.
    let client_data_json = unsafe { credential.rawClientDataJSON() };
    Ok(RegistrationResult::new(
        credential_id,
        attestation_object.to_vec(),
        client_data_json.to_vec(),
        None,
        None,
    ))
}

/// `AuthenticateControllerDelegate.didCompleteWithAuthorization`'s typed
/// build: the credential must be a platform public-key assertion.
fn authentication_result(credential: &AnyObject) -> Result<AuthenticationResult, PasskeyError> {
    let Some(credential) =
        credential.downcast_ref::<ASAuthorizationPlatformPublicKeyCredentialAssertion>()
    else {
        return Err(PasskeyError::from_platform_error(
            "unexpected passkey assertion credential type",
        ));
    };
    // SAFETY: the credential accessors read immutable ceremony results.
    let credential_id = crate::CredentialId::new(unsafe { credential.credentialID() }.to_vec())?;
    // SAFETY: the credential accessors read immutable ceremony results.
    let (authenticator_data, client_data_json, signature, user_handle) = unsafe {
        (
            credential.rawAuthenticatorData(),
            credential.rawClientDataJSON(),
            credential.signature(),
            credential.userID(),
        )
    };
    Ok(AuthenticationResult::new(
        credential_id,
        authenticator_data.to_vec(),
        client_data_json.to_vec(),
        signature.to_vec(),
        Some(user_handle.to_vec()),
    ))
}

impl WaterkitPasskeyDelegate {
    /// `alloc.init` on the main thread with the ceremony's oneshot.
    fn new(mtm: MainThreadMarker, state: PasskeyState) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WaterkitPasskeyDelegateIvars {
            state: RefCell::new(Some(state)),
            controller: RefCell::new(None),
            keep_alive: RefCell::new(None),
        });
        // SAFETY: `this` is a freshly allocated `WaterkitPasskeyDelegate` and
        // `NSObject`'s `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }
}

/// `perform(request:delegate:)` — the delegate owns the controller for the
/// duration of the ceremony.
fn perform(
    request: Retained<ASAuthorizationRequest>,
    delegate: &Retained<WaterkitPasskeyDelegate>,
) {
    // SAFETY: `initWithAuthorizationRequests:` requires at least one request;
    // the single-element array satisfies that.
    let controller = unsafe {
        ASAuthorizationController::initWithAuthorizationRequests(
            ASAuthorizationController::alloc(),
            &NSArray::from_retained_slice(&[request]),
        )
    };
    // SAFETY: `setDelegate:`/`setPresentationContextProvider:` are weak
    // properties; the delegate self-retains below so it outlives the call.
    unsafe {
        controller.setDelegate(Some(ProtocolObject::from_ref(&**delegate)));
        controller.setPresentationContextProvider(Some(ProtocolObject::from_ref(&**delegate)));
    }
    delegate
        .ivars()
        .controller
        .borrow_mut()
        .replace(controller.clone());
    delegate
        .ivars()
        .keep_alive
        .borrow_mut()
        .replace(delegate.retain());
    // SAFETY: `performRequests` begins the authorization flow; the delegate
    // wired above receives the result.
    unsafe { controller.performRequests() };
}

/// `passkey_is_available` — the platform credential provider API requires
/// iOS 16/macOS 13.
pub fn passkey_is_available() -> bool {
    has_availability(16, 13)
}

/// `passkey_register`'s typed port: build the platform registration request
/// on the main thread and resolve it through the delegate's oneshot.
async fn start_registration(
    options: RegisterOptions,
    sender: oneshot::Sender<Result<RegistrationResult, PasskeyError>>,
) {
    waterkit_core::apple::on_main(move |mtm| {
        // SAFETY: `initWithRelyingPartyIdentifier:` constructs the request's
        // provider; the identifier is a validated non-empty string.
        let provider = unsafe {
            ASAuthorizationPlatformPublicKeyCredentialProvider::initWithRelyingPartyIdentifier(
                ASAuthorizationPlatformPublicKeyCredentialProvider::alloc(),
                &NSString::from_str(options.rp().id().as_str()),
            )
        };
        let challenge = NSData::from_vec(options.challenge().as_bytes().to_vec());
        let user_id = NSData::from_vec(options.user().id().as_bytes().to_vec());
        // SAFETY: the registration request is built through the platform
        // provider; every argument is a live object.
        let request = unsafe {
            provider.createCredentialRegistrationRequestWithChallenge_name_userID(
                &challenge,
                &NSString::from_str(options.user().name()),
                &user_id,
            )
        };
        // SAFETY: property setters on a live request.
        unsafe {
            request.setDisplayName(Some(&NSString::from_str(options.user().display_name())));
            request.setUserVerificationPreference(user_verification_preference(
                options.user_verification_value(),
            ));
            request.setAttestationPreference(attestation_preference(options.attestation_value()));
        }

        let delegate = WaterkitPasskeyDelegate::new(mtm, PasskeyState::Register(sender));
        // SAFETY: a platform registration request is-an `ASAuthorizationRequest`.
        perform(
            unsafe { Retained::cast_unchecked::<ASAuthorizationRequest>(request) },
            &delegate,
        );
    })
    .await;
}

/// `passkey_authenticate`'s typed port: build the platform assertion request
/// on the main thread and resolve it through the delegate's oneshot.
async fn start_authentication(
    options: AuthenticateOptions,
    sender: oneshot::Sender<Result<AuthenticationResult, PasskeyError>>,
) {
    waterkit_core::apple::on_main(move |mtm| {
        // SAFETY: `initWithRelyingPartyIdentifier:` constructs the request's
        // provider; the identifier is a validated non-empty string.
        let provider = unsafe {
            ASAuthorizationPlatformPublicKeyCredentialProvider::initWithRelyingPartyIdentifier(
                ASAuthorizationPlatformPublicKeyCredentialProvider::alloc(),
                &NSString::from_str(options.rp_id().as_str()),
            )
        };
        let challenge = NSData::from_vec(options.challenge().as_bytes().to_vec());
        // SAFETY: the assertion request is built through the platform
        // provider; every argument is a live object.
        let request = unsafe { provider.createCredentialAssertionRequestWithChallenge(&challenge) };
        // SAFETY: property setters on a live request.
        unsafe {
            request.setUserVerificationPreference(user_verification_preference(
                options.user_verification_value(),
            ));
        }

        if !options.allow_credentials_ref().is_empty() {
            let descriptors: Vec<Retained<ASAuthorizationPlatformPublicKeyCredentialDescriptor>> =
                options
                    .allow_credentials_ref()
                    .iter()
                    .map(|credential| {
                        let id = NSData::from_vec(credential.id().as_bytes().to_vec());
                        // SAFETY: `initWithCredentialID:` on a fresh alloc.
                        unsafe {
                            ASAuthorizationPlatformPublicKeyCredentialDescriptor::initWithCredentialID(
                                ASAuthorizationPlatformPublicKeyCredentialDescriptor::alloc(),
                                &id,
                            )
                        }
                    })
                    .collect();
            if !descriptors.is_empty() {
                // SAFETY: property setter on a live request.
                unsafe {
                    request.setAllowedCredentials(&NSArray::from_retained_slice(&descriptors));
                }
            }
        }

        let delegate = WaterkitPasskeyDelegate::new(mtm, PasskeyState::Authenticate(sender));
        // SAFETY: a platform assertion request is-an `ASAuthorizationRequest`.
        perform(
            unsafe { Retained::cast_unchecked::<ASAuthorizationRequest>(request) },
            &delegate,
        );
    })
    .await;
}

pub struct PlatformBackend;

#[async_trait]
impl PasskeyBackend for PlatformBackend {
    async fn is_available(&self) -> Result<Availability, PasskeyError> {
        if passkey_is_available() {
            Ok(Availability::supported())
        } else {
            Ok(Availability::unavailable())
        }
    }

    async fn register(
        &self,
        options: &RegisterOptions,
    ) -> Result<RegistrationResult, PasskeyError> {
        if !has_availability(16, 13) {
            return Err(PasskeyError::from_platform_error(
                "passkey registration requires iOS 16+/macOS 13+",
            ));
        }
        let (tx, rx) = oneshot::channel();
        start_registration(options.clone(), tx).await;
        rx.await.unwrap_or_else(|_| {
            Err(PasskeyError::Platform(
                "apple passkey register callback channel closed".into(),
            ))
        })
    }

    async fn authenticate(
        &self,
        options: &AuthenticateOptions,
    ) -> Result<AuthenticationResult, PasskeyError> {
        if !has_availability(16, 13) {
            return Err(PasskeyError::from_platform_error(
                "passkey authentication requires iOS 16+/macOS 13+",
            ));
        }
        let (tx, rx) = oneshot::channel();
        start_authentication(options.clone(), tx).await;
        rx.await.unwrap_or_else(|_| {
            Err(PasskeyError::Platform(
                "apple passkey authenticate callback channel closed".into(),
            ))
        })
    }
}
