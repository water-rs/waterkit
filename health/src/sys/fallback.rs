use crate::{HealthDataType, HealthError, HealthSample};
use waterkit_core::Timestamp;

pub fn is_available() -> bool {
    false
}

#[allow(clippy::unused_async)]
pub async fn query_samples(
    _data_type: HealthDataType,
    _start: Timestamp,
    _end: Timestamp,
) -> Result<Vec<HealthSample>, HealthError> {
    Err(HealthError::Unsupported)
}

#[allow(clippy::unused_async)]
pub async fn write_sample(_sample: HealthSample) -> Result<(), HealthError> {
    Err(HealthError::Unsupported)
}
