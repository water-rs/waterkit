//! Apple platform implementation of Bluetooth (BLE and Classic SPP).
//!
//! BLE runs through `CoreBluetooth` (`CBCentralManager`); Classic Bluetooth
//! runs through `IOBluetooth` and exists on macOS only — on iOS the Classic
//! entry points fail fast, matching the previous implementation.
//!
//! All Objective-C work happens on the main queue: the managers, delegates
//! and their callback state are main-thread-bound `define_class!` objects
//! whose ivars own the pending senders (no global/static callback registry —
//! each delegate instance owns its state). Objects never cross threads:
//! owning `*Inner` structs hold `MainThreadBound<Retained<T>>` handles that
//! are only touched on the main thread, and every hop onto the main queue is
//! `exec_async` plus a `futures` oneshot — never `exec_sync`, so nothing
//! blocks an executor thread. Results that cross back are plain `Send` data
//! or the `MainThreadBound` handle itself.

use std::collections::HashMap;
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::Mutex;

use async_channel::{Receiver, Sender};
use core::cell::RefCell;
use core::ffi::c_void;
use dispatch2::{DispatchQueue, MainThreadBound};
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
use waterkit_core::apple::on_main;

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
#[cfg(target_os = "macos")]
type StreamOwner = Arc<MainThreadBound<Retained<classic::SppStream>>>;

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
        // SAFETY: `as_bytes_unchecked` reads a live contiguous buffer.
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

pub struct CentralDelegateIvars {
    manager: RefCell<Option<Retained<CBCentralManager>>>,
    state_txs: RefCell<Vec<oneshot::Sender<AdapterState>>>,
    scan_tx: RefCell<Option<Sender<ScanResult>>>,
    connect_txs: RefCell<HashMap<String, oneshot::Sender<Result<(), BluetoothError>>>>,
    peripheral: RefCell<Option<Retained<CBPeripheral>>>,
    peripheral_delegate: RefCell<Option<Retained<PeripheralDelegate>>>,
    /// Self-retain keeping the delegate (and its manager) alive across async
    /// callbacks; released once the pending sender count hits zero.
    keep_alive: RefCell<Option<Retained<CentralDelegate>>>,
}

define_class!(
    // SAFETY: ivars are all `RefCell`/plain data touched only on the main
    // thread; the delegate object itself stays on the main queue for life.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WaterkitBleCentralDelegate"]
    #[ivars = CentralDelegateIvars]
    pub struct CentralDelegate;

    unsafe impl NSObjectProtocol for CentralDelegate {}

    unsafe impl CBCentralManagerDelegate for CentralDelegate {
        #[unsafe(method(centralManagerDidUpdateState:))]
        fn did_update_state(&self, _central: &CBCentralManager) {
            // SAFETY: `state` is a read-only accessor on a live manager.
            let state = map_state(unsafe { _central.state() });
            if state == AdapterState::Unknown {
                return;
            }
            let txs: Vec<_> = self.ivars().state_txs.borrow_mut().drain(..).collect();
            for tx in txs {
                let _ = tx.send(state);
            }
            if self.ivars().state_txs.borrow().is_empty()
                && self.ivars().connect_txs.borrow().is_empty()
            {
                *self.ivars().keep_alive.borrow_mut() = None;
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
            let Some(scan_tx) = self.ivars().scan_tx.borrow().clone() else {
                return;
            };
            // SAFETY: `identifier`/`UUIDString` are read-only accessors.
            let address = unsafe { peripheral.identifier().UUIDString() }.to_string();
            let name = unsafe { peripheral.name() }.map(|name| name.to_string());
            let service_uuids = advertisement_data
                .objectForKey(unsafe { CBAdvertisementDataServiceUUIDsKey })
                .map_or_else(Vec::new, |object| {
                    // SAFETY: `CBAdvertisementDataServiceUUIDsKey` always maps
                    // to an `NSArray<CBUUID>` in advertisement data.
                    let uuids = unsafe { &*Retained::as_ptr(&object).cast::<NSArray<CBUUID>>() };
                    uuids
                        .iter()
                        .map(|uuid| Uuid::new(unsafe { uuid.UUIDString() }.to_string()))
                        .collect()
                });
            // SAFETY: `state` is a read-only accessor.
            let is_connected = unsafe { peripheral.state() } == CBPeripheralState::Connected;
            let result = ScanResult {
                device: BluetoothDevice {
                    id: DeviceId::new(&address),
                    name,
                    rssi: Some(rssi.shortValue()),
                    is_connected,
                },
                service_uuids,
                manufacturer_data: HashMap::new(),
            };
            let _ = scan_tx.try_send(result);
        }

        #[unsafe(method(centralManager:didConnectPeripheral:))]
        fn did_connect(&self, _central: &CBCentralManager, peripheral: &CBPeripheral) {
            // SAFETY: read-only accessors.
            let key = unsafe { peripheral.identifier().UUIDString() }.to_string();
            if let Some(tx) = self.ivars().connect_txs.borrow_mut().remove(&key) {
                let _ = tx.send(Ok(()));
            }
            *self.ivars().keep_alive.borrow_mut() = None;
        }

        #[unsafe(method(centralManager:didFailToConnectPeripheral:error:))]
        fn did_fail_connect(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            let key = unsafe { peripheral.identifier().UUIDString() }.to_string();
            if let Some(tx) = self.ivars().connect_txs.borrow_mut().remove(&key) {
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(ns_error(
                    error,
                    "connection failed",
                ))));
            }
            *self.ivars().keep_alive.borrow_mut() = None;
        }
    }
);

impl CentralDelegate {
    fn spawn(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CentralDelegateIvars {
            manager: RefCell::new(None),
            state_txs: RefCell::new(Vec::new()),
            scan_tx: RefCell::new(None),
            connect_txs: RefCell::new(HashMap::new()),
            peripheral: RefCell::new(None),
            peripheral_delegate: RefCell::new(None),
            keep_alive: RefCell::new(None),
        });
        let delegate: Retained<Self> = unsafe { msg_send![super(this), init] };
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

