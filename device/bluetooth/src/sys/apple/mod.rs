//! Apple platform implementation of Bluetooth (BLE and Classic SPP).
//!
//! BLE runs through `CoreBluetooth` (`CBCentralManager`); Classic Bluetooth
//! runs through `IOBluetooth` and exists on macOS only — on iOS the Classic
//! entry points fail fast, matching the previous implementation.
//!
//! All Objective-C work happens on the main queue: the managers, delegates
//! and their callback state are main-thread-bound `define_class!` objects
//! whose ivars own the pending senders (no global/static callback registry —
//! each delegate instance owns its state). Public entry points hop onto the
//! main queue and shuttle results back through channels; the raw object
//! addresses crossing queue hops are `usize` values reconstructed into
//! `Retained`/references on the main thread.

use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::sync::Mutex;

use async_channel::{Receiver, Sender};
use core::cell::RefCell;
use core::ffi::c_void;
use dispatch2::DispatchQueue;
use futures::channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_bluetooth::{
    CBAdvertisementDataServiceUUIDsKey, CBCentralManager, CBCentralManagerDelegate,
    CBCentralManagerScanOptionAllowDuplicatesKey, CBCharacteristic, CBCharacteristicProperties,
    CBCharacteristicWriteType, CBManagerState, CBPeripheral, CBPeripheralDelegate,
    CBPeripheralState, CBService, CBUUID,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSError, NSNumber, NSObjectProtocol, NSString, NSUUID,
};

use crate::{
    AdapterState, BluetoothDevice, BluetoothError, CharacteristicProperties, ClassicDevice,
    DeviceId, GattCharacteristic, GattService, ScanFilter, ScanResult, Uuid,
};

/// `kern_return_t` success code (`kIOReturnSuccess`); `objc2-io-bluetooth`
/// keeps `IOReturn` crate-private, so the code is declared here.
#[cfg(target_os = "macos")]
const K_IO_RETURN_SUCCESS: core::ffi::c_int = 0;

type ReadTx = oneshot::Sender<Result<Vec<u8>, BluetoothError>>;
type WriteUnitTx = oneshot::Sender<Result<(), BluetoothError>>;
#[cfg(target_os = "macos")]
type WriteTx = oneshot::Sender<Result<usize, BluetoothError>>;
#[cfg(target_os = "macos")]
type PendingRead = (usize, oneshot::Sender<Result<Vec<u8>, BluetoothError>>);
#[cfg(target_os = "macos")]
type PendingWrite = (
    oneshot::Sender<Result<usize, BluetoothError>>,
    objc2::rc::Retained<NSData>,
);

/// Run `work` on the main queue, blocking the caller until it finishes.
fn on_main<R: Send>(work: impl FnOnce() -> R + Send) -> R {
    if MainThreadMarker::new().is_some() {
        return work();
    }
    let mut slot = Some(work);
    let mut result: Option<R> = None;
    let result_ref = &mut result;
    let slot_ref = &mut slot;
    DispatchQueue::main().exec_sync(move || {
        *result_ref = Some(slot_ref.take().expect("main-queue hop runs exactly once")());
    });
    result.expect("dispatch_sync on the main queue always runs")
}

/// Submit `work` for execution on the main queue without blocking.
fn dispatch_main(work: impl FnOnce() + Send + 'static) {
    if MainThreadMarker::new().is_some() {
        work();
    } else {
        DispatchQueue::main().exec_async(work);
    }
}

/// Cast an object reference to `&AnyObject` for the informal-delegate
/// parameters in `IOBluetooth` (`setDelegate:`, `performSDPQuery:` target).
const fn as_any_object<T: objc2::Message>(obj: &T) -> &AnyObject {
    // SAFETY: every Objective-C object has the same layout — an `isa`
    // pointer — so any `Message` reference is a valid `AnyObject` reference.
    unsafe { &*core::ptr::from_ref(obj).cast::<AnyObject>() }
}

/// Rebuild a `&T` reference from an address previously produced by
/// `Retained::into_raw`, on the main thread where the object is owned.
///
/// # Safety
/// `ptr` must be a live address previously leaked with `Retained::into_raw`
/// and this must run on the main thread.
const unsafe fn from_addr<'a, T>(ptr: usize) -> &'a T {
    // SAFETY: guaranteed by the caller — see the doc comment above.
    unsafe { &*(ptr as *const T) }
}

fn ns_error(error: Option<&NSError>, fallback: &str) -> String {
    error.map_or_else(
        || fallback.to_string(),
        |error| error.localizedDescription().to_string(),
    )
}

fn map_state(state: CBManagerState) -> AdapterState {
    if state == CBManagerState::PoweredOn {
        AdapterState::PoweredOn
    } else if state == CBManagerState::PoweredOff {
        AdapterState::PoweredOff
    } else if state == CBManagerState::Unauthorized {
        AdapterState::Unauthorized
    } else if state == CBManagerState::Unsupported {
        AdapterState::Unavailable
    } else {
        AdapterState::Unknown
    }
}

fn characteristic_key(characteristic: &CBCharacteristic) -> String {
    // SAFETY: `uuid` is a read-only accessor on a live characteristic.
    unsafe { characteristic.UUID().UUIDString() }.to_string()
}

fn gatt_characteristic(characteristic: &CBCharacteristic) -> GattCharacteristic {
    // SAFETY: `properties` is a read-only accessor on a live characteristic.
    let props = unsafe { characteristic.properties() };
    GattCharacteristic {
        uuid: Uuid::new(characteristic_key(characteristic)),
        properties: CharacteristicProperties {
            read: props.contains(CBCharacteristicProperties::Read),
            write: props.contains(CBCharacteristicProperties::Write),
            write_without_response: props
                .contains(CBCharacteristicProperties::WriteWithoutResponse),
            notify: props.contains(CBCharacteristicProperties::Notify),
            indicate: props.contains(CBCharacteristicProperties::Indicate),
        },
    }
}

fn gatt_service(service: &CBService) -> GattService {
    // SAFETY: read-only accessors on a live CBService delivered by the
    // framework delegate callback on the main thread.
    let (uuid, is_primary, characteristics) = unsafe {
        (
            service.UUID().UUIDString().to_string(),
            service.isPrimary(),
            service.characteristics(),
        )
    };
    let characteristics = characteristics.map_or_else(Vec::new, |chars| {
        chars.iter().map(|c| gatt_characteristic(&c)).collect()
    });
    GattService {
        uuid: Uuid::new(uuid),
        is_primary,
        characteristics,
    }
}

fn characteristic_value(characteristic: &CBCharacteristic) -> Vec<u8> {
    // SAFETY: `value` is a read-only accessor; the NSData is copied out
    // immediately.
    unsafe { characteristic.value() }.map_or_else(Vec::new, |data| {
        // SAFETY: `bytes`/`length` describe a live contiguous buffer.
        unsafe { data.as_bytes_unchecked().to_vec() }
    })
}

