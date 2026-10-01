//! `tool_search` — promote `deferred` tools into the active direct set.

use std::sync::Arc;

use rho_sdk::model::ToolSpec;
use rho_sdk::tool::{
    Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolOutput,
};
use serde::Deserialize;
use serde_json::json;

use super::exposure::ExposureController;

pub const TOOL_SEARCH_NAME: &str = "tool_search";

#[derive(Debug, Deserialize)]
struct ToolSearchArgs {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    10
}

/// Keyword/substring search over indexed tools; promotes deferred hits.
pub struct ToolSearchTool {
    exposure: Arc<ExposureController>,
}

impl ToolSearchTool {
    pub fn new(exposure: Arc<ExposureController>) -> Self {
        Self { exposure }
    }
}

impl Tool for ToolSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: TOOL_SEARCH_NAME.to_owned(),
            description: "Search indexed tools by keyword/substring over name and description. \
Matching deferred tools are promoted into the active tool list for subsequent turns. \
Does not dump full server or tool catalogs."
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Keyword or substring to match against tool name and description."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max hits to return (default 10).",
                        "minimum": 1
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        }
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        let args = invocation.arguments().clone();
        let exposure = Arc::clone(&self.exposure);
        Box::pin(async move {
            if context.cancellation().is_cancelled() {
                return Err(ToolError::cancelled());
            }
            let parsed: ToolSearchArgs = serde_json::from_value(args)
                .map_err(|err| ToolError::new(ToolErrorKind::InvalidArguments, err.to_string()))?;
            let hits = exposure.search(&parsed.query, parsed.limit);
            let mut promoted = Vec::new();
            for hit in &hits {
                if exposure.promote(&hit.name) {
                    promoted.push(hit.name.clone());
                }
            }
            let matches: Vec<_> = hits
                .iter()
                .map(|h| {
                    json!({
                        "name": h.name,
                        "description": h.description,
                        "exposure": exposure.effective(&h.name).as_str(),
                    })
                })
                .collect();
            Ok(ToolOutput::text(
                serde_json::to_string_pretty(&json!({
                    "query": parsed.query,
                    "matches": matches,
                    "promoted": promoted,
                }))
                .unwrap_or_else(|_| "{}".into()),
            ))
        })
    }
}
