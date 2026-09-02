use crate::{
    AdapterState, BluetoothDevice, BluetoothError, CharacteristicProperties, ClassicDevice,
    DeviceId, GattCharacteristic, GattService, ScanFilter, ScanResult, Uuid,
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
};
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic as WinGattCharacteristic, GattCharacteristicProperties,
    GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus,
    GattDeviceService, GattValueChangedEventArgs,
};
use windows::Devices::Bluetooth::Rfcomm::RfcommServiceId;
use windows::Devices::Bluetooth::{
    BluetoothAdapter, BluetoothDevice as WinBluetoothDevice, BluetoothLEDevice,
};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Devices::Radios::RadioState;
use windows::Foundation::TypedEventHandler;
use windows::Networking::Sockets::StreamSocket;
use windows::Storage::Streams::{DataReader, DataWriter};

fn guid_to_uuid(guid: windows::core::GUID) -> Uuid {
    Uuid::new(format!("{guid:?}"))
}

fn parse_guid(uuid: &Uuid) -> Result<windows::core::GUID, BluetoothError> {
    windows::core::GUID::try_from(uuid.as_str())
        .map_err(|_| BluetoothError::GattError("Invalid UUID".into()))
}

fn connection_failed(message: impl Into<String>) -> BluetoothError {
    BluetoothError::ConnectionFailed(message.into())
}

fn connection_failed_with_context(context: &str, error: impl std::fmt::Display) -> BluetoothError {
    connection_failed(format!("{context}: {error}"))
}

fn device_from_info(info: &DeviceInformation, paired: bool) -> ClassicDevice {
    let device_id = info
        .Id()
        .map_or_else(|_| String::new(), |value| value.to_string());
    let name = info
        .Name()
        .map(|value| value.to_string())
        .ok()
        .filter(|value| !value.is_empty());
    ClassicDevice {
        device: BluetoothDevice {
            id: DeviceId::new(device_id),
            name,
            rssi: None,
            is_connected: false,
        },
        device_class: 0,
        is_paired: paired,
    }
}

pub async fn adapter_state() -> Result<AdapterState, BluetoothError> {
    let adapter = BluetoothAdapter::GetDefaultAsync()
        .map_err(|e| BluetoothError::Platform(e.to_string()))?
        .await
        .map_err(|e| BluetoothError::Platform(e.to_string()))?;
    let radio = adapter
        .GetRadioAsync()
        .map_err(|e| BluetoothError::Platform(e.to_string()))?
        .await
        .map_err(|e| BluetoothError::Platform(e.to_string()))?;
    match radio
        .State()
        .map_err(|e| BluetoothError::Platform(e.to_string()))?
    {
        RadioState::On => Ok(AdapterState::PoweredOn),
        RadioState::Off => Ok(AdapterState::PoweredOff),
        RadioState::Disabled => Ok(AdapterState::Unavailable),
        _ => Ok(AdapterState::Unknown),
    }
}

#[derive(Debug)]
pub struct BleScannerInner {
    watcher: BluetoothLEAdvertisementWatcher,
}

impl BleScannerInner {
    pub async fn new() -> Result<Self, BluetoothError> {
        let state = adapter_state().await?;
        if state != AdapterState::PoweredOn {
            return Err(BluetoothError::NotAvailable);
        }
        let watcher = BluetoothLEAdvertisementWatcher::new()
            .map_err(|e| BluetoothError::Platform(e.to_string()))?;
        Ok(Self { watcher })
    }

