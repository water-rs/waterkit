use std::{num::NonZeroU64, path::PathBuf};

use async_channel::{Receiver, Sender};
use async_fs::File;
use futures::{AsyncWriteExt as _, FutureExt as _, StreamExt as _, select_biased};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;
use waterkit_video_core::Error;

use crate::{AssetCache, MediaRequest, atomic, fetch_media};

const STATE_VERSION: u16 = 1;
const STATE_FILE_NAME: &str = "package.json";
const MANIFEST_FILE_NAME: &str = "manifest.bin";

/// Stable identifier for one persisted offline media package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OfflinePackageId(Uuid);

impl OfflinePackageId {
    /// Creates a globally unique offline package identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Returns the UUID representation used by persistent package paths.
    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for OfflinePackageId {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable, header-independent identity for one resource in an offline package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OfflineResourceId(String);

impl OfflineResourceId {
    fn from_request(request: &MediaRequest) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(request.url().as_str().as_bytes());
        match request.byte_range() {
            Some(range) => {
                hasher.update(&range.start().to_le_bytes());
                hasher.update(&range.end_exclusive().to_le_bytes());
            }
            None => {
                hasher.update(b"complete-resource");
            }
        }
        Self(hasher.finalize().to_hex().to_string())
    }

    /// Returns the stable hexadecimal resource identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One bounded Zenwave resource selected for offline availability.
#[derive(Debug, Clone)]
pub struct OfflineResource {
    id: OfflineResourceId,
    request: MediaRequest,
}

impl OfflineResource {
    /// Creates an offline resource from an already bounded media request.
    #[must_use]
    pub fn new(request: MediaRequest) -> Self {
        let id = OfflineResourceId::from_request(&request);
        Self { id, request }
    }

    /// Returns the stable identity used by the persisted resource map.
    #[must_use]
    pub const fn id(&self) -> &OfflineResourceId {
        &self.id
    }

    /// Returns the remote request used while the task is online.
    #[must_use]
    pub const fn request(&self) -> &MediaRequest {
        &self.request
    }
}

/// Complete immutable plan for one resumable offline package download.
#[derive(Debug, Clone)]
pub struct OfflinePackagePlan {
    id: OfflinePackageId,
    source_identity: String,
    manifest_snapshot: Vec<u8>,
    resources: Vec<OfflineResource>,
}

impl OfflinePackagePlan {
    /// Creates a package plan with an immutable manifest and ordered resources.
    ///
    /// Header values are deliberately excluded from persistent identities, so
    /// authorization and cookie material never enters the package index.
    ///
    /// # Errors
    ///
    /// Returns an error when no resource was selected for offline playback.
    pub fn new(
        id: OfflinePackageId,
        source: &Url,
        manifest_snapshot: Vec<u8>,
        resources: impl IntoIterator<Item = OfflineResource>,
    ) -> Result<Self, Error> {
        let resources = resources.into_iter().collect::<Vec<_>>();
        if resources.is_empty() {
            return Err(Error::Streaming(String::from(
                "offline package requires at least one media resource",
            )));
        }
        let distinct_resources = resources
            .iter()
            .map(OfflineResource::id)
            .collect::<std::collections::BTreeSet<_>>();
        if distinct_resources.len() != resources.len() {
            return Err(Error::Streaming(String::from(
                "offline package contains duplicate media resources",
            )));
        }
        let source_identity = blake3::hash(source.as_str().as_bytes())
            .to_hex()
            .to_string();
        Ok(Self {
            id,
            source_identity,
            manifest_snapshot,
            resources,
        })
    }

    /// Returns the persistent package identifier.
    #[must_use]
    pub const fn id(&self) -> OfflinePackageId {
        self.id
    }

    /// Returns the immutable manifest bytes captured for offline playback.
    #[must_use]
    pub fn manifest_snapshot(&self) -> &[u8] {
        &self.manifest_snapshot
    }

    /// Returns selected resources in deterministic download order.
    #[must_use]
    pub fn resources(&self) -> &[OfflineResource] {
        &self.resources
    }
}

/// Aggregate progress for one resumable package download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfflineDownloadProgress {
    /// Number of completely committed resources.
    pub completed_resources: usize,
    /// Total resources selected by the immutable plan.
    pub total_resources: usize,
    /// Total committed media bytes, excluding index and manifest files.
    pub committed_bytes: u64,
}

/// Observable state transition emitted by an offline download task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfflineDownloadEvent {
    /// The package state has been opened or created.
    Started(OfflineDownloadProgress),
    /// One resource was atomically committed and indexed.
    ResourceCommitted {
        /// Identity of the committed resource.
        resource: OfflineResourceId,
        /// Updated aggregate progress.
        progress: OfflineDownloadProgress,
    },
    /// Network work is suspended until an explicit resume command.
    Paused(OfflineDownloadProgress),
    /// A paused task has resumed network work.
    Resumed(OfflineDownloadProgress),
    /// Every selected resource and the manifest are durable.
    Finished(OfflineDownloadProgress),
    /// The package and every committed resource were removed.
    Cancelled(OfflineDownloadProgress),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfflineDownloadCommand {
    Pause,
    Resume,
    Cancel,
}

/// Event receiver for one offline download task.
#[derive(Debug, Clone)]
pub struct OfflineDownloadEvents {
    receiver: Receiver<OfflineDownloadEvent>,
}

impl OfflineDownloadEvents {
    /// Waits for the next task event.
    ///
    /// # Errors
    ///
    /// Returns an error if the task exits without producing a terminal event.
    pub async fn next(&self) -> Result<OfflineDownloadEvent, Error> {
        self.receiver.recv().await.map_err(|_| {
            Error::Streaming(String::from(
                "offline download event channel closed before a terminal event",
            ))
        })
    }
}

/// Cloneable command handle for one offline download task.
#[derive(Debug, Clone)]
pub struct OfflineDownloadController {
    sender: Sender<OfflineDownloadCommand>,
}

impl OfflineDownloadController {
    /// Suspends the active request and preserves committed package state.
    ///
    /// # Errors
    ///
    /// Returns an error when the task has already terminated.
    pub async fn pause(&self) -> Result<(), Error> {
        self.send(OfflineDownloadCommand::Pause).await
    }

