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

use std::{path::PathBuf, sync::Arc};

use serde::Deserialize;
use serde_json::Value;

use rho_sdk::{
    tool::{
        AuthorizedToolContext, OperationKind, PreparedToolInvocation, Tool, ToolAsset, ToolError,
        ToolErrorKind, ToolFuture, ToolInvocation, ToolMetadata, ToolOutput,
        ToolPreparationContext, ToolPrepareFuture, ToolProgress, ToolResource, ToolResourceAccess,
        ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, ResolvedWorkspacePath, Workspace, WorkspacePathState,
};

#[cfg(test)]
use rho_sdk::tool::{DuplicateToolName, ToolRegistry};

use crate::{
    sdk_search::{GlobTool, GrepTool},
    sdk_support::{
        check_preparation_cancelled, map_app_error, map_path_error, parse_args, path_request,
        preparation_workspace, PathCapability,
    },
    tool::{compact_display_path, truncate, Tool as AppTool, ToolError as AppToolError},
    DEFAULT_MAX_OUTPUT_BYTES,
};

use super::{
    file_mutation::FileMutationOutcome,
    hashline::{
        apply_prepared_sections, claim_unique_path, parse_hashline, proposed_sections, Edit,
        PreparedSection,
    },
    list_dir::{list_directory, ListDir},
    read_file::{read_file_content, read_file_display_content, ReadFile},
    write_file::{write_file_content, WriteFile},
};

/// Options for coding tools registered on an SDK runtime.
#[derive(Clone)]
pub struct CodingToolOptions {
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
}

impl Default for CodingToolOptions {
    fn default() -> Self {
        Self {
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            mutation_observer: None,
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

    pub fn mutation_observer(
        mut self,
        observer: Arc<dyn crate::WorkspaceMutationObserver>,
    ) -> Self {
        self.mutation_observer = Some(observer);
        self
    }

    #[cfg(test)]
    pub fn output_budget(&self) -> usize {
        self.max_output_bytes
    }
}

/// Registers the workspace coding tools on an SDK registry.
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
    Edit,
    Grep,
    Glob,
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
            mutation_observer: options.mutation_observer.clone(),
        }),
        CodingToolKind::Edit => Arc::new(EditTool {
            max_output_bytes: options.max_output_bytes,
            mutation_observer: options.mutation_observer.clone(),
        }),
        CodingToolKind::Grep => Arc::new(GrepTool::new(options.max_output_bytes)),
        CodingToolKind::Glob => Arc::new(GlobTool::new(options.max_output_bytes)),
    }
}

/// Returns all SDK coding tools as shared trait objects.
pub fn coding_tools(options: CodingToolOptions) -> Vec<Arc<dyn Tool>> {
    [
        CodingToolKind::ListDir,
        CodingToolKind::ReadFile,
        CodingToolKind::WriteFile,
        CodingToolKind::Edit,
        CodingToolKind::Grep,
        CodingToolKind::Glob,
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
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
}

struct EditTool {
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    input: String,
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

impl Tool for EditTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        Edit.spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Write, CapabilityKind::Read])
    }

    fn start_metadata(&self, arguments: &Value) -> ToolMetadata {
        edit_start_metadata(arguments)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            check_preparation_cancelled(&context)?;
            let metadata = edit_start_metadata(invocation.arguments());
            let args: EditArgs = parse_args(invocation.into_arguments())?;
            let sections = parse_hashline(&args.input).map_err(|error| {
                ToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
            })?;
            let workspace = preparation_workspace(&context)?.clone();
            // Resolve while collecting so the executor inherits the authorized
            // target for each section instead of re-parsing the document.
            let mut targets = EditTargetSet::default();
            let mut prepared = Vec::with_capacity(sections.len());
            for section in sections {
                let path = targets.push_existing(&workspace, &section.path)?;
                prepared.push(PreparedSection {
                    display_path: compact_display_path(workspace.root(), &section.path),
                    section,
                    path,
                });
            }
            let EditTargetSet {
                resolved,
                accesses,
                capabilities,
                claimed_as: _,
            } = targets;

            Ok(PreparedToolInvocation::resource_aware(
                accesses,
                capabilities,
                metadata,
                move |context| {
                    execute_prepared_edit(
                        self.max_output_bytes,
                        self.mutation_observer.clone(),
                        workspace,
                        resolved,
                        prepared,
                        context,
                    )
                },
            ))
        })
    }
}

