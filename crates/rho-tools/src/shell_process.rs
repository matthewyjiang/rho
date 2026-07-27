//! Shared shell-tool process runner.
//!
//! Platform tools supply only process-supervision policy (Unix process groups
//! versus Windows job objects). Argument shapes, stream collection, the run
//! loop, and result formatting live here so bash and PowerShell cannot drift.

use crate::cancellation::RunCancellation;
use crate::process_env::apply_process_environment;
use crate::process_stream::{capture_failure_notice, StreamKind};
use crate::tool::{truncate, ToolError, ToolResult};
use rho_sdk::{ExecutableSelection, ProcessExecution, ProcessInvocation};
use serde::Deserialize;
use std::{process::Stdio, time::Duration, time::Instant};
use tokio::{io::AsyncReadExt, process::Command};

const FINAL_OUTPUT_GRACE: Duration = Duration::from_millis(250);
const UPDATE_INTERVAL: Duration = Duration::from_millis(50);

/// Arguments accepted by the application shell tools.
#[derive(Deserialize)]
pub(crate) struct ShellArgs {
    pub command: String,
    pub timeout_seconds: Option<u64>,
}

impl ShellArgs {
    pub(crate) fn parse(args: serde_json::Value) -> Result<Self, ToolError> {
        Ok(serde_json::from_value(args)?)
    }

    pub(crate) fn timeout(&self) -> Option<Duration> {
        self.timeout_seconds.map(Duration::from_secs)
    }
}

/// Platform-specific child supervision (process groups, job objects).
///
/// `run` calls [`Self::kill`] on completion, cancellation, and timeout. Drop
/// implementations typically call `kill` again, so implementors must make
/// `kill` idempotent and safe under repeated invocation. `kill` must terminate
/// the full supervised tree (process group or job object), not only the direct
/// child, so background descendants cannot outlive the tool call.
pub(crate) trait ProcessSupervisor: Sized {
    fn prepare(command: &mut Command);

    fn attach(child: &tokio::process::Child) -> Result<Self, ToolError>;

    fn kill(&mut self);
}

/// Spawns `execution`, supervises it with `S`, and streams output updates.
pub(crate) async fn run<S: ProcessSupervisor>(
    execution: ProcessExecution,
    id: String,
    tool_name: &str,
    cancellation: RunCancellation,
    on_update: &mut (dyn FnMut(Vec<String>) + Send),
) -> Result<ToolResult, ToolError> {
    let mut command = build_command(&execution, tool_name)?;
    S::prepare(&mut command);
    let mut child = command.spawn()?;
    let mut supervisor = S::attach(&child)?;

    let start = Instant::now();
    let max_output_bytes = execution.output_limits().max_output_bytes();
    let mut streams = StreamSession::attach(&mut child, max_output_bytes);
    let timeout = execution.output_limits().timeout();
    let mut timeout_sleep = Box::pin(tokio::time::sleep(timeout.unwrap_or(Duration::MAX)));
    let mut update_tick = tokio::time::interval(UPDATE_INTERVAL);
    update_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    update_tick.tick().await;

    let status = loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                supervisor.kill();
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ToolError::Cancelled);
            }
            status = child.wait() => break status?,
            chunk = streams.recv(), if streams.output_open => {
                streams.apply_chunk(chunk);
            }
            _ = update_tick.tick() => {
                on_update(vec![running_content(&streams.stdout, &streams.stderr)]);
            }
            _ = &mut timeout_sleep, if timeout.is_some() => {
                supervisor.kill();
                let _ = child.start_kill();
                let _ = child.wait().await;
                let output = streams.finish().await;
                return Err(timeout_error(
                    &output.stdout,
                    &output.stderr,
                    timeout.unwrap_or_default(),
                    max_output_bytes,
                ));
            }
        }
    };

    supervisor.kill();
    let output = streams.finish().await;
    Ok(finished_result(
        id,
        status,
        &output.stdout,
        &output.stderr,
        start.elapsed(),
        max_output_bytes,
    ))
}

fn build_command(execution: &ProcessExecution, tool_name: &str) -> Result<Command, ToolError> {
    let ProcessInvocation::Shell {
        executable,
        selection: ExecutableSelection::SearchPath,
        arguments,
        command: shell_script,
    } = execution.invocation()
    else {
        return Err(ToolError::Message(format!(
            "{tool_name} received an unsupported process plan"
        )));
    };
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .arg(shell_script)
        .current_dir(execution.working_directory())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_process_environment(&mut command, execution.environment()).map_err(ToolError::Message)?;
    Ok(command)
}

/// In-flight chunk queue bound. Keeps producer backpressure without allowing
/// unbounded allocation between the OS pipes and the retained output budget.
const CHUNK_CHANNEL_CAPACITY: usize = 32;

/// Collects child stdout/stderr and owns reader teardown.
///
/// Retained stdout+stderr never exceed `max_output_bytes`. Readers keep draining
/// the pipes after that budget is full so a noisy command cannot exhaust memory
/// before timeout or completion. `finish` is the only graceful shutdown path;
/// Drop aborts any still-running readers so cancel/error returns cannot leak
/// tasks behind a live pipe writer.
struct StreamSession {
    chunk_rx: tokio::sync::mpsc::Receiver<(StreamKind, Vec<u8>)>,
    readers: Vec<tokio::task::JoinHandle<()>>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    retained_bytes: usize,
    max_output_bytes: usize,
    output_open: bool,
}

