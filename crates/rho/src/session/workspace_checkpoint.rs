use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use rho_sdk::{Revision, SessionId};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

#[path = "workspace_checkpoint_restore.rs"]
mod restore;
pub(crate) use restore::{plan_restore, RestoreAudit, RestoreClassification, RestorePlan};
#[cfg(test)]
pub(crate) use restore::{RestoreAuditEntry, RestorePlanEntry};

use super::{
    layout::{set_private_dir_permissions, set_private_file_permissions, SessionUnit},
    tree::NodeId,
    Session,
};

const CHECKPOINT_FORMAT_VERSION: u32 = 1;
const CHECKPOINT_DIR_NAME: &str = "workspace-checkpoints";
const CHECKPOINT_JOURNAL_NAME: &str = "checkpoints.jsonl";

/// Default maximum content captured for one regular file: 2 MiB.
pub(crate) const DEFAULT_MAX_CHECKPOINT_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Default maximum serialized checkpoint data for one session: 64 MiB.
pub(crate) const DEFAULT_MAX_CHECKPOINT_SESSION_BYTES: u64 = 64 * 1024 * 1024;

/// Storage bounds applied at capture and append time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CheckpointLimits {
    pub(crate) max_file_bytes: u64,
    pub(crate) max_session_bytes: u64,
}

impl Default for CheckpointLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_CHECKPOINT_FILE_BYTES,
            max_session_bytes: DEFAULT_MAX_CHECKPOINT_SESSION_BYTES,
        }
    }
}

/// Portable metadata that affects restored file behavior.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BasicFileMetadata {
    pub(crate) readonly: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unix_mode: Option<u32>,
}

/// Why an existing path could not be captured safely.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum UnsupportedPath {
    Symlink,
    NonRegular,
    TooLarge { size: u64, limit: u64 },
    Unreadable,
}

/// SHA-256 digest encoded as lowercase hexadecimal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct FileDigest(String);

impl FileDigest {
    fn for_bytes(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        let mut encoded = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Self(encoded)
    }
}

/// A regular file captured before the first native mutation in a turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CapturedRegularFile {
    #[serde(with = "base64_bytes")]
    pub(crate) bytes: Vec<u8>,
    pub(crate) metadata: BasicFileMetadata,
    pub(crate) digest: FileDigest,
}

/// State before the first tracked mutation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum OriginalFileState {
    Absent,
    Regular(CapturedRegularFile),
    Unsupported { reason: UnsupportedPath },
}

/// Content-free state used for expected-after and current comparisons.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum ObservedFileState {
    Absent,
    Regular {
        digest: FileDigest,
        size: u64,
        metadata: BasicFileMetadata,
    },
    Unsupported {
        reason: UnsupportedPath,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileCheckpoint {
    pub(crate) path: PathBuf,
    pub(crate) original: OriginalFileState,
    pub(crate) expected_after: ObservedFileState,
}

/// A side effect which this checkpoint does not claim to reverse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UntrackedEffect {
    pub(crate) kind: UntrackedEffectKind,
    pub(crate) source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UntrackedEffectKind {
    ShellCommand,
    UntrackedMutatingTool,
    ExternalEffect,
}

/// Why a turn reached its durable boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckpointOutcome {
    Completed,
    Cancelled,
    Failed,
}

/// One finalized workspace checkpoint tied to a stable conversation node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkspaceCheckpoint {
    pub(crate) session_id: SessionId,
    pub(crate) node_id: NodeId,
    pub(crate) revision: Revision,
    pub(crate) started_at: u64,
    pub(crate) finalized_at: u64,
    pub(crate) outcome: CheckpointOutcome,
    pub(crate) files: Vec<FileCheckpoint>,
    #[serde(default)]
    pub(crate) limitations: Vec<UntrackedEffect>,
}

/// Result of capturing a path before mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureDisposition {
    Captured,
    AlreadyCaptured,
}