    /// Send `tx` the adapter state — immediately if already known, else when
    /// the first `centralManagerDidUpdateState:` arrives.
    fn watch_state(&self, tx: oneshot::Sender<AdapterState>) {
        let manager = self.ivars().manager.borrow();
        let manager = manager.as_ref().expect("manager created in spawn");
        // SAFETY: `state` is a read-only accessor on a live manager.
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

struct DiscoverState {
    remaining: usize,
    services: Vec<GattService>,
    sender: oneshot::Sender<Result<Vec<GattService>, BluetoothError>>,
}

pub struct PeripheralDelegateIvars {
    discover: RefCell<Option<DiscoverState>>,
    read_txs: RefCell<HashMap<String, ReadTx>>,
    write_txs: RefCell<HashMap<String, WriteUnitTx>>,
    notify_txs: RefCell<HashMap<String, Sender<Vec<u8>>>>,
}

define_class!(
    // SAFETY: ivars are only touched through the main-queue callbacks below.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WaterkitBlePeripheralDelegate"]
    #[ivars = PeripheralDelegateIvars]
    pub struct PeripheralDelegate;

    unsafe impl NSObjectProtocol for PeripheralDelegate {}

    unsafe impl CBPeripheralDelegate for PeripheralDelegate {
        #[unsafe(method(peripheralDidDiscoverServices:))]
        fn did_discover_services(&self, peripheral: &CBPeripheral, error: Option<&NSError>) {
            if let Some(error) = error {
                self.finish_discover(Err(BluetoothError::GattError(
                    error.localizedDescription().to_string(),
                )));
                return;
            }
            // SAFETY: read-only accessor.
            let services = unsafe { peripheral.services() }.unwrap_or_default();
            if services.is_empty() {
                self.finish_discover(Ok(Vec::new()));
                return;
            }
            if let Some(discover) = self.ivars().discover.borrow_mut().as_mut() {
                discover.remaining = services.len();
            }
            for service in &services {
                // SAFETY: kicks off characteristic discovery on a live
                // discovered service; results return on this delegate.
                unsafe {
                    peripheral.discoverCharacteristics_forService(None, &service);
                }
            }
        }

        #[unsafe(method(peripheral:didDiscoverCharacteristicsForService:error:))]
        fn did_discover_characteristics(
            &self,
            _peripheral: &CBPeripheral,
            service: &CBService,
            error: Option<&NSError>,
        ) {
            if error.is_some() {
                // The previous implementation ignored the per-service error
                // and still counted the service's (empty) characteristic set.
                // Keep that behaviour.
            }
            let service = gatt_service(service);
            let done = {
                let mut slot = self.ivars().discover.borrow_mut();
                if let Some(discover) = slot.as_mut() {
                    discover.services.push(service);
                    discover.remaining = discover.remaining.saturating_sub(1);
                    discover.remaining == 0
                } else {
                    false
                }
            };
            if done && let Some(discover) = self.ivars().discover.borrow_mut().take() {
                let _ = discover.sender.send(Ok(discover.services));
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
    fn spawn(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PeripheralDelegateIvars {
            discover: RefCell::new(None),
            read_txs: RefCell::new(HashMap::new()),
            write_txs: RefCell::new(HashMap::new()),
            notify_txs: RefCell::new(HashMap::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn finish_discover(&self, result: Result<Vec<GattService>, BluetoothError>) {
        if let Some(discover) = self.ivars().discover.borrow_mut().take() {
            let _ = discover.sender.send(result);
        }
    }
}

// ---------------------------------------------------------------------------
// Adapter state / scanning / connections (public entry points)

/// One-shot probe: spawn a temporary central delegate, ask it for the first
/// known adapter state, then release it.
pub async fn adapter_state() -> Result<AdapterState, BluetoothError> {
    let (delegate, rx) = on_main(|mtm| {
        let delegate = CentralDelegate::spawn(mtm);
        let (tx, rx) = oneshot::channel();
        delegate.watch_state(tx);
        (MainThreadBound::new(delegate, mtm), rx)
    })
    .await;
    let state = rx
        .await
        .map_err(|_| BluetoothError::Platform("adapter state callback dropped".into()))?;
    // Release the delegate on the main queue where it lives.
    DispatchQueue::main().exec_async(move || drop(delegate));
    Ok(state)
}

pub struct BleScannerInner {
    delegate: Arc<MainThreadBound<Retained<CentralDelegate>>>,
    pub(crate) scan_rx: Receiver<ScanResult>,
}

impl core::fmt::Debug for BleScannerInner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BleScannerInner")
            .field("scan_rx", &self.scan_rx)
            .finish_non_exhaustive()
    }
}

impl BleScannerInner {
    /// Fail fast unless the adapter is powered on.
    pub async fn new() -> Result<Self, BluetoothError> {
        let (delegate, state_rx) = on_main(|mtm| {
            let delegate = CentralDelegate::spawn(mtm);
            // The delegate must outlive the adapter-state wait.
            *delegate.ivars().keep_alive.borrow_mut() = Some(delegate.clone());
            let (tx, rx) = oneshot::channel();
            delegate.watch_state(tx);
            (Arc::new(MainThreadBound::new(delegate, mtm)), rx)
        })
        .await;
        let state = state_rx
            .await
            .map_err(|_| BluetoothError::Platform("adapter state callback dropped".into()))?;
        if state != AdapterState::PoweredOn {
            DispatchQueue::main().exec_async(move || drop(delegate));
            return Err(BluetoothError::NotAvailable);
        }
        let (scan_tx, scan_rx) = async_channel::bounded(64);
        let central = Arc::clone(&delegate);
        on_main(move |mtm| {
            central.get(mtm).ivars().scan_tx.replace(Some(scan_tx));
        })
        .await;
        Ok(Self { delegate, scan_rx })
    }

    /// Begin scanning; returns the stream of deduplicated results.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "result kept for parity with fallible platform impls"
    )]
    pub fn start_scan(&self, filter: &ScanFilter) -> Result<Receiver<ScanResult>, BluetoothError> {
        let delegate = Arc::clone(&self.delegate);
        let service_uuids: Vec<String> = filter
            .service_uuids
            .iter()
            .map(|uuid| uuid.as_str().to_string())
            .collect();
        DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("on the main queue");
            let delegate = delegate.get(mtm);
            let manager = delegate.ivars().manager.borrow();
            let manager = manager.as_ref().expect("manager created in spawn");
            // SAFETY: `UUIDWithString` builds a CBUUID from a known-form
            // string (the `Uuid` newtype guarantees the format).
            let services: Option<Retained<NSArray<CBUUID>>> = if service_uuids.is_empty() {
                None
            } else {
                let uuids: Vec<Retained<CBUUID>> = service_uuids
                    .iter()
                    .map(|uuid| unsafe { CBUUID::UUIDWithString(&NSString::from_str(uuid)) })
                    .collect();
                Some(NSArray::from_retained_slice(&uuids))
            };
            // Discoveries are de-duplicated by the scan options below.
            let allow_duplicates = NSNumber::numberWithBool(false);
            let options: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
                &[unsafe { CBCentralManagerScanOptionAllowDuplicatesKey }],
                &[&***allow_duplicates],
            );
            // SAFETY: CoreBluetooth scan entry point on a powered-on manager.
            unsafe {
                manager.scanForPeripheralsWithServices_options(services.as_deref(), Some(&options));
            }
        });
        Ok(self.scan_rx.clone())
    }

    /// Stop scanning and release the manager (the delegate is dropped on the
    /// main queue where it lives).
    pub fn stop_scan(&self) {
        let delegate = Arc::clone(&self.delegate);
        DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("on the main queue");
            let delegate = delegate.get(mtm);
            delegate.ivars().scan_tx.borrow_mut().take();
            let manager = delegate.ivars().manager.borrow();
            if let Some(manager) = manager.as_ref() {
                // SAFETY: `stopScan` on a live manager.
                unsafe { manager.stopScan() };
            }
        });
    }
}

