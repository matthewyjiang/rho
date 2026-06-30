use crate::tool::*;
use serde::Deserialize;
use serde_json::json;
use std::{process::Stdio, time::Instant};
use tokio::{io::AsyncReadExt, process::Command};

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
        let mut raw_args = args.clone();
        let mut args: Args = serde_json::from_value(args)?;
        if self.rtk_enabled {
            if let Some(command) = super::rtk::rewrite(&args.command).await {
                args.command = command;
                raw_args["command"] = serde_json::Value::String(args.command.clone());
            }
        }
        let start = Instant::now();
        let mut child = Command::new("bash")
            .arg("-lc")
            .arg(&args.command)
            .current_dir(&ctx.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

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
        let timeout = args.timeout_seconds.map(std::time::Duration::from_secs);
        let status = loop {
            while let Ok((kind, bytes)) = chunk_rx.try_recv() {
                match kind {
                    StreamKind::Stdout => stdout.extend(bytes),
                    StreamKind::Stderr => stderr.extend(bytes),
                }
            }

            if last_update.elapsed() >= std::time::Duration::from_millis(50) {
                on_update(display_lines_with_content(
                    &raw_args,
                    &running_content(&stdout, &stderr),
                ));
                last_update = Instant::now();
            }

            if let Some(status) = child.try_wait()? {
                break status;
            }

            if timeout.is_some_and(|timeout| start.elapsed() >= timeout) {
                let _ = child.kill().await;
                let secs = args.timeout_seconds.unwrap_or_default();
                return Err(ToolError::Message(format!(
                    "command timed out after {secs}s"
                )));
            }

            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        };

        while let Ok((kind, bytes)) = chunk_rx.try_recv() {
            match kind {
                StreamKind::Stdout => stdout.extend(bytes),
                StreamKind::Stderr => stderr.extend(bytes),
            }
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        let exit_code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        let mut content = finished_content(
            String::from_utf8(stdout)?,
            String::from_utf8(stderr)?,
            elapsed_secs,
            &exit_code,
        );
        content = truncate(content, ctx.max_output_bytes);
        on_update(display_lines_with_content(&raw_args, &content));
        Ok(ToolResult {
            id,
            ok: status.success(),
            content,
        })
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

fn command_lines(args: &serde_json::Value) -> Vec<String> {
    let mut lines = vec![
        match args.get("command").and_then(|command| command.as_str()) {
            Some(command) if !command.trim().is_empty() => format!("bash {command}"),
            _ => "bash".into(),
        },
    ];
    lines.push(format_timeout(args));
    lines
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

fn format_timeout(args: &serde_json::Value) -> String {
    match args.get("timeout_seconds").and_then(|value| value.as_u64()) {
        Some(seconds) => format!("timeout: {seconds}s"),
        None => "timeout: none".into(),
    }
}
