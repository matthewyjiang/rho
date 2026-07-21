use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use {
    crate::diagnostics::RuntimeDiagnostics,
    rho_sdk::tool::{
        OperationKind, PreparedToolInvocation, Tool as SdkTool, ToolContext as SdkToolContext,
        ToolError as SdkToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolMetadata,
        ToolOutput, ToolPreparationContext, ToolPrepareFuture, ToolSecurity,
    },
    rho_tools::tool::{Tool, ToolContext, ToolError, ToolResult, ToolSpec},
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

    fn execute(&self, args: Args) -> Result<ToolOutput, SdkToolError> {
        self.diagnostics
            .response(&args.action)
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

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: SdkToolContext) -> ToolFuture<'a> {
        Box::pin(async move { self.execute(parse_args(invocation.into_arguments())?) })
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let args = parse_args(invocation.into_arguments());
        Box::pin(async move {
            let args = args?;
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                ToolMetadata::new().operation(OperationKind::Read),
                move |_context| Box::pin(async move { self.execute(args) }),
            ))
        })
    }
}

fn parse_args(arguments: serde_json::Value) -> Result<Args, SdkToolError> {
    let args: Args = serde_json::from_value(arguments)
        .map_err(|error| SdkToolError::new(ToolErrorKind::InvalidArguments, error.to_string()))?;
    if matches!(
        args.action.as_str(),
        "info" | "context" | "prompt_sources" | "tools" | "config"
    ) {
        Ok(args)
    } else {
        Err(SdkToolError::new(
            ToolErrorKind::InvalidArguments,
            format!(
                "unknown rho diagnostics action '{}'; load the rho-diagnostics skill for usage",
                args.action
            ),
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

#[async_trait]
impl Tool for Rho {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rho".into(),
            description: "Inspect the running Rho harness. Use info for basic runtime identity."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "Read-only diagnostics action"
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        _ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let content = self
            .diagnostics
            .response(&args.action)
            .map_err(ToolError::Message)?;
        Ok(ToolResult {
            id,
            ok: true,
            content,
        })
    }
}

#[cfg(test)]
#[path = "rho_tests.rs"]
mod tests;
