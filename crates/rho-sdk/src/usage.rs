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

/// Shared recorder and bounded diagnostics for every model request owned by a host.
#[derive(Clone, Default)]
pub struct ProviderRequestUsageRecording {
    recorder: Option<std::sync::Arc<dyn ProviderRequestUsageRecorder>>,
    diagnostics: std::sync::Arc<UsageRecorderDiagnostics>,
}

impl ProviderRequestUsageRecording {
    pub fn new<R>(recorder: R) -> Self
    where
        R: ProviderRequestUsageRecorder + 'static,
    {
        Self::new_shared(std::sync::Arc::new(recorder))
    }

    pub fn new_shared(recorder: std::sync::Arc<dyn ProviderRequestUsageRecorder>) -> Self {
        Self {
            recorder: Some(recorder),
            diagnostics: std::sync::Arc::default(),
        }
    }

    pub async fn record(&self, event: ProviderRequestUsageEvent) {
        let Some(recorder) = &self.recorder else {
            return;
        };
        if let Err(error) = recorder.record(event).await {
            self.diagnostics.push(error);
        }
    }

    pub fn diagnostics(&self) -> Vec<UsageRecorderDiagnostic> {
        self.diagnostics.snapshot()
    }

    pub fn is_enabled(&self) -> bool {
        self.recorder.is_some()
    }
}

impl std::fmt::Debug for ProviderRequestUsageRecording {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderRequestUsageRecording")
            .field("enabled", &self.is_enabled())
            .finish_non_exhaustive()
    }
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
    session_id: Option<SessionId>,
    parent_session_id: Option<SessionId>,
    run_id: Option<RunId>,
    step_index: Option<usize>,
    attempt_index: Option<usize>,
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
            session_id: Some(session_id),
            parent_session_id: None,
            run_id: Some(run_id),
            step_index: Some(step_index),
            attempt_index: Some(attempt_index),
            workspace_path,
            purpose,
        }
    }

    /// Creates context for a model request outside the agent loop.
    pub fn for_purpose(identity: ModelIdentity, purpose: impl Into<String>) -> Self {
        Self {
            identity,
            session_id: None,
            parent_session_id: None,
            run_id: None,
            step_index: None,
            attempt_index: None,
            workspace_path: None,
            purpose: purpose.into(),
        }
    }

    pub fn with_session_id(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    pub fn with_parent_session_id(mut self, parent_session_id: SessionId) -> Self {
        self.parent_session_id = Some(parent_session_id);
        self
    }

    pub fn with_run_id(mut self, run_id: RunId) -> Self {
        self.run_id = Some(run_id);
        self
    }

    pub fn with_step_index(mut self, step_index: usize) -> Self {
        self.step_index = Some(step_index);
        self
    }

    pub fn with_attempt_index(mut self, attempt_index: usize) -> Self {
        self.attempt_index = Some(attempt_index);
        self
    }

    pub fn with_workspace_path(mut self, workspace_path: impl Into<PathBuf>) -> Self {
        self.workspace_path = Some(workspace_path.into());
        self
    }

    pub fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    pub fn parent_session_id(&self) -> Option<&SessionId> {
        self.parent_session_id.as_ref()
    }

    pub fn run_id(&self) -> Option<&RunId> {
        self.run_id.as_ref()
    }

    /// One-based agent loop step containing this request, when applicable.
    pub fn step_index(&self) -> Option<usize> {
        self.step_index
    }

    /// One-based physical request attempt within the step, when applicable.
    pub fn attempt_index(&self) -> Option<usize> {
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
    pub fn observed(
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
