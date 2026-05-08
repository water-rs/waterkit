use crate::{
    AdapterState, BluetoothDevice, BluetoothError, CharacteristicProperties, ClassicDevice,
    DeviceId, GattCharacteristic, GattService, ScanFilter, ScanResult, Uuid,
};
use futures::StreamExt;
use futures::channel::oneshot;
use std::collections::HashMap;
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;
use zbus::Connection;
use zbus::names::InterfaceName;
use zbus::zvariant::{Array, OwnedValue};

const BLUEZ_SERVICE: &str = "org.bluez";
const ADAPTER_PATH: &str = "/org/bluez/hci0";
const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
const DEVICE_IFACE: &str = "org.bluez.Device1";
const GATT_SERVICE_IFACE: &str = "org.bluez.GattService1";
const GATT_CHAR_IFACE: &str = "org.bluez.GattCharacteristic1";
const BTPROTO_RFCOMM: i32 = 3;
type OwnedPropertyMap = HashMap<String, OwnedValue>;
type SignalPropertyMap<'a> = HashMap<&'a str, zbus::zvariant::Value<'a>>;

#[repr(C)]
struct SockAddrRc {
    family: libc::sa_family_t,
    bdaddr: [u8; 6],
    channel: u8,
}

fn parse_device_address(addr: &str) -> Result<[u8; 6], BluetoothError> {
    let parts: Vec<&str> = addr.split(':').collect();
    if parts.len() != 6 {
        return Err(BluetoothError::ConnectionFailed(format!(
            "invalid Bluetooth address format: {addr}"
        )));
    }
    let mut parsed = [0u8; 6];
    for (index, part) in parts.iter().rev().enumerate() {
        parsed[index] = u8::from_str_radix(part, 16).map_err(|error| {
            BluetoothError::ConnectionFailed(format!(
                "invalid Bluetooth address byte '{part}' in {addr}: {error}"
            ))
        })?;
    }
    Ok(parsed)
}

