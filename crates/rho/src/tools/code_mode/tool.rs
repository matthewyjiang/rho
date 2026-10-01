//! `codemode` Tool: model writes Starlark; nested calls go through ToolHost (native + MCP).
//!
//! Nested approvals pause the script via shared ToolHost session approvals — see
//! [`super::bridge`] and `docs/design/code-mode-starlark-v0.md`.

use std::collections::BTreeSet;
use std::sync::Arc;

use rho_sdk::model::ToolSpec;
use rho_sdk::tool::{
    Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolOutput,
};
use serde_json::json;

use super::bridge::{GuardedBridge, ToolHostBridge, CODEMODE_TOOL_NAME};
use super::engine::{evaluate_code_mode, format_engine_output, EngineLimits};

/// Starlark code-mode tool (`codemode`).
///
/// TODOs toward ship (not all required for this prototype PR):
/// - wire shared ApprovalSession when registering into a live run
/// - tool search / deferred (see [`super::search`])
/// - `/codemode on|yolo|off` toggle + config lock
/// - planner-aware parallel (after sequential v0)
/// - default coding-tool registry entry + TUI nested cards
pub struct CodeModeTool {
    bridge: Arc<GuardedBridge>,
    limits: EngineLimits,
}

impl CodeModeTool {
    /// Build against a live [`rho_sdk::ToolHost`].
    ///
    /// The host **must** share the parent session approval handler/session so
    /// nested gated tools pause the script under the same yolo/auto/bypass
    /// knobs as a direct call (no double-prompt).
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
            name: CODEMODE_TOOL_NAME.into(),
            description: "Run a Starlark script that composes ToolHost tools (native and MCP) \
via sequential call_tool(name, args). Only the script's distilled result returns to the model; \
nested tool payloads stay on the host/TUI path. Gated nested tools pause this script until \
the same session approval knobs as a direct call resolve (approve → continue, deny → error)."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "Starlark body. Use call_tool(name, args) for nested tools \
(including MCP), sequentially. Assign `result = ...` for the distilled return. print() is captured."
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
            // Starlark is sync; nested ToolHost::invoke (incl. approval waits)
            // runs under block_in_place on this async tool call — outer codemode
            // stays the in-flight agent turn until the script finishes or errors.
            let output = tokio::task::block_in_place(|| {
                evaluate_code_mode(&script, bridge, limits)
            })
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            Ok(ToolOutput::text(format_engine_output(&output)))
        })
    }
}