    pub fn start_scan(
        &self,
        _filter: &ScanFilter,
    ) -> Result<async_channel::Receiver<ScanResult>, BluetoothError> {
        let (tx, rx) = async_channel::bounded(64);
        self.watcher
            .Received(&TypedEventHandler::<
                BluetoothLEAdvertisementWatcher,
                BluetoothLEAdvertisementReceivedEventArgs,
            >::new(move |_, args| {
                let Some(args) = args.as_ref() else {
                    return Ok(());
                };
                let addr = args.BluetoothAddress().unwrap_or(0);
                let rssi = args.RawSignalStrengthInDBm().unwrap_or(0);
                let device_id = format!("{addr:012X}");
                let mut service_uuids = Vec::new();
                if let Ok(adv) = args.Advertisement()
                    && let Ok(uuids) = adv.ServiceUuids()
                {
                    for uuid in &uuids {
                        service_uuids.push(guid_to_uuid(uuid));
                    }
                }
                let result = ScanResult {
                    device: BluetoothDevice {
                        id: DeviceId::new(device_id),
                        name: args
                            .Advertisement()
                            .ok()
                            .and_then(|a| a.LocalName().ok())
                            .map(|n| n.to_string())
                            .filter(|n| !n.is_empty()),
                        rssi: Some(rssi),
                        is_connected: false,
                    },
                    service_uuids,
                    manufacturer_data: HashMap::new(),
                };
                let _ = tx.try_send(result);
                Ok(())
            }))
            .map_err(|e| BluetoothError::Platform(e.to_string()))?;
        self.watcher
            .Start()
            .map_err(|e| BluetoothError::Platform(e.to_string()))?;
        Ok(rx)
    }

    pub fn stop_scan(&self) {
        let _ = self.watcher.Stop();
    }
}

#[derive(Debug)]
pub struct BleConnectionInner {
    device: BluetoothLEDevice,
    subscriptions: Mutex<Vec<SubscriptionState>>,
}

#[derive(Debug)]
struct SubscriptionState {
    characteristic: WinGattCharacteristic,
    token: i64,
}

impl BleConnectionInner {
    pub async fn connect(device_id: &DeviceId) -> Result<Self, BluetoothError> {
        let addr = u64::from_str_radix(device_id.as_str(), 16)
            .map_err(|e| BluetoothError::DeviceNotFound(e.to_string()))?;
        let device = BluetoothLEDevice::FromBluetoothAddressAsync(addr)
            .map_err(|e| BluetoothError::ConnectionFailed(e.to_string()))?
            .await
            .map_err(|e| BluetoothError::ConnectionFailed(e.to_string()))?;
        Ok(Self {
            device,
            subscriptions: Mutex::new(Vec::new()),
        })
    }

    #[allow(clippy::future_not_send)]
    pub async fn discover_services(&self) -> Result<Vec<GattService>, BluetoothError> {
        let result = self
            .device
            .GetGattServicesAsync()
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        if result
            .Status()
            .unwrap_or(GattCommunicationStatus::Unreachable)
            != GattCommunicationStatus::Success
        {
            return Err(BluetoothError::GattError("Service discovery failed".into()));
        }
        let services = result
            .Services()
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let mut out = Vec::new();
        for svc in &services {
            let uuid = Uuid::new(
                svc.Uuid()
                    .map_or_else(|_| String::new(), |u| format!("{u:?}")),
            );
            let chars_result = svc
                .GetCharacteristicsAsync()
                .map_err(|e| BluetoothError::GattError(e.to_string()))?
                .await
                .map_err(|e| BluetoothError::GattError(e.to_string()))?;
            let mut characteristics = Vec::new();
            if let Ok(chars) = chars_result.Characteristics() {
                for c in &chars {
                    let cuuid = Uuid::new(
                        c.Uuid()
                            .map_or_else(|_| String::new(), |u| format!("{u:?}")),
                    );
                    let props = c
                        .CharacteristicProperties()
                        .unwrap_or(GattCharacteristicProperties::None);
                    characteristics.push(GattCharacteristic {
                        uuid: cuuid,
                        properties: CharacteristicProperties {
                            read: props.contains(GattCharacteristicProperties::Read),
                            write: props.contains(GattCharacteristicProperties::Write),
                            write_without_response: props
                                .contains(GattCharacteristicProperties::WriteWithoutResponse),
                            notify: props.contains(GattCharacteristicProperties::Notify),
                            indicate: props.contains(GattCharacteristicProperties::Indicate),
                        },
                    });
                }
            }
            out.push(GattService {
                uuid,
                is_primary: true,
                characteristics,
            });
        }
        Ok(out)
    }

