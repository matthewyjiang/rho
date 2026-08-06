use crate::cancellation::RunCancellation;
use crate::shell_process::{self, ProcessSupervisor, ShellArgs};
use crate::tool::*;
use rho_sdk::{ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits};
use serde_json::json;
use tokio::process::Command;

pub struct Bash {
    rtk_enabled: bool,
}

impl Bash {
    pub const fn new(rtk_enabled: bool) -> Self {
        Self { rtk_enabled }
    }
}

impl Tool for Bash {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".into(),
            description: "Runs a bash command in the current working directory.".into(),
            input_schema: json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer"}},"required":["command"]}),
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
            if self.rtk_enabled {
                if let Some(command) = super::rtk::rewrite(&args.command).await {
                    args.command = command;
                }
            }
            let execution = ProcessExecution::new(
                &ctx.cwd,
                ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], &args.command),
                ProcessEnvironment::InheritAll,
                ProcessOutputLimits::new(ctx.max_output_bytes, args.timeout()),
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
    shell_process::run::<ProcessGroupGuard>(execution, id, "bash", cancellation, on_update).await
}

struct ProcessGroupGuard {
    pid: Option<u32>,
}

impl ProcessSupervisor for ProcessGroupGuard {
    fn prepare(command: &mut Command) {
        command.process_group(0);
    }

    fn attach(child: &tokio::process::Child) -> Result<Self, ToolError> {
        Ok(Self { pid: child.id() })
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

    // Kills `pid` on drop. Waiting stays in the test body so Drop stays non-blocking.
    struct KillOnDrop(i32);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = unsafe { libc::kill(self.0, libc::SIGKILL) };
        }
    }

    async fn wait_for_pid_file(path: &std::path::Path) -> i32 {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
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
        // Flush+fsync the pid file before sleeping so the parent test can observe
        // the escaped child even when the process keeps the pipe open.
        let command = "python3 -c 'import os,time\nif os.fork()==0:\n os.setsid()\n f=open(\"escaped.pid\",\"w\")\n f.write(str(os.getpid()))\n f.flush()\n os.fsync(f.fileno())\n f.close()\n time.sleep(10)'; sleep 10";

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            Bash::new(false).call(
                json!({
                    "command": command,
                    // Leave enough startup time for Python to fork on loaded
                    // macOS runners before exercising the timeout cleanup.
                    "timeout_seconds": 3
                }),
                ToolContext {
                    cwd: dir.path().to_path_buf(),
                    max_output_bytes: 12_000,
                },
                "call_1".into(),
            ),
        )
        .await
        .expect("timeout arm blocked on the escaped process");

        let pid = wait_for_pid_file(&pid_path).await;
        let _kill_escaped = KillOnDrop(pid);

        let err = result.unwrap_err();
        assert!(err.to_string().contains("timed out after 3s"));
    }

    #[tokio::test]
    async fn inherit_except_scrubs_named_credentials_from_child_env() {
        const CREDENTIAL_VAR: &str = "RHO_TEST_PROVIDER_API_KEY";
        const MARKER_VAR: &str = "RHO_TEST_SAFE_ENV_MARKER";

        // Child-only overrides: do not mutate the test process environment.
        let mut command = tokio::process::Command::new("bash");
        command
            .args([
                "-lc",
                &format!(
                    "printf 'credential=%s;marker=%s' \"${{{CREDENTIAL_VAR}-}}\" \"${{{MARKER_VAR}-}}\""
                ),
            ])
            .env(CREDENTIAL_VAR, "secret-should-not-leak")
            .env(MARKER_VAR, "keep-me")
            .kill_on_drop(true);
        crate::apply_process_environment(
            &mut command,
            &ProcessEnvironment::inherit_except([CREDENTIAL_VAR]),
        )
        .expect("inherit_except must apply");

        let output = command.output().await.expect("scrubbed command should run");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "command should succeed: status={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            stdout, "credential=;marker=keep-me",
            "credential must be absent while non-sensitive vars remain"
        );
    }
}

#[cfg(all(test, unix))]
#[path = "bash_output_tests.rs"]
mod output_tests;
