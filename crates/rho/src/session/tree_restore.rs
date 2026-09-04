use std::{borrow::Cow, collections::HashSet};

use rho_sdk::SessionSnapshot;

use super::super::snapshot_delta::SnapshotReplay;

use super::{
    snapshot_less_parent_state_changed, NodeId, PersistedSessionState, SessionNode,
    SessionNodeKind, StoredDisplayMessage, StoredSnapshotDelta, StoredStateTransition,
};

#[cfg(test)]
#[path = "tree_replay_tests.rs"]
mod tests;

#[derive(Clone, Debug)]
pub(super) struct ReplayState {
    snapshot: SnapshotReplay,
    display: Vec<StoredDisplayMessage>,
}

impl super::SessionTree {
    pub(super) fn insert_explicit_node(&mut self, node: SessionNode) -> anyhow::Result<()> {
        if node.kind == SessionNodeKind::Commit && node.compaction_facts.is_some() {
            anyhow::bail!("commit node '{}' cannot store compaction facts", node.id);
        }
        if node.parent_id.is_none() && !self.nodes.is_empty() {
            anyhow::bail!("session node '{}' creates a disconnected root", node.id);
        }
        if let (Some(parent_id), StoredStateTransition::SnapshotDelta { delta }) =
            (&node.parent_id, &node.transition)
        {
            return self.insert_delta_node(&node, parent_id, delta);
        }
        self.ensure_active_state()?;
        let state = match (&node.parent_id, &node.transition) {
            (None, StoredStateTransition::Snapshot { snapshot }) => {
                if node.kind == SessionNodeKind::Compaction {
                    anyhow::bail!("root node '{}' cannot be a compaction", node.id);
                }
                self.state_from_snapshot(snapshot.as_ref().clone(), &node.display_messages)?
            }
            (None, StoredStateTransition::SnapshotDelta { .. }) => {
                anyhow::bail!("root node '{}' cannot store a snapshot delta", node.id)
            }
            (Some(parent_id), StoredStateTransition::Snapshot { snapshot }) => {
                let parent_facts = self
                    .nodes
                    .get(parent_id)
                    .ok_or_else(|| {
                        anyhow::anyhow!("node '{}' names missing parent '{parent_id}'", node.id)
                    })?
                    .facts()
                    .clone();
                let parent_snapshot = self.parent_snapshot(parent_id)?;
                let snapshot = snapshot.as_ref().clone();
                let state_changed = match parent_snapshot.as_ref() {
                    Some(parent_snapshot) => snapshot != **parent_snapshot,
                    // Message-only v1 parents keep restored state without a
                    // complete snapshot. A full snapshot child may restate that
                    // current revision when materializing the explicit tree,
                    // including resume-time history normalization.
                    None => {
                        let parent_state = self.reconstruct_state(parent_id)?;
                        snapshot_less_parent_state_changed(&snapshot, &parent_state)
                    }
                };
                if state_changed && snapshot.revision() <= parent_facts.revision {
                    anyhow::bail!(
                        "node '{}' changed state without advancing parent revision {}",
                        node.id,
                        parent_facts.revision
                    );
                }
                let compaction_changed = snapshot.compaction() != &parent_facts.compaction;
                if compaction_changed != (node.kind == SessionNodeKind::Compaction) {
                    anyhow::bail!(
                        "node '{}' kind does not match its compaction state transition",
                        node.id
                    );
                }
                let mut state = self.state_from_snapshot(snapshot, &[])?;
                state.display = self.display_from_parent(parent_id)?;
                state.display.extend(node.display_messages.iter().cloned());
                state
            }
            (Some(_), StoredStateTransition::SnapshotDelta { .. }) => {
                unreachable!("delta nodes use the incremental replay path above")
            }
        };
        self.insert_restored_node(node, state)
    }

