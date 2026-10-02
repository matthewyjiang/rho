//! Model-side discovery, including native tools omitted in `only` mode.

use std::sync::Arc;

use rho_sdk::model::ToolSpec;
use rho_sdk::tool::{Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation};
use serde::Deserialize;
use serde_json::json;

use super::exposure::{CodeModeSurface, ToolCatalogEntry};

pub(crate) const TOOL_SEARCH_NAME: &str = "tool_search";

#[derive(Deserialize)]
struct Args {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    10
}

pub(super) struct ToolSearchTool {
    surface: Arc<CodeModeSurface>,
}

impl ToolSearchTool {
    pub(super) fn new(surface: Arc<CodeModeSurface>) -> Self {
        Self { surface }
    }
}

impl Tool for ToolSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: TOOL_SEARCH_NAME.into(),
            description: "Discover script-callable tools by name or description, including MCP tools and native tools in codemode only mode. Returns names, descriptions, and parameter and return schemas; call discovered tools through codemode.".into(),
            input_schema: json!({"type": "object", "properties": {"query": {"type": "string"}, "limit": {"type": "integer", "minimum": 1, "default": 10}}, "required": ["query"], "additionalProperties": false}),
        }
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(rho_tools::output_schema::<Vec<ToolCatalogEntry>>())
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            if context.cancellation().is_cancelled() {
                return Err(ToolError::cancelled());
            }
            let args: Args =
                serde_json::from_value(invocation.arguments().clone()).map_err(|error| {
                    ToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
                })?;
            let hits = self.surface.search(&args.query, args.limit);
            let text = serde_json::to_string_pretty(&hits).expect("serializable catalog");
            rho_tools::Rendered::new(text, hits).into_tool_output(Default::default())
        })
    }
}
