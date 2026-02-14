use crate::{
    AdapterState, BluetoothDevice, BluetoothError, CharacteristicProperties, ClassicDevice,
    DeviceId, GattCharacteristic, GattService, ScanFilter, ScanResult, Uuid,
};
use std::collections::HashMap;

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn bluetooth_adapter_state(callback: Box<dyn FnOnce(String) -> ()>);
        fn bluetooth_start_scan(scan_ctx: u64, service_uuids: String);
        fn bluetooth_stop_scan(scan_ctx: u64);
        fn bluetooth_connect(device_id: &str, callback: Box<dyn FnOnce(String) -> ()>);
        fn bluetooth_disconnect(device_id: &str);
        fn bluetooth_discover_services(
            device_id: &str,
            callback: Box<dyn FnOnce(String, String) -> ()>,
        );
        fn bluetooth_read_characteristic(
            device_id: &str,
            service_uuid: &str,
            char_uuid: &str,
            callback: Box<dyn FnOnce(Vec<u8>, String) -> ()>,
        );
        fn bluetooth_write_characteristic(
            device_id: &str,
            service_uuid: &str,
            char_uuid: &str,
            data: &[u8],
            callback: Box<dyn FnOnce(String) -> ()>,
        );
        fn bluetooth_subscribe(
            device_id: &str,
            service_uuid: &str,
            char_uuid: &str,
            notify_ctx: u64,
        );
    }

    extern "Rust" {
        fn on_scan_result_raw(
            scan_ctx: u64,
            device_id: &str,
            name: Option<String>,
            rssi: i16,
            service_uuids_csv: &str,
        );
        fn on_notify_value_raw(notify_ctx: u64, data: Vec<u8>);
    }
}

/// Called from Swift when a BLE scan result is found.
/// `scan_ctx` is a raw pointer to a `async_channel::Sender<ScanResult>`.
#[allow(clippy::cast_possible_truncation)]
fn on_scan_result_raw(
    scan_ctx: u64,
    device_id: &str,
    name: Option<String>,
    rssi: i16,
    service_uuids_csv: &str,
) {
    let tx = unsafe { &*(scan_ctx as usize as *const async_channel::Sender<ScanResult>) };
    let service_uuids: Vec<Uuid> = service_uuids_csv
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| Uuid(s.to_string()))
        .collect();
    let result = ScanResult {
        device: BluetoothDevice {
            id: DeviceId(device_id.to_string()),
            name,
            rssi: Some(rssi),
            is_connected: false,
        },
        service_uuids,
        manufacturer_data: HashMap::new(),
    };
    let _ = tx.try_send(result);
}

/// Called from Swift when a BLE notification value is received.
/// `notify_ctx` is a raw pointer to a `async_channel::Sender<Vec<u8>>`.
#[allow(clippy::cast_possible_truncation)]
fn on_notify_value_raw(notify_ctx: u64, data: Vec<u8>) {
    let tx = unsafe { &*(notify_ctx as usize as *const async_channel::Sender<Vec<u8>>) };
    let _ = tx.try_send(data);
}

pub async fn adapter_state() -> Result<AdapterState, BluetoothError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::bluetooth_adapter_state(Box::new(move |state: String| {
        let result = match state.as_str() {
            "poweredOn" => AdapterState::PoweredOn,
            "poweredOff" => AdapterState::PoweredOff,
            "unauthorized" => AdapterState::Unauthorized,
            "unsupported" => AdapterState::Unavailable,
            _ => AdapterState::Unknown,
        };
        let _ = tx.send(result);
    }));
    rx.await
        .map_err(|_| BluetoothError::PlatformError("callback dropped".into()))
}

#[derive(Debug)]
pub struct BleScannerInner {
    /// Boxed sender kept alive for the duration of scanning.
    /// Swift holds a raw pointer to it.
    _scan_tx: Box<async_channel::Sender<ScanResult>>,
    scan_ctx: u64,
}

impl BleScannerInner {
    pub async fn new() -> Result<Self, BluetoothError> {
        let state = adapter_state().await?;
        if state != AdapterState::PoweredOn {
            return Err(BluetoothError::NotAvailable);
        }
        let (tx, _rx) = async_channel::bounded(64);
        let scan_tx = Box::new(tx);
        let scan_ctx = (&raw const *scan_tx) as usize as u64;
        Ok(Self {
            _scan_tx: scan_tx,
            scan_ctx,
        })
    }

    #[allow(clippy::unnecessary_wraps, clippy::unused_self)]
    pub fn start_scan(
        &self,
        filter: &ScanFilter,
    ) -> Result<async_channel::Receiver<ScanResult>, BluetoothError> {
        let (tx, rx) = async_channel::bounded(64);
        let scan_tx = Box::new(tx);
        let ctx = Box::into_raw(scan_tx) as usize as u64;
        let uuids = filter
            .service_uuids
            .iter()
            .map(|u| u.0.as_str())
            .collect::<Vec<_>>()
            .join(",");
        ffi::bluetooth_start_scan(ctx, uuids);
        Ok(rx)
    }

    pub fn stop_scan(&self) {
        ffi::bluetooth_stop_scan(self.scan_ctx);
    }
}

#[derive(Debug)]
pub struct BleConnectionInner {
    device_id: String,
}