    /// Resumes a suspended task from its first uncommitted resource.
    ///
    /// # Errors
    ///
    /// Returns an error when the task has already terminated.
    pub async fn resume(&self) -> Result<(), Error> {
        self.send(OfflineDownloadCommand::Resume).await
    }

    /// Cancels the task and removes its complete persisted package directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the task has already terminated.
    pub async fn cancel(&self) -> Result<(), Error> {
        self.send(OfflineDownloadCommand::Cancel).await
    }

    async fn send(&self, command: OfflineDownloadCommand) -> Result<(), Error> {
        self.sender.send(command).await.map_err(|_| {
            Error::Streaming(String::from(
                "offline download command was sent after task termination",
            ))
        })
    }
}

/// Terminal result produced by [`OfflineDownload::run`].
#[derive(Debug)]
pub enum OfflineDownloadOutcome {
    /// Every selected resource is available and checksum-verified.
    Complete(OfflinePackage),
    /// The controller cancelled and removed the package.
    Cancelled,
}

/// Application-owned offline package manager with an explicit storage quota.
#[derive(Debug, Clone)]
pub struct OfflineManager {
    cache: AssetCache,
    maximum_package_bytes: NonZeroU64,
}

impl OfflineManager {
    /// Creates a manager rooted in an application-selected cache directory.
    #[must_use]
    pub const fn new(cache: AssetCache, maximum_package_bytes: NonZeroU64) -> Self {
        Self {
            cache,
            maximum_package_bytes,
        }
    }

