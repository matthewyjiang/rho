//! Compatibility adapter that exposes application coding tools through the
//! public [`rho_sdk::tool::Tool`] contract.
//!
//! Shared filesystem implementations live with the application tools. This
//! module only supplies SDK-facing wrappers that require an explicit workspace
//! and authorize every read or write through
//! [`WorkspacePolicy`](rho_sdk::WorkspacePolicy) and
//! [`ApprovalHandler`](rho_sdk::ApprovalHandler). Default SDK construction still
//! grants no capabilities.
//!
//! Resource-aware adapters implement [`Tool::prepare`] only and rely on the
//! default [`Tool::call`] body from `rho-sdk`. That default must stay available
//! in the crates.io `rho-sdk` version pinned by this crate; bump both together
//! when the prepare-only contract changes.
//!
//! The interactive and automation runtimes register these adapters on the public
//! SDK runtime. They do not participate in tool presentation, which is derived
//! from SDK events and metadata by the application presenter.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use rho_sdk::{
    tool::{
        AuthorizedToolContext, OperationKind, PreparedToolInvocation, Tool, ToolAsset, ToolFuture,
        ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
        ToolProgress, ToolResource, ToolResourceAccess, ToolSecurity,
    },
    CapabilityKind, ResolvedWorkspacePath, Workspace, WorkspacePathState,
};

use crate::{
    sdk_support::{
        check_preparation_cancelled, map_app_error, map_path_error, parse_args, path_request,
        preparation_workspace, PathCapability,
    },
    tool::{compact_display_path, truncate, Tool as AppTool},
};

use super::{
    list_dir::{list_directory, ListDir},
    read_file::{read_file_content, read_file_display_content, ReadFile},
    write_file::{write_file_content, WriteFile},
};

#[path = "sdk_adapter/edit.rs"]
mod edit;
pub(crate) fn build_edit_sdk_tool(
    format: crate::EditFormat,
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
) -> Arc<dyn Tool> {
    edit::build_sdk_tool(format, max_output_bytes, mutation_observer)
}

#[path = "sdk_adapter/mutation.rs"]
mod mutation;
use mutation::{mutation_output, run_observed_mutation};

#[path = "sdk_adapter/registry.rs"]
mod registry;
pub use registry::{coding_tool, coding_tools, CodingToolKind, CodingToolOptions};

/// Compatibility name for the canonical [`crate::EditFormat`] type.
///
/// # Next major
///
/// NEXT_MAJOR(rho-tools): remove the EditToolKind alias and use EditFormat directly.
///
/// The alias keeps source compatibility for 1.x hosts. New code should name
/// [`crate::EditFormat`].
pub type EditToolKind = crate::EditFormat;

// Tool selection and registry mechanics live in `registry`; adapters below
// only translate individual filesystem operations to the SDK contract.

pub(super) struct ListDirTool {
    pub(super) max_output_bytes: usize,
}

pub(super) struct ReadFileTool {
    pub(super) max_output_bytes: usize,
    pub(super) mint_tag: bool,
}

pub(super) struct WriteFileTool {
    pub(super) max_output_bytes: usize,
    pub(super) mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
    pub(super) mint_tag: bool,
}

#[derive(Deserialize)]
struct PathArgs {
    path: String,
}

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}

impl Tool for ListDirTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        ListDir.spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Read])
    }

    fn start_metadata(&self, arguments: &Value) -> ToolMetadata {
        path_start_metadata(arguments, OperationKind::Read)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            check_preparation_cancelled(&context)?;
            let args: PathArgs = parse_args(invocation.into_arguments())?;
            let workspace = preparation_workspace(&context)?.clone();
            let resolved = workspace
                .resolve_for_read(&args.path)
                .map_err(map_path_error)?;
            let capability = path_request(&resolved, PathCapability::Read, "list_dir");
            let accesses = [
                ToolResourceAccess::shared(ToolResource::directory_tree(resolved.path())),
                ToolResourceAccess::shared(ToolResource::directory_membership(resolved.path())),
            ];
            let metadata =
                path_start_metadata(&serde_json::json!({"path": args.path}), OperationKind::Read);
            Ok(PreparedToolInvocation::resource_aware(
                accesses,
                [capability],
                metadata,
                move |_context| {
                    Box::pin(async move {
                        workspace.revalidate(&resolved).map_err(map_path_error)?;
                        let content = list_directory(resolved.path())
                            .await
                            .map_err(map_app_error)?;
                        let display = compact_display_path(workspace.root(), &args.path);
                        Ok(
                            ToolOutput::text(truncate(content, self.max_output_bytes)).metadata(
                                ToolMetadata::new()
                                    .operation(OperationKind::Read)
                                    .affected_path(display),
                            ),
                        )
                    })
                },
            ))
        })
    }
}

