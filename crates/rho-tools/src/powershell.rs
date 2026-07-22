use crate::cancellation::RunCancellation;
use crate::tool::*;
use rho_sdk::{
    ExecutableSelection, ProcessEnvironment, ProcessExecution, ProcessInvocation,
    ProcessOutputLimits,
};
use serde::Deserialize;
use serde_json::json;
use std::{process::Stdio, time::Instant};
use tokio::{io::AsyncReadExt, process::Command};

pub struct PowerShell {
    rtk_enabled: bool,
}

impl PowerShell {
    pub const fn new(rtk_enabled: bool) -> Self {
        Self { rtk_enabled }
    }
}

#[derive(Deserialize)]
struct Args {
    command: String,
    timeout_seconds: Option<u64>,
}

#[derive(Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
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
        let mut args: Args = serde_json::from_value(args)?;
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
            ProcessOutputLimits::new(
                ctx.max_output_bytes,
                args.timeout_seconds.map(std::time::Duration::from_secs),
            ),
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
    let ProcessInvocation::Shell {
        executable,
        selection: ExecutableSelection::SearchPath,
        arguments,
        command: shell_command,
    } = execution.invocation()
    else {
        return Err(ToolError::Message(
            "PowerShell received an unsupported process plan".into(),
        ));
    };
    let start = Instant::now();
    let mut command = Command::new(executable);
    command
        .kill_on_drop(true)
        .args(arguments)
        .arg(shell_command)
        .current_dir(execution.working_directory())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    super::process_env::apply_process_environment(&mut command, execution.environment())
        .map_err(ToolError::Message)?;
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
            let seconds = timeout.unwrap_or_default().as_secs();
            return Err(ToolError::Message(truncate(
                timeout_content(&stdout, &stderr, seconds),
                execution.output_limits().max_output_bytes(),
            )));
        }

        tokio::select! {
            () = cancellation.cancelled() => {
                process_tree.kill();
                let _ = child.wait().await;
                return Err(ToolError::Message("tool interrupted".into()));
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(25)) => {}
        }
    };

    process_tree.kill();
    drain_stream_chunks(&mut chunk_rx, &mut stdout, &mut stderr).await;

    let elapsed_secs = start.elapsed().as_secs_f64();
    let exit_code = status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".into());
    let content = truncate(
        finished_content(
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
            elapsed_secs,
            &exit_code,
        ),
        execution.output_limits().max_output_bytes(),
    );
    Ok(ToolResult {
        id,
        ok: status.success(),
        content,
    })
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

async fn read_stream<R>(
    kind: StreamKind,
    mut reader: R,
    chunk_tx: tokio::sync::mpsc::UnboundedSender<(StreamKind, Vec<u8>)>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let _ = chunk_tx.send((kind, buffer[..n].to_vec()));
            }
        }
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

fn running_content(stdout: &[u8], stderr: &[u8]) -> String {
    format!(
        "stdout:\n{}\n\nstderr:\n{}\n\ntime: running",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
}

fn finished_content(stdout: String, stderr: String, elapsed_secs: f64, exit_code: &str) -> String {
    format!(
        "stdout:\n{stdout}\n\nstderr:\n{stderr}\n\ntime: {elapsed_secs:.1}s  exit code: {exit_code}"
    )
}

fn timeout_content(stdout: &[u8], stderr: &[u8], secs: u64) -> String {
    format!(
        "command timed out after {secs}s\n\nstdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
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
