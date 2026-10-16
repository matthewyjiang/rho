//! Model-side discovery loads tools for direct calls on the next request.
//!
//! Hits stay compact. Loaded tools get full argument schemas in the next
//! provider request; scripts retrieve schemas with `describe_tool(name)`.

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
            description: "Search for tools and load matching tools for direct calls on the next model request, including MCP tools and native tools hidden by codemode only mode. Searches names and descriptions (multi-word queries match any word; a name match ranks above a description match). Returns names, descriptions, and top-level parameter names (`?` marks optional); full argument schemas are declared on the next request. Use a focused query and small limit: every returned tool adds its schema to subsequent requests. Wait for this search to finish before calling a loaded tool directly. Script-side `search_tools`, `list_tools`, and `describe_tool` are read-only and do not load tools; use `call_tool` or `call_tools` inside codemode to call tools without loading them.".into(),
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
                .search_and_load(&args.query, args.limit)
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
                format!(
                    "loaded tools for direct calls on the next model request; their full argument schemas will be declared then:\n{}",
                    serde_json::to_string_pretty(&hits).expect("serializable hits"),
                )
            };
            rho_tools::Rendered::new(text, hits).into_tool_output(Default::default())
        })
    }
}
