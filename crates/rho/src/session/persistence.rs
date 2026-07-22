use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use serde::{Deserialize, Serialize};

use rho_providers::model::{ContentBlock, Message};
use rho_sdk::{CompactionState, Revision, SessionSnapshot};

use super::snapshot_delta::{SnapshotDeltaBase, StoredSnapshotDelta};
use super::{index, Session, SessionHistories, SessionIndexRecord, SessionSummary};

const MIN_SESSION_VERSION: u32 = 1;
pub(super) const SESSION_VERSION: u32 = 3;

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
        Ok(self.ensure_dir()?.join(format!("{created_at}_{id}.jsonl")))
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
    pub(super) fn state(&self) -> anyhow::Result<PersistedSessionState> {
        read_session_state(&self.path)
    }
    pub(super) fn summary(&self, cwd: &Path) -> anyhow::Result<SessionIndexRecord> {
        summarize_session_file(&self.path, cwd)
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
pub(super) struct StoredDisplayMessage {
    pub(super) timestamp: String,
    pub(super) message: Message,
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
        if let Err(error) = file.write_all(&serialized).and_then(|()| file.sync_data()) {
            drop(file);
            let _ = restore_file_len(&self.path, previous_len);
            return Err(error.into());
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
        self.append_entry(&SessionEntry::Message {
            timestamp: timestamp(),
            message: message.clone(),
            display_message: display_message.cloned().map(Box::new),
        })?;
        let _ = index::record_message(self, display_message.unwrap_or(message));
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn append_replaced_history(&self, messages: &[Message]) -> anyhow::Result<()> {
        self.append_entry(&SessionEntry::ReplaceHistory {
            timestamp: timestamp(),
            messages: messages.to_vec(),
        })?;
        let _ = index::record_replaced(self);
        Ok(())
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

#[derive(Default)]
pub(super) struct PersistedSessionState {
    pub(super) model: Vec<Message>,
    pub(super) display: Vec<StoredDisplayMessage>,
    pub(super) snapshot: Option<SessionSnapshot>,
    pub(super) revision: Revision,
    pub(super) compaction: CompactionState,
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
    let state = read_session_state(path)?;
    Ok(SessionHistories {
        model: drop_incomplete_tool_turn_tail(state.model),
        display: drop_incomplete_tool_turn_tail(
            state
                .display
                .into_iter()
                .map(|entry| entry.message)
                .collect(),
        ),
    })
}

pub(super) fn read_session_state(path: &Path) -> anyhow::Result<PersistedSessionState> {
    let mut state = PersistedSessionState::default();
    visit_entries(path, |entry| {
        match entry {
            SessionEntry::Session { .. } => {}
            SessionEntry::Message {
                timestamp,
                message,
                display_message,
            } => {
                state.display.push(StoredDisplayMessage {
                    timestamp,
                    message: display_message.map_or_else(|| message.clone(), |message| *message),
                });
                state.model.push(message);
                state.revision = next_revision(state.revision)?;
            }
            SessionEntry::ReplaceHistory { messages, .. } => {
                let previous_messages = state.model.len();
                let previous_tokens =
                    rho_sdk::model::context::estimate_messages_tokens(&state.model);
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
            }
            SessionEntry::Snapshot {
                snapshot,
                display_messages,
                ..
            } => {
                state.model = snapshot.history().to_vec();
                state.display.extend(display_messages);
                state.revision = snapshot.revision();
                state.compaction = snapshot.compaction().clone();
                state.snapshot = Some(*snapshot);
            }
            SessionEntry::SnapshotDelta {
                delta,
                display_messages,
                ..
            } => {
                let previous = state.snapshot.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("snapshot delta does not have a complete base snapshot")
                })?;
                let snapshot = delta.restore(previous)?;
                state.model = snapshot.history().to_vec();
                state.display.extend(display_messages);
                state.revision = snapshot.revision();
                state.compaction = snapshot.compaction().clone();
                state.snapshot = Some(snapshot);
            }
        }
        Ok(())
    })?;
    Ok(state)
}

fn next_revision(revision: Revision) -> anyhow::Result<Revision> {
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

fn validate_session_version(version: u32, path: &Path) -> anyhow::Result<()> {
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
    let id = session_id_from_path(path)
        .ok_or_else(|| anyhow::anyhow!("session file has invalid name: {}", path.display()))?;
    let mut cwd = fallback_cwd.to_path_buf();
    let mut created_at = timestamp_from_filename(path).unwrap_or_default();
    let mut updated_at = created_at;
    let mut messages = Vec::new();

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
        }
        Ok(())
    })?;

    let messages = drop_incomplete_tool_turn_tail(messages);
    let (file_size, file_mtime) = session_file_stats(path);
    if updated_at == 0 {
        updated_at = file_mtime.map(|mtime| mtime as u64).unwrap_or_default();
    }
    if created_at == 0 {
        created_at = updated_at;
    }

    Ok(SessionIndexRecord {
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
    })
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
            let path = entry.path();
            let id = session_id_from_path(&path)?;
            id.starts_with(id_prefix).then_some(path)
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn list_session_summaries_by_scan(dir: &Path, cwd: &Path) -> anyhow::Result<Vec<SessionSummary>> {
    let mut summaries = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| summarize_session_file(&entry.path(), cwd).ok())
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
    let Ok(metadata) = fs::metadata(path) else {
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
    path.file_stem()?
        .to_str()?
        .split_once('_')
        .and_then(|(timestamp, _)| parse_timestamp(timestamp))
}

fn system_time_secs(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| clamp_u64_to_i64(duration.as_secs()))
        .unwrap_or_default()
}

pub(super) fn session_id_from_path(path: &Path) -> Option<String> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return None;
    }
    path.file_stem()?
        .to_str()?
        .rsplit_once('_')
        .map(|(_, id)| id.to_string())
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
