//! Bounded capture of child process stderr for diagnostics.
//!
//! Reading stderr to EOF into one buffer would let a chatty child grow memory
//! without bound, so the head is dropped as chunks arrive. Keeping the tail
//! matches what a log file would show: the closing lines carry the failure,
//! while the head is startup noise.

use tokio::io::{AsyncRead, AsyncReadExt};

/// Bytes of child stderr kept for diagnosis.
pub(crate) const MAX_STDERR_BYTES: usize = 8 * 1024;

const READ_CHUNK_BYTES: usize = 8 * 1024;

/// The last [`MAX_STDERR_BYTES`] of a child's stderr.
#[derive(Debug, Default)]
pub(crate) struct StderrTail {
    bytes: Vec<u8>,
    elided: bool,
}

impl StderrTail {
    /// Read `stderr` to EOF, keeping only the tail. `None` (stderr redirected
    /// elsewhere) yields an empty tail. A read error is not worth failing a run
    /// over: the tail collected so far still explains what happened.
    pub(crate) async fn capture<R>(stderr: Option<R>) -> Self
    where
        R: AsyncRead + Unpin,
    {
        let mut tail = Self::default();
        let Some(mut stderr) = stderr else {
            return tail;
        };
        let mut chunk = vec![0_u8; READ_CHUNK_BYTES];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => return tail,
                Ok(count) => tail.push(&chunk[..count]),
            }
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() <= MAX_STDERR_BYTES {
            return;
        }
        let cut = ceil_utf8_boundary(&self.bytes, self.bytes.len() - MAX_STDERR_BYTES);
        self.bytes.drain(..cut);
        self.elided = true;
    }

    /// Whether any head bytes were dropped to stay within the budget.
    pub(crate) fn elided(&self) -> bool {
        self.elided
    }

    /// Trimmed text, prefixed with an ellipsis when the head was dropped.
    pub(crate) fn finish(self) -> String {
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
#[path = "stderr_tail_tests.rs"]
mod tests;