    pub async fn read_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<Vec<u8>, BluetoothError> {
        let svc = self.find_service(service).await?;
        let chr = self.find_characteristic(&svc, characteristic).await?;
        let result = chr
            .ReadValueAsync()
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        if result
            .Status()
            .unwrap_or(GattCommunicationStatus::Unreachable)
            != GattCommunicationStatus::Success
        {
            return Err(BluetoothError::GattError("Read failed".into()));
        }
        let buf = result
            .Value()
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let reader = windows::Storage::Streams::DataReader::FromBuffer(&buf)
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let len = reader
            .UnconsumedBufferLength()
            .map_err(|e| BluetoothError::GattError(e.to_string()))? as usize;
        let mut data = vec![0u8; len];
        reader
            .ReadBytes(&mut data)
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        Ok(data)
    }

    #[allow(clippy::future_not_send)]
    pub async fn write_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
        data: &[u8],
    ) -> Result<(), BluetoothError> {
        let svc = self.find_service(service).await?;
        let chr = self.find_characteristic(&svc, characteristic).await?;
        let writer = windows::Storage::Streams::DataWriter::new()
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        writer
            .WriteBytes(data)
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let buf = writer
            .DetachBuffer()
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let result = chr
            .WriteValueAsync(&buf)
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        if result != GattCommunicationStatus::Success {
            return Err(BluetoothError::GattError("Write failed".into()));
        }
        Ok(())
    }

    #[allow(clippy::future_not_send)]
    async fn find_subscription_characteristic(
        device: &BluetoothLEDevice,
        service_guid: windows::core::GUID,
        characteristic_guid: windows::core::GUID,
    ) -> Result<WinGattCharacteristic, BluetoothError> {
        let service_operation =
            device
                .GetGattServicesForUuidAsync(service_guid)
                .map_err(|error| {
                    connection_failed_with_context("start GATT service query failed", error)
                })?;
        let service_result = service_operation
            .await
            .map_err(|error| connection_failed_with_context("GATT service query failed", error))?;
        if service_result
            .Status()
            .unwrap_or(GattCommunicationStatus::Unreachable)
            != GattCommunicationStatus::Success
        {
            return Err(connection_failed(
                "GATT service query returned unsuccessful status",
            ));
        }

        let services = service_result
            .Services()
            .map_err(|error| connection_failed_with_context("load GATT services failed", error))?;
        let service = services
            .First()
            .and_then(|iter| iter.Current())
            .map_err(|_| connection_failed("GATT service not found"))?;

        let characteristic_operation = service
            .GetCharacteristicsForUuidAsync(characteristic_guid)
            .map_err(|error| {
                connection_failed_with_context("start GATT characteristic query failed", error)
            })?;
        let chars_result = characteristic_operation.await.map_err(|error| {
            connection_failed_with_context("GATT characteristic query failed", error)
        })?;
        if chars_result
            .Status()
            .unwrap_or(GattCommunicationStatus::Unreachable)
            != GattCommunicationStatus::Success
        {
            return Err(connection_failed(
                "GATT characteristic query returned unsuccessful status",
            ));
        }

        let characteristics = chars_result.Characteristics().map_err(|error| {
            connection_failed_with_context("load GATT characteristics failed", error)
        })?;
        characteristics
            .First()
            .and_then(|iter| iter.Current())
            .map_err(|_| connection_failed("GATT characteristic not found"))
    }

    fn register_value_changed_handler(
        characteristic: &WinGattCharacteristic,
        tx: async_channel::Sender<Vec<u8>>,
    ) -> Result<i64, BluetoothError> {
        characteristic
            .ValueChanged(&TypedEventHandler::<
                WinGattCharacteristic,
                GattValueChangedEventArgs,
            >::new(move |_, args| {
                let Some(args) = args.as_ref() else {
                    return Ok(());
                };
                let Ok(buffer) = args.CharacteristicValue() else {
                    return Ok(());
                };
                let Ok(reader) = DataReader::FromBuffer(&buffer) else {
                    return Ok(());
                };
                let Ok(len) = reader.UnconsumedBufferLength() else {
                    return Ok(());
                };
                let mut bytes = vec![0u8; len as usize];
                if reader.ReadBytes(&mut bytes).is_ok() {
                    let _ = tx.try_send(bytes);
                }
                Ok(())
            }))
            .map_err(|error| {
                connection_failed_with_context("register notification handler failed", error)
            })
    }

    async fn set_notification_state(
        characteristic: &WinGattCharacteristic,
        mode: GattClientCharacteristicConfigurationDescriptorValue,
    ) -> Result<GattCommunicationStatus, BluetoothError> {
        let operation = characteristic
            .WriteClientCharacteristicConfigurationDescriptorAsync(mode)
            .map_err(|error| {
                connection_failed_with_context("start notification state write failed", error)
            })?;
        operation.await.map_err(|error| {
            connection_failed_with_context("notification state write failed", error)
        })
    }

    async fn enable_notifications(
        characteristic: &WinGattCharacteristic,
    ) -> Result<bool, BluetoothError> {
        let notify_status = Self::set_notification_state(
            characteristic,
            GattClientCharacteristicConfigurationDescriptorValue::Notify,
        )
        .await;
        if notify_status? == GattCommunicationStatus::Success {
            return Ok(true);
        }

        Ok(Self::set_notification_state(
            characteristic,
            GattClientCharacteristicConfigurationDescriptorValue::Indicate,
        )
        .await?
            == GattCommunicationStatus::Success)
    }

    #[allow(clippy::future_not_send)]
    pub async fn subscribe(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<async_channel::Receiver<Vec<u8>>, BluetoothError> {
        let service_guid = parse_guid(service)?;
        let characteristic_guid = parse_guid(characteristic)?;
        let target =
            Self::find_subscription_characteristic(&self.device, service_guid, characteristic_guid)
                .await?;
        let (tx, rx) = async_channel::bounded(64);
        let token = Self::register_value_changed_handler(&target, tx)?;
        if !Self::enable_notifications(&target).await? {
            let _ = target.RemoveValueChanged(token);
            return Err(connection_failed(
                "BLE characteristic does not support notifications",
            ));
        }
        self.subscriptions
            .lock()
            .map_err(|error| {
                BluetoothError::Platform(format!(
                    "BLE subscription registry mutex poisoned: {error}"
                ))
            })?
            .push(SubscriptionState {
                characteristic: target,
                token,
            });

        Ok(rx)
    }

    async fn teardown_subscriptions(&self) {
        let states = if let Ok(mut guard) = self.subscriptions.lock() {
            std::mem::take(&mut *guard)
        } else {
            return;
        };
        for state in states {
            let _ = Self::set_notification_state(
                &state.characteristic,
                GattClientCharacteristicConfigurationDescriptorValue::None,
            )
            .await;
            let _ = state.characteristic.RemoveValueChanged(state.token);
        }
    }

    pub async fn disconnect(self) {
        self.teardown_subscriptions().await;
        drop(self.device);
    }

    async fn find_service(&self, uuid: &Uuid) -> Result<GattDeviceService, BluetoothError> {
        let guid = parse_guid(uuid)?;
        let result = self
            .device
            .GetGattServicesForUuidAsync(guid)
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let services = result
            .Services()
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        services
            .First()
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .Current()
            .map_err(|_| BluetoothError::GattError("Service not found".into()))
    }

    async fn find_characteristic(
        &self,
        service: &GattDeviceService,
        uuid: &Uuid,
    ) -> Result<
        windows::Devices::Bluetooth::GenericAttributeProfile::GattCharacteristic,
        BluetoothError,
    > {
        let guid = parse_guid(uuid)?;
        let result = service
            .GetCharacteristicsForUuidAsync(guid)
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .await
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        let chars = result
            .Characteristics()
            .map_err(|e| BluetoothError::GattError(e.to_string()))?;
        chars
            .First()
            .map_err(|e| BluetoothError::GattError(e.to_string()))?
            .Current()
            .map_err(|_| BluetoothError::GattError("Characteristic not found".into()))
    }
}