impl Drop for BleScannerInner {
    fn drop(&mut self) {
        let delegate = Arc::clone(&self.delegate);
        // The delegate (and its manager) is released on the main queue.
        DispatchQueue::main().exec_async(move || drop(delegate));
    }
}

pub struct BleConnectionInner {
    device_id: DeviceId,
    delegate: Arc<MainThreadBound<Retained<CentralDelegate>>>,
}

impl core::fmt::Debug for BleConnectionInner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BleConnectionInner")
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

impl BleConnectionInner {
    /// Look the peripheral up by identifier and drive `connectPeripheral` on
    /// the main queue; resolves once `didConnect` fires.
    pub async fn connect(device_id: &DeviceId) -> Result<Self, BluetoothError> {
        let id = device_id.as_str().to_string();
        let (delegate, state_rx) = on_main(move |mtm| {
            let delegate = CentralDelegate::spawn(mtm);
            let device_id_string = id;
            let ns = NSString::from_str(&device_id_string);
            let ns_uuid = NSUUID::initWithUUIDString(
                // SAFETY: `alloc` on a live class.
                unsafe { msg_send![NSUUID::class(), alloc] },
                &ns,
            );
            let Some(ns_uuid) = ns_uuid else {
                return Err(BluetoothError::DeviceNotFound(device_id_string));
            };
            {
                let manager = delegate.ivars().manager.borrow();
                let manager = manager.as_ref().expect("manager created in spawn");
                let identifiers = NSArray::from_retained_slice(&[ns_uuid]);
                // SAFETY: lookup on a live manager.
                let peripherals =
                    unsafe { manager.retrievePeripheralsWithIdentifiers(&identifiers) };
                let Some(peripheral) = peripherals.firstObject() else {
                    return Err(BluetoothError::DeviceNotFound(device_id_string));
                };
                *delegate.ivars().peripheral.borrow_mut() = Some(peripheral.clone());
                let periph_delegate = PeripheralDelegate::spawn(mtm);
                // SAFETY: `setDelegate` on a live peripheral; the delegate
                // object is retained by the ivar below.
                unsafe {
                    peripheral.setDelegate(Some(ProtocolObject::from_ref(&*periph_delegate)));
                };
                *delegate.ivars().peripheral_delegate.borrow_mut() = Some(periph_delegate);
            }
            let (tx, rx) = oneshot::channel();
            delegate.watch_state(tx);
            Ok((Arc::new(MainThreadBound::new(delegate, mtm)), rx))
        })
        .await?;
        let state = state_rx
            .await
            .map_err(|_| BluetoothError::Platform("adapter state callback dropped".into()))?;
        match state {
            AdapterState::PoweredOn => {}
            AdapterState::PoweredOff => {
                DispatchQueue::main().exec_async(move || drop(delegate));
                return Err(BluetoothError::PoweredOff);
            }
            _ => {
                DispatchQueue::main().exec_async(move || drop(delegate));
                return Err(BluetoothError::NotAvailable);
            }
        }
        let central = Arc::clone(&delegate);
        let key = device_id.as_str().to_string();
        let (tx, rx) = oneshot::channel();
        on_main(move |mtm| {
            let delegate = central.get(mtm);
            delegate.ivars().connect_txs.borrow_mut().insert(key, tx);
            let manager = delegate.ivars().manager.borrow();
            let manager = manager.as_ref().expect("manager created in spawn");
            let peripheral = delegate.ivars().peripheral.borrow();
            let peripheral = peripheral.as_ref().expect("peripheral stored at connect");
            // SAFETY: `connectPeripheral` on a live manager+peripheral pair;
            // the delegate self-retains until `didConnect`/`didFailToConnect`
            // fires.
            *delegate.ivars().keep_alive.borrow_mut() = Some(delegate.clone());
            unsafe { manager.connectPeripheral_options(peripheral, None) };
        })
        .await;
        rx.await
            .map_err(|_| BluetoothError::ConnectionFailed("callback dropped".into()))??;
        Ok(Self {
            device_id: device_id.clone(),
            delegate,
        })
    }

    /// Discover GATT services (and characteristics) via the peripheral
    /// delegate's countdown — resolves when every service answered.
    pub async fn discover_services(&self) -> Result<Vec<GattService>, BluetoothError> {
        let central = Arc::clone(&self.delegate);
        let (tx, rx) = oneshot::channel();
        let found = on_main(move |mtm| {
            let delegate = central.get(mtm);
            let peripheral = delegate.ivars().peripheral.borrow();
            let periph_delegate = delegate.ivars().peripheral_delegate.borrow();
            let (Some(peripheral), Some(periph_delegate)) =
                (peripheral.as_ref(), periph_delegate.as_ref())
            else {
                return false;
            };
            *periph_delegate.ivars().discover.borrow_mut() = Some(DiscoverState {
                remaining: 0,
                services: Vec::new(),
                sender: tx,
            });
            // SAFETY: `discoverServices` on a connected peripheral.
            unsafe { peripheral.discoverServices(None) };
            true
        })
        .await;
        if !found {
            return Err(BluetoothError::GattError("peripheral unavailable".into()));
        }
        rx.await
            .map_err(|_| BluetoothError::GattError("callback dropped".into()))?
    }

