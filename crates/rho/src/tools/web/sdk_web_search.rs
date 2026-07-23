use rho_sdk::{
    tool::{
        OperationKind, PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind,
        ToolFuture, ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext,
        ToolPrepareFuture, ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, NetworkTarget,
};
use rho_tools::tool::{Tool as LegacyTool, ToolContext as LegacyToolContext};

use super::WebSearch;

const WEB_SEARCH_TOOL: &str = "web_search";

pub(crate) struct SdkWebSearch {
    inner: WebSearch,
    max_output_bytes: usize,
}

impl SdkWebSearch {
    pub(crate) fn new(inner: WebSearch, max_output_bytes: usize) -> Self {
        Self {
            inner,
            max_output_bytes,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
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
            String::new(),
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

impl Tool for SdkWebSearch {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        self.inner.spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Network])
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        rho_sdk::tool::call_prepared(self, invocation, context)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let arguments = invocation.into_arguments();
        Box::pin(async move {
            let capability = CapabilityRequest::network(
                NetworkTarget::ToolManaged,
                CapabilitySource::built_in_tool(WEB_SEARCH_TOOL),
            );
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [capability],
                metadata(),
                move |context| Box::pin(async move { self.execute(arguments, &context).await }),
            ))
        })
    }
}

fn metadata() -> ToolMetadata {
    ToolMetadata::new().operation(OperationKind::Network)
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
