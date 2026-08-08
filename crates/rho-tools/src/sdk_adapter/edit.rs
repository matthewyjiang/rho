//! SDK adapters and authorization policy for file edit tools.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::Arc,
};

use serde::Deserialize;
use serde_json::Value;

use rho_sdk::{
    tool::{
        AuthorizedToolContext, OperationKind, PreparedToolInvocation, Tool, ToolError,
        ToolErrorKind, ToolFuture, ToolInvocation, ToolMetadata, ToolOutput,
        ToolPreparationContext, ToolPrepareFuture, ToolProgress, ToolResource, ToolResourceAccess,
        ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, ResolvedWorkspacePath, Workspace, WorkspacePathState,
};

use crate::{
    apply_patch::{
        apply_hunks, parse_patch, patch_paths_lenient, reject_symlink_entry, validate_hunk_paths,
        ApplyPatch, Hunk,
    },
    edit_file::{edit_file_content, EditFile, EditFileArgs},
    hashline::{
        apply_prepared_sections, claim_unique_path, parse_hashline, proposed_sections, Edit,
        PreparedSection,
    },
    sdk_support::{
        check_preparation_cancelled, map_app_error, map_invalid_app_error, map_path_error,
        parse_args, path_request, preparation_workspace, PathCapability,
    },
    tool::{compact_display_path, Tool as AppTool, ToolError as AppToolError},
};

use super::{mutation_output, path_start_metadata, run_observed_mutation, write_accesses};

struct HashlineEditTool {
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
}

struct ApplyPatchTool {
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
}

struct EditFileTool {
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
}

pub(super) fn build_sdk_tool(
    format: crate::EditFormat,
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
) -> Arc<dyn Tool> {
    match format {
        crate::EditFormat::Hashline => Arc::new(HashlineEditTool {
            max_output_bytes,
            mutation_observer,
        }),
        crate::EditFormat::ApplyPatch => Arc::new(ApplyPatchTool {
            max_output_bytes,
            mutation_observer,
        }),
        crate::EditFormat::EditFile => Arc::new(EditFileTool {
            max_output_bytes,
            mutation_observer,
        }),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    input: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyPatchArgs {
    input: String,
}

impl Tool for HashlineEditTool {
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

impl Tool for EditFileTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        EditFile.spec()
    }

    fn security(&self) -> ToolSecurity {
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
            let args: EditFileArgs = parse_args(invocation.into_arguments())?;
            args.validate().map_err(map_invalid_edit_args)?;
            let workspace = preparation_workspace(&context)?.clone();
            let resolved = workspace
                .resolve_for_read(&args.path)
                .map_err(map_path_error)?;
            let metadata = path_start_metadata(
                &serde_json::json!({"path": args.path}),
                OperationKind::Write,
            );
            Ok(PreparedToolInvocation::resource_aware(
                [ToolResourceAccess::exclusive(ToolResource::workspace_path(
                    resolved.path(),
                ))],
                [
                    path_request(&resolved, PathCapability::Write, "edit_file"),
                    path_request(&resolved, PathCapability::Read, "edit_file"),
                ],
                metadata,
                move |context| {
                    execute_prepared_string_edit(
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

impl Tool for ApplyPatchTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        ApplyPatch.spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Write, CapabilityKind::Read])
    }

    fn start_metadata(&self, arguments: &Value) -> ToolMetadata {
        patch_start_metadata(arguments)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            check_preparation_cancelled(&context)?;
            let metadata = patch_start_metadata(invocation.arguments());
            let args: ApplyPatchArgs = parse_args(invocation.into_arguments())?;
            let hunks = parse_patch(&args.input).map_err(|error| {
                ToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
            })?;
            let workspace = preparation_workspace(&context)?.clone();
            let mut path_set = PatchPathSet::default();
            for hunk in &hunks {
                validate_hunk_paths(hunk).map_err(map_invalid_app_error)?;
                let mutates_source_entry = hunk.mutates_source_entry();
                path_set.collect(
                    &workspace,
                    hunk.source_path(),
                    /*require_existing*/ hunk.requires_existing_source(),
                    mutates_source_entry,
                )?;
                if let Some(destination) = hunk.move_destination() {
                    path_set.collect(
                        &workspace,
                        destination,
                        /*require_existing*/ false,
                        /*reject_symlink_leaf*/ false,
                    )?;
                }
            }
            let PatchPathSet {
                resolved_by_request,
                resolved_by_canonical,
                accesses,
                capabilities,
            } = path_set;
            Ok(PreparedToolInvocation::resource_aware(
                accesses,
                capabilities,
                metadata,
                move |context| {
                    execute_prepared_patch(
                        self.max_output_bytes,
                        self.mutation_observer.clone(),
                        workspace,
                        resolved_by_canonical,
                        resolved_by_request,
                        hunks,
                        context,
                    )
                },
            ))
        })
    }
}

#[derive(Default)]
struct PatchPathSet {
    resolved_by_request: HashMap<String, PathBuf>,
    resolved_by_canonical: BTreeMap<PathBuf, ResolvedWorkspacePath>,
    accesses: Vec<ToolResourceAccess>,
    capabilities: Vec<CapabilityRequest>,
}

impl PatchPathSet {
    fn collect(
        &mut self,
        workspace: &Workspace,
        requested_path: &str,
        require_existing: bool,
        reject_symlink_leaf: bool,
    ) -> Result<(), ToolError> {
        if self.resolved_by_request.contains_key(requested_path) {
            return Ok(());
        }
        if reject_symlink_leaf {
            let lexical = workspace.resolve(requested_path).map_err(map_path_error)?;
            reject_symlink_entry(&lexical, requested_path).map_err(map_invalid_app_error)?;
        }
        let resolved = if require_existing {
            workspace
                .resolve_for_read(requested_path)
                .map_err(map_path_error)?
        } else {
            workspace
                .resolve_for_write(requested_path)
                .map_err(map_path_error)?
        };
        self.resolved_by_request
            .insert(requested_path.to_string(), resolved.path().to_path_buf());
        if self.resolved_by_canonical.contains_key(resolved.path()) {
            return Ok(());
        }
        self.accesses.extend(write_accesses(&resolved));
        self.capabilities.push(path_request(
            &resolved,
            PathCapability::Write,
            "apply_patch",
        ));
        if require_existing || resolved.state() == WorkspacePathState::Existing {
            self.capabilities
                .push(path_request(&resolved, PathCapability::Read, "apply_patch"));
        }
        self.resolved_by_canonical
            .insert(resolved.path().to_path_buf(), resolved);
        Ok(())
    }
}

/// Existing edit targets collected during prepare. Edit never creates paths, so
/// this set has no missing-write / rename seam.
///
/// Duplicate paths use [`claim_unique_path`] (same predicate as execute) and map
/// to `InvalidArguments` so authorization never starts for a multi-claim doc.
#[derive(Default)]
struct EditTargetSet {
    resolved: BTreeMap<PathBuf, ResolvedWorkspacePath>,
    /// Document claim string per canonical path - shared uniqueness owner.
    claimed_as: BTreeMap<PathBuf, String>,
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

fn execute_prepared_edit(
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
    workspace: Workspace,
    resolved: BTreeMap<PathBuf, ResolvedWorkspacePath>,
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

fn execute_prepared_string_edit(
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
    workspace: Workspace,
    resolved: ResolvedWorkspacePath,
    args: EditFileArgs,
    context: AuthorizedToolContext,
) -> ToolFuture<'static> {
    Box::pin(async move {
        let display = compact_display_path(workspace.root(), &args.path);
        let _ = context
            .progress()
            .send(
                ToolProgress::message(format!("editing {display}"))
                    .metadata(ToolMetadata::new().operation(OperationKind::Write)),
            )
            .await;
        workspace.revalidate(&resolved).map_err(map_path_error)?;
        let mutation_paths = [resolved.path()];
        let outcome = run_observed_mutation(
            mutation_observer.as_ref(),
            &mutation_paths,
            edit_file_content(
                resolved.path(),
                &display,
                &args.old_string,
                &args.new_string,
                args.replace_all,
                max_output_bytes,
            ),
        )
        .await?;
        Ok(mutation_output(outcome))
    })
}

fn execute_prepared_patch(
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
    workspace: Workspace,
    resolved: BTreeMap<PathBuf, ResolvedWorkspacePath>,
    requested_paths: HashMap<String, PathBuf>,
    hunks: Vec<Hunk>,
    context: AuthorizedToolContext,
) -> ToolFuture<'static> {
    Box::pin(async move {
        let total = hunks.len() as u64;
        let _ = context
            .progress()
            .send(
                ToolProgress::message(format!("applying patch ({total} file op(s))"))
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
            apply_hunks(
                hunks,
                |requested| {
                    let path = requested_paths.get(requested).ok_or_else(|| {
                        AppToolError::Message(format!(
                            "patch path '{requested}' was not prepared for this invocation"
                        ))
                    })?;
                    let prepared = resolved.get(path).ok_or_else(|| {
                        AppToolError::Message(format!(
                            "patch target '{}' was not prepared for this invocation",
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
            ),
        )
        .await?;
        let _ = context
            .progress()
            .send(
                ToolProgress::message(format!("updated {} file(s)", outcome.display_paths.len()))
                    .units(total.max(1), total.max(1))
                    .metadata(ToolMetadata::new().operation(OperationKind::Write)),
            )
            .await;
        let mut metadata = ToolMetadata::new().operation(OperationKind::Write);
        for path in &outcome.display_paths {
            metadata = metadata.affected_path(path);
        }
        Ok(ToolOutput::text(outcome.content).metadata(metadata.diff(outcome.diff)))
    })
}

fn map_invalid_edit_args(error: AppToolError) -> ToolError {
    match error {
        AppToolError::Message(message) => ToolError::new(ToolErrorKind::InvalidArguments, message),
        other => map_app_error(other),
    }
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

fn patch_start_metadata(arguments: &Value) -> ToolMetadata {
    let mut metadata = ToolMetadata::new().operation(OperationKind::Write);
    if let Some(input) = arguments.get("input").and_then(Value::as_str) {
        for path in patch_paths_lenient(input) {
            metadata = metadata.affected_path(path);
        }
    }
    metadata
}