/// In-memory checkpoint under construction for one turn.
#[derive(Debug)]
pub(crate) struct OpenWorkspaceCheckpoint {
    session_id: SessionId,
    node_id: NodeId,
    started_at: u64,
    max_file_bytes: u64,
    originals: BTreeMap<PathBuf, OriginalFileState>,
    expected_after: BTreeMap<PathBuf, ObservedFileState>,
    limitations: Vec<UntrackedEffect>,
}

impl OpenWorkspaceCheckpoint {
    /// Captures a caller-authorized path once. This method does no workspace authorization.
    pub(crate) fn capture_path(&mut self, path: &Path) -> CaptureDisposition {
        if self.originals.contains_key(path) {
            return CaptureDisposition::AlreadyCaptured;
        }
        let state = capture_original(path, self.max_file_bytes);
        self.originals.insert(path.to_path_buf(), state);
        CaptureDisposition::Captured
    }

    pub(crate) fn record_untracked_effect(&mut self, effect: UntrackedEffect) {
        if !self.limitations.contains(&effect) {
            self.limitations.push(effect);
        }
    }
}

/// Session-owned append-only checkpoint persistence.
#[derive(Clone, Debug)]
pub(crate) struct WorkspaceCheckpointStore {
    session_id: SessionId,
    checkpoint_dir: PathBuf,
    journal_path: PathBuf,
    limits: CheckpointLimits,
}

impl WorkspaceCheckpointStore {
    fn for_session(session: &Session, limits: CheckpointLimits) -> anyhow::Result<Option<Self>> {
        let unit = SessionUnit::from_path(&session.path)
            .context("session path does not identify a durable session unit")?;
        let SessionUnit::Folder { dir } = unit else {
            return Ok(None);
        };
        let checkpoint_dir = dir.join(CHECKPOINT_DIR_NAME);
        Ok(Some(Self {
            session_id: SessionId::from_string(session.id.clone())?,
            journal_path: checkpoint_dir.join(CHECKPOINT_JOURNAL_NAME),
            checkpoint_dir,
            limits,
        }))
    }

    pub(crate) fn open(&self, node_id: NodeId) -> OpenWorkspaceCheckpoint {
        OpenWorkspaceCheckpoint {
            session_id: self.session_id.clone(),
            node_id,
            started_at: super::persistence::unix_timestamp_secs(),
            max_file_bytes: self.limits.max_file_bytes,
            originals: BTreeMap::new(),
            expected_after: BTreeMap::new(),
            limitations: Vec::new(),
        }
    }

    pub(crate) fn finalize_for_node(
        &self,
        mut open: OpenWorkspaceCheckpoint,
        node_id: NodeId,
        revision: Revision,
        outcome: CheckpointOutcome,
    ) -> anyhow::Result<WorkspaceCheckpoint> {
        open.node_id = node_id;
        self.finalize(open, revision, outcome)
    }

    /// Finalizes current states and appends one durable record.
    ///
    /// The caller must authorize every path before capture and again before any later restore.
    pub(crate) fn finalize(
        &self,
        open: OpenWorkspaceCheckpoint,
        revision: Revision,
        outcome: CheckpointOutcome,
    ) -> anyhow::Result<WorkspaceCheckpoint> {
        anyhow::ensure!(
            open.session_id == self.session_id,
            "checkpoint belongs to a different session"
        );

        let mut expected_after = open.expected_after;
        let files = open
            .originals
            .into_iter()
            .map(|(path, original)| FileCheckpoint {
                expected_after: expected_after
                    .remove(&path)
                    .unwrap_or_else(|| observe_path(&path, self.limits.max_file_bytes)),
                path,
                original,
            })
            .collect();
        let checkpoint = WorkspaceCheckpoint {
            session_id: self.session_id.clone(),
            node_id: open.node_id,
            revision,
            started_at: open.started_at,
            finalized_at: super::persistence::unix_timestamp_secs(),
            outcome,
            files,
            limitations: open.limitations,
        };
        self.append(&checkpoint)?;
        Ok(checkpoint)
    }