fn find_characteristic(
    peripheral: &CBPeripheral,
    service_uuid: &str,
    characteristic_uuid: &str,
) -> Option<Retained<CBCharacteristic>> {
    // SAFETY: read-only accessor.
    let services = unsafe { peripheral.services() }?;
    for service in &services {
        // SAFETY: read-only accessors.
        let uuid = unsafe { service.UUID().UUIDString() }.to_string();
        if !uuid.eq_ignore_ascii_case(service_uuid) {
            continue;
        }
        // SAFETY: read-only accessor.
        let Some(characteristics) = (unsafe { service.characteristics() }) else {
            continue;
        };
        for characteristic in &characteristics {
            if characteristic_key(&characteristic).eq_ignore_ascii_case(characteristic_uuid) {
                return Some(characteristic);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// CoreBluetooth central delegate
// ---------------------------------------------------------------------------

/// State carried by the `CBCentralManager` delegate. Each session (a one-shot
/// adapter-state query, a `BleScanner`, a `BleConnection`) owns its own
/// delegate, which in turn owns the manager — `CBCentralManager.delegate` is
/// a weak property.
#[derive(Debug)]
struct CentralDelegateIvars {
    manager: RefCell<Option<Retained<CBCentralManager>>>,
    /// One-shot senders waiting for the first non-unknown adapter state.
    state_txs: RefCell<Vec<oneshot::Sender<AdapterState>>>,
    /// Scan-result stream (the `BleScanner` case).
    scan_tx: RefCell<Option<Sender<ScanResult>>>,
    /// Connect completions keyed by peripheral UUID string.
    connect_txs: RefCell<HashMap<String, oneshot::Sender<Result<(), BluetoothError>>>>,
    /// Connected peripheral + its delegate (the `BleConnection` case).
    peripheral: RefCell<Option<Retained<CBPeripheral>>>,
    peripheral_delegate: RefCell<Option<Retained<PeripheralDelegate>>>,
    /// Self-retain keeping a bare `adapter_state` session alive until the
    /// state resolves.
    keep_alive: RefCell<Option<Retained<CentralDelegate>>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `CentralDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WaterkitBleCentralDelegate"]
    #[ivars = CentralDelegateIvars]
    #[derive(Debug)]
    struct CentralDelegate;

    unsafe impl NSObjectProtocol for CentralDelegate {}

    unsafe impl CBCentralManagerDelegate for CentralDelegate {
        #[unsafe(method(centralManagerDidUpdateState:))]
        fn did_update_state(&self, central: &CBCentralManager) {
            // SAFETY: `state` is a read-only accessor on the live manager.
            let state = map_state(unsafe { central.state() });
            if state == AdapterState::Unknown {
                return;
            }
            for tx in self.ivars().state_txs.borrow_mut().drain(..) {
                let _ = tx.send(state);
            }
            if self.ivars().state_txs.borrow().is_empty() {
                // A bare adapter-state session is done once resolved;
                // dropping the self-retain tears it down.
                self.ivars().keep_alive.borrow_mut().take();
            }
        }

        #[unsafe(method(centralManager:didDiscoverPeripheral:advertisementData:RSSI:))]
        fn did_discover(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            advertisement_data: &NSDictionary<NSString, AnyObject>,
            rssi: &NSNumber,
        ) {
            let scan_tx = self.ivars().scan_tx.borrow();
            let Some(tx) = scan_tx.as_ref() else {
                return;
            };
            // SAFETY: read-only accessors on live objects delivered by the
            // framework on the main thread.
            let (identifier, name, connected) = unsafe {
                (
                    peripheral.identifier().UUIDString().to_string(),
                    peripheral.name().map(|name| name.to_string()),
                    peripheral.state() == CBPeripheralState::Connected,
                )
            };
            let service_uuids = advertisement_data
                .objectForKey(unsafe { CBAdvertisementDataServiceUUIDsKey })
                .map_or_else(Vec::new, |object| {
                    // SAFETY: `CBAdvertisementDataServiceUUIDsKey` always
                    // maps to an `NSArray<CBUUID>` in advertisement data.
                    let uuids = unsafe { &*Retained::as_ptr(&object).cast::<NSArray<CBUUID>>() };
                    uuids
                        .iter()
                        .map(|uuid| Uuid::new(unsafe { uuid.UUIDString() }.to_string()))
                        .collect()
                });
            let device = BluetoothDevice {
                id: DeviceId::new(identifier),
                name,
                rssi: Some(rssi.shortValue()),
                is_connected: connected,
            };
            let _ = tx.try_send(ScanResult {
                device,
                service_uuids,
                manufacturer_data: HashMap::new(),
            });
        }

        #[unsafe(method(centralManager:didConnectPeripheral:))]
        fn did_connect(&self, _central: &CBCentralManager, peripheral: &CBPeripheral) {
            // SAFETY: read-only accessor used as the callback key.
            let key = unsafe { peripheral.identifier() }.UUIDString().to_string();
            if let Some(tx) = self.ivars().connect_txs.borrow_mut().remove(&key) {
                let _ = tx.send(Ok(()));
            }
        }

        #[unsafe(method(centralManager:didFailToConnectPeripheral:error:))]
        fn did_fail_connect(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            // SAFETY: read-only accessor used as the callback key.
            let key = unsafe { peripheral.identifier() }.UUIDString().to_string();
            if let Some(tx) = self.ivars().connect_txs.borrow_mut().remove(&key) {
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(ns_error(
                    error,
                    "failed to connect",
                ))));
            }
        }
    }
);

impl CentralDelegate {
    /// Create a delegate + `CBCentralManager` pair. Must run on the main
    /// thread (the manager dispatches delegate callbacks on the main queue).
    fn spawn() -> Retained<Self> {
        let mtm = MainThreadMarker::new().expect("on main queue");
        let this = Self::alloc(mtm).set_ivars(CentralDelegateIvars {
            manager: RefCell::new(None),
            state_txs: RefCell::new(Vec::new()),
            connect_txs: RefCell::new(HashMap::new()),
            scan_tx: RefCell::new(None),
            peripheral: RefCell::new(None),
            peripheral_delegate: RefCell::new(None),
            keep_alive: RefCell::new(None),
        });
        let delegate: Retained<Self> = unsafe { msg_send![super(this), init] };
        // SAFETY: `initWithDelegate:queue:` is the documented CoreBluetooth
        // initializer; the delegate conforms to `CBCentralManagerDelegate`
        // and the ivar keeps the manager alive.
        let manager = unsafe {
            CBCentralManager::initWithDelegate_queue(
                msg_send![CBCentralManager::class(), alloc],
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(DispatchQueue::main()),
            )
        };
        *delegate.ivars().manager.borrow_mut() = Some(manager);
        delegate
    }

    /// Queue `tx` for the next non-unknown adapter state (or resolve it
    /// immediately when the manager already knows its state).
    fn watch_state(&self, tx: oneshot::Sender<AdapterState>) {
        let manager = self.ivars().manager.borrow();
        let manager = manager.as_ref().expect("central manager exists");
        // SAFETY: `state` is a read-only accessor.
        let state = map_state(unsafe { manager.state() });
        if state == AdapterState::Unknown {
            self.ivars().state_txs.borrow_mut().push(tx);
        } else {
            let _ = tx.send(state);
        }
    }
}

// ---------------------------------------------------------------------------
// CoreBluetooth peripheral delegate
// ---------------------------------------------------------------------------

/// A pending `discover_services` round trip: the accumulated services plus a
/// countdown of `didDiscoverCharacteristicsForService` callbacks.
#[derive(Debug)]
struct DiscoverState {
    remaining: usize,
    services: Vec<GattService>,
    sender: oneshot::Sender<Result<Vec<GattService>, BluetoothError>>,
}

#[derive(Debug)]
struct PeripheralDelegateIvars {
    discover: RefCell<Option<DiscoverState>>,
    read_txs: RefCell<HashMap<String, ReadTx>>,
    write_txs: RefCell<HashMap<String, WriteUnitTx>>,
    notify_txs: RefCell<HashMap<String, Sender<Vec<u8>>>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `PeripheralDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WaterkitBlePeripheralDelegate"]
    #[ivars = PeripheralDelegateIvars]
    #[derive(Debug)]
    struct PeripheralDelegate;

    unsafe impl NSObjectProtocol for PeripheralDelegate {}

    unsafe impl CBPeripheralDelegate for PeripheralDelegate {
        #[unsafe(method(peripheralDidDiscoverServices:))]
        fn did_discover_services(&self, peripheral: &CBPeripheral, error: Option<&NSError>) {
            if error.is_some() {
                self.finish_discover(Err(BluetoothError::GattError(ns_error(
                    error,
                    "service discovery failed",
                ))));
                return;
            }
            // SAFETY: read-only accessor.
            let Some(services) = (unsafe { peripheral.services() }) else {
                self.finish_discover(Ok(Vec::new()));
                return;
            };
            {
                let mut slot = self.ivars().discover.borrow_mut();
                let Some(state) = slot.as_mut() else {
                    return;
                };
                state.remaining = services.len();
            }
            if services.is_empty() {
                self.finish_discover(Ok(Vec::new()));
                return;
            }
            for service in &services {
                // SAFETY: kicks off characteristic discovery on a live
                // discovered service; results return on this delegate.
                unsafe { peripheral.discoverCharacteristics_forService(None, &service) };
            }
        }

        #[unsafe(method(peripheral:didDiscoverCharacteristicsForService:error:))]
        fn did_discover_characteristics(
            &self,
            _peripheral: &CBPeripheral,
            service: &CBService,
            _error: Option<&NSError>,
        ) {
            // The previous implementation ignored the per-service error and
            // still accumulated whatever `service.characteristics` holds;
            // keep that behaviour.
            let done = {
                let mut slot = self.ivars().discover.borrow_mut();
                let Some(state) = slot.as_mut() else {
                    return;
                };
                state.services.push(gatt_service(service));
                state.remaining = state.remaining.saturating_sub(1);
                state.remaining == 0
            };
            if done {
                let services = self
                    .ivars()
                    .discover
                    .borrow_mut()
                    .take()
                    .map_or_else(Vec::new, |state| state.services);
                self.finish_discover(Ok(services));
            }
        }

        #[unsafe(method(peripheral:didUpdateValueForCharacteristic:error:))]
        fn did_update_value(
            &self,
            _peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&NSError>,
        ) {
            let key = characteristic_key(characteristic);
            if let Some(tx) = self.ivars().read_txs.borrow_mut().remove(&key) {
                let result = error.map_or_else(
                    || Ok(characteristic_value(characteristic)),
                    |error| {
                        Err(BluetoothError::GattError(
                            error.localizedDescription().to_string(),
                        ))
                    },
                );
                let _ = tx.send(result);
                return;
            }
            if let Some(tx) = self.ivars().notify_txs.borrow().get(&key) {
                let _ = tx.try_send(characteristic_value(characteristic));
            }
        }

        #[unsafe(method(peripheral:didWriteValueForCharacteristic:error:))]
        fn did_write_value(
            &self,
            _peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&NSError>,
        ) {
            let key = characteristic_key(characteristic);
            if let Some(tx) = self.ivars().write_txs.borrow_mut().remove(&key) {
                let result = error.map_or(Ok(()), |error| {
                    Err(BluetoothError::GattError(
                        error.localizedDescription().to_string(),
                    ))
                });
                let _ = tx.send(result);
            }
        }
    }
);

impl PeripheralDelegate {
    fn spawn() -> Retained<Self> {
        let mtm = MainThreadMarker::new().expect("on main queue");
        let this = Self::alloc(mtm).set_ivars(PeripheralDelegateIvars {
            discover: RefCell::new(None),
            read_txs: RefCell::new(HashMap::new()),
            write_txs: RefCell::new(HashMap::new()),
            notify_txs: RefCell::new(HashMap::new()),
        });
        // SAFETY: `init` on a plain NSObject subclass.
        unsafe { msg_send![super(this), init] }
    }

    /// Complete a pending `discover_services` round trip at most once.
    fn finish_discover(&self, result: Result<Vec<GattService>, BluetoothError>) {
        if let Some(state) = self.ivars().discover.borrow_mut().take() {
            let _ = state.sender.send(result);
        }
    }
}

// ---------------------------------------------------------------------------
// BLE entry points
// ---------------------------------------------------------------------------

pub async fn adapter_state() -> Result<AdapterState, BluetoothError> {
    let (tx, rx) = oneshot::channel();
    on_main(move || {
        let delegate = CentralDelegate::spawn();
        *delegate.ivars().keep_alive.borrow_mut() = Some(delegate.clone());
        delegate.watch_state(tx);
    });
    rx.await
        .map_err(|_| BluetoothError::Platform("adapter state callback dropped".into()))
}

/// `BleScanner` session: owns the central delegate (address in `central`)
/// and the scan-result receiver.
pub struct BleScannerInner {
    central: usize,
    pub(crate) scan_rx: Receiver<ScanResult>,
}

impl std::fmt::Debug for BleScannerInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BleScannerInner")
            .field("central", &self.central)
            .finish_non_exhaustive()
    }
}

impl BleScannerInner {
    pub async fn new() -> Result<Self, BluetoothError> {
        let state = adapter_state().await?;
        if state != AdapterState::PoweredOn {
            return Err(BluetoothError::NotAvailable);
        }
        let (tx, rx) = async_channel::bounded(64);
        let central = on_main(move || {
            let delegate = CentralDelegate::spawn();
            *delegate.ivars().scan_tx.borrow_mut() = Some(tx);
            // SAFETY: `into_raw` keeps the +1 retain; `Drop` reconstructs it
            // on the main thread to release the session.
            Retained::into_raw(delegate) as usize
        });
        Ok(Self {
            central,
            scan_rx: rx,
        })
    }

    #[allow(clippy::unnecessary_wraps)]
    pub fn start_scan(
        &self,
        filter: &ScanFilter,
    ) -> Result<async_channel::Receiver<ScanResult>, BluetoothError> {
        let central = self.central;
        let uuids: Vec<String> = filter
            .service_uuids
            .iter()
            .map(|uuid| uuid.as_str().to_string())
            .collect();
        on_main(move || {
            let delegate = unsafe { from_addr::<CentralDelegate>(central) };
            let manager = delegate.ivars().manager.borrow();
            let manager = manager.as_ref().expect("central manager exists");
            let service_uuids = if uuids.is_empty() {
                None
            } else {
                let cb_uuids: Vec<Retained<CBUUID>> = uuids
                    .iter()
                    // SAFETY: `UUIDWithString` parses UUID strings.
                    .map(|uuid| unsafe { CBUUID::UUIDWithString(&NSString::from_str(uuid)) })
                    .collect();
                Some(NSArray::from_retained_slice(&cb_uuids))
            };
            // `CBCentralManagerScanOptionAllowDuplicatesKey = false` — the
            // Discoveries are de-duplicated by the scan options below.
            let options: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
                &[unsafe { CBCentralManagerScanOptionAllowDuplicatesKey }],
                &[as_any_object::<NSNumber>(
                    NSNumber::numberWithBool(false).as_ref(),
                )],
            );
            // SAFETY: CoreBluetooth scan entry point on a powered-on manager.
            unsafe {
                manager.scanForPeripheralsWithServices_options(
                    service_uuids.as_deref(),
                    Some(&options),
                );
            };
        });
        Ok(self.scan_rx.clone())
    }

    pub fn stop_scan(&self) {
        let central = self.central;
        dispatch_main(move || {
            let delegate = unsafe { from_addr::<CentralDelegate>(central) };
            delegate.ivars().scan_tx.borrow_mut().take();
            if let Some(manager) = delegate.ivars().manager.borrow().as_ref() {
                // SAFETY: stops an in-flight scan on a live manager.
                unsafe { manager.stopScan() };
            }
        });
    }
}

impl Drop for BleScannerInner {
    fn drop(&mut self) {
        self.stop_scan();
        let central = self.central;
        dispatch_main(move || {
            // SAFETY: reclaims the retain created by `into_raw` in `new`.
            // SAFETY: reclaims the retain created by `into_raw` in `new`.
            drop(unsafe {
                Retained::from_raw(central as *mut CentralDelegate)
                    .expect("scanner delegate retain held by inner")
            });
        });
    }
}

/// `BleConnection` session: owns its central delegate plus the peripheral
/// delegate kept inside that delegate's ivars.
pub struct BleConnectionInner {
    device_id: DeviceId,
    central: usize,
}

impl std::fmt::Debug for BleConnectionInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BleConnectionInner")
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

impl BleConnectionInner {
    pub async fn connect(device_id: &DeviceId) -> Result<Self, BluetoothError> {
        let id = device_id.as_str().to_string();
        let (central, state_rx) = on_main(move || {
            let delegate = CentralDelegate::spawn();
            // The peripheral is looked up through `retrievePeripheralsWithIdentifiers`,
            // manager's dictionary; `retrievePeripheralsWithIdentifiers` is
            // the CoreBluetooth equivalent for a previously discovered or
            // connected device identifier.
            let ns_uuid = {
                let ns = NSString::from_str(&id);
                // SAFETY: documented NSUUID initializer; nil on unparseable
                // input.
                NSUUID::initWithUUIDString(
                    // SAFETY: `alloc` on a live class.
                    unsafe { msg_send![NSUUID::class(), alloc] },
                    &ns,
                )
            };
            let Some(ns_uuid) = ns_uuid else {
                return Err(BluetoothError::DeviceNotFound(id));
            };
            let identifiers = NSArray::from_retained_slice(&[ns_uuid]);
            // SAFETY: `retrievePeripheralsWithIdentifiers` on a live manager.
            let peripherals = unsafe {
                delegate
                    .ivars()
                    .manager
                    .borrow()
                    .as_ref()
                    .expect("central manager exists")
                    .retrievePeripheralsWithIdentifiers(&identifiers)
            };
            let Some(peripheral) = peripherals.firstObject() else {
                return Err(BluetoothError::DeviceNotFound(id));
            };
            let periph_delegate = PeripheralDelegate::spawn();
            // SAFETY: `setDelegate` takes a `CBPeripheralDelegate`-conforming
            // object; `delegate` is weak so the ivar keeps it alive.
            unsafe { peripheral.setDelegate(Some(ProtocolObject::from_ref(&*periph_delegate))) };
            *delegate.ivars().peripheral.borrow_mut() = Some(peripheral);
            *delegate.ivars().peripheral_delegate.borrow_mut() = Some(periph_delegate);
            let (tx, rx) = oneshot::channel();
            delegate.watch_state(tx);
            // SAFETY: `into_raw` holds the session retain until `disconnect`.
            Ok((Retained::into_raw(delegate) as usize, rx))
        })?;
        match state_rx
            .await
            .map_err(|_| BluetoothError::Platform("adapter state callback dropped".into()))?
        {
            AdapterState::PoweredOn => {}
            AdapterState::PoweredOff => return Err(BluetoothError::PoweredOff),
            _ => return Err(BluetoothError::NotAvailable),
        }

        let (tx, rx) = oneshot::channel();
        on_main(move || {
            let delegate = unsafe { from_addr::<CentralDelegate>(central) };
            let peripheral = delegate.ivars().peripheral.borrow();
            let peripheral = peripheral.as_ref().expect("peripheral stored at connect");
            let manager = delegate.ivars().manager.borrow();
            let manager = manager.as_ref().expect("central manager exists");
            // SAFETY: read-only accessor used as the callback key.
            let key = unsafe { peripheral.identifier() }.UUIDString().to_string();
            delegate.ivars().connect_txs.borrow_mut().insert(key, tx);
            // SAFETY: `connectPeripheral` on a powered-on manager.
            unsafe { manager.connectPeripheral_options(peripheral, None) };
        });
        rx.await
            .map_err(|_| BluetoothError::ConnectionFailed("callback dropped".into()))??;
        Ok(Self {
            device_id: device_id.clone(),
            central,
        })
    }

    pub async fn discover_services(&self) -> Result<Vec<GattService>, BluetoothError> {
        let central = self.central;
        let (tx, rx) = oneshot::channel();
        on_main(move || {
            let delegate = unsafe { from_addr::<CentralDelegate>(central) };
            let peripheral = delegate.ivars().peripheral.borrow();
            let periph_delegate = delegate.ivars().peripheral_delegate.borrow();
            let (Some(peripheral), Some(periph_delegate)) =
                (peripheral.as_ref(), periph_delegate.as_ref())
            else {
                let _ = tx.send(Err(BluetoothError::GattError(
                    "peripheral unavailable".into(),
                )));
                return;
            };
            *periph_delegate.ivars().discover.borrow_mut() = Some(DiscoverState {
                remaining: 0,
                services: Vec::new(),
                sender: tx,
            });
            // SAFETY: `discoverServices` on the live connected peripheral.
            unsafe { peripheral.discoverServices(None) };
        });
        rx.await
            .map_err(|_| BluetoothError::GattError("callback dropped".into()))?
    }

    /// Shared lookup + sender registration for `read_characteristic` and
    /// `write_characteristic`; `tx` is failed with `GattError` when the
    /// characteristic is not found, mirroring the previous `"Characteristic not
    /// found"` path.
    fn with_characteristic(
        central: usize,
        service: &Uuid,
        characteristic: &Uuid,
        op: impl FnOnce(&PeripheralDelegate, &CBPeripheral, Retained<CBCharacteristic>) + Send + 'static,
    ) -> Result<(), BluetoothError> {
        let svc = service.as_str().to_string();
        let chr = characteristic.as_str().to_string();
        on_main(move || {
            let delegate = unsafe { from_addr::<CentralDelegate>(central) };
            let peripheral = delegate.ivars().peripheral.borrow();
            let periph_delegate = delegate.ivars().peripheral_delegate.borrow();
            let (Some(peripheral), Some(periph_delegate)) =
                (peripheral.as_ref(), periph_delegate.as_ref())
            else {
                return Err(BluetoothError::GattError("peripheral unavailable".into()));
            };
            let Some(characteristic) = find_characteristic(peripheral, &svc, &chr) else {
                return Err(BluetoothError::GattError("characteristic not found".into()));
            };
            op(periph_delegate, peripheral, characteristic);
            Ok(())
        })
    }

    pub async fn read_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<Vec<u8>, BluetoothError> {
        let central = self.central;
        let (tx, rx) = oneshot::channel::<Result<Vec<u8>, BluetoothError>>();
        let mut tx = Some(tx);
        Self::with_characteristic(
            central,
            service,
            characteristic,
            move |periph_delegate, peripheral, characteristic| {
                let Some(tx) = tx.take() else { return };
                periph_delegate
                    .ivars()
                    .read_txs
                    .borrow_mut()
                    .insert(characteristic_key(&characteristic), tx);
                // SAFETY: `readValueForCharacteristic` on a live
                // characteristic of the connected peripheral.
                unsafe { peripheral.readValueForCharacteristic(&characteristic) };
            },
        )?;
        rx.await
            .map_err(|_| BluetoothError::GattError("callback dropped".into()))?
    }

    pub async fn write_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
        data: &[u8],
    ) -> Result<(), BluetoothError> {
        let central = self.central;
        let payload = data.to_vec();
        let (tx, rx) = oneshot::channel::<Result<(), BluetoothError>>();
        let mut tx = Some(tx);
        Self::with_characteristic(
            central,
            service,
            characteristic,
            move |periph_delegate, peripheral, characteristic| {
                let Some(tx) = tx.take() else { return };
                periph_delegate
                    .ivars()
                    .write_txs
                    .borrow_mut()
                    .insert(characteristic_key(&characteristic), tx);
                // SAFETY: `dataWithBytes` copies `payload` into a live
                // NSData for the duration of the call.
                let data = unsafe {
                    NSData::dataWithBytes_length(
                        payload.as_ptr().cast::<c_void>().cast_mut(),
                        payload.len(),
                    )
                };
                // SAFETY: `writeValue` on a live characteristic;
                // `WithResponse` matches the previous behaviour.
                unsafe {
                    peripheral.writeValue_forCharacteristic_type(
                        &data,
                        &characteristic,
                        CBCharacteristicWriteType::WithResponse,
                    );
                };
            },
        )?;
        rx.await
            .map_err(|_| BluetoothError::GattError("callback dropped".into()))?
    }

    pub fn subscribe(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<Receiver<Vec<u8>>, BluetoothError> {
        let central = self.central;
        let (tx, rx) = async_channel::bounded(64);
        Self::with_characteristic(
            central,
            service,
            characteristic,
            move |periph_delegate, peripheral, characteristic| {
                periph_delegate
                    .ivars()
                    .notify_txs
                    .borrow_mut()
                    .insert(characteristic_key(&characteristic), tx);
                // SAFETY: `setNotifyValue` on a live characteristic.
                unsafe { peripheral.setNotifyValue_forCharacteristic(true, &characteristic) };
            },
        )?;
        Ok(rx)
    }

    pub fn disconnect(self) {
        let central = self.central;
        dispatch_main(move || {
            // SAFETY: reclaims the retain created by `into_raw` in `connect`;
            // dropping the delegate releases manager + peripheral delegates.
            let delegate = unsafe {
                Retained::from_raw(central as *mut CentralDelegate)
                    .expect("connection delegate retain held by inner")
            };
            if let (Some(manager), Some(peripheral)) = (
                delegate.ivars().manager.borrow().as_ref(),
                delegate.ivars().peripheral.borrow().as_ref(),
            ) {
                // SAFETY: `cancelPeripheralConnection` on a live pair.
                unsafe { manager.cancelPeripheralConnection(peripheral) };
            }
            drop(delegate);
        });
    }
}

// ---------------------------------------------------------------------------
// Classic Bluetooth — macOS only
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod classic {
    use super::{
        BluetoothDevice, BluetoothError, ClassType, ClassicDevice, DefinedClass, DeviceId, HashMap,
        K_IO_RETURN_SUCCESS, MainThreadMarker, MainThreadOnly, NSObject, NSObjectProtocol,
        PendingRead, PendingWrite, Receiver, RefCell, Retained, Sender, Uuid, WriteTx,
        as_any_object, c_void, define_class, dispatch_main, from_addr, msg_send, on_main, oneshot,
    };
    use objc2_foundation::{NSArray, NSData, NSString};
    use objc2_io_bluetooth::{
        IOBluetoothDevice, IOBluetoothDeviceAsyncCallbacks, IOBluetoothDeviceInquiry,
        IOBluetoothDeviceInquiryDelegate, IOBluetoothDeviceSearchTypesBits,
        IOBluetoothRFCOMMChannel, IOBluetoothRFCOMMChannelDelegate, IOBluetoothSDPUUID,
    };
    use std::cell::Cell;

    /// The discovery delegate: owns the `IOBluetoothDeviceInquiry` and the
    /// stream sender.
    #[derive(Debug)]
    struct InquiryIvars {
        inquiry: RefCell<Option<Retained<IOBluetoothDeviceInquiry>>>,
        tx: RefCell<Option<Sender<ClassicDevice>>>,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements.
        // - `Inquiry` does not implement `Drop`.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WaterkitClassicInquiry"]
        #[ivars = InquiryIvars]
        #[derive(Debug)]
        struct Inquiry;

        unsafe impl NSObjectProtocol for Inquiry {}

        unsafe impl IOBluetoothDeviceInquiryDelegate for Inquiry {
            #[unsafe(method(deviceInquiryDeviceFound:device:))]
            fn device_found(
                &self,
                _sender: Option<&IOBluetoothDeviceInquiry>,
                device: Option<&IOBluetoothDevice>,
            ) {
                let tx = self.ivars().tx.borrow();
                let (Some(device), Some(tx)) = (device, tx.as_ref()) else {
                    return;
                };
                let _ = tx.try_send(classic_device(device));
            }
        }
    );

    impl Inquiry {
        fn spawn(tx: Sender<ClassicDevice>) -> Retained<Self> {
            let mtm = MainThreadMarker::new().expect("on main queue");
            let this = Self::alloc(mtm).set_ivars(InquiryIvars {
                inquiry: RefCell::new(None),
                tx: RefCell::new(Some(tx)),
            });
            // SAFETY: `init` on a plain NSObject subclass.
            unsafe { msg_send![super(this), init] }
        }
    }

    fn classic_device(device: &IOBluetoothDevice) -> ClassicDevice {
        // SAFETY: read-only accessors on a live IOBluetoothDevice.
        let (address, name, class_of_device, connected, paired) = unsafe {
            (
                device.addressString(),
                device.name(),
                device.classOfDevice(),
                device.isConnected(),
                device.isPaired(),
            )
        };
        ClassicDevice {
            device: BluetoothDevice {
                id: DeviceId::new(address.map_or_else(String::new, |a| a.to_string())),
                name: Some(name.to_string()),
                rssi: None,
                is_connected: connected,
            },
            device_class: class_of_device,
            is_paired: paired,
        }
    }

    /// The `connect_spp` connector: performs `performSDPQuery:` then
    /// `openRFCOMMChannelAsync:withChannelID:delegate:` and resolves with the
    /// address of the `SppStream` delegate it creates.
    #[derive(Debug)]
    struct ConnectorIvars {
        sdp_uuid: Retained<IOBluetoothSDPUUID>,
        tx: RefCell<Option<oneshot::Sender<Result<usize, BluetoothError>>>>,
        channel: RefCell<Option<Retained<IOBluetoothRFCOMMChannel>>>,
        keep_alive: RefCell<Option<Retained<Connector>>>,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements.
        // - `Connector` does not implement `Drop`.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WaterkitSppConnector"]
        #[ivars = ConnectorIvars]
        #[derive(Debug)]
        struct Connector;

        unsafe impl NSObjectProtocol for Connector {}

        unsafe impl IOBluetoothDeviceAsyncCallbacks for Connector {
            #[unsafe(method(sdpQueryComplete:status:))]
            fn sdp_query_complete(
                &self,
                device: Option<&IOBluetoothDevice>,
                status: core::ffi::c_int,
            ) {
                if status != K_IO_RETURN_SUCCESS {
                    self.finish(Err(BluetoothError::ConnectionFailed(format!(
                        "SDP query failed: {status}"
                    ))));
                    return;
                }
                let Some(device) = device else {
                    self.finish(Err(BluetoothError::ConnectionFailed(
                        "SDP query returned no device".into(),
                    )));
                    return;
                };
                // SAFETY: `getServiceRecordForUUID` on a device whose SDP
                // query just completed.
                let record =
                    unsafe { device.getServiceRecordForUUID(Some(&self.ivars().sdp_uuid)) };
                let Some(record) = record else {
                    self.finish(Err(BluetoothError::ConnectionFailed(
                        "SPP service record not found".into(),
                    )));
                    return;
                };
                let mut channel_id: objc2_io_bluetooth::BluetoothRFCOMMChannelID = 0;
                // SAFETY: `getRFCOMMChannelID` writes `channel_id` on
                // success.
                let status = unsafe { record.getRFCOMMChannelID(&raw mut channel_id) };
                if status != K_IO_RETURN_SUCCESS {
                    self.finish(Err(BluetoothError::ConnectionFailed(format!(
                        "RFCOMM channel id lookup failed: {status}"
                    ))));
                    return;
                }
                let mut channel: Option<Retained<IOBluetoothRFCOMMChannel>> = None;
                // SAFETY: `openRFCOMMChannelAsync` writes the channel into
                // `channel` and reports completion through this delegate's
                // `rfcommChannelOpenComplete:status:`.
                let status = unsafe {
                    device.openRFCOMMChannelAsync_withChannelID_delegate(
                        Some(&mut channel),
                        channel_id,
                        Some(as_any_object(self)),
                    )
                };
                if status != K_IO_RETURN_SUCCESS {
                    self.finish(Err(BluetoothError::ConnectionFailed(format!(
                        "open RFCOMM channel failed: {status}"
                    ))));
                    return;
                }
                // The sender + self-retain stay parked in the ivars until
                // `rfcommChannelOpenComplete:status:` resolves them.
                *self.ivars().channel.borrow_mut() = channel;
            }
        }

        unsafe impl IOBluetoothRFCOMMChannelDelegate for Connector {
            #[unsafe(method(rfcommChannelOpenComplete:status:))]
            fn open_complete(
                &self,
                channel: Option<&IOBluetoothRFCOMMChannel>,
                status: core::ffi::c_int,
            ) {
                if status != K_IO_RETURN_SUCCESS {
                    self.finish(Err(BluetoothError::ConnectionFailed(format!(
                        "RFCOMM channel open failed: {status}"
                    ))));
                    return;
                }
                let channel = self
                    .ivars()
                    .channel
                    .borrow_mut()
                    .take()
                    .or_else(|| channel.map(Retained::from));
                let Some(channel) = channel else {
                    self.finish(Err(BluetoothError::ConnectionFailed(
                        "RFCOMM channel missing on open".into(),
                    )));
                    return;
                };
                let stream = SppStream::spawn(channel.clone());
                // SAFETY: the channel delegate must respond to the
                // IOBluetoothRFCOMMChannelDelegate selectors the stream
                // implements.
                let status = unsafe { channel.setDelegate(Some(as_any_object(&*stream))) };
                if status != K_IO_RETURN_SUCCESS {
                    self.finish(Err(BluetoothError::ConnectionFailed(format!(
                        "RFCOMM channel delegate failed: {status}"
                    ))));
                    return;
                }
                // SAFETY: `into_raw` transfers the stream retain to
                // `SppStreamInner`.
                self.finish(Ok(Retained::into_raw(stream) as usize));
            }

            #[unsafe(method(rfcommChannelClosed:))]
            fn channel_closed(&self, _channel: Option<&IOBluetoothRFCOMMChannel>) {
                self.finish(Err(BluetoothError::ConnectionFailed(
                    "RFCOMM channel closed during connect".into(),
                )));
            }
        }
    );

    impl Connector {
        fn spawn(
            sdp_uuid: Retained<IOBluetoothSDPUUID>,
            tx: oneshot::Sender<Result<usize, BluetoothError>>,
        ) -> Retained<Self> {
            let mtm = MainThreadMarker::new().expect("on main queue");
            let this = Self::alloc(mtm).set_ivars(ConnectorIvars {
                sdp_uuid,
                tx: RefCell::new(Some(tx)),
                channel: RefCell::new(None),
                keep_alive: RefCell::new(None),
            });
            // SAFETY: `init` on a plain NSObject subclass.
            unsafe { msg_send![super(this), init] }
        }

        /// Resolve the connect sender (at most once) and release the
        /// self-retain that kept the connect session alive.
        fn finish(&self, result: Result<usize, BluetoothError>) {
            if let Some(tx) = self.ivars().tx.borrow_mut().take() {
                let _ = tx.send(result);
            }
            self.ivars().keep_alive.borrow_mut().take();
        }
    }

    /// The SPP stream delegate: owns the channel, the read buffer, pending
    /// reads and pending writes (the `refcon` key is a per-write token — no
    /// global registry).
    #[derive(Debug)]
    struct SppStreamIvars {
        channel: RefCell<Option<Retained<IOBluetoothRFCOMMChannel>>>,
        buffer: RefCell<Vec<u8>>,
        pending_reads: RefCell<VecDeque<PendingRead>>,
        pending_writes: RefCell<HashMap<usize, PendingWrite>>,
        next_write: Cell<usize>,
        closed: Cell<bool>,
    }

    use std::collections::VecDeque;

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements.
        // - `SppStream` does not implement `Drop`; teardown goes through
        //   `close_stream`/`rfcommChannelClosed:`.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WaterkitSppStream"]
        #[ivars = SppStreamIvars]
        #[derive(Debug)]
        struct SppStream;

        unsafe impl NSObjectProtocol for SppStream {}

        unsafe impl IOBluetoothRFCOMMChannelDelegate for SppStream {
            #[unsafe(method(rfcommChannelData:data:length:))]
            fn did_receive(
                &self,
                _channel: Option<&IOBluetoothRFCOMMChannel>,
                data: *mut c_void,
                length: usize,
            ) {
                // SAFETY: `data` points to `length` valid bytes for the
                // duration of this callback.
                let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
                self.ivars().buffer.borrow_mut().extend_from_slice(bytes);
                self.drain_reads();
            }

            #[unsafe(method(rfcommChannelOpenComplete:status:))]
            fn open_complete(
                &self,
                channel: Option<&IOBluetoothRFCOMMChannel>,
                status: core::ffi::c_int,
            ) {
                let _ = channel;
                if status != K_IO_RETURN_SUCCESS {
                    self.ivars().closed.set(true);
                    self.fail_all(&BluetoothError::ConnectionFailed(format!(
                        "RFCOMM channel closed with status {status}"
                    )));
                }
            }

            #[unsafe(method(rfcommChannelClosed:))]
            fn channel_closed(&self, _channel: Option<&IOBluetoothRFCOMMChannel>) {
                self.ivars().closed.set(true);
                self.fail_all(&BluetoothError::ConnectionFailed(
                    "RFCOMM channel closed".into(),
                ));
            }

            #[unsafe(method(rfcommChannelWriteComplete:refcon:status:))]
            fn write_complete(
                &self,
                _channel: Option<&IOBluetoothRFCOMMChannel>,
                refcon: *mut c_void,
                status: core::ffi::c_int,
            ) {
                self.finish_write(refcon, status, None);
            }

            #[unsafe(method(rfcommChannelWriteComplete:refcon:status:bytesWritten:))]
            fn write_complete_bytes(
                &self,
                _channel: Option<&IOBluetoothRFCOMMChannel>,
                refcon: *mut c_void,
                status: core::ffi::c_int,
                bytes_written: usize,
            ) {
                self.finish_write(refcon, status, Some(bytes_written));
            }
        }
    );

    impl SppStream {
        fn spawn(channel: Retained<IOBluetoothRFCOMMChannel>) -> Retained<Self> {
            let mtm = MainThreadMarker::new().expect("on main queue");
            let this = Self::alloc(mtm).set_ivars(SppStreamIvars {
                channel: RefCell::new(Some(channel)),
                buffer: RefCell::new(Vec::new()),
                pending_reads: RefCell::new(VecDeque::new()),
                pending_writes: RefCell::new(HashMap::new()),
                next_write: Cell::new(1),
                closed: Cell::new(false),
            });
            // SAFETY: `init` on a plain NSObject subclass.
            unsafe { msg_send![super(this), init] }
        }

        fn drain_reads(&self) {
            loop {
                let entry = {
                    let mut reads = self.ivars().pending_reads.borrow_mut();
                    if self.ivars().buffer.borrow().is_empty() {
                        None
                    } else {
                        reads.pop_front()
                    }
                };
                let Some((max, tx)) = entry else { break };
                let chunk = {
                    let mut buffer = self.ivars().buffer.borrow_mut();
                    let n = buffer.len().min(max);
                    buffer.drain(..n).collect::<Vec<u8>>()
                };
                let _ = tx.send(Ok(chunk));
            }
        }

        fn enqueue_read(&self, max: usize, tx: oneshot::Sender<Result<Vec<u8>, BluetoothError>>) {
            if self.ivars().closed.get() {
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(
                    "RFCOMM channel closed".into(),
                )));
                return;
            }
            let chunk = {
                let mut buffer = self.ivars().buffer.borrow_mut();
                (!buffer.is_empty()).then(|| {
                    let n = buffer.len().min(max);
                    buffer.drain(..n).collect::<Vec<u8>>()
                })
            };
            match chunk {
                Some(chunk) => {
                    let _ = tx.send(Ok(chunk));
                }
                None => self.ivars().pending_reads.borrow_mut().push_back((max, tx)),
            }
        }

        fn enqueue_write(&self, data: &[u8], tx: oneshot::Sender<Result<usize, BluetoothError>>) {
            if self.ivars().closed.get() {
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(
                    "RFCOMM channel closed".into(),
                )));
                return;
            }
            let Ok(length) = u16::try_from(data.len()) else {
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(
                    "write payload exceeds RFCOMM limits".into(),
                )));
                return;
            };
            // SAFETY: `dataWithBytes` copies `data` into a live NSData that
            // stays retained in `pending_writes` until the write completes.
            let ns_data = unsafe {
                NSData::dataWithBytes_length(data.as_ptr().cast::<c_void>().cast_mut(), data.len())
            };
            let token = self.ivars().next_write.get();
            self.ivars().next_write.set(token + 1);
            self.ivars()
                .pending_writes
                .borrow_mut()
                .insert(token, (tx, ns_data.clone()));
            let channel = self.ivars().channel.borrow();
            let Some(channel) = channel.as_ref() else {
                self.ivars().pending_writes.borrow_mut().remove(&token);
                return;
            };
            // SAFETY: `token` is the refcon; `ns_data` is retained until the
            // write-completion callback.
            let status = unsafe {
                channel.writeAsync_length_refcon(
                    ns_data
                        .as_bytes_unchecked()
                        .as_ptr()
                        .cast::<c_void>()
                        .cast_mut(),
                    length,
                    token as *mut c_void,
                )
            };
            if status != K_IO_RETURN_SUCCESS
                && let Some((tx, _)) = self.ivars().pending_writes.borrow_mut().remove(&token)
            {
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(format!(
                    "RFCOMM write failed: {status}"
                ))));
            }
        }

        fn finish_write(
            &self,
            refcon: *mut c_void,
            status: core::ffi::c_int,
            bytes_written: Option<usize>,
        ) {
            let Some((tx, data)) = self
                .ivars()
                .pending_writes
                .borrow_mut()
                .remove(&(refcon as usize))
            else {
                return;
            };
            if status != K_IO_RETURN_SUCCESS {
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(format!(
                    "RFCOMM write failed: {status}"
                ))));
                return;
            }
            let _ = tx.send(Ok(bytes_written.unwrap_or_else(|| data.length())));
        }

        fn fail_all(&self, error: &BluetoothError) {
            let message = error.to_string();
            for (_, tx) in self.ivars().pending_reads.borrow_mut().drain(..) {
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(message.clone())));
            }
            for (_, (tx, _)) in self.ivars().pending_writes.borrow_mut().drain() {
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(message.clone())));
            }
        }

        /// Close the channel and fail every pending operation.
        fn close_stream(&self) -> Result<(), BluetoothError> {
            if self.ivars().closed.replace(true) {
                return Err(BluetoothError::ConnectionFailed(
                    "RFCOMM channel already closed".into(),
                ));
            }
            let status = self.ivars().channel.borrow_mut().take().map_or(
                K_IO_RETURN_SUCCESS,
                // SAFETY: `closeChannel` on a live channel.
                |channel| unsafe { channel.closeChannel() },
            );
            self.fail_all(&BluetoothError::ConnectionFailed(
                "RFCOMM channel closed".into(),
            ));
            if status != K_IO_RETURN_SUCCESS {
                return Err(BluetoothError::Platform(format!(
                    "RFCOMM close failed: {status}"
                )));
            }
            Ok(())
        }
    }

    /// Parse `4`, `8` or `32` hex digits into an `IOBluetoothSDPUUID` —
    /// Parses an SPP UUID string (16/32/128-bit hex) into an `IOBluetoothSDPUUID`.
    fn parse_sdp_uuid(uuid: &str) -> Option<Retained<IOBluetoothSDPUUID>> {
        let cleaned: String = uuid.chars().filter(char::is_ascii_hexdigit).collect();
        let bytes = match cleaned.len() {
            4 => u16::from_str_radix(&cleaned, 16)
                .ok()?
                .to_be_bytes()
                .to_vec(),
            8 => u32::from_str_radix(&cleaned, 16)
                .ok()?
                .to_be_bytes()
                .to_vec(),
            32 => {
                let mut bytes = Vec::with_capacity(16);
                for i in (0..32).step_by(2) {
                    bytes.push(u8::from_str_radix(&cleaned[i..i + 2], 16).ok()?);
                }
                bytes
            }
            _ => return None,
        };
        // SAFETY: `uuidWithBytes:length:` reads `bytes.len()` bytes.
        unsafe {
            IOBluetoothSDPUUID::uuidWithBytes_length(
                bytes.as_ptr().cast(),
                u32::try_from(bytes.len()).ok()?,
            )
        }
    }

    pub fn start_discovery() -> Result<(usize, Receiver<ClassicDevice>), BluetoothError> {
        let (tx, rx) = async_channel::bounded(64);
        let result = on_main(move || {
            let delegate = Inquiry::spawn(tx);
            // SAFETY: `initWithDelegate:` is the documented inquiry
            // initializer; the delegate conforms to
            // `IOBluetoothDeviceInquiryDelegate` and is owned via the raw
            // retain returned below.
            let inquiry = unsafe {
                IOBluetoothDeviceInquiry::initWithDelegate(
                    msg_send![IOBluetoothDeviceInquiry::class(), alloc],
                    Some(as_any_object(&*delegate)),
                )
            };
            let Some(inquiry) = inquiry else {
                return Err(BluetoothError::Platform(
                    "IOBluetoothDeviceInquiry unavailable".into(),
                ));
            };
            // SAFETY: classic-only inquiry search configuration.
            unsafe {
                inquiry.setSearchType(IOBluetoothDeviceSearchTypesBits::Classic.0);
                inquiry.setUpdateNewDeviceNames(false);
            }
            // SAFETY: `start` on a configured inquiry.
            let status = unsafe { inquiry.start() };
            if status != K_IO_RETURN_SUCCESS {
                return Err(BluetoothError::Platform(format!(
                    "Bluetooth inquiry failed to start: {status}"
                )));
            }
            *delegate.ivars().inquiry.borrow_mut() = Some(inquiry);
            // SAFETY: `into_raw` holds the session retain until
            // `stop_discovery`.
            Ok(Retained::into_raw(delegate) as usize)
        })?;
        Ok((result, rx))
    }

    /// Stop + release the inquiry delegate whose address is `addr`.
    pub fn stop_discovery(addr: usize) {
        dispatch_main(move || {
            // SAFETY: address produced by `Retained::into_raw` in
            // `start_discovery`.
            let delegate = unsafe {
                Retained::from_raw(addr as *mut Inquiry)
                    .expect("inquiry delegate retain held by inner")
            };
            if let Some(inquiry) = delegate.ivars().inquiry.borrow_mut().take() {
                // SAFETY: `stop` on a live inquiry.
                unsafe { inquiry.stop() };
            }
            delegate.ivars().tx.borrow_mut().take();
        });
    }

    pub fn paired_devices() -> Vec<ClassicDevice> {
        on_main(|| {
            // SAFETY: `pairedDevices` is the documented class accessor.
            let devices = unsafe { IOBluetoothDevice::pairedDevices() };
            let Some(devices) = devices else {
                return Vec::new();
            };
            devices
                .iter()
                .filter_map(|object| {
                    object
                        .downcast_ref::<IOBluetoothDevice>()
                        .map(classic_device)
                })
                .collect()
        })
    }

    /// Look up the device, query its SDP record for `uuid`, open the RFCOMM
    /// channel and resolve with the `SppStream` delegate's address.
    pub fn connect_spp(
        device_id: &DeviceId,
        uuid: &Uuid,
        tx: oneshot::Sender<Result<usize, BluetoothError>>,
    ) {
        let address = device_id.as_str().to_string();
        let uuid_string = uuid.as_str().to_string();
        on_main(move || {
            let mut tx = Some(tx);
            let fail =
                |error: BluetoothError,
                 tx: &mut Option<oneshot::Sender<Result<usize, BluetoothError>>>| {
                    if let Some(tx) = tx.take() {
                        let _ = tx.send(Err(error));
                    }
                };
            let ns_address = NSString::from_str(&address);
            // SAFETY: `deviceWithAddressString` returns nil for unknown
            // addresses.
            let device = unsafe { IOBluetoothDevice::deviceWithAddressString(Some(&ns_address)) };
            let Some(device) = device else {
                fail(
                    BluetoothError::ConnectionFailed(format!(
                        "Classic Bluetooth device not found: {address}"
                    )),
                    &mut tx,
                );
                return;
            };
            let Some(sdp_uuid) = parse_sdp_uuid(&uuid_string) else {
                fail(
                    BluetoothError::ConnectionFailed("Invalid SPP UUID".into()),
                    &mut tx,
                );
                return;
            };
            let Some(tx) = tx.take() else { return };
            let connector = Connector::spawn(sdp_uuid.clone(), tx);
            *connector.ivars().keep_alive.borrow_mut() = Some(connector.clone());
            let uuids = NSArray::from_retained_slice(&[sdp_uuid]);
            // SAFETY: `NSArray<IOBluetoothSDPUUID>` → `NSArray<AnyObject>`
            // is an element-type upcast of an immutable array.
            let uuids: &NSArray = unsafe { &*(&raw const *uuids).cast() };
            // SAFETY: `performSDPQuery:uuids:` reports completion on the
            // connector via `sdpQueryComplete:status:`.
            let status = unsafe {
                device.performSDPQuery_uuids(Some(as_any_object(&*connector)), Some(uuids))
            };
            if status != K_IO_RETURN_SUCCESS {
                if let Some(tx) = connector.ivars().tx.borrow_mut().take() {
                    let _ = tx.send(Err(BluetoothError::ConnectionFailed(format!(
                        "SDP query failed to start: {status}"
                    ))));
                }
                connector.ivars().keep_alive.borrow_mut().take();
            }
        });
    }

    /// `SppStream` is private to this module; the `SppStreamInner` entry
    /// points go through these thin wrappers around the stream's methods.
    pub fn spp_read(addr: usize, max: usize, tx: oneshot::Sender<Result<Vec<u8>, BluetoothError>>) {
        // SAFETY: address produced by `Retained::into_raw` when the channel
        // opened; the stream lives as long as `SppStreamInner`.
        unsafe { from_addr::<SppStream>(addr) }.enqueue_read(max, tx);
    }

    pub fn spp_write(addr: usize, payload: &[u8], tx: WriteTx) {
        // SAFETY: see `spp_read`.
        unsafe { from_addr::<SppStream>(addr) }.enqueue_write(payload, tx);
    }

    /// Reclaim the stream retain, close the channel and fail pendings.
    pub fn spp_close(addr: usize) {
        // SAFETY: reclaims the retain created by `into_raw` on open.
        let stream = unsafe {
            Retained::from_raw(addr as *mut SppStream).expect("stream retain held by inner")
        };
        let _ = stream.close_stream();
    }
}

