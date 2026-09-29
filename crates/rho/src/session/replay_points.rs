//! Read-only access to a saved transcript's uncompacted model history, for the
//! offline compaction eval. Never takes a session lease or writes.

use std::path::Path;

use rho_providers::model::Message;

use super::{
    layout::{session_id_from_path, SessionUnit},
    persistence::insert_interrupted_tool_placeholders,
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
    let transcript = SessionUnit::from_path(path)
        .ok_or_else(|| anyhow::anyhow!("not a session transcript: {}", path.display()))?
        .transcript_path();
    let tree = SessionTree::load(&transcript)?;
    let id = session_id_from_path(&transcript)
        .ok_or_else(|| anyhow::anyhow!("session file has invalid name: {}", path.display()))?;
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
