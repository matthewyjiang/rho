use std::fs;

use rho_providers::model::{Message, ModelIdentity};
use rho_sdk::{SessionId, SessionSnapshot};

use super::persistence::{read_session_state, timestamp, SessionEntry, StoredDisplayMessage};
use super::snapshot_delta::{SnapshotDeltaBase, StoredSnapshotDelta};
use super::{drop_incomplete_tool_turn_tail, index, Session};

impl Session {
    /// Persists one SDK snapshot state and its newly visible transcript tail.
    ///
    /// The snapshot update and display update share one JSONL record. Readers
    /// ignore a truncated final record, so a failed or interrupted append
    /// retains the previous complete snapshot state and transcript.
    pub(crate) fn save_snapshot(
        &self,
        snapshot: &SessionSnapshot,
        display_tail: &[Message],
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
        let display_messages: Vec<StoredDisplayMessage> = display_tail
            .iter()
            .cloned()
            .map(|message| StoredDisplayMessage {
                timestamp: timestamp(),
                message,
            })
            .collect();
        let actual_len = fs::metadata(&self.path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if cursor
            .valid_len
            .is_some_and(|valid_len| valid_len != actual_len)
        {
            cursor.valid_len = None;
            cursor.last_snapshot = None;
        }
        let entry = match cursor
            .last_snapshot
            .as_ref()
            .and_then(|previous| StoredSnapshotDelta::after(previous, snapshot))
        {
            Some(delta) => SessionEntry::SnapshotDelta {
                timestamp: timestamp(),
                delta: Box::new(delta),
                display_messages,
            },
            None => SessionEntry::Snapshot {
                timestamp: timestamp(),
                snapshot: Box::new(snapshot.clone()),
                display_messages,
            },
        };
        self.append_entry_unlocked(&mut cursor, &entry)?;
        cursor.last_snapshot = Some(SnapshotDeltaBase::from_snapshot(snapshot));
        let _ = index::record_snapshot(self);
        Ok(())
    }

    pub(crate) fn snapshot_for_resume(
        &self,
        provider: ModelIdentity,
        prompt_cache_key: String,
    ) -> anyhow::Result<SessionSnapshot> {
        let state = read_session_state(&self.path)?;
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
