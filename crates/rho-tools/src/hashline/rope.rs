//! Request-local line lookup for large UTF-8 reads.
//!
//! Rho still has to scan the whole file once: the hashline TAG fingerprints
//! every line. OpenCode's augmented rope exists because that reader can stop
//! after the selected window. Here the same 256 KiB chunks are hashed and
//! counted in one sequential pass, and only the selected window is retained.
//! There is no persistent index.
//!
//! Small files stay on the direct split path.
//!
//! Algorithm notes: Boehm, Atkinson, and Plass, "Ropes: An Alternative to
//! Strings"; generic order-statistic trees. Those structures skip unread
//! prefixes when a later page is the only work. Rho's TAG makes the unread
//! suffix required, so this module keeps the request-local summary idea and
//! drops the second seek pass.

use std::path::Path;

use tokio::io::AsyncReadExt;

#[cfg(test)]
use super::format::format_text_view;
use super::format::{
    format_file_hash, format_numbered_line, offset_past_end, trim_hash_line, view_header,
    window_footer, Fnv1a32,
};
use crate::document::MAX_DOCUMENT_INPUT_BYTES;
use crate::tool::ToolError;

/// Chunk size used for the sequential scan. Matches the OpenCode read-tool rope.
pub(crate) const CHUNK_SIZE: usize = 256 * 1024;

struct HashState {
    hasher: Fnv1a32,
    pending: Vec<u8>,
    started: bool,
}

