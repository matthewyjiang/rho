use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{workspace::CapabilityRequest, RunId, SessionId, ToolCallId};

use super::{
    bounds::HookPayloadBounds,
    envelope::{HookEnvelope, HookEnvelopeBuilder, HookHostLabels, HookIdentity},
    gate::{HookDecision, PreToolUseGate, PreToolUseRequest},
    payload::{
        bounded_failure, summarize_capability, AfterToolUsePayload, BoundedFailure, HookFailure,
        HookPayload, HookTool, HookToolStatus, SessionCompletedPayload, SessionFailedPayload,
    },
};

/// Sink for observational lifecycle events.
///
/// This call runs on the path that produced the event. Implementations must
/// perform only a non-blocking enqueue into their own bounded queue. The
/// synchronous boundary deliberately makes it impossible to await hook work on
/// the agent's path.
pub trait HookObserver: Send + Sync {
    fn observe(&self, envelope: HookEnvelope);
}

impl std::fmt::Debug for dyn HookObserver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HookObserver(..)")
    }
}

/// Identity a delegated runtime reports as its parent session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookDelegation {
    parent_session_id: Option<SessionId>,
}

impl HookDelegation {
    pub fn new(parent_session_id: SessionId) -> Self {
        Self {
            parent_session_id: Some(parent_session_id),
        }
    }
}

/// Hook ports shared by every session created from one runtime.
///
/// This is wiring, not the host pipeline that owns config, workers, and
/// shutdown. Hosts keep that separately and install the gate/observer here.
#[derive(Clone, Default)]
pub(crate) struct HookWiring {
    observer: Option<Arc<dyn HookObserver>>,
    gate: Option<Arc<dyn PreToolUseGate>>,
    bounds: HookPayloadBounds,
    delegation: HookDelegation,
    host_labels: HookHostLabels,
}

impl HookWiring {
    pub(crate) fn new(
        observer: Option<Arc<dyn HookObserver>>,
        gate: Option<Arc<dyn PreToolUseGate>>,
        bounds: HookPayloadBounds,
        delegation: HookDelegation,
    ) -> Self {
        Self {
            observer,
            gate,
            bounds,
            delegation,
            host_labels: HookHostLabels::default(),
        }
    }

    pub(crate) fn with_host_labels(mut self, host_labels: HookHostLabels) -> Self {
        self.host_labels = host_labels;
        self
    }

    pub(crate) fn bounds(&self) -> HookPayloadBounds {
        self.bounds
    }

    pub(crate) fn gate(&self) -> Option<&Arc<dyn PreToolUseGate>> {
        self.gate.as_ref()
    }

    pub(crate) fn observes(&self) -> bool {
        self.observer.is_some()
    }

    pub(crate) fn identity(
        &self,
        session_id: Option<&SessionId>,
        run_id: Option<&RunId>,
    ) -> HookIdentity {
        HookIdentity {
            session_id: session_id.cloned(),
            parent_session_id: self.delegation.parent_session_id.clone(),
            run_id: run_id.cloned(),
        }
    }

    pub(crate) fn builder(
        &self,
        session_id: Option<&SessionId>,
        run_id: Option<&RunId>,
        workspace_root: Option<&Path>,
    ) -> HookEnvelopeBuilder {
        HookEnvelopeBuilder::with_host_labels(
            self.identity(session_id, run_id),
            self.host_labels.clone(),
            workspace_root,
            self.bounds,
        )
    }

    /// Builds and delivers one observational event.
    ///
    /// `build` runs only when an observer is installed, so runtimes without
    /// hooks pay nothing beyond an `Option` check.
    pub(crate) fn observe<F>(
        &self,
        session_id: Option<&SessionId>,
        run_id: Option<&RunId>,
        workspace_root: Option<&Path>,
        build: F,
    ) where
        F: FnOnce(&mut HookEnvelopeBuilder) -> HookPayload,
    {
        let Some(observer) = self.observer.as_ref() else {
            return;
        };
        let mut builder = self.builder(session_id, run_id, workspace_root);
        let payload = build(&mut builder);
        observer.observe(builder.finish(payload));
    }

