//! Shared `\n` splitting for window reads and grep.
//!
//! Content visits match [`super::iter_content_lines`]: a trailing newline does
//! not invent a blank line, and a trailing `\r` is stripped.

use std::{io::Read, ops::ControlFlow};

/// Streaming split of `\n` segments.
pub(super) struct LineSplit {
    pending: Vec<u8>,
    /// Next 1-based content line number.
    line_number: usize,
    pub(super) bytes: usize,
}

impl LineSplit {
    pub(super) fn new() -> Self {
        Self {
            pending: Vec::new(),
            line_number: 1,
            bytes: 0,
        }
    }

    /// Feed a chunk that may contain several `\n`-terminated segments.
    pub(super) fn push<E>(
        &mut self,
        chunk: &[u8],
        mut visit: impl FnMut(usize, &[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        self.bytes = self.bytes.saturating_add(chunk.len());
        let mut rest = chunk;
        if !self.pending.is_empty() {
            match rest.iter().position(|&byte| byte == b'\n') {
                Some(index) => {
                    self.pending.extend_from_slice(&rest[..index]);
                    self.finish_content_line(&mut visit)?;
                    rest = &rest[index + 1..];
                }
                None => {
                    self.pending.extend_from_slice(rest);
                    return Ok(());
                }
            }
        }
        while let Some(index) = rest.iter().position(|&byte| byte == b'\n') {
            self.consume_line(&rest[..index], &mut visit)?;
            rest = &rest[index + 1..];
        }
        if !rest.is_empty() {
            self.pending.extend_from_slice(rest);
        }
        Ok(())
    }

    /// Visits the unterminated last line, if any, and returns the content
    /// line count. An empty `pending` means an empty file or a trailing
    /// newline, neither of which adds a line.
    pub(super) fn finish<E>(
        mut self,
        mut visit: impl FnMut(usize, &[u8]) -> Result<(), E>,
    ) -> Result<usize, E> {
        if !self.pending.is_empty() {
            self.finish_content_line(&mut visit)?;
        }
        Ok(self.content_lines())
    }

    pub(super) fn content_lines(&self) -> usize {
        if self.bytes == 0 {
            0
        } else {
            self.line_number.saturating_sub(1)
        }
    }

    fn finish_content_line<E>(
        &mut self,
        visit: &mut impl FnMut(usize, &[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        let pending = std::mem::take(&mut self.pending);
        self.consume_line(&pending, visit)
    }

    fn consume_line<E>(
        &mut self,
        line: &[u8],
        visit: &mut impl FnMut(usize, &[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        visit(self.line_number, line)?;
        self.line_number = self.line_number.saturating_add(1);
        Ok(())
    }
}

pub(super) fn decode_content_line(bytes: &[u8]) -> Result<&str, ()> {
    let line = std::str::from_utf8(bytes).map_err(|_| ())?;
    Ok(line.strip_suffix('\r').unwrap_or(line))
}

/// Streaming content-line visit matching [`super::iter_content_lines`].
///
/// Returns `None` when `max_bytes` is exceeded, a NUL appears in the first
/// `sniff_bytes`, or a visited line is not UTF-8 text. `visit` may stop the
/// scan early with [`ControlFlow::Break`].
pub(crate) fn read_searchable_lines(
    mut reader: impl Read,
    max_bytes: u64,
    sniff_bytes: usize,
    mut visit: impl FnMut(usize, &str) -> ControlFlow<()>,
) -> Option<()> {
    let mut split = LineSplit::new();
    let mut buf = [0_u8; 64 * 1024];
    let mut sniff_remaining = sniff_bytes;
    let mut visiting = true;
    loop {
        let n = reader.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        if sniff_remaining > 0 {
            let sniff = &chunk[..sniff_remaining.min(chunk.len())];
            if sniff.contains(&0) {
                return None;
            }
            sniff_remaining = sniff_remaining.saturating_sub(sniff.len());
        }
        if split.bytes.saturating_add(n) as u64 > max_bytes {
            return None;
        }
        split
            .push(chunk, |line_number, line| {
                if visiting {
                    let decoded = decode_content_line(line)?;
                    if visit(line_number, decoded).is_break() {
                        visiting = false;
                    }
                }
                Ok::<(), ()>(())
            })
            .ok()?;
        if !visiting {
            return Some(());
        }
    }
    split
        .finish(|line_number, line| {
            if visiting {
                let decoded = decode_content_line(line)?;
                if visit(line_number, decoded).is_break() {
                    visiting = false;
                }
            }
            Ok::<(), ()>(())
        })
        .ok()?;
    Some(())
}
