use std::{sync::Arc, time::Duration};

use rho_sdk::tool::{
    OperationKind, PreparedToolInvocation, Tool, ToolContext, ToolError, ToolFuture,
    ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
    ToolProgress, ToolSecurity,
};

pub(crate) const NAME: &str = "tui_fixture_progress";

pub(super) fn sdk_bundle() -> Option<super::sdk_registry::StaticToolBundle> {
    (std::env::var_os("RHO_TUI_TEST_MODE").as_deref() == Some(std::ffi::OsStr::new("matrix")))
        .then(|| super::sdk_registry::StaticToolBundle::new(vec![Arc::new(TuiFixtureProgressTool)]))
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
                "properties": {
                    "label": {"type": "string"},
                    "delay_ms": {"type": "integer", "minimum": 0}
                },
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        let fixture = FixtureRun::from_invocation(&invocation);
        Box::pin(async move {
            send_progress(&context, &fixture.first_progress, 1).await?;
            fixture_sleep(&context, fixture.delay).await?;
            send_progress(&context, &fixture.second_progress, 2).await?;
            fixture_sleep(&context, Duration::from_millis(300)).await?;
            Ok(ToolOutput::text(fixture.result).metadata(
                ToolMetadata::new().operation(OperationKind::Other("tui_fixture".into())),
            ))
        })
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let fixture = FixtureRun::from_invocation(&invocation);
        Box::pin(async move {
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                ToolMetadata::new().operation(OperationKind::Other("tui_fixture".into())),
                move |context| {
                    Box::pin(async move {
                        send_authorized_progress(&context, &fixture.first_progress, 1).await?;
                        authorized_fixture_sleep(&context, fixture.delay).await?;
                        send_authorized_progress(&context, &fixture.second_progress, 2).await?;
                        authorized_fixture_sleep(&context, Duration::from_millis(300)).await?;
                        Ok(ToolOutput::text(fixture.result).metadata(
                            ToolMetadata::new()
                                .operation(OperationKind::Other("tui_fixture".into())),
                        ))
                    })
                },
            ))
        })
    }
}

struct FixtureRun {
    first_progress: String,
    second_progress: String,
    result: String,
    delay: Duration,
}

impl FixtureRun {
    fn from_invocation(invocation: &ToolInvocation) -> Self {
        let label = invocation
            .arguments()
            .get("label")
            .and_then(serde_json::Value::as_str);
        let delay = invocation
            .arguments()
            .get("delay_ms")
            .and_then(serde_json::Value::as_u64)
            .map_or(Duration::from_secs(3), Duration::from_millis);
        match label {
            Some(label) => Self {
                first_progress: format!("{label} progress one"),
                second_progress: format!("{label} progress two"),
                result: format!("{label} result"),
                delay,
            },
            None => Self {
                first_progress: "deterministic progress update one".into(),
                second_progress: "deterministic progress update two".into(),
                result: "deterministic fixture tool result".into(),
                delay,
            },
        }
    }
}

async fn send_authorized_progress(
    context: &rho_sdk::tool::AuthorizedToolContext,
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

async fn authorized_fixture_sleep(
    context: &rho_sdk::tool::AuthorizedToolContext,
    duration: Duration,
) -> Result<(), ToolError> {
    tokio::select! {
        () = tokio::time::sleep(duration) => Ok(()),
        () = context.cancellation().cancelled() => Err(ToolError::cancelled()),
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