/// `ClassicBluetooth` session state: the active inquiry delegate's address
/// (macOS only).
pub struct ClassicBluetoothInner {
    #[cfg(target_os = "macos")]
    inquiry: Mutex<Option<usize>>,
}

impl std::fmt::Debug for ClassicBluetoothInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClassicBluetoothInner")
            .finish_non_exhaustive()
    }
}

#[cfg(target_os = "ios")]
const fn ios_classic_unavailable<T>() -> Result<T, BluetoothError> {
    Err(BluetoothError::NotAvailable)
}

impl ClassicBluetoothInner {
    #[cfg_attr(
        target_os = "ios",
        expect(clippy::unused_async, reason = "iOS stub returns immediately")
    )]
    pub async fn new() -> Result<Self, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            ios_classic_unavailable()
        }
        #[cfg(target_os = "macos")]
        {
            let state = adapter_state().await?;
            if state != AdapterState::PoweredOn {
                return Err(BluetoothError::NotAvailable);
            }
            Ok(Self {
                inquiry: Mutex::new(None),
            })
        }
    }

    #[cfg_attr(
        target_os = "ios",
        expect(clippy::missing_const_for_fn, reason = "macOS cfg body is non-const")
    )]
    pub fn start_discovery(&self) -> Result<Receiver<ClassicDevice>, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
            ios_classic_unavailable()
        }
        #[cfg(target_os = "macos")]
        {
            self.stop_discovery();
            let (addr, rx) = classic::start_discovery()?;
            *self.inquiry.lock().expect("inquiry mutex poisoned") = Some(addr);
            Ok(rx)
        }
    }

    #[cfg_attr(
        target_os = "ios",
        expect(clippy::missing_const_for_fn, reason = "macOS cfg body is non-const")
    )]
    pub fn stop_discovery(&self) {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
        }
        #[cfg(target_os = "macos")]
        {
            let addr = self.inquiry.lock().expect("inquiry mutex poisoned").take();
            if let Some(addr) = addr {
                classic::stop_discovery(addr);
            }
        }
    }

    #[cfg_attr(
        target_os = "macos",
        expect(
            clippy::unnecessary_wraps,
            reason = "iOS cfg always errors; kept fallible for parity"
        )
    )]
    #[cfg_attr(
        target_os = "macos",
        expect(clippy::unused_self, reason = "method kept for API symmetry")
    )]
    #[cfg_attr(
        target_os = "ios",
        expect(clippy::missing_const_for_fn, reason = "macOS cfg body is non-const")
    )]
    pub fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
            ios_classic_unavailable()
        }
        #[cfg(target_os = "macos")]
        {
            Ok(classic::paired_devices())
        }
    }

    #[cfg_attr(
        target_os = "ios",
        expect(clippy::unused_async, reason = "iOS stub returns immediately")
    )]
    pub async fn connect_spp(
        &self,
        device_id: &DeviceId,
        uuid: &Uuid,
    ) -> Result<SppStreamInner, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = (self, device_id, uuid);
            ios_classic_unavailable()
        }
        #[cfg(target_os = "macos")]
        {
            let _ = self;
            let (tx, rx) = oneshot::channel();
            classic::connect_spp(device_id, uuid, tx);
            let stream = rx.await.map_err(|_| {
                BluetoothError::ConnectionFailed("classic SPP connect callback dropped".into())
            })??;
            Ok(SppStreamInner { stream })
        }
    }
}