    pub(crate) fn list(&self) -> anyhow::Result<Vec<WorkspaceCheckpoint>> {
        read_journal(&self.journal_path).map(|journal| journal.checkpoints)
    }

    pub(crate) fn get(&self, node_id: &NodeId) -> anyhow::Result<Option<WorkspaceCheckpoint>> {
        Ok(self
            .list()?
            .into_iter()
            .find(|checkpoint| checkpoint.node_id == *node_id))
    }

    /// Reads current states without authorizing paths. Callers must authorize them first.
    #[cfg(test)]
    pub(crate) fn observe_current(
        &self,
        checkpoint: &WorkspaceCheckpoint,
    ) -> BTreeMap<PathBuf, ObservedFileState> {
        checkpoint
            .files
            .iter()
            .map(|file| {
                (
                    file.path.clone(),
                    observe_path(&file.path, self.limits.max_file_bytes),
                )
            })
            .collect()
    }

    pub(crate) fn observe_path(&self, path: &Path) -> ObservedFileState {
        observe_path(path, self.limits.max_file_bytes)
    }

    fn append(&self, checkpoint: &WorkspaceCheckpoint) -> anyhow::Result<()> {
        ensure_checkpoint_directory(&self.checkpoint_dir)?;

        let mut file = open_journal(&self.journal_path)?;
        fs2::FileExt::lock_exclusive(&file)?;
        let journal = read_locked_journal(&mut file)?;
        anyhow::ensure!(
            !journal
                .checkpoints
                .iter()
                .any(|stored| stored.node_id == checkpoint.node_id),
            "workspace checkpoint for node '{}' already exists",
            checkpoint.node_id
        );

        let mut encoded = serde_json::to_vec(&StoredCheckpointRecord {
            version: CHECKPOINT_FORMAT_VERSION,
            checkpoint: checkpoint.clone(),
        })?;
        encoded.push(b'\n');
        let new_size = journal
            .valid_len
            .checked_add(u64::try_from(encoded.len())?)
            .context("checkpoint journal size overflow")?;
        anyhow::ensure!(
            new_size <= self.limits.max_session_bytes,
            "checkpoint session quota exceeded: {new_size} bytes would exceed {} bytes",
            self.limits.max_session_bytes
        );

        file.set_len(journal.valid_len)?;
        file.seek(SeekFrom::Start(journal.valid_len))?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        Ok(())
    }
}

/// Turn-scoped mutation observer shared by native workspace tools.
#[derive(Clone, Debug)]
pub(crate) struct WorkspaceCheckpointTracker {
    enabled: bool,
    active: Arc<Mutex<Option<ActiveCheckpoint>>>,
}

#[derive(Debug)]
struct ActiveCheckpoint {
    store: WorkspaceCheckpointStore,
    open: OpenWorkspaceCheckpoint,
}

impl WorkspaceCheckpointTracker {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            active: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn begin_turn(&self, session: Option<&Session>) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let Some(session) = session else {
            return Ok(());
        };
        let Some(store) = session.workspace_checkpoint_store()? else {
            return Ok(());
        };
        let open = store.open(NodeId::new());
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        anyhow::ensure!(
            active.is_none(),
            "a workspace checkpoint turn is already active"
        );
        *active = Some(ActiveCheckpoint { store, open });
        Ok(())
    }

    pub(crate) fn discard_turn(&self) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = None;
    }

    pub(crate) fn finalize_turn(
        &self,
        node_id: NodeId,
        revision: Revision,
        outcome: CheckpointOutcome,
    ) -> anyhow::Result<Option<WorkspaceCheckpoint>> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(active) = active else {
            return Ok(None);
        };
        active
            .store
            .finalize_for_node(active.open, node_id, revision, outcome)
            .map(Some)
    }
}

