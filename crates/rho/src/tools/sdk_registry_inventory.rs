//! The writable app inventory and its single codemode publication seam.

use std::sync::Arc;

use rho_sdk::tool::Tool;

use super::super::code_mode::{CodeModeSurface, ContextExposure, ScriptTool};

/// The parent registry can read tools, but only `mutate` can write them. Every
/// change publishes execution and discovery together in one surface snapshot.
#[derive(Default)]
pub(super) struct ToolInventory {
    tools: Vec<Arc<dyn Tool>>,
    /// Names and handles share registration order and are refreshed only at
    /// the mutation boundary, never during read-only inventory lookups.
    names: Vec<String>,
    /// Handles their owners registered as [`ContextExposure::Hidden`]. Matched
    /// by identity, so a same-named replacement never inherits the exposure.
    hidden: Vec<Arc<dyn Tool>>,
    surface: Arc<CodeModeSurface>,
}

impl ToolInventory {
    pub(super) fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    pub(super) fn surface(&self) -> &Arc<CodeModeSurface> {
        &self.surface
    }

    pub(super) fn named_tools(&self) -> impl Iterator<Item = (&str, &Arc<dyn Tool>)> {
        self.names.iter().map(String::as_str).zip(&self.tools)
    }

    /// Appends one owner's tools with the exposure that owner chose.
    pub(super) fn extend(&mut self, tools: &[Arc<dyn Tool>], exposure: ContextExposure) {
        match exposure {
            ContextExposure::Advertised => {}
            ContextExposure::Hidden => self.hidden.extend(tools.iter().cloned()),
        }
        self.mutate(|all| all.extend(tools.iter().cloned()));
    }

    pub(super) fn mutate<T>(&mut self, mutation: impl FnOnce(&mut Vec<Arc<dyn Tool>>) -> T) -> T {
        let result = mutation(&mut self.tools);
        self.names = self.tools.iter().map(|tool| tool.spec().name).collect();
        // Release handles the mutation removed.
        let tools = &self.tools;
        self.hidden
            .retain(|hidden| tools.iter().any(|tool| Arc::ptr_eq(tool, hidden)));
        let hidden = &self.hidden;
        self.surface
            .sync_entries(self.named_tools().map(|(name, tool)| {
                let exposure = if hidden.iter().any(|entry| Arc::ptr_eq(entry, tool)) {
                    ContextExposure::Hidden
                } else {
                    ContextExposure::Advertised
                };
                ScriptTool::new(name.to_owned(), tool.clone(), exposure)
            }));
        result
    }
}
