//! Drive one spawned CLI child: prompt in, stream-json out, exit status.
//!
//! External CLI runtimes (Claude Code and Cursor) share this loop. It owns the
//! mechanics - concurrent stdin, line decoding, bounded stderr capture,
//! cancellation - and leaves policy to the caller's mapper, effect closure,
//! and how the caller reads [`DrainEnd`]. Only labels and stream mappers
//! differ per CLI.
//!
//! # Stream-json stdin
//!
//! [`DrainInput::StreamJson`] takes an already-encoded `initial_line` plus an
//! optional [`FollowUpSource`]. Claude's user-turn JSON
//! (`encode_user_turn` / `frame_parent_message`) stays in the Claude messaging
//! module; the drain only writes bytes. That keeps this module free of
//! CLI-specific wire formats while avoiding a renamed generic inbox used at
//! every parent-message call site.

use std::future::Future;
use std::pin::Pin;

use rho_providers::provider_backend::line_decoder::LineDecoder;
use rho_sdk::CancellationToken;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, oneshot};

use super::line_decoder::LineDecodeError;
use super::stream_effect::{StreamEffect, TerminalResult};
use super::{OwnedChild, StderrTail};

const READ_CHUNK_BYTES: usize = 8 * 1024;

/// One CLI's NDJSON-line → effect policy. Implementors must be fail-soft:
/// bad JSON becomes a notice effect, never an error.
pub(crate) trait StreamLineMapper: Send {
    fn push_line(&mut self, line: &str) -> Vec<StreamEffect>;
}

/// Drain bounds and labels that differ per CLI.
pub(crate) struct DrainConfig {
    pub(crate) program_label: &'static str,
    pub(crate) max_line_bytes: usize,
}

/// Typed stdin write failure so broken-pipe can be ignored without comparing
/// formatted strings.
enum StdinWriteError {
    BrokenPipe,
    Other(String),
}

/// Source of already-encoded follow-up stdin lines after the initial user turn.
///
/// Implementors own framing. `recv` is boxed so the trait stays object-safe
/// for [`DrainInput::StreamJson`].
pub(crate) trait FollowUpSource: Send {
    fn try_recv(&mut self) -> Result<String, mpsc::error::TryRecvError>;
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>>;
    fn seal(&self);
}

/// How the drain feeds the child's stdin.
pub(crate) enum DrainInput {
    /// One plain-text prompt, then close stdin (one-shot path).
    Text { prompt: String },
    /// stream-json user turns. Writes `initial_line`, keeps stdin open for
    /// parent follow-ups, and closes after a terminal `result` once the parent
    /// queue is empty.
    StreamJson {
        initial_line: String,
        follow_ups: Option<Box<dyn FollowUpSource>>,
    },
}

/// How a drained child stopped.
pub(crate) enum DrainEnd {
    /// Cancellation fired. The child is still mid-protocol.
    Cancelled,
    /// The prompt could not be written.
    StdinFailed(String),
    /// Stdout could not be read or decoded.
    StreamFailed(String),
    /// The child was waited on. `Ok` means it was reaped, so its tree is gone;
    /// every other end leaves a live tree for the caller to terminate.
    Exited(std::io::Result<std::process::ExitStatus>),
}

/// What one drained child produced, before its exit status was judged.
pub(crate) struct Drained {
    /// Last terminal result seen on the stream, if any.
    pub(crate) terminal: Option<TerminalResult>,
    /// Tail of the child's stderr, empty when stderr was not piped.
    pub(crate) stderr: String,
    pub(crate) end: DrainEnd,
}

