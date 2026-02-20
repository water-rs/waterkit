use crate::{HealthDataType, HealthError, HealthSample};

pub const fn is_available() -> bool {
    false
}

#[allow(clippy::unused_async)]
pub async fn request_authorization(
    _read: &[HealthDataType],
    _write: &[HealthDataType],
) -> Result<(), HealthError> {
    Err(HealthError::PlatformError(
        "Android: use JNI context directly".into(),
    ))
}

#[allow(clippy::unused_async)]
pub async fn query_samples(
    _data_type: HealthDataType,
    _start: &str,
    _end: &str,
) -> Result<Vec<HealthSample>, HealthError> {
    Err(HealthError::PlatformError(
        "Android: use JNI context directly".into(),
    ))
}

#[allow(clippy::unused_async)]
pub async fn write_sample(_sample: HealthSample) -> Result<(), HealthError> {
    Err(HealthError::PlatformError(
        "Android: use JNI context directly".into(),
    ))
}