impl rho_tools::WorkspaceMutationObserver for WorkspaceCheckpointTracker {
    fn before_mutation<'a>(
        &'a self,
        paths: &'a [&'a Path],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        let result = {
            let mut active = self
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(active) = active.as_mut() {
                for path in paths {
                    active.open.capture_path(path);
                }
            }
            Ok(())
        };
        Box::pin(std::future::ready(result))
    }

    fn after_mutation<'a>(
        &'a self,
        paths: &'a [&'a Path],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        let result = {
            let mut active = self
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(active) = active.as_mut() {
                for path in paths {
                    let state = active.store.observe_path(path);
                    active
                        .open
                        .expected_after
                        .insert((*path).to_path_buf(), state);
                }
            }
            Ok(())
        };
        Box::pin(std::future::ready(result))
    }

    fn mark_untracked_effect(&self, kind: rho_tools::UntrackedWorkspaceEffect, source: &str) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(active) = active.as_mut() else {
            return;
        };
        let effect = UntrackedEffect {
            kind: match kind {
                rho_tools::UntrackedWorkspaceEffect::ShellCommand => {
                    UntrackedEffectKind::ShellCommand
                }
                rho_tools::UntrackedWorkspaceEffect::MutatingTool => {
                    UntrackedEffectKind::UntrackedMutatingTool
                }
            },
            source: source.to_string(),
        };
        active.open.record_untracked_effect(effect);
    }
}

impl Session {
    pub(crate) fn active_checkpoint_target(&self) -> anyhow::Result<Option<(NodeId, Revision)>> {
        let tree = self.session_tree()?;
        Ok(tree.active_leaf_id().and_then(|id| {
            tree.node(id)
                .map(|node| (id.clone(), node.state().revision))
        }))
    }

    pub(crate) fn workspace_checkpoint_store(
        &self,
    ) -> anyhow::Result<Option<WorkspaceCheckpointStore>> {
        self.workspace_checkpoint_store_with_limits(CheckpointLimits::default())
    }

    pub(crate) fn workspace_checkpoint_store_with_limits(
        &self,
        limits: CheckpointLimits,
    ) -> anyhow::Result<Option<WorkspaceCheckpointStore>> {
        WorkspaceCheckpointStore::for_session(self, limits)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredCheckpointRecord {
    version: u32,
    checkpoint: WorkspaceCheckpoint,
}

#[derive(Debug)]
struct ReadJournal {
    checkpoints: Vec<WorkspaceCheckpoint>,
    valid_len: u64,
}

fn ensure_checkpoint_directory(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "checkpoint storage path is not a safe directory: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).with_context(|| {
                format!("failed to create checkpoint directory {}", path.display())
            })?;
        }
        Err(error) => return Err(error.into()),
    }
    set_private_dir_permissions(path)
}

fn open_journal(path: &Path) -> anyhow::Result<File> {
    reject_symlink_if_present(path)?;
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(path)
        .with_context(|| format!("failed to open checkpoint journal {}", path.display()))?;
    set_private_file_permissions(&file)?;
    Ok(file)
}

fn read_journal(path: &Path) -> anyhow::Result<ReadJournal> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ReadJournal {
                checkpoints: Vec::new(),
                valid_len: 0,
            });
        }
        Err(error) => return Err(error.into()),
        Ok(metadata) => anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "checkpoint journal cannot be a symlink: {}",
            path.display()
        ),
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path)?;
    read_locked_journal(&mut file)
}

fn reject_symlink_if_present(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "checkpoint journal cannot be a symlink: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn read_locked_journal(file: &mut File) -> anyhow::Result<ReadJournal> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;

    let mut checkpoints = Vec::new();
    let mut identities = HashSet::new();
    let mut valid_len = 0u64;
    let mut lines = bytes.split_inclusive(|byte| *byte == b'\n').peekable();
    while let Some(line) = lines.next() {
        if !line.ends_with(b"\n") {
            break;
        }
        let record = match serde_json::from_slice::<StoredCheckpointRecord>(&line[..line.len() - 1])
        {
            Ok(record) => record,
            Err(_) if lines.peek().is_none() => break,
            Err(error) => return Err(error).context("invalid checkpoint journal record"),
        };
        anyhow::ensure!(
            record.version == CHECKPOINT_FORMAT_VERSION,
            "unsupported workspace checkpoint version {}",
            record.version
        );
        anyhow::ensure!(
            identities.insert(record.checkpoint.node_id.clone()),
            "duplicate workspace checkpoint node '{}'",
            record.checkpoint.node_id
        );
        valid_len += u64::try_from(line.len())?;
        checkpoints.push(record.checkpoint);
    }
    Ok(ReadJournal {
        checkpoints,
        valid_len,
    })
}

