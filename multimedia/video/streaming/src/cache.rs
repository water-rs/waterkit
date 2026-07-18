use std::path::{Path, PathBuf};

use url::Url;

#[cfg(feature = "offline")]
use {
    serde::{Deserialize, Serialize},
    std::{collections::BTreeSet, num::NonZeroU64},
    uuid::Uuid,
    waterkit_video_core::Error,
};

#[cfg(feature = "offline")]
use crate::{MediaByteRange, MediaValidator, atomic};

/// Application-owned location and stable keying policy for cached media assets.
#[derive(Debug, Clone)]
pub struct AssetCache {
    root: PathBuf,
}

impl AssetCache {
    /// Creates a cache rooted at an application-selected directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the configured cache root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Produces a stable local path for a remote URL.
    ///
    /// # Panics
    ///
    /// Panics when `default_extension` is empty or contains non-ASCII-alphanumeric text.
    #[must_use]
    pub fn path_for(&self, url: &Url, default_extension: &str) -> PathBuf {
        assert!(
            is_safe_extension(default_extension),
            "default media cache extension must be non-empty ASCII alphanumeric text"
        );
        let digest = blake3::hash(url.as_str().as_bytes()).to_hex();
        let extension = url
            .path_segments()
            .and_then(Iterator::last)
            .and_then(|name| name.rsplit_once('.').map(|(_, extension)| extension))
            .filter(|extension| is_safe_extension(extension))
            .unwrap_or(default_extension);
        self.root.join(format!("{digest}.{extension}"))
    }
}

fn is_safe_extension(extension: &str) -> bool {
    !extension.is_empty()
        && extension
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
}

/// Stable cache identity derived only from URL and origin validators.
///
/// Request headers are intentionally absent so credentials never enter cache
/// keys or the persisted cache index.
#[cfg(feature = "offline")]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheResourceKey(String);

#[cfg(feature = "offline")]
impl CacheResourceKey {
    /// Creates a stable cache identity for one validated resource revision.
    #[must_use]
    pub fn new(url: &Url, validator: &MediaValidator) -> Self {
        let mut hasher = blake3::Hasher::new();
        hash_field(&mut hasher, url.as_str().as_bytes());
        hash_optional_field(&mut hasher, validator.etag());
        hash_optional_field(&mut hasher, validator.last_modified());
        Self(hasher.finalize().to_hex().to_string())
    }

    /// Returns the stable hexadecimal identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(feature = "offline")]
fn hash_field(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(value.len().to_string().as_bytes());
    hasher.update(&[0]);
    hasher.update(value);
}

#[cfg(feature = "offline")]
fn hash_optional_field(hasher: &mut blake3::Hasher, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update(&[1]);
            hash_field(hasher, value.as_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

/// Sparse byte coverage currently available for one resource revision.
#[cfg(feature = "offline")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheCoverage {
    complete: bool,
    ranges: Vec<MediaByteRange>,
}

#[cfg(feature = "offline")]
impl CacheCoverage {
    /// Returns whether a complete-resource object is cached.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }

    /// Returns merged, sorted half-open byte ranges.
    #[must_use]
    pub fn ranges(&self) -> &[MediaByteRange] {
        &self.ranges
    }
}

/// Metadata returned after a cache object is atomically committed.
#[cfg(feature = "offline")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedObject {
    resource: CacheResourceKey,
    range: Option<MediaByteRange>,
    path: PathBuf,
    bytes: u64,
    checksum: String,
    pinned: bool,
}

#[cfg(feature = "offline")]
impl CachedObject {
    /// Returns the owning resource revision.
    #[must_use]
    pub const fn resource(&self) -> &CacheResourceKey {
        &self.resource
    }

    /// Returns the represented byte range, or `None` for a complete object.
    #[must_use]
    pub const fn byte_range(&self) -> Option<MediaByteRange> {
        self.range
    }

    /// Returns the durable content-addressed object path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the committed object length.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Returns the BLAKE3 content checksum.
    #[must_use]
    pub fn checksum(&self) -> &str {
        &self.checksum
    }

