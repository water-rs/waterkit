//! Apple platform (iOS/macOS) permission implementation backed by the
//! Objective-C frameworks through `objc2`.
//!
//! Status probes are class-side framework queries callable on the calling
//! thread. Requests resolve through a `block2` completion handler that
//! answers a `futures` oneshot — except location, which needs a
//! `CLLocationManager` delegate: a `define_class!` object driven on the
//! main run loop that owns its own strong self-retain (the manager's
//! `delegate` property is weak).

use core::cell::RefCell;
use core::ptr::NonNull;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use futures::channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureDevice, AVMediaType, AVMediaTypeAudio, AVMediaTypeVideo,
};
use objc2_contacts::{CNAuthorizationStatus, CNContactStore, CNEntityType};
use objc2_core_location::{CLAuthorizationStatus, CLLocationManager, CLLocationManagerDelegate};
use objc2_event_kit::{EKAuthorizationStatus, EKEntityType, EKEventStore};
use objc2_foundation::{NSError, NSObjectProtocol, NSOperatingSystemVersion, NSProcessInfo};
use objc2_photos::{PHAuthorizationStatus, PHPhotoLibrary};

use crate::{Permission, PermissionError, PermissionStatus};

/// The subset of [`Permission`] that has an Apple authorization API; kept
/// as one internal enum so [`check`] and [`request`] share the mapping.
#[derive(Clone, Copy, Debug)]
enum ApplePermission {
    Location,
    Camera,
    Microphone,
    Photos,
    Contacts,
    Calendar,
}

/// Maps a [`Permission`] to its Apple counterpart, returning `None` for
/// variants that have no Apple mapping yet (Bluetooth runtime permission
/// is implicit on Apple, NFC has no equivalent gating, etc.).
const fn permission_to_apple(permission: Permission) -> Option<ApplePermission> {
    Some(match permission {
        Permission::Location | Permission::LocationWhenInUse | Permission::LocationAlways => {
            ApplePermission::Location
        }
        Permission::Camera => ApplePermission::Camera,
        Permission::Microphone => ApplePermission::Microphone,
        Permission::Photos => ApplePermission::Photos,
        Permission::Contacts => ApplePermission::Contacts,
        Permission::Calendar => ApplePermission::Calendar,
        // Reminders, Bluetooth*, Nfc, Notification, SpeechRecognition,
        // Tracking, MediaLibrary, BodySensors, HealthRead/Write — not
        // queried on Apple; falls through with `None`. The wildcard also
        // catches any future `Permission` variants added to waterkit-core
        // before they are wired up.
        _ => return None,
    })
}

/// The oneshot a request's completion handler resolves.
type RequestSender = oneshot::Sender<PermissionStatus>;

/// Completion-handler state: a block can be invoked more than once, so the
/// sender sits behind a `RefCell<Option>` — the first call resolves and
/// later calls do nothing.
type Pending = RefCell<Option<RequestSender>>;

/// Answers the request's oneshot once from a completion handler.
fn resolve(pending: &Pending, status: PermissionStatus) {
    if let Some(sender) = pending.borrow_mut().take() {
        // The receiver is gone only when the caller stopped waiting (for
        // example a timeout); the answer then has no reader.
        let _ = sender.send(status);
    }
}

/// The `granted -> PermissionStatus` mapping the Swift realization used for
/// every boolean request callback.
const fn granted_status(granted: Bool) -> PermissionStatus {
    if granted.as_bool() {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    }
}

// MARK: - Status checks

/// `CLAuthorizationStatus` → [`PermissionStatus`]. An unknown future status
/// maps to `NotDetermined`, matching the Swift switch's `@unknown default`.
fn status_from_cl_authorization_status(status: CLAuthorizationStatus) -> PermissionStatus {
    if status == CLAuthorizationStatus::Restricted {
        PermissionStatus::Restricted
    } else if status == CLAuthorizationStatus::Denied {
        PermissionStatus::Denied
    } else if status == CLAuthorizationStatus::AuthorizedAlways
        || status == CLAuthorizationStatus::AuthorizedWhenInUse
    {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotDetermined
    }
}

/// `AVAuthorizationStatus` → [`PermissionStatus`].
fn status_from_av_authorization_status(status: AVAuthorizationStatus) -> PermissionStatus {
    if status == AVAuthorizationStatus::Restricted {
        PermissionStatus::Restricted
    } else if status == AVAuthorizationStatus::Denied {
        PermissionStatus::Denied
    } else if status == AVAuthorizationStatus::Authorized {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotDetermined
    }
}

