//! Shared `\n` splitting for window reads and grep.
//!
//! Fingerprints see the same segments as `split('\n')`, including the empty
//! last segment after a trailing newline and the single empty segment of an
//! empty file. Content visits match [`super::iter_content_lines`]: a trailing
//! newline does not invent a blank line, and a trailing `\r` is stripped.

use std::{io::Read, ops::ControlFlow};

/// Optional per-`\n` digest collected while scanning.
///
/// `push_line` sees the same segments as `split('\n')`, including the empty
/// last segment after a trailing newline and the single empty segment of an
/// empty file.
pub(crate) trait LineFingerprint {
    fn push_line(&mut self, line: &[u8]);
    fn finish(self) -> String;
}

/// Streaming split of `\n` segments.
pub(super) struct LineSplit<F> {
    fingerprint: Option<F>,
    pending: Vec<u8>,
    /// Next 1-based content line number.
    line_number: usize,
    pub(super) bytes: usize,
}

impl<F: LineFingerprint> LineSplit<F> {
    pub(super) fn new(fingerprint: Option<F>) -> Self {
        Self {
            fingerprint,
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
            self.consume_line(&rest[..index], /*content*/ true, &mut visit)?;
            rest = &rest[index + 1..];
        }
        if !rest.is_empty() {
            self.pending.extend_from_slice(rest);
        }
        Ok(())
    }

    pub(super) fn finish<E>(
        mut self,
        mut visit: impl FnMut(usize, &[u8]) -> Result<(), E>,
    ) -> Result<(Option<F>, usize), E> {
        if !self.pending.is_empty() {
            self.finish_content_line(&mut visit)?;
        } else {
            // Empty file, or the empty last `split('\n')` segment after a
            // trailing newline. `pending` is empty only in those cases.
            self.consume_line(b"", /*content*/ false, &mut visit)?;
        }
        let total = self.content_lines();
        Ok((self.fingerprint, total))
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
        self.consume_line(&pending, /*content*/ true, visit)
    }

    fn consume_line<E>(
        &mut self,
        line: &[u8],
        content: bool,
        visit: &mut impl FnMut(usize, &[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        if let Some(fingerprint) = &mut self.fingerprint {
            fingerprint.push_line(line);
        }
        if content {
            visit(self.line_number, line)?;
            self.line_number = self.line_number.saturating_add(1);
        }
        Ok(())
    }
}

pub(super) fn decode_content_line(bytes: &[u8]) -> Result<&str, ()> {
    let line = std::str::from_utf8(bytes).map_err(|_| ())?;
    Ok(line.strip_suffix('\r').unwrap_or(line))
}

/// Streaming content-line visit matching [`super::iter_content_lines`].
///
/// Fingerprints see every `\n` segment, including the empty last segment after
/// a trailing newline. Content visits omit that phantom. Returns `None` when
/// `max_bytes` is exceeded, a NUL appears in the first `sniff_bytes`, or the
/// file is not UTF-8 text. `visit` may stop content visits with
/// [`ControlFlow::Break`]; hashing still finishes when a fingerprint was
/// supplied.
pub(crate) fn read_searchable_lines<F: LineFingerprint>(
    mut reader: impl Read,
    fingerprint: Option<F>,
    max_bytes: u64,
    sniff_bytes: usize,
    mut visit: impl FnMut(usize, &str) -> ControlFlow<()>,
) -> Option<Option<String>> {
    let need_hash = fingerprint.is_some();
    let mut split = LineSplit::new(fingerprint);
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
        if !visiting && !need_hash {
            return Some(None);
        }
    }
    let (hasher, _) = split
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
    Some(hasher.map(LineFingerprint::finish))
}