    /// Run `op` against the characteristic on the main queue; fails fast when
    /// the peripheral or characteristic is missing.
    fn with_characteristic(
        central: Arc<MainThreadBound<Retained<CentralDelegate>>>,
        service: &Uuid,
        characteristic: &Uuid,
        op: impl FnOnce(&PeripheralDelegate, &CBPeripheral, Retained<CBCharacteristic>) + Send + 'static,
    ) {
        let service = service.as_str().to_string();
        let characteristic = characteristic.as_str().to_string();
        DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("on the main queue");
            let delegate = central.get(mtm);
            let periph_delegate = delegate.ivars().peripheral_delegate.borrow();
            let peripheral = delegate.ivars().peripheral.borrow();
            let (Some(periph_delegate), Some(peripheral)) =
                (periph_delegate.as_ref(), peripheral.as_ref())
            else {
                return;
            };
            if let Some(characteristic) = find_characteristic(peripheral, &service, &characteristic)
            {
                op(periph_delegate, peripheral, characteristic);
            }
        });
    }

    /// Prime the read oneshot and kick `readValueForCharacteristic`.
    pub async fn read_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<Vec<u8>, BluetoothError> {
        let central = Arc::clone(&self.delegate);
        let service_uuid = service.as_str().to_string();
        let characteristic_uuid = characteristic.as_str().to_string();
        let (tx, rx) = oneshot::channel();
        on_main(move |mtm| {
            let delegate = central.get(mtm);
            let periph_delegate = delegate.ivars().peripheral_delegate.borrow();
            let peripheral = delegate.ivars().peripheral.borrow();
            let (Some(periph_delegate), Some(peripheral)) =
                (periph_delegate.as_ref(), peripheral.as_ref())
            else {
                return Err(BluetoothError::GattError("peripheral unavailable".into()));
            };
            let Some(characteristic) =
                find_characteristic(peripheral, &service_uuid, &characteristic_uuid)
            else {
                return Err(BluetoothError::GattError("characteristic not found".into()));
            };
            periph_delegate
                .ivars()
                .read_txs
                .borrow_mut()
                .insert(characteristic_key(&characteristic), tx);
            // SAFETY: `readValueForCharacteristic` on a live characteristic.
            unsafe { peripheral.readValueForCharacteristic(&characteristic) };
            Ok(())
        })
        .await?;
        rx.await
            .map_err(|_| BluetoothError::GattError("callback dropped".into()))?
    }

    /// Queue the write oneshot and kick `writeValue:forCharacteristic:type:`.
    pub async fn write_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
        data: &[u8],
    ) -> Result<(), BluetoothError> {
        let central = Arc::clone(&self.delegate);
        let service_uuid = service.as_str().to_string();
        let characteristic_uuid = characteristic.as_str().to_string();
        let payload = data.to_vec();
        let (tx, rx) = oneshot::channel();
        on_main(move |mtm| -> Result<(), BluetoothError> {
            let delegate = central.get(mtm);
            let periph_delegate = delegate.ivars().peripheral_delegate.borrow();
            let peripheral = delegate.ivars().peripheral.borrow();
            let (Some(periph_delegate), Some(peripheral)) =
                (periph_delegate.as_ref(), peripheral.as_ref())
            else {
                return Err(BluetoothError::GattError("peripheral unavailable".into()));
            };
            let Some(characteristic) =
                find_characteristic(peripheral, &service_uuid, &characteristic_uuid)
            else {
                return Err(BluetoothError::GattError("characteristic not found".into()));
            };
            periph_delegate
                .ivars()
                .write_txs
                .borrow_mut()
                .insert(characteristic_key(&characteristic), tx);
            // SAFETY: `dataWithBytes` copies `payload` immediately.
            let ns_data = unsafe {
                NSData::dataWithBytes_length(payload.as_ptr().cast::<c_void>(), payload.len())
            };
            // `WithResponse` matches the previous behaviour.
            unsafe {
                peripheral.writeValue_forCharacteristic_type(
                    &ns_data,
                    &characteristic,
                    CBCharacteristicWriteType::WithResponse,
                );
            }
            Ok(())
        })
        .await?;
        rx.await
            .map_err(|_| BluetoothError::GattError("callback dropped".into()))?
    }

    /// Register a notify channel and enable notifications on the
    /// characteristic.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "fallible for parity with the other platform impls"
    )]
    pub fn subscribe(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<Receiver<Vec<u8>>, BluetoothError> {
        let central = Arc::clone(&self.delegate);
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
        );
        Ok(rx)
    }

    /// Cancel the connection and release the central delegate on the main
    /// queue.
    pub fn disconnect(self) {
        let delegate = Arc::clone(&self.delegate);
        DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("on the main queue");
            let delegate = delegate.get(mtm);
            let manager = delegate.ivars().manager.borrow();
            let peripheral = delegate.ivars().peripheral.borrow();
            if let (Some(manager), Some(peripheral)) = (manager.as_ref(), peripheral.as_ref()) {
                // SAFETY: `cancelPeripheralConnection` on a live pair.
                unsafe { manager.cancelPeripheralConnection(peripheral) };
            }
            // `delegate` (Arc<MainThreadBound>) drops at the end of this
            // closure, releasing the object on the main queue.
        });
    }
}

impl Drop for BleConnectionInner {
    fn drop(&mut self) {
        let delegate = Arc::clone(&self.delegate);
        DispatchQueue::main().exec_async(move || drop(delegate));
    }
}

// ---------------------------------------------------------------------------
// Classic Bluetooth (macOS only)
#[cfg(target_os = "macos")]
mod classic {
    use super::{
        BluetoothDevice, BluetoothError, ClassicDevice, DeviceId, HashMap, K_IO_RETURN_SUCCESS,
        PendingRead, PendingWrite, ReadTx, RefCell, Retained, Sender, StreamOwner, Uuid, WriteTx,
        oneshot,
    };
    use core::ffi::c_void;
    use dispatch2::MainThreadBound;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{
        ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    };
    use objc2_foundation::{NSArray, NSData, NSObjectProtocol, NSString};
    use objc2_io_bluetooth::{
        BluetoothRFCOMMChannelID, IOBluetoothDevice, IOBluetoothDeviceAsyncCallbacks,
        IOBluetoothDeviceInquiry, IOBluetoothDeviceInquiryDelegate,
        IOBluetoothDeviceSearchTypesBits, IOBluetoothRFCOMMChannel,
        IOBluetoothRFCOMMChannelDelegate, IOBluetoothSDPUUID,
    };
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::sync::Arc;

    fn classic_device(device: &IOBluetoothDevice) -> ClassicDevice {
        // SAFETY: read-only accessors on a live device object.
        let (address, name, class_of_device, connected, paired) = unsafe {
            (
                device
                    .addressString()
                    .expect("a discovered/paired device always has an address")
                    .to_string(),
                Some(device.name().to_string()),
                device.classOfDevice(),
                device.isConnected(),
                device.isPaired(),
            )
        };
        ClassicDevice {
            device: BluetoothDevice {
                id: DeviceId::new(&address),
                name,
                rssi: None,
                is_connected: connected,
            },
            device_class: class_of_device,
            is_paired: paired,
        }
    }

    pub struct InquiryIvars {
        inquiry: RefCell<Option<Retained<IOBluetoothDeviceInquiry>>>,
        tx: RefCell<Option<Sender<ClassicDevice>>>,
    }

    define_class!(
        // SAFETY: main-thread-only like every other delegate here.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WaterkitClassicInquiry"]
        #[ivars = InquiryIvars]
        pub struct Inquiry;

        unsafe impl NSObjectProtocol for Inquiry {}

        unsafe impl IOBluetoothDeviceInquiryDelegate for Inquiry {
            #[unsafe(method(deviceInquiryDeviceFound:device:))]
            fn device_found(&self, _sender: &IOBluetoothDeviceInquiry, device: &IOBluetoothDevice) {
                if let Some(tx) = self.ivars().tx.borrow().as_ref() {
                    let _ = tx.try_send(classic_device(device));
                }
            }
        }
    );

