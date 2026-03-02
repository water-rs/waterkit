use crate::{
    AdapterState, BluetoothDevice, BluetoothError, CharacteristicProperties, ClassicDevice,
    DeviceId, GattCharacteristic, GattService, ScanFilter, ScanResult, Uuid,
};
use futures::channel::oneshot;
use std::collections::HashMap;
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;
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
    Uuid(format!("{guid:?}"))
}

fn parse_guid(uuid: &Uuid) -> Result<windows::core::GUID, BluetoothError> {
    windows::core::GUID::try_from(uuid.0.as_str())
        .map_err(|_| BluetoothError::GattError("Invalid UUID".into()))
}

fn device_from_info(info: &DeviceInformation, paired: bool) -> ClassicDevice {
    let device_id = info
        .Id()
        .map(|value| value.to_string())
        .unwrap_or_else(|_| String::new());
    let name = info
        .Name()
        .map(|value| value.to_string())
        .ok()
        .filter(|value| !value.is_empty());
    ClassicDevice {
        device: BluetoothDevice {
            id: DeviceId(device_id),
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
        .map_err(|e| BluetoothError::PlatformError(e.to_string()))?
        .await
        .map_err(|e| BluetoothError::PlatformError(e.to_string()))?;
    let radio = adapter
        .GetRadioAsync()
        .map_err(|e| BluetoothError::PlatformError(e.to_string()))?
        .await
        .map_err(|e| BluetoothError::PlatformError(e.to_string()))?;
    match radio
        .State()
        .map_err(|e| BluetoothError::PlatformError(e.to_string()))?
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
            .map_err(|e| BluetoothError::PlatformError(e.to_string()))?;
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
                        id: DeviceId(device_id),
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
            .map_err(|e| BluetoothError::PlatformError(e.to_string()))?;
        self.watcher
            .Start()
            .map_err(|e| BluetoothError::PlatformError(e.to_string()))?;
        Ok(rx)
    }

    pub fn stop_scan(&self) {
        let _ = self.watcher.Stop();
    }
}

#[derive(Debug)]
pub struct BleConnectionInner {
    device: BluetoothLEDevice,
}

impl BleConnectionInner {
    pub async fn connect(device_id: &DeviceId) -> Result<Self, BluetoothError> {
        let addr = u64::from_str_radix(&device_id.0, 16)
            .map_err(|e| BluetoothError::DeviceNotFound(e.to_string()))?;
        let device = BluetoothLEDevice::FromBluetoothAddressAsync(addr)
            .map_err(|e| BluetoothError::ConnectionFailed(e.to_string()))?
            .await
            .map_err(|e| BluetoothError::ConnectionFailed(e.to_string()))?;
        Ok(Self { device })
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
            let uuid = Uuid(
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
                    let cuuid = Uuid(
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

    pub fn subscribe(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<async_channel::Receiver<Vec<u8>>, BluetoothError> {
        let service_guid = parse_guid(service)?;
        let characteristic_guid = parse_guid(characteristic)?;
        let device = self.device.clone();
        let (tx, rx) = async_channel::bounded(64);

        std::thread::spawn(move || {
            futures::executor::block_on(async move {
                let service_result = match device.GetGattServicesForUuidAsync(service_guid) {
                    Ok(op) => op.await,
                    Err(_) => return,
                };
                let Ok(service_result) = service_result else {
                    return;
                };
                if service_result
                    .Status()
                    .unwrap_or(GattCommunicationStatus::Unreachable)
                    != GattCommunicationStatus::Success
                {
                    return;
                }
                let services = match service_result.Services() {
                    Ok(services) => services,
                    Err(_) => return,
                };
                let service = match services.First().and_then(|iter| iter.Current()) {
                    Ok(service) => service,
                    Err(_) => return,
                };

                let chars_result = match service.GetCharacteristicsForUuidAsync(characteristic_guid)
                {
                    Ok(op) => op.await,
                    Err(_) => return,
                };
                let Ok(chars_result) = chars_result else {
                    return;
                };
                if chars_result
                    .Status()
                    .unwrap_or(GattCommunicationStatus::Unreachable)
                    != GattCommunicationStatus::Success
                {
                    return;
                }
                let characteristics = match chars_result.Characteristics() {
                    Ok(characteristics) => characteristics,
                    Err(_) => return,
                };
                let characteristic = match characteristics.First().and_then(|iter| iter.Current()) {
                    Ok(characteristic) => characteristic,
                    Err(_) => return,
                };

                let event_tx = tx.clone();
                let token = match characteristic.ValueChanged(&TypedEventHandler::<
                    WinGattCharacteristic,
                    GattValueChangedEventArgs,
                >::new(
                    move |_, args| {
                        let Some(args) = args.as_ref() else {
                            return Ok(());
                        };
                        let Ok(buffer) = args.CharacteristicValue() else {
                            return Ok(());
                        };
                        let Ok(reader) = windows::Storage::Streams::DataReader::FromBuffer(&buffer)
                        else {
                            return Ok(());
                        };
                        let Ok(len) = reader.UnconsumedBufferLength() else {
                            return Ok(());
                        };
                        let mut bytes = vec![0u8; len as usize];
                        if reader.ReadBytes(&mut bytes).is_ok() {
                            let _ = event_tx.try_send(bytes);
                        }
                        Ok(())
                    },
                )) {
                    Ok(token) => token,
                    Err(_) => return,
                };

                let notify_status = match characteristic
                    .WriteClientCharacteristicConfigurationDescriptorAsync(
                        GattClientCharacteristicConfigurationDescriptorValue::Notify,
                    ) {
                    Ok(op) => op.await.unwrap_or(GattCommunicationStatus::Unreachable),
                    Err(_) => GattCommunicationStatus::Unreachable,
                };

                let final_status = if notify_status == GattCommunicationStatus::Success {
                    notify_status
                } else {
                    match characteristic.WriteClientCharacteristicConfigurationDescriptorAsync(
                        GattClientCharacteristicConfigurationDescriptorValue::Indicate,
                    ) {
                        Ok(op) => op.await.unwrap_or(GattCommunicationStatus::Unreachable),
                        Err(_) => GattCommunicationStatus::Unreachable,
                    }
                };

                if final_status != GattCommunicationStatus::Success {
                    let _ = characteristic.RemoveValueChanged(token);
                    return;
                }

                while !tx.is_closed() {
                    std::thread::sleep(Duration::from_millis(200));
                }

                if let Ok(op) = characteristic
                    .WriteClientCharacteristicConfigurationDescriptorAsync(
                        GattClientCharacteristicConfigurationDescriptorValue::None,
                    )
                {
                    let _ = op.await;
                }
                let _ = characteristic.RemoveValueChanged(token);
            });
        });

        Ok(rx)
    }

    #[allow(clippy::unused_async)]
    pub async fn disconnect(self) {
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
pub struct ClassicBluetoothInner;

impl ClassicBluetoothInner {
    pub async fn new() -> Result<Self, BluetoothError> {
        let state = adapter_state().await?;
        if state != AdapterState::PoweredOn {
            return Err(BluetoothError::NotAvailable);
        }
        Ok(Self)
    }

    pub fn start_discovery(
        &self,
    ) -> Result<async_channel::Receiver<ClassicDevice>, BluetoothError> {
        let (tx, rx) = async_channel::bounded(64);
        std::thread::spawn(move || {
            futures::executor::block_on(async move {
                let selector = match WinBluetoothDevice::GetDeviceSelector() {
                    Ok(selector) => selector,
                    Err(_) => return,
                };
                let devices = match DeviceInformation::FindAllAsyncAqsFilter(&selector) {
                    Ok(op) => op.await,
                    Err(_) => return,
                };
                let Ok(devices) = devices else {
                    return;
                };
                for info in &devices {
                    let paired = info
                        .Pairing()
                        .and_then(|pairing| pairing.IsPaired())
                        .unwrap_or(false);
                    if tx.try_send(device_from_info(&info, paired)).is_err() {
                        break;
                    }
                }
            });
        });
        Ok(rx)
    }

    pub fn stop_discovery(&self) {
        let _ = self;
    }

    pub async fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        let selector = WinBluetoothDevice::GetDeviceSelectorFromPairingState(true)
            .map_err(|e| BluetoothError::PlatformError(e.to_string()))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|e| BluetoothError::PlatformError(e.to_string()))?
            .await
            .map_err(|e| BluetoothError::PlatformError(e.to_string()))?;
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
        let device_id = device_id.0.clone();
        let service_uuid = uuid.0.clone();
        let (command_tx, command_rx) = async_channel::unbounded();
        let (connect_tx, connect_rx) = oneshot::channel::<Result<(), BluetoothError>>();

        let worker = std::thread::Builder::new()
            .name("waterkit-spp-windows".to_owned())
            .spawn(move || {
                futures::executor::block_on(async move {
                    let guid = match windows::core::GUID::try_from(service_uuid.as_str()) {
                        Ok(guid) => guid,
                        Err(_) => {
                            let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                "invalid SPP UUID".into(),
                            )));
                            return;
                        }
                    };
                    let service_id = match RfcommServiceId::FromUuid(guid) {
                        Ok(service_id) => service_id,
                        Err(error) => {
                            let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                format!("create RFCOMM service id failed: {error}"),
                            )));
                            return;
                        }
                    };

                    let device = match WinBluetoothDevice::FromIdAsync(
                        &windows::core::HSTRING::from(device_id),
                    ) {
                        Ok(op) => match op.await {
                            Ok(device) => device,
                            Err(error) => {
                                let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                    format!("resolve Bluetooth device failed: {error}"),
                                )));
                                return;
                            }
                        },
                        Err(error) => {
                            let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                format!("start Bluetooth device resolve failed: {error}"),
                            )));
                            return;
                        }
                    };

                    let services_result = match device.GetRfcommServicesForIdAsync(&service_id) {
                        Ok(op) => match op.await {
                            Ok(result) => result,
                            Err(error) => {
                                let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                    format!("query RFCOMM services failed: {error}"),
                                )));
                                return;
                            }
                        },
                        Err(error) => {
                            let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                format!("start RFCOMM query failed: {error}"),
                            )));
                            return;
                        }
                    };

                    let services = match services_result.Services() {
                        Ok(services) => services,
                        Err(error) => {
                            let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                format!("load RFCOMM services failed: {error}"),
                            )));
                            return;
                        }
                    };
                    let service = match services.First().and_then(|iter| iter.Current()) {
                        Ok(service) => service,
                        Err(_) => {
                            let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                "RFCOMM service not found".into(),
                            )));
                            return;
                        }
                    };

                    let host = match service.ConnectionHostName() {
                        Ok(host) => host,
                        Err(error) => {
                            let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                format!("get RFCOMM host failed: {error}"),
                            )));
                            return;
                        }
                    };
                    let service_name = match service.ConnectionServiceName() {
                        Ok(name) => name,
                        Err(error) => {
                            let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                format!("get RFCOMM service name failed: {error}"),
                            )));
                            return;
                        }
                    };

                    let socket = match StreamSocket::new() {
                        Ok(socket) => socket,
                        Err(error) => {
                            let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                format!("create stream socket failed: {error}"),
                            )));
                            return;
                        }
                    };
                    let connect_result = match socket.ConnectAsync(&host, &service_name) {
                        Ok(op) => op.await,
                        Err(error) => {
                            let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(
                                format!("start RFCOMM socket connect failed: {error}"),
                            )));
                            return;
                        }
                    };
                    if let Err(error) = connect_result {
                        let _ = connect_tx.send(Err(BluetoothError::ConnectionFailed(format!(
                            "connect RFCOMM socket failed: {error}"
                        ))));
                        return;
                    }
                    let _ = connect_tx.send(Ok(()));

                    while let Ok(command) = command_rx.recv_blocking() {
                        match command {
                            SppCommand::Read { max_bytes, tx } => {
                                let result = async {
                                    let input_stream = socket.InputStream().map_err(|error| {
                                        BluetoothError::ConnectionFailed(format!(
                                            "get RFCOMM input stream failed: {error}"
                                        ))
                                    })?;
                                    let reader = DataReader::CreateDataReader(&input_stream)
                                        .map_err(|error| {
                                            BluetoothError::ConnectionFailed(format!(
                                                "create RFCOMM reader failed: {error}"
                                            ))
                                        })?;
                                    let request_len = u32::try_from(max_bytes).map_err(|_| {
                                        BluetoothError::ConnectionFailed(format!(
                                            "read size exceeds u32: {max_bytes}"
                                        ))
                                    })?;
                                    let loaded =
                                        reader.LoadAsync(request_len).map_err(|error| {
                                            BluetoothError::ConnectionFailed(format!(
                                                "start RFCOMM read failed: {error}"
                                            ))
                                        })?;
                                    let loaded = loaded.await.map_err(|error| {
                                        BluetoothError::ConnectionFailed(format!(
                                            "RFCOMM read failed: {error}"
                                        ))
                                    })?;
                                    if loaded == 0 {
                                        return Err(BluetoothError::ConnectionFailed(
                                            "RFCOMM stream closed".into(),
                                        ));
                                    }
                                    let mut data = vec![0u8; loaded as usize];
                                    reader.ReadBytes(&mut data).map_err(|error| {
                                        BluetoothError::ConnectionFailed(format!(
                                            "decode RFCOMM read bytes failed: {error}"
                                        ))
                                    })?;
                                    Ok::<Vec<u8>, BluetoothError>(data)
                                }
                                .await;
                                let _ = tx.send(result);
                            }
                            SppCommand::Write { data, tx } => {
                                let result = async {
                                    let output_stream = socket.OutputStream().map_err(|error| {
                                        BluetoothError::ConnectionFailed(format!(
                                            "get RFCOMM output stream failed: {error}"
                                        ))
                                    })?;
                                    let writer = DataWriter::CreateDataWriter(&output_stream)
                                        .map_err(|error| {
                                            BluetoothError::ConnectionFailed(format!(
                                                "create RFCOMM writer failed: {error}"
                                            ))
                                        })?;
                                    writer.WriteBytes(&data).map_err(|error| {
                                        BluetoothError::ConnectionFailed(format!(
                                            "encode RFCOMM write bytes failed: {error}"
                                        ))
                                    })?;
                                    let stored = writer.StoreAsync().map_err(|error| {
                                        BluetoothError::ConnectionFailed(format!(
                                            "start RFCOMM write failed: {error}"
                                        ))
                                    })?;
                                    let stored = stored.await.map_err(|error| {
                                        BluetoothError::ConnectionFailed(format!(
                                            "RFCOMM write failed: {error}"
                                        ))
                                    })?;
                                    let flush = writer.FlushAsync().map_err(|error| {
                                        BluetoothError::ConnectionFailed(format!(
                                            "start RFCOMM flush failed: {error}"
                                        ))
                                    })?;
                                    let _ = flush.await.map_err(|error| {
                                        BluetoothError::ConnectionFailed(format!(
                                            "RFCOMM flush failed: {error}"
                                        ))
                                    })?;
                                    usize::try_from(stored).map_err(|_| {
                                        BluetoothError::ConnectionFailed(format!(
                                            "RFCOMM write size exceeds usize: {stored}"
                                        ))
                                    })
                                }
                                .await;
                                let _ = tx.send(result);
                            }
                            SppCommand::Close { tx } => {
                                let _ = tx.send(());
                                break;
                            }
                        }
                    }

                    drop(socket);
                });
            })
            .map_err(|error| {
                BluetoothError::PlatformError(format!("spawn SPP worker failed: {error}"))
            })?;

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
