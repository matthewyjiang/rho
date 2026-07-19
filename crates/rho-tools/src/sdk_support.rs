use std::path::Path;

use rho_sdk::{
    tool::{ToolContext, ToolError, ToolErrorKind},
    Workspace,
};

const WORKSPACE_REQUIRED: &str = "workspace is required for built-in tools";

pub fn check_cancelled(context: &ToolContext) -> Result<(), ToolError> {
    if context.cancellation().is_cancelled() {
        Err(ToolError::cancelled())
    } else {
        Ok(())
    }
}

pub fn workspace(context: &ToolContext) -> Result<&Workspace, ToolError> {
    context
        .workspace()
        .ok_or_else(|| ToolError::new(ToolErrorKind::Execution, WORKSPACE_REQUIRED))
}

pub fn workspace_root(context: &ToolContext) -> Result<&Path, ToolError> {
    context
        .workspace_root()
        .ok_or_else(|| ToolError::new(ToolErrorKind::Execution, WORKSPACE_REQUIRED))
}

pub fn required_string<'a>(
    arguments: &'a serde_json::Value,
    field: &str,
) -> Result<&'a str, ToolError> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                format!("missing string argument '{field}'"),
            )
        })
}
