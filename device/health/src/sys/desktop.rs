use std::path::{Path, PathBuf};

use crate::{HealthDataType, HealthError, HealthSample};
use waterkit_core::Timestamp;
use waterkit_fs::WaterFs;

const STORE_FILE_NAME: &str = "health.json";

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct HealthStore {
    samples: Vec<HealthSample>,
}

pub const fn is_available() -> bool {
    true
}

pub async fn query_samples(
    data_type: HealthDataType,
    start: Timestamp,
    end: Timestamp,
) -> Result<Vec<HealthSample>, HealthError> {
    blocking::unblock(move || {
        if start > end {
            return Err(HealthError::Platform(
                "start instant must be less than or equal to end".to_string(),
            ));
        }
        let path = store_path()?;
        let store = load_store(&path)?;
        Ok(store
            .samples
            .into_iter()
            .filter(|sample| sample.data_type() == data_type)
            .filter(|sample| sample.end() >= start && sample.start() <= end)
            .collect())
    })
    .await
}

pub async fn write_sample(sample: HealthSample) -> Result<(), HealthError> {
    blocking::unblock(move || {
        let path = store_path()?;
        let mut store = load_store(&path)?;
        store.samples.push(sample);
        write_store(&path, &store)
    })
    .await
}

fn store_path() -> Result<PathBuf, HealthError> {
    WaterFs::data_local_path(Path::new("waterkit").join("health").join(STORE_FILE_NAME))
        .map_err(|error| HealthError::Platform(format!("resolve health store path: {error}")))
}

fn load_store(path: &Path) -> Result<HealthStore, HealthError> {
    WaterFs::load_json_store(path).map_err(|error| {
        HealthError::Platform(format!("load health store {}: {error}", path.display()))
    })
}

fn write_store(path: &Path, store: &HealthStore) -> Result<(), HealthError> {
    WaterFs::write_json_store(path, store).map_err(|error| {
        HealthError::Platform(format!("write health store {}: {error}", path.display()))
    })
}
