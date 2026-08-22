use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use serde::{Deserialize, Serialize};

#[cfg(test)]
use rho_providers::model::ModelIdentity;
use rho_providers::model::{ContentBlock, Message};
#[cfg(test)]
use rho_sdk::SessionId;
use rho_sdk::{CompactionState, Revision, SessionSnapshot};

use super::snapshot_delta::StoredSnapshotDelta;
use super::tree::{NodeId, SessionNode, SessionTree};
use super::{index, Session, SessionHistories, SessionIndexRecord, SessionSummary};

// Layout helpers live in `layout`; re-export the historical `persistence::` surface.
pub(super) use super::layout::{
    ensure_session_dir, parse_timestamp, resolve_transcript_path, session_dir_in_root,
    session_id_from_path, session_root, session_web_dir, set_private_dir_permissions,
    set_private_file_permissions, timestamp, unix_timestamp_secs, workspace_key, SessionUnit,
    SESSION_TRANSCRIPT_FILE_NAME,
};

const MIN_SESSION_VERSION: u32 = 1;
pub(super) const SESSION_VERSION: u32 = 4;
#[derive(Clone, Debug)]
pub(super) struct ResolvedSession {
    pub(super) id: String,
    pub(super) path: PathBuf,
    /// The workspace the session belongs to: the current directory for a local
    /// match, or the session's own stored workspace for a by-id match found
    /// elsewhere. Resume roots to this so it never runs one project's history
    /// against another's tree.
    pub(super) cwd: PathBuf,
}

#[derive(Clone, Debug)]
pub(super) struct SessionStore {
    root: PathBuf,
    cwd: PathBuf,
}

impl SessionStore {
    pub(super) fn new(root: &Path, cwd: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            cwd: cwd.to_path_buf(),
        }
    }

    fn ensure_dir(&self) -> anyhow::Result<PathBuf> {
        ensure_session_dir(&self.root, &self.cwd)
    }

    pub(super) fn resolve(&self, id_prefix: &str) -> anyhow::Result<ResolvedSession> {
        if let Some(local) = self.resolve_local(id_prefix)? {
            return Ok(local);
        }

        // A foreign transcript may exist before any process has indexed it, or
        // after a best-effort index write failed. Reconcile once before global
        // prefix resolution so cold caches do not make valid sessions invisible.
        index::reconcile_all_workspaces(&self.root)?;
        let global = index::matching_sessions_any_workspace(&self.root, id_prefix)?;
        let Some((path, cwd)) = single_match(&global, id_prefix)? else {
            anyhow::bail!("no session found matching '{id_prefix}'");
        };
        Ok(ResolvedSession {
            id: session_id(path)?,
            path: path.clone(),
            cwd: cwd.clone(),
        })
    }

    pub(super) fn resolve_in_workspace(&self, id_prefix: &str) -> anyhow::Result<ResolvedSession> {
        self.resolve_local(id_prefix)?.ok_or_else(|| {
            anyhow::anyhow!(
                "no session found matching '{id_prefix}' in {}",
                self.cwd.display()
            )
        })
    }

    fn resolve_local(&self, id_prefix: &str) -> anyhow::Result<Option<ResolvedSession>> {
        let dir = self.ensure_dir()?;
        let local = matching_session_files(&dir, id_prefix)?
            .into_iter()
            .filter_map(|path| {
                let cwd = read_session_cwd(&path).ok()?;
                (workspace_key(&cwd) == workspace_key(&self.cwd)).then_some((path, cwd))
            })
            .collect::<Vec<_>>();
        for (path, _) in &local {
            let _ = index::sync_session_file(&self.root, &self.cwd, path);
        }
        let Some((path, cwd)) = single_match(&local, id_prefix)? else {
            return Ok(None);
        };
        Ok(Some(ResolvedSession {
            id: session_id(path)?,
            path: path.clone(),
            cwd: cwd.clone(),
        }))
    }

    pub(super) fn create_path(&self, id: &str, created_at: u64) -> anyhow::Result<PathBuf> {
        // New sessions are folders so transcripts and sidecars (web blobs, etc.)
        // share one unit. Legacy flat `*.jsonl` files remain readable.
        let session_dir = self.ensure_dir()?.join(format!("{created_at}_{id}"));
        fs::create_dir_all(&session_dir)?;
        set_private_dir_permissions(&session_dir)?;
        Ok(session_dir.join(SESSION_TRANSCRIPT_FILE_NAME))
    }

    pub(super) fn list(&self) -> anyhow::Result<Vec<SessionSummary>> {
        self.ensure_dir()?;
        match index::list_workspace_sessions(&self.root, &self.cwd) {
            Ok(summaries) => Ok(summaries),
            Err(_) => self.list_by_scan(),
        }
    }

    pub(super) fn set_title(&self, id_prefix: &str, title: &str) -> anyhow::Result<()> {
        for path in matching_session_files(&self.ensure_dir()?, id_prefix)? {
            index::sync_session_file(&self.root, &self.cwd, &path)?;
        }
        index::set_title(&self.root, &self.cwd, id_prefix, title)
    }

    pub(super) fn title(&self, id: &str) -> anyhow::Result<Option<String>> {
        index::title(&self.root, &self.cwd, id)
    }

    pub(super) fn set_title_if_absent(&self, id: &str, title: &str) -> anyhow::Result<bool> {
        index::set_title_if_absent(&self.root, &self.cwd, id, title)
    }

    fn list_by_scan(&self) -> anyhow::Result<Vec<SessionSummary>> {
        list_session_summaries_by_scan(&self.ensure_dir()?, &self.cwd)
    }
}

