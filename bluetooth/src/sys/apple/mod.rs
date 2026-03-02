use crate::{
    AdapterState, BluetoothDevice, BluetoothError, CharacteristicProperties, ClassicDevice,
    DeviceId, GattCharacteristic, GattService, ScanFilter, ScanResult, Uuid,
};
use futures::channel::oneshot;
use std::collections::HashMap;
use std::sync::Mutex;

#[cfg(target_os = "ios")]
fn ios_classic_unavailable_error() -> BluetoothError {
    BluetoothError::PlatformError(
        "iOS does not expose Classic Bluetooth / SPP APIs; use BLE GATT APIs instead".into(),
    )
}

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
        fn bluetooth_classic_is_available() -> bool;
        fn bluetooth_classic_start_discovery(scan_ctx: u64) -> String;
        fn bluetooth_classic_stop_discovery(scan_ctx: u64);
        fn bluetooth_classic_paired_devices(query_ctx: u64);
        fn bluetooth_classic_connect_spp(
            device_id: &str,
            uuid: &str,
            stream_ctx: u64,
            connect_ctx: u64,
        );
        fn bluetooth_classic_spp_read(stream_ctx: u64, max_bytes: u64, read_ctx: u64);
        fn bluetooth_classic_spp_write(stream_ctx: u64, data: &[u8], write_ctx: u64);
        fn bluetooth_classic_spp_close(stream_ctx: u64, close_ctx: u64);
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
        fn on_classic_scan_result_raw(
            scan_ctx: u64,
            device_id: &str,
            name: Option<String>,
            device_class: u32,
            is_paired: bool,
            is_connected: bool,
        );
        fn on_classic_paired_devices_result_raw(query_ctx: u64, payload: &str, error: &str);
        fn on_classic_connect_result_raw(connect_ctx: u64, error: &str);
        fn on_classic_spp_read_result_raw(read_ctx: u64, data: Vec<u8>, error: &str);
        fn on_classic_spp_write_result_raw(write_ctx: u64, written: u64, error: &str);
        fn on_classic_spp_close_result_raw(close_ctx: u64, error: &str);
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

/// Called from Swift when a Classic Bluetooth device is discovered.
/// `scan_ctx` is a raw pointer to a `async_channel::Sender<ClassicDevice>`.
#[allow(clippy::cast_possible_truncation)]
fn on_classic_scan_result_raw(
    scan_ctx: u64,
    device_id: &str,
    name: Option<String>,
    device_class: u32,
    is_paired: bool,
    is_connected: bool,
) {
    let tx = unsafe { &*(scan_ctx as usize as *const async_channel::Sender<ClassicDevice>) };
    let _ = tx.try_send(ClassicDevice {
        device: BluetoothDevice {
            id: DeviceId(device_id.to_string()),
            name,
            rssi: None,
            is_connected,
        },
        device_class,
        is_paired,
    });
}

#[allow(clippy::cast_possible_truncation)]
fn on_classic_paired_devices_result_raw(query_ctx: u64, payload: &str, error: &str) {
    let tx = unsafe {
        Box::from_raw(query_ctx as usize as *mut oneshot::Sender<Result<String, BluetoothError>>)
    };
    let result = if error.is_empty() {
        Ok(payload.to_string())
    } else {
        Err(BluetoothError::PlatformError(error.to_string()))
    };
    let _ = tx.send(result);
}

#[allow(clippy::cast_possible_truncation)]
fn on_classic_connect_result_raw(connect_ctx: u64, error: &str) {
    let tx = unsafe {
        Box::from_raw(connect_ctx as usize as *mut oneshot::Sender<Result<(), BluetoothError>>)
    };
    let result = if error.is_empty() {
        Ok(())
    } else {
        Err(BluetoothError::ConnectionFailed(error.to_string()))
    };
    let _ = tx.send(result);
}

#[allow(clippy::cast_possible_truncation)]
fn on_classic_spp_read_result_raw(read_ctx: u64, data: Vec<u8>, error: &str) {
    let tx = unsafe {
        Box::from_raw(read_ctx as usize as *mut oneshot::Sender<Result<Vec<u8>, BluetoothError>>)
    };
    let result = if error.is_empty() {
        Ok(data)
    } else {
        Err(BluetoothError::ConnectionFailed(error.to_string()))
    };
    let _ = tx.send(result);
}

#[allow(clippy::cast_possible_truncation)]
fn on_classic_spp_write_result_raw(write_ctx: u64, written: u64, error: &str) {
    let tx = unsafe {
        Box::from_raw(write_ctx as usize as *mut oneshot::Sender<Result<u64, BluetoothError>>)
    };
    let result = if error.is_empty() {
        Ok(written)
    } else {
        Err(BluetoothError::ConnectionFailed(error.to_string()))
    };
    let _ = tx.send(result);
}

