use rho_providers::model::{Message, ModelIdentity};
use rho_sdk::{SessionId, SessionSnapshot};

use super::persistence::{read_session_state, timestamp, SessionEntry, StoredDisplayMessage};
use super::snapshot_delta::{SnapshotDeltaBase, StoredSnapshotDelta};
#[cfg(test)]
use super::tree::SessionTreeFacts;
use super::tree::{
    NodeId, SessionNode, SessionNodeKind, SessionTree, StoredCompactionFacts, StoredStateTransition,
};
use super::{drop_incomplete_tool_turn_tail, index, Session};

impl Session {
    /// Persists one SDK snapshot state and its newly visible transcript tail.
    ///
    /// The state and display update share one explicit tree node. Readers ignore
    /// a truncated final record, so an interrupted append retains the previous
    /// complete state and active leaf.
    pub(crate) fn save_snapshot(
        &self,
        snapshot: &SessionSnapshot,
        display_tail: &[Message],
    ) -> anyhow::Result<()> {
        self.save_snapshot_with_compaction_facts(snapshot, display_tail, None)
    }

    pub(crate) fn save_compaction_snapshot(
        &self,
        snapshot: &SessionSnapshot,
        display_tail: &[Message],
        outcome: &rho_sdk::CompactionOutcome,
    ) -> anyhow::Result<()> {
        self.save_snapshot_with_compaction_facts(
            snapshot,
            display_tail,
            Some(StoredCompactionFacts {
                previous_messages: outcome.previous_messages(),
                current_messages: outcome.current_messages(),
                previous_tokens: outcome.previous_tokens(),
                current_tokens: outcome.current_tokens(),
                cost_usd_micros: outcome.cost_usd_micros(),
            }),
        )
    }

    fn save_snapshot_with_compaction_facts(
        &self,
        snapshot: &SessionSnapshot,
        display_tail: &[Message],
        supplied_compaction_facts: Option<StoredCompactionFacts>,
    ) -> anyhow::Result<()> {
        if snapshot.session_id().as_str() != self.id {
            anyhow::bail!(
                "snapshot session id '{}' does not match store id '{}'",
                snapshot.session_id(),
                self.id
            );
        }
        let mut cursor = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tree = SessionTree::load(&self.path)?;
        let parent_id = tree.active_leaf_id().cloned();
        let parent_snapshot = tree
            .active_state()
            .and_then(|state| state.snapshot.as_ref());
        let compaction_changed =
            parent_snapshot.is_some_and(|parent| parent.compaction() != snapshot.compaction());
        let kind = if compaction_changed {
            SessionNodeKind::Compaction
        } else {
            SessionNodeKind::Commit
        };
        let compaction_facts = compaction_changed.then(|| {
            supplied_compaction_facts.unwrap_or_else(|| StoredCompactionFacts {
                previous_messages: parent_snapshot.map_or(0, |parent| parent.history().len()),
                current_messages: snapshot.history().len(),
                previous_tokens: snapshot
                    .compaction()
                    .last_previous_tokens()
                    .unwrap_or_default(),
                current_tokens: snapshot
                    .compaction()
                    .last_current_tokens()
                    .unwrap_or_default(),
                cost_usd_micros: None,
            })
        });
        let transition = if compaction_changed {
            StoredStateTransition::Snapshot {
                snapshot: Box::new(snapshot.clone()),
            }
        } else {
            parent_snapshot
                .and_then(|parent| {
                    StoredSnapshotDelta::after(&SnapshotDeltaBase::from_snapshot(parent), snapshot)
                })
                .map_or_else(
                    || StoredStateTransition::Snapshot {
                        snapshot: Box::new(snapshot.clone()),
                    },
                    |delta| StoredStateTransition::SnapshotDelta {
                        delta: Box::new(delta),
                    },
                )
        };
        let display_messages = display_tail
            .iter()
            .cloned()
            .map(|message| StoredDisplayMessage {
                timestamp: timestamp(),
                message,
            })
            .collect();

        if tree.needs_upgrade_marker() {
            let active_leaf_id = parent_id.clone().ok_or_else(|| {
                anyhow::anyhow!("legacy session has no state node to upgrade from")
            })?;
            self.append_entry_unlocked(
                &mut cursor,
                &SessionEntry::Upgrade {
                    timestamp: timestamp(),
                    active_leaf_id,
                },
            )?;
        }
        self.append_entry_unlocked(
            &mut cursor,
            &SessionEntry::Node {
                node: SessionNode {
                    id: NodeId::new(),
                    parent_id,
                    timestamp: timestamp(),
                    kind,
                    compaction_facts,
                    transition,
                    display_messages,
                },
            },
        )?;
        cursor.last_snapshot = Some(SnapshotDeltaBase::from_snapshot(snapshot));
        let _ = index::record_snapshot(self);
        Ok(())
    }

