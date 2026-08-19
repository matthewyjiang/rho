//! SDK adapter for the in-process `grep` and `glob` workspace tools.
//!
//! One adapter serves every [`WorkspaceSearch`]: it requests a single `Read`
//! capability on the resolved search root and declares a shared directory-tree
//! resource so concurrent walks can overlap safely. Adding a search tool means
//! implementing the trait, not writing another adapter. Like the filesystem
//! adapters, these implement [`rho_sdk::tool::Tool::prepare`] only and need the
//! published default [`rho_sdk::tool::Tool::call`] body.

use serde_json::Value;

use rho_sdk::{
    tool::{
        OperationKind, PreparedToolInvocation, Tool, ToolError, ToolErrorKind, ToolInvocation,
        ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture, ToolResource,
        ToolResourceAccess, ToolSecurity,
    },
    CapabilityKind,
};

use crate::{
    file_view::FileViewPolicy,
    glob::GlobSearch,
    grep::GrepSearch,
    sdk_support::{
        check_preparation_cancelled, map_app_error, map_path_error, path_request,
        preparation_workspace, PathCapability,
    },
    search::WorkspaceSearch,
    tool::{compact_display_path, truncate},
};

/// SDK adapter for [`GrepSearch`].
pub(crate) type GrepTool = SearchTool<GrepSearch>;
/// SDK adapter for [`GlobSearch`].
pub(crate) type GlobTool = SearchTool<GlobSearch>;

pub(crate) struct SearchTool<S> {
    max_output_bytes: usize,
    search: S,
}

impl SearchTool<GrepSearch> {
    pub(crate) fn grep(max_output_bytes: usize, file_view: FileViewPolicy) -> Self {
        Self {
            max_output_bytes: max_output_bytes.max(1),
            search: GrepSearch::new(file_view),
        }
    }
}

impl SearchTool<GlobSearch> {
    pub(crate) fn new(max_output_bytes: usize) -> Self {
        Self {
            max_output_bytes: max_output_bytes.max(1),
            search: GlobSearch,
        }
    }
}

fn start_metadata(arguments: &Value) -> ToolMetadata {
    let mut metadata = ToolMetadata::new().operation(OperationKind::Read);
    if let Some(path) = arguments.get("path").and_then(Value::as_str) {
        metadata = metadata.affected_path(path);
    }
    metadata
}

impl<S: WorkspaceSearch> Tool for SearchTool<S> {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        S::spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Read])
    }

    fn start_metadata(&self, arguments: &Value) -> ToolMetadata {
        start_metadata(arguments)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            check_preparation_cancelled(&context)?;
            let arguments = invocation.into_arguments();
            let metadata = start_metadata(&arguments);
            // Parsing before resolving keeps an invalid pattern from costing an
            // authorization round trip.
            let request = S::parse(arguments).map_err(map_app_error)?;
            let workspace = preparation_workspace(&context)?.clone();
            let requested_root = S::root(&request).to_owned();
            let resolved = workspace
                .resolve_for_read(&requested_root)
                .map_err(map_path_error)?;
            let capability = path_request(&resolved, PathCapability::Read, S::NAME);
            let accesses = [ToolResourceAccess::shared(ToolResource::directory_tree(
                resolved.path(),
            ))];
            let max_output_bytes = self.max_output_bytes;
            let search = self.search.clone();
            Ok(PreparedToolInvocation::resource_aware(
                accesses,
                [capability],
                metadata,
                move |context| {
                    Box::pin(async move {
                        workspace.revalidate(&resolved).map_err(map_path_error)?;
                        let display = compact_display_path(workspace.root(), &requested_root);
                        let root = resolved.path().to_path_buf();
                        let cancellation = context.cancellation().clone();
                        let content = tokio::task::spawn_blocking({
                            let display = display.clone();
                            move || {
                                search
                                    .run(&root, &display, &request, &|| cancellation.is_cancelled())
                            }
                        })
                        .await
                        .map_err(|error| {
                            ToolError::new(
                                ToolErrorKind::Execution,
                                format!("{} task failed: {error}", S::NAME),
                            )
                        })?
                        .map_err(map_app_error)?;
                        Ok(
                            ToolOutput::text(truncate(content, max_output_bytes)).metadata(
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

#[cfg(test)]
#[path = "sdk_search_tests.rs"]
mod tests;
