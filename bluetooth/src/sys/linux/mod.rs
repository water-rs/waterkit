use crate::{
    AdapterState, BluetoothDevice, BluetoothError, CharacteristicProperties, ClassicDevice,
    DeviceId, GattCharacteristic, GattService, ScanFilter, ScanResult, Uuid,
};
use std::collections::HashMap;
use zbus::Connection;

const BLUEZ_SERVICE: &str = "org.bluez";
const ADAPTER_PATH: &str = "/org/bluez/hci0";
const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
const DEVICE_IFACE: &str = "org.bluez.Device1";
const GATT_SERVICE_IFACE: &str = "org.bluez.GattService1";
const GATT_CHAR_IFACE: &str = "org.bluez.GattCharacteristic1";

async fn get_connection() -> Result<Connection, BluetoothError> {
    Connection::system()
        .await
        .map_err(|e| BluetoothError::PlatformError(format!("D-Bus connection failed: {e}")))
}

pub async fn adapter_state() -> Result<AdapterState, BluetoothError> {
    let conn = get_connection().await?;
    let proxy = zbus::fdo::PropertiesProxy::builder(&conn)
        .destination(BLUEZ_SERVICE)
        .map_err(|e| BluetoothError::PlatformError(e.to_string()))?
        .path(ADAPTER_PATH)
        .map_err(|e| BluetoothError::PlatformError(e.to_string()))?
        .build()
        .await
        .map_err(|e| BluetoothError::PlatformError(e.to_string()))?;
    let powered: bool = proxy
        .get(ADAPTER_IFACE, "Powered")
        .await
        .map_err(|e| BluetoothError::PlatformError(e.to_string()))?
        .try_into()
        .map_err(|e: zbus::zvariant::Error| BluetoothError::PlatformError(e.to_string()))?;
    if powered {
        Ok(AdapterState::PoweredOn)
    } else {
        Ok(AdapterState::PoweredOff)
    }
}

#[derive(Debug)]
pub struct BleScannerInner {
    connection: Connection,
}

impl BleScannerInner {
    pub async fn new() -> Result<Self, BluetoothError> {
        let state = adapter_state().await?;
        if state != AdapterState::PoweredOn {
            return Err(BluetoothError::NotAvailable);
        }
        let connection = get_connection().await?;
        Ok(Self { connection })
    }

