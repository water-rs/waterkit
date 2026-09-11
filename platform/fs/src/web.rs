use std::io;
use std::path::{Path, PathBuf};

use indexed_db_futures::database::Database;
use indexed_db_futures::error::{DomException, Error as IdbError, OpenDbError};
use indexed_db_futures::prelude::{Build, BuildPrimitive, QuerySource};
use indexed_db_futures::transaction::{Transaction, TransactionMode};
use indexed_db_futures::typed_array::{Uint8Array, Uint8ArraySlice};

use crate::cache_path_candidate;

const DATABASE_NAME: &str = "waterui-waterfs";
const DATABASE_VERSION: u32 = 1;
const FILE_STORE_NAME: &str = "files";
const VIRTUAL_CACHE_ROOT: &str = "/waterfs/cache";

// Every handle in this module -- `Database`, `Transaction`, `ObjectStore` and
// the request futures -- wraps a `JsValue` bound to the browser thread that
// created it, so nothing here can be `Send`. That is a property of the
// IndexedDB API rather than of this code, hence the per-function expectations
// below.

#[expect(
    clippy::future_not_send,
    reason = "IndexedDB handles are `JsValue`s bound to the browser thread that created them"
)]
pub async fn read(path: &Path) -> io::Result<Vec<u8>> {
    let key = path_key(path)?;
    let database = open_database().await?;
    let transaction = read_transaction(&database)?;
    let request = transaction
        .object_store(FILE_STORE_NAME)
        .map_err(storage_error)?
        .get::<Uint8Array, _, _>(key.as_str())
        .primitive()
        .map_err(storage_error)?;
    let value = request.await.map_err(storage_error)?;
    transaction.commit().await.map_err(storage_error)?;

    let Some(value) = value else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("waterfs path does not exist: {key}"),
        ));
    };

    Ok(value.into())
}

#[expect(
    clippy::future_not_send,
    reason = "IndexedDB handles are `JsValue`s bound to the browser thread that created them"
)]
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

#[expect(
    clippy::future_not_send,
    reason = "IndexedDB handles are `JsValue`s bound to the browser thread that created them"
)]
async fn open_database() -> io::Result<Database> {
    Database::open(DATABASE_NAME)
        .with_version(DATABASE_VERSION)
        .with_on_upgrade_needed(|_event, database| {
            if !database
                .object_store_names()
                .any(|store_name| store_name == FILE_STORE_NAME)
            {
                database.create_object_store(FILE_STORE_NAME).build()?;
            }
            Ok(())
        })
        .await
        .map_err(open_error)
}

fn read_transaction(database: &Database) -> io::Result<Transaction<'_>> {
    database
        .transaction(FILE_STORE_NAME)
        .build()
        .map_err(storage_error)
}

#[expect(
    clippy::future_not_send,
    reason = "IndexedDB handles are `JsValue`s bound to the browser thread that created them"
)]
async fn contains_path(database: &Database, key: &str) -> io::Result<bool> {
    let transaction = read_transaction(database)?;
    let request = transaction
        .object_store(FILE_STORE_NAME)
        .map_err(storage_error)?
        .count()
        .with_query(key)
        .primitive()
        .map_err(storage_error)?;
    let matches = request.await.map_err(storage_error)?;
    transaction.commit().await.map_err(storage_error)?;
    Ok(matches > 0)
}

#[expect(
    clippy::future_not_send,
    reason = "IndexedDB handles are `JsValue`s bound to the browser thread that created them"
)]
async fn store_bytes(database: &Database, key: &str, bytes: &[u8]) -> io::Result<()> {
    let transaction = database
        .transaction(FILE_STORE_NAME)
        .with_mode(TransactionMode::Readwrite)
        .build()
        .map_err(storage_error)?;
    let request = transaction
        .object_store(FILE_STORE_NAME)
        .map_err(storage_error)?
        .put(Uint8ArraySlice::new(bytes))
        .with_key(key)
        .without_key_type()
        .primitive()
        .map_err(storage_error)?;
    request.await.map_err(storage_error)?;
    transaction.commit().await.map_err(storage_error)?;
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

fn open_error(error: OpenDbError) -> io::Error {
    match error {
        OpenDbError::VersionZero => io::Error::new(
            io::ErrorKind::InvalidInput,
            "waterfs web storage error: database version cannot be zero",
        ),
        OpenDbError::UnsupportedEnvironment => io::Error::new(
            io::ErrorKind::Unsupported,
            "waterfs web storage error: IndexedDB is unavailable in this environment",
        ),
        OpenDbError::NullFactory => io::Error::new(
            io::ErrorKind::Unsupported,
            "waterfs web storage error: the `indexedDB` getter returned null",
        ),
        OpenDbError::Base(error) => storage_error(error),
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a `map_err` argument, which hands the error over by value"
)]
fn storage_error(error: IdbError) -> io::Error {
    let kind = match &error {
        IdbError::DomException(exception) => dom_exception_kind(exception),
        IdbError::Serialisation(_) | IdbError::MissingData(_) => io::ErrorKind::InvalidData,
        IdbError::Unknown(_) => io::ErrorKind::Other,
    };
    io::Error::new(kind, format!("waterfs web storage error: {error}"))
}

/// Maps the `DomException` variants `IndexedDB` raises onto the
/// [`io::ErrorKind`] the native backends produce for the same situation, so
/// callers see one set of kinds across every platform. Conditions with no
/// filesystem analogue -- a request against a finished transaction, an
/// unrecognised exception -- stay [`io::ErrorKind::Other`] and carry their
/// detail in the message.
const fn dom_exception_kind(exception: &DomException) -> io::ErrorKind {
    match exception {
        DomException::NotFoundError(_) => io::ErrorKind::NotFound,
        DomException::ConstraintError(_) => io::ErrorKind::AlreadyExists,
        DomException::ReadOnlyError(_) | DomException::InvalidAccessError(_) => {
            io::ErrorKind::PermissionDenied
        }
        DomException::DataError(_)
        | DomException::DataCloneError(_)
        | DomException::SyntaxError(_) => io::ErrorKind::InvalidData,
        DomException::AbortError(_) => io::ErrorKind::Interrupted,
        DomException::InvalidStateError(_)
        | DomException::TransactionInactiveError(_)
        | DomException::Other(_) => io::ErrorKind::Other,
    }
}
