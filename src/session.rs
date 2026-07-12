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

    fn open_by_id_with_histories_in_root(
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

    fn create_in_root(session_root: &Path, cwd: &Path) -> anyhow::Result<Self> {
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
        })?;
        let _ = index::record_message(self, message);
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
    let mut display = Vec::new();
    let mut model_display_start = 0;
    visit_entries(path, |entry| {
        match entry {
            SessionEntry::Session { .. } => {}
            SessionEntry::Message { message, .. } => display.push(message),
            SessionEntry::ReplaceHistory { messages, .. } => {
                replacement = messages;
                model_display_start = display.len();
            }
        }
        Ok(())
    })?;
    replacement.extend(display[model_display_start..].iter().cloned());
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
            SessionEntry::Message { timestamp, message } => {
                if let Some(timestamp) = parse_timestamp(&timestamp) {
                    updated_at = updated_at.max(timestamp);
                }
                messages.push(message);
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

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use tempfile::TempDir;

    use super::*;
    use crate::{
        model::{ContentBlock, ImageContent},
        tool::{ToolCall, ToolResult},
    };

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    struct TestDir(TempDir);

    impl Deref for TestDir {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            self.0.path()
        }
    }

    impl AsRef<Path> for TestDir {
        fn as_ref(&self) -> &Path {
            self.0.path()
        }
    }

    #[test]
    fn persists_and_loads_messages() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::User(vec![
                ContentBlock::Text("hello".into()),
                ContentBlock::Image(ImageContent {
                    data: "aW1n".into(),
                    mime_type: "image/png".into(),
                }),
            ]))
            .unwrap();
        session
            .append_message(&Message::assistant_text("hi"))
            .unwrap();

        let (_session, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();
        assert_eq!(messages.len(), 2);
        assert!(matches!(&messages[0], Message::User(blocks) if matches!(
            blocks.as_slice(),
            [ContentBlock::Text(text), ContentBlock::Image(image)]
                if text == "hello" && image.mime_type == "image/png" && image.data == "aW1n"
        )));
        assert!(matches!(&messages[1], Message::Assistant(_)));
    }

    #[test]
    fn replace_history_round_trips_compacted_messages() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session.append_message(&Message::user_text("old")).unwrap();
        session
            .replace_history(&[
                Message::user_text("summary"),
                Message::assistant_text("recent answer"),
            ])
            .unwrap();

        let (_session, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();

        assert_eq!(messages.len(), 2);
        assert!(
            matches!(&messages[0], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "summary"))
        );
        assert!(
            matches!(&messages[1], Message::Assistant(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "recent answer"))
        );
    }

    #[test]
    fn replace_history_is_append_only_but_model_replay_uses_latest_replacement() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::user_text("old user"))
            .unwrap();
        session
            .append_message(&Message::assistant_text("old assistant"))
            .unwrap();
        session
            .replace_history(&[
                Message::user_text("summary"),
                Message::assistant_text("recent answer"),
            ])
            .unwrap();
        session
            .append_message(&Message::user_text("after replacement"))
            .unwrap();

        let entries = read_entries(session.path()).unwrap();
        assert!(entries.iter().any(|entry| {
            matches!(entry, SessionEntry::Message { message, .. }
                if matches!(message, Message::User(blocks)
                    if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "old user")))
        }));
        assert!(entries
            .iter()
            .any(|entry| matches!(entry, SessionEntry::ReplaceHistory { .. })));

        let (_session, histories) =
            Session::open_by_id_with_histories_in_root(&root, &cwd, session.id()).unwrap();

        assert_eq!(histories.model.len(), 3);
        assert!(
            matches!(&histories.model[0], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "summary"))
        );
        assert!(
            matches!(&histories.model[2], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "after replacement"))
        );
        assert_eq!(histories.display.len(), 3);
        assert!(
            matches!(&histories.display[0], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "old user"))
        );
        assert!(
            matches!(&histories.display[1], Message::Assistant(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "old assistant"))
        );
        assert!(
            matches!(&histories.display[2], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "after replacement"))
        );
    }

    #[test]
    fn replace_history_updates_session_summary() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session.append_message(&Message::user_text("old")).unwrap();
        session
            .replace_history(&[Message::user_text("summary"), Message::user_text("latest")])
            .unwrap();

        let summaries = Session::list_in_root(&root, &cwd).unwrap();

        assert_eq!(summaries[0].message_count, 2);
        assert_eq!(summaries[0].first_user_message.as_deref(), Some("summary"));
        assert_eq!(summaries[0].last_user_message.as_deref(), Some("latest"));
    }

    #[test]
    fn opens_session_by_uuid_prefix() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::user_text("prefix match"))
            .unwrap();

        let prefix = &session.id()[..8];
        let (opened, messages) = Session::open_by_id_in_root(&root, &cwd, prefix).unwrap();

        assert_eq!(opened.id(), session.id());
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn errors_when_uuid_prefix_is_ambiguous() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        write_minimal_session_file(&root, &cwd, "aaaaaaaa-1111-4111-8111-111111111111");
        write_minimal_session_file(&root, &cwd, "aaaaaaaa-2222-4222-8222-222222222222");

        let err = Session::open_by_id_in_root(&root, &cwd, "aaaaaaaa").unwrap_err();

        assert!(err.to_string().contains("multiple sessions match"));
    }

    #[test]
    fn errors_when_uuid_prefix_is_missing() {
        let root = temp_session_root();
        let cwd = temp_cwd();

        let err = Session::open_by_id_in_root(&root, &cwd, "missing").unwrap_err();

        assert!(err.to_string().contains("no session found"));
    }

    #[test]
    fn stores_sessions_under_session_root_workspace_key() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        let expected_parent = root.join(workspace_key(&cwd));

        assert_eq!(session.path().parent(), Some(expected_parent.as_path()));
    }

    #[test]
    fn workspace_key_avoids_separator_collisions() {
        let slash_path = PathBuf::from("/tmp/rho-workspace/a/b");
        let dash_path = PathBuf::from("/tmp/rho-workspace/a-b");

        assert_eq!(encode_cwd(&slash_path), encode_cwd(&dash_path));
        assert_ne!(workspace_key(&slash_path), workspace_key(&dash_path));
    }

    #[test]
    fn drops_incomplete_tool_call_tail_on_load() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::user_text("run a tool"))
            .unwrap();
        session
            .append_message(&Message::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "echo hi"}),
                },
            )]))
            .unwrap();

        let (_session, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();

        assert_eq!(messages.len(), 1);
        assert!(matches!(&messages[0], Message::User(_)));
    }

    #[test]
    fn tolerates_only_truncated_final_json() {
        for (tail, should_load) in [
            (b"{\"type\":\"message\"".as_slice(), true),
            (b"{not json}\n".as_slice(), false),
            (b"{not json}".as_slice(), false),
        ] {
            let root = temp_session_root();
            let cwd = temp_cwd();
            let session = Session::create_in_root(&root, &cwd).unwrap();
            session
                .append_message(&Message::user_text("complete"))
                .unwrap();
            OpenOptions::new()
                .append(true)
                .open(session.path())
                .unwrap()
                .write_all(tail)
                .unwrap();

            assert_eq!(
                Session::open_by_id_in_root(&root, &cwd, session.id()).is_ok(),
                should_load
            );
        }
    }

    #[test]
    fn keeps_complete_tool_call_turn_on_load() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "echo hi"}),
                },
            )]))
            .unwrap();
        session
            .append_message(&Message::ToolResult(ToolResult {
                id: "call-1".into(),
                ok: true,
                content: "hi".into(),
            }))
            .unwrap();

        let (_session, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();

        assert_eq!(messages.len(), 2);
        assert!(matches!(&messages[0], Message::Assistant(_)));
        assert!(matches!(&messages[1], Message::ToolResult(_)));
    }

    #[test]
    fn list_backfills_existing_sessions_and_sorts_newest_first() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let older_id = "11111111-1111-4111-8111-111111111111";
        let newer_id = "22222222-2222-4222-8222-222222222222";
        write_session_file(&root, &cwd, older_id, 10, &["older prompt"]);
        write_session_file(&root, &cwd, newer_id, 20, &["newer prompt"]);

        let summaries = Session::list_in_root(&root, &cwd).unwrap();

        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].id, newer_id);
        assert_eq!(summaries[0].message_count, 1);
        assert_eq!(
            summaries[0].first_user_message.as_deref(),
            Some("newer prompt")
        );
        assert_eq!(
            summaries[0].last_user_message.as_deref(),
            Some("newer prompt")
        );
        assert_eq!(summaries[1].id, older_id);
        assert!(root.join("index.sqlite3").exists());
    }

    #[test]
    fn append_message_updates_session_summary() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::user_text("remember this"))
            .unwrap();
        session
            .append_message(&Message::assistant_text("remembered"))
            .unwrap();

        let summaries = Session::list_in_root(&root, &cwd).unwrap();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, session.id());
        assert_eq!(summaries[0].message_count, 2);
        assert_eq!(
            summaries[0].first_user_message.as_deref(),
            Some("remember this")
        );
        assert_eq!(
            summaries[0].last_user_message.as_deref(),
            Some("remember this")
        );
        assert!(summaries[0].updated_at >= summaries[0].created_at);
    }

    #[test]
    fn set_title_updates_session_summary() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::user_text("write tests"))
            .unwrap();

        Session::set_title_in_root(&root, &cwd, session.id(), "Testing plan").unwrap();
        let summaries = Session::list_in_root(&root, &cwd).unwrap();

        assert_eq!(summaries[0].title.as_deref(), Some("Testing plan"));
        assert_eq!(
            summaries[0].first_user_message.as_deref(),
            Some("write tests")
        );
    }

    #[test]
    fn list_removes_stale_index_rows() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        assert_eq!(Session::list_in_root(&root, &cwd).unwrap().len(), 1);
        fs::remove_file(session.path()).unwrap();

        let summaries = Session::list_in_root(&root, &cwd).unwrap();

        assert!(summaries.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn creates_session_paths_with_private_permissions() {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();

        let root_mode = fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        let dir_mode = fs::metadata(session.path().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(session.path()).unwrap().permissions().mode() & 0o777;

        assert_eq!(root_mode, 0o700);
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    fn temp_session_root() -> TestDir {
        TestDir(tempfile::tempdir().unwrap())
    }

    fn temp_cwd() -> TestDir {
        TestDir(tempfile::tempdir().unwrap())
    }

    fn write_minimal_session_file(root: &Path, cwd: &Path, id: &str) {
        write_session_file(root, cwd, id, 0, &[]);
    }

    fn write_session_file(root: &Path, cwd: &Path, id: &str, timestamp: u64, prompts: &[&str]) {
        let dir = session_dir_in_root(root, cwd);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{timestamp}_{id}.jsonl"));
        let mut entries = vec![SessionEntry::Session {
            version: SESSION_VERSION,
            id: id.into(),
            timestamp: timestamp.to_string(),
            cwd: cwd.to_path_buf(),
        }];
        entries.extend(prompts.iter().map(|prompt| SessionEntry::Message {
            timestamp: timestamp.to_string(),
            message: Message::user_text(*prompt),
        }));
        let contents = entries
            .into_iter()
            .map(|entry| serde_json::to_string(&entry).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(path, contents).unwrap();
    }
}