impl HashState {
    fn new() -> Self {
        Self {
            hasher: Fnv1a32::new(),
            pending: Vec::new(),
            started: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        let mut rest = chunk;
        if !self.pending.is_empty() {
            match rest.iter().position(|&byte| byte == b'\n') {
                Some(index) => {
                    self.pending.extend_from_slice(&rest[..index]);
                    self.hash_pending();
                    rest = &rest[index + 1..];
                }
                None => {
                    self.pending.extend_from_slice(rest);
                    return;
                }
            }
        }
        while let Some(index) = rest.iter().position(|&byte| byte == b'\n') {
            self.hash_line(&rest[..index]);
            rest = &rest[index + 1..];
        }
        self.pending.extend_from_slice(rest);
    }

    fn hash_line(&mut self, line: &[u8]) {
        if self.started {
            self.hasher.write(b"\n");
        }
        self.started = true;
        self.hasher.write(trim_hash_line(line));
    }

    fn hash_pending(&mut self) {
        if self.started {
            self.hasher.write(b"\n");
        }
        self.started = true;
        self.hasher.write(trim_hash_line(&self.pending));
        self.pending.clear();
    }

    fn finish(mut self) -> String {
        // `split('\n')` always yields a last segment, including the empty one
        // after a trailing newline and the single empty segment of an empty file.
        self.hash_pending();
        format_file_hash(self.hasher.finish())
    }
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

fn validate_window(
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

fn render_window(
    display_path: &str,
    tag: Option<&str>,
    total: usize,
    start: usize,
    limit: Option<usize>,
    lines: &[String],
) -> Result<String, String> {
    if total == 0 {
        if start > 1 {
            return Err(offset_past_end(start, 0));
        }
        return Ok(view_header(display_path, tag.map(str::to_string)));
    }
    if start > total {
        return Err(offset_past_end(start, total));
    }
    let end = match limit {
        Some(limit) => start.saturating_add(limit).saturating_sub(1).min(total),
        None => total,
    };
    let header = view_header(display_path, tag.map(str::to_string));
    let footer = window_footer(start, end, total);
    Ok(emit_window(&header, start, lines, footer.as_deref()))
}

struct WindowScan {
    hasher: Option<HashState>,
    utf8: Utf8Check,
    pending: Vec<u8>,
    line_number: usize,
    start: usize,
    want: Option<usize>,
    selected: Vec<String>,
    ends_with_newline: bool,
    bytes: usize,
}

impl WindowScan {
    fn new(start: usize, limit: Option<usize>, mint_tag: bool) -> Self {
        Self {
            hasher: mint_tag.then(HashState::new),
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

    fn push(&mut self, chunk: &[u8]) -> Result<(), String> {
        self.utf8
            .push(chunk)
            .map_err(|()| "file is not valid UTF-8 text".to_string())?;
        if let Some(hasher) = &mut self.hasher {
            hasher.push(chunk);
        }
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
            if self.in_window() {
                self.selected.push(decode_line(&rest[..index])?);
            }
            self.line_number = self.line_number.saturating_add(1);
            rest = &rest[index + 1..];
            self.ends_with_newline = true;
        }
        if !rest.is_empty() {
            self.pending.extend_from_slice(rest);
            self.ends_with_newline = false;
        }
        Ok(())
    }

    fn finish_content_line(&mut self) -> Result<(), String> {
        if self.in_window() {
            self.selected.push(decode_line(&self.pending)?);
        }
        self.pending.clear();
        self.line_number = self.line_number.saturating_add(1);
        Ok(())
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

    fn finish(mut self) -> Result<ScannedWindow, String> {
        self.utf8
            .finish()
            .map_err(|()| "file is not valid UTF-8 text".to_string())?;
        if !self.pending.is_empty() || (self.bytes > 0 && !self.ends_with_newline) {
            if self.in_window() {
                self.selected.push(decode_line(&self.pending)?);
            }
            self.line_number = self.line_number.saturating_add(1);
        }
        let total = if self.bytes == 0 {
            0
        } else {
            self.line_number.saturating_sub(1)
        };
        Ok(ScannedWindow {
            tag: self.hasher.map(HashState::finish),
            total,
            selected: self.selected,
        })
    }
}

struct ScannedWindow {
    tag: Option<String>,
    total: usize,
    selected: Vec<String>,
}

#[cfg(test)]
fn scan_bytes(bytes: &[u8], start: usize, limit: Option<usize>) -> Result<ScannedWindow, String> {
    let mut scan = WindowScan::new(start, limit, /*mint_tag*/ true);
    for chunk in bytes.chunks(CHUNK_SIZE) {
        scan.push(chunk)?;
    }
    scan.finish()
}

/// In-memory scan used by tests and the large-file oracle comparison.
#[cfg(test)]
pub(super) fn format_hashline_view_bytes(
    display_path: &str,
    bytes: &[u8],
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, String> {
    let (start, limit) = validate_window(offset, limit)?;
    if bytes.len() <= CHUNK_SIZE {
        let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        return format_text_view(
            display_path,
            text,
            Some(start),
            limit,
            /*mint_tag*/ true,
        );
    }
    let scanned = scan_bytes(bytes, start, limit)?;
    render_window(
        display_path,
        scanned.tag.as_deref(),
        scanned.total,
        start,
        limit,
        &scanned.selected,
    )
}

/// Paginate a large on-disk UTF-8 file without retaining every prefix.
pub(crate) async fn read_hashline_window(
    path: &Path,
    display_path: &str,
    source_len: u64,
    offset: Option<usize>,
    limit: Option<usize>,
    mint_tag: bool,
) -> Result<String, ToolError> {
    let (start, limit) = validate_window(offset, limit).map_err(ToolError::Message)?;
    if source_len > MAX_DOCUMENT_INPUT_BYTES as u64 {
        return Err(ToolError::Message(format!(
            "document '{}' is {source_len} bytes; the input limit is {MAX_DOCUMENT_INPUT_BYTES} bytes",
            path.display()
        )));
    }
    let mut file = tokio::fs::File::open(path).await?;
    let mut scan = WindowScan::new(start, limit, mint_tag);
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
        scan.push(&buf[..read]).map_err(|error| {
            if error == "file is not valid UTF-8 text" {
                ToolError::Message(format!(
                    "could not read '{}' as UTF-8 text: invalid utf-8",
                    path.display()
                ))
            } else {
                ToolError::Message(error)
            }
        })?;
    }
    let scanned = scan.finish().map_err(|error| {
        if error == "file is not valid UTF-8 text" {
            ToolError::Message(format!(
                "could not read '{}' as UTF-8 text: invalid utf-8",
                path.display()
            ))
        } else {
            ToolError::Message(error)
        }
    })?;
    render_window(
        display_path,
        scanned.tag.as_deref(),
        scanned.total,
        start,
        limit,
        &scanned.selected,
    )
    .map_err(ToolError::Message)
}

#[cfg(test)]
#[path = "rope_tests.rs"]
mod tests;
