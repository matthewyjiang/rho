//! Running one hook program.
//!
//! Spawn without a shell, write exactly one bounded JSON event to stdin, read
//! bounded stdout and stderr, enforce the timeout, and terminate the whole
//! process tree on the way out.

use std::{process::Stdio, time::Duration};

use tokio::io::AsyncWriteExt;

use super::{
    config::HookDefinition,
    environment::child_environment,
    protocol::MAX_DECISION_BYTES,
    supervisor::{ProcessTree, SupervisedTree},
};

/// Largest stderr capture retained for diagnostics.
pub const MAX_STDERR_BYTES: usize = 4 * 1024;

/// What running one hook program produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookRunOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
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
        String::from_utf8_lossy(&self.stderr)
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

/// Runs `hook` with `event` on stdin.
///
/// Cancellation and timeout both kill the supervised tree before returning, so
/// no descendant outlives the call.
pub async fn run_hook(
    hook: &HookDefinition,
    event: &str,
    cancellation: rho_sdk::CancellationToken,
) -> Result<HookRunOutput, HookRunError> {
    let started = std::time::Instant::now();
    let mut command = build_command(hook);
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
    let stdout_reader = tokio::spawn(read_bounded(stdout, MAX_DECISION_BYTES + 1));
    let stderr_reader = tokio::spawn(read_bounded(stderr, MAX_STDERR_BYTES));

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
    let (stderr, stderr_truncated) = stderr_reader.await.unwrap_or_default();
    let status = status.map_err(|error| HookRunError::Io(error.to_string()))?;
    Ok(HookRunOutput {
        exit_code: status.code(),
        stdout,
        stderr,
        duration: started.elapsed(),
        truncated: stdout_truncated || stderr_truncated,
    })
}

fn build_command(hook: &HookDefinition) -> tokio::process::Command {
    let argv = hook.command();
    let mut command = tokio::process::Command::new(hook.executable());
    command
        .args(&argv[1..])
        .current_dir(hook.working_directory())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // Nothing ambient reaches a hook: the child gets the documented base set
        // plus its own allowlist, so provider credentials cannot leak through.
        .env_clear()
        .envs(child_environment(hook.env(), super::environment::ambient));
    command
}

/// Reads at most `limit` bytes, then keeps draining so the child never blocks on
/// a full pipe. Returns the capture and whether the bound was reached.
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
