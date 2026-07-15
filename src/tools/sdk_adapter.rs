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
//! Automation registers these adapters on the public SDK runtime while the TUI
//! continues to use the application tool registry during its later migration.

use std::{path::PathBuf, sync::Arc};

use serde::Deserialize;
use serde_json::Value;

use rho_sdk::{
    tool::{
        OperationKind, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation,
        ToolMetadata, ToolOutput, ToolProgress,
    },
    CapabilityRequest, Error as SdkError,
};

#[cfg(test)]
use rho_sdk::tool::{DuplicateToolName, ToolRegistry};

use crate::tool::{compact_display_path, truncate, Tool as AppTool, ToolError as AppToolError};

use super::{
    edit_file::{apply_edits, EditFile},
    edit_file_args::Args as EditArgs,
    list_dir::{list_directory, ListDir},
    read_file::{read_file_content, read_file_display_content, ReadFile},
    write_file::{write_file_content, WriteFile},
};

/// Default tool-output budget, matching the application configuration default.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 12_000;

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

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            check_cancelled(&context)?;
            let args: PathArgs = parse_args(invocation.into_arguments())?;
            let path = authorize_existing_path(&context, &args.path, PathCapability::Read).await?;
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

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            check_cancelled(&context)?;
            let args: ReadArgs = parse_args(invocation.into_arguments())?;
            let path = authorize_existing_path(&context, &args.path, PathCapability::Read).await?;
            let content = read_file_content(&path, args.offset, args.limit)
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

impl Tool for WriteFileTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        WriteFile.spec()
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            check_cancelled(&context)?;
            let args: WriteArgs = parse_args(invocation.into_arguments())?;
            let path = authorize_write_path(&context, &args.path).await?;
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

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            check_cancelled(&context)?;
            let args: EditArgs = parse_args(invocation.into_arguments())?;
            let edits = args.into_edits().map_err(map_app_error)?;
            let root = workspace_root(&context)?.to_path_buf();
            for edit in &edits {
                let _ =
                    authorize_existing_path(&context, &edit.path, PathCapability::Write).await?;
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
                |path| resolve_workspace_path(&context, path),
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

fn check_cancelled(context: &ToolContext) -> Result<(), ToolError> {
    if context.cancellation().is_cancelled() {
        Err(ToolError::cancelled())
    } else {
        Ok(())
    }
}

fn workspace_root(context: &ToolContext) -> Result<&std::path::Path, ToolError> {
    context.workspace_root().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::Execution,
            "workspace is required for coding tools",
        )
    })
}

fn resolve_workspace_path(context: &ToolContext, path: &str) -> PathBuf {
    match context.workspace() {
        Some(workspace) => workspace
            .resolve(path)
            .unwrap_or_else(|_| workspace.root().join(path)),
        None => PathBuf::from(path),
    }
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
) -> Result<PathBuf, ToolError> {
    let workspace = context.workspace().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::Execution,
            "workspace is required for coding tools",
        )
    })?;
    let lexical = workspace.resolve(path).map_err(map_sdk_error)?;
    authorize_path(context, lexical.clone(), capability).await?;
    match workspace.resolve_existing(path) {
        Ok(path) => Ok(path),
        Err(error) if lexical.exists() => Err(map_sdk_error(error)),
        // Missing paths keep the lexical location so callers see normal I/O errors.
        Err(_) => Ok(lexical),
    }
}

async fn authorize_write_path(context: &ToolContext, path: &str) -> Result<PathBuf, ToolError> {
    let workspace = context.workspace().ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::Execution,
            "workspace is required for coding tools",
        )
    })?;
    let lexical = workspace.resolve(path).map_err(map_sdk_error)?;
    authorize_path(context, lexical.clone(), PathCapability::Write).await?;
    Ok(lexical)
}

async fn authorize_path(
    context: &ToolContext,
    path: PathBuf,
    capability: PathCapability,
) -> Result<(), ToolError> {
    let request = match capability {
        PathCapability::Read => CapabilityRequest::ReadPath { path },
        PathCapability::Write => CapabilityRequest::WritePath { path },
    };
    context.authorize(request).await.map_err(map_sdk_error)
}

fn map_sdk_error(error: SdkError) -> ToolError {
    match error {
        SdkError::Cancelled => ToolError::cancelled(),
        SdkError::PolicyDenied { message } => ToolError::new(ToolErrorKind::Execution, message),
        SdkError::InvalidConfiguration { message } => {
            ToolError::new(ToolErrorKind::Execution, message)
        }
        other => ToolError::new(ToolErrorKind::Execution, other.to_string()),
    }
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
