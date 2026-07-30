use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use crate::{RunId, SessionId};

use super::{
    bounds::HookPayloadBounds,
    envelope::{HookEnvelope, HookEnvelopeBuilder, HookIdentity},
    event::HookEventKind,
    gate::{HookDecision, PreToolUseGate, PreToolUseRequest},
    payload::{
        bounded_failure, HookFailure, HookPayload, SessionCompletedPayload, SessionFailedPayload,
    },
};

/// Future returned by a [`HookObserver`].
pub type HookObserveFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Sink for observational lifecycle events.
///
/// The runtime awaits this future on the path that produced the event, so an
/// implementation must hand the envelope to its own bounded queue and return.
/// Doing real work here makes an observational event blocking, which the hook
/// contract does not allow.
pub trait HookObserver: Send + Sync {
    fn observe(&self, envelope: HookEnvelope) -> HookObserveFuture<'_>;
}

impl std::fmt::Debug for dyn HookObserver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HookObserver(..)")
    }
}

/// Identity a delegated runtime reports as its parent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookDelegation {
    parent_session_id: Option<SessionId>,
    parent_run_id: Option<RunId>,
}

impl HookDelegation {
    pub fn new(parent_session_id: SessionId) -> Self {
        Self {
            parent_session_id: Some(parent_session_id),
            parent_run_id: None,
        }
    }

    pub fn parent_run_id(mut self, parent_run_id: RunId) -> Self {
        self.parent_run_id = Some(parent_run_id);
        self
    }
}

/// Hook wiring shared by every session created from one runtime.
#[derive(Clone, Default)]
pub(crate) struct HookRuntime {
    observer: Option<Arc<dyn HookObserver>>,
    gate: Option<Arc<dyn PreToolUseGate>>,
    bounds: HookPayloadBounds,
    delegation: HookDelegation,
}

impl HookRuntime {
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
        }
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
            parent_run_id: self.delegation.parent_run_id.clone(),
        }
    }

    pub(crate) fn builder(
        &self,
        event: HookEventKind,
        session_id: Option<&SessionId>,
        run_id: Option<&RunId>,
        workspace_root: Option<&Path>,
    ) -> HookEnvelopeBuilder {
        HookEnvelopeBuilder::new(event, self.identity(session_id, run_id), workspace_root)
    }

    /// Builds and delivers one observational event.
    ///
    /// `build` runs only when an observer is installed, so runtimes without
    /// hooks pay nothing beyond an `Option` check.
    pub(crate) async fn observe<F>(
        &self,
        event: HookEventKind,
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
        debug_assert!(
            event.is_delivered(),
            "only delivered events reach observers"
        );
        let mut builder = self.builder(event, session_id, run_id, workspace_root);
        let payload = build(&mut builder);
        observer.observe(builder.finish(payload)).await;
    }

    pub(crate) async fn evaluate_pre_tool_use(&self, request: PreToolUseRequest) -> HookDecision {
        match self.gate.as_ref() {
            Some(gate) => gate.evaluate(request).await,
            None => HookDecision::Continue,
        }
    }
}

impl std::fmt::Debug for HookRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HookRuntime")
            .field("observer", &self.observer.is_some())
            .field("gate", &self.gate.is_some())
            .field("bounds", &self.bounds)
            .field("delegation", &self.delegation)
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
    hooks: HookRuntime,
    workspace_root: Option<PathBuf>,
}

impl HookDispatcher {
    pub(crate) fn new(hooks: HookRuntime, workspace_root: Option<PathBuf>) -> Self {
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
    pub async fn session_completed(&self, session_id: &SessionId, runs: u64) {
        self.hooks
            .observe(
                HookEventKind::SessionCompleted,
                Some(session_id),
                None,
                self.workspace_root.as_deref(),
                |_| HookPayload::SessionCompleted(SessionCompletedPayload { runs }),
            )
            .await;
    }

    /// Reports that a session ended because of `reason`.
    pub async fn session_failed(&self, session_id: &SessionId, kind: &str, reason: &str) {
        let bounds = self.hooks.bounds();
        self.hooks
            .observe(
                HookEventKind::SessionFailed,
                Some(session_id),
                None,
                self.workspace_root.as_deref(),
                |builder| {
                    let failure: HookFailure = bounded_failure(
                        kind,
                        reason,
                        bounds,
                        builder.truncation(),
                        "payload.failure.message",
                    );
                    HookPayload::SessionFailed(SessionFailedPayload { failure })
                },
            )
            .await;
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