struct CollectedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl StreamSession {
    fn attach(child: &mut tokio::process::Child, max_output_bytes: usize) -> Self {
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(CHUNK_CHANNEL_CAPACITY);
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(tokio::spawn(read_stream(
                StreamKind::Stdout,
                stdout,
                chunk_tx.clone(),
            )));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(tokio::spawn(read_stream(
                StreamKind::Stderr,
                stderr,
                chunk_tx,
            )));
        }
        Self {
            chunk_rx,
            readers,
            stdout: Vec::new(),
            stderr: Vec::new(),
            retained_bytes: 0,
            max_output_bytes: max_output_bytes.max(1),
            output_open: true,
        }
    }

    fn recv(&mut self) -> impl std::future::Future<Output = Option<(StreamKind, Vec<u8>)>> + '_ {
        self.chunk_rx.recv()
    }

    fn apply_chunk(&mut self, chunk: Option<(StreamKind, Vec<u8>)>) {
        match chunk {
            Some((kind, bytes)) => {
                let remaining = self.max_output_bytes.saturating_sub(self.retained_bytes);
                if remaining == 0 {
                    return;
                }
                let take = bytes.len().min(remaining);
                match kind {
                    StreamKind::Stdout => self.stdout.extend_from_slice(&bytes[..take]),
                    StreamKind::Stderr => self.stderr.extend_from_slice(&bytes[..take]),
                }
                self.retained_bytes += take;
            }
            None => self.output_open = false,
        }
    }

    async fn finish(mut self) -> CollectedOutput {
        let drain = async {
            while let Some(chunk) = self.chunk_rx.recv().await {
                self.apply_chunk(Some(chunk));
            }
        };
        let _ = tokio::time::timeout(FINAL_OUTPUT_GRACE, drain).await;
        while let Ok(chunk) = self.chunk_rx.try_recv() {
            self.apply_chunk(Some(chunk));
        }
        CollectedOutput {
            stdout: std::mem::take(&mut self.stdout),
            stderr: std::mem::take(&mut self.stderr),
        }
    }

    fn abort_readers(&mut self) {
        for handle in self.readers.drain(..) {
            handle.abort();
        }
    }
}

impl Drop for StreamSession {
    fn drop(&mut self) {
        self.abort_readers();
    }
}

async fn read_stream<R>(
    kind: StreamKind,
    mut reader: R,
    chunk_tx: tokio::sync::mpsc::Sender<(StreamKind, Vec<u8>)>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Err(error) => {
                // Report the truncation instead of returning a silently short
                // capture that reads like complete command output.
                let _ = chunk_tx
                    .send((
                        StreamKind::Stderr,
                        capture_failure_notice(kind, &error).into_bytes(),
                    ))
                    .await;
                break;
            }
            Ok(n) => {
                // Stop once the consumer is gone so escaped writers cannot keep
                // these tasks allocating and discarding output forever.
                if chunk_tx.send((kind, buffer[..n].to_vec())).await.is_err() {
                    break;
                }
            }
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

fn finished_result(
    id: String,
    status: std::process::ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
    elapsed: Duration,
    max_output_bytes: usize,
) -> ToolResult {
    let exit_code = status
        .code()
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "signal".into());
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

fn timeout_error(
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

/// Structured view of the shell tool's stable output envelope.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ShellContent {
    pub notice: Option<String>,
    pub stdout: String,
    pub exit_code: Option<i64>,
    /// Non-numeric exit token (for example `signal`) when no exit code is present.
    pub exit_status: Option<String>,
    pub duration_ms: Option<u64>,
    pub running: bool,
}

/// Parse output produced by this module into presentation fields.
pub fn parse_shell_content(content: &str) -> ShellContent {
    let mut parsed = ShellContent::default();
    let (notice, rest) = if let Some(stdout) = content.strip_prefix("stdout:\n") {
        (None, stdout)
    } else if let Some((notice, stdout)) = content.split_once("\n\nstdout:\n") {
        (Some(notice.to_string()), stdout)
    } else if content.trim().is_empty() {
        return parsed;
    } else {
        parsed.notice = Some(content.trim().to_string());
        return parsed;
    };
    parsed.notice = notice;

    let (stdout_and_maybe_more, footer) = rest
        .rsplit_once("\n\ntime:")
        .map_or((rest, None), |(body, footer)| (body, Some(footer.trim())));
    parsed.stdout = stdout_and_maybe_more
        .rsplit_once("\n\nstderr:")
        .map_or(stdout_and_maybe_more, |(stdout, _)| stdout)
        .trim_end()
        .to_string();

    if let Some(footer) = footer {
        if footer.starts_with("running") {
            parsed.running = true;
        } else {
            parsed.duration_ms = footer
                .split_whitespace()
                .next()
                .and_then(|token| token.strip_suffix('s'))
                .and_then(|seconds| seconds.parse::<f64>().ok())
                .map(|seconds| (seconds * 1000.0).round() as u64);
            if let Some(raw) = footer
                .split("exit code:")
                .nth(1)
                .map(str::trim)
                .filter(|code| !code.is_empty())
            {
                if let Ok(code) = raw.parse::<i64>() {
                    parsed.exit_code = Some(code);
                } else {
                    parsed.exit_status = Some(raw.to_string());
                }
            }
        }
    }
    parsed
}

#[cfg(test)]
#[path = "shell_process_tests.rs"]
mod tests;