/// `PHAuthorizationStatus` → [`PermissionStatus`]; `Limited` reads as
/// `Granted` exactly like the Swift realization.
fn status_from_ph_authorization_status(status: PHAuthorizationStatus) -> PermissionStatus {
    if status == PHAuthorizationStatus::Restricted {
        PermissionStatus::Restricted
    } else if status == PHAuthorizationStatus::Denied {
        PermissionStatus::Denied
    } else if status == PHAuthorizationStatus::Authorized
        || status == PHAuthorizationStatus::Limited
    {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotDetermined
    }
}

/// `CNAuthorizationStatus` → [`PermissionStatus`].
fn status_from_cn_authorization_status(status: CNAuthorizationStatus) -> PermissionStatus {
    if status == CNAuthorizationStatus::Restricted {
        PermissionStatus::Restricted
    } else if status == CNAuthorizationStatus::Denied {
        PermissionStatus::Denied
    } else if status == CNAuthorizationStatus::Authorized {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotDetermined
    }
}

/// `EKAuthorizationStatus` → [`PermissionStatus`]; `FullAccess` and
/// `WriteOnly` both read as `Granted` exactly like the Swift realization.
fn status_from_ek_authorization_status(status: EKAuthorizationStatus) -> PermissionStatus {
    if status == EKAuthorizationStatus::Restricted {
        PermissionStatus::Restricted
    } else if status == EKAuthorizationStatus::Denied {
        PermissionStatus::Denied
    } else if status == EKAuthorizationStatus::FullAccess
        || status == EKAuthorizationStatus::WriteOnly
    {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotDetermined
    }
}

/// `CLLocationManager.authorizationStatus()` — the class-side accessor the
/// Swift realization called.
#[expect(
    deprecated,
    reason = "the port keeps the exact selectors the Swift realization called"
)]
fn check_location_permission() -> PermissionStatus {
    // SAFETY: class-side status query, callable on any thread.
    let status = unsafe { CLLocationManager::authorizationStatus_class() };
    status_from_cl_authorization_status(status)
}

/// `AVMediaTypeVideo` — the extern string constant `AVFoundation` exports.
fn av_media_type_video() -> &'static AVMediaType {
    // SAFETY: `AVMediaTypeVideo` is an immutable extern string constant
    // that ships with AVFoundation; read-only access.
    unsafe { AVMediaTypeVideo }.expect("AVMediaTypeVideo ships with AVFoundation")
}

/// `AVMediaTypeAudio` — the extern string constant `AVFoundation` exports.
fn av_media_type_audio() -> &'static AVMediaType {
    // SAFETY: `AVMediaTypeAudio` is an immutable extern string constant
    // that ships with AVFoundation; read-only access.
    unsafe { AVMediaTypeAudio }.expect("AVMediaTypeAudio ships with AVFoundation")
}

/// `AVCaptureDevice.authorizationStatus(for:)` for `media_type`.
fn check_av_permission(media_type: &'static AVMediaType) -> PermissionStatus {
    // SAFETY: class-side status query, callable on any thread; `media_type`
    // is always one of AVMediaTypeVideo/AVMediaTypeAudio.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    status_from_av_authorization_status(status)
}

/// `PHPhotoLibrary.authorizationStatus()` — the class-side accessor the
/// Swift realization called.
#[expect(
    deprecated,
    reason = "the port keeps the exact selectors the Swift realization called"
)]
fn check_photos_permission() -> PermissionStatus {
    // SAFETY: class-side status query, callable on any thread.
    let status = unsafe { PHPhotoLibrary::authorizationStatus() };
    status_from_ph_authorization_status(status)
}

/// `CNContactStore.authorizationStatus(for: .contacts)`.
fn check_contacts_permission() -> PermissionStatus {
    // SAFETY: class-side status query, callable on any thread.
    let status =
        unsafe { CNContactStore::authorizationStatusForEntityType(CNEntityType::Contacts) };
    status_from_cn_authorization_status(status)
}

/// `EKEventStore.authorizationStatus(for: .event)`.
fn check_calendar_permission() -> PermissionStatus {
    // SAFETY: class-side status query, callable on any thread.
    let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
    status_from_ek_authorization_status(status)
}

// MARK: - Requests

/// `AVCaptureDevice.requestAccess(for:)` for `media_type`; the handler runs
/// on an arbitrary queue chosen by `AVFoundation`.
fn request_av_permission(media_type: &'static AVMediaType, sender: RequestSender) {
    let pending = RefCell::new(Some(sender));
    let block = RcBlock::new(move |granted: Bool| {
        resolve(&pending, granted_status(granted));
    });
    // SAFETY: `media_type` is always AVMediaTypeVideo or AVMediaTypeAudio
    // and the framework retains the completion block until it runs.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &block) };
}

