use std::collections::HashSet;

use rho_sdk::SessionSnapshot;

use super::{
    snapshot_less_parent_state_changed, NodeId, PersistedSessionState, SessionNode,
    SessionNodeKind, StoredDisplayMessage, StoredSnapshotDelta, StoredStateTransition,
};

impl super::SessionTree {
    pub(super) fn insert_explicit_node(&mut self, node: SessionNode) -> anyhow::Result<()> {
        if node.kind == SessionNodeKind::Commit && node.compaction_facts.is_some() {
            anyhow::bail!("commit node '{}' cannot store compaction facts", node.id);
        }
        if node.parent_id.is_none() && !self.nodes.is_empty() {
            anyhow::bail!("session node '{}' creates a disconnected root", node.id);
        }
        let state = match (&node.parent_id, &node.transition) {
            (None, StoredStateTransition::Snapshot { snapshot }) => {
                if node.kind == SessionNodeKind::Compaction {
                    anyhow::bail!("root node '{}' cannot be a compaction", node.id);
                }
                self.state_from_snapshot(snapshot.as_ref(), &node.display_messages)?
            }
            (None, StoredStateTransition::SnapshotDelta { .. }) => {
                anyhow::bail!("root node '{}' cannot store a snapshot delta", node.id)
            }
            (Some(parent_id), transition) => {
                let parent_facts = self
                    .nodes
                    .get(parent_id)
                    .ok_or_else(|| {
                        anyhow::anyhow!("node '{}' names missing parent '{parent_id}'", node.id)
                    })?
                    .facts()
                    .clone();
                if node.kind == SessionNodeKind::Compaction
                    && !matches!(transition, StoredStateTransition::Snapshot { .. })
                {
                    anyhow::bail!("compaction node '{}' must store a full snapshot", node.id);
                }
                let parent_snapshot = self.parent_snapshot(parent_id)?;
                let snapshot = match transition {
                    StoredStateTransition::Snapshot { snapshot } => snapshot.as_ref().clone(),
                    StoredStateTransition::SnapshotDelta { delta } => {
                        let base = parent_snapshot.as_ref().ok_or_else(|| {
                            anyhow::anyhow!("parent '{parent_id}' has no complete snapshot")
                        })?;
                        delta.restore(base)?
                    }
                };
                let state_changed = match parent_snapshot.as_ref() {
                    Some(parent_snapshot) => snapshot != *parent_snapshot,
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
                let mut state = self.state_from_snapshot(&snapshot, &[])?;
                state.display = self.display_from_parent(parent_id)?;
                state.display.extend(node.display_messages.iter().cloned());
                state
            }
        };
        self.insert_restored_node(node, state)
    }

    /// Restores one node's full state. The active leaf is already materialized.
    pub(crate) fn state_for(&self, id: &NodeId) -> anyhow::Result<PersistedSessionState> {
        if self.active_leaf_id.as_ref() == Some(id) {
            return self
                .active_state
                .clone()
                .ok_or_else(|| anyhow::anyhow!("active leaf '{id}' has no materialized state"));
        }
        self.reconstruct_state(id)
    }

    pub(super) fn reconstruct_state(
        &self,
        target_id: &NodeId,
    ) -> anyhow::Result<PersistedSessionState> {
        let snapshot = self.reconstruct_snapshot(target_id)?;
        let mut state = self.state_from_snapshot(&snapshot, &[])?;
        state.display = self.accumulated_display(target_id)?;
        Ok(state)
    }

    /// Sequential load parents the next node on the current leaf, so this is a
    /// cache hit. Branches reconstruct the named parent from stored transitions.
    pub(super) fn parent_snapshot(
        &self,
        parent_id: &NodeId,
    ) -> anyhow::Result<Option<SessionSnapshot>> {
        let parent = self
            .nodes
            .get(parent_id)
            .ok_or_else(|| anyhow::anyhow!("session tree is missing node '{parent_id}'"))?;
        if !parent.facts.has_snapshot {
            return Ok(None);
        }
        if self.active_leaf_id.as_ref() == Some(parent_id) {
            return Ok(self
                .active_state
                .as_ref()
                .and_then(|state| state.snapshot.clone()));
        }
        Ok(Some(self.reconstruct_snapshot(parent_id)?))
    }

    /// Sequential load reuses the leaf display vec. Branches rebuild from tails.
    pub(super) fn display_from_parent(
        &mut self,
        parent_id: &NodeId,
    ) -> anyhow::Result<Vec<StoredDisplayMessage>> {
        if self.active_leaf_id.as_ref() == Some(parent_id) {
            return Ok(self
                .active_state
                .as_mut()
                .map(|state| std::mem::take(&mut state.display))
                .unwrap_or_default());
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
                    let mut snapshot = snapshot.as_ref().clone();
                    for delta in deltas.into_iter().rev() {
                        snapshot = delta.restore(&snapshot)?;
                    }
                    return Ok(snapshot);
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