impl BleConnectionInner {
    pub async fn connect(device_id: &DeviceId) -> Result<Self, BluetoothError> {
        let (tx, rx) = futures::channel::oneshot::channel();
        ffi::bluetooth_connect(
            &device_id.0,
            Box::new(move |error: String| {
                if error.is_empty() {
                    let _ = tx.send(Ok(()));
                } else {
                    let _ = tx.send(Err(BluetoothError::ConnectionFailed(error)));
                }
            }),
        );
        rx.await
            .map_err(|_| BluetoothError::ConnectionFailed("callback dropped".into()))??;
        Ok(Self {
            device_id: device_id.0.clone(),
        })
    }

    pub async fn discover_services(&self) -> Result<Vec<GattService>, BluetoothError> {
        let (tx, rx) = futures::channel::oneshot::channel();
        ffi::bluetooth_discover_services(
            &self.device_id,
            Box::new(move |services_json: String, error: String| {
                if error.is_empty() {
                    let _ = tx.send(Ok(services_json));
                } else {
                    let _ = tx.send(Err(BluetoothError::GattError(error)));
                }
            }),
        );
        let json = rx
            .await
            .map_err(|_| BluetoothError::GattError("callback dropped".into()))??;
        Ok(parse_services_json(&json))
    }

    pub async fn read_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<Vec<u8>, BluetoothError> {
        let (tx, rx) = futures::channel::oneshot::channel();
        ffi::bluetooth_read_characteristic(
            &self.device_id,
            &service.0,
            &characteristic.0,
            Box::new(move |data: Vec<u8>, error: String| {
                if error.is_empty() {
                    let _ = tx.send(Ok(data));
                } else {
                    let _ = tx.send(Err(BluetoothError::GattError(error)));
                }
            }),
        );
        rx.await
            .map_err(|_| BluetoothError::GattError("callback dropped".into()))?
    }

    pub async fn write_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
        data: &[u8],
    ) -> Result<(), BluetoothError> {
        let (tx, rx) = futures::channel::oneshot::channel();
        ffi::bluetooth_write_characteristic(
            &self.device_id,
            &service.0,
            &characteristic.0,
            data,
            Box::new(move |error: String| {
                if error.is_empty() {
                    let _ = tx.send(Ok(()));
                } else {
                    let _ = tx.send(Err(BluetoothError::GattError(error)));
                }
            }),
        );
        rx.await
            .map_err(|_| BluetoothError::GattError("callback dropped".into()))?
    }

    #[allow(clippy::unnecessary_wraps)]
    pub fn subscribe(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<async_channel::Receiver<Vec<u8>>, BluetoothError> {
        let (tx, rx) = async_channel::bounded(64);
        let notify_tx = Box::new(tx);
        let ctx = Box::into_raw(notify_tx) as usize as u64;
        ffi::bluetooth_subscribe(&self.device_id, &service.0, &characteristic.0, ctx);
        Ok(rx)
    }

    #[allow(clippy::unused_async)]
    pub async fn disconnect(self) {
        ffi::bluetooth_disconnect(&self.device_id);
    }
}

#[derive(Debug)]
pub struct ClassicBluetoothInner;

impl ClassicBluetoothInner {
    #[allow(clippy::unused_async)]
    pub async fn new() -> Result<Self, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    #[allow(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        clippy::missing_const_for_fn
    )]
    pub fn start_discovery(
        &self,
    ) -> Result<async_channel::Receiver<ClassicDevice>, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    #[allow(clippy::unused_self, clippy::missing_const_for_fn)]
    pub fn stop_discovery(&self) {}

    #[allow(clippy::unused_async)]
    pub async fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn connect_spp(
        &self,
        _device_id: &DeviceId,
        _uuid: &Uuid,
    ) -> Result<SppStreamInner, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }
}

#[derive(Debug)]
pub struct SppStreamInner;

impl SppStreamInner {
    #[allow(clippy::unused_async)]
    pub async fn read(&self, _buf: &mut [u8]) -> Result<usize, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn write(&self, _data: &[u8]) -> Result<usize, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn close(self) {}
}

fn parse_services_json(json: &str) -> Vec<GattService> {
    let mut services = Vec::new();
    for svc_str in json.split(';').filter(|s| !s.is_empty()) {
        let parts: Vec<&str> = svc_str.splitn(3, ':').collect();
        if parts.len() < 2 {
            continue;
        }
        let uuid = Uuid(parts[0].to_string());
        let is_primary = parts[1] == "1";
        let mut characteristics = Vec::new();
        if parts.len() == 3 {
            for char_str in parts[2].split(',').filter(|s| !s.is_empty()) {
                let cparts: Vec<&str> = char_str.split(':').collect();
                if cparts.len() >= 6 {
                    characteristics.push(GattCharacteristic {
                        uuid: Uuid(cparts[0].to_string()),
                        properties: CharacteristicProperties {
                            read: cparts[1] == "1",
                            write: cparts[2] == "1",
                            write_without_response: cparts[3] == "1",
                            notify: cparts[4] == "1",
                            indicate: cparts[5] == "1",
                        },
                    });
                }
            }
        }
        services.push(GattService {
            uuid,
            is_primary,
            characteristics,
        });
    }
    services
}