impl Drop for ClassicBluetoothInner {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let addr = self.inquiry.lock().expect("inquiry mutex poisoned").take();
            if let Some(addr) = addr {
                classic::stop_discovery(addr);
            }
        }
    }
}

/// `SppStream` session state: the stream delegate's address (macOS only).
pub struct SppStreamInner {
    #[cfg(target_os = "macos")]
    stream: usize,
}

impl std::fmt::Debug for SppStreamInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SppStreamInner").finish_non_exhaustive()
    }
}

impl SppStreamInner {
    #[cfg_attr(
        target_os = "ios",
        expect(clippy::unused_async, reason = "iOS stub returns immediately")
    )]
    pub async fn read(&self, buf: &mut [u8]) -> Result<usize, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = (self, buf);
            ios_classic_unavailable()
        }
        #[cfg(target_os = "macos")]
        {
            let stream = self.stream;
            let max = buf.len();
            let (tx, rx) = oneshot::channel();
            dispatch_main(move || classic::spp_read(stream, max, tx));
            let data = rx.await.map_err(|_| {
                BluetoothError::ConnectionFailed("classic SPP read callback dropped".into())
            })??;
            let n = data.len().min(buf.len());
            buf[..n].copy_from_slice(&data[..n]);
            Ok(n)
        }
    }

    #[cfg_attr(
        target_os = "ios",
        expect(clippy::unused_async, reason = "iOS stub returns immediately")
    )]
    pub async fn write(&self, data: &[u8]) -> Result<usize, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = (self, data);
            ios_classic_unavailable()
        }
        #[cfg(target_os = "macos")]
        {
            let stream = self.stream;
            let payload = data.to_vec();
            let (tx, rx) = oneshot::channel();
            dispatch_main(move || classic::spp_write(stream, &payload, tx));
            rx.await.map_err(|_| {
                BluetoothError::ConnectionFailed("classic SPP write callback dropped".into())
            })?
        }
    }

    pub fn close(self) {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
        }
        #[cfg(target_os = "macos")]
        {
            let stream = self.stream;
            // The retain is reclaimed on the main queue; `self` must not be
            // dropped (its `Drop` would reclaim it a second time).
            core::mem::forget(self);
            dispatch_main(move || classic::spp_close(stream));
        }
    }
}

impl Drop for SppStreamInner {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let stream = self.stream;
            dispatch_main(move || classic::spp_close(stream));
        }
    }
}