    impl Inquiry {
        fn spawn(mtm: MainThreadMarker, tx: Sender<ClassicDevice>) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(InquiryIvars {
                inquiry: RefCell::new(None),
                tx: RefCell::new(Some(tx)),
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    /// Start a classic-device inquiry on the main thread.
    ///
    /// Returns the found-device stream; the inquiry lives in
    /// `ClassicBluetoothInner`'s `MainThreadBound` slot until stopped.
    pub fn start_discovery(
        mtm: MainThreadMarker,
        tx: Sender<ClassicDevice>,
    ) -> Result<MainThreadBound<Retained<Inquiry>>, BluetoothError> {
        let delegate = Inquiry::spawn(mtm, tx);
        let inquiry = unsafe {
            IOBluetoothDeviceInquiry::initWithDelegate(
                msg_send![IOBluetoothDeviceInquiry::class(), alloc],
                Some(&***delegate),
            )
        };
        let Some(inquiry) = inquiry else {
            return Err(BluetoothError::Platform(
                "IOBluetoothDeviceInquiry init failed".into(),
            ));
        };
        // SAFETY: classic-only inquiry search configuration.
        unsafe {
            inquiry.setSearchType(IOBluetoothDeviceSearchTypesBits::Classic.0);
            inquiry.setUpdateNewDeviceNames(false);
        }
        *delegate.ivars().inquiry.borrow_mut() = Some(inquiry.clone());
        // SAFETY: `start` begins an inquiry on a fully configured object.
        let status = unsafe { inquiry.start() };
        if status != K_IO_RETURN_SUCCESS {
            return Err(BluetoothError::Platform(format!(
                "IOBluetoothDeviceInquiry start failed ({status})"
            )));
        }
        Ok(MainThreadBound::new(delegate, mtm))
    }

    /// Extract a `ClassicDevice` for each paired `IOBluetoothDevice`.
    ///
    /// # Safety
    /// Calls `pairedDevices` on `IOBluetoothDevice` — run on the main thread.
    pub fn paired_devices() -> Vec<ClassicDevice> {
        // SAFETY: `pairedDevices` enumerates the system's paired devices.
        let Some(devices) = (unsafe { IOBluetoothDevice::pairedDevices() }) else {
            return Vec::new();
        };
        devices
            .iter()
            .map(|d| {
                let device = d
                    .downcast::<IOBluetoothDevice>()
                    .expect("pairedDevices returns IOBluetoothDevice objects");
                classic_device(&device)
            })
            .collect()
    }

    /// SPP connector: runs `performSDPQuery` to learn the RFCOMM channel, then
    /// opens the channel; its own RFCOMM delegate methods resolve the pending
    /// connect sender.
    pub struct ConnectorIvars {
        sdp_uuid: RefCell<Option<Retained<IOBluetoothSDPUUID>>>,
        tx: RefCell<Option<oneshot::Sender<Result<StreamOwner, BluetoothError>>>>,
        channel: RefCell<Option<Retained<IOBluetoothRFCOMMChannel>>>,
        keep_alive: RefCell<Option<Retained<Connector>>>,
    }

    define_class!(
        // SAFETY: main-thread-only like every other delegate here.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WaterkitSppConnector"]
        #[ivars = ConnectorIvars]
        pub struct Connector;

        unsafe impl NSObjectProtocol for Connector {}

        unsafe impl IOBluetoothDeviceAsyncCallbacks for Connector {
            #[unsafe(method(sdpQueryComplete:status:))]
            fn sdp_query_complete(&self, _device: &IOBluetoothDevice, status: i32) {
                if status != K_IO_RETURN_SUCCESS {
                    self.finish(Err(BluetoothError::ConnectionFailed(format!(
                        "SDP query failed ({status})"
                    ))));
                    return;
                }
                let mtm = MainThreadMarker::new().expect("on the main queue");
                let uuid = self.ivars().sdp_uuid.borrow().clone();
                let Some(uuid) = uuid else {
                    self.finish(Err(BluetoothError::Platform("missing SDP UUID".into())));
                    return;
                };
                // SAFETY: `getServiceRecordForUUID` on a completed query.
                let record = unsafe { _device.getServiceRecordForUUID(Some(&uuid)) };
                let Some(record) = record else {
                    self.finish(Err(BluetoothError::ConnectionFailed(
                        "SDP service record not found".into(),
                    )));
                    return;
                };
                let mut channel_id: BluetoothRFCOMMChannelID = 0;
                // SAFETY: writes the channel id into `channel_id`.
                let status = unsafe { record.getRFCOMMChannelID(&raw mut channel_id) };
                if status != K_IO_RETURN_SUCCESS || channel_id == 0 {
                    self.finish(Err(BluetoothError::ConnectionFailed(
                        "no RFCOMM channel in SDP record".into(),
                    )));
                    return;
                }
                let mut channel: Option<Retained<IOBluetoothRFCOMMChannel>> = None;
                // SAFETY: opens an RFCOMM channel; `self` is the delegate for
                // the open-complete/close callbacks below.
                let status = unsafe {
                    _device.openRFCOMMChannelAsync_withChannelID_delegate(
                        Some(&mut channel),
                        channel_id,
                        Some(&***self),
                    )
                };
                if status != K_IO_RETURN_SUCCESS {
                    self.finish(Err(BluetoothError::ConnectionFailed(format!(
                        "openRFCOMMChannelAsync failed ({status})"
                    ))));
                    return;
                }
                *self.ivars().channel.borrow_mut() = channel;
                let _ = mtm;
            }
        }

        unsafe impl IOBluetoothRFCOMMChannelDelegate for Connector {
            #[unsafe(method(rfcommChannelOpenComplete:status:))]
            fn open_complete(&self, _channel: &IOBluetoothRFCOMMChannel, status: i32) {
                if status != K_IO_RETURN_SUCCESS {
                    self.finish(Err(BluetoothError::ConnectionFailed(format!(
                        "RFCOMM channel open failed ({status})"
                    ))));
                    return;
                }
                let mtm = MainThreadMarker::new().expect("on the main queue");
                let channel = self.ivars().channel.borrow_mut().take();
                let Some(channel) = channel else {
                    self.finish(Err(BluetoothError::Platform(
                        "channel missing on open".into(),
                    )));
                    return;
                };
                let stream = SppStream::spawn(mtm, channel);
                // SAFETY: `setDelegate` keeps `self` informed; the stream is
                // handed to the owner through the oneshot.
                let status = unsafe {
                    stream
                        .ivars()
                        .channel
                        .borrow()
                        .as_ref()
                        .expect("channel stored at spawn")
                        .setDelegate(Some(&***stream))
                };
                if status != K_IO_RETURN_SUCCESS {
                    self.finish(Err(BluetoothError::ConnectionFailed(format!(
                        "setDelegate failed ({status})"
                    ))));
                    return;
                }
                self.finish(Ok(Arc::new(MainThreadBound::new(stream, mtm))));
            }

            #[unsafe(method(rfcommChannelClosed:))]
            fn channel_closed(&self, _channel: &IOBluetoothRFCOMMChannel) {
                self.finish(Err(BluetoothError::ConnectionFailed(
                    "RFCOMM channel closed".into(),
                )));
            }

            #[unsafe(method(rfcommChannelData:data:length:))]
            fn did_receive(
                &self,
                _channel: &IOBluetoothRFCOMMChannel,
                _data: *mut c_void,
                _length: usize,
            ) {
            }

            #[unsafe(method(rfcommChannelWriteComplete:refcon:))]
            fn write_complete(&self, _channel: &IOBluetoothRFCOMMChannel, _refcon: *mut c_void) {}

            #[unsafe(method(rfcommChannelWriteComplete:refcon:status:))]
            fn write_complete_status(
                &self,
                _channel: &IOBluetoothRFCOMMChannel,
                _refcon: *mut c_void,
                _status: i32,
            ) {
            }

            #[unsafe(method(rfcommChannelWriteComplete:refcon:status:bytesWritten:))]
            fn write_complete_bytes(
                &self,
                _channel: &IOBluetoothRFCOMMChannel,
                _refcon: *mut c_void,
                _status: i32,
                _bytes_written: usize,
            ) {
            }
        }
    );

    impl Connector {
        fn spawn(
            mtm: MainThreadMarker,
            sdp_uuid: Retained<IOBluetoothSDPUUID>,
            tx: oneshot::Sender<Result<StreamOwner, BluetoothError>>,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ConnectorIvars {
                sdp_uuid: RefCell::new(Some(sdp_uuid)),
                tx: RefCell::new(Some(tx)),
                channel: RefCell::new(None),
                keep_alive: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        /// Deliver the connect result and release the self-retain that kept
        /// the connector alive across the async callbacks.
        fn finish(&self, result: Result<StreamOwner, BluetoothError>) {
            if let Some(tx) = self.ivars().tx.borrow_mut().take() {
                let _ = tx.send(result);
            }
            *self.ivars().keep_alive.borrow_mut() = None;
        }
    }

    /// Live RFCOMM stream; delegate callbacks run on the main queue where the
    /// channel was opened.
    pub struct SppStreamIvars {
        channel: RefCell<Option<Retained<IOBluetoothRFCOMMChannel>>>,
        buffer: RefCell<Vec<u8>>,
        pending_reads: RefCell<VecDeque<PendingRead>>,
        pending_writes: RefCell<HashMap<usize, PendingWrite>>,
        next_write: Cell<usize>,
        closed: Cell<bool>,
    }

    define_class!(
        // SAFETY: main-thread-only like every other delegate here.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WaterkitSppStream"]
        #[ivars = SppStreamIvars]
        pub struct SppStream;

        unsafe impl NSObjectProtocol for SppStream {}

        unsafe impl IOBluetoothRFCOMMChannelDelegate for SppStream {
            #[unsafe(method(rfcommChannelData:data:length:))]
            fn did_receive(
                &self,
                _channel: &IOBluetoothRFCOMMChannel,
                data: *mut c_void,
                length: usize,
            ) {
                // SAFETY: IOBluetooth guarantees `data` points at `length`
                // valid bytes for the duration of this callback.
                let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
                self.ivars().buffer.borrow_mut().extend_from_slice(bytes);
                self.drain_reads();
            }

            #[unsafe(method(rfcommChannelOpenComplete:status:))]
            fn open_complete(&self, _channel: &IOBluetoothRFCOMMChannel, status: i32) {
                if status != K_IO_RETURN_SUCCESS {
                    self.ivars().closed.set(true);
                    self.fail_all(&BluetoothError::ConnectionFailed(format!(
                        "RFCOMM channel open failed ({status})"
                    )));
                }
            }

            #[unsafe(method(rfcommChannelClosed:))]
            fn channel_closed(&self, _channel: &IOBluetoothRFCOMMChannel) {
                self.ivars().closed.set(true);
                self.fail_all(&BluetoothError::ConnectionFailed(
                    "RFCOMM channel closed".into(),
                ));
            }

            #[unsafe(method(rfcommChannelWriteComplete:refcon:status:))]
            fn write_complete(
                &self,
                _channel: &IOBluetoothRFCOMMChannel,
                refcon: *mut c_void,
                status: i32,
            ) {
                self.finish_write(refcon, status, None);
            }

            #[unsafe(method(rfcommChannelWriteComplete:refcon:status:bytesWritten:))]
            fn write_complete_bytes(
                &self,
                _channel: &IOBluetoothRFCOMMChannel,
                refcon: *mut c_void,
                status: i32,
                bytes_written: usize,
            ) {
                self.finish_write(refcon, status, Some(bytes_written));
            }
        }
    );

    impl SppStream {
        fn spawn(
            mtm: MainThreadMarker,
            channel: Retained<IOBluetoothRFCOMMChannel>,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(SppStreamIvars {
                channel: RefCell::new(Some(channel)),
                buffer: RefCell::new(Vec::new()),
                pending_reads: RefCell::new(VecDeque::new()),
                pending_writes: RefCell::new(HashMap::new()),
                next_write: Cell::new(0),
                closed: Cell::new(false),
            });
            unsafe { msg_send![super(this), init] }
        }

        /// Serve buffered data to pending reads, in order.
        fn drain_reads(&self) {
            loop {
                let mut buffer = self.ivars().buffer.borrow_mut();
                let mut pending = self.ivars().pending_reads.borrow_mut();
                let Some(&(max, _)) = pending.front() else {
                    return;
                };
                if buffer.is_empty() {
                    return;
                }
                let take = buffer.len().min(max);
                let chunk: Vec<u8> = buffer.drain(..take).collect();
                let (_, tx) = pending.pop_front().expect("front read exists");
                drop(pending);
                drop(buffer);
                let _ = tx.send(Ok(chunk));
            }
        }

        /// Queue a read of up to `max` bytes; serves from the buffer first.
        fn enqueue_read(&self, max: usize, tx: ReadTx) {
            if self.ivars().closed.get() {
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(
                    "RFCOMM channel closed".into(),
                )));
                return;
            }
            let mut buffer = self.ivars().buffer.borrow_mut();
            if buffer.is_empty() {
                drop(buffer);
                self.ivars().pending_reads.borrow_mut().push_back((max, tx));
            } else {
                let take = buffer.len().min(max);
                let chunk: Vec<u8> = buffer.drain(..take).collect();
                drop(buffer);
                let _ = tx.send(Ok(chunk));
            }
        }

        /// Start an async RFCOMM write; the token key is the API's `refcon`.
        fn enqueue_write(&self, data: &[u8], tx: WriteTx) {
            if self.ivars().closed.get() {
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(
                    "RFCOMM channel closed".into(),
                )));
                return;
            }
            let Ok(length) = u16::try_from(data.len()) else {
                let _ = tx.send(Err(BluetoothError::GattError(
                    "write payload exceeds RFCOMM limits".into(),
                )));
                return;
            };
            // SAFETY: `dataWithBytes` copies the payload immediately.
            let ns_data =
                unsafe { NSData::dataWithBytes_length(data.as_ptr().cast::<c_void>(), data.len()) };
            let token = self.ivars().next_write.get();
            self.ivars().next_write.set(token + 1);
            self.ivars()
                .pending_writes
                .borrow_mut()
                .insert(token, (tx, ns_data));
            let pending = self.ivars().pending_writes.borrow();
            let (_, ns_data) = pending.get(&token).expect("write just inserted");
            let status = unsafe {
                self.ivars()
                    .channel
                    .borrow()
                    .as_ref()
                    .expect("channel stored at spawn")
                    .writeAsync_length_refcon(
                        // SAFETY: the NSData stays retained in
                        // `pending_writes` until `writeComplete` fires.
                        ns_data
                            .as_bytes_unchecked()
                            .as_ptr()
                            .cast::<c_void>()
                            .cast_mut(),
                        length,
                        token as *mut c_void,
                    )
            };
            drop(pending);
            if status != K_IO_RETURN_SUCCESS {
                let (tx, _) = self
                    .ivars()
                    .pending_writes
                    .borrow_mut()
                    .remove(&token)
                    .expect("write just inserted");
                let _ = tx.send(Err(BluetoothError::ConnectionFailed(format!(
                    "writeAsync failed ({status})"
                ))));
            }
        }

        /// Resolve the write whose `refcon` token just completed.
        fn finish_write(&self, refcon: *mut c_void, status: i32, bytes_written: Option<usize>) {
            let token = refcon as usize;
            let Some((tx, data)) = self.ivars().pending_writes.borrow_mut().remove(&token) else {
                return;
            };
            let result = if status == K_IO_RETURN_SUCCESS {
                Ok(bytes_written.unwrap_or_else(|| data.length()))
            } else {
                Err(BluetoothError::ConnectionFailed(format!(
                    "RFCOMM write failed ({status})"
                )))
            };
            let _ = tx.send(result);
        }

        /// Fail every pending read/write with `error`'s message.
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
            let status =
                self.ivars()
                    .channel
                    .borrow_mut()
                    .take()
                    .map_or(K_IO_RETURN_SUCCESS, |channel| {
                        // SAFETY: `closeChannel` on a live channel.
                        unsafe { channel.closeChannel() }
                    });
            self.fail_all(&BluetoothError::ConnectionFailed(
                "RFCOMM channel closed".into(),
            ));
            if status == K_IO_RETURN_SUCCESS {
                Ok(())
            } else {
                Err(BluetoothError::Platform(format!(
                    "closeChannel failed ({status})"
                )))
            }
        }
    }

