//! The writable app inventory and its single codemode publication seam.

use std::sync::Arc;

use rho_sdk::tool::Tool;

use super::super::code_mode::CodeModeSurface;

/// The parent registry can read tools, but only `mutate` can write them. Every
/// change publishes execution and discovery together in one surface snapshot.
#[derive(Default)]
pub(super) struct ToolInventory {
    tools: Vec<Arc<dyn Tool>>,
    surface: Arc<CodeModeSurface>,
}

impl ToolInventory {
    pub(super) fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    pub(super) fn surface(&self) -> &Arc<CodeModeSurface> {
        &self.surface
    }

    pub(super) fn mutate<T>(&mut self, mutation: impl FnOnce(&mut Vec<Arc<dyn Tool>>) -> T) -> T {
        let result = mutation(&mut self.tools);
        self.surface.sync(&self.tools);
        result
    }
}
