use std::sync::Arc;

use rho_sdk::tool::{
    OperationKind, PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture,
    ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
    ToolProgress, ToolSecurity,
};
use rho_tools::tool::{Tool as LegacyTool, ToolContext as LegacyToolContext};

use super::AgentTool;

pub(super) struct SdkAgent {
    inner: AgentTool,
    max_output_bytes: usize,
}

impl SdkAgent {
    pub(super) fn new(inner: AgentTool, max_output_bytes: usize) -> Self {
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
        let (update_sender, mut updates) = tokio::sync::mpsc::unbounded_channel::<Vec<String>>();
        let mut on_update = move |lines: Vec<String>| {
            let _ = update_sender.send(lines);
        };
        let call = self.inner.call_with_updates_and_cancellation(
            arguments,
            LegacyToolContext {
                cwd,
                max_output_bytes: self.max_output_bytes,
            },
            call_id,
            context.cancellation().clone(),
            &mut on_update,
        );
        tokio::pin!(call);
        let mut updates_open = true;
        let result = loop {
            tokio::select! {
                result = &mut call => break result,
                update = updates.recv(), if updates_open => {
                    match update {
                        Some(lines) => {
                            let _ = context
                                .progress()
                                .send(ToolProgress::message(lines.join("\n")))
                                .await;
                        }
                        None => updates_open = false,
                    }
                }
            }
        };
        while let Ok(lines) = updates.try_recv() {
            let _ = context
                .progress()
                .send(ToolProgress::message(lines.join("\n")))
                .await;
        }
        let result = result.map_err(map_legacy_error)?;
        if !result.ok {
            return Err(ToolError::new(ToolErrorKind::Execution, result.content));
        }
        Ok(ToolOutput::text(result.content).metadata(metadata()))
    }
}

impl Tool for SdkAgent {
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
    ToolMetadata::new().operation(OperationKind::Other("agent".into()))
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

pub(super) fn tool(inner: AgentTool, max_output_bytes: usize) -> Arc<dyn Tool> {
    Arc::new(SdkAgent::new(inner, max_output_bytes))
}
