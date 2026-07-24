use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
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

use super::snapshot_delta::{SnapshotDeltaBase, StoredSnapshotDelta};
use super::tree::{NodeId, SessionNode};
use super::{index, Session, SessionHistories, SessionIndexRecord, SessionSummary};

const MIN_SESSION_VERSION: u32 = 1;
pub(super) const SESSION_VERSION: u32 = 4;
/// Canonical transcript name inside a session folder.
pub(super) const SESSION_TRANSCRIPT_FILE_NAME: &str = "session.jsonl";
/// Sidecar directory name for web-access blobs inside a session folder.
pub(super) const SESSION_WEB_DIR_NAME: &str = "web";

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
        let dir = self.ensure_dir()?;
        let local = matching_session_files(&dir, id_prefix)?;
        for path in &local {
            let _ = index::sync_session_file(&self.root, &self.cwd, path);
        }
        if let Some(path) = single_match(&local, id_prefix)? {
            return Ok(ResolvedSession {
                id: session_id(path)?,
                path: path.clone(),
                cwd: self.cwd.clone(),
            });
        }

        // Recover by id across every workspace, keeping the session's own
        // workspace so resume does not silently continue in the current one.
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

    fn list_by_scan(&self) -> anyhow::Result<Vec<SessionSummary>> {
        list_session_summaries_by_scan(&self.ensure_dir()?, &self.cwd)
    }
}

impl ResolvedSession {
    pub(super) fn histories(&self) -> anyhow::Result<SessionHistories> {
        read_histories(&self.path)
    }
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

/// Cached append position shared by all clones of one `Session`.
///
/// After a successful append the file is known to end with a complete,
/// newline-terminated record at `valid_len`, so the next append can skip
/// re-reading the file to find a recoverable end. Any mismatch with the
/// file's actual length (external writer, reopened session) or a failed
/// write clears the cache and falls back to full validation.
#[derive(Debug, Default)]
pub(super) struct AppendCursor {
    pub(super) valid_len: Option<u64>,
    pub(super) last_snapshot: Option<SnapshotDeltaBase>,
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

impl Session {
    fn append_entry(&self, entry: &SessionEntry) -> anyhow::Result<()> {
        let mut cursor = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(
            entry,
            SessionEntry::Snapshot { .. } | SessionEntry::SnapshotDelta { .. }
        ) {
            cursor.last_snapshot = None;
        }
        self.append_entry_unlocked(&mut cursor, entry)
    }

