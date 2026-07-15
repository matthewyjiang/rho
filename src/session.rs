use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use serde::{Deserialize, Serialize};
#[cfg(test)]
use uuid::Uuid;

use crate::model::{ContentBlock, Message, ModelIdentity};
use rho_sdk::{CompactionState, Revision, SessionId, SessionSnapshot};

mod index;
#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "session_version_tests.rs"]
mod version_tests;

const MIN_SESSION_VERSION: u32 = 1;
const SESSION_VERSION: u32 = 2;

#[derive(Clone, Debug)]
pub struct Session {
    id: String,
    path: PathBuf,
    session_root: PathBuf,
    cwd: PathBuf,
    workspace_key: String,
    write_lock: Arc<Mutex<()>>,
}

#[derive(Clone, Debug)]
pub struct SessionHistories {
    pub model: Vec<Message>,
    pub display: Vec<Message>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: String,
    pub path: PathBuf,
    pub cwd: PathBuf,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: u64,
    pub title: Option<String>,
    pub first_user_message: Option<String>,
    pub last_user_message: Option<String>,
}

/// A display-history message paired with the unix timestamp it was recorded at.
#[derive(Clone, Debug)]
pub struct ExportedMessage {
    pub timestamp: Option<u64>,
    pub message: Message,
}

/// Everything needed to render a session transcript outside the TUI.
#[derive(Clone, Debug)]
pub struct SessionExport {
    pub id: String,
    pub cwd: PathBuf,
    pub created_at: u64,
    pub updated_at: u64,
    pub title: Option<String>,
    pub messages: Vec<ExportedMessage>,
}