fn resolve_rfcomm_channel(device_id: &str, uuid: &str) -> Result<u8, BluetoothError> {
    let output = std::process::Command::new("sdptool")
        .args(["search", "--bdaddr", device_id, uuid])
        .output()
        .map_err(|error| {
            BluetoothError::ConnectionFailed(format!(
                "failed to execute sdptool for RFCOMM discovery: {error}"
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BluetoothError::ConnectionFailed(format!(
            "sdptool failed during RFCOMM discovery: {stderr}"
        )));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|error| {
        BluetoothError::ConnectionFailed(format!("sdptool output is not valid UTF-8: {error}"))
    })?;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("Channel:") {
            continue;
        }
        let channel = trimmed
            .split(':')
            .nth(1)
            .map(str::trim)
            .ok_or_else(|| {
                BluetoothError::ConnectionFailed(format!("invalid sdptool channel line: {trimmed}"))
            })?
            .parse::<u8>()
            .map_err(|error| {
                BluetoothError::ConnectionFailed(format!(
                    "invalid RFCOMM channel value in sdptool output '{trimmed}': {error}"
                ))
            })?;
        return Ok(channel);
    }
    Err(BluetoothError::ConnectionFailed(format!(
        "sdptool output did not contain RFCOMM channel for UUID {uuid}"
    )))
}

fn open_rfcomm_socket(device_id: &str, uuid: &str) -> Result<i32, BluetoothError> {
    let channel = resolve_rfcomm_channel(device_id, uuid)?;
    let bdaddr = parse_device_address(device_id)?;
    let family = libc::sa_family_t::try_from(libc::AF_BLUETOOTH).map_err(|_| {
        BluetoothError::ConnectionFailed(format!(
            "AF_BLUETOOTH value {} does not fit sa_family_t",
            libc::AF_BLUETOOTH
        ))
    })?;
    let sockaddr_len =
        libc::socklen_t::try_from(std::mem::size_of::<SockAddrRc>()).map_err(|_| {
            BluetoothError::ConnectionFailed(format!(
                "SockAddrRc size {} does not fit socklen_t",
                std::mem::size_of::<SockAddrRc>()
            ))
        })?;

    let fd = unsafe { libc::socket(libc::AF_BLUETOOTH, libc::SOCK_STREAM, BTPROTO_RFCOMM) };
    if fd < 0 {
        return Err(BluetoothError::ConnectionFailed(format!(
            "create RFCOMM socket failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    let sockaddr = SockAddrRc {
        family,
        bdaddr,
        channel,
    };
    let connect_result = unsafe {
        libc::connect(
            fd,
            (&raw const sockaddr).cast::<libc::sockaddr>(),
            sockaddr_len,
        )
    };
    if connect_result < 0 {
        let error = std::io::Error::last_os_error();
        let _ = unsafe { libc::close(fd) };
        return Err(BluetoothError::ConnectionFailed(format!(
            "connect RFCOMM socket failed: {error}"
        )));
    }
    Ok(fd)
}

fn prop_str(props: &OwnedPropertyMap, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|v| <&str>::try_from(v).ok())
        .map(str::to_owned)
}

fn prop_bool(props: &OwnedPropertyMap, key: &str) -> Option<bool> {
    props.get(key).and_then(|v| bool::try_from(v).ok())
}

fn prop_i16(props: &OwnedPropertyMap, key: &str) -> Option<i16> {
    props.get(key).and_then(|v| i16::try_from(v).ok())
}

fn prop_u32(props: &OwnedPropertyMap, key: &str) -> Option<u32> {
    props.get(key).and_then(|v| u32::try_from(v).ok())
}

fn prop_str_array(props: &OwnedPropertyMap, key: &str) -> Vec<String> {
    props
        .get(key)
        .and_then(|v| <&Array<'_>>::try_from(v).ok())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.downcast_ref::<zbus::zvariant::Str<'_>>()
                        .ok()
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn signal_prop_str(props: &SignalPropertyMap<'_>, key: &str) -> Option<String> {
    props.get(key).and_then(|v| {
        v.downcast_ref::<zbus::zvariant::Str<'_>>()
            .ok()
            .map(|s| s.to_string())
    })
}

fn signal_prop_i16(props: &SignalPropertyMap<'_>, key: &str) -> Option<i16> {
    props.get(key).and_then(|v| v.downcast_ref::<i16>().ok())
}

fn signal_prop_u32(props: &SignalPropertyMap<'_>, key: &str) -> Option<u32> {
    props.get(key).and_then(|v| v.downcast_ref::<u32>().ok())
}

fn signal_prop_bool(props: &SignalPropertyMap<'_>, key: &str) -> Option<bool> {
    props.get(key).and_then(|v| v.downcast_ref::<bool>().ok())
}

async fn get_connection() -> Result<Connection, BluetoothError> {
    Connection::system()
        .await
        .map_err(|e| BluetoothError::Platform(format!("D-Bus connection failed: {e}")))
}

pub async fn adapter_state() -> Result<AdapterState, BluetoothError> {
    let conn = get_connection().await?;
    let proxy = zbus::fdo::PropertiesProxy::builder(&conn)
        .destination(BLUEZ_SERVICE)
        .map_err(|e| BluetoothError::Platform(e.to_string()))?
        .path(ADAPTER_PATH)
        .map_err(|e| BluetoothError::Platform(e.to_string()))?
        .build()
        .await
        .map_err(|e| BluetoothError::Platform(e.to_string()))?;
    let adapter_iface = InterfaceName::try_from(ADAPTER_IFACE)
        .map_err(|e| BluetoothError::Platform(e.to_string()))?;
    let powered_value = proxy
        .get(adapter_iface, "Powered")
        .await
        .map_err(|e| BluetoothError::Platform(e.to_string()))?;
    let powered =
        bool::try_from(&powered_value).map_err(|e| BluetoothError::Platform(e.to_string()))?;
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
        let _bus_name = self.connection.unique_name().ok_or_else(|| {
            BluetoothError::Platform("D-Bus connection is missing unique bus name".into())
        })?;

        let (tx, rx) = async_channel::bounded(64);
        let conn = self.connection.clone();
        std::thread::spawn(move || {
            futures::executor::block_on(async {
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
                while let Some(signal) = stream.next().await {
                    if let Ok(args) = signal.args() {
                        let path = args.object_path().to_string();
                        if path.starts_with("/org/bluez/hci0/dev_") {
                            let ifaces = args.interfaces_and_properties();
                            if let Some(props) = ifaces.get(DEVICE_IFACE) {
                                let name = signal_prop_str(props, "Name");
                                let addr = signal_prop_str(props, "Address").unwrap_or_default();
                                let rssi = signal_prop_i16(props, "RSSI").unwrap_or(0);
                                let result = ScanResult {
                                    device: BluetoothDevice {
                                        id: DeviceId::new(addr),
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
        let addr = device_id.as_str().replace(':', "_");
        let device_path = format!("{ADAPTER_PATH}/dev_{addr}");
        let proxy = zbus::Proxy::new(&conn, BLUEZ_SERVICE, device_path.as_str(), DEVICE_IFACE)
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
                let uuid = prop_str(props, "UUID").unwrap_or_default();
                let is_primary = prop_bool(props, "Primary").unwrap_or(false);
                let mut characteristics = Vec::new();
                for (cpath, cifaces) in &objects {
                    let cpath_str = cpath.to_string();
                    if cpath_str.starts_with(&path_str)
                        && let Some(cprops) = cifaces.get(GATT_CHAR_IFACE)
                    {
                        let cuuid = prop_str(cprops, "UUID").unwrap_or_default();
                        let flags = prop_str_array(cprops, "Flags");
                        characteristics.push(GattCharacteristic {
                            uuid: Uuid::new(cuuid),
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
                services.push(GattService {
                    uuid: Uuid::new(uuid),
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
        let proxy = zbus::Proxy::new(
            &self.connection,
            BLUEZ_SERVICE,
            char_path.as_str(),
            GATT_CHAR_IFACE,
        )
        .await
        .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let opts: HashMap<String, OwnedValue> = HashMap::new();
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
        let proxy = zbus::Proxy::new(
            &self.connection,
            BLUEZ_SERVICE,
            char_path.as_str(),
            GATT_CHAR_IFACE,
        )
        .await
        .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let opts: HashMap<String, OwnedValue> = HashMap::new();
        proxy
            .call_method("WriteValue", &(data, opts))
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        Ok(())
    }

    #[allow(clippy::unused_async)]
    pub async fn subscribe(
        &self,
        _service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<async_channel::Receiver<Vec<u8>>, BluetoothError> {
        if characteristic.0.is_empty() {
            return Err(BluetoothError::GattError(
                "characteristic UUID is empty".into(),
            ));
        }

        let (tx, rx) = async_channel::bounded(64);
        let conn = self.connection.clone();
        let characteristic_uuid = characteristic.0.clone();
        let device_path = self.device_path.clone();

        std::thread::spawn(move || {
            futures::executor::block_on(async move {
                let object_manager = match zbus::fdo::ObjectManagerProxy::builder(&conn)
                    .destination(BLUEZ_SERVICE)
                    .and_then(|builder| builder.path("/"))
                {
                    Ok(builder) => match builder.build().await {
                        Ok(proxy) => proxy,
                        Err(_) => return,
                    },
                    Err(_) => return,
                };

                let Ok(objects) = object_manager.get_managed_objects().await else {
                    return;
                };
                let Some(char_path) = objects.iter().find_map(|(path, ifaces)| {
                    if !path.to_string().starts_with(&device_path) {
                        return None;
                    }
                    let props = ifaces.get(GATT_CHAR_IFACE)?;
                    let uuid = prop_str(props, "UUID")?;
                    if uuid == characteristic_uuid {
                        Some(path.to_string())
                    } else {
                        None
                    }
                }) else {
                    return;
                };

                let Ok(char_proxy) =
                    zbus::Proxy::new(&conn, BLUEZ_SERVICE, char_path.as_str(), GATT_CHAR_IFACE)
                        .await
                else {
                    return;
                };

                if char_proxy.call_method("StartNotify", &()).await.is_err() {
                    return;
                }

                let mut last_value: Option<Vec<u8>> = None;
                while !tx.is_closed() {
                    let opts: HashMap<String, OwnedValue> = HashMap::new();
                    let read_result = char_proxy.call_method("ReadValue", &(opts,)).await;
                    let Ok(reply) = read_result else {
                        break;
                    };
                    let Ok(value) = reply.body().deserialize::<Vec<u8>>() else {
                        break;
                    };
                    if last_value.as_deref() != Some(value.as_slice()) {
                        if tx.try_send(value.clone()).is_err() {
                            break;
                        }
                        last_value = Some(value);
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }

                let _ = char_proxy.call_method("StopNotify", &()).await;
            });
        });

        Ok(rx)
    }

    pub async fn disconnect(self) {
        let proxy = zbus::Proxy::new(
            &self.connection,
            BLUEZ_SERVICE,
            self.device_path.as_str(),
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
            if path_str.starts_with(&self.device_path)
                && let Some(props) = ifaces.get(GATT_CHAR_IFACE)
            {
                let cuuid = prop_str(props, "UUID").unwrap_or_default();
                if cuuid == uuid.as_str() {
                    return Ok(path_str);
                }
            }
        }
        Err(BluetoothError::GattError("Characteristic not found".into()))
    }
}

#[derive(Debug)]
pub struct ClassicBluetoothInner {
    connection: Connection,
}

impl ClassicBluetoothInner {
    pub async fn new() -> Result<Self, BluetoothError> {
        let state = adapter_state().await?;
        if state != AdapterState::PoweredOn {
            return Err(BluetoothError::NotAvailable);
        }
        Ok(Self {
            connection: get_connection().await?,
        })
    }

    #[allow(clippy::unused_async)]
    pub async fn start_discovery(
        &self,
    ) -> Result<async_channel::Receiver<ClassicDevice>, BluetoothError> {
        let _bus_name = self.connection.unique_name().ok_or_else(|| {
            BluetoothError::Platform("D-Bus connection is missing unique bus name".into())
        })?;

        let (tx, rx) = async_channel::bounded(64);
        let conn = self.connection.clone();

        std::thread::spawn(move || {
            futures::executor::block_on(async move {
                let Ok(adapter_proxy) =
                    zbus::Proxy::new(&conn, BLUEZ_SERVICE, ADAPTER_PATH, ADAPTER_IFACE).await
                else {
                    return;
                };
                let _ = adapter_proxy.call_method("StartDiscovery", &()).await;

                let object_manager = match zbus::fdo::ObjectManagerProxy::builder(&conn)
                    .destination(BLUEZ_SERVICE)
                    .and_then(|builder| builder.path("/"))
                {
                    Ok(builder) => match builder.build().await {
                        Ok(proxy) => proxy,
                        Err(_) => return,
                    },
                    Err(_) => return,
                };
                let Ok(mut stream) = object_manager.receive_interfaces_added().await else {
                    return;
                };

                while let Some(signal) = stream.next().await {
                    let Ok(args) = signal.args() else {
                        continue;
                    };
                    let path = args.object_path().to_string();
                    if !path.starts_with("/org/bluez/hci0/dev_") {
                        continue;
                    }
                    let ifaces = args.interfaces_and_properties();
                    let Some(props) = ifaces.get(DEVICE_IFACE) else {
                        continue;
                    };
                    let addr = signal_prop_str(props, "Address").unwrap_or_default();
                    if addr.is_empty() {
                        continue;
                    }
                    let device = ClassicDevice {
                        device: BluetoothDevice {
                            id: DeviceId::new(addr),
                            name: signal_prop_str(props, "Name"),
                            rssi: signal_prop_i16(props, "RSSI"),
                            is_connected: signal_prop_bool(props, "Connected").unwrap_or(false),
                        },
                        device_class: signal_prop_u32(props, "Class").unwrap_or(0),
                        is_paired: signal_prop_bool(props, "Paired").unwrap_or(false),
                    };
                    if tx.try_send(device).is_err() {
                        break;
                    }
                }
            });
        });

        Ok(rx)
    }

    #[allow(clippy::unused_async)]
    pub async fn stop_discovery(&self) {
        let conn = self.connection.clone();
        std::thread::spawn(move || {
            futures::executor::block_on(async move {
                let proxy = zbus::Proxy::new(&conn, BLUEZ_SERVICE, ADAPTER_PATH, ADAPTER_IFACE)
                    .await
                    .ok();
                if let Some(proxy) = proxy {
                    let _ = proxy.call_method("StopDiscovery", &()).await;
                }
            });
        });
    }

    pub async fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        let object_manager = zbus::fdo::ObjectManagerProxy::builder(&self.connection)
            .destination(BLUEZ_SERVICE)
            .map_err(|e| BluetoothError::Platform(e.to_string()))?
            .path("/")
            .map_err(|e| BluetoothError::Platform(e.to_string()))?
            .build()
            .await
            .map_err(|e| BluetoothError::Platform(e.to_string()))?;
        let objects = object_manager
            .get_managed_objects()
            .await
            .map_err(|e| BluetoothError::Platform(e.to_string()))?;
        let mut paired = Vec::new();
        for ifaces in objects.values() {
            let Some(props) = ifaces.get(DEVICE_IFACE) else {
                continue;
            };
            if !prop_bool(props, "Paired").unwrap_or(false) {
                continue;
            }
            let addr = prop_str(props, "Address").unwrap_or_default();
            if addr.is_empty() {
                continue;
            }
            paired.push(ClassicDevice {
                device: BluetoothDevice {
                    id: DeviceId::new(addr),
                    name: prop_str(props, "Name"),
                    rssi: prop_i16(props, "RSSI"),
                    is_connected: prop_bool(props, "Connected").unwrap_or(false),
                },
                device_class: prop_u32(props, "Class").unwrap_or(0),
                is_paired: true,
            });
        }
        Ok(paired)
    }

    pub async fn connect_spp(
        &self,
        device_id: &DeviceId,
        uuid: &Uuid,
    ) -> Result<SppStreamInner, BluetoothError> {
        let (command_tx, command_rx) = async_channel::unbounded();
        let (connect_tx, connect_rx) = oneshot::channel::<Result<(), BluetoothError>>();
        let worker = spawn_spp_worker(device_id.as_str().to_string(), uuid.as_str().to_string(), command_rx, connect_tx)?;

        match connect_rx.await.map_err(|error| {
            BluetoothError::ConnectionFailed(format!("SPP connect callback dropped: {error}"))
        })? {
            Ok(()) => Ok(SppStreamInner {
                command_tx,
                worker: Mutex::new(Some(worker)),
            }),
            Err(error) => {
                std::thread::spawn(move || {
                    let _ = worker.join();
                });
                Err(error)
            }
        }
    }
}

fn spawn_spp_worker(
    device_id: String,
    service_uuid: String,
    command_rx: async_channel::Receiver<SppCommand>,
    connect_tx: oneshot::Sender<Result<(), BluetoothError>>,
) -> Result<JoinHandle<()>, BluetoothError> {
    std::thread::Builder::new()
        .name("waterkit-spp-linux".to_owned())
        .spawn(move || run_spp_worker(device_id, service_uuid, command_rx, connect_tx))
        .map_err(|error| BluetoothError::Platform(format!("spawn SPP worker failed: {error}")))
}

#[allow(clippy::needless_pass_by_value)]
fn run_spp_worker(
    device_id: String,
    service_uuid: String,
    command_rx: async_channel::Receiver<SppCommand>,
    connect_tx: oneshot::Sender<Result<(), BluetoothError>>,
) {
    let fd = match open_rfcomm_socket(&device_id, &service_uuid) {
        Ok(fd) => {
            let _ = connect_tx.send(Ok(()));
            fd
        }
        Err(error) => {
            let _ = connect_tx.send(Err(error));
            return;
        }
    };

    while let Ok(command) = command_rx.recv_blocking() {
        match command {
            SppCommand::Read { max_bytes, tx } => {
                let _ = tx.send(read_from_rfcomm(fd, max_bytes));
            }
            SppCommand::Write { data, tx } => {
                let _ = tx.send(write_to_rfcomm(fd, &data));
            }
            SppCommand::Close { tx } => {
                let _ = tx.send(());
                break;
            }
        }
    }

    let _ = unsafe { libc::close(fd) };
}

fn read_from_rfcomm(fd: i32, max_bytes: usize) -> Result<Vec<u8>, BluetoothError> {
    let mut buffer = vec![0u8; max_bytes];
    let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast::<libc::c_void>(), max_bytes) };
    if read < 0 {
        return Err(BluetoothError::ConnectionFailed(format!(
            "RFCOMM read failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    if read == 0 {
        return Err(BluetoothError::ConnectionFailed(
            "RFCOMM stream closed".into(),
        ));
    }
    let read_len = usize::try_from(read).map_err(|_| {
        BluetoothError::ConnectionFailed(format!("RFCOMM read size is negative: {read}"))
    })?;
    buffer.truncate(read_len);
    Ok(buffer)
}

fn write_to_rfcomm(fd: i32, data: &[u8]) -> Result<usize, BluetoothError> {
    let mut written_total = 0usize;
    while written_total < data.len() {
        let remaining = &data[written_total..];
        let written = unsafe {
            libc::write(
                fd,
                remaining.as_ptr().cast::<libc::c_void>(),
                remaining.len(),
            )
        };
        if written < 0 {
            return Err(BluetoothError::ConnectionFailed(format!(
                "RFCOMM write failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        let written_len = usize::try_from(written).map_err(|_| {
            BluetoothError::ConnectionFailed(format!(
                "RFCOMM write returned negative size: {written}"
            ))
        })?;
        if written_len == 0 {
            return Err(BluetoothError::ConnectionFailed(
                "RFCOMM write returned 0 bytes".into(),
            ));
        }
        written_total += written_len;
    }
    Ok(written_total)
}

#[derive(Debug)]
enum SppCommand {
    Read {
        max_bytes: usize,
        tx: oneshot::Sender<Result<Vec<u8>, BluetoothError>>,
    },
    Write {
        data: Vec<u8>,
        tx: oneshot::Sender<Result<usize, BluetoothError>>,
    },
    Close {
        tx: oneshot::Sender<()>,
    },
}

pub struct SppStreamInner {
    command_tx: async_channel::Sender<SppCommand>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for SppStreamInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SppStreamInner").finish()
    }
}

impl SppStreamInner {
    pub async fn read(&self, buf: &mut [u8]) -> Result<usize, BluetoothError> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(SppCommand::Read {
                max_bytes: buf.len(),
                tx,
            })
            .await
            .map_err(|error| {
                BluetoothError::ConnectionFailed(format!("SPP read command send failed: {error}"))
            })?;
        let data = rx.await.map_err(|error| {
            BluetoothError::ConnectionFailed(format!("SPP read response dropped: {error}"))
        })??;
        let read = data.len().min(buf.len());
        buf[..read].copy_from_slice(&data[..read]);
        Ok(read)
    }

    pub async fn write(&self, data: &[u8]) -> Result<usize, BluetoothError> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(SppCommand::Write {
                data: data.to_vec(),
                tx,
            })
            .await
            .map_err(|error| {
                BluetoothError::ConnectionFailed(format!("SPP write command send failed: {error}"))
            })?;
        rx.await.map_err(|error| {
            BluetoothError::ConnectionFailed(format!("SPP write response dropped: {error}"))
        })?
    }

    pub async fn close(self) {
        let (tx, rx) = oneshot::channel();
        let _ = self.command_tx.send(SppCommand::Close { tx }).await;
        let _ = rx.await;
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            std::thread::spawn(move || {
                let _ = worker.join();
            });
        }
    }
}

impl Drop for SppStreamInner {
    fn drop(&mut self) {
        let (tx, _rx) = oneshot::channel();
        let _ = self.command_tx.try_send(SppCommand::Close { tx });
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            std::thread::spawn(move || {
                let _ = worker.join();
            });
        }
    }
}
