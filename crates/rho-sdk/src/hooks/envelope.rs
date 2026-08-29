use std::{
    collections::BTreeMap,
    fmt,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{ser::SerializeStruct, Serialize, Serializer};

use crate::{HookEventId, RunId, SessionId};

use super::{
    bounds::{bounded_string, HookPayloadBounds, HookTruncation},
    event::HookEventKind,
    payload::{
        AfterToolUsePayload, HookCapability, HookFailure, HookPayload, HookTool, HookToolStatus,
        HookWorkspace,
    },
};

/// Wire schema version of [`HookEnvelope`]. Handlers must reject other values.
pub const HOOK_SCHEMA_VERSION: u32 = 2;

/// Generic, non-secret labels supplied by the embedding host.
///
/// Labels let a host attribute capability events to its own bounded execution
/// context without adding host-specific model types to the SDK. Keys and values
/// are shortened to the configured hook field bound when an envelope is built.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct HookHostLabels {
    labels: BTreeMap<String, String>,
}

impl HookHostLabels {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one label. Do not use labels for prompts, credentials, environment
    /// values, or tool output.
    pub fn label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.labels.get(key).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.labels
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }
}

impl fmt::Debug for HookHostLabels {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HookHostLabels")
            .field("keys", &self.labels.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Session and run identity, including delegated parent session.
///
/// A delegated Rho subagent reports its own ids plus the session that
/// delegated to it, so a hook can attribute nested work.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HookIdentity {
    pub session_id: Option<SessionId>,
    pub parent_session_id: Option<SessionId>,
    pub run_id: Option<RunId>,
}

/// One typed lifecycle event delivered to a hook handler.
///
/// The envelope is a serialization contract: the runtime writes it, handlers
/// read it. Everything a handler needs to attribute and bound the event lives
/// here, and nothing secret does.
///
/// # Next major
///
/// NEXT_MAJOR(rho-sdk): move `after_tool_use` capability onto
/// [`AfterToolUsePayload`] as a public field and remove
/// [`Self::after_tool_use_capability`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookEnvelope {
    schema_version: u32,
    event: HookEventKind,
    event_id: HookEventId,
    timestamp_unix_ms: u64,
    identity: HookIdentity,
    host_labels: HookHostLabels,
    workspace: HookWorkspace,
    truncation: HookTruncation,
    payload: HookPayload,
    /// First capability an `after_tool_use` call passed to authorize.
    ///
    /// Stored beside the exhaustive payload so this minor does not add a field
    /// to [`AfterToolUsePayload`]. Serialized as `payload.capability`.
    after_tool_use_capability: Option<HookCapability>,
}

impl HookEnvelope {
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn event(&self) -> HookEventKind {
        self.event
    }

    pub fn event_id(&self) -> &HookEventId {
        &self.event_id
    }

    pub fn timestamp_unix_ms(&self) -> u64 {
        self.timestamp_unix_ms
    }

    pub fn identity(&self) -> &HookIdentity {
        &self.identity
    }

    pub fn host_labels(&self) -> &HookHostLabels {
        &self.host_labels
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace.root.as_deref()
    }

    pub fn truncation(&self) -> &HookTruncation {
        &self.truncation
    }

    pub fn payload(&self) -> &HookPayload {
        &self.payload
    }

    /// First capability an `after_tool_use` call passed to authorize.
    ///
    /// `None` when this event is not `after_tool_use`, or when the call never
    /// authorized. Prefer this accessor until the next major folds the field
    /// into [`AfterToolUsePayload`].
    pub fn after_tool_use_capability(&self) -> Option<&HookCapability> {
        match &self.payload {
            HookPayload::AfterToolUse(_) => self.after_tool_use_capability.as_ref(),
            _ => None,
        }
    }

    /// Serializes the envelope, refusing to emit one larger than `bounds`.
    ///
    /// Field-level truncation happens while the payload is built; this is the
    /// final backstop so a handler's stdin is bounded even when many small
    /// fields add up.
    pub fn to_bounded_json(&self, bounds: HookPayloadBounds) -> Result<String, HookEnvelopeError> {
        let encoded = serde_json::to_string(self).map_err(HookEnvelopeError::Serialization)?;
        if encoded.len() > bounds.max_envelope_bytes() {
            return Err(HookEnvelopeError::TooLarge(HookEnvelopeTooLarge {
                event: self.event,
                size: encoded.len(),
                limit: bounds.max_envelope_bytes(),
            }));
        }
        Ok(encoded)
    }
}

#[derive(Serialize)]
struct AfterToolUseWire<'a> {
    tool: &'a HookTool,
    capability: Option<&'a HookCapability>,
    status: HookToolStatus,
    failure: &'a Option<HookFailure>,
    duration_ms: Option<u64>,
}

impl Serialize for HookEnvelope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("HookEnvelope", 9)?;
        state.serialize_field("schema_version", &self.schema_version)?;
        state.serialize_field("event", &self.event)?;
        state.serialize_field("event_id", &self.event_id)?;
        state.serialize_field("timestamp_unix_ms", &self.timestamp_unix_ms)?;
        state.serialize_field("identity", &self.identity)?;
        state.serialize_field("host_labels", &self.host_labels)?;
        state.serialize_field("workspace", &self.workspace)?;
        state.serialize_field("bounds", &self.truncation)?;
        match &self.payload {
            HookPayload::AfterToolUse(payload) => state.serialize_field(
                "payload",
                &AfterToolUseWire {
                    tool: &payload.tool,
                    capability: self.after_tool_use_capability.as_ref(),
                    status: payload.status,
                    failure: &payload.failure,
                    duration_ms: payload.duration_ms,
                },
            )?,
            other => state.serialize_field("payload", other)?,
        }
        state.end()
    }
}

