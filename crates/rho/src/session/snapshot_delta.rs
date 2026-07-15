use std::collections::BTreeMap;

use rho_sdk::{CompactionState, Revision, SessionId, SessionSnapshot};
use serde::{Deserialize, Serialize};

use crate::model::{Message, ModelIdentity};

/// Minimal state retained between appends to validate that history only grew.
#[derive(Clone, Debug)]
pub(super) struct SnapshotDeltaBase {
    session_id: SessionId,
    revision: Revision,
    history_len: usize,
    history_tail: Option<Message>,
    compaction: CompactionState,
}

impl SnapshotDeltaBase {
    pub(super) fn from_snapshot(snapshot: &SessionSnapshot) -> Self {
        Self {
            session_id: snapshot.session_id().clone(),
            revision: snapshot.revision(),
            history_len: snapshot.history().len(),
            history_tail: snapshot.history().last().cloned(),
            compaction: snapshot.compaction().clone(),
        }
    }
}

/// The changing snapshot fields plus only the history appended since a base snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct StoredSnapshotDelta {
    base_revision: Revision,
    session_id: SessionId,
    revision: Revision,
    appended_history: Vec<Message>,
    provider: ModelIdentity,
    compaction: CompactionState,
    metadata: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
}

impl StoredSnapshotDelta {
    pub(super) fn after(base: &SnapshotDeltaBase, current: &SessionSnapshot) -> Option<Self> {
        if &base.session_id != current.session_id()
            || base.revision > current.revision()
            || base.compaction != *current.compaction()
            || base.history_len > current.history().len()
            || base.history_tail.as_ref()
                != base
                    .history_len
                    .checked_sub(1)
                    .and_then(|index| current.history().get(index))
        {
            return None;
        }
        let appended_history = current.history()[base.history_len..].to_vec();
        Some(Self {
            base_revision: base.revision,
            session_id: current.session_id().clone(),
            revision: current.revision(),
            appended_history,
            provider: current.provider().clone(),
            compaction: current.compaction().clone(),
            metadata: current.metadata().clone(),
            prompt_cache_key: current.prompt_cache_key().map(str::to_owned),
        })
    }

    pub(super) fn restore(&self, previous: &SessionSnapshot) -> anyhow::Result<SessionSnapshot> {
        if previous.session_id() != &self.session_id {
            anyhow::bail!(
                "snapshot delta session id '{}' does not match base session id '{}'",
                self.session_id,
                previous.session_id()
            );
        }
        if previous.revision() != self.base_revision {
            anyhow::bail!(
                "snapshot delta base revision {} does not match previous revision {}",
                self.base_revision,
                previous.revision()
            );
        }
        if self.revision < self.base_revision {
            anyhow::bail!(
                "snapshot delta revision {} precedes base revision {}",
                self.revision,
                self.base_revision
            );
        }

        let mut history = Vec::with_capacity(
            previous
                .history()
                .len()
                .saturating_add(self.appended_history.len()),
        );
        history.extend_from_slice(previous.history());
        history.extend_from_slice(&self.appended_history);
        let mut snapshot = SessionSnapshot::new(
            self.session_id.clone(),
            self.revision,
            history,
            self.provider.clone(),
            self.compaction.clone(),
        );
        for (key, value) in &self.metadata {
            snapshot = snapshot.with_metadata(key.clone(), value.clone());
        }
        if let Some(prompt_cache_key) = &self.prompt_cache_key {
            snapshot = snapshot.with_prompt_cache_key(prompt_cache_key);
        }
        Ok(snapshot)
    }
}
