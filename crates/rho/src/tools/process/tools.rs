use super::*;
use rho_tools::tool::*;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const STOP_GRACE: Duration = Duration::from_secs(2);

macro_rules! result {
    ($id:expr,$value:expr) => {
        Ok(ToolResult {
            id: $id,
            ok: true,
            content: serde_json::to_string(&$value).unwrap(),
        })
    };
}

#[derive(Clone)]
pub struct Process(ProcessManager);

impl Process {
    pub fn new(manager: ProcessManager) -> Self {
        Self(manager)
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ProcessArgs {
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

#[async_trait::async_trait]
impl Tool for Process {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "process".into(),
            description: "Manage a background process: start requires command; poll and stop require process_id.".into(),
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

    async fn call(
        &self,
        args: serde_json::Value,
        context: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        self.call_with_updates(args, context, id, &mut |_| {}).await
    }

    async fn call_with_updates(
        &self,
        args: serde_json::Value,
        context: ToolContext,
        id: String,
        on_update: &mut (dyn FnMut(Vec<String>) + Send),
    ) -> Result<ToolResult, ToolError> {
        match serde_json::from_value(args)? {
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
                result!(id, snapshot)
            }
            ProcessArgs::Poll {
                process_id,
                cursor,
                wait_seconds,
            } => {
                if wait_seconds > 30 {
                    return Err(ToolError::Message(
                        "wait_seconds must be between 0 and 30".into(),
                    ));
                }
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
                result!(id, snapshot)
            }
            ProcessArgs::Stop { process_id } => {
                self.0
                    .stop(&process_id, STOP_GRACE)
                    .await
                    .map_err(ToolError::Message)?;
                on_update(vec![format!("stop requested: {process_id}")]);
                result!(id, json!({"process_id":process_id,"stop_requested":true}))
            }
        }
    }
}