impl Tool for ReadFileTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        ReadFile.spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Read])
    }

    fn start_metadata(&self, arguments: &Value) -> ToolMetadata {
        path_start_metadata(arguments, OperationKind::Read)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            check_preparation_cancelled(&context)?;
            let args: ReadArgs = parse_args(invocation.into_arguments())?;
            let workspace = preparation_workspace(&context)?.clone();
            let resolved = workspace
                .resolve_for_read(&args.path)
                .map_err(map_path_error)?;
            let metadata =
                path_start_metadata(&serde_json::json!({"path": args.path}), OperationKind::Read);
            Ok(PreparedToolInvocation::resource_aware(
                [ToolResourceAccess::shared(ToolResource::workspace_path(
                    resolved.path(),
                ))],
                [path_request(&resolved, PathCapability::Read, "read_file")],
                metadata,
                move |_context| {
                    Box::pin(async move {
                        workspace.revalidate(&resolved).map_err(map_path_error)?;
                        let display_path = compact_display_path(workspace.root(), &args.path);
                        let output = read_file_content(
                            resolved.path(),
                            &display_path,
                            args.offset,
                            args.limit,
                            self.mint_tag,
                        )
                        .await
                        .map_err(map_app_error)?;
                        let display = read_file_display_content(
                            workspace.root(),
                            &args.path,
                            &serde_json::json!({"offset": args.offset, "limit": args.limit}),
                        );
                        let mut metadata = ToolMetadata::new()
                            .operation(OperationKind::Read)
                            .affected_path(display);
                        if let Some(image) = output.image {
                            metadata =
                                metadata.asset(ToolAsset::new(image.media_type, image.bytes));
                        }
                        if let Some(error) = output.preview_error {
                            metadata = metadata.presentation_notice(error);
                        }
                        let content = truncate(output.content, self.max_output_bytes);
                        Ok(ToolOutput::text(content).metadata(metadata))
                    })
                },
            ))
        })
    }
}

impl Tool for WriteFileTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        WriteFile.spec()
    }

    fn security(&self) -> ToolSecurity {
        // Diff-producing writes read existing content, so both capabilities are
        // independently required and independently authorized.
        ToolSecurity::built_in([CapabilityKind::Write, CapabilityKind::Read])
    }

    fn start_metadata(&self, arguments: &Value) -> ToolMetadata {
        path_start_metadata(arguments, OperationKind::Write)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            check_preparation_cancelled(&context)?;
            let args: WriteArgs = parse_args(invocation.into_arguments())?;
            let workspace = preparation_workspace(&context)?.clone();
            let resolved = workspace
                .resolve_for_write(&args.path)
                .map_err(map_path_error)?;
            let mut capabilities = vec![path_request(&resolved, PathCapability::Write, "write")];
            if resolved.state() == WorkspacePathState::Existing {
                capabilities.push(path_request(&resolved, PathCapability::Read, "write"));
            }
            let accesses = write_accesses(&resolved);
            let metadata = path_start_metadata(
                &serde_json::json!({"path": args.path}),
                OperationKind::Write,
            );
            Ok(PreparedToolInvocation::resource_aware(
                accesses,
                capabilities,
                metadata,
                move |context| {
                    execute_prepared_write(
                        self.max_output_bytes,
                        self.mutation_observer.clone(),
                        self.mint_tag,
                        workspace,
                        resolved,
                        args,
                        context,
                    )
                },
            ))
        })
    }
}

fn execute_prepared_write(
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
    mint_tag: bool,
    workspace: Workspace,
    resolved: ResolvedWorkspacePath,
    args: WriteArgs,
    context: AuthorizedToolContext,
) -> ToolFuture<'static> {
    Box::pin(async move {
        let display = compact_display_path(workspace.root(), &args.path);
        let _ = context
            .progress()
            .send(
                ToolProgress::message(format!("writing {display}"))
                    .metadata(ToolMetadata::new().operation(OperationKind::Write)),
            )
            .await;
        workspace.revalidate(&resolved).map_err(map_path_error)?;
        let mutation_paths = [resolved.path()];
        let outcome = run_observed_mutation(
            mutation_observer.as_ref(),
            &mutation_paths,
            write_file_content(
                resolved.path(),
                &display,
                &args.content,
                max_output_bytes,
                mint_tag,
            ),
        )
        .await?;
        Ok(mutation_output(outcome))
    })
}

// Mutation observation is shared by write and every edit adapter; its paired
// before/after semantics live in the focused `mutation` module.

pub(super) fn write_accesses(path: &ResolvedWorkspacePath) -> Vec<ToolResourceAccess> {
    let mut accesses = vec![ToolResourceAccess::exclusive(ToolResource::workspace_path(
        path.path(),
    ))];
    if path.state() != WorkspacePathState::MissingWriteTarget {
        return accesses;
    }
    let mut child = path.path();
    while let Some(parent) = child.parent() {
        accesses.push(ToolResourceAccess::exclusive(
            ToolResource::directory_membership(parent),
        ));
        if parent.exists() {
            break;
        }
        child = parent;
    }
    accesses
}

pub(super) fn path_start_metadata(arguments: &Value, operation: OperationKind) -> ToolMetadata {
    let mut metadata = ToolMetadata::new().operation(operation);
    if let Some(path) = arguments.get("path").and_then(Value::as_str) {
        metadata = metadata.affected_path(path);
    }
    metadata
}

/// Test helper: build a deny-by-default tool context rooted at `workspace`.
#[cfg(test)]
pub(crate) fn deny_context(
    workspace: Option<rho_sdk::Workspace>,
) -> (
    rho_sdk::tool::ToolContext,
    rho_sdk::tool::ToolProgressReceiver,
) {
    use rho_sdk::tool::ToolContext;

    let (progress, receiver) =
        rho_sdk::tool::tool_progress_channel(std::num::NonZeroUsize::new(4).unwrap());
    (
        ToolContext::new(workspace, rho_sdk::CancellationToken::new(), progress),
        receiver,
    )
}

#[cfg(test)]
#[path = "sdk_adapter_tests.rs"]
mod tests;
