//! `codemode` Tool: model writes Starlark; nested calls go through ToolHost (native + MCP).
//!
//! Nested approvals pause the script via shared ToolHost session approvals — see
//! [`super::bridge`] and `docs/design/code-mode-starlark-v0.md`.

use std::sync::Arc;

use rho_sdk::model::ToolSpec;
use rho_sdk::tool::{
    Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolOutput,
};
use serde_json::json;

use super::bridge::{GuardedBridge, ToolHostBridge, CODEMODE_TOOL_NAME};
use super::engine::{evaluate_code_mode_with_exposure, format_engine_output, EngineLimits};
use super::exposure::ExposureController;
use super::nesting::{CodeModeNesting, DEFAULT_MAX_NESTED_CALLS};

/// Starlark code-mode tool (`codemode`).
///
/// Nested calls stream status lines onto this call's progress (see
/// [`ToolHostBridge`]). Planner-aware parallel `call_tool` is deferred.
pub struct CodeModeTool {
    nesting: Arc<CodeModeNesting>,
    exposure: Arc<ExposureController>,
    limits: EngineLimits,
}

impl CodeModeTool {
    /// Live constructor: `nesting` supplies sibling tools; each call builds a
    /// child ToolHost that inherits the parent call's authorization.
    pub fn new(nesting: Arc<CodeModeNesting>, exposure: Arc<ExposureController>) -> Self {
        Self {
            nesting,
            exposure,
            limits: EngineLimits::default(),
        }
    }
}

impl Tool for CodeModeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: CODEMODE_TOOL_NAME.into(),
            description: "Run a Starlark script that composes ToolHost tools (native and MCP) \
via sequential call_tool(name, args). Use search_tools/list_tools inside the script to discover \
MCP tools (default exposure: codemode). Only the script's distilled result returns to the model; \
nested tool payloads stay on the host/TUI path. Nested calls follow the session permission mode \
exactly like direct calls; a gated call pauses this script until approved (deny → error)."
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
        let nesting = Arc::clone(&self.nesting);
        let exposure = Arc::clone(&self.exposure);
        let limits = self.limits.clone();
        Box::pin(async move {
            if context.cancellation().is_cancelled() {
                return Err(ToolError::cancelled());
            }
            let script = script.ok_or_else(|| {
                ToolError::new(ToolErrorKind::InvalidArguments, "missing script argument")
            })?;

            let host = nesting.build_host(&context)?;
            let bridge = Arc::new(GuardedBridge::with_exposure(
                Arc::new(ToolHostBridge::new(Arc::new(host), context.clone())),
                None,
                DEFAULT_MAX_NESTED_CALLS,
                Some(Arc::clone(&exposure)),
            ));

            // Starlark is sync; nested ToolHost::invoke (incl. approval waits)
            // runs under block_in_place on this async tool call — outer codemode
            // stays the in-flight agent turn until the script finishes or errors.
            let output = tokio::task::block_in_place(|| {
                evaluate_code_mode_with_exposure(&script, bridge, limits, Some(exposure))
            })
            .map_err(|error| {
                if context.cancellation().is_cancelled() {
                    ToolError::cancelled()
                } else {
                    ToolError::new(ToolErrorKind::Execution, error.to_string())
                }
            })?;
            Ok(ToolOutput::text(format_engine_output(&output)))
        })
    }
}
