//! ToolHost bridge for Starlark code-mode.
//!
//! Nested calls (native **or MCP-backed**) go through [`CodeModeBridge`] so
//! policy, approvals, and hooks stay on ToolHost. There is no in-guest MCP client.
//!
//! # Nested approvals (locked model)
//!
//! Production hosts must build [`ToolHostBridge`] from a [`ToolHost`] that
//! **shares** the parent run's [`rho_sdk::ApprovalHandler`] /
//! [`rho_sdk::ApprovalSession`] (`ToolHostBuilder::approval_handler_shared` or
//! `approval_session`). Then:
//!
//! - A gated nested `call_tool` **blocks** inside `ToolHost::invoke` until the
//!   session handler returns Allow*/Deny (or the run is cancelled / times out).
//! - That pauses the Starlark thread (outer `codemode` stays in-flight).
//! - Deny surfaces as [`BridgeError::Host`] / policy denial into the script.
//! - There is **no** second “approve codemode” prompt and no parallel approval
//!   system — nested calls reuse the same yolo/auto/bypass/allowlist knobs as a
//!   direct call of that tool.
//!
//! See `docs/design/code-mode-starlark-v0.md`.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::exposure::ExposureController;

use async_trait::async_trait;
use rho_sdk::tool::ToolOutput;
use rho_sdk::{Error as SdkError, ToolHost, ToolHostCall};
use serde_json::Value;
use thiserror::Error;

/// Model-facing tool name (aligns with future `/codemode on|yolo|off`).
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

/// Production bridge: every nested call is `ToolHost::invoke`
/// (MCP tools included when registered on the host).
///
/// Construct the host with the **parent session's** approval handler/session so
/// nested gating reuses yolo/auto/bypass/allowlist without a second prompt.
#[allow(dead_code)]
pub struct ToolHostBridge {
    host: Arc<ToolHost>,
}

impl ToolHostBridge {
    /// `host` should already carry the shared session approval wiring.
    #[allow(dead_code)]
    pub fn new(host: Arc<ToolHost>) -> Self {
        Self { host }
    }
}

#[async_trait]
impl CodeModeBridge for ToolHostBridge {
    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<ToolOutput, BridgeError> {
        // Blocks until ToolHost finishes — including any nested ApprovalHandler
        // wait. Fuel/timeout/cancel must come from the ToolHost / run token so
        // we do not hang forever on an unanswered approval prompt.
        match self.host.invoke(ToolHostCall::new(name, arguments)).await {
            Ok(output) => Ok(output),
            Err(error) => {
                let message = error.to_string();
                if message_looks_like_policy_deny(&message) {
                    Err(BridgeError::NestedDenied {
                        name: name.to_owned(),
                        reason: message,
                    })
                } else {
                    Err(BridgeError::Host(error))
                }
            }
        }
    }
}

fn message_looks_like_policy_deny(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("denied") || lower.contains("policy") || lower.contains("approval")
}
