#[cfg(test)]
use std::{fs, fs::OpenOptions, io::Write};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[cfg(test)]
use uuid::Uuid;

use rho_providers::model::Message;
#[cfg(test)]
use rho_providers::model::{ContentBlock, ModelIdentity};
#[cfg(test)]
use rho_sdk::{CompactionState, Revision, SessionId, SessionSnapshot};

mod index;
mod persistence;
mod snapshot_delta;
mod snapshot_store;
#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "session_version_tests.rs"]
mod version_tests;

#[cfg(test)]
use persistence::{
    encode_cwd, read_entries, read_histories, session_dir_in_root, summarize_session_file,
    SessionEntry, SESSION_VERSION,
};
use persistence::{
    parse_timestamp, session_root, unix_timestamp_secs, workspace_key, AppendCursor, SessionStore,
};

#[derive(Clone, Debug)]
pub struct Session {
    id: String,
    path: PathBuf,
    session_root: PathBuf,
    cwd: PathBuf,
    workspace_key: String,
    write_lock: Arc<Mutex<AppendCursor>>,
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
        let resolved = SessionStore::new(session_root, cwd).resolve(id_prefix)?;
        let histories = resolved.histories()?;
        Ok((
            Self::from_parts(session_root, cwd, resolved.id, resolved.path),
            histories,
        ))
    }

    pub fn export_by_id(cwd: &Path, id_prefix: &str) -> anyhow::Result<SessionExport> {
        Self::export_by_id_in_root(&session_root()?, cwd, id_prefix)
    }

    pub(crate) fn export_by_id_in_root(
        session_root: &Path,
        cwd: &Path,
        id_prefix: &str,
    ) -> anyhow::Result<SessionExport> {
        let store = SessionStore::new(session_root, cwd);
        let resolved = store.resolve(id_prefix)?;
        let record = resolved.summary(cwd)?;
        let title = Self::list_in_root(session_root, cwd)
            .ok()
            .and_then(|summaries| {
                summaries
                    .into_iter()
                    .find(|summary| summary.id == resolved.id)
                    .and_then(|summary| summary.title)
            });

        let state = resolved.state()?;
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

    pub(crate) fn stored_agent_identity(&self) -> anyhow::Result<Option<(String, String)>> {
        persistence::read_agent_identity(&self.path)
    }

    pub(crate) fn validate_agent_identity(
        &self,
        selected_id: &str,
        selected_fingerprint: &str,
    ) -> anyhow::Result<()> {
        let Some((stored_id, stored_fingerprint)) = self.stored_agent_identity()? else {
            anyhow::bail!(
                "cannot resume this session as agent '{selected_id}': the session has no stored agent definition identity"
            );
        };
        if stored_id != selected_id {
            anyhow::bail!(
                "cannot resume session created by agent '{stored_id}' as selected agent '{selected_id}'"
            );
        }
        if stored_fingerprint != selected_fingerprint {
            anyhow::bail!(
                "cannot resume agent '{selected_id}': its definition changed since the session was created"
            );
        }
        Ok(())
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
        SessionStore::new(session_root, cwd).set_title(id_prefix, title)
    }

    fn list_in_root(session_root: &Path, cwd: &Path) -> anyhow::Result<Vec<SessionSummary>> {
        SessionStore::new(session_root, cwd).list()
    }

    pub(crate) fn create_with_id(
        cwd: &Path,
        id: &str,
        agent_id: &str,
        agent_fingerprint: &str,
    ) -> anyhow::Result<Self> {
        Self::create_with_id_in_root(
            &session_root()?,
            cwd,
            id,
            Some((agent_id, agent_fingerprint)),
        )
    }

    #[cfg(test)]
    pub(crate) fn create_in_root(session_root: &Path, cwd: &Path) -> anyhow::Result<Self> {
        Self::create_with_id_in_root(session_root, cwd, &Uuid::new_v4().to_string(), None)
    }

    #[cfg(test)]
    pub(crate) fn create_in_root_with_agent(
        session_root: &Path,
        cwd: &Path,
        agent_id: &str,
        agent_fingerprint: &str,
    ) -> anyhow::Result<Self> {
        Self::create_with_id_in_root(
            session_root,
            cwd,
            &Uuid::new_v4().to_string(),
            Some((agent_id, agent_fingerprint)),
        )
    }

    fn create_with_id_in_root(
        session_root: &Path,
        cwd: &Path,
        id: &str,
        agent: Option<(&str, &str)>,
    ) -> anyhow::Result<Self> {
        let store = SessionStore::new(session_root, cwd);
        let id = id.to_string();
        let created_at = unix_timestamp_secs();
        let path = store.create_path(&id, created_at)?;
        let session = Self::from_parts(session_root, cwd, id.clone(), path);
        session.append_session_metadata(id, created_at, agent)?;
        Ok(session)
    }

    #[cfg(test)]
    pub fn append_message(&self, message: &Message) -> anyhow::Result<()> {
        self.append_stored_message(message, None)
    }

    #[cfg(test)]
    pub fn append_message_with_display(
        &self,
        message: &Message,
        display_message: &Message,
    ) -> anyhow::Result<()> {
        self.append_stored_message(message, Some(display_message))
    }

    #[cfg(test)]
    pub fn replace_history(&self, messages: &[Message]) -> anyhow::Result<()> {
        self.append_replaced_history(messages)
    }

    fn from_parts(session_root: &Path, cwd: &Path, id: String, path: PathBuf) -> Self {
        Self {
            id,
            path,
            session_root: session_root.to_path_buf(),
            cwd: cwd.to_path_buf(),
            workspace_key: workspace_key(cwd),
            write_lock: Arc::new(Mutex::new(AppendCursor::default())),
        }
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub fn id(&self) -> &str {
        &self.id
    }
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
