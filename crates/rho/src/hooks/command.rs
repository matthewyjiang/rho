//! Running one hook program.
//!
//! Spawn without a shell, write exactly one bounded JSON event to stdin, read
//! bounded stdout and stderr, enforce the timeout, and terminate the whole
//! process tree on the way out.

use std::{process::Stdio, time::Duration};

use tokio::io::AsyncWriteExt;

use crate::{
    cli_runtime::StderrTail,
    process_tree::{ProcessTree, SupervisedTree},
    session_environment::Ambient,
};

use super::{config::HookDefinition, protocol::MAX_DECISION_BYTES};

/// What running one hook program produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookRunOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    /// Trimmed tail of stderr, ellipsis-prefixed when the head was dropped.
    pub stderr: String,
    pub duration: Duration,
    /// Whether either stream hit its capture bound.
    pub truncated: bool,
}

impl HookRunOutput {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// Sanitized one-line stderr summary for diagnostics.
    pub fn stderr_summary(&self) -> String {
        self.stderr
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .unwrap_or_default()
            .to_owned()
    }
}

/// Why a hook program did not produce usable output.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HookRunError {
    #[error("timed out after {}", humantime::format_duration(*timeout))]
    TimedOut { timeout: Duration },
    #[error("could not start: {0}")]
    Spawn(String),
    #[error("failed while running: {0}")]
    Io(String),
    #[error("was cancelled before it finished")]
    Cancelled,
}

/// Runs `hook` with `event` on stdin, its environment taken from `environment`,
/// its session's (#1659).
///
/// Cancellation and timeout both kill the supervised tree before returning, so
/// no descendant outlives the call.
pub async fn run_hook(
    hook: &HookDefinition,
    event: &str,
    environment: &Ambient,
    cancellation: rho_sdk::CancellationToken,
) -> Result<HookRunOutput, HookRunError> {
    let started = std::time::Instant::now();
    let mut command = build_command(hook, environment, crate::child_env::rho_home().as_deref());
    SupervisedTree::prepare(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| HookRunError::Spawn(error.to_string()))?;
    let mut tree = SupervisedTree::attach(&child).map_err(|error| {
        let _ = child.start_kill();
        HookRunError::Spawn(error.to_string())
    })?;

    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let payload = event.to_owned();
    // Writing stdin concurrently with reading the pipes keeps a handler that
    // starts printing before it finishes reading from deadlocking us.
    let writer = tokio::spawn(async move {
        if let Some(stdin) = stdin.as_mut() {
            let _ = stdin.write_all(payload.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        drop(stdin);
    });
    // stdout keeps its head: the decision protocol rejects oversize output, so
    // the first bytes are what matter. stderr keeps its tail: the closing lines
    // carry the failure.
    let stdout_reader = tokio::spawn(read_bounded(stdout, MAX_DECISION_BYTES + 1));
    let stderr_reader = tokio::spawn(StderrTail::capture(stderr));

    let status = tokio::select! {
        status = child.wait() => status,
        () = cancellation.cancelled() => {
            tree.kill();
            let _ = child.start_kill();
            let _ = child.wait().await;
            writer.abort();
            stdout_reader.abort();
            stderr_reader.abort();
            return Err(HookRunError::Cancelled);
        }
        () = tokio::time::sleep(hook.timeout()) => {
            tree.kill();
            let _ = child.start_kill();
            let _ = child.wait().await;
            writer.abort();
            stdout_reader.abort();
            stderr_reader.abort();
            return Err(HookRunError::TimedOut { timeout: hook.timeout() });
        }
    };

    tree.kill();
    let _ = writer.await;
    let (stdout, stdout_truncated) = stdout_reader.await.unwrap_or_default();
    let stderr = stderr_reader.await.unwrap_or_default();
    let status = status.map_err(|error| HookRunError::Io(error.to_string()))?;
    Ok(HookRunOutput {
        exit_code: status.code(),
        stdout,
        truncated: stdout_truncated || stderr.elided(),
        stderr: stderr.finish(),
        duration: started.elapsed(),
    })
}

/// The command that runs `hook`, with exactly the environment the spawn
/// contract promises, read from `environment`, its session's (#1659), and
/// `rho_home` as the home of the daemon serving it.
///
/// Nothing else of `environment` reaches a hook: the child gets the documented
/// base set of [`crate::child_env`], which says where the session's daemon is
/// so a hook's `rho` reaches it, plus the names the hook allowlisted, so
/// provider credentials and unrelated secrets cannot reach a hook program by
/// accident. A name that is unset is simply absent from the child. A hook that
/// needs one must handle its absence; the runtime does not invent a value or
/// fail the spawn, because a missing optional token is a hook concern.
///
/// [`super::IN_HOOK_ENV`] is always set last, so an allowlist cannot override
/// it; it is Rho's own marker rather than an inherited value.
fn build_command(
    hook: &HookDefinition,
    environment: &Ambient,
    rho_home: Option<&std::path::Path>,
) -> tokio::process::Command {
    let argv = hook.command();
    let mut command = tokio::process::Command::new(hook.executable());
    command
        .args(&argv[1..])
        .current_dir(hook.working_directory())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    environment.apply_base(&mut command, rho_home);
    for name in hook.env() {
        if let Some(value) = environment.read_os(name) {
            command.env(name, value);
        }
    }
    // Stops a hook that runs `rho` from re-entering the hooks that invoked it.
    command.env(super::IN_HOOK_ENV, "1");
    command
}

/// Reads at most `limit` bytes of the head, then keeps draining so the child
/// never blocks on a full pipe. Returns the capture and whether the bound was
/// reached.
async fn read_bounded<R>(reader: Option<R>, limit: usize) -> (Vec<u8>, bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let Some(mut reader) = reader else {
        return (Vec::new(), false);
    };
    let mut captured = Vec::new();
    let mut truncated = false;
    let mut buffer = [0u8; 4096];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let remaining = limit.saturating_sub(captured.len());
                if remaining == 0 {
                    truncated = true;
                    continue;
                }
                let take = read.min(remaining);
                captured.extend_from_slice(&buffer[..take]);
                truncated |= take < read;
            }
        }
    }
    (captured, truncated)
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