    /// Parse a 16/32/128-bit hex SPP UUID into an `IOBluetoothSDPUUID`.
    pub fn parse_sdp_uuid(uuid: &str) -> Option<Retained<IOBluetoothSDPUUID>> {
        let hex: String = uuid.chars().filter(char::is_ascii_hexdigit).collect();
        let bytes = match hex.len() {
            4 => u16::from_str_radix(&hex, 16).ok()?.to_be_bytes().to_vec(),
            8 => u32::from_str_radix(&hex, 16).ok()?.to_be_bytes().to_vec(),
            32 => {
                let mut out = Vec::with_capacity(16);
                for i in (0..32).step_by(2) {
                    out.push(u8::from_str_radix(&hex[i..i + 2], 16).ok()?);
                }
                out
            }
            _ => return None,
        };
        // SAFETY: `uuidWithBytes` copies `bytes` immediately.
        unsafe {
            IOBluetoothSDPUUID::uuidWithBytes_length(
                bytes.as_ptr().cast::<c_void>(),
                u32::try_from(bytes.len()).expect("uuid length fits u32"),
            )
        }
    }

    /// Kick an SPP connect on the main queue: SDP query for the UUID, then
    /// RFCOMM open — the connector resolves the pending oneshot.
    ///
    /// # Safety
    /// Calls `deviceWithAddressString`/`performSDPQuery` — main thread only.
    pub fn connect_spp(
        mtm: MainThreadMarker,
        device_id: &DeviceId,
        uuid: &Uuid,
        tx: oneshot::Sender<Result<StreamOwner, BluetoothError>>,
    ) {
        let address = device_id.as_str().to_string();
        let uuid_string = uuid.as_str().to_string();
        let ns_address = NSString::from_str(&address);
        // SAFETY: `deviceWithAddressString` returns nil for unknown devices.
        let device = unsafe { IOBluetoothDevice::deviceWithAddressString(Some(&ns_address)) };
        let Some(device) = device else {
            let _ = tx.send(Err(BluetoothError::ConnectionFailed(format!(
                "Classic Bluetooth device not found ({address})"
            ))));
            return;
        };
        let Some(sdp_uuid) = parse_sdp_uuid(&uuid_string) else {
            let _ = tx.send(Err(BluetoothError::Platform(format!(
                "Invalid SPP UUID ({uuid_string})"
            ))));
            return;
        };
        let connector = Connector::spawn(mtm, sdp_uuid, tx);
        *connector.ivars().keep_alive.borrow_mut() = Some(connector.clone());
        // SAFETY: `Retained::cast` performs a checked-class upcast —
        // every `IOBluetoothSDPUUID` is an `AnyObject`.
        let uuids = NSArray::from_retained_slice(&[unsafe {
            Retained::cast_unchecked::<AnyObject>(
                connector
                    .ivars()
                    .sdp_uuid
                    .borrow()
                    .clone()
                    .expect("uuid stored at spawn"),
            )
        }]);
        // SAFETY: `performSDPQuery` kicks the SDP query; `self` (the
        // connector) is the async-callback target on the main queue.
        let status = unsafe { device.performSDPQuery_uuids(Some(&***connector), Some(&uuids)) };
        if status != K_IO_RETURN_SUCCESS {
            let _ = connector.ivars().keep_alive.borrow_mut().take();
            connector.finish(Err(BluetoothError::ConnectionFailed(format!(
                "performSDPQuery failed ({status})"
            ))));
        }
    }