#[derive(Clone, Debug)]
pub(super) struct SessionIndexRecord {
    pub(super) summary: SessionSummary,
    pub(super) file_size: Option<i64>,
    pub(super) file_mtime: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredDisplayMessage {
    timestamp: String,
    message: Message,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SessionEntry {
    Session {
        version: u32,
        id: String,
        timestamp: String,
        cwd: PathBuf,
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
        snapshot: SessionSnapshot,
        display_messages: Vec<StoredDisplayMessage>,
    },
}

impl Session {
    pub fn open_by_id_with_histories(
        cwd: &Path,
        id_prefix: &str,
    ) -> anyhow::Result<(Self, SessionHistories)> {
        Self::open_by_id_with_histories_in_root(&session_root()?, cwd, id_prefix)
    }

    #[cfg(test)]
    fn open_by_id_in_root(
        session_root: &Path,
        cwd: &Path,
        id_prefix: &str,
    ) -> anyhow::Result<(Self, Vec<Message>)> {
        let (session, histories) =
            Self::open_by_id_with_histories_in_root(session_root, cwd, id_prefix)?;
        Ok((session, histories.model))
    }

    pub(crate) fn open_by_id_with_histories_in_root(
        session_root: &Path,
        cwd: &Path,
        id_prefix: &str,
    ) -> anyhow::Result<(Self, SessionHistories)> {
        let dir = ensure_session_dir(session_root, cwd)?;
        let matches = matching_session_files(&dir, id_prefix)?;
        for path in &matches {
            let _ = index::sync_session_file(session_root, cwd, path);
        }
        match matches.as_slice() {
            [] => anyhow::bail!("no session found matching '{id_prefix}'"),
            [path] => {
                let id = session_id_from_path(path).ok_or_else(|| {
                    anyhow::anyhow!("session file has invalid name: {}", path.display())
                })?;
                let histories = read_histories(path)?;
                Ok((
                    Self::from_parts(session_root, cwd, id, path.clone()),
                    histories,
                ))
            }
            _ => anyhow::bail!("multiple sessions match '{id_prefix}'; use a longer UUID prefix"),
        }
    }

    pub fn export_by_id(cwd: &Path, id_prefix: &str) -> anyhow::Result<SessionExport> {
        Self::export_by_id_in_root(&session_root()?, cwd, id_prefix)
    }

    pub(crate) fn export_by_id_in_root(
        session_root: &Path,
        cwd: &Path,
        id_prefix: &str,
    ) -> anyhow::Result<SessionExport> {
        let dir = ensure_session_dir(session_root, cwd)?;
        let matches = matching_session_files(&dir, id_prefix)?;
        let path = match matches.as_slice() {
            [] => anyhow::bail!("no session found matching '{id_prefix}'"),
            [path] => path,
            _ => anyhow::bail!("multiple sessions match '{id_prefix}'; use a longer UUID prefix"),
        };
        let record = summarize_session_file(path, cwd)?;
        let title = Self::list_in_root(session_root, cwd)
            .ok()
            .and_then(|summaries| {
                summaries
                    .into_iter()
                    .find(|summary| summary.id == record.summary.id)
                    .and_then(|summary| summary.title)
            });

        let state = read_session_state(path)?;
        let mut messages = state.display;
        let complete_len = drop_incomplete_tool_turn_tail(
            messages.iter().map(|entry| entry.message.clone()).collect(),
        )
        .len();
        messages.truncate(complete_len);
        Ok(SessionExport {
            id: record.summary.id,
            cwd: record.summary.cwd,
            created_at: record.summary.created_at,
            updated_at: record.summary.updated_at,
            title,
            messages: messages
                .into_iter()
                .map(|message| ExportedMessage {
                    timestamp: parse_timestamp(&message.timestamp),
                    message: message.message,
                })
                .collect(),
        })
    }

    pub fn list(cwd: &Path) -> anyhow::Result<Vec<SessionSummary>> {
        Self::list_in_root(&session_root()?, cwd)
    }

    pub fn set_title(cwd: &Path, id_prefix: &str, title: &str) -> anyhow::Result<()> {
        Self::set_title_in_root(&session_root()?, cwd, id_prefix, title)
    }

    fn set_title_in_root(
        session_root: &Path,
        cwd: &Path,
        id_prefix: &str,
        title: &str,
    ) -> anyhow::Result<()> {
        let dir = ensure_session_dir(session_root, cwd)?;
        for path in matching_session_files(&dir, id_prefix)? {
            index::sync_session_file(session_root, cwd, &path)?;
        }
        index::set_title(session_root, cwd, id_prefix, title)
    }

    fn list_in_root(session_root: &Path, cwd: &Path) -> anyhow::Result<Vec<SessionSummary>> {
        let dir = ensure_session_dir(session_root, cwd)?;
        match index::list_workspace_sessions(session_root, cwd) {
            Ok(summaries) => Ok(summaries),
            Err(_) => list_session_summaries_by_scan(&dir, cwd),
        }
    }

    pub(crate) fn create_with_id(cwd: &Path, id: &str) -> anyhow::Result<Self> {
        Self::create_with_id_in_root(&session_root()?, cwd, id)
    }

    #[cfg(test)]
    pub(crate) fn create_in_root(session_root: &Path, cwd: &Path) -> anyhow::Result<Self> {
        Self::create_with_id_in_root(session_root, cwd, &Uuid::new_v4().to_string())
    }

    fn create_with_id_in_root(session_root: &Path, cwd: &Path, id: &str) -> anyhow::Result<Self> {
        let dir = ensure_session_dir(session_root, cwd)?;
        let id = id.to_string();
        let created_at = unix_timestamp_secs();
        let path = dir.join(format!("{created_at}_{id}.jsonl"));
        let session = Self::from_parts(session_root, cwd, id.clone(), path);
        session.append_entry(&SessionEntry::Session {
            version: SESSION_VERSION,
            id,
            timestamp: created_at.to_string(),
            cwd: cwd.to_path_buf(),
        })?;
        let _ = index::record_created(&session, created_at);
        Ok(session)
    }

    #[cfg(test)]
    pub fn append_message(&self, message: &Message) -> anyhow::Result<()> {
        self.append_entry(&SessionEntry::Message {
            timestamp: timestamp(),
            message: message.clone(),
            display_message: None,
        })?;
        let _ = index::record_message(self, message);
        Ok(())
    }

    #[cfg(test)]
    pub fn append_message_with_display(
        &self,
        message: &Message,
        display_message: &Message,
    ) -> anyhow::Result<()> {
        self.append_entry(&SessionEntry::Message {
            timestamp: timestamp(),
            message: message.clone(),
            display_message: Some(Box::new(display_message.clone())),
        })?;
        let _ = index::record_message(self, display_message);
        Ok(())
    }

    #[cfg(test)]
    pub fn replace_history(&self, messages: &[Message]) -> anyhow::Result<()> {
        self.append_entry(&SessionEntry::ReplaceHistory {
            timestamp: timestamp(),
            messages: messages.to_vec(),
        })?;
        let _ = index::record_replaced(self);
        Ok(())
    }

    /// Saves one complete SDK snapshot and its newly visible transcript tail.
    ///
    /// The snapshot and display update share one JSONL record. Readers ignore a
    /// truncated final record, so a failed or interrupted append retains the
    /// previous complete snapshot and transcript.
    pub(crate) fn save_snapshot(
        &self,
        snapshot: &SessionSnapshot,
        display_tail: &[Message],
    ) -> anyhow::Result<()> {
        if snapshot.session_id().as_str() != self.id {
            anyhow::bail!(
                "snapshot session id '{}' does not match store id '{}'",
                snapshot.session_id(),
                self.id
            );
        }
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let display_messages = display_tail
            .iter()
            .cloned()
            .map(|message| StoredDisplayMessage {
                timestamp: timestamp(),
                message,
            })
            .collect();
        self.append_entry_unlocked(&SessionEntry::Snapshot {
            timestamp: timestamp(),
            snapshot: snapshot.clone(),
            display_messages,
        })?;
        let _ = index::record_snapshot(self);
        Ok(())
    }

    pub(crate) fn snapshot_for_resume(
        &self,
        provider: ModelIdentity,
        prompt_cache_key: String,
    ) -> anyhow::Result<SessionSnapshot> {
        let state = read_session_state(&self.path)?;
        let history = drop_incomplete_tool_turn_tail(state.model);
        let mut snapshot = if let Some(snapshot) = state.snapshot {
            if snapshot.session_id().as_str() != self.id {
                anyhow::bail!(
                    "stored snapshot session id '{}' does not match file id '{}'",
                    snapshot.session_id(),
                    self.id
                );
            }
            let mut migrated = SessionSnapshot::new(
                snapshot.session_id().clone(),
                state.revision,
                history,
                snapshot.provider().clone(),
                state.compaction,
            );
            for (key, value) in snapshot.metadata() {
                migrated = migrated.with_metadata(key.clone(), value.clone());
            }
            if let Some(key) = snapshot.prompt_cache_key() {
                migrated.with_prompt_cache_key(key)
            } else {
                migrated.with_prompt_cache_key(prompt_cache_key)
            }
        } else {
            SessionSnapshot::new(
                SessionId::from_string(self.id.clone())?,
                state.revision,
                history,
                provider,
                state.compaction,
            )
            .with_prompt_cache_key(prompt_cache_key)
        };
        if snapshot.schema_version() != rho_sdk::SESSION_SNAPSHOT_SCHEMA_VERSION {
            snapshot = SessionSnapshot::from_json(&snapshot.to_json()?)?;
        }
        Ok(snapshot)
    }

    fn from_parts(session_root: &Path, cwd: &Path, id: String, path: PathBuf) -> Self {
        Self {
            id,
            path,
            session_root: session_root.to_path_buf(),
            cwd: cwd.to_path_buf(),
            workspace_key: workspace_key(cwd),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn append_entry(&self, entry: &SessionEntry) -> anyhow::Result<()> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.append_entry_unlocked(entry)
    }

    fn append_entry_unlocked(&self, entry: &SessionEntry) -> anyhow::Result<()> {
        let mut serialized = serde_json::to_vec(entry)?;
        serialized.push(b'\n');
        let mut options = OpenOptions::new();
        options.create(true).read(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);

        let original_len = fs::metadata(&self.path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let (previous_len, needs_separator) = recoverable_jsonl_end(&self.path)?;
        if previous_len != original_len {
            restore_file_len(&self.path, previous_len)?;
        }
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
        Ok(())
    }
}

#[derive(Default)]
struct PersistedSessionState {
    model: Vec<Message>,
    display: Vec<StoredDisplayMessage>,
    snapshot: Option<SessionSnapshot>,
    revision: Revision,
    compaction: CompactionState,
}

impl rho_sdk::SessionStore for Session {
    fn load<'a>(
        &'a self,
        id: &'a SessionId,
    ) -> rho_sdk::SessionStoreFuture<'a, Option<SessionSnapshot>> {
        Box::pin(async move {
            if id.as_str() != self.id {
                return Ok(None);
            }
            let state = read_session_state(&self.path).map_err(persistence_error)?;
            let Some(stored) = state.snapshot else {
                return Ok(None);
            };
            let cache_key = stored
                .prompt_cache_key()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("rho:{}", self.id));
            self.snapshot_for_resume(stored.provider().clone(), cache_key)
                .map(Some)
                .map_err(persistence_error)
        })
    }

    fn save<'a>(&'a self, snapshot: SessionSnapshot) -> rho_sdk::SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            self.save_snapshot(&snapshot, &[])
                .map_err(persistence_error)
        })
    }
}

