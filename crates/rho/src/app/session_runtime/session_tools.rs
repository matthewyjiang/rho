//! What a host may read of a session's tools.

use std::sync::Arc;

use rho_sdk::tool::Tool;

use crate::tools::{
    mcp::{McpCatalog, McpSessionReport},
    sdk_registry::AppToolSet,
    todo::TodoState,
};

/// A read-only view of a session's tools, for hosts that list what the
/// session offers or reach its background work. It reaches nothing the
/// runtime owns: binding the tools to the session's file or search, or
/// taking a plan decision.
#[derive(Clone, Copy)]
pub(crate) struct SessionTools<'a>(&'a AppToolSet);

impl<'a> SessionTools<'a> {
    pub(super) fn of(tools: &'a AppToolSet) -> Self {
        Self(tools)
    }

    /// A view of `tools`, for host tests that have no runtime.
    #[cfg(test)]
    pub(in crate::app) fn of_tool_set_for_tests(tools: &'a AppToolSet) -> Self {
        Self(tools)
    }

    /// Whether the session offers a tool named `name`.
    pub(crate) fn contains(self, name: &str) -> bool {
        self.0.contains(name)
    }

    /// The tool the session offers as `name`, to call outside a model turn.
    pub(crate) fn tool_named(self, name: &str) -> Option<Arc<dyn Tool>> {
        self.0.tool_named(name)
    }

    pub(crate) fn mcp_catalog(self) -> &'a McpCatalog {
        self.0.mcp_catalog()
    }

    pub(crate) fn mcp_report(self) -> &'a McpSessionReport {
        self.0.mcp_report()
    }

    /// The session's todo checklist, shared with its `todo` tool.
    pub(crate) fn todo_state(self) -> TodoState {
        self.0.todo_state()
    }
}
