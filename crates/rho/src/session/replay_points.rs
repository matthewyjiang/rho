//! Read-only access to a saved transcript's uncompacted model history, for the
//! offline compaction and classifier evals. Never takes a session lease or writes.

use std::path::{Path, PathBuf};

use rho_providers::model::Message;

use super::{
    layout::{session_id_from_path, SessionUnit},
    persistence::{insert_interrupted_tool_placeholders, read_session_cwd},
    tree::{SessionNodeKind, SessionTree},
};

/// Model history as it stood after one committed turn.
#[derive(Clone, Debug)]
pub(crate) struct ReplayPoint {
    pub(crate) node_id: String,
    pub(crate) messages: Vec<Message>,
}

/// A saved session's id and the model history after each committed turn on
/// its active path, oldest first. Stops at the first compaction: later
/// histories already hold a summary, and replaying on top of one would score
/// the earlier compaction as well as the one under test.
pub(crate) fn load(path: &Path) -> anyhow::Result<(String, Vec<ReplayPoint>)> {
    let (id, tree) = open(path)?;
    let mut points = Vec::new();
    for node in tree.active_path()? {
        match node.kind() {
            SessionNodeKind::Compaction => break,
            SessionNodeKind::Commit => points.push(ReplayPoint {
                node_id: node.id().to_string(),
                messages: insert_interrupted_tool_placeholders(tree.state_for(node.id())?.model),
            }),
        }
    }
    Ok((id, points))
}

/// One stretch of the active path between compactions.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistorySegment {
    /// Model history at the stretch's last committed turn.
    pub(crate) messages: Vec<Message>,
    /// Index of the first message added after the compaction that opened the
    /// stretch; earlier messages are the compacted history. 0 for the first.
    pub(crate) new_from: usize,
}

/// A saved session's id and every stretch of its active path between
/// compactions, oldest first. Within a stretch, history only grows, so the
/// last turn's history holds every earlier turn's history as a prefix.
pub(crate) fn segments(path: &Path) -> anyhow::Result<(String, Vec<HistorySegment>)> {
    let (id, tree) = open(path)?;
    let model = |node_id| {
        anyhow::Ok(insert_interrupted_tool_placeholders(
            tree.state_for(node_id)?.model,
        ))
    };
    let mut segments = Vec::new();
    let mut new_from = 0;
    let mut last_commit = None;
    for node in tree.active_path()? {
        match node.kind() {
            SessionNodeKind::Compaction => {
                if let Some(commit) = last_commit.take() {
                    segments.push(HistorySegment {
                        messages: model(commit)?,
                        new_from,
                    });
                }
                new_from = model(node.id())?.len();
            }
            SessionNodeKind::Commit => last_commit = Some(node.id()),
        }
    }
    if let Some(commit) = last_commit {
        segments.push(HistorySegment {
            messages: model(commit)?,
            new_from,
        });
    }
    Ok((id, segments))
}

fn open(path: &Path) -> anyhow::Result<(String, SessionTree)> {
    let transcript = SessionUnit::from_path(path)
        .ok_or_else(|| anyhow::anyhow!("not a session transcript: {}", path.display()))?
        .transcript_path();
    let tree = SessionTree::load(&transcript)?;
    let id = session_id_from_path(&transcript)
        .ok_or_else(|| anyhow::anyhow!("session file has invalid name: {}", path.display()))?;
    Ok((id, tree))
}

/// Workspace directory a saved session ran in.
pub(crate) fn session_cwd(path: &Path) -> anyhow::Result<PathBuf> {
    let transcript = SessionUnit::from_path(path)
        .ok_or_else(|| anyhow::anyhow!("not a session transcript: {}", path.display()))?
        .transcript_path();
    read_session_cwd(&transcript)
}

#[cfg(test)]
#[path = "replay_points_tests.rs"]
mod tests;
