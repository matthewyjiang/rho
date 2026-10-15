//! Model-side discovery, including native tools omitted in `only` mode.
//!
//! Hits stay compact: MCP servers repeat large union schemas across tools, so
//! full schemas sit behind script `describe_tool(name)` instead.

use std::sync::Arc;

use rho_sdk::model::ToolSpec;
use rho_sdk::tool::{Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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

/// One `tool_search` hit, as both model text and structured data.
#[derive(Debug, Serialize, JsonSchema)]
struct ToolSearchHit {
    name: String,
    description: String,
    /// Top-level argument names; optional ones end in `?`.
    parameters: Vec<String>,
}

impl From<ToolCatalogEntry> for ToolSearchHit {
    fn from(entry: ToolCatalogEntry) -> Self {
        Self {
            parameters: parameter_names(&entry.parameters),
            name: entry.name,
            description: entry.description,
        }
    }
}

/// Required names in `required` order, then the other properties, each with
/// a `?` suffix.
fn parameter_names(schema: &Value) -> Vec<String> {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let optional = schema["properties"]
        .as_object()
        .into_iter()
        .flat_map(|properties| properties.keys())
        .filter(|name| !required.contains(&name.as_str()))
        .map(|name| format!("{name}?"));
    required
        .iter()
        .map(|name| (*name).to_owned())
        .chain(optional)
        .collect()
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
            description: "Discover script-callable tools by keyword search over names and descriptions (multi-word queries match any word; a name match ranks above a description match), including MCP tools and native tools in codemode only mode. Returns names, descriptions, and top-level parameter names (`?` marks optional). Call discovered tools through codemode; when names alone do not settle the arguments, first get full parameter and result schemas with `describe_tool(name)` in a script.".into(),
            input_schema: json!({"type": "object", "properties": {"query": {"type": "string"}, "limit": {"type": "integer", "minimum": 1, "default": 10}}, "required": ["query"], "additionalProperties": false}),
        }
    }

    fn output_schema(&self) -> Option<Value> {
        Some(rho_tools::output_schema::<Vec<ToolSearchHit>>())
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
            let hits: Vec<ToolSearchHit> = self
                .surface
                .search(&args.query, args.limit)
                .into_iter()
                .map(ToolSearchHit::from)
                .collect();
            // A bare `[]` reads as "this server is not connected", so a miss
            // says how to browse the catalog instead. `data` stays `[]`.
            let text = if hits.is_empty() {
                format!(
                    "no tools matched {:?}; call `list_tools()` in a codemode script to browse every script-callable tool.",
                    args.query,
                )
            } else {
                serde_json::to_string_pretty(&hits).expect("serializable hits")
            };
            rho_tools::Rendered::new(text, hits).into_tool_output(Default::default())
        })
    }
}
