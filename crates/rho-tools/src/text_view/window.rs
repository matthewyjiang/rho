//! Request-local line window for large UTF-8 reads.
//!
//! The scan walks the file once in 256 KiB chunks and keeps only the selected
//! window. Callers that need a full-file fingerprint supply a
//! [`LineFingerprint`]; untagged reads still finish the file so the footer can
//! report `of {total}`.

use std::{io::Read, ops::ControlFlow, path::Path};

use tokio::io::AsyncReadExt;

use super::{
    format_numbered_line,
    line_split::{decode_content_line, LineFingerprint, LineSplit},
    offset_past_end, window_footer,
};

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
    decode_content_line(bytes)
        .map(str::to_string)
        .map_err(|()| "invalid utf-8 sequence".into())
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
    split: LineSplit<F>,
    utf8: Utf8Check,
    start: usize,
    want: Option<usize>,
    selected: Vec<String>,
}

impl<F: LineFingerprint> WindowScan<F> {
    fn new(start: usize, limit: Option<usize>, fingerprint: Option<F>) -> Self {
        Self {
            split: LineSplit::new(fingerprint),
            utf8: Utf8Check::new(),
            start,
            want: limit,
            selected: Vec::new(),
        }
    }

    fn bytes(&self) -> usize {
        self.split.bytes
    }

    fn push(&mut self, chunk: &[u8]) -> Result<(), ScanError> {
        self.utf8.push(chunk).map_err(|()| ScanError::InvalidUtf8)?;
        let start = self.start;
        let want = self.want;
        let selected = &mut self.selected;
        self.split.push(chunk, |line_number, line| {
            if in_window(line_number, start, want, selected.len()) {
                selected.push(decode_line(line)?);
            }
            Ok(())
        })
    }

    fn finish(self) -> Result<ScannedWindow, ScanError> {
        self.utf8.finish().map_err(|()| ScanError::InvalidUtf8)?;
        let mut selected = self.selected;
        let start = self.start;
        let want = self.want;
        let (fingerprint, total) = self.split.finish(|line_number, line| {
            if in_window(line_number, start, want, selected.len()) {
                selected.push(decode_line(line)?);
            }
            Ok::<(), ScanError>(())
        })?;
        Ok(ScannedWindow {
            tag: fingerprint.map(LineFingerprint::finish),
            total,
            selected,
        })
    }
}

fn in_window(line_number: usize, start: usize, want: Option<usize>, selected: usize) -> bool {
    line_number >= start
        && match want {
            Some(limit) => selected < limit,
            None => true,
        }
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
        let next = scan.bytes().saturating_add(read);
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