#[derive(Debug)]
pub struct ClassicBluetoothInner {
    discovering: AtomicBool,
}

impl ClassicBluetoothInner {
    pub async fn new() -> Result<Self, BluetoothError> {
        let state = adapter_state().await?;
        if state != AdapterState::PoweredOn {
            return Err(BluetoothError::NotAvailable);
        }
        Ok(Self {
            discovering: AtomicBool::new(false),
        })
    }

    pub async fn start_discovery(
        &self,
    ) -> Result<async_channel::Receiver<ClassicDevice>, BluetoothError> {
        self.discovering.store(true, Ordering::Relaxed);
        let selector = WinBluetoothDevice::GetDeviceSelector().map_err(|error| {
            connection_failed_with_context("get classic selector failed", error)
        })?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|error| {
                connection_failed_with_context("start classic discovery failed", error)
            })?
            .await
            .map_err(|error| connection_failed_with_context("classic discovery failed", error))?;

        let (tx, rx) = async_channel::bounded(64);
        for info in &devices {
            let paired = info
                .Pairing()
                .and_then(|pairing| pairing.IsPaired())
                .unwrap_or(false);
            if tx.try_send(device_from_info(&info, paired)).is_err() {
                break;
            }
        }
        Ok(rx)
    }

    #[allow(clippy::unused_async)]
    pub async fn stop_discovery(&self) {
        self.discovering.store(false, Ordering::Relaxed);
    }

    pub async fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        let selector = WinBluetoothDevice::GetDeviceSelectorFromPairingState(true)
            .map_err(|e| BluetoothError::Platform(e.to_string()))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|e| BluetoothError::Platform(e.to_string()))?
            .await
            .map_err(|e| BluetoothError::Platform(e.to_string()))?;
        let mut paired = Vec::new();
        for info in &devices {
            paired.push(device_from_info(&info, true));
        }
        Ok(paired)
    }

    pub async fn connect_spp(
        &self,
        device_id: &DeviceId,
        uuid: &Uuid,
    ) -> Result<SppStreamInner, BluetoothError> {
        let _ = self.discovering.load(Ordering::Relaxed);
        let service = resolve_rfcomm_service(device_id.as_str(), uuid.as_str()).await?;
        let host = service
            .ConnectionHostName()
            .map_err(|error| connection_failed_with_context("get RFCOMM host failed", error))?;
        let service_name = service.ConnectionServiceName().map_err(|error| {
            connection_failed_with_context("get RFCOMM service name failed", error)
        })?;

        let socket = StreamSocket::new().map_err(|error| {
            connection_failed_with_context("create stream socket failed", error)
        })?;
        socket
            .ConnectAsync(&host, &service_name)
            .map_err(|error| {
                connection_failed_with_context("start RFCOMM socket connect failed", error)
            })?
            .await
            .map_err(|error| {
                connection_failed_with_context("connect RFCOMM socket failed", error)
            })?;

        Ok(SppStreamInner { socket })
    }
}

