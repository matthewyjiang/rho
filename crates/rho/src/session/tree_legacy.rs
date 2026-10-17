//! Projects version 1-3 legacy log records (`message`, `replace_history`,
//! `snapshot`, `snapshot_delta`) onto virtual tree nodes. These files are read
//! as-is; the first tree change appends an upgrade marker and explicit nodes.

use rho_sdk::SessionSnapshot;

use super::{
    next_revision, LoadedNode, NodeId, PersistedSessionState, SessionEntry, SessionNodeKind,
    StoredDisplayMessage, StoredStateTransition,
};

impl super::SessionTree {
    pub(super) fn apply_legacy_entry(
        &mut self,
        entry: SessionEntry,
        offset: u64,
        state: &mut PersistedSessionState,
    ) -> anyhow::Result<()> {
        let previous = state.clone();
        let (timestamp, display_messages, suggested_kind, changed) = match entry {
            SessionEntry::Message {
                timestamp,
                message,
                display_message,
            } => {
                let display = StoredDisplayMessage {
                    timestamp: timestamp.clone(),
                    message: display_message.map_or_else(|| message.clone(), |message| *message),
                };
                state.model.push(message);
                state.display.push(display.clone());
                state.revision = next_revision(state.revision)?;
                refresh_snapshot_state(state);
                (timestamp, vec![display], SessionNodeKind::Commit, true)
            }
            SessionEntry::ReplaceHistory {
                timestamp,
                messages,
            } => {
                super::super::persistence::apply_legacy_history_replacement(state, messages)?;
                refresh_snapshot_state(state);
                (timestamp, Vec::new(), SessionNodeKind::Compaction, true)
            }
            SessionEntry::Snapshot {
                timestamp,
                snapshot,
                display_messages,
            } => {
                let replaced_history = !state.model.is_empty()
                    && !snapshot.history().starts_with(state.model.as_slice());
                let compaction = snapshot.compaction() != &state.compaction || replaced_history;
                let changed = state.snapshot.is_none()
                    || snapshot.revision() != state.revision
                    || snapshot.history() != state.model
                    || snapshot.compaction() != &state.compaction;
                state.model = snapshot.history().to_vec();
                state.display.extend(display_messages.clone());
                state.revision = snapshot.revision();
                state.compaction = snapshot.compaction().clone();
                state.snapshot = Some(*snapshot);
                (
                    timestamp,
                    display_messages,
                    if compaction {
                        SessionNodeKind::Compaction
                    } else {
                        SessionNodeKind::Commit
                    },
                    changed,
                )
            }
            SessionEntry::SnapshotDelta {
                timestamp,
                delta,
                display_messages,
            } => {
                let base = state.snapshot.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("snapshot delta does not have a complete base snapshot")
                })?;
                let snapshot = delta.restore(base)?;
                let changed = snapshot.revision() != state.revision
                    || snapshot.history() != state.model
                    || snapshot.compaction() != &state.compaction;
                let kind = if snapshot.compaction() != &state.compaction {
                    SessionNodeKind::Compaction
                } else {
                    SessionNodeKind::Commit
                };
                state.model = snapshot.history().to_vec();
                state.display.extend(display_messages.clone());
                state.revision = snapshot.revision();
                state.compaction = snapshot.compaction().clone();
                state.snapshot = Some(snapshot);
                (timestamp, display_messages, kind, changed)
            }
            SessionEntry::Session { .. }
            | SessionEntry::Node { .. }
            | SessionEntry::SetLeaf { .. }
            | SessionEntry::Upgrade { .. } => {
                unreachable!("entry was classified before projection")
            }
        };

        if !changed {
            if let Some(active) = self.active_leaf_id.as_ref() {
                if let Some(node) = self.nodes.get_mut(active) {
                    node.record.display_messages.extend(display_messages);
                }
            }
            if let Some(active_state) = self.active_state.as_mut() {
                active_state.display = state.display.clone();
            }
            return Ok(());
        }

        let id = NodeId::legacy(offset);
        let parent_id = self.active_leaf_id.clone();
        let snapshot = state.snapshot.clone().or(previous.snapshot).ok_or_else(|| {
            anyhow::anyhow!("legacy state at byte offset {offset} has no snapshot transition")
        });
        let transition = match snapshot {
            Ok(snapshot) => StoredStateTransition::Snapshot {
                snapshot: Box::new(snapshot),
            },
            Err(_) => {
                // Message-only v1 records predate snapshots. Their restored state is retained
                // directly on the virtual node and the transition is never serialized.
                StoredStateTransition::Snapshot {
                    snapshot: Box::new(synthetic_snapshot(self.session_id.as_deref(), state)?),
                }
            }
        };
        let node = LoadedNode {
            id,
            parent_id,
            timestamp,
            kind: suggested_kind,
            compaction_facts: None,
            transition,
            display_messages,
        };
        self.insert_restored_node(node, state.clone())
    }
}

fn refresh_snapshot_state(state: &mut PersistedSessionState) {
    let Some(previous) = state.snapshot.take() else {
        return;
    };
    let mut snapshot = SessionSnapshot::new(
        previous.session_id().clone(),
        state.revision,
        state.model.clone(),
        previous.provider().clone(),
        state.compaction.clone(),
    );
    for (key, value) in previous.metadata() {
        snapshot = snapshot.with_metadata(key.clone(), value.clone());
    }
    if let Some(prompt_cache_key) = previous.prompt_cache_key() {
        snapshot = snapshot.with_prompt_cache_key(prompt_cache_key);
    }
    state.snapshot = Some(snapshot);
}

fn synthetic_snapshot(
    session_id: Option<&str>,
    state: &PersistedSessionState,
) -> anyhow::Result<SessionSnapshot> {
    use rho_providers::model::ModelIdentity;
    use rho_sdk::SessionId;

    let id = session_id.ok_or_else(|| anyhow::anyhow!("legacy record appears before header"))?;
    Ok(SessionSnapshot::new(
        SessionId::from_string(id.to_owned())?,
        state.revision,
        state.model.clone(),
        ModelIdentity::new("legacy", "legacy", "legacy"),
        state.compaction.clone(),
    ))
}
