//! Live nesting for `codemode`: sibling tools under the parent call's authorization.
//!
//! On each `codemode` call the tool builds a short-lived child [`ToolHost`] via
//! [`ToolHost::child_builder`], which inherits the parent call's workspace,
//! workspace policy, hook gate, and approval session (handler, exact-request
//! memory, audit). Nested `call_tool` is therefore judged exactly like a direct
//! call in the same run: no second prompt, no hook bypass, no policy snapshot.

use std::sync::{Arc, RwLock};

use rho_sdk::tool::{Tool, ToolContext, ToolError, ToolErrorKind};
use rho_sdk::ToolHost;

use super::bridge::CODEMODE_TOOL_NAME;

/// Per-script nested `call_tool` budget enforced by [`super::bridge::GuardedBridge`].
///
/// Tripwire against runaway loops, not a measured workload ceiling; scripts
/// that hit it get a loud `CallLimit` bridge error naming the limit.
pub const DEFAULT_MAX_NESTED_CALLS: usize = 64;

/// Sibling tools callable from `codemode`, refreshed by
/// [`crate::tools::AppToolSet`] whenever its tool list changes.
#[derive(Default)]
pub struct CodeModeNesting {
    /// Never contains `codemode` itself: that would be a recursion path and an
    /// `Arc` cycle (`codemode -> nesting -> codemode`).
    tools: RwLock<Vec<Arc<dyn Tool>>>,
}

impl CodeModeNesting {
    pub fn set_tools(&self, tools: &[Arc<dyn Tool>]) {
        let siblings = tools
            .iter()
            .filter(|tool| tool.spec().name != CODEMODE_TOOL_NAME)
            .cloned()
            .collect();
        *self.tools.write().expect("code_mode nesting tools") = siblings;
    }

    /// Build the child host for one `codemode` call.
    pub fn build_host(&self, context: &ToolContext) -> Result<ToolHost, ToolError> {
        let tools = self.tools.read().expect("code_mode nesting tools").clone();
        tools
            .into_iter()
            .fold(ToolHost::child_builder(context), |builder, tool| {
                builder.tool_shared(tool)
            })
            .build()
            .map_err(|error| {
                ToolError::new(
                    ToolErrorKind::Execution,
                    format!("codemode: failed to build nested ToolHost: {error}"),
                )
            })
    }
}
