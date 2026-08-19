//! Request-local line window for large UTF-8 reads.
//!
//! The scan walks the file once in 256 KiB chunks and keeps only the selected
//! window. Callers that need a full-file fingerprint supply a
//! [`LineFingerprint`]; untagged reads still finish the file so the footer can
//! report `of {total}`.

use std::path::Path;

use tokio::io::AsyncReadExt;

use super::{format_numbered_line, offset_past_end, window_footer};

#[cfg(test)]
use super::format_numbered_view;
use crate::document::MAX_DOCUMENT_INPUT_BYTES;
use crate::tool::ToolError;

/// Failures while scanning a UTF-8 window.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScanError {
    InvalidUtf8,
    Message(String),
}

impl From<String> for ScanError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl ScanError {
    fn into_tool_error(self, path: &Path) -> ToolError {
        match self {
            Self::InvalidUtf8 => ToolError::Message(format!(
                "could not read '{}' as UTF-8 text: invalid utf-8 sequence",
                path.display()
            )),
            Self::Message(message) => ToolError::Message(message),
        }
    }
}

/// Chunk size used for the sequential scan.
pub(crate) const CHUNK_SIZE: usize = 256 * 1024;

/// Optional per-`\n` digest collected while scanning.
///
/// `push_line` sees the same segments as `split('\n')`, including the empty
/// last segment after a trailing newline and the single empty segment of an
/// empty file.
pub(crate) trait LineFingerprint {
    fn push_line(&mut self, line: &[u8]);
    fn finish(self) -> String;
}

struct Utf8Check {
    pending: Vec<u8>,
}

impl Utf8Check {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    fn push(&mut self, chunk: &[u8]) -> Result<(), ()> {
        if self.pending.is_empty() {
            return self.take(chunk);
        }
        let needed = match self.pending.first().copied() {
            Some(first) => utf8_width(first).saturating_sub(self.pending.len()),
            None => 0,
        };
        if needed == 0 {
            return Err(());
        }
        let take = needed.min(chunk.len());
        self.pending.extend_from_slice(&chunk[..take]);
        if self.pending.len() < utf8_width(self.pending[0]) {
            return Ok(());
        }
        std::str::from_utf8(&self.pending).map_err(|_| ())?;
        self.pending.clear();
        self.take(&chunk[take..])
    }

    fn take(&mut self, bytes: &[u8]) -> Result<(), ()> {
        match std::str::from_utf8(bytes) {
            Ok(_) => Ok(()),
            Err(error) if error.error_len().is_none() => {
                let rest = &bytes[error.valid_up_to()..];
                if rest.len() > 3 {
                    return Err(());
                }
                self.pending.extend_from_slice(rest);
                Ok(())
            }
            Err(_) => Err(()),
        }
    }

    fn finish(&self) -> Result<(), ()> {
        if self.pending.is_empty() {
            Ok(())
        } else {
            Err(())
        }
    }
}

fn utf8_width(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first & 0xE0 == 0xC0 {
        2
    } else if first & 0xF0 == 0xE0 {
        3
    } else if first & 0xF8 == 0xF0 {
        4
    } else {
        0
    }
}

fn decode_line(bytes: &[u8]) -> Result<String, String> {
    let line = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    Ok(line.strip_suffix('\r').unwrap_or(line).to_string())
}

fn emit_window(header: &str, start: usize, lines: &[String], footer: Option<&str>) -> String {
    if lines.is_empty() {
        return header.to_string();
    }
    let mut out = header.to_string();
    out.push('\n');
    for (index, line) in lines.iter().enumerate() {
        out.push_str(&format_numbered_line(start + index, line));
        out.push('\n');
    }
    out.pop();
    if let Some(footer) = footer {
        out.push_str("\n\n");
        out.push_str(footer);
    }
    out
}

