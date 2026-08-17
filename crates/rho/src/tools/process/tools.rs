use super::{output, *};
use rho_tools::tool::*;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const STOP_GRACE: Duration = Duration::from_secs(2);

fn result(id: String, content: String) -> Result<ToolResult, ToolError> {
    Ok(ToolResult {
        id,
        ok: true,
        content,
    })
}

#[derive(Clone)]
pub struct Process(ProcessManager);

impl Process {
    pub fn new(manager: ProcessManager) -> Self {
        Self(manager)
    }

    pub(super) async fn start_execution(
        &self,
        execution: rho_sdk::ProcessExecution,
    ) -> Result<Snapshot, String> {
        self.0.start_execution(execution).await
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum ProcessArgs {
    Start {
        command: String,
        timeout_seconds: Option<u64>,
    },
    Poll {
        process_id: String,
        cursor: Option<u64>,
        #[serde(default)]
        wait_seconds: u64,
    },
    Stop {
        process_id: String,
    },
}

impl ProcessArgs {
    pub(super) fn parse(args: serde_json::Value) -> Result<Self, ToolError> {
        let args: Self = serde_json::from_value(args)?;
        if matches!(
            args,
            Self::Poll {
                wait_seconds: 31..,
                ..
            }
        ) {
            return Err(ToolError::Message(
                "wait_seconds must be between 0 and 30".into(),
            ));
        }
        Ok(args)
    }
}

impl Tool for Process {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "process".into(),
            description: "Manage a background process: start requires command; poll and stop require process_id. Started processes deliver their result when they exit; do not poll in a loop.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["start", "poll", "stop"]},
                    "command": {"type": "string"},
                    "timeout_seconds": {"type": "integer", "minimum": 1},
                    "process_id": {"type": "string"},
                    "cursor": {"type": "integer", "minimum": 0},
                    "wait_seconds": {"type": "integer", "minimum": 0, "maximum": 30}
                },
                "required": ["action"]
            }),
        }
    }

    fn call<'a>(
        &'a self,
        args: serde_json::Value,
        context: ToolContext,
        id: String,
    ) -> AppToolFuture<'a> {
        Box::pin(async move { self.call_with_updates(args, context, id, &mut |_| {}).await })
    }

    fn call_with_updates<'a>(
        &'a self,
        args: serde_json::Value,
        context: ToolContext,
        id: String,
        on_update: &'a mut (dyn FnMut(Vec<String>) + Send),
    ) -> AppToolFuture<'a> {
        Box::pin(async move {
            self.execute(ProcessArgs::parse(args)?, context, id, on_update)
                .await
        })
    }
}

impl Process {
    pub(super) async fn execute(
        &self,
        args: ProcessArgs,
        context: ToolContext,
        id: String,
        on_update: &mut (dyn FnMut(Vec<String>) + Send),
    ) -> Result<ToolResult, ToolError> {
        match args {
            ProcessArgs::Start {
                command,
                timeout_seconds,
            } => {
                let snapshot = self
                    .0
                    .start(
                        command,
                        &context.cwd,
                        timeout_seconds.map(Duration::from_secs),
                    )
                    .await
                    .map_err(ToolError::Message)?;
                on_update(display::snapshot_progress_lines(&snapshot));
                result(id, output::format_snapshot(&snapshot))
            }
            ProcessArgs::Poll {
                process_id,
                cursor,
                wait_seconds,
            } => {
                let snapshot = self
                    .0
                    .poll_bounded(
                        &process_id,
                        cursor,
                        Duration::from_secs(wait_seconds),
                        context.max_output_bytes,
                    )
                    .await
                    .map_err(ToolError::Message)?;
                on_update(display::snapshot_progress_lines(&snapshot));
                result(id, output::format_snapshot(&snapshot))
            }
            ProcessArgs::Stop { process_id } => {
                self.0
                    .stop(&process_id, STOP_GRACE)
                    .await
                    .map_err(ToolError::Message)?;
                on_update(vec![format!("stop requested: {process_id}")]);
                result(id, output::format_stop(&process_id))
            }
        }
    }
}
