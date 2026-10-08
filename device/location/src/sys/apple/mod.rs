//! Apple platform (iOS/macOS) location implementation backed by Core Location.
//!
//! The whole request runs on the main run loop: the `CLLocationManager` and
//! its delegate are created, configured and driven on the main queue, and a
//! ten-second timer bounds
//! the wait. The delegate is a small `define_class!` object whose ivars own
//! the manager, the oneshot sender that resolves the future, and a strong
//! self-retain that keeps the request alive until `finish` — the manager's
//! `delegate` property is weak.

use core::cell::RefCell;
use core::time::Duration;

use dispatch2::{DispatchQoS, DispatchQueue, DispatchTime, GlobalQueueIdentifier, MainThreadBound};
use futures::channel::oneshot;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_core_location::{
    CLAuthorizationStatus, CLError, CLLocation, CLLocationManager, CLLocationManagerDelegate,
    kCLErrorDomain, kCLLocationAccuracyBest,
};
use objc2_foundation::{NSArray, NSError, NSObjectProtocol};

use crate::{Location, LocationCapabilities, LocationError, LocationProvider, Timestamp};

/// Bounds a single `requestLocation` round trip.
const LOCATION_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Request state consumed by `finish`, which resolves at most once.
#[derive(Debug)]
struct RequestState {
    /// Resolves the `get_location` future; `None` once the request resolved.
    sender: Option<oneshot::Sender<Result<Location, LocationError>>>,
    /// Strong self-retain: `CLLocationManager.delegate` is a weak property,
    /// so the request owns itself for its lifetime.
    keep_alive: Option<Retained<LocationRequest>>,
}

/// The `ObjC` ivars of [`LocationRequest`]: the manager running the request
/// plus its resolution state.
#[derive(Debug)]
struct LocationRequestIvars {
    manager: Retained<CLLocationManager>,
    state: RefCell<RequestState>,
}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `LocationRequest` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WaterkitLocationRequest"]
    #[ivars = LocationRequestIvars]
    #[derive(Debug)]
    struct LocationRequest;

    unsafe impl NSObjectProtocol for LocationRequest {}

    unsafe impl CLLocationManagerDelegate for LocationRequest {
        #[unsafe(method(locationManager:didUpdateLocations:))]
        #[expect(
            clippy::cast_possible_truncation,
            reason = "milliseconds since 1970 fit an i64 for any CLLocation timestamp; the sub-millisecond fraction is dropped on purpose"
        )]
        fn did_update_locations(
            &self,
            _manager: &CLLocationManager,
            locations: &NSArray<CLLocation>,
        ) {
            let Some(location) = locations.lastObject() else {
                self.finish(Err(LocationError::NotAvailable));
                return;
            };
            // SAFETY: read-only accessors on a live CLLocation delivered by
            // the framework on the main thread.
            let (coordinate, altitude, horizontal_accuracy, vertical_accuracy, timestamp_ms) = unsafe {
                (
                    location.coordinate(),
                    location.altitude(),
                    location.horizontalAccuracy(),
                    location.verticalAccuracy(),
                    (location.timestamp().timeIntervalSince1970() * 1000.0) as i64,
                )
            };
            let timestamp = match Timestamp::from_millisecond(timestamp_ms) {
                Ok(timestamp) => timestamp,
                Err(error) => {
                    self.finish(Err(LocationError::Platform(error.to_string())));
                    return;
                }
            };
            let result =
                Location::from_degrees(coordinate.latitude, coordinate.longitude, timestamp).map(
                    |mut location| {
                        // CoreLocation reports altitude validity through
                        // verticalAccuracy: a negative value means the altitude is
                        // invalid. The altitude itself is a plain double (0.0 is a
                        // legal reading), never NaN.
                        if vertical_accuracy >= 0.0 {
                            location = location
                                .with_altitude(altitude)
                                .with_vertical_accuracy(vertical_accuracy);
                        }
                        if horizontal_accuracy >= 0.0 {
                            location = location.with_horizontal_accuracy(horizontal_accuracy);
                        }
                        location
                    },
                );
            self.finish(result);
        }

        #[unsafe(method(locationManager:didFailWithError:))]
        fn did_fail_with_error(&self, manager: &CLLocationManager, error: &NSError) {
            // SAFETY: `kCLErrorDomain` is a constant string emitted by
            // CoreLocation; read-only access.
            if !error.domain().isEqualToString(unsafe { kCLErrorDomain }) {
                self.finish(Err(LocationError::NotAvailable));
                return;
            }
            let code = CLError(error.code());
            if code == CLError::Denied {
                // SAFETY: read-only accessor on the main thread, like every
                // other manager use.
                let status = unsafe { manager.authorizationStatus() };
                tracing::warn!(
                    path = "didFailWithError",
                    status = i64::from(status.0),
                    "location permission denied"
                );
                self.finish(Err(LocationError::PermissionDenied));
            } else if code != CLError::LocationUnknown {
                self.finish(Err(LocationError::NotAvailable));
            }
        }

        #[unsafe(method(locationManagerDidChangeAuthorization:))]
        fn did_change_authorization(&self, manager: &CLLocationManager) {
            // SAFETY: read-only accessor on the main thread.
            let status = unsafe { manager.authorizationStatus() };
            if status == CLAuthorizationStatus::Denied
                || status == CLAuthorizationStatus::Restricted
            {
                tracing::warn!(
                    path = "didChangeAuthorization",
                    status = i64::from(status.0),
                    "location permission denied"
                );
                self.finish(Err(LocationError::PermissionDenied));
            }
        }
    }
);

