use std::collections::BTreeMap;

use rho_sdk::{CompactionState, Revision, SessionId, SessionSnapshot};
use serde::{Deserialize, Serialize};

use rho_providers::model::{Message, ModelIdentity};

/// Borrowed state used to validate that an append only grows history.
#[derive(Clone, Debug)]
pub(super) struct SnapshotDeltaBase<'a> {
    session_id: &'a SessionId,
    revision: Revision,
    history: &'a [Message],
    compaction: &'a CompactionState,
}

impl<'a> SnapshotDeltaBase<'a> {
    pub(super) fn from_snapshot(snapshot: &'a SessionSnapshot) -> Self {
        Self {
            session_id: snapshot.session_id(),
            revision: snapshot.revision(),
            history: snapshot.history(),
            compaction: snapshot.compaction(),
        }
    }

    pub(super) fn compaction(&self) -> &'a CompactionState {
        self.compaction
    }

    pub(super) fn history_len(&self) -> usize {
        self.history.len()
    }
}

/// The changing snapshot fields plus only the history appended since a base snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredSnapshotDelta {
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

/// Mutable replay state. The header has no history; only the final snapshot
/// materializes a second history copy for the session's model/snapshot pair.
#[derive(Clone, Debug)]
pub(super) struct SnapshotReplay {
    pub(super) header: SessionSnapshot,
    pub(super) history: Vec<Message>,
}

impl SnapshotReplay {
    pub(super) fn new(snapshot: &SessionSnapshot, history: Vec<Message>) -> Self {
        let mut header = SessionSnapshot::new(
            snapshot.session_id().clone(),
            snapshot.revision(),
            Vec::new(),
            snapshot.provider().clone(),
            snapshot.compaction().clone(),
        );
        for (key, value) in snapshot.metadata() {
            header = header.with_metadata(key.clone(), value.clone());
        }
        if let Some(key) = snapshot.prompt_cache_key() {
            header = header.with_prompt_cache_key(key);
        }
        Self { header, history }
    }

    /// The replayed state as a base for the next delta, without materializing
    /// a second history copy.
    pub(super) fn delta_base(&self) -> SnapshotDeltaBase<'_> {
        SnapshotDeltaBase {
            session_id: self.header.session_id(),
            revision: self.header.revision(),
            history: &self.history,
            compaction: self.header.compaction(),
        }
    }

    pub(super) fn into_snapshot(self) -> SessionSnapshot {
        let mut snapshot = SessionSnapshot::new(
            self.header.session_id().clone(),
            self.header.revision(),
            self.history,
            self.header.provider().clone(),
            self.header.compaction().clone(),
        );
        for (key, value) in self.header.metadata() {
            snapshot = snapshot.with_metadata(key.clone(), value.clone());
        }
        if let Some(key) = self.header.prompt_cache_key() {
            snapshot = snapshot.with_prompt_cache_key(key);
        }
        snapshot
    }
}

impl StoredSnapshotDelta {
    pub(super) fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }

    pub(super) fn appended_history(&self) -> &[Message] {
        &self.appended_history
    }

    /// The delta from `base` to `current`, or `None` unless `current` extends
    /// `base` exactly. The prefix check reads every base message: `current` is
    /// the caller's independent copy, so nothing cheaper proves its prefix is
    /// unchanged, and a wrong append-only answer would corrupt restore.
    pub(super) fn after(base: &SnapshotDeltaBase<'_>, current: &SessionSnapshot) -> Option<Self> {
        if base.session_id != current.session_id()
            || base.revision > current.revision()
            || base.compaction != current.compaction()
            || base.history.len() > current.history().len()
            || current.history().get(..base.history.len()) != Some(base.history)
        {
            return None;
        }
        let appended_history = current.history()[base.history.len()..].to_vec();
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
        let mut replay = SnapshotReplay::new(previous, previous.history().to_vec());
        self.apply(&mut replay)?;
        Ok(replay.into_snapshot())
    }

    /// Validates even no-op deltas before appending. Returns whether any state
    /// changed, including provider, metadata and cache identity at equal revision.
    pub(super) fn apply(&self, replay: &mut SnapshotReplay) -> anyhow::Result<bool> {
        let previous = &replay.header;
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

        let mut snapshot = SessionSnapshot::new(
            self.session_id.clone(),
            self.revision,
            Vec::new(),
            self.provider.clone(),
            self.compaction.clone(),
        );
        for (key, value) in &self.metadata {
            snapshot = snapshot.with_metadata(key.clone(), value.clone());
        }
        if let Some(prompt_cache_key) = &self.prompt_cache_key {
            snapshot = snapshot.with_prompt_cache_key(prompt_cache_key);
        }
        let changed = snapshot != replay.header || !self.appended_history.is_empty();
        replay.history.extend_from_slice(&self.appended_history);
        replay.header = snapshot;
        Ok(changed)
    }
}