    pub(crate) fn session_tree(&self) -> anyhow::Result<SessionTree> {
        SessionTree::load(&self.path)
    }

    #[cfg(test)]
    pub(crate) fn tree_facts(&self) -> anyhow::Result<SessionTreeFacts> {
        Ok(self.session_tree()?.facts())
    }

    pub(crate) fn tree_items(&self) -> anyhow::Result<Vec<super::tree::SessionTreeItem>> {
        self.session_tree()?.items()
    }

    pub(crate) fn histories_for_node(
        &self,
        target_id: &NodeId,
    ) -> anyhow::Result<super::SessionHistories> {
        let tree = self.session_tree()?;
        let state = tree
            .node(target_id)
            .ok_or_else(|| anyhow::anyhow!("session tree is missing node '{target_id}'"))?
            .state();
        let display = tree.projected_display(target_id)?;
        Ok(super::SessionHistories {
            model: drop_incomplete_tool_turn_tail(state.model.clone()),
            display: drop_incomplete_tool_turn_tail(
                display.into_iter().map(|entry| entry.message).collect(),
            ),
        })
    }

    pub(crate) fn snapshot_for_node(
        &self,
        target_id: &NodeId,
        provider: ModelIdentity,
        prompt_cache_key: String,
    ) -> anyhow::Result<SessionSnapshot> {
        let tree = self.session_tree()?;
        let state = tree
            .node(target_id)
            .ok_or_else(|| anyhow::anyhow!("session tree is missing node '{target_id}'"))?
            .state();
        self.snapshot_from_state(state.clone(), provider, prompt_cache_key)
    }

    /// Selects an existing valid node without changing any stored state.
    pub(crate) fn set_leaf(&self, target_id: &NodeId) -> anyhow::Result<()> {
        let mut cursor = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tree = SessionTree::load(&self.path)?;
        if tree.node(target_id).is_none() {
            anyhow::bail!("cannot select missing session node '{target_id}'");
        }
        if tree.active_leaf_id() == Some(target_id) {
            return Ok(());
        }
        if tree.needs_upgrade_marker() {
            let active_leaf_id = tree.active_leaf_id().cloned().ok_or_else(|| {
                anyhow::anyhow!("legacy session has no state node to upgrade from")
            })?;
            self.append_entry_unlocked(
                &mut cursor,
                &SessionEntry::Upgrade {
                    timestamp: timestamp(),
                    active_leaf_id,
                },
            )?;
        }
        self.append_entry_unlocked(
            &mut cursor,
            &SessionEntry::SetLeaf {
                timestamp: timestamp(),
                target_id: target_id.clone(),
            },
        )?;
        cursor.last_snapshot = tree
            .node(target_id)
            .and_then(|node| node.state().snapshot.as_ref())
            .map(SnapshotDeltaBase::from_snapshot);
        let _ = index::record_snapshot(self);
        Ok(())
    }

    pub(crate) fn snapshot_for_resume(
        &self,
        provider: ModelIdentity,
        prompt_cache_key: String,
    ) -> anyhow::Result<SessionSnapshot> {
        self.snapshot_from_state(read_session_state(&self.path)?, provider, prompt_cache_key)
    }

    fn snapshot_from_state(
        &self,
        state: super::persistence::PersistedSessionState,
        provider: ModelIdentity,
        prompt_cache_key: String,
    ) -> anyhow::Result<SessionSnapshot> {
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