/// `PHPhotoLibrary.requestAuthorization`; the Swift realization answered
/// with a fresh `authorizationStatus` query rather than the delivered
/// status, and the port does the same.
#[expect(
    deprecated,
    reason = "the port keeps the exact selectors the Swift realization called"
)]
fn request_photos_permission(sender: RequestSender) {
    let pending = RefCell::new(Some(sender));
    let block = RcBlock::new(move |_status: PHAuthorizationStatus| {
        let status = check_photos_permission();
        resolve(&pending, status);
    });
    // SAFETY: the framework retains the completion block until it runs.
    unsafe { PHPhotoLibrary::requestAuthorization(&block) };
}

/// `CNContactStore.requestAccess(for: .contacts)`.
fn request_contacts_permission(sender: RequestSender) {
    // SAFETY: `CNContactStore` is `[[CNContactStore alloc] init]`.
    let store = unsafe { CNContactStore::new() };
    let pending = RefCell::new(Some(sender));
    // The block owns a clone of the store so the request's target lives
    // until the framework answers.
    let block = {
        let store = store.clone();
        RcBlock::new(move |granted: Bool, _error: *mut NSError| {
            let _store = &store;
            resolve(&pending, granted_status(granted));
        })
    };
    // SAFETY: the completion block is retained by the framework until the
    // request resolves; its NSError argument is never dereferenced.
    unsafe {
        store.requestAccessForEntityType_completionHandler(CNEntityType::Contacts, &block);
    }
}

/// The `#available(macOS 14.0, iOS 17.0, *)` gate the Swift realization put
/// on `requestFullAccessToEventsWithCompletion:`. `NSProcessInfo` reports
/// the macOS version on Mac Catalyst, which is the same boundary (macOS 14
/// pairs with iOS 17).
fn has_full_access_events() -> bool {
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: if cfg!(all(target_os = "ios", not(target_abi = "macabi"))) {
            17
        } else {
            14
        },
        minorVersion: 0,
        patchVersion: 0,
    })
}

/// `EKEventStore.requestFullAccessToEvents` (iOS 17 / macOS 14 and later)
/// or the older `requestAccess(to: .event)` below that boundary.
fn request_calendar_permission(sender: RequestSender) {
    let pending = RefCell::new(Some(sender));
    // SAFETY: `EKEventStore` is `[[EKEventStore alloc] init]`. The
    // completion block is retained by the framework until the request
    // resolves; its NSError argument is never dereferenced, and the block
    // owns a clone of the store so the request's target lives until then.
    let store = unsafe { EKEventStore::new() };
    let block = {
        let store = store.clone();
        RcBlock::new(move |granted: Bool, _error: *mut NSError| {
            let _store = &store;
            resolve(&pending, granted_status(granted));
        })
    };
    let completion = NonNull::from(&*block).as_ptr();
    if has_full_access_events() {
        // SAFETY: `completion` is a valid block pointer; the framework
        // copies and retains it until the request resolves.
        unsafe { store.requestFullAccessToEventsWithCompletion(completion) };
    } else {
        // SAFETY: same contract as above, on the pre-iOS 17 API.
        #[expect(
            deprecated,
            reason = "the port keeps the pre-iOS 17 selector the Swift realization called on those OS versions"
        )]
        unsafe {
            store.requestAccessToEntityType_completion(EKEntityType::Event, completion);
        }
    }
}

// MARK: - Location request

/// Request state consumed by `finish`, which resolves at most once.
#[derive(Debug)]
struct LocationRequestState {
    /// Resolves the `request` future; `None` once the request resolved.
    sender: Option<RequestSender>,
    /// Strong self-retain: `CLLocationManager.delegate` is a weak property,
    /// so the request owns itself for its lifetime.
    keep_alive: Option<Retained<LocationPermissionRequest>>,
}

