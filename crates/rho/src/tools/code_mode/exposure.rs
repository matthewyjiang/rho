//! One live inventory for script execution, discovery, and model advertisement.

use std::sync::{Arc, RwLock};

use rho_sdk::{
    model::ToolSpec,
    tool::{Tool, ToolContext, ToolError, ToolErrorKind, ToolVisibility},
    ToolHost,
};
use schemars::JsonSchema;
use serde::Serialize;

use crate::{
    config::CodemodeMode,
    tools::mcp::exported_name::{parse_exported_name, ExportedNameDialect},
};

use super::{
    script_output,
    tool::CodeModeTool,
    tool_search::{ToolSearchTool, TOOL_SEARCH_NAME},
    CODEMODE_TOOL_NAME,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub(crate) struct ToolCatalogEntry {
    pub name: String,
    pub description: String,
    /// The script envelope schema, with the tool's nullable data schema.
    pub returns: serde_json::Value,
}

struct ScriptTool {
    tool: Arc<dyn Tool>,
    entry: ToolCatalogEntry,
    mcp: bool,
}

#[derive(Default)]
struct SurfaceState {
    tools: Vec<ScriptTool>,
    mode: CodemodeMode,
}

/// Owns the script-callable siblings. Each script snapshots execution and
/// discovery together; orchestration tools never enter that snapshot.
#[derive(Default)]
pub(crate) struct CodeModeSurface {
    state: RwLock<SurfaceState>,
}

impl CodeModeSurface {
    pub(crate) fn orchestration_tools(self: &Arc<Self>) -> [Arc<dyn Tool>; 2] {
        [
            Arc::new(CodeModeTool::new(self.clone())),
            Arc::new(ToolSearchTool::new(self.clone())),
        ]
    }

    pub(crate) fn sync(&self, tools: &[Arc<dyn Tool>]) {
        let mut siblings: Vec<_> = tools
            .iter()
            .filter_map(|tool| {
                let spec = tool.spec();
                if is_orchestration_tool(&spec.name) {
                    return None;
                }
                let mcp = parse_exported_name(&spec.name, ExportedNameDialect::Rho).is_some();
                Some(ScriptTool {
                    tool: tool.clone(),
                    entry: ToolCatalogEntry {
                        name: spec.name,
                        description: spec.description,
                        returns: script_output::schema(tool.output_schema()),
                    },
                    mcp,
                })
            })
            .collect();
        siblings.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
        self.state.write().expect("codemode surface").tools = siblings;
    }

    pub(crate) fn snapshot(
        &self,
        context: &ToolContext,
    ) -> Result<(ToolHost, Vec<ToolCatalogEntry>), ToolError> {
        let state = self.state.read().expect("codemode surface");
        let host = state
            .tools
            .iter()
            .fold(ToolHost::child_builder(context), |builder, sibling| {
                builder.tool_shared(sibling.tool.clone())
            })
            .build()
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
        Ok((
            host,
            state
                .tools
                .iter()
                .map(|sibling| sibling.entry.clone())
                .collect(),
        ))
    }

    pub(crate) fn search(&self, query: &str, limit: usize) -> Vec<ToolCatalogEntry> {
        let state = self.state.read().expect("codemode surface");
        search_entries(
            state.tools.iter().map(|sibling| &sibling.entry),
            query,
            limit,
        )
    }

    pub(crate) fn mode(&self) -> CodemodeMode {
        self.state.read().expect("codemode surface").mode
    }

    pub(crate) fn set_mode(&self, mode: CodemodeMode) {
        self.state.write().expect("codemode surface").mode = mode;
    }
}

pub(super) fn search_entries<'a>(
    entries: impl Iterator<Item = &'a ToolCatalogEntry>,
    query: &str,
    limit: usize,
) -> Vec<ToolCatalogEntry> {
    let query = query.trim().to_ascii_lowercase();
    entries
        .filter(|entry| {
            entry.name.to_ascii_lowercase().contains(&query)
                || entry.description.to_ascii_lowercase().contains(&query)
        })
        .take(limit)
        .cloned()
        .collect()
}

fn is_orchestration_tool(name: &str) -> bool {
    matches!(name, CODEMODE_TOOL_NAME | TOOL_SEARCH_NAME)
}

impl ToolVisibility for CodeModeSurface {
    fn is_advertised(&self, name: &str) -> bool {
        if is_orchestration_tool(name) {
            return true;
        }
        let state = self.state.read().expect("codemode surface");
        state.mode == CodemodeMode::On
            && state
                .tools
                .iter()
                .any(|sibling| sibling.entry.name == name && !sibling.mcp)
    }

    fn describe(&self, spec: &ToolSpec) -> Option<String> {
        let state = self.state.read().expect("codemode surface");
        if state.mode != CodemodeMode::On {
            return None;
        }
        state
            .tools
            .iter()
            .find(|sibling| sibling.entry.name == spec.name)?;
        Some(format!(
            "{}\n\nCodemode: `call_tool(\"{}\", args)` returns `{{ is_error, content, data }}`; batch independent calls with `call_tools`.",
            spec.description.trim_end(), spec.name,
        ))
    }
}
