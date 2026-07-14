use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{ContentBlock, Message};

mod index;
#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "session_version_tests.rs"]
mod version_tests;

const SESSION_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct Session {
    id: String,
    path: PathBuf,
    session_root: PathBuf,
    cwd: PathBuf,
    workspace_key: String,
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

        let mut messages = Vec::new();
        visit_entries(path, |entry| {
            if let SessionEntry::Message {
                timestamp,
                message,
                display_message,
            } = entry
            {
                messages.push(ExportedMessage {
                    timestamp: parse_timestamp(&timestamp),
                    message: display_message.map_or(message, |message| *message),
                });
            }
            Ok(())
        })?;
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
            messages,
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

    pub fn create(cwd: &Path) -> anyhow::Result<Self> {
        Self::create_in_root(&session_root()?, cwd)
    }

    pub(crate) fn create_in_root(session_root: &Path, cwd: &Path) -> anyhow::Result<Self> {
        let dir = ensure_session_dir(session_root, cwd)?;
        let id = Uuid::new_v4().to_string();
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

    pub fn append_message(&self, message: &Message) -> anyhow::Result<()> {
        self.append_entry(&SessionEntry::Message {
            timestamp: timestamp(),
            message: message.clone(),
            display_message: None,
        })?;
        let _ = index::record_message(self, message);
        Ok(())
    }

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

    pub fn replace_history(&self, messages: &[Message]) -> anyhow::Result<()> {
        self.append_entry(&SessionEntry::ReplaceHistory {
            timestamp: timestamp(),
            messages: messages.to_vec(),
        })?;
        let _ = index::record_replaced(self);
        Ok(())
    }

    fn from_parts(session_root: &Path, cwd: &Path, id: String, path: PathBuf) -> Self {
        Self {
            id,
            path,
            session_root: session_root.to_path_buf(),
            cwd: cwd.to_path_buf(),
            workspace_key: workspace_key(cwd),
        }
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.path
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn append_entry(&self, entry: &SessionEntry) -> anyhow::Result<()> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);

        let mut file = options.open(&self.path)?;
        set_private_file_permissions(&file)?;
        serde_json::to_writer(&mut file, entry)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(())
    }
}

fn read_histories(path: &Path) -> anyhow::Result<SessionHistories> {
    let mut replacement = Vec::new();
    let mut model_tail = Vec::new();
    let mut display = Vec::new();
    visit_entries(path, |entry| {
        match entry {
            SessionEntry::Session { .. } => {}
            SessionEntry::Message {
                message,
                display_message,
                ..
            } => {
                display.push(display_message.map_or_else(|| message.clone(), |message| *message));
                model_tail.push(message);
            }
            SessionEntry::ReplaceHistory { messages, .. } => {
                replacement = messages;
                model_tail.clear();
            }
        }
        Ok(())
    })?;
    replacement.extend(model_tail);
    Ok(SessionHistories {
        model: drop_incomplete_tool_turn_tail(replacement),
        display: drop_incomplete_tool_turn_tail(display),
    })
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
        0..=SESSION_VERSION => Ok(()),
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
        let Message::Assistant(blocks) = &messages[index] else {
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