    /// Serve a read on a live stream (main thread).
    pub fn spp_read(stream: &StreamOwner, max: usize, tx: ReadTx, mtm: MainThreadMarker) {
        stream.get(mtm).enqueue_read(max, tx);
    }

    /// Kick a write on a live stream (main thread).
    pub fn spp_write(stream: &StreamOwner, payload: &[u8], tx: WriteTx, mtm: MainThreadMarker) {
        stream.get(mtm).enqueue_write(payload, tx);
    }

    /// Close a live stream (main thread); drops the bound right after.
    pub fn spp_close(stream: StreamOwner, mtm: MainThreadMarker) {
        let _ = stream.get(mtm).close_stream();
        drop(stream);
    }

    /// Stop a running inquiry (main thread); drops the bound right after.
    pub fn stop_inquiry(inquiry: MainThreadBound<Retained<Inquiry>>, mtm: MainThreadMarker) {
        let inquiry = inquiry.into_inner(mtm);
        if let Some(object) = inquiry.ivars().inquiry.borrow_mut().take() {
            // SAFETY: `stop` on a live inquiry.
            unsafe { object.stop() };
        }
        inquiry.ivars().tx.borrow_mut().take();
        drop(inquiry);
    }
}

#[cfg(target_os = "ios")]
const fn ios_classic_unavailable<T>() -> Result<T, BluetoothError> {
    Err(BluetoothError::NotAvailable)
}

