//! The model-facing Starlark tool. Scripts run off the async event-draining task.

use std::sync::Arc;

use rho_sdk::model::ToolSpec;
use rho_sdk::tool::{Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation};
use serde::Deserialize;
use serde_json::json;

use super::bridge::{ToolHostBridge, CODEMODE_TOOL_NAME};
use super::engine::{evaluate_code_mode, format_engine_output, EngineLimits, EngineOutput};
use super::exposure::CodeModeSurface;

/// Model-facing contract. Batching guidance lives here and in the system
/// prompt so models fan out independent calls instead of issuing them in turn.
const DESCRIPTION: &str = "\
Run a Starlark (Python-like) script that calls other tools. Only the script's output reaches you, \
so use it to batch independent calls, chain dependent ones, and filter large results.
Tool calls return native Starlark dictionaries with string keys \"is_error\", \"content\", and \
\"data\". Use bracket indexing, not dot access. JSON objects in data and discovery entries \
are also dictionaries; JSON arrays are lists.
Example:
for response in call_tools([(\"read_file\", {\"path\": \"a.rs\"}), (\"grep\", {\"pattern\": \"todo\"})]):
    print(response[\"content\"])
- `call_tools([(name, args), ...])` runs independent calls concurrently and returns their results \
in order. Prefer it whenever calls do not depend on each other.
- `call_tool(name, args)` runs one call using the exact tool name and an argument dictionary. \
There is no `tools` namespace.
- Each result dictionary contains `\"is_error\"`, `\"content\"` (the text you would see), and \
`\"data\"` (the tool's structured value or None). Check `\"is_error\"` before using a result \
and check that `\"data\"` is not None before indexing it.
- `list_tools()` and `search_tools(query)` return `[{name, description}]`; `describe_tool(name)` \
adds the parameter and return schemas.
- Output: `print()` lines, then the global `result` as JSON. Other variable names are your choice.
- Starlark has `def`, `for`, `if`, comprehensions, and f-strings, but no `while`, `try`, imports, \
or exceptions. At most 64 nested calls per script.
- Nested calls follow the session's permissions and pause for approvals. Scripts get no process \
exit notifications: poll using data[\"next_cursor\"] until data[\"state\"] is no longer running or starting.";

#[derive(Deserialize)]
struct Args {
    script: String,
}

pub(super) struct CodeModeTool {
    surface: Arc<CodeModeSurface>,
}

impl CodeModeTool {
    pub(super) fn new(surface: Arc<CodeModeSurface>) -> Self {
        Self { surface }
    }
}

impl Tool for CodeModeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: CODEMODE_TOOL_NAME.into(),
            description: DESCRIPTION.into(),
            input_schema: json!({"type": "object", "properties": {"script": {"type": "string", "description": "Starlark source."}}, "required": ["script"], "additionalProperties": false}),
        }
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(rho_tools::output_schema::<EngineOutput>())
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            if context.cancellation().is_cancelled() {
                return Err(ToolError::cancelled());
            }
            let Args { script } =
                serde_json::from_value(invocation.arguments().clone()).map_err(|error| {
                    ToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
                })?;
            let bridge = Arc::new(ToolHostBridge::new(self.surface.clone(), context.clone())?);
            let script_bridge = bridge.clone();
            let script_thread = tokio::task::spawn_blocking(move || {
                evaluate_code_mode(&script, script_bridge, EngineLimits::default())
            });
            // Both evaluator ticks and native waits observe parent cancellation.
            // Join the blocking worker rather than abandoning a still-running script.
            let evaluation = script_thread
                .await
                .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            if context.cancellation().is_cancelled() {
                return Err(ToolError::cancelled());
            }
            let (return_value, error) = match evaluation.result {
                Ok(value) => (value, None),
                Err(error) => (serde_json::Value::Null, Some(format!("{error:#}"))),
            };
            let output = EngineOutput {
                return_value,
                prints: evaluation.prints,
                calls: bridge.started_calls().await,
                error,
            };
            let failed = output.error.is_some();
            rho_tools::Rendered::new(format_engine_output(&output), output)
                .failed_if(failed)
                .into_tool_output(Default::default())
        })
    }
}
