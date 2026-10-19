//! The writable app inventory and its single codemode publication seam.

use std::sync::Arc;

use rho_sdk::tool::Tool;

use super::super::code_mode::CodeModeSurface;

/// The parent registry can read tools, but only `mutate` can write them. Every
/// change publishes execution and discovery together in one surface snapshot.
#[derive(Default)]
pub(super) struct ToolInventory {
    tools: Vec<Arc<dyn Tool>>,
    /// Names and handles share registration order and are refreshed only at
    /// the mutation boundary, never during read-only inventory lookups.
    names: Vec<String>,
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

    pub(super) fn mutate<T>(&mut self, mutation: impl FnOnce(&mut Vec<Arc<dyn Tool>>) -> T) -> T {
        let result = mutation(&mut self.tools);
        self.names = self.tools.iter().map(|tool| tool.spec().name).collect();
        self.surface.sync_named(
            self.named_tools()
                .map(|(name, tool)| (name.to_owned(), tool.clone())),
        );
        result
    }
}