pub struct ClassicBluetoothInner {
    #[cfg(target_os = "macos")]
    inquiry: Mutex<Option<MainThreadBound<Retained<classic::Inquiry>>>>,
}

impl core::fmt::Debug for ClassicBluetoothInner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ClassicBluetoothInner")
            .finish_non_exhaustive()
    }
}

impl ClassicBluetoothInner {
    /// Fail fast unless the adapter is powered on.
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
            match adapter_state().await? {
                AdapterState::PoweredOn => Ok(Self {
                    inquiry: Mutex::new(None),
                }),
                AdapterState::PoweredOff => Err(BluetoothError::PoweredOff),
                _ => Err(BluetoothError::NotAvailable),
            }
        }
    }

    /// Start a classic inquiry; the stream yields found devices until
    /// `stop_discovery`.
    #[cfg_attr(
        target_os = "ios",
        expect(clippy::unused_async, reason = "iOS stub returns immediately")
    )]
    pub async fn start_discovery(&self) -> Result<Receiver<ClassicDevice>, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
            ios_classic_unavailable()
        }
        #[cfg(target_os = "macos")]
        {
            self.stop_discovery();
            let (tx, rx) = async_channel::unbounded();
            *self.inquiry.lock().expect("inquiry mutex") =
                on_main(|mtm| classic::start_discovery(mtm, tx)).await.ok();
            if self.inquiry.lock().expect("inquiry mutex").is_none() {
                return Err(BluetoothError::Platform(
                    "IOBluetoothDeviceInquiry start failed".into(),
                ));
            }
            Ok(rx)
        }
    }

    /// Stop the running inquiry (if any) and release its delegate on the
    /// main queue.
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
            let inquiry = self.inquiry.lock().expect("inquiry mutex").take();
            if let Some(inquiry) = inquiry {
                DispatchQueue::main().exec_async(move || {
                    let mtm = MainThreadMarker::new().expect("on the main queue");
                    classic::stop_inquiry(inquiry, mtm);
                });
            }
        }
    }

    /// List paired/bonded devices (main-queue hop since it touches
    /// `IOBluetooth` objects).
    #[cfg_attr(
        target_os = "ios",
        expect(clippy::unused_async, reason = "iOS stub returns immediately")
    )]
    pub async fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
            ios_classic_unavailable()
        }
        #[cfg(target_os = "macos")]
        {
            Ok(on_main(|_mtm| classic::paired_devices()).await)
        }
    }

    /// Drive the SDP→RFCOMM-open chain on the main queue; resolves with the
    /// live stream once `rfcommChannelOpenComplete` fires.
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
            let device_id = device_id.clone();
            let uuid = uuid.clone();
            let (tx, rx) = oneshot::channel();
            on_main(move |mtm| {
                classic::connect_spp(mtm, &device_id, &uuid, tx);
            })
            .await;
            let stream = rx
                .await
                .map_err(|_| BluetoothError::ConnectionFailed("callback dropped".into()))??;
            Ok(SppStreamInner { stream })
        }
    }
}

impl Drop for ClassicBluetoothInner {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let inquiry = self.inquiry.get_mut().expect("inquiry mutex").take();
            if let Some(inquiry) = inquiry {
                DispatchQueue::main().exec_async(move || {
                    let mtm = MainThreadMarker::new().expect("on the main queue");
                    classic::stop_inquiry(inquiry, mtm);
                });
            }
        }
    }
}

/// Live RFCOMM stream; the `SppStream` object lives on the main queue.
pub struct SppStreamInner {
    #[cfg(target_os = "macos")]
    stream: StreamOwner,
}

impl core::fmt::Debug for SppStreamInner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SppStreamInner").finish_non_exhaustive()
    }
}

impl SppStreamInner {
    /// Read up to `buf.len()` bytes; resolves as soon as any data arrives.
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
            let stream = Arc::clone(&self.stream);
            let max = buf.len();
            let (tx, rx) = oneshot::channel();
            on_main(move |mtm| {
                classic::spp_read(&stream, max, tx, mtm);
            })
            .await;
            let chunk = rx
                .await
                .map_err(|_| BluetoothError::ConnectionFailed("callback dropped".into()))??;
            buf[..chunk.len()].copy_from_slice(&chunk);
            Ok(chunk.len())
        }
    }

    /// Write `data`; resolves when the channel reports the write complete.
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
            let stream = Arc::clone(&self.stream);
            let payload = data.to_vec();
            let (tx, rx) = oneshot::channel();
            on_main(move |mtm| {
                classic::spp_write(&stream, &payload, tx, mtm);
            })
            .await;
            rx.await
                .map_err(|_| BluetoothError::ConnectionFailed("callback dropped".into()))?
        }
    }

    /// Close the channel on the main queue and release the stream.
    pub fn close(self) {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
        }
        #[cfg(target_os = "macos")]
        {
            let stream = Arc::clone(&self.stream);
            DispatchQueue::main().exec_async(move || {
                let mtm = MainThreadMarker::new().expect("on the main queue");
                classic::spp_close(stream, mtm);
            });
            // `self` drops here without touching the stream bound again.
        }
    }
}

impl Drop for SppStreamInner {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let stream = Arc::clone(&self.stream);
            DispatchQueue::main().exec_async(move || {
                let mtm = MainThreadMarker::new().expect("on the main queue");
                classic::spp_close(stream, mtm);
            });
        }
    }
}