    /// Prepares a resumable task and its independent command/event handles.
    #[must_use]
    pub fn prepare(
        &self,
        plan: OfflinePackagePlan,
    ) -> (
        OfflineDownload,
        OfflineDownloadController,
        OfflineDownloadEvents,
    ) {
        let (command_sender, command_receiver) = async_channel::unbounded();
        let (event_sender, event_receiver) = async_channel::unbounded();
        (
            OfflineDownload {
                root: self.package_root(plan.id),
                maximum_package_bytes: self.maximum_package_bytes,
                plan,
                commands: command_receiver,
                events: event_sender,
            },
            OfflineDownloadController {
                sender: command_sender,
            },
            OfflineDownloadEvents {
                receiver: event_receiver,
            },
        )
    }

    /// Opens and validates a previously completed package.
    ///
    /// # Errors
    ///
    /// Returns an error for missing, incomplete, incompatible, or corrupt state.
    pub async fn open(&self, id: OfflinePackageId) -> Result<OfflinePackage, Error> {
        OfflinePackage::open(self.package_root(id), id).await
    }

    fn package_root(&self, id: OfflinePackageId) -> PathBuf {
        self.cache
            .root()
            .join("offline")
            .join(id.as_uuid().to_string())
    }
}

/// Resumable future owner for one offline package.
#[derive(Debug)]
pub struct OfflineDownload {
    root: PathBuf,
    maximum_package_bytes: NonZeroU64,
    plan: OfflinePackagePlan,
    commands: Receiver<OfflineDownloadCommand>,
    events: Sender<OfflineDownloadEvent>,
}

impl OfflineDownload {
    /// Runs until completion or explicit cancellation.
    ///
    /// Pausing drops the active Zenwave request immediately. Resuming restarts
    /// only that uncommitted resource; committed resources are checksum-verified
    /// from the crash-safe package index before network work begins.
    ///
    /// # Errors
    ///
    /// Returns an error for network, quota, persistence, or integrity failures.
    pub async fn run(self) -> Result<OfflineDownloadOutcome, Error> {
        async_fs::create_dir_all(&self.root).await?;
        remove_partial_files(&self.root).await?;
        let mut state = PackageState::load_or_create(&self.root, &self.plan).await?;
        state.verify_committed(&self.root).await?;
        let mut progress = state.progress(self.plan.resources.len())?;
        self.emit(OfflineDownloadEvent::Started(progress)).await;
        let mut paused = false;

        for resource in &self.plan.resources {
            if state.contains(resource.id()) {
                continue;
            }
            loop {
                while let Ok(command) = self.commands.try_recv() {
                    if self.apply_command(command, &mut paused, progress).await? {
                        return Ok(OfflineDownloadOutcome::Cancelled);
                    }
                }

                if paused {
                    let command = self.next_command().await?;
                    if self.apply_command(command, &mut paused, progress).await? {
                        return Ok(OfflineDownloadOutcome::Cancelled);
                    }
                    continue;
                }

                let fetch = fetch_media(resource.request().clone()).fuse();
                let command = self.commands.recv().fuse();
                futures::pin_mut!(fetch, command);
                select_biased! {
                    command = command => {
                        let command = command.map_err(|_| command_channel_closed())?;
                        if self.apply_command(command, &mut paused, progress).await? {
                            return Ok(OfflineDownloadOutcome::Cancelled);
                        }
                    }
                    response = fetch => {
                        let bytes = response?.into_bytes();
                        let bytes_len = u64::try_from(bytes.len()).map_err(|_| {
                            Error::Streaming(String::from(
                                "offline resource length does not fit u64",
                            ))
                        })?;
                        let next_total = progress.committed_bytes.checked_add(bytes_len).ok_or_else(|| {
                            Error::Streaming(String::from("offline package byte count overflowed u64"))
                        })?;
                        if next_total > self.maximum_package_bytes.get() {
                            return Err(Error::Streaming(format!(
                                "offline package {} exceeds its {}-byte quota",
                                self.plan.id.as_uuid(),
                                self.maximum_package_bytes,
                            )));
                        }
                        let committed = commit_resource(&self.root, resource.id(), &bytes).await?;
                        state.resources.push(committed);
                        state.store(&self.root).await?;
                        progress = state.progress(self.plan.resources.len())?;
                        self.emit(OfflineDownloadEvent::ResourceCommitted {
                            resource: resource.id().clone(),
                            progress,
                        }).await;
                        break;
                    }
                }
            }
        }

        state.complete = true;
        state.store(&self.root).await?;
        progress = state.progress(self.plan.resources.len())?;
        self.emit(OfflineDownloadEvent::Finished(progress)).await;
        let package = OfflinePackage::open(self.root, self.plan.id).await?;
        Ok(OfflineDownloadOutcome::Complete(package))
    }