    pub(super) fn append_entry_unlocked(
        &self,
        cursor: &mut AppendCursor,
        entry: &SessionEntry,
    ) -> anyhow::Result<()> {
        let mut serialized = serde_json::to_vec(entry)?;
        serialized.push(b'\n');
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

pub(super) fn read_histories(path: &Path) -> anyhow::Result<SessionHistories> {
    let tree = super::tree::SessionTree::load(path)?;
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
    let id = session_id_from_path(path)
        .ok_or_else(|| anyhow::anyhow!("session file has invalid name: {}", path.display()))?;
    let mut cwd = fallback_cwd.to_path_buf();
    let mut created_at = timestamp_from_filename(path).unwrap_or_default();
    let mut updated_at = created_at;
    let mut messages = Vec::new();
    let mut has_tree_records = false;

    visit_entries(path, |entry| {
        match entry {
            SessionEntry::Session {
                timestamp,
                cwd: session_cwd,
                ..
            } => {
                cwd = session_cwd;
                if let Some(timestamp) = parse_timestamp(&timestamp) {
                    created_at = timestamp;
                    updated_at = updated_at.max(timestamp);
                }
            }
            SessionEntry::Message {
                timestamp,
                message,
                display_message,
            } => {
                if let Some(timestamp) = parse_timestamp(&timestamp) {
                    updated_at = updated_at.max(timestamp);
                }
                messages.push(display_message.map_or(message, |message| *message));
            }
            SessionEntry::ReplaceHistory {
                timestamp,
                messages: replacement,
            } => {
                if let Some(timestamp) = parse_timestamp(&timestamp) {
                    updated_at = updated_at.max(timestamp);
                }
                messages = replacement;
            }
            SessionEntry::Snapshot {
                timestamp,
                display_messages,
                ..
            }
            | SessionEntry::SnapshotDelta {
                timestamp,
                display_messages,
                ..
            } => {
                if let Some(timestamp) = parse_timestamp(&timestamp) {
                    updated_at = updated_at.max(timestamp);
                }
                messages.extend(display_messages.into_iter().map(|entry| entry.message));
            }
            SessionEntry::Node { node } => {
                has_tree_records = true;
                if let Some(timestamp) = parse_timestamp(&node.timestamp) {
                    updated_at = updated_at.max(timestamp);
                }
                messages.extend(node.display_messages.into_iter().map(|entry| entry.message));
            }
            SessionEntry::SetLeaf { timestamp, .. } | SessionEntry::Upgrade { timestamp, .. } => {
                has_tree_records = true;
                if let Some(timestamp) = parse_timestamp(&timestamp) {
                    updated_at = updated_at.max(timestamp);
                }
            }
        }
        Ok(())
    })?;

    let tree = super::tree::SessionTree::load(path)?;
    if has_tree_records {
        messages = tree
            .active_state()
            .map(|state| {
                state
                    .display
                    .iter()
                    .map(|entry| entry.message.clone())
                    .collect()
            })
            .unwrap_or_default();
    }
    let messages = drop_incomplete_tool_turn_tail(messages);
    let (file_size, file_mtime) = session_file_stats(path);
    if updated_at == 0 {
        updated_at = file_mtime.map(|mtime| mtime as u64).unwrap_or_default();
    }
    if created_at == 0 {
        created_at = updated_at;
    }

    let facts = tree.facts();
    let record = SessionIndexRecord {
        summary: SessionSummary {
            id,
            path: path.to_path_buf(),
            cwd,
            created_at,
            updated_at,
            message_count: messages.len() as u64,
            title: None,
            first_user_message: messages.iter().find_map(user_message_text),
            last_user_message: messages.iter().rev().find_map(user_message_text),
        },
        file_size,
        file_mtime,
        node_count: facts.node_count as u64,
        branch_count: facts.branch_count as u64,
        active_leaf_id: facts.active_leaf_id.map(|id| id.to_string()),
        effective_format_version: tree.effective_format_version(),
    };
    Ok((record, tree))
}

fn drop_incomplete_tool_turn_tail(mut messages: Vec<Message>) -> Vec<Message> {
    let mut index = 0usize;
    while index < messages.len() {
        let Some(blocks) = messages[index].completed_assistant_content() else {
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
        if results_end > messages.len() {
            messages.truncate(index);
            return messages;
        }

        let complete = tool_call_ids.iter().enumerate().all(|(offset, id)| {
            matches!(
                &messages[results_start + offset],
                Message::ToolResult(result) if result.id == *id
            )
        });
        if !complete {
            messages.truncate(index);
            return messages;
        }
        index = results_end;
    }
    messages
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
            summarize_session_file(&unit.transcript_path(), cwd).ok()
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

pub(super) fn parse_timestamp(timestamp: &str) -> Option<u64> {
    timestamp.parse().ok()
}

pub(super) fn clamp_u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn timestamp_from_filename(path: &Path) -> Option<u64> {
    SessionUnit::from_path(path)?.created_at_from_name()
}

fn system_time_secs(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| clamp_u64_to_i64(duration.as_secs()))
        .unwrap_or_default()
}

/// One durable session unit on disk: folder layout or legacy flat transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SessionUnit {
    /// `<created-at>_<id>/session.jsonl` plus optional `web/` sidecar.
    Folder { dir: PathBuf },
    /// Legacy flat `<created-at>_<id>.jsonl` with optional `*.web/` companion.
    LegacyFile { path: PathBuf },
}

impl SessionUnit {
    /// Parses a workspace directory entry or an already-resolved transcript path.
    pub(super) fn from_path(path: &Path) -> Option<Self> {
        if path.is_dir() {
            let transcript = path.join(SESSION_TRANSCRIPT_FILE_NAME);
            return transcript.is_file().then(|| Self::Folder {
                dir: path.to_path_buf(),
            });
        }
        if is_folder_transcript(path) {
            return Some(Self::Folder {
                dir: path.parent()?.to_path_buf(),
            });
        }
        if is_legacy_transcript(path) {
            return Some(Self::LegacyFile {
                path: path.to_path_buf(),
            });
        }
        None
    }

    pub(super) fn transcript_path(&self) -> PathBuf {
        match self {
            Self::Folder { dir } => dir.join(SESSION_TRANSCRIPT_FILE_NAME),
            Self::LegacyFile { path } => path.clone(),
        }
    }

    pub(super) fn web_dir(&self) -> PathBuf {
        match self {
            Self::Folder { dir } => dir.join(SESSION_WEB_DIR_NAME),
            Self::LegacyFile { path } => {
                let stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("session");
                path.parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(format!("{stem}.web"))
            }
        }
    }

    pub(super) fn unit_name(&self) -> Option<&str> {
        match self {
            Self::Folder { dir } => dir.file_name()?.to_str(),
            Self::LegacyFile { path } => path.file_stem()?.to_str(),
        }
    }

    pub(super) fn id(&self) -> Option<String> {
        self.unit_name()?
            .rsplit_once('_')
            .map(|(_, id)| id.to_string())
    }

    pub(super) fn created_at_from_name(&self) -> Option<u64> {
        self.unit_name()?
            .split_once('_')
            .and_then(|(timestamp, _)| parse_timestamp(timestamp))
    }
}

/// Resolves a workspace entry or transcript path to the session JSONL file.
pub(super) fn resolve_transcript_path(path: &Path) -> Option<PathBuf> {
    SessionUnit::from_path(path).map(|unit| unit.transcript_path())
}

/// Web-access sidecar directory for a session transcript path.
pub(super) fn session_web_dir(transcript_path: &Path) -> Option<PathBuf> {
    SessionUnit::from_path(transcript_path).map(|unit| unit.web_dir())
}

pub(super) fn session_id_from_path(path: &Path) -> Option<String> {
    SessionUnit::from_path(path)?.id()
}

fn is_folder_transcript(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(SESSION_TRANSCRIPT_FILE_NAME)
}

fn is_legacy_transcript(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.contains('_'))
        && !is_folder_transcript(path)
}

pub(super) fn session_root() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join("sessions"))
}

pub(super) fn session_dir_in_root(session_root: &Path, cwd: &Path) -> PathBuf {
    session_root.join(workspace_key(cwd))
}

fn ensure_session_dir(session_root: &Path, cwd: &Path) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(session_root)?;
    set_private_dir_permissions(session_root)?;

    let dir = session_dir_in_root(session_root, cwd);
    fs::create_dir_all(&dir)?;
    set_private_dir_permissions(&dir)?;
    Ok(dir)
}

pub(super) fn set_private_dir_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn set_private_file_permissions(file: &fs::File) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
    Ok(())
}

pub(super) fn workspace_key(cwd: &Path) -> String {
    format!("{}-{:016x}", encode_cwd(cwd), stable_path_hash(cwd))
}

pub(super) fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

fn stable_path_hash(path: &Path) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    path.to_string_lossy()
        .as_bytes()
        .iter()
        .fold(FNV_OFFSET, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
        })
}

pub(super) fn timestamp() -> String {
    unix_timestamp_secs().to_string()
}

pub(super) fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