/// The `ObjC` ivars of [`LocationPermissionRequest`]: the manager driving
/// the authorization prompt plus its resolution state.
#[derive(Debug)]
struct LocationPermissionRequestIvars {
    manager: Retained<CLLocationManager>,
    state: RefCell<LocationRequestState>,
}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `LocationPermissionRequest` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WaterkitLocationPermissionRequest"]
    #[ivars = LocationPermissionRequestIvars]
    #[derive(Debug)]
    struct LocationPermissionRequest;

    unsafe impl NSObjectProtocol for LocationPermissionRequest {}

    unsafe impl CLLocationManagerDelegate for LocationPermissionRequest {
        /// The authorization answer arrives here once the user decides; a
        /// still-`NotDetermined` status means the request is in flight.
        #[unsafe(method(locationManagerDidChangeAuthorization:))]
        fn did_change_authorization(&self, manager: &CLLocationManager) {
            // SAFETY: read-only accessor on the main thread.
            let status = unsafe { manager.authorizationStatus() };
            if status != CLAuthorizationStatus::NotDetermined {
                self.finish(status_from_cl_authorization_status(status));
            }
        }
    }
);

impl LocationPermissionRequest {
    fn new(mtm: MainThreadMarker, sender: RequestSender) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(LocationPermissionRequestIvars {
            // SAFETY: `new` is `[[CLLocationManager alloc] init]`; the
            // manager is created on the main thread and is only ever used
            // there.
            manager: unsafe { CLLocationManager::new() },
            state: RefCell::new(LocationRequestState {
                sender: Some(sender),
                keep_alive: None,
            }),
        });
        // SAFETY: `this` is a freshly allocated `LocationPermissionRequest`
        // and `NSObject`'s `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }

    /// Kicks off the request; called on the main thread.
    fn start(&self) {
        self.ivars().state.borrow_mut().keep_alive = Some(self.retain());
        let manager = &self.ivars().manager;
        // SAFETY: `delegate` is a weak property; the request stays alive
        // through `keep_alive` so it cannot dangle.
        // `requestWhenInUseAuthorization` drives the system prompt on the
        // main run loop this request is pinned to.
        unsafe {
            manager.setDelegate(Some(ProtocolObject::from_ref(self)));
            manager.requestWhenInUseAuthorization();
        }
    }

    /// Resolves the request exactly once: the first call sends the status
    /// and releases the self-retain; later delegate calls return early.
    fn finish(&self, status: PermissionStatus) {
        let (sender, keep_alive) = {
            let mut state = self.ivars().state.borrow_mut();
            let Some(sender) = state.sender.take() else {
                return;
            };
            (sender, state.keep_alive.take())
        };
        // SAFETY: `delegate` is a weak property; nil-ing it on the main
        // thread keeps later callbacks from reaching a resolved request.
        unsafe { self.ivars().manager.setDelegate(None) };
        let _ = sender.send(status);
        drop(keep_alive);
    }
}

/// Checks the status of a permission on Apple platforms.
///
/// Returns [`PermissionStatus::NotDetermined`] for permissions that have
/// no Apple mapping yet — callers should pair this with [`request`] which
/// returns a typed error.
///
pub async fn check(permission: Permission) -> PermissionStatus {
    let Some(permission) = permission_to_apple(permission) else {
        return PermissionStatus::NotDetermined;
    };
    match permission {
        ApplePermission::Location => check_location_permission(),
        ApplePermission::Camera => check_av_permission(av_media_type_video()),
        ApplePermission::Microphone => check_av_permission(av_media_type_audio()),
        ApplePermission::Photos => check_photos_permission(),
        ApplePermission::Contacts => check_contacts_permission(),
        ApplePermission::Calendar => check_calendar_permission(),
    }
}

/// Requests a permission on Apple platforms.
///
/// The system request runs on its own completion handler; this future only
/// awaits the answer, so the calling thread is never blocked while a prompt
/// is waiting for the user.
///
/// # Errors
///
/// Returns [`PermissionError::Unsupported`] for permissions that have no
/// Apple mapping, and [`PermissionError::Platform`] when the system drops
/// the completion handler without answering.
pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    let apple_permission = permission_to_apple(permission).ok_or(PermissionError::Unsupported)?;
    let (sender, receiver) = oneshot::channel();
    match apple_permission {
        // `CLLocationManager` delivers authorization changes to a delegate
        // on the run loop it was created on — the main one.
        ApplePermission::Location => DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("the main queue only runs on the main thread");
            LocationPermissionRequest::new(mtm, sender).start();
        }),
        ApplePermission::Camera => request_av_permission(av_media_type_video(), sender),
        ApplePermission::Microphone => request_av_permission(av_media_type_audio(), sender),
        ApplePermission::Photos => request_photos_permission(sender),
        ApplePermission::Contacts => request_contacts_permission(sender),
        ApplePermission::Calendar => request_calendar_permission(sender),
    }
    receiver.await.map_err(|_| {
        PermissionError::Platform(format!("{permission:?} permission callback dropped"))
    })
}