    async fn next_command(&self) -> Result<OfflineDownloadCommand, Error> {
        self.commands
            .recv()
            .await
            .map_err(|_| command_channel_closed())
    }

    async fn apply_command(
        &self,
        command: OfflineDownloadCommand,
        paused: &mut bool,
        progress: OfflineDownloadProgress,
    ) -> Result<bool, Error> {
        match command {
            OfflineDownloadCommand::Pause if !*paused => {
                *paused = true;
                self.emit(OfflineDownloadEvent::Paused(progress)).await;
            }
            OfflineDownloadCommand::Resume if *paused => {
                *paused = false;
                self.emit(OfflineDownloadEvent::Resumed(progress)).await;
            }
            OfflineDownloadCommand::Cancel => {
                remove_package(&self.root).await?;
                self.emit(OfflineDownloadEvent::Cancelled(progress)).await;
                return Ok(true);
            }
            OfflineDownloadCommand::Pause | OfflineDownloadCommand::Resume => {}
        }
        Ok(false)
    }

    async fn emit(&self, event: OfflineDownloadEvent) {
        let _ = self.events.send(event).await;
    }
}

/// Verified, complete package usable by an offline playback source.
#[derive(Debug)]
pub struct OfflinePackage {
    root: PathBuf,
    state: PackageState,
}

impl OfflinePackage {
    async fn open(root: PathBuf, expected_id: OfflinePackageId) -> Result<Self, Error> {
        let state = PackageState::load(&root).await?;
        if state.id != expected_id {
            return Err(Error::Streaming(format!(
                "offline package path contains {}, expected {}",
                state.id.as_uuid(),
                expected_id.as_uuid(),
            )));
        }
        if !state.complete {
            return Err(Error::Streaming(format!(
                "offline package {} is incomplete",
                expected_id.as_uuid(),
            )));
        }
        state.verify_committed(&root).await?;
        Ok(Self { root, state })
    }

    /// Returns the package identifier.
    #[must_use]
    pub const fn id(&self) -> OfflinePackageId {
        self.state.id
    }