fn persistence_error(error: impl std::fmt::Display) -> rho_sdk::Error {
    rho_sdk::Error::Persistence {
        message: error.to_string(),
    }
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

fn read_histories(path: &Path) -> anyhow::Result<SessionHistories> {
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

fn read_session_state(path: &Path) -> anyhow::Result<PersistedSessionState> {
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
                state.model = messages;
                state.revision = next_revision(state.revision)?;
                state.compaction = CompactionState::from_parts(
                    state.compaction.completed_compactions().saturating_add(1),
                    state
                        .compaction
                        .removed_messages()
                        .saturating_add(previous_messages.saturating_sub(state.model.len()) as u64),
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
fn read_entries(path: &Path) -> anyhow::Result<Vec<SessionEntry>> {
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
                crate::model::ContentBlock::ToolCall(call) => Some(call.id.as_str()),
                crate::model::ContentBlock::Text(_) | crate::model::ContentBlock::Image(_) => None,
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

fn session_id_from_path(path: &Path) -> Option<String> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return None;
    }
    path.file_stem()?
        .to_str()?
        .rsplit_once('_')
        .map(|(_, id)| id.to_string())
}

fn session_root() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join("sessions"))
}

fn session_dir_in_root(session_root: &Path, cwd: &Path) -> PathBuf {
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

fn set_private_dir_permissions(path: &Path) -> anyhow::Result<()> {
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

fn workspace_key(cwd: &Path) -> String {
    format!("{}-{:016x}", encode_cwd(cwd), stable_path_hash(cwd))
}

fn encode_cwd(cwd: &Path) -> String {
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

fn timestamp() -> String {
    unix_timestamp_secs().to_string()
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
