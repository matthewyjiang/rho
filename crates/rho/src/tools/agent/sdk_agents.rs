use std::sync::Arc;

use rho_sdk::tool::{
    OperationKind, PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture,
    ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
    ToolSecurity,
};
use rho_tools::tool::{Tool as LegacyTool, ToolContext as LegacyToolContext};

use super::AgentsTool;

pub(super) struct SdkAgents {
    inner: AgentsTool,
    max_output_bytes: usize,
}

impl SdkAgents {
    pub(super) fn new(inner: AgentsTool, max_output_bytes: usize) -> Self {
        Self {
            inner,
            max_output_bytes,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        call_id: String,
        context: &rho_sdk::tool::AuthorizedToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let cwd = context
            .workspace_root()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let execution = self.inner.call(
            arguments,
            LegacyToolContext {
                cwd,
                max_output_bytes: self.max_output_bytes,
            },
            call_id,
        );
        let result = tokio::select! {
            result = execution => result,
            () = context.cancellation().cancelled() => return Err(ToolError::cancelled()),
        }
        .map_err(map_legacy_error)?;
        if !result.ok {
            return Err(ToolError::new(ToolErrorKind::Execution, result.content));
        }
        Ok(ToolOutput::text(result.content).metadata(metadata()))
    }
}

impl Tool for SdkAgents {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        self.inner.spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        rho_sdk::tool::call_prepared(self, invocation, context)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let call_id = invocation.id().to_string();
        let arguments = invocation.into_arguments();
        Box::pin(async move {
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                metadata(),
                move |context| {
                    Box::pin(async move { self.execute(arguments, call_id, &context).await })
                },
            ))
        })
    }
}

fn metadata() -> ToolMetadata {
    ToolMetadata::new().operation(OperationKind::Other("agents".into()))
}

fn map_legacy_error(error: rho_tools::tool::ToolError) -> ToolError {
    match error {
        rho_tools::tool::ToolError::InvalidArguments(error) => {
            ToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
        }
        rho_tools::tool::ToolError::Message(message) if message == "tool interrupted" => {
            ToolError::cancelled()
        }
        error => ToolError::new(ToolErrorKind::Execution, error.to_string()),
    }
}

pub(super) fn tool(inner: AgentsTool, max_output_bytes: usize) -> Arc<dyn Tool> {
    Arc::new(SdkAgents::new(inner, max_output_bytes))
}