impl LocationRequest {
    fn new(
        mtm: MainThreadMarker,
        sender: oneshot::Sender<Result<Location, LocationError>>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(LocationRequestIvars {
            // SAFETY: `new` is `[[CLLocationManager alloc] init]`; the manager
            // is created on the main thread and is only ever used there.
            manager: unsafe { CLLocationManager::new() },
            state: RefCell::new(RequestState {
                sender: Some(sender),
                keep_alive: None,
            }),
        });
        // SAFETY: `this` is a freshly allocated `LocationRequest` and
        // `NSObject`'s `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }

    /// Kicks off the request; called on the main thread.
    fn start(&self, mtm: MainThreadMarker) {
        self.ivars().state.borrow_mut().keep_alive = Some(self.retain());
        // The hop holds the request weakly: `keep_alive` is the request's
        // only owner.
        let request = MainThreadBound::new(Weak::new(self), mtm);
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(
            DispatchQoS::UserInitiated,
        ))
        .exec_async(move || {
            // SAFETY: `+locationServicesEnabled` is a stateless class query;
            // it is deliberately queried off the main thread.
            let services_enabled = unsafe { CLLocationManager::locationServicesEnabled_class() };
            DispatchQueue::main().exec_async(move || {
                let mtm =
                    MainThreadMarker::new().expect("the main queue only runs on the main thread");
                if let Some(request) = request.get(mtm).load() {
                    request.start_on_main(mtm, services_enabled);
                }
            });
        });
    }

    fn start_on_main(&self, mtm: MainThreadMarker, services_enabled: bool) {
        if !services_enabled {
            self.finish(Err(LocationError::ServiceDisabled));
            return;
        }
        let manager = &self.ivars().manager;
        // SAFETY: `authorizationStatus` is a read-only accessor called on the
        // main thread, like every other manager use.
        let status = unsafe { manager.authorizationStatus() };
        if status == CLAuthorizationStatus::Denied || status == CLAuthorizationStatus::Restricted {
            tracing::warn!(
                path = "start_on_main",
                status = i64::from(status.0),
                "location permission denied"
            );
            self.finish(Err(LocationError::PermissionDenied));
            return;
        }
        // SAFETY: `delegate` is a weak property; the request stays alive
        // through `keep_alive` and the timeout capture, so it cannot dangle.
        // `kCLLocationAccuracyBest` is a constant double read once.
        unsafe {
            manager.setDelegate(Some(ProtocolObject::from_ref(self)));
            manager.setDesiredAccuracy(kCLLocationAccuracyBest);
        }
        // The timeout holds the request weakly, so a resolved request is
        // released at once instead of when the timer fires; a timer firing
        // after resolution finds nothing to finish.
        let request = MainThreadBound::new(Weak::new(self), mtm);
        DispatchQueue::main()
            .after(
                DispatchTime::try_from(LOCATION_REQUEST_TIMEOUT)
                    .expect("ten seconds encodes as a dispatch_time delta"),
                move || {
                    let mtm = MainThreadMarker::new()
                        .expect("the main queue only runs on the main thread");
                    if let Some(request) = request.get(mtm).load() {
                        request.finish(Err(LocationError::Timeout));
                    }
                },
            )
            .expect("the main queue accepts delayed work");
        // SAFETY: `requestLocation` arms the one-shot delivery on the main
        // run loop this request is pinned to.
        unsafe { manager.requestLocation() };
    }

    /// Resolves the request exactly once: the first call sends the result and
    /// releases the self-retain; later calls (e.g. the uncancellable timeout
    /// firing after success) return early.
    fn finish(&self, result: Result<Location, LocationError>) {
        let (sender, keep_alive) = {
            let mut state = self.ivars().state.borrow_mut();
            let Some(sender) = state.sender.take() else {
                return;
            };
            (sender, state.keep_alive.take())
        };
        // SAFETY: `delegate` is a weak property; nil-ing it on the main thread
        // keeps later callbacks from reaching a resolved request.
        unsafe { self.ivars().manager.setDelegate(None) };
        let _ = sender.send(result);
        drop(keep_alive);
    }
}

/// Get the current location on Apple platforms.
///
/// # Errors
/// Returns a `LocationError` if the location cannot be retrieved.
pub async fn get_location() -> Result<Location, LocationError> {
    let (sender, receiver) = oneshot::channel();
    // `CLLocationManager` and its delegate live on the main run loop.
    DispatchQueue::main().exec_async(move || {
        let mtm = MainThreadMarker::new().expect("the main queue only runs on the main thread");
        LocationRequest::new(mtm, sender).start(mtm);
    });
    receiver
        .await
        .map_err(|_| LocationError::Platform("location callback dropped".into()))?
}

/// Core Location ships with every iOS and macOS release.
pub async fn capabilities() -> LocationCapabilities {
    LocationCapabilities {
        provider: Some(LocationProvider::CoreLocation),
    }
}
