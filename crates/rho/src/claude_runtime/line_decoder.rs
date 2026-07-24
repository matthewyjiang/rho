//! Incremental LF/CRLF line decoder for local NDJSON streams.
//!
//! Owned here so the Claude adapter does not depend on private provider
//! backend types. Behaviour matches the provider line decoder, plus a hard
//! cap so a hostile or runaway child cannot grow the incomplete-line buffer
//! without bound.

/// Maximum accepted bytes in one NDJSON line, including an incomplete tail.
///
/// Claude stream-json lines are usually small. Tool inputs can be large, but
/// multi-megabyte single lines are not treated as legitimate protocol traffic.
pub(crate) const MAX_NDJSON_LINE_BYTES: usize = 1024 * 1024;

/// Failure while decoding a local stream-json byte stream into lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LineDecodeError {
    /// A complete line or finish tail was not valid UTF-8.
    InvalidUtf8(std::str::Utf8Error),
    /// A complete or unterminated line exceeded [`MAX_NDJSON_LINE_BYTES`].
    LineTooLong { bytes: usize, limit: usize },
}

impl std::fmt::Display for LineDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUtf8(error) => {
                write!(f, "invalid UTF-8 in stream-json line: {error}")
            }
            Self::LineTooLong { bytes, limit } => {
                write!(
                    f,
                    "stream-json line exceeds {limit} bytes (saw at least {bytes})"
                )
            }
        }
    }
}

impl std::error::Error for LineDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidUtf8(error) => Some(error),
            Self::LineTooLong { .. } => None,
        }
    }
}

impl From<std::str::Utf8Error> for LineDecodeError {
    fn from(error: std::str::Utf8Error) -> Self {
        Self::InvalidUtf8(error)
    }
}

/// Incrementally decodes LF- or CRLF-terminated UTF-8 lines without moving the
/// unprocessed buffer after every line.
///
/// `push` stays infallible so existing call sites keep compiling. Bound and
/// UTF-8 failures surface on `next_line` / `finish`. Session callers must treat
/// `finish` errors the same as `next_line` errors (do not ignore them with
/// `if let Ok(...)`).
#[derive(Default)]
pub(crate) struct LineDecoder {
    buffer: Vec<u8>,
    start: usize,
    pending_error: Option<LineDecodeError>,
}

impl LineDecoder {
    /// Append the next stdout chunk. Oversize incomplete lines set a pending
    /// error and stop accepting further bytes for that tail.
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        if self.pending_error.is_some() || chunk.is_empty() {
            return;
        }
        self.compact();

        // Segment by newlines so oversize tails fail without a per-byte loop
        // and without retaining multi-megabyte runaway input.
        let mut offset = 0;
        while offset < chunk.len() {
            if self.pending_error.is_some() {
                return;
            }
            let rest = &chunk[offset..];
            let line_start = current_line_start(&self.buffer, self.start);
            let current_len = self.buffer.len() - line_start;

            if let Some(newline_at) = rest.iter().position(|byte| *byte == b'\n') {
                // Bytes before the newline form the remainder of this line.
                let total = current_len.saturating_add(newline_at);
                if total > MAX_NDJSON_LINE_BYTES {
                    self.pending_error = Some(LineDecodeError::LineTooLong {
                        bytes: total,
                        limit: MAX_NDJSON_LINE_BYTES,
                    });
                    // Keep already-complete lines; drop only the runaway tail.
                    self.buffer.truncate(line_start);
                    // Drop the rest of this chunk. Session fails the run after
                    // the pending error surfaces.
                    return;
                }
                self.buffer.extend_from_slice(&rest[..=newline_at]);
                offset += newline_at + 1;
            } else {
                let total = current_len.saturating_add(rest.len());
                if total > MAX_NDJSON_LINE_BYTES {
                    self.pending_error = Some(LineDecodeError::LineTooLong {
                        bytes: total,
                        limit: MAX_NDJSON_LINE_BYTES,
                    });
                    self.buffer.truncate(line_start);
                    return;
                }
                self.buffer.extend_from_slice(rest);
                return;
            }
        }
    }

    pub(crate) fn next_line(&mut self) -> Result<Option<&str>, LineDecodeError> {
        if let Some(relative_end) = self.buffer[self.start..]
            .iter()
            .position(|byte| *byte == b'\n')
        {
            if relative_end > MAX_NDJSON_LINE_BYTES {
                let end = self.start + relative_end;
                self.start = end + 1;
                return Err(LineDecodeError::LineTooLong {
                    bytes: relative_end,
                    limit: MAX_NDJSON_LINE_BYTES,
                });
            }
            let end = self.start + relative_end;
            let line_end = end - usize::from(end > self.start && self.buffer[end - 1] == b'\r');
            let line = std::str::from_utf8(&self.buffer[self.start..line_end])?;
            self.start = end + 1;
            return Ok(Some(line));
        }

        if let Some(error) = self.pending_error.take() {
            return Err(error);
        }
        Ok(None)
    }

    /// Return the final unterminated tail, if any.
    ///
    /// Session handling must not discard `Err` here. Invalid UTF-8 and oversize
    /// tails are fatal stream errors for the Claude runtime.
    pub(crate) fn finish(&mut self) -> Result<Option<&str>, LineDecodeError> {
        if let Some(error) = self.pending_error.take() {
            self.start = self.buffer.len();
            return Err(error);
        }
        if self.start == self.buffer.len() {
            return Ok(None);
        }
        let raw_len = self.buffer.len() - self.start;
        if raw_len > MAX_NDJSON_LINE_BYTES {
            self.start = self.buffer.len();
            return Err(LineDecodeError::LineTooLong {
                bytes: raw_len,
                limit: MAX_NDJSON_LINE_BYTES,
            });
        }
        let line_end =
            self.buffer.len() - usize::from(self.buffer.last().is_some_and(|byte| *byte == b'\r'));
        let line = std::str::from_utf8(&self.buffer[self.start..line_end])?;
        self.start = self.buffer.len();
        Ok(Some(line))
    }

    fn compact(&mut self) {
        if self.start == 0 {
            return;
        }
        if self.start == self.buffer.len() {
            self.buffer.clear();
        } else {
            self.buffer.copy_within(self.start.., 0);
            self.buffer.truncate(self.buffer.len() - self.start);
        }
        self.start = 0;
    }
}

fn current_line_start(buffer: &[u8], start: usize) -> usize {
    buffer[start..]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|relative| start + relative + 1)
        .unwrap_or(start)
}

#[cfg(test)]
#[path = "line_decoder_tests.rs"]
mod tests;