    /// Returns whether quota eviction is disabled for this object.
    #[must_use]
    pub const fn is_pinned(&self) -> bool {
        self.pinned
    }
}

/// Single-owner, crash-safe persistent range and segment cache.
///
/// Mutation requires `&mut self`, making concurrent index writers impossible
/// without locks or hidden global state. Applications share one owner through
/// their existing media pipeline actor.
#[cfg(feature = "offline")]
#[derive(Debug)]
pub struct PersistentMediaCache {
    root: PathBuf,
    quota: NonZeroU64,
    index: CacheIndex,
}

#[cfg(feature = "offline")]
impl PersistentMediaCache {
    /// Opens or creates the cache and verifies every indexed object.
    ///
    /// # Errors
    ///
    /// Returns an error for incompatible/corrupt index state, missing objects,
    /// checksum failures, or I/O errors.
    pub async fn open(cache: &AssetCache, quota: NonZeroU64) -> Result<Self, Error> {
        let root = cache.root().join("media-cache");
        async_fs::create_dir_all(root.join("objects")).await?;
        let index_path = root.join("index.json");
        let index = match async_fs::read(&index_path).await {
            Ok(bytes) => serde_json::from_slice::<CacheIndex>(&bytes).map_err(|error| {
                Error::Streaming(format!("failed to parse media cache index: {error}"))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => CacheIndex::new(),
            Err(error) => return Err(error.into()),
        };
        index.validate(&root).await?;
        remove_cache_partials(&root).await?;
        remove_orphan_objects(&root, &index).await?;
        let mut cache = Self { root, quota, index };
        let required_eviction = cache.used_bytes()?.saturating_sub(quota.get());
        let evictions = cache.index.evictions(None, required_eviction)?;
        if !evictions.is_empty() {
            let removed = evictions
                .iter()
                .filter_map(|lookup| cache.index.record(lookup).cloned())
                .collect::<Vec<_>>();
            let mut next = cache.index.clone();
            for eviction in &evictions {
                next.remove(eviction);
            }
            store_cache_index(&cache.root, &next).await?;
            cache.index = next;
            cache.remove_records_if_unreferenced(&removed).await?;
        }
        Ok(cache)
    }

    /// Returns the configured cache quota.
    #[must_use]
    pub const fn quota(&self) -> NonZeroU64 {
        self.quota
    }

    /// Returns total bytes referenced by the durable index.
    ///
    /// # Errors
    ///
    /// Returns an error if indexed byte counts overflow `u64`.
    pub fn used_bytes(&self) -> Result<u64, Error> {
        self.index.used_bytes()
    }

    /// Atomically commits one complete resource or exact byte range.
    ///
    /// Least-recently-used unpinned objects are removed from the next atomic
    /// index revision when required by quota. A write fails before changing the
    /// index when pinned objects leave insufficient capacity.
    ///
    /// # Errors
    ///
    /// Returns an error for range-length mismatch, quota exhaustion, overflow,
    /// serialization, or I/O failure.
    pub async fn store(
        &mut self,
        resource: CacheResourceKey,
        range: Option<MediaByteRange>,
        bytes: &[u8],
        pinned: bool,
    ) -> Result<CachedObject, Error> {
        let bytes_len = u64::try_from(bytes.len())
            .map_err(|_| Error::Streaming(String::from("cached media length does not fit u64")))?;
        if let Some(range) = range
            && range.len() != bytes_len
        {
            return Err(Error::Streaming(format!(
                "cached media range contains {bytes_len} bytes, expected {}",
                range.len(),
            )));
        }
        if bytes_len > self.quota.get() {
            return Err(Error::Streaming(format!(
                "cached media object has {bytes_len} bytes, exceeding the {}-byte quota",
                self.quota,
            )));
        }

        let lookup = CacheLookup::new(&resource, range);
        let checksum = blake3::hash(bytes).to_hex().to_string();
        let file_name = format!("{}-{checksum}.blob", lookup.0);
        let object_path = self.root.join("objects").join(&file_name);
        let replaced = self.index.record(&lookup).cloned();
        let current_without_replaced = self
            .index
            .used_bytes()?
            .saturating_sub(replaced.as_ref().map_or(0, |record| record.bytes));
        let required_total = current_without_replaced
            .checked_add(bytes_len)
            .ok_or_else(|| Error::Streaming(String::from("media cache quota math overflowed")))?;
        let required_eviction = required_total.saturating_sub(self.quota.get());
        let evictions = self.index.evictions(Some(&lookup), required_eviction)?;
        let evicted_records = evictions
            .iter()
            .filter_map(|lookup| self.index.record(lookup).cloned())
            .collect::<Vec<_>>();

        match async_fs::metadata(&object_path).await {
            Ok(_) => verify_cache_object(&object_path, bytes_len, &checksum).await?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write_cache_object(&object_path, bytes).await?;
            }
            Err(error) => return Err(error.into()),
        }

        let mut next = self.index.clone();
        next.remove(&lookup);
        for eviction in &evictions {
            next.remove(eviction);
        }
        let access = next.next_access()?;
        next.records.push(CacheRecord {
            lookup: lookup.clone(),
            resource: resource.0.clone(),
            range: range.map(PersistedRange::from),
            file_name: file_name.clone(),
            bytes: bytes_len,
            checksum: checksum.clone(),
            pinned,
            access,
        });
        next.sort_records();
        store_cache_index(&self.root, &next).await?;
        self.index = next;
        let mut removed_records = replaced.into_iter().collect::<Vec<_>>();
        removed_records.extend(evicted_records);
        self.remove_records_if_unreferenced(&removed_records)
            .await?;

        Ok(CachedObject {
            resource,
            range,
            path: object_path,
            bytes: bytes_len,
            checksum,
            pinned,
        })
    }

    /// Reads and checksum-verifies one exact cached object, updating LRU order.
    ///
    /// # Errors
    ///
    /// Returns an error for a cache miss or corrupt/missing object.
    pub async fn read(
        &mut self,
        resource: &CacheResourceKey,
        range: Option<MediaByteRange>,
    ) -> Result<Vec<u8>, Error> {
        let lookup = CacheLookup::new(resource, range);
        let record = self.index.record(&lookup).cloned().ok_or_else(|| {
            Error::Streaming(format!("media cache miss for {}", resource.as_str()))
        })?;
        let path = self.root.join("objects").join(&record.file_name);
        let bytes = async_fs::read(&path).await.map_err(|error| {
            Error::Streaming(format!(
                "failed to read cached media {}: {error}",
                path.display()
            ))
        })?;
        verify_cache_bytes(&path, &bytes, record.bytes, &record.checksum)?;
        let mut next = self.index.clone();
        let access = next.next_access()?;
        next.record_mut(&lookup)
            .ok_or_else(|| {
                Error::Streaming(String::from(
                    "media cache record disappeared during an exclusive read",
                ))
            })?
            .access = access;
        store_cache_index(&self.root, &next).await?;
        self.index = next;
        Ok(bytes)
    }

    /// Pins or unpins an exact cached object and persists the policy atomically.
    ///
    /// # Errors
    ///
    /// Returns an error for a cache miss or index persistence failure.
    pub async fn set_pinned(
        &mut self,
        resource: &CacheResourceKey,
        range: Option<MediaByteRange>,
        pinned: bool,
    ) -> Result<(), Error> {
        let lookup = CacheLookup::new(resource, range);
        let mut next = self.index.clone();
        next.record_mut(&lookup)
            .ok_or_else(|| Error::Streaming(format!("media cache miss for {}", resource.as_str())))?
            .pinned = pinned;
        store_cache_index(&self.root, &next).await?;
        self.index = next;
        Ok(())
    }

    /// Returns complete-resource and merged sparse-range coverage.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted range endpoints are invalid.
    pub fn coverage(&self, resource: &CacheResourceKey) -> Result<CacheCoverage, Error> {
        let mut complete = false;
        let mut ranges = self
            .index
            .records
            .iter()
            .filter(|record| record.resource == resource.0)
            .filter_map(|record| {
                record.range.map_or_else(
                    || {
                        complete = true;
                        None
                    },
                    |range| Some(range.into_media_range()),
                )
            })
            .collect::<Result<Vec<_>, Error>>()?;
        ranges.sort_by_key(|range| (range.start(), range.end_exclusive()));
        let mut merged: Vec<MediaByteRange> = Vec::new();
        for range in ranges {
            if let Some(previous) = merged.last_mut()
                && range.start() <= previous.end_exclusive()
            {
                *previous = MediaByteRange::new(
                    previous.start(),
                    previous.end_exclusive().max(range.end_exclusive()),
                )?;
            } else {
                merged.push(range);
            }
        }
        Ok(CacheCoverage {
            complete,
            ranges: merged,
        })
    }

    /// Removes every cached range and complete object for one resource revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the updated index or object deletion fails.
    pub async fn remove_resource(&mut self, resource: &CacheResourceKey) -> Result<(), Error> {
        let removed = self
            .index
            .records
            .iter()
            .filter(|record| record.resource == resource.0)
            .cloned()
            .collect::<Vec<_>>();
        if removed.is_empty() {
            return Ok(());
        }
        let mut next = self.index.clone();
        next.records.retain(|record| record.resource != resource.0);
        store_cache_index(&self.root, &next).await?;
        self.index = next;
        self.remove_records_if_unreferenced(&removed).await
    }

    async fn remove_records_if_unreferenced(&self, records: &[CacheRecord]) -> Result<(), Error> {
        let active_files = self
            .index
            .records
            .iter()
            .map(|record| record.file_name.as_str())
            .collect::<BTreeSet<_>>();
        for record in records {
            if active_files.contains(record.file_name.as_str()) {
                continue;
            }
            let path = self.root.join("objects").join(&record.file_name);
            match async_fs::remove_file(path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

#[cfg(feature = "offline")]
const CACHE_INDEX_VERSION: u16 = 1;

#[cfg(feature = "offline")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheIndex {
    version: u16,
    access_sequence: u64,
    records: Vec<CacheRecord>,
}

#[cfg(feature = "offline")]
impl CacheIndex {
    const fn new() -> Self {
        Self {
            version: CACHE_INDEX_VERSION,
            access_sequence: 0,
            records: Vec::new(),
        }
    }

    fn record(&self, lookup: &CacheLookup) -> Option<&CacheRecord> {
        self.records.iter().find(|record| &record.lookup == lookup)
    }

    fn record_mut(&mut self, lookup: &CacheLookup) -> Option<&mut CacheRecord> {
        self.records
            .iter_mut()
            .find(|record| &record.lookup == lookup)
    }

    fn remove(&mut self, lookup: &CacheLookup) {
        self.records.retain(|record| &record.lookup != lookup);
    }

    fn used_bytes(&self) -> Result<u64, Error> {
        self.records.iter().try_fold(0_u64, |total, record| {
            total
                .checked_add(record.bytes)
                .ok_or_else(|| Error::Streaming(String::from("media cache size overflowed u64")))
        })
    }

    fn next_access(&mut self) -> Result<u64, Error> {
        self.access_sequence = self.access_sequence.checked_add(1).ok_or_else(|| {
            Error::Streaming(String::from("media cache LRU sequence overflowed u64"))
        })?;
        Ok(self.access_sequence)
    }

    fn evictions(
        &self,
        replacing: Option<&CacheLookup>,
        required_bytes: u64,
    ) -> Result<Vec<CacheLookup>, Error> {
        if required_bytes == 0 {
            return Ok(Vec::new());
        }
        let mut candidates = self
            .records
            .iter()
            .filter(|record| replacing != Some(&record.lookup) && !record.pinned)
            .collect::<Vec<_>>();
        candidates.sort_by_key(|record| (record.access, record.lookup.0.clone()));
        let mut reclaimed = 0_u64;
        let mut selected = Vec::new();
        for candidate in candidates {
            reclaimed = reclaimed.checked_add(candidate.bytes).ok_or_else(|| {
                Error::Streaming(String::from("media cache eviction size overflowed u64"))
            })?;
            selected.push(candidate.lookup.clone());
            if reclaimed >= required_bytes {
                return Ok(selected);
            }
        }
        Err(Error::Streaming(format!(
            "media cache needs {required_bytes} bytes but pinned objects prevent quota eviction",
        )))
    }

    fn sort_records(&mut self) {
        self.records
            .sort_by(|left, right| left.lookup.cmp(&right.lookup));
    }

    async fn validate(&self, root: &Path) -> Result<(), Error> {
        if self.version != CACHE_INDEX_VERSION {
            return Err(Error::Streaming(format!(
                "media cache index version {} is unsupported; expected {CACHE_INDEX_VERSION}",
                self.version,
            )));
        }
        let mut lookups = BTreeSet::new();
        for record in &self.records {
            if !lookups.insert(&record.lookup) {
                return Err(Error::Streaming(String::from(
                    "media cache index contains a duplicate lookup key",
                )));
            }
            record.validate(root).await?;
        }
        if self
            .records
            .iter()
            .any(|record| record.access > self.access_sequence)
        {
            return Err(Error::Streaming(String::from(
                "media cache record access exceeds the persisted LRU sequence",
            )));
        }
        self.used_bytes()?;
        Ok(())
    }
}

#[cfg(feature = "offline")]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct CacheLookup(String);

#[cfg(feature = "offline")]
impl CacheLookup {
    fn new(resource: &CacheResourceKey, range: Option<MediaByteRange>) -> Self {
        let mut hasher = blake3::Hasher::new();
        hash_field(&mut hasher, resource.as_str().as_bytes());
        match range {
            Some(range) => {
                hasher.update(&[1]);
                hasher.update(&range.start().to_le_bytes());
                hasher.update(&range.end_exclusive().to_le_bytes());
            }
            None => {
                hasher.update(&[0]);
            }
        }
        Self(hasher.finalize().to_hex().to_string())
    }
}

#[cfg(feature = "offline")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheRecord {
    lookup: CacheLookup,
    resource: String,
    range: Option<PersistedRange>,
    file_name: String,
    bytes: u64,
    checksum: String,
    pinned: bool,
    access: u64,
}

#[cfg(feature = "offline")]
impl CacheRecord {
    async fn validate(&self, root: &Path) -> Result<(), Error> {
        if self.resource.len() != blake3::OUT_LEN * 2
            || self.checksum.len() != blake3::OUT_LEN * 2
            || self.file_name != format!("{}-{}.blob", self.lookup.0, self.checksum)
        {
            return Err(Error::Streaming(String::from(
                "media cache index contains an invalid content identity",
            )));
        }
        if let Some(range) = self.range {
            let range = range.into_media_range()?;
            if range.len() != self.bytes {
                return Err(Error::Streaming(String::from(
                    "media cache index range length differs from object length",
                )));
            }
        }
        let path = root.join("objects").join(&self.file_name);
        verify_cache_object(&path, self.bytes, &self.checksum).await
    }
}

#[cfg(feature = "offline")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct PersistedRange {
    start: u64,
    end_exclusive: u64,
}

#[cfg(feature = "offline")]
impl PersistedRange {
    fn into_media_range(self) -> Result<MediaByteRange, Error> {
        MediaByteRange::new(self.start, self.end_exclusive)
    }
}

#[cfg(feature = "offline")]
impl From<MediaByteRange> for PersistedRange {
    fn from(range: MediaByteRange) -> Self {
        Self {
            start: range.start(),
            end_exclusive: range.end_exclusive(),
        }
    }
}

#[cfg(feature = "offline")]
async fn store_cache_index(root: &Path, index: &CacheIndex) -> Result<(), Error> {
    let bytes = serde_json::to_vec(index)
        .map_err(|error| Error::Streaming(format!("failed to serialize media cache: {error}")))?;
    let destination = root.join("index.json");
    write_cache_object(&destination, &bytes).await
}

#[cfg(feature = "offline")]
async fn write_cache_object(destination: &Path, bytes: &[u8]) -> Result<(), Error> {
    use async_fs::File;
    use futures::AsyncWriteExt as _;

    let parent = destination
        .parent()
        .ok_or_else(|| Error::Streaming(String::from("media cache destination has no parent")))?;
    async_fs::create_dir_all(parent).await?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Streaming(String::from("media cache file name is not UTF-8")))?;
    let partial = parent.join(format!(".{file_name}.{}.part", Uuid::new_v4()));
    let mut file = File::create(&partial).await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);
    if let Err(error) = atomic::replace(partial.clone(), destination.to_path_buf()).await {
        let _ = async_fs::remove_file(partial).await;
        return Err(error);
    }
    Ok(())
}

#[cfg(feature = "offline")]
async fn verify_cache_object(path: &Path, bytes: u64, checksum: &str) -> Result<(), Error> {
    let contents = async_fs::read(path).await.map_err(|error| {
        Error::Streaming(format!(
            "failed to read cached media object {}: {error}",
            path.display(),
        ))
    })?;
    verify_cache_bytes(path, &contents, bytes, checksum)
}

#[cfg(feature = "offline")]
fn verify_cache_bytes(
    path: &Path,
    contents: &[u8],
    expected_bytes: u64,
    expected_checksum: &str,
) -> Result<(), Error> {
    let actual_bytes = u64::try_from(contents.len())
        .map_err(|_| Error::Streaming(String::from("cached media length does not fit u64")))?;
    if actual_bytes != expected_bytes {
        return Err(Error::Streaming(format!(
            "cached media object {} has {actual_bytes} bytes, expected {expected_bytes}",
            path.display(),
        )));
    }
    let checksum = blake3::hash(contents).to_hex().to_string();
    if checksum != expected_checksum {
        return Err(Error::Streaming(format!(
            "cached media checksum mismatch for {}",
            path.display(),
        )));
    }
    Ok(())
}

#[cfg(feature = "offline")]
async fn remove_cache_partials(root: &Path) -> Result<(), Error> {
    use futures::StreamExt as _;

    for directory in [root.to_path_buf(), root.join("objects")] {
        let mut entries = async_fs::read_dir(directory).await?;
        while let Some(entry) = entries.next().await {
            let entry = entry?;
            if atomic::is_partial_file_name(&entry.file_name()) {
                async_fs::remove_file(entry.path()).await?;
            }
        }
    }
    Ok(())
}

#[cfg(feature = "offline")]
async fn remove_orphan_objects(root: &Path, index: &CacheIndex) -> Result<(), Error> {
    use futures::StreamExt as _;

    let referenced = index
        .records
        .iter()
        .map(|record| record.file_name.as_str())
        .collect::<BTreeSet<_>>();
    let mut entries = async_fs::read_dir(root.join("objects")).await?;
    while let Some(entry) = entries.next().await {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name = file_name.to_str().ok_or_else(|| {
            Error::Streaming(String::from("media cache object file name is not UTF-8"))
        })?;
        if !referenced.contains(file_name) {
            async_fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use url::Url;

    use super::AssetCache;

    #[test]
    fn cache_keys_are_stable_and_preserve_safe_extensions() {
        let cache = AssetCache::new("media-cache");
        let url = Url::parse("https://waterui.dev/assets/trailer.mp4?revision=2")
            .expect("test URL must be valid");
        let first = cache.path_for(&url, "bin");
        let second = cache.path_for(&url, "bin");

        assert_eq!(first, second);
        assert_eq!(first.parent(), Some(Path::new("media-cache")));
        assert_eq!(
            first.extension().and_then(|value| value.to_str()),
            Some("mp4")
        );
    }

    #[cfg(feature = "offline")]
    #[test]
    fn persistent_cache_merges_sparse_ranges_and_evicts_lru_objects() {
        use std::num::NonZeroU64;

        use futures::executor::block_on;
        use uuid::Uuid;

        use super::{CacheResourceKey, PersistentMediaCache};
        use crate::{MediaByteRange, MediaValidator};

        let root = std::env::temp_dir().join(format!("waterkit-cache-{}", Uuid::new_v4()));
        let cache = AssetCache::new(&root);
        let mut store = block_on(PersistentMediaCache::open(
            &cache,
            NonZeroU64::new(12).expect("test quota must be non-zero"),
        ))
        .expect("test cache must open");
        let first = CacheResourceKey::new(
            &Url::parse("https://waterui.dev/video/first.m4s").expect("test URL must be valid"),
            &MediaValidator::none(),
        );
        let second = CacheResourceKey::new(
            &Url::parse("https://waterui.dev/video/second.m4s").expect("test URL must be valid"),
            &MediaValidator::none(),
        );
        block_on(store.store(
            first.clone(),
            Some(MediaByteRange::new(0, 4).expect("test range must be valid")),
            b"aaaa",
            false,
        ))
        .expect("first range must store");
        block_on(store.store(
            first.clone(),
            Some(MediaByteRange::new(4, 8).expect("test range must be valid")),
            b"bbbb",
            false,
        ))
        .expect("second range must store");
        let coverage = store.coverage(&first).expect("coverage must be valid");
        assert_eq!(coverage.ranges().len(), 1);
        assert_eq!(coverage.ranges()[0].start(), 0);
        assert_eq!(coverage.ranges()[0].end_exclusive(), 8);

        block_on(store.store(second.clone(), None, b"cccccccc", false))
            .expect("new object must evict the least-recently-used range");
        let coverage = store.coverage(&first).expect("coverage must be valid");
        assert_eq!(coverage.ranges().len(), 1);
        assert_eq!(coverage.ranges()[0].start(), 4);
        assert_eq!(coverage.ranges()[0].end_exclusive(), 8);
        assert_eq!(
            block_on(store.read(&second, None)).expect("second object must read"),
            b"cccccccc",
        );
        drop(store);
        let reopened = block_on(PersistentMediaCache::open(
            &cache,
            NonZeroU64::new(8).expect("test quota must be non-zero"),
        ))
        .expect("reopening with a smaller quota must evict unpinned LRU objects");
        assert!(
            reopened
                .coverage(&first)
                .expect("coverage must be valid")
                .ranges()
                .is_empty()
        );
        assert_eq!(reopened.used_bytes().expect("cache size must be valid"), 8);
        std::fs::remove_dir_all(root).expect("test cache directory must remove");
    }

    #[cfg(feature = "offline")]
    #[test]
    fn pinned_cache_objects_fail_fast_when_quota_cannot_be_satisfied() {
        use std::num::NonZeroU64;

        use futures::executor::block_on;
        use uuid::Uuid;

        use super::{CacheResourceKey, PersistentMediaCache};
        use crate::MediaValidator;

        let root = std::env::temp_dir().join(format!("waterkit-cache-{}", Uuid::new_v4()));
        let cache = AssetCache::new(&root);
        let mut store = block_on(PersistentMediaCache::open(
            &cache,
            NonZeroU64::new(8).expect("test quota must be non-zero"),
        ))
        .expect("test cache must open");
        let pinned = CacheResourceKey::new(
            &Url::parse("https://waterui.dev/video/pinned.m4s").expect("test URL must be valid"),
            &MediaValidator::none(),
        );
        let rejected = CacheResourceKey::new(
            &Url::parse("https://waterui.dev/video/rejected.m4s").expect("test URL must be valid"),
            &MediaValidator::none(),
        );
        block_on(store.store(pinned.clone(), None, b"123456", true))
            .expect("pinned object must store");
        assert!(block_on(store.store(rejected, None, b"abcdef", false)).is_err());
        assert_eq!(
            block_on(store.read(&pinned, None)).expect("pinned object must remain"),
            b"123456",
        );
        std::fs::remove_dir_all(root).expect("test cache directory must remove");
    }
}
