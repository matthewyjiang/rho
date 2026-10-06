use std::{path::Path, sync::Arc};

use serde::Deserialize;

use {
    crate::diagnostics::RuntimeDiagnostics,
    rho_sdk::tool::{
        OperationKind, PreparedToolInvocation, Tool as SdkTool, ToolError as SdkToolError,
        ToolErrorKind, ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext,
        ToolPrepareFuture, ToolSecurity,
    },
    rho_tools::tool::{AppToolFuture, Tool, ToolContext, ToolError, ToolResult, ToolSpec},
};

pub(super) fn sdk_bundle(
    diagnostics: RuntimeDiagnostics,
    max_output_bytes: usize,
) -> super::sdk_registry::StaticToolBundle {
    super::sdk_registry::StaticToolBundle::new(vec![Arc::new(SdkRho::with_max_output_bytes(
        diagnostics,
        max_output_bytes,
    ))])
}

pub(super) struct SdkRho {
    diagnostics: RuntimeDiagnostics,
    max_output_bytes: usize,
}

impl SdkRho {
    #[cfg(test)]
    pub(super) fn new(diagnostics: RuntimeDiagnostics) -> Self {
        Self::with_max_output_bytes(diagnostics, rho_tools::DEFAULT_MAX_OUTPUT_BYTES)
    }

    fn with_max_output_bytes(diagnostics: RuntimeDiagnostics, max_output_bytes: usize) -> Self {
        Self {
            diagnostics,
            max_output_bytes: max_output_bytes.max(1),
        }
    }

    fn execute(&self, args: Args, cwd: Option<&Path>) -> Result<ToolOutput, SdkToolError> {
        respond(&self.diagnostics, &args.action, cwd)
            .map(|content| {
                ToolOutput::text(rho_tools::tool::truncate(content, self.max_output_bytes))
            })
            .map_err(|message| SdkToolError::new(ToolErrorKind::InvalidArguments, message))
    }
}

impl SdkTool for SdkRho {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        Rho::new(self.diagnostics.clone()).spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let args = parse_args(invocation.into_arguments());
        let cwd = context.workspace_root().map(Path::to_path_buf);
        Box::pin(async move {
            let args = args?;
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                ToolMetadata::new().operation(OperationKind::Read),
                move |_context| Box::pin(async move { self.execute(args, cwd.as_deref()) }),
            ))
        })
    }
}

/// Answers one action: `agents` from disk for `cwd`, the rest from the
/// diagnostics snapshot.
fn respond(
    diagnostics: &RuntimeDiagnostics,
    action: &str,
    cwd: Option<&Path>,
) -> Result<String, String> {
    if action != crate::diagnostics::AGENTS_ACTION {
        return diagnostics.response(action);
    }
    let cwd = cwd.ok_or("rho agents action requires a workspace")?;
    let report = crate::agent::check_agents(
        cwd,
        crate::paths::home_dir().as_deref(),
        crate::workspace::ProjectTrust::from_agents_env(),
    );
    serde_json::to_string_pretty(&report).map_err(|error| error.to_string())
}

fn parse_args(arguments: serde_json::Value) -> Result<Args, SdkToolError> {
    let args: Args = serde_json::from_value(arguments)
        .map_err(|error| SdkToolError::new(ToolErrorKind::InvalidArguments, error.to_string()))?;
    if crate::diagnostics::supports_action(&args.action) {
        Ok(args)
    } else {
        Err(SdkToolError::new(
            ToolErrorKind::InvalidArguments,
            crate::diagnostics::unsupported_action_error(&args.action),
        ))
    }
}

pub struct Rho {
    diagnostics: RuntimeDiagnostics,
}

impl Rho {
    pub fn new(diagnostics: RuntimeDiagnostics) -> Self {
        Self { diagnostics }
    }
}

#[derive(Deserialize)]
struct Args {
    action: String,
}

impl Tool for Rho {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rho".into(),
            description: "Inspect the running Rho harness. Request only what you need: info returns runtime identity; context returns token usage; compaction returns context accounting, thresholds, and the last automatic-compaction decisions; prompt_sources returns source paths and byte contributions without contents; tools returns available tool names; hooks returns sanitized hook configuration and activity; config returns sanitized live configuration; agents rereads agent definition files and returns their save directories plus each loaded agent's path, or the first invalid file and field."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "Read-only diagnostics action",
                        "enum": crate::diagnostics::ACTIONS
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
        }
    }

    fn call<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> AppToolFuture<'a> {
        Box::pin(async move {
            let args: Args = serde_json::from_value(args)?;
            let content = respond(&self.diagnostics, &args.action, Some(&ctx.cwd))
                .map_err(ToolError::Message)?;
            Ok(ToolResult {
                id,
                ok: true,
                content,
            })
        })
    }
}

#[cfg(test)]
#[path = "rho_tests.rs"]
mod tests;
