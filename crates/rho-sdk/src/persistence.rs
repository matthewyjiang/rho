use std::{collections::BTreeMap, fmt, future::Future, pin::Pin, sync::Mutex};

use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    model::{Message, ModelIdentity},
    CompactionState, Error, Revision, SessionId,
};

/// Oldest portable session snapshot schema accepted by this SDK.
pub const MIN_SESSION_SNAPSHOT_SCHEMA_VERSION: u32 = 1;
/// Current portable session snapshot schema.
pub const SESSION_SNAPSHOT_SCHEMA_VERSION: u32 = 2;

/// Versioned, portable state required to continue an SDK session.
///
/// Snapshots retain provider-native context because each block is scoped to an
/// exact provider/API/model identity. Restoring with another provider leaves
/// those blocks in history but they are omitted by handoff logic. Raw reasoning
/// is always cleared before a snapshot is created or serialized.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionSnapshot {
    schema_version: u32,
    session_id: SessionId,
    revision: Revision,
    history: Vec<Message>,
    provider: ModelIdentity,
    compaction: CompactionState,
    metadata: BTreeMap<String, String>,
    prompt_cache_key: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SessionSnapshotWire {
    schema_version: u32,
    session_id: SessionId,
    revision: Revision,
    history: Vec<Message>,
    provider: ModelIdentity,
    #[serde(default)]
    compaction: CompactionState,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    metadata: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
}

impl SessionSnapshot {
    /// Constructs a portable snapshot for a host persistence adapter.
    pub fn new(
        session_id: SessionId,
        revision: Revision,
        history: Vec<Message>,
        provider: ModelIdentity,
        compaction: CompactionState,
    ) -> Self {
        Self {
            schema_version: SESSION_SNAPSHOT_SCHEMA_VERSION,
            session_id,
            revision,
            history: sanitized_history(history),
            provider,
            compaction,
            metadata: BTreeMap::new(),
            prompt_cache_key: None,
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

    pub fn compaction(&self) -> &CompactionState {
        &self.compaction
    }

    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }

    /// Returns the opaque, non-secret provider prompt-cache identity.
    pub fn prompt_cache_key(&self) -> Option<&str> {
        self.prompt_cache_key.as_deref()
    }

    pub fn with_prompt_cache_key(mut self, prompt_cache_key: impl Into<String>) -> Self {
        self.prompt_cache_key = Some(prompt_cache_key.into());
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Reports provider-native blocks that cannot replay to `target`.
    pub fn provider_context_omissions(
        &self,
        target: &ModelIdentity,
    ) -> crate::model::handoff::HandoffReport {
        crate::model::handoff::report_message_omissions(&self.history, target)
    }

    pub fn to_json(&self) -> Result<String, Error> {
        serde_json::to_string_pretty(self).map_err(|error| Error::Persistence {
            message: format!("failed to serialize session snapshot: {error}"),
        })
    }

    pub fn from_json(json: &str) -> Result<Self, Error> {
        serde_json::from_str(json).map_err(|error| Error::Persistence {
            message: format!("failed to deserialize session snapshot: {error}"),
        })
    }
}

impl Serialize for SessionSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SessionSnapshotWire {
            schema_version: SESSION_SNAPSHOT_SCHEMA_VERSION,
            session_id: self.session_id.clone(),
            revision: self.revision,
            history: sanitized_history(self.history.clone()),
            provider: self.provider.clone(),
            compaction: self.compaction.clone(),
            metadata: self.metadata.clone(),
            prompt_cache_key: self.prompt_cache_key.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SessionSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = SessionSnapshotWire::deserialize(deserializer)?;
        if !(MIN_SESSION_SNAPSHOT_SCHEMA_VERSION..=SESSION_SNAPSHOT_SCHEMA_VERSION)
            .contains(&wire.schema_version)
        {
            return Err(D::Error::custom(format!(
                "unsupported session snapshot schema {}; supported versions are {} through {}",
                wire.schema_version,
                MIN_SESSION_SNAPSHOT_SCHEMA_VERSION,
                SESSION_SNAPSHOT_SCHEMA_VERSION
            )));
        }
        Ok(Self {
            schema_version: SESSION_SNAPSHOT_SCHEMA_VERSION,
            session_id: wire.session_id,
            revision: wire.revision,
            history: sanitized_history(wire.history),
            provider: wire.provider,
            compaction: wire.compaction,
            metadata: wire.metadata,
            prompt_cache_key: wire.prompt_cache_key,
        })
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

/// Future returned by [`SessionStore`] operations.
pub type SessionStoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

/// Storage boundary for complete, versioned session snapshots.
///
/// A successful save atomically replaces the complete prior snapshot for the
/// session ID. A failed save must leave the prior snapshot loadable. Stores do
/// not support concurrent writers to one session unless they document stronger
/// revision-conflict behavior.
pub trait SessionStore: Send + Sync {
    fn load<'a>(&'a self, id: &'a SessionId) -> SessionStoreFuture<'a, Option<SessionSnapshot>>;

    fn save<'a>(&'a self, snapshot: SessionSnapshot) -> SessionStoreFuture<'a, ()>;
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

impl SessionStore for InMemorySessionStore {
    fn load<'a>(&'a self, id: &'a SessionId) -> SessionStoreFuture<'a, Option<SessionSnapshot>> {
        Box::pin(async move { Ok(InMemorySessionStore::load(self, id)) })
    }

    fn save<'a>(&'a self, snapshot: SessionSnapshot) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            InMemorySessionStore::save(self, snapshot);
            Ok(())
        })
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
