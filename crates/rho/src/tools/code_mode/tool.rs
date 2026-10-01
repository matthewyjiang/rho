//! `code_mode` Tool: model writes Starlark; nested calls go through ToolHost (native + MCP).

use std::collections::BTreeSet;
use std::sync::Arc;

use rho_sdk::model::ToolSpec;
use rho_sdk::tool::{
    Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolOutput,
};
use serde_json::json;

use super::bridge::{GuardedBridge, ToolHostBridge, CODE_MODE_TOOL_NAME};
use super::engine::{evaluate_code_mode, format_engine_output, EngineLimits};

/// Starlark code-mode tool.
///
/// TODOs (not in v0):
/// - nested approval UX / batching
/// - session mode toggle (on / write-locked / yolo)
/// - planner-aware parallel (`parallel([...])` host helper)
/// - wire into the default coding tool registry (currently opt-in constructor only)
pub struct CodeModeTool {
    bridge: Arc<GuardedBridge>,
    limits: EngineLimits,
}

impl CodeModeTool {
    /// Build against a live [`rho_sdk::ToolHost`] (MCP tools work when registered on that host).
    #[allow(dead_code)]
    pub fn with_tool_host(
        host: Arc<rho_sdk::ToolHost>,
        allowlist: Option<BTreeSet<String>>,
        max_nested_calls: usize,
    ) -> Self {
        let inner = Arc::new(ToolHostBridge::new(host));
        Self {
            bridge: Arc::new(GuardedBridge::new(inner, allowlist, max_nested_calls)),
            limits: EngineLimits::default(),
        }
    }

    /// Test / custom bridge constructor.
    pub fn with_bridge(bridge: Arc<GuardedBridge>, limits: EngineLimits) -> Self {
        Self { bridge, limits }
    }
}

impl Tool for CodeModeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: CODE_MODE_TOOL_NAME.into(),
            description: "Run a Starlark script that composes ToolHost tools (native and MCP) \
via call_tool(name, args). Only the script's distilled result returns to the model; \
nested tool payloads stay on the host/TUI path."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "Starlark body. Use call_tool(name, args) for nested tools \
(including MCP). Assign `result = ...` for the distilled return. print() is captured."
                    }
                },
                "required": ["script"]
            }),
        }
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        let script = invocation
            .arguments()
            .get("script")
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        let bridge = Arc::clone(&self.bridge);
        let limits = self.limits.clone();
        Box::pin(async move {
            if context.cancellation().is_cancelled() {
                return Err(ToolError::cancelled());
            }
            let script = script.ok_or_else(|| {
                ToolError::new(ToolErrorKind::InvalidArguments, "missing script argument")
            })?;
            let output = tokio::task::block_in_place(|| {
                evaluate_code_mode(&script, bridge, limits)
            })
            .map_err(|error| {
                ToolError::new(ToolErrorKind::Execution, error.to_string())
            })?;
            Ok(ToolOutput::text(format_engine_output(&output)))
        })
    }
}
