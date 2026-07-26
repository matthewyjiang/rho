//! SDK adapters for the in-process `grep` and `glob` workspace tools.
//!
//! Both tools request a single `Read` capability on the resolved search root
//! and declare a shared directory-tree resource so concurrent walks can
//! overlap safely.

use std::sync::Arc;

use serde_json::Value;

use rho_sdk::{
    tool::{
        OperationKind, PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind,
        ToolFuture, ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext,
        ToolPrepareFuture, ToolResource, ToolResourceAccess, ToolSecurity,
    },
    CapabilityKind,
};

use crate::{
    glob::{glob_workspace, Glob, GlobRequest},
    grep::{grep_workspace, Grep, GrepRequest},
    sdk_support::{
        check_preparation_cancelled, map_app_error, map_path_error, path_request,
        preparation_workspace, PathCapability,
    },
    tool::{compact_display_path, truncate, Tool as AppTool},
};

/// SDK adapter for [`Grep`].
pub struct GrepTool {
    max_output_bytes: usize,
}

impl GrepTool {
    pub fn new(max_output_bytes: usize) -> Self {
        Self {
            max_output_bytes: max_output_bytes.max(1),
        }
    }
}

/// SDK adapter for [`Glob`].
pub struct GlobTool {
    max_output_bytes: usize,
}

impl GlobTool {
    pub fn new(max_output_bytes: usize) -> Self {
        Self {
            max_output_bytes: max_output_bytes.max(1),
        }
    }
}

pub fn grep_tool(max_output_bytes: usize) -> Arc<dyn Tool> {
    Arc::new(GrepTool::new(max_output_bytes))
}

pub fn glob_tool(max_output_bytes: usize) -> Arc<dyn Tool> {
    Arc::new(GlobTool::new(max_output_bytes))
}

fn search_start_metadata(arguments: &Value) -> ToolMetadata {
    let mut metadata = ToolMetadata::new().operation(OperationKind::Read);
    if let Some(path) = arguments.get("path").and_then(Value::as_str) {
        metadata = metadata.affected_path(path);
    }
    metadata
}

impl Tool for GrepTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        Grep.spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Read])
    }

    fn start_metadata(&self, arguments: &Value) -> ToolMetadata {
        search_start_metadata(arguments)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            check_preparation_cancelled(&context)?;
            let arguments = invocation.into_arguments();
            let metadata = search_start_metadata(&arguments);
            let request = GrepRequest::from_arguments(arguments).map_err(map_app_error)?;
            let workspace = preparation_workspace(&context)?.clone();
            let resolved = workspace
                .resolve_for_read(&request.path)
                .map_err(map_path_error)?;
            let capability = path_request(&resolved, PathCapability::Read, "grep");
            let accesses = [ToolResourceAccess::shared(ToolResource::directory_tree(
                resolved.path(),
            ))];
            let max_output_bytes = self.max_output_bytes;
            let requested_path = request.path.clone();
            Ok(PreparedToolInvocation::resource_aware(
                accesses,
                [capability],
                metadata,
                move |context| {
                    Box::pin(async move {
                        workspace.revalidate(&resolved).map_err(map_path_error)?;
                        let display = compact_display_path(workspace.root(), &requested_path);
                        let root = resolved.path().to_path_buf();
                        let cancellation = context.cancellation().clone();
                        let content = tokio::task::spawn_blocking(move || {
                            grep_workspace(&root, &display, &request, &|| {
                                cancellation.is_cancelled()
                            })
                        })
                        .await
                        .map_err(|error| {
                            ToolError::new(
                                ToolErrorKind::Execution,
                                format!("grep task failed: {error}"),
                            )
                        })?
                        .map_err(map_app_error)?;
                        let affected = compact_display_path(workspace.root(), &requested_path);
                        Ok(
                            ToolOutput::text(truncate(content, max_output_bytes)).metadata(
                                ToolMetadata::new()
                                    .operation(OperationKind::Read)
                                    .affected_path(affected),
                            ),
                        )
                    })
                },
            ))
        })
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        rho_sdk::tool::call_prepared(self, invocation, context)
    }
}

impl Tool for GlobTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        Glob.spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Read])
    }

    fn start_metadata(&self, arguments: &Value) -> ToolMetadata {
        search_start_metadata(arguments)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            check_preparation_cancelled(&context)?;
            let arguments = invocation.into_arguments();
            let metadata = search_start_metadata(&arguments);
            let request = GlobRequest::from_arguments(arguments).map_err(map_app_error)?;
            let workspace = preparation_workspace(&context)?.clone();
            let resolved = workspace
                .resolve_for_read(&request.path)
                .map_err(map_path_error)?;
            let capability = path_request(&resolved, PathCapability::Read, "glob");
            let accesses = [ToolResourceAccess::shared(ToolResource::directory_tree(
                resolved.path(),
            ))];
            let max_output_bytes = self.max_output_bytes;
            let requested_path = request.path.clone();
            Ok(PreparedToolInvocation::resource_aware(
                accesses,
                [capability],
                metadata,
                move |context| {
                    Box::pin(async move {
                        workspace.revalidate(&resolved).map_err(map_path_error)?;
                        let display = compact_display_path(workspace.root(), &requested_path);
                        let root = resolved.path().to_path_buf();
                        let cancellation = context.cancellation().clone();
                        let content = tokio::task::spawn_blocking(move || {
                            glob_workspace(&root, &display, &request, &|| {
                                cancellation.is_cancelled()
                            })
                        })
                        .await
                        .map_err(|error| {
                            ToolError::new(
                                ToolErrorKind::Execution,
                                format!("glob task failed: {error}"),
                            )
                        })?
                        .map_err(map_app_error)?;
                        let affected = compact_display_path(workspace.root(), &requested_path);
                        Ok(
                            ToolOutput::text(truncate(content, max_output_bytes)).metadata(
                                ToolMetadata::new()
                                    .operation(OperationKind::Read)
                                    .affected_path(affected),
                            ),
                        )
                    })
                },
            ))
        })
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        rho_sdk::tool::call_prepared(self, invocation, context)
    }
}

#[cfg(test)]
#[path = "sdk_search_tests.rs"]
mod tests;
