use crate::{HealthDataType, HealthError, HealthSample};

pub fn is_available() -> bool {
    false
}

#[allow(clippy::unused_async)]
pub async fn request_authorization(
    _read: &[HealthDataType],
    _write: &[HealthDataType],
) -> Result<(), HealthError> {
    Err(HealthError::NotSupported)
}

#[allow(clippy::unused_async)]
pub async fn query_samples(
    _data_type: HealthDataType,
    _start: &str,
    _end: &str,
) -> Result<Vec<HealthSample>, HealthError> {
    Err(HealthError::NotSupported)
}

#[allow(clippy::unused_async)]
pub async fn write_sample(_sample: HealthSample) -> Result<(), HealthError> {
    Err(HealthError::NotSupported)
}
