//! Bounded child probes for external CLI binaries.
//!
//! Short timeout, capped stdout/stderr, and kill-on-timeout. Callers own
//! program-specific exit interpretation and output parsing.

use std::{io, process::Stdio, time::Duration};

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::{CliExecutable, CliExecutableError};

/// Hard cap for each of stdout and stderr on a probe.
pub(crate) const PROBE_OUTPUT_CAP_BYTES: usize = 64 * 1024;

/// Captured stdout/stderr from a bounded probe, including non-zero exits.
#[derive(Debug)]
pub(crate) struct BoundedOutput {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

impl BoundedOutput {
    pub(crate) fn stderr_lossy_trimmed(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

/// Failures from spawning or bounding a CLI probe. Callers wrap this with
/// program-specific labels and parse errors.
#[derive(Debug, Error)]
pub(crate) enum ProbeError {
    #[error("binary not found on PATH")]
    BinaryMissing,
    #[error("failed to run `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("`{program}` timed out after {timeout:?}")]
    Timeout { program: String, timeout: Duration },
    #[error("`{program}` produced more than {cap} bytes of {stream}")]
    OutputTooLarge {
        program: String,
        stream: &'static str,
        cap: usize,
    },
    #[error("`{program}` cannot be invoked safely: {source}")]
    Invocation {
        program: String,
        #[source]
        source: CliExecutableError,
    },
}

impl ProbeError {
    #[cfg(test)]
    pub(crate) fn is_binary_missing(&self) -> bool {
        matches!(self, Self::BinaryMissing)
    }
}

/// Run `executable` with `args` under the standard probe bounds.
pub(crate) async fn run_bounded_probe(
    executable: &CliExecutable,
    args: &[&str],
    timeout: Duration,
) -> Result<BoundedOutput, ProbeError> {
    let program = executable.display();
    let command = executable
        .try_command(args.iter().copied())
        .map_err(|source| ProbeError::Invocation {
            program: program.clone(),
            source,
        })?;
    run_bounded_command_with_timeout(command, program, timeout).await
}

/// Run an already-built probe command with the standard bounds.
///
/// Production helpers build the command from a resolved [`CliExecutable`].
/// Tests inject a stable system shell (`/bin/sh -c …`) so they never exec a
/// freshly written file (which can race with `ETXTBSY` under parallel load).
pub(crate) async fn run_bounded_command_with_timeout(
    mut command: Command,
    program: String,
    timeout: Duration,
) -> Result<BoundedOutput, ProbeError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|source| map_spawn_error(&program, source))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let collect = async {
        let stdout_task = async {
            match stdout {
                Some(pipe) => read_capped(pipe, PROBE_OUTPUT_CAP_BYTES).await,
                None => Ok(Vec::new()),
            }
        };
        let stderr_task = async {
            match stderr {
                Some(pipe) => read_capped(pipe, PROBE_OUTPUT_CAP_BYTES).await,
                None => Ok(Vec::new()),
            }
        };
        let (stdout, stderr) = tokio::join!(stdout_task, stderr_task);
        let stdout = map_capped_read(stdout, &program, "stdout")?;
        let stderr = map_capped_read(stderr, &program, "stderr")?;
        let status = child.wait().await.map_err(|source| ProbeError::Spawn {
            program: program.clone(),
            source,
        })?;
        Ok::<BoundedOutput, ProbeError>(BoundedOutput {
            status,
            stdout,
            stderr,
        })
    };

    match tokio::time::timeout(timeout, collect).await {
        Ok(result) => result,
        Err(_) => {
            // Kill and reap the direct child. Probes use short budgets and
            // `kill_on_drop`; this path covers the timeout case explicitly so
            // the direct child is not left running. Full process-tree ownership
            // (descendants of a misbehaving binary) belongs to the execution
            // lifecycle, not these short status/version probes.
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(ProbeError::Timeout { program, timeout })
        }
    }
}

/// Bounded-read failure: I/O errors stay distinct from the hard size cap.
enum CappedReadError {
    Io(io::Error),
    TooLarge,
}

async fn read_capped<R>(mut reader: R, cap: usize) -> Result<Vec<u8>, CappedReadError>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 2048];
    loop {
        let read = reader.read(&mut chunk).await.map_err(CappedReadError::Io)?;
        if read == 0 {
            return Ok(buffer);
        }
        if buffer.len().saturating_add(read) > cap {
            return Err(CappedReadError::TooLarge);
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn map_capped_read(
    result: Result<Vec<u8>, CappedReadError>,
    program: &str,
    stream: &'static str,
) -> Result<Vec<u8>, ProbeError> {
    match result {
        Ok(bytes) => Ok(bytes),
        Err(CappedReadError::TooLarge) => Err(ProbeError::OutputTooLarge {
            program: program.into(),
            stream,
            cap: PROBE_OUTPUT_CAP_BYTES,
        }),
        Err(CappedReadError::Io(source)) => Err(ProbeError::Spawn {
            program: program.into(),
            source,
        }),
    }
}

fn map_spawn_error(program: &str, source: io::Error) -> ProbeError {
    if source.kind() == io::ErrorKind::NotFound {
        ProbeError::BinaryMissing
    } else {
        ProbeError::Spawn {
            program: program.into(),
            source,
        }
    }
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