    pub(crate) async fn evaluate_pre_tool_use(&self, request: PreToolUseRequest) -> HookDecision {
        match self.gate.as_ref() {
            Some(gate) => gate.evaluate(request).await,
            None => HookDecision::Continue,
        }
    }

    pub(crate) fn observe_after_tool_use(
        &self,
        identity: HookToolIdentity<'_>,
        status: HookToolStatus,
        failure: Option<BoundedFailure<'_>>,
        duration_ms: Option<u64>,
        capability: Option<&CapabilityRequest>,
    ) {
        let bounds = self.bounds();
        self.observe(
            identity.session_id,
            identity.run_id,
            identity.workspace_root,
            |builder| {
                let tool = HookTool::new(
                    identity.tool_name,
                    Some(identity.call_id.as_str().to_owned()),
                    bounds,
                    builder.truncation(),
                );
                HookPayload::AfterToolUse(AfterToolUsePayload {
                    tool,
                    capability: capability
                        .map(|request| summarize_capability(request, bounds, builder.truncation())),
                    status,
                    failure: failure
                        .map(|failure| bounded_failure(failure, bounds, builder.truncation())),
                    duration_ms,
                })
            },
        );
    }
}

pub(crate) struct HookToolIdentity<'a> {
    pub(crate) session_id: Option<&'a SessionId>,
    pub(crate) run_id: Option<&'a RunId>,
    pub(crate) workspace_root: Option<&'a Path>,
    pub(crate) tool_name: &'a str,
    pub(crate) call_id: &'a ToolCallId,
}

impl std::fmt::Debug for HookWiring {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HookWiring")
            .field("observer", &self.observer.is_some())
            .field("gate", &self.gate.is_some())
            .field("bounds", &self.bounds)
            .field("delegation", &self.delegation)
            .field("host_labels", &self.host_labels)
            .finish()
    }
}

/// Host-driven dispatcher for the session boundary events the runtime cannot see.
///
/// The runtime dispatches everything inside a session's lifetime. Only the host
/// knows when an interactive session ends, which can be hours after its last
/// run, so the host reports that boundary through this handle.
#[derive(Clone, Debug)]
pub struct HookDispatcher {
    hooks: HookWiring,
    workspace_root: Option<PathBuf>,
}

/// Stable classification for a host-reported session failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookSessionFailureKind {
    RunFailed,
    Provider,
    Other(String),
}

impl HookSessionFailureKind {
    fn wire_name(&self) -> &str {
        match self {
            Self::RunFailed => "run_failed",
            Self::Provider => "provider",
            Self::Other(kind) => kind,
        }
    }
}

impl From<&str> for HookSessionFailureKind {
    fn from(kind: &str) -> Self {
        match kind {
            "run_failed" => Self::RunFailed,
            "provider" => Self::Provider,
            kind => Self::Other(kind.to_owned()),
        }
    }
}

impl HookDispatcher {
    pub(crate) fn new(hooks: HookWiring, workspace_root: Option<PathBuf>) -> Self {
        Self {
            hooks,
            workspace_root,
        }
    }

    /// Whether any hook sink is installed.
    pub fn is_enabled(&self) -> bool {
        self.hooks.observes() || self.hooks.gate().is_some()
    }

    /// Reports that a session ended normally after `runs` completed runs.
    pub fn session_completed(&self, session_id: &SessionId, runs: u64) {
        self.hooks.observe(
            Some(session_id),
            None,
            self.workspace_root.as_deref(),
            |_| HookPayload::SessionCompleted(SessionCompletedPayload { runs }),
        );
    }

    /// Reports that a session ended because of `reason`.
    pub fn session_failed(
        &self,
        session_id: &SessionId,
        kind: HookSessionFailureKind,
        reason: &str,
    ) {
        let bounds = self.hooks.bounds();
        self.hooks.observe(
            Some(session_id),
            None,
            self.workspace_root.as_deref(),
            |builder| {
                let failure: HookFailure = bounded_failure(
                    BoundedFailure {
                        kind: kind.wire_name(),
                        message: reason,
                        field: "payload.failure",
                    },
                    bounds,
                    builder.truncation(),
                );
                HookPayload::SessionFailed(SessionFailedPayload { failure })
            },
        );
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
