use rho_sdk::tool::{
    OperationKind, PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture,
    ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
    ToolResource, ToolResourceAccess, ToolSecurity,
};
use rho_tools::tool::Tool as LegacyTool;

use super::{adapters::GetSearchContentArgs, GetSearchContent};

pub(crate) struct SdkGetSearchContent {
    max_output_bytes: usize,
}

impl SdkGetSearchContent {
    pub(crate) fn new(max_output_bytes: usize) -> Self {
        Self { max_output_bytes }
    }

    fn execute(&self, args: GetSearchContentArgs) -> Result<ToolOutput, ToolError> {
        let result = GetSearchContent
            .execute(args, self.max_output_bytes, String::new())
            .map_err(map_legacy_error)?;
        if !result.ok {
            return Err(ToolError::new(ToolErrorKind::Execution, result.content));
        }
        Ok(ToolOutput::text(result.content).metadata(metadata()))
    }
}

impl Tool for SdkGetSearchContent {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        GetSearchContent.spec()
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
        let args = parse_args(invocation.into_arguments());
        Box::pin(async move {
            let args = args?;
            let access =
                ToolResourceAccess::shared(ToolResource::response_store(&args.response_id));
            Ok(PreparedToolInvocation::resource_aware(
                [access],
                [],
                metadata(),
                move |_context| Box::pin(async move { self.execute(args) }),
            ))
        })
    }
}

fn parse_args(arguments: serde_json::Value) -> Result<GetSearchContentArgs, ToolError> {
    let args: GetSearchContentArgs = serde_json::from_value(arguments)
        .map_err(|error| ToolError::new(ToolErrorKind::InvalidArguments, error.to_string()))?;
    super::storage::validate_response_id(&args.response_id)
        .map_err(|error| ToolError::new(ToolErrorKind::InvalidArguments, error.to_string()))?;
    Ok(args)
}

fn metadata() -> ToolMetadata {
    ToolMetadata::new().operation(OperationKind::Read)
}

fn map_legacy_error(error: rho_tools::tool::ToolError) -> ToolError {
    match error {
        rho_tools::tool::ToolError::InvalidArguments(error) => {
            ToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
        }
        error => ToolError::new(ToolErrorKind::Execution, error.to_string()),
    }
}