    pub fn start_scan(
        &self,
        _filter: &ScanFilter,
    ) -> Result<async_channel::Receiver<ScanResult>, BluetoothError> {
        let (tx, rx) = async_channel::bounded(64);
        let conn = self.connection.clone();
        std::thread::spawn(move || {
            let rt = futures::executor::block_on(async {
                let proxy = zbus::fdo::PropertiesProxy::builder(&conn)
                    .destination(BLUEZ_SERVICE)
                    .unwrap()
                    .path(ADAPTER_PATH)
                    .unwrap()
                    .build()
                    .await
                    .unwrap();
                // Start discovery via method call
                let adapter_proxy =
                    zbus::Proxy::new(&conn, BLUEZ_SERVICE, ADAPTER_PATH, ADAPTER_IFACE)
                        .await
                        .unwrap();
                let _ = adapter_proxy.call_method("StartDiscovery", &()).await;

                // Listen for InterfacesAdded signals
                let object_manager = zbus::fdo::ObjectManagerProxy::builder(&conn)
                    .destination(BLUEZ_SERVICE)
                    .unwrap()
                    .path("/")
                    .unwrap()
                    .build()
                    .await
                    .unwrap();
                let mut stream = object_manager.receive_interfaces_added().await.unwrap();
                use futures::StreamExt;
                while let Some(signal) = stream.next().await {
                    if let Ok(args) = signal.args() {
                        let path = args.object_path().to_string();
                        if path.starts_with("/org/bluez/hci0/dev_") {
                            let ifaces = args.interfaces_and_properties();
                            if let Some(props) = ifaces.get(DEVICE_IFACE) {
                                let name = props
                                    .get("Name")
                                    .and_then(|v| v.downcast_ref::<str>())
                                    .map(String::from);
                                let addr = props
                                    .get("Address")
                                    .and_then(|v| v.downcast_ref::<str>())
                                    .unwrap_or("")
                                    .to_string();
                                let rssi: i16 = props
                                    .get("RSSI")
                                    .and_then(|v| v.downcast_ref::<i16>().copied())
                                    .unwrap_or(0);
                                let result = ScanResult {
                                    device: BluetoothDevice {
                                        id: DeviceId(addr),
                                        name,
                                        rssi: Some(rssi),
                                        is_connected: false,
                                    },
                                    service_uuids: Vec::new(),
                                    manufacturer_data: HashMap::new(),
                                };
                                if tx.try_send(result).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        });
        Ok(rx)
    }

    pub fn stop_scan(&self) {
        let conn = self.connection.clone();
        std::thread::spawn(move || {
            futures::executor::block_on(async {
                let proxy = zbus::Proxy::new(&conn, BLUEZ_SERVICE, ADAPTER_PATH, ADAPTER_IFACE)
                    .await
                    .ok();
                if let Some(p) = proxy {
                    let _ = p.call_method("StopDiscovery", &()).await;
                }
            });
        });
    }
}

#[derive(Debug)]
pub struct BleConnectionInner {
    connection: Connection,
    device_path: String,
}

impl BleConnectionInner {
    pub async fn connect(device_id: &DeviceId) -> Result<Self, BluetoothError> {
        let conn = get_connection().await?;
        let addr = device_id.0.replace(':', "_");
        let device_path = format!("{ADAPTER_PATH}/dev_{addr}");
        let proxy = zbus::Proxy::new(&conn, BLUEZ_SERVICE, &device_path, DEVICE_IFACE)
            .await
            .map_err(|e| BluetoothError::ConnectionFailed(e.to_string()))?;
        proxy
            .call_method("Connect", &())
            .await
            .map_err(|e| BluetoothError::ConnectionFailed(e.to_string()))?;
        Ok(Self {
            connection: conn,
            device_path,
        })
    }

    pub async fn discover_services(&self) -> Result<Vec<GattService>, BluetoothError> {
        let object_manager = zbus::fdo::ObjectManagerProxy::builder(&self.connection)
            .destination(BLUEZ_SERVICE)
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .path("/")
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .build()
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let objects = object_manager
            .get_managed_objects()
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let mut services = Vec::new();
        for (path, ifaces) in &objects {
            let path_str = path.to_string();
            if !path_str.starts_with(&self.device_path) {
                continue;
            }
            if let Some(props) = ifaces.get(GATT_SERVICE_IFACE) {
                let uuid = props
                    .get("UUID")
                    .and_then(|v| v.downcast_ref::<str>())
                    .unwrap_or("")
                    .to_string();
                let is_primary = props
                    .get("Primary")
                    .and_then(|v| v.downcast_ref::<bool>().copied())
                    .unwrap_or(false);
                let mut characteristics = Vec::new();
                for (cpath, cifaces) in &objects {
                    let cpath_str = cpath.to_string();
                    if cpath_str.starts_with(&path_str) && cifaces.contains_key(GATT_CHAR_IFACE) {
                        if let Some(cprops) = cifaces.get(GATT_CHAR_IFACE) {
                            let cuuid = cprops
                                .get("UUID")
                                .and_then(|v| v.downcast_ref::<str>())
                                .unwrap_or("")
                                .to_string();
                            let flags: Vec<String> = cprops
                                .get("Flags")
                                .and_then(|v| {
                                    v.downcast_ref::<zbus::zvariant::Array>().map(|a| {
                                        a.iter()
                                            .filter_map(|f| {
                                                f.downcast_ref::<str>().map(String::from)
                                            })
                                            .collect()
                                    })
                                })
                                .unwrap_or_default();
                            characteristics.push(GattCharacteristic {
                                uuid: Uuid(cuuid),
                                properties: CharacteristicProperties {
                                    read: flags.iter().any(|f| f == "read"),
                                    write: flags.iter().any(|f| f == "write"),
                                    write_without_response: flags
                                        .iter()
                                        .any(|f| f == "write-without-response"),
                                    notify: flags.iter().any(|f| f == "notify"),
                                    indicate: flags.iter().any(|f| f == "indicate"),
                                },
                            });
                        }
                    }
                }
                services.push(GattService {
                    uuid: Uuid(uuid),
                    is_primary,
                    characteristics,
                });
            }
        }
        Ok(services)
    }

    pub async fn read_characteristic(
        &self,
        _service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<Vec<u8>, BluetoothError> {
        let char_path = self.find_char_path(characteristic).await?;
        let proxy = zbus::Proxy::new(&self.connection, BLUEZ_SERVICE, &char_path, GATT_CHAR_IFACE)
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let opts: HashMap<String, zbus::zvariant::Value> = HashMap::new();
        let result: Vec<u8> = proxy
            .call_method("ReadValue", &(opts,))
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .body()
            .deserialize()
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        Ok(result)
    }

    pub async fn write_characteristic(
        &self,
        _service: &Uuid,
        characteristic: &Uuid,
        data: &[u8],
    ) -> Result<(), BluetoothError> {
        let char_path = self.find_char_path(characteristic).await?;
        let proxy = zbus::Proxy::new(&self.connection, BLUEZ_SERVICE, &char_path, GATT_CHAR_IFACE)
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let opts: HashMap<String, zbus::zvariant::Value> = HashMap::new();
        proxy
            .call_method("WriteValue", &(data, opts))
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        Ok(())
    }

    pub fn subscribe(
        &self,
        _service: &Uuid,
        _characteristic: &Uuid,
    ) -> Result<async_channel::Receiver<Vec<u8>>, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    pub async fn disconnect(self) {
        let proxy = zbus::Proxy::new(
            &self.connection,
            BLUEZ_SERVICE,
            &self.device_path,
            DEVICE_IFACE,
        )
        .await
        .ok();
        if let Some(p) = proxy {
            let _ = p.call_method("Disconnect", &()).await;
        }
    }

    async fn find_char_path(&self, uuid: &Uuid) -> Result<String, BluetoothError> {
        let object_manager = zbus::fdo::ObjectManagerProxy::builder(&self.connection)
            .destination(BLUEZ_SERVICE)
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .path("/")
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .build()
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let objects = object_manager
            .get_managed_objects()
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        for (path, ifaces) in &objects {
            let path_str = path.to_string();
            if path_str.starts_with(&self.device_path) {
                if let Some(props) = ifaces.get(GATT_CHAR_IFACE) {
                    let cuuid = props
                        .get("UUID")
                        .and_then(|v| v.downcast_ref::<str>())
                        .unwrap_or("");
                    if cuuid == uuid.0 {
                        return Ok(path_str);
                    }
                }
            }
        }
        Err(BluetoothError::GattError("Characteristic not found".into()))
    }
}

#[derive(Debug)]
pub struct ClassicBluetoothInner;

impl ClassicBluetoothInner {
    #[allow(clippy::unused_async)]
    pub async fn new() -> Result<Self, BluetoothError> {
        Ok(Self)
    }

    pub fn start_discovery(
        &self,
    ) -> Result<async_channel::Receiver<ClassicDevice>, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

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