/// An envelope could not be delivered within its size bound.
///
/// Blocking dispatch treats this as an infrastructure failure and denies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookEnvelopeTooLarge {
    event: HookEventKind,
    size: usize,
    limit: usize,
}

/// Failure to encode an envelope or fit it within its configured bound.
#[derive(Debug)]
pub enum HookEnvelopeError {
    Serialization(serde_json::Error),
    TooLarge(HookEnvelopeTooLarge),
}

impl std::fmt::Display for HookEnvelopeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialization(error) => {
                write!(formatter, "hook envelope serialization failed: {error}")
            }
            Self::TooLarge(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for HookEnvelopeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialization(error) => Some(error),
            Self::TooLarge(error) => Some(error),
        }
    }
}

impl HookEnvelopeTooLarge {
    pub fn event(&self) -> HookEventKind {
        self.event
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn limit(&self) -> usize {
        self.limit
    }
}

impl std::fmt::Display for HookEnvelopeTooLarge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "hook event '{}' was not delivered: {} bytes exceeds the {} byte limit",
            self.event, self.size, self.limit
        )
    }
}

impl std::error::Error for HookEnvelopeTooLarge {}

/// Assembles one envelope with its identity, clock reading, and bounds report.
pub struct HookEnvelopeBuilder {
    identity: HookIdentity,
    host_labels: HookHostLabels,
    workspace: HookWorkspace,
    truncation: HookTruncation,
    bounds: HookPayloadBounds,
    timestamp_unix_ms: u64,
}

impl HookEnvelopeBuilder {
    pub(crate) fn new(
        identity: HookIdentity,
        workspace_root: Option<&Path>,
        bounds: HookPayloadBounds,
    ) -> Self {
        Self::with_host_labels(identity, HookHostLabels::default(), workspace_root, bounds)
    }

    pub(crate) fn with_host_labels(
        identity: HookIdentity,
        host_labels: HookHostLabels,
        workspace_root: Option<&Path>,
        bounds: HookPayloadBounds,
    ) -> Self {
        let mut truncation = HookTruncation::default();
        let identity = bounded_identity(identity, bounds, &mut truncation);
        let host_labels = bounded_host_labels(host_labels, bounds, &mut truncation);
        let workspace = HookWorkspace::from_root(workspace_root, bounds, &mut truncation);
        Self {
            identity,
            host_labels,
            workspace,
            truncation,
            bounds,
            timestamp_unix_ms: now_unix_ms(),
        }
    }

    pub(crate) fn truncation(&mut self) -> &mut HookTruncation {
        &mut self.truncation
    }

    pub(crate) fn bounded_string(&mut self, value: impl Into<String>, field: &str) -> String {
        bounded_string(value, field, self.bounds, &mut self.truncation)
    }

    pub(crate) fn finish(self, payload: HookPayload) -> HookEnvelope {
        self.assemble(payload, None)
    }

    pub(crate) fn finish_after_tool_use(
        self,
        payload: AfterToolUsePayload,
        capability: Option<HookCapability>,
    ) -> HookEnvelope {
        self.assemble(HookPayload::AfterToolUse(payload), capability)
    }

    fn assemble(
        self,
        payload: HookPayload,
        after_tool_use_capability: Option<HookCapability>,
    ) -> HookEnvelope {
        HookEnvelope {
            schema_version: HOOK_SCHEMA_VERSION,
            event: payload.event(),
            event_id: HookEventId::new(),
            timestamp_unix_ms: self.timestamp_unix_ms,
            identity: self.identity,
            host_labels: self.host_labels,
            workspace: self.workspace,
            truncation: self.truncation,
            payload,
            after_tool_use_capability,
        }
    }
}

fn bounded_host_labels(
    labels: HookHostLabels,
    bounds: HookPayloadBounds,
    truncation: &mut HookTruncation,
) -> HookHostLabels {
    let mut bounded = BTreeMap::new();
    let mut bytes = 0usize;
    for (index, (key, value)) in labels.labels.into_iter().enumerate() {
        let key_field = format!("host_labels.keys[{index}]");
        let key = bounded_string(key, &key_field, bounds, truncation);
        let value_field = format!("host_labels.{key}");
        let value = bounded_string(value, &value_field, bounds, truncation);
        if bytes.saturating_add(key.len()).saturating_add(value.len()) > bounds.max_envelope_bytes()
        {
            truncation.record("host_labels");
            break;
        }
        bytes = bytes.saturating_add(key.len()).saturating_add(value.len());
        match bounded.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(value);
            }
            std::collections::btree_map::Entry::Occupied(_) => truncation.record(key_field),
        }
    }
    HookHostLabels { labels: bounded }
}

fn bounded_identity(
    identity: HookIdentity,
    bounds: HookPayloadBounds,
    truncation: &mut HookTruncation,
) -> HookIdentity {
    HookIdentity {
        session_id: identity.session_id.map(|id| {
            let bounded = bounded_string(id.as_str(), "identity.session_id", bounds, truncation);
            SessionId::from_string(bounded)
                .unwrap_or_else(|_| SessionId::from_string("_").unwrap_or(id))
        }),
        parent_session_id: identity.parent_session_id.map(|id| {
            let bounded = bounded_string(
                id.as_str(),
                "identity.parent_session_id",
                bounds,
                truncation,
            );
            SessionId::from_string(bounded)
                .unwrap_or_else(|_| SessionId::from_string("_").unwrap_or(id))
        }),
        run_id: identity.run_id.map(|id| {
            let bounded = bounded_string(id.as_str(), "identity.run_id", bounds, truncation);
            RunId::from_string(bounded).unwrap_or_else(|_| RunId::from_string("_").unwrap_or(id))
        }),
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "envelope_tests.rs"]
mod tests;