impl ResolvedSession {
    pub(super) fn tree(&self) -> anyhow::Result<super::tree::SessionTree> {
        super::tree::SessionTree::load(&self.path)
    }
    pub(super) fn summary_with_tree(
        &self,
        cwd: &Path,
    ) -> anyhow::Result<(SessionIndexRecord, super::tree::SessionTree)> {
        summarize_session_file_with_tree(&self.path, cwd)
    }
}

/// Cached append position and parsed tree shared by all clones of one `Session`.
///
/// After a successful append the file is known to end with a complete,
/// newline-terminated record at `valid_len`, so the next append can skip
/// re-reading the file to find a recoverable end. Any mismatch with the
/// file's actual length (external writer, reopened session) or a failed
/// write clears `valid_len` and falls back to full validation.
///
/// `tree` is the last successfully parsed or mutated `SessionTree`, valid for
/// the recorded [`CacheStamp`]. Writers are serialized on this cursor, so the
/// cache is the sole in-process view of the file. External writes fail the
/// stamp check (length, mtime, or tail fingerprint) and force a reload.
#[derive(Debug, Default)]
pub(super) struct AppendCursor {
    pub(super) valid_len: Option<u64>,
    tree: Option<SessionTree>,
    cached_stamp: Option<CacheStamp>,
}

impl AppendCursor {
    /// Returns the cached tree when the file still matches the state it was
    /// parsed from; clears the cache otherwise.
    pub(super) fn take_tree(&mut self, path: &Path) -> Option<SessionTree> {
        let valid = self.cached_stamp == Some(CacheStamp::of(path));
        if !valid {
            self.invalidate_tree();
        }
        self.tree.take()
    }

    pub(super) fn store_tree(&mut self, tree: SessionTree, path: &Path) {
        // Re-stamp against current disk state so the next validation reflects
        // the append that just landed, not an older snapshot of the file.
        self.cached_stamp = Some(CacheStamp::of(path));
        self.tree = Some(tree);
    }

    /// Adopts a freshly parsed tree as the cache seed and marks the cursor's
    /// append position valid when the file ends in a complete record.
    pub(super) fn seed_loaded_tree(&mut self, tree: SessionTree, path: &Path) {
        let len = CacheStamp::of(path).len();
        self.store_tree(tree, path);
        if len == 0 || file_ends_with_newline(path) {
            self.valid_len = Some(len);
        }
    }

    pub(super) fn invalidate_tree(&mut self) {
        self.tree = None;
        self.cached_stamp = None;
    }
}