    /// Keep one growable history during JSONL replay instead of copying every
    /// prefix. Save callers materialize the result before publishing the cache.
    fn insert_delta_node(
        &mut self,
        node: &SessionNode,
        parent_id: &NodeId,
        delta: &StoredSnapshotDelta,
    ) -> anyhow::Result<()> {
        let parent = self.nodes.get(parent_id).ok_or_else(|| {
            anyhow::anyhow!("node '{}' names missing parent '{parent_id}'", node.id)
        })?;
        let parent_facts = parent.facts().clone();
        if !parent_facts.has_snapshot {
            anyhow::bail!("parent '{parent_id}' has no complete snapshot");
        }
        if node.kind == SessionNodeKind::Compaction {
            anyhow::bail!("compaction node '{}' must store a full snapshot", node.id);
        }
        let mut replay = if self.active_leaf_id.as_ref() == Some(parent_id) {
            self.active_replay.take()
        } else {
            None
        };
        if replay.is_none() {
            let state = if self.active_leaf_id.as_ref() == Some(parent_id) {
                self.active_state.take()
            } else {
                None
            };
            let state = match state {
                Some(state) => state,
                None => self.reconstruct_state(parent_id)?,
            };
            let snapshot = state
                .snapshot
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("parent '{parent_id}' has no complete snapshot"))?;
            replay = Some(ReplayState {
                snapshot: SnapshotReplay::new(snapshot, state.model),
                display: state.display,
            });
        }
        let mut replay = replay.expect("parent state initialized");
        let changed = delta.apply(&mut replay.snapshot)?;
        if changed && replay.snapshot.header.revision() <= parent_facts.revision {
            anyhow::bail!(
                "node '{}' changed state without advancing parent revision {}",
                node.id,
                parent_facts.revision
            );
        }
        if replay.snapshot.header.compaction() != &parent_facts.compaction {
            anyhow::bail!(
                "node '{}' kind does not match its compaction state transition",
                node.id
            );
        }
        let facts = super::NodeFacts {
            revision: replay.snapshot.header.revision(),
            model_len: replay.snapshot.history.len(),
            has_snapshot: true,
            compaction: replay.snapshot.header.compaction().clone(),
        };
        replay.display.extend(node.display_messages.iter().cloned());
        self.insert_node(node.clone(), facts)?;
        self.active_state = None;
        self.active_replay = Some(replay);
        Ok(())
    }

    /// Restores one node's full state. The active leaf is already materialized
    /// after load; a pending `set_leaf` reconstructs on demand.
    pub(crate) fn state_for(&self, id: &NodeId) -> anyhow::Result<PersistedSessionState> {
        if self.active_leaf_id.as_ref() == Some(id) {
            if let Some(state) = self.active_state.clone() {
                return Ok(state);
            }
        }
        self.reconstruct_state(id)
    }

    pub(super) fn reconstruct_state(
        &self,
        target_id: &NodeId,
    ) -> anyhow::Result<PersistedSessionState> {
        let snapshot = self.reconstruct_snapshot(target_id)?;
        let mut state = self.state_from_snapshot(snapshot, &[])?;
        // Message-only v1 nodes store a synthetic snapshot on the transition
        // that was never serialized. They must not look like a delta base.
        if !self
            .nodes
            .get(target_id)
            .is_some_and(|node| node.facts().has_snapshot)
        {
            state.snapshot = None;
        }
        state.display = self.accumulated_display(target_id)?;
        Ok(state)
    }

    pub(crate) fn ensure_active_state(&mut self) -> anyhow::Result<()> {
        let Some(id) = self.active_leaf_id.clone() else {
            return Ok(());
        };
        if self.active_state.is_none() {
            self.active_state = Some(if let Some(replay) = self.active_replay.take() {
                let mut state = self.state_from_snapshot(replay.snapshot.into_snapshot(), &[])?;
                state.display = replay.display;
                state
            } else {
                self.reconstruct_state(&id)?
            });
        }
        Ok(())
    }

    /// Sequential load parents the next node on the current leaf, so this is a
    /// cache hit. Branches reconstruct the named parent from stored transitions.
    pub(super) fn parent_snapshot(
        &self,
        parent_id: &NodeId,
    ) -> anyhow::Result<Option<Cow<'_, SessionSnapshot>>> {
        let parent = self
            .nodes
            .get(parent_id)
            .ok_or_else(|| anyhow::anyhow!("session tree is missing node '{parent_id}'"))?;
        if !parent.facts.has_snapshot {
            return Ok(None);
        }
        if self.active_leaf_id.as_ref() == Some(parent_id) {
            if let Some(state) = &self.active_state {
                return Ok(state.snapshot.as_ref().map(Cow::Borrowed));
            }
        }
        Ok(Some(Cow::Owned(self.reconstruct_snapshot(parent_id)?)))
    }

    /// Sequential load reuses the leaf display vec. Branches rebuild from tails.
    pub(super) fn display_from_parent(
        &mut self,
        parent_id: &NodeId,
    ) -> anyhow::Result<Vec<StoredDisplayMessage>> {
        if self.active_leaf_id.as_ref() == Some(parent_id) {
            if let Some(state) = self.active_state.as_mut() {
                return Ok(std::mem::take(&mut state.display));
            }
        }
        self.accumulated_display(parent_id)
    }

    fn accumulated_display(&self, target_id: &NodeId) -> anyhow::Result<Vec<StoredDisplayMessage>> {
        let mut display = Vec::new();
        for node in self.path_to(target_id)? {
            display.extend(node.display_messages().iter().cloned());
        }
        Ok(display)
    }

    fn reconstruct_snapshot(&self, target_id: &NodeId) -> anyhow::Result<SessionSnapshot> {
        let mut deltas: Vec<&StoredSnapshotDelta> = Vec::new();
        let mut id = target_id;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(id.clone()) {
                anyhow::bail!("session tree contains a cycle at node '{id}'");
            }
            let node = self
                .nodes
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("session tree is missing node '{id}'"))?;
            match node.transition() {
                StoredStateTransition::Snapshot { snapshot } => {
                    let mut replay = SnapshotReplay::new(snapshot, snapshot.history().to_vec());
                    for delta in deltas.into_iter().rev() {
                        delta.apply(&mut replay)?;
                    }
                    return Ok(replay.into_snapshot());
                }
                StoredStateTransition::SnapshotDelta { delta } => {
                    deltas.push(delta.as_ref());
                    id = node.parent_id().ok_or_else(|| {
                        anyhow::anyhow!("delta node '{id}' is missing a parent snapshot")
                    })?;
                }
            }
        }
    }
}
