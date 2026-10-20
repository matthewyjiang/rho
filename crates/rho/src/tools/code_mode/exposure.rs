//! One live inventory for script execution, discovery, and model advertisement.

use std::{
    collections::BTreeSet,
    sync::{Arc, RwLock},
};

use rho_sdk::{
    model::ToolSpec,
    tool::{Tool, ToolContext, ToolError, ToolErrorKind, ToolVisibility},
    ToolHost,
};
use serde::Serialize;

use crate::config::CodemodeMode;

use super::{
    search::search_entries,
    tool::CodeModeTool,
    tool_search::{ToolSearchHit, ToolSearchTool, TOOL_SEARCH_NAME},
    CODEMODE_TOOL_NAME,
};

/// The full contract script `describe_tool` returns. `tool_search` and script
/// discovery send only compact summaries of it.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ToolCatalogEntry {
    pub name: String,
    pub description: String,
    /// The complete schema of arguments accepted by the tool.
    pub parameters: serde_json::Value,
    /// The schema of a successful call's `data`, or `None` for text-only
    /// tools. The shared `{is_error, content, data}` envelope is documented
    /// once in the `codemode` tool description instead of per entry.
    pub returns: Option<serde_json::Value>,
}

/// Whether a tool is declared to the model before `tool_search` loads it.
/// Owners choose this per bundle; codemode never infers it from the tool name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextExposure {
    /// Declared directly while `[codemode] mode` is `on`.
    Direct,
    /// Kept out of model context in every mode until top-level `tool_search`
    /// loads it. Scripts can still discover and call it. Large generated
    /// catalogs, such as MCP tools, use this so they cost no context up front.
    Hidden,
}

impl ContextExposure {
    /// Whether an unloaded tool with this exposure is declared in `mode`.
    fn declared_in(self, mode: CodemodeMode) -> bool {
        match self {
            Self::Direct => mode == CodemodeMode::On,
            Self::Hidden => false,
        }
    }
}

/// One script-callable inventory entry as the owning registry publishes it.
#[derive(Clone)]
pub(crate) struct ScriptTool {
    tool: Arc<dyn Tool>,
    name: String,
    exposure: ContextExposure,
}

impl ScriptTool {
    pub(crate) fn new(name: String, tool: Arc<dyn Tool>, exposure: ContextExposure) -> Self {
        Self {
            tool,
            name,
            exposure,
        }
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    // Definitions can refresh independently of the app inventory. A bridge
    // freezes descriptions and schemas only on its first discovery request.
    pub(super) fn catalog_entry(&self) -> ToolCatalogEntry {
        let spec = self.tool.spec();
        ToolCatalogEntry {
            name: spec.name,
            description: spec.description,
            parameters: spec.input_schema,
            returns: self.tool.output_schema(),
        }
    }
}

#[derive(Default)]
struct SurfaceState {
    tools: Vec<ScriptTool>,
    loaded: BTreeSet<String>,
    mode: CodemodeMode,
}

/// Owns the script-callable siblings. Each script snapshots callable handles;
/// discovery freezes their contracts on first use. Orchestration stays outside.
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

    /// Publishes direct siblings; tests use it in place of a registry.
    #[cfg(test)]
    pub(crate) fn sync(&self, tools: &[Arc<dyn Tool>]) {
        self.sync_entries(
            tools.iter().map(|tool| {
                ScriptTool::new(tool.spec().name, tool.clone(), ContextExposure::Direct)
            }),
        );
    }

    pub(crate) fn sync_entries(&self, tools: impl IntoIterator<Item = ScriptTool>) {
        let mut siblings: Vec<_> = tools
            .into_iter()
            .filter(|sibling| !is_orchestration_tool(&sibling.name))
            .collect();
        siblings.sort_by(|a, b| a.name.cmp(&b.name));
        self.state.write().expect("codemode surface").tools = siblings;
    }