/// Existing edit targets collected during prepare. Edit never creates paths, so
/// this set has no missing-write / rename seam.
///
/// Duplicate paths use [`claim_unique_path`] (same predicate as execute) and map
/// to `InvalidArguments` so authorization never starts for a multi-claim doc.
#[derive(Default)]
struct EditTargetSet {
    resolved: std::collections::BTreeMap<PathBuf, ResolvedWorkspacePath>,
    /// Document claim string per canonical path - shared uniqueness owner.
    claimed_as: std::collections::BTreeMap<PathBuf, String>,
    accesses: Vec<ToolResourceAccess>,
    capabilities: Vec<CapabilityRequest>,
}

impl EditTargetSet {
    fn push_existing(
        &mut self,
        workspace: &Workspace,
        requested_path: &str,
    ) -> Result<PathBuf, ToolError> {
        // Existing edit targets are rewritten in place; resolve for write so path
        // policy matches mutation rather than a read-only open.
        let resolved = workspace
            .resolve_for_write(requested_path)
            .map_err(map_path_error)?;
        let canonical = resolved.path().to_path_buf();
        claim_unique_path(&mut self.claimed_as, canonical.clone(), requested_path)
            .map_err(|message| ToolError::new(ToolErrorKind::InvalidArguments, message))?;
        self.accesses
            .push(ToolResourceAccess::exclusive(ToolResource::workspace_path(
                resolved.path(),
            )));
        self.capabilities
            .push(path_request(&resolved, PathCapability::Write, "edit"));
        self.capabilities
            .push(path_request(&resolved, PathCapability::Read, "edit"));
        self.resolved.insert(canonical.clone(), resolved);
        Ok(canonical)
    }
}

fn execute_prepared_write(
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
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
            write_file_content(resolved.path(), &display, &args.content, max_output_bytes),
        )
        .await?;
        Ok(mutation_output(outcome))
    })
}

fn execute_prepared_edit(
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
    workspace: Workspace,
    resolved: std::collections::BTreeMap<PathBuf, ResolvedWorkspacePath>,
    sections: Vec<PreparedSection>,
    context: AuthorizedToolContext,
) -> ToolFuture<'static> {
    Box::pin(async move {
        let _ = context
            .progress()
            .send(
                ToolProgress::message(format!("applying edit ({} path(s))", resolved.len()))
                    .metadata(ToolMetadata::new().operation(OperationKind::Write)),
            )
            .await;
        let mutation_paths = resolved
            .values()
            .map(ResolvedWorkspacePath::path)
            .collect::<Vec<_>>();
        for prepared in resolved.values() {
            workspace.revalidate(prepared).map_err(map_path_error)?;
        }
        let outcome = run_observed_mutation(
            mutation_observer.as_ref(),
            &mutation_paths,
            apply_prepared_sections(sections, max_output_bytes),
        )
        .await?;
        Ok(mutation_output(outcome))
    })
}

async fn run_observed_mutation<T>(
    observer: Option<&Arc<dyn crate::WorkspaceMutationObserver>>,
    paths: &[&std::path::Path],
    op: impl std::future::Future<Output = Result<T, AppToolError>>,
) -> Result<T, ToolError> {
    if let Some(observer) = observer {
        observer
            .before_mutation(paths)
            .await
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error))?;
    }
    let op_result = op.await.map_err(map_app_error);
    let capture_result = match observer {
        Some(observer) => observer
            .after_mutation(paths)
            .await
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error)),
        None => Ok(()),
    };
    match (op_result, capture_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(capture_error)) => Err(ToolError::new(
            ToolErrorKind::Execution,
            format!(
                "mutation succeeded but capturing the resulting workspace state failed: {capture_error}"
            ),
        )),
        (Err(op_error), Err(capture_error)) => Err(ToolError::new(
            ToolErrorKind::Execution,
            format!("{op_error}; failed to capture resulting workspace state: {capture_error}"),
        )),
    }
}

fn mutation_output(outcome: FileMutationOutcome) -> ToolOutput {
    let mut metadata = ToolMetadata::new()
        .operation(OperationKind::Write)
        .diff(outcome.diff);
    for path in outcome.display_paths {
        metadata = metadata.affected_path(path);
    }
    ToolOutput::text(outcome.content).metadata(metadata)
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
    metadata
}

fn edit_start_metadata(arguments: &Value) -> ToolMetadata {
    let mut metadata = ToolMetadata::new().operation(OperationKind::Write);
    if let Some(input) = arguments.get("input").and_then(Value::as_str) {
        for section in proposed_sections(input) {
            metadata = metadata.affected_path(section.path);
        }
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
