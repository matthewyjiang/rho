use crate::cancellation::RunCancellation;
use crate::tool::*;
use serde::Deserialize;
use serde_json::json;
use std::{process::Stdio, time::Instant};
use tokio::{io::AsyncReadExt, process::Command};

const FINAL_OUTPUT_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

pub struct Bash {
    rtk_enabled: bool,
}

impl Bash {
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
impl Tool for Bash {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".into(),
            description: "Runs a bash command in the current working directory.".into(),
            input_schema: json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer"}},"required":["command"]}),
        }
    }

    fn display_style(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::file_or_command()
    }

    fn display_command(&self, args: &serde_json::Value) -> Option<String> {
        args.get("command")
            .and_then(|command| command.as_str())
            .map(str::to_string)
    }

    fn display_preview_lines(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Vec<String> {
        vec![command_line(args)]
    }

    fn display_start_lines(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Vec<String> {
        command_lines(args)
    }

    fn display_lines(
        &self,
        args: &serde_json::Value,
        _ctx: &ToolContext,
        result: &ToolResult,
    ) -> Vec<String> {
        display_lines_with_content(args, &result.content)
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
        let mut raw_args = args.clone();
        let mut args: Args = serde_json::from_value(args)?;
        if self.rtk_enabled {
            if let Some(command) = super::rtk::rewrite(&args.command).await {
                args.command = command;
                raw_args["command"] = serde_json::Value::String(args.command.clone());
            }
        }
        let mut command = Command::new("bash");
        command
            .arg("-lc")
            .arg(&args.command)
            .current_dir(&ctx.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn()?;
        let mut process_group = ProcessGroupGuard::new(child.id());

        let start = Instant::now();
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(read_stream(StreamKind::Stdout, stdout, chunk_tx.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(read_stream(StreamKind::Stderr, stderr, chunk_tx));
        }

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut output_open = true;
        let timeout = args.timeout_seconds.map(std::time::Duration::from_secs);
        let mut timeout_sleep = Box::pin(tokio::time::sleep(
            timeout.unwrap_or(std::time::Duration::MAX),
        ));
        let mut update_tick = tokio::time::interval(std::time::Duration::from_millis(50));
        update_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        update_tick.tick().await;
        let status = loop {
            tokio::select! {
                () = cancellation.cancelled() => {
                    return Err(ToolError::Message("tool interrupted".into()));
                }
                status = child.wait() => break status?,
                chunk = chunk_rx.recv(), if output_open => {
                    match chunk {
                        Some((kind, bytes)) => {
                            append_stream_chunk(kind, bytes, &mut stdout, &mut stderr);
                        }
                        None => output_open = false,
                    }
                }
                _ = update_tick.tick() => {
                    on_update(display_lines_with_content(
                        &raw_args,
                        &running_content(&stdout, &stderr),
                    ));
                }
                _ = &mut timeout_sleep, if timeout.is_some() => {
                    process_group.kill();
                    let _ = child.start_kill();
                    drain_ready_stream_chunks(&mut chunk_rx, &mut stdout, &mut stderr);
                    let _ = child.wait().await;
                    drain_stream_chunks(&mut chunk_rx, &mut stdout, &mut stderr).await;
                    let secs = args.timeout_seconds.unwrap_or_default();
                    return Err(ToolError::Message(truncate(
                        timeout_content(&stdout, &stderr, secs),
                        ctx.max_output_bytes,
                    )));
                }
            }
        };

        process_group.kill();
        let _ = tokio::time::timeout(
            FINAL_OUTPUT_GRACE,
            drain_stream_chunks(&mut chunk_rx, &mut stdout, &mut stderr),
        )
        .await;
        drain_ready_stream_chunks(&mut chunk_rx, &mut stdout, &mut stderr);

        let elapsed_secs = start.elapsed().as_secs_f64();
        let exit_code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        let mut content = finished_content(
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
            elapsed_secs,
            &exit_code,
        );
        content = truncate(content, ctx.max_output_bytes);
        let result = ToolResult {
            id,
            ok: status.success(),
            content,
        };
        if self.rtk_enabled {
            super::rtk::log_execution(&ctx.cwd, &args.command, &result).await;
        }
        on_update(display_lines_with_content(&raw_args, &result.content));
        Ok(result)
    }
}

struct ProcessGroupGuard {
    pid: Option<u32>,
}

impl ProcessGroupGuard {
    const fn new(pid: Option<u32>) -> Self {
        Self { pid }
    }

    fn kill(&mut self) {
        kill_process_group(self.pid.take());
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(unix)]
fn kill_process_group(pid: Option<u32>) {
    let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) else {
        return;
    };
    // A negative PID targets the process group created with `process_group(0)`.
    let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
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

fn append_stream_chunk(
    kind: StreamKind,
    bytes: Vec<u8>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) {
    match kind {
        StreamKind::Stdout => stdout.extend(bytes),
        StreamKind::Stderr => stderr.extend(bytes),
    }
}

fn drain_ready_stream_chunks(
    chunk_rx: &mut tokio::sync::mpsc::UnboundedReceiver<(StreamKind, Vec<u8>)>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) {
    while let Ok((kind, bytes)) = chunk_rx.try_recv() {
        append_stream_chunk(kind, bytes, stdout, stderr);
    }
}

async fn drain_stream_chunks(
    chunk_rx: &mut tokio::sync::mpsc::UnboundedReceiver<(StreamKind, Vec<u8>)>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) {
    while let Some((kind, bytes)) = chunk_rx.recv().await {
        append_stream_chunk(kind, bytes, stdout, stderr);
    }
}

fn command_line(args: &serde_json::Value) -> String {
    match args.get("command").and_then(|command| command.as_str()) {
        Some(command) if !command.trim().is_empty() => format!("bash {command}"),
        _ => "bash".into(),
    }
}

fn command_lines(args: &serde_json::Value) -> Vec<String> {
    vec![command_line(args), format_timeout(args)]
}

fn display_lines_with_content(args: &serde_json::Value, content: &str) -> Vec<String> {
    let mut lines = command_lines(args);
    if !content.trim().is_empty() {
        lines.push(String::new());
        lines.push(content.to_string());
    }
    lines
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

fn format_timeout(args: &serde_json::Value) -> String {
    match args.get("timeout_seconds").and_then(|value| value.as_u64()) {
        Some(seconds) => format!("timeout: {seconds}s"),
        None => "timeout: none".into(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use serde_json::json;

    use super::*;

    fn test_context() -> ToolContext {
        ToolContext {
            cwd: std::env::temp_dir(),
            max_output_bytes: 12000,
        }
    }

    #[tokio::test]
    async fn command_receives_eof_on_stdin() {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Bash::new(false).call(
                json!({"command": "if read -r value; then printf 'input:%s' \"$value\"; else printf 'eof'; fi"}),
                test_context(),
                "call_1".into(),
            ),
        )
        .await
        .expect("command should not wait for terminal input")
        .unwrap();

        assert!(result.ok);
        assert!(result.content.contains("eof"));
    }

    #[tokio::test]
    async fn returns_lossy_output_for_non_utf8_bytes() {
        let result = Bash::new(false)
            .call(
                json!({"command": "printf 'ok\\xff'"}),
                test_context(),
                "call_1".into(),
            )
            .await
            .unwrap();

        assert!(result.ok);
        assert!(result.content.contains("ok\u{FFFD}"));
    }

    #[tokio::test]
    async fn timeout_error_includes_partial_output() {
        let err = Bash::new(false)
            .call(
                json!({"command": "printf 'started\\n'; sleep 10", "timeout_seconds": 5}),
                test_context(),
                "call_1".into(),
            )
            .await
            .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("timed out after 5s"));
        assert!(message.contains("started"));
    }

    #[tokio::test]
    async fn dropping_call_terminates_background_processes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            max_output_bytes: 12000,
        };
        let started = ctx.cwd.join("started");
        let marker = ctx.cwd.join("background-process-survived");

        let bash = Bash::new(false);
        let mut call = Box::pin(bash.call(
            json!({
                "command": "touch started; sh -c 'sleep 2; touch background-process-survived' </dev/null >/dev/null 2>&1 & wait"
            }),
            ctx,
            "call_1".into(),
        ));
        tokio::select! {
            result = &mut call => panic!("command completed unexpectedly: {result:?}"),
            _ = async {
                while !started.exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            } => {}
        }
        drop(call);

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert!(!marker.exists(), "background process survived cancellation");
    }

    #[tokio::test]
    async fn timeout_terminates_background_processes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            max_output_bytes: 12000,
        };
        let marker = ctx.cwd.join("background-process-survived");

        Bash::new(false)
            .call(
                json!({
                    "command": "sh -c 'sleep 2; touch background-process-survived' </dev/null >/dev/null 2>&1 & wait",
                    "timeout_seconds": 1
                }),
                ctx,
                "call_1".into(),
            )
            .await
            .unwrap_err();

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        assert!(!marker.exists(), "background process survived the timeout");
    }
}

#[cfg(all(test, unix))]
#[path = "bash_output_tests.rs"]
mod output_tests;