#[allow(clippy::cast_possible_truncation)]
fn on_classic_spp_close_result_raw(close_ctx: u64, error: &str) {
    if close_ctx == 0 {
        return;
    }
    let tx = unsafe {
        Box::from_raw(close_ctx as usize as *mut oneshot::Sender<Result<(), BluetoothError>>)
    };
    let result = if error.is_empty() {
        Ok(())
    } else {
        Err(BluetoothError::ConnectionFailed(error.to_string()))
    };
    let _ = tx.send(result);
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
    scan_rx: async_channel::Receiver<ScanResult>,
    scan_ctx: u64,
}

impl BleScannerInner {
    pub async fn new() -> Result<Self, BluetoothError> {
        let state = adapter_state().await?;
        if state != AdapterState::PoweredOn {
            return Err(BluetoothError::NotAvailable);
        }
        let (tx, rx) = async_channel::bounded(64);
        let scan_tx = Box::new(tx);
        let scan_ctx = (&raw const *scan_tx) as usize as u64;
        Ok(Self {
            _scan_tx: scan_tx,
            scan_rx: rx,
            scan_ctx,
        })
    }

    #[allow(clippy::unnecessary_wraps)]
    pub fn start_scan(
        &self,
        filter: &ScanFilter,
    ) -> Result<async_channel::Receiver<ScanResult>, BluetoothError> {
        let uuids = filter
            .service_uuids
            .iter()
            .map(|u| u.0.as_str())
            .collect::<Vec<_>>()
            .join(",");
        ffi::bluetooth_start_scan(self.scan_ctx, uuids);
        Ok(self.scan_rx.clone())
    }

    pub fn stop_scan(&self) {
        ffi::bluetooth_stop_scan(self.scan_ctx);
    }
}

#[derive(Debug)]
pub struct BleConnectionInner {
    device_id: String,
    notify_txs: Mutex<HashMap<String, Box<async_channel::Sender<Vec<u8>>>>>,
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
            notify_txs: Mutex::new(HashMap::new()),
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
        let ctx = (&raw const *notify_tx) as usize as u64;
        let key = format!("{}:{}", service.0, characteristic.0);
        self.notify_txs
            .lock()
            .expect("notify callback registry poisoned")
            .insert(key, notify_tx);
        ffi::bluetooth_subscribe(&self.device_id, &service.0, &characteristic.0, ctx);
        Ok(rx)
    }

    #[allow(clippy::unused_async)]
    pub async fn disconnect(self) {
        ffi::bluetooth_disconnect(&self.device_id);
    }
}

#[derive(Debug)]
pub struct ClassicBluetoothInner {
    #[cfg(target_os = "macos")]
    scan_tx: Mutex<Option<Box<async_channel::Sender<ClassicDevice>>>>,
    #[cfg(target_os = "macos")]
    scan_ctx: Mutex<Option<u64>>,
}

impl ClassicBluetoothInner {
    pub async fn new() -> Result<Self, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            return Err(ios_classic_unavailable_error());
        }

        #[cfg(target_os = "macos")]
        {
            let state = adapter_state().await?;
            if state != AdapterState::PoweredOn {
                return Err(BluetoothError::NotAvailable);
            }
            if !ffi::bluetooth_classic_is_available() {
                return Err(BluetoothError::NotAvailable);
            }
            Ok(Self {
                scan_tx: Mutex::new(None),
                scan_ctx: Mutex::new(None),
            })
        }
    }

    pub fn start_discovery(
        &self,
    ) -> Result<async_channel::Receiver<ClassicDevice>, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
            return Err(ios_classic_unavailable_error());
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(ctx) = self
                .scan_ctx
                .lock()
                .expect("classic scan context mutex poisoned")
                .take()
            {
                ffi::bluetooth_classic_stop_discovery(ctx);
            }
            let _ = self
                .scan_tx
                .lock()
                .expect("classic scan sender mutex poisoned")
                .take();

            let (tx, rx) = async_channel::bounded(64);
            let scan_tx = Box::new(tx);
            let scan_ctx = (&raw const *scan_tx) as usize as u64;
            let error = ffi::bluetooth_classic_start_discovery(scan_ctx);
            if !error.is_empty() {
                return Err(BluetoothError::PlatformError(error));
            }

            self.scan_tx
                .lock()
                .expect("classic scan sender mutex poisoned")
                .replace(scan_tx);
            self.scan_ctx
                .lock()
                .expect("classic scan context mutex poisoned")
                .replace(scan_ctx);
            Ok(rx)
        }
    }

    pub fn stop_discovery(&self) {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(ctx) = self
                .scan_ctx
                .lock()
                .expect("classic scan context mutex poisoned")
                .take()
            {
                ffi::bluetooth_classic_stop_discovery(ctx);
            }
            let _ = self
                .scan_tx
                .lock()
                .expect("classic scan sender mutex poisoned")
                .take();
        }
    }

    pub async fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
            return Err(ios_classic_unavailable_error());
        }

        #[cfg(target_os = "macos")]
        {
            let (tx, rx) = oneshot::channel::<Result<String, BluetoothError>>();
            let query_ctx = Box::into_raw(Box::new(tx)) as usize as u64;
            ffi::bluetooth_classic_paired_devices(query_ctx);
            let payload = rx.await.map_err(|_| {
                BluetoothError::PlatformError("classic paired devices callback dropped".into())
            })??;
            parse_classic_paired_devices(&payload)
        }
    }

    pub async fn connect_spp(
        &self,
        device_id: &DeviceId,
        uuid: &Uuid,
    ) -> Result<SppStreamInner, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
            let _ = device_id;
            let _ = uuid;
            return Err(ios_classic_unavailable_error());
        }

        #[cfg(target_os = "macos")]
        {
            let _ = self;
            let stream_ctx_token = Box::new(0_u8);
            let stream_ctx = (&raw const *stream_ctx_token) as usize as u64;
            let (tx, rx) = oneshot::channel::<Result<(), BluetoothError>>();
            let connect_ctx = Box::into_raw(Box::new(tx)) as usize as u64;
            ffi::bluetooth_classic_connect_spp(&device_id.0, &uuid.0, stream_ctx, connect_ctx);
            rx.await.map_err(|_| {
                BluetoothError::ConnectionFailed("classic SPP connect callback dropped".into())
            })??;
            Ok(SppStreamInner {
                stream_ctx,
                _stream_ctx_token: stream_ctx_token,
            })
        }
    }
}

