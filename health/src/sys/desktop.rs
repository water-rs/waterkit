use std::path::{Path, PathBuf};

use crate::{HealthDataType, HealthError, HealthSample};
use std::collections::HashSet;
use waterkit_fs::WaterFs;

const STORE_FILE_NAME: &str = "health.json";

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct HealthStore {
    authorized_read: HashSet<HealthDataType>,
    authorized_write: HashSet<HealthDataType>,
    samples: Vec<HealthSample>,
}

pub const fn is_available() -> bool {
    true
}

pub async fn request_authorization(
    read: &[HealthDataType],
    write: &[HealthDataType],
) -> Result<(), HealthError> {
    let read = read.to_vec();
    let write = write.to_vec();
    blocking::unblock(move || {
        let path = store_path()?;
        let mut store = load_store(&path)?;
        store.authorized_read.extend(read);
        store.authorized_write.extend(write);
        write_store(&path, &store)
    })
    .await
}

pub async fn query_samples(
    data_type: HealthDataType,
    start: &str,
    end: &str,
) -> Result<Vec<HealthSample>, HealthError> {
    let start = start.to_string();
    let end = end.to_string();
    blocking::unblock(move || {
        if start > end {
            return Err(HealthError::PlatformError(
                "start date must be less than or equal to end date".to_string(),
            ));
        }
        let path = store_path()?;
        let store = load_store(&path)?;
        if !store.authorized_read.contains(&data_type) {
            return Err(HealthError::PermissionDenied);
        }
        Ok(store
            .samples
            .into_iter()
            .filter(|sample| sample.data_type() == data_type)
            .filter(|sample| sample_overlaps(sample, &start, &end))
            .collect())
    })
    .await
}

pub async fn write_sample(sample: HealthSample) -> Result<(), HealthError> {
    let data_type = sample.data_type();
    blocking::unblock(move || {
        let path = store_path()?;
        let mut store = load_store(&path)?;
        if !store.authorized_write.contains(&data_type) {
            return Err(HealthError::PermissionDenied);
        }
        store.samples.push(sample);
        write_store(&path, &store)
    })
    .await
}

fn sample_overlaps(sample: &HealthSample, start: &str, end: &str) -> bool {
    sample.end_date() >= start && sample.start_date() <= end
}

fn store_path() -> Result<PathBuf, HealthError> {
    WaterFs::data_local_path(Path::new("waterkit").join("health").join(STORE_FILE_NAME))
        .map_err(|error| HealthError::PlatformError(format!("resolve health store path: {error}")))
}

fn load_store(path: &Path) -> Result<HealthStore, HealthError> {
    WaterFs::load_json_store(path).map_err(|error| {
        HealthError::PlatformError(format!("load health store {}: {error}", path.display()))
    })
}

fn write_store(path: &Path, store: &HealthStore) -> Result<(), HealthError> {
    WaterFs::write_json_store(path, store).map_err(|error| {
        HealthError::PlatformError(format!("write health store {}: {error}", path.display()))
    })
}
