use std::{collections::BTreeMap, fmt, sync::Arc};

use super::{Tool, ToolExecutionMode, ToolSecurity};
use crate::model::ToolSpec;

/// Chooses which registered tools a model request advertises.
///
/// Install with [`crate::RhoBuilder::tool_visibility_shared`]. The runtime asks
/// before **every** model request, so a change (for example a search tool that
/// promotes a deferred tool) takes effect on the next request of the same run.
/// Context estimates and compaction use the same advertised set.
///
/// Registration is separate from advertisement: every registered tool stays
/// available to host-sourced calls and to nested hosts built with
/// [`crate::ToolHost::child_builder`]. A model-sourced call to a tool that is
/// not advertised in the request that produced it resolves as unavailable.
/// Visibility changes affect subsequent requests, never calls already advertised.
///
/// Implementors must be cheap, non-blocking, and deterministic for a given
/// state. Each advertisement projection calls visibility once per registered
/// tool, including model requests, context estimates, and compaction.
pub trait ToolVisibility: Send + Sync {
    /// Returns whether the model may see and call `name` on the next request.
    fn is_advertised(&self, name: &str) -> bool;

    /// Optionally replaces an advertised description without changing identity
    /// or schema. Keep it stable across requests for provider prompt caches.
    fn describe(&self, _spec: &ToolSpec) -> Option<String> {
        None
    }
}

/// Projects registry specs to the advertised subset, applying description overrides.
///
/// Use this same projection for provider requests and host-side context estimates.
pub fn advertised_specs(specs: &[ToolSpec], visibility: &dyn ToolVisibility) -> Vec<ToolSpec> {
    specs
        .iter()
        .filter(|spec| visibility.is_advertised(&spec.name))
        .map(|spec| {
            let mut advertised = spec.clone();
            if let Some(description) = visibility.describe(spec) {
                advertised.description = description;
            }
            advertised
        })
        .collect()
}

/// Error returned when two tools use the same stable name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateToolName {
    name: String,
}

impl DuplicateToolName {
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for DuplicateToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "duplicate tool name '{}'", self.name)
    }
}

impl std::error::Error for DuplicateToolName {}

/// Deterministically ordered registry of SDK tools.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&mut self, tool: T) -> Result<(), DuplicateToolName>
    where
        T: Tool + 'static,
    {
        self.register_shared(Arc::new(tool))
    }

    pub fn register_shared(&mut self, tool: Arc<dyn Tool>) -> Result<(), DuplicateToolName> {
        let name = tool.spec().name;
        if self.tools.contains_key(&name) {
            return Err(DuplicateToolName { name });
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    /// Names of registered tools that declare [`ToolExecutionMode::Async`].
    pub fn async_tool_names(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter(|(_, tool)| tool.execution_mode() == ToolExecutionMode::Async)
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub(crate) fn diagnostics(&self) -> Vec<(String, ToolSecurity)> {
        self.tools
            .values()
            .map(|tool| (tool.spec().name, tool.security()))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }
}

impl fmt::Debug for ToolRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolRegistry")
            .field("tool_names", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}