    pub(super) fn snapshot(
        &self,
        context: &ToolContext,
    ) -> Result<(ToolHost, Vec<ScriptTool>), ToolError> {
        let state = self.state.read().expect("codemode surface");
        let host = state
            .tools
            .iter()
            .fold(ToolHost::child_builder(context), |builder, sibling| {
                builder.tool_shared(sibling.tool.clone())
            })
            .max_parallel_tools(crate::app::sdk_config::parallel_tool_limit())
            .build()
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
        Ok((host, state.tools.clone()))
    }

    /// Top-level search loads only returned hits for subsequent model requests,
    /// including in `only` mode. Script discovery stays read-only in its bridge.
    pub(crate) fn search_and_load(&self, query: &str, limit: usize) -> Vec<ToolSearchHit> {
        let mut state = self.state.write().expect("codemode surface");
        // Search needs argument summaries, not return contracts. In particular,
        // do not generate every sibling's output schema just to load a hit.
        let entries: Vec<_> = state
            .tools
            .iter()
            .map(|sibling| {
                let spec = sibling.tool.spec();
                ToolCatalogEntry {
                    name: spec.name,
                    description: spec.description,
                    parameters: spec.input_schema,
                    returns: None,
                }
            })
            .collect();
        let hits = search_entries(&entries, query, limit);
        state
            .loaded
            .extend(hits.iter().map(|entry| entry.name.clone()));
        // Keep only compact results once this fresh catalog goes out of scope.
        hits.into_iter().map(ToolSearchHit::from).collect()
    }

    pub(crate) fn mode(&self) -> CodemodeMode {
        self.state.read().expect("codemode surface").mode
    }

    pub(crate) fn set_mode(&self, mode: CodemodeMode) {
        self.state.write().expect("codemode surface").mode = mode;
    }

    /// Store branch-local loading separately from history so compaction need
    /// not retain the original search results. Absent tools may reconnect later.
    pub(crate) fn decorate(&self, snapshot: rho_sdk::SessionSnapshot) -> rho_sdk::SessionSnapshot {
        let (key, value) = self.checkpoint();
        snapshot.with_metadata(key, value)
    }

    /// Freeze the loadout at a compaction boundary, before asynchronous work
    /// can observe later searches. The SDK commits it with the replacement.
    pub(crate) fn checkpoint(&self) -> (&'static str, String) {
        let state = self.state.read().expect("codemode surface");
        (
            LOADED_TOOLS_KEY,
            serde_json::to_string(&state.loaded).expect("serializable tool names"),
        )
    }

    /// Conversation replacement restores its own loadout; policy-only runtime
    /// rebuilds keep the current loadout instead of resetting it.
    pub(crate) fn restore(&self, snapshot: &rho_sdk::SessionSnapshot) {
        let loaded = snapshot
            .metadata()
            .get(LOADED_TOOLS_KEY)
            .and_then(|value| serde_json::from_str(value).ok())
            .unwrap_or_default();
        self.state.write().expect("codemode surface").loaded = loaded;
    }

    pub(crate) fn clear_loaded(&self) {
        self.state.write().expect("codemode surface").loaded.clear();
    }
}

const LOADED_TOOLS_KEY: &str = "rho.codemode.loaded_tools";

fn is_orchestration_tool(name: &str) -> bool {
    matches!(name, CODEMODE_TOOL_NAME | TOOL_SEARCH_NAME)
}

impl ToolVisibility for CodeModeSurface {
    fn is_advertised(&self, name: &str) -> bool {
        if is_orchestration_tool(name) {
            return true;
        }
        let state = self.state.read().expect("codemode surface");
        state
            .tools
            .binary_search_by(|sibling| sibling.name.as_str().cmp(name))
            .is_ok_and(|index| {
                state.loaded.contains(name) || state.tools[index].exposure.declared_in(state.mode)
            })
    }

    fn describe(&self, spec: &ToolSpec) -> Option<String> {
        let state = self.state.read().expect("codemode surface");
        if state.mode != CodemodeMode::On {
            return None;
        }
        state
            .tools
            .binary_search_by(|sibling| sibling.name.cmp(&spec.name))
            .ok()?;
        Some(format!(
            "{}\n\nCodemode: `call_tool(\"{}\", args)` returns `{{ is_error, content, data }}`; batch independent calls with `call_tools`.",
            spec.description.trim_end(), spec.name,
        ))
    }
}
