use std::{
    collections::VecDeque,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    model::{ModelIdentity, ModelUsage},
    ProviderErrorKind, RunId, SessionId,
};

const MAX_DIAGNOSTICS: usize = 16;
const MAX_DIAGNOSTIC_BYTES: usize = 1024;

/// Future returned by a [`ProviderRequestUsageRecorder`].
pub type ProviderRequestUsageRecorderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), ProviderRequestUsageRecorderError>> + Send + 'a>>;

/// Records normalized usage for each physical request sent to an agent provider.
///
/// Implementations should durably record an event before resolving the future.
/// Recorder failures are retained as bounded runtime diagnostics and never fail
/// or cancel the agent run.
pub trait ProviderRequestUsageRecorder: Send + Sync {
    fn record(&self, event: ProviderRequestUsageEvent) -> ProviderRequestUsageRecorderFuture<'_>;
}

/// A bounded failure returned by a usage recorder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRequestUsageRecorderError {
    message: String,
}

impl ProviderRequestUsageRecorderError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: truncate(message.into(), MAX_DIAGNOSTIC_BYTES),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ProviderRequestUsageRecorderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProviderRequestUsageRecorderError {}

/// Immutable identifying context for one physical provider request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRequestUsageContext {
    identity: ModelIdentity,
    session_id: SessionId,
    run_id: RunId,
    step_index: usize,
    attempt_index: usize,
    workspace_path: Option<PathBuf>,
    purpose: String,
}

impl ProviderRequestUsageContext {
    pub(crate) fn new(
        identity: ModelIdentity,
        session_id: SessionId,
        run_id: RunId,
        step_index: usize,
        attempt_index: usize,
        workspace_path: Option<PathBuf>,
        purpose: String,
    ) -> Self {
        Self {
            identity,
            session_id,
            run_id,
            step_index,
            attempt_index,
            workspace_path,
            purpose,
        }
    }

    pub fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// One-based agent loop step containing this request.
    pub fn step_index(&self) -> usize {
        self.step_index
    }

    /// One-based physical request attempt within the step.
    pub fn attempt_index(&self) -> usize {
        self.attempt_index
    }

    pub fn workspace_path(&self) -> Option<&Path> {
        self.workspace_path.as_deref()
    }

    pub fn purpose(&self) -> &str {
        &self.purpose
    }
}

/// Terminal classification of one physical provider request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProviderRequestOutcome {
    Completed,
    InvalidResponse,
    Failed(ProviderErrorKind),
    Cancelled,
}

/// Usage observed while executing one physical provider request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRequestUsageEvent {
    event_id: String,
    timestamp_utc_ms: u64,
    context: ProviderRequestUsageContext,
    usage: ModelUsage,
    outcome: ProviderRequestOutcome,
}

impl ProviderRequestUsageEvent {
    pub(crate) fn observed(
        context: ProviderRequestUsageContext,
        usage: ModelUsage,
        outcome: ProviderRequestOutcome,
    ) -> Self {
        let timestamp_utc_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        Self {
            event_id: uuid::Uuid::new_v4().to_string(),
            timestamp_utc_ms,
            context,
            usage,
            outcome,
        }
    }

    /// Random stable identifier generated once for this event.
    pub fn event_id(&self) -> &str {
        &self.event_id
    }

    pub fn timestamp_utc_ms(&self) -> u64 {
        self.timestamp_utc_ms
    }

    pub fn context(&self) -> &ProviderRequestUsageContext {
        &self.context
    }

    pub fn usage(&self) -> &ModelUsage {
        &self.usage
    }

    pub fn outcome(&self) -> ProviderRequestOutcome {
        self.outcome
    }
}

/// A bounded diagnostic produced when usage persistence fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageRecorderDiagnostic {
    message: String,
}

impl UsageRecorderDiagnostic {
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Default)]
pub(crate) struct UsageRecorderDiagnostics {
    entries: Mutex<VecDeque<UsageRecorderDiagnostic>>,
}

impl UsageRecorderDiagnostics {
    pub(crate) fn push(&self, error: ProviderRequestUsageRecorderError) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if entries.len() == MAX_DIAGNOSTICS {
            entries.pop_front();
        }
        entries.push_back(UsageRecorderDiagnostic {
            message: truncate(error.message, MAX_DIAGNOSTIC_BYTES),
        });
    }

    pub(crate) fn snapshot(&self) -> Vec<UsageRecorderDiagnostic> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

fn truncate(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