async fn resolve_rfcomm_service(
    device_id: &str,
    service_uuid: &str,
) -> Result<windows::Devices::Bluetooth::Rfcomm::RfcommDeviceService, BluetoothError> {
    let guid = windows::core::GUID::try_from(service_uuid)
        .map_err(|_| connection_failed("invalid SPP UUID"))?;
    let service_id = RfcommServiceId::FromUuid(guid).map_err(|error| {
        connection_failed_with_context("create RFCOMM service id failed", error)
    })?;

    let device = WinBluetoothDevice::FromIdAsync(&windows::core::HSTRING::from(device_id))
        .map_err(|error| {
            connection_failed_with_context("start Bluetooth device resolve failed", error)
        })?
        .await
        .map_err(|error| {
            connection_failed_with_context("resolve Bluetooth device failed", error)
        })?;

    let services_result = device
        .GetRfcommServicesForIdAsync(&service_id)
        .map_err(|error| connection_failed_with_context("start RFCOMM query failed", error))?
        .await
        .map_err(|error| connection_failed_with_context("query RFCOMM services failed", error))?;

    let services = services_result
        .Services()
        .map_err(|error| connection_failed_with_context("load RFCOMM services failed", error))?;
    services
        .First()
        .and_then(|iter| iter.Current())
        .map_err(|_| connection_failed("RFCOMM service not found"))
}

