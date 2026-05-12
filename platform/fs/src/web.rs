use std::io;
use std::path::{Path, PathBuf};

use indexed_db_futures::prelude::*;
use indexed_db_futures::web_sys::DomException;
use js_sys::Uint8Array;
use wasm_bindgen::JsValue;

use crate::cache_path_candidate;

const DATABASE_NAME: &str = "waterui-waterfs";
const DATABASE_VERSION: u32 = 1;
const FILE_STORE_NAME: &str = "files";
const VIRTUAL_CACHE_ROOT: &str = "/waterfs/cache";

pub async fn read(path: &Path) -> io::Result<Vec<u8>> {
    let key = path_key(path)?;
    let database = open_database().await?;
    let transaction = database
        .transaction_on_one(FILE_STORE_NAME)
        .map_err(storage_error)?;
    let store = transaction
        .object_store(FILE_STORE_NAME)
        .map_err(storage_error)?;
    let value = store
        .get_owned(key.clone())
        .map_err(storage_error)?
        .await
        .map_err(storage_error)?;
    transaction.await.into_result().map_err(storage_error)?;

    let Some(value) = value else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("waterfs path does not exist: {key}"),
        ));
    };

    let bytes = Uint8Array::new(&value);
    let mut output = vec![0; bytes.length() as usize];
    bytes.copy_to(&mut output);
    Ok(output)
}

pub async fn import_bytes_to_cache(
    bytes: &[u8],
    file_name: &str,
    cache_subdir: &Path,
) -> io::Result<PathBuf> {
    let file_name = Path::new(file_name).file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "selected path has no file name",
        )
    })?;
    let base_dir = Path::new(VIRTUAL_CACHE_ROOT).join(cache_subdir);
    let database = open_database().await?;
    let mut index = 0usize;

    loop {
        let candidate = cache_path_candidate(&base_dir, file_name, index);
        let candidate_key = path_key(&candidate)?;
        if !contains_path(&database, &candidate_key).await? {
            store_bytes(&database, &candidate_key, bytes).await?;
            return Ok(candidate);
        }
        index += 1;
    }
}

async fn open_database() -> io::Result<IdbDatabase> {
    let mut request =
        IdbDatabase::open_u32(DATABASE_NAME, DATABASE_VERSION).map_err(storage_error)?;
    request.set_on_upgrade_needed(Some(
        |event: &IdbVersionChangeEvent| -> Result<(), JsValue> {
            if event
                .db()
                .object_store_names()
                .find(|store_name| store_name == FILE_STORE_NAME)
                .is_none()
            {
                event.db().create_object_store(FILE_STORE_NAME)?;
            }
            Ok(())
        },
    ));
    request.await.map_err(storage_error)
}

async fn contains_path(database: &IdbDatabase, key: &str) -> io::Result<bool> {
    let transaction = database
        .transaction_on_one(FILE_STORE_NAME)
        .map_err(storage_error)?;
    let store = transaction
        .object_store(FILE_STORE_NAME)
        .map_err(storage_error)?;
    let value = store
        .get_owned(key)
        .map_err(storage_error)?
        .await
        .map_err(storage_error)?;
    transaction.await.into_result().map_err(storage_error)?;
    Ok(value.is_some())
}

async fn store_bytes(database: &IdbDatabase, key: &str, bytes: &[u8]) -> io::Result<()> {
    let transaction = database
        .transaction_on_one_with_mode(FILE_STORE_NAME, IdbTransactionMode::Readwrite)
        .map_err(storage_error)?;
    let store = transaction
        .object_store(FILE_STORE_NAME)
        .map_err(storage_error)?;
    let value = Uint8Array::from(bytes);
    store
        .put_key_val_owned(key, &value)
        .map_err(storage_error)?
        .await
        .map_err(storage_error)?;
    transaction.await.into_result().map_err(storage_error)?;
    Ok(())
}

fn path_key(path: &Path) -> io::Result<String> {
    let key = path.to_string_lossy().to_string();
    if key.starts_with(VIRTUAL_CACHE_ROOT) {
        Ok(key)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is outside waterfs web cache: {key}"),
        ))
    }
}

fn storage_error(error: DomException) -> io::Error {
    io::Error::other(format!(
        "waterfs web storage error: {} ({})",
        error.name(),
        error.message()
    ))
}