/// Write stdin according to [`DrainInput`], map stream-json stdout, and wait.
///
/// Every mapped effect reaches `on_effect`; terminal results are also recorded
/// on the returned [`Drained`], because both callers judge them after exit.
pub(crate) async fn drain_child(
    child: &mut OwnedChild,
    config: DrainConfig,
    mapper: &mut dyn StreamLineMapper,
    input: DrainInput,
    cancellation: &CancellationToken,
    on_effect: &mut (dyn FnMut(StreamEffect) + Send),
) -> Drained {
    let Some(stdout) = child.stdout() else {
        return Drained {
            terminal: None,
            stderr: String::new(),
            end: DrainEnd::StreamFailed(format!(
                "{}: child stdout was not captured",
                config.program_label
            )),
        };
    };

    let Some(stdin) = child.stdin() else {
        return Drained {
            terminal: None,
            stderr: String::new(),
            end: DrainEnd::StdinFailed(format!(
                "{}: child stdin was not captured",
                config.program_label
            )),
        };
    };

    let program_label = config.program_label;
    let (close_tx, stdin_write) = match input {
        DrainInput::Text { prompt } => {
            let write =
                tokio::spawn(async move { write_text_stdin(stdin, prompt, program_label).await });
            (None, write)
        }
        DrainInput::StreamJson {
            initial_line,
            follow_ups,
        } => {
            let (close_tx, close_rx) = oneshot::channel::<()>();
            let write = tokio::spawn(async move {
                write_stream_json_stdin(stdin, initial_line, follow_ups, close_rx, program_label)
                    .await
            });
            (Some(close_tx), write)
        }
    };
    tokio::pin!(stdin_write);

    // Stderr is optional: session redirects it to a log file.
    let read_stderr = StderrTail::capture(child.stderr());
    tokio::pin!(read_stderr);

    let mut stdout = BufReader::new(stdout);
    let mut decoder = LineDecoder::with_max_line_bytes(config.max_line_bytes);
    let mut terminal: Option<TerminalResult> = None;
    let mut stderr_text = String::new();
    let mut stderr_done = false;
    let mut stdout_done = false;
    let mut stdin_done = false;
    let mut exit_result = None;
    let mut close_tx = close_tx;
    let mut chunk = vec![0_u8; READ_CHUNK_BYTES];

    // `None` means every pipe closed and the child was reaped. Observe exit in
    // parallel with the pipes: a descendant may inherit one of them, and
    // `OwnedChild::wait` kills those leftover process-group members once the
    // leader exits. Waiting for their EOF before reaping would deadlock.
    let early_end: Option<DrainEnd> = loop {
        if stdin_done && stdout_done && stderr_done && exit_result.is_some() {
            break None;
        }
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                // Dropping the stdin task closes ChildStdin; the caller
                // terminates the tree so nothing is left orphaned.
                break Some(DrainEnd::Cancelled);
            }
            result = &mut stdin_write, if !stdin_done => {
                stdin_done = true;
                if let Ok(Err(error)) = result {
                    // Broken pipe means the child closed stdin, usually because
                    // it already exited (flag rejection, early error). Keep
                    // draining so exit status and stderr diagnosis win over a
                    // bare pipe error. Other write failures still abort: the
                    // child often exits uncleanly once its stdin is dropped
                    // mid-protocol.
                    match error {
                        StdinWriteError::BrokenPipe => {}
                        StdinWriteError::Other(message) => {
                            break Some(DrainEnd::StdinFailed(message));
                        }
                    }
                }
            }
            captured = &mut read_stderr, if !stderr_done => {
                stderr_done = true;
                stderr_text = captured.finish();
            }
            status = child.wait(), if exit_result.is_none() => {
                exit_result = Some(status);
                // Child is gone; stop waiting on stdin writes.
                drop(close_tx.take());
            }
            read = stdout.read(&mut chunk), if !stdout_done => {
                match read {
                    Ok(0) => stdout_done = true,
                    Ok(count) => {
                        decoder.push(&chunk[..count]);
                        let mut decode_error = None;
                        loop {
                            match decoder.next_line() {
                                Ok(Some(line)) => {
                                    let line = line.to_string();
                                    for effect in mapper.push_line(&line) {
                                        if let StreamEffect::Terminal(result) = &effect {
                                            // Later terminals replace earlier
                                            // pending protocol-error metadata.
                                            terminal = Some(result.clone());
                                            // Ask the stdin writer to close once
                                            // its parent queue is idle.
                                            if let Some(tx) = close_tx.take() {
                                                let _ = tx.send(());
                                            }
                                        }
                                        on_effect(effect);
                                    }
                                }
                                Ok(None) => break,
                                Err(error) => {
                                    decode_error = Some(format_line_error(&error, program_label));
                                    break;
                                }
                            }
                        }
                        if let Some(error) = decode_error {
                            break Some(DrainEnd::StreamFailed(error));
                        }
                    }
                    Err(error) => {
                        break Some(DrainEnd::StreamFailed(format!(
                            "{program_label}: failed reading stdout: {error}"
                        )));
                    }
                }
            }
        }
    };

    let end = match early_end {
        Some(end) => end,
        None => match decoder.finish() {
            Err(error) => DrainEnd::StreamFailed(format_line_error(&error, program_label)),
            Ok(tail) => {
                if let Some(line) = tail {
                    for effect in mapper.push_line(line) {
                        if let StreamEffect::Terminal(result) = &effect {
                            terminal = Some(result.clone());
                        }
                        on_effect(effect);
                    }
                }
                DrainEnd::Exited(exit_result.expect("completed drain reaped the child"))
            }
        },
    };

    Drained {
        terminal,
        stderr: stderr_text,
        end,
    }
}

