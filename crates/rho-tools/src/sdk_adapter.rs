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
//! The interactive and automation runtimes register these adapters on the public
//! SDK runtime. They do not participate in tool presentation, which is derived
//! from SDK events and metadata by the application presenter.

use std::{path::PathBuf, sync::Arc};

use serde::Deserialize;
use serde_json::Value;

use rho_sdk::{
    tool::{
        AuthorizedToolContext, OperationKind, PreparedToolInvocation, Tool, ToolAsset, ToolContext,
        ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolMetadata, ToolOutput,
        ToolPreparationContext, ToolPrepareFuture, ToolProgress, ToolResource, ToolResourceAccess,
        ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, ResolvedWorkspacePath, Workspace,
    WorkspacePathError, WorkspacePathState,
};

#[cfg(test)]
use rho_sdk::tool::{DuplicateToolName, ToolRegistry};

use crate::{
    tool::{compact_display_path, truncate, Tool as AppTool, ToolError as AppToolError},
    DEFAULT_MAX_OUTPUT_BYTES,
};

use super::{
    edit_file::{apply_edits, EditFile},
    edit_file_args::{Args as EditArgs, Edit},
    list_dir::{list_directory, ListDir},
    read_file::{read_file_content, read_file_display_content, ReadFile},
    write_file::{write_file_content, WriteFile},
};

/// Options for coding tools registered on an SDK runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodingToolOptions {
    max_output_bytes: usize,
}

impl Default for CodingToolOptions {
    fn default() -> Self {
        Self {
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

impl CodingToolOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes.max(1);
        self
    }

    #[cfg(test)]
    pub fn output_budget(&self) -> usize {
        self.max_output_bytes
    }
}

/// Registers the four workspace coding tools on an SDK registry.
///
/// The tools do not grant capabilities by themselves. Hosts must attach a
/// workspace and a non-default policy on the runtime before reads or writes
/// succeed.
#[cfg(test)]
pub fn register_coding_tools(
    registry: &mut ToolRegistry,
    options: CodingToolOptions,
) -> Result<(), DuplicateToolName> {
    for tool in coding_tools(options) {
        registry.register_shared(tool)?;
    }
    Ok(())
}

/// A workspace coding tool selected by a host capability set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodingToolKind {
    ListDir,
    ReadFile,
    WriteFile,
    EditFile,
}

/// Returns one selected SDK coding tool.
pub fn coding_tool(kind: CodingToolKind, options: CodingToolOptions) -> Arc<dyn Tool> {
    match kind {
        CodingToolKind::ListDir => Arc::new(ListDirTool {
            max_output_bytes: options.max_output_bytes,
        }),
        CodingToolKind::ReadFile => Arc::new(ReadFileTool {
            max_output_bytes: options.max_output_bytes,
        }),
        CodingToolKind::WriteFile => Arc::new(WriteFileTool {
            max_output_bytes: options.max_output_bytes,
        }),
        CodingToolKind::EditFile => Arc::new(EditFileTool {
            max_output_bytes: options.max_output_bytes,
        }),
    }
}

/// Returns all SDK coding tools as shared trait objects.
pub fn coding_tools(options: CodingToolOptions) -> Vec<Arc<dyn Tool>> {
    [
        CodingToolKind::ListDir,
        CodingToolKind::ReadFile,
        CodingToolKind::WriteFile,
        CodingToolKind::EditFile,
    ]
    .into_iter()
    .map(|kind| coding_tool(kind, options.clone()))
    .collect()
}

struct ListDirTool {
    max_output_bytes: usize,
}

struct ReadFileTool {
    max_output_bytes: usize,
}

struct WriteFileTool {
    max_output_bytes: usize,
}

struct EditFileTool {
    max_output_bytes: usize,
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

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        rho_sdk::tool::call_prepared(self, invocation, context)
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
                        let output = read_file_content(resolved.path(), args.offset, args.limit)
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
                        Ok(
                            ToolOutput::text(truncate(output.content, self.max_output_bytes))
                                .metadata(metadata),
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
            let mut capabilities =
                vec![path_request(&resolved, PathCapability::Write, "write_file")];
            if resolved.state() == WorkspacePathState::Existing {
                capabilities.push(path_request(&resolved, PathCapability::Read, "write_file"));
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
                        workspace,
                        resolved,
                        args,
                        context,
                    )
                },
            ))
        })
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        rho_sdk::tool::call_prepared(self, invocation, context)
    }
}

impl Tool for EditFileTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        EditFile.spec()
    }

    fn security(&self) -> ToolSecurity {
        // Edits always read current file contents before applying replacements.
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
            let metadata = path_start_metadata(invocation.arguments(), OperationKind::Write);
            let args: EditArgs = parse_args(invocation.into_arguments())?;
            let edits = args.into_edits().map_err(map_app_error)?;
            let workspace = preparation_workspace(&context)?.clone();
            let mut resolved_by_request = std::collections::HashMap::new();
            let mut resolved_by_canonical = std::collections::BTreeMap::new();
            for edit in &edits {
                let resolved = workspace
                    .resolve_for_read(&edit.path)
                    .map_err(map_path_error)?;
                resolved_by_request.insert(edit.path.clone(), resolved.path().to_path_buf());
                resolved_by_canonical
                    .entry(resolved.path().to_path_buf())
                    .or_insert(resolved);
            }
            let accesses = resolved_by_canonical
                .keys()
                .map(|path| ToolResourceAccess::exclusive(ToolResource::workspace_path(path)))
                .collect::<Vec<_>>();
            let capabilities = resolved_by_canonical
                .values()
                .flat_map(|resolved| {
                    [
                        path_request(resolved, PathCapability::Write, "edit_file"),
                        path_request(resolved, PathCapability::Read, "edit_file"),
                    ]
                })
                .collect::<Vec<_>>();
            Ok(PreparedToolInvocation::resource_aware(
                accesses,
                capabilities,
                metadata,
                move |context| {
                    execute_prepared_edits(
                        self.max_output_bytes,
                        workspace,
                        resolved_by_canonical,
                        resolved_by_request,
                        edits,
                        context,
                    )
                },
            ))
        })
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        rho_sdk::tool::call_prepared(self, invocation, context)
    }
}