fn capture_original(path: &Path, max_file_bytes: u64) -> OriginalFileState {
    match read_regular(path, max_file_bytes) {
        ReadFileState::Absent => OriginalFileState::Absent,
        ReadFileState::Regular { bytes, metadata } => {
            OriginalFileState::Regular(CapturedRegularFile {
                digest: FileDigest::for_bytes(&bytes),
                bytes,
                metadata,
            })
        }
        ReadFileState::Unsupported(reason) => OriginalFileState::Unsupported { reason },
    }
}

fn observe_path(path: &Path, max_file_bytes: u64) -> ObservedFileState {
    match read_regular(path, max_file_bytes) {
        ReadFileState::Absent => ObservedFileState::Absent,
        ReadFileState::Regular { bytes, metadata } => ObservedFileState::Regular {
            digest: FileDigest::for_bytes(&bytes),
            size: bytes.len() as u64,
            metadata,
        },
        ReadFileState::Unsupported(reason) => ObservedFileState::Unsupported { reason },
    }
}

enum ReadFileState {
    Absent,
    Regular {
        bytes: Vec<u8>,
        metadata: BasicFileMetadata,
    },
    Unsupported(UnsupportedPath),
}

fn read_regular(path: &Path, max_file_bytes: u64) -> ReadFileState {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ReadFileState::Absent,
        Err(_) => return ReadFileState::Unsupported(UnsupportedPath::Unreadable),
    };
    if path_metadata.file_type().is_symlink() {
        return ReadFileState::Unsupported(UnsupportedPath::Symlink);
    }
    if !path_metadata.is_file() {
        return ReadFileState::Unsupported(UnsupportedPath::NonRegular);
    }
    if path_metadata.len() > max_file_bytes {
        return ReadFileState::Unsupported(UnsupportedPath::TooLarge {
            size: path_metadata.len(),
            limit: max_file_bytes,
        });
    }

    let file = match open_regular_no_follow(path) {
        Ok(file) => file,
        Err(_) => return ReadFileState::Unsupported(UnsupportedPath::Unreadable),
    };
    let metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => basic_metadata(&metadata),
        Ok(_) => return ReadFileState::Unsupported(UnsupportedPath::NonRegular),
        Err(_) => return ReadFileState::Unsupported(UnsupportedPath::Unreadable),
    };
    let read_limit = max_file_bytes.saturating_add(1);
    let mut bytes = Vec::new();
    if file.take(read_limit).read_to_end(&mut bytes).is_err() {
        return ReadFileState::Unsupported(UnsupportedPath::Unreadable);
    }
    if bytes.len() as u64 > max_file_bytes {
        return ReadFileState::Unsupported(UnsupportedPath::TooLarge {
            size: bytes.len() as u64,
            limit: max_file_bytes,
        });
    }
    ReadFileState::Regular { bytes, metadata }
}

fn open_regular_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

fn basic_metadata(metadata: &fs::Metadata) -> BasicFileMetadata {
    #[cfg(unix)]
    let unix_mode = {
        use std::os::unix::fs::PermissionsExt as _;
        Some(metadata.permissions().mode() & 0o7777)
    };
    #[cfg(not(unix))]
    let unix_mode = None;

    BasicFileMetadata {
        readonly: metadata.permissions().readonly(),
        unix_mode,
    }
}

mod base64_bytes {
    use super::*;
    use serde::de::Error as _;

    pub(super) fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&BASE64.encode(bytes))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        BASE64.decode(encoded).map_err(D::Error::custom)
    }
}

#[cfg(test)]
#[path = "workspace_checkpoint_tests.rs"]
mod tests;
