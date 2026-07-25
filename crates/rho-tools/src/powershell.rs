use crate::cancellation::RunCancellation;
use crate::shell_process::{
    finished_result, interrupted, log_rtk_execution, read_stream, running_content, shell_command,
    timeout_error, ShellArgs, StreamKind,
};
use crate::tool::*;
use rho_sdk::{ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits};
use serde_json::json;
use std::time::Instant;

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
        let args = ShellArgs::parse(args, self.rtk_enabled).await?;
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
        log_rtk_execution(self.rtk_enabled, &ctx.cwd, &args.command, &result).await;
        Ok(result)
    }
}

pub(super) async fn execute_process(
    execution: ProcessExecution,
    id: String,
    cancellation: RunCancellation,
    on_update: &mut (dyn FnMut(Vec<String>) + Send),
) -> Result<ToolResult, ToolError> {
    let mut command = shell_command(&execution, "PowerShell")?;
    let start = Instant::now();
    let mut child = command.spawn()?;
    let mut process_tree = ProcessTreeGuard::attach(&child)?;

    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(read_stream(StreamKind::Stdout, stdout, chunk_tx.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(read_stream(StreamKind::Stderr, stderr, chunk_tx));
    }

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut last_update = Instant::now();
    let timeout = execution.output_limits().timeout();
    let status = loop {
        while let Ok((kind, bytes)) = chunk_rx.try_recv() {
            match kind {
                StreamKind::Stdout => stdout.extend(bytes),
                StreamKind::Stderr => stderr.extend(bytes),
            }
        }

        if last_update.elapsed() >= std::time::Duration::from_millis(50) {
            on_update(vec![running_content(&stdout, &stderr)]);
            last_update = Instant::now();
        }

        if let Some(status) = child.try_wait()? {
            break status;
        }

        if timeout.is_some_and(|timeout| start.elapsed() >= timeout) {
            process_tree.kill();
            let _ = child.wait().await;
            drain_stream_chunks(&mut chunk_rx, &mut stdout, &mut stderr).await;
            return Err(timeout_error(
                &stdout,
                &stderr,
                timeout.unwrap_or_default(),
                execution.output_limits().max_output_bytes(),
            ));
        }

        tokio::select! {
            () = cancellation.cancelled() => {
                process_tree.kill();
                let _ = child.wait().await;
                return Err(interrupted());
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(25)) => {}
        }
    };

    process_tree.kill();
    drain_stream_chunks(&mut chunk_rx, &mut stdout, &mut stderr).await;

    Ok(finished_result(
        id,
        status,
        &stdout,
        &stderr,
        start.elapsed(),
        execution.output_limits().max_output_bytes(),
    ))
}

#[cfg(windows)]
struct ProcessTreeGuard {
    job: Option<windows_sys::Win32::Foundation::HANDLE>,
}

#[cfg(windows)]
unsafe impl Send for ProcessTreeGuard {}

#[cfg(windows)]
impl ProcessTreeGuard {
    fn attach(child: &tokio::process::Child) -> std::io::Result<Self> {
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};

        let process = child
            .raw_handle()
            .ok_or_else(|| std::io::Error::other("spawned PowerShell process has no handle"))?;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
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
                return Err(error);
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

#[cfg(windows)]
impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

async fn drain_stream_chunks(
    chunk_rx: &mut tokio::sync::mpsc::UnboundedReceiver<(StreamKind, Vec<u8>)>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) {
    while let Some((kind, bytes)) = chunk_rx.recv().await {
        match kind {
            StreamKind::Stdout => stdout.extend(bytes),
            StreamKind::Stderr => stderr.extend(bytes),
        }
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
