use crate::{
    AdapterState, BluetoothDevice, BluetoothError, CharacteristicProperties, ClassicDevice,
    DeviceId, GattCharacteristic, GattService, ScanFilter, ScanResult, Uuid,
};
use std::collections::HashMap;
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
};
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristicProperties, GattCommunicationStatus, GattDeviceService,
};
use windows::Devices::Bluetooth::{BluetoothAdapter, BluetoothLEDevice};
use windows::Devices::Radios::RadioState;
use windows::Foundation::TypedEventHandler;

fn guid_to_uuid(guid: windows::core::GUID) -> Uuid {
    Uuid(format!("{guid:?}"))
}

fn parse_guid(uuid: &Uuid) -> Result<windows::core::GUID, BluetoothError> {
    windows::core::GUID::try_from(uuid.0.as_str())
        .map_err(|_| BluetoothError::GattError("Invalid UUID".into()))
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

    pub fn discover_services(&self) -> Result<Vec<GattService>, BluetoothError> {
        let result = futures::executor::block_on(async {
            self.device
                .GetGattServicesAsync()
                .map_err(|e| BluetoothError::GattError(e.to_string()))?
                .await
                .map_err(|e| BluetoothError::GattError(e.to_string()))
        })?;
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
            let chars_result = futures::executor::block_on(async {
                svc.GetCharacteristicsAsync()
                    .map_err(|e| BluetoothError::GattError(e.to_string()))?
                    .await
                    .map_err(|e| BluetoothError::GattError(e.to_string()))
            })?;
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

    pub fn write_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
        data: &[u8],
    ) -> Result<(), BluetoothError> {
        let svc = futures::executor::block_on(self.find_service(service))?;
        let chr = futures::executor::block_on(self.find_characteristic(&svc, characteristic))?;
        let write_operation = {
            let writer = windows::Storage::Streams::DataWriter::new()
                .map_err(|e| BluetoothError::GattError(e.to_string()))?;
            writer
                .WriteBytes(data)
                .map_err(|e| BluetoothError::GattError(e.to_string()))?;
            let buffer = writer
                .DetachBuffer()
                .map_err(|e| BluetoothError::GattError(e.to_string()))?;
            chr.WriteValueAsync(&buffer)
                .map_err(|e| BluetoothError::GattError(e.to_string()))?
        };
        let result = futures::executor::block_on(async {
            write_operation
                .await
                .map_err(|e| BluetoothError::GattError(e.to_string()))
        })?;
        if result != GattCommunicationStatus::Success {
            return Err(BluetoothError::GattError("Write failed".into()));
        }
        Ok(())
    }

    pub const fn subscribe(
        &self,
        _service: &Uuid,
        _characteristic: &Uuid,
    ) -> Result<async_channel::Receiver<Vec<u8>>, BluetoothError> {
        let _ = self;
        // Notifications require GattCharacteristic.ValueChanged event handler
        // For now return not supported until full implementation
        Err(BluetoothError::NotSupported)
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

    pub const fn start_discovery(
        &self,
    ) -> Result<async_channel::Receiver<ClassicDevice>, BluetoothError> {
        let _ = self;
        Err(BluetoothError::NotSupported)
    }

    pub const fn stop_discovery(&self) {
        let _ = self;
    }

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
