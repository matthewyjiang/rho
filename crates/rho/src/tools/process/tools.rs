use super::{output, *};
use rho_tools::tool::*;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const STOP_GRACE: Duration = Duration::from_secs(2);

/// Whether `process start` advertises `keep_warm`. Only the interactive TUI
/// runs a cache warmer, so every other host keeps it hidden.
#[derive(Clone, Debug, Default)]
pub(crate) enum KeepWarmOffer {
    #[default]
    Hidden,
    /// Advertised; the route says whether a requested hold can warm right now.
    Offered(CacheWarmRoute),
}

#[derive(Clone)]
pub struct Process {
    manager: ProcessManager,
    keep_warm: KeepWarmOffer,
}

impl Process {
    pub fn new(manager: ProcessManager) -> Self {
        Self {
            manager,
            keep_warm: KeepWarmOffer::Hidden,
        }
    }

    pub(crate) fn with_keep_warm(mut self, offer: KeepWarmOffer) -> Self {
        self.keep_warm = offer;
        self
    }

    /// The hold requested by `args`, honored only when the option is offered
    /// and the bound model route can send a capped ping.
    pub(super) fn keep_warm_for(&self, requested: bool) -> KeepWarm {
        match (&self.keep_warm, requested) {
            (KeepWarmOffer::Offered(route), true) if route.is_supported() => KeepWarm::On,
            (KeepWarmOffer::Offered(_), true) => KeepWarm::Unavailable,
            (KeepWarmOffer::Offered(_) | KeepWarmOffer::Hidden, _) => KeepWarm::Off,
        }
    }

    pub(super) async fn start_execution(
        &self,
        execution: rho_sdk::ProcessExecution,
        keep_warm: KeepWarm,
    ) -> Result<Snapshot, String> {
        self.manager.start_execution(execution, keep_warm).await
    }
}

const KEEP_WARM_DESCRIPTION: &str = " Set keep_warm on start only when you will end your turn and sit idle waiting for this process for more than about 5 minutes (long builds, test suites, training runs); Rho then sends small capped requests that keep the provider prompt cache warm so your next turn is not re-billed in full. Do not set it for quick commands, servers, watchers, or while you keep working. Warming stops when the process exits or is stopped.";

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum ProcessArgs {
    Start {
        command: String,
        timeout_seconds: Option<u64>,
        #[serde(default)]
        keep_warm: bool,
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
        let mut description = "Manage a background process: start requires command; poll and stop require process_id. Started processes deliver their result when they exit; do not poll in a loop.".to_string();
        let mut input_schema = json!({
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
        });
        match self.keep_warm {
            KeepWarmOffer::Offered(_) => {
                description.push_str(KEEP_WARM_DESCRIPTION);
                input_schema["properties"]["keep_warm"] = json!({"type": "boolean"});
            }
            KeepWarmOffer::Hidden => {}
        }
        ToolSpec {
            name: "process".into(),
            description,
            input_schema,
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
            self.execute(ProcessArgs::parse(args)?, context, on_update)
                .await
                .map(|run| run.into_result(id))
        })
    }
}

impl Process {
    pub(super) async fn execute(
        &self,
        args: ProcessArgs,
        context: ToolContext,
        on_update: &mut (dyn FnMut(Vec<String>) + Send),
    ) -> Result<rho_tools::Rendered<output::ProcessOutput>, ToolError> {
        match args {
            ProcessArgs::Start {
                command,
                timeout_seconds,
                keep_warm,
            } => {
                let keep_warm = self.keep_warm_for(keep_warm);
                let snapshot = self
                    .manager
                    .start_with_keep_warm(
                        command,
                        &context.cwd,
                        timeout_seconds.map(Duration::from_secs),
                        keep_warm,
                    )
                    .await
                    .map_err(ToolError::Message)?;
                on_update(display::snapshot_progress_lines(&snapshot));
                Ok(output::render_start(snapshot, keep_warm))
            }
            ProcessArgs::Poll {
                process_id,
                cursor,
                wait_seconds,
            } => {
                let snapshot = self
                    .manager
                    .poll_bounded(
                        &process_id,
                        cursor,
                        Duration::from_secs(wait_seconds),
                        context.max_output_bytes,
                    )
                    .await
                    .map_err(ToolError::Message)?;
                on_update(display::snapshot_progress_lines(&snapshot));
                Ok(output::render_snapshot(snapshot))
            }
            ProcessArgs::Stop { process_id } => {
                self.manager
                    .stop(&process_id, STOP_GRACE)
                    .await
                    .map_err(ToolError::Message)?;
                on_update(vec![format!("stop requested: {process_id}")]);
                Ok(rho_tools::Rendered::new(
                    output::format_stop(&process_id),
                    output::ProcessOutput::StopRequested { process_id },
                ))
            }
        }
    }
}
