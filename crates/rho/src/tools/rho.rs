use async_trait::async_trait;
use serde::Deserialize;

use {
    crate::diagnostics::RuntimeDiagnostics,
    rho_tools::tool::{Tool, ToolContext, ToolError, ToolResult, ToolSpec},
};

pub(super) fn sdk_bundle(
    diagnostics: RuntimeDiagnostics,
    max_output_bytes: usize,
) -> super::sdk_registry::StaticToolBundle {
    let tool = rho_tools::legacy_sdk_adapter::rho(Rho::new(diagnostics), max_output_bytes)
        .expect("rho is a supported legacy tool");
    super::sdk_registry::StaticToolBundle::new(vec![tool])
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
