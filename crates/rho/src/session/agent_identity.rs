//! Agent identity on the session header line: which agent definition a
//! session belongs to, and whether ACP rebound it as a delegated child.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};

use super::{index, layout::set_private_file_permissions, persistence::SessionEntry, Session};

/// How the session's agent was bound. Absent from the header means `Root`:
/// the process launch agent. `Delegated` marks an ACP session rebound with
/// `/agent:<name>`, whose reload must restore the child's tool boundary even
/// when the child shares the launch agent's id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionAgentRole {
    #[default]
    Root,
    Delegated,
}

impl SessionAgentRole {
    pub(super) fn is_root(&self) -> bool {
        matches!(self, Self::Root)
    }
}

fn read_header(path: &Path) -> anyhow::Result<SessionEntry> {
    let file = fs::File::open(path)?;
    let line = BufReader::new(file)
        .lines()
        .next()
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("session file is empty"))?;
    Ok(serde_json::from_str(&line)?)
}

pub(super) fn read_agent_identity(path: &Path) -> anyhow::Result<Option<(String, String)>> {
    match read_header(path)? {
        SessionEntry::Session {
            agent_id: Some(id),
            agent_fingerprint: Some(fingerprint),
            ..
        } => Ok(Some((id, fingerprint))),
        SessionEntry::Session { .. } => Ok(None),
        _ => anyhow::bail!("session file does not start with session metadata"),
    }
}

/// The agent id of a session rebound as a delegated child, or `None` for a
/// root session.
pub(super) fn read_delegated_agent_id(path: &Path) -> anyhow::Result<Option<String>> {
    match read_header(path)? {
        SessionEntry::Session {
            agent_role: SessionAgentRole::Root,
            ..
        } => Ok(None),
        SessionEntry::Session {
            agent_role: SessionAgentRole::Delegated,
            agent_id: Some(id),
            ..
        } => Ok(Some(id)),
        SessionEntry::Session {
            agent_role: SessionAgentRole::Delegated,
            agent_id: None,
            ..
        } => anyhow::bail!("delegated session header has no agent id"),
        _ => anyhow::bail!("session file does not start with session metadata"),
    }
}

impl Session {
    pub(super) fn rewrite_fresh_delegated_identity(
        &self,
        agent_id: &str,
        fingerprint: &str,
    ) -> anyhow::Result<()> {
        let mut cursor = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let contents = fs::read_to_string(&self.path)?;
        let mut lines = contents.lines();
        let header = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("session file is empty"))?;
        if lines.next().is_some() {
            anyhow::bail!("cannot rebind agent: the session already has history");
        }
        let mut entry: SessionEntry = serde_json::from_str(header)?;
        let SessionEntry::Session {
            agent_id: stored_id,
            agent_fingerprint,
            agent_role,
            ..
        } = &mut entry
        else {
            anyhow::bail!("session file does not start with session metadata");
        };
        *stored_id = Some(agent_id.to_owned());
        *agent_fingerprint = Some(fingerprint.to_owned());
        *agent_role = SessionAgentRole::Delegated;

        let parent = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("session file has no parent directory"))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        set_private_file_permissions(temporary.as_file())?;
        serde_json::to_writer(&mut temporary, &entry)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path)?;
        cursor.valid_len = None;
        cursor.invalidate_tree();
        // The index records transcript stats, not agent identity. Refresh them
        // best-effort, like every other persistence operation.
        let _ = index::sync_session_file(&self.session_root, &self.cwd, &self.path);
        Ok(())
    }
}

#[cfg(test)]
#[path = "agent_identity_tests.rs"]
mod tests;
