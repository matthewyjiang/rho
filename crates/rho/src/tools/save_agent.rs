//! Host-owned persist for user agent definitions.
//!
//! Models draft Markdown through the guided creator, then call this instead of
//! `write`. Validation, canonical serialization, root authorization, and
//! conflict-aware persistence stay in the agent layer.

use std::sync::Arc;

use serde::Deserialize;

use crate::agent::{
    parse_definition, persist_definition, persist_destination_path, AgentSaveLocation,
    PersistDefinitionError,
};
use crate::workspace::ProjectTrust;
use rho_sdk::{
    tool::{
        OperationKind, PreparedToolInvocation, Tool as SdkTool, ToolError as SdkToolError,
        ToolErrorKind, ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext,
        ToolPrepareFuture, ToolResource, ToolResourceAccess, ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, PathScope,
};

pub(super) const NAME: &str = "save_agent";

pub(super) fn sdk_bundle(max_output_bytes: usize) -> super::sdk_registry::StaticToolBundle {
    super::sdk_registry::StaticToolBundle::new(vec![Arc::new(SaveAgentTool { max_output_bytes })])
}

struct SaveAgentTool {
    max_output_bytes: usize,
}

#[derive(Deserialize)]
struct Args {
    location: Location,
    contents: String,
    #[serde(default)]
    expected_revision: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Location {
    AgentsHome,
    RhoHome,
    Project,
}

impl Location {
    fn save_location(self) -> AgentSaveLocation {
        match self {
            Self::AgentsHome => AgentSaveLocation::AgentsHome,
            Self::RhoHome => AgentSaveLocation::RhoHome,
            Self::Project => AgentSaveLocation::Project,
        }
    }
}

impl SdkTool for SaveAgentTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        rho_sdk::model::ToolSpec {
            name: NAME.into(),
            description: "Validate, canonicalize, and save a Rho agent definition. Use this instead of write or a shell. location is agents-home (~/.agents/agents), rho-home (~/.rho/agents), or project (<project>/.agents/agents). Parent directories are created. Existing files return a revision; pass that expected_revision after the user confirms replacing that exact file.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "enum": ["agents-home", "rho-home", "project"],
                        "description": "Authorized discovery tree for the definition file"
                    },
                    "contents": {
                        "type": "string",
                        "description": "Markdown agent definition with YAML frontmatter and prompt body"
                    },
                    "expected_revision": {
                        "type": "string",
                        "description": "Revision from an exists response. Required to replace that exact file; omit to create only"
                    }
                },
                "required": ["location", "contents"],
                "additionalProperties": false
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Write, CapabilityKind::Read])
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            let args: Args =
                serde_json::from_value(invocation.into_arguments()).map_err(|error| {
                    SdkToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
                })?;
            let cwd = context
                .workspace_root()
                .ok_or_else(|| {
                    SdkToolError::new(
                        ToolErrorKind::InvalidArguments,
                        "save_agent requires a workspace",
                    )
                })?
                .to_path_buf();
            let home = crate::paths::home_dir();
            let location = args.location.save_location();
            let draft = parse_definition(std::path::Path::new("<draft>"), "draft", &args.contents)
                .map_err(|error| {
                    SdkToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
                })?;
            let dest = persist_destination_path(
                location,
                &cwd,
                home.as_deref(),
                ProjectTrust::from_agents_env(),
                draft.id.as_str(),
            )
            .map_err(map_persist_error)?;
            let scope = path_scope(&cwd, &dest);
            let source = CapabilitySource::built_in_tool(NAME);
            // Execute always inspects the destination. Authorize read even when
            // prepare does not see the file, so a create-after-prepare race
            // cannot disclose contents without Read.
            let capabilities = vec![
                CapabilityRequest::write_path(dest.clone(), scope.clone(), source.clone()),
                CapabilityRequest::read_path(dest.clone(), scope, source),
            ];
            let access = ToolResourceAccess::exclusive(ToolResource::workspace_path(&dest));
            let max_output_bytes = self.max_output_bytes;
            Ok(PreparedToolInvocation::resource_aware(
                [access],
                capabilities,
                ToolMetadata::new().operation(OperationKind::Write),
                move |_context| {
                    Box::pin(async move { execute_save(args, cwd, home, max_output_bytes) })
                },
            ))
        })
    }
}

fn execute_save(
    args: Args,
    cwd: std::path::PathBuf,
    home: Option<std::path::PathBuf>,
    max_output_bytes: usize,
) -> Result<ToolOutput, SdkToolError> {
    match persist_definition(
        args.location.save_location(),
        &args.contents,
        args.expected_revision.as_deref(),
        &cwd,
        home.as_deref(),
        ProjectTrust::from_agents_env(),
    ) {
        Ok(outcome) => {
            let action = if outcome.created {
                "created"
            } else {
                "updated"
            };
            let path = crate::paths::display(&outcome.path);
            let content = format!("{action} {path}\n\n{}", outcome.contents);
            Ok(ToolOutput::text(rho_tools::tool::truncate(
                content,
                max_output_bytes,
            )))
        }
        Err(PersistDefinitionError::Exists {
            path,
            contents,
            revision,
        }) => {
            let path = crate::paths::display(&path);
            let content = format!(
                "exists {path}\nrevision {revision}\nset expected_revision to this value after the user confirms replacing this exact file\n\n{contents}"
            );
            Ok(ToolOutput::text(rho_tools::tool::truncate(
                content,
                max_output_bytes,
            )))
        }
        Err(error) => Err(map_persist_error(error)),
    }
}

fn path_scope(cwd: &std::path::Path, dest: &std::path::Path) -> PathScope {
    if dest.starts_with(cwd) {
        PathScope::PrimaryWorkspace
    } else {
        PathScope::UnrestrictedFilesystem
    }
}

fn map_persist_error(error: PersistDefinitionError) -> SdkToolError {
    let kind = match error {
        PersistDefinitionError::Validation(_) => ToolErrorKind::InvalidArguments,
        PersistDefinitionError::Unauthorized(_) => ToolErrorKind::PolicyDenied,
        PersistDefinitionError::Exists { .. }
        | PersistDefinitionError::Conflict
        | PersistDefinitionError::Write(_) => ToolErrorKind::Execution,
    };
    SdkToolError::new(kind, error.to_string())
}

#[cfg(test)]
#[path = "save_agent_tests.rs"]
mod tests;