#[allow(clippy::future_not_send)]
async fn read_spp_bytes(
    socket: &StreamSocket,
    max_bytes: usize,
) -> Result<Vec<u8>, BluetoothError> {
    let input_stream = socket
        .InputStream()
        .map_err(|error| connection_failed_with_context("get RFCOMM input stream failed", error))?;
    let reader = DataReader::CreateDataReader(&input_stream)
        .map_err(|error| connection_failed_with_context("create RFCOMM reader failed", error))?;
    let request_len = u32::try_from(max_bytes)
        .map_err(|_| connection_failed(format!("read size exceeds u32: {max_bytes}")))?;
    let loaded = reader
        .LoadAsync(request_len)
        .map_err(|error| connection_failed_with_context("start RFCOMM read failed", error))?
        .await
        .map_err(|error| connection_failed_with_context("RFCOMM read failed", error))?;
    if loaded == 0 {
        return Err(connection_failed("RFCOMM stream closed"));
    }

    let mut data = vec![0u8; loaded as usize];
    reader.ReadBytes(&mut data).map_err(|error| {
        connection_failed_with_context("decode RFCOMM read bytes failed", error)
    })?;
    Ok(data)
}

#[allow(clippy::future_not_send)]
async fn write_spp_bytes(socket: &StreamSocket, data: &[u8]) -> Result<usize, BluetoothError> {
    let output_stream = socket.OutputStream().map_err(|error| {
        connection_failed_with_context("get RFCOMM output stream failed", error)
    })?;
    let writer = DataWriter::CreateDataWriter(&output_stream)
        .map_err(|error| connection_failed_with_context("create RFCOMM writer failed", error))?;
    writer.WriteBytes(data).map_err(|error| {
        connection_failed_with_context("encode RFCOMM write bytes failed", error)
    })?;
    let stored = writer
        .StoreAsync()
        .map_err(|error| connection_failed_with_context("start RFCOMM write failed", error))?
        .await
        .map_err(|error| connection_failed_with_context("RFCOMM write failed", error))?;
    let _ = writer
        .FlushAsync()
        .map_err(|error| connection_failed_with_context("start RFCOMM flush failed", error))?
        .await
        .map_err(|error| connection_failed_with_context("RFCOMM flush failed", error))?;
    usize::try_from(stored)
        .map_err(|_| connection_failed(format!("RFCOMM write size exceeds usize: {stored}")))
}

pub struct SppStreamInner {
    socket: StreamSocket,
}

impl std::fmt::Debug for SppStreamInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SppStreamInner").finish()
    }
}

impl SppStreamInner {
    #[allow(clippy::future_not_send)]
    pub async fn read(&self, buf: &mut [u8]) -> Result<usize, BluetoothError> {
        let data = read_spp_bytes(&self.socket, buf.len()).await?;
        let read = data.len().min(buf.len());
        buf[..read].copy_from_slice(&data[..read]);
        Ok(read)
    }

    #[allow(clippy::future_not_send)]
    pub async fn write(&self, data: &[u8]) -> Result<usize, BluetoothError> {
        write_spp_bytes(&self.socket, data).await
    }

    #[allow(clippy::unused_async)]
    pub async fn close(self) {
        drop(self.socket);
    }
}
