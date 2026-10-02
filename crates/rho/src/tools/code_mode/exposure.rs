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
    /// The complete schema of arguments accepted by the tool.
    pub parameters: serde_json::Value,
    /// The script envelope schema, with the tool's nullable data schema.
    pub returns: serde_json::Value,
}

struct ScriptTool {
    tool: Arc<dyn Tool>,
    name: String,
    mcp: bool,
}

impl ScriptTool {
    // Definitions can refresh independently of the app inventory. Only a
    // script's bridge snapshot freezes descriptions and schemas.
    fn catalog_entry(&self) -> ToolCatalogEntry {
        let spec = self.tool.spec();
        ToolCatalogEntry {
            name: spec.name,
            description: spec.description,
            parameters: spec.input_schema,
            returns: script_output::schema(self.tool.output_schema()),
        }
    }
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
                    name: spec.name,
                    mcp,
                })
            })
            .collect();
        siblings.sort_by(|a, b| a.name.cmp(&b.name));
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
            state.tools.iter().map(ScriptTool::catalog_entry).collect(),
        ))
    }

    pub(crate) fn search(&self, query: &str, limit: usize) -> Vec<ToolCatalogEntry> {
        let state = self.state.read().expect("codemode surface");
        let entries: Vec<_> = state.tools.iter().map(ScriptTool::catalog_entry).collect();
        search_entries(entries.iter(), query, limit)
    }

    pub(crate) fn mode(&self) -> CodemodeMode {
        self.state.read().expect("codemode surface").mode
    }

    pub(crate) fn set_mode(&self, mode: CodemodeMode) {
        self.state.write().expect("codemode surface").mode = mode;
    }
}

/// Keyword search shared by `tool_search` and script `search_tools`.
///
/// Models write queries like `memorywhale remember` or `+github issue`, so the
/// query is split into distinct terms on anything but alphanumerics and `_`.
/// Terms shorter than [`MIN_TERM_LEN`] are dropped when longer ones exist, so
/// filler like `a` or `to` cannot match every description. An entry matches
/// when any term is a substring of its name or description; a name hit scores
/// above a description hit, higher scores rank first, and ties keep catalog
/// (name) order. A query without terms lists every entry.
pub(super) fn search_entries<'a>(
    entries: impl Iterator<Item = &'a ToolCatalogEntry>,
    query: &str,
    limit: usize,
) -> Vec<ToolCatalogEntry> {
    let terms = search_terms(query);
    let mut hits: Vec<_> = entries
        .filter_map(|entry| {
            let name = entry.name.to_ascii_lowercase();
            let description = entry.description.to_ascii_lowercase();
            let score: usize = terms
                .iter()
                .map(|term| {
                    2 * usize::from(name.contains(term.as_str()))
                        + usize::from(description.contains(term.as_str()))
                })
                .sum();
            (terms.is_empty() || score > 0).then_some((score, entry))
        })
        .collect();
    // Stable sort keeps name order within equal scores.
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter()
        .take(limit)
        .map(|(_, entry)| entry.clone())
        .collect()
}

/// Shortest term that counts when the query also has longer terms. Two-letter
/// words (`a`, `to`, `in`) are substrings of most descriptions; a query made
/// only of short terms (`gh`) still searches with them.
const MIN_TERM_LEN: usize = 3;

fn search_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = query
        .to_ascii_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|term| !term.is_empty())
        .map(str::to_owned)
        .collect();
    terms.sort();
    terms.dedup();
    if terms.iter().any(|term| term.len() >= MIN_TERM_LEN) {
        terms.retain(|term| term.len() >= MIN_TERM_LEN);
    }
    terms
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
                .any(|sibling| sibling.name == name && !sibling.mcp)
    }

    fn describe(&self, spec: &ToolSpec) -> Option<String> {
        let state = self.state.read().expect("codemode surface");
        if state.mode != CodemodeMode::On {
            return None;
        }
        state
            .tools
            .iter()
            .find(|sibling| sibling.name == spec.name)?;
        Some(format!(
            "{}\n\nCodemode: `call_tool(\"{}\", args)` returns `{{ is_error, content, data }}`; batch independent calls with `call_tools`.",
            spec.description.trim_end(), spec.name,
        ))
    }
}

#[cfg(test)]
#[path = "exposure_tests.rs"]
mod tests;