    /// Returns the immutable offline manifest snapshot path.
    #[must_use]
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE_NAME)
    }

    /// Returns the committed path for one planned resource.
    #[must_use]
    pub fn resource_path(&self, id: &OfflineResourceId) -> Option<PathBuf> {
        self.state
            .resources
            .iter()
            .find(|resource| &resource.id == id)
            .map(|resource| self.root.join(&resource.file_name))
    }

    /// Revalidates every resource checksum without network access.
    ///
    /// # Errors
    ///
    /// Returns an error when any indexed file is missing, truncated, or corrupt.
    pub async fn verify(&self) -> Result<(), Error> {
        self.state.verify_committed(&self.root).await
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PackageState {
    version: u16,
    id: OfflinePackageId,
    source_identity: String,
    manifest_checksum: String,
    planned_resources: Vec<OfflineResourceId>,
    resources: Vec<CommittedResource>,
    complete: bool,
}

impl PackageState {
    async fn load_or_create(
        root: &std::path::Path,
        plan: &OfflinePackagePlan,
    ) -> Result<Self, Error> {
        let state_path = root.join(STATE_FILE_NAME);
        match async_fs::metadata(&state_path).await {
            Ok(_) => {
                let state = Self::load(root).await?;
                state.matches(plan)?;
                return Ok(state);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let manifest_checksum = blake3::hash(&plan.manifest_snapshot).to_hex().to_string();
        write_atomic(root.join(MANIFEST_FILE_NAME), &plan.manifest_snapshot).await?;
        let state = Self {
            version: STATE_VERSION,
            id: plan.id,
            source_identity: plan.source_identity.clone(),
            manifest_checksum,
            planned_resources: plan
                .resources
                .iter()
                .map(|resource| resource.id().clone())
                .collect(),
            resources: Vec::new(),
            complete: false,
        };
        state.store(root).await?;
        Ok(state)
    }

    async fn load(root: &std::path::Path) -> Result<Self, Error> {
        let bytes = async_fs::read(root.join(STATE_FILE_NAME))
            .await
            .map_err(|error| {
                Error::Streaming(format!("failed to read offline package state: {error}"))
            })?;
        let state = serde_json::from_slice::<Self>(&bytes).map_err(|error| {
            Error::Streaming(format!("failed to parse offline package state: {error}"))
        })?;
        if state.version != STATE_VERSION {
            return Err(Error::Streaming(format!(
                "offline package state version {} is unsupported; expected {STATE_VERSION}",
                state.version,
            )));
        }
        state.validate()?;
        Ok(state)
    }

    fn matches(&self, plan: &OfflinePackagePlan) -> Result<(), Error> {
        if self.id != plan.id || self.source_identity != plan.source_identity {
            return Err(Error::Streaming(String::from(
                "offline package resume plan does not match persisted media identity",
            )));
        }
        let manifest_checksum = blake3::hash(&plan.manifest_snapshot).to_hex().to_string();
        if self.manifest_checksum != manifest_checksum {
            return Err(Error::Streaming(String::from(
                "offline package resume manifest differs from persisted snapshot",
            )));
        }
        let planned = plan
            .resources
            .iter()
            .map(|resource| resource.id().clone())
            .collect::<Vec<_>>();
        if self.planned_resources != planned {
            return Err(Error::Streaming(String::from(
                "offline package resume resources differ from the immutable persisted plan",
            )));
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        let planned = self
            .planned_resources
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        if planned.len() != self.planned_resources.len() {
            return Err(Error::Streaming(String::from(
                "offline package state contains duplicate planned resources",
            )));
        }
        let committed = self
            .resources
            .iter()
            .map(|resource| &resource.id)
            .collect::<std::collections::BTreeSet<_>>();
        if committed.len() != self.resources.len() {
            return Err(Error::Streaming(String::from(
                "offline package state contains duplicate committed resources",
            )));
        }
        if !committed.is_subset(&planned) {
            return Err(Error::Streaming(String::from(
                "offline package state contains an unplanned committed resource",
            )));
        }
        if self.complete && committed != planned {
            return Err(Error::Streaming(String::from(
                "completed offline package does not contain every planned resource",
            )));
        }
        if self
            .resources
            .iter()
            .any(|resource| resource.file_name != format!("{}.blob", resource.id.as_str()))
        {
            return Err(Error::Streaming(String::from(
                "offline package state contains an invalid resource path",
            )));
        }
        Ok(())
    }

    fn contains(&self, id: &OfflineResourceId) -> bool {
        self.resources.iter().any(|resource| &resource.id == id)
    }

    fn progress(&self, total_resources: usize) -> Result<OfflineDownloadProgress, Error> {
        let committed_bytes = self.resources.iter().try_fold(0_u64, |total, resource| {
            total.checked_add(resource.bytes).ok_or_else(|| {
                Error::Streaming(String::from("offline package byte count overflowed u64"))
            })
        })?;
        Ok(OfflineDownloadProgress {
            completed_resources: self.resources.len(),
            total_resources,
            committed_bytes,
        })
    }

    async fn store(&self, root: &std::path::Path) -> Result<(), Error> {
        let bytes = serde_json::to_vec(self).map_err(|error| {
            Error::Streaming(format!(
                "failed to serialize offline package state: {error}"
            ))
        })?;
        write_atomic(root.join(STATE_FILE_NAME), &bytes).await
    }

    async fn verify_committed(&self, root: &std::path::Path) -> Result<(), Error> {
        verify_file(
            &root.join(MANIFEST_FILE_NAME),
            &self.manifest_checksum,
            None,
        )
        .await?;
        for resource in &self.resources {
            verify_file(
                &root.join(&resource.file_name),
                &resource.checksum,
                Some(resource.bytes),
            )
            .await?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommittedResource {
    id: OfflineResourceId,
    file_name: String,
    bytes: u64,
    checksum: String,
}

async fn commit_resource(
    root: &std::path::Path,
    id: &OfflineResourceId,
    bytes: &[u8],
) -> Result<CommittedResource, Error> {
    let checksum = blake3::hash(bytes).to_hex().to_string();
    let file_name = format!("{}.blob", id.as_str());
    write_atomic(root.join(&file_name), bytes).await?;
    let bytes = u64::try_from(bytes.len())
        .map_err(|_| Error::Streaming(String::from("offline resource length does not fit u64")))?;
    Ok(CommittedResource {
        id: id.clone(),
        file_name,
        bytes,
        checksum,
    })
}

async fn write_atomic(destination: PathBuf, bytes: &[u8]) -> Result<(), Error> {
    let parent = destination.parent().ok_or_else(|| {
        Error::Streaming(String::from("offline package destination has no parent"))
    })?;
    async_fs::create_dir_all(parent).await?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Streaming(String::from("offline package file name is not UTF-8")))?;
    let partial = parent.join(format!(".{file_name}.{}.part", Uuid::new_v4()));
    let mut file = File::create(&partial).await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);
    if let Err(error) = atomic::replace(partial.clone(), destination).await {
        let _ = async_fs::remove_file(partial).await;
        return Err(error);
    }
    Ok(())
}

async fn verify_file(
    path: &std::path::Path,
    expected_checksum: &str,
    expected_bytes: Option<u64>,
) -> Result<(), Error> {
    let bytes = async_fs::read(path).await.map_err(|error| {
        Error::Streaming(format!(
            "failed to read offline package file {}: {error}",
            path.display(),
        ))
    })?;
    if let Some(expected_bytes) = expected_bytes {
        let actual_bytes = u64::try_from(bytes.len()).map_err(|_| {
            Error::Streaming(String::from("offline resource length does not fit u64"))
        })?;
        if actual_bytes != expected_bytes {
            return Err(Error::Streaming(format!(
                "offline package file {} has {actual_bytes} bytes, expected {expected_bytes}",
                path.display(),
            )));
        }
    }
    let actual_checksum = blake3::hash(&bytes).to_hex().to_string();
    if actual_checksum != expected_checksum {
        return Err(Error::Streaming(format!(
            "offline package checksum mismatch for {}",
            path.display(),
        )));
    }
    Ok(())
}

async fn remove_partial_files(root: &std::path::Path) -> Result<(), Error> {
    let mut entries = async_fs::read_dir(root).await?;
    while let Some(entry) = entries.next().await {
        let entry = entry?;
        if atomic::is_partial_file_name(&entry.file_name()) {
            async_fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

async fn remove_package(root: &std::path::Path) -> Result<(), Error> {
    match async_fs::remove_dir_all(root).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn command_channel_closed() -> Error {
    Error::Streaming(String::from(
        "offline download controller was dropped before task termination",
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        num::{NonZeroU64, NonZeroUsize},
        thread,
    };

    use futures::executor::block_on;
    use url::Url;
    use uuid::Uuid;

    use super::{
        OfflineDownloadEvent, OfflineDownloadOutcome, OfflineManager, OfflinePackageId,
        OfflinePackagePlan, OfflineResource,
    };
    use crate::{AssetCache, MediaByteRange, MediaRequest};

    #[test]
    fn package_plan_requires_selected_resources() {
        let result = OfflinePackagePlan::new(
            OfflinePackageId::new(),
            &Url::parse("https://waterui.dev/video/master.m3u8").expect("test URL must be valid"),
            Vec::new(),
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn package_download_commits_and_reopens_verified_resources() {
        let server = TcpListener::bind(("127.0.0.1", 0)).expect("test server must bind");
        let address = server.local_addr().expect("test server address must exist");
        let worker = thread::spawn(move || {
            for body in [b"first-segment".as_slice(), b"second-segment".as_slice()] {
                let (mut stream, _) = server.accept().expect("test request must arrive");
                let mut request = [0_u8; 4_096];
                let read = stream.read(&mut request).expect("test request must read");
                assert!(read > 0);
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len(),
                );
                stream
                    .write_all(headers.as_bytes())
                    .expect("test response headers must write");
                stream
                    .write_all(body)
                    .expect("test response body must write");
            }
        });

        let root = std::env::temp_dir().join(format!("waterkit-offline-{}", Uuid::new_v4()));
        let source = Url::parse(&format!("http://{address}/master.m3u8"))
            .expect("test source URL must be valid");
        let resources = ["first.m4s", "second.m4s"].map(|name| {
            let url = source.join(name).expect("test resource URL must resolve");
            OfflineResource::new(MediaRequest::new(
                url,
                NonZeroUsize::new(1024).expect("test response bound must be non-zero"),
            ))
        });
        let resource_ids = resources.each_ref().map(|resource| resource.id().clone());
        let id = OfflinePackageId::new();
        let plan = OfflinePackagePlan::new(id, &source, b"manifest".to_vec(), resources)
            .expect("test package plan must be valid");
        let manager = OfflineManager::new(
            AssetCache::new(&root),
            NonZeroU64::new(1024 * 1024).expect("test quota must be non-zero"),
        );
        let (download, _controller, events) = manager.prepare(plan);
        let outcome = block_on(download.run()).expect("test package download must succeed");
        let OfflineDownloadOutcome::Complete(package) = outcome else {
            panic!("test package must complete");
        };
        block_on(package.verify()).expect("completed package must verify");
        for id in &resource_ids {
            assert!(package.resource_path(id).is_some());
        }
        let reopened = block_on(manager.open(id)).expect("completed package must reopen");
        block_on(reopened.verify()).expect("reopened package must verify");

        let mut committed = 0;
        loop {
            match block_on(events.next()).expect("task event must be available") {
                OfflineDownloadEvent::ResourceCommitted { .. } => committed += 1,
                OfflineDownloadEvent::Finished(progress) => {
                    assert_eq!(progress.completed_resources, 2);
                    break;
                }
                OfflineDownloadEvent::Started(_)
                | OfflineDownloadEvent::Paused(_)
                | OfflineDownloadEvent::Resumed(_)
                | OfflineDownloadEvent::Cancelled(_) => {}
            }
        }
        assert_eq!(committed, 2);
        worker.join().expect("test server must exit cleanly");
        std::fs::remove_dir_all(root).expect("test package directory must remove");
    }

    #[test]
    fn cancellation_removes_persisted_package_state_without_contacting_network() {
        let root = std::env::temp_dir().join(format!("waterkit-offline-{}", Uuid::new_v4()));
        let source = Url::parse("https://waterui.dev/video/master.m3u8")
            .expect("test source URL must be valid");
        let resource = OfflineResource::new(MediaRequest::new(
            source
                .join("segment.m4s")
                .expect("test resource URL must resolve"),
            NonZeroUsize::new(1024).expect("test response bound must be non-zero"),
        ));
        let id = OfflinePackageId::new();
        let plan = OfflinePackagePlan::new(id, &source, b"manifest".to_vec(), [resource])
            .expect("test package plan must be valid");
        let manager = OfflineManager::new(
            AssetCache::new(&root),
            NonZeroU64::new(1024 * 1024).expect("test quota must be non-zero"),
        );
        let (download, controller, events) = manager.prepare(plan);
        block_on(controller.cancel()).expect("test cancellation command must send");
        let outcome = block_on(download.run()).expect("test cancellation must succeed");
        assert!(matches!(outcome, OfflineDownloadOutcome::Cancelled));
        let started = block_on(events.next()).expect("started event must be available");
        assert!(matches!(started, OfflineDownloadEvent::Started(_)));
        let cancelled = block_on(events.next()).expect("cancelled event must be available");
        assert!(matches!(cancelled, OfflineDownloadEvent::Cancelled(_)));
        assert!(block_on(manager.open(id)).is_err());
        if root.exists() {
            std::fs::remove_dir_all(root).expect("test package root must remove");
        }
    }

    #[test]
    fn pause_resume_and_cancel_commands_produce_ordered_durable_state_transitions() {
        let root = std::env::temp_dir().join(format!("waterkit-offline-{}", Uuid::new_v4()));
        let source = Url::parse("https://waterui.dev/video/master.m3u8")
            .expect("test source URL must be valid");
        let resource = OfflineResource::new(MediaRequest::new(
            source
                .join("segment.m4s")
                .expect("test resource URL must resolve"),
            NonZeroUsize::new(1024).expect("test response bound must be non-zero"),
        ));
        let plan = OfflinePackagePlan::new(
            OfflinePackageId::new(),
            &source,
            b"manifest".to_vec(),
            [resource],
        )
        .expect("test package plan must be valid");
        let manager = OfflineManager::new(
            AssetCache::new(&root),
            NonZeroU64::new(1024 * 1024).expect("test quota must be non-zero"),
        );
        let (download, controller, events) = manager.prepare(plan);
        block_on(controller.pause()).expect("pause command must send");
        block_on(controller.resume()).expect("resume command must send");
        block_on(controller.cancel()).expect("cancel command must send");
        let outcome = block_on(download.run()).expect("command sequence must succeed");
        assert!(matches!(outcome, OfflineDownloadOutcome::Cancelled));
        assert!(matches!(
            block_on(events.next()).expect("started event must be available"),
            OfflineDownloadEvent::Started(_),
        ));
        assert!(matches!(
            block_on(events.next()).expect("paused event must be available"),
            OfflineDownloadEvent::Paused(_),
        ));
        assert!(matches!(
            block_on(events.next()).expect("resumed event must be available"),
            OfflineDownloadEvent::Resumed(_),
        ));
        assert!(matches!(
            block_on(events.next()).expect("cancelled event must be available"),
            OfflineDownloadEvent::Cancelled(_),
        ));
        if root.exists() {
            std::fs::remove_dir_all(root).expect("test package root must remove");
        }
    }

    #[test]
    fn byte_ranges_produce_distinct_header_independent_resource_ids() {
        let url =
            Url::parse("https://waterui.dev/video/segments.m4s").expect("test URL must be valid");
        let first = OfflineResource::new(
            MediaRequest::ranged(
                url.clone(),
                MediaByteRange::new(0, 100).expect("test range must be valid"),
            )
            .expect("test request must be valid"),
        );
        let second = OfflineResource::new(
            MediaRequest::ranged(
                url,
                MediaByteRange::new(100, 200).expect("test range must be valid"),
            )
            .expect("test request must be valid"),
        );
        assert_ne!(first.id(), second.id());
    }
}
