//! ToolHost bridge for Starlark code-mode.
//!
//! Nested calls (native **or MCP-backed**) go through [`CodeModeBridge`] so
//! policy, approvals, and hooks stay on ToolHost. There is no in-guest MCP client.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use rho_sdk::tool::ToolOutput;
use rho_sdk::{Error as SdkError, ToolHost, ToolHostCall};
use serde_json::Value;
use thiserror::Error;

/// Stable tool name for this prototype.
pub const CODE_MODE_TOOL_NAME: &str = "code_mode";

/// Errors from the code-mode host bridge (loud, actionable).
#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("code_mode: tool `{name}` is not on the allowlist (v0 loud limit)")]
    NotAllowlisted { name: String },
    #[error("code_mode: refusing recursive invocation of `{name}`")]
    Recursive { name: String },
    #[error("code_mode: nested call limit exceeded (max {max})")]
    CallLimit { max: usize },
    #[error("code_mode: ToolHost error: {0}")]
    Host(#[from] SdkError),
    #[error("code_mode: {0}")]
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
        Self {
            inner,
            allowlist,
            max_calls: max_calls.max(1),
            calls: std::sync::Mutex::new(0),
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
        if trimmed == CODE_MODE_TOOL_NAME {
            return Err(BridgeError::Recursive {
                name: trimmed.to_owned(),
            });
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
        self.inner.invoke_tool(trimmed, arguments).await
    }
}

/// Production bridge: every nested call is `ToolHost::invoke`
/// (MCP tools included when registered on the host).
#[allow(dead_code)]
pub struct ToolHostBridge {
    host: Arc<ToolHost>,
}

impl ToolHostBridge {
    pub fn new(host: Arc<ToolHost>) -> Self {
        Self { host }
    }
}

#[async_trait]
impl CodeModeBridge for ToolHostBridge {
    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<ToolOutput, BridgeError> {
        let output = self
            .host
            .invoke(ToolHostCall::new(name, arguments))
            .await?;
        Ok(output)
    }
}
