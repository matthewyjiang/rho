use std::{sync::Arc, time::Duration};

use rho_sdk::tool::{
    OperationKind, PreparedToolInvocation, Tool, ToolError, ToolInvocation, ToolMetadata,
    ToolOutput, ToolPreparationContext, ToolPrepareFuture, ToolProgress, ToolSecurity,
};

pub(crate) const NAME: &str = "tui_fixture_progress";

pub(super) fn sdk_bundle(
    processes: Option<super::process::ProcessManager>,
) -> Option<super::sdk_registry::StaticToolBundle> {
    (std::env::var_os("RHO_TUI_TEST_MODE").as_deref() == Some(std::ffi::OsStr::new("matrix"))).then(
        || {
            super::sdk_registry::StaticToolBundle::new(vec![Arc::new(TuiFixtureProgressTool {
                processes,
            })])
        },
    )
}

struct TuiFixtureProgressTool {
    processes: Option<super::process::ProcessManager>,
}

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
                    "delay_ms": {"type": "integer", "minimum": 0},
                    "release_prefix": {"type": "string"}
                },
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let fixture = FixtureRun::from_invocation(&invocation);
        let processes = self.processes.clone();
        let notification = invocation
            .arguments()
            .get("label")
            .and_then(serde_json::Value::as_str)
            == Some("boundary notification");
        Box::pin(async move {
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                ToolMetadata::new().operation(OperationKind::Other("tui_fixture".into())),
                move |context| {
                    Box::pin(async move {
                        if notification {
                            let processes =
                                processes.expect("process capability in boundary fixture");
                            let started = processes
                                .start(
                                    "exit 7".into(),
                                    std::path::Path::new("."),
                                    /*timeout*/ None,
                                )
                                .await
                                .map_err(|error| {
                                    ToolError::new(rho_sdk::tool::ToolErrorKind::Execution, error)
                                })?;
                            loop {
                                let notified = processes.notified_owned();
                                if processes.has_pending_notification() {
                                    break;
                                }
                                notified.await;
                            }
                            return Ok(ToolOutput::text(format!(
                                "background fixture ready: {}",
                                started.process_id
                            )));
                        }
                        send_authorized_progress(&context, &fixture.first_progress, 1).await?;
                        fixture.wait(&context, 1, fixture.delay).await?;
                        send_authorized_progress(&context, &fixture.second_progress, 2).await?;
                        fixture
                            .wait(&context, 2, Duration::from_millis(300))
                            .await?;
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
    release_prefix: Option<String>,
}

impl FixtureRun {
    /// Marker-gated runs hold each update until the PTY observes it. The marker,
    /// not the observation interval (shared with provider fixtures), synchronizes.
    async fn wait(
        &self,
        context: &rho_sdk::tool::AuthorizedToolContext,
        stage: u64,
        delay: Duration,
    ) -> Result<(), ToolError> {
        let Some(prefix) = &self.release_prefix else {
            return authorized_fixture_sleep(context, delay).await;
        };
        let marker = format!("{prefix}-{stage}");
        loop {
            match std::fs::remove_file(&marker) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ToolError::new(
                        rho_sdk::tool::ToolErrorKind::Execution,
                        format!("consume fixture release marker {marker}: {error}"),
                    ));
                }
            }
            authorized_fixture_sleep(context, Duration::from_millis(20)).await?;
        }
    }

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
        let release_prefix = invocation
            .arguments()
            .get("release_prefix")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        match label {
            Some(label) => Self {
                first_progress: format!("{label} progress one"),
                second_progress: format!("{label} progress two"),
                result: format!("{label} result"),
                delay,
                release_prefix,
            },
            None => Self {
                first_progress: "deterministic progress update one".into(),
                second_progress: "deterministic progress update two".into(),
                result: "deterministic fixture tool result".into(),
                delay,
                release_prefix,
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
