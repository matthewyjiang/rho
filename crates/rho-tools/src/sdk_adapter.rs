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
        OperationKind, Tool, ToolAsset, ToolContext, ToolError, ToolErrorKind, ToolFuture,
        ToolInvocation, ToolMetadata, ToolOutput, ToolProgress, ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, WorkspacePathError, WorkspacePathState,
};

#[cfg(test)]
use rho_sdk::tool::{DuplicateToolName, ToolRegistry};

use crate::{
    tool::{compact_display_path, truncate, Tool as AppTool, ToolError as AppToolError},
    DEFAULT_MAX_OUTPUT_BYTES,
};

use super::{
    edit_file::{apply_edits, EditFile},
    edit_file_args::Args as EditArgs,
    list_dir::{list_directory, ListDir},
    read_file::{read_file_content, read_file_display_content, ReadFile},
    sdk_support::{check_cancelled, workspace, workspace_root},
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

/// Returns the SDK coding tools as shared trait objects.
pub fn coding_tools(options: CodingToolOptions) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ListDirTool {
            max_output_bytes: options.max_output_bytes,
        }),
        Arc::new(ReadFileTool {
            max_output_bytes: options.max_output_bytes,
        }),
        Arc::new(WriteFileTool {
            max_output_bytes: options.max_output_bytes,
        }),
        Arc::new(EditFileTool {
            max_output_bytes: options.max_output_bytes,
        }),
    ]
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

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            check_cancelled(&context)?;
            let args: PathArgs = parse_args(invocation.into_arguments())?;
            let path =
                authorize_existing_path(&context, &args.path, PathCapability::Read, "list_dir")
                    .await?;
            let content = list_directory(&path).await.map_err(map_app_error)?;
            let display = display_path(&context, &args.path);
            Ok(
                ToolOutput::text(truncate(content, self.max_output_bytes)).metadata(
                    ToolMetadata::new()
                        .operation(OperationKind::Read)
                        .affected_path(display),
                ),
            )
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

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            check_cancelled(&context)?;
            let args: ReadArgs = parse_args(invocation.into_arguments())?;
            let path =
                authorize_existing_path(&context, &args.path, PathCapability::Read, "read_file")
                    .await?;
            let output = read_file_content(&path, args.offset, args.limit)
                .await
                .map_err(map_app_error)?;
            let display = read_file_display_content(
                workspace_root(&context)?,
                &args.path,
                &serde_json::json!({
                    "offset": args.offset,
                    "limit": args.limit,
                }),
            );
            let mut metadata = ToolMetadata::new()
                .operation(OperationKind::Read)
                .affected_path(display);
            if let Some(image) = output.image {
                metadata = metadata.asset(ToolAsset::new(image.media_type, image.bytes));
            }
            if let Some(error) = output.preview_error {
                metadata = metadata.presentation_notice(error);
            }
            Ok(
                ToolOutput::text(truncate(output.content, self.max_output_bytes))
                    .metadata(metadata),
            )
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

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            check_cancelled(&context)?;
            let args: WriteArgs = parse_args(invocation.into_arguments())?;
            let path = authorize_write_path(&context, &args.path, "write_file").await?;
            let display = display_path(&context, &args.path);
            let _ = context
                .progress()
                .send(
                    ToolProgress::message(format!("writing {display}"))
                        .metadata(ToolMetadata::new().operation(OperationKind::Write)),
                )
                .await;
            let outcome = write_file_content(&path, &display, &args.content, self.max_output_bytes)
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

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            check_cancelled(&context)?;
            let args: EditArgs = parse_args(invocation.into_arguments())?;
            let edits = args.into_edits().map_err(map_app_error)?;
            let root = workspace_root(&context)?.to_path_buf();
            let mut authorized_paths = std::collections::HashMap::new();
            for edit in &edits {
                let workspace = workspace(&context)?;
                let resolved = workspace
                    .resolve_for_read(&edit.path)
                    .map_err(map_path_error)?;
                authorize_path(&context, &resolved, PathCapability::Write, "edit_file").await?;
                authorize_path(&context, &resolved, PathCapability::Read, "edit_file").await?;
                workspace.revalidate(&resolved).map_err(map_path_error)?;
                authorized_paths.insert(edit.path.clone(), resolved.path().to_path_buf());
            }
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
                |path| {
                    authorized_paths.get(path).cloned().ok_or_else(|| {
                        AppToolError::Message(format!(
                            "edit path '{path}' was not authorized for this invocation"
                        ))
                    })
                },
                |path| compact_display_path(&root, path),
                self.max_output_bytes,
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
            metadata = metadata.diff(outcome.diffs);
            Ok(ToolOutput::text(outcome.content).metadata(metadata))
        })
    }
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

fn display_path(context: &ToolContext, path: &str) -> String {
    match context.workspace_root() {
        Some(root) => compact_display_path(root, path),
        None => path.to_string(),
    }
}

async fn authorize_existing_path(
    context: &ToolContext,
    path: &str,
    capability: PathCapability,
    tool_name: &str,
) -> Result<PathBuf, ToolError> {
    let workspace = workspace(context)?;
    let resolved = workspace.resolve_for_read(path).map_err(map_path_error)?;
    authorize_path(context, &resolved, capability, tool_name).await?;
    workspace.revalidate(&resolved).map_err(map_path_error)?;
    Ok(resolved.path().to_path_buf())
}

async fn authorize_write_path(
    context: &ToolContext,
    path: &str,
    tool_name: &str,
) -> Result<PathBuf, ToolError> {
    let workspace = workspace(context)?;
    let resolved = workspace.resolve_for_write(path).map_err(map_path_error)?;
    authorize_path(context, &resolved, PathCapability::Write, tool_name).await?;
    // Existing targets are read to build unified diffs, so write-only policies
    // must not observe old content through tool output.
    if resolved.state() == WorkspacePathState::Existing {
        authorize_path(context, &resolved, PathCapability::Read, tool_name).await?;
    }
    workspace.revalidate(&resolved).map_err(map_path_error)?;
    Ok(resolved.path().to_path_buf())
}

async fn authorize_path(
    context: &ToolContext,
    path: &rho_sdk::ResolvedWorkspacePath,
    capability: PathCapability,
    tool_name: &str,
) -> Result<(), ToolError> {
    let source = CapabilitySource::built_in_tool(tool_name);
    let request = match capability {
        PathCapability::Read => {
            CapabilityRequest::read_path(path.path(), path.scope().clone(), source)
        }
        PathCapability::Write => {
            CapabilityRequest::write_path(path.path(), path.scope().clone(), source)
        }
    };
    context
        .authorize(request)
        .await
        .map(|_| ())
        .map_err(|error| {
            if error.kind() == rho_sdk::AuthorizationDenialKind::Cancelled {
                ToolError::cancelled()
            } else {
                ToolError::policy_denied(&error)
            }
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
