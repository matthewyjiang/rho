use std::{collections::BTreeMap, fmt, sync::Mutex};

use serde::{Deserialize, Serialize};

use crate::{
    model::{Message, ModelIdentity},
    Error, Revision, SessionId,
};

/// Current portable session snapshot schema.
pub const SESSION_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Versioned, portable state required to continue an SDK session.
///
/// Snapshots retain provider-native context because each block is scoped to an
/// exact provider/API/model identity. Restoring with another provider leaves
/// those blocks in history but they are omitted by handoff logic. Raw reasoning
/// is always cleared before a snapshot is created or serialized.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    schema_version: u32,
    session_id: SessionId,
    revision: Revision,
    history: Vec<Message>,
    provider: ModelIdentity,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    metadata: BTreeMap<String, String>,
}

impl SessionSnapshot {
    pub(crate) fn new(
        session_id: SessionId,
        revision: Revision,
        history: Vec<Message>,
        provider: ModelIdentity,
    ) -> Self {
        Self {
            schema_version: SESSION_SNAPSHOT_SCHEMA_VERSION,
            session_id,
            revision,
            history: sanitized_history(history),
            provider,
            metadata: BTreeMap::new(),
        }
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn history(&self) -> &[Message] {
        &self.history
    }

    pub fn provider(&self) -> &ModelIdentity {
        &self.provider
    }

    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn to_json(&self) -> Result<String, Error> {
        serde_json::to_string_pretty(self).map_err(|error| Error::Persistence {
            message: format!("failed to serialize session snapshot: {error}"),
        })
    }

    pub fn from_json(json: &str) -> Result<Self, Error> {
        let mut snapshot: Self =
            serde_json::from_str(json).map_err(|error| Error::Persistence {
                message: format!("failed to deserialize session snapshot: {error}"),
            })?;
        if snapshot.schema_version != SESSION_SNAPSHOT_SCHEMA_VERSION {
            return Err(Error::Persistence {
                message: format!(
                    "unsupported session snapshot schema {}; expected {}",
                    snapshot.schema_version, SESSION_SNAPSHOT_SCHEMA_VERSION
                ),
            });
        }
        snapshot.history = sanitized_history(snapshot.history);
        Ok(snapshot)
    }
}

fn sanitized_history(mut history: Vec<Message>) -> Vec<Message> {
    for message in &mut history {
        if let Message::AbortedAssistant(assistant) = message {
            assistant.reasoning.clear();
        }
    }
    history
}

/// Concrete in-memory snapshot adapter for hosts, tests, and examples.
///
/// The store replaces one complete snapshot while holding its mutex, so readers
/// observe either the previous revision or the complete new revision.
#[derive(Default)]
pub struct InMemorySessionStore {
    snapshots: Mutex<BTreeMap<SessionId, SessionSnapshot>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn save(&self, snapshot: SessionSnapshot) -> Option<SessionSnapshot> {
        self.snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(snapshot.session_id.clone(), snapshot)
    }

    pub fn load(&self, id: &SessionId) -> Option<SessionSnapshot> {
        self.snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .cloned()
    }

    pub fn remove(&self, id: &SessionId) -> Option<SessionSnapshot> {
        self.snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(id)
    }

    pub fn len(&self) -> usize {
        self.snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Debug for InMemorySessionStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let snapshots = self
            .snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        formatter
            .debug_struct("InMemorySessionStore")
            .field("session_ids", &snapshots.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