async fn write_text_stdin(
    mut stdin: ChildStdin,
    prompt: String,
    program_label: &'static str,
) -> Result<(), StdinWriteError> {
    write_all(&mut stdin, prompt.as_bytes(), program_label).await?;
    shutdown_stdin(&mut stdin, program_label).await
}

async fn write_stream_json_stdin(
    mut stdin: ChildStdin,
    initial_line: String,
    mut follow_ups: Option<Box<dyn FollowUpSource>>,
    close_rx: oneshot::Receiver<()>,
    program_label: &'static str,
) -> Result<(), StdinWriteError> {
    write_all(&mut stdin, initial_line.as_bytes(), program_label).await?;

    let mut close_rx = Some(close_rx);
    loop {
        // Drain any parent messages already queued, then wait for either a new
        // parent turn or the close signal from the stream side.
        if let Some(inbox) = follow_ups.as_mut() {
            match inbox.try_recv() {
                Ok(line) => {
                    write_all(&mut stdin, line.as_bytes(), program_label).await?;
                    continue;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    follow_ups = None;
                }
            }
        }

        let Some(close) = close_rx.as_mut() else {
            break;
        };

        tokio::select! {
            biased;
            result = close => {
                let _ = result;
                close_rx = None;
                // Stop acknowledging new parent sends before the final drain so
                // a concurrent `agents message` cannot succeed and then vanish.
                if let Some(inbox) = follow_ups.as_ref() {
                    inbox.seal();
                }
            }
            maybe_line = recv_follow_up(&mut follow_ups), if follow_ups.is_some() => {
                if let Some(line) = maybe_line {
                    write_all(&mut stdin, line.as_bytes(), program_label).await?;
                }
            }
        }
    }

    // After seal, wait until every accepted (including in-flight) body is
    // written. `recv` ends only when all sender clones are gone.
    if let Some(mut inbox) = follow_ups.take() {
        // Seal is idempotent; covers paths that broke without a close signal.
        inbox.seal();
        while let Some(line) = inbox.recv().await {
            write_all(&mut stdin, line.as_bytes(), program_label).await?;
        }
    }

    shutdown_stdin(&mut stdin, program_label).await
}

async fn write_all(
    stdin: &mut ChildStdin,
    mut bytes: &[u8],
    program_label: &'static str,
) -> Result<(), StdinWriteError> {
    while !bytes.is_empty() {
        match stdin.write(bytes).await {
            Ok(0) => {
                return Err(StdinWriteError::Other(format!(
                    "{program_label}: failed to write prompt to stdin: wrote 0 bytes"
                )));
            }
            Ok(count) => bytes = &bytes[count..],
            Err(error) => return Err(map_stdin_io_error(error, program_label)),
        }
    }
    Ok(())
}

async fn shutdown_stdin(
    stdin: &mut ChildStdin,
    program_label: &'static str,
) -> Result<(), StdinWriteError> {
    match stdin.shutdown().await {
        Ok(()) => Ok(()),
        Err(error) => Err(map_stdin_io_error(error, program_label)),
    }
}

fn map_stdin_io_error(error: std::io::Error, program_label: &str) -> StdinWriteError {
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        StdinWriteError::BrokenPipe
    } else {
        StdinWriteError::Other(format!(
            "{program_label}: failed to write prompt to stdin: {error}"
        ))
    }
}

/// Waits for the next follow-up line when a source is still installed.
///
/// Returns `None` when the parent handle is dropped (channel closed).
async fn recv_follow_up(follow_ups: &mut Option<Box<dyn FollowUpSource>>) -> Option<String> {
    let Some(inbox) = follow_ups.as_mut() else {
        std::future::pending::<()>().await;
        unreachable!("pending future resolved");
    };
    match inbox.recv().await {
        Some(line) => Some(line),
        None => {
            *follow_ups = None;
            None
        }
    }
}

pub(crate) fn format_line_error(error: &LineDecodeError, program_label: &str) -> String {
    match error {
        LineDecodeError::InvalidUtf8(_) => {
            format!("{program_label}: malformed UTF-8 on stream-json stdout: {error}")
        }
        LineDecodeError::LineTooLong { .. } => {
            format!("{program_label}: oversize stream-json line: {error}")
        }
    }
}

#[cfg(test)]
#[path = "drain_tests.rs"]
mod tests;
