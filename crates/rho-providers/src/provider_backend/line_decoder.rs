//! Incremental LF/CRLF UTF-8 line decoder for streaming byte sources.
//!
//! Shared infrastructure for provider HTTP streams and local process NDJSON
//! (Claude CLI). Bound policy is explicit at construction so call sites stay
//! self-documenting.

/// Policy for the maximum accepted bytes in one decoded line (including an
/// incomplete tail retained between chunks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxLineBytes {
    /// No hard cap. Provider HTTP streams use this default.
    Unlimited,
    /// Reject a complete or unterminated line longer than this many bytes.
    Limited(usize),
}

impl MaxLineBytes {
    /// Bound each line to at most `bytes`.
    pub const fn limited(bytes: usize) -> Self {
        Self::Limited(bytes)
    }

    fn limit(self) -> Option<usize> {
        match self {
            Self::Unlimited => None,
            Self::Limited(limit) => Some(limit),
        }
    }
}

/// Failure while decoding a byte stream into UTF-8 lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineDecodeError {
    /// A complete line or finish tail was not valid UTF-8.
    InvalidUtf8(std::str::Utf8Error),
    /// A complete or unterminated line exceeded the configured byte limit.
    LineTooLong { bytes: usize, limit: usize },
}

impl std::fmt::Display for LineDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUtf8(error) => {
                write!(f, "invalid UTF-8 in stream line: {error}")
            }
            Self::LineTooLong { bytes, limit } => {
                write!(
                    f,
                    "stream line exceeds {limit} bytes (saw at least {bytes})"
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
/// Complete lines borrow the decoder's buffer. Before appending another chunk,
/// the decoder compacts at most the unconsumed tail from the previous chunk.
///
/// `push` stays infallible. Bound and UTF-8 failures surface on `next_line` /
/// `finish`. Callers must treat `finish` errors the same as `next_line` errors.
#[derive(Debug)]
pub struct LineDecoder {
    buffer: Vec<u8>,
    start: usize,
    max_line_bytes: MaxLineBytes,
    pending_error: Option<LineDecodeError>,
}

impl Default for LineDecoder {
    /// Unbounded decoder for provider HTTP streams.
    fn default() -> Self {
        Self::unlimited()
    }
}

impl LineDecoder {
    /// Build a decoder with an explicit line-size policy.
    pub fn new(max_line_bytes: MaxLineBytes) -> Self {
        Self {
            buffer: Vec::new(),
            start: 0,
            max_line_bytes,
            pending_error: None,
        }
    }

    /// Unbounded incomplete-line buffer (provider HTTP streams).
    pub fn unlimited() -> Self {
        Self::new(MaxLineBytes::Unlimited)
    }

    /// Reject any single line longer than `max_line_bytes`.
    pub fn with_max_line_bytes(max_line_bytes: usize) -> Self {
        Self::new(MaxLineBytes::limited(max_line_bytes))
    }

    /// Append the next chunk. Oversize incomplete lines set a pending error and
    /// stop accepting further bytes for that tail when a limit is configured.
    pub fn push(&mut self, chunk: &[u8]) {
        if self.pending_error.is_some() || chunk.is_empty() {
            return;
        }
        self.compact();

        let Some(limit) = self.max_line_bytes.limit() else {
            self.buffer.extend_from_slice(chunk);
            return;
        };

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
                let total = current_len.saturating_add(newline_at);
                if total > limit {
                    self.pending_error = Some(LineDecodeError::LineTooLong {
                        bytes: total,
                        limit,
                    });
                    // Keep already-complete lines; drop only the runaway tail.
                    self.buffer.truncate(line_start);
                    return;
                }
                self.buffer.extend_from_slice(&rest[..=newline_at]);
                offset += newline_at + 1;
            } else {
                let total = current_len.saturating_add(rest.len());
                if total > limit {
                    self.pending_error = Some(LineDecodeError::LineTooLong {
                        bytes: total,
                        limit,
                    });
                    self.buffer.truncate(line_start);
                    return;
                }
                self.buffer.extend_from_slice(rest);
                return;
            }
        }
    }

    /// Return the next complete line, if any.
    pub fn next_line(&mut self) -> Result<Option<&str>, LineDecodeError> {
        if let Some(relative_end) = self.buffer[self.start..]
            .iter()
            .position(|byte| *byte == b'\n')
        {
            if let Some(limit) = self.max_line_bytes.limit() {
                if relative_end > limit {
                    let end = self.start + relative_end;
                    self.start = end + 1;
                    return Err(LineDecodeError::LineTooLong {
                        bytes: relative_end,
                        limit,
                    });
                }
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
    /// Callers must not discard `Err` here. Invalid UTF-8 and oversize tails are
    /// stream errors for bounded decoders.
    pub fn finish(&mut self) -> Result<Option<&str>, LineDecodeError> {
        if let Some(error) = self.pending_error.take() {
            self.start = self.buffer.len();
            return Err(error);
        }
        if self.start == self.buffer.len() {
            return Ok(None);
        }
        let raw_len = self.buffer.len() - self.start;
        if let Some(limit) = self.max_line_bytes.limit() {
            if raw_len > limit {
                self.start = self.buffer.len();
                return Err(LineDecodeError::LineTooLong {
                    bytes: raw_len,
                    limit,
                });
            }
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
