//! Mechanics shared by the platform shell tools (`bash`, `powershell`).
//!
//! Each platform tool keeps its own process supervision policy (Unix process
//! groups versus Windows job objects) and provides the shell invocation, while
//! argument parsing, child stream reading, and result formatting live here so
//! both tools report identical output.

use crate::process_env::apply_process_environment;
use crate::tool::{truncate, ToolError, ToolResult};
use rho_sdk::{ExecutableSelection, ProcessExecution, ProcessInvocation};
use serde::Deserialize;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

/// Arguments accepted by every shell tool.
#[derive(Deserialize)]
pub(crate) struct ShellArgs {
    pub command: String,
    pub timeout_seconds: Option<u64>,
}

impl ShellArgs {
    /// Parses tool arguments, applying the RTK command rewrite when enabled.
    pub(crate) async fn parse(
        args: serde_json::Value,
        rtk_enabled: bool,
    ) -> Result<Self, ToolError> {
        let mut parsed: Self = serde_json::from_value(args)?;
        if rtk_enabled {
            if let Some(command) = crate::rtk::rewrite(&parsed.command).await {
                parsed.command = command;
            }
        }
        Ok(parsed)
    }

    pub(crate) fn timeout(&self) -> Option<Duration> {
        self.timeout_seconds.map(Duration::from_secs)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum StreamKind {
    Stdout,
    Stderr,
}

pub(crate) type ChunkSender = tokio::sync::mpsc::UnboundedSender<(StreamKind, Vec<u8>)>;

/// Builds the child process for a shell execution, leaving supervision setup
/// (process groups, job objects) to the caller.
///
/// Returns an error when the execution does not describe a shell command found
/// on `PATH`, so tools fail closed on unsupported process plans.
pub(crate) fn shell_command(
    execution: &ProcessExecution,
    tool_name: &str,
) -> Result<Command, ToolError> {
    let ProcessInvocation::Shell {
        executable,
        selection: ExecutableSelection::SearchPath,
        arguments,
        command: shell_command,
    } = execution.invocation()
    else {
        return Err(ToolError::Message(format!(
            "{tool_name} received an unsupported process plan"
        )));
    };
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .arg(shell_command)
        .current_dir(execution.working_directory())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_process_environment(&mut command, execution.environment()).map_err(ToolError::Message)?;
    Ok(command)
}

pub(crate) async fn read_stream<R>(kind: StreamKind, mut reader: R, chunk_tx: ChunkSender)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                // Stop once the consumer is gone so escaped writers cannot keep
                // these tasks allocating and discarding output forever.
                if chunk_tx.send((kind, buffer[..n].to_vec())).is_err() {
                    break;
                }
            }
        }
    }
}

pub(crate) fn running_content(stdout: &[u8], stderr: &[u8]) -> String {
    format!(
        "stdout:\n{}\n\nstderr:\n{}\n\ntime: running",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
}

/// Formats the terminal output of a completed command.
pub(crate) fn finished_result(
    id: String,
    status: std::process::ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
    elapsed: Duration,
    max_output_bytes: usize,
) -> ToolResult {
    let exit_code = status
        .code()
        .map_or_else(|| "signal".into(), |code| code.to_string());
    let elapsed_secs = elapsed.as_secs_f64();
    let content = truncate(
        format!(
            "stdout:\n{}\n\nstderr:\n{}\n\ntime: {elapsed_secs:.1}s  exit code: {exit_code}",
            String::from_utf8_lossy(stdout),
            String::from_utf8_lossy(stderr)
        ),
        max_output_bytes,
    );
    ToolResult {
        id,
        ok: status.success(),
        content,
    }
}

/// Formats the error returned when a command exceeds its timeout.
pub(crate) fn timeout_error(
    stdout: &[u8],
    stderr: &[u8],
    timeout: Duration,
    max_output_bytes: usize,
) -> ToolError {
    let secs = timeout.as_secs();
    ToolError::Message(truncate(
        format!(
            "command timed out after {secs}s\n\nstdout:\n{}\n\nstderr:\n{}",
            String::from_utf8_lossy(stdout),
            String::from_utf8_lossy(stderr)
        ),
        max_output_bytes,
    ))
}

pub(crate) fn interrupted() -> ToolError {
    ToolError::Message("tool interrupted".into())
}

/// Records a completed shell run for RTK when the feature is enabled.
pub(crate) async fn log_rtk_execution(
    rtk_enabled: bool,
    cwd: &Path,
    command: &str,
    result: &ToolResult,
) {
    if rtk_enabled {
        crate::rtk::log_execution(cwd, command, result).await;
    }
}
