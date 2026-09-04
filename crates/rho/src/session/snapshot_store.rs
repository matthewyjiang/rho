use rho_providers::model::{Message, ModelIdentity};
use rho_sdk::{SessionId, SessionSnapshot};

use super::persistence::{
    insert_interrupted_tool_placeholders, timestamp, AppendCursor, PersistedSessionState,
    SessionEntry, StoredDisplayMessage,
};
use super::snapshot_delta::{SnapshotDeltaBase, StoredSnapshotDelta};
#[cfg(test)]
use super::tree::SessionTreeFacts;
use super::tree::{
    NodeId, SessionNode, SessionNodeKind, SessionTree, StoredCompactionFacts, StoredStateTransition,
};
use super::{index, Session};

#[cfg(test)]
#[path = "snapshot_store_tests.rs"]
mod tests;

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
        self.save_snapshot_with_compaction_facts(snapshot, display_tail, None)?;
        Ok(())
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
        )?;
        Ok(())
    }

    fn save_snapshot_with_compaction_facts(
        &self,
        snapshot: &SessionSnapshot,
        display_tail: &[Message],
        supplied_compaction_facts: Option<StoredCompactionFacts>,
    ) -> anyhow::Result<Option<super::SessionIndexRecord>> {
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
        let mut tree = load_cached_tree(&mut cursor, &self.path)?;
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
        // Force a full snapshot if compaction changed or if upgrading a legacy (< v4)
        // session. Legacy sessions and compacted baselines do not share delta continuity
        // with their predecessor.
        let transition = if compaction_changed || tree.needs_upgrade_marker() {
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
        let node_timestamp = timestamp();
        let display_messages = display_tail
            .iter()
            .cloned()
            .map(|message| StoredDisplayMessage {
                timestamp: node_timestamp.clone(),
                message,
            })
            .collect();

        let node = SessionNode {
            id: NodeId::new(),
            parent_id,
            timestamp: node_timestamp,
            kind,
            compaction_facts,
            transition,
            display_messages,
        };

        self.commit_tree_entry(&mut cursor, &mut tree, SessionEntry::Node { node })?;
        let record = self.record_mirrored_index(&tree);
        store_cached_tree(&mut cursor, &self.path, tree);
        Ok(record)
    }

    fn record_mirrored_index(&self, tree: &SessionTree) -> Option<super::SessionIndexRecord> {
        match tree.summary_record(&self.path, &self.cwd) {
            Ok(record) => {
                let _ = index::record_snapshot_record(self, &record);
                Some(record)
            }
            Err(err) => {
                tracing::warn!(
                    "failed to generate session index summary from in-memory tree: {err:#}"
                );
                None
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn save_snapshot_mirrored_record(
        &self,
        snapshot: &SessionSnapshot,
        display_tail: &[Message],
    ) -> anyhow::Result<super::SessionIndexRecord> {
        self.save_snapshot_with_compaction_facts(snapshot, display_tail, None)?
            .ok_or_else(|| anyhow::anyhow!("save produced no mirrored index record"))
    }

    #[cfg(test)]
    pub(crate) fn set_leaf_mirrored_record(
        &self,
        target_id: &NodeId,
    ) -> anyhow::Result<super::SessionIndexRecord> {
        self.set_leaf_and_record(target_id)?
            .ok_or_else(|| anyhow::anyhow!("set_leaf produced no mirrored index record"))
    }

    #[cfg(test)]
    pub(crate) fn session_tree(&self) -> anyhow::Result<SessionTree> {
        self.with_session_tree(|tree| Ok(tree.clone()))
    }

    /// Adopts a tree parsed elsewhere (e.g. session open) as the cache seed.
    pub(super) fn cache_loaded_tree(&self, tree: SessionTree) {
        let mut cursor = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cursor.seed_loaded_tree(tree, &self.path);
    }

    /// Runs `visit` on the cached tree when the file still matches the state
    /// it was parsed from. Reloads from disk when another writer changed the
    /// transcript.
    pub(crate) fn with_session_tree<R>(
        &self,
        visit: impl FnOnce(&SessionTree) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        let mut cursor = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(tree) = cursor.take_tree(&self.path) {
            let result = visit(&tree);
            cursor.store_tree(tree, &self.path);
            return result;
        }
        let tree = SessionTree::load(&self.path)?;
        let result = visit(&tree);
        cursor.store_tree(tree, &self.path);
        result
    }

    #[cfg(test)]
    pub(crate) fn tree_facts(&self) -> anyhow::Result<SessionTreeFacts> {
        self.with_session_tree(|tree| Ok(tree.facts()))
    }

    pub(crate) fn tree_items(&self) -> anyhow::Result<Vec<super::tree::SessionTreeItem>> {
        self.with_session_tree(SessionTree::items)
    }

    pub(crate) fn histories_for_node(
        &self,
        target_id: &NodeId,
    ) -> anyhow::Result<super::SessionHistories> {
        self.with_session_tree(|tree| {
            let state = tree.state_for(target_id)?;
            let display = tree.projected_display(target_id)?;
            Ok(super::SessionHistories {
                model: insert_interrupted_tool_placeholders(state.model),
                display: insert_interrupted_tool_placeholders(
                    display.into_iter().map(|entry| entry.message).collect(),
                ),
            })
        })
    }

    pub(crate) fn snapshot_for_node(
        &self,
        target_id: &NodeId,
        provider: ModelIdentity,
        prompt_cache_key: String,
    ) -> anyhow::Result<SessionSnapshot> {
        self.with_session_tree(|tree| {
            let state = tree.state_for(target_id)?;
            self.snapshot_from_state(state, provider, prompt_cache_key)
        })
    }

    /// Selects an existing valid node without changing any stored state.
    pub(crate) fn set_leaf(&self, target_id: &NodeId) -> anyhow::Result<()> {
        self.set_leaf_and_record(target_id)?;
        Ok(())
    }

    fn set_leaf_and_record(
        &self,
        target_id: &NodeId,
    ) -> anyhow::Result<Option<super::SessionIndexRecord>> {
        let mut cursor = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut tree = load_cached_tree(&mut cursor, &self.path)?;
        if tree.node(target_id).is_none() {
            restore_cached_tree(&mut cursor, &self.path, tree);
            anyhow::bail!("cannot select missing session node '{target_id}'");
        }
        if tree.active_leaf_id() == Some(target_id) {
            restore_cached_tree(&mut cursor, &self.path, tree);
            return Ok(None);
        }
        let ts = timestamp();
        self.commit_tree_entry(
            &mut cursor,
            &mut tree,
            SessionEntry::SetLeaf {
                timestamp: ts,
                target_id: target_id.clone(),
            },
        )?;
        let record = self.record_mirrored_index(&tree);
        store_cached_tree(&mut cursor, &self.path, tree);
        Ok(record)
    }

    pub(crate) fn snapshot_for_resume(
        &self,
        provider: ModelIdentity,
        prompt_cache_key: String,
    ) -> anyhow::Result<SessionSnapshot> {
        let state = self.active_persisted_state()?;
        self.snapshot_from_state(state, provider, prompt_cache_key)
    }

    fn active_persisted_state(&self) -> anyhow::Result<PersistedSessionState> {
        self.with_session_tree(|tree| Ok(tree.active_state().cloned().unwrap_or_default()))
    }

    /// Applies a tree-mutating entry and keeps the in-memory tree only when the
    /// file write succeeds. A failed append rolls the file back, so the cache
    /// must not keep the rejected node.
    fn commit_tree_entry(
        &self,
        cursor: &mut AppendCursor,
        tree: &mut SessionTree,
        entry: SessionEntry,
    ) -> anyhow::Result<()> {
        match self.append_tree_entry(cursor, tree, entry) {
            Ok(()) => Ok(()),
            Err(error) => {
                cursor.invalidate_tree();
                Err(error)
            }
        }
    }

    fn snapshot_from_state(
        &self,
        state: super::persistence::PersistedSessionState,
        provider: ModelIdentity,
        prompt_cache_key: String,
    ) -> anyhow::Result<SessionSnapshot> {
        let history = insert_interrupted_tool_placeholders(state.model);
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
            let state = self.active_persisted_state().map_err(persistence_error)?;
            let Some(stored) = state.snapshot.as_ref() else {
                return Ok(None);
            };
            let cache_key = stored
                .prompt_cache_key()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("rho:{}", self.id));
            let provider = stored.provider().clone();
            self.snapshot_from_state(state, provider, cache_key)
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

fn load_cached_tree(
    cursor: &mut AppendCursor,
    path: &std::path::Path,
) -> anyhow::Result<SessionTree> {
    if let Some(tree) = cursor.take_tree(path) {
        return Ok(tree);
    }
    SessionTree::load(path)
}

fn restore_cached_tree(cursor: &mut AppendCursor, path: &std::path::Path, tree: SessionTree) {
    cursor.store_tree(tree, path);
}

fn store_cached_tree(cursor: &mut AppendCursor, path: &std::path::Path, tree: SessionTree) {
    // The valid_len gate keeps a rejected/rolled-back write out of the cache.
    match cursor.valid_len {
        Some(_) => cursor.store_tree(tree, path),
        None => cursor.invalidate_tree(),
    }
}