/// Identity of one on-disk transcript state: length, mtime, and a fingerprint
/// of the final bytes. Length alone misses same-size in-place edits; mtime is
/// unreliable on coarse-clock filesystems for rapid successive writes; the
/// tail fingerprint catches both deterministically. Corruption beyond the
/// tail window is caught on the next cold load, e.g. when the session is
/// reopened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CacheStamp {
    len: u64,
    mtime: Option<SystemTime>,
    tail_fingerprint: Option<u64>,
}

/// Bytes of the transcript tail covered by [`CacheStamp`].
const TAIL_FINGERPRINT_BYTES: u64 = 4096;

impl CacheStamp {
    fn of(path: &Path) -> Self {
        let (len, mtime) = match fs::metadata(path) {
            Ok(metadata) => (metadata.len(), metadata.modified().ok()),
            Err(_) => (0, None),
        };
        Self {
            len,
            mtime,
            tail_fingerprint: tail_fingerprint(path),
        }
    }

    fn len(&self) -> u64 {
        self.len
    }
}

/// FNV-1a over the last [`TAIL_FINGERPRINT_BYTES`] bytes of `path`.
fn tail_fingerprint(path: &Path) -> Option<u64> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let tail = TAIL_FINGERPRINT_BYTES.min(len);
    file.seek(SeekFrom::End(-(tail as i64))).ok()?;
    let mut buf = vec![0u8; tail as usize];
    file.read_exact(&mut buf).ok()?;
    Some(buf.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    }))
}

