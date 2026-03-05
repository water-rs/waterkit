use crate::{
    AdapterState, BluetoothError, ClassicDevice, DeviceId, GattService, ScanFilter, ScanResult,
    Uuid,
};

#[allow(clippy::unused_async)]
pub async fn adapter_state() -> Result<AdapterState, BluetoothError> {
    Err(BluetoothError::Unsupported)
}

#[derive(Debug)]
pub struct BleScannerInner;

impl BleScannerInner {
    #[allow(clippy::unused_async)]
    pub async fn new() -> Result<Self, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    pub fn start_scan(
        &self,
        _filter: &ScanFilter,
    ) -> Result<async_channel::Receiver<ScanResult>, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    pub fn stop_scan(&self) {}
}

#[derive(Debug)]
pub struct BleConnectionInner;

impl BleConnectionInner {
    #[allow(clippy::unused_async)]
    pub async fn connect(_device_id: &DeviceId) -> Result<Self, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn discover_services(&self) -> Result<Vec<GattService>, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn read_characteristic(
        &self,
        _service: &Uuid,
        _characteristic: &Uuid,
    ) -> Result<Vec<u8>, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn write_characteristic(
        &self,
        _service: &Uuid,
        _characteristic: &Uuid,
        _data: &[u8],
    ) -> Result<(), BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn subscribe(
        &self,
        _service: &Uuid,
        _characteristic: &Uuid,
    ) -> Result<async_channel::Receiver<Vec<u8>>, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn disconnect(self) {}
}

#[derive(Debug)]
pub struct ClassicBluetoothInner;

impl ClassicBluetoothInner {
    #[allow(clippy::unused_async)]
    pub async fn new() -> Result<Self, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn start_discovery(
        &self,
    ) -> Result<async_channel::Receiver<ClassicDevice>, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn stop_discovery(&self) {}

    #[allow(clippy::unused_async)]
    pub async fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn connect_spp(
        &self,
        _device_id: &DeviceId,
        _uuid: &Uuid,
    ) -> Result<SppStreamInner, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }
}

#[derive(Debug)]
pub struct SppStreamInner;

impl SppStreamInner {
    #[allow(clippy::unused_async)]
    pub async fn read(&self, _buf: &mut [u8]) -> Result<usize, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn write(&self, _data: &[u8]) -> Result<usize, BluetoothError> {
        Err(BluetoothError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn close(self) {}
}
