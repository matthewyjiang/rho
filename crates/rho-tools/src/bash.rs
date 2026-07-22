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
            ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], &args.command),
            ProcessEnvironment::inherit_default(),
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
            "bash received an unsupported process plan".into(),
        ));
    };
    if !matches!(
        execution.environment(),
        ProcessEnvironment::InheritAll | ProcessEnvironment::InheritExcept { .. }
    ) {
        return Err(ToolError::Message(
            "bash received an unsupported process environment".into(),
        ));
    }

    let mut command = Command::new(executable);
    command
        .args(arguments)
        .arg(shell_command)
        .current_dir(execution.working_directory())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    super::process_env::apply_process_environment(&mut command, execution.environment());
    let mut child = command.spawn()?;
    let mut process_group = ProcessGroupGuard::new(child.id());

    let start = Instant::now();
    let mut streams = StreamSession::attach(&mut child);
    let timeout = execution.output_limits().timeout();
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
            chunk = streams.recv(), if streams.output_open => {
                streams.apply_chunk(chunk);
            }
            _ = update_tick.tick() => {
                on_update(vec![streams.running_content()]);
            }
            _ = &mut timeout_sleep, if timeout.is_some() => {
                process_group.kill();
                let _ = child.start_kill();
                let _ = child.wait().await;
                let output = streams.finish().await;
                let seconds = timeout.unwrap_or_default().as_secs();
                return Err(ToolError::Message(truncate(
                    timeout_content(&output.stdout, &output.stderr, seconds),
                    execution.output_limits().max_output_bytes(),
                )));
            }
        }
    };

    process_group.kill();
    let output = streams.finish().await;

    let elapsed_secs = start.elapsed().as_secs_f64();
    let exit_code = status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".into());
    let content = truncate(
        finished_content(
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
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

/// Collects child stdout/stderr and owns reader teardown.
///
/// `finish` is the only graceful shutdown path. Drop aborts any still-running
/// readers so cancel/error returns cannot leak tasks behind a live pipe writer.
struct StreamSession {
    chunk_rx: tokio::sync::mpsc::UnboundedReceiver<(StreamKind, Vec<u8>)>,
    readers: Vec<tokio::task::JoinHandle<()>>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    output_open: bool,
}

struct CollectedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl StreamSession {
    fn attach(child: &mut tokio::process::Child) -> Self {
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::unbounded_channel();
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
            output_open: true,
        }
    }

    fn recv(&mut self) -> impl std::future::Future<Output = Option<(StreamKind, Vec<u8>)>> + '_ {
        self.chunk_rx.recv()
    }

    fn apply_chunk(&mut self, chunk: Option<(StreamKind, Vec<u8>)>) {
        match chunk {
            Some((StreamKind::Stdout, bytes)) => self.stdout.extend(bytes),
            Some((StreamKind::Stderr, bytes)) => self.stderr.extend(bytes),
            None => self.output_open = false,
        }
    }

    fn running_content(&self) -> String {
        running_content(&self.stdout, &self.stderr)
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
                // Stop once the consumer is gone so escaped writers cannot keep
                // these tasks allocating and discarding output forever.
                if chunk_tx.send((kind, buffer[..n].to_vec())).is_err() {
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
        // `read -t` bounds a bad inherited stdin so the test fails fast, while the
        // tool timeout stays loose enough for slow `bash -lc` startup under CI load.
        // Null stdin should make `read` return EOF immediately (not the timeout path).
        let result = Bash::new(false)
            .call(
                json!({
                    "command": "if read -r -t 2 value; then printf 'input:%s' \"$value\"; elif [ $? -gt 128 ]; then printf 'timeout'; else printf 'eof'; fi",
                    "timeout_seconds": 30
                }),
                test_context(),
                "call_1".into(),
            )
            .await
            .expect("command should not wait for terminal input");

        assert!(
            result.ok,
            "command should complete on closed stdin: {}",
            result.content
        );
        assert!(
            result.content.contains("eof"),
            "expected eof marker in output: {}",
            result.content
        );
        assert!(
            !result.content.contains("timeout"),
            "read timed out waiting for stdin instead of seeing EOF: {}",
            result.content
        );
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

    // Kills `pid` on drop. Waiting stays in the test body so Drop stays non-blocking.
    struct KillOnDrop(i32);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = unsafe { libc::kill(self.0, libc::SIGKILL) };
        }
    }

    async fn wait_for_pid_file(path: &std::path::Path) -> i32 {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Ok(contents) = std::fs::read_to_string(path) {
                if let Ok(pid) = contents.trim().parse::<i32>() {
                    return pid;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "escaped child did not write pid file at {}",
                path.display()
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn timeout_returns_when_an_escaped_process_holds_the_output_pipe() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("escaped.pid");
        // Use Python os.setsid rather than setsid(1): the binary is Linux-only and
        // missing on macOS CI, while Python is present on both runner images.
        // Keep the script on one Rust string so indentation inside the Python
        // block is not eaten by `\` line continuations.
        let command = "python3 -c 'import os,time\nif os.fork()==0:\n os.setsid()\n open(\"escaped.pid\",\"w\").write(str(os.getpid()))\n time.sleep(10)'; sleep 10";

        let start = std::time::Instant::now();
        let result = Bash::new(false)
            .call(
                json!({
                    "command": command,
                    "timeout_seconds": 1
                }),
                ToolContext {
                    cwd: dir.path().to_path_buf(),
                    max_output_bytes: 12_000,
                },
                "call_1".into(),
            )
            .await;

        let pid = wait_for_pid_file(&pid_path).await;
        let _kill_escaped = KillOnDrop(pid);

        let err = result.unwrap_err();
        assert!(err.to_string().contains("timed out after 1s"));
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "timeout arm blocked on the escaped process: {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn dropping_call_terminates_background_processes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            max_output_bytes: 12000,
        };
        let background_process_started = ctx.cwd.join("background-process-started");
        let marker = ctx.cwd.join("background-process-survived");

        let bash = Bash::new(false);
        let mut call = Box::pin(bash.call(
            json!({
                "command": "sh -c 'touch background-process-started; sleep 2; touch background-process-survived' </dev/null >/dev/null 2>&1 & wait"
            }),
            ctx,
            "call_1".into(),
        ));
        tokio::select! {
            result = &mut call => panic!("command completed unexpectedly: {result:?}"),
            _ = async {
                while !background_process_started.exists() {
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

    #[tokio::test]
    async fn inherit_except_scrubs_named_credentials_from_child_env() {
        const CREDENTIAL_VAR: &str = "RHO_TEST_PROVIDER_API_KEY";
        const MARKER_VAR: &str = "RHO_TEST_SAFE_ENV_MARKER";

        struct EnvGuard {
            keys: &'static [&'static str],
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                for key in self.keys {
                    std::env::remove_var(key);
                }
            }
        }

        let _guard = EnvGuard {
            keys: &[CREDENTIAL_VAR, MARKER_VAR],
        };
        // Scoped env mutation for this test only.
        std::env::set_var(CREDENTIAL_VAR, "secret-should-not-leak");
        std::env::set_var(MARKER_VAR, "keep-me");

        let execution = ProcessExecution::new(
            std::env::temp_dir(),
            ProcessInvocation::shell_from_path(
                "bash",
                vec!["-lc".into()],
                format!(
                    "printf 'credential=%s;marker=%s' \"${{{CREDENTIAL_VAR}-}}\" \"${{{MARKER_VAR}-}}\""
                ),
            ),
            ProcessEnvironment::inherit_except([CREDENTIAL_VAR]),
            ProcessOutputLimits::new(12_000, Some(std::time::Duration::from_secs(30))),
        );
        let result = execute_process(
            execution,
            "call_1".into(),
            RunCancellation::default(),
            &mut |_| {},
        )
        .await
        .expect("scrubbed command should run");

        assert!(result.ok, "command should succeed: {}", result.content);
        assert!(
            result.content.contains("credential=;marker=keep-me"),
            "credential must be absent while non-sensitive vars remain: {}",
            result.content
        );
    }
}

#[cfg(all(test, unix))]
#[path = "bash_output_tests.rs"]
mod output_tests;