fn execute_prepared_write(
    max_output_bytes: usize,
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
        let outcome =
            write_file_content(resolved.path(), &display, &args.content, max_output_bytes)
                .await
                .map_err(map_app_error)?;
        Ok(ToolOutput::text(outcome.content).metadata(
            ToolMetadata::new()
                .operation(OperationKind::Write)
                .affected_path(outcome.display_path)
                .diff(outcome.diff),
        ))
    })
}

fn execute_prepared_edits(
    max_output_bytes: usize,
    workspace: Workspace,
    resolved: std::collections::BTreeMap<PathBuf, ResolvedWorkspacePath>,
    requested_paths: std::collections::HashMap<String, PathBuf>,
    edits: Vec<Edit>,
    context: AuthorizedToolContext,
) -> ToolFuture<'static> {
    Box::pin(async move {
        let total = edits.len() as u64;
        let _ = context
            .progress()
            .send(
                ToolProgress::message(format!("editing {total} change(s)"))
                    .units(0, total.max(1))
                    .metadata(ToolMetadata::new().operation(OperationKind::Write)),
            )
            .await;
        let outcome = apply_edits(
            edits,
            |requested| {
                let path = requested_paths.get(requested).ok_or_else(|| {
                    AppToolError::Message(format!(
                        "edit path '{requested}' was not prepared for this invocation"
                    ))
                })?;
                let prepared = resolved.get(path).ok_or_else(|| {
                    AppToolError::Message(format!(
                        "edit target '{}' was not prepared for this invocation",
                        path.display()
                    ))
                })?;
                workspace
                    .revalidate(prepared)
                    .map_err(|error| AppToolError::Message(error.to_string()))?;
                Ok(path.clone())
            },
            |path| compact_display_path(workspace.root(), path),
            max_output_bytes,
        )
        .await
        .map_err(map_app_error)?;
        let _ = context
            .progress()
            .send(
                ToolProgress::message(format!("edited {} file(s)", outcome.file_count))
                    .units(total.max(1), total.max(1))
                    .metadata(ToolMetadata::new().operation(OperationKind::Write)),
            )
            .await;
        let mut metadata = ToolMetadata::new().operation(OperationKind::Write);
        for path in &outcome.display_paths {
            metadata = metadata.affected_path(path);
        }
        Ok(ToolOutput::text(outcome.content).metadata(metadata.diff(outcome.diffs)))
    })
}

fn check_preparation_cancelled(context: &ToolPreparationContext) -> Result<(), ToolError> {
    if context.cancellation().is_cancelled() {
        Err(ToolError::cancelled())
    } else {
        Ok(())
    }
}

fn preparation_workspace(context: &ToolPreparationContext) -> Result<&Workspace, ToolError> {
    context.workspace().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::Execution,
            "workspace is required for built-in tools",
        )
    })
}

fn path_request(
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

fn write_accesses(path: &ResolvedWorkspacePath) -> Vec<ToolResourceAccess> {
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

fn path_start_metadata(arguments: &Value, operation: OperationKind) -> ToolMetadata {
    let mut metadata = ToolMetadata::new().operation(operation);
    if let Some(path) = arguments.get("path").and_then(Value::as_str) {
        metadata = metadata.affected_path(path);
    }
    for path in arguments
        .get("edits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edit| edit.get("path").and_then(Value::as_str))
    {
        metadata = metadata.affected_path(path);
    }
    metadata
}

#[derive(Clone, Copy)]
enum PathCapability {
    Read,
    Write,
}

fn parse_args<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|error| {
        ToolError::new(
            ToolErrorKind::InvalidArguments,
            format!("invalid arguments: {error}"),
        )
    })
}

fn map_path_error(error: WorkspacePathError) -> ToolError {
    let kind = match error.kind() {
        rho_sdk::WorkspacePathErrorKind::ParentTraversal
        | rho_sdk::WorkspacePathErrorKind::OutsideGrantedRoots
        | rho_sdk::WorkspacePathErrorKind::InvalidPlatformPath
        | rho_sdk::WorkspacePathErrorKind::ChangedAfterAuthorization => ToolErrorKind::PolicyDenied,
        _ => ToolErrorKind::Execution,
    };
    ToolError::new(kind, error.to_string())
}

fn map_app_error(error: AppToolError) -> ToolError {
    match error {
        AppToolError::InvalidArguments(error) => ToolError::new(
            ToolErrorKind::InvalidArguments,
            format!("invalid arguments: {error}"),
        ),
        AppToolError::Io(error) => ToolError::new(ToolErrorKind::Execution, error.to_string()),
        AppToolError::Utf8(error) => ToolError::new(ToolErrorKind::Execution, error.to_string()),
        AppToolError::Message(message) if message == "tool interrupted" => ToolError::cancelled(),
        AppToolError::Message(message) => ToolError::new(ToolErrorKind::Execution, message),
    }
}

/// Test helper: build a deny-by-default tool context rooted at `workspace`.
#[cfg(test)]
pub(super) fn deny_context(
    workspace: Option<rho_sdk::Workspace>,
) -> (ToolContext, rho_sdk::tool::ToolProgressReceiver) {
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
