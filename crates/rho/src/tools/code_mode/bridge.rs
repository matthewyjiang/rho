//! ToolHost bridge for Starlark code-mode.
//!
//! Nested calls (native **or MCP-backed**) go through [`CodeModeBridge`] so
//! policy, approvals, and hooks stay on ToolHost. There is no in-guest MCP client.
//!
//! # Nested approvals (locked model)
//!
//! Production hosts build [`ToolHostBridge`] from a child [`ToolHost`] made with
//! [`ToolHost::child_builder`] (see [`super::nesting`]), which inherits the
//! parent call's workspace policy, hook gate, and approval session. Then:
//!
//! - A gated nested `call_tool` **blocks** inside `ToolHost::invoke` until the
//!   session handler returns Allow*/Deny (or the run is cancelled / times out).
//! - That pauses the Starlark thread (outer `codemode` stays in-flight).
//! - Deny surfaces as [`BridgeError::NestedDenied`] (typed SDK kind, not text).
//! - There is **no** second “approve codemode” prompt and no parallel approval
//!   system — nested calls follow the session permission mode
//!   (bypass/auto/allow_edits/plan/supervised) exactly like a direct call.
//!
//! See `docs/design/code-mode-starlark-v0.md`.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::exposure::ExposureController;

use async_trait::async_trait;
use rho_sdk::tool::{ToolContext, ToolErrorKind, ToolOutput, ToolProgress};
use rho_sdk::{Error as SdkError, ToolHost, ToolHostCall, ToolHostEvent, ToolHostRun};
use serde_json::Value;
use thiserror::Error;

/// Model-facing tool name (also the `/codemode on|only` command name).
pub const CODEMODE_TOOL_NAME: &str = "codemode";

/// Errors from the code-mode host bridge (loud, actionable).
#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("codemode: tool `{name}` is not on the allowlist (v0 loud limit)")]
    NotAllowlisted { name: String },
    #[error("codemode: refusing recursive invocation of `{name}`")]
    Recursive { name: String },
    #[error("codemode: nested call limit exceeded (max {max})")]
    CallLimit { max: usize },
    #[error("codemode: ToolHost error: {0}")]
    Host(#[from] SdkError),
    #[error("codemode: nested tool `{name}` denied by session policy: {reason}")]
    NestedDenied { name: String, reason: String },
    #[error("codemode: {0}")]
    Message(String),
}

/// Abstraction over ToolHost so unit tests can stub nested tools (including MCP names).
#[async_trait]
pub trait CodeModeBridge: Send + Sync {
    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<ToolOutput, BridgeError>;
}

/// Allowlist + call budget wrapping any [`CodeModeBridge`].
pub struct GuardedBridge {
    inner: Arc<dyn CodeModeBridge>,
    allowlist: Option<BTreeSet<String>>,
    max_calls: usize,
    calls: std::sync::Mutex<usize>,
    exposure: Option<Arc<ExposureController>>,
}

impl GuardedBridge {
    /// `allowlist = None` means all ToolHost-registered names are eligible
    /// (still subject to ToolHost policy). `Some(...)` is a loud v0 gate for
    /// tests / gradual rollout; include MCP tool names the same way as native.
    #[cfg(test)]
    pub fn new(
        inner: Arc<dyn CodeModeBridge>,
        allowlist: Option<BTreeSet<String>>,
        max_calls: usize,
    ) -> Self {
        Self::with_exposure(inner, allowlist, max_calls, None)
    }

    pub fn with_exposure(
        inner: Arc<dyn CodeModeBridge>,
        allowlist: Option<BTreeSet<String>>,
        max_calls: usize,
        exposure: Option<Arc<ExposureController>>,
    ) -> Self {
        Self {
            inner,
            allowlist,
            max_calls: max_calls.max(1),
            calls: std::sync::Mutex::new(0),
            exposure,
        }
    }

    pub fn call_count(&self) -> usize {
        *self.calls.lock().expect("call counter")
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolOutput, BridgeError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(BridgeError::Message("tool name must not be empty".into()));
        }
        if trimmed == CODEMODE_TOOL_NAME {
            return Err(BridgeError::Recursive {
                name: trimmed.to_owned(),
            });
        }
        if let Some(exposure) = &self.exposure {
            if !exposure.is_script_callable(trimmed) {
                return Err(BridgeError::Message(format!(
                    "tool `{trimmed}` is hidden and unreachable from codemode"
                )));
            }
        }
        if let Some(allow) = &self.allowlist {
            if !allow.contains(trimmed) {
                return Err(BridgeError::NotAllowlisted {
                    name: trimmed.to_owned(),
                });
            }
        }
        {
            let mut calls = self.calls.lock().expect("call counter");
            if *calls >= self.max_calls {
                return Err(BridgeError::CallLimit {
                    max: self.max_calls,
                });
            }
            *calls += 1;
        }
        // Sequential v0: one nested invoke at a time from the Starlark thread.
        // When `inner` is ToolHostBridge with a shared ApprovalSession, a gated
        // tool blocks here until approve/deny/cancel — pausing the whole script.
        self.inner.invoke_tool(trimmed, arguments).await
    }
}

