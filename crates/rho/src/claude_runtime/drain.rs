//! Drive one spawned Claude child: prompt in, stream-json out, exit status.
//!
//! Both Claude paths share this loop. It owns the mechanics - concurrent stdin,
//! line decoding, bounded stderr capture, cancellation - and leaves policy to
//! the caller's effect closure and to how the caller reads [`DrainEnd`].

use rho_sdk::CancellationToken;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, oneshot};

use super::{
    child::OwnedChild,
    line_decoder::{claude_ndjson_line_decoder, LineDecodeError},
    messaging,
    stream::{StreamEffect, StreamMapper, TerminalResult},
};

/// Bytes of child stderr kept for diagnosis. The one-shot path writes no log
/// file, so a failure has to explain itself from memory.
const MAX_STDERR_BYTES: usize = 8 * 1024;
const READ_CHUNK_BYTES: usize = 8 * 1024;
/// Child closed stdin early (flag rejection, quick exit). Non-fatal for the
/// drain so exit status and stderr diagnosis still win.
const STDIN_BROKEN_PIPE: &str = "claude code: stdin closed by child (broken pipe)";

/// How the drain feeds the child's stdin.
pub(crate) enum DrainInput {
    /// One plain-text prompt, then close stdin (one-shot path).
    Text { prompt: String },
    /// stream-json user turns. Writes the initial prompt, keeps stdin open for
    /// parent follow-ups, and closes after a terminal `result` once the parent
    /// queue is empty.
    StreamJson {
        initial_prompt: String,
        parent_messages: Option<messaging::ClaudeMessageInbox>,
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
    input: DrainInput,
    cancellation: &CancellationToken,
    on_effect: &mut (dyn FnMut(StreamEffect) + Send),
) -> Drained {
    let Some(stdout) = child.stdout() else {
        return Drained {
            terminal: None,
            stderr: String::new(),
            end: DrainEnd::StreamFailed("claude code: child stdout was not captured".into()),
        };
    };

    let Some(stdin) = child.stdin() else {
        return Drained {
            terminal: None,
            stderr: String::new(),
            end: DrainEnd::StdinFailed("claude code: child stdin was not captured".into()),
        };
    };

    let (close_tx, stdin_write) = match input {
        DrainInput::Text { prompt } => {
            let write = tokio::spawn(async move { write_text_stdin(stdin, prompt).await });
            (None, write)
        }
        DrainInput::StreamJson {
            initial_prompt,
            parent_messages,
        } => {
            let (close_tx, close_rx) = oneshot::channel::<()>();
            let write = tokio::spawn(async move {
                write_stream_json_stdin(stdin, initial_prompt, parent_messages, close_rx).await
            });
            (Some(close_tx), write)
        }
    };
    tokio::pin!(stdin_write);

    // Stderr is optional: session redirects it to a log file.
    let stderr = child.stderr();
    let read_stderr = async move {
        let mut tail = StderrTail::default();
        let Some(mut stderr) = stderr else {
            return tail;
        };
        let mut chunk = vec![0_u8; READ_CHUNK_BYTES];
        loop {
            match stderr.read(&mut chunk).await {
                // A stderr read error is not worth failing the run over; the
                // tail collected so far still explains what happened.
                Ok(0) | Err(_) => return tail,
                Ok(count) => tail.push(&chunk[..count]),
            }
        }
    };
    tokio::pin!(read_stderr);

    let mut stdout = BufReader::new(stdout);
    let mut decoder = claude_ndjson_line_decoder();
    let mut mapper = StreamMapper::new();
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
                    if error != STDIN_BROKEN_PIPE {
                        break Some(DrainEnd::StdinFailed(error));
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
                                    decode_error = Some(format_line_error(&error));
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
                            "claude code: failed reading stdout: {error}"
                        )));
                    }
                }
            }
        }
    };

    let end = match early_end {
        Some(end) => end,
        None => match decoder.finish() {
            Err(error) => DrainEnd::StreamFailed(format_line_error(&error)),
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

async fn write_text_stdin(mut stdin: ChildStdin, prompt: String) -> Result<(), String> {
    write_all(&mut stdin, prompt.as_bytes()).await?;
    shutdown_stdin(&mut stdin).await
}

async fn write_stream_json_stdin(
    mut stdin: ChildStdin,
    initial_prompt: String,
    mut parent_messages: Option<messaging::ClaudeMessageInbox>,
    close_rx: oneshot::Receiver<()>,
) -> Result<(), String> {
    write_all(
        &mut stdin,
        messaging::encode_user_turn(&initial_prompt).as_bytes(),
    )
    .await?;

    let mut close_rx = Some(close_rx);
    loop {
        // Drain any parent messages already queued, then wait for either a new
        // parent turn or the close signal from the stream side.
        if let Some(inbox) = parent_messages.as_mut() {
            match inbox.try_recv() {
                Ok(text) => {
                    write_parent_turn(&mut stdin, &text).await?;
                    continue;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    parent_messages = None;
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
                if let Some(inbox) = parent_messages.as_ref() {
                    inbox.seal();
                }
            }
            maybe_text = recv_parent(&mut parent_messages), if parent_messages.is_some() => {
                if let Some(text) = maybe_text {
                    write_parent_turn(&mut stdin, &text).await?;
                }
            }
        }
    }

    // After seal, wait until every accepted (including in-flight) body is
    // written. `recv` ends only when all sender clones are gone.
    if let Some(mut inbox) = parent_messages.take() {
        // Seal is idempotent; covers paths that broke without a close signal.
        inbox.seal();
        while let Some(text) = inbox.recv().await {
            write_parent_turn(&mut stdin, &text).await?;
        }
    }

    shutdown_stdin(&mut stdin).await
}

async fn write_parent_turn(stdin: &mut ChildStdin, text: &str) -> Result<(), String> {
    write_all(
        stdin,
        messaging::encode_user_turn(&messaging::frame_parent_message(text)).as_bytes(),
    )
    .await
}

async fn write_all(stdin: &mut ChildStdin, mut bytes: &[u8]) -> Result<(), String> {
    while !bytes.is_empty() {
        match stdin.write(bytes).await {
            Ok(0) => {
                return Err("claude code: failed to write prompt to stdin: wrote 0 bytes".into());
            }
            Ok(count) => bytes = &bytes[count..],
            Err(error) => return Err(map_stdin_io_error(error)),
        }
    }
    Ok(())
}

async fn shutdown_stdin(stdin: &mut ChildStdin) -> Result<(), String> {
    match stdin.shutdown().await {
        Ok(()) => Ok(()),
        Err(error) => Err(map_stdin_io_error(error)),
    }
}

fn map_stdin_io_error(error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        STDIN_BROKEN_PIPE.into()
    } else {
        format!("claude code: failed to write prompt to stdin: {error}")
    }
}

/// Waits for the next parent message when a receiver is still installed.
///
/// Returns `None` when the parent handle is dropped (channel closed).
async fn recv_parent(
    parent_messages: &mut Option<messaging::ClaudeMessageInbox>,
) -> Option<String> {
    let Some(inbox) = parent_messages.as_mut() else {
        std::future::pending::<()>().await;
        unreachable!("pending future resolved");
    };
    match inbox.recv().await {
        Some(text) => Some(text),
        None => {
            *parent_messages = None;
            None
        }
    }
}

pub(crate) fn format_line_error(error: &LineDecodeError) -> String {
    match error {
        LineDecodeError::InvalidUtf8(_) => {
            format!("claude code: malformed UTF-8 on stream-json stdout: {error}")
        }
        LineDecodeError::LineTooLong { .. } => {
            format!("claude code: oversize stream-json line: {error}")
        }
    }
}

/// The last [`MAX_STDERR_BYTES`] of a child's stderr.
///
/// Reading to EOF into one buffer would let a chatty child grow without bound,
/// so the head is dropped as chunks arrive. Keeping the tail matches the log
/// file the session path reads: the closing lines carry the failure, while the
/// head is startup noise.
#[derive(Default)]
struct StderrTail {
    bytes: Vec<u8>,
    elided: bool,
}

impl StderrTail {
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() <= MAX_STDERR_BYTES {
            return;
        }
        let cut = ceil_utf8_boundary(&self.bytes, self.bytes.len() - MAX_STDERR_BYTES);
        self.bytes.drain(..cut);
        self.elided = true;
    }

    fn finish(self) -> String {
        let text = String::from_utf8_lossy(&self.bytes);
        let trimmed = text.trim();
        if self.elided {
            format!("{}{trimmed}", rho_sdk::ELLIPSIS)
        } else {
            trimmed.to_string()
        }
    }
}

/// First character start at or after `index`.
///
/// [`rho_sdk::ceil_char_boundary`] answers this for `&str`; the stderr tail is
/// cut while it is still raw bytes, before any decode, so the walk is over the
/// UTF-8 continuation-byte pattern instead.
fn ceil_utf8_boundary(bytes: &[u8], index: usize) -> usize {
    let mut index = index.min(bytes.len());
    while index < bytes.len() && bytes[index] & 0b1100_0000 == 0b1000_0000 {
        index += 1;
    }
    index
}

#[cfg(test)]
#[path = "drain_tests.rs"]
mod tests;
