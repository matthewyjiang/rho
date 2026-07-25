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

#[async_trait::async_trait]
impl Tool for PowerShell {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "powershell".into(),
            description: "Runs a PowerShell command in the current working directory.".into(),
            input_schema: json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer"}},"required":["command"]}),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        self.call_with_updates(args, ctx, id, &mut |_| {}).await
    }

    async fn call_with_updates(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
        on_update: &mut (dyn FnMut(Vec<String>) + Send),
    ) -> Result<ToolResult, ToolError> {
        self.call_with_updates_and_cancellation(
            args,
            ctx,
            id,
            RunCancellation::default(),
            on_update,
        )
        .await
    }

    async fn call_with_updates_and_cancellation(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
        cancellation: RunCancellation,
        on_update: &mut (dyn FnMut(Vec<String>) + Send),
    ) -> Result<ToolResult, ToolError> {
        let mut args = ShellArgs::parse(args)?;
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
            ProcessOutputLimits::new(ctx.max_output_bytes, args.timeout()),
        );
        let result = execute_process(execution, id, cancellation, on_update).await?;
        if self.rtk_enabled {
            super::rtk::log_execution(&ctx.cwd, &args.command, &result).await;
        }
        Ok(result)
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

struct ProcessTreeGuard {
    job: Option<windows_sys::Win32::Foundation::HANDLE>,
}

unsafe impl Send for ProcessTreeGuard {}

impl ProcessSupervisor for ProcessTreeGuard {
    fn prepare(_command: &mut Command) {}

    fn attach(child: &tokio::process::Child) -> Result<Self, ToolError> {
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};

        let process = child
            .raw_handle()
            .ok_or_else(|| ToolError::Message("spawned PowerShell process has no handle".into()))?;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(ToolError::Message(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            );
            if configured == 0 || AssignProcessToJobObject(job, process as _) == 0 {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(ToolError::Message(error.to_string()));
            }
            Ok(Self { job: Some(job) })
        }
    }

    fn kill(&mut self) {
        if let Some(job) = self.job.take() {
            unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(job, 1);
                windows_sys::Win32::Foundation::CloseHandle(job);
            }
        }
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        self.kill();
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

#[cfg(all(test, windows))]
#[path = "powershell_tests.rs"]
mod tests;