#[derive(Debug)]
pub struct SppStreamInner {
    #[cfg(target_os = "macos")]
    stream_ctx: u64,
    #[cfg(target_os = "macos")]
    _stream_ctx_token: Box<u8>,
}

impl SppStreamInner {
    pub async fn read(&self, buf: &mut [u8]) -> Result<usize, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
            let _ = buf;
            return Err(ios_classic_unavailable_error());
        }

        #[cfg(target_os = "macos")]
        {
            let max_bytes = u64::try_from(buf.len())
                .map_err(|_| BluetoothError::ConnectionFailed("buffer length overflow".into()))?;
            let (tx, rx) = oneshot::channel::<Result<Vec<u8>, BluetoothError>>();
            let read_ctx = Box::into_raw(Box::new(tx)) as usize as u64;
            ffi::bluetooth_classic_spp_read(self.stream_ctx, max_bytes, read_ctx);
            let data = rx.await.map_err(|_| {
                BluetoothError::ConnectionFailed("classic SPP read callback dropped".into())
            })??;
            let read = data.len().min(buf.len());
            buf[..read].copy_from_slice(&data[..read]);
            Ok(read)
        }
    }

    pub async fn write(&self, data: &[u8]) -> Result<usize, BluetoothError> {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
            let _ = data;
            return Err(ios_classic_unavailable_error());
        }

        #[cfg(target_os = "macos")]
        {
            let (tx, rx) = oneshot::channel::<Result<u64, BluetoothError>>();
            let write_ctx = Box::into_raw(Box::new(tx)) as usize as u64;
            ffi::bluetooth_classic_spp_write(self.stream_ctx, data, write_ctx);
            let written = rx.await.map_err(|_| {
                BluetoothError::ConnectionFailed("classic SPP write callback dropped".into())
            })??;
            usize::try_from(written).map_err(|_| {
                BluetoothError::ConnectionFailed(format!(
                    "classic SPP write size exceeds usize: {written}"
                ))
            })
        }
    }

    pub async fn close(self) {
        #[cfg(target_os = "ios")]
        {
            let _ = self;
        }

        #[cfg(target_os = "macos")]
        {
            let (tx, rx) = oneshot::channel::<Result<(), BluetoothError>>();
            let close_ctx = Box::into_raw(Box::new(tx)) as usize as u64;
            ffi::bluetooth_classic_spp_close(self.stream_ctx, close_ctx);
            let _ = rx.await;
        }
    }
}

impl Drop for SppStreamInner {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            ffi::bluetooth_classic_spp_close(self.stream_ctx, 0);
        }
    }
}

#[cfg(target_os = "macos")]
fn parse_classic_paired_devices(payload: &str) -> Result<Vec<ClassicDevice>, BluetoothError> {
    payload
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() != 4 {
                return Err(BluetoothError::PlatformError(format!(
                    "malformed classic paired device row: {line}"
                )));
            }
            let device_class = parts[2].parse::<u32>().map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "invalid classic device class '{}' in row '{line}': {error}",
                    parts[2]
                ))
            })?;
            let is_connected = parts[3] == "1";
            let name = if parts[1].is_empty() {
                None
            } else {
                Some(parts[1].to_string())
            };
            Ok(ClassicDevice {
                device: BluetoothDevice {
                    id: DeviceId(parts[0].to_string()),
                    name,
                    rssi: None,
                    is_connected,
                },
                device_class,
                is_paired: true,
            })
        })
        .collect()
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
