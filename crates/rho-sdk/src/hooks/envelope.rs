use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use crate::{HookEventId, RunId, SessionId};

use super::{
    bounds::{HookPayloadBounds, HookTruncation},
    event::HookEventKind,
    payload::{HookPayload, HookWorkspace},
};

/// Wire schema version of [`HookEnvelope`]. Handlers must reject other values.
pub const HOOK_SCHEMA_VERSION: u32 = 1;

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
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HookEnvelope {
    schema_version: u32,
    event: HookEventKind,
    event_id: HookEventId,
    timestamp_unix_ms: u64,
    identity: HookIdentity,
    workspace: HookWorkspace,
    #[serde(rename = "bounds")]
    truncation: HookTruncation,
    payload: HookPayload,
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

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace.root.as_deref()
    }

    pub fn truncation(&self) -> &HookTruncation {
        &self.truncation
    }

    pub fn payload(&self) -> &HookPayload {
        &self.payload
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
    event: HookEventKind,
    identity: HookIdentity,
    workspace: HookWorkspace,
    truncation: HookTruncation,
    timestamp_unix_ms: u64,
}

impl HookEnvelopeBuilder {
    pub(crate) fn new(
        event: HookEventKind,
        identity: HookIdentity,
        workspace_root: Option<&Path>,
    ) -> Self {
        Self {
            event,
            identity,
            workspace: HookWorkspace::from_root(workspace_root),
            truncation: HookTruncation::default(),
            timestamp_unix_ms: now_unix_ms(),
        }
    }

    pub(crate) fn truncation(&mut self) -> &mut HookTruncation {
        &mut self.truncation
    }

    pub(crate) fn finish(self, payload: HookPayload) -> HookEnvelope {
        HookEnvelope {
            schema_version: HOOK_SCHEMA_VERSION,
            event: self.event,
            event_id: HookEventId::new(),
            timestamp_unix_ms: self.timestamp_unix_ms,
            identity: self.identity,
            workspace: self.workspace,
            truncation: self.truncation,
            payload,
        }
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