pub(crate) fn validate_window(
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<(usize, Option<usize>), String> {
    if offset == Some(0) {
        return Err("offset must be greater than 0".into());
    }
    if limit == Some(0) {
        return Err("limit must be greater than 0".into());
    }
    Ok((offset.unwrap_or(1), limit))
}

pub(crate) fn render_window(
    header: &str,
    total: usize,
    start: usize,
    limit: Option<usize>,
    lines: &[String],
) -> Result<String, String> {
    if total == 0 {
        if start > 1 {
            return Err(offset_past_end(start, 0));
        }
        return Ok(header.to_string());
    }
    if start > total {
        return Err(offset_past_end(start, total));
    }
    let end = match limit {
        Some(limit) => start.saturating_add(limit).saturating_sub(1).min(total),
        None => total,
    };
    let footer = window_footer(start, end, total);
    Ok(emit_window(header, start, lines, footer.as_deref()))
}

struct WindowScan<F> {
    fingerprint: Option<F>,
    utf8: Utf8Check,
    pending: Vec<u8>,
    line_number: usize,
    start: usize,
    want: Option<usize>,
    selected: Vec<String>,
    ends_with_newline: bool,
    bytes: usize,
}

impl<F: LineFingerprint> WindowScan<F> {
    fn new(start: usize, limit: Option<usize>, fingerprint: Option<F>) -> Self {
        Self {
            fingerprint,
            utf8: Utf8Check::new(),
            pending: Vec::new(),
            line_number: 1,
            start,
            want: limit,
            selected: Vec::new(),
            ends_with_newline: false,
            bytes: 0,
        }
    }

    fn push(&mut self, chunk: &[u8]) -> Result<(), ScanError> {
        self.utf8.push(chunk).map_err(|()| ScanError::InvalidUtf8)?;
        self.bytes = self.bytes.saturating_add(chunk.len());
        let mut rest = chunk;
        if !self.pending.is_empty() {
            match rest.iter().position(|&byte| byte == b'\n') {
                Some(index) => {
                    self.pending.extend_from_slice(&rest[..index]);
                    self.finish_content_line()?;
                    rest = &rest[index + 1..];
                    self.ends_with_newline = true;
                }
                None => {
                    self.pending.extend_from_slice(rest);
                    self.ends_with_newline = false;
                    return Ok(());
                }
            }
        }
        while let Some(index) = rest.iter().position(|&byte| byte == b'\n') {
            self.consume_line(&rest[..index], /*content*/ true)?;
            rest = &rest[index + 1..];
            self.ends_with_newline = true;
        }
        if !rest.is_empty() {
            self.pending.extend_from_slice(rest);
            self.ends_with_newline = false;
        }
        Ok(())
    }

    fn consume_line(&mut self, line: &[u8], content: bool) -> Result<(), ScanError> {
        if let Some(fingerprint) = &mut self.fingerprint {
            fingerprint.push_line(line);
        }
        if content && self.in_window() {
            self.selected.push(decode_line(line)?);
        }
        if content {
            self.line_number = self.line_number.saturating_add(1);
        }
        Ok(())
    }

    fn finish_content_line(&mut self) -> Result<(), ScanError> {
        let pending = std::mem::take(&mut self.pending);
        self.consume_line(&pending, /*content*/ true)
    }

    fn in_window(&self) -> bool {
        if self.line_number < self.start {
            return false;
        }
        match self.want {
            Some(limit) => self.selected.len() < limit,
            None => true,
        }
    }

    fn finish(mut self) -> Result<ScannedWindow, ScanError> {
        self.utf8.finish().map_err(|()| ScanError::InvalidUtf8)?;
        if !self.pending.is_empty() {
            self.finish_content_line()?;
        } else {
            // Empty file, or the empty last `split('\n')` segment after a
            // trailing newline. `pending` is empty only in those cases.
            self.consume_line(b"", /*content*/ false)?;
        }
        let total = if self.bytes == 0 {
            0
        } else {
            self.line_number.saturating_sub(1)
        };
        Ok(ScannedWindow {
            tag: self.fingerprint.map(LineFingerprint::finish),
            total,
            selected: self.selected,
        })
    }
}

pub(crate) struct ScannedWindow {
    pub(crate) tag: Option<String>,
    pub(crate) total: usize,
    pub(crate) selected: Vec<String>,
}

/// In-memory scan used by tests and the large-file oracle comparison.
#[cfg(test)]
pub(crate) fn format_window_bytes<F, H>(
    bytes: &[u8],
    offset: Option<usize>,
    limit: Option<usize>,
    fingerprint: Option<F>,
    header: H,
) -> Result<String, ScanError>
where
    F: LineFingerprint,
    H: FnOnce(Option<&str>) -> String,
{
    let (start, limit) = validate_window(offset, limit)?;
    if bytes.len() <= CHUNK_SIZE && fingerprint.is_none() {
        let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        return Ok(format_numbered_view(
            &header(None),
            text,
            Some(start),
            limit,
        )?);
    }
    let mut scan = WindowScan::new(start, limit, fingerprint);
    for chunk in bytes.chunks(CHUNK_SIZE) {
        scan.push(chunk)?;
    }
    let scanned = scan.finish()?;
    Ok(render_window(
        &header(scanned.tag.as_deref()),
        scanned.total,
        start,
        limit,
        &scanned.selected,
    )?)
}

/// Paginate a large on-disk UTF-8 file without retaining every prefix.
pub(crate) async fn read_text_window<F, H>(
    path: &Path,
    source_len: u64,
    offset: Option<usize>,
    limit: Option<usize>,
    fingerprint: Option<F>,
    header: H,
) -> Result<String, ToolError>
where
    F: LineFingerprint,
    H: FnOnce(Option<&str>) -> String,
{
    let (start, limit) = validate_window(offset, limit).map_err(ToolError::Message)?;
    if source_len > MAX_DOCUMENT_INPUT_BYTES as u64 {
        return Err(ToolError::Message(format!(
            "document '{}' is {source_len} bytes; the input limit is {MAX_DOCUMENT_INPUT_BYTES} bytes",
            path.display()
        )));
    }
    let mut file = tokio::fs::File::open(path).await?;
    let mut scan = WindowScan::new(start, limit, fingerprint);
    let mut buf = vec![0_u8; CHUNK_SIZE];
    loop {
        let read = file.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        let next = scan.bytes.saturating_add(read);
        if next > MAX_DOCUMENT_INPUT_BYTES {
            return Err(ToolError::Message(format!(
                "document '{}' is larger than the {MAX_DOCUMENT_INPUT_BYTES} byte input limit",
                path.display()
            )));
        }
        scan.push(&buf[..read])
            .map_err(|error| error.into_tool_error(path))?;
    }
    let scanned = scan.finish().map_err(|error| error.into_tool_error(path))?;
    render_window(
        &header(scanned.tag.as_deref()),
        scanned.total,
        start,
        limit,
        &scanned.selected,
    )
    .map_err(ToolError::Message)
}