/// Production bridge: every nested call runs on a child [`ToolHost`]
/// (MCP tools included when registered on the host).
///
/// Build the host with [`ToolHost::child_builder`] so nested gating reuses the
/// parent's policy, hooks, and approvals without a second prompt. The bridge
/// also keeps nested calls visible and interruptible from the parent call:
/// - nested progress and per-call status go to the parent's progress stream,
///   so the `codemode` card shows each nested call while the script runs
/// - nested host-input requests are relayed through the parent call
/// - cancelling the parent call cancels the in-flight nested call
pub struct ToolHostBridge {
    host: Arc<ToolHost>,
    parent: ToolContext,
    log: tokio::sync::Mutex<NestedCallLog>,
}

impl ToolHostBridge {
    /// `host` should already carry the parent call's authorization.
    pub fn new(host: Arc<ToolHost>, parent: ToolContext) -> Self {
        Self {
            host,
            parent,
            log: tokio::sync::Mutex::default(),
        }
    }

    /// Records one nested call's state and republishes the whole log.
    ///
    /// Progress replaces a card's body, so each update carries every nested
    /// call so far. The log is bounded by the nested call budget.
    async fn report(&self, index: usize, state: NestedCallState) {
        let mut log = self.log.lock().await;
        log.set(index, state);
        let _ = self
            .parent
            .progress()
            .send(ToolProgress::message(log.render()))
            .await;
    }

    async fn run_nested(&self, mut run: ToolHostRun, index: usize) -> Result<ToolOutput, SdkError> {
        let parent_cancellation = self.parent.cancellation().clone();
        loop {
            tokio::select! {
                () = parent_cancellation.cancelled() => {
                    run.cancel();
                    return run.outcome().await;
                }
                event = run.next_event() => match event {
                    Some(ToolHostEvent::Progress(progress)) => {
                        self.report(index, NestedCallState::Running(progress.text().to_owned()))
                            .await;
                    }
                    Some(ToolHostEvent::HostInputRequested(mut pending)) => {
                        // Dropping `pending` unanswered fails the nested call.
                        if let Ok(response) =
                            self.parent.request_host_input(pending.request().clone()).await
                        {
                            let _ = pending.respond(response);
                        }
                    }
                    // Future event kinds have no parent mapping yet.
                    Some(_) => {}
                    None => return run.outcome().await,
                },
            }
        }
    }
}

#[async_trait]
impl CodeModeBridge for ToolHostBridge {
    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<ToolOutput, BridgeError> {
        // Blocks until the nested call finishes — including any approval wait
        // on the inherited session handler. Parent cancellation interrupts it.
        let run = self.host.start(ToolHostCall::new(name, arguments))?;
        let index = self.log.lock().await.start(name);
        self.report(index, NestedCallState::Running(String::new()))
            .await;
        match self.run_nested(run, index).await {
            Ok(output) => {
                self.report(index, NestedCallState::Succeeded).await;
                Ok(output)
            }
            Err(error) => {
                self.report(index, NestedCallState::Failed).await;
                Err(BridgeError::from_nested(name, error))
            }
        }
    }
}

impl BridgeError {
    /// Classifies a nested failure by its typed SDK kind, never by message text.
    pub(super) fn from_nested(name: &str, error: SdkError) -> Self {
        match error {
            SdkError::Tool(tool) if tool.kind() == ToolErrorKind::PolicyDenied => {
                Self::NestedDenied {
                    name: name.to_owned(),
                    reason: tool.message().to_owned(),
                }
            }
            SdkError::PolicyDenied { message } => Self::NestedDenied {
                name: name.to_owned(),
                reason: message,
            },
            other => Self::Host(other),
        }
    }
}

enum NestedCallState {
    Running(String),
    Succeeded,
    Failed,
}

/// One line per nested call, in call order.
#[derive(Default)]
struct NestedCallLog {
    calls: Vec<(String, NestedCallState)>,
}

impl NestedCallLog {
    fn start(&mut self, name: &str) -> usize {
        self.calls
            .push((name.to_owned(), NestedCallState::Running(String::new())));
        self.calls.len() - 1
    }

    fn set(&mut self, index: usize, state: NestedCallState) {
        if let Some((_, slot)) = self.calls.get_mut(index) {
            *slot = state;
        }
    }

    fn render(&self) -> String {
        self.calls
            .iter()
            .map(|(name, state)| match state {
                NestedCallState::Running(detail) if detail.trim().is_empty() => {
                    format!("{name}: running")
                }
                NestedCallState::Running(detail) => format!("{name}: {}", detail.trim()),
                NestedCallState::Succeeded => format!("{name}: done"),
                NestedCallState::Failed => format!("{name}: failed"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
