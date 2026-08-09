//! Drive one spawned Claude child: prompt in, stream-json out, exit status.
//!
//! Both Claude paths share this loop. It owns the mechanics - concurrent stdin,
//! line decoding, bounded stderr capture, cancellation - and leaves policy to
//! the caller's effect closure and to how the caller reads [`DrainEnd`].

use rho_sdk::CancellationToken;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

use super::{
    child::OwnedChild,
    line_decoder::{claude_ndjson_line_decoder, LineDecodeError},
    stream::{StreamEffect, StreamMapper, TerminalResult},
};

/// Bytes of child stderr kept for diagnosis. The one-shot path writes no log
/// file, so a failure has to explain itself from memory.
const MAX_STDERR_BYTES: usize = 8 * 1024;
const READ_CHUNK_BYTES: usize = 8 * 1024;

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

/// Write `prompt` to the child, map its stream-json stdout, and wait for exit.
///
/// Every mapped effect reaches `on_effect`; terminal results are also recorded
/// on the returned [`Drained`], because both callers judge them after exit.
pub(crate) async fn drain_child(
    child: &mut OwnedChild,
    prompt: &str,
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

    // Prompt on stdin so shell metacharacters cannot break the command line.
    // Write stdin concurrently with the stdout drain: a child that emits enough
    // output before consuming stdin would otherwise fill the pipe and deadlock
    // if we awaited the full prompt write first.
    let stdin = child.stdin();
    let prompt = prompt.to_string();
    let stdin_write = async move {
        let Some(mut stdin) = stdin else {
            return Ok(());
        };
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.shutdown().await
    };
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
    tokio::pin!(stdin_write);
    tokio::pin!(read_stderr);

    let mut stdout = BufReader::new(stdout);
    let mut decoder = claude_ndjson_line_decoder();
    let mut mapper = StreamMapper::new();
    let mut terminal: Option<TerminalResult> = None;
    let mut stderr_text = String::new();
    let mut stdin_done = false;
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut exit_result = None;
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
                // Dropping the pinned stdin future closes ChildStdin; the caller
                // terminates the tree so nothing is left orphaned.
                break Some(DrainEnd::Cancelled);
            }
            result = &mut stdin_write, if !stdin_done => {
                stdin_done = true;
                if let Err(error) = result {
                    // Stdin write failures take precedence over later stream
                    // noise: the child often exits uncleanly once its stdin pipe
                    // is dropped mid-protocol.
                    break Some(DrainEnd::StdinFailed(format!(
                        "claude code: failed to write prompt to stdin: {error}"
                    )));
                }
            }
            captured = &mut read_stderr, if !stderr_done => {
                stderr_done = true;
                stderr_text = captured.finish();
            }
            status = child.wait(), if exit_result.is_none() => {
                exit_result = Some(status);
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
                                    apply_line(&mut mapper, &mut terminal, on_effect, &line);
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
                    let line = line.to_string();
                    apply_line(&mut mapper, &mut terminal, on_effect, &line);
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

fn apply_line(
    mapper: &mut StreamMapper,
    terminal: &mut Option<TerminalResult>,
    on_effect: &mut (dyn FnMut(StreamEffect) + Send),
    line: &str,
) {
    for effect in mapper.push_line(line) {
        if let StreamEffect::Terminal(result) = &effect {
            // Later terminals (for example a final `result`) replace earlier
            // pending protocol-error metadata.
            *terminal = Some(result.clone());
        }
        on_effect(effect);
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
