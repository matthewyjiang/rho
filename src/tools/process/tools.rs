use super::*;
use crate::tool::*;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

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
pub struct StartProcess(ProcessManager);
impl StartProcess {
    pub fn new(m: ProcessManager) -> Self {
        Self(m)
    }
}
#[derive(Deserialize)]
struct StartArgs {
    command: String,
    timeout_seconds: Option<u64>,
}
#[async_trait::async_trait]
impl Tool for StartProcess {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "start_process".into(),
            description: "Start a managed background shell process.".into(),
            input_schema: json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1}},"required":["command"]}),
        }
    }
    fn display_command(&self, a: &serde_json::Value) -> Option<String> {
        a["command"].as_str().map(str::to_owned)
    }
    async fn call(
        &self,
        a: serde_json::Value,
        c: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let a: StartArgs = serde_json::from_value(a)?;
        let s = self
            .0
            .start(
                a.command,
                &c.cwd,
                a.timeout_seconds.map(Duration::from_secs),
            )
            .await
            .map_err(ToolError::Message)?;
        result!(id, s)
    }
}
#[derive(Clone)]
pub struct PollProcess(ProcessManager);
impl PollProcess {
    pub fn new(m: ProcessManager) -> Self {
        Self(m)
    }
}
#[derive(Deserialize)]
struct PollArgs {
    process_id: String,
    cursor: Option<u64>,
    #[serde(default)]
    wait_seconds: u64,
}
#[async_trait::async_trait]
impl Tool for PollProcess {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "poll_process".into(),
            description: "Read retained process output, optionally waiting for changes.".into(),
            input_schema: json!({"type":"object","properties":{"process_id":{"type":"string"},"cursor":{"type":"integer","minimum":0},"wait_seconds":{"type":"integer","minimum":0,"maximum":30}},"required":["process_id"]}),
        }
    }
    async fn call(
        &self,
        a: serde_json::Value,
        context: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let a: PollArgs = serde_json::from_value(a)?;
        if a.wait_seconds > 30 {
            return Err(ToolError::Message(
                "wait_seconds must be between 0 and 30".into(),
            ));
        }
        let s = self
            .0
            .poll_bounded(
                &a.process_id,
                a.cursor,
                Duration::from_secs(a.wait_seconds),
                context.max_output_bytes,
            )
            .await
            .map_err(ToolError::Message)?;
        result!(id, s)
    }
}
#[derive(Clone)]
pub struct WriteProcess(ProcessManager);
impl WriteProcess {
    pub fn new(m: ProcessManager) -> Self {
        Self(m)
    }
}
#[derive(Deserialize)]
struct WriteArgs {
    process_id: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    close_stdin: bool,
}
#[async_trait::async_trait]
impl Tool for WriteProcess {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_process".into(),
            description: "Write text to a managed process or close its stdin.".into(),
            input_schema: json!({"type":"object","properties":{"process_id":{"type":"string"},"text":{"type":"string"},"close_stdin":{"type":"boolean"}},"required":["process_id"]}),
        }
    }
    async fn call(
        &self,
        a: serde_json::Value,
        _: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let a: WriteArgs = serde_json::from_value(a)?;
        self.0
            .write(&a.process_id, &a.text, a.close_stdin)
            .await
            .map_err(ToolError::Message)?;
        result!(
            id,
            json!({"process_id":a.process_id,"written":a.text.len(),"stdin_closed":a.close_stdin})
        )
    }
}
#[derive(Clone)]
pub struct StopProcess(ProcessManager);
impl StopProcess {
    pub fn new(m: ProcessManager) -> Self {
        Self(m)
    }
}
#[derive(Deserialize)]
struct StopArgs {
    process_id: String,
    grace_seconds: Option<u64>,
}
#[async_trait::async_trait]
impl Tool for StopProcess {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "stop_process".into(),
            description: "Stop a managed process tree gracefully, then forcibly.".into(),
            input_schema: json!({"type":"object","properties":{"process_id":{"type":"string"},"grace_seconds":{"type":"integer","minimum":0}},"required":["process_id"]}),
        }
    }
    async fn call(
        &self,
        a: serde_json::Value,
        _: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let a: StopArgs = serde_json::from_value(a)?;
        self.0
            .stop(
                &a.process_id,
                Duration::from_secs(a.grace_seconds.unwrap_or(2)),
            )
            .await
            .map_err(ToolError::Message)?;
        result!(id, json!({"process_id":a.process_id,"stop_requested":true}))
    }
}
#[derive(Clone)]
pub struct ListProcesses(ProcessManager);
impl ListProcesses {
    pub fn new(m: ProcessManager) -> Self {
        Self(m)
    }
}
#[async_trait::async_trait]
impl Tool for ListProcesses {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_processes".into(),
            description: "List processes owned by this Rho instance.".into(),
            input_schema: json!({"type":"object","properties":{}}),
        }
    }
    async fn call(
        &self,
        _: serde_json::Value,
        _: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        result!(id, self.0.list())
    }
}
