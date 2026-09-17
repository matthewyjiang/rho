use crate::cancellation::RunCancellation;
use crate::shell_process::{self, ProcessSupervisor, ShellArgs};
use crate::tool::*;
use rho_sdk::{ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits};
use serde_json::json;
use tokio::process::Command;

pub struct PowerShell {
    rtk_enabled: bool,
}

impl PowerShell {
    pub const fn new(rtk_enabled: bool) -> Self {
        Self { rtk_enabled }
    }
}

impl Tool for PowerShell {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "powershell".into(),
            description: "Runs a PowerShell command in the current working directory.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_seconds": {"type": "integer", "minimum": 1}
                },
                "required": ["command"]
            }),
        }
    }

    fn call<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> AppToolFuture<'a> {
        Box::pin(async move { self.call_with_updates(args, ctx, id, &mut |_| {}).await })
    }

    fn call_with_updates<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
        on_update: &'a mut (dyn FnMut(Vec<String>) + Send),
    ) -> AppToolFuture<'a> {
        Box::pin(async move {
            self.call_with_updates_and_cancellation(
                args,
                ctx,
                id,
                RunCancellation::default(),
                on_update,
            )
            .await
        })
    }

    fn call_with_updates_and_cancellation<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
        cancellation: RunCancellation,
        on_update: &'a mut (dyn FnMut(Vec<String>) + Send),
    ) -> AppToolFuture<'a> {
        Box::pin(async move {
            let mut args = ShellArgs::parse(args)?;
            // Validate before RTK rewrite so invalid timeouts never launch `rtk`.
            let timeout = args.timeout()?;
            if self.rtk_enabled {
                if let Some(command) = super::rtk::rewrite(&args.command).await {
                    args.command = command;
                }
            }
            let execution = ProcessExecution::new(
                &ctx.cwd,
                ProcessInvocation::shell_from_path(
                    "powershell.exe",
                    vec![
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-Command".into(),
                    ],
                    wrapped_command(&args.command),
                ),
                ProcessEnvironment::InheritAll,
                ProcessOutputLimits::new(ctx.max_output_bytes, timeout),
            );
            let result = execute_process(execution, id, cancellation, on_update).await?;
            if self.rtk_enabled {
                super::rtk::log_execution(&ctx.cwd, &args.command, &result).await;
            }
            Ok(result)
        })
    }
}

pub(super) async fn execute_process(
    execution: ProcessExecution,
    id: String,
    cancellation: RunCancellation,
    on_update: &mut (dyn FnMut(Vec<String>) + Send),
) -> Result<ToolResult, ToolError> {
    shell_process::run::<ProcessTreeGuard>(execution, id, "PowerShell", cancellation, on_update)
        .await
}

struct ProcessTreeGuard(crate::process_supervision::WindowsJob);

impl ProcessSupervisor for ProcessTreeGuard {
    fn prepare(command: &mut Command) {
        crate::process_supervision::WindowsJob::prepare(command);
    }

    fn attach(child: &tokio::process::Child) -> Result<Self, ToolError> {
        crate::process_supervision::WindowsJob::attach(child)
            .map(Self)
            .map_err(|error| ToolError::Message(error.to_string()))
    }

    fn kill(&mut self) {
        self.0.kill();
    }
}

/// Wrap a PowerShell command with UTF-8 output and reliable exit-code handling.
pub fn wrapped_command(command: &str) -> String {
    format!(
        "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); \
         $OutputEncoding = [Console]::OutputEncoding; \
         & {{ {command} }}; \
         if ($null -ne $LASTEXITCODE) {{ exit $LASTEXITCODE }}; \
         if (-not $?) {{ exit 1 }}; \
         exit 0"
    )
}