fn file_ends_with_newline(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    if file.seek(SeekFrom::End(-1)).is_err() {
        return false;
    }
    let mut last = [0u8; 1];
    matches!(file.read_exact(&mut last), Ok(())) && last[0] == b'\n'
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredDisplayMessage {
    pub(crate) timestamp: String,
    pub(crate) message: Message,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum SessionEntry {
    Session {
        version: u32,
        id: String,
        timestamp: String,
        cwd: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_fingerprint: Option<String>,
    },
    Message {
        timestamp: String,
        message: Message,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_message: Option<Box<Message>>,
    },
    ReplaceHistory {
        timestamp: String,
        messages: Vec<Message>,
    },
    Snapshot {
        timestamp: String,
        snapshot: Box<SessionSnapshot>,
        display_messages: Vec<StoredDisplayMessage>,
    },
    SnapshotDelta {
        timestamp: String,
        delta: Box<StoredSnapshotDelta>,
        display_messages: Vec<StoredDisplayMessage>,
    },
    Node {
        #[serde(flatten)]
        node: SessionNode,
    },
    SetLeaf {
        timestamp: String,
        target_id: NodeId,
    },
    Upgrade {
        timestamp: String,
        active_leaf_id: NodeId,
    },
}

impl SessionEntry {
    /// Event timestamp used for summary `updated_at` / `created_at` accumulation.
    pub(crate) fn event_timestamp(&self) -> &str {
        match self {
            SessionEntry::Session { timestamp, .. }
            | SessionEntry::Message { timestamp, .. }
            | SessionEntry::ReplaceHistory { timestamp, .. }
            | SessionEntry::Snapshot { timestamp, .. }
            | SessionEntry::SnapshotDelta { timestamp, .. }
            | SessionEntry::SetLeaf { timestamp, .. }
            | SessionEntry::Upgrade { timestamp, .. } => timestamp,
            SessionEntry::Node { node } => node.timestamp.as_str(),
        }
    }
}

impl Session {
    fn append_entry(&self, entry: &SessionEntry) -> anyhow::Result<()> {
        let mut cursor = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Metadata appends do not update the cached tree. Drop it so the next
        // reader reloads instead of parenting on a stale header-less view.
        cursor.invalidate_tree();
        self.append_entry_unlocked(&mut cursor, entry)
    }

    /// Appends a tree-mutating entry, emitting a legacy upgrade marker first when
    /// the tree still needs one. This is the only path for `Node` / `SetLeaf`.
    ///
    /// The in-memory tree is updated before the file write so a rejected node
    /// never remains in the transcript. When a marker is required, the marker
    /// and entry are written as one append so a failed entry write cannot leave
    /// a bare upgrade marker on disk.
    pub(super) fn append_tree_entry(
        &self,
        cursor: &mut AppendCursor,
        tree: &mut super::tree::SessionTree,
        entry: SessionEntry,
    ) -> anyhow::Result<()> {
        match &entry {
            SessionEntry::Node { .. } | SessionEntry::SetLeaf { .. } => {}
            SessionEntry::Session { .. }
            | SessionEntry::Message { .. }
            | SessionEntry::ReplaceHistory { .. }
            | SessionEntry::Snapshot { .. }
            | SessionEntry::SnapshotDelta { .. }
            | SessionEntry::Upgrade { .. } => {
                anyhow::bail!("append_tree_entry only accepts Node or SetLeaf entries");
            }
        }
        let upgrade = if tree.needs_upgrade_marker() {
            let active_leaf_id = tree.active_leaf_id().cloned().ok_or_else(|| {
                anyhow::anyhow!("legacy session has no state node to upgrade from")
            })?;
            let upgrade_ts = timestamp();
            tree.apply_upgrade(active_leaf_id.clone(), &upgrade_ts)?;
            Some(SessionEntry::Upgrade {
                timestamp: upgrade_ts,
                active_leaf_id,
            })
        } else {
            None
        };
        match &entry {
            SessionEntry::Node { node } => tree.apply_explicit_node(node.clone())?,
            SessionEntry::SetLeaf {
                timestamp,
                target_id,
            } => {
                tree.apply_set_leaf(target_id.clone(), timestamp)?;
                tree.ensure_active_state()?;
            }
            SessionEntry::Session { .. }
            | SessionEntry::Message { .. }
            | SessionEntry::ReplaceHistory { .. }
            | SessionEntry::Snapshot { .. }
            | SessionEntry::SnapshotDelta { .. }
            | SessionEntry::Upgrade { .. } => {
                anyhow::bail!("append_tree_entry only accepts Node or SetLeaf entries");
            }
        }
        if let Some(upgrade) = upgrade.as_ref() {
            self.write_jsonl_entries(cursor, &[upgrade, &entry])?;
        } else {
            self.write_jsonl_entry(cursor, &entry)?;
        }
        Ok(())
    }

    pub(super) fn append_entry_unlocked(
        &self,
        cursor: &mut AppendCursor,
        entry: &SessionEntry,
    ) -> anyhow::Result<()> {
        if matches!(
            entry,
            SessionEntry::Node { .. } | SessionEntry::SetLeaf { .. }
        ) {
            anyhow::bail!("tree-mutating entries must use append_tree_entry");
        }
        self.write_jsonl_entry(cursor, entry)
    }

    fn write_jsonl_entry(
        &self,
        cursor: &mut AppendCursor,
        entry: &SessionEntry,
    ) -> anyhow::Result<()> {
        self.write_jsonl_entries(cursor, &[entry])
    }

    fn write_jsonl_entries(
        &self,
        cursor: &mut AppendCursor,
        entries: &[&SessionEntry],
    ) -> anyhow::Result<()> {
        let mut serialized = Vec::new();
        for entry in entries {
            serde_json::to_writer(&mut serialized, entry)?;
            serialized.push(b'\n');
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);

        let original_len = fs::metadata(&self.path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        // A cached cursor that matches the file's actual length proves our
        // last append completed with a trailing newline, so the whole-file
        // recoverable-end scan can be skipped on this hot per-turn path.
        let (previous_len, needs_separator) = match cursor.valid_len {
            Some(valid_len) if valid_len == original_len => (valid_len, false),
            _ => {
                let (previous_len, needs_separator) = recoverable_jsonl_end(&self.path)?;
                if previous_len != original_len {
                    restore_file_len(&self.path, previous_len)?;
                }
                (previous_len, needs_separator)
            }
        };
        cursor.valid_len = None;
        let mut file = options.open(&self.path)?;
        set_private_file_permissions(&file)?;
        if needs_separator {
            serialized.insert(0, b'\n');
        }
        if let Err(write_error) = file.write_all(&serialized).and_then(|()| file.sync_data()) {
            drop(file);
            return match restore_file_len(&self.path, previous_len) {
                Ok(()) => Err(write_error.into()),
                Err(rollback_error) => Err(anyhow::anyhow!(
                    "session append failed: {write_error}; could not roll back file length: {rollback_error}"
                )),
            };
        }
        cursor.valid_len = Some(previous_len + serialized.len() as u64);
        Ok(())
    }

    pub(super) fn append_session_metadata(
        &self,
        id: String,
        created_at: u64,
        agent: Option<(&str, &str)>,
    ) -> anyhow::Result<()> {
        self.append_entry(&SessionEntry::Session {
            version: SESSION_VERSION,
            id,
            timestamp: created_at.to_string(),
            cwd: self.cwd.clone(),
            agent_id: agent.map(|(id, _)| id.to_string()),
            agent_fingerprint: agent.map(|(_, fingerprint)| fingerprint.to_string()),
        })?;
        let _ = index::record_created(self, created_at);
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn append_stored_message(
        &self,
        message: &Message,
        display_message: Option<&Message>,
    ) -> anyhow::Result<()> {
        let state = read_session_state(&self.path)?;
        let revision = next_revision(state.revision)?;
        let mut history = state.model;
        history.push(message.clone());
        let provider = state.snapshot.as_ref().map_or_else(
            || ModelIdentity::new("test", "test", "test"),
            |snapshot| snapshot.provider().clone(),
        );
        let snapshot = SessionSnapshot::new(
            SessionId::from_string(self.id.clone())?,
            revision,
            history,
            provider,
            state.compaction,
        );
        self.save_snapshot(&snapshot, &[display_message.unwrap_or(message).clone()])
    }

    #[cfg(test)]
    pub(super) fn append_replaced_history(&self, messages: &[Message]) -> anyhow::Result<()> {
        let mut state = read_session_state(&self.path)?;
        apply_legacy_history_replacement(&mut state, messages.to_vec())?;
        let provider = state.snapshot.as_ref().map_or_else(
            || ModelIdentity::new("test", "test", "test"),
            |snapshot| snapshot.provider().clone(),
        );
        let snapshot = SessionSnapshot::new(
            SessionId::from_string(self.id.clone())?,
            state.revision,
            state.model,
            provider,
            state.compaction,
        );
        self.save_snapshot(&snapshot, &[])
    }
}

pub(super) fn read_agent_identity(path: &Path) -> anyhow::Result<Option<(String, String)>> {
    let file = fs::File::open(path)?;
    let line = BufReader::new(file)
        .lines()
        .next()
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("session file is empty"))?;
    match serde_json::from_str::<SessionEntry>(&line)? {
        SessionEntry::Session {
            agent_id: Some(id),
            agent_fingerprint: Some(fingerprint),
            ..
        } => Ok(Some((id, fingerprint))),
        SessionEntry::Session { .. } => Ok(None),
        _ => anyhow::bail!("session file does not start with session metadata"),
    }
}

/// Workspace directory recorded on the session header line.
///
/// Reads only the first line, so callers can recover a workspace's cwd from
/// its one-way hashed directory name without parsing whole transcripts.
pub(super) fn read_session_cwd(path: &Path) -> anyhow::Result<PathBuf> {
    let file = fs::File::open(path)?;
    let line = BufReader::new(file)
        .lines()
        .next()
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("session file is empty"))?;
    match serde_json::from_str::<SessionEntry>(&line)? {
        SessionEntry::Session { cwd, .. } => Ok(cwd),
        _ => anyhow::bail!("session file does not start with session metadata"),
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PersistedSessionState {
    pub(crate) model: Vec<Message>,
    pub(crate) display: Vec<StoredDisplayMessage>,
    pub(crate) snapshot: Option<SessionSnapshot>,
    pub(crate) revision: Revision,
    pub(crate) compaction: CompactionState,
}

fn restore_file_len(path: &Path, len: u64) -> std::io::Result<()> {
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(len)?;
    file.sync_data()
}

fn recoverable_jsonl_end(path: &Path) -> anyhow::Result<(u64, bool)> {
    let Ok(contents) = fs::read(path) else {
        return Ok((0, false));
    };
    if contents.is_empty() || contents.ends_with(b"\n") {
        return Ok((contents.len() as u64, false));
    }
    let tail_start = contents
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    match serde_json::from_slice::<SessionEntry>(&contents[tail_start..]) {
        Ok(_) => Ok((contents.len() as u64, true)),
        Err(error) if error.is_eof() => Ok((tail_start as u64, false)),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn histories_from_tree(
    tree: &super::tree::SessionTree,
) -> anyhow::Result<SessionHistories> {
    let Some(active_leaf_id) = tree.active_leaf_id() else {
        return Ok(SessionHistories {
            model: Vec::new(),
            display: Vec::new(),
        });
    };
    let state = tree.active_state().expect("active leaf has restored state");
    Ok(SessionHistories {
        model: drop_incomplete_tool_turn_tail(state.model.clone()),
        display: drop_incomplete_tool_turn_tail(
            tree.projected_display(active_leaf_id)?
                .into_iter()
                .map(|entry| entry.message)
                .collect(),
        ),
    })
}

pub(super) fn read_session_state(path: &Path) -> anyhow::Result<PersistedSessionState> {
    Ok(super::tree::SessionTree::load(path)?
        .active_state()
        .cloned()
        .unwrap_or_default())
}

pub(super) fn apply_legacy_history_replacement(
    state: &mut PersistedSessionState,
    messages: Vec<Message>,
) -> anyhow::Result<()> {
    let previous_messages = state.model.len();
    let previous_tokens = rho_sdk::model::context::estimate_messages_tokens(&state.model);
    let current_tokens = rho_sdk::model::context::estimate_messages_tokens(&messages);
    state.model = messages;
    state.revision = next_revision(state.revision)?;
    state.compaction = CompactionState::from_accounting(
        state.compaction.completed_compactions().saturating_add(1),
        state
            .compaction
            .removed_messages()
            .saturating_add(previous_messages.saturating_sub(state.model.len()) as u64),
        state
            .compaction
            .removed_tokens()
            .saturating_add(previous_tokens.saturating_sub(current_tokens)),
        state.compaction.removed_cost_usd_micros(),
        Some(previous_tokens),
        Some(current_tokens),
        Some(state.revision),
    );
    Ok(())
}

pub(super) fn next_revision(revision: Revision) -> anyhow::Result<Revision> {
    revision
        .checked_next()
        .ok_or_else(|| anyhow::anyhow!("session revision is exhausted"))
}

#[cfg(test)]
fn visit_entries(
    path: &Path,
    mut visit: impl FnMut(SessionEntry) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        let terminated = line.ends_with('\n');
        match serde_json::from_str::<SessionEntry>(&line) {
            Ok(entry) => {
                if let SessionEntry::Session { version, .. } = &entry {
                    validate_session_version(*version, path)?;
                }
                visit(entry)?;
            }
            Err(err) if !terminated && err.is_eof() => return Ok(()),
            Err(err) => return Err(err.into()),
        }
    }
}

pub(super) fn validate_session_version(version: u32, path: &Path) -> anyhow::Result<()> {
    match version {
        MIN_SESSION_VERSION..=SESSION_VERSION => Ok(()),
        _ => {
            eprintln!(
                "warning: skipping session {} with unsupported version {version} (maximum supported: {SESSION_VERSION})",
                path.display()
            );
            anyhow::bail!("unsupported session version {version}")
        }
    }
}

#[cfg(test)]
pub(super) fn read_entries(path: &Path) -> anyhow::Result<Vec<SessionEntry>> {
    let mut entries = Vec::new();
    visit_entries(path, |entry| {
        entries.push(entry);
        Ok(())
    })?;
    Ok(entries)
}

pub(super) fn summarize_session_file(
    path: &Path,
    fallback_cwd: &Path,
) -> anyhow::Result<SessionIndexRecord> {
    summarize_session_file_with_tree(path, fallback_cwd).map(|(record, _)| record)
}

fn summarize_session_file_with_tree(
    path: &Path,
    fallback_cwd: &Path,
) -> anyhow::Result<(SessionIndexRecord, super::tree::SessionTree)> {
    let tree = super::tree::SessionTree::load(path)?;
    let record = tree.summary_record(path, fallback_cwd)?;
    Ok((record, tree))
}

pub(crate) fn complete_turn_tail_len<T, F>(items: &[T], get_message: F) -> usize
where
    F: Fn(&T) -> &Message,
{
    let mut index = 0usize;
    while index < items.len() {
        let message = get_message(&items[index]);
        let Some(blocks) = message.completed_assistant_content() else {
            index += 1;
            continue;
        };
        let tool_call_ids = blocks
            .iter()
            .filter_map(|block| match block {
                rho_providers::model::ContentBlock::ToolCall(call) => Some(call.id.as_str()),
                rho_providers::model::ContentBlock::Text(_)
                | rho_providers::model::ContentBlock::Image(_) => None,
            })
            .collect::<Vec<_>>();
        if tool_call_ids.is_empty() {
            index += 1;
            continue;
        }

        let results_start = index + 1;
        let results_end = results_start + tool_call_ids.len();
        if results_end > items.len() {
            return index;
        }

        let complete = tool_call_ids.iter().enumerate().all(|(offset, id)| {
            matches!(
                get_message(&items[results_start + offset]),
                Message::ToolResult(result) if result.id == *id
            )
        });
        if !complete {
            return index;
        }
        index = results_end;
    }
    items.len()
}

pub(crate) fn complete_message_len(messages: &[Message]) -> usize {
    complete_turn_tail_len(messages, |m| m)
}

pub(crate) fn drop_incomplete_tool_turn_tail(mut messages: Vec<Message>) -> Vec<Message> {
    let valid_len = complete_message_len(&messages);
    messages.truncate(valid_len);
    messages
}

/// History after the same resume normalization `snapshot_for_resume` applies.
///
/// TODO(rho-sdk-release): call SessionSnapshot::sanitize_history and delete this
/// local copy when `crates/rho/Cargo.toml` moves off `rho-sdk = "4.0.0"`.
/// Keep `resume_normalized_history_matches_sdk_sanitize_history` until then.
pub(crate) fn resume_normalized_history(history: Vec<Message>) -> Vec<Message> {
    let mut history = drop_incomplete_tool_turn_tail(history);
    for message in &mut history {
        if let Message::AbortedAssistant(assistant) = message {
            assistant.reasoning.clear();
        }
    }
    history
}

/// Collapses id-prefix matches to at most one, erroring on an ambiguous prefix.
/// Returns `None` when nothing matched.
fn single_match<'a, T>(matches: &'a [T], id_prefix: &str) -> anyhow::Result<Option<&'a T>> {
    match matches {
        [] => Ok(None),
        [only] => Ok(Some(only)),
        _ => anyhow::bail!("multiple sessions match '{id_prefix}'; use a longer UUID prefix"),
    }
}

fn session_id(path: &Path) -> anyhow::Result<String> {
    session_id_from_path(path)
        .ok_or_else(|| anyhow::anyhow!("session file has invalid name: {}", path.display()))
}

fn matching_session_files(dir: &Path, id_prefix: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let unit = SessionUnit::from_path(&entry.path())?;
            let id = unit.id()?;
            id.starts_with(id_prefix).then(|| unit.transcript_path())
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn list_session_summaries_by_scan(dir: &Path, cwd: &Path) -> anyhow::Result<Vec<SessionSummary>> {
    let mut summaries = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let unit = SessionUnit::from_path(&entry.path())?;
            summarize_session_file(&unit.transcript_path(), cwd)
                .ok()
                .filter(|record| record.summary.cwd == cwd)
        })
        .map(|record| record.summary)
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(summaries)
}

pub(super) fn session_file_stats(path: &Path) -> (Option<i64>, Option<i64>) {
    let stats_path = resolve_transcript_path(path).unwrap_or_else(|| path.to_path_buf());
    let Ok(metadata) = fs::metadata(stats_path) else {
        return (None, None);
    };
    let file_size = Some(clamp_u64_to_i64(metadata.len()));
    let file_mtime = metadata.modified().ok().map(system_time_secs);
    (file_size, file_mtime)
}

pub(super) fn user_message_text(message: &Message) -> Option<String> {
    let Message::User(blocks) = message else {
        return None;
    };
    let text = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.trim()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

pub(super) fn clamp_u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub(super) fn timestamp_from_filename(path: &Path) -> Option<u64> {
    SessionUnit::from_path(path)?.created_at_from_name()
}

fn system_time_secs(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| clamp_u64_to_i64(duration.as_secs()))
        .unwrap_or_default()
}
