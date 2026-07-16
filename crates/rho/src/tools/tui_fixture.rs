use std::{sync::Arc, time::Duration};

use rho_sdk::tool::{
    OperationKind, Tool, ToolContext, ToolError, ToolFuture, ToolInvocation, ToolMetadata,
    ToolOutput, ToolProgress, ToolSecurity,
};

pub(super) const NAME: &str = "tui_fixture_progress";

pub(super) fn from_env() -> Option<Arc<dyn Tool>> {
    (std::env::var_os("RHO_TUI_TEST_MODE").as_deref() == Some(std::ffi::OsStr::new("matrix")))
        .then(|| Arc::new(TuiFixtureProgressTool) as Arc<dyn Tool>)
}

struct TuiFixtureProgressTool;

impl Tool for TuiFixtureProgressTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        rho_sdk::model::ToolSpec {
            name: NAME.into(),
            description: "Deterministic progress fixture for source-build TUI smoke tests.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {},
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            send_progress(&context, "deterministic progress update one", 1).await?;
            fixture_sleep(&context, Duration::from_secs(3)).await?;
            send_progress(&context, "deterministic progress update two", 2).await?;
            fixture_sleep(&context, Duration::from_millis(300)).await?;
            Ok(
                ToolOutput::text("deterministic fixture tool result").metadata(
                    ToolMetadata::new().operation(OperationKind::Other("tui_fixture".into())),
                ),
            )
        })
    }
}

async fn send_progress(
    context: &ToolContext,
    message: &str,
    completed: u64,
) -> Result<(), ToolError> {
    if !context
        .progress()
        .send(
            ToolProgress::message(message).units(completed, 2).metadata(
                ToolMetadata::new().operation(OperationKind::Other("tui_fixture".into())),
            ),
        )
        .await
    {
        return Err(ToolError::cancelled());
    }
    Ok(())
}

async fn fixture_sleep(context: &ToolContext, duration: Duration) -> Result<(), ToolError> {
    tokio::select! {
        () = tokio::time::sleep(duration) => Ok(()),
        () = context.cancellation().cancelled() => Err(ToolError::cancelled()),
    }
}
