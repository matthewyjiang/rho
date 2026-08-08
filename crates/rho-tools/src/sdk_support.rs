use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use rho_sdk::{
    tool::{ToolContext, ToolError, ToolErrorKind, ToolPreparationContext},
    CapabilityRequest, CapabilitySource, ResolvedWorkspacePath, Workspace, WorkspacePathError,
};

use crate::tool::ToolError as AppToolError;

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

pub(crate) fn check_preparation_cancelled(
    context: &ToolPreparationContext,
) -> Result<(), ToolError> {
    if context.cancellation().is_cancelled() {
        Err(ToolError::cancelled())
    } else {
        Ok(())
    }
}

pub(crate) fn preparation_workspace(
    context: &ToolPreparationContext,
) -> Result<&Workspace, ToolError> {
    context
        .workspace()
        .ok_or_else(|| ToolError::new(ToolErrorKind::Execution, WORKSPACE_REQUIRED))
}

#[derive(Clone, Copy)]
pub(crate) enum PathCapability {
    Read,
    Write,
}

pub(crate) fn path_request(
    path: &ResolvedWorkspacePath,
    capability: PathCapability,
    tool_name: &str,
) -> CapabilityRequest {
    let source = CapabilitySource::built_in_tool(tool_name);
    match capability {
        PathCapability::Read => {
            CapabilityRequest::read_path(path.path(), path.scope().clone(), source)
        }
        PathCapability::Write => {
            CapabilityRequest::write_path(path.path(), path.scope().clone(), source)
        }
    }
}

pub(crate) fn parse_args<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|error| {
        ToolError::new(
            ToolErrorKind::InvalidArguments,
            format!("invalid arguments: {error}"),
        )
    })
}

pub(crate) fn map_path_error(error: WorkspacePathError) -> ToolError {
    let kind = match error.kind() {
        rho_sdk::WorkspacePathErrorKind::ParentTraversal
        | rho_sdk::WorkspacePathErrorKind::OutsideGrantedRoots
        | rho_sdk::WorkspacePathErrorKind::InvalidPlatformPath
        | rho_sdk::WorkspacePathErrorKind::ChangedAfterAuthorization => ToolErrorKind::PolicyDenied,
        _ => ToolErrorKind::Execution,
    };
    ToolError::new(kind, error.to_string())
}

pub(crate) fn map_app_error(error: AppToolError) -> ToolError {
    match error {
        AppToolError::InvalidArguments(error) => ToolError::new(
            ToolErrorKind::InvalidArguments,
            format!("invalid arguments: {error}"),
        ),
        AppToolError::Io(error) => ToolError::new(ToolErrorKind::Execution, error.to_string()),
        AppToolError::Utf8(error) => ToolError::new(ToolErrorKind::Execution, error.to_string()),
        AppToolError::Cancelled => ToolError::cancelled(),
        AppToolError::Message(message) => ToolError::new(ToolErrorKind::Execution, message),
    }
}

pub(crate) fn map_invalid_app_error(error: AppToolError) -> ToolError {
    match error {
        AppToolError::Message(message) => ToolError::new(ToolErrorKind::InvalidArguments, message),
        other => map_app_error(other),
    }
}
